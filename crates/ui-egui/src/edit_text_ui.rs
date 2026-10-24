//! Edit a PDF ▸ Edit text & images: boxes around the paragraphs and images already on the page.
//! Click a paragraph to edit it in place (⌘Enter or clicking away applies and rewraps it to the
//! box, Esc cancels); drag it to move it, or drag the handle on its left or right edge to rewrap
//! it to a new width. Click an image to select it: drag to move, drag a corner to resize (keeping
//! its proportions), right-click for rotate, flip, replace, save and delete; Delete removes it.

use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Stroke};
use pdfcraft_engine::{Edit, ObjectKind, ObjectTarget};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};

const ACCENT: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

#[derive(Clone, Copy, PartialEq)]
enum TextTarget {
    Block(usize),
    Added(usize),
}

/// A cached, read-only page background. The source glyphs are removed before rasterization;
/// colour, pictures and unselected text beneath the editor remain visible.
#[derive(Default)]
pub(crate) struct TextBackground {
    key: Option<(u64, usize, TextTarget)>,
    renderer: Option<pdfcraft_render::RenderPool>,
    texture: Option<egui::TextureHandle>,
    tag: Option<u64>,
    received: Option<u64>,
    draft_key: Option<(String, pdfcraft_engine::BlockStyle)>,
    draft_rect: Option<[f64; 4]>,
    /// This frame's place for the textless page under the open paragraph's editor. It stays
    /// empty, so the page shows its own text, until the paragraph is changed ([`Self::reveal`]).
    slot: Option<(egui::Painter, egui::layers::ShapeIdx, egui::Shape)>,
    /// How the open paragraph looked in its editor last frame.
    look: Option<EditorLook>,
    /// An applied paragraph's last look. The page raster still has the old text until the page
    /// is rendered again, so the textless page and this look stay up until then.
    settling: Option<EditorLook>,
}

/// A paragraph editor's text as last drawn: its laid-out rows, where they were on the page (in
/// the page's 0..1 coordinates), and the page's width and turn on screen then.
struct EditorLook {
    page: usize,
    block: usize,
    galley: Arc<egui::Galley>,
    at: (f32, f32),
    color: Color32,
    page_width: f32,
    rot: u16,
}

impl TextBackground {
    pub fn pending(&self) -> bool {
        self.renderer.is_some() && self.received != self.tag
    }

    /// The open paragraph's editor drew `galley` at `pos` this frame.
    pub fn remember(&mut self, xf: &PageXform, page: usize, block: usize, galley: Arc<egui::Galley>, pos: Pos2, color: Color32) {
        self.look = Some(EditorLook { page, block, galley, at: xf.screen_to_norm(pos), color, page_width: xf.rect.width(), rot: xf.rot });
    }

    /// An edit was applied. When it is the open paragraph's, its last look stays up over the
    /// textless page until the page is rendered again; any other edit ends both.
    pub fn committed(&mut self, edit: &Edit) {
        let look = self.look.take().filter(|look| {
            matches!(edit, Edit::EditTextBlock { page, block, .. } if (look.page, look.block) == (*page, *block))
                && self.key.is_some_and(|(_, page, target)| (page, target) == (look.page, TextTarget::Block(look.block)))
                && self.texture.is_some()
        });
        match look {
            Some(look) => {
                self.renderer = None;
                self.settling = Some(look);
            }
            None => *self = Self::default(),
        }
    }

    pub fn page_received(&mut self, page: usize) {
        if self.settling.as_ref().is_some_and(|look| look.page == page) {
            *self = Self::default();
        }
    }

    pub fn prepare(
        &mut self,
        ctx: &egui::Context,
        doc: &pdfcraft_engine::Document,
        editor: Option<&LineEditor>,
        draft: Option<&crate::content_ui::TextDraft>,
        scale: f32,
    ) {
        let key = editor
            .map(|ed| (doc.edit_generation(), ed.page, TextTarget::Block(ed.block)))
            .or_else(|| draft.and_then(|d| d.index.map(|index| (doc.edit_generation(), d.page, TextTarget::Added(index)))));
        self.slot = None;
        if key.is_none() && self.settling.is_some() {
            return;
        }
        if self.key != key {
            *self = Self { key, ..Default::default() };
            if let Some((_, page, target)) = key {
                self.renderer = match target {
                    TextTarget::Block(block) => doc.text_edit_preview(page, block),
                    TextTarget::Added(index) => doc.added_text_preview(page, index),
                }
                .ok()
                .map(|p| p.renderer());
            }
        }
        if let Some(ed) = editor {
            let draft_key = (ed.text.clone(), ed.style());
            if self.draft_key.as_ref() != Some(&draft_key) {
                self.draft_rect = doc.text_edit_layout(ed.page, ed.block, &ed.text, &draft_key.1).ok().and_then(|layout| layout.draft_rect);
                self.draft_key = Some(draft_key);
            }
        }
        let (Some((_, page, _)), Some(renderer)) = (self.key, self.renderer.as_ref()) else { return };
        let Some(p) = doc.info.pages.get(page) else { return };
        // One raster stands in for the page's tiles while a paragraph is open, so everything
        // around the editor is drawn from it: at the tiling threshold it stays sharp on a Retina
        // display up to about 2.5x on a Letter or A4 page, where the backdrop side (2048) went
        // soft past 1.2x.
        let max_side = ctx.input(|i| i.max_texture_side) as f32;
        let scale = scale.min(crate::canvas::TILE_THRESHOLD.min(max_side) / p.width.max(p.height).max(1.0));
        let tag = (scale * 1000.0) as u64;
        if self.tag != Some(tag) {
            renderer.set_queue(vec![pdfcraft_render::RenderRequest { page, scale, tag, ..Default::default() }]);
            self.tag = Some(tag);
        }
        if let Some(out) = renderer.try_recv() {
            if out.error.is_some() {
                self.renderer = None;
                self.texture = None;
                return;
            }
            if out.request.tag == tag {
                self.texture = Some(ctx.load_texture(
                    "text-editor-background",
                    crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba),
                    egui::TextureOptions::LINEAR,
                ));
                self.received = Some(tag);
            }
        }
        if self.received != Some(tag) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// The draft's measured extent, when it was measured for paragraph `block` on `page`.
    pub fn draft_rect_for(&self, page: usize, block: usize) -> Option<[f64; 4]> {
        self.draft_rect.filter(|_| self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Block(block))))
    }

    pub fn ready(&self, page: usize, block: usize) -> bool {
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Block(block))) && self.texture.is_some()
    }

    pub fn ready_added(&self, page: usize, index: usize) -> bool {
        self.key.is_some_and(|(_, p, target)| (p, target) == (page, TextTarget::Added(index))) && self.texture.is_some()
    }

    pub fn paint(
        &mut self,
        painter: &egui::Painter,
        xf: &PageXform,
        page: usize,
        editor: Option<&LineEditor>,
        draft: Option<&crate::content_ui::TextDraft>,
    ) {
        let for_editor = editor.is_some_and(|ed| ed.page == page && self.ready(page, ed.block));
        // A zoom or a turn of the view since the paragraph was applied would misplace its rows:
        // the page raster (with the old text) shows until the new one arrives instead.
        let settled =
            self.settling.as_ref().filter(|look| look.page == page && look.rot == xf.rot && (look.page_width - xf.rect.width()).abs() < 0.5);
        let ready =
            for_editor || draft.is_some_and(|d| d.page == page && d.index.is_some_and(|index| self.ready_added(page, index))) || settled.is_some();
        if !ready {
            return;
        }
        let Some(tex) = &self.texture else { return };
        // Font metrics can underestimate the actual glyph ink. Paint the complete textless
        // page so overhanging glyphs cannot remain outside the estimated source rectangle.
        let painter = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
        if for_editor {
            // Its place under everything drawn on the page later; the editor fills it once the
            // paragraph changes (an unchanged paragraph looks as the page draws it).
            let mut mesh = egui::Mesh::with_texture(tex.id());
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                mesh.vertices.push(egui::epaint::Vertex { pos: xf.norm_to_screen(u, v), uv: Pos2::new(u, v), color: Color32::WHITE });
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            let idx = painter.add(egui::Shape::Noop);
            self.slot = Some((painter, idx, egui::Shape::mesh(mesh)));
        } else {
            xf.paint_image(&painter, tex.id(), 0.0, 0.0, 1.0, 1.0);
            if let Some(look) = settled {
                painter.galley(xf.norm_to_screen(look.at.0, look.at.1), Arc::clone(&look.galley), look.color);
            }
        }
    }

    /// The open paragraph has changed: show the textless page under its editor this frame.
    pub fn reveal(&mut self) {
        if let Some((painter, idx, shape)) = self.slot.take() {
            painter.set(idx, shape);
        }
    }
}

/// How many faint Edit-text outlines are drawn per page per frame. CAD plots can draw
/// ~1000 tiny images on one page; stroking all of them every frame (plus a hover query
/// per box) stalls the UI. Past this, only the selected and hovered boxes are outlined:
/// hit-testing still covers every image, so nothing becomes unselectable.
const MAX_OUTLINES: usize = 250;

/// A paragraph being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEditor {
    pub selection: Option<std::ops::Range<usize>>,
    pub font_resource: Option<String>,
    pub page: usize,
    pub block: usize,
    pub text: String,
    original: String,
    rect: Rect,
    /// The original PDF rectangle, in user space. It is converted again each frame so the editor
    /// stays attached while the page is zoomed, scrolled, or rotated.
    source_rect: [f32; 4],
    multiline: bool,
    source: Option<(pdfcraft_engine::TextBlock, Vec<pdfcraft_engine::TextLine>)>,
    pub(crate) fonts: BTreeMap<String, String>,
    /// The draft extends outside the source container; its frame follows the live draft bounds.
    pub overflow: bool,
    /// The initial source container width in screen pixels.
    max_width: f32,
    /// Current screen-space size. Recomputed from the current page transform before painting.
    size: f32,
    /// Take the keyboard focus on the next frame.
    pub(crate) focus: bool,
    /// The Format text panel's values, and what the paragraph had (to send only changes).
    pub look: pdfcraft_engine::AddedText,
    look0: pdfcraft_engine::AddedText,
    /// A standard family was picked from the font list. The family guessed from a PDF font name
    /// (Calibri reads as Helvetica) can be the one picked, so comparing `look` with `look0` alone
    /// would miss the choice.
    family_chosen: bool,
    /// Underline, line spacing (× size; 0 = the paragraph's own), character spacing (pt) and
    /// horizontal scale (%), and what they were.
    pub extras: Extras,
    extras0: Extras,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extras {
    pub underline: bool,
    pub line_spacing: f64,
    pub char_spacing: f64,
    pub scale: f64,
}

impl Default for Extras {
    fn default() -> Self {
        Extras { underline: false, line_spacing: 0.0, char_spacing: 0.0, scale: 100.0 }
    }
}

