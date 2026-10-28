//! A table of text fields (Prepare a form ▸ Text field table): an area divided into rows and
//! columns, one text field in each cell, every column naming its cells from a pattern.
//!
//! The table is laid out in view space: points from the top-left of the page as displayed, after
//! `/Rotate`, so its rows run across the page as the person sees it whatever the page's rotation.
//! The engine maps each cell into user space and adds them all as one undoable edit.
//!
//! [`FieldTable::cells_for`] refuses a table [`add_field`](crate::add_field) would refuse part of
//! (a cell under [`MIN_FIELD_SIDE`], a name with a period, two cells with one name, a name the
//! form already has), so either all of the table is added or none of it is.

use std::collections::HashSet;
use std::fmt;

use crate::MIN_FIELD_SIDE;

/// The most rows a table can have.
pub const MAX_ROWS: usize = 100;
/// The most columns a table can have.
pub const MAX_COLUMNS: usize = 50;
/// The highest number the first row can count from.
pub const MAX_FIRST_ROW: usize = 99_999;

/// One column of a [`FieldTable`].
#[derive(Clone, Debug, PartialEq)]
pub struct TableColumn {
    /// How the column names its cells: see [`cell_name`].
    pub pattern: String,
    /// The column's share of the table's width, against the other columns' (any positive number).
    pub width: f64,
}

/// An area divided into rows and columns of text fields.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldTable {
    /// The area in view space, `[x0, y0, x1, y1]` with y down, corners in either order.
    pub rect: [f64; 4],
    pub rows: usize,
    pub columns: Vec<TableColumn>,
    /// Points left empty between two rows.
    pub row_gap: f64,
    /// Points left empty between two columns.
    pub column_gap: f64,
    /// The number the first row's names count from.
    pub first_row: usize,
}

/// One cell of a [`FieldTable`]: where its field goes (view space, `[x0, y0, x1, y1]` with y down)
/// and what it is called.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub rect: [f64; 4],
    pub name: String,
}

impl FieldTable {
    /// Four rows in one column filling `rect`, named after `base` (see [`column_pattern`]).
    pub fn new(rect: [f64; 4], base: &str) -> Self {
        FieldTable {
            rect,
            rows: 4,
            columns: vec![TableColumn { pattern: column_pattern(base, 1), width: 1.0 }],
            row_gap: 0.0,
            column_gap: 0.0,
            first_row: 1,
        }
    }

    /// Make the table `n` columns wide, `n` held to 1 to [`MAX_COLUMNS`]. Columns past `n` are
    /// dropped; a new one is named after `base` and is as wide as the others are on average, so
    /// the widths already set keep their proportions.
    pub fn set_columns(&mut self, n: usize, base: &str) {
        let n = n.clamp(1, MAX_COLUMNS);
        self.columns.truncate(n);
        let sum: f64 = self.columns.iter().map(|c| c.width).sum();
        let average = sum / self.columns.len().max(1) as f64;
        let width = if average.is_finite() && average > 0.0 { average } else { 1.0 };
        while self.columns.len() < n {
            let pattern = column_pattern(base, self.columns.len() + 1);
            self.columns.push(TableColumn { pattern, width });
        }
    }

