use super::*;

/// Arabic and Kurdish switch stock chrome to mirror from the right; English switches it
/// back. This is the `Style::right_to_left` half of the explicit RTL policy.
#[test]
fn arabic_chrome_is_right_to_left_after_the_language_changes() {
    let previous = crate::i18n::current();
    let ctx = egui::Context::default();
    crate::i18n::set_current(crate::i18n::Lang::from_code("ar").expect("registered language"));
    apply(&ctx, ThemeKind::Light);
    assert!(ctx.global_style().right_to_left);
    crate::i18n::set_current(crate::i18n::Lang::EN);
    sync_direction(&ctx);
    assert!(!ctx.global_style().right_to_left);
    crate::i18n::set_current(previous);
}

/// #3193: a bookmark's own colour is kept where it reads on the panel, and moved towards the
/// theme's text colour where it doesn't (dark blue on the dark panel), never past it.
#[test]
fn a_document_colour_is_kept_where_it_reads() {
    let (light, dark) = (Tokens::for_kind(ThemeKind::Light), Tokens::for_kind(ThemeKind::Dark));
    let red = Color32::from_rgb(255, 0, 0);
    assert_eq!(readable(red, light.panel, light.text), red, "red reads on white");
    let navy = Color32::from_rgb(0, 0, 128);
    assert_eq!(readable(navy, light.panel, light.text), navy);
    let lifted = readable(navy, dark.panel, dark.text);
    assert_ne!(lifted, navy, "navy is lost on the dark panel");
    assert!(contrast(lifted, dark.panel) >= 3.0, "{lifted:?}");
    assert_ne!(lifted, dark.text, "only as far as it needs");
    // Even the panel's own colour comes out readable.
    assert!(contrast(readable(dark.panel, dark.panel, dark.text), dark.panel) >= 3.0);
    assert!(contrast(readable(light.panel, light.panel, light.text), light.panel) >= 3.0);
}
use egui::epaint::text::{Fonts, TextOptions};

const HINTING_CORPUS: [&str; 4] = ["Il1MWmw", "Hamburgefonts", "0123456789 .,;:!?()[]{}+-=/\\", "Café Ångström déjà-vu Ωμέγα Ελληνικά Журнал"];
const HINTING_SIZES: [f32; 6] = [10.0, 11.0, 12.0, 13.0, 14.0, 16.0];
const HINTING_PPPS: [f32; 4] = [1.0, 1.25, 1.5, 2.0];

