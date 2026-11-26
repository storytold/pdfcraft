//! Legacy formulas (no dynamic-array metadata) keep one value through GridCraft (issue #410), on
//! a calculated workbook:
//! - every function that returns several values: a legacy cell formula calling it opens with
//!   Excel's implicit `@` and does not spill, and a typed `=@F(…)` saves and reopens with the same
//!   single value. The library's functions are found by calling each with sample arguments, so
//!   one added later is covered without editing this test.
//! - a legacy formula the analysis cannot finish does not spill and saves as read, without `cm`.

use std::io::Write;

use gridcraft_engine::Session;
use gridcraft_engine::core::{CellRef, DateSystem, Value};
use gridcraft_functions::{Arg, Ctx, FnSpec};
use serde_json::json;

struct Probe;

impl Ctx for Probe {
    fn date_system(&self) -> DateSystem {
        DateSystem::D1900
    }
    fn now_serial(&self) -> f64 {
        45_000.0
    }
    fn random(&mut self) -> f64 {
        0.5
    }
}

/// The 2×2 block of numbers at `A1:B2` in the probe sheets.
fn block() -> Value {
    Value::Array(std::sync::Arc::new(
        gridcraft_engine::core::Array::new(2, 2, vec![Value::Number(1.0), Value::Number(2.0), Value::Number(3.0), Value::Number(4.0)]).unwrap(),
    ))
}

/// Sample arguments (formula text, value). Single values only at a position the function lifts
/// over, so an array result is the function's own, not lifting.
fn candidates(scalar: bool) -> Vec<(&'static str, Arg)> {
    let mut out = vec![
        ("3", Arg::val(Value::Number(3.0))),
        ("1", Arg::val(Value::Number(1.0))),
        ("\"a1,b22\"", Arg::val(Value::text("a1,b22"))),
        ("\",\"", Arg::val(Value::text(","))),
        ("\"[0-9]+\"", Arg::val(Value::text("[0-9]+"))),
        ("TRUE", Arg::val(Value::Bool(true))),
    ];
    if !scalar {
        out.insert(0, ("A1:B2", Arg::reference(block())));
        out.insert(1, ("{1,2;3,4}", Arg::val(block())));
    }
    out
}

/// The first sample call of `spec` that returns more than one value, as formula text.
fn array_call(spec: &FnSpec) -> Option<String> {
    let max = spec.max_args.min(spec.min_args.max(1) + 2).min(4).max(spec.min_args);
    for n in spec.min_args..=max {
        let per: Vec<_> = (0..n).map(|i| candidates(spec.is_scalar_arg(i))).collect();
        let per: Vec<_> = if n > 4 { per.into_iter().map(|c| c.into_iter().take(3).collect::<Vec<_>>()).collect() } else { per };
        let mut idx = vec![0usize; n];
        let mut tries = 0;
        loop {
            let args: Vec<Arg> = idx.iter().zip(&per).map(|(&i, c)| c[i].1.clone()).collect();
            if let Value::Array(a) = gridcraft_functions::call(spec, &args, &mut Probe)
                && a.data.len() > 1
            {
                let text: Vec<&str> = idx.iter().zip(&per).map(|(&i, c)| c[i].0).collect();
                return Some(format!("{}({})", spec.name, text.join(",")));
            }
            tries += 1;
            // Next combination, last position fastest.
            let mut k = n;
            loop {
                if k == 0 || tries > 3000 {
                    break;
                }
                k -= 1;
                idx[k] += 1;
                if idx[k] < per[k].len() {
                    break;
                }
                idx[k] = 0;
                if k == 0 {
                    k = usize::MAX;
                    break;
                }
            }
            if n == 0 || k == usize::MAX || tries > 3000 {
                break;
            }
        }
    }
    None
}

/// Every library function with an array result, as a sample call, and the functions the
/// evaluator implements itself that return several values or a range.
fn array_calls() -> Vec<String> {
    let mut calls: Vec<String> = gridcraft_functions::all().iter().filter_map(array_call).collect();
    for found in ["MUNIT", "MODE.MULT", "TEXTSPLIT", "REGEXEXTRACT", "MMULT", "SEQUENCE"] {
        assert!(calls.iter().any(|c| c.starts_with(&format!("{found}("))), "{found} not found: {calls:?}");
    }
    calls.extend(
        ["TRIMRANGE(A1:B2)", "OFFSET(A1:B2,0,0)", "INDIRECT(\"A1:B2\")", "INDEX(A1:B2,0,1)", "FILTERXML(\"<a><b>1</b><b>2</b></a>\",\"//b\")"]
            .map(String::from),
    );
    calls
}

