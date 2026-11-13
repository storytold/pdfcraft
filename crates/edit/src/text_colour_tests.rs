//! The colour a text run is read in when it is set with `cs` + `sc`/`scn` (ISO 32000-2 §8.6.8,
//! Table 73): the operands are read in the space `cs` selected, not by their count.

use std::sync::Arc;

use pdfcraft_cos::Document;

use crate::text::{reading_blocks, text_lines};

/// A one-page PDF drawing `content` with `/F1` Helvetica, `colour_spaces` as the page's
/// `/ColorSpace` resources and `/Fm` a form XObject drawing `form` with `form_spaces` as its own.
/// Objects 7 and 8 are ICC profile streams with `/N 3` and `/N 1` (no profile data: their
/// component count is what the colour is read by).
fn page(content: &str, colour_spaces: &str, form: &str, form_spaces: &str) -> Document {
    page_with(content, colour_spaces, form, form_spaces, &[])
}

/// [`page`], with `extra` as objects 9 onwards.
fn page_with(content: &str, colour_spaces: &str, form: &str, form_spaces: &str, extra: &[String]) -> Document {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> /ColorSpace << {colour_spaces} >> /XObject << /Fm 6 0 R >> >> >>"
        ),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into(),
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /ColorSpace << {form_spaces} >> >> \
             /Length {} >>\nstream\n{form}\nendstream",
            form.len()
        ),
        "<< /N 3 /Length 0 >>\nstream\n\nendstream".into(),
        "<< /N 1 /Length 0 >>\nstream\n\nendstream".into(),
    ];
    objs.extend_from_slice(extra);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// Each line of `shows` (`colour-setting operators`, `text`) shown on a line of its own.
fn content(shows: &[(&str, &str)]) -> String {
    shows.iter().enumerate().map(|(i, (set, text))| format!("BT /F1 12 Tf 72 {} Td {set} ({text}) Tj ET\n", 700 - 30 * i)).collect()
}

/// The colour each line of the page's own content is read in, by its text.
fn colours(doc: &Document) -> Vec<(String, [f64; 3])> {
    text_lines(doc, 0).unwrap().into_iter().map(|l| (l.text, l.color)).collect()
}

fn colour_of(doc: &Document, text: &str) -> [f64; 3] {
    let all = colours(doc);
    all.iter().find(|(t, _)| t == text).map(|(_, c)| *c).unwrap_or_else(|| panic!("no line {text:?} in {all:?}"))
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
}

const SEP_BLACK: &str = "/Sep [/Separation /Black /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>]";

/// §8.6.6.4: a Separation's one operand is a tint, 1.0 the full colourant. A black ink at full
/// tint is black, at none white, at half a mid gray, through the tint transform into its
/// alternate DeviceGray.
#[test]
fn a_separation_tint_is_ink_not_a_gray_level() {
    let doc = page(&content(&[("/Sep cs 1 scn", "Full"), ("/Sep cs 0 scn", "None"), ("/Sep cs 0.5 sc", "Half")]), SEP_BLACK, "", "");
    assert!(near(colour_of(&doc, "Full"), [0.0; 3]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "None"), [1.0; 3]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "Half"), [0.5; 3]), "{:?}", colours(&doc));
}

/// The case the reports print in: `/Black` through a sampled (Type 0) tint transform into an
/// ICCBased RGB space. The transform isn't evaluated, so the tint reads as the darkness of the
/// ink: full black at 1.0, never white.
#[test]
fn a_separation_with_a_sampled_tint_transform_reads_its_tint_as_darkness() {
    let spaces = "/Sep [/Separation /Black [/ICCBased 7 0 R] << /FunctionType 0 /Domain [0 1] /Range [0 1 0 1 0 1] \
                  /Size [2] /BitsPerSample 8 /Length 6 >>]";
    let doc = page(&content(&[("/Sep cs 1 scn", "Ink")]), spaces, "", "");
    assert!(near(colour_of(&doc, "Ink"), [0.0; 3]), "{:?}", colours(&doc));
}

