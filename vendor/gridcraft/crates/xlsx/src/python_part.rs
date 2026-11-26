//! The Python part ([MS-XLSX] 2.1.23, 2.3.10): where a workbook keeps the code of its Python cells.
//!
//! A Python cell's formula is `_xlfn._xlws.PY(index, return_type, …)` ([MS-XLSX] 2.3.10.2): the
//! first argument is the zero-based index of a `pythonScript` element in this part, whose `code`
//! is the cell's Python code, and the second is 0 (an Excel value) or 1 (a Python object). Further
//! arguments are references the code names as `%Px%`, `x` being the argument's zero-based index
//! ([MS-XLSX] 2.3.10.4). The part's root is `python` ([MS-XLSX] 2.4.102, CT_Python 2.6.249), with
//! `pythonScripts` (2.6.253) holding the `pythonScript` elements (2.6.252).
//!
//! GridCraft writes one script per Python cell, in the order the cells are written, with the code
//! as it is (its `xl()` references stay text in the code, so no further arguments). Reading
//! resolves the index and puts each `%Px%` back as the text of argument `x`. A formula that names
//! no script the part has keeps only its cached value. Hostile input is bounded: the part's size,
//! the number of scripts, each script's length, and the code all cells take from the part
//! together.

use gridcraft_formula::Expr;
use gridcraft_model::PyCode;
use gridcraft_model::python::{FILE_PREFIX, MAX_FORMULA_CHARS};

use crate::IoError;
use crate::read::Ctx;
use crate::xml::{self, El};

/// Namespace of the part.
pub const NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2023/python";
/// Relationship type from the workbook part.
pub const REL: &str = "http://schemas.microsoft.com/office/2023/09/relationships/Python";
/// Last segment of [`REL`], as [`crate::package::Rel::kind`] holds it.
const REL_KIND: &str = "Python";
/// Content type of the part.
pub const CONTENT_TYPE: &str = "application/vnd.ms-excel.python+xml";
/// Part name we write (the format names none; the relationship finds it).
pub const PART: &str = "xl/python.xml";

/// Largest Python part read.
const MAX_PART_BYTES: u64 = 64 * 1024 * 1024;
/// Most scripts read from the part.
const MAX_SCRIPTS: usize = 1_000_000;
/// Most characters kept of one script: one more than a cell's code may have, so a longer script
/// still fails [`PyCode::fits`] and its cell keeps only its value, with the usual warning.
const MAX_SCRIPT_CHARS: usize = MAX_FORMULA_CHARS + 1;
/// Most bytes of code all the cells of a workbook take from the part together. Several formulas
/// may name one script, so without it a small part could fill memory.
pub const MAX_CODE_BYTES: usize = 64 * 1024 * 1024;

/// The scripts read from a workbook's Python part, and what the cells took from them.
#[derive(Default)]
pub struct ScriptsIn {
    scripts: Vec<String>,
    /// Bytes of code the cells may still take ([`MAX_CODE_BYTES`] in all).
    budget: usize,
    /// Python cells that kept only their value: no such script, or over the budget.
    missing: usize,
}

impl ScriptsIn {
    /// The warning for the Python cells that kept only their value, if any.
    pub fn warning(&self) -> Option<String> {
        (self.missing > 0).then(|| {
            format!(
                "{} Python cell(s) name code the file's Python part doesn't hold (or holds too much of); they kept only their values",
                self.missing
            )
        })
    }
}

