//! The "reading colours" view setting (Acrobat's Replace Document Colors): solid text and
//! line-art paints are remapped for readability while rendering. Display only — the document
//! itself never changes. The setting flows into [`hayro::RenderSettings`] through
//! `RenderConfig`.

/// The reading-colours setting for one document. `params: None` means off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReadingColors {
    /// The colours and flags, or `None` when the setting is off.
    pub params: Option<ReadingParams>,
    /// How a dark setting draws scanned pages (see [`ScanColors`]). Settings saved before it
    /// existed read the default.
    #[serde(default)]
    pub scans: ScanColors,
}

/// Colours and flags of the reading-colours mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReadingParams {
    /// The sRGB colour that dark paints are mapped towards.
    pub text: [u8; 3],
    /// The sRGB colour that light paints are mapped towards.
    pub background: [u8; 3],
    /// When `true`, only near-black paints are mapped; lighter ones keep their colour.
    pub only_black: bool,
    /// When `true`, line-art paints are mapped in addition to text paints.
    pub line_art: bool,
}

impl ReadingColors {
    /// Black text on a white page.
    pub const BLACK_ON_WHITE: Self = Self {
        params: Some(ReadingParams { text: [0x00, 0x00, 0x00], background: [0xff, 0xff, 0xff], only_black: false, line_art: true }),
        scans: ScanColors::DEFAULT,
    };
    /// White text on a black page.
    pub const WHITE_ON_BLACK: Self = Self {
        params: Some(ReadingParams { text: [0xff, 0xff, 0xff], background: [0x00, 0x00, 0x00], only_black: false, line_art: true }),
        scans: ScanColors::DEFAULT,
    };
    /// Yellow text on a black page.
    pub const YELLOW_ON_BLACK: Self = Self {
        params: Some(ReadingParams { text: [0xff, 0xff, 0x00], background: [0x00, 0x00, 0x00], only_black: false, line_art: true }),
        scans: ScanColors::DEFAULT,
    };
    /// Green text on a black page.
    pub const GREEN_ON_BLACK: Self = Self {
        params: Some(ReadingParams { text: [0x00, 0xff, 0x00], background: [0x00, 0x00, 0x00], only_black: false, line_art: true }),
        scans: ScanColors::DEFAULT,
    };

    /// Parses tool/settings input. `mode` is one of `off | black-on-white | white-on-black |
    /// yellow-on-black | green-on-black | custom`; `custom` needs `text` and `background` as
    /// `#rrggbb`; the flags override the preset defaults when given. `off` ignores colours.
    /// Errors are actionable strings.
    pub fn parse(mode: &str, text: Option<&str>, background: Option<&str>, only_black: Option<bool>, line_art: Option<bool>) -> Result<Self, String> {
        // Every mode accepts the flags: they override the preset defaults, and are the custom
        // mode's own values.
        let with_flags = |mut colors: Self| {
            if let Some(p) = colors.params.as_mut() {
                if let Some(v) = only_black {
                    p.only_black = v;
                }
                if let Some(v) = line_art {
                    p.line_art = v;
                }
            }
            colors
        };
        match mode {
            "off" => Ok(Self::default()),
            "black-on-white" => Ok(with_flags(Self::BLACK_ON_WHITE)),
            "white-on-black" => Ok(with_flags(Self::WHITE_ON_BLACK)),
            "yellow-on-black" => Ok(with_flags(Self::YELLOW_ON_BLACK)),
            "green-on-black" => Ok(with_flags(Self::GREEN_ON_BLACK)),
            "custom" => match (text, background) {
                (None, None) => Err("custom needs text and background as #rrggbb".into()),
                (None, Some(_)) => Err("custom needs text as #rrggbb".into()),
                (Some(_), None) => Err("custom needs background as #rrggbb".into()),
                (Some(text), Some(background)) => Ok(with_flags(Self {
                    params: Some(ReadingParams {
                        text: parse_hex("text", text)?,
                        background: parse_hex("background", background)?,
                        only_black: false,
                        line_art: true,
                    }),
                    scans: ScanColors::DEFAULT,
                })),
            },
            other => {
                Err(format!("unknown mode {other:?}; expected one of: off, black-on-white, white-on-black, yellow-on-black, green-on-black, custom"))
            }
        }
    }

