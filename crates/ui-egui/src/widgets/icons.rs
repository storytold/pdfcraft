//! Vector icons: embedded Lucide SVGs, recoloured to white, rasterized by egui_extras at the exact
//! on-screen pixel size, and tinted per use (PhotoCraft lesson: never use Unicode glyphs as icons).

use std::sync::OnceLock;

use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
use craft_ui::icons::{SvgIconSet, SvgStyle};

use egui::{Color32, Rect, Response, Stroke, Vec2};

use crate::icon_data::ICONS;
use crate::theme::{self, Tokens};

fn white_icons() -> &'static SvgIconSet {
    static MAP: OnceLock<SvgIconSet> = OnceLock::new();
    MAP.get_or_init(|| SvgIconSet::new(ICONS, SvgStyle { current_color: "#ffffff", stroke_width: Some("1.75") }, "bytes://icons/", "square"))
}

pub fn exists(name: &str) -> bool {
    white_icons().contains(name)
}

pub(crate) fn source(name: &str) -> egui::ImageSource<'static> {
    // PdfCraft caches an unknown name under the resolved fallback name.
    white_icons().source(white_icons().canonical_name(name).unwrap_or("square"))
}

pub fn image(name: &str, size: f32, tint: Color32) -> egui::Image<'static> {
    egui::Image::new(source(name)).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}

pub fn paint(ui: &egui::Ui, rect: Rect, name: &str, size: f32, tint: Color32) {
    image(name, size, tint).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(size)));
}

/// Square icon button: transparent until hovered; `selected` gets the accent treatment.
/// `tooltip` is both its accessible name and its one tooltip.
pub fn button(ui: &mut egui::Ui, name: &str, box_size: f32, selected: bool, tooltip: &str) -> Response {
    labeled_button(ui, name, box_size, selected, tooltip, tooltip)
}

/// [`button`] whose tooltip says more than its accessible name (`label`). It shows that one
/// tooltip, so callers add no `on_hover_text` of their own (#6548: two tooltips stacked).
pub fn labeled_button(ui: &mut egui::Ui, name: &str, box_size: f32, selected: bool, label: &str, tooltip: &str) -> Response {
    styled_button(ui, name, box_size, selected, label, tooltip, None)
}

/// [`button`], with the glyph in `tint` rather than the theme's icon colour (an icon that shows a
/// colour the user chose). A colour that would vanish on the bar gets a backing (#6644).
pub fn tinted_button(ui: &mut egui::Ui, name: &str, box_size: f32, tooltip: &str, tint: Color32) -> Response {
    styled_button(ui, name, box_size, false, tooltip, tooltip, Some(tint))
}

/// The contrast below which a user's colour can't be told from the surface it is drawn on.
const MIN_TINT_CONTRAST: f32 = 2.0;
const BACKING_DARK: Color32 = Color32::from_rgb(58, 58, 64);
const BACKING_LIGHT: Color32 = Color32::from_rgb(238, 238, 242);

/// The backing a glyph in `tint` needs on `surface`: none if it shows, else whichever neutral it
/// stands out from.
fn tint_backing(tint: Color32, surface: Color32) -> Option<Color32> {
    (theme::contrast(tint, surface) < MIN_TINT_CONTRAST)
        .then(|| if theme::contrast(tint, BACKING_DARK) >= theme::contrast(tint, BACKING_LIGHT) { BACKING_DARK } else { BACKING_LIGHT })
}

