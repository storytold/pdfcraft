use super::*;

/// Sheet1 and Sheet2 with a workbook range name, a name scoped to each sheet, a constant, a
/// formula and a hidden name.
fn go_to_workbook() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("home.insertSheet", json!({"name": "Sheet2"})).unwrap();
    s.execute("selection.set", json!({"sheet": 0})).unwrap();
    let define = |s: &mut Session, name: &str, refers: &str, scope: &str| {
        s.execute("formulas.defineName", json!({"name": name, "refersTo": refers, "scope": scope})).unwrap();
    };
    define(&mut s, "Price", "=Sheet1!$B$2:$B$5", "Workbook");
    define(&mut s, "type", "=Sheet1!$D$2:$D$5", "Workbook");
    define(&mut s, "Areas", "=Sheet1!$A$1,Sheet1!$C$3:$C$4", "Workbook");
    define(&mut s, "Local", "=Sheet2!$C$3", "Sheet2");
    define(&mut s, "Own", "=Sheet1!$E$1", "Sheet1");
    define(&mut s, "Rate", "=0.07", "Workbook");
    define(&mut s, "Total", "=SUM(Sheet1!$B$2:$B$5)", "Workbook");
    let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
    wb.names.push(gridcraft_model::DefinedName {
        name: "Secret".into(),
        scope: None,
        formula: "Sheet1!$Z$9".into(),
        comment: String::new(),
        hidden: true,
    });
    s
}

/// "The Go to popup window shows named ranges on every worksheet in your workbook." Names
/// that are not ranges, and hidden names, are not listed.
#[test]
fn go_to_lists_the_named_ranges_of_every_sheet() {
    let mut s = go_to_workbook();
    let list = s.execute("edit.goTo", json!({"list": true})).unwrap();
    assert_eq!(list["names"], json!(["Areas", "Own", "Price", "Sheet2!Local", "type"]), "{list}");
    assert_eq!(list["recent"], json!([]), "{list}");
    // Seen from Sheet2, its own name is bare and Sheet1's is qualified.
    s.execute("selection.set", json!({"sheet": 1})).unwrap();
    let list = s.execute("edit.goTo", json!({"list": true})).unwrap();
    assert_eq!(list["names"], json!(["Areas", "Local", "Price", "Sheet1!Own", "type"]), "{list}");
}

/// Thousands of names that alias other names are listed with one lookup per alias, not a scan of
/// every name for each; aliases resolve through the chain like a single name does.
#[test]
fn go_to_lists_many_aliases_and_follows_them() {
    let mut s = go_to_workbook();
    let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
    let alias = |name: String, formula: String| gridcraft_model::DefinedName { name, scope: None, formula, comment: String::new(), hidden: false };
    wb.names.push(alias("Chain0".into(), "Areas".into()));
    for i in 1..20_000 {
        wb.names.push(alias(format!("Chain{i}"), format!("Chain{},price", i - 1)));
    }
    let list = s.execute("edit.goTo", json!({"list": true})).unwrap();
    let names: Vec<&str> = list["names"].as_array().unwrap().iter().filter_map(|n| n.as_str()).collect();
    // Chain0..Chain6 are within the name depth Go To follows; longer chains are not ranges.
    for i in 0..7 {
        assert!(names.contains(&format!("Chain{i}").as_str()), "Chain{i} in {names:?}");
    }
    assert!(!names.contains(&"Chain7"), "{names:?}");
    let out = s.execute("edit.goTo", json!({"reference": "chain1"})).unwrap();
    assert_eq!(out["selection"], "A1,C3:C4,B2:B5", "{out}");
}

#[test]
fn go_to_a_name_scoped_to_another_sheet_goes_to_that_sheet() {
    let mut s = go_to_workbook();
    let out = s.execute("edit.goTo", json!({"reference": "Sheet2!Local"})).unwrap();
    assert_eq!(out["selection"], "C3", "{out}");
    assert_eq!(s.doc().unwrap().wb.active_sheet(), 1);
    // A name of several areas selects all of them.
    let out = s.execute("edit.goTo", json!({"reference": "Areas"})).unwrap();
    assert_eq!(out["selection"], "A1,C3:C4", "{out}");
    assert_eq!(s.doc().unwrap().wb.active_sheet(), 0);
}

/// A name matches in any case and any script, as a formula matches it.
#[test]
fn go_to_matches_a_sheet_name_in_any_case() {
    let mut s = go_to_workbook();
    s.execute("formulas.defineName", json!({"name": "Größe", "refersTo": "=Sheet2!$D$4", "scope": "Sheet2"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "GRÖSSE", "refersTo": "=Sheet1!$F$6", "scope": "Workbook"})).unwrap();
    let out = s.execute("edit.goTo", json!({"reference": "Sheet2!GRÖSSE"})).unwrap();
    assert_eq!(out["selection"], "D4", "{out}");
    // Seen from Sheet2, its own name hides the workbook name it shares.
    let list = s.execute("edit.goTo", json!({"list": true})).unwrap();
    assert_eq!(list["names"], json!(["Areas", "Größe", "Local", "Price", "Sheet1!Own", "type"]), "{list}");
}

