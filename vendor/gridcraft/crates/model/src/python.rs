//! Python cells: the code a cell holds, its last result, and the text forms around it (the
//! `PY("…",0|1)` formula of the function's syntax, and the `xl("…")` references inside it). An
//! xlsx file keeps the code in its Python part instead (see the xlsx crate).

use gridcraft_core::{CellError, Value};
use serde::{Deserialize, Serialize};

/// Excel's limit on the characters of one formula. A Python cell's code must fit in it once it is
/// written as the PY formula of the function's syntax ([`PyCode::to_file_formula`]): quotes are
/// doubled and every 255 characters add `"&"`, so the limit on the code itself depends on what the
/// code holds.
pub const MAX_FORMULA_CHARS: usize = 8_192;

/// The most cells a Python result may have (a crafted file can claim any size).
pub const MAX_RESULT_CELLS: usize = 1_000_000;

/// The prefix files put before `PY(`, as Excel writes it ([MS-XLSX]: `_xlfn._xlws.PY`).
pub const FILE_PREFIX: &str = "_xlfn._xlws.";

/// Characters in one string literal of a `PY(…)` formula (Excel's limit for a literal).
const CHUNK_CHARS: usize = 255;

/// A Python cell.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PyCode {
    pub code: String,
    /// Show the result as a Python object (true) or convert it to cell values (false).
    pub object: bool,
    /// The last result, when the code ran without an error.
    pub output: Option<PyOutput>,
    /// The cell's inputs changed while calculation was manual: its value is out of date until
    /// the workbook is calculated.
    pub stale: bool,
}

/// A Python result kept on the cell.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PyOutput {
    /// The result as cells (an array spills), or its preview as text when it has no cell form.
    pub value: Value,
    /// Python's name for the result's type.
    pub type_name: String,
    /// The result's `repr`, shortened.
    pub preview: String,
    /// Whether `value` is the result converted to cells.
    pub convertible: bool,
}

impl PyCode {
    pub fn new(code: &str) -> PyCode {
        PyCode { code: normalize_code(code), ..PyCode::default() }
    }

    /// What the cell shows for its result: cell values (an array spills from the cell), or, for a
    /// Python object, its type name. `None` before the code has run.
    pub fn result_value(&self) -> Option<Value> {
        let out = self.output.as_ref()?;
        let scalar = !matches!(out.value, Value::Array(_)) && out.convertible;
        Some(if self.object && !scalar { Value::text(out.type_name.as_str()) } else { out.value.clone() })
    }

    /// The cell as a formula (without `=`): `PY("…"&"…",0)`, with the code in string literals of
    /// at most 255 characters and the return type Excel documents: 0 for an Excel value, 1 for a
    /// Python object.
    pub fn to_formula(&self) -> String {
        let chunks = chunk(&self.code, CHUNK_CHARS);
        let lits: Vec<String> = chunks.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect();
        let lits = if lits.is_empty() { "\"\"".to_string() } else { lits.join("&") };
        format!("PY({lits},{})", u8::from(self.object))
    }

    /// [`PyCode::to_formula`] with Excel's `_xlfn._xlws.` prefix: the string-literal form
    /// [`PyCode::from_formula`] reads from files that hold it, and what the size limit measures.
    /// An xlsx file names the code by index instead ([MS-XLSX] 2.3.10.2).
    pub fn to_file_formula(&self) -> String {
        format!("{FILE_PREFIX}{}", self.to_formula())
    }

    /// Whether the code fits in a PY formula of at most [`MAX_FORMULA_CHARS`] characters.
    pub fn fits(&self) -> bool {
        // Code longer than the limit can't fit; this also bounds the work for hostile input.
        self.code.chars().nth(MAX_FORMULA_CHARS).is_none() && self.to_file_formula().chars().nth(MAX_FORMULA_CHARS).is_none()
    }

