//! Latency benchmark harness (spec §15): drives a REAL `SessionHandle` through
//! N record -> stop -> answer cycles against fake audio / STT / provider ports
//! that inject known delays, on the real (not paused) clock, and prints
//! p50/p90 of the four `llm:done` metrics.
//!
//! It also checks the metric definitions end to end (the latency clock starts
//! at Stop acceptance, BEFORE the drain):
//!
//! * check 1 (pass/fail): p50 of `firstTokenMs - audioDrainMs - sttFinalizeMs
//!   - <measured provider wait>` — the session's own overhead — is within the
//!   tolerance. Robust to the OS oversleeping the fakes' delays.
//! * check 2 (warning only): p50 `firstTokenMs` is within the tolerance of the
//!   CONFIGURED drain + finalize + first-token delays. Passes on an idle
//!   machine; on a loaded one the fakes oversleep and it only warns.
//!
//! No network, no audio device, no API key.
//!
//! ```text
//! export CARGO_TARGET_DIR=target-core
//! cargo run -p callcore-session --example latency_bench --release
//! cargo run -p callcore-session --example latency_bench --release -- \
//!     --cycles 50 --drain 40 --finalize 150 --first-token 300 --delta 20 --deltas 10 --tolerance 15
//! ```
//!
//! Exit code 0 when every cycle succeeded and check 1 passed, 1 otherwise.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use callcore_contract::config::SessionTimeouts;
use callcore_contract::ports::{
    AnswerConfig, AnswerProvider, AudioError, AudioFrame, AudioMsg, AudioSource, DrainReport,
    EventSink, FrameSink, HeaderValue, PreparedRequest, PromptParts, ProviderFailure,
    ProviderRegistry, SecretReadError, SettingsReader, StreamOutcome, SttConnection, SttConnector,
    SttEvent, SttFailure, SttSender, SystemClock,
};
use callcore_contract::{
    AnswerStyle, CallType, CoreEvent, Finish, Metrics, Profile, Secret, SessionId,
};
use callcore_session::{SessionDeps, SessionHandle};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Instant};

/// Sleep for exactly `d`. Windows' default timer tick is ~15.6 ms, so a plain
/// `tokio::time::sleep(20ms)` routinely lasts ~31 ms. The fakes must inject
/// EXACT delays so that whatever the metrics show above them is the session's
/// own overhead: coarse-sleep to within one tick, then yield-spin.
async fn precise_sleep(d: Duration) {
    const TICK: Duration = Duration::from_millis(20);
    let target = Instant::now() + d;
    if d > TICK {
        sleep(d - TICK).await;
    }
    while Instant::now() < target {
        tokio::task::yield_now().await;
    }
}

// ───────────────────────────── config ─────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Config {
    cycles: usize,
    /// Injected `stop_and_drain` duration.
    drain_ms: u64,
    /// CloseStream -> `Flushed` delay.
    finalize_ms: u64,
    /// Request start -> first delta.
    first_token_ms: u64,
    /// Gap between later deltas.
    delta_ms: u64,
    deltas: usize,
    /// How long each cycle "records" before Stop (the socket is open by then).
    record_ms: u64,
    /// Allowed |p50(firstTokenMs) - expected| in ms.
    tolerance_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cycles: 20,
            drain_ms: 40,
            finalize_ms: 150,
            first_token_ms: 300,
            delta_ms: 20,
            deltas: 10,
            record_ms: 300,
            tolerance_ms: 15,
        }
    }
}

fn parse_args() -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            return Err(String::new());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let n: u64 = value
            .parse()
            .map_err(|_| format!("{flag}: not a number: {value}"))?;
        match flag.as_str() {
            "--cycles" => cfg.cycles = n.max(1) as usize,
            "--drain" => cfg.drain_ms = n,
            "--finalize" => cfg.finalize_ms = n,
            "--first-token" => cfg.first_token_ms = n,
            "--delta" => cfg.delta_ms = n,
            "--deltas" => cfg.deltas = n.max(1) as usize,
            "--record" => cfg.record_ms = n,
            "--tolerance" => cfg.tolerance_ms = n,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(cfg)
}

const USAGE: &str = "usage: latency_bench [--cycles N] [--drain MS] [--finalize MS] \
[--first-token MS] [--delta MS] [--deltas N] [--record MS] [--tolerance MS]";

// ───────────────────────────── fake audio ─────────────────────────────

fn frame() -> AudioFrame {
    AudioFrame {
        samples: vec![100; 2048],
        rms: 0.2,
    }
}

