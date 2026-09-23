//! §14.1 integration test: the REAL app wiring — `build_core`, `AppCtx`, the
//! same command functions the Tauri handlers call, the real event pump and
//! the production `ChannelDelivery` over a real `tauri::ipc::Channel` —
//! against fakes behind the ports (audio, STT, provider, keystore, window).
//!
//! v3 shipped four commits where every command worked but no event ever
//! reached the page, because each side's unit tests mocked the other. These
//! tests fail if an event emitted by the session does not land on the
//! channel the page subscribed with.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aicallassistant_lib::app::{self, AppCtx, Core, CoreDeps, Platform};
use aicallassistant_lib::commands::ChannelDelivery;
use async_trait::async_trait;
use callcore_contract::config::SessionTimeouts;
use callcore_contract::ports::{
    AnswerProvider, AudioError, AudioFrame, AudioMsg, AudioSource, DrainReport, FrameSink,
    HeaderValue, Keystore, KeystoreError, PreparedRequest, PromptParts, ProviderFailure,
    ProviderRegistry, StreamOutcome, SttConnection, SttConnector, SttEvent, SttFailure, SttSender,
    SystemClock,
};
use callcore_contract::{
    copy, AppError, Bounds, BuildInfo, CmdResult, ErrorCode, Finish, HotkeyStatus, LayoutMode,
    Phase, Protection, ProviderInfo, Secret, SecretAction, SecretChange, SessionId, SettingsPatch,
};
use callcore_settings::SettingsStore;
use callcore_shell::close_guard::CloseDecision;
use serde_json::Value;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(15);
const DG_KEY: &str = "dg0123456789abcdef0123456789abcdef0123";
const LLM_KEY: &str = "sk-ant-test-SECRETSECRET";

// ───────────────────────────── fakes ─────────────────────────────

/// Reversible "encryption" so the settings store's fail-closed path is real.
struct FakeKeystore;
impl Keystore for FakeKeystore {
    fn protect(&self, p: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        Ok(p.iter().rev().copied().collect())
    }
    fn unprotect(&self, b: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        Ok(b.iter().rev().copied().collect())
    }
}

#[derive(Default)]
struct FakeAudio {
    sink: Mutex<Option<FrameSink>>,
    starts: AtomicUsize,
}

impl FakeAudio {
    fn push(&self, n: usize) -> bool {
        let g = self.sink.lock().unwrap();
        let Some(s) = g.as_ref() else { return false };
        for _ in 0..n {
            let _ = s.send(AudioMsg::Frame(AudioFrame {
                samples: vec![100; 2048],
                rms: 0.2,
            }));
        }
        true
    }
}