    /// Reads a `PY(…)` formula (with or without `=`, and with or without the `_xlfn.` and
    /// `_xlws.` prefixes): string literals joined by `&`, optionally followed by `,0` or `,1`.
    /// `None` for anything else.
    pub fn from_formula(text: &str) -> Option<PyCode> {
        let body = py_body(text);
        let rest = body.get(..3).filter(|h| h.eq_ignore_ascii_case("PY(")).and_then(|_| body.get(3..))?;
        let mut chars = rest.chars().peekable();
        let mut code = String::new();
        loop {
            skip_space(&mut chars);
            if chars.next()? != '"' {
                return None;
            }
            loop {
                match chars.next()? {
                    '"' if chars.peek() == Some(&'"') => {
                        chars.next();
                        code.push('"');
                    }
                    '"' => break,
                    c => code.push(c),
                }
            }
            skip_space(&mut chars);
            match chars.next()? {
                '&' => continue,
                ',' => {
                    skip_space(&mut chars);
                    let object = match chars.next()? {
                        '0' => false,
                        '1' => true,
                        _ => return None,
                    };
                    skip_space(&mut chars);
                    return (chars.next()? == ')' && chars.all(char::is_whitespace)).then(|| PyCode { object, ..PyCode::new(&code) });
                }
                ')' => return chars.all(char::is_whitespace).then(|| PyCode::new(&code)),
                _ => return None,
            }
        }
    }
}

/// `text` without a leading `=`, spaces and the `_xlfn.` and `_xlws.` prefixes files put before
/// newer functions (in any letter case).
fn py_body(text: &str) -> &str {
    let mut body = text.trim().trim_start_matches('=').trim_start();
    for prefix in ["_xlfn.", "_xlws."] {
        if body.get(..prefix.len()).is_some_and(|h| h.eq_ignore_ascii_case(prefix)) {
            body = body.get(prefix.len()..).unwrap_or_default();
        }
    }
    body
}

fn skip_space(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while chars.peek().is_some_and(|c| c.is_whitespace()) {
        chars.next();
    }
}

/// Line breaks as `\n`.
pub fn normalize_code(code: &str) -> String {
    code.replace("\r\n", "\n").replace('\r', "\n")
}

/// `s` in pieces of at most `n` characters.
fn chunk(s: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(n.max(1)).map(|c| c.iter().collect()).collect()
}

/// Whether `text` (a formula, with or without `=`) is a `PY(` call: such formulas are Python cells,
/// set with `python.set`.
pub fn is_py_formula(text: &str) -> bool {
    let body = py_body(text);
    body.get(..3).is_some_and(|h| h.eq_ignore_ascii_case("PY(")) || body.eq_ignore_ascii_case("PY")
}

/// An `xl()` call in Python code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XlCall {
    /// The reference text, when the first argument is a plain string literal.
    pub reference: Option<String>,
    /// Byte range of the literal's contents (between the quotes) in the code.
    pub span: Option<std::ops::Range<usize>>,
}

