//! Table styles: how a cell inside a table looks (our own palettes, keyed by the familiar
//! `TableStyleLight1…21 / Medium1…28 / Dark1…11` names so files round-trip).

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{BorderLine, BorderStyle, Borders, Cell, Color, Fill, Formula, PatternType, Sheet, Style, Table, Workbook};

/// (family, number) from a style name, e.g. `TableStyleMedium2` → ("Medium", 2).
pub fn parse_style_name(name: &str) -> (&'static str, u32) {
    let rest = name.strip_prefix("TableStyle").unwrap_or(name);
    for fam in ["Light", "Medium", "Dark"] {
        if let Some(n) = rest.strip_prefix(fam) {
            let k = n.parse::<u32>().unwrap_or(2).max(1); // the gallery starts at 1; 0 would underflow `n - 1`
            return (fam, k);
        }
    }
    ("Medium", 2)
}

/// Accent theme slot (4..=9) for a style number; 1 and every 7th style are neutral (dk1).
fn accent_for(n: u32) -> Option<u8> {
    let k = (n.saturating_sub(1)) % 7;
    if k == 0 { None } else { Some(3 + k as u8) }
}

/// Fills, text colours and thin lines of a table style, per part of the table (the recipes in
/// plan/excel/table-style-spec.json). Band 1 is the first data row; unbanded rows use band 2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableLook {
    pub header_fill: Option<Color>,
    pub header_font: Color,
    pub band1: Option<Color>,
    pub band2: Option<Color>,
    pub body_font: Color,
    pub top: Option<Color>,
    pub header_bottom: Option<Color>,
    pub inner_h: Option<Color>,
    pub inner_v: Option<Color>,
    pub sides: Option<Color>,
    pub bottom: Option<Color>,
    /// The totals row's double top rule (the gallery doesn't show totals).
    pub total_top: Color,
}

pub fn look(style: &str) -> TableLook {
    let (fam, n) = parse_style_name(style);
    let acc = accent_for(n);
    // An accent tint, or the neutral column's colour.
    let a = |tint: i16, neutral: Color| acc.map_or(neutral, |a| Color::Theme(a, tint));
    let (black, white) = (Color::Theme(1, 0), Color::Theme(0, 0));
    let (grey, mid_grey) = (Color::Theme(0, -150), Color::Theme(0, -350));
    let main = a(0, black);
    let plain = TableLook {
        header_fill: None,
        header_font: black,
        band1: None,
        band2: None,
        body_font: black,
        top: None,
        header_bottom: None,
        inner_h: None,
        inner_v: None,
        sides: None,
        bottom: None,
        total_top: main,
    };
    match (fam, n.saturating_sub(1) / 7) {
        ("Light", 0) => {
            let text = a(-250, black);
            let line = Some(main);
            TableLook { header_font: text, body_font: text, band1: Some(a(800, grey)), top: line, header_bottom: line, bottom: line, ..plain }
        }
        ("Light", 1) => {
            let line = Some(main);
            TableLook { header_fill: line, header_font: white, top: line, inner_h: line, sides: line, bottom: line, ..plain }
        }
        ("Light", _) => {
            let line = Some(main);
            TableLook { band1: Some(a(800, grey)), top: line, header_bottom: line, inner_v: line, sides: line, bottom: line, ..plain }
        }
        ("Medium", 0) => {
            let line = Some(a(400, black));
            TableLook { header_fill: Some(main), header_font: white, band1: Some(a(800, grey)), top: line, sides: line, bottom: line, ..plain }
        }
        ("Medium", 1) => TableLook {
            header_fill: Some(main),
            header_font: white,
            band1: Some(a(600, mid_grey)),
            band2: Some(a(800, grey)),
            header_bottom: Some(white),
            inner_v: Some(white),
            ..plain
        },
        ("Medium", 2) => {
            // Accent header over neutral grey bands and black rules; the neutral style adds a grid.
            let line = Some(black);
            let grid = if acc.is_none() { line } else { None };
            let header_fill = Some(main);
            TableLook {
                header_fill,
                header_font: white,
                band1: Some(grey),
                top: line,
                header_bottom: line,
                inner_v: grid,
                sides: grid,
                bottom: line,
                total_top: black,
                ..plain
            }
        }
        ("Medium", _) => {
            let line = Some(a(400, black));
            let pale = Some(a(800, grey));
            TableLook { header_fill: pale, band1: Some(a(600, mid_grey)), band2: pale, top: line, inner_v: line, sides: line, bottom: line, ..plain }
        }
        ("Dark", 0) => TableLook {
            header_fill: Some(black),
            header_font: white,
            band1: Some(a(-250, Color::Theme(1, 250))),
            band2: Some(a(0, Color::Theme(1, 450))),
            body_font: white,
            header_bottom: Some(white),
            total_top: white,
            ..plain
        },
        _ if n == 8 => TableLook { header_fill: Some(black), header_font: white, band1: Some(mid_grey), band2: Some(grey), ..plain },
        _ => {
            // Dark 9-11 pair accents 1+2, 3+4 and 5+6: the header takes the even one.
            let k = n.clamp(9, 11) as u8 - 8;
            let (head, body) = (Color::Theme(3 + 2 * k, 0), 2 + 2 * k);
            let band1 = Some(Color::Theme(body, 600));
            TableLook { header_fill: Some(head), header_font: white, band1, band2: Some(Color::Theme(body, 800)), total_top: head, ..plain }
        }
    }
}

