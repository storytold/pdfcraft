//! Fill & Sign (Acrobat's Fill & Sign tool, execution plan M5.7): type text onto the page, place
//! ✓ ✕ ● ─ marks and today's date, and sign with a drawn signature. Everything is an annotation
//! (typewriter text, PdfCraft-drawn stamps, ink), so it can be moved, deleted and undone like
//! any comment.

use egui::{Color32, CornerRadius, Pos2, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::{Edit, FillMark, NewAnnotation, Shape, Style};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FillTool {
    Text,
    Check,
    Cross,
    Dot,
    Line,
    Date,
    Signature,
    Initials,
}

pub const FILL_TOOLS: [FillTool; 8] =
    [FillTool::Text, FillTool::Cross, FillTool::Check, FillTool::Dot, FillTool::Line, FillTool::Date, FillTool::Signature, FillTool::Initials];

/// A saved signature or initials: drawn strokes (normalised to the pad width, y up) or typed
/// text (drawn in the script font).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SavedSig {
    Drawn(Vec<Vec<[f32; 2]>>),
    Typed(String),
}

/// The Create signature / initials dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SigDraft {
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub text: String,
    /// The Draw tab (otherwise Type).
    pub drawing: bool,
    /// Creating initials (otherwise the signature).
    pub initials: bool,
    /// Replacing an existing saved value.
    pub editing: bool,
}

impl SigDraft {
    pub fn new(initials: bool, name: &str) -> Self {
        let text = if initials { name.split_whitespace().filter_map(|w| w.chars().next()).collect() } else { name.trim().to_string() };
        Self { strokes: Vec::new(), text, drawing: false, initials, editing: false }
    }

    /// Edit a copy of the saved value; Cancel must leave the original untouched.
    pub fn from_saved(initials: bool, saved: &SavedSig) -> Self {
        match saved {
            SavedSig::Drawn(strokes) => Self { strokes: strokes.clone(), drawing: true, initials, editing: true, ..Default::default() },
            SavedSig::Typed(text) => Self { text: text.clone(), initials, editing: true, ..Default::default() },
        }
    }

    fn ready(&self) -> bool {
        if self.drawing {
            self.strokes.iter().any(|s| s.len() > 1)
        } else {
            !self.text.trim().is_empty()
                && self.text.chars().take(pdfcraft_engine::MAX_SIGNATURE_CHARS + 1).count() <= pdfcraft_engine::MAX_SIGNATURE_CHARS
        }
    }

    pub fn saved(&self) -> SavedSig {
        if self.drawing { SavedSig::Drawn(self.strokes.clone()) } else { SavedSig::Typed(self.text.trim().to_string()) }
    }
}

impl FillTool {
    pub fn command(self) -> &'static str {
        match self {
            FillTool::Text => "sign.fill.text",
            FillTool::Check => "sign.fill.check",
            FillTool::Cross => "sign.fill.cross",
            FillTool::Dot => "sign.fill.dot",
            FillTool::Line => "sign.fill.line",
            FillTool::Date => "sign.fill.date",
            FillTool::Signature => "sign.fill.signature",
            FillTool::Initials => "sign.fill.initials",
        }
    }

    pub fn from_command(id: &str) -> Option<Self> {
        FILL_TOOLS.into_iter().find(|t| t.command() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            FillTool::Text => "Add text",
            FillTool::Check => "Checkmark",
            FillTool::Cross => "Cross",
            FillTool::Dot => "Dot",
            FillTool::Line => "Line",
            FillTool::Date => "Date",
            FillTool::Signature => "Sign",
            FillTool::Initials => "Initials",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            FillTool::Text => "type",
            FillTool::Check => "check",
            FillTool::Cross => "x",
            FillTool::Dot => "circle-dot",
            FillTool::Line => "minus",
            FillTool::Date => "clock-3",
            FillTool::Signature | FillTool::Initials => "signature",
        }
    }

    fn mark(self) -> Option<FillMark> {
        match self {
            FillTool::Check => Some(FillMark::Check),
            FillTool::Cross => Some(FillMark::Cross),
            FillTool::Dot => Some(FillMark::Dot),
            FillTool::Line => Some(FillMark::Line),
            _ => None,
        }
    }
}

