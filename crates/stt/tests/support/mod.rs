//! Loopback WebSocket server that impersonates Deepgram for socket tests.
//! Binds 127.0.0.1:0 only; never touches the network.

#![allow(dead_code)]
// The tungstenite `Callback` signature fixes the (large) error type.
#![allow(clippy::result_large_err)]

use std::future::Future;
use std::time::Duration;

use callcore_contract::ports::SttEvent;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Upper bound for every wait in socket tests (condition-based, never a sleep).
pub const WAIT: Duration = Duration::from_secs(5);

pub const TEST_KEY: &str = "dg-test-key-SECRET-7f3a";

/// What the client sent in its HTTP upgrade request.
#[derive(Debug, Clone)]
pub struct Handshake {
    pub uri: String,
    pub protocol: Option<String>,
    pub all_headers: String,
}

#[derive(Clone, Copy)]
pub enum Mode {
    /// Accept and echo `Sec-WebSocket-Protocol: token` like Deepgram.
    Accept,
    /// Reject the upgrade with this HTTP status.
    Reject(u16),
}

pub struct TestServer {
    pub url: String,
    handshakes: mpsc::UnboundedReceiver<Handshake>,
    conns: mpsc::UnboundedReceiver<ServerConn>,
}

pub struct ServerConn {
    pub ws: WebSocketStream<TcpStream>,
}

pub async fn start(mode: Mode) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().unwrap();
    let (hs_tx, hs_rx) = mpsc::unbounded_channel();
    let (conn_tx, conn_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let hs_tx = hs_tx.clone();
            let conn_tx = conn_tx.clone();
            tokio::spawn(async move {
                let callback = move |req: &Request, mut resp: Response| {
                    let protocol = req
                        .headers()
                        .get("sec-websocket-protocol")
                        .map(|v| v.to_str().unwrap_or("<non-ascii>").to_string());
                    let all_headers = format!("{:?}", req.headers());
                    let _ = hs_tx.send(Handshake {
                        uri: req.uri().to_string(),
                        protocol,
                        all_headers,
                    });
                    match mode {
                        Mode::Accept => {
                            resp.headers_mut().insert(
                                "sec-websocket-protocol",
                                HeaderValue::from_static("token"),
                            );
                            Ok(resp)
                        }
                        Mode::Reject(status) => {
                            let mut err = ErrorResponse::new(Some("nope".to_string()));
                            *err.status_mut() = StatusCode::from_u16(status).unwrap();
                            Err(err)
                        }
                    }
                };
                if let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, callback).await {
                    let _ = conn_tx.send(ServerConn { ws });
                }
            });
        }
    });
    TestServer {
        url: format!("ws://{addr}/v1/listen?model=nova-3&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true"),
        handshakes: hs_rx,
        conns: conn_rx,
    }
}

impl TestServer {
    pub async fn handshake(&mut self) -> Handshake {
        within(self.handshakes.recv()).await.expect("handshake")
    }
    pub async fn accept(&mut self) -> ServerConn {
        within(self.conns.recv()).await.expect("connection")
    }
}

impl ServerConn {
    /// Next data message from the client (text/binary/close), skipping
    /// ping/pong. `None` = EOF or error.
    pub async fn recv(&mut self) -> Option<Message> {
        within(async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => continue,
                    Some(Ok(m)) => return Some(m),
                    _ => return None,
                }
            }
        })
        .await
    }

    /// Next message that is not a KeepAlive.
    pub async fn recv_non_keepalive(&mut self) -> Option<Message> {
        loop {
            match self.recv().await {
                Some(Message::Text(t)) if t == r#"{"type":"KeepAlive"}"# => continue,
                other => return other,
            }
        }
    }

    pub async fn send_text(&mut self, text: impl Into<String>) {
        self.ws
            .send(Message::Text(text.into()))
            .await
            .expect("server send");
    }

    pub async fn send(&mut self, msg: Message) {
        self.ws.send(msg).await.expect("server send");
    }

    pub async fn close(&mut self, code: u16, reason: &str) {
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: reason.to_string().into(),
        };
        let _ = self.ws.close(Some(frame)).await;
    }

    /// Wait until the client side is gone (EOF, error or close frame).
    pub async fn wait_client_gone(&mut self) {
        within(async {
            loop {
                match self.ws.next().await {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                    Some(Ok(_)) => continue,
                }
            }
        })
        .await
    }
}

/// Deepgram `Results` frame.
pub fn results(transcript: &str, is_final: bool) -> String {
    serde_json::json!({
        "type": "Results",
        "channel_index": [0, 1],
        "duration": 0.5,
        "start": 0.0,
        "is_final": is_final,
        "speech_final": is_final,
        "channel": { "alternatives": [ { "transcript": transcript, "confidence": 0.9, "words": [] } ] },
        "metadata": { "request_id": "r1", "model_info": { "name": "nova-3" } }
    })
    .to_string()
}

pub fn metadata() -> String {
    r#"{"type":"Metadata","transaction_key":"deprecated","request_id":"r1","sha256":"x","created":"2026-01-01T00:00:00Z","duration":1.0,"channels":1}"#.to_string()
}

/// Bounded wait; panics (fails the test) instead of hanging.
pub async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(WAIT, f)
        .await
        .expect("timed out waiting for condition")
}

/// Next event, bounded.
pub async fn next_event(rx: &mut mpsc::Receiver<SttEvent>) -> Option<SttEvent> {
    within(rx.recv()).await
}

/// Drain events until the channel closes (reader task exited).
pub async fn drain_events(rx: &mut mpsc::Receiver<SttEvent>) -> Vec<SttEvent> {
    let mut out = Vec::new();
    while let Some(e) = next_event(rx).await {
        out.push(e);
    }
    out
}