/// Which parts of a table are on (the Table Style Options checkboxes).
#[derive(Clone, Copy, Debug)]
pub struct TableParts {
    pub header_row: bool,
    pub totals_row: bool,
    pub banded_rows: bool,
    pub banded_cols: bool,
    pub first_col: bool,
    pub last_col: bool,
}

impl TableParts {
    pub fn of(t: &Table) -> TableParts {
        TableParts {
            header_row: t.header_row,
            totals_row: t.totals_row,
            banded_rows: t.banded_rows,
            banded_cols: t.banded_cols,
            first_col: t.first_col,
            last_col: t.last_col,
        }
    }
}

/// What a table style paints on one cell of a `rows` x `cols` table (`row`, `col` from its
/// top-left). Each line has one owner: a horizontal line is a cell's bottom (top only on the
/// first row and the totals row), a vertical one a cell's right (left only on the first column).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellLook {
    pub fill: Option<Color>,
    pub font: Color,
    pub bold: bool,
    pub borders: Borders,
}

pub fn cell_look(lk: &TableLook, p: TableParts, row: u32, col: u32, rows: u32, cols: u32) -> CellLook {
    let last_row = rows.saturating_sub(1);
    let last_col = cols.saturating_sub(1);
    let is_header = p.header_row && row == 0;
    let is_total = p.totals_row && row == last_row && !is_header;
    let thin = |c: Option<Color>| c.map(|color| BorderLine { style: BorderStyle::Thin, color }).unwrap_or_default();
    let mut b = Borders::default();
    if row == 0 {
        b.top = thin(lk.top);
    }
    b.bottom = thin(if row == last_row {
        lk.bottom
    } else if is_header {
        lk.header_bottom
    } else if p.totals_row && row.saturating_add(1) == last_row {
        None // the totals row draws its own top rule
    } else {
        lk.inner_h
    });
    if col == 0 {
        b.left = thin(lk.sides);
    }
    b.right = thin(if col == last_col { lk.sides } else { lk.inner_v });
    let mut out = CellLook { borders: b, font: lk.body_font, ..Default::default() };
    if is_header {
        out.fill = lk.header_fill;
        out.font = lk.header_font;
        out.bold = true;
    } else if is_total {
        out.fill = lk.band2;
        out.bold = true;
        out.borders.top = BorderLine { style: BorderStyle::Double, color: lk.total_top };
    } else {
        let data_row = row.saturating_sub(p.header_row as u32);
        let band1 = (p.banded_rows && data_row.is_multiple_of(2)) || (p.banded_cols && col.is_multiple_of(2));
        out.fill = if band1 { lk.band1 } else { lk.band2 };
    }
    if (p.first_col && col == 0) || (p.last_col && col == last_col) {
        out.bold = true;
    }
    out
}

