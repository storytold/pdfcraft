//! Copy with formatting selects and copies text only the page's recovered layer reads (glyph
//! ids in the standard Macintosh glyph order, glyph names that number Latin-1 characters),
//! while Find, plain Copy and the strict text layer stay as they were.

use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use pdfcraft_testkit::truetype::{Cmap, mac_gid, subset};
use pdfcraft_ui_egui::PdfCraftApp;

/// One 600×300 page: `before` in Helvetica, then `text` in a Type 0 font over a TrueType subset
/// that kept its original glyph ids (no `/ToUnicode`, `cmap` or glyph names), then `after` in
/// Helvetica on the next line.
fn mac_order_pdf(before: &str, text: &str, after: &str) -> Vec<u8> {
    let gids: Vec<u16> = text.chars().map(mac_gid).collect();
    let shown: String = gids.iter().map(|g| format!("{g:04X}")).collect();
    let mut content = format!("BT /F1 20 Tf 120 250 Td <{shown}> Tj ET");
    if !before.is_empty() {
        content = format!("BT /F2 20 Tf 20 250 Td ({before}) Tj ET {content}");
    }
    if !after.is_empty() {
        content = format!("{content} BT /F2 20 Tf 20 220 Td ({after}) Tj ET");
    }
    pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 9 0 R >> >> >>")
        .stream("", content)
        .obj("<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+ArialMT /Encoding /Identity-H /DescendantFonts [6 0 R] >>")
        .obj("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /ABCDEF+ArialMT /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /CIDToGIDMap /Identity /DW 600 /FontDescriptor 7 0 R >>")
        .obj("<< /Type /FontDescriptor /FontName /ABCDEF+ArialMT /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 8 0 R >>")
        .stream("", subset(300, &gids, Cmap::None, None))
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
        .build()
}

/// What the stand-in for the system clipboard was given: `(html, plain text)` per copy.
type HtmlSets = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// The document open with its page's text layers loaded, and the pointer over glyph 2 of the
/// selection layer; `html` puts a stand-in clipboard that takes Copy with formatting.
fn harness(pdf: Vec<u8>, html: bool) -> (Harness<'static, PdfCraftApp>, Pos2, HtmlSets) {
    harness_taking(pdf, html, true)
}

/// [`harness`] whose stand-in clipboard takes HTML only when `takes`.
fn harness_taking(pdf: Vec<u8>, html: bool, takes: bool) -> (Harness<'static, PdfCraftApp>, Pos2, HtmlSets) {
    let sets = HtmlSets::default();
    let seen = sets.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("recovered.pdf", None, pdf).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.system_clipboard = html;
        if html {
            app.html_clipboard_override = Some(Box::new(move |html, text| {
                seen.lock().unwrap().push((html.to_owned(), text.to_owned()));
                takes
            }));
        }
        app
    });
    crate::support::wait_until(&mut h, "the page's text layer", |app| app.views[0].layer_glyph_screen_pos(0, 2).is_some() && !app.render_pending());
    let at = h.state().views[0].layer_glyph_screen_pos(0, 2).expect("text layer loaded");
    h.hover_at(at);
    h.run_steps(1);
    (h, at, sets)
}

/// `n` quick clicks at `pos`: four select the page's text.
fn clicks(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2, n: usize) {
    for _ in 0..n {
        for pressed in [true, false] {
            h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        }
        h.run_steps(1);
    }
}

