//! #3132: embedded TrueType programs go to the Windows spooler under names no installed font has.

use super::*;
use crate::font_names::{distinct, distinct_within, renamed_sfnt};
use crate::spool::spooled;
use pdfcraft_cos::DecodeBudget;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use skrifa::raw::TableProvider as _;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider as _};

/// Inter (OFL, `ATTRIBUTION.toml`) stands in for the subset ReportLab embeds as `AAAAAA+ArialMT`.
const INTER: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");

fn name_of(program: &[u8], id: StringId) -> Option<String> {
    FontRef::new(program).unwrap().localized_strings(id).english_or_first().map(|s| s.to_string())
}

/// A TrueType font over `program`, named `base_font`, as ReportLab writes one; its descriptor is
/// indirect.
fn true_type_font(doc: &mut Document, base_font: &str, program: ObjRef) -> ObjRef {
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(base_font));
    descriptor.set(b"Flags".to_vec(), Object::Int(32));
    descriptor.set(b"FontBBox".to_vec(), Object::Array([-500, -300, 1500, 1000].into_iter().map(Object::Int).collect()));
    for (key, value) in [(b"ItalicAngle".as_slice(), 0), (b"Ascent", 900), (b"Descent", -200), (b"CapHeight", 700), (b"StemV", 80)] {
        descriptor.set(key.to_vec(), Object::Int(value));
    }
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let descriptor = doc.add(Object::Dict(descriptor));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("TrueType"));
    font.set(b"BaseFont".to_vec(), Object::name(base_font));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(descriptor));
    doc.add(Object::Dict(font))
}

fn program_stream(doc: &mut Document) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Length1".to_vec(), Object::Int(INTER.len() as i64));
    doc.add(Object::Stream(Stream::flate(d, INTER)))
}

/// One 200×300 page writing "Hola" in a regular and a bold Arial subset, the way the report's
/// proposal is set, and its print-ready PDF.
fn print_ready() -> Vec<u8> {
    let mut doc = fixture(1);
    let (regular, bold) = (program_stream(&mut doc), program_stream(&mut doc));
    let regular = true_type_font(&mut doc, "AAAAAA+ArialMT", regular);
    let bold = true_type_font(&mut doc, "AAAAAA+Arial-BoldMT", bold);
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(regular));
    fonts.set(b"F2".to_vec(), Object::Ref(bold));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), b"BT /F1 40 Tf 10 200 Td (Hola) Tj ET BT /F2 40 Tf 10 100 Td (Hola) Tj ET")));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| {
        d.remove(b"Annots");
        d.set(b"Resources".to_vec(), Object::Dict(resources));
        d.set(b"Contents".to_vec(), Object::Ref(contents));
    })
    .unwrap();
    let settings = Settings { pages: vec![0], paper: (200.0, 300.0), layout: Layout::Size(SizeMode::Actual), ..Settings::default() };
    impose(&doc, &settings).unwrap()
}

fn pixels(pdf: &[u8]) -> Vec<u8> {
    let page = PageRenderer::new(Arc::new(pdf.to_vec()), RenderConfig::default()).render(RenderRequest {
        page: 0,
        kind: RequestKind::Pixels,
        tile: None,
        scale: 1.0,
        tag: 0,
        preview: false,
    });
    assert_eq!(page.error, None);
    page.rgba[..].to_vec()
}

/// Each TrueType font in `pdf`: its `/BaseFont`, its descriptor's `/FontName`, its decoded
/// program and that program's `/Length1`.
fn true_type_fonts(pdf: &[u8]) -> Vec<(String, String, Vec<u8>, i64)> {
    let doc = Document::open(Arc::new(pdf.to_vec())).unwrap();
    let mut fonts = Vec::new();
    for (_, object) in doc.scan_objects() {
        let Object::Dict(font) = object.as_ref() else { continue };
        if font.name(b"Subtype") != Some(b"TrueType") {
            continue;
        }
        let descriptor = doc.dict(font.get(b"FontDescriptor").unwrap()).unwrap();
        let program = doc.resolve(descriptor.get(b"FontFile2").unwrap());
        let Object::Stream(program) = program.as_ref() else { panic!("no program") };
        let name = |d: &Dict, key: &[u8]| String::from_utf8(d.name(key).unwrap().to_vec()).unwrap();
        fonts.push((name(font, b"BaseFont"), name(&descriptor, b"FontName"), program.decoded().unwrap(), program.dict.int(b"Length1").unwrap()));
    }
    fonts.sort();
    fonts
}

