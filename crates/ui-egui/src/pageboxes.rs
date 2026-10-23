//! Set Page Boxes (Acrobat: Organize pages ▸ Set page boxes / Crop pages ▸ double-click), M4.4.
//!
//! Choose a box (crop, trim, bleed, art, media), margins from the media box in points, inches,
//! millimetres or centimetres, and the pages. A preview shows the current page's media box and the new box.
//! Remove White Margins crops each page to its own content instead; the margins then show the
//! current page's.

use std::sync::{Arc, Mutex};

use egui::{Align, Color32, CornerRadius, Layout, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::{BoxSpec, DocId, Document, Edit, PageBox};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

pub use crate::units::Unit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    All,
    Current,
    /// 1-based, inclusive.
    Pages(usize, usize),
}

/// Dialog state. Margins are kept in points: left, bottom, right, top.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxesDraft {
    pub which: PageBox,
    pub unit: Unit,
    pub margins: [f64; 4],
    pub range: Range,
    /// Remove White Margins: each page's box goes to the bounds of its own content.
    pub remove_white: bool,
    /// The draft was seeded from this page and box (re-seed when either changes).
    pub seeded: Option<(usize, PageBox)>,
}

impl Default for BoxesDraft {
    fn default() -> Self {
        Self { which: PageBox::Crop, unit: Unit::Inches, margins: [0.0; 4], range: Range::All, remove_white: false, seeded: None }
    }
}

impl BoxesDraft {
    /// The pages (0-based) the dialog applies to.
    pub fn pages(&self, current: usize, count: usize) -> Vec<usize> {
        match self.range {
            Range::All => (0..count).collect(),
            Range::Current => vec![current.min(count.saturating_sub(1))],
            Range::Pages(a, b) => (a.max(1) - 1..b.min(count)).collect(),
        }
    }

    pub fn edit(&self, current: usize, count: usize) -> Edit {
        let spec =
            if self.margins.iter().all(|m| *m == 0.0) && self.which != PageBox::Media { BoxSpec::Remove } else { BoxSpec::Margins(self.margins) };
        Edit::SetPageBox { pages: self.pages(current, count), which: self.which, spec }
    }
}

fn box_index(b: PageBox) -> usize {
    match b {
        PageBox::Media => 0,
        PageBox::Crop => 1,
        PageBox::Bleed => 2,
        PageBox::Trim => 3,
        PageBox::Art => 4,
    }
}

/// The margins to show for `current`: those of its content when removing white margins (a blank
/// page shows its box), otherwise those of its chosen box.
fn seed_margins(doc: &Document, boxes: &[[[f64; 4]; 5]], current: usize, d: &BoxesDraft) -> Option<[f64; 4]> {
    let m = boxes.get(current)?[0];
    let to = |x: [f64; 4]| [x[0] - m[0], x[1] - m[1], m[2] - x[2], m[3] - x[3]].map(|v| (v.max(0.0) * 100.0).round() / 100.0);
    if let Some(content) = d.remove_white.then(|| doc.content_bounds(current)).flatten() {
        return Some(to(content));
    }
    Some(if d.which == PageBox::Media { [0.0; 4] } else { to(boxes.get(current)?[box_index(d.which)]) })
}

