//! Table styles look like the Format as Table gallery: every row of the gallery (Light 1-7, 8-14,
//! 15-21, Medium 1-7, …) has its own fills and lines, as recorded in plan/excel/table-style-spec.json.

use gridcraft_core::CellRef;
use gridcraft_engine::Session;
use gridcraft_engine::model::*;
use gridcraft_engine::tables::{ShownEdges, shown_edges, table_cell_shown_style, table_cell_style};
use serde_json::json;

const ROWS: u32 = 6; // header + 5 data rows
const COLS: u32 = 5;

fn session() -> Session {
    let mut s = Session::new();
    s.new_workbook();
    s.execute("range.setValues", json!({"range": "A1", "values": [["a", "b", "c", "d", "e"]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:E6", "header": true})).unwrap();
    s
}

/// The look of every cell of a 6x5 table (header on, banded rows) in `style`, row by row.
fn grid_with(s: &Session, style: &str, edit: impl Fn(&mut Table)) -> Vec<Vec<Style>> {
    let wb = &s.doc().unwrap().wb;
    let mut t = wb.sheet(0).unwrap().tables[0].clone();
    t.style = format!("TableStyle{style}");
    edit(&mut t);
    (0..ROWS).map(|r| (0..COLS).map(|c| table_cell_style(wb, 0, &t, CellRef::new(r, c))).collect()).collect()
}

fn grid(style: &str) -> Vec<Vec<Style>> {
    grid_with(&session(), style, |_| {})
}

fn thin(color: Color) -> BorderLine {
    BorderLine { style: BorderStyle::Thin, color }
}

const NONE: BorderLine = BorderLine { style: BorderStyle::None, color: Color::Auto };
const BLACK: Color = Color::Theme(1, 0);
const WHITE: Color = Color::Theme(0, 0);

#[test]
fn each_row_of_the_gallery_has_its_own_look() {
    // Same accent (or neutral) across the gallery's rows, so only the row-group differs.
    for set in [
        &["Light1", "Light8", "Light15", "Medium1", "Medium8", "Medium15", "Medium22", "Dark1", "Dark8"][..],
        &["Light2", "Light9", "Light16", "Medium2", "Medium9", "Medium16", "Medium23", "Dark2", "Dark9"][..],
    ] {
        let looks: Vec<_> = set.iter().map(|s| grid(s)).collect();
        for (i, a) in looks.iter().enumerate() {
            for (j, b) in looks.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "{} and {} look the same", set[i], set[j]);
            }
        }
    }
}

#[test]
fn neutral_bands_are_visibly_grey() {
    let theme = Theme::default();
    let fill = |style: &str, row: usize| grid(style)[row][0].fill.fg.resolve(&theme).unwrap();
    // lt1 -150 (about #D9D9D9) and lt1 -350 (about #A6A6A6).
    for ch in fill("Light1", 1) {
        assert!((210..=225).contains(&ch), "Light1 band {:?}", fill("Light1", 1));
    }
    for ch in fill("Medium8", 1) {
        assert!((160..=172).contains(&ch), "Medium8 band1 {:?}", fill("Medium8", 1));
    }
}

#[test]
fn light15_has_a_vertical_grid_and_no_row_lines() {
    let g = grid("Light15");
    assert_eq!(g[0][0].borders.top, thin(BLACK));
    assert_eq!(g[0][2].borders.bottom, thin(BLACK), "header bottom");
    assert_eq!(g[2][1].borders.right, thin(BLACK), "inner vertical");
    assert_eq!(g[0][1].borders.right, thin(BLACK), "inner vertical in the header");
    assert_eq!(g[2][0].borders.left, thin(BLACK), "left side");
    assert_eq!(g[2][4].borders.right, thin(BLACK), "right side");
    assert_eq!(g[5][2].borders.bottom, thin(BLACK), "bottom");
    assert_eq!(g[2][1].borders.bottom, NONE, "no inner horizontal");
    assert_eq!(g[2][1].borders.top, NONE, "no inner horizontal");
    assert_eq!(g[1][0].fill.fg, Color::Theme(0, -150));
}

#[test]
fn medium2_has_a_pale_accent_frame_and_no_row_lines() {
    let g = grid("Medium2");
    let line = thin(Color::Theme(4, 400));
    assert_eq!(g[0][0].fill.fg, Color::Theme(4, 0));
    assert_eq!(g[0][0].font.color, WHITE);
    assert_eq!(g[0][2].borders.top, line);
    assert_eq!(g[0][0].borders.left, line, "the sides run down the header too");
    assert_eq!(g[3][0].borders.left, line);
    assert_eq!(g[3][4].borders.right, line);
    assert_eq!(g[5][2].borders.bottom, line);
    assert_eq!(g[0][2].borders.bottom, NONE, "no header bottom");
    assert_eq!(g[2][2].borders.bottom, NONE, "no inner horizontal");
    assert_eq!(g[2][2].borders.right, NONE, "no inner vertical");
    assert_eq!(g[1][2].fill.fg, Color::Theme(4, 800));
    assert_eq!(g[2][2].fill.pattern, PatternType::None);
}

#[test]
fn medium9_has_white_verticals_and_header_bottom() {
    let g = grid("Medium9");
    assert_eq!(g[0][1].borders.bottom, thin(WHITE));
    assert_eq!(g[0][1].borders.right, thin(WHITE));
    assert_eq!(g[3][1].borders.right, thin(WHITE));
    assert_eq!(g[3][1].borders.bottom, NONE);
    assert_eq!(g[3][0].borders.left, NONE, "no frame");
    assert_eq!(g[5][4].borders.bottom, NONE, "no frame");
    assert_eq!(g[1][0].fill.fg, Color::Theme(4, 600));
    assert_eq!(g[2][0].fill.fg, Color::Theme(4, 800));
}

#[test]
fn dark2_bands_in_shaded_and_full_accent_under_a_black_header() {
    let g = grid("Dark2");
    assert_eq!(g[0][0].fill.fg, BLACK);
    assert_eq!(g[0][0].borders.bottom, thin(WHITE));
    assert_eq!(g[1][0].fill.fg, Color::Theme(4, -250));
    assert_eq!(g[2][0].fill.fg, Color::Theme(4, 0));
    assert_eq!(g[2][0].font.color, WHITE);
    assert_eq!(g[2][0].borders, Borders::default());
}

#[test]
fn dark9_pairs_two_accents() {
    let g = grid("Dark9");
    assert_eq!(g[0][0].fill.fg, Color::Theme(5, 0));
    assert_eq!(g[0][0].font.color, WHITE);
    assert_eq!(g[1][0].fill.fg, Color::Theme(4, 600));
    assert_eq!(g[2][0].fill.fg, Color::Theme(4, 800));
    assert_eq!(g[2][0].font.color, BLACK);
    let g = grid("Dark11");
    assert_eq!(g[0][0].fill.fg, Color::Theme(9, 0));
    assert_eq!(g[1][0].fill.fg, Color::Theme(8, 600));
}

#[test]
fn table_options_change_the_look() {
    let s = session();
    // Banded rows off: every data row looks like the second band.
    let g = grid_with(&s, "Medium9", |t| t.banded_rows = false);
    assert_eq!(g[1][0].fill.fg, Color::Theme(4, 800));
    assert_eq!(g[2][0].fill.fg, Color::Theme(4, 800));
    // Banded columns alternate by column.
    let g = grid_with(&s, "Medium9", |t| {
        t.banded_rows = false;
        t.banded_cols = true;
    });
    assert_eq!(g[2][0].fill.fg, Color::Theme(4, 600));
    assert_eq!(g[2][1].fill.fg, Color::Theme(4, 800));
    // Header off: the top line moves to the first data row, and nothing is drawn as a header.
    let g = grid_with(&s, "Light15", |t| t.header_row = false);
    assert_eq!(g[0][2].borders.top, thin(BLACK));
    assert_eq!(g[0][2].borders.bottom, NONE);
    assert!(!g[0][2].font.bold);
    // Totals row: bold, double top rule, no banding.
    let g = grid_with(&s, "Medium2", |t| t.totals_row = true);
    assert!(g[5][1].font.bold);
    assert_eq!(g[5][1].borders.top, BorderLine { style: BorderStyle::Double, color: Color::Theme(4, 0) });
    assert_eq!(g[5][1].borders.bottom, thin(Color::Theme(4, 400)));
    // First and last column bold.
    let g = grid_with(&s, "Medium2", |t| {
        t.first_col = true;
        t.last_col = true;
    });
    assert!(g[2][0].font.bold && g[2][4].font.bold && !g[2][2].font.bold);
}

#[test]
fn explicit_formatting_wins() {
    let mut s = session();
    s.execute("home.fillColor", json!({"range": "B3", "color": "#FF0000"})).unwrap();
    s.execute("home.fontColor", json!({"range": "B3", "color": "#00FF00"})).unwrap();
    s.execute("home.borders", json!({"range": "B3", "preset": "bottom"})).unwrap();
    let g = grid_with(&s, "Dark2", |_| {});
    assert_eq!(g[2][1].fill, Fill::solid(Color::Rgb(0xFF0000)));
    assert_eq!(g[2][1].font.color, Color::Rgb(0x00FF00));
    assert_eq!(g[2][1].borders.bottom.style, BorderStyle::Thin);
    assert_eq!(g[2][1].borders.bottom.color, Color::Auto);
}

#[test]
fn every_style_name_is_safe() {
    for name in ["Light0", "Light99", "Medium0", "Medium4000000000", "Dark0", "Dark12", "Dark99", "Bogus"] {
        let _ = grid(name);
    }
}

/// What the grid and print draw: like `grid_with`, but through the hidden-row/column aware
/// `table_cell_shown_style`.
fn shown_with(s: &Session, style: &str, edit: impl Fn(&mut Table)) -> Vec<Vec<Style>> {
    let wb = &s.doc().unwrap().wb;
    let mut t = wb.sheet(0).unwrap().tables[0].clone();
    t.style = format!("TableStyle{style}");
    edit(&mut t);
    let e = shown_edges(wb.sheet(0).unwrap(), &t);
    (0..ROWS).map(|r| (0..COLS).map(|c| table_cell_shown_style(wb, 0, &t, e, CellRef::new(r, c))).collect()).collect()
}

#[test]
fn frame_follows_the_last_visible_row_and_column() {
    let frame = thin(Color::Theme(4, 400));
    // Nothing hidden: the shown look is the table look.
    let s = session();
    assert_eq!(shown_with(&s, "Medium2", |_| {}), grid_with(&s, "Medium2", |_| {}));
    // Last data row (and the one before it) and last column hidden: the frame moves in.
    let mut s = session();
    s.execute("home.hideRows", json!({"rows": "5:6"})).unwrap();
    s.execute("home.hideColumns", json!({"cols": "E:E"})).unwrap();
    let t = &s.doc().unwrap().wb.sheet(0).unwrap().tables[0];
    assert_eq!(shown_edges(s.doc().unwrap().wb.sheet(0).unwrap(), t), ShownEdges { top: 0, bottom: 3, left: 0, right: 3 });
    let g = shown_with(&s, "Medium2", |_| {});
    for (c, cell) in g[3].iter().take(4).enumerate() {
        assert_eq!(cell.borders.bottom, frame, "bottom frame on row 4, col {c}");
    }
    for (r, row) in g.iter().take(4).enumerate() {
        assert_eq!(row[3].borders.right, frame, "right frame on col D, row {r}");
    }
    // Rows above the last visible one keep their inner look (Medium 2 has no row lines).
    assert_eq!(g[2][1].borders.bottom, NONE);
    // Convert to Range and friends still see the table as it is.
    assert_eq!(grid_with(&s, "Medium2", |_| {})[3][1].borders.bottom, NONE);
    // Header off and the first data row filtered out: the top line moves down.
    let mut s = session();
    s.execute("home.hideRows", json!({"rows": "1:1"})).unwrap();
    s.execute("home.hideColumns", json!({"cols": "A:A"})).unwrap();
    let g = shown_with(&s, "Light15", |t| t.header_row = false);
    assert_eq!(g[1][2].borders.top, thin(BLACK));
    assert_eq!(g[2][1].borders.left, thin(BLACK));
    // Explicit borders still win.
    let mut s = session();
    s.execute("home.borders", json!({"range": "B5", "preset": "none"})).unwrap();
    s.execute("home.fillColor", json!({"range": "B5", "color": "#FF0000"})).unwrap();
    s.execute("home.borders", json!({"range": "C5", "preset": "bottom"})).unwrap();
    s.execute("home.hideRows", json!({"rows": "6:6"})).unwrap();
    let g = shown_with(&s, "Medium2", |_| {});
    assert_eq!(g[4][1].borders.bottom, frame);
    assert_eq!(g[4][2].borders.bottom.color, Color::Auto);
}
