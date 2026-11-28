//! Bates numbering ▸ Add to files…: one run over several files, written as numbered copies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::support::AppHarness as _;
use craft::host::{Picked, ScriptedFiles};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::PdfCraftApp;

fn harness(files: Arc<ScriptedFiles>) -> Harness<'static, PdfCraftApp> {
    let host = crate::support::host_with_files(files);
    Harness::builder().with_size(egui::vec2(1200.0, 900.0)).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        let mut app = crate::support::test_app(host);
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app
    })
}

/// The Bates number on each page of `pdf`: the run of digits in the page's text after "CASE-".
fn numbers(pdf: Vec<u8>) -> Vec<u64> {
    let mut r = PageRenderer::new(Arc::new(pdf), Default::default());
    (0..r.page_count())
        .map(|p| {
            let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            let text = out.text.map(|t| t.plain_text()).unwrap_or_default();
            let at = text.find("CASE-").expect("a Bates number on every page") + "CASE-".len();
            text[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
        })
        .collect()
}

/// Opens the dialog, fills its list with `sources` through Browse, types the prefix and presses Number.
fn number(h: &mut Harness<'static, PdfCraftApp>) {
    assert!(h.execute("edit.bates_files"));
    h.run_steps(3);
    let dialog = h.get_by_role_and_label(Role::Dialog, "Add Bates Numbering to Files");
    dialog.get_by_label("Browse…").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::TextInput, "Prefix").type_text("CASE-");
    h.run_steps(2);
    h.get_by_label("Number").click();
    h.run_steps(4);
}

fn paths() -> (PathBuf, PathBuf) {
    (Path::new("case").join("b.pdf"), Path::new("case").join("a.pdf"))
}

#[test]
fn each_copy_continues_the_numbers_of_the_one_before_and_the_originals_stay() {
    let (b, a) = paths();
    let (b_bytes, a_bytes) = (crate::support::fixtures::pdf_pages(2), crate::support::fixtures::pdf_pages(3));
    let files =
        Arc::new(ScriptedFiles::answering([Picked::Paths(vec![b.clone(), a.clone()])]).with_file(&b, b_bytes.clone()).with_file(&a, a_bytes.clone()));
    let mut h = harness(files.clone());
    number(&mut h);
    let first = numbers(files.file(&Path::new("case").join("b_Bates.pdf")).expect("the first file's copy"));
    let second = numbers(files.file(&Path::new("case").join("a_Bates.pdf")).expect("the second file's copy"));
    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 3);
    for pages in [&first, &second] {
        assert!(pages.windows(2).all(|w| w[1] == w[0] + 1), "pages count up by one: {pages:?}");
    }
    assert_eq!(second[0], first[1] + 1, "the list's second file starts after the first one's last page");
    assert_eq!(files.file(&b), Some(b_bytes), "the original is not changed");
    assert_eq!(files.file(&a), Some(a_bytes), "the original is not changed");
}

#[test]
fn a_copy_that_exists_already_means_nothing_is_written() {
    let (b, a) = paths();
    let taken = Path::new("case").join("a_Bates.pdf");
    let files = Arc::new(
        ScriptedFiles::answering([Picked::Paths(vec![b.clone(), a.clone()])])
            .with_file(&b, crate::support::fixtures::pdf_pages(2))
            .with_file(&a, crate::support::fixtures::pdf_pages(1))
            .with_file(&taken, b"keep".to_vec()),
    );
    let mut h = harness(files.clone());
    number(&mut h);
    assert!(files.written().is_empty(), "no copy is written when one can't be: {:?}", files.written());
    assert_eq!(files.file(&taken).as_deref(), Some(&b"keep"[..]), "the file there is kept");
    let notice = crate::support::shown_notice(&h).unwrap_or_default();
    assert!(notice.contains("a_Bates.pdf"), "the notice names the copy in the way: {notice}");
}

#[test]
fn two_copies_that_would_share_a_name_mean_nothing_is_written() {
    // On a case-insensitive disk the two copies are one file, so the second would replace the first.
    let (lower, upper) = (Path::new("case").join("a.pdf"), Path::new("case").join("A.pdf"));
    let files = Arc::new(
        ScriptedFiles::answering([Picked::Paths(vec![lower.clone(), upper.clone()])])
            .with_file(&lower, crate::support::fixtures::pdf_pages(1))
            .with_file(&upper, crate::support::fixtures::pdf_pages(1)),
    );
    let mut h = harness(files.clone());
    number(&mut h);
    assert!(files.written().is_empty(), "neither copy is written: {:?}", files.written());
    let notice = crate::support::shown_notice(&h).unwrap_or_default();
    assert!(notice.contains("_Bates.pdf"), "the notice names the shared copy name: {notice}");
}
