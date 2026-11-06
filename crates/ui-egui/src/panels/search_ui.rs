//! Edit ▸ Advanced Search (⇧⌘F): the search in a side panel that lists every match with its
//! context, page by page; click a result to go to it. It shares the find bar's search.

use egui::text::LayoutJob;
use egui::{Color32, FontId, TextFormat};

use crate::canvas::{DocView, FindHit};
use crate::theme::{self, Tokens};
use crate::widgets;

/// Glyphs of context shown on each side of a match.
const CONTEXT: usize = 36;

/// Draw the Search panel with its query, options, progress and clickable result excerpts.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, view: &mut DocView, pages: usize, annotations: &[pdfcraft_render::Annotation]) {
    crate::widgets::begin_dialog(ui);
    let searched = view.texts.len() + view.text_failed.len();
    let Some(find) = view.find.as_mut() else { return };
    find.in_panel = true;
    let l = ui.label(egui::RichText::new(tl!("What word or phrase would you like to search for?")).color(t.text_muted));
    let r = ui
        .add(crate::widgets::line(&mut find.query).id(egui::Id::unique("search-panel-input")).hint_text(tl!("Search")).desired_width(f32::INFINITY))
        .accessible_name(tl!("Search"))
        .labelled_by(l.id);
    if find.focus {
        r.request_focus();
        find.focus = false;
    }
    ui.vertical(|ui| {
        // Outlined boxes (the panel and the check-box fill are alike), as in dialogs.
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, t.border);
        w.inactive.bg_fill = t.hover;
        let (a, b) = ui
            .horizontal(|ui| {
                let a = ui.add(widgets::checkbox(&mut find.whole_words, tl!("Whole words only"))).changed();
                let b = ui.add(widgets::checkbox(&mut find.case_sensitive, tl!("Case-sensitive"))).changed();
                (a, b)
            })
            .inner;
        let c = ui.add(widgets::checkbox(&mut find.include_comments, tl!("Include comments"))).changed();
        if a || b || c {
            find.case_query.clear();
        }
    });
    if find.query != find.case_query {
        view.rerun_find_in(annotations);
    }
    let Some(find) = view.find.as_ref() else { return };
    ui.add_space(6.0);
    let status = if find.query.trim().is_empty() {
        String::new()
    } else if find.matches.is_empty() && searched < pages {
        crate::i18n::fmt(tl!("Searching… {s} of {p} pages"), &[("s", &searched.to_string()), ("p", &pages.to_string())])
    } else {
        let n = find.matches.len();
        let more = if searched < pages {
            crate::i18n::fmt(tl!(" (searching… {s} of {p} pages)"), &[("s", &searched.to_string()), ("p", &pages.to_string())])
        } else {
            String::new()
        };
        // English keeps its singular; Chinese has no plural.
        let count = if n == 1 { crate::i18n::fmt(tl!("1 instance"), &[]) } else { crate::i18n::fmt(tl!("{n} instances"), &[("n", &n.to_string())]) };
        format!("{count}{more}")
    };
    ui.label(egui::RichText::new(status).font(theme::semibold(12.5)));
    ui.add_space(4.0);
    let mut go = None;
    let mut last_page = None;
    for (i, hit) in find.matches.iter().enumerate().take(2000) {
        let page = hit.page();
        if last_page != Some(page) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Page {label}"), &[("label", &(page + 1).to_string())])).small().color(t.text_faint));
            last_page = Some(page);
        }
        let Some(mut job) = result_job(view, hit, t) else { continue };
        job.wrap.max_width = ui.available_width();
        let selected = find.current == Some(i);
        let resp = ui.add(egui::Button::selectable(selected, job).wrap()).on_hover_text(tl!("Go to this result"));
        if resp.clicked() {
            go = Some(i);
        }
    }
    if let Some(i) = go {
        view.go_to_match(i);
    }
}

/// The result row: page text with context, or a comment's kind and the field that matched.
fn result_job(view: &DocView, hit: &FindHit, t: &Tokens) -> Option<LayoutJob> {
    let font = FontId::new(12.5, egui::FontFamily::Proportional);
    let plain = TextFormat { font_id: font.clone(), color: t.text_muted, ..Default::default() };
    let marked = TextFormat {
        font_id: theme::semibold(12.5),
        color: t.text,
        background: Color32::from_rgba_unmultiplied(255, 214, 0, 90),
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    match hit {
        FindHit::Text { page, range } => {
            let text = view.texts.get(page)?;
            let n = text.glyphs.len();
            let before = text.text_of(range.start.saturating_sub(CONTEXT)..range.start.min(n));
            let found = text.text_of(range.start.min(n)..range.end.min(n));
            let after = text.text_of(range.end.min(n)..(range.end.saturating_add(CONTEXT)).min(n));
            let flat = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");
            job.append(&format!("…{} ", flat(before)), 0.0, plain.clone());
            job.append(&flat(found), 0.0, marked);
            job.append(&format!(" {}…", flat(after)), 0.0, plain);
        }
        FindHit::Comment { kind, excerpt, .. } => {
            job.append(&format!("{}: ", crate::i18n::t(kind)), 0.0, plain);
            job.append(excerpt, 0.0, marked);
        }
    }
    Some(job)
}
