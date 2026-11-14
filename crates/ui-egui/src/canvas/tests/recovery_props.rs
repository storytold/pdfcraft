//! Property tests: random synthetic pages mixing strict glyphs and glyphs rule 1 recovers, and
//! random selection gestures, with the recovered layer arriving before, during or after them.
//! After every gesture, Copy keeps the strict text exactly, Copy with formatting is Copy plus
//! recovered characters only, and every strict glyph keeps its place in the recovered layer.
//!
//! Cases: `PDFCRAFT_RECOVERY_CASES` (default 400).

use super::*;
use proptest::prelude::*;

/// One glyph of a word: a lowercase letter, read strictly (`/a`) or only by rule 1 (`/C97`).
#[derive(Clone, Debug)]
struct G {
    letter: u8,
    recovered: bool,
}

/// A word: its glyphs, its font (`F1` Helvetica, strict only, or `F3` the mixed font), and
/// whether it is drawn twice at the same place (fake bold).
#[derive(Clone, Debug)]
struct Word {
    glyphs: Vec<G>,
    helvetica: bool,
    twice: bool,
}

/// A selection gesture, with glyph positions as fractions of the page's glyphs.
#[derive(Clone, Debug)]
enum Op {
    Click(u16),
    Drag(u16, u16),
    DragOn(u16),
    Shift(u16),
    SelectAll,
    SelectText(u16, u16),
    Column(u16, u16, u16, u16),
    LayerArrives,
}

fn word() -> impl Strategy<Value = Word> {
    let glyph = (0u8..26, any::<bool>()).prop_map(|(l, r)| G { letter: b'a' + l, recovered: r });
    (
        prop_oneof![
            prop::collection::vec(glyph.clone(), 1..4),
            prop::collection::vec(glyph, 4..8),
            // A run of recovered glyphs only.
            prop::collection::vec((0u8..26).prop_map(|l| G { letter: b'a' + l, recovered: true }), 1..6),
        ],
        prop::bool::weighted(0.15),
        prop::bool::weighted(0.25),
    )
        .prop_map(|(glyphs, helvetica, twice)| {
            let glyphs = if helvetica { glyphs.into_iter().map(|g| G { recovered: false, ..g }).collect() } else { glyphs };
            Word { glyphs, helvetica, twice }
        })
}

fn op() -> impl Strategy<Value = Op> {
    let at = || 0u16..1000;
    prop_oneof![
        at().prop_map(Op::Click),
        (at(), at()).prop_map(|(a, b)| Op::Drag(a, b)),
        at().prop_map(Op::DragOn),
        at().prop_map(Op::Shift),
        Just(Op::SelectAll),
        (at(), at()).prop_map(|(a, b)| Op::SelectText(a, b)),
        (at(), at(), at(), at()).prop_map(|(a, b, c, d)| Op::Column(a, b, c, d)),
        Just(Op::LayerArrives),
    ]
}

/// The page: `lines` of words. Codes 1–26 of `F3` draw `a`–`z` named `/a`…`/z` (strict),
/// codes 27–52 the same letters named `/C97`…`/C122` (rule 1 only).
fn page(lines: &[Vec<Word>]) -> Vec<u8> {
    const CFF: &[u8] = include_bytes!("../../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    let mut content = String::new();
    for (l, words) in lines.iter().enumerate() {
        let y = 760 - 24 * l;
        let mut x = 20.0;
        for w in words {
            let show = if w.helvetica {
                let s: String = w.glyphs.iter().map(|g| char::from(g.letter)).collect();
                format!("BT /F1 12 Tf {x:.1} {y} Td ({s}) Tj ET ")
            } else {
                let codes: String =
                    w.glyphs.iter().map(|g| format!("{:02x}", u32::from(g.letter - b'a') + 1 + if g.recovered { 26 } else { 0 })).collect();
                format!("BT /F3 12 Tf {x:.1} {y} Td <{codes}> Tj ET ")
            };
            content.push_str(&show);
            if w.twice {
                content.push_str(&show);
            }
            x += (w.glyphs.len() as f32 + 1.0) * 7.2;
        }
    }
    let strict: Vec<String> = (b'a'..=b'z').map(|c| format!("/{}", char::from(c))).collect();
    let numbered: Vec<String> = (b'a'..=b'z').map(|c| format!("/C{c}")).collect();
    let widths = vec!["600"; 52].join(" ");
    pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F3 6 0 R >> >> >>")
        .stream("", content)
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+AdvTTSynthetic /FirstChar 1 /LastChar 52 /Widths [{widths}] /FontDescriptor 7 0 R /Encoding << /Type /Encoding /Differences [1 {} {}] >> >>",
            strict.join(" "),
            numbered.join(" ")
        ))
        .obj("<< /Type /FontDescriptor /FontName /ABCDEF+AdvTTSynthetic /Flags 32 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 8 0 R >>")
        .stream("/Subtype /Type1C", CFF)
        .build()
}