/// The style a table gives a cell (merged over the cell's own style: explicit fills, fonts and
/// borders win).
pub fn table_cell_style(wb: &Workbook, sheet: usize, t: &Table, c: CellRef) -> Style {
    let own = wb.sheet(sheet).map(|sh| wb.styles.get(sh.style_id(c)).clone()).unwrap_or_default();
    let (start, end) = (t.range.start, t.range.end);
    let rows = end.row.saturating_sub(start.row).saturating_add(1);
    let cols = end.col.saturating_sub(start.col).saturating_add(1);
    let lk = cell_look(&look(&t.style), TableParts::of(t), c.row.saturating_sub(start.row), c.col.saturating_sub(start.col), rows, cols);
    let mut s = own.clone();
    if let Some(f) = lk.fill
        && own.fill.pattern == PatternType::None
    {
        s.fill = Fill::solid(f);
    }
    if own.font.color == Color::Auto {
        s.font.color = lk.font;
    }
    s.font.bold = s.font.bold || lk.bold;
    for (side, line) in [
        (&mut s.borders.top, lk.borders.top),
        (&mut s.borders.bottom, lk.borders.bottom),
        (&mut s.borders.left, lk.borders.left),
        (&mut s.borders.right, lk.borders.right),
    ] {
        if side.is_none() {
            *side = line;
        }
    }
    s
}

/// A table's first and last visible row and column (hidden or filtered lines skipped).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShownEdges {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

/// Finds [`ShownEdges`]. It scans the hidden lines at each edge, so the grid and print call it
/// once per table per paint (through [`crate::cf::CfCache::table_edges`]), not once per cell.
pub fn shown_edges(sh: &Sheet, t: &Table) -> ShownEdges {
    let (start, end) = (t.range.start, t.range.end);
    let row_shown = |r: &u32| sh.row_height(*r) > 0.0;
    let col_shown = |k: &u32| sh.col_width(*k) > 0.0;
    // With every row (or column) hidden, each cell takes both edges' lines; nothing is drawn.
    ShownEdges {
        top: (start.row..=end.row).find(row_shown).unwrap_or(end.row),
        bottom: (start.row..=end.row).rev().find(row_shown).unwrap_or(start.row),
        left: (start.col..=end.col).find(col_shown).unwrap_or(end.col),
        right: (start.col..=end.col).rev().find(col_shown).unwrap_or(start.col),
    }
}

/// The style the grid and print draw for a table cell. Like [`table_cell_style`], but the
/// table's outer frame follows hidden rows and columns: the last visible row takes the table's
/// bottom line, the last visible column its right side, and the first visible row and column its
/// top line and left side. (Each frame line is owned by the edge cell, which is never drawn while
/// it is hidden or filtered out.) Convert to Range uses [`table_cell_style`], the table as it is.
pub fn table_cell_shown_style(wb: &Workbook, sheet: usize, t: &Table, e: ShownEdges, c: CellRef) -> Style {
    let mut s = table_cell_style(wb, sheet, t, c);
    let Some(sh) = wb.sheet(sheet) else { return s };
    let (start, end) = (t.range.start, t.range.end);
    let below = c.row < end.row && c.row >= e.bottom;
    let above = c.row > start.row && c.row <= e.top;
    let right = c.col < end.col && c.col >= e.right;
    let left = c.col > start.col && c.col <= e.left;
    if !(below || above || right || left) {
        return s;
    }
    let own = wb.styles.get(sh.style_id(c)).borders;
    if below && own.bottom.is_none() {
        s.borders.bottom = table_cell_style(wb, sheet, t, CellRef::new(end.row, c.col)).borders.bottom;
    }
    if above && own.top.is_none() {
        s.borders.top = table_cell_style(wb, sheet, t, CellRef::new(start.row, c.col)).borders.top;
    }
    if right && own.right.is_none() {
        s.borders.right = table_cell_style(wb, sheet, t, CellRef::new(c.row, end.col)).borders.right;
    }
    if left && own.left.is_none() {
        s.borders.left = table_cell_style(wb, sheet, t, CellRef::new(c.row, start.col)).borders.left;
    }
    s
}

