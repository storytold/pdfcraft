//! Themes: colour scheme, font scheme and format scheme (the fill/line/effect "style matrix").

use deckcraft_color::{ColorScheme, ColorTransform, Rgba, SchemeSlot};
use serde::{Deserialize, Serialize};

use crate::style::{ColorRef, Effects, Fill, Glow, Gradient, GradientShape, GradientStop, Line, Shadow};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FontSet {
    pub latin: String,
    pub ea: String,
    pub cs: String,
    /// Per-script typefaces (`a:font script="Arab" typeface="…"`), in document order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<ScriptFont>,
}

/// A theme font for one script (ISO 15924 tag, e.g. `Arab`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScriptFont {
    pub script: String,
    pub typeface: String,
}

impl FontSet {
    /// The typeface the theme gives `script` (`"Arab"`), if any.
    pub fn script(&self, script: &str) -> Option<&str> {
        self.scripts.iter().find(|f| f.script.eq_ignore_ascii_case(script) && !f.typeface.is_empty()).map(|f| f.typeface.as_str())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FontScheme {
    pub name: String,
    /// Headings.
    pub major: FontSet,
    /// Body.
    pub minor: FontSet,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FormatScheme {
    pub name: String,
    /// Subtle, moderate, intense (style index 1–3).
    pub fills: Vec<Fill>,
    pub lines: Vec<Line>,
    pub effects: Vec<Effects>,
    /// Background fills (index 1001–1003).
    pub bg_fills: Vec<Fill>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Theme {
    pub name: String,
    pub colors: ColorScheme,
    pub fonts: FontScheme,
    pub format: FormatScheme,
    /// Raw theme XML extras kept for round-trip (object defaults, extra colour lists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_extra: Option<String>,
}

impl Default for Theme {
    fn default() -> Self {
        builtin_themes().into_iter().next().unwrap_or_else(|| Theme {
            name: "DeckCraft".into(),
            colors: ColorScheme::new("DeckCraft", [Rgba::BLACK; 12]),
            fonts: FontScheme::default(),
            format: FormatScheme::default(),
            raw_extra: None,
        })
    }
}

impl Theme {
    /// Resolve a theme font reference (`+mj-lt`, `+mn-ea`…) to a family name.
    pub fn font(&self, name: &str) -> String {
        match name {
            "+mj-lt" => self.fonts.major.latin.clone(),
            "+mn-lt" => self.fonts.minor.latin.clone(),
            "+mj-ea" => self.fonts.major.ea.clone(),
            "+mn-ea" => self.fonts.minor.ea.clone(),
            "+mj-cs" => self.fonts.major.cs.clone(),
            "+mn-cs" => self.fonts.minor.cs.clone(),
            other => other.to_string(),
        }
    }
}

fn hex(s: &str) -> Rgba {
    Rgba::from_hex(s).unwrap_or(Rgba::BLACK)
}

fn scheme(name: &str, c: [&str; 12]) -> ColorScheme {
    ColorScheme::new(name, c.map(hex))
}

fn ph() -> ColorRef {
    ColorRef::scheme(SchemeSlot::PhClr)
}

/// Our default format scheme: flat solid fills, a soft gradient, and a gentle shadow for "intense".
pub fn default_format() -> FormatScheme {
    let grad = |a: i32, b: i32| {
        Fill::Gradient(Gradient {
            tile: [0.0; 4],
            stops: vec![
                GradientStop { pos: 0.0, color: ph().with(ColorTransform::LumMod(a)).with(ColorTransform::SatMod(103000)) },
                GradientStop { pos: 1.0, color: ph().with(ColorTransform::LumMod(b)) },
            ],
            shape: GradientShape::Linear { angle: 90.0, scaled: false },
            rotate_with_shape: true,
            flip: None,
        })
    };
    FormatScheme {
        name: "DeckCraft".into(),
        fills: vec![Fill::solid(ph()), grad(110000, 92000), grad(105000, 80000)],
        lines: vec![Line::solid(ph(), 0.75), Line::solid(ph(), 1.0), Line::solid(ph(), 1.5)],
        effects: vec![
            Effects::default(),
            Effects::default(),
            Effects {
                outer_shadow: Some(Shadow {
                    color: ColorRef::rgb(Rgba::BLACK).with(ColorTransform::Alpha(40000)),
                    blur: 6.0,
                    dist: 3.0,
                    dir: 90.0,
                    inner: false,
                    sx: 1.0,
                    sy: 1.0,
                    kx: 0.0,
                    ky: 0.0,
                    align: "t".into(),
                    rotate_with_shape: false,
                }),
                ..Default::default()
            },
        ],
        bg_fills: vec![
            Fill::solid(ph()),
            Fill::solid(ph().with(ColorTransform::Tint(95000)).with(ColorTransform::SatMod(170000))),
            grad(102000, 85000),
        ],
    }
}

/// Design ▸ Variants: four colour variations of `t`, built from its built-in colours when it is
/// a built-in theme (so picking one variant after another doesn't drift), else from its own.
/// 1: as designed; 2 and 3: the accents rotated; 4: dark (light and dark colours swapped).
pub fn theme_variants(t: &Theme) -> Vec<ColorScheme> {
    let base = builtin_themes().into_iter().find(|b| b.name == t.name).map_or_else(|| t.colors.clone(), |b| b.colors);
    let rotate = |by: usize| {
        let mut c = base.clone();
        let accents: Vec<_> = base.colors.get(4..10).map(<[_]>::to_vec).unwrap_or_default();
        for (i, slot) in c.colors.iter_mut().skip(4).take(6).enumerate() {
            if let Some(a) = accents.get((i + by) % accents.len().max(1)) {
                *slot = *a;
            }
        }
        c
    };
    let mut dark = base.clone();
    dark.colors.swap(0, 1);
    dark.colors.swap(2, 3);
    let mut out = vec![base.clone(), rotate(1), rotate(3), dark];
    for (i, c) in out.iter_mut().enumerate().skip(1) {
        c.name = format!("{} {}", base.name, i + 1);
    }
    out
}

/// Our original built-in themes (shown in Design ▸ Themes). Colours and names are our own.
pub fn builtin_themes() -> Vec<Theme> {
    let t = |name: &str, colors: ColorScheme, major: &str, minor: &str| Theme {
        name: name.into(),
        colors,
        fonts: FontScheme {
            name: name.into(),
            major: FontSet { latin: major.into(), ..Default::default() },
            minor: FontSet { latin: minor.into(), ..Default::default() },
        },
        format: default_format(),
        raw_extra: None,
    };
    vec![
        t(
            "DeckCraft",
            scheme(
                "DeckCraft",
                ["000000", "FFFFFF", "1E2B3C", "EDEBE7", "2E6FD8", "F26B1D", "2EA36F", "8A5CD8", "D83F6B", "E3B21C", "2E6FD8", "7A5BA6"],
            ),
            "Inter",
            "Inter",
        ),
        t(
            "Harbor",
            scheme(
                "Harbor",
                ["0B1F2A", "FFFFFF", "12384D", "E4EEF2", "1B7FA6", "36B3A8", "F2C14E", "F78154", "5D576B", "8AA29E", "1B7FA6", "5D576B"],
            ),
            "Merriweather",
            "Open Sans",
        ),
        t(
            "Ember",
            scheme("Ember", ["1A1A1A", "FFFCF7", "3D2B1F", "F6EDE3", "D9480F", "F59F00", "A61E4D", "5C940D", "1971C2", "862E9C", "D9480F", "862E9C"]),
            "Montserrat",
            "Lato",
        ),
        t(
            "Meadow",
            scheme(
                "Meadow",
                ["14281D", "FFFFFF", "2D4A37", "EEF5EE", "4C9A2A", "A4C639", "2F6F4E", "E9B44C", "9B2915", "50A2A7", "2F6F4E", "9B2915"],
            ),
            "Merriweather",
            "Open Sans",
        ),
        t(
            "Nocturne",
            scheme(
                "Nocturne",
                ["FFFFFF", "121826", "F2F4F8", "1E2638", "7C5CFF", "00C2A8", "FF6B9A", "FFC53D", "4DA3FF", "B2F35F", "7C5CFF", "FF6B9A"],
            ),
            "Inter",
            "Inter",
        ),
        t(
            "Paper",
            scheme("Paper", ["262626", "FFFFFF", "404040", "F2F2F2", "595959", "8C8C8C", "C0504D", "4F81BD", "9BBB59", "F79646", "4F81BD", "8064A2"]),
            "Playfair Display",
            "Lato",
        ),
        t(
            "Coral Reef",
            scheme(
                "Coral Reef",
                ["0F2A33", "FFFFFF", "0E4D64", "FDF1EC", "FF6F59", "254441", "43AA8B", "B2B09B", "EF3054", "3F88C5", "3F88C5", "EF3054"],
            ),
            "Poppins",
            "Nunito Sans",
        ),
        t(
            "Slate",
            scheme("Slate", ["111827", "FFFFFF", "374151", "F3F4F6", "6366F1", "0EA5E9", "10B981", "F59E0B", "EF4444", "8B5CF6", "6366F1", "8B5CF6"]),
            "Roboto",
            "Roboto",
        ),
    ]
}

/// Named colour schemes for Design ▸ Variants ▸ Colors.
pub fn builtin_color_schemes() -> Vec<ColorScheme> {
    let mut v: Vec<ColorScheme> = builtin_themes().into_iter().map(|t| t.colors).collect();
    v.push(scheme(
        "Grayscale",
        ["000000", "FFFFFF", "000000", "F8F8F8", "DDDDDD", "B2B2B2", "969696", "808080", "5F5F5F", "4D4D4D", "5F5F5F", "919191"],
    ));
    v.push(scheme("Ocean", ["000000", "FFFFFF", "13293D", "E8F1F2", "006494", "247BA0", "1B98E0", "6CC1E8", "13293D", "0B3954", "1B98E0", "6CC1E8"]));
    v.push(scheme(
        "Sunset",
        ["2B2118", "FFFFFF", "6F1A07", "FFF3E6", "FF9F1C", "FFBF69", "CB997E", "E76F51", "F4A261", "2A9D8F", "E76F51", "2A9D8F"],
    ));
    v.push(scheme(
        "Forest",
        ["1B1B1B", "FFFFFF", "283618", "FEFAE0", "606C38", "DDA15E", "BC6C25", "283618", "8A9A5B", "4F772D", "606C38", "BC6C25"],
    ));
    v.push(scheme("Berry", ["1F0A1D", "FFFFFF", "4A1942", "F7E8F3", "893168", "C6426E", "EB6A8F", "4A1942", "7D7ABC", "56A3A6", "893168", "56A3A6"]));
    v
}

/// Named font pairs for Design ▸ Variants ▸ Fonts.
pub fn builtin_font_schemes() -> Vec<FontScheme> {
    let f = |name: &str, major: &str, minor: &str| FontScheme {
        name: name.into(),
        major: FontSet { latin: major.into(), ..Default::default() },
        minor: FontSet { latin: minor.into(), ..Default::default() },
    };
    vec![
        f("Inter", "Inter", "Inter"),
        f("Classic", "Merriweather", "Open Sans"),
        f("Geometric", "Montserrat", "Lato"),
        f("Editorial", "Playfair Display", "Lato"),
        f("Humanist", "Merriweather", "Open Sans"),
        f("Friendly", "Poppins", "Nunito Sans"),
        f("Neutral", "Roboto", "Roboto"),
        f("Office Compatible", "Carlito", "Carlito"),
        f("Times / Arial", "Liberation Serif", "Liberation Sans"),
    ]
}

fn shadow(blur: f64, dist: f64, alpha: i32) -> Effects {
    Effects {
        outer_shadow: Some(Shadow {
            color: ColorRef::rgb(Rgba::BLACK).with(ColorTransform::Alpha(alpha)),
            blur,
            dist,
            dir: 90.0,
            inner: false,
            sx: 1.0,
            sy: 1.0,
            kx: 0.0,
            ky: 0.0,
            align: "b".into(),
            rotate_with_shape: false,
        }),
        ..Default::default()
    }
}

fn glow(radius: f64) -> Effects {
    Effects { glow: Some(Glow { color: ph().with(ColorTransform::Alpha(60_000)), radius }), ..Default::default() }
}

/// Named effect schemes for Design ▸ Effects. Each one replaces the theme's format scheme
/// (subtle, moderate and intense fills, lines and effects) and leaves the colour and font schemes.
pub fn builtin_format_schemes() -> Vec<FormatScheme> {
    let mut soft = default_format();
    soft.name = "Soft".into();
    soft.lines = vec![Line::solid(ph(), 0.5), Line::solid(ph(), 0.75), Line::solid(ph(), 1.0)];
    soft.effects = vec![shadow(16.0, 1.0, 18_000), shadow(12.0, 2.0, 22_000), shadow(10.0, 3.0, 28_000)];

    let mut crisp = default_format();
    crisp.name = "Crisp".into();
    crisp.fills = vec![Fill::solid(ph()), Fill::solid(ph()), Fill::solid(ph())];
    crisp.lines = vec![Line::solid(ph(), 1.0), Line::solid(ph(), 1.5), Line::solid(ph(), 2.25)];
    crisp.effects = vec![Effects::default(), Effects::default(), shadow(0.0, 2.0, 55_000)];

    let mut lift = default_format();
    lift.name = "Lift".into();
    lift.effects = vec![Effects::default(), shadow(8.0, 6.0, 28_000), shadow(12.0, 10.0, 35_000)];

    let mut glow_scheme = default_format();
    glow_scheme.name = "Glow".into();
    glow_scheme.effects = vec![Effects::default(), glow(6.0), glow(12.0)];

    let mut plain = default_format();
    plain.name = "Plain".into();
    plain.fills = vec![Fill::solid(ph()), Fill::solid(ph()), Fill::solid(ph())];
    plain.lines = vec![Line::solid(ph(), 0.75), Line::solid(ph(), 0.75), Line::solid(ph(), 0.75)];
    plain.effects = vec![Effects::default(), Effects::default(), Effects::default()];

    vec![default_format(), soft, crisp, lift, glow_scheme, plain]
}
