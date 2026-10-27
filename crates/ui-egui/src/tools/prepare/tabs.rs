use super::draft::{ACTION_KINDS, ActionDraft, FieldDraft};
use super::*;

/// The categories of the Format tab.
#[derive(Clone, Copy, PartialEq)]
enum Category {
    None,
    Number,
    Percent,
    Date,
    Time,
    Special,
}

fn category(f: &Format) -> Category {
    match f {
        Format::None => Category::None,
        Format::Number { .. } => Category::Number,
        Format::Percent { .. } => Category::Percent,
        Format::Date(_) => Category::Date,
        Format::Time(_) => Category::Time,
        Format::Special(_) | Format::Mask(_) => Category::Special,
    }
}

const SEPARATORS: [&str; 5] = ["1,234.56", "1234.56", "1.234,56", "1234,56", "1'234.56"];

const NEGATIVES: [&str; 4] = ["-1,234.01", "1,234.01 (red)", "(1,234.01)", "(1,234.01) (red)"];

const SPECIALS: [&str; 5] = ["Zip Code", "Zip Code + 4", "Phone Number", "Social Security Number", "Arbitrary Mask"];

/// Draw the field-format editor for the selected category and its options.
pub(super) fn format_tab(ui: &mut egui::Ui, d: &mut FieldDraft, t: &crate::theme::Tokens) {
    let cat = category(&d.format);
    let mut picked = cat;
    ui.horizontal(|ui| {
        ui.label(tl!("Select format category:"));
        egui::ComboBox::from_id_salt("format-cat")
            .selected_text(tl!(match cat {
                Category::None => "None",
                Category::Number => "Number",
                Category::Percent => "Percentage",
                Category::Date => "Date",
                Category::Time => "Time",
                Category::Special => "Special",
            }))
            .show_ui(ui, |ui| {
                for (c, l) in [
                    (Category::None, tl!("None")),
                    (Category::Number, tl!("Number")),
                    (Category::Percent, tl!("Percentage")),
                    (Category::Date, tl!("Date")),
                    (Category::Time, tl!("Time")),
                    (Category::Special, tl!("Special")),
                ] {
                    ui.selectable_value(&mut picked, c, l);
                }
            });
    });
    if picked != cat {
        d.format = match picked {
            Category::None => Format::None,
            Category::Number => Format::Number { decimals: 2, sep: 0, neg: 0, currency: String::new(), prepend: true },
            Category::Percent => Format::Percent { decimals: 2, sep: 0 },
            Category::Date => Format::Date("mm/dd/yyyy".into()),
            Category::Time => Format::Time("HH:MM".into()),
            Category::Special => Format::Special(0),
        };
    }
    ui.add_space(8.0);
    let sample = match &d.format {
        Format::Number { .. } | Format::Percent { .. } => "-1234.5",
        Format::Date(_) => "10/1/2026",
        Format::Time(_) => "14:05",
        Format::Special(2) => "5551234567",
        Format::Special(3) => "123456789",
        Format::Special(_) => "123456789",
        _ => "",
    };
    match &mut d.format {
        Format::None => {
            ui.label(egui::RichText::new(tl!("The value is shown as typed.")).color(t.text_muted));
        }
        Format::Number { decimals, sep, neg, currency, prepend } => {
            egui::Grid::new("fmt-number").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tl!("Decimal places:"));
                ui.add(egui::DragValue::new(decimals).range(0..=10));
                ui.end_row();
                ui.label(tl!("Separator style:"));
                egui::ComboBox::from_id_salt("fmt-sep").selected_text(SEPARATORS[(*sep).min(4) as usize]).show_ui(ui, |ui| {
                    for (i, s) in SEPARATORS.iter().enumerate() {
                        ui.selectable_value(sep, i as u8, *s);
                    }
                });
                ui.end_row();
                ui.label(tl!("Currency symbol:"));
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("fmt-cur")
                        .selected_text(if currency.is_empty() { tl!("None") } else { currency.as_str() })
                        .width(70.0)
                        .show_ui(ui, |ui| {
                            for c in ["", "$", "€", "£", "¥", "CHF "] {
                                ui.selectable_value(currency, c.to_string(), if c.is_empty() { tl!("None") } else { c });
                            }
                        });
                    ui.add(widgets::checkbox(prepend, tl!("Before the number")));
                });
                ui.end_row();
                ui.label(tl!("Negative number style:"));
                egui::ComboBox::from_id_salt("fmt-neg").selected_text(NEGATIVES[(*neg).min(3) as usize]).show_ui(ui, |ui| {
                    for (i, s) in NEGATIVES.iter().enumerate() {
                        ui.selectable_value(neg, i as u8, *s);
                    }
                });
                ui.end_row();
            });
        }
        Format::Percent { decimals, sep } => {
            egui::Grid::new("fmt-pct").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tl!("Decimal places:"));
                ui.add(egui::DragValue::new(decimals).range(0..=10));
                ui.end_row();
                ui.label(tl!("Separator style:"));
                egui::ComboBox::from_id_salt("fmt-psep").selected_text(SEPARATORS[(*sep).min(4) as usize]).show_ui(ui, |ui| {
                    for (i, s) in SEPARATORS.iter().enumerate() {
                        ui.selectable_value(sep, i as u8, *s);
                    }
                });
                ui.end_row();
            });
        }
        Format::Date(p) | Format::Time(p) => {
            let presets: Vec<&str> = if matches!(cat, Category::Time) || picked == Category::Time {
                TIME_PRESETS.to_vec()
            } else {
                DATE_PRESETS.iter().copied().chain(["m/d/yyyy", "mm/dd/yyyy", "yyyy-mm-dd", "dd/mm/yyyy"]).collect()
            };
            ui.horizontal(|ui| {
                ui.label(tl!("Format:"));
                egui::ComboBox::from_id_salt("fmt-date").selected_text(p.as_str()).width(170.0).show_ui(ui, |ui| {
                    for pr in presets {
                        ui.selectable_value(p, pr.to_string(), pr);
                    }
                });
                let custom = ui.label(tl!("Custom:"));
                ui.add(crate::widgets::line(p).desired_width(120.0)).labelled_by(custom.id);
            });
        }
        Format::Special(n) => {
            let mut idx = *n as usize;
            egui::ComboBox::from_id_salt("fmt-special").selected_text(tl!(SPECIALS[idx.min(4)])).show_ui(ui, |ui| {
                for (i, s) in SPECIALS.iter().enumerate() {
                    ui.selectable_value(&mut idx, i, tl!(s));
                }
            });
            if idx == 4 {
                d.format = Format::Mask("999-999".into());
            } else {
                *n = idx as u8;
            }
        }
        Format::Mask(m) => {
            let mut idx = 4usize;
            egui::ComboBox::from_id_salt("fmt-special").selected_text(tl!(SPECIALS[4])).show_ui(ui, |ui| {
                for (i, s) in SPECIALS.iter().enumerate() {
                    ui.selectable_value(&mut idx, i, tl!(s));
                }
            });
            ui.horizontal(|ui| {
                let mask = ui.label(tl!("Mask:"));
                ui.add(crate::widgets::line(m).desired_width(160.0)).labelled_by(mask.id);
            });
            ui.label(egui::RichText::new(tl!("9 digit, A letter, O letter or digit, X any character")).small().color(t.text_faint));
            if idx < 4 {
                d.format = Format::Special(idx as u8);
            }
        }
    }
    if !sample.is_empty() && d.format != Format::None {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(crate::i18n::fmt(tl!("Example: {sample}"), &[("sample", &format_value(&d.format, sample))])).color(t.text_muted),
        );
    }
}