#[test]
fn windows_gets_embedded_subsets_under_names_no_installed_font_has() {
    let pdf = print_ready();
    let before = true_type_fonts(&pdf);
    assert_eq!(before.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["AAAAAA+Arial-BoldMT", "AAAAAA+ArialMT"]);

    let spooled = spooled(&pdf, true);
    let after = true_type_fonts(&spooled);

    // Each program has a name of its own, in the PDF and in its `name` table; the subset tag stays.
    let names: Vec<_> = after.iter().map(|f| (f.0.as_str(), f.1.as_str())).collect();
    assert_eq!(names, [("AAAAAA+PdfCraftPrint1", "AAAAAA+PdfCraftPrint1"), ("AAAAAA+PdfCraftPrint2", "AAAAAA+PdfCraftPrint2")]);
    for (n, (_, _, program, length1)) in after.iter().enumerate() {
        let name = format!("PdfCraftPrint{}", n + 1);
        for id in [StringId::FAMILY_NAME, StringId::FULL_NAME, StringId::POSTSCRIPT_NAME, StringId::UNIQUE_ID] {
            assert_eq!(name_of(program, id).as_deref(), Some(name.as_str()), "{id:?}");
        }
        assert_eq!(name_of(program, StringId::SUBFAMILY_NAME).as_deref(), Some("Regular"));
        assert_eq!(*length1, program.len() as i64);
        // Nothing that draws a glyph changed: the same glyph for every character, and the same
        // tables but `name` and `head`'s checksum adjustment.
        let (old, new) = (FontRef::new(INTER).unwrap(), FontRef::new(program).unwrap());
        assert_eq!(old.charmap().mappings().collect::<Vec<_>>(), new.charmap().mappings().collect::<Vec<_>>());
        let table = |font: &FontRef, tag| {
            let mut data = font.table_data(tag).unwrap().as_bytes().to_vec();
            if &tag == b"head" {
                data[8..12].fill(0);
            }
            data
        };
        for tag in old.table_directory.table_records().iter().map(|r| r.tag()).filter(|t| t != b"name") {
            assert_eq!(table(&old, tag), table(&new, tag), "{tag}");
        }
        // `head`'s checksum adjustment holds for the new file.
        let sum = program.chunks(4).fold(0_u32, |s, c| s.wrapping_add(u32::from_be_bytes([0, 1, 2, 3].map(|i| c.get(i).copied().unwrap_or(0)))));
        assert_eq!(sum, 0xb1b0afba);
    }
    assert!(before.iter().all(|f| name_of(&f.2, StringId::FAMILY_NAME).as_deref() == Some("Inter")));

    // The page draws exactly as before, and draws something.
    let (old, new) = (pixels(&pdf), pixels(&spooled));
    assert!(old.chunks(4).any(|p| p[0] < 128), "the text is drawn");
    assert!(old == new, "renaming changed what prints");
}

#[test]
fn other_platforms_and_pdfs_without_embedded_programs_spool_as_they_are() {
    let pdf = print_ready();
    assert!(matches!(spooled(&pdf, false), std::borrow::Cow::Borrowed(p) if p == pdf.as_slice()));
    let helvetica = impose(&fixture(1), &Settings { pages: vec![0], paper: (200.0, 300.0), ..Settings::default() }).unwrap();
    assert_eq!(distinct(&helvetica).unwrap(), None);
    assert!(matches!(spooled(&helvetica, true), std::borrow::Cow::Borrowed(_)));
    // Not a PDF: it goes as it is, and the spooler reports what it makes of it.
    assert!(matches!(spooled(b"not a pdf", true), std::borrow::Cow::Borrowed(b"not a pdf")));
}

