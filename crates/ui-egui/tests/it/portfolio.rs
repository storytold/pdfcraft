//! A PDF portfolio (`/Collection`, ISO 32000-2 §7.11.6) opens on its documents, not on the cover
//! sheet its pages are (#1652): its tab shows its file list, the Portfolio files panel, even with
//! the right panel closed, and shows the file picked there, starting with the one `/D` names,
//! in place of the cover, read-only.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

/// A one-page PDF.
const PAGE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A one-page PDF with a comment on its page.
const NOTED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Text /Rect [20 160 40 180] /Contents (Sign here) /T (HR) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// [`PAGE`] encrypted, opening with the password `pw`.
fn locked() -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(PAGE.to_vec())).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "pw",
        owner_password: "owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

/// A one-page portfolio of `files` (name, bytes), with `catalog` (its `/Collection` and any other
/// entries) in its catalog.
fn portfolio(catalog: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let names: String = files.iter().enumerate().map(|(i, (name, _))| format!("({name}) {} 0 R ", 5 + 2 * i)).collect();
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} /Names << /EmbeddedFiles << /Names [{names}] >> >> >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> >>".to_vec(),
    ];
    for (i, (name, data)) in files.iter().enumerate() {
        objs.push([format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n", data.len()).as_bytes(), data, b"\nendstream"].concat());
        objs.push(format!("<< /Type /Filespec /F ({name}) /UF ({name}) /EF << /F {} 0 R >> >>", 4 + 2 * i).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The app with `pdf` opened as `portfolio.pdf`, the reader keeping the right panel closed or not.
fn opened(pdf: Vec<u8>, panel_closed: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.right_panel_closed = panel_closed;
        app.open_bytes("portfolio.pdf", None, pdf.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

/// The open tabs' documents' names (a portfolio's tab's is the portfolio's), and the active one's.
fn tabs(h: &Harness<'static, PdfCraftApp>) -> (Vec<String>, Option<String>) {
    let s = h.state();
    let name = |i: usize| s.tab_doc(i).and_then(|id| s.session.get(id)).map(|d| d.name.clone());
    ((0..s.views.len()).filter_map(name).collect(), s.active.and_then(name))
}

/// The name of the document tab `index` shows: a portfolio's file, or the portfolio's own pages.
fn shown(h: &Harness<'static, PdfCraftApp>, index: usize) -> Option<String> {
    let s = h.state();
    s.views.get(index).and_then(|v| s.session.get(v.id)).map(|d| d.name.clone())
}

/// The Portfolio files panel's row for `name`.
fn row<'a>(h: &'a Harness<'static, PdfCraftApp>, name: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(Role::ListBoxOption, name)
}

/// Make tab `index` the active one.
fn select(h: &mut Harness<'static, PdfCraftApp>, index: usize) {
    h.state_mut().active = Some(index);
    h.run_steps(2);
}

/// The initial document is shown in the portfolio's own tab, which lists its files whatever the
/// reader's panel preference, giving the panel back when left.
#[test]
fn a_portfolio_presents_its_initial_document_in_its_tab_and_shows_its_files_with_the_panel_closed() {
    let mut h = opened(portfolio("/Collection << /D (form.pdf) /View /T >>", &[("intro.pdf", PAGE), ("form.pdf", PAGE)]), true);
    // The document `/D` names is presented in the portfolio's tab, not a tab of its own.
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into()], Some("portfolio.pdf".into())));
    assert_eq!(shown(&h, 0).as_deref(), Some("form.pdf"));
    // The tab shows its files, without changing the reader's preference.
    assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles));
    assert!(h.state().right_panel_closed);
    // Leaving it gives back the closed panel; coming back shows the files again.
    h.state_mut().open_bytes("other.pdf", None, PAGE.to_vec()).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().right, None);
    select(&mut h, 0);
    assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles));
    // The reader can still close the list on the portfolio's tab.
    h.state_mut().choose_right_panel(None);
    h.run_steps(2);
    assert_eq!(h.state().right, None);
    // A panel the reader had open comes back once they leave the portfolio's tab.
    select(&mut h, 1);
    h.state_mut().choose_right_panel(Some(RightPanel::Bookmarks));
    select(&mut h, 0);
    assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles));
    select(&mut h, 1);
    assert_eq!(h.state().right, Some(RightPanel::Bookmarks));
}

