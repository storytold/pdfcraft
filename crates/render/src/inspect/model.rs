use super::{HashMap, HashSet};

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
    /// The points that shape a line (its two ends, `/L`) or connected lines, a polygon or a
    /// cloud (their corners, `/Vertices`), in user space; empty for other comments.
    pub points: Vec<[f32; 2]>,
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
/// following the links reaches a comment of its own. An `/IRT` to an unlisted object, a pop-up,
/// the annotation itself or a loop leaves it a comment of its own, so it stays in view instead of
/// hanging off nothing. A chain of any length settles: each reviewer's state changes reply to
/// their previous one (ISO 32000-2 §12.5.6.3), so a long review history is a long chain. Each
/// annotation is walked once, with a seen-set against loops. Both comment readers (inspect, and
/// the engine's list after an edit) run this.
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
    // Whether each annotation's chain reaches a comment of its own, filled in along each walk.
    let mut settled: Vec<Option<bool>> = vec![None; annotations.len()];
    for start in 0..annotations.len() {
        let (mut path, mut seen, mut at) = (Vec::new(), HashSet::new(), start);
        let ends_well = loop {
            match settled.get(at).copied().flatten() {
                Some(known) => break known,
                None if !seen.insert(at) => break false,
                None => {}
            }
            path.push(at);
            match links.get(at).copied().flatten() {
                Some(up) => at = up,
                None => break true,
            }
        };
        for i in path {
            if let Some(s) = settled.get_mut(i) {
                *s = Some(ends_well);
            }
        }
    }
    // A reply keeps its parent when it has a listed one and its chain reaches a comment; one whose
    // `/IRT` leads nowhere listed (itself, an unlisted object) is a comment of its own.
    for ((a, ok), link) in annotations.iter_mut().zip(settled).zip(links) {
        if link.is_none() || ok != Some(true) {
            a.parent = None;
        }
    }
}

/// The comment each reply's thread starts from, by the reply's page and `/Annots` position: a
/// reply to a reply files under that comment too. Expects links [`link_replies`] settled; a loop
/// in an unsettled list ends where it comes back round.
pub fn thread_tops(annotations: &[Annotation]) -> HashMap<(usize, usize), (usize, usize)> {
    let parents: HashMap<(usize, usize), (usize, usize)> = annotations.iter().filter_map(|a| Some(((a.page, a.index), a.parent?))).collect();
    let mut tops: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    for &reply in parents.keys() {
        let (mut path, mut seen, mut at) = (Vec::new(), HashSet::new(), reply);
        let top = loop {
            if let Some(&known) = tops.get(&at) {
                break known;
            }
            let Some(&up) = parents.get(&at) else { break at };
            if !seen.insert(at) {
                break up;
            }
            path.push(at);
            at = up;
        };
        for r in path {
            tops.insert(r, top);
        }
    }
    tops
}

/// The status and checkmark replies of a document's annotations, by the annotation whose state
/// each one sets, found by object reference (the settled `parent` links of [`link_replies`]).
///
/// ISO 32000-2 §12.5.6.3: a state change is a text annotation whose `/IRT` "shall refer to the
/// original annotation" and whose `/State` and `/StateModel` "update the state of the original
/// annotation for the specified user" (`/T`); "Additional state changes shall be made by adding
/// text annotations in reply to the previous reply for a given user". So a state reply sets the
/// state of the first annotation up its `/IRT` chain that is not itself a state reply: the
/// comment, or the reply the state was set on (a reply is an annotation with a state of its own,
/// and is never folded into its thread's comment). There is one status per reviewer, and a
/// checkmark only for the reviewer who set it.
pub struct ReviewStates<'a> {
    by_target: HashMap<(usize, usize), Vec<&'a Annotation>>,
}