/// Pushes a few frames on start; the drain takes `drain` and then delivers a
/// final partial frame, like the real worker.
struct BenchAudio {
    drain: Duration,
    sink: Mutex<Option<FrameSink>>,
}

#[async_trait]
impl AudioSource for BenchAudio {
    async fn start(&self, sink: FrameSink) -> Result<(), AudioError> {
        for _ in 0..3 {
            let _ = sink.send(AudioMsg::Frame(frame()));
        }
        *self.sink.lock().unwrap() = Some(sink);
        Ok(())
    }

    async fn stop_discard(&self) {
        self.sink.lock().unwrap().take();
    }

    async fn stop_and_drain(&self, limit: Duration) -> Result<DrainReport, AudioError> {
        precise_sleep(self.drain.min(limit)).await;
        if let Some(sink) = self.sink.lock().unwrap().take() {
            let _ = sink.send(AudioMsg::Frame(AudioFrame {
                samples: vec![100; 512],
                rms: 0.2,
            }));
        }
        Ok(DrainReport {
            frames_flushed: 1,
            timed_out: self.drain > limit,
        })
    }
}

// ───────────────────────────── fake STT ─────────────────────────────

const TRANSCRIPT: &str = "Tell me about a time you led a project under a tight deadline.";

struct BenchStt {
    finalize: Duration,
}

struct BenchSender {
    events: mpsc::Sender<SttEvent>,
    finalize: Duration,
    frames: usize,
}

#[async_trait]
impl SttConnector for BenchStt {
    async fn connect(&self, _key: &Secret) -> Result<SttConnection, SttFailure> {
        let (tx, rx) = mpsc::channel(64);
        Ok(SttConnection {
            sender: Box::new(BenchSender {
                events: tx,
                finalize: self.finalize,
                frames: 0,
            }),
            events: rx,
        })
    }
}

#[async_trait]
impl SttSender for BenchSender {
    async fn send_audio(&mut self, _frame: &AudioFrame) -> Result<(), SttFailure> {
        self.frames += 1;
        if self.frames == 2 {
            let _ = self.events.try_send(SttEvent::Transcript {
                text: TRANSCRIPT.to_string(),
                is_final: false,
            });
        }
        Ok(())
    }

    async fn close_stream(&mut self) -> Result<(), SttFailure> {
        let events = self.events.clone();
        let delay = self.finalize;
        tokio::spawn(async move {
            precise_sleep(delay).await;
            let _ = events
                .send(SttEvent::Flushed {
                    transcript: TRANSCRIPT.to_string(),
                })
                .await;
        });
        Ok(())
    }

    fn abort(&mut self) {}
}

// ───────────────────────────── fake provider ─────────────────────────────

struct BenchProvider {
    first_token: Duration,
    per_delta: Duration,
    deltas: usize,
    /// Measured stream-start -> first-delta time of the latest stream (ms),
    /// i.e. what the fake REALLY waited (a loaded machine oversleeps).
    last_first_token_ms: Mutex<Option<u32>>,
}

#[async_trait]
impl AnswerProvider for BenchProvider {
    fn id(&self) -> &'static str {
        "bench"
    }
    fn display_name(&self) -> &'static str {
        "Bench (fake)"
    }
    fn key_id(&self) -> &'static str {
        "bench"
    }
    fn model(&self) -> &'static str {
        "bench-model"
    }

    fn build_request(&self, prompt: &PromptParts, _key: &Secret) -> PreparedRequest {
        PreparedRequest {
            url: "http://127.0.0.1:9/bench".into(),
            headers: vec![("x-bench".into(), HeaderValue::Plain("1".into()))],
            body: bytes::Bytes::from(prompt.user_message.clone()),
        }
    }

    async fn stream(
        &self,
        _req: &PreparedRequest,
        deltas: mpsc::UnboundedSender<String>,
    ) -> Result<StreamOutcome, ProviderFailure> {
        let started = Instant::now();
        let mut answer = String::new();
        for i in 0..self.deltas {
            precise_sleep(if i == 0 {
                self.first_token
            } else {
                self.per_delta
            })
            .await;
            let piece = format!("word{i} ");
            answer.push_str(&piece);
            if i == 0 {
                let ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
                *self.last_first_token_ms.lock().unwrap() = Some(ms);
            }
            let _ = deltas.send(piece);
        }
        Ok(StreamOutcome {
            finish: Finish::Complete,
            answer,
        })
    }

    fn prewarm(&self) {}
}

struct BenchRegistry(Arc<dyn AnswerProvider>);

