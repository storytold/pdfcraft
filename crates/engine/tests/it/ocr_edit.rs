//! Edit text on a scanned page: rewriting an OCR word (invisible text over the scan) shows the
//! new text and hides the scanned word. Synthetic image only.

use pdfcraft_engine::ocr::PlacedWord;
use pdfcraft_engine::{Edit, Session, export};
use pdfcraft_render::{PageRenderer, RenderRequest};

const PAPER: [u8; 3] = [230, 225, 200];
const INK: [u8; 3] = [20, 30, 90];
/// The scanned "word": a block of ink, in image pixels (72 dpi, so points), y down.
const WORD: [u32; 4] = [40, 40, 100, 60];

/// A 200 × 100 pt "scan" on tinted paper, with OCR text over its word.
fn scanned() -> (Session, pdfcraft_engine::DocId) {
    scanned_with(vec![PlacedWord { text: "John".into(), origin: [40.0, 40.0], across: [60.0, 0.0], up: [0.0, 20.0] }])
}

fn scanned_with(words: Vec<PlacedWord>) -> (Session, pdfcraft_engine::DocId) {
    let (w, h) = (200u32, 100u32);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let ink = (WORD[0]..WORD[2]).contains(&x) && (WORD[1]..WORD[3]).contains(&y);
            rgba.extend(if ink { INK } else { PAPER });
            rgba.push(255);
        }
    }
    let png = export::encode_png(w, h, &rgba).unwrap();
    let mut s = Session::new();
    let bytes = s.create_from_images(&[("scan.png".into(), png)]).unwrap();
    let id = s.open("scan.pdf", None, bytes, None).unwrap();
    s.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
    (s, id)
}

fn word_pixels(s: &Session, id: pdfcraft_engine::DocId) -> Vec<[u8; 3]> {
    let doc = s.get(id).unwrap();
    let out = PageRenderer::new(doc.bytes.clone(), Default::default()).render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let mut px = Vec::new();
    for y in WORD[1] + 2..WORD[3] - 2 {
        for x in WORD[0] + 2..WORD[2] - 2 {
            let i = ((y * out.width + x) * 4) as usize;
            px.push([out.rgba[i], out.rgba[i + 1], out.rgba[i + 2]]);
        }
    }
    px
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24)
}

#[test]
fn editing_an_ocr_word_covers_the_scan_and_shows_the_new_text() {
    let (mut s, id) = scanned();
    let blocks = s.get(id).unwrap().text_blocks(0);
    assert_eq!(blocks.len(), 1);
    assert!(blocks[0].invisible, "the OCR layer is invisible text");
    assert!(word_pixels(&s, id).iter().all(|p| near(*p, INK)), "the scanned word fills its box");

    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Jane".into(), style: Default::default() }).unwrap();

    let blocks = s.get(id).unwrap().text_blocks(0);
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Jane"]);
    assert!(!blocks[0].invisible, "the new text is drawn");
    let px = word_pixels(&s, id);
    let paper = px.iter().filter(|p| near(**p, PAPER)).count();
    let ink = px.iter().filter(|p| near(**p, INK)).count();
    // The scanned word is covered in the paper's colour; the new letters are in the ink's.
    assert!(paper * 10 > px.len() * 4, "mostly paper now: {paper} of {}", px.len());
    assert!(ink * 100 > px.len() * 3, "the new text shows in the scan's ink colour: {ink} of {}", px.len());
}

#[test]
fn an_explicit_colour_is_kept_for_the_new_text() {
    let (mut s, id) = scanned();
    let style = pdfcraft_engine::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Jane".into(), style }).unwrap();
    let px = word_pixels(&s, id);
    assert!(px.iter().any(|p| p[0] > 200 && p[1] < 60 && p[2] < 60), "red text");
    assert!(!px.iter().any(|p| near(*p, INK)), "no scanned ink is left");
}

