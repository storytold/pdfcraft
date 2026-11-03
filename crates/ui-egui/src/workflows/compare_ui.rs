//! Compare files: choose the older version, then the Compare panel lists the differences
//! (replaced blue, inserted green, deleted red), shades them on the newer document's pages, and
//! offers a report or the differences as comments.

use egui::{Align, Color32, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::compare::{Comparison, Kind};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, RightPanel, widgets};

/// The last comparison.
pub struct CompareState {
    pub old: DocId,
    pub new: DocId,
    pub result: Comparison,
    /// Regions that look different (page, user-space box), page n against page n.
    pub visual: Vec<(usize, [f64; 4])>,
    pub selected: Option<usize>,
    /// Side by side: the old file on the left, the new on the right, scrolled and zoomed
    /// together.
    pub side_by_side: bool,
    /// Which half leads the other (the one the pointer was over last): 0 old, 1 new.
    pub leader: usize,
    /// Each half's scroll offset last frame, to tell which one moved.
    pub prev_scroll: [egui::Vec2; 2],
    /// Show: whether the text changes are in the list and the shading, and whether the visual
    /// differences (counted, never listed) are shaded (Acrobat's Filter).
    pub show_text: bool,
    pub show_visual: bool,
}

/// Visual differences are shaded orange.
pub const VISUAL: Color32 = Color32::from_rgb(0xF0, 0x8C, 0x1A);

/// What the panel asks for.
pub enum PanelAction {
    Select(usize),
    Mark,
    Report,
    Clear,
    SideBySide(bool),
    Show { text: bool, visual: bool },
}

pub fn colour(kind: Kind) -> Color32 {
    let [r, g, b] = pdfcraft_engine::compare::colour(kind);
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// The Compare Files dialog: pick the older document. Returns (compare, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Compare Files")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let Some((_, new)) = app.active_ids() else { return (false, true) };
    let others: Vec<(DocId, String)> =
        app.views.iter().filter(|v| v.id != new).filter_map(|v| app.session.get(v.id).map(|d| (d.id, d.name.clone()))).collect();
    if app.compare_old.is_none_or(|o| !others.iter().any(|(id, _)| *id == o)) {
        app.compare_old = others.first().map(|(id, _)| *id);
    }
    let new_name = app.session.get(new).map(|d| d.name.clone()).unwrap_or_default();
    egui::Grid::new("compare-files").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
        ui.label(tl!("Old file:"));
        let shown =
            others.iter().find(|(id, _)| Some(*id) == app.compare_old).map_or(tl!("Open the older version first").to_string(), |(_, n)| n.clone());
        craft_ui::ComboBox::from_id_salt("compare-old").selected_text(shown).width(320.0).show_ui(ui, |ui| {
            for (id, n) in &others {
                ui.selectable_value(&mut app.compare_old, Some(*id), n);
            }
        });
        ui.end_row();
        ui.label(tl!("New file:"));
        ui.label(egui::RichText::new(new_name).strong());
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Text is compared word by word; the differences are shown on the new file.")).small().color(t.text_muted));
    ui.add_space(10.0);
    let (mut go, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(app.compare_old.is_some(), |ui| widgets::pill_button(ui, tl!("Compare"), true)).inner.clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (go, cancel)
}

/// The Compare panel.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, state: &Option<CompareState>, id: DocId) -> Option<PanelAction> {
    let Some(s) = state.as_ref().filter(|s| s.new == id) else {
        ui.label(egui::RichText::new(tl!("Compare this file with an older version: Compare files… in All tools.")).color(t.text_muted));
        return None;
    };
    let mut action = None;
    let c = &s.result;
    if c.identical() {
        ui.label(tl!("No differences in the text."));
    }
    ui.horizontal_wrapped(|ui| {
        for k in [Kind::Replaced, Kind::Inserted, Kind::Deleted] {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(dot, egui::CornerRadius::same(2), colour(k));
            ui.label(crate::i18n::fmt(tl!("{n} {kind}"), &[("n", &c.count(k).to_string()), ("kind", tl!(k.label()))]));
        }
        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(dot, egui::CornerRadius::same(2), VISUAL);
        ui.label(crate::i18n::fmt(tl!("{n} {kind}"), &[("n", &s.visual.len().to_string()), ("kind", tl!("Visual"))]));
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Show:"));
        let (mut text, mut visual) = (s.show_text, s.show_visual);
        let text_changed = ui.add(widgets::checkbox(&mut text, tl!("Text"))).changed();
        if ui.add(widgets::checkbox(&mut visual, tl!("Visual"))).changed() || text_changed {
            action = Some(PanelAction::Show { text, visual });
        }
    });
    ui.add_space(6.0);
    // Side by side, or the new file alone.
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Side by side"), s.side_by_side).clicked() {
            action = Some(PanelAction::SideBySide(true));
        }
        if widgets::pill_button(ui, tl!("New file only"), !s.side_by_side).clicked() {
            action = Some(PanelAction::SideBySide(false));
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button(tl!("Mark as comments")).clicked() {
            action = Some(PanelAction::Mark);
        }
        if ui.button(tl!("Report…")).clicked() {
            action = Some(PanelAction::Report);
        }
        if ui.button(tl!("Clear")).clicked() {
            action = Some(PanelAction::Clear);
        }
    });
    ui.separator();
    for (i, ch) in c.changes.iter().enumerate().filter(|_| s.show_text).take(2000) {
        let text = match ch.kind {
            Kind::Replaced => format!("\"{}\" → \"{}\"", ch.old.text, ch.new.text),
            Kind::Inserted => format!("+ \"{}\"", ch.new.text),
            Kind::Deleted => format!("− \"{}\"", ch.old.text),
        };
        let selected = s.selected == Some(i);
        let r = ui.push_id(i, |ui| {
            ui.horizontal(|ui| {
                let (bar, _) = ui.allocate_exact_size(egui::vec2(4.0, 34.0), egui::Sense::hover());
                ui.painter().rect_filled(bar, egui::CornerRadius::same(2), colour(ch.kind));
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::fmt(
                            tl!("{kind} · page {p}"),
                            &[("kind", tl!(ch.kind.label())), ("p", &(ch.new.page + 1).to_string())],
                        ))
                        .small()
                        .color(t.text_muted),
                    );
                    ui.selectable_label(selected, text)
                })
                .inner
            })
            .inner
        });
        if r.inner.clicked() {
            action = Some(PanelAction::Select(i));
        }
    }
    action
}