    /// The mode name for echoes and display: `"off"`, a preset name, or `"custom"`.
    pub fn mode_name(&self) -> &'static str {
        match self.params {
            None => "off",
            Some(params) => {
                if Some(params) == Self::BLACK_ON_WHITE.params {
                    "black-on-white"
                } else if Some(params) == Self::WHITE_ON_BLACK.params {
                    "white-on-black"
                } else if Some(params) == Self::YELLOW_ON_BLACK.params {
                    "yellow-on-black"
                } else if Some(params) == Self::GREEN_ON_BLACK.params {
                    "green-on-black"
                } else {
                    "custom"
                }
            }
        }
    }

    /// The text colour as lowercase `#rrggbb`, or `None` when off.
    pub fn text_hex(&self) -> Option<String> {
        self.params.map(|p| hex(p.text))
    }

    /// The background colour as lowercase `#rrggbb`, or `None` when off.
    pub fn background_hex(&self) -> Option<String> {
        self.params.map(|p| hex(p.background))
    }

    /// Whether the setting draws light text on a dark page (white, yellow or green on black, or
    /// a custom pair like them): its background is darker than its text.
    pub fn is_dark(&self) -> bool {
        self.params.is_some_and(|p| luminance(p.background) < luminance(p.text))
    }

    /// The filter scanned pages are drawn through, or `None` when they keep their colours: the
    /// setting is off, it is not dark (a scan is already dark text on a light page), or scans are
    /// set to stay as they are.
    pub fn scan_filter(&self) -> Option<ScanFilter> {
        let params = self.params.filter(|_| self.is_dark())?;
        let chroma = match self.scans.mode {
            ScanMode::Off => return None,
            ScanMode::Invert => return Some(ScanFilter::Invert),
            ScanMode::Smart => 1.0,
            ScanMode::Palette => 0.0,
        };
        // Contrast widens the luminance treated as plain paper or plain ink: at the default (50)
        // anything lighter than 85% is paper, so a grey or yellowed scan gets an even background.
        let contrast = f32::from(self.scans.contrast.min(100)) / 100.0;
        let unit = |c: [u8; 3]| c.map(|v| f32::from(v) / 255.0);
        Some(ScanFilter::Map {
            ink: 0.2 * contrast,
            paper: 1.0 - 0.3 * contrast,
            chroma,
            text: unit(params.text),
            background: unit(params.background),
        })
    }
}

/// How a dark reading-colours setting draws a scanned page. Recolouring paints cannot reach a
/// scan: the page is one picture, and pictures keep their colours, so the page stayed white.
/// A page is a scan when a picture covers at least half of it and it has no visible text of its
/// own (an invisible OCR layer does not count), the rule Edit uses to recognise text on scans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScanColors {
    pub mode: ScanMode,
    /// 0–100: how much of the scan's luminance range becomes plain paper or plain ink. Larger
    /// values clean up grey or yellowed paper and faint ink; 0 maps the range linearly. Values
    /// above 100 read as 100.
    pub contrast: u8,
}

impl ScanColors {
    /// Smart invert at contrast 50.
    pub const DEFAULT: Self = Self { mode: ScanMode::Smart, contrast: 50 };

    /// Parses tool input: `mode` is one of `smart | palette | invert | off` and `contrast` is
    /// 0–100; either may be left out to keep `self`'s. Errors are actionable strings.
    pub fn parse(self, mode: Option<&str>, contrast: Option<i64>) -> Result<Self, String> {
        let mode = match mode {
            None => self.mode,
            Some(name) => ScanMode::from_name(name).ok_or_else(|| format!("unknown scans mode {name:?}; expected one of: {}", ScanMode::NAMES))?,
        };
        let contrast = match contrast {
            None => self.contrast,
            Some(c) => u8::try_from(c).ok().filter(|c| *c <= 100).ok_or_else(|| format!("scan_contrast {c} is out of range; expected 0 to 100"))?,
        };
        Ok(Self { mode, contrast })
    }
}

impl Default for ScanColors {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How a dark setting draws scanned pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScanMode {
    /// Paper takes the page background and ink the text colour, while coloured marks keep their
    /// hue (red ink stays red, lightened).
    #[default]
    Smart,
    /// Like `Smart`, but every mark takes the setting's two colours.
    Palette,
    /// Every pixel is inverted: a photographic negative.
    Invert,
    /// Scans keep their colours.
    Off,
}

impl ScanMode {
    /// Every mode, in menu order.
    pub const ALL: [Self; 4] = [Self::Smart, Self::Palette, Self::Invert, Self::Off];
    const NAMES: &str = "smart, palette, invert, off";