#[async_trait]
impl AudioSource for FakeAudio {
    async fn start(&self, sink: FrameSink) -> Result<(), AudioError> {
        *self.sink.lock().unwrap() = Some(sink);
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn stop_discard(&self) {
        self.sink.lock().unwrap().take();
    }
    async fn stop_and_drain(&self, _timeout: Duration) -> Result<DrainReport, AudioError> {
        let sink = self.sink.lock().unwrap().take();
        if let Some(s) = sink {
            let _ = s.send(AudioMsg::Frame(AudioFrame {
                samples: vec![50; 700],
                rms: 0.1,
            }));
        }
        Ok(DrainReport {
            frames_flushed: 1,
            timed_out: false,
        })
    }
}

const TRANSCRIPT: &str = "what is rust";

struct FakeStt;

struct FakeSender {
    tx: mpsc::Sender<SttEvent>,
    frames: usize,
}

#[async_trait]
impl SttSender for FakeSender {
    async fn send_audio(&mut self, _frame: &AudioFrame) -> Result<(), SttFailure> {
        self.frames += 1;
        let _ = self
            .tx
            .send(SttEvent::Transcript {
                text: TRANSCRIPT.into(),
                is_final: false,
            })
            .await;
        Ok(())
    }
    async fn close_stream(&mut self) -> Result<(), SttFailure> {
        let _ = self
            .tx
            .send(SttEvent::Flushed {
                transcript: TRANSCRIPT.into(),
            })
            .await;
        Ok(())
    }
    fn abort(&mut self) {}
}

#[async_trait]
impl SttConnector for FakeStt {
    async fn connect(&self, key: &Secret) -> Result<SttConnection, SttFailure> {
        assert_eq!(
            key.expose(),
            DG_KEY,
            "the key saved through set_settings reaches STT"
        );
        let (tx, rx) = mpsc::channel(256);
        Ok(SttConnection {
            sender: Box::new(FakeSender { tx, frames: 0 }),
            events: rx,
        })
    }
}

const DELTAS: [&str; 3] = ["Rust is ", "a systems ", "language."];

struct FakeProvider;

#[async_trait]
impl AnswerProvider for FakeProvider {
    fn id(&self) -> &'static str {
        "anthropic"
    }
    fn display_name(&self) -> &'static str {
        "Fake Claude"
    }
    fn key_id(&self) -> &'static str {
        "anthropic"
    }
    fn model(&self) -> &'static str {
        "fake-model"
    }
    fn build_request(&self, _prompt: &PromptParts, key: &Secret) -> PreparedRequest {
        PreparedRequest {
            url: "http://127.0.0.1:9/never".into(),
            headers: vec![("x-api-key".into(), HeaderValue::Secret(key.expose().into()))],
            body: bytes::Bytes::from_static(b"{}"),
        }
    }
    async fn stream(
        &self,
        _req: &PreparedRequest,
        deltas: mpsc::UnboundedSender<String>,
    ) -> Result<StreamOutcome, ProviderFailure> {
        for d in DELTAS {
            let _ = deltas.send(d.to_string());
        }
        Ok(StreamOutcome {
            finish: Finish::Complete,
            answer: DELTAS.concat(),
        })
    }
    fn prewarm(&self) {}
}

struct FakeRegistry(Arc<dyn AnswerProvider>);
impl ProviderRegistry for FakeRegistry {
    fn get(&self, id: &str) -> Option<Arc<dyn AnswerProvider>> {
        (id == "anthropic").then(|| Arc::clone(&self.0))
    }
    fn default_provider(&self) -> Arc<dyn AnswerProvider> {
        Arc::clone(&self.0)
    }
    fn all(&self) -> Vec<Arc<dyn AnswerProvider>> {
        vec![Arc::clone(&self.0)]
    }
}

/// Fake window/OS. Records calls; protection and hotkey results scriptable.
#[derive(Default)]
struct FakePlatform {
    calls: Mutex<Vec<String>>,
    /// Scripted protection verdicts (front first); empty -> Protected.
    protect_script: Mutex<VecDeque<Protection>>,
    /// While set, `register_hotkey` blocks (simulates a busy UI thread).
    hotkey_gate: Mutex<bool>,
    hotkey_cv: std::sync::Condvar,
    taken: Mutex<Vec<String>>,
}

impl FakePlatform {
    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn open_gate(&self) {
        *self.hotkey_gate.lock().unwrap() = false;
        self.hotkey_cv.notify_all();
    }
}