    /// Where each cell goes and what it is called, row by row. Refuses a table out of bounds or
    /// with a cell under [`MIN_FIELD_SIDE`]; the names are not checked (see [`Self::cells_for`]).
    pub fn cells(&self) -> Result<Vec<Cell>, TableError> {
        let (rows, cols) = (self.rows, self.columns.len());
        if !(1..=MAX_ROWS).contains(&rows) {
            return Err(TableError::Rows);
        }
        if !(1..=MAX_COLUMNS).contains(&cols) {
            return Err(TableError::Columns);
        }
        if self.first_row > MAX_FIRST_ROW {
            return Err(TableError::FirstRow);
        }
        let gaps = [self.row_gap, self.column_gap];
        if !self.rect.iter().all(|v| v.is_finite())
            || !gaps.iter().all(|g| g.is_finite() && *g >= 0.0)
            || !self.columns.iter().all(|c| c.width.is_finite() && c.width > 0.0)
        {
            return Err(TableError::Numbers);
        }
        let r = self.rect;
        let (x0, y0, x1, y1) = (r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3]));
        // `rows` and `cols` are at most 100 and 50 from here on, so the counts below can't overflow
        // and are exact as floats.
        let row_height = (y1 - y0 - self.row_gap * (rows - 1) as f64) / rows as f64;
        let room = x1 - x0 - self.column_gap * (cols - 1) as f64;
        let total: f64 = self.columns.iter().map(|c| c.width).sum();
        // Corners far enough apart overflow to infinity.
        if !(row_height.is_finite() && room.is_finite() && total.is_finite()) {
            return Err(TableError::Numbers);
        }
        // Every number from here on is finite: the corners, gaps and widths are, and their sums were
        // checked above.
        if row_height < MIN_FIELD_SIDE {
            return Err(TableError::RowsTooShort);
        }
        let mut spans = Vec::with_capacity(cols);
        // The widths of the columns so far: each edge is worked out from the start, not from the
        // last edge, so rounding doesn't add up across the table.
        let mut before = 0.0;
        for (c, column) in self.columns.iter().enumerate() {
            let left = x0 + room * before / total + c as f64 * self.column_gap;
            before += column.width;
            // The last column ends at the table's edge, not where rounding puts it.
            let right = if c + 1 == cols { x1 } else { x0 + room * before / total + c as f64 * self.column_gap };
            if right - left < MIN_FIELD_SIDE {
                return Err(TableError::ColumnTooNarrow);
            }
            spans.push((left, right));
        }
        let mut cells = Vec::with_capacity(rows * cols);
        for row in 0..rows {
            let top = y0 + row as f64 * (row_height + self.row_gap);
            let bottom = if row + 1 == rows { y1 } else { top + row_height };
            if bottom - top < MIN_FIELD_SIDE {
                return Err(TableError::RowsTooShort);
            }
            for (c, (&(left, right), column)) in spans.iter().zip(&self.columns).enumerate() {
                // At most MAX_FIRST_ROW + MAX_ROWS * MAX_COLUMNS.
                let seq = self.first_row + row * cols + c;
                let name = cell_name(&column.pattern, self.first_row + row, c + 1, seq);
                cells.push(Cell { rect: [left, top, right, bottom], name });
            }
        }
        Ok(cells)
    }

    /// [`Self::cells`] for a form whose fields are called `existing` (full names), refusing a
    /// name with a period, a name two cells share and a name the form already has, as a field
    /// or as the parent of one.
    pub fn cells_for<'a>(&self, existing: impl IntoIterator<Item = &'a str>) -> Result<Vec<Cell>, TableError> {
        let cells = self.cells()?;
        // "a.b" also takes "a": a new field of that name would sit beside the parent of "a.b".
        let mut taken = HashSet::new();
        for name in existing {
            taken.insert(name);
            taken.extend(name.match_indices('.').filter_map(|(at, _)| name.get(..at)));
        }
        let mut seen = HashSet::with_capacity(cells.len());
        for cell in &cells {
            let name = cell.name.as_str();
            if name.contains('.') {
                return Err(TableError::Period(cell.name.clone()));
            }
            if !seen.insert(name) {
                return Err(TableError::Twice(cell.name.clone()));
            }
            if taken.contains(name) {
                return Err(TableError::Taken(cell.name.clone()));
            }
        }
        Ok(cells)
    }
}

/// The pattern a new table's column `column` (counted from 1) starts with: `{base}_N` for the
/// first, `{base}_c2_N`, `{base}_c3_N`, … after it.
pub fn column_pattern(base: &str, column: usize) -> String {
    if column <= 1 { format!("{base}_N") } else { format!("{base}_c{column}_N") }
}