/// Draw the dialog body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let Some((i, id)) = app.active_ids() else { return (false, true) };
    let current = app.views[i].current;
    let Some(doc) = app.session.get(id) else { return (false, true) };
    let boxes = doc.page_boxes();
    let count = boxes.len().max(1);
    let units = app.units;
    let d = &mut app.boxes_draft;
    // Opening the dialog starts in the unit chosen in Preferences.
    if d.seeded.is_none() {
        d.unit = units;
        d.remove_white = false;
    }
    // Seed the margins from the current page's box (or, removing white margins, its content) when
    // the dialog opens or the box changes.
    if d.seeded != Some((current, d.which))
        && let Some(m) = seed_margins(doc, &boxes, current, d)
    {
        d.margins = m;
        d.seeded = Some((current, d.which));
    }
    ui.label(egui::RichText::new(tl!("Set Page Boxes")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            egui::Grid::new("boxes-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tl!("Box"));
                let names = [
                    (PageBox::Crop, "CropBox"),
                    (PageBox::Trim, "TrimBox"),
                    (PageBox::Bleed, "BleedBox"),
                    (PageBox::Art, "ArtBox"),
                    (PageBox::Media, "MediaBox"),
                ];
                let cur = names.iter().find(|(b, _)| *b == d.which).map_or("CropBox", |(_, l)| *l);
                egui::ComboBox::from_id_salt("boxes-which").selected_text(cur).show_ui(ui, |ui| {
                    for (b, l) in names {
                        ui.selectable_value(&mut d.which, b, l);
                    }
                });
                ui.end_row();
                ui.label(tl!("Units"));
                egui::ComboBox::from_id_salt("boxes-unit").selected_text(d.unit.label()).show_ui(ui, |ui| {
                    for u in Unit::ALL {
                        ui.selectable_value(&mut d.unit, u, u.label());
                    }
                });
                ui.end_row();
                let k = d.unit.per_point();
                for (label, idx) in [(tl!("Top"), 3), (tl!("Bottom"), 1), (tl!("Left"), 0), (tl!("Right"), 2)] {
                    let l = ui.label(label);
                    let mut v = d.margins[idx] * k;
                    let speed = d.unit.speed();
                    let r = ui
                        .add(egui::DragValue::new(&mut v).speed(speed).range(0.0..=10_000.0).max_decimals(3).suffix(format!(" {}", d.unit.label())))
                        .labelled_by(l.id);
                    if r.changed() {
                        d.margins[idx] = (v / k).max(0.0);
                        d.remove_white = false;
                    }
                    ui.end_row();
                }
                ui.label("");
                if ui.add(widgets::checkbox(&mut d.remove_white, tl!("Remove white margins"))).changed()
                    && let Some(m) = seed_margins(doc, &boxes, current, d)
                {
                    d.margins = m;
                }
                ui.end_row();
                ui.label("");
                if ui.button(tl!("Set to zero")).clicked() {
                    d.margins = [0.0; 4];
                    d.remove_white = false;
                }
                ui.end_row();
                ui.label(tl!("Pages"));
                ui.vertical(|ui| {
                    widgets::radio_value(ui, &mut d.range, Range::All, tl!("All"));
                    widgets::radio_value(
                        ui,
                        &mut d.range,
                        Range::Current,
                        crate::i18n::fmt(tl!("Current page ({n})"), &[("n", &(current + 1).to_string())]),
                    );
                    let (mut a, mut b) = match d.range {
                        Range::Pages(a, b) => (a, b),
                        _ => (1, count),
                    };
                    ui.horizontal(|ui| {
                        let on = matches!(d.range, Range::Pages(..));
                        if ui.add(widgets::radio(on, tl!("From"))).clicked() {
                            d.range = Range::Pages(a, b);
                        }
                        let ra = ui.add(egui::DragValue::new(&mut a).range(1..=count));
                        ui.label(tl!("to"));
                        let rb = ui.add(egui::DragValue::new(&mut b).range(1..=count));
                        if ra.changed() || rb.changed() {
                            d.range = Range::Pages(a.min(b), b.max(a));
                        }
                    });
                });
                ui.end_row();
            });
        });
        ui.add_space(16.0);
        // Preview: the media box and the new box, to scale.
        if let Some(b) = boxes.get(current) {
            let m = b[0];
            let (w, h) = ((m[2] - m[0]).max(1.0), (m[3] - m[1]).max(1.0));
            let s = (180.0 / w.max(h)) as f32;
            let (r, _) = ui.allocate_exact_size(vec2(200.0, 200.0), egui::Sense::hover());
            let page = Rect::from_center_size(r.center(), vec2(w as f32 * s, h as f32 * s));
            ui.painter().rect_filled(page, CornerRadius::ZERO, Color32::WHITE);
            ui.painter().rect_stroke(page, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
            let [l, bo, ri, to] = d.margins.map(|v| v as f32 * s);
            let inner = Rect::from_min_max(pos2(page.left() + l, page.top() + to), pos2(page.right() - ri, page.bottom() - bo));
            if inner.is_positive() {
                ui.painter().rect_stroke(inner, CornerRadius::ZERO, Stroke::new(1.5, t.accent), egui::StrokeKind::Middle);
            }
            ui.painter().text(
                page.center_bottom() + vec2(0.0, 6.0),
                egui::Align2::CENTER_TOP,
                crate::i18n::fmt(tl!("Page {label}"), &[("label", &(current + 1).to_string())]),
                theme::regular(11.0),
                t.text_faint,
            );
        }
    });
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

/// Remove White Margins over a range, measured on a worker: pages done and total, the edit once
/// measured (`None` when cancelled), and whether Cancel was asked for.
#[derive(Default)]
pub struct WhiteMarginsProgress {
    pub done: usize,
    pub total: usize,
    pub result: Option<Option<Edit>>,
    pub cancel: bool,
}

pub struct WhiteMarginsRun {
    pub doc: DocId,
    /// The document's edit generation when measuring started: a change since makes the measures stale.
    pub generation: u64,
    /// What was measured: the same file under a newer generation is still what the measures describe.
    pub display: Arc<Vec<u8>>,
    pub progress: Arc<Mutex<WhiteMarginsProgress>>,
}

