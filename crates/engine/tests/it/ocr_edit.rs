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
    scanned_with(vec![PlacedWord {
        text: "John".into(),
        origin: [40.0, 40.0],
        across: [60.0, 0.0],
        up: [0.0, 20.0],
        confidence: None,
        source: "ocrs".into(),
    }])
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
    PlacedWord {
        text: text.into(),
        origin: [x0, baseline - low],
        across: [x1 - x0, 0.0],
        up: [0.0, 0.718 * size + low],
        confidence: None,
        source: "ocrs".into(),
    }
}

fn word(text: &str, x0: f64, x1: f64, baseline: f64) -> PlacedWord {
    PlacedWord { text: text.into(), origin: [x0, baseline], across: [x1 - x0, 0.0], up: [0.0, 10.0], confidence: None, source: "ocrs".into() }
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

/// Words added to an OCR phrase grow rightwards as one line, without wrapping onto the line below.
#[test]
fn a_longer_ocr_phrase_grows_rightwards() {
    let (mut s, id) = scanned_with(vec![
        scanned_word("Project", 20.0, 55.0, 70.0, 11.0),
        scanned_word("Lead", 59.0, 80.0, 70.0, 11.0),
        scanned_word("Testing", 20.0, 56.0, 56.0, 11.0),
    ]);
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Project Coordinator Lead".into(), style: Default::default() }).unwrap();
    let blocks = s.get(id).unwrap().text_blocks(0);
    assert_eq!(blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Project", "Coordinator Lead", "Testing"]);
    assert_eq!(blocks[1].rows.len(), 1, "one line");
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
    // Every word changes, so every word is redrawn. "Jahn" is as wide as "John" in Helvetica.
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Jahn Smyth Exam".into(), style: Default::default() }).unwrap();
    let lines = s.get(id).unwrap().text_lines(0);
    assert_eq!(lines.iter().map(|l| l.text.trim()).collect::<Vec<_>>(), ["Jahn Smyth Exam"]);
    let ink = &lines[0].ink;
    assert!(ink.len() == 3, "three words: {ink:?}");
    assert!((ink[0][2] - 45.0).abs() < 0.5, "Jahn keeps John's width: {ink:?}");
    for pair in ink.windows(2) {
        assert!(pair[1][0] >= pair[0][2] - 0.5, "{pair:?} run into each other");
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

    // Every word changes, so every word is redrawn. "Jahn" is as wide as "John" in Helvetica.
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Jahn Smyth Exbmple".into(), style: Default::default() }).unwrap();
    let lines = s.get(id).unwrap().text_lines(0);
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<String>().replace(' ', ""), "JahnSmythExbmple");
    for line in &lines {
        assert!((line.size - 11.0).abs() < 0.05, "{:?} is drawn at {} pt, not the phrase's 11", line.text, line.size);
    }
    let ink = &lines[0].ink;
    assert!((ink[0][2] - ink[0][0] - 25.0).abs() < 0.5, "Jahn keeps John's width: {ink:?}");
}

/// Only invisible OCR text is set in one phrase size: visible text keeps each run's own size, and
/// a phrase whose size is NaN, zero, negative or infinite falls back to its words' sizes (#3313).
#[test]
fn only_an_ocr_phrase_with_a_usable_size_is_set_in_one_size() {
    let (s, id) = uneven_phrase();
    let block = s.get(id).unwrap().text_blocks(0).remove(0);
    assert!(block.invisible);
    assert_eq!(block.phrase_size(), Some(block.size));
    let visible = pdfcraft_engine::TextBlock { invisible: false, ..block.clone() };
    assert_eq!(visible.phrase_size(), None, "visible text keeps its runs' sizes");
    for size in [f64::NAN, 0.0, -3.0, f64::INFINITY] {
        assert_eq!(pdfcraft_engine::TextBlock { size, ..block.clone() }.phrase_size(), None, "a {size} pt phrase falls back to its words");
    }
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
    // Every word changes, so every word is redrawn. "Jahn" is as wide as "John" in Helvetica.
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Jahn Smyth Exam".into(), style: Default::default() }).unwrap();
    let lines = s.get(id).unwrap().text_lines(0);
    let ink = &lines[0].ink;
    assert!(ink.len() == 3, "three words: {ink:?}");
    assert!((ink[0][2] - ink[0][0] - 25.0).abs() < 0.5, "Jahn keeps John's width: {ink:?}");
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

/// The lines drawn invisibly (still the scan's) and visibly (redrawn), by their text.
fn scan_and_drawn(s: &Session, id: pdfcraft_engine::DocId) -> (Vec<String>, Vec<String>) {
    let blocks = s.get(id).unwrap().text_blocks(0);
    let of = |invisible: bool| blocks.iter().filter(|b| b.invisible == invisible).map(|b| b.text.clone()).collect::<Vec<_>>();
    (of(true), of(false))
}

/// A phrase over the scan's block of ink ([`WORD`]): two words side by side.
fn two_words_on_the_ink() -> (Session, pdfcraft_engine::DocId) {
    scanned_with(vec![scanned_word("John", 42.0, 66.0, 42.0, 18.0), scanned_word("Smith", 70.0, 98.0, 42.0, 18.0)])
}

/// The share of paper among the page's pixels from x0 to x1 inside the scanned word's height.
fn paper_between(s: &Session, id: pdfcraft_engine::DocId, x0: u32, x1: u32) -> f64 {
    let doc = s.get(id).unwrap();
    let out = PageRenderer::new(doc.bytes.clone(), Default::default()).render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let (mut paper, mut all) = (0, 0);
    for y in WORD[1] + 4..WORD[3] - 4 {
        for x in x0..x1 {
            let i = ((y * out.width + x) * 4) as usize;
            paper += usize::from(near([out.rgba[i], out.rgba[i + 1], out.rgba[i + 2]], PAPER));
            all += 1;
        }
    }
    paper as f64 / all as f64
}

/// Deleting a word covers that word and nothing else: the rest of the line is still the scan, and
/// the words after it stay where the scan shows them (#3313).
#[test]
fn deleting_an_ocr_word_leaves_the_rest_of_the_line_as_the_scan() {
    let (mut s, id) = uneven_phrase();
    let before = s.get(id).unwrap().text_lines(0);
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Example".into(), style: Default::default() }).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["John".to_owned(), "Example".to_owned()], Vec::new()), "nothing is redrawn");
    let after = s.get(id).unwrap().text_lines(0);
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].rect, before[0].rect, "John hasn't moved");
    assert_eq!(after[1].rect, before[2].rect, "Example hasn't closed up the gap");
    assert_eq!(after[0].size, before[0].size, "and kept words keep the layer's own sizes");
}

/// On the page: the deleted word's pixels are covered, its neighbour's aren't.
#[test]
fn a_deleted_ocr_word_is_covered_and_its_neighbour_is_not() {
    let (mut s, id) = two_words_on_the_ink();
    assert!(paper_between(&s, id, 44, 64) < 0.02 && paper_between(&s, id, 72, 96) < 0.02, "both words are ink to begin with");
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John".into(), style: Default::default() }).unwrap();
    assert!(paper_between(&s, id, 44, 64) < 0.02, "John is still the scan");
    assert!(paper_between(&s, id, 72, 96) > 0.98, "Smith is covered");
    assert_eq!(s.get(id).unwrap().text_blocks(0).iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["John"], "and gone from the text");
}

/// Replacing a word redraws that word, at the phrase's size and in its place.
#[test]
fn replacing_an_ocr_word_redraws_only_that_word() {
    let (mut s, id) = uneven_phrase();
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smyth Example".into(), style: Default::default() }).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["John".to_owned(), "Example".to_owned()], vec!["Smyth".to_owned()]));
    let lines = s.get(id).unwrap().text_lines(0);
    let smyth = lines.iter().find(|l| l.text == "Smyth").unwrap();
    assert!((smyth.size - 11.0).abs() < 0.05, "at the phrase's size: {}", smyth.size);
    assert!((smyth.rect[0] - 49.0).abs() < 0.5, "where Smith was: {:?}", smyth.rect);
}

/// Changes apart from each other are each their own, with the word between them kept, as one step.
#[test]
fn two_changed_ocr_words_keep_the_word_between_them() {
    let (mut s, id) = uneven_phrase();
    let original = s.get(id).unwrap().bytes.clone();
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Jon Smith Exam".into(), style: Default::default() }).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["Smith".to_owned()], vec!["Jon".to_owned(), "Exam".to_owned()]));
    s.undo(id).unwrap();
    assert_eq!(s.get(id).unwrap().bytes, original, "one undo brings the phrase back");
    s.redo(id).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["Smith".to_owned()], vec!["Jon".to_owned(), "Exam".to_owned()]));
}