impl LineEditor {
    /// What Format text's font list shows: the chosen page font, a standard family picked
    /// instead, or the paragraph's own font (its `/BaseFont` without the subset tag).
    pub(crate) fn font_shown(&self, sources: &[(String, String)]) -> String {
        if let Some(face) = self.font_resource.as_ref().and_then(|r| sources.iter().find(|(res, _)| res == r)).map(|(_, face)| face) {
            return face.clone();
        }
        if self.look.family != self.look0.family || self.family_chosen {
            return self.look.family.label().to_string();
        }
        let base = self.source.as_ref().map(|(block, _)| block.base_font.as_str()).unwrap_or_default();
        // "ABCDEF+Calibri" is a subset of Calibri.
        let name = match base.split_once('+') {
            Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
            _ => base,
        };
        if name.is_empty() { tl!("Keep source fonts").to_string() } else { name.to_string() }
    }

    /// Back to the paragraph's own font: no page font chosen and the family it started with.
    pub(crate) fn keep_source_font(&mut self) {
        self.font_resource = None;
        self.look.family = self.look0.family;
        self.family_chosen = false;
    }

    /// A standard family picked from the font list, replacing the paragraph's font even when it
    /// is the family the paragraph's font name reads as.
    pub(crate) fn choose_family(&mut self, family: pdfcraft_engine::FontFamily) {
        self.font_resource = None;
        self.look.family = family;
        self.family_chosen = true;
    }

    pub(crate) fn has_changes(&self) -> bool {
        self.changes_with(&self.text)
    }

    /// Whether the paragraph with `text` in it differs from what was opened.
    fn changes_with(&self, text: &str) -> bool {
        text != self.original || self.look != self.look0 || self.extras != self.extras0 || self.family_chosen
    }

    /// Extra width permitted by the editor. Source containers now remain fixed.
    pub fn growth(&self) -> Option<f32> {
        (self.max_width > self.rect.width()).then_some(self.max_width)
    }

    /// Whether the edited text differs from the original (unsaved changes).
    pub fn has_unsaved_text(&self) -> bool {
        self.text != self.original
    }

    /// The formatting the panel changed.
    pub fn style(&self) -> pdfcraft_engine::BlockStyle {
        let (l, o) = (&self.look, &self.look0);
        let mut style = pdfcraft_engine::BlockStyle {
            resource: self.font_resource.clone(),
            family: (l.family != o.family || self.family_chosen || l.bold != o.bold || l.italic != o.italic).then_some((l.family, l.bold, l.italic)),
            size: (l.size != o.size).then_some(l.size),
            color: (l.color != o.color).then_some(l.color),
            align: (l.align != o.align).then_some(l.align),
            underline: (self.extras.underline != self.extras0.underline).then_some(self.extras.underline),
            line_spacing: (self.extras.line_spacing != self.extras0.line_spacing && self.extras.line_spacing > 0.0)
                .then_some(self.extras.line_spacing),
            char_spacing: (self.extras.char_spacing != self.extras0.char_spacing).then_some(self.extras.char_spacing),
            scale: (self.extras.scale != self.extras0.scale).then_some(self.extras.scale),
            ..Default::default()
        };
        if let Some(range) = &self.selection
            && (style.family.is_some()
                || style.resource.is_some()
                || style.size.is_some()
                || style.color.is_some()
                || style.underline.is_some()
                || style.char_spacing.is_some()
                || style.scale.is_some())
        {
            style.ranges.push(pdfcraft_engine::RangeStyle {
                range: range.clone(),
                family: style.family.take(),
                resource: style.resource.take(),
                size: style.size.take(),
                color: style.color.take(),
                underline: style.underline.take(),
                char_spacing: style.char_spacing.take(),
                scale: style.scale.take(),
            });
        }
        style
    }

    /// After applying formatting: the paragraph now has it.
    pub fn applied(&mut self) {
        self.selection = None;
        self.font_resource = None;
        self.family_chosen = false;
        self.look0 = self.look.clone();
        self.extras0 = self.extras;
        self.original = self.text.clone();
        self.focus = true;
    }

    /// Adopt the rewritten paragraph's current geometry before the next overlay frame.
    pub(crate) fn refresh_source(&mut self, block: &pdfcraft_engine::TextBlock, lines: Vec<pdfcraft_engine::TextLine>) {
        self.source_rect = block.rect.map(|v| v as f32);
        self.multiline = block.rows.len() > 1;
        self.source = Some((block.clone(), lines));
        self.text = block.editor_text.clone();
        self.original = self.text.clone();
    }
}

/// The look shown for a paragraph: the family and weight guessed from its PDF font name.
fn source_look(base_font: &str, size: f64, color: [f64; 3], detected_bold: bool, detected_italic: bool) -> pdfcraft_engine::AddedText {
    use pdfcraft_engine::FontFamily as F;
    let name = base_font.to_ascii_lowercase();
    let family = if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console"].iter().any(|s| name.contains(s)) {
        F::Courier
    } else if !name.contains("sans") && ["times", "serif", "roman", "cambria", "georgia", "palatino", "garamond"].iter().any(|s| name.contains(s)) {
        F::Times
    } else {
        F::Helvetica
    };
    pdfcraft_engine::AddedText {
        family,
        bold: detected_bold || ["bold", "black", "heavy", "semibold", "demi"].iter().any(|s| name.contains(s)),
        italic: detected_italic || ["italic", "oblique", "slanted"].iter().any(|s| name.contains(s)),
        size: (size * 10.0).round() / 10.0,
        color,
        ..Default::default()
    }
}

fn look_of(b: &pdfcraft_engine::TextBlock) -> pdfcraft_engine::AddedText {
    source_look(&b.base_font, b.size, b.color, b.bold, b.italic)
}

pub(crate) fn editor_font(look: &pdfcraft_engine::AddedText, size: f32) -> FontId {
    let family = match (look.family, look.bold) {
        (pdfcraft_engine::FontFamily::Courier, _) => FontFamily::Monospace,
        (_, true) => FontFamily::Name("semibold".into()),
        (_, false) => FontFamily::Proportional,
    };
    FontId::new(size, family)
}

fn color32(color: [f64; 3]) -> Color32 {
    Color32::from_rgb(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// The dark box behind text too pale to read on white.
const DARK_EDITOR_FILL: Color32 = Color32::from_gray(0x26);

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio between two colours (1 to 21).
fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The inline editor's background for text drawn in `text`: white, or a dark box when the text
/// reads better on that (white or pale text, which would vanish on white).
pub(crate) fn editor_fill(text: Color32) -> Color32 {
    if contrast(text, Color32::WHITE) >= contrast(text, DARK_EDITOR_FILL) { Color32::WHITE } else { DARK_EDITOR_FILL }
}

/// A selected page image, and what the pointer is doing to it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSelection {
    pub page: usize,
    pub index: usize,
    /// Dragging: the start point and, for a corner, the opposite corner (screen).
    drag: Option<(Pos2, Option<Pos2>)>,
    /// Crop mode (Acrobat's crop handles): the box kept so far.
    pub crop: Option<CropMode>,
    /// The drag is turning the picture by its rotate handle (the `drag` start is then unused).
    turning: bool,
    /// A move or resize applied on release: the frame it had and the one it got, in the page's
    /// 0..1 coordinates, while the page raster still has the picture where it was.
    settled: Option<(Rect, Rect)>,
}

/// Cropping a page image: the handles trim instead of resizing, the part that will go is
/// dimmed, Enter or Done applies and Esc or Cancel leaves it.
#[derive(Clone, Debug, PartialEq)]
pub struct CropMode {
    /// The box to keep (user space).
    pub keep: [f64; 4],
    /// A drag in progress: the grip (0–3 corners, 4–7 edges, 8 the inside), where it started
    /// and the box then.
    drag: Option<(usize, Pos2, [f64; 4])>,
}

impl CropMode {
    pub fn new(keep: [f64; 4]) -> Self {
        Self { keep, drag: None }
    }
}

/// The screen boxes of the crop's eight handles: corners, then the top, right, bottom and left
/// edge midpoints.
fn crop_handles(k: Rect) -> [Pos2; 8] {
    [k.left_top(), k.right_top(), k.left_bottom(), k.right_bottom(), k.center_top(), k.right_center(), k.center_bottom(), k.left_center()]
}

/// Crop mode for the selected image `i` whose visible frame is `b` (screen). Returns `true`
/// (the pointer and keys are the crop's while it lasts).
#[allow(clippy::too_many_arguments)]
fn crop_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    view: &mut DocView,
    i: usize,
    b: Rect,
    allow_interaction: bool,
) -> bool {
    let painter = ui.painter();
    let Some(mode) = view.image_selection.as_mut().and_then(|s| s.crop.as_mut()) else { return false };
    let hover_pos = ui.input(|inp| inp.pointer.hover_pos());
    let keep_screen = |keep: [f64; 4]| xf.user_rect(info, page, keep.map(|v| v as f32)).intersect(b);
    // A drag in progress moves a grip; the box stays inside the picture and at least 6 px.
    if allow_interaction
        && resp.drag_started()
        && let Some(o) = ui.input(|inp| inp.pointer.press_origin())
    {
        let k = keep_screen(mode.keep);
        let grip = crop_handles(k).iter().position(|h| h.distance(o) < 8.0).or_else(|| k.contains(o).then_some(8));
        if let Some(g) = grip {
            mode.drag = Some((g, o, mode.keep));
        }
    }
    if let Some((grip, start, at_start)) = mode.drag
        && let Some(p) = hover_pos
    {
        let d = p - start;
        let k0 = keep_screen(at_start);
        let (mut min, mut max) = (k0.min, k0.max);
        match grip {
            0 => {
                min.x += d.x;
                min.y += d.y;
            }
            1 => {
                max.x += d.x;
                min.y += d.y;
            }
            2 => {
                min.x += d.x;
                max.y += d.y;
            }
            3 => {
                max.x += d.x;
                max.y += d.y;
            }
            4 => min.y += d.y,
            5 => max.x += d.x,
            6 => max.y += d.y,
            7 => min.x += d.x,
            _ => {
                let shift = egui::vec2(d.x.clamp(b.min.x - k0.min.x, b.max.x - k0.max.x), d.y.clamp(b.min.y - k0.min.y, b.max.y - k0.max.y));
                min += shift;
                max += shift;
            }
        }
        let (min_w, min_h) = (6.0_f32.min(b.width()), 6.0_f32.min(b.height()));
        min.x = min.x.clamp(b.min.x, b.max.x - min_w);
        min.y = min.y.clamp(b.min.y, b.max.y - min_h);
        max.x = max.x.clamp(min.x + min_w, b.max.x);
        max.y = max.y.clamp(min.y + min_h, b.max.y);
        mode.keep = user_box(xf, info, page, Rect::from_min_max(min, max));
        if resp.drag_stopped() || !ui.input(|inp| inp.pointer.primary_down()) {
            mode.drag = None;
        }
    }
    let k = keep_screen(mode.keep);
    // What goes is dimmed; what stays keeps its frame and handles.
    let dim = Color32::from_black_alpha(96);
    for r in [
        Rect::from_min_max(b.min, Pos2::new(b.max.x, k.min.y)),
        Rect::from_min_max(Pos2::new(b.min.x, k.max.y), b.max),
        Rect::from_min_max(Pos2::new(b.min.x, k.min.y), Pos2::new(k.min.x, k.max.y)),
        Rect::from_min_max(Pos2::new(k.max.x, k.min.y), Pos2::new(b.max.x, k.max.y)),
    ] {
        if r.is_positive() {
            painter.rect_filled(r.intersect(xf.rect), CornerRadius::ZERO, dim);
        }
    }
    painter.rect_stroke(k, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
    for h in crop_handles(k) {
        painter.rect(
            Rect::from_center_size(h, egui::vec2(8.0, 8.0)),
            CornerRadius::ZERO,
            Color32::WHITE,
            Stroke::new(1.0, ACCENT),
            egui::StrokeKind::Middle,
        );
    }
    // Done and Cancel, painted under the box (above it near the bottom of the view).
    let font = egui::FontId::proportional(12.5);
    let pill = |label: &str, right: f32, y: f32| -> Rect {
        let w = painter.layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE).size().x + 20.0;
        Rect::from_min_size(Pos2::new(right - w, y), egui::vec2(w, 22.0))
    };
    let y = if k.max.y + 30.0 < xf.rect.max.y { k.max.y + 6.0 } else { k.min.y - 28.0 };
    let done = pill(tl!("Done"), k.max.x, y);
    let cancel = pill(tl!("Cancel"), done.min.x - 6.0, y);
    let t = crate::theme::Tokens::get(ui.ctx());
    painter.rect_filled(done, CornerRadius::same(11), ACCENT);
    painter.text(done.center(), egui::Align2::CENTER_CENTER, tl!("Done"), font.clone(), Color32::WHITE);
    painter.rect(cancel, CornerRadius::same(11), t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    painter.text(cancel.center(), egui::Align2::CENTER_CENTER, tl!("Cancel"), font, t.text);
    if let Some(p) = hover_pos {
        let icon = if let Some(g) = crop_handles(k).iter().position(|h| h.distance(p) < 8.0) {
            match g {
                0 | 3 => egui::CursorIcon::ResizeNwSe,
                1 | 2 => egui::CursorIcon::ResizeNeSw,
                4 | 6 => egui::CursorIcon::ResizeVertical,
                _ => egui::CursorIcon::ResizeHorizontal,
            }
        } else if done.contains(p) || cancel.contains(p) {
            egui::CursorIcon::PointingHand
        } else if k.contains(p) {
            egui::CursorIcon::Move
        } else {
            egui::CursorIcon::Default
        };
        ui.ctx().set_cursor_icon(icon);
    }
    let clicked = |r: Rect| allow_interaction && resp.clicked() && hover_pos.is_some_and(|p| r.contains(p));
    let keys = |key: egui::Key| allow_interaction && ui.input(|inp| inp.key_pressed(key)) && !ui.ctx().egui_wants_keyboard_input();
    if clicked(done) || keys(egui::Key::Enter) {
        let keep = mode.keep;
        if let Some(s) = view.image_selection.as_mut() {
            s.crop = None;
        }
        if keep_screen(keep) != b {
            view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Crop(keep) });
        }
    } else if (clicked(cancel) || keys(egui::Key::Escape))
        && let Some(s) = view.image_selection.as_mut()
    {
        s.crop = None;
    }
    true
}

