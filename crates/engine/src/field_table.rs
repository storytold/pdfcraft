//! Prepare a form ▸ Text field table: one undoable edit adding a [`FieldTable`]'s cells as text
//! fields. The table is laid out and named in `pdfcraft_forms::table`; this maps its cells from
//! view space onto the page and refuses, before anything is added, what `add_field` would refuse.

pub use pdfcraft_forms::table::*;

use pdfcraft_forms::MIN_FIELD_SIDE;
use pdfcraft_render::PageInfo;

use crate::{Edit, FormField, NewField};

/// The edit's name in the history ("Undo Add text field table"), one of [`crate::BATCH_LABELS`].
pub const LABEL: &str = "Add text field table";

/// The edit adding `table`'s cells to `page` (0-based, shown as `info`) of a document whose
/// fields are `form`. Refused, with nothing added, when a cell would be too small on the page or
/// its name can't be used.
pub fn edit(table: &FieldTable, page: usize, info: &PageInfo, form: &[FormField]) -> Result<Edit, TableError> {
    let cells = table.cells_for(form.iter().map(|f| f.name.as_str()))?;
    // A page turned a quarter runs down the screen along user space's x.
    let sideways = matches!(info.rotation, 90 | 270);
    let mut edits = Vec::with_capacity(cells.len());
    for cell in cells {
        let [x0, y0, x1, y1] = cell.rect;
        let rect = info.view_rect_to_user([x0 as f32, y0 as f32, x1 as f32, y1 as f32]).map(f64::from);
        // A corner past `f32`'s range comes back infinite.
        if !rect.iter().all(|v| v.is_finite()) {
            return Err(TableError::Numbers);
        }
        // `add_field`'s own test, after the page's scale (`/UserUnit`) and rounding to `f32`.
        let (across, down) = if sideways { (rect[3] - rect[1], rect[2] - rect[0]) } else { (rect[2] - rect[0], rect[3] - rect[1]) };
        if down < MIN_FIELD_SIDE {
            return Err(TableError::RowsTooShort);
        }
        if across < MIN_FIELD_SIDE {
            return Err(TableError::ColumnTooNarrow);
        }
        edits.push(Edit::AddField { page, rect, kind: NewField::Text { multiline: false }, name: Some(cell.name) });
    }
    Ok(Edit::Batch { label: LABEL.to_owned(), edits })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;

    fn page(width: f32, height: f32, crop: [f32; 4], rotation: u16) -> PageInfo {
        PageInfo { width, height, label: "1".into(), crop, rotation }
    }

    fn grid(rect: [f64; 4], rows: usize, patterns: &[&str]) -> FieldTable {
        let mut t = FieldTable::new(rect, "unused");
        t.rows = rows;
        t.columns = patterns.iter().map(|p| TableColumn { pattern: (*p).to_owned(), width: 1.0 }).collect();
        t
    }

    #[test]
    fn a_table_is_added_and_undone_as_one_step() {
        let mut s = Session::new();
        let bytes = s.create_blank(300.0, 400.0, 1).unwrap();
        let id = s.open("t.pdf", None, bytes, None).unwrap();
        // 200 × 100 points from (50, 40) down the page as displayed: two rows of two.
        let table = grid([50.0, 40.0, 250.0, 140.0], 2, &["desc_N", "price_N"]);
        let add = |s: &Session| {
            let doc = s.get(id).unwrap();
            edit(&table, 0, &doc.info.pages[0], &doc.form)
        };
        s.apply(id, add(&s).unwrap()).unwrap();
        let doc = s.get(id).unwrap();
        let rect = |name: &str| doc.form.iter().find(|f| f.name == name).map(|f| f.widgets[0].rect);
        // User space has y up: the first row is the higher one.
        assert_eq!(rect("desc_1"), Some([50.0, 310.0, 150.0, 360.0]));
        assert_eq!(rect("price_1"), Some([150.0, 310.0, 250.0, 360.0]));
        assert_eq!(rect("desc_2"), Some([50.0, 260.0, 150.0, 310.0]));
        assert_eq!(rect("price_2"), Some([150.0, 260.0, 250.0, 310.0]));
        // Laid out again over the same names, it is refused before anything is added.
        assert_eq!(add(&s).err(), Some(TableError::Taken("desc_1".into())));
        assert_eq!(s.undo(id).unwrap().to_string(), LABEL);
        assert!(s.get(id).unwrap().form.is_empty(), "one undo takes the whole table back");
    }

    #[test]
    fn rows_run_down_a_turned_page_as_it_is_shown() {
        let mut s = Session::new();
        let bytes = s.create_blank(300.0, 400.0, 1).unwrap();
        let id = s.open("t.pdf", None, bytes, None).unwrap();
        s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        let doc = s.get(id).unwrap();
        let info = doc.info.pages[0].clone();
        assert_eq!((info.rotation, info.width, info.height), (90, 400.0, 300.0));
        let table = grid([40.0, 30.0, 240.0, 130.0], 2, &["a_N", "b_N"]);
        s.apply(id, edit(&table, 0, &info, &doc.form).unwrap()).unwrap();
        // Each field, taken back to the page as shown, is where its cell was laid out.
        let doc = s.get(id).unwrap();
        for cell in table.cells().unwrap() {
            let r = doc.form.iter().find(|f| f.name == cell.name).unwrap().widgets[0].rect;
            let (a, b) = (info.user_to_view(r[0] as f32, r[1] as f32), info.user_to_view(r[2] as f32, r[3] as f32));
            let shown = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])].map(f64::from);
            assert!(shown.iter().zip(cell.rect).all(|(s, c)| (s - c).abs() < 1e-3), "{}: {shown:?} vs {:?}", cell.name, cell.rect);
        }
    }

    #[test]
    fn a_cell_too_small_on_the_page_is_refused_along_the_right_side() {
        // Half a point of user space for each point shown across, a whole one down: a quarter
        // turn puts "across" on user space's y.
        let info = page(200.0, 100.0, [0.0, 0.0, 100.0, 100.0], 90);
        let narrow = grid([0.0, 0.0, 12.0, 12.0], 2, &["a_N", "b_N"]);
        assert!(narrow.cells().is_ok(), "6 × 6 shown");
        assert_eq!(edit(&narrow, 0, &info, &[]).err(), Some(TableError::ColumnTooNarrow));
        let unturned = page(100.0, 200.0, [0.0, 0.0, 100.0, 100.0], 0);
        assert_eq!(edit(&narrow, 0, &unturned, &[]).err(), Some(TableError::RowsTooShort));
        let roomy = page(100.0, 100.0, [0.0, 0.0, 100.0, 100.0], 90);
        assert!(edit(&narrow, 0, &roomy, &[]).is_ok());
    }

    /// A corner finite as an `f64` but past `f32`'s range lays out, then comes back from the page's
    /// `f32` mapping infinite: one such corner leaves a side infinite, not NaN, and the cell used to
    /// pass, for `add_field` to refuse it once the edit was applied.
    #[test]
    fn a_table_past_the_pages_number_range_is_refused_before_anything_is_added() {
        let huge = grid([0.0, 0.0, 1e39, 100.0], 2, &["a_N"]);
        assert!(huge.cells().is_ok(), "finite as laid out");
        for rotation in [0, 90] {
            let info = page(300.0, 400.0, [0.0, 0.0, 300.0, 400.0], rotation);
            assert_eq!(edit(&huge, 0, &info, &[]).err(), Some(TableError::Numbers), "/Rotate {rotation}");
        }
    }
}
