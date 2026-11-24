//! Constructs only a stream's data shows: inline images and marked-content properties in content
//! streams (§8.9.7, §14.6), and the OpenType wrapper of a font program (§9.9).

use pdfcraft_cos::{Dict, Object};

use super::keys;
use super::scan::{Notes, lossy};

/// Note what content stream `data` (decoded) needs.
pub(super) fn scan_content(data: &[u8], notes: &mut Notes) {
    for op in pdfcraft_content::ops(data) {
        if let Some((params, _)) = &op.inline {
            inline_image(params, notes);
            continue;
        }
        if !matches!(op.op.as_slice(), b"BDC" | b"DP" | b"BMC" | b"MP") {
            continue;
        }
        let tag = op.operands.first().and_then(Object::as_name);
        marked_content(tag, op.operands.get(1).and_then(Object::as_dict), notes);
    }
}

/// §8.9.7 Table 92: `/L` (and its long form) is PDF 2.0; 16-bit samples are PDF 1.5.
fn inline_image(params: &Dict, notes: &mut Notes) {
    if params.contains(b"L") || params.contains(b"Length") {
        notes.insert((20, "inline image /L".to_owned()));
    }
    if [b"BPC".as_slice(), b"BitsPerComponent"].iter().any(|k| params.int(k) == Some(16)) {
        notes.insert((15, "16-bit image samples".to_owned()));
    }
    let names: Vec<&[u8]> = match params.get(b"Filter").or_else(|| params.get(b"F")) {
        Some(Object::Name(n)) => vec![n.as_slice()],
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_name).collect(),
        _ => Vec::new(),
    };
    for n in names {
        if let Some(ver) = keys::filter(n) {
            notes.insert((ver, format!("/Filter /{}", lossy(n))));
        }
    }
}

/// §14.6 and §14.9: the optional-content tag is PDF 1.5, `ReversedChars` 1.4, `TagSuspect` 1.6;
/// inline properties `ActualText` (1.4 on `Span`, else 1.5), `Alt`, `E`, `Lang` (1.5) and `AF`,
/// `MCAF`, `Artifact` (2.0).
fn marked_content(tag: Option<&[u8]>, props: Option<&Dict>, notes: &mut Notes) {
    let tag_ver = match tag {
        Some(b"OC") => Some(15),
        Some(b"ReversedChars") => Some(14),
        Some(b"TagSuspect") => Some(16),
        _ => None,
    };
    if let (Some(ver), Some(t)) = (tag_ver, tag) {
        notes.insert((ver, format!("marked-content tag /{}", lossy(t))));
    }
    let Some(props) = props else { return };
    for (k, _) in props.iter() {
        let ver = match k.as_slice() {
            b"ActualText" if tag == Some(b"Span") => 14,
            b"ActualText" | b"Alt" | b"E" | b"Lang" => 15,
            b"AF" | b"MCAF" | b"Artifact" => 20,
            _ => continue,
        };
        notes.insert((ver, format!("marked-content /{}", lossy(k))));
    }
}

/// §9.9 Table 124: an OpenType font program is PDF 1.6.
pub(super) fn scan_font(data: &[u8], notes: &mut Notes) {
    if data.starts_with(b"OTTO") {
        notes.insert((16, "OpenType font program".to_owned()));
    }
}
