use super::*;

thread_local! {
    /// Bytes the lexer read before an operand failed, on this thread.
    pub(crate) static RESCANNED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// Bytes the searches for an `EI` or an end-of-data marker, and the count of a decoder's
    /// characters, read on this thread.
    pub(crate) static EOD_SEARCHED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// Pieces [`Pieces::splice`] looked at while copying, on this thread.
    pub(crate) static PIECE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The renderer's tokenizer passes over the first `EI` (too few operators follow it, and none shows
/// text) and ends the image inside `/GSEI`. A reader that decodes the data ends it after the
/// ASCIIHex end-of-data marker `>`, at the first `EI`, and draws the form after it, which the
/// renderer reads as data: where the operators resume is a guess.
#[test]
fn an_inline_image_holding_an_earlier_end_is_ambiguous() {
    let p = parse(b"q 10 0 0 10 0 0 cm BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 00FF> EI Q /Fm Do q /GSEI gs Q");
    assert!(!p.ops.iter().any(|op| op.is("Do")), "the renderer reads the form's draw as image data");
    assert_eq!(p.ambiguous_images, 1);
    // Without a later `EI`, the renderer ends the image at the first, as other readers do.
    let p = parse(b"q 10 0 0 10 0 0 cm BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 00FF> EI Q /Fm Do q /GS1 gs Q");
    assert!(p.ops.iter().any(|op| op.is("Do")));
    assert_eq!(p.ambiguous_images, 0);
}

/// A reader that decodes the data reads it until it has the samples: past the renderer's `EI`
/// here, which ends the first image inside it (`!! EI`), where a second one then reads
/// `Q /Fm Do q /GS` as data. That reader ends after `~> EI` and draws the form.
#[test]
fn an_image_a_decoding_reader_reads_past_the_renderers_end_is_ambiguous() {
    let p = parse(b"q 10 0 0 10 0 0 cm BI /W 20 /H 1 /CS /G /BPC 8 /F /A85 ID !! EI BI /W 1 /H 1 /CS /G /BPC 8 ID s~> EI Q /Fm Do q /GSEI gs Q");
    assert!(!p.ops.iter().any(|op| op.is("Do")));
    assert!(p.ambiguous_images > 0);
}

/// A decoding reader stops once it has the samples the image needs, not at the end-of-data marker,
/// and ends the image at the first `EI` after: here at the first, where the renderer goes on to
/// `/GSEI`.
#[test]
fn a_decoding_reader_ends_the_image_once_it_has_its_samples() {
    for data in [
        &b"q 10 0 0 10 0 0 cm BI /W 1 /H 1 /CS /G /BPC 8 /F /A85 ID z EI Q /Fm Do q /GSEI gs Q zz~> EI"[..],
        b"q 10 0 0 10 0 0 cm BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 00FF EI Q /Fm Do q /GSEI gs Q",
    ] {
        let p = parse(data);
        assert!(!p.ops.iter().any(|op| op.is("Do")), "{}", String::from_utf8_lossy(data));
        assert_eq!(p.ambiguous_images, 1, "{}", String::from_utf8_lossy(data));
    }
}

/// A declared length (`/L`) doesn't hide where the data ends: readers go by the samples and the
/// end-of-data marker, not by it.
#[test]
fn a_declared_length_does_not_hide_where_the_data_ends() {
    for length in ["/L 23", "/Length 23"] {
        let data = format!("q 10 0 0 10 0 0 cm BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx {length} ID 00FF> EI Q /Fm Do q /GSEI gs Q");
        assert_eq!(parse(data.as_bytes()).ambiguous_images, 1, "{length}");
    }
}

/// A decoding reader ends the image at the first `EI` after the data, whatever the bytes around it:
/// `EIQ`, or an `EI` right after other bytes.
#[test]
fn a_decoding_reader_ends_at_an_ei_whatever_surrounds_it() {
    for data in [&b"BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 00FF> EIQ EI Q"[..], b"BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 00FF>xEI EI Q"] {
        assert_eq!(parse(data).ambiguous_images, 1, "{}", String::from_utf8_lossy(data));
    }
}

/// An image that names its filters twice, `/Filter` and `/F` saying different things: a reader
/// that takes `/F` ends the image after the ASCIIHex marker and draws the form, which the renderer
/// reads as data; one that takes `/Filter` reads Flate. Where it ends can't be told, and no more
/// when the filters' parameters or a key of its layout are given twice, differently, in both
/// spellings or in one (Ghostscript keeps the first of a key given twice). Given alike, the image
/// is as with one.
#[test]
fn an_image_whose_spellings_disagree_is_ambiguous() {
    for (filters, ambiguous) in [
        ("/Filter /Fl /F /AHx", 1),
        ("/F /Fl /Filter /AHx", 1),
        ("/Filter /Fl /F /Fl", 0),
        // Read by its last value or with one spelling, each of these is Flate's, whose end stands.
        ("/F /AHx /F /Fl", 1),
        ("/Filter /AHx /Filter /Fl", 1),
        ("/F /Fl /DecodeParms null /DecodeParms << /Predictor 2 >>", 1),
        ("/F /Fl /DecodeParms null /DP << /Predictor 2 >>", 1),
        ("/F /Fl /DP null /DP << /Predictor 2 >>", 1),
        ("/F /Fl /DP null /DecodeParms null /DP null", 0),
    ] {
        let src = format!("q BI /W 2 /H 1 /CS /G /BPC 8 {filters} ID 00FF> EI Q /Fm Do q /GSEI gs Q");
        assert_eq!(parse(src.as_bytes()).ambiguous_images, ambiguous, "{filters}");
    }
    for (layout, ambiguous) in [
        ("/Width 6 /W 2 /H 1", 1),
        ("/W 6 /Width 2 /H 1", 1),
        ("/W 2 /H 1 /Height 3", 1),
        ("/W 2 /H 1 /ColorSpace /RGB", 1),
        ("/W 2 /H 1 /BitsPerComponent 4", 1),
        ("/W 2 /H 1 /ImageMask false /IM 1", 1),
        ("/Width 2 /W 2 /H 1", 0),
        ("/W 6 /W 2 /H 1", 1),
        ("/W 2 /H 1 /H 1 /W 2", 0),
        // `/Interpolate` doesn't decide where the image ends.
        ("/W 2 /H 1 /I true /I false", 0),
    ] {
        let src = format!("q BI {layout} /CS /G /BPC 8 ID ab EI Q");
        assert_eq!(parse(src.as_bytes()).ambiguous_images, ambiguous, "{layout}");
    }
}

/// An image behind a filter no reader decodes (an unknown name, `/Crypt`, a number, or such a name
/// after a known one): a reader that decodes ends it at the first `EI` after `ID`, here before the
/// renderer's, and draws the form the renderer reads as data. Without an `EI` before the renderer's,
/// the end is the renderer's.
#[test]
fn an_image_behind_a_filter_no_reader_decodes_ends_at_its_first_ei() {
    for filters in ["/F /Foo", "/Filter /FooDecode", "/F /Crypt", "/F 5", "/F [/Fl /Foo]"] {
        let src = format!("q BI /W 2 /H 1 /CS /G /BPC 8 {filters} ID 00FF> EI Q /Fm Do q /GSEI gs Q");
        assert_eq!(parse(src.as_bytes()).ambiguous_images, 1, "{filters}");
    }
    assert_eq!(parse(b"q BI /W 2 /H 1 /CS /G /BPC 8 /F /Foo ID 00FF> EI Q").ambiguous_images, 0);
}

/// An unfiltered image whose colour space is named as a resource has a layout only the page's
/// resources tell: an `EI` in its data before the renderer's may be where a reader ends it.
/// Without one, a reader that can't look the name up ends it at the renderer's `EI`.
#[test]
fn an_image_of_a_named_colour_space_with_an_earlier_ei_is_ambiguous() {
    let p = parse(b"q 10 0 0 10 0 0 cm BI /W 2 /H 1 /CS /Cs1 /BPC 8 ID \x00\xff EI Q /Fm Do q /GSEI gs Q");
    assert!(!p.ops.iter().any(|op| op.is("Do")));
    assert_eq!(p.ambiguous_images, 1);
    let p = parse(b"q 10 0 0 10 0 0 cm BI /W 2 /H 1 /CS /Cs1 /BPC 8 ID \x00\xff EI Q");
    assert_eq!(p.ambiguous_images, 0);
}

/// ASCII85 of `bytes`, without the end-of-data marker: `z` for a group of zeros.
fn a85(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(4) {
        let mut group = [0u8; 4];
        group[..chunk.len()].copy_from_slice(chunk);
        let mut value = u32::from_be_bytes(group);
        if chunk.len() == 4 && value == 0 {
            out.push('z');
            continue;
        }
        let mut digits = [0u8; 5];
        for digit in digits.iter_mut().rev() {
            *digit = (value % 85) as u8 + b'!';
            value /= 85;
        }
        out.push_str(std::str::from_utf8(&digits[..chunk.len() + 1]).unwrap());
    }
    out
}

/// `payload` as zlib data in one stored deflate block.
fn stored_zlib(payload: &[u8]) -> Vec<u8> {
    let n = u16::try_from(payload.len()).unwrap();
    let mut out = vec![0x78, 0x01, 0x01];
    out.extend(n.to_le_bytes());
    out.extend((!n).to_le_bytes());
    out.extend(payload);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in payload {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    out.extend((b << 16 | a).to_be_bytes());
    out
}

/// Behind ASCII85 and Flate or RunLength, a reader decoding both reads the text until the inner filter
/// has the samples the image needs, or reaches its own end, a group at a time, and ends the image
/// at the first `EI` after that. An `EI` in the text before then is not where it ends; one after
/// it, before the renderer's, is. Here the four bytes `71 79 7d ec`, the group `EI!!!`, are placed
/// in what the inner filter takes in (no count), or after it (counts):
/// - **Flate:** in a stored block's five samples, or after the stream's end, or in the header of
///   the block after the samples, which a reader with its samples doesn't read.
/// - **RunLength:** in a literal run of seven samples, or after its end-of-data byte; its samples
///   may be in before that byte, or the byte before them.
///
/// An `E` that ends the last group the reader takes in is no `EI` it sees. Decode parameters that
/// say nothing (`null`, an empty dictionary, an array of those) are none. Damaged Flate, deflate
/// without a zlib header (a reader may refuse it outright), DCT (a JPEG decoder may stop before
/// its end, which its markers don't tell), other inner filters, decode parameters and a colour
/// space named as a resource leave the stop untold, and an `EI` before the renderer's counts as
/// before.
#[test]
fn where_a_decoding_reader_stops_behind_a_second_filter_is_told_by_decoding() {
    const EI: [u8; 4] = [0x71, 0x79, 0x7d, 0xec];
    let at = |params: &str, filters: &str, decoded: &[u8], placed: usize| {
        let text = a85(decoded);
        assert_eq!(text.matches("EI").count(), placed, "the `EI`s placed in the text, and no other: {text}");
        let src = format!("BI /W {params} /F [/A85 {filters}] ID {text}~> EI Q");
        parse(src.as_bytes()).ambiguous_images
    };
    let flate = stored_zlib(&[1, EI[0], EI[1], EI[2], EI[3]]);
    let rl = [&[6, 9, 9, 9][..], &EI, &[128]].concat();
    let jpeg = [&[0xff, 0xd8, 0xff, 0xda, 0x00, 0x02, 0x01, 0x02][..], &EI, &[0xff, 0xd9]].concat();
    // Five samples in a stored block that isn't the last, then the next block's header, read as
    // `EI!!!`: a reader with its samples stops before it, so it is where that reader ends the image.
    let next_block = [&[0x78, 0x01, 0x00, 5, 0, 0xfa, 0xff, 1, 2, 3, 4, 5][..], &EI, &[0x82]].concat();
    // Each: the image's parameters, its inner filter, the data before ASCII85, how many `EI`s its
    // text holds, and whether the image counts.
    let cases: [(&str, &str, Vec<u8>, usize, usize); 19] = [
        ("5 /H 1 /CS /G /BPC 8", "/Fl", flate.clone(), 1, 0),
        ("5 /H 1 /CS /G /BPC 8", "/Fl", [&flate[..], &EI].concat(), 2, 1),
        ("5 /H 1 /CS /G /BPC 8", "/Fl", next_block, 1, 1),
        ("5 /H 1 /CS /G /BPC 8", "/Fl", [&[0x78, 0x01, 0x01, 5, 0, 0, 0, 1][..], &EI, &[0; 4]].concat(), 1, 1),
        // The same stored block without its zlib header.
        ("5 /H 1 /CS /G /BPC 8", "/Fl", [&[0x01, 5, 0, 0xfa, 0xff, 1, 2, 3][..], &EI, &[0; 4]].concat(), 1, 1),
        ("5 /H 1 /CS /G /BPC 8 /DP [null << /Predictor 1 >>]", "/Fl", flate.clone(), 1, 1),
        ("5 /H 1 /CS /G /BPC 8 /DP [null null]", "/Fl", flate.clone(), 1, 0),
        ("5 /H 1 /CS /G /BPC 8 /DP [null << >>]", "/Fl", flate.clone(), 1, 0),
        ("5 /H 1 /CS /G /BPC 8 /DecodeParms << >>", "/Fl", flate.clone(), 1, 0),
        // Parameters that say nothing in one spelling, or given first, and a predictor in the
        // other: a reader may take the predictor, which it can't set up.
        ("5 /H 1 /CS /G /BPC 8 /DecodeParms null /DP [null << /Predictor 99 >>]", "/Fl", flate.clone(), 1, 1),
        ("5 /H 1 /CS /G /BPC 8 /DP [null << /Predictor 99 >>] /DP [null null]", "/Fl", flate.clone(), 1, 1),
        ("5 /H 1 /CS /Cs1 /BPC 8", "/Fl", flate.clone(), 1, 1),
        ("7 /H 1 /CS /G /BPC 8", "/RL", rl.clone(), 1, 0),
        ("7 /H 1 /CS /G /BPC 8", "/RL", [&rl[..], &[0, 0, 0], &EI].concat(), 2, 1),
        // The samples are all in, the first `EI` among them, before a second run holding another,
        // and its end-of-data byte.
        ("7 /H 1 /CS /G /BPC 8", "/RL", [&rl[..8], &[6, 9, 9, 9], &EI, &[128]].concat(), 2, 1),
        // The end-of-data byte before the samples are all in.
        ("7 /H 1 /CS /G /BPC 8", "/RL", [&[0, 9, 128, 6][..], &EI, &[1, 1, 1]].concat(), 1, 1),
        // The `E` of `!!!!E` ends the last group the reader takes in; `I!!!!` follows.
        ("7 /H 1 /CS /G /BPC 8", "/RL", vec![6, 9, 9, 9, 0, 0, 0, 0x24, 0x7c, 0x74, 0xbb, 0xa8], 1, 0),
        ("1 /H 1 /CS /G /BPC 8", "/DCT", jpeg.clone(), 1, 1),
        ("1 /H 1 /CS /G /BPC 8", "/LZW", jpeg, 1, 1),
    ];
    for (params, filters, decoded, placed, expected) in cases {
        assert_eq!(at(params, filters, &decoded, placed), expected, "/W {params} /F [/A85 {filters}]: {:02x?}", decoded);
    }
}

/// Where a decoding reader stops, in each way it can be told, against images it ends where the
/// renderer does (0) and ones it ends elsewhere (1).
#[test]
fn where_a_decoding_reader_stops_is_told_from_the_filter_and_the_layout() {
    let at = |params: &str, data: &[u8]| {
        let src = [format!("BI /W {params} ID ").as_bytes(), data].concat();
        parse(&src).ambiguous_images
    };
    let cases: [(&str, &[u8], usize); 18] = [
        // No filter: two samples `EI`, then the image's own `EI`.
        ("2 /H 1 /CS /G /BPC 8", b"EI EI Q", 0),
        // The samples `xE` start after the one byte of white space after `ID`: the `EI` their last
        // byte starts is not where a reader ends the image.
        ("2 /H 1 /CS /G /BPC 8", b"xEI EI Q", 0),
        // ASCIIHex and ASCII85, alone: the marker before the samples are all read stops the reader.
        ("4 /H 1 /CS /G /BPC 8 /F /AHx", b"00> EI Q", 0),
        ("4 /H 1 /CS /G /BPC 8 /F /A85", b"!!~> EI Q", 0),
        // A reader that needs more than the data before the renderer's `EI` reads past it.
        ("20 /H 1 /CS /G /BPC 8 /F /A85", b"!! EI Q", 1),
        // ASCII85: one group gives the four bytes; an `EI` in the data after it is where the reader ends.
        ("4 /H 1 /CS /G /BPC 8 /F /A85", b"!!EI!~> EI Q", 0),
        ("4 /H 1 /CS /G /BPC 8 /F /A85", b"!!!!!EI~> EI Q", 1),
        // `z` stands for a group at its start, and for nothing a reader agrees on inside one.
        ("8 /H 1 /CS /G /BPC 8 /F /A85", b"zz~> EI Q", 0),
        ("8 /H 1 /CS /G /BPC 8 /F /A85", b"zzEI~> EI Q", 1),
        ("4 /H 1 /CS /G /BPC 8 /F /A85", b"!z~> EI Q", 1),
        // One filter in an array is that filter alone.
        ("4 /H 1 /CS /G /BPC 8 /F [/A85]", b"!!EI!~> EI Q", 0),
        // Behind another filter, an `EI` before the renderer's counts where a reader decoding both
        // may stop before it (`where_a_decoding_reader_stops_behind_a_second_filter_is_told_by_decoding`),
        // and a marker after it counts. Here the four bytes are no zlib data, which a reader may
        // refuse at once: where it stops isn't told.
        ("4 /H 1 /CS /G /BPC 8 /F [/A85 /Fl]", b"!!EI!~> EI Q", 1),
        ("4 /H 1 /CS /G /BPC 8 /F [/A85 /Fl]", b"!!!!!~> EI Q", 0),
        ("4 /H 1 /CS /G /BPC 8 /F [/A85 /Fl]", b"!!!!! EI Q", 1),
        ("4 /H 1 /CS /G /BPC 8 /F [/AHx /Fl]", b"0011> EI Q", 0),
        ("4 /H 1 /CS /G /BPC 8 /F [/AHx /Fl]", b"0011 EI Q", 1),
        // An empty filter array is no filter: a named colour space with an `EI` in its data counts.
        ("2 /H 1 /CS /Cs1 /BPC 8 /F []", b"EI EI Q", 1),
        ("2 /H 1 /CS /G /BPC 8 /F []", b"EI EI Q", 0),
    ];
    for (params, data, expected) in cases {
        assert_eq!(at(params, data), expected, "/W {params} ID {}", String::from_utf8_lossy(data));
    }
}

/// Many inline images: whether each is counted, by its decoder's characters or by the end-of-data
/// marker and the `EI`s before it, reads the stream about once in all, not once for each image.
#[test]
fn inline_images_are_read_about_once_however_many_a_stream_holds() {
    // The last stream has its only end-of-data marker at the very end: each image's search finds
    // it far ahead, and the next uses that answer.
    let far = [b"BI /W 1 /H 1 /CS /G /BPC 8 /F [/AHx /Fl] ID 7F EI ".repeat(2_000), b">".to_vec()].concat();
    // Images that need far more samples than they hold: each one's count stops at its own `EI`.
    let short = b"BI /W 1000000 /H 1 /CS /G /BPC 8 /F /AHx ID 7F EI ".repeat(2_000);
    for data in [
        b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F EI ".repeat(2_000),
        b"BI /W 1 /H 1 /CS /G /BPC 8 /F [/AHx /Fl] ID 7F EI ".repeat(2_000),
        far,
        short,
    ] {
        EOD_SEARCHED.with(|c| c.set(0));
        let p = parse(&data);
        assert!(p.ops.len() >= 2_000);
        let searched = EOD_SEARCHED.with(std::cell::Cell::get);
        assert!(searched <= 2 * data.len(), "searched {searched} bytes of {}", data.len());
    }
}

#[test]
fn splicing_visits_each_piece_a_few_times() {
    let n = 5_000;
    let pieces: Vec<String> = (0..n).map(|i| format!("({i}) Tj")).collect();
    let joined = Pieces::join(&pieces);
    let parsed = joined.parse();
    let edits: Vec<_> = parsed.ops.iter().map(|op| (op.span.clone(), b"(x) Tj".to_vec())).collect();
    assert_eq!(edits.len(), n);
    PIECE_VISITS.with(|c| c.set(0));
    let out = joined.splice(edits);
    let visits = PIECE_VISITS.with(|c| c.get());
    assert!(visits <= 4 * n, "{visits} piece visits for {n} pieces");
    assert!(out.iter().all(|p| p == b"(x) Tj"));
    // A replacement spanning two pieces stays whole in the first and leaves the rest of the second.
    let two = Pieces::join(&["q (a)", "Tj Q"]);
    let tj = two.parse().ops.into_iter().find(|o| o.is("Tj")).map(|o| o.span).unwrap_or_default();
    assert_eq!(two.splice([(tj, b"(b) Tj".to_vec())]), [b"q (b) Tj".to_vec(), b" Q".to_vec()]);
}

#[test]
fn a_long_unreadable_number_is_read_once() {
    for junk in ["+", "5"] {
        let n = 20_000;
        let src = format!("{}{} 1 0 0 1 0 0 cm", junk.repeat(n), if junk == "5" { "x" } else { "" });
        RESCANNED.with(|c| c.set(0));
        let p = parse(src.as_bytes());
        let rescanned = RESCANNED.with(|c| c.get());
        assert!(rescanned <= 2 * n, "{rescanned} bytes read for a {n}-byte token");
        assert_eq!(p.ops.last().and_then(|o| o.nums::<6>()), Some([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
    }
    // A short damaged number still recovers byte by byte.
    let p = parse(b"-+5 0 Td");
    assert_eq!((p.ops.len(), p.skipped), (1, 1));
    assert_eq!(p.ops[0].nums::<2>(), Some([5.0, 0.0]));
}

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
        // Indexed samples permit at most 8 bits, even though device spaces permit 16, so the
        // length is unknown, and the renderer takes the second `EI`.
        let invalid = parameters.replace("/W 4", "/W 2").replace("/BPC 8", "/BPC 16");
        let parsed = inline_pixels(invalid.as_bytes(), b" EI ");
        assert_eq!(parsed.ops[1].inline.as_ref().unwrap().1, b" EI ");
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
fn filtered_unknown_and_damaged_inline_images_end_where_the_renderer_ends_them() {
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
        // What follows the first `EI` (`!!~>`) does not read as content, so the renderer takes
        // the later one.
        let parsed = inline_pixels(parameters, b"! EI !!~>");
        let label = String::from_utf8_lossy(parameters);
        assert_eq!(parsed.ops[1].inline.as_ref().unwrap().1, b"! EI !!~>", "{label}");
        assert_eq!(parsed.ops.iter().map(|op| op.op.as_slice()).collect::<Vec<_>>(), [b"q".as_slice(), b"BI", b"Q"], "{label}");
        // Only the ASCII85 image has a layout a decoding reader takes the same end from; the others,
        // damaged or of a colour space named as a resource it can't look up, it can't read, and it
        // ends them at the first `EI` after `ID`, here the one inside the data.
        assert_eq!(parsed.ambiguous_images, usize::from(!parameters.ends_with(b"/F /A85")), "{label}");
    }
    // A short payload is read whole; without an `EI`, the renderer reads no image and this
    // parser's end is a guess.
    for (src, ambiguous) in [(b"BI /W 4 /H 1 /BPC 8 /CS /G ID abc\nEI Q".as_slice(), 0), (b"BI /W 4 /H 1 /BPC 8 /CS /G ID abc", 1)] {
        let parsed = parse(src);
        assert_eq!(parsed.ops[0].inline.as_ref().unwrap().1, b"abc");
        assert_eq!(parsed.ambiguous_images, ambiguous);
        let mut reader = ops(src);
        reader.by_ref().for_each(drop);
        assert_eq!(reader.ambiguous_images(), ambiguous, "the streaming reader counts it too");
    }
}

/// The renderer reads an image whose parameters this parser can't (a stray operand before `ID`):
/// the page draws an image `ops` doesn't hold, so it is counted as ambiguous rather than dropped
/// silently.
#[test]
fn an_inline_image_read_by_the_renderer_but_not_here_is_ambiguous() {
    let parsed = parse(b"q BI /W 1 /H 1 /BPC 8 /CS /G 7 ID \x80 EI Q");
    assert_eq!(parsed.ops.iter().map(|op| op.op.as_slice()).collect::<Vec<_>>(), [b"q".as_slice(), b"Q"]);
    assert_eq!(parsed.ambiguous_images, 1);
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

/// The operators the renderer's tokenizer reads from `data`.
fn renderer_operators(data: &[u8]) -> Vec<Vec<u8>> {
    let mut iter = hayro_syntax::content::UntypedIter::new(data);
    let mut out = Vec::new();
    while let Some(instruction) = iter.next() {
        out.push(instruction.operator.as_ref().to_vec());
    }
    out
}

/// Inline images end where the renderer ends them, so both read the same operators after one:
/// an `EI` without whitespace before it in the data, upper-case text ending in "EI" after the
/// image, and an `EI` inside compressed bytes (#3579).
#[test]
fn inline_images_end_where_the_renderer_ends_them() {
    let flate: Vec<u8> = (0..FLATE_WITH_EI.len()).step_by(2).map(|i| u8::from_str_radix(&FLATE_WITH_EI[i..i + 2], 16).unwrap()).collect();
    let mut stored = b"q BI /W 64 /H 1 /BPC 8 /CS /G /F /Fl ID ".to_vec();
    stored.extend_from_slice(&flate);
    stored.extend_from_slice(b"\nEI Q BT /F1 24 Tf 72 200 Td (AFTER) Tj ET");
    let fixtures: [&[u8]; 6] = [
        b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7FEI 1 0 0 1 200 0 cm EI BT /F1 10 Tf 10 100 Td (SECRET) Tj ET",
        b"BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 7F> EI BT /F1 10 Tf 10 100 Td (ZWEI) Tj (EIN FREI BEI) Tj ET",
        b"q BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 7F7F> EI Q BT (DREI) Tj ET BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 00> EI",
        b"BI /W 4 /H 1 /BPC 8 /CS /G /F /A85 ID ! EI !!~>\nEI Q",
        b"q BI /W 4 /H 1 /BPC 8 /CS /G ID  EI \nEI Q",
        &stored,
    ];
    for data in fixtures {
        let parsed = parse(data);
        let ours: Vec<Vec<u8>> = parsed.ops.iter().map(|op| op.op.clone()).collect();
        assert_eq!(ours, renderer_operators(data), "{}", String::from_utf8_lossy(data));
        assert_eq!((parsed.skipped, parsed.ambiguous_images), (0, 0), "{}", String::from_utf8_lossy(data));
    }
}

#[test]
fn invert_accepts_small_scale_and_rejects_singular() {
    let m = Matrix([1e-7, 0., 0., 1e-7, 0., 0.]);
    let inv = m.invert().expect("a uniform 1e-7 scale is invertible");
    let id = m.then(&inv).0;
    for (got, want) in id.iter().zip(Matrix::IDENTITY.0) {
        assert!((got - want).abs() < 1e-9, "{id:?}");
    }
    assert!(Matrix([1., 2., 2., 4., 0., 0.]).invert().is_none());
    assert!(Matrix([f64::NAN, 0., 0., 1., 0., 0.]).invert().is_none());
    // A subnormal scale passes the relative check but its inverse overflows (CodeRabbit on #5703).
    assert!(Matrix([1e-310, 0., 0., 1., 0., 0.]).invert().is_none());
}

/// What inflating a stream's inline images may give is 64 MiB in all. Past it, where a reader
/// stops behind ASCII85 and Flate isn't told, and an `EI` before the renderer's counts: a first
/// image whose samples take all of it leaves the second image's `EI`, which a reader would pass
/// over, counted. Alone, the second image doesn't count.
#[test]
fn inflating_a_streams_inline_images_stops_at_its_allowance() {
    const EI: [u8; 4] = [0x71, 0x79, 0x7d, 0xec];
    // The `EI` after the large image's Flate data, on a group of its own.
    let mut large = pdfcraft_filters::encode_flate(&vec![0u8; 64 << 20]);
    large.resize(large.len().next_multiple_of(4), 1);
    large.extend(EI);
    let small = stored_zlib(&[1, EI[0], EI[1], EI[2], EI[3]]);
    let image = |params: &str, decoded: &[u8]| format!("BI /W {params} /CS /G /BPC 8 /F [/A85 /Fl] ID {}~> EI ", a85(decoded));
    let second = image("5 /H 1", &small);
    assert_eq!(parse(format!("{second}Q").as_bytes()).ambiguous_images, 0, "the second image alone");
    let both = format!("{}{second}Q", image("8192 /H 8192", &large));
    assert_eq!(parse(both.as_bytes()).ambiguous_images, 2, "after an image that spends the allowance");
}

/// What decoding a stream's inline images may give counts their decoded text too, not only what
/// inflating gives: an image whose ASCII85 text decodes past 64 MiB (`z` gives four zero bytes a
/// character, here after its RunLength data's end) isn't decoded to tell where a reader stops, and
/// its earlier `EI` counts; one whose text decodes to exactly 64 MiB is decoded, and doesn't count.
/// The allowance is the stream's, spent by each image in turn.
#[test]
fn decoding_a_streams_inline_images_counts_their_text_too() {
    const EI: [u8; 4] = [0x71, 0x79, 0x7d, 0xec];
    // A literal run of seven samples holding `EI!!!`, the end-of-data byte, and three bytes after
    // it: 12 bytes, then `zs` groups of zeros.
    let data = [&[6, 9, 9, 9][..], &EI, &[128, 0, 0, 0]].concat();
    let image = |zs: usize| format!("BI /W 7 /H 1 /CS /G /BPC 8 /F [/A85 /RL] ID {}{}~> EI Q", a85(&data), "z".repeat(zs));
    let exactly = ((64 << 20) - data.len()) / 4;
    assert_eq!(parse(image(4).as_bytes()).ambiguous_images, 0, "well within the allowance");
    assert_eq!(parse(image(exactly).as_bytes()).ambiguous_images, 0, "exactly the allowance");
    assert_eq!(parse(image(exactly + 1).as_bytes()).ambiguous_images, 1, "past it");
    // 60 MiB of the first image's text leave the second too little for its 8 MiB.
    let two = format!("{} {}", image((60 << 20) / 4), image((8 << 20) / 4));
    assert_eq!(parse(two.as_bytes()).ambiguous_images, 1, "the second image, past what the first left");
}