/// §8.6.8: `cs` alone sets the space's initial colour: a full tint of a Separation, the first
/// entry of an Indexed table.
#[test]
fn cs_alone_sets_the_initial_colour_of_its_space() {
    let spaces = format!("{SEP_BLACK} /Idx [/Indexed /DeviceRGB 1 <FF000000FF00>]");
    let doc = page(&content(&[("/Sep cs", "Ink"), ("/Idx cs", "First")]), &spaces, "", "");
    assert!(near(colour_of(&doc, "Ink"), [0.0; 3]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "First"), [1.0, 0.0, 0.0]), "{:?}", colours(&doc));
}

/// §8.6.6.3: an Indexed operand is a place in the table, so index 0 is the table's first colour.
#[test]
fn an_indexed_operand_picks_its_table_entry() {
    let spaces = "/Idx [/Indexed /DeviceRGB 1 <FF000000FF00>]";
    let doc = page(&content(&[("/Idx cs 0 scn", "Red"), ("/Idx cs 1 sc", "Green")]), spaces, "", "");
    assert!(near(colour_of(&doc, "Red"), [1.0, 0.0, 0.0]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "Green"), [0.0, 1.0, 0.0]), "{:?}", colours(&doc));
}

/// A DeviceN space of three colourants is not RGB: its tint transform maps them into DeviceCMYK.
#[test]
fn a_devicen_colour_goes_through_its_tint_transform() {
    let spaces = "/DN [/DeviceN [/Cyan /Magenta /Yellow] /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 0 1] /N 1 >>]";
    let doc = page(&content(&[("/DN cs 1 0 0 scn", "Dark")]), spaces, "", "");
    // The Type 2 transform reads the first input: 1 → 100 K.
    assert!(near(colour_of(&doc, "Dark"), [0.0; 3]), "{:?}", colours(&doc));
}

/// ICCBased spaces are read by their component count: `/N 3` as RGB, `/N 1` as gray.
#[test]
fn icc_based_spaces_read_by_their_component_count() {
    let spaces = "/Rgb [/ICCBased 7 0 R] /Gray [/ICCBased 8 0 R]";
    let doc = page(&content(&[("/Rgb cs 0 0 1 scn", "Blue"), ("/Gray cs 0.25 scn", "Dim")]), spaces, "", "");
    assert!(near(colour_of(&doc, "Blue"), [0.0, 0.0, 1.0]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "Dim"), [0.25; 3]), "{:?}", colours(&doc));
}

/// A `cs` in a form XObject reads its name in the form's resources, and the colour the page set
/// before the `Do` is read in the page's resources even where the form names another space so.
#[test]
fn a_form_reads_colour_spaces_in_the_resources_in_force_where_cs_ran() {
    let form = "q BT /F1 12 Tf 72 600 Td /Sep cs 1 scn (Inside) Tj ET Q\nBT /F1 12 Tf 72 500 Td (Unset) Tj ET\n";
    // The page's /Sep is red ink; the form's /Sep is black ink.
    let page_sep = "/Sep [/Separation /Red /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [1 0 0] /N 1 >>]";
    let doc = page("/Sep cs 1 scn /Fm Do\n", page_sep, form, SEP_BLACK);
    let blocks = reading_blocks(&doc, 0).unwrap();
    let colour = |text: &str| blocks.iter().find(|b| b.text.contains(text)).map(|b| b.color).unwrap_or_else(|| panic!("{text}: {blocks:?}"));
    assert!(near(colour("Inside"), [0.0; 3]), "{blocks:?}");
    // The form inherits the page's red ink; its own /Sep isn't what that `cs` selected.
    assert!(near(colour("Unset"), [1.0, 0.0, 0.0]), "{blocks:?}");
}