#[test]
fn a_program_that_cannot_be_rewritten_keeps_its_name_in_the_pdf_too() {
    // A viewer that can't load the program either picks a substitute by the PDF's name, so that
    // name must still say Arial.
    let mut doc = fixture(1);
    let good = program_stream(&mut doc);
    let mut d = Dict::new();
    d.set(b"Length1".to_vec(), Object::Int(10));
    let broken = doc.add(Object::Stream(Stream::flate(d, b"not a font")));
    let good = true_type_font(&mut doc, "AAAAAA+ArialMT", good);
    let broken = true_type_font(&mut doc, "BAAAAA+Arial-BoldMT", broken);
    let mut fonts = Dict::new();
    fonts.set(b"F1".to_vec(), Object::Ref(good));
    fonts.set(b"F2".to_vec(), Object::Ref(broken));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let pdf = write_full(&doc, &SaveOptions::default()).unwrap();

    let names: Vec<_> = true_type_fonts(&spooled(&pdf, true)).into_iter().map(|f| (f.0, f.1, f.2 == b"not a font")).collect();
    assert_eq!(
        names,
        [
            ("AAAAAA+PdfCraftPrint1".to_owned(), "AAAAAA+PdfCraftPrint1".to_owned(), false),
            ("BAAAAA+Arial-BoldMT".to_owned(), "BAAAAA+Arial-BoldMT".to_owned(), true),
        ]
    );
    // With nothing it can rewrite, the PDF goes as it is.
    let mut fonts = Dict::new();
    fonts.set(b"F2".to_vec(), Object::Ref(broken));
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    assert_eq!(distinct(&write_full(&doc, &SaveOptions::default()).unwrap()).unwrap(), None);
}

#[test]
fn programs_past_the_shared_decode_budget_keep_their_names() {
    // Each program decodes to Inter's whole length; the budget covers one and a half.
    let pdf = print_ready();
    let mut budget = DecodeBudget::new(INTER.len() * 3 / 2, 0);
    let renamed = distinct_within(&pdf, &mut budget).unwrap().unwrap();
    let names: Vec<_> = true_type_fonts(&renamed).into_iter().map(|f| (f.0, name_of(&f.2, StringId::FAMILY_NAME).unwrap())).collect();
    let (new, old): (Vec<_>, Vec<_>) = names.iter().partition(|(base_font, _)| base_font.contains("PdfCraftPrint"));
    assert_eq!((new.len(), old.len()), (1, 1), "{names:?}");
    assert!(new[0].0.starts_with("AAAAAA+PdfCraftPrint") && new[0].1.starts_with("PdfCraftPrint"), "{names:?}");
    assert!(old[0].0.starts_with("AAAAAA+Arial") && old[0].1 == "Inter", "{names:?}");
    // With the default allowance both are renamed.
    assert_eq!(true_type_fonts(&distinct(&pdf).unwrap().unwrap()).iter().filter(|f| f.0.contains("PdfCraftPrint")).count(), 2);
}

/// A CIDFontType2 named `base_font` over `program`, written as a dictionary with its descriptor
/// inline, as some producers put it straight into `/DescendantFonts`.
fn inline_cid_font(base_font: &str, program: ObjRef) -> Dict {
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(base_font));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    cid.set(b"BaseFont".to_vec(), Object::name(base_font));
    cid.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    cid
}

fn type0_font(doc: &mut Document, base_font: &str, descendants: Object) -> ObjRef {
    let mut type0 = Dict::new();
    type0.set(b"Type".to_vec(), Object::name("Font"));
    type0.set(b"Subtype".to_vec(), Object::name("Type0"));
    type0.set(b"BaseFont".to_vec(), Object::name(base_font));
    type0.set(b"Encoding".to_vec(), Object::name("Identity-H"));
    type0.set(b"DescendantFonts".to_vec(), descendants);
    doc.add(Object::Dict(type0))
}

