//! The ordered core -> page event pump (spec §9, §14.1, §14.6).
//!
//! * `emit` never blocks (a short mutex section, no I/O) — it is called from
//!   the session actor and from window callbacks.
//! * Every accepted event gets a strictly increasing `seq` (per process, never
//!   reset — not by attach, detach or delivery failure).
//! * ONE worker thread delivers envelopes in queue order to the attached
//!   [`Delivery`] target (production: the page's Tauri `Channel`).
//! * Backpressure: the queue is soft-bounded. Over the bound only
//!   `audio:level` events are dropped/coalesced (latest-wins per session).
//!   Must-deliver events (terminal, protection, core, …) and ordered content
//!   (`stt:partial`, `llm:delta`) are kept. A very large hard cap
//!   ([`PumpConfig::hard_cap`]) exists only so a page that never re-attaches
//!   cannot grow memory forever; past it the OLDEST non-must-deliver events
//!   are dropped (the page re-syncs via `get_status` when it attaches).
//! * `attach` replaces the target (page reload re-subscribes). A delivery
//!   failure marks the target detached and puts the envelope back at the head
//!   of the queue, so nothing is lost; the next `attach` flushes it. There is
//!   deliberately NO global failure counter that could ratchet shut (§14.6).

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use callcore_contract::ports::EventSink;
use callcore_contract::{CoreEvent, EventEnvelope};

/// Where envelopes go. Production: the webview's Tauri Channel. Must not
/// block for long (a Channel send only serializes and posts to the webview).
pub trait Delivery: Send + Sync {
    fn deliver(&self, env: &EventEnvelope) -> Result<(), DeliveryError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryError(pub String);

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "event delivery failed: {}", self.0)
    }
}

impl std::error::Error for DeliveryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpConfig {
    /// Soft bound while a target is attached. Above it, levels are dropped.
    pub attached_capacity: usize,
    /// Soft bound while no target is attached (buffering for the next attach).
    pub detached_capacity: usize,
    /// Absolute bound; past it the oldest non-must-deliver events are dropped.
    pub hard_cap: usize,
}

impl Default for PumpConfig {
    fn default() -> Self {
        Self {
            attached_capacity: 1_024,
            detached_capacity: 4_096,
            hard_cap: 50_000,
        }
    }
}

/// Counters for diagnostics. Monotonic; never used to make decisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PumpStats {
    pub emitted: u64,
    pub delivered: u64,
    pub coalesced_levels: u64,
    pub dropped_levels: u64,
    pub dropped_overflow: u64,
    pub delivery_failures: u64,
    pub attaches: u64,
    pub queued: usize,
    pub attached: bool,
}

struct Queue {
    items: VecDeque<EventEnvelope>,
    next_seq: u64,
    generation: u64,
    target: Option<(u64, Arc<dyn Delivery>)>,
    in_flight: bool,
    closed: bool,
    stats: PumpStats,
}

struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
    config: PumpConfig,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        // A poisoned lock only means a panic happened while holding it; the
        // queue itself is still structurally valid, so keep going (§14.6: the
        // pump must always recover).
        self.q.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// See the module docs. Wrap in `Arc` and hand out as `Arc<dyn EventSink>`.
pub struct EventPump {
    shared: Arc<Shared>,
}

impl EventPump {
    pub fn new() -> Self {
        Self::with_config(PumpConfig::default())
    }

    pub fn with_config(config: PumpConfig) -> Self {
        let shared = Arc::new(Shared {
            q: Mutex::new(Queue {
                items: VecDeque::new(),
                next_seq: 1,
                generation: 0,
                target: None,
                in_flight: false,
                closed: false,
                stats: PumpStats::default(),
            }),
            cv: Condvar::new(),
            config,
        });
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("event-pump".into())
            .spawn(move || worker_loop(&worker))
            .expect("spawn event-pump thread");
        Self { shared }
    }

    /// Attach (or replace) the delivery target. Buffered events are flushed to
    /// it in order. Returns the attach generation.
    pub fn attach(&self, delivery: Arc<dyn Delivery>) -> u64 {
        let mut q = self.shared.lock();
        q.generation += 1;
        let gen = q.generation;
        q.target = Some((gen, delivery));
        q.stats.attaches += 1;
        drop(q);
        self.shared.cv.notify_all();
        gen
    }