/// The file list shows over the initial document, whatever panel the portfolio asks for.
#[test]
fn the_initial_document_shows_with_the_portfolios_file_list() {
    // The portfolio asks for the Attachments panel (`/PageMode /UseAttachments`), the reader keeps
    // the right panel open and the document it presents has comments: its tab shows its files.
    let h = opened(portfolio("/PageMode /UseAttachments /Collection << /D (form.pdf) >>", &[("intro.pdf", PAGE), ("form.pdf", NOTED)]), false);
    assert_eq!(shown(&h, 0).as_deref(), Some("form.pdf"));
    assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles));
}

/// A file opened together with a portfolio, after it, stays in front.
#[test]
fn a_file_opened_after_the_portfolio_keeps_the_front() {
    // Two files opened at once, the portfolio first: the second stays in front, and the
    // portfolio's initial document waits for the reader, whose list shows on the portfolio's tab.
    let pdf = portfolio("/Collection << /D (form.pdf) >>", &[("form.pdf", PAGE)]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.right_panel_closed = true;
        app.open_bytes("portfolio.pdf", None, pdf.clone()).unwrap();
        app.open_bytes("other.pdf", None, PAGE.to_vec()).unwrap();
        app
    });
    h.run_steps(4);
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into(), "other.pdf".into()], Some("other.pdf".into())));
    assert_eq!(h.state().right, None);
    select(&mut h, 0);
    assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles));
}

/// A password asked for a file opened together with the portfolio stays asked: the initial
/// document waits for the answer, then gives way to that file, or opens once the prompt is
/// cancelled.
#[test]
fn a_password_asked_for_a_file_opened_with_the_portfolio_stays_asked() {
    for answer in [Some("pw".to_string()), None] {
        let pdf = portfolio("/Collection << /D (form.pdf) >>", &[("form.pdf", PAGE)]);
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("portfolio.pdf", None, pdf.clone()).unwrap();
            app.open_bytes("locked.pdf", None, locked()).unwrap();
            app
        });
        h.run_steps(4);
        assert!(h.state().password_prompt.is_some(), "the prompt for locked.pdf is still up");
        assert_eq!(tabs(&h).0, ["portfolio.pdf"]);
        assert_eq!(shown(&h, 0).as_deref(), Some("portfolio.pdf"), "the initial document waits");
        let cancelled = answer.is_none();
        h.state_mut().submit_password(answer);
        if cancelled {
            // Nothing else opens: the portfolio presents its initial document in its tab.
            h.run_steps(4);
            assert_eq!(tabs(&h), (vec!["portfolio.pdf".into()], Some("portfolio.pdf".into())));
            assert_eq!(shown(&h, 0).as_deref(), Some("form.pdf"));
        } else {
            // The answered file opens in the background worker, in front.
            crate::support::wait_until(&mut h, "a second tab", |app| app.views.len() == 2);
            h.run_steps(4);
            assert_eq!(tabs(&h), (vec!["portfolio.pdf".into(), "locked.pdf".into()], Some("locked.pdf".into())));
        }
    }
}

/// An attachment deleted while the initial document waits (here for a password asked for a file
/// opened with the portfolio) doesn't change which document opens.
#[test]
fn a_deleted_attachment_doesnt_change_which_document_opens() {
    let pdf = portfolio("/Collection << /D (form.pdf) >>", &[("intro.pdf", PAGE), ("form.pdf", PAGE)]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("portfolio.pdf", None, pdf.clone()).unwrap();
        app.open_bytes("locked.pdf", None, locked()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::DeleteAttachment { key: "intro.pdf".into() }));
    h.state_mut().submit_password(None);
    h.run_steps(4);
    assert_eq!(shown(&h, 0).as_deref(), Some("form.pdf"));
}