/// "You can enter multiple named ranges or cell references in the Reference box. Separating
/// each with a comma ... Excel will highlight all the ranges."
#[test]
fn go_to_selects_every_reference_and_name_in_a_list() {
    let mut s = go_to_workbook();
    let out = s.execute("edit.goTo", json!({"reference": "B14:C22,F19:G30,H21:H29"})).unwrap();
    assert_eq!(out["selection"], "B14:C22,F19:G30,H21:H29", "{out}");
    let out = s.execute("edit.goTo", json!({"reference": "Price, Type"})).unwrap();
    assert_eq!(out["selection"], "B2:B5,D2:D5", "{out}");
    let out = s.execute("edit.goTo", json!({"reference": "Sheet2!$A$1,Sheet2!B2"})).unwrap();
    assert_eq!(out["selection"], "A1,B2", "{out}");
    assert_eq!(s.doc().unwrap().wb.active_sheet(), 1);
    // Areas on two sheets, and a part that is not a reference, leave the selection alone.
    for bad in ["A1,Sheet1!B2", "A1,Rate", "A1,", "Total"] {
        assert!(s.execute("edit.goTo", json!({"reference": bad})).is_err(), "{bad}");
        assert_eq!(s.doc().unwrap().selection.a1(), "A1,B2", "{bad}");
    }
}

#[test]
fn go_to_reads_the_parts_of_a_long_reference_one_at_a_time() {
    let mut parts = reference_parts("A1, 'Q1, Q2'!B2,SUM(C3,D4)");
    assert_eq!(parts.next(), Some("A1"));
    assert_eq!(parts.next(), Some("'Q1, Q2'!B2"));
    assert_eq!(parts.next(), Some("SUM(C3,D4)"));
    assert_eq!(parts.next(), None);
    assert_eq!(reference_parts("").collect::<Vec<_>>(), vec![""]);
    let mut s = go_to_workbook();
    let err = s.execute("edit.goTo", json!({"reference": "A1,".repeat(GO_TO_MAX_AREAS + 1)})).unwrap_err();
    assert!(err.to_string().contains("at most"), "{err}");
    let bad = format!("Total{}", ",".repeat(1_000_000));
    assert!(s.execute("edit.goTo", json!({"reference": bad})).is_err());
}

/// "The Go to box keeps track of ranges as you enter them, and you can return to any of them."
#[test]
fn go_to_keeps_the_references_entered_newest_first() {
    let mut s = go_to_workbook();
    s.execute("edit.goTo", json!({"reference": "B14:C22,F19:G30", "record": true})).unwrap();
    s.execute("edit.goTo", json!({"reference": "Sheet2!A5", "record": true})).unwrap();
    // Names are listed already; a call without `record` (the Name Box) is not kept.
    s.execute("edit.goTo", json!({"reference": "Price", "record": true})).unwrap();
    s.execute("edit.goTo", json!({"reference": "Z9"})).unwrap();
    s.execute("edit.goTo", json!({"reference": "b14:c22,f19:g30", "record": true})).unwrap();
    let list = s.execute("edit.goTo", json!({"list": true})).unwrap();
    assert_eq!(list["recent"], json!(["Sheet1!B14:C22,Sheet1!F19:G30", "Sheet2!A5"]), "{list}");
    // Going back from Sheet2 to a recent reference returns to its sheet.
    s.execute("selection.set", json!({"sheet": 1})).unwrap();
    let out = s.execute("edit.goTo", json!({"reference": "Sheet1!B14:C22,Sheet1!F19:G30", "record": true})).unwrap();
    assert_eq!(out["selection"], "B14:C22,F19:G30", "{out}");
    assert_eq!(s.doc().unwrap().wb.active_sheet(), 0);
    for i in 0..GO_TO_RECENT + 2 {
        s.execute("edit.goTo", json!({"reference": format!("A{}", i + 1), "record": true})).unwrap();
    }
    let list = s.execute("edit.goTo", json!({"list": true})).unwrap();
    assert_eq!(list["recent"].as_array().map(Vec::len), Some(GO_TO_RECENT), "{list}");
    assert_eq!(list["recent"][0], format!("Sheet1!A{}", GO_TO_RECENT + 2), "{list}");
}

#[test]
fn cell_set_takes_numbers_for_input_and_value() {
    // #697: a JSON number for `input` used to clear the cell instead of entering it.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": 123.5})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "value": 77})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": true})).unwrap();
    s.execute("cell.set", json!({"cell": "A4", "input": "=A1+A2"})).unwrap();
    let get = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap();
    assert_eq!(get(&mut s, "A1")["value"], json!(123.5));
    assert_eq!(get(&mut s, "A2")["value"], json!(77.0));
    assert_eq!(get(&mut s, "A3")["value"], json!(true));
    assert_eq!(get(&mut s, "A4")["value"], json!(200.5));
    assert!(s.execute("cell.set", json!({"cell": "A5"})).is_err());
}

