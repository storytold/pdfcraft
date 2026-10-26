//! Format text, laid out as Acrobat's panel: the font across the panel; size, colour, lists and
//! alignment; bold, italic, underline, superscript and subscript; line and paragraph spacing;
//! horizontal scale and character spacing. One panel serves a paragraph being edited (with the
//! fonts already on its page and the spacing controls) and added text (the standard families,
//! size, colour, style, alignment and character spacing). What PdfCraft can't apply yet (lists,
//! superscript and subscript, paragraph spacing, or line spacing and scale for added text) is shown
//! disabled and says so.
//!
//! The icons are Lucide's (AGENTS.md §1); only the arrangement follows Acrobat.

use egui::{Align, Color32, CornerRadius, Layout, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::{AddedText, FontFamily, TextAlign};

use crate::edit_text_ui::Extras;
use crate::theme::Tokens;
use crate::{icons, widgets};

/// A font picked from the font list. The caller applies it, since keeping the paragraph's own
/// font also means restoring the family it started with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FontChoice {
    /// The fonts the paragraph already uses.
    KeepSource,
    /// One of the page's fonts, by resource name.
    Resource(String),
    /// A standard family.
    Family(FontFamily),
    /// Load font…: pick a font file.
    Load,
    /// A font file installed on this computer.
    Installed(std::path::PathBuf),
}

/// What the font list offers and shows.
pub(crate) struct Fonts<'a> {
    /// The fonts on the paragraph's page (resource, face); `None` for added text, which has the
    /// standard families only.
    pub sources: Option<&'a [(String, String)]>,
    /// The name the list shows for the current choice.
    pub shown: String,
}

/// What the panel changed: the style (in place) and a font picked from the list.
#[derive(Default)]
pub(crate) struct Outcome {
    pub changed: bool,
    pub font: Option<FontChoice>,
}

const SIZES: [f64; 14] = [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0, 48.0, 72.0];
const LINE_SPACINGS: [f64; 7] = [1.0, 1.15, 1.2, 1.5, 2.0, 2.5, 3.0];
const SCALES: [f64; 8] = [50.0, 75.0, 90.0, 100.0, 110.0, 125.0, 150.0, 200.0];
const CHAR_SPACINGS: [f64; 8] = [-1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 3.0, 5.0];
/// A row's height: the tallest control in it (the 30 pt icon buttons). Every control is centred
/// on the row, whatever its own height, so lists, swatches and buttons line up (#3037).
const ROW: f32 = 30.0;
/// The gap between a row's controls.
const GAP: f32 = 6.0;
/// The icon naming a list or field ([`labelled_icon`]).
const LABEL_ICON: f32 = 22.0;
/// The button beside a number field that opens its list of common values: just an arrow, so the
/// field keeps the room its value needs.
const PRESET_LIST: f32 = 22.0;
/// What can be typed: horizontal scale in percent, character spacing in points.
const SCALE_RANGE: std::ops::RangeInclusive<f64> = 10.0..=400.0;
const CHAR_SPACING_RANGE: std::ops::RangeInclusive<f64> = -5.0..=50.0;

