//! The expanded All tools view: every tool, filed under Acrobat-style categories, with a search
//! field, filling the workspace instead of the sidebar. The sidebar's expand button opens it;
//! its collapse button, Esc or choosing a tool closes it again.

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::catalog::{self, Availability, TOOL_CATEGORIES, ToolGroup};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, widgets};

const TILE: egui::Vec2 = vec2(148.0, 118.0);
/// Item names a tile lists when the search found the tool by them.
const MATCHES_SHOWN: usize = 2;

/// Whether `query` (already trimmed and lower-cased) is in `label` or its translation.
/// Translations and queries stay in logical order. For right-to-left labels, also accept
/// multiword queries in a different word order, as in the command palette.
fn label_hit(label: &str, translated: &str, query: &str, rtl: bool) -> bool {
    let translated = translated.to_lowercase();
    label.to_lowercase().contains(query)
        || translated.contains(query)
        || (rtl && query.contains(char::is_whitespace) && query.split_whitespace().all(|word| translated.contains(word)))
}

/// Whether `group` matches `query` (already trimmed and lower-cased), in English or the UI
/// language. `None` is no match; otherwise the items that matched, empty when the tool's own
/// name did. An entry with a menu (Watermark: Add, Update, Remove) also matches by its commands'
/// names, so "remove watermark" finds it.
fn matches(query: &str, group: &ToolGroup) -> Option<Vec<&'static str>> {
    let rtl = crate::i18n::current().rtl();
    let hit = |label: &str| label_hit(label, tl!(label), query, rtl);
    if query.is_empty() || hit(group.label) {
        return Some(Vec::new());
    }
    let action_hit = |id: &&str| {
        pdfcraft_engine::commands::command(id).is_some_and(|spec| label_hit(spec.label, &crate::i18n::menu_label(spec.id, spec.label), query, rtl))
    };
    let items: Vec<_> =
        group.sections.iter().flat_map(|s| s.items.iter()).filter(|i| hit(i.label) || i.actions.iter().any(action_hit)).map(|i| i.label).collect();
    (!items.is_empty()).then_some(items)
}

/// Draw the view over the workspace: the title with Collapse, the search field, and each
/// category's tiles that match the search. Choosing a tile opens that tool's panel.
pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // A dialog, prompt or the command palette over the view owns Escape.
    if !app.modal_open() && !app.palette_open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.all_tools_full = false;
        return;
    }
    let mut chosen = None;
    egui::Frame::NONE.fill(t.panel).show(ui, |ui| {
        ui.set_min_size(ui.available_size());
        egui::Frame::NONE.inner_margin(egui::Margin { left: 36, right: 28, top: 20, bottom: 12 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("All tools")).font(theme::semibold(22.0)).color(t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::button(ui, "minimize-2", 28.0, false, tl!("Collapse all tools")).clicked() {
                        app.all_tools_full = false;
                    }
                });
            });
            ui.add_space(10.0);
            let field = ui.add(
                widgets::line(&mut app.all_tools_query).hint_text(tl!("Search tools and commands…")).desired_width(ui.available_width().min(520.0)),
            );
            // Typing goes straight to the search, as in Acrobat's tools view.
            if !ui.memory(|m| m.focused().is_some()) {
                field.request_focus();
            }
        });
        ui.separator();
        let query = app.all_tools_query.trim().to_lowercase();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 36, right: 28, top: 8, bottom: 28 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut any = false;
                for category in TOOL_CATEGORIES {
                    let found: Vec<_> =
                        category.groups.iter().filter_map(|id| catalog::group(id)).filter_map(|g| matches(&query, g).map(|m| (g, m))).collect();
                    if found.is_empty() {
                        continue;
                    }
                    any = true;
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new(tl!(category.label)).font(theme::semibold(16.0)).color(t.text));
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(14.0, 14.0);
                        for (g, items) in found {
                            if tile(ui, &t, g, &items).clicked() {
                                chosen = Some(g.id);
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
                if !any {
                    ui.add_space(24.0);
                    ui.label(egui::RichText::new(tl!("No tools match your search.")).color(t.text_muted));
                }
            });
        });
    });
    if let Some(id) = chosen {
        app.left_open = true;
        app.select_tool(id);
    }
}