/// Spaces that can't be read change nothing for the worse: an unknown name, a family no reader
/// knows, a name naming a name (here two naming each other), a table too short for its index
/// and a Separation with no tint transform all give a colour without a panic.
#[test]
fn malformed_colour_spaces_fall_back_without_panicking() {
    let spaces = "/Odd [/NoSuchFamily 1 2] /A /B /B /A /Short [/Indexed /DeviceRGB 3 <FF0000>] /Bare [/Separation /Black] \
                  /Deep [/Indexed [/Indexed [/Indexed [/Indexed [/Indexed [/Indexed /DeviceRGB 0 <00>] 0 <00>] 0 <00>] 0 <00>] 0 <00>] 0 <00>] \
                  /Huge [/Separation /Black /DeviceGray << /FunctionType 2 /C0 [1] /C1 [0] /N -5 >>]";
    let shows = [
        ("/Missing cs 0 0 1 scn", "Missing"),
        ("/Odd cs 0 0 1 sc", "Odd"),
        ("/A cs 1 0 0 scn", "Cycle"),
        ("/Short cs 3 scn", "Short"),
        ("/Bare cs 1 scn", "Bare"),
        ("/Deep cs 0 scn", "Deep"),
        ("/Huge cs 0 scn", "Huge"),
        ("/Sep cs /P0 scn", "Name"),
        ("/Sep cs scn", "Empty"),
    ];
    let doc = page(&content(&shows), &format!("{spaces} {SEP_BLACK}"), "", "");
    let all = colours(&doc);
    assert!(all.iter().all(|(_, c)| c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))), "{all:?}");
    // A space the resources don't hold reads by the count, as before: three numbers are RGB.
    assert!(near(colour_of(&doc, "Missing"), [0.0, 0.0, 1.0]), "{all:?}");
    assert!(near(colour_of(&doc, "Odd"), [0.0, 0.0, 1.0]), "{all:?}");
    assert!(near(colour_of(&doc, "Cycle"), [1.0, 0.0, 0.0]), "{all:?}");
    // An index past the table's end has no entry: black, not a panic.
    assert!(near(colour_of(&doc, "Short"), [0.0; 3]), "{all:?}");
    // No tint transform: the tint is the darkness of the ink.
    assert!(near(colour_of(&doc, "Bare"), [0.0; 3]), "{all:?}");
}

/// §8.6.8, Table 73: only `DeviceGray`, `DeviceRGB`, `DeviceCMYK` and `Pattern` name a space
/// directly. Any other name, an inline image's abbreviation such as `RGB` included, is a key in
/// the `/ColorSpace` resources, so `/RGB` naming a four-component ICC profile reads its operands
/// as CMYK. A reserved name still means its own space even when the resources hold an entry of
/// that name, and an abbreviation the resources don't hold reads as its device space.
#[test]
fn only_the_reserved_names_select_a_space_without_the_resources() {
    let cmyk_profile = "<< /N 4 /Length 0 >>\nstream\n\nendstream".to_string();
    let spaces = "/RGB [/ICCBased 9 0 R] /CalRGB [/ICCBased 9 0 R] /DeviceRGB [/ICCBased 9 0 R]";
    let shows = [
        ("/RGB cs 0 0 0 0 scn", "Abbreviation"),
        ("/CalRGB cs 0 0 0 1 scn", "Family"),
        ("/DeviceRGB cs 0 0 1 scn", "Reserved"),
        ("/CMYK cs 0 0 0 1 scn", "Unheld"),
    ];
    let doc = page_with(&content(&shows), spaces, "", "", &[cmyk_profile]);
    // No ink at all in CMYK is white; read as RGB it was black.
    assert!(near(colour_of(&doc, "Abbreviation"), [1.0; 3]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "Family"), [0.0; 3]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "Reserved"), [0.0, 0.0, 1.0]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "Unheld"), [0.0; 3]), "{:?}", colours(&doc));
}