/// A replacement a little wider than its word is squeezed into its place, and the next word stays.
#[test]
fn a_slightly_longer_ocr_word_is_squeezed_into_its_place() {
    let (mut s, id) = uneven_phrase();
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smithy Example".into(), style: Default::default() }).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["John".to_owned(), "Example".to_owned()], vec!["Smithy".to_owned()]));
    let lines = s.get(id).unwrap().text_lines(0);
    let smithy = lines.iter().find(|l| l.text == "Smithy").unwrap();
    let example = lines.iter().find(|l| l.text == "Example").unwrap();
    assert!(smithy.rect[2] <= 80.5, "it ends before the middle of the gap: {:?}", smithy.rect);
    assert_eq!(example.rect[0].round(), 82.0, "and Example hasn't moved");
    assert!((smithy.size - 11.0).abs() < 0.05, "squeezed across, not smaller: {}", smithy.size);
}

/// A replacement too wide to squeeze takes over the word it would run into, and no more.
#[test]
fn a_longer_ocr_word_takes_over_the_word_it_would_run_into() {
    let (mut s, id) = scanned_with(vec![
        scanned_word("John", 20.0, 45.0, 70.0, 11.0),
        scanned_word("Smith", 49.0, 78.0, 70.0, 11.5),
        scanned_word("Jones", 82.0, 112.0, 70.0, 12.0),
        scanned_word("Brown", 118.5, 150.0, 70.0, 12.5),
    ]);
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smithsons Jones Brown".into(), style: Default::default() }).unwrap();
    let (scan, drawn) = scan_and_drawn(&s, id);
    assert_eq!(scan, ["John", "Brown"], "the words it doesn't reach stay: {drawn:?}");
    assert_eq!(drawn, ["Smithsons Jones"]);
    let lines = s.get(id).unwrap().text_lines(0);
    let brown = lines.iter().find(|l| l.text == "Brown").unwrap();
    let jones = lines.iter().find(|l| l.text.ends_with("Jones")).unwrap();
    assert!(jones.rect[2] <= brown.rect[0], "{:?} runs into {:?}", jones.rect, brown.rect);
}

