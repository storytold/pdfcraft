//! What the tab strip shows at its left end: nothing (the default), the app's logo and name, or
//! a logo and name of the user's own.
//!
//! With the strip standing in for the title bar (`window_controls`) a window otherwise carries no
//! name at all, as a browser's does not; this puts one back for those who want it, and lets a
//! company put its own there (Preferences ▸ Show in the tab strip). A logo of the user's own is a
//! PNG or SVG file on disk, read once and kept as a texture; a file that cannot be read falls
//! back to the app's own logo and says so in the log. The name is plain text.

use std::path::{Path, PathBuf};

use crate::theme::{self, Tokens};
use egui::Vec2;

/// What to show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TitleBrand {
    /// Nothing, as before.
    #[default]
    Off,
    Logo,
    Name,
    LogoAndName,
}

impl TitleBrand {
    pub const ALL: [Self; 4] = [Self::Off, Self::Logo, Self::Name, Self::LogoAndName];

    /// The settings file's and `set title-brand`'s word for it.
    pub fn token(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Logo => "logo",
            Self::Name => "name",
            Self::LogoAndName => "logo-and-name",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.token() == s)
    }

    /// The preference's label, in English (translated where shown).
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Nothing",
            Self::Logo => "Logo",
            Self::Name => "Name",
            Self::LogoAndName => "Logo and name",
        }
    }

    pub fn shows_logo(self) -> bool {
        matches!(self, Self::Logo | Self::LogoAndName)
    }

    pub fn shows_name(self) -> bool {
        matches!(self, Self::Name | Self::LogoAndName)
    }
}

/// The app's own name, shown when the user has not given one.
pub const APP_NAME: &str = "PdfCraft";

/// The longest name kept: a settings file is untrusted input, and the strip is not wide.
pub const MAX_NAME_CHARS: usize = 60;

/// The app's own logo: its icon at 128 px (assets/app-icon, contributor-original), enough for the
/// strip at any interface scale.
const APP_LOGO_PNG: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/128x128/apps/ai.storyteller.pdfcraft.png");

/// A logo image too large to be meant for a 20-point strip is refused rather than decoded: a
/// settings file is untrusted input, and a huge PNG would take a texture to match.
const MAX_LOGO_PIXELS: u32 = 2048;

/// A logo file larger than this is refused before it is read: room for a PNG of
/// [`MAX_LOGO_PIXELS`] on a side or any sensible SVG, and a bound on what a wrong path (a video, a
/// disk image) or a saved path pointing at one costs the interface.
const MAX_LOGO_BYTES: u64 = 8 * 1024 * 1024;

/// A decoded logo, ready to draw.
enum Loaded {
    /// A texture egui owns while the handle lives. An SVG is rasterised when it is loaded, so it
    /// is checked then (a broken one falls back to the app's logo), it is read afresh each time it
    /// is chosen, and it is sized like a PNG.
    Raster(egui::TextureHandle),
}

/// The preference and what it has loaded.
#[derive(Default)]
pub struct Brand {
    pub show: TitleBrand,
    /// Empty: the app's own name.
    pub name: String,
    /// None: the app's own logo.
    pub logo: Option<PathBuf>,
    /// What `logo` (or the app's logo, for None) decoded to, and which path it came from, so a
    /// changed preference loads again and a failed one is not retried every frame.
    loaded: Option<(Option<PathBuf>, Option<Loaded>)>,
}

impl Brand {
    /// The name to show: the user's, or the app's own.
    pub fn display_name(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() { APP_NAME } else { name }
    }

    /// Forget the loaded logo, so the next frame reads the file again (Preferences ▸ Reset, or a
    /// new file chosen).
    pub fn reload_logo(&mut self) {
        self.loaded = None;
    }

    /// Takes a logo file of the user's own (`None`: back to the app's own), and forgets the one
    /// loaded so the next frame reads the new file. A path that is not valid UTF-8 is refused: the
    /// settings file is JSON, which cannot carry it, so it could never come back after a restart.
    /// So is a file over [`MAX_LOGO_BYTES`], which the load would refuse every frame anyway.
    pub fn set_logo(&mut self, path: Option<PathBuf>) -> Result<(), String> {
        if let Some(p) = &path {
            if p.to_str().is_none() {
                return Err(
                    tl!("This file can't be used as the logo: its path isn't valid UTF-8, so it couldn't be saved with the settings.").to_owned()
                );
            }
            if file_too_large(p) {
                return Err(crate::i18n::fmt(
                    tl!("This file can't be used as the logo: it's larger than {max} MB."),
                    &[("max", &(MAX_LOGO_BYTES / (1024 * 1024)).to_string())],
                ));
            }
        }
        self.logo = path;
        self.reload_logo();
        Ok(())
    }