/// The `xl(…)` calls in `code`, skipping comments and other string literals. A call whose first
/// argument isn't a plain string literal has no reference: the cells it reads are only known when
/// it runs.
pub fn xl_calls(code: &str) -> Vec<XlCall> {
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'#' => {
                // A comment runs to the end of the line.
                while bytes.get(i).is_some_and(|c| *c != b'\n') {
                    i += 1;
                }
            }
            b'"' | b'\'' => i = skip_string(bytes, i),
            b'x' if bytes.get(i + 1) == Some(&b'l') && (i == 0 || bytes.get(i - 1).is_some_and(|p| !is_ident(*p) && *p != b'.')) => {
                let mut j = i + 2;
                if bytes.get(j).is_some_and(|c| is_ident(*c)) {
                    i = j;
                    continue;
                }
                while bytes.get(j).is_some_and(|c| *c == b' ' || *c == b'\t') {
                    j += 1;
                }
                if bytes.get(j) != Some(&b'(') {
                    i = j;
                    continue;
                }
                j += 1;
                while bytes.get(j).is_some_and(|c| c.is_ascii_whitespace()) {
                    j += 1;
                }
                let call = match bytes.get(j) {
                    Some(&q @ (b'"' | b'\'')) => {
                        let start = j + 1;
                        let end = skip_string(bytes, j);
                        // A plain literal: closed on its own line, not part of an expression
                        // (`xl("A" + n)`), not a triple-quoted string, and escaping nothing but a
                        // quote or a backslash (`\'`, `\"`, `\\`), which a sheet name may need.
                        let close = end.saturating_sub(1);
                        let mut k = end;
                        while bytes.get(k).is_some_and(|c| *c == b' ' || *c == b'\t') {
                            k += 1;
                        }
                        let plain = close > start
                            && bytes.get(close) == Some(&q)
                            && bytes.get(start) != Some(&q)
                            && matches!(bytes.get(k), Some(b')' | b','));
                        i = end;
                        match code.get(start..close).filter(|_| plain).and_then(unescape) {
                            Some(t) => XlCall { reference: Some(t), span: Some(start..close) },
                            None => XlCall { reference: None, span: None },
                        }
                    }
                    _ => {
                        i = j;
                        XlCall { reference: None, span: None }
                    }
                };
                out.push(call);
            }
            _ => i += 1,
        }
    }
    out
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// The index just past the string literal starting at `at` (single, double or triple quoted).
fn skip_string(bytes: &[u8], at: usize) -> usize {
    let Some(&q) = bytes.get(at) else { return at.saturating_add(1) };
    let triple = bytes.get(at + 1) == Some(&q) && bytes.get(at + 2) == Some(&q);
    let mut i = if triple { at + 3 } else { at + 1 };
    while let Some(&c) = bytes.get(i) {
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == q {
            if !triple {
                return i + 1;
            }
            if bytes.get(i + 1) == Some(&q) && bytes.get(i + 2) == Some(&q) {
                return i + 3;
            }
        } else if c == b'\n' && !triple {
            return i;
        }
        i += 1;
    }
    i
}

/// The text of a plain literal's contents: `\'`, `\"` and `\\` stand for the character after the
/// backslash. `None` for any other escape or a line break, which make the literal not plain.
fn unescape(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\n' => return None,
            '\\' => match chars.next()? {
                e @ ('\\' | '\'' | '"') => out.push(e),
                _ => return None,
            },
            c => out.push(c),
        }
    }
    Some(out)
}

/// `text` as a plain literal that was delimited by `quote`: its delimiter (`quote`, or the other
/// quote when `text` holds `quote` but not the other one, so `xl('Sheet2!B2')` becomes
/// `xl("'Q4 2026'!B2")` on a rename) and its contents, with backslashes escaped, and the
/// delimiter too when `text` holds both quotes. `None` for a line break, which no literal on
/// one line holds.
fn literal(text: &str, quote: char) -> Option<(char, String)> {
    if text.contains('\n') {
        return None;
    }
    let other = if quote == '\'' { '"' } else { '\'' };
    let delim = if text.contains(quote) && !text.contains(other) { other } else { quote };
    let mut body = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' || c == delim {
            body.push('\\');
        }
        body.push(c);
    }
    Some((delim, body))
}

/// Rewrites the reference of every `xl("…")` literal in `code` with `f`; `None` from `f` leaves
/// it. Returns the new code when anything changed. A new reference holding the literal's quote
/// (a sheet name that needs quotes, `'Q4 2026'!B2`) gets the other quote as its delimiter, or an
/// escaped quote when it holds both, so it never ends the string early.
pub fn rewrite_xl(code: &str, mut f: impl FnMut(&str) -> Option<String>) -> Option<String> {
    let mut out = String::with_capacity(code.len());
    let mut last = 0usize;
    let mut changed = false;
    for call in xl_calls(code) {
        let (Some(reference), Some(span)) = (call.reference, call.span) else { continue };
        let Some(new) = f(&reference).filter(|n| *n != reference) else { continue };
        let open = span.start.saturating_sub(1);
        let Some(quote) = code.get(open..span.start).and_then(|q| q.chars().next()) else { continue };
        let Some((delim, body)) = literal(&new, quote) else { continue };
        // The literal with its quotes: the delimiter may change.
        out.push_str(code.get(last..open).unwrap_or_default());
        out.push(delim);
        out.push_str(&body);
        out.push(delim);
        last = span.end.saturating_add(1);
        changed = true;
    }
    if !changed {
        return None;
    }
    out.push_str(code.get(last..).unwrap_or_default());
    Some(out)
}