/// A word added with nothing to replace redraws a neighbour; the rest stay.
#[test]
fn an_added_ocr_word_redraws_only_a_neighbour() {
    let (mut s, id) = uneven_phrase();
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smith Example again".into(), style: Default::default() }).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["John Smith".to_owned()], vec!["Example again".to_owned()]));
}

/// Words OCR sized alike share a run in the text lines; they are still kept one by one.
#[test]
fn ocr_words_of_one_size_are_kept_one_by_one() {
    let (mut s, id) = scanned_with(vec![
        scanned_word("JOHN", 20.0, 45.0, 70.0, 11.0),
        scanned_word("SMITH", 49.0, 78.0, 70.0, 11.0),
        scanned_word("JONES", 82.0, 112.0, 70.0, 11.0),
    ]);
    assert_eq!(s.get(id).unwrap().text_lines(0).len(), 1, "one run to begin with");
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "JOHN SMYTH JONES".into(), style: Default::default() }).unwrap();
    let (scan, drawn) = scan_and_drawn(&s, id);
    assert_eq!((scan, drawn), (vec!["JOHN".to_owned(), "JONES".to_owned()], vec!["SMYTH".to_owned()]));
}

/// Changing the formatting is about the whole phrase, so all of it is redrawn.
#[test]
fn a_formatting_change_redraws_the_whole_ocr_phrase() {
    let (mut s, id) = uneven_phrase();
    let style = pdfcraft_engine::BlockStyle { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smyth Example".into(), style }).unwrap();
    let (scan, drawn) = scan_and_drawn(&s, id);
    assert!(scan.is_empty(), "nothing is left as the scan: {scan:?}");
    assert_eq!(drawn.join(" "), "John Smyth Example");
}

/// A layer whose words follow one another with no position of their own: a word that went would
/// move those after it, so keeping them isn't possible and the whole phrase is redrawn instead.
#[test]
fn words_that_depend_on_the_one_before_are_redrawn_rather_than_moved() {
    let bytes = under_scan("BT /F1 10 Tf 3 Tr 20 44 Td (John) Tj ( ) Tj (Smith) Tj ( ) Tj (Example) Tj ET");
    let mut s = Session::new();
    let id = s.open("relative.pdf", None, bytes, None).unwrap();
    assert_eq!(texts(&s, id), ["John Smith Example"]);
    edit_block(&mut s, id, 0, "John Example");
    let (scan, drawn) = scan_and_drawn(&s, id);
    assert!(scan.is_empty(), "no word is left to shift under the scan: {scan:?}");
    assert_eq!(drawn, ["John Example"]);
}

/// The edit survives a save and reopen, and each piece can be edited again.
#[test]
fn a_kept_ocr_word_can_be_edited_after_a_save() {
    let (mut s, id) = uneven_phrase();
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "John Smyth Example".into(), style: Default::default() }).unwrap();
    let saved = s.get(id).unwrap().bytes.clone();
    let id = s.open("saved.pdf", None, saved, None).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["John".to_owned(), "Example".to_owned()], vec!["Smyth".to_owned()]));
    let john = s.get(id).unwrap().text_blocks(0).iter().position(|b| b.text == "John").unwrap();
    s.apply(id, Edit::EditTextBlock { page: 0, block: john, text: "Jon".into(), style: Default::default() }).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["Example".to_owned()], vec!["Jon".to_owned(), "Smyth".to_owned()]));
}

