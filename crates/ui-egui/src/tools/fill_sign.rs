//! Fill & Sign (Acrobat's Fill & Sign tool, execution plan M5.7): type text onto the page, place
//! ✓ ✕ ● ─ marks and today's date (in Preferences ▸ Date format), and sign with a drawn
//! signature. Everything is an annotation (typewriter text, PdfCraft-drawn stamps, ink), so it
//! can be moved, deleted and undone like any comment.

use egui::{Color32, CornerRadius, Pos2, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::signature_scan::{DEFAULT_SENSITIVITY, PREVIEW_MAX_SIDE, SCAN_MAX_SIDE, ScanError};
use pdfcraft_engine::{Edit, FillMark, NewAnnotation, PhotoRect, Shape, SignatureImage, SignaturePhoto, Style, Turn};
use pdfcraft_render::{DocInfo, PageInfo};

use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::theme::Tokens;

mod scan;

pub use scan::ScanDraft;
use scan::scan_tab;

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
/// text (drawn in the script font), or a locally imported image.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SavedSig {
    Drawn(Vec<Vec<[f32; 2]>>),
    Typed(String),
    Image(#[serde(with = "image_data")] SignatureImage),
}

impl SavedSig {
    /// This signature as the graphic of a visible digital signature. `None` for typed text the
    /// script font has nothing to draw for.
    pub fn graphic(&self) -> Option<pdfcraft_engine::sign::Graphic> {
        match self {
            SavedSig::Drawn(strokes) => Some(pdfcraft_engine::sign::Graphic::Strokes(
                strokes.iter().map(|s| s.iter().map(|p| [f64::from(p[0]), f64::from(p[1])]).collect()).collect(),
            )),
            SavedSig::Typed(text) => pdfcraft_engine::typed_signature_graphic(text),
            SavedSig::Image(image) => Some(image.graphic()),
        }
    }
}

/// How many signatures Fill & Sign keeps, and, in a list of their own, how many initials.
pub const MAX_SAVED_SIGNATURES: usize = 20;
/// The longest signature or initials label, in characters.
pub const MAX_SIGNATURE_LABEL_CHARS: usize = 60;

/// A saved signature and the label it is listed under ("Work", "Ada Lovelace", "Signature 2").
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LabelledSig {
    pub label: String,
    pub sig: SavedSig,
}

/// The saved Fill & Sign signatures (or, in a second library, initials), at most
/// [`MAX_SAVED_SIGNATURES`], and the active one: the one Sign (Initials) places, Change edits and
/// Remove deletes, and for signatures the one a visible digital signature shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignatureLibrary {
    entries: Vec<LabelledSig>,
    active: usize,
    /// A list of initials: new entries are offered "Initials N" rather than "Signature N".
    initials: bool,
}

impl SignatureLibrary {
    /// Up to [`MAX_SAVED_SIGNATURES`] of `entries`, labels cleaned and `active` clamped into range.
    pub fn new(entries: Vec<LabelledSig>, active: usize) -> Self {
        Self::build(false, entries, active)
    }

    /// [`Self::new`] for saved initials.
    pub fn new_initials(entries: Vec<LabelledSig>, active: usize) -> Self {
        Self::build(true, entries, active)
    }

    fn build(initials: bool, mut entries: Vec<LabelledSig>, active: usize) -> Self {
        entries.truncate(MAX_SAVED_SIGNATURES);
        let mut library = Self { entries: Vec::with_capacity(entries.len()), active: 0, initials };
        for e in entries {
            library.add(&e.label, e.sig);
        }
        library.active = active.min(library.entries.len().saturating_sub(1));
        library
    }

    /// This library holds initials.
    pub fn holds_initials(&self) -> bool {
        self.initials
    }

    /// The saved signatures, in the order they are listed.
    pub fn entries(&self) -> &[LabelledSig] {
        &self.entries
    }

    /// How many signatures are saved.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No signature is saved.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// [`MAX_SAVED_SIGNATURES`] are saved: [`Self::add`] refuses another.
    pub fn is_full(&self) -> bool {
        self.entries.len() >= MAX_SAVED_SIGNATURES
    }

    /// The active entry's index, `None` when nothing is saved.
    pub fn active_index(&self) -> Option<usize> {
        (self.active < self.entries.len()).then_some(self.active)
    }

    /// The active signature with its label.
    pub fn active_entry(&self) -> Option<&LabelledSig> {
        self.entries.get(self.active)
    }

    /// The signature Fill & Sign places.
    pub fn active(&self) -> Option<&SavedSig> {
        self.active_entry().map(|e| &e.sig)
    }

    /// Make entry `i` the active one. False (and nothing changes) when there is no entry `i`.
    pub fn select(&mut self, i: usize) -> bool {
        let ok = i < self.entries.len();
        if ok {
            self.active = i;
        }
        ok
    }

    /// Select the first entry labelled `label`, ignoring case and surrounding spaces.
    pub fn select_label(&mut self, label: &str) -> bool {
        let want = label.trim();
        match self.entries.iter().position(|e| same_label(&e.label, want)) {
            Some(i) => self.select(i),
            None => false,
        }
    }

    /// Add a signature and make it active; a blank label becomes [`Self::next_label`]. False
    /// (and nothing changes) when the library is full.
    pub fn add(&mut self, label: &str, sig: SavedSig) -> bool {
        if self.is_full() {
            return false;
        }
        let label = self.label_or_next(label, None);
        self.entries.push(LabelledSig { label, sig });
        self.active = self.entries.len().saturating_sub(1);
        true
    }

    /// Replace the active entry's signature and label, or add the first one when none is saved.
    pub fn replace_active(&mut self, label: &str, sig: SavedSig) {
        let label = self.label_or_next(label, Some(self.active));
        match self.entries.get_mut(self.active) {
            Some(e) => *e = LabelledSig { label, sig },
            None => {
                self.entries.push(LabelledSig { label, sig });
                self.active = self.entries.len().saturating_sub(1);
            }
        }
    }

    /// Set the active signature, keeping its label: `Some` replaces it (adding one when none is
    /// saved), `None` removes it.
    pub fn set_active(&mut self, sig: Option<SavedSig>) {
        match sig {
            Some(sig) => {
                let label = self.active_entry().map(|e| e.label.clone()).unwrap_or_default();
                self.replace_active(&label, sig);
            }
            None => self.remove_active(),
        }
    }