impl<'a> ReviewStates<'a> {
    pub fn new(all: &'a [Annotation]) -> Self {
        let at: HashMap<(usize, usize), &Annotation> = all.iter().filter(|a| a.subtype != "Popup").map(|a| ((a.page, a.index), a)).collect();
        // The annotation each state reply's chain reaches past other state replies, `None` for a
        // loop or a reference to nothing listed; filled in along each walk, so a long history
        // costs one walk, not one per reply.
        let mut reaches: HashMap<(usize, usize), Option<(usize, usize)>> = HashMap::new();
        let mut by_target: HashMap<(usize, usize), Vec<&Annotation>> = HashMap::new();
        for r in all.iter().filter(|a| a.state.is_some()) {
            let (mut path, mut seen, mut up) = (vec![(r.page, r.index)], HashSet::from([(r.page, r.index)]), r.parent);
            let target = loop {
                let Some(key) = up else { break None };
                if let Some(&known) = reaches.get(&key) {
                    break known;
                }
                match at.get(&key) {
                    None => break None,
                    Some(p) if p.state.is_none() => break Some(key),
                    Some(_) if !seen.insert(key) => break None,
                    Some(p) => {
                        path.push(key);
                        up = p.parent;
                    }
                }
            };
            for key in path {
                reaches.insert(key, target);
            }
            if let Some(t) = target {
                by_target.entry(t).or_default().push(r);
            }
        }
        Self { by_target }
    }

    /// Every status and checkmark reply about `target`, in document order.
    pub fn history(&self, target: &Annotation) -> &[&'a Annotation] {
        self.by_target.get(&(target.page, target.index)).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Each reviewer's review status on `target`: their latest, in the order they first set one.
    /// Empty when nobody set one; a status set back to "None" stays listed, since None is a state
    /// (ISO 32000-2 Table 174: "The user has indicated nothing about the change").
    pub fn statuses(&self, target: &Annotation) -> Vec<(&'a str, &'a str)> {
        self.by_reviewer(target, false).into_iter().filter_map(|(who, mine)| Some((who, latest(&mine)?.state.as_deref()?))).collect()
    }

    /// The latest review status set on `target` by anyone, for callers that show one.
    pub fn latest_status(&self, target: &Annotation) -> Option<&'a str> {
        self.history(target).iter().rev().filter(|r| !r.is_mark()).find_map(|r| r.state.as_deref())
    }

    /// Whether `author`'s latest checkmark on `target` is "Marked". Checkmarks are each
    /// reviewer's own (ISO 32000-2 Table 174: "The annotation has been marked by the user").
    pub fn marked_by(&self, target: &Annotation, author: &str) -> bool {
        let mine: Vec<&Annotation> = self.history(target).iter().copied().filter(|r| r.is_mark() && reviewer(r) == author.trim()).collect();
        latest(&mine).is_some_and(|r| r.state.as_deref() == Some("Marked"))
    }

    /// The reviewers whose latest checkmark on `target` is "Marked".
    pub fn marked(&self, target: &Annotation) -> Vec<&'a str> {
        self.by_reviewer(target, true)
            .into_iter()
            .filter(|(_, mine)| latest(mine).is_some_and(|r| r.state.as_deref() == Some("Marked")))
            .map(|(who, _)| who)
            .collect()
    }

    /// The checkmark (`marks`) or status replies about `target`, by reviewer, in the order each
    /// reviewer first set one.
    fn by_reviewer(&self, target: &Annotation, marks: bool) -> Vec<(&'a str, Vec<&'a Annotation>)> {
        let mut out: Vec<(&str, Vec<&Annotation>)> = Vec::new();
        for &r in self.history(target).iter().filter(|r| r.is_mark() == marks) {
            match out.iter_mut().find(|(who, _)| *who == reviewer(r)) {
                Some((_, mine)) => mine.push(r),
                None => out.push((reviewer(r), vec![r])),
            }
        }
        out
    }
}

/// Of one reviewer's replies in one state model, the latest: the end of their chain, a reply none
/// of the others replies to (by reference), whatever order the file lists them in. Among several
/// such (changes that don't chain), the last listed.
fn latest<'b>(mine: &[&'b Annotation]) -> Option<&'b Annotation> {
    let answered: HashSet<(usize, usize)> = mine.iter().filter_map(|r| r.parent).collect();
    mine.iter().rev().find(|r| !answered.contains(&(r.page, r.index))).copied()
}

/// A reply's state: `/State`, or for a reply that gives only `/StateModel` the model's default
/// (ISO 32000-2 Table 175: "Unmarked if StateModel is Marked; None if StateModel is Review").
pub(super) fn state_or_default(state: Option<String>, model: Option<&str>) -> Option<String> {
    state.or_else(|| match model? {
        "Marked" => Some("Unmarked".into()),
        "Review" => Some("None".into()),
        _ => None,
    })
}