/// A word as OCR boxes it on a scan set at `size` on `baseline`: tight around its ink, from the
/// descender line when it has descenders, up to the cap height.
fn scanned_word(text: &str, x0: f64, x1: f64, baseline: f64, size: f64) -> PlacedWord {
    let low = if text.chars().any(|c| "gjpqy".contains(c)) { 0.21 * size } else { 0.0 };
    PlacedWord { text: text.into(), origin: [x0, baseline - low], across: [x1 - x0, 0.0], up: [0.0, 0.718 * size + low] }
}

fn word(text: &str, x0: f64, x1: f64, baseline: f64) -> PlacedWord {
    PlacedWord { text: text.into(), origin: [x0, baseline], across: [x1 - x0, 0.0], up: [0.0, 10.0] }
}

/// OCR words keep the scan's layout: a phrase is one paragraph, a value in the next table cell and
/// the line below are others, so rewriting one never moves the rest.
#[test]
fn ocr_words_group_into_phrases_that_edit_in_place() {
    let (mut s, id) = scanned_with(vec![
        word("EMPLOYEE", 10.0, 60.0, 70.0),
        word("NAME", 64.0, 90.0, 70.0),
        word("John", 120.0, 145.0, 70.0),
        word("Smith", 120.0, 148.0, 56.0),
    ]);
    let texts = |s: &Session| s.get(id).unwrap().text_blocks(0).iter().map(|b| b.text.clone()).collect::<Vec<_>>();
    assert_eq!(texts(&s), ["EMPLOYEE NAME", "John", "Smith"]);
    let john = s.get(id).unwrap().text_blocks(0)[1].rect;
    s.apply(id, Edit::EditTextBlock { page: 0, block: 1, text: "Jane".into(), style: Default::default() }).unwrap();
    assert_eq!(texts(&s), ["EMPLOYEE NAME", "Jane", "Smith"]);
    let blocks = s.get(id).unwrap().text_blocks(0);
    assert!(blocks[0].invisible && blocks[2].invisible, "the other OCR words are left alone");
    assert!(!blocks[1].invisible);
    assert!((blocks[1].rect[0] - john[0]).abs() < 1.0, "the new word starts where the old one did: {:?} vs {john:?}", blocks[1].rect);
}

/// Several words of one OCR phrase rewrite as one line, without wrapping onto the line below.
#[test]
fn a_longer_ocr_phrase_grows_rightwards() {
    let (mut s, id) = scanned_with(vec![
        scanned_word("Project", 20.0, 55.0, 70.0, 11.0),
        scanned_word("Lead", 59.0, 80.0, 70.0, 11.0),
        scanned_word("Testing", 20.0, 56.0, 56.0, 11.0),
    ]);
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Project Coordinator Lead".into(), style: Default::default() }).unwrap();
    let blocks = s.get(id).unwrap().text_blocks(0);
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Project Coordinator Lead", "Testing"]);
    assert_eq!(blocks[0].rows.len(), 1, "one line");
}

/// OCR words differ in size and are stretched to fit the scan: a rewritten phrase keeps each
/// word's width, so the words don't run into one another.
#[test]
fn a_rewritten_ocr_phrase_keeps_its_word_widths() {
    let (mut s, id) = scanned_with(vec![
        scanned_word("John", 20.0, 45.0, 70.0, 10.5),
        scanned_word("Smith", 49.0, 78.0, 70.0, 11.0),
        scanned_word("Example", 82.0, 124.0, 70.0, 11.5),
    ]);
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smith Exam".into(), style: Default::default() }).unwrap();
    let lines = s.get(id).unwrap().text_lines(0);
    assert_eq!(lines.iter().map(|l| l.text.trim()).collect::<Vec<_>>(), ["John", "Smith", "Exam"]);
    assert!((lines[0].rect[2] - 45.0).abs() < 0.5, "John keeps its width: {:?}", lines[0].rect);
    for pair in lines.windows(2) {
        assert!(pair[1].rect[0] >= pair[0].rect[2] - 0.5, "{:?} runs into {:?}", pair[1].text, pair[0].text);
    }
}

