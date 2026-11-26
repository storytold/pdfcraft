//! Fill & Sign (Acrobat's Fill & Sign tool, execution plan M5.7): type text onto the page, place
//! ✓ ✕ ● ─ marks and today's date (in Preferences ▸ Date format), and sign with a drawn
//! signature. Everything is an annotation (typewriter text, PdfCraft-drawn stamps, ink), so it
//! can be moved, deleted and undone like any comment.

use base64::Engine as _;
use craft::behavior::selection::Selection;
use craft::behavior::{CancelReason, Cursor, CursorIcon, IconId, PressState, keyed};
use craft::keymap::CommandRef;
use craft::review::ui::signature::{SignatureSettings, paint_signature};
use craft::review::{SavedSignature, SignatureImage as SavedImage};
use craft::ui::canvas::{CanvasTool, ToolAction, ToolContext, ToolKey, ToolPath, ToolPointer};
use craft::ui::contract::{ActivateOn, Bind, CollectionEvent, Props as _, SelectMode, Show as _, ShowCollection as _, Sizing};
use craft::ui::list::ThumbAspect;
use craft::ui::primitives::Draw;
use craft::ui::style::DataColor;
use craft::ui::tile_grid::{Tile, TileActionPlace, TileGrid, TilePartEvent};
use egui::Pos2;
use pdfcraft_engine::{BorderShape, Edit, FillMark, NewAnnotation, Rgb, Shape, Style};
use pdfcraft_render::{DocInfo, PageInfo};

use crate::canvas::PageXform;
use crate::canvas::page_tools::PageToolCx;

pub(crate) mod scan;
pub mod type_box;

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

/// A saved signature as the graphic of a visible digital signature. `None` for typed text the
/// signature face has nothing to draw for, or an image the engine refuses.
pub fn graphic(sig: &SavedSignature) -> Option<pdfcraft_engine::sign::Graphic> {
    match sig {
        SavedSignature::Drawn(strokes) => Some(pdfcraft_engine::sign::Graphic::Strokes(
            strokes.iter().map(|s| s.iter().map(|p| [f64::from(p[0]), f64::from(p[1])]).collect()).collect(),
        )),
        SavedSignature::Typed(text) => pdfcraft_engine::typed_signature_graphic(text),
        SavedSignature::Image(image) => engine_image(image).map(|i| i.graphic()),
    }
}

/// The engine's copy of a saved image, `None` when its size or buffer is refused.
fn engine_image(image: &SavedImage) -> Option<pdfcraft_engine::SignatureImage> {
    pdfcraft_engine::SignatureImage::from_rgba(image.width(), image.height(), image.rgba().to_vec()).ok()
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
    pub size: f64,
    pub color: [f64; 3],
    /// The border around the text.
    pub border: TextBorder,
    pub focus: bool,
}

/// The border around typed text: its width in points (0: none), its own colour (`None`: the
/// text's) and its shape (an oval or a circle goes around the text).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextBorder {
    pub width: f64,
    pub color: Option<Rgb>,
    pub shape: BorderShape,
}

impl TextBorder {
    pub const NONE: Self = Self { width: 0.0, color: None, shape: BorderShape::Rectangle };
}

/// The default size Fill & Sign uses for typed text.
pub const TEXT_SIZE: f64 = 10.0;
pub const MIN_TEXT_SIZE: f64 = 4.0;
pub const MAX_TEXT_SIZE: f64 = 300.0;

pub fn valid_text_size(size: f64) -> Option<f64> {
    (size.is_finite() && (MIN_TEXT_SIZE..=MAX_TEXT_SIZE).contains(&size)).then_some(size)
}

pub fn valid_text_color(color: [f64; 3]) -> Option<[f64; 3]> {
    color.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)).then_some(color)
}

/// The borders the toolbar offers around typed text, in points (0: none).
pub const TEXT_BORDERS: [f64; 6] = [0.0, 0.5, 1.0, 1.5, 2.0, 3.0];
pub const MAX_TEXT_BORDER: f64 = 12.0;

pub fn valid_text_border(width: f64) -> Option<f64> {
    (width.is_finite() && (0.0..=MAX_TEXT_BORDER).contains(&width)).then_some(width)
}

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

fn new(page: usize, shape: Shape, contents: String, author: &str) -> Edit {
    let style = Style::default_for(&shape);
    Edit::AddAnnotation(NewAnnotation { page, shape, style, contents, author: author.to_string() })
}

/// A typewriter annotation sized to its text, its displayed top-left at `at` on a page turned by
/// `rotation` (its `/Rotate`), in black and without a border.
pub fn typed(page: usize, at: [f64; 2], text: &str, size: f64, rotation: i64, author: &str) -> Edit {
    typed_with(page, at, text, size, rotation, author, TypedLook::default())
}

