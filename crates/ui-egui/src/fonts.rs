//! The interface fonts: PdfCraft's own faces and the craft-fonts input, handed to the framework's
//! font stack, which orders the scripts, installs the faces and loads installed system fonts on
//! demand.

use std::sync::{Arc, LazyLock};

use craft::ui::fonts::{CjkRows, Emoji, Family, FontRef, I18nScripts, NoCandidates, SEMIBOLD, SystemCandidates, UiFonts, noto_emoji};
use egui::epaint::text::{FallbackAlign, FontData, FontTweak, HintingTarget, SmoothHinting};

use crate::i18n::{LANGUAGES, Lang};

/// The family of the medium-weight interface face.
const MEDIUM: &str = "medium";

/// Maps a language to the scripts it is written in, from craft-i18n's table.
static SCRIPTS: I18nScripts = I18nScripts;

/// The platform whose font rules apply; a parameter so a test can compare them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontPolicyPlatform {
    Windows,
    Other,
}

fn current_platform() -> FontPolicyPlatform {
    if cfg!(target_os = "windows") { FontPolicyPlatform::Windows } else { FontPolicyPlatform::Other }
}

/// The tweak of the app's own face `name`: on Windows JetBrains Mono renders without symmetric
/// smoothing, which keeps its vertical stems sharp at low pixel densities.
fn primary_tweak(platform: FontPolicyPlatform, name: &str) -> FontTweak {
    let mut tweak = FontTweak::default();
    if platform == FontPolicyPlatform::Windows && name == "JetBrainsMono" {
        tweak.hinting_target = HintingTarget::Smooth(SmoothHinting { symmetric_rendering: false, ..SmoothHinting::default() });
    }
    tweak
}

/// `data` with its glyphs placed on the row's baseline (#1676). epaint otherwise centres a
/// fallback face's line box on the primary's, so a face with different metrics sits higher or
/// lower than the Latin beside it. `FallbackAlign::Baseline` positions every face's glyphs from
/// the primary face's ascent.
pub fn on_interface_baseline(mut data: FontData) -> FontData {
    data.tweak.fallback_align = FallbackAlign::Baseline;
    data
}

fn own_face(platform: FontPolicyPlatform, name: &str, bytes: &'static [u8]) -> FontData {
    FontData::from_static(bytes).tweak(primary_tweak(platform, name))
}

/// The interface font stack for `lang`: Inter and JetBrains Mono first, then the craft-fonts faces
/// (none without `CRAFT_FONTS_DIR`) and installed system fonts on demand, unless
/// `PDFCRAFT_SYSTEM_FONTS=0`. Install it with [`UiFonts::install`] when the language changes.
pub fn ui_fonts(lang: Lang) -> UiFonts<'static> {
    let platform = current_platform();
    let proportional = FontRef::proportional();
    let monospace = FontRef::monospace();
    let medium = FontRef::named(MEDIUM);
    let semibold = FontRef::named(SEMIBOLD);
    let codes: Vec<&str> = LANGUAGES.iter().map(|l| l.code).collect();
    let system_fonts_off = std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0");
    let mut emoji = Emoji::default();
    emoji.monochrome = noto_emoji();
    let fonts = UiFonts::new(&SCRIPTS)
        .app_face("JetBrainsMono", own_face(platform, "JetBrainsMono", JETBRAINS_MONO), std::slice::from_ref(&monospace))
        .app_face("Inter", own_face(platform, "Inter", pdfcraft_fonts::INTER_REGULAR), &[proportional.clone(), monospace.clone()])
        .app_face("Inter-Medium", own_face(platform, "Inter-Medium", INTER_MEDIUM), std::slice::from_ref(&medium))
        .app_face("Inter-SemiBold", own_face(platform, "Inter-SemiBold", INTER_SEMIBOLD), std::slice::from_ref(&semibold))
        .family(Family::named(MEDIUM).weight(500))
        .family(Family::named(SEMIBOLD).weight(600))
        // As production's default fonts, with no system face: Hack draws the triangles and arrows (▸, ⇧) that
        // Inter and JetBrains Mono lack in every family, the weighted ones included, and Noto Emoji the check mark
        // and other text symbols.
        .egui_face("Hack", &[proportional, monospace, medium, semibold])
        .emoji(emoji)
        .craft_faces(pdfcraft_fonts::CRAFT_FONTS)
        .languages(&codes)
        .ui_language(lang.code())
        .cjk_rows(CjkRows::Grow);
    let fonts = match os_locale() {
        Some(locale) => fonts.os_locale(&locale),
        None => fonts,
    };
    if system_fonts_off { fonts.on_demand(Arc::new(NoCandidates)) } else { fonts.on_demand(Arc::new(SystemCandidates)) }
}

const INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