impl Platform for FakePlatform {
    fn set_always_on_top(&self, on: bool) -> Result<(), String> {
        self.log(format!("always_on_top {on}"));
        Ok(())
    }
    fn apply_layout(&self, layout: LayoutMode, saved: Option<Bounds>) -> Result<Bounds, String> {
        self.log(format!("layout {layout:?} saved={}", saved.is_some()));
        Ok(Bounds {
            x: 0,
            y: 0,
            width: 900,
            height: 200,
        })
    }
    fn dock(&self) -> Result<Bounds, String> {
        self.log("dock".into());
        Ok(Bounds {
            x: 740,
            y: 0,
            width: 440,
            height: 640,
        })
    }
    fn current_bounds(&self) -> Option<Bounds> {
        Some(Bounds {
            x: 10,
            y: 20,
            width: 440,
            height: 640,
        })
    }
    fn register_hotkey(&self, accelerator: &str) -> Result<(), String> {
        let mut g = self.hotkey_gate.lock().unwrap();
        while *g {
            g = self.hotkey_cv.wait(g).unwrap();
        }
        drop(g);
        self.log(format!("register {accelerator}"));
        if self.taken.lock().unwrap().iter().any(|t| t == accelerator) {
            return Err("already registered".into());
        }
        Ok(())
    }
    fn unregister_hotkey(&self, accelerator: &str) {
        self.log(format!("unregister {accelerator}"));
    }
    fn open_url(&self, url: &str) -> Result<(), String> {
        self.log(format!("open {url}"));
        Ok(())
    }
    fn protect_once(&self) -> Protection {
        self.log("protect".into());
        self.protect_script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Protection::Protected)
    }
}

/// Stands in for the webview: a REAL `tauri::ipc::Channel` whose message
/// callback records what the page would receive (as JSON).
#[derive(Clone, Default)]
struct Page {
    got: Arc<Mutex<Vec<Value>>>,
}

impl Page {
    fn channel(&self) -> Channel<callcore_contract::EventEnvelope> {
        let got = Arc::clone(&self.got);
        Channel::new(move |body| {
            let v: Value = match body {
                InvokeResponseBody::Json(s) => serde_json::from_str(&s).expect("json"),
                InvokeResponseBody::Raw(b) => serde_json::from_slice(&b).expect("json"),
            };
            got.lock().unwrap().push(v);
            Ok(())
        })
    }
    fn events(&self) -> Vec<Value> {
        self.got.lock().unwrap().clone()
    }
    fn of_type(&self, t: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == t)
            .collect()
    }
    async fn wait_for(&self, what: &str, pred: impl Fn(&[Value]) -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            if pred(&self.events()) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}; got {:#?}",
                self.events()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    async fn wait_type(&self, t: &str) {
        self.wait_for(t, |evs| evs.iter().any(|e| e["type"] == t))
            .await;
    }
}

