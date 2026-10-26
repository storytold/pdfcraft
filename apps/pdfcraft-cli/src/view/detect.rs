//! Does this terminal speak the kitty graphics protocol, and how big are its cells? Pure
//! decisions from what the environment says and from what the terminal answers; asking it is the
//! caller's job.

/// What the environment says, without asking the terminal: kitty sets `TERM=xterm-kitty` and
/// `KITTY_WINDOW_ID`, Ghostty `TERM=xterm-ghostty` and `TERM_PROGRAM=ghostty`.
pub(crate) fn env_says_kitty(term: Option<&str>, term_program: Option<&str>, kitty_window_id: bool) -> bool {
    kitty_window_id || term.is_some_and(term_name_says_kitty) || term_program.is_some_and(|p| p.eq_ignore_ascii_case("ghostty"))
}

/// A terminal name (`TERM`, or tmux's `#{client_termname}`) of a terminal known to speak it.
pub(crate) fn term_name_says_kitty(name: &str) -> bool {
    matches!(name.trim(), "xterm-kitty" | "xterm-ghostty")
}

/// The query the protocol recommends: a 1×1 RGB image the terminal checks but doesn't keep
/// (`a=q`, id 31), then a Primary Device Attributes request, which every terminal answers. A
/// graphics reply before the attributes reply means the protocol is supported.
pub(crate) const QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

/// The most reply bytes worth reading: both replies together are a few dozen.
pub(crate) const MAX_REPLY: usize = 1024;

/// What the terminal has answered to [`QUERY`] so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reply {
    /// A graphics reply for id 31 came before the device attributes reply.
    pub(crate) graphics: bool,
    /// The device attributes reply came: nothing more is coming.
    pub(crate) done: bool,
}

/// Read the replies in `bytes`. Anything else in there (keys the user typed, garbage) is ignored.
pub(crate) fn parse_reply(bytes: &[u8]) -> Reply {
    let attributes = find_attributes(bytes);
    let graphics = find_graphics(bytes).is_some_and(|g| attributes.is_none_or(|a| g < a));
    Reply { graphics, done: attributes.is_some() }
}

/// Asks for the cell size in pixels (XTWINOPS 16, from xterm's control sequences), for a
/// terminal whose tty reports no pixel size, then for the device attributes again: a terminal
/// that doesn't know the first still answers the second, so the wait ends at once.
pub(crate) const CELL_QUERY: &[u8] = b"\x1b[16t\x1b[c";

/// What the terminal has answered to [`CELL_QUERY`] so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CellReply {
    /// The cell's width and height in pixels, from an `ESC [ 6 ; height ; width t` before the
    /// device attributes reply.
    pub(crate) cell: Option<(u16, u16)>,
    /// The device attributes reply came: nothing more is coming.
    pub(crate) done: bool,
}

/// Read the replies to [`CELL_QUERY`] in `bytes`. Anything else in there is ignored.
pub(crate) fn parse_cell_reply(bytes: &[u8]) -> CellReply {
    let attributes = find_attributes(bytes);
    let before = bytes.get(..attributes.unwrap_or(bytes.len())).unwrap_or_default();
    CellReply { cell: find_cell(before), done: attributes.is_some() }
}

/// The first complete `ESC [ 6 ; height ; width t` in `bytes`, as (width, height). A size of 0,
/// or past `u16`, is no answer.
fn find_cell(bytes: &[u8]) -> Option<(u16, u16)> {
    let mut from = 0;
    while let Some(start) = find(bytes, b"\x1b[6;", from) {
        let params = start.saturating_add(4);
        let rest = bytes.get(params..).unwrap_or_default();
        let end = rest.iter().position(|b| !(b.is_ascii_digit() || *b == b';'))?;
        if rest.get(end) == Some(&b't') {
            let number = |n: &[u8]| std::str::from_utf8(n).ok()?.parse::<u16>().ok().filter(|v| *v > 0);
            let mut numbers = rest.get(..end).unwrap_or_default().split(|b| *b == b';');
            if let (Some(h), Some(w), None) = (numbers.next(), numbers.next(), numbers.next())
                && let (Some(h), Some(w)) = (number(h), number(w))
            {
                return Some((w, h));
            }
        }
        from = params;
    }
    None
}

/// Where a complete graphics reply for id 31 (`ESC _G … i=31 … ESC \`) starts.
fn find_graphics(bytes: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(start) = find(bytes, b"\x1b_G", from) {
        let body = start.saturating_add(3);
        let end = find(bytes, b"\x1b\\", body)?;
        let control = bytes.get(body..end)?.split(|b| *b == b';').next().unwrap_or_default();
        if control.split(|b| *b == b',').any(|key| key == b"i=31") {
            return Some(start);
        }
        from = end;
    }
    None
}

/// Where a complete Primary Device Attributes reply (`ESC [ ? <digits and ;> c`) starts.
fn find_attributes(bytes: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(start) = find(bytes, b"\x1b[?", from) {
        let params = start.saturating_add(3);
        let rest = bytes.get(params..).unwrap_or_default();
        match rest.iter().position(|b| !(b.is_ascii_digit() || *b == b';')) {
            Some(i) if rest.get(i) == Some(&b'c') => return Some(start),
            Some(_) => from = params,
            None => return None,
        }
    }
    None
}

