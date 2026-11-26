use super::{SaveTarget, unredacted_file_name};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft::host::{AppInfo, Host, ManualJobs, Picked, ScriptedFiles, Wake};
use egui_kittest::Harness;

#[test]
fn restored_content_is_not_proposed_under_the_redacted_name() {
    assert_eq!(unredacted_file_name("contract_Redacted.pdf"), "contract.pdf");
    assert_eq!(unredacted_file_name("SCAN_Redacted.PDF"), "SCAN.PDF");
    assert_eq!(unredacted_file_name("Untitled_Redacted"), "Untitled");
    assert_eq!(unredacted_file_name("contract.pdf"), "contract.pdf");
    assert_eq!(unredacted_file_name("_Redacted.pdf"), "_Redacted.pdf");
}

/// A fresh, empty folder for one save test.
fn staging_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-staging-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The app on a host whose picks wait until the test answers them, in a window: the save flow
/// takes a pick's answer on a pass, as it does for the user.
fn shown_with_picks() -> (Harness<'static, crate::PdfCraftApp>, Arc<ScriptedFiles>, Arc<ManualJobs>) {
    let files = Arc::new(ScriptedFiles::held());
    let jobs = Arc::new(ManualJobs::new());
    let info = AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };
    let host = Host::builder(info).jobs(jobs.clone()).files(files.clone()).build();
    let (intake, _) = craft::platform::open::FileIntake::new(Wake::none());
    let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::EventClipboard::new(|| 0, |_| Ok(0)));
    let watch = craft::platform::watch::FileWatch::new(Wake::none()).inline();
    let services = crate::AppServices::new(Wake::none(), clipboard, None, None, intake, watch);
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
        crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
        {
            let mut app = crate::PdfCraftApp::with_host(Arc::new(host), services);
            app.set_option(&cc.egui_ctx, "language", "en").unwrap();
            app
        }
    });
    h.run_steps(2);
    (h, files, jobs)
}

/// Answers the open Save picker with `path`, and lets the app take the answer on later passes and run the
/// jobs it starts for the write.
fn answer_save(h: &mut Harness<'static, crate::PdfCraftApp>, files: &ScriptedFiles, jobs: &ManualJobs, path: &Path) {
    assert!(files.answer(Picked::Paths(vec![path.to_path_buf()])).is_some(), "a Save picker was open");
    for _ in 0..6 {
        jobs.run_all();
        h.run_steps(1);
    }
}

/// A one-page document with a proper xref table.
fn one_page_pdf() -> Vec<u8> {
    let objs =
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>", "<< /Type /Page /Parent 2 0 R >>"];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// `one_page_pdf`, opening without a password, whose permissions refuse changes.
fn no_changes_pdf() -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(one_page_pdf())).unwrap();
    let encryption = pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 8,
        encrypt_metadata: true,
        seed: [4; 32],
    };
    doc.set_encryption(&encryption).unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

/// Undo with a new text box open on a document that refuses changes: the box is put back as
/// it was. Finishing it had asked for the new item to be selected, and that request stayed
/// behind once the add was refused.
#[test]
fn a_refused_text_box_is_put_back_whole() {
    let mut app = crate::PdfCraftApp::for_unit_tests();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app.open_bytes(&egui::Context::default(), "a.pdf", None, no_changes_pdf()).unwrap();
    let style = crate::content_ui::default_style();
    app.views[0].session.content.draft = Some(crate::content_ui::TextDraft::new(0, None, [20.0, 20.0, 120.0, 40.0], "Typed".into(), style));
    app.undo(&egui::Context::default());
    let content = &app.views[0].session.content;
    assert_eq!(content.draft.as_ref().map(|d| d.text.as_str()), Some("Typed"), "the refused box stays open");
    assert_eq!(content.select_added, None, "nothing was added to select");
}

/// Save commits the open text box before anything is written, even when it then asks where.
#[test]
fn save_commits_the_open_text_box() {
    let dir = staging_dir("open-text-box");
    let dest = dir.join("out.pdf");
    let (mut h, files, jobs) = shown_with_picks();
    let app = h.state_mut();
    app.open_bytes(&egui::Context::default(), "a.pdf", None, one_page_pdf()).unwrap();
    let style = crate::content_ui::default_style();
    let mut draft = crate::content_ui::TextDraft::new(0, None, [20.0, 20.0, 120.0, 40.0], "Typed text".into(), style);
    draft.focus = true;
    app.views[0].session.content.draft = Some(draft);

    assert!(!app.save_active(&egui::Context::default(), SaveTarget::InPlace), "a new document asks where to save");
    let id = app.views[0].id;
    let added = app.session.get(id).unwrap().added();
    assert_eq!(added.len(), 1, "the open text box was committed before the save");
    assert!(app.views[0].session.content.draft.is_none());
    answer_save(&mut h, &files, &jobs, &dest);
    assert!(files.file(&dest).unwrap_or_default().starts_with(b"%PDF"), "the document with its text went where the user chose");
    assert!(!h.state().session.get(id).unwrap().dirty);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Moving pages cancels a drawing whose page index would now name another page; rotating the
/// page keeps it.
#[test]
fn moving_pages_cancels_an_in_progress_drawing() {
    use craft::review::ui::AnnotateTool;
    use craft::review::{AnnotationKind, AnnotationTool};
    use craft::ui::testing::{ToolFixture, pointer};

    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>",
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());

    // A polygon being clicked out on page 0, two points so far, drawn through the tool's pointer
    // input; each click waits past the double-click time, so none ends the drawing. The view puts the page's (0, 0) at
    // the canvas's top left corner at zoom 1, so the clicks land on the page.
    let view = |room: egui::Rect| {
        craft::behavior::CanvasView::new(room, craft::behavior::kurbo::Point::new(f64::from(room.width()) / 2.0, f64::from(room.height()) / 2.0), 1.0)
    };
    let tool = AnnotateTool::new(AnnotationTool::Draw(AnnotationKind::Polygon), 0);
    let mut rig = ToolFixture::with_view("Document", tool, view, |_, _| {});
    for (x, y) in [(50.0, 50.0), (150.0, 50.0)] {
        let at = rig.canvas().rect.min + egui::vec2(x, y);
        let fixture = rig.fixture();
        fixture.frame(vec![pointer::moved(at)]);
        fixture.frame(vec![pointer::press(at)]);
        fixture.frame(vec![pointer::release(at)]);
        fixture.idle(20);
    }
    let tool = std::mem::take(&mut *rig.tool_mut());
    assert_eq!(tool.drawing().len(), 2, "the drawing is under way");

    let mut app = crate::PdfCraftApp::for_unit_tests();
    app.set_option(&egui::Context::default(), "language", "en").unwrap();
    app.open_bytes(&egui::Context::default(), "a.pdf", None, pdf).unwrap();
    app.views[0].session.comments.tool = tool;
    assert!(app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    assert_eq!(app.views[0].session.comments.tool.drawing().len(), 2, "rotating the page keeps the drawing on it");
    assert!(app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::MovePages { pages: vec![1], to: 0 }));
    assert!(app.views[0].session.comments.tool.drawing().is_empty(), "the drawing's page index would now be a different page");
}

