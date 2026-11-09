use pdfcraft_cos::{Dict, Document, Object, Stream};

use super::*;

/// Span records read while `f` runs.
fn span_reads(f: impl FnOnce()) -> usize {
    SPAN_READS.with(|c| c.set(0));
    f();
    SPAN_READS.with(|c| c.get())
}

/// A composite font with `encoding` as its CMap, `w` as its widths and `to_unicode`.
fn composite(doc: &mut Document, encoding: &str, w: Vec<Object>, to_unicode: &str) -> Dict {
    let mut desc = Dict::new();
    desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    desc.set(b"W".to_vec(), Object::Array(w));
    let enc = Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), encoding.as_bytes()))));
    let tu = Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), to_unicode.as_bytes()))));
    font(
        doc,
        vec![("Subtype", Object::name("Type0")), ("Encoding", enc), ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])), ("ToUnicode", tu)],
    )
}

/// Many `/W` ranges that leave a code out cost a glyph of it a binary search, not a scan of them all.
#[test]
fn widths_of_a_code_no_range_covers_take_few_steps() {
    let mut doc = Document::new_empty();
    let w: Vec<Object> = (1..=10_000i64).flat_map(|k| [Object::Int(2 * k), Object::Int(2 * k), Object::Int(500)]).collect();
    let f = composite(&mut doc, "", w, "");
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!(m.width(4), 0.5);
    let reads = span_reads(|| {
        for _ in 0..1000 {
            assert_eq!(m.width(3), 1.0);
        }
    });
    assert!(reads <= 1000 * 64, "{reads} span reads");
}

/// Where spans overlap, the first-declared one wins, as a scan in declaration order finds it.
#[test]
fn overlapping_spans_resolve_to_the_first_declared() {
    let spans = Spans::new(vec![(0, 10, 'a'), (5, 20, 'b'), (3, 3, 'c'), (30, 25, 'd'), (u32::MAX, u32::MAX, 'e')]);
    let found = |code| spans.find(code).map(|s| s.2);
    let want = [(0, Some('a')), (3, Some('a')), (10, Some('a')), (11, Some('b')), (20, Some('b')), (21, None), (27, None), (u32::MAX, Some('e'))];
    for (code, value) in want {
        assert_eq!(found(code), value, "code {code}");
    }
}

/// Checking the glyph of every explicit ToUnicode entry looks its code up once in the CID map.
#[test]
fn many_unicode_entries_look_up_their_cids_in_few_steps() {
    let n = 2000;
    let cidchars: String = (0..n).map(|k| format!("<{:04X}> {} ", 0x8000 + k, k + 1)).collect();
    let encoding = format!("1 begincodespacerange <0000> <FFFF> endcodespacerange {n} begincidchar {cidchars} endcidchar");
    let bfchars: String = (0..n).map(|k| format!("<{:04X}> <{:04X}> ", 0x1000 + k, 0x4E00 + k)).collect();
    let to_unicode = format!("{n} beginbfchar {bfchars} endbfchar");
    let mut doc = Document::new_empty();
    let f = composite(&mut doc, &encoding, Vec::new(), &to_unicode);
    let reads = span_reads(|| {
        Metrics::from_dict(&doc, &f);
    });
    assert!(reads <= n * 64, "{reads} span reads");
}

/// Every glyph of a sequential range builds its text from the destination, so a long one is cut
/// to the CMap limit once, when it is read.
#[test]
fn unicode_destinations_are_cut_to_the_cmap_limit() {
    let long = "0041".repeat(4096);
    let cmap = format!("2 beginbfrange <0010> <0020> <{long}> <0030> <0030> [<{long}>] endbfrange 1 beginbfchar <0040> <{long}> endbfchar");
    let mut u = Unicode::default();
    parse_to_unicode(cmap.as_bytes(), &mut u);
    for code in [0x10, 0x20, 0x30, 0x40] {
        let text = u.text(code).unwrap_or_default();
        assert!(!text.is_empty() && text.len() <= MAX_DESTINATION_UNITS * 3, "code {code:#x}: {} bytes", text.len());
    }
}

/// A font whose ranges cover a few low codes keeps tables that size, not a fixed 64k-entry one.
#[test]
fn low_code_tables_end_with_the_last_covered_code() {
    // One range over codes 0x20..=0x21, then nothing.
    let cover = [(0x20u64, Some(0usize)), (0x22, None)];
    let owner = super::owners_below_64k(&cover);
    assert_eq!(owner.len(), 0x22);
    assert_eq!((owner[0x1F], owner[0x20], owner[0x21]), (u32::MAX, 0, 0));
    assert!(super::owners_below_64k(&[(0x2_0000, Some(0))]).is_empty(), "no code below 65 536");
    let bits = super::bit_set_below_64k([3u32, 130, 0x2_0000].into_iter());
    assert_eq!(bits.len(), 3);
    assert_eq!((bits[0], bits[2]), (1 << 3, 1 << 2));
    assert!(super::bit_set_below_64k([0x1_0000u32].into_iter()).is_empty());
}

fn font(doc: &mut Document, entries: Vec<(&str, Object)>) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Font"));
    for (k, v) in entries {
        d.set(k.as_bytes().to_vec(), v);
    }
    let _ = doc;
    d
}

/// `data` behind Flate and a PNG predictor whose `/DecodeParms` is a reference: read without
/// it, every row of four bytes keeps its tag byte.
fn behind_parms_by_reference(doc: &mut Document, data: &[u8]) -> Object {
    let parms = doc.add(Object::Dict(Dict::from_iter([(b"Predictor".to_vec(), Object::Int(12)), (b"Columns".to_vec(), Object::Int(4))])));
    let mut data = data.to_vec();
    data.resize(data.len().next_multiple_of(4), 0);
    let rows: Vec<u8> = data.chunks(4).flat_map(|row| std::iter::once(0).chain(row.iter().copied())).collect();
    let mut stream = Stream::flate(Dict::new(), &rows);
    stream.dict.set(b"DecodeParms".to_vec(), Object::Ref(parms));
    Object::Ref(doc.add(Object::Stream(stream)))
}

#[test]
fn embedded_editor_fonts_are_validated_without_installing_or_exporting() {
    use pdfcraft_cos::Stream;
    let mut doc = Document::new_empty();
    let bytes = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), bytes)));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let simple = font(&mut doc, vec![("FontDescriptor", Object::Dict(descriptor.clone()))]);
    assert_eq!(embedded_sfnt(&doc, &simple).unwrap(), bytes);
    let composite = font(&mut doc, vec![("DescendantFonts", Object::Array(vec![Object::Dict(simple.clone())]))]);
    assert_eq!(embedded_sfnt(&doc, &composite).unwrap(), bytes);
    let broken = doc.add(Object::Stream(Stream::flate(Dict::new(), b"broken font")));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(broken));
    let broken = font(&mut doc, vec![("FontDescriptor", Object::Dict(descriptor))]);
    assert!(embedded_sfnt(&doc, &broken).is_none());
    assert!(embedded_sfnt(&doc, &Dict::new()).is_none());
}

/// #2130: with no `/Widths`, an embedded program's advances win over a guess from the name
/// ("Mono" would mean Courier, 600 units for every glyph), the program's `/DecodeParms` given
/// directly or by reference.
#[test]
fn a_font_without_widths_measures_with_its_embedded_program() {
    use pdfcraft_cos::Stream;
    use skrifa::MetadataProvider as _;
    let bytes = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
    let face = skrifa::FontRef::new(bytes).unwrap();
    let (size, location) = (skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let upem = f64::from(face.metrics(size, location).units_per_em);
    let advance = |c: char| f64::from(face.glyph_metrics(size, location).advance_width(face.charmap().map(c).unwrap()).unwrap()) / upem;
    for by_reference in [false, true] {
        let mut doc = Document::new_empty();
        let program = if by_reference {
            behind_parms_by_reference(&mut doc, bytes)
        } else {
            Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), bytes))))
        };
        let mut descriptor = Dict::new();
        descriptor.set(b"FontFile2".to_vec(), program);
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("TrueType")),
                ("BaseFont", Object::name("HouseMono")),
                ("Encoding", Object::name("WinAnsiEncoding")),
                ("FontDescriptor", Object::Dict(descriptor)),
            ],
        );
        let m = Metrics::from_dict(&doc, &f);
        for c in ['i', 'l', 'W', ' '] {
            assert!(
                (m.width(u32::from(c)) - advance(c)).abs() < 1e-9,
                "{c}, by reference {by_reference}: {} vs {}",
                m.width(u32::from(c)),
                advance(c)
            );
        }
        assert!(m.width(u32::from('i')) < 0.4, "not Courier's 0.6, by reference {by_reference}");
    }
}

/// Inter with its cmap replaced by a Mac format-6 one mapping code 65 to Ω's glyph, which egui
/// can't use; and that glyph.
fn mac_subset() -> (Vec<u8>, skrifa::GlyphId) {
    use skrifa::MetadataProvider as _;
    let original = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
    let face = skrifa::FontRef::new(original).unwrap();
    let expected = face.charmap().map('Ω').unwrap();
    let mut subset = original.to_vec();
    let count = u16::from_be_bytes(subset[4..6].try_into().unwrap()) as usize;
    let entry = (0..count).map(|i| 12 + i * 16).find(|i| &subset[*i..*i + 4] == b"cmap").unwrap();
    let mut cmap = vec![0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 12];
    cmap.extend_from_slice(&[0, 6, 0, 12, 0, 0, 0, 65, 0, 1]);
    cmap.extend_from_slice(&u16::try_from(expected.to_u32()).unwrap().to_be_bytes());
    let offset = subset.len() as u32;
    subset[entry + 8..entry + 12].copy_from_slice(&offset.to_be_bytes());
    subset[entry + 12..entry + 16].copy_from_slice(&(cmap.len() as u32).to_be_bytes());
    subset.extend_from_slice(&cmap);
    (subset, expected)
}

#[test]
fn mac_subset_cmap_uses_pdf_unicode_without_changing_source_program() {
    use skrifa::{MetadataProvider as _, raw::TableProvider as _};
    let (subset, expected) = mac_subset();
    let face = skrifa::FontRef::new(&subset).unwrap();
    assert!(!face.charmap().has_map(), "egui cannot directly use Mac format 6");
    assert_eq!(face.cmap().unwrap().map_codepoint(65_u32), Some(expected));
    let mut doc = Document::new_empty();
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), &subset)));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), b"1 beginbfchar <41> <03A9> endbfchar")));
    let source = font(&mut doc, vec![("FontDescriptor", Object::Dict(descriptor)), ("ToUnicode", Object::Ref(unicode))]);
    let editor = embedded_sfnt(&doc, &source).unwrap();
    let face = skrifa::FontRef::new(&editor).unwrap();
    assert_eq!(face.charmap().map('Ω'), Some(expected), "Unicode selects the original glyph");
    let Object::Stream(source) = doc.resolve(&Object::Ref(program)).as_ref().clone() else {
        panic!("source stream");
    };
    assert_eq!(source.decoded().unwrap(), subset, "only an in-memory editor copy is adapted");
    let mut broken = vec![0; 16];
    assert!(append_editor_cmap(&mut broken, &std::collections::BTreeMap::from([(65, 1)])).is_none());
}

/// The metrics read that adapts a program's cmap is paid for from the editor's allowance too: a
/// step short of the program, its ToUnicode and the program again for its widths and for the
/// readings of its codes by glyph name and as Mac OS Roman, the metrics are read past the allowance
/// and no copy is adapted.
#[test]
fn adapting_a_programs_cmap_is_paid_for() {
    use skrifa::MetadataProvider as _;
    let (subset, expected) = mac_subset();
    let mut doc = Document::new_empty();
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), &subset)));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let cmap = b"1 beginbfchar <41> <03A9> endbfchar";
    let unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap)));
    let source = font(&mut doc, vec![("FontDescriptor", Object::Dict(descriptor)), ("ToUnicode", Object::Ref(unicode))]);
    let cost = 4 * subset.len() + cmap.len();
    let mut budget = DecodeBudget::new(cost, 0);
    let editor = embedded_sfnt_within(&doc, &source, &mut budget).unwrap();
    assert_eq!(skrifa::FontRef::new(&editor).unwrap().charmap().map('Ω'), Some(expected));
    assert_eq!(budget.remaining(), 0);
    assert!(embedded_sfnt_within(&doc, &source, &mut DecodeBudget::new(cost - 1, 0)).is_none());
    // A composite font's metrics are read too, and paid for: the program and its ToUnicode, which
    // give no glyph here (its codes aren't Identity-H).
    let mut budget = DecodeBudget::new(cost, 0);
    let mut composite = source.clone();
    composite.set(b"Subtype".to_vec(), Object::name("Type0"));
    assert!(embedded_sfnt_within(&doc, &composite, &mut budget).is_none());
    assert_eq!(budget.remaining(), cost - subset.len() - cmap.len());
}

/// A Type 0 subset font whose `/Encoding`, program, `/CIDToGIDMap` and ToUnicode are streams, and
/// what each decodes to.
fn subset_type0(doc: &mut Document, to_unicode: &[u8]) -> (Dict, [usize; 4]) {
    let encoding = b"1 begincodespacerange <0000> <FFFF> endcodespacerange";
    let program = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
    let map = [0u8, 0, 0, 1, 0, 2];
    let mut stream = |data: &[u8]| Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), data))));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), stream(program));
    let mut descendant = Dict::new();
    descendant.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    descendant.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    descendant.set(b"CIDToGIDMap".to_vec(), stream(&map));
    let entries = vec![
        ("Subtype", Object::name("Type0")),
        ("BaseFont", Object::name("ABCDEF+Inter")),
        ("Encoding", stream(encoding)),
        ("DescendantFonts", Object::Array(vec![Object::Dict(descendant)])),
        ("ToUnicode", stream(to_unicode)),
    ];
    (font(doc, entries), [encoding.len(), program.len(), map.len(), to_unicode.len()])
}