/// The first `needle` in `haystack` at or after `from`.
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let rest = haystack.get(from..)?;
    rest.windows(needle.len()).position(|w| w == needle).map(|i| i.saturating_add(from))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &[u8] = b"\x1b_Gi=31;OK\x1b\\";
    const ATTRIBUTES: &[u8] = b"\x1b[?62;c";

    #[test]
    fn the_environment_names_kitty_and_ghostty() {
        assert!(env_says_kitty(Some("xterm-kitty"), None, false));
        assert!(env_says_kitty(Some("xterm-ghostty"), None, false));
        assert!(env_says_kitty(None, Some("ghostty"), false));
        assert!(env_says_kitty(Some("xterm-256color"), None, true));
        assert!(!env_says_kitty(Some("xterm-256color"), Some("Apple_Terminal"), false));
        assert!(!env_says_kitty(Some("tmux-256color"), Some("tmux"), false));
        assert!(!env_says_kitty(None, None, false));
    }

    #[test]
    fn a_graphics_reply_before_the_attributes_means_supported() {
        assert_eq!(parse_reply(&[OK, ATTRIBUTES].concat()), Reply { graphics: true, done: true });
        // An error is still an answer from a terminal that knows the protocol.
        let error = b"\x1b_Gi=31;ENOTSUPPORTED:no\x1b\\";
        assert_eq!(parse_reply(&[error, ATTRIBUTES].concat()), Reply { graphics: true, done: true });
    }

    #[test]
    fn the_attributes_alone_mean_unsupported() {
        assert_eq!(parse_reply(ATTRIBUTES), Reply { graphics: false, done: true });
        assert_eq!(parse_reply(b"\x1b[?1;2c"), Reply { graphics: false, done: true });
        // A graphics reply after the attributes is not this terminal answering the query.
        assert_eq!(parse_reply(&[ATTRIBUTES, OK].concat()), Reply { graphics: false, done: true });
    }

    #[test]
    fn partial_replies_wait_for_more() {
        assert_eq!(parse_reply(b""), Reply::default());
        assert_eq!(parse_reply(OK), Reply { graphics: true, done: false });
        assert_eq!(parse_reply(b"\x1b_Gi=31;O"), Reply::default());
        assert_eq!(parse_reply(&[OK, b"\x1b[?62;"].concat()), Reply { graphics: true, done: false });
        assert_eq!(parse_reply(b"\x1b[?"), Reply::default());
    }

    #[test]
    fn other_bytes_are_ignored() {
        // Keys typed while waiting, another image's reply, a cursor report, a stray ESC.
        let noisy = [b"jk\x1b".as_slice(), b"\x1b_Gi=7;OK\x1b\\", b"\x1b[12;1R", b"\x1b[?x", OK, b"q", ATTRIBUTES].concat();
        assert_eq!(parse_reply(&noisy), Reply { graphics: true, done: true });
        let other_id = [b"\x1b_Gi=310;OK\x1b\\".as_slice(), ATTRIBUTES].concat();
        assert_eq!(parse_reply(&other_id), Reply { graphics: false, done: true });
    }

    #[test]
    fn garbage_never_panics() {
        for bytes in [&b"\x1b"[..], b"\x1b_", b"\x1b_G", b"\x1b_G\x1b\\", b"\x1b_G;\x1b\\", b"\x1b[", b"\x1b[?c", &[0xff; 64]] {
            let _ = parse_reply(bytes);
        }
        assert_eq!(parse_reply(b"\x1b[?c"), Reply { graphics: false, done: true });
    }

    #[test]
    fn the_cell_size_comes_before_the_attributes() {
        // kitty's answer: cells 14 px wide and 25 tall.
        let kitty = [b"\x1b[6;25;14t".as_slice(), b"\x1b[?62;52;c"].concat();
        assert_eq!(parse_cell_reply(&kitty), CellReply { cell: Some((14, 25)), done: true });
        // A terminal that doesn't know the question answers the attributes alone.
        assert_eq!(parse_cell_reply(ATTRIBUTES), CellReply { cell: None, done: true });
        // An answer after the attributes is not this terminal answering the query.
        assert_eq!(parse_cell_reply(&[ATTRIBUTES, b"\x1b[6;25;14t"].concat()), CellReply { cell: None, done: true });
    }

    #[test]
    fn a_cell_size_cut_off_waits_for_more() {
        assert_eq!(parse_cell_reply(b""), CellReply::default());
        assert_eq!(parse_cell_reply(b"\x1b[6;25;1"), CellReply::default());
        assert_eq!(parse_cell_reply(b"\x1b[6;25;14t"), CellReply { cell: Some((14, 25)), done: false });
    }

    #[test]
    fn a_cell_size_that_makes_no_sense_is_no_answer() {
        for odd in [&b"\x1b[6;0;14t"[..], b"\x1b[6;25;0t", b"\x1b[6;25t", b"\x1b[6;25;14;3t", b"\x1b[6;;14t", b"\x1b[6;65536;14t", b"\x1b[6;25;14x"] {
            assert_eq!(parse_cell_reply(&[odd, ATTRIBUTES].concat()).cell, None, "{odd:?}");
        }
        // Keys typed while waiting, other reports, a stray ESC: ignored, the answer still found.
        let noisy = [b"j\x1b".as_slice(), b"\x1b[4;2100;3444t", b"\x1b[6;0;0t", b"\x1b[6;25;14t", b"q", ATTRIBUTES].concat();
        assert_eq!(parse_cell_reply(&noisy), CellReply { cell: Some((14, 25)), done: true });
    }

    proptest::proptest! {
        #[test]
        fn any_bytes_parse(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300)) {
            let _ = parse_reply(&bytes);
            let _ = parse_cell_reply(&bytes);
        }

        #[test]
        fn any_cell_size_is_read_back(w in 1u16.., h in 1u16..) {
            let reply = format!("\x1b[6;{h};{w}t\x1b[?62;c");
            proptest::prop_assert_eq!(parse_cell_reply(reply.as_bytes()), CellReply { cell: Some((w, h)), done: true });
        }
    }
}
