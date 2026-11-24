//! Reading the HTML flavour of the system clipboard on paste.
//!
//! Spreadsheets put an HTML table on the clipboard next to the tab-separated text. The text holds
//! what each cell *shows* ("Rp10,000.00"), which en-US input parsing can't always turn back into a
//! number. The HTML of the common sources also carries the underlying number as a cell attribute:
//! `x:num="10000"` (Excel, Excel for the web) and `sdval="10000"` (LibreOffice Calc). Paste uses
//! it to keep such cells numeric (#257). Only the table structure, the cell text and these two
//! attributes are read; everything else in the markup is ignored.

/// Largest clipboard HTML we look at; past this the paste uses the plain text alone.
const MAX_HTML: usize = 16 * 1024 * 1024;
/// Widest `colspan` honoured (the sheet's column count).
const MAX_SPAN: usize = gridcraft_core::MAX_COLS as usize;
/// Tallest `rowspan` honoured (the sheet's row count).
const MAX_ROW_SPAN: usize = gridcraft_core::MAX_ROWS as usize;
/// Most cells, span placeholders included, one paste reads from the HTML. Past it the rest of the
/// table is ignored: the HTML only refines cells of the tab-separated text, which stays the source.
const MAX_HTML_CELLS: usize = 1_000_000;

/// One table cell: its visible text, the number the source recorded for it, if any, and whether
/// it is a header cell (`<th>`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct HtmlCell {
    pub text: String,
    pub num: Option<f64>,
    pub header: bool,
}

/// The cells of the first table in `html`, row by row, read no further than `max_rows` rows of
/// `max_cols` cells (the pasted text's shape, itself clamped to the sheet) and [`MAX_HTML_CELLS`]
/// in all. A cell spanning columns is followed by empty cells, and a column covered by a `rowspan`
/// from above holds an empty cell, so positions line up with the tab-separated text. Empty when
/// there is no table.
pub(crate) fn table_cells(html: &str, max_rows: usize, max_cols: usize) -> Vec<Vec<HtmlCell>> {
    let mut grid = Grid::new(max_rows, max_cols);
    if html.len() > MAX_HTML {
        return grid.rows;
    }
    let mut cell: Option<OpenCell> = None;
    let mut in_table = false;
    let mut rest = html;
    while !rest.is_empty() && !grid.full {
        let Some(lt) = rest.find('<') else {
            if let Some(c) = cell.as_mut() {
                c.cell.text.push_str(rest);
            }
            break;
        };
        if let Some(c) = cell.as_mut() {
            c.cell.text.push_str(rest.get(..lt).unwrap_or_default());
        }
        rest = rest.get(lt..).unwrap_or_default();
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").and_then(|e| after.get(e + 3..)).unwrap_or_default();
            continue;
        }
        let Some(gt) = tag_end(rest) else { break };
        let tag = rest.get(1..gt).unwrap_or_default();
        rest = rest.get(gt + 1..).unwrap_or_default();
        let closing = tag.starts_with('/');
        let body = tag.trim_start_matches('/');
        let name_end = body.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(body.len());
        let name = body.get(..name_end).unwrap_or_default().to_ascii_lowercase();
        let attrs = body.get(name_end..).unwrap_or_default();
        match (name.as_str(), closing) {
            ("style" | "script", false) => {
                // Raw text: skip to the closing tag.
                let end = format!("</{name}");
                rest = find_ci(rest, &end).and_then(|i| rest.get(i..)).unwrap_or_default();
            }
            ("table", false) => {
                if in_table {
                    break; // a nested table: the outer one is the clipboard grid, stop here
                }
                in_table = true;
            }
            ("table", true) => {
                grid.finish(&mut cell);
                if !grid.rows.is_empty() {
                    break;
                }
                in_table = false;
            }
            ("tr", false) if in_table => {
                grid.finish(&mut cell);
                grid.start_row();
            }
            ("td" | "th", false) if in_table => {
                grid.finish(&mut cell);
                let num = attr(attrs, "x:num").or_else(|| attr(attrs, "sdval")).and_then(|v| v.trim().parse::<f64>().ok()).filter(|n| n.is_finite());
                let colspan = span(attrs, "colspan", MAX_SPAN);
                let rowspan = span(attrs, "rowspan", MAX_ROW_SPAN);
                cell = Some(OpenCell { cell: HtmlCell { text: String::new(), num, header: name == "th" }, colspan, rowspan });
            }
            ("td" | "th", true) | ("tr", true) => grid.finish(&mut cell),
            ("br", _) => {
                if let Some(c) = cell.as_mut() {
                    c.cell.text.push(' ');
                }
            }
            _ => {}
        }
    }
    grid.finish(&mut cell);
    grid.rows
}