/// Leaving the portfolio's tab in Read mode: Read mode gives back the panel the file list
/// replaced, not the list, on the tab that is active when it ends.
#[test]
fn read_mode_gives_back_the_panel_the_file_list_replaced() {
    let mut h = opened(portfolio("/Collection << /View /D >>", &[("form.pdf", PAGE)]), true);
    h.state_mut().open_bytes("other.pdf", None, PAGE.to_vec()).unwrap();
    h.run_steps(2);
    select(&mut h, 0);
    assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles));
    assert!(h.state_mut().execute("view.read_mode"));
    h.run_steps(2);
    assert_eq!(h.state().right, None);
    select(&mut h, 1);
    assert!(h.state_mut().execute("view.read_mode"));
    h.run_steps(2);
    assert_eq!(h.state().right, None, "the panel was closed before the list showed");
}

/// The initial document is a PDF by its bytes, not its name.
#[test]
fn an_initial_pdf_named_without_pdf_opens() {
    let h = opened(portfolio("/Collection << /D (Form) >>", &[("Form", PAGE)]), true);
    assert_eq!(tabs(&h).0.len(), 1);
    assert_ne!(shown(&h, 0).as_deref(), Some("portfolio.pdf"));
}

/// A panel the reader picks in Read mode on the portfolio's tab stays when they switch tabs.
#[test]
fn a_panel_picked_in_read_mode_stays_when_the_tab_changes() {
    let mut h = opened(portfolio("/Collection << /View /D >>", &[("form.pdf", PAGE)]), true);
    h.state_mut().open_bytes("other.pdf", None, PAGE.to_vec()).unwrap();
    h.run_steps(2);
    select(&mut h, 0);
    assert!(h.state_mut().execute("view.read_mode"));
    h.run_steps(2);
    h.state_mut().choose_right_panel(Some(RightPanel::Attachments));
    select(&mut h, 1);
    assert_eq!(h.state().right, Some(RightPanel::Attachments));
}

/// `/View /H` presents the initial document but leaves the file list to the reader.
#[test]
fn a_portfolio_that_keeps_its_list_hidden_still_presents_its_initial_document() {
    let h = opened(portfolio("/Collection << /D (form.pdf) /View /H >>", &[("intro.pdf", PAGE), ("form.pdf", PAGE)]), true);
    assert_eq!(shown(&h, 0).as_deref(), Some("form.pdf"));
    assert_eq!(h.state().right, None, "/View /H: the list shows when the reader asks for it");
}

/// Without a PDF to present, the portfolio stays in front with its files listed.
#[test]
fn a_portfolio_without_an_initial_document_shows_its_files_over_the_cover() {
    // Without `/D` the cover is the initial document; a `/D` naming a file that isn't a PDF
    // opens nothing either.
    for collection in ["/Collection << /View /D >>", "/Collection << /D (notes.txt) >>"] {
        let h = opened(portfolio(collection, &[("notes.txt", b"hello"), ("form.pdf", PAGE)]), true);
        assert_eq!(tabs(&h), (vec!["portfolio.pdf".into()], Some("portfolio.pdf".into())), "{collection}");
        assert_eq!(shown(&h, 0).as_deref(), Some("portfolio.pdf"), "{collection}");
        assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles), "{collection}");
        assert!(h.state().toast.is_none(), "an initial document that can't be shown stays quiet: {collection}");
    }
}

/// A portfolio nested in the initial document doesn't open a chain of tabs.
#[test]
fn a_portfolio_inside_the_initial_document_waits_for_the_reader() {
    // The initial document is itself a portfolio presenting a third file: opening the outer
    // one opens one tab, not a chain of them.
    let inner = portfolio("/Collection << /D (deep.pdf) >>", &[("deep.pdf", PAGE)]);
    let h = opened(portfolio("/Collection << /D (inner.pdf) >>", &[("inner.pdf", &inner)]), true);
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into()], Some("portfolio.pdf".into())));
    assert_eq!(shown(&h, 0).as_deref(), Some("inner.pdf"));
    // The tab still lists the outer portfolio's files.
    assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles));
    row(&h, "inner.pdf");
}

