//! Document inspection: everything panels need that is not pixels.
//!
//! Page geometry comes from hayro (which resolves inheritance and rotation); the rest comes from
//! the lazy COS reader through a read-only lopdf object adapter. The original lopdf loader
//! remains the compatibility fallback. When neither can inspect a renderable file, panels
//! are empty and `warnings` says why.

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro::hayro_syntax::DecryptionError;
use hayro::hayro_syntax::LoadPdfError;
use hayro::hayro_syntax::Pdf;
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId};
use pdfcraft_cos::page_labels::{
    MAX_LABEL_BYTES, MAX_LABEL_TOTAL_BYTES, MAX_LABEL_TREE_DEPTH, MAX_LABEL_TREE_WORK, MAX_PREFIX_BYTES, MAX_PREFIX_TOTAL_BYTES, alpha, roman,
};

use crate::OpenError;
use crate::structure::{LazyStructure, Structure};

/// lopdf decodes object and cross-reference streams while it loads, with no limit unless one is
/// set: a few hundred bytes of nested FlateDecode then inflate to gigabytes. Real object and
/// xref streams are far below this.
const LOAD_STREAM_LIMIT: usize = 256 << 20;

/// At most this many `/State` entries of a set-layer-visibility action are read.
const MAX_LAYER_STATE: usize = 1024;

/// Decoded bytes read from an XFA form's configuration packet (or a whole-XDP `/XFA` stream) to
/// find `dynamicRender`. Designer's packets are a few kilobytes; a whole XDP can hold the template.
const MAX_XFA_CONFIG_BYTES: usize = 16 << 20;

/// File specifications retained from a remote-Go-To action are deliberately small.
const MAX_REMOTE_FILE_SPEC_BYTES: usize = 4096;
const MAX_REMOTE_DESTINATION_NAME_BYTES: usize = 1024;

/// Text and geometry `DocInfo::annotations` keeps, as counted by [`retained`]. Past it the rest
/// are not listed: real comments stay far below, many small ones sharing one long string do not.
const MAX_ANNOTATION_BYTES: usize = 64 << 20;

/// Web-address text the link listing, and name, description and embedded-file key text the
/// attachment listing, keep together. Many links or attachments can share one long string, which
/// each would otherwise copy. Past it, a link to a web address lists as a URI action without its
/// address, an attachment under its generic name without a description, and an embedded file whose
/// key does not fit is not listed.
const MAX_LINK_ATTACHMENT_TEXT_BYTES: usize = 16 << 20;

/// The bytes a listed annotation keeps.
fn retained(a: &Annotation) -> usize {
    let text = [&a.author, &a.contents, &a.modified, &a.name, &a.in_reply_to, &a.state, &a.intent];
    text.into_iter()
        .map(|t| t.as_ref().map_or(0, String::len))
        .chain([
            a.subtype.len(),
            std::mem::size_of_val(a.quads.as_slice()),
            std::mem::size_of_val(a.points.as_slice()),
            std::mem::size_of::<Annotation>(),
        ])
        .fold(0, usize::saturating_add)
}

/// At most this many layers are read from all of `/RBGroups` together.
const MAX_LAYER_GROUP_ENTRIES: usize = 4096;

/// `DocInfo::outline` holds the first this many bookmarks in outline order. Opening a document
/// reads only this window; the rest are listed by page through `pdfcraft_organize`.
const OUTLINE_WINDOW: usize = 10_000;

/// Deepest level `DocInfo::outline` nests (the top level is 0); deeper bookmarks are left out.
const OUTLINE_DEPTH: usize = 32;

fn load_options(password: Option<&str>) -> LoadOptions {
    LoadOptions { password: password.map(str::to_owned), max_decompressed_size: Some(LOAD_STREAM_LIMIT), ..LoadOptions::default() }
}

#[derive(Clone, Debug, Default)]
pub struct DocInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    /// Custom document properties: `/Info` entries with a key the spec does not name and a text
    /// value, as (name, text), in the dictionary's order (at most 256).
    pub custom: Vec<(String, String)>,
    pub pdf_version: String,
    pub file_size: usize,
    pub encrypted: bool,
    pub tagged: bool,
    pub has_javascript: bool,
    /// The form is (also) an XFA form, which isn't read yet (`form.xfa-*` in the parity list).
    pub xfa: Option<Xfa>,
    pub pages: Vec<PageInfo>,
    pub outline: Vec<OutlineItem>,
    /// The outline has bookmarks that `outline` leaves out (see `OUTLINE_WINDOW`, `OUTLINE_DEPTH`).
    pub outline_more: bool,
    pub annotations: Vec<Annotation>,
    pub fields: Vec<Field>,
    pub links: Vec<Link>,
    pub layers: Vec<Layer>,
    /// Radio-button layer groups (`/OCProperties /D /RBGroups`): turning one layer of a group on
    /// turns the others off.
    pub layer_groups: Vec<Vec<(u32, u16)>>,
    pub fonts: Vec<FontInfo>,
    pub attachments: Vec<Attachment>,
    /// Pages with a presentation transition (`/Trans`) or display time (`/Dur`), in page order.
    pub transitions: Vec<PageTransition>,
    /// Where the document opens (`/OpenAction`): the (0-based page, view) of its destination,
    /// named destinations resolved. `None` without one, for an action that goes nowhere in this
    /// document (a script, say), or for one that doesn't resolve to a page of it.
    pub open_dest: Option<(usize, DestView)>,
    pub warnings: Vec<String>,
}

/// What kind of XFA form a document has (`/AcroForm /XFA`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Xfa {
    /// The pages and fields are ordinary PDF; the XFA packets describe the same form (and hold
    /// its data, which Acrobat shows in preference to the fields).
    Static,
    /// The form is laid out from the XFA packets at open (`/NeedsRendering`, or no fields and a
    /// configuration that doesn't forbid dynamic rendering): the PDF pages are only a placeholder.
    Dynamic,
}

/// A page's presentation transition and display time (ISO 32000-2 §12.4.4): how the page comes
/// into view in a full-screen presentation, and how long it stays before advancing. Only pages
/// carrying `/Trans` or `/Dur` are listed. Values outside the spec's ranges fall back to the
/// defaults rather than being refused.
#[derive(Clone, Debug, PartialEq)]
pub struct PageTransition {
    pub page: usize,
    /// `/Trans /S`; `None` when the page has only `/Dur`.
    pub style: Option<TransitionStyle>,
    /// `/D`: how long the transition takes, in seconds (default 1).
    pub duration: f32,
    /// `/Dm /H` (true) or `/V` (Split, Blinds).
    pub horizontal: bool,
    /// `/M /I` (true) or `/O` (Split, Box, Fly).
    pub inward: bool,
    /// `/Di`: 0, 90, 180, 270 or 315 degrees (Wipe, Glitter, Fly, Cover, Uncover, Push); `None`
    /// for `/None` (Fly only).
    pub direction: Option<u16>,
    /// `/SS`: the Fly scale (default 1).
    pub scale: f32,
    /// `/B`: Fly with an opaque backdrop.
    pub opaque: bool,
    /// `/Dur`: seconds the page shows before the presentation advances; `None` waits for the
    /// presenter.
    pub display: Option<f32>,
}

/// The transition styles of ISO 32000-2 table 164 (`/Trans /S`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionStyle {
    Split,
    Blinds,
    Box,
    Wipe,
    Dissolve,
    Glitter,
    /// `/R`: the next page replaces the old one with no effect (the default).
    Replace,
    Fly,
    Push,
    Cover,
    Uncover,
    Fade,
}

impl TransitionStyle {
    /// The style a `/S` name means; an unknown name is `Replace`, the spec's default.
    pub fn from_name(name: &str) -> Self {
        match name {
            "Split" => Self::Split,
            "Blinds" => Self::Blinds,
            "Box" => Self::Box,
            "Wipe" => Self::Wipe,
            "Dissolve" => Self::Dissolve,
            "Glitter" => Self::Glitter,
            "Fly" => Self::Fly,
            "Push" => Self::Push,
            "Cover" => Self::Cover,
            "Uncover" => Self::Uncover,
            "Fade" => Self::Fade,
            _ => Self::Replace,
        }
    }

    /// The name as written in a PDF (`/S`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Split => "Split",
            Self::Blinds => "Blinds",
            Self::Box => "Box",
            Self::Wipe => "Wipe",
            Self::Dissolve => "Dissolve",
            Self::Glitter => "Glitter",
            Self::Replace => "R",
            Self::Fly => "Fly",
            Self::Push => "Push",
            Self::Cover => "Cover",
            Self::Uncover => "Uncover",
            Self::Fade => "Fade",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PageInfo {
    /// Displayed size in points, after `/Rotate` and `/UserUnit`.
    pub width: f32,
    pub height: f32,
    /// Page label (`/PageLabels`), falling back to the 1-based page number.
    pub label: String,
    /// Effective crop box in user space [x0, y0, x1, y1] (the visible region).
    pub crop: [f32; 4],
    /// Clockwise page rotation in degrees (0, 90, 180, 270).
    pub rotation: u16,
}

impl PageInfo {
    /// A point in view space (points, y down, after `/Rotate`; what the text layer uses) →
    /// PDF user space.
    pub fn view_to_user(&self, x: f32, y: f32) -> [f32; 2] {
        let [cx0, cy0, cx1, cy1] = self.crop;
        let (a, b) = (x / self.width.max(1e-3), y / self.height.max(1e-3));
        let (u, v) = match self.rotation {
            90 => (b, 1.0 - a),
            180 => (1.0 - a, 1.0 - b),
            270 => (1.0 - b, a),
            _ => (a, b),
        };
        [cx0 + u * (cx1 - cx0), cy1 - v * (cy1 - cy0)]
    }

    /// PDF user space → view space (the inverse of [`Self::view_to_user`]).
    pub fn user_to_view(&self, x: f32, y: f32) -> [f32; 2] {
        let [cx0, cy0, cx1, cy1] = self.crop;
        let (u, v) = ((x - cx0) / (cx1 - cx0).max(1e-3), (cy1 - y) / (cy1 - cy0).max(1e-3));
        let (a, b) = match self.rotation {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        [a * self.width, b * self.height]
    }

    /// A point as displayed (points from the page's top-left, y down, before any view rotation)
    /// → the `/XYZ` position `[left, top]` in user space that puts it at the window's top-left.
    /// Only a coordinate that is given is pinned: a bookmark made at the top of the window
    /// leaves the horizontal position alone. On a page turned a quarter, the displayed `y` decides
    /// the user `left` and the displayed `x` the user `top` (the inverse of [`Self::dest_fraction`]).
    /// A value that is not finite counts as not given.
    pub fn xyz_for_view(&self, x: Option<f32>, y: Option<f32>) -> [Option<f32>; 2] {
        let (x, y) = (x.filter(|v| v.is_finite()), y.filter(|v| v.is_finite()));
        let [ux, uy] = self.view_to_user(x.unwrap_or(0.0), y.unwrap_or(0.0));
        if self.rotation.is_multiple_of(180) { [x.map(|_| ux), y.map(|_| uy)] } else { [y.map(|_| ux), x.map(|_| uy)] }
    }

    /// A destination's user-space point (each coordinate possibly unspecified) → the same point
    /// as a fraction of the displayed page `[x, y]` (0..1 from its top-left corner, y down),
    /// after `/Rotate` and a further clockwise `view_rotation` (0, 90, 180 or 270). On a page
    /// turned a quarter, the user `x` decides the displayed `y` and vice versa, so each output
    /// is `None` when the input it comes from is (or is not finite). Points off the page are
    /// clamped to its edge.
    pub fn dest_fraction(&self, x: Option<f32>, y: Option<f32>, view_rotation: u16) -> [Option<f32>; 2] {
        let [cx0, cy0, cx1, cy1] = self.crop;
        let frac = |v: f32| if v.is_finite() { Some(v.clamp(0.0, 1.0)) } else { None };
        // Unrotated fractions across (u, from the crop's left) and down (v, from its top).
        let u = x.and_then(|x| frac((x - cx0) / (cx1 - cx0).max(1e-3)));
        let v = y.and_then(|y| frac((cy1 - y) / (cy1 - cy0).max(1e-3)));
        let flip = |f: Option<f32>| f.map(|f| 1.0 - f);
        match (u32::from(self.rotation) + u32::from(view_rotation)) % 360 {
            90 => [flip(v), u],
            180 => [flip(u), flip(v)],
            270 => [v, flip(u)],
            _ => [u, v],
        }
    }

    /// A view-space rectangle → the normalized user-space rectangle covering the same area of the
    /// page (`[x0, y0, x1, y1]`, `x0 <= x1`, `y0 <= y1`). Under `/Rotate` the corners swap roles,
    /// so a view rectangle's top-left is not in general the user rectangle's `[x0, y1]`.
    pub fn view_rect_to_user(&self, r: [f32; 4]) -> [f32; 4] {
        let (a, b) = (self.view_to_user(r[0], r[1]), self.view_to_user(r[2], r[3]));
        [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
    }

    /// A view-space rectangle as a text-markup quad in user space: top-left, top-right,
    /// bottom-left, bottom-right as read on screen.
    pub fn view_rect_to_quad(&self, r: [f32; 4]) -> [f64; 8] {
        let mut q = [0.0; 8];
        for (i, (x, y)) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])].into_iter().enumerate() {
            let [ux, uy] = self.view_to_user(x, y);
            q[2 * i] = ux as f64;
            q[2 * i + 1] = uy as f64;
        }
        q
    }
}

/// A clickable link annotation.
#[derive(Clone, Debug)]
pub struct Link {
    pub page: usize,
    /// Zero-based position in the page's raw `/Annots` array.
    pub index: usize,
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    /// A destination in this document: the 0-based page and where on it the view goes.
    Page(usize, DestView),
    Uri(String),
    /// Source of a JavaScript action, decoded with a bounded stream reader.
    JavaScript(String),
    /// A set-layer-visibility action (`SetOCGState`, ISO 32000-2 §12.6.4.13): each change in
    /// order, naming the layer by its optional content group. With `preserve_rb`, a layer turned
    /// on turns off the other layers of its radio-button groups.
    SetLayers {
        changes: Vec<(LayerOp, (u32, u16))>,
        preserve_rb: bool,
    },
    /// A local-file remote Go-To action. The caller resolves `file` relative to the source path.
    /// `launch` marks a Launch action that names a PDF file (see `launch_pdf`), opened the same
    /// way: as a document in PdfCraft, never by the operating system.
    RemoteGoTo {
        launch: bool,
        file: String,
        destination: RemoteDestination,
        new_window: Option<bool>,
    },
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RemoteDestination {
    Default,
    Page { page: usize, view: DestView },
    Named(String),
}

/// What a set-layer-visibility action does to a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerOp {
    On,
    Off,
    Toggle,
}

/// Where an explicit destination (ISO 32000-2 §12.3.2.2, Table 149) places its page in the
/// window. Coordinates are PDF user space; `None` is a `null` (or unusable) operand, which keeps
/// that part of the view as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DestView {
    /// The top of the page at the current zoom: a destination with no position, or one whose
    /// type is missing or unknown.
    #[default]
    Top,
    /// `/XYZ left top zoom`: the point (left, top) at the window's upper-left corner, at `zoom`
    /// (1.0 = 100%; 0 or null keeps the current zoom).
    Xyz { left: Option<f32>, top: Option<f32>, zoom: Option<f32> },
    /// `/Fit`, and `/FitB` (fit the page's content box; read as the whole page).
    Fit,
    /// `/FitH top`, and `/FitBH`: fit the width, with `top` at the window's top edge.
    FitH { top: Option<f32> },
    /// `/FitV left`, and `/FitBV`: fit the height, with `left` at the window's left edge.
    FitV { left: Option<f32> },
    /// `/FitR left bottom right top`: fit the rectangle `[x0, y0, x1, y1]` (normalized).
    FitR { rect: [f32; 4] },
}

impl DestView {
    /// The view an explicit destination array asks for, from its elements after the page
    /// (`/XYZ left top zoom`, `/FitH top`, …). Hostile input never fails: a missing or unknown
    /// type, or a `/FitR` without four usable numbers, is [`DestView::Top`]; a non-numeric,
    /// non-finite or missing operand is `None`; a zoom that is not positive is `None`.
    pub fn from_operands(kind: Option<&[u8]>, operands: &[Option<f32>]) -> Self {
        let n = |i: usize| operands.get(i).copied().flatten().filter(|v| v.is_finite());
        match kind {
            Some(b"XYZ") => Self::Xyz { left: n(0), top: n(1), zoom: n(2).filter(|z| *z > 0.0) },
            Some(b"Fit" | b"FitB") => Self::Fit,
            Some(b"FitH" | b"FitBH") => Self::FitH { top: n(0) },
            Some(b"FitV" | b"FitBV") => Self::FitV { left: n(0) },
            Some(b"FitR") => match (n(0), n(1), n(2), n(3)) {
                (Some(x0), Some(y0), Some(x1), Some(y1)) => Self::FitR { rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)] },
                _ => Self::Top,
            },
            _ => Self::Top,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<usize>,
    /// Where on `page` the bookmark goes ([`DestView::Top`] for a bookmark without a page).
    pub view: DestView,
    pub children: Vec<OutlineItem>,
    /// Whether the bookmark starts open: only a positive `/Count` says so (ISO 32000-2 §12.3.3,
    /// Table 151). A negative one is closed, and so is a missing one, which a bookmark with
    /// children should not have.
    pub open: bool,
    /// The title's colour (`/C`, DeviceRGB, ISO 32000-2 §12.3.3 Table 151), or `None` for the
    /// default black or a value that isn't three numbers.
    pub color: Option<[f32; 3]>,
    /// The title's style (`/F`, Table 152): bit 1 italic, bit 2 bold.
    pub italic: bool,
    pub bold: bool,
}

#[derive(Clone, Debug)]
pub struct Annotation {
    pub page: usize,
    pub subtype: String,
    pub author: Option<String>,
    pub contents: Option<String>,
    pub modified: Option<String>,
    /// `/NM` of this annotation, and of the one it replies to (`/IRT`), for threading.
    pub name: Option<String>,
    pub in_reply_to: Option<String>,
    /// Rect in PDF user space: [x0, y0, x1, y1].
    pub rect: [f32; 4],
    pub color: Option<[f32; 3]>,
    /// Position in the page's `/Annots` (how edits address the comment).
    pub index: usize,
    /// For a status reply (`/State`, Acrobat's "Set status"): the state it sets.
    pub state: Option<String>,
    /// Text markup quadrilaterals (`/QuadPoints`), 8 numbers each.
    pub quads: Vec<[f32; 8]>,
    /// The Locked flag (`/F` bit 8): the comment can't be moved, resized, restyled or deleted.
    pub locked: bool,
    /// `/IT`, the intent: `FreeTextCallout`, `PolygonCloud`, `FreeTextTypeWriter`…
    pub intent: Option<String>,
    /// A Fill & Sign mark: typed text, a check, cross, dot or line, or a typed, drawn or picture
    /// signature or initials (the rules of `pdfcraft_annot::is_fill_sign`).
    pub fill_sign: bool,
    /// The NoRotate flag (`/F` bit 5), set on sticky notes and attachments: on a turned page the
    /// appearance stays upright, turned about the upper-left corner of `rect`
    /// ([`Self::shown_rect`]).
    pub no_rotate: bool,
    /// The points that shape a line (its two ends, `/L`) or connected lines, a polygon or a
    /// cloud (their corners, `/Vertices`), in user space; empty for other comments.
    pub points: Vec<[f32; 2]>,
}

impl Annotation {
    /// A checkmark reply (`/StateModel /Marked`): "Marked" or "Unmarked" rather than a review status.
    pub fn is_mark(&self) -> bool {
        matches!(self.state.as_deref(), Some("Marked" | "Unmarked"))
    }

    /// Where the comment is drawn on a page turned by `rotation` (`PageInfo::rotation`), in user
    /// space: `rect`, or for a NoRotate comment `rect` turned upright about its upper-left corner
    /// (see [`Self::no_rotate`]). Hit-testing, selection and anything placed beside the comment
    /// use this, so they match the drawing.
    pub fn shown_rect(&self, rotation: u16) -> [f32; 4] {
        self.shown(self.rect.map(f64::from), rotation).map(|v| v as f32)
    }

    /// [`Self::shown_rect`] for this comment with its `/Rect` at `rect` (one being moved or
    /// resized).
    pub fn shown(&self, rect: [f64; 4], rotation: u16) -> [f64; 4] {
        if self.no_rotate { upright_rect(rect, rotation) } else { rect }
    }

    /// The `/Rect` that shows this comment in the user-space box `shown`: the inverse of
    /// [`Self::shown`], for a box resized as drawn.
    pub fn rect_for_shown(&self, shown: [f64; 4], rotation: u16) -> [f64; 4] {
        if self.no_rotate { rect_from_upright(shown, rotation) } else { shown }
    }
}

/// The user-space box a NoRotate annotation with `/Rect` `rect` is drawn in on a page turned
/// clockwise by `rotation` (ISO 32000-2 §12.5.3): `rect` turned as far counterclockwise about its
/// upper-left corner `(x0, y1)`, which stays where it is on the page. As displayed, the box's
/// top-left is that corner, the rectangle's width runs across and its height down, as on an
/// unturned page. `rect` itself for 0 or anything that isn't a quarter turn. The vendored
/// renderer draws the appearance there (hayro-interpret patch 37).
fn upright_rect(rect: [f64; 4], rotation: u16) -> [f64; 4] {
    let [x0, y0, x1, y1] = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    let (w, h) = (x1 - x0, y1 - y0);
    match rotation {
        90 => [x0, y1, x0 + h, y1 + w],
        180 => [x0 - w, y1, x0, y1 + h],
        270 => [x0 - h, y1 - w, x0, y1],
        _ => [x0, y0, x1, y1],
    }
}

/// The `/Rect` whose [`upright_rect`] is `shown`: `shown` turned back about the corner that
/// stays put.
fn rect_from_upright(shown: [f64; 4], rotation: u16) -> [f64; 4] {
    let [s0, s1, s2, s3] = [shown[0].min(shown[2]), shown[1].min(shown[3]), shown[0].max(shown[2]), shown[1].max(shown[3])];
    let (sw, sh) = (s2 - s0, s3 - s1);
    // The upper-left corner of `/Rect` and the rectangle's width and height.
    let ((x0, y1), w, h) = match rotation {
        90 => ((s0, s1), sh, sw),
        180 => ((s2, s1), sw, sh),
        270 => ((s2, s3), sh, sw),
        _ => return [s0, s1, s2, s3],
    };
    [x0, y1 - h, x0 + w, y1]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    CheckBox,
    Radio,
    PushButton,
    Combo,
    List,
    Signature,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    pub value: Option<String>,
    pub page: Option<usize>,
    pub tooltip: Option<String>,
    pub has_actions: bool,
    /// Rect of the first widget, in user space.
    pub rect: Option<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct Layer {
    /// Object number and generation of the optional content group.
    pub id: (u32, u16),
    pub name: String,
    pub visible: bool,
    /// Shown as the file opens: the default configuration (`/OCProperties /D`).
    pub initial: bool,
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub description: Option<String>,
    pub size: Option<usize>,
    pub source: AttachmentSource,
}

/// Where an attachment lives (so its bytes can be fetched on demand).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentSource {
    /// Document-level: `/Names /EmbeddedFiles` entry with this key.
    Document { key: String },
    /// A FileAttachment annotation: page index and position in the page's `/Annots`.
    Annotation { page: usize, index: usize },
}

/// A font used by the document (Document Properties ▸ Fonts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    /// BaseFont without the subset tag.
    pub name: String,
    /// Type1, TrueType, Type0, Type3, MMType1, CIDFontType0/2 …
    pub kind: String,
    pub embedded: bool,
    pub subset: bool,
    pub encoding: Option<String>,
    /// The face standing in for a program the document does not embed. `None` when the font is
    /// embedded (nothing stands in) or when no face was available at all -- `substitution` then
    /// says which of the two it is.
    pub substitute: Option<String>,
    /// Why a face was substituted, as a sentence for whoever has to act on it. `None` only when
    /// the document's own glyphs are being shown.
    pub substitution: Option<String>,
}

/// Check that `bytes` still open as the same document: the parser reads them and the page
/// count matches. Content-only edits skip the full [`inspect`] (seconds on very large files)
/// but must still catch an edit that produced something the viewer cannot display, so the
/// caller can roll it back. Never panics: parser crashes are caught and reported.
pub fn check_pages(bytes: Arc<Vec<u8>>, password: Option<&str>, expected: usize) -> Result<(), OpenError> {
    let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes, password.unwrap_or(""))))
        .map_err(|p| OpenError::Invalid(format!("the parser crashed: {}", crate::raster::panic_message(&p))))?
        .map_err(|e| match e {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                if password.is_some() {
                    OpenError::WrongPassword
                } else {
                    OpenError::NeedsPassword
                }
            }
            LoadPdfError::Decryption(other) => OpenError::Unsupported(format!("encryption not supported: {other:?}")),
            LoadPdfError::Invalid => OpenError::Invalid("no readable page tree or cross-reference data was found".into()),
        })?;
    let pages = catch_unwind(AssertUnwindSafe(|| pdf.pages().len()))
        .map_err(|p| OpenError::Invalid(format!("the page tree could not be read: {}", crate::raster::panic_message(&p))))?;
    if pages != expected {
        return Err(OpenError::Invalid(format!("the edit changed the page count from {expected} to {pages}")));
    }
    Ok(())
}