/// The first of "Table1", "Table2", … that no field in `existing` (full names) starts with, so a
/// new table's names are free. One pass over the names, however many the form has.
pub fn free_base<'a>(existing: impl IntoIterator<Item = &'a str>) -> String {
    // The K of every "TableK" a name starts with: "Table1", "Table1_3" and "Table1.x" take 1, and
    // "Table12_1" takes 12, not 1.
    let taken: HashSet<u64> = existing
        .into_iter()
        .filter_map(|name| {
            let rest = name.strip_prefix("Table")?;
            let digits = rest.get(..rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len()))?;
            digits.parse().ok()
        })
        .collect();
    // One of the first `taken.len() + 1` is always free.
    let k = (1..=taken.len() as u64 + 1).find(|k| !taken.contains(k)).unwrap_or(1);
    format!("Table{k}")
}

/// What may stand on either side of an `N` that is the row number.
const SEPARATORS: [char; 4] = ['_', '-', '/', ':'];

/// The name `pattern` gives the cell in `row` and `column` (`column` counted from 1); `seq` is the
/// cell's number counting along the rows.
///
/// - `{row}`, `{r}`, `{n}` and `#`: the row number; `[row]`, `[r]` and `[n]`: the row number in
///   brackets (`item[N]` → `item[1]`);
/// - `{col}` and `{c}`, `[col]` and `[c]`: the column number;
/// - `{seq}` and `[seq]`: the cell number, counting along the rows from the table's first row
///   number, so a table starting at row 1 numbers its cells 1, 2, 3, …;
/// - with no row or cell number, an `N` or `n` standing alone after `_`, `-`, `/` or `:`
///   (`price_N`, `line-n-total`) is the row number, so the N in `Name` or `NET_N` stays;
/// - with no row or cell number still, the row number goes at the end.
///
/// Braces and brackets match ignoring case. Anything else is kept as written, a period included,
/// though no field name may contain one: [`FieldTable::cells_for`] refuses those.
pub fn cell_name(pattern: &str, row: usize, column: usize, seq: usize) -> String {
    let row_number = row.to_string();
    let mut out = String::with_capacity(pattern.len() + 8);
    // Whether the name has a row or cell number yet.
    let mut numbered = false;
    let mut rest = pattern.trim();
    while let Some(at) = rest.find(['{', '[', '#']) {
        // `at` is where an ASCII character starts, so both halves are whole characters.
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        if let Some(after) = from.strip_prefix('#') {
            out.push_str(&row_number);
            numbered = true;
            rest = after;
            continue;
        }
        let (open, close) = if from.starts_with('[') { ('[', ']') } else { ('{', '}') };
        let inside = from.get(1..).unwrap_or_default();
        let token = inside.split_once(close).and_then(|(word, after)| {
            let is = |name: &str| word.eq_ignore_ascii_case(name);
            if is("row") || is("r") || is("n") {
                Some((row, true, after))
            } else if is("seq") {
                Some((seq, true, after))
            } else if is("col") || is("c") {
                Some((column, false, after))
            } else {
                None
            }
        });
        match token {
            Some((n, counts, after)) => {
                if open == '[' {
                    out.push('[');
                }
                out.push_str(&n.to_string());
                if open == '[' {
                    out.push(']');
                }
                numbered |= counts;
                rest = after;
            }
            None => {
                out.push(open);
                rest = inside;
            }
        }
    }
    out.push_str(rest);
    if numbered {
        return out;
    }
    if out == "N" || out == "n" {
        return row_number;
    }
    let chars: Vec<char> = out.chars().collect();
    let mut named = String::with_capacity(out.len() + row_number.len());
    for (i, &ch) in chars.iter().enumerate() {
        let after_separator = i.checked_sub(1).and_then(|p| chars.get(p)).is_some_and(|c| SEPARATORS.contains(c));
        let before_separator = chars.get(i + 1).is_none_or(|c| SEPARATORS.contains(c));
        if matches!(ch, 'N' | 'n') && after_separator && before_separator {
            named.push_str(&row_number);
            numbered = true;
        } else {
            named.push(ch);
        }
    }
    if !numbered {
        named.push_str(&row_number);
    }
    named
}

