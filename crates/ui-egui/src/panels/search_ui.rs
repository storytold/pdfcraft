//! Edit ▸ Advanced Search (⇧⌘F): the search in a side panel that lists every match with its
//! context, page by page; click a result to go to it. It shares the find bar's search.

use egui::text::LayoutJob;
use egui::{Color32, FontId, TextFormat};

use crate::canvas::DocView;
use crate::theme::{self, Tokens};
use crate::widgets;

/// Glyphs of context shown on each side of a match.
const CONTEXT: usize = 36;

/// Draw the Search panel with its query, options, progress and clickable result excerpts.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, view: &mut DocView, pages: usize) {
    crate::widgets::begin_dialog(ui);
    let searched = view.texts.len() + view.text_failed.len();
    // Pages whose text couldn't be read count as searched, but a match on them can't be found.
    let unread = view.text_failed.len();
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
    ui.horizontal(|ui| {
        // Outlined boxes (the panel and the check-box fill are alike), as in dialogs.
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, t.border);
        w.inactive.bg_fill = t.hover;
        let a = ui.add(widgets::checkbox(&mut find.whole_words, tl!("Whole words only"))).changed();
        let b = ui.add(widgets::checkbox(&mut find.case_sensitive, tl!("Case-sensitive"))).changed();
        if a || b {
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
    // The pages with a match, once every page has been searched (#4334): selected, so Extract,
    // Print or Delete act on them; or copied as a page list ("1-3, 7") for a page field.
    let mut select = None;
    if !find.matches.is_empty() && searched >= pages {
        let found: std::collections::BTreeSet<usize> = find.matches.iter().map(|(p, _)| *p).collect();
        // Said beside the actions, so a selection or list that leaves those pages out isn't
        // taken for every page with a match.
        if unread > 0 {
            let text = crate::i18n::fmt(tl!("Pages whose text couldn't be read: {n}. A match on them isn't listed."), &[("n", &unread.to_string())]);
            ui.label(egui::RichText::new(text).color(t.text_muted));
        }
        ui.horizontal(|ui| {
            let n = found.len().to_string();
            let tip = crate::i18n::fmt(tl!("Select the {n} pages with a match, for Extract, Print or Delete"), &[("n", &n)]);
            if ui.button(tl!("Select pages")).on_hover_text(tip).clicked() {
                select = Some(found.iter().copied().collect::<Vec<_>>());
            }
            if ui.button(tl!("Copy page numbers")).on_hover_text(tl!("Copy the pages with a match as a list such as 1-3, 7")).clicked() {
                ui.ctx().copy_text(crate::page_selection_ui::formatted(&found));
            }
        });
    }
    ui.add_space(4.0);
    let mut go = None;
    let mut last_page = None;
    for (i, (p, range)) in find.matches.iter().enumerate().take(2000) {
        if last_page != Some(*p) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Page {label}"), &[("label", &(p + 1).to_string())])).small().color(t.text_faint));
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
        let resp = ui.add(egui::Button::selectable(selected, job).wrap()).on_hover_text(tl!("Go to this result"));
        if resp.clicked() {
            go = Some(i);
        }
    }
    if let Some(i) = go {
        view.go_to_match(i);
    }
    if let Some(pages) = select {
        view.select_pages(&pages);
    }
}

#[cfg(test)]
mod tests {
    use crate::PdfCraftApp;
    use egui_kittest::{Harness, kittest::Queryable};

    /// CodeRabbit on #4578: a page whose text couldn't be read counts as searched, so Select pages
    /// and Copy page numbers show; the panel says such pages are left out of the results.
    #[test]
    fn unread_pages_are_said_beside_the_page_actions() {
        let bytes = pdfcraft_engine::Session::new().create_from_text("Pages", "Hello one\u{c}Hello two\u{c}Hello three").unwrap();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(|_| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("pages.pdf", None, bytes.as_ref().clone()).unwrap();
            app
        });
        h.run_steps(2);
        assert!(h.state_mut().execute("edit.advanced_search"));
        h.run_steps(2);
        if let Some(f) = h.state_mut().views[0].find.as_mut() {
            f.query = "Hello".into();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while h.query_by_label("Select pages").is_none() {
            assert!(std::time::Instant::now() < deadline, "the search finishes");
            h.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.query_by_label_contains("couldn't be read").is_none(), "every page was read");
        // The third page's text can't be read.
        let view = &mut h.state_mut().views[0];
        view.texts.remove(&2);
        view.text_failed.insert(2);
        h.run_steps(2);
        assert!(h.query_by_label("Select pages").is_some(), "the actions stay");
        h.get_by_label_contains("Pages whose text couldn't be read: 1.");
    }
}