/// Inspect a document. Fails only if the renderer itself cannot open the file (or needs a
/// password). Never panics: parser crashes are caught and reported.
pub fn inspect(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<DocInfo, OpenError> {
    let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes.clone(), password.unwrap_or(""))))
        .map_err(|p| OpenError::Invalid(format!("the parser crashed: {}", crate::raster::panic_message(&p))))?
        .map_err(|e| match e {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                if password.is_some() {
                    OpenError::WrongPassword
                } else {
                    OpenError::NeedsPassword
                }
            }
            LoadPdfError::Decryption(other) => OpenError::Unsupported(format!("encryption not supported: {other:?}")),
            LoadPdfError::Invalid => OpenError::Invalid("no readable page tree or cross-reference data was found".into()),
        })?;
    let mut info = DocInfo { file_size: bytes.len(), pdf_version: version_label(&format!("{:?}", pdf.version())), ..Default::default() };
    let pages = catch_unwind(AssertUnwindSafe(|| {
        let mut out = Vec::new();
        for (i, page) in pdf.pages().iter().enumerate() {
            let (w, h) = page.render_dimensions();
            let c = page.intersected_crop_box();
            let rotation = match page.rotation() {
                hayro::hayro_syntax::page::Rotation::None => 0,
                hayro::hayro_syntax::page::Rotation::Horizontal => 90,
                hayro::hayro_syntax::page::Rotation::Flipped => 180,
                hayro::hayro_syntax::page::Rotation::FlippedHorizontal => 270,
            };
            let crop = [c.x0 as f32, c.y0 as f32, c.x1 as f32, c.y1 as f32];
            // Degenerate boxes still get a usable placeholder size so layout never divides by zero.
            let (w, h) = if w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0 { (w, h) } else { (612.0, 792.0) };
            out.push(PageInfo { width: w, height: h, label: (i + 1).to_string(), crop, rotation });
        }
        out
    }));
    match pages {
        Ok(p) => info.pages = p,
        Err(p) => return Err(OpenError::Invalid(format!("the page tree could not be read: {}", crate::raster::panic_message(&p)))),
    }
    let options = load_options(password);
    // Geometry no longer needs the renderer parser. Do not overlap its allocations with
    // structural inspection, including the compatibility fallback for damaged files.
    drop(pdf);
    inspect_structure(&mut info, |tmp| {
        // The lazy reader first; lopdf's repair/tolerance path when it can't expose the same
        // page tree or an object a panel needs. No partial result is published.
        let lazy = catch_unwind(AssertUnwindSafe(|| {
            let doc = LazyStructure::new(bytes.clone(), password).ok()?;
            let mut t = DocInfo { pages: tmp.pages.clone(), ..Default::default() };
            let inspector = Inspector::new(&doc);
            if inspector.page_index.len() != t.pages.len() {
                return None;
            }
            inspector.fill(&mut t);
            (!doc.failed()).then_some(t)
        }))
        .ok()
        .flatten();
        if let Some(t) = lazy {
            *tmp = t;
            return Ok(());
        }
        let doc = Document::load_mem_with_options(&bytes, options).map_err(|e| e.to_string())?;
        Inspector::new(&doc).fill(tmp);
        Ok(())
    });
    if info.pages.is_empty() {
        return Err(OpenError::Invalid("the document has no pages".into()));
    }
    Ok(info)
}

// Commit auxiliary metadata only after inspection succeeds; renderer geometry is the fallback.
fn inspect_structure(info: &mut DocInfo, fill: impl FnOnce(&mut DocInfo) -> Result<(), String>) {
    let structure = catch_unwind(AssertUnwindSafe(|| {
        let mut tmp = DocInfo { pages: info.pages.clone(), ..Default::default() };
        fill(&mut tmp)?;
        Ok::<_, String>(tmp)
    }));
    match structure {
        Ok(Ok(mut filled)) => {
            filled.file_size = info.file_size;
            filled.pdf_version = std::mem::take(&mut info.pdf_version);
            *info = filled;
        }
        Ok(Err(e)) => info.warnings.push(format!("Some document structure (bookmarks, comments, fields) could not be read: {e}")),
        Err(p) => {
            info.warnings.push(format!("Document structure inspection crashed and was skipped: {}", crate::raster::panic_message(&p)));
        }
    }
}

/// Negative cache of failed script-stream decompressions during one structure inspection.
/// Callers must supply stream addresses that stay stable for that inspection; these are
/// opaque identities, never dereferenced here. Do not reuse the cache across documents,
/// edits or inspections, where an address may identify a different stream.
/// FIFO eviction bounds it to 4096 identities and allows an evicted failure to be retried;
/// a cache hit does not refresh its position. This bounds retained failures, not the
/// total decoding work, and does not cache successful sources or spend their text budget.
#[derive(Default)]
struct RejectedJavaScript {
    identities: std::collections::HashSet<*const lopdf::Stream>,
    order: std::collections::VecDeque<*const lopdf::Stream>,
}

impl RejectedJavaScript {
    const CAPACITY: usize = 4096;

    fn contains(&self, identity: *const lopdf::Stream) -> bool {
        self.identities.contains(&identity)
    }

    fn insert(&mut self, identity: *const lopdf::Stream) {
        if self.identities.contains(&identity) {
            return;
        }
        if self.order.len() >= Self::CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.identities.remove(&oldest);
        }
        self.identities.insert(identity);
        self.order.push_back(identity);
    }
}

struct Inspector<'a> {
    doc: &'a dyn Structure,
    page_ids: Vec<ObjectId>,
    page_index: HashMap<ObjectId, usize>,
    /// Named destinations (`/Names /Dests` tree), keyed by raw string bytes, built once.
    /// Looking each name up by walking the tree was O(links × names): minutes on manuals.
    named: HashMap<Vec<u8>, &'a Object>,
    /// The page of each annotation (the first page that lists it), built when a field's widget
    /// has no `/P`. Searching every page's `/Annots` for each such field was O(fields ×
    /// annotations): a second on a form of 8,000 fields.
    annotation_pages: OnceCell<HashMap<ObjectId, usize>>,
}