/// Why a table can't be added. Each is a mistake a person can make, with the numbers and names
/// apart from the words, so an interface can say it in its own language with [`TableError::said`].
#[derive(Clone, Debug, PartialEq)]
pub enum TableError {
    /// Fewer than one row, or more than [`MAX_ROWS`].
    Rows,
    /// Fewer than one column, or more than [`MAX_COLUMNS`].
    Columns,
    /// A first row number past [`MAX_FIRST_ROW`].
    FirstRow,
    /// A corner, gap or width that is not a usable number.
    Numbers,
    /// The rows come out under [`MIN_FIELD_SIDE`] high.
    RowsTooShort,
    /// A column comes out under [`MIN_FIELD_SIDE`] wide.
    ColumnTooNarrow,
    /// A cell's name has a period, which would make it a child of another field.
    Period(String),
    /// Two cells would have this name.
    Twice(String),
    /// The form already has a field of this name.
    Taken(String),
}

const ROWS: &str = "a table has 1 to {max} rows";
const COLUMNS: &str = "a table has 1 to {max} columns";
const FIRST_ROW: &str = "the first row number can be at most {max}";
const NUMBERS: &str = "the area, the spacing and the column widths must be numbers, the spacing 0 or more and each width more than 0";
const ROWS_TOO_SHORT: &str = "the rows are too short: make the area taller, or use fewer rows or less row spacing";
const COLUMN_TOO_NARROW: &str =
    "a column is too narrow: make the area wider, give the column more of it, or use fewer columns or less column spacing";
const PERIOD: &str = "field names can't contain a period: {name}";
const TWICE: &str = "two cells would both be named {name}";
const TAKEN: &str = "a field named {name} already exists";

/// Every sentence a [`TableError`] says, in English with its holes still in it, for an interface
/// to translate; `wordings_are_said` keeps the list in step with the errors.
pub const WORDINGS: &[&str] = &[ROWS, COLUMNS, FIRST_ROW, NUMBERS, ROWS_TOO_SHORT, COLUMN_TOO_NARROW, PERIOD, TWICE, TAKEN];

/// How an interface says one of our wordings in its own language, holes and all: given the English
/// in [`WORDINGS`], the same wording in that language. [`in_english`] keeps the English.
pub type Words<'a> = &'a dyn Fn(&str) -> String;

/// The [`Words`] that translate nothing: the wording as it is written here.
pub fn in_english(wording: &str) -> String {
    wording.to_owned()
}

impl TableError {
    /// This error as one sentence, said in the language `words` translates into. The wording is
    /// translated and the numbers and names go in afterwards, in a single pass, so a name with
    /// braces in it is never read as a hole. With [`in_english`] this is the error's `Display`.
    pub fn said(&self, words: Words) -> String {
        let limit = |wording: &str, max: usize| say(&words(wording), &[("{max}", max.to_string())]);
        // A name goes in quoted, as `add_field` quotes it.
        let named = |wording: &str, name: &str| say(&words(wording), &[("{name}", format!("{name:?}"))]);
        match self {
            TableError::Rows => limit(ROWS, MAX_ROWS),
            TableError::Columns => limit(COLUMNS, MAX_COLUMNS),
            TableError::FirstRow => limit(FIRST_ROW, MAX_FIRST_ROW),
            TableError::Numbers => words(NUMBERS),
            TableError::RowsTooShort => words(ROWS_TOO_SHORT),
            TableError::ColumnTooNarrow => words(COLUMN_TOO_NARROW),
            TableError::Period(name) => named(PERIOD, name),
            TableError::Twice(name) => named(TWICE, name),
            TableError::Taken(name) => named(TAKEN, name),
        }
    }
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.said(&in_english))
    }
}

impl std::error::Error for TableError {}

