//! An incremental decoder for `text/event-stream` (the WHATWG HTML "server-sent events"
//! interpretation rules). Bytes go in as the transport delivers them, split anywhere; complete
//! events come out. Lines and event data are capped, so a hostile stream can't grow memory.

/// Longest line kept (1 MiB). A longer line is dropped, and the event it belongs to with it.
pub const MAX_LINE: usize = 1 << 20;
/// Most data one event may carry (1 MiB).
pub const MAX_EVENT_DATA: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, or `message` when there was none.
    pub event: String,
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SseItem {
    Event(SseEvent),
    /// An event was dropped because a line or its data passed the caps.
    Oversized,
}

#[derive(Default)]
pub struct SseDecoder {
    line: Vec<u8>,
    /// The current line passed [`MAX_LINE`]; skip to its end.
    skipping_line: bool,
    /// The last byte was CR, so a LF right after it ends the same line.
    after_cr: bool,
    event: String,
    data: String,
    has_data: bool,
    /// The current event lost a line or data to the caps; drop it when it ends.
    spoiled: bool,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes; complete events are appended to `out`.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<SseItem>) {
        for &b in bytes {
            if std::mem::take(&mut self.after_cr) && b == b'\n' {
                continue;
            }
            match b {
                b'\n' => self.end_line(out),
                b'\r' => {
                    self.end_line(out);
                    self.after_cr = true;
                }
                _ if self.skipping_line => {}
                _ if self.line.len() >= MAX_LINE => {
                    self.line.clear();
                    self.skipping_line = true;
                    self.spoil(out);
                }
                _ => self.line.push(b),
            }
        }
    }

    /// The stream ended. An event not yet ended by a blank line is discarded, as the spec says.
    pub fn finish(&mut self) {
        *self = Self::default();
    }

    fn spoil(&mut self, out: &mut Vec<SseItem>) {
        if !self.spoiled {
            self.spoiled = true;
            out.push(SseItem::Oversized);
        }
    }

    fn end_line(&mut self, out: &mut Vec<SseItem>) {
        if std::mem::take(&mut self.skipping_line) {
            return;
        }
        let line = std::mem::take(&mut self.line);
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        let line = String::from_utf8_lossy(&line);
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_ref(), ""),
        };
        match field {
            "event" => {
                self.event.clear();
                self.event.push_str(crate::text::prefix(value, 256));
            }
            "data" => {
                if self.spoiled {
                    return;
                }
                if self.data.len().saturating_add(value.len()).saturating_add(1) > MAX_EVENT_DATA {
                    self.data.clear();
                    self.spoil(out);
                    return;
                }
                self.data.push_str(value);
                self.data.push('\n');
                self.has_data = true;
            }
            // `id` and `retry` steer reconnection, which the assistant never does.
            _ => {}
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseItem>) {
        let event = std::mem::take(&mut self.event);
        let mut data = std::mem::take(&mut self.data);
        let has_data = std::mem::take(&mut self.has_data);
        if std::mem::take(&mut self.spoiled) || !has_data {
            return;
        }
        if data.ends_with('\n') {
            data.pop();
        }
        let event = if event.is_empty() { "message".to_owned() } else { event };
        out.push(SseItem::Event(SseEvent { event, data }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(chunks: &[&[u8]]) -> Vec<SseItem> {
        let mut d = SseDecoder::new();
        let mut out = Vec::new();
        for c in chunks {
            d.push(c, &mut out);
        }
        d.finish();
        out
    }

    fn ev(event: &str, data: &str) -> SseItem {
        SseItem::Event(SseEvent { event: event.into(), data: data.into() })
    }

    #[test]
    fn spec_examples() {
        // Fields, comments, multi-line data, missing space after the colon, CR and CRLF endings.
        let out = decode(&[b": comment\nevent: ping\ndata: {\"type\":\"ping\"}\n\ndata:a\ndata: b\r\n\r\nevent:x\rdata\r\r"]);
        assert_eq!(out, vec![ev("ping", "{\"type\":\"ping\"}"), ev("message", "a\nb"), ev("x", "")]);
        // No data lines: nothing is dispatched. An unterminated event at the end is dropped.
        assert_eq!(decode(&[b"event: a\n\nid: 1\n\ndata: tail"]), vec![]);
    }

    #[test]
    fn crlf_split_across_chunks_is_one_line_end() {
        assert_eq!(decode(&[b"data: a\r", b"\n\r", b"\n"]), vec![ev("message", "a")]);
    }

    #[test]
    fn oversized_lines_and_data_drop_their_event_only() {
        let mut long = b"data: ".to_vec();
        long.extend(std::iter::repeat_n(b'x', MAX_LINE + 10));
        long.extend_from_slice(b"\ndata: more\n\ndata: next\n\n");
        assert_eq!(decode(&[&long]), vec![SseItem::Oversized, ev("message", "next")]);

        // A line that isn't data is capped too: it is never buffered whole.
        let mut name = b"event: ".to_vec();
        name.extend(std::iter::repeat_n(b'n', MAX_LINE + 10));
        name.extend_from_slice(b"\ndata: x\n\n");
        assert_eq!(decode(&[&name]), vec![SseItem::Oversized]);

        let line = format!("data: {}\n", "y".repeat(MAX_LINE / 2));
        let many = format!("{}{}{}\ndata: ok\n\n", line, line, line);
        assert_eq!(decode(&[many.as_bytes()]), vec![SseItem::Oversized, ev("message", "ok")]);
    }

    #[test]
    fn invalid_utf8_is_replaced_not_fatal() {
        assert_eq!(decode(&[b"data: \xff\xfe\n\n"]), vec![ev("message", "\u{fffd}\u{fffd}")]);
    }

    proptest::proptest! {
        /// Where the transport splits the stream never changes the events.
        #[test]
        fn split_invariance(body in proptest::collection::vec(proptest::sample::select(vec![b'a', b':', b' ', b'\n', b'\r', b'd', b'e', 0xc3, 0xa9]), 0..200), cut in 0usize..200) {
            let cut = cut.min(body.len());
            let (a, b) = body.split_at(cut);
            proptest::prop_assert_eq!(decode(&[&body]), decode(&[a, b]));
        }

        #[test]
        fn arbitrary_bytes_never_panic(body in proptest::collection::vec(proptest::num::u8::ANY, 0..2000)) {
            let _ = decode(&[&body]);
        }
    }
}
