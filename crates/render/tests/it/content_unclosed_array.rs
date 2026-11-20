//! Contributor-original synthetic pages: a content stream with an array operand that is never
//! closed. Content readers step over operator keywords inside an array, so an unclosed `[` read
//! to the end of the stream, failed there and ended the stream: everything drawn after it was
//! lost, text included. A `[` whose array can't be read is now stepped over and its items read
//! as operands of the next operator, so the damage stays with that operator (ISO 32000-2
//! §7.8.2, §9.4.3; vendored hayro-syntax patch 50).
use hayro_syntax::content::ops::{ShowTexts, TypedInstruction};
use hayro_syntax::content::{TypedIter, UntypedIter};
use hayro_syntax::object::Object;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

/// A 120 × 60 page with Helvetica as /F1 and `content` as its content stream.
fn pdf(content: &str) -> Arc<Vec<u8>> {
    let objects: [Vec<u8>; 4] = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 60] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> /Contents 4 0 R >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    Arc::new(out)
}

fn request(kind: RequestKind) -> RenderRequest {
    RenderRequest { page: 0, kind, tile: None, scale: 1.0, tag: 0, preview: false }
}

/// The page's extracted text.
fn text(content: &str) -> String {
    let mut renderer = PageRenderer::new(pdf(content), RenderConfig::default());
    let out = renderer.render(request(RequestKind::Text));
    assert!(out.error.is_none(), "{content}: {:?}", out.error);
    out.text.expect("a text layer").plain_text()
}

/// The RGBA of the pixel at (`x`, `y`) from the top left, at 72 dpi.
fn pixel(content: &str, x: usize, y: usize) -> [u8; 4] {
    let mut renderer = PageRenderer::new(pdf(content), RenderConfig::default());
    let out = renderer.render(request(RequestKind::Pixels));
    assert!(out.error.is_none(), "{content}: {:?}", out.error);
    assert_eq!((out.width, out.height), (120, 60));
    let at = (y * 120 + x) * 4;
    let rgba = out.rgba.get(at..at + 4).expect("the pixel is on the page");
    [rgba[0], rgba[1], rgba[2], rgba[3]]
}

/// `BEFORE`, then `broken` in a text object of its own, then `AFTER`.
fn around(broken: &str) -> String {
    format!("BT /F1 8 Tf 2 50 Td (BEFORE) Tj ET BT /F1 8 Tf 2 30 Td {broken} ET BT /F1 8 Tf 2 10 Td (AFTER) Tj ET")
}

/// The shape of the reported invoice: a `TJ` array begun, abandoned and written again whole
/// inside the first, which is never closed. The text after it was lost; the line itself is
/// shown once, from the closed array nearest `TJ`.
#[test]
fn text_after_an_unclosed_tj_array_is_read() {
    let page = text(&around("[(PART)75(IAL) [(WHOLE)-100(LINE)] TJ"));
    assert!(page.contains("BEFORE"), "{page:?}");
    assert!(page.contains("AFTER"), "the rest of the page was lost: {page:?}");
    assert!(page.contains("WHOLE"), "the closed array is shown: {page:?}");
    assert!(!page.contains("PART"), "the abandoned items are not shown: {page:?}");
    assert!(page.find("BEFORE") < page.find("WHOLE") && page.find("WHOLE") < page.find("AFTER"), "{page:?}");
}

/// The same with nothing closed at all, with a stray delimiter inside or after an array and with
/// arrays nested correctly: the broken operator alone is affected, and well-formed arrays read as
/// they did.
#[test]
fn a_malformed_array_affects_only_its_own_operator() {
    for (broken, shown) in [
        // An unclosed `[` before a `Tj`: its string is the operand nearest `Tj`, and is shown.
        ("[(ALSO) Tj", Some("ALSO")),
        // An unclosed `[` with no array after it: `TJ` has no array and draws nothing.
        ("[(LOST) TJ", None),
        // A stray `)` inside an array: its `TJ` is dropped, and the page goes on.
        ("[(KEPT) ) (MORE)] TJ", None),
        // A stray `]` is stepped over and the `TJ` before it drew.
        ("[(KEPT)] TJ ]", Some("KEPT")),
        // Nested arrays are well formed: `TJ` skips the inner one and shows the rest.
        ("[(NEST) [1 [2]] (ED)] TJ", Some("NESTED")),
        // An operator keyword inside a closed array is still skipped, not ended at.
        ("[(OPER) Tf (ATOR)] TJ", Some("OPERATOR")),
    ] {
        let page = text(&around(broken));
        assert!(page.contains("BEFORE"), "{broken}: {page:?}");
        assert!(page.contains("AFTER"), "{broken}: the rest of the page was lost: {page:?}");
        if let Some(word) = shown {
            assert!(page.contains(word), "{broken}: {page:?}");
        }
    }
}