/// Each stream a font's metrics come from is paid for from the allowance they are read within,
/// down to the byte; a stream it can't pay for is left unread, and the metrics say so.
#[test]
fn a_fonts_streams_are_paid_for_from_its_allowance() {
    let mut doc = Document::new_empty();
    let (f, sizes) = subset_type0(&mut doc, b"1 beginbfchar <0001> <0058> endbfchar");
    let cost: usize = sizes.iter().sum();
    let whole = Metrics::from_dict(&doc, &f);
    assert!(!whole.past_allowance);
    assert_eq!((whole.code_len, whole.text_of(1).as_deref()), (2, Some("X")));
    assert!(!whole.outlined_cids.is_empty(), "the program and its map were read");
    let mut budget = DecodeBudget::new(cost, 0);
    assert_eq!(Metrics::from_dict_within(&doc, &f, &mut budget), whole);
    assert_eq!(budget.remaining(), 0);
    // A byte short, the ToUnicode, read last, is left unread; what came before is kept.
    let mut budget = DecodeBudget::new(cost - 1, 0);
    let m = Metrics::from_dict_within(&doc, &f, &mut budget);
    assert!(m.past_allowance);
    assert_eq!((m.code_len, m.text_of(1)), (2, None));
    assert_eq!(m.outlined_cids, whole.outlined_cids);
    // With nothing left, none of them is read.
    let m = Metrics::from_dict_within(&doc, &f, &mut budget);
    assert!(m.past_allowance && m.outlined_cids.is_empty());
    assert_eq!(m.codes, Codes::Two, "the Encoding CMap was left unread");
    // Only a subset's program says which glyphs it lacks: the whole font pays for its CMaps alone.
    let mut whole_font = f.clone();
    whole_font.set(b"BaseFont".to_vec(), Object::name("Inter"));
    let mut budget = DecodeBudget::new(cost, 0);
    let m = Metrics::from_dict_within(&doc, &whole_font, &mut budget);
    assert!(!m.past_allowance && m.outlined_cids.is_empty());
    assert_eq!(cost - budget.remaining(), sizes[0] + sizes[3]);
}

/// With nothing left, an empty stream still reads, as the allowance reads one, and is no reason to
/// call the font read past it; a stream with data is left unread, and is.
#[test]
fn an_empty_stream_reads_with_nothing_left() {
    let past = |data: &[u8]| {
        let mut doc = Document::new_empty();
        let unicode = Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), data.to_vec()))));
        let f = font(&mut doc, vec![("Subtype", Object::name("TrueType")), ("ToUnicode", unicode)]);
        Metrics::from_dict_within(&doc, &f, &mut DecodeBudget::new(0, 0)).past_allowance
    };
    assert!(!past(b""));
    assert!(past(b"1 beginbfchar <41> <0058> endbfchar"));
}

/// A stream behind one filter past its own limit, with more left than its limit, is left unread as
/// it always was, paying for the limit, and is no reason to call the font read past the allowance.
#[test]
fn a_cmap_past_its_own_limit_is_not_past_the_allowance() {
    let mut doc = Document::new_empty();
    let mut cmap = b"1 beginbfchar <0001> <0058> endbfchar".to_vec();
    cmap.resize(MAX_CMAP_BYTES + 1, b' ');
    let (f, sizes) = subset_type0(&mut doc, &cmap);
    let mut budget = DecodeBudget::new(usize::MAX / 2, 0);
    let m = Metrics::from_dict_within(&doc, &f, &mut budget);
    assert!(!m.past_allowance);
    assert_eq!(m.text_of(1), None);
    assert_eq!(usize::MAX / 2 - budget.remaining(), sizes[..3].iter().sum::<usize>() + MAX_CMAP_BYTES);
}

/// A `/CIDToGIDMap` as long as a map can be, which compresses well, reads the same behind no
/// filter, Flate, or ASCIIHex and Flate: each stage may make as much as the map's limit, so the hex
/// stage's output on top of it doesn't leave it unread. A map past the limit is unread behind each.
#[test]
fn a_full_cid_map_reads_behind_one_or_two_filters() {
    let read = |entries: usize, filters: usize| {
        let mut doc = Document::new_empty();
        let (mut f, _) = subset_type0(&mut doc, b"");
        let map: Vec<u8> = (0..entries).flat_map(|cid| u16::try_from(cid % 2000).unwrap().to_be_bytes()).collect();
        let flate = Stream::flate(Dict::new(), &map);
        let stream = match filters {
            0 => Stream::from_raw(Dict::new(), map),
            1 => flate,
            _ => {
                let hex: String = flate.raw.iter().map(|b| format!("{b:02X}")).chain([">".to_string()]).collect();
                let mut dict = Dict::new();
                dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("FlateDecode")]));
                Stream::from_raw(dict, hex.into_bytes())
            }
        };
        let map = Object::Ref(doc.add(Object::Stream(stream)));
        let Some(Object::Array(mut descendants)) = f.get(b"DescendantFonts").cloned() else { panic!("descendant") };
        if let Some(Object::Dict(d)) = descendants.get_mut(0) {
            d.set(b"CIDToGIDMap".to_vec(), map);
        }
        f.set(b"DescendantFonts".to_vec(), Object::Array(descendants));
        Metrics::from_dict(&doc, &f).outlined_cids
    };
    let full = read(1 << 16, 1);
    assert!(!full.is_empty());
    assert_eq!(read(1 << 16, 0), full);
    assert_eq!(read(1 << 16, 2), full);
    for filters in 0..=2 {
        assert!(read((1 << 16) + 1, filters).is_empty(), "{filters} filters");
    }
}

/// The editor's program read holds every filter stage to the program's limit: a program behind
/// ASCIIHex and Flate whose Flate stage would make 24 MiB stops at 16 MiB, charged that and what
/// the hex stage made, where the stages' total alone would let it make all 24 MiB. A program
/// within the limit behind the same two filters reads (see also
/// `an_editors_program_reads_behind_ascii_hex_and_flate_within_its_limit`).
#[test]
fn an_editors_program_read_holds_each_stage_to_its_limit() {
    let mut doc = Document::new_empty();
    let flate = Stream::flate(Dict::new(), &vec![b' '; 24 << 20]);
    let hex: Vec<u8> = flate.raw.iter().flat_map(|b| format!("{b:02X}").into_bytes()).chain(*b">").collect();
    let mut dict = Dict::new();
    dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("FlateDecode")]));
    let program = doc.add(Object::Stream(Stream::from_raw(dict, hex)));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let f = font(&mut doc, vec![("Subtype", Object::name("TrueType")), ("FontDescriptor", Object::Dict(descriptor))]);
    let mut budget = DecodeBudget::new(usize::MAX / 2, 0);
    assert!(embedded_sfnt_within(&doc, &f, &mut budget).is_none());
    assert_eq!(usize::MAX / 2 - budget.remaining(), flate.raw.len() + MAX_SFNT);
}

/// Each stage of a stream a font's metrics are read from is held to the stream's limit, as a
/// decode without an allowance holds it: a `/CIDToGIDMap` behind Flate and ASCIIHex whose Flate
/// stage makes hex text past the map's limit is unread, as `Stream::decoded_within` leaves it,
/// though the two stages together make less than twice the limit; with its hex text within the
/// limit it reads.
#[test]
fn each_stage_of_a_metrics_stream_is_held_to_its_limit() {
    let read = |cids: usize| {
        let mut doc = Document::new_empty();
        let (mut f, _) = subset_type0(&mut doc, b"");
        let map: Vec<u8> = (0..cids).flat_map(|cid| u16::try_from(cid % 2000).unwrap().to_be_bytes()).collect();
        let hex: Vec<u8> = map.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("FlateDecode"), Object::name("ASCIIHexDecode")]));
        let stream = Stream::from_raw(dict, Stream::flate(Dict::new(), &hex).raw.to_vec());
        let unread = stream.decoded_within(MAX_CID_TO_GID).is_err();
        let map = Object::Ref(doc.add(Object::Stream(stream)));
        let Some(Object::Array(mut descendants)) = f.get(b"DescendantFonts").cloned() else { panic!("descendant") };
        if let Some(Object::Dict(d)) = descendants.get_mut(0) {
            d.set(b"CIDToGIDMap".to_vec(), map);
        }
        f.set(b"DescendantFonts".to_vec(), Object::Array(descendants));
        (Metrics::from_dict(&doc, &f).outlined_cids.is_empty(), unread)
    };
    // 0.6 of the limit: its hex text, 1.2 of it, is past the limit; the stages make 1.8 of it.
    assert_eq!(read(MAX_CID_TO_GID * 6 / 20), (true, true));
    assert_eq!(read(MAX_CID_TO_GID / 4), (false, false));
}

/// The editor's program is held to its limit as the metrics reads' streams are: a 12 MiB program
/// whose Flate data is about 8 MiB reads behind ASCIIHex and Flate as behind Flate alone, though
/// the two stages make about 20 MiB together. A program of the limit reads behind either, and one a
/// byte past it is unread behind either.
#[test]
fn an_editors_program_reads_behind_ascii_hex_and_flate_within_its_limit() {
    let program = |len: usize, noise: usize| {
        let mut program = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf").to_vec();
        // Bytes Flate can't shrink, then zeros.
        let mut x = 1u32;
        program.extend((0..noise).map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            x.to_be_bytes()[0]
        }));
        program.resize(len, 0);
        program
    };
    let read = |program: &[u8], hex: bool| {
        let mut doc = Document::new_empty();
        let flate = Stream::flate(Dict::new(), program);
        let stream = if hex {
            let digits = b"0123456789ABCDEF";
            let mut data: Vec<u8> = flate.raw.iter().flat_map(|b| [digits[usize::from(b >> 4)], digits[usize::from(b & 15)]]).collect();
            data.push(b'>');
            let mut dict = Dict::new();
            dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("FlateDecode")]));
            Stream::from_raw(dict, data)
        } else {
            flate
        };
        let mut descriptor = Dict::new();
        descriptor.set(b"FontFile2".to_vec(), Object::Ref(doc.add(Object::Stream(stream))));
        let simple = font(&mut doc, vec![("FontDescriptor", Object::Dict(descriptor))]);
        embedded_sfnt(&doc, &simple)
    };
    let near = program(12 << 20, 8 << 20);
    let full = program(MAX_SFNT, 0);
    for hex in [false, true] {
        assert_eq!(read(&near, hex).as_deref(), Some(near.as_slice()), "behind ASCIIHex too: {hex}");
        assert_eq!(read(&full, hex).as_deref(), Some(full.as_slice()), "behind ASCIIHex too: {hex}");
        assert!(read(&program(MAX_SFNT + 1, 0), hex).is_none(), "behind ASCIIHex too: {hex}");
    }
}

/// `data` in rows of `columns` bytes behind Flate and a PNG predictor, each row tagged Up (the
/// difference from the row above), so read without the predictor it is garbled; with its
/// `/DecodeParms` given by reference or direct.
fn predicted(doc: &mut Document, data: &[u8], columns: usize, by_reference: bool) -> Object {
    let mut above = vec![0u8; columns];
    let mut rows = Vec::new();
    for row in data.chunks(columns) {
        rows.push(2);
        rows.extend(row.iter().zip(&above).map(|(b, a)| b.wrapping_sub(*a)));
        above[..row.len()].copy_from_slice(row);
    }
    let mut s = Stream::flate(Dict::new(), &rows);
    let parms: Dict =
        [(b"Predictor".to_vec(), Object::Int(12)), (b"Columns".to_vec(), Object::Int(i64::try_from(columns).unwrap()))].into_iter().collect();
    let parms = if by_reference { Object::Ref(doc.add(Object::Dict(parms))) } else { Object::Dict(parms) };
    s.dict.set(b"DecodeParms".to_vec(), parms);
    Object::Ref(doc.add(Object::Stream(s)))
}

/// A stream whose `/DecodeParms` are given by reference reads as it does with them direct: a PNG
/// predictor's tag bytes are taken out of the map, the CMap and the editor's program, not left in.
#[test]
fn parameters_given_by_reference_are_read() {
    let read = |by_reference: bool| {
        let mut doc = Document::new_empty();
        let (mut f, _) = subset_type0(&mut doc, b"");
        let map: Vec<u8> = (0..1000u16).flat_map(u16::to_be_bytes).collect();
        let map = predicted(&mut doc, &map, 2, by_reference);
        let mut cmap = b"1 beginbfchar <0001> <0058> endbfchar".to_vec();
        cmap.resize(48, b' ');
        let unicode = predicted(&mut doc, &cmap, 16, by_reference);
        let Some(Object::Array(mut descendants)) = f.get(b"DescendantFonts").cloned() else { panic!("descendant") };
        if let Some(Object::Dict(d)) = descendants.get_mut(0) {
            d.set(b"CIDToGIDMap".to_vec(), map);
        }
        f.set(b"DescendantFonts".to_vec(), Object::Array(descendants));
        f.set(b"ToUnicode".to_vec(), unicode);
        let m = Metrics::from_dict(&doc, &f);
        (m.outlined_cids.clone(), m.text_of(1).map(Cow::into_owned))
    };
    let direct = read(false);
    assert!(!direct.0.is_empty());
    assert_eq!(direct.1.as_deref(), Some("X"));
    assert_eq!(read(true), direct);
    let mut program = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf").to_vec();
    program.resize(program.len().next_multiple_of(4), 0);
    for by_reference in [false, true] {
        let mut doc = Document::new_empty();
        let file = predicted(&mut doc, &program, 4, by_reference);
        let mut descriptor = Dict::new();
        descriptor.set(b"FontFile2".to_vec(), file);
        let simple = font(&mut doc, vec![("FontDescriptor", Object::Dict(descriptor))]);
        assert_eq!(embedded_sfnt(&doc, &simple).as_deref(), Some(program.as_slice()), "by reference: {by_reference}");
    }
}