fn styled_button(ui: &mut egui::Ui, name: &str, box_size: f32, selected: bool, label: &str, tooltip: &str, chosen: Option<Color32>) -> Response {
    let t = Tokens::get(ui.ctx());
    let icon = chosen.unwrap_or(t.icon);
    // The shared button paints no fill of its own: the eased press wash below replaces it.
    let clear = |icon| ButtonVisuals { fill: Color32::TRANSPARENT, stroke: Stroke::NONE, icon };
    let style = IconButtonStyle {
        normal: clear(icon),
        hovered: clear(icon),
        pressed: Some(clear(icon)),
        selected: clear(t.accent_text),
        corner_radius: t.radius.into(),
        focus_stroke: Stroke::new(1.0, t.accent),
    };
    let label = if label.is_empty() { name } else { label };
    // The wash eases from the response, so reserve its place under the icon and focus ring.
    let wash = ui.painter().add(egui::Shape::Noop);
    let mut painted = None;
    let resp = IconButton::new(label, Vec2::splat(box_size), &style).selected(selected).show(ui, |_, rect, tint| painted = Some((rect, tint)));
    if let Some((rect, tint)) = painted {
        let press = theme::Press::track(ui, &resp);
        let mut shapes = Vec::with_capacity(2);
        let fill = press.wash_color(&t, selected);
        if fill.a() > 0 {
            shapes.push(egui::Shape::rect_filled(rect, t.radius, fill));
        }
        if selected {
            shapes.push(egui::Shape::rect_stroke(
                rect.shrink(0.5),
                t.radius,
                Stroke::new(1.0, t.accent.gamma_multiply(0.45)),
                egui::StrokeKind::Inside,
            ));
        }
        if let Some(backing) = chosen.and_then(|c| tint_backing(c, t.card)) {
            let side = (box_size * 0.5).round() + 8.0;
            shapes.push(egui::Shape::rect_filled(Rect::from_center_size(rect.center() + press.offset(), Vec2::splat(side)), t.radius, backing));
        }
        ui.painter().set(wash, egui::Shape::Vec(shapes));
        // The glyph shrinks a little and drops a pixel while the pointer is down.
        let size = (box_size * 0.5).round() * (1.0 - 0.08 * press.down);
        paint(ui, rect.translate(press.offset()), name, size, tint);
    }
    let resp = theme::hand(resp);
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_tint_that_would_vanish_gets_a_backing_it_stands_out_from() {
        let (white, black, red) = (Color32::WHITE, Color32::BLACK, Color32::from_rgb(227, 33, 33));
        let (light_bar, dark_bar) = (Color32::from_rgb(255, 255, 255), Color32::from_rgb(47, 47, 52));
        assert_eq!(tint_backing(white, light_bar), Some(BACKING_DARK), "white on a light bar");
        assert_eq!(tint_backing(black, dark_bar), Some(BACKING_LIGHT), "black on a dark bar");
        assert_eq!(tint_backing(black, light_bar), None);
        assert_eq!(tint_backing(white, dark_bar), None);
        assert_eq!(tint_backing(red, light_bar), None);
        assert_eq!(tint_backing(red, dark_bar), None);
    }

    #[test]
    fn every_catalog_icon_exists() {
        for g in pdfcraft_engine::catalog::TOOL_GROUPS {
            assert!(super::exists(g.icon), "missing icon {}", g.icon);
            for s in g.sections {
                for i in s.items {
                    assert!(super::exists(i.icon), "missing icon {}", i.icon);
                }
            }
        }
    }

    #[test]
    fn every_command_icon_exists() {
        for c in pdfcraft_engine::commands::COMMANDS {
            assert!(super::exists(c.icon), "missing icon {} ({})", c.icon, c.id);
        }
    }

    #[test]
    fn shared_catalog_preserves_existing_icons_and_fallback_uri() {
        assert_eq!(ICONS.len(), 191);
        assert!(exists("minimize-2"));
        for (name, original) in ICONS {
            let expected =
                String::from_utf8_lossy(original).replace("currentColor", "#ffffff").replace("stroke-width=\"2\"", "stroke-width=\"1.75\"");
            let egui::ImageSource::Bytes { uri, bytes } = source(name) else { panic!("expected SVG bytes") };
            assert_eq!(uri.as_ref(), format!("bytes://icons/{name}.svg"));
            assert_eq!(bytes.as_ref(), expected.as_bytes(), "{name}");
        }
        let egui::ImageSource::Bytes { uri, bytes } = source("missing-icon") else { panic!("expected SVG bytes") };
        let egui::ImageSource::Bytes { bytes: fallback, .. } = source("square") else { panic!("expected SVG bytes") };
        assert_eq!(uri.as_ref(), "bytes://icons/square.svg");
        assert_eq!(bytes.as_ref(), fallback.as_ref());
        assert!(!exists("missing-icon"));
    }

    #[test]
    fn shared_button_preserves_pointer_responses() {
        // The shared button must respond like the original allocate-and-paint path in both
        // themes. Its painting differs on purpose (the eased press wash, the selected outline
        // and the sinking glyph); the keyboard-focus outline is covered in craft-ui.
        fn frame(
            ctx: &egui::Context,
            events: Vec<egui::Event>,
            selected: bool,
            enabled: bool,
            legacy: bool,
        ) -> (egui::Response, Vec<egui::epaint::ClippedShape>) {
            let mut response = None;
            let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                response = Some(
                    ui.add_enabled_ui(enabled, |ui| {
                        if legacy {
                            let t = Tokens::get(ui.ctx());
                            let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::click());
                            if selected {
                                ui.painter().rect_filled(rect, t.radius, t.accent_soft);
                            } else if response.is_pointer_button_down_on() {
                                ui.painter().rect_filled(rect, t.radius, t.pressed);
                            } else if response.hovered() {
                                ui.painter().rect_filled(rect, t.radius, t.hover);
                            }
                            let tint = if selected { t.accent_text } else { t.icon };
                            paint(ui, rect, "square", (28.0_f32 * 0.5).round(), tint);
                            response
                        } else {
                            button(ui, "square", 28.0, selected, "")
                        }
                    })
                    .inner,
                );
            });
            output.textures_delta.clear();
            (response.unwrap(), output.shapes)
        }
        for theme in [crate::theme::ThemeKind::Dark, crate::theme::ThemeKind::Light] {
            for selected in [false, true] {
                for enabled in [false, true] {
                    let old = egui::Context::default();
                    let new = egui::Context::default();
                    for ctx in [&old, &new] {
                        crate::theme::apply(ctx, theme);
                        egui_extras::install_image_loaders(ctx);
                    }
                    let mut center = egui::Pos2::ZERO;
                    for step in 0..7 {
                        let events = match step {
                            2 => vec![egui::Event::PointerMoved(center)],
                            3 | 4 => vec![egui::Event::PointerButton {
                                pos: center,
                                button: egui::PointerButton::Primary,
                                pressed: step == 3,
                                modifiers: egui::Modifiers::NONE,
                            }],
                            5 => vec![egui::Event::PointerMoved(egui::pos2(300.0, 200.0))],
                            _ => vec![],
                        };
                        let (a, _) = frame(&old, events.clone(), selected, enabled, true);
                        let (b, after) = frame(&new, events, selected, enabled, false);
                        center = a.rect.center();
                        assert_eq!(a.rect, b.rect);
                        assert_eq!(
                            (a.clicked(), a.hovered(), a.is_pointer_button_down_on()),
                            (b.clicked(), b.hovered(), b.is_pointer_button_down_on())
                        );
                        assert!(!after.is_empty(), "{theme:?}, selected={selected}, enabled={enabled}, step={step}");
                    }
                }
            }
        }
    }
}