pub(super) fn validate_tab(ui: &mut egui::Ui, d: &mut FieldDraft) {
    let mut ranged = matches!(d.validate, Validate::Range { .. });
    widgets::radio_value(ui, &mut ranged, false, tl!("Field value is not validated"));
    widgets::radio_value(ui, &mut ranged, true, tl!("Field value is in range:"));
    if !ranged {
        d.validate = Validate::None;
        return;
    }
    if d.validate == Validate::None {
        d.validate = Validate::Range { min: Some(0.0), max: Some(100.0) };
    }
    if let Validate::Range { min, max } = &mut d.validate {
        ui.horizontal(|ui| {
            let mut has = min.is_some();
            ui.add(widgets::checkbox(&mut has, tl!("From:")));
            let mut v = min.unwrap_or(0.0);
            ui.add_enabled(has, egui::DragValue::new(&mut v).speed(1.0));
            *min = has.then_some(v);
            let mut has = max.is_some();
            ui.add(widgets::checkbox(&mut has, tl!("To:")));
            let mut v = max.unwrap_or(100.0);
            ui.add_enabled(has, egui::DragValue::new(&mut v).speed(1.0));
            *max = has.then_some(v);
        });
    }
}

pub(super) fn actions_tab(ui: &mut egui::Ui, d: &mut FieldDraft, t: &crate::theme::Tokens) {
    use pdfcraft_engine::FieldTrigger as T;
    let editing = d.editing.is_some();
    ui.label(egui::RichText::new(if editing { tl!("Edit Action") } else { tl!("Add an Action") }).font(theme::semibold(13.0)));
    let a = &mut d.new_action;
    egui::Grid::new("field-actions-add").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Select Trigger:"));
        // Editing changes the script of one trigger's action: its trigger and kind stay.
        ui.add_enabled_ui(!editing, |ui| {
            egui::ComboBox::from_id_salt("action-trigger").selected_text(tl!(a.trigger.label())).width(220.0).show_ui(ui, |ui| {
                for tr in T::ALL {
                    ui.selectable_value(&mut a.trigger, tr, tl!(tr.label()));
                }
            });
        });
        ui.end_row();
        ui.label(tl!("Select Action:"));
        ui.add_enabled_ui(!editing, |ui| {
            egui::ComboBox::from_id_salt("action-kind").selected_text(tl!(ACTION_KINDS[a.kind])).width(220.0).show_ui(ui, |ui| {
                for (i, k) in ACTION_KINDS.iter().enumerate() {
                    ui.selectable_value(&mut a.kind, i, tl!(k));
                }
            });
        });
        ui.end_row();
        let hint = match a.kind {
            0 => "JavaScript",
            1 => "https://…",
            2 => tl!("Fields to reset (comma-separated; empty: all)"),
            3 => tl!("Print, NextPage, PrevPage, FirstPage or LastPage"),
            4 => tl!("Page number"),
            5 | 6 => tl!("Field names (comma-separated)"),
            _ => tl!("URL to submit to"),
        };
        ui.label(if a.kind == 0 { tl!("Script:") } else { tl!("Value:") });
        if a.kind == 0 {
            ui.add(crate::widgets::block(&mut a.text).code_editor().desired_rows(3).desired_width(340.0).hint_text(hint).id_salt("action-text"));
        } else {
            ui.add(crate::widgets::line(&mut a.text).desired_width(340.0).hint_text(hint).id_salt("action-text"));
        }
        ui.end_row();
    });
    let mut error = None;
    let (mut commit, mut discard) = (false, false);
    ui.horizontal(|ui| {
        commit = ui.button(if editing { tl!("Update") } else { tl!("Add") }).clicked();
        discard = editing && ui.button(tl!("Discard")).clicked();
    });
    if commit {
        match d.new_action.action() {
            Ok(act) => {
                let tr = d.new_action.trigger;
                d.actions.retain(|(x, _)| *x != tr);
                d.actions.push((tr, act));
                d.actions.sort_by_key(|(x, _)| T::ALL.iter().position(|y| y == x));
                d.new_action.text.clear();
                d.editing = None;
            }
            Err(e) => error = Some(e),
        }
    }
    if discard {
        d.new_action = ActionDraft::default();
        d.editing = None;
    }
    if let Some(e) = error {
        ui.label(egui::RichText::new(tl!(&e)).small().color(t.text_muted));
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Actions")).font(theme::semibold(13.0)));
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if d.actions.is_empty() {
            ui.label(egui::RichText::new(tl!("No actions")).color(t.text_muted));
        }
        let (mut remove, mut edit) = (None, None);
        for (i, (tr, act)) in d.actions.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!(tr.label())).strong());
                let more = d.chained.iter().find(|(x, _)| x == tr).map_or(0, |(_, n)| *n);
                if more == 0 {
                    ui.label(act.describe());
                } else {
                    let then = crate::i18n::trn(crate::i18n::current(), more as u64, "then {n} more action", "then {n} more actions");
                    ui.label(format!("{}, {then}", act.describe()));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.push_id(i, |ui| {
                        if ui.button(tl!("Delete")).clicked() {
                            remove = Some(i);
                        }
                        if let pdfcraft_engine::FieldAction::JavaScript(js) = act
                            && ui.add_enabled(d.editing.is_none(), egui::Button::new(tl!("Edit"))).clicked()
                        {
                            edit = Some((*tr, js.clone()));
                        }
                    });
                });
            });
        }
        if let Some((trigger, text)) = edit {
            d.new_action = ActionDraft { trigger, kind: 0, text };
            d.editing = Some(trigger);
        }
        if let Some(i) = remove {
            if d.actions.get(i).map(|(x, _)| Some(*x)) == Some(d.editing) {
                d.new_action = ActionDraft::default();
                d.editing = None;
            }
            d.actions.remove(i);
        }
    });
}