impl ImageSelection {
    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// The page raster's picture of the box `from`, stretched into `to`, over blanks at `blank` (where
/// the screen may still show the picture).
fn stretched_picture(painter: &egui::Painter, xf: &PageXform, tex: egui::TextureId, blank: &[Rect], from: Rect, to: Rect) {
    let clipped = painter.with_clip_rect(painter.clip_rect().intersect(xf.rect));
    for r in blank {
        clipped.rect_filled(*r, CornerRadius::ZERO, Color32::WHITE);
    }
    let mut mesh = egui::Mesh::with_texture(tex);
    for (a, b) in [
        (from.left_top(), to.left_top()),
        (from.right_top(), to.right_top()),
        (from.right_bottom(), to.right_bottom()),
        (from.left_bottom(), to.left_bottom()),
    ] {
        let (u, v) = xf.screen_to_norm(a);
        mesh.vertices.push(egui::epaint::Vertex { pos: b, uv: Pos2::new(u, v), color: Color32::WHITE });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    clipped.add(egui::Shape::mesh(mesh));
}

/// Once a selected corner or the rotate knob is grabbed, text under it cannot take over the gesture. The same
/// applies after the pointer leaves the image/page while dragging.
pub(crate) fn image_owns_pointer(ui: &egui::Ui, xf: &PageXform, page: usize, info: &DocInfo, view: &DocView) -> bool {
    let Some(selection) = view.image_selection.as_ref().filter(|s| s.page == page) else { return false };
    if selection.drag.is_some() {
        return true;
    }
    let Some(image) = view.edit_images.get(&page).and_then(|(_, images)| images.get(selection.index)) else { return false };
    let Some(rect) = image.visible_rect else { return false };
    let b = xf.user_rect(info, page, rect.map(|v| v as f32));
    let p = ui.input(|i| if i.pointer.primary_down() { i.pointer.press_origin() } else { i.pointer.hover_pos() });
    let knob = rotate_knob(b, ui.clip_rect()).0;
    p.is_some_and(|p| [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom(), knob].iter().any(|c| c.distance(p) < 8.0))
}

/// What a drag on a paragraph box takes hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grip {
    Move,
    /// An edge (screen left or right), dragged to rewrap the paragraph.
    Left,
    Right,
}

/// A paragraph box being dragged: moved, or (from a handle on its left or right edge) resized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockDrag {
    page: usize,
    block: usize,
    /// A paragraph index belongs to the document generation where the press was resolved.
    generation: u64,
    /// Where the drag started (screen).
    start: Pos2,
    grip: Grip,
}

/// How far an edge grip reaches either side of a box's edge (screen pixels).
const EDGE_REACH: f32 = 6.0;
/// How far outside an open paragraph's frame the pointer still means the frame (screen pixels).
const FRAME_REACH: f32 = 4.0;

/// The rewrap handles drawn on a paragraph box's left and right edges (screen).
fn width_handles(b: Rect) -> [Rect; 2] {
    let h = (b.height() * 0.5).clamp(14.0, 28.0);
    [b.left(), b.right()].map(|x| Rect::from_center_size(Pos2::new(x, b.center().y), egui::vec2(7.0, h)))
}

/// The edge of `b` that `p` grabs: anywhere along its height, within reach of the line. A narrow
/// box keeps its middle for moving.
fn edge_at(b: Rect, p: Pos2) -> Option<Grip> {
    if p.y < b.top() - EDGE_REACH || p.y > b.bottom() + EDGE_REACH {
        return None;
    }
    let inside = EDGE_REACH.min(b.width() / 4.0);
    let (dl, dr) = (p.x - b.left(), b.right() - p.x);
    match (dl >= -EDGE_REACH && dl <= inside, dr >= -EDGE_REACH && dr <= inside) {
        (true, true) if dl <= dr => Some(Grip::Left),
        (_, true) => Some(Grip::Right),
        (true, false) => Some(Grip::Left),
        _ => None,
    }
}

/// The topmost box under `p` and what it grabs there; edges only on an upright page.
fn grab_at(boxes: &[Rect], p: Pos2, upright: bool) -> Option<(usize, Grip)> {
    boxes.iter().enumerate().rev().find_map(|(i, b)| match upright.then(|| edge_at(*b, p)).flatten() {
        Some(g) => Some((i, g)),
        None => b.contains(p).then_some((i, Grip::Move)),
    })
}

/// A paragraph's box on screen: its text rect (user space), with a little room around it.
fn paragraph_box(xf: &PageXform, info: &DocInfo, page: usize, rect: [f64; 4]) -> Rect {
    xf.user_rect(info, page, rect.map(|v| v as f32)).expand(2.0)
}

/// The width handles need the page upright (or upside down): on a quarter-turned page the
/// screen's horizontal is the paragraph's vertical.
fn has_edges(info: &DocInfo, page: usize) -> bool {
    info.pages.get(page).is_some_and(|p| p.rotation % 180 == 0)
}

/// Whether a press at `p` takes hold of the width handle of one of the paragraphs `rects` (user
/// space). The handle is drawn half outside the box and grabs from a little further out still,
/// where a press is not on the box itself, so area selection must leave it to [`page_input`].
pub(crate) fn grabs_edge(xf: &PageXform, info: &DocInfo, page: usize, mut rects: impl Iterator<Item = [f64; 4]>, p: Pos2) -> bool {
    has_edges(info, page) && rects.any(|r| edge_at(paragraph_box(xf, info, page, r), p).is_some())
}

/// What a right-click on a selected image asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ImageAction {
    Replace(usize, usize),
    Save(usize, usize),
    Copy(usize, usize),
    CopyFile(usize, usize),
    /// Copy the picture, then remove it from the page.
    Cut(usize, usize),
    /// Mark the picture for redaction (applied later from Redact a PDF).
    Redact(usize, usize),
    /// Create a link over the picture.
    Link(usize, usize),
    /// Open Recognize Text for the picture's page.
    Recognize(usize),
}

/// Where the rotate knob sits, and the edge point its stem starts from: above the middle of the
/// top edge, or below the bottom edge when the area on screen (`view`) has no room above.
fn rotate_knob(b: Rect, view: Rect) -> (Pos2, Pos2) {
    let above = b.center_top() - egui::vec2(0.0, 22.0);
    if above.y - 6.0 >= view.top() { (above, b.center_top()) } else { (b.center_bottom() + egui::vec2(0.0, 22.0), b.center_bottom()) }
}

/// Degrees clockwise (-180..=180) that turn "straight up" from `centre` onto the pointer; `snap`
/// rounds to steps of 15 degrees (Shift). A pointer on the centre turns nothing.
fn turn_degrees(centre: Pos2, pointer: Pos2, snap: bool) -> f32 {
    let v = pointer - centre;
    if !(v.x.is_finite() && v.y.is_finite()) || v.length() < 4.0 {
        return 0.0;
    }
    // Screen y points down: up is (0, -1); clockwise is atan2(x, -y).
    let d = v.x.atan2(-v.y).to_degrees();
    if snap { (d / 15.0).round() * 15.0 } else { d }
}

fn user_box(xf: &PageXform, info: &DocInfo, page: usize, r: Rect) -> [f64; 4] {
    let p = &info.pages[page];
    let (a, b) = (xf.screen_to_view(r.min), xf.screen_to_view(r.max));
    let (u, v) = (p.view_to_user(a.0, a.1), p.view_to_user(b.0, b.1));
    [u[0].min(v[0]) as f64, u[1].min(v[1]) as f64, u[0].max(v[0]) as f64, u[1].max(v[1]) as f64]
}

