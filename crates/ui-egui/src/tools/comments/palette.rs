//! Saved highlight presets and their draft editor. The first preset is the new-highlight default.
use super::{CommentPrefs, CommentTool, Rgb, SWATCHES, color32, icons};
use serde::{Deserialize, Serialize};

pub const MAX_PRESETS: usize = 12;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HighlightPreset {
    pub color: Rgb,
    pub opacity: f64,
}

impl HighlightPreset {
    pub fn name(&self) -> String {
        SWATCHES
            .iter()
            .find(|(_, c)| super::menu::same_colour(c, &self.color))
            .map(|(name, _)| tl!(name).to_string())
            .unwrap_or_else(|| hex(self.color))
    }
}

pub(super) fn defaults() -> Vec<HighlightPreset> {
    SWATCHES.iter().take(6).map(|(_, color)| HighlightPreset { color: *color, opacity: 1.0 }).collect()
}

pub fn hex(color: Rgb) -> String {
    let [r, g, b, _] = color32(color).to_array();
    format!("#{r:02X}{g:02X}{b:02X}")
}

pub fn parse_hex(text: &str) -> Option<Rgb> {
    let s = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some([f64::from((n >> 16) & 255) / 255.0, f64::from((n >> 8) & 255) / 255.0, f64::from(n & 255) / 255.0])
}

/// Read persisted settings or the control channel's JSON, without partially applying bad input.
pub fn from_json(value: &serde_json::Value) -> Result<Vec<HighlightPreset>, String> {
    let rows = value.as_array().filter(|v| !v.is_empty() && v.len() <= MAX_PRESETS).ok_or("Choose 1 to 12 highlight colors")?;
    rows.iter()
        .map(|row| {
            let color = match row["color"].as_str() {
                Some(s) => parse_hex(s),
                None => serde_json::from_value::<Rgb>(row["color"].clone()).ok(),
            }
            .filter(|c| c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)))
            .ok_or("Use a six-digit hex color or three RGB values from 0 to 1")?;
            let opacity = row["opacity"].as_f64().filter(|v| v.is_finite() && (0.1..=1.0).contains(v)).ok_or("Opacity must be between 0.1 and 1")?;
            Ok(HighlightPreset { color, opacity })
        })
        .collect()
}

impl CommentPrefs {
    pub fn highlight_palette(&self) -> &[HighlightPreset] {
        &self.highlight_palette
    }

    pub fn set_highlight_palette(&mut self, presets: Vec<HighlightPreset>) -> Result<(), String> {
        // Validate public callers too; no invisible colors or unbounded toolbar.
        let presets = from_json(&serde_json::to_value(presets).map_err(|e| e.to_string())?)?;
        if let Some(first) = presets.first() {
            self.set_color(CommentTool::Highlight, first.color);
            self.set_opacity(CommentTool::Highlight, first.opacity);
            self.set_part_opacity(CommentTool::Highlight, None, None);
        }
        self.highlight_palette = presets;
        Ok(())
    }

    pub(crate) fn begin_palette_edit(&mut self) {
        self.palette_draft =
            Some(PaletteDraft { rows: self.highlight_palette.iter().map(|p| DraftRow { hex: hex(p.color), opacity: p.opacity }).collect() });
    }
}

