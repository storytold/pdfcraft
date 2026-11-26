//! Build a workbook using every feature, write it, read it back, compare.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, DateSystem, RangeRef, Value};
use gridcraft_model::*;

use crate::package::Package;
use crate::{read_xlsx, write_xlsx};

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}
fn rr(a: &str) -> RangeRef {
    RangeRef::parse(a).unwrap()
}

fn sample() -> Workbook {
    let mut wb = Workbook::new();
    wb.date_system = DateSystem::D1904;
    wb.calc = CalcSettings { mode: CalcMode::Manual, iterative: true, max_iterations: 42, max_change: 0.5, precision_as_displayed: true, r1c1: true };
    wb.protected_structure = true;
    wb.structure_password = Some(gridcraft_model::PasswordHash::new_sha512("s3cret", b"0123456789abcdef"));
    wb.props = DocProps {
        title: "T & <x>".into(),
        subject: "S".into(),
        author: "Me".into(),
        company: "Co".into(),
        keywords: "k".into(),
        description: "d".into(),
    };
    wb.theme.colors[4] = 0x112233;
    wb.theme.minor_font = "Aptos".into();

    let bold = wb.styles.derive(StyleId::DEFAULT, |s| {
        s.font.bold = true;
        s.font.color = Color::Theme(4, 400);
        s.fill = Fill::solid(Color::Rgb(0xFFEE00));
        s.borders.bottom = BorderLine { style: BorderStyle::Double, color: Color::Rgb(0xFF0000) };
        s.borders.diag_down = BorderLine::thin();
        s.align = Alignment {
            h: HAlign::Center,
            v: VAlign::Top,
            wrap: true,
            shrink: false,
            indent: 1,
            rotation: -30,
            reading_order: Some(ReadingOrder::RightToLeft),
        };
        s.protection.locked = false;
    });
    let money = wb.styles.derive(StyleId::DEFAULT, |s| s.num_fmt = NumFmt::new("#,##0.00 \"€\""));
    let pct = wb.styles.derive(StyleId::DEFAULT, |s| {
        s.num_fmt = NumFmt::new("0.00%");
        s.font = Font {
            name: "Arial".into(),
            size: 9.5,
            italic: true,
            underline: Underline::SingleAccounting,
            strike: true,
            vert: VertAlign::Subscript,
            ..Font::default()
        };
        s.fill = Fill { pattern: PatternType::LightTrellis, fg: Color::Theme(5, -250), bg: Color::Rgb(0x00FF00) };
    });
    let red = Style {
        font: Font { color: Color::Rgb(0x9C0006), bold: true, ..Font::default() },
        fill: Fill::solid(Color::Rgb(0xFFC7CE)),
        ..Style::default()
    };

    let mut s = Sheet::new("Data & Stuff");
    let set = |s: &mut Sheet, a: &str, v: Value, style: StyleId| s.cells.set(at(a), Cell { value: v, formula: None, style, py: None });
    set(&mut s, "A1", Value::text("Region"), bold);
    set(&mut s, "B1", Value::text("Amount"), bold);
    set(&mut s, "C1", Value::text("  spaced\nline _x0041_ \u{1} "), StyleId::DEFAULT);
    set(&mut s, "A2", Value::text("North"), StyleId::DEFAULT);
    set(&mut s, "B2", Value::Number(1234.5), money);
    set(&mut s, "A3", Value::text("South"), StyleId::DEFAULT);
    set(&mut s, "B3", Value::Number(-0.000123), pct);
    set(&mut s, "D1", Value::Bool(true), StyleId::DEFAULT);
    set(&mut s, "D2", Value::Error(CellError::NA), StyleId::DEFAULT);
    set(&mut s, "D3", Value::Number(1e300), StyleId::DEFAULT);
    set(&mut s, "E5", Value::Empty, bold);
    let mut f = Cell::formula(Formula::new("=SUM(B2:B3)*2"));
    f.value = Value::Number(2468.9);
    s.cells.set(at("B4"), f);
    let mut f = Cell::formula(Formula::new("XLOOKUP(\"North\",A2:A3,B2:B3)&TEXTJOIN(\",\",TRUE,A2:A3)"));
    f.value = Value::text("x");
    s.cells.set(at("C4"), f);
    let mut f = Cell::formula(Formula::new("ISNUMBER(B2)"));
    f.value = Value::Bool(true);
    s.cells.set(at("D4"), f);
    let mut f = Cell::formula(Formula::new("1/0"));
    f.value = Value::Error(CellError::Div0);
    s.cells.set(at("E4"), f);
    s.cells.set(at("F4"), Cell::formula(Formula::new("NOW()")));
    let mut arr = Formula::new("B2:B3*2");
    arr.array = Some(rr("G2:G3"));
    let mut c = Cell::formula(arr);
    c.value = Value::Number(2469.0);
    s.cells.set(at("G2"), c);
    // A dynamic-array formula with spilled values.
    let mut dynf = Cell::formula(Formula::new("SEQUENCE(3)"));
    dynf.value = Value::Number(1.0);
    s.cells.set(at("H1"), dynf);
    s.spill_ranges.insert(at("H1"), rr("H1:H3"));
    s.spill.insert(at("H2"), Value::Number(2.0));
    s.spill.insert(at("H3"), Value::Number(3.0));

    s.cols.insert(0, LineInfo { size: Some(100.0), ..Default::default() });
    s.cols.insert(1, LineInfo { size: Some(100.0), ..Default::default() });
    s.cols.insert(3, LineInfo { hidden: true, outline: 1, ..Default::default() });
    s.cols.insert(5, LineInfo { style: Some(money), ..Default::default() });
    s.rows.insert(0, LineInfo { size: Some(30.0), ..Default::default() });
    s.rows.insert(9, LineInfo { hidden: true, outline: 2, collapsed: true, ..Default::default() });
    s.rows.insert(10, LineInfo { style: Some(bold), ..Default::default() });
    s.default_row_height = 24.0;
    s.default_col_width = 72.0;
    s.merges.push(rr("A20:C21"));
    s.freeze = Some((1, 2));
    s.tab_color = Some(Color::Rgb(0x00B050));
    s.show_gridlines = false;
    s.show_headings = false;
    s.show_zeros = false;
    s.right_to_left = true;
    s.zoom = 125;
    s.view = gridcraft_model::SheetViewMode::PageLayout;
    s.view_active = at("C7");

    let red_b = Box::new(red.clone());
    let cfs = vec![
        CfRule::CellIs { op: CfOperator::Between, a: "1".into(), b: Some("10".into()), style: red_b.clone() },
        CfRule::Expression { formula: "MOD(ROW(),2)=0".into(), style: red_b.clone() },
        CfRule::ContainsText { text: "a\"b".into(), style: red_b.clone() },
        CfRule::NotContainsText { text: "zz".into(), style: red_b.clone() },
        CfRule::BeginsWith { text: "No".into(), style: red_b.clone() },
        CfRule::EndsWith { text: "th".into(), style: red_b.clone() },
        CfRule::Blanks { style: red_b.clone() },
        CfRule::NoBlanks { style: red_b.clone() },
        CfRule::Errors { style: red_b.clone() },
        CfRule::NoErrors { style: red_b.clone() },
        CfRule::Duplicate { style: red_b.clone() },
        CfRule::Unique { style: red_b.clone() },
        CfRule::Top10 { bottom: true, percent: true, rank: 5, style: red_b.clone() },
        CfRule::AboveAverage { below: true, equal: true, std_dev: 1, style: red_b.clone() },
        CfRule::TimePeriod { period: "last7Days".into(), style: red_b.clone() },
        CfRule::ColorScale {
            stops: vec![
                (CfValueKind::Min, Color::Rgb(0xF8696B)),
                (CfValueKind::Percentile(50.0), Color::Rgb(0xFFEB84)),
                (CfValueKind::Max, Color::Rgb(0x63BE7B)),
            ],
        },
        CfRule::DataBar {
            min: CfValueKind::Number(0.0),
            max: CfValueKind::Formula("$B$9".into()),
            color: Color::Rgb(0x638EC6),
            gradient: true,
            show_value: false,
        },
        CfRule::IconSet {
            set: "3Arrows".into(),
            thresholds: vec![CfValueKind::Percent(0.0), CfValueKind::Percent(33.0), CfValueKind::Percent(67.0)],
            reverse: true,
            show_value: false,
        },
    ];
    for (i, rule) in cfs.into_iter().enumerate() {
        s.cond_formats.push(CondFormat { ranges: vec![rr("B2:B10"), rr("D2")], rule, priority: i as u32 + 1, stop_if_true: i == 0 });
    }
    s.validations.push(Validation {
        ranges: vec![rr("A2:A10")],
        kind: ValidationKind::List,
        f1: "\"North,South\"".into(),
        input_title: "Pick".into(),
        input_message: "One".into(),
        ..Default::default()
    });
    s.validations.push(Validation {
        ranges: vec![rr("I1")],
        kind: ValidationKind::List,
        f1: "=$A$2:$A$3".into(),
        in_cell_dropdown: false,
        ..Default::default()
    });
    s.validations.push(Validation {
        ranges: vec![rr("B2:B10")],
        kind: ValidationKind::Decimal,
        op: CfOperator::GreaterOrEqual,
        f1: "0".into(),
        error_style: ErrorStyle::Information,
        error_title: "Bad".into(),
        error_message: "No".into(),
        allow_blank: false,
        ..Default::default()
    });
    s.edit_ranges = vec![
        EditRange { title: "Open & <free>".into(), ranges: vec![rr("A1:B2"), rr("D4")], password: None, security_descriptor: None },
        EditRange {
            title: "Old".into(),
            ranges: vec![rr("C1:C9")],
            password: Some(PasswordHash::Legacy(0x83AF)),
            security_descriptor: Some("O:WDG:WDD:(A;;CC;;;WD)".into()),
        },
        EditRange {
            title: "New".into(),
            ranges: vec![rr("E1")],
            password: Some(PasswordHash::new_sha512("pw", b"0123456789abcdef")),
            security_descriptor: None,
        },
    ];
    s.hyperlinks.insert(at("A2"), Hyperlink { target: "https://example.com/?a=1&b=2".into(), tooltip: Some("go".into()), external: true });
    s.hyperlinks.insert(at("A3"), Hyperlink { target: "'Data & Stuff'!B2".into(), tooltip: None, external: false });
    s.hyperlinks.insert(at("A4"), Hyperlink { target: "mailto:me@example.com".into(), tooltip: None, external: true });
    s.comments.insert(
        at("B2"),
        Comment {
            author: "Ann".into(),
            text: "Note & <stuff>\nline 2".into(),
            replies: vec![],
            threaded: false,
            resolved: false,
            visible: false,
            dates: vec![],
            note_box: None,
        },
    );
    s.comments.insert(
        at("C3"),
        Comment {
            author: "Bob".into(),
            text: "second".into(),
            replies: vec![],
            threaded: false,
            resolved: false,
            visible: true,
            dates: vec![],
            note_box: None,
        },
    );
    s.comments.insert(
        at("D4"),
        Comment {
            author: "Ann & Co".into(),
            text: "Threaded <one>\nsecond line".into(),
            replies: vec![("Bob".into(), "first reply".into()), ("Ann & Co".into(), "second reply".into())],
            threaded: true,
            resolved: true,
            visible: false,
            dates: vec!["2026-10-11T09:30:00.00".into(), String::new(), "2026-10-11T10:15:00.00".into()],
            note_box: None,
        },
    );
    s.comments.insert(
        at("E5"),
        Comment {
            author: "Cy".into(),
            text: "open thread".into(),
            replies: vec![],
            threaded: true,
            resolved: false,
            visible: false,
            dates: vec![],
            note_box: None,
        },
    );
    s.autofilter = Some(AutoFilter {
        range: rr("K1:N20"),
        criteria: vec![
            (0, FilterCriterion::Values { values: vec!["a".into(), "b".into()], blanks: true }),
            (1, FilterCriterion::Custom { a: (">=".into(), "5".into()), b: Some(("<".into(), "9".into())), and: true }),
            (2, FilterCriterion::Top10 { bottom: true, percent: false, count: 3 }),
            (3, FilterCriterion::FillColor(Color::Rgb(0xFF0000))),
        ],
    });
    s.protection = Some(SheetProtection {
        format_cells: true,
        sort: true,
        select_locked: true,
        select_unlocked: true,
        password: Some(gridcraft_model::PasswordHash::new_sha512("sheet pw", b"fedcba9876543210")),
        ..Default::default()
    });
    s.print = PrintSettings {
        orientation: Orientation::Landscape,
        paper: "A4".into(),
        margins: [0.5, 0.6, 0.7, 0.8, 0.2, 0.25],
        print_area: Some(rr("A1:H30")),
        title_rows: Some((0, 1)),
        title_cols: Some((0, 0)),
        scale: 80,
        fit_to: Some((1, 0)),
        gridlines: true,
        headings: true,
        center_h: true,
        center_v: false,
        header: "&L&D&RPage &P".into(),
        footer: "&C&F".into(),
        row_breaks: vec![15, 30],
        col_breaks: vec![4],
        different_first: true,
        different_odd_even: true,
        first_header: "&CCover".into(),
        first_footer: String::new(),
        even_header: "&LEven &P".into(),
        even_footer: "&R&A".into(),
        hf_scale_with_doc: false,
        hf_align_with_margins: true,
        page_order: PageOrder::OverThenDown,
        first_page_number: Some(7),
    };
    s.sparklines.push(Sparkline { cell: at("J2"), source: "B2:B3".into(), kind: SparklineKind::WinLoss, color: Color::Rgb(0x112233), markers: true });
    s.tables.push(Table {
        id: 1,
        name: "Sales Table".into(),
        range: rr("P1:R5"),
        header_row: true,
        totals_row: true,
        columns: vec![
            TableColumn {
                name: "Region".into(),
                totals: TotalsFn::None,
                totals_label: Some("Total".into()),
                formula: None,
                unique_name: None,
                xml: None,
            },
            TableColumn { name: "Amount".into(), totals: TotalsFn::Sum, totals_label: None, formula: None, unique_name: None, xml: None },
            TableColumn {
                name: "Tax".into(),
                totals: TotalsFn::Custom,
                totals_label: Some("SUM(R2:R4)".into()),
                formula: Some("Q2*0.1".into()),
                unique_name: None,
                xml: None,
            },
        ],
        style: "TableStyleLight9".into(),
        banded_rows: true,
        banded_cols: true,
        first_col: true,
        last_col: false,
        filter_button: true,
        filter: vec![
            (0, FilterCriterion::Values { values: vec!["East".into()], blanks: false }),
            (1, FilterCriterion::Icon { set: "3TrafficLights1".into(), id: Some(1) }),
        ],
        table_type: None,
        connection_id: None,
    });
    let png = b"\x89PNG\r\n\x1a\n0000IHDRfake".to_vec();
    s.images.push(Image {
        id: 10,
        // A non-default anchor mode, to prove it survives an xlsx round-trip.
        anchor: Anchor { cell: at("C10"), dx: 5.0, dy: 4.0, width: 120.0, height: 80.0, mode: AnchorMode::MoveOnly },
        data: png.into(),
        mime: "image/png".into(),
        alt: "Logo \"x\"".into(),
    });
    s.shapes.push(Shape {
        math: Default::default(),
        id: 11,
        kind: ShapeKind::Ellipse,
        anchor: Anchor { cell: at("E10"), dx: 0.0, dy: 0.0, width: 64.0, height: 40.0, mode: AnchorMode::Absolute },
        fill: Color::Rgb(0x4472C4),
        line: Color::Rgb(0x000000),
        text: "Hi\nthere".into(),
        format: Default::default(),
        rotation: 30.0,
    });
    s.shapes.push(Shape {
        math: Default::default(),
        id: 12,
        kind: ShapeKind::Arrow,
        anchor: Anchor { cell: at("E14"), dx: 0.0, dy: 0.0, width: 64.0, height: 0.0, ..Default::default() },
        fill: Color::Auto,
        line: Color::Rgb(0xFF0000),
        text: String::new(),
        format: Default::default(),
        rotation: 0.0,
    });
    s.shapes.push(Shape {
        math: Default::default(),
        id: 13,
        kind: ShapeKind::TextBox,
        anchor: Anchor { cell: at("E16"), dx: 0.0, dy: 0.0, width: 64.0, height: 40.0, ..Default::default() },
        fill: Color::Auto,
        line: Color::Auto,
        text: "box".into(),
        format: ShapeFormat {
            no_line: true,
            font_size: Some(20.0),
            bold: true,
            italic: true,
            font: Some("Arial".into()),
            text_color: Some(Color::Rgb(0x112233)),
            text_outline: Some((Color::Rgb(0xFF0000), 1.5)),
            text_shadow: Some(Color::Rgb(0x445566)),
            align: Some(ShapeTextAlign::Right),
            anchor: Some(ShapeTextAnchor::Bottom),
            rtl: true,
        },
        rotation: 0.0,
    });
    let base_chart = Chart {
        id: 20,
        kind: ChartKind::ColumnClustered,
        anchor: Anchor { cell: at("J5"), dx: 0.0, dy: 0.0, width: 480.0, height: 288.0, ..Default::default() },
        title: Some("Sales".into()),
        series: vec![Series {
            name: Some("'Data & Stuff'!$B$1".into()),
            categories: Some("'Data & Stuff'!$A$2:$A$3".into()),
            values: "'Data & Stuff'!$B$2:$B$3".into(),
            bubble_sizes: None,
            color: Some(Color::Rgb(0xFF0000)),
            secondary: false,
            kind: None,
            quartiles: Default::default(),
            smooth: false,
            trendlines: Vec::new(),
        }],
        legend: LegendPos::Bottom,
        data_labels: true,
        data_table: None,
        gridlines: true,
        style: 2,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: Some("Region".into()),
        y_title: Some("Amount".into()),
        source: None,
        by_rows: false,
        pivot: None,
    };
    for kind in [
        ChartKind::ColumnClustered,
        ChartKind::ColumnStacked,
        ChartKind::ColumnStacked100,
        ChartKind::BarClustered,
        ChartKind::BarStacked,
        ChartKind::BarStacked100,
        ChartKind::Line,
        ChartKind::LineMarkers,
        ChartKind::LineStacked,
        ChartKind::Pie,
        ChartKind::Doughnut,
        ChartKind::Area,
        ChartKind::AreaStacked,
        ChartKind::AreaStacked100,
        ChartKind::Scatter,
        ChartKind::ScatterLines,
        ChartKind::Bubble,
        ChartKind::Radar,
    ] {
        s.charts.push(Chart { kind, ..base_chart.clone() });
    }
    let mut combo = Chart { kind: ChartKind::Combo, legend: LegendPos::Right, ..base_chart.clone() };
    combo.series.push(Series {
        name: Some("Literal".into()),
        kind: Some(ChartKind::Line),
        secondary: true,
        color: None,
        smooth: false,
        ..base_chart.series[0].clone()
    });
    s.charts.push(combo);
    s.charts.push(Chart { kind: ChartKind::Waterfall, legend: LegendPos::None, title: None, ..base_chart.clone() });
    let mut doughnut = Chart { kind: ChartKind::Doughnut, hole_size: 25, first_slice_angle: 180, legend: LegendPos::Right, ..base_chart.clone() };
    doughnut.series.push(base_chart.series[0].clone());
    s.charts.push(doughnut);

    wb.sheets = vec![Arc::new(s)];
    let mut s2 = Sheet::new("Hidden");
    s2.visibility = Visibility::Hidden;
    s2.set_value(at("A1"), Value::Number(5.0));
    wb.sheets.push(Arc::new(s2));
    let mut s3 = Sheet::new("2024");
    s3.visibility = Visibility::VeryHidden;
    s3.comments.insert(
        at("A1"),
        Comment {
            author: "Bob".into(),
            text: "other sheet".into(),
            replies: vec![],
            threaded: true,
            resolved: false,
            visible: false,
            dates: vec![],
            note_box: None,
        },
    );
    wb.sheets.push(Arc::new(s3));
    wb.names.push(DefinedName { name: "Rate".into(), scope: None, formula: "0.07".into(), comment: "c".into(), hidden: false });
    wb.names.push(DefinedName { name: "Local".into(), scope: Some(1), formula: "Hidden!$A$1".into(), comment: String::new(), hidden: true });
    wb.names.push(DefinedName {
        name: "Pick".into(),
        scope: None,
        formula: "XLOOKUP(1,Hidden!A:A,Hidden!A:A)".into(),
        comment: String::new(),
        hidden: false,
    });
    wb.cell_styles.push(("Good".into(), red));
    wb
}