impl<'a> Inspector<'a> {
    fn new(doc: &'a dyn Structure) -> Self {
        let page_ids = doc.page_ids();
        let page_index = page_ids.iter().copied().enumerate().map(|(index, id)| (id, index)).collect();
        let mut me = Self { doc, page_ids, page_index, named: HashMap::new(), annotation_pages: OnceCell::new() };
        let mut named = HashMap::new();
        if let Some(tree) =
            doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| me.dict(o)).and_then(|n| n.get(b"Dests").ok()).and_then(|o| me.dict(o))
        {
            me.name_tree_raw(tree, &mut HashSet::new(), 0, &mut named);
        }
        me.named = named;
        me
    }

    fn fill(&self, info: &mut DocInfo) {
        // lopdf drops /Encrypt from the trailer once it has decrypted the file.
        info.encrypted = self.doc.encrypted();
        if let Some(d) = self.doc.trailer().get(b"Info").ok().and_then(|o| self.dict(o)) {
            info.title = self.text(d, b"Title");
            info.author = self.text(d, b"Author");
            info.subject = self.text(d, b"Subject");
            info.keywords = self.text(d, b"Keywords");
            info.creator = self.text(d, b"Creator");
            info.producer = self.text(d, b"Producer");
            const STANDARD: [&[u8]; 9] =
                [b"Title", b"Author", b"Subject", b"Keywords", b"Creator", b"Producer", b"CreationDate", b"ModDate", b"Trapped"];
            // The same rule as the engine's list after an edit (`pdfcraft_organize::custom_info`,
            // trimmed and without empty values): text strings only, so a name value is not a
            // custom property, and the cap counts listed entries.
            info.custom = d
                .iter()
                .filter(|(k, _)| !STANDARD.contains(&k.as_slice()))
                .filter_map(|(k, v)| match self.resolve(v) {
                    Object::String(bytes, _) => {
                        let v = text_string(bytes).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
                        (!v.is_empty()).then(|| (String::from_utf8_lossy(k).into_owned(), v))
                    }
                    _ => None,
                })
                .take(256)
                .collect();
        }
        let Ok(catalog) = self.doc.catalog() else { return };
        info.tagged = catalog
            .get(b"MarkInfo")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|m| m.get(b"Marked").ok())
            // Any value may be written as an indirect reference, `/Marked 9 0 R` included, so the
            // flag is resolved before it is read as a boolean (#3671).
            .map(|o| self.resolve(o))
            .and_then(|o| o.as_bool().ok())
            .unwrap_or(false);
        match self.page_labels(catalog, &mut info.pages) {
            Ok(notes) => info.warnings.extend(notes.into_iter().map(|note| format!("Page labels: {note}."))),
            Err(reason) => info.warnings.push(format!("Page labels were skipped: {reason}; using physical page numbers.")),
        }
        if let Some(first) = catalog.get(b"Outlines").ok().and_then(|o| self.dict(o)).and_then(|d| d.get(b"First").ok()) {
            (info.outline, info.outline_more) = self.outline_window(first);
        }
        let mut text_budget = MAX_LINK_ATTACHMENT_TEXT_BYTES;
        self.annotations(info, &mut text_budget);
        self.transitions(info);
        let mut field_seen = HashSet::new();
        if let Some(form) = catalog.get(b"AcroForm").ok().and_then(|o| self.dict(o))
            && let Ok(fields) = form.get(b"Fields").and_then(|o| self.resolve(o).as_array())
        {
            for f in fields {
                self.field(f, None, &mut info.fields, &mut field_seen, 0);
            }
        }
        // Widgets no `/Fields` entry reaches are fields too (Acrobat and the browsers fill them);
        // each is adopted through its topmost unlisted `/Parent`, so a split field stays one.
        for &pid in &self.page_ids {
            let Ok(page_dict) = self.doc.get_dictionary(pid) else { continue };
            let Ok(Object::Array(annots)) = page_dict.get(b"Annots").map(|o| self.resolve(o)) else { continue };
            for a in annots {
                let Some(d) = self.dict(a) else { continue };
                if self.name(d, b"Subtype").as_deref() != Some("Widget")
                    || (d.get(b"FT").is_err() && d.get(b"T").is_err() && d.get(b"Parent").is_err())
                {
                    continue;
                }
                if let Object::Reference(id) = a
                    && let Some(root) = self.unlisted_field(*id, &field_seen)
                {
                    self.field(&Object::Reference(root), None, &mut info.fields, &mut field_seen, 0);
                }
            }
        }
        info.has_javascript |= info.fields.iter().any(|f| f.has_actions);
        let needs_rendering = catalog.get(b"NeedsRendering").ok().and_then(|o| self.resolve(o).as_bool().ok()).unwrap_or(false);
        if let Some(xfa) = catalog.get(b"AcroForm").ok().and_then(|o| self.dict(o)).and_then(|form| form.get(b"XFA").ok()) {
            // A form with no fields is taken to be laid out from its template, unless its
            // configuration says it is a static form (#3180): Designer writes
            // `<dynamicRender>forbidden</dynamicRender>` for one, whose pages are the form.
            let static_form = self.xfa_dynamic_render(xfa).as_deref() == Some("forbidden");
            info.xfa = Some(if needs_rendering || (info.fields.is_empty() && !static_form) { Xfa::Dynamic } else { Xfa::Static });
        }
        self.layers(catalog, &mut info.layers);
        info.layer_groups = self.layer_groups(catalog);
        self.fonts(&mut info.fonts);
        if let Some(names) = catalog.get(b"Names").ok().and_then(|o| self.dict(o)) {
            if let Some(ef) = names.get(b"EmbeddedFiles").ok().and_then(|o| self.dict(o)) {
                let mut seen = HashSet::new();
                for (name, spec) in self.name_tree(ef, &mut seen, 0, &mut text_budget) {
                    // The key is kept a second time when it is also the name.
                    let fallback = |budget: &mut usize| spend(name.as_bytes(), budget, |_| name.clone());
                    info.attachments.push(self.attachment(spec, AttachmentSource::Document { key: name.clone() }, &mut text_budget, fallback));
                }
            }
            info.has_javascript |= names.get(b"JavaScript").is_ok();
        }
        if let Ok(action) = catalog.get(b"OpenAction")
            && let Some(dict) = self.dict(action)
            && dict.get(b"S").ok().and_then(|o| self.resolve(o).as_name().ok()) == Some(&b"JavaScript"[..])
        {
            info.has_javascript = true;
        }
        info.open_dest = catalog.get(b"OpenAction").ok().and_then(|o| self.open_dest(o));
    }

    /// The destination `/OpenAction` opens at (ISO 32000-2 §7.7.2 Table 29): an explicit
    /// destination array, a `/GoTo` action's `/D`, or (leniently, since Table 29 asks for an array
    /// or an action) a destination name or string; a name or string is looked up as a named
    /// destination (§12.3.2.4). Any other action (a script, a remote go-to) has none here, and
    /// neither has a page number past the end of the document.
    fn open_dest(&self, action: &Object) -> Option<(usize, DestView)> {
        let dest = match self.resolve(action) {
            Object::Dictionary(d) => {
                if d.get(b"S").ok().and_then(|o| self.resolve(o).as_name().ok()) != Some(&b"GoTo"[..]) {
                    return None;
                }
                d.get(b"D").ok()?
            }
            other => other,
        };
        self.dest(dest, 0).filter(|(page, _)| *page < self.page_ids.len())
    }

    // ── object helpers ──────────────────────────────────────────────────────────────────────

    /// `/Trans` and `/Dur` of every page (ISO 32000-2 §12.4.4), in page order. Lenient: a value
    /// outside the spec's choices takes that entry's default, and a page with neither entry is
    /// not listed. Bounded by the page count.
    fn transitions(&self, info: &mut DocInfo) {
        for (page, &pid) in self.page_ids.iter().enumerate() {
            let Ok(d) = self.doc.get_dictionary(pid) else { continue };
            let display = d.get(b"Dur").ok().and_then(|o| self.resolve(o).as_float().ok()).filter(|s| s.is_finite() && *s >= 0.0);
            let trans = d.get(b"Trans").ok().and_then(|o| self.dict(o));
            if trans.is_none() && display.is_none() {
                continue;
            }
            let mut t = PageTransition {
                page,
                style: None,
                duration: 1.0,
                horizontal: true,
                inward: true,
                direction: Some(0),
                scale: 1.0,
                opaque: false,
                display,
            };
            if let Some(tr) = trans {
                t.style = Some(self.name(tr, b"S").map_or(TransitionStyle::Replace, |n| TransitionStyle::from_name(&n)));
                let num = |key: &[u8]| tr.get(key).ok().and_then(|o| self.resolve(o).as_float().ok()).filter(|v| v.is_finite());
                t.duration = num(b"D").filter(|d| *d >= 0.0).unwrap_or(1.0);
                t.horizontal = self.name(tr, b"Dm").as_deref() != Some("V");
                t.inward = self.name(tr, b"M").as_deref() != Some("O");
                t.direction = match tr.get(b"Di").map(|o| self.resolve(o)) {
                    Ok(Object::Name(n)) if n.as_slice() == b"None" => None,
                    Ok(o) => match o.as_i64().ok() {
                        Some(0) => Some(0),
                        Some(90) => Some(90),
                        Some(180) => Some(180),
                        Some(270) => Some(270),
                        Some(315) => Some(315),
                        _ => Some(0),
                    },
                    Err(_) => Some(0),
                };
                t.scale = num(b"SS").filter(|s| *s > 0.0).unwrap_or(1.0);
                t.opaque = matches!(tr.get(b"B").map(|o| self.resolve(o)), Ok(Object::Boolean(true)));
            }
            info.transitions.push(t);
        }
    }

    /// The value of `dynamicRender` in an XFA form's configuration packet (`config`, under
    /// `acrobat/acrobat7`): `required` for a dynamic form, `forbidden` for a static one. `/XFA` is
    /// either one stream holding the whole XDP or an array of packet names and streams (ISO
    /// 32000-1 §12.7.8). Lenient: `None` when the packet is missing, unreadable or over
    /// `MAX_XFA_CONFIG_BYTES` decoded, or has no such element.
    fn xfa_dynamic_render(&self, xfa: &'a Object) -> Option<String> {
        let stream = match self.resolve(xfa) {
            Object::Array(items) => {
                let at = items.iter().position(|o| matches!(self.resolve(o), Object::String(name, _) if name.as_slice() == b"config"))?;
                self.resolve(items.get(at.checked_add(1)?)?)
            }
            whole => whole,
        };
        let stream = stream.as_stream().ok()?;
        let bytes = if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(MAX_XFA_CONFIG_BYTES).ok()?
        } else {
            stream.content.get(..stream.content.len().min(MAX_XFA_CONFIG_BYTES))?.to_vec()
        };
        xfa_config_dynamic_render(&xfa_packet_text(&bytes))
    }

    fn resolve(&self, o: &'a Object) -> &'a Object {
        self.doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
    }

    /// The object a reference ends at after following any chain of references; `None` for a
    /// direct object or a broken reference.
    fn resolved_id(&self, o: &Object) -> Option<ObjectId> {
        self.doc.dereference(o).ok().and_then(|(id, _)| id)
    }

    fn dict(&self, o: &'a Object) -> Option<&'a Dictionary> {
        match self.resolve(o) {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    fn text(&self, d: &Dictionary, key: &[u8]) -> Option<String> {
        let s = trimmed_text(self.resolve(d.get(key).ok()?))?;
        (!s.is_empty()).then_some(s)
    }

    /// [`Self::text`], converted and spent from `budget` as [`spend`] does, so text that trims
    /// short or to nothing still spends its raw length.
    fn budgeted_text(&self, d: &Dictionary, key: &[u8], budget: &mut usize) -> Option<String> {
        let o = self.resolve(d.get(key).ok()?);
        let (Object::String(bytes, _) | Object::Name(bytes)) = o else { return None };
        let text = spend(bytes, budget, |_| trimmed_text(o).unwrap_or_default())?;
        (!text.is_empty()).then_some(text)
    }

    fn name(&self, d: &Dictionary, key: &[u8]) -> Option<String> {
        self.resolve(d.get(key).ok()?).as_name().ok().map(|n| String::from_utf8_lossy(n).into_owned())
    }

    /// Whether an annotation is a Fill & Sign mark. Keep in step with
    /// `pdfcraft_annot::is_fill_sign`, which the engine's comment list uses.
    fn is_fill_sign(&self, d: &Dictionary, subtype: &str) -> bool {
        let yes = |key: &[u8]| matches!(d.get(key).map(|o| self.resolve(o)), Ok(Object::Boolean(true)));
        if yes(b"PCFillSign") {
            return true;
        }
        match subtype {
            "FreeText" => self.name(d, b"IT").as_deref() == Some("FreeTextTypeWriter"),
            "Stamp" => match self.name(d, b"Name").as_deref() {
                Some("PCCheck" | "PCCross" | "PCDot" | "PCLine" | "PCTypedSignature") => true,
                Some("PCCustomSignature" | "PCCustomInitials") => yes(b"PCPictureImage"),
                _ => false,
            },
            "Ink" => self.text(d, b"Subj").as_deref() == Some("Signature"),
            _ => false,
        }
    }

    fn page_of(&self, o: &Object) -> Option<usize> {
        match o {
            Object::Reference(id) => self.page_index.get(id).copied(),
            // Some producers write page *numbers* in remote-style destinations.
            Object::Integer(n) => usize::try_from(*n).ok(),
            _ => None,
        }
    }

    // ── destinations ────────────────────────────────────────────────────────────────────────

    /// The page and view of a destination: an explicit array (`[page /XYZ left top zoom]`, …),
    /// a dictionary with `/D`, or a named destination that resolves to one.
    fn dest(&self, dest: &Object, depth: u32) -> Option<(usize, DestView)> {
        if depth > 8 {
            return None;
        }
        match self.resolve(dest) {
            Object::Array(a) => {
                let page = a.first().and_then(|p| self.page_of(p))?;
                let kind = a.get(1).map(|k| self.resolve(k)).and_then(|k| k.as_name().ok());
                let operands: Vec<Option<f32>> = a.iter().skip(2).take(4).map(|o| self.resolve(o).as_float().ok()).collect();
                Some((page, DestView::from_operands(kind, &operands)))
            }
            Object::Dictionary(d) => d.get(b"D").ok().and_then(|d| self.dest(d, depth + 1)),
            Object::String(key, _) | Object::Name(key) => self.named_dest(key).and_then(|d| self.dest(d, depth + 1)),
            _ => None,
        }
    }

    fn named_dest(&self, key: &[u8]) -> Option<&'a Object> {
        let catalog = self.doc.catalog().ok()?;
        if let Some(v) = self.named.get(key) {
            return Some(v);
        }
        // PDF 1.1 style /Dests dictionary.
        catalog.get(b"Dests").ok().and_then(|o| self.dict(o)).and_then(|d| d.get(key).ok())
    }

    /// A name tree's leaves keyed by their raw key bytes (first occurrence wins).
    fn name_tree_raw(&self, node: &'a Dictionary, seen: &mut HashSet<*const Dictionary>, depth: u32, out: &mut HashMap<Vec<u8>, &'a Object>) {
        if depth > 32 || !seen.insert(node as *const _) {
            return;
        }
        if let Ok(Object::Array(pairs)) = node.get(b"Names").map(|o| self.resolve(o)) {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    let key = match self.resolve(k) {
                        Object::String(s, _) => s.clone(),
                        Object::Name(n) => n.clone(),
                        _ => continue,
                    };
                    out.entry(key).or_insert(v);
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k) {
                    self.name_tree_raw(d, seen, depth + 1, out);
                }
            }
        }
    }

    /// The entries of an embedded-files name tree. Each string key is spent from `text_budget`
    /// (see [`MAX_LINK_ATTACHMENT_TEXT_BYTES`]), and an entry whose key does not fit is left out.
    fn name_tree(
        &self,
        node: &'a Dictionary,
        seen: &mut HashSet<*const Dictionary>,
        depth: u32,
        text_budget: &mut usize,
    ) -> Vec<(String, &'a Object)> {
        let mut out = Vec::new();
        if depth > 32 || !seen.insert(node as *const _) {
            return out;
        }
        if let Ok(Object::Array(pairs)) = node.get(b"Names").map(|o| self.resolve(o)) {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    let key = match self.resolve(k) {
                        Object::String(bytes, _) => match spend(bytes, text_budget, text_string) {
                            Some(key) => key,
                            None => continue,
                        },
                        _ => String::new(),
                    };
                    out.push((key, v));
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k) {
                    out.extend(self.name_tree(d, seen, depth + 1, text_budget));
                }
            }
        }
        out
    }

    /// The first entry of an embedded-files name tree whose key is `key`, in the order
    /// [`Self::name_tree`] lists them, with no key budget. It keeps no key, and converts a key
    /// object or `/Names` array that several places reference only once (a second visit cannot
    /// match, since the first would have returned).
    fn name_tree_find(&self, node: &'a Dictionary, key: &str, seen: &mut NameTreeSeen, depth: u32) -> Option<&'a Object> {
        if depth > 32 || !seen.nodes.insert(node as *const _) {
            return None;
        }
        let names = node.get(b"Names").ok();
        if let Some(names) = names
            && self.resolved_id(names).is_none_or(|id| seen.objects.insert(id))
            && let Object::Array(pairs) = self.resolve(names)
        {
            for pair in pairs.chunks(2) {
                if let [k, v] = pair {
                    if self.resolved_id(k).is_some_and(|id| !seen.objects.insert(id)) {
                        continue;
                    }
                    let found = match self.resolve(k) {
                        Object::String(bytes, _) => text_string(bytes) == key,
                        _ => key.is_empty(),
                    };
                    if found {
                        return Some(v);
                    }
                }
            }
        }
        if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| self.resolve(o)) {
            for k in kids {
                if let Some(d) = self.dict(k)
                    && let Some(v) = self.name_tree_find(d, key, seen, depth + 1)
                {
                    return Some(v);
                }
            }
        }
        None
    }

    // ── outline ─────────────────────────────────────────────────────────────────────────────

    /// The window `DocInfo::outline` keeps: the first `OUTLINE_WINDOW` bookmarks in outline order,
    /// nested no deeper than `OUTLINE_DEPTH`, and whether any bookmark was left out. Iterative, and
    /// each bookmark is read once, so the work and memory stay within the window.
    fn outline_window(&self, first: &'a Object) -> (Vec<OutlineItem>, bool) {
        let mut seen = HashSet::new();
        // The listed bookmarks in outline order, with their depth.
        let mut listed: Vec<(usize, OutlineItem)> = Vec::new();
        // One entry per open level: its next sibling, or `None` once that run of siblings has ended.
        let mut levels: Vec<Option<&'a Object>> = vec![Some(first)];
        let mut more = false;
        while let Some(depth) = levels.len().checked_sub(1) {
            let Some(slot) = levels.get_mut(depth) else { break };
            let Some(o) = slot.take() else {
                levels.pop();
                continue;
            };
            if let Object::Reference(id) = o
                && !seen.insert(*id)
            {
                continue; // cycle: this run of siblings ends here
            }
            let Some(d) = self.dict(o) else { continue };
            if listed.len() >= OUTLINE_WINDOW {
                more = true;
                break;
            }
            *slot = d.get(b"Next").ok();
            let dest = d
                .get(b"Dest")
                .ok()
                .and_then(|dest| self.dest(dest, 0))
                .or_else(|| d.get(b"A").ok().and_then(|a| self.dict(a)).and_then(|a| a.get(b"D").ok()).and_then(|dest| self.dest(dest, 0)));
            let (page, view) = dest.map_or((None, DestView::Top), |(p, v)| (Some(p), v));
            match d.get(b"First").ok() {
                Some(child) if depth < OUTLINE_DEPTH => levels.push(Some(child)),
                Some(_) => more = true,
                None => {}
            }
            let open = d.get(b"Count").ok().and_then(|c| c.as_i64().ok()).is_some_and(|c| c > 0);
            let title = self.text(d, b"Title").unwrap_or_default();
            let color = match d.get(b"C").map(|o| self.resolve(o)) {
                Ok(Object::Array(c)) => outline_color(c.iter().map(|v| self.resolve(v).as_float().ok())),
                _ => None,
            };
            let flags = d.get(b"F").ok().and_then(|f| self.resolve(f).as_i64().ok()).unwrap_or(0);
            let (italic, bold) = (flags & 1 != 0, flags & 2 != 0);
            listed.push((depth, OutlineItem { title, page, view, children: Vec::new(), open, color, italic, bold }));
        }
        (nest_outline(listed), more)
    }

    // ── page labels (ISO 32000-2 §12.4.2) ───────────────────────────────────────────────────

    fn label_object(&self, object: &'a Object) -> Result<&'a Object, &'static str> {
        self.doc.dereference(object).map(|(_, o)| o).map_err(|_| "a number tree reference is missing or cyclic")
    }

    fn label_dict(&self, object: &'a Object) -> Result<Option<&'a Dictionary>, &'static str> {
        Ok(match self.label_object(object)? {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        })
    }

    /// Apply `/PageLabels` to `pages`. `Err`: the tree is unusable and no label is applied.
    /// `Ok(notes)`: labels applied, with what was repaired on the way (a duplicated start page,
    /// a range whose labels are too long to show).
    fn page_labels(&self, catalog: &Dictionary, pages: &mut [PageInfo]) -> Result<Vec<String>, &'static str> {
        let mut notes = Vec::new();
        let Ok(tree) = catalog.get(b"PageLabels") else { return Ok(notes) };
        let Some(tree) = self.label_dict(tree)? else { return Ok(notes) };
        let mut ranges: BTreeMap<usize, &Dictionary> = BTreeMap::new();
        let mut stack = vec![(tree, 0usize)];
        let mut seen = HashSet::new();
        let mut left = MAX_LABEL_TREE_WORK;
        while let Some((node, depth)) = stack.pop() {
            label_work(&mut left, 1)?;
            if depth > MAX_LABEL_TREE_DEPTH {
                return Err("the number tree is too deep");
            }
            if !seen.insert(node as *const Dictionary) {
                return Err("the number tree repeats a node or contains a cycle");
            }
            if let Ok(nums) = node.get(b"Nums")
                && let Object::Array(nums) = self.label_object(nums)?
            {
                label_work(&mut left, nums.len().div_ceil(2))?;
                for pair in nums.chunks(2) {
                    if let [k, v] = pair
                        && let (Ok(start), Some(d)) = (self.label_object(k)?.as_i64(), self.label_dict(v)?)
                        && let Ok(start) = usize::try_from(start)
                        && ranges.insert(start, d).is_some()
                    {
                        // The later range wins, as in readers that overwrite.
                        notes.push(format!("a range starts on page {} twice; the later one is used", start.saturating_add(1)));
                    }
                }
            }
            if let Ok(kids) = node.get(b"Kids")
                && let Object::Array(kids) = self.label_object(kids)?
            {
                label_work(&mut left, kids.len())?;
                if kids.len() > MAX_LABEL_TREE_WORK.saturating_sub(stack.len()) {
                    return Err("the number tree has too many pending nodes");
                }
                if !kids.is_empty() && depth == MAX_LABEL_TREE_DEPTH {
                    return Err("the number tree is too deep");
                }
                for kid in kids {
                    if let Some(d) = self.label_dict(kid)? {
                        stack.push((d, depth + 1));
                    }
                }
            }
        }
        // Decode each bounded prefix once, rather than once for every page in its range.
        let mut specs = BTreeMap::new();
        let mut prefix_total = 0usize;
        for (start, d) in ranges {
            let first = d.get(b"St").ok().and_then(|o| self.resolve(o).as_i64().ok()).unwrap_or(1).max(1) as u64;
            let prefix = match d.get(b"P").ok().map(|o| self.resolve(o)) {
                Some(Object::String(raw, _)) => {
                    if raw.len() > MAX_PREFIX_BYTES {
                        return Err("a raw prefix exceeds 2050 bytes");
                    }
                    // A label prefix is literal text, not trimmed document metadata.
                    text_string(raw).trim_matches('\0').to_string()
                }
                Some(Object::Name(raw)) => {
                    if raw.len() > MAX_PREFIX_BYTES {
                        return Err("a raw prefix exceeds 2050 bytes");
                    }
                    // Retain the existing name compatibility and boundary-NUL handling.
                    String::from_utf8_lossy(raw).trim_matches('\0').to_string()
                }
                _ => String::new(),
            };
            if prefix.len() > MAX_LABEL_BYTES {
                return Err("a decoded prefix exceeds 1024 UTF-8 bytes");
            }
            // A prefix object shared by many ranges is decoded once per range: count every copy.
            prefix_total = prefix_total.saturating_add(prefix.len());
            if prefix_total > MAX_PREFIX_TOTAL_BYTES {
                return Err("decoded prefixes exceed 4 MiB in total");
            }
            let style = d.get(b"S").ok().and_then(|o| self.resolve(o).as_name().ok());
            specs.insert(start, (first, prefix, style));
        }
        let mut staged = Vec::new();
        let mut total = 0usize;
        for (i, _) in pages.iter().enumerate() {
            let Some((&start, (first, prefix, style))) = specs.range(..=i).next_back() else { continue };
            let n = if matches!(*style, Some(b"D" | b"R" | b"r" | b"A" | b"a")) {
                let offset = u64::try_from(i - start).map_err(|_| "a page number is too large")?;
                first.checked_add(offset).ok_or("a page number overflows")?
            } else {
                0 // /St is unused when the label consists only of its prefix.
            };
            let remaining = MAX_LABEL_BYTES - prefix.len();
            let number = match *style {
                Some(b"D") => Some(n.to_string()),
                Some(b"R" | b"r") => roman(n, remaining),
                Some(b"A" | b"a") => alpha(n, remaining),
                _ => Some(String::new()),
            }
            .filter(|number| number.len() <= remaining);
            // A label too long to show keeps that page's physical number; other ranges stay.
            let Some(mut number) = number else {
                let note = format!(
                    "labels from page {} exceed {MAX_LABEL_BYTES} UTF-8 bytes; those pages show their physical page numbers",
                    start.saturating_add(1)
                );
                if !notes.contains(&note) {
                    notes.push(note);
                }
                continue;
            };
            if matches!(*style, Some(b"R" | b"A")) {
                number.make_ascii_uppercase();
            }
            let mut label = prefix.clone();
            label.push_str(&number);
            if label.is_empty() {
                continue; // Keep the existing physical number for an empty custom label.
            }
            total = total.checked_add(label.len()).ok_or("the total label size overflows")?;
            if total > MAX_LABEL_TOTAL_BYTES {
                return Err("custom labels exceed 4 MiB in total");
            }
            staged.push((i, label));
        }
        // A malformed later range must not leave a partially applied custom label sequence.
        for (i, label) in staged {
            if let Some(page) = pages.get_mut(i) {
                page.label = label;
            }
        }
        Ok(notes)
    }

    // ── annotations ─────────────────────────────────────────────────────────────────────────

    /// `text_budget` is what is left of [`MAX_LINK_ATTACHMENT_TEXT_BYTES`].
    fn annotations(&self, info: &mut DocInfo, text_budget: &mut usize) {
        let mut pages: Vec<(ObjectId, usize)> = self.page_index.iter().map(|(&id, &page)| (id, page)).collect();
        pages.sort_unstable_by_key(|&(_, page)| page);
        // An annotation, or a whole `/Annots` array, that several entries or pages list is on the
        // first of them (§12.5.2 allows one page per annotation). Both are told apart by the object
        // a reference resolves to, since an indirect object may itself hold a reference.
        let mut seen_arrays = HashSet::new();
        let mut seen_annotations = HashSet::new();
        let mut kept = 0usize;
        // Once the comment budget is spent, comments are no longer built, but links and attachments
        // are still listed.
        let mut comments_full = false;
        // Shared JavaScript actions have a separate source budget from link text.
        let mut javascript_remaining = 4 << 20;
        // Per-inspection negative cache: identity is the resolved stream, not the
        // annotation/action that refers to it. FIFO eviction bounds retained identities.
        let mut rejected_javascript = RejectedJavaScript::default();
        let mut javascript_limit_reported = false;
        for (id, page) in pages {
            let Ok(page_dict) = self.doc.get_dictionary(id) else { continue };
            if let Ok(annots) = page_dict.get(b"Annots")
                && let Some(r) = self.resolved_id(annots)
                && !seen_arrays.insert(r)
            {
                continue;
            }
            let Ok(Object::Array(annots)) = page_dict.get(b"Annots").map(|o| self.resolve(o)) else { continue };
            for (index, a) in annots.iter().enumerate() {
                if let Some(r) = self.resolved_id(a)
                    && !seen_annotations.insert(r)
                {
                    continue;
                }
                let Some(d) = self.dict(a) else { continue };
                let Some(subtype) = self.name(d, b"Subtype") else { continue };
                if subtype == "FileAttachment"
                    && let Ok(fs) = d.get(b"FS")
                {
                    let fallback = |budget: &mut usize| self.budgeted_text(d, b"Contents", budget);
                    info.attachments.push(self.attachment(fs, AttachmentSource::Annotation { page, index }, text_budget, fallback));
                }
                if subtype == "Link" {
                    if let Some(link) = self.link(page, index, d, text_budget, &mut javascript_remaining, &mut rejected_javascript) {
                        if matches!(&link.target, LinkTarget::JavaScript(_)) || matches!(&link.target, LinkTarget::Other(s) if s == "JavaScript") {
                            info.has_javascript = true;
                        }
                        if javascript_remaining == 0
                            && matches!(&link.target, LinkTarget::Other(s) if s == "JavaScript")
                            && !javascript_limit_reported
                        {
                            info.warnings.push(
                                "Link JavaScript sources reached the 4 MiB document inspection limit; remaining JavaScript actions are unavailable."
                                    .into(),
                            );
                            javascript_limit_reported = true;
                        }
                        info.links.push(link);
                    }
                    continue;
                }
                // Not comments: form widgets, pop-ups and non-markup annotations such as the
                // Screen annotation that drives a LaTeX `animate` player (keep in step with
                // `pdfcraft_annot::is_comment_subtype`).
                if matches!(subtype.as_str(), "Widget" | "Popup" | "Screen" | "Movie" | "RichMedia" | "3D" | "PrinterMark" | "TrapNet" | "Watermark")
                {
                    continue;
                }
                if comments_full {
                    continue;
                }
                let rect = rect4(self.resolve(d.get(b"Rect").unwrap_or(&Object::Null)));
                let mut color = match d.get(b"C").map(|o| self.resolve(o)) {
                    Ok(Object::Array(c)) if c.len() == 3 => {
                        let f = |i: usize| c[i].as_float().unwrap_or(0.0);
                        Some([f(0), f(1), f(2)])
                    }
                    _ => None,
                };
                // A text box's /C is its background; its text colour is in /DA.
                if subtype == "FreeText"
                    && let Some(da) = self.text(d, b"DA")
                {
                    let t: Vec<&str> = da.split_whitespace().collect();
                    if let Some(i) = t.iter().position(|x| *x == "rg")
                        && i >= 3
                    {
                        let f = |k: usize| t[k].parse::<f32>().unwrap_or(0.0);
                        color = Some([f(i - 3), f(i - 2), f(i - 1)]);
                    }
                }
                let quads = match d.get(b"QuadPoints").map(|o| self.resolve(o)) {
                    Ok(Object::Array(q)) => {
                        let v: Vec<f32> = q.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
                        v.as_chunks::<8>().0.to_vec()
                    }
                    _ => Vec::new(),
                };
                let in_reply_to = d.get(b"IRT").ok().and_then(|o| self.dict(o)).and_then(|p| self.text(p, b"NM"));
                let fill_sign = self.is_fill_sign(d, &subtype);
                // Keep in step with `pdfcraft_annot::summaries`.
                let key: &[u8] = if subtype == "Line" { b"L" } else { b"Vertices" };
                let points = match d.get(key).map(|o| self.resolve(o)) {
                    Ok(Object::Array(v)) if matches!(subtype.as_str(), "Line" | "PolyLine" | "Polygon") => {
                        let v: Vec<f32> = v.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
                        let pts: Vec<[f32; 2]> = v.as_chunks::<2>().0.to_vec();
                        if subtype == "Line" && pts.len() != 2 { Vec::new() } else { pts }
                    }
                    _ => Vec::new(),
                };
                let annotation = Annotation {
                    page,
                    subtype,
                    author: self.text(d, b"T"),
                    // Acrobat ends lines with CR, which the interface would draw as nothing (keep
                    // in step with `pdfcraft_annot::summaries`).
                    contents: self.text(d, b"Contents").map(|s| s.replace("\r\n", "\n").replace('\r', "\n")),
                    modified: self.text(d, b"M").map(|m| pretty_date(&m)),
                    name: self.text(d, b"NM"),
                    in_reply_to,
                    rect,
                    color,
                    index,
                    state: self.text(d, b"State"),
                    quads,
                    locked: d.get(b"F").ok().and_then(|f| self.resolve(f).as_i64().ok()).unwrap_or(0) & 128 != 0,
                    intent: self.name(d, b"IT"),
                    fill_sign,
                    no_rotate: d.get(b"F").is_ok_and(|f| draw_flags(self.resolve(f)) & 16 != 0),
                    points,
                };
                kept = kept.saturating_add(retained(&annotation));
                if kept > MAX_ANNOTATION_BYTES {
                    comments_full = true;
                    continue;
                }
                info.annotations.push(annotation);
            }
        }
        info.annotations.sort_by(|a, b| a.page.cmp(&b.page).then(b.rect[3].total_cmp(&a.rect[3])));
    }

    /// Inspect the direct destination/action; JavaScript source is data, never executed here.
    /// Script strings and decompressed streams are capped at 256 KiB before PDF-text
    /// conversion. `annotations` shares a 4 MiB budget for retained UTF-8 script text
    /// and one negative stream cache across the inspection. Decompression failures do
    /// not debit that budget; a decoded source exceeding its remainder exhausts it.
    /// Unsupported/refused script sources become Other("JavaScript"); malformed or
    /// missing actions can return None. This does not traverse an action's Next chain.
    fn link(
        &self,
        page: usize,
        index: usize,
        d: &Dictionary,
        text_budget: &mut usize,
        javascript_remaining: &mut usize,
        rejected_javascript: &mut RejectedJavaScript,
    ) -> Option<Link> {
        let rect = rect4(self.resolve(d.get(b"Rect").ok()?));
        let target = if let Ok(dest) = d.get(b"Dest") {
            let (page, view) = self.dest(dest, 0)?;
            LinkTarget::Page(page, view)
        } else {
            let a = d.get(b"A").ok().and_then(|a| self.dict(a))?;
            match self.name(a, b"S").as_deref() {
                Some("GoTo") => {
                    let (page, view) = self.dest(a.get(b"D").ok()?, 0)?;
                    LinkTarget::Page(page, view)
                }
                Some("URI") => {
                    let uri = a.get(b"URI").ok().and_then(|u| self.resolve(u).as_str().ok())?;
                    match spend(uri, text_budget, |b| String::from_utf8_lossy(b).into_owned()) {
                        Some(uri) => LinkTarget::Uri(uri),
                        None => LinkTarget::Other("URI".into()),
                    }
                }
                Some("SetOCGState") => self.layer_state(a),
                Some("GoToR") => self.remote_go_to(a).unwrap_or_else(|| LinkTarget::Other("GoToR".into())),
                Some("Launch") => self.launch_pdf(a).unwrap_or_else(|| LinkTarget::Other("Launch".into())),
                Some("JavaScript") => a
                    .get(b"JS")
                    .ok()
                    .and_then(|js| {
                        if *javascript_remaining == 0 {
                            return None;
                        }
                        const LIMIT: usize = 256 << 10;
                        let bytes = match self.resolve(js) {
                            Object::String(bytes, _) if bytes.len() <= LIMIT => bytes.clone(),
                            Object::Stream(stream) => {
                                // Structure returns stable references throughout this inspection.
                                // A resolved object address also identifies direct streams without
                                // conflating distinct actions or aliases to an indirect stream.
                                let identity = std::ptr::from_ref(stream);
                                if rejected_javascript.contains(identity) {
                                    return None;
                                }
                                #[cfg(test)]
                                JS_DECODE_COUNT.with(|n| n.set(n.get() + 1));
                                match stream.decompressed_content_with_limit(LIMIT) {
                                    Ok(bytes) => bytes,
                                    Err(_) => {
                                        rejected_javascript.insert(identity);
                                        return None;
                                    }
                                }
                            }
                            _ => return None,
                        };
                        let source = pdfcraft_cos::PdfString::literal(bytes).to_text();
                        let Some(remaining) = javascript_remaining.checked_sub(source.len()) else {
                            *javascript_remaining = 0;
                            return None;
                        };
                        *javascript_remaining = remaining;
                        Some(LinkTarget::JavaScript(source))
                    })
                    .unwrap_or_else(|| LinkTarget::Other("JavaScript".into())),
                Some(other) => LinkTarget::Other(other.to_string()),
                None => return None,
            }
        };
        Some(Link { page, index, rect, target })
    }

    fn remote_go_to(&self, action: &Dictionary) -> Option<LinkTarget> {
        let file = self.action_file(action)?;
        let destination = match action.get(b"D") {
            Err(_) => RemoteDestination::Default,
            Ok(o) => match self.resolve(o) {
                Object::Array(items) => {
                    let page = items.first().and_then(|o| self.resolve(o).as_i64().ok()).and_then(|n| usize::try_from(n).ok())?;
                    let kind = items.get(1).map(|o| self.resolve(o)).and_then(|o| o.as_name().ok());
                    let operands: [Option<f32>; 4] = std::array::from_fn(|i| items.get(i + 2).and_then(|o| self.resolve(o).as_float().ok()));
                    RemoteDestination::Page { page, view: DestView::from_operands(kind, &operands) }
                }
                Object::Name(name) if name.len() <= MAX_REMOTE_DESTINATION_NAME_BYTES => {
                    let name = String::from_utf8_lossy(name).into_owned();
                    if name.is_empty() || name.len() > MAX_REMOTE_DESTINATION_NAME_BYTES {
                        return None;
                    }
                    RemoteDestination::Named(name)
                }
                Object::String(bytes, _) if bytes.len() <= MAX_REMOTE_DESTINATION_NAME_BYTES => {
                    let name = text_string(bytes);
                    if name.is_empty() || name.len() > MAX_REMOTE_DESTINATION_NAME_BYTES {
                        return None;
                    }
                    RemoteDestination::Named(name)
                }
                _ => return None,
            },
        };
        Some(LinkTarget::RemoteGoTo { launch: false, file, destination, new_window: self.new_window(action) })
    }

    /// The non-empty local file an action's `/F` names (a string or a file specification), shared
    /// by GoToR and Launch. A URL file system (`/FS /URL`) names no local file.
    fn action_file(&self, action: &Dictionary) -> Option<String> {
        if self.name(action, b"FS").as_deref() == Some("URL") {
            return None;
        }
        let file = match self.resolve(action.get(b"F").ok()?) {
            Object::String(bytes, _) => limited_pdf_string(bytes, MAX_REMOTE_FILE_SPEC_BYTES)?,
            Object::Dictionary(spec) => {
                if self.name(spec, b"FS").as_deref() == Some("URL") {
                    return None;
                }
                spec.get(b"UF")
                    .ok()
                    .and_then(|o| self.pdf_text(o, MAX_REMOTE_FILE_SPEC_BYTES))
                    .or_else(|| spec.get(b"F").ok().and_then(|o| self.pdf_text(o, MAX_REMOTE_FILE_SPEC_BYTES)))?
            }
            _ => return None,
        };
        (!file.is_empty()).then_some(file)
    }

    fn new_window(&self, action: &Dictionary) -> Option<bool> {
        match action.get(b"NewWindow").map(|o| self.resolve(o)) {
            Ok(Object::Boolean(value)) => Some(*value),
            _ => None,
        }
    }

    /// A Launch action (ISO 32000-2 §12.6.4.6, Table 207) whose `/F` names a `.pdf` file, read as
    /// the remote Go-To to that file's first page that it amounts to (#4168). Launching anything
    /// else, or with the deprecated `/Win` launch parameters (an application, an operation, its
    /// arguments), is not followed: a document must not start programs. `\` separators, which
    /// Windows writers put in such paths, are read as `/` (§7.11.2.1 separates components with
    /// `/`) when the path has no `/` of its own. Launch has no `/D`, so one present (written
    /// for GoToR, or junk) is ignored rather than checked as a destination.
    fn launch_pdf(&self, action: &Dictionary) -> Option<LinkTarget> {
        if action.has(b"Win") {
            return None;
        }
        let file = self.action_file(action)?;
        let new_window = self.new_window(action);
        let file = if file.contains('/') { file } else { file.replace('\\', "/") };
        let pdf = file.rsplit('/').next().is_some_and(|name| name.len() > 4 && name.to_ascii_lowercase().ends_with(".pdf"));
        pdf.then_some(LinkTarget::RemoteGoTo { launch: true, file, destination: RemoteDestination::Default, new_window })
    }

    fn pdf_text(&self, o: &Object, max_bytes: usize) -> Option<String> {
        match self.resolve(o) {
            Object::String(bytes, _) => limited_pdf_string(bytes, max_bytes),
            _ => None,
        }
    }

    /// A set-OCG-state action: `/State` is `ON`, `OFF` or `Toggle`, each followed by the groups
    /// it applies to; `/PreserveRB` is true unless it is `false`.
    fn layer_state(&self, a: &Dictionary) -> LinkTarget {
        let mut changes = Vec::new();
        if let Ok(Object::Array(items)) = a.get(b"State").map(|o| self.resolve(o)) {
            let mut op = None;
            for item in items.iter().take(MAX_LAYER_STATE) {
                match item {
                    Object::Name(n) => {
                        op = match n.as_slice() {
                            b"ON" => Some(LayerOp::On),
                            b"OFF" => Some(LayerOp::Off),
                            b"Toggle" => Some(LayerOp::Toggle),
                            _ => None,
                        }
                    }
                    Object::Reference(id) => changes.extend(op.map(|op| (op, *id))),
                    _ => {}
                }
            }
        }
        let preserve_rb = !matches!(a.get(b"PreserveRB").map(|o| self.resolve(o)), Ok(Object::Boolean(false)));
        LinkTarget::SetLayers { changes, preserve_rb }
    }

    // ── form fields ─────────────────────────────────────────────────────────────────────────

    fn field(
        &self,
        o: &'a Object,
        parent: Option<(&str, Option<String>, Option<i64>)>,
        out: &mut Vec<Field>,
        seen: &mut HashSet<ObjectId>,
        depth: u32,
    ) {
        if depth > 32 {
            return;
        }
        if let Object::Reference(id) = o
            && !seen.insert(*id)
        {
            return;
        }
        let Some(d) = self.dict(o) else { return };
        let (parent_name, parent_ft, parent_ff) = parent.unwrap_or(("", None, None));
        let partial = self.text(d, b"T");
        let full = match (&partial, parent_name.is_empty()) {
            (Some(t), true) => t.clone(),
            (Some(t), false) => format!("{parent_name}.{t}"),
            (None, _) => parent_name.to_string(),
        };
        let ft = self.name(d, b"FT").or(parent_ft);
        let ff = d.get(b"Ff").ok().and_then(|x| x.as_i64().ok()).or(parent_ff);
        // Non-terminal: has kids that are fields (they carry /T). Widgets-only kids are terminal.
        let kids = match d.get(b"Kids").map(|k| self.resolve(k)) {
            Ok(Object::Array(k)) => k.as_slice(),
            _ => &[],
        };
        let field_kids: Vec<_> = kids.iter().filter(|k| self.dict(k).is_some_and(|kd| kd.get(b"T").is_ok())).collect();
        if !field_kids.is_empty() {
            for k in field_kids {
                self.field(k, Some((&full, ft.clone(), ff)), out, seen, depth + 1);
            }
            return;
        }
        let flags = ff.unwrap_or(0);
        let kind = match ft.as_deref() {
            Some("Tx") => FieldKind::Text,
            Some("Btn") if flags & (1 << 16) != 0 => FieldKind::PushButton,
            Some("Btn") if flags & (1 << 15) != 0 => FieldKind::Radio,
            Some("Btn") => FieldKind::CheckBox,
            Some("Ch") if flags & (1 << 17) != 0 => FieldKind::Combo,
            Some("Ch") => FieldKind::List,
            Some("Sig") => FieldKind::Signature,
            _ => FieldKind::Unknown,
        };
        let value = match d.get(b"V").map(|v| self.resolve(v)) {
            Ok(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
            Ok(Object::String(bytes, _)) => Some(text_string(bytes)),
            Ok(Object::Array(a)) => Some(
                a.iter()
                    .filter_map(|x| match self.resolve(x) {
                        Object::String(bytes, _) => Some(text_string(bytes)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Ok(Object::Dictionary(_)) if kind == FieldKind::Signature => Some("signed".into()),
            _ => None,
        };
        // The widget is either this dict (merged field/widget) or its first kid.
        let widget = if d.get(b"Rect").is_ok() { Some(d) } else { kids.first().and_then(|k| self.dict(k)) };
        let page = widget.and_then(|w| w.get(b"P").ok()).and_then(|p| self.page_of(p)).or_else(|| self.find_widget_page(o, kids));
        let has_actions = d.get(b"AA").is_ok() || widget.is_some_and(|w| w.get(b"AA").is_ok() || w.get(b"A").is_ok());
        let rect = widget.and_then(|w| w.get(b"Rect").ok()).map(|r| rect4(self.resolve(r)));
        out.push(Field { name: full, kind, value, page, tooltip: self.text(d, b"TU"), has_actions, rect });
    }

    /// The first page whose `/Annots` lists the field or one of its kids.
    fn find_widget_page(&self, field: &Object, kids: &[Object]) -> Option<usize> {
        let pages = self.annotation_pages.get_or_init(|| self.annotation_pages());
        std::iter::once(field).chain(kids).filter_map(|o| if let Object::Reference(id) = o { pages.get(id).copied() } else { None }).min()
    }

    /// Each annotation listed in a page's `/Annots`, with the first page that lists it.
    fn annotation_pages(&self) -> HashMap<ObjectId, usize> {
        let mut pages = HashMap::new();
        for (idx, &pid) in self.page_ids.iter().enumerate() {
            let Some(annots) = self.doc.get_dictionary(pid).ok().and_then(|d| d.get(b"Annots").ok()) else { continue };
            let Ok(annots) = self.resolve(annots).as_array() else { continue };
            for a in annots {
                if let Object::Reference(id) = a {
                    pages.entry(*id).or_insert(idx);
                }
            }
        }
        pages
    }

    /// The topmost field above widget `id` (itself without a `/Parent`), or `None` when it or an
    /// ancestor is already reachable from the `/Fields` tree, or a `/Parent` loop hides the top.
    fn unlisted_field(&self, id: ObjectId, seen: &HashSet<ObjectId>) -> Option<ObjectId> {
        let mut top = id;
        let mut visited = HashSet::from([id]);
        loop {
            let o = Object::Reference(top);
            let d = self.dict(&o)?;
            let parent = match d.get(b"Parent") {
                Ok(Object::Reference(p)) => Some(*p),
                _ => None,
            };
            let Some(p) = parent else {
                return if seen.contains(&top) { None } else { Some(top) };
            };
            if !visited.insert(p) || visited.len() > 64 {
                return None;
            }
            if seen.contains(&p) {
                return None;
            }
            top = p;
        }
    }

    // ── optional content ────────────────────────────────────────────────────────────────────

    fn layers(&self, catalog: &Dictionary, out: &mut Vec<Layer>) {
        let Some(props) = catalog.get(b"OCProperties").ok().and_then(|o| self.dict(o)) else { return };
        let config = props.get(b"D").ok().and_then(|o| self.dict(o));
        let off: HashSet<ObjectId> = config
            .and_then(|c| c.get(b"OFF").ok())
            .and_then(|o| self.resolve(o).as_array().ok())
            .map(|a| a.iter().filter_map(|x| x.as_reference().ok()).collect())
            .unwrap_or_default();
        let base_off = config.and_then(|c| self.name(c, b"BaseState")).as_deref() == Some("OFF");
        let on: HashSet<ObjectId> = config
            .and_then(|c| c.get(b"ON").ok())
            .and_then(|o| self.resolve(o).as_array().ok())
            .map(|a| a.iter().filter_map(|x| x.as_reference().ok()).collect())
            .unwrap_or_default();
        let Ok(Object::Array(ocgs)) = props.get(b"OCGs").map(|o| self.resolve(o)) else { return };
        for g in ocgs {
            let Some(d) = self.dict(g) else { continue };
            let id = g.as_reference().ok();
            let visible = match id {
                Some(id) if off.contains(&id) => false,
                Some(id) if on.contains(&id) => true,
                _ => !base_off,
            };
            let Some(id) = id else { continue };
            out.push(Layer { id, name: self.text(d, b"Name").unwrap_or_else(|| "Layer".into()), visible, initial: visible });
        }
    }

    /// The default configuration's radio-button groups (`/RBGroups`) of two or more layers.
    fn layer_groups(&self, catalog: &Dictionary) -> Vec<Vec<ObjectId>> {
        let groups = catalog
            .get(b"OCProperties")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|p| p.get(b"D").ok())
            .and_then(|o| self.dict(o))
            .and_then(|c| c.get(b"RBGroups").ok())
            .and_then(|o| self.resolve(o).as_array().ok());
        let mut left = MAX_LAYER_GROUP_ENTRIES;
        let mut out = Vec::new();
        for g in groups.into_iter().flatten() {
            let Ok(members) = self.resolve(g).as_array() else { continue };
            let group: Vec<ObjectId> = members.iter().filter_map(|x| x.as_reference().ok()).take(left).collect();
            left = left.saturating_sub(group.len());
            if group.len() > 1 {
                out.push(group);
            }
            if left == 0 {
                break;
            }
        }
        out
    }

    fn fonts(&self, out: &mut Vec<FontInfo>) {
        let mut seen = HashSet::new();
        for &pid in &self.page_ids {
            let Ok(fonts) = self.doc.get_page_fonts(pid) else { continue };
            for (_, f) in fonts {
                let base = f
                    .get(b"BaseFont")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_else(|| "(unnamed)".into());
                let kind = self.name(f, b"Subtype").unwrap_or_default();
                let encoding = match f.get(b"Encoding").map(|o| self.resolve(o)) {
                    Ok(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
                    Ok(Object::Dictionary(_)) => Some("Custom".into()),
                    Ok(Object::Stream(_)) => Some("Embedded CMap".into()),
                    _ => None,
                };
                // Embedded: FontFile/2/3 in the descriptor (for Type0, in the descendant font).
                let descriptor_of = |d: &'a Dictionary| d.get(b"FontDescriptor").ok().and_then(|o| self.dict(o));
                let descendant =
                    f.get(b"DescendantFonts").ok().and_then(|o| self.resolve(o).as_array().ok()).and_then(|a| a.first()).and_then(|o| self.dict(o));
                let desc = descriptor_of(f).or_else(|| descendant.and_then(descriptor_of));
                let embedded =
                    kind == "Type3" || desc.is_some_and(|d| d.get(b"FontFile").is_ok() || d.get(b"FontFile2").is_ok() || d.get(b"FontFile3").is_ok());
                // What the substitution chain needs beyond the name: the descriptor flags, and for
                // a composite font the script its descendant declares (ISO 32000-2 9.7.3).
                let flags = desc
                    .and_then(|d| d.get(b"Flags").ok())
                    .and_then(|o| self.resolve(o).as_i64().ok())
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0);
                let ordering =
                    descendant.and_then(|d| d.get(b"CIDSystemInfo").ok()).and_then(|o| self.dict(o)).and_then(|c| self.text(c, b"Ordering"));
                let subset = base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase());
                let name = if subset { base[7..].to_string() } else { base };
                let chosen = pdfcraft_fonts::fallback::substitute(&pdfcraft_fonts::fallback::Request {
                    base_font: &name,
                    embedded,
                    flags,
                    ordering: ordering.as_deref(),
                });
                let info = FontInfo {
                    name,
                    kind,
                    embedded,
                    subset,
                    encoding,
                    substitute: chosen.face_name(),
                    substitution: (!chosen.why.is_faithful()).then(|| chosen.why.note().to_owned()),
                };
                if seen.insert((info.name.clone(), info.kind.clone(), info.embedded)) {
                    out.push(info);
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// The attachment's name is the file specification's, else `fallback`'s, else a generic one.
    fn attachment(
        &self,
        spec: &Object,
        source: AttachmentSource,
        text_budget: &mut usize,
        fallback: impl FnOnce(&mut usize) -> Option<String>,
    ) -> Attachment {
        let d = self.dict(spec);
        let size =
            d.and_then(|d| d.get(b"EF").ok()).and_then(|o| self.dict(o)).and_then(|ef| ef.get(b"F").ok().or_else(|| ef.get(b"UF").ok())).and_then(
                |f| match self.resolve(f) {
                    Object::Stream(s) => s
                        .dict
                        .get(b"Params")
                        .ok()
                        .and_then(|p| self.dict(p))
                        .and_then(|p| p.get(b"Size").ok())
                        .and_then(|s| s.as_i64().ok())
                        .map(|s| s as usize)
                        .or(Some(s.content.len())),
                    _ => None,
                },
            );
        let display = d
            .and_then(|d| self.budgeted_text(d, b"UF", text_budget).or_else(|| self.budgeted_text(d, b"F", text_budget)))
            .or_else(|| fallback(text_budget))
            .unwrap_or_else(|| "attachment".into());
        Attachment { name: display, description: d.and_then(|d| self.budgeted_text(d, b"Desc", text_budget)), size, source }
    }
}

/// A text string's value, decoded by the same rules as the engine's object model
/// ([`pdfcraft_cos::PdfString::to_text`]) so the panels and outline editing agree.
fn text_string(bytes: &[u8]) -> String {
    pdfcraft_cos::PdfString::literal(bytes).to_text()
}

/// What [`Inspector::name_tree_find`] has visited: node dictionaries, and the key and `/Names`
/// array objects it reached by reference.
#[derive(Default)]
struct NameTreeSeen {
    nodes: HashSet<*const Dictionary>,
    objects: HashSet<ObjectId>,
}

/// A string or name value as text, without surrounding NULs and white space.
fn trimmed_text(o: &Object) -> Option<String> {
    let s = match o {
        Object::String(bytes, _) => text_string(bytes),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => return None,
    };
    Some(s.trim_matches('\0').trim().to_string())
}

/// `convert(bytes)` when the result fits in `budget`, which it then spends by the longer of the raw
/// and converted lengths. Bytes longer than the budget are not converted. A conversion that comes
/// out too long spends the raw length, so a string many entries share is converted only until the
/// budget runs out.
fn spend(bytes: &[u8], budget: &mut usize, convert: impl FnOnce(&[u8]) -> String) -> Option<String> {
    if bytes.len() > *budget {
        return None;
    }
    let text = convert(bytes);
    if text.len() > *budget {
        *budget = budget.saturating_sub(bytes.len());
        return None;
    }
    // The conversion's work follows the raw length, even when the text it keeps is shorter.
    *budget = budget.saturating_sub(text.len().max(bytes.len()));
    Some(text)
}

fn limited_pdf_string(bytes: &[u8], max_bytes: usize) -> Option<String> {
    if bytes.len() > max_bytes {
        return None;
    }
    let text = text_string(bytes);
    (text.len() <= max_bytes).then_some(text)
}

/// A bookmark title's colour from its `/C` entries, each kept within 0–1 (ISO 32000-2 §12.3.3,
/// Table 151: "An array of three numbers in the range 0.0 to 1.0, representing the components in
/// the DeviceRGB colour space"). Black, the default, and anything that isn't three numbers give
/// `None`.
pub(crate) fn outline_color(values: impl Iterator<Item = Option<f32>>) -> Option<[f32; 3]> {
    let v: Vec<Option<f32>> = values.take(4).collect();
    let [Some(r), Some(g), Some(b)] = v.as_slice() else { return None };
    // A number past the range, however large, is kept at its edge.
    let rgb = [*r, *g, *b].map(|c| c.clamp(0.0, 1.0));
    (rgb != [0.0; 3]).then_some(rgb)
}

/// Nests a pre-order list of `(depth, item)` into the outline tree.
fn nest_outline(listed: Vec<(usize, OutlineItem)>) -> Vec<OutlineItem> {
    let mut roots = Vec::new();
    // The open ancestors, outermost first; each collects its children until it closes.
    let mut open: Vec<(usize, OutlineItem)> = Vec::new();
    for (depth, item) in listed {
        while open.last().is_some_and(|(d, _)| *d >= depth) {
            close_outline(&mut open, &mut roots);
        }
        open.push((depth, item));
    }
    while !open.is_empty() {
        close_outline(&mut open, &mut roots);
    }
    roots
}

/// Closes the innermost open bookmark: it joins its parent's children, or the top level.
fn close_outline(open: &mut Vec<(usize, OutlineItem)>, roots: &mut Vec<OutlineItem>) {
    if let Some((_, item)) = open.pop() {
        match open.last_mut() {
            Some((_, parent)) => parent.children.push(item),
            None => roots.push(item),
        }
    }
}

/// An annotation's `/F` as the renderer reads it: a real is truncated, and a value that isn't a
/// `u32` counts as no flags.
fn draw_flags(o: &Object) -> u32 {
    let f = match o {
        Object::Integer(i) => *i,
        Object::Real(r) => *r as i64,
        _ => 0,
    };
    u32::try_from(f).unwrap_or(0)
}

fn rect4(o: &Object) -> [f32; 4] {
    match o {
        Object::Array(a) if a.len() == 4 => {
            let v: Vec<f32> = a.iter().map(|x| x.as_float().unwrap_or(0.0)).collect();
            [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]
        }
        _ => [0.0; 4],
    }
}

fn label_work(left: &mut usize, count: usize) -> Result<(), &'static str> {
    *left = left.checked_sub(count).ok_or("the number tree has too many entries")?;
    Ok(())
}

/// `D:20260930104512-04'00'` → `2026-09-30 10:45`.
/// A PDF date (`D:20261001123000Z`) as "2026-10-01 12:30"; other strings unchanged.
pub fn pretty_date(s: &str) -> String {
    let d = s.trim_start_matches("D:");
    if let Some(d) = d.get(..12).filter(|d| d.bytes().all(|b| b.is_ascii_digit())) {
        // Twelve ASCII digits make every slice below a UTF-8 character boundary.
        format!("{}-{}-{} {}:{}", &d[0..4], &d[4..6], &d[6..8], &d[8..10], &d[10..12])
    } else {
        s.to_string()
    }
}

/// Fetch an attachment's bytes (decoded). Capped at 1 GiB to defuse decompression bombs.
pub fn attachment_data(bytes: &[u8], password: Option<&str>, att: &Attachment) -> Result<Vec<u8>, String> {
    let run = || -> Result<Vec<u8>, String> {
        let options = load_options(password);
        let doc = Document::load_mem_with_options(bytes, options).map_err(|e| e.to_string())?;
        let insp = Inspector::new(&doc);
        let spec: &Object = match &att.source {
            AttachmentSource::Document { key } => {
                let names = doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| insp.dict(o)).ok_or("no /Names")?;
                let ef = names.get(b"EmbeddedFiles").ok().and_then(|o| insp.dict(o)).ok_or("no /EmbeddedFiles")?;
                insp.name_tree_find(ef, key, &mut NameTreeSeen::default(), 0).ok_or("attachment not found")?
            }
            AttachmentSource::Annotation { page, index } => {
                let (&pid, _) = insp.page_index.iter().find(|(_, i)| **i == *page).ok_or("page not found")?;
                let annots = doc.get_dictionary(pid).map_err(|e| e.to_string())?.get(b"Annots").map_err(|e| e.to_string())?;
                let a = insp.resolve(annots).as_array().map_err(|e| e.to_string())?.get(*index).ok_or("annotation not found")?;
                insp.dict(a).and_then(|d| d.get(b"FS").ok()).ok_or("annotation has no file")?
            }
        };
        let fs = insp.dict(spec).ok_or("bad file specification")?;
        let ef = fs.get(b"EF").ok().and_then(|o| insp.dict(o)).ok_or("the file is not embedded (external reference)")?;
        let stream = ef.get(b"UF").ok().or_else(|| ef.get(b"F").ok()).map(|o| insp.resolve(o)).ok_or("no embedded stream")?;
        let stream = stream.as_stream().map_err(|e| e.to_string())?;
        if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(1 << 30).map_err(|e| e.to_string())
        } else {
            Ok(stream.content.clone())
        }
    };
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|p| Err(format!("reading the attachment crashed: {}", crate::raster::panic_message(&p))))
}

/// An XFA packet's bytes as text: UTF-16 (little- or big-endian, told by a byte order mark or by
/// how `<` is written) or UTF-8, as `pdfcraft_xfa::packets::decode` reads them. Lossy: only one
/// element is looked for.
fn xfa_packet_text(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], from: fn([u8; 2]) -> u16| {
        let units = b.as_chunks::<2>().0.iter().map(|c| from(*c));
        char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect::<String>()
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [b'<', 0, ..] => utf16(bytes, u16::from_le_bytes),
        [0, b'<', ..] => utf16(bytes, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// One markup tag found by [`next_tag`]: where it starts (`<`) and ends (after `>`), whether it
/// closes (`</x>`) or closes itself (`<x/>`), and its name without a namespace prefix.
struct Tag<'a> {
    start: usize,
    end: usize,
    close: bool,
    empty: bool,
    local: &'a str,
}

/// The next element tag in `xml` at or after `from`, skipping comments, CDATA sections,
/// processing instructions and declarations. A byte scan, not an XML parser: enough to find one
/// known element in an XFA packet, whose tags Designer writes with a line break before each `>`.
fn next_tag(xml: &str, mut from: usize) -> Option<Tag<'_>> {
    loop {
        let lt = from + xml.get(from..)?.find('<')?;
        let rest = xml.get(lt..)?;
        let skip_to = |end: &str| rest.find(end).map(|i| lt + i + end.len());
        if rest.starts_with("<!--") {
            from = skip_to("-->")?;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            from = skip_to("]]>")?;
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            from = skip_to(">")?;
            continue;
        }
        let gt = lt + rest.find('>')?;
        let inner = xml.get(lt + 1..gt)?;
        let (close, inner) = match inner.strip_prefix('/') {
            Some(i) => (true, i),
            None => (false, inner),
        };
        let empty = !close && inner.ends_with('/');
        let name = inner.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default();
        let local = name.rsplit(':').next().unwrap_or(name);
        return Some(Tag { start: lt, end: gt + 1, close, empty, local });
    }
}

/// What `<name>…</name>` holds: the content of the first top-level element of `xml` called `name`
/// (any prefix). Elements nested in other elements are not looked at. `None` when there is none,
/// it closes itself, or it is never closed.
fn xml_child_body<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let mut at = 0;
    // The elements open around `at`, innermost last: a closing tag must name the innermost one,
    // or the markup is malformed and has no value.
    let mut open_tags: Vec<&str> = Vec::new();
    // Where the wanted element's content starts, once its opening tag is seen at the top level.
    let mut open = None;
    loop {
        let tag = next_tag(xml, at)?;
        at = tag.end;
        if tag.close {
            if open_tags.pop()? != tag.local {
                return None;
            }
            if open_tags.is_empty()
                && let Some(start) = open
            {
                return xml.get(start..tag.start);
            }
        } else if !tag.empty {
            if open_tags.is_empty() && tag.local == name {
                open = Some(tag.end);
            }
            open_tags.push(tag.local);
        } else if open_tags.is_empty() && tag.local == name {
            return None;
        }
    }
}

/// The trimmed text at `path` (element names from the outside in, any prefix, each a child of the
/// one before) in `xml`, if it is plain text.
fn xml_path_text(xml: &str, path: &[&str]) -> Option<String> {
    let body = path.iter().try_fold(xml, |within, name| xml_child_body(within, name))?;
    let text = body.trim();
    (!text.is_empty() && !text.contains('<')).then(|| text.to_owned())
}

/// `dynamicRender` at `config/acrobat/acrobat7` in the configuration packet: the `config` child
/// of the `xdp` root in a whole-XDP stream, or the `config` packet itself. The same names inside
/// the form data or the template are not the configuration.
fn xfa_config_dynamic_render(packet: &str) -> Option<String> {
    const IN_CONFIG: [&str; 4] = ["config", "acrobat", "acrobat7", "dynamicRender"];
    xml_path_text(packet, &["xdp", IN_CONFIG[0], IN_CONFIG[1], IN_CONFIG[2], IN_CONFIG[3]]).or_else(|| xml_path_text(packet, &IN_CONFIG))
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_predictor_rows_longer_than_the_data_are_refused_before_allocating() {
        // Vendored lopdf patch: a fuzzed `/Columns 4294967295` allocated two 4 GiB rows before
        // reading any data. A row that cannot fit in the data is refused up front.
        let e = lopdf::filters::png::decode_frame(&[2, 0, 0, 0], 1, 64 << 20).unwrap_err();
        assert!(e.to_string().contains("longer than the data"), "{e}");
        // A real frame still decodes: two 3-byte rows, the second Up-filtered.
        assert_eq!(lopdf::filters::png::decode_frame(&[0, 1, 2, 3, 2, 1, 1, 1], 1, 3).unwrap(), [1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn empty_png_predictor_frames_do_not_allocate_rows() {
        // usize::MAX cannot be reserved, so the old code safely errors before allocating.
        // An empty frame needs no rows regardless of the declared width.
        assert!(lopdf::filters::png::decode_frame(&[], 1, usize::MAX).unwrap().is_empty());
    }

    fn tiff_predictor_stream(data: Vec<u8>, columns: i64, colors: i64, bits: i64) -> lopdf::Stream {
        let mut params = lopdf::Dictionary::new();
        params.set("Predictor", 2i64);
        params.set("Columns", columns);
        params.set("Colors", colors);
        params.set("BitsPerComponent", bits);
        let mut dict = lopdf::Dictionary::new();
        dict.set("DecodeParms", params);
        let mut stream = lopdf::Stream::new(dict, data);
        stream.compress().unwrap();
        assert_eq!(stream.dict.get(b"Filter").unwrap().as_name().unwrap(), b"FlateDecode");
        stream
    }

    #[test]
    fn tiff_subbyte_predictor_row_width_overflow_is_refused() {
        // Each multiplication overflows before any scratch allocation in the old debug build.
        for bits in [1, 2, 4] {
            let stream = tiff_predictor_stream(vec![0; 64], i64::MAX, 3, bits);
            assert!(
                matches!(stream.decompressed_content_with_limit(64), Err(lopdf::Error::Decompress(lopdf::DecompressError::Predictor(_)))),
                "{bits}-bit row"
            );
        }
    }

    #[test]
    fn tiff_subbyte_predictor_scratch_is_bounded_by_available_samples() {
        // 1/2-bit row widths fit usize on 32- and 64-bit hosts; Colors previously made Vec<u16>
        // reject the capacity before allocating. A partial row has no preceding pixel.
        for bits in [1, 2] {
            let data = vec![0b1010_0110; 64];
            let stream = tiff_predictor_stream(data.clone(), 1, i64::try_from(isize::MAX).unwrap(), bits);
            assert_eq!(stream.decompressed_content_with_limit(64).unwrap(), data, "{bits}-bit row");
        }
    }

    #[test]
    fn tiff_subbyte_predictors_keep_components_rows_and_padding() {
        // Repeated tiny rows compress through the public Stream API. Rows remain independent,
        // differences wrap at each component depth, and trailing padding bits survive.
        for (columns, colors, bits, encoded, decoded) in [
            (16, 1, 1, vec![255, 170, 8, 255], vec![170, 204, 15, 85]),
            (6, 1, 2, vec![85, 179], vec![108, 147]),
            (3, 1, 4, vec![25, 16], vec![26, 176]),
            (2, 2, 4, vec![18, 34], vec![18, 52]),
        ] {
            let stream = tiff_predictor_stream(encoded.repeat(32), columns, colors, bits);
            assert_eq!(stream.decompressed_content_with_limit(128).unwrap(), decoded.repeat(32), "{bits}-bit, {colors} colours");
        }
    }

    // Xref-stream fixtures exercise indirect /Length lookup during document loading: stream 2's /Length is `3 0 R`,
    // `containers` lists compressed (id, container, index) entries and `objects` the other objects in the file.
    fn compressed_length_pdf(containers: &[(u32, u32, u16)], objects: &[(u32, &[u8])]) -> Vec<u8> {
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let ids = containers.iter().flat_map(|&(id, container, _)| [id, container]).chain(objects.iter().map(|&(id, _)| id));
        let xref_id = ids.max().unwrap_or(3) + 1;
        let mut records = vec![[0u8; 7]; (xref_id + 1) as usize];
        records[0] = [0, 0, 0, 0, 0, 255, 255];
        let mut append = |id: u32, body: &[u8]| {
            records[id as usize][0] = 1;
            records[id as usize][1..5].copy_from_slice(&(pdf.len() as u32).to_be_bytes());
            pdf.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
            pdf.extend_from_slice(body);
            pdf.extend_from_slice(b"\nendobj\n");
        };
        append(1, b"<< /Type /Catalog >>");
        append(2, b"<< /Length 3 0 R >>\nstream\nhello\nendstream");
        for &(id, body) in objects {
            append(id, body);
        }
        for &(id, container, index) in containers {
            records[id as usize][0] = 2;
            records[id as usize][1..5].copy_from_slice(&container.to_be_bytes());
            records[id as usize][5..7].copy_from_slice(&index.to_be_bytes());
        }
        let xref_offset = pdf.len();
        records[xref_id as usize][0] = 1;
        records[xref_id as usize][1..5].copy_from_slice(&(xref_offset as u32).to_be_bytes());
        pdf.extend_from_slice(
            format!("{xref_id} 0 obj\n<< /Type /XRef /Size {} /Root 1 0 R /W [1 4 2] /Length {} >>\nstream\n", records.len(), records.len() * 7)
                .as_bytes(),
        );
        for record in records {
            pdf.extend_from_slice(&record);
        }
        pdf.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes());
        pdf
    }

    // Stream 2's /Length starts a chain of `hops` indirect /Length lookups, each naming a new object; only the last
    // link's /Length is direct. Compressed: link k is the integer 3 + 2k (the length it measures) alone in the object
    // stream 4 + 2k, whose own /Length is link k + 1. Plain: link k is the stream 3 + k, whose /Length is link k + 1.
    fn length_chain_pdf(hops: u32, compressed: bool) -> Vec<u8> {
        let length = |hop: u32, next: u32, data: &str| if hop + 1 < hops { format!("{next} 0 R") } else { data.len().to_string() };
        let (mut containers, mut objects) = (Vec::new(), Vec::new());
        let mut value = 5; // the length of stream 2's data, "hello"
        for hop in 0..hops {
            if compressed {
                let (id, container) = (3 + 2 * hop, 4 + 2 * hop);
                let index = format!("{id} 0 ");
                let data = format!("{index}{value}");
                let dict = format!("/Type /ObjStm /N 1 /First {} /Length {}", index.len(), length(hop, id + 2, &data));
                objects.push((container, format!("<< {dict} >>\nstream\n{data}\nendstream")));
                containers.push((id, container, 0));
                value = data.len();
            } else {
                objects.push((3 + hop, format!("<< /Length {} >>\nstream\n\nendstream", length(hop, 4 + hop, ""))));
            }
        }
        let objects: Vec<_> = objects.iter().map(|(id, body)| (*id, body.as_bytes())).collect();
        compressed_length_pdf(&containers, &objects)
    }

    // Lenient loading keeps a document whose stream 2 has an unresolvable /Length: the catalog, and stream 2's
    // dictionary without its data.
    fn load_without_stream_2_data(pdf: &[u8]) -> Document {
        let doc = Document::load_mem(pdf).unwrap();
        assert!(doc.catalog().unwrap().has_type(b"Catalog"));
        assert!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content.is_empty());
        doc
    }

    #[test]
    fn compressed_object_self_container_does_not_recurse() {
        let doc = load_without_stream_2_data(&compressed_length_pdf(&[(3, 3, 0)], &[]));
        assert!(doc.get_object((3, 0)).is_err());
    }

    #[test]
    fn compressed_object_mutual_containers_do_not_recurse() {
        let doc = load_without_stream_2_data(&compressed_length_pdf(&[(3, 4, 0), (4, 3, 0)], &[]));
        assert!(doc.get_object((3, 0)).is_err());
        assert!(doc.get_object((4, 0)).is_err());
    }

    #[test]
    fn object_stream_length_cycle_does_not_recurse() {
        let pdf = compressed_length_pdf(&[(3, 4, 0)], &[(4, b"<< /Type /ObjStm /N 1 /First 4 /Length 3 0 R >>\nstream\n3 0 5\nendstream")]);
        let doc = load_without_stream_2_data(&pdf);
        // The container's /Length is the object it holds: the container is kept without data, so 3 never loads.
        assert!(doc.get_object((4, 0)).unwrap().as_stream().unwrap().content.is_empty());
        assert!(doc.get_object((3, 0)).is_err());
    }

    #[test]
    fn compressed_object_container_chain_does_not_recurse() {
        let containers: Vec<_> = (3..100_003).map(|id| (id, id + 1, 0)).collect();
        let pdf = compressed_length_pdf(&containers, &[(100_003, b"<< /Type /ObjStm /N 1 /First 4 /Length 5 >>\nstream\n3 0 5\nendstream")]);
        let doc = load_without_stream_2_data(&pdf);
        // The object stream in the file loads, but the xref puts 3 in the compressed container 4, which is refused.
        assert_eq!(doc.get_object((100_003, 0)).unwrap().as_stream().unwrap().content, b"3 0 5");
        assert!(doc.get_object((3, 0)).is_err());
    }

    #[test]
    fn compressed_objects_and_indirect_stream_length_still_load() {
        let pdf = compressed_length_pdf(
            &[(3, 4, 0), (5, 4, 1)],
            &[(4, b"<< /Type /ObjStm /N 2 /First 8 /Length 17 >>\nstream\n3 0 5 2 5 (value)\nendstream")],
        );
        let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict: true, ..Default::default() }).unwrap();
        assert_eq!(doc.get_object((3, 0)).unwrap().as_i64().unwrap(), 5);
        assert_eq!(doc.get_object((5, 0)).unwrap().as_str().unwrap(), b"value");
        assert_eq!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content, b"hello");
    }

    #[test]
    fn compressed_length_in_a_container_with_an_indirect_length_still_loads() {
        // Two hops, as in a valid file: stream 2's /Length (3) is compressed in container 4, whose own /Length (5) is
        // a plain indirect integer.
        let pdf =
            compressed_length_pdf(&[(3, 4, 0)], &[(4, b"<< /Type /ObjStm /N 1 /First 4 /Length 5 0 R >>\nstream\n3 0 5\nendstream"), (5, b"5")]);
        let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict: true, ..Default::default() }).unwrap();
        assert_eq!(doc.get_object((3, 0)).unwrap().as_i64().unwrap(), 5);
        assert_eq!(doc.get_object((5, 0)).unwrap().as_i64().unwrap(), 5);
        assert_eq!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content, b"hello");
    }

    #[test]
    fn compressed_length_chain_does_not_recurse() {
        // Every container is an ordinary object in the file and every hop names a new object, so neither the
        // container check nor the cycle history stops the chain; each hop recursed once more (a 512 KiB stack
        // overflowed within 150 hops). Lookups past the cap fail: the first link never loads, while the last one,
        // next to the direct /Length, does.
        let hops = 5_000;
        let doc = load_without_stream_2_data(&length_chain_pdf(hops, true));
        assert!(doc.get_object((3, 0)).is_err());
        assert!(doc.get_object((3 + 2 * (hops - 1), 0)).unwrap().as_i64().is_ok());
        // The cap itself: loading link j reads its container's /Length, a chain of 2 * (hops - 1 - j) lookups, and
        // 16 are allowed, so the 9th link from the end loads and the 10th does not.
        assert!(doc.get_object((3 + 2 * (hops - 9), 0)).is_ok());
        assert!(doc.get_object((3 + 2 * (hops - 10), 0)).is_err());
    }

    #[test]
    fn plain_stream_length_chain_does_not_recurse() {
        // The same recursion without object streams: each stream's /Length names the next stream (a 512 KiB stack
        // overflowed within 200 hops). A stream is no length, so every link is kept as a stream without data.
        let hops = 5_000;
        let doc = load_without_stream_2_data(&length_chain_pdf(hops, false));
        assert!((3..3 + hops).all(|id| doc.get_object((id, 0)).is_ok_and(|link| link.as_stream().is_ok_and(|s| s.content.is_empty()))));
    }

    fn indexed_object_stream(pairs: &[(u32, usize)], members: &[u8]) -> lopdf::Stream {
        let mut content = Vec::new();
        for &(id, offset) in pairs {
            content.extend_from_slice(format!("{id} {offset} ").as_bytes());
        }
        let mut dict = Dictionary::new();
        dict.set("Type", "ObjStm");
        dict.set("N", pairs.len() as i64);
        dict.set("First", content.len() as i64);
        content.extend_from_slice(members);
        lopdf::Stream::new(dict, content)
    }

    #[test]
    fn object_stream_member_extents_bound_owned_strings() {
        let mut members = vec![b'('; 32];
        members.extend(vec![b'x'; 1 << 20]);
        members.extend([b')'; 32]);
        // Put the innermost (complete) string first in index order, then overlapping starts
        // and duplicates. Only this first member at offset 31 may own the large string.
        let mut pairs = vec![(100, 31)];
        pairs.extend((0..31).map(|offset| (offset as u32 + 1, offset)));
        pairs.extend([(200, 31), (201, 31)]);
        let stream = indexed_object_stream(&pairs, &members);
        let parsed = lopdf::ObjectStream::new_with_limit(&stream, Some(2 << 20)).unwrap();
        assert_eq!(parsed.objects.get(&(100, 0)).unwrap().as_str().unwrap().len(), 1 << 20);
        let total: usize = parsed.objects.values().filter_map(|object| object.as_str().ok()).map(<[u8]>::len).sum();
        assert!(total <= stream.content.len(), "owned {total} string bytes from {} decoded bytes", stream.content.len());
        assert!(!parsed.objects.contains_key(&(200, 0)));
        assert!(!parsed.objects.contains_key(&(201, 0)));
    }

    #[test]
    fn object_stream_member_extents_preserve_index_order_and_whitespace() {
        let members = b" \n42\n /Name \n[true 7]\n << /Key (value) >>\n (last)";
        // Unsorted offsets, and object 9 appears twice: its last index entry still wins.
        let pairs = [(9, 0), (12, 21), (10, 5), (11, 12), (9, 41)];
        let parsed = lopdf::ObjectStream::new(&indexed_object_stream(&pairs, members)).unwrap();
        assert_eq!(parsed.objects.len(), 4);
        assert_eq!(parsed.objects.get(&(9, 0)).unwrap().as_str().unwrap(), b"last");
        assert_eq!(parsed.objects.get(&(10, 0)).unwrap().as_name().unwrap(), b"Name");
        assert_eq!(parsed.objects.get(&(11, 0)).unwrap().as_array().unwrap(), &[Object::Boolean(true), Object::Integer(7)]);
        assert_eq!(parsed.objects.get(&(12, 0)).unwrap().as_dict().unwrap().get(b"Key").unwrap().as_str().unwrap(), b"value");
    }

    #[test]
    fn view_and_user_space_round_trip_for_every_rotation() {
        for rotation in [0u16, 90, 180, 270] {
            let (w, h) = if rotation % 180 == 0 { (200.0, 300.0) } else { (300.0, 200.0) };
            let p = super::PageInfo { width: w, height: h, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation };
            for (x, y) in [(0.0, 0.0), (15.0, 40.0), (w, h)] {
                let [ux, uy] = p.view_to_user(x, y);
                let [vx, vy] = p.user_to_view(ux, uy);
                assert!((vx - x).abs() < 1e-3 && (vy - y).abs() < 1e-3, "{rotation}: {x},{y} → {ux},{uy} → {vx},{vy}");
            }
        }
        // Unrotated: the view's top-left is the crop box's top-left.
        let p = super::PageInfo { width: 200.0, height: 300.0, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 0 };
        assert_eq!(p.view_to_user(0.0, 0.0), [10.0, 320.0]);
        assert_eq!(p.view_rect_to_quad([0.0, 0.0, 10.0, 5.0]), [10.0, 320.0, 20.0, 320.0, 10.0, 315.0, 20.0, 315.0]);
    }

    /// #2583: a NoRotate comment is drawn upright with its top-left as displayed at the upper-left
    /// corner of `/Rect`, its width across and its height down; a box resized as drawn maps back to
    /// the `/Rect` that shows it. Other comments, and pages that aren't quarter-turned, keep `/Rect`.
    #[test]
    fn no_rotate_comments_are_shown_upright_from_their_corner_and_back() {
        let mut note = super::Annotation {
            page: 0,
            subtype: "Text".into(),
            author: None,
            contents: None,
            modified: None,
            name: None,
            in_reply_to: None,
            rect: [40.0, 50.0, 70.0, 70.0],
            color: None,
            index: 0,
            state: None,
            quads: Vec::new(),
            locked: false,
            intent: None,
            fill_sign: false,
            no_rotate: true,
            points: Vec::new(),
        };
        let rect = note.rect.map(f64::from);
        for rotation in [0u16, 90, 180, 270, 45] {
            let (w, h) = if rotation % 180 == 90 { (140.0, 200.0) } else { (200.0, 140.0) };
            let p = super::PageInfo { width: w, height: h, label: String::new(), crop: [0.0, 0.0, 200.0, 140.0], rotation };
            let shown = note.shown_rect(rotation);
            let (a, b) = (p.user_to_view(shown[0], shown[1]), p.user_to_view(shown[2], shown[3]));
            let [cx, cy] = p.user_to_view(40.0, 70.0);
            let want = [cx, cy, cx + 30.0, cy + 20.0];
            let got = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
            assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-3), "{rotation}: shown {got:?}, want {want:?}");
            assert_eq!(note.rect_for_shown(note.shown(rect, rotation), rotation), rect, "{rotation}");
            // Moved by (5, -7) and resized to 50 × 10 as drawn, back to a /Rect.
            let resized = [shown[0] as f64 + 5.0, shown[1] as f64 - 7.0, 0.0, 0.0];
            let across = if rotation % 180 == 90 { [10.0, 50.0] } else { [50.0, 10.0] };
            let resized = [resized[0], resized[1], resized[0] + across[0], resized[1] + across[1]];
            let back = note.rect_for_shown(resized, rotation);
            assert_eq!((back[2] - back[0], back[3] - back[1]), (50.0, 10.0), "{rotation}: {back:?}");
            assert_eq!(note.shown(back, rotation), resized, "{rotation}");
        }
        note.no_rotate = false;
        assert_eq!(note.shown_rect(90), note.rect);
        assert_eq!(note.rect_for_shown([1.0, 2.0, 3.0, 4.0], 90), [1.0, 2.0, 3.0, 4.0]);
    }

    use super::*;

    #[test]
    fn roman_and_alpha_labels() {
        assert_eq!(roman(4, MAX_LABEL_BYTES).as_deref(), Some("iv"));
        assert_eq!(roman(1994, MAX_LABEL_BYTES).as_deref(), Some("mcmxciv"));
        assert_eq!(alpha(1, MAX_LABEL_BYTES).as_deref(), Some("a"));
        assert_eq!(alpha(27, MAX_LABEL_BYTES).as_deref(), Some("aa"));
        assert_eq!(alpha(53, MAX_LABEL_BYTES).as_deref(), Some("aaa"));
    }

    // Synthetic label-only inspection fixtures: never construct an explosive branching tree.
    fn label_info(nums: Vec<Object>, kids: Vec<Object>) -> DocInfo {
        let mut doc = Document::with_version("1.7");
        let mut tree = Dictionary::new();
        tree.set("Nums", nums);
        tree.set("Kids", kids);
        doc.objects.insert((6, 0), Object::Dictionary(tree));
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("PageLabels", Object::Reference((6, 0)));
        doc.objects.insert((1, 0), Object::Dictionary(catalog));
        doc.trailer.set("Root", Object::Reference((1, 0)));
        let mut info = DocInfo {
            pages: (1..=3)
                .map(|n| PageInfo { width: 200.0, height: 300.0, crop: [0.0, 0.0, 200.0, 300.0], rotation: 0, label: n.to_string() })
                .collect(),
            ..Default::default()
        };
        Inspector::new(&doc).fill(&mut info);
        info
    }

    fn label_spec(style: &str, prefix: Vec<u8>, first: i64) -> Object {
        let mut d = Dictionary::new();
        d.set("S", Object::Name(style.as_bytes().to_vec()));
        d.set("P", Object::String(prefix, lopdf::StringFormat::Hexadecimal));
        d.set("St", first);
        Object::Dictionary(d)
    }

    fn assert_label_fallback(info: &DocInfo) {
        assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
        assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
        assert!(info.warnings[0].contains("Page labels"), "{:?}", info.warnings);
        assert!(info.warnings[0].contains("physical page numbers"), "{:?}", info.warnings);
    }

    // Contributor-original, blank three-page PDFs. The public importer reads both labels and metadata.
    fn page_prefix_space_pdf(prefix: Vec<u8>, decimal: bool, indirect: bool) -> Vec<u8> {
        use lopdf::dictionary;
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let kids: Vec<Object> = (0..3)
            .map(|_| {
                doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
                    "Resources" => Dictionary::new(),
                })
                .into()
            })
            .collect();
        doc.objects.insert(pages_id, dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 3 }.into());
        let prefix = Object::String(prefix, lopdf::StringFormat::Hexadecimal);
        let prefix = if indirect { Object::Reference(doc.add_object(prefix)) } else { prefix };
        let mut label = Dictionary::new();
        label.set("P", prefix);
        if decimal {
            label.set("S", Object::Name(b"D".to_vec()));
        }
        let labels = dictionary! { "Nums" => vec![Object::Integer(0), Object::Dictionary(label)] };
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "PageLabels" => labels });
        let info_id = doc.add_object(dictionary! { "Title" => Object::string_literal("  Metadata title  ") });
        doc.trailer.set("Root", catalog_id);
        doc.trailer.set("Info", info_id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("write original page-prefix fixture");
        bytes
    }

    #[test]
    fn page_label_prefix_spaces_survive_public_inspection() {
        let mut utf16 = vec![0xfe, 0xff];
        utf16.extend(" Ω ".encode_utf16().flat_map(u16::to_be_bytes));
        for (raw, prefix, decimal, indirect) in [
            (b" leading".to_vec(), " leading", true, false),
            (b"trailing ".to_vec(), "trailing ", true, false),
            (b" both ".to_vec(), " both ", false, false),
            (b" ".to_vec(), " ", false, false),
            (b" Caf\xe9 ".to_vec(), " Café ", true, false),
            (utf16.clone(), " Ω ", true, false),
            (utf16, " Ω ", false, true),
            (Vec::new(), "", true, false),
            (Vec::new(), "", false, false),
        ] {
            let bytes = page_prefix_space_pdf(raw, decimal, indirect);
            let info = inspect(Arc::new(bytes), None).expect("public import of original page-prefix fixture");
            let expected: Vec<String> = (1..=3)
                .map(|n| {
                    if decimal {
                        format!("{prefix}{n}")
                    } else if prefix.is_empty() {
                        n.to_string()
                    } else {
                        prefix.to_owned()
                    }
                })
                .collect();
            assert_eq!(
                info.pages.iter().map(|p| p.label.clone()).collect::<Vec<_>>(),
                expected,
                "{prefix:?}, decimal={decimal}, indirect={indirect}"
            );
            assert!(info.warnings.is_empty(), "{:?}", info.warnings);
            assert_eq!(info.title.as_deref(), Some("Metadata title"), "page-prefix decoding must not change metadata normalization");
        }
    }

    #[test]
    fn page_label_limits_keep_normal_pdf_numbering() {
        for (style, first, expected) in [
            ("r", 4, ["iv", "v", "vi"]),
            ("R", 1994, ["MCMXCIV", "MCMXCV", "MCMXCVI"]),
            ("a", 26, ["z", "aa", "bb"]),
            ("A", 52, ["ZZ", "AAA", "BBB"]),
        ] {
            let info = label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), first)], Vec::new());
            assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), expected);
            assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        }
    }

    #[test]
    fn page_label_limits_reject_long_prefixes_without_partial_labels() {
        for prefix in [vec![b'x'; 1025], vec![b'x'; 2051]] {
            let nums = vec![Object::Integer(0), label_spec("D", b"ok-".to_vec(), 1), Object::Integer(1), label_spec("D", prefix, 1)];
            assert_label_fallback(&label_info(nums, Vec::new()));
        }
    }

    #[test]
    fn page_label_limits_count_decoded_utf8_bytes() {
        let mut prefix = vec![0xfe, 0xff];
        for _ in 0..342 {
            prefix.extend_from_slice(&0x2022u16.to_be_bytes());
        }
        assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec("", prefix, 1)], Vec::new()));
    }

    #[test]
    fn page_label_limits_bound_roman_and_alpha_output() {
        // Just 1025 output bytes on the old path, so RED does not consume excessive memory.
        for (style, first) in [("r", 1_025_000), ("a", 26 * 1024 + 1)] {
            assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), first)], Vec::new()));
        }
    }

    #[test]
    fn page_label_limits_detect_one_node_cycle() {
        let nums = vec![Object::Integer(0), label_spec("D", b"custom-".to_vec(), 1)];
        assert_label_fallback(&label_info(nums, vec![Object::Reference((6, 0))]));
    }

    #[test]
    fn page_label_limits_accept_utf16_ascii_at_the_output_boundary() {
        let mut prefix = vec![0xfe, 0xff];
        for _ in 0..1023 {
            prefix.extend_from_slice(&u16::from(b'x').to_be_bytes());
        }
        let info = label_info(vec![Object::Integer(0), label_spec("D", prefix, 1)], Vec::new());
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        assert_eq!(info.pages[0].label, format!("{}1", "x".repeat(1023)));
        assert!(info.pages.iter().all(|p| p.label.len() == MAX_LABEL_BYTES));
    }

    #[test]
    fn page_label_limits_refuse_hostile_numbers_before_allocating() {
        for style in ["r", "R", "a", "A"] {
            assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), i64::MAX)], Vec::new()));
        }
        // Decimal labels remain small even when their first number exceeds 32-bit usize.
        let info = label_info(vec![Object::Integer(0), label_spec("D", Vec::new(), i64::MAX)], Vec::new());
        assert_eq!(info.pages[0].label, i64::MAX.to_string());
        assert_eq!(info.pages[1].label, (i64::MAX as u64 + 1).to_string());
        assert!(info.warnings.is_empty());
    }

    #[test]
    fn page_label_limits_bound_wide_number_trees() {
        assert_label_fallback(&label_info(Vec::new(), vec![Object::Null; MAX_LABEL_TREE_WORK + 1]));
        let pair = [Object::Integer(0), label_spec("D", Vec::new(), 1)];
        let nums = (0..MAX_LABEL_TREE_WORK).flat_map(|_| pair.clone()).collect();
        assert_label_fallback(&label_info(nums, Vec::new()));
    }

    #[test]
    fn page_label_limits_bound_total_custom_output_atomically() {
        let doc = Document::with_version("1.7");
        let mut tree = Dictionary::new();
        tree.set("Nums", vec![Object::Integer(0), label_spec("", vec![b'x'; MAX_LABEL_BYTES], 1)]);
        let mut catalog = Dictionary::new();
        catalog.set("PageLabels", Object::Dictionary(tree));
        let count = MAX_LABEL_TOTAL_BYTES / MAX_LABEL_BYTES + 1;
        let mut pages: Vec<_> = (1..=count)
            .map(|n| PageInfo { width: 200.0, height: 300.0, crop: [0.0, 0.0, 200.0, 300.0], rotation: 0, label: n.to_string() })
            .collect();
        let err = Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap_err();
        assert!(err.contains("4 MiB"), "{err}");
        assert!(pages.iter().enumerate().all(|(i, p)| p.label == (i + 1).to_string()));
        // Exact aggregate boundary remains supported.
        pages.pop();
        Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap();
        assert!(pages.iter().all(|p| p.label.len() == MAX_LABEL_BYTES));
    }

    #[test]
    fn page_label_limits_bound_the_total_of_decoded_prefixes() {
        // `ranges` ranges naming one shared prefix object of `prefix` bytes, without a style.
        let shared = |ranges: i64, prefix: usize| {
            let mut doc = Document::with_version("1.7");
            doc.objects.insert((7, 0), Object::String(vec![b'x'; prefix], lopdf::StringFormat::Literal));
            let mut spec = Dictionary::new();
            spec.set("P", Object::Reference((7, 0)));
            doc.objects.insert((8, 0), Object::Dictionary(spec));
            let mut tree = Dictionary::new();
            tree.set("Nums", (0..ranges).flat_map(|i| [Object::Integer(i), Object::Reference((8, 0))]).collect::<Vec<_>>());
            let mut catalog = Dictionary::new();
            catalog.set("PageLabels", Object::Dictionary(tree));
            (doc, catalog)
        };
        let mut pages: Vec<_> =
            (1..=3).map(|n| PageInfo { width: 200.0, height: 300.0, crop: [0.0, 0.0, 200.0, 300.0], rotation: 0, label: n.to_string() }).collect();
        // Each range decodes its prefix, so a small file sharing one prefix object asks for a
        // copy per range: 5,000 ranges and a 1,000-byte prefix make 5 MB, past the 4 MiB total.
        // Three pages make three labels, far below the label total: the prefixes alone refuse it.
        let (doc, catalog) = shared(5000, 1000);
        let err = Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap_err();
        assert!(err.contains("prefixes exceed 4 MiB"), "{err}");
        assert!(pages.iter().enumerate().all(|(i, p)| p.label == (i + 1).to_string()));
        // Exactly 4 MiB of prefixes is still read.
        let (doc, catalog) = shared((MAX_PREFIX_TOTAL_BYTES / MAX_LABEL_BYTES) as i64, MAX_LABEL_BYTES);
        Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap();
        assert!(pages.iter().all(|p| p.label.len() == MAX_LABEL_BYTES));
    }

    #[test]
    fn page_label_limits_handle_prefix_only_and_reference_cycles() {
        let info = label_info(vec![Object::Integer(0), label_spec("", b"prefix".to_vec(), i64::MAX)], Vec::new());
        assert!(info.warnings.is_empty());
        assert!(info.pages.iter().all(|p| p.label == "prefix"));
        let mut doc = Document::with_version("1.7");
        doc.objects.insert((6, 0), Object::Reference((6, 0)));
        let mut catalog = Dictionary::new();
        catalog.set("PageLabels", Object::Reference((6, 0)));
        assert!(Inspector::new(&doc).page_labels(&catalog, &mut []).unwrap_err().contains("cyclic"));
    }

    #[test]
    fn page_label_limits_bound_depth_and_count_combined_work() {
        for depth in [MAX_LABEL_TREE_DEPTH, MAX_LABEL_TREE_DEPTH + 1] {
            let mut child = Dictionary::new();
            child.set("Nums", vec![Object::Integer(0), label_spec("D", b"custom-".to_vec(), 1)]);
            let mut child = Object::Dictionary(child);
            for _ in 1..depth {
                let mut parent = Dictionary::new();
                parent.set("Kids", vec![child]);
                child = Object::Dictionary(parent);
            }
            let info = label_info(Vec::new(), vec![child]);
            if depth == MAX_LABEL_TREE_DEPTH {
                assert!(info.warnings.is_empty());
                assert_eq!(info.pages[0].label, "custom-1");
            } else {
                assert_label_fallback(&info);
            }
        }
        // Each array alone fits: their combined work still must be bounded.
        let half = MAX_LABEL_TREE_WORK / 2;
        let spec = label_spec("D", Vec::new(), 1);
        let nums = (0..half).flat_map(|i| [Object::Integer(i as i64), spec.clone()]).collect();
        assert_label_fallback(&label_info(nums, vec![Object::Null; half]));
    }

    #[test]
    fn page_label_limits_keep_the_later_of_duplicate_ranges() {
        let nums = vec![Object::Integer(0), label_spec("D", b"first-".to_vec(), 1), Object::Integer(0), label_spec("D", b"last-".to_vec(), 1)];
        let info = label_info(nums, Vec::new());
        assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["last-1", "last-2", "last-3"]);
        assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
        assert!(info.warnings[0].contains("twice"), "{:?}", info.warnings);
    }

    #[test]
    fn page_label_limits_keep_other_ranges_when_one_is_too_long() {
        // Page 1 is labelled normally; pages 2–3 would need over 1024 bytes of roman numerals.
        let nums = vec![Object::Integer(0), label_spec("D", b"ok-".to_vec(), 1), Object::Integer(1), label_spec("r", Vec::new(), 1_025_000)];
        let info = label_info(nums, Vec::new());
        assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["ok-1", "2", "3"]);
        assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
        assert!(info.warnings[0].contains("physical page numbers"), "{:?}", info.warnings);
    }

    const ATTACHMENTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(notes.txt) 6 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] /Resources << /Font << /F1 9 0 R >> >> >> endobj