/// A two-page PDF.
const PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A file picked in the list is shown in the portfolio's tab, read-only, its row marked as the
/// one shown; going back to a file comes back to the view the reader left it in.
#[test]
fn a_file_picked_in_the_list_is_shown_read_only_in_the_portfolios_tab() {
    let mut h = opened(portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGES), ("two.pdf", PAGE)]), true);
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    assert_eq!(row(&h, "one.pdf").accesskit_node().is_selected(), Some(true));
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(2);
    row(&h, "two.pdf").click();
    h.run_steps(2);
    // Still one tab, the portfolio's, now showing the other file, which can't be changed.
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into()], Some("portfolio.pdf".into())));
    assert_eq!(shown(&h, 0).as_deref(), Some("two.pdf"));
    assert_eq!(row(&h, "two.pdf").accesskit_node().is_selected(), Some(true));
    assert_ne!(row(&h, "one.pdf").accesskit_node().is_selected(), Some(true));
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert!(!doc.allows_modification() && doc.read_only_reason.is_some(), "a portfolio's file is read-only in its tab");
    }
    // Back to the first file: on the page the reader left it at.
    row(&h, "one.pdf").click();
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    assert_eq!(h.state().views[0].current, 1);
}

/// ↑ and ↓ move the keyboard between the files, and Enter shows the one that has it.
#[test]
fn the_keyboard_moves_between_files_and_shows_one() {
    let mut h = opened(portfolio("/Collection << /View /D >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), true);
    row(&h, "one.pdf").focus();
    // The row has the keyboard from the next frame, and keeps ↑ and ↓ from the one after.
    h.run_steps(2);
    assert!(row(&h, "one.pdf").accesskit_node().is_focused());
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(1);
    assert!(row(&h, "two.pdf").accesskit_node().is_focused(), "↓ moves the keyboard to the next file");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("two.pdf"));
}

/// Closing the portfolio's tab closes the files it showed.
#[test]
fn closing_the_portfolios_tab_closes_the_files_it_showed() {
    let mut h = opened(portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), true);
    row(&h, "two.pdf").click();
    h.run_steps(2);
    h.state_mut().close_tab(0);
    h.run_steps(2);
    assert!(h.state().views.is_empty());
    assert!(h.state().session.docs().is_empty(), "no file of the portfolio stays open");
}

/// A file that isn't a PDF stays in the list: picking it says why it isn't shown.
#[test]
fn a_file_that_isnt_a_pdf_says_why_it_isnt_shown() {
    let mut h = opened(portfolio("/Collection << /D (one.pdf) >>", &[("notes.txt", b"hello"), ("one.pdf", PAGE)]), true);
    row(&h, "notes.txt").click();
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    let toast = h.state().toast.as_ref().map(|(m, _)| m.clone()).unwrap_or_default();
    assert!(toast.contains("notes.txt"), "the reader is told why: {toast:?}");
}

/// Unsaved changes to the portfolio, made before one of its files was shown, are asked about
/// when its tab closes, on the portfolio's own pages.
#[test]
fn unsaved_changes_to_the_portfolio_are_asked_about_when_its_tab_closes() {
    let mut h = opened(portfolio("/Collection << /View /D >>", &[("one.pdf", PAGE), ("two.pdf", PAGE), ("three.pdf", PAGE)]), true);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::DeleteAttachment { key: "three.pdf".into() }));
    h.run_steps(2);
    row(&h, "one.pdf").click();
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    h.state_mut().request_close_tab(0);
    h.run_steps(1);
    let s = h.state();
    assert!(s.close_request.is_some(), "closing asks about the unsaved portfolio");
    assert_eq!(s.views.len(), 1);
    assert_eq!(shown(&h, 0).as_deref(), Some("portfolio.pdf"));
}

/// The Menu key on a file's row opens its menu, whose Open in a new tab opens a copy to edit in a
/// tab of its own, beside the portfolio's. (Shift+F10 is asked for too, but the menu bar takes
/// F10 with an extra Shift before the panel reads it.)
#[test]
fn a_files_menu_opens_from_the_keyboard_and_opens_a_copy_in_a_new_tab() {
    let mut h = opened(portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), true);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    row(&h, "two.pdf").focus();
    h.run_steps(2);
    h.key_press(egui::Key::Menu);
    h.run_steps(3);
    // Its entries are drawn, under the row: there is no pointer to place the menu at.
    h.get_by_label("Open in a new tab").click();
    h.run_steps(3);
    assert_eq!(tabs(&h), (vec!["portfolio.pdf".into(), "two.pdf".into()], Some("two.pdf".into())));
    let s = h.state();
    assert!(s.session.get(s.views[1].id).is_some_and(|d| d.allows_modification()), "the copy can be edited");
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"), "the portfolio's tab keeps its file");
}