/// A draft with the same words, a space typed after the last of them, changes nothing: not the
/// page, and not the undo history. It used to redraw the whole line.
#[test]
fn a_draft_with_the_same_words_changes_nothing() {
    let (mut s, id) = uneven_phrase();
    let (bytes, generation) = (s.get(id).unwrap().bytes.clone(), s.get(id).unwrap().edit_generation());
    for draft in ["John Smith Example ", " John Smith Example"] {
        s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: draft.into(), style: Default::default() }).unwrap();
        assert_eq!(s.get(id).unwrap().bytes, bytes, "{draft:?} wrote nothing");
        assert_eq!(s.get(id).unwrap().edit_generation(), generation, "{draft:?} is no undo step");
        assert_eq!(scan_and_drawn(&s, id).1, Vec::<String>::new(), "{draft:?} drew nothing");
    }
}

/// Visible text positioned after the end of an OCR word depends on that word. An edit may not
/// leave the word gone and the visible text somewhere else while calling the rest kept.
#[test]
fn visible_text_that_follows_an_ocr_word_stops_it_being_kept_alone() {
    let bytes = under_scan("BT /F1 10 Tf 3 Tr 20 44 Td (John) Tj ( ) Tj (Smith) Tj 0 Tr /F1 14 Tf ( Label) Tj ET");
    let mut s = Session::new();
    let id = s.open("label.pdf", None, bytes, None).unwrap();
    let label = |s: &Session| s.get(id).unwrap().text_lines(0).iter().find(|l| l.text.contains("Label")).map(|l| l.rect[0]);
    assert_eq!(scan_and_drawn(&s, id).0, ["John Smith"]);
    let before = label(&s).unwrap();
    edit_block(&mut s, id, 0, "John");
    let (scan, _) = scan_and_drawn(&s, id);
    assert!(scan.is_empty(), "John can't stay alone when Label would move with it: {scan:?}, Label at {:?} from {before}", label(&s));
}