/// A number as the lists show it: no trailing zeros ("1.2", "100", "-0.5").
fn number(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, look: &mut AddedText, extras: Option<&mut Extras>, fonts: Fonts) -> Outcome {
    let before = (look.clone(), extras.as_deref().copied());
    let mut out = Outcome::default();
    widgets::section_title(ui, tl!("Format text"));

    // The font, across the panel.
    let width = ui.available_width();
    egui::ComboBox::from_id_salt("format-font")
        .selected_text(fonts.shown.as_str())
        .width(width)
        .show_ui(ui, |ui| {
            if let Some(sources) = fonts.sources {
                if ui.selectable_label(false, tl!("Keep source fonts")).clicked() {
                    out.font = Some(FontChoice::KeepSource);
                }
                for (resource, face) in sources.iter().filter(|(r, _)| !r.starts_with("PCApproved")) {
                    if ui.selectable_label(false, face).clicked() {
                        out.font = Some(FontChoice::Resource(resource.clone()));
                    }
                }
                ui.separator();
            }
            for f in [FontFamily::Helvetica, FontFamily::Times, FontFamily::Courier] {
                if ui.selectable_label(fonts.sources.is_none() && look.family == f, f.label()).clicked() {
                    out.font = Some(FontChoice::Family(f));
                }
            }
            if fonts.sources.is_some() {
                ui.separator();
                if ui.selectable_label(false, tl!("Load font…")).clicked() {
                    out.font = Some(FontChoice::Load);
                }
                #[cfg(not(target_arch = "wasm32"))]
                ui.menu_button(tl!("Installed fonts…"), |ui| match crate::files::installed_font_files(ui.ctx()) {
                    Some(list) if list.is_empty() => {
                        ui.label(tl!("No openly licensed document fonts found. Load a font file."));
                    }
                    Some(list) => {
                        for font in list {
                            if ui.button(&font.face).clicked() {
                                out.font = Some(FontChoice::Installed(font.path));
                                ui.close();
                            }
                        }
                    }
                    None => {
                        ui.label(tl!("Finding document fonts…"));
                    }
                });
            }
        })
        .response
        .on_hover_text(format!("{}\n{}", tl!("Font"), tl!("Thai and Arabic use regular fallback fonts when available.")));
    ui.add_space(4.0);

    // Size and colour | bullets, numbering and alignment.
    row(ui, |ui| {
        let _ = egui::ComboBox::from_id_salt("format-size")
            .selected_text(number(look.size))
            .width(64.0)
            .show_ui(ui, |ui| {
                for size in SIZES {
                    ui.selectable_value(&mut look.size, size, number(size));
                }
            })
            .response
            .on_hover_text(tl!("Font size"));
        let swatch = colour_button(ui, t, look.color);
        egui::Popup::menu(&swatch).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            if let Some(picked) = crate::comments::swatch_grid(ui, Some(look.color)) {
                look.color = picked;
                ui.close();
            }
        });
        let (sep, _) = ui.allocate_exact_size(vec2(9.0, 26.0), Sense::hover());
        ui.painter().vline(sep.center().x, sep.y_range().shrink(3.0), Stroke::new(1.0, t.divider));
        planned(ui, "list", tl!("Bulleted list"));
        planned(ui, "list-ordered", tl!("Numbered list"));
        let current = match look.align {
            TextAlign::Left => "align-left",
            TextAlign::Center => "align-center",
            TextAlign::Right => "align-right",
            TextAlign::Justify => "align-justify",
        };
        let align = icons::button(ui, current, 30.0, false, tl!("Alignment"));
        let align = flyout(ui, t, align);
        egui::Popup::menu(&align).show(|ui| {
            ui.horizontal(|ui| {
                for (a, icon, tip) in [
                    (TextAlign::Left, "align-left", tl!("Align left")),
                    (TextAlign::Center, "align-center", tl!("Centre")),
                    (TextAlign::Right, "align-right", tl!("Align right")),
                    (TextAlign::Justify, "align-justify", tl!("Justify")),
                ] {
                    if icons::button(ui, icon, 30.0, look.align == a, tip).clicked() {
                        look.align = a;
                        ui.close();
                    }
                }
            });
        });
    });

    // Bold, italic, underline, superscript, subscript.
    let mut extras = extras;
    row(ui, |ui| {
        if icons::button(ui, "bold", 30.0, look.bold, tl!("Bold")).clicked() {
            look.bold = !look.bold;
        }
        if icons::button(ui, "italic", 30.0, look.italic, tl!("Italic")).clicked() {
            look.italic = !look.italic;
        }
        match extras.as_deref_mut() {
            Some(e) => {
                if icons::button(ui, "underline", 30.0, e.underline, tl!("Underline")).clicked() {
                    e.underline = !e.underline;
                }
            }
            None => planned(ui, "underline", tl!("Underline")),
        }
        planned(ui, "superscript", tl!("Superscript"));
        planned(ui, "subscript", tl!("Subscript"));
    });

    // Line and paragraph spacing; horizontal scale and character spacing: two columns, each an
    // icon and its control, `half` wide.
    let half = ((ui.available_width() - 2.0 * LABEL_ICON - 8.0 - 3.0 * GAP) / 2.0).max(40.0);
    let editing = extras.is_some();
    let mut fallback = Extras::default();
    let e = extras.as_deref_mut().unwrap_or(&mut fallback);
    ui.add_enabled_ui(editing, |ui| {
        row(ui, |ui| {
            labelled_icon(ui, t, "arrow-up-down", tl!("Line spacing"));
            let own = tl!("Auto").to_string();
            let shown = if e.line_spacing == 0.0 { own.clone() } else { number(e.line_spacing) };
            egui::ComboBox::from_id_salt("format-line-spacing").selected_text(shown).width(half).show_ui(ui, |ui| {
                ui.selectable_value(&mut e.line_spacing, 0.0, own.as_str());
                for v in LINE_SPACINGS {
                    ui.selectable_value(&mut e.line_spacing, v, number(v));
                }
            });
            ui.add_space(8.0);
            ui.add_enabled_ui(false, |ui| {
                labelled_icon(ui, t, "pilcrow", tl!("Paragraph spacing"));
                let tip = format!("{} — {}", tl!("Paragraph spacing"), tl!("Planned"));
                let _ = egui::ComboBox::from_id_salt("format-paragraph-spacing")
                    .selected_text("0")
                    .width(half)
                    .show_ui(ui, |_| {})
                    .response
                    .on_disabled_hover_text(tip);
            });
        });
    });
    // Any value the engine takes can be typed; the lists hold the common ones. Added text has
    // character spacing of its own (#4245), but no scale yet.
    row(ui, |ui| {
        ui.add_enabled_ui(editing, |ui| {
            let label = labelled_icon(ui, t, "move-horizontal", tl!("Horizontal scale"));
            let scale = Number { value: &mut e.scale, range: SCALE_RANGE, speed: 1.0, suffix: " %" };
            number_with_presets(ui, scale, &SCALES, half, (label.id, tl!("Horizontal scale")));
        });
        ui.add_space(8.0);
        let label = labelled_icon(ui, t, "arrow-left-right", tl!("Character spacing"));
        let value = if editing { &mut e.char_spacing } else { &mut look.char_spacing };
        let spacing = Number { value, range: CHAR_SPACING_RANGE, speed: 0.1, suffix: " pt" };
        number_with_presets(ui, spacing, &CHAR_SPACINGS, half, (label.id, tl!("Character spacing")));
    });

    out.changed = (look.clone(), extras.as_deref().copied()) != before;
    ui.add_space(6.0);
    ui.separator();
    out
}