/// The app with `pdf` written to a file of test `test`'s own and opened from it, as a portfolio
/// on disk is; the file's path.
fn opened_from_file(pdf: Vec<u8>, test: &str) -> (Harness<'static, PdfCraftApp>, String) {
    let dir = std::env::temp_dir().join(format!("pdfcraft-portfolio-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("portfolio.pdf");
    std::fs::write(&path, &pdf).unwrap();
    let path = path.to_string_lossy().into_owned();
    let open = path.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.right_panel_closed = true;
        // The file watcher looks once per frame on the test's thread, as in `reload.rs`.
        app.run_inline = true;
        app.open_bytes("portfolio.pdf", Some(open.clone()), pdf.clone()).unwrap();
        app
    });
    h.run_steps(4);
    (h, path)
}

/// Save on a portfolio's tab showing one of its files writes the portfolio's unsaved changes,
/// on the portfolio's own pages, not the file shown.
#[test]
fn save_on_a_portfolios_tab_writes_the_portfolio() {
    let pdf = portfolio("/Collection << /View /D >>", &[("one.pdf", PAGE), ("two.pdf", PAGE), ("three.pdf", PAGE)]);
    let (mut h, path) = opened_from_file(pdf.clone(), "save");
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::DeleteAttachment { key: "three.pdf".into() }));
    row(&h, "one.pdf").click();
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    h.state_mut().save_view(0, pdfcraft_ui_egui::SaveTarget::InPlace);
    crate::support::wait_until(&mut h, "the portfolio saved", |app| app.tab_doc(0).and_then(|id| app.session.get(id)).is_some_and(|d| !d.dirty));
    assert_eq!(shown(&h, 0).as_deref(), Some("portfolio.pdf"), "saved on its own pages");
    assert_ne!(std::fs::read(&path).unwrap(), pdf, "the portfolio's file was written");
}

/// Opening a portfolio already open, from Open Recent, comes back to its tab, whichever of its
/// files it shows.
#[test]
fn opening_an_open_portfolio_again_comes_back_to_its_tab() {
    let (mut h, path) = opened_from_file(portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), "recent");
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    h.state_mut().open_recent(&path);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1, "no second copy of the portfolio");
    assert_eq!(h.state().active, Some(0));
}

/// A portfolio's tab keeps its files when its document is replaced (the file changed on disk).
#[test]
fn a_portfolios_tab_keeps_its_files_when_its_document_is_replaced() {
    let mut h = opened(portfolio("/Collection << /View /D >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), true);
    let info = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().info.clone()
    };
    h.state_mut().views[0].document_replaced(&info);
    h.run_steps(2);
    row(&h, "two.pdf").click();
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("two.pdf"), "the tab still shows the portfolio's files");
}

/// A two-page PDF that opens on its second page (`/OpenAction`).
const OPENS_ON_TWO: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /OpenAction [4 0 R /Fit] >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// A file shown for the first time opens the way it asks to, here on its second page, and the
/// tab keeps its file list.
#[test]
fn a_file_shown_opens_at_its_own_initial_view() {
    let h = opened(portfolio("/Collection << /D (two.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", OPENS_ON_TWO)]), true);
    assert_eq!(shown(&h, 0).as_deref(), Some("two.pdf"));
    assert_eq!(h.state().views[0].current, 1, "on the page its /OpenAction names");
    assert_eq!(h.state().right, Some(RightPanel::PortfolioFiles));
}

/// Save on a portfolio's tab showing one of its files saves the portfolio even when it has no
/// changes, on its own pages: Save As writes the portfolio, not the file shown.
#[test]
fn save_on_a_portfolios_tab_is_for_the_portfolio_even_without_changes() {
    let (mut h, _path) = opened_from_file(portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), "save-clean");
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    h.state_mut().save_view(0, pdfcraft_ui_egui::SaveTarget::InPlace);
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("portfolio.pdf"));
}

