//! Copy with formatting's document-wide recovery state (hayro-interpret patch 107,
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

use super::cjk_fixture::{Doc, Font, Show};

/// A document's survey, run as Copy with formatting runs it before reading page 0, for a rule
/// that needs the usage of every kind of font the survey counts (no rule of this layer does).
fn surveyed(doc: &Doc) -> Arc<RecoveryDocument> {
    let pdf = parse(doc.build());
    let document = Arc::new(RecoveryDocument::needing_usage_for_tests());
    extract_page_recovering(&pdf, 0, &settings(Some(document.clone()))).unwrap();
    assert!(document.prepared());
    document
}

/// The survey counts every code a page or a form it draws shows (a form drawn twice counts
/// twice), in a Type 0 font through `Identity-H` with an embedded program and no `/ToUnicode`
/// stream; by the glyph id the `/CIDToGIDMap` gives a TrueType program, by CID in a CFF one.
#[test]
fn the_survey_counts_each_glyph_shown_across_the_document() {
    let mut mapped = Font::truetype("ABCDEF+Mapped", 40);
    mapped.cid_to_gid = Some((0..40).map(|cid| 39 - cid).collect());
    let mut named = Font::truetype("ABCDEF+Named", 40);
    named.to_unicode = true;
    let doc = Doc {
        fonts: vec![Font::truetype("ABCDEF+Plain", 40), mapped, Font::cff("ABCDEF+Cff", vec![5, 6, 7]), named],
        pages: vec![
            vec![Show::Text(0, vec![3, 3, 4]), Show::Text(1, vec![1]), Show::Text(3, vec![9])],
            vec![Show::Form(vec![Show::Text(0, vec![3]), Show::Text(2, vec![5, 7])], 2)],
        ],
    };
    let before = recovery_counters();
    let document = surveyed(&doc);
    let cost = recovery_counters().since(&before);
    assert_eq!(
        document.surveyed_fonts(),
        vec![(vec![(3, 4), (4, 1)], None), (vec![(5, 2), (7, 2)], None), (vec![(38, 1)], None)],
        "the font with a /ToUnicode stream is not counted"
    );
    assert_eq!((cost.survey_pages, cost.survey_glyphs), (2, 10), "{cost:?}");

    // Read again (another page, the same document), nothing is surveyed twice.
    let pdf = parse(doc.build());
    let before = recovery_counters();
    extract_page_recovering(&pdf, 1, &settings(Some(document.clone()))).unwrap();
    assert_eq!(recovery_counters().since(&before).survey_pages, 0);
}

/// Fonts with the same `/BaseFont` after the subset tag, the same glyph count and the same kind
/// of program form a group whose counts are added; a font differing in any of them is alone,
/// and so is a font that showed nothing.
#[test]
fn the_survey_groups_subsets_of_one_font() {
    let doc = Doc {
        fonts: vec![
            Font::truetype("AAAAAA+Kai", 50),
            Font::truetype("BBBBBB+Kai", 50),
            Font::truetype("CCCCCC+Kai", 51),
            Font::truetype("DDDDDD+Hei", 50),
            Font::cff("EEEEEE+Kai", (1..50).collect()),
            Font::truetype("FFFFFF+Kai", 50),
        ],
        pages: vec![vec![
            Show::Text(0, vec![10, 11]),
            Show::Text(1, vec![11, 12]),
            Show::Text(2, vec![13]),
            Show::Text(3, vec![14]),
            Show::Text(4, vec![15]),
        ]],
    };
    let document = surveyed(&doc);
    let group = Some((2, vec![(10, 1), (11, 2), (12, 1)]));
    assert_eq!(
        document.surveyed_fonts(),
        vec![
            (vec![(10, 1), (11, 1)], group.clone()),
            (vec![(11, 1), (12, 1)], group),
            (vec![(13, 1)], None),
            (vec![(14, 1)], None),
            (vec![(15, 1)], None),
        ]
    );
}

/// The survey reads the first 400 pages: a font shown on page 401 only has no evidence, one
/// shown on page 400 has.
#[test]
fn the_survey_reads_the_first_400_pages() {
    let mut pages = vec![Vec::new(); 401];
    pages[399] = vec![Show::Text(0, vec![7])];
    pages[400] = vec![Show::Text(1, vec![8])];
    let doc = Doc { fonts: vec![Font::truetype("ABCDEF+Early", 20), Font::truetype("ABCDEF+Late", 20)], pages };
    let before = recovery_counters();
    let document = surveyed(&doc);
    assert_eq!(recovery_counters().since(&before).survey_pages, 400);
    assert_eq!(document.surveyed_fonts(), vec![(vec![(7, 1)], None)]);
}