/// Text being typed onto a page: (page, top-left in user space, text, focus requested).
#[derive(Clone, Debug, PartialEq)]
pub struct TypeBox {
    pub page: usize,
    pub at: [f64; 2],
    pub text: String,
    pub focus: bool,
}

/// The size Fill & Sign uses for typed text (Acrobat's default is 10 pt).
pub const TEXT_SIZE: f64 = 10.0;

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

fn new(page: usize, shape: Shape, contents: String, author: &str) -> Edit {
    let style = Style::default_for(&shape);
    Edit::AddAnnotation(NewAnnotation { page, shape, style, contents, author: author.to_string() })
}

/// A typewriter annotation sized to its text.
pub fn typed(page: usize, at: [f64; 2], text: &str, author: &str) -> Edit {
    let rect = crate::comments::text_box_rect(at, text, TEXT_SIZE);
    new(page, Shape::Typewriter { rect, font_size: TEXT_SIZE }, text.to_string(), author)
}

/// Place a saved signature (strokes normalised to a 0–1 box, y up) with its left edge at `at`,
/// 150 pt wide.
pub fn signature_at(page: usize, at: [f64; 2], strokes: &[Vec<[f32; 2]>], author: &str) -> Option<Edit> {
    let w = 150.0;
    let (min_y, max_y) = strokes.iter().flatten().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    if !min_y.is_finite() {
        return None;
    }
    let h = f64::from(max_y - min_y).max(0.05) * w;
    let strokes: Vec<Vec<[f64; 2]>> = strokes
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.iter().map(|p| [at[0] + f64::from(p[0]) * w, at[1] - h / 2.0 + f64::from(p[1] - min_y) * w]).collect())
        .collect();
    (!strokes.is_empty()).then(|| new(page, Shape::Signature { strokes }, String::new(), author))
}

/// Place typed text in the script font with its left edge at `at`, `height` points tall.
pub fn typed_signature_at(page: usize, at: [f64; 2], text: &str, height: f64, author: &str) -> Option<Edit> {
    pdfcraft_engine::typed_signature_shape(at, text, height).map(|shape| new(page, shape, String::new(), author))
}

/// Place a saved signature or initials.
pub fn place(page: usize, at: [f64; 2], sig: &SavedSig, initials: bool, author: &str) -> Option<Edit> {
    match sig {
        SavedSig::Drawn(strokes) => signature_at(page, at, strokes, author),
        SavedSig::Typed(text) => {
            let [left, bottom, right, top] = pdfcraft_engine::script_outline(text).bounds();
            let height = if initials { 24.0_f64 } else { 32.0_f64 };
            // Keep long names within the same placement width as drawn signatures.
            let height = height.min(150.0 * (top - bottom).max(0.1) / (right - left).max(0.01));
            typed_signature_at(page, at, text, height, author)
        }
    }
}

