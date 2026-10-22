use super::*;

#[test]
fn operators_keep_operands_and_spans() {
    let src = b"q 1 0 0 1 72 720 cm BT /F1 12 Tf (Hello \\(world\\)) Tj [(A) -120 (B)] TJ ET Q";
    let p = parse(src);
    assert_eq!(p.skipped, 0);
    let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"q"[..], b"cm", b"BT", b"Tf", b"Tj", b"TJ", b"ET", b"Q"]);
    let cm = &p.ops[1];
    assert_eq!(cm.nums::<6>(), Some([1.0, 0.0, 0.0, 1.0, 72.0, 720.0]));
    assert_eq!(&src[cm.span.clone()], b"1 0 0 1 72 720 cm");
    assert_eq!(p.ops[4].operands[0].as_string().unwrap().bytes, b"Hello (world)");
    assert_eq!(p.ops[5].operands[0].as_array().unwrap().len(), 3);
    assert_eq!(p.ops[3].name(0), Some(&b"F1"[..]));
}

#[test]
fn round_trips_through_the_serializer() {
    let src = b"0.5 g 10 10 100 50 re f /GS1 gs true false null 3 d0 [3 2] 0 d (x) ' 1 2 (y) \"";
    let a = parse(src);
    let b = parse(&serialize_ops(&a.ops));
    assert_eq!(a.ops.iter().map(|o| (&o.op, &o.operands)).collect::<Vec<_>>(), b.ops.iter().map(|o| (&o.op, &o.operands)).collect::<Vec<_>>());
}

#[test]
fn inline_images_stay_whole() {
    let mut src = b"q 10 0 0 10 0 0 cm BI /W 2 /H 2 /CS /G /BPC 8 ID ".to_vec();
    src.extend_from_slice(&[0, b'E', b'I', 255]);
    src.extend_from_slice(b"\nEI Q 1 g");
    let p = parse(&src);
    let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"q"[..], b"cm", b"BI", b"Q", b"g"]);
    let (d, data) = p.ops[2].inline.clone().unwrap();
    assert_eq!(d.int(b"W"), Some(2));
    assert_eq!(data, [0, b'E', b'I', 255], "an EI inside the data without whitespace before it is data");
    let again = parse(&serialize_ops(&p.ops));
    assert_eq!(again.ops[2].inline, p.ops[2].inline);
}

#[test]
fn unfiltered_inline_image_keeps_whitespace_bounded_ei_pixels() {
    let src = b"q 4 0 0 1 0 0 cm BI /W 4 /H 1 /BPC 8 /CS /G ID  EI \nEI Q";
    let parsed = parse(src);
    assert_eq!(parsed.ops[2].inline.as_ref().unwrap().1, b" EI ");
    assert_eq!(parsed.ops.iter().map(|op| op.op.as_slice()).collect::<Vec<_>>(), [b"q".as_slice(), b"cm", b"BI", b"Q"]);
    let rewritten = serialize_ops(&parsed.ops);
    assert_eq!(parse(&rewritten).ops[2].inline, parsed.ops[2].inline);
}

fn inline_pixels(parameters: &[u8], pixels: &[u8]) -> Parsed {
    let mut src = b"q BI ".to_vec();
    src.extend_from_slice(parameters);
    src.extend_from_slice(b" ID ");
    src.extend_from_slice(pixels);
    src.extend_from_slice(b"\nEI Q");
    parse(&src)
}