/// Every string in `values` went through typed input, read with en-US rules, in silence: a
/// caller sending "05.04.2026" from a dd.mm.yyyy export stored 4 May, "2.500" became 2.5 and
/// "4.25 %" became a percentage (#2255). `parse: false` stores a string as the text it is.
#[test]
fn set_values_can_store_strings_as_the_text_they_are() {
    let mut s = Session::new();
    s.new_workbook();
    let strings = ["05.04.2026", "02/11/2026", "2.500", "4.25 %", "=1+1", "'keep"];
    let row: Vec<Json> = strings.iter().map(|v| json!(v)).collect();
    let get = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap();

    // The default is unchanged: the dates swap, the thousands separator is a decimal point.
    s.execute("range.setValues", json!({"range": "A1", "values": [row.clone()]})).unwrap();
    assert_eq!(get(&mut s, "A1")["text"], "5/4/2026", "dd.mm.yyyy read as mm.dd.yyyy");
    assert_eq!(get(&mut s, "C1")["value"], json!(2.5), "\"2.500\" read as a decimal");
    assert_eq!(get(&mut s, "D1")["value"], json!(0.0425), "\"4.25 %\" read as a percentage");

    // With `parse: false` each string is the text it was given, and a formula is still a
    // formula.
    s.execute("range.setValues", json!({"range": "A3", "parse": false, "values": [row.clone()]})).unwrap();
    for (i, want) in strings.iter().enumerate() {
        let cell = format!("{}3", (b'A' + i as u8) as char);
        let got = get(&mut s, &cell);
        if *want == "=1+1" {
            assert_eq!(got["value"], json!(2.0), "{cell} is still a formula: {got}");
            assert_eq!(got["formula"], "=1+1", "{got}");
        } else {
            assert_eq!(got["value"], json!(*want), "{cell} is the text it was given: {got}");
            assert!(got["formula"].is_null(), "{cell} is not a formula: {got}");
        }
    }
    // Numbers and booleans are untouched either way.
    s.execute("range.setValues", json!({"range": "A5", "parse": false, "values": [[12.5, true, null]]})).unwrap();
    assert_eq!(get(&mut s, "A5")["value"], json!(12.5));
    assert_eq!(get(&mut s, "B5")["value"], json!(true));
    assert_eq!(get(&mut s, "C5")["text"], "");
    // `parse` is a flag, and anything else is a usage error rather than a quiet default.
    s.execute("range.setValues", json!({"range": "A7", "parse": true, "values": [["2.500"]]})).unwrap();
    assert_eq!(get(&mut s, "A7")["value"], json!(2.5));
    assert!(s.execute("range.setValues", json!({"range": "A8", "parse": "no", "values": [["x"]]})).is_err());
    assert!(s.execute("range.setValues", json!({"range": "A8", "parse": 0, "values": [["x"]]})).is_err());
    // A line break still turns on Wrap Text.
    s.execute("range.setValues", json!({"range": "A9", "parse": false, "values": [["one\ntwo"]]})).unwrap();
    assert_eq!(get(&mut s, "A9")["value"], json!("one\ntwo"));
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    assert!(d.wb.styles.get(sh.style_id(CellRef::parse("A9").unwrap())).align.wrap);
}

/// The result names the strings typed input read as something else, so a script notices without
/// reading the file back (#2255). A formula and a leading apostrophe are not conversions.
#[test]
fn set_values_reports_the_strings_typed_input_read() {
    let mut s = Session::new();
    s.new_workbook();
    let row = json!(["05.04.2026", "02/11/2026", "2.500", "4.25 %", "=1+1", "'keep"]);
    let r = s.execute("range.setValues", json!({"range": "A1", "values": [row.clone()]})).unwrap();
    assert_eq!(r["cells"], 6, "{r}");
    assert_eq!(r["convertedCount"], 4, "{r}");
    assert_eq!(r["converted"][0], json!({"cell": "A1", "input": "05.04.2026", "stored": "5/4/2026"}), "{r}");
    assert_eq!(r["converted"][1], json!({"cell": "B1", "input": "02/11/2026", "stored": "2/11/2026"}), "{r}");
    assert_eq!(r["converted"][2], json!({"cell": "C1", "input": "2.500", "stored": "2.5"}), "{r}");
    assert_eq!(r["converted"][3], json!({"cell": "D1", "input": "4.25 %", "stored": "4.25%"}), "{r}");
    // The report is a diagnostic, not a second copy of the data: a long input is cut.
    let long = s.execute("range.setValues", json!({"range": "J1", "values": [["9".repeat(100)]]})).unwrap();
    assert_eq!(long["convertedCount"], 1, "{long}");
    let input = long["converted"][0]["input"].as_str().unwrap_or_default();
    assert_eq!(input.chars().count(), 33, "32 characters and the ellipsis: {input}");
    assert!(input.ends_with('…'), "{input}");
    // Nothing is read with `parse: false`, so nothing is reported.
    let verbatim = s.execute("range.setValues", json!({"range": "A3", "parse": false, "values": [row]})).unwrap();
    assert_eq!(verbatim["convertedCount"], 0, "{verbatim}");
    assert_eq!(verbatim["converted"], json!([]), "{verbatim}");
}

/// An empty string clears the cell with `parse: false` too, as `null` and the default path do: a
/// script clearing cells with "" must not leave cells `COUNTA` counts.
#[test]
fn set_values_parse_false_clears_an_empty_string() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1:B1", "values": [["keep", "keep"]]})).unwrap();
    s.execute("home.bold", json!({"range": "A1"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "parse": false, "values": [["", ""]]})).unwrap();
    let get = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap();
    assert_eq!(get(&mut s, "A1")["type"], "empty");
    assert_eq!(get(&mut s, "B1")["type"], "empty");
    let d = s.doc().unwrap();
    let sh = d.wb.sheet(0).unwrap();
    assert!(d.wb.styles.get(sh.style_id(CellRef::parse("A1").unwrap())).font.bold, "clearing with \"\" keeps the formatting");
    assert_eq!(s.execute("formulas.evaluate", json!({"formula": "=COUNTA(A1:B1)"})).unwrap(), json!(0.0));
}