/// How typed text and dates look: their colour, and the border around them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TypedLook {
    pub color: [f64; 3],
    pub border: TextBorder,
}

/// [`typed`] in `look`: the colour applies to the annotation's appearance and survives saving; a
/// border goes around the box sized to the text, so the text sits where it would without one.
pub fn typed_with(page: usize, at: [f64; 2], text: &str, size: f64, rotation: i64, author: &str, look: TypedLook) -> Edit {
    let width = valid_text_border(look.border.width).unwrap_or(0.0);
    // A circle goes around the text, so its box is no wider than the text needs.
    let [x0, y0, x1, y1] = if width > 0.0 && look.border.shape == BorderShape::Circle {
        crate::comments::snug_text_box_rect(at, text, size, rotation)
    } else {
        crate::comments::text_box_rect(at, text, size, rotation)
    };
    let shape = Shape::Typewriter { rect: [x0 - width, y0 - width, x1 + width, y1 + width], font_size: size };
    let style = Style {
        color: valid_text_color(look.color).unwrap_or([0.0; 3]),
        width,
        border_color: look.border.color.filter(|_| width > 0.0),
        border_shape: if width > 0.0 { look.border.shape } else { BorderShape::Rectangle },
        ..Style::default_for(&shape)
    };
    Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: text.to_string(), author: author.to_string() })
}

/// Place a saved signature (strokes normalised to a 0–1 box, y up) with its left edge at `at`,
/// 150 pt wide. Left, centred and upright are as displayed on a page turned by `rotation` (its
/// `/Rotate`), so the strokes are turned back into user space.
pub fn signature_at(page: usize, at: [f64; 2], strokes: &[Vec<[f32; 2]>], rotation: i64, author: &str) -> Option<Edit> {
    let w = 150.0;
    let (min_y, max_y) = strokes.iter().flatten().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    if !min_y.is_finite() {
        return None;
    }
    let h = f64::from(max_y - min_y).max(0.05) * w;
    // Displayed right and up as user-space unit vectors (the identity on an unturned page).
    let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(rotation, [0.0; 4]);
    let strokes: Vec<Vec<[f64; 2]>> = strokes
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.iter()
                .map(|p| {
                    let (dx, dy) = (f64::from(p[0]) * w, f64::from(p[1] - min_y) * w - h / 2.0);
                    [at[0] + a * dx + c * dy, at[1] + b * dx + d * dy]
                })
                .collect()
        })
        .collect();
    (!strokes.is_empty()).then(|| new(page, Shape::Signature { strokes }, String::new(), author))
}

/// Place typed text in the script font with its left edge at `at`, `height` points tall, upright
/// as displayed on a page turned by `rotation`.
pub fn typed_signature_at(page: usize, at: [f64; 2], text: &str, height: f64, rotation: i64, author: &str) -> Option<Edit> {
    pdfcraft_engine::typed_signature_shape(at, text, height, rotation).map(|shape| new(page, shape, String::new(), author))
}

/// Place a saved signature or initials, upright as `info`'s page is displayed.
pub fn place(page: usize, info: &PageInfo, at: [f64; 2], sig: &SavedSignature, initials: bool, author: &str) -> Option<Edit> {
    let rotation = i64::from(info.rotation);
    match sig {
        SavedSignature::Drawn(strokes) => signature_at(page, at, strokes, rotation, author),
        SavedSignature::Image(image) => engine_image(image)?.edit(page, info, at, initials, author),
        SavedSignature::Typed(text) => {
            let [left, bottom, right, top] = pdfcraft_engine::script_outline(text).bounds();
            let height = if initials { 24.0_f64 } else { 32.0_f64 };
            // Keep long names within the same placement width as drawn signatures.
            let height = height.min(150.0 * (top - bottom).max(0.1) / (right - left).max(0.01));
            typed_signature_at(page, at, text, height, rotation, author)
        }
    }
}