/// Past a cap the survey gives up and the document has no evidence at all, every time it is
/// read: counts, not time, decide.
#[test]
fn a_survey_past_its_cap_leaves_the_document_without_evidence() {
    use hayro::hayro_interpret::font::test_hooks::set_survey_glyph_cap;
    let doc = Doc { fonts: vec![Font::truetype("ABCDEF+Kai", 20)], pages: vec![vec![Show::Text(0, vec![1, 2, 3, 4])]] };
    set_survey_glyph_cap(Some(4));
    let within = surveyed(&doc);
    set_survey_glyph_cap(Some(3));
    let past = surveyed(&doc);
    set_survey_glyph_cap(None);
    assert!(!within.survey_over_cap());
    assert_eq!(within.surveyed_fonts().len(), 1);
    assert!(past.survey_over_cap());
    assert!(past.surveyed_fonts().is_empty());
}

/// A cancelled read keeps nothing: the next read surveys the document.
#[test]
fn a_cancelled_survey_keeps_nothing() {
    let pdf = parse(Doc { fonts: vec![Font::truetype("ABCDEF+Kai", 20)], pages: vec![vec![Show::Text(0, vec![1])]] }.build());
    let document = Arc::new(RecoveryDocument::needing_usage_for_tests());
    let cancelled = InterpreterSettings { cancelled: Some(Arc::new(std::sync::atomic::AtomicBool::new(true))), ..settings(Some(document.clone())) };
    document.prepare(&pdf, &cancelled);
    assert!(!document.prepared());
    document.prepare(&pdf, &settings(Some(document.clone())));
    assert_eq!(document.surveyed_fonts(), vec![(vec![(1, 1)], None)]);
}

/// A document without a font the survey counts (here a font with a `/ToUnicode` map, one
/// through another CMap, one of an Adobe collection and one without a program) costs no read
/// of its content.
#[test]
fn a_document_without_such_fonts_is_not_read() {
    let mut named = Font::truetype("ABCDEF+Named", 20);
    named.to_unicode = true;
    let mut cmap = Font::truetype("ABCDEF+Cmap", 20);
    cmap.encoding = "UniCNS-UCS2-H";
    let mut adobe = Font::truetype("ABCDEF+Adobe", 20);
    adobe.ordering = "CNS1";
    adobe.encoding = "Identity-H";
    let mut bare = Font::truetype("Bare", 20);
    bare.program = super::cjk_fixture::Program::None;
    let doc = Doc { fonts: vec![named, cmap, adobe, bare], pages: vec![vec![Show::Text(0, vec![1]), Show::Text(1, vec![1])]] };
    let before = recovery_counters();
    let document = surveyed(&doc);
    let cost = recovery_counters().since(&before);
    assert_eq!(cost.survey_pages, 0, "{cost:?}");
    assert!(document.surveyed_fonts().is_empty());
}

/// Two font objects with the same dictionary (one descendant, one program) are two fonts to the
/// survey: each shows its own glyphs, and a producer that merged subsets into one program may
/// order each one's glyphs differently. They are subsets of one font, so they form a group. Their
/// one program is read once.
#[test]
fn two_objects_of_one_dictionary_are_counted_apart() {
    let mut second = Font::truetype("ABCDEF+Kai", 40);
    second.alias_of = Some(0);
    let doc = Doc { fonts: vec![Font::truetype("ABCDEF+Kai", 40), second], pages: vec![vec![Show::Text(0, vec![3, 4]), Show::Text(1, vec![5])]] };
    let bytes = doc.build();
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(text.matches("/DescendantFonts [4 0 R]").count(), 2, "the second object repeats the first");
    let group = Some((2, vec![(3, 1), (4, 1), (5, 1)]));
    let before = recovery_counters();
    assert_eq!(surveyed(&doc).surveyed_fonts(), vec![(vec![(3, 1), (4, 1)], group.clone()), (vec![(5, 1)], group)]);
    let cost = recovery_counters().since(&before);
    assert_eq!(cost.survey_programs, 1, "their one program is read once: {cost:?}");
}