async fn wait_until(what: &str, f: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !f() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn seqs(evs: &[Value]) -> Vec<u64> {
    evs.iter()
        .map(|e| e["seq"].as_u64().expect("seq"))
        .collect()
}

fn assert_increasing(s: &[u64]) {
    for w in s.windows(2) {
        assert!(w[0] < w[1], "seq not strictly increasing: {s:?}");
    }
}

fn ok<T: std::fmt::Debug>(r: CmdResult<T>) -> T {
    match r {
        CmdResult::Ok(v) => v,
        CmdResult::Err(e) => panic!("command failed: {e:?}"),
    }
}

fn err<T: std::fmt::Debug>(r: CmdResult<T>) -> AppError {
    match r {
        CmdResult::Err(e) => e,
        CmdResult::Ok(v) => panic!("expected an error, got {v:?}"),
    }
}

// ───────────────────────────── harness ─────────────────────────────

struct Harness {
    ctx: Arc<AppCtx>,
    audio: Arc<FakeAudio>,
    platform: Arc<FakePlatform>,
    _dir: tempfile::TempDir,
}

fn build_info() -> BuildInfo {
    BuildInfo {
        version: "4.0.0".into(),
        git_revision: "abc123".into(),
        dirty: false,
        build_time: "t".into(),
    }
}

fn infos() -> Vec<ProviderInfo> {
    vec![ProviderInfo {
        id: "anthropic".into(),
        display_name: "Fake Claude".into(),
        key_id: "anthropic".into(),
        model: "fake-model".into(),
    }]
}

impl Harness {
    /// Build exactly what `run()` builds, minus Tauri: settings store,
    /// platform, `build_core`, `mark_core_ready`. Keys are saved through the
    /// real `set_settings` command.
    async fn new() -> Self {
        let h = Self::without_core();
        let store = h.ctx.settings_store().unwrap();
        let audio = Arc::clone(&h.audio);
        let core: Arc<Core> = app::build_core(
            &h.ctx,
            CoreDeps {
                settings: store,
                audio,
                stt: Arc::new(FakeStt),
                providers: Arc::new(FakeRegistry(Arc::new(FakeProvider))),
                clock: Arc::new(SystemClock),
                timeouts: SessionTimeouts::default(),
                audio_shutdown: None,
            },
        );
        h.ctx.mark_core_ready(core);
        let rev = ok(app::get_settings(&h.ctx).await).settings_revision;
        ok(app::set_settings(
            &h.ctx,
            SettingsPatch {
                base_revision: rev,
                secrets: Some(vec![
                    SecretChange {
                        key_id: "deepgram".into(),
                        action: SecretAction::Set {
                            value: DG_KEY.into(),
                        },
                    },
                    SecretChange {
                        key_id: "anthropic".into(),
                        action: SecretAction::Set {
                            value: LLM_KEY.into(),
                        },
                    },
                ]),
                ..Default::default()
            },
        )
        .await);
        h
    }

    fn without_core() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SettingsStore::open(
            dir.path(),
            Arc::new(FakeKeystore),
            infos(),
        ));
        let ctx = AppCtx::new(
            build_info(),
            "Windows 10.0.test".into(),
            tokio::runtime::Handle::current(),
            Box::new(|| vec!["log line one".to_string(), format!("oops {LLM_KEY}")]),
        );
        ctx.set_settings_store(store);
        let platform = Arc::new(FakePlatform::default());
        ctx.set_platform(platform.clone());
        Self {
            ctx,
            audio: Arc::new(FakeAudio::default()),
            platform,
            _dir: dir,
        }
    }

    async fn subscribe(&self, page: &Page) {
        ok(app::subscribe_events(&self.ctx, Arc::new(ChannelDelivery(page.channel()))).await);
    }

    async fn wait_audio_started(&self) {
        let audio = Arc::clone(&self.audio);
        wait_until("audio start", move || {
            audio.starts.load(Ordering::SeqCst) > 0 && audio.sink.lock().unwrap().is_some()
        })
        .await;
    }
}

fn for_session<'a>(evs: &'a [Value], sid: &SessionId) -> impl Iterator<Item = &'a Value> {
    let sid = sid.0.clone();
    evs.iter().filter(move |e| e["sessionId"] == sid.as_str())
}