/// The saved signature and initials slots as the app's older settings file stored them: strokes
/// under `signature`, text under `signature_text`, an image under `signature_image` and any of the
/// three under `initials`, each externally tagged (`{"Image": "<base64 png>"}`). Typed text wins
/// over strokes and an image over both; a slot that does not read is `None` and the other slot is
/// kept.
pub fn legacy_signatures(v: &serde_json::Value) -> SignatureSettings {
    let mut slots = SignatureSettings::default();
    if let Ok(strokes) = serde_json::from_value::<Vec<Vec<[f32; 2]>>>(v["signature"].clone())
        && strokes.iter().all(|s| s.iter().all(|p| p.iter().all(|x| x.is_finite())))
    {
        slots.signature = Some(SavedSignature::Drawn(strokes));
    }
    if let Some(text) = v["signature_text"].as_str().filter(|t| !t.trim().is_empty()) {
        slots.signature = Some(SavedSignature::Typed(text.to_string()));
    }
    if let Some(image @ SavedSignature::Image(_)) = legacy_saved(&v["signature_image"]) {
        slots.signature = Some(image);
    }
    slots.initials = legacy_saved(&v["initials"]);
    slots
}

/// One legacy slot: a single-key object `Drawn`, `Typed` or `Image`.
fn legacy_saved(v: &serde_json::Value) -> Option<SavedSignature> {
    let (kind, value) = v.as_object()?.iter().next()?;
    match kind.as_str() {
        "Drawn" => {
            let strokes = serde_json::from_value::<Vec<Vec<[f32; 2]>>>(value.clone()).ok()?;
            strokes.iter().all(|s| s.iter().all(|p| p.iter().all(|x| x.is_finite()))).then_some(SavedSignature::Drawn(strokes))
        }
        "Typed" => value.as_str().map(|t| SavedSignature::Typed(t.to_string())),
        "Image" => legacy_image(value).map(SavedSignature::Image),
        _ => None,
    }
}

/// An image stored as a bare base64 PNG string (the older settings form), within the 4 MiB cap.
pub fn legacy_image(v: &serde_json::Value) -> Option<SavedImage> {
    let encoded = v.as_str()?;
    if encoded.len() > pdfcraft_engine::signature_image::MAX_SIGNATURE_IMAGE_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    let image = pdfcraft_engine::SignatureImage::from_bytes(&bytes).ok()?;
    let [w, h] = image.size();
    Some(SavedImage::new(u32::try_from(w).ok()?, u32::try_from(h).ok()?, image.rgba().to_vec()))
}

/// What a Fill & Sign click asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum FillOutcome {
    Edit(Box<Edit>),
    /// Place once on `page`, then select the new signature or initials there for adjustment.
    Signature {
        edit: Box<Edit>,
        page: usize,
    },
    /// No signature yet: open the signature pad.
    CreateSignature,
    /// No initials yet.
    CreateInitials,
    /// Nothing placed, and why (today's date can't be written into the PDF yet).
    Refused(String),
    /// Escape with nothing left to close: the tool goes back to Select.
    Leave,
}

/// Values that shape how Fill & Sign handles a pass's input, with the two session fields its text
/// click writes (the open type box, and the group member a new box lets go of; the comments tool
/// it runs beneath clears the selection on a press no mark takes).
pub(crate) struct FillInput<'a> {
    pub tool: FillTool,
    pub text_size: f64,
    pub text_color: [f64; 3],
    pub text_border: TextBorder,
    pub signature: Option<&'a SavedSignature>,
    pub initials: Option<&'a SavedSignature>,
    pub author: &'a str,
    pub date_text: &'a Result<String, String>,
    pub fill_text: &'a mut Option<TypeBox>,
    pub member: &'a mut Option<(CellAt, CellAt)>,
}

/// The Fill & Sign tool over the placed pages: a click places what the tool makes and reports it
/// through the outcome slot; a drag places nothing.
pub(crate) struct FillSignTool<'a> {
    input: FillInput<'a>,
    info: &'a DocInfo,
    xforms: Vec<(usize, PageXform)>,
    outcome: &'a mut Option<FillOutcome>,
    /// This pass's press: its release is a click only when it never moved past the click distance, which the press
    /// itself records (the press and its release come in different passes).
    press: PressState,
}

impl<'a> FillSignTool<'a> {
    pub(crate) fn new(cx: &PageToolCx<'a>, input: FillInput<'a>, outcome: &'a mut Option<FillOutcome>, press: PressState) -> Self {
        let size = |page: usize| cx.sizes.get(page).copied().unwrap_or_default();
        let xforms = cx.pages.iter().map(|placed| (placed.page, PageXform::of(placed, cx.rotation, size(placed.page)))).collect();
        FillSignTool { input, info: cx.info, xforms, outcome, press }
    }

    /// The page under `pos` and its transform.
    fn page_at(&self, pos: Pos2) -> Option<(usize, PageXform)> {
        self.xforms.iter().find(|(_, xf)| xf.rect.contains(pos)).copied()
    }