/// Reads the workbook's Python part, if it has one. A damaged part is a warning: its cells keep
/// their values.
pub fn read(cx: &mut Ctx<'_>, rels: &[crate::package::Rel]) -> Result<ScriptsIn, IoError> {
    let mut out = ScriptsIn { budget: MAX_CODE_BYTES, ..ScriptsIn::default() };
    let Some(rel) = rels.iter().find(|r| !r.external && r.kind.eq_ignore_ascii_case(REL_KIND)) else { return Ok(out) };
    let bytes = match cx.pkg.read_optional_limited(&rel.target, MAX_PART_BYTES) {
        Ok(Some(b)) => b,
        Ok(None) => {
            cx.warn(format!("missing part {}", rel.target));
            return Ok(out);
        }
        Err(IoError::TooLarge(m)) => {
            cx.warn(format!("the Python part is too large ({m}); Python cells kept only their values"));
            return Ok(out);
        }
        // A damaged entry (a bad checksum or compressed stream) is a damaged part: its cells keep
        // their values, as when the part is missing.
        Err(e) => {
            cx.warn(format!("the Python part is damaged ({e}); Python cells kept only their values"));
            return Ok(out);
        }
    };
    let mut over = 0usize;
    let parsed = xml::parse_streaming(&bytes, &["pythonScript"], &mut |el: El| {
        if out.scripts.len() < MAX_SCRIPTS {
            out.scripts.push(script_code(&el));
        } else {
            over += 1;
        }
        Ok(())
    });
    match parsed {
        Ok(root) if root.name != "python" => {
            cx.warn(format!("the Python part has the root <{}>; Python cells kept only their values", root.name));
            out.scripts.clear();
        }
        Ok(_) => {}
        Err(e) => cx.warn(format!("the Python part is damaged ({e}); kept {} scripts", out.scripts.len())),
    }
    if over > 0 {
        cx.warn(format!("the Python part holds more than {MAX_SCRIPTS} scripts; {over} were not read"));
    }
    Ok(out)
}

/// Encoded characters of a script that are decoded: enough for its first [`MAX_SCRIPT_CHARS`]
/// characters of code, whatever its escapes. One character takes at most 14 encoded characters
/// (a UTF-16 pair of `_xHHHH_` escapes) and a `\r\n` line break becomes one character, so
/// `2 × (MAX_SCRIPT_CHARS + 1)` decoded characters cover the code kept, and two more escapes'
/// worth keep the last of them, and the line break after it, from being cut.
const MAX_ENCODED_CHARS: usize = 14 * (2 * (MAX_SCRIPT_CHARS + 1) + 2);

/// A `pythonScript` element's code (ST_Xstring), with `\n` line breaks, cut after
/// [`MAX_SCRIPT_CHARS`] characters. The limit is measured on the decoded code, so escapes in the
/// file (`_x000D_`, `_x0009_`) don't cut a script that fits; decoding is linear and reads at most
/// [`MAX_ENCODED_CHARS`] characters.
fn script_code(el: &El) -> String {
    let text = el.child("code").map(|c| c.text.as_str()).unwrap_or_default();
    let encoded = match text.char_indices().nth(MAX_ENCODED_CHARS) {
        Some((at, _)) => text.get(..at).unwrap_or_default(),
        None => text,
    };
    let code = gridcraft_model::python::normalize_code(&xml::decode_xstring(encoded));
    match code.char_indices().nth(MAX_SCRIPT_CHARS) {
        Some((at, _)) => code.get(..at).unwrap_or_default().to_string(),
        None => code,
    }
}

/// `PY(index, 0|1, params…)`, as a file's formula holds it: the index, whether the return type is
/// a Python object, and the parameters. `None` for anything else (the code in a string literal
/// is [`PyCode::from_formula`]'s form).
fn py_call(expr: &Expr) -> Option<(usize, bool, &[Expr])> {
    let Expr::Call(name, args) = expr else { return None };
    let base = name.trim_start_matches("_XLFN.").trim_start_matches("_XLWS.");
    if !base.eq_ignore_ascii_case("PY") {
        return None;
    }
    let index = match args.first()? {
        Expr::Number(n) if *n >= 0.0 && n.fract() == 0.0 && *n < MAX_SCRIPTS as f64 => *n as usize,
        _ => return None,
    };
    let object = match args.get(1)? {
        Expr::Number(n) if *n == 0.0 => false,
        Expr::Number(n) if *n == 1.0 => true,
        _ => return None,
    };
    Some((index, object, args.get(2..).unwrap_or_default()))
}

