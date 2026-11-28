//! The reader-chosen lighting schemes of a 3D view (ISO 32000-1 §13.6.4.5, Tables 309 and 310;
//! ISO 32000-2 Tables 320 and 321, same values), and how [`crate::render`] shades a surface under
//! each.
//!
//! Every scheme light is infinite (directional). Faces are lit on both sides, as the headlight lights
//! them, so a light's direction counts only up to sign. The standard does not say which frame the
//! directions are in; they are taken in the camera's (x right, y up, z forward), so the lights stay
//! put as a model turns.
//!
//! Adapted from the author's own MIT-licensed pdfcer (`crates/pdfcer-3d/src/lighting.rs` at commit
//! `4c155aea`), which was written from the same tables.

use crate::vec3::{Vec3, dot, normalize};

/// A lighting scheme: a `/3DLightingScheme` dictionary's `/Subtype`, as [`crate::Lighting::Scheme`].
///
/// `/Artwork` (the model's own lights) and `/None` are not schemes here: [`LightingScheme::lighting_for`]
/// maps them to [`crate::Lighting::Headlight`] and [`crate::Lighting::Unlit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LightingScheme {
    /// `/White`: three white lights.
    White,
    /// `/Day`: three daylight lights.
    Day,
    /// `/Night`: three night-coloured lights.
    Night,
    /// `/Hard`: three grey lights and an ambient term.
    Hard,
    /// `/Primary`: red, green and blue lights.
    Primary,
    /// `/Blue`: three blue lights.
    Blue,
    /// `/Red`: three red lights.
    Red,
    /// `/Cube`: six grey lights along the axes.
    Cube,
    /// `/CAD`: a light at the camera and three more.
    Cad,
    /// `/Headlamp`: one light at the camera and an ambient term.
    Headlamp,
}

/// One infinite light: a DeviceRGB colour and a camera-frame direction. A zero direction shines
/// along the line of sight, as the headlight does.
struct Light {
    colour: Vec3,
    direction: Vec3,
}

const fn light(colour: Vec3, direction: Vec3) -> Light {
    Light { colour, direction }
}

const D1: Vec3 = [-2.0, -1.5, -0.5];
const D2: Vec3 = [2.0, 1.1, -2.5];
const UP: Vec3 = [0.0, 0.0, 2.0];
const AT_CAMERA: Vec3 = [0.0; 3];
const GREY: Vec3 = [0.4; 3];

const WHITE: [Light; 3] = [light([0.38, 0.38, 0.45], D1), light([0.6, 0.6, 0.67], D2), light([0.5, 0.5, 0.57], [-0.5, 0.0, 2.0])];
const DAY: [Light; 3] = [light([0.5, 0.5, 0.5], D1), light([0.8, 0.8, 0.9], D2), light([0.9, 0.9, 0.9], [0.02, 0.01, 2.0])];
const NIGHT: [Light; 3] = [light([1.0, 0.75, 0.39], D1), light([0.31, 0.47, 0.55], D2), light([0.5, 0.5, 1.0], UP)];
const HARD: [Light; 3] =
    [light([0.5, 0.5, 0.5], [-1.5, -1.5, -1.5]), light([0.8, 0.8, 0.9], [1.5, 1.5, -1.5]), light([0.9, 0.9, 0.9], [-0.5, 0.0, 2.0])];
const PRIMARY: [Light; 3] = [light([1.0, 0.2, 0.5], D1), light([0.2, 1.0, 0.5], D2), light([0.0, 0.0, 1.0], UP)];
const BLUE: [Light; 3] = [light([0.4, 0.4, 0.7], D1), light([0.75, 0.75, 0.95], D2), light([0.7, 0.7, 0.95], UP)];
const RED: [Light; 3] = [light([0.8, 0.3, 0.4], D1), light([0.95, 0.5, 0.7], D2), light([0.95, 0.4, 0.5], UP)];
const CUBE: [Light; 6] = [
    light(GREY, [1.0, 0.01, 0.01]),
    light(GREY, [0.01, 1.0, 0.01]),
    light(GREY, [0.01, 0.01, 1.0]),
    light(GREY, [-1.0, 0.01, 0.01]),
    light(GREY, [0.01, -1.0, 0.01]),
    light(GREY, [0.01, 0.01, -1.0]),
];
const CAD: [Light; 4] =
    [light([0.72, 0.72, 0.81], AT_CAMERA), light([0.2, 0.2, 0.2], D1), light([0.32, 0.32, 0.32], D2), light([0.36, 0.36, 0.36], [0.04, 0.01, 2.0])];