/// A wording with its holes filled. A hole is a name in braces, such as `{max}`; what is not in
/// `values` is left as it is written.
fn say(wording: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(wording.len() + 8);
    let mut rest = wording;
    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        out.push_str(before);
        // `find` counts from the brace and `}` is one byte, so this is a char boundary.
        let Some(close) = from_brace.find('}') else {
            out.push_str(from_brace);
            return out;
        };
        let Some(hole) = from_brace.get(..=close) else { return out + from_brace };
        match values.iter().find(|(name, _)| *name == hole) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(hole),
        }
        rest = from_brace.get(close + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn table(rect: [f64; 4], rows: usize, patterns: &[&str]) -> FieldTable {
        FieldTable {
            rect,
            rows,
            columns: patterns.iter().map(|p| TableColumn { pattern: (*p).to_owned(), width: 1.0 }).collect(),
            row_gap: 0.0,
            column_gap: 0.0,
            first_row: 1,
        }
    }

    #[test]
    fn a_row_number_goes_where_the_pattern_says() {
        assert_eq!(cell_name("fieldCategory_N", 1, 1, 1), "fieldCategory_1");
        assert_eq!(cell_name("fieldCategory_N", 10, 1, 10), "fieldCategory_10");
        assert_eq!(cell_name("row_{n}", 3, 1, 3), "row_3");
        assert_eq!(cell_name("item-#", 5, 1, 5), "item-5");
        assert_eq!(cell_name("cell[N]", 2, 1, 2), "cell[2]");
        assert_eq!(cell_name("N", 4, 1, 4), "4");
        assert_eq!(cell_name("line-n-total", 6, 1, 6), "line-6-total");
        assert_eq!(cell_name("a:N/b", 7, 1, 7), "a:7/b");
    }

    #[test]
    fn rows_columns_and_cells_have_their_own_tokens() {
        assert_eq!(cell_name("item_{row}_{col}", 2, 3, 5), "item_2_3");
        assert_eq!(cell_name("cell_R{row}C{col}", 1, 4, 4), "cell_R1C4");
        assert_eq!(cell_name("grid[{r},{c}]", 3, 2, 8), "grid[3,2]");
        assert_eq!(cell_name("table_r{r}_c{c}", 4, 5, 20), "table_r4_c5");
        assert_eq!(cell_name("cell{seq}", 2, 2, 7), "cell7");
        assert_eq!(cell_name("cell[seq]", 2, 2, 7), "cell[7]");
        // Braces and brackets match ignoring case.
        assert_eq!(cell_name("x{ROW}y{Col}z[SEQ]", 2, 3, 9), "x2y3z[9]");
        // A column number alone leaves the row to be found or added.
        assert_eq!(cell_name("price{c}_N", 2, 3, 9), "price3_2");
        assert_eq!(cell_name("col{c}", 1, 2, 2), "col21");
    }

    #[test]
    fn without_a_row_token_the_row_number_goes_at_the_end() {
        assert_eq!(cell_name("", 1, 1, 1), "1");
        assert_eq!(cell_name("  ", 3, 1, 3), "3");
        assert_eq!(cell_name("test_", 1, 1, 1), "test_1");
        assert_eq!(cell_name("Phone_Number", 4, 1, 4), "Phone_Number4");
        // What isn't a token is kept as written.
        assert_eq!(cell_name("{x}_N", 1, 1, 1), "{x}_1");
        assert_eq!(cell_name("{row", 1, 1, 1), "{row1");
        assert_eq!(cell_name("a]b[", 2, 1, 2), "a]b[2");
        // A period is kept, for `cells_for` to refuse.
        assert_eq!(cell_name("test.", 1, 1, 1), "test.1");
    }

    #[test]
    fn an_n_inside_a_word_is_not_the_row_number() {
        assert_eq!(cell_name("Name_N", 1, 1, 1), "Name_1");
        assert_eq!(cell_name("ItemNumber_N", 2, 1, 2), "ItemNumber_2");
        assert_eq!(cell_name("NET_AMOUNT_N", 3, 1, 3), "NET_AMOUNT_3");
        assert_eq!(cell_name("CustomerName_{row}", 5, 1, 5), "CustomerName_5");
        assert_eq!(cell_name("NOTE_#", 6, 1, 6), "NOTE_6");
        assert_eq!(cell_name("BankName[N]", 7, 1, 7), "BankName[7]");
        assert_eq!(cell_name("PLAN_N", 8, 1, 8), "PLAN_8");
        assert_eq!(cell_name("line_n", 9, 1, 9), "line_9");
        assert_eq!(cell_name("Prix_é_N", 1, 1, 1), "Prix_é_1");
        assert_eq!(cell_name("名前_N", 2, 1, 2), "名前_2");
    }

    #[test]
    fn a_grid_fills_its_area_with_gaps_between_the_cells() {
        let mut t = table([100.0, 100.0, 300.0, 300.0], 2, &["cell_{r}_{c}", "cell_{r}_{c}"]);
        t.row_gap = 10.0;
        t.column_gap = 20.0;
        // Width (200 - 20) / 2 = 90, height (200 - 10) / 2 = 95, rows from the top down.
        let cells = t.cells().unwrap();
        let got: Vec<_> = cells.iter().map(|c| (c.rect, c.name.as_str())).collect();
        assert_eq!(
            got,
            [
                ([100.0, 100.0, 190.0, 195.0], "cell_1_1"),
                ([210.0, 100.0, 300.0, 195.0], "cell_1_2"),
                ([100.0, 205.0, 190.0, 300.0], "cell_2_1"),
                ([210.0, 205.0, 300.0, 300.0], "cell_2_2"),
            ]
        );
        // Corners in either order are the same area.
        t.rect = [300.0, 300.0, 100.0, 100.0];
        assert_eq!(t.cells().unwrap(), cells);
    }

    #[test]
    fn columns_share_the_width_by_their_widths() {
        let mut t = table([100.0, 100.0, 300.0, 300.0], 2, &["desc_N", "price_N"]);
        t.columns[0].width = 3.0;
        let cells = t.cells().unwrap();
        assert_eq!(cells[0].rect, [100.0, 100.0, 250.0, 200.0]);
        assert_eq!(cells[1].rect, [250.0, 100.0, 300.0, 200.0]);
        assert_eq!((cells[2].name.as_str(), cells[3].name.as_str()), ("desc_2", "price_2"));
        // However the shares round, the last column ends at the table's edge.
        let mut t = table([0.0, 0.0, 100.0, 50.0], 1, &["a", "b", "c"]);
        t.column_gap = 1.0 / 3.0;
        assert_eq!(t.cells().unwrap()[2].rect[2], 100.0);
    }

    #[test]
    fn rows_count_from_the_first_row_and_cells_along_the_rows() {
        let mut t = table([0.0, 0.0, 100.0, 100.0], 2, &["a_{seq}_{r}", "b_{seq}_{r}"]);
        t.first_row = 10;
        let names: Vec<_> = t.cells().unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["a_10_10", "b_11_10", "a_12_11", "b_13_11"]);
    }

    #[test]
    fn a_table_out_of_bounds_is_refused() {
        let ok = table([0.0, 0.0, 200.0, 200.0], 4, &["a_N"]);
        assert!(ok.cells().is_ok());
        let with = |f: &dyn Fn(&mut FieldTable)| {
            let mut t = ok.clone();
            f(&mut t);
            t.cells()
        };
        assert_eq!(with(&|t| t.rows = 0), Err(TableError::Rows));
        assert_eq!(with(&|t| t.rows = MAX_ROWS + 1), Err(TableError::Rows));
        assert_eq!(with(&|t| t.columns.clear()), Err(TableError::Columns));
        // 200 points hold 50 columns of exactly 4, and 199 don't.
        assert_eq!(with(&|t| t.set_columns(MAX_COLUMNS, "x")).map(|c| c.len()), Ok(4 * MAX_COLUMNS));
        assert_eq!(
            with(&|t| {
                t.rect[2] = 199.0;
                t.set_columns(MAX_COLUMNS, "x");
            }),
            Err(TableError::ColumnTooNarrow)
        );
        assert_eq!(with(&|t| t.columns = vec![t.columns[0].clone(); MAX_COLUMNS + 1]), Err(TableError::Columns));
        assert_eq!(with(&|t| t.first_row = MAX_FIRST_ROW + 1), Err(TableError::FirstRow));
        assert_eq!(with(&|t| t.rect[0] = f64::NAN), Err(TableError::Numbers));
        assert_eq!(with(&|t| t.rect = [-f64::MAX, 0.0, f64::MAX, 200.0]), Err(TableError::Numbers));
        assert_eq!(with(&|t| t.row_gap = -1.0), Err(TableError::Numbers));
        assert_eq!(with(&|t| t.column_gap = f64::INFINITY), Err(TableError::Numbers));
        assert_eq!(with(&|t| t.columns[0].width = 0.0), Err(TableError::Numbers));
        // 200 points hold 50 rows of exactly 4, not 51.
        assert_eq!(with(&|t| t.rows = 50).map(|c| c.len()), Ok(50));
        assert_eq!(with(&|t| t.rows = 51), Err(TableError::RowsTooShort));
        assert_eq!(with(&|t| t.row_gap = 70.0), Err(TableError::RowsTooShort));
        assert_eq!(with(&|t| t.rect = [0.0, 0.0, 3.0, 200.0]), Err(TableError::ColumnTooNarrow));
        // One column squeezed under 4 points by the others' widths.
        assert_eq!(
            with(&|t| t.columns = vec![TableColumn { pattern: "a_N".into(), width: 1.0 }, TableColumn { pattern: "b_N".into(), width: 99.0 }]),
            Err(TableError::ColumnTooNarrow)
        );
    }

    #[test]
    fn names_a_form_would_refuse_are_refused_before_anything_is_added() {
        let t = table([0.0, 0.0, 200.0, 200.0], 2, &["a_N", "b_N"]);
        assert!(t.cells_for(["c_1", "a_10"]).is_ok());
        assert_eq!(t.cells_for(["x", "b_2"]), Err(TableError::Taken("b_2".into())));
        // The parent of an existing field is taken too.
        assert_eq!(t.cells_for(["a_1.street"]), Err(TableError::Taken("a_1".into())));
        let same = table([0.0, 0.0, 200.0, 200.0], 2, &["item_N", "item_N"]);
        assert_eq!(same.cells_for([]), Err(TableError::Twice("item_1".into())));
        let dotted = table([0.0, 0.0, 200.0, 200.0], 2, &["row.{n}"]);
        assert_eq!(dotted.cells_for([]), Err(TableError::Period("row.1".into())));
    }

    #[test]
    fn a_new_table_takes_the_first_free_base() {
        assert_eq!(free_base([]), "Table1");
        assert_eq!(free_base(["Table1_1", "Table1_c2_4"]), "Table2");
        assert_eq!(free_base(["Table1", "Table2.x", "Table3_N"]), "Table4");
        // "Table12_1" belongs to Table12, not Table1.
        assert_eq!(free_base(["Table12_1", "Tables"]), "Table1");
        // As fast for a form with thousands of fields: one pass, not one per candidate.
        let many: Vec<String> = (1..=20_000).map(|k| format!("Table{k}_1")).collect();
        assert_eq!(free_base(many.iter().map(String::as_str)), "Table20001");
        let t = FieldTable::new([0.0, 0.0, 200.0, 200.0], &free_base(["Table1_1"]));
        let names: Vec<_> = t.cells_for(["Table1_1"]).unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["Table2_1", "Table2_2", "Table2_3", "Table2_4"]);
    }

    #[test]
    fn added_columns_keep_the_widths_already_set() {
        let mut t = FieldTable::new([0.0, 0.0, 400.0, 100.0], "T");
        t.columns[0].width = 2.0;
        t.set_columns(3, "T");
        let got: Vec<_> = t.columns.iter().map(|c| (c.pattern.as_str(), c.width)).collect();
        assert_eq!(got, [("T_N", 2.0), ("T_c2_N", 2.0), ("T_c3_N", 2.0)]);
        t.columns[2].width = 4.0;
        t.set_columns(4, "T");
        assert_eq!(t.columns[3].width, 8.0 / 3.0);
        t.set_columns(2, "T");
        assert_eq!(t.columns.len(), 2);
        // Held to the bounds.
        t.set_columns(0, "T");
        assert_eq!(t.columns.len(), 1);
        t.set_columns(usize::MAX, "T");
        assert_eq!(t.columns.len(), MAX_COLUMNS);
    }

    #[test]
    fn wordings_are_said() {
        let mistakes = [
            TableError::Rows,
            TableError::Columns,
            TableError::FirstRow,
            TableError::Numbers,
            TableError::RowsTooShort,
            TableError::ColumnTooNarrow,
            TableError::Period("a.1".into()),
            TableError::Twice("a_1".into()),
            TableError::Taken("a_1".into()),
        ];
        assert_eq!(WORDINGS.len(), mistakes.len(), "one wording per mistake");
        // Each mistake says one of the wordings: translated to a marker, only the values remain.
        for e in &mistakes {
            let marked = e.said(&|w| format!("<{}>", WORDINGS.iter().position(|x| *x == w).unwrap_or(usize::MAX)));
            assert!(!marked.contains(&usize::MAX.to_string()), "{e:?} says a wording that isn't listed");
            assert_eq!(e.said(&in_english), e.to_string());
        }
        assert_eq!(TableError::Rows.to_string(), "a table has 1 to 100 rows");
        assert_eq!(TableError::Taken("a_1".into()).to_string(), "a field named \"a_1\" already exists");
        // A name goes in as it stands, braces and all, and is never read as a hole.
        let other = |w: &str| if w == TAKEN { "{name} existe ya".to_owned() } else { w.to_owned() };
        assert_eq!(TableError::Taken("{max}".into()).said(&other), "\"{max}\" existe ya");
    }

    proptest! {
        /// Any pattern names every cell, and never panics.
        #[test]
        fn any_pattern_gives_a_name(pattern in "(\\PC|[{}\\[\\]#Nn_:/.-]){0,24}", row in 0usize..200_000, column in 1usize..60, seq in 0usize..300_000) {
            let name = cell_name(&pattern, row, column, seq);
            prop_assert!(!name.is_empty());
            prop_assert_eq!(name.trim(), name.as_str());
        }

        /// Any table either lays out cells of at least the smallest side inside its area, or says
        /// why not.
        #[test]
        fn any_table_lays_out_or_says_why(
            rect in prop::array::uniform4(-2000.0f64..2000.0),
            rows in 0usize..120,
            widths in prop::collection::vec(0.01f64..100.0, 0..60),
            row_gap in -1.0f64..50.0,
            column_gap in -1.0f64..50.0,
        ) {
            let t = FieldTable {
                rect,
                rows,
                columns: widths.iter().map(|w| TableColumn { pattern: "a{seq}".into(), width: *w }).collect(),
                row_gap,
                column_gap,
                first_row: 1,
            };
            if let Ok(cells) = t.cells_for([]) {
                prop_assert_eq!(cells.len(), rows * widths.len());
                let (x0, y0, x1, y1) = (rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3]));
                for c in &cells {
                    let [l, t, r, b] = c.rect;
                    prop_assert!(r - l >= MIN_FIELD_SIDE && b - t >= MIN_FIELD_SIDE, "{:?}", c);
                    prop_assert!(l >= x0 - 1e-6 && r <= x1 + 1e-6 && t >= y0 - 1e-6 && b <= y1 + 1e-6, "{:?}", c);
                }
            }
        }
    }
}