#[test]
fn save_asks_where_to_keep_an_email_attachment_and_leaves_the_temporary_copy_alone() {
    let dir = staging_dir("mail-temp");
    let cache = dir.join("INetCache").join("Content.Outlook").join("AB12CD34");
    std::fs::create_dir_all(&cache).unwrap();
    let attachment = cache.join("Quote.pdf");
    let original = one_page_pdf();
    std::fs::write(&attachment, &original).unwrap();
    let kept = dir.join("Quote.pdf");

    let (mut h, files, jobs) = shown_with_picks();
    let app = h.state_mut();
    app.open_bytes(&egui::Context::default(), "Quote.pdf", Some(attachment.to_string_lossy().into_owned()), original.clone()).unwrap();
    assert!(app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));

    assert!(!app.save_active(&egui::Context::default(), SaveTarget::InPlace), "Save asks instead of writing the temporary copy");
    answer_save(&mut h, &files, &jobs, &kept);
    assert_eq!(std::fs::read(&attachment).unwrap(), original, "the attachment's copy is untouched");
    assert!(files.file(&kept).unwrap_or_default().starts_with(b"%PDF"), "the document went where the user chose");

    // The chosen file is the document's file now: the next Save writes it without asking.
    let app = h.state_mut();
    assert!(app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    assert!(app.save_active(&egui::Context::default(), SaveTarget::InPlace));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #6440: recolouring gray or CMYK text in its open box drops the ink at once. Finishing the box
/// then adds no empty Undo step (the draft equals the item the page reads back), picking the
/// ink's own look again in the same box paints RGB rather than bringing the ink back, and new
/// boxes don't take the ink.
#[test]
fn recolouring_print_ink_text_in_its_box_drops_the_ink_without_an_empty_step() {
    // 100 K text, open in its box.
    let open = || {
        let mut app = crate::PdfCraftApp::for_unit_tests();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes(&egui::Context::default(), "a.pdf", None, one_page_pdf()).unwrap();
        let mut text = pdfcraft_engine::AddedText { rect: [20.0, 200.0, 180.0, 220.0], text: "Proof".into(), ..Default::default() };
        text.set_ink(Some(pdfcraft_engine::TextInk::Cmyk([0.0, 0.0, 0.0, 1.0])));
        assert!(app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::AddText { page: 0, text }));
        let stored = item(&app);
        assert!(stored.painted_ink().is_some());
        let mut draft = crate::content_ui::TextDraft::new(0, Some(0), stored.rect, stored.text.clone(), stored);
        draft.focus = true;
        app.views[0].session.content.draft = Some(draft);
        app
    };
    fn item(app: &crate::PdfCraftApp) -> pdfcraft_engine::AddedText {
        match &app.session.get(app.views[0].id).unwrap().added()[0].content {
            pdfcraft_engine::AddedContent::Text(t) => t.clone(),
            other => panic!("{other:?}"),
        }
    }
    // The box's style with another colour, set as the Format panel hands it over.
    let recolour = |app: &mut crate::PdfCraftApp, color: [f64; 3]| {
        let style = app.views[0].session.content.draft.as_ref().map(|d| d.style.clone()).unwrap();
        app.restyle_draft(&egui::Context::default(), 0, pdfcraft_engine::AddedText { color, ..style });
    };
    let finish = |app: &mut crate::PdfCraftApp| {
        let added = app.session.get(app.views[0].id).unwrap().added().to_vec();
        app.views[0].session.content.finish_draft(&added)
    };

    // Red, then Done.
    let mut app = open();
    recolour(&mut app, [1.0, 0.0, 0.0]);
    assert_eq!(item(&app).ink, None, "the page paints red in RGB");
    let finished = finish(&mut app);
    assert!(finished.is_none(), "an empty Undo step: {finished:?}");
    assert_eq!(app.text_style.ink, None, "new boxes don't take the ink");

    // Red, then black in the same box: RGB black, not the 100 K back.
    let mut app = open();
    recolour(&mut app, [1.0, 0.0, 0.0]);
    recolour(&mut app, [0.0; 3]);
    let now = item(&app);
    assert_eq!((now.ink, now.color), (None, [0.0; 3]));
    assert_eq!(app.text_style.ink, None);
    let finished = finish(&mut app);
    assert!(finished.is_none(), "an empty Undo step: {finished:?}");
}
