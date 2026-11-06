//! What `verify` answers for each clause it implements, pinned as a known answer.
//!
//! Each test builds a small file by hand, from the shape the clause describes rather than from
//! anything this repository saves, so a bug shared between the verifier and the writer cannot make a
//! case pass. Two things in the file are not hand-written: the output intent profiles come from
//! `icc::srgb()`, the crate's own sRGB generator, and the encrypted document in the 6.1.3 case is
//! encrypted by the object layer, because writing an encryption dictionary by hand would restate the
//! rules under test.
//!
//! The clause strings are the verifier's own citations, pinned here as `lib.rs` writes them. They
//! have not been compared with the text of the profile yet, so a number that does not name the rule
//! it is attached to is a finding about the verifier and not about this file.
//!
//! The positive case expects no issue under the clause and the negative case expects one; both are
//! asserted by clause string, so a later change that alters an answer fails here first. Cases where
//! the verifier is known to disagree with the standard are left out on purpose and recorded as gaps
//! where the change that adds them is reviewed. Nothing here is a conformance claim.

use pdfcraft_cos::{Algorithm, Document, NewEncryption};
use pdfcraft_preflight::{Level, convert, declared, icc, verify};

/// A document from hand-written objects and extra trailer entries (copied from the crate's unit
/// tests).
fn build(objs: &[&str], trailer_extra: &str) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    // ISO 19005-2 6.1.3: the trailer names a file identifier; a test about another clause gets one
    // unless it gives its own.
    let id = if trailer_extra.contains("/ID") { "" } else { "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]" };
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {id} {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(std::sync::Arc::new(out)).unwrap()
}

/// The byte-order mark an XMP packet's `xpacket` instruction names.
const BOM: char = '\u{feff}';

/// The XMP packet identifying a file as `part`/`conformance`, with `title` as its `dc:title`.
fn xmp(part: u8, conformance: &str, title: Option<&str>) -> String {
    let title = match title {
        Some(t) => format!("   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{t}</rdf:li></rdf:Alt></dc:title>\n"),
        None => String::new(),
    };
    format!(
        r#"<?xpacket begin="{BOM}" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" xmlns:dc="http://purl.org/dc/elements/1.1/">
   <pdfaid:part>{part}</pdfaid:part>
   <pdfaid:conformance>{conformance}</pdfaid:conformance>
{title}  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
    )
}

/// The XMP packet of a PDF/A-2b file, with `title` as its title when one is given.
fn a2b_packet(title: Option<&str>) -> String {
    xmp(2, "B", title)
}

/// The catalog's `/Metadata` stream object holding `packet`.
fn xmp_object(packet: &str) -> String {
    format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{packet}\nendstream", packet.len())
}

/// A content stream object holding `content`.
fn content(content: &str) -> String {
    format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
}