#[derive(Clone, Debug)]
struct DraftRow {
    hex: String,
    opacity: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct PaletteDraft {
    rows: Vec<DraftRow>,
}

impl PaletteDraft {
    fn presets(&self) -> Option<Vec<HighlightPreset>> {
        self.rows.iter().map(|row| Some(HighlightPreset { color: parse_hex(&row.hex)?, opacity: row.opacity })).collect()
    }
}

/// A compact modal so choosing colors can use a nested native egui picker. Save applies only
/// preferences; closing, Escape or Cancel drops the draft and never restyles the document.
pub(crate) fn editor(ui: &mut egui::Ui, prefs: &mut CommentPrefs) -> bool {
    let Some(mut draft) = prefs.palette_draft.take() else { return true };
    ui.heading(tl!("Highlight palette"));
    ui.label(tl!("Your first color is the default for new highlights."));
    ui.label(egui::RichText::new(tl!("Each color remembers its opacity. Existing highlights stay unchanged until you choose a color.")).small());
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Colors in toolbar"));
        let mut count = draft.rows.len();
        let mut delta: isize = 0;
        if ui.add_enabled_ui(count > 1, |ui| icons::button(ui, "minus", 26.0, false, tl!("Fewer colors"))).inner.clicked() {
            delta -= 1;
        }
        ui.add(egui::DragValue::new(&mut count).range(1..=MAX_PRESETS).speed(0.1)).labelled_by(label.id);
        if ui.add_enabled_ui(count < MAX_PRESETS, |ui| icons::button(ui, "plus", 26.0, false, tl!("More colors"))).inner.clicked() {
            delta += 1;
        }
        count = (count as isize + delta).clamp(1, MAX_PRESETS as isize) as usize;
        if count != draft.rows.len() {
            let count = count.clamp(1, MAX_PRESETS);
            draft.rows.truncate(count);
            while draft.rows.len() < count {
                let color = SWATCHES.get(draft.rows.len()).map_or([0.2, 0.7, 0.4], |(_, c)| *c);
                draft.rows.push(DraftRow { hex: hex(color), opacity: 0.5 });
            }
        }
    });
    ui.add_space(8.0);
    let max = (ui.ctx().content_rect().height() - 310.0).clamp(100.0, 390.0);
    let len = draft.rows.len();
    let mut move_to = None;
    let mut remove = None;
    egui::ScrollArea::vertical().id_salt("highlight-palette-rows").max_height(max).show(ui, |ui| {
        for (i, row) in draft.rows.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(420.0);
                    ui.horizontal(|ui| {
                        ui.strong(format!("{} {}", tl!("Color"), i + 1));
                        if i == 0 {
                            ui.label(egui::RichText::new(tl!("Default")).small().color(ui.visuals().selection.stroke.color));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled_ui(len > 1, |ui| {
                                    icons::button(
                                        ui,
                                        "trash-2",
                                        26.0,
                                        false,
                                        &crate::i18n::fmt(tl!("Remove color {n}"), &[("n", &(i + 1).to_string())]),
                                    )
                                })
                                .inner
                                .clicked()
                            {
                                remove = Some(i);
                            }
                            if ui
                                .add_enabled_ui(i + 1 < len, |ui| {
                                    icons::button(
                                        ui,
                                        "chevron-down",
                                        26.0,
                                        false,
                                        &crate::i18n::fmt(tl!("Move color {n} down"), &[("n", &(i + 1).to_string())]),
                                    )
                                })
                                .inner
                                .clicked()
                            {
                                move_to = Some((i, i + 1));
                            }
                            if ui
                                .add_enabled_ui(i > 0, |ui| {
                                    icons::button(
                                        ui,
                                        "chevron-up",
                                        26.0,
                                        false,
                                        &crate::i18n::fmt(tl!("Move color {n} up"), &[("n", &(i + 1).to_string())]),
                                    )
                                })
                                .inner
                                .clicked()
                            {
                                move_to = Some((i, i - 1));
                            }
                            if ui
                                .add_enabled_ui(i > 0, |ui| {
                                    icons::button(
                                        ui,
                                        "star",
                                        26.0,
                                        i == 0,
                                        &crate::i18n::fmt(tl!("Make color {n} default"), &[("n", &(i + 1).to_string())]),
                                    )
                                })
                                .inner
                                .clicked()
                            {
                                move_to = Some((i, 0));
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        let color = parse_hex(&row.hex).unwrap_or([1.0, 1.0, 1.0]);
                        let c = color32(color);
                        let mut bytes = [c.r(), c.g(), c.b()];
                        if ui.color_edit_button_srgb(&mut bytes).changed() {
                            row.hex = hex(bytes.map(|v| f64::from(v) / 255.0));
                        }
                        ui.label(tl!("Hex"));
                        let before = row.hex.clone();
                        let field = ui.add(egui::TextEdit::singleline(&mut row.hex).desired_width(80.0).char_limit(7));
                        field.widget_info(|| {
                            let mut info = egui::WidgetInfo::text_edit(true, &before, &row.hex, "#RRGGBB");
                            info.label = Some(crate::i18n::fmt(tl!("Hex {n}"), &[("n", &(i + 1).to_string())]));
                            info
                        });
                        ui.label(tl!("Opacity"));
                        let mut percent = row.opacity * 100.0;
                        let opacity = ui.add(egui::DragValue::new(&mut percent).range(10.0..=100.0).suffix("%").speed(1.0));
                        opacity.widget_info(|| {
                            let mut info = egui::WidgetInfo::drag_value(true, percent);
                            info.label = Some(crate::i18n::fmt(tl!("Opacity {n}"), &[("n", &(i + 1).to_string())]));
                            info
                        });
                        if opacity.changed() {
                            row.opacity = if percent.is_finite() { percent.clamp(10.0, 100.0) / 100.0 } else { row.opacity };
                        }
                        // Preview over page white, independent of the application's theme.
                        let (r, _) = ui.allocate_exact_size(egui::vec2(106.0, 28.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 4.0, egui::Color32::WHITE);
                        ui.painter().rect_filled(
                            r.shrink2(egui::vec2(5.0, 5.0)),
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (row.opacity * 255.0).round() as u8),
                        );
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            tl!("Sample text"),
                            super::theme::regular(12.0),
                            egui::Color32::BLACK,
                        );
                    });
                    if parse_hex(&row.hex).is_none() {
                        ui.colored_label(ui.visuals().error_fg_color, tl!("Enter six hex digits, such as #FFD54F."));
                    }
                });
            });
            ui.add_space(5.0);
        }
    });
    if let Some((from, to)) = move_to.filter(|(from, to)| *from < draft.rows.len() && *to < draft.rows.len()) {
        let row = draft.rows.remove(from);
        draft.rows.insert(to, row);
        // Text fields are indexed by row; moved rows must not keep an old cursor's focus.
        ui.memory_mut(|m| m.stop_text_input());
    }
    if let Some(i) = remove.filter(|i| *i < draft.rows.len() && draft.rows.len() > 1) {
        draft.rows.remove(i);
    }
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Toolbar preview")).small());
    ui.horizontal_wrapped(|ui| {
        for row in &draft.rows {
            let c = parse_hex(&row.hex).unwrap_or([0.7; 3]);
            let (r, _) = ui.allocate_exact_size(egui::vec2(24.0, 28.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 8.0, color32(c));
        }
        ui.add_enabled_ui(false, |ui| {
            icons::button(ui, "palette", 28.0, false, tl!("Edit highlight palette"));
            icons::button(ui, "trash-2", 28.0, false, tl!("Delete highlight"));
        });
    });
    ui.separator();
    let valid = draft.presets();
    let mut close = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled(valid.is_some(), egui::Button::new(tl!("Save palette"))).clicked()
            && let Some(presets) = valid
        {
            close = prefs.set_highlight_palette(presets).is_ok();
        }
        if ui.button(tl!("Cancel")).clicked() {
            close = true;
        }
    });
    if !close {
        prefs.palette_draft = Some(draft);
    }
    close
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_palettes_never_partially_replace_saved_preferences() {
        let mut prefs = CommentPrefs::default();
        let original = prefs.highlight_palette().to_vec();
        for json in [
            serde_json::json!([]),
            serde_json::json!([{"color":"#BADHEX","opacity":0.5}]),
            serde_json::json!([{"color":[1,0,0],"opacity":0.0}]),
            serde_json::json!([{"color":[2,0,0],"opacity":0.5}]),
            serde_json::json!(vec![serde_json::json!({"color":"#123456","opacity":0.5}); 13]),
        ] {
            assert!(from_json(&json).is_err());
            assert_eq!(prefs.highlight_palette(), original);
        }
        assert!(prefs.set_highlight_palette(vec![HighlightPreset { color: [f64::NAN, 0.0, 0.0], opacity: 0.5 }]).is_err());
        assert_eq!(prefs.highlight_palette(), original);
        for icon in ["palette", "trash-2", "star", "chevron-up", "chevron-down"] {
            assert!(icons::exists(icon), "{icon}");
        }
    }
}
