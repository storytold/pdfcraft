use craft::review::CommentThread;
use craft::ui::contract::{Show as _, ShowContainer as _, Trigger};
use craft::ui::menu::{Menu, MenuItem};

use super::{kind_label, review_status};

fn filter_checkbox<T: PartialEq>(ui: &mut egui::Ui, hidden: &mut Vec<T>, value: T, label: &str) {
    let shown = !hidden.contains(&value);
    if MenuItem::new(label.to_owned()).check(shown).keep_open(true).show(ui).ran {
        if shown {
            hidden.push(value);
        } else {
            hidden.retain(|entry| entry != &value);
        }
    }
}

pub(super) fn show(
    ui: &mut egui::Ui,
    opener: &egui::Response,
    doc: &pdfcraft_engine::Document,
    threads: &[CommentThread],
    filter: &mut craft::review::CommentFilter,
) -> Option<Vec<String>> {
    use std::collections::BTreeSet;

    let mut hidden_types = doc.hidden_comment_types().to_vec();
    Menu::new(tl!("Filter comments")).opened_by(opener, Trigger::Click).show(ui, |menu, _| {
        let _ = Menu::submenu_row(menu, tl!("Type"), |ui, _| {
            let types: BTreeSet<_> = doc
                .info
                .annotations
                .iter()
                .filter(|a| a.is_comment())
                .map(kind_label)
                .chain(doc.hidden_comment_types().iter().map(String::as_str))
                .collect();
            for label in types {
                filter_checkbox(ui, &mut hidden_types, label.to_owned(), tl!(label));
            }
        });
        let _ = Menu::submenu_row(menu, tl!("Author"), |ui, _| {
            let authors: BTreeSet<_> = threads.iter().filter_map(|t| t.messages.first()).map(|m| m.author.as_str()).collect();
            for author in authors {
                let label = if author.is_empty() { tl!("Unknown author") } else { author };
                filter_checkbox(ui, &mut filter.hidden_authors, author.to_owned(), label);
            }
        });
        let _ = Menu::submenu_row(menu, tl!("Status"), |ui, _| {
            for label in ["None", "Accepted", "Rejected", "Cancelled", "Completed"] {
                if let Some(status) = review_status(label) {
                    filter_checkbox(ui, &mut filter.hidden_statuses, status, tl!(label));
                }
            }
        });
        let _ = Menu::submenu_row(menu, tl!("Colour"), |ui, _| {
            let colors: BTreeSet<_> = threads.iter().map(|thread| thread.color).collect();
            for color in colors {
                let label = color.map_or_else(|| tl!("No colour").to_owned(), |[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}"));
                filter_checkbox(ui, &mut filter.hidden_colors, color, &label);
            }
        });
        let _ = Menu::submenu_row(menu, tl!("Checkmark"), |ui, _| {
            filter_checkbox(ui, &mut filter.hidden_checks, true, tl!("Checked"));
            filter_checkbox(ui, &mut filter.hidden_checks, false, tl!("Unchecked"));
        });
        if (filter.is_active() || !hidden_types.is_empty()) && MenuItem::new(tl!("Show all").to_string()).show(menu).ran {
            filter.clear();
            hidden_types.clear();
        }
    });
    (hidden_types != doc.hidden_comment_types()).then_some(hidden_types)
}