fn primary_families() -> [(FontFamily, &'static str); 4] {
    [
        (FontFamily::Proportional, "Inter"),
        (FontFamily::Name("medium".into()), "Inter-Medium"),
        (FontFamily::Name("semibold".into()), "Inter-SemiBold"),
        (FontFamily::Monospace, "JetBrainsMono"),
    ]
}

fn raster_edge_and_ink(fonts: &mut Fonts, id: FontId, text: &str, ppp: f32) -> (u64, u64) {
    let galley = {
        let mut view = fonts.with_pixels_per_point(ppp);
        view.layout_no_wrap(text.to_owned(), id, Color32::BLACK).as_ref().clone()
    };
    let image = fonts.image();
    let mut edge = 0u64;
    let mut ink = 0u64;
    for glyph in galley.rows.iter().flat_map(|row| &row.glyphs) {
        let [x0, y0] = glyph.uv_rect.min.map(usize::from);
        let [x1, y1] = glyph.uv_rect.max.map(usize::from);
        for y in y0..y1 {
            for x in x0..x1 {
                let alpha = image.pixels[y * image.size[0] + x].a();
                ink += u64::from(alpha);
                if x > x0 {
                    let left = image.pixels[y * image.size[0] + x - 1].a();
                    edge += u64::from(alpha.abs_diff(left));
                }
            }
        }
    }
    (edge, ink)
}

fn mono_raster_measure(fonts: &mut Fonts, ppp: f32) -> (u64, u64) {
    let mut edge = 0;
    let mut ink = 0;
    for size in HINTING_SIZES {
        for text in HINTING_CORPUS {
            let (glyph_edge, glyph_ink) = raster_edge_and_ink(fonts, FontId::monospace(size), text, ppp);
            edge += glyph_edge;
            ink += glyph_ink;
        }
    }
    (edge, ink)
}

fn layout_signature(galley: &egui::Galley) -> Vec<u32> {
    let mut signature = vec![
        galley.rect.min.x.to_bits(),
        galley.rect.min.y.to_bits(),
        galley.rect.max.x.to_bits(),
        galley.rect.max.y.to_bits(),
        u32::from(galley.elided),
        galley.rows.len() as u32,
    ];
    for row in &galley.rows {
        signature.extend([
            u32::from(row.ends_with_newline),
            row.pos.x.to_bits(),
            row.pos.y.to_bits(),
            row.size.x.to_bits(),
            row.size.y.to_bits(),
            row.glyphs.len() as u32,
        ]);
        for glyph in &row.glyphs {
            signature.extend([
                glyph.chr as u32,
                glyph.pos.x.to_bits(),
                glyph.pos.y.to_bits(),
                glyph.advance_width.to_bits(),
                glyph.line_height.to_bits(),
                glyph.font_ascent.to_bits(),
                glyph.font_height.to_bits(),
                glyph.font_face_ascent.to_bits(),
                glyph.font_face_height.to_bits(),
            ]);
        }
    }
    signature
}

fn galley(fonts: &mut Fonts, family: FontFamily, size: f32, ppp: f32, text: &str) -> egui::Galley {
    let mut view = fonts.with_pixels_per_point(ppp);
    view.layout_no_wrap(text.to_owned(), FontId::new(size, family), Color32::BLACK).as_ref().clone()
}

fn hinting_test_fonts(platform: FontPolicyPlatform) -> Fonts {
    Fonts::new(
        TextOptions {
            subpixel_binning: false,
            color_transfer_function: egui::epaint::FontColorTransferFunction::Gamma(0.75),
            ..TextOptions::default()
        },
        font_definitions_for_platform(false, platform),
    )
}

/// The y position of each glyph of `text` laid out by egui at `size` points, in points from
/// the top of the row.
fn baselines(fallback: FontData, size: f32, text: &str) -> Vec<f32> {
    let mut defs = FontDefinitions::empty();
    defs.font_data.insert("Inter".into(), Arc::new(FontData::from_static(pdfcraft_fonts::INTER_REGULAR)));
    defs.font_data.insert("fallback".into(), Arc::new(fallback));
    defs.families.insert(FontFamily::Proportional, vec!["Inter".into(), "fallback".into()]);
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    let g = galley(&mut fonts, FontFamily::Proportional, size, 1.0, text);
    g.rows[0].glyphs.iter().map(|gl| gl.pos.y).collect()
}

#[test]
fn fallback_faces_share_the_interface_baseline() {
    // Dancing Script's metrics differ from Inter's the way a Chinese face's do (by 0.044 em),
    // and it has letters Inter doesn't (Ǆ), which egui then takes from it.
    let dancing: &'static [u8] = include_bytes!("../../../../assets/fonts/DancingScript.ttf");
    let size = 100.0;
    // As egui lays it out by itself, Ǆ sits ~4 pt off H's baseline: the bug (#1676).
    let raw = baselines(FontData::from_static(dancing), size, "HǄ");
    assert!((raw[1] - raw[0]).abs() > 3.0, "the test fonts must differ: {raw:?}");
    // `FallbackAlign::Baseline` puts every face on the same baseline, to the pixel.
    let aligned = baselines(on_interface_baseline(FontData::from_static(dancing)), size, "HǄ");
    assert!((aligned[1] - aligned[0]).abs() <= 1.0, "{aligned:?}");
    assert_eq!(on_interface_baseline(FontData::from_static(dancing)).tweak.fallback_align, FallbackAlign::Baseline);
}

/// The policy only exists on native platforms (`installed_font_definitions` has no installed
/// faces on wasm), so the test must not use it in a wasm test build.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn traditional_chinese_keeps_an_installed_face_when_hans_is_embedded() {
    use crate::system_fonts::Need;
    assert_eq!(installed_fallback_need(CjkPreference::SimplifiedChinese, true), Need::Other);
    assert_eq!(installed_fallback_need(CjkPreference::SimplifiedChinese, false), Need::Hans);
    assert_eq!(installed_fallback_need(CjkPreference::TraditionalChinese, true), Need::Hant);
    assert_eq!(installed_fallback_need(CjkPreference::TraditionalChinese, false), Need::Hant);
    assert_eq!(installed_fallback_need(CjkPreference::Default, true), Need::Other);
}

#[test]
fn windows_hinting_changes_only_the_jetbrains_symmetric_rendering_field() {
    let baseline = font_definitions_for_platform(false, FontPolicyPlatform::Other);
    let windows = font_definitions_for_platform(false, FontPolicyPlatform::Windows);

    assert_eq!(baseline.families, windows.families);
    assert_eq!(baseline.font_data.keys().collect::<Vec<_>>(), windows.font_data.keys().collect::<Vec<_>>());
    for (family, name) in primary_families() {
        let baseline_data = &baseline.font_data[name];
        let windows_data = &windows.font_data[name];
        assert_eq!(baseline_data.tweak, FontTweak::default(), "baseline {name}");
        if name == "JetBrainsMono" {
            assert_eq!(
                windows_data.tweak,
                FontTweak {
                    hinting_target: HintingTarget::Smooth(SmoothHinting { symmetric_rendering: false, ..SmoothHinting::default() }),
                    ..FontTweak::default()
                },
                "Windows {name}"
            );
        } else {
            assert_eq!(windows_data.tweak, baseline_data.tweak, "Windows {name}");
        }
        assert_eq!(primary_font_tweak(FontPolicyPlatform::Other, name), baseline_data.tweak, "non-Windows policy for {family:?}");
    }
    for (name, data) in &baseline.font_data {
        if !["Inter", "Inter-Medium", "Inter-SemiBold", "JetBrainsMono"].contains(&name.as_str()) {
            assert_eq!(windows.font_data[name].tweak, data.tweak, "Windows fallback {name}");
        }
    }
    assert_eq!(
        font_definitions_for(false).font_data["JetBrainsMono"].tweak,
        font_definitions_for_platform(false, current_font_policy_platform()).font_data["JetBrainsMono"].tweak
    );
}

