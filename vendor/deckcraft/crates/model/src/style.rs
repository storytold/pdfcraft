//! Colours, fills, lines and effects.

use deckcraft_color::{ColorScheme, ColorTransform, Rgba, SchemeSlot};
use serde::{Deserialize, Serialize};

use crate::MediaId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ColorBase {
    Rgb {
        rgb: Rgba,
    },
    Scheme {
        slot: SchemeSlot,
    },
    Preset {
        name: String,
    },
    /// A system colour (`windowText`…) with the value it had when saved.
    System {
        name: String,
        last: Rgba,
    },
}

/// A colour as written in a document: a base (RGB, theme slot…) plus transforms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorRef {
    pub base: ColorBase,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<ColorTransform>,
}

impl ColorRef {
    pub fn rgb(c: Rgba) -> Self {
        ColorRef { base: ColorBase::Rgb { rgb: c }, mods: vec![] }
    }
    pub fn scheme(slot: SchemeSlot) -> Self {
        ColorRef { base: ColorBase::Scheme { slot }, mods: vec![] }
    }
    pub fn with(mut self, m: ColorTransform) -> Self {
        self.mods.push(m);
        self
    }
    /// Resolve against a colour scheme; `ph` substitutes `phClr` (style matrix colour).
    pub fn resolve(&self, scheme: &ColorScheme, ph: Option<Rgba>) -> Rgba {
        let base = match &self.base {
            ColorBase::Rgb { rgb } => *rgb,
            ColorBase::Scheme { slot: SchemeSlot::PhClr } => ph.unwrap_or_else(|| scheme.get(SchemeSlot::Accent1)),
            ColorBase::Scheme { slot } => scheme.get(*slot),
            ColorBase::Preset { name } => deckcraft_color::preset(name).unwrap_or(Rgba::BLACK),
            ColorBase::System { last, .. } => *last,
        };
        deckcraft_color::apply(base, &self.mods)
    }
    /// Alpha (0–1) from the `alpha`, `alphaMod` and `alphaOff` transforms, applied in order.
    pub fn alpha(&self) -> f64 {
        self.mods.iter().fold(1.0, |a, m| match m {
            ColorTransform::Alpha(v) => (*v as f64 / 100_000.0).clamp(0.0, 1.0),
            ColorTransform::AlphaMod(v) => (a * *v as f64 / 100_000.0).clamp(0.0, 1.0),
            ColorTransform::AlphaOff(v) => (a + *v as f64 / 100_000.0).clamp(0.0, 1.0),
            _ => a,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradientStop {
    /// 0–1.
    pub pos: f64,
    pub color: ColorRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GradientFlip {
    None,
    X,
    Y,
    Xy,
}

impl GradientFlip {
    pub fn xml(self) -> &'static str {
        match self {
            GradientFlip::None => "none",
            GradientFlip::X => "x",
            GradientFlip::Y => "y",
            GradientFlip::Xy => "xy",
        }
    }

    pub fn from_xml(value: &str) -> Option<Self> {
        match value {
            "none" => Some(GradientFlip::None),
            "x" => Some(GradientFlip::X),
            "y" => Some(GradientFlip::Y),
            "xy" => Some(GradientFlip::Xy),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GradientShape {
    /// Angle in degrees clockwise from left→right.
    Linear {
        angle: f64,
        #[serde(default)]
        scaled: bool,
    },
    /// `circle`, `rect` or `shape` path gradient; `focus` is the fill-to rect as fractions (l, t, r, b).
    Path { path: String, focus: [f64; 4] },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gradient {
    pub stops: Vec<GradientStop>,
    pub shape: GradientShape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip: Option<GradientFlip>,
    #[serde(default = "crate::yes")]
    pub rotate_with_shape: bool,
    /// `a:tileRect`: the box the gradient is laid out in, as insets from the shape's box
    /// (fractions; negative reaches outside it). PowerPoint's "from corner" radial presets use it
    /// to centre the gradient on the corner.
    #[serde(default, skip_serializing_if = "is_zero4")]
    pub tile: [f64; 4],
}

fn is_zero4(v: &[f64; 4]) -> bool {
    v.iter().all(|x| *x == 0.0)
}

/// How a picture fills its box: stretched into a (fractional) inset rect, or tiled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PictureMode {
    Stretch {
        #[serde(default)]
        fill_rect: [f64; 4],
    },
    Tile {
        tx: f64,
        ty: f64,
        sx: f64,
        sy: f64,
        #[serde(default)]
        flip: String,
        #[serde(default)]
        align: String,
    },
}

impl Default for PictureMode {
    fn default() -> Self {
        PictureMode::Stretch { fill_rect: [0.0; 4] }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PictureFill {
    pub media: MediaId,
    /// Source crop as fractions of the image (left, top, right, bottom); negative values pad.
    pub crop: [f64; 4],
    pub mode: PictureMode,
    /// 0–1 opacity of the picture.
    pub alpha: Option<f64>,
    pub adjust: PictureAdjust,
    /// `rotWithShape="0"`: the picture keeps to the slide's axes when its shape is rotated or
    /// flipped.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub upright: bool,
    /// An artistic effect (Picture Format ▸ Artistic Effects). `media` is then the processed
    /// picture; the original is kept to change or remove the effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artistic: Option<Artistic>,
}

/// An applied artistic effect: its `a14` name (e.g. `artisticBlur`) and the original picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artistic {
    pub effect: String,
    pub original: MediaId,
}

/// Picture corrections and colour (Picture Format ▸ Corrections / Color / Transparency).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PictureAdjust {
    /// -1..1
    pub brightness: f64,
    /// -1..1
    pub contrast: f64,
    /// 0..4 (1 = unchanged)
    pub saturation: Option<f64>,
    pub grayscale: bool,
    /// Recolour duotone (dark, light).
    pub duotone: Option<(ColorRef, ColorRef)>,
    /// Make a colour transparent (Set Transparent Color).
    pub clear_color: Option<Rgba>,
    pub sharpen: f64,
    /// `a:biLevel`: black and white, split at this luminance (0–1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bilevel: Option<f64>,
    /// `a:alphaInv`: opacity inverted.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub alpha_inv: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternFill {
    pub preset: String,
    pub fg: ColorRef,
    pub bg: ColorRef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Fill {
    None,
    Solid {
        color: ColorRef,
    },
    Gradient(Gradient),
    Picture(PictureFill),
    Pattern(PatternFill),
    /// Use the containing group's fill.
    Group,
    /// Use the slide background (`useBgFill`).
    Background,
}

impl Fill {
    pub fn solid(c: ColorRef) -> Self {
        Fill::Solid { color: c }
    }
    pub fn is_none(&self) -> bool {
        matches!(self, Fill::None)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineCap {
    #[default]
    Flat,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineJoin {
    #[default]
    Round,
    Bevel,
    Miter,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineAlign {
    #[default]
    Center,
    Inside,
}

impl LineAlign {
    pub fn xml(self) -> &'static str {
        match self {
            LineAlign::Center => "ctr",
            LineAlign::Inside => "in",
        }
    }

    pub fn from_xml(value: &str) -> Option<Self> {
        match value {
            "ctr" => Some(LineAlign::Center),
            "in" => Some(LineAlign::Inside),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Compound {
    #[default]
    Single,
    Double,
    ThickThin,
    ThinThick,
    Triple,
}

impl Compound {
    pub fn xml(self) -> &'static str {
        match self {
            Compound::Single => "sng",
            Compound::Double => "dbl",
            Compound::ThickThin => "thickThin",
            Compound::ThinThick => "thinThick",
            Compound::Triple => "tri",
        }
    }
    pub fn from_xml(s: &str) -> Self {
        match s {
            "dbl" => Compound::Double,
            "thickThin" => Compound::ThickThin,
            "thinThick" => Compound::ThinThick,
            "tri" => Compound::Triple,
            _ => Compound::Single,
        }
    }
}

/// Dash style: a preset name (`solid`, `dot`, `dash`, `lgDash`, `dashDot`, `lgDashDot`,
/// `lgDashDotDot`, `sysDash`, `sysDot`, `sysDashDot`, `sysDashDotDot`) or custom (dash, space) pairs
/// in multiples of the line width.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Dash {
    Preset { name: String },
    Custom { pattern: Vec<(f64, f64)> },
}

impl Dash {
    pub const PRESETS: [&str; 11] =
        ["solid", "sysDot", "sysDash", "dash", "dashDot", "lgDash", "lgDashDot", "lgDashDotDot", "dot", "sysDashDot", "sysDashDotDot"];
    /// Dash/space lengths in multiples of the line width.
    pub fn pattern(&self) -> Vec<f64> {
        match self {
            Dash::Custom { pattern } => pattern.iter().flat_map(|(d, s)| [*d, *s]).collect(),
            Dash::Preset { name } => match name.as_str() {
                "dot" => vec![1.0, 3.0],
                "dash" => vec![4.0, 3.0],
                "lgDash" => vec![8.0, 3.0],
                "dashDot" => vec![4.0, 3.0, 1.0, 3.0],
                "lgDashDot" => vec![8.0, 3.0, 1.0, 3.0],
                "lgDashDotDot" => vec![8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
                "sysDash" => vec![3.0, 1.0],
                "sysDot" => vec![1.0, 1.0],
                "sysDashDot" => vec![3.0, 1.0, 1.0, 1.0],
                "sysDashDotDot" => vec![3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
                _ => vec![],
            },
        }
    }
}

/// Arrowhead at a line end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineEnd {
    /// `none`, `triangle`, `stealth`, `diamond`, `oval`, `arrow`.
    pub kind: String,
    /// `sm`, `med`, `lg`.
    #[serde(default = "med")]
    pub w: String,
    #[serde(default = "med")]
    pub len: String,
}

fn med() -> String {
    "med".into()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Line {
    pub fill: Option<Fill>,
    /// Width in points.
    pub width: Option<f64>,
    pub dash: Option<Dash>,
    pub cap: Option<LineCap>,
    pub join: Option<LineJoin>,
    pub miter_limit: Option<u32>,
    pub align: Option<LineAlign>,
    pub compound: Option<Compound>,
    pub head: Option<LineEnd>,
    pub tail: Option<LineEnd>,
}

impl Line {
    pub fn solid(c: ColorRef, width: f64) -> Self {
        Line { fill: Some(Fill::solid(c)), width: Some(width), ..Default::default() }
    }
    pub fn none() -> Self {
        Line { fill: Some(Fill::None), ..Default::default() }
    }
    /// Fill unset fields from `parent`.
    pub fn inherit(&mut self, parent: &Line) {
        if self.fill.is_none() {
            self.fill.clone_from(&parent.fill);
        }
        self.width = self.width.or(parent.width);
        if self.dash.is_none() {
            self.dash.clone_from(&parent.dash);
        }
        self.cap = self.cap.or(parent.cap);
        self.join = self.join.or(parent.join);
        self.miter_limit = self.miter_limit.or(parent.miter_limit);
        self.align = self.align.or(parent.align);
        self.compound = self.compound.or(parent.compound);
        if self.head.is_none() {
            self.head.clone_from(&parent.head);
        }
        if self.tail.is_none() {
            self.tail.clone_from(&parent.tail);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shadow {
    pub color: ColorRef,
    /// Blur radius, points.
    pub blur: f64,
    /// Distance, points.
    pub dist: f64,
    /// Direction, degrees (0 = right, 90 = down).
    pub dir: f64,
    #[serde(default)]
    pub inner: bool,
    /// Scale (1 = 100%) for perspective shadows.
    #[serde(default = "one")]
    pub sx: f64,
    #[serde(default = "one")]
    pub sy: f64,
    #[serde(default)]
    pub kx: f64,
    #[serde(default)]
    pub ky: f64,
    #[serde(default)]
    pub align: String,
    #[serde(default = "crate::yes")]
    pub rotate_with_shape: bool,
}

fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Glow {
    pub color: ColorRef,
    pub radius: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Reflection {
    pub blur: f64,
    pub start_alpha: f64,
    pub end_alpha: f64,
    pub end_pos: f64,
    pub dist: f64,
    /// DrawingML reflection transform values retain their native integer units.
    pub st_pos: Option<i32>,
    pub dir: Option<i32>,
    pub fade_dir: Option<i32>,
    pub sx: Option<i32>,
    pub sy: Option<i32>,
    pub kx: Option<i32>,
    pub ky: Option<i32>,
    pub align: Option<String>,
    pub rotate_with_shape: Option<bool>,
}

impl Default for Reflection {
    fn default() -> Self {
        Self {
            blur: 0.0,
            start_alpha: 0.0,
            end_alpha: 0.0,
            end_pos: 0.0,
            dist: 0.0,
            st_pos: None,
            dir: Some(5_400_000),
            fade_dir: None,
            sx: None,
            sy: Some(-100_000),
            kx: None,
            ky: None,
            align: Some("bl".into()),
            rotate_with_shape: Some(false),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Effects {
    pub outer_shadow: Option<Shadow>,
    pub inner_shadow: Option<Shadow>,
    pub glow: Option<Glow>,
    /// Soft edges radius in points.
    pub soft_edge: Option<f64>,
    pub reflection: Option<Reflection>,
    /// Bevel preset name for 3-D format (rendered as an approximation).
    pub bevel: Option<String>,
    /// Unmodelled 3-D XML kept for round-trip (`scene3d`, `sp3d`).
    pub raw3d: Option<String>,
    /// `a:fillOverlay`: a fill blended over the shape's fill (`blend`: `over`, `mult`, `screen`,
    /// `darken`, `lighten`). Only solid and gradient overlays are kept.
    pub fill_overlay: Option<FillOverlay>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillOverlay {
    pub blend: String,
    pub fill: Fill,
}

impl Effects {
    pub fn is_empty(&self) -> bool {
        self.outer_shadow.is_none()
            && self.inner_shadow.is_none()
            && self.glow.is_none()
            && self.soft_edge.is_none()
            && self.reflection.is_none()
            && self.bevel.is_none()
            && self.fill_overlay.is_none()
    }
}

/// A shape's theme style references (`p:style`): which fill/line/effect of the theme's format scheme
/// to use, with which colour, and the theme font and colour for its text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeStyle {
    pub line_ref: (u32, ColorRef),
    pub fill_ref: (u32, ColorRef),
    pub effect_ref: (u32, ColorRef),
    /// `major`, `minor` or `none`, with the text colour.
    pub font_ref: (String, Option<ColorRef>),
}

impl ShapeStyle {
    /// The default for newly drawn shapes: accent 1 fill, darker accent 1 outline, light text.
    pub fn accent(slot: SchemeSlot) -> Self {
        ShapeStyle {
            line_ref: (2, ColorRef::scheme(slot).with(ColorTransform::Shade(50000))),
            fill_ref: (1, ColorRef::scheme(slot)),
            effect_ref: (0, ColorRef::scheme(slot)),
            font_ref: ("minor".into(), Some(ColorRef::scheme(SchemeSlot::Lt1))),
        }
    }
    /// The default for lines/connectors.
    pub fn line(slot: SchemeSlot) -> Self {
        ShapeStyle {
            line_ref: (1, ColorRef::scheme(slot)),
            fill_ref: (0, ColorRef::scheme(slot)),
            effect_ref: (0, ColorRef::scheme(slot)),
            font_ref: ("minor".into(), Some(ColorRef::scheme(SchemeSlot::Tx1))),
        }
    }
}