/// The text `parse: false` stores is still text after the workbook is saved and reopened, and the
/// default's reading of a date survives as a date (#2255).
#[test]
fn set_values_text_survives_an_xlsx_roundtrip() {
    let mut s = Session::new();
    s.new_workbook();
    let values = json!([["05.04.2026", "2.500", "4.25 %", "=1+1", 3, true]]);
    s.execute("range.setValues", json!({"range": "A1", "parse": false, "values": values})).unwrap();
    s.execute("range.setValues", json!({"range": "A3", "values": [["05.04.2026"]]})).unwrap();
    let bytes = crate::io::save_bytes(&s.doc().unwrap().wb, "book.xlsx").unwrap();
    let (wb, _) = crate::io::open_bytes("book.xlsx", &bytes).unwrap();
    let sh = wb.sheet(0).unwrap();
    for (col, want) in [(0, "05.04.2026"), (1, "2.500"), (2, "4.25 %")] {
        assert_eq!(sh.value(CellRef::new(0, col)), Value::text(want), "column {col} is the text it was given");
    }
    assert_eq!(sh.value(CellRef::new(0, 3)), Value::number(2.0), "=1+1 is still a formula");
    assert_eq!(sh.value(CellRef::new(0, 4)), Value::number(3.0));
    assert_eq!(sh.value(CellRef::new(0, 5)), Value::Bool(true));
    // The default read the date, and the file keeps it as one.
    let at = CellRef::new(2, 0);
    assert_eq!(crate::display::cell_text(&wb, sh, at), "5/4/2026");
    assert_eq!(wb.styles.get(sh.style_id(at)).num_fmt.as_str(), "m/d/yyyy");
}

#[test]
fn legacy_array_entry() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "B2", "values": [[1], [2]]})).unwrap();
    s.execute("cell.set", json!({"cell": "C4", "input": "old"})).unwrap();
    s.execute("selection.set", json!({"range": "C2:C4"})).unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "=B2:B3*2", "array": true})).unwrap();
    let get = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap();
    assert_eq!(get(&mut s, "C3")["value"], json!(4.0));
    // Past the result Excel shows #N/A; the old content of the range is replaced.
    assert_eq!(get(&mut s, "C4")["text"], "#N/A");
    let bar = s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse("C2").unwrap()).map(|c| c.bar_text());
    assert_eq!(bar.as_deref(), Some("{=B2:B3*2}"));
}