#[test]
fn unfiltered_inline_image_lengths_use_components_and_parameter_aliases() {
    for (parameters, pixels) in [
        (b"/W 4 /H 1 /CS /G /BPC 8".as_slice(), b" EI ".as_slice()),
        (b"/W 4 /H 1 /CS /G /BPC 8 /Filter null", b" EI "),
        (b"/W 4 /H 1 /CS /G /BPC 8 /F []", b" EI "),
        (b"/W 4 /Width 4 /H 1 /CS /G /BPC 8", b" EI "),
        (b"/Width 4 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b" EI "),
        (b"/W 2 /H 1 /CS /RGB /BPC 8", b"\0 EI \xff"),
        (b"/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8", b"\0 EI \xff"),
        (b"/W 1 /H 1 /CS /CMYK /BPC 8", b" EI "),
        (b"/Width 1 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8", b" EI "),
    ] {
        let parsed = inline_pixels(parameters, pixels);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.ops.iter().map(|op| op.op.as_slice()).collect::<Vec<_>>(), [b"q".as_slice(), b"BI", b"Q"]);
        assert_eq!(parsed.ops[1].inline.as_ref().unwrap().1, pixels);
        assert_eq!(parse(&serialize_ops(&parsed.ops)).ops[1].inline, parsed.ops[1].inline);
    }
}

#[test]
fn unfiltered_inline_image_rows_are_byte_aligned_for_every_sample_depth() {
    // Two rows of two bytes each. Padding belongs to each row, not only the final row.
    for (bits, width) in [(1, 9), (2, 5), (4, 3), (8, 2), (16, 1)] {
        let parameters = format!("/W {width} /H 2 /BPC {bits} /CS /G");
        let parsed = inline_pixels(parameters.as_bytes(), b" EI ");
        assert_eq!(parsed.ops[1].inline.as_ref().unwrap().1, b" EI ", "bits={bits}");
        assert_eq!(parsed.ops.len(), 3);
    }
}

#[test]
fn unfiltered_inline_masks_default_to_one_bit_and_indexed_images_have_one_component() {
    for parameters in [b"/W 9 /H 2 /IM true".as_slice(), b"/Width 9 /Height 2 /ImageMask true /BitsPerComponent 1"] {
        let parsed = inline_pixels(parameters, b" EI ");
        assert_eq!(parsed.ops[1].inline.as_ref().unwrap().1, b" EI ");
        assert_eq!(parsed.ops.len(), 3);
    }
    for name in ["Indexed", "I"] {
        // A valid 256-entry RGB lookup table; samples are indices regardless of the base space.
        let parameters = format!("/W 4 /H 1 /BPC 8 /CS [/{name} /DeviceRGB 255 <{}>]", "00".repeat(768));
        let parsed = inline_pixels(parameters.as_bytes(), b" EI ");
        assert_eq!(parsed.ops[1].inline.as_ref().unwrap().1, b" EI ");
        assert_eq!(parsed.ops.len(), 3);
        // Indexed samples permit at most 8 bits, even though device spaces permit 16.
        let invalid = parameters.replace("/W 4", "/W 2").replace("/BPC 8", "/BPC 16");
        let parsed = inline_pixels(invalid.as_bytes(), b" EI ");
        assert!(parsed.ops[1].inline.as_ref().unwrap().1.is_empty());
    }
}

#[test]
fn unfiltered_inline_image_terminator_must_follow_its_samples() {
    for separator in [b" ".as_slice(), b"\r\n", b"\0\t\x0c\n"] {
        let mut src = b"BI /W 4 /H 1 /BPC 8 /CS /G ID  EI ".to_vec();
        src.extend_from_slice(separator);
        src.extend_from_slice(b"EI/Q q");
        let parsed = parse(&src);
        assert_eq!(parsed.ops[0].inline.as_ref().unwrap().1, b" EI ");
        assert_eq!(&src[parsed.ops[0].span.clone()], &src[..src.len() - b"/Q q".len()]);
    }
    let parsed = parse(b"BI /W 4 /H 1 /BPC 8 /CS /G ID  EI \nEI");
    assert_eq!(parsed.ops[0].inline.as_ref().unwrap().1, b" EI ");
    // EI is a keyword: a prefix of a larger token is not a terminator.
    let parsed = parse(b"BI /W 4 /H 1 /BPC 8 /CS /G ID abcd EIx\nEI Q");
    assert_eq!(parsed.ops[0].inline.as_ref().unwrap().1, b"abcd EIx");
}