/// Fonts that each name one large ToUnicode share the allowance they are read within: it pays for
/// as many readings as it holds, and the rest are left unread, where each reading used to decode
/// it afresh.
#[test]
fn fonts_naming_one_cmap_share_one_allowance() {
    let mut doc = Document::new_empty();
    let mut cmap = b"1 beginbfchar <41> <0058> endbfchar".to_vec();
    cmap.resize(1_000_000, b' ');
    let unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), &cmap)));
    let fonts: Vec<Dict> = (0..100)
        .map(|_| {
            font(&mut doc, vec![("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Helvetica")), ("ToUnicode", Object::Ref(unicode))])
        })
        .collect();
    let mut budget = DecodeBudget::new(64 << 20, 0);
    let read: Vec<Metrics> = fonts.iter().map(|f| Metrics::from_dict_within(&doc, f, &mut budget)).collect();
    let whole = read.iter().filter(|m| !m.past_allowance).count();
    assert_eq!(whole, (64 << 20) / cmap.len());
    assert!(read[..whole].iter().all(|m| m.text_of(0x41).as_deref() == Some("X")));
    assert!(read[whole..].iter().all(|m| m.text_of(0x41).as_deref() == Some("A")), "the encoding's meaning, unread past the allowance");
    assert_eq!(budget.remaining(), 0);
}

/// The names read once give what `Post::glyph_name` gives, for every glyph of a font with its
/// own names (Inter's version 2.0 table), and none past the last glyph.
#[test]
fn post_names_read_once_match_the_table() {
    use skrifa::raw::TableProvider as _;
    let face = skrifa::FontRef::new(include_bytes!("../../../../assets/fonts/Inter-Regular.ttf")).unwrap();
    let post = face.post().unwrap();
    let custom = post_custom_names(&post);
    assert!(custom.len() > 100, "Inter names glyphs of its own: {}", custom.len());
    let glyphs = face.maxp().unwrap().num_glyphs();
    for g in 0..glyphs {
        assert_eq!(post_name(&post, &custom, g), post.glyph_name(skrifa::raw::types::GlyphId16::new(g)), "glyph {g}");
    }
    assert_eq!(post_name(&post, &custom, glyphs), None);
}

/// #3962: a symbolic TrueType subset with no `/Encoding` whose codes are glyph positions in
/// the Macintosh order, 29 below ASCII, mapped by a Mac (1,0) cmap. Its text is what the
/// `post` table names the glyphs its codes select, and typing encodes back to those codes.
/// A (3,0) symbol cmap at 0xF000 + code is read too: code 0x20, `.notdef` in the Mac table,
/// shows `k` there, code 0xC0 `é`, and code 0xC1 the variant `a.1`, whose suffix is dropped.
/// Records are read in order, each with its offsets, as the renderer reads them: 0xC2 is the
/// symbol cmap's `x`, not a later (3,1) cmap's `z`.
#[test]
fn a_symbolic_truetype_font_without_an_encoding_reads_its_glyph_names() {
    use skrifa::{MetadataProvider as _, raw::TableProvider as _};
    const TEXT: &str = "Solid state";
    let original = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
    let face = skrifa::FontRef::new(original).unwrap();
    let charmap = face.charmap();
    let post = face.post().unwrap();
    let variant = (1..face.maxp().unwrap().num_glyphs())
        .find(|&g| post.glyph_name(skrifa::raw::types::GlyphId16::new(g)) == Some("a.1"))
        .expect("Inter has a.1");
    // A glyph whose name has no Unicode value: its code reads as WinAnsi, as the renderer reads it.
    let unnamed = (1..face.maxp().unwrap().num_glyphs())
        .find(|&g| post.glyph_name(skrifa::raw::types::GlyphId16::new(g)).is_some_and(|n| !n.starts_with('.') && glyph_text(n).is_none()))
        .expect("Inter has a glyph whose name has no text");
    // A format 6 Mac cmap over codes 3..=0x57: each letter at its code minus 29.
    let (first, last) = (3u16, 0x57u16);
    let mut glyphs = vec![0u16; usize::from(last - first + 1)];
    for c in TEXT.chars() {
        glyphs[usize::from(c as u16 - 29 - first)] = u16::try_from(charmap.map(c).unwrap().to_u32()).unwrap();
    }
    let mac_len = 10 + 2 * glyphs.len() as u16;
    let gid = |c: char| u16::try_from(charmap.map(c).unwrap().to_u32()).unwrap();
    let mut mac = vec![0, 6];
    mac.extend_from_slice(&mac_len.to_be_bytes());
    mac.extend_from_slice(&[0, 0]);
    mac.extend_from_slice(&first.to_be_bytes());
    mac.extend_from_slice(&(glyphs.len() as u16).to_be_bytes());
    glyphs.iter().for_each(|g| mac.extend_from_slice(&g.to_be_bytes()));
    // A format 4 subtable: one segment per code, then the closing 0xFFFF segment.
    let format4 = |map: &[(u16, u16)]| {
        let segments: Vec<(u16, u16)> = map.iter().copied().chain([(0xFFFF, 0)]).collect();
        let n = segments.len() as u16;
        let words: Vec<u16> = [4, 16 + 8 * n, 0, 2 * n, 2, 0, 2 * n - 2]
            .into_iter()
            .chain(segments.iter().map(|s| s.0))
            .chain([0])
            .chain(segments.iter().map(|s| s.0))
            .chain(segments.iter().map(|&(code, glyph)| if code == 0xFFFF { 1 } else { glyph.wrapping_sub(code) }))
            .chain(segments.iter().map(|_| 0))
            .collect();
        words.iter().flat_map(|w| w.to_be_bytes()).collect::<Vec<u8>>()
    };
    // A glyph whose name the table spells `uni1eae`, lowercase, below: the glyph list
    // specification's hex digits are uppercase (ISO 32000-2 §9.10.2), so it names nothing.
    let lowercase = (1..face.maxp().unwrap().num_glyphs())
        .find(|&g| post.glyph_name(skrifa::raw::types::GlyphId16::new(g)) == Some("uni1EAE"))
        .expect("Inter has uni1EAE");
    // Symbol (3,0): 0xF020 → k, 0xF0C0 → é, 0xF0C1 → a.1, 0xF0C2 → x, 0xF0C4 → the unnamed glyph,
    // 0xF0C5 → the one spelt in lowercase.
    let symbol = format4(&[
        (0xF020, gid('k')),
        (0xF0C0, gid('é')),
        (0xF0C1, variant),
        (0xF0C2, gid('x')),
        (0xF0C3, gid('w')),
        (0xF0C4, unnamed),
        (0xF0C5, lowercase),
    ]);
    // Unicode (3,1), after it: 0xC2 → z. The renderer reads the (3,0) record, offsets and
    // all, before this one, so code 0xC2 is `x`.
    let unicode = format4(&[(0xC2, gid('z'))]);
    // A Unicode-platform (0,3) record, first, which the renderer doesn't read: 0xC3 → q there,
    // so code 0xC3 is the symbol cmap's `w`.
    let platform0 = format4(&[(0xC3, gid('q'))]);
    // Four records, in platform order as the cmap table requires.
    let at = |n: usize| (36 + n) as u32;
    let mut cmap = vec![0, 0, 0, 4];
    for (platform, encoding, offset) in [
        (0u16, 3u16, at(0)),
        (1, 0, at(platform0.len())),
        (3, 0, at(platform0.len() + mac.len())),
        (3, 1, at(platform0.len() + mac.len() + symbol.len())),
    ] {
        cmap.extend_from_slice(&platform.to_be_bytes());
        cmap.extend_from_slice(&encoding.to_be_bytes());
        cmap.extend_from_slice(&offset.to_be_bytes());
    }
    cmap.extend(platform0);
    cmap.extend(mac);
    cmap.extend(symbol);
    cmap.extend(unicode);
    let mut subset = original.to_vec();
    let count = u16::from_be_bytes(subset[4..6].try_into().unwrap()) as usize;
    let entry = (0..count).map(|i| 12 + i * 16).find(|i| &subset[*i..*i + 4] == b"cmap").unwrap();
    let offset = subset.len() as u32;
    subset[entry + 8..entry + 12].copy_from_slice(&offset.to_be_bytes());
    subset[entry + 12..entry + 16].copy_from_slice(&(cmap.len() as u32).to_be_bytes());
    subset.extend_from_slice(&cmap);
    // The `post` table's own spelling of that name, a Pascal string, lowercased in place.
    let spelt = b"\x07uni1EAE";
    assert_eq!(subset.windows(spelt.len()).filter(|w| w == spelt).count(), 1);
    let spot = subset.windows(spelt.len()).position(|w| w == spelt).unwrap();
    subset[spot..spot + spelt.len()].copy_from_slice(b"\x07uni1eae");
    let codes: Vec<u8> = TEXT.bytes().map(|b| b - 29).collect();
    let metrics = |flags: i64, encoding: Option<Object>| {
        let mut doc = Document::new_empty();
        let program = doc.add(Object::Stream(Stream::flate(Dict::new(), &subset)));
        let mut descriptor = Dict::new();
        descriptor.set(b"Flags".to_vec(), Object::Int(flags));
        descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
        let mut entries = vec![("Subtype", Object::name("TrueType")), ("FontDescriptor", Object::Dict(descriptor))];
        if let Some(encoding) = encoding {
            entries.push(("Encoding", encoding));
        }
        let f = font(&mut doc, entries);
        Metrics::from_dict(&doc, &f)
    };
    let symbolic = metrics(4, None);
    assert_eq!(symbolic.decode(&codes), TEXT);
    // The same font with its `post` version halves swapped (`00 00 00 02`) reads the same,
    // as the renderer reads it (#2946).
    let post = (0..count).map(|i| 12 + i * 16).find(|i| &subset[*i..*i + 4] == b"post").unwrap();
    let at = u32::from_be_bytes(subset[post + 8..post + 12].try_into().unwrap()) as usize;
    assert_eq!(subset[at..at + 4], [0, 2, 0, 0], "Inter's post is version 2.0");
    let mut swapped = subset.clone();
    swapped[at..at + 4].copy_from_slice(&[0, 0, 0, 2]);
    let mut doc = Document::new_empty();
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), &swapped)));
    let mut descriptor = Dict::new();
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let f = font(&mut doc, vec![("Subtype", Object::name("TrueType")), ("FontDescriptor", Object::Dict(descriptor))]);
    assert_eq!(Metrics::from_dict(&doc, &f).decode(&codes), TEXT, "a swapped post version");
    assert_eq!(symbolic.decode(&[0x20, 0xC0, 0xC1, 0xC2, 0xC3]), "kéaxw");
    assert_eq!(symbolic.decode(&[0xC4]), "Ä", "a glyph name with no text reads as WinAnsi, not Standard's `tilde`");
    assert_eq!(symbolic.decode(&[0xC5]), "Å", "`uni1eae` names nothing, so the code reads as WinAnsi, not U+1EAE");
    assert_eq!(symbolic.encode("Solid").as_deref(), Some(&codes[..5]), "typing sets the same glyphs");
    // A named encoding, or the Nonsymbolic flag, says what the codes are: they read through it.
    assert_eq!(metrics(4, Some(Object::name("WinAnsiEncoding"))).decode(&codes), "6ROLGVWDWH");
    // `/Differences` still names its codes: 0x36 shows `A` there, not the glyph's `S`.
    let mut differences = Dict::new();
    differences.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(0x36), Object::name("A")]));
    assert_eq!(metrics(4, Some(Object::Dict(differences))).decode(&codes), "Aolid state");
    assert_eq!(metrics(32, None).decode(&codes), "6ROLGVWDWH");
    // A font named as a symbol font reports nothing, as before: its glyph names can't be
    // trusted to be Latin either.
    let mut doc = Document::new_empty();
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), &subset)));
    let mut descriptor = Dict::new();
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("TrueType")),
            ("BaseFont", Object::name("KNMDCP+WPTypographicSymbols")),
            ("FontDescriptor", Object::Dict(descriptor)),
        ],
    );
    assert_eq!(Metrics::from_dict(&doc, &f).decode(&codes), "");
}

/// The glyph list specification's `uni` and `u` forms spell their hex digits in uppercase
/// (ISO 32000-2 §9.10.2); list names keep their own case.
#[test]
fn glyph_names_with_lowercase_hex_digits_name_nothing() {
    for name in ["uni1EAE", "u1F600", "a", "ecircumflex", "f_i", "a.ss01", "uni0041_uni0042"] {
        assert!(uppercase_hex_forms(name), "{name}");
    }
    for name in ["uni1eae", "u1f600", "uni20ac.alt", "f_uni20ac", "uni0041_uni00e9"] {
        assert!(!uppercase_hex_forms(name), "{name}");
    }
}

