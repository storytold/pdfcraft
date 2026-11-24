//! Slide transitions and object animations.
//!
//! Animations are kept as the flat, ordered list the Animation Pane shows; the PPTX writer builds
//! the nested timing tree from it and the reader flattens files back into it.

use serde::{Deserialize, Serialize};

use crate::ShapeId;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Transition {
    /// Effect id: `none`, `fade`, `push`, `wipe`, `split`, `reveal`, `cut`, `randomBar`, `shape`,
    /// `uncover`, `cover`, `flash`, `morph`, `dissolve`, `checker`, `blinds`, `clock`, `zoom`… (see
    /// [`TRANSITIONS`]).
    pub kind: String,
    /// Effect option, e.g. direction `l`, `r`, `u`, `d`, `horz`, `vert`, `in`, `out`.
    pub option: String,
    pub duration_ms: u32,
    pub advance_on_click: bool,
    pub advance_after_ms: Option<u32>,
    /// Embedded sound media.
    pub sound: Option<crate::MediaId>,
    /// Loop Until Next Sound (`p:stSnd loop="1"`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sound_loop: bool,
    /// [Stop Previous Sound] (`p:endSnd`): the slide stops a sound still playing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stop_sound: bool,
    /// Unmodelled transition XML kept for round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl Default for Transition {
    fn default() -> Self {
        Transition {
            kind: "none".into(),
            option: String::new(),
            duration_ms: 1000,
            advance_on_click: true,
            advance_after_ms: None,
            sound: None,
            sound_loop: false,
            stop_sound: false,
            raw: None,
        }
    }
}

/// (id, label, category, default duration ms, effect options)
pub static TRANSITIONS: &[(&str, &str, &str, u32, &[&str])] = &[
    ("none", "None", "Subtle", 0, &[]),
    ("morph", "Morph", "Subtle", 2000, &["objects", "words", "characters"]),
    ("fade", "Fade", "Subtle", 700, &["smoothly", "throughBlack"]),
    ("push", "Push", "Subtle", 1000, &["u", "d", "l", "r"]),
    ("wipe", "Wipe", "Subtle", 1000, &["r", "l", "u", "d", "ru", "lu", "rd", "ld"]),
    ("split", "Split", "Subtle", 1500, &["vertOut", "vertIn", "horzOut", "horzIn"]),
    ("reveal", "Reveal", "Subtle", 2000, &["smoothlyLeft", "smoothlyRight", "blackLeft", "blackRight"]),
    ("cut", "Cut", "Subtle", 0, &["", "throughBlack"]),
    ("randomBar", "Random Bars", "Subtle", 1000, &["vert", "horz"]),
    ("shape", "Shape", "Subtle", 2000, &["circle", "diamond", "plus", "in", "out"]),
    ("uncover", "Uncover", "Subtle", 1000, &["l", "r", "u", "d", "lu", "ru", "ld", "rd"]),
    ("cover", "Cover", "Subtle", 1000, &["l", "r", "u", "d", "lu", "ru", "ld", "rd"]),
    ("flash", "Flash", "Subtle", 1000, &[]),
    ("fallOver", "Fall Over", "Exciting", 2000, &["l", "r"]),
    ("drape", "Drape", "Exciting", 2000, &["l", "r"]),
    ("curtains", "Curtains", "Exciting", 2000, &[]),
    ("wind", "Wind", "Exciting", 2000, &["l", "r"]),
    ("prestige", "Prestige", "Exciting", 2000, &[]),
    ("fracture", "Fracture", "Exciting", 2000, &[]),
    ("crush", "Crush", "Exciting", 2000, &[]),
    ("peelOff", "Peel Off", "Exciting", 1500, &["l", "r"]),
    ("pageCurlDouble", "Page Curl", "Exciting", 1500, &["l", "r", "singleL", "singleR"]),
    ("airplane", "Airplane", "Exciting", 2000, &["l", "r"]),
    ("origami", "Origami", "Exciting", 2000, &["l", "r"]),
    ("dissolve", "Dissolve", "Exciting", 1200, &[]),
    ("checker", "Checkerboard", "Exciting", 1500, &["horz", "vert"]),
    ("blinds", "Blinds", "Exciting", 1600, &["vert", "horz"]),
    ("clock", "Clock", "Exciting", 2000, &["clockwise", "counterClockwise", "wedge"]),
    ("ripple", "Ripple", "Exciting", 1400, &["center", "lu", "ru", "ld", "rd"]),
    ("honeycomb", "Honeycomb", "Exciting", 3000, &[]),
    ("glitter", "Glitter", "Exciting", 2500, &["l", "r", "u", "d", "hexL", "hexR", "hexU", "hexD"]),
    ("vortex", "Vortex", "Exciting", 3000, &["l", "r", "u", "d"]),
    ("shred", "Shred", "Exciting", 2000, &["stripsIn", "stripsOut", "particlesIn", "particlesOut"]),
    ("switch", "Switch", "Exciting", 1250, &["l", "r"]),
    ("flip", "Flip", "Exciting", 1250, &["l", "r"]),
    ("gallery", "Gallery", "Exciting", 1500, &["l", "r"]),
    ("cube", "Cube", "Exciting", 1250, &["l", "r", "u", "d"]),
    ("doors", "Doors", "Exciting", 1400, &["vert", "horz"]),
    ("box", "Box", "Exciting", 1250, &["l", "r", "u", "d"]),
    ("comb", "Comb", "Exciting", 1000, &["horz", "vert"]),
    ("zoom", "Zoom", "Exciting", 1000, &["in", "out", "rotate"]),
    ("random", "Random", "Exciting", 2000, &[]),
    ("pan", "Pan", "Dynamic Content", 1000, &["u", "d", "l", "r"]),
    ("ferris", "Ferris Wheel", "Dynamic Content", 2000, &["l", "r"]),
    ("conveyor", "Conveyor", "Dynamic Content", 2000, &["l", "r"]),
    ("rotate", "Rotate", "Dynamic Content", 2000, &["l", "r", "u", "d"]),
    ("window", "Window", "Dynamic Content", 1250, &["vert", "horz"]),
    ("orbit", "Orbit", "Dynamic Content", 2000, &["l", "r", "u", "d"]),
    ("flythrough", "Fly Through", "Dynamic Content", 1000, &["in", "out", "inBounce", "outBounce"]),
];