/// What a file formula is to Python.
#[derive(Debug, PartialEq)]
pub enum PyFormula {
    /// Not a `PY(…)` call: an ordinary formula.
    Not,
    /// A Python cell with this code.
    Code(PyCode),
    /// A `PY(index, …)` call naming a script the part doesn't hold (or over the budget): the
    /// cell keeps only its cached value, since no calculation can evaluate the formula.
    Missing,
}

/// What a file formula makes: a Python cell from the Python part when it names a script, or from
/// string literals ([`PyCode::from_formula`]). `formula` is the parsed formula, `raw` the
/// formula's own text, which keeps line breaks in literal code.
pub fn cell_code(scripts: &mut ScriptsIn, formula: &gridcraft_model::Formula, raw: &str) -> PyFormula {
    // The cheap test first: most formulas are not `PY(` calls.
    if !gridcraft_model::python::is_py_formula(&formula.text) {
        return PyFormula::Not;
    }
    let expr = formula.expr_arc();
    let Some((index, object, params)) = expr.as_deref().and_then(py_call) else {
        return PyCode::from_formula(raw).map_or(PyFormula::Not, PyFormula::Code);
    };
    let code = scripts.scripts.get(index).map(|code| with_params(code, params));
    match code.filter(|c| c.len() <= scripts.budget) {
        Some(code) => {
            scripts.budget -= code.len();
            PyFormula::Code(PyCode { object, ..PyCode::new(&code) })
        }
        None => {
            scripts.missing += 1;
            PyFormula::Missing
        }
    }
}

