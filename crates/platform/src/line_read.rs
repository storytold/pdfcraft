//! Reading a line-delimited protocol (the MCP server's stdin, the UI control channel) from an
//! untrusted peer without buffering more than a bound per line.

use std::io::{BufRead, Read};

/// What [`read_line_limited`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum LineRead {
    /// A line, now in the buffer (newline and a trailing `\r` stripped).
    Line,
    /// End of stream.
    End,
    /// The line is longer than the limit; at most `max + 1` bytes of it were read.
    TooLong,
}

/// Read one `\n`-terminated line into `buf`. At most `max` bytes may come before the `\n`, and a
/// `\r` before it counts toward them: a line ending in `\n` holds at most `max` payload bytes, one
/// ending in `\r\n` at most `max - 1`, and `max` payload bytes followed by `\r\n` are
/// [`LineRead::TooLong`].
pub fn read_line_limited(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> std::io::Result<LineRead> {
    buf.clear();
    // `take` bounds what is buffered for one line: never more than `max + 1` bytes.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    if reader.take(limit).read_until(b'\n', buf)? == 0 {
        return Ok(LineRead::End);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(LineRead::Line);
    }
    // No newline: either the limit was passed, or this is a last line before end of stream.
    Ok(if buf.len() > max { LineRead::TooLong } else { LineRead::Line })
}

/// Discard the rest of an overlong line up to `\n` or end of stream without buffering it.
pub fn drain_line(reader: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(());
        }
        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            reader.consume(pos + 1);
            return Ok(());
        }
        let len = chunk.len();
        reader.consume(len);
    }
}

#[cfg(test)]
mod tests {
    use super::{LineRead, drain_line, read_line_limited};

    #[test]
    fn lines_are_read_up_to_the_limit() {
        let mut buf = Vec::new();
        let mut input: &[u8] = b"aaaaaaaaaa\nbb\r\ncc";
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"aaaaaaaaaa", "a line exactly at the limit is accepted");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"bb", "CRLF is stripped");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"cc", "a last line without a newline is still read");
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::End, "then end of stream");
        // The `\r` of a CRLF ending counts toward the limit.
        let mut input: &[u8] = b"aaaaaaaaaa\r\n";
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::TooLong, "max payload bytes + CRLF");
        let mut input: &[u8] = b"aaaaaaaaa\r\n";
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"aaaaaaaaa", "max - 1 payload bytes + CRLF fit");
        let long = [b'x'; 100];
        let mut input: &[u8] = &long;
        assert_eq!(read_line_limited(&mut input, &mut buf, 10).unwrap(), LineRead::TooLong);
        assert_eq!(buf.len(), 11, "at most max + 1 bytes are buffered");
        assert_eq!(input.len(), 100 - 11, "the rest is left");
    }

    #[test]
    fn an_overlong_line_is_drained_and_the_next_one_read_cleanly() {
        let mut buf = Vec::new();
        let mut overlong: &[u8] = b"0123456789abcdef\nafter\n";
        assert_eq!(read_line_limited(&mut overlong, &mut buf, 10).unwrap(), LineRead::TooLong);
        drain_line(&mut overlong).unwrap();
        assert_eq!(read_line_limited(&mut overlong, &mut buf, 10).unwrap(), LineRead::Line);
        assert_eq!(buf, b"after", "the next line after an overlong line is read cleanly");
        let mut unterminated: &[u8] = b"0123456789abcdef";
        assert_eq!(read_line_limited(&mut unterminated, &mut buf, 10).unwrap(), LineRead::TooLong);
        drain_line(&mut unterminated).unwrap();
        assert_eq!(read_line_limited(&mut unterminated, &mut buf, 10).unwrap(), LineRead::End);
    }
}