/// Paths after an unclosed array are painted too, not only text.
#[test]
fn a_fill_after_an_unclosed_array_is_painted() {
    let blue = [0, 0, 255, 255];
    assert_eq!(pixel("1 0 0 rg 0 0 4 4 re f [ 1 2 0 0 1 rg 100 40 10 10 re f", 104, 15), blue);
}

/// An array nested past the parser's cap is refused as a resource limit, not repaired: the
/// stream still ends there, as it does for dictionaries nested past the cap.
#[test]
fn an_array_nested_past_the_cap_still_ends_the_stream() {
    let (red, blue, white) = ([255, 0, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]);
    for (depth, closed) in [(64, true), (65, true), (65, false)] {
        let close = if closed { "]".repeat(depth) } else { String::new() };
        let content = format!("1 0 0 rg 0 0 4 4 re f {}{close} discard 0 0 1 rg 100 40 10 10 re f", "[".repeat(depth));
        assert_eq!(pixel(&content, 1, 58), red, "depth {depth}, closed {closed}");
        let expected = if depth <= 64 { blue } else { white };
        assert_eq!(pixel(&content, 104, 15), expected, "depth {depth}, closed {closed}");
    }
}

/// The operators the content reader returns for `data`, in order.
fn operators(data: &[u8]) -> Vec<String> {
    let mut iter = UntypedIter::new(data);
    let mut out = Vec::new();
    while let Some(instruction) = iter.next() {
        out.push(String::from_utf8_lossy(instruction.operator.as_ref()).into_owned());
    }
    out
}

/// At the reader: the operators after an unclosed `[` are returned, and `TJ` takes the operand
/// nearest it, the closed array, not the abandoned items before it.
#[test]
fn the_reader_returns_the_operators_after_an_unclosed_array() {
    assert_eq!(operators(b"q [(a)75(b) [(a)75(b)(c)] TJ 0 -9 Td [(d)] TJ Q"), ["q", "TJ", "Td", "TJ", "Q"]);
    assert_eq!(operators(b"[ 1 2 q [ Q"), ["q", "Q"]);
    // Well-formed arrays, nested or holding an operator keyword, read as before.
    assert_eq!(operators(b"[(a) [1 [2]] (b)] TJ [(a) Tf (b)] TJ q"), ["TJ", "TJ", "q"]);

    let mut iter = TypedIter::new(b"[(a)75(b) [(c)-100(d)] TJ");
    let Some(TypedInstruction::ShowTexts(ShowTexts(array))) = iter.next() else {
        panic!("TJ reads its closed array");
    };
    assert_eq!(array.iter::<Object<'_>>().count(), 3);
    assert!(iter.next().is_none());
}

/// Each unclosed `[` re-reads the rest of the stream when its array fails, so recovery shares a
/// budget of eight times the stream's length, and at least 1 MiB. Once it is spent the stream
/// ends at the next unclosed `[`, as it did before, instead of taking quadratic time.
#[test]
fn unclosed_arrays_share_a_bounded_recovery_budget() {
    // 60 unclosed brackets (within the nesting cap), each with a `q` and 128 KiB of operands:
    // about 7.7 MiB, so the first brackets alone spend the budget of about 61 MiB.
    let filler = "0 ".repeat(64 * 1024);
    let data: String = (0..60).map(|_| format!("[ q {filler}")).collect();
    let read = operators(data.as_bytes()).len();
    assert!(read > 0, "the first brackets are recovered from");
    assert!(read < 60, "recovery stops once its budget is spent ({read} of 60 read)");

    // A short stream gets the floor, so a handful of brackets is always recovered from.
    assert_eq!(operators("[ q ".repeat(60).as_bytes()).len(), 60);
}

/// Random runs of brackets, strings, numbers, names and operators: the reader never panics, ends,
/// and returns at most one instruction per operator keyword in the input.
#[test]
fn random_bracket_soup_is_read_to_the_end_without_panicking() {
    const TOKENS: [&str; 14] = ["[", "]", "[", "(a)", "(", ")", "1", "-2.5", "/N", "<41>", "TJ", "Tj", "q", "Q"];
    let mut state: u64 = 0x5207;
    let mut next = || {
        // A 64-bit linear congruential generator (Knuth's MMIX constants).
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    for _ in 0..2000 {
        let len = next() % 40;
        let picks: Vec<&str> = (0..len).map(|_| TOKENS[next() % TOKENS.len()]).collect();
        let keywords = picks.iter().filter(|t| t.bytes().all(|b| b.is_ascii_alphabetic())).count();
        let data = picks.join(" ");
        let read = operators(data.as_bytes());
        assert!(read.len() <= keywords, "{data:?}: {read:?}");
    }
}