/// An ASCII-hex stream object holding `bytes`, with `dict` as its other entries.
fn hex_stream(dict: &str, bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    let data = format!("{hex}>");
    format!("<< /Filter /ASCIIHexDecode {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// A 128-byte ICC header for a four-component profile, with no `/N` so the header decides.
fn cmyk_profile_header() -> Vec<u8> {
    let mut header = vec![0u8; 128];
    header[0..4].copy_from_slice(&128u32.to_be_bytes());
    header[16..20].copy_from_slice(b"CMYK");
    header[20..24].copy_from_slice(b"XYZ ");
    header[36..40].copy_from_slice(b"acsp");
    header
}

/// The catalog's `/OutputIntents`: a `GTS_PDFA1` intent naming the profile in object `n`.
fn output_intent(n: usize) -> String {
    let condition = "/OutputConditionIdentifier (sRGB IEC61966-2.1)";
    let intent = format!("<< /Type /OutputIntent /S /GTS_PDFA1 {condition} /DestOutputProfile {n} 0 R >>");
    format!("/OutputIntents [{intent}]")
}

/// An embedded font program stream object, compressed with `filter` when one is given. Its bytes
/// are not read by any rule: a font program is data, not content.
fn font_program(filter: Option<&str>) -> String {
    let filter = match filter {
        Some(filter) => format!("/Filter {filter} "),
        None => String::new(),
    };
    format!("<< /Length1 4 {filter}/Length 4 >>\nstream\nabcd\nendstream")
}

/// An image XObject that asks to be interpolated, or does not.
fn image(interpolate: bool) -> String {
    format!(
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate {interpolate} /Length 1 >>\nstream\n\u{0}\nendstream"
    )
}

/// A document whose catalog (object 1) carries `catalog_extra` and, when `packet` is given, the
/// metadata object 3; whose page tree (object 2) holds the single page `page` (object 4); and whose
/// objects 5 onwards are `more`, which `page` and `catalog_extra` refer to by number. An absent
/// `packet` leaves object 3 empty. The page must resolve, so a case that loses it fails instead of
/// passing because no rule ever reached it.
fn page_document(page: &str, packet: Option<&str>, catalog_extra: &str, more: &[&str], trailer_extra: &str) -> Document {
    let metadata = match packet {
        Some(packet) => xmp_object(packet),
        None => "null".to_owned(),
    };
    let catalog = match packet {
        Some(_) => format!("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R {catalog_extra} >>"),
        None => format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
    };
    let pages = "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 200] >>";
    let mut objs = vec![catalog.as_str(), pages, metadata.as_str(), page];
    objs.extend_from_slice(more);
    let doc = build(&objs, trailer_extra);
    assert_eq!(pdfcraft_model::pages(&doc).len(), 1, "the page of this case does not resolve");
    doc
}

/// Asserts that `verify` reports `clause` for `doc`.
fn assert_reported(doc: &Document, level: Level, clause: &str) {
    let issues = verify(doc, level);
    assert!(issues.iter().any(|i| i.clause == clause), "no {clause} issue in {issues:#?}");
}

/// Asserts that `verify` reports nothing under `clause` for `doc`.
fn assert_not_reported(doc: &Document, level: Level, clause: &str) {
    let found: Vec<_> = verify(doc, level).into_iter().filter(|i| i.clause == clause).collect();
    assert!(found.is_empty(), "{clause} was reported: {found:#?}");
}

#[test]
fn xmp_metadata_is_required_clause_6_6_2_1() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let identified = page_document(page, Some(packet.as_str()), "", &[], "");
    assert_not_reported(&identified, Level::A2b, "6.6.2.1");
    assert_eq!(declared(&identified).pdfa, Some((2, "B".into())));
    let bare = page_document(page, None, "", &[], "");
    assert_reported(&bare, Level::A2b, "6.6.2.1");
}

#[test]
fn the_declared_part_and_conformance_must_be_the_level_clause_6_6_4() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let matching = a2b_packet(None);
    let doc = page_document(page, Some(matching.as_str()), "", &[], "");
    assert_not_reported(&doc, Level::A2b, "6.6.4");
    for (part, conformance) in [(3, "B"), (2, "A")] {
        let packet = xmp(part, conformance, None);
        let doc = page_document(page, Some(packet.as_str()), "", &[], "");
        assert_reported(&doc, Level::A2b, "6.6.4");
        assert_eq!(declared(&doc).pdfa, Some((part, conformance.to_owned())));
    }
}

#[test]
fn the_xmp_title_must_match_the_information_dictionary_clause_6_6_3() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let info = "/Info << /Title (Report) >>";
    let matching = a2b_packet(Some("Report"));
    let doc = page_document(page, Some(matching.as_str()), "", &[], info);
    assert_not_reported(&doc, Level::A2b, "6.6.3");
    let differing = a2b_packet(Some("Draft"));
    let doc = page_document(page, Some(differing.as_str()), "", &[], info);
    assert_reported(&doc, Level::A2b, "6.6.3");
}

#[test]
fn the_trailer_must_not_name_encryption_clause_6_1_3() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let plain = page_document(page, Some(packet.as_str()), "", &[], "");
    assert_not_reported(&plain, Level::A2b, "6.1.3");
    // An encrypted file's keys are not written out by hand here, so the /Encrypt dictionary comes
    // from the object layer; the rule under test only asks whether the trailer names one.
    let mut encrypted = page_document(page, Some(packet.as_str()), "", &[], "");
    let params = NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -4,
        encrypt_metadata: true,
        seed: [7; 32],
    };
    encrypted.set_encryption(&params).unwrap();
    assert_reported(&encrypted, Level::A2b, "6.1.3");
    let report = convert(&mut encrypted, Level::A2b).unwrap();
    assert!(!report.remaining.iter().any(|i| i.clause == "6.1.3"), "{:#?}", report.remaining);
}