impl PdfCraftApp {
    /// Remove White Margins on `pages` of the active document: measured in the background (inline
    /// in tests and on the web), with progress and Cancel, then applied as one step.
    pub fn start_white_margins(&mut self, pages: Vec<usize>, which: PageBox) {
        let Some((_, id)) = self.active_ids() else { return };
        if self.white_margins_run.is_some() {
            self.notify_tr("Remove white margins is already running");
            return;
        }
        let job = match self.session.white_margins_job(id, &pages, which) {
            Ok(job) => job,
            Err(e) => return self.notify_error(e),
        };
        let Some((generation, display)) = self.session.get(id).map(|d| (d.edit_generation(), d.display.clone())) else { return };
        let progress = Arc::new(Mutex::new(WhiteMarginsProgress { total: job.pages.len(), ..Default::default() }));
        let p = progress.clone();
        let work = move || {
            let edit = job.run(|done, total| {
                let Ok(mut s) = p.lock() else { return false };
                s.done = done;
                s.total = total;
                !s.cancel
            });
            if let Ok(mut s) = p.lock() {
                s.result = Some(edit);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else if std::thread::Builder::new().name("pdfcraft-white-margins".into()).spawn(work).is_err()
            && let Ok(mut s) = progress.lock()
        {
            // No worker: end the run instead of waiting for a result that never comes.
            s.result = Some(None);
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.white_margins_run = Some(WhiteMarginsRun { doc: id, generation, display, progress });
        self.poll_white_margins();
    }

    /// Stop measuring (nothing is cropped).
    pub fn cancel_white_margins(&mut self) {
        if let Some(r) = &self.white_margins_run
            && let Ok(mut s) = r.progress.lock()
        {
            s.cancel = true;
        }
    }

    /// Show progress; crop the pages once the worker is done.
    pub(crate) fn poll_white_margins(&mut self) {
        let Some(run) = self.white_margins_run.as_ref() else { return };
        let (doc, generation, display, progress) = (run.doc, run.generation, run.display.clone(), run.progress.clone());
        let Ok(mut s) = progress.lock() else {
            // The worker panicked while holding the lock: it will never report back.
            self.white_margins_run = None;
            self.progress_notice = None;
            return;
        };
        let Some(result) = s.result.take() else {
            let label = if s.cancel {
                tl!("Cancelling…").to_string()
            } else {
                crate::i18n::fmt(
                    tl!("Removing white margins… page {d} of {t}"),
                    &[("d", &(s.done + 1).min(s.total.max(1)).to_string()), ("t", &s.total.max(1).to_string())],
                )
            };
            let fraction = s.done as f32 / s.total.max(1) as f32;
            let cancellable = !s.cancel;
            drop(s);
            self.progress_notice = Some(crate::widgets::ProgressNotice { label, fraction, cancellable });
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        };
        drop(s);
        self.white_margins_run = None;
        self.progress_notice = None;
        let Some(edit) = result else { return };
        // The document was closed meanwhile: nothing to crop, nothing to say.
        let Some(unchanged) = self.session.get(doc).map(|d| d.edit_generation() == generation || Arc::ptr_eq(&d.display, &display)) else { return };
        if !unchanged {
            self.notify_tr("The document changed while its margins were measured; nothing was cropped");
            return;
        }
        if self.active_ids().map(|(_, active)| active) == Some(doc) {
            self.apply_edit(edit);
        } else {
            match self.session.apply(doc, edit) {
                Ok(()) => {
                    if let Some(info) = self.session.get(doc).map(|d| d.info.clone())
                        && let Some(view) = self.views.iter_mut().find(|v| v.id == doc)
                    {
                        view.document_changed(&info);
                    }
                }
                Err(e) => self.notify_error(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// #1156: measuring a range shows its progress, and Cancel stops it with the document unchanged.
    #[test]
    fn removing_white_margins_shows_progress_and_cancels() {
        let progress = Arc::new(Mutex::new(WhiteMarginsProgress { done: 3, total: 10, ..Default::default() }));
        let p = progress.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("notes.txt", None, b"hello".to_vec()).unwrap();
            let (_, doc) = app.active_ids().unwrap();
            let d = app.session.get(doc).unwrap();
            let (generation, display) = (d.edit_generation(), d.display.clone());
            app.white_margins_run = Some(WhiteMarginsRun { doc, generation, display, progress: p });
            app
        });
        h.run_steps(2);
        h.get_by_label("Removing white margins… page 4 of 10");
        h.state_mut().start_white_margins(vec![0], PageBox::Crop);
        assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some("Remove white margins is already running"));
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(progress.lock().unwrap().cancel, "the worker is asked to stop");
        h.get_by_label("Cancelling…");
        // The worker stops and reports nothing to apply.
        progress.lock().unwrap().result = Some(None);
        h.run_steps(2);
        let s = h.state();
        assert!(s.white_margins_run.is_none() && s.progress_notice.is_none());
        assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), None, "nothing was cropped");
    }

    #[test]
    fn drafts_pick_pages_and_reset_to_default_at_zero() {
        let mut d = BoxesDraft::default();
        assert_eq!(d.pages(2, 5), [0, 1, 2, 3, 4]);
        d.range = Range::Current;
        assert_eq!(d.pages(2, 5), [2]);
        d.range = Range::Pages(2, 9);
        assert_eq!(d.pages(0, 5), [1, 2, 3, 4]);
        assert!(matches!(d.edit(0, 5), Edit::SetPageBox { spec: BoxSpec::Remove, .. }));
        d.margins = [72.0, 0.0, 0.0, 0.0];
        assert!(matches!(d.edit(0, 5), Edit::SetPageBox { spec: BoxSpec::Margins(_), which: PageBox::Crop, .. }));
    }
}
