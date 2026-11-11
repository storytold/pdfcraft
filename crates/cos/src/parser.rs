//! Lexer and object parser (ISO 32000-2 §7.2–7.3), tolerant of common real-world damage.
//!
//! The parser works on a byte slice and a position; it never allocates for skipped data and
//! never panics on malformed input (every index is bounds-checked). Depth is limited to defend
//! against pathological nesting.

use std::sync::Arc;

use crate::object::{Dict, ObjRef, Object, PdfString, Stream};
use crate::{Bytes, CosError};

const MAX_DEPTH: usize = 128;
/// Objects one top-level [`Lexer::object`] call may build. Real arrays (page trees, parent
/// trees, widths) stay far below; a decompressed object stream of `0 0 0 ...` does not.
const MAX_NODES: usize = 1 << 22;

/// One of the six PDF whitespace characters (ISO 32000-2 §7.2.3, Table 1).
pub fn is_whitespace(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ')
}

pub(crate) fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn is_regular(b: u8) -> bool {
    !is_whitespace(b) && !is_delimiter(b)
}

/// A cursor over PDF bytes.
pub struct Lexer<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    /// Objects built by the current top-level [`Lexer::object`] call.
    nodes: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos: pos.min(data.len()), nodes: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Skip whitespace and comments.
    pub fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if is_whitespace(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(c) = self.peek() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// Read a run of regular characters (a keyword or number).
    pub fn token(&mut self) -> &'a [u8] {
        let start = self.pos.min(self.data.len());
        while self.peek().is_some_and(is_regular) {
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    /// `true` (and consumes it) if the next token is exactly `kw`.
    pub fn eat_keyword(&mut self, kw: &[u8]) -> bool {
        self.skip_ws();
        let end = self.pos + kw.len();
        if self.data.get(self.pos..end) == Some(kw) && self.data.get(end).is_none_or(|b| !is_regular(*b)) {
            self.pos = end;
            true
        } else {
            false
        }
    }

    fn err(&self, what: &str) -> CosError {
        CosError::Syntax { offset: self.pos, detail: what.to_string() }
    }

    /// Parse one direct object. References `N G R` are recognised.
    pub fn object(&mut self) -> Result<Object, CosError> {
        self.nodes = 0;
        self.object_depth(0)
    }

    fn over_budget(&self) -> bool {
        self.nodes > MAX_NODES
    }

    fn object_depth(&mut self, depth: usize) -> Result<Object, CosError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        self.nodes = self.nodes.saturating_add(1);
        if self.over_budget() {
            return Err(self.err("object too large"));
        }
        self.skip_ws();
        let Some(b) = self.peek() else { return Err(self.err("unexpected end of data")) };
        match b {
            b'/' => {
                self.pos += 1;
                Ok(Object::Name(self.name_body()))
            }
            b'(' => {
                self.pos += 1;
                Ok(Object::String(PdfString { bytes: self.literal_string()?, hex: false }))
            }
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                Ok(Object::Dict(self.dict_body(depth)?))
            }
            b'<' => {
                self.pos += 1;
                Ok(Object::String(PdfString { bytes: self.hex_string()?, hex: true }))
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek() {
                        None => return Err(self.err("unterminated array")),
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        _ => {
                            let before = self.pos;
                            match self.object_depth(depth + 1) {
                                Ok(o) => items.push(o),
                                Err(e) if self.over_budget() => return Err(e),
                                // Tolerate junk inside arrays by skipping one byte.
                                Err(_) if self.pos == before => self.pos += 1,
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
                Ok(Object::Array(items))
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.number_or_ref(),
            _ => {
                let start = self.pos;
                let tok = self.token();
                match tok {
                    b"true" => Ok(Object::Bool(true)),
                    b"false" => Ok(Object::Bool(false)),
                    b"null" => Ok(Object::Null),
                    b"" => {
                        self.pos = start + 1;
                        Err(CosError::Syntax { offset: start, detail: format!("unexpected byte 0x{b:02x}") })
                    }
                    other => Err(CosError::Syntax { offset: start, detail: format!("unexpected keyword {:?}", String::from_utf8_lossy(other)) }),
                }
            }
        }
    }

    fn name_body(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if !is_regular(b) {
                break;
            }
            self.pos += 1;
            if b == b'#'
                && let (Some(h), Some(l)) = (self.data.get(self.pos).and_then(|c| hexval(*c)), self.data.get(self.pos + 1).and_then(|c| hexval(*c)))
            {
                out.push(h << 4 | l);
                self.pos += 2;
                continue;
            }
            out.push(b);
        }
        out
    }

    fn literal_string(&mut self) -> Result<Vec<u8>, CosError> {
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                    out.push(b);
                }
                b'\\' => {
                    let Some(e) = self.peek() else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'(' | b')' | b'\\' => out.push(e),
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xFF) as u8);
                        }
                        other => out.push(other),
                    }
                }
                // End-of-line markers inside strings are normalised to \n (§7.3.4.2).
                b'\r' => {
                    if self.peek() == Some(b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                }
                other => out.push(other),
            }
        }
        Err(self.err("unterminated string"))
    }

    fn hex_string(&mut self) -> Result<Vec<u8>, CosError> {
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while let Some(b) = self.peek() {
            self.pos += 1;
            if b == b'>' {
                if let Some(h) = hi {
                    out.push(h << 4);
                }
                return Ok(out);
            }
            if is_whitespace(b) {
                continue;
            }
            let Some(v) = hexval(b) else { return Err(self.err("invalid hex digit")) };
            match hi.take() {
                None => hi = Some(v),
                Some(h) => out.push(h << 4 | v),
            }
        }
        Err(self.err("unterminated hex string"))
    }

    fn dict_body(&mut self, depth: usize) -> Result<Dict, CosError> {
        let mut d = Dict::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(self.err("unterminated dictionary")),
                Some(b'>') if self.data.get(self.pos + 1) == Some(&b'>') => {
                    self.pos += 2;
                    return Ok(d);
                }
                Some(b'/') => {
                    self.pos += 1;
                    let key = self.name_body();
                    self.skip_ws();
                    // A key directly followed by `>>` has no value: treat as null. (A following
                    // `/` is a name *value*, so it cannot signal a missing value.)
                    if self.peek() == Some(b'>') && self.data.get(self.pos + 1) == Some(&b'>') {
                        d.set(key, Object::Null);
                        continue;
                    }
                    let value = self.object_depth(depth + 1)?;
                    d.set(key, value);
                }
                Some(_) => {
                    // Tolerate garbage between entries (seen in damaged files): skip a token.
                    let before = self.pos;
                    let _ = self.object_depth(depth + 1);
                    if self.over_budget() {
                        return Err(self.err("object too large"));
                    }
                    if self.pos == before {
                        self.pos += 1;
                    }
                }
            }
        }
    }

    fn number_or_ref(&mut self) -> Result<Object, CosError> {
        let start = self.pos;
        let first = self.number()?;
        // Look ahead for "G R" to form a reference.
        if let Object::Int(n) = first
            && n >= 0
        {
            let save = self.pos;
            self.skip_ws();
            if self.peek().is_some_and(|b| b.is_ascii_digit()) {
                let gstart = self.pos;
                let g = self.token();
                if g.iter().all(u8::is_ascii_digit) && self.eat_keyword(b"R") {
                    let generation = std::str::from_utf8(g).ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
                    if n <= u32::MAX as i64 {
                        return Ok(Object::Ref(ObjRef::new(n as u32, generation.min(u16::MAX as u32) as u16)));
                    }
                }
                let _ = gstart;
            }
            self.pos = save;
        }
        let _ = start;
        Ok(first)
    }

    fn number(&mut self) -> Result<Object, CosError> {
        let start = self.pos;
        let tok = self.token();
        let s = std::str::from_utf8(tok).map_err(|_| self.err("bad number"))?;
        if s.contains('.') {
            // Tolerate forms like "--5", "5-" or "1.2.3" by parsing the longest valid prefix.
            // Only a doubled sign needs a cleaned copy: a page of InDesign `/PieceInfo` is
            // thousands of plain reals, and building a copy and a substring search for each
            // was a large part of reading one (#5873).
            let t = s.trim_start_matches('+');
            let cleaned = if t.as_bytes().windows(2).any(|w| w == b"--") { std::borrow::Cow::Owned(t.replacen("--", "-", 1)) } else { t.into() };
            let v = cleaned.get(..real_prefix_len(cleaned.as_bytes())).and_then(|p| parse_counted::<f64>(p).ok());
            return Ok(Object::Real(v.unwrap_or(0.0)));
        }
        // Tolerate a doubled sign ("--280") like the real branch does.
        let t = s.trim_start_matches('+');
        let cleaned = t.strip_prefix('-').filter(|r| r.starts_with('-')).unwrap_or(t);
        match cleaned.parse::<i64>() {
            Ok(v) => Ok(Object::Int(v)),
            Err(_) => {
                // Out-of-range integers are treated as reals (§7.3.3 allows implementation limits).
                cleaned.parse::<f64>().map(Object::Real).map_err(|_| CosError::Syntax { offset: start, detail: format!("bad number {s:?}") })
            }
        }
    }
}