    /// Remove the active entry; the one before it (or the new first one) becomes active.
    pub fn remove_active(&mut self) {
        if self.active < self.entries.len() {
            self.entries.remove(self.active);
            self.active = self.active.saturating_sub(1).min(self.entries.len().saturating_sub(1));
        }
    }

    /// "Signature N" (or "Initials N") for the lowest N no entry is labelled with yet.
    pub fn next_label(&self) -> String {
        let (template, plain) = if self.initials { (tl!("Initials {n}"), tl!("Initials")) } else { (tl!("Signature {n}"), tl!("Signature")) };
        (1..=self.entries.len().saturating_add(1))
            .map(|n| crate::i18n::fmt(template, &[("n", &n.to_string())]))
            .find(|l| !self.label_taken(l, None))
            .unwrap_or_else(|| plain.to_string())
    }

    /// `label` cleaned, or [`Self::next_label`] when nothing is left of it. A label another
    /// entry (not `keep`, the one being replaced) already has gets the lowest free number, as
    /// "Work (2)", so [`Self::select_label`] can reach every entry.
    fn label_or_next(&self, label: &str, keep: Option<usize>) -> String {
        let label = clean_label(label);
        if label.is_empty() {
            return self.next_label();
        }
        if !self.label_taken(&label, keep) {
            return label;
        }
        // `len + 1` candidates, all different (each ends in its own number), against at most
        // `len` labels: one is always free, so the fallback below is never reached.
        (2..=self.entries.len().saturating_add(2))
            .map(|n| {
                let suffix = format!(" ({n})");
                let room = MAX_SIGNATURE_LABEL_CHARS.saturating_sub(suffix.chars().count());
                let base: String = label.chars().take(room).collect();
                format!("{}{suffix}", base.trim_end())
            })
            .find(|l| !self.label_taken(l, keep))
            .unwrap_or(label)
    }

    /// Some entry other than `keep` is labelled `label`, compared as [`Self::select_label`] does.
    fn label_taken(&self, label: &str, keep: Option<usize>) -> bool {
        self.entries.iter().enumerate().any(|(i, e)| Some(i) != keep && same_label(&e.label, label))
    }
}