4 0 obj << /Type /Annot /Subtype /FileAttachment /Rect [10 10 30 30] /Contents (data.csv) /FS 8 0 R >> endobj
5 0 obj << /Type /EmbeddedFile /Length 11 >> stream
hello notes
endstream endobj
6 0 obj << /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 5 0 R >> /Desc (Doc-level) >> endobj
7 0 obj << /Type /EmbeddedFile /Length 5 >> stream
a,b,c
endstream endobj
8 0 obj << /Type /Filespec /F (data.csv) /EF << /F 7 0 R >> >> endobj
9 0 obj << /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn attachments_of_both_kinds_are_listed_and_extracted() {
        let bytes = Arc::new(ATTACHMENTS.to_vec());
        let info = inspect(bytes.clone(), None).expect("opens");
        let names: Vec<_> = info.attachments.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"notes.txt") && names.contains(&"data.csv"), "{names:?}");
        for a in &info.attachments {
            let data = attachment_data(&bytes, None, a).expect("extracts");
            let expected: &[u8] = if a.name == "notes.txt" { b"hello notes" } else { b"a,b,c" };
            assert_eq!(data, expected, "{}", a.name);
        }
        // A non-embedded /Helvetica is reported as standing in for itself: the standard-14
        // metrics are exact, so the substitution is named rather than left implicit.
        assert_eq!(
            info.fonts,
            vec![FontInfo {
                name: "Helvetica".into(),
                kind: "Type1".into(),
                embedded: false,
                subset: true,
                encoding: None,
                substitute: Some("Helvetica".into()),
                substitution: Some(pdfcraft_fonts::fallback::Why::NamedStandard14.note().into()),
            }]
        );
    }

    const LAYERS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R 7 0 R] /D << /OFF [6 0 R] /RBGroups [[5 0 R 6 0 R] [7 0 R] 8 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [10 0 R 11 0 R] >> endobj
5 0 obj << /Type /OCG /Name (Red) >> endobj
6 0 obj << /Type /OCG /Name (Green) >> endobj
7 0 obj << /Type /OCG /Name (Blue) >> endobj
8 0 obj [6 0 R 7 0 R] endobj
9 0 obj [/OFF 5 0 R] endobj
10 0 obj << /Type /Annot /Subtype /Link /Rect [10 10 50 30] /A << /S /SetOCGState /State [7 0 R /ON 6 0 R /Bogus 5 0 R /Toggle 5 0 R 1 7 0 R] /PreserveRB false >> >> endobj
11 0 obj << /Type /Annot /Subtype /Link /Rect [60 10 100 30] /A << /S /SetOCGState /State 9 0 R >> >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    /// `/Trans` and `/Dur` (ISO 32000-2 §12.4.4): read with the spec's defaults for what a page
    /// leaves out, lenient about what is out of range, and only for pages that have either.
    const TRANSITIONS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R 7 0 R] /Count 5 /MediaBox [0 0 200 200] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Trans << /Type /Trans /S /Wipe /D 2.5 /Di 270 >> /Dur 5 >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Dur 3 >> endobj