/// The Indexed lookup tables a page's text colours select are decoded against one allowance for
/// the page ([`crate::colour::MAX_PAGE_LOOKUPS`]), not each on its own: twenty tables padded to
/// 60 KiB, each with a red first entry, read red until the allowance runs out and then black (an
/// empty table). Decoded each on its own, all twenty read red, however many there were.
#[test]
fn lookup_tables_share_one_allowance_for_the_page() {
    const PADDED: usize = 60 << 10;
    const TABLES: usize = 20;
    let data = format!("FF0000{}>", "0".repeat(2 * (PADDED - 3)));
    let extra: Vec<String> =
        (0..TABLES).map(|_| format!("<< /Filter /ASCIIHexDecode /Length {} >>\nstream\n{data}\nendstream", data.len())).collect();
    let spaces: String = (0..TABLES).map(|i| format!("/I{i} [/Indexed /DeviceRGB 0 {} 0 R] ", 9 + i)).collect();
    let shows: Vec<(String, String)> = (0..TABLES).map(|i| (format!("/I{i} cs 0 scn"), format!("T{i}"))).collect();
    let shows: Vec<(&str, &str)> = shows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let doc = page_with(&content(&shows), &spaces, "", "", &extra);
    let reads = |colour: [f64; 3]| (0..TABLES).filter(|i| near(colour_of(&doc, &format!("T{i}")), colour)).count();
    assert_eq!(reads([1.0, 0.0, 0.0]), crate::colour::MAX_PAGE_LOOKUPS / PADDED, "{:?}", colours(&doc));
    assert_eq!(reads([0.0; 3]), TABLES - crate::colour::MAX_PAGE_LOOKUPS / PADDED, "{:?}", colours(&doc));
}

/// An Indexed colour whose table has no entry for it reads as black whatever the base space: a
/// CMYK table past the per-table limit, or too short, isn't read as zero ink (white).
#[test]
fn a_missing_lookup_entry_is_black_in_any_base() {
    let big = format!("FFFFFFFF{}>", "0".repeat(2 * (crate::colour::MAX_LOOKUP_DECODED + 4)));
    let extra = vec![format!("<< /Filter /ASCIIHexDecode /Length {} >>\nstream\n{big}\nendstream", big.len())];
    let spaces = "/Big [/Indexed /DeviceCMYK 0 9 0 R] /Short [/Indexed /DeviceCMYK 1 <00000000>]";
    let doc = page_with(&content(&[("/Big cs 0 scn", "Big"), ("/Short cs 1 scn", "Short"), ("/Short cs 0 scn", "Held")]), spaces, "", "", &extra);
    assert!(near(colour_of(&doc, "Big"), [0.0; 3]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "Short"), [0.0; 3]), "{:?}", colours(&doc));
    assert!(near(colour_of(&doc, "Held"), [1.0; 3]), "an entry it has still reads: {:?}", colours(&doc));
}

/// A lookup table given as a byte string is charged to the page's allowance as a stream is, so
/// many names for one string-backed Indexed space can't each keep a copy of it for free.
/// Twenty-two names alias one space over a 255-colourant DeviceN base: 256 entries need 65,280
/// bytes of a string padded to 64 KiB, the first entry's first tint full (red through the Type 2
/// transform). Names read red while the allowance lasts (the last one in part) and black after;
/// uncharged, all twenty-two read red, however many names there were.
#[test]
fn string_lookup_aliases_share_the_page_allowance() {
    const COLOURANTS: usize = 255;
    const NEEDED: usize = 256 * COLOURANTS;
    const PADDED: usize = 64 << 10;
    const NAMES: usize = 22;
    let names: String = (0..COLOURANTS).map(|i| format!("/C{i} ")).collect();
    let mut table = "FF".to_string();
    table.push_str(&"0".repeat(2 * (PADDED - 1)));
    let extra = vec![
        format!("[/Indexed [/DeviceN [{names}] /DeviceRGB 10 0 R] 255 <{table}>]"),
        "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 0 0] /N 1 >>".to_string(),
    ];
    let spaces: String = (0..NAMES).map(|i| format!("/I{i} 9 0 R ")).collect();
    let shows: Vec<(String, String)> = (0..NAMES).map(|i| (format!("/I{i} cs 0 scn"), format!("T{i}"))).collect();
    let shows: Vec<(&str, &str)> = shows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let doc = page_with(&content(&shows), &spaces, "", "", &extra);
    let reads = |colour: [f64; 3]| (0..NAMES).filter(|i| near(colour_of(&doc, &format!("T{i}")), colour)).count();
    let red = crate::colour::MAX_PAGE_LOOKUPS.div_ceil(NEEDED);
    assert_eq!(reads([1.0, 0.0, 0.0]), red, "{:?}", colours(&doc));
    assert_eq!(reads([0.0; 3]), NAMES - red, "{:?}", colours(&doc));
}

