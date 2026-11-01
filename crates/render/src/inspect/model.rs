use super::HashMap;

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
    /// `/NM` of this annotation, and of the one it replies to (`/IRT`).
    pub name: Option<String>,
    pub in_reply_to: Option<String>,
    /// The page and `/Annots` position of the comment `/IRT` points at, found by object
    /// reference. Threads follow this rather than `/NM`, which need not be unique: PyMuPDF
    /// names the first comment on every page `fitz-A0`. `None` when `/IRT` leads to no listed
    /// comment ([`link_replies`]).
    pub parent: Option<(usize, usize)>,
    /// `/RT /Group`: grouped with the annotation `/IRT` points at (Replace Text's caret with its
    /// strikeout) rather than a reply to it.
    pub grouped: bool,
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
    /// The points that shape a line (its two ends, `/L`), connected lines, a polygon or a cloud
    /// (their corners, `/Vertices`), or a callout (its arrow point and knee: `/CL` without its
    /// end, which follows the text box), in user space; empty for other comments.
    pub points: Vec<[f32; 2]>,
    /// A callout's text box (`/Rect` inset by `/RD`), in user space; `None` for other comments.
    pub text_box: Option<[f32; 4]>,
}

impl Annotation {
    /// A checkmark reply (`/StateModel /Marked`): "Marked" or "Unmarked" rather than a review status.
    pub fn is_mark(&self) -> bool {
        matches!(self.state.as_deref(), Some("Marked" | "Unmarked"))
    }

    /// A reply (or status reply) to another comment rather than a comment of its own.
    pub fn is_reply(&self) -> bool {
        self.parent.is_some()
    }

    /// A comment of its own: not a reply, and not a status or checkmark. One whose comment can't
    /// be found (deleted from another page, or never listed) has nothing to show, so it is listed
    /// nowhere: not as a card, on the page or in a count.
    pub fn is_comment(&self) -> bool {
        !self.is_reply() && self.state.is_none()
    }

    /// Whether this annotation's `/IRT` points at `other`.
    pub fn replies_to(&self, other: &Annotation) -> bool {
        self.parent == Some((other.page, other.index))
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

/// Settle the reply links of a document's annotations: a reply keeps its `parent` only when
/// following the links reaches a comment of its own within 64 steps. An `/IRT` to an unlisted
/// object, a pop-up, the annotation itself or a loop leaves it a comment of its own, so it stays
/// in view instead of hanging off nothing. Both comment readers (inspect, and the engine's list
/// after an edit) run this.
///
/// Only comments can be parents: both readers leave links, form widgets, pop-ups and other
/// non-markup annotations out of `annotations` (`pdfcraft_annot::is_comment_subtype`), so an
/// `/IRT` to one of those finds no parent here. Pop-ups are skipped again in case a caller's list
/// has them.
pub fn link_replies(annotations: &mut [Annotation]) {
    let at: HashMap<(usize, usize), usize> =
        annotations.iter().enumerate().filter(|(_, a)| a.subtype != "Popup").map(|(i, a)| ((a.page, a.index), i)).collect();
    let links: Vec<Option<usize>> =
        annotations.iter().enumerate().map(|(i, a)| a.parent.and_then(|p| at.get(&p).copied()).filter(|&j| j != i)).collect();
    for (i, a) in annotations.iter_mut().enumerate() {
        let mut up = links.get(i).copied().flatten();
        let mut settled = false;
        for _ in 0..64 {
            let Some(j) = up else { break };
            up = links.get(j).copied().flatten();
            if up.is_none() {
                settled = true;
                break;
            }
        }
        if !settled {
            a.parent = None;
        }
    }
}

/// The comment each reply's thread starts from, by the reply's page and `/Annots` position: a
/// reply to a reply files under that comment too. Expects links [`link_replies`] settled.
pub fn thread_tops(annotations: &[Annotation]) -> HashMap<(usize, usize), (usize, usize)> {
    let parents: HashMap<(usize, usize), (usize, usize)> = annotations.iter().filter_map(|a| Some(((a.page, a.index), a.parent?))).collect();
    parents
        .iter()
        .map(|(&reply, &first)| {
            let mut top = first;
            for _ in 0..64 {
                let Some(&up) = parents.get(&top) else { break };
                top = up;
            }
            (reply, top)
        })
        .collect()
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