/// The text in the script font as a picture (`w`×`h` px, black on transparent), for previews.
pub(crate) fn script_preview(text: &str, w: usize, h: usize) -> egui::ColorImage {
    let o = pdfcraft_engine::script_outline(text);
    let mut img = egui::ColorImage::filled([w, h], Color32::TRANSPARENT);
    let [left, bottom, right, top] = o.bounds();
    let span = (top - bottom).max(0.1);
    let width = (right - left).max(0.01);
    if o.contours.is_empty() {
        return img;
    }
    let k = ((h as f64 * 0.9) / span).min((w as f64 * 0.95) / width);
    let x0 = (w as f64 - width * k) / 2.0;
    // Device points (y down), then an even-odd scanline fill.
    let polys: Vec<Vec<(f64, f64)>> = o
        .contours
        .iter()
        .map(|c| c.iter().map(|p| (x0 + (p[0] - left) * k, h as f64 * 0.5 + (top + bottom) / 2.0 * k - p[1] * k)).collect())
        .collect();
    for y in 0..h {
        let sy = y as f64 + 0.5;
        let mut xs: Vec<f64> = Vec::new();
        for poly in &polys {
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if (a.1 <= sy) != (b.1 <= sy) {
                    xs.push(a.0 + (sy - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for pair in xs.as_chunks::<2>().0 {
            let (from, to) = (pair[0].round().clamp(0.0, w as f64) as usize, pair[1].round().clamp(0.0, w as f64) as usize);
            for x in from..to {
                img[(x, y)] = Color32::BLACK;
            }
        }
    }
    img
}

/// Saved previews and add/remove controls, shared by the left panel and quick-tool picker.
pub(crate) fn signature_entries(ui: &mut egui::Ui, app: &mut crate::PdfCraftApp, t: &Tokens) -> Option<&'static str> {
    ui.set_width(ui.available_width().clamp(240.0, 248.0));
    let mut command = None;
    for (i, (saved, what, use_id, change_id, remove_id)) in [
        (app.signature.as_ref(), "signature", "sign.fill.signature", "sign.fill.signature.change", "sign.fill.signature.remove"),
        (app.initials.as_ref(), "initials", "sign.fill.initials", "sign.fill.initials.change", "sign.fill.initials.remove"),
    ]
    .into_iter()
    .enumerate()
    {
        let Some(saved) = saved else {
            if crate::widgets::ghost_button(ui, "plus", &format!("Add {what}")).clicked() {
                command = Some(use_id);
            }
            continue;
        };
        ui.horizontal(|ui| {
            let (rect, response) = ui.allocate_exact_size(vec2((ui.available_width() - 68.0).max(140.0), 52.0), Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Use {what}")));
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, CornerRadius::same(4), Color32::WHITE);
            painter.rect_stroke(
                rect,
                CornerRadius::same(4),
                Stroke::new(1.0, if response.hovered() { t.accent } else { t.border }),
                egui::StrokeKind::Inside,
            );
            let preview_rect = rect.shrink(8.0);
            match saved {
                SavedSig::Typed(text) => {
                    let cache = &mut app.saved_signature_previews[i];
                    if cache.as_ref().is_none_or(|(s, _)| s != text) {
                        *cache = Some((
                            text.clone(),
                            ui.ctx().load_texture(format!("saved-{what}"), script_preview(text, 480, 104), egui::TextureOptions::LINEAR),
                        ));
                    }
                    if let Some((_, tex)) = cache {
                        let size = vec2(480.0, 104.0) * (preview_rect.width() / 480.0).min(preview_rect.height() / 104.0);
                        painter.image(
                            tex.id(),
                            egui::Rect::from_center_size(preview_rect.center(), size),
                            egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                }
                SavedSig::Drawn(strokes) => {
                    let bounds = strokes
                        .iter()
                        .flatten()
                        .filter(|p| p.iter().all(|v| v.is_finite()))
                        .fold(egui::Rect::NOTHING, |r, p| r.union(egui::Rect::from_min_max(pos2(p[0], p[1]), pos2(p[0], p[1]))));
                    if bounds.is_finite() {
                        let scale = (preview_rect.width() / bounds.width().max(0.01)).min(preview_rect.height() / bounds.height().max(0.01));
                        for stroke in strokes {
                            let pts = stroke
                                .iter()
                                .filter(|p| p.iter().all(|v| v.is_finite()))
                                .map(|p| preview_rect.center() + vec2(p[0] - bounds.center().x, bounds.center().y - p[1]) * scale)
                                .collect();
                            painter.add(egui::Shape::line(pts, Stroke::new(1.5, Color32::BLACK)));
                        }
                    }
                }
            }
            if response.on_hover_text(format!("Place saved {what}")).clicked() {
                command = Some(use_id);
            }
            if crate::icons::button(ui, "pencil", 26.0, false, &format!("Change {what}")).clicked() {
                command = Some(change_id);
            }
            if crate::icons::button(ui, "x", 26.0, false, &format!("Remove saved {what}")).clicked() {
                command = Some(remove_id);
            }
        });
        ui.add_space(4.0);
    }
    command
}

/// Clicks with a Fill & Sign tool on one page. Returns `true` when the click was used.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    tool: FillTool,
    view: &mut DocView,
    signature: Option<&SavedSig>,
    initials: Option<&SavedSig>,
    author: &str,
    today: (i64, u32, u32),
) -> Option<FillAction> {
    let pointer = ui.input(|i| i.pointer.hover_pos())?;
    if !xf.rect.contains(pointer) {
        return None;
    }
    ui.ctx().set_cursor_icon(if tool == FillTool::Text { egui::CursorIcon::Text } else { egui::CursorIcon::Crosshair });
    if !resp.clicked() {
        return None;
    }
    let at = to_user(xf, info, page, pointer);
    match tool {
        FillTool::Text => {
            view.fill_text = Some(TypeBox { page, at: [at[0], at[1] + TEXT_SIZE * 0.6], text: String::new(), focus: true });
            None
        }
        FillTool::Date => {
            let (y, m, d) = today;
            Some(FillAction::Edit(Box::new(typed(page, [at[0], at[1] + TEXT_SIZE * 0.6], &format!("{m}/{d}/{y}"), author))))
        }
        FillTool::Signature => match signature {
            Some(s) => place(page, at, s, false, author).map(|e| FillAction::Edit(Box::new(e))),
            None => Some(FillAction::CreateSignature),
        },
        FillTool::Initials => match initials {
            Some(s) => place(page, at, s, true, author).map(|e| FillAction::Edit(Box::new(e))),
            None => Some(FillAction::CreateInitials),
        },
        mark => {
            let mark = mark.mark()?;
            let (w, h) = if mark == FillMark::Line { (36.0, 4.0) } else { (12.0, 12.0) };
            let rect = [at[0] - w / 2.0, at[1] - h / 2.0, at[0] + w / 2.0, at[1] + h / 2.0];
            Some(FillAction::Edit(Box::new(new(page, Shape::Mark { rect, mark }, String::new(), author))))
        }
    }
}

/// What a Fill & Sign click asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum FillAction {
    Edit(Box<Edit>),
    /// No signature yet: open the signature pad.
    CreateSignature,
    /// No initials yet.
    CreateInitials,
}

/// The in-place editor for typed text. Returns the edit once committed.
pub(crate) fn type_box(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, author: &str) -> Option<Edit> {
    let tb = view.fill_text.clone()?;
    let xf = view.page_xform(tb.page)?;
    let v = info.pages.get(tb.page)?.user_to_view(tb.at[0] as f32, tb.at[1] as f32);
    let pos = xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph);
    let zoom = xf.rect.width() / xf.pw.max(1.0);
    let mut commit = false;
    let mut cancel = false;
    egui::Area::new(egui::Id::new(("fill-text", view.id.0))).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let Some(t) = view.fill_text.as_mut() else { return };
        let width = ((t.text.len().max(8) as f32) * TEXT_SIZE as f32 * 0.6 * zoom).clamp(60.0, 600.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut t.text)
                .font(egui::FontId::proportional((TEXT_SIZE as f32 * zoom).max(8.0)))
                .desired_width(width)
                .background_color(Color32::from_rgba_unmultiplied(255, 255, 255, 230))
                .text_color(Color32::BLACK)
                .hint_text("Type text")
                .id_salt("fill-text-edit"),
        );
        if t.focus {
            r.request_focus();
            t.focus = false;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        } else if r.lost_focus() {
            commit = true;
        }
    });
    if cancel {
        view.fill_text = None;
        return None;
    }
    if commit {
        let tb = view.fill_text.take()?;
        if !tb.text.trim().is_empty() {
            return Some(typed(tb.page, tb.at, tb.text.trim(), author));
        }
    }
    None
}

