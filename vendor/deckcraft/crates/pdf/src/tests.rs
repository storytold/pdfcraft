use std::sync::Arc;

use std::collections::HashMap;

use deckcraft_fonts::FontDb;
use deckcraft_render::RenderOpts;
use kurbo::Rect;

use super::{
    MAX_RASTER_PIXELS, PAPERS, PageLayout, PdfError, PdfOptions, Presentation, export, fill_clear, glyph_text_ranges, handout_boxes,
    master_text_pieces, master_texts, notes_frames, notes_slide_box, opaque, paper_size, raster_scale, straight,
};
use deckcraft_model::text::TextBody;

fn deck(n: usize) -> Presentation {
    let mut p = deckcraft_model::defaults::new_presentation(None);
    let layout = p.slides[0].layout;
    while p.slides.len() < n {
        let s = deckcraft_model::defaults::new_slide(&mut p, layout).expect("slide");
        p.slides.push(Arc::new(s));
    }
    for (i, s) in p.slides.iter_mut().enumerate() {
        let s = Arc::make_mut(s);
        if let Some(sh) = s.shapes.first_mut() {
            sh.text = Some(TextBody::from_text(&format!("Slide title {i}")));
        }
        s.notes = TextBody::from_text("Speaker notes that wrap across the page width to test line breaking of long notes.");
    }
    p
}

