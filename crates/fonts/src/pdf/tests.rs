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
/// ("Mono" would mean Courier, 600 units for every glyph).
#[test]
fn a_font_without_widths_measures_with_its_embedded_program() {
    use pdfcraft_cos::Stream;
    use skrifa::MetadataProvider as _;
    let mut doc = Document::new_empty();
    let bytes = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
    let face = skrifa::FontRef::new(bytes).unwrap();
    let (size, location) = (skrifa::instance::Size::unscaled(), skrifa::instance::LocationRef::default());
    let upem = f64::from(face.metrics(size, location).units_per_em);
    let advance = |c: char| f64::from(face.glyph_metrics(size, location).advance_width(face.charmap().map(c).unwrap()).unwrap()) / upem;
    let program = doc.add(Object::Stream(Stream::flate(Dict::new(), bytes)));
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
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
        assert!((m.width(u32::from(c)) - advance(c)).abs() < 1e-9, "{c}: {} vs {}", m.width(u32::from(c)), advance(c));
    }
    assert!(m.width(u32::from('i')) < 0.4, "not Courier's 0.6");
}

#[test]
fn mac_subset_cmap_uses_pdf_unicode_without_changing_source_program() {
    use skrifa::{MetadataProvider as _, raw::TableProvider as _};
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
    assert_eq!(unicode_map(&doc, &f, false).text(u32::MAX).as_deref(), Some("B"));
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
