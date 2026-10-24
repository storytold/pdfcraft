//! The two content-stream parsers PdfCraft runs, operator by operator on the same bytes.
//!
//! The renderer, text extraction and Find read a page through hayro's tokenizer
//! (`hayro_syntax::content::UntypedIter`, typed through `TypedIter`). Edit text, redaction,
//! the redaction audit, Office export, accessibility and the automation tools read it through
//! `pdfcraft_content::parse`. Where the two disagree on the same bytes, a feature acts on a
//! page the renderer never showed: #3107, #3108, #3111 and #3177 are that class.
//!
//! Each fixture here is parsed by both and the operator sequences compared. Where the parsers
//! agree, that is asserted. Where they are known to disagree, the test pins what *each* side
//! does today and names the issue, so a change on either side fails here and the expectation
//! is updated with it. Nothing in this file touches product code, and no oracle runs here: the
//! MuPDF and pdftotext readings quoted in comments were taken by hand on one-page files built
//! from the same bytes.

use std::fmt::Write as _;

use hayro_syntax::content::ops::TypedInstruction;
use hayro_syntax::content::{TypedIter, UntypedIter};
use hayro_syntax::object::{MaybeRef, Object as HObject};
use pdfcraft_cos::Object as CObject;

/// One operator with its operands, normalized so the two parsers can be compared as text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Tok {
    op: String,
    operands: Vec<String>,
}

fn tok(op: &str, operands: &[&str]) -> Tok {
    Tok { op: op.into(), operands: operands.iter().map(|s| s.to_string()).collect() }
}