/// Where the `i`th sample call goes: column E, five rows apart, away from the data in `A1:B2`.
fn slot(i: usize) -> CellRef {
    CellRef::new(9 + 5 * i as u32, 4)
}

fn value(s: &Session, c: CellRef) -> Value {
    s.doc().unwrap().wb.active().unwrap().value(c)
}

/// The cells right of and below `c` are empty and `c` is not #SPILL!: the formula did not spill.
fn one_value(s: &Session, c: CellRef, what: &str) -> Result<Value, String> {
    let v = value(s, c);
    let right = value(s, CellRef::new(c.row, c.col + 1));
    let below = value(s, CellRef::new(c.row + 1, c.col));
    if right != Value::Empty || below != Value::Empty || v.display().contains("SPILL") {
        return Err(format!("{what}: {} spilled ({right:?} right, {below:?} below)", v.display()));
    }
    Ok(v)
}

#[test]
fn legacy_formulas_with_array_functions_show_one_value() {
    let calls = array_calls();
    let formulas: Vec<_> = calls.iter().enumerate().map(|(i, f)| (slot(i).a1(), f.clone())).collect();
    let s = open(&legacy_xlsx(&formulas, ""));
    let wrong: Vec<_> = calls.iter().enumerate().filter_map(|(i, f)| one_value(&s, slot(i), f).err()).collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn typed_at_of_array_functions_saves_one_value() {
    let calls = array_calls();
    let mut s = session();
    for (a, v) in [("A1", 1), ("B1", 2), ("A2", 3), ("B2", 4)] {
        s.execute("cell.set", json!({"cell": a, "input": v.to_string()})).unwrap();
    }
    for (i, f) in calls.iter().enumerate() {
        s.execute("cell.set", json!({"cell": slot(i).a1(), "input": format!("=@{f}")})).unwrap();
    }
    let before: Vec<_> = (0..calls.len()).map(|i| value(&s, slot(i))).collect();
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let mut back = Session::new();
    back.execute("file.open", json!({"name": "typed.xlsx", "base64": saved["base64"]})).unwrap();
    let wrong: Vec<_> = calls
        .iter()
        .enumerate()
        .filter_map(|(i, f)| match one_value(&back, slot(i), f) {
            Ok(v) if v == before[i] => None,
            // A volatile function recalculates on open: one number again.
            Ok(Value::Number(_)) if volatile(f) => None,
            Ok(v) => Some(format!("@{f}: {} before saving, {} after", before[i].display(), v.display())),
            Err(e) => Some(e),
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

fn volatile(call: &str) -> bool {
    call.split('(').next().and_then(gridcraft_functions::lookup).is_some_and(|spec| spec.volatile)
}

/// Formulas the analysis covers at Excel's nesting limits, and one it cannot finish (names too
/// large to analyse, whose ranges would spill three rows): calculated, none spills, and saving
/// writes each as read, as a plain `<f>`.
#[test]
fn deep_and_unfinished_legacy_formulas_calculate_and_save_as_read() {
    let nested = format!("{}A1:A3{}", "ABS(".repeat(200), ")".repeat(200));
    let parens = format!("{}A1:A3{}", "ABS((".repeat(64), "))".repeat(64));
    let parts = (1..=30).map(|i| format!("Part_{i}")).collect::<Vec<_>>().join("+");
    let part = format!("Sheet1!$A$1:$A$3+{}", vec!["Sheet1!$B$1"; 599].join("+"));
    let names: String = (1..=30).map(|i| format!(r#"<definedName name="Part_{i}">{part}</definedName>"#)).collect();
    let formulas = [("E10".to_string(), nested), ("E15".into(), parens), ("E20".into(), parts)];
    let mut s = open(&legacy_xlsx(&formulas, &names));
    for (a, _) in &formulas {
        one_value(&s, CellRef::parse(a).unwrap(), a).unwrap();
    }
    // Legacy Excel intersects A1:A3 with row 10 and 15: #VALUE!. The unfinished formula shows the
    // top-left value of its sum, 1 + 599 × 2 per name.
    assert_eq!(value(&s, CellRef::parse("E10").unwrap()).display(), "#VALUE!");
    assert_eq!(value(&s, CellRef::parse("E15").unwrap()).display(), "#VALUE!");
    assert_eq!(value(&s, CellRef::parse("E20").unwrap()), Value::Number(30.0 * (1.0 + 599.0 * 2.0)));
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let bytes = gridcraft_engine::io::base64_decode(saved["base64"].as_str().unwrap()).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(), &mut xml).unwrap();
    for (a, f) in &formulas {
        assert!(xml.contains(&format!("<f>{f}</f>")), "{a} not saved as read: {xml}");
    }
    assert!(!xml.contains("cm=") && !xml.contains("SINGLE") && !xml.contains("t=\"array\""), "{xml}");
}

/// Legacy unions where one value is expected, a name whose definition is IF of a range, and
/// multi-valued results in the file's own parentheses: calculated, none spills (legacy Excel
/// intersects a union to #VALUE!), and saving writes each as read, as a plain `<f>`.
#[test]
fn legacy_unions_names_and_parentheses_calculate_and_save_as_read() {
    let formulas: Vec<(String, String)> = ["SQRT((A1:A2,B1:B2))", "ABS(((A1:A2,B1:B2)))", "(A1:A2,B1:B2)+1", "SQRT(Flag)", "({1,2}+1)", "{1,2}+1"]
        .iter()
        .enumerate()
        .map(|(i, f)| (slot(i).a1(), f.to_string()))
        .collect();
    let names = r#"<definedName name="Flag">IF(Sheet1!$A$1:$A$3&gt;1,1,0)</definedName>"#;
    let mut s = open(&legacy_xlsx(&formulas, names));
    for (a, _) in &formulas {
        one_value(&s, CellRef::parse(a).unwrap(), a).unwrap();
    }
    let shown: Vec<String> = formulas.iter().map(|(a, _)| value(&s, CellRef::parse(a).unwrap()).display()).collect();
    assert_eq!(shown, ["#VALUE!", "#VALUE!", "#VALUE!", "0", "2", "2"]);
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let bytes = gridcraft_engine::io::base64_decode(saved["base64"].as_str().unwrap()).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(), &mut xml).unwrap();
    for (a, f) in &formulas {
        assert!(xml.contains(&format!("<f>{f}</f>")), "{a} not saved as read: {xml}");
    }
    assert!(!xml.contains("cm=") && !xml.contains("SINGLE") && !xml.contains("t=\"array\""), "{xml}");
}

/// A whole column in a scalar argument of a legacy formula meets the formula's row (issue #1636,
/// from PR #1660): `ISBLANK(B:B)` in row 4 tests `B4`, so the formula shows `C4` instead of
/// spilling into `G5`, and in row 6, where `B6` is blank, shows 0.
#[test]
fn whole_column_in_a_legacy_scalar_argument_meets_the_formulas_row() {
    let formulas: Vec<(String, String)> = [
        ("B4", r#""Thierry""#),
        ("C4", "7"),
        ("G4", r#"IF(ISBLANK(B:B),0,IF(B4="Thierry",C4,0))"#),
        ("G5", "1"),
        ("G6", r#"IF(ISBLANK(B:B),0,IF(B6="Thierry",C6,0))"#),
    ]
    .iter()
    .map(|(a, f)| (a.to_string(), f.to_string()))
    .collect();
    let s = open(&legacy_xlsx(&formulas, ""));
    let at = |a: &str| CellRef::parse(a).unwrap();
    assert_eq!(value(&s, at("G4")), Value::Number(7.0), "B:B meets row 4 instead of spilling");
    assert_eq!(value(&s, at("G5")), Value::Number(1.0), "nothing spills below G4");
    assert_eq!(one_value(&s, at("G6"), "G6").unwrap(), Value::Number(0.0), "B6 is blank");
}

/// One-cell ranges written as ranges (`B1:B1`) outside any `@`, and formulas whose reading
/// reaches the parser's nesting limit (254 nested functions, far past Excel's 64): saved as read,
/// without `@`. Only a one-cell range under an `@` counts as one an edit may have shrunk. All but
/// the last are calculated without spilling and saved as a plain `<f>`.
#[test]
fn one_cell_ranges_and_formulas_at_the_nesting_limit_save_as_read() {
    let deep = |inner: &str| format!("{}{inner}{}", "ABS(".repeat(254), ")".repeat(254));
    let formulas: Vec<(String, String)> = [
        "SQRT(A1:A3)+B1:B1".to_string(),
        "A1:A3*B2:B2".into(),
        "SQRT(A1:A3)+$B$2:$B$2".into(),
        // Names with a one-cell range inside CHOOSE's index or IF's condition.
        "SQRT(NC)".into(),
        "SQRT(NI)".into(),
        "NC+1".into(),
        "NI*2".into(),
        deep("A1:A3%"),
        deep("-A1:A3"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, f)| (slot(i).a1(), f))
    .collect();
    let names = r#"<definedName name="NC">CHOOSE(Sheet1!$B$1:$B$1,Sheet1!$A$1:$A$3,Sheet1!$A$1:$A$3)</definedName><definedName name="NI">IF(Sheet1!$B$1:$B$1&gt;0,Sheet1!$A$1:$A$3)</definedName>"#;
    let (last, rest) = formulas.split_last().unwrap();
    let mut s = open(&legacy_xlsx(&formulas, names));
    for (a, _) in rest {
        one_value(&s, CellRef::parse(a).unwrap(), a).unwrap();
    }
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let bytes = gridcraft_engine::io::base64_decode(saved["base64"].as_str().unwrap()).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(), &mut xml).unwrap();
    for (a, f) in rest {
        assert!(xml.contains(&format!("<f>{f}</f>")), "{a} not saved as read: {xml}");
    }
    // A reading the parser cannot nest leaves the formula as read: it calculates as a
    // dynamic-array formula would, but the file never gets an `@`.
    let (a, f) = last;
    assert!(xml.contains(&format!("{f}</f>")), "{a} not saved as read: {xml}");
    assert!(!xml.contains('@') && !xml.contains("SINGLE"), "{xml}");
    assert_eq!(xml.matches("cm=").count(), 1, "{xml}");
}

/// Deleting rows shrinks the ranges names return (directly, through another name, CHOOSE's and
/// IF's results, parentheses) to one cell: the `@` the reader put on each name is still the one
/// legacy Excel implies, so the formulas save as legacy ones, without `_xlfn.SINGLE`.
#[test]
fn names_shrunk_by_deleting_rows_save_as_legacy() {
    let formulas: Vec<(String, String)> =
        ["SQRT(MM)", "SQRT(CHM)", "SQRT(NCH)", "SQRT(NIF)", "SQRT(PM)"].into_iter().enumerate().map(|(i, f)| (slot(i).a1(), f.to_string())).collect();
    let names = r#"<definedName name="MM">Sheet1!$A$1:$A$3</definedName><definedName name="CHM">MM</definedName><definedName name="NCH">CHOOSE(1,Sheet1!$A$1:$A$3,Sheet1!$B$1:$B$3)</definedName><definedName name="NIF">IF(1,Sheet1!$A$1:$A$3,Sheet1!$B$1:$B$3)</definedName><definedName name="PM">(Sheet1!$A$1:$A$3)</definedName>"#;
    let mut s = open(&legacy_xlsx(&formulas, names));
    s.execute("home.deleteRows", json!({"rows": "2:3"})).unwrap();
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let bytes = gridcraft_engine::io::base64_decode(saved["base64"].as_str().unwrap()).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(), &mut xml).unwrap();
    for (a, f) in &formulas {
        assert!(xml.contains(&format!("<f>{f}</f>")), "{a} not saved as legacy {f}: {xml}");
    }
    assert!(!xml.contains("SINGLE") && !xml.contains("cm="), "{xml}");
}

const CT: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;

/// A one-sheet package: numbers 1–4 in `A1:B2` and `formulas` (address, file text) saved as
/// legacy formulas, without dynamic-array metadata; `names` is `<definedName>` elements.
fn legacy_xlsx(formulas: &[(String, String)], names: &str) -> Vec<u8> {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut rows =
        String::from(r#"<row r="1"><c r="A1"><v>1</v></c><c r="B1"><v>2</v></c></row><row r="2"><c r="A2"><v>3</v></c><c r="B2"><v>4</v></c></row>"#);
    // Cells in one row share its `<row>`, in the order given.
    let mut by_row: Vec<(String, String)> = Vec::new();
    for (a, f) in formulas {
        let r: String = a.chars().filter(char::is_ascii_digit).collect();
        let c = format!(r#"<c r="{a}"><f>{}</f></c>"#, esc(f));
        match by_row.iter_mut().find(|(row, _)| *row == r) {
            Some((_, cells)) => cells.push_str(&c),
            None => by_row.push((r, c)),
        }
    }
    for (r, cells) in by_row {
        rows.push_str(&format!(r#"<row r="{r}">{cells}</row>"#));
    }
    let names = if names.is_empty() { String::new() } else { format!("<definedNames>{names}</definedNames>") };
    let parts: [(&str, String); 5] = [
        ("[Content_Types].xml", CT.into()),
        ("_rels/.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into()),
        ("xl/workbook.xml", format!(r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>{names}</workbook>"#)),
        ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.into()),
        ("xl/worksheets/sheet1.xml", format!(r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#)),
    ];
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in parts {
        z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(data.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn open(bytes: &[u8]) -> Session {
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "legacy.xlsx", "base64": gridcraft_engine::io::base64_encode(bytes)})).unwrap();
    s
}