/// Length of the longest prefix of `s` that reads as a decimal number: a sign, digits with at
/// most one `.`, and an exponent with digits. Zero when that prefix has no digit. Found in one
/// pass so a long damaged token is converted once, not once per shorter prefix.
fn real_prefix_len(s: &[u8]) -> usize {
    let digits = |from: usize| s.get(from..).map_or(0, |r| r.iter().take_while(|b| b.is_ascii_digit()).count());
    let mut at = usize::from(matches!(s.first(), Some(b'+' | b'-')));
    let int = digits(at);
    at += int;
    let mut frac = 0;
    if s.get(at) == Some(&b'.') {
        frac = digits(at + 1);
        at += 1 + frac;
    }
    if int + frac == 0 {
        return 0;
    }
    if matches!(s.get(at), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(s.get(at + 1), Some(b'+' | b'-')));
        let exp = digits(at + 1 + sign);
        if exp > 0 {
            at += 1 + sign + exp;
        }
    }
    at
}

#[cfg(test)]
thread_local! {
    /// Bytes searched for `endstream` on this thread.
    pub(crate) static ENDSTREAM_SCAN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Bytes handed to number conversion on this thread.
    static NUMBER_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn parse_counted<T: std::str::FromStr>(s: &str) -> Result<T, T::Err> {
    #[cfg(test)]
    NUMBER_BYTES.with(|c| c.set(c.get() + s.len()));
    s.parse()
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse an indirect object at `offset`: `N G obj <object> [stream…endstream] endobj`.
/// `resolve_length` resolves an indirect `/Length`. Returns the object and its (num, gen).
pub fn parse_indirect(data: &[u8], offset: usize, resolve_length: &dyn Fn(ObjRef) -> Option<i64>) -> Result<(ObjRef, Object), CosError> {
    parse_indirect_in(data, None, offset, resolve_length, None)
}

/// [`parse_indirect`] for a document's own bytes: stream data points into `data` instead of
/// being copied out of it.
/// `endstreams` is the document's [`EndstreamIndex`] for `data`.
pub(crate) fn parse_indirect_shared(
    data: &Arc<Vec<u8>>,
    offset: usize,
    resolve_length: &dyn Fn(ObjRef) -> Option<i64>,
    endstreams: &EndstreamIndex,
) -> Result<(ObjRef, Object), CosError> {
    parse_indirect_in(data, Some(data), offset, resolve_length, Some(endstreams))
}

/// Where `endstream` occurs in a document's bytes, found once, the first time a stream's
/// /Length is wrong or missing. Searching the rest of the file for each such stream instead
/// makes many of them ending at one late `endstream` quadratic to read.
pub(crate) type EndstreamIndex = std::sync::OnceLock<Vec<usize>>;

fn endstream_offsets(data: &[u8]) -> Vec<usize> {
    #[cfg(test)]
    ENDSTREAM_SCAN.with(|c| c.set(c.get() + data.len()));
    data.windows(9).enumerate().filter(|(i, w)| *w == b"endstream" && endstream_at(data, *i)).map(|(i, _)| i).collect()
}

fn parse_indirect_in(
    data: &[u8],
    shared: Option<&Arc<Vec<u8>>>,
    offset: usize,
    resolve_length: &dyn Fn(ObjRef) -> Option<i64>,
    endstreams: Option<&EndstreamIndex>,
) -> Result<(ObjRef, Object), CosError> {
    let mut lx = Lexer::new(data, offset);
    lx.skip_ws();
    let num_tok = lx.token();
    lx.skip_ws();
    let gen_tok = lx.token();
    let parse_u = |t: &[u8]| std::str::from_utf8(t).ok().and_then(|s| s.parse::<u64>().ok());
    let (Some(num), Some(generation)) = (parse_u(num_tok), parse_u(gen_tok)) else {
        return Err(CosError::Syntax { offset, detail: "expected object header".into() });
    };
    if !lx.eat_keyword(b"obj") {
        return Err(CosError::Syntax { offset: lx.pos, detail: "expected 'obj'".into() });
    }
    let id = ObjRef::new(num.min(u32::MAX as u64) as u32, generation.min(u16::MAX as u64) as u16);
    let obj = match lx.object() {
        Ok(o) => o,
        // "N G obj endobj" (empty) is treated as null.
        Err(_) if lx.eat_keyword(b"endobj") => return Ok((id, Object::Null)),
        Err(e) => return Err(e),
    };
    lx.skip_ws();
    if let Object::Dict(dict) = &obj
        && lx.eat_keyword(b"stream")
    {
        // The keyword is followed by CRLF or LF (a lone CR is tolerated). Microsoft Reporting
        // Services writes a space first (`stream \r\n`); spaces and tabs count only when a line
        // break follows, so they are never taken from the data.
        let mut eol = lx.pos;
        while matches!(data.get(eol), Some(b' ' | b'\t')) {
            eol += 1;
        }
        if matches!(data.get(eol), Some(b'\r' | b'\n')) {
            lx.pos = eol;
        }
        match (data.get(lx.pos), data.get(lx.pos + 1)) {
            (Some(b'\r'), Some(b'\n')) => lx.pos += 2,
            (Some(b'\n'), _) | (Some(b'\r'), _) => lx.pos += 1,
            _ => {}
        }
        let start = lx.pos;
        let declared = match dict.get(b"Length") {
            Some(Object::Int(n)) => Some(*n),
            Some(Object::Ref(r)) => resolve_length(*r),
            _ => None,
        };
        // A /Length that fits in the file (not negative, and no wrap on a 32-bit target).
        let fits = declared.and_then(|l| usize::try_from(l).ok()).filter(|l| start.checked_add(*l).is_some_and(|end| end <= data.len()));
        // What follows the /Length bytes, white space aside.
        let after = |len: usize| {
            let mut after = Lexer::new(data, start.saturating_add(len));
            after.skip_ws();
            data.get(after.pos..).unwrap_or_default()
        };
        let len = match fits {
            // The /Length ends at `endstream`, or where it would be: misspelt (`endstrea`), or
            // left out before `endobj` or the end of the file. The stream is the bytes its /Length
            // gives, "how many bytes of the PDF file are used for the stream's data" (ISO 32000-2
            // §7.3.8.2), as other readers take it. A producer that wrote `endstrea` after every
            // stream lost every font and every page's content, and searching on would run into a
            // later object's `endstream`.
            Some(l) if at_stream_end(after(l)) => l,
            // Wrong or missing /Length: search for "endstream" (§7.3.8.1 recovery). With none
            // left in the file, a /Length that fits is still the stream's extent.
            _ => match find_endstream(data, start, endstreams) {
                Some(end) => end - start,
                None => fits.ok_or_else(|| CosError::Syntax { offset: start, detail: "stream without endstream".into() })?,
            },
        };
        let range = start..start + len;
        let raw = match shared {
            Some(buf) => Bytes::view(buf, range),
            None => data.get(range).map(|r| Bytes::from(r.to_vec())),
        }
        .ok_or_else(|| CosError::Syntax { offset: start, detail: "stream data past the end of the file".into() })?;
        let mut dict = dict.clone();
        dict.set(b"Length".to_vec(), Object::Int(raw.len() as i64));
        return Ok((id, Object::Stream(Stream { dict, raw })));
    }
    Ok((id, obj))
}

/// Whether `rest`, what follows a stream's /Length bytes with white space skipped, is where the
/// stream ends. A whole `endstream` token is: a token runs on through regular characters (ISO
/// 32000-2 §7.2.3), so payload such as `endstreamer` is not one. A misspelt `endstream`
/// (`endstrea`), an `endobj` with the `endstream` left out, or both, end it only where the object
/// ends as well, before the end of the data, another object's `N G obj` header, or `xref`,
/// `trailer` or `startxref`: payload such as `endobj more` or `endobject`, where a wrong /Length
/// stops, is not, and the search for `endstream` runs instead. So does anything else but the end
/// of the data.
fn at_stream_end(rest: &[u8]) -> bool {
    let mut lx = Lexer::new(rest, 0);
    if lx.eat_keyword(b"endstream") {
        return true;
    }
    let misspelt = lx.eat_keyword(b"endstrea");
    let endobj = lx.eat_keyword(b"endobj");
    lx.skip_ws();
    if lx.pos >= rest.len() {
        return true;
    }
    if !(misspelt || endobj) {
        return false;
    }
    if [&b"xref"[..], b"trailer", b"startxref"].iter().any(|kw| Lexer::new(rest, lx.pos).eat_keyword(kw)) {
        return true;
    }
    let number = |lx: &mut Lexer| {
        lx.skip_ws();
        let t = lx.token();
        !t.is_empty() && t.iter().all(u8::is_ascii_digit)
    };
    number(&mut lx) && number(&mut lx) && lx.eat_keyword(b"obj")
}

/// Whether a whole `endstream` token starts at `at` in `data`: one followed by a regular
/// character (`endstreamer`) is payload (ISO 32000-2 §7.2.3).
fn endstream_at(data: &[u8], at: usize) -> bool {
    data.get(at..).is_some_and(|d| d.starts_with(b"endstream")) && !data.get(at.saturating_add(9)).copied().is_some_and(is_regular)
}

/// Offset where stream data ends (before the EOL preceding "endstream").
fn find_endstream(data: &[u8], from: usize, endstreams: Option<&EndstreamIndex>) -> Option<usize> {
    let at = match endstreams {
        Some(index) => {
            let index = index.get_or_init(|| endstream_offsets(data));
            index.get(index.partition_point(|&p| p < from)).copied()?
        }
        None => {
            let hay = data.get(from..)?;
            let found = hay.windows(9).enumerate().position(|(i, w)| w == b"endstream" && endstream_at(hay, i));
            #[cfg(test)]
            ENDSTREAM_SCAN.with(|c| c.set(c.get() + found.map_or(hay.len(), |p| p + 9)));
            found? + from
        }
    };
    let mut end = at;
    if end > from && data[end - 1] == b'\n' {
        end -= 1;
    }
    if end > from && data[end - 1] == b'\r' {
        end -= 1;
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_with_multibyte_garbage_do_not_panic() {
        // From `cargo xtask fuzz`: "2 0 obj\n.\u{2014}" (an em dash after a dot).
        let doc = crate::Document::open(std::sync::Arc::new("2 0 obj\n.\u{2014}\n".as_bytes().to_vec()));
        assert!(doc.is_err());
        for t in [".\u{2014}", "1.\u{e9}5", "-.\u{1F600}", "+.5\u{2014}"] {
            let _ = Lexer::new(t.as_bytes(), 0).object();
        }
        assert_eq!(Lexer::new("1.5\u{2014}".as_bytes(), 0).object().unwrap(), Object::Real(1.5));
    }

    #[test]
    fn a_damaged_real_is_converted_once() {
        let token = format!("1.{}{}", "5".repeat(2_000), "x".repeat(20_000));
        NUMBER_BYTES.with(|c| c.set(0));
        let Ok(Object::Real(v)) = Lexer::new(token.as_bytes(), 0).object() else { panic!("not a real") };
        let converted = NUMBER_BYTES.with(|c| c.get());
        assert!(converted <= token.len(), "{converted} bytes converted for a {}-byte token", token.len());
        assert!((v - 1.5555).abs() < 1e-3);
        // The longest numeric prefix wins, as before.
        for (t, want) in
            [("1.2.3", 1.2), ("--5.", -5.0), ("+.5x", 0.5), ("5.-", 5.0), ("1.5e3", 1500.0), ("1.5e+", 1.5), ("-.x", 0.0), (".", 0.0), ("-inf.", 0.0)]
        {
            assert_eq!(Lexer::new(t.as_bytes(), 0).object().ok(), Some(Object::Real(want)), "{t}");
        }
    }

    fn obj(s: &str) -> Object {
        Lexer::new(s.as_bytes(), 0).object().unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn doubled_sign_integer_reads_like_the_real_form() {
        // Some writers emit a TJ kerning as "--280"; the real form "--280.0" is already read.
        assert_eq!(obj("--280.0"), Object::Real(-280.0));
        assert_eq!(obj("--280"), Object::Int(-280));
        match obj("[(Hello) --280 (World)]") {
            Object::Array(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[1], Object::Int(-280));
            }
            other => panic!("not an array: {other:?}"),
        }
    }

    #[test]
    fn scalars() {
        assert_eq!(obj("true"), Object::Bool(true));
        assert_eq!(obj("null"), Object::Null);
        assert_eq!(obj("  -17 "), Object::Int(-17));
        assert_eq!(obj("+42"), Object::Int(42));
        assert_eq!(obj("-.5"), Object::Real(-0.5));
        assert_eq!(obj("4."), Object::Real(4.0));
        assert_eq!(obj("99999999999999999999"), Object::Real(1e20));
        assert_eq!(obj("12 0 R"), Object::Ref(ObjRef::new(12, 0)));
        assert_eq!(obj("12 0 obj").as_int(), Some(12), "12 followed by 0 obj is not a reference");
    }

    #[test]
    fn names_decode_hex_escapes() {
        assert_eq!(obj("/A#20B"), Object::Name(b"A B".to_vec()));
        assert_eq!(obj("/Adobe#"), Object::Name(b"Adobe#".to_vec()));
        assert_eq!(obj("/"), Object::Name(Vec::new()));
    }

    #[test]
    fn literal_strings() {
        let s = |x: &str| obj(x).as_string().expect("string").bytes.clone();
        assert_eq!(s("(a (nested) b)"), b"a (nested) b");
        assert_eq!(s(r"(\n\t\\\(\))"), b"\n\t\\()");
        assert_eq!(s(r"(\101\1011)"), b"AA1");
        assert_eq!(s("(line\\\ncontinued)"), b"linecontinued");
        assert_eq!(s("(cr\r\nlf)"), b"cr\nlf");
    }

    #[test]
    fn hex_strings() {
        let s = obj("<48 65 6C6c6f7>");
        assert_eq!(s.as_string().unwrap().bytes, b"Hellop");
        assert!(s.as_string().unwrap().hex);
    }

    #[test]
    fn arrays_and_dicts() {
        let o = obj("<< /Type /Page /Kids [1 0 R 2 0 R] /Box [0 0 612.5 792] /Nested << /A true >> /Empty >>");
        let d = o.as_dict().unwrap();
        assert_eq!(d.name(b"Type"), Some(&b"Page"[..]));
        assert_eq!(d.get(b"Kids").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(d.get(b"Box").unwrap().as_array().unwrap()[2], Object::Real(612.5));
        assert_eq!(d.get(b"Empty"), Some(&Object::Null));
    }

    #[test]
    fn comments_are_whitespace() {
        assert_eq!(obj("% hello\n  [1 % two\n 3]"), Object::Array(vec![Object::Int(1), Object::Int(3)]));
    }

    #[test]
    fn indirect_stream_with_wrong_length_recovers() {
        let data = b"7 0 obj\n<< /Length 999 >>\nstream\nHELLO\nendstream\nendobj\n";
        let (id, o) = parse_indirect(data, 0, &|_| None).unwrap();
        assert_eq!(id, ObjRef::new(7, 0));
        let Object::Stream(s) = o else { panic!("stream expected") };
        assert_eq!(&s.raw[..], b"HELLO");
        assert_eq!(s.dict.int(b"Length"), Some(5));
    }

    /// With no `endstream` left in the file, as a producer that writes `endstrea` leaves every
    /// stream, a stream is the bytes its /Length gives, "how many bytes of the PDF file are used
    /// for the stream's data" (ISO 32000-2 §7.3.8.2): it was an error, and the object null, so
    /// such a file lost every font and page content. A /Length past the file, or none, is still
    /// no stream.
    #[test]
    fn indirect_stream_without_endstream_is_its_length() {
        for end in [&b"\r\nendstrea\rendobj\r\n"[..], b"\nendobj\n", b""] {
            let mut data = b"7 0 obj\n<< /Length 5 >>\nstream\r\nHELLO".to_vec();
            data.extend_from_slice(end);
            let (_, o) = parse_indirect(&data, 0, &|_| None).unwrap();
            let Object::Stream(s) = o else { panic!("stream expected") };
            assert_eq!(&s.raw[..], b"HELLO", "{end:?}");
        }
        assert!(parse_indirect(b"7 0 obj\n<< /Length 6 >>\nstream\nHELLO", 0, &|_| None).is_err(), "past the file");
        assert!(parse_indirect(b"7 0 obj\n<< >>\nstream\nHELLO\nendstrea\nendobj\n", 0, &|_| None).is_err(), "no /Length");
    }

    /// A stream ending in `endstrea` is its /Length even when a later object does have an
    /// `endstream`: the search for one would otherwise run on into that object and take its
    /// header and data as this stream's.
    #[test]
    fn a_stream_without_endstream_is_not_read_into_a_later_one() {
        let data = b"7 0 obj\n<< /Length 5 >>\nstream\r\nHELLO\r\nendstrea\rendobj\r\n8 0 obj\n<< /Length 5 >>\nstream\nWORLD\nendstream\nendobj\n";
        let second = data.windows(7).position(|w| w == b"8 0 obj").unwrap();
        for (offset, want) in [(0, &b"HELLO"[..]), (second, b"WORLD")] {
            let (_, o) = parse_indirect(data, offset, &|_| None).unwrap();
            let Object::Stream(s) = o else { panic!("stream expected at {offset}") };
            assert_eq!(&s.raw[..], want, "at {offset}");
        }
        // Through the loader's shared index of `endstream` offsets as well.
        let index = EndstreamIndex::new();
        let (_, o) = parse_indirect_in(data, None, 0, &|_| None, Some(&index)).unwrap();
        let Object::Stream(s) = o else { panic!("stream expected") };
        assert_eq!(&s.raw[..], b"HELLO");
    }

    /// The same for a stream whose `endstream` is left out before `endobj`: the later object's
    /// `endstream` is not this stream's end.
    #[test]
    fn a_stream_ending_at_endobj_is_not_read_into_a_later_one() {
        let data = b"7 0 obj\n<< /Length 5 >>\nstream\nHELLO\nendobj\n8 0 obj\n<< /Length 5 >>\nstream\nWORLD\nendstream\nendobj\n";
        let (_, o) = parse_indirect(data, 0, &|_| None).unwrap();
        let Object::Stream(s) = o else { panic!("stream expected") };
        assert_eq!(&s.raw[..], b"HELLO");
    }

    /// A wrong /Length that stops before payload bytes beginning like an end marker (`endobject`,
    /// `endstreak`, `endstreamer`) is not taken for the stream's end: a token runs on through
    /// regular characters (ISO 32000-2 §7.2.3), so the search for `endstream` finds the whole
    /// payload, and skips `endstreamer` too, by itself and through the loader's shared index.
    #[test]
    fn a_length_ending_inside_a_longer_word_is_not_an_end_marker() {
        for word in ["endobject", "endstreak", "endstreamer"] {
            let pdf = format!("7 0 obj\n<< /Length 5 >>\nstream\nHELLO {word} more\nendstream\nendobj\n");
            let (_, o) = parse_indirect(pdf.as_bytes(), 0, &|_| None).unwrap();
            let Object::Stream(s) = o else { panic!("stream expected") };
            assert_eq!(s.raw[..], *format!("HELLO {word} more").as_bytes(), "{word}");
            let index = EndstreamIndex::new();
            let (_, o) = parse_indirect_in(pdf.as_bytes(), None, 0, &|_| None, Some(&index)).unwrap();
            let Object::Stream(s) = o else { panic!("stream expected") };
            assert_eq!(s.raw[..], *format!("HELLO {word} more").as_bytes(), "{word}, indexed");
        }
        // Whole markers still end it, followed by white space, a delimiter or the end of the data.
        for tail in ["endobj\n", "endstrea%", "endobj"] {
            let pdf = format!("7 0 obj\n<< /Length 5 >>\nstream\nHELLO\n{tail}");
            let (_, o) = parse_indirect(pdf.as_bytes(), 0, &|_| None).unwrap();
            let Object::Stream(s) = o else { panic!("stream expected") };
            assert_eq!(&s.raw[..], b"HELLO", "{tail:?}");
        }
    }

    /// A whole `endobj` or `endstrea` token where a wrong /Length stops is payload, not the
    /// stream's end, unless the object ends there too: what follows is the end of the data,
    /// another object's header, or `xref`, `trailer` or `startxref`. Otherwise the search for
    /// `endstream` finds the whole payload.
    #[test]
    fn an_end_marker_inside_the_payload_does_not_end_the_stream() {
        for payload in ["endobj more", "endstrea more", "endstrea endobj more", "endobj 12 more"] {
            let pdf = format!("7 0 obj\n<< /Length 5 >>\nstream\nHELLO {payload}\nendstream\nendobj\n");
            let (_, o) = parse_indirect(pdf.as_bytes(), 0, &|_| None).unwrap();
            let Object::Stream(s) = o else { panic!("stream expected") };
            assert_eq!(s.raw[..], *format!("HELLO {payload}").as_bytes(), "{payload}");
        }
        // Where the object does end, the /Length is the stream, also with an `endstream` later.
        for next in ["8 0 obj\n<< >>", "%c\n12 0 obj", "xref", "trailer", "startxref"] {
            for marker in ["endobj", "endstrea\nendobj", "endstrea"] {
                let pdf = format!("7 0 obj\n<< /Length 5 >>\nstream\nHELLO\n{marker}\n{next}\nstream\nWORLD\nendstream\nendobj\n");
                let (_, o) = parse_indirect(pdf.as_bytes(), 0, &|_| None).unwrap();
                let Object::Stream(s) = o else { panic!("stream expected") };
                assert_eq!(&s.raw[..], b"HELLO", "{marker:?} then {next:?}");
            }
        }
    }
    /// Without an `endstream`, the /Length is trusted as written when it fits, also when it is
    /// wrong, and an indirect one is resolved first; one that doesn't fit, a negative one and one
    /// too large for any file are errors rather than wrapped into a short read.
    #[test]
    fn a_stream_without_endstream_takes_its_length_only_when_it_fits() {
        let stream = |length: &str, tail: &str| format!("7 0 obj\n<< /Length {length} >>\nstream\nHELLO{tail}");
        let raw = |pdf: &str, lookup: &dyn Fn(ObjRef) -> Option<i64>| match parse_indirect(pdf.as_bytes(), 0, lookup) {
            Ok((_, Object::Stream(s))) => Ok(s.raw.to_vec()),
            Ok((_, other)) => panic!("{pdf:?} is {other:?}"),
            Err(e) => Err(e),
        };
        let none = |_: ObjRef| None;
        // Wrong but fitting: the bytes it gives, not the bytes up to `endstrea`.
        assert_eq!(raw(&stream("3", "\nendstrea\nendobj\n"), &none).unwrap(), b"HEL");
        // Exactly to the end of the file, and nothing at all.
        assert_eq!(raw(&stream("5", ""), &none).unwrap(), b"HELLO");
        assert_eq!(raw(&stream("0", "\nendstrea\nendobj\n"), &none).unwrap(), b"");
        // Indirect, resolved through the lookup.
        let lookup = |r: ObjRef| (r == ObjRef::new(9, 0)).then_some(4);
        assert_eq!(raw(&stream("9 0 R", "\nendstrea\nendobj\n"), &lookup).unwrap(), b"HELL");
        assert!(raw(&stream("9 0 R", "\nendstrea\nendobj\n"), &none).is_err(), "an unresolved /Length");
        // One past the end, negative, the largest integer and one beyond it.
        for length in ["6", "-1", "-9223372036854775808", "9223372036854775807", "18446744073709551621"] {
            assert!(raw(&stream(length, ""), &none).is_err(), "/Length {length}");
        }
    }

    #[test]
    fn indirect_stream_with_a_space_before_the_line_break() {
        // `stream \r\n` as Microsoft Reporting Services writes it: the data starts after the
        // line break, so the /Length is still right and the data keeps no leading space.
        for eol in [&b" \r\n"[..], b" \n", b"\t \r"] {
            let mut data = b"7 0 obj\n<< /Length 5 >>\nstream".to_vec();
            data.extend_from_slice(eol);
            data.extend_from_slice(b"HELLO\r\nendstream\nendobj\n");
            let (_, o) = parse_indirect(&data, 0, &|_| None).unwrap();
            let Object::Stream(s) = o else { panic!("stream expected") };
            assert_eq!(&s.raw[..], b"HELLO", "{eol:?}");
        }
        // Spaces with no line break after them are data, not a keyword terminator.
        let data = b"7 0 obj\n<< /Length 7 >>\nstream   HELLO\nendstream\nendobj\n";
        let (_, o) = parse_indirect(data, 0, &|_| None).unwrap();
        let Object::Stream(s) = o else { panic!("stream expected") };
        assert_eq!(&s.raw[..], b"   HELLO");
    }

    #[test]
    fn indirect_length_is_resolved() {
        let data = b"7 0 obj\n<< /Length 8 0 R >>\nstream\r\nAB\r\nendstream\nendobj\n";
        let (_, o) = parse_indirect(data, 0, &|r| (r.num == 8).then_some(2)).unwrap();
        let Object::Stream(s) = o else { panic!() };
        assert_eq!(&s.raw[..], b"AB");
    }

    #[test]
    fn garbage_never_panics() {
        for input in ["<<", "[", "(", "<", "<< /A", "[1 2", ")", "]", ">>", "<<>>>", "1 0", "\0\0", "/A#", "<zz>", "((("] {
            let _ = Lexer::new(input.as_bytes(), 0).object();
        }
        let deep = "[".repeat(10_000);
        assert!(Lexer::new(deep.as_bytes(), 0).object().is_err());
    }

    #[test]
    fn one_object_has_a_node_budget() {
        // A wide array (from a tiny compressed object stream) must not grow without bound.
        let wide = |n: usize| format!("[{}]", "0 ".repeat(n));
        let at_cap = Lexer::new(wide(MAX_NODES - 1).as_bytes(), 0).object();
        assert!(matches!(at_cap, Ok(Object::Array(ref items)) if items.len() == MAX_NODES - 1));
        assert!(Lexer::new(wide(MAX_NODES).as_bytes(), 0).object().is_err());
        // The same inside a dictionary value and behind tolerated junk.
        assert!(Lexer::new(format!("<< /A {} >>", wide(MAX_NODES)).as_bytes(), 0).object().is_err());
        assert!(Lexer::new(format!("[[{}] ]", "0 ".repeat(MAX_NODES)).as_bytes(), 0).object().is_err());
        // The budget is per top-level object: a lexer reused for many operands keeps working.
        let two = wide(MAX_NODES - 1).repeat(2);
        let mut lx = Lexer::new(two.as_bytes(), 0);
        assert!(lx.object().is_ok() && lx.object().is_ok());
    }

    /// A plain real is read without a cleaned copy of its token; the forms that need one read
    /// as they did (#5873).
    #[test]
    fn reals_read_the_same_with_and_without_a_doubled_sign() {
        for (token, want) in [("1.5", 1.5), ("-375.993", -375.993), ("+2.25", 2.25), ("--5.5", -5.5), ("1.2.3", 1.2), ("-.5", -0.5), ("0.0", 0.0)] {
            assert_eq!(Lexer::new(token.as_bytes(), 0).object().unwrap(), Object::Real(want), "{token}");
        }
    }
}