/// Opening a portfolio already open with a display asked for (as a launch with a page layout
/// does) applies it to the portfolio's own pages, not to the file its tab shows.
#[test]
fn opening_an_open_portfolio_with_a_display_applies_it_to_the_portfolio() {
    let (mut h, path) = opened_from_file(portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), "display");
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    h.state_mut().open_path_with_display(&path, pdfcraft_ui_egui::canvas::PageLayout::TwoUp, true);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1);
    assert_eq!(shown(&h, 0).as_deref(), Some("portfolio.pdf"));
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp);
}

/// The portfolio's file is watched while its tab shows one of its files: a new version on disk
/// reloads the portfolio, on its own pages, and a file picked again comes from the new version.
#[test]
fn a_portfolio_rewritten_on_disk_reloads_while_its_tab_shows_a_file() {
    let (mut h, path) = opened_from_file(portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), "reload");
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    row(&h, "two.pdf").click();
    h.run_steps(2);
    // A build rewrites it: one.pdf now has two pages; a later modification time.
    std::fs::write(&path, portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGES), ("two.pdf", PAGE)])).unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60)).unwrap();
    drop(file);
    crate::support::wait_until(&mut h, "the portfolio reloaded", |app| app.views[0].id == app.tab_doc(0).unwrap());
    row(&h, "one.pdf").click();
    h.run_steps(2);
    let s = h.state();
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    assert_eq!(s.session.get(s.views[0].id).unwrap().info.pages.len(), 2, "from the new version, not the one shown before");
}

/// A file shown in full screen keeps its own initial view for when full screen ends: here a
/// portfolio opened in full screen presents a file that asks for two columns.
#[test]
fn a_file_shown_in_full_screen_keeps_its_initial_view_after_it() {
    let two_columns =
        String::from_utf8_lossy(PAGES).replace("/Type /Catalog /Pages 2 0 R", "/Type /Catalog /Pages 2 0 R /PageLayout /TwoColumnLeft").into_bytes();
    let pdf = portfolio("/Collection << /D (two.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", &two_columns)]);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("other.pdf", None, PAGE.to_vec()).unwrap();
        app
    });
    h.run_steps(2);
    assert!(h.state_mut().execute("view.full_screen"));
    h.run_steps(2);
    h.state_mut().open_bytes("portfolio.pdf", None, pdf).unwrap();
    h.run_steps(4);
    let tab = h.state().active.unwrap();
    assert_eq!(shown(&h, tab).as_deref(), Some("two.pdf"));
    assert!(h.state_mut().execute("view.full_screen"), "full screen ends");
    h.run_steps(2);
    assert_eq!(h.state().views[tab].layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp, "the file's two columns, after full screen");
}

/// A view put aside follows full screen when it comes back: the portfolio's own pages brought
/// back in full screen (Save does) are presented, and a file put aside in full screen comes back
/// without presenting once full screen has ended.
#[test]
fn a_view_put_aside_follows_full_screen_when_it_comes_back() {
    let mut h = opened(portfolio("/Collection << /D (one.pdf) >>", &[("one.pdf", PAGE), ("two.pdf", PAGE)]), true);
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    assert!(h.state_mut().execute("view.full_screen"));
    h.run_steps(2);
    assert!(h.state().views[0].presenting());
    // Save comes back to the portfolio's own pages, put aside before full screen.
    h.state_mut().save_view(0, pdfcraft_ui_egui::SaveTarget::InPlace);
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("portfolio.pdf"));
    assert!(h.state().views[0].presenting(), "the portfolio's pages are presented too");
    assert!(h.state_mut().execute("view.full_screen"), "full screen ends");
    h.run_steps(2);
    row(&h, "one.pdf").click();
    h.run_steps(2);
    assert_eq!(shown(&h, 0).as_deref(), Some("one.pdf"));
    assert!(!h.state().views[0].presenting(), "the file put aside in full screen no longer presents");
}