#[test]
fn filtered_unknown_and_damaged_inline_images_keep_the_existing_fallback() {
    for parameters in [
        b"/W 4 /H 1 /BPC 8 /CS /G /F /A85".as_slice(),
        b"/W 4 /H 1 /BPC 8 /CS /ResourceColor",
        b"/W 0 /H 1 /BPC 8 /CS /G",
        b"/W -1 /H 1 /BPC 8 /CS /G",
        b"/W 4 /H -1 /BPC 8 /CS /G",
        b"/W 4 /H 1 /BPC 3 /CS /G",
        b"/W 4 /H 1 /IM true /BPC 8",
        b"/W 4 /H 1 /IM true /BPC /Bad",
        b"/Width 4 /W 1 /H 1 /BPC 8 /CS /G",
        b"/W 4 /H 1 /BPC 8 /CS /G /ImageMask false /IM true",
        b"/W 9223372036854775807 /H 9223372036854775807 /BPC 16 /CS /CMYK",
        b"/W 9223372036854775807 /H 2 /BPC 8 /CS /G",
    ] {
        let parsed = inline_pixels(parameters, b"! EI !!~>");
        assert_eq!(parsed.ops[1].inline.as_ref().unwrap().1, b"!", "{}", String::from_utf8_lossy(parameters));
    }
    // A short payload still uses the tolerant recovery, including when there is no EI.
    for src in [b"BI /W 4 /H 1 /BPC 8 /CS /G ID abc\nEI Q".as_slice(), b"BI /W 4 /H 1 /BPC 8 /CS /G ID abc"] {
        assert_eq!(parse(src).ops[0].inline.as_ref().unwrap().1, b"abc");
    }
}

/// A zlib stream (one stored block) for 64 DeviceGray samples whose bytes hold " EI\n" at offset 41,
/// the way a stored deflate block copies incompressible samples verbatim (#3579).
const FLATE_WITH_EI: &str = "789c014000bfffed746159d50788f7ea604984cd08e00b4c8983207f56fe82c77ef8dfb8b9a3a586382045490a3120\
    31206c2025630aaa4353e6813e233b96539c18bc3a2c5009e9d51b81";

#[test]
fn flate_inline_image_is_not_cut_at_an_ei_inside_its_compressed_bytes() {
    let flate: Vec<u8> = (0..FLATE_WITH_EI.len()).step_by(2).map(|i| u8::from_str_radix(&FLATE_WITH_EI[i..i + 2], 16).unwrap()).collect();
    assert_eq!(flate.len(), 75);
    assert_eq!(&flate[41..45], b" EI\n");
    for parameters in [b"/W 64 /H 1 /BPC 8 /CS /G /F /Fl".as_slice(), b"/W 64 /H 1 /BPC 8 /CS /G /F /Fl /L 75"] {
        let mut src = b"q 200 0 0 200 100 300 cm BI ".to_vec();
        src.extend_from_slice(parameters);
        src.extend_from_slice(b" ID ");
        src.extend_from_slice(&flate);
        src.extend_from_slice(b"\nEI Q BT /F1 24 Tf 72 200 Td (AFTER) Tj ET");
        let parsed = parse(&src);
        let label = String::from_utf8_lossy(parameters);
        assert_eq!(parsed.ops[2].inline.as_ref().unwrap().1.len(), 75, "{label}");
        let names: Vec<&[u8]> = parsed.ops.iter().map(|o| o.op.as_slice()).collect();
        assert_eq!(names, [&b"q"[..], b"cm", b"BI", b"Q", b"BT", b"Tf", b"Td", b"Tj", b"ET"], "{label}");
        assert_eq!(parsed.skipped, 0, "{label}");
        assert_eq!(parse(&serialize_ops(&parsed.ops)).ops[2].inline, parsed.ops[2].inline, "{label}");
    }
}

