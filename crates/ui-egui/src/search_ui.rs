//! Edit ▸ Advanced Search (⇧⌘F): the search in a side panel that lists every match with its
//! context, page by page; click a result to go to it. It shares the find bar's search.

use egui::text::LayoutJob;
use egui::{Color32, FontId, TextFormat};

use crate::canvas::DocView;
use crate::theme::{self, Tokens};

/// Glyphs of context shown on each side of a match.
const CONTEXT: usize = 36;

pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, view: &mut DocView, pages: usize) {
    let searched = view.texts.len() + view.text_failed.len();
    let Some(find) = view.find.as_mut() else { return };
    find.in_panel = true;
    let prompt = ui.label(egui::RichText::new("What word or phrase would you like to search for?").color(t.text_muted));
    let query_edit = ui
        .add(egui::TextEdit::singleline(&mut find.query).id(egui::Id::new("search-panel-input")).hint_text("Search").desired_width(f32::INFINITY))
        .labelled_by(prompt.id);
    if find.focus {
        query_edit.request_focus();
        find.focus = false;
    }
    ui.horizontal(|ui| {
        // Outlined boxes (the panel and the check-box fill are alike), as in dialogs.
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.bg_stroke = egui::Stroke::new(1.0, t.border);
        widgets.inactive.bg_fill = t.hover;
        let whole_words = ui.checkbox(&mut find.whole_words, "Whole words only").changed();
        let case_sensitive = ui.checkbox(&mut find.case_sensitive, "Case-sensitive").changed();
        if whole_words || case_sensitive {
            find.case_query.clear();
        }
    });
    if find.query != find.case_query {
        view.rerun_find();
    }
    let Some(find) = view.find.as_ref() else { return };
    ui.add_space(6.0);
    let status = if find.query.trim().is_empty() {
        String::new()
    } else if find.matches.is_empty() && searched < pages {
        format!("Searching… {searched} of {pages} pages")
    } else {
        let n = find.matches.len();
        let more = if searched < pages { format!(" (searching… {searched} of {pages} pages)") } else { String::new() };
        format!("{n} instance{}{more}", if n == 1 { "" } else { "s" })
    };
    ui.label(egui::RichText::new(status).font(theme::semibold(12.5)));
    ui.add_space(4.0);
    let mut go = None;
    let mut last_page = None;
    for (i, (p, range)) in find.matches.iter().enumerate().take(2000) {
        if last_page != Some(*p) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("Page {}", p + 1)).small().color(t.text_faint));
            last_page = Some(*p);
        }
        let Some(text) = view.texts.get(p) else { continue };
        let n = text.glyphs.len();
        let before = text.text_of(range.start.saturating_sub(CONTEXT)..range.start.min(n));
        let hit = text.text_of(range.start.min(n)..range.end.min(n));
        let after = text.text_of(range.end.min(n)..(range.end + CONTEXT).min(n));
        let flat = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut job = LayoutJob::default();
        let font = FontId::new(12.5, egui::FontFamily::Proportional);
        let plain = TextFormat { font_id: font.clone(), color: t.text_muted, ..Default::default() };
        job.append(&format!("…{} ", flat(before)), 0.0, plain.clone());
        job.append(
            &flat(hit),
            0.0,
            TextFormat {
                font_id: theme::semibold(12.5),
                color: t.text,
                background: Color32::from_rgba_unmultiplied(255, 214, 0, 90),
                ..Default::default()
            },
        );
        job.append(&format!(" {}…", flat(after)), 0.0, plain);
        job.wrap.max_width = ui.available_width();
        let selected = find.current == Some(i);
        let resp = ui.add(egui::Button::selectable(selected, job).wrap()).on_hover_text("Go to this result");
        if resp.clicked() {
            go = Some(i);
        }
    }
    if let Some(i) = go {
        view.go_to_match(i);
    }
}
