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
    let objs: Vec<String> = vec![
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
    shows.iter().enumerate().map(|(i, (set, text))| format!("BT /F1 12 Tf 72 {} Td {set} ({text}) Tj ET\n", 700 - 40 * i)).collect()
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