fn inline_pdf(content: &[u8], object_streams: bool) -> Vec<u8> {
    use pdfcraft_cos::{Document, SaveOptions, Stream, write_full};

    let mut doc = Document::new_empty();
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), content)));
    let page = doc.add(Object::Dict(
        [
            (b"Type".to_vec(), Object::name("Page")),
            (b"Parent".to_vec(), Object::Ref(pages)),
            (b"MediaBox".to_vec(), Object::Array([0, 0, 4, 1].map(Object::Int).to_vec())),
            (b"Resources".to_vec(), Object::Dict(Dict::new())),
            (b"Contents".to_vec(), Object::Ref(contents)),
        ]
        .into_iter()
        .collect(),
    ));
    doc.update_dict(pages, |dict| {
        dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
        dict.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    write_full(&doc, &SaveOptions { object_streams, ..SaveOptions::default() }).unwrap()
}

#[test]
fn rewritten_inline_image_pixels_survive_saving_and_reopening_a_pdf() {
    use pdfcraft_cos::{Document, Object};
    use std::sync::Arc;

    let original = b"q 4 0 0 1 0 0 cm BI /W 4 /H 1 /BPC 8 /CS /G ID  EI \nEI Q";
    let rewritten = serialize_ops(&parse(original).ops);
    for object_streams in [false, true] {
        let bytes = inline_pdf(&rewritten, object_streams);
        let doc = Document::open(Arc::new(bytes.clone())).unwrap();
        assert!(doc.repair_log().is_empty());
        let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
        let pages = doc.get(pages);
        let page = doc.resolve(&pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0]);
        let content = doc.resolve(page.as_dict().unwrap().get(b"Contents").unwrap());
        let Object::Stream(stream) = &*content else { panic!("page content stream") };
        assert_eq!(parse(&stream.decoded_strict().unwrap()).ops[2].inline.as_ref().unwrap().1, b" EI ");
        // Opt-in artifacts for independent readers/renderers; synthetic PDFs are never committed.
        if let Some(dir) = std::env::var_os("PDFCRAFT_INLINE_FIXTURE_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("original.pdf"), inline_pdf(original, false)).unwrap();
            std::fs::write(dir.join(if object_streams { "rewritten-packed.pdf" } else { "rewritten-classic.pdf" }), bytes).unwrap();
        }
    }
}

#[test]
fn damage_is_skipped_not_fatal() {
    let p = parse(b"1 0 0 RG ) ] 10 20 m 30 40 l S (unterminated");
    let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"RG"[..], b"m", b"l", b"S"]);
    assert!(p.skipped >= 2);
    assert!(parse(b"").ops.is_empty());
    assert!(parse(b"BI /W 1").ops.len() == 1);
}

/// Issue #1318: an inline image whose dictionary can't be read is skipped up to its `EI`, counted,
/// and what follows it is parsed.
#[test]
fn an_unreadable_inline_image_is_skipped_to_its_end() {
    let src = b"q BI /W 4 H 4 ID aa EI Q BT (After) Tj ET";
    let p = parse(src);
    let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"q"[..], b"Q", b"BT", b"Tj", b"ET"]);
    assert_eq!(p.skipped, b"BI /W 4 H 4 ID aa EI".len());
    let control = parse(b"q BI /W 4 /H 4 ID aa EI Q BT (After) Tj ET");
    assert_eq!(control.skipped, 0);
    assert_eq!(control.ops.len(), 6);
}

/// Issue #1318, follow-up: without an `EI`, an unreadable inline image runs to the end of the
/// stream, and those bytes are counted. A readable one whose data is cut short is kept, data and
/// all, so nothing of it is skipped.
#[test]
fn an_unreadable_inline_image_without_ei_is_counted() {
    let p = parse(b"q BI /W 4 H 4 ID aa");
    let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"q"[..]]);
    assert_eq!(p.skipped, b"BI /W 4 H 4 ID aa".len());
    let control = parse(b"q BI /W 4 /H 4 ID aa");
    assert_eq!(control.skipped, 0);
    let names: Vec<&[u8]> = control.ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"q"[..], b"BI"]);
    assert_eq!(control.ops[1].inline.as_ref().map(|(_, data)| data.as_slice()), Some(&b"aa"[..]));
}