impl PdfCraftApp {
    /// Set gaps for the active document and both visible comparison panes, leaving other tabs alone.
    pub(crate) fn set_active_page_gaps(&mut self, show: bool) {
        let Some((index, id)) = self.active_ids() else { return };
        let partner = side_by_side(self, index).and_then(|(old, _)| self.views.get(old).map(|v| v.id));
        for view in &mut self.views {
            if view.id == id || partner == Some(view.id) {
                view.set_page_gaps(show);
            }
        }
    }

    /// Compare the chosen older document with the active one and show the differences.
    pub fn run_compare(&mut self) {
        let Some((i, new)) = self.active_ids() else { return };
        let Some(old) = self.compare_old else { return };
        match self.session.compare(old, new) {
            Ok(result) => {
                let visual = self.session.compare_visual(old, new, 72.0).unwrap_or_default();
                let n = result.changes.len();
                // Side by side when the old version is open in a tab and the window has the room.
                let side_by_side = self.views.iter().any(|v| v.id == old) && self.ctx.as_ref().is_none_or(|c| c.content_rect().width() >= 1000.0);
                self.compare = Some(crate::compare_ui::CompareState {
                    old,
                    new,
                    result,
                    visual,
                    selected: None,
                    side_by_side,
                    leader: 1,
                    prev_scroll: [egui::Vec2::ZERO; 2],
                    show_text: true,
                    show_visual: true,
                });
                self.shade_compare(i);
                self.right = Some(RightPanel::Compare);
                self.notify(if n == 0 {
                    tl!("The text is the same").to_string()
                } else if n == 1 {
                    tl!("1 difference found").to_string()
                } else {
                    crate::i18n::fmt(tl!("{n} differences found"), &[("n", &n.to_string())])
                });
            }
            Err(e) => self.notify_error(e),
        }
    }

