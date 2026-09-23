//! Deepgram streaming client. PUBLIC API PINNED.
//!
//! `DeepgramConnector::connect` opens the socket (key in the
//! `Sec-WebSocket-Protocol` header, never in the URL) and splits it into:
//!
//! * a **sender task** that owns the write half, forwards audio / CloseStream
//!   commands from [`DeepgramSender`] and sends `{"type":"KeepAlive"}` while
//!   idle;
//! * a **reader task** that parses server frames as hostile input, feeds the
//!   [`TranscriptAccumulator`] and emits [`SttEvent`]s on a bounded channel.
//!
//! The reader emits exactly one terminal event (`Flushed` or `Failed`) and
//! nothing after it, so a late close after the final transcript can never
//! surface as an error (spec §5.5).

pub mod accumulator;
pub mod parse;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use callcore_contract::config::{STT_KEEPALIVE_INTERVAL, TRANSPORT_WRITE_TIMEOUT};
use callcore_contract::copy;
use callcore_contract::ports::{
    AudioFrame, SttConnection, SttConnector, SttEvent, SttFailure, SttFailureKind, SttSender,
};
use callcore_contract::Secret;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, timeout, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

pub use accumulator::{TranscriptAccumulator, TranscriptUpdate};
pub use parse::{parse_results, ResultSegment, MAX_RESULT_FRAME_BYTES};

pub const DEEPGRAM_URL: &str = "wss://api.deepgram.com/v1/listen?model=nova-3&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true";

/// Capacity of the event channel handed to the session (it drains promptly).
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

/// tungstenite's own incoming-message cap. Deliberately ABOVE
/// [`MAX_RESULT_FRAME_BYTES`]: frames between the two are read and ignored;
/// only a frame over this limit is a protocol failure (the stream fails with
/// `Server`, it never panics or allocates unbounded memory).
pub const WS_MAX_MESSAGE_BYTES: usize = 4 << 20;

pub const KEEPALIVE_MESSAGE: &str = r#"{"type":"KeepAlive"}"#;
pub const CLOSE_STREAM_MESSAGE: &str = r#"{"type":"CloseStream"}"#;

/// How long the reader keeps polling after a close frame so tungstenite can
/// finish the close handshake politely. Nothing is emitted during it.
const CLOSE_HANDSHAKE_GRACE: Duration = Duration::from_secs(2);

const CLOSED_MESSAGE: &str = "The Deepgram connection is closed.";
const BAD_KEY_HTTP_PREFIX: &str = "Deepgram rejected the API key";

#[derive(Debug, Clone)]
pub struct DeepgramConnector {
    url: String,
    keepalive: Duration,
}

impl DeepgramConnector {
    pub fn new() -> Self {
        Self::with_url(DEEPGRAM_URL)
    }
    /// Test seam: e.g. "ws://127.0.0.1:PORT/v1/listen?...".
    pub fn with_url(url: &str) -> Self {
        Self {
            url: url.to_string(),
            keepalive: STT_KEEPALIVE_INTERVAL,
        }
    }