    /// The saved image the tool places and, for a pointer at `pos` on a page, the page's transform and info and the
    /// image's user-space box there (`image_signature_box`, the box a click places it in).
    fn ghost(&self, pos: Pos2) -> Option<(&'a SavedImage, PageXform, &'a PageInfo, [f64; 4])> {
        let saved = match self.input.tool {
            FillTool::Signature => self.input.signature,
            FillTool::Initials => self.input.initials,
            _ => None,
        };
        let Some(SavedSignature::Image(image)) = saved else { return None };
        let (page, xf) = self.page_at(pos)?;
        let info: &'a DocInfo = self.info;
        let p = info.pages.get(page)?;
        let at = to_user(&xf, info, page, pos);
        let size = [image.width() as usize, image.height() as usize];
        let rect = pdfcraft_engine::signature_image::image_signature_box(size, p, at, self.input.tool == FillTool::Initials)?;
        Some((image, xf, p, rect))
    }

    /// What a click at `pointer` on `page` makes. Typed text starts above the click, as displayed.
    fn click(&mut self, page: usize, xf: PageXform, pointer: Pos2) -> Option<FillOutcome> {
        let info = self.info;
        let p = info.pages.get(page)?;
        let at = to_user(&xf, info, page, pointer);
        let (tool, text_size, text_color, text_border) = (self.input.tool, self.input.text_size, self.input.text_color, self.input.text_border);
        let (signature, initials, author, date_text) = (self.input.signature, self.input.initials, self.input.author, self.input.date_text);
        let rotation = i64::from(p.rotation);
        let top = pdfcraft_engine::displayed_offset(rotation, at, 0.0, text_size * 0.6);
        let look = TypedLook { color: text_color, border: text_border };
        match tool {
            FillTool::Text => {
                // Clicking away from an open box commits it: the page sees this click before the box
                // sees its lost focus, so replacing the box first would drop what was typed. A new box
                // deselects, as a comment tool's note or text box does: Escape then closes the box and
                // leaves the tool in two presses.
                *self.input.member = None;
                let open = self.input.fill_text.replace(TypeBox {
                    page,
                    at: top,
                    text: String::new(),
                    size: text_size,
                    color: text_color,
                    border: text_border,
                    focus: true,
                });
                open.filter(|tb| !tb.text.trim().is_empty()).map(|tb| {
                    FillOutcome::Edit(Box::new(typed_with(
                        tb.page,
                        tb.at,
                        &tb.text,
                        tb.size,
                        crate::comments::page_rotation(info, tb.page),
                        author,
                        TypedLook { color: tb.color, border: tb.border },
                    )))
                })
            }
            FillTool::Date => Some(match date_text {
                Ok(text) => FillOutcome::Edit(Box::new(typed_with(page, top, text, text_size, rotation, author, look))),
                Err(why) => FillOutcome::Refused(why.clone()),
            }),
            FillTool::Signature => match signature {
                Some(s) => place(page, p, at, s, false, author).map(|e| FillOutcome::Signature { edit: Box::new(e), page }),
                None => Some(FillOutcome::CreateSignature),
            },
            FillTool::Initials => match initials {
                Some(s) => place(page, p, at, s, true, author).map(|e| FillOutcome::Signature { edit: Box::new(e), page }),
                None => Some(FillOutcome::CreateInitials),
            },
            mark => {
                let mark = mark.mark()?;
                let rect = mark.rect_at(at, rotation);
                Some(FillOutcome::Edit(Box::new(new(page, Shape::Mark { rect, mark }, String::new(), author))))
            }
        }
    }
}

impl CanvasTool for FillSignTool<'_> {
    fn press(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) -> ToolAction {
        if self.page_at(pointer.pos).is_some() { ToolAction::Claimed } else { ToolAction::Ignored }
    }

    fn drag(&mut self, _pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {}

    /// A drag never places anything: only a click does.
    fn release(&mut self, pointer: &ToolPointer, _cx: &mut ToolContext<'_>) {
        if !self.press.phase.is_click() {
            return;
        }
        if let Some((page, xf)) = self.page_at(pointer.pos) {
            *self.outcome = self.click(page, xf, pointer.pos);
        }
    }

    fn cancel(&mut self, _reason: CancelReason, _cx: &mut ToolContext<'_>) {}

    /// Escape steps back one thing per press, as a comment tool's does: the open type box, then the tool, so the Text
    /// tool does not open a box at every click with no way out by the keyboard.
    fn key(&mut self, key: &ToolKey, _cx: &mut ToolContext<'_>) -> bool {
        if !matches!(key, ToolKey::Cancel) {
            return false;
        }
        if self.input.fill_text.take().is_none() {
            *self.outcome = Some(FillOutcome::Leave);
        }
        true
    }

    /// A saved image signature or initials follows the pointer at the size a click places it, upright as displayed
    /// at every page and view rotation.
    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        let Some((image, xf, p, rect)) = cx.pointer.and_then(|pointer| self.ghost(pointer.pos)) else { return };
        let size = [image.width() as usize, image.height() as usize];
        // An image whose bytes don't fill its size paints nothing.
        if image.rgba().len() != size[0] * size[1] * 4 {
            return;
        }
        let tex = draw.cached_texture(("fill-sign-ghost", image.key()), || egui::ColorImage::from_rgba_unmultiplied(size, image.rgba()));
        let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        draw.texture_quad(xf.user_image_corners(p, rect, i64::from(p.rotation)), tex, whole);
    }

    /// The image takes the pointer's place where it is shown.
    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        if cx.pointer.is_some_and(|pointer| self.ghost(pointer.pos).is_some()) {
            return Cursor::System(CursorIcon::None);
        }
        Cursor::System(match self.input.tool {
            FillTool::Text => CursorIcon::Text,
            _ => CursorIcon::Crosshair,
        })
    }

    fn command_form(&self) -> ToolPath {
        ToolPath::Command("the Fill & Sign commands")
    }
}