/// Recognize text sizes each word from its box and a guess at how much of the em its letters fill,
/// so one phrase's words come out several points apart (#3313). Here the first word is the
/// largest and the middle one the smallest; the phrase is set in 11 pt.
fn uneven_phrase() -> (Session, pdfcraft_engine::DocId) {
    scanned_with(vec![
        scanned_word("John", 20.0, 45.0, 70.0, 15.0),
        scanned_word("Smith", 49.0, 78.0, 70.0, 9.0),
        scanned_word("Example", 82.0, 124.0, 70.0, 11.0),
    ])
}

/// An edited OCR phrase is redrawn in one size, the median of its words', and each word keeps
/// the width the scan shows it at (#3313).
#[test]
fn an_edited_ocr_phrase_is_drawn_at_one_size() {
    let (mut s, id) = uneven_phrase();
    let block = s.get(id).unwrap().text_blocks(0).remove(0);
    assert!((block.size - 11.0).abs() < 0.05, "the phrase's size is its words' median: {}", block.size);

    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Example".into(), style: Default::default() }).unwrap();
    let lines = s.get(id).unwrap().text_lines(0);
    assert_eq!(lines.iter().map(|l| l.text.trim()).collect::<Vec<_>>(), ["John", "Example"]);
    for line in &lines {
        assert!((line.size - 11.0).abs() < 0.05, "{:?} is drawn at {} pt, not the phrase's 11", line.text, line.size);
    }
    // Each word keeps its width; the rest of the phrase closes up where Smith was.
    assert!((lines[0].rect[2] - lines[0].rect[0] - 25.0).abs() < 0.5, "John keeps its width: {:?}", lines[0].rect);
    assert!((lines[1].rect[2] - lines[1].rect[0] - 42.0).abs() < 0.5, "Example keeps its width: {:?}", lines[1].rect);
    assert!(lines[1].rect[0] >= lines[0].rect[2] - 0.5, "Example runs into John");
}

/// Choosing a font sends the phrase through the single-style rewrite, which takes its size from
/// the first run: that is the phrase's size too, not the first word's guess.
#[test]
fn a_new_font_for_an_ocr_phrase_keeps_its_one_size() {
    let (mut s, id) = uneven_phrase();
    let style = pdfcraft_engine::BlockStyle { family: Some((pdfcraft_engine::FontFamily::Times, false, false)), ..Default::default() };
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smith Example".into(), style }).unwrap();
    let lines = s.get(id).unwrap().text_lines(0);
    assert!(!lines.is_empty());
    for line in &lines {
        assert!((line.size - 11.0).abs() < 0.05, "{:?} is drawn at {} pt, not the phrase's 11", line.text, line.size);
    }
}

/// Two neighbouring words OCR sized alike join into one run; the median still counts each word,
/// so 15, 9 and 9 pt make a 9 pt phrase, not the 12 pt of its two runs.
#[test]
fn words_joined_into_one_run_each_count_toward_the_phrase_size() {
    let (s, id) = scanned_with(vec![
        scanned_word("John", 20.0, 45.0, 70.0, 15.0),
        scanned_word("SMITH", 49.0, 78.0, 70.0, 9.0),
        scanned_word("JONES", 82.0, 112.0, 70.0, 9.0),
    ]);
    let doc = s.get(id).unwrap();
    assert_eq!(doc.text_lines(0).len(), 2, "the two 9 pt words share a run");
    let size = doc.text_blocks(0)[0].size;
    assert!((size - 9.0).abs() < 0.05, "the phrase is set in its words' median, 9 pt: {size}");
}

/// A word OCR sized at more than twice the phrase's size still keeps the width it has on the scan.
#[test]
fn a_word_far_larger_than_its_phrase_keeps_its_width() {
    let (mut s, id) = scanned_with(vec![
        scanned_word("John", 20.0, 45.0, 70.0, 25.0),
        scanned_word("Smith", 49.0, 78.0, 70.0, 10.0),
        scanned_word("Example", 82.0, 124.0, 70.0, 10.5),
    ]);
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smith Exam".into(), style: Default::default() }).unwrap();
    let lines = s.get(id).unwrap().text_lines(0);
    assert_eq!(lines[0].text.trim(), "John");
    assert!((lines[0].rect[2] - lines[0].rect[0] - 25.0).abs() < 0.5, "John keeps its width: {:?}", lines[0].rect);
}