const HEADLAMP: [Light; 1] = [light([0.8, 0.8, 0.9], AT_CAMERA)];

impl LightingScheme {
    /// Every scheme, in Table 310's order.
    pub const ALL: [LightingScheme; 10] = [
        LightingScheme::White,
        LightingScheme::Day,
        LightingScheme::Night,
        LightingScheme::Hard,
        LightingScheme::Primary,
        LightingScheme::Blue,
        LightingScheme::Red,
        LightingScheme::Cube,
        LightingScheme::Cad,
        LightingScheme::Headlamp,
    ];

    /// The scheme's `/Subtype` name, e.g. `CAD`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            LightingScheme::White => "White",
            LightingScheme::Day => "Day",
            LightingScheme::Night => "Night",
            LightingScheme::Hard => "Hard",
            LightingScheme::Primary => "Primary",
            LightingScheme::Blue => "Blue",
            LightingScheme::Red => "Red",
            LightingScheme::Cube => "Cube",
            LightingScheme::Cad => "CAD",
            LightingScheme::Headlamp => "Headlamp",
        }
    }

    /// How a view whose `/LS` dictionary has `/Subtype` `name` is lit: `Artwork` (the model's own
    /// lights, which this crate does not read yet) is the headlight, `None` is unlit, and a scheme
    /// name is that scheme. `None` for a name Table 310 does not list, whose dictionary a reader
    /// ignores (so the artwork's lighting applies).
    #[must_use]
    pub fn lighting_for(name: &str) -> Option<crate::Lighting> {
        match name {
            "Artwork" => Some(crate::Lighting::Headlight),
            "None" => Some(crate::Lighting::Unlit),
            _ => LightingScheme::ALL.into_iter().find(|s| s.name() == name).map(crate::Lighting::Scheme),
        }
    }

    /// The ambient term added to every surface, each channel 0–1.
    fn ambient(self) -> Vec3 {
        match self {
            LightingScheme::Hard => [0.5; 3],
            LightingScheme::Headlamp => [0.1; 3],
            _ => [0.0; 3],
        }
    }

    fn lights(self) -> &'static [Light] {
        match self {
            LightingScheme::White => &WHITE,
            LightingScheme::Day => &DAY,
            LightingScheme::Night => &NIGHT,
            LightingScheme::Hard => &HARD,
            LightingScheme::Primary => &PRIMARY,
            LightingScheme::Blue => &BLUE,
            LightingScheme::Red => &RED,
            LightingScheme::Cube => &CUBE,
            LightingScheme::Cad => &CAD,
            LightingScheme::Headlamp => &HEADLAMP,
        }
    }
}

/// A scheme resolved against one camera: world-space unit light directions (`None` along the line
/// of sight) and their colours.
pub(crate) struct SchemeShader {
    ambient: Vec3,
    lights: Vec<(Option<Vec3>, Vec3)>,
}

impl SchemeShader {
    /// Turns `scheme`'s camera-frame directions into world space through `basis` (right, up,
    /// forward).
    pub(crate) fn new(scheme: LightingScheme, basis: &[Vec3; 3]) -> SchemeShader {
        let [right, up, forward] = basis;
        let to_world = |[x, y, z]: Vec3| [0, 1, 2].map(|i| x * at(right, i) + y * at(up, i) + z * at(forward, i));
        let lights = scheme.lights().iter().map(|l| (normalize(to_world(l.direction)), l.colour)).collect();
        SchemeShader { ambient: scheme.ambient(), lights }
    }

    /// The per-channel factor for a surface with unit normal `n`; `towards` points to the eye.
    /// The pixel saturates at full colour, so the factor is not clamped here.
    pub(crate) fn shade(&self, n: Vec3, towards: Option<Vec3>) -> Vec3 {
        let mut out = self.ambient;
        for (direction, colour) in &self.lights {
            let k = direction.or(towards).map_or(1.0, |d| dot(n, d).abs().min(1.0));
            for (o, c) in out.iter_mut().zip(colour) {
                *o += c * k;
            }
        }
        out
    }
}

fn at(v: &Vec3, i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests;