fn metrics_with_flags(flags: Option<Object>) -> Metrics {
    let mut doc = Document::new_empty();
    let mut descriptor = Dict::new();
    if let Some(flags) = flags {
        descriptor.set(b"Flags".to_vec(), flags);
    }
    let font = font(&mut doc, vec![("Subtype", Object::name("TrueType")), ("FontDescriptor", Object::Dict(descriptor))]);
    Metrics::from_dict(&doc, &font)
}

#[test]
fn font_descriptor_flags_stay_within_the_u32_domain() {
    let valid = metrics_with_flags(Some(Object::Int(64 | 262_144)));
    assert!(valid.italic && valid.bold);

    for flags in [Some(Object::Int(-1)), Some(Object::Int(4_294_967_296)), Some(Object::Bool(true)), None] {
        let metrics = metrics_with_flags(flags);
        assert!(!metrics.italic && !metrics.bold);
    }
}

#[test]
fn type3_descriptor_names_the_font_without_changing_its_metrics() {
    let mut doc = Document::new_empty();
    let mut f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("Type3")),
            (
                "FontMatrix",
                Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
            ),
            ("FontBBox", Object::Array(vec![Object::Int(0), Object::Int(-300), Object::Int(1000), Object::Int(1000)])),
        ],
    );
    let before = Metrics::from_dict(&doc, &f);
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name("ExampleMincho-Regular"));
    descriptor.set(b"Flags".to_vec(), Object::Int(6));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    let reference = doc.add(Object::Dict(descriptor));
    f.set(b"FontDescriptor".to_vec(), Object::Ref(reference));
    let after = Metrics::from_dict(&doc, &f);
    assert_eq!(after.base_font, "ExampleMincho-Regular");
    assert_eq!((after.ascent, after.descent, after.scale), (before.ascent, before.descent, before.scale));
    assert!(!after.bold && !after.italic && !after.subset);
    f.set(b"BaseFont".to_vec(), Object::name("ExplicitName"));
    assert_eq!(Metrics::from_dict(&doc, &f).base_font, "ExplicitName", "an explicit BaseFont takes precedence");
}

/// A Type 3 font with `matrix`, `bbox` and one glyph description per entry of `procs`.
fn type3(doc: &mut Document, matrix: [f64; 6], bbox: [i64; 4], procs: &[&str]) -> Dict {
    let mut charprocs = Dict::new();
    for (i, proc) in procs.iter().enumerate() {
        let glyph = doc.add(Object::Stream(Stream::flate(Dict::new(), proc.as_bytes())));
        charprocs.set(format!("g{i}").into_bytes(), Object::Ref(glyph));
    }
    font(
        doc,
        vec![
            ("Subtype", Object::name("Type3")),
            ("FontMatrix", Object::Array(matrix.iter().map(|&v| Object::Real(v)).collect())),
            ("FontBBox", Object::Array(bbox.iter().map(|&v| Object::Int(v)).collect())),
            ("CharProcs", Object::Dict(charprocs)),
        ],
    )
}

/// ISO 32000-2 §9.6.4: a bitmap font whose `/FontBBox` is all zero (no assumption about glyph
/// sizes) is as high as the boxes its glyphs declare with `d1`, not a fixed 0.6 of the size.
#[test]
fn a_type3_font_is_as_high_as_its_d1_glyph_boxes() {
    let mut doc = Document::new_empty();
    let unit = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let f = type3(&mut doc, unit, [0, 0, 0, 0], &["100 0 0 0 100 100 d1\nq 100 0 0 100 0 0 cm BI /W 1 /H 1 /IM true ID \x00 EI Q"]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (100.0, 0.0));
    // Shown at `0.12 Tf`, the glyph is 12 points high.
    assert!((m.ascent * 0.12 - 12.0).abs() < 1e-9);

    // The union of every glyph's box, leading comments and white space allowed; `d0` glyphs
    // declare none, and a non-numeric operand isn't a box.
    let f = type3(&mut doc, unit, [0, 0, 0, 0], &["% c\n 50 0 0 -20 40 60 d1", "60 0 d0", "60 0 0 -30 40 x d1", "60 0 0 0 40 80 d1"]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (80.0, -20.0));

    // A wrong but non-zero `/FontBBox` (one unit high) gives way to the glyphs.
    let f = type3(&mut doc, unit, [0, 0, 1, 1], &["100 0 0 -10 100 90 d1"]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (90.0, -10.0));

    // No `d1` anywhere: the `/FontBBox` still decides, scaled by the matrix.
    let f = type3(&mut doc, [0.001, 0.0, 0.0, 0.001, 0.0, 0.0], [0, -300, 1000, 900], &["600 0 d0"]);
    let m = Metrics::from_dict(&doc, &f);
    assert!((m.ascent - 0.9).abs() < 1e-12 && (m.descent + 0.3).abs() < 1e-12, "{} {}", m.ascent, m.descent);
}

/// Small glyphs keep their small box: there is no 0.5 em / 0.1 em minimum for `d1` boxes, which
/// made a 58-point line of 0.15 em glyphs 35 points high. The shape of a scanned-text bitmap
/// font: matrix 0.0042, `/FontBBox` all zero, `d1` boxes from -8 to 33 units.
#[test]
fn small_type3_glyphs_keep_their_d1_height() {
    let mut doc = Document::new_empty();
    let small = [0.0042, 0.0, 0.0, 0.0042, 0.0, 0.0];
    let f = type3(&mut doc, small, [0, 0, 0, 0], &["13 0 2 19 9 33 d1", "26 0 2 -8 24 28 d1", "13 0 0 0 1 1 d1"]);
    let m = Metrics::from_dict(&doc, &f);
    assert!((m.ascent - 33.0 * 0.0042).abs() < 1e-12 && (m.descent + 8.0 * 0.0042).abs() < 1e-12, "{} {}", m.ascent, m.descent);
    // Glyphs that all sit above the baseline still reach down to it.
    let f = type3(&mut doc, small, [0, 0, 0, 0], &["13 0 2 19 9 33 d1"]);
    let m = Metrics::from_dict(&doc, &f);
    assert!((m.ascent - 33.0 * 0.0042).abs() < 1e-12 && m.descent == 0.0, "{} {}", m.ascent, m.descent);
}

/// A font matrix that turns glyph space over (`[1 0 0 -1 0 0]`, drawn under a flipped text
/// matrix) puts the top of the glyph box at text-space y -top: the box is as high as before,
/// upside down, not clamped to a sliver.
#[test]
fn a_flipped_type3_font_matrix_keeps_the_glyph_height() {
    let mut doc = Document::new_empty();
    let flipped = [1.0, 0.0, 0.0, -1.0, 0.0, 0.0];
    let f = type3(&mut doc, flipped, [-4, -18, 51, 57], &["37 0 2 -1 34 38 d1", "42 0 2 -18 40 38 d1", "46 0 3 -1 42 57 d1"]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (18.0, -57.0));
    let f = type3(&mut doc, flipped, [-4, -18, 51, 57], &[]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (18.0, -57.0), "the /FontBBox, turned over the same way");
}

/// Glyph descriptions are untrusted: unreadable ones, a box of infinities, a degenerate matrix
/// and a font with more glyphs than are read all leave finite metrics.
#[test]
fn type3_glyph_boxes_are_read_within_bounds() {
    let mut doc = Document::new_empty();
    let unit = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    for procs in [&["inf 0 0 -inf 1 inf d1"][..], &["d1"], &["1 2 3 d1"], &[""], &["1 0 0 0 1 0 d1"], &["1e308 0 -1e308 -1e308 1e308 1e308 d1"]] {
        let f = type3(&mut doc, unit, [0, 0, 0, 0], procs);
        let m = Metrics::from_dict(&doc, &f);
        assert!(m.ascent > 0.0 && m.descent < 0.0, "{procs:?}");
        assert!(m.ascent <= MAX_TYPE3_EXTENT && m.descent >= -MAX_TYPE3_EXTENT, "{procs:?}");
    }
    // A matrix with no y scale gives the box no height: the fallback metrics stand.
    let f = type3(&mut doc, [1.0, 0.0, 0.0, 0.0, 0.0, 0.0], [0, 0, 0, 0], &["1 0 0 0 1 100 d1"]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (Metrics::fallback().ascent, Metrics::fallback().descent));
    // Glyphs past the first MAX_TYPE3_GLYPHS are not read.
    let mut procs = vec!["1 0 0 0 1 10 d1"; MAX_TYPE3_GLYPHS];
    procs.push("1 0 0 0 1 1000 d1");
    let f = type3(&mut doc, unit, [0, 0, 0, 0], &procs);
    assert_eq!(Metrics::from_dict(&doc, &f).ascent, 10.0);
}

/// `/Differences` with the glyph names `names` from code 0, set on the Type 3 font `f`.
fn with_differences(f: &mut Dict, names: &[String]) {
    let mut items = vec![Object::Int(0)];
    items.extend(names.iter().map(|n| Object::name(n)));
    let mut enc = Dict::new();
    enc.set(b"Differences".to_vec(), Object::Array(items));
    f.set(b"Encoding".to_vec(), Object::Dict(enc));
}

/// The glyph descriptions the encoding reaches are the ones read, since its `/Differences`
/// "shall specify the complete character encoding" (ISO 32000-2 Table 110): a drawn glyph past
/// the first [`MAX_TYPE3_GLYPHS`] descriptions still counts.
#[test]
fn type3_glyph_boxes_are_read_for_the_glyphs_the_encoding_reaches() {
    let mut doc = Document::new_empty();
    let mut procs = vec!["1 0 0 0 1 10 d1"; MAX_TYPE3_GLYPHS + 44];
    procs.push("1 0 0 0 1 1000 d1");
    let mut f = type3(&mut doc, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, 0, 0, 0], &procs);
    with_differences(&mut f, &[format!("g{}", MAX_TYPE3_GLYPHS + 44), "g0".into()]);
    assert_eq!(Metrics::from_dict(&doc, &f).ascent, 1000.0);
}

/// Byte limits on glyph descriptions: one larger than [`MAX_TYPE3_GLYPH_BYTES`] is skipped, its
/// `d1` box unread, and the rest are read; once the font's [`MAX_TYPE3_FONT_BYTES`] run out,
/// the boxes read so far may miss glyphs that are drawn, so a nonzero `/FontBBox` stands.
#[test]
fn type3_glyph_byte_limits_leave_honest_metrics() {
    let mut doc = Document::new_empty();
    let unit = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let pad = |head: &str, len: usize| format!("{head}{}", " ".repeat(len.saturating_sub(head.len())));
    let oversized = pad("1 0 0 0 1 1000 d1", MAX_TYPE3_GLYPH_BYTES + 100);
    let procs = [oversized.as_str(), "1 0 0 0 1 3 d1"];
    let mut f = type3(&mut doc, unit, [0, -2, 10, 5], &procs);
    with_differences(&mut f, &["g0".into(), "g1".into()]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (3.0, 0.0), "the oversized glyph is skipped, the other read");

    let filler = pad("0 0 m", MAX_TYPE3_GLYPH_BYTES - 1000);
    let fillers = MAX_TYPE3_FONT_BYTES / (MAX_TYPE3_GLYPH_BYTES - 1000) + 1;
    let mut procs = vec!["1 0 0 0 1 1 d1"];
    procs.extend(std::iter::repeat_n(filler.as_str(), fillers));
    procs.push("1 0 0 0 1 1000 d1");
    let mut f = type3(&mut doc, unit, [0, -2, 10, 5], &procs);
    let names: Vec<String> = (0..procs.len()).map(|i| format!("g{i}")).collect();
    with_differences(&mut f, &names);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (5.0, -2.0), "the /FontBBox, not the first glyph's box alone");
}

/// A Type 3 font's glyph descriptions are paid for from the allowance its metrics are read within,
/// each what it decodes to: one the allowance can't pay for is left unread, and the metrics say so.
#[test]
fn type3_glyph_boxes_are_paid_for_from_the_allowance() {
    let mut doc = Document::new_empty();
    let procs = ["1 0 0 -20 1 300 d1", "1 0 0 0 1 400 d1"];
    let f = type3(&mut doc, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, 0, 0, 0], &procs);
    let cost: usize = procs.iter().map(|p| p.len()).sum();
    let whole = Metrics::from_dict(&doc, &f);
    assert!(!whole.past_allowance);
    assert_eq!((whole.ascent, whole.descent), (400.0, -20.0));
    let mut budget = DecodeBudget::new(cost, 0);
    assert_eq!(Metrics::from_dict_within(&doc, &f, &mut budget), whole);
    assert_eq!(budget.remaining(), 0);
    let m = Metrics::from_dict_within(&doc, &f, &mut DecodeBudget::new(cost - 1, 0));
    assert!(m.past_allowance, "a byte short, the second description is left unread");
}

/// A Type 3 font measured from its glyphs' `d1` boxes keeps them over a descriptor's `/Ascent`
/// and `/Descent`, which are optional for Type 3 (ISO 32000-2 Table 120).
#[test]
fn type3_d1_glyph_boxes_win_over_descriptor_metrics() {
    let mut doc = Document::new_empty();
    let small = [0.0042, 0.0, 0.0, 0.0042, 0.0, 0.0];
    let mut f = type3(&mut doc, small, [0, 0, 0, 0], &["13 0 2 -8 9 33 d1"]);
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"Ascent".to_vec(), Object::Int(891));
    descriptor.set(b"Descent".to_vec(), Object::Int(-216));
    let reference = doc.add(Object::Dict(descriptor));
    f.set(b"FontDescriptor".to_vec(), Object::Ref(reference));
    let m = Metrics::from_dict(&doc, &f);
    assert!((m.ascent - 33.0 * 0.0042).abs() < 1e-12 && (m.descent + 8.0 * 0.0042).abs() < 1e-12, "{} {}", m.ascent, m.descent);
}

