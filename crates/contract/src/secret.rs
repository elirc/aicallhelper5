use std::fmt;

/// An API key (or any secret) in memory.
///
/// Deliberately has NO `Display`, NO `Serialize`, and a redacting `Debug`, so a
/// key can never leak through `{:?}`, a log line, an error message, a panic or a
/// view sent to the page. Call [`Secret::expose`] only at the exact point the
/// key goes onto the wire (a header / WebSocket subprotocol).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// The raw key. Only for building a request header.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // Best-effort wipe of the heap buffer.
        // SAFETY: we overwrite bytes of our own String with zeros, which is
        // valid UTF-8, then the String is dropped.
        unsafe {
            for b in self.0.as_bytes_mut() {
                std::ptr::write_volatile(b, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_key() {
        let s = Secret::new("sk-ant-very-secret");
        let dbg = format!("{s:?} {:?}", Some(&s));
        assert!(!dbg.contains("very-secret"), "{dbg}");
    }
}