/// Images on a page: select, move, resize, right-click. Returns `true` when the pointer was used.
#[allow(clippy::too_many_arguments)]
pub(crate) fn image_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    images: &[pdfcraft_engine::PageImage],
    view: &mut DocView,
    action: &mut Option<ImageAction>,
    allow_interaction: bool,
) -> bool {
    let boxes: Vec<Rect> = images.iter().map(|im| im.visible_rect.map_or(Rect::NOTHING, |r| xf.user_rect(info, page, r.map(|v| v as f32)))).collect();
    let painter = ui.painter();
    let hover_pos = ui.input(|i| i.pointer.hover_pos());
    let image_contains = |index: usize, point: Pos2| {
        let (x, y) = xf.screen_to_view(point);
        let Some(p) = info.pages.get(page) else { return false };
        let user = p.view_to_user(x, y);
        images.get(index).is_some_and(|im| im.contains(user.map(f64::from)))
    };
    let selected = view.image_selection.as_ref().filter(|s| s.page == page).map(|s| s.index).filter(|i| *i < boxes.len());
    let dragging = view.image_selection.as_ref().is_some_and(|s| s.drag.is_some());
    // Few images: faint outlines for all of them, as before. Many: only the selected and
    // hovered boxes, so a ~1000-image CAD page stays interactive in Edit text & images.
    // Fully clipped placements (empty visible frames) get no outline.
    let faint_all = boxes.len() <= MAX_OUTLINES;
    // The picture being dragged has its frame at the pointer; its outline where it was would
    // trail behind until the release.
    let moving = selected.filter(|_| dragging);
    if faint_all {
        for (_, b) in boxes.iter().enumerate().filter(|(i, b)| b.is_positive() && moving != Some(*i)) {
            painter.rect_stroke(*b, CornerRadius::ZERO, Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
        }
    }
    let hovered_box = hover_pos.and_then(|p| boxes.iter().enumerate().rposition(|(i, b)| b.is_positive() && b.contains(p) && image_contains(i, p)));
    if !faint_all {
        for i in selected.filter(|_| moving.is_none()).into_iter().chain(hovered_box.filter(|h| Some(*h) != selected)) {
            if let Some(b) = boxes.get(i).filter(|b| b.is_positive()) {
                painter.rect_stroke(*b, CornerRadius::ZERO, Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
            }
        }
    }
    for i in selected.into_iter().chain(hovered_box).collect::<Vec<_>>() {
        let Some(b) = boxes.get(i).copied().filter(|b| b.is_positive()) else { continue };
        if selected == Some(i) {
            // The selected frame and handles are drawn below (or the drag preview instead).
            if !dragging {
                painter.rect_filled(b, CornerRadius::ZERO, ACCENT.gamma_multiply(0.04));
            }
        } else {
            painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.5, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
    // The selected image: frame, corner handles, dragging.
    if let Some(i) = selected.filter(|i| boxes.get(*i).is_some_and(|b| b.is_positive())) {
        let b = boxes[i];
        if view.image_selection.as_ref().is_some_and(|s| s.crop.is_some()) {
            return crop_input(ui, resp, xf, page, info, view, i, b, allow_interaction);
        }
        let corners = [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom()];
        let handles = |rect: Rect| {
            for c in [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()] {
                painter.rect(
                    Rect::from_center_size(c, egui::vec2(8.0, 8.0)),
                    CornerRadius::ZERO,
                    Color32::WHITE,
                    Stroke::new(1.0, ACCENT),
                    egui::StrokeKind::Middle,
                );
            }
        };
        // Until the page is rendered again its raster has an applied move or resize's picture
        // where it was: it stays blanked there and drawn where it went.
        // The whole-page raster and its tiles are replaced separately: the old frame stays blanked
        // while anything still shows the picture there, but its pixels come from the old frame
        // only while the whole-page raster itself is the old one. Once that is fresh it has the
        // picture where it went, and that is where they come from.
        let (out_of_date, raster_old, raster) = (view.page_out_of_date(page), view.page_raster_out_of_date(page), view.page_raster(page));
        let screen = |r: Rect| Rect::from_two_pos(xf.norm_to_screen(r.min.x, r.min.y), xf.norm_to_screen(r.max.x, r.max.y));
        // Where the screen may still show the picture from before an applied move, while the page
        // isn't drawn again yet. Another drag keeps it blanked too.
        let left_at = view.image_selection.as_ref().and_then(|s| s.settled).filter(|_| out_of_date).map(|(from, _)| from);
        if let Some(s) = view.image_selection.as_mut().filter(|s| s.drag.is_none()) {
            match (s.settled, raster) {
                (Some((from, to)), Some(tex)) if out_of_date => {
                    let source = if raster_old { from } else { to };
                    stretched_picture(painter, xf, tex, &[screen(from)], screen(source), screen(to));
                }
                _ => s.settled = None,
            }
        }
        if view.image_selection.as_ref().is_some_and(|s| s.drag.is_none()) {
            painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
            handles(b);
        }
        if allow_interaction && let Some(corner) = hover_pos.and_then(|p| corners.iter().position(|c| c.distance(p) < 8.0)) {
            ui.ctx().set_cursor_icon(if matches!(corner, 0 | 3) { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::ResizeNeSw });
        }
        // The rotate handle: a knob above the top edge, joined to it by a short stem.
        let (knob, stem) = rotate_knob(b, ui.clip_rect());
        if view.image_selection.as_ref().is_some_and(|s| s.drag.is_none()) {
            painter.line_segment([stem, knob], Stroke::new(1.0, ACCENT));
            painter.circle(knob, 5.0, Color32::WHITE, Stroke::new(1.0, ACCENT));
            if allow_interaction && hover_pos.is_some_and(|p| p.distance(knob) < 8.0) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
        let origin = ui.input(|i| i.pointer.press_origin());
        if allow_interaction
            && resp.drag_started()
            && let Some(o) = origin
            && o.distance(knob) < 8.0
        {
            if let Some(s) = view.image_selection.as_mut() {
                s.drag = Some((o, None));
                s.turning = true;
            }
        } else if allow_interaction
            && resp.drag_started()
            && let Some(o) = origin
        {
            let corner = corners.iter().position(|c| c.distance(o) < 8.0);
            if corner.is_some() || image_contains(i, o) {
                let opposite = corner.map(|k| corners[3 - k]);
                if let Some(s) = view.image_selection.as_mut() {
                    s.drag = Some((o, opposite));
                }
            }
        }
        if view.image_selection.as_ref().is_some_and(|s| s.turning) {
            let shift = ui.input(|i| i.modifiers.shift);
            let degrees = hover_pos.map_or(0.0, |p| turn_degrees(b.center(), p, shift));
            // The turned outline as a preview; the engine redraws the pixels on release.
            let centre = b.center();
            let (sin, cos) = degrees.to_radians().sin_cos();
            let turn = |c: Pos2| {
                let v = c - centre;
                centre + egui::vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
            };
            let outline = [b.left_top(), b.right_top(), b.right_bottom(), b.left_bottom()].map(turn).to_vec();
            painter.add(egui::Shape::closed_line(outline, Stroke::new(1.5, ACCENT)));
            let galley = painter.layout_no_wrap(format!("{degrees:.0}\u{b0}"), egui::FontId::proportional(12.0), ACCENT);
            let label = Rect::from_center_size(turn(knob) - egui::vec2(0.0, 14.0), galley.size() + egui::vec2(8.0, 4.0));
            painter.rect(label, CornerRadius::same(3), Color32::WHITE, Stroke::new(1.0, ACCENT), egui::StrokeKind::Middle);
            painter.galley(label.center() - galley.size() / 2.0, galley, ACCENT);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                if let Some(s) = view.image_selection.as_mut() {
                    s.drag = None;
                    s.turning = false;
                }
                if degrees.abs() >= 0.05 {
                    view.pending_edit =
                        Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::RotateBy(f64::from(degrees)) });
                }
            }
            return true;
        }
        if let Some((start, opposite)) = view.image_selection.as_ref().and_then(|s| s.drag)
            && let Some(p) = hover_pos
        {
            let preview = match opposite {
                // Resize from the opposite corner, keeping the aspect ratio.
                Some(fixed) => {
                    let (w0, h0) = (b.width().max(1.0), b.height().max(1.0));
                    // Use the axis with the larger relative pointer change. Taking the maximum
                    // absolute size freezes shrinking when the other axis hasn't moved. Measure
                    // from the press point so grabbing a handle off-centre cannot cause a jump.
                    let horizontal = (p.x - start.x) * (start.x - fixed.x).signum() / w0;
                    let vertical = (p.y - start.y) * (start.y - fixed.y).signum() / h0;
                    let delta = if horizontal.abs() > vertical.abs() { horizontal } else { vertical };
                    let k = (1.0 + delta).max(0.05);
                    let (w, h) = (w0 * k, h0 * k);
                    let x = if start.x < fixed.x { fixed.x - w } else { fixed.x };
                    let y = if start.y < fixed.y { fixed.y - h } else { fixed.y };
                    Rect::from_min_size(Pos2::new(x, y), egui::vec2(w, h))
                }
                None => b.translate(p - start),
            };
            // Move/resize the full placement by the visible frame's change. The engine
            // transforms an isolated crop together with its image, preserving its contents.
            let raw = images.get(i).map_or(b, |im| xf.user_rect(info, page, im.rect.map(|v| v as f32)));
            // A zero-width or zero-height visible frame (or a degenerate ratio) keeps the
            // placement's scale instead of producing inf/NaN.
            let ratio = |new: f32, old: f32| if old.abs() > 1e-3 && (new / old).is_finite() { new / old } else { 1.0 };
            let factor = egui::vec2(ratio(preview.width(), b.width()), ratio(preview.height(), b.height()));
            let target = Rect::from_min_max(preview.min + (raw.min - b.min) * factor, preview.min + (raw.max - b.min) * factor);
            let visible = preview.intersect(xf.rect);
            if visible.is_positive() {
                // The pixels follow the frame: the page raster's picture of the image, stretched
                // into the preview over a blank where it was. Re-rendering every frame of a drag
                // would lag; the edit renders it properly on release.
                if preview != b
                    && let Some(tex) = view.page_raster(page)
                {
                    // Its pixels: where the whole-page raster has it.
                    let source = left_at.filter(|_| raster_old).map_or(b, screen);
                    let blank: Vec<Rect> = left_at.map(screen).into_iter().chain(std::iter::once(b)).collect();
                    stretched_picture(painter, xf, tex, &blank, source, preview);
                }
                painter.rect_filled(visible, CornerRadius::ZERO, ACCENT.gamma_multiply(0.04));
                painter.rect_stroke(visible, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
                handles(preview);
            }
            if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                if let Some(s) = view.image_selection.as_mut() {
                    s.drag = None;
                    let norm = |r: Rect| {
                        let ((u0, v0), (u1, v1)) = (xf.screen_to_norm(r.min), xf.screen_to_norm(r.max));
                        Rect::from_two_pos(Pos2::new(u0, v0), Pos2::new(u1, v1))
                    };
                    s.settled = (preview != b).then(|| (left_at.unwrap_or_else(|| norm(b)), norm(preview)));
                }
                if preview != b {
                    view.pending_edit =
                        Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Move(user_box(xf, info, page, target)) });
                }
            }
            ui.ctx().set_cursor_icon(match opposite {
                Some(fixed) if (start.x - fixed.x) * (start.y - fixed.y) < 0.0 => egui::CursorIcon::ResizeNeSw,
                Some(_) => egui::CursorIcon::ResizeNwSe,
                None => egui::CursorIcon::Move,
            });
            return true;
        }
        if allow_interaction
            && ui.input(|inp| inp.key_pressed(egui::Key::Delete) || inp.key_pressed(egui::Key::Backspace))
            && !ui.ctx().egui_wants_keyboard_input()
        {
            view.image_selection = None;
            view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Delete });
            return true;
        }
    }
    if !allow_interaction {
        return false;
    }
    // An open menu stays drawn wherever the pointer is: most of its items lie outside the picture
    // (a logo is smaller than its menu), and a menu that is not drawn for a frame cannot be clicked.
    // Only a menu opened by a right-click on the picture: one opened over text or empty page while
    // a picture stays selected is the page's own menu.
    let menu_key = resp.id.with("image-menu");
    if resp.secondary_clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(menu_key, false));
    }
    let mut menu_drawn = false;
    if let Some(sel) = selected
        && resp.context_menu_opened()
        && ui.ctx().data(|d| d.get_temp::<bool>(menu_key)).unwrap_or(false)
    {
        resp.context_menu(|ui| image_menu(ui, page, sel, images, view, action));
        menu_drawn = true;
    }
    let Some(p) = hover_pos.filter(|p| xf.rect.contains(*p)) else { return false };
    let Some(hit) = boxes.iter().enumerate().rposition(|(i, b)| b.contains(p) && image_contains(i, p)) else { return false };
    if selected != Some(hit) {
        painter.rect_stroke(boxes[hit], CornerRadius::ZERO, Stroke::new(1.5, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    }
    let Some(b) = boxes.get(hit).copied() else { return false };
    let corner = [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom()].iter().position(|c| c.distance(p) < 8.0);
    ui.ctx().set_cursor_icon(match (selected == Some(hit), corner) {
        (true, Some(0 | 3)) => egui::CursorIcon::ResizeNwSe,
        (true, Some(_)) => egui::CursorIcon::ResizeNeSw,
        (true, None) => egui::CursorIcon::Move,
        _ => egui::CursorIcon::PointingHand,
    });
    if resp.clicked() || resp.secondary_clicked() {
        view.image_selection = Some(ImageSelection { page, index: hit, drag: None, crop: None, turning: false, settled: None });
    }
    if resp.secondary_clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(menu_key, true));
    }
    if selected == Some(hit) && !menu_drawn {
        resp.context_menu(|ui| image_menu(ui, page, hit, images, view, action));
    }
    true
}

