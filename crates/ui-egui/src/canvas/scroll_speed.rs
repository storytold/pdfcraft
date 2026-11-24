//! How far a mouse-wheel notch scrolls (#444, #186), and ↓ / ↑ with it (#985).
//!
//! macOS already accelerates line deltas, and browsers there scroll about that far per line,
//! but on Windows and Linux winit reports one line per notch, where browsers scroll about
//! 100 pixels, so the desktop default is larger than a text line. The canvas reads the two
//! distances through its navigation options.

/// The user's choice, relative to the platform's normal speed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollSpeed {
    Slow,
    #[default]
    Normal,
    Fast,
}

/// Points one wheel line scrolls at Normal speed.
const NORMAL_LINE_POINTS: f32 = if cfg!(target_arch = "wasm32") {
    // The browser's line deltas, which egui already scales for the web.
    8.0
} else if cfg!(target_os = "macos") {
    40.0
} else {
    100.0
};

/// Points one arrow-key step scrolls at Normal speed.
const KEY_LINE_POINTS: f32 = 40.0;

impl ScrollSpeed {
    pub const ALL: [Self; 3] = [Self::Slow, Self::Normal, Self::Fast];

    pub fn label(self) -> &'static str {
        match self {
            Self::Slow => "Slow",
            Self::Normal => "Normal",
            Self::Fast => "Fast",
        }
    }

    /// This speed relative to Normal.
    pub fn factor(self) -> f32 {
        match self {
            Self::Slow => 0.5,
            Self::Normal => 1.0,
            Self::Fast => 2.0,
        }
    }

    /// Points one wheel line scrolls.
    pub fn wheel_line(self) -> f32 {
        NORMAL_LINE_POINTS * self.factor()
    }

    /// Points one arrow-key step scrolls.
    pub fn key_line(self) -> f32 {
        KEY_LINE_POINTS * self.factor()
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.label().eq_ignore_ascii_case(value))
    }
}