    /// Test seam: like [`with_url`](Self::with_url) with a custom KeepAlive
    /// interval (tests use ~100 ms so they don't wait 8 s of real time).
    #[doc(hidden)]
    pub fn with_url_and_keepalive(url: &str, keepalive: Duration) -> Self {
        Self {
            url: url.to_string(),
            keepalive,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn keepalive_interval(&self) -> Duration {
        self.keepalive
    }
}

impl Default for DeepgramConnector {
    fn default() -> Self {
        Self::new()
    }
}

fn connect_failure() -> SttFailure {
    SttFailure {
        kind: SttFailureKind::Connect,
        message: copy::STT_CONNECT.to_string(),
    }
}

fn closed_failure() -> SttFailure {
    SttFailure {
        kind: SttFailureKind::Closed,
        message: CLOSED_MESSAGE.to_string(),
    }
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(WS_MAX_MESSAGE_BYTES),
        max_frame_size: Some(WS_MAX_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

/// `token, <key>` as a sensitive header value. The temporary byte buffer is
/// wiped after the header copies it.
fn auth_header(key: &Secret) -> Result<HeaderValue, SttFailure> {
    let mut raw = Vec::with_capacity(7 + key.expose().len());
    raw.extend_from_slice(b"token, ");
    raw.extend_from_slice(key.expose().as_bytes());
    let parsed = HeaderValue::from_bytes(&raw);
    // Best-effort wipe (like `Secret::drop`); black_box keeps it from being
    // optimized away.
    raw.iter_mut().for_each(|b| *b = 0);
    std::hint::black_box(&raw);
    let mut value = parsed.map_err(|_| SttFailure {
        kind: SttFailureKind::BadKey,
        message: "The Deepgram API key contains characters that are not allowed. Re-enter it in Settings."
            .to_string(),
    })?;
    value.set_sensitive(true);
    Ok(value)
}

#[async_trait]
impl SttConnector for DeepgramConnector {
    async fn connect(&self, key: &Secret) -> Result<SttConnection, SttFailure> {
        let mut request = self.url.as_str().into_client_request().map_err(|e| {
            tracing::warn!(error = %e, "deepgram: invalid connect URL");
            connect_failure()
        })?;
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_PROTOCOL, auth_header(key)?);

        match tokio_tungstenite::connect_async_with_config(request, Some(ws_config()), true).await {
            Ok((ws, _response)) => Ok(spawn_connection(ws, self.keepalive)),
            Err(WsError::Http(response)) => {
                let status = response.status();
                tracing::warn!(status = status.as_u16(), "deepgram: handshake rejected");
                if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                    Err(SttFailure {
                        kind: SttFailureKind::BadKey,
                        message: format!(
                            "{BAD_KEY_HTTP_PREFIX} (HTTP {}). Check the key in Settings.",
                            status.as_u16()
                        ),
                    })
                } else {
                    Err(connect_failure())
                }
            }
            Err(e) => {
                // tungstenite errors never contain request headers.
                tracing::warn!(error = %e, "deepgram: connect failed");
                Err(connect_failure())
            }
        }
    }
}

// ───────────────────────────── connection tasks ─────────────────────────────

enum Command {
    Audio(Vec<u8>),
    CloseStream,
}

#[derive(Default)]
struct Shared {
    /// Set before the reader emits its terminal event, or when a write fails.
    dead: AtomicBool,
    /// Set by `close_stream` BEFORE the command is queued, so a normal close
    /// racing the command is still classified as a flush.
    close_requested: AtomicBool,
    /// Reader -> writer: stop now.
    stop_writer: Notify,
    /// Writer -> reader: a write failed, the socket is gone.
    writer_failed: Notify,
}

fn spawn_connection<S>(ws: WebSocketStream<S>, keepalive: Duration) -> SttConnection
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, stream) = ws.split();
    let shared = Arc::new(Shared::default());
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);

    let writer = tokio::spawn(writer_loop(sink, cmd_rx, keepalive, shared.clone()));
    let reader = tokio::spawn(reader_loop(stream, event_tx, shared.clone()));

    SttConnection {
        sender: Box::new(DeepgramSender {
            cmd_tx: Some(cmd_tx),
            shared,
            writer: Some(writer),
            reader: Some(reader),
            close_sent: false,
        }),
        events: event_rx,
    }
}

async fn writer_loop<S>(
    mut sink: SplitSink<WebSocketStream<S>, Message>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    keepalive: Duration,
    shared: Arc<Shared>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut next_keepalive = Instant::now() + keepalive;
    loop {
        let (message, last) = tokio::select! {
            biased;
            _ = shared.stop_writer.notified() => return,
            cmd = commands.recv() => match cmd {
                // Sender dropped/aborted: stop writing (no close frame; the
                // reader or the abort decides the socket's fate).
                None => return,
                Some(Command::Audio(bytes)) => (Message::Binary(bytes), false),
                Some(Command::CloseStream) => (Message::text(CLOSE_STREAM_MESSAGE), true),
            },
            _ = sleep_until(next_keepalive) => (Message::text(KEEPALIVE_MESSAGE), false),
        };
        let ok = matches!(
            timeout(TRANSPORT_WRITE_TIMEOUT, sink.send(message)).await,
            Ok(Ok(()))
        );
        if !ok {
            shared.dead.store(true, Ordering::SeqCst);
            shared.writer_failed.notify_one();
            return;
        }
        if last {
            // Nothing may follow CloseStream (not even KeepAlive). The reader
            // keeps the socket alive until the server closes it.
            return;
        }
        next_keepalive = Instant::now() + keepalive;
    }
}