/// The right-click menu of the picture `hit` (its rotate, flip, replace, copy, cut, redact, link,
/// recognize and delete items).
fn image_menu(
    ui: &mut egui::Ui,
    page: usize,
    hit: usize,
    images: &[pdfcraft_engine::PageImage],
    view: &mut DocView,
    action: &mut Option<ImageAction>,
) {
    use pdfcraft_engine::ImageEdit as E;
    let items: [(&str, Option<E>); 4] = [
        (tl!("Rotate Clockwise"), Some(E::Rotate(1))),
        (tl!("Rotate Counterclockwise"), Some(E::Rotate(3))),
        (tl!("Flip Horizontal"), Some(E::Flip { horizontal: true })),
        (tl!("Flip Vertical"), Some(E::Flip { horizontal: false })),
    ];
    for (label, change) in items {
        if ui.button(label).clicked() {
            view.pending_edit = change.map(|c| Edit::EditPageImage { page, index: hit, change: c });
            ui.close();
        }
    }
    // Crop: the handles trim instead of resizing until Done.
    if ui.button(tl!("Crop Image")).clicked() {
        if let (Some(s), Some(im)) = (view.image_selection.as_mut(), images.get(hit)) {
            s.crop = Some(CropMode::new(im.visible_rect.unwrap_or(im.rect)));
        }
        ui.close();
    }
    let cropped = images.get(hit).is_some_and(|im| im.visible_rect.is_some_and(|v| v.iter().zip(im.rect.iter()).any(|(a, b)| (a - b).abs() > 0.01)));
    if ui.add_enabled(cropped, egui::Button::new(tl!("Remove Crop"))).clicked() {
        if let Some(im) = images.get(hit) {
            view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Crop(im.rect) });
        }
        ui.close();
    }
    let raster = images.get(hit).is_some_and(|image| !image.is_form);
    if ui.add_enabled(raster, egui::Button::new(tl!("Replace Image…"))).clicked() {
        *action = Some(ImageAction::Replace(page, hit));
        ui.close();
    }
    if ui.add_enabled(raster, egui::Button::new(tl!("Save Image As…"))).clicked() {
        *action = Some(ImageAction::Save(page, hit));
        ui.close();
    }
    // Cutting and copying also find the picture inside grouped Form artwork.
    if ui.button(tl!("Cut Image")).clicked() {
        *action = Some(ImageAction::Cut(page, hit));
        ui.close();
    }
    if ui.button(tl!("Copy Image")).clicked() {
        *action = Some(ImageAction::Copy(page, hit));
        ui.close();
    }
    if ui.button(tl!("Copy Image in Original Format")).clicked() {
        *action = Some(ImageAction::CopyFile(page, hit));
        ui.close();
    }
    ui.separator();
    if ui.button(tl!("Redact Image")).clicked() {
        *action = Some(ImageAction::Redact(page, hit));
        ui.close();
    }
    if ui.button(tl!("Create Link…")).clicked() {
        *action = Some(ImageAction::Link(page, hit));
        ui.close();
    }
    if ui.button(tl!("Recognize Text…")).clicked() {
        *action = Some(ImageAction::Recognize(page));
        ui.close();
    }
    ui.separator();
    if ui.button(tl!("Delete")).clicked() {
        view.image_selection = None;
        view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Delete });
        ui.close();
    }
}

/// Open `block` on `page` in the inline editor. The box's geometry and source are read from the
/// `lines` handed in, so callers pass the document's current inventory.
#[allow(clippy::too_many_arguments)]
fn open_editor(
    ui: &egui::Ui,
    xf: &PageXform,
    doc: &pdfcraft_engine::Document,
    lines: &[pdfcraft_engine::TextBlock],
    boxes: &[Rect],
    page: usize,
    block: usize,
    view: &mut DocView,
) {
    view.pending_text_open = None;
    let (Some(l), Some(b)) = (lines.get(block), boxes.get(block)) else { return };
    view.image_selection = None;
    let source_lines = doc.text_lines(page);
    let text = l.editor_text.clone();
    // Screen pixels per point, from the box's width.
    let scale = (b.width() - 4.0) / ((l.rect[2] - l.rect[0]).max(1.0) as f32);
    // How far the editor box may grow: a single-line paragraph's rewrite grows to the
    // page's right edge, a multi-line paragraph rewraps to its own width.
    let right = xf.rect.right().min(view.viewport_rect().right()) - 6.0;
    let max_width = if l.rows.len() == 1 { (right - b.left()).max(b.width()) } else { b.width() };
    view.line_editor = Some(LineEditor {
        selection: None,
        font_resource: None,
        page,
        block,
        text: text.clone(),
        original: text,
        rect: *b,
        source_rect: l.rect.map(|v| v as f32),
        multiline: l.rows.len() > 1,
        source: Some((l.clone(), source_lines)),
        fonts: install_source_fonts(ui.ctx(), doc, page),
        overflow: false,
        max_width,
        size: (l.size as f32 * scale).clamp(8.0, 72.0),
        focus: true,
        look: look_of(l),
        look0: look_of(l),
        family_chosen: false,
        extras: Extras::default(),
        extras0: Extras::default(),
    });
}