/// With a new font the phrase is one run in its first word's text state. How wide it is must not
/// depend on how far OCR misjudged that word's size: the same boxes give the same line.
#[test]
fn a_new_font_for_an_ocr_phrase_is_as_wide_whatever_its_first_word_was_sized() {
    let width = |first: f64| {
        let (mut s, id) = scanned_with(vec![
            scanned_word("John", 20.0, 45.0, 70.0, first),
            scanned_word("Smith", 49.0, 78.0, 70.0, 11.0),
            scanned_word("Example", 82.0, 124.0, 70.0, 11.0),
        ]);
        let style = pdfcraft_engine::BlockStyle { family: Some((pdfcraft_engine::FontFamily::Helvetica, false, false)), ..Default::default() };
        // Short enough not to wrap in the phrase's box either way.
        s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smith".into(), style }).unwrap();
        let lines = s.get(id).unwrap().text_lines(0);
        assert_eq!(lines.len(), 1, "one line: {lines:?}");
        lines.iter().map(|l| l.rect[2]).fold(f64::MIN, f64::max) - lines.iter().map(|l| l.rect[0]).fold(f64::MAX, f64::min)
    };
    let (guessed, right) = (width(15.0), width(11.0));
    assert!((guessed - right).abs() < 0.5, "a 15 pt guess for John makes the line {guessed} wide, an 11 pt one {right}");
}

/// Visible text set with a horizontally stretched text matrix (3 × 1), as some generators write.
fn stretched_visible() -> std::sync::Arc<Vec<u8>> {
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream};
    let mut d = Document::new_empty();
    let pages = d.add(Object::Dict(Dict::new()));
    let contents = d.add(Object::Stream(Stream::flate(Dict::new(), b"BT /F1 12 Tf 3 0 0 1 20 60 Tm (Source) Tj ET")));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Dict(font));
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"Contents".to_vec(), Object::Ref(contents));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"MediaBox".to_vec(), Object::Array([0, 0, 400, 120].map(Object::Int).to_vec()));
    let page = d.add(Object::Dict(p));
    d.update_dict(pages, |p| {
        p.set(b"Type".to_vec(), Object::name("Pages"));
        p.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        p.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    let mut root = Dict::new();
    root.set(b"Type".to_vec(), Object::name("Catalog"));
    root.set(b"Pages".to_vec(), Object::Ref(pages));
    let root = d.add(Object::Dict(root));
    d.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    std::sync::Arc::new(pdfcraft_cos::write_full(&d, &SaveOptions::default()).unwrap())
}

/// Visible text is not OCR: a styled rewrite of stretched text keeps its width, draws no cover
/// and stays visible.
#[test]
fn visible_stretched_text_rewrites_unchanged() {
    let mut s = Session::new();
    let id = s.open("stretched.pdf", None, stretched_visible(), None).unwrap();
    let before = s.get(id).unwrap().text_blocks(0);
    assert_eq!(before.len(), 1);
    assert!(!before[0].invisible);
    let style = pdfcraft_engine::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Source".into(), style }).unwrap();
    let after = s.get(id).unwrap().text_blocks(0);
    assert_eq!(after.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Source"]);
    assert!(!after[0].invisible);
    let (w0, w1) = (before[0].rect[2] - before[0].rect[0], after[0].rect[2] - after[0].rect[0]);
    assert!((w0 - w1).abs() < 1.0, "the stretched word keeps its width: {w0} → {w1}");
    assert!((before[0].rect[0] - after[0].rect[0]).abs() < 0.5);
    // No paper-coloured cover: outside the glyphs the page is still white.
    let doc = s.get(id).unwrap();
    let out = PageRenderer::new(doc.bytes.clone(), Default::default()).render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none());
    let r = after[0].rect;
    let (x, y) = ((r[0] - 1.0).max(0.0) as u32, (120.0 - (r[1] + r[3]) / 2.0) as u32);
    let i = ((y * out.width + x) * 4) as usize;
    assert_eq!(&out.rgba[i..i + 3], &[255, 255, 255]);
}