5 0 obj << /Type /Page /Parent 2 0 R /Trans 8 0 R >> endobj
6 0 obj << /Type /Page /Parent 2 0 R /Trans << /S /Bogus /D (slow) /Di 45 /Dm /Q >> /Dur -1 >> endobj
7 0 obj << /Type /Page /Parent 2 0 R >> endobj
8 0 obj << /S /Fly /M /O /SS 0.5 /B true /Di /None /Dm /V >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn page_transitions_and_display_times_are_read() {
        use super::TransitionStyle as S;
        let info = inspect(Arc::new(TRANSITIONS.to_vec()), None).expect("opens");
        let t = &info.transitions;
        assert_eq!(t.iter().map(|t| t.page).collect::<Vec<_>>(), [0, 1, 2, 3], "pages with /Trans or /Dur, in order; page 5 has neither");
        assert_eq!((t[0].style, t[0].duration, t[0].direction, t[0].display), (Some(S::Wipe), 2.5, Some(270), Some(5.0)));
        assert!((t[0].horizontal, t[0].inward, t[0].scale, t[0].opaque) == (true, true, 1.0, false), "defaults: {:?}", t[0]);
        assert_eq!((t[1].style, t[1].duration, t[1].display), (None, 1.0, Some(3.0)), "/Dur without /Trans");
        assert_eq!(
            (t[2].style, t[2].inward, t[2].scale, t[2].opaque, t[2].direction, t[2].horizontal),
            (Some(S::Fly), false, 0.5, true, None, false),
            "an indirect /Trans: {:?}",
            t[2]
        );
        // Out of range: an unknown style is the spec's default R, a non-numeric /D the default
        // second, a direction that is not one of the five falls back to 0, a bad /Dm to
        // horizontal, and a negative /Dur is no display time.
        assert_eq!(
            (t[3].style, t[3].duration, t[3].direction, t[3].horizontal, t[3].display),
            (Some(S::Replace), 1.0, Some(0), true, None),
            "{:?}",
            t[3]
        );
    }

    const CUSTOM_INFO: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Title (Report) /Producer (Test) /Project (Apollo) /Revision 7 /Dept (R&D) /Trapped /False /Note <FEFF90E8> >> endobj