/// A `/Differences` naming more distinct glyphs than [`MAX_TYPE3_GLYPHS`] leaves some unread, so
/// a nonzero `/FontBBox` stands over the boxes of those read.
#[test]
fn type3_glyphs_past_the_limit_leave_the_font_bbox_standing() {
    let mut doc = Document::new_empty();
    let mut procs = vec!["1 0 0 0 1 10 d1"; MAX_TYPE3_GLYPHS];
    procs.push("1 0 0 0 1 1000 d1");
    let mut f = type3(&mut doc, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, -2, 10, 50], &procs);
    let names: Vec<String> = (0..procs.len()).map(|i| format!("g{i}")).collect();
    with_differences(&mut f, &names);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (50.0, -2.0));
}

/// `d1` boxes with no height in text space (all flat) give way to a usable `/FontBBox`.
#[test]
fn flat_type3_glyph_boxes_give_way_to_the_font_bbox() {
    let mut doc = Document::new_empty();
    let f = type3(&mut doc, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, -2, 10, 50], &["1 0 0 5 1 5 d1"]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (50.0, -2.0));
}

/// A Type 3 font measured from its `/FontBBox` keeps that box through its font matrix over a
/// descriptor's `/Ascent` and `/Descent`, which would be read as 1000-unit values.
#[test]
fn a_type3_font_bbox_wins_over_descriptor_metrics() {
    let mut doc = Document::new_empty();
    let small = [0.0042, 0.0, 0.0, 0.0042, 0.0, 0.0];
    let mut f = type3(&mut doc, small, [0, -50, 100, 200], &[]);
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"Ascent".to_vec(), Object::Int(891));
    descriptor.set(b"Descent".to_vec(), Object::Int(-216));
    let reference = doc.add(Object::Dict(descriptor));
    f.set(b"FontDescriptor".to_vec(), Object::Ref(reference));
    let m = Metrics::from_dict(&doc, &f);
    // 200 and -50 units at 0.0042, kept at the 0.5 / -0.1 em floor.
    assert!((m.ascent - 0.84).abs() < 1e-9 && (m.descent + 0.21).abs() < 1e-9, "{} {}", m.ascent, m.descent);
}

/// An `/Encoding` with a `/BaseEncoding` reaches glyphs its `/Differences` doesn't name, whose
/// boxes go unread, so a nonzero `/FontBBox` stands over the boxes of those named.
#[test]
fn a_type3_base_encoding_leaves_the_font_bbox_standing() {
    let mut doc = Document::new_empty();
    let mut f = type3(&mut doc, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], [0, -2, 10, 50], &["1 0 0 0 1 10 d1", "1 0 0 0 1 1000 d1"]);
    let mut enc = Dict::new();
    enc.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
    enc.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(0), Object::name("g0")]));
    f.set(b"Encoding".to_vec(), Object::Dict(enc));
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.ascent, m.descent), (50.0, -2.0));
}

#[test]
fn simple_fonts_decode_and_encode_through_their_encoding() {
    let mut doc = Document::new_empty();
    let f = font(
        &mut doc,
        vec![("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Helvetica")), ("Encoding", Object::name("WinAnsiEncoding"))],
    );
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!(m.decode(b"Caf\xe9 \x80 \x93x\x94"), "Café € “x”");
    assert_eq!(m.encode("Café €").as_deref(), Some(&b"Caf\xe9 \x80"[..]));
    assert_eq!(m.encode("Ω"), None, "not in WinAnsi");
    // Differences rename codes by glyph name.
    let mut enc = Dict::new();
    enc.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(65), Object::name("eacute"), Object::name("uni03A9")]));
    let f = font(&mut doc, vec![("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Custom")), ("Encoding", Object::Dict(enc))]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!(m.decode(b"AB"), "éΩ");
    assert_eq!(m.encode("Ω").as_deref(), Some(&b"B"[..]));
}

/// A standard-14 font with no `/Widths` is measured by the glyph each code draws, not by the
/// glyph WinAnsi gives that code (#2894); `/Widths`, when present, still go by code.
#[test]
fn standard_fonts_without_widths_measure_the_glyph_their_differences_name() {
    let mut doc = Document::new_empty();
    let mut enc = Dict::new();
    enc.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
    let diffs = vec![Object::Int(65), Object::name("W"), Object::Int(87), Object::name("A"), Object::name("uni03A9"), Object::name("not.a.glyph")];
    enc.set(b"Differences".to_vec(), Object::Array(diffs));
    let f =
        font(&mut doc, vec![("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Helvetica")), ("Encoding", Object::Dict(enc.clone()))]);
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!((m.width(65), m.width(87)), (0.944, 0.667), "A and W swap their widths");
    // Omega has no WinAnsi code and the last name reads as nothing: both keep their code's width.
    assert_eq!((m.width(88), m.width(89)), (0.667, 0.667));
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("Type1")),
            ("BaseFont", Object::name("Helvetica")),
            ("FirstChar", Object::Int(87)),
            ("Widths", Object::Array(vec![Object::Int(500)])),
            ("Encoding", Object::Dict(enc)),
        ],
    );
    assert_eq!(Metrics::from_dict(&doc, &f).width(87), 0.5, "/Widths go by code");
}

#[test]
fn subset_fonts_only_encode_the_glyphs_they_have() {
    let mut doc = Document::new_empty();
    // Widths for a, b only (97, 98); c (99) is 0.
    let widths = Object::Array(vec![Object::Int(500), Object::Int(520), Object::Int(0)]);
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("TrueType")),
            ("BaseFont", Object::name("ABCDEF+Arial")),
            ("FirstChar", Object::Int(97)),
            ("Widths", widths),
            ("Encoding", Object::name("WinAnsiEncoding")),
        ],
    );
    let m = Metrics::from_dict(&doc, &f);
    assert!(m.subset);
    assert_eq!(m.encode("ab").as_deref(), Some(&b"ab"[..]));
    assert_eq!(m.encode("abc"), None);
}

/// #6131: pdfTeX writes `/FirstChar` below 32 and `/Widths` of 0 for the codes it never uses, the space
/// among them: the subset has no space glyph, and the words are apart by `TJ` gaps. Writing a typed space as
/// code 32 would glue the words together at width 0, so the font can't encode one and the edit sets the
/// space as a gap instead (as for a subset whose widths stop short of 32).
#[test]
fn a_subset_that_gives_the_space_no_width_cannot_encode_one() {
    // pdfTeX writes Type 1 fonts; the same rule holds for a TrueType subset.
    for subtype in ["TrueType", "Type1"] {
        a_zero_width_space_in_a_subset_of(subtype);
    }
}

fn a_zero_width_space_in_a_subset_of(subtype: &str) {
    let metrics = |base: &str, space: i64| {
        let mut doc = Document::new_empty();
        // Codes 32..=98: the space, then a and b (97, 98) with a width, everything else 0.
        let widths: Vec<Object> = (32..=98)
            .map(|code| {
                Object::Int(match code {
                    32 => space,
                    97 | 98 => 500,
                    _ => 0,
                })
            })
            .collect();
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name(subtype)),
                ("BaseFont", Object::name(base)),
                ("FirstChar", Object::Int(32)),
                ("Widths", Object::Array(widths)),
                ("Encoding", Object::name("WinAnsiEncoding")),
            ],
        );
        Metrics::from_dict(&doc, &f)
    };
    let subset = metrics("ABCDEF+Sample", 0);
    assert!(subset.subset);
    assert_eq!(subset.encode("ab").as_deref(), Some(&b"ab"[..]));
    assert_eq!(subset.encode("a b"), None, "no space glyph: a gap sets it");
    assert_eq!(subset.encode(" "), None);
    assert_eq!(metrics("ABCDEF+Sample", 250).encode("a b").as_deref(), Some(&b"a b"[..]), "a subset that does draw the space");
    assert_eq!(metrics("Sample", 0).encode("a b").as_deref(), Some(&b"a b"[..]), "a full font keeps its space whatever the width says");
}

/// Only a *space* at code 32 is given up for a width of 0: a subset that puts another glyph there (a symbol
/// font, say) keeps encoding it.
#[test]
fn a_subset_keeps_a_zero_width_glyph_that_is_not_a_space_at_code_32() {
    let mut doc = Document::new_empty();
    let widths: Vec<Object> = (32..=98).map(|code| Object::Int(if code == 97 { 500 } else { 0 })).collect();
    let mut enc = Dict::new();
    enc.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(32), Object::name("Z")]));
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("TrueType")),
            ("BaseFont", Object::name("ABCDEF+Sample")),
            ("FirstChar", Object::Int(32)),
            ("Widths", Object::Array(widths)),
            ("Encoding", Object::Dict(enc)),
        ],
    );
    let m = Metrics::from_dict(&doc, &f);
    assert!(m.subset);
    assert_eq!(m.text_of(32).as_deref(), Some("Z"));
    assert_eq!(m.encode("Z").as_deref(), Some(&[32][..]), "code 32 is a glyph here, not a space");
}

#[test]
fn cmap_streams_decode_within_a_size_cap() {
    // A valid CMap padded past the cap (a small Flate stream of whitespace) is not decoded whole.
    let to_unicode = b"1 begincodespacerange <0000> <FFFF> endcodespacerange 1 beginbfrange <0024> <0026> <0041> endbfrange";
    let encoding = b"1 begincodespacerange <0000> <FFFF> endcodespacerange 1 begincidrange <0000> <FFFF> 0 endcidrange";
    for padded in [false, true] {
        let mut doc = Document::new_empty();
        let mut stream = |cmap: &[u8]| {
            let mut data = cmap.to_vec();
            if padded {
                data.resize(MAX_CMAP_BYTES + 1, b' ');
            }
            Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), &data))))
        };
        let (tu, enc) = (stream(to_unicode), stream(encoding));
        let mut desc = Dict::new();
        desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("Type0")),
                ("Encoding", enc),
                ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])),
                ("ToUnicode", tu),
            ],
        );
        let m = Metrics::from_dict(&doc, &f);
        assert_eq!(m.decode(&[0, 0x24]) == "A", !padded, "ToUnicode, padded: {padded}");
        assert_eq!(matches!(m.codes, Codes::Ranges(_)), !padded, "Encoding, padded: {padded}");
    }
}

#[test]
fn cmap_streams_read_their_parameters_given_by_reference() {
    // Both maps behind a predictor whose `/DecodeParms` is a reference: read without it, every
    // row keeps its tag byte inside the map's tokens.
    let to_unicode = b"1 begincodespacerange <0000> <FFFF> endcodespacerange 1 beginbfrange <0024> <0026> <0041> endbfrange";
    let encoding = b"1 begincodespacerange <0000> <FFFF> endcodespacerange 1 begincidrange <0000> <FFFF> 0 endcidrange";
    let mut doc = Document::new_empty();
    let (tu, enc) = (behind_parms_by_reference(&mut doc, to_unicode), behind_parms_by_reference(&mut doc, encoding));
    let mut desc = Dict::new();
    desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    let f = font(
        &mut doc,
        vec![("Subtype", Object::name("Type0")), ("Encoding", enc), ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])), ("ToUnicode", tu)],
    );
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!(m.decode(&[0, 0x24]), "A", "ToUnicode");
    assert!(matches!(m.codes, Codes::Ranges(_)), "Encoding");
}

/// A Type 0 subset over Inter with `/DW` alone, whose CIDs 1 to 7 draw "A B C D" (ToUnicode
/// gives CIDs 1 and 2), its program and its `/CIDToGIDMap` each behind a predictor whose
/// `/DecodeParms` is a reference, or behind Flate alone.
fn inter_subset(program_by_reference: bool, map_by_reference: bool) -> Metrics {
    use skrifa::MetadataProvider as _;
    let bytes = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
    let face = skrifa::FontRef::new(bytes).unwrap();
    let gid = |c: char| u16::try_from(face.charmap().map(c).unwrap().to_u32()).unwrap();
    let gids: Vec<u8> = std::iter::once(0).chain("A B C D".chars().map(gid)).flat_map(u16::to_be_bytes).collect();
    let stream = |doc: &mut Document, data: &[u8], by_reference: bool| {
        if by_reference { behind_parms_by_reference(doc, data) } else { Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), data)))) }
    };
    let mut doc = Document::new_empty();
    let program = stream(&mut doc, bytes, program_by_reference);
    let map = stream(&mut doc, &gids, map_by_reference);
    let to_unicode = b"1 begincodespacerange <0000> <FFFF> endcodespacerange 2 beginbfchar <0001> <0041> <0002> <0020> endbfchar";
    let to_unicode = stream(&mut doc, to_unicode, false);
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), program);
    let mut desc = Dict::new();
    desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    desc.set(b"DW".to_vec(), Object::Int(600));
    desc.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    desc.set(b"CIDToGIDMap".to_vec(), map);
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("Type0")),
            ("BaseFont", Object::name("ABCDEF+Inter")),
            ("Encoding", Object::name("Identity-H")),
            ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])),
            ("ToUnicode", to_unicode),
        ],
    );
    Metrics::from_dict(&doc, &f)
}