#[test]
fn full_roundtrip() {
    let wb = sample();
    let bytes = write_xlsx(&wb).unwrap();
    assert_eq!(write_xlsx(&wb).unwrap(), bytes, "output is deterministic");
    let (back, rep) = read_xlsx(&bytes).unwrap();
    assert!(rep.warnings.iter().all(|w| !w.contains("skipped") || w.contains("chart")), "{:?}", rep.warnings);
    assert_workbook_metadata_roundtrip(&wb, &back);

    let (a, b) = (wb.sheet(0).unwrap(), back.sheet(0).unwrap());
    assert_sheet_cells_and_settings(&wb, &back, a, b);
    assert_table_roundtrip(a, b);
    assert_drawing_and_chart_roundtrip(a, b);
    assert_eq!(back.sheet(1).unwrap().value(at("A1")), Value::Number(5.0));
}

fn assert_workbook_metadata_roundtrip(wb: &Workbook, back: &Workbook) {
    assert_eq!(back.date_system, wb.date_system);
    assert_eq!(back.calc, wb.calc);
    assert!(back.protected_structure);
    assert_eq!(back.structure_password, wb.structure_password);
    assert!(back.structure_password.as_ref().unwrap().verify("s3cret").unwrap());
    assert_eq!(back.props, wb.props);
    assert_eq!(back.theme, wb.theme);
    assert_eq!(back.names, wb.names);
    assert_eq!(back.cell_styles.len(), 1);
    assert_eq!(back.sheets.len(), 3);
    assert_eq!(back.sheets[1].visibility, Visibility::Hidden);
    assert_eq!(back.sheets[2].visibility, Visibility::VeryHidden);
    assert_eq!(back.sheets[2].name, "2024");
    assert_eq!(back.sheets[2].comments, wb.sheets[2].comments);
}

fn assert_sheet_cells_and_settings(wb: &Workbook, back: &Workbook, a: &Sheet, b: &Sheet) {
    assert_eq!(b.name, a.name);
    // Cells: values, formulas and resolved styles.
    let skip = [at("H2"), at("H3")];
    for (c, cell) in a.cells.iter() {
        let got = b.cell(c).unwrap_or_else(|| panic!("missing {c}"));
        assert_eq!(got.value, cell.value, "value at {c}");
        assert_eq!(got.formula.as_ref().map(|f| (&f.text, f.array)), cell.formula.as_ref().map(|f| (&f.text, f.array)), "formula at {c}");
        assert_eq!(back.styles.get(got.style), wb.styles.get(cell.style), "style at {c}");
    }
    for (c, _) in b.cells.iter() {
        assert!(a.cell(c).is_some() || skip.contains(&c) || a.tables.iter().any(|t| t.range.contains(c)), "extra {c}");
    }
    // Spilled cells are written as cached values and dropped again on read.
    assert!(b.cell(at("H2")).is_none());
    assert_eq!(b.cols, a.cols);
    assert_eq!(b.rows, a.rows);
    assert_eq!(b.default_row_height, a.default_row_height);
    assert_eq!(b.default_col_width, a.default_col_width);
    assert_eq!(b.merges, a.merges);
    assert_eq!(b.freeze, a.freeze);
    assert_eq!(b.tab_color, a.tab_color);
    assert_eq!((b.show_gridlines, b.show_headings, b.show_zeros, b.right_to_left, b.zoom), (false, false, false, true, 125));
    assert_eq!(b.view_active, a.view_active);
    assert_eq!(b.view, gridcraft_model::SheetViewMode::PageLayout, "sheetView view=\"pageLayout\" round-trips");
    assert_eq!(b.cond_formats, a.cond_formats);
    assert_eq!(b.validations, a.validations);
    assert_eq!(b.hyperlinks, a.hyperlinks);
    assert_eq!(b.comments, a.comments);
    assert_eq!(b.autofilter, a.autofilter);
    assert_eq!(b.protection, a.protection);
    assert_eq!(b.edit_ranges, a.edit_ranges);
    assert_eq!(b.print, a.print);
    let mut sp = a.sparklines.clone();
    sp[0].source = "'Data & Stuff'!B2:B3".into();
    assert_eq!(b.sparklines, sp);
}

fn assert_table_roundtrip(a: &Sheet, b: &Sheet) {
    // Tables: name sanitized, ids reassigned.
    let (ta, tb) = (&a.tables[0], &b.tables[0]);
    assert_eq!(tb.name, "Sales_Table");
    assert_eq!((tb.range, tb.header_row, tb.totals_row, &tb.columns, &tb.style), (ta.range, ta.header_row, ta.totals_row, &ta.columns, &ta.style));
    assert_eq!((tb.banded_rows, tb.banded_cols, tb.first_col, tb.last_col, tb.filter_button), (true, true, true, false, true));
    assert_eq!(tb.filter, ta.filter, "the table's filter criteria survive (SUBTOTAL tells filtered rows apart)");
    assert_eq!(b.value(at("P1")), Value::text("Region"), "header cells carry the column names");
}

fn assert_drawing_and_chart_roundtrip(a: &Sheet, b: &Sheet) {
    // Drawings.
    assert_eq!(b.images.len(), 1);
    assert_eq!(b.images[0].data, a.images[0].data);
    assert_eq!(b.images[0].alt, a.images[0].alt);
    let close = |x: f32, y: f32| (x - y).abs() < 0.01;
    let (ia, ib) = (a.images[0].anchor, b.images[0].anchor);
    assert!(
        ia.cell == ib.cell && close(ia.dx, ib.dx) && close(ia.dy, ib.dy) && close(ia.width, ib.width) && close(ia.height, ib.height),
        "{ia:?} {ib:?}"
    );
    // The anchor mode (Excel's editAs) survives the round-trip.
    assert_eq!(ib.mode, AnchorMode::MoveOnly, "image anchor mode");
    assert_eq!(b.shapes[0].anchor.mode, AnchorMode::Absolute, "shape anchor mode");
    assert_eq!(b.shapes[1].anchor.mode, AnchorMode::MoveAndSize, "default anchor mode");
    assert_eq!(b.shapes.len(), 3);
    for (x, y) in a.shapes.iter().zip(&b.shapes) {
        assert_eq!((x.kind, &x.text, x.fill, x.line, &x.format), (y.kind, &y.text, y.fill, y.line, &y.format));
    }
    assert_eq!(b.charts.len(), a.charts.len());
    for (x, y) in a.charts.iter().zip(&b.charts) {
        assert_eq!(y.kind, x.kind, "chart kind");
        assert_eq!(y.title, x.title);
        assert_eq!(y.legend, x.legend);
        assert_eq!(y.data_labels, x.data_labels);
        assert_eq!((y.hole_size, y.first_slice_angle), (x.hole_size, x.first_slice_angle));
        if !matches!(x.kind, ChartKind::Pie | ChartKind::Doughnut) {
            assert_eq!(y.gridlines, x.gridlines);
            assert_eq!((&y.x_title, &y.y_title), (&x.x_title, &x.y_title));
        }
        assert_eq!(y.series.len(), x.series.len());
        for (sx, sy) in x.series.iter().zip(&y.series) {
            assert_eq!(sy.name, sx.name);
            assert_eq!(sy.categories, sx.categories);
            assert_eq!(sy.values, sx.values);
            assert_eq!(sy.color, sx.color);
            assert_eq!(sy.secondary, sx.secondary);
        }
        assert!(close(y.anchor.width, x.anchor.width) && close(y.anchor.height, x.anchor.height));
    }
}

#[test]
fn package_structure() {
    let bytes = write_xlsx(&sample()).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names: Vec<String> = (0..z.len()).map(|i| z.name_for_index(i).unwrap().to_string()).collect();
    assert_eq!(names[0], "[Content_Types].xml");
    for n in [
        "_rels/.rels",
        "docProps/core.xml",
        "docProps/app.xml",
        "xl/workbook.xml",
        "xl/_rels/workbook.xml.rels",
        "xl/styles.xml",
        "xl/theme/theme1.xml",
        "xl/sharedStrings.xml",
        "xl/metadata.xml",
        "xl/worksheets/sheet1.xml",
        "xl/worksheets/_rels/sheet1.xml.rels",
        "xl/tables/table1.xml",
        "xl/comments1.xml",
        "xl/drawings/vmlDrawing1.vml",
        "xl/drawings/drawing1.xml",
        "xl/drawings/_rels/drawing1.xml.rels",
        "xl/charts/chart1.xml",
        "xl/media/image1.png",
    ] {
        assert!(names.iter().any(|x| x == n), "missing {n}");
    }
    // Every part parses as XML; every override names an existing part; every internal
    // relationship target exists.
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    for n in &names {
        let mut f = z.by_name(n).unwrap();
        let mut d = vec![];
        f.read_to_end(&mut d).unwrap();
        if n.ends_with(".xml") || n.ends_with(".rels") || n.ends_with(".vml") {
            let t = String::from_utf8(d).unwrap();
            crate::xml::parse(t.as_bytes()).unwrap_or_else(|e| panic!("{n}: {e}"));
            texts.insert(n.clone(), t);
        }
    }
    let ct = crate::xml::parse(texts["[Content_Types].xml"].as_bytes()).unwrap();
    for o in ct.kids("Override") {
        let p = o.attr("PartName").unwrap().trim_start_matches('/');
        assert!(names.iter().any(|x| x == p), "override for missing {p}");
    }
    for (n, t) in &texts {
        if !n.ends_with(".rels") {
            continue;
        }
        let src = n.replace("_rels/", "").trim_end_matches(".rels").to_string();
        let dir = crate::package::split_dir(&src).0;
        for r in crate::xml::parse(t.as_bytes()).unwrap().kids("Relationship") {
            if r.attr("TargetMode") == Some("External") {
                continue;
            }
            let target = crate::package::resolve(dir, r.attr("Target").unwrap());
            assert!(names.contains(&target), "{n}: dangling {target}");
        }
    }
    // Newer functions carry the _xlfn. prefix in the file.
    assert!(texts["xl/worksheets/sheet1.xml"].contains("_xlfn.XLOOKUP("));
    assert!(texts["xl/worksheets/sheet1.xml"].contains("cm=\"1\""));
    assert!(texts["xl/workbook.xml"].contains("_xlfn.XLOOKUP("));
    assert!(texts["xl/workbook.xml"].contains("fullCalcOnLoad"));
}