/// A `colspan`/`rowspan` attribute: at least 1, at most `max` (the sheet's extent), so a hostile
/// "999999999999" is clamped before anything is expanded.
fn span(attrs: &str, name: &str, max: usize) -> usize {
    attr(attrs, name).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, max)
}

/// A cell whose closing tag hasn't been seen yet.
struct OpenCell {
    cell: HtmlCell,
    colspan: usize,
    rowspan: usize,
}

/// The table being read, with its size limits.
struct Grid {
    rows: Vec<Vec<HtmlCell>>,
    max_rows: usize,
    max_cols: usize,
    /// Cells still allowed before the table is cut short.
    budget: usize,
    /// Per column: the row index (exclusive) down to which a `rowspan` from above covers it.
    covered_until: Vec<usize>,
    /// The row or cell budget ran out: nothing more is read.
    full: bool,
}

impl Grid {
    fn new(max_rows: usize, max_cols: usize) -> Self {
        let max_rows = max_rows.min(MAX_ROW_SPAN);
        let max_cols = max_cols.min(MAX_SPAN);
        let budget = MAX_HTML_CELLS.min(max_rows.saturating_mul(max_cols));
        Grid { rows: Vec::new(), max_rows, max_cols, budget, covered_until: Vec::new(), full: false }
    }

    /// Opens a new row, or marks the grid full when the pasted text has no more rows.
    fn start_row(&mut self) {
        if self.rows.len() >= self.max_rows {
            self.full = true;
        } else {
            self.rows.push(Vec::new());
        }
    }

    /// Appends one cell to the current row; false when the row or the cell budget is used up.
    fn push(&mut self, cell: HtmlCell) -> bool {
        let Some(row) = self.rows.last_mut() else { return false };
        if row.len() >= self.max_cols {
            return false;
        }
        let Some(left) = self.budget.checked_sub(1) else {
            self.full = true;
            return false;
        };
        self.budget = left;
        row.push(cell);
        true
    }

    /// Closes the open cell (if any) into the current row, decoding its text: first past the
    /// columns a `rowspan` above still covers, then the cell, then a placeholder per extra column.
    fn finish(&mut self, open: &mut Option<OpenCell>) {
        let Some(OpenCell { mut cell, colspan, rowspan }) = open.take() else { return };
        if self.rows.is_empty() {
            self.start_row();
        }
        let Some(r) = self.rows.len().checked_sub(1) else { return };
        while self.covered_until.get(self.col()).is_some_and(|&until| until > r) {
            if !self.push(HtmlCell::default()) {
                return;
            }
        }
        let start = self.col();
        cell.text = decode(&cell.text);
        if !self.push(cell) {
            return;
        }
        for _ in 1..colspan {
            if !self.push(HtmlCell::default()) {
                break;
            }
        }
        if rowspan > 1 {
            let (end, until) = (self.col(), r.saturating_add(rowspan));
            if self.covered_until.len() < end {
                self.covered_until.resize(end, 0);
            }
            for u in self.covered_until.get_mut(start..end).into_iter().flatten() {
                *u = (*u).max(until);
            }
        }
    }