/// The OS locale tag, which orders the regional Han faces when the interface language has none.
#[cfg(not(target_arch = "wasm32"))]
fn os_locale() -> Option<String> {
    static LOCALE: LazyLock<Option<String>> =
        LazyLock::new(|| ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty())));
    LOCALE.clone()
}

#[cfg(target_arch = "wasm32")]
fn os_locale() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use egui::epaint::text::{FontDefinitions, Fonts, TextOptions};
    use egui::{Color32, FontFamily, FontId};

    use super::*;

    fn layout(defs: FontDefinitions, family: FontFamily, size: f32, ppp: f32, text: &str) -> Vec<(char, f32, f32)> {
        let mut fonts = Fonts::new(TextOptions::default(), defs);
        let galley = fonts.with_pixels_per_point(ppp).layout_no_wrap(text.to_owned(), FontId::new(size, family), Color32::WHITE);
        galley.rows.iter().flat_map(|row| row.glyphs.iter().map(|g| (g.chr, g.pos.x, g.pos.y))).collect()
    }

    fn baselines(fallback: FontData, size: f32, text: &str) -> Vec<f32> {
        let mut defs = FontDefinitions::empty();
        defs.font_data.insert("Inter".to_owned(), Arc::new(FontData::from_static(pdfcraft_fonts::INTER_REGULAR)));
        defs.font_data.insert("fallback".to_owned(), Arc::new(fallback));
        defs.families.insert(FontFamily::Proportional, vec!["Inter".to_owned(), "fallback".to_owned()]);
        layout(defs, FontFamily::Proportional, size, 1.0, text).into_iter().map(|(_, _, y)| y).collect()
    }

    fn mono_fonts(platform: FontPolicyPlatform) -> FontDefinitions {
        let mut defs = FontDefinitions::empty();
        defs.font_data.insert("JetBrainsMono".to_owned(), Arc::new(own_face(platform, "JetBrainsMono", JETBRAINS_MONO)));
        defs.families.insert(FontFamily::Monospace, vec!["JetBrainsMono".to_owned()]);
        defs
    }

    #[test]
    fn fallback_faces_share_the_interface_baseline() {
        // Dancing Script's metrics differ from Inter's the way a Chinese face's do (by 0.044 em),
        // and it has letters Inter doesn't (Ǆ), which egui then takes from it.
        let dancing: &'static [u8] = include_bytes!("../../../assets/fonts/DancingScript.ttf");
        let size = 100.0;
        // As egui lays it out by itself, Ǆ sits ~4 pt off H's baseline: the bug (#1676).
        let raw = baselines(FontData::from_static(dancing), size, "HǄ");
        assert!((raw[1] - raw[0]).abs() > 3.0, "the test fonts must differ: {raw:?}");
        let aligned = baselines(on_interface_baseline(FontData::from_static(dancing)), size, "HǄ");
        assert!((aligned[1] - aligned[0]).abs() <= 1.0, "{aligned:?}");
        assert_eq!(on_interface_baseline(FontData::from_static(dancing)).tweak.fallback_align, FallbackAlign::Baseline);
    }

    #[test]
    fn windows_hinting_changes_only_the_jetbrains_symmetric_rendering_field() {
        for name in ["Inter", "Inter-Medium", "Inter-SemiBold", "JetBrainsMono"] {
            assert_eq!(primary_tweak(FontPolicyPlatform::Other, name), FontTweak::default(), "non-Windows {name}");
            let windows = primary_tweak(FontPolicyPlatform::Windows, name);
            if name == "JetBrainsMono" {
                assert_eq!(
                    windows,
                    FontTweak {
                        hinting_target: HintingTarget::Smooth(SmoothHinting { symmetric_rendering: false, ..SmoothHinting::default() }),
                        ..FontTweak::default()
                    }
                );
            } else {
                assert_eq!(windows, FontTweak::default(), "Windows {name}");
            }
        }
    }

    #[test]
    #[ignore = "measurement probe against egui's rasteriser; run with --ignored when retuning the hinting"]
    fn windows_hinting_improves_low_dpi_mono_raster_without_changing_layout_or_coverage() {
        // The raster sharpness itself is a visual measure; what must hold is that the layout the
        // hinting leaves alone is identical, whatever the density.
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            for size in [11.0, 13.0, 16.0] {
                for text in ["fn main() { 0Oo1lI }", "Ünïcode ѝ", "0123456789 {}[]()"] {
                    let a = layout(mono_fonts(FontPolicyPlatform::Other), FontFamily::Monospace, size, ppp, text);
                    let b = layout(mono_fonts(FontPolicyPlatform::Windows), FontFamily::Monospace, size, ppp, text);
                    assert_eq!(a, b, "{size} pt, {ppp} ppp, {text:?}");
                }
            }
        }
    }
}