    /// The mode's name in tools and settings.
    pub fn name(self) -> &'static str {
        match self {
            Self::Smart => "smart",
            Self::Palette => "palette",
            Self::Invert => "invert",
            Self::Off => "off",
        }
    }

    /// The mode named `name`, or `None`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// The pixel filter for scanned pages, from [`ReadingColors::scan_filter`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScanFilter {
    /// Every channel inverted.
    Invert,
    /// Luminance at or below `ink` becomes `text`, at or above `paper` becomes `background`, and
    /// what lies between mixes the two; `chroma` (0–1) of each pixel's own colour is kept on top.
    /// Channels are 0–1, gamma-encoded sRGB.
    Map { ink: f32, paper: f32, chroma: f32, text: [f32; 3], background: [f32; 3] },
}

impl ScanFilter {
    /// Filters one opaque sRGB pixel. Never panics.
    pub fn map(&self, rgb: [u8; 3]) -> [u8; 3] {
        let Self::Map { ink, paper, chroma, text, background } = *self else { return rgb.map(|v| 255 - v) };
        let c = rgb.map(|v| f32::from(v) / 255.0);
        let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let range = paper - ink;
        let t = if range > f32::EPSILON {
            ((y - ink) / range).clamp(0.0, 1.0)
        } else if y >= paper {
            1.0
        } else {
            0.0
        };
        // Grey paper and a scanner's faint colour cast (yellowed paper is about 0.15) carry no
        // colour worth keeping: only saturated marks keep theirs, fully from 0.35 up.
        let saturation = c.iter().fold(0.0f32, |m, v| m.max(*v)) - c.iter().fold(1.0f32, |m, v| m.min(*v));
        let keep = chroma * ((saturation - 0.15) / 0.2).clamp(0.0, 1.0);
        let mut out = [0u8; 3];
        for (i, o) in out.iter_mut().enumerate() {
            let base = background[i] * t + text[i] * (1.0 - t);
            // Clamped to 0–1 first, so the cast is total.
            *o = ((base + (c[i] - y) * keep).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        out
    }

    /// Filters a raster of premultiplied RGBA8 bytes in place. Transparent pixels stay as they
    /// are; others are filtered at their own colour and keep their alpha. Runs of one colour
    /// (a scan's paper) are mapped once.
    pub fn apply(&self, rgba: &mut [u8]) {
        let mut last: Option<([u8; 4], [u8; 4])> = None;
        for px in rgba.chunks_exact_mut(4) {
            let from = [px[0], px[1], px[2], px[3]];
            if let Some((seen, mapped)) = last
                && seen == from
            {
                px.copy_from_slice(&mapped);
                continue;
            }
            let a = from[3];
            let mapped = if a == 0 {
                from
            } else {
                // Premultiplied channels never exceed alpha in a valid raster; `min` keeps a bad
                // one total.
                let straight = [0, 1, 2].map(|i| ((u16::from(from[i].min(a)) * 255 + u16::from(a) / 2) / u16::from(a)) as u8);
                let [r, g, b] = self.map(straight);
                let pre = |v: u8| ((u16::from(v) * u16::from(a) + 127) / 255) as u8;
                [pre(r), pre(g), pre(b), a]
            };
            px.copy_from_slice(&mapped);
            last = Some((from, mapped));
        }
    }
}

/// Luminance of gamma-encoded sRGB bytes, 0–1: the measure `hayro::ColorReplacement` maps by.
fn luminance([r, g, b]: [u8; 3]) -> f32 {
    (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0
}

impl From<ReadingParams> for hayro::ColorReplacement {
    fn from(p: ReadingParams) -> Self {
        hayro::ColorReplacement { text: p.text, background: p.background, only_black: p.only_black, line_art: p.line_art }
    }
}

/// Parses a colour given as `#rrggbb` — exactly this shape, either case of digits. The error
/// names the argument and the expected shape.
fn parse_hex(name: &str, value: &str) -> Result<[u8; 3], String> {
    let invalid = || format!("invalid {name} colour {value:?}; expected #rrggbb (six hex digits, e.g. #00ff00)");
    let Some(digits) = value.strip_prefix('#') else { return Err(invalid()) };
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let mut rgb = [0u8; 3];
    for (i, channel) in rgb.iter_mut().enumerate() {
        // Length and charset are checked above, so the parse cannot actually fail; the mapping
        // only keeps it total without panicking.
        *channel = u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).map_err(|_| invalid())?;
    }
    Ok(rgb)
}

/// Formats sRGB bytes as lowercase `#rrggbb`.
fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_every_mode() {
        let off = ReadingColors::parse("off", None, None, None, None).unwrap();
        assert_eq!(off, ReadingColors::default());
        assert_eq!(off.mode_name(), "off");
        assert_eq!(off.text_hex(), None);
        assert_eq!(off.background_hex(), None);
        // `off` ignores colour and flag arguments.
        assert_eq!(ReadingColors::parse("off", Some("zzz"), Some("#000000"), Some(true), Some(false)).unwrap(), off);

        for (mode, text, background) in [
            ("black-on-white", "#000000", "#ffffff"),
            ("white-on-black", "#ffffff", "#000000"),
            ("yellow-on-black", "#ffff00", "#000000"),
            ("green-on-black", "#00ff00", "#000000"),
        ] {
            let colors = ReadingColors::parse(mode, None, None, None, None).unwrap();
            assert_eq!(colors.mode_name(), mode, "{mode}");
            assert_eq!(colors.text_hex().as_deref(), Some(text), "{mode}");
            assert_eq!(colors.background_hex().as_deref(), Some(background), "{mode}");
            let params = colors.params.unwrap();
            assert!(!params.only_black && params.line_art, "{mode}");
        }

        // The flags override the preset defaults; a changed preset is no longer one.
        let overridden = ReadingColors::parse("white-on-black", None, None, Some(true), Some(false)).unwrap();
        let params = overridden.params.unwrap();
        assert!(params.only_black && !params.line_art);
        assert_eq!(overridden.mode_name(), "custom");

        // `custom` takes either case of hex and echoes lowercase.
        let custom = ReadingColors::parse("custom", Some("#AaBbCc"), Some("#123456"), Some(true), None).unwrap();
        assert_eq!(custom.mode_name(), "custom");
        assert_eq!(custom.text_hex().as_deref(), Some("#aabbcc"));
        assert_eq!(custom.background_hex().as_deref(), Some("#123456"));
        let params = custom.params.unwrap();
        assert!(params.only_black && params.line_art);
    }

    #[test]
    fn dark_settings_have_a_scan_filter_and_light_ones_none() {
        for dark in [ReadingColors::WHITE_ON_BLACK, ReadingColors::YELLOW_ON_BLACK, ReadingColors::GREEN_ON_BLACK] {
            assert!(dark.is_dark() && dark.scan_filter().is_some(), "{}", dark.mode_name());
        }
        assert!(ReadingColors::default().scan_filter().is_none(), "off");
        assert!(ReadingColors::BLACK_ON_WHITE.scan_filter().is_none(), "a scan already reads dark on light");
        let off = ReadingColors { scans: ScanColors { mode: ScanMode::Off, contrast: 50 }, ..ReadingColors::WHITE_ON_BLACK };
        assert!(off.scan_filter().is_none());
        // The scan choice is no part of the preset.
        assert_eq!(off.mode_name(), "white-on-black");
    }

    #[test]
    fn smart_invert_cleans_the_paper_and_keeps_coloured_ink() {
        let smart = ReadingColors::WHITE_ON_BLACK.scan_filter().unwrap();
        assert_eq!(smart.map([255, 255, 255]), [0, 0, 0], "white paper");
        assert_eq!(smart.map([242, 235, 205]), [0, 0, 0], "yellowed paper is as clean");
        assert_eq!(smart.map([20, 20, 20]), [255, 255, 255], "black ink");
        assert_eq!(smart.map([128, 128, 128]), [118, 118, 118], "mid grey stays mid grey");
        let [r, g, b] = smart.map([200, 30, 30]);
        assert!(r > 230 && g < r - 80 && g == b, "red ink stays red, lightened: {r} {g} {b}");
        // Palette drops the colour; the preset's own colours take its place.
        let palette = ReadingColors { scans: ScanColors { mode: ScanMode::Palette, contrast: 50 }, ..ReadingColors::YELLOW_ON_BLACK };
        let palette = palette.scan_filter().unwrap();
        assert_eq!(palette.map([20, 20, 20]), [255, 255, 0], "black ink becomes the yellow text colour");
        let [r, g, b] = palette.map([200, 30, 30]);
        assert!(r == g && b == 0, "no red left: {r} {g} {b}");
        // Contrast 0 maps the range linearly: paper is the darkest grey, not yet black.
        let linear = ReadingColors { scans: ScanColors { mode: ScanMode::Smart, contrast: 0 }, ..ReadingColors::WHITE_ON_BLACK };
        assert_eq!(linear.scan_filter().unwrap().map([242, 242, 242]), [13, 13, 13]);
        // Out-of-range contrast from a settings file reads as 100.
        let high = ReadingColors { scans: ScanColors { mode: ScanMode::Smart, contrast: 255 }, ..ReadingColors::WHITE_ON_BLACK };
        let full = ReadingColors { scans: ScanColors { mode: ScanMode::Smart, contrast: 100 }, ..ReadingColors::WHITE_ON_BLACK };
        assert_eq!(high.scan_filter(), full.scan_filter());
    }

    #[test]
    fn the_filter_keeps_alpha_and_leaves_transparent_pixels() {
        let smart = ReadingColors::WHITE_ON_BLACK.scan_filter().unwrap();
        // Premultiplied: white at half alpha is 128s; it becomes black at half alpha.
        let mut rgba = [0, 0, 0, 0, 128, 128, 128, 128, 255, 255, 255, 255, 255, 255, 255, 255, 9, 200, 3, 4];
        smart.apply(&mut rgba);
        assert_eq!(rgba[..16], [0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 255, 0, 0, 0, 255]);
        // A channel above its alpha (no valid raster has one) is read as the alpha.
        assert_eq!(rgba[19], 4);
        assert!(rgba[16..19].iter().all(|v| *v <= 4));
        let mut every = (0..=255u8).flat_map(|v| [v, 255 - v, v / 2, v]).collect::<Vec<_>>();
        ScanFilter::Invert.apply(&mut every);
    }

    #[test]
    fn scan_colours_parse_and_read_old_settings() {
        let d = ScanColors::DEFAULT;
        assert_eq!(d.parse(None, None), Ok(d));
        assert_eq!(d.parse(Some("invert"), Some(80)), Ok(ScanColors { mode: ScanMode::Invert, contrast: 80 }));
        assert_eq!(d.parse(Some("nope"), None), Err("unknown scans mode \"nope\"; expected one of: smart, palette, invert, off".to_string()));
        for c in [-1, 101, i64::MAX] {
            assert_eq!(d.parse(None, Some(c)), Err(format!("scan_contrast {c} is out of range; expected 0 to 100")));
        }
        // Saved before scans existed: the default.
        let old: ReadingColors =
            serde_json::from_str(r#"{"params":{"text":[255,255,255],"background":[0,0,0],"only_black":false,"line_art":true}}"#).unwrap();
        assert_eq!(old, ReadingColors::WHITE_ON_BLACK);
        let saved = serde_json::to_string(&ReadingColors { scans: ScanColors { mode: ScanMode::Palette, contrast: 7 }, ..old }).unwrap();
        assert!(saved.contains(r#""scans":{"mode":"palette","contrast":7}"#), "{saved}");
    }

    #[test]
    fn parse_rejects_unknown_mode() {
        assert_eq!(
            ReadingColors::parse("nope", None, None, None, None),
            Err("unknown mode \"nope\"; expected one of: off, black-on-white, white-on-black, yellow-on-black, green-on-black, custom".to_string())
        );
    }

    #[test]
    fn parse_rejects_malformed_hex() {
        for bad in ["zzz", "000000", "#000", "#0000000", "#00g000", ""] {
            assert_eq!(
                ReadingColors::parse("custom", Some(bad), Some("#000000"), None, None),
                Err(format!("invalid text colour {bad:?}; expected #rrggbb (six hex digits, e.g. #00ff00)")),
                "{bad}"
            );
        }
        assert_eq!(
            ReadingColors::parse("custom", Some("#000000"), Some("ffffff"), None, None),
            Err("invalid background colour \"ffffff\"; expected #rrggbb (six hex digits, e.g. #00ff00)".to_string())
        );
    }

    #[test]
    fn parse_rejects_custom_missing_colours() {
        assert_eq!(ReadingColors::parse("custom", None, None, None, None), Err("custom needs text and background as #rrggbb".to_string()));
        assert_eq!(ReadingColors::parse("custom", None, Some("#000000"), None, None), Err("custom needs text as #rrggbb".to_string()));
        assert_eq!(ReadingColors::parse("custom", Some("#000000"), None, None, None), Err("custom needs background as #rrggbb".to_string()));
    }
}