/// The Effect Options label PowerPoint shows for a transition option. Options are stored as the
/// file format's values: for moving transitions (Push, Wipe, Cover, Pan, Cube…) the direction
/// things move, so "l" is the effect coming "From Right"; Glitter and the preset effects (Fall
/// Over, Wind…) name the side itself; Ripple names where the wave heads.
pub fn transition_option_label(kind: &str, option: &str) -> String {
    let side = |c: char| match c {
        'l' => "Left",
        'r' => "Right",
        'u' => "Top",
        _ => "Bottom",
    };
    let opposite = |c: char| match c {
        'l' => 'r',
        'r' => 'l',
        'u' => 'd',
        _ => 'u',
    };
    let corner = |o: &str, flip: bool| -> Option<String> {
        let mut cs = o.chars();
        let (h, v) = (cs.next()?, cs.next()?);
        let (h, v) = if flip { (opposite(h), opposite(v)) } else { (h, v) };
        Some(format!("From {}-{}", side(v), side(h)))
    };
    let moving = matches!(
        kind,
        "push"
            | "wipe"
            | "cover"
            | "uncover"
            | "pan"
            | "vortex"
            | "cube"
            | "box"
            | "rotate"
            | "orbit"
            | "gallery"
            | "conveyor"
            | "ferris"
            | "switch"
            | "flip"
    );
    match (kind, option) {
        (_, "horz") => "Horizontal".into(),
        (_, "vert") => "Vertical".into(),
        ("split", o) => format!("{} {}", if o.starts_with("horz") { "Horizontal" } else { "Vertical" }, if o.ends_with("In") { "In" } else { "Out" }),
        ("fade" | "cut", "throughBlack") => "Through Black".into(),
        ("fade", _) => "Smoothly".into(),
        ("cut", _) => "Cut".into(),
        ("clock", "counterClockwise") => "Counterclockwise".into(),
        ("clock", "wedge") => "Wedge".into(),
        ("clock", _) => "Clockwise".into(),
        ("zoom", "rotate") => "Zoom and Rotate".into(),
        ("ripple", "center") => "Center".into(),
        ("ripple", o) => corner(o, true).unwrap_or_else(|| "Center".into()),
        ("glitter", o) => {
            let (pat, d) = match o.strip_prefix("hex") {
                Some(d) => ("Hexagons", d.to_ascii_lowercase()),
                None => ("Diamonds", o.to_string()),
            };
            format!("{pat} From {}", side(d.chars().next().unwrap_or('l')))
        }
        ("pageCurlDouble", o) => match o {
            "singleL" => "Single Left".into(),
            "singleR" => "Single Right".into(),
            "r" => "Double Right".into(),
            _ => "Double Left".into(),
        },
        ("shred", o) => {
            format!("{} {}", if o.starts_with("particles") { "Particles" } else { "Strips" }, if o.ends_with("Out") { "Out" } else { "In" })
        }
        ("reveal", o) => format!(
            "{} {}",
            if o.starts_with("black") { "Through Black" } else { "Smoothly" },
            if o.ends_with("Right") { "From Right" } else { "From Left" }
        ),
        ("flythrough", o) => {
            format!("{}{}", if o.starts_with("out") { "Out" } else { "In" }, if o.ends_with("Bounce") { " with Bounce" } else { "" })
        }
        ("morph", o) => match o {
            "words" => "Words".into(),
            "characters" => "Characters".into(),
            _ => "Objects".into(),
        },
        (_, o) if o.len() == 2 && moving => corner(o, true).unwrap_or_default(),
        (_, o) if o.len() == 1 && moving => format!("From {}", side(opposite(o.chars().next().unwrap_or('l')))),
        (_, "l") => "Left".into(),
        (_, "r") => "Right".into(),
        (_, "u") => "Top".into(),
        (_, "d") => "Bottom".into(),
        (_, "in") => "In".into(),
        (_, "out") => "Out".into(),
        (_, o) => o.to_string(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnimClass {
    #[default]
    Entrance,
    Emphasis,
    Exit,
    Path,
    Media,
}

impl AnimClass {
    pub fn xml(self) -> &'static str {
        match self {
            AnimClass::Entrance => "entr",
            AnimClass::Emphasis => "emph",
            AnimClass::Exit => "exit",
            AnimClass::Path => "path",
            AnimClass::Media => "mediacall",
        }
    }
    pub fn from_xml(s: &str) -> Self {
        match s {
            "emph" => AnimClass::Emphasis,
            "exit" => AnimClass::Exit,
            "path" => AnimClass::Path,
            "mediacall" => AnimClass::Media,
            _ => AnimClass::Entrance,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnimStart {
    #[default]
    OnClick,
    WithPrevious,
    AfterPrevious,
}

/// Text build: as one object, all at once by paragraph, or by paragraph level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextBuild {
    #[default]
    AsOne,
    ByParagraph,
    ByWord,
    ByLetter,
}

/// Most paragraphs a by-paragraph build animates separately; later ones follow the last of them.
pub const MAX_BUILD_PARAGRAPHS: usize = 1_000;

/// What a paragraph does in a by-paragraph build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildRole {
    /// Animated on its own: a filled paragraph at the shallowest filled level.
    Top,
    /// Follows the top paragraph with this index (deeper, empty or blank paragraphs after it).
    Member(usize),
    /// Before the first top paragraph: not animated.
    Unowned,
}

/// Does the paragraph show any text? A by-paragraph build doesn't animate blank ones on their own.
pub fn build_filled(p: &crate::text::Paragraph) -> bool {
    p.runs.iter().any(|r| !r.text.trim().is_empty())
}

/// The role of every paragraph of `text` in a by-paragraph build, by paragraph index: what the
/// timeline plays, and what the PPTX reader and writer map to one file effect per paragraph.
pub fn paragraph_build_roles(text: &crate::text::TextBody) -> Vec<BuildRole> {
    let top = text.paragraphs.iter().filter(|p| build_filled(p)).map(|p| p.level).min().unwrap_or(0);
    let mut tops = 0;
    let mut owner: Option<usize> = None;
    text.paragraphs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if p.level <= top && build_filled(p) && tops < MAX_BUILD_PARAGRAPHS {
                tops += 1;
                owner = Some(i);
                BuildRole::Top
            } else {
                owner.map_or(BuildRole::Unowned, BuildRole::Member)
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Animation {
    pub shape: ShapeId,
    pub class: AnimClass,
    /// Effect id from [`ANIMATIONS`] (e.g. `fade`, `fly`, `zoom`, `spin`, `pulse`, `lines`…).
    pub effect: String,
    /// Effect option (direction etc.).
    pub option: String,
    pub start: AnimStart,
    pub duration_ms: u32,
    pub delay_ms: u32,
    /// Repeat count (0/1 = once); `u32::MAX` = until next click.
    pub repeat: u32,
    pub rewind: bool,
    pub text_build: TextBuild,
    /// Paragraph index when the effect animates one paragraph of a text body.
    pub paragraph: Option<u32>,
    /// Motion path in slide-relative units (SVG-like `M 0 0 L 0.25 0 E`).
    pub path: Option<String>,
    /// Triggered by a click on another shape.
    pub trigger: Option<ShapeId>,
    /// Emphasis amount: spin degrees, grow/shrink scale (1.5 = 150%), transparency…
    pub amount: Option<f64>,
    /// Emphasis colour (fill/line/font colour change).
    pub color: Option<crate::style::ColorRef>,
    /// PowerPoint preset id/subtype read from or written to files.
    pub preset_id: Option<u32>,
    pub preset_subtype: Option<u32>,
    /// After animation: dim to colour, hide, hide on next click.
    pub after: Option<String>,
    pub sound: Option<crate::MediaId>,
    pub smooth_start: f64,
    pub smooth_end: f64,
    pub bounce_end: f64,
    pub auto_reverse: bool,
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            shape: ShapeId(0),
            class: AnimClass::Entrance,
            effect: "fade".into(),
            option: String::new(),
            start: AnimStart::OnClick,
            duration_ms: 500,
            delay_ms: 0,
            repeat: 1,
            rewind: false,
            text_build: TextBuild::AsOne,
            paragraph: None,
            path: None,
            trigger: None,
            amount: None,
            color: None,
            preset_id: None,
            preset_subtype: None,
            after: None,
            sound: None,
            smooth_start: 0.0,
            smooth_end: 0.0,
            bounce_end: 0.0,
            auto_reverse: false,
        }
    }
}

/// (id, label, class, PowerPoint preset id, default duration ms, options)
pub static ANIMATIONS: &[(&str, &str, AnimClass, u32, u32, &[&str])] = &[
    ("appear", "Appear", AnimClass::Entrance, 1, 0, &[]),
    ("fade", "Fade", AnimClass::Entrance, 10, 500, &[]),
    ("fly", "Fly In", AnimClass::Entrance, 2, 500, &["b", "lb", "l", "lt", "t", "rt", "r", "rb"]),
    ("float", "Float In", AnimClass::Entrance, 42, 1000, &["u", "d"]),
    ("split", "Split", AnimClass::Entrance, 16, 500, &["horzIn", "horzOut", "vertIn", "vertOut"]),
    ("wipe", "Wipe", AnimClass::Entrance, 22, 500, &["b", "l", "t", "r"]),
    ("shape", "Shape", AnimClass::Entrance, 6, 2000, &["circleIn", "circleOut", "boxIn", "boxOut", "diamondIn", "diamondOut", "plusIn", "plusOut"]),
    ("wheel", "Wheel", AnimClass::Entrance, 21, 2000, &["1", "2", "3", "4", "8"]),
    ("randomBars", "Random Bars", AnimClass::Entrance, 14, 500, &["horz", "vert"]),
    ("grow", "Grow & Turn", AnimClass::Entrance, 31, 1000, &[]),
    ("zoom", "Zoom", AnimClass::Entrance, 53, 500, &["in", "out"]),
    ("swivel", "Swivel", AnimClass::Entrance, 45, 2000, &["horz", "vert"]),
    ("bounce", "Bounce", AnimClass::Entrance, 26, 2000, &[]),
    ("blinds", "Blinds", AnimClass::Entrance, 3, 500, &["horz", "vert"]),
    ("box", "Box", AnimClass::Entrance, 4, 2000, &["in", "out"]),
    ("checkerboard", "Checkerboard", AnimClass::Entrance, 5, 500, &["across", "down"]),
    ("circle", "Circle", AnimClass::Entrance, 6, 2000, &["in", "out"]),
    ("diamond", "Diamond", AnimClass::Entrance, 8, 2000, &["in", "out"]),
    ("dissolve", "Dissolve In", AnimClass::Entrance, 9, 500, &[]),
    ("peek", "Peek In", AnimClass::Entrance, 12, 500, &["b", "l", "t", "r"]),
    ("strips", "Strips", AnimClass::Entrance, 18, 500, &["lu", "ru", "ld", "rd"]),
    ("expand", "Expand", AnimClass::Entrance, 55, 1000, &[]),
    ("rise", "Rise Up", AnimClass::Entrance, 37, 1000, &[]),
    ("spinner", "Spinner", AnimClass::Entrance, 49, 500, &[]),
    ("pulse", "Pulse", AnimClass::Emphasis, 26, 500, &[]),
    ("colorPulse", "Color Pulse", AnimClass::Emphasis, 27, 250, &[]),
    ("teeter", "Teeter", AnimClass::Emphasis, 32, 1000, &[]),
    ("spin", "Spin", AnimClass::Emphasis, 8, 2000, &["clockwise", "counterClockwise"]),
    ("growShrink", "Grow/Shrink", AnimClass::Emphasis, 6, 2000, &["both", "horz", "vert"]),
    ("desaturate", "Desaturate", AnimClass::Emphasis, 25, 500, &[]),
    ("darken", "Darken", AnimClass::Emphasis, 24, 500, &[]),
    ("lighten", "Lighten", AnimClass::Emphasis, 30, 500, &[]),
    ("transparency", "Transparency", AnimClass::Emphasis, 9, 500, &[]),
    ("objectColor", "Object Color", AnimClass::Emphasis, 19, 500, &[]),
    ("complementaryColor", "Complementary Color", AnimClass::Emphasis, 21, 500, &[]),
    ("lineColor", "Line Color", AnimClass::Emphasis, 7, 2000, &[]),
    ("fillColor", "Fill Color", AnimClass::Emphasis, 1, 2000, &[]),
    ("fontColor", "Font Color", AnimClass::Emphasis, 3, 2000, &[]),
    ("underline", "Underline", AnimClass::Emphasis, 18, 500, &[]),
    ("boldFlash", "Bold Flash", AnimClass::Emphasis, 10, 2000, &[]),
    ("wave", "Wave", AnimClass::Emphasis, 34, 500, &[]),
    ("disappear", "Disappear", AnimClass::Exit, 1, 0, &[]),
    ("fadeOut", "Fade", AnimClass::Exit, 10, 500, &[]),
    ("flyOut", "Fly Out", AnimClass::Exit, 2, 500, &["b", "lb", "l", "lt", "t", "rt", "r", "rb"]),
    ("floatOut", "Float Out", AnimClass::Exit, 42, 1000, &["u", "d"]),
    ("splitOut", "Split", AnimClass::Exit, 16, 500, &["horzIn", "horzOut", "vertIn", "vertOut"]),
    ("wipeOut", "Wipe", AnimClass::Exit, 22, 500, &["b", "l", "t", "r"]),
    ("shapeOut", "Shape", AnimClass::Exit, 6, 2000, &["circleIn", "circleOut"]),
    ("wheelOut", "Wheel", AnimClass::Exit, 21, 2000, &["1", "2", "3", "4", "8"]),
    ("randomBarsOut", "Random Bars", AnimClass::Exit, 14, 500, &["horz", "vert"]),
    ("shrinkTurn", "Shrink & Turn", AnimClass::Exit, 31, 1000, &[]),
    ("zoomOut", "Zoom", AnimClass::Exit, 53, 500, &["in", "out"]),
    ("swivelOut", "Swivel", AnimClass::Exit, 45, 2000, &["horz", "vert"]),
    ("bounceOut", "Bounce", AnimClass::Exit, 26, 2000, &[]),
    ("dissolveOut", "Dissolve Out", AnimClass::Exit, 9, 500, &[]),
    ("blindsOut", "Blinds", AnimClass::Exit, 3, 500, &["horz", "vert"]),
    ("boxOut", "Box", AnimClass::Exit, 4, 2000, &["in", "out"]),
    ("checkerboardOut", "Checkerboard", AnimClass::Exit, 5, 500, &["across", "down"]),
    ("diamondOut", "Diamond", AnimClass::Exit, 8, 2000, &["in", "out"]),
    ("peekOut", "Peek Out", AnimClass::Exit, 12, 500, &["b", "l", "t", "r"]),
    ("stripsOut", "Strips", AnimClass::Exit, 18, 500, &["lu", "ru", "ld", "rd"]),
    ("sinkDown", "Sink Down", AnimClass::Exit, 37, 1000, &[]),
    ("spinnerOut", "Spinner", AnimClass::Exit, 49, 500, &[]),
    ("contract", "Contract", AnimClass::Exit, 55, 1000, &[]),
    ("lines", "Lines", AnimClass::Path, 64, 2000, &["down", "up", "right", "left"]),
    ("arcs", "Arcs", AnimClass::Path, 37, 2000, &["down", "up", "right", "left"]),
    ("turns", "Turns", AnimClass::Path, 43, 2000, &["down", "up", "right", "left"]),
    (
        "shapes",
        "Shapes",
        AnimClass::Path,
        1,
        2000,
        &["circle", "diamond", "hexagon", "triangle", "square", "trapezoid", "octagon", "parallelogram", "pentagon"],
    ),
    ("loops", "Loops", AnimClass::Path, 61, 2000, &[]),
    ("diagonalDownRight", "Diagonal Down Right", AnimClass::Path, 49, 2000, &[]),
    ("diagonalUpRight", "Diagonal Up Right", AnimClass::Path, 56, 2000, &[]),
    ("zigzag", "Zigzag", AnimClass::Path, 38, 2000, &[]),
    ("sineWave", "Sine Wave", AnimClass::Path, 40, 2000, &[]),
    ("pathWave", "Wave", AnimClass::Path, 47, 2000, &[]),
    ("decayingWave", "Decaying Wave", AnimClass::Path, 60, 2000, &[]),
    ("heartbeat", "Heartbeat", AnimClass::Path, 45, 2000, &[]),
    ("sCurve1", "S Curve 1", AnimClass::Path, 59, 2000, &[]),
    ("sCurve2", "S Curve 2", AnimClass::Path, 39, 2000, &[]),
    ("stairsDown", "Stairs Down", AnimClass::Path, 62, 2000, &[]),
    ("bounceRight", "Bounce Right", AnimClass::Path, 54, 2000, &[]),
    ("bounceLeft", "Bounce Left", AnimClass::Path, 41, 2000, &[]),
    ("spiralRight", "Spiral Right", AnimClass::Path, 46, 2000, &[]),
    ("spiralLeft", "Spiral Left", AnimClass::Path, 55, 2000, &[]),
    ("spring", "Spring", AnimClass::Path, 53, 2000, &[]),
    ("funnel", "Funnel", AnimClass::Path, 52, 2000, &[]),
    ("swoosh", "Swoosh", AnimClass::Path, 30, 2000, &[]),
    ("star4", "4 Point Star", AnimClass::Path, 16, 2000, &[]),
    ("star5", "5 Point Star", AnimClass::Path, 5, 2000, &[]),
    ("star6", "6 Point Star", AnimClass::Path, 11, 2000, &[]),
    ("star8", "8 Point Star", AnimClass::Path, 17, 2000, &[]),
    ("heart", "Heart", AnimClass::Path, 9, 2000, &[]),
    ("crescentMoon", "Crescent Moon", AnimClass::Path, 6, 2000, &[]),
    ("teardrop", "Teardrop", AnimClass::Path, 18, 2000, &[]),
    ("football", "Football", AnimClass::Path, 12, 2000, &[]),
    ("plusPath", "Plus", AnimClass::Path, 32, 2000, &[]),
    ("rightTriangle", "Right Triangle", AnimClass::Path, 2, 2000, &[]),
    ("curvedSquare", "Curved Square", AnimClass::Path, 20, 2000, &[]),
    ("figure8Horizontal", "Horizontal Figure 8", AnimClass::Path, 26, 2000, &[]),
    ("figure8Vertical", "Vertical Figure 8", AnimClass::Path, 22, 2000, &[]),
    ("loopDeLoop", "Loop de Loop", AnimClass::Path, 24, 2000, &[]),
    ("bean", "Bean", AnimClass::Path, 31, 2000, &[]),
    ("peanut", "Peanut", AnimClass::Path, 27, 2000, &[]),
    ("neutron", "Neutron", AnimClass::Path, 29, 2000, &[]),
    ("customPath", "Custom Path", AnimClass::Path, 0, 2000, &[]),
    ("play", "Play", AnimClass::Media, 1, 0, &[]),
    ("pause", "Pause", AnimClass::Media, 2, 0, &[]),
    ("stop", "Stop", AnimClass::Media, 3, 0, &[]),
];

pub fn animation_info(effect: &str, class: AnimClass) -> Option<&'static (&'static str, &'static str, AnimClass, u32, u32, &'static [&'static str])> {
    ANIMATIONS.iter().find(|a| a.0 == effect && a.2 == class).or_else(|| ANIMATIONS.iter().find(|a| a.0 == effect))
}

/// The path of a motion-path preset beyond Lines, Arcs, Turns, Shapes and Loops, in slide
/// fractions from the shape's position (our path syntax: `M`, `L`, `Z`, ending in `E`).
/// `None` for an effect that isn't one of these presets.
pub fn preset_path(effect: &str) -> Option<String> {
    use std::f64::consts::{PI, TAU};
    use std::fmt::Write;
    // Sample `f` over 0..=1 into a polyline starting at (0, 0).
    fn poly(n: usize, closed: bool, f: impl Fn(f64) -> (f64, f64)) -> String {
        let (x0, y0) = f(0.0);
        let mut s = String::from("M 0 0");
        for k in 1..=n {
            let (x, y) = f(k as f64 / n as f64);
            let _ = write!(s, " L {:.4} {:.4}", x - x0, y - y0);
        }
        if closed {
            s.push_str(" Z");
        }
        s.push_str(" E");
        s
    }
    fn pts(p: &[(f64, f64)], closed: bool) -> String {
        let mut s = String::from("M 0 0");
        for (x, y) in p {
            let _ = write!(s, " L {x:.4} {y:.4}");
        }
        if closed {
            s.push_str(" Z");
        }
        s.push_str(" E");
        s
    }
    // A closed curve in polar form r(θ), starting at its top.
    fn polar(scale: f64, r: impl Fn(f64) -> f64) -> String {
        poly(96, true, |t| {
            let a = -PI / 2.0 + TAU * t;
            let rr = r(a + PI / 2.0) * scale;
            (rr * a.cos(), rr * a.sin())
        })
    }
    fn star(n: usize, outer: f64, inner: f64) -> String {
        let mut p = Vec::new();
        let start = (0.0, -outer);
        for k in 1..=2 * n {
            let a = -PI / 2.0 + PI * k as f64 / n as f64;
            let r = if k % 2 == 1 { inner } else { outer };
            p.push((r * a.cos() - start.0, r * a.sin() - start.1));
        }
        pts(&p, true)
    }
    let spiral = |dir: f64| {
        poly(120, false, move |t| {
            let r = 0.12 * (1.0 - t);
            let a = dir * 3.0 * TAU * t;
            (dir * (0.12 - r * a.cos()), -r * a.sin())
        })
    };
    let bounce = |dir: f64| poly(90, false, move |t| (dir * 0.3 * t, 0.15 * t - 0.1 * (1.0 - t) * (3.0 * PI * t).sin().abs()));
    Some(match effect {
        "diagonalDownRight" => pts(&[(0.2, 0.2)], false),
        "diagonalUpRight" => pts(&[(0.2, -0.2)], false),
        "zigzag" => pts(&[(0.05, 0.04), (0.1, 0.0), (0.15, 0.04), (0.2, 0.0), (0.25, 0.04), (0.3, 0.0)], false),
        "sineWave" => poly(64, false, |t| (0.3 * t, 0.04 * (2.0 * TAU * t).sin())),
        "pathWave" => poly(48, false, |t| (0.3 * t, 0.06 * (TAU * t).sin())),
        "decayingWave" => poly(96, false, |t| (0.3 * t, 0.07 * (-3.0 * t).exp() * (3.0 * TAU * t).sin())),
        "heartbeat" => pts(&[(0.08, 0.0), (0.1, -0.08), (0.13, 0.06), (0.15, 0.0), (0.25, 0.0)], false),
        "sCurve1" => poly(48, false, |t| (0.2 * t, 0.1 - 0.1 * (PI * t).cos())),
        "sCurve2" => poly(48, false, |t| (0.1 - 0.1 * (PI * t).cos(), 0.2 * t)),
        "stairsDown" => pts(&[(0.06, 0.0), (0.06, 0.06), (0.12, 0.06), (0.12, 0.12), (0.18, 0.12), (0.18, 0.18)], false),
        "bounceRight" => bounce(1.0),
        "bounceLeft" => bounce(-1.0),
        "spiralRight" => spiral(1.0),
        "spiralLeft" => spiral(-1.0),
        "spring" => poly(160, false, |t| (0.3 * t + 0.03 * (5.0 * TAU * t).cos(), 0.03 * (5.0 * TAU * t).sin())),
        "funnel" => poly(160, false, |t| {
            let r = 0.12 * (1.0 - t);
            (r * (3.0 * TAU * t).cos(), 0.25 * t + 0.03 * (3.0 * TAU * t).sin() * (1.0 - t))
        }),
        "swoosh" => poly(48, false, |t| (0.3 * t, -0.12 * (PI * t).sin() + 0.1 * t * t)),
        "star4" => star(4, 0.1, 0.04),
        "star5" => star(5, 0.1, 0.04),
        "star6" => star(6, 0.1, 0.05),
        "star8" => star(8, 0.1, 0.06),
        "heart" => poly(96, true, |t| {
            let a = TAU * t;
            (0.006 * 16.0 * a.sin().powi(3), -0.006 * (13.0 * a.cos() - 5.0 * (2.0 * a).cos() - 2.0 * (3.0 * a).cos() - (4.0 * a).cos()))
        }),
        "crescentMoon" => {
            // The outer arc of a circle, back along a smaller, offset one.
            let mut p: Vec<(f64, f64)> = (1..=32)
                .map(|k| {
                    let a = -PI / 2.0 - PI * k as f64 / 32.0;
                    (0.1 * a.cos(), 0.1 + 0.1 * a.sin())
                })
                .collect();
            p.extend((1..=32).map(|k| {
                let a = PI / 2.0 - PI * k as f64 / 32.0;
                (0.06 * a.cos() - 0.03, 0.1 + 0.1 * a.sin())
            }));
            pts(&p, true)
        }
        "teardrop" => polar(0.1, |a| 0.6 + 0.4 * (a / 2.0).cos().abs().powf(3.0)),
        "football" => poly(64, true, |t| {
            let a = TAU * t;
            (0.12 * a.sin(), 0.06 * (1.0 - a.cos()) * (1.0 - 0.3 * (2.0 * a).cos().abs()))
        }),
        "plusPath" => pts(
            &[
                (0.04, 0.0),
                (0.04, 0.04),
                (0.08, 0.04),
                (0.08, 0.08),
                (0.04, 0.08),
                (0.04, 0.12),
                (0.0, 0.12),
                (0.0, 0.08),
                (-0.04, 0.08),
                (-0.04, 0.04),
                (0.0, 0.04),
            ],
            true,
        ),
        "rightTriangle" => pts(&[(0.0, 0.15), (0.15, 0.15)], true),
        "curvedSquare" => poly(96, true, |t| {
            let a = -PI / 2.0 + TAU * t;
            let sg = |v: f64| v.signum() * v.abs().sqrt();
            (0.1 * sg(a.cos()), 0.1 + 0.1 * sg(a.sin()))
        }),
        "figure8Horizontal" => poly(96, true, |t| {
            let a = TAU * t;
            (0.15 * a.sin(), 0.06 * (2.0 * a).sin())
        }),
        "figure8Vertical" => poly(96, true, |t| {
            let a = TAU * t;
            (0.06 * (2.0 * a).sin(), 0.15 * (1.0 - a.cos()))
        }),
        "loopDeLoop" => poly(120, false, |t| (0.3 * t - 0.05 * (2.0 * TAU * t).sin(), -0.05 * (1.0 - (2.0 * TAU * t).cos()))),
        "bean" => polar(0.1, |a| 0.75 + 0.25 * a.cos() + 0.15 * (2.0 * a).cos()),
        "peanut" => polar(0.1, |a| 0.7 + 0.3 * (2.0 * a).cos().abs()),
        "neutron" => polar(0.1, |a| 0.35 + 0.65 * (3.0 * a).cos().abs()),
        _ => return None,
    })
}

#[cfg(test)]
mod option_label_tests {
    use super::*;

    #[test]
    fn transition_option_labels() {
        assert_eq!(transition_option_label("push", "l"), "From Right");
        assert_eq!(transition_option_label("push", "u"), "From Bottom");
        assert_eq!(transition_option_label("wipe", "r"), "From Left");
        assert_eq!(transition_option_label("cover", "ru"), "From Bottom-Left");
        assert_eq!(transition_option_label("ripple", "ru"), "From Bottom-Left");
        assert_eq!(transition_option_label("glitter", "hexU"), "Hexagons From Top");
        assert_eq!(transition_option_label("glitter", "l"), "Diamonds From Left");
        assert_eq!(transition_option_label("wind", "l"), "Left");
        assert_eq!(transition_option_label("pageCurlDouble", "singleR"), "Single Right");
        assert_eq!(transition_option_label("shred", "particlesOut"), "Particles Out");
        assert_eq!(transition_option_label("zoom", "rotate"), "Zoom and Rotate");
        assert_eq!(transition_option_label("split", "vertIn"), "Vertical In");
        for (id, _, _, _, opts) in TRANSITIONS {
            for o in opts.iter() {
                assert!(!transition_option_label(id, o).is_empty(), "{id}/{o}");
            }
        }
    }
}

#[cfg(test)]
mod build_role_tests {
    use super::*;
    use crate::text::TextBody;

    #[test]
    fn blank_and_deeper_paragraphs_follow_the_top_one_before_them() {
        let mut t = TextBody::from_text("Before\nLead\n \nSub\nNext");
        for (p, lvl) in t.paragraphs.iter_mut().zip([1, 0, 0, 1, 0]) {
            p.level = lvl;
        }
        use BuildRole::*;
        assert_eq!(paragraph_build_roles(&t), vec![Unowned, Top, Member(1), Member(1), Top]);
    }
}