pub(super) fn calculate_tab(ui: &mut egui::Ui, d: &mut FieldDraft, t: &crate::theme::Tokens) {
    #[derive(PartialEq, Clone, Copy)]
    enum Mode {
        None,
        Simple,
        Notation,
    }
    let mode = match d.calculate {
        Calculate::None => Mode::None,
        Calculate::Simple { .. } => Mode::Simple,
        Calculate::Notation(_) => Mode::Notation,
    };
    let mut m = mode;
    widgets::radio_value(ui, &mut m, Mode::None, tl!("Value is not calculated"));
    widgets::radio_value(ui, &mut m, Mode::Simple, tl!("Value is the"));
    if m != mode {
        d.calculate = match m {
            Mode::None => Calculate::None,
            Mode::Simple => Calculate::Simple { op: CalcOp::Sum, fields: Vec::new() },
            Mode::Notation => Calculate::Notation(String::new()),
        };
    }
    if let Calculate::Simple { op, fields } = &mut d.calculate {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            egui::ComboBox::from_id_salt("calc-op").selected_text(tl!(op.label())).show_ui(ui, |ui| {
                for o in CalcOp::ALL {
                    ui.selectable_value(op, o, tl!(o.label()));
                }
            });
            ui.label(tl!("of the following fields:"));
        });
        egui::ScrollArea::vertical().max_height(110.0).show(ui, |ui| {
            for name in &d.others {
                let mut on = fields.contains(name);
                if ui.add(widgets::checkbox(&mut on, name)).changed() {
                    if on {
                        fields.push(name.clone());
                    } else {
                        fields.retain(|f| f != name);
                    }
                }
            }
        });
    }
    let mut m2 = match d.calculate {
        Calculate::Notation(_) => Mode::Notation,
        _ => m,
    };
    if widgets::radio_value(ui, &mut m2, Mode::Notation, tl!("Simplified field notation:")).clicked()
        && !matches!(d.calculate, Calculate::Notation(_))
    {
        d.calculate = Calculate::Notation(String::new());
    }
    if let Calculate::Notation(expr) = &mut d.calculate {
        ui.add(crate::widgets::block(expr).hint_text("Price * Quantity").desired_rows(2).desired_width(420.0));
        ui.label(egui::RichText::new(tl!("Field names with + - * / and parentheses; put \\ before spaces in names.")).small().color(t.text_faint));
    }
}