    pub(crate) fn compare_action(&mut self, index: usize, a: PanelAction) {
        let Some(s) = self.compare.as_mut() else { return };
        let (old, new) = (s.old, s.new);
        match a {
            PanelAction::Select(i) => {
                s.selected = Some(i);
                let Some(ch) = s.result.changes.get(i) else { return };
                let rect = ch.new.rects.first().copied().or(ch.new.near).unwrap_or_default();
                let page = ch.new.page;
                let old_target = (ch.old.page, ch.old.rects.first().copied().or(ch.old.near).unwrap_or_default());
                let v = &mut self.views[index];
                v.go_to_page(page);
                v.flash = Some((page, rect.map(|x| x as f32), 0.0));
                // The old file's half jumps to its side of the change too.
                if let Some(oi) = self.views.iter().position(|v| v.id == old) {
                    let v = &mut self.views[oi];
                    v.go_to_page(old_target.0);
                    v.flash = Some((old_target.0, old_target.1.map(|x| x as f32), 0.0));
                }
            }
            PanelAction::SideBySide(on) => {
                s.side_by_side = on;
                s.leader = 1;
            }
            PanelAction::Show { text, visual } => {
                s.show_text = text;
                s.show_visual = visual;
                self.shade_compare(index);
            }
            PanelAction::Mark => match self.session.mark_differences(old, new) {
                Ok(n) => {
                    if let Some(info) = self.session.get(new).map(|d| d.info.clone()) {
                        self.views[index].document_changed(&info);
                    }
                    self.notify(if n == 1 {
                        tl!("Added 1 comment").to_string()
                    } else {
                        crate::i18n::fmt(tl!("Added {n} comments"), &[("n", &n.to_string())])
                    });
                }
                Err(e) => self.notify_error(e),
            },
            PanelAction::Report => match self.session.compare_report(old, new) {
                Ok(bytes) => {
                    if let Err(e) = self.open_bytes("Compare Report.pdf", None, bytes.to_vec()) {
                        self.notify_error(e);
                    }
                }
                Err(e) => self.notify_error(e),
            },
            PanelAction::Clear => {
                self.compare = None;
                self.views[index].compare_marks.clear();
                if let Some(oi) = self.views.iter().position(|v| v.id == old) {
                    self.views[oi].compare_marks.clear();
                }
                self.right = None;
            }
        }
    }
}

impl PdfCraftApp {
    /// Shade the comparison on the new file's pages (view `index`) and, when it is open, the old
    /// file's, with the kinds of change the filter shows.
    fn shade_compare(&mut self, index: usize) {
        let Some(s) = self.compare.as_ref() else { return };
        let old = s.old;
        let (old_marks, new_marks) = (shading(s, true), shading(s, false));
        if let Some(v) = self.views.get_mut(index) {
            v.compare_marks = new_marks;
        }
        if let Some(v) = self.views.iter_mut().find(|v| v.id == old) {
            v.compare_marks = old_marks;
        }
    }
}

/// One half's shading: its side of the text changes (the old file has no side of an insertion)
/// and the visual differences no text change there covers, so both versions read with their own
/// shading. Hidden kinds are left out after the overlap test, so hiding the text does not uncover
/// the visual differences a text change explains.
fn shading(s: &CompareState, old: bool) -> Vec<(usize, [f32; 4], Color32)> {
    let overlaps = |p: usize, r: [f32; 4], marks: &[(usize, [f32; 4], Color32)]| {
        marks.iter().any(|(mp, m, _)| *mp == p && m[0] <= r[2] && r[0] <= m[2] && m[1] <= r[3] && r[1] <= m[3])
    };
    let mut text = Vec::new();
    for ch in s.result.changes.iter().filter(|ch| !(old && ch.kind == Kind::Inserted)) {
        let side = if old { &ch.old } else { &ch.new };
        text.extend(side.rects.iter().map(|r| (side.page, r.map(|v| v as f32), colour(ch.kind))));
    }
    let mut visual = Vec::new();
    for (p, r) in &s.visual {
        let r = r.map(|v| v as f32);
        // The old file's half also skips a region overlapping one it already shades.
        if !overlaps(*p, r, &text) && !(old && overlaps(*p, r, &visual)) {
            visual.push((*p, r, VISUAL));
        }
    }
    let mut out = if s.show_text { text } else { Vec::new() };
    if s.show_visual {
        out.extend(visual);
    }
    out
}

/// The two halves of a side-by-side comparison, as view indexes (old, new), when the active
/// document is the new file of a comparison shown side by side and the old file is open too.
pub(crate) fn side_by_side(app: &PdfCraftApp, index: usize) -> Option<(usize, usize)> {
    let c = app.compare.as_ref().filter(|c| c.side_by_side)?;
    let new = app.views.get(index).filter(|v| v.id == c.new)?;
    let old = app.views.iter().position(|v| v.id == c.old)?;
    (old != index).then_some((old, app.views.iter().position(|v| v.id == new.id)?))
}