async fn reader_loop<S>(
    mut stream: SplitStream<WebSocketStream<S>>,
    events: mpsc::Sender<SttEvent>,
    shared: Arc<Shared>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut acc = TranscriptAccumulator::new();
    let mut saw_close_frame = false;
    let terminal = loop {
        let next = tokio::select! {
            biased;
            _ = events.closed() => {
                // Nobody listens any more: tear down quietly.
                shared.dead.store(true, Ordering::SeqCst);
                shared.stop_writer.notify_one();
                return;
            }
            _ = shared.writer_failed.notified() => {
                break SttEvent::Failed(SttFailure {
                    kind: SttFailureKind::Server,
                    message: "Lost the connection to Deepgram while sending audio.".to_string(),
                });
            }
            next = stream.next() => next,
        };
        match next {
            Some(Ok(Message::Text(text))) => {
                let Some(segment) = parse_results(&text) else {
                    continue;
                };
                let Some(update) = acc.apply(&segment.transcript, segment.is_final) else {
                    continue;
                };
                let event = SttEvent::Transcript {
                    text: update.text,
                    is_final: update.is_final,
                };
                if events.send(event).await.is_err() {
                    shared.dead.store(true, Ordering::SeqCst);
                    shared.stop_writer.notify_one();
                    return;
                }
            }
            Some(Ok(Message::Close(frame))) => {
                saw_close_frame = true;
                let (code, reason) = match &frame {
                    Some(f) => (Some(u16::from(f.code)), f.reason.as_ref()),
                    None => (None, ""),
                };
                let requested = shared.close_requested.load(Ordering::SeqCst);
                break match parse::classify_close(code, reason, requested) {
                    parse::CloseOutcome::Flushed => SttEvent::Flushed {
                        transcript: acc.full_text(),
                    },
                    parse::CloseOutcome::Failed(f) => SttEvent::Failed(f),
                };
            }
            // Binary / ping / pong / raw frames from the server carry nothing
            // for us (tungstenite answers pings itself).
            Some(Ok(_)) => {}
            // The stream ended after a completed close handshake we didn't see
            // as a frame: clean end.
            None => {
                break if shared.close_requested.load(Ordering::SeqCst) {
                    SttEvent::Flushed {
                        transcript: acc.full_text(),
                    }
                } else {
                    SttEvent::Failed(SttFailure {
                        kind: SttFailureKind::Server,
                        message: parse::UNEXPECTED_END.to_string(),
                    })
                };
            }
            Some(Err(e)) => {
                let detail = parse::sanitize_reason(&e.to_string());
                tracing::warn!(error = %detail, "deepgram: stream error");
                break SttEvent::Failed(SttFailure {
                    kind: SttFailureKind::Server,
                    message: format!("Lost the connection to Deepgram mid-stream ({detail})."),
                });
            }
        }
    };

    shared.dead.store(true, Ordering::SeqCst);
    shared.stop_writer.notify_one();
    let _ = events.send(terminal).await;
    drop(events);

    if saw_close_frame {
        // Let tungstenite flush its close reply; ignore whatever comes.
        let _ = timeout(CLOSE_HANDSHAKE_GRACE, async {
            while let Some(Ok(_)) = stream.next().await {}
        })
        .await;
    }
}

/// The session's handle on a live Deepgram socket.
pub struct DeepgramSender {
    cmd_tx: Option<mpsc::UnboundedSender<Command>>,
    shared: Arc<Shared>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    close_sent: bool,
}