    /// The preference as saved in the settings file: `{"show", "name", "logo"}`. The path goes in
    /// as text (`to_str`), never through serde's `Path` support, which `json!` would unwrap into a
    /// panic on a non-UTF-8 path; such a path is saved as null (`set_logo` refuses it anyway).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "show": self.show.token(),
            "name": self.name,
            // Null: the app's own logo.
            "logo": self.logo.as_deref().and_then(Path::to_str),
        })
    }

    /// Reads what [`Self::to_json`] wrote. Settings are untrusted: an unknown choice keeps the
    /// default, and a name is cut at [`MAX_NAME_CHARS`].
    pub fn restore(&mut self, v: &serde_json::Value) {
        if let Some(show) = v["show"].as_str().and_then(TitleBrand::from_token) {
            self.show = show;
        }
        if let Some(name) = v["name"].as_str() {
            self.name = name.chars().take(MAX_NAME_CHARS).collect();
        }
        // An explicit null is what `to_json` writes for the app's own logo; a missing key (which
        // indexing would also read as null) or anything else unreadable keeps the current one.
        match v.get("logo") {
            Some(serde_json::Value::Null) => {
                if self.logo.take().is_some() {
                    self.reload_logo();
                }
            }
            Some(serde_json::Value::String(path)) => {
                self.logo = Some(PathBuf::from(path));
                self.reload_logo();
            }
            _ => {}
        }
    }

    /// Draws the logo and/or name at the strip's current position, `height` points tall and at
    /// most `max_width` wide (a long name is cut with "…"), and moves the cursor past them. Nothing
    /// is drawn (and no space taken) when the preference is off. The widgets sense nothing, so the
    /// strip's drag passes through them.
    pub fn show(&mut self, ui: &mut egui::Ui, height: f32, max_width: f32) {
        if self.show == TitleBrand::Off {
            return;
        }
        let t = Tokens::get(ui.ctx());
        let start = ui.cursor().min.x;
        if self.show.shows_logo() {
            self.ensure_loaded(ui.ctx());
            let drawn = match self.loaded.as_ref().and_then(|(_, l)| l.as_ref()) {
                Some(Loaded::Raster(tex)) => {
                    let mut size = fit(tex.size_vec2(), height);
                    // Narrower still when the budget is: the proportions stay.
                    if size.x > max_width && size.x > 0.0 {
                        size *= max_width.max(0.0) / size.x;
                    }
                    ui.add(egui::Image::new(tex).fit_to_exact_size(size).sense(egui::Sense::hover()));
                    true
                }
                None => false,
            };
            if drawn {
                ui.add_space(6.0);
            }
        }
        // What the logo left of the budget; too little for a few letters shows no name at all.
        let left = max_width - (ui.cursor().min.x - start);
        if self.show.shows_name() && left >= 24.0 {
            let text = egui::RichText::new(self.display_name()).font(theme::medium(13.0)).color(t.text);
            ui.scope(|ui| {
                ui.set_max_width(left);
                ui.add(egui::Label::new(text).selectable(false).truncate());
            });
            ui.add_space(6.0);
        }
    }

    /// Loads the logo once per path. A file that cannot be read or decoded leaves the app's own
    /// logo in place and logs why, so a broken path in the settings never leaves a hole.
    fn ensure_loaded(&mut self, ctx: &egui::Context) {
        if matches!(&self.loaded, Some((path, _)) if *path == self.logo) {
            return;
        }
        let loaded = match &self.logo {
            Some(path) => match load_file(path) {
                Ok(bytes) => decode(ctx, &path.display().to_string(), &bytes)
                    .map_err(|e| log::warn!("the tab strip logo {} is not used: {e}", path.display()))
                    .ok(),
                Err(e) => {
                    log::warn!("the tab strip logo {} is not used: {e}", path.display());
                    None
                }
            },
            None => None,
        };
        let loaded = loaded.or_else(|| decode(ctx, "app-logo", APP_LOGO_PNG).ok());
        self.loaded = Some((self.logo.clone(), loaded));
    }
}

