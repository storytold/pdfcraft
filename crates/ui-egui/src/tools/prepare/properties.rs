use super::draft::{FieldDraft, FieldTab};
use super::tabs::{actions_tab, calculate_tab, format_tab, validate_tab};
use super::{BorderStyle, FieldFont, FormFieldKind, theme, widgets};

/// A check box for a `/Ff` bit (`inverted`: checked when the bit is clear).
fn flag_box(ui: &mut egui::Ui, flags: &mut u32, bit: u32, inverted: bool, label: &str) {
    let mut on = (*flags & bit != 0) != inverted;
    if ui.add(widgets::checkbox(&mut on, label)).changed() {
        if on != inverted {
            *flags |= bit;
        } else {
            *flags &= !bit;
        }
    }
}

/// Shared PDF `/Q` picker for text and choice-field Options.
fn alignment(ui: &mut egui::Ui, quadding: &mut i64) {
    ui.label(tl!("Alignment:"));
    craft_ui::ComboBox::from_id_salt("quadding")
        .selected_text(tl!(match (*quadding).clamp(0, 2) {
            1 => "Center",
            2 => "Right",
            _ => "Left",
        }))
        .show_ui(ui, |ui| {
            for (q, label) in [(0, tl!("Left")), (1, tl!("Center")), (2, tl!("Right"))] {
                ui.selectable_value(quadding, q, label);
            }
        });
}