#[test]
fn matrices_compose_and_invert() {
    let a = Matrix([2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
    let b = Matrix([0.0, 1.0, -1.0, 0.0, 5.0, 0.0]);
    let ab = a.then(&b);
    let (x, y) = ab.apply(1.0, 1.0);
    let (x1, y1) = a.apply(1.0, 1.0);
    assert_eq!((x, y), b.apply(x1, y1));
    let inv = ab.invert().unwrap();
    let (u, v) = inv.apply(x, y);
    assert!((u - 1.0).abs() < 1e-9 && (v - 1.0).abs() < 1e-9);
    assert_eq!(Matrix([2.0, 0.0, 0.0, 2.0, 1.0, 1.0]).bbox([0.0, 0.0, 1.0, 1.0]), [1.0, 1.0, 3.0, 3.0]);
    assert!(overlaps([0.0, 0.0, 10.0, 10.0], [5.0, 5.0, 20.0, 20.0], 0.0));
    assert!(!overlaps([0.0, 0.0, 10.0, 10.0], [10.0, 0.0, 20.0, 10.0], 0.0));
}

#[test]
fn pieces_parse_as_one_stream_and_splice_back() {
    // An operator's operands end one piece and its keyword starts the next (§7.8.2).
    let pieces = Pieces::join(&[&b"BT 72 700 Td [(After) -20 (wards)]"[..], b"TJ ET", b"q Q"]);
    assert_eq!(pieces.len(), 3);
    let ops = pieces.parse().ops;
    let names: Vec<&[u8]> = ops.iter().map(|o| o.op.as_slice()).collect();
    assert_eq!(names, [&b"BT"[..], b"Td", b"TJ", b"ET", b"q", b"Q"]);
    assert_eq!(pieces.pieces_of(&ops[2]), (0, 1));
    assert_eq!(pieces.pieces_of(&ops[3]), (1, 1));
    assert_eq!(pieces.piece_of(usize::MAX), 2);
    // Nothing changed: the pieces come back byte for byte.
    assert_eq!(pieces.splice([]), [&b"BT 72 700 Td [(After) -20 (wards)]"[..], b"TJ ET", b"q Q"]);
    // Replacing the split TJ rewrites both pieces it spans; the third is untouched.
    let out = pieces.splice([(ops[2].span.clone(), b"(New) Tj".to_vec())]);
    assert_eq!(out, [&b"BT 72 700 Td (New) Tj"[..], b" ET", b"q Q"]);
    // Removing it leaves no stray operands or keyword.
    let out = pieces.splice([(ops[2].span.clone(), Vec::new())]);
    assert_eq!(out, [&b"BT 72 700 Td "[..], b" ET", b"q Q"]);
    // Out-of-range or reversed edits are clamped, never a panic.
    let out = pieces.splice([(std::ops::Range { start: 10, end: 5 }, b"x".to_vec()), (1000..2000, Vec::new())]);
    assert_eq!(out.len(), 3);
    assert!(Pieces::join::<&[u8]>(&[]).splice([(0..1, b"x".to_vec())]).is_empty());
}

#[test]
fn stream_boundary_comment_matches_space_concatenation() {
    for eol in [b"\n".as_slice(), b"\r", b"\r\n"] {
        let mut tail = b"(HIDDEN) Tj".to_vec();
        tail.extend(eol);
        tail.extend(b"(LIVE) Tj");
        let pieces = Pieces::join(&[b"BT % comment".as_slice(), b"(ALSO HIDDEN) Tj", &tail, b"ET"]);
        let parsed = pieces.parse();
        let shown: Vec<_> = parsed.ops.iter().filter(|op| op.is("Tj")).collect();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].operands[0].as_string().unwrap().bytes, b"LIVE");
    }
}