/// The size an image of `image` points fills `height` points tall, keeping its proportions; a
/// very wide logo is capped at six times the height so it cannot push the tabs away.
fn fit(image: Vec2, height: f32) -> Vec2 {
    if image.y <= 0.0 || image.x <= 0.0 {
        return Vec2::splat(height);
    }
    let width = (image.x / image.y * height).min(height * 6.0);
    Vec2::new(width, height)
}

/// Reads a logo file of up to [`MAX_LOGO_BYTES`]. The read itself is bounded, not only its
/// metadata, which a special file may lack and a file may outgrow after it was chosen.
#[cfg(not(target_arch = "wasm32"))]
fn load_file(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    // One byte past the cap tells a file that is too large from one that just fits.
    file.take(MAX_LOGO_BYTES.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_LOGO_BYTES {
        return Err(format!("larger than {} MB", MAX_LOGO_BYTES / (1024 * 1024)));
    }
    Ok(bytes)
}

/// Whether a logo file is known to be over [`MAX_LOGO_BYTES`] from its metadata alone; a file
/// that cannot be measured is not, and is judged when it is read.
#[cfg(not(target_arch = "wasm32"))]
fn file_too_large(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOGO_BYTES)
}

/// The web build has no files on disk; a saved path means the app's own logo there.
#[cfg(target_arch = "wasm32")]
fn load_file(_path: &Path) -> Result<Vec<u8>, String> {
    Err("no files on the web".into())
}

#[cfg(target_arch = "wasm32")]
fn file_too_large(_path: &Path) -> bool {
    false
}

/// PNG through the shared codecs, SVG rasterised through resvg; anything else is refused by its
/// first bytes, not its name.
fn decode(ctx: &egui::Context, key: &str, bytes: &[u8]) -> Result<Loaded, String> {
    let image = if bytes.starts_with(b"\x89PNG") {
        decode_png(bytes)?
    } else if looks_like_svg(bytes) {
        decode_svg(bytes)?
    } else {
        return Err("not a PNG or SVG file".into());
    };
    Ok(Loaded::Raster(ctx.load_texture(format!("brand-logo:{key}"), image, egui::TextureOptions::LINEAR)))
}

/// The height an SVG logo is rasterised at: several times the strip's, so it stays sharp at high
/// display scales, and small enough to cost little.
const SVG_RASTER_HEIGHT: u32 = 128;

/// Rasterises an SVG as large as fits [`SVG_RASTER_HEIGHT`] tall and six times that wide (the
/// width cap the strip applies anyway), keeping its proportions. A file resvg cannot parse, or
/// one with no size, is an error, so the app's own logo shows instead.
fn decode_svg(bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let hint = egui::SizeHint::Size { width: SVG_RASTER_HEIGHT * 6, height: SVG_RASTER_HEIGHT, maintain_aspect_ratio: true };
    let image = egui_extras::image::load_svg_bytes_with_size(bytes, hint, &resvg::usvg::Options::default())?;
    if image.width() == 0 || image.height() == 0 {
        return Err("the image has no pixels".into());
    }
    Ok(image)
}

/// An SVG starts with `<svg` or an XML prologue, after optional whitespace and a BOM.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = bytes.get(..bytes.len().min(512)).unwrap_or(bytes);
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))
}

/// Decodes a PNG of any colour type or bit depth to RGBA8 through the shared codecs, refusing one
/// larger than [`MAX_LOGO_PIXELS`] on a side before its pixels are allocated.
fn decode_png(bytes: &[u8]) -> Result<egui::ColorImage, String> {
    use craft_image_codecs::{DecodeOptions, Format, Limits};
    let side = MAX_LOGO_PIXELS;
    let limits = Limits {
        max_width: side,
        max_height: side,
        max_pixels: u64::from(side) * u64::from(side),
        // RGBA at 16 bits a sample, the most a PNG can decode to.
        max_alloc: u64::from(side) * u64::from(side) * 8,
    };
    let opts = DecodeOptions { limits, ..DecodeOptions::default() };
    let image =
        craft_image_codecs::decode_as_with(Format::Png, bytes, &opts).map_err(|e| format!("{e}; a logo may be up to {side} pixels on a side"))?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Err("the image has no pixels".into());
    }
    let rgba = image.to_rgba8();
    let pixels = (width as usize).checked_mul(height as usize).ok_or("the image's size overflows")?;
    if rgba.len() != pixels.checked_mul(4).ok_or("the image's size overflows")? {
        return Err("the decoded image has the wrong size".into());
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba))
}