#[test]
fn an_array_entered_outside_the_selection_covers_only_its_cell() {
    // Found by the command-sequence fuzzer: the array took the selection's range even with
    // `cell` elsewhere (or on another sheet), so its formula sat outside its own range and the
    // selected cells were wiped.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "K20", "input": "keep"})).unwrap();
    s.execute("selection.set", json!({"range": "K20:K22"})).unwrap();
    s.execute("cell.set", json!({"cell": "H5", "input": "=1+1", "array": true})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    let f = sh.cell(CellRef::parse("H5").unwrap()).and_then(|c| c.formula.clone()).unwrap();
    assert_eq!(f.array, Some(RangeRef::parse("H5").unwrap()));
    assert_eq!(sh.value(CellRef::parse("K20").unwrap()), Value::text("keep"));
}

#[test]
fn wildcards() {
    assert!(wildcard("hello", "h*o"));
    assert!(wildcard("hello", "h?llo"));
    assert!(!wildcard("hello", "h?lo"));
    assert!(wildcard("a*b", "a~*b"));
    assert!(!wildcard("axb", "a~*b"));
    assert!(wildcard("", "*"));
}

/// The backtracking matcher `wildcard` replaced, as the reference for its results.
fn wildcard_backtracking(text: &str, pat: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let mut p: Vec<(char, bool)> = Vec::new();
    let mut it = pat.chars();
    while let Some(c) = it.next() {
        if c == '~' {
            if let Some(n) = it.next() {
                p.push((n, true));
            }
        } else {
            p.push((c, false));
        }
    }
    let (mut ti, mut pi) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == ('?', false) || (p[pi].0 == t[ti] && p[pi] != ('*', false))) {
            ti += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == ('*', false) {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == ('*', false) {
        pi += 1;
    }
    pi == p.len()
}

#[test]
fn wildcard_agrees_with_backtracking() {
    // Every text up to 5 characters over a small alphabet against every pattern up to 4 items.
    let alphabet = ['a', 'b', '*', '?', '~'];
    let all = |n: usize, chars: &[char]| -> Vec<String> {
        let mut out = vec![String::new()];
        let mut level = vec![String::new()];
        for _ in 0..n {
            level = level.iter().flat_map(|s| chars.iter().map(move |c| format!("{s}{c}"))).collect();
            out.extend(level.iter().cloned());
        }
        out
    };
    let texts = all(5, &['a', 'b', '*']);
    for pat in all(4, &alphabet) {
        for text in &texts {
            assert_eq!(wildcard(text, &pat), wildcard_backtracking(text, &pat), "{text:?} ~ {pat:?}");
        }
    }
    // Patterns longer than one 64-bit word.
    let long = format!("*{}*b?", "a".repeat(70));
    for text in [format!("x{}yb1", "a".repeat(70)), format!("{}b", "a".repeat(71)), "a".repeat(200)] {
        assert_eq!(wildcard(&text, &long), wildcard_backtracking(&text, &long), "{text}");
    }
}

#[test]
fn wildcard_with_many_distinct_literals_agrees_with_backtracking() {
    // 600 distinct characters in the pattern: more than the masks kept, so the rest are rebuilt.
    let chars: Vec<char> = (0x4E00u32..0x4E00 + 600).filter_map(char::from_u32).collect();
    let word: String = chars.iter().collect();
    let pat = format!("*{word}?{}", chars[3]);
    for text in [format!("x{word}yq{}", chars[3]), word.clone(), format!("{word}{word}"), chars.iter().rev().collect::<String>()] {
        assert_eq!(wildcard(&text, &pat), wildcard_backtracking(&text, &pat), "{} chars", text.chars().count());
    }
    assert!(wildcard(&format!("{word}zz{}", chars[3]).replace("zz", "z"), &pat));
}

#[test]
fn wildcard_keeps_masks_of_frequent_literals_past_the_cache_limit() {
    // 300 distinct characters come before the repeated `a`: past the cached masks, `a` (5,000 positions)
    // still has to be read from a kept mask, or it is rebuilt for each of the 20,000 `a` in the text.
    let rare: String = (0x4E00u32..0x4E00 + 300).filter_map(char::from_u32).collect();
    let pat = format!("*{rare}{}b", "a".repeat(5_000));
    let text = "a".repeat(20_000);
    assert!(!wildcard(&text, &pat));
    assert!(wildcard(&format!("{rare}{}b", "a".repeat(5_000)), &pat));
    assert!(!wildcard(&format!("{rare}{}", "a".repeat(5_000)), &pat));
}

#[test]
fn wildcard_on_long_repetitive_text_is_linear() {
    // The backtracking matcher compared about text × pattern characters here (10^8).
    let text = "a".repeat(20_000);
    let pat = format!("*{}b", "a".repeat(5_000));
    assert!(!wildcard(&text, &pat));
    assert!(wildcard(&format!("{text}b"), &pat));
}

#[test]
fn tsv() {
    assert_eq!(parse_tsv("a\tb\n1\t2\n"), vec![vec!["a", "b"], vec!["1", "2"]]);
    assert_eq!(parse_tsv("\"x\ty\"\tz"), vec![vec!["x\ty", "z"]]);
    assert_eq!(parse_tsv("\"he said \"\"hi\"\"\""), vec![vec!["he said \"hi\""]]);
    // #2225: a quote that doesn't close the field leaves it as written, as Excel pastes it.
    assert_eq!(parse_tsv("\"hi\" there\tx\n"), vec![vec!["\"hi\" there", "x"]]);
    assert_eq!(parse_tsv("say \"hi\"\n"), vec![vec!["say \"hi\""]]);
}

#[test]
fn paste_special_operations_keep_text_errors_and_formulas() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [["'123", "Total", "=NA()", "=B3*2", 5], [1, null, null, null, null], [null, 4, null, null, null]]}),
    )
    .unwrap();
    let get = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap();
    assert_eq!(get(&mut s, "A1")["value"], "123");
    s.execute("edit.copy", json!({"range": "A2"})).unwrap();
    s.execute("selection.set", json!({"range": "A1:E1"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "values", "operation": "multiply"})).unwrap();
    assert_eq!(get(&mut s, "A1")["value"], json!(123.0));
    assert_eq!(get(&mut s, "B1")["value"], "Total");
    assert_eq!(get(&mut s, "C1")["value"]["error"], "#N/A");
    assert_eq!(get(&mut s, "D1")["formula"], "=(B3*2)*1");
    assert_eq!(get(&mut s, "D1")["value"], json!(8.0));
    assert_eq!(get(&mut s, "E1")["value"], json!(5.0));
}

#[test]
fn cut_paste_that_overflows_the_grid_moves_nothing() {
    // #997: the source was emptied first and the cells that fell off the grid were skipped.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2]]})).unwrap();
    let get = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap();
    s.execute("edit.cut", json!({"range": "A1:A2"})).unwrap();
    // Transposed, the 2 rows become 2 columns and the second one is past XFD.
    assert!(s.execute("edit.pasteSpecial", json!({"what": "all", "transpose": true, "at": "XFD1"})).is_err());
    assert_eq!(get(&mut s, "A1")["value"], json!(1.0));
    assert_eq!(get(&mut s, "A2")["value"], json!(2.0));
    // Not transposed, the second row would be past the last row.
    assert!(s.execute("edit.pasteSpecial", json!({"what": "all", "at": "A1048576"})).is_err());
    assert_eq!(get(&mut s, "A1")["value"], json!(1.0));
    assert_eq!(get(&mut s, "A2")["value"], json!(2.0));
    // Not transposed it still fits in column XFD.
    s.execute("edit.pasteSpecial", json!({"what": "all", "at": "XFD1"})).unwrap();
    assert_eq!(get(&mut s, "XFD1")["value"], json!(1.0));
    assert_eq!(get(&mut s, "XFD2")["value"], json!(2.0));
}

