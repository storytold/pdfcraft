//! The DeckCraft document model.
//!
//! A [`Presentation`] has slide masters (each with a theme and layouts) and slides. Each slide
//! uses a layout; placeholders on a slide inherit position, formatting and text styles from the
//! matching placeholder on its layout and master ([`resolve`]). Slides and masters sit behind
//! `Arc`s so an undo snapshot of the whole deck costs one clone of the slide list.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod anim;
pub mod bidi;
pub mod chart;
pub mod defaults;
pub mod edit;
pub use craft_drawingml::numfmt;
pub mod resolve;
pub mod style;
pub mod table;
pub mod text;
pub mod theme;
mod trendline;
pub mod words;

use std::sync::Arc;

pub use deckcraft_color::{ColorScheme, ColorTransform, Rgba, SchemeSlot};
pub use deckcraft_geom::{Rect, Size, Xfrm};
use serde::{Deserialize, Serialize};

pub use anim::{AnimClass, AnimStart, Animation, Transition};
pub use bidi::{first_strong_is_rtl, is_rtl_script};
pub use chart::Chart;
pub use style::{ColorBase, ColorRef, Effects, Fill, Line, ShapeStyle};
pub use table::{Cell, Table};
pub use text::{Paragraph, Run, RunProps, TextBody};
pub use theme::Theme;

pub(crate) fn yes() -> bool {
    true
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}
id_type!(SlideId);
id_type!(ShapeId);
id_type!(LayoutId);
id_type!(MasterId);
id_type!(MediaId);

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("no slide {0}")]
    NoSlide(String),
    #[error("no shape {0}")]
    NoShape(ShapeId),
    #[error("no layout {0}")]
    NoLayout(LayoutId),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ModelError>;

/// Placeholder types.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PhType {
    Title,
    CtrTitle,
    SubTitle,
    #[default]
    Body,
    Obj,
    Chart,
    Table,
    ClipArt,
    Diagram,
    Media,
    Picture,
    Date,
    Footer,
    SlideNum,
    Header,
    SlideImage,
}

impl PhType {
    pub fn xml(self) -> &'static str {
        match self {
            PhType::Title => "title",
            PhType::CtrTitle => "ctrTitle",
            PhType::SubTitle => "subTitle",
            PhType::Body => "body",
            PhType::Obj => "obj",
            PhType::Chart => "chart",
            PhType::Table => "tbl",
            PhType::ClipArt => "clipArt",
            PhType::Diagram => "dgm",
            PhType::Media => "media",
            PhType::Picture => "pic",
            PhType::Date => "dt",
            PhType::Footer => "ftr",
            PhType::SlideNum => "sldNum",
            PhType::Header => "hdr",
            PhType::SlideImage => "sldImg",
        }
    }
    pub fn from_xml(s: &str) -> Self {
        match s {
            "title" => PhType::Title,
            "ctrTitle" => PhType::CtrTitle,
            "subTitle" => PhType::SubTitle,
            "obj" => PhType::Obj,
            "chart" => PhType::Chart,
            "tbl" => PhType::Table,
            "clipArt" => PhType::ClipArt,
            "dgm" => PhType::Diagram,
            "media" => PhType::Media,
            "pic" => PhType::Picture,
            "dt" => PhType::Date,
            "ftr" => PhType::Footer,
            "sldNum" => PhType::SlideNum,
            "hdr" => PhType::Header,
            "sldImg" => PhType::SlideImage,
            _ => PhType::Body,
        }
    }
    pub fn is_title(self) -> bool {
        matches!(self, PhType::Title | PhType::CtrTitle)
    }
    /// The prompt shown in an empty placeholder in Normal view.
    pub fn prompt(self) -> &'static str {
        match self {
            PhType::Title | PhType::CtrTitle => "Click to add title",
            PhType::SubTitle => "Click to add subtitle",
            PhType::Body | PhType::Obj => "Click to add text",
            PhType::Chart => "Click icon to add chart",
            PhType::Table => "Click icon to add table",
            PhType::Diagram => "Click icon to add SmartArt graphic",
            PhType::Media => "Click icon to add media",
            PhType::Picture | PhType::ClipArt => "Click icon to add picture",
            PhType::Date => "Date",
            PhType::Footer => "Footer",
            PhType::SlideNum => "‹#›",
            PhType::Header => "Header",
            PhType::SlideImage => "",
        }
    }
    /// Footer-row placeholders are not shown on slides unless the deck turns them on.
    pub fn is_footer_kind(self) -> bool {
        matches!(self, PhType::Date | PhType::Footer | PhType::SlideNum | PhType::Header)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Placeholder {
    pub kind: PhType,
    pub idx: u32,
    /// `vert` orientation flag.
    pub vertical: bool,
    /// `quarter`, `half`, `full` size hint.
    pub size: Option<String>,
    pub has_custom_prompt: bool,
}

/// Shape geometry: a preset name with adjust values, or a custom path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Geom {
    Preset {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        adj: Vec<f64>,
    },
    /// Custom geometry: sub-paths in their own coordinate space (w, h) scaled to the shape.
    Custom { paths: Vec<CustomPath> },
}