impl ProviderRegistry for BenchRegistry {
    fn get(&self, id: &str) -> Option<Arc<dyn AnswerProvider>> {
        (id == self.0.id()).then(|| self.0.clone())
    }
    fn default_provider(&self) -> Arc<dyn AnswerProvider> {
        self.0.clone()
    }
    fn all(&self) -> Vec<Arc<dyn AnswerProvider>> {
        vec![self.0.clone()]
    }
}

// ───────────────────────────── settings + sink ─────────────────────────────

struct BenchSettings;

impl SettingsReader for BenchSettings {
    fn answer_config(&self) -> AnswerConfig {
        AnswerConfig {
            profile: Profile {
                id: "default".into(),
                name: "Default".into(),
                call_type: CallType::Behavioral,
                focus: String::new(),
                resume: "Led a team of five engineers.".into(),
                job_description: String::new(),
                notes: String::new(),
            },
            call_type: CallType::Behavioral,
            style: AnswerStyle::Balanced,
            provider_id: "bench".into(),
        }
    }

    fn get_secret(&self, _key_id: &str) -> Result<Option<Secret>, SecretReadError> {
        Ok(Some(Secret::new("bench-key")))
    }
}

struct ChannelSink(mpsc::UnboundedSender<CoreEvent>);

impl EventSink for ChannelSink {
    fn emit(&self, event: CoreEvent) {
        let _ = self.0.send(event);
    }
}

// ───────────────────────────── run ─────────────────────────────

async fn wait_for<F>(
    rx: &mut mpsc::UnboundedReceiver<CoreEvent>,
    id: &SessionId,
    mut pick: F,
) -> Result<CoreEvent, String>
where
    F: FnMut(&CoreEvent) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ev = timeout(
            deadline.saturating_duration_since(Instant::now()),
            rx.recv(),
        )
        .await
        .map_err(|_| format!("{id}: timed out waiting for an event"))?
        .ok_or("event channel closed")?;
        if ev.session_id() != Some(id) {
            continue;
        }
        if let CoreEvent::SessionError { error, .. } = &ev {
            return Err(format!(
                "{id}: session:error {:?}: {}",
                error.code, error.message
            ));
        }
        if pick(&ev) {
            return Ok(ev);
        }
    }
}

/// One cycle's `llm:done` metrics plus the provider's measured first-token wait.
struct Sample {
    metrics: Metrics,
    provider_first_token_ms: u32,
}

impl Sample {
    /// firstTokenMs minus every wait the fakes actually performed on the
    /// Stop -> first-token path: the session's own overhead. If the latency
    /// clock started after the drain, this would be about -audioDrainMs.
    fn overhead_ms(&self) -> i64 {
        let m = &self.metrics;
        i64::from(m.first_token_ms)
            - i64::from(m.audio_drain_ms)
            - i64::from(m.stt_finalize_ms)
            - i64::from(self.provider_first_token_ms)
    }
}

async fn one_cycle(
    session: &SessionHandle,
    provider: &BenchProvider,
    rx: &mut mpsc::UnboundedReceiver<CoreEvent>,
    record: Duration,
) -> Result<Sample, String> {
    provider.last_first_token_ms.lock().unwrap().take();
    let id = session
        .start_session()
        .await
        .map_err(|e| format!("start_session: {}", e.message))?;
    wait_for(rx, &id, |e| matches!(e, CoreEvent::SessionRecording { .. })).await?;
    // "Record" for a while; the STT socket is open well before Stop, so the
    // drain starts at Stop acceptance.
    sleep(record).await;
    session
        .stop_session(&id)
        .await
        .map_err(|e| format!("stop_session: {}", e.message))?;
    match wait_for(rx, &id, |e| matches!(e, CoreEvent::LlmDone { .. })).await? {
        CoreEvent::LlmDone { metrics, .. } => Ok(Sample {
            metrics,
            provider_first_token_ms: provider
                .last_first_token_ms
                .lock()
                .unwrap()
                .ok_or("provider never sent a delta")?,
        }),
        _ => unreachable!(),
    }
}

/// Picks one metric out of an `llm:done` payload.
type MetricFn = fn(&Metrics) -> u32;