/// A large word right before a small one, as OCR's sizes can make them: the large word's cover,
/// padded in proportion to its height, stops at the small word, or that word would lose ink while
/// staying in the text.
#[test]
fn a_cover_stops_at_the_word_beside_it() {
    let (mut s, id) = scanned_with(vec![scanned_word("John", 42.0, 66.0, 42.0, 30.0), scanned_word("Smith", 67.2, 98.0, 42.0, 8.0)]);
    assert_eq!(s.get(id).unwrap().text_blocks(0)[0].text, "John Smith");
    assert!(paper_between(&s, id, 67, 70) < 0.02, "Smith's start is ink to begin with");
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Smith".into(), style: Default::default() }).unwrap();
    assert_eq!(scan_and_drawn(&s, id), (vec!["Smith".to_owned()], Vec::new()), "only John goes");
    assert!(paper_between(&s, id, 44, 64) > 0.98, "John is covered");
    // John's padding would reach to x = 68, over this column of Smith's ink.
    let smith = paper_between(&s, id, 67, 68);
    assert!(smith < 0.02, "Smith keeps its ink: {smith}");
}

/// Deleting a word with Backspace leaves the spaces either side of it. That isn't spacing anyone
/// asked for: the words around it stay where the scan has them, with the gap already between them.
#[test]
fn the_spaces_a_deleted_word_leaves_keep_the_scan() {
    for (draft, scan, drawn) in [
        ("John  Example", vec!["John", "Example"], vec![]),
        ("  John Example", vec!["John", "Example"], vec![]),
        ("John   Smyth  Example", vec!["John", "Example"], vec!["Smyth"]),
    ] {
        let (mut s, id) = uneven_phrase();
        s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: draft.into(), style: Default::default() }).unwrap();
        let (kept, redrawn) = scan_and_drawn(&s, id);
        assert_eq!(
            (kept.iter().map(String::as_str).collect::<Vec<_>>(), redrawn.iter().map(String::as_str).collect::<Vec<_>>()),
            (scan, drawn),
            "{draft:?}"
        );
    }
}

/// Spacing that has to be drawn goes to the rewrite that can draw it: inside new words, or typed
/// between words nothing else changed. It isn't quietly dropped.
#[test]
fn spacing_that_has_to_be_drawn_redraws_the_phrase() {
    for draft in ["Jon  Smyth Example", "John  Smith Example"] {
        let (mut s, id) = uneven_phrase();
        s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: draft.into(), style: Default::default() }).unwrap();
        let (scan, _) = scan_and_drawn(&s, id);
        assert!(scan.is_empty(), "{draft:?} keeps nothing as the scan: {scan:?}");
    }
    let (mut s, id) = uneven_phrase();
    s.apply(id, Edit::EditTextBlock { page: 0, block: 0, text: "Jon  Smyth Example".into(), style: Default::default() }).unwrap();
    let (_, drawn) = scan_and_drawn(&s, id);
    assert!(drawn.join("").contains("Jon  Smy"), "the typed spaces are there: {drawn:?}");
}