/// Lines and their screen boxes for a page; hover outlines, click opens the editor. Returns
/// `true` when the pointer was used.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    doc: &pdfcraft_engine::Document,
    lines: &[pdfcraft_engine::TextBlock],
    view: &mut DocView,
    allow_interaction: bool,
) -> bool {
    let info = &doc.info;
    if view.block_drag.is_some_and(|d| d.generation != doc.edit_generation()) {
        view.block_drag = None;
        return true;
    }
    let boxes: Vec<Rect> = lines.iter().map(|l| paragraph_box(xf, info, page, l.rect)).collect();
    // Outlines stay on their page, like its text (#3061).
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(xf.rect.expand(1.0)));
    let active = view.line_editor.as_ref().filter(|e| e.page == page).map(|e| e.block);
    for (i, b) in boxes.iter().enumerate() {
        if active != Some(i) {
            painter.rect_stroke(*b, CornerRadius::same(2), Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
        }
    }
    // A box a switching click asked for opens now, once the edit queued with that click has
    // applied and `lines`/`boxes` are the document's new inventory (#2022).
    if allow_interaction
        && active.is_none()
        && let Some((open_page, key)) = view.pending_text_open.clone()
        && open_page == page
    {
        // The edit may have regrouped the page: open the paragraph that was clicked, wherever it
        // is now, and none if it was merged into another (#2516).
        match key.find(lines, 0) {
            Some(block) => open_editor(ui, xf, doc, lines, &boxes, page, block, view),
            None => view.pending_text_open = None,
        }
    }
    if !allow_interaction {
        return false;
    }
    let upright = has_edges(info, page);
    let draw_handles = |b: Rect| {
        for h in width_handles(b) {
            painter.rect(h, CornerRadius::same(2), Color32::WHITE, Stroke::new(1.0, ACCENT), egui::StrokeKind::Middle);
        }
    };
    // A drag in progress: the box follows the pointer (or the grabbed edge does); releasing applies it.
    if let Some(d) = view.block_drag.filter(|d| d.page == page)
        && let (Some(b), Some(l)) = (boxes.get(d.block).copied(), lines.get(d.block))
    {
        let p = ui.input(|i| i.pointer.interact_pos()).unwrap_or(d.start);
        let dx = p.x - d.start.x;
        let preview = match d.grip {
            Grip::Move => b.translate(p - d.start),
            Grip::Left => Rect::from_min_max(Pos2::new((b.left() + dx).min(b.right() - 12.0), b.min.y), b.max),
            Grip::Right => Rect::from_min_max(b.min, Pos2::new((b.right() + dx).max(b.left() + 12.0), b.max.y)),
        };
        painter.rect_stroke(preview, CornerRadius::same(2), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
        let resizing = d.grip != Grip::Move;
        if resizing {
            draw_handles(preview);
        }
        ui.ctx().set_cursor_icon(if resizing { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::Move });
        if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
            view.block_drag = None;
            let (from, to) = (user_box(xf, info, page, b), user_box(xf, info, page, preview));
            let edit = if resizing {
                // The new width in user space: the box's change, added to the paragraph's own. A
                // moved left side (in user space; on an upside-down page that's the screen's right
                // edge) moves the paragraph with it.
                let width = (l.rect[2] - l.rect[0]) + (to[2] - to[0]) - (from[2] - from[0]);
                let shift = to[0] - from[0];
                let style = pdfcraft_engine::BlockStyle { width: Some(width), offset: (shift != 0.0).then_some([shift, 0.0]), ..Default::default() };
                Edit::EditTextBlock { page, block: d.block, text: l.text.clone(), style }
            } else {
                // Translation must keep the selected source operators. Rewriting unchanged text
                // would also remove unrelated paragraphs that now overlap this one (#594).
                Edit::MoveObjects {
                    page,
                    objects: vec![ObjectTarget { kind: ObjectKind::Text, index: d.block }],
                    offset: [to[0] - from[0], to[1] - from[1]],
                }
            };
            if preview != b {
                view.pending_edit = Some(edit);
            }
        }
        return true;
    }
    // Dragging a box (not while a paragraph is open for typing) moves it; from an edge, resizes it.
    // What it grabs comes from where the button went down, not where the pointer is now: egui
    // only calls it a drag once the pointer has moved a few pixels (or been held a moment), and a
    // quick flick has left the edge by then.
    if resp.drag_started()
        && active.is_none()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
        && let Some((block, grip)) = grab_at(&boxes, o, upright)
    {
        view.block_drag = Some(BlockDrag { page, block, generation: doc.edit_generation(), start: o, grip });
        return true;
    }
    let Some(p) = ui.input(|i| i.pointer.hover_pos()).filter(|p| xf.rect.contains(*p)) else { return false };
    let Some((hit, grip)) = grab_at(&boxes, p, upright) else { return false };
    if active != Some(hit) {
        painter.rect_stroke(boxes[hit], CornerRadius::same(2), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
    }
    let on_edge = active.is_none() && grip != Grip::Move;
    if upright && active.is_none() {
        draw_handles(boxes[hit]);
    }
    // A closed paragraph moves when dragged (the four-way arrow says so; a click still opens it
    // for typing); an edge rewraps it. The open one takes the text cursor. Only where the page
    // shows, so the mode bar and side panels over a paragraph scrolled under them keep their own
    // cursor (#2755).
    if resp.contains_pointer() {
        ui.ctx().set_cursor_icon(if on_edge {
            egui::CursorIcon::ResizeHorizontal
        } else if active.is_none() {
            egui::CursorIcon::Move
        } else {
            egui::CursorIcon::Text
        });
    }
    if resp.clicked() {
        view.image_selection = None;
        // A click that moves the editor to another paragraph applies the open one first, so its
        // typed text is not dropped with the switched-away editor (#2022). A click on the open
        // box's own frame, which the editor's overlay does not cover, keeps it open instead.
        if let Some(open) = view.line_editor.as_ref().filter(|e| e.has_changes()) {
            if open.page == page && open.block == hit {
                return true;
            }
            if let Some(old) = view.line_editor.take() {
                let style = old.style();
                if old.text != old.original || style != pdfcraft_engine::BlockStyle::default() {
                    view.applied_editor = Some(old.clone());
                    view.pending_edit = Some(Edit::EditTextBlock { page: old.page, block: old.block, text: old.text, style });
                }
            }
            // The clicked box opens next frame, from the document with that edit applied.
            view.pending_text_open = lines.get(hit).map(|l| (page, pdfcraft_engine::BlockKey::of(l)));
            return true;
        }
        open_editor(ui, xf, doc, lines, &boxes, page, hit, view);
    }
    true
}

/// One TextEdit lays out all source runs; its caret and selection span the whole region.
pub(crate) fn install_source_fonts(ctx: &egui::Context, doc: &pdfcraft_engine::Document, page: usize) -> BTreeMap<String, String> {
    let programs = doc.text_editor_fonts(page);
    let mut definitions = ctx.fonts(|f| f.definitions().clone());
    // Only the current editor's document fonts live in the atlas. Never accumulate the
    // programs of previously opened files or add them to the interface's default families.
    definitions.font_data.retain(|name, _| !name.starts_with("pdf-source-"));
    definitions.families.retain(|family, _| !matches!(family, FontFamily::Name(name) if name.starts_with("pdf-source-")));
    let mut families = BTreeMap::new();
    for (index, program) in programs.iter().enumerate() {
        let name = format!("pdf-source-{index}");
        definitions.font_data.insert(name.clone(), Arc::new(egui::FontData::from_owned(program.bytes.clone())));
        families.insert(program.resource.clone(), name);
    }
    for program in &programs {
        let Some(name) = families.get(&program.resource) else { continue };
        // Word-created PDFs often split one face into several subsets. Use all of the
        // document's matching subsets before falling back for newly inserted characters.
        let mut stack = vec![name.clone()];
        stack.extend(
            programs
                .iter()
                .filter(|p| {
                    pdfcraft_fonts::embedding::face_key(&p.face) == pdfcraft_fonts::embedding::face_key(&program.face)
                        && p.resource != program.resource
                })
                .filter_map(|p| families.get(&p.resource).cloned()),
        );
        if let Some(fallback) = definitions.families.get(&FontFamily::Proportional) {
            stack.extend(fallback.iter().cloned());
        }
        definitions.families.insert(FontFamily::Name(name.clone().into()), stack);
    }
    if !families.is_empty() {
        ctx.set_fonts(definitions);
    }
    families
}

fn editor_job(ui: &egui::Ui, ed: &LineEditor, text: &str, scale: f32, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    // An unchanged PDF row already has an authoritative line break. Do not rewrap it
    // because font rounding or a fallback face measures a fraction of a pixel wider.
    job.wrap.max_width = if text == ed.original && ed.look == ed.look0 && ed.extras == ed.extras0 { f32::INFINITY } else { width };
    if let Some((block, lines)) = &ed.source {
        let lefts: Vec<_> = block.rows.iter().filter_map(|r| r.first().and_then(|i| lines.get(*i)).map(|l| l.rect[0])).collect();
        let left_span = lefts.iter().copied().fold(f64::NEG_INFINITY, f64::max) - lefts.iter().copied().fold(f64::INFINITY, f64::min);
        let centered = block.rows.len() > 1
            && left_span > block.size * 0.5
            && block.rows.iter().all(|row| {
                let rect = row
                    .iter()
                    .filter_map(|i| lines.get(*i))
                    .map(|l| l.rect)
                    .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
                rect.is_some_and(|r| ((r[0] + r[2] - block.rect[0] - block.rect[2]) / 2.0).abs() < block.size)
            });
        if centered {
            job.halign = egui::Align::Center;
        }
        for (range, index) in block.edited_spans(text) {
            let Some(line) = lines.get(index) else { continue };
            let mut look = source_look(&line.base_font, line.size, line.color, line.bold, line.italic);
            look.size = line.size;
            if ed.look.family != ed.look0.family || ed.family_chosen {
                look.family = ed.look.family;
            }
            if ed.look.bold != ed.look0.bold {
                look.bold = ed.look.bold;
            }
            if ed.look.italic != ed.look0.italic {
                look.italic = ed.look.italic;
            }
            if ed.look.color != ed.look0.color {
                look.color = ed.look.color;
            }
            if ed.look.size != ed.look0.size {
                look.size = ed.look.size;
            }
            let mut font = editor_font(&look, (look.size as f32 * scale).clamp(1.0, 300.0));
            let original_face =
                ed.look.family == ed.look0.family && !ed.family_chosen && ed.look.bold == ed.look0.bold && ed.look.italic == ed.look0.italic;
            let embedded = original_face && ed.fonts.contains_key(&line.font);
            if embedded && let Some(name) = ed.fonts.get(&line.font) {
                font.family = FontFamily::Name(name.clone().into());
            }
            let native_width = ui.fonts_mut(|f| f.layout_no_wrap(line.text.clone(), font.clone(), Color32::BLACK).size().x);
            let target_width = ((line.rect[2] - line.rect[0]) * look.size / line.size.max(0.01)) as f32 * scale;
            let spacing = if original_face { (target_width - native_width) / line.text.chars().count().saturating_sub(1).max(1) as f32 } else { 0.0 };
            job.append(
                text.get(range).unwrap_or_default(),
                0.0,
                egui::TextFormat {
                    font_id: font,
                    extra_letter_spacing: spacing + ed.extras.char_spacing as f32 * scale,
                    color: color32(look.color),
                    italics: look.italic && !embedded,
                    ..Default::default()
                },
            );
        }
    } else {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: editor_font(&ed.look, ed.size),
                color: color32(ed.look.color),
                italics: ed.look.italic,
                ..Default::default()
            },
        );
    }
    job
}

/// `galley` drawn in no colour: its rows still place the caret and the selection.
fn unseen(mut galley: Arc<egui::Galley>) -> Arc<egui::Galley> {
    for placed in &mut Arc::make_mut(&mut galley).rows {
        for vertex in &mut Arc::make_mut(&mut placed.row).visuals.mesh.vertices {
            vertex.color = Color32::TRANSPARENT;
        }
    }
    galley
}