impl Default for Geom {
    fn default() -> Self {
        Geom::Preset { name: "rect".into(), adj: vec![] }
    }
}

impl Geom {
    pub fn preset(name: &str) -> Self {
        Geom::Preset { name: name.into(), adj: vec![] }
    }
    pub fn preset_name(&self) -> Option<&str> {
        match self {
            Geom::Preset { name, .. } => Some(name),
            Geom::Custom { .. } => None,
        }
    }
}

/// One sub-path of a custom geometry: SVG-like commands in path units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomPath {
    pub w: f64,
    pub h: f64,
    /// `M x y`, `L x y`, `C x1 y1 x2 y2 x y`, `Q x1 y1 x y`, `Z`.
    pub d: String,
    pub fill: deckcraft_geom::preset::FillMode,
    pub stroke: bool,
}

impl Default for CustomPath {
    fn default() -> Self {
        CustomPath { w: 0.0, h: 0.0, d: String::new(), fill: deckcraft_geom::preset::FillMode::Norm, stroke: true }
    }
}

/// Embedded audio or video on a slide.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MediaClip {
    pub media: MediaId,
    pub video: bool,
    /// Poster frame image.
    pub poster: Option<MediaId>,
    /// Crop of the picture (left, top, right, bottom as fractions), PowerPoint's `srcRect` on the video.
    pub crop: [f64; 4],
    pub trim_start_ms: u32,
    pub trim_end_ms: u32,
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
    /// 0–1.
    pub volume: f64,
    pub autoplay: bool,
    pub loop_play: bool,
    pub rewind: bool,
    pub play_across_slides: bool,
    pub hide_while_not_playing: bool,
    pub full_screen: bool,
    /// Bookmarks (name, ms).
    pub bookmarks: Vec<(String, u32)>,
    /// Probed length in ms (0 = unknown).
    pub duration_ms: u32,
    /// Probed picture size of a video in pixels (0 = unknown).
    pub width: u32,
    pub height: u32,
}

/// One ink stroke (Draw tab): points in slide coordinates with pressure (`0..=1`,
/// where `1` is [`InkStroke::width`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InkStroke {
    pub points: Vec<(f64, f64, f32)>,
    pub color: Rgba,
    pub width: f64,
    pub highlighter: bool,
}

/// True when every sample is the placeholder `0.5` written before a device reported pressure.
/// Those strokes were drawn at [`InkStroke::width`] and still are.
pub fn ink_pressure_is_legacy(points: &[(f64, f64, f32)]) -> bool {
    !points.is_empty() && points.iter().all(|p| (p.2 - 0.5).abs() <= 1.0e-3)
}