#[test]
fn comment_edges_terminate_at_next_piece_byte_zero() {
    for eol in [b"\n".as_slice(), b"\r", b"\r\n"] {
        let mut next = eol.to_vec();
        next.extend_from_slice(b"q Q");
        let pieces = Pieces::join(&[b"% outgoing".as_slice(), next.as_slice()]);
        let parsed = pieces.parse();
        assert_eq!(pieces.comment_edges(&parsed), vec![(false, true), (true, false)], "EOL {eol:?}");
        assert_eq!(parsed.ops.iter().map(|op| op.op.as_slice()).collect::<Vec<_>>(), vec![b"q".as_slice(), b"Q"]);
    }
}

#[test]
fn comment_edges_span_empty_pieces_and_end_of_content() {
    let pieces = Pieces::join(&[b"q % outgoing".as_slice(), b"", b"", b"ignored\nQ"]);
    let parsed = pieces.parse();
    assert_eq!(pieces.comment_edges(&parsed), vec![(false, true), (true, true), (true, true), (true, false)]);
    assert_eq!(parsed.ops.iter().map(|op| op.op.as_slice()).collect::<Vec<_>>(), vec![b"q".as_slice(), b"Q"]);

    let pieces = Pieces::join(&[b"% outgoing".as_slice(), b"", b"still ignored"]);
    let parsed = pieces.parse();
    assert_eq!(pieces.comment_edges(&parsed), vec![(false, true), (true, true), (true, true)]);
    assert!(parsed.ops.is_empty());

    let pieces = Pieces::join::<&[u8]>(&[]);
    assert!(pieces.comment_edges(&pieces.parse()).is_empty());
}

#[test]
fn comment_edges_ignore_percent_inside_operands_and_image_data() {
    for first in [
        b"(literal % not a comment) Tj".as_slice(),
        b"/Percent#25Name MP",
        b"[(literal % not a comment)] TJ",
        b"/P << /ActualText (literal % not a comment) >> BDC EMC",
        b"BI /W 1 /H 1 /BPC 8 /CS /G ID % EI",
    ] {
        let pieces = Pieces::join(&[first, b"q Q"]);
        let parsed = pieces.parse();
        assert_eq!(pieces.comment_edges(&parsed), vec![(false, false), (false, false)], "{first:?}");
        assert_eq!(parsed.ops.last().unwrap().op, b"Q");
        assert_eq!(parsed.skipped, 0);
    }
}

#[test]
fn compound_operand_comment_split_is_owned_by_spanning_operator() {
    for parts in [[b"[(A) % ignored".as_slice(), b"still ignored\n(B)] TJ"], [b"/P << /MCID 0 % ignored".as_slice(), b"still ignored\n>> BDC"]] {
        let pieces = Pieces::join(&parts);
        let parsed = pieces.parse();
        // The object lexer consumes this nested comment. Its containing operator spans
        // both pieces, which forces both to be rewritten together by the redactor.
        assert_eq!(pieces.comment_edges(&parsed), vec![(false, false), (false, false)]);
        assert_eq!(parsed.ops.len(), 1);
        assert_eq!(pieces.pieces_of(&parsed.ops[0]), (0, 1));
        assert_eq!(parsed.skipped, 0);
    }
}

#[test]
fn comment_edges_do_not_confuse_local_comments_with_boundary_comments() {
    let pieces = Pieces::join(&[b"% local\nq % crossing".as_slice(), b"ignored\n% local again\nQ\n% final local\n", b"q Q"]);
    let parsed = pieces.parse();
    assert_eq!(pieces.comment_edges(&parsed), vec![(false, true), (true, false), (false, false)]);
    assert_eq!(parsed.ops.iter().map(|op| op.op.as_slice()).collect::<Vec<_>>(), vec![b"q".as_slice(), b"Q", b"q", b"Q"]);
}

#[test]
fn comment_edge_storage_depends_on_piece_count_not_comment_count() {
    let mut first = b"%\n".repeat(16_384);
    first.extend_from_slice(b"q % crossing");
    let pieces = Pieces::join(&[first.as_slice(), b"ignored\nQ"]);
    let parsed = pieces.parse();
    assert_eq!(pieces.comment_edges(&parsed), vec![(false, true), (true, false)]);
    assert_eq!(parsed.ops.len(), 2);
    // Also exercise public parsing without piece-boundary metadata.
    assert_eq!(parse(&first).ops.len(), 1);
}