/// Who set a state: `/T`, trimmed (an author setting typed with spaces is the same reviewer).
fn reviewer(r: &Annotation) -> &str {
    r.author.as_deref().unwrap_or("").trim()
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

/// The review states and reply links of synthetic annotation lists.
#[cfg(test)]
mod review_state_tests {
    use super::{Annotation, ReviewStates, link_replies, thread_tops};

    /// One annotation of a synthetic page: `key` is its `/NM`, and `irt` the key of the
    /// annotation its `/IRT` references, resolved by position as the readers resolve references.
    type Spec<'s> = (&'s str, Option<&'s str>, &'s str, Option<&'s str>);

    /// The annotations of `specs` on page 0, in order, with their reply links settled as the
    /// readers settle them. An `irt` that names no key references nothing listed.
    fn listed(specs: &[Spec<'_>]) -> Vec<Annotation> {
        let mut all: Vec<Annotation> = specs
            .iter()
            .enumerate()
            .map(|(index, &(nm, irt, author, state))| Annotation {
                page: 0,
                subtype: "Text".into(),
                author: Some(author.into()),
                contents: None,
                modified: None,
                name: Some(nm.into()),
                in_reply_to: irt.map(Into::into),
                parent: irt.map(|k| (0, specs.iter().position(|s| s.0 == k).unwrap_or(usize::MAX))),
                grouped: false,
                rect: [0.0; 4],
                color: None,
                index,
                state: state.map(Into::into),
                quads: Vec::new(),
                locked: false,
                intent: None,
                fill_sign: false,
                no_rotate: false,
                points: Vec::new(),
            })
            .collect();
        link_replies(&mut all);
        all
    }

    fn by_name<'a>(all: &'a [Annotation], nm: &str) -> &'a Annotation {
        all.iter().find(|a| a.name.as_deref() == Some(nm)).expect("listed")
    }

    /// The states these replies set (ISO 32000-2 §12.5.6.3, with synthetic files): one status per
    /// reviewer, their latest, whether or not their changes chain; "None" stays listed; a
    /// checkmark only for the reviewer who set it.
    #[test]
    fn review_states_are_each_reviewers_latest_through_chains() {
        let all = listed(&[
            ("c", None, "Alice", None),
            ("b1", Some("c"), "Bob", Some("Accepted")),
            ("k1", Some("c"), "Carol", Some("Completed")),
            ("b2", Some("b1"), "Bob", Some("Rejected")), // chained, per §12.5.6.3
            ("r", Some("c"), "Dan", None),               // a reply, not a state
            ("rs", Some("r"), "Dan", Some("Cancelled")), // a state on the reply, not on c
            ("m1", Some("c"), "Bob", Some("Marked")),
            ("m2", Some("m1"), "Bob", Some("Unmarked")),
            ("m3", Some("c"), "Carol", Some("Marked")),
            ("n", None, "Alice", None),
            ("n1", Some("n"), "Bob", Some("Completed")),
            ("n2", Some("n"), "Bob", Some("None")), // unchained, as PdfCraft wrote it before
            // A loop of state replies belongs to nothing.
            ("x", Some("y"), "Eve", Some("Accepted")),
            ("y", Some("x"), "Eve", Some("Accepted")),
        ]);
        let states = ReviewStates::new(&all);
        let (c, r, n, x) = (by_name(&all, "c"), by_name(&all, "r"), by_name(&all, "n"), by_name(&all, "x"));
        assert_eq!(states.statuses(c), [("Bob", "Rejected"), ("Carol", "Completed")]);
        assert_eq!(states.latest_status(c), Some("Rejected"));
        assert_eq!(states.history(c).len(), 6, "every change, the checkmarks included");
        assert_eq!(states.statuses(r), [("Dan", "Cancelled")]);
        assert!(!states.marked_by(c, "Bob"), "Bob took his off");
        assert!(states.marked_by(c, "Carol"));
        assert!(!states.marked_by(c, "Alice"), "nobody else's checkmark shows");
        assert_eq!(states.marked(c), ["Carol"]);
        assert_eq!(states.statuses(n), [("Bob", "None")], "a status set back to None stays listed");
        assert!(states.statuses(x).is_empty() && states.history(x).is_empty());
    }

    /// §12.5.6.3: a state reply's `/IRT` "shall refer to the original annotation", and a reply is
    /// an annotation of its own. A status set on a reply, and the changes that chain from it, are
    /// that reply's and never its thread's comment's, at any depth of the thread.
    #[test]
    fn a_status_on_a_reply_and_its_chain_stay_the_replys() {
        let all = listed(&[
            ("c", None, "Alice", None),
            ("r", Some("c"), "Bob", None),
            ("rr", Some("r"), "Carol", None),
            ("s1", Some("rr"), "Dan", Some("Accepted")),
            ("s2", Some("s1"), "Dan", Some("Rejected")),
            ("e1", Some("s2"), "Eve", Some("Completed")), // Eve answering Dan's state reply
            ("m1", Some("r"), "Dan", Some("Marked")),
        ]);
        let states = ReviewStates::new(&all);
        let (c, r, rr) = (by_name(&all, "c"), by_name(&all, "r"), by_name(&all, "rr"));
        assert!(states.history(c).is_empty() && states.statuses(c).is_empty(), "not folded to the comment");
        assert!(states.statuses(r).is_empty() && states.marked_by(r, "Dan"));
        assert_eq!(states.statuses(rr), [("Dan", "Rejected"), ("Eve", "Completed")]);
        assert!(states.history(by_name(&all, "s1")).is_empty(), "a state reply has no states of its own");
    }

    /// States follow object references, not `/NM`: two comments with the same `/NM` (some writers
    /// name the first comment on every page alike) each keep their own statuses.
    #[test]
    fn states_follow_references_not_repeated_names() {
        let mut all = listed(&[
            ("a", None, "Alice", None),
            ("b", None, "Alice", None),
            ("sa", Some("a"), "Bob", Some("Accepted")),
            ("sb", Some("b"), "Bob", Some("Rejected")),
            ("sb2", Some("sb"), "Bob", Some("Completed")),
        ]);
        for a in all.iter_mut().take(2) {
            a.name = Some("dup".into());
        }
        for s in all.iter_mut().skip(2) {
            if s.in_reply_to.as_deref().is_some_and(|k| k == "a" || k == "b") {
                s.in_reply_to = Some("dup".into());
            }
        }
        let states = ReviewStates::new(&all);
        assert_eq!(states.statuses(&all[0]), [("Bob", "Accepted")]);
        assert_eq!(states.statuses(&all[1]), [("Bob", "Completed")]);
    }

    /// A state reply whose `/IRT` references nothing listed, or nothing at all, sets no
    /// annotation's state, and a loop that never reaches an annotation without a state belongs to
    /// nothing even in a list whose links were not settled.
    #[test]
    fn a_state_reply_without_a_listed_target_belongs_to_nothing() {
        let all = listed(&[
            ("c", None, "Alice", None),
            ("gone", Some("deleted"), "Bob", Some("Accepted")),
            ("bare", None, "Bob", Some("Rejected")),
            ("next", Some("gone"), "Bob", Some("Completed")),
        ]);
        let states = ReviewStates::new(&all);
        assert!(all.iter().all(|a| states.history(a).is_empty()));
        // Unsettled: the loop's links are still in place.
        let mut raw = listed(&[("c", None, "Alice", None)]);
        let mut x = raw[0].clone();
        (x.index, x.name, x.state, x.parent) = (1, Some("x".into()), Some("Accepted".into()), Some((0, 2)));
        let mut y = x.clone();
        (y.index, y.name, y.parent) = (2, Some("y".into()), Some((0, 1)));
        raw.extend([x, y]);
        let states = ReviewStates::new(&raw);
        assert!(raw.iter().all(|a| states.history(a).is_empty()));
    }

    /// A long reply chain, as a reviewer's long status history makes (§12.5.6.3: each change
    /// replies to the previous), settles, and its replies file under the comment; a loop, an `/IRT`
    /// to itself and one to nothing listed leave an annotation a comment of its own.
    #[test]
    fn reply_links_settle_at_any_depth_but_not_in_a_loop() {
        let keys: Vec<String> = (0..200).map(|i| format!("r{i}")).collect();
        let mut specs: Vec<Spec<'_>> = vec![("c", None, "Alice", None)];
        for (i, k) in keys.iter().enumerate() {
            specs.push((k, Some(if i == 0 { "c" } else { keys[i - 1].as_str() }), "Bob", None));
        }
        specs.extend([
            ("x", Some("y"), "Eve", None),
            ("y", Some("x"), "Eve", None),
            ("z", Some("x"), "Eve", None),
            ("me", Some("me"), "Eve", None),
            ("lost", Some("unlisted"), "Eve", None),
        ]);
        // In file order and newest first: the first reply read then walks the whole chain at once.
        for newest_first in [false, true] {
            if newest_first {
                specs.reverse();
            }
            let all = listed(&specs);
            let replies: Vec<&Annotation> = all.iter().filter(|a| a.name.as_deref().is_some_and(|n| n.starts_with('r'))).collect();
            assert!(replies.len() == 200 && replies.iter().all(|a| a.is_reply()), "{newest_first}");
            let nowhere = all.iter().filter(|a| matches!(a.name.as_deref(), Some("x" | "y" | "z" | "me" | "lost")));
            assert!(nowhere.clone().count() == 5 && nowhere.clone().all(|a| !a.is_reply()), "a loop, a chain into one, itself or nothing listed");
            let c = by_name(&all, "c");
            let tops = thread_tops(&all);
            assert_eq!(tops.len(), 200, "{newest_first}");
            assert!(tops.values().all(|&t| t == (c.page, c.index)), "{newest_first}");
        }
    }

    /// Review on #4126: a history has no length limit, and a reviewer's name is compared without
    /// surrounding spaces.
    #[test]
    fn a_long_chain_still_reaches_its_comment_and_names_are_trimmed() {
        let keys: Vec<String> = (0..200).map(|i| format!("s{i}")).collect();
        let mut specs: Vec<Spec<'_>> = vec![("c", None, "Alice", None)];
        for (i, k) in keys.iter().enumerate() {
            let parent = if i == 0 { "c" } else { keys[i - 1].as_str() };
            specs.push((k, Some(parent), "Bob", Some(if i == 199 { "Completed" } else { "Accepted" })));
        }
        specs.push(("m", Some("c"), " Carol ", Some("Marked")));
        let all = listed(&specs);
        let states = ReviewStates::new(&all);
        assert_eq!(states.history(&all[0]).len(), 201);
        assert_eq!(states.statuses(&all[0]), [("Bob", "Completed")]);
        assert!(states.marked_by(&all[0], "Carol") && states.marked_by(&all[0], "Carol "));
        assert_eq!(states.marked(&all[0]), ["Carol"]);
        // Listed newest first, the first reply read walks the whole chain at once.
        specs.reverse();
        let all = listed(&specs);
        let states = ReviewStates::new(&all);
        let c = by_name(&all, "c");
        assert_eq!(states.statuses(c), [("Bob", "Completed")]);
        assert_eq!(states.history(c).len(), 201);
    }

    /// The inspector reads a `/StateModel`-only reply as the model's default state, so it counts
    /// as Bob's status (None) on the note.
    #[test]
    fn the_inspector_reads_a_state_model_alone_as_its_default() {
        let objs: Vec<&[u8]> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [4 0 R 5 0 R] >>",
            b"<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /NM (note) /T (Ann) >>",
            b"<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /NM (bare) /T (Bob) /IRT 4 0 R /F 30 /StateModel (Review) >>",
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
        let info = crate::inspect(std::sync::Arc::new(out), None).expect("inspects");
        let note = by_name(&info.annotations, "note");
        assert_eq!(ReviewStates::new(&info.annotations).statuses(note), [("Bob", "None")]);
    }

    /// ISO 32000-2 Table 175: a missing `/State` is "Unmarked if StateModel is Marked; None if
    /// StateModel is Review".
    #[test]
    fn a_state_model_alone_gives_its_default_state() {
        use super::state_or_default;
        assert_eq!(state_or_default(None, Some("Review")).as_deref(), Some("None"));
        assert_eq!(state_or_default(None, Some("Marked")).as_deref(), Some("Unmarked"));
        assert_eq!(state_or_default(None, Some("Other")), None);
        assert_eq!(state_or_default(None, None), None);
        assert_eq!(state_or_default(Some("Accepted".into()), Some("Review")).as_deref(), Some("Accepted"));
    }
}
