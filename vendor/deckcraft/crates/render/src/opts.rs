use super::{DateTime, Fields, Rgba, ShapeId, Vec2, datetime};

/// Animation state of one shape during a slide show or preview.
#[derive(Clone, Debug)]
pub struct ShapeState {
    pub visible: bool,
    pub opacity: f64,
    /// Applied in slide space around the shape's centre: translation (pt), scale, rotation (deg).
    pub offset: Vec2,
    pub scale: (f64, f64),
    pub rotate: f64,
    /// Reveal clip in shape-local unit space (0..1): l, t, r, b.
    pub clip: Option<[f64; 4]>,
    /// Colour override for emphasis effects (fill).
    pub tint: Option<Rgba>,
    /// Per-paragraph states of by-paragraph text builds (paragraph index, state).
    pub paras: Vec<(usize, ParaState)>,
}

impl Default for ShapeState {
    fn default() -> Self {
        ShapeState { visible: true, opacity: 1.0, offset: Vec2::ZERO, scale: (1.0, 1.0), rotate: 0.0, clip: None, tint: None, paras: vec![] }
    }
}

/// Animation state of one paragraph, relative to its shape.
#[derive(Clone, Copy, Debug)]
pub struct ParaState {
    pub visible: bool,
    pub opacity: f64,
    /// Translation in shape-local points.
    pub offset: Vec2,
}

impl Default for ParaState {
    fn default() -> Self {
        ParaState { visible: true, opacity: 1.0, offset: Vec2::ZERO }
    }
}

pub struct RenderOpts<'a> {
    /// Pixels per point.
    pub scale: f64,
    /// Editor view: placeholder prompts and dashed placeholder outlines.
    pub edit: bool,
    /// Draw shapes marked hidden (Selection pane) — the editor hides them like the show does.
    pub show_hidden: bool,
    /// Per-shape animation state.
    pub state: Option<&'a dyn Fn(ShapeId) -> ShapeState>,
    /// Don't draw these shapes (being edited in place, drawn by the UI).
    pub skip: &'a [ShapeId],
    /// Hide the text of this shape (the UI draws it while editing).
    pub skip_text: Option<ShapeId>,
    /// Text being edited: replace its prompt with gray markers on empty list paragraphs.
    /// Ignored outside editor rendering.
    pub editing_text: Option<ShapeId>,
    /// Animated GIF pictures and how far into their play they are (seconds); others show frame 1.
    pub gif_times: &'a [(ShapeId, f64)],
    /// Worker threads (0 = this thread; required when effects use filters).
    pub threads: u16,
    /// Translate the slide within the output (pixels) and output size override.
    pub offset: (f64, f64),
    pub size: Option<(u32, u32)>,
    /// Draw a background colour outside the slide (output larger than slide).
    pub clear: Option<Rgba>,
    /// Show this text in every date field instead of the date.
    pub date_text: Option<String>,
    /// The local date and time date fields show (`None`: the system clock).
    pub now: Option<DateTime>,
    /// Leave the slide background out (neither the white underlay nor the background fill), so
    /// only what is on the slide is drawn over transparency: PNG export with a transparent
    /// background. Shapes filled with the background (`useBgFill`) still show it.
    pub transparent_background: bool,
}

impl Default for RenderOpts<'_> {
    fn default() -> Self {
        RenderOpts {
            scale: 1.0,
            edit: false,
            show_hidden: false,
            state: None,
            skip: &[],
            skip_text: None,
            editing_text: None,
            gif_times: &[],
            threads: 0,
            offset: (0.0, 0.0),
            size: None,
            clear: None,
            date_text: None,
            now: None,
            transparent_background: false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SlideFields {
    pub(crate) num: u32,
    /// Text for every date field instead of the date ([`RenderOpts::date_text`]).
    pub(crate) date: Option<String>,
    pub(crate) footer: String,
    /// The local date and time date fields show; `None`: they keep their saved text.
    pub(crate) now: Option<DateTime>,
}

impl SlideFields {
    pub(crate) fn new(num: u32, footer: String, opts: &RenderOpts) -> Self {
        SlideFields { num, date: opts.date_text.clone(), footer, now: opts.now.or_else(DateTime::now_local) }
    }
}

impl Fields for SlideFields {
    fn field(&self, kind: &str) -> Option<String> {
        self.field_in(kind, None)
    }

    fn field_in(&self, kind: &str, lang: Option<&str>) -> Option<String> {
        if kind == "slidenum" {
            return Some(self.num.to_string());
        }
        if datetime::is_date_field(kind) {
            // An automatic date: today's, in the format of its type. (A fixed date is plain text
            // in the slide's placeholder, not a field.)
            return self.date.clone().or_else(|| datetime::field_text(kind, lang, self.now?));
        }
        if kind == "footer" {
            return Some(self.footer.clone());
        }
        None
    }
}