/// A word drawn twice, its copy earlier in the layer: rewriting the word takes the copy with it, and
/// the words after the copy move up the list. The next change must not land on the wrong word.
#[test]
fn a_change_lands_on_its_own_word_when_a_rewrite_removes_an_earlier_copy() {
    let bytes = under_scan(
        "BT /F1 12 Tf 3 Tr 1 0 0 1 84 44 Tm (Jones) Tj 1 0 0 1 20 44 Tm (John) Tj 1 0 0 1 50 44 Tm (Smith) Tj 1 0 0 1 84 44 Tm (Jones) Tj ET",
    );
    let mut s = Session::new();
    let id = s.open("copy.pdf", None, bytes, None).unwrap();
    let block = texts(&s, id).iter().position(|t| t == "John Smith Jones").expect("the phrase, after its copy");
    // John goes. A deletion draws nothing, so nothing else would notice it landing on Smith.
    edit_block(&mut s, id, block, "Smith Joness");
    let text = s.get(id).unwrap().text_lines(0).iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
    let words: Vec<&str> = text.split_whitespace().collect();
    assert!(words.contains(&"Smith"), "Smith wasn't changed: {words:?}");
    assert!(!words.contains(&"John"), "John is the word that goes: {words:?}");
    assert!(words.contains(&"Joness"), "{words:?}");
}

/// A layer whose words are already narrowed (50 Tz): squeezing a replacement narrows it further,
/// rather than setting a scale wider than the one it had and running it into the next word.
#[test]
fn a_squeeze_is_relative_to_the_words_own_scale() {
    let bytes = under_scan("BT /F1 12 Tf 3 Tr 50 Tz 1 0 0 1 20 44 Tm (John) Tj 1 0 0 1 36 44 Tm (Smith) Tj 1 0 0 1 56 44 Tm (Jones) Tj ET");
    let mut s = Session::new();
    let id = s.open("narrow.pdf", None, bytes, None).unwrap();
    assert_eq!(scan_and_drawn(&s, id).0, ["John Smith Jones"]);
    edit_block(&mut s, id, 0, "John Smithy Jones");
    let (scan, drawn) = scan_and_drawn(&s, id);
    assert_eq!(drawn, ["Smithy"], "Jones isn't taken over; the scan has {scan:?}");
    let lines = s.get(id).unwrap().text_lines(0);
    let smithy = lines.iter().find(|l| l.text == "Smithy").unwrap();
    let jones = lines.iter().find(|l| l.text == "Jones").unwrap();
    assert!(smithy.rect[2] <= jones.rect[0], "{:?} runs into {:?}", smithy.rect, jones.rect);
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

/// A scan whose OCR layer is drawn first and the opaque page image over it, as some OCR tools
/// write it (#3973): invisible Helvetica `text` on the scanned word, then the image.
fn ocr_under_scan(text: &str) -> std::sync::Arc<Vec<u8>> {
    under_scan(&format!("BT /F1 20 Tf 3 Tr 42 44 Td ({text}) Tj ET"))
}

/// A page whose `text` content is followed by the opaque 200 × 100 scan image.
fn under_scan(text: &str) -> std::sync::Arc<Vec<u8>> {
    under_scan_with(text, |_| pdfcraft_cos::Dict::new())
}

/// As [`under_scan`], with the page's ExtGState resources made by `ext_gstates`.
fn under_scan_with(text: &str, ext_gstates: impl FnOnce(&mut pdfcraft_cos::Document) -> pdfcraft_cos::Dict) -> std::sync::Arc<Vec<u8>> {
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream};
    let (w, h) = (200u32, 100u32);
    let mut rgb = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let ink = (WORD[0]..WORD[2]).contains(&x) && (WORD[1]..WORD[3]).contains(&y);
            rgb.extend(if ink { INK } else { PAPER });
        }
    }
    let mut d = Document::new_empty();
    let pages = d.add(Object::Dict(Dict::new()));
    let content = format!("{text} q 200 0 0 100 0 0 cm /Im Do Q");
    let contents = d.add(Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
    let mut image = Dict::new();
    image.set(b"Type".to_vec(), Object::name("XObject"));
    image.set(b"Subtype".to_vec(), Object::name("Image"));
    image.set(b"Width".to_vec(), Object::Int(i64::from(w)));
    image.set(b"Height".to_vec(), Object::Int(i64::from(h)));
    image.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    image.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let image = d.add(Object::Stream(Stream::flate(image, &rgb)));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Dict(font));
    let mut xobjects = Dict::new();
    xobjects.set(b"Im".to_vec(), Object::Ref(image));
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    res.set(b"XObject".to_vec(), Object::Dict(xobjects));
    res.set(b"ExtGState".to_vec(), Object::Dict(ext_gstates(&mut d)));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"Contents".to_vec(), Object::Ref(contents));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"MediaBox".to_vec(), Object::Array([0, 0, 200, 100].map(Object::Int).to_vec()));
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