    /// The column the next cell of the current row lands in.
    fn col(&self) -> usize {
        self.rows.last().map_or(0, Vec::len)
    }
}

/// Byte index of the `>` that closes the tag `rest` starts with; a `>` inside a quoted attribute
/// value (an inline `mso-number-format` condition such as `[>=1000]`) doesn't end it.
fn tag_end(rest: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in rest.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// Case-insensitive ASCII search.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h.get(i..i + n.len()).is_some_and(|w| w.eq_ignore_ascii_case(n)))
}

/// The value of attribute `name` in a tag's attribute text (quoted or bare). A bare attribute
/// without `=` has no value.
fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let mut s = attrs;
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if s.is_empty() {
            return None;
        }
        let key_end = s.find(|c: char| c.is_whitespace() || c == '=' || c == '/').unwrap_or(s.len());
        let key = s.get(..key_end).unwrap_or_default();
        s = s.get(key_end..).unwrap_or_default().trim_start();
        let mut value = None;
        if let Some(after) = s.strip_prefix('=') {
            let after = after.trim_start();
            let (v, next) = match after.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let inner = after.get(1..).unwrap_or_default();
                    let end = inner.find(q).unwrap_or(inner.len());
                    (inner.get(..end).unwrap_or_default(), inner.get(end + 1..).unwrap_or_default())
                }
                _ => {
                    let end = after.find(char::is_whitespace).unwrap_or(after.len());
                    (after.get(..end).unwrap_or_default(), after.get(end..).unwrap_or_default())
                }
            };
            value = Some(v);
            s = next;
        }
        if key.eq_ignore_ascii_case(name) {
            return value;
        }
        if key.is_empty() && value.is_none() {
            // Stray character (e.g. a lone '='): skip it so the loop always advances.
            let mut chars = s.chars();
            chars.next();
            s = chars.as_str();
        }
    }
}

/// Entities are at most `&` + 8 chars + `;` (a decimal code point at most 7 digits deep), so
/// the semicolon is only looked for in the first 12 bytes — and `unwrap_or(rest)` never falls
/// back to scanning the remainder of the text either (that turn re-scans per `&`, quadratic
/// when a paste holds many bare `&`).
const ENTITY_WINDOW: usize = 12;

/// HTML text to plain text: entities decoded, whitespace runs (and `<br>`) collapsed to one
/// space, trimmed. The text only serves to match a cell with the tab-separated text.
fn decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(rest.get(..amp).unwrap_or_default());
        rest = rest.get(amp..).unwrap_or_default();
        // `.min(len)` (not an `unwrap_or(rest)`) — a `get(..12)` that fails on a multibyte
        // boundary used to fall back to the complete suffix, re-scanning per '&'.
        let semi = rest.get(..ENTITY_WINDOW.min(rest.len())).and_then(|w| w.find(';'));
        let entity = semi.and_then(|e| rest.get(1..e));
        let ch = entity.and_then(|e| match e {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => {
                let n = e
                    .strip_prefix("#x")
                    .or_else(|| e.strip_prefix("#X"))
                    .map(|h| u32::from_str_radix(h, 16))
                    .or_else(|| e.strip_prefix('#').map(str::parse::<u32>));
                n.and_then(Result::ok).and_then(char::from_u32)
            }
        });
        match (ch, semi) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = rest.get(e + 1..).unwrap_or_default();
            }
            _ => {
                out.push('&');
                rest = rest.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    let mut text = String::with_capacity(out.len());
    let mut space = false;
    for c in out.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            space = true;
            continue;
        }
        if space && !text.is_empty() {
            text.push(' ');
        }
        space = false;
        text.push(c);
    }
    text
}