#[cfg(test)]
mod tests {
    use super::{Brand, SVG_RASTER_HEIGHT, TitleBrand, decode_png, decode_svg, fit, looks_like_svg};

    fn png(width: u32, height: u32, layout: craft_image_codecs::ChannelLayout) -> Vec<u8> {
        let channels = match layout {
            craft_image_codecs::ChannelLayout::Rgba => 4,
            craft_image_codecs::ChannelLayout::Rgb => 3,
            craft_image_codecs::ChannelLayout::GrayA => 2,
            _ => 1,
        };
        let image = craft_image_codecs::Image::from_u8(width, height, layout, vec![0x80; (width * height * channels) as usize]).expect("image");
        craft_image_codecs::encode(&image, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).expect("png")
    }

    #[test]
    fn tokens_round_trip_and_unknown_ones_are_refused() {
        for b in TitleBrand::ALL {
            assert_eq!(TitleBrand::from_token(b.token()), Some(b));
        }
        assert_eq!(TitleBrand::from_token("logo and name"), None);
        assert_eq!(TitleBrand::default(), TitleBrand::Off);
        assert!(TitleBrand::LogoAndName.shows_logo() && TitleBrand::LogoAndName.shows_name());
        assert!(!TitleBrand::Logo.shows_name() && !TitleBrand::Name.shows_logo());
    }

    #[test]
    fn the_preference_round_trips_through_json_and_shrugs_off_bad_input() {
        let mut brand = Brand { show: TitleBrand::LogoAndName, name: "Acme".into(), logo: Some("C:/acme/logo.png".into()), ..Brand::default() };
        let mut back = Brand::default();
        back.restore(&brand.to_json());
        assert_eq!((back.show, back.name.as_str(), back.logo.clone()), (TitleBrand::LogoAndName, "Acme", Some("C:/acme/logo.png".into())));
        // Nothing saved, or junk: the defaults stay.
        brand = Brand::default();
        brand.restore(&serde_json::json!(null));
        brand.restore(&serde_json::json!({"show": "banner", "name": 7, "logo": false}));
        assert_eq!((brand.show, brand.name.as_str(), brand.logo.clone()), (TitleBrand::Off, "", None));
        // A name longer than the strip could ever show is cut, at a character boundary.
        brand.restore(&serde_json::json!({"name": "é".repeat(500)}));
        assert_eq!(brand.name.chars().count(), super::MAX_NAME_CHARS);
    }