/// Gallery list of style names.
pub fn gallery() -> Vec<String> {
    let mut v = Vec::new();
    for n in 1..=21 {
        v.push(format!("TableStyleLight{n}"));
    }
    for n in 1..=28 {
        v.push(format!("TableStyleMedium{n}"));
    }
    for n in 1..=11 {
        v.push(format!("TableStyleDark{n}"));
    }
    v
}

/// Editing a table's header row renames the column, as in Excel: an empty or repeated header
/// gets a unique name written back into the cell, and structured references follow the rename.
pub(crate) fn sync_headers(cx: &mut crate::cmd::Ctx) {
    if cx.wb.sheets.iter().all(|s| s.tables.is_empty()) {
        return;
    }
    // (sheet, table name, table range, old column name, new column name)
    let mut renames: Vec<(usize, String, RangeRef, String, String)> = Vec::new();
    let heads: Vec<(usize, RangeRef)> = (0..cx.wb.sheets.len())
        .flat_map(|si| cx.wb.sheet(si).into_iter().flat_map(move |sh| sh.tables.iter().filter(|t| t.header_row).map(move |t| (si, t.range))))
        .map(|(si, r)| (si, RangeRef::new(r.start, CellRef::new(r.start.row, r.end.col))))
        .collect();
    // A structural edit can move anything into a header row: then every header is checked.
    let changed: std::collections::BTreeSet<(usize, CellRef)> = if cx.structural {
        heads.iter().flat_map(|(s, r)| r.iter().map(move |c| (*s, c))).collect()
    } else {
        cx.changed.iter().filter(|(si, c)| heads.iter().any(|(s, r)| s == si && r.contains(*c))).copied().collect()
    };
    // Per table: lower-case column name → column, and the next number to try for a repeated name
    // (both kept up to date, so a wide header row stays linear).
    type Names = (std::collections::HashMap<String, usize>, std::collections::HashMap<String, usize>);
    let mut names: std::collections::HashMap<(usize, usize), Names> = std::collections::HashMap::new();
    for (si, c) in changed {
        let Some(sh) = cx.wb.sheet(si) else { continue };
        let Some(ti) = sh.tables.iter().position(|t| t.header_row && c.row == t.range.start.row && t.range.contains(c)) else { continue };
        let Some(t) = sh.tables.get(ti) else { continue };
        let col = (c.col - t.range.start.col) as usize;
        let Some(old) = t.columns.get(col).map(|x| x.name.clone()) else { continue };
        // A text header names its column as it is: a number format only changes how it looks (an
        // accounting format pads text with spaces), and the file writes the name into the cell.
        let value = sh.value(c);
        let text = match value.as_text() {
            Some(t) => t.to_string(),
            None => crate::display::cell_text(&cx.wb, sh, c),
        };
        // Headers hold constant text: a formula (or a spilled value) there becomes its text.
        let is_text = value.is_text() && sh.cell(c).is_some_and(|x| x.formula.is_none());
        if text == old && is_text {
            continue;
        }
        let (taken, next) = names
            .entry((si, ti))
            .or_insert_with(|| (t.columns.iter().enumerate().map(|(i, x)| (x.name.to_lowercase(), i)).collect(), Default::default()));
        if taken.get(&old.to_lowercase()) == Some(&col) {
            taken.remove(&old.to_lowercase());
        }
        let base = if text.trim().is_empty() { format!("Column{}", col + 1) } else { text.clone() };
        let k = next.entry(base.to_lowercase()).or_insert(2);
        let mut name = base.clone();
        while taken.contains_key(&name.to_lowercase()) {
            name = format!("{base}{k}");
            *k += 1;
        }
        taken.insert(name.to_lowercase(), col);
        let (table, range) = (t.name.clone(), t.range);
        let Some(sh) = cx.wb.sheet_mut(si) else { continue };
        if let Some(x) = sh.tables.get_mut(ti).and_then(|t| t.columns.get_mut(col)) {
            x.name = name.clone();
        }
        // Headers are text: a number typed there, or a generated name, is written back as text.
        if name != text || !is_text {
            let style = sh.style_id(c);
            sh.cells.set(c, Cell { value: gridcraft_core::Value::text(name.as_str()), style, ..Default::default() });
        }
        if !old.eq_ignore_ascii_case(&name) {
            renames.push((si, table, range, old, name));
        }
    }
    if renames.is_empty() {
        return;
    }
    // Structured references follow the new names.
    for i in 0..cx.wb.sheets.len() {
        let formulas: Vec<(CellRef, std::sync::Arc<Formula>)> =
            cx.wb.sheets.get(i).map(|s| s.cells.iter().filter_map(|(c, x)| x.formula.clone().map(|f| (c, f))).collect()).unwrap_or_default();
        for (c, f) in formulas {
            let Some(e) = f.expr() else { continue };
            let mut hit = false;
            let ne = e.map(&mut |x| match x {
                gridcraft_formula::Expr::Struct(mut st) => {
                    for (si, table, range, old, name) in &renames {
                        let this = if st.table.is_empty() { i == *si && range.contains(c) } else { st.table.eq_ignore_ascii_case(table) };
                        if this {
                            for end in [&mut st.col_start, &mut st.col_end].into_iter().flatten() {
                                if end.eq_ignore_ascii_case(old) {
                                    *end = name.clone();
                                    hit = true;
                                }
                            }
                        }
                    }
                    gridcraft_formula::Expr::Struct(st)
                }
                o => o,
            });
            if hit && let Some(cell) = cx.wb.sheet_mut(i).and_then(|s| s.cells.get_mut(c)) {
                cell.formula = Some(std::sync::Arc::new(Formula::from_expr(ne)));
            }
        }
    }
    cx.structural = true;
}