impl fmt::Debug for DeepgramSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeepgramSender")
            .field("aborted", &self.cmd_tx.is_none())
            .field("dead", &self.shared.dead.load(Ordering::SeqCst))
            .field("close_sent", &self.close_sent)
            .finish()
    }
}

impl DeepgramSender {
    fn queue(&mut self, cmd: Command) -> Result<(), SttFailure> {
        if self.shared.dead.load(Ordering::SeqCst) {
            return Err(closed_failure());
        }
        let tx = self.cmd_tx.as_ref().ok_or_else(closed_failure)?;
        tx.send(cmd).map_err(|_| closed_failure())
    }
}

#[async_trait]
impl SttSender for DeepgramSender {
    async fn send_audio(&mut self, frame: &AudioFrame) -> Result<(), SttFailure> {
        if self.close_sent {
            return Err(closed_failure());
        }
        self.queue(Command::Audio(frame.to_le_bytes()))
    }

    async fn close_stream(&mut self) -> Result<(), SttFailure> {
        if self.close_sent {
            return Ok(());
        }
        self.shared.close_requested.store(true, Ordering::SeqCst);
        self.queue(Command::CloseStream)?;
        self.close_sent = true;
        Ok(())
    }

    fn abort(&mut self) {
        self.cmd_tx = None;
        self.shared.dead.store(true, Ordering::SeqCst);
        if let Some(h) = self.writer.take() {
            h.abort();
        }
        if let Some(h) = self.reader.take() {
            h.abort();
        }
    }
}

impl Drop for DeepgramSender {
    fn drop(&mut self) {
        // Before CloseStream the socket is useless without us: tear it down.
        // After CloseStream the reader must stay alive to deliver `Flushed`;
        // it exits on server close or when the event receiver is dropped.
        if !self.close_sent {
            self.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_url_has_no_endpointing_or_no_delay_and_no_key() {
        let c = DeepgramConnector::new();
        assert_eq!(c.url(), DEEPGRAM_URL);
        assert!(!DEEPGRAM_URL.contains("endpointing"));
        assert!(!DEEPGRAM_URL.contains("no_delay"));
        assert!(!DEEPGRAM_URL.contains("token"));
        assert!(DEEPGRAM_URL.contains("encoding=linear16"));
        assert!(DEEPGRAM_URL.contains("sample_rate=16000"));
        assert!(DEEPGRAM_URL.contains("interim_results=true"));
    }

    #[test]
    fn production_keepalive_is_the_config_constant() {
        assert_eq!(
            DeepgramConnector::new().keepalive_interval(),
            STT_KEEPALIVE_INTERVAL
        );
        assert_eq!(
            DeepgramConnector::default().keepalive_interval(),
            Duration::from_secs(8)
        );
    }

    #[test]
    fn auth_header_is_token_subprotocol_and_sensitive() {
        let v = auth_header(&Secret::new("dg-key-123")).unwrap();
        assert_eq!(v.to_str().unwrap(), "token, dg-key-123");
        assert!(v.is_sensitive());
        let dbg = format!("{v:?}");
        assert!(!dbg.contains("dg-key-123"), "{dbg}");
    }

    #[test]
    fn auth_header_rejects_control_chars_without_echoing_key() {
        let err = auth_header(&Secret::new("bad\nkey-SECRETPART")).unwrap_err();
        assert_eq!(err.kind, SttFailureKind::BadKey);
        assert!(!err.message.contains("SECRETPART"));
        assert!(!format!("{err:?}").contains("SECRETPART"));
    }

    #[test]
    fn ws_config_caps_incoming_messages_above_the_parse_cap() {
        let c = ws_config();
        assert_eq!(c.max_message_size, Some(WS_MAX_MESSAGE_BYTES));
        const { assert!(WS_MAX_MESSAGE_BYTES > MAX_RESULT_FRAME_BYTES) };
    }

    #[test]
    fn rustls_has_a_crypto_provider_for_wss() {
        // tokio-tungstenite calls `ClientConfig::builder()`, which panics when
        // no rustls crypto provider is compiled in. This crate enables `ring`.
        let _ = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
    }
}