/// A subset with `/DW` alone has a glyph for a CID when the program's glyph it maps to has an
/// outline, so the space, Inter's empty glyph, is refused. Its program, or its `/CIDToGIDMap`,
/// behind parameters given by reference says the same: read without them, no program says
/// which glyphs are empty, or the map points the CIDs at other glyphs.
#[test]
fn a_subsets_program_and_cid_map_read_their_parameters_given_by_reference() {
    let direct = inter_subset(false, false);
    assert!(!direct.outlined_cids.is_empty());
    assert_eq!(direct.encode("A").as_deref(), Some(&[0, 1][..]));
    assert_eq!(direct.encode(" "), None, "the space's glyph is empty");
    for (program, map) in [(true, false), (false, true)] {
        let m = inter_subset(program, map);
        assert_eq!(m.outlined_cids, direct.outlined_cids, "program by reference: {program}, map: {map}");
        assert_eq!(m.encode(" "), None, "program by reference: {program}, map: {map}");
    }
}

#[test]
fn composite_fonts_use_to_unicode() {
    let mut doc = Document::new_empty();
    let cmap = b"/CIDInit /ProcSet findresource begin 1 begincodespacerange <0000> <FFFF> endcodespacerange \
            2 beginbfchar <0003> <0020> <0010> <00E9> endbfchar 1 beginbfrange <0024> <0026> <0041> endbfrange \
            1 beginbfrange <0030> <0031> [<0048> <0069>] endbfrange end";
    let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.to_vec())));
    let mut desc = Dict::new();
    desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    desc.set(
        b"W".to_vec(),
        Object::Array(vec![
            Object::Int(3),
            Object::Array(vec![Object::Int(250)]),
            Object::Int(16),
            Object::Int(16),
            Object::Int(500),
            Object::Int(36),
            Object::Int(49),
            Object::Int(600),
        ]),
    );
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("Type0")),
            ("BaseFont", Object::name("QWERTY+Noto")),
            ("Encoding", Object::name("Identity-H")),
            ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])),
            ("ToUnicode", Object::Ref(tu)),
        ],
    );
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!(m.decode(&[0, 0x24, 0, 0x25, 0, 3, 0, 0x30, 0, 0x31, 0, 0x10]), "AB Hié");
    assert_eq!(m.encode("Hi A").as_deref(), Some(&[0, 0x30, 0, 0x31, 0, 3, 0, 0x24][..]));
    assert_eq!(m.encode("Z"), None);
}

/// Word's Type 0 subsets map CID 0 as well as the real space glyph to U+0020 and give CID 0 a
/// width; CID 0 is `.notdef`, so a typed space takes the real one (#3223).
#[test]
fn composite_fonts_never_encode_with_cid_0() {
    let mut doc = Document::new_empty();
    let cmap = b"/CIDInit /ProcSet findresource begin 1 begincodespacerange <0000> <FFFF> endcodespacerange \
            1 beginbfrange <0000> <0002> [<0020> <004D> <00E1>] endbfrange \
            1 beginbfrange <0005> <0007> [<0065> <0072> <0020>] endbfrange end";
    let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.to_vec())));
    let mut desc = Dict::new();
    desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    let widths = [889, 444, 389, 278, 444, 333, 250].map(Object::Int).to_vec();
    desc.set(b"W".to_vec(), Object::Array(vec![Object::Int(0), Object::Array(vec![Object::Int(778)]), Object::Int(1), Object::Array(widths)]));
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("Type0")),
            ("BaseFont", Object::name("BCDEEE+TimesNewRomanPSMT")),
            ("Encoding", Object::name("Identity-H")),
            ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])),
            ("ToUnicode", Object::Ref(tu)),
        ],
    );
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!(m.encode("Me er").as_deref(), Some(&[0, 1, 0, 5, 0, 7, 0, 5, 0, 6][..]));
    assert_eq!(m.decode(&[0, 1, 0, 7, 0, 2]), "M á");
}

/// WSCAD writes `/ToUnicode /Identity-H` (a name, not a CMap stream) on composite fonts whose
/// codes are UTF-16 code units: the codes read as the same Unicode, as in Acrobat. Controls
/// and lone surrogates map to nothing, and a simple font's name is not a ToUnicode map.
#[test]
fn identity_to_unicode_names_read_codes_as_unicode() {
    let doc = Document::new_empty();
    for name in ["Identity-H", "Identity-V"] {
        let f = font(
            &mut Document::new_empty(),
            vec![
                ("Subtype", Object::name("Type0")),
                ("BaseFont", Object::name("RobotoMono-Regular")),
                ("Encoding", Object::name("Identity-H")),
                ("DescendantFonts", Object::Array(vec![Object::Dict(Dict::new())])),
                ("ToUnicode", Object::name(name)),
            ],
        );
        let m = Metrics::from_dict(&doc, &f);
        assert_eq!(m.decode(&[0, b'Z', 0, 0xFC, 0, b'r', 0x20, 0xAC, 0x4E, 0x2D]), "Zür€中", "{name}");
        assert_eq!(m.text_of(0x0001), None);
        assert_eq!(m.text_of(0x007F), None, "DEL is a control");
        assert_eq!(m.text_of(0x0085), None, "a C1 control");
        assert_eq!(m.text_of(0x00A0).as_deref(), Some("\u{A0}"), "the range resumes after C1");
        assert_eq!(m.text_of(0x007E).as_deref(), Some("~"));
        assert_eq!(m.text_of(0xD800), None);
        assert_eq!(m.text_of(0xFFFD).as_deref(), Some("\u{FFFD}"));
    }
    let simple = font(
        &mut Document::new_empty(),
        vec![("Subtype", Object::name("TrueType")), ("BaseFont", Object::name("Arial")), ("ToUnicode", Object::name("Identity-H"))],
    );
    let m = Metrics::from_dict(&doc, &simple);
    assert_eq!(m.text_of(0x41).as_deref(), Some("A"), "the encoding still applies");
}

/// Chrome's subsets leave the glyphs of `/DW` width out of `/W` (#1443): "s" (CID 0x56) is
/// still in the font, but nothing is when the font gives no default width to point at.
#[test]
fn composite_subset_glyphs_of_the_default_width_are_present() {
    let metrics = |dw: bool| {
        let mut doc = Document::new_empty();
        let cmap = b"/CIDInit /ProcSet findresource begin 1 begincodespacerange <0000> <FFFF> endcodespacerange \
                1 beginbfrange <0055> <0057> <0072> endbfrange end";
        let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.to_vec())));
        let mut desc = Dict::new();
        desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
        if dw {
            desc.set(b"DW".to_vec(), Object::Int(500));
        }
        desc.set(
            b"W".to_vec(),
            Object::Array(vec![Object::Int(0x55), Object::Array(vec![Object::Int(333)]), Object::Int(0x57), Object::Array(vec![Object::Int(277)])]),
        );
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("Type0")),
                ("BaseFont", Object::name("AAAAAA+LiberationSans")),
                ("Encoding", Object::name("Identity-H")),
                ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])),
                ("ToUnicode", Object::Ref(tu)),
            ],
        );
        Metrics::from_dict(&doc, &f)
    };
    let chrome = metrics(true);
    assert_eq!(chrome.encode("rst").as_deref(), Some(&[0, 0x55, 0, 0x56, 0, 0x57][..]));
    assert_eq!(chrome.width(0x56), 0.5);
    assert_eq!(chrome.encode("x"), None, "a character the ToUnicode map doesn't give");
    assert_eq!(metrics(false).encode("s"), None);
}

/// A `/W` run that starts at the last CID stops there instead of overflowing onto CID 0.
#[test]
fn composite_width_run_past_the_last_cid_is_dropped() {
    let widths = |first: i64| {
        let mut doc = Document::new_empty();
        let mut desc = Dict::new();
        desc.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
        desc.set(b"DW".to_vec(), Object::Int(1000));
        desc.set(b"W".to_vec(), Object::Array(vec![Object::Int(first), Object::Array(vec![Object::Int(100), Object::Int(200)])]));
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("Type0")),
                ("Encoding", Object::name("Identity-H")),
                ("DescendantFonts", Object::Array(vec![Object::Dict(desc)])),
            ],
        );
        Metrics::from_dict(&doc, &f)
    };
    let ordinary = widths(0);
    assert_eq!((ordinary.width(0), ordinary.width(1)), (0.1, 0.2));
    let last = widths(i64::from(u32::MAX));
    assert_eq!((last.width(0), last.width(u32::MAX)), (1.0, 0.1));
}

#[test]
fn glyph_names() {
    assert_eq!(glyph_unicode("quotedblleft"), Some('“'));
    assert_eq!(glyph_unicode("uni20AC"), Some('€'));
    assert_eq!(glyph_unicode("u1F600"), Some('😀'));
    assert_eq!(glyph_unicode("nonsense"), None);
}

fn to_unicode(cmap: &[u8]) -> HashMap<u32, String> {
    let mut u = Unicode::default();
    parse_to_unicode(cmap, &mut u);
    let codes: Vec<u32> = u.explicit.keys().copied().chain(u.ranges.iter().flat_map(|r| r.lo..=r.hi)).collect();
    codes.into_iter().filter_map(|c| u.text(c).map(|t| (c, t.into_owned()))).collect()
}

#[test]
fn cmap_tokens_need_no_space_between_them() {
    // Hex strings, brackets and operators may touch; comments and literal strings are skipped.
    let cmap = b"/CIDSystemInfo << /Registry (Adobe (x) \\) beginbfchar) >> def % beginbfchar <0001> <0058>\n\
            2 beginbfchar<0003><0020><0010><00 E9>endbfchar \
            1 beginbfrange<0024><0026><0041>endbfrange 1 beginbfrange<0030><0031>[<0048><0069>]endbfrange";
    let map = to_unicode(cmap);
    let mut got: Vec<_> = map.iter().map(|(c, u)| (*c, u.as_str())).collect();
    got.sort_unstable();
    assert_eq!(got, [(3, " "), (0x10, "é"), (0x24, "A"), (0x25, "B"), (0x26, "C"), (0x30, "H"), (0x31, "i")]);
    let (spaces, cids) = parse_cmap(b"1 begincodespacerange<00><FF>endcodespacerange 1 begincidrange<20><7E>1 endcidrange");
    assert_eq!(spaces, [(1, vec![0], vec![0xff])]);
    assert_eq!(cids, [(0x20, 0x7e, 1)]);
}

#[test]
fn malformed_cmaps_never_panic() {
    // Non-ASCII bytes inside hex strings once split a UTF-8 character and panicked.
    for cmap in [
        "1 beginbfchar <0é0> <0041> <AéB> <0042> endbfchar".as_bytes(),
        "1 beginbfrange <0é0> <00é> <0041> endbfrange".as_bytes(),
        b"1 beginbfchar <0001> <004",
        b"1 beginbfchar <123> <0041> <0002> <zz> <0003> endbfchar",
        b"1 beginbfrange <FFFF> <0001> [<0041>] endbfrange",
        b"1 beginbfrange <0001> <0002> [<0041> (",
        b"1 beginbfrange <0001>",
        b"\xff\xfe beginbfchar \x80<\xc3\xa9> <0041> endbfchar %",
    ] {
        to_unicode(cmap);
        parse_cmap(cmap);
    }
    // Code counters at the top of the range stop instead of overflowing.
    let map = to_unicode(b"1 beginbfrange <FFFFFFFE> <FFFFFFFF> [<0041> <0042> <0043>] endbfrange");
    assert_eq!(map.len(), 2);
    assert_eq!(map.get(&u32::MAX).map(String::as_str), Some("B"));
    // A malformed array entry keeps its place, so the next one maps the next code.
    let map = to_unicode(b"1 beginbfrange <0001> <0003> [<0041> <zz> <0043>] endbfrange");
    assert_eq!((map.get(&2), map.get(&3).map(String::as_str)), (None, Some("C")));
    let mut m = Metrics::fallback();
    m.cid_map = Spans::new(parse_cmap(b"1 begincidrange <0000> <FFFF> 4294967295 endcidrange").1);
    assert_eq!(m.cid(5), u32::MAX);
    let mut doc = Document::new_empty();
    let mut enc = Dict::new();
    enc.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(i64::MAX), Object::name("A"), Object::name("B")]));
    let f = font(&mut doc, vec![("Subtype", Object::name("Type1")), ("Encoding", Object::Dict(enc))]);
    assert_eq!(unicode_map(&doc, &f, false, &mut Reads::new(&doc, &mut DecodeBudget::new(usize::MAX, 0))).text(u32::MAX).as_deref(), Some("B"));
}

#[test]
fn sequential_ranges_past_the_entry_cap_still_map() {
    // 17 ranges of 65 536 codes: 1 114 112 codes from 0.4 KB of CMap, more than MAX_CMAP_ENTRIES
    // (the first 16 ranges use up the whole cap). Each range maps its codes upward from 'A', so
    // the last code of the last range, 0x100019, is 'A' + 0x19 = 'Z'.
    let mut cmap = b"17 beginbfrange ".to_vec();
    for k in 0..17u32 {
        cmap.extend(format!("<{:08X}> <{:08X}> <0041> ", k << 16, (k << 16) | 0xffff).bytes());
    }
    cmap.extend(b"endbfrange");
    let mut doc = Document::new_empty();
    let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap)));
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("Type0")),
            ("BaseFont", Object::name("Wide")),
            ("Encoding", Object::name("Identity-H")),
            ("ToUnicode", Object::Ref(tu)),
        ],
    );
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!(m.text_of(0x0010_0019).as_deref(), Some("Z"));
}

