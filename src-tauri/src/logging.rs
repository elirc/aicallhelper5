//! Rotating file log + in-memory tail for "Copy diagnostics" + panic hook
//! (spec §2, §17). Never log secrets or prompt/profile text: callers log
//! codes, phases and ids only; the panic payload is redacted and truncated.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use callcore_shell::diagnostics::redact;

pub const LOG_FILE: &str = "aica.log";
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Total files kept: `aica.log`, `aica.1.log`, `aica.2.log`.
pub const KEEP_FILES: usize = 3;
pub const TAIL_LINES: usize = 200;

struct State {
    dir: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    tail: VecDeque<String>,
    partial: String,
}

/// Thread-safe sink shared by the tracing writer, the panic hook and
/// diagnostics.
pub struct LogSink {
    state: Mutex<State>,
    max_bytes: u64,
}

fn rotated(dir: &Path, i: usize) -> PathBuf {
    if i == 0 {
        dir.join(LOG_FILE)
    } else {
        dir.join(format!("aica.{i}.log"))
    }
}

impl LogSink {
    /// `dir = None` keeps only the in-memory tail (tests, or when the log
    /// directory can't be created).
    pub fn new(dir: Option<PathBuf>) -> Arc<Self> {
        Self::with_max_bytes(dir, MAX_FILE_BYTES)
    }

    pub fn with_max_bytes(dir: Option<PathBuf>, max_bytes: u64) -> Arc<Self> {
        let mut st = State {
            dir: None,
            file: None,
            size: 0,
            tail: VecDeque::new(),
            partial: String::new(),
        };
        if let Some(d) = dir {
            if fs::create_dir_all(&d).is_ok() {
                let path = rotated(&d, 0);
                if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
                    st.size = f.metadata().map(|m| m.len()).unwrap_or(0);
                    st.file = Some(f);
                    st.dir = Some(d);
                }
            }
        }
        Arc::new(Self {
            state: Mutex::new(st),
            max_bytes,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Last lines written (for diagnostics).
    pub fn tail(&self) -> Vec<String> {
        self.lock().tail.iter().cloned().collect()
    }

    pub fn write_bytes(&self, buf: &[u8]) {
        let mut st = self.lock();
        let text = String::from_utf8_lossy(buf);
        st.partial.push_str(&text);
        while let Some(i) = st.partial.find('\n') {
            let line: String = st.partial.drain(..=i).collect();
            let line = line.trim_end().to_string();
            if st.tail.len() >= TAIL_LINES {
                st.tail.pop_front();
            }
            st.tail.push_back(line);
        }
        if st.size + buf.len() as u64 > self.max_bytes && st.size > 0 {
            Self::rotate(&mut st);
        }
        if let Some(f) = st.file.as_mut() {
            if f.write_all(buf).is_ok() {
                st.size += buf.len() as u64;
            }
        }
    }

    fn rotate(st: &mut State) {
        let Some(dir) = st.dir.clone() else { return };
        st.file = None; // close before renaming (Windows)
        let _ = fs::remove_file(rotated(&dir, KEEP_FILES - 1));
        for i in (0..KEEP_FILES - 1).rev() {
            let _ = fs::rename(rotated(&dir, i), rotated(&dir, i + 1));
        }
        st.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(rotated(&dir, 0))
            .ok();
        st.size = 0;
    }
}

/// `MakeWriter` adapter for tracing-subscriber.
#[derive(Clone)]
pub struct SinkWriter(pub Arc<LogSink>);

impl Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_bytes(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SinkWriter {
    type Writer = SinkWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Install the global tracing subscriber (INFO+, no ANSI). Safe to call once.
pub fn init_tracing(sink: Arc<LogSink>) {
    let _ = tracing_subscriber::fmt()
        .with_writer(SinkWriter(sink))
        .with_ansi(false)
        .with_target(true)
        .with_max_level(tracing::Level::INFO)
        .try_init();
}

/// Render a panic for the log: location + redacted, truncated payload.
/// (Takes the parts instead of the hook-info type so it builds on the
/// workspace MSRV.)
pub fn panic_line(
    payload: &(dyn std::any::Any + Send),
    location: Option<&std::panic::Location<'_>>,
) -> String {
    let payload = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string payload>".into());
    let payload: String = redact(&payload).chars().take(200).collect();
    let loc = location
        .map(|l| format!("{}:{}", l.file(), l.line()))
        .unwrap_or_default();
    let thread = std::thread::current().name().unwrap_or("?").to_string();
    format!("PANIC thread={thread} at {loc}: {payload}")
}

/// Log panics to the file (and keep the default stderr hook in debug).
pub fn install_panic_hook(sink: Arc<LogSink>) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let line = panic_line(info.payload(), info.location());
        sink.write_bytes(format!("{line}\n").as_bytes());
        if cfg!(debug_assertions) {
            default(info);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_last_lines() {
        let sink = LogSink::new(None);
        for i in 0..(TAIL_LINES + 10) {
            sink.write_bytes(format!("line {i}\n").as_bytes());
        }
        let t = sink.tail();
        assert_eq!(t.len(), TAIL_LINES);
        assert_eq!(t.last().unwrap(), &format!("line {}", TAIL_LINES + 9));
    }

    #[test]
    fn partial_writes_join_into_one_line() {
        let sink = LogSink::new(None);
        sink.write_bytes(b"hello ");
        sink.write_bytes(b"world\nnext");
        assert_eq!(sink.tail(), vec!["hello world".to_string()]);
    }

    #[test]
    fn rotates_by_size_and_keeps_three_files() {
        let dir = tempfile::tempdir().unwrap();
        let sink = LogSink::with_max_bytes(Some(dir.path().to_path_buf()), 100);
        let line = [b'x'; 59];
        for _ in 0..20 {
            sink.write_bytes(&line);
            sink.write_bytes(b"\n");
        }
        drop(sink);
        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["aica.1.log", "aica.2.log", "aica.log"]);
        for n in &names {
            let len = fs::metadata(dir.path().join(n)).unwrap().len();
            assert!(len <= 100, "{n} is {len} bytes");
        }
    }

    #[test]
    fn panic_line_redacts_keys() {
        let payload: Box<dyn std::any::Any + Send> =
            Box::new(String::from("bad key sk-ant-api03-SECRETSECRET"));
        let line = panic_line(payload.as_ref(), None);
        assert!(line.starts_with("PANIC"), "{line}");
        assert!(!line.contains("SECRETSECRET"), "{line}");
        let long: Box<dyn std::any::Any + Send> = Box::new("y ".repeat(500));
        assert!(panic_line(long.as_ref(), None).len() < 300);
    }
}