/// Align the editable galley's baselines to the source rows instead of egui's interface
/// leading. Glyphs, meshes, caret and selection receive the same transform.
fn editor_galley(ui: &egui::Ui, ed: &LineEditor, text: &str, scale: f32, width: f32) -> Arc<egui::Galley> {
    let mut job = editor_job(ui, ed, text, scale, width);
    let centered = job.halign == egui::Align::Center;
    job.halign = egui::Align::LEFT;
    let mut galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let Some((block, lines)) = &ed.source else { return galley };
    let sources: Vec<_> = block
        .rows
        .iter()
        .filter_map(|row| {
            let first = row.first().and_then(|i| lines.get(*i))?;
            let rect = row
                .iter()
                .filter_map(|i| lines.get(*i))
                .map(|l| l.rect)
                .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
            Some((first.baseline()[1], rect))
        })
        .collect();
    let Some((first_baseline, first_rect)) = sources.first().copied() else { return galley };
    let leading = if ed.extras.line_spacing > 0.0 {
        ed.extras.line_spacing * ed.look.size
    } else {
        let fallback = if block.lines.len() == 1 && !text.contains('\n') {
            block.lines.first().and_then(|i| lines.get(*i)).map_or(block.size * 1.2, |line| line.wrap_leading())
        } else {
            block.size * 1.5
        };
        sources.windows(2).map(|r| (r[0].0 - r[1].0).abs()).rfind(|v| *v > 0.0).unwrap_or(fallback)
    };
    let g = Arc::make_mut(&mut galley);
    let mut bounds = Rect::NOTHING;
    let mut meshes = Rect::NOTHING;
    for (index, placed) in g.rows.iter_mut().enumerate() {
        let (last_baseline, last_rect) = sources.last().copied().unwrap_or((first_baseline, first_rect));
        let (baseline, source_rect) = sources
            .get(index)
            .copied()
            .unwrap_or((last_baseline - index.saturating_add(1).saturating_sub(sources.len()) as f64 * leading, last_rect));
        let changed_size = ed.look.size != ed.look0.size;
        let ratio = if changed_size { ed.look.size / ed.look0.size.max(0.01) } else { 1.0 };
        let ascent = (source_rect[3] - sources.get(index).map_or(last_baseline, |s| s.0)) as f32 * scale * ratio as f32;
        let descent = (sources.get(index).map_or(last_baseline, |s| s.0) - source_rect[1]) as f32 * scale * ratio as f32;
        let base_y = (block.rect[3] - baseline) as f32 * scale;
        let row = Arc::make_mut(&mut placed.row);
        placed.pos.y = base_y - ascent;
        placed.pos.x = if centered { (width - row.size.x) / 2.0 } else { (source_rect[0] - block.rect[0]) as f32 * scale };
        for glyph in &mut row.glyphs {
            let dy = ascent - glyph.pos.y;
            glyph.pos.y = ascent;
            glyph.font_ascent = ascent;
            glyph.font_height = ascent + descent;
            glyph.line_height = ascent + descent;
            if !glyph.uv_rect.is_nothing() {
                for vertex in row.visuals.mesh.vertices.iter_mut().skip(glyph.first_vertex as usize).take(4) {
                    vertex.pos.y += dy;
                }
            }
        }
        row.size.y = (ascent + descent).max(1.0);
        row.visuals.mesh_bounds = row.visuals.mesh.vertices.iter().fold(Rect::NOTHING, |r, v| r.union(Rect::from_min_max(v.pos, v.pos)));
        bounds = bounds.union(Rect::from_min_size(placed.pos, row.size));
        meshes = meshes.union(row.visuals.mesh_bounds.translate(placed.pos.to_vec2()));
    }
    g.rect = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(bounds.right().max(0.0), bounds.bottom().max(0.0)));
    g.mesh_bounds = meshes;
    galley
}

/// The inline editor; returns the edit once the text is applied.
pub(crate) fn overlay(ctx: &egui::Context, view: &mut DocView, info: &DocInfo) -> Option<Edit> {
    // Clicks outside the document (the Format text panel) keep the paragraph open.
    let outside = ctx.input(|i| i.pointer.latest_pos()).is_some_and(|p| !view.viewport_rect().contains(p));
    let page = view.line_editor.as_ref()?.page;
    let xf = view.page_xform(page)?;
    let hold = view.objects.hold_editor;
    // An OCR word is invisible text over a scan: the textless background still shows the scanned
    // word, so the editor covers it, as the applied edit will.
    let background_ready = view
        .line_editor
        .as_ref()
        .is_some_and(|ed| view.text_background.ready(ed.page, ed.block) && !ed.source.as_ref().is_some_and(|(b, _)| b.invisible));
    // The measured extent of this paragraph's draft: a click straight from another paragraph
    // opens this one after the frame's measuring, which was the other one's.
    let draft_rect = view
        .line_editor
        .as_ref()
        .and_then(|ed| view.text_background.draft_rect_for(ed.page, ed.block))
        .map(|r| xf.user_rect(info, page, r.map(|v| v as f32)));
    // The editor draws on its page only, as the page itself does: a paragraph running past the
    // page's edge (a converter relying on the page clip) must not cover the next page or the
    // tool bars (#3061).
    let viewport = view.viewport_rect();
    let page_clip = xf.rect.intersect(viewport);
    let ed = view.line_editor.as_mut()?;
    if hold {
        ed.focus = true;
    }
    // Reproject the source box every frame. The page may have been zoomed, scrolled or rotated
    // while the format panel was open.
    // A font definition becomes active at the start of the next egui pass. Keep the source
    // raster visible until then instead of flashing an unrelated interface face.
    if !ctx.fonts(|f| ed.fonts.values().all(|name| f.definitions().families.contains_key(&FontFamily::Name(name.clone().into())))) {
        ctx.request_repaint();
        return None;
    }
    ed.rect = xf.user_rect(info, ed.page, ed.source_rect);
    ed.max_width = ed.rect.width();
    let scale = (ed.rect.width() / (ed.source_rect[2] - ed.source_rect[0]).abs().max(0.01)).max(0.01);
    ed.size = (ed.look.size as f32 * scale).clamp(1.0, 300.0);
    // egui draws the caret at least one row of this font tall. Each row already has its source
    // row's height, so the font stays below every row (half the region's smallest size): on a
    // 12 pt row under a 24 pt title the caret is as tall as the row, not the title.
    let smallest =
        ed.source.as_ref().and_then(|(block, lines)| block.lines.iter().filter_map(|i| lines.get(*i)).map(|l| l.size as f32).reduce(f32::min));
    let font = editor_font(&ed.look, smallest.map_or(ed.size, |s| (s * scale * 0.5).clamp(1.0, 300.0).min(ed.size)));
    let text_color = color32(ed.look.color);
    let block = ed.block;
    let mut done = None;
    let mut reveal = false;
    let mut look = None;
    // No fade-in: while it faded, the page's own copy of the text showed through the cover.
    // Over the page, so only where the document shows: a paragraph scrolled under the mode bar or
    // a side panel must leave them their pointer (#2755). It stays at its paragraph, cut off as
    // the page is, rather than pushed back into the window.
    egui::Area::new(egui::Id::new("edit-text-line"))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .fixed_pos(Pos2::new(ed.rect.left(), ed.rect.top()))
        .constrain_to(viewport)
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(page_clip));
            // The editor opens at the source size. Once text changes, show the real draft
            // extent: single rows grow toward the page edge, paragraphs rewrap at their
            // source width, and the visible frame grows/shrinks vertically with their rows.
            // An OCR phrase (several stretched words on one scanned line) also grows rightwards.
            let single_run = ed.source.as_ref().is_some_and(|(block, _)| block.rows.len() == 1 || block.invisible);
            let width = if !ed.multiline && single_run && !ed.text.contains('\n') && ed.has_changes() {
                let job = editor_job(ui, ed, &ed.text, scale, f32::INFINITY);
                let natural = ui.fonts_mut(|f| f.layout_job(job)).size().x;
                natural.max(ed.size * 0.4).min((xf.rect.right() - ed.rect.left() - 36.0 * scale).max(ed.rect.width()))
            } else {
                ed.rect.width().max(1.0)
            };
            ui.set_width(width);
            let measured = editor_galley(ui, ed, &ed.text, scale, width);
            ed.overflow = ed.has_changes() && (measured.rect.width() > ed.rect.width() + 1.5 || measured.rect.height() > ed.rect.height() + 1.5);
            let visible = Rect::from_min_size(ed.rect.min, measured.rect.size().max(egui::vec2(1.0, 1.0)));
            // Unsupported source programs use an interface fallback whose glyph advances
            // can differ from the PDF's. Keep every visible row inside the growing frame.
            let live = draft_rect.map_or(visible, |bounds| bounds.union(visible));
            ui.painter().rect_stroke(live.expand(2.0), CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
            // On the frame, a press takes hold of the box (a click selects it, a drag moves it):
            // the pointer says so there, and reads as text inside.
            if let Some(p) = ui.input(|i| i.pointer.hover_pos())
                && ui.clip_rect().contains(p)
                && live.expand(2.0 + FRAME_REACH).contains(p)
                && !live.expand(2.0).contains(p)
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            }
            // White, unless the text is too pale to read on white (white text from a coloured
            // banner, #913): then a dark box, with a caret that shows on it.
            let fill = if background_ready { Color32::TRANSPARENT } else { editor_fill(text_color) };
            // An unchanged paragraph is the page's own text: the editor draws only its frame and
            // caret over it. Once it changes, the textless page goes under the editor's text or,
            // until that background is ready, a cover over the entire source box, including lines
            // the draft no longer occupies. Shrinking the text must never expose a copy. Both are
            // decided after this frame's typing, in places kept under the editor's text.
            let cover = ui.painter().add(egui::Shape::Noop);
            let unchanged = !ed.has_changes();
            egui::Frame::NONE.show(ui, |ui| {
                // A textless PDF preview, or the page itself, keeps its real background through
                // the transparent editor. The temporary opaque cover uses the shared caret-contrast rule.
                ui.visuals_mut().text_cursor.stroke.color = if background_ready || unchanged {
                    if luminance(text_color) < 0.5 { Color32::BLACK } else { Color32::WHITE }
                } else {
                    caret_color_on(fill)
                };
                let rows = 1;
                let snapshot = ed.clone();
                let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
                    let galley = editor_galley(ui, &snapshot, text.as_str(), scale, wrap_width);
                    if snapshot.changes_with(text.as_str()) { galley } else { unseen(galley) }
                };
                let output = egui::TextEdit::multiline(&mut ed.text)
                    .id(egui::Id::new("edit-text-line-input"))
                    .font(font.clone())
                    .margin(egui::Margin::ZERO)
                    .layouter(&mut layouter)
                    .text_color(text_color)
                    .frame(egui::Frame::NONE)
                    .desired_width(width)
                    .desired_rows(rows)
                    .show(ui);
                if ed.has_changes() {
                    if background_ready {
                        reveal = true;
                    } else {
                        ui.painter().set(cover, egui::Shape::rect_filled(ed.rect.expand(1.0).union(output.response.rect), CornerRadius::ZERO, fill));
                    }
                }
                look = Some((Arc::clone(&output.galley), output.galley_pos));
                if let Some(cursor) = output.cursor_range {
                    let selected = cursor.as_sorted_char_range();
                    let byte = |index| ed.text.char_indices().nth(index).map_or(ed.text.len(), |(at, _)| at);
                    ed.selection = (!selected.is_empty()).then(|| byte(selected.start.into())..byte(selected.end.into()));
                }
                let r = output.response;
                if r.changed() {
                    // The next pass measures the updated draft with the Apply/Save rewrite.
                    ctx.request_repaint();
                }
                if ed.focus {
                    r.request_focus();
                    ed.focus = false;
                }
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let apply = ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
                if esc {
                    done = Some(false);
                } else if apply || (r.lost_focus() && !outside && !hold) {
                    done = Some(true);
                }
            });
        });
    if reveal {
        view.text_background.reveal();
    }
    if let Some((galley, pos)) = look {
        view.text_background.remember(&xf, page, block, galley, pos, text_color);
    }
    match done {
        Some(apply) => {
            let ed = view.line_editor.take()?;
            if !apply {
                return None;
            }
            view.applied_editor = Some(ed.clone());
            finished_edit(ed)
        }
        None => None,
    }
}