#[test]
fn piece_lookup_preserves_exact_boundary_and_empty_piece_mapping() {
    // Joined layout: q<space><space>Q<space>; separator belongs to prior piece.
    let pieces = Pieces::join(&[b"q".as_slice(), b"", b"Q"]);
    for (at, expected) in [(0, 0), (1, 0), (2, 1), (3, 2), (4, 2), (5, 2), (usize::MAX, 2)] {
        assert_eq!(pieces.piece_of(at), expected, "position {at}");
    }
    let empty = Pieces::join::<&[u8]>(&[]);
    assert_eq!(empty.piece_of(0), 0);
    assert_eq!(empty.piece_of(usize::MAX), 0);
}

#[test]
fn inline_image_terminator_without_preceding_whitespace() {
    let cases: &[(&[u8], &[u8])] = &[
        (b"BI /W 4 /H 1 /BPC 8 /CS /G /F /AHx ID 2060A0E0>EI Q BT /F1 18 Tf (After) Tj ET", b"2060A0E0>"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G /Filter /ASCIIHexDecode ID 2060A0E0>EI Q BT /F1 18 Tf (After) Tj ET", b"2060A0E0>"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G /F [/AHx] ID 2060A0E0>EI Q BT /F1 18 Tf (After) Tj ET", b"2060A0E0>"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G /F [/AHx /RL] ID 2060A0E0>EI Q BT /F1 18 Tf (After) Tj ET", b"2060A0E0>"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G /F /A85 ID 5s,^g~>EI Q BT /F1 18 Tf (After) Tj ET", b"5s,^g~>"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G /Filter /ASCII85Decode ID 5s,^g~>EI Q BT /F1 18 Tf (After) Tj ET", b"5s,^g~>"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G /F [/A85 /RL] ID 5s,^g~>EI Q BT /F1 18 Tf (After) Tj ET", b"5s,^g~>"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G ID ab()EI Q BT /F1 18 Tf (After) Tj ET", b"ab()"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G ID ab[]EI Q BT /F1 18 Tf (After) Tj ET", b"ab[]"),
        (b"BI /W 4 /H 1 /BPC 8 /CS /G ID \x20\x60\xa0\xe0EI Q BT /F1 18 Tf (After) Tj ET", b"\x20\x60\xa0\xe0"),
    ];

    for (src, expected_data) in cases {
        let p = parse(src);
        assert_eq!(p.skipped, 0, "stream should have 0 skipped bytes: {}", String::from_utf8_lossy(src));
        let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
        assert_eq!(
            names,
            [&b"BI"[..], b"Q", b"BT", b"Tf", b"Tj", b"ET"],
            "operators after inline image must be preserved: {}",
            String::from_utf8_lossy(src)
        );
        let (_, img) = p.ops[0].inline.as_ref().unwrap();
        assert_eq!(img, expected_data, "inline image data must match: {}", String::from_utf8_lossy(src));
    }
}

#[test]
fn ascii85_inline_image_does_not_terminate_at_gt_ei_inside_payload() {
    for src in [
        b"BI /W 4 /H 1 /BPC 8 /CS /G /F /A85 ID !!!!>EI !!~>\nEI Q BT /F1 18 Tf (After) Tj ET".as_slice(),
        b"BI /W 4 /H 1 /BPC 8 /CS /G /F [/A85 /AHx] ID !!!!>EI !!~>\nEI Q BT /F1 18 Tf (After) Tj ET".as_slice(),
    ] {
        let p = parse(src);
        assert_eq!(p.skipped, 0);
        let names: Vec<&[u8]> = p.ops.iter().map(|o| o.op.as_slice()).collect();
        assert_eq!(names, [&b"BI"[..], b"Q", b"BT", b"Tf", b"Tj", b"ET"]);
        let (_, img) = p.ops[0].inline.as_ref().unwrap();
        assert_eq!(img, b"!!!!>EI !!~>");
    }
}