    /// `json!` unwraps serde's `Path` serialisation, which fails on a path that is not UTF-8:
    /// saving the settings would have panicked. The path now goes in as text or null.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_logo_path_never_panics_the_settings_save_and_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let bad = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/logo-\xff.png"));
        let mut brand = Brand::default();
        assert!(brand.set_logo(Some(bad.clone())).is_err(), "refused at the door");
        assert_eq!(brand.logo, None);
        // Even if one got in (an old settings file, a future caller), saving must not panic.
        brand.logo = Some(bad);
        let json = brand.to_json();
        assert!(json["logo"].is_null());
        // A normal path round-trips, and None clears.
        assert!(brand.set_logo(Some("/tmp/logo.png".into())).is_ok());
        assert_eq!(brand.to_json()["logo"], "/tmp/logo.png");
        assert!(brand.set_logo(None).is_ok());
        assert_eq!(brand.logo, None);
    }

    #[test]
    fn an_empty_or_blank_name_shows_the_apps_own() {
        let mut brand = Brand::default();
        assert_eq!(brand.display_name(), "PdfCraft");
        brand.name = "   ".into();
        assert_eq!(brand.display_name(), "PdfCraft");
        brand.name = " Multiplan ".into();
        assert_eq!(brand.display_name(), "Multiplan");
    }

    #[test]
    fn every_png_colour_type_decodes_to_rgba() {
        for color in [
            craft_image_codecs::ChannelLayout::Rgba,
            craft_image_codecs::ChannelLayout::Rgb,
            craft_image_codecs::ChannelLayout::GrayA,
            craft_image_codecs::ChannelLayout::Gray,
        ] {
            let image = decode_png(&png(3, 2, color)).expect("decodes");
            assert_eq!(image.size, [3, 2]);
            assert_eq!(image.pixels.len(), 6);
        }
    }

    #[test]
    fn the_apps_own_logo_decodes() {
        let image = decode_png(super::APP_LOGO_PNG).expect("the embedded icon decodes");
        assert_eq!(image.size, [128, 128]);
    }

    /// A logo file over [`super::MAX_LOGO_BYTES`] is refused when chosen and, should a saved path
    /// point at one, when read: the read stops one byte past the cap rather than taking the file.
    #[test]
    fn an_oversized_logo_file_is_refused_before_it_is_read() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-brand-logo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let big = dir.join("big.png");
        // Sparse: a length, not 8 MB written.
        std::fs::File::create(&big).expect("create").set_len(super::MAX_LOGO_BYTES + 1).expect("set_len");
        let mut brand = Brand::default();
        let refused = brand.set_logo(Some(big.clone())).expect_err("refused at the door");
        assert!(refused.contains("8 MB"), "{refused}");
        assert_eq!(brand.logo, None);
        assert_eq!(super::load_file(&big).expect_err("refused when read"), "larger than 8 MB");
        // A file that just fits is read whole.
        let fits = dir.join("fits.png");
        std::fs::write(&fits, png(2, 2, craft_image_codecs::ChannelLayout::Rgba)).expect("write");
        assert!(brand.set_logo(Some(fits.clone())).is_ok());
        assert_eq!(super::load_file(&fits).expect("read"), png(2, 2, craft_image_codecs::ChannelLayout::Rgba));
        assert!(super::load_file(&dir.join("missing.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_and_broken_pngs_are_refused_not_panics() {
        assert!(decode_png(&png(2049, 1, craft_image_codecs::ChannelLayout::Rgba)).is_err());
        assert!(decode_png(b"\x89PNG\r\n\x1a\n not really").is_err());
        assert!(decode_png(&[]).is_err());
        assert!(decode_png(&png(2, 2, craft_image_codecs::ChannelLayout::Rgba)[..20]).is_err());
    }

    #[test]
    fn svgs_are_rasterised_within_the_caps_and_broken_ones_refused() {
        let square =
            decode_svg(b"<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><rect width='10' height='10'/></svg>").expect("square");
        assert_eq!(square.size, [SVG_RASTER_HEIGHT as usize; 2]);
        // 30:1 is held to six times the height, proportions kept.
        let wide = decode_svg(b"<svg xmlns='http://www.w3.org/2000/svg' width='300' height='10'/>").expect("wide");
        assert!(wide.width() <= SVG_RASTER_HEIGHT as usize * 6 && wide.height() < SVG_RASTER_HEIGHT as usize);
        // Passes the content sniff, but is no SVG: refused, so the app's logo shows instead.
        assert!(looks_like_svg(b"<svg"));
        assert!(decode_svg(b"<svg").is_err());
        assert!(decode_svg(b"<svg xmlns='http://www.w3.org/2000/svg' width='0' height='0'/>").is_err());
    }

    #[test]
    fn a_saved_null_logo_means_the_apps_own() {
        let mut brand = Brand { logo: Some("/tmp/logo.png".into()), ..Brand::default() };
        brand.restore(&serde_json::json!({ "logo": null }));
        assert_eq!(brand.logo, None);
        // Missing or not text: the current one stays.
        let mut brand = Brand { logo: Some("/tmp/logo.png".into()), ..Brand::default() };
        brand.restore(&serde_json::json!({ "logo": 7 }));
        brand.restore(&serde_json::json!({}));
        assert_eq!(brand.logo.as_deref(), Some(std::path::Path::new("/tmp/logo.png")));
    }

    #[test]
    fn svg_is_recognised_by_content() {
        assert!(looks_like_svg(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(looks_like_svg(b"\xef\xbb\xbf  <?xml version='1.0'?>\n<svg/>"));
        assert!(!looks_like_svg(b"<html><body>no</body></html>"));
        assert!(!looks_like_svg(b""));
    }

    #[test]
    fn a_logo_keeps_its_proportions_within_a_cap() {
        assert_eq!(fit(egui::vec2(128.0, 128.0), 20.0), egui::vec2(20.0, 20.0));
        assert_eq!(fit(egui::vec2(400.0, 100.0), 20.0), egui::vec2(80.0, 20.0));
        // Six times the height at most, and no division by zero.
        assert_eq!(fit(egui::vec2(10_000.0, 10.0), 20.0), egui::vec2(120.0, 20.0));
        assert_eq!(fit(egui::vec2(0.0, 0.0), 20.0), egui::vec2(20.0, 20.0));
    }
}
