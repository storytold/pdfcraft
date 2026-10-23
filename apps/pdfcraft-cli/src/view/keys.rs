//! Keys from a terminal in raw mode: bytes in, keys out. Pure, and total over any bytes.
//!
//! The terminal's answers arrive among the keys: to graphics commands as `ESC _ G … ESC \` (the
//! kitty graphics protocol), and to `view`'s questions as CSI sequences (the cell size, device
//! attributes), late ones included. They are never keys, even cut in two: one that says an image
//! is missing is [`Key::Missing`], and the rest are dropped.

/// A key the pager knows. Anything else is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Enter,
    Escape,
    Backspace,
    /// Ctrl-C, which raw mode delivers as a byte instead of a signal.
    Interrupt,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    /// Not typed: the terminal's answer that image `id`, which it was asked to place, isn't there
    /// (it drops images to stay under its storage quota).
    Missing(u32),
}

/// The longest escape sequence worth waiting for; longer ones are dropped.
const MAX_SEQUENCE: usize = 16;
/// The longest graphics answer worth waiting for. kitty's are under 200 bytes.
pub(crate) const MAX_ANSWER: usize = 512;
/// How a graphics answer starts.
const ANSWER: &[u8] = b"\x1b_G";

/// Decode `bytes` into keys, and say how many bytes were used. An escape sequence cut off at the
/// end is left for the next read, whole: past its ESC it is no key, however long the rest takes
/// (the caller drops what never completes). Only a lone ESC at the end can be a key, the Escape
/// key, and is when `flush` (the user pressed Esc and nothing followed).
pub(crate) fn decode(bytes: &[u8], flush: bool) -> (Vec<Key>, usize) {
    let mut keys = Vec::new();
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        let rest = bytes.get(i..).unwrap_or_default();
        let (key, used) = match b {
            0x1b if rest.starts_with(ANSWER) => match answer(rest) {
                Some(found) => found,
                None => break,
            },
            // The start of what may be an answer waits too.
            0x1b if rest == b"\x1b_" => break,
            0x1b => match escape(rest) {
                Some(found) => found,
                None if flush && rest.len() == 1 => (Some(Key::Escape), 1),
                None => break,
            },
            b'\r' | b'\n' => (Some(Key::Enter), 1),
            0x7f | 0x08 => (Some(Key::Backspace), 1),
            0x03 => (Some(Key::Interrupt), 1),
            // Printable ASCII; the pager's keys are all ASCII, so other bytes, UTF-8 included, are
            // dropped one at a time.
            0x20..=0x7e => (Some(Key::Char(char::from(b))), 1),
            0x0c => (Some(Key::Char('\u{c}')), 1),
            _ => (None, 1),
        };
        keys.extend(key);
        i = i.saturating_add(used);
    }
    (keys, i)
}

/// The key an escape sequence at the start of `bytes` (which starts with ESC) stands for, and
/// its length. `None` when it is cut off.
fn escape(bytes: &[u8]) -> Option<(Option<Key>, usize)> {
    match bytes.get(1) {
        None => None,
        // CSI: parameters and intermediates, then a final byte in 0x40..=0x7e.
        Some(b'[') => {
            let body = bytes.get(2..).unwrap_or_default();
            let Some(end) = body.iter().position(|b| (0x40..=0x7e).contains(b)) else {
                return if body.len() >= MAX_SEQUENCE { Some((None, bytes.len())) } else { None };
            };
            let params = body.get(..end).unwrap_or_default();
            let key = match (params, body.get(end)) {
                (b"" | b"1", Some(b'A')) => Some(Key::Up),
                (b"" | b"1", Some(b'B')) => Some(Key::Down),
                (b"" | b"1", Some(b'C')) => Some(Key::Right),
                (b"" | b"1", Some(b'D')) => Some(Key::Left),
                (b"" | b"1", Some(b'H')) | (b"1" | b"7", Some(b'~')) => Some(Key::Home),
                (b"" | b"1", Some(b'F')) | (b"4" | b"8", Some(b'~')) => Some(Key::End),
                (b"5", Some(b'~')) => Some(Key::PageUp),
                (b"6", Some(b'~')) => Some(Key::PageDown),
                _ => None,
            };
            Some((key, end.saturating_add(3)))
        }
        // SS3, as some terminals send arrows, Home and End.
        Some(b'O') => {
            let key = match bytes.get(2)? {
                b'A' => Some(Key::Up),
                b'B' => Some(Key::Down),
                b'C' => Some(Key::Right),
                b'D' => Some(Key::Left),
                b'H' => Some(Key::Home),
                b'F' => Some(Key::End),
                _ => None,
            };
            Some((key, 3))
        }
        // A string terminator on its own: the end of something cut off, never a key.
        Some(b'\\') => Some((None, 2)),
        // Esc, then another key typed quickly enough to arrive in the same read.
        Some(_) => Some((Some(Key::Escape), 1)),
    }
}