/// One row of the panel, [`ROW`] high, its controls [`GAP`] apart and centred on it.
fn row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let size = vec2(ui.available_width(), ROW);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        // A list lays itself out from the default control height and grows downward, which leaves
        // it below the row's middle; at a list's own height it is centred like the rest.
        let list = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
        ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(list);
        add(ui)
    })
    .inner
}

/// A control PdfCraft doesn't have yet: its icon, disabled, saying so when hovered.
fn planned(ui: &mut egui::Ui, icon: &str, label: &str) {
    let _ = ui
        .add_enabled_ui(false, |ui| icons::button(ui, icon, 30.0, false, label))
        .inner
        .on_disabled_hover_text(format!("{label} — {}", tl!("Planned")));
}

/// The small corner mark Acrobat puts on a button that opens a menu.
fn flyout(ui: &egui::Ui, t: &Tokens, resp: egui::Response) -> egui::Response {
    let r = resp.rect;
    let tri = vec![r.right_bottom() + vec2(-3.0, -3.0), r.right_bottom() + vec2(-8.0, -3.0), r.right_bottom() + vec2(-3.0, -8.0)];
    ui.painter().add(egui::Shape::convex_polygon(tri, t.text_faint, Stroke::NONE));
    resp
}

/// An icon naming the list beside it, for screen readers too. Returns it, for the control it
/// labels.
fn labelled_icon(ui: &mut egui::Ui, t: &Tokens, icon: &str, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(LABEL_ICON, 26.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, ui.is_enabled(), label));
    let tint = if ui.is_enabled() { t.icon } else { t.text_faint };
    icons::paint(ui, Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), icon, 17.0, tint);
    resp.on_hover_text(label)
}

/// A value typed in a number field: what it edits, what may be typed, and how it reads.
struct Number<'a> {
    value: &'a mut f64,
    range: std::ops::RangeInclusive<f64>,
    speed: f64,
    suffix: &'static str,
}

/// A number field with an arrow beside it that lists common values, in `width` all told: type any
/// value in its range, or pick one. Both are named by `label`: the icon before them and its text.
fn number_with_presets(ui: &mut egui::Ui, n: Number<'_>, presets: &[f64], width: f32, label: (egui::Id, &str)) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let field = egui::DragValue::new(&mut *n.value).range(n.range).speed(n.speed).max_decimals(2).suffix(n.suffix);
        let height = ui.spacing().interact_size.y;
        ui.add_sized([(width - PRESET_LIST - 2.0).max(30.0), height], field).labelled_by(label.0);
        let list = icons::button(ui, "chevron-down", PRESET_LIST, false, label.1);
        egui::Popup::menu(&list).show(|ui| {
            for &v in presets {
                if ui.selectable_label(*n.value == v, number(v)).clicked() {
                    *n.value = v;
                    ui.close();
                }
            }
        });
    });
}

/// The text colour as a square swatch that opens the palette.
fn colour_button(ui: &mut egui::Ui, t: &Tokens, color: [f64; 3]) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Colour")));
    let [r, g, b] = color.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    let fill = rect.shrink(4.0);
    ui.painter().rect_filled(fill, CornerRadius::same(2), Color32::from_rgb(r, g, b));
    let stroke = if resp.hovered() { t.text } else { t.border };
    ui.painter().rect_stroke(fill, CornerRadius::same(2), Stroke::new(1.0, stroke), egui::StrokeKind::Outside);
    crate::theme::hand(resp).on_hover_text(tl!("Colour"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn numbers_read_like_the_lists() {
        assert_eq!(super::number(1.2), "1.2");
        assert_eq!(super::number(100.0), "100");
        assert_eq!(super::number(-0.5), "-0.5");
        assert_eq!(super::number(0.0), "0");
        assert_eq!(super::number(1.15), "1.15");
    }
}