/// The value a Python cell shows when it can't run: `#BLOCKED!`.
pub const BLOCKED: Value = Value::Error(CellError::Blocked);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_round_trip_with_chunks_and_quotes() {
        let code = format!("s = \"quoted\"\n{}\nlen(s)", "x".repeat(600));
        let py = PyCode { object: true, ..PyCode::new(&code) };
        let f = py.to_formula();
        assert!(f.starts_with("PY(\"s = \"\"quoted\"\"\n"));
        assert!(f.ends_with(",1)"));
        assert_eq!(f.matches("\"&\"").count(), 2, "{f}");
        let back = PyCode::from_formula(&format!("={f}")).unwrap();
        assert_eq!(back.code, code);
        assert!(back.object);
        assert_eq!(PyCode::from_formula("_xlfn.PY(\"1+1\")").unwrap().code, "1+1");
        // Excel writes `_xlfn._xlws.PY`; the reader takes it in any letter case.
        assert_eq!(PyCode::from_formula("=_xlfn._xlws.PY(\"1+1\",1)").unwrap().code, "1+1");
        assert_eq!(PyCode::from_formula("_XLFN._XLWS.py(\"2\",0)").unwrap().code, "2");
        assert_eq!(PyCode::from_formula("_xlws.PY(\"3\",0)").unwrap().code, "3");
        assert!(is_py_formula("=_xlfn._xlws.PY(\"1\",0)"));
        assert_eq!(py.to_file_formula(), format!("_xlfn._xlws.{f}"));
        assert_eq!(PyCode::from_formula(&py.to_file_formula()).unwrap(), PyCode::from_formula(&f).unwrap());
        assert_eq!(PyCode::from_formula(" py( \"a\" & \"b\" , 0 ) ").unwrap().code, "ab");
        assert_eq!(PyCode::new("a\r\nb").code, "a\nb");
        for bad in ["PY(A1)", "PY(\"a\",2)", "PY(\"a\"", "PY(\"a\")+1", "SUM(1)", "PY(\"a\"&B1)", "PY(", ""] {
            assert!(PyCode::from_formula(bad).is_none(), "{bad}");
        }
        assert!(is_py_formula("=PY(\"1\")") && is_py_formula("=py") && !is_py_formula("=PYTHON(1)"));
    }

    #[test]
    fn the_size_limit_counts_the_encoded_formula() {
        // Plain code: the limit is the formula's, so it is a little under 8,192 characters of code.
        // `_xlfn._xlws.PY(` + 31 literals of 255 + 30 `&` + `&"…"` + `,0)`: 8,079 characters of
        // code give a formula of exactly 8,192.
        let at = PyCode::new(&"y".repeat(8_079));
        assert_eq!(at.to_file_formula().chars().count(), MAX_FORMULA_CHARS);
        assert!(at.fits());
        assert!(!PyCode::new(&"y".repeat(8_080)).fits());
        assert_eq!(PyCode::from_formula(&at.to_file_formula()).unwrap().code, at.code);
        // Quotes are doubled in the file, so 7,000 characters with 1,200 of them don't fit.
        let quoted = format!("{}{}", "\"".repeat(1_200), "y".repeat(5_800));
        assert!(PyCode::new(&quoted).to_file_formula().chars().count() > MAX_FORMULA_CHARS);
        assert!(!PyCode::new(&quoted).fits());
        // Hostile sizes are refused without encoding them.
        assert!(!PyCode::new(&"y".repeat(4_000_000)).fits());
        assert!(PyCode::new("").fits());
    }

    #[test]
    fn result_values() {
        let arr = gridcraft_core::Array::new(2, 1, vec![1.0.into(), 2.0.into()]).map(Value::from).unwrap();
        let out = PyOutput { value: arr.clone(), type_name: "list".into(), preview: "[1, 2]".into(), convertible: true };
        let mut py = PyCode { output: Some(out), ..PyCode::new("[1, 2]") };
        assert_eq!(py.result_value(), Some(arr));
        py.object = true;
        assert_eq!(py.result_value(), Some(Value::text("list")));
        py.output = Some(PyOutput { value: Value::Number(3.0), type_name: "int".into(), preview: "3".into(), convertible: true });
        assert_eq!(py.result_value(), Some(Value::Number(3.0)), "scalars show as values in object mode too");
        py.output = None;
        assert_eq!(py.result_value(), None);
    }

    #[test]
    fn finds_xl_literals() {
        let code = "a = xl(\"A1:B2\", headers=True)\n# xl('C1') in a comment\ns = \"xl('D1')\"\nb = xl( 'Sheet2!E5' )\nc = xl(name)\nd = df.xl('F1')\nxlx('G1')\ne = xl(\"A\" + \"1\")";
        let calls = xl_calls(code);
        let refs: Vec<Option<&str>> = calls.iter().map(|c| c.reference.as_deref()).collect();
        assert_eq!(refs, vec![Some("A1:B2"), Some("Sheet2!E5"), None, None]);
        let new = rewrite_xl(code, |r| (r == "A1:B2").then(|| "A2:B3".to_string())).unwrap();
        assert!(new.starts_with("a = xl(\"A2:B3\", headers=True)"));
        assert_eq!(new.len(), code.len());
        assert_eq!(rewrite_xl(code, |_| None), None);
        // A replacement can't break out of its string: a quote in it changes the delimiter, or is
        // escaped when it holds both quotes, and the result reads back as the same reference.
        assert_eq!(rewrite_xl("xl(\"A1\")", |_| Some("'Q'!A1".into())), Some("xl(\"'Q'!A1\")".into()));
        assert_eq!(rewrite_xl("xl('Sheet2!B2', headers=True)", |_| Some("'Q4 2026'!B2".into())), Some("xl(\"'Q4 2026'!B2\", headers=True)".into()));
        assert_eq!(rewrite_xl("xl(\"A1\")", |_| Some("'Q\"4'!A1".into())), Some("xl(\"'Q\\\"4'!A1\")".into()));
        assert_eq!(rewrite_xl("xl('A1')", |_| Some("'Q\"4'!A1".into())), Some("xl('\\'Q\"4\\'!A1')".into()));
        for (delim, new) in [("'", "'Q4 2026'!B2"), ("'", "'Q\"4'!A1"), ("\"", "'Q\"4'!A1"), ("\"", "'a\\b'!A1"), ("'", "B2")] {
            let code = format!("x = xl({delim}A1{delim}) + 1");
            let out = rewrite_xl(&code, |_| Some(new.to_string())).unwrap();
            assert_eq!(xl_calls(&out)[0].reference.as_deref(), Some(new), "{out}");
            assert!(out.ends_with(") + 1"), "{out}");
        }
        assert_eq!(rewrite_xl("xl('A1')", |_| Some("A\n1".into())), None, "a line break can't go in a literal");
        // Escaped quotes and backslashes read as the characters; any other escape is not plain.
        assert_eq!(xl_calls("xl('Bob\\'s!A1')")[0].reference.as_deref(), Some("Bob's!A1"));
        assert_eq!(xl_calls("xl(\"A\\\\1\")")[0].reference.as_deref(), Some("A\\1"));
        assert_eq!(xl_calls("xl(\"A\\n1\")")[0].reference, None);
        assert_eq!(xl_calls("xl(\"A1\\\")")[0].reference, None, "an escaped closing quote leaves the literal open");
        // Unclosed and odd input doesn't loop or panic.
        for odd in ["xl(", "xl(\"", "xl('''A1''')", "x", "xl", "\"unterminated", "xl  (  \"A1\"  )", "é xl('A1')"] {
            let _ = xl_calls(odd);
            let _ = rewrite_xl(odd, |_| Some("B2".into()));
        }
        assert_eq!(xl_calls("xl  (  \"A1\"  )")[0].reference.as_deref(), Some("A1"));
        assert_eq!(xl_calls("xl('''A1''')")[0].reference, None);
    }
}
