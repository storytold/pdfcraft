//! Lexer and object parser (ISO 32000-2 §7.2–7.3), tolerant of common real-world damage.
//!
//! The parser works on a byte slice and a position; it never allocates for skipped data and
//! never panics on malformed input (every index is bounds-checked). Depth is limited to defend
//! against pathological nesting.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
    /// The document's large nested dictionaries, when objects are read from a document.
    shared: Option<&'a SharedDicts>,
    /// The dictionary being read holds one of [`SHARE_FROM`] bytes or more.
    large_child: bool,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos: pos.min(data.len()), nodes: 0, shared: None, large_child: false }
    }

    /// Read nested dictionaries through `shared`: one that recurs byte for byte is parsed once.
    pub(crate) fn sharing(mut self, shared: &'a SharedDicts) -> Self {
        self.shared = Some(shared);
        self
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
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => self.dict_at(depth),
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

    /// The dictionary starting at `<<`. A nested one that the document has met before, byte for
    /// byte at the same depth, is the earlier parse; it counts the same nodes against the budget.
    fn dict_at(&mut self, depth: usize) -> Result<Object, CosError> {
        let start = self.pos;
        let shared = self.shared.filter(|_| depth > 0);
        if let Some((dict, len, nodes)) = shared.and_then(|s| s.find(self.data, start, depth)) {
            self.pos = start.saturating_add(len).min(self.data.len());
            self.nodes = self.nodes.saturating_add(nodes);
            self.large_child = true;
            if self.over_budget() {
                return Err(self.err("object too large"));
            }
            return Ok(Object::Dict(dict));
        }
        let (nodes, outer_large) = (self.nodes, std::mem::replace(&mut self.large_child, false));
        self.pos += 2;
        let body = self.dict_body(depth);
        let inner_large = std::mem::replace(&mut self.large_child, outer_large);
        let dict = body?;
        let span = self.data.get(start..self.pos).unwrap_or_default();
        // Only the innermost large dictionary is offered: the copies of one that encloses it
        // usually differ elsewhere (each page's /Resources names its own XObjects).
        if let Some(shared) = shared
            && !inner_large
        {
            shared.offer(span, depth, self.nodes.saturating_sub(nodes), &dict);
        }
        self.large_child = outer_large || inner_large || span.len() >= SHARE_FROM;
        Ok(Object::Dict(dict))
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
            let cleaned: String = s.trim_start_matches('+').replacen("--", "-", 1);
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

/// Nested dictionaries of this many bytes or more are offered to [`SharedDicts`].
const SHARE_FROM: usize = 8 * 1024;
/// Dictionaries a document keeps for sharing, and the bytes they span together.
const SHARED_MAX: usize = 16;
const SHARED_BYTES: usize = 16 << 20;

/// Large nested dictionaries a document has parsed, kept so that a copy of one is not parsed
/// again. Producers repeat one inline resource dictionary in every page: a CAD plot rewritten by
/// a header-and-footer tool lists the same 10,816 shadings in each of its 3,753 pages, 730 MB of
/// dictionaries that would otherwise be parsed in full on every walk over the pages.
///
/// A dictionary is reused only where the same bytes start a dictionary at the same depth, so it
/// reads exactly as a fresh parse would. Bytes are compared, never hashed alone, and encrypted
/// strings stay as parsed: decryption happens after the parse, per object.
#[derive(Default)]
pub(crate) struct SharedDicts {
    entries: Mutex<Vec<Arc<SharedDict>>>,
    /// `entries.len()`, read without the lock: most documents never offer a dictionary.
    count: AtomicUsize,
}

struct SharedDict {
    bytes: Box<[u8]>,
    depth: usize,
    /// Nodes the dictionary's own parse built below it.
    nodes: usize,
    dict: Dict,
    hits: AtomicUsize,
}

impl SharedDicts {
    /// The dictionary, its byte length and its node count, if `data[at..]` starts with the bytes
    /// of one kept at `depth`.
    fn find(&self, data: &[u8], at: usize, depth: usize) -> Option<(Dict, usize, usize)> {
        if self.count.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let entries = self.entries.lock().ok()?;
        let rest = data.get(at..)?;
        let found = entries.iter().find(|e| {
            let n = e.bytes.len();
            // Copies of one dictionary that differ tend to differ in their last entries.
            let tail = n.saturating_sub(64);
            e.depth == depth && rest.get(tail..n).is_some_and(|t| e.bytes.get(tail..) == Some(t)) && rest.get(..n) == Some(&e.bytes[..])
        })?;
        found.hits.fetch_add(1, Ordering::Relaxed);
        Some((found.dict.clone(), found.bytes.len(), found.nodes))
    }

    /// Keep `dict`, parsed from `bytes` at `depth`, if it is large enough and there is room. When
    /// full, it replaces the oldest kept dictionary that no copy has reused yet.
    fn offer(&self, bytes: &[u8], depth: usize, nodes: usize, dict: &Dict) {
        if bytes.len() < SHARE_FROM || bytes.len() > SHARED_BYTES {
            return;
        }
        let Ok(mut entries) = self.entries.lock() else { return };
        if entries.iter().any(|e| e.depth == depth && *e.bytes == *bytes) {
            return;
        }
        let used = |entries: &[Arc<SharedDict>]| entries.iter().map(|e| e.bytes.len()).sum::<usize>();
        while entries.len() >= SHARED_MAX || used(&entries).saturating_add(bytes.len()) > SHARED_BYTES {
            let Some(unused) = entries.iter().position(|e| e.hits.load(Ordering::Relaxed) == 0) else { return };
            entries.remove(unused);
        }
        entries.push(Arc::new(SharedDict { bytes: bytes.into(), depth, nodes, dict: dict.clone(), hits: AtomicUsize::new(0) }));
        self.count.store(entries.len(), Ordering::Relaxed);
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
    parse_indirect_in(data, None, offset, resolve_length, None, None)
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
    parse_indirect_in(data, Some(data), offset, resolve_length, Some(endstreams), None)
}

/// [`parse_indirect_shared`] reading nested dictionaries through the document's `shared`.
pub(crate) fn parse_indirect_sharing(
    data: &Arc<Vec<u8>>,
    offset: usize,
    resolve_length: &dyn Fn(ObjRef) -> Option<i64>,
    endstreams: &EndstreamIndex,
    shared: &SharedDicts,
) -> Result<(ObjRef, Object), CosError> {
    parse_indirect_in(data, Some(data), offset, resolve_length, Some(endstreams), Some(shared))
}

/// Where `endstream` occurs in a document's bytes, found once, the first time a stream's
/// /Length is wrong or missing. Searching the rest of the file for each such stream instead
/// makes many of them ending at one late `endstream` quadratic to read.
pub(crate) type EndstreamIndex = std::sync::OnceLock<Vec<usize>>;

fn endstream_offsets(data: &[u8]) -> Vec<usize> {
    #[cfg(test)]
    ENDSTREAM_SCAN.with(|c| c.set(c.get() + data.len()));
    data.windows(9).enumerate().filter(|(_, w)| *w == b"endstream").map(|(i, _)| i).collect()
}

fn parse_indirect_in(
    data: &[u8],
    shared: Option<&Arc<Vec<u8>>>,
    offset: usize,
    resolve_length: &dyn Fn(ObjRef) -> Option<i64>,
    endstreams: Option<&EndstreamIndex>,
    dicts: Option<&SharedDicts>,
) -> Result<(ObjRef, Object), CosError> {
    let mut lx = Lexer::new(data, offset);
    lx.shared = dicts;
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
            _ => {
                find_endstream(data, start, endstreams)
                    .ok_or_else(|| CosError::Syntax { offset: start, detail: "stream without endstream".into() })?
                    - start
            }
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
fn find_endstream(data: &[u8], from: usize, endstreams: Option<&EndstreamIndex>) -> Option<usize> {
    let at = match endstreams {
        Some(index) => {
            let index = index.get_or_init(|| endstream_offsets(data));
            index.get(index.partition_point(|&p| p < from)).copied()?
        }
        None => {
            let hay = data.get(from..)?;
            let found = hay.windows(9).position(|w| w == b"endstream");
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

    /// `<< /Sh0 0 0 R /Sh1 1 0 R … >>` of `n` entries: a resource list of plot shadings.
    fn shadings(n: usize) -> String {
        format!("<< {}>>", (0..n).map(|i| format!("/Sh{i} {i} 0 R ")).collect::<String>())
    }

    fn shared_obj(s: &str, dicts: &SharedDicts) -> Object {
        Lexer::new(s.as_bytes(), 0).sharing(dicts).object().unwrap_or_else(|e| panic!("{e}"))
    }

    fn at<'a>(o: &'a Object, path: &[&[u8]]) -> &'a Dict {
        path.iter().fold(o.as_dict().expect("a dictionary"), |d, key| d.get(key).and_then(Object::as_dict).expect("a dictionary"))
    }

    #[test]
    fn a_large_nested_dictionary_is_parsed_once_for_all_its_copies() {
        let big = shadings(1000);
        assert!(big.len() >= SHARE_FROM);
        let dicts = SharedDicts::default();
        // Each page's /Resources differs (its own XObject); the /Shading list in it does not.
        let page = |n: usize| format!("<< /Type /Page /Resources << /Shading {big} /XObject << /Fm1 {n} 0 R >> >> >>");
        let pages: Vec<Object> = (1..4).map(|n| shared_obj(&page(n), &dicts)).collect();
        let shading = |o: &Object| at(o, &[b"Resources", b"Shading"]).clone();
        assert!(shading(&pages[0]).shares(&shading(&pages[1])) && shading(&pages[1]).shares(&shading(&pages[2])));
        assert!(!at(&pages[0], &[b"Resources"]).shares(at(&pages[1], &[b"Resources"])));
        // Read as a fresh parse reads it, and only the innermost large dictionary is kept.
        for (n, p) in (1..4).zip(&pages) {
            assert_eq!(*p, obj(&page(n)));
        }
        assert_eq!(dicts.count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_kept_dictionary_is_reused_only_for_the_same_bytes_at_the_same_depth() {
        let big = shadings(1000);
        let dicts = SharedDicts::default();
        let first = shared_obj(&format!("<< /S {big} >>"), &dicts);
        // The last entry differs: compared byte for byte, not by its start.
        let changed = big.replacen("/Sh999 999 0 R", "/Sh999 998 0 R", 1);
        let other = shared_obj(&format!("<< /S {changed} >>"), &dicts);
        assert!(!at(&other, &[b"S"]).shares(at(&first, &[b"S"])));
        assert_eq!(at(&other, &[b"S"]).get(b"Sh999"), Some(&Object::Ref(ObjRef::new(998, 0))));
        // One level deeper the same bytes are read afresh, so nesting limits apply there as before.
        let deeper = shared_obj(&format!("<< /A << /S {big} >> >>"), &dicts);
        assert!(!at(&deeper, &[b"A", b"S"]).shares(at(&first, &[b"S"])));
        assert_eq!(deeper, obj(&format!("<< /A << /S {big} >> >>")));
        // A top-level dictionary is never taken from the table.
        let top = shared_obj(&big, &dicts);
        assert!(!at(&top, &[]).shares(at(&first, &[b"S"])));
    }

    #[test]
    fn a_shared_dictionary_counts_its_nodes_against_the_budget() {
        let big = shadings(1000);
        let copies = |n: usize| format!("[{}]", format!("{big} ").repeat(n));
        let nodes = |text: &str, dicts: Option<&SharedDicts>| {
            let mut lx = Lexer::new(text.as_bytes(), 0);
            lx.shared = dicts;
            lx.object().unwrap();
            lx.nodes
        };
        let dicts = SharedDicts::default();
        assert_eq!(nodes(&copies(3), Some(&dicts)), nodes(&copies(3), None));
        // Copies past the budget fail as they do without sharing: 5,002 nodes in 10 kB each.
        let wide = format!("<< /A [{}] >>", "0 ".repeat(5000));
        let past = format!("[{}]", format!("{wide} ").repeat(MAX_NODES / 5002 + 1));
        assert!(Lexer::new(past.as_bytes(), 0).object().is_err());
        assert!(Lexer::new(past.as_bytes(), 0).sharing(&dicts).object().is_err());
        let below = format!("[{}]", format!("{wide} ").repeat(MAX_NODES / 5002 - 1));
        assert!(Lexer::new(below.as_bytes(), 0).sharing(&dicts).object().is_ok());
    }

    #[test]
    fn the_kept_dictionaries_are_bounded_and_reused_ones_stay() {
        let dicts = SharedDicts::default();
        let reused = shadings(1000);
        for _ in 0..2 {
            shared_obj(&format!("[{reused}]"), &dicts);
        }
        for i in 0..2 * SHARED_MAX {
            shared_obj(&format!("[{}]", shadings(1000 + i)), &dicts);
            assert!(dicts.count.load(Ordering::Relaxed) <= SHARED_MAX);
        }
        let entries = dicts.entries.lock().unwrap();
        assert!(entries.iter().any(|e| *e.bytes == *reused.as_bytes()), "the dictionary read twice is still kept");
        assert!(entries.iter().map(|e| e.bytes.len()).sum::<usize>() <= SHARED_BYTES);
    }

    /// `<< /K tag >>` padded with spaces to exactly `len` bytes: large in bytes, few nodes.
    fn padded(len: usize, tag: usize) -> String {
        let pad = len.checked_sub(format!("<< /K {tag} >>").len()).expect("room for the padding");
        format!("<< /K {tag} {}>>", " ".repeat(pad))
    }

    fn kept(dicts: &SharedDicts) -> Vec<Vec<u8>> {
        dicts.entries.lock().unwrap().iter().map(|e| e.bytes.to_vec()).collect()
    }

    #[test]
    fn a_dictionary_of_exactly_the_sharing_size_is_kept_and_one_byte_less_is_not() {
        let (at_size, below) = (padded(SHARE_FROM, 1), padded(SHARE_FROM - 1, 2));
        assert_eq!((at_size.len(), below.len()), (SHARE_FROM, SHARE_FROM - 1));
        let dicts = SharedDicts::default();
        let read = |d: &str| shared_obj(&format!("<< /S {d} >>"), &dicts);
        let (first, second) = (read(&below), read(&below));
        assert!(kept(&dicts).is_empty());
        assert!(!at(&second, &[b"S"]).shares(at(&first, &[b"S"])));
        let (first, second) = (read(&at_size), read(&at_size));
        assert_eq!(kept(&dicts), [at_size.as_bytes()]);
        assert!(at(&second, &[b"S"]).shares(at(&first, &[b"S"])));
        assert_eq!(second, obj(&format!("<< /S {at_size} >>")));
    }

    #[test]
    fn the_kept_dictionaries_stay_within_their_byte_budget() {
        // Few entries but many bytes each, so the byte budget is reached before the entry count.
        let half = SHARED_BYTES / 2 + 1;
        let (a, b, c) = (padded(half, 1), padded(half, 2), padded(half, 3));
        let dicts = SharedDicts::default();
        let read = |d: &str, dicts: &SharedDicts| shared_obj(&format!("<< /S {d} >>"), dicts);
        // Two halves don't fit together: the second replaces the first, which no copy reused.
        read(&a, &dicts);
        read(&b, &dicts);
        assert_eq!(kept(&dicts), [b.as_bytes()]);
        // A kept dictionary that a copy reused stays; the one that does not fit beside it is not kept.
        let again = read(&b, &dicts);
        read(&c, &dicts);
        assert_eq!(kept(&dicts), [b.as_bytes()]);
        assert_eq!(again, obj(&format!("<< /S {b} >>")));
        // A dictionary of exactly the budget is kept; one byte more never is.
        let fresh = SharedDicts::default();
        read(&padded(SHARED_BYTES + 1, 4), &fresh);
        assert!(kept(&fresh).is_empty());
        let whole = padded(SHARED_BYTES, 5);
        read(&whole, &fresh);
        assert_eq!(kept(&fresh), [whole.as_bytes()]);
    }

    #[test]
    fn a_repeated_key_reads_through_a_kept_dictionary_as_it_reads_afresh() {
        // The same name twice in one large dictionary: the copy taken from the table keeps
        // whichever entry a fresh parse keeps.
        let text = format!("<< /S {} >>", shadings(1000).replacen(">>", "/Sh5 777 0 R >>", 1));
        let dicts = SharedDicts::default();
        let (first, second) = (shared_obj(&text, &dicts), shared_obj(&text, &dicts));
        assert!(at(&second, &[b"S"]).shares(at(&first, &[b"S"])));
        let fresh = obj(&text);
        assert_eq!(at(&second, &[b"S"]).get(b"Sh5"), at(&fresh, &[b"S"]).get(b"Sh5"));
        assert_eq!(at(&second, &[b"S"]).len(), at(&fresh, &[b"S"]).len());
        assert_eq!(second, fresh);
    }

    #[test]
    fn a_damaged_large_dictionary_reads_through_sharing_as_it_reads_without() {
        let big = shadings(1000);
        let damaged = [
            big.replacen("/Sh500 500 0 R", "/Sh500 500 0 R ) ]", 1),       // stray tokens among the entries
            big.replacen(">>", "/Last >>", 1),                             // a key without a value
            big.replacen("/Sh500 500 0 R", "(key) 500 /Sh500 500 0 R", 1), // a string where a key belongs
            big.replacen(">>", "", 1),                                     // no end of its own
        ];
        for (n, d) in damaged.iter().enumerate() {
            for text in [format!("<< /S {d} >>"), format!("[{d}]"), format!("[{d}")] {
                let fresh = Lexer::new(text.as_bytes(), 0).object().ok();
                let dicts = SharedDicts::default();
                // The second read can take what the first one kept; both read as a fresh parse.
                for read in 0..2 {
                    assert_eq!(Lexer::new(text.as_bytes(), 0).sharing(&dicts).object().ok(), fresh, "case {n}, read {read}: {}", &text[..40]);
                }
                // Whatever was kept reads, from its own bytes, as the dictionary kept for them.
                for e in dicts.entries.lock().unwrap().iter() {
                    let alone = obj(&format!("<< /S {} >>", String::from_utf8_lossy(&e.bytes)));
                    assert_eq!(at(&alone, &[b"S"]), &e.dict, "case {n}");
                }
            }
        }
        // A copy cut short by the end of the data is not taken for the kept dictionary.
        let dicts = SharedDicts::default();
        shared_obj(&format!("<< /S {big} >>"), &dicts);
        let cut = format!("<< /S {}", &big[..big.len() - 1]);
        assert!(Lexer::new(cut.as_bytes(), 0).object().is_err());
        assert!(Lexer::new(cut.as_bytes(), 0).sharing(&dicts).object().is_err());
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
}