fn percentile(sorted: &[u32], p: f64) -> u32 {
    // Nearest-rank.
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn stats(values: impl Iterator<Item = u32>) -> (u32, u32, u32, u32) {
    let mut v: Vec<u32> = values.collect();
    v.sort_unstable();
    (
        percentile(&v, 50.0),
        percentile(&v, 90.0),
        v[0],
        v[v.len() - 1],
    )
}

#[tokio::main]
async fn main() {
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("{msg}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let bench_provider = Arc::new(BenchProvider {
        first_token: Duration::from_millis(cfg.first_token_ms),
        per_delta: Duration::from_millis(cfg.delta_ms),
        deltas: cfg.deltas,
        last_first_token_ms: Mutex::new(None),
    });
    let provider: Arc<dyn AnswerProvider> = bench_provider.clone();
    let session = SessionHandle::spawn(SessionDeps {
        audio: Arc::new(BenchAudio {
            drain: Duration::from_millis(cfg.drain_ms),
            sink: Mutex::new(None),
        }),
        stt: Arc::new(BenchStt {
            finalize: Duration::from_millis(cfg.finalize_ms),
        }),
        providers: Arc::new(BenchRegistry(provider)),
        settings: Arc::new(BenchSettings),
        sink: Arc::new(ChannelSink(tx)),
        clock: Arc::new(SystemClock),
        timeouts: SessionTimeouts::default(),
    });

    println!(
        "latency_bench: {} cycles | injected drain {} ms, finalize {} ms, first token {} ms, \
         {} deltas x {} ms",
        cfg.cycles, cfg.drain_ms, cfg.finalize_ms, cfg.first_token_ms, cfg.deltas, cfg.delta_ms
    );

    let mut results = Vec::with_capacity(cfg.cycles);
    let mut failures = 0usize;
    for n in 0..cfg.cycles {
        match one_cycle(
            &session,
            &bench_provider,
            &mut rx,
            Duration::from_millis(cfg.record_ms),
        )
        .await
        {
            Ok(m) => results.push(m),
            Err(e) => {
                failures += 1;
                eprintln!("cycle {}: FAILED: {e}", n + 1);
            }
        }
    }
    session.shutdown().await;

    if results.is_empty() {
        eprintln!("no successful cycles");
        std::process::exit(1);
    }

    println!();
    println!(
        "{:<16}{:>8}{:>8}{:>8}{:>8}",
        "metric (ms)", "p50", "p90", "min", "max"
    );
    let rows: [(&str, MetricFn); 4] = [
        ("audioDrainMs", |m| m.audio_drain_ms),
        ("sttFinalizeMs", |m| m.stt_finalize_ms),
        ("firstTokenMs", |m| m.first_token_ms),
        ("totalMs", |m| m.total_ms),
    ];
    let mut p50s = [0u32; 4];
    for (i, (name, get)) in rows.iter().enumerate() {
        let (p50, p90, min, max) = stats(results.iter().map(|r| get(&r.metrics)));
        p50s[i] = p50;
        println!("{name:<16}{p50:>8}{p90:>8}{min:>8}{max:>8}");
    }
    let (p50, p90, min, max) = stats(results.iter().map(|r| r.provider_first_token_ms));
    println!("{:<16}{p50:>8}{p90:>8}{min:>8}{max:>8}", "(provider wait)");

    // Check 1 (authoritative): the session's own overhead on the Stop ->
    // first-token path, i.e. firstTokenMs minus the waits the fakes really
    // performed. Independent of how precisely the OS honoured the sleeps.
    let mut overheads: Vec<i64> = results.iter().map(Sample::overhead_ms).collect();
    overheads.sort_unstable();
    let overhead_p50 = overheads[overheads.len().div_ceil(2) - 1];
    let overhead_ok = overhead_p50.unsigned_abs() <= cfg.tolerance_ms;

    // Check 2: against the CONFIGURED delays. Only as good as the fakes'
    // sleeps; on a heavily loaded machine they oversleep, so this warns
    // instead of failing.
    let expected = cfg.drain_ms + cfg.finalize_ms + cfg.first_token_ms;
    let got = u64::from(p50s[2]);
    let diff = got.abs_diff(expected);
    let injected_ok = diff <= cfg.tolerance_ms;

    println!();
    println!(
        "check 1: p50 session overhead (firstTokenMs - drain - finalize - provider wait) = {overhead_p50} ms (tolerance ±{} ms) -> {}",
        cfg.tolerance_ms,
        if overhead_ok { "PASS" } else { "FAIL" }
    );
    println!(
        "check 2: p50 firstTokenMs {got} vs injected drain+finalize+first-token {expected} (diff {diff} ms, tolerance ±{} ms) -> {}",
        cfg.tolerance_ms,
        if injected_ok {
            "PASS"
        } else {
            "WARN (the fakes overslept their injected delays - machine under load; check 1 is authoritative)"
        }
    );
    if failures > 0 {
        println!("{failures} cycle(s) failed");
    }
    std::process::exit(if overhead_ok && failures == 0 { 0 } else { 1 });
}
