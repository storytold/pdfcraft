//! The template model: what the layout engine needs from an XFA template (XFA 3.3 part 2,
//! "Template Specification"). Every measurement is in points, every position is top-left
//! based, as in XFA itself.

/// Whether an object is shown and whether it takes space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presence {
    #[default]
    Visible,
    /// Takes space, draws nothing.
    Invisible,
    /// Takes no space.
    Hidden,
    /// Hidden, and its scripts don't run.
    Inactive,
}

impl Presence {
    pub fn parse(s: Option<&str>) -> Presence {
        match s.map(str::trim) {
            Some("invisible") => Presence::Invisible,
            Some("hidden") => Presence::Hidden,
            Some("inactive") => Presence::Inactive,
            _ => Presence::Visible,
        }
    }

    /// Takes space in the layout.
    pub fn occupies(self) -> bool {
        matches!(self, Presence::Visible | Presence::Invisible)
    }
}

/// How a subform places its children.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Positioned,
    /// Top to bottom.
    Tb,
    /// Left to right, then top to bottom.
    LrTb,
    /// Right to left, then top to bottom (laid out like `LrTb`, mirrored).
    RlTb,
    Table,
    Row,
}

impl Layout {
    pub fn parse(s: Option<&str>) -> Layout {
        match s.map(str::trim) {
            Some("tb") => Layout::Tb,
            Some("lr-tb") => Layout::LrTb,
            Some("rl-tb") => Layout::RlTb,
            Some("table") => Layout::Table,
            Some("row") => Layout::Row,
            _ => Layout::Positioned,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Margin {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Margin {
    pub fn horizontal(&self) -> f64 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f64 {
        self.top + self.bottom
    }
}

/// An RGB colour, components 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub [f64; 3]);

impl Color {
    pub const BLACK: Color = Color([0.0, 0.0, 0.0]);
    pub const WHITE: Color = Color([1.0, 1.0, 1.0]);

    /// `value="r,g,b"` with components 0–255.
    pub fn parse(s: &str) -> Option<Color> {
        let mut parts = s.split(',').map(|p| p.trim().parse::<f64>().ok());
        let (r, g, b) = (parts.next()??, parts.next()??, parts.next()??);
        let c = |v: f64| if v.is_finite() { (v / 255.0).clamp(0.0, 1.0) } else { 0.0 };
        Some(Color([c(r), c(g), c(b)]))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stroke {
    #[default]
    Solid,
    Dashed,
    Dotted,
    /// Raised, lowered, etched and embossed: drawn as a solid grey line.
    Relief,
}

/// One side of a border.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub presence: Presence,
    pub thickness: f64,
    pub color: Color,
    pub stroke: Stroke,
}

impl Default for Edge {
    fn default() -> Self {
        Edge { presence: Presence::Visible, thickness: 0.5, color: Color::BLACK, stroke: Stroke::Solid }
    }
}

impl Edge {
    pub fn visible(&self) -> bool {
        self.presence == Presence::Visible && self.thickness > 0.0
    }
}

/// A box border: four edges (top, right, bottom, left) and a fill.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Border {
    pub presence: Presence,
    pub edges: [Edge; 4],
    pub fill: Option<Color>,
}

impl Border {
    pub fn visible_edges(&self) -> [bool; 4] {
        if self.presence != Presence::Visible {
            return [false; 4];
        }
        [self.edges[0].visible(), self.edges[1].visible(), self.edges[2].visible(), self.edges[3].visible()]
    }

    pub fn fill(&self) -> Option<Color> {
        if self.presence == Presence::Visible { self.fill } else { None }
    }
}

/// Font properties as written on an element; `None` inherits from the ancestors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontSpec {
    pub typeface: Option<String>,
    pub size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub color: Option<Color>,
}

/// A font with every property resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub typeface: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Color,
}

impl Default for Font {
    /// XFA's default font: Courier 10 pt.
    fn default() -> Self {
        Font { typeface: "Courier".into(), size: 10.0, bold: false, italic: false, underline: false, color: Color::BLACK }
    }
}

impl Font {
    pub fn apply(&self, spec: &FontSpec) -> Font {
        Font {
            typeface: spec.typeface.clone().unwrap_or_else(|| self.typeface.clone()),
            size: spec.size.unwrap_or(self.size),
            bold: spec.bold.unwrap_or(self.bold),
            italic: spec.italic.unwrap_or(self.italic),
            underline: spec.underline.unwrap_or(self.underline),
            color: spec.color.unwrap_or(self.color),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl HAlign {
    pub fn parse(s: Option<&str>) -> Option<HAlign> {
        match s.map(str::trim) {
            Some("left") => Some(HAlign::Left),
            Some("center") => Some(HAlign::Center),
            Some("right") => Some(HAlign::Right),
            Some("justify" | "justifyAll") => Some(HAlign::Justify),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

impl VAlign {
    pub fn parse(s: Option<&str>) -> Option<VAlign> {
        match s.map(str::trim) {
            Some("top") => Some(VAlign::Top),
            Some("middle") => Some(VAlign::Middle),
            Some("bottom") => Some(VAlign::Bottom),
            _ => None,
        }
    }
}

/// Paragraph properties as written on an element; `None` inherits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParaSpec {
    pub h_align: Option<HAlign>,
    pub v_align: Option<VAlign>,
    pub line_height: Option<f64>,
    pub margin_left: Option<f64>,
    pub margin_right: Option<f64>,
    pub space_above: Option<f64>,
    pub space_below: Option<f64>,
    pub text_indent: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Para {
    pub h_align: HAlign,
    pub v_align: VAlign,
    /// `None`: 1.15 × the font size.
    pub line_height: Option<f64>,
    pub margin_left: f64,
    pub margin_right: f64,
    pub space_above: f64,
    pub space_below: f64,
    pub text_indent: f64,
}

impl Para {
    pub fn apply(&self, spec: &ParaSpec) -> Para {
        Para {
            h_align: spec.h_align.unwrap_or(self.h_align),
            v_align: spec.v_align.unwrap_or(self.v_align),
            line_height: spec.line_height.or(self.line_height),
            margin_left: spec.margin_left.unwrap_or(self.margin_left),
            margin_right: spec.margin_right.unwrap_or(self.margin_right),
            space_above: spec.space_above.unwrap_or(self.space_above),
            space_below: spec.space_below.unwrap_or(self.space_below),
            text_indent: spec.text_indent.unwrap_or(self.text_indent),
        }
    }
}

/// What every container and leaf shares: name, box, presence, margins, border, text defaults.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Common {
    pub name: Option<String>,
    pub id: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub min_w: Option<f64>,
    pub max_w: Option<f64>,
    pub min_h: Option<f64>,
    pub max_h: Option<f64>,
    pub presence: Presence,
    pub margin: Margin,
    pub border: Option<Border>,
    pub font: FontSpec,
    pub para: ParaSpec,
    /// Table cells: how many columns this cell spans.
    pub col_span: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Subform(Box<Subform>),
    Area(Box<Area>),
    Field(Box<Field>),
    Draw(Box<Draw>),
    ExclGroup(Box<ExclGroup>),
}

impl Node {
    pub fn common(&self) -> &Common {
        match self {
            Node::Subform(s) => &s.common,
            Node::Area(a) => &a.common,
            Node::Field(f) => &f.common,
            Node::Draw(d) => &d.common,
            Node::ExclGroup(g) => &g.common,
        }
    }
}

/// How often a subform occurs when no data drives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occur {
    pub min: usize,
    /// `None`: unbounded.
    pub max: Option<usize>,
    pub initial: usize,
}

impl Default for Occur {
    fn default() -> Self {
        Occur { min: 1, max: Some(1), initial: 1 }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subform {
    pub common: Common,
    pub layout: Layout,
    pub children: Vec<Node>,
    /// `<event activity="…"><script>` on the subform (initialize, calculate, …).
    pub scripts: Vec<Script>,
    pub page_set: Option<PageSet>,
    pub occur: Occur,
    /// `<breakBefore targetType="pageArea">`: start on a new page.
    pub break_before_page: bool,
    pub break_after_page: bool,
    /// Tables: the width of each column.
    pub column_widths: Vec<f64>,
    /// `<overflow leader="…">`: the named child (a header row) is repeated after a page break.
    pub overflow_leader: Option<String>,
}

/// A positioned group without a box of its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Area {
    pub common: Common,
    pub children: Vec<Node>,
}

/// A radio button group.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExclGroup {
    pub common: Common,
    pub layout: Layout,
    pub fields: Vec<Field>,
    pub tooltip: Option<String>,
    pub scripts: Vec<Script>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Ui {
    #[default]
    TextEdit,
    NumericEdit,
    DateTimeEdit,
    CheckButton,
    ChoiceList,
    Button,
    Signature,
    ImageEdit,
    Barcode,
    PasswordEdit,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub activity: String,
    pub text: String,
    /// FormCalc (the default language), not JavaScript.
    pub formcalc: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    #[default]
    Open,
    ReadOnly,
    Protected,
    NonInteractive,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Field {
    pub common: Common,
    pub ui: Ui,
    /// The border drawn by the widget itself (`<ui><textEdit><border>`), inside the field.
    pub ui_border: Option<Border>,
    pub ui_margin: Margin,
    /// Check buttons: the side of the square (default 10 pt) and whether it is round.
    pub check_size: Option<f64>,
    pub check_round: bool,
    pub multiline: bool,
    pub caption: Option<Caption>,
    pub value: Value,
    /// Check buttons: the on value then the off value; choice lists: the items.
    pub items: Vec<String>,
    pub max_chars: Option<usize>,
    pub tooltip: Option<String>,
    pub access: Access,
    /// The edit picture clause (`<ui>…<picture>`), e.g. `date{YYYY-MM-DD}`.
    pub picture: Option<String>,
    /// `<event activity="…"><script>`: click, change, exit, initialize, …
    pub scripts: Vec<Script>,
    /// `<calculate><script>`: the value is the script's result.
    pub calculate: Option<Script>,
    /// `<validate><script>`: false (or an exception) shows `validate_message`; the value stays,
    /// as Acrobat marks the field invalid rather than reverting it.
    pub validate: Option<Script>,
    pub validate_message: Option<String>,
    /// Numeric fields (numericEdit, or a decimal/integer/float value) give scripts numbers.
    pub numeric: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draw {
    pub common: Common,
    pub value: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    Empty,
    Text(String),
    Rich(RichText),
    Image {
        content_type: String,
        data: Vec<u8>,
    },
    /// A diagonal line through the box: `\` (top-left to bottom-right) or `/`.
    Line {
        slope_up: bool,
        edge: Edge,
    },
    Rectangle(Border),
}

impl Value {
    pub fn is_empty(&self) -> bool {
        match self {
            Value::Empty => true,
            Value::Text(t) => t.trim().is_empty(),
            Value::Rich(r) => r.paragraphs.iter().all(|p| p.runs.iter().all(|r| r.text.trim().is_empty() && r.embed.is_none())),
            _ => false,
        }
    }

    /// The plain text of a text or rich value.
    pub fn plain(&self) -> Option<String> {
        match self {
            Value::Text(t) => Some(t.clone()),
            Value::Rich(r) => Some(r.plain()),
            _ => None,
        }
    }
}

/// Rich text (`exData` with XHTML): paragraphs of styled runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichText {
    pub paragraphs: Vec<Paragraph>,
}

impl RichText {
    pub fn plain(&self) -> String {
        self.paragraphs.iter().map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    pub h_align: Option<HAlign>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub size: Option<f64>,
    /// A floating field embedded in the text (`xfa:embed="#id"`), shown by its value.
    pub embed: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
    Inline,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Caption {
    pub placement: Placement,
    /// Space set aside for the caption along its placement axis.
    pub reserve: Option<f64>,
    pub presence: Presence,
    pub value: Value,
    pub font: FontSpec,
    pub para: ParaSpec,
}

/// A rectangle in points, top-left based.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h: h.max(0.0) }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn inset(&self, m: &Margin) -> Rect {
        Rect::new(self.x + m.left, self.y + m.top, (self.w - m.horizontal()).max(0.0), (self.h - m.vertical()).max(0.0))
    }
}

/// The master pages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageSet {
    pub areas: Vec<PageArea>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageArea {
    pub name: Option<String>,
    pub width: f64,
    pub height: f64,
    /// Where flowed content goes, in order.
    pub content: Vec<Rect>,
    /// Draws and fields printed on every page that uses this master.
    pub items: Vec<Node>,
    /// How many pages may use this master (`None`: unbounded).
    pub occur_max: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Template {
    pub root: Subform,
    /// Fields that compute the page number or the page count on layout, by `id`, so text that
    /// embeds them can show the numbers.
    pub page_roles: Vec<(String, PageRole)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRole {
    Number,
    Count,
}