/// The scanned word's box shows the cover and new ink, not the scan.
fn assert_edit_shows(s: &Session, id: pdfcraft_engine::DocId, label: &str) {
    let px = word_pixels(s, id);
    let paper = px.iter().filter(|p| near(**p, PAPER)).count();
    let ink = px.iter().filter(|p| near(**p, INK)).count();
    assert!(paper * 10 > px.len() * 4, "{label}: the cover is drawn over the scan: {paper} of {} paper", px.len());
    assert!(ink * 100 > px.len() * 3, "{label}: the new text shows: {ink} of {} ink", px.len());
}

fn texts(s: &Session, id: pdfcraft_engine::DocId) -> Vec<String> {
    s.get(id).unwrap().text_blocks(0).iter().map(|b| b.text.replace('\n', " ")).collect()
}

fn edit_block(s: &mut Session, id: pdfcraft_engine::DocId, block: usize, text: &str) {
    s.apply(id, Edit::EditTextBlock { page: 0, block, text: text.into(), style: Default::default() }).unwrap();
}

/// The OCR layer is under the scan image: the rewritten paragraph is drawn on top of the image,
/// not in the OCR text's place where the image would hide it (#3973). One line takes the
/// single-style rewrite, a typed line break the styled one. Undo restores the hidden layer, and
/// the edit survives redo, a save and reopen, and being edited again.
#[test]
fn an_ocr_layer_under_the_scan_image_shows_its_edit() {
    for edit in ["Jane", "Jane\nDoe"] {
        let mut s = Session::new();
        let id = s.open("ocr-under-scan.pdf", None, ocr_under_scan("John"), None).unwrap();
        let blocks = s.get(id).unwrap().text_blocks(0);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].invisible, "the OCR layer is invisible text");
        assert!(word_pixels(&s, id).iter().all(|p| near(*p, INK)), "the scanned word fills its box");

        edit_block(&mut s, id, 0, edit);
        assert_eq!(texts(&s, id), [edit.replace('\n', " ")], "{edit:?}");
        assert_edit_shows(&s, id, edit);

        s.undo(id).unwrap();
        assert_eq!(texts(&s, id), ["John"], "{edit:?}: undo");
        assert!(s.get(id).unwrap().text_blocks(0)[0].invisible, "{edit:?}: undo hides the layer again");
        assert!(word_pixels(&s, id).iter().all(|p| near(*p, INK)), "{edit:?}: undo shows the scan again");
        s.redo(id).unwrap();
        assert_edit_shows(&s, id, &format!("{edit:?} redo"));

        let saved = s.save_bytes(id).unwrap();
        let mut reopened = Session::new();
        let id = reopened.open("saved.pdf", None, saved, None).unwrap();
        assert_eq!(texts(&reopened, id), [edit.replace('\n', " ")], "{edit:?}: reopened");
        assert_edit_shows(&reopened, id, &format!("{edit:?} reopened"));

        edit_block(&mut reopened, id, 0, "Ann");
        assert_eq!(texts(&reopened, id), ["Ann"], "{edit:?}: edited again");
        assert_edit_shows(&reopened, id, &format!("{edit:?} edited again"));
    }
}