trailer << /Root 1 0 R /Info 4 0 R >>
%%EOF";

    #[test]
    fn custom_document_properties_are_listed_apart_from_the_standard_ones() {
        let info = inspect(Arc::new(CUSTOM_INFO.to_vec()), None).expect("opens");
        assert_eq!((info.title.as_deref(), info.producer.as_deref()), (Some("Report"), Some("Test")));
        let mut custom = info.custom.clone();
        custom.sort();
        // Revision is a number, not text, so it is not listed; Trapped is a standard entry.
        assert_eq!(
            custom,
            [("Dept".to_string(), "R&D".to_string()), ("Note".to_string(), "部".to_string()), ("Project".to_string(), "Apollo".to_string())]
        );
    }

    #[test]
    fn radio_button_layer_groups_are_read() {
        let info = inspect(Arc::new(LAYERS.to_vec()), None).expect("opens");
        let layers: Vec<_> = info.layers.iter().map(|l| (l.id, l.name.as_str(), l.visible)).collect();
        assert_eq!(layers, [((5, 0), "Red", true), ((6, 0), "Green", false), ((7, 0), "Blue", true)]);
        // A group of one constrains nothing; an indirect group is read.
        assert_eq!(info.layer_groups, [vec![(5, 0), (6, 0)], vec![(6, 0), (7, 0)]]);
    }

    #[test]
    fn initial_layer_visibility_is_the_default_configuration() {
        // BaseState OFF: Red is listed ON; Green is listed both ON and OFF, and OFF wins; Blue
        // isn't listed, so it takes the base state.
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R 7 0 R] /D << /BaseState /OFF /ON [5 0 R 6 0 R] /OFF [6 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
5 0 obj << /Type /OCG /Name (Red) >> endobj
6 0 obj << /Type /OCG /Name (Green) >> endobj
7 0 obj << /Type /OCG /Name (Blue) >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let info = inspect(Arc::new(pdf.to_vec()), None).expect("opens");
        let layers: Vec<_> = info.layers.iter().map(|l| (l.name.as_str(), l.initial)).collect();
        assert_eq!(layers, [("Red", true), ("Green", false), ("Blue", false)]);
    }

    const ORPHAN_FIELDS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R 6 0 R] >> endobj