#[cfg(test)]
mod tests {
    use gridcraft_core::Value;
    use serde_json::json;

    use crate::Session;

    #[test]
    fn editing_a_header_renames_the_column() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [["a", "b", "c"], [1, 2, 3]]})).unwrap();
        s.execute("insert.table", json!({"range": "A1:C2", "header": true})).unwrap();
        s.execute("cell.set", json!({"cell": "E1", "input": "=SUM(Table1[a])"})).unwrap();
        s.execute("cell.set", json!({"cell": "A1", "input": "zz"})).unwrap();
        s.execute("edit.clearContents", json!({"range": "B1"})).unwrap();
        s.execute("cell.set", json!({"cell": "C1", "input": "ZZ"})).unwrap();
        let get = |s: &mut Session, a: &str| s.execute("cell.get", json!({"cell": a})).unwrap();
        assert_eq!(get(&mut s, "E1")["formula"], "=SUM(Table1[zz])");
        assert_eq!(get(&mut s, "E1")["value"], json!(1.0));
        let d = s.doc().unwrap();
        let sh = d.wb.sheet(0).unwrap();
        let names: Vec<&str> = sh.tables[0].columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["zz", "Column2", "ZZ2"]);
        assert_eq!(sh.value(gridcraft_core::CellRef::new(0, 1)), Value::from("Column2"));
        assert_eq!(sh.value(gridcraft_core::CellRef::new(0, 2)), Value::from("ZZ2"));
        s.execute("range.setValues", json!({"range": "A1", "values": [["x", "x", 5]]})).unwrap();
        let sh = s.doc().unwrap().wb.sheet(0).unwrap();
        let names: Vec<&str> = sh.tables[0].columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["x", "x2", "5"]);
        assert_eq!(sh.value(gridcraft_core::CellRef::new(0, 2)), Value::from("5"));
    }

    #[test]
    fn go_to_a_table_selects_its_data() {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1", "values": [["a", "b"], [1, 2], [3, 4]]})).unwrap();
        s.execute("insert.table", json!({"range": "A1:B3", "header": true})).unwrap();
        assert_eq!(s.execute("edit.goTo", json!({"reference": "Table1"})).unwrap()["selection"], "A2:B3");
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;

    #[test]
    fn zero_numbered_styles_do_not_underflow() {
        assert_eq!(parse_style_name("TableStyleLight0"), ("Light", 1));
        assert_eq!(parse_style_name("TableStyleMedium0"), ("Medium", 1));
        for name in ["TableStyleLight0", "TableStyleMedium0", "TableStyleDark0"] {
            let _ = look(name);
        }
    }
}