/// A group keeps its name and each member's own counts, with every glyph a member shows (a CID
/// its `/CIDToGIDMap` leaves out too), for rules that weigh the group against its members.
#[test]
fn a_group_keeps_its_name_and_each_members_counts() {
    let mut mapped = Font::truetype("BBBBBB+Kai", 50);
    mapped.cid_to_gid = Some(vec![0, 20, 21]);
    let doc = Doc {
        fonts: vec![Font::truetype("AAAAAA+Kai", 50), mapped],
        pages: vec![vec![Show::Text(0, vec![10, 10, 11]), Show::Text(1, vec![1, 2, 7])]],
    };
    assert_eq!(
        surveyed(&doc).surveyed_groups(),
        vec![(b"Kai".to_vec(), vec![(vec![(10, 2), (11, 1)], 3), (vec![(20, 1), (21, 1)], 3)])],
        "CID 7, left out of the map, counts among the glyphs shown"
    );
}

/// A document is surveyed only for a rule that reads its usage evidence
/// (`Rule::NEEDS_DOCUMENT_USAGE`). No rule of this layer does, so a document full of fonts the
/// survey would count is prepared without reading anything, and its fonts have no evidence; the
/// same document read for such a rule is surveyed.
#[test]
fn a_document_is_surveyed_only_for_a_rule_that_needs_its_usage() {
    let doc = Doc {
        fonts: vec![Font::truetype("ABCDEF+Kai", 20), Font::cff("ABCDEF+Ming", vec![1, 2])],
        pages: vec![vec![Show::Text(0, vec![1, 2]), Show::Text(1, vec![1])]],
    };
    let pdf = parse(doc.build());
    let document = Arc::new(RecoveryDocument::new());
    let before = recovery_counters();
    extract_page_recovering(&pdf, 0, &settings(Some(document.clone()))).unwrap();
    let cost = recovery_counters().since(&before);
    assert!(document.prepared());
    assert_eq!((cost.survey_runs, cost.survey_pages, cost.survey_glyphs), (0, 0, 0), "{cost:?}");
    assert!(document.surveyed_fonts().is_empty());

    let before = recovery_counters();
    let needing = surveyed(&doc);
    let cost = recovery_counters().since(&before);
    assert_eq!((cost.survey_runs, cost.survey_pages, cost.survey_glyphs), (1, 1, 3), "{cost:?}");
    assert_eq!(needing.surveyed_fonts().len(), 2);
}

/// The survey reads of a font only what it counts by, without building the font, and reads it
/// as the built font does: the same fonts counted, of the same kind and glyph count, with the
/// same number for every code. Here over TrueType programs through the identity map, a map
/// stream, a short one and one of more than 65,536 entries, `Identity-V`, an accepted and a
/// rejected `post` version; a CID-keyed CFF program, one with a map stream, a CFF program that
/// is not CID-keyed, a Type 1 program and bytes that are no program; and fonts the survey
/// passes over before reading a program; and a second object of a font, whose program and map
/// the survey has read already.
#[test]
fn the_survey_reads_each_font_as_the_built_font_does() {
    use super::cjk_fixture::Program;
    use hayro::hayro_interpret::font::test_hooks::survey_classifications;
    const TYPE1: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    let with = |base: &str, program: Program| Font { program, ..Font::truetype(base, 1) };
    let mut mapped = Font::truetype("AAAAAA+Mapped", 40);
    mapped.cid_to_gid = Some((0..40).map(|cid| 39 - cid).collect());
    let mut short = Font::truetype("AAAAAB+Short", 40);
    short.cid_to_gid = Some(vec![0, 5, 6]);
    let mut wrapped = Font::truetype("AAAAAC+Wrapped", 300);
    wrapped.cid_to_gid = Some((0..70_000_u32).map(|i| u16::try_from(i % 300).unwrap()).collect());
    let mut vertical = Font::truetype("AAAAAD+Vertical", 20);
    vertical.encoding = "Identity-V";
    let mut cff_mapped = Font::cff("AAAAAE+CffMapped", vec![5, 6]);
    cff_mapped.cid_to_gid = Some(vec![0, 1, 2]);
    let mut adobe = Font::truetype("AAAAAF+Adobe", 20);
    adobe.ordering = "CNS1";
    let mut named = Font::truetype("AAAAAG+Named", 20);
    named.to_unicode = true;
    let fonts = vec![
        Font::truetype("ABCDEF+Plain", 30),
        mapped,
        short,
        wrapped,
        vertical,
        with("ABCDEG+Swapped", Program::TrueTypePost(20, 0x0000_0003)),
        with("ABCDEH+BadPost", Program::TrueTypePost(20, 0x0004_0000)),
        Font::cff("ABCDEI+Cff", vec![3, 9, 12]),
        cff_mapped,
        with("ABCDEJ+Bare", Program::Raw(super::recovery_tests::cff_with_names(&["A", "B"]))),
        with("ABCDEK+Type1", Program::Raw(TYPE1.to_vec())),
        with("ABCDEL+Junk", Program::Raw(b"no font program".to_vec())),
        adobe,
        named,
        Font { alias_of: Some(1), ..Font::truetype("AAAAAA+Mapped", 40) },
        Font { alias_of: Some(7), ..Font::cff("ABCDEI+Cff", vec![3, 9, 12]) },
    ];
    let shows = (0..fonts.len()).map(|f| Show::Text(f, vec![1, 2, 7])).collect();
    let pdf = parse(Doc { fonts, pages: vec![shows] }.build());
    let mut found = survey_classifications(&pdf, &InterpreterSettings::default());
    found.sort_by_key(|(object, ..)| *object);
    assert_eq!(found.len(), 16);
    for (object, light, built) in &found {
        assert!(
            light == built,
            "font object {object:?}: {:?} against {:?}",
            light.as_ref().map(|l| (&l.0, l.1)),
            built.as_ref().map(|b| (&b.0, b.1))
        );
    }
    let kinds: Vec<Option<(&str, Option<u32>)>> = found.iter().map(|(_, light, _)| light.as_ref().map(|(k, g, _)| (k.as_str(), *g))).collect();
    let tt = |glyphs| Some(("CidTrueType", Some(glyphs)));
    assert_eq!(
        kinds,
        vec![
            tt(30),
            tt(40),
            tt(40),
            tt(300),
            tt(20),
            tt(20),
            None,
            Some(("CidCff", Some(4))),
            None,
            None,
            None,
            None,
            None,
            None,
            tt(40),
            Some(("CidCff", Some(4)))
        ]
    );
    let wrapped = &found[3].1.as_ref().unwrap().2;
    assert_eq!((wrapped[7], wrapped[5000]), (Some((65_536 + 7) % 300), Some(5000 % 300)), "CID 7 takes entry 65,543; CID 5000 its own");
}