#[test]
fn a_cmap_adds_at_most_max_entries() {
    // 200 ranges of 65 536 codes would be 13 million codes from 4 KB of stream: 200 records.
    let mut cmap = b"200 beginbfrange ".to_vec();
    for k in 0..200u32 {
        cmap.extend(format!("<{:08X}> <{:08X}> <0041> ", k << 16, (k << 16) | 0xffff).bytes());
    }
    cmap.extend(b"endbfrange");
    let mut u = Unicode::default();
    parse_to_unicode(&cmap, &mut u);
    assert_eq!((u.ranges.len(), u.explicit.len()), (200, 0));
    // Explicit entries are what the budget counts: 250 of the 300 here are kept.
    let mut many = b"300 beginbfchar ".to_vec();
    for k in 0..300u32 {
        many.extend(format!("<{k:04X}> <{:04X}> ", 0x4E00 + k).bytes());
    }
    many.extend(b"endbfchar");
    let mut u = Unicode::default();
    parse_to_unicode_within(&many, &mut u, 250);
    assert_eq!((u.explicit.len(), u.text(249).is_some(), u.text(250).is_some()), (250, true, false));
    let mut many = b"begincidrange ".to_vec();
    for k in 0..MAX_CMAP_RANGES + 10 {
        many.extend(format!("<{k:08X}> <{k:08X}> 1 ").bytes());
    }
    assert_eq!(parse_cmap(&many).1.len(), MAX_CMAP_RANGES);
}

#[test]
fn a_range_is_one_record_however_many_codes() {
    let mut u = Unicode::default();
    parse_to_unicode(b"1 beginbfrange <0000> <FFFF> <0041> endbfrange", &mut u);
    assert_eq!((u.ranges.len(), u.explicit.len()), (1, 0));
    assert_eq!(u.text(0xFFFF).as_deref(), Some("@"), "0x41 + 0xFFFF wraps to U+0040");
}

#[test]
fn sequential_ranges_count_against_the_budget() {
    // Range k maps codes k and k + 1 and costs two, so a budget of 250 keeps ranges 0 to 124.
    let mut cmap = Vec::new();
    for k in 0..300u32 {
        cmap.extend(format!("1 beginbfrange <{k:08X}> <{:08X}> <0041> endbfrange ", k + 1).bytes());
    }
    let mut u = Unicode::default();
    parse_to_unicode_within(&cmap, &mut u, 250);
    assert_eq!((u.ranges.len(), u.text(125).is_some(), u.text(126).is_none()), (125, true, true));
}

#[test]
fn explicit_and_range_mappings_override_the_encoding() {
    let mut doc = Document::new_empty();
    let cmap = b"1 beginbfchar <41> <0058> endbfchar 1 beginbfrange <42> <43> <0059> endbfrange";
    let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.to_vec())));
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("Type1")),
            ("BaseFont", Object::name("Custom")),
            ("Encoding", Object::name("WinAnsiEncoding")),
            ("ToUnicode", Object::Ref(tu)),
        ],
    );
    let m = Metrics::from_dict(&doc, &f);
    let shown: Vec<Option<String>> = (0x41..=0x44).map(|c| m.text_of(c).map(|t| t.into_owned())).collect();
    assert_eq!(shown, [Some("X".to_string()), Some("Y".to_string()), Some("Z".to_string()), Some("D".to_string())]);
}

/// Deterministic input for the differential test below.
struct Gen(u64);

impl Gen {
    fn below(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % u64::from(n)) as u32
    }

    /// A destination: a unit from a pool of edge cases, a surrogate pair, or a lone high surrogate.
    fn units(&mut self) -> Vec<u16> {
        const POOL: [u16; 12] = [0x41, 0x42, 0xE9, 0x0, 0xFFFD, 0xFFFE, 0xFFFF, 0x100, 0x3042, 0xD800, 0xDC00, 0xDFFF];
        match self.below(6) {
            0 => vec![0xD835, 0xDC00 + self.below(0x400) as u16],
            1 => vec![0xD800, 0x41],
            _ => vec![POOL[self.below(POOL.len() as u32) as usize]],
        }
    }
}

fn hex(units: &[u16]) -> String {
    units.iter().map(|u| format!("{u:04X}")).collect()
}

#[test]
fn lookups_match_the_expanded_mappings() {
    // Random ToUnicode CMaps; `expected` is what expanding their entries in order gives.
    let mut g = Gen(0x2545_f491_4f6c_dd1d);
    for _ in 0..300 {
        let mut cmap = String::from("1 begincodespacerange <0000> <FFFF> endcodespacerange ");
        let mut expected: HashMap<u32, String> = HashMap::new();
        for _ in 0..1 + g.below(12) {
            match g.below(3) {
                0 => {
                    let (code, u) = (g.below(300), g.units());
                    cmap += &format!("1 beginbfchar <{code:04X}> <{}> endbfchar ", hex(&u));
                    expected.insert(code, String::from_utf16_lossy(&u));
                }
                1 => {
                    let (lo, width, u) = (g.below(300), g.below(13), g.units());
                    cmap += &format!("1 beginbfrange <{lo:04X}> <{:04X}> <{}> endbfrange ", lo + width, hex(&u));
                    for k in 0..=width {
                        let mut v = u.clone();
                        if let Some(last) = v.last_mut() {
                            *last = last.wrapping_add(k as u16);
                        }
                        expected.insert(lo + k, String::from_utf16_lossy(&v));
                    }
                }
                _ => {
                    let (lo, width) = (g.below(300), g.below(5));
                    let dests: Vec<Vec<u16>> = (0..=width).map(|_| g.units()).collect();
                    let list: Vec<String> = dests.iter().map(|d| format!("<{}>", hex(d))).collect();
                    cmap += &format!("1 beginbfrange <{lo:04X}> <{:04X}> [{}] endbfrange ", lo + width, list.join(" "));
                    for (k, d) in dests.iter().enumerate() {
                        expected.insert(lo + k as u32, String::from_utf16_lossy(d));
                    }
                }
            }
        }
        let mut doc = Document::new_empty();
        let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.clone().into_bytes())));
        let f = font(
            &mut doc,
            vec![
                ("Subtype", Object::name("Type0")),
                ("BaseFont", Object::name("Gen")),
                ("Encoding", Object::name("Identity-H")),
                ("ToUnicode", Object::Ref(tu)),
            ],
        );
        let m = Metrics::from_dict(&doc, &f);
        for code in 0..=320u32 {
            let got = m.text_of(code).map(|t| t.into_owned());
            assert_eq!(got, expected.get(&code).cloned(), "code {code:#06x} in {cmap}");
        }
        let mut reverse: HashMap<char, u32> = HashMap::new();
        // Code 0 is CID 0, `.notdef`, which no character is encoded with.
        for (&code, text) in expected.iter().filter(|&(&code, _)| code != 0) {
            if let Some(c) = only_char(text) {
                keep(&mut reverse, c, code);
            }
        }
        let mut chars: Vec<char> = expected.values().flat_map(|t| t.chars()).chain(['A', '\u{FFFD}', '\u{0}', '\u{1D400}']).collect();
        chars.sort_unstable();
        chars.dedup();
        for c in chars {
            let want = reverse.get(&c).map(|code| code.to_be_bytes()[2..].to_vec());
            assert_eq!(m.encode(&c.to_string()), want, "char {c:?} in {cmap}");
        }
    }
}

#[test]
fn fffd_lookups_skip_overridden_surrogates() {
    // Each record maps every code to itself, so its surrogate offsets decode to U+FFFD. The last
    // record overrides the surrogates with letters, so each lookup walks past them to U+FFFD itself.
    let mut cmap = "1 beginbfrange <0000> <FFFF> <0000> endbfrange ".repeat(300);
    cmap.push_str("1 beginbfrange <D800> <DFFF> <0041> endbfrange");
    let mut doc = Document::new_empty();
    let tu = doc.add(Object::Stream(Stream::from_raw(Dict::new(), cmap.into_bytes())));
    let f = font(
        &mut doc,
        vec![
            ("Subtype", Object::name("Type0")),
            ("BaseFont", Object::name("Gen")),
            ("Encoding", Object::name("Identity-H")),
            ("ToUnicode", Object::Ref(tu)),
        ],
    );
    let m = Metrics::from_dict(&doc, &f);
    assert_eq!(m.encode("\u{FFFD}"), Some(vec![0xFF, 0xFD]));
}

#[test]
fn hostile_ranges_stay_bounded() {
    // A range wider than 65 536 codes is ignored, a reversed one maps nothing, and a maximal
    // one-code range maps just its own code.
    let mut u = Unicode::default();
    let cmap = b"1 beginbfrange <00000000> <FFFFFFFF> <0041> endbfrange \
            1 beginbfrange <0005> <0001> <0041> endbfrange \
            1 beginbfrange <FFFFFFFF> <FFFFFFFF> <FFFF> endbfrange";
    parse_to_unicode(cmap, &mut u);
    assert_eq!((u.ranges.len(), u.text(u32::MAX).as_deref()), (0, Some("\u{FFFF}")));
}

#[test]
fn low_codes_resolve_to_the_covering_range() {
    let mut u = Unicode::default();
    let cmap = b"1 beginbfrange <0005> <0007> <0041> endbfrange 1 beginbfrange <0064> <0065> <0042> endbfrange \
            1 beginbfrange <0000FFFE> <00010001> <0043> endbfrange";
    parse_to_unicode(cmap, &mut u);
    let covered: Vec<u32> = (0..0x70).filter(|&c| u.covering(c).is_some()).collect();
    assert_eq!(covered, [5, 6, 7, 100, 101]);
    assert_eq!((u.covering(0xFFFF).is_some(), u.covering(0x1_0001).is_some(), u.covering(0x1_0002).is_none()), (true, true, true));
}

#[test]
fn refused_subset_reads_do_not_encode_unverified_glyphs() {
    let mut doc = Document::new_empty();
    let (mut f, sizes) = subset_type0(&mut doc, b"");
    f.set(b"Encoding".to_vec(), Object::name("Identity-H"));
    f.set(b"ToUnicode".to_vec(), Object::name("Identity-H"));
    if let Some(Object::Array(descendants)) = f.get_mut(b"DescendantFonts")
        && let Some(Object::Dict(descendant)) = descendants.first_mut()
    {
        descendant.set(b"DW".to_vec(), Object::Int(1000));
    }
    let whole = Metrics::from_dict(&doc, &f);
    assert!(!whole.past_allowance && !whole.outlined_cids.is_empty());
    assert!(whole.encode("\u{ffff}").is_none());
    // Refuse the program, then the CID map after reading the complete program.
    for allowance in [0, sizes[1]] {
        let partial = Metrics::from_dict_within(&doc, &f, &mut DecodeBudget::new(allowance, 0));
        assert!(partial.past_allowance);
        assert_eq!(partial.encode(""), Some(Vec::new()));
        assert!(partial.encode("\u{ffff}").is_none(), "allowance {allowance} must not turn /DW into proof of a glyph");
    }
}

/// [`cmap_only_sfnt_named`] without a `post` table.
fn cmap_only_sfnt(mac: Option<&[(u8, u8)]>, windows: &[(u16, u16)]) -> Vec<u8> {
    cmap_only_sfnt_named(mac, windows, false)
}

/// An sfnt holding a `cmap` table with these subtables: (1, 0) format 0 mapping codes to glyphs,
/// and (3, 0) format 4 mapping each code point to its glyph, one segment each; and, if `named`, a
/// version 2.0 `post` table.
fn cmap_only_sfnt_named(mac: Option<&[(u8, u8)]>, windows: &[(u16, u16)], named: bool) -> Vec<u8> {
    let be16 = |out: &mut Vec<u8>, n: u16| out.extend_from_slice(&n.to_be_bytes());
    let mut subtables: Vec<((u16, u16), Vec<u8>)> = Vec::new();
    if let Some(map) = mac {
        let mut t = Vec::new();
        for n in [0u16, 262, 0] {
            be16(&mut t, n);
        }
        let mut ids = [0u8; 256];
        for &(code, glyph) in map {
            ids[usize::from(code)] = glyph;
        }
        t.extend(ids);
        subtables.push(((1, 0), t));
    }
    let segments: Vec<(u16, u16)> = windows.iter().copied().chain([(0xFFFF, 0)]).collect();
    let count = segments.len() as u16;
    let mut t = Vec::new();
    for n in [4u16, 16 + 8 * count, 0, 2 * count, 2, 0, 0] {
        be16(&mut t, n);
    }
    segments.iter().for_each(|&(c, _)| be16(&mut t, c));
    be16(&mut t, 0);
    segments.iter().for_each(|&(c, _)| be16(&mut t, c));
    segments.iter().for_each(|&(c, g)| be16(&mut t, if c == 0xFFFF { 1 } else { g.wrapping_sub(c) }));
    segments.iter().for_each(|_| be16(&mut t, 0));
    subtables.push(((3, 0), t));

    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, subtables.len() as u16);
    let mut offset = 4 + 8 * subtables.len() as u32;
    for ((platform, encoding), bytes) in &subtables {
        be16(&mut cmap, *platform);
        be16(&mut cmap, *encoding);
        cmap.extend_from_slice(&offset.to_be_bytes());
        offset += bytes.len() as u32;
    }
    subtables.iter().for_each(|(_, bytes)| cmap.extend(bytes));
    let mut tables = vec![(*b"cmap", cmap)];
    if named {
        // Version 2.0 with no glyphs to name.
        let mut post = vec![0, 2, 0, 0];
        post.resize(34, 0);
        tables.push((*b"post", post));
    }
    let mut out = vec![0, 1, 0, 0];
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend([0; 6]);
    let mut offset = 12 + 16 * tables.len() as u32;
    for (tag, table) in &tables {
        out.extend(tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&(table.len() as u32).to_be_bytes());
        offset += table.len() as u32;
    }
    tables.into_iter().for_each(|(_, table)| out.extend(table));
    out
}