#[test]
fn drag_and_drop_asks_before_replacing_data() {
    // #3136: dragging column B's border onto column A replaced A without asking.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1:B3", "values": [["Apple", "Kiwi"], ["Pear", "Plum"], ["Fig", "Lime"]]})).unwrap();
    s.execute("edit.copy", json!({"range": "D9"})).unwrap();
    let text = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap()["input"].clone();
    let r = s.execute("range.move", json!({"range": "B:B", "to": "A1"}));
    let Err(EngineError::Confirm(q)) = r else { panic!("{r:?}") };
    assert_eq!(q, "There's already data here. Do you want to replace it?");
    // Nothing moved, and the copy made before is still on the clipboard.
    assert_eq!(text(&mut s, "A1"), json!("Apple"));
    assert_eq!(text(&mut s, "B1"), json!("Kiwi"));
    assert!(s.clipboard.as_ref().is_some_and(|c| !c.cut && c.range.a1() == "D9"));
    // Confirmed, column B replaces column A.
    s.execute("range.move", json!({"range": "B:B", "to": "A1", "overwrite": true})).unwrap();
    assert_eq!(text(&mut s, "A1"), json!("Kiwi"));
    assert_eq!(text(&mut s, "A3"), json!("Lime"));
    assert_eq!(text(&mut s, "B1"), json!(""));
    // Onto empty cells, or overlapping only the moved cells themselves, nothing is asked.
    s.execute("range.move", json!({"range": "A1:A3", "to": "A2"})).unwrap();
    assert_eq!(text(&mut s, "A4"), json!("Lime"));
    s.execute("range.move", json!({"range": "A2:A4", "to": "C2"})).unwrap();
    assert_eq!(text(&mut s, "C2"), json!("Kiwi"));
    assert!(s.execute("range.move", json!({"range": "C2"})).is_err(), "`to` is required");
}

#[test]
fn drag_and_drop_asks_before_replacing_spilled_values() {
    // Cells showing a dynamic array's spilled values hold no cell of their own, but they are data.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)"})).unwrap();
    s.execute("range.setValues", json!({"range": "C2:C3", "values": [["Kiwi"], ["Plum"]]})).unwrap();
    let r = s.execute("range.move", json!({"range": "C2:C3", "to": "A2"}));
    let Err(EngineError::Confirm(q)) = r else { panic!("{r:?}") };
    assert_eq!(q, "There's already data here. Do you want to replace it?");
    let sh = s.doc().unwrap().wb.active().unwrap();
    assert_eq!(sh.value(CellRef::new(2, 2)), Value::text("Plum"));
    s.execute("range.move", json!({"range": "C2:C3", "to": "A2", "overwrite": true})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    assert_eq!(sh.value(CellRef::new(1, 0)), Value::text("Kiwi"));
}

#[test]
fn drag_and_drop_on_another_sheet_stays_on_that_sheet() {
    // The move was checked on the range's sheet but pasted on the active one, replacing its data
    // without asking.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("home.insertSheet", json!({"name": "Two"})).unwrap();
    s.execute("range.setValues", json!({"range": "B1:B2", "values": [["Kiwi"], ["Plum"]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "Keep"})).unwrap();
    s.execute("selection.set", json!({"range": "C5"})).unwrap();
    s.execute("range.move", json!({"range": "Two!B1:B2", "to": "A1"})).unwrap();
    // Sheet1 stays active with its own selection; the moved cells are on Two.
    assert_eq!(s.doc().unwrap().wb.active_sheet(), 0);
    assert_eq!(s.doc().unwrap().selection.a1(), "C5");
    let wb = &s.doc().unwrap().wb;
    let two = wb.sheet_index("Two").unwrap();
    assert_eq!(wb.sheet(0).unwrap().value(CellRef::new(0, 0)), Value::text("Keep"));
    assert_eq!(wb.sheet(two).unwrap().value(CellRef::new(0, 0)), Value::text("Kiwi"));
    assert_eq!(wb.sheet(two).unwrap().value(CellRef::new(1, 0)), Value::text("Plum"));
    assert_eq!(wb.sheet(two).unwrap().value(CellRef::new(0, 1)), Value::Empty);
}

#[test]
fn replace_refuses_text_that_would_grow_past_a_cell() {
    // 10,000 matches × a 1,000-character replacement would make a 10 MB cell (and a few such
    // cells gigabytes); the replacement is refused before that text is built.
    let mut s = Session::new();
    s.new_workbook();
    let long = "a".repeat(10_000);
    s.execute("range.setValues", json!({"range": "A1", "values": [[long.clone()], ["abc"]]})).unwrap();
    for case in [false, true] {
        let err = s.execute("edit.replace", json!({"what": "a", "with": "x".repeat(1_000), "matchCase": case})).expect_err("too long");
        assert!(err.to_string().contains("too long"), "{err}");
        assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["value"], json!(long));
        assert_eq!(s.execute("cell.get", json!({"cell": "A2"})).unwrap()["value"], json!("abc"));
    }
    // A replacement that stays within a cell's limit still happens.
    s.execute("edit.replace", json!({"what": "b", "with": "x".repeat(100)})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "A2"})).unwrap()["value"], json!(format!("a{}c", "x".repeat(100))));
}

#[test]
fn replace_refuses_a_result_past_the_character_limit_though_it_fits_the_byte_limit() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [["z"]]})).unwrap();
    for whole in [false, true] {
        let err = s.execute("edit.replace", json!({"what": "z", "with": "x".repeat(32_768), "wholeCell": whole})).expect_err("32,768 characters");
        assert!(err.to_string().contains("too long"), "{err}");
        assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["value"], json!("z"));
    }
    s.execute("edit.replace", json!({"what": "z", "with": "x".repeat(32_767)})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["value"], json!("x".repeat(32_767)));
}

