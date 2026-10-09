//! Lexer and object parser (ISO 32000-2 §7.2–7.3), tolerant of common real-world damage.
//!
//! The parser works on a byte slice and a position; it never allocates for skipped data and
//! never panics on malformed input (every index is bounds-checked). Depth is limited to defend
//! against pathological nesting.

use std::sync::Arc;

use crate::object::{Dict, ObjRef, Object, PdfString, Stream};
use crate::{Bytes, CosError};

const MAX_DEPTH: usize = 128;

pub(crate) fn is_whitespace(b: u8) -> bool {
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
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos: pos.min(data.len()) }
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
        self.object_depth(0)
    }

    fn object_depth(&mut self, depth: usize) -> Result<Object, CosError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
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
            let cleaned: String = s.trim_start_matches('+').replacen("--", "-", 1);
            let mut end = cleaned.len();
            while end > 0 {
                // Only cut at character boundaries: a token can hold multi-byte UTF-8 (fuzzing
                // found ".—", which panicked here).
                if cleaned.is_char_boundary(end)
                    && let Ok(v) = cleaned[..end].parse::<f64>()
                {
                    return Ok(Object::Real(v));
                }
                end -= 1;
            }
            return Ok(Object::Real(0.0));
        }
        match s.trim_start_matches('+').parse::<i64>() {
            Ok(v) => Ok(Object::Int(v)),
            Err(_) => {
                // Out-of-range integers are treated as reals (§7.3.3 allows implementation limits).
                s.parse::<f64>().map(Object::Real).map_err(|_| CosError::Syntax { offset: start, detail: format!("bad number {s:?}") })
            }
        }
    }
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
    parse_indirect_in(data, None, offset, resolve_length)
}

/// [`parse_indirect`] for a document's own bytes: stream data points into `data` instead of
/// being copied out of it.
pub(crate) fn parse_indirect_shared(
    data: &Arc<Vec<u8>>,
    offset: usize,
    resolve_length: &dyn Fn(ObjRef) -> Option<i64>,
) -> Result<(ObjRef, Object), CosError> {
    parse_indirect_in(data, Some(data), offset, resolve_length)
}

fn parse_indirect_in(
    data: &[u8],
    shared: Option<&Arc<Vec<u8>>>,
    offset: usize,
    resolve_length: &dyn Fn(ObjRef) -> Option<i64>,
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
        // The keyword is followed by CRLF or LF (a lone CR is tolerated).
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
        let valid = |len: i64| -> bool {
            if len < 0 {
                return false;
            }
            let end = start.saturating_add(len as usize);
            if end > data.len() {
                return false;
            }
            let mut after = Lexer::new(data, end);
            after.eat_keyword(b"endstream")
        };
        let len = match declared {
            Some(l) if valid(l) => l as usize,
            // Wrong or missing /Length: search for "endstream" (§7.3.8.1 recovery).
            _ => find_endstream(data, start).ok_or_else(|| CosError::Syntax { offset: start, detail: "stream without endstream".into() })? - start,
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

/// Offset where stream data ends (before the EOL preceding "endstream").
fn find_endstream(data: &[u8], from: usize) -> Option<usize> {
    let hay = data.get(from..)?;
    let at = hay.windows(9).position(|w| w == b"endstream")? + from;
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

    fn obj(s: &str) -> Object {
        Lexer::new(s.as_bytes(), 0).object().unwrap_or_else(|e| panic!("{s}: {e}"))
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
}