/// The edit a closed paragraph editor applies, or `None` when nothing in it changed.
pub(crate) fn finished_edit(ed: LineEditor) -> Option<Edit> {
    let style = ed.style();
    (ed.text != ed.original || style != pdfcraft_engine::BlockStyle::default()).then_some(Edit::EditTextBlock {
        page: ed.page,
        block: ed.block,
        text: ed.text,
        style,
    })
}

/// Caret colour that stays visible on a field filled with `fill`: black on light fills, white on dark ones.
pub(crate) fn caret_color_on(fill: Color32) -> Color32 {
    let luma = 299 * u32::from(fill.r()) + 587 * u32::from(fill.g()) + 114 * u32::from(fill.b());
    if luma > 127_500 { Color32::BLACK } else { Color32::WHITE }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rotate_knob_moves_below_when_there_is_no_room_above() {
        let view = Rect::from_min_max(Pos2::new(0.0, 100.0), Pos2::new(500.0, 500.0));
        let roomy = Rect::from_min_max(Pos2::new(100.0, 200.0), Pos2::new(200.0, 250.0));
        assert_eq!(rotate_knob(roomy, view).0, Pos2::new(150.0, 178.0));
        let at_top = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(200.0, 150.0));
        let (knob, stem) = rotate_knob(at_top, view);
        assert_eq!((knob, stem), (Pos2::new(150.0, 172.0), Pos2::new(150.0, 150.0)));
    }

    #[test]
    fn the_rotate_knob_measures_clockwise_degrees_from_straight_up() {
        let c = Pos2::new(100.0, 100.0);
        let at = |x: f32, y: f32| turn_degrees(c, Pos2::new(x, y), false);
        assert!(at(100.0, 0.0).abs() < 1e-3);
        assert!((at(200.0, 100.0) - 90.0).abs() < 1e-3, "right is a clockwise quarter");
        assert!((at(0.0, 100.0) + 90.0).abs() < 1e-3);
        assert!((at(100.0, 200.0).abs() - 180.0).abs() < 1e-3);
        // Shift rounds to 15 degrees; the centre and non-finite points turn nothing.
        assert_eq!(turn_degrees(c, Pos2::new(180.0, 50.0), true) % 15.0, 0.0);
        assert_eq!(at(101.0, 101.0), 0.0);
        assert_eq!(at(f32::NAN, 5.0), 0.0);
    }

    /// An app with one text document open, and an editor on its first paragraph.
    fn preview_app() -> (crate::PdfCraftApp, LineEditor) {
        let mut app = crate::PdfCraftApp::new();
        let bytes = app.session.create_from_text("Preview", "Preview text").unwrap();
        app.open_bytes("preview.pdf", None, bytes.as_ref().clone()).unwrap();
        let doc = app.session.get(app.views[0].id).unwrap();
        let block = doc.text_blocks(0).remove(0);
        let ed = LineEditor {
            selection: None,
            font_resource: None,
            page: 0,
            block: 0,
            text: block.text.clone(),
            original: block.text.clone(),
            rect: Rect::NOTHING,
            source_rect: block.rect.map(|v| v as f32),
            multiline: block.rows.len() > 1,
            source: None,
            fonts: BTreeMap::new(),
            overflow: false,
            max_width: 400.0,
            size: block.size as f32,
            focus: true,
            look: look_of(&block),
            look0: look_of(&block),
            family_chosen: false,
            extras: Extras::default(),
            extras0: Extras::default(),
        };
        (app, ed)
    }

    #[test]
    fn picking_the_family_the_font_name_reads_as_still_changes_the_font() {
        // A Calibri paragraph reads as Helvetica; picking Helvetica from the list used to leave
        // `look` equal to what the paragraph had, so no family change was sent and Calibri stayed.
        let (_app, mut ed) = preview_app();
        let family = ed.look0.family;
        assert_eq!(ed.style().family, None, "nothing chosen yet");
        ed.choose_family(family);
        assert_eq!(ed.style().family, Some((family, ed.look.bold, ed.look.italic)));
        assert!(ed.has_changes());
        assert_eq!(ed.font_shown(&[]), family.label());
        ed.keep_source_font();
        assert_eq!(ed.style().family, None, "Keep source fonts withdraws the choice");
        ed.choose_family(family);
        ed.applied();
        assert_eq!(ed.style().family, None, "an applied choice is the paragraph's font now");
    }

    #[test]
    fn a_loaded_font_is_applied_to_the_paragraph_being_edited() {
        // Loading a font used to only register it on the page: the paragraph kept its source
        // face until the font was found again in the document-font menu (#2591).
        let (mut app, ed) = preview_app();
        app.views[0].line_editor = Some(ed);
        let inter = include_bytes!("../../../assets/fonts/Inter-Regular.ttf").to_vec();
        app.use_files(crate::files::FilePurpose::TextFont { page: 0 }, vec![("Inter-Regular.ttf".into(), inter)]);
        let doc = app.session.get(app.views[0].id).unwrap();
        let lines = doc.text_lines(0);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>(), "Preview text");
        assert!(lines.iter().all(|l| l.base_font == "Inter-Regular"), "{:?}", lines.iter().map(|l| &l.base_font).collect::<Vec<_>>());
        assert!(app.views[0].line_editor.is_some(), "the paragraph stays open for editing");
    }

    #[test]
    fn save_applies_the_paragraph_open_in_the_editor() {
        // Save wrote the file without the paragraph still open in the editor (#3496).
        let (mut app, mut ed) = preview_app();
        ed.text = "Text preview".into();
        app.views[0].line_editor = Some(ed);
        let dir = std::env::temp_dir().join(format!("pdfcraft-ui-open-paragraph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        app.save_override = Some(dir.join("out.pdf").to_string_lossy().into_owned());

        assert!(app.save_active(crate::SaveTarget::InPlace));
        assert!(app.views[0].line_editor.is_none());
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(doc.text_lines(0).iter().map(|l| l.text.as_str()).collect::<String>(), "Text preview");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_text_box_does_not_lose_the_open_paragraph() {
        let (mut app, mut ed) = preview_app();
        ed.text = "Text preview".into();
        app.views[0].line_editor = Some(ed);
        let draft = crate::content_ui::TextDraft {
            page: 9,
            index: None,
            rect: [20.0, 20.0, 120.0, 40.0],
            text: "Typed text".into(),
            style: crate::content_ui::default_style(),
            focus: true,
        };
        app.views[0].content.draft = Some(draft.clone());

        assert!(!app.commit_typing_in(app.views[0].id));
        assert_eq!(app.views[0].content.draft, Some(draft), "the refused text box stays open");
        assert!(app.views[0].line_editor.is_none(), "the paragraph was applied");
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(doc.text_lines(0).iter().map(|l| l.text.as_str()).collect::<String>(), "Text preview");
    }

    #[test]
    fn a_refused_paragraph_stays_open_with_its_text() {
        let (mut app, mut ed) = preview_app();
        ed.text = "Text preview".into();
        ed.page = 9;
        app.views[0].line_editor = Some(ed.clone());

        assert!(!app.commit_typing_in(app.views[0].id));
        assert_eq!(app.views[0].line_editor, Some(ed), "the paragraph keeps its unsaved text");
    }

    #[test]
    fn text_background_is_cached_without_editing_and_released_on_cancel() {
        let (app, ed) = preview_app();
        let doc = app.session.get(app.views[0].id).unwrap();
        let source = doc.bytes.clone();
        let generation = doc.edit_generation();
        let ctx = egui::Context::default();
        let mut background = TextBackground::default();
        for _ in 0..100 {
            background.prepare(&ctx, doc, Some(&ed), None, 1.0);
            if background.ready(0, 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(background.ready(0, 0));
        assert!(!background.pending());
        let texture = background.texture.as_ref().unwrap().id();
        background.prepare(&ctx, doc, Some(&ed), None, 1.0);
        assert_eq!(background.texture.as_ref().unwrap().id(), texture);
        assert_eq!(doc.bytes, source);
        assert_eq!(doc.edit_generation(), generation);
        assert!(!doc.dirty && doc.can_undo().is_none());
        background.prepare(&ctx, doc, None, None, 1.0);
        assert!(background.texture.is_none() && background.renderer.is_none());
    }

    #[test]
    fn the_editing_background_stays_sharp_on_a_zoomed_page() {
        // While a paragraph is open, the page around it is drawn from this one raster: it is
        // asked for at the tiling threshold, not the 2048-pixel backdrop side, which went soft
        // beyond about 1.2x on a Retina display.
        let (app, ed) = preview_app();
        let doc = app.session.get(app.views[0].id).unwrap();
        let page = &doc.info.pages[0];
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput { max_texture_side: Some(16384), ..Default::default() });
        let mut background = TextBackground::default();
        background.prepare(&ctx, doc, Some(&ed), None, 8.0);
        let expected = 8.0f32.min(crate::canvas::TILE_THRESHOLD / page.width.max(page.height));
        assert!(expected > crate::canvas::BASE_SIDE / page.width.max(page.height), "the test page needs the larger side");
        assert_eq!(background.tag, Some((expected * 1000.0) as u64));
        // A GPU that can't hold such a texture caps it.
        ctx.end_pass().textures_delta.clear();
        ctx.begin_pass(egui::RawInput { max_texture_side: Some(2048), ..Default::default() });
        let mut small = TextBackground::default();
        small.prepare(&ctx, doc, Some(&ed), None, 8.0);
        assert_eq!(small.tag, Some(((2048.0 / page.width.max(page.height)) * 1000.0) as u64));
        ctx.end_pass().textures_delta.clear();
    }

    #[test]
    fn pale_text_is_edited_on_a_dark_box() {
        // #913: white text from a coloured banner was typed onto a white box and vanished.
        for pale in [Color32::WHITE, Color32::from_rgb(0xFF, 0xFF, 0x00), Color32::from_gray(0xE0), Color32::from_rgb(0xFF, 0xA5, 0x00)] {
            assert_eq!(editor_fill(pale), DARK_EDITOR_FILL, "{pale:?}");
            assert!(contrast(pale, editor_fill(pale)) >= 4.5, "{pale:?} is readable");
        }
        // Ordinary dark or saturated text keeps the white box.
        for dark in [Color32::BLACK, Color32::from_rgb(0x1F, 0x2A, 0x6B), Color32::from_rgb(0xC0, 0x00, 0x00), Color32::from_gray(0x60)] {
            assert_eq!(editor_fill(dark), Color32::WHITE, "{dark:?}");
        }
    }

    #[test]
    fn caret_contrasts_with_its_fill() {
        assert_eq!(caret_color_on(Color32::WHITE), Color32::BLACK);
        assert_eq!(caret_color_on(Color32::from_rgb(0xFF, 0xFF, 0xF4)), Color32::BLACK);
        assert_eq!(caret_color_on(Color32::BLACK), Color32::WHITE);
        assert_eq!(caret_color_on(DARK_EDITOR_FILL), Color32::WHITE);
    }
}