/// One saved slot of the signature cards: what it holds and the commands it runs.
struct Slot<'a> {
    saved: &'a SavedSignature,
    what: &'static str,
    use_id: &'static str,
    change_id: &'static str,
    remove_id: &'static str,
    on: bool,
}

/// Saved previews and add/remove controls, shared by the left panel and quick-tool picker. Returns
/// the command the user chose.
pub(crate) fn signature_entries(ui: &mut egui::Ui, app: &mut crate::PdfCraftApp) -> Option<&'static str> {
    let label = |template: &str, what: &str| crate::i18n::fmt(tl!(template), &[("what", tl!(what))]);
    let slots = [
        (app.signature.as_ref(), "signature", "sign.fill.signature", "sign.fill.signature.change", "sign.fill.signature.remove", FillTool::Signature),
        (app.initials.as_ref(), "initials", "sign.fill.initials", "sign.fill.initials.change", "sign.fill.initials.remove", FillTool::Initials),
    ];
    let mut command = None;
    let mut shown = Vec::new();
    for (saved, what, use_id, change_id, remove_id, tool) in slots {
        let Some(saved) = saved else {
            if craft::ui::buttons::Button::new(label("Add {what}", what)).icon(IconId::new("plus")).show(ui).clicked() {
                command = Some(use_id);
            }
            continue;
        };
        shown.push(Slot { saved, what, use_id, change_id, remove_id, on: app.quick_tool == crate::QuickTool::Fill(tool) });
    }
    if shown.is_empty() {
        return command;
    }
    let mut selection = shown.iter().position(|s| s.on).map_or_else(Selection::default, Selection::single);
    let keys: Vec<usize> = (0..shown.len()).collect();
    let source = keyed(&keys, |k: &usize| *k);
    let ctx_less_label = &label;
    // The grid places the actions of every tile; a tile never sets it. Two cards at most, in a panel body that
    // scrolls: the grid is as tall as its cards and owns no scroll.
    let out = TileGrid::new(tl!("Saved signatures"))
        .sizing(Sizing::Content)
        .natural_height(true)
        .select(SelectMode::Single)
        .activate(ActivateOn::Click)
        .action_place(TileActionPlace::Band)
        .selection(Bind::bound(&mut selection))
        .show_items(ui, &source, &mut |key, _| {
            let slot = &shown[*key];
            let saved = slot.saved;
            Tile::new(ctx_less_label("Use {what}", slot.what))
                .thumb(ThumbAspect::new(3, 1), move |cx, rect| {
                    cx.with_draw(|draw| paint_signature(cx.painter.ctx(), draw, rect, saved, DataColor::new(cx.ink())));
                })
                .action_named(IconId::new("pencil"), CommandRef::new(slot.change_id), ctx_less_label("Change {what}", slot.what))
                .action_named(IconId::new("close"), CommandRef::new(slot.remove_id), ctx_less_label("Remove saved {what}", slot.what))
        });
    for event in out.response.events {
        command = match event {
            CollectionEvent::Activate(key) => shown.get(key).map(|s| s.use_id),
            CollectionEvent::Item(key, TilePartEvent::Action(0)) => shown.get(key).map(|s| s.change_id),
            CollectionEvent::Item(key, TilePartEvent::Action(1)) => shown.get(key).map(|s| s.remove_id),
            _ => command,
        };
    }
    command
}

/// A pair of indices naming one place in a grid.
type CellAt = (usize, usize);