#[test]
fn a_cidfont_written_inside_its_type0_font_is_renamed_with_it() {
    let mut doc = fixture(1);
    let (shared, own) = (program_stream(&mut doc), program_stream(&mut doc));
    let simple = true_type_font(&mut doc, "AAAAAA+ArialMT", shared);
    // In `/DescendantFonts` itself, sharing the simple font's program; and in an array object of
    // its own, over a program nothing else uses.
    let direct = type0_font(&mut doc, "BAAAAA+ArialMT-Identity-H", Object::Array(vec![Object::Dict(inline_cid_font("BAAAAA+ArialMT", shared))]));
    let array = doc.add(Object::Array(vec![Object::Dict(inline_cid_font("CAAAAA+ArialMT", own))]));
    let in_array = type0_font(&mut doc, "CAAAAA+ArialMT-Identity-H", Object::Ref(array));
    let mut fonts = Dict::new();
    for (key, font) in [(b"F1", simple), (b"F2", direct), (b"F3", in_array)] {
        fonts.set(key.to_vec(), Object::Ref(font));
    }
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let pdf = write_full(&doc, &SaveOptions::default()).unwrap();

    let renamed = Document::open(Arc::new(distinct(&pdf).unwrap().unwrap())).unwrap();
    let page = renamed.dict(&Object::Ref(pdfcraft_model::pages(&renamed)[0].obj)).unwrap();
    let fonts = renamed.dict(renamed.dict(page.get(b"Resources").unwrap()).unwrap().get(b"Font").unwrap()).unwrap();
    let name = |d: &Dict, key: &[u8]| String::from_utf8(d.name(key).unwrap().to_vec()).unwrap();
    let simple = renamed.dict(fonts.get(b"F1").unwrap()).unwrap();
    for (key, tag) in [(b"F2", "BAAAAA+"), (b"F3", "CAAAAA+")] {
        let type0 = renamed.dict(fonts.get(key).unwrap()).unwrap();
        let descendants = renamed.resolve(type0.get(b"DescendantFonts").unwrap());
        let Some(Object::Dict(cid)) = descendants.as_array().and_then(|a| a.first()) else { panic!("inline CIDFont") };
        let Some(Object::Dict(descriptor)) = cid.get(b"FontDescriptor") else { panic!("inline descriptor") };
        let program = renamed.resolve(descriptor.get(b"FontFile2").unwrap());
        let Object::Stream(program) = program.as_ref() else { panic!("program") };
        let family = name_of(&program.decoded().unwrap(), StringId::FAMILY_NAME).unwrap();
        // The Type 0 font, its CIDFont and the descriptor all say the program's new name.
        let expected = format!("{tag}{family}");
        assert!(family.starts_with("PdfCraftPrint"), "{family}");
        assert_eq!(
            (name(&type0, b"BaseFont"), name(cid, b"BaseFont"), name(descriptor, b"FontName")),
            (expected.clone(), expected.clone(), expected)
        );
    }
    // The shared program has one name, whichever font led to it.
    let shared_name = name(&simple, b"BaseFont").replace("AAAAAA+", "");
    assert_eq!(name(&renamed.dict(fonts.get(b"F2").unwrap()).unwrap(), b"BaseFont"), format!("BAAAAA+{shared_name}"));
}