/// The signature pad: draw with the pointer; returns the strokes (normalised) on Apply.
pub(crate) fn signature_pad(ui: &mut egui::Ui, t: &Tokens, d: &mut SigDraft, preview: &mut Option<(String, egui::TextureHandle)>) -> (bool, bool) {
    let what = if d.initials { "initials" } else { "signature" };
    let action = if d.editing { "Change" } else { "Create" };
    ui.label(egui::RichText::new(format!("{action} {what}")).font(crate::theme::semibold(18.0)));
    ui.horizontal(|ui| {
        if crate::widgets::pill_button(ui, "Type", !d.drawing).clicked() {
            d.drawing = false;
        }
        if crate::widgets::pill_button(ui, "Draw", d.drawing).clicked() {
            d.drawing = true;
        }
    });
    ui.add_space(6.0);
    if !d.drawing {
        let l = ui.label(egui::RichText::new(format!("Type your {what}.")).color(t.text_muted));
        ui.add(
            egui::TextEdit::singleline(&mut d.text).char_limit(pdfcraft_engine::MAX_SIGNATURE_CHARS).desired_width(460.0).hint_text(if d.initials {
                "Initials"
            } else {
                "Your name"
            }),
        )
        .labelled_by(l.id);
        let (rect, _) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        if preview.as_ref().is_none_or(|(s, _)| *s != d.text) {
            let img = script_preview(&d.text, 920, 300);
            *preview = Some((d.text.clone(), ui.ctx().load_texture("typed-signature", img, egui::TextureOptions::LINEAR)));
        }
        if let Some((_, tex)) = preview {
            painter.image(tex.id(), rect.shrink(4.0), egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        return pad_buttons(ui, d);
    }
    ui.label(egui::RichText::new(format!("Draw your {what} below.")).color(t.text_muted));
    let strokes = &mut d.strokes;
    let (rect, resp) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
    painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    painter.hline(rect.x_range().shrink(24.0), rect.bottom() - 34.0, Stroke::new(1.0, t.divider));
    // Normalised: x 0–1 across the pad, y up, in units of the pad's width.
    let norm = |p: Pos2| [(p.x - rect.left()) / rect.width(), (rect.bottom() - p.y) / rect.width()];
    if resp.drag_started() {
        strokes.push(Vec::new());
    }
    if resp.dragged()
        && let (Some(p), Some(s)) = (resp.interact_pointer_pos(), strokes.last_mut())
    {
        let n = norm(p.clamp(rect.min, rect.max));
        if s.last().is_none_or(|l| (l[0] - n[0]).abs() + (l[1] - n[1]).abs() > 0.002) {
            s.push(n);
        }
    }
    for s in strokes.iter() {
        let pts: Vec<Pos2> = s.iter().map(|p| pos2(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.width())).collect();
        painter.add(egui::Shape::line(pts, Stroke::new(2.0, Color32::BLACK)));
    }
    pad_buttons(ui, d)
}

fn pad_buttons(ui: &mut egui::Ui, d: &mut SigDraft) -> (bool, bool) {
    ui.add_space(10.0);
    let (mut apply, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if ui.button("Clear").clicked() {
            d.strokes.clear();
            d.text.clear();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ready = d.ready();
            if ui.add_enabled_ui(ready, |ui| crate::widgets::pill_button(ui, "Apply", true)).inner.clicked() {
                apply = true;
            }
            if crate::widgets::pill_button(ui, "Cancel", false).clicked() {
                cancel = true;
            }
        });
    });
    (apply, cancel)
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_signature_previews_have_clear_margins() {
        for text in ["Alexandria Catherine Elizabeth Montgomery-Wellington", "Jg Jg Jg Jg Jg Jg Jg Jg Jg Jg Jg"] {
            for [w, h] in [[920, 300], [480, 104]] {
                let image = super::script_preview(text, w, h);
                assert!(image.pixels.iter().any(|p| p.a() > 0));
                for x in 0..w {
                    assert_eq!(image[(x, 0)].a(), 0);
                    assert_eq!(image[(x, h - 1)].a(), 0);
                }
                for y in 0..h {
                    assert_eq!(image[(0, y)].a(), 0);
                    assert_eq!(image[(w - 1, y)].a(), 0);
                }
            }
        }
    }
}