#[test]
fn device_colour_needs_an_output_intent_clause_6_2_3() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>";
    let packet = a2b_packet(None);
    let rgb = content("1 0 0 rg 10 10 20 20 re f");
    let doc = page_document(page, Some(packet.as_str()), "", &[rgb.as_str()], "");
    assert_reported(&doc, Level::A2b, "6.2.3");
    let none = content("q Q");
    let doc = page_document(page, Some(packet.as_str()), "", &[none.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.3");
}

#[test]
fn device_rgb_needs_an_rgb_output_intent_clause_6_2_4_3() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>";
    let packet = a2b_packet(None);
    let rgb = content("1 0 0 rg 10 10 20 20 re f");
    let srgb = hex_stream("/N 3", &icc::srgb());
    let doc = page_document(page, Some(packet.as_str()), &output_intent(6), &[rgb.as_str(), srgb.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.4.3");
    // The intent is there, so the neighbour rule about device colour without one stays quiet.
    assert_not_reported(&doc, Level::A2b, "6.2.3");
    let cmyk = hex_stream("", &cmyk_profile_header());
    let doc = page_document(page, Some(packet.as_str()), &output_intent(6), &[rgb.as_str(), cmyk.as_str()], "");
    assert_reported(&doc, Level::A2b, "6.2.4.3");
}

#[test]
fn the_catalog_must_not_have_additional_actions_clause_6_5_2() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let plain = page_document(page, Some(packet.as_str()), "", &[], "");
    assert_not_reported(&plain, Level::A2b, "6.5.2");
    let acting = page_document(page, Some(packet.as_str()), "/AA << /WC << /S /GoTo >> >>", &[], "");
    assert_reported(&acting, Level::A2b, "6.5.2");
}

#[test]
fn the_catalog_must_not_have_document_javascript_clause_6_5_1() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let plain = page_document(page, Some(packet.as_str()), "", &[], "");
    assert_not_reported(&plain, Level::A2b, "6.5.1");
    // An empty name tree is enough: an action object here would report the same clause, and this
    // case is about the catalog's own entry.
    let tree = "/Names << /JavaScript << /Names [] >> >>";
    let scripted = page_document(page, Some(packet.as_str()), tree, &[], "");
    assert_reported(&scripted, Level::A2b, "6.5.1");
}

#[test]
fn the_form_must_not_ask_viewers_for_appearances_clause_6_4_1() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let plain = page_document(page, Some(packet.as_str()), "/AcroForm << /Fields [] >>", &[], "");
    assert_not_reported(&plain, Level::A2b, "6.4.1");
    let asking = "/AcroForm << /Fields [] /NeedAppearances true >>";
    let doc = page_document(page, Some(packet.as_str()), asking, &[], "");
    assert_reported(&doc, Level::A2b, "6.4.1");
}

#[test]
fn forbidden_annotation_types_are_reported_clause_6_3_1() {
    let page = "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>";
    let packet = a2b_packet(None);
    let link = "<< /Type /Annot /Subtype /Link /Rect [10 10 30 30] /P 4 0 R /F 4 >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[link], "");
    assert_not_reported(&doc, Level::A2b, "6.3.1");
    let sound = "<< /Type /Annot /Subtype /Sound /Rect [10 10 30 30] /P 4 0 R /F 4 >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[sound], "");
    assert_reported(&doc, Level::A2b, "6.3.1");
}

#[test]
fn annotations_must_print_and_not_hide_clause_6_3_2() {
    let page = "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>";
    let packet = a2b_packet(None);
    let appearance = "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream";
    let printing = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 4 /AP << /N 6 0 R >> >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[printing, appearance], "");
    assert_not_reported(&doc, Level::A2b, "6.3.2");
    let unprinted = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 0 /AP << /N 6 0 R >> >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[unprinted, appearance], "");
    assert_reported(&doc, Level::A2b, "6.3.2");
    let hidden = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 6 /AP << /N 6 0 R >> >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[hidden, appearance], "");
    assert_reported(&doc, Level::A2b, "6.3.2");
}

#[test]
fn annotations_need_an_appearance_stream_clause_6_3_3() {
    let page = "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>";
    let packet = a2b_packet(None);
    let appearance = "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream";
    let with = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 4 /AP << /N 6 0 R >> >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[with, appearance], "");
    assert_not_reported(&doc, Level::A2b, "6.3.3");
    let without = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 4 >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[without], "");
    assert_reported(&doc, Level::A2b, "6.3.3");
}

