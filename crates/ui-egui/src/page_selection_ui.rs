//! The Organize Pages selection, as an editable list of physical page numbers.
use std::collections::BTreeSet;

use pdfcraft_engine::print::PrintError;

#[derive(Default)]
pub(crate) struct PageSelectionInput {
    text: String,
    selection: BTreeSet<usize>,
    error: Option<String>,
}

/// Blank clears the explicit selection. Repeated/reversed ranges become a sorted set.
/// Bound both the input and expansion work, including repeated overlapping ranges.
pub(crate) fn parse(text: &str, count: usize) -> Result<Vec<usize>, PrintError> {
    if text.len() > 16_384 {
        return Err(PrintError::NotAPage(text.chars().take(80).collect()));
    }
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut pages = BTreeSet::new();
    let mut budget = 1_000_000usize;
    let number = |token: &str| {
        let n = token.trim().parse::<usize>().map_err(|_| PrintError::NotAPage(token.into()))?;
        if n == 0 || n > count { Err(PrintError::OutOfRange(n, count)) } else { Ok(n - 1) }
    };
    for part in text.split([',', ';']).map(str::trim) {
        let (a, b) = if let Some((a, b)) = part.split_once('-') {
            (number(a)?, number(b)?)
        } else {
            let p = number(part)?;
            (p, p)
        };
        let (a, b) = (a.min(b), a.max(b));
        let size = b.saturating_sub(a).saturating_add(1);
        budget = budget.checked_sub(size).ok_or_else(|| PrintError::NotAPage(part.into()))?;
        pages.extend(a..=b);
    }
    Ok(pages.into_iter().collect())
}

fn formatted(pages: &BTreeSet<usize>) -> String {
    let mut parts = Vec::new();
    let mut it = pages.iter().copied().peekable();
    while let Some(first) = it.next() {
        let mut last = first;
        while it.peek().is_some_and(|p| last.checked_add(1) == Some(*p)) {
            if let Some(next) = it.next() {
                last = next;
            }
        }
        let first = first.saturating_add(1);
        let last = last.saturating_add(1);
        parts.push(if first == last { first.to_string() } else { format!("{first}-{last}") });
    }
    parts.join(", ")
}

impl PageSelectionInput {
    /// The field, as Acrobat's Pages selected: typed pages apply on Enter or when the field loses
    /// focus, and the chevron opens `menu` (pages by kind), whose choice is returned the same way.
    /// With nothing selected it shows the current page (0-based `current`), which actions apply to.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        selected: &BTreeSet<usize>,
        count: usize,
        current: usize,
        menu: impl FnOnce(&mut egui::Ui) -> Option<Vec<usize>>,
    ) -> Option<Vec<usize>> {
        if self.selection != *selected {
            self.selection.clone_from(selected);
            self.text = formatted(selected);
            self.error = None;
        }
        let mut commit = false;
        let id = ui.make_persistent_id("organize-page-range");
        // Egui can surrender focus before this frame's widgets run on Escape.
        let cancel = ui.is_enabled()
            && ui.memory(|m| m.has_focus(id) || m.had_focus_last_frame(id))
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if cancel {
            self.text = formatted(selected);
            self.error = None;
            ui.memory_mut(|m| m.surrender_focus(id));
        }
        let mut chosen = None;
        ui.horizontal(|ui| {
            let label = tl!("Selected pages");
            let width = (ui.available_width() - 32.0).max(40.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .id(id)
                    .desired_width(width)
                    .char_limit(16_384)
                    .hint_text(current.saturating_add(1).to_string()),
            );
            field.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), label));
            commit = ui.is_enabled() && !cancel && field.lost_focus();
            let pick = crate::icons::button(ui, "chevron-down", 26.0, false, tl!("Select pages"));
            chosen = egui::Popup::menu(&pick).show(menu).and_then(|r| r.inner);
        });
        if let Some(pages) = chosen {
            self.selection = pages.iter().copied().collect();
            self.text = formatted(&self.selection);
            self.error = None;
            return Some(pages);
        }
        if cancel {
            self.text = formatted(selected);
            self.error = None;
        } else if commit {
            match parse(&self.text, count) {
                Ok(pages) => {
                    self.selection = pages.iter().copied().collect();
                    self.text = formatted(&self.selection);
                    self.error = None;
                    return Some(pages);
                }
                Err(e) => self.error = Some(e.said(&|s| crate::i18n::t(s).to_string())),
            }
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_ranges_are_sorted_unique_and_bounded() {
        let pages = parse("1,12,16-18,89,92,93,97,99", 100).unwrap();
        assert_eq!(pages, [0, 11, 15, 16, 17, 88, 91, 92, 96, 98]);
        assert_eq!(formatted(&pages.into_iter().collect()), "1, 12, 16-18, 89, 92-93, 97, 99");
        assert_eq!(parse("3-1,2", 3).unwrap(), [0, 1, 2]);
        assert!(parse("  ", 0).unwrap().is_empty());
        for text in ["0", "101", "1,", "x", "1--3", "18446744073709551616", "1-100000000"] {
            assert!(parse(text, 100).is_err(), "{text}");
        }
        assert!(parse(&"1,".repeat(10_000), 100).is_err());
        assert!(parse("1-1000000,1-1000000", 1_000_000).is_err());
    }
}