#[test]
fn fonts_sharing_a_program_share_its_name_and_type0_fonts_follow_their_cidfont() {
    let mut doc = fixture(1);
    let program = program_stream(&mut doc);
    let simple = true_type_font(&mut doc, "BCDEFG+Calibri", program);
    let again = true_type_font(&mut doc, "Calibri", program);
    // A Type 0 font over a CIDFontType2 whose descriptor is written inline.
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name("HIJKLM+Calibri"));
    descriptor.set(b"FontFile2".to_vec(), Object::Ref(program));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    cid.set(b"BaseFont".to_vec(), Object::name("HIJKLM+Calibri"));
    cid.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    let cid = doc.add(Object::Dict(cid));
    let mut type0 = Dict::new();
    type0.set(b"Type".to_vec(), Object::name("Font"));
    type0.set(b"Subtype".to_vec(), Object::name("Type0"));
    type0.set(b"BaseFont".to_vec(), Object::name("HIJKLM+Calibri-Identity-H"));
    type0.set(b"Encoding".to_vec(), Object::name("Identity-H"));
    type0.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Ref(cid)]));
    let type0 = doc.add(Object::Dict(type0));
    let mut fonts = Dict::new();
    for (key, font) in [(b"F1", simple), (b"F2", again), (b"F3", type0)] {
        fonts.set(key.to_vec(), Object::Ref(font));
    }
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(resources))).unwrap();
    let pdf = write_full(&doc, &SaveOptions::default()).unwrap();

    // The file is written anew, so the fonts are found again through the page.
    let renamed = Document::open(Arc::new(distinct(&pdf).unwrap().unwrap())).unwrap();
    let page = renamed.dict(&Object::Ref(pdfcraft_model::pages(&renamed)[0].obj)).unwrap();
    let fonts = renamed.dict(renamed.dict(page.get(b"Resources").unwrap()).unwrap().get(b"Font").unwrap()).unwrap();
    let font = |key: &[u8]| renamed.dict(fonts.get(key).unwrap()).unwrap();
    let base_font = |d: &Dict| String::from_utf8(d.name(b"BaseFont").unwrap().to_vec()).unwrap();
    assert_eq!(base_font(&font(b"F1")), "BCDEFG+PdfCraftPrint1");
    assert_eq!(base_font(&font(b"F2")), "PdfCraftPrint1");
    assert_eq!(base_font(&font(b"F3")), "HIJKLM+PdfCraftPrint1");
    let Some(Object::Array(descendants)) = font(b"F3").get(b"DescendantFonts").cloned() else { panic!("descendants") };
    let cid = renamed.dict(&descendants[0]).unwrap();
    assert_eq!(base_font(&cid), "HIJKLM+PdfCraftPrint1");
    let Some(Object::Dict(inline)) = cid.get(b"FontDescriptor") else { panic!("inline descriptor") };
    assert_eq!(inline.name(b"FontName"), Some(b"HIJKLM+PdfCraftPrint1".as_slice()));
    // Renamed once, not once per font.
    let program = renamed.resolve(inline.get(b"FontFile2").unwrap());
    let Object::Stream(program) = program.as_ref() else { panic!("program") };
    assert_eq!(name_of(&program.decoded().unwrap(), StringId::FAMILY_NAME).as_deref(), Some("PdfCraftPrint1"));
}

/// A `name` table of `format` with the given records `(platform, encoding, language, id, text)`
/// and language tags, inside a minimal sfnt with only that table.
fn sfnt_with_name(format: u16, records: &[(u16, u16, u16, u16, &[u8])], tags: &[&[u8]]) -> Vec<u8> {
    let mut table = Vec::new();
    let mut strings = Vec::new();
    table.extend_from_slice(&format.to_be_bytes());
    table.extend_from_slice(&(records.len() as u16).to_be_bytes());
    let header = 6 + records.len() * 12 + if format == 1 { 2 + tags.len() * 4 } else { 0 };
    table.extend_from_slice(&(header as u16).to_be_bytes());
    for (platform, encoding, language, id, text) in records {
        for v in [*platform, *encoding, *language, *id, text.len() as u16, strings.len() as u16] {
            table.extend_from_slice(&v.to_be_bytes());
        }
        strings.extend_from_slice(text);
    }
    if format == 1 {
        table.extend_from_slice(&(tags.len() as u16).to_be_bytes());
        for tag in tags {
            table.extend_from_slice(&(tag.len() as u16).to_be_bytes());
            table.extend_from_slice(&(strings.len() as u16).to_be_bytes());
            strings.extend_from_slice(tag);
        }
    }
    table.extend_from_slice(&strings);
    let mut sfnt = vec![0, 1, 0, 0, 0, 1, 0, 16, 0, 0, 0, 0];
    sfnt.extend_from_slice(b"name");
    sfnt.extend_from_slice(&0_u32.to_be_bytes());
    sfnt.extend_from_slice(&28_u32.to_be_bytes());
    sfnt.extend_from_slice(&(table.len() as u32).to_be_bytes());
    sfnt.extend_from_slice(&table);
    sfnt
}

/// A `name` record: platform, encoding, language, name ID and the string's bytes.
type Record = (u16, u16, u16, u16, Vec<u8>);

/// The records and language tags of the renamed font's `name` table.
fn records_of(sfnt: &[u8]) -> (Vec<Record>, Vec<Vec<u8>>) {
    let font = FontRef::new(sfnt).unwrap();
    let name = font.name().unwrap();
    let data = name.string_data();
    let text = |length: u16, offset: u16| data.as_bytes()[offset as usize..][..length as usize].to_vec();
    let records = name
        .name_record()
        .iter()
        .map(|r| (r.platform_id(), r.encoding_id(), r.language_id(), r.name_id().to_u16(), text(r.length(), r.string_offset().to_u32() as u16)))
        .collect();
    let tags = name.lang_tag_record().unwrap_or_default().iter().map(|t| text(t.length(), t.lang_tag_offset().to_u32() as u16)).collect();
    (records, tags)
}