// ───────────────────────────── tests ─────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_flow_events_land_on_the_page_channel() {
    let h = Harness::new().await;
    let page = Page::default();
    h.subscribe(&page).await;

    let sid = ok(app::start_session(&h.ctx).await);
    assert_eq!(sid.0.chars().next(), Some('s'));
    h.wait_audio_started().await;
    assert!(h.audio.push(3));
    page.wait_type("stt:partial").await;
    ok(app::stop_session(&h.ctx, sid.clone()).await);
    page.wait_type("llm:done").await;

    let evs = page.events();
    assert_increasing(&seqs(&evs));
    let mine: Vec<&Value> = for_session(&evs, &sid).collect();
    let pos = |t: &str| {
        mine.iter()
            .position(|e| e["type"] == t)
            .unwrap_or_else(|| panic!("no {t} in {mine:#?}"))
    };
    assert!(pos("stt:partial") < pos("llm:delta"));
    assert!(pos("llm:delta") < pos("llm:done"));
    let partial = &mine[pos("stt:partial")];
    assert_eq!(partial["text"], TRANSCRIPT);
    let deltas: String = mine
        .iter()
        .filter(|e| e["type"] == "llm:delta")
        .map(|e| e["delta"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(deltas, DELTAS.concat(), "every delta, in order");
    let done = &mine[pos("llm:done")];
    assert_eq!(done["answer"], DELTAS.concat());
    assert_eq!(done["transcript"], TRANSCRIPT);
    assert_eq!(done["finish"], "complete");
    assert_eq!(
        mine.iter()
            .filter(|e| e["type"] == "llm:done" || e["type"] == "session:error")
            .count(),
        1
    );
    // The subscribe also published the protection verdict.
    page.wait_type("protection:ok").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resubscribe_mid_session_still_delivers_the_terminal_event() {
    let h = Harness::new().await;
    let first = Page::default();
    h.subscribe(&first).await;
    let sid = ok(app::start_session(&h.ctx).await);
    h.wait_audio_started().await;
    h.audio.push(2);
    first.wait_type("stt:partial").await;

    // Page reload: a NEW channel replaces the old one mid-session.
    let reloaded = Page::default();
    h.subscribe(&reloaded).await;
    // The reloaded page re-syncs from get_status and re-adopts the session.
    let snap = ok(app::get_status(&h.ctx).await);
    assert_eq!(snap.session.id.as_ref(), Some(&sid));
    assert_ne!(snap.session.phase, Phase::Idle);

    ok(app::stop_session(&h.ctx, sid.clone()).await);
    reloaded.wait_type("llm:done").await;
    assert!(
        first.of_type("llm:done").is_empty(),
        "the replaced channel gets nothing new"
    );
    let mut all = first.events();
    all.extend(reloaded.events());
    assert_increasing(&seqs(&all));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_flow_and_status_revisions() {
    let h = Harness::new().await;
    let page = Page::default();
    h.subscribe(&page).await;
    let before = ok(app::get_status(&h.ctx).await);
    assert_eq!(before.core, callcore_contract::CoreState::Ready);

    let sid = ok(app::ask(&h.ctx, "  what is rust?  ".into()).await);
    page.wait_for("ask done", |evs| {
        for_session(evs, &sid).any(|e| e["type"] == "llm:done")
    })
    .await;
    let evs = page.events();
    let q: Vec<&Value> = for_session(&evs, &sid)
        .filter(|e| e["type"] == "stt:partial")
        .collect();
    assert_eq!(q[0]["text"], "what is rust?");
    assert_eq!(q[0]["isFinal"], true);

    let hub = Arc::clone(&h.ctx.hub);
    wait_until("idle", move || hub.snapshot().session.phase == Phase::Idle).await;
    let after = ok(app::get_status(&h.ctx).await);
    assert!(
        after.revision > before.revision,
        "session changes bump the revision"
    );
    // Empty question: error, nothing started.
    let e = err(app::ask(&h.ctx, "   ".into()).await);
    assert_eq!(e.message, copy::EMPTY_QUESTION);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn core_failure_gives_actionable_errors_and_status_still_answers() {
    let h = Harness::without_core();
    let page = Page::default();
    h.subscribe(&page).await;
    h.ctx.mark_core_failed(AppError::internal("boom"));
    page.wait_type("core:failed").await;
    let e = err(app::start_session(&h.ctx).await);
    assert_eq!(
        (e.code, e.message.as_str()),
        (ErrorCode::Internal, copy::CORE_FAILED)
    );
    assert_eq!(
        err(app::get_settings(&h.ctx).await).message,
        copy::CORE_FAILED
    );
    let snap = ok(app::get_status(&h.ctx).await);
    assert_eq!(snap.core, callcore_contract::CoreState::Failed);
    let ev = &page.of_type("core:failed")[0];
    assert!(ev["revision"].as_u64().unwrap() <= snap.revision);
    // Cancel is always ok, even with no core.
    ok(app::cancel_session(&h.ctx, SessionId("s9".into())).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn set_settings_side_effects_run_and_stale_revision_applies_nothing() {
    let h = Harness::new().await;
    let v = ok(app::get_settings(&h.ctx).await);
    assert_eq!(
        v.build.git_revision, "abc123",
        "build info filled by the shell"
    );
    let v2 = ok(app::set_settings(
        &h.ctx,
        SettingsPatch {
            base_revision: v.settings_revision,
            hotkey: Some("alt+shift+k".into()),
            always_on_top: Some(!v.always_on_top),
            layout_mode: Some(LayoutMode::Prompter),
            ..Default::default()
        },
    )
    .await);
    assert_eq!(v2.hotkey_status, HotkeyStatus::Registered);
    assert!(v2.hotkey_registered);
    let calls = h.platform.calls();
    assert!(
        calls.contains(&"register Alt+Shift+K".to_string()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!("always_on_top {}", !v.always_on_top)),
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("layout Prompter")),
        "{calls:?}"
    );
    assert_eq!(h.ctx.layout(), LayoutMode::Prompter);
    // The outgoing layout's geometry was persisted before switching.
    assert!(h
        .ctx
        .settings_store()
        .unwrap()
        .bounds(LayoutMode::Full)
        .is_some());

    let n = h.platform.calls().len();
    let e = err(app::set_settings(
        &h.ctx,
        SettingsPatch {
            base_revision: v.settings_revision,
            hotkey: Some("Ctrl+Q".into()),
            ..Default::default()
        },
    )
    .await);
    assert_eq!(e.code, ErrorCode::Internal);
    assert!(
        h.platform.calls()[n..].iter().all(|c| c == "protect"),
        "no side effects: {:?}",
        &h.platform.calls()[n..]
    );
    assert_eq!(ok(app::get_settings(&h.ctx).await).hotkey, "alt+shift+k");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn taken_and_invalid_hotkeys_report_honestly() {
    let h = Harness::new().await;
    h.platform
        .taken
        .lock()
        .unwrap()
        .push("Ctrl+Shift+Space".into());
    let r = h.ctx.apply_hotkey("Ctrl+Shift+Space").await;
    assert_eq!(r.status, HotkeyStatus::Unavailable);
    assert_eq!(
        r.message.as_deref(),
        Some("Ctrl+Shift+Space is already taken by another app. Choose a different shortcut in Settings.")
    );
    let v = ok(app::get_settings(&h.ctx).await);
    assert!(!v.hotkey_registered);
    assert_eq!(v.hotkey_status, HotkeyStatus::Unavailable);
    assert_eq!(
        h.ctx.apply_hotkey("Ctrl+Banana").await.status,
        HotkeyStatus::Invalid
    );
    assert_eq!(h.ctx.apply_hotkey("").await.status, HotkeyStatus::Disabled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_hotkey_registration_is_undone() {
    let h = Harness::new().await;
    let page = Page::default();
    h.subscribe(&page).await;
    *h.platform.hotkey_gate.lock().unwrap() = true; // UI thread "busy"
    let slow = h.ctx.apply_hotkey("Ctrl+Q").await; // gives up after the wait bound
    assert!(!slow.registered());
    h.platform.open_gate();
    // Once the gate opens the old registration lands; a newer request
    // replaced it by then? Not yet — so it is simply applied late.
    let platform = Arc::clone(&h.platform);
    wait_until("late register", move || {
        platform.calls().contains(&"register Ctrl+Q".to_string())
    })
    .await;
    page.wait_type("settings:changed").await;
    assert!(
        h.ctx.hotkey_report().registered(),
        "late success of the CURRENT request is adopted"
    );

    // Now the real race: a registration stuck in flight, superseded by a
    // newer request, must be undone when it finally succeeds.
    *h.platform.hotkey_gate.lock().unwrap() = true;
    let stuck = h.ctx.apply_hotkey("Ctrl+W").await;
    assert!(!stuck.registered());
    // Newer request (the gate is still shut, so release it from a thread
    // right after this request starts waiting).
    let p2 = Arc::clone(&h.platform);
    let opener = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        p2.open_gate();
    });
    let newer = h.ctx.apply_hotkey("Alt+F9").await;
    opener.join().unwrap();
    assert!(newer.registered(), "{newer:?}");
    let platform = Arc::clone(&h.platform);
    wait_until("undo of the stale Ctrl+W", move || {
        platform.calls().contains(&"unregister Ctrl+W".to_string())
    })
    .await;
    assert_eq!(h.ctx.hotkey_report().status, HotkeyStatus::Registered);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protection_is_verified_with_retries_and_never_claimed_early() {
    let h = Harness::without_core();
    let page = Page::default();
    h.subscribe(&page).await;
    // Let the subscribe-triggered verify finish first.
    page.wait_type("protection:ok").await;
    h.platform.protect_script.lock().unwrap().extend([
        Protection::Unprotected,
        Protection::Unprotected,
        Protection::Protected,
    ]);
    let v = h.ctx.verify_protection(&[0, 10, 10, 10], true).await;
    assert_eq!(v, Protection::Protected);

    // All attempts fail -> Unprotected, published as protection:failed with
    // the hub's revision.
    h.platform
        .protect_script
        .lock()
        .unwrap()
        .extend([Protection::Unprotected; 3]);
    let v = h.ctx.verify_protection(&[0, 10, 10], true).await;
    assert_eq!(v, Protection::Unprotected);
    page.wait_type("protection:failed").await;
    let ev = page.of_type("protection:failed").pop().unwrap();
    assert_eq!(ev["protection"], "unprotected");
    let snap = ok(app::get_status(&h.ctx).await);
    assert_eq!(snap.protection, Protection::Unprotected);
    assert_eq!(ev["revision"].as_u64().unwrap(), snap.revision);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_guard_cancels_once_and_notifies_the_page() {
    let h = Harness::without_core();
    assert_eq!(h.ctx.on_close_request(), CloseDecision::Allow, "no guard");
    ok(app::set_close_guard(&h.ctx, true).await);
    assert_eq!(
        h.ctx.on_close_request(),
        CloseDecision::Allow,
        "no page attached: never uncloseable"
    );
    let page = Page::default();
    h.subscribe(&page).await;
    assert_eq!(h.ctx.on_close_request(), CloseDecision::CancelAndNotify);
    page.wait_type("window:close-requested").await;
    assert_eq!(
        h.ctx.on_close_request(),
        CloseDecision::Allow,
        "second close within 10 s closes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn open_external_validates_before_touching_the_os() {
    let h = Harness::without_core();
    for bad in [
        "http://example.com",
        "javascript:alert(1)",
        "https://a b",
        "https://user@x.com",
    ] {
        err(app::open_external(&h.ctx, bad.into()).await);
    }
    assert!(!h.platform.calls().iter().any(|c| c.starts_with("open")));
    ok(app::open_external(&h.ctx, "https://console.deepgram.com/".into()).await);
    assert!(h
        .platform
        .calls()
        .contains(&"open https://console.deepgram.com/".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dock_persists_bounds_for_the_current_layout() {
    let h = Harness::without_core();
    ok(app::dock_window(&h.ctx).await);
    assert_eq!(
        h.ctx.settings_store().unwrap().bounds(LayoutMode::Full),
        Some(Bounds {
            x: 740,
            y: 0,
            width: 440,
            height: 640
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diagnostics_have_status_and_no_secrets() {
    let h = Harness::new().await;
    let d = ok(app::get_diagnostics(&h.ctx).await);
    assert!(d.contains("version: 4.0.0"), "{d}");
    assert!(d.contains("os: Windows 10.0.test"), "{d}");
    assert!(d.contains("core: ready"), "{d}");
    assert!(d.contains("log line one"), "{d}");
    assert!(!d.contains("SECRETSECRET") && !d.contains(DG_KEY), "{d}");
    // Settings views never carry key material either.
    let v = serde_json::to_string(&ok(app::get_settings(&h.ctx).await)).unwrap();
    assert!(!v.contains("SECRETSECRET") && !v.contains(DG_KEY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_is_bounded_and_idempotent() {
    let h = Harness::new().await;
    let sid = ok(app::start_session(&h.ctx).await);
    h.wait_audio_started().await;
    tokio::time::timeout(callcore_contract::config::SHUTDOWN_BUDGET, h.ctx.shutdown())
        .await
        .expect("shutdown within budget");
    // After shutdown commands fail cleanly (no panic, no hang).
    let r = tokio::time::timeout(Duration::from_secs(5), app::stop_session(&h.ctx, sid))
        .await
        .unwrap();
    assert!(matches!(r, CmdResult::Err(_)));
    tokio::time::timeout(callcore_contract::config::SHUTDOWN_BUDGET, h.ctx.shutdown())
        .await
        .unwrap();
}

// ───────────────────────────── invoke level (tauri mock runtime) ─────────────────────────────

mod invoke {
    use super::*;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::Manager;

    fn request(cmd: &str, body: Value) -> InvokeRequest {
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            // The app's own (local) origin: Windows serves it from
            // http://tauri.localhost, elsewhere tauri://localhost.
            url: if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        }
    }

    fn json(r: Result<InvokeResponseBody, Value>) -> Value {
        match r.expect("invoke resolved") {
            InvokeResponseBody::Json(s) => serde_json::from_str(&s).unwrap(),
            InvokeResponseBody::Raw(b) => serde_json::from_slice(&b).unwrap(),
        }
    }

    /// Drives the real `#[tauri::command]` handlers through Tauri's IPC
    /// (argument decoding, camelCase mapping, the `{ok, value}` envelope,
    /// Channel argument) on the mock runtime.
    #[test]
    fn commands_resolve_through_tauri_ipc() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let h = rt.block_on(async { Harness::new().await });
        let app = mock_builder()
            .manage(Arc::clone(&h.ctx))
            .invoke_handler(aicallassistant_lib::invoke_handler())
            .build(mock_context(noop_assets()))
            .expect("mock app");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("webview");
        assert!(app.get_webview_window("main").is_some());

        let status = json(get_ipc_response(
            &webview,
            request("get_status", serde_json::json!({})),
        ));
        assert_eq!(status["ok"], true, "{status}");
        assert_eq!(status["value"]["core"], "ready");

        let settings = json(get_ipc_response(
            &webview,
            request("get_settings", serde_json::json!({})),
        ));
        assert_eq!(settings["ok"], true);
        assert!(settings["value"]["settingsRevision"].is_number());

        // camelCase arg mapping + error envelope (not an IPC rejection).
        let stop = json(get_ipc_response(
            &webview,
            request("stop_session", serde_json::json!({"sessionId": "s99"})),
        ));
        assert_eq!(stop["ok"], false, "{stop}");
        assert_eq!(stop["error"]["code"], "internal");

        let cancel = json(get_ipc_response(
            &webview,
            request("cancel_session", serde_json::json!({"sessionId": "s99"})),
        ));
        assert_eq!(cancel, serde_json::json!({"ok": true, "value": null}));

        let bad_url = json(get_ipc_response(
            &webview,
            request("open_external", serde_json::json!({"url": "http://x"})),
        ));
        assert_eq!(bad_url["ok"], false);

        // The Channel argument decodes and attaches to the pump.
        h.ctx.pump.detach();
        let sub = json(get_ipc_response(
            &webview,
            request(
                "subscribe_events",
                serde_json::json!({"channel": "__CHANNEL__:7"}),
            ),
        ));
        assert_eq!(sub, serde_json::json!({"ok": true, "value": null}));
        assert!(h.ctx.pump.is_attached());

        let started = json(get_ipc_response(
            &webview,
            request("start_session", serde_json::json!({})),
        ));
        assert_eq!(started["ok"], true, "{started}");
        assert!(started["value"].as_str().unwrap().starts_with('s'));
        rt.block_on(h.ctx.shutdown());
    }
}