/// A symbolic TrueType font with no `/Encoding` reads a code as Mac OS Roman when its (1, 0) `cmap`
/// subtable selects the glyph the code draws and the glyph sits where the standard Macintosh glyph
/// order puts that character, as the renderer's text layer reads it (vendored hayro-interpret
/// patch (101), ISO 32000-2 §9.6.5.4). Otherwise the codes read as before (Standard): WinAnsi codes
/// in that order (0x92 drawn with `quoteright`), no (1, 0) subtable, one selecting other glyphs
/// than the (3, 0) one, a font flagged non-symbolic, or one that names its glyphs.
#[test]
fn a_symbolic_truetype_font_in_the_macintosh_order_reads_its_codes_as_mac_roman() {
    let read = |program: Vec<u8>, flags: i64, codes: [u32; 2]| {
        let mut doc = Document::new_empty();
        let program = doc.add(Object::Stream(Stream::flate(Dict::new(), &program)));
        let mut descriptor = Dict::new();
        descriptor.set(b"Flags".to_vec(), Object::Int(flags));
        descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
        let f = font(&mut doc, vec![("Subtype", Object::name("TrueType")), ("FontDescriptor", Object::Dict(descriptor))]);
        let u = unicode_map(&doc, &f, false, &mut Reads::new(&doc, &mut DecodeBudget::new(usize::MAX, 0)));
        codes.map(|c| u.text(c).unwrap_or_default().into_owned())
    };
    // `agrave` and `eacute` are glyphs 106 and 112 of the standard Macintosh order, `endash` and
    // `quoteright` 178 and 183.
    let mac_order = cmap_only_sfnt(Some(&[(0x88, 106), (0x8E, 112)]), &[(0xF088, 106), (0xF08E, 112)]);
    assert_eq!(read(mac_order.clone(), 4, [0x88, 0x8E]), ["à", "é"]);
    assert_eq!(read(mac_order, 32, [0x88, 0x8E]), ["", ""], "flagged non-symbolic");
    let winansi = cmap_only_sfnt(Some(&[(0x92, 183), (0x96, 178)]), &[(0xF092, 183), (0xF096, 178)]);
    assert_eq!(read(winansi, 4, [0x92, 0x96]), ["", ""], "WinAnsi codes in the Macintosh order");
    let windows_only = cmap_only_sfnt(None, &[(0xF088, 106), (0xF08E, 112)]);
    assert_eq!(read(windows_only, 4, [0x88, 0x8E]), ["", ""], "no (1, 0) subtable");
    let disagreeing = cmap_only_sfnt(Some(&[(0x88, 112), (0x8E, 106)]), &[(0xF088, 106), (0xF08E, 112)]);
    assert_eq!(read(disagreeing, 4, [0x88, 0x8E]), ["", ""], "a (1, 0) subtable selecting other glyphs");
    // A `post` table of version 2.0 names the glyphs; what it calls them doesn't matter here.
    let named = cmap_only_sfnt_named(Some(&[(0x88, 106), (0x8E, 112)]), &[(0xF088, 106), (0xF08E, 112)], true);
    assert_eq!(read(named, 4, [0x88, 0x8E]), ["", ""], "a font that names its glyphs");
}

/// The Mac OS Roman reading applies only to a font that names no base encoding and isn't Symbol or
/// ZapfDingbats (ISO 32000-2 §9.6.5.4, §9.6.6.1): a named `/BaseEncoding` or `/Encoding` keeps its
/// own reading, a `/Differences` without a base encoding still gets it, and a Symbol font keeps no
/// implicit mapping.
#[test]
fn the_mac_roman_reading_needs_no_base_encoding_and_no_symbolic_builtin() {
    let read = |entries: Vec<(&str, Object)>| {
        let mut doc = Document::new_empty();
        let program = cmap_only_sfnt(Some(&[(0x88, 106), (0x8E, 112)]), &[(0xF088, 106), (0xF08E, 112)]);
        let program = doc.add(Object::Stream(Stream::flate(Dict::new(), &program)));
        let mut descriptor = Dict::new();
        descriptor.set(b"Flags".to_vec(), Object::Int(4));
        descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
        let mut all = vec![("Subtype", Object::name("TrueType")), ("FontDescriptor", Object::Dict(descriptor))];
        all.extend(entries);
        let f = font(&mut doc, all);
        let u = unicode_map(&doc, &f, false, &mut Reads::new(&doc, &mut DecodeBudget::new(usize::MAX, 0)));
        [0x88, 0x8E].map(|c| u.text(c).unwrap_or_default().into_owned())
    };
    assert_eq!(read(vec![]), ["à", "é"]);
    assert_eq!(read(vec![("Encoding", Object::name("WinAnsiEncoding"))]), ["ˆ", "Ž"], "a named base encoding");
    let mut based = Dict::new();
    based.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
    assert_eq!(read(vec![("Encoding", Object::Dict(based))]), ["ˆ", "Ž"], "a /BaseEncoding");
    let mut diffs = Dict::new();
    diffs.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(0x8E), Object::name("ecircumflex")]));
    assert_eq!(read(vec![("Encoding", Object::Dict(diffs))]), ["à", "ê"], "/Differences without a base encoding");
    assert_eq!(read(vec![("BaseFont", Object::name("Symbol"))]), ["", ""], "Symbol");
    assert_eq!(read(vec![("BaseFont", Object::name("ZapfDingbats"))]), ["", ""], "ZapfDingbats");
    // A name it can't read clears the Mac OS Roman reading for its code, as it clears a base
    // encoding's.
    let mut notdef = Dict::new();
    notdef.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(0x8E), Object::name(".notdef")]));
    assert_eq!(read(vec![("Encoding", Object::Dict(notdef))]), ["à", ""], "/Differences naming .notdef");
}

/// The program reads that tell a symbolic TrueType font's codes by its glyph names and as Mac OS
/// Roman are each paid for from the allowance the metrics are read within: one it can't pay for is
/// left unread, and the metrics say so.
#[test]
fn the_symbolic_readings_of_a_program_are_paid_for() {
    let program = cmap_only_sfnt(Some(&[(0x88, 106)]), &[(0xF088, 106)]);
    let mut doc = Document::new_empty();
    let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), &program)));
    let mut descriptor = Dict::new();
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(stream));
    // With `/Widths` the program isn't read for its advances: these are its only reads.
    let entries = vec![
        ("Subtype", Object::name("TrueType")),
        ("FontDescriptor", Object::Dict(descriptor)),
        ("FirstChar", Object::Int(0x88)),
        ("Widths", Object::Array(vec![Object::Int(500)])),
    ];
    let f = font(&mut doc, entries);
    let whole = Metrics::from_dict(&doc, &f);
    assert!(!whole.past_allowance);
    assert_eq!(whole.text_of(0x88).as_deref(), Some("à"));
    let mut budget = DecodeBudget::new(2 * program.len(), 0);
    assert_eq!(Metrics::from_dict_within(&doc, &f, &mut budget), whole);
    assert_eq!(budget.remaining(), 0);
    let m = Metrics::from_dict_within(&doc, &f, &mut DecodeBudget::new(2 * program.len() - 1, 0));
    assert!(m.past_allowance, "a byte short, the Mac OS Roman reading's read is left unread");
    assert_eq!(m.text_of(0x88), None);
}

/// A Type 0 font over Inter's program without its `cmap`, Identity-H, whose `/CIDToGIDMap` sends
/// CID 17 to Inter's Ω and whose ToUnicode says code 17 is Ω; also the program object, the
/// program, Ω's glyph and what the ToUnicode and the map decode to.
fn cmapless_composite(doc: &mut Document) -> (Dict, pdfcraft_cos::ObjRef, Vec<u8>, skrifa::GlyphId, [usize; 2]) {
    use skrifa::{MetadataProvider as _, raw::TableProvider as _};
    let original = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
    let expected = skrifa::FontRef::new(original).unwrap().charmap().map('Ω').unwrap();
    let mut subset = original.to_vec();
    let count = u16::from_be_bytes(subset[4..6].try_into().unwrap()) as usize;
    let records: Vec<_> = subset[12..12 + count * 16].as_chunks::<16>().0.iter().filter(|r| &r[..4] != b"cmap").map(|r| r.to_vec()).collect();
    subset[4..6].copy_from_slice(&(records.len() as u16).to_be_bytes());
    for (index, record) in records.iter().enumerate() {
        subset[12 + index * 16..28 + index * 16].copy_from_slice(record);
    }
    assert!(skrifa::FontRef::new(&subset).unwrap().cmap().is_err());
    let program = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), &subset)));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let mut gids = vec![0; 36];
    gids[34..36].copy_from_slice(&(expected.to_u32() as u16).to_be_bytes());
    let gids = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), &gids)));
    let mut descendant = Dict::new();
    descendant.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    descendant.set(b"CIDToGIDMap".to_vec(), Object::Ref(gids));
    let cmap = b"1 beginbfchar <0011> <03a9> endbfchar";
    let unicode = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), cmap)));
    let source = font(
        doc,
        vec![
            ("Subtype", Object::name("Type0")),
            ("Encoding", Object::name("Identity-H")),
            ("DescendantFonts", Object::Array(vec![Object::Dict(descendant)])),
            ("ToUnicode", Object::Ref(unicode)),
        ],
    );
    (source, program, subset, expected, [cmap.len(), 36])
}

#[test]
fn composite_subset_without_cmap_uses_pdf_glyph_ids_in_editor_and_shaper_without_replacing_the_program() {
    use skrifa::MetadataProvider as _;
    let mut doc = Document::new_empty();
    let (source, program, subset, expected, _) = cmapless_composite(&mut doc);
    let editor = embedded_sfnt(&doc, &source).expect("the source outlines must be usable in the editor");
    assert_eq!(skrifa::FontRef::new(&editor).unwrap().charmap().map('Ω'), Some(expected));
    let shaped = crate::source_shaping::shape(&editor, "Ω").expect("the rebuilt cmap must work in the actual shaping engine too");
    assert_eq!(shaped.glyphs[0].id, expected);
    let Object::Stream(saved) = doc.get(program).as_ref().clone() else { panic!("fixture") };
    assert_eq!(saved.decoded().unwrap(), subset, "the source PDF font program stays byte-for-byte intact");
}

/// The metrics read that rebuilds a composite subset's cmap is paid for from the editor's
/// allowance, as a simple font's is: the program, its ToUnicode, then its `/CIDToGIDMap`. With the
/// ToUnicode left unread no copy is adapted, though what is left would pay for the map.
#[test]
fn adapting_a_composite_subsets_cmap_is_paid_for() {
    use skrifa::MetadataProvider as _;
    let mut doc = Document::new_empty();
    let (source, _, subset, expected, [cmap, map]) = cmapless_composite(&mut doc);
    let cost = subset.len() + cmap + map;
    let mut budget = DecodeBudget::new(cost, 0);
    let editor = embedded_sfnt_within(&doc, &source, &mut budget).unwrap();
    assert_eq!(skrifa::FontRef::new(&editor).unwrap().charmap().map('Ω'), Some(expected));
    assert_eq!(budget.remaining(), 0);
    assert!(cmap > map);
    assert!(embedded_sfnt_within(&doc, &source, &mut DecodeBudget::new(subset.len() + cmap - 1, 0)).is_none());
}

/// The `/CIDToGIDMap` a composite subset's cmap is rebuilt from reads as the font's other streams
/// do: a full map, which compresses well, reads behind ASCIIHex and Flate, though the hex stage's
/// output comes on top of the map.
#[test]
fn a_full_cid_map_rebuilds_a_composite_subsets_cmap_behind_two_filters() {
    use skrifa::MetadataProvider as _;
    let mut doc = Document::new_empty();
    let (mut source, _, _, expected, _) = cmapless_composite(&mut doc);
    let mut map = vec![0u8; MAX_CID_TO_GID];
    map[34..36].copy_from_slice(&(expected.to_u32() as u16).to_be_bytes());
    let flate = Stream::flate(Dict::new(), &map);
    let hex: String = flate.raw.iter().map(|b| format!("{b:02X}")).chain([">".to_string()]).collect();
    let mut dict = Dict::new();
    dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("ASCIIHexDecode"), Object::name("FlateDecode")]));
    let map = Object::Ref(doc.add(Object::Stream(Stream::from_raw(dict, hex.into_bytes()))));
    let Some(Object::Array(mut descendants)) = source.get(b"DescendantFonts").cloned() else { panic!("descendant") };
    if let Some(Object::Dict(d)) = descendants.get_mut(0) {
        d.set(b"CIDToGIDMap".to_vec(), map);
    }
    source.set(b"DescendantFonts".to_vec(), Object::Array(descendants));
    let editor = embedded_sfnt(&doc, &source).expect("the map behind two filters is read");
    assert_eq!(skrifa::FontRef::new(&editor).unwrap().charmap().map('Ω'), Some(expected));
}