/// Visible text earlier in the same text object makes the rewrite split it; the OCR phrase still
/// goes on top of the scan, and the visible text stays.
#[test]
fn an_ocr_phrase_split_from_visible_text_under_the_scan_shows_its_edit() {
    for edit in ["Jane", "Jane\nDoe"] {
        let mut s = Session::new();
        let page = under_scan("BT /F1 20 Tf 10 80 Td (Head) Tj 3 Tr 32 -36 Td (John) Tj ET");
        let id = s.open("split.pdf", None, page, None).unwrap();
        let blocks = s.get(id).unwrap().text_blocks(0);
        let john = blocks.iter().position(|b| b.text == "John").unwrap();
        assert!(blocks[john].invisible && blocks.iter().any(|b| b.text == "Head" && !b.invisible), "{blocks:?}");
        edit_block(&mut s, id, john, edit);
        let mut after = texts(&s, id);
        after.sort();
        let mut want = vec!["Head".to_string(), edit.replace('\n', " ")];
        want.sort();
        assert_eq!(after, want, "{edit:?}");
        assert_edit_shows(&s, id, &format!("{edit:?} split"));
    }
}

/// `/SM`: a luminosity soft mask that is white over 30..110 × 30..70 in the space it is selected
/// in, black elsewhere.
fn soft_mask_state(d: &mut pdfcraft_cos::Document) -> pdfcraft_cos::Dict {
    use pdfcraft_cos::{Dict, Object, Stream};
    let mut group = Dict::new();
    group.set(b"S".to_vec(), Object::name("Transparency"));
    group.set(b"CS".to_vec(), Object::name("DeviceRGB"));
    let mut form = Dict::new();
    form.set(b"Type".to_vec(), Object::name("XObject"));
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([0, 0, 200, 100].map(Object::Int).to_vec()));
    form.set(b"Group".to_vec(), Object::Dict(group));
    let mut mask = Dict::new();
    mask.set(b"Type".to_vec(), Object::name("Mask"));
    mask.set(b"S".to_vec(), Object::name("Luminosity"));
    mask.set(b"G".to_vec(), Object::Ref(d.add(Object::Stream(Stream::flate(form, b"1 g 30 30 80 40 re f")))));
    let mut state = Dict::new();
    state.set(b"Type".to_vec(), Object::name("ExtGState"));
    state.set(b"SMask".to_vec(), Object::Dict(mask));
    let mut states = Dict::new();
    states.set(b"SM".to_vec(), Object::Dict(state));
    states
}

/// The soft mask is selected before a translation, so it is placed by the CTM before it. The
/// paragraph lifted over the scan selects it again after its own `cm`: there it must be placed
/// as it was, not 100 pt to the right where it would hide the new text, so the edit renders
/// exactly as the same edit on the page without the mask. Both rewrite paths.
#[test]
fn a_soft_mask_selected_before_a_cm_stays_put_when_the_edit_is_lifted() {
    for edit in ["Jane", "Jane\nDoe"] {
        let mut plain = Session::new();
        let page = under_scan("q 1 0 0 1 100 0 cm BT /F1 20 Tf 3 Tr -58 44 Td (John) Tj ET Q");
        let plain_id = plain.open("plain.pdf", None, page, None).unwrap();
        edit_block(&mut plain, plain_id, 0, edit);
        let mut s = Session::new();
        let page = under_scan_with("q /SM gs 1 0 0 1 100 0 cm BT /F1 20 Tf 3 Tr -58 44 Td (John) Tj ET Q", soft_mask_state);
        let id = s.open("masked.pdf", None, page, None).unwrap();
        assert_eq!(texts(&s, id), ["John"]);
        edit_block(&mut s, id, 0, edit);
        assert_eq!(texts(&s, id), [edit.replace('\n', " ")], "{edit:?}");
        assert_edit_shows(&s, id, &format!("{edit:?} masked"));
        let ink = |s: &Session, id| word_pixels(s, id).iter().filter(|p| near(**p, INK)).count();
        assert_eq!(ink(&s, id), ink(&plain, plain_id), "{edit:?}: the mask does not hide the new text");
    }
}