4 0 obj << /Fields [] >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (alpha) /V (one) /Rect [10 100 90 120] >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (beta) /Rect [10 60 90 80] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn fields_listed_only_on_the_pages_are_listed() {
        let info = inspect(Arc::new(ORPHAN_FIELDS.to_vec()), None).expect("opens");
        let listed: Vec<_> = info.fields.iter().map(|f| (f.name.as_str(), f.value.as_deref(), f.page)).collect();
        assert_eq!(listed, [("alpha", Some("one"), Some(0)), ("beta", None, Some(0))], "{:?}", info.fields);
    }

    /// A `Structure` that counts the objects read through it.
    struct CountingReads<'a> {
        doc: &'a Document,
        reads: std::cell::Cell<usize>,
    }

    impl Structure for CountingReads<'_> {
        fn trailer(&self) -> &Dictionary {
            &self.doc.trailer
        }
        fn get_object(&self, id: ObjectId) -> lopdf::Result<&Object> {
            self.reads.set(self.reads.get() + 1);
            self.doc.get_object(id)
        }
        fn encrypted(&self) -> bool {
            false
        }
    }

    /// One comment referenced many times, or a page-shared `/Annots` array, is listed once, and many
    /// small comments sharing one large `/Contents` string stop at the retained-bytes budget.
    #[test]
    fn repeated_annotations_are_listed_once_within_a_byte_budget() {
        let small = "<< /Subtype /Text /Rect [0 0 1 1] /Contents 7 0 R >> ".repeat(1200);
        let pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 5 0 R >> endobj\n\
             4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 5 0 R >> endobj\n\
             5 0 obj [{} {small}] endobj\n\
             6 0 obj << /Type /Annot /Subtype /Text /Rect [0 0 1 1] /Contents (note) >> endobj\n\
             7 0 obj ({}) endobj\ntrailer << /Root 1 0 R >>\n%%EOF",
            "6 0 R ".repeat(1000),
            "x".repeat(64 << 10),
        );
        let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
        let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
        let mut info = DocInfo { pages: vec![page.clone(), page], ..Default::default() };
        Inspector::new(&doc).fill(&mut info);
        let notes = &info.annotations;
        assert_eq!(notes.iter().filter(|a| a.contents.as_deref() == Some("note")).count(), 1);
        assert!(notes.iter().all(|a| a.page == 0), "the shared array is the first page's");
        assert!(notes.len() > 1);
        assert!(notes.iter().map(retained).sum::<usize>() <= MAX_ANNOTATION_BYTES);
    }

    /// A link reached through an indirect object that holds only a reference to it is the same
    /// annotation as the link itself, and so is a page array reached the same way: each is listed once.
    #[test]
    fn an_annotation_reached_through_a_reference_chain_is_listed_once() {
        let pdf = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 7 0 R >> endobj\n\
             4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 8 0 R >> endobj\n\
             5 0 obj 6 0 R endobj\n\
             6 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 1 1] /A << /S /URI /URI (https://example.com) >> >> endobj\n\
             7 0 obj [5 0 R 6 0 R] endobj\n\
             8 0 obj 7 0 R endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF";
        let mut doc = Document::load_mem(pdf.as_bytes()).expect("loads");
        let mut saved = Vec::new();
        doc.save_to(&mut saved).expect("saves");
        let lazy = LazyStructure::new(Arc::new(saved), None).expect("opens");
        let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
        for reader in [&doc as &dyn Structure, &lazy] {
            let mut info = DocInfo { pages: vec![page.clone(), page.clone()], ..Default::default() };
            Inspector::new(reader).fill(&mut info);
            assert_eq!(info.links.len(), 1, "{:?}", info.links);
        }
    }

    /// A conversion rejected as too long, or trimmed short or to nothing, still spends the raw
    /// length, so a string many entries share is converted only a few times.
    #[test]
    fn rejected_text_conversions_spend_the_budget() {
        let mut budget = 10;
        let mut conversions = 0;
        for _ in 0..10 {
            // Each 0xFF becomes the three-byte replacement character.
            let kept = spend(&[0xFF; 5], &mut budget, |b| {
                conversions += 1;
                String::from_utf8_lossy(b).into_owned()
            });
            assert_eq!(kept, None);
        }
        assert_eq!((conversions, budget), (2, 0));

        let pdf = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF";
        let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
        let d = Dictionary::from_iter([("Contents", Object::string_literal("   "))]);
        let mut budget = 10;
        assert_eq!(Inspector::new(&doc).budgeted_text(&d, b"Contents", &mut budget), None);
        assert_eq!(budget, 7);
        // Text that trims short spends its raw length.
        let d = Dictionary::from_iter([("Contents", Object::string_literal("      x"))]);
        let mut budget = 10;
        assert_eq!(Inspector::new(&doc).budgeted_text(&d, b"Contents", &mut budget).as_deref(), Some("x"));
        assert_eq!(budget, 3);
    }

    /// An annotation attachment's `/Contents` spends the text budget only when it names the
    /// attachment, so long descriptions of named files leave a later web address listed.
    #[test]
    fn a_named_attachment_leaves_its_contents_unspent() {
        let files = "<< /Subtype /FileAttachment /Rect [0 0 1 1] /Contents 6 0 R /FS << /F (a.txt) >> >> ".repeat(20);
        // Longer than what 20 MiB of charged `/Contents` would leave of the 16 MiB budget.
        let uri = format!("https://example.com/{}", "a".repeat(2 << 20));
        let pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{files} \
             << /Subtype /Link /Rect [0 0 1 1] /A << /S /URI /URI ({uri}) >> >>] >> endobj\n\
             6 0 obj ({}) endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
            "x".repeat(1 << 20),
        );
        let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
        let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
        let mut info = DocInfo { pages: vec![page], ..Default::default() };
        Inspector::new(&doc).fill(&mut info);
        assert!(info.attachments.iter().all(|a| a.name == "a.txt"));
        assert_eq!(info.links.len(), 1);
        assert!(matches!(&info.links[0].target, LinkTarget::Uri(u) if *u == uri));
    }

    /// Embedded files whose name-tree keys share one long string keep it, as key and as name, only
    /// within `MAX_LINK_ATTACHMENT_TEXT_BYTES`.
    #[test]
    fn embedded_file_keys_sharing_a_long_string_keep_it_within_a_budget() {
        let entries = "4 0 R << /Type /Filespec >> ".repeat(20);
        let pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [{entries}] >> >> >> endobj\n\
             2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\n\
             4 0 obj ({}) endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
            "x".repeat(1 << 20),
        );
        let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
        let mut info = DocInfo::default();
        Inspector::new(&doc).fill(&mut info);
        let kept: usize =
            info.attachments.iter().map(|a| a.name.len() + if let AttachmentSource::Document { key } = &a.source { key.len() } else { 0 }).sum();
        assert!(!info.attachments.is_empty());
        // Past the budget an attachment still has the generic name.
        let generic = info.attachments.len() * "attachment".len();
        assert!(kept <= MAX_LINK_ATTACHMENT_TEXT_BYTES + generic, "{kept} bytes kept");
    }

    /// An embedded file the listing keeps is extracted, though earlier keys the listing could not
    /// keep would fill a fresh key budget before it.
    #[test]
    fn a_listed_embedded_file_is_extracted_past_keys_the_listing_left_out() {
        let skipped = "6 0 R 7 0 R ".repeat(9);
        let pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [{skipped}(b.txt) 9 0 R] >> >> >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [\
             << /Subtype /Link /Rect [0 0 1 1] /A << /S /URI /URI ({}) >> >>] >> endobj\n\
             6 0 obj ({}) endobj\n\
             7 0 obj << /Type /Filespec /F (a.txt) >> endobj\n\
             8 0 obj << /Type /EmbeddedFile /Length 5 >> stream\nhello\nendstream endobj\n\
             9 0 obj << /Type /Filespec /F (b.txt) /EF << /F 8 0 R >> >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
            "u".repeat(15 << 20),
            "k".repeat(2 << 20),
        );
        let bytes = Arc::new(pdf.into_bytes());
        let info = inspect(bytes.clone(), None).expect("opens");
        let listed = info.attachments.iter().find(|a| a.name == "b.txt").expect("b.txt is listed");
        assert_eq!(attachment_data(&bytes, None, listed).as_deref(), Ok(&b"hello"[..]));
    }

    /// Links and attachments sharing one long string each keep a copy only within
    /// `MAX_LINK_ATTACHMENT_TEXT_BYTES`, and every one is still listed.
    #[test]
    fn links_and_attachments_sharing_a_long_string_keep_it_within_a_budget() {
        const EACH: usize = 20;
        let links = "<< /Subtype /Link /Rect [0 0 1 1] /A 5 0 R >> ".repeat(EACH);
        let files = "<< /Subtype /FileAttachment /Rect [0 0 1 1] /FS 7 0 R >> ".repeat(EACH);
        let pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{links} {files}] >> endobj\n\
             5 0 obj << /S /URI /URI 6 0 R >> endobj\n\
             6 0 obj ({}) endobj\n\
             7 0 obj << /Type /Filespec /F (a.txt) /Desc 6 0 R >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
            "x".repeat(1 << 20),
        );
        let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
        let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
        let mut info = DocInfo { pages: vec![page], ..Default::default() };
        Inspector::new(&doc).fill(&mut info);
        assert_eq!(info.links.len(), EACH);
        assert_eq!(info.attachments.len(), EACH);
        let uris = info.links.iter().map(|l| if let LinkTarget::Uri(u) = &l.target { u.len() } else { 0 });
        let descriptions = info.attachments.iter().map(|a| a.description.as_ref().map_or(0, String::len));
        let kept: usize = uris.chain(descriptions).sum();
        assert!(kept > 0 && kept <= MAX_LINK_ATTACHMENT_TEXT_BYTES, "{kept} bytes kept");
        assert!(info.links.iter().any(|l| matches!(&l.target, LinkTarget::Other(s) if s == "URI")));
    }

    /// Links and attachments after the comment budget fills are still listed, and a JavaScript link
    /// among them still marks the document.
    #[test]
    fn links_and_attachments_past_the_comment_budget_are_listed() {
        let small = "<< /Subtype /Text /Rect [0 0 1 1] /Contents 6 0 R >> ".repeat(1100);
        let pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{small} \
             << /Subtype /Link /Rect [0 0 1 1] /A << /S /JavaScript /JS (1) >> >>] >> endobj\n\
             4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [\
             << /Subtype /FileAttachment /Rect [0 0 1 1] /FS << /Type /Filespec /F (a.txt) >> >>] >> endobj\n\
             6 0 obj ({}) endobj\ntrailer << /Root 1 0 R >>\n%%EOF",
            "x".repeat(64 << 10),
        );
        let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
        let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
        let mut info = DocInfo { pages: vec![page.clone(), page], ..Default::default() };
        Inspector::new(&doc).fill(&mut info);
        assert!(info.annotations.len() < 1100, "the comment budget filled");
        assert_eq!(info.links.len(), 1);
        assert!(info.has_javascript);
        assert_eq!(info.attachments.len(), 1);
    }

    /// Widgets without `/P` are placed on the page that lists them with one pass over the pages'
    /// `/Annots`, not a search of every page per field: that was O(fields × annotations), a
    /// second to open a form of 8,000 fields. A widget two pages list is on the first of them.
    #[test]
    fn widgets_without_a_page_entry_are_placed_in_one_pass() {
        const PAGES: usize = 100;
        const PER_PAGE: usize = 10;
        let field = |p: usize, k: usize| 4 + PAGES + p * PER_PAGE + k;
        let mut pdf = String::from("%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm 3 0 R >> endobj\n");
        let kids: Vec<String> = (0..PAGES).map(|p| format!("{} 0 R", 4 + p)).collect();
        pdf += &format!("2 0 obj << /Type /Pages /Kids [{}] /Count {PAGES} >> endobj\n", kids.join(" "));
        let fields: Vec<String> = (0..PAGES).flat_map(|p| (0..PER_PAGE).map(move |k| format!("{} 0 R", field(p, k)))).collect();
        pdf += &format!("3 0 obj << /Fields [{}] >> endobj\n", fields.join(" "));
        for p in 0..PAGES {
            let mut annots: Vec<String> = (0..PER_PAGE).map(|k| format!("{} 0 R", field(p, k))).collect();
            // The last page lists the first page's first widget too.
            if p == PAGES - 1 {
                annots.push(format!("{} 0 R", field(0, 0)));
            }
            pdf += &format!("{} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{}] >> endobj\n", 4 + p, annots.join(" "));
        }
        for p in 0..PAGES {
            for k in 0..PER_PAGE {
                pdf += &format!("{} 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (f{p}_{k}) /Rect [10 10 90 30] >> endobj\n", field(p, k));
            }
        }
        pdf += "trailer << /Root 1 0 R >>\n%%EOF";
        let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
        let counting = CountingReads { doc: &doc, reads: std::cell::Cell::new(0) };
        let mut info = DocInfo {
            pages: (0..PAGES)
                .map(|_| PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() })
                .collect(),
            ..Default::default()
        };
        Inspector::new(&counting).fill(&mut info);
        let placed: Vec<_> = info.fields.iter().map(|f| (f.name.clone(), f.page)).collect();
        let want: Vec<_> = (0..PAGES).flat_map(|p| (0..PER_PAGE).map(move |k| (format!("f{p}_{k}"), Some(p)))).collect();
        assert_eq!(placed, want);
        // A handful of reads per page and field; searching the pages for each field read the
        // page dictionaries about 50,000 times.
        let reads = counting.reads.get();
        assert!(reads < 20 * (PAGES + PAGES * PER_PAGE), "{reads} object reads");
    }

    /// #2365: the issue's fixture is two PDFs concatenated, the second cut off before its
    /// cross-reference data, and the first's only trailer is a cross-reference stream. The
    /// lazy reader refuses a file that needed repair, and lopdf's rebuild found no `trailer`
    /// keyword and failed, so Document Properties showed every field empty. The rebuild now
    /// takes the cross-reference stream's dictionary as the trailer.
    #[test]
    fn rebuilt_file_keeps_the_info_of_its_cross_reference_stream() {
        let header = "%PDF-1.5\n";
        let obj1 = "1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n";
        let obj2 = "2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n";
        let obj3 = "3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>endobj\n";
        let obj4 = "4 0 obj<</Title(Kept)/Creator(Me)>>endobj\n";
        let o1 = header.len();
        let o2 = o1 + obj1.len();
        let o3 = o2 + obj2.len();
        let o4 = o3 + obj3.len();
        let o5 = o4 + obj4.len();
        let rows: Vec<u8> =
            [[0u8, 0, 0, 255], [1, 0, o1 as u8, 0], [1, 0, o2 as u8, 0], [1, 0, o3 as u8, 0], [1, 0, o4 as u8, 0], [1, (o5 >> 8) as u8, o5 as u8, 0]]
                .concat();
        let mut bytes = format!("{header}{obj1}{obj2}{obj3}{obj4}").into_bytes();
        bytes.extend_from_slice(format!("5 0 obj<</Type/XRef/Root 1 0 R/Info 4 0 R/Size 6/W[1 2 1]/Length {}>>stream\n", rows.len()).as_bytes());
        bytes.extend_from_slice(&rows);
        // A wrong startxref, and a second file after %%EOF: every reader rebuilds the table.
        bytes.extend_from_slice(b"\nendstream\nendobj\nstartxref\n999999\n%%EOF\n%PDF-1.3\n");
        bytes.extend_from_slice(&[b'x'; 1024]);

        let info = inspect(Arc::new(bytes), None).unwrap();
        assert_eq!(info.pages.len(), 1);
        assert_eq!(info.title.as_deref(), Some("Kept"));
        assert_eq!(info.creator.as_deref(), Some("Me"));
    }

    /// #2365 review: the rebuild prefers a trailer whose /Root is the catalog. A later
    /// cross-reference stream whose /Root points at the Info dictionary used to replace the
    /// earlier `trailer`, so Document Properties lost the catalog it should have read.
    #[test]
    fn rebuilt_file_prefers_a_trailer_whose_root_is_the_catalog() {
        let mut bytes = b"%PDF-1.5\n".to_vec();
        for body in [
            "<</Type/Catalog/Pages 2 0 R>>",
            "<</Type/Pages/Kids[3 0 R]/Count 1>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>",
            "<</Title(Kept)/Creator(Me)>>",
        ] {
            let n = bytes.iter().filter(|b| **b == b'\n').count();
            bytes.extend_from_slice(format!("{n} 0 obj{body}endobj\n").as_bytes());
        }
        bytes.extend_from_slice(b"trailer\n<</Size 5/Root 1 0 R/Info 4 0 R>>\n");
        bytes.extend_from_slice(b"5 0 obj<</Type/XRef/Root 4 0 R/Size 6/W[1 2 1]/Length 4>>stream\n\x01\x00\x09\x00\nendstream\nendobj\n");
        // A wrong startxref, and a second file after %%EOF: every reader rebuilds the table.
        bytes.extend_from_slice(b"startxref\n999999\n%%EOF\n%PDF-1.3\n");
        bytes.extend_from_slice(&[b'x'; 1024]);

        let info = inspect(Arc::new(bytes.clone()), None).unwrap();
        assert_eq!(info.pages.len(), 1);
        assert_eq!(info.title.as_deref(), Some("Kept"));
        // A later stream whose /Root names the catalog's number with another generation does
        // not count either: loading `1 1 R` finds no such object.
        let at = bytes.windows(10).position(|w| w == b"/Root 4 0 ").unwrap();
        bytes[at..at + 10].copy_from_slice(b"/Root 1 1 ");
        let info = inspect(Arc::new(bytes), None).unwrap();
        assert_eq!((info.pages.len(), info.title.as_deref()), (1, Some("Kept")));
    }

    #[test]
    fn lazy_structure_matches_the_compatibility_inspector() {
        // Each fixture as lopdf writes it, and with its objects in object streams.
        let packed = |bytes: &[u8]| {
            let doc = pdfcraft_cos::Document::open(Arc::new(bytes.to_vec())).unwrap();
            pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions { object_streams: true, ..Default::default() }).unwrap()
        };
        let fixtures = [ATTACHMENTS, LAYERS, DESTS].into_iter().flat_map(|bytes| {
            let mut original = Document::load_mem(bytes).unwrap();
            let mut plain = Vec::new();
            original.save_to(&mut plain).unwrap();
            let stm = packed(&plain);
            [plain, stm]
        });
        for bytes in fixtures {
            let lazy = LazyStructure::new(Arc::new(bytes.clone()), None).unwrap();
            let eager = Document::load_mem(&bytes).unwrap();
            let mut a = inspect(Arc::new(bytes), None).unwrap();
            let mut b = DocInfo { pages: a.pages.clone(), ..Default::default() };
            let mut c = DocInfo { pages: a.pages.clone(), ..Default::default() };
            Inspector::new(&lazy).fill(&mut b);
            Inspector::new(&eager).fill(&mut c);
            assert!(!lazy.failed());
            assert_eq!(format!("{b:?}"), format!("{c:?}"));
            a.file_size = 0;
            a.pdf_version.clear();
            assert_eq!(format!("{a:?}"), format!("{c:?}"));
        }
    }

    #[test]
    fn links_read_set_layer_actions() {
        let info = inspect(Arc::new(LAYERS.to_vec()), None).expect("opens");
        let targets: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
        use LayerOp::{Off, On, Toggle};
        assert_eq!(
            targets,
            [
                // Groups before the first name or after an unknown one are skipped, and so is
                // anything that isn't a group.
                LinkTarget::SetLayers { changes: vec![(On, (6, 0)), (Toggle, (5, 0)), (Toggle, (7, 0))], preserve_rb: false },
                // An indirect /State; /PreserveRB defaults to true.
                LinkTarget::SetLayers { changes: vec![(Off, (5, 0))], preserve_rb: true },
            ]
        );
    }

    /// #4168: a Launch action naming a `.pdf` file is read as the link to that file it amounts
    /// to, `\` separators included, and a stray `/D` (which Launch has no use for) doesn't stop
    /// it. One naming anything else, with no file, or with `/Win` launch parameters stays an
    /// action that isn't followed.
    #[test]
    fn launch_actions_that_name_a_pdf_open_it_like_a_remote_link() {
        let pdf = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /Annots [30 0 R 31 0 R 32 0 R 33 0 R 34 0 R 35 0 R 36 0 R 37 0 R 38 0 R] >> endobj\n\
             30 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /Launch /F (..\\\\se\\\\se_ST.pdf) >> >> endobj\n\
             31 0 obj << /Type /Annot /Subtype /Link /Rect [0 20 10 30] /A << /S /Launch /F << /Type /Filespec /F (chapters/Two.PDF) >> /NewWindow true >> >> endobj\n\
             32 0 obj << /Type /Annot /Subtype /Link /Rect [0 40 10 50] /A << /S /Launch /F (tool.exe) >> >> endobj\n\
             33 0 obj << /Type /Annot /Subtype /Link /Rect [0 60 10 70] /A << /S /Launch /F (a.pdf) /Win << /F (cmd.exe) /P (/c x) >> >> >> endobj\n\
             34 0 obj << /Type /Annot /Subtype /Link /Rect [0 80 10 90] /A << /S /Launch /Win << /F (b.pdf) >> >> >> endobj\n\
             35 0 obj << /Type /Annot /Subtype /Link /Rect [0 100 10 110] /A << /S /Launch /F (.pdf) >> >> endobj\n\
             36 0 obj << /Type /Annot /Subtype /Link /Rect [0 120 10 130] /A << /S /Launch /F (dir/a\\\\b.pdf) >> >> endobj\n\
             37 0 obj << /Type /Annot /Subtype /Link /Rect [0 140 10 150] /A << /S /Launch /FS /URL /F (https://example.com/x.pdf) >> >> endobj\n\
             38 0 obj << /Type /Annot /Subtype /Link /Rect [0 160 10 170] /A << /S /Launch /F (c.pdf) /D 0 >> >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF";
        let info = inspect(Arc::new(pdf.as_bytes().to_vec()), None).expect("opens");
        let targets: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
        let launch =
            |file: &str, new_window| LinkTarget::RemoteGoTo { launch: true, file: file.into(), destination: RemoteDestination::Default, new_window };
        let other = || LinkTarget::Other("Launch".into());
        assert_eq!(
            targets,
            [
                launch("../se/se_ST.pdf", None),
                launch("chapters/Two.PDF", Some(true)),
                other(),
                other(),
                other(),
                other(),
                launch("dir/a\\b.pdf", None),
                other(),
                launch("c.pdf", None),
            ]
        );
    }

    #[test]
    fn links_read_bounded_local_remote_go_to_actions() {
        use DestView::{Fit, Xyz};
        let too_long = "a".repeat(MAX_REMOTE_FILE_SPEC_BYTES + 1);
        let wide_file = format!("<FEFF{}>", "4E00".repeat(1400));
        let wide_name = format!("<FEFF{}>", "4E00".repeat(342));
        let pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /Annots [99 0 R 30 0 R 31 0 R 32 0 R 33 0 R 34 0 R 35 0 R 36 0 R 37 0 R 38 0 R 39 0 R 40 0 R 41 0 R] >> endobj\n\
             30 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /GoToR /F (chapters/remote.pdf) /D [2 /XYZ 10 20 1.5] /NewWindow true >> >> endobj\n\
             31 0 obj << /Type /Annot /Subtype /Link /Rect [0 20 10 30] /A << /S /GoToR /F << /Type /Filespec /F (fallback.pdf) /UF <FEFF0066006F006F002E007000640066> >> /D [0 /Fit] /NewWindow false >> >> endobj\n\
             32 0 obj << /Type /Annot /Subtype /Link /Rect [0 40 10 50] /A << /S /GoToR /F (remote.pdf) >> >> endobj\n\
             33 0 obj << /Type /Annot /Subtype /Link /Rect [0 60 10 70] /A << /S /GoToR /F (chapter.pdf) /D (chapter) >> >> endobj\n\
             34 0 obj << /Type /Annot /Subtype /Link /Rect [0 80 10 90] /A << /S /GoToR /FS /URL /F (https://example.com) /D [0 /Fit] >> >> endobj\n\
             35 0 obj << /Type /Annot /Subtype /Link /Rect [0 100 10 110] /A << /S /GoToR /F ({too_long}) /D [0 /Fit] >> >> endobj\n\
             36 0 obj << /Type /Annot /Subtype /Link /Rect [0 120 10 130] /A << /S /GoToR /F (chapter.pdf) /D /chapter >> >> endobj\n\
             37 0 obj << /Type /Annot /Subtype /Link /Rect [0 140 10 150] /A << /S /GoToR /F (chapter.pdf) /D [-1 /Fit] >> >> endobj\n\
             38 0 obj << /Type /Annot /Subtype /Link /Rect [0 160 10 170] /A << /S /GoToR /F (chapter.pdf) /D [1.5 /Fit] >> >> endobj\n\
             39 0 obj << /Type /Annot /Subtype /Link /Rect [0 180 10 190] /A << /S /GoToR /F (chapter.pdf) /D [9223372036854775807 /Fit] >> >> endobj\n\
             40 0 obj << /Type /Annot /Subtype /Link /Rect [0 200 10 210] /A << /S /GoToR /F << /Type /Filespec /UF {wide_file} >> /D [0 /Fit] >> >> endobj\n\
             41 0 obj << /Type /Annot /Subtype /Link /Rect [0 220 10 230] /A << /S /GoToR /F (chapter.pdf) /D {wide_name} >> >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF"
        );
        let info = inspect(Arc::new(pdf.into_bytes()), None).expect("opens");
        let targets: Vec<_> = info.links.iter().map(|l| (l.index, l.target.clone())).collect();
        let huge_page_target = usize::try_from(i64::MAX)
            .map(|page| LinkTarget::RemoteGoTo {
                launch: false,
                file: "chapter.pdf".into(),
                destination: RemoteDestination::Page { page, view: Fit },
                new_window: None,
            })
            .unwrap_or_else(|_| LinkTarget::Other("GoToR".into()));
        assert_eq!(
            targets,
            [
                (
                    1,
                    LinkTarget::RemoteGoTo {
                        launch: false,
                        file: "chapters/remote.pdf".into(),
                        destination: RemoteDestination::Page { page: 2, view: Xyz { left: Some(10.0), top: Some(20.0), zoom: Some(1.5) } },
                        new_window: Some(true),
                    }
                ),
                (
                    2,
                    LinkTarget::RemoteGoTo {
                        launch: false,
                        file: "foo.pdf".into(),
                        destination: RemoteDestination::Page { page: 0, view: Fit },
                        new_window: Some(false),
                    }
                ),
                (3, LinkTarget::RemoteGoTo { launch: false, file: "remote.pdf".into(), destination: RemoteDestination::Default, new_window: None }),
                (
                    4,
                    LinkTarget::RemoteGoTo {
                        launch: false,
                        file: "chapter.pdf".into(),
                        destination: RemoteDestination::Named("chapter".into()),
                        new_window: None
                    }
                ),
                (5, LinkTarget::Other("GoToR".into())),
                (6, LinkTarget::Other("GoToR".into())),
                (
                    7,
                    LinkTarget::RemoteGoTo {
                        launch: false,
                        file: "chapter.pdf".into(),
                        destination: RemoteDestination::Named("chapter".into()),
                        new_window: None
                    }
                ),
                (8, LinkTarget::Other("GoToR".into())),
                (9, LinkTarget::Other("GoToR".into())),
                (10, huge_page_target),
                (11, LinkTarget::Other("GoToR".into())),
                (12, LinkTarget::Other("GoToR".into())),
            ]
        );
    }

    #[test]
    fn destination_operands_become_views() {
        use DestView::*;
        let v = |kind: &[u8], ops: &[Option<f32>]| DestView::from_operands(Some(kind), ops);
        // ISO 32000-2 Table 149, each type.
        assert_eq!(v(b"XYZ", &[Some(0.0), Some(420.0), Some(0.0)]), Xyz { left: Some(0.0), top: Some(420.0), zoom: None });
        assert_eq!(v(b"XYZ", &[Some(72.0), Some(500.0), Some(1.5)]), Xyz { left: Some(72.0), top: Some(500.0), zoom: Some(1.5) });
        assert_eq!(v(b"Fit", &[]), Fit);
        assert_eq!(v(b"FitB", &[]), Fit);
        assert_eq!(v(b"FitH", &[Some(250.0)]), FitH { top: Some(250.0) });
        assert_eq!(v(b"FitBH", &[None]), FitH { top: None });
        assert_eq!(v(b"FitV", &[Some(120.0)]), FitV { left: Some(120.0) });
        assert_eq!(v(b"FitBV", &[Some(5.0)]), FitV { left: Some(5.0) });
        // FitR's corners are normalized.
        assert_eq!(v(b"FitR", &[Some(110.0), Some(220.0), Some(10.0), Some(20.0)]), FitR { rect: [10.0, 20.0, 110.0, 220.0] });
        // Missing operands are null; so are non-finite ones, and a zoom that isn't positive.
        assert_eq!(v(b"XYZ", &[]), Xyz { left: None, top: None, zoom: None });
        assert_eq!(v(b"XYZ", &[Some(f32::NAN), Some(f32::INFINITY), Some(f32::NEG_INFINITY)]), Xyz { left: None, top: None, zoom: None });
        assert_eq!(v(b"XYZ", &[None, Some(-5.0), Some(-1.0)]), Xyz { left: None, top: Some(-5.0), zoom: None });
        assert_eq!(v(b"XYZ", &[None, None, Some(f32::NAN)]), Xyz { left: None, top: None, zoom: None });
        assert_eq!(v(b"FitH", &[Some(f32::NAN)]), FitH { top: None });
        // A FitR without four usable numbers, an unknown type or no type: the top of the page.
        assert_eq!(v(b"FitR", &[Some(1.0), Some(2.0)]), Top);
        assert_eq!(v(b"FitR", &[Some(1.0), Some(2.0), Some(f32::INFINITY), Some(4.0)]), Top);
        assert_eq!(v(b"Zoom", &[Some(3.0)]), Top);
        assert_eq!(v(b"xyz", &[Some(3.0)]), Top);
        assert_eq!(DestView::from_operands(None, &[Some(1.0)]), Top);
    }

    /// Two 300×400 pages. Bookmarks and links on page 1 go to page 2 in every destination form,
    /// named (`/Names /Dests` and the PDF 1.1 `/Dests`) and malformed.
    const DESTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R /Names << /Dests 40 0 R >> /Dests << /old << /D [4 0 R /FitV 120] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [30 0 R 31 0 R 32 0 R 33 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 20 0 R /Count 10 >> endobj
11 0 obj << /Title (xyz) /Parent 10 0 R /Next 12 0 R /Dest [4 0 R /XYZ 0 420 0] >> endobj
12 0 obj << /Title (fith) /Parent 10 0 R /Next 13 0 R /Dest [4 0 R /FitH 250] >> endobj
13 0 obj << /Title (named) /Parent 10 0 R /Next 14 0 R /Dest (sec) >> endobj
14 0 obj << /Title (old named) /Parent 10 0 R /Next 15 0 R /Dest /old >> endobj
15 0 obj << /Title (action) /Parent 10 0 R /Next 16 0 R /A << /S /GoTo /D [4 0 R /FitR 110 220 10 20] >> >> endobj
16 0 obj << /Title (wrong types) /Parent 10 0 R /Next 17 0 R /Dest [4 0 R /XYZ (left) /Top true] >> endobj
17 0 obj << /Title (short) /Parent 10 0 R /Next 18 0 R /Dest [4 0 R /FitR 1 2] >> endobj
18 0 obj << /Title (no type) /Parent 10 0 R /Next 19 0 R /Dest [4 0 R] >> endobj
19 0 obj << /Title (huge) /Parent 10 0 R /Next 20 0 R /Dest [4 0 R /XYZ 1000000000000 -5 -1] >> endobj
20 0 obj << /Title (no page) /Parent 10 0 R /Dest [/XYZ 0 420 0] >> endobj
30 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [4 0 R /XYZ 72 500 1.5] >> endobj
31 0 obj << /Type /Annot /Subtype /Link /Rect [0 20 10 30] /A << /S /GoTo /D (sec) >> >> endobj
32 0 obj << /Type /Annot /Subtype /Link /Rect [0 40 10 50] /Dest [4 0 R /FitBH 100] >> endobj
33 0 obj << /Type /Annot /Subtype /Link /Rect [0 60 10 70] /Dest [4 0 R /Fit] >> endobj
40 0 obj << /Names [(sec) << /D [4 0 R /XYZ null 300 2] >>] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    #[test]
    fn bookmarks_and_links_keep_their_destination_view() {
        use DestView::*;
        let info = inspect(Arc::new(DESTS.to_vec()), None).expect("opens");
        let outline: Vec<_> = info.outline.iter().map(|o| (o.title.as_str(), o.page, o.view)).collect();
        assert_eq!(
            outline,
            [
                ("xyz", Some(1), Xyz { left: Some(0.0), top: Some(420.0), zoom: None }),
                ("fith", Some(1), FitH { top: Some(250.0) }),
                ("named", Some(1), Xyz { left: None, top: Some(300.0), zoom: Some(2.0) }),
                ("old named", Some(1), FitV { left: Some(120.0) }),
                ("action", Some(1), FitR { rect: [10.0, 20.0, 110.0, 220.0] }),
                // Hostile operands fall back without failing the bookmark.
                ("wrong types", Some(1), Xyz { left: None, top: None, zoom: None }),
                ("short", Some(1), Top),
                ("no type", Some(1), Top),
                ("huge", Some(1), Xyz { left: Some(1e12), top: Some(-5.0), zoom: None }),
                ("no page", None, Top),
            ]
        );
        let links: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
        assert_eq!(
            links,
            [
                LinkTarget::Page(1, Xyz { left: Some(72.0), top: Some(500.0), zoom: Some(1.5) }),
                LinkTarget::Page(1, Xyz { left: None, top: Some(300.0), zoom: Some(2.0) }),
                LinkTarget::Page(1, FitH { top: Some(100.0) }),
                LinkTarget::Page(1, Fit),
            ]
        );
    }

    #[test]
    fn outline_titles_decode_every_text_string_encoding() {
        // Issue #142: CJK bookmark titles showed as mojibake or with a stray BOM.
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 16 0 R /Count 6 >> endobj
11 0 obj << /Title <FEFF7B2C4E007AE0> /Parent 10 0 R /Next 12 0 R /Dest [3 0 R /Fit] >> endobj
12 0 obj << /Title <EFBBBFE79BAEE5BD95> /Parent 10 0 R /Next 13 0 R /Dest [3 0 R /Fit] >> endobj
13 0 obj << /Title <EFBBBF41FF42> /Parent 10 0 R /Next 14 0 R /Dest [3 0 R /Fit] >> endobj
14 0 obj << /Title <E6A682E8BFB0> /Parent 10 0 R /Next 15 0 R /Dest [3 0 R /Fit] >> endobj
15 0 obj << /Title <436166E9> /Parent 10 0 R /Next 16 0 R /Dest [3 0 R /Fit] >> endobj
16 0 obj << /Title <FEFFFEFF0041> /Parent 10 0 R /Dest [3 0 R /Fit] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let info = inspect(Arc::new(pdf.to_vec()), None).expect("opens");
        let titles: Vec<_> = info.outline.iter().map(|o| o.title.as_str()).collect();
        // UTF-16BE, UTF-8 with BOM, invalid UTF-8 after a BOM (lossy, not dropped), BOM-less
        // raw UTF-8, PDFDocEncoding Latin-1, and a doubled BOM.
        assert_eq!(titles, ["第一章", "目录", "A\u{FFFD}B", "概述", "Café", "A"]);
    }

    #[test]
    fn xfa_dynamic_render_is_read_from_config_acrobat7_only() {
        let text = xfa_config_dynamic_render;
        let config =
            |inner: &str| format!("<config xmlns=\"http://www.xfa.org/schema/xci/3.0/\"><acrobat><acrobat7>{inner}</acrobat7></acrobat></config>");
        // Designer's style: a line break before each `>`.
        assert_eq!(
            text("<config\n><acrobat\n><acrobat7\n><dynamicRender\n>forbidden</dynamicRender\n></acrobat7\n></acrobat\n></config\n>").as_deref(),
            Some("forbidden")
        );
        assert_eq!(text(&config("<xci:dynamicRender> required </xci:dynamicRender>")).as_deref(), Some("required"));
        // Commented out, in CDATA, or a longer name: not the element.
        assert_eq!(text(&config("<!-- a > b <dynamicRender>forbidden</dynamicRender> -->")), None);
        assert_eq!(text(&config("<![CDATA[ a > b <dynamicRender>forbidden</dynamicRender> ]]>")), None);
        assert_eq!(text(&config("<dynamicRenderX>forbidden</dynamicRenderX>")), None);
        // Self-closing, empty, or holding markup: no value.
        assert_eq!(text(&config("<dynamicRender/>")), None);
        assert_eq!(text(&config("<dynamicRender></dynamicRender>")), None);
        assert_eq!(text(&config("<dynamicRender><b>forbidden</b></dynamicRender>")), None);
        // Closing tags that don't match what they close: malformed, no value (review on #3192).
        assert_eq!(text(&config("<dynamicRender>forbidden</other>")), None);
        assert_eq!(text("<config><acrobat><acrobat7><dynamicRender>forbidden</dynamicRender></acrobat></acrobat7></config>"), None);
        // The same name anywhere but config/acrobat/acrobat7 (form data, another config branch).
        let xdp = "<xdp:xdp><xfa:datasets><xfa:data><dynamicRender>forbidden</dynamicRender></xfa:data></xfa:datasets><config><present><dynamicRender>forbidden</dynamicRender></present></config></xdp:xdp>";
        assert_eq!(text(xdp), None);
        // A whole XDP with the config after the template.
        let xdp = format!("<xdp:xdp><template><subform/></template>{}</xdp:xdp>", config("<dynamicRender>forbidden</dynamicRender>"));
        assert_eq!(text(&xdp).as_deref(), Some("forbidden"));
        // A whole config path inside the form data, before the real config packet, is data: the
        // real packet's value counts (review on #3192).
        let xdp = format!(
            "<?xml version=\"1.0\"?><xdp:xdp><xfa:datasets><xfa:data>{}</xfa:data></xfa:datasets>{}</xdp:xdp>",
            config("<dynamicRender>forbidden</dynamicRender>"),
            config("<dynamicRender>required</dynamicRender>")
        );
        assert_eq!(text(&xdp).as_deref(), Some("required"));
        // Only in the data, or nested in the template: no configuration at all.
        let xdp =
            format!("<xdp:xdp><xfa:datasets><xfa:data>{}</xfa:data></xfa:datasets></xdp:xdp>", config("<dynamicRender>forbidden</dynamicRender>"));
        assert_eq!(text(&xdp), None);
        let xdp = format!("<xdp:xdp><template>{}</template></xdp:xdp>", config("<dynamicRender>forbidden</dynamicRender>"));
        assert_eq!(text(&xdp), None);
        // acrobat7 must be a child of acrobat, not deeper.
        assert_eq!(text("<config><acrobat><x><acrobat7><dynamicRender>forbidden</dynamicRender></acrobat7></x></acrobat></config>"), None);
        // Cut short anywhere: no panic, nothing read.
        let whole = config("<!-- x --><dynamicRender>forbidden</dynamicRender>");
        for end in 0..whole.len() {
            assert_eq!(text(&whole[..end]), None, "cut at {end}");
        }
        assert_eq!(text(&whole).as_deref(), Some("forbidden"));
    }

    #[test]
    fn xfa_config_packets_are_read_in_utf16_too() {
        let xml = "<config><acrobat><acrobat7><dynamicRender>forbidden</dynamicRender></acrobat7></acrobat></config>";
        let le: Vec<u8> = xml.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = xml.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for bytes in [
            le.clone(),
            be.clone(),
            [&[0xFF, 0xFE][..], &le].concat(),
            [&[0xFE, 0xFF][..], &be].concat(),
            [&[0xEF, 0xBB, 0xBF][..], xml.as_bytes()].concat(),
        ] {
            assert_eq!(xfa_packet_text(&bytes), xml);
        }
        // An odd byte or a broken surrogate doesn't stop the scan.
        assert!(xfa_packet_text(&[0xFF, 0xFE, b'<', 0, 0x00, 0xD8, b'x']).starts_with('<'));
    }

    /// #3180: an XFA form with no fields was always taken to be dynamic, so its pages were
    /// replaced by a layout on open, even when its configuration says it is a static form.
    #[test]
    fn an_xfa_form_without_fields_is_static_when_its_config_forbids_dynamic_rendering() {
        let xfa_of = |config: &str, catalog_extra: &str| {
            let bodies = [
                format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} /AcroForm << /Fields [] /XFA [(config) 4 0 R (template) 5 0 R] >> >>"),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_string(),
                "<< /Type /Page /Parent 2 0 R >>".to_string(),
                format!("<< /Length {} >>\nstream\n{config}\nendstream", config.len()),
                "<< /Length 11 >>\nstream\n<template/>\nendstream".to_string(),
            ];
            inspect(Arc::new(pdf_of(&bodies)), None).expect("opens").xfa
        };
        let config = |value: &str| {
            format!(
                r#"<config xmlns="http://www.xfa.org/schema/xci/3.0/"><acrobat><acrobat7><dynamicRender>{value}</dynamicRender></acrobat7></acrobat></config>"#
            )
        };
        assert_eq!(xfa_of(&config("forbidden"), ""), Some(Xfa::Static));
        assert_eq!(xfa_of(&config("required"), ""), Some(Xfa::Dynamic));
        assert_eq!(xfa_of("<config/>", ""), Some(Xfa::Dynamic), "no field and no word from the config: dynamic, as before");
        assert_eq!(xfa_of(&config("forbidden"), "/NeedsRendering true"), Some(Xfa::Dynamic), "/NeedsRendering still wins");
    }

    /// A file whose objects are `bodies` in order (object 1 first), with a cross-reference table.
    fn pdf_of(bodies: &[String]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
        out
    }

    /// A one-page file whose outline is `items` (objects 5, 6, … in order).
    fn outline_file(items: &[String]) -> Vec<u8> {
        let mut bodies = vec![
            "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R >>".to_string(),
            format!("<< /Type /Outlines /First 5 0 R /Count {} >>", items.len()),
        ];
        bodies.extend(items.iter().cloned());
        pdf_of(&bodies)
    }

    /// The titles of `items`, with children in brackets (`Parent[Child]`).
    fn shape(items: &[OutlineItem]) -> Vec<String> {
        items.iter().map(|o| if o.children.is_empty() { o.title.clone() } else { format!("{}[{}]", o.title, shape(&o.children).join(",")) }).collect()
    }

    /// #3193: a bookmark's colour and style (ISO 32000-2 §12.3.3, Tables 151 and 152). `/C` is
    /// three numbers within 0–1 (one outside is kept at the edge); black, the default, and a `/C`
    /// that isn't three numbers read as no colour. `/F` bit 1 is italic, bit 2 bold.
    #[test]
    fn outline_items_carry_their_colour_and_style() {
        let titles = [
            ("Plain", ""),
            ("Red", "/C [1 0 0]"),
            ("Italic", "/F 1"),
            ("Bold", "/F 2"),
            ("Both", "/C [0 0 1] /F 3"),
            ("Black", "/C [0 0 0]"),
            ("Bright", "/C [2 -1 0.5]"),
            ("Short", "/C [1 0]"),
            ("Long", "/C [1 0 0 0]"),
            ("Named", "/C [1 /x 0]"),
            ("Other bits", "/F 4"),
        ];
        let n = titles.len();
        let items: Vec<String> = titles
            .iter()
            .enumerate()
            .map(|(k, (title, extra))| {
                let prev = if k == 0 { String::new() } else { format!(" /Prev {} 0 R", 4 + k) };
                let next = if k + 1 == n { String::new() } else { format!(" /Next {} 0 R", 6 + k) };
                format!("<< /Title ({title}) /Parent 4 0 R{prev}{next} /Dest [3 0 R /Fit] {extra} >>")
            })
            .collect();
        let info = inspect(Arc::new(outline_file(&items)), None).expect("opens");
        let got: Vec<(&str, Option<[f32; 3]>, bool, bool)> = info.outline.iter().map(|o| (o.title.as_str(), o.color, o.italic, o.bold)).collect();
        assert_eq!(
            got,
            [
                ("Plain", None, false, false),
                ("Red", Some([1.0, 0.0, 0.0]), false, false),
                ("Italic", None, true, false),
                ("Bold", None, false, true),
                ("Both", Some([0.0, 0.0, 1.0]), true, true),
                ("Black", None, false, false),
                ("Bright", Some([1.0, 0.0, 0.5]), false, false),
                ("Short", None, false, false),
                ("Long", None, false, false),
                ("Named", None, false, false),
                ("Other bits", None, false, false),
            ]
        );
    }

    #[test]
    fn outline_window_keeps_the_first_bookmarks_and_says_when_more_follow() {
        const COUNT: usize = OUTLINE_WINDOW + 5;
        let items: Vec<String> = (0..COUNT)
            .map(|k| {
                let prev = if k == 0 { String::new() } else { format!(" /Prev {} 0 R", 4 + k) };
                let next = if k + 1 == COUNT { String::new() } else { format!(" /Next {} 0 R", 6 + k) };
                format!("<< /Title (Item {k}) /Parent 4 0 R{prev}{next} /Dest [3 0 R /Fit] >>")
            })
            .collect();
        let info = inspect(Arc::new(outline_file(&items)), None).expect("opens");
        assert_eq!(info.outline.len(), OUTLINE_WINDOW);
        assert_eq!(info.outline.last().map(|o| o.title.as_str()), Some("Item 9999"));
        assert!(info.outline_more);

        let one = inspect(Arc::new(outline_file(&["<< /Title (Only) /Parent 4 0 R /Dest [3 0 R /Fit] >>".into()])), None).expect("opens");
        assert_eq!(shape(&one.outline), ["Only"]);
        assert!(!one.outline_more);
    }

    #[test]
    fn outline_window_nests_no_deeper_than_its_depth_limit() {
        // A chain of 40 bookmarks, each the first child of the one before.
        let items: Vec<String> = (0..40)
            .map(|k| {
                let parent = if k == 0 { 4 } else { 4 + k };
                let first = if k + 1 == 40 { String::new() } else { format!(" /First {} 0 R", 6 + k) };
                format!("<< /Title (Deep {k}) /Parent {parent} 0 R{first} /Dest [3 0 R /Fit] >>")
            })
            .collect();
        let info = inspect(Arc::new(outline_file(&items)), None).expect("opens");
        let mut levels = 0;
        let mut level = info.outline.first();
        while let Some(item) = level {
            levels += 1;
            level = item.children.first();
        }
        assert_eq!(levels, OUTLINE_DEPTH + 1);
        assert!(info.outline_more);
    }

    #[test]
    fn outline_window_ends_cycles_and_broken_links_without_looping() {
        let listed = |items: &[&str]| {
            let items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
            shape(&inspect(Arc::new(outline_file(&items)), None).expect("opens").outline)
        };
        assert_eq!(listed(&["<< /Title (Self) /Parent 4 0 R /Next 5 0 R >>"]), ["Self"]);
        assert_eq!(listed(&["<< /Title (Self) /Parent 4 0 R /First 5 0 R >>"]), ["Self"]);
        assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 6 0 R >>", "<< /Title (B) /Parent 4 0 R /Next 5 0 R >>"]), ["A", "B"]);
        assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /First 6 0 R >>", "<< /Title (B) /Parent 5 0 R /First 5 0 R >>"]), ["A[B]"]);
        assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 99 0 R >>"]), ["A"]);
        assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 6 0 R >>", "42"]), ["A"]);
    }

    #[test]
    fn destination_points_map_to_the_displayed_page() {
        // A crop box away from the origin: x 100..400, y 50..450.
        let mut p = PageInfo { width: 300.0, height: 400.0, label: "1".into(), crop: [100.0, 50.0, 400.0, 450.0], rotation: 0 };
        assert_eq!(p.dest_fraction(Some(100.0), Some(450.0), 0), [Some(0.0), Some(0.0)]);
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.5), Some(0.25)]);
        // Unspecified stays unspecified; off-page and huge values are clamped to the page.
        assert_eq!(p.dest_fraction(None, Some(350.0), 0), [None, Some(0.25)]);
        assert_eq!(p.dest_fraction(Some(-1e30), Some(1e30), 0), [Some(0.0), Some(0.0)]);
        assert_eq!(p.dest_fraction(Some(f32::INFINITY), Some(f32::NAN), 0), [None, None]);
        // /Rotate 90 (clockwise): user y runs left to right across the displayed page, user x
        // top to bottom. A view rotation adds to it.
        p.rotation = 90;
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.75), Some(0.5)]);
        assert_eq!(p.dest_fraction(None, Some(350.0), 0), [Some(0.75), None]);
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 90), [Some(0.5), Some(0.75)]);
        p.rotation = 180;
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.5), Some(0.75)]);
        p.rotation = 270;
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.25), Some(0.5)]);
        assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 90), [Some(0.5), Some(0.25)]);
        // A degenerate crop box never divides by zero.
        let flat = PageInfo { crop: [0.0, 0.0, 0.0, 0.0], rotation: 0, ..p };
        assert!(flat.dest_fraction(Some(5.0), Some(-5.0), 0).iter().all(|f| f.is_some_and(|f| (0.0..=1.0).contains(&f))));
    }

    #[test]
    fn a_displayed_point_becomes_the_xyz_position_that_goes_back_to_it() {
        // Every rotation, on a crop box away from the origin: pinning a displayed point and
        // reading the destination back lands on the same fraction of the displayed page.
        for rotation in [0u16, 90, 180, 270] {
            let (width, height) = if rotation.is_multiple_of(180) { (300.0, 400.0) } else { (400.0, 300.0) };
            let p = PageInfo { width, height, label: "1".into(), crop: [100.0, 50.0, 400.0, 450.0], rotation };
            let (x, y) = (0.25 * width, 0.75 * height);
            let near = |got: [Option<f32>; 2], want: [Option<f32>; 2]| {
                for (g, w) in got.iter().zip(want) {
                    match (g, w) {
                        (Some(g), Some(w)) => assert!((g - w).abs() < 1e-4, "{rotation}: {got:?} vs {want:?}"),
                        (None, None) => {}
                        _ => panic!("{rotation}: {got:?} vs {want:?}"),
                    }
                }
            };
            let [l, t] = p.xyz_for_view(Some(x), Some(y));
            assert!(l.is_some() && t.is_some(), "{rotation}: a point pins both");
            near(p.dest_fraction(l, t, 0), [Some(0.25), Some(0.75)]);
            // Only a height: the horizontal position is left alone, whichever user axis that is.
            let [l, t] = p.xyz_for_view(None, Some(y));
            assert_eq!((l.is_some(), t.is_some()), if rotation.is_multiple_of(180) { (false, true) } else { (true, false) }, "{rotation}");
            near(p.dest_fraction(l, t, 0), [None, Some(0.75)]);
            let [l, t] = p.xyz_for_view(Some(x), None);
            near(p.dest_fraction(l, t, 0), [Some(0.25), None]);
            assert_eq!(p.xyz_for_view(None, None), [None, None]);
            assert_eq!(p.xyz_for_view(Some(f32::NAN), Some(f32::INFINITY)), [None, None], "not finite is not given");
        }
        // The top of an upright page is the crop box's top edge.
        let p = PageInfo { width: 300.0, height: 400.0, label: "1".into(), crop: [100.0, 50.0, 400.0, 450.0], rotation: 0 };
        assert_eq!(p.xyz_for_view(None, Some(0.0)), [None, Some(450.0)]);
        assert_eq!(p.xyz_for_view(None, Some(100.0)), [None, Some(350.0)]);
    }

    #[test]
    fn structure_failure_preserves_renderer_pages() {
        for crash in [true, false] {
            let mut info = DocInfo {
                file_size: 321,
                pdf_version: "1.7".into(),
                pages: vec![PageInfo { width: 300.0, height: 200.0, label: "1".into(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 90 }],
                ..Default::default()
            };
            inspect_structure(&mut info, |tmp| {
                tmp.pages.clear();
                tmp.title = Some("partially inspected".into());
                if crash {
                    panic!("synthetic structure inspection failure");
                }
                Err("synthetic structure inspection failure".into())
            });

            assert_eq!(info.pages.len(), 1);
            let page = &info.pages[0];
            assert_eq!((page.width, page.height, page.label.as_str(), page.crop, page.rotation), (300.0, 200.0, "1", [10.0, 20.0, 210.0, 320.0], 90));
            assert_eq!(info.file_size, 321);
            assert_eq!(info.pdf_version, "1.7");
            assert!(info.title.is_none());
            assert_eq!(info.warnings.len(), 1);
            assert!(info.warnings[0].contains("synthetic structure inspection failure"));
            assert_eq!(info.warnings[0].contains("crashed"), crash);
        }
    }

    #[test]
    fn invalid_dates_with_unicode_are_unchanged() {
        for input in ["", "D:", "D:20260930104", "D:202609x01045", "yesterday"] {
            assert_eq!(pretty_date(input), input);
        }
        // Cover every position before the 12-byte prefix, including characters that
        // straddle its end. None of these strings is an ASCII PDF date.
        for character in ['é', '€', '😀'] {
            for prefix_len in 0..12 {
                let input = format!("D:{}{character}123456789012", "1".repeat(prefix_len));
                assert_eq!(pretty_date(&input), input, "{character} after {prefix_len} digits");
            }
        }
    }

    #[test]
    fn unicode_annotation_date_does_not_prevent_opening() {
        use lopdf::dictionary;

        let date = "D:12345678901éX";
        let mut encoded_date = vec![0xFE, 0xFF];
        encoded_date.extend(date.encode_utf16().flat_map(u16::to_be_bytes));
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let annot_id = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![10.into(), 10.into(), 30.into(), 30.into()],
            "M" => Object::String(encoded_date, lopdf::StringFormat::Hexadecimal),
        });
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
            "Annots" => vec![annot_id.into()],
        });
        doc.objects.insert(
            pages_id,
            dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }
            .into(),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("write synthetic fixture");

        let info = inspect(Arc::new(bytes), None).expect("opens despite a non-date /M string");
        assert_eq!(info.pages.len(), 1);
        assert_eq!((info.pages[0].width, info.pages[0].height), (200.0, 300.0));
        assert_eq!(info.annotations.len(), 1);
        assert_eq!(info.annotations[0].modified.as_deref(), Some(date));
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    }

    #[test]
    fn carriage_returns_in_comment_text_become_line_breaks() {
        use lopdf::dictionary;

        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let annot_id = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![10.into(), 10.into(), 30.into(), 30.into()],
            // Acrobat ends lines in /Contents with CR; the interface only breaks lines at LF.
            "Contents" => Object::string_literal("one\r\rtwo\r\nthree"),
        });
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
            "Annots" => vec![annot_id.into()],
        });
        doc.objects.insert(pages_id, dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }.into());
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("write synthetic fixture");

        let info = inspect(Arc::new(bytes), None).expect("opens");
        assert_eq!(info.annotations[0].contents.as_deref(), Some("one\n\ntwo\nthree"));
    }

    #[test]
    fn dates_are_prettified() {
        assert_eq!(pretty_date("D:20260930104512-04'00'"), "2026-09-30 10:45");
        assert_eq!(pretty_date("yesterday"), "yesterday");
    }

    #[test]
    fn jspdf_high_precision_mediabox_does_not_overflow_dimensions() {
        let pdf = b"%PDF-1.3
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 3874.9606299212600788 5493.5433070866147318] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
        let info = inspect(std::sync::Arc::new(pdf.to_vec()), None).expect("opens");
        assert_eq!(info.pages.len(), 1);
        let p = &info.pages[0];
        assert!((p.width - 3874.96).abs() < 0.1, "width expected ~3874.96, got {}", p.width);
        assert!((p.height - 5493.54).abs() < 0.1, "height expected ~5493.54, got {}", p.height);
    }
}