#[test]
fn iterative_calculation_settings_roundtrip() {
    let calc_pr = |wb: &Workbook| {
        let bytes = write_xlsx(wb).unwrap();
        let mut pkg = Package::open(&bytes).unwrap();
        let book = pkg.read_xml("xl/workbook.xml").unwrap().unwrap();
        let c = book.child("calcPr").unwrap();
        let attrs = ["calcMode", "iterate", "iterateCount", "iterateDelta", "fullPrecision"].map(|a| c.attr(a).map(str::to_string));
        (attrs, read_xlsx(&bytes).unwrap().0.calc)
    };
    let s = |v: &str| Some(v.to_string());
    // Defaults: nothing written.
    let mut wb = Workbook::new();
    assert_eq!(calc_pr(&wb), ([None, None, None, None, None], CalcSettings::default()));
    // On: the switch and both limits, beside the other calculation settings.
    wb.calc.mode = CalcMode::Manual;
    wb.calc.iterative = true;
    wb.calc.precision_as_displayed = true;
    let (attrs, back) = calc_pr(&wb);
    assert_eq!(attrs, [s("manual"), s("1"), s("100"), s("0.001"), s("0")]);
    assert_eq!(back, wb.calc);
    // Off: limits that differ from the defaults are kept for when it is switched on again.
    wb.calc.iterative = false;
    wb.calc.set_max_iterations(250);
    wb.calc.set_max_change(0.0005);
    let (attrs, back) = calc_pr(&wb);
    assert_eq!(attrs, [s("manual"), None, s("250"), s("0.0005"), s("0")]);
    assert_eq!(back, wb.calc);
    // Out-of-range values in a file are kept within Excel's limits.
    let xml = |attrs: &str| {
        format!(
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets><calcPr {attrs}/></workbook>"#
        )
    };
    for (attrs, count, change) in [
        (r#"iterate="1" iterateCount="0" iterateDelta="-2""#, 1, 2.0),
        (r#"iterate="true" iterateCount="99999" iterateDelta="NaN""#, CalcSettings::MAX_ITERATIONS, CalcSettings::DEFAULT_CHANGE),
    ] {
        let wbx = xml(attrs);
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
        let ws = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#;
        let bytes = super::make_zip(&[
            ("[Content_Types].xml", super::CT.as_bytes()),
            ("_rels/.rels", super::ROOT_RELS.as_bytes()),
            ("xl/workbook.xml", wbx.as_bytes()),
            ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
            ("xl/worksheets/sheet1.xml", ws.as_bytes()),
        ]);
        let calc = read_xlsx(&bytes).unwrap().0.calc;
        assert!(calc.iterative, "{attrs}");
        assert_eq!((calc.max_iterations, calc.max_change), (count, change), "{attrs}");
    }
}

#[test]
fn show_objects_roundtrip() {
    let saved = |wb: &Workbook| {
        let bytes = write_xlsx(wb).unwrap();
        let mut pkg = Package::open(&bytes).unwrap();
        let book = pkg.read_xml("xl/workbook.xml").unwrap().unwrap();
        let attr = book.child("workbookPr").and_then(|p| p.attr("showObjects")).map(str::to_string);
        (attr, read_xlsx(&bytes).unwrap().0.show_objects)
    };
    // Default: nothing written, All read back.
    let mut wb = Workbook::new();
    assert_eq!(saved(&wb), (None, ShowObjects::All));
    // Beside date1904, which shares the element.
    wb.date_system = DateSystem::D1904;
    for (show, attr) in [(ShowObjects::None, "none"), (ShowObjects::Placeholders, "placeholders")] {
        wb.show_objects = show;
        assert_eq!(saved(&wb), (Some(attr.to_string()), show));
        assert_eq!(read_xlsx(&write_xlsx(&wb).unwrap()).unwrap().0.date_system, DateSystem::D1904);
    }
    // A file written elsewhere: the attribute among the others Excel writes, or absent.
    let book = |pr: &str| {
        let wbx = format!(
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">{pr}<sheets><sheet name="A" sheetId="1" r:id="rId1"/></sheets></workbook>"#
        );
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#;
        let ws = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#;
        let bytes = super::make_zip(&[
            ("[Content_Types].xml", super::CT.as_bytes()),
            ("_rels/.rels", super::ROOT_RELS.as_bytes()),
            ("xl/workbook.xml", wbx.as_bytes()),
            ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
            ("xl/worksheets/sheet1.xml", ws.as_bytes()),
        ]);
        read_xlsx(&bytes).unwrap().0
    };
    let hidden = book(r#"<workbookPr showObjects="none" defaultThemeVersion="166925"/>"#);
    assert_eq!(hidden.show_objects, ShowObjects::None);
    assert_eq!(saved(&hidden).0.as_deref(), Some("none"));
    for pr in [r#"<workbookPr defaultThemeVersion="166925"/>"#, "", r#"<workbookPr showObjects="bogus"/>"#] {
        let wb = book(pr);
        assert_eq!(wb.show_objects, ShowObjects::All, "{pr}");
        assert_eq!(saved(&wb).0, None, "{pr}: a file without the attribute stays without it");
    }
}

#[test]
fn empty_workbook_roundtrip() {
    let wb = Workbook::new();
    let bytes = write_xlsx(&wb).unwrap();
    let (back, rep) = read_xlsx(&bytes).unwrap();
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    assert_eq!(back.sheets.len(), 1);
    assert_eq!(back.sheets[0].name, "Sheet1");
    assert_eq!(back.styles.len(), 1);
    assert_eq!(back.sheets[0].default_col_width, 64.0);
    assert_eq!(back.sheets[0].default_row_height, 20.0);
    assert!(crate::sniff(&bytes) == crate::Format::Xlsx);
}

#[test]
fn cell_text_carriage_returns_survive_xlsx_roundtrip() {
    let mut wb = Workbook::new();
    let values = [("A1", "Line 1\r\nLine 2"), ("A2", "Part A\rPart B"), ("A3", "Item 3\nItem 4"), ("A4", "literal _x000D_ marker")];
    let sheet = wb.sheet_mut(0).unwrap();
    for (address, text) in values {
        sheet.set_value(at(address), Value::text(text));
    }
    let mut formula = Cell::formula(Formula::new("A1"));
    formula.value = Value::text("Cached\r\ntext");
    sheet.cells.set(at("B1"), formula);
    sheet.comments.insert(
        at("A1"),
        Comment {
            author: "Tester".into(),
            text: "Note\r\ntext".into(),
            replies: vec![],
            threaded: false,
            resolved: false,
            visible: false,
            dates: vec![],
            note_box: None,
        },
    );

    let bytes = write_xlsx(&wb).unwrap();
    let mut package = Package::open(&bytes).unwrap();
    let shared_strings = String::from_utf8(package.read("xl/sharedStrings.xml").unwrap().unwrap()).unwrap();
    assert!(shared_strings.contains("Line 1_x000D_\nLine 2"), "CR must be encoded before XML parsing");
    assert!(shared_strings.contains("Part A_x000D_Part B"));
    assert!(!shared_strings.contains('\r'), "raw CR is normalized to LF by XML parsers");
    let sheet_xml = String::from_utf8(package.read("xl/worksheets/sheet1.xml").unwrap().unwrap()).unwrap();
    assert!(sheet_xml.contains("Cached_x000D_\ntext"), "formula caches also use ST_Xstring");

    let (back, _) = read_xlsx(&bytes).unwrap();
    for (address, text) in values {
        assert_eq!(back.sheets[0].value(at(address)), Value::text(text), "{address} changed on save/reopen");
    }
    assert_eq!(back.sheets[0].value(at("B1")), Value::text("Cached\r\ntext"));
    assert_eq!(back.sheets[0].comments.get(&at("A1")).map(|c| c.text.as_str()), Some("Note\r\ntext"));
}

#[test]
fn writing_repairs_invalid_models() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    s.visibility = Visibility::Hidden;
    // A table whose header cells disagree with its columns, with duplicate column names.
    s.set_value(at("A1"), Value::Number(1.0));
    s.tables.push(Table {
        id: 1,
        name: "1bad name".into(),
        range: rr("A1:C3"),
        header_row: true,
        totals_row: false,
        columns: vec![
            TableColumn { name: "X".into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None },
            TableColumn { name: "x".into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None },
        ],
        style: String::new(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    s.cells.set(at("B2"), Cell { value: Value::Number(1.0), formula: None, style: StyleId(9999), py: None });
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let s = back.sheet(0).unwrap();
    assert_eq!(s.visibility, Visibility::Visible, "at least one sheet stays visible");
    let names: Vec<&str> = s.tables[0].columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["X", "x2", "Column3"]);
    assert_eq!(s.value(at("A1")), Value::text("X"));
    assert_eq!(s.tables[0].name, "Table1");
    assert_eq!(s.cell(at("B2")).unwrap().style, StyleId::DEFAULT);
}

#[test]
fn vba_project_roundtrip() {
    // A VBA project blob is preserved byte-identically through write and read, and the package
    // carries the macro-enabled content type plus the vbaProject relationship.
    let vba = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1 fake OLE2 vbaProject".to_vec();
    let mut wb = Workbook::new();
    wb.vba_project = Some(vba.clone().into());
    let bytes = write_xlsx(&wb).unwrap();
    assert_eq!(write_xlsx(&wb).unwrap(), bytes, "output is deterministic");

    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
    let names: Vec<String> = (0..z.len()).map(|i| z.name_for_index(i).unwrap().to_string()).collect();
    assert!(names.iter().any(|x| x == "xl/vbaProject.bin"), "missing vbaProject.bin");
    let mut bin = vec![];
    z.by_name("xl/vbaProject.bin").unwrap().read_to_end(&mut bin).unwrap();
    assert_eq!(bin, vba, "vbaProject.bin written byte-identically");
    let mut ct = String::new();
    z.by_name("[Content_Types].xml").unwrap().read_to_string(&mut ct).unwrap();
    assert!(ct.contains("macroEnabled.main+xml"), "macro-enabled main content type: {ct}");
    assert!(ct.contains("/xl/vbaProject.bin"), "vbaProject content type override: {ct}");
    let mut rels = String::new();
    z.by_name("xl/_rels/workbook.xml.rels").unwrap().read_to_string(&mut rels).unwrap();
    assert!(rels.contains("relationships/vbaProject"), "vbaProject relationship: {rels}");

    let (back, rep) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.vba_project.as_deref(), Some(&vba), "vbaProject survives the read");
    assert!(rep.warnings.iter().any(|w| w.contains("not executed")), "{:?}", rep.warnings);

    // A workbook without macros stays macro-free and free of VBA warnings.
    let (plain, rp) = read_xlsx(&write_xlsx(&Workbook::new()).unwrap()).unwrap();
    assert_eq!(plain.vba_project, None);
    assert!(rp.warnings.iter().all(|w| !w.contains("VBA")), "{:?}", rp.warnings);
}

#[test]
fn vba_project_read_from_foreign_file() {
    // An .xlsm built by another producer is captured on read (not dropped with a plain warning).
    let vba: &[u8] = b"PK\x03\x04-not-really-but-opaque-bytes";
    let bytes = super::minimal(
        "<sheetData/>",
        &[("xl/vbaProject.bin", "ignored-as-text")],
        r#"<Relationship Id="rId9" Type="http://schemas.microsoft.com/office/2006/relationships/vbaProject" Target="vbaProject.bin"/>"#,
        "",
    );
    let (wb, rep) = read_xlsx(&bytes).unwrap();
    assert!(wb.vba_project.is_some(), "VBA blob captured");
    assert!(rep.warnings.iter().any(|w| w.contains("not executed")), "{:?}", rep.warnings);
    let _ = vba;
}

fn sheet1_xml(bytes: &[u8]) -> (String, bool) {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    z.by_name("xl/worksheets/sheet1.xml").unwrap().read_to_string(&mut s).unwrap();
    let metadata = z.by_name("xl/metadata.xml").is_ok();
    (s, metadata)
}

/// Excel refuses to open a file whose cached values include `#SPILL!` or `#CALC!`.
#[test]
fn spill_and_calc_errors_are_cached_as_value_errors() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    s.set_value(at("A1"), Value::Number(1.0));
    s.cells.set(at("C1"), Cell { value: Value::Error(CellError::Spill), ..Cell::formula(Formula::new("A1:A3*2")) });
    s.cells.set(at("C2"), Cell { value: Value::Error(CellError::Calc), ..Cell::formula(Formula::new("FILTER(A1:A3,A1:A3>5)")) });
    s.cells.set(at("C3"), Cell { value: Value::Error(CellError::NA), ..Cell::formula(Formula::new("NA()")) });
    let bytes = write_xlsx(&wb).unwrap();
    let (xml, _) = sheet1_xml(&bytes);
    assert!(!xml.contains("#SPILL!") && !xml.contains("#CALC!"), "{xml}");
    let mut pkg = Package::open(&bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    for (address, text, value) in [("C1", "A1:A3*2", "#VALUE!"), ("C2", "_xlfn._xlws.FILTER(A1:A3,A1:A3>5)", "#VALUE!"), ("C3", "NA()", "#N/A")] {
        let cell = sheet.child("sheetData").unwrap().kids("row").flat_map(|row| row.kids("c")).find(|cell| cell.attr("r") == Some(address)).unwrap();
        assert_eq!(cell.attr("t"), Some("e"));
        assert_eq!(cell.child("f").unwrap().text, text);
        assert_eq!(cell.child("v").unwrap().text, value);
    }
}

/// `CT_IconSet` needs one `cfvo` per icon (at least two); a rule without stored thresholds
/// gets Excel's defaults, one without the lower bound gets it prepended (#235).
#[test]
fn icon_sets_always_write_one_cfvo_per_icon() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    for (i, (set, thresholds)) in [
        ("3Arrows", vec![]),
        ("4Rating", vec![CfValueKind::Number(2.0), CfValueKind::Number(5.0), CfValueKind::Number(8.0)]),
        ("9Bogus", vec![CfValueKind::Percent(50.0)]),
    ]
    .into_iter()
    .enumerate()
    {
        s.cond_formats.push(CondFormat {
            ranges: vec![rr(&format!("A{}", i + 1))],
            rule: CfRule::IconSet { set: set.into(), thresholds, reverse: false, show_value: true },
            priority: i as u32 + 1,
            stop_if_true: false,
        });
    }
    let bytes = write_xlsx(&wb).unwrap();
    let (xml, _) = sheet1_xml(&bytes);
    assert!(
        xml.contains(
            "<iconSet iconSet=\"3Arrows\"><cfvo type=\"percent\" val=\"0\"/><cfvo type=\"percent\" val=\"33\"/><cfvo type=\"percent\" val=\"67\"/></iconSet>"
        ),
        "{xml}"
    );
    assert!(
        xml.contains("<iconSet iconSet=\"4Rating\"><cfvo type=\"percent\" val=\"0\"/><cfvo type=\"num\" val=\"2\"/><cfvo type=\"num\" val=\"5\"/><cfvo type=\"num\" val=\"8\"/></iconSet>"),
        "{xml}"
    );
    assert!(xml.contains("<iconSet iconSet=\"9Bogus\"><cfvo type=\"percent\" val=\"0\"/>"), "{xml}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    let rules: Vec<usize> = back
        .sheet(0)
        .unwrap()
        .cond_formats
        .iter()
        .map(|cf| match &cf.rule {
            CfRule::IconSet { thresholds, .. } => thresholds.len(),
            _ => 0,
        })
        .collect();
    assert_eq!(rules, [3, 4, 5]);
}

const X14_CF_SHEET: &str = r#"<sheetData/>
  <conditionalFormatting sqref="B1:B5"><cfRule type="dataBar" priority="2"><dataBar><cfvo type="min"/><cfvo type="max"/><color rgb="FF638EC6"/></dataBar><extLst><ext uri="{B025F937-C7B1-47D3-B67F-A62EFF666E3E}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:id>{00000000-0000-0000-0000-00000000000A}</x14:id></ext></extLst></cfRule></conditionalFormatting>
  <extLst><ext uri="{78C0D931-6437-407d-A8EE-F0AAD7539E65}" xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"><x14:conditionalFormattings>
    <x14:conditionalFormatting xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main">
      <x14:cfRule type="cellIs" priority="3" operator="between" id="{00000000-0000-0000-0000-000000000001}"><xm:f>Lists!$A$1</xm:f><xm:f>Lists!$A$2</xm:f><x14:dxf><font><b/><color rgb="FF4F5051"/></font><numFmt numFmtId="170" formatCode="0.0%"/><fill><patternFill><bgColor rgb="FFFFC7CE"/></patternFill></fill><border><left style="thin"><color rgb="FF000000"/></left></border></x14:dxf></x14:cfRule>
      <x14:cfRule type="expression" priority="1" stopIfTrue="1" id="{00000000-0000-0000-0000-000000000002}"><xm:f>A1=Lists!$B$1</xm:f><x14:dxf><fill><patternFill><bgColor rgb="FFFFEB9C"/></patternFill></fill></x14:dxf></x14:cfRule>
      <xm:sqref>A1:A10 C1</xm:sqref></x14:conditionalFormatting>
    <x14:conditionalFormatting xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><x14:cfRule type="dataBar" id="{00000000-0000-0000-0000-00000000000A}"><x14:dataBar minLength="0" maxLength="100" gradient="0"><x14:cfvo type="autoMin"/><x14:cfvo type="autoMax"/></x14:dataBar></x14:cfRule><xm:sqref>B1:B5</xm:sqref></x14:conditionalFormatting>
    <x14:conditionalFormatting xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><x14:cfRule type="iconSet" priority="4" id="{00000000-0000-0000-0000-000000000003}"><x14:iconSet iconSet="3Stars"><x14:cfvo type="percent"><xm:f>0</xm:f></x14:cfvo><x14:cfvo type="percent"><xm:f>33</xm:f></x14:cfvo><x14:cfvo type="percent"><xm:f>67</xm:f></x14:cfvo></x14:iconSet></x14:cfRule><xm:sqref>D1:D3</xm:sqref></x14:conditionalFormatting>
  </x14:conditionalFormattings></ext></extLst>"#;

/// Rules in the `x14:conditionalFormattings` extension (such as those referring to other sheets)
/// carry `xm:f` formulas and an inline `x14:dxf`; their priorities interleave with the main
/// section's. A data bar's linked extension adds no rule, only the bar's solid fill (its other
/// settings are skipped with a warning); x14 icon sets are skipped with a warning.
#[test]
fn x14_conditional_formats_are_read() {
    let (wb, rep) = read_xlsx(&super::minimal(X14_CF_SHEET, &[], "", "")).unwrap();
    let mut cfs = wb.sheet(0).unwrap().cond_formats.clone();
    cfs.sort_by_key(|cf| cf.priority);
    assert_eq!(cfs.iter().map(|cf| cf.priority).collect::<Vec<_>>(), [1, 2, 3], "{cfs:?}");
    let [expr, bar, cell] = &cfs[..] else { panic!("{cfs:?}") };
    assert_eq!(expr.ranges, [rr("A1:A10"), rr("C1")]);
    assert!(expr.stop_if_true);
    match &expr.rule {
        CfRule::Expression { formula, style } => {
            assert_eq!(formula, "A1=Lists!$B$1");
            assert_eq!(style.fill, Fill::solid(Color::Rgb(0xFFEB9C)));
        }
        r => panic!("{r:?}"),
    }
    assert!(matches!(bar.rule, CfRule::DataBar { gradient: false, .. }), "{bar:?}");
    assert_eq!(bar.ranges, [rr("B1:B5")]);
    match &cell.rule {
        CfRule::CellIs { op, a, b, style } => {
            assert_eq!((*op, a.as_str(), b.as_deref()), (CfOperator::Between, "Lists!$A$1", Some("Lists!$A$2")));
            assert!(style.font.bold);
            assert_eq!(style.font.color, Color::Rgb(0x4F5051));
            assert_eq!(style.num_fmt.as_str(), "0.0%");
            assert_eq!(style.fill, Fill::solid(Color::Rgb(0xFFC7CE)));
            assert_eq!(style.borders.left.style, BorderStyle::Thin);
        }
        r => panic!("{r:?}"),
    }
    // Unsupported x14 kinds still warn, by name; nothing else does.
    assert!(rep.warnings.iter().any(|w| w.contains("\"dataBar\"")), "{:?}", rep.warnings);
    assert!(rep.warnings.iter().any(|w| w.contains("\"iconSet\"")), "{:?}", rep.warnings);
    assert_eq!(rep.warnings.iter().filter(|w| w.contains("conditional format")).count(), 2, "{:?}", rep.warnings);
}

/// A rule referring to another sheet is written in the x14 extension (as the LibreOffice-saved
/// file that motivated this places it), ahead of the sparklines in the one `extLst`; other rules
/// stay in the main section, and so does a rule with a 3-D span, which MS-XLSX 2.6.27 forbids in
/// `xm:f`.
#[test]
fn cross_sheet_rules_are_written_in_the_x14_extension() {
    let mut wb = Workbook::new();
    wb.sheets.push(Arc::new(Sheet::new("Vehicle Range")));
    let grey = Style { font: Font { color: Color::Rgb(0x4F5051), ..Style::default().font }, num_fmt: NumFmt::new("0.0%"), ..Style::default() };
    let s = wb.sheet_mut(0).unwrap();
    let rules = [
        (CfRule::Expression { formula: "E1>Sheet1!$B$1".into(), style: Box::new(grey.clone()) }, false),
        (CfRule::CellIs { op: CfOperator::Equal, a: "'Vehicle Range'!$A$8".into(), b: None, style: Box::new(grey.clone()) }, true),
        (CfRule::CellIs { op: CfOperator::Greater, a: "5".into(), b: None, style: Box::new(grey.clone()) }, false),
        (CfRule::Expression { formula: "SUM('Sheet1:Vehicle Range'!$A$1)>0".into(), style: Box::new(grey) }, false),
    ];
    for (i, (rule, stop_if_true)) in rules.into_iter().enumerate() {
        s.cond_formats.push(CondFormat { ranges: vec![rr("E1:E20")], rule, priority: i as u32 + 1, stop_if_true });
    }
    s.sparklines.push(Sparkline { cell: at("J2"), source: "B2:B3".into(), kind: SparklineKind::Line, color: Color::Auto, markers: false });
    let bytes = write_xlsx(&wb).unwrap();
    let (xml, _) = sheet1_xml(&bytes);
    let (main, ext) = xml.split_once("<extLst>").unwrap();
    assert_eq!(main.matches("<cfRule ").count(), 3, "{main}");
    assert!(!main.contains("$A$8"), "{main}");
    assert!(main.contains("<formula>E1&gt;Sheet1!$B$1</formula>"), "{main}");
    assert!(main.contains("<formula>SUM('Sheet1:Vehicle Range'!$A$1)&gt;0</formula>"), "{main}");
    assert_eq!(ext.matches("<x14:cfRule ").count(), 1, "{ext}");
    assert_eq!(ext.matches("<extLst>").count(), 0, "one extLst: {ext}");
    let cf = ext.find("{78C0D931-6437-407d-A8EE-F0AAD7539E65}").unwrap();
    assert!(cf < ext.find("{05C60535-1F16-4fd2-B633-F4F36F0B64E0}").unwrap(), "conditional formats before sparklines: {ext}");
    assert!(ext.contains("xmlns:x14=\"http://schemas.microsoft.com/office/spreadsheetml/2009/9/main\""), "{ext}");
    assert!(ext.contains("<x14:conditionalFormatting xmlns:xm=\"http://schemas.microsoft.com/office/excel/2006/main\">"), "{ext}");
    assert!(ext.contains("<x14:cfRule type=\"cellIs\" priority=\"2\" stopIfTrue=\"1\" operator=\"equal\" id=\"{"), "{ext}");
    assert!(ext.contains("<xm:f>'Vehicle Range'!$A$8</xm:f><x14:dxf><font><color rgb=\"FF4F5051\"/></font><numFmt numFmtId=\""), "{ext}");
    assert!(ext.contains("</x14:dxf></x14:cfRule><xm:sqref>E1:E20</xm:sqref></x14:conditionalFormatting>"), "{ext}");
    // The inline number format's id is the one `styles.xml` declares for its code.
    let id = ext.split("<numFmt numFmtId=\"").nth(1).and_then(|r| r.split('"').next()).unwrap();
    let mut styles = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().by_name("xl/styles.xml").unwrap().read_to_string(&mut styles).unwrap();
    assert!(styles.contains(&format!("<numFmt numFmtId=\"{id}\" formatCode=\"0.0%\"/>")), "{styles}");

    let (back, rep) = read_xlsx(&bytes).unwrap();
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    let mut got = back.sheet(0).unwrap().cond_formats.clone();
    got.sort_by_key(|cf| cf.priority);
    assert_eq!(got, wb.sheet(0).unwrap().cond_formats);
}

/// A differential format's protection is written after its alignment and border (the CT_Dxf
/// order: font, numFmt, fill, alignment, border, protection), both inline in an `x14:dxf` and in `styles.xml`, and reads back.
#[test]
fn dxf_protection_is_written_in_schema_order() {
    let mut wb = Workbook::new();
    wb.sheets.push(Arc::new(Sheet::new("Other")));
    let mut st = Style { fill: Fill::solid(Color::Rgb(0xFFC7CE)), ..Style::default() };
    st.align.h = HAlign::Center;
    st.protection = gridcraft_model::style::Protection { locked: false, hidden: true };
    st.borders.left.style = BorderStyle::Thin;
    let s = wb.sheet_mut(0).unwrap();
    let rules = [
        CfRule::Expression { formula: "A1=Other!$A$1".into(), style: Box::new(st.clone()) },
        CfRule::Expression { formula: "A1>5".into(), style: Box::new(st) },
    ];
    for (i, rule) in rules.into_iter().enumerate() {
        s.cond_formats.push(CondFormat { ranges: vec![rr("A1:A5")], rule, priority: i as u32 + 1, stop_if_true: false });
    }
    let bytes = write_xlsx(&wb).unwrap();
    let (xml, _) = sheet1_xml(&bytes);
    let mut styles = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().by_name("xl/styles.xml").unwrap().read_to_string(&mut styles).unwrap();
    let dxfs = &styles[styles.find("<dxfs").unwrap()..];
    for part in [xml.as_str(), dxfs] {
        let p = part.find("<protection locked=\"0\" hidden=\"1\"/>").unwrap_or_else(|| panic!("{part}"));
        assert!(part.find("<alignment").unwrap() < part.find("<border>").unwrap() && part.find("<border>").unwrap() < p, "{part}");
    }
    let (back, rep) = read_xlsx(&bytes).unwrap();
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    let mut got = back.sheet(0).unwrap().cond_formats.clone();
    got.sort_by_key(|cf| cf.priority);
    assert_eq!(got, wb.sheet(0).unwrap().cond_formats);
}

/// Files have no `@` operator. An `@` that legacy Excel implies is written without it in a
/// legacy formula (no dynamic-array metadata); any other `@x` is written as `_xlfn.SINGLE(x)`
/// in a dynamic-array formula. Both read back as written in the model.
#[test]
fn implicit_intersection_is_written_as_legacy_or_single() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    for (r, n) in [(0, 1.0), (1, 3.0), (2, 5.0)] {
        s.set_value(CellRef::new(r, 0), Value::Number(n));
    }
    s.cells.set(at("D3"), Cell { value: Value::Number(10.0), ..Cell::formula(Formula::new("@A1:A3*2")) });
    s.cells.set(at("D4"), Cell { value: Value::Number(2.0), ..Cell::formula(Formula::new("A1*2")) });
    s.cells.set(at("D5"), Cell { value: Value::Number(2.0), ..Cell::formula(Formula::new("@(A1:A3*2)")) });
    let bytes = write_xlsx(&wb).unwrap();
    let (xml, metadata) = sheet1_xml(&bytes);
    assert!(xml.contains("<c r=\"D3\"><f>A1:A3*2</f><v>10</v>"), "{xml}");
    assert!(xml.contains("<c r=\"D4\"><f>A1*2</f>"), "{xml}");
    assert!(xml.contains("<c r=\"D5\" cm=\"1\"><f t=\"array\" ref=\"D5\">_xlfn.SINGLE(A1:A3*2)</f><v>2</v>"), "{xml}");
    assert!(!xml.contains('@'), "{xml}");
    assert!(metadata);
    let (back, _) = read_xlsx(&bytes).unwrap();
    let sh = back.sheet(0).unwrap();
    let text = |a: &str| sh.cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    assert_eq!(text("D3").as_deref(), Some("@A1:A3*2"));
    assert_eq!(text("D4").as_deref(), Some("A1*2"));
    assert_eq!(text("D5").as_deref(), Some("@(A1:A3*2)"));
    assert_eq!(sh.cell(at("D5")).and_then(|c| c.formula.as_ref()).and_then(|f| f.array), None);
}

/// A legacy formula read from a file (shown with Excel's implicit `@`) is saved as it was read,
/// without dynamic-array metadata; a dynamic-array formula keeps its metadata.
#[test]
fn legacy_and_dynamic_formulas_keep_their_kind() {
    let body = r#"<sheetData>
      <row r="1"><c r="A1"><v>1</v></c></row>
      <row r="2"><c r="A2"><v>2</v></c><c r="B2"><f>TANH(A1:A3)+A1:A3</f><v>4</v></c><c r="C2" cm="1"><f t="array" ref="C2:C4">TANH(A1:A3)</f><v>0.76</v></c><c r="D2"><f t="array" ref="D2:D4">TANH(A1:A3)</f><v>0.76</v></c></row>
      <row r="3"><c r="A3"><v>3</v></c><c r="B3"><f>SUM(A1:A3)</f><v>6</v></c></row>
    </sheetData>"#;
    let (wb, _) = read_xlsx(&super::minimal(body, &[], "", "")).unwrap();
    let text = |wb: &Workbook, a: &str| wb.sheet(0).unwrap().cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| (f.text.clone(), f.array)).unwrap();
    assert_eq!(text(&wb, "B2"), ("TANH(@A1:A3)+@A1:A3".to_string(), None));
    let bytes = write_xlsx(&wb).unwrap();
    let (xml, _) = sheet1_xml(&bytes);
    assert!(xml.contains("<c r=\"B2\"><f>TANH(A1:A3)+A1:A3</f>"), "{xml}");
    assert!(xml.contains("<c r=\"B3\"><f>SUM(A1:A3)</f>"), "{xml}");
    // Uncalculated, the dynamic array keeps the area the file says it spilled over.
    assert!(xml.contains("<c r=\"C2\" cm=\"1\"><f t=\"array\" ref=\"C2:C4\">TANH(A1:A3)</f>"), "{xml}");
    assert!(xml.contains("<c r=\"D2\"><f t=\"array\" ref=\"D2:D4\">TANH(A1:A3)</f>"), "{xml}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    for (a, dynamic) in [("B2", false), ("B3", false), ("C2", true), ("D2", false)] {
        assert_eq!(text(&back, a), text(&wb, a), "{a}");
        let formula = back.sheet(0).unwrap().cell(at(a)).unwrap().formula.as_ref().unwrap();
        assert_eq!(formula.dynamic, dynamic, "{a}: dynamic-array metadata survives saving");
    }
}

/// The `<f>` text and `cm` attribute of a cell in a written package's first sheet.
fn written_formula(bytes: &[u8], a: &str) -> (Option<String>, Option<String>) {
    let mut pkg = Package::open(bytes).unwrap();
    let sheet = pkg.read_xml("xl/worksheets/sheet1.xml").unwrap().unwrap();
    let c = sheet.child("sheetData").unwrap().kids("row").flat_map(|r| r.kids("c")).find(|c| c.attr("r") == Some(a)).unwrap();
    (c.child("f").map(|f| f.text.clone()), c.attr("cm").map(str::to_string))
}

/// A long operator chain is analysed in full (its ranges are intersected), and so are 64 nested
/// functions with parenthesised arguments (128 levels), 200 nested functions and a long name used
/// several times. A legacy formula the analysis cannot finish (names too large to analyse) reads
/// with one whole-formula `@`, so it shows one value as in legacy Excel, and saves as read.
/// Saving writes none of them as `_xlfn.SINGLE`, which pre-dynamic-array Excel shows as #NAME?,
/// nor with `cm`. (`legacy_array_functions.rs` in the engine checks the same on a calculated
/// workbook.)
#[test]
fn long_and_deeply_nested_legacy_formulas_never_gain_single() {
    let chain = (1..=70).map(|r| format!("B{r}")).collect::<Vec<_>>().join("+");
    let nested = |depth: usize, inner: &str| format!("{}{inner}{}", "ABS(".repeat(depth), ")".repeat(depth));
    let parens = |depth: usize, inner: &str| format!("{}{inner}{}", "ABS((".repeat(depth), "))".repeat(depth));
    let big = vec!["Data!$B$1"; 399].join("+");
    let uses = format!("{}A1:A3", "Big+".repeat(6));
    let parts = (1..=30).map(|i| format!("Part_{i}")).collect::<Vec<_>>().join("+");
    let body = format!(
        r#"<sheetData>
      <row r="1"><c r="A1"><v>1</v></c></row>
      <row r="2"><c r="A2"><v>2</v></c><c r="D2"><f>{chain}</f></c></row>
      <row r="3"><c r="A3"><v>3</v></c><c r="D3"><f>A1:A3+{chain}</f></c></row>
      <row r="4"><c r="D4"><f>{}</f></c></row>
      <row r="5"><c r="D5"><f>{uses}</f></c></row>
      <row r="6"><c r="D6"><f>{}</f></c></row>
      <row r="7"><c r="D7"><f>{}</f></c></row>
      <row r="8"><c r="D8"><f>{parts}</f></c></row>
    </sheetData>"#,
        nested(64, "A1:A3"),
        nested(200, "A1:A3"),
        parens(64, "A1:A3"),
    );
    let part = format!("Data!$A$1:$A$3+{}", vec!["Data!$B$1"; 599].join("+"));
    let defs: String = (1..=30).map(|i| format!(r#"<definedName name="Part_{i}">{part}</definedName>"#)).collect();
    let names = format!(r#"<definedNames><definedName name="Big">{big}</definedName>{defs}</definedNames>"#);
    let (wb, _) = read_xlsx(&super::minimal(&body, &[], "", &names)).unwrap();
    let text = |wb: &Workbook, a: &str| wb.sheet(0).unwrap().cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap();
    assert_eq!(text(&wb, "D2"), chain);
    assert_eq!(text(&wb, "D3"), format!("@A1:A3+{chain}"));
    assert_eq!(text(&wb, "D4"), nested(64, "@A1:A3"));
    assert_eq!(text(&wb, "D5"), format!("{}@A1:A3", "Big+".repeat(6)));
    assert_eq!(text(&wb, "D6"), nested(200, "@A1:A3"));
    assert_eq!(text(&wb, "D7"), parens(64, "@A1:A3"));
    assert_eq!(text(&wb, "D8"), format!("@({parts})"));
    let bytes = write_xlsx(&wb).unwrap();
    assert_eq!(written_formula(&bytes, "D2"), (Some(chain.clone()), None));
    assert_eq!(written_formula(&bytes, "D3"), (Some(format!("A1:A3+{chain}")), None));
    assert_eq!(written_formula(&bytes, "D4"), (Some(nested(64, "A1:A3")), None));
    assert_eq!(written_formula(&bytes, "D5"), (Some(uses), None));
    assert_eq!(written_formula(&bytes, "D6"), (Some(nested(200, "A1:A3")), None));
    assert_eq!(written_formula(&bytes, "D7"), (Some(parens(64, "A1:A3")), None));
    assert_eq!(written_formula(&bytes, "D8"), (Some(parts), None));
    let (xml, _) = sheet1_xml(&bytes);
    assert!(!xml.contains("SINGLE") && !xml.contains("t=\"array\""), "{xml}");
}

/// `@` of a union keeps the union's parentheses when saved, with or without `_xlfn.SINGLE`:
/// without them the comma separates function arguments or the text does not parse. Redundant
/// parentheses under `@` survive too, as only those the reader adds back are dropped.
#[test]
fn implicit_intersection_keeps_its_parentheses() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    let formulas = [
        "@(A1:A2,C1:C2)+1",
        "SQRT(@(A1:A2,C1:C2))",
        "@(A1:A2,C1:C2)+A1:A3",
        "@((A1:A2,C1:C2))",
        "SQRT(@((A1:A2,C1:C2)))",
        "@(((A1:A2,C1:C2)))",
        "@((A1:A3*2))",
        "@(A1:A3)",
        "@(A1:A3*2)",
        "@(-A1:A3)",
    ];
    for (i, f) in formulas.iter().enumerate() {
        s.cells.set(CellRef::new(4 + i as u32, 4), Cell::formula(Formula::new(f)));
    }
    let bytes = write_xlsx(&wb).unwrap();
    let (back, _) = read_xlsx(&bytes).unwrap();
    for (i, f) in formulas.iter().enumerate() {
        let cell = back.sheet(0).unwrap().cell(CellRef::new(4 + i as u32, 4)).and_then(|c| c.formula.as_ref());
        assert_eq!(cell.map(|c| (c.text.as_str(), c.expr().is_some())), Some((*f, true)), "{f}");
    }
}

/// A legacy formula read from a file saves back exactly as it was read, without `cm` or
/// `_xlfn.SINGLE`: a union already in parentheses where one value is expected, a name whose
/// definition holds IF of a range (legacy Excel evaluates a name's definition as an array), and a
/// multi-valued result in the file's own parentheses. A union given to a function is intersected
/// (`SQRT((@(A1:A2,C1:C2)))`), as the function would otherwise spill it; one under an operator is
/// shown as Excel shows it. The engine's `legacy_array_functions.rs` checks the same formulas on a
/// calculated workbook, where nothing spills.
#[test]
fn legacy_unions_in_parentheses_and_names_with_if_save_as_read() {
    let body = r#"<sheetData>
      <row r="1"><c r="A1"><v>1</v></c><c r="C1"><v>4</v></c><c r="E1"><f>SQRT((A1:A2,C1:C2))</f></c></row>
      <row r="2"><c r="A2"><v>2</v></c><c r="C2"><v>9</v></c><c r="E2"><f>(A1:A2,C1:C2)+1</f></c></row>
      <row r="3"><c r="A3"><v>3</v></c><c r="E3"><f>SUM(Flag)</f></c><c r="F3"><f>SQRT(Flag)</f></c></row>
      <row r="4"><c r="E4"><f>({1,2}+1)</f></c><c r="F4"><f>{1,2}+1</f></c><c r="G4"><f>(((MYUDF(A1)*2)))</f></c></row>
    </sheetData>"#;
    let names = r#"<definedNames><definedName name="Flag">IF(Data!$A$1:$A$3&gt;1,1,0)</definedName></definedNames>"#;
    let (wb, _) = read_xlsx(&super::minimal(body, &[], "", names)).unwrap();
    let text = |a: &str| wb.sheet(0).unwrap().cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap();
    let bytes = write_xlsx(&wb).unwrap();
    let cases = [
        ("E1", "SQRT((A1:A2,C1:C2))"),
        ("E2", "(A1:A2,C1:C2)+1"),
        ("E3", "SUM(Flag)"),
        ("F3", "SQRT(Flag)"),
        ("E4", "({1,2}+1)"),
        ("F4", "{1,2}+1"),
        ("G4", "(((MYUDF(A1)*2)))"),
    ];
    let wrong: Vec<_> =
        cases.iter().map(|(a, f)| (*a, *f, written_formula(&bytes, a))).filter(|(_, f, w)| *w != (Some(f.to_string()), None)).collect();
    assert!(wrong.is_empty(), "{wrong:?}");
    assert_eq!(text("E1"), "SQRT((@(A1:A2,C1:C2)))");
    assert_eq!(text("E2"), "(A1:A2,C1:C2)+1");
    assert_eq!(text("F3"), "@SQRT(Flag)");
    let (xml, _) = sheet1_xml(&bytes);
    assert!(!xml.contains("SINGLE") && !xml.contains("cm=") && !xml.contains("t=\"array\""), "{xml}");
}

/// An implied `@` whose operand an edit has made a single cell (deleting rows under
/// `SQRT(@A1:A3)` leaves `A1:A1`, and a name's `$A$1:$A$3` becomes `$A$1:$A$1`) is the one legacy
/// Excel implies for that range: the formula still saves as a legacy one, without
/// `_xlfn.SINGLE` or `cm`.
#[test]
fn implied_intersection_made_redundant_by_an_edit_saves_legacy() {
    let body = r#"<sheetData>
      <row r="5"><c r="B5"><f>SQRT(A1:A3)</f></c><c r="C5"><f>SQRT(Rng)</f></c></row>
    </sheetData>"#;
    let names = r#"<definedNames><definedName name="Rng">Data!$A$1:$A$3</definedName></definedNames>"#;
    let (mut wb, _) = read_xlsx(&super::minimal(body, &[], "", names)).unwrap();
    let text = |wb: &Workbook, a: &str| wb.sheet(0).unwrap().cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap();
    assert_eq!(text(&wb, "B5"), "SQRT(@A1:A3)");
    assert_eq!(text(&wb, "C5"), "SQRT(@Rng)");
    // What deleting rows 2:3 leaves.
    wb.sheet_mut(0).unwrap().cells.set(at("B5"), Cell::formula(Formula::new("SQRT(@A1:A1)")));
    wb.names.iter_mut().find(|n| n.name == "Rng").unwrap().formula = "Data!$A$1:$A$1".into();
    let bytes = write_xlsx(&wb).unwrap();
    assert_eq!(written_formula(&bytes, "B5"), (Some("SQRT(A1:A1)".into()), None));
    assert_eq!(written_formula(&bytes, "C5"), (Some("SQRT(Rng)".into()), None));
    let (xml, _) = sheet1_xml(&bytes);
    assert!(!xml.contains("SINGLE") && !xml.contains("t=\"array\""), "{xml}");
}

/// Excel keeps an `@` that legacy evaluation does not imply as `_xlfn.SINGLE` inside a legacy
/// formula (Excel Online's UnionSpillIntersectionOperatorTest.xlsx, EPPlus's
/// DefinedNameRelative.xlsx): such formulas save back with it, without `cm`. The reader shows the
/// implied intersection of a union that is an operator's operand as Excel does, without `@`, and a
/// spill reference's `@` without parentheses. A typed `@` of a single value saves the same way.
#[test]
fn single_in_a_legacy_formula_saves_as_read() {
    let body = r#"<sheetData>
      <row r="5"><c r="F5"><v>1</v></c><c r="O6"><f>_xlfn.SINGLE(SUM(_xlfn.ANCHORARRAY(F5)))</f></c><c r="J6"><f>_xlfn.ANCHORARRAY(F5)%</f></c><c r="K6"><f>_xlfn.ANCHORARRAY($F$5)</f></c></row>
      <row r="25"><c r="F25"><v>1</v></c><c r="G25"><f>(F25,F26)%</f></c><c r="H25"><f>SUM((F25,F26)%)</f></c></row>
      <row r="83"><c r="F83"><f>_xlfn.SINGLE(5)</f></c><c r="G83"><f>_xlfn.SINGLE(RelativeRow)</f></c><c r="H83"><f>_xlfn.SINGLE(H12)</f></c></row>
      <row r="85"><c r="F85"><f>_xlfn.SINGLE(SUM(F25:F26))</f></c></row>
      <row r="102"><c r="F102"><f>(F25, _xlfn.SINGLE(F26))</f></c></row>
    </sheetData>"#;
    let names = r#"<definedNames><definedName name="RelativeRow">Data!$H1</definedName></definedNames>"#;
    let (mut wb, _) = read_xlsx(&super::minimal(body, &[], "", names)).unwrap();
    let text = |wb: &Workbook, a: &str| wb.sheet(0).unwrap().cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone()).unwrap();
    let shown = [
        ("O6", "@SUM(F5#)"),
        ("J6", "@F5#%"),
        ("K6", "@$F$5#"),
        ("G25", "(F25,F26)%"),
        ("H25", "SUM((F25,F26)%)"),
        ("F83", "@5"),
        ("G83", "@RelativeRow"),
        ("H83", "@H12"),
        ("F85", "@SUM(F25:F26)"),
        ("F102", "(F25,@F26)"),
    ];
    let wrong: Vec<_> = shown.iter().map(|(a, f)| (*a, *f, text(&wb, a))).filter(|(_, f, t)| t != f).collect();
    assert!(wrong.is_empty(), "{wrong:?}");
    let s = wb.sheet_mut(0).unwrap();
    s.cells.set(at("B1"), Cell::formula(Formula::new("@SQRT(A1)")));
    s.cells.set(at("B2"), Cell::formula(Formula::new("@MUNIT(3)")));
    let bytes = write_xlsx(&wb).unwrap();
    let saved = [
        ("O6", "_xlfn.SINGLE(SUM(_xlfn.ANCHORARRAY(F5)))"),
        ("J6", "_xlfn.ANCHORARRAY(F5)%"),
        ("K6", "_xlfn.ANCHORARRAY($F$5)"),
        ("G25", "(F25,F26)%"),
        ("H25", "SUM((F25,F26)%)"),
        ("F83", "_xlfn.SINGLE(5)"),
        ("G83", "_xlfn.SINGLE(RelativeRow)"),
        ("H83", "_xlfn.SINGLE(H12)"),
        ("F85", "_xlfn.SINGLE(SUM(F25:F26))"),
        ("F102", "(F25,_xlfn.SINGLE(F26))"),
        ("B1", "_xlfn.SINGLE(SQRT(A1))"),
        ("B2", "_xlfn.MUNIT(3)"),
    ];
    let wrong: Vec<_> =
        saved.iter().map(|(a, f)| (*a, *f, written_formula(&bytes, a))).filter(|(_, f, w)| *w != (Some(f.to_string()), None)).collect();
    assert!(wrong.is_empty(), "{wrong:?}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    for (a, _) in saved {
        assert_eq!(text(&back, a), text(&wb, a), "{a}");
    }
}

#[test]
fn threaded_comments_write_persons_and_linked_legacy_notes() {
    let wb = sample();
    let bytes = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&bytes).unwrap();
    let text = |pkg: &mut Package, part: &str| String::from_utf8(pkg.read(part).unwrap().unwrap()).unwrap();
    // One person per author across the workbook, ids unique.
    let persons = text(&mut pkg, "xl/persons/person.xml");
    assert_eq!(persons.matches("<person ").count(), 3, "{persons}");
    assert!(persons.contains("displayName=\"Ann &amp; Co\""));
    let ct = text(&mut pkg, "[Content_Types].xml");
    assert!(ct.contains("application/vnd.ms-excel.threadedcomments+xml") && ct.contains("application/vnd.ms-excel.person+xml"));
    let tc = text(&mut pkg, "xl/threadedComments/threadedComment1.xml");
    assert_eq!(tc.matches("<threadedComment ").count(), 4, "{tc}");
    assert_eq!(tc.matches("parentId=").count(), 2);
    assert_eq!(tc.matches("done=\"1\"").count(), 1);
    // The legacy note of each thread points at it.
    let legacy = text(&mut pkg, "xl/comments1.xml");
    for id in tc.split("id=\"{").skip(1).filter_map(|s| s.split('}').next()).take(1) {
        assert!(legacy.contains(&format!("<author>tc={{{id}}}</author>")) && legacy.contains(&format!("xr:uid=\"{{{id}}}\"")), "{legacy}");
    }
}

#[test]
fn bad_comment_dates_are_left_out() {
    let mut wb = Workbook::new();
    let s = Arc::make_mut(&mut wb.sheets[0]);
    let c = Comment {
        author: "A".into(),
        text: "t".into(),
        replies: vec![],
        threaded: true,
        resolved: false,
        visible: false,
        dates: vec!["\"/><evil".into()],
        note_box: None,
    };
    s.comments.insert(at("A1"), c);
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let c = &back.sheets[0].comments[&at("A1")];
    assert!(c.threaded && c.dates.is_empty());
}

/// #378: the saved scroll position survives a round trip, with and without frozen panes (where it
/// is the scrolling pane's `topLeftCell`).
#[test]
fn scroll_position_round_trips_with_and_without_frozen_panes() {
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().view_top_left = at("A31");
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(back.sheet(0).unwrap().view_top_left, at("A31"));

    let s = wb.sheet_mut(0).unwrap();
    s.freeze = Some((1, 0));
    s.view_top_left = at("C40");
    let bytes = write_xlsx(&wb).unwrap();
    let (xml, _) = sheet1_xml(&bytes);
    assert!(xml.contains(r#"<pane ySplit="1" topLeftCell="C40""#), "{xml}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(back.sheet(0).unwrap().view_top_left, at("C40"));
    assert_eq!(back.sheet(0).unwrap().freeze, Some((1, 0)));

    // A scroll position inside the frozen rows is written as the first scrolling row.
    wb.sheet_mut(0).unwrap().view_top_left = at("A1");
    let (xml, _) = sheet1_xml(&write_xlsx(&wb).unwrap());
    assert!(xml.contains(r#"topLeftCell="A2""#), "{xml}");
}

/// Excel stores LET/LAMBDA parameters as `_xlpm.name`; the model shows bare names.
#[test]
fn let_lambda_parameters_drop_xlpm_on_read() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    s.cells.set(at("A1"), Cell { value: Value::Number(2.0), ..Cell::formula(Formula::new("LET(x,1,x+1)")) });
    s.cells.set(at("A2"), Cell { value: Value::Number(6.0), ..Cell::formula(Formula::new("LAMBDA(a,a*2)(3)")) });
    let mut arr = Formula::new("MAP(B1:B2,LAMBDA(v,v+1))");
    arr.array = Some(rr("C1:C2"));
    s.cells.set(at("C1"), Cell::formula(arr));
    let bytes = write_xlsx(&wb).unwrap();
    let (xml, _) = sheet1_xml(&bytes);
    assert!(xml.contains("_xlfn.LET(_xlpm.x,1,_xlpm.x+1)"), "{xml}");
    assert!(xml.contains("_xlpm.v"), "{xml}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    let sh = back.sheet(0).unwrap();
    let text = |a: &str| sh.cell(at(a)).and_then(|c| c.formula.as_ref()).map(|f| f.text.clone());
    assert_eq!(text("A1").as_deref(), Some("LET(x,1,x+1)"));
    assert_eq!(text("A2").as_deref(), Some("LAMBDA(a,a*2)(3)"));
    assert_eq!(text("C1").as_deref(), Some("MAP(B1:B2,LAMBDA(v,v+1))"));
}

/// A line break in a table column name (and its header cell) survives; a
/// leading backslash in a table name is legal and kept.
#[test]
fn table_newline_column_and_backslash_name_survive() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    s.set_value(at("A1"), Value::text("Handle\nBars"));
    s.set_value(at("B1"), Value::text("Name"));
    s.set_value(at("B2"), Value::text("x"));
    let col = |n: &str| TableColumn { name: n.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None };
    s.tables.push(Table {
        id: 1,
        name: "\\_Prime.1".into(),
        range: rr("A1:B2"),
        header_row: true,
        totals_row: false,
        columns: vec![col("Handle\nBars"), col("Name")],
        style: String::new(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    s.cells.set(at("D1"), Cell::formula(Formula::new("COUNTA(\\_Prime.1[Name])")));
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let s = back.sheet(0).unwrap();
    assert_eq!(s.tables[0].name, "\\_Prime.1");
    assert_eq!(s.tables[0].columns[0].name, "Handle\nBars");
    assert_eq!(s.value(at("A1")), Value::text("Handle\nBars"));
    assert_eq!(s.cell(at("D1")).and_then(|c| c.formula.as_ref()).map(|f| f.text.as_str()), Some("COUNTA(\\_Prime.1[Name])"));
    assert!(gridcraft_formula::parse("COUNTA(\\_Prime.1[Name])").is_ok(), "backslash table name parses as a structured reference");
}

/// #4296: a table whose Total Row is turned off keeps each column's calculation in the file, as
/// Excel writes it, so turning the row back on in Excel restores a custom formula or Average.
#[test]
fn hidden_total_row_keeps_its_calculations() {
    let col = |name: &str, totals: TotalsFn, label: Option<&str>| TableColumn {
        name: name.into(),
        totals,
        totals_label: label.map(str::to_string),
        formula: None,
        unique_name: None,
        xml: None,
    };
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    s.tables.push(Table {
        id: 1,
        name: "Table1".into(),
        range: rr("A1:C3"),
        header_row: true,
        totals_row: false,
        columns: vec![
            col("Group", TotalsFn::None, Some("Total")),
            col("Amount", TotalsFn::Custom, Some("SUM(B2:B3)+10")),
            col("Mean", TotalsFn::Average, None),
        ],
        style: "TableStyleMedium2".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    let bytes = write_xlsx(&wb).unwrap();
    let mut xml = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().by_name("xl/tables/table1.xml").unwrap().read_to_string(&mut xml).unwrap();
    // Still a table without a total row: no totalsRowCount, and its range unchanged.
    assert!(xml.contains(r#"ref="A1:C3""#) && xml.contains(r#"totalsRowShown="0""#) && !xml.contains("totalsRowCount"), "{xml}");
    for part in [
        r#"name="Group" totalsRowLabel="Total"/>"#,
        r#"name="Amount" totalsRowFunction="custom"><totalsRowFormula>SUM(B2:B3)+10</totalsRowFormula></tableColumn>"#,
        r#"name="Mean" totalsRowFunction="average"/>"#,
    ] {
        assert!(xml.contains(part), "{part}: {xml}");
    }
    let (back, report) = read_xlsx(&bytes).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let t = &back.sheet(0).unwrap().tables[0];
    assert!(!t.totals_row);
    assert_eq!(t.range.a1(), "A1:C3");
    let got: Vec<_> = t.columns.iter().map(|c| (c.totals, c.totals_label.clone())).collect();
    assert_eq!(got, vec![(TotalsFn::None, Some("Total".into())), (TotalsFn::Custom, Some("SUM(B2:B3)+10".into())), (TotalsFn::Average, None)]);
    // A table that never had a total row writes no calculation.
    let mut plain = wb.clone();
    plain.sheet_mut(0).unwrap().tables[0].columns =
        vec![col("Group", TotalsFn::None, None), col("Amount", TotalsFn::None, None), col("Mean", TotalsFn::None, None)];
    let mut xml = String::new();
    zip::ZipArchive::new(std::io::Cursor::new(write_xlsx(&plain).unwrap()))
        .unwrap()
        .by_name("xl/tables/table1.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    for attr in ["totalsRowFunction", "totalsRowLabel", "totalsRowFormula"] {
        assert!(!xml.contains(attr), "{attr}: {xml}");
    }
}

/// A table that is only a header row and a totals row (Insert Table on one row, then Total
/// Row) is written like the header-only and one-data-row shapes, not dropped on save.
#[test]
fn header_and_totals_table_without_data_row_survives() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    s.tables.push(Table {
        id: 1,
        name: "Table1".into(),
        range: rr("A1:B2"),
        header_row: true,
        totals_row: true,
        columns: vec![
            TableColumn {
                name: "Name".into(),
                totals: TotalsFn::None,
                totals_label: Some("Total".into()),
                formula: None,
                unique_name: None,
                xml: None,
            },
            TableColumn { name: "Amount".into(), totals: TotalsFn::Sum, totals_label: None, formula: None, unique_name: None, xml: None },
        ],
        style: "TableStyleMedium2".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: true,
        last_col: true,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    let (back, report) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let tables = &back.sheet(0).unwrap().tables;
    assert_eq!(tables.len(), 1);
    let t = &tables[0];
    assert_eq!((t.name.as_str(), t.range.a1(), t.header_row, t.totals_row), ("Table1", "A1:B2".to_string(), true, true));
    assert_eq!((t.style.as_str(), t.banded_rows, t.first_col, t.last_col), ("TableStyleMedium2", true, true, true));
    assert_eq!(t.columns[0].totals_label.as_deref(), Some("Total"));
    assert_eq!(t.columns[1].totals, TotalsFn::Sum);
    assert_eq!(back.sheet(0).unwrap().value(at("A1")), Value::text("Name"));
}

/// Templates and macro-enabled files differ from `.xlsx` only in the workbook part's content
/// type; a template saved with the workbook type is rejected by Excel.
#[test]
fn package_kind_sets_workbook_content_type() {
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(at("A1"), Value::Number(7.0));
    for (path, kind, ct) in [
        ("a.xlsx", crate::XlsxKind::Workbook, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"),
        ("a.XLTX", crate::XlsxKind::Template, "application/vnd.openxmlformats-officedocument.spreadsheetml.template.main+xml"),
        ("a.xlsm", crate::XlsxKind::MacroWorkbook, "application/vnd.ms-excel.sheet.macroEnabled.main+xml"),
        ("dir.x/a.xltm", crate::XlsxKind::MacroTemplate, "application/vnd.ms-excel.template.macroEnabled.main+xml"),
    ] {
        assert_eq!(crate::XlsxKind::from_path(path), kind);
        let bytes = crate::write_xlsx_as(&wb, kind).unwrap();
        let mut pkg = Package::open(&bytes).unwrap();
        let types = pkg.read_xml("[Content_Types].xml").unwrap().unwrap();
        let main: Vec<_> =
            types.kids("Override").filter(|e| e.attr("PartName") == Some("/xl/workbook.xml")).filter_map(|e| e.attr("ContentType")).collect();
        assert_eq!(main, [ct], "{path}");
        // Every kind reads back.
        let (back, report) = read_xlsx(&bytes).unwrap();
        assert!(report.warnings.is_empty(), "{path}: {:?}", report.warnings);
        assert_eq!(back.sheet(0).unwrap().value(at("A1")), Value::Number(7.0));
    }
    assert_eq!(crate::XlsxKind::from_path("noext"), crate::XlsxKind::Workbook);
}

/// The workbook's default font (`fonts[0]`, the Normal style's) is what every default-formatted
/// cell shows, typed or empty; saving keeps it instead of resetting it to Calibri 11 (#805).
#[test]
fn default_font_survives_a_round_trip() {
    let styles = |font0: &str| {
        format!(
            r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="2"><font>{font0}</font><font><b/><sz val="14"/><color theme="1"/><name val="Arial"/><family val="2"/></font></fonts>
            <fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
            <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
            <cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs>
            <cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#
        )
    };
    let rels = r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#;
    let body = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>typed text</t></is></c><c r="B1" s="1"><v>1</v></c></row></sheetData>"#;
    let font0 = |bytes: &[u8]| {
        let mut pkg = Package::open(bytes).unwrap();
        let st = pkg.read_xml("xl/styles.xml").unwrap().unwrap();
        let f = st.child("fonts").unwrap().kids("font").next().unwrap().clone();
        let val = |n: &str| f.child(n).and_then(|e| e.attr("val")).map(str::to_string);
        (val("sz"), val("name"), val("family"), val("scheme"))
    };

    let file =
        super::minimal(body, &[("xl/styles.xml", &styles(r#"<sz val="10"/><color theme="1"/><name val="Arial"/><family val="2"/>"#))], rels, "");
    let (wb, _) = read_xlsx(&file).unwrap();
    let saved = write_xlsx(&wb).unwrap();
    assert_eq!(font0(&saved), (Some("10".into()), Some("Arial".into()), Some("2".into()), None));
    let (back, _) = read_xlsx(&saved).unwrap();
    let normal = &back.styles.get(StyleId::DEFAULT).font;
    assert_eq!((normal.name.as_str(), normal.size), ("Arial", 10.0), "empty cells and new input take the Normal font");
    let s = back.sheet(0).unwrap();
    assert_eq!(s.cell(at("A1")).unwrap().style, StyleId::DEFAULT);
    let b1 = &back.styles.get(s.cell(at("B1")).unwrap().style).font;
    assert!(b1.bold && b1.size == 14.0, "an explicit font is still its own: {b1:?}");

    // Excel's own Normal font follows the theme's body font; the scheme link survives too.
    let file = super::minimal(
        body,
        &[("xl/styles.xml", &styles(r#"<name val="Calibri"/><family val="2"/><color theme="1"/><sz val="12"/><scheme val="minor"/>"#))],
        rels,
        "",
    );
    let saved = write_xlsx(&read_xlsx(&file).unwrap().0).unwrap();
    assert_eq!(font0(&saved), (Some("12".into()), Some("Calibri".into()), Some("2".into()), Some("minor".into())));
}

/// Every font that follows the theme's body font keeps `<scheme val="minor"/>`, not only font 0,
/// and a body-face font without the scheme does not gain one (#2517).
#[test]
fn minor_scheme_survives_on_every_font() {
    let styles = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="3"><font><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/><scheme val="minor"/></font><font><b/><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/><scheme val="minor"/></font><font><i/><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/></font></fonts>
        <fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
        <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
        <cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/><xf numFmtId="0" fontId="2" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs>
        <cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#;
    let rels = r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#;
    let body = r#"<sheetData><row r="1"><c r="A1" s="1"><v>1</v></c><c r="B1" s="2"><v>2</v></c></row></sheetData>"#;
    let file = super::minimal(body, &[("xl/styles.xml", styles)], rels, "");
    let saved = write_xlsx(&read_xlsx(&file).unwrap().0).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let st = pkg.read_xml("xl/styles.xml").unwrap().unwrap();
    let fonts: Vec<(bool, bool, Option<String>)> = st
        .child("fonts")
        .unwrap()
        .kids("font")
        .map(|f| (f.child("b").is_some(), f.child("i").is_some(), f.child("scheme").and_then(|e| e.attr("val")).map(str::to_string)))
        .collect();
    let scheme = |b: bool, i: bool| fonts.iter().find(|f| f.0 == b && f.1 == i).map(|f| f.2.clone());
    assert_eq!(scheme(true, false), Some(Some("minor".into())), "{fonts:?}");
    assert_eq!(scheme(false, true), Some(None), "{fonts:?}");
}

fn one_font_workbook(font: &str) -> Vec<u8> {
    let styles = format!(
        r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font>{font}</font></fonts><fills count="1"><fill><patternFill patternType="none"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#
    );
    let rels = r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#;
    super::minimal("<sheetData/>", &[("xl/styles.xml", &styles)], rels, "")
}

#[test]
fn font_charset_and_family_roundtrip() {
    let file = one_font_workbook(r#"<name val="Traditional Arabic"/><family val="1"/><charset val="178"/>"#);
    let (wb, _) = read_xlsx(&file).unwrap();
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let xml = String::from_utf8(pkg.read("xl/styles.xml").unwrap().unwrap()).unwrap();
    assert!(xml.contains("<name val=\"Traditional Arabic\"/><family val=\"1\"/><charset val=\"178\"/>"), "{xml}");

    let (back, _) = read_xlsx(&saved).unwrap();
    let font = &back.styles.get(StyleId::DEFAULT).font;
    let back_saved = write_xlsx(&back).unwrap();
    let mut pkg = Package::open(&back_saved).unwrap();
    let xml = String::from_utf8(pkg.read("xl/styles.xml").unwrap().unwrap()).unwrap();
    assert!(xml.contains("<name val=\"Traditional Arabic\"/><family val=\"1\"/><charset val=\"178\"/>"), "{xml}");
    assert_eq!(font.name, "Traditional Arabic");
    assert_eq!(font.family, Some(1));
    assert_eq!(font.charset, Some(178));
}

#[test]
fn fonts_without_charset_write_unchanged() {
    let file = one_font_workbook(r#"<name val="Arial"/>"#);
    let saved = write_xlsx(&read_xlsx(&file).unwrap().0).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let xml = String::from_utf8(pkg.read("xl/styles.xml").unwrap().unwrap()).unwrap();
    assert!(xml.contains("<name val=\"Arial\"/><family val=\"2\"/>"), "{xml}");
    assert!(!xml.contains("<charset"), "{xml}");
}

#[test]
fn out_of_range_family_is_ignored() {
    let file = one_font_workbook(r#"<name val="Arial"/><family val="15"/>"#);
    let (wb, _) = read_xlsx(&file).unwrap();
    assert_eq!(wb.styles.get(StyleId::DEFAULT).font.family, None);
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let xml = String::from_utf8(pkg.read("xl/styles.xml").unwrap().unwrap()).unwrap();
    assert!(xml.contains("<name val=\"Arial\"/><family val=\"2\"/>"), "{xml}");

    let mut set_directly = wb.clone();
    let wide = set_directly.styles.derive(StyleId::DEFAULT, |st| st.font.family = Some(200));
    set_directly.sheet_mut(0).unwrap().set_value(CellRef::new(0, 0), Value::text("x"));
    set_directly.sheet_mut(0).unwrap().set_style(CellRef::new(0, 0), wide);
    let saved = write_xlsx(&set_directly).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let xml = String::from_utf8(pkg.read("xl/styles.xml").unwrap().unwrap()).unwrap();
    assert!(!xml.contains("<family val=\"200\"/>"), "{xml}");
}

#[test]
fn solid_data_bar_survives_with_sparklines_and_gradient_neighbours() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    s.set_value(at("A1"), Value::Number(1.0));
    let bar =
        |gradient: bool, max: CfValueKind| CfRule::DataBar { min: CfValueKind::Min, max, color: Color::Rgb(0x638EC6), gradient, show_value: true };
    let rules = [(true, "A1", CfValueKind::Max), (false, "B1", CfValueKind::Formula("$C$1".into())), (false, "D1:D3", CfValueKind::Number(9.0))];
    for (i, (gradient, range, max)) in rules.into_iter().enumerate() {
        s.cond_formats.push(CondFormat { ranges: vec![rr(range)], rule: bar(gradient, max), priority: i as u32 + 1, stop_if_true: false });
    }
    s.sparklines.push(Sparkline { cell: at("F1"), source: "A1:C1".into(), kind: SparklineKind::Line, color: Color::Rgb(0x376092), markers: false });
    let bytes = write_xlsx(&wb).unwrap();
    // x14 cfvo values are `xm:f` children; a `val` attribute there is not valid x14.
    let (xml, _) = sheet1_xml(&bytes);
    assert!(xml.contains("<x14:cfvo type=\"num\"><xm:f>9</xm:f></x14:cfvo>"), "{xml}");
    // Both solid bars sit in the one x14 conditional format extension, under its MS-XLSX uri.
    assert_eq!(xml.matches("<ext uri=\"{78C0D931-6437-407d-A8EE-F0AAD7539E65}\"").count(), 1, "{xml}");
    assert_eq!(xml.matches("<x14:cfRule type=\"dataBar\"").count(), 2, "{xml}");
    // As drawn here: negatives in the bar's colour, no axis (Excel's x14 defaults differ), and read back without a warning.
    assert_eq!(xml.matches("<x14:dataBar gradient=\"0\" negativeBarColorSameAsPositive=\"1\" axisPosition=\"none\">").count(), 2, "{xml}");
    let (back, rep) = read_xlsx(&bytes).unwrap();
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    let b = &back.sheets[0];
    assert_eq!(b.cond_formats, wb.sheets[0].cond_formats);
    assert_eq!(b.sparklines.len(), 1);
}

#[test]
fn doc_props_keep_boundary_whitespace() {
    let mut wb = Workbook::new();
    wb.props = DocProps {
        title: "  Title  ".into(),
        subject: "\n".into(),
        author: "\tMe\t".into(),
        company: " Co ".into(),
        keywords: " a, b ".into(),
        description: "\n  d\n".into(),
    };
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(back.props, wb.props);
}

/// Japanese paper sizes survive an XLSX save and reopen. IDs 12/13 are the JIS B4/B5; the
/// Japanese postcards and envelopes have their own IDs, and `B4 (ISO)` keeps ISO B4. The ISO B5/B6
/// names only exist in the engine: ECMA-376 has no `paperSize` id for them (only ISO B4), so they
/// are not part of the round-trip.
#[test]
fn japanese_paper_sizes_round_trip() {
    for paper in [
        "B4",
        "B5",
        "B6",
        "B4 (ISO)",
        "A6",
        "Japanese Postcard",
        "Japanese Double Postcard",
        "Japanese Envelope #3",
        "Japanese Envelope #4",
        "Japanese Envelope Kaku #2",
        "Japanese Envelope Kaku #3",
    ] {
        let mut wb = Workbook::new();
        wb.sheet_mut(0).unwrap().print.paper = paper.into();
        let (back, report) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert!(report.warnings.is_empty(), "{paper}: {:?}", report.warnings);
        assert_eq!(back.sheet(0).unwrap().print.paper, paper);
    }
    // An alias is written as the file's own name and read back as that name.
    let mut wb = Workbook::new();
    wb.sheet_mut(0).unwrap().print.paper = "B5 (JIS)".into();
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert_eq!(back.sheet(0).unwrap().print.paper, "B5");
}

/// A table's dropdown filter lives in `sheet.autofilter` (the engine's only live filter state),
/// not in `Table::filter`. Writing used to suppress the sheet `<autoFilter>` because a table
/// intersects it and write the table part from `Table::filter`, which was empty: the criteria
/// were lost on every save, leaving the rows hidden with no filter to clear on reopen.
#[test]
fn a_table_filter_criteria_survive_a_round_trip() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    for (r, v) in [(0u32, "Region"), (1, "East"), (2, "West"), (3, "East")] {
        s.set_value(at(&format!("A{}", r + 1)), Value::text(v));
    }
    s.tables.push(Table {
        id: 1,
        name: "Sales".into(),
        range: rr("A1:A4"),
        header_row: true,
        totals_row: false,
        columns: vec![TableColumn { name: "Region".into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None }],
        style: "TableStyleMedium2".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    // The dropdown of a table's header cell installs the sheet AutoFilter over the table's
    // range, and `data.filterBy` stores its criteria there.
    s.autofilter =
        Some(AutoFilter { range: rr("A1:A4"), criteria: vec![(0, FilterCriterion::Values { values: vec!["East".into()], blanks: false })] });
    s.rows.entry(1).or_default().hidden = true;

    let (back, rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    let b = back.sheet(0).unwrap();
    let t = b.tables.first().expect("the table survived");
    let (_, crit) = t.filter.first().expect("the table's filter criteria were kept on save");
    assert!(matches!(crit, FilterCriterion::Values { values, .. } if values == &["East".to_string()]), "the criteria were dropped: {:?}", t.filter);
    assert!(b.is_row_hidden(1), "the row the filter hid is still hidden");
    assert!(b.autofilter.is_none(), "a table's filter belongs to the table part, not the sheet");
}

/// Stored `colId` offsets are relative to the filter range's first column, so criteria of a filter
/// that starts right of the table's first column must not be folded into the table part, where
/// they would apply to the wrong column.
#[test]
fn a_partial_sheet_filter_is_not_folded_into_a_table() {
    let mut wb = Workbook::new();
    let s = wb.sheet_mut(0).unwrap();
    for (a, v) in [("A1", "Id"), ("B1", "Region"), ("A2", "1"), ("B2", "East"), ("A3", "2"), ("B3", "West")] {
        s.set_value(at(a), Value::text(v));
    }
    let column =
        |name: &str| TableColumn { name: name.into(), totals: TotalsFn::None, totals_label: None, formula: None, unique_name: None, xml: None };
    s.tables.push(Table {
        id: 1,
        name: "Sales".into(),
        range: rr("A1:B3"),
        header_row: true,
        totals_row: false,
        columns: vec![column("Id"), column("Region")],
        style: "TableStyleMedium2".into(),
        banded_rows: true,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    s.autofilter =
        Some(AutoFilter { range: rr("B1:B3"), criteria: vec![(0, FilterCriterion::Values { values: vec!["East".into()], blanks: false })] });

    let (back, rep) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    let b = back.sheet(0).unwrap();
    let t = b.tables.first().expect("the table survived");
    assert!(t.filter.is_empty(), "the criteria landed on the wrong table column: {:?}", t.filter);
    assert!(b.autofilter.is_none(), "a sheet filter inside a table would be invalid");
}

#[test]
fn workbook_view_settings_survive_a_round_trip() {
    // #3057: a workbook Excel saved with the sheet tabs and both scroll bars hidden
    // (and a non-default tab split) must open with those settings in the model and
    // save with them still in the file.
    let bytes = super::minimal(
        "",
        &[],
        "",
        "<bookViews><workbookView activeTab=\"0\" showSheetTabs=\"0\" showHorizontalScroll=\"0\" showVerticalScroll=\"0\" tabRatio=\"250\"/></bookViews>",
    );
    let (wb, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(
        wb.view,
        WorkbookView { show_sheet_tabs: false, show_horizontal_scroll: false, show_vertical_scroll: false, tab_ratio: 250 },
        "the reader takes the view settings from <workbookView>"
    );

    let out = write_xlsx(&wb).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(out)).unwrap();
    let mut xml = String::new();
    z.by_name("xl/workbook.xml").unwrap().read_to_string(&mut xml).unwrap();
    for attr in ["showSheetTabs=\"0\"", "showHorizontalScroll=\"0\"", "showVerticalScroll=\"0\"", "tabRatio=\"250\""] {
        assert!(xml.contains(attr), "the writer keeps {attr} in {xml}");
    }

    // A workbook with the format's defaults writes the bare element it always has.
    let plain = write_xlsx(&Workbook::new()).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(plain)).unwrap();
    let mut xml = String::new();
    z.by_name("xl/workbook.xml").unwrap().read_to_string(&mut xml).unwrap();
    assert!(xml.contains("<workbookView activeTab=\"0\"/>"), "defaults stay unwritten: {xml}");
}

/// Three original empty worksheets let view selection be checked through `activeTab`,
/// independently of the flags and ratio carried by the same window.
fn workbook_with_window_views(views: &str) -> Vec<u8> {
    let workbook = format!(
        r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><bookViews>{views}</bookViews><sheets><sheet name="First" sheetId="1" r:id="rId1"/><sheet name="Second" sheetId="2" r:id="rId2"/><sheet name="Third" sheetId="3" r:id="rId3"/></sheets></workbook>"#
    );
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/></Relationships>"#;
    let sheet = br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#;
    super::make_zip(&[
        ("[Content_Types].xml", super::CT.as_bytes()),
        ("_rels/.rels", super::ROOT_RELS.as_bytes()),
        ("xl/workbook.xml", workbook.as_bytes()),
        ("xl/_rels/workbook.xml.rels", rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet),
        ("xl/worksheets/sheet2.xml", sheet),
        ("xl/worksheets/sheet3.xml", sheet),
    ])
}

fn saved_workbook_view_xml(wb: &Workbook) -> String {
    let bytes = write_xlsx(wb).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("xl/workbook.xml").unwrap().read_to_string(&mut xml).unwrap();
    xml
}

#[test]
fn workbook_view_attributes_round_trip_independently() {
    for (attribute, value, expected) in [
        ("showSheetTabs", "0", WorkbookView { show_sheet_tabs: false, ..Default::default() }),
        ("showSheetTabs", "1", WorkbookView::default()),
        ("showHorizontalScroll", "0", WorkbookView { show_horizontal_scroll: false, ..Default::default() }),
        ("showHorizontalScroll", "1", WorkbookView::default()),
        ("showVerticalScroll", "0", WorkbookView { show_vertical_scroll: false, ..Default::default() }),
        ("showVerticalScroll", "1", WorkbookView::default()),
        ("tabRatio", "250", WorkbookView { tab_ratio: 250, ..Default::default() }),
        ("tabRatio", "600", WorkbookView::default()),
    ] {
        let view = format!("<workbookView {attribute}=\"{value}\"/>");
        let (wb, _) = read_xlsx(&workbook_with_window_views(&view)).unwrap();
        assert_eq!(wb.view, expected, "only {attribute} is supplied");
        let xml = saved_workbook_view_xml(&wb);
        assert_eq!(xml.contains(&format!("{attribute}=\"{value}\"")), expected != WorkbookView::default(), "{xml}");
        for other in ["showSheetTabs", "showHorizontalScroll", "showVerticalScroll", "tabRatio"] {
            if other != attribute {
                assert!(!xml.contains(&format!("{other}=")), "unrelated defaults stay omitted: {xml}");
            }
        }
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(back.view, expected, "{attribute}={value} survives a round trip");
    }
}

#[test]
fn workbook_view_selects_first_visible_window_consistently() {
    let cases = [
        (
            r#"<workbookView visibility="hidden" activeTab="0" showSheetTabs="0" showHorizontalScroll="0" showVerticalScroll="0" tabRatio="111"/><workbookView visibility="veryHidden" activeTab="1" tabRatio="222"/><workbookView visibility="visible" activeTab="2" showSheetTabs="1" showHorizontalScroll="0" showVerticalScroll="1" tabRatio="750"/><workbookView visibility="visible" activeTab="0" tabRatio="250"/>"#,
            2,
            WorkbookView { show_sheet_tabs: true, show_horizontal_scroll: false, show_vertical_scroll: true, tab_ratio: 750 },
        ),
        // Visibility defaults to visible, so this view takes precedence over a later explicit one.
        (
            r#"<workbookView visibility="hidden" activeTab="0" showSheetTabs="0" tabRatio="111"/><workbookView activeTab="1" showVerticalScroll="0" tabRatio="350"/><workbookView visibility="visible" activeTab="2" tabRatio="900"/>"#,
            1,
            WorkbookView { show_vertical_scroll: false, tab_ratio: 350, ..Default::default() },
        ),
        // No visible window: retain the first window's settings and active sheet as one unit.
        (
            r#"<workbookView visibility="hidden" activeTab="1" showSheetTabs="0" tabRatio="100"/><workbookView visibility="veryHidden" activeTab="2" tabRatio="900"/>"#,
            1,
            WorkbookView { show_sheet_tabs: false, tab_ratio: 100, ..Default::default() },
        ),
    ];
    for (views, active, view) in cases {
        let (wb, _) = read_xlsx(&workbook_with_window_views(views)).unwrap();
        assert_eq!(wb.active_sheet(), active, "activeTab must come from the selected view: {views}");
        assert_eq!(wb.view, view, "all settings must come from the selected view: {views}");
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(back.active_sheet(), active);
        assert_eq!(back.view, view);
    }
}

#[test]
fn workbook_view_without_a_window_uses_defaults() {
    for bytes in [super::minimal("", &[], "", ""), workbook_with_window_views("")] {
        let (wb, _) = read_xlsx(&bytes).unwrap();
        assert_eq!(wb.active_sheet(), 0);
        assert_eq!(wb.view, WorkbookView::default());
        let xml = saved_workbook_view_xml(&wb);
        assert!(xml.contains("<workbookView activeTab=\"0\"/>"), "missing views keep defaults omitted: {xml}");
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(back.active_sheet(), 0);
        assert_eq!(back.view, WorkbookView::default());
    }
}

#[test]
fn workbook_view_tab_ratio_normalizes_imported_values() {
    for (raw, expected) in [
        (None, 600),
        (Some("0"), 0),
        (Some("600"), 600),
        (Some("1000"), 1000),
        (Some("1001"), 1000),
        (Some("4294967295"), 1000),
        (Some("4294967296"), 600),
        (Some("-1"), 600),
        (Some("invalid"), 600),
        (Some(""), 600),
    ] {
        let attr = raw.map(|value| format!(" tabRatio=\"{value}\"")).unwrap_or_default();
        let views = format!("<workbookView{attr}/>");
        let (wb, _) = read_xlsx(&workbook_with_window_views(&views)).unwrap();
        assert_eq!(wb.view.tab_ratio, expected, "tabRatio={raw:?}");
        let xml = saved_workbook_view_xml(&wb);
        if expected == 600 {
            assert!(!xml.contains("tabRatio="), "the default is omitted: {xml}");
        } else {
            assert!(xml.contains(&format!("tabRatio=\"{expected}\"")), "{xml}");
        }
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(back.view.tab_ratio, expected);
    }
}

#[test]
fn workbook_view_writer_clamps_out_of_range_model_values() {
    for raw in [1001, u32::MAX] {
        let mut wb = Workbook::new();
        wb.view.tab_ratio = raw;
        let xml = saved_workbook_view_xml(&wb);
        assert!(xml.contains("tabRatio=\"1000\""), "an out-of-range model value is bounded on write: {xml}");
        let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
        assert_eq!(back.view.tab_ratio, 1000);
    }
}

#[test]
fn generated_rule_formulas_use_the_top_left_cell_of_every_area() {
    // Issue #3152: the areas aren't in top-to-bottom order; the formula is written for C2, not C5.
    let mut wb = Workbook::new();
    let red = Box::new(Style::default());
    wb.sheet_mut(0).unwrap().cond_formats.push(CondFormat {
        ranges: vec![rr("C5:C6"), rr("C2:C3")],
        rule: CfRule::ContainsText { text: "x".into(), style: red },
        priority: 1,
        stop_if_true: false,
    });
    let bytes = write_xlsx(&wb).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    z.by_name("xl/worksheets/sheet1.xml").unwrap().read_to_string(&mut xml).unwrap();
    assert!(xml.contains("<formula>NOT(ISERROR(SEARCH(\"x\",C2)))</formula>"), "{xml}");
}

#[test]
fn r1c1_reference_style_is_kept() {
    // #2809: `<calcPr refMode="R1C1"/>` was dropped on open, so a workbook saved by Excel in R1C1
    // style came back in A1 style.
    let bytes = super::minimal("<sheetData/>", &[], "", r#"<calcPr calcId="191029" refMode="R1C1"/>"#);
    let (wb, _) = read_xlsx(&bytes).unwrap();
    assert!(wb.calc.r1c1);
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let book = pkg.read_xml("xl/workbook.xml").unwrap().unwrap();
    assert_eq!(book.child("calcPr").and_then(|c| c.attr("refMode")), Some("R1C1"));
    // A1 is the default and isn't written.
    let saved = write_xlsx(&Workbook::new()).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let book = pkg.read_xml("xl/workbook.xml").unwrap().unwrap();
    assert_eq!(book.child("calcPr").and_then(|c| c.attr("refMode")), None);
}

#[test]
fn remove_personal_information_is_kept_and_honoured() {
    // #3089: `<workbookPr filterPrivacy="1"/>` was dropped, so a workbook cleaned of personal
    // information came back with it switched off, and GridCraft wrote the author and company back.
    let bytes = super::minimal("<sheetData/>", &[], "", r#"<workbookPr filterPrivacy="1"/>"#);
    let (mut wb, _) = read_xlsx(&bytes).unwrap();
    assert!(wb.remove_personal_info);
    wb.props.author = "Jane Example".into();
    wb.props.company = "Example Co".into();
    wb.props.title = "Budget".into();
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let book = pkg.read_xml("xl/workbook.xml").unwrap().unwrap();
    assert_eq!(book.child("workbookPr").and_then(|p| p.attr("filterPrivacy")), Some("1"));
    let core = String::from_utf8(pkg.read("docProps/core.xml").unwrap().unwrap()).unwrap();
    let app = String::from_utf8(pkg.read("docProps/app.xml").unwrap().unwrap()).unwrap();
    assert!(!core.contains("Jane Example") && !app.contains("Example Co"), "{core}\n{app}");
    assert!(core.contains("<dc:title>Budget</dc:title>"), "other properties stay: {core}");
    // Off (the default), nothing changes.
    wb.remove_personal_info = false;
    let saved = write_xlsx(&wb).unwrap();
    let mut pkg = Package::open(&saved).unwrap();
    let book = pkg.read_xml("xl/workbook.xml").unwrap().unwrap();
    assert_eq!(book.child("workbookPr").and_then(|p| p.attr("filterPrivacy")), None);
    assert!(String::from_utf8(pkg.read("docProps/core.xml").unwrap().unwrap()).unwrap().contains("Jane Example"));
}

#[test]
fn issue_778_xlsx_xml_import_save_reopen_roundtrip() {
    let mut wb = Workbook::new();
    let mut s = Sheet::new("Delegated");
    s.cells.set(at("A1"), Cell { value: Value::text("Item"), formula: None, style: StyleId::DEFAULT, py: None });
    s.cells.set(at("B1"), Cell { value: Value::text("Amount"), formula: None, style: StyleId::DEFAULT, py: None });
    s.cells.set(at("A2"), Cell { value: Value::text("Alpha"), formula: None, style: StyleId::DEFAULT, py: None });
    s.cells.set(at("B2"), Cell { value: Value::Number(42.5), formula: None, style: StyleId::DEFAULT, py: None });
    let mut f = Cell::formula(Formula::new("=B2*2"));
    f.value = Value::Number(85.0);
    s.cells.set(at("B3"), f);
    s.merges.push(rr("A4:B4"));
    s.hyperlinks.insert(at("A2"), Hyperlink { target: "https://example.com".into(), tooltip: Some("Example".into()), external: true });
    s.comments.insert(
        at("B2"),
        Comment {
            author: "Reviewer".into(),
            text: "Checked".into(),
            replies: vec![],
            threaded: false,
            resolved: false,
            visible: true,
            dates: vec![],
            note_box: None,
        },
    );
    s.shapes.push(Shape {
        id: 1,
        kind: ShapeKind::Rectangle,
        anchor: Anchor { cell: at("D2"), dx: 4.0, dy: 4.0, width: 120.0, height: 60.0, mode: AnchorMode::MoveAndSize },
        fill: Color::Rgb(0x336699),
        line: Color::Rgb(0x112233),
        text: "Box".into(),
        format: Default::default(),
        rotation: 0.0,
        math: Default::default(),
    });
    s.charts.push(Chart {
        id: 2,
        kind: ChartKind::ColumnClustered,
        anchor: Anchor { cell: at("D6"), dx: 0.0, dy: 0.0, width: 320.0, height: 200.0, mode: AnchorMode::MoveAndSize },
        title: Some("Sales".into()),
        series: vec![Series {
            name: Some("Delegated!$B$1".into()),
            categories: Some("Delegated!$A$2:$A$2".into()),
            values: "Delegated!$B$2:$B$2".into(),
            ..Default::default()
        }],
        legend: LegendPos::Bottom,
        data_labels: true,
        data_table: Some(ChartDataTable::default()),
        gridlines: true,
        style: 2,
        hole_size: 75,
        first_slice_angle: 0,
        x_title: None,
        y_title: None,
        source: None,
        by_rows: false,
        pivot: None,
    });
    wb.sheets[0] = Arc::new(s);

    let bytes = write_xlsx(&wb).unwrap();
    let (reopened, _) = read_xlsx(&bytes).unwrap();
    let bytes2 = write_xlsx(&reopened).unwrap();
    let (again, _) = read_xlsx(&bytes2).unwrap();

    let sh = &again.sheets[0];
    assert_eq!(sh.cells.get(at("A2")).unwrap().value, Value::text("Alpha"));
    assert_eq!(sh.cells.get(at("B3")).unwrap().value, Value::Number(85.0));
    assert_eq!(sh.cells.get(at("B3")).unwrap().formula.as_ref().map(|f| f.text.as_str()), Some("B2*2"));
    assert_eq!(sh.merges, vec![rr("A4:B4")]);
    assert_eq!(sh.hyperlinks.get(&at("A2")).map(|h| h.target.as_str()), Some("https://example.com"));
    assert_eq!(sh.comments.get(&at("B2")).map(|c| c.text.as_str()), Some("Checked"));
    assert_eq!(sh.shapes.len(), 1);
    assert_eq!(sh.shapes[0].text, "Box");
    assert_eq!(sh.charts.len(), 1);
    assert_eq!(sh.charts[0].title.as_deref(), Some("Sales"));
    assert_eq!(sh.charts[0].data_table, Some(ChartDataTable::default()));
}

#[test]
fn issue_778_full_workbook_import_save_reopen_roundtrip() {
    const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const NS_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

    let png = b"\x89PNG\r\n\x1a\n0000IHDRfake".to_vec();
    let external_link = format!(
        r#"<?xml version="1.0"?><externalLink xmlns="{NS_MAIN}" xmlns:r="{NS_REL}"><externalBook r:id="rId1"><sheetNames><sheetName val="Sheet1"/></sheetNames><sheetDataSet><sheetData sheetId="0"><row r="1"><cell r="A1"><v>10</v></cell></row></sheetData></sheetDataSet></externalBook></externalLink>"#
    );
    let external_link_rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_REL}/externalLinkPath" Target="file:///C:/never/opened/Book2.xlsx" TargetMode="External"/></Relationships>"#
    );
    let types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/><Override PartName="/xl/charts/chart1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/><Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/></Types>"#;
    let root_rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
    );
    let wb_xml = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{NS_MAIN}" xmlns:r="{NS_REL}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Notes" sheetId="2" state="hidden" r:id="rId2"/></sheets><externalReferences><externalReference r:id="rId3"/></externalReferences></workbook>"#
    );
    let wb_rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_REL}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{NS_REL}/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="{NS_REL}/externalLink" Target="externalLinks/externalLink1.xml"/></Relationships>"#
    );
    let sheet1 = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_REL}"><sheetData><row r="1"><c r="A1" t="str"><v>Item</v></c><c r="B1" t="str"><v>Amount</v></c></row><row r="2"><c r="A2" t="str"><v>Alpha</v></c><c r="B2"><v>42.5</v></c><c r="C2"><f>[1]Sheet1!$A$1</f><v>10</v></c></row><row r="3"><c r="B3"><f>B2*2</f><v>85</v></c><c r="C3"><f>SUM([1]Sheet1!A1:A3)*2</f><v>60</v></c></row></sheetData><drawing r:id="rId1"/></worksheet>"#
    );
    let sheet1_rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_REL}/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#
    );
    let sheet2 =
        format!(r#"<?xml version="1.0"?><worksheet xmlns="{NS_MAIN}"><sheetData><row r="1"><c r="A1"><v>7</v></c></row></sheetData></worksheet>"#);
    let drawing1 = format!(
        r#"<?xml version="1.0"?><xdr:wsDr xmlns:xdr="{NS_XDR}" xmlns:a="{NS_A}" xmlns:r="{NS_REL}"><xdr:twoCellAnchor editAs="oneCell"><xdr:from><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>9</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>13</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:pic><xdr:nvPicPr><xdr:cNvPr id="10" name="Picture 1" descr="Logo"/><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="rId1"/></xdr:blipFill><xdr:spPr/></xdr:pic><xdr:clientData/></xdr:twoCellAnchor><xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>5</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>4</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="11" name="Rect 1"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr><xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Box</a:t></a:r></a:p></xdr:txBody></xdr:sp><xdr:clientData/></xdr:twoCellAnchor><xdr:twoCellAnchor><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>5</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>8</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>15</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:graphicFrame><xdr:nvGraphicFramePr><xdr:cNvPr id="12" name="Chart 1"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="{NS_C}" r:id="rId2"/></a:graphicData></a:graphic></xdr:graphicFrame><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"#
    );
    let drawing1_rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{NS_REL}/image" Target="../media/image1.png"/><Relationship Id="rId2" Type="{NS_REL}/chart" Target="../charts/chart1.xml"/></Relationships>"#
    );
    let chart1 = format!(
        r#"<?xml version="1.0"?><c:chartSpace xmlns:c="{NS_C}" xmlns:a="{NS_A}"><c:chart><c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Sales</a:t></a:r></a:p></c:rich></c:tx></c:title><c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>Data!$B$1</c:f></c:strRef></c:tx><c:cat><c:strRef><c:f>Data!$A$2:$A$2</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>Data!$B$2:$B$2</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="1"/><c:pt idx="0"><c:v>42.5</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser></c:barChart><c:dTable><c:showKeys/><c:showHorzBorder/><c:showVertBorder/><c:showOutline/></c:dTable></c:plotArea></c:chart></c:chartSpace>"#
    );

    let input = super::make_zip(&[
        ("[Content_Types].xml", types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("xl/workbook.xml", wb_xml.as_bytes()),
        ("xl/_rels/workbook.xml.rels", wb_rels.as_bytes()),
        ("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
        ("xl/worksheets/_rels/sheet1.xml.rels", sheet1_rels.as_bytes()),
        ("xl/worksheets/sheet2.xml", sheet2.as_bytes()),
        ("xl/drawings/drawing1.xml", drawing1.as_bytes()),
        ("xl/drawings/_rels/drawing1.xml.rels", drawing1_rels.as_bytes()),
        ("xl/media/image1.png", &png),
        ("xl/charts/chart1.xml", chart1.as_bytes()),
        ("xl/externalLinks/externalLink1.xml", external_link.as_bytes()),
        ("xl/externalLinks/_rels/externalLink1.xml.rels", external_link_rels.as_bytes()),
    ]);

    let (imported, rep1) = read_xlsx(&input).unwrap();
    assert!(!rep1.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep1.warnings);
    let saved1 = write_xlsx(&imported).unwrap();
    let (reopened, rep2) = read_xlsx(&saved1).unwrap();
    assert!(!rep2.warnings.iter().any(|w| w.contains("dropped")), "{:?}", rep2.warnings);
    let saved2 = write_xlsx(&reopened).unwrap();
    let (again, _) = read_xlsx(&saved2).unwrap();

    assert_eq!(again.sheets.len(), 2);
    assert_eq!(again.sheets[0].name, "Data");
    assert_eq!(again.sheets[0].visibility, Visibility::Visible);
    assert_eq!(again.sheets[1].name, "Notes");
    assert_eq!(again.sheets[1].visibility, Visibility::Hidden);
    assert_eq!(again.sheets[1].value(at("A1")), Value::Number(7.0));

    let sh = &again.sheets[0];
    assert_eq!(sh.cells.get(at("B3")).unwrap().value, Value::Number(85.0));
    assert_eq!(sh.cells.get(at("B3")).unwrap().formula.as_ref().map(|f| f.text.as_str()), Some("B2*2"));
    assert_eq!(sh.cells.get(at("C2")).unwrap().value, Value::Number(10.0));
    assert_eq!(sh.cells.get(at("C2")).unwrap().formula.as_ref().map(|f| f.text.as_str()), Some("[1]Sheet1!$A$1"));
    assert_eq!(sh.cells.get(at("C3")).unwrap().value, Value::Number(60.0));
    assert_eq!(sh.cells.get(at("C3")).unwrap().formula.as_ref().map(|f| f.text.as_str()), Some("SUM([1]Sheet1!A1:A3)*2"));

    assert_eq!(sh.images.len(), 1);
    assert_eq!(sh.images[0].data.as_ref(), png.as_slice());
    assert_eq!(sh.images[0].anchor.mode, AnchorMode::MoveOnly);
    assert_eq!(sh.shapes.len(), 1);
    assert_eq!(sh.shapes[0].text, "Box");
    assert_eq!(sh.charts.len(), 1);
    assert_eq!(sh.charts[0].title.as_deref(), Some("Sales"));
    assert_eq!(sh.charts[0].data_table, Some(ChartDataTable::default()));

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(saved2)).unwrap();
    let mut read_part = |name: &str| {
        let mut s = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    };
    let out_wb = read_part("xl/workbook.xml");
    assert!(out_wb.contains("<externalReferences"), "{out_wb}");
    let out_ext = read_part("xl/externalLinks/externalLink1.xml");
    assert!(out_ext.contains("<sheetName val=\"Sheet1\"/>"), "{out_ext}");
    let out_ext_rels = read_part("xl/externalLinks/_rels/externalLink1.xml.rels");
    assert!(out_ext_rels.contains("file:///C:/never/opened/Book2.xlsx") && out_ext_rels.contains("TargetMode=\"External\""), "{out_ext_rels}");
    let out_chart = read_part("xl/charts/chart1.xml");
    assert!(out_chart.contains("<c:numCache>") && out_chart.contains("<c:v>42.5</c:v>"), "{out_chart}");
}