/// The graphics answer at the start of `bytes` (which starts with [`ANSWER`]) and its length,
/// through its `ESC \`: [`Key::Missing`] when it says an image isn't there, nothing otherwise.
/// `None` while it is cut off; one longer than [`MAX_ANSWER`] is dropped whole.
fn answer(bytes: &[u8]) -> Option<(Option<Key>, usize)> {
    let body = bytes.get(ANSWER.len()..).unwrap_or_default();
    let Some(end) = body.windows(2).position(|w| w == b"\x1b\\") else {
        return (bytes.len() > MAX_ANSWER).then_some((None, bytes.len()));
    };
    let used = ANSWER.len().saturating_add(end).saturating_add(2);
    // `i=<id>,…;ENOENT:<message>`, as kitty answers a placement of an image it doesn't have.
    let text = std::str::from_utf8(body.get(..end).unwrap_or_default()).unwrap_or_default();
    let (control, message) = text.split_once(';').unwrap_or((text, ""));
    let id = control.split(',').find_map(|kv| kv.strip_prefix("i=")?.parse::<u32>().ok());
    Some((id.filter(|_| message.starts_with("ENOENT")).map(Key::Missing), used))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(bytes: &[u8]) -> Vec<Key> {
        let (keys, used) = decode(bytes, true);
        assert_eq!(used, bytes.len());
        keys
    }

    #[test]
    fn plain_keys() {
        assert_eq!(
            keys(b" bq7G\r\n\x7f\x03"),
            [Key::Char(' '), Key::Char('b'), Key::Char('q'), Key::Char('7'), Key::Char('G'), Key::Enter, Key::Enter, Key::Backspace, Key::Interrupt]
        );
    }

    #[test]
    fn arrows_pages_home_and_end() {
        assert_eq!(keys(b"\x1b[A\x1b[B\x1b[C\x1b[D\x1bOA\x1bOD"), [Key::Up, Key::Down, Key::Right, Key::Left, Key::Up, Key::Left]);
        assert_eq!(keys(b"\x1b[5~\x1b[6~"), [Key::PageUp, Key::PageDown]);
        assert_eq!(keys(b"\x1b[H\x1b[1~\x1b[7~\x1bOH"), [Key::Home; 4]);
        assert_eq!(keys(b"\x1b[F\x1b[4~\x1b[8~\x1bOF"), [Key::End; 4]);
    }

    #[test]
    fn unknown_sequences_and_bytes_are_dropped() {
        // F5, Shift+Up, a mouse report, UTF-8 "é", a NUL.
        assert_eq!(keys(b"\x1b[15~\x1b[1;2A\x1b[<0;3;4M\xc3\xa9\0j"), [Key::Char('j')]);
    }

    #[test]
    fn a_sequence_cut_off_waits_for_the_next_read() {
        assert_eq!(decode(b"j\x1b", false), (vec![Key::Char('j')], 1));
        assert_eq!(decode(b"j\x1b[", false), (vec![Key::Char('j')], 1));
        assert_eq!(decode(b"\x1b[6", false), (vec![], 0));
        assert_eq!(decode(b"\x1bO", false), (vec![], 0));
        // Nothing followed a lone ESC: it was the Esc key.
        assert_eq!(decode(b"\x1b", true), (vec![Key::Escape], 1));
        assert_eq!(decode(b"j\x1b", true), (vec![Key::Char('j'), Key::Escape], 2));
        // Past the ESC, a sequence waits for the rest even when nothing more came yet: it is no
        // Esc (which quits), nor its bytes keys.
        assert_eq!(decode(b"j\x1b[", true), (vec![Key::Char('j')], 1));
        assert_eq!(decode(b"\x1b[6;", true), (vec![], 0));
        assert_eq!(decode(b"\x1bO", true), (vec![], 0));
        // Esc typed before another key.
        assert_eq!(keys(b"\x1bq"), [Key::Escape, Key::Char('q')]);
    }

    #[test]
    fn a_reply_cut_anywhere_past_its_esc_is_never_keys() {
        // A late cell size and device attributes, as the terminal answers `view`'s question, and
        // both together; each cut in two, the first part read alone with nothing more to come.
        // (Not cut just past an ESC: an ESC alone with nothing after it is the Esc key, which is
        // what the pager waits a while to tell.)
        let cell = b"\x1b[6;25;14t".as_slice();
        let attributes = b"\x1b[?62;22c".as_slice();
        for reply in [cell, attributes, &[cell, attributes].concat()] {
            for cut in (2..reply.len()).filter(|&cut| reply[cut - 1] != 0x1b) {
                let (first, used) = decode(&reply[..cut], true);
                let pending = [&reply[used..cut], &reply[cut..]].concat();
                let (rest, all) = decode(&pending, true);
                assert_eq!((first, rest, all), (vec![], vec![], pending.len()), "{:?} cut at {cut}", String::from_utf8_lossy(reply));
            }
        }
        // A key cut in two, as over SSH, is the key.
        let page_down = b"\x1b[6~".as_slice();
        for cut in 2..page_down.len() {
            let (first, used) = decode(&page_down[..cut], true);
            let pending = [&page_down[used..cut], &page_down[cut..]].concat();
            assert_eq!((first, decode(&pending, true)), (vec![], (vec![Key::PageDown], pending.len())), "cut at {cut}");
        }
    }

    #[test]
    fn graphics_answers_are_never_keys() {
        // kitty's answer to placing an image it dropped, between two keys.
        let missing = b"j\x1b_Gi=3000,p=1;ENOENT:Put command refers to non-existent image with id: 3000 and number: 0\x1b\\k";
        assert_eq!(keys(missing), [Key::Char('j'), Key::Missing(3000), Key::Char('k')]);
        // Any other answer is dropped, an OK or another error alike: its ESC isn't Esc, nor its
        // letters keys (`q` would quit).
        assert_eq!(keys(b"\x1b_Gi=7;OK\x1b\\\x1b_Gi=7;EINVAL:quit\x1b\\\x1b_G;ENOENT:no id\x1b\\"), []);
        assert_eq!(keys(b"\x1b_Gi=x;ENOENT:\x1b\\\x1b\\"), []);
        // Cut off anywhere, it waits for the rest even when nothing more came yet.
        let answer = &missing[1..missing.len() - 1];
        for end in ANSWER.len()..answer.len() {
            assert_eq!(decode(&answer[..end], true), (vec![], 0), "cut at {end}");
        }
        // Too long to be an answer: dropped whole.
        let long = [b"\x1b_Gi=1;".as_slice(), &[b'q'; MAX_ANSWER]].concat();
        assert_eq!(decode(&long, false), (vec![], long.len()));
        // Its first bytes wait for the rest, even when nothing more came yet.
        assert_eq!(decode(b"j\x1b_", false), (vec![Key::Char('j')], 1));
        assert_eq!(decode(b"j\x1b_", true), (vec![Key::Char('j')], 1));
        // Esc and _ typed together, then another key: Esc, and the keys.
        assert_eq!(keys(b"\x1b_j"), [Key::Escape, Key::Char('_'), Key::Char('j')]);
    }

    #[test]
    fn an_endless_sequence_is_dropped() {
        let long = [b"\x1b[".as_slice(), &[b'1'; 40]].concat();
        assert_eq!(decode(&long, false), (vec![], long.len()));
    }

    proptest::proptest! {
        #[test]
        fn any_bytes_decode(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200), flush in proptest::prelude::any::<bool>()) {
            let (_, used) = decode(&bytes, flush);
            proptest::prop_assert!(used <= bytes.len());
            if flush {
                // All of it, but for an escape sequence still arriving past its ESC: never a lone
                // ESC, and short unless it is a graphics answer.
                let rest = &bytes[used..];
                proptest::prop_assert!(
                    rest.is_empty() || (rest.len() > 1 && rest[0] == 0x1b && (rest.starts_with(ANSWER) || rest.len() < MAX_SEQUENCE + 2))
                );
            }
        }
    }
}