fn opts(layout: PageLayout) -> PdfOptions {
    PdfOptions { layout, dpi: 48.0, ..Default::default() }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn exports_slides_notes_and_handouts() {
    let p = deck(5);
    for layout in [PageLayout::Slides, PageLayout::Notes, PageLayout::Handouts { per_page: 3 }, PageLayout::Handouts { per_page: 6 }] {
        let bytes = export(&p, &opts(layout)).expect("export");
        assert!(bytes.starts_with(b"%PDF-"), "{layout:?}");
        assert!(bytes.len() > 1000);
        let pdf = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
        let (pages, size) = match layout {
            PageLayout::Slides => (5, (960.0, 540.0)),
            PageLayout::Notes => (5, (612.0, 792.0)),
            PageLayout::Handouts { per_page: 3 } => (2, (612.0, 792.0)),
            _ => (1, (612.0, 792.0)),
        };
        assert_eq!(pdf.pages().len(), pages);
        assert!(pdf.pages().iter().all(|page| page.render_dimensions() == size));
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn empty_range_is_an_error() {
    let p = deck(1);
    let o = PdfOptions { slides: Some(vec![7]), ..opts(PageLayout::Slides) };
    assert!(matches!(export(&p, &o), Err(PdfError::NoPages)));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn hidden_slides_skipped_unless_asked() {
    let mut p = deck(2);
    Arc::make_mut(&mut p.slides[1]).hidden = true;
    let a = export(&p, &opts(PageLayout::Slides)).unwrap();
    let b = export(&p, &PdfOptions { include_hidden: true, ..opts(PageLayout::Slides) }).unwrap();
    assert_eq!(hayro_syntax::Pdf::new(a.clone()).unwrap().pages().len(), 1);
    assert_eq!(hayro_syntax::Pdf::new(b.clone()).unwrap().pages().len(), 2);
    let ranged = export(&p, &PdfOptions { slides: Some(vec![1, 0, 9]), ..opts(PageLayout::Slides) }).unwrap();
    assert_eq!(hayro_syntax::Pdf::new(ranged.clone()).unwrap().pages().len(), 2);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn handout_boxes_fit_the_paper() {
    for n in [1, 2, 3, 4, 6, 9] {
        let v = handout_boxes(n, (612.0, 792.0), 16.0 / 9.0);
        assert_eq!(v.len(), n as usize);
        for r in &v {
            assert!(r.x0 >= 0.0 && r.y0 >= 0.0 && r.x1 <= 612.0 && r.y1 <= 792.0, "{n}: {r:?}");
            assert!((r.width() / r.height() - 16.0 / 9.0).abs() < 1e-6);
        }
        for (i, a) in v.iter().enumerate() {
            for b in &v[i + 1..] {
                assert!(a.intersect(*b).area() <= 0.0, "{n}: overlap");
            }
        }
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn metadata_retains_values_and_omits_blank_fields() {
    let mut p = deck(1);
    p.props.title = "  Deck title  ".into();
    p.props.author = "  Author  ".into();
    let bytes = export(&p, &opts(PageLayout::Slides)).unwrap();
    let pdf = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    assert_eq!(pdf.metadata().title.as_deref(), Some(b"  Deck title  ".as_slice()));
    assert_eq!(pdf.metadata().author.as_deref(), Some(b"  Author  ".as_slice()));
    assert_eq!(pdf.metadata().creator.as_deref(), Some(b"DeckCraft".as_slice()));
    assert_eq!(pdf.metadata().producer.as_deref(), Some(b"DeckCraft".as_slice()));
    assert!(pdf.metadata().creation_date.is_none());
    p.props.author = " ".into();
    let bytes = export(&p, &PdfOptions { title: Some(" ".into()), ..opts(PageLayout::Slides) }).unwrap();
    let pdf = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    assert!(pdf.metadata().title.is_none());
    assert!(pdf.metadata().author.is_none());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn text_layer_links_and_bookmarks_survive_serialization() {
    use deckcraft_model::text::{Action, Hyperlink};
    use hayro_syntax::object::{Array, Dict};
    let mut p = deck(2);
    let last = p.slides[1].id;
    let slide = Arc::make_mut(&mut p.slides[0]);
    for (shape, action) in slide.shapes.iter_mut().zip([Action::Url { url: "https://example.com/deck".into() }, Action::Slide { slide: last }]) {
        shape.click = Some(Hyperlink { action, tooltip: "Link".into(), highlight_click: false, sound: None, stop_sound: false, zoom_return: None });
    }
    for text_layer in [true, false] {
        let bytes = export(&p, &PdfOptions { text_layer, ..opts(PageLayout::Slides) }).unwrap();
        let pdf = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
        let page = &pdf.pages()[0];
        let annotations = page.raw().get::<Array<'_>>(b"Annots").unwrap();
        assert_eq!(annotations.iter::<Dict<'_>>().count(), 2);
        let resources = page.raw().get::<Dict<'_>>(b"Resources").unwrap();
        let fonts = resources.get::<Dict<'_>>(b"Font");
        assert_eq!(fonts.is_some_and(|fonts| !fonts.is_empty()), text_layer);
        let catalog = pdf.xref().get::<Dict<'_>>(pdf.xref().root_id()).unwrap();
        let outlines = catalog.get::<Dict<'_>>(b"Outlines").unwrap();
        let first = outlines.get::<Dict<'_>>(b"First").unwrap();
        assert!(first.get::<Dict<'_>>(b"Next").is_some());
    }
}

/// Links from an untrusted deck: only web and mail links become PDF links. Scripts, `data:`,
/// local and relative files, OS protocol handlers and network shares (a UNC path leaks the user's
/// NTLM credentials on Windows) do not.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn unsafe_url_links_are_not_exported() {
    use deckcraft_model::text::{Action, Hyperlink};
    use deckcraft_model::{Shape, ShapeId, Xfrm};
    use hayro_syntax::object::{Array, Dict, String as PdfString};
    let safe = ["https://example.com/deck", "mailto:someone@example.com"];
    let refused = [
        "file:///C:/talks/notes.pdf",
        "notes.pdf",
        "ms-msdt:/id PCWDiagnostic /skip force",
        "ms-word:ofe|u|\\\\attacker.example\\share\\x.docx",
        "ldap://attacker.example/x",
        "file:///%5C%5Cattacker.example/share/x",
        "javascript:alert(1)",
        "\t JavaScript:alert(1)",
        "java\nscript:alert(1)",
        "vbscript:MsgBox(1)",
        "data:text/html,<script>alert(1)</script>",
        "\\\\attacker.example\\share\\x",
        "//attacker.example/share/x",
        "file://attacker.example/share/x",
        "file:////attacker.example/share/x",
        "smb://attacker.example/share/x",
        "search-ms:query=x&crumb=location:\\\\attacker.example\\share",
        "search:query=x&crumb=location:\\\\attacker.example\\share",
    ];
    let mut p = deck(1);
    let slide = Arc::make_mut(&mut p.slides[0]);
    slide.shapes = (0..)
        .zip(safe.iter().chain(&refused))
        .map(|(i, url)| Shape {
            id: ShapeId(9000 + i),
            xfrm: Some(Xfrm::new(20.0, 20.0 + 32.0 * f64::from(i), 400.0, 26.0)),
            click: Some(Hyperlink {
                action: Action::Url { url: url.to_string() },
                tooltip: String::new(),
                highlight_click: false,
                sound: None,
                stop_sound: false,
                zoom_return: None,
            }),
            ..Default::default()
        })
        .collect();
    let bytes = export(&p, &opts(PageLayout::Slides)).unwrap();
    let pdf = hayro_syntax::Pdf::new(bytes.clone()).unwrap();
    let annotations = pdf.pages()[0].raw().get::<Array<'_>>(b"Annots").unwrap();
    let uris: Vec<String> = annotations
        .iter::<Dict<'_>>()
        .filter_map(|a| a.get::<Dict<'_>>(b"A")?.get::<PdfString<'_>>(b"URI"))
        .map(|s| String::from_utf8_lossy(s.as_bytes()).into_owned())
        .collect();
    assert_eq!(uris, safe);
}

/// Build a small damaged derivative of our openly licensed last-resort font entirely in
/// memory. Its naming/shaping tables still parse, but its missing head table prevents PDF
/// embedding. No font bytes or vendor assets are stored as test fixtures.
fn font_without_head() -> Vec<u8> {
    // The final bundled face is always Ubuntu Light; a system lookup may select a variable font.
    let mut bytes = deckcraft_fonts::bundled().last().unwrap().to_vec();
    for (old, new) in [(b"Ubuntu".as_slice(), b"BadPDF".as_slice()), (b"\0U\0b\0u\0n\0t\0u".as_slice(), b"\0B\0a\0d\0P\0D\0F".as_slice())] {
        let mut offset = 0;
        while offset + old.len() <= bytes.len() {
            if bytes[offset..offset + old.len()] == *old {
                bytes[offset..offset + old.len()].copy_from_slice(new);
                offset += old.len();
            } else {
                offset += 1;
            }
        }
    }
    let tables = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    for i in 0..tables {
        let offset = 12 + i * 16;
        if &bytes[offset..offset + 4] == b"head" {
            bytes[offset..offset + 4].copy_from_slice(b"zzzz");
        }
    }
    bytes
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn unembeddable_font_returns_error_instead_of_a_pdf_missing_text() {
    use deckcraft_model::{Shape, ShapeId, Xfrm};
    let db = FontDb::global();
    assert_eq!(db.add_font(font_without_head()), 1);
    assert_eq!(db.face("BadPDF", "Regular").family, "BadPDF");
    let mut p = deck(1);
    let slide = Arc::make_mut(&mut p.slides[0]);
    let mut body = TextBody::from_text("Text that must not disappear silently");
    for paragraph in &mut body.paragraphs {
        for run in &mut paragraph.runs {
            run.props.font = Some("BadPDF".into());
            run.props.size = Some(12.0);
        }
    }
    slide.shapes = vec![Shape { id: ShapeId(9000), xfrm: Some(Xfrm::new(20.0, 20.0, 500.0, 200.0)), text: Some(body), ..Default::default() }];
    let error = export(&p, &opts(PageLayout::Slides)).unwrap_err();
    assert!(matches!(error, PdfError::Write(_)));
    assert!(error.to_string().contains("font could not be read for embedding"));
    // Master text remains visible even when the optional slide text layer is off.
    // It must propagate the same embedding error through notes and handout pages.
    let mut header = ph(deckcraft_model::PhType::Header, 0.0, 0.0, 500.0, 40.0, "");
    header.text = p.slides[0].shapes[0].text.clone();
    for layout in [PageLayout::Notes, PageLayout::Handouts { per_page: 3 }] {
        let mut p = deck(1);
        let master = Some(Arc::new(deckcraft_model::Master { shapes: vec![header.clone()], ..Default::default() }));
        if layout == PageLayout::Notes {
            p.notes_master = master;
        } else {
            p.handout_master = master;
        }
        let error = export(&p, &PdfOptions { text_layer: false, ..opts(layout) }).unwrap_err();
        assert!(matches!(error, PdfError::Write(_)));
        assert!(error.to_string().contains("font could not be read for embedding"), "{layout:?}: {error}");
    }
    // The failed export leaves no poisoned writer state behind.
    assert!(export(&deck(1), &opts(PageLayout::Slides)).is_ok());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn supported_unicode_is_shaped_and_recovered_from_the_page_text_layer() {
    use deckcraft_model::{Shape, ShapeId, Xfrm};

    let text = "Café Ελληνικά Привет";
    let face = FontDb::global().face("Ubuntu", "Regular");
    assert_eq!(face.family, "Ubuntu");
    assert!(text.chars().all(|c| face.glyph_for(c) != 0), "missing: {:?}", text.chars().filter(|c| face.glyph_for(*c) == 0).collect::<Vec<_>>());
    let mut body = TextBody::from_text(text);
    for paragraph in &mut body.paragraphs {
        for run in &mut paragraph.runs {
            run.props.font = Some("Ubuntu".into());
            run.props.size = Some(20.0);
        }
    }
    let mut p = deck(1);
    Arc::make_mut(&mut p.slides[0]).shapes =
        vec![Shape { id: ShapeId(9001), xfrm: Some(Xfrm::new(20.0, 20.0, 920.0, 100.0)), text: Some(body), ..Default::default() }];
    let placed = deckcraft_render::place_slide(&p, 0);
    let runs: Vec<_> = placed.texts.iter().flat_map(|placed| &placed.layout.runs).collect();
    assert!(!runs.is_empty());
    assert!(runs.iter().all(|run| run.face.family == "Ubuntu" && !run.glyphs.is_empty() && run.glyphs.iter().all(|glyph| glyph.0 != 0)));

    let bytes = export(&p, &opts(PageLayout::Slides)).unwrap();
    assert_eq!(page_text(&bytes, 0), text);
}

/// The text of page `index`, read the way a PDF reader does: the content stream's text-showing
/// operators decoded through the page font's ToUnicode map.
fn page_text(bytes: &[u8], index: usize) -> String {
    use hayro_syntax::content::ops::TypedInstruction;
    use hayro_syntax::object::{Dict, Object, Stream};

    let pdf = hayro_syntax::Pdf::new(bytes.to_vec()).unwrap();
    let page = &pdf.pages()[index];
    let resources = page.raw().get::<Dict<'_>>(b"Resources").unwrap();
    let fonts = resources.get::<Dict<'_>>(b"Font").unwrap();
    assert_eq!(fonts.len(), 1);
    let font = fonts.get::<Dict<'_>>(fonts.keys().next().unwrap()).unwrap();
    let cmap = font.get::<Stream<'_>>(b"ToUnicode").unwrap().decoded().unwrap().into_owned();
    let cmap = std::str::from_utf8(&cmap).unwrap();
    // This fixture uses the writer's two-byte CIDs and bfchar mappings. Decode the
    // content's text-showing operators through that map, not document metadata.
    let mut mapping = HashMap::<u16, String>::new();
    let mut in_char_map = false;
    for line in cmap.lines() {
        if line.ends_with("beginbfchar") {
            in_char_map = true;
        } else if line == "endbfchar" {
            in_char_map = false;
        } else if in_char_map {
            let parts: Vec<_> = line.split_whitespace().collect();
            let cid = u16::from_str_radix(parts[0].trim_matches(['<', '>']), 16).unwrap();
            let value = parts[1].trim_matches(['<', '>']);
            let units: Vec<_> = value
                .as_bytes()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| u16::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap())
                .collect();
            mapping.insert(cid, String::from_utf16(&units).unwrap());
        }
    }
    let mut extracted = String::new();
    let mut append = |bytes: &[u8]| {
        assert!(bytes.len().is_multiple_of(2));
        for pair in bytes.as_chunks::<2>().0 {
            extracted.push_str(mapping.get(&u16::from_be_bytes([pair[0], pair[1]])).unwrap());
        }
    };
    let mut operations = page.typed_operations();
    while let Some(operation) = operations.next() {
        match operation {
            TypedInstruction::ShowText(value) => append(value.0.as_bytes()),
            TypedInstruction::ShowTexts(values) => {
                for value in values.0.iter::<Object<'_>>() {
                    if let Object::String(value) = value {
                        append(value.as_bytes());
                    }
                }
            }
            _ => {}
        }
    }
    extracted
}

fn ubuntu_runs(runs: Vec<deckcraft_model::text::Run>) -> TextBody {
    let paragraph = deckcraft_model::text::Paragraph { runs, ..Default::default() };
    let mut body = TextBody { paragraphs: vec![paragraph], ..Default::default() };
    for run in &mut body.paragraphs[0].runs {
        run.props.font = Some("Ubuntu".into());
        run.props.size = Some(20.0);
    }
    body
}

fn deck_with_body(n: usize, body: TextBody) -> Presentation {
    use deckcraft_model::{Shape, ShapeId, Xfrm};
    let mut p = deck(n);
    for slide in &mut p.slides {
        Arc::make_mut(slide).shapes =
            vec![Shape { id: ShapeId(9001), xfrm: Some(Xfrm::new(20.0, 20.0, 920.0, 100.0)), text: Some(body.clone()), ..Default::default() }];
    }
    p
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn slide_number_field_is_recovered_from_the_page_text_layer() {
    use deckcraft_model::text::{Run, RunKind};
    let mut field = Run::new("<#>");
    field.kind = RunKind::Field { field: "slidenum".into() };
    let p = deck_with_body(3, ubuntu_runs(vec![Run::new("Page "), field]));
    let bytes = export(&p, &opts(PageLayout::Slides)).unwrap();
    for (i, want) in ["Page 1", "Page 2", "Page 3"].into_iter().enumerate() {
        assert_eq!(page_text(&bytes, i), want);
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn line_break_keeps_the_text_layer_aligned_with_the_layout() {
    use deckcraft_model::text::{Run, RunKind};
    let mut brk = Run::new("");
    brk.kind = RunKind::Break;
    let p = deck_with_body(1, ubuntu_runs(vec![Run::new("one"), brk, Run::new("two")]));
    let bytes = export(&p, &opts(PageLayout::Slides)).unwrap();
    assert_eq!(page_text(&bytes, 0), "onetwo");
}

/// Notes pages and handouts print on the chosen paper, landscape included, with every slide box
/// on the page and room left for the notes.
#[test]
fn notes_and_handouts_fit_any_paper_and_orientation() {
    let p = deck(3);
    for (id, _, _) in PAPERS {
        for landscape in [false, true] {
            let paper = paper_size(id, landscape).expect("known paper");
            assert_eq!(paper.0 > paper.1, landscape, "{id}");
            let b = notes_slide_box(paper, 16.0 / 9.0);
            assert!(b.x0 >= 0.0 && b.x1 <= paper.0 && b.y1 <= paper.1 * 0.5, "{id} {landscape}: {b:?}");
            assert!((b.width() / b.height() - 16.0 / 9.0).abs() < 1e-6);
            for n in [1, 3, 6, 9] {
                for r in handout_boxes(n, paper, 16.0 / 9.0) {
                    assert!(r.x0 >= 0.0 && r.y0 >= 0.0 && r.x1 <= paper.0 && r.y1 <= paper.1, "{id} {landscape} {n}: {r:?}");
                }
            }
            for layout in [PageLayout::Notes, PageLayout::Handouts { per_page: 6 }] {
                let bytes = export(&p, &PdfOptions { paper, ..opts(layout) }).expect("export");
                let text = String::from_utf8_lossy(&bytes);
                let media = format!("/MediaBox[0 0 {} {}]", fmt(paper.0), fmt(paper.1));
                assert!(text.contains(&media), "{id} {landscape} {layout:?}: no {media}");
            }
        }
    }
    assert_eq!(paper_size("bogus", false), None);
}

fn fmt(v: f64) -> String {
    let s = format!("{:.2}", v as f32);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn ph(kind: deckcraft_model::PhType, x: f64, y: f64, w: f64, h: f64, text: &str) -> deckcraft_model::Shape {
    deckcraft_model::Shape {
        xfrm: Some(deckcraft_model::Xfrm::new(x, y, w, h)),
        ph: Some(deckcraft_model::Placeholder { kind, ..Default::default() }),
        text: Some(TextBody::from_text(text)),
        ..Default::default()
    }
}

#[test]
fn notes_pages_and_handouts_follow_their_masters() {
    use deckcraft_model::PhType;
    let mut p = deck(2);
    let paper = (612.0, 792.0);
    // No masters: the default layout (slide at the top, one-inch side margins).
    let (slide, notes) = notes_frames(&p, paper);
    assert_eq!((slide.x0, slide.y0), (72.0, 54.0));
    assert!(notes.y0 > slide.y1);
    // A notes master moves the slide and the notes, scaled from the notes size to the paper.
    p.notes_size = deckcraft_model::Size::new(612.0, 792.0);
    p.notes_master = Some(Arc::new(deckcraft_model::Master {
        shapes: vec![
            ph(PhType::SlideImage, 100.0, 400.0, 412.0, 300.0, ""),
            ph(PhType::Body, 50.0, 50.0, 500.0, 300.0, ""),
            ph(PhType::Header, 0.0, 0.0, 300.0, 30.0, "Q3 review"),
            ph(PhType::Date, 312.0, 0.0, 300.0, 30.0, ""),
            ph(PhType::SlideNum, 312.0, 760.0, 300.0, 30.0, "<#>"),
        ],
        ..Default::default()
    }));
    let (slide, notes) = notes_frames(&p, paper);
    assert!(slide.y0 >= 400.0 && slide.y1 <= 700.0 + 1e-6 && slide.x0 >= 100.0 - 1e-6, "{slide:?}");
    assert!(((slide.width() / slide.height()) - p.slide_size.width / p.slide_size.height).abs() < 1e-6);
    assert_eq!(notes, Rect::new(50.0, 50.0, 550.0, 350.0));
    let texts = master_texts(&p, p.notes_master.as_deref().unwrap(), paper, 7);
    let got: Vec<(&str, bool)> = texts.iter().map(|(_, t, r, _)| (t.as_str(), *r)).collect();
    assert_eq!(got, [("Q3 review", false), ("7", true)], "empty date left out, number is the page");
    // Notes pages and handouts export with the masters.
    for layout in [PageLayout::Notes, PageLayout::Handouts { per_page: 3 }] {
        if layout != PageLayout::Notes {
            p.handout_master = Some(Arc::new(deckcraft_model::Master {
                shapes: vec![ph(PhType::Footer, 0.0, 760.0, 300.0, 30.0, "Confidential")],
                ..Default::default()
            }));
        }
        let bytes = export(&p, &PdfOptions { layout, ..Default::default() }).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
    }
}

#[test]
fn master_text_keeps_its_own_formatting() {
    use deckcraft_model::text::RunProps;
    use deckcraft_model::{ColorRef, Fill, PhType, Rgba};
    let mut p = deck(1);
    p.notes_size = deckcraft_model::Size::new(612.0, 792.0);
    let mut header = ph(PhType::Header, 0.0, 0.0, 300.0, 30.0, "Board pack");
    if let Some(run) = header.text.as_mut().and_then(|t| t.paragraphs.first_mut()).and_then(|p| p.runs.first_mut()) {
        run.props = RunProps {
            size: Some(20.0),
            bold: Some(true),
            fill: Some(Fill::solid(ColorRef::rgb(Rgba::rgb(200, 0, 0)))),
            font: Some("+mj-lt".into()),
            ..Default::default()
        };
    }
    let m = deckcraft_model::Master { shapes: vec![header, ph(PhType::Footer, 0.0, 760.0, 300.0, 30.0, "plain")], ..Default::default() };
    let texts = master_texts(&p, &m, (612.0, 792.0), 1);
    let st = &texts[0].3;
    assert_eq!((st.size, st.bold, st.italic, st.color), (20.0, true, false, Rgba::rgb(200, 0, 0)));
    assert_eq!(st.family, m.theme.font("+mj-lt"), "theme font resolved");
    // Unstyled text falls back through the master's styles; sizes scale with the paper.
    let half = master_texts(&p, &m, (306.0, 396.0), 1);
    assert!((half[0].3.size - 10.0).abs() < 1e-9);
    assert!(texts[1].3.size > 0.0 && !texts[1].3.bold);
    p.notes_master = Some(Arc::new(m));
    assert!(export(&p, &PdfOptions { layout: PageLayout::Notes, ..Default::default() }).unwrap().starts_with(b"%PDF"));
}

#[test]
fn master_text_wraps_and_keeps_mixed_formatting() {
    use deckcraft_model::text::{Run, RunProps};
    use deckcraft_model::{PhType, Rgba};
    let mut p = deck(1);
    p.notes_size = deckcraft_model::Size::new(612.0, 792.0);
    let mut header = ph(PhType::Header, 0.0, 0.0, 200.0, 80.0, "");
    if let Some(para) = header.text.as_mut().and_then(|t| t.paragraphs.first_mut()) {
        para.runs = vec![
            Run::with("Quarterly board pack ", RunProps { size: Some(18.0), bold: Some(true), ..Default::default() }),
            Run::with(
                "for internal circulation only, please do not forward",
                RunProps {
                    size: Some(9.0),
                    fill: Some(deckcraft_model::Fill::solid(deckcraft_model::ColorRef::rgb(Rgba::rgb(200, 0, 0)))),
                    ..Default::default()
                },
            ),
        ];
    }
    let m = deckcraft_model::Master { shapes: vec![header, ph(PhType::SlideNum, 312.0, 760.0, 300.0, 30.0, "")], ..Default::default() };
    let placed = deckcraft_render::place_master_texts(&p, &m, 4);
    assert_eq!(placed.len(), 2, "header and page number");
    let h = &placed[0].layout;
    assert!(h.lines.len() >= 2, "wraps inside its 200 pt placeholder: {} lines", h.lines.len());
    let sizes: Vec<f64> = h.runs.iter().filter(|r| !r.glyphs.is_empty()).map(|r| r.size).collect();
    assert!(sizes.iter().any(|s| (*s - 18.0).abs() < 0.5) && sizes.iter().any(|s| (*s - 9.0).abs() < 0.5), "{sizes:?}");
    assert!(h.runs.iter().any(|r| r.color == Rgba::rgb(200, 0, 0)));
    let num: String = placed[1].body.text();
    assert_eq!(num, "4");
    // The same layout as pieces for the Notes Page view: on more than one line, scaled to paper.
    let pieces = master_text_pieces(&p, &m, (306.0, 396.0), 4);
    let mut ys: Vec<i64> = pieces.iter().map(|x| x.at.1.round() as i64).collect();
    ys.dedup();
    assert!(ys.len() >= 3, "two header lines and the number: {pieces:?}");
    assert!(pieces.iter().any(|x| x.bold && (x.size - 9.0).abs() < 0.5), "18 pt bold at half size");
    assert!(pieces.iter().any(|x| x.color == Rgba::rgb(200, 0, 0) && !x.bold));
    assert!(pieces.iter().any(|x| x.text == "4"));
    p.notes_master = Some(Arc::new(m));
    assert!(export(&p, &PdfOptions { layout: PageLayout::Notes, ..Default::default() }).unwrap().starts_with(b"%PDF"));
}

/// Slides that share their layout store their backdrop raster once in the exported PDF, over which
/// each slide's own shapes draw as their own raster (#845).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn slides_share_one_backdrop_raster_and_keep_their_pixels() {
    use deckcraft_model::{Shape, ShapeId, Xfrm};
    use hayro_syntax::object::{Dict, Number, Stream};

    /// The image XObjects the export stores, one entry per distinct object: the pages that draw
    /// it, its filters, color channel, alpha channel (the SMask) and pixel size.
    fn images(p: &Presentation) -> Vec<(Vec<usize>, Vec<hayro_syntax::Filter>, Vec<u8>, Option<Vec<u8>>, (u32, u32))> {
        let bytes = export(p, &opts(PageLayout::Slides)).expect("export");
        let pdf = hayro_syntax::Pdf::new(bytes).expect("pdf");
        let mut out: Vec<(String, Vec<usize>, Vec<hayro_syntax::Filter>, Vec<u8>, Option<Vec<u8>>, (u32, u32))> = Vec::new();
        for (n, page) in pdf.pages().iter().enumerate() {
            let resources = page.raw().get::<Dict>(b"Resources").expect("resources");
            let xobjects = resources.get::<Dict>(b"XObject").expect("xobjects");
            for key in xobjects.keys() {
                let stream = xobjects.get::<Stream>(key).expect("stream");
                let id = format!("{:?}", stream.obj_id());
                match out.iter_mut().find(|(seen, ..)| *seen == id) {
                    Some(entry) => entry.1.push(n),
                    None => {
                        let dict = stream.dict();
                        let alpha = dict.get::<Stream>(b"SMask").map(|mask| mask.decoded().expect("decoded").into_owned());
                        let size = (
                            dict.get::<Number>(b"Width").expect("width").as_f64() as u32,
                            dict.get::<Number>(b"Height").expect("height").as_f64() as u32,
                        );
                        out.push((id, vec![n], stream.filters().into_vec(), stream.decoded().expect("decoded").into_owned(), alpha, size));
                    }
                }
            }
        }
        out.into_iter().map(|(_, pages, filters, rgb, alpha, size)| (pages, filters, rgb, alpha, size)).collect()
    }

    // The raster scale the exporter uses for one slide per page.
    let scale = |p: &Presentation| (p.slide_size.width / 72.0 * opts(PageLayout::Slides).dpi) / p.slide_size.width;

    // Three slides without shapes: every page is pure backdrop and shares one raster, which is
    // exactly what the renderer draws.
    let mut p = deck(3);
    for slide in &mut p.slides {
        Arc::make_mut(slide).shapes.clear();
    }
    let stored = images(&p);
    assert_eq!(stored.len(), 1, "all three pages must share one backdrop raster");
    let (pages, filters, rgb, alpha, size) = stored.into_iter().next().expect("one image");
    assert_eq!(pages, [0, 1, 2]);
    assert_eq!(filters, [hayro_syntax::Filter::FlateDecode], "backdrop artwork stays lossless");
    assert!(alpha.is_none(), "the backdrop is opaque and needs no alpha mask");
    let expect = deckcraft_render::render_slide(&p, 0, &RenderOpts { scale: scale(&p), threads: 4, ..Default::default() });
    assert_eq!(size, (expect.width, expect.height));
    let want: Vec<u8> = expect.pixels.as_chunks::<4>().0.iter().flat_map(|px| px[..3].iter().copied()).collect();
    assert_eq!(rgb, want, "the shared raster is exactly what the renderer draws");

    // Slides with different text still share the backdrop: one raster on every page plus one per
    // slide's own shapes.
    let stored = images(&deck(3));
    assert_eq!(stored.len(), 4, "one shared backdrop plus three content rasters");
    assert_eq!(stored.iter().filter(|(pages, ..)| pages.len() == 3).count(), 1, "the backdrop is the only raster every page draws");

    // A slide's own shapes draw over the shared backdrop as a second raster, stored as straight
    // RGBA (color channel plus alpha mask), never the renderer's premultiplied pixels.
    let mut p = deck(1);
    Arc::make_mut(&mut p.slides[0]).shapes.push(Shape {
        id: ShapeId(9002),
        xfrm: Some(Xfrm::new(20.0, 20.0, 400.0, 300.0)),
        fill: Some(deckcraft_model::Fill::solid(deckcraft_model::ColorRef::rgb(deckcraft_model::Rgba::rgb(200, 30, 30)))),
        ..Default::default()
    });
    let stored = images(&p);
    assert_eq!(stored.len(), 2, "shared backdrop plus the slide's content");
    let (_, filters, rgb, alpha, _) = stored.into_iter().find(|(_, _, _, alpha, _)| alpha.is_some()).expect("content raster");
    assert_eq!(filters, [hayro_syntax::Filter::FlateDecode]);
    let expect = deckcraft_render::render_slide_part(
        &p,
        0,
        deckcraft_render::SlidePart::Content,
        &RenderOpts { scale: scale(&p), threads: 4, ..Default::default() },
    );
    let mut straight = expect.to_straight();
    // Fully transparent pixels carry the backdrop colour, not black (#1455).
    let mut back = deckcraft_render::render_slide_part(
        &p,
        0,
        deckcraft_render::SlidePart::Backdrop,
        &RenderOpts { scale: scale(&p), threads: 4, ..Default::default() },
    );
    opaque(&mut back.pixels);
    for (px, b) in straight.as_chunks_mut::<4>().0.iter_mut().zip(back.pixels.as_chunks::<4>().0) {
        if px[3] == 0 {
            px[..3].copy_from_slice(&b[..3]);
        }
    }
    let want_rgb: Vec<u8> = straight.as_chunks::<4>().0.iter().flat_map(|px| px[..3].iter().copied()).collect();
    let want_alpha: Vec<u8> = straight.as_chunks::<4>().0.iter().map(|px| px[3]).collect();
    assert_eq!(rgb, want_rgb, "content colors are straight, not premultiplied");
    assert_eq!(alpha, Some(want_alpha), "content alpha keeps the shape's coverage");

    // A backdrop that comes back after another one is still stored once, and every page draws its
    // own slide's backdrop.
    let mut p = deck(4);
    for (i, slide) in p.slides.iter_mut().enumerate() {
        let s = Arc::make_mut(slide);
        s.shapes.clear();
        let c = if i % 2 == 0 { deckcraft_model::Rgba::rgb(20, 90, 160) } else { deckcraft_model::Rgba::rgb(230, 230, 210) };
        s.background = Some(deckcraft_model::Background::Fill { fill: deckcraft_model::Fill::solid(deckcraft_model::ColorRef::rgb(c)) });
    }
    let stored = images(&p);
    assert_eq!(stored.iter().map(|(pages, ..)| pages.clone()).collect::<Vec<_>>(), [vec![0, 2], vec![1, 3]]);
    for (pages, _, rgb, alpha, _) in &stored {
        let expect = deckcraft_render::render_slide(&p, pages[0], &RenderOpts { scale: scale(&p), threads: 4, ..Default::default() });
        let want: Vec<u8> = expect.pixels.as_chunks::<4>().0.iter().flat_map(|px| px[..3].iter().copied()).collect();
        assert_eq!(rgb, &want, "pages {pages:?} draw their own backdrop");
        assert!(alpha.is_none());
    }
}

/// The block-skipping pixel conversions give exactly the per-pixel formulas, in blocks that skip
/// and blocks that don't, and in a tail shorter than a block.
#[test]
fn backdrop_and_content_pixels_convert_per_pixel() {
    let mut px: Vec<u8> = Vec::new();
    for a in 0..=255u8 {
        for c in 0..=255u8 {
            px.extend([c, c / 2, 255 - c, a]);
        }
    }
    px.extend([0, 0, 0, 0].repeat(32));
    px.extend([9, 8, 7, 255].repeat(32));
    px.extend([1, 2, 3, 128, 4, 5, 6, 0]);
    let (mut o, mut s) = (px.clone(), px.clone());
    opaque(&mut o);
    straight(&mut s);
    for ((p, o), s) in px.chunks(4).zip(o.chunks(4)).zip(s.chunks(4)) {
        let a = p[3] as u32;
        for k in 0..3 {
            assert_eq!(o[k] as u32, (p[k] as u32 + 255 - a).min(255), "{p:?}");
            let want = if a == 0 || a == 255 { p[k] as u32 } else { ((p[k] as u32 * 255 + a / 2) / a).min(255) };
            assert_eq!(s[k] as u32, want, "{p:?}");
        }
        assert_eq!((o[3], s[3]), (255, p[3]));
    }
}

/// Fully transparent content pixels take the backdrop colour (not black) so a smoothing viewer
/// draws no hairline at the edge of a picture; every other pixel is untouched (#1455).
#[test]
fn transparent_content_pixels_take_the_backdrop_colour() {
    let mut front = vec![0, 0, 0, 0, 10, 20, 30, 255, 5, 5, 5, 1, 0, 0, 0, 0];
    fill_clear(&mut front, &[255, 250, 240, 255].repeat(4));
    assert_eq!(front, [255, 250, 240, 0, 10, 20, 30, 255, 5, 5, 5, 1, 255, 250, 240, 0]);
}

#[test]
fn glyph_ranges_follow_shaping_clusters() {
    // "fl" ligature: its glyph maps to both characters, the rest stay one to one (#164).
    assert_eq!(glyph_text_ranges("flash", (0, 5), &[(0, 'f'), (2, 'a'), (3, 's'), (4, 'h')], 4), vec![0..2, 2..3, 3..4, 4..5]);
    // Two glyphs in one cluster (base and mark) share its range.
    assert_eq!(glyph_text_ranges("ab", (0, 2), &[(0, 'a'), (0, 'a'), (1, 'b')], 3), vec![0..1, 0..1, 1..2]);
    // A run starting mid-paragraph, multi-byte characters in visual (right-to-left) order.
    assert_eq!(glyph_text_ranges("αβγ", (3, 6), &[(5, 'γ'), (4, 'β'), (3, 'α')], 3), vec![4..6, 2..4, 0..2]);
}

#[test]
fn raster_scale_caps_total_pixels_not_each_side() {
    // A huge square slide at 600 dpi would be 16,000 x 16,000 px (~1 GB RGBA) without the cap.
    let s = raster_scale(20_000.0, (20_000.0, 20_000.0), 600.0);
    let px = 20_000.0 * s * 20_000.0 * s;
    assert!(px <= MAX_RASTER_PIXELS * 1.0001, "{px}");
    // A normal slide keeps the requested dpi.
    let s = raster_scale(720.0, (960.0, 540.0), 200.0);
    assert!((s - 720.0 / 72.0 * 200.0 / 960.0).abs() < 1e-12);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn huge_paper_and_long_notes_export_in_bounded_work() {
    // At 1e19 pt adding a rule's 22 pt leaves y unchanged: the rules are counted, not stepped. A narrow page keeps the raster small.
    let p = deck(1);
    let bytes = export(&p, &PdfOptions { paper: (612.0, 1.0e19), ..opts(PageLayout::Handouts { per_page: 3 }) }).expect("export");
    assert!(bytes.starts_with(b"%PDF-"));
    // 20,000 short words of notes, wrapped with the line width kept as words are added.
    let mut p = deck(1);
    Arc::make_mut(&mut p.slides[0]).notes = TextBody::from_text(&"ab ".repeat(20_000));
    let bytes = export(&p, &opts(PageLayout::Notes)).expect("export");
    assert!(bytes.starts_with(b"%PDF-"));
}