/// Width of one ink sample. `pressure` of `1` is the pen width. A legacy flat stroke
/// (`legacy`) draws at the pen width. A missing reading draws at the pen width too.
/// A light touch stays visible (at least 8% of the pen).
pub fn ink_sample_width(pen: f64, pressure: f32, legacy: bool) -> f64 {
    let pen = if pen.is_finite() { pen.abs() } else { 0.0 };
    if legacy {
        return pen;
    }
    let p = if pressure.is_finite() { pressure.clamp(0.08, 1.0) } else { 1.0 };
    pen * f64::from(p)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ShapeKind {
    /// An autoshape or text box.
    Shape,
    Picture {
        fill: style::PictureFill,
    },
    Group {
        children: Vec<Shape>,
        /// Child coordinate space.
        child: Xfrm,
    },
    Connector {
        start: Option<(ShapeId, u32)>,
        end: Option<(ShapeId, u32)>,
    },
    Table(Table),
    Chart(Box<Chart>),
    Media(MediaClip),
    Ink {
        strokes: Vec<InkStroke>,
    },
    /// A 3-D model (`p:contentPart` with a `model3d` relationship, usually a `.glb`).
    /// `poster` is the fallback picture an `mc:AlternateContent` choice carried for older readers.
    Model3d {
        media: MediaId,
        #[serde(default)]
        poster: Option<MediaId>,
    },
    /// Something we read but don't model (SmartArt without fallback, OLE…): its XML is kept and its
    /// fallback picture, if any, is shown.
    Opaque {
        xml: String,
        preview: Option<MediaId>,
        label: String,
        /// The parts its XML points to (SmartArt data, an OLE embedding…), so a save writes them
        /// back: (relationship id used in `xml`, relationship type, the part as a media item).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parts: Vec<(String, String, MediaId)>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Shape {
    pub id: ShapeId,
    pub name: String,
    /// `None`: placeholder inheriting its box from the layout/master.
    pub xfrm: Option<Xfrm>,
    pub kind: ShapeKind,
    pub geom: Geom,
    pub ph: Option<Placeholder>,
    /// `None`: inherited (placeholder chain or style); `Some(Fill::None)`: no fill.
    pub fill: Option<Fill>,
    pub line: Option<Line>,
    pub effects: Option<Effects>,
    pub style: Option<ShapeStyle>,
    pub text: Option<TextBody>,
    pub hidden: bool,
    pub locked: bool,
    /// Text box (`txBox="1"`): no fill, autosize, wrap off by default.
    pub text_box: bool,
    /// Alt text.
    pub descr: String,
    pub title: String,
    pub decorative: bool,
    pub click: Option<text::Hyperlink>,
    pub hover: Option<text::Hyperlink>,
    /// Add-in tags (`p:custDataLst` → `ppt/tags/tagN.xml`): name/value pairs kept for round trip.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<(String, String)>,
    /// Unmodelled XML kept for round-trip (extension lists etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_ext: Option<String>,
    /// On a SmartArt group: the diagram's text outline, from which its shapes are regenerated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagram: Option<Diagram>,
}

/// A SmartArt diagram's data: its layout and its outline of nodes (the Text Pane).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Diagram {
    /// The layout: list, blockList, hBulletList, process, chevron, verticalProcess, stepUp, cycle,
    /// blockCycle, radial, hierarchy, pyramid, invertedPyramid, matrix or titledMatrix.
    pub kind: String,
    pub nodes: Vec<DiagramNode>,
    /// SmartArt Style: `flat` (default), `subtle`, `intense` or `outline`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub style: String,
    /// Change Colors: `colorful` (default: each shape its own accent) or `accent1`…`accent6`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub colors: String,
    /// The group's shapes are the file's drawing, not generated from `nodes`: text typed into them
    /// isn't mapped back to nodes (the first outline edit regenerates them).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub imported: bool,
    /// PowerPoint's layout id (`layoutDef@uniqueId`) the diagram came with.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub layout_id: String,
    /// The file's own layout, quick style and colour definitions, written back unchanged while the
    /// layout isn't switched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout_xml: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_xml: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors_xml: Option<String>,
}

/// One line of the Text Pane: its text and outline level (0 = top level).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiagramNode {
    pub text: String,
    pub level: u8,
    /// The picture shown in the node's picture placeholder (picture layouts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<MediaId>,
    /// An assistant (organisation charts): drawn beside its parent, off the row of its siblings.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub assistant: bool,
    /// The job title under the name (Name and Title Organization Chart).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            id: ShapeId(0),
            name: String::new(),
            xfrm: None,
            kind: ShapeKind::Shape,
            geom: Geom::default(),
            ph: None,
            fill: None,
            line: None,
            effects: None,
            style: None,
            text: None,
            hidden: false,
            locked: false,
            text_box: false,
            descr: String::new(),
            title: String::new(),
            decorative: false,
            click: None,
            hover: None,
            tags: vec![],
            raw_ext: None,
            diagram: None,
        }
    }
}