/// `code` with every `%Px%` (`x` at least 2, naming one of `params`) replaced by the parameter's
/// text. Stops growing past [`MAX_SCRIPT_CHARS`] characters: such code can't be a cell's.
fn with_params(code: &str, params: &[Expr]) -> String {
    if params.is_empty() || !code.contains("%P") {
        return code.to_string();
    }
    let texts: Vec<String> = params.iter().map(gridcraft_formula::print).collect();
    let mut out = String::with_capacity(code.len());
    let mut chars = 0usize;
    let mut push = |out: &mut String, piece: &str| {
        out.push_str(piece);
        chars = chars.saturating_add(piece.chars().count());
        chars > MAX_SCRIPT_CHARS
    };
    let mut rest = code;
    while let Some(at) = rest.find("%P") {
        if push(&mut out, rest.get(..at).unwrap_or_default()) {
            return out;
        }
        let after = rest.get(at + 2..).unwrap_or_default();
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let param = (1..=6)
            .contains(&digits)
            .then(|| after.get(..digits).and_then(|d| d.parse::<usize>().ok()))
            .flatten()
            .filter(|_| after.as_bytes().get(digits) == Some(&b'%'))
            .and_then(|x| x.checked_sub(2))
            .and_then(|x| texts.get(x));
        let full = match param {
            Some(text) => {
                rest = after.get(digits + 1..).unwrap_or_default();
                push(&mut out, text)
            }
            None => {
                rest = after;
                push(&mut out, "%P")
            }
        };
        if full {
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// The formula a Python cell is saved as: `_xlfn._xlws.PY(index,0|1)`, its code being script
/// `index` of the part ([`write`]).
pub fn file_formula(index: usize, object: bool) -> String {
    format!("{FILE_PREFIX}PY({index},{})", u8::from(object))
}

/// The Python part holding `scripts`, in order. Only written when there is at least one: the
/// format requires one `pythonScript` in `pythonScripts`.
pub fn write(scripts: &[String]) -> String {
    let mut xml = format!("{}<python xmlns=\"{NS}\"><pythonScripts>", crate::write::XML_DECL);
    for code in scripts {
        xml.push_str("<pythonScript><code>");
        xml.push_str(&xml::esc(&xml::encode_xstring(code)));
        xml.push_str("</code></pythonScript>");
    }
    xml.push_str("</pythonScripts></python>");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formula(text: &str) -> gridcraft_model::Formula {
        gridcraft_model::Formula::new(&crate::fmla::from_file(text))
    }

    fn scripts(list: &[&str]) -> ScriptsIn {
        ScriptsIn { scripts: list.iter().map(|s| s.to_string()).collect(), budget: MAX_CODE_BYTES, missing: 0 }
    }

    #[test]
    fn reads_the_index_form_and_parameters() {
        let mut s = scripts(&["1 + 1", "df = xl(\"%P2%\", headers=True)\nxl('%P3%') + %P9% + %P1%"]);
        let code = |s: &mut ScriptsIn, text: &str| match cell_code(s, &formula(text), text) {
            PyFormula::Code(py) => Some(py),
            _ => None,
        };
        let a = code(&mut s, "_xlfn._xlws.PY(0,1)").unwrap();
        assert_eq!((a.code.as_str(), a.object), ("1 + 1", true));
        let b = code(&mut s, "_xlfn._xlws.PY(1,0,A1:B3,Sheet2!$C$4)").unwrap();
        assert_eq!(b.code, "df = xl(\"A1:B3\", headers=True)\nxl('Sheet2!$C$4') + %P9% + %P1%");
        assert!(!b.object);
        // The literal form still reads (other applications, older files).
        assert_eq!(code(&mut s, "_xlfn._xlws.PY(\"x = 1\",0)").unwrap().code, "x = 1");
        // A script the part doesn't hold: the cell keeps its value, and is counted for the warning.
        let gone = "_xlfn._xlws.PY(7,0)";
        assert_eq!(cell_code(&mut s, &formula(gone), gone), PyFormula::Missing);
        assert!(s.warning().is_some());
        for not in ["_xlfn._xlws.PY(-1,0)", "_xlfn._xlws.PY(0.5,0)", "_xlfn._xlws.PY(0,2)", "PY(A1,0)", "SUM(0,1)", "PYTHON_TYPE(A1)"] {
            assert_eq!(cell_code(&mut s, &formula(not), not), PyFormula::Not, "{not}");
        }
    }

    #[test]
    fn hostile_scripts_are_bounded() {
        // Many formulas naming one script take from one budget.
        let code = "x".repeat(1_000);
        let mut s = ScriptsIn { scripts: vec![code], budget: 2_500, missing: 0 };
        let f = formula("_xlfn._xlws.PY(0,0)");
        let taken = (0..5).filter(|_| matches!(cell_code(&mut s, &f, ""), PyFormula::Code(_))).count();
        assert_eq!(taken, 2);
        assert_eq!(s.missing, 3);
        // A parameter repeated many times stops growing the code just past a cell's limit.
        let many = "%P2%".repeat(5_000);
        let out = with_params(&many, &[Expr::Name("A_VERY_LONG_NAME_FOR_A_RANGE".into())]);
        assert!((MAX_SCRIPT_CHARS..MAX_SCRIPT_CHARS + 40).contains(&out.chars().count()), "{}", out.len());
        assert!(!PyCode::new(&out).fits());
        // Odd tokens stay as they are.
        assert_eq!(with_params("%P%P2%%P99999999%%P", &[Expr::Number(1.0)]), "%P1%P99999999%%P");
        // A long script is cut just past the limit, so it fails the cell's check.
        let el = El {
            name: "pythonScript".into(),
            children: vec![El { name: "code".into(), text: "y".repeat(20_000), ..El::default() }],
            ..El::default()
        };
        assert_eq!(script_code(&el).chars().count(), MAX_SCRIPT_CHARS);
    }

    #[test]
    fn written_scripts_read_back() {
        let codes = vec!["a = 1\nb = \"two\" & 'three' <tag>".to_string(), "_x0041_ \u{1}".to_string(), String::new()];
        let xml = write(&codes);
        assert!(xml.contains(NS) && xml.contains("<pythonScripts><pythonScript><code>a = 1\nb = \"two\" &amp; 'three' &lt;tag&gt;</code>"), "{xml}");
        let mut read = Vec::new();
        let root = xml::parse_streaming(xml.as_bytes(), &["pythonScript"], &mut |el| {
            read.push(script_code(&el));
            Ok(())
        })
        .unwrap_or_default();
        assert_eq!(root.name, "python");
        assert_eq!(read, codes);
        assert_eq!(file_formula(3, true), "_xlfn._xlws.PY(3,1)");
    }
}