#[test]
fn lzw_compressed_streams_are_reported_clause_6_1_7_2() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> >>";
    let packet = a2b_packet(None);
    let text = content("BT /F1 12 Tf 10 10 Td (Hi) Tj ET");
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Sans /FirstChar 72 /LastChar 105 /Widths [500 500] /FontDescriptor 7 0 R >>";
    let descriptor = "<< /Type /FontDescriptor /FontName /Sans /Flags 32 /FontFile2 8 0 R >>";
    let flate = font_program(Some("/FlateDecode"));
    let doc = page_document(page, Some(packet.as_str()), "", &[text.as_str(), font, descriptor, flate.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.1.7.2");
    // The neighbour rule holds: the font still reads as embedded.
    assert_not_reported(&doc, Level::A2b, "6.2.11.4");
    let lzw = font_program(Some("/LZWDecode"));
    let doc = page_document(page, Some(packet.as_str()), "", &[text.as_str(), font, descriptor, lzw.as_str()], "");
    assert_reported(&doc, Level::A2b, "6.1.7.2");
}

#[test]
fn streams_must_not_read_external_files_clause_6_1_7_1() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let held = "<< /Length 4 >>\nstream\nabcd\nendstream";
    let doc = page_document(page, Some(packet.as_str()), "", &[held], "");
    assert_not_reported(&doc, Level::A2b, "6.1.7.1");
    // A stream object nothing refers to: the rule looks at every stream in the file.
    let external = "<< /Length 4 /F (data.bin) >>\nstream\nabcd\nendstream";
    let doc = page_document(page, Some(packet.as_str()), "", &[external], "");
    assert_reported(&doc, Level::A2b, "6.1.7.1");
}

#[test]
fn images_must_not_be_interpolated_clause_6_2_8() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>";
    let packet = a2b_packet(None);
    let drawing = content("q 10 0 0 10 0 0 cm /Im0 Do Q");
    let smooth = image(true);
    let doc = page_document(page, Some(packet.as_str()), "", &[smooth.as_str(), drawing.as_str()], "");
    assert_reported(&doc, Level::A2b, "6.2.8");
    let crisp = image(false);
    let doc = page_document(page, Some(packet.as_str()), "", &[crisp.as_str(), drawing.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.8");
}

#[test]
fn postscript_xobjects_are_reported_clause_6_2_9() {
    let plain = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>";
    let with = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /XObject << /PS0 6 0 R >> >> >>";
    let packet = a2b_packet(None);
    let drawing = content("q Q");
    let doc = page_document(plain, Some(packet.as_str()), "", &[drawing.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.9");
    let postscript = "<< /Type /XObject /Subtype /PS /Length 0 >>\nstream\n\nendstream";
    let doc = page_document(with, Some(packet.as_str()), "", &[drawing.as_str(), postscript], "");
    assert_reported(&doc, Level::A2b, "6.2.9");
}

#[test]
fn fonts_must_be_embedded_clause_6_2_11_4() {
    let page = "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> >>";
    let packet = a2b_packet(None);
    let embedded = "<< /Type /Font /Subtype /TrueType /BaseFont /Sans /FontDescriptor 6 0 R >>";
    let descriptor = "<< /Type /FontDescriptor /FontName /Sans /Flags 32 /FontFile2 7 0 R >>";
    let program = font_program(None);
    let doc = page_document(page, Some(packet.as_str()), "", &[embedded, descriptor, program.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.11.4");
    let missing = "<< /Type /Font /Subtype /TrueType /BaseFont /Sans >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[missing], "");
    assert_reported(&doc, Level::A2b, "6.2.11.4");
}

/// A file built by hand, apart from its output profile, with everything the verifier asks for: an
/// XMP packet naming PDF/A-2b with a title that matches the document information, an sRGB output
/// intent and a blank page. It checks that the rules agree with each other, which is not by itself a
/// conformance claim.
#[test]
fn minimal_document_has_no_verifier_issues() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(Some("Report"));
    let profile = hex_stream("/N 3", &icc::srgb());
    let info = "/Info << /Title (Report) >>";
    let mut doc = page_document(page, Some(packet.as_str()), &output_intent(5), &[profile.as_str()], info);
    let issues = verify(&doc, Level::A2b);
    assert!(issues.is_empty(), "{issues:#?}");
    assert_eq!(declared(&doc).pdfa, Some((2, "B".into())));
    assert_eq!(declared(&doc).output_intents, ["sRGB IEC61966-2.1"]);
    let report = convert(&mut doc, Level::A2b).unwrap();
    assert!(report.remaining.is_empty(), "{:#?}", report.remaining);
    assert_eq!(declared(&doc).pdfa, Some((2, "B".into())));
}