    /// Drop the current target; events buffer until the next `attach`.
    pub fn detach(&self) {
        let mut q = self.shared.lock();
        q.target = None;
        drop(q);
        self.shared.cv.notify_all();
    }

    pub fn is_attached(&self) -> bool {
        self.shared.lock().target.is_some()
    }

    pub fn stats(&self) -> PumpStats {
        let q = self.shared.lock();
        let mut s = q.stats;
        s.queued = q.items.len();
        s.attached = q.target.is_some();
        s
    }

    /// Block until everything queued has been delivered (queue empty and no
    /// delivery in flight), or `timeout` passes. Returns true when idle.
    /// Used by tests and by shutdown; never called on the async runtime.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut q = self.shared.lock();
        loop {
            if q.items.is_empty() && !q.in_flight {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            q = self
                .shared
                .cv
                .wait_timeout(q, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    fn push(&self, event: CoreEvent) {
        let cfg = self.shared.config;
        let mut q = self.shared.lock();
        q.stats.emitted += 1;

        if let CoreEvent::AudioLevel { session_id, .. } = &event {
            // Latest-wins: replace a still-queued level of the same session in
            // place (it keeps its seq, so seq order == queue order holds).
            let existing = q.items.iter_mut().rev().find(|e| {
                matches!(&e.event, CoreEvent::AudioLevel { session_id: s, .. } if s == session_id)
            });
            if let Some(slot) = existing {
                slot.event = event;
                q.stats.coalesced_levels += 1;
                return;
            }
        }

        let soft = if q.target.is_some() {
            cfg.attached_capacity
        } else {
            cfg.detached_capacity
        };
        if q.items.len() >= soft {
            if event.is_coalescable() {
                q.stats.dropped_levels += 1;
                return;
            }
            if let Some(pos) = q.items.iter().position(|e| e.event.is_coalescable()) {
                q.items.remove(pos);
                q.stats.dropped_levels += 1;
            }
        }
        if q.items.len() >= cfg.hard_cap {
            if let Some(pos) = q.items.iter().position(|e| !e.event.is_must_deliver()) {
                q.items.remove(pos);
                q.stats.dropped_overflow += 1;
            }
        }

        let seq = q.next_seq;
        q.next_seq += 1;
        q.items.push_back(EventEnvelope { seq, event });
        drop(q);
        self.shared.cv.notify_all();
    }
}

impl Default for EventPump {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EventPump {
    fn drop(&mut self) {
        let mut q = self.shared.lock();
        q.closed = true;
        drop(q);
        self.shared.cv.notify_all();
    }
}

impl EventSink for EventPump {
    fn emit(&self, event: CoreEvent) {
        self.push(event);
    }
}

fn worker_loop(shared: &Shared) {
    loop {
        let mut q = shared.lock();
        while !q.closed && (q.items.is_empty() || q.target.is_none()) {
            q = shared.cv.wait(q).unwrap_or_else(|p| p.into_inner());
        }
        if q.closed {
            return;
        }
        let Some(env) = q.items.pop_front() else {
            continue;
        };
        let Some((gen, target)) = q.target.clone() else {
            q.items.push_front(env);
            continue;
        };
        q.in_flight = true;
        drop(q);

        let result = catch_unwind(AssertUnwindSafe(|| target.deliver(&env)));
        drop(target);

        let mut q = shared.lock();
        q.in_flight = false;
        match result {
            Ok(Ok(())) => q.stats.delivered += 1,
            Ok(Err(e)) => {
                tracing::warn!(seq = env.seq, error = %e, "event delivery failed; target detached");
                failed(&mut q, gen, env);
            }
            Err(_) => {
                tracing::warn!(seq = env.seq, "event delivery panicked; target detached");
                failed(&mut q, gen, env);
            }
        }
        drop(q);
        shared.cv.notify_all();
    }
}

fn failed(q: &mut Queue, gen: u64, env: EventEnvelope) {
    q.stats.delivery_failures += 1;
    // Only detach the target that actually failed; a newer attach stays.
    if q.target.as_ref().map(|(g, _)| *g) == Some(gen) {
        q.target = None;
    }
    q.items.push_front(env);
}

#[cfg(test)]
mod tests {
    use super::*;
    use callcore_contract::{AppError, Finish, Metrics, Protection, SessionId};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const WAIT: Duration = Duration::from_secs(10);

    /// Fake webview Channel: records envelopes, can fail N times, can be gated
    /// shut to simulate a stalled renderer.
    #[derive(Default)]
    struct Recorder {
        got: Mutex<Vec<EventEnvelope>>,
        fail_next: AtomicUsize,
        gate_closed: Mutex<bool>,
        gate_cv: Condvar,
    }

    impl Recorder {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn close_gate(&self) {
            *self.gate_closed.lock().unwrap() = true;
        }
        fn open_gate(&self) {
            *self.gate_closed.lock().unwrap() = false;
            self.gate_cv.notify_all();
        }
        fn got(&self) -> Vec<EventEnvelope> {
            self.got.lock().unwrap().clone()
        }
    }

    impl Delivery for Recorder {
        fn deliver(&self, env: &EventEnvelope) -> Result<(), DeliveryError> {
            let mut closed = self.gate_closed.lock().unwrap();
            while *closed {
                closed = self.gate_cv.wait(closed).unwrap();
            }
            drop(closed);
            if self
                .fail_next
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(DeliveryError("webview gone".into()));
            }
            self.got.lock().unwrap().push(env.clone());
            Ok(())
        }
    }

    fn sid(s: &str) -> SessionId {
        SessionId(s.into())
    }
    fn delta(s: &str, d: &str) -> CoreEvent {
        CoreEvent::LlmDelta {
            session_id: sid(s),
            delta: d.into(),
        }
    }
    fn level(s: &str, rms: f32) -> CoreEvent {
        CoreEvent::AudioLevel {
            session_id: sid(s),
            rms,
        }
    }
    fn done(s: &str) -> CoreEvent {
        CoreEvent::LlmDone {
            session_id: sid(s),
            transcript: "q".into(),
            answer: "a".into(),
            finish: Finish::Complete,
            call_type: Default::default(),
            metrics: Metrics::default(),
        }
    }
    fn assert_strictly_increasing(envs: &[EventEnvelope]) {
        for w in envs.windows(2) {
            assert!(
                w[0].seq < w[1].seq,
                "seq not increasing: {} then {}",
                w[0].seq,
                w[1].seq
            );
        }
    }
    fn deltas_of(envs: &[EventEnvelope]) -> Vec<String> {
        envs.iter()
            .filter_map(|e| match &e.event {
                CoreEvent::LlmDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn delivers_in_order_with_increasing_seq() {
        let pump = EventPump::new();
        let rec = Recorder::new();
        pump.attach(rec.clone());
        for i in 0..200 {
            pump.emit(delta("s1", &i.to_string()));
        }
        pump.emit(done("s1"));
        assert!(pump.wait_idle(WAIT));
        let got = rec.got();
        assert_eq!(got.len(), 201);
        assert_strictly_increasing(&got);
        assert_eq!(got[0].seq, 1);
        let expected: Vec<String> = (0..200).map(|i| i.to_string()).collect();
        assert_eq!(deltas_of(&got), expected);
        assert!(matches!(got[200].event, CoreEvent::LlmDone { .. }));
    }

    #[test]
    fn buffers_while_detached_and_flushes_on_attach() {
        let pump = EventPump::new();
        pump.emit(CoreEvent::CoreReady { revision: 1 });
        pump.emit(delta("s1", "a"));
        // Nothing attached: the worker never pops, so both stay queued.
        assert_eq!(pump.stats().queued, 2);
        assert!(!pump.stats().attached);
        let rec = Recorder::new();
        pump.attach(rec.clone());
        assert!(pump.wait_idle(WAIT));
        let got = rec.got();
        assert_eq!(got.len(), 2);
        assert!(matches!(got[0].event, CoreEvent::CoreReady { .. }));
    }

    #[test]
    fn seq_is_monotonic_across_attach_and_detach() {
        let pump = EventPump::new();
        let a = Recorder::new();
        pump.attach(a.clone());
        pump.emit(delta("s1", "1"));
        assert!(pump.wait_idle(WAIT));
        pump.detach();
        pump.emit(delta("s1", "2"));
        let b = Recorder::new();
        pump.attach(b.clone());
        pump.emit(delta("s1", "3"));
        assert!(pump.wait_idle(WAIT));
        let mut all = a.got();
        all.extend(b.got());
        assert_strictly_increasing(&all);
        assert_eq!(deltas_of(&all), vec!["1", "2", "3"]);
    }

    #[test]
    fn levels_coalesce_latest_wins_per_session() {
        let pump = EventPump::new();
        // Detached: everything queues, so coalescing is deterministic.
        for i in 0..50 {
            pump.emit(level("s1", i as f32 / 100.0));
        }
        pump.emit(level("s2", 0.9));
        pump.emit(level("s1", 0.77));
        let rec = Recorder::new();
        pump.attach(rec.clone());
        assert!(pump.wait_idle(WAIT));
        let got = rec.got();
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].event, level("s1", 0.77));
        assert_eq!(got[1].event, level("s2", 0.9));
        assert_eq!(pump.stats().coalesced_levels, 50);
    }

    #[test]
    fn must_deliver_and_deltas_survive_saturation() {
        let pump = EventPump::with_config(PumpConfig {
            attached_capacity: 16,
            detached_capacity: 16,
            hard_cap: 100_000,
        });
        let rec = Recorder::new();
        rec.close_gate(); // renderer stalled
        pump.attach(rec.clone());
        let mut want_deltas = Vec::new();
        for i in 0..2_000 {
            // Levels from many sessions defeat coalescing, forcing drops.
            pump.emit(level(&format!("x{i}"), 0.5));
            let d = format!("d{i}");
            pump.emit(delta("s1", &d));
            want_deltas.push(d);
            if i % 100 == 0 {
                pump.emit(CoreEvent::ProtectionOk {
                    revision: i,
                    protection: Protection::Protected,
                });
            }
        }
        pump.emit(CoreEvent::SessionError {
            session_id: sid("s1"),
            error: AppError::internal("x"),
        });
        rec.open_gate();
        assert!(pump.wait_idle(WAIT));
        let got = rec.got();
        assert_strictly_increasing(&got);
        assert_eq!(deltas_of(&got), want_deltas, "no delta may be dropped");
        let prot = got
            .iter()
            .filter(|e| matches!(e.event, CoreEvent::ProtectionOk { .. }))
            .count();
        assert_eq!(prot, 20);
        assert!(matches!(
            got.last().unwrap().event,
            CoreEvent::SessionError { .. }
        ));
        assert!(
            pump.stats().dropped_levels > 0,
            "levels are the pressure valve"
        );
    }

    #[test]
    fn delivery_failure_detaches_and_keeps_the_event() {
        let pump = EventPump::new();
        let a = Recorder::new();
        a.fail_next.store(1, Ordering::SeqCst);
        pump.attach(a.clone());
        pump.emit(done("s1"));
        // Wait for the failure to detach the target.
        let deadline = Instant::now() + WAIT;
        while pump.is_attached() {
            assert!(Instant::now() < deadline, "target never detached");
            std::thread::yield_now();
        }
        assert!(a.got().is_empty());
        pump.emit(delta("s2", "later"));
        let b = Recorder::new();
        pump.attach(b.clone());
        assert!(pump.wait_idle(WAIT));
        let got = b.got();
        assert_eq!(got.len(), 2);
        assert!(
            matches!(got[0].event, CoreEvent::LlmDone { .. }),
            "failed event re-delivered first"
        );
        assert_strictly_increasing(&got);
    }

    #[test]
    fn recovers_after_many_failures_and_reattaches() {
        // v3 regression (§14.6): a global counter ratcheted shut after two
        // renderer crashes. Here 25 failing reattaches must not matter.
        let pump = EventPump::new();
        for round in 0..25u64 {
            let bad = Recorder::new();
            bad.fail_next.store(usize::MAX, Ordering::SeqCst);
            pump.attach(bad.clone());
            pump.emit(CoreEvent::CoreReady { revision: round });
            let deadline = Instant::now() + WAIT;
            while pump.is_attached() {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        let good = Recorder::new();
        pump.attach(good.clone());
        pump.emit(done("s9"));
        assert!(pump.wait_idle(WAIT));
        let got = good.got();
        assert_eq!(got.len(), 26, "all buffered events + the new one arrive");
        assert_strictly_increasing(&got);
        assert!(pump.stats().delivery_failures >= 25);
    }

    #[test]
    fn a_panicking_delivery_is_treated_as_a_failure() {
        struct Panicker;
        impl Delivery for Panicker {
            fn deliver(&self, _env: &EventEnvelope) -> Result<(), DeliveryError> {
                panic!("renderer exploded");
            }
        }
        let pump = EventPump::new();
        pump.attach(Arc::new(Panicker));
        pump.emit(done("s1"));
        let deadline = Instant::now() + WAIT;
        while pump.is_attached() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let rec = Recorder::new();
        pump.attach(rec.clone());
        assert!(pump.wait_idle(WAIT));
        assert_eq!(rec.got().len(), 1);
    }

    #[test]
    fn detached_buffer_drops_levels_first() {
        let pump = EventPump::with_config(PumpConfig {
            attached_capacity: 8,
            detached_capacity: 8,
            hard_cap: 1_000,
        });
        for i in 0..8 {
            pump.emit(level(&format!("x{i}"), 0.1));
        }
        for i in 0..8 {
            pump.emit(delta("s1", &i.to_string()));
        }
        pump.emit(level("late", 0.3)); // queue full of deltas now -> dropped
        let rec = Recorder::new();
        pump.attach(rec.clone());
        assert!(pump.wait_idle(WAIT));
        let got = rec.got();
        assert_eq!(deltas_of(&got).len(), 8);
        assert!(got.iter().all(|e| !e.event.is_coalescable()), "{got:?}");
    }

    #[test]
    fn hard_cap_never_drops_must_deliver() {
        let pump = EventPump::with_config(PumpConfig {
            attached_capacity: 4,
            detached_capacity: 4,
            hard_cap: 10,
        });
        for i in 0..10 {
            pump.emit(done(&format!("s{i}")));
        }
        for i in 0..50 {
            pump.emit(delta("z", &i.to_string()));
        }
        pump.emit(done("last"));
        let rec = Recorder::new();
        pump.attach(rec.clone());
        assert!(pump.wait_idle(WAIT));
        let got = rec.got();
        let dones = got
            .iter()
            .filter(|e| matches!(e.event, CoreEvent::LlmDone { .. }))
            .count();
        assert_eq!(dones, 11);
        assert_strictly_increasing(&got);
    }

    #[test]
    fn replacing_the_target_routes_new_events_to_the_new_one() {
        let pump = EventPump::new();
        let a = Recorder::new();
        let b = Recorder::new();
        pump.attach(a.clone());
        pump.emit(delta("s1", "1"));
        assert!(pump.wait_idle(WAIT));
        pump.attach(b.clone());
        pump.emit(delta("s1", "2"));
        assert!(pump.wait_idle(WAIT));
        assert_eq!(deltas_of(&a.got()), vec!["1"]);
        assert_eq!(deltas_of(&b.got()), vec!["2"]);
    }

    #[test]
    fn emit_does_not_block_while_the_renderer_is_stalled() {
        let pump = EventPump::new();
        let rec = Recorder::new();
        rec.close_gate();
        pump.attach(rec.clone());
        let t0 = Instant::now();
        for i in 0..10_000 {
            pump.emit(delta("s1", &i.to_string()));
        }
        // Generous bound: 10k pushes of a short mutex section.
        assert!(t0.elapsed() < Duration::from_secs(5));
        rec.open_gate();
        assert!(pump.wait_idle(WAIT));
        assert_eq!(rec.got().len(), 10_000);
    }
}