/// Draw Field Properties; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, d: &mut FieldDraft, t: &crate::theme::Tokens) -> (bool, bool) {
    use crate::widgets;
    ui.set_width(600.0);
    ui.label(egui::RichText::new(tl!(d.title())).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for (tab, label) in d.tabs() {
            if widgets::mode_tab(ui, tl!(label), d.tab == tab).clicked() {
                d.tab = tab;
            }
        }
    });
    ui.separator();
    ui.add_space(6.0);
    let mut enter = false;
    // A locked field keeps its properties until it is unlocked (Acrobat greys them out).
    let was_locked = d.original.as_ref().as_ref().is_some_and(|o| o.locked) && d.locked;
    if was_locked {
        ui.label(egui::RichText::new(tl!("This field is locked. Unlock it to change its properties.")).small().color(t.text_muted));
    }
    ui.scope(|ui| {
        if was_locked {
            ui.disable();
        }
        match d.tab {
            FieldTab::General => {
                egui::Grid::new("field-general").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                    let l = ui.label(tl!("Name:"));
                    let r = ui.add(crate::widgets::line(&mut d.name).desired_width(340.0)).labelled_by(l.id);
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.end_row();
                    let l = ui.label(tl!("Tooltip:"));
                    ui.add(crate::widgets::line(&mut d.tooltip).desired_width(340.0)).labelled_by(l.id);
                    ui.end_row();
                });
                ui.add_space(12.0);
                widgets::section_title(ui, tl!("Common Properties"));
                ui.add(widgets::checkbox(&mut d.read_only, tl!("Read Only")));
                ui.add(widgets::checkbox(&mut d.required, tl!("Required")));
            }
            FieldTab::Appearance => {
                egui::Grid::new("field-appearance").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                    ui.label(tl!("Font Size:"));
                    let mut auto = d.font_size == 0.0;
                    ui.horizontal(|ui| {
                        if ui.add(widgets::checkbox(&mut auto, tl!("Auto"))).changed() {
                            d.font_size = if auto { 0.0 } else { 12.0 };
                        }
                        ui.add_enabled(
                            !auto,
                            egui::DragValue::new(&mut d.font_size).range(2.0..=100.0).clamp_existing_to_range(false).speed(0.25).suffix(" pt"),
                        );
                    });
                    ui.end_row();
                    if let Some(l) = d.look.as_mut() {
                        ui.label(tl!("Font:"));
                        craft_ui::ComboBox::from_id_salt("field-font").selected_text(l.font.label()).show_ui(ui, |ui| {
                            for f in FieldFont::ALL {
                                ui.selectable_value(&mut l.font, f, f.label());
                            }
                        });
                        ui.end_row();
                        ui.label(tl!("Text Color:"));
                        if let Some(c) = crate::comments::swatch_grid(ui, Some(l.text)) {
                            l.text = c;
                        }
                        ui.end_row();
                    }
                });
                if let Some(l) = d.look.as_mut() {
                    widgets::section_title(ui, tl!("Borders and Colors"));
                    egui::Grid::new("field-borders").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        for (index, (label, slot)) in
                            [(tl!("Border Color:"), &mut l.border), (tl!("Fill Color:"), &mut l.fill)].into_iter().enumerate()
                        {
                            ui.label(label);
                            ui.push_id(("field-color", index), |ui| {
                                ui.horizontal(|ui| {
                                    let mut none = slot.is_none();
                                    if ui.add(widgets::checkbox(&mut none, tl!("No color"))).changed() {
                                        *slot = if none { None } else { Some([0.0; 3]) };
                                    }
                                    if let Some(c) = crate::comments::swatch_grid(ui, *slot) {
                                        *slot = Some(c);
                                    }
                                });
                            });
                            ui.end_row();
                        }
                        ui.label(tl!("Line Thickness:"));
                        let thick = |w: f64| {
                            if w <= 1.0 {
                                tl!("Thin")
                            } else if w <= 2.0 {
                                tl!("Medium")
                            } else {
                                tl!("Thick")
                            }
                        };
                        craft_ui::ComboBox::from_id_salt("field-width").selected_text(thick(l.width)).show_ui(ui, |ui| {
                            for (w, label) in [(1.0, tl!("Thin")), (2.0, tl!("Medium")), (3.0, tl!("Thick"))] {
                                ui.selectable_value(&mut l.width, w, label);
                            }
                        });
                        ui.end_row();
                        ui.label(tl!("Line Style:"));
                        craft_ui::ComboBox::from_id_salt("field-style").selected_text(tl!(l.style.label())).show_ui(ui, |ui| {
                            for s in BorderStyle::ALL {
                                ui.selectable_value(&mut l.style, s, tl!(s.label()));
                            }
                        });
                        ui.end_row();
                    });
                }
            }
            FieldTab::Position => {
                egui::Grid::new("field-position").num_columns(4).spacing([12.0, 10.0]).show(ui, |ui| {
                    for (i, label) in [tl!("Left:"), tl!("Bottom:"), tl!("Width:"), tl!("Height:")].into_iter().enumerate() {
                        ui.label(label);
                        let range = if i < 2 { -14_400.0..=14_400.0 } else { 4.0..=14_400.0 };
                        ui.add(egui::DragValue::new(&mut d.position[i]).range(range).speed(0.5).suffix(" pt"));
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
                ui.label(egui::RichText::new(tl!("Points from the page's bottom-left corner.")).small().color(t.text_faint));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let l = ui.label(tl!("Rotation:"));
                    let before = d.rotation;
                    craft_ui::ComboBox::from_id_salt("field-rotation")
                        .selected_text(format!("{}°", d.rotation))
                        .show_ui(ui, |ui| {
                            for r in [0, 90, 180, 270] {
                                if ui.selectable_value(&mut d.rotation, r, format!("{r}°")).changed() && (before % 180 == 0) != (r % 180 == 0) {
                                    // Keep the same center. set_props skips its own swap when the rect changed too.
                                    let [x, y, w, h] = d.position;
                                    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
                                    d.position = [cx - h / 2.0, cy - w / 2.0, h, w];
                                }
                            }
                        })
                        .response
                        .labelled_by(l.id);
                });
            }
            FieldTab::Format => format_tab(ui, d, t),
            FieldTab::Validate => validate_tab(ui, d),
            FieldTab::Calculate => calculate_tab(ui, d, t),
            FieldTab::Actions => actions_tab(ui, d, t),
            FieldTab::Options => match d.kind {
                FormFieldKind::PushButton => {
                    ui.horizontal(|ui| {
                        let l = ui.label(tl!("Label"));
                        ui.add(crate::widgets::line(&mut d.caption).desired_width(340.0)).labelled_by(l.id);
                    });
                }
                FormFieldKind::Text => {
                    use pdfcraft_engine::field_flags as ff;
                    egui::Grid::new("text-options").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        alignment(ui, &mut d.quadding);
                        ui.end_row();
                        let l = ui.label(tl!("Default Value:"));
                        ui.add(crate::widgets::line(&mut d.default).desired_width(260.0)).labelled_by(l.id);
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    ui.add(widgets::checkbox(&mut d.multiline, tl!("Multi-line")));
                    flag_box(ui, &mut d.flags, ff::DO_NOT_SCROLL, true, tl!("Scroll long text"));
                    flag_box(ui, &mut d.flags, ff::RICH_TEXT, false, tl!("Allow Rich Text Formatting"));
                    ui.horizontal(|ui| {
                        ui.add(widgets::checkbox(&mut d.limit, tl!("Limit of")));
                        ui.add_enabled(d.limit, egui::DragValue::new(&mut d.max_len).range(0..=10_000));
                        ui.label(tl!("characters"));
                    });
                    flag_box(ui, &mut d.flags, ff::PASSWORD, false, tl!("Password"));
                    flag_box(ui, &mut d.flags, ff::FILE_SELECT, false, tl!("Field is used for file selection"));
                    flag_box(ui, &mut d.flags, ff::DO_NOT_SPELL_CHECK, true, tl!("Check spelling"));
                    ui.horizontal(|ui| {
                        flag_box(ui, &mut d.flags, ff::COMB, false, tl!("Comb of"));
                        if d.flags & ff::COMB != 0 {
                            d.limit = true;
                            d.multiline = false;
                            d.flags &= !(ff::PASSWORD | ff::FILE_SELECT);
                            if d.max_len == 0 {
                                d.max_len = 10;
                            }
                        }
                        ui.add_enabled(d.flags & ff::COMB != 0, egui::DragValue::new(&mut d.max_len).range(1..=500));
                        ui.label(tl!("characters"));
                    });
                }
                FormFieldKind::CheckBox | FormFieldKind::Radio => {
                    use pdfcraft_engine::field_flags as ff;
                    let on = d.on_state.clone();
                    ui.horizontal(|ui| {
                        let label = ui.label(tl!("Export value"));
                        ui.text_edit_singleline(&mut d.export_value).labelled_by(label.id);
                    });
                    let mut checked = !d.default.is_empty() && d.default == on;
                    let label =
                        if d.kind == FormFieldKind::CheckBox { tl!("Check box is checked by default") } else { tl!("Button is checked by default") };
                    if ui.add(widgets::checkbox(&mut checked, label)).changed() {
                        d.default = if checked { on } else { String::new() };
                    }
                    if let Some(style) = d.check_style.as_mut() {
                        ui.horizontal(|ui| {
                            let l = ui.label(tl!(if d.kind == FormFieldKind::CheckBox { "Check Box Style:" } else { "Button Style:" }));
                            craft_ui::ComboBox::from_id_salt("check-style")
                                .selected_text(tl!(style.label()))
                                .show_ui(ui, |ui| {
                                    for s in pdfcraft_engine::CheckStyle::ALL {
                                        ui.selectable_value(style, s, tl!(s.label()));
                                    }
                                })
                                .response
                                .labelled_by(l.id);
                        });
                    }
                    if d.kind == FormFieldKind::Radio {
                        flag_box(ui, &mut d.flags, ff::RADIOS_IN_UNISON, false, tl!("Buttons with the same name and value are selected in unison"));
                    }
                }
                _ => {
                    ui.horizontal(|ui| alignment(ui, &mut d.quadding));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let l = ui.label(tl!("Item:"));
                        let r = ui.add(crate::widgets::line(&mut d.new_option).desired_width(240.0)).labelled_by(l.id);
                        let typed = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (widgets::pill_button(ui, tl!("Add"), false).clicked() || typed) && !d.new_option.trim().is_empty() {
                            d.options.push(d.new_option.trim().to_string());
                            d.new_option.clear();
                        }
                    });
                    ui.add_space(6.0);
                    widgets::section_title(ui, tl!("Item List"));
                    let mut remove = None;
                    let mut up = None;
                    egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                        for (i, o) in d.options.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(o);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button(tl!("Delete")).on_hover_text(crate::i18n::fmt(tl!("Delete {o}"), &[("o", o)])).clicked() {
                                        remove = Some(i);
                                    }
                                    if i > 0 && ui.small_button(tl!("Up")).clicked() {
                                        up = Some(i);
                                    }
                                });
                            });
                        }
                    });
                    if let Some(i) = remove {
                        d.options.remove(i);
                    }
                    if let Some(i) = up {
                        d.options.swap(i - 1, i);
                    }
                    if d.options.is_empty() {
                        ui.label(egui::RichText::new(tl!("Add the choices people pick from.")).color(t.text_muted));
                    }
                    use pdfcraft_engine::field_flags as ff;
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(tl!("Default:"));
                        let shown = if d.default.is_empty() { tl!("(none)").to_string() } else { d.default.clone() };
                        craft_ui::ComboBox::from_id_salt("choice-default").selected_text(shown).show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.default, String::new(), tl!("(none)"));
                            for o in d.options.clone() {
                                ui.selectable_value(&mut d.default, o.clone(), o);
                            }
                        });
                    });
                    flag_box(ui, &mut d.flags, ff::SORT, false, tl!("Sort items"));
                    if d.kind == FormFieldKind::Combo {
                        flag_box(ui, &mut d.flags, ff::EDIT, false, tl!("Allow user to enter custom text"));
                        flag_box(ui, &mut d.flags, ff::DO_NOT_SPELL_CHECK, true, tl!("Check spelling"));
                    } else {
                        flag_box(ui, &mut d.flags, ff::MULTI_SELECT, false, tl!("Multiple selection"));
                    }
                    flag_box(ui, &mut d.flags, ff::COMMIT_ON_SEL_CHANGE, false, tl!("Commit selected value immediately"));
                }
            },
        }
    });
    ui.add_space(8.0);
    ui.add(widgets::checkbox(&mut d.locked, tl!("Locked")));
    ui.add_space(6.0);
    let (mut apply, mut cancel) = (enter, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}