/// One tool's tile: its icon and name, then the `items` the search found it by or, when there are
/// none, its availability (a planned tool is greyed out with its milestone). A name or note cut
/// short shows in full on hover.
fn tile(ui: &mut egui::Ui, t: &Tokens, g: &ToolGroup, items: &[&'static str]) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(TILE, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!(g.label)));
    let press = theme::Press::track(ui, &resp);
    crate::home::card_lift(ui, t, rect, press);
    ui.painter().rect(rect, CornerRadius::same(10), press.fill(t.card, t.hover, t.pressed), Stroke::new(1.0, t.divider), egui::StrokeKind::Inside);
    let body = rect.translate(press.offset());
    let ready = g.availability == Availability::Ready;
    let hue = Color32::from_rgb(g.hue[0], g.hue[1], g.hue[2]);
    icons::paint(ui, Rect::from_center_size(body.center_top() + vec2(0.0, 34.0), vec2(32.0, 32.0)), g.icon, 30.0, hue);
    let mut job =
        egui::text::LayoutJob::simple(tl!(g.label).to_owned(), theme::medium(13.0), if ready { t.text } else { t.text_muted }, body.width() - 16.0);
    job.halign = egui::Align::Center;
    job.wrap.max_rows = 2;
    let title = ui.fonts_mut(|f| f.layout_job(job));
    let mut elided = title.elided;
    let title_h = title.size().y;
    ui.painter().galley(body.center_top() + vec2(0.0, 60.0), title, t.text);
    // What found the tool, or else whether it works today.
    let note = if items.is_empty() {
        match g.availability {
            Availability::Ready => String::new(),
            Availability::Planned(m) => format!("{} · {m}", tl!("Planned")),
            Availability::Provider => g.badge.map(|b| tl!(b).to_owned()).unwrap_or_default(),
        }
    } else {
        items.iter().take(MATCHES_SHOWN).map(|l| tl!(l)).collect::<Vec<_>>().join(" · ")
    };
    if !note.is_empty() {
        let color = if items.is_empty() { t.text_faint } else { t.accent_text };
        let mut job = egui::text::LayoutJob::simple_singleline(note.clone(), theme::regular(11.0), color);
        job.halign = egui::Align::Center;
        job.wrap = egui::text::TextWrapping { max_width: body.width() - 16.0, max_rows: 1, break_anywhere: true, ..Default::default() };
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        elided |= galley.elided;
        ui.painter().galley(body.center_top() + vec2(0.0, 64.0 + title_h), galley, color);
    }
    if let Some(b) = g.badge.filter(|_| g.availability == Availability::Ready) {
        let font = theme::semibold(9.5);
        let w = widgets::text_width(ui, tl!(b), &font);
        let r = Rect::from_min_size(body.right_top() + vec2(-w - 18.0, 8.0), vec2(w + 10.0, 16.0));
        ui.painter().rect_filled(r, CornerRadius::same(4), t.badge_new);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, tl!(b), font, Color32::WHITE);
    }
    let resp = if elided { resp.on_hover_text(format!("{}\n{note}", tl!(g.label))) } else { resp };
    theme::hand(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str) -> &'static ToolGroup {
        catalog::group(id).expect("catalogue group")
    }

    #[test]
    fn search_matches_tool_names_and_their_items() {
        assert_eq!(matches("", group("organize")), Some(Vec::new()), "an empty query shows everything");
        assert_eq!(matches("organize", group("organize")), Some(Vec::new()), "the tool's own name");
        let rotate = matches("rotate", group("organize")).expect("Organize pages lists Rotate");
        assert!(!rotate.is_empty() && rotate.iter().all(|l| l.to_lowercase().contains("rotate")), "{rotate:?}");
        assert_eq!(matches("no such tool anywhere", group("organize")), None);
    }

    #[test]
    fn search_finds_an_entry_by_its_menus_commands() {
        let found = matches("remove watermark", group("edit")).expect("Watermark's menu has Remove watermark");
        assert!(found.contains(&"Watermark"), "{found:?}");
    }

    #[test]
    fn right_to_left_labels_match_a_query_typed_in_logical_order() {
        let arabic = crate::i18n::Lang::from_code("ar").expect("Arabic is registered");
        let translated = crate::i18n::tr(arabic, "Organize pages");
        assert_eq!(translated, "تنظيم الصفحات", "the catalogue keeps logical text");
        assert!(label_hit("Organize pages", translated, "تنظيم الصفحات", true));
        assert!(label_hit("Organize pages", translated, "الصفحات", true), "one word");
        assert!(label_hit("Organize pages", translated, "الصفحات تنظيم", true), "RTL word-order fallback");
        assert!(!label_hit("Organize pages", translated, "تنظيم الملفات", true), "every word must be there");
        assert!(!label_hit("Organize pages", translated, "الصفحات تنظيم", false), "left-to-right labels stay exact");
    }
}