/// "1.7" from the parser's version name ("Pdf17", "V1_7", …).
fn version_label(name: &str) -> String {
    let digits: Vec<char> = name.chars().filter(char::is_ascii_digit).collect();
    match digits[..] {
        [a, b] => format!("{a}.{b}"),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod version_tests {
    #[test]
    fn version_labels_read_as_numbers() {
        assert_eq!(super::version_label("Pdf17"), "1.7");
        assert_eq!(super::version_label("V2_0"), "2.0");
        assert_eq!(super::version_label("Unknown"), "Unknown");
    }
}

#[cfg(test)]
mod substitution_tests {
    use std::sync::Arc;

    use super::{FontInfo, inspect};

    /// One page naming four fonts the document does not embed: a Chinese CID font (Adobe-GB1), a
    /// Japanese one (Adobe-Japan1), an unknown serif-flagged simple font, and an aliased
    /// standard-14 name.
    const NOT_EMBEDDED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 4 0 R /F2 6 0 R /F3 8 0 R /F4 9 0 R /F5 11 0 R >> >> >> endobj
4 0 obj << /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [5 0 R] >> endobj
5 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> >> endobj
6 0 obj << /Type /Font /Subtype /Type0 /BaseFont /KozMinPro-Regular /Encoding /UniJIS-UCS2-H /DescendantFonts [7 0 R] >> endobj
7 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /KozMinPro-Regular /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 4 >> >> endobj
8 0 obj << /Type /Font /Subtype /TrueType /BaseFont /SomeHouseSerif /FontDescriptor 10 0 R >> endobj
9 0 obj << /Type /Font /Subtype /TrueType /BaseFont /ArialMT >> endobj
10 0 obj << /Type /FontDescriptor /FontName /SomeHouseSerif /Flags 34 >> endobj
11 0 obj << /Type /Font /Subtype /TrueType /BaseFont /HouseGrotesk /FontDescriptor 12 0 R >> endobj
12 0 obj << /Type /FontDescriptor /FontName /HouseGrotesk /Flags 33 >> endobj
trailer << /Size 13 /Root 1 0 R >>
%%EOF";

    fn font<'a>(fonts: &'a [FontInfo], name: &str) -> &'a FontInfo {
        fonts.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name} listed; got {:?}", fonts.iter().map(|f| &f.name).collect::<Vec<_>>()))
    }

    #[test]
    fn doc_info_says_which_face_stands_in_and_why() {
        let info = inspect(Arc::new(NOT_EMBEDDED.to_vec()), None).expect("opens");

        // The one that matters most to a user: a Chinese PDF that draws nothing. Before this,
        // doc_info said only `embedded: false` and left them to guess.
        let cjk = font(&info.fonts, "STSong-Light");
        assert!(!cjk.embedded);
        assert_eq!(cjk.substitute, None, "no licensed Chinese face, so nothing stands in");
        let why = cjk.substitution.as_deref().unwrap_or_default();
        assert!(why.contains("no licensed face") && why.contains("rather than the wrong glyphs"), "{why}");

        // An aliased standard-14 name resolves to the real face, whose metrics are exact.
        let arial = font(&info.fonts, "ArialMT");
        assert_eq!(arial.substitute.as_deref(), Some("Helvetica"));
        assert!(arial.substitution.as_deref().unwrap_or_default().contains("standard 14"));

        // A house font whose name merely contains "Serif" gets Times -- reported as the guess it
        // is, not as a standard-14 name, which would be untrue of a font nobody has heard of.
        let serif = font(&info.fonts, "SomeHouseSerif");
        assert_eq!(serif.substitute.as_deref(), Some("Times-Roman"));
        assert!(serif.substitution.as_deref().unwrap_or_default().contains("hints at one"), "{serif:?}");

        // No hint in the name at all, so the descriptor flags decide: 33 = fixed pitch + nonsymbolic.
        let grotesk = font(&info.fonts, "HouseGrotesk");
        assert_eq!(grotesk.substitute.as_deref(), Some("Courier"));
        assert!(grotesk.substitution.as_deref().unwrap_or_default().contains("descriptor flags"), "{grotesk:?}");

        // Japanese depends on the optional craft-fonts build input; either way it is reported,
        // and never as a Latin face.
        let jpan = font(&info.fonts, "KozMinPro-Regular");
        assert!(jpan.substitution.is_some(), "a substitution is always explained");
        match &jpan.substitute {
            Some(face) => {
                assert!(!face.starts_with("Helvetica") && !face.starts_with("Times"), "a Latin face is never a Japanese substitute: {face}")
            }
            None => assert!(jpan.substitution.as_deref().unwrap_or_default().contains("no licensed face")),
        }

        // Nothing claims to be embedded, so every font here carries a reason.
        assert!(info.fonts.iter().all(|f| !f.embedded && f.substitution.is_some()), "{:?}", info.fonts);
    }

    /// An embedded font is reported as faithful: nothing stands in, and there is nothing to explain.
    const EMBEDDED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 4 0 R >> >> >> endobj
4 0 obj << /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+SomeHouseSans /FontDescriptor 5 0 R >> endobj
5 0 obj << /Type /FontDescriptor /FontName /ABCDEF+SomeHouseSans /Flags 32 /FontFile2 6 0 R >> endobj
6 0 obj << /Length 4 >> stream
test
endstream endobj
trailer << /Size 7 /Root 1 0 R >>
%%EOF";

    #[test]
    fn an_embedded_font_reports_no_substitution_at_all() {
        let info = inspect(Arc::new(EMBEDDED.to_vec()), None).expect("opens");
        let f = font(&info.fonts, "SomeHouseSans");
        assert!(f.embedded && f.subset, "{f:?}");
        assert_eq!((f.substitute.as_deref(), f.substitution.as_deref()), (None, None), "the document's own glyphs are shown");
    }
}

#[cfg(test)]
mod open_action_javascript_tests {
    use std::sync::Arc;

    use super::inspect;

    /// One-page PDF whose catalog points its /OpenAction at object 4 (written by the caller).
    fn pdf_with_open_action(action: &[u8]) -> Vec<u8> {
        let objs: Vec<&[u8]> = vec![
            b"<< /Type /Catalog /Pages 2 0 R /OpenAction 4 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
            action,
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    #[test]
    fn open_action_javascript_sets_has_javascript() {
        let bytes = pdf_with_open_action(b"<< /S /JavaScript /JS (app.alert('hi')) >>");
        let info = inspect(Arc::new(bytes), None).expect("inspects");
        assert!(info.has_javascript, "an /OpenAction /JavaScript is JavaScript");
    }

    #[test]
    fn open_action_goto_does_not_set_has_javascript() {
        let bytes = pdf_with_open_action(b"<< /S /GoTo /D [3 0 R /Fit] >>");
        let info = inspect(Arc::new(bytes), None).expect("inspects");
        assert!(!info.has_javascript, "a /GoTo /OpenAction is not JavaScript");
    }
}

#[cfg(test)]
mod javascript_links_tests {
    use super::*;
    use lopdf::dictionary;

    #[test]
    fn shared_javascript_actions_have_a_document_wide_source_budget() {
        let mut d = Document::with_version("1.7");
        let pages = d.new_object_id();
        let page = d.new_object_id();
        let action = d.add_object(dictionary! { "S" => "JavaScript", "JS" => Object::string_literal(vec![b'x'; 256 << 10]) });
        // Distinct annotations share the action: repeated references to the same annotation
        // are intentionally deduplicated by inspection before the source budget is charged.
        let mut annots: Vec<_> = (0..33)
            .map(|_| {
                Object::Reference(d.add_object(
                    dictionary! { "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![0.into(),0.into(),50.into(),50.into()], "A" => action },
                ))
            })
            .collect();
        let ordinary = d.add_object(dictionary! { "Subtype" => "Link", "Rect" => vec![0.into(),0.into(),50.into(),50.into()], "Dest" => vec![Object::Reference(page), Object::Name(b"Fit".to_vec())] });
        annots.push(Object::Reference(ordinary));
        d.objects.insert(page, dictionary! { "Type" => "Page", "Parent" => pages, "Annots" => annots }.into());
        d.objects.insert(
            pages,
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1, "MediaBox" => vec![0.into(),0.into(),200.into(),200.into()] }
                .into(),
        );
        let root = d.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        d.trailer.set("Root", root);
        let mut bytes = Vec::new();
        d.save_to(&mut bytes).unwrap();
        let info = inspect(Arc::new(bytes), None).unwrap();
        assert_eq!(info.links.len(), 34, "the resource limit must not drop annotations");
        assert!(info.has_javascript);
        let total: usize = info
            .links
            .iter()
            .map(|link| match &link.target {
                LinkTarget::JavaScript(source) => source.len(),
                _ => 0,
            })
            .sum();
        assert!(total <= 4 << 20, "shared action sources expanded to {total} bytes");
        assert!(matches!(&info.links[0].target, LinkTarget::JavaScript(_)));
        assert!(matches!(&info.links[32].target, LinkTarget::Other(kind) if kind == "JavaScript"));
        assert!(matches!(&info.links[33].target, LinkTarget::Page(0, _)), "ordinary links after the limit must still resolve");
        assert_eq!(info.warnings.iter().filter(|warning| warning.contains("4 MiB")).count(), 1);
    }

    #[test]
    fn javascript_link_sources_direct_indirect_stream_and_utf16() {
        for storage in 0..7 {
            let mut d = Document::with_version("1.7");
            let pages = d.new_object_id();
            let source = "app.alert('é');";
            let string = if storage == 3 {
                {
                    let mut bytes = vec![0xfe, 0xff];
                    for unit in source.encode_utf16() {
                        bytes.extend_from_slice(&unit.to_be_bytes());
                    }
                    Object::String(bytes, lopdf::StringFormat::Hexadecimal)
                }
            } else {
                Object::string_literal(source)
            };
            let js = match storage {
                1 => Object::Reference(d.add_object(string)),
                2 => {
                    let mut stream = lopdf::Stream::new(dictionary! {}, source.as_bytes().to_vec());
                    stream.compress().unwrap();
                    Object::Reference(d.add_object(stream))
                }
                4 => {
                    let cycle = d.new_object_id();
                    d.objects.insert(cycle, Object::Reference(cycle));
                    Object::Reference(cycle)
                }
                5 => {
                    let mut stream = lopdf::Stream::new(dictionary! {}, vec![b'x'; 300 << 10]);
                    stream.compress().unwrap();
                    Object::Reference(d.add_object(stream))
                }
                6 => Object::Integer(42),
                _ => string,
            };
            let action = d.add_object(dictionary! { "S" => "JavaScript", "JS" => js });
            let annotation = d.add_object(
                dictionary! { "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![0.into(),0.into(),50.into(),50.into()], "A" => action },
            );
            let page = d.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "Annots" => vec![annotation.into()] });
            d.objects.insert(pages, dictionary!{ "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1, "MediaBox" => vec![0.into(),0.into(),200.into(),200.into()] }.into());
            let root = d.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
            d.trailer.set("Root", root);
            let mut bytes = Vec::new();
            d.save_to(&mut bytes).unwrap();
            let info = inspect(Arc::new(bytes), None).unwrap();
            assert!(info.has_javascript);
            if storage >= 4 {
                assert!(matches!(&info.links[0].target, LinkTarget::Other(kind) if kind == "JavaScript"));
                continue;
            }
            let LinkTarget::JavaScript(script) = &info.links[0].target else { panic!("missing source") };
            assert_eq!(script, source);
        }
    }
}

#[cfg(test)]
thread_local! { static JS_DECODE_COUNT: std::cell::Cell<usize> = const {std::cell::Cell::new(0)}; }

#[cfg(test)]
mod remediation_probes {
    use super::*;
    use lopdf::dictionary;
    #[test]
    fn remediation_rejected_stream_cache_has_a_fixed_bound() {
        let streams: Vec<_> = (0..8192).map(|_| lopdf::Stream::new(dictionary! {}, vec![])).collect();
        let mut cache = RejectedJavaScript::default();
        for stream in &streams {
            cache.insert(std::ptr::from_ref(stream));
            assert!(cache.identities.len() <= 4096);
            assert!(cache.order.len() <= 4096);
        }
        assert!(!cache.contains(std::ptr::from_ref(streams.first().unwrap())));
        assert!(cache.contains(std::ptr::from_ref(streams.last().unwrap())));
    }
    fn shared_bad_stream(n: usize, repeated_annotation: bool) -> Vec<u8> {
        let mut d = Document::with_version("1.7");
        let pages = d.new_object_id();
        let mut stream = lopdf::Stream::new(dictionary! {}, vec![b'x'; 300 << 10]);
        stream.compress().unwrap();
        let stream_id = d.add_object(stream);
        let action = d.add_object(dictionary! {"S"=>"JavaScript","JS"=>stream_id});
        let mut annots = Vec::new();
        let mut first = None;
        for _ in 0..n {
            let annotation_action = if n == 33 {
                let alias = d.add_object(Object::Reference(stream_id));
                d.add_object(dictionary! {"S"=>"JavaScript","JS"=>alias})
            } else {
                action
            };
            let id = if let Some(id) = first.filter(|_| repeated_annotation) {
                id
            } else {
                let id = d.add_object(
                    dictionary! {"Type"=>"Annot","Subtype"=>"Link","Rect"=>vec![0.into(),0.into(),50.into(),50.into()],"A"=>annotation_action},
                );
                first = Some(id);
                id
            };
            annots.push(Object::Reference(id));
        }
        let good=d.add_object(dictionary!{"Subtype"=>"Link","Rect"=>vec![0.into(),0.into(),50.into(),50.into()],"A"=>dictionary!{"S"=>"JavaScript","JS"=>Object::string_literal("console.println('valid');")}});
        annots.push(good.into());
        let page = d.add_object(dictionary! {"Type"=>"Page","Parent"=>pages,"Annots"=>annots});
        d.objects.insert(
            pages,
            dictionary! {"Type"=>"Pages","Kids"=>vec![page.into()],"Count"=>1,"MediaBox"=>vec![0.into(),0.into(),200.into(),200.into()]}.into(),
        );
        let root = d.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
        d.trailer.set("Root", root);
        let mut bytes = Vec::new();
        d.save_to(&mut bytes).unwrap();
        bytes
    }
    #[test]
    fn remediation_rejected_stream_decode_is_cached_and_preserves_budget() {
        for (n, repeated) in [(1, false), (33, false), (256, false), (1024, false), (1024, true)] {
            let bytes = shared_bad_stream(n, repeated);
            let len = bytes.len();

            let mut times = Vec::new();
            let mut calls = 0;
            for _ in 0..3 {
                JS_DECODE_COUNT.with(|n| n.set(0));
                let now = std::time::Instant::now();
                let info = inspect(Arc::new(bytes.clone()), None).unwrap();
                times.push(now.elapsed().as_micros());
                calls = JS_DECODE_COUNT.with(|n| n.get());
                assert_eq!(calls, 1);
                assert_eq!(info.links.len(), if repeated { 2 } else { n + 1 });
                assert!(matches!(&info.links.last().unwrap().target,LinkTarget::JavaScript(s) if s=="console.println('valid');"));
                assert!(!info.warnings.iter().any(|w| w.contains("4 MiB")));
            }
            println!("REJECT n={n} repeated_annotation={repeated} bytes={len} decode_calls={calls} micros={times:?}");
        }
    }
}

#[cfg(test)]
mod open_dest_tests {
    use std::sync::Arc;

    use super::{DestView, inspect};

    /// Three 612×792 pages (objects 3, 4, 5); `catalog` is added to the catalog dictionary and
    /// `extra` become objects 6, 7, ….
    fn pdf(catalog: &str, extra: &[String]) -> Vec<u8> {
        let mut objs =
            vec![format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"), "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_string()];
        for _ in 0..3 {
            objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string());
        }
        objs.extend(extra.iter().cloned());
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn open_dest(catalog: &str, extra: &[String]) -> Option<(usize, DestView)> {
        inspect(Arc::new(pdf(catalog, extra)), None).expect("inspects").open_dest
    }

    const XYZ: DestView = DestView::Xyz { left: Some(100.0), top: Some(500.0), zoom: Some(2.0) };

    #[test]
    fn an_explicit_destination_opens_at_its_page_and_position() {
        assert_eq!(open_dest("/OpenAction [5 0 R /XYZ 100 500 2]", &[]), Some((2, XYZ)));
        assert_eq!(open_dest("/OpenAction [4 0 R /FitH 400]", &[]), Some((1, DestView::FitH { top: Some(400.0) })));
        // Indirect, and a zoom of 0 keeps the zoom (Table 149).
        assert_eq!(
            open_dest("/OpenAction 6 0 R", &["[4 0 R /XYZ 0 792 0]".into()]),
            Some((1, DestView::Xyz { left: Some(0.0), top: Some(792.0), zoom: None }))
        );
    }

    #[test]
    fn a_go_to_action_opens_at_its_destination_explicit_or_named() {
        assert_eq!(open_dest("/OpenAction << /S /GoTo /D [5 0 R /XYZ 100 500 2] >>", &[]), Some((2, XYZ)));
        // A name, through the PDF 1.1 catalog /Dests dictionary (§12.3.2.4).
        assert_eq!(open_dest("/OpenAction << /S /GoTo /D /there >> /Dests << /there [5 0 R /XYZ 100 500 2] >>", &[]), Some((2, XYZ)));
        // A byte string, through the /Names /Dests name tree, whose value is a dictionary with /D.
        assert_eq!(
            open_dest(
                "/OpenAction << /S /GoTo /D (there) >> /Names << /Dests 6 0 R >>",
                &["<< /Kids [7 0 R] >>".into(), "<< /Names [(here) [3 0 R /Fit] (there) << /D [5 0 R /XYZ 100 500 2] >>] >>".into()]
            ),
            Some((2, XYZ))
        );
    }

    #[test]
    fn an_open_action_given_as_a_name_or_string_is_looked_up() {
        assert_eq!(open_dest("/OpenAction /there /Dests << /there [4 0 R /Fit] >>", &[]), Some((1, DestView::Fit)));
        assert_eq!(
            open_dest("/OpenAction (there) /Names << /Dests << /Names [(there) [5 0 R /FitV 30]] >> >>", &[]),
            Some((2, DestView::FitV { left: Some(30.0) }))
        );
    }

    #[test]
    fn an_open_action_that_goes_nowhere_here_has_no_destination() {
        for catalog in [
            "",
            "/OpenAction << /S /JavaScript /JS (app.alert(1)) >>",
            // A remote go-to's /D is a page of another file.
            "/OpenAction << /S /GoToR /F (other.pdf) /D [1 /Fit] >>",
            "/OpenAction << /D [4 0 R /Fit] >>",
            // An unknown name, and no name dictionaries at all.
            "/OpenAction << /S /GoTo /D /missing >> /Dests << /there [4 0 R /Fit] >>",
            "/OpenAction /missing",
            // A page number past the end, and a page reference that is no page.
            "/OpenAction [7 /Fit]",
            "/OpenAction [2 0 R /Fit]",
            "/OpenAction 42",
        ] {
            assert_eq!(open_dest(catalog, &[]), None, "{catalog}");
        }
    }

    #[test]
    fn cyclic_and_deep_named_destinations_end() {
        // Names that refer to each other.
        assert_eq!(open_dest("/OpenAction /a /Dests << /a /b /b /a >>", &[]), None);
        assert_eq!(open_dest("/OpenAction /a /Dests << /a << /D /a >> >>", &[]), None);
        // A name tree whose node is its own kid.
        assert_eq!(open_dest("/OpenAction (x) /Names << /Dests 6 0 R >>", &["<< /Kids [6 0 R] >>".into()]), None);
        // A name tree deeper than the depth bound: the leaf past it isn't read.
        let depth = 40;
        let mut nodes: Vec<String> = (0..depth).map(|i| format!("<< /Kids [{} 0 R] >>", 7 + i)).collect();
        nodes.push("<< /Names [(x) [4 0 R /Fit]] >>".into());
        assert_eq!(open_dest("/OpenAction (x) /Names << /Dests 6 0 R >>", &nodes), None);
        // A shallow one is read.
        let shallow = ["<< /Kids [7 0 R] >>".to_string(), "<< /Kids [8 0 R] >>".into(), "<< /Names [(x) [4 0 R /Fit]] >>".into()];
        assert_eq!(open_dest("/OpenAction (x) /Names << /Dests 6 0 R >>", &shallow), Some((1, DestView::Fit)));
    }
}