/// Same-looking text: non-breaking spaces as spaces, whitespace runs collapsed, trimmed.
pub(crate) fn normalized(s: &str) -> String {
    s.replace('\u{a0}', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A number format that shows `n` exactly as `shown` ("Rp10,000.00" for 10000 →
/// `"Rp"#,##0.00`), inferred from the text around the digits. `None` unless the format renders
/// `shown` back character for character.
pub(crate) fn format_for(shown: &str, n: f64) -> Option<String> {
    let shown = normalized(shown);
    let start = shown.find(|c: char| c.is_ascii_digit())?;
    let end = shown.rfind(|c: char| c.is_ascii_digit()).map(|i| i + 1)?;
    let core = shown.get(start..end)?;
    let mut prefix = shown.get(..start)?.to_string();
    let suffix = shown.get(end..)?;
    if core.chars().any(|c| !(c.is_ascii_digit() || c == ',' || c == '.')) {
        return None;
    }
    if n < 0.0 {
        // A one-section format shows the minus sign itself, in front of everything.
        prefix = prefix.strip_prefix('-')?.to_string();
    }
    let decimals = core.split_once('.').map_or(0, |(_, f)| f.len());
    let mut code = String::new();
    let quote = |s: &str| if s.is_empty() { String::new() } else { format!("\"{}\"", s.replace('"', "")) };
    code.push_str(&quote(&prefix));
    code.push_str(if core.contains(',') { "#,##0" } else { "0" });
    if decimals > 0 {
        code.push('.');
        code.push_str(&"0".repeat(decimals.min(30)));
    }
    code.push_str(&quote(suffix));
    let rendered = gridcraft_numfmt::text_function(&gridcraft_core::Value::number(n), &code, gridcraft_core::date::DateSystem::D1900).ok()?;
    (normalized(&rendered) == shown).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_excel_and_libreoffice_cell_values() {
        let excel = r#"<html><head><style>td{mso-number-format:General}</style></head><body><table>
            <!--StartFragment--><tr height=20><td class=xl65 x:num="10000">Rp10,000.00</td><td x:str>Name</td></tr>
            <tr><td colspan=2 x:num="2.5">2.50 &amp; up</td><td>x</td></tr><!--EndFragment--></table></body></html>"#;
        let rows = table_cells(excel, 2, 3);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], HtmlCell { text: "Rp10,000.00".into(), num: Some(10000.0), ..HtmlCell::default() });
        assert_eq!(rows[0][1], HtmlCell { text: "Name".into(), num: None, ..HtmlCell::default() });
        assert_eq!(rows[1].len(), 3, "colspan keeps the next cell in its column");
        assert_eq!(rows[1][0].text, "2.50 & up");
        assert_eq!(rows[1][2].text, "x");

        let calc = r#"<table cellspacing="0" border="0"><colgroup width="85"></colgroup><tr>
            <td height="17" align="right" sdval="10000" sdnum="1033;0;[$Rp-421]#,##0.00"><font face="Liberation Sans">Rp10,000.00</font></td></tr></table>"#;
        assert_eq!(table_cells(calc, 1, 1), vec![vec![HtmlCell { text: "Rp10,000.00".into(), num: Some(10000.0), ..HtmlCell::default() }]]);

        let quoted_gt = r#"<table><tr><td style='mso-number-format:"\[>=1000\]0"' x:num="1500">1500</td></tr></table>"#;
        assert_eq!(table_cells(quoted_gt, 1, 1), vec![vec![HtmlCell { text: "1500".into(), num: Some(1500.0), ..HtmlCell::default() }]]);
    }

    #[test]
    fn entities_recognized_only_within_the_twelve_byte_window() {
        // The window never scans the rest of the text for a semicolon: a bare '&' followed by
        // long plain text stays a bare '&', and one '&' after another is linear.
        assert_eq!(decode("a&amplifier; b"), "a&amplifier; b");
        assert_eq!(decode("&ampampampampAMP;"), "&ampampampampAMP;");
        // Inside the window the usual forms still decode, and a trailing bare '&' too.
        assert_eq!(decode("&#38;"), "&");
        assert_eq!(decode("&#x266A;"), "\u{266A}");
        assert_eq!(decode("a&amp;b&nbsp;c"), "a&b\u{a0}c");
        assert_eq!(decode("amp&"), "amp&");
    }

    #[test]
    fn hostile_markup_does_not_panic() {
        for html in [
            "",
            "<",
            "<td",
            "<table><td x:num=>",
            "<table><tr><td colspan=999999999>a",
            "&#xFFFFFFFF;&",
            "<table><td x:num='1e999'>1</td>",
            "<!--",
            "<style>",
            "<table><table><td>",
        ] {
            let _ = table_cells(html, MAX_ROW_SPAN, MAX_SPAN);
        }
        assert_eq!(table_cells("<table><td x:num='1e999'>1</td></table>", 1, 1)[0][0].num, None);
        assert!(table_cells("<table><tr><td colspan=999999999>a</td></tr></table>", 1, usize::MAX)[0].len() <= MAX_SPAN);
    }

    #[test]
    fn huge_spans_stay_within_the_cell_budget() {
        // #2171: 100,000 rows of `colspan=16384` once expanded to ~1.6 billion cells (OOM). Even
        // with the sheet's full extent as the limit, at most MAX_HTML_CELLS are materialized.
        let row = "<tr><td colspan=16384 rowspan=99999999999 x:num='1'>1</td><td colspan=4294967296>2</td></tr>";
        let html = format!("<table>{}</table>", row.repeat(100_000));
        let rows = table_cells(&html, usize::MAX, usize::MAX);
        let cells: usize = rows.iter().map(Vec::len).sum();
        assert!(cells <= MAX_HTML_CELLS, "{cells} cells");
        assert!(rows.iter().all(|r| r.len() <= MAX_SPAN));
        assert_eq!(rows[0].len(), MAX_SPAN, "the first span fills the sheet's width");
        assert_eq!(rows[0][0], HtmlCell { text: "1".into(), num: Some(1.0), ..HtmlCell::default() });

        // Bounded by the pasted text's shape, the same markup yields just that shape.
        let rows = table_cells(&html, 3, 2);
        assert_eq!(rows.iter().map(Vec::len).collect::<Vec<_>>(), [2, 2, 2]);
        assert_eq!(rows[0][0].num, Some(1.0));
        assert!(table_cells(&html, 0, 5).is_empty());
        assert!(table_cells(&html, 4, 0).iter().all(Vec::is_empty));
    }

    #[test]
    fn rowspan_keeps_later_rows_in_their_columns() {
        // A merged A1:A2 is one `rowspan=2` cell; row 2 omits it, and its text has an empty field there.
        let html = r#"<table><tr><td rowspan=2 colspan=2>Merged</td><td x:num="1">1</td></tr>
            <tr><td x:num="2">2</td></tr><tr><td x:num="3">3</td><td>c</td></tr></table>"#;
        let rows = table_cells(html, 3, 3);
        assert_eq!(rows.iter().map(Vec::len).collect::<Vec<_>>(), [3, 3, 2]);
        assert_eq!(rows[0][2].num, Some(1.0));
        assert_eq!((rows[1][0].clone(), rows[1][1].clone()), (HtmlCell::default(), HtmlCell::default()));
        assert_eq!(rows[1][2].num, Some(2.0));
        assert_eq!(rows[2][0].num, Some(3.0), "the span ends after its second row");
    }

    #[test]
    fn infers_a_format_that_shows_the_same_text() {
        assert_eq!(format_for("Rp10,000.00", 10000.0).as_deref(), Some("\"Rp\"#,##0.00"));
        assert_eq!(format_for("1 234 kr", 1234.0), None, "space grouping is not reproduced");
        assert_eq!(format_for("10.000,00 €", 10000.0), None, "comma decimals don't render back in en-US");
        assert_eq!(format_for("-Rp5", -5.0).as_deref(), Some("\"Rp\"0"));
        assert_eq!(format_for("12 EUR", 12.0).as_deref(), Some("0\" EUR\""));
    }
}