fn num(v: f64) -> String {
    let r = (v * 1e6).round() / 1e6;
    if r.fract() == 0.0 && r.abs() < 1e15 { format!("{}", r as i64) } else { format!("{r}") }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn norm_cos(o: &CObject) -> String {
    match o {
        CObject::Null => "null".into(),
        CObject::Bool(b) => b.to_string(),
        CObject::Int(i) => num(*i as f64),
        CObject::Real(r) => num(*r),
        CObject::String(s) => format!("({})", hex(&s.bytes)),
        CObject::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        CObject::Array(a) => format!("[{}]", a.iter().map(norm_cos).collect::<Vec<_>>().join(" ")),
        CObject::Dict(d) => {
            let mut entries: Vec<String> = d.iter().map(|(k, v)| format!("/{} {}", String::from_utf8_lossy(k), norm_cos(v))).collect();
            entries.sort();
            format!("<<{}>>", entries.join(" "))
        }
        CObject::Ref(r) => format!("{} {} R", r.num, r.generation),
        CObject::Stream(_) => "stream".into(),
    }
}

fn norm_hayro(o: &HObject<'_>) -> String {
    match o {
        HObject::Null(_) => "null".into(),
        HObject::Boolean(b) => b.to_string(),
        HObject::Number(n) => num(n.as_f64()),
        HObject::String(s) => format!("({})", hex(s.as_bytes())),
        HObject::Name(n) => format!("/{}", String::from_utf8_lossy(n.as_ref())),
        HObject::Array(a) => format!("[{}]", a.raw_iter().map(|m| norm_maybe(&m)).collect::<Vec<_>>().join(" ")),
        HObject::Dict(d) => {
            let mut entries: Vec<String> = d.entries().map(|(k, v)| format!("/{} {}", String::from_utf8_lossy(k.as_ref()), norm_maybe(&v))).collect();
            entries.sort();
            format!("<<{}>>", entries.join(" "))
        }
        // An inline image: its dictionary and data. hayro keeps the white-space byte that
        // delimits `EI` inside the data; ISO 32000-2 §8.9.7 (p.268) defines the data as the bytes
        // between `ID` and `EI` "excluding the white-space delimiting those operators", which is
        // what the content crate keeps. `hayro_keeps_the_delimiter_before_ei_in_the_image_data`
        // pins that; the comparisons strip it so that every other difference stands out.
        HObject::Stream(s) => {
            let raw = s.raw_data();
            let data = match raw.last() {
                Some(b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ') => &raw[..raw.len() - 1],
                _ => &raw[..],
            };
            format!("{} data:{}", norm_hayro(&HObject::Dict(s.dict().clone())), hex(data))
        }
    }
}

fn norm_maybe(m: &MaybeRef<HObject<'_>>) -> String {
    match m {
        MaybeRef::NotRef(o) => norm_hayro(o),
        MaybeRef::Ref(r) => format!("{} {} R", r.obj_number, r.gen_number),
    }
}

/// The content crate's operators.
fn content(data: &[u8]) -> Vec<Tok> {
    pdfcraft_content::parse(data)
        .ops
        .iter()
        .map(|op| Tok {
            op: String::from_utf8_lossy(&op.op).into_owned(),
            operands: match &op.inline {
                Some((dict, image)) => vec![format!("{} data:{}", norm_cos(&CObject::Dict(dict.clone())), hex(image))],
                None => op.operands.iter().map(norm_cos).collect(),
            },
        })
        .collect()
}

/// hayro's untyped instructions: every operator the tokenizer reads, with its operand stack.
fn hayro(data: &[u8]) -> Vec<Tok> {
    let mut iter = UntypedIter::new(data);
    let mut out = Vec::new();
    while let Some(instr) = iter.next() {
        out.push(Tok { op: String::from_utf8_lossy(instr.operator).into_owned(), operands: instr.operands().map(norm_hayro).collect() });
    }
    out
}

/// How many instructions hayro's typed iterator yields before it stops. The typed layer, which
/// the renderer consumes, ends the stream at a path operator whose operands don't convert.
fn hayro_typed_len(data: &[u8]) -> usize {
    let mut iter = TypedIter::new(data);
    let mut n = 0;
    while iter.next().is_some() {
        n += 1;
    }
    n
}

fn ops(toks: &[Tok]) -> Vec<&str> {
    toks.iter().map(|t| t.op.as_str()).collect()
}

/// Both parsers on `data`, asserting they agree and that the operators are `expected`.
fn agree(name: &str, data: &[u8], expected: &[&str]) {
    let (c, h) = (content(data), hayro(data));
    assert_eq!(c, h, "{name}: content crate (left) and hayro (right) differ on {:?}", String::from_utf8_lossy(data));
    assert_eq!(ops(&c), expected, "{name}");
}

/// An unfiltered 4 × 1 grey inline image whose samples are the four bytes of `samples`.
fn inline_gray(samples: &[u8; 4], after: &[u8]) -> Vec<u8> {
    let mut v = b"q BI /W 4 /H 1 /BPC 8 /CS /G ID ".to_vec();
    v.extend_from_slice(samples);
    v.extend_from_slice(b"\nEI Q ");
    v.extend_from_slice(after);
    v
}

const GRAY_4X1: &str = "<</BPC 8 /CS /G /H 1 /W 4>>";
const GRAY_1X1: &str = "<</BPC 8 /CS /G /H 1 /W 1>>";

// ── where the two parsers agree ─────────────────────────────────────────────────────────────

#[test]
fn the_two_parsers_agree_on_well_formed_streams_and_common_damage() {
    agree("plain", b"q 1 0 0 1 10 20 cm BT /F1 12 Tf (Hi) Tj ET Q", &["q", "cm", "BT", "Tf", "Tj", "ET", "Q"]);
    agree("number forms", b"0.5 .5 6. -.5 +3 -0 5 w", &["w"]);
    assert_eq!(content(b"0.5 .5 6. -.5 +3 -0 5 w")[0].operands, ["0.5", "0.5", "6", "-0.5", "3", "0", "5"]);
    agree("strings", b"BT (a(b)c) Tj (es\\)c\\\\ape) Tj (line\\\ncont) Tj <48 65 6c> Tj ET", &["BT", "Tj", "Tj", "Tj", "Tj", "ET"]);
    assert_eq!(content(b"(line\\\ncont) Tj")[0].operands, [format!("({})", hex(b"linecont"))], "a backslash joins the lines");
    agree("unterminated string ends both streams", b"BT (open Tj ET q Q", &["BT"]);
    agree("names with escapes", b"/A#20B gs /#41 gs", &["gs", "gs"]);
    assert_eq!(content(b"/A#20B gs /#41 gs").iter().map(|t| t.operands[0].as_str()).collect::<Vec<_>>(), ["/A B", "/A"]);
    agree("array and dictionary operands", b"/Span <</MCID 1 /ActualText (x)>> BDC [(a) -250 (b) 1.5] TJ EMC", &["BDC", "TJ", "EMC"]);
    agree("ten operands", b"1 2 3 4 5 6 7 8 9 10 d0 q Q", &["d0", "q", "Q"]);
    agree("unknown operators", b"q foo 1 bar Q BT ET", &["q", "foo", "bar", "Q", "BT", "ET"]);
    agree("comments everywhere", b"q % c1\n1 0 % c2\n0 1 0 0 cm [1 % c3\n 2] TJ /N % c4\ngs Q % end", &["q", "cm", "TJ", "gs", "Q"]);
    agree("comment without end of line", b"q Q % trailing", &["q", "Q"]);
    agree("unbalanced q/Q and BT/ET", b"Q Q q BT BT ET q", &["Q", "Q", "q", "BT", "BT", "ET", "q"]);
    agree("operands without an operator at the end", b"q 1 0 0 1 5", &["q"]);
    agree("empty", b"", &[]);
    agree("only a comment", b"% nothing", &[]);
}

/// The content crate exposes two operand conventions: `Op::nums::<N>` takes the last N, as
/// hayro's typed operators do (`0 20 Tc` is `Tc 20` to the renderer), while `Op::num(i)` counts
/// from the front. Both tokenizers keep every operand, so the choice is each consumer's: the
/// edit reader reads `Tc` through `num(0)` and gets 0 where the renderer gets 20
/// (`01-readers-map.md` B.5; MuPDF renders the 20). No issue is filed for the convention.
#[test]
fn surplus_operands_are_read_from_the_end_by_hayro_and_either_way_by_the_content_crate() {
    let data = b"BT 0 20 Tc (x) Tj ET";
    agree("surplus operand before Tc", data, &["BT", "Tc", "Tj", "ET"]);
    let parsed = pdfcraft_content::parse(data);
    let tc = &parsed.ops[1];
    assert_eq!(tc.nums::<1>(), Some([20.0]), "last N, as the renderer");
    assert_eq!(tc.num(0), Some(0.0), "the i-th from the front");
    let mut iter = TypedIter::new(data);
    let mut typed_tc = None;
    while let Some(instr) = iter.next() {
        if let TypedInstruction::CharacterSpacing(c) = instr {
            typed_tc = Some(c.0.as_f64());
        }
    }
    assert_eq!(typed_tc, Some(20.0), "hayro's typed Tc takes the last operand");
}

// ── known divergences, pinned on both sides ─────────────────────────────────────────────────

/// `01-readers-map.md` B.3, no issue filed. The typed layer ends the whole stream at a path
/// operator whose operands don't convert (`vendor/hayro-syntax/src/content/mod.rs`,
/// `TypedIter::next`), so the renderer and text extraction show nothing after it. The content
/// crate keeps the operator with the operands it has and goes on, so the editor, export and
/// redaction read the text that follows. MuPDF and pdftotext go on too (`After` is shown).
#[test]
fn hayro_typed_stream_ends_at_a_path_operator_with_too_few_operands() {
    for (data, expected) in
        [(&b"0 0 100 re f BT /F1 12 Tf (After) Tj ET"[..], &["re", "f", "BT", "Tf", "Tj", "ET"][..]), (b"5 m 0 0 100 100 re f", &["m", "re", "f"])]
    {
        agree("untyped: both keep the short operator", data, expected);
        assert_eq!(hayro_typed_len(data), 0, "hayro's typed stream ends at the first operator");
    }
    assert_eq!(hayro_typed_len(b"0 0 100 100 re f BT /F1 12 Tf (After) Tj ET"), 6, "control: a complete re is read through");
}

/// `01-readers-map.md` B.17, no issue filed. hayro's operand stack holds ten entries; an
/// eleventh ends its stream. The content crate has no cap. MuPDF and pdftotext read past the
/// eleven operands (`AFTER` is shown).
#[test]
fn more_than_ten_operands_end_hayros_stream_but_not_the_content_crates() {
    let data = b"1 2 3 4 5 6 7 8 9 10 11 d0 q Q";
    assert_eq!(ops(&content(data)), ["d0", "q", "Q"]);
    assert_eq!(content(data)[0].operands.len(), 11);
    assert_eq!(hayro(data), [], "hayro: nothing, the stream ended at the eleventh operand");
}

/// No issue filed. A stray closing delimiter is skipped by the content crate and ends hayro's
/// stream. MuPDF reports a syntax error and goes on (`AFTER` is shown).
#[test]
fn a_stray_closing_delimiter_ends_hayros_stream_and_is_skipped_by_the_content_crate() {
    let data = b"q ) > ] } { Q";
    assert_eq!(ops(&content(data)), ["q", "Q"]);
    assert_eq!(ops(&hayro(data)), ["q"]);
}

/// No issue filed. A name whose `#` escape is not followed by two hex digits (ISO 32000-2
/// §7.3.5, p.28) is kept as written by the content crate and ends hayro's stream. An empty name
/// (`/`) is read by both. MuPDF goes on in both cases, reporting the resource as missing.
#[test]
fn a_malformed_name_escape_ends_hayros_stream_and_is_kept_by_the_content_crate() {
    let data = b"/C#2 gs q Q";
    assert_eq!(content(data), [tok("gs", &["/C#2"]), tok("q", &[]), tok("Q", &[])]);
    assert_eq!(hayro(data), [], "hayro: the stream ended at the name");
    agree("an empty name", b"/ gs q Q", &["gs", "q", "Q"]);
}

/// No issue filed; #2877 (open) changes number reading in cos and render. ISO 32000-2 §7.3.3
/// (p.24) says a writer "shall not use … exponential format", so neither reading is the
/// spec's. The content crate reads `1e3` as the number 1000; hayro reads it as an unknown
/// operator and drops the operands before it. MuPDF stops the whole stream at it
/// ("unknown keyword: '1e3'"); pdftotext goes on.
#[test]
fn scientific_notation_is_a_number_to_the_content_crate_and_an_unknown_operator_to_hayro() {
    let data = b"1e3 w q Q";
    assert_eq!(content(data), [tok("w", &["1000"]), tok("q", &[]), tok("Q", &[])]);
    assert_eq!(hayro(data), [tok("w", &[]), tok("q", &[]), tok("Q", &[])]);
    // A number glued to letters: the content crate skips the digits and reads the letters as an
    // operator; hayro reads the whole token as an operator (and clears the operands before it).
    let data = b"12abc w q Q";
    assert_eq!(content(data), [tok("abc", &[]), tok("w", &[]), tok("q", &[]), tok("Q", &[])]);
    assert_eq!(hayro(data), [tok("w", &[]), tok("q", &[]), tok("Q", &[])]);
}

/// #3111 (closed by #3299) made the cos lexer, and so the content crate, read `--280` as -280.
/// hayro reads it as the two numbers 0 and -280, so `cm` takes the six operands after an extra
/// one: the renderer draws this page with the matrix `0 0 1 0 -280 10` and shows nothing,
/// while MuPDF places the text at x = 120 (-280 applied). No issue covers the hayro side.
#[test]
fn a_doubled_minus_is_one_number_to_the_content_crate_and_two_to_hayro() {
    let data = b"1 0 0 1 --280 10 cm";
    assert_eq!(content(data), [tok("cm", &["1", "0", "0", "1", "-280", "10"])]);
    assert_eq!(hayro(data), [tok("cm", &["1", "0", "0", "1", "0", "-280", "10"])]);
}

// ── inline images ───────────────────────────────────────────────────────────────────────────

/// ISO 32000-2 §8.9.7 (p.268): the data lies between `ID` and `EI` "excluding the white-space
/// delimiting those operators". The content crate keeps exactly that; hayro's stream object
/// keeps the delimiter before `EI` as its last byte. Decoders ignore the extra byte, so the
/// other tests strip it (see `norm_hayro`).
#[test]
fn hayro_keeps_the_delimiter_before_ei_in_the_image_data() {
    let data = inline_gray(b"ABCD", b"");
    let parsed = pdfcraft_content::parse(&data);
    assert_eq!(parsed.ops[1].inline.as_ref().map(|(_, d)| d.as_slice()), Some(&b"ABCD"[..]));
    let mut iter = UntypedIter::new(&data);
    iter.next();
    let bi = iter.next().unwrap_or_else(|| panic!("BI"));
    let raw = bi.operands().next().and_then(|o| match o {
        HObject::Stream(s) => Some(s.raw_data().to_vec()),
        _ => None,
    });
    assert_eq!(raw.as_deref(), Some(&b"ABCD\n"[..]));
}

#[test]
fn inline_images_agree_when_ei_is_delimited() {
    agree("raw samples", &inline_gray(b"ABCD", b"BT (t) Tj ET"), &["q", "BI", "Q", "BT", "Tj", "ET"]);
    agree("ASCIIHex with a space before EI", b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41> EI Q BT (t) Tj ET", &["q", "BI", "Q", "BT", "Tj", "ET"]);
    agree("ASCII85 with a space before EI", b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID 5l~> EI Q BT (t) Tj ET", &["q", "BI", "Q", "BT", "Tj", "ET"]);
    agree("two images in a row", b"q BI /W 1 /H 1 /BPC 8 /CS /G ID A\nEI BI /W 1 /H 1 /BPC 8 /CS /G ID B\nEI Q", &["q", "BI", "BI", "Q"]);
    agree("a comment inside the parameters", b"q BI /W 1 % w\n/H 1 /BPC 8 /CS /G ID A\nEI Q", &["q", "BI", "Q"]);
    // A fake ` EI ` inside the samples with no text operator after the image: hayro's look-ahead
    // keeps searching and finds the real EI, so both read the four bytes.
    agree("fake EI, nothing but q/Q after", &inline_gray(b" EI ", b"q Q"), &["q", "BI", "Q", "q", "Q"]);
    assert_eq!(content(&inline_gray(b" EI ", b"q Q"))[1].operands, [format!("{GRAY_4X1} data:20454920")]);
}

/// #3107 (fix proposed in #3438). Unfiltered samples that happen to hold ` EI ` followed by a
/// text operator: hayro's look-ahead accepts the first `EI` because text follows it, so the
/// image holds nothing but the delimiter (no sample data at all) and the rest of the samples
/// run as content (`EI` becomes an operator). The content crate knows the sample length from `/W /H /BPC /CS` and keeps all
/// four bytes (ISO 32000-2 §8.9.7). MuPDF and pdftotext show the text after the image either
/// way. When #3438 lands, hayro's row here changes to the content crate's.
#[test]
fn a_fake_ei_inside_unfiltered_samples_cuts_hayros_image_short() {
    let data = inline_gray(b" EI ", b"BT (t) Tj ET");
    assert_eq!(
        content(&data),
        [tok("q", &[]), tok("BI", &[&format!("{GRAY_4X1} data:20454920")]), tok("Q", &[]), tok("BT", &[]), tok("Tj", &["(74)"]), tok("ET", &[])]
    );
    assert_eq!(
        hayro(&data),
        [
            tok("q", &[]),
            tok("BI", &[&format!("{GRAY_4X1} data:")]),
            tok("EI", &[]),
            tok("Q", &[]),
            tok("BT", &[]),
            tok("Tj", &["(74)"]),
            tok("ET", &[])
        ]
    );
}

/// #3108 (fix proposed in #3259). `EI` written straight after the filter's end-of-data marker
/// (`>EI`, `~>EI`): hayro ends the image there; the content crate finds no delimited `EI` and
/// swallows the rest of the stream into the image. MuPDF and pdftotext show the text after.
#[test]
fn ei_glued_to_the_filters_end_marker_swallows_the_rest_of_the_stream_for_the_content_crate() {
    for (name, data, dict) in [
        ("ASCIIHex", &b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41>EI Q BT (t) Tj ET"[..], "<</BPC 8 /CS /G /F /AHx /H 1 /W 1>>"),
        ("ASCII85", b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID 5l~>EI Q BT (t) Tj ET", "<</BPC 8 /CS /G /F /A85 /H 1 /W 1>>"),
    ] {
        let c = content(data);
        assert_eq!(ops(&c), ["q", "BI"], "{name}: the content crate's image runs to the end of the stream");
        let tail = hex(&data[data.iter().position(|&b| b == b'D').map_or(0, |i| i + 2)..]);
        assert_eq!(c[1].operands, [format!("{dict} data:{tail}")], "{name}");
        let h = hayro(data);
        assert_eq!(ops(&h), ["q", "BI", "Q", "BT", "Tj", "ET"], "{name}: hayro ends the image at EI");
        let marker = if name == "ASCIIHex" { "34313e" } else { "356c7e3e" };
        assert_eq!(h[1].operands, [format!("{dict} data:{marker}")], "{name}");
    }
}

/// No issue filed. `EI` glued to the next operator (`EIQ`): the content crate swallows the rest
/// of the stream into the image; hayro yields a `BI` with no image at all, skips to the next
/// delimited token and reads on, so the `Q` glued to `EI` is lost too. MuPDF ends the whole
/// stream ("syntax error after inline image").
#[test]
fn ei_glued_to_the_next_operator_loses_the_image_on_both_sides_differently() {
    let data = b"q BI /W 1 /H 1 /BPC 8 /CS /G ID A EIQ BT (t) Tj ET";
    let c = content(data);
    assert_eq!(ops(&c), ["q", "BI"]);
    assert_eq!(c[1].operands, [format!("{GRAY_1X1} data:{}", hex(b"A EIQ BT (t) Tj ET"))]);
    assert_eq!(hayro(data), [tok("q", &[]), tok("BI", &[]), tok("BT", &[]), tok("Tj", &["(74)"]), tok("ET", &[])]);
}

/// No issue filed. An inline image with no `EI` at all: the content crate makes the rest of the
/// stream the image's data; hayro yields a `BI` with no image and then reads the samples and
/// everything after them as operators. MuPDF ends the whole stream.
#[test]
fn an_inline_image_without_ei_is_data_to_the_content_crate_and_operators_to_hayro() {
    let data = b"q BI /W 1 /H 1 /BPC 8 /CS /G ID A Q BT (t) Tj ET";
    let c = content(data);
    assert_eq!(ops(&c), ["q", "BI"]);
    assert_eq!(c[1].operands, [format!("{GRAY_1X1} data:{}", hex(b"A Q BT (t) Tj ET"))]);
    assert_eq!(ops(&hayro(data)), ["q", "BI", "A", "Q", "BT", "Tj", "ET"]);
}

/// No issue filed. Parameters that can't be read (`/W` with no value): the content crate skips
/// to the `EI` and goes on; hayro ends its stream. MuPDF reports the dictionary and goes on.
#[test]
fn an_inline_image_with_unreadable_parameters_is_skipped_by_the_content_crate_and_ends_hayros_stream() {
    let data = b"q BI /W ID A\nEI Q";
    assert_eq!(ops(&content(data)), ["q", "Q"]);
    assert_eq!(ops(&hayro(data)), ["q"]);
}