/// Read with a document, font objects with byte-identical dictionaries are fonts of their own,
/// each decided by the rules, so the page's budget of fonts asked (1,024) counts each of them: a
/// page past it keeps its strict text. Counted by their shared dictionary, they were one font,
/// and any number of them ran their guards.
#[test]
fn font_objects_with_the_same_dictionary_each_count_toward_the_page_font_budget() {
    const FONTS_ASKED: usize = 1 << 10;
    const CFF: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    // Objects: 3 the page, 4 its content, 5 the descriptor, 6 the program, 7 onwards the fonts.
    let page = |fonts: usize| {
        let resources: String = (0..fonts).map(|i| format!("/F{i} {} 0 R ", 7 + i)).collect();
        // Each font shows its word in a cell of its own, all of them on the page.
        let content: String =
            (0..fonts).map(|i| format!("/F{i} 6 Tf 1 0 0 1 {} {} Tm <010203> Tj ", 10 + 20 * (i % 29), 290 - 8 * (i / 29))).collect();
        let mut pdf = pdfcraft_testkit::Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>")
            .obj(format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << {resources}>> >> >>"))
            .stream("", format!("BT {content}ET"))
            .obj(
                "<< /Type /FontDescriptor /FontName /Numbered /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 \
                 /CapHeight 700 /StemV 80 /FontFile3 6 0 R >>",
            )
            .stream("/Subtype /Type1C", CFF);
        for _ in 0..fonts {
            pdf = pdf.obj(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Numbered /FirstChar 1 /LastChar 3 /Widths [600 600 600] /FontDescriptor 5 0 R \
                 /Encoding << /Type /Encoding /Differences [1 /C83 /C111 /C99] >> >>",
            );
        }
        parse(pdf.build())
    };
    let document = || settings(Some(Arc::new(RecoveryDocument::new())));

    let within = page(FONTS_ASKED);
    let before = recovery_counters();
    let text = recovered(&within, 0, &document());
    assert_eq!(recovery_counters().since(&before).guard_decisions, FONTS_ASKED, "each font object is decided");
    assert_eq!(text.matches("Soc").count(), FONTS_ASKED);

    let past = page(FONTS_ASKED + 1);
    let (strict, layer) = extract_page_recovering(&past, 0, &document()).unwrap();
    assert!(layer.is_none(), "past the fonts asked, the page has no recovered layer");
    assert!(!strict.plain_text().contains("Soc"), "the strict text is kept as it is");
}