/// Only the `hival + 1` entries of an Indexed table are kept (ISO 32000-2 §8.6.6.3): a byte
/// string is charged what is kept of it, and a stream what it decoded to, but neither keeps the
/// padding past the last entry.
#[test]
fn an_indexed_table_keeps_only_its_entries() {
    use crate::colour::{Lookups, MAX_PAGE_LOOKUPS, Space, space_within};
    use pdfcraft_cos::{Dict, ObjRef, Object};
    const PADDED: usize = 4096;
    let padding = "0".repeat(2 * (PADDED - 6));
    let extra = vec![
        format!("[/Indexed /DeviceRGB 1 <FF000000FF00{padding}>]"),
        "[/Indexed /DeviceRGB 1 11 0 R]".to_string(),
        format!("<< /Filter /ASCIIHexDecode /Length {} >>\nstream\nFF000000FF00{padding}>\nendstream", padding.len() + 13),
    ];
    let doc = page_with("", "", "", "", &extra);
    let table = |num: u32, lookups: &mut Lookups| match space_within(&doc, &Object::Ref(ObjRef { num, generation: 0 }), &Dict::default(), 0, lookups)
    {
        Space::Indexed { table, .. } => table,
        other => panic!("not Indexed: {other:?}"),
    };
    let mut lookups = Lookups::page();
    assert_eq!(table(9, &mut lookups), [0xFF, 0, 0, 0, 0xFF, 0]);
    assert_eq!(lookups.left(), MAX_PAGE_LOOKUPS - 6, "a string is charged what is kept of it");
    assert_eq!(table(10, &mut lookups).len(), 6);
    assert_eq!(lookups.left(), MAX_PAGE_LOOKUPS - 6 - PADDED, "a stream is charged what it decoded to");
}

/// A Separation or DeviceN space is read once for each name naming it, so it keeps only the C0
/// and C1 numbers its Type 2 transform outputs (the alternate space's components, ISO 32000-2
/// §8.6.6.4), however long those arrays are in the file: twenty names for one space whose C0 and
/// C1 hold 20,000 numbers each keep three apiece, and a colour still reads through them.
#[test]
fn a_tint_transform_keeps_only_the_outputs_its_alternate_reads() {
    use crate::colour::{Space, space_of};
    use pdfcraft_cos::{Dict, ObjRef, Object};
    const LONG: usize = 20_000;
    const NAMES: usize = 20;
    let tail = " 0".repeat(LONG - 3);
    let extra = vec![
        "[/Separation /Spot /DeviceRGB 10 0 R]".to_string(),
        format!("<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0{tail}] /C1 [1 0 0{tail}] /N 1 >>"),
    ];
    let spaces: String = (0..NAMES).map(|i| format!("/S{i} 9 0 R ")).collect();
    let shows: Vec<(String, String)> = (0..NAMES).map(|i| (format!("/S{i} cs 1 scn"), format!("T{i}"))).collect();
    let shows: Vec<(&str, &str)> = shows.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let doc = page_with(&content(&shows), &spaces, "", "", &extra);
    match space_of(&doc, &Object::Ref(ObjRef { num: 9, generation: 0 }), &Dict::default(), 0) {
        Space::Tint { exp: Some((c0, c1, _)), .. } => assert_eq!((c0.len(), c1.len()), (3, 3)),
        other => panic!("not a Type 2 tint: {other:?}"),
    }
    assert!((0..NAMES).all(|i| near(colour_of(&doc, &format!("T{i}")), [1.0, 0.0, 0.0])), "{:?}", colours(&doc));
}