/// Two labels name the same signature: equal ignoring case.
fn same_label(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// A label on one line, trimmed, and at most [`MAX_SIGNATURE_LABEL_CHARS`] characters.
fn clean_label(label: &str) -> String {
    let one_line: String = label.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    one_line.trim().chars().take(MAX_SIGNATURE_LABEL_CHARS).collect::<String>().trim_end().to_string()
}

pub(crate) mod image_data {
    use base64::Engine as _;
    use pdfcraft_engine::{SignatureImage, signature_image::MAX_SIGNATURE_IMAGE_BYTES};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(image: &SignatureImage, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(image.bytes().as_slice()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SignatureImage, D::Error> {
        let encoded = String::deserialize(d)?;
        if encoded.len() > MAX_SIGNATURE_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err(serde::de::Error::custom("signature image exceeds 4 MiB"));
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(serde::de::Error::custom)?;
        SignatureImage::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// The Create signature / initials dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SigDraft {
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub text: String,
    /// The Draw tab (otherwise Type).
    pub drawing: bool,
    /// The Image tab. The file is held in the draft until Apply.
    pub image_mode: bool,
    /// The Image or Scan tab's result.
    pub image: Option<SignatureImage>,
    /// The Scan tab: a photo of a signature on paper, cleaned into `image`.
    pub scan_mode: bool,
    pub scan: Option<ScanDraft>,
    /// Creating initials (otherwise the signature).
    pub initials: bool,
    /// Replacing an existing saved value.
    pub editing: bool,
    /// The signature's or initials' label in its saved list.
    pub label: String,
}

impl SigDraft {
    pub fn new(initials: bool, name: &str) -> Self {
        let text = if initials { name.split_whitespace().filter_map(|w| w.chars().next()).collect() } else { name.trim().to_string() };
        Self { text, initials, ..Default::default() }
    }

    /// Edit a copy of the saved value; Cancel must leave the original untouched.
    pub fn from_saved(initials: bool, saved: &SavedSig) -> Self {
        match saved {
            SavedSig::Drawn(strokes) => Self { strokes: strokes.clone(), drawing: true, initials, editing: true, ..Default::default() },
            SavedSig::Typed(text) => Self { text: text.clone(), initials, editing: true, ..Default::default() },
            SavedSig::Image(image) => Self { image: Some(image.clone()), image_mode: true, initials, editing: true, ..Default::default() },
        }
    }

    /// Take back the last drawn stroke (and any dot-less empty one a bare click left behind it).
    /// Returns whether a visible stroke was removed.
    pub fn undo_stroke(&mut self) -> bool {
        while let Some(s) = self.strokes.pop() {
            if s.len() > 1 {
                return true;
            }
        }
        false
    }

    /// The Draw tab is showing (not Type, Image or Scan).
    fn draw_tab(&self) -> bool {
        self.drawing && !self.image_mode && !self.scan_mode
    }

    fn ready(&self) -> bool {
        if self.scan_mode {
            // A preview made while the frame or slider is still moving is not the result.
            self.scan.as_ref().is_some_and(|s| s.drag.is_none() && s.full && !s.scanning()) && self.image.is_some()
        } else if self.image_mode {
            self.image.is_some()
        } else if self.drawing {
            self.strokes.iter().any(|s| s.len() > 1)
        } else {
            !self.text.trim().is_empty()
                && self.text.chars().take(pdfcraft_engine::MAX_SIGNATURE_CHARS + 1).count() <= pdfcraft_engine::MAX_SIGNATURE_CHARS
        }
    }

    pub fn saved(&self) -> Option<SavedSig> {
        if self.scan_mode {
            self.ready().then(|| self.image.clone().map(SavedSig::Image)).flatten()
        } else if self.image_mode {
            self.image.clone().map(SavedSig::Image)
        } else if self.drawing {
            Some(SavedSig::Drawn(self.strokes.clone()))
        } else {
            Some(SavedSig::Typed(self.text.trim().to_string()))
        }
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
    pub size: f64,
    pub focus: bool,
}

/// The default size Fill & Sign uses for typed text.
pub const TEXT_SIZE: f64 = 10.0;
pub const MIN_TEXT_SIZE: f64 = 4.0;
pub const MAX_TEXT_SIZE: f64 = 300.0;

pub fn valid_text_size(size: f64) -> Option<f64> {
    (size.is_finite() && (MIN_TEXT_SIZE..=MAX_TEXT_SIZE).contains(&size)).then_some(size)
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
/// `rotation` (its `/Rotate`).
pub fn typed(page: usize, at: [f64; 2], text: &str, size: f64, rotation: i64, author: &str) -> Edit {
    let rect = crate::comments::text_box_rect(at, text, size, rotation);
    new(page, Shape::Typewriter { rect, font_size: size }, text.to_string(), author)
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
pub fn place(page: usize, info: &PageInfo, at: [f64; 2], sig: &SavedSig, initials: bool, author: &str) -> Option<Edit> {
    let rotation = i64::from(info.rotation);
    match sig {
        SavedSig::Drawn(strokes) => signature_at(page, at, strokes, rotation, author),
        SavedSig::Image(image) => image.edit(page, info, at, initials, author),
        SavedSig::Typed(text) => {
            let [left, bottom, right, top] = pdfcraft_engine::script_outline(text).bounds();
            let height = if initials { 24.0_f64 } else { 32.0_f64 };
            // Keep long names within the same placement width as drawn signatures.
            let height = height.min(150.0 * (top - bottom).max(0.1) / (right - left).max(0.01));
            typed_signature_at(page, at, text, height, rotation, author)
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
    // Device points (y down), then a non-zero winding scanline fill so overlapping same-direction
    // TrueType contours (such as Inter's composite Ђ and ћ) stay filled where strokes cross.
    let polys: Vec<Vec<(f64, f64)>> = o
        .contours
        .iter()
        .map(|c| c.iter().map(|p| (x0 + (p[0] - left) * k, h as f64 * 0.5 + (top + bottom) / 2.0 * k - p[1] * k)).collect())
        .collect();
    for y in 0..h {
        let sy = y as f64 + 0.5;
        let mut xs: Vec<(f64, i32)> = Vec::new();
        for poly in &polys {
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if (a.1 <= sy) != (b.1 <= sy) {
                    let dir = if a.1 < b.1 { 1 } else { -1 };
                    xs.push((a.0 + (sy - a.1) / (b.1 - a.1) * (b.0 - a.0), dir));
                }
            }
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0i32;
        let mut start = 0.0;
        for (x, dir) in xs {
            if winding == 0 {
                start = x;
            }
            winding += dir;
            if winding == 0 {
                let (from, to) = (start.round().clamp(0.0, w as f64) as usize, x.round().clamp(0.0, w as f64) as usize);
                for px in from..to {
                    img[(px, y)] = Color32::BLACK;
                }
            }
        }
    }
    img
}

/// Saved previews and add/remove controls, shared by the left panel and quick-tool picker: every
/// saved signature under its label (the active one highlighted), then every saved initials.
pub(crate) fn signature_entries(ui: &mut egui::Ui, app: &mut crate::PdfCraftApp, t: &Tokens) -> Option<&'static str> {
    ui.set_width(ui.available_width().clamp(240.0, 248.0));
    // Signature `i`'s preview is slot `2i` and initials `i`'s slot `2i + 1`, so a change to one
    // list never moves the other's previews.
    let slots = app.signatures.len().max(app.initials.len()).saturating_mul(2);
    if app.saved_signature_previews.len() < slots {
        app.saved_signature_previews.resize_with(slots, || None);
    }
    let signatures = library_cards(ui, t, &mut app.signatures, &mut app.saved_signature_previews, 330.0);
    ui.add_space(4.0);
    let initials = library_cards(ui, t, &mut app.initials, &mut app.saved_signature_previews, 170.0);
    ui.add_space(4.0);
    signatures.or(initials)
}

/// One library's cards under their labels, then its Add button or, when full, why there is none.
/// Returns the command a click asks for.
fn library_cards(
    ui: &mut egui::Ui,
    t: &Tokens,
    library: &mut SignatureLibrary,
    previews: &mut [Option<(SavedSig, egui::TextureHandle)>],
    max_height: f32,
) -> Option<&'static str> {
    let label = |template: &str, what: &str| crate::i18n::fmt(tl!(template), &[("what", what)]);
    let initials = library.holds_initials();
    let (what, kind) = if initials { (tl!("initials"), "initials") } else { (tl!("signature"), "signature") };
    let active = library.active_index();
    let mut clicked = None;
    egui::ScrollArea::vertical().id_salt(("saved", kind)).max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
        for (i, e) in library.entries().iter().enumerate() {
            let on = active == Some(i);
            ui.label(egui::RichText::new(&e.label).size(12.0).color(if on { t.accent } else { t.text_muted }));
            // Several cards are listed, so each button names its entry.
            let name = |template: &str| format!("{}: {}", label(template, what), e.label);
            let names = [name("Use {what}"), name("Change {what}"), name("Remove saved {what}"), name("Place saved {what}")];
            let Some(cache) = previews.get_mut(i.saturating_mul(2).saturating_add(usize::from(initials))) else { continue };
            if let Some(c) = saved_card(ui, t, &e.sig, on, &names, cache, &format!("saved-{kind}-{i}")) {
                clicked = Some((i, c));
            }
            ui.add_space(4.0);
        }
    });
    let mut command = None;
    if let Some((i, c)) = clicked
        && library.select(i)
    {
        command = Some(match (c, initials) {
            (CardClick::Use, false) => "sign.fill.signature",
            (CardClick::Change, false) => "sign.fill.signature.change",
            (CardClick::Remove, false) => "sign.fill.signature.remove",
            (CardClick::Use, true) => "sign.fill.initials",
            (CardClick::Change, true) => "sign.fill.initials.change",
            (CardClick::Remove, true) => "sign.fill.initials.remove",
        });
    }
    if library.is_full() {
        let n = MAX_SAVED_SIGNATURES.to_string();
        let full = if initials {
            crate::i18n::fmt(tl!("You can keep up to {n} sets of initials. Remove one to add another."), &[("n", &n)])
        } else {
            crate::i18n::fmt(tl!("You can keep up to {n} signatures. Remove one to add another."), &[("n", &n)])
        };
        ui.label(egui::RichText::new(full).size(12.0).color(t.text_muted));
    } else if crate::widgets::ghost_button(ui, "plus", &label("Add {what}", what)).clicked() {
        command = Some(if initials { "sign.fill.initials.add" } else { "sign.fill.signature.add" });
    }
    command
}

/// What a saved signature or initials card was clicked for.
#[derive(Clone, Copy)]
enum CardClick {
    Use,
    Change,
    Remove,
}

/// One saved signature or initials: its preview (click to place it), then Change and Remove.
/// `names` are the Use, Change and Remove buttons' accessible names and the preview's tooltip.
fn saved_card(
    ui: &mut egui::Ui,
    t: &Tokens,
    saved: &SavedSig,
    active: bool,
    names: &[String; 4],
    cache: &mut Option<(SavedSig, egui::TextureHandle)>,
    key: &str,
) -> Option<CardClick> {
    let [use_name, change_name, remove_name, hover] = names;
    let mut clicked = None;
    ui.horizontal(|ui| {
        let (rect, response) = ui.allocate_exact_size(vec2((ui.available_width() - 68.0).max(140.0), 52.0), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, use_name.clone()));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(4), Color32::WHITE);
        let (width, color) = if active { (2.0, t.accent) } else { (1.0, if response.hovered() { t.accent } else { t.border }) };
        painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(width, color), egui::StrokeKind::Inside);
        paint_saved(ui, rect.shrink(8.0), saved, cache, key);
        if response.on_hover_text(hover.as_str()).clicked() {
            clicked = Some(CardClick::Use);
        }
        if crate::icons::button(ui, "pencil", 26.0, false, change_name).clicked() {
            clicked = Some(CardClick::Change);
        }
        if crate::icons::button(ui, "x", 26.0, false, remove_name).clicked() {
            clicked = Some(CardClick::Remove);
        }
    });
    clicked
}

/// Draw a saved signature as large as fits in `rect`, centred: typed text and images through
/// `cache` (a texture named `key`), strokes as lines.
pub(crate) fn paint_saved(ui: &egui::Ui, rect: egui::Rect, saved: &SavedSig, cache: &mut Option<(SavedSig, egui::TextureHandle)>, key: &str) {
    let painter = ui.painter_at(rect);
    match saved {
        SavedSig::Typed(text) => {
            if cache.as_ref().is_none_or(|(s, _)| s != saved) {
                *cache = Some((saved.clone(), ui.ctx().load_texture(key, script_preview(text, 480, 104), egui::TextureOptions::LINEAR)));
            }
            if let Some((_, tex)) = cache {
                let size = vec2(480.0, 104.0) * (rect.width() / 480.0).min(rect.height() / 104.0);
                painter.image(
                    tex.id(),
                    egui::Rect::from_center_size(rect.center(), size),
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        SavedSig::Image(image) => image_preview(ui, rect, image, cache),
        SavedSig::Drawn(strokes) => {
            let bounds = strokes
                .iter()
                .flatten()
                .filter(|p| p.iter().all(|v| v.is_finite()))
                .fold(egui::Rect::NOTHING, |r, p| r.union(egui::Rect::from_min_max(pos2(p[0], p[1]), pos2(p[0], p[1]))));
            if bounds.is_finite() {
                let scale = (rect.width() / bounds.width().max(0.01)).min(rect.height() / bounds.height().max(0.01));
                for stroke in strokes {
                    let pts = stroke
                        .iter()
                        .filter(|p| p.iter().all(|v| v.is_finite()))
                        .map(|p| rect.center() + vec2(p[0] - bounds.center().x, bounds.center().y - p[1]) * scale)
                        .collect();
                    painter.add(egui::Shape::line(pts, Stroke::new(1.5, Color32::BLACK)));
                }
            }
        }
    }
}

/// Values that shape how Fill & Sign handles this page's input.
pub(crate) struct PageInput<'a> {
    pub(crate) tool: FillTool,
    pub(crate) text_size: f64,
    pub(crate) signature: Option<&'a SavedSig>,
    pub(crate) initials: Option<&'a SavedSig>,
    pub(crate) preview: &'a mut Option<(SavedSig, egui::TextureHandle)>,
    pub(crate) author: &'a str,
    pub(crate) date_text: &'a Result<String, String>,
}

/// Preview and clicks with a Fill & Sign tool on one page.
pub(crate) fn page_input(cx: PageToolContext<'_>, input: PageInput<'_>, view: &mut DocView) -> Option<FillAction> {
    let PageToolContext { ui, resp, xf, ptr, page, info, .. } = cx;
    let PageInput { tool, text_size, signature, initials, preview, author, date_text } = input;
    let pointer = ptr.within(xf.rect)?;
    let p = info.pages.get(page)?;
    let at = to_user(xf, info, page, pointer);
    let saved = match tool {
        FillTool::Signature => signature,
        FillTool::Initials => initials,
        _ => None,
    };
    if let Some(SavedSig::Image(image)) = saved {
        if let Some(rect) = image.rect(p, at, tool == FillTool::Initials) {
            let tex = image_texture(ui, image, preview);
            xf.paint_user_image(ui.painter(), tex, p, rect, i64::from(p.rotation), Color32::WHITE);
            view.cursor.claim(Layer::ToolArea, egui::CursorIcon::None);
        }
    } else {
        view.cursor.claim(Layer::ToolArea, if tool == FillTool::Text { egui::CursorIcon::Text } else { egui::CursorIcon::Crosshair });
    }
    if !resp.clicked() {
        return None;
    }
    // Typed text starts a little above the click, as displayed.
    let rotation = i64::from(p.rotation);
    let top = pdfcraft_engine::displayed_offset(rotation, at, 0.0, text_size * 0.6);
    match tool {
        FillTool::Text => {
            // Clicking away from an open box commits it: the page sees this click before the box
            // sees its lost focus, so replacing the box first would drop what was typed.
            let open = view.fill_text.replace(TypeBox { page, at: top, text: String::new(), size: text_size, focus: true });
            open.filter(|tb| !tb.text.trim().is_empty()).map(|tb| {
                FillAction::Edit(Box::new(typed(tb.page, tb.at, tb.text.trim(), tb.size, crate::comments::page_rotation(info, tb.page), author)))
            })
        }
        FillTool::Date => Some(match date_text {
            Ok(text) => FillAction::Edit(Box::new(typed(page, top, text, text_size, rotation, author))),
            Err(why) => FillAction::Refused(why.clone()),
        }),
        FillTool::Signature => match signature {
            Some(s) => place(page, p, at, s, false, author).map(|e| FillAction::Signature(Box::new(e))),
            None => Some(FillAction::CreateSignature),
        },
        FillTool::Initials => match initials {
            Some(s) => place(page, p, at, s, true, author).map(|e| FillAction::Signature(Box::new(e))),
            None => Some(FillAction::CreateInitials),
        },
        mark => {
            let mark = mark.mark()?;
            let rect = mark.rect_at(at, rotation);
            Some(FillAction::Edit(Box::new(new(page, Shape::Mark { rect, mark }, String::new(), author))))
        }
    }
}

/// What a Fill & Sign click asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum FillAction {
    Edit(Box<Edit>),
    /// Place once, then select the new signature or initials for adjustment.
    Signature(Box<Edit>),
    /// No signature yet: open the signature pad.
    CreateSignature,
    /// No initials yet.
    CreateInitials,
    /// Nothing placed, and why (today's date can't be written into the PDF yet).
    Refused(String),
}

/// The in-place editor for typed text. Returns the edit once committed.
pub(crate) fn type_box(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, author: &str) -> Option<Edit> {
    let tb = view.fill_text.clone()?;
    let xf = view.page_xform(tb.page)?;
    let v = info.pages.get(tb.page)?.user_to_view(tb.at[0] as f32, tb.at[1] as f32);
    let pos = xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph);
    let zoom = xf.zoom();
    let mut commit = false;
    let mut cancel = false;
    // Shown at once, like the paragraph editor: a fade-in shows the page's copy through it.
    // Over the page, so only where the document shows: a box scrolled under the mode bar or a
    // side panel must leave them their pointer (#2755). It stays where it was placed, cut off as
    // the page is, rather than pushed back into the window.
    egui::Area::new(egui::Id::unique(("fill-text", view.id.0)))
        .fade_in(false)
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain_to(view.viewport_rect())
        .constrain(false)
        .show(ctx, |ui| {
            let Some(t) = view.fill_text.as_mut() else { return };
            // The box is white in every theme, so the caret must not follow the theme (pale in dark mode) (#655).
            ui.visuals_mut().text_cursor.stroke.color = crate::edit_text_ui::caret_color_on(Color32::WHITE);
            let width = ((t.text.chars().count().max(8) as f32) * t.size as f32 * 0.6 * zoom).clamp(60.0, 600.0);
            // The area's max rect is last frame's size, which caps the edit's width; widen it so the
            // box grows as text is typed.
            ui.set_min_width(width);
            let r = ui
                .add(
                    egui::TextEdit::singleline(&mut t.text)
                        .font(egui::FontId::proportional((t.size as f32 * zoom).max(8.0)))
                        .desired_width(width)
                        .background_color(Color32::from_rgba_unmultiplied(255, 255, 255, 230))
                        .text_color(Color32::BLACK)
                        .hint_text(tl!("Type text"))
                        .id_salt("fill-text-edit"),
                )
                .accessible_name(tl!("Text to place"));
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
            return Some(typed(tb.page, tb.at, tb.text.trim(), tb.size, crate::comments::page_rotation(info, tb.page), author));
        }
    }
    None
}

/// The signature pad: type, draw, import an image or scan a photo. Returns Apply, Cancel and
/// Browse (for an image or, on the Scan tab, a photo).
pub(crate) fn signature_pad(
    ui: &mut egui::Ui,
    t: &Tokens,
    d: &mut SigDraft,
    preview: &mut Option<(SavedSig, egui::TextureHandle)>,
    photo_texture: &mut Option<(u64, egui::TextureHandle)>,
) -> (bool, bool, bool) {
    let what = if d.initials { tl!("initials") } else { tl!("signature") };
    let title = if d.editing { tl!("Change {what}") } else { tl!("Create {what}") };
    ui.label(egui::RichText::new(crate::i18n::fmt(title, &[("what", what)])).font(crate::theme::semibold(18.0)));
    ui.horizontal(|ui| {
        if crate::widgets::pill_button(ui, tl_ctx!("signature pad", "Type"), !d.drawing && !d.image_mode && !d.scan_mode).clicked() {
            d.drawing = false;
            d.image_mode = false;
            d.scan_mode = false;
        }
        if crate::widgets::pill_button(ui, tl!("Draw"), d.drawing && !d.image_mode && !d.scan_mode).clicked() {
            d.drawing = true;
            d.image_mode = false;
            d.scan_mode = false;
        }
        if crate::widgets::pill_button(ui, tl!("Image"), d.image_mode).clicked() {
            d.image_mode = true;
            d.scan_mode = false;
        }
        if crate::widgets::pill_button(ui, tl!("Scan"), d.scan_mode).clicked() && !d.scan_mode {
            d.scan_mode = true;
            d.image_mode = false;
            // The Image tab shares the result; show this tab's own again.
            if let Some(scan) = d.scan.as_mut() {
                scan.rescan_in_background(&mut d.image, true, Some(ui.ctx()));
            }
        }
    });
    ui.horizontal(|ui| {
        let l = ui.label(tl!("Label"));
        ui.add(
            egui::TextEdit::singleline(&mut d.label)
                .char_limit(MAX_SIGNATURE_LABEL_CHARS)
                .desired_width(400.0)
                .hint_text(tl!("Work, Personal, a name…")),
        )
        .labelled_by(l.id);
    });
    ui.add_space(6.0);
    if d.scan_mode {
        let browse = scan_tab(ui, t, d, preview, photo_texture);
        let (apply, cancel) = pad_buttons(ui, d);
        return (apply, cancel, browse);
    }
    if d.image_mode {
        ui.label(egui::RichText::new(tl!("Choose a PNG or JPEG image (up to 4 MiB). Transparency is preserved.")).color(t.text_muted));
        let browse = crate::widgets::ghost_button(ui, "folder", tl!("Browse…")).clicked();
        let (rect, _) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        if let Some(image) = &d.image {
            image_preview(ui, rect.shrink(8.0), image, preview);
        }
        let (apply, cancel) = pad_buttons(ui, d);
        return (apply, cancel, browse);
    }
    if !d.drawing {
        let l = ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Type your {what}."), &[("what", what)])).color(t.text_muted));
        ui.add(
            egui::TextEdit::singleline(&mut d.text)
                .char_limit(pdfcraft_engine::MAX_SIGNATURE_CHARS)
                .desired_width(460.0)
                .hint_text(tl!(if d.initials { "Initials" } else { "Your name" })),
        )
        .labelled_by(l.id);
        let (rect, _) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let key = SavedSig::Typed(d.text.clone());
        if preview.as_ref().is_none_or(|(s, _)| *s != key) {
            let img = script_preview(&d.text, 920, 300);
            *preview = Some((key, ui.ctx().load_texture("typed-signature", img, egui::TextureOptions::LINEAR)));
        }
        if let Some((_, tex)) = preview {
            painter.image(tex.id(), rect.shrink(4.0), egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        let (apply, cancel) = pad_buttons(ui, d);
        return (apply, cancel, false);
    }
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Draw your {what} below."), &[("what", what)])).color(t.text_muted));
    // The dialog is modal, so ⌘Z / Ctrl+Z here can only mean the last stroke (the document's own
    // undo is blocked while it is open).
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
        d.undo_stroke();
    }
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
    let (apply, cancel) = pad_buttons(ui, d);
    (apply, cancel, false)
}

fn image_texture(ui: &egui::Ui, image: &SignatureImage, cache: &mut Option<(SavedSig, egui::TextureHandle)>) -> egui::TextureId {
    let key = SavedSig::Image(image.clone());
    if cache.as_ref().is_some_and(|(s, _)| *s != key) {
        *cache = None;
    }
    let (_, tex) = cache.get_or_insert_with(|| {
        let pixels = egui::ColorImage::from_rgba_unmultiplied(image.size(), image.rgba());
        (key, ui.ctx().load_texture("signature-image", pixels, egui::TextureOptions::LINEAR))
    });
    tex.id()
}

fn image_preview(ui: &egui::Ui, rect: egui::Rect, image: &SignatureImage, cache: &mut Option<(SavedSig, egui::TextureHandle)>) {
    let tex = image_texture(ui, image, cache);
    let [w, h] = image.size();
    let size = vec2(w as f32, h as f32);
    let size = size * (rect.width() / size.x).min(rect.height() / size.y);
    ui.painter().image(
        tex,
        egui::Rect::from_center_size(rect.center(), size),
        egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

fn pad_buttons(ui: &mut egui::Ui, d: &mut SigDraft) -> (bool, bool) {
    ui.add_space(10.0);
    let (mut apply, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if d.draw_tab() && ui.add_enabled(!d.strokes.is_empty(), egui::Button::new(tl!("Undo"))).clicked() {
            d.undo_stroke();
        }
        if ui.button(tl!("Clear")).clicked() {
            d.strokes.clear();
            d.text.clear();
            d.image = None;
            if d.scan_mode {
                d.scan = None;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ready = d.ready();
            if ui.add_enabled_ui(ready, |ui| crate::widgets::pill_button(ui, tl!("Apply"), true)).inner.clicked() {
                apply = true;
            }
            if crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    (apply, cancel)
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Default)]
pub(crate) struct ImageInbox {
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    picked: std::sync::Arc<std::sync::Mutex<Option<PickedImage>>>,
}

#[cfg(target_arch = "wasm32")]
struct PickedImage {
    epoch: u64,
    initials: bool,
    /// A photo for the Scan tab (otherwise an image for the Image tab).
    scan: bool,
    bytes: Option<Result<Vec<u8>, String>>,
}

impl crate::PdfCraftApp {
    /// Browse asynchronously, answering only into the dialog (and tab) that asked for the image
    /// or, on the Scan tab, the photo.
    pub(crate) fn pick_signature_image(&mut self) {
        let epoch = self.dialog_epoch();
        let initials = self.signature_draft.initials;
        let scan = self.signature_draft.scan_mode;
        let dialog = rfd::AsyncFileDialog::new().add_filter(tl!(if scan { "Photo" } else { "Image" }), &["png", "jpg", "jpeg"]);
        #[cfg(not(target_arch = "wasm32"))]
        self.ask_one(crate::pickers::Ask::File(dialog), None, move |app, path| {
            if app.dialog_epoch() != epoch
                || app.dialog != Some(crate::Dialog::Signature)
                || app.signature_draft.initials != initials
                || app.signature_draft.scan_mode != scan
            {
                return;
            }
            if scan {
                let result = std::fs::File::open(path).map_err(ScanError::from).and_then(SignaturePhoto::read);
                app.use_signature_photo(result.map_err(|e| e.to_string()));
                return;
            }
            let result =
                std::fs::File::open(path).map_err(pdfcraft_engine::signature_image::SignatureImageError::from).and_then(SignatureImage::read);
            app.use_signature_image(result.map_err(|e| e.to_string()));
        });
        #[cfg(target_arch = "wasm32")]
        {
            use std::sync::atomic::Ordering;
            if self.signature_images.busy.swap(true, Ordering::SeqCst) {
                self.notify_tr("Another file dialog is still open. Finish with it first.");
                return;
            }
            let inbox = self.signature_images.clone();
            let ctx = self.ctx.clone();
            let (limit, too_large) = if scan {
                (pdfcraft_engine::signature_scan::MAX_PHOTO_BYTES, ScanError::Size.to_string())
            } else {
                (pdfcraft_engine::signature_image::MAX_SIGNATURE_IMAGE_BYTES, pdfcraft_engine::signature_image::SignatureImageError::Size.to_string())
            };
            wasm_bindgen_futures::spawn_local(async move {
                let bytes = if let Some(file) = dialog.pick_file().await {
                    if file.inner().size() > limit as f64 {
                        Some(Err(too_large))
                    } else {
                        // `pick_file` returns a readable handle. Read its Blob directly so a
                        // rejected browser read is an error (rfd's read helper unwraps it).
                        Some(match wasm_bindgen_futures::JsFuture::from(file.inner().array_buffer()).await {
                            Ok(buffer) => {
                                let bytes = js_sys::Uint8Array::new(&buffer);
                                if bytes.length() as usize > limit { Err(too_large) } else { Ok(bytes.to_vec()) }
                            }
                            Err(error) => Err(format!("{error:?}")),
                        })
                    }
                } else {
                    None
                };
                *inbox.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PickedImage { epoch, initials, scan, bytes });
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn process_signature_images(&mut self) {
        let picked = self.signature_images.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        let Some(picked) = picked else { return };
        self.signature_images.busy.store(false, std::sync::atomic::Ordering::SeqCst);
        if self.dialog_epoch() != picked.epoch
            || self.dialog != Some(crate::Dialog::Signature)
            || self.signature_draft.initials != picked.initials
            || self.signature_draft.scan_mode != picked.scan
        {
            return;
        }
        let Some(bytes) = picked.bytes else { return };
        if picked.scan {
            let result = bytes.and_then(|bytes| {
                pdfcraft_engine::guard(|| SignaturePhoto::from_bytes(&bytes)).map_err(|e| e.to_string())?.map_err(|e| e.to_string())
            });
            self.use_signature_photo(result);
            return;
        }
        let result = bytes
            .and_then(|bytes| pdfcraft_engine::guard(|| SignatureImage::from_bytes(&bytes)).map_err(|e| e.to_string())?.map_err(|e| e.to_string()));
        self.use_signature_image(result);
    }

    fn use_signature_image(&mut self, result: Result<SignatureImage, String>) {
        let cleaned = result.and_then(|image| image.remove_white_background(245, 35).map_err(|e| e.to_string()));
        match cleaned {
            Ok(image) => {
                self.signature_draft.image = Some(image);
                self.toast = None;
            }
            Err(e) => self.notify_fmt("Couldn't import the signature image: {e}", &[("e", &e)]),
        }
    }

    /// Files dropped while the signature pad shows the Image or Scan tab are imported there (the
    /// first one) instead of opening behind the dialog.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn drop_on_signature_pad(&mut self, dropped: Vec<egui::DroppedFileHandle>) -> Vec<egui::DroppedFileHandle> {
        let d = &self.signature_draft;
        if dropped.is_empty() || self.dialog != Some(crate::Dialog::Signature) || !(d.scan_mode || d.image_mode) {
            return dropped;
        }
        let scan = d.scan_mode;
        let Some(file) = dropped.into_iter().next() else { return Vec::new() };
        let limit = pdfcraft_engine::signature_image::MAX_SIGNATURE_IMAGE_BYTES;
        let path = file.path().to_path_buf();
        let bytes: Result<Vec<u8>, String> = if path.is_absolute() {
            use std::io::Read as _;
            let mut bytes = Vec::new();
            std::fs::File::open(&path).and_then(|f| f.take(limit as u64 + 1).read_to_end(&mut bytes)).map(|_| bytes).map_err(|e| e.to_string())
        } else {
            file.bytes().map_err(|e| e.to_string())
        };
        if scan {
            let photo =
                bytes.and_then(|b| pdfcraft_engine::guard(|| SignaturePhoto::from_bytes(&b)).map_err(|e| e.to_string())?.map_err(|e| e.to_string()));
            self.use_signature_photo(photo);
        } else {
            let image =
                bytes.and_then(|b| pdfcraft_engine::guard(|| SignatureImage::from_bytes(&b)).map_err(|e| e.to_string())?.map_err(|e| e.to_string()));
            self.use_signature_image(image);
        }
        Vec::new()
    }

    /// Frame the signature in a new photo and clean it up. A previous photo stays on failure.
    fn use_signature_photo(&mut self, result: Result<SignaturePhoto, String>) {
        match result.and_then(|photo| pdfcraft_engine::guard(|| ScanDraft::new(photo))) {
            Ok(mut scan) => {
                scan.rescan_in_background(&mut self.signature_draft.image, true, self.ctx.as_ref());
                self.signature_draft.scan = Some(scan);
                self.toast = None;
            }
            Err(e) => self.notify_fmt("Couldn't use the photo: {e}", &[("e", &e)]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LabelledSig, MAX_SAVED_SIGNATURES, MAX_SIGNATURE_LABEL_CHARS, SavedSig, SignatureLibrary};

    fn typed(s: &str) -> SavedSig {
        SavedSig::Typed(s.into())
    }

    fn labels(l: &SignatureLibrary) -> Vec<&str> {
        l.entries().iter().map(|e| e.label.as_str()).collect()
    }

    #[test]
    fn the_signature_library_holds_twenty_labelled_signatures() {
        let mut l = SignatureLibrary::default();
        assert_eq!(l.active(), None);
        assert_eq!(l.active_index(), None);
        assert!(l.add("Work", typed("Ada Lovelace")));
        assert!(l.add("  ", typed("A. Lovelace")), "a blank label is given the next free one");
        assert!(l.add("Home", typed("Ada")));
        assert_eq!(labels(&l), ["Work", "Signature 1", "Home"]);
        assert_eq!(l.active(), Some(&typed("Ada")), "a new signature becomes the active one");
        for n in l.len()..MAX_SAVED_SIGNATURES {
            assert!(l.add("", typed(&format!("Signer {n}"))));
        }
        assert!(l.is_full());
        let before = l.clone();
        assert!(!l.add("One too many", typed("Grace Hopper")));
        assert_eq!(l, before, "a full library is left as it was");
        assert_eq!(labels(&l).iter().filter(|x| x.starts_with("Signature ")).count(), MAX_SAVED_SIGNATURES - 2);
    }

    #[test]
    fn an_initials_library_offers_initials_labels_and_the_same_limit() {
        let mut l = SignatureLibrary::new_initials(vec![LabelledSig { label: String::new(), sig: typed("AL") }], 0);
        assert!(l.holds_initials());
        assert!(!SignatureLibrary::default().holds_initials());
        assert_eq!(labels(&l), ["Initials 1"], "a restored entry without a label is given one");
        assert!(l.add("", typed("AdL")));
        assert!(l.add("Mine", typed("A")));
        assert_eq!(labels(&l), ["Initials 1", "Initials 2", "Mine"]);
        assert_eq!(l.next_label(), "Initials 3");
        while l.add("", typed("AL")) {}
        assert_eq!(l.len(), MAX_SAVED_SIGNATURES);
        let mut kept = l.clone();
        kept.remove_active();
        assert!(kept.holds_initials(), "removing an entry keeps the kind");
    }

    #[test]
    fn selecting_changing_and_removing_follow_the_active_signature() {
        let mut l = SignatureLibrary::default();
        for (label, name) in [("Work", "Ada Lovelace"), ("Home", "Ada"), ("Initialled", "A. L.")] {
            l.add(label, typed(name));
        }
        assert!(l.select(0));
        assert!(!l.select(3), "there is no fourth signature");
        assert_eq!(l.active_index(), Some(0));
        assert!(l.select_label(" home "), "labels match ignoring case and spaces");
        assert_eq!(l.active(), Some(&typed("Ada")));
        assert!(!l.select_label("Office"));
        l.replace_active("House", typed("Ada L."));
        assert_eq!(labels(&l), ["Work", "House", "Initialled"]);
        l.set_active(Some(typed("Ada K.")));
        assert_eq!(l.active_entry(), Some(&LabelledSig { label: "House".into(), sig: typed("Ada K.") }), "set_active keeps the label");
        l.remove_active();
        assert_eq!(labels(&l), ["Work", "Initialled"]);
        assert_eq!(l.active_index(), Some(0), "the one before the removed signature becomes active");
        l.remove_active();
        l.remove_active();
        assert!(l.is_empty());
        assert_eq!(l.active_index(), None);
        l.remove_active();
        l.set_active(Some(typed("Grace Hopper")));
        assert_eq!(labels(&l), ["Signature 1"], "replacing with nothing saved adds the first signature");
    }

    #[test]
    fn labels_are_one_trimmed_line_and_fill_the_lowest_free_number() {
        let mut l = SignatureLibrary::default();
        l.add("  Work\nphone\t ", typed("Ada"));
        assert_eq!(labels(&l), ["Work phone"]);
        l.add(&"x".repeat(MAX_SIGNATURE_LABEL_CHARS * 3), typed("Ada"));
        assert_eq!(l.entries()[1].label.chars().count(), MAX_SIGNATURE_LABEL_CHARS);
        l.add(&"é".repeat(MAX_SIGNATURE_LABEL_CHARS + 1), typed("Ada"));
        assert_eq!(l.entries()[2].label, "é".repeat(MAX_SIGNATURE_LABEL_CHARS), "cut on a character, not a byte");
        l.add("Signature 1", typed("Ada"));
        assert_eq!(l.next_label(), "Signature 2");
        l.add("", typed("Ada"));
        l.add("Signature 4", typed("Ada"));
        assert_eq!(l.next_label(), "Signature 3");
    }

    #[test]
    fn every_label_names_one_signature() {
        // CodeRabbit on #3876: `select_label` takes the first match, so a second "Work" could
        // never be selected by label.
        let mut l = SignatureLibrary::default();
        l.add("Work", typed("Ada"));
        l.add("work", typed("Ada L."));
        l.add(" WORK ", typed("A. L."));
        assert_eq!(labels(&l), ["Work", "work (2)", "WORK (3)"]);
        assert!(l.select_label("work (2)"));
        assert_eq!(l.active(), Some(&typed("Ada L.")));
        // Keeping its own label is not a clash; taking another entry's is.
        l.replace_active("Work (2)", typed("Ada K."));
        assert_eq!(labels(&l), ["Work", "Work (2)", "WORK (3)"]);
        l.replace_active("WORK", typed("Ada K."));
        assert_eq!(labels(&l), ["Work", "WORK (2)", "WORK (3)"]);
        // A numbered label still fits the length limit.
        let long = "x".repeat(MAX_SIGNATURE_LABEL_CHARS);
        l.add(&long, typed("Ada"));
        l.add(&long, typed("Ada"));
        assert_eq!(l.entries()[4].label, format!("{} (2)", "x".repeat(MAX_SIGNATURE_LABEL_CHARS - 4)));
        // Restored settings with duplicates come back distinct too.
        let dup = |label: &str| LabelledSig { label: label.into(), sig: typed("Ada") };
        let restored = SignatureLibrary::new(vec![dup("Home"), dup("home"), dup("Signature 1"), dup("")], 0);
        assert_eq!(labels(&restored), ["Home", "home (2)", "Signature 1", "Signature 2"]);
    }

    #[test]
    fn a_library_from_settings_is_capped_and_its_active_index_clamped() {
        let many: Vec<LabelledSig> = (0..50).map(|i| LabelledSig { label: format!(" {i} "), sig: typed("Ada") }).collect();
        let l = SignatureLibrary::new(many, 999);
        assert_eq!(l.len(), MAX_SAVED_SIGNATURES);
        assert_eq!(l.entries()[0].label, "0");
        assert_eq!(l.active_index(), Some(MAX_SAVED_SIGNATURES - 1));
        let blank = SignatureLibrary::new(vec![LabelledSig { label: "\n".into(), sig: typed("Ada") }], 0);
        assert_eq!(labels(&blank), ["Signature 1"]);
        assert_eq!(SignatureLibrary::new(Vec::new(), 5).active_index(), None);
    }

    #[test]
    fn undo_removes_the_last_stroke_and_skips_empty_ones() {
        let mut d = super::SigDraft { drawing: true, ..Default::default() };
        assert!(!d.undo_stroke());
        d.strokes = vec![vec![[0.1, 0.1], [0.2, 0.2]], vec![[0.3, 0.3], [0.4, 0.4]], vec![[0.5, 0.5]], vec![]];
        assert!(d.undo_stroke());
        assert_eq!(d.strokes.len(), 1);
        assert!(d.undo_stroke());
        assert!(d.strokes.is_empty());
        assert!(!d.undo_stroke());
    }

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

    #[test]
    fn overlapping_glyph_contours_fill_without_holes_while_keeping_counters() {
        let runs =
            |img: &egui::ColorImage, y: usize| (0..img.width()).filter(|&x| img[(x, y)].a() > 0 && (x == 0 || img[(x - 1, y)].a() == 0)).count();
        // #1786: Inter's Ђ (T + right hook) and ћ (h + crossbar) have two same-direction contours
        // that overlap on the stem; non-zero winding fills the join instead of punching a hole.
        let dje = super::script_preview("Ђ", 200, 200);
        assert_eq!(dje[(97, 108)], egui::Color32::BLACK, "stem-hook overlap of Ђ is filled");
        assert_eq!(runs(&dje, 108), 2, "stem and right loop of Ђ are two solid strokes");
        let tshe = super::script_preview("ћ", 200, 200);
        assert_eq!(tshe[(93, 53)], egui::Color32::BLACK, "stem-crossbar overlap of ћ is filled");
        assert_eq!(runs(&tshe, 53), 1, "crossbar of ћ is one unbroken stroke across the stem");
        // Counter-clockwise inner contours remain open holes.
        let omicron = super::script_preview("О", 200, 200);
        assert_eq!(omicron[(109, 94)].a(), 0, "counter of О stays transparent");
        assert_eq!(runs(&omicron, 94), 2);
        assert!(super::script_preview("", 200, 200).pixels.iter().all(|p| p.a() == 0));
        assert!(super::script_preview("Ђћ", 0, 0).pixels.is_empty());
    }
}
