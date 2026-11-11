//! Copy with formatting's document-wide recovery state (hayro-interpret patch 104,
//! `RecoveryDocument`): each font's rule decisions are made once for the document, whichever
//! page asks first and on whichever thread, and read every page as a decision made on that page
//! would.

use super::*;
use hayro::hayro_interpret::font::{RecoveryDocument, recovery_counters};
use std::sync::Arc;

fn parse(bytes: Vec<u8>) -> Pdf {
    Pdf::new(Arc::new(bytes)).unwrap()
}

/// `pages` pages, each showing `C83 C111 C99` (`Soc`) in a simple Type 1 font whose
/// `/Differences` name its glyphs `/C83 /C111 /C99`: a font the numbered-names rule reads.
fn numbered_names_pages(pages: usize) -> Vec<u8> {
    const CFF: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    let first_page = 6;
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", first_page + 2 * i)).collect();
    let mut pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 600 300] >>", kids.join(" ")))
        .obj(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Numbered /FirstChar 1 /LastChar 3 /Widths [600 600 600] /FontDescriptor 4 0 R \
             /Encoding << /Type /Encoding /Differences [1 /C83 /C111 /C99] >> >>",
        )
        .obj(
            "<< /Type /FontDescriptor /FontName /Numbered /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 \
             /CapHeight 700 /StemV 80 /FontFile3 5 0 R >>",
        )
        .stream("/Subtype /Type1C", CFF);
    for _ in 0..pages {
        let contents = pdf.next_obj() + 1;
        pdf = pdf
            .obj(format!("<< /Type /Page /Parent 2 0 R /Contents {contents} 0 R /Resources << /Font << /F3 3 0 R >> >> >>"))
            .stream("", "BT /F3 20 Tf 60 250 Td <010203> Tj ET");
    }
    pdf.build()
}

fn settings(document: Option<Arc<RecoveryDocument>>) -> InterpreterSettings {
    InterpreterSettings { recovery_document: document, ..InterpreterSettings::default() }
}

/// The recovered text of `page`.
fn recovered(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> String {
    extract_page_recovering(pdf, page, settings).unwrap().1.map(|r| r.text.plain_text()).unwrap_or_default()
}

/// With a document, a font shown on every page is decided once; each further page takes the
/// decision the first one made. Without one, every page decides it again.
#[test]
fn a_document_decides_each_font_once_for_all_its_pages() {
    let pdf = parse(numbered_names_pages(3));

    let before = recovery_counters();
    let alone: Vec<String> = (0..3).map(|p| recovered(&pdf, p, &settings(None))).collect();
    let cost = recovery_counters().since(&before);
    assert_eq!(cost.guard_decisions, 3, "{cost:?}");
    assert_eq!(cost.decisions_reused, 0, "{cost:?}");

    let document = settings(Some(Arc::new(RecoveryDocument::new())));
    let before = recovery_counters();
    let shared: Vec<String> = (0..3).map(|p| recovered(&pdf, p, &document)).collect();
    let cost = recovery_counters().since(&before);
    assert_eq!(cost.guard_decisions, 1, "{cost:?}");
    assert_eq!(cost.decisions_reused, 2, "{cost:?}");

    assert_eq!(shared, alone);
    assert!(shared.iter().all(|t| t.contains("Soc")), "{shared:?}");
}

/// The pages read the same whichever is read first, and from however many threads share the
/// document's decisions.
#[test]
fn a_documents_pages_read_the_same_in_any_order_and_on_any_thread() {
    let bytes = numbered_names_pages(4);
    let pdf = parse(bytes.clone());
    let alone: Vec<String> = (0..4).map(|p| recovered(&pdf, p, &settings(None))).collect();

    for order in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 0, 3, 1]] {
        let document = settings(Some(Arc::new(RecoveryDocument::new())));
        let mut read = vec![String::new(); 4];
        for p in order {
            read[p] = recovered(&pdf, p, &document);
        }
        assert_eq!(read, alone, "order {order:?}");
    }

    let document = Arc::new(RecoveryDocument::new());
    let read: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|p| {
                let (bytes, document) = (bytes.clone(), document.clone());
                // Each thread parses the document itself, as each render worker may.
                s.spawn(move || recovered(&parse(bytes), p, &settings(Some(document))))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(read, alone);
}

/// A pool's workers share one recovery state: the font every page shows is decided once for the
/// pool, whichever worker reads which page.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_render_pools_workers_share_the_documents_decisions() {
    use crate::{RenderConfig, RenderPool, RenderRequest, RequestKind};
    let pool = RenderPool::new(Arc::new(numbered_names_pages(4)), 3, RenderConfig::default());
    pool.set_queue((0..4).map(|page| RenderRequest { page, kind: RequestKind::RecoveredText, ..Default::default() }).collect());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut done = 0;
    while done < 4 {
        assert!(std::time::Instant::now() < until, "the pool answered {done} pages of 4");
        let Some(page) = pool.try_recv() else {
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        };
        assert!(page.recovered.is_some_and(|t| t.text.plain_text().contains("Soc")), "page {}", page.request.page);
        done += 1;
    }
    assert_eq!(pool.recovery_state(), "RecoveryDocument { fonts: 1 }");
}

/// A renderer's recovery scope is shared only by renderers of the same bytes: another
/// version's renderer keeps its own.
#[test]
fn a_recovery_scope_is_shared_by_renderers_of_the_same_bytes_only() {
    use crate::{PageRenderer, RecoveryScope, RenderConfig, RenderRequest, RequestKind};
    let bytes = Arc::new(numbered_names_pages(2));
    let scope = RecoveryScope::for_bytes(&bytes);
    let read = |r: &mut PageRenderer, page: usize| {
        let out = r.render(RenderRequest { page, kind: RequestKind::RecoveredText, ..Default::default() });
        out.recovered.map(|t| t.text.plain_text()).unwrap_or_default()
    };

    let mut first = PageRenderer::new(bytes.clone(), RenderConfig::default()).with_recovery_scope(&scope);
    let mut second = PageRenderer::new(bytes.clone(), RenderConfig::default()).with_recovery_scope(&scope);
    let before = recovery_counters();
    assert!(read(&mut first, 0).contains("Soc"));
    assert!(read(&mut second, 1).contains("Soc"));
    let cost = recovery_counters().since(&before);
    assert_eq!((cost.guard_decisions, cost.decisions_reused), (1, 1), "{cost:?}");

    // The same content in another allocation is another version: its renderer decides anew.
    let other = Arc::new(bytes.as_ref().clone());
    let mut third = PageRenderer::new(other, RenderConfig::default()).with_recovery_scope(&scope);
    let before = recovery_counters();
    assert!(read(&mut third, 0).contains("Soc"));
    let cost = recovery_counters().since(&before);
    assert_eq!((cost.guard_decisions, cost.decisions_reused), (1, 0), "{cost:?}");
}
