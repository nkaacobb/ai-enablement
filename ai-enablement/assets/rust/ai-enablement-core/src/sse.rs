//! Server-Sent Events, read the way EventSource does (`core/transport.js` createSseParser): the fields `event` and
//! `data`, a blank line dispatches, lines starting with `:` are comments, CR, LF and CRLF all end a line, and a record
//! may be split across chunks anywhere — a lone CR at the end of a chunk waits for the next one. Bytes are decoded as
//! UTF-8 across chunk boundaries, invalid sequences becoming U+FFFD (TextDecoder's default).

/// One dispatched record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseRecord {
    /// The `event` field, or `message`.
    pub event: String,
    /// The `data` lines joined by `\n`.
    pub data: String,
}

/// The incremental parser.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: String,
    event: String,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    fn dispatch(&mut self, out: &mut Vec<SseRecord>) {
        if !self.data.is_empty() {
            let event = if self.event.is_empty() { "message".to_string() } else { std::mem::take(&mut self.event) };
            out.push(SseRecord { event, data: self.data.join("\n") });
        }
        self.event.clear();
        self.data.clear();
    }

    fn line(&mut self, text: &str, out: &mut Vec<SseRecord>) {
        if text.is_empty() {
            self.dispatch(out);
            return;
        }
        if text.starts_with(':') {
            return;
        }
        let (field, value) = match text.find(':') {
            Some(i) => (&text[..i], &text[i + 1..]),
            None => (text, ""),
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = value.to_string(),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
    }

    /// Add decoded text; returns the records it completed.
    pub fn push(&mut self, chunk: &str) -> Vec<SseRecord> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        let buf = std::mem::take(&mut self.buf);
        let bytes = buf.as_bytes();
        let mut start = 0;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    self.line(&buf[start..i], &mut out);
                    i += 1;
                    start = i;
                }
                b'\r' => {
                    if i + 1 == bytes.len() {
                        break; // maybe the first half of CRLF: wait for the next chunk
                    }
                    self.line(&buf[start..i], &mut out);
                    i += if bytes[i + 1] == b'\n' { 2 } else { 1 };
                    start = i;
                }
                _ => i += 1,
            }
        }
        self.buf = buf[start..].to_string();
        out
    }

    /// The stream ended: the last line and record.
    pub fn end(&mut self) -> Vec<SseRecord> {
        let mut out = Vec::new();
        let rest = std::mem::take(&mut self.buf);
        if !rest.is_empty() {
            self.line(&rest, &mut out);
        }
        self.dispatch(&mut out);
        out
    }
}

/// UTF-8 decoding across chunks (TextDecoder with `stream: true`).
#[derive(Debug, Default)]
pub struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode what can be decoded; an incomplete sequence at the end waits for the next chunk.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        let mut input: &[u8] = &self.pending;
        loop {
            match std::str::from_utf8(input) {
                Ok(s) => {
                    out.push_str(s);
                    input = &[];
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&input[..valid]).expect("valid prefix"));
                    match e.error_len() {
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            input = &input[valid + bad..];
                        }
                        None => {
                            input = &input[valid..];
                            break; // incomplete at the end
                        }
                    }
                }
            }
        }
        self.pending = input.to_vec();
        out
    }

    /// The stream ended: an incomplete sequence becomes U+FFFD.
    pub fn end(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            "\u{FFFD}".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(chunks: &[&str]) -> Vec<SseRecord> {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(p.push(c));
        }
        out.extend(p.end());
        out
    }

    #[test]
    fn records_comments_and_line_endings() {
        let r =
            all(&[": open\n\nevent: delta\ndata: {\"text\":\"a\"}\n\n", "data: one\r", "\ndata: two\r\r\ndata:x\n\n"]);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], SseRecord { event: "delta".into(), data: "{\"text\":\"a\"}".into() });
        assert_eq!(r[1], SseRecord { event: "message".into(), data: "one\ntwo".into() });
        assert_eq!(r[2].data, "x");
    }

    #[test]
    fn last_record_without_blank_line() {
        let r = all(&["data: end"]);
        assert_eq!(r, vec![SseRecord { event: "message".into(), data: "end".into() }]);
    }

    #[test]
    fn utf8_split_across_chunks() {
        let bytes = "Grüße 🙂".as_bytes();
        let mut d = Utf8Stream::new();
        let mut s = String::new();
        for b in bytes {
            s.push_str(&d.push(&[*b]));
        }
        s.push_str(&d.end());
        assert_eq!(s, "Grüße 🙂");
        let mut d = Utf8Stream::new();
        assert_eq!(d.push(&[b'a', 0xff, b'b']), "a\u{FFFD}b");
        assert_eq!(d.push(&[0xe2, 0x82]), "");
        assert_eq!(d.end(), "\u{FFFD}");
    }
}