impl Shape {
    /// A stand-in shape for laying out one table cell's `body`: a table, so the cell's text takes
    /// the master's other-text style as PowerPoint's does.
    pub fn table_cell(body: text::TextBody) -> Shape {
        Shape { text: Some(body), kind: ShapeKind::Table(Table::default()), ..Default::default() }
    }
    pub fn is_placeholder(&self) -> bool {
        self.ph.is_some()
    }
    pub fn ph_type(&self) -> Option<PhType> {
        self.ph.as_ref().map(|p| p.kind)
    }
    pub fn is_group(&self) -> bool {
        matches!(self.kind, ShapeKind::Group { .. })
    }
    pub fn children(&self) -> &[Shape] {
        match &self.kind {
            ShapeKind::Group { children, .. } => children,
            _ => &[],
        }
    }
    pub fn children_mut(&mut self) -> Option<&mut Vec<Shape>> {
        match &mut self.kind {
            ShapeKind::Group { children, .. } => Some(children),
            _ => None,
        }
    }
    /// Is this a line-like shape (no area)?
    pub fn is_line(&self) -> bool {
        matches!(self.kind, ShapeKind::Connector { .. }) || self.geom.preset_name().is_some_and(deckcraft_geom::preset::is_line_like)
    }
    pub fn kind_name(&self) -> &'static str {
        match &self.kind {
            ShapeKind::Shape if self.ph.is_some() => "placeholder",
            ShapeKind::Shape if self.text_box => "textBox",
            ShapeKind::Shape => "shape",
            ShapeKind::Picture { .. } => "picture",
            ShapeKind::Group { .. } => "group",
            ShapeKind::Connector { .. } => "connector",
            ShapeKind::Table(_) => "table",
            ShapeKind::Chart(_) => "chart",
            ShapeKind::Media(m) if m.video => "video",
            ShapeKind::Media(_) => "audio",
            ShapeKind::Ink { .. } => "ink",
            ShapeKind::Model3d { .. } => "model",
            ShapeKind::Opaque { .. } => "object",
        }
    }
}

/// Walk shapes depth-first (groups included).
pub fn walk<'a>(shapes: &'a [Shape], f: &mut dyn FnMut(&'a Shape, usize)) {
    fn rec<'a>(shapes: &'a [Shape], depth: usize, f: &mut dyn FnMut(&'a Shape, usize)) {
        if depth > 64 {
            return;
        }
        for s in shapes {
            f(s, depth);
            rec(s.children(), depth + 1, f);
        }
    }
    rec(shapes, 0, f);
}

pub fn find_shape(shapes: &[Shape], id: ShapeId) -> Option<&Shape> {
    fn rec(shapes: &[Shape], id: ShapeId, depth: usize) -> Option<&Shape> {
        if depth > 64 {
            return None;
        }
        for s in shapes {
            if s.id == id {
                return Some(s);
            }
            if let Some(f) = rec(s.children(), id, depth + 1) {
                return Some(f);
            }
        }
        None
    }
    rec(shapes, id, 0)
}

pub fn find_shape_mut(shapes: &mut [Shape], id: ShapeId) -> Option<&mut Shape> {
    fn rec(shapes: &mut [Shape], id: ShapeId, depth: usize) -> Option<&mut Shape> {
        if depth > 64 {
            return None;
        }
        for s in shapes.iter_mut() {
            if s.id == id {
                return Some(s);
            }
            if let ShapeKind::Group { children, .. } = &mut s.kind
                && let Some(f) = rec(children, id, depth + 1)
            {
                return Some(f);
            }
        }
        None
    }
    rec(shapes, id, 0)
}

/// The list holding `id` among its siblings: the tree's top level, or a group's children.
pub fn sibling_list_mut(shapes: &mut Vec<Shape>, id: ShapeId) -> Option<&mut Vec<Shape>> {
    fn rec(shapes: &mut Vec<Shape>, id: ShapeId, depth: usize) -> Option<&mut Vec<Shape>> {
        if depth > 64 {
            return None;
        }
        if shapes.iter().any(|s| s.id == id) {
            return Some(shapes);
        }
        for s in shapes.iter_mut() {
            if let ShapeKind::Group { children, .. } = &mut s.kind
                && let Some(f) = rec(children, id, depth + 1)
            {
                return Some(f);
            }
        }
        None
    }
    rec(shapes, id, 0)
}

/// The group directly holding `id`: `Some(None)` for a top-level shape, `None` when absent.
pub fn parent_of(shapes: &[Shape], id: ShapeId) -> Option<Option<ShapeId>> {
    fn rec(shapes: &[Shape], parent: Option<ShapeId>, id: ShapeId, depth: usize) -> Option<Option<ShapeId>> {
        if depth > 64 {
            return None;
        }
        for s in shapes {
            if s.id == id {
                return Some(parent);
            }
            if let Some(f) = rec(s.children(), Some(s.id), id, depth + 1) {
                return Some(f);
            }
        }
        None
    }
    rec(shapes, None, id, 0)
}