/// What a copy put on the clipboard: (HTML if any, plain text).
type Copied = Option<(Option<String>, String)>;

fn copy(app: &mut PdfCraftApp, formatted: bool, sets: &std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>) -> Copied {
    let before = sets.lock().map(|s| s.len()).unwrap_or(0);
    let (copied, texts) = try_copy_with(app, formatted);
    if !copied {
        return None;
    }
    let html = sets.lock().ok().and_then(|s| s.get(before).cloned());
    match html {
        Some((html, plain)) => Some((Some(html), plain)),
        None => texts.last().cloned().map(|t| (None, t)),
    }
}

/// `b`'s characters without white space are `a`'s without white space with characters
/// inserted, all of them from `extra`.
fn inserted_only(a: &str, b: &str, extra: &mut Vec<char>) -> bool {
    let mut want = a.chars().filter(|c| !c.is_whitespace()).peekable();
    for c in b.chars().filter(|c| !c.is_whitespace()) {
        if want.peek() == Some(&c) {
            want.next();
        } else if let Some(k) = extra.iter().position(|e| *e == c) {
            extra.swap_remove(k);
        } else {
            return false;
        }
    }
    want.peek().is_none()
}

/// The selection as the test models it, independently of the view: where its run lies and what
/// strict glyphs it means.
#[derive(Clone, Copy, Debug)]
enum Model {
    None,
    /// A run between two strict-layer indices (made before the recovered layer arrived).
    Strict(usize, usize),
    /// A run between two recovered-layer indices; `Some` limits it to strict glyphs `lo..=hi`.
    Layer(usize, usize, Option<(usize, usize)>),
    /// The whole page (Select All, before or after the layer arrived).
    Whole,
    Column([f32; 4]),
}