#[test]
#[ignore = "measurement probe: asserts exact egui rasterisation deltas, which change with egui upgrades; run with --ignored when retuning the hinting"]
fn windows_hinting_improves_low_dpi_mono_raster_without_changing_layout_or_coverage() {
    let mut baseline = hinting_test_fonts(FontPolicyPlatform::Other);
    let mut windows = hinting_test_fonts(FontPolicyPlatform::Windows);

    for ppp in HINTING_PPPS {
        let (baseline_edge, baseline_ink) = mono_raster_measure(&mut baseline, ppp);
        let (windows_edge, windows_ink) = mono_raster_measure(&mut windows, ppp);
        // Compare horizontal alpha-edge energy per ink alpha; this is a deterministic
        // vertical-stem sharpness proxy, not an absolute typography quality score.
        let baseline_sharpness = baseline_edge as f64 / baseline_ink as f64;
        let windows_sharpness = windows_edge as f64 / windows_ink as f64;
        let sharpness_change = windows_sharpness / baseline_sharpness - 1.0;
        let weight_change = windows_ink as f64 / baseline_ink as f64 - 1.0;
        if ppp < 2.0 {
            assert!(sharpness_change > 0.002, "{ppp} ppp sharpness change: {sharpness_change:.2}%", sharpness_change = sharpness_change * 100.0);
        } else {
            assert!(sharpness_change >= -0.01, "{ppp} ppp sharpness change: {sharpness_change:.2}%", sharpness_change = sharpness_change * 100.0);
        }
        assert!(weight_change.abs() < 0.05, "{ppp} ppp ink change: {weight_change:.2}%", weight_change = weight_change * 100.0);
    }

    for ppp in HINTING_PPPS {
        for size in HINTING_SIZES {
            for (family, _) in primary_families() {
                for text in HINTING_CORPUS {
                    assert_eq!(
                        baseline.has_glyphs(&FontId::new(size, family.clone()), text),
                        windows.has_glyphs(&FontId::new(size, family.clone()), text),
                        "{family:?} {text:?}"
                    );
                    assert_eq!(
                        layout_signature(&galley(&mut baseline, family.clone(), size, ppp, text)),
                        layout_signature(&galley(&mut windows, family.clone(), size, ppp, text)),
                        "{family:?}, {size} pt, {ppp} ppp, {text:?}"
                    );
                }
            }
        }
    }
}

/// "Use system setting" draws what the operating system says, dark or light.
#[test]
fn following_the_system_draws_what_the_system_says() {
    assert_eq!(ThemePreference::System.resolve(Some(true), ThemeKind::Light), ThemeKind::Dark);
    assert_eq!(ThemePreference::System.resolve(Some(false), ThemeKind::Dark), ThemeKind::Light);
}

/// An operating system that says nothing leaves the last choice standing, and an explicit
/// Light or Dark is never overridden by what the system says.
#[test]
fn an_answerless_system_leaves_the_choice_and_an_explicit_one_stands() {
    assert_eq!(ThemePreference::System.resolve(None, ThemeKind::Light), ThemeKind::Light);
    assert_eq!(ThemePreference::System.resolve(None, ThemeKind::Dark), ThemeKind::Dark);
    assert_eq!(ThemePreference::Light.resolve(Some(true), ThemeKind::Dark), ThemeKind::Light);
    assert_eq!(ThemePreference::Dark.resolve(Some(false), ThemeKind::Light), ThemeKind::Dark);
}

#[test]
fn press_darkens_then_sinks() {
    let rest = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    let hover = Color32::from_rgb(0xF0, 0xF0, 0xF3);
    let pressed = Color32::from_rgb(0xE4, 0xE4, 0xE9);
    let idle = Press { hover: 0.0, down: 0.0 };
    assert_eq!(idle.fill(rest, hover, pressed), rest);
    assert_eq!(idle.offset(), Vec2::ZERO);
    let held = Press { hover: 1.0, down: 1.0 };
    assert_eq!(held.fill(rest, hover, pressed), pressed);
    assert_eq!(held.offset(), Vec2::new(0.0, 1.0));
}

#[test]
fn text_is_readable_on_every_surface() {
    // #76: the faintest text (hints, zoom level, captions) was 3.3:1 in the light theme.
    for kind in [ThemeKind::Light, ThemeKind::Dark] {
        let t = Tokens::for_kind(kind);
        for (name, fg) in [("text", t.text), ("text_muted", t.text_muted), ("text_faint", t.text_faint)] {
            for bg in [t.chrome, t.panel, t.card, t.pasteboard] {
                let r = contrast(fg, bg);
                assert!(r >= 4.5, "{kind:?} {name} on {bg:?}: {r:.2}:1, WCAG AA needs 4.5:1");
            }
        }
    }
}
