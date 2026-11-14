//! Typing right-to-left text (Arabic, Hebrew) in a text box.
//!
//! egui lays text out with the Unicode bidirectional algorithm: an Arabic line is shaped, drawn
//! right to left and given its carets in display order. [`layout`] is egui's text-box layout with
//! the rows of right-to-left paragraphs placed as the page will place them (flush right, or
//! left, centred or right as the box is aligned). Text without right-to-left characters is laid
//! out exactly as egui's own text box does.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, FontId, Galley, Ui};

use crate::bidi::is_rtl_script;

/// The galley for `text` in a multiline text box, laid out like egui's default with `font` and
/// `color`. `align` places the rows of right-to-left paragraphs (left, centre, right); `None` sets
/// them flush right, as right-to-left paragraphs are.
pub fn layout(ui: &Ui, text: &str, font: &FontId, color: Color32, wrap_width: f32, align: Option<egui::Align>) -> Arc<Galley> {
    let line_height = ui.fonts_mut(|f| f.row_height(font)) + ui.spacing().extra_text_line_spacing;
    let mut job = LayoutJob::simple(text.to_owned(), font.clone(), color, wrap_width);
    // As egui's text box: trailing spaces stay while typing.
    job.keep_trailing_whitespace = true;
    for section in &mut job.sections {
        section.format.line_height = Some(line_height);
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    if !wrap_width.is_finite() || !text.chars().any(is_rtl_script) {
        return galley;
    }
    let mut galley = Arc::unwrap_or_clone(galley);
    for placed in galley.rows.iter_mut().filter(|p| p.row.rtl_paragraph) {
        let free = (wrap_width - placed.row.size.x).max(0.0);
        placed.pos.x = match align.unwrap_or(egui::Align::RIGHT) {
            egui::Align::Min => 0.0,
            egui::Align::Center => free / 2.0,
            egui::Align::Max => free,
        };
    }
    galley.rect.min.x = galley.rect.min.x.min(0.0);
    galley.rect.max.x = galley.rect.max.x.max(galley.rect.min.x + wrap_width);
    galley.mesh_bounds = galley.mesh_bounds.union(galley.rect);
    Arc::new(galley)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::text::CCursor;

    /// Wrapped Arabic laid out for a box: one galley per alignment.
    fn laid(text: &str, align: Option<egui::Align>) -> Arc<Galley> {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::theme::installed_font_definitions(crate::theme::CjkPreference::Default));
        let mut galley = None;
        // Two frames: the fonts are installed during the first.
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                galley = Some(layout(ui, text, &FontId::proportional(20.0), Color32::BLACK, 120.0, align));
            });
            // No renderer takes the font texture here.
            output.textures_delta.clear();
        }
        galley.unwrap()
    }

    /// The caret of every position of wrapped Arabic lies on its own row, moving left as the text
    /// goes on, and the end of the text is at the left end of its last row.
    #[test]
    fn carets_follow_right_to_left_rows() {
        let text = "الى مصرف ئيش التركي المحترم";
        let galley = laid(text, Some(egui::Align::Min));
        assert!(galley.rows.len() > 1, "wraps: {}", galley.rows.len());
        let n = text.chars().count();
        let end = galley.pos_from_cursor(CCursor::new(n));
        let last = galley.rows.last().unwrap();
        assert!(end.center().y > last.min_y() && end.center().y < last.max_y(), "end on the last row: {end:?}");
        assert!(end.min.x <= last.pos.x + 2.0, "end at the row's left: {end:?} row at {:?}", last.pos);
        // Within the first row, carets move left.
        let first_len = galley.rows[0].glyphs.len();
        let xs: Vec<f32> = (0..first_len).map(|i| galley.pos_from_cursor(CCursor::new(i)).min.x).collect();
        assert!(xs.windows(2).all(|w| w[1] <= w[0] + 0.5), "{xs:?}");
    }

    /// Right-to-left rows sit where the box's alignment puts them: flush right by default.
    #[test]
    fn right_to_left_rows_follow_the_alignment() {
        let text = "الى مصرف ئيش التركي المحترم";
        let right = laid(text, None);
        for placed in &right.rows {
            assert!((placed.pos.x + placed.row.size.x - 120.0).abs() < 0.5, "flush right: {:?} {:?}", placed.pos, placed.row.size);
        }
        let left = laid(text, Some(egui::Align::Min));
        assert!(left.rows.iter().all(|p| p.pos.x == 0.0));
        let centre = laid(text, Some(egui::Align::Center));
        for placed in &centre.rows {
            assert!((placed.pos.x * 2.0 + placed.row.size.x - 120.0).abs() < 0.5, "centred: {:?}", placed.pos);
        }
        // Latin text is egui's own layout.
        let latin = laid("plain text", None);
        assert_eq!(latin.rows[0].pos.x, 0.0);
    }
}