/// Side by side: the old file on the left, the new on the right, each a page view of its own
/// with its shading, scrolled and zoomed together. The half the pointer is over leads; the
/// other follows its zoom, layout and scroll offset.
pub(crate) fn compare_area(app: &mut PdfCraftApp, old: usize, new: usize, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let rect = ui.available_rect_before_wrap();
    let gap = 1.0;
    let half_w = ((rect.width() - gap) / 2.0).max(1.0);
    let caption_h = 22.0;
    let left = egui::Rect::from_min_size(rect.min, egui::vec2(half_w, rect.height()));
    let right = egui::Rect::from_min_size(egui::pos2(rect.min.x + half_w + gap, rect.min.y), egui::vec2(half_w, rect.height()));
    // Who leads: the half under the pointer (remembered), the new file at first.
    if let Some(p) = ui.input(|i| i.pointer.hover_pos())
        && let Some(c) = app.compare.as_mut()
    {
        if left.contains(p) {
            c.leader = 0;
        } else if right.contains(p) {
            c.leader = 1;
        }
    }
    let leader = app.compare.as_ref().map_or(1, |c| c.leader);
    let (lead, follow) = if leader == 0 { (old, new) } else { (new, old) };
    // The follower takes the leader's zoom and layout before either is drawn.
    let (zoom, fit, layout, continuous, page_gaps, rotation) = {
        let v = &app.views[lead];
        (v.zoom, v.fit, v.layout, v.continuous, v.show_page_gaps, v.rotation)
    };
    {
        let v = &mut app.views[follow];
        if v.zoom != zoom
            || v.fit != fit
            || v.layout != layout
            || v.continuous != continuous
            || v.show_page_gaps != page_gaps
            || v.rotation != rotation
        {
            v.zoom = zoom;
            v.fit = fit;
            v.layout = layout;
            v.continuous = continuous;
            v.show_page_gaps = page_gaps;
            v.rotation = rotation;
        }
    }
    let names = [old, new].map(|i| app.session.get(app.views[i].id).map(|d| d.name.clone()).unwrap_or_default());
    // Drawing a half can close its own tab (its snapshot bar's Close, #3915), which shifts or
    // removes the indexes below. Each half is found again by id, and the frame stops when one is gone.
    let ids = [app.views[old].id, app.views[new].id];
    let find = |app: &PdfCraftApp, id| app.views.iter().position(|v| v.id == id);
    ui.painter().vline(rect.min.x + half_w, rect.y_range(), egui::Stroke::new(gap, t.border));
    for (k, r) in [left, right].into_iter().enumerate() {
        // Both halves must still be open: once either has closed, the pair is over.
        let (Some(old_index), Some(new_index)) = (find(app, ids[0]), find(app, ids[1])) else { return };
        let index = if k == 0 { old_index } else { new_index };
        let caption = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), caption_h));
        ui.painter().rect_filled(caption, 0.0, t.panel);
        let label = crate::i18n::fmt(if k == 0 { tl!("Old file: {name}") } else { tl!("New file: {name}") }, &[("name", &names[k])]);
        ui.painter().text(
            egui::pos2(caption.min.x + 10.0, caption.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.0),
            t.text_muted,
        );
        let body = egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y + caption_h), r.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt(("compare-half", k)));
        child.set_clip_rect(body);
        crate::canvas::document_area(app, index, &mut child);
    }
    // After both drew: the leader's scroll, when it moved, is the follower's next offset.
    let (Some(old), Some(new)) = (find(app, ids[0]), find(app, ids[1])) else { return };
    let follow = if leader == 0 { new } else { old };
    let offsets = [app.views[old].last_scroll, app.views[new].last_scroll];
    if let Some(c) = app.compare.as_mut() {
        let lead_k = if leader == 0 { 0 } else { 1 };
        if (offsets[lead_k] - c.prev_scroll[lead_k]).length() > 0.5 && (offsets[1 - lead_k] - offsets[lead_k]).length() > 0.5 {
            app.views[follow].sync_scroll = Some(offsets[lead_k]);
            ui.ctx().request_repaint();
        }
        c.prev_scroll = offsets;
    }
}