fn copied_text(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// A page whose only text is recovered can be selected (the pointer finds its glyphs), and
/// Copy with formatting copies it, HTML and plain alternative; the strict layer, which Find
/// and plain Copy read, still has nothing.
#[test]
fn a_page_of_recovered_text_selects_and_copies_with_formatting() {
    let text = "Articles of Amendment";
    let (mut h, at, sets) = harness(mac_order_pdf("", text, ""), true);
    assert!(h.state().views[0].has_page_text(0));
    clicks(&mut h, at, 4);
    let view = &h.state().views[0];
    assert_eq!(view.selected_text(), None, "no strict text is selected");
    assert!(view.selection_copies(true));
    assert!(!h.state_mut().execute("edit.copy"), "Copy has no strict text to copy");
    assert!(h.state_mut().execute("edit.copy-with-formatting"));
    h.step();
    let sets = sets.lock().unwrap().clone();
    assert_eq!(sets.len(), 1, "{sets:?}");
    assert!(sets[0].0.contains(text), "{:?}", sets[0].0);
    assert_eq!(sets[0].1, text);
}

/// Copy is strict, with or without a clipboard for HTML: a selection of recovered text only
/// has nothing for it. Copy with formatting copies the recovered text: as its own plain text
/// without a clipboard for HTML (the web build, the system clipboard off), and so where the
/// clipboard refuses the HTML.
#[test]
fn copy_skips_recovered_text_and_copy_with_formatting_falls_back_to_it() {
    let text = "Articles of Amendment";
    let (mut h, at, _) = harness(mac_order_pdf("", text, ""), false);
    clicks(&mut h, at, 4);
    assert!(!h.state().views[0].selection_copies(false));
    h.state_mut().execute("edit.copy");
    h.step();
    assert!(copied_text(&h).is_empty(), "{:?}", copied_text(&h));
    assert!(h.state_mut().execute("edit.copy-with-formatting"));
    h.step();
    assert_eq!(copied_text(&h), [text], "no clipboard for HTML: its own plain text");

    let (mut h, at, sets) = harness_taking(mac_order_pdf("", text, ""), true, false);
    clicks(&mut h, at, 4);
    assert!(h.state_mut().execute("edit.copy-with-formatting"));
    h.step();
    assert_eq!(sets.lock().unwrap().len(), 1, "the HTML was offered first");
    assert_eq!(copied_text(&h), [text]);
}

/// Copy never carries recovered glyphs, in its HTML or its plain text; Copy with formatting
/// carries them in both.
#[test]
fn only_copy_with_formatting_carries_recovered_glyphs() {
    let (mut h, at, sets) = harness(mac_order_pdf("Form 4", "Clauses modificatrices", "end"), true);
    clicks(&mut h, at, 4);
    assert!(h.state_mut().execute("edit.copy"));
    h.step();
    let copy = sets.lock().unwrap().clone();
    assert_eq!(copy.len(), 1, "{copy:?}");
    assert!(!copy[0].0.contains("Clauses") && copy[0].0.contains("Form 4"), "{}", copy[0].0);
    assert_eq!(
        copy[0].1,
        "Form 4
end"
    );
    assert!(h.state_mut().execute("edit.copy-with-formatting"));
    h.step();
    let formatted = sets.lock().unwrap().clone();
    let (html, plain) = formatted.last().cloned().unwrap_or_default();
    assert!(html.contains("Clauses modificatrices") && plain.contains("Clauses modificatrices"), "{html} / {plain:?}");
}

/// The page's context menu on selected text offers Copy and, right after it, Copy with
/// formatting (Acrobat's Select tool: right-click the selected text, then Copy or Copy With
/// Formatting).
#[test]
fn the_context_menu_on_selected_text_offers_both_copies() {
    use egui_kittest::kittest::Queryable;
    let (mut h, at, _) = harness(mac_order_pdf("Form 4", "Clauses modificatrices", "end"), true);
    clicks(&mut h, at, 4);
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(2);
    let formatted = h.get_by_label("Copy with formatting").rect();
    // The menu's own Copy (the selection's floating bar has one too) sits just above it.
    let copies: Vec<egui::Rect> = h.query_all_by_label_contains("Copy").map(|n| n.rect()).filter(|r| *r != formatted).collect();
    let above = copies.iter().any(|c| (c.left() - formatted.left()).abs() < 2.0 && (formatted.top() - c.bottom()).abs() < 8.0);
    assert!(above, "right after Copy: {copies:?} {formatted:?}");
}

/// `select_text` and `glyph_screen_pos` take indices into the strict layer, as `has_page_text`,
/// Find and `text_extract` count glyphs, also on a page whose selection works on its recovered
/// layer: strict glyph `k` is the `e` of `end` below the recovered words, not the recovered
/// glyph at the same index of the selection layer.
#[test]
fn strict_indices_aim_at_strict_glyphs_on_a_page_with_a_recovered_layer() {
    let (mut h, _, _) = harness(mac_order_pdf("Form 4", "Clauses modificatrices", "end"), false);
    let k = (0..20)
        .find(|&k| {
            h.state_mut().views[0].select_text(0, k, k + 2);
            h.state().views[0].selected_text().as_deref() == Some("end")
        })
        .expect("strict glyphs k..=k+2 read `end`");
    let view = &h.state().views[0];
    let (first, e) = (view.glyph_screen_pos(0, 0).unwrap(), view.glyph_screen_pos(0, k).unwrap());
    assert!(e.y > first.y + 10.0, "strict glyph {k} is on the line below `Form 4`: {first:?} {e:?}");
}

/// Beside strict text, a selection over the recovered layer copies the strict text of its
/// strict glyphs with plain Copy, exactly as before, and everything with Copy with formatting.
#[test]
fn a_mixed_selection_copies_strict_text_plainly_and_all_with_formatting() {
    let pdf = mac_order_pdf("Form 4", "Clauses modificatrices", "end");
    let (mut h, at, _) = harness(pdf.clone(), false);
    clicks(&mut h, at, 4);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Form 4\nend"));
    assert!(h.state_mut().execute("edit.copy"));
    h.step();
    assert_eq!(copied_text(&h), ["Form 4\nend"]);
    // A selection from `Form 4` into the recovered words: plain Copy takes the strict glyphs it
    // holds (indices into the recovered layer, mapped to the strict one), not the strict
    // layer's glyphs at the same indices.
    h.state_mut().views[0].select_layer_text(0, 0, 7);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Form 4"));

    let (mut h, at, sets) = harness(pdf, true);
    clicks(&mut h, at, 4);
    assert!(h.state_mut().execute("edit.copy-with-formatting"));
    h.step();
    let sets = sets.lock().unwrap().clone();
    let (html, plain) = sets.first().cloned().unwrap_or_default();
    assert!(html.contains("Clauses modificatrices") && html.contains("Form 4") && html.contains("end"), "{html}");
    assert!(plain.contains("Form 4") && plain.contains("Clauses modificatrices") && plain.ends_with("end"), "{plain:?}");
}

/// Redaction made from a text selection covers what the selection holds on the page's
/// recovered layer, recovered glyphs included, and applying it removes those glyphs from the
/// page: neither layer reads them afterwards.
#[test]
fn a_redaction_from_a_selection_of_recovered_text_removes_it() {
    let text = "Articles of Amendment";
    let (mut h, at, _) = harness(mac_order_pdf("", text, ""), false);
    clicks(&mut h, at, 4);
    assert!(h.state_mut().execute("redact.mark"));
    h.step();
    let id = h.state().views[0].id;
    assert_eq!(h.state().session.get(id).unwrap().redaction_marks(), 1, "the selection was marked");
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::ApplyRedactions { pages: None }));
    let bytes = h.state().session.get(id).unwrap().bytes.clone();
    let mut r = pdfcraft_render::PageRenderer::new(bytes, Default::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::RecoveredText, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let left = out.recovered.map(|l| l.text.plain_text()).unwrap_or_default();
    assert!(left.trim().is_empty(), "the redacted glyphs are gone: {left:?}");
}

/// Review finding: the selection's floating bar offers Copy, which is Copy (strict text only):
/// on a selection of recovered text alone it is unavailable.
#[test]
fn the_selection_bar_copy_is_unavailable_without_strict_text() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let (mut h, at, _) = harness(mac_order_pdf("", "Articles of Amendment", ""), true);
    clicks(&mut h, at, 4);
    h.run_steps(3);
    assert!(h.get_by_label("Copy").accesskit_node().is_disabled(), "nothing strict to copy");
    let (mut h, at, _) = harness(mac_order_pdf("Form 4", "Clauses modificatrices", "end"), true);
    clicks(&mut h, at, 4);
    h.run_steps(3);
    assert!(!h.get_by_label("Copy").accesskit_node().is_disabled(), "strict text to copy");
}
