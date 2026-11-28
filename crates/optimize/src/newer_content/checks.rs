//! Rules a key and value table cannot express: flag bits, numeric values, string pairs and value
//! shapes.

use pdfcraft_cos::{Dict, Object};

use super::ctx::{ANNOT, EXTENSIONS, FIELD, FT_BTN, FT_CH, FT_SIG, FT_TX, SEED_VALUE, SUBMIT};
use super::scan::Notes;

/// Value-dependent requirements of dictionary `d` (context `bits`), noted into `notes`.
pub(super) fn custom(d: &Dict, bits: u64, notes: &mut Notes) {
    let int = |k: &[u8]| d.int(k);
    let mut note = |v: u8, what: &str| {
        notes.insert((v, what.to_owned()));
    };
    if int(b"BitsPerComponent") == Some(16) {
        note(15, "16-bit image samples");
    }
    if bits & ANNOT != 0 {
        flag_versions(int(b"F"), &[(128, 14), (256, 15), (512, 17)], "annotation flag", &mut note);
    }
    if bits & FIELD != 0 {
        field(d, bits, int(b"Ff"), &mut note);
    }
    if bits & SEED_VALUE != 0 {
        flag_versions(int(b"Ff"), &[(0x180, 20)], "seed-value flag", &mut note);
    }
    if bits & SUBMIT != 0 {
        flag_versions(int(b"Flags"), &[(0x0FE0, 14), (0x2000, 15)], "submit-form flag", &mut note);
    }
    let text = |k: &[u8]| d.get(k).and_then(Object::as_string).map(|s| s.bytes.as_slice());
    if text(b"Registry") == Some(b"Adobe") && text(b"Ordering") == Some(b"KR") {
        note(20, "Adobe-KR character collection");
    }
    if matches!(d.get(b"SpotFunction"), Some(Object::Array(_))) {
        note(20, "/SpotFunction array");
    }
    if bits & EXTENSIONS != 0 && d.iter().any(|(_, v)| matches!(v, Object::Array(_))) {
        note(20, "several extensions under one prefix");
    }
}

fn flag_versions(value: Option<i64>, masks: &[(i64, u8)], what: &str, note: &mut impl FnMut(u8, &str)) {
    let Some(value) = value else { return };
    for &(mask, ver) in masks {
        if value & mask != 0 {
            note(ver, what);
        }
    }
}

/// §12.7.4 Tables 226, 229 and 231: field flags 21 to 24 are PDF 1.4 and 25 to 27 PDF 1.5; `Opt`
/// on a button and `I` on a choice field are PDF 1.4.
fn field(d: &Dict, bits: u64, ff: Option<i64>, note: &mut impl FnMut(u8, &str)) {
    flag_versions(ff, &[(0x00F0_0000, 14), (0x0700_0000, 15)], "field flag", note);
    let untyped = bits & (FT_BTN | FT_CH | FT_TX | FT_SIG) == 0;
    if d.contains(b"Opt") && (untyped || bits & FT_BTN != 0) {
        note(14, "/Opt on a button field");
    }
    if d.contains(b"I") && (untyped || bits & FT_CH != 0) {
        note(14, "/I on a choice field");
    }
}