#[test]
fn bounded_replace_stops_at_its_limit() {
    use super::find::{replace_ci_within, replace_cs_within};
    assert_eq!(replace_cs_within("abab", "a", "xy", 6).as_deref(), Some("xybxyb"));
    assert_eq!(replace_cs_within("abab", "a", "xy", 5), None);
    assert_eq!(replace_ci_within("AbAb", "a", "xy", 6).as_deref(), Some("xybxyb"));
    assert_eq!(replace_ci_within("AbAb", "a", "xy", 5), None);
    assert_eq!(replace_cs_within("abc", "z", "", 3).as_deref(), Some("abc"));
    assert_eq!(replace_cs_within("abc", "z", "", 2), None);
}

#[test]
fn replace_case_insensitive() {
    assert_eq!(replace_ci("Apple apple APPLE", "apple", "pear"), "pear pear pear");
}

#[test]
fn replace_case_insensitive_keeps_text_when_folding_moves_boundaries() {
    // U+212A KELVIN SIGN folds to `k` (3→1 bytes) while U+0130 folds to `i` plus a
    // combining dot (2→3 bytes): the folded text is the same length but its byte positions
    // no longer line up with the original, and the old slicing dropped the tail.
    let mut s = Session::new();
    s.new_workbook();
    let text = "\u{212A}\u{0130}\u{0130}";
    s.execute("cell.set", json!({"cell": "A1", "input": text})).unwrap();
    s.execute("edit.replace", json!({"what": "k", "with": "x"})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    assert_eq!(sh.value(CellRef::new(0, 0)), Value::text(text));
}

#[test]
fn next_corner_of_selection_goes_clockwise_and_anchors_there() {
    // #1442, as Excel does it: B2 → D2 → D4 → B4 → B2, the area stays selected, and Shift+arrows
    // then move the corner opposite the new active cell.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("selection.set", json!({"range": "B2:D4", "active": "B2"})).unwrap();
    let mut seen = Vec::new();
    for _ in 0..4 {
        let r = s.execute("selection.nextCorner", json!({})).unwrap();
        assert_eq!(r["selection"], "B2:D4");
        seen.push(r["active"].as_str().unwrap().to_string());
    }
    assert_eq!(seen, ["D2", "D4", "B4", "B2"]);
    s.execute("selection.nextCorner", json!({})).unwrap();
    s.execute("selection.nextCorner", json!({})).unwrap();
    s.execute("selection.move", json!({"dr": 0, "dc": 1, "extend": true})).unwrap();
    assert_eq!(s.doc().unwrap().selection.current().a1(), "C2:D4");
    // One row goes end to end; a single cell stays.
    s.execute("selection.set", json!({"range": "B2:D2", "active": "B2"})).unwrap();
    assert_eq!(s.execute("selection.nextCorner", json!({})).unwrap()["active"], "D2");
    assert_eq!(s.execute("selection.nextCorner", json!({})).unwrap()["active"], "B2");
    s.execute("selection.set", json!({"range": "C5"})).unwrap();
    assert_eq!(s.execute("selection.nextCorner", json!({})).unwrap()["active"], "C5");
}

#[test]
fn next_corner_skips_corners_the_sheet_protection_does_not_let_you_select() {
    // With locked cells unselectable, Ctrl+. goes on to the next corner it may select.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("home.lockCell", json!({"range": "B2:C3", "on": false})).unwrap();
    s.execute("selection.set", json!({"range": "B2:D3", "active": "B2"})).unwrap();
    s.execute("review.protectSheet", json!({"selectLockedCells": false})).unwrap();
    let r = s.execute("selection.nextCorner", json!({})).unwrap();
    assert_eq!((r["active"].as_str(), r["selection"].as_str()), (Some("B3"), Some("B2:D3")), "D2 and D3 are locked");
    assert_eq!(s.execute("selection.nextCorner", json!({})).unwrap()["active"], "B2");
}

#[test]
fn next_corner_stops_once_on_a_merged_corner_at_its_top_left_cell() {
    // A corner inside a merged cell is that merged cell, whose active cell is its top-left one.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("home.mergeCells", json!({"range": "C2:D2"})).unwrap();
    s.execute("selection.set", json!({"range": "B2:D3", "active": "B2"})).unwrap();
    let seen: Vec<String> = (0..4).map(|_| s.execute("selection.nextCorner", json!({})).unwrap()["active"].as_str().unwrap().to_string()).collect();
    assert_eq!(seen, ["C2", "D3", "B3", "B2"]);
    // A selection that is one merged cell has no other corner.
    s.execute("selection.set", json!({"range": "C2:D2", "active": "C2"})).unwrap();
    assert_eq!(s.execute("selection.nextCorner", json!({})).unwrap()["active"], "C2");
}

#[test]
fn next_corner_skips_hidden_corners_and_stays_inside_the_selected_area() {
    // A corner in a hidden column is passed over, as keyboard moves pass over hidden cells.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("home.hideColumns", json!({"cols": "D:D"})).unwrap();
    s.execute("selection.set", json!({"range": "B2:D3", "active": "B2"})).unwrap();
    assert_eq!(s.execute("selection.nextCorner", json!({})).unwrap()["active"], "B3");
    // Column C selected with B1:C1 merged: its top corner's merged cell starts in column B,
    // outside the selection, so the active cell doesn't go there.
    let mut s = Session::new();
    s.new_workbook();
    s.execute("home.mergeCells", json!({"range": "B1:C1"})).unwrap();
    s.execute("selection.set", json!({"cell": "C5"})).unwrap();
    s.execute("selection.column", json!({})).unwrap();
    let r = s.execute("selection.nextCorner", json!({})).unwrap();
    assert_eq!((r["active"].as_str(), r["selection"].as_str()), (Some("C1048576"), Some("C:C")));
}

#[test]
fn copy_quotes_only_cells_with_tabs_or_line_breaks() {
    // #2225: a cell holding a quote was wrapped in quotes on copy; Excel quotes only text with a
    // tab or line break, and pastes the result back as the same cells.
    let mut s = Session::new();
    s.new_workbook();
    for (cell, input) in [("A1", "say \"hi\""), ("A2", "plain"), ("A3", "\"hi\" there"), ("A4", "x \"q\"\ny"), ("A5", "tab\tinside")] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    let text = s.execute("edit.copy", json!({"range": "A1:A5"})).unwrap()["text"].as_str().unwrap().to_string();
    assert_eq!(text, "say \"hi\"\nplain\n\"hi\" there\n\"x \"\"q\"\"\ny\"\n\"tab\tinside\"\n");
    let rows = parse_tsv(&text);
    let cells: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(cells, ["say \"hi\"", "plain", "\"hi\" there", "x \"q\"\ny", "tab\tinside"]);
}

#[test]
fn a_field_that_is_not_really_quoted_keeps_its_separators() {
    // A field opening with a quote is quoted only if a closing quote ends it; when it doesn't (or
    // never closes), the text is read as written, and the tabs and line breaks inside it still
    // separate cells, as they would have without the leading quote.
    assert_eq!(parse_tsv("\"a\tb\"x\tc"), vec![vec!["\"a", "b\"x", "c"]]);
    assert_eq!(parse_tsv("\"a\tb\nc"), vec![vec!["\"a", "b"], vec!["c"]]);
    assert_eq!(parse_tsv("\"a\tb\tc\n"), vec![vec!["\"a", "b", "c"]]);
    // Cells copied without quotes (they hold no tab or line break) read back as the same cells.
    let mut s = Session::new();
    s.new_workbook();
    for (cell, input) in [("A1", "'\"a"), ("B1", "b\"x"), ("C1", "c")] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    let text = s.execute("edit.copy", json!({"range": "A1:C1"})).unwrap()["text"].as_str().unwrap().to_string();
    assert_eq!(parse_tsv(&text), vec![vec!["\"a", "b\"x", "c"]], "{text:?}");
    // Many fields that only look quoted: each is replayed at most once (the replay budget is the
    // input's length), and every one is still its own cell.
    let hostile = "\"x\t".repeat(50_000);
    assert_eq!(parse_tsv(&hostile).first().map(Vec::len), Some(50_001));
}

/// `within: selection` searches only the selected cells, as Excel does with a range selected
/// (#3224): Find All lists them, Find Next moves the active cell through them and keeps the range
/// selected, and Replace changes only them.
#[test]
fn find_and_replace_within_the_selection() {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1:B4", "values": [["pear", "pear"], ["plum", "pear"], ["pear", "fig"], ["pear", "pear"]]}))
        .unwrap();
    s.execute("selection.set", json!({"range": "B1:B4"})).unwrap();
    let all = s.execute("edit.find", json!({"what": "pear", "all": true, "within": "selection"})).unwrap();
    let cells: Vec<&str> = all["results"].as_array().unwrap().iter().filter_map(|r| r["cell"].as_str()).collect();
    assert_eq!(cells, ["B1", "B2", "B4"], "only column B");
    assert_eq!(s.execute("edit.find", json!({"what": "pear", "within": "selection"})).unwrap()["cell"], "B2");
    assert_eq!(s.execute("edit.find", json!({"what": "pear", "within": "selection"})).unwrap()["cell"], "B4");
    assert_eq!(s.execute("edit.find", json!({"what": "pear", "within": "selection"})).unwrap()["cell"], "B1", "wraps inside the range");
    let sel = &s.doc().unwrap().selection;
    assert_eq!((sel.a1(), sel.active.a1()), ("B1:B4".to_string(), "B1".to_string()), "the range stays selected");
    assert!(s.execute("edit.find", json!({"what": "plum", "within": "selection"})).is_err(), "A2 is outside the range");
    assert_eq!(s.execute("edit.replace", json!({"what": "pear", "with": "kiwi", "within": "selection"})).unwrap()["replaced"], 3);
    let sh = s.doc().unwrap().wb.active().unwrap();
    let at = |a: &str| sh.value(CellRef::parse(a).unwrap());
    assert_eq!((at("A1"), at("B1"), at("B4"), at("A4")), (Value::text("pear"), Value::text("kiwi"), Value::text("kiwi"), Value::text("pear")));
    // The sheet scope is unchanged.
    let all = s.execute("edit.find", json!({"what": "pear", "all": true})).unwrap();
    assert_eq!(all["count"], 3, "A1, A3 and A4 on the whole sheet");
}
