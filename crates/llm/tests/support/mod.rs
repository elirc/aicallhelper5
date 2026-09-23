//! Tiny scripted HTTP/1.1 loopback server for provider conformance tests.
//!
//! Each accepted connection consumes the next [`Script`] (the last one repeats),
//! records the request, and replays the scripted response with controllable
//! framing (chunked / content-length / close-delimited), split writes, and
//! abrupt closes. Real sockets, real time — never a paused clock.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    Chunked,
    ContentLength,
    CloseDelimited,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Each part is written (and flushed) separately.
    pub parts: Vec<Vec<u8>>,
    pub framing: Framing,
    /// Close the socket before the body is complete (no final chunk / fewer
    /// bytes than the declared Content-Length).
    pub truncate: bool,
}

impl Reply {
    pub fn sse(parts: Vec<Vec<u8>>) -> Self {
        Reply {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            parts,
            framing: Framing::Chunked,
            truncate: false,
        }
    }

    pub fn status(status: u16, body: &str) -> Self {
        Reply {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            parts: vec![body.as_bytes().to_vec()],
            framing: Framing::ContentLength,
            truncate: false,
        }
    }

    pub fn framing(mut self, f: Framing) -> Self {
        self.framing = f;
        self
    }

    pub fn truncated(mut self) -> Self {
        self.truncate = true;
        self
    }
}

#[derive(Debug, Clone)]
pub enum Script {
    Reply(Reply),
    /// Read the request, then close without sending a single byte.
    CloseWithoutResponse,
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Lower-cased names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct TestServer {
    pub port: u16,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    arrived: tokio::sync::Mutex<mpsc::UnboundedReceiver<Recorded>>,
}

impl TestServer {
    pub async fn start(scripts: Vec<Script>) -> TestServer {
        assert!(!scripts.is_empty());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let reqs = requests.clone();
        tokio::spawn(async move {
            let mut n = 0usize;
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let script = scripts[n.min(scripts.len() - 1)].clone();
                n += 1;
                let reqs = reqs.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = serve(sock, script, reqs, tx).await;
                });
            }
        });
        TestServer {
            port,
            requests,
            arrived: tokio::sync::Mutex::new(rx),
        }
    }

    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Wait (bounded, real time) for the next request to arrive.
    pub async fn next_request(&self) -> Recorded {
        let mut rx = self.arrived.lock().await;
        tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("request did not arrive in time")
            .expect("server gone")
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve(
    mut sock: TcpStream,
    script: Script,
    reqs: Arc<Mutex<Vec<Recorded>>>,
    tx: mpsc::UnboundedSender<Recorded>,
) -> std::io::Result<()> {
    sock.set_nodelay(true)?;
    let rec = read_request(&mut sock).await?;
    reqs.lock().unwrap().push(rec.clone());
    let _ = tx.send(rec);
    let reply = match script {
        Script::CloseWithoutResponse => {
            drop(sock);
            return Ok(());
        }
        Script::Reply(r) => r,
    };
    let total: usize = reply.parts.iter().map(Vec::len).sum();
    let mut head = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (k, v) in &reply.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    match reply.framing {
        Framing::Chunked => head.push_str("transfer-encoding: chunked\r\n"),
        Framing::ContentLength => {
            let declared = if reply.truncate { total + 64 } else { total };
            head.push_str(&format!("content-length: {declared}\r\n"));
        }
        Framing::CloseDelimited => {}
    }
    head.push_str("connection: close\r\n\r\n");
    sock.write_all(head.as_bytes()).await?;
    sock.flush().await?;
    for part in &reply.parts {
        match reply.framing {
            Framing::Chunked => {
                if part.is_empty() {
                    continue; // an empty chunk would terminate the body
                }
                let mut frame = format!("{:x}\r\n", part.len()).into_bytes();
                frame.extend_from_slice(part);
                frame.extend_from_slice(b"\r\n");
                sock.write_all(&frame).await?;
            }
            _ => sock.write_all(part).await?,
        }
        sock.flush().await?;
        // Encourage separate TCP segments so the client sees the splits.
        tokio::task::yield_now().await;
    }
    if reply.framing == Framing::Chunked && !reply.truncate {
        sock.write_all(b"0\r\n\r\n").await?;
    }
    sock.flush().await?;
    // Graceful FIN either way: a truncated body is detected by the framing
    // (missing final chunk / short Content-Length). An RST could make the
    // client discard bytes it already received, which would make "deltas
    // before the drop" assertions flaky.
    let _ = sock.shutdown().await;
    drop(sock);
    Ok(())
}

async fn read_request(sock: &mut TcpStream) -> std::io::Result<Recorded> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut it = first.split(' ');
    let method = it.next().unwrap_or_default().to_owned();
    let path = it.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Ok(Recorded {
        method,
        path,
        headers,
        body,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        529 => "Overloaded",
        _ => "Status",
    }
}

/// Split `bytes` into hostile parts: sizes cycle 1..=7 so cuts land mid-line,
/// mid-JSON and mid-UTF-8 character.
pub fn hostile(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut size = 1;
    while i < bytes.len() {
        let end = (i + size).min(bytes.len());
        out.push(bytes[i..end].to_vec());
        i = end;
        size = size % 7 + 1;
    }
    out
}

/// A port with nothing listening on it (connection refused).
pub fn dead_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

/// Collect everything currently in the delta channel.
pub fn drain(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
    let mut v = Vec::new();
    while let Ok(s) = rx.try_recv() {
        v.push(s);
    }
    v
}