/// Background of a slide, layout or master.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Background {
    Fill {
        fill: Fill,
    },
    /// Theme background style reference (1001–1003) with a colour.
    Ref {
        idx: u32,
        color: ColorRef,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Comment {
    pub author: String,
    pub initials: String,
    pub text: String,
    /// ISO 8601.
    pub date: String,
    pub x: f64,
    pub y: f64,
    pub resolved: bool,
    pub replies: Vec<Comment>,
    /// The shape the comment is attached to.
    pub shape: Option<ShapeId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Slide {
    pub id: SlideId,
    pub layout: LayoutId,
    pub name: String,
    pub shapes: Vec<Shape>,
    pub background: Option<Background>,
    pub hidden: bool,
    pub show_master_shapes: bool,
    pub transition: Option<Transition>,
    pub animations: Vec<Animation>,
    /// Speaker notes.
    pub notes: TextBody,
    /// Whether the slide has an associated notes slide even if its text is empty.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_notes_slide: bool,
    pub comments: Vec<Comment>,
    /// `clrMapOvr`: a colour map replacing the layout's or master's (`bg1`→`dk1`…); None follows
    /// the layout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_map: Option<Vec<(String, String)>>,
    /// Add-in tags (`p:custDataLst` → `ppt/tags/tagN.xml`): name/value pairs kept for round trip.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<(String, String)>,
    /// Unmodelled XML kept for round-trip (custom data).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_ext: Option<String>,
}

impl Default for Slide {
    fn default() -> Self {
        Slide {
            id: SlideId(0),
            layout: LayoutId(0),
            name: String::new(),
            shapes: vec![],
            background: None,
            hidden: false,
            show_master_shapes: true,
            transition: None,
            animations: vec![],
            notes: TextBody::default(),
            has_notes_slide: false,
            comments: vec![],
            color_map: None,
            tags: vec![],
            raw_ext: None,
        }
    }
}

impl Slide {
    pub fn shape(&self, id: ShapeId) -> Option<&Shape> {
        find_shape(&self.shapes, id)
    }
    pub fn shape_mut(&mut self, id: ShapeId) -> Option<&mut Shape> {
        find_shape_mut(&mut self.shapes, id)
    }
    /// The slide's title text (first title placeholder), for thumbnails, outline and navigation.
    pub fn title(&self) -> String {
        self.shapes
            .iter()
            .find(|s| s.ph_type().is_some_and(PhType::is_title))
            .and_then(|s| s.text.as_ref())
            .map(|t| t.text().replace('\u{b}', " "))
            .unwrap_or_default()
    }
    pub fn notes_text(&self) -> String {
        self.notes.text()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayoutType {
    Title,
    #[default]
    TitleAndContent,
    SectionHeader,
    TwoContent,
    Comparison,
    TitleOnly,
    Blank,
    ContentWithCaption,
    PictureWithCaption,
    TitleAndVerticalText,
    VerticalTitleAndText,
    Custom,
}

impl LayoutType {
    pub fn xml(self) -> &'static str {
        match self {
            LayoutType::Title => "title",
            LayoutType::TitleAndContent => "obj",
            LayoutType::SectionHeader => "secHead",
            LayoutType::TwoContent => "twoObj",
            LayoutType::Comparison => "twoTxTwoObj",
            LayoutType::TitleOnly => "titleOnly",
            LayoutType::Blank => "blank",
            LayoutType::ContentWithCaption => "objTx",
            LayoutType::PictureWithCaption => "picTx",
            LayoutType::TitleAndVerticalText => "vertTx",
            LayoutType::VerticalTitleAndText => "vertTitleAndTx",
            LayoutType::Custom => "cust",
        }
    }
    pub fn from_xml(s: &str) -> Self {
        match s {
            "title" => LayoutType::Title,
            "obj" | "tx" => LayoutType::TitleAndContent,
            "secHead" => LayoutType::SectionHeader,
            "twoObj" | "twoColTx" => LayoutType::TwoContent,
            "twoTxTwoObj" => LayoutType::Comparison,
            "titleOnly" => LayoutType::TitleOnly,
            "blank" => LayoutType::Blank,
            "objTx" => LayoutType::ContentWithCaption,
            "picTx" => LayoutType::PictureWithCaption,
            "vertTx" => LayoutType::TitleAndVerticalText,
            "vertTitleAndTx" => LayoutType::VerticalTitleAndText,
            _ => LayoutType::Custom,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Layout {
    pub id: LayoutId,
    pub name: String,
    pub kind: LayoutType,
    pub shapes: Vec<Shape>,
    pub background: Option<Background>,
    pub show_master_shapes: bool,
    pub preserve: bool,
    /// `clrMapOvr`: a colour map replacing the master's for this layout and the slides using it
    /// (dark "section" layouts map `bg1` to `dk1`); None follows the master.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_map: Option<Vec<(String, String)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_ext: Option<String>,
}

/// Which placeholders slides show in the footer row (Header & Footer dialog).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HeaderFooter {
    pub date: bool,
    pub slide_number: bool,
    pub footer: bool,
    pub header: bool,
    pub footer_text: String,
    /// Fixed date text; empty = automatic date.
    pub date_text: String,
    /// `datetime1`…`datetime13` format for automatic dates.
    pub date_format: String,
    pub hide_on_title: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Master {
    pub id: MasterId,
    pub name: String,
    pub theme: Theme,
    pub shapes: Vec<Shape>,
    pub background: Option<Background>,
    pub title_style: text::ListStyle,
    pub body_style: text::ListStyle,
    pub other_style: text::ListStyle,
    pub layouts: Vec<Layout>,
    /// `bg1`→`lt1` etc. Default mapping when empty.
    pub color_map: Vec<(String, String)>,
    pub preserve: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_ext: Option<String>,
}

impl Master {
    pub fn layout(&self, id: LayoutId) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.id == id)
    }
    /// Resolve `bg1`/`tx1`/`bg2`/`tx2` through the colour map.
    pub fn map_slot(&self, slot: SchemeSlot) -> SchemeSlot {
        map_slot(&self.color_map, slot)
    }
    /// The theme's colour scheme with the colour map applied (`bg1`… resolve to mapped colours).
    pub fn scheme(&self) -> ColorScheme {
        self.scheme_with(&self.color_map)
    }
    /// The theme's colour scheme with `map` (a layout's or slide's override, or the master's own)
    /// applied.
    pub fn scheme_with(&self, map: &[(String, String)]) -> ColorScheme {
        let mut s = self.theme.colors.clone();
        if map.is_empty() {
            return s;
        }
        let base = self.theme.colors.clone();
        for (alias, slot) in [
            (SchemeSlot::Bg1, SchemeSlot::Lt1),
            (SchemeSlot::Tx1, SchemeSlot::Dk1),
            (SchemeSlot::Bg2, SchemeSlot::Lt2),
            (SchemeSlot::Tx2, SchemeSlot::Dk2),
        ] {
            s.set(slot, base.get(map_slot(map, alias)));
        }
        s
    }
}

/// Resolve `bg1`/`tx1`/`bg2`/`tx2` through the colour map `map` (the default mapping when it has
/// no entry).
pub fn map_slot(map: &[(String, String)], slot: SchemeSlot) -> SchemeSlot {
    let key = match slot {
        SchemeSlot::Bg1 => "bg1",
        SchemeSlot::Tx1 => "tx1",
        SchemeSlot::Bg2 => "bg2",
        SchemeSlot::Tx2 => "tx2",
        other => return other,
    };
    map.iter().find(|(k, _)| k == key).and_then(|(_, v)| SchemeSlot::from_xml(v)).unwrap_or(match slot {
        SchemeSlot::Bg1 => SchemeSlot::Lt1,
        SchemeSlot::Tx1 => SchemeSlot::Dk1,
        SchemeSlot::Bg2 => SchemeSlot::Lt2,
        _ => SchemeSlot::Dk2,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Section {
    pub name: String,
    pub slides: Vec<SlideId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CustomShow {
    pub name: String,
    pub slides: Vec<SlideId>,
}

/// Slide Show ▸ Set Up Slide Show.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShowSettings {
    /// `speaker`, `browsed`, `kiosk`.
    pub show_type: String,
    pub loop_until_esc: bool,
    pub without_narration: bool,
    pub without_animation: bool,
    pub use_timings: bool,
    /// 1-based inclusive range; `None` = all.
    pub range: Option<(u32, u32)>,
    pub custom_show: Option<String>,
    pub pen_color: Rgba,
    pub laser_color: Rgba,
    pub show_media_controls: bool,
}

impl Default for ShowSettings {
    fn default() -> Self {
        ShowSettings {
            show_type: "speaker".into(),
            loop_until_esc: false,
            without_narration: false,
            without_animation: false,
            use_timings: true,
            range: None,
            custom_show: None,
            pen_color: Rgba::rgb(0xFF, 0x00, 0x00),
            laser_color: Rgba::rgb(0xFF, 0x00, 0x00),
            show_media_controls: true,
        }
    }
}

/// Document properties (File ▸ Properties).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Properties {
    pub title: String,
    pub subject: String,
    pub author: String,
    pub keywords: String,
    pub comments: String,
    pub category: String,
    pub company: String,
    pub created: String,
    pub modified: String,
    pub last_modified_by: String,
    pub revision: u32,
}

/// One typeface embedded in the presentation (`p:embeddedFont`). Each slot is a font file in
/// [`Presentation::media`]: regular, bold, italic, bold italic. A slot is absent when the file
/// didn't include that face.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmbeddedFont {
    pub typeface: String,
    /// `panose` on `p:font`, when the file gave the 20 hex digits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panose: Option<String>,
    /// `pitchFamily` on `p:font`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_family: Option<u8>,
    /// `charset` on `p:font`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charset: Option<u8>,
    pub regular: Option<MediaId>,
    pub bold: Option<MediaId>,
    pub italic: Option<MediaId>,
    pub bold_italic: Option<MediaId>,
}

impl EmbeddedFont {
    /// The four face slots, in the order the package stores them.
    pub fn faces(&self) -> [Option<MediaId>; 4] {
        [self.regular, self.bold, self.italic, self.bold_italic]
    }
}

/// An embedded file (image, audio, video, font, workbook).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MediaItem {
    pub id: MediaId,
    /// Original file name, e.g. `image1.png`.
    pub name: String,
    pub content_type: String,
    /// Bytes; kept out of the JSON manifest and stored as a separate file in native files.
    #[serde(skip)]
    pub data: Arc<Vec<u8>>,
    /// External link instead of embedded bytes.
    pub link: Option<String>,
}

/// A size whose sides may be missing, null or not numbers (NaN then); see `Presentation::slide_size`.
fn lenient_size<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Size, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let side = |k: &str| v.get(k).and_then(serde_json::Value::as_f64).unwrap_or(f64::NAN);
    Ok(Size::new(side("width"), side("height")))
}

/// An XML package part DeckCraft doesn't model, kept as written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeptPart {
    /// Part name from the package root (`customXml/item1.xml`).
    pub name: String,
    /// Content type; empty for a relationships part.
    pub content_type: String,
    pub xml: String,
    /// Full relationship type linking it, empty when only its sibling parts point at it.
    pub rel_type: String,
    /// Linked from the package root rather than from the presentation part.
    pub from_root: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Presentation {
    /// Slide size in points (default 16:9, 960 × 540). Read leniently: a missing, null or
    /// non-numeric side becomes NaN for `deckcraft_format::repair` to replace, instead of the
    /// whole file failing to open.
    #[serde(deserialize_with = "lenient_size")]
    pub slide_size: Size,
    #[serde(deserialize_with = "lenient_size")]
    pub notes_size: Size,
    pub slides: Vec<Arc<Slide>>,
    pub masters: Vec<Arc<Master>>,
    pub notes_master: Option<Arc<Master>>,
    pub handout_master: Option<Arc<Master>>,
    pub sections: Vec<Section>,
    pub custom_shows: Vec<CustomShow>,
    pub show: ShowSettings,
    pub header_footer: HeaderFooter,
    pub props: Properties,
    /// Presentation-wide default text style (shapes that aren't placeholders).
    pub default_text_style: text::ListStyle,
    pub media: Vec<MediaItem>,
    pub first_slide_number: u32,
    /// Next free shape/slide/layout/media id.
    pub next_id: u32,
    /// Embedded fonts. The face bytes live in [`Self::media`].
    pub embedded_fonts: Vec<EmbeddedFont>,
    /// Table styles defined in the file (`ppt/tableStyles.xml`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub table_styles: Vec<table::TableStyleDef>,
    /// Add-in tags (`p:custDataLst` → `ppt/tags/tagN.xml`): name/value pairs kept for round trip.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<(String, String)>,
    /// Package parts kept verbatim for round trip: custom XML data (SharePoint metadata), the
    /// sensitivity label and custom document properties.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_parts: Vec<KeptPart>,
    /// Drawing guides (View › Guides), shared by every slide (`p:guideLst` in viewProps).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<DrawGuide>,
    /// Unmodelled presentation-level XML kept for round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_ext: Option<String>,
}

/// A drawing guide: a vertical line at `pos` points from the slide's left edge, or a horizontal
/// one from its top.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrawGuide {
    pub vertical: bool,
    pub pos: f64,
}

impl Default for Presentation {
    fn default() -> Self {
        defaults::new_presentation(None)
    }
}

impl Presentation {
    /// A fresh id, unique across slides, shapes, layouts, masters and media in this deck. `None`
    /// once every id is in use (a hostile file can arrive that way): the deck cannot grow
    /// further, and callers must refuse the change rather than duplicate ids (#407).
    pub fn alloc_id(&mut self) -> Option<u32> {
        let next = self.next_id.max(1).checked_add(1)?;
        self.next_id = next;
        Some(next)
    }
    /// Make sure `next_id` is above every id in use (after loading a file).
    pub fn fix_next_id(&mut self) {
        let mut m = self.next_id;
        for s in &self.slides {
            m = m.max(s.id.0);
            walk(&s.shapes, &mut |sh, _| m = m.max(sh.id.0));
        }
        for ma in &self.masters {
            m = m.max(ma.id.0);
            walk(&ma.shapes, &mut |sh, _| m = m.max(sh.id.0));
            for l in &ma.layouts {
                m = m.max(l.id.0);
                walk(&l.shapes, &mut |sh, _| m = m.max(sh.id.0));
            }
        }
        for md in &self.media {
            m = m.max(md.id.0);
        }
        for extra in [self.notes_master.as_deref(), self.handout_master.as_deref()].into_iter().flatten() {
            m = m.max(extra.id.0);
            walk(&extra.shapes, &mut |sh, _| m = m.max(sh.id.0));
        }
        self.next_id = m;
    }
    pub fn slide_index(&self, id: SlideId) -> Option<usize> {
        self.slides.iter().position(|s| s.id == id)
    }
    pub fn slide(&self, id: SlideId) -> Option<&Slide> {
        self.slides.iter().find(|s| s.id == id).map(AsRef::as_ref)
    }
    pub fn slide_mut(&mut self, id: SlideId) -> Option<&mut Slide> {
        self.slides.iter_mut().find(|s| s.id == id).map(Arc::make_mut)
    }
    pub fn slide_at(&self, i: usize) -> Option<&Slide> {
        self.slides.get(i).map(AsRef::as_ref)
    }
    /// Master and layout for a layout id.
    pub fn layout(&self, id: LayoutId) -> Option<(&Master, &Layout)> {
        self.masters.iter().find_map(|m| m.layout(id).map(|l| (m.as_ref(), l)))
    }
    /// The master (and layout) a slide uses, falling back to the first.
    pub fn master_for(&self, slide: &Slide) -> Option<(&Master, Option<&Layout>)> {
        if let Some((m, l)) = self.layout(slide.layout) {
            return Some((m, Some(l)));
        }
        self.masters.first().map(|m| (m.as_ref(), m.layouts.first()))
    }
    pub fn first_master(&self) -> Option<&Master> {
        self.masters.first().map(AsRef::as_ref)
    }
    pub fn media(&self, id: MediaId) -> Option<&MediaItem> {
        self.media.iter().find(|m| m.id == id)
    }
    /// Add media bytes, reusing an identical existing item.
    pub fn add_media(&mut self, name: &str, content_type: &str, data: Vec<u8>) -> Option<MediaId> {
        if let Some(m) = self.media.iter().find(|m| m.data.len() == data.len() && *m.data == data) {
            return Some(m.id);
        }
        let id = MediaId(self.alloc_id()?);
        self.media.push(MediaItem { id, name: name.into(), content_type: content_type.into(), data: Arc::new(data), link: None });
        Some(id)
    }
    /// Section index containing a slide (by position), when sections exist.
    pub fn section_of(&self, slide: SlideId) -> Option<usize> {
        self.sections.iter().position(|s| s.slides.contains(&slide))
    }
    /// 1-based slide number as displayed (respects `first_slide_number`).
    pub fn slide_number(&self, index: usize) -> u32 {
        self.first_slide_number.saturating_add(u32::try_from(index).unwrap_or(u32::MAX))
    }
    /// Basic consistency check: unique slide/shape ids, layouts exist. Returns problems found.
    pub fn validate(&self) -> Vec<String> {
        let mut out = vec![];
        let mut seen = std::collections::HashSet::new();
        // Indexed once: a per-slide scan of every layout is slides × layouts work.
        let layouts: std::collections::HashSet<_> = self.masters.iter().flat_map(|m| m.layouts.iter().map(|l| l.id)).collect();
        for s in &self.slides {
            if !seen.insert(s.id) {
                out.push(format!("duplicate slide id {}", s.id));
            }
            if !layouts.contains(&s.layout) {
                out.push(format!("slide {} uses missing layout {}", s.id, s.layout));
            }
            let mut ids = std::collections::HashSet::new();
            walk(&s.shapes, &mut |sh, _| {
                if !ids.insert(sh.id) {
                    out.push(format!("slide {}: duplicate shape id {}", s.id, sh.id));
                }
            });
        }
        out
    }
}

#[cfg(test)]
mod tests;