fn check(lines: Vec<Vec<Word>>, ops: Vec<Op>, arrives_at: usize) -> Result<(), TestCaseError> {
    let pdf = page(&lines);
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(pdf.clone()), Default::default());
    let out = r.render(RenderRequest { page: 0, kind: RequestKind::RecoveredText, ..Default::default() });
    let strict = out.text.ok_or_else(|| TestCaseError::fail("no text"))?;
    let layer = out.recovered;
    if let Some(layer) = &layer {
        // 3. Every strict glyph has its place in the recovered layer.
        for k in 0..strict.glyphs.len() {
            prop_assert!(layer.layer_index_of(k).is_some(), "strict glyph {} has no place", k);
        }
    }
    let mut app = PdfCraftApp::new();
    app.open_bytes("p.pdf", None, pdf).map_err(|e| TestCaseError::fail(e.to_string()))?;
    app.views[0].texts.insert(0, strict.clone());
    app.views[0].current = 0;
    let sets: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> = Default::default();
    let seen = sets.clone();
    app.system_clipboard = true;
    app.html_clipboard_override = Some(Box::new(move |html, text| {
        if let Ok(mut s) = seen.lock() {
            s.push((html.to_owned(), text.to_owned()));
        }
        true
    }));
    let n_strict = strict.glyphs.len();
    let mut arrived = layer.is_none();
    let mut model = Model::None;
    for (n, op) in ops.into_iter().enumerate() {
        if !arrived && (n == arrives_at || matches!(op, Op::LayerArrives)) {
            if let Some(layer) = &layer {
                let before = app.views[0].selected_text();
                app.views[0].move_selection_onto(0, layer);
                app.views[0].recovered.insert(0, Some(layer.clone()));
                model = match model {
                    Model::Strict(a, b) => exact(layer, a, b),
                    m => m,
                };
                // The layer's arrival keeps the strict glyphs selected (Select All grows to
                // the whole layer, checked below).
                if !matches!(model, Model::Whole) {
                    prop_assert_eq!(app.views[0].selected_text(), before, "the arrival changed the strict selection");
                }
            }
            arrived = true;
        }
        let on_layer = arrived && layer.is_some();
        let len = app.views[0].selection_layer(0).map_or(0, |t| t.glyphs.len());
        let at = |f: u16| (usize::from(f) * len) / 1000;
        let run = |a: usize, b: usize| if on_layer { Model::Layer(a, b, None) } else { Model::Strict(a, b) };
        let mut strict_range = None;
        match op {
            // The pointer selects nothing on a page with no text to point at.
            Op::Click(_) | Op::Drag(..) | Op::DragOn(_) | Op::Shift(_) if len == 0 => {}
            Op::Click(a) => {
                app.views[0].select_layer_text(0, at(a), at(a));
                model = run(at(a), at(a));
            }
            Op::Drag(a, b) => {
                app.views[0].select_layer_text(0, at(a), at(a));
                if let Some(s) = app.views[0].selection.as_mut() {
                    s.set_run(at(a), at(b));
                }
                model = run(at(a), at(b));
            }
            Op::DragOn(b) | Op::Shift(b) => {
                let extended = match op {
                    Op::Shift(_) => app.views[0].extend_selection(0, at(b)),
                    _ => match app.views[0].selection.as_mut() {
                        Some(s) => match s.span {
                            selection::Span::Run { anchor, .. } => {
                                s.set_run(anchor, at(b));
                                true
                            }
                            selection::Span::Column { .. } => false,
                        },
                        None => false,
                    },
                };
                // The run keeps its anchor and takes everything up to the new end.
                model = match (extended, model) {
                    (true, Model::Strict(a, _)) => run(a, at(b)),
                    (true, Model::Layer(a, _, _)) => run(a, at(b)),
                    (true, Model::Whole) => run(0, at(b)),
                    (_, m) => m,
                };
            }
            Op::SelectAll => {
                if app.views[0].select_all() {
                    model = Model::Whole;
                }
            }
            Op::SelectText(a, b) => {
                if n_strict > 0 {
                    let (a, b) = (((usize::from(a) * n_strict) / 1000).min(n_strict - 1), ((usize::from(b) * n_strict) / 1000).min(n_strict - 1));
                    app.views[0].select_text(0, a, b);
                    strict_range = Some((a.min(b), a.max(b)));
                    model = match &layer {
                        Some(l) if on_layer => exact(l, a, b),
                        _ => Model::Strict(a, b),
                    };
                }
            }
            Op::Column(x0, y0, x1, y1) => {
                let f = |v: u16, m: f32| f32::from(v) * m / 1000.0;
                let rect = [f(x0, 600.0), f(y0, 800.0), f(x1, 600.0), f(y1, 800.0)];
                app.views[0].selection = Some(selection::Selection::column(0, rect));
                model = Model::Column(rect);
            }
            Op::LayerArrives => {}
        }
        // The strict glyphs the model says are selected, independently of the view's paths.
        let held: Vec<usize> = match (model, &layer) {
            (Model::None, _) => Vec::new(),
            (Model::Strict(a, b), _) => (a.min(b)..=a.max(b)).filter(|&k| k < n_strict).collect(),
            (Model::Whole, _) => (0..n_strict).collect(),
            (Model::Column(rect), _) => strict.glyphs_in(rect),
            // Limited to a strict range: exactly that range.
            (Model::Layer(_, _, Some((lo, hi))), _) => (lo..=hi).filter(|&k| k < n_strict).collect(),
            (Model::Layer(a, b, None), Some(l)) => {
                (0..n_strict).filter(|&k| l.layer_index_of(k).is_some_and(|p| (a.min(b)..=a.max(b)).contains(&p))).collect()
            }
            (Model::Layer(..), None) => Vec::new(),
        };
        let expect = match model {
            Model::Column(_) => strict.column_text(&held),
            _ => strict.text_of_glyphs(&held),
        };
        let plain = copy(&mut app, false, &sets);
        let formatted = copy(&mut app, true, &sets);
        let plain_text = plain.as_ref().map(|p| p.1.clone()).unwrap_or_default();
        // 1. Copy's plain text is exactly the strict glyphs the selection holds, each once.
        prop_assert_eq!(&plain_text, &expect, "model {:?}", model);
        let mut chars: Vec<char> =
            held.iter().filter_map(|&i| strict.glyphs.get(i)).flat_map(|g| g.text.chars()).filter(|c| !c.is_whitespace()).collect();
        let mut got: Vec<char> = plain_text.chars().filter(|c| !c.is_whitespace()).collect();
        chars.sort_unstable();
        got.sort_unstable();
        prop_assert_eq!(&got, &chars, "Copy drops or doubles strict characters");
        // ... and a strict range copies as it would with no recovered layer at all.
        if let Some((a, b)) = strict_range {
            prop_assert_eq!(&plain_text, &strict.text_of(a..b + 1));
        }
        // Select All, before or after the arrival, holds the whole layer once it is there.
        if matches!(model, Model::Whole)
            && let Some(l) = layer.as_ref().filter(|_| on_layer)
        {
            let all: Vec<usize> = (0..l.text.glyphs.len()).collect();
            prop_assert_eq!(app.views[0].selected_layer_glyphs().map(|(_, _, g)| g), Some(all));
        }
        check_formatted(&app, model, layer.as_ref().filter(|_| on_layer), &plain, &formatted, &plain_text)?;
    }
    Ok(())
}

