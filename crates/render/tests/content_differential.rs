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
//!
//! Several tests pin what `vendor/hayro-syntax` does today, so a hayro upgrade or a new patch
//! to it can fail them on purpose. When that happens, update the pinned side to the new
//! behaviour and note the upstream change (the version or patch, and its PR) in that test's
//! comment, as the tests under "divergences closed" do.

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
        // An inline image: its dictionary and data. ISO 32000-2 §8.9.7 (p.268) defines the data as
        // the bytes between `ID` and `EI` "excluding the white-space delimiting those operators",
        // which is what the content crate keeps. hayro keeps exactly the samples of an unfiltered
        // image whose length its parameters give (#3438), but a filtered image, which it ends at
        // a candidate `EI`, still keeps the white-space byte before that `EI` as its last byte.
        // `hayro_keeps_the_delimiter_before_ei_only_in_filtered_image_data` pins both. The
        // comparisons strip that byte from filtered images only, so that every other difference
        // stands out: stripping it from an unfiltered image would cut a sample that happens to
        // be white space.
        HObject::Stream(s) => {
            let raw = s.raw_data();
            let filtered = s.dict().contains_key(b"F") || s.dict().contains_key(b"Filter");
            let data = match raw.split_last() {
                Some((b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ', rest)) if filtered => rest,
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

/// How many instructions hayro's typed iterator, the layer the renderer consumes, yields before
/// it stops.
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
/// edit reader reads `Tc` through `num(0)` and gets 0 where the renderer gets 20, so the editor
/// and the renderer disagree on the character spacing (MuPDF renders the 20). No issue is filed
/// for the convention.
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

// ── divergences closed since this file was written: both parsers now agree ─────────────────

/// hayro's typed layer used to end the whole stream at a path operator whose operands don't
/// convert, so the renderer and text extraction showed nothing after it.
/// Closed by #3636 (30ed249a2): `TypedIter::next` (`vendor/hayro-syntax/src/content/mod.rs`)
/// now yields such an operator as `Fallback`, which adds nothing to the path, and reads on, as
/// the content crate, MuPDF and pdftotext do (`After` is shown).
#[test]
fn both_parsers_read_on_past_a_path_operator_with_too_few_operands() {
    for (data, expected) in
        [(&b"0 0 100 re f BT /F1 12 Tf (After) Tj ET"[..], &["re", "f", "BT", "Tf", "Tj", "ET"][..]), (b"5 m 0 0 100 100 re f", &["m", "re", "f"])]
    {
        agree("untyped: both keep the short operator", data, expected);
        assert_eq!(hayro_typed_len(data), expected.len(), "hayro's typed stream reads on past the short operator");
    }
    assert_eq!(hayro_typed_len(b"0 0 100 100 re f BT /F1 12 Tf (After) Tj ET"), 6, "control: a complete re is read through");
}

/// hayro's operand stack used to hold ten entries, and an eleventh ended its stream. Closed by
/// #4345 (hayro-syntax patch 28, #4260): an instruction keeps up to 256 operands and past that
/// drops the oldest, so both parsers read all eleven and go on, as MuPDF and pdftotext do
/// (`AFTER` is shown).
#[test]
fn both_parsers_keep_more_than_ten_operands() {
    let data = b"1 2 3 4 5 6 7 8 9 10 11 d0 q Q";
    agree("eleven operands", data, &["d0", "q", "Q"]);
    assert_eq!(content(data)[0].operands.len(), 11);
}

/// A stray closing delimiter used to end hayro's stream while the content crate skipped it.
/// Closed by #3636 (30ed249a2): hayro steps over the malformed token too. MuPDF reports a
/// syntax error and goes on (`AFTER` is shown).
#[test]
fn both_parsers_skip_a_stray_closing_delimiter() {
    agree("stray delimiters", b"q ) > ] } { Q", &["q", "Q"]);
}

/// A name whose `#` escape is not followed by two hex digits (ISO 32000-2 §7.3.5, p.28) used to
/// end hayro's stream. Since #3636 (30ed249a2) hayro steps over the name and reads on, so both
/// parsers now read the same operators. One difference is still pinned: the content crate keeps
/// the name as written as the `gs` operand, while hayro drops it, so `gs` reaches the renderer
/// with no operand and does nothing, where the editor reads `/C#2`. No issue is filed. An empty
/// name (`/`) is read by both. MuPDF goes on in both cases, reporting the resource as missing.
#[test]
fn both_parsers_read_on_past_a_malformed_name_escape_but_only_the_content_crate_keeps_the_name() {
    let data = b"/C#2 gs q Q";
    assert_eq!(content(data), [tok("gs", &["/C#2"]), tok("q", &[]), tok("Q", &[])]);
    assert_eq!(hayro(data), [tok("gs", &[]), tok("q", &[]), tok("Q", &[])], "hayro: the operators, without the name");
    agree("an empty name", b"/ gs q Q", &["gs", "q", "Q"]);
}

/// #3111 (closed by #3299) made the cos lexer, and so the content crate, read `--280` as -280.
/// hayro used to read it as the two numbers 0 and -280, so `cm` took an extra operand and the
/// renderer drew the page with the matrix `0 0 1 0 -280 10`. Closed by #3630 (766ff9d05): a
/// doubled sign is one number to hayro too, so both read the matrix MuPDF applies (text at
/// x = 120).
#[test]
fn both_parsers_read_a_doubled_minus_as_one_number() {
    let data = b"1 0 0 1 --280 10 cm";
    agree("doubled minus", data, &["cm"]);
    assert_eq!(content(data)[0].operands, ["1", "0", "0", "1", "-280", "10"]);
}

// ── known divergences, pinned on both sides ─────────────────────────────────────────────────

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

// ── inline images ───────────────────────────────────────────────────────────────────────────

/// The bytes of the first inline image each parser reads in `data`: the content crate's, then
/// hayro's raw stream data (nothing stripped).
fn inline_bytes(data: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let parsed = pdfcraft_content::parse(data);
    let c = parsed.ops.iter().find_map(|op| op.inline.as_ref().map(|(_, d)| d.clone()));
    let mut iter = UntypedIter::new(data);
    let mut h = None;
    while let Some(instr) = iter.next() {
        if let Some(HObject::Stream(s)) = instr.operands().next() {
            h = Some(s.raw_data().to_vec());
            break;
        }
    }
    (c, h)
}

/// ISO 32000-2 §8.9.7 (p.268): the data lies between `ID` and `EI` "excluding the white-space
/// delimiting those operators". The content crate keeps exactly that. hayro used to keep the
/// delimiter before `EI` as the last byte of every inline image. Since #3438 (f9d37c089) it
/// takes an unfiltered image's length from `/W /H /BPC /CS` and keeps exactly the samples, so
/// both read `ABCD`. A filtered image, which hayro still ends at a candidate `EI`, keeps the
/// delimiter: that part is still pinned. Decoders stop at the filter's end-of-data marker and
/// ignore the extra byte, so the other tests strip it from filtered images (see `norm_hayro`).
#[test]
fn hayro_keeps_the_delimiter_before_ei_only_in_filtered_image_data() {
    let unfiltered = inline_bytes(&inline_gray(b"ABCD", b""));
    assert_eq!(unfiltered, (Some(b"ABCD".to_vec()), Some(b"ABCD".to_vec())), "unfiltered: both keep exactly the samples");
    let filtered = inline_bytes(b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41> EI Q");
    assert_eq!(filtered, (Some(b"41>".to_vec()), Some(b"41> ".to_vec())), "filtered: hayro keeps the space before EI");
}

/// Samples that end in a white-space byte (here ` EI ` as four gray samples, the last one 0x20)
/// are all image data when the length is computed from the parameters (4 × 1 × 8 bits = 4
/// bytes, ISO 32000-2 §8.9.7). Both parsers keep all four bytes; hayro's raw data, unstripped,
/// is the same four bytes, so its unfiltered-length path (#3438) does not take a white-space
/// sample for the delimiter before `EI`.
#[test]
fn both_parsers_keep_a_last_sample_that_is_white_space() {
    for samples in [b" EI ", b"\x10\x20\x30\x20", b"\x10\x20\x30\x0A"] {
        let (c, h) = inline_bytes(&inline_gray(samples, b"q Q"));
        assert_eq!(c.as_deref(), Some(&samples[..]), "content crate: {samples:02x?}");
        assert_eq!(h.as_deref(), Some(&samples[..]), "hayro: {samples:02x?}");
    }
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

/// #3107. Unfiltered samples that happen to hold ` EI ` followed by a text operator: hayro's
/// look-ahead used to accept the first `EI` because text follows it, so its image held no
/// samples and the rest of them ran as content (`EI` became an operator). Closed by #3438
/// (f9d37c089): hayro now takes the length from `/W /H /BPC /CS`, as the content crate does, and
/// both keep all four bytes (ISO 32000-2 §8.9.7). MuPDF and pdftotext show the text after.
#[test]
fn both_parsers_keep_a_fake_ei_inside_unfiltered_samples() {
    let data = inline_gray(b" EI ", b"BT (t) Tj ET");
    agree("fake EI, text after", &data, &["q", "BI", "Q", "BT", "Tj", "ET"]);
    assert_eq!(content(&data)[1].operands, [format!("{GRAY_4X1} data:20454920")]);
}

/// #3108. `EI` written straight after the filter's end-of-data marker (`>EI`, `~>EI`): hayro
/// ends the image there, and the content crate used to find no delimited `EI` and swallow the
/// rest of the stream into the image. Closed by #3259 (the content crate accepts `EI` after the
/// marker) and b72078871 (the content crate takes a filtered image's end from hayro's
/// tokenizer): both end the image at the marker and read the text after, as MuPDF and pdftotext
/// do.
#[test]
fn both_parsers_end_the_image_at_ei_glued_to_the_filters_end_marker() {
    for (name, data, dict, marker) in [
        ("ASCIIHex", &b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 41>EI Q BT (t) Tj ET"[..], "<</BPC 8 /CS /G /F /AHx /H 1 /W 1>>", "34313e"),
        ("ASCII85", b"q BI /W 1 /H 1 /BPC 8 /CS /G /F /A85 ID 5l~>EI Q BT (t) Tj ET", "<</BPC 8 /CS /G /F /A85 /H 1 /W 1>>", "356c7e3e"),
    ] {
        agree(name, data, &["q", "BI", "Q", "BT", "Tj", "ET"]);
        assert_eq!(content(data)[1].operands, [format!("{dict} data:{marker}")], "{name}");
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

/// One page, 612 × 792, whose content stream is `content`.
fn one_page_pdf(content: &[u8]) -> Vec<u8> {
    let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend_from_slice(content);
    stream.extend_from_slice(b"\nendstream");
    let objs: [&[u8]; 4] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << >> >>",
        &stream,
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// End to end: the renderer draws all four samples of an unfiltered 4 × 1 gray inline image
/// whose last sample is a white-space byte (0x20 or 0x0A), so hayro's computed length (#3438)
/// reaches the pixels and no sample is taken for the delimiter before `EI`.
#[test]
fn the_renderer_draws_a_last_sample_that_is_white_space() {
    use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
    for samples in [[0x20, 0x45, 0x49, 0x20], [0x10, 0x60, 0xB0, 0x20], [0x10, 0x60, 0xB0, 0x0A]] {
        // Each sample is a 100 × 100 pt block from x = 100, y = 300 (PDF space).
        let mut content = b"q 400 0 0 100 100 300 cm BI /W 4 /H 1 /BPC 8 /CS /G ID ".to_vec();
        content.extend_from_slice(&samples);
        content.extend_from_slice(b"\nEI Q");
        let pdf = std::sync::Arc::new(one_page_pdf(&content));
        let mut renderer = PageRenderer::new(pdf, RenderConfig::default());
        let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, preview: false });
        assert!(page.error.is_none(), "{samples:02x?}: {:?}", page.error);
        // Block centres: x = 150 + 100 i, raster y = 792 - 350 = 442.
        let drawn: Vec<u8> = (0..4u32)
            .map(|i| {
                let idx = ((442 * page.width + 150 + 100 * i) * 4) as usize;
                page.rgba.get(idx).copied().unwrap_or(0xFF)
            })
            .collect();
        assert_eq!(drawn, samples, "{samples:02x?}: the gray of each block");
    }
}