#[test]
fn identity_records_are_respelled_per_platform_and_the_rest_are_kept() {
    let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<u8>>();
    let arial = utf16("Arial");
    let copyright = utf16("© The Monotype Corporation");
    let sjis_family: &[u8] = &[0x82, 0xa0];
    let sfnt = sfnt_with_name(
        1,
        &[
            (1, 0, 0, 1, b"Arial"),
            (1, 0, 0, 2, b"Bold"),
            (3, 1, 0x409, 0, &copyright),
            (3, 1, 0x409, 1, &arial),
            (3, 1, 0x409, 6, &utf16("Arial-BoldMT")),
            (3, 2, 0x411, 1, sjis_family),
            (3, 1, 0x8000, 4, &utf16("Arial Bold")),
        ],
        &[b"\0e\0n"],
    );
    let renamed = renamed_sfnt(&sfnt, "PdfCraftPrint7").unwrap();
    let (records, tags) = records_of(&renamed);
    assert_eq!(
        records,
        [
            (1, 0, 0, 1, b"PdfCraftPrint7".to_vec()),
            (1, 0, 0, 2, b"Bold".to_vec()),
            (3, 1, 0x409, 0, copyright),
            (3, 1, 0x409, 1, utf16("PdfCraftPrint7")),
            (3, 1, 0x409, 6, utf16("PdfCraftPrint7")),
            // The Shift-JIS family can't be respelled in ASCII here, so it goes.
            (3, 1, 0x8000, 4, utf16("PdfCraftPrint7")),
        ]
    );
    assert_eq!(tags, [b"\0e\0n".to_vec()]);
}

#[test]
fn damaged_programs_are_left_alone_and_never_panic() {
    let good = sfnt_with_name(0, &[(3, 1, 0x409, 1, &[0, b'A'])], &[]);
    assert!(renamed_sfnt(&good, "X").is_some());
    // Every truncation, of the small font and of the start of a real one.
    for n in 0..good.len() {
        let _ = renamed_sfnt(&good[..n], "X");
    }
    for n in (0..2048).chain([INTER.len() - 1]) {
        let _ = renamed_sfnt(&INTER[..n], "X");
    }
    // A collection, a CFF-in-name-only header, an absurd table count, a `name` entry past the
    // end, a format-2 `name` table and a string past the table's end.
    let mut collection = good.clone();
    collection[..4].copy_from_slice(b"ttcf");
    assert_eq!(renamed_sfnt(&collection, "X"), None);
    let mut many = good.clone();
    many[4..6].copy_from_slice(&u16::MAX.to_be_bytes());
    assert_eq!(renamed_sfnt(&many, "X"), None);
    let mut past = good.clone();
    past[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(renamed_sfnt(&past, "X"), None);
    let mut format2 = good.clone();
    format2[28..30].copy_from_slice(&2_u16.to_be_bytes());
    assert_eq!(renamed_sfnt(&format2, "X"), None);
    let mut long = sfnt_with_name(0, &[(3, 1, 0x409, 0, &[0, b'C']), (3, 1, 0x409, 1, &[0, b'A'])], &[]);
    let first_length = 28 + 6 + 8;
    long[first_length..first_length + 2].copy_from_slice(&u16::MAX.to_be_bytes());
    let (records, _) = records_of(&renamed_sfnt(&long, "X").unwrap());
    assert_eq!(records, [(3, 1, 0x409, 1, vec![0, b'X'])], "the broken copyright record is left out");
    // A broken language tag fails the table, since records point at tags by position.
    let mut tagged = sfnt_with_name(1, &[(3, 1, 0x409, 1, &[0, b'A'])], &[b"\0e"]);
    let tag_length = 28 + 6 + 12 + 2;
    tagged[tag_length..tag_length + 2].copy_from_slice(&u16::MAX.to_be_bytes());
    assert_eq!(renamed_sfnt(&tagged, "X"), None);
}