/// Invariants 2–4 after one gesture: Copy with formatting against Copy (`plain`, `plain_text`)
/// and the model's selection on the recovered `layer` (when it has arrived).
fn check_formatted(
    app: &PdfCraftApp,
    model: Model,
    layer: Option<&Arc<RecoveredText>>,
    plain: &Copied,
    formatted: &Copied,
    plain_text: &str,
) -> Result<(), TestCaseError> {
    // The layer glyphs the model says are selected (its own rules: a run between two
    // places, limited to a strict range; the whole page; a column's glyphs in its box).
    let on_layer_glyphs: Vec<usize> = match (model, layer) {
        (Model::Layer(a, b, limit), Some(l)) => (a.min(b)..=a.max(b))
            .filter(|&i| i < l.text.glyphs.len())
            .filter(|&i| match (l.strict_index_of(i), limit) {
                (Some(k), Some((lo, hi))) => (lo..=hi).contains(&k),
                _ => true,
            })
            .collect(),
        (Model::Whole, Some(l)) => (0..l.text.glyphs.len()).collect(),
        (Model::Column(rect), Some(l)) => l.text.glyphs_in(rect),
        _ => Vec::new(),
    };
    let model_recovered: Vec<usize> = match layer {
        Some(l) => on_layer_glyphs.iter().copied().filter(|&i| l.recovery_of(i).is_some()).collect(),
        None => Vec::new(),
    };
    // 2. Without recovered glyphs, Copy with formatting is Copy; the view agrees with the
    // model on whether the selection holds any.
    let any_recovered = app.views[0].selection_has_recovered();
    prop_assert_eq!(any_recovered, !model_recovered.is_empty(), "model {:?}", model);
    if model_recovered.is_empty() {
        prop_assert_eq!(formatted, plain);
    }
    // 3. With recovered glyphs Copy with formatting always copies, and 4. its plain text is
    // the model's selection read on the recovered layer, every recovered character in it;
    // where the layer keeps the strict glyphs in the strict layer's order, it is Copy's text
    // with exactly those characters inserted.
    if let (false, Some(l)) = (model_recovered.is_empty(), layer) {
        prop_assert!(formatted.is_some(), "Copy with formatting copied nothing: model {:?}", model);
        let f = formatted.as_ref().map(|(_, f)| f.clone()).unwrap_or_default();
        let strip = |t: &str| t.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let on_layer_text = match model {
            Model::Column(_) => l.text.column_text(&on_layer_glyphs),
            _ => l.text.text_of_glyphs(&on_layer_glyphs),
        };
        prop_assert_eq!(strip(&f), strip(&on_layer_text), "Copy with formatting reads the selection otherwise than the layer");
        let strict_order: Vec<usize> = on_layer_glyphs.iter().filter_map(|&i| l.strict_index_of(i)).collect();
        if strict_order.windows(2).all(|w| w[0] < w[1]) {
            let mut extra: Vec<char> =
                model_recovered.iter().filter_map(|&i| l.text.glyphs.get(i)).flat_map(|g| g.text.chars()).filter(|c| !c.is_whitespace()).collect();
            prop_assert!(inserted_only(plain_text, &f, &mut extra), "{:?} is not {:?} with recovered characters inserted", f, plain_text);
            prop_assert!(extra.is_empty(), "recovered characters {:?} are missing from {:?}", extra, f);
        }
    }
    Ok(())
}

/// The run on `layer` that holds exactly strict glyphs `a..=b` (either way round): from the
/// first to the last of their places, in the run's direction.
fn exact(layer: &RecoveredText, a: usize, b: usize) -> Model {
    let (lo, hi) = (a.min(b), a.max(b));
    let places: Vec<usize> = (lo..=hi).filter_map(|k| layer.layer_index_of(k)).collect();
    // No strict glyph of the range is in the layer (none exists): the run holds nothing.
    if places.is_empty() {
        return Model::None;
    }
    let (first, last) = (places.iter().copied().min().unwrap_or(0), places.iter().copied().max().unwrap_or(0));
    let (anchor, head) = if a <= b { (first, last) } else { (last, first) };
    Model::Layer(anchor, head, Some((lo, hi)))
}

fn cases() -> u32 {
    std::env::var("PDFCRAFT_RECOVERY_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(400)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn copies_keep_strict_text_and_add_only_recovered_characters(
        lines in prop::collection::vec(prop::collection::vec(word(), 1..5), 1..4),
        ops in prop::collection::vec(op(), 1..10),
        arrives_at in 0usize..12,
    ) {
        check(lines, ops, arrives_at)?;
    }
}
