//! What kind of dictionary each dictionary is: a bit set built from its own entries (`/Type`,
//! `/Subtype`, `/FT`, `/S`) and from the keys it is reached through. A key whose version depends
//! on its dictionary is judged by these bits; a dictionary with none is `UNKNOWN`.

use pdfcraft_cos::Dict;

pub(super) const TRAILER: u64 = 1;
pub(super) const CATALOG: u64 = 1 << 1;
pub(super) const PAGE: u64 = 1 << 2;
pub(super) const STREAM: u64 = 1 << 3;
pub(super) const ENCRYPT: u64 = 1 << 4;
pub(super) const FILESPEC: u64 = 1 << 5;
pub(super) const EXTENSIONS: u64 = 1 << 6;
pub(super) const DEV_EXT: u64 = 1 << 7;
pub(super) const RESOURCES: u64 = 1 << 8;
pub(super) const GS_MAP: u64 = 1 << 9;
pub(super) const EXTGSTATE: u64 = 1 << 10;
pub(super) const PROPS_MAP: u64 = 1 << 11;
pub(super) const MC_PROPS: u64 = 1 << 12;
pub(super) const IMAGE: u64 = 1 << 13;
pub(super) const FORM: u64 = 1 << 14;
pub(super) const FONT_DESC: u64 = 1 << 15;
pub(super) const OUTLINES: u64 = 1 << 16;
pub(super) const OUTLINE: u64 = 1 << 17;
pub(super) const THREAD: u64 = 1 << 18;
pub(super) const TRANSITION: u64 = 1 << 19;
pub(super) const ANNOT: u64 = 1 << 20;
pub(super) const MARKUP: u64 = 1 << 21;
pub(super) const NON_MARKUP: u64 = 1 << 22;
pub(super) const LINK: u64 = 1 << 23;
pub(super) const FREE_TEXT: u64 = 1 << 24;
pub(super) const LINE: u64 = 1 << 25;
pub(super) const STAMP: u64 = 1 << 26;
pub(super) const ANNOT_3D: u64 = 1 << 27;
pub(super) const AA: u64 = 1 << 28;
pub(super) const ACTION: u64 = 1 << 29;
pub(super) const SUBMIT: u64 = 1 << 30;
pub(super) const ACROFORM: u64 = 1 << 31;
pub(super) const FIELD: u64 = 1 << 32;
pub(super) const FT_BTN: u64 = 1 << 33;
pub(super) const FT_CH: u64 = 1 << 34;
pub(super) const FT_TX: u64 = 1 << 35;
pub(super) const FT_SIG: u64 = 1 << 36;
pub(super) const SIG_LOCK: u64 = 1 << 37;
pub(super) const SEED_VALUE: u64 = 1 << 38;
pub(super) const SIGNATURE: u64 = 1 << 39;
pub(super) const STRUCT_ROOT: u64 = 1 << 40;
pub(super) const STRUCT_ELEM: u64 = 1 << 41;
pub(super) const ATTR_OBJ: u64 = 1 << 42;
pub(super) const OUTPUT_INTENT: u64 = 1 << 43;
pub(super) const COLLECTION: u64 = 1 << 44;
pub(super) const FOLDER: u64 = 1 << 45;
pub(super) const THREE_D: u64 = 1 << 46;
pub(super) const MEASURE: u64 = 1 << 47;
pub(super) const REQUIREMENT: u64 = 1 << 48;
pub(super) const CONTENT: u64 = 1 << 49;
pub(super) const TYPE3_FONT: u64 = 1 << 50;
pub(super) const CHAR_PROCS: u64 = 1 << 51;
pub(super) const BORDER_EFFECT: u64 = 1 << 52;
pub(super) const WIDGET: u64 = 1 << 53;
pub(super) const VIEW_3D: u64 = 1 << 54;
pub(super) const NODE_3D: u64 = 1 << 55;
/// A stream named in a page's `/Contents` array: read joined with the rest of the array, so not
/// on its own unless something else reaches it as content.
pub(super) const PIECE: u64 = 1 << 56;
pub(super) const UNKNOWN: u64 = 1 << 63;

/// Every bit: a rule that applies whatever the dictionary is.
pub(super) const ANY: u64 = u64::MAX;

/// The field-type bits a field's `/Kids` inherit (§12.7.4.1: `FT` is inheritable).
const FT_BITS: u64 = FT_BTN | FT_CH | FT_TX | FT_SIG;

/// Annotation subtypes that are markup annotations (ISO 32000-2 Table 171).
const MARKUP_SUBTYPES: &[&[u8]] = &[
    b"Text",
    b"FreeText",
    b"Line",
    b"Square",
    b"Circle",
    b"Polygon",
    b"PolyLine",
    b"Highlight",
    b"Underline",
    b"Squiggly",
    b"StrikeOut",
    b"Caret",
    b"Stamp",
    b"Ink",
    b"FileAttachment",
    b"Sound",
    b"Redact",
    b"Projection",
];

/// Annotation subtypes that are not markup annotations (Table 171).
const OTHER_SUBTYPES: &[&[u8]] = &[b"Link", b"Popup", b"Movie", b"Screen", b"Widget", b"PrinterMark", b"TrapNet", b"Watermark", b"3D", b"RichMedia"];

/// Action types (§12.6.4, Table 201).
const ACTION_TYPES: &[&[u8]] = &[
    b"GoTo",
    b"GoToR",
    b"GoToE",
    b"GoToDp",
    b"Launch",
    b"Thread",
    b"URI",
    b"Sound",
    b"Movie",
    b"Hide",
    b"Named",
    b"SubmitForm",
    b"ResetForm",
    b"ImportData",
    b"JavaScript",
    b"SetOCGState",
    b"Rendition",
    b"Trans",
    b"GoTo3DView",
    b"RichMediaExecute",
];

/// `/Type` values naming a 3D dictionary (§13.6).
const THREE_D_TYPES: &[&[u8]] = &[
    b"3D",
    b"3DRef",
    b"3DView",
    b"3DBG",
    b"3DCrossSection",
    b"3DNode",
    b"3DRenderMode",
    b"3DLightingScheme",
    b"3DAnimationStyle",
    b"3DMeasure",
    b"3DUnits",
];

/// The bits a dictionary earns from its own entries.
pub(super) fn own(d: &Dict, is_stream: bool) -> u64 {
    let mut bits = if is_stream { STREAM } else { 0 };
    let subtype = d.name(b"Subtype");
    bits |= d.name(b"Type").map_or(0, by_type);
    if is_stream {
        bits |= match subtype {
            Some(b"Image") => IMAGE,
            Some(b"Form") => FORM | CONTENT,
            Some(b"U3D" | b"PRC") => THREE_D,
            _ => 0,
        };
        if d.int(b"PatternType") == Some(1) {
            bits |= CONTENT;
        }
    }
    if d.contains(b"Rect") || bits & ANNOT != 0 {
        bits |= subtype.map_or(0, annot_bits);
    }
    bits |= match d.name(b"FT") {
        Some(b"Btn") => FIELD | FT_BTN,
        Some(b"Ch") => FIELD | FT_CH,
        Some(b"Tx") => FIELD | FT_TX,
        Some(b"Sig") => FIELD | FT_SIG,
        _ => 0,
    };
    if let Some(s) = d.name(b"S").filter(|s| ACTION_TYPES.contains(s)) {
        bits |= ACTION | if s == b"SubmitForm" { SUBMIT } else { 0 };
    }
    if d.contains(b"ByteRange") && d.contains(b"Contents") {
        bits |= SIGNATURE;
    }
    if subtype == Some(b"Type3") {
        bits |= TYPE3_FONT;
    }
    bits
}

fn by_type(t: &[u8]) -> u64 {
    match t {
        b"Catalog" => CATALOG,
        b"Page" => PAGE,
        b"ExtGState" => EXTGSTATE,
        b"FontDescriptor" => FONT_DESC,
        b"Annot" => ANNOT,
        b"Filespec" => FILESPEC,
        b"Folder" => FOLDER,
        b"Collection" => COLLECTION,
        b"Trans" => TRANSITION,
        b"Thread" => THREAD,
        b"StructElem" => STRUCT_ELEM,
        b"StructTreeRoot" => STRUCT_ROOT,
        b"Measure" => MEASURE,
        b"Requirement" => REQUIREMENT,
        b"SV" => SEED_VALUE,
        b"SigFieldLock" => SIG_LOCK,
        b"Sig" | b"DocTimeStamp" => SIGNATURE,
        b"DeveloperExtensions" => DEV_EXT,
        b"OutputIntent" => OUTPUT_INTENT,
        b"Action" => ACTION,
        b"Outlines" => OUTLINES,
        b"3DView" => THREE_D | VIEW_3D,
        b"3DNode" => THREE_D | NODE_3D,
        t if THREE_D_TYPES.contains(&t) => THREE_D,
        _ => 0,
    }
}

fn annot_bits(subtype: &[u8]) -> u64 {
    let kind = if MARKUP_SUBTYPES.contains(&subtype) {
        ANNOT | MARKUP
    } else if OTHER_SUBTYPES.contains(&subtype) {
        ANNOT | NON_MARKUP
    } else {
        return 0;
    };
    kind | match subtype {
        b"Link" => LINK,
        b"Widget" => WIDGET,
        b"FreeText" => FREE_TEXT,
        b"Line" => LINE,
        b"Stamp" => STAMP,
        b"3D" => ANNOT_3D,
        _ => 0,
    }
}

/// The bits a value reached from a `parent` dictionary through `key` earns.
pub(super) fn edge(parent: u64, key: &[u8]) -> u64 {
    let on = |mask: u64, bits: u64| if parent & mask != 0 { bits } else { 0 };
    let mut bits = match key {
        b"Resources" => RESOURCES,
        b"Measure" => MEASURE,
        b"FontDescriptor" => FONT_DESC,
        b"AA" => AA,
        b"A" | b"OpenAction" => on(ANNOT | OUTLINE | CATALOG, ACTION),
        b"Next" => on(ACTION, ACTION) | on(OUTLINE, OUTLINE) | on(FOLDER, FOLDER),
        _ => 0,
    };
    bits |= match key {
        b"Root" => on(TRAILER, CATALOG),
        b"Encrypt" => on(TRAILER, ENCRYPT),
        b"ExtGState" => on(RESOURCES, GS_MAP),
        b"Properties" => on(RESOURCES, PROPS_MAP),
        b"Annots" => on(PAGE, ANNOT),
        b"Trans" => on(PAGE, TRANSITION),
        b"Contents" => on(PAGE, CONTENT),
        b"OutputIntents" => on(CATALOG | PAGE, OUTPUT_INTENT),
        b"Outlines" => on(CATALOG, OUTLINES),
        b"First" | b"Last" | b"Prev" => on(OUTLINES | OUTLINE, OUTLINE),
        b"Threads" => on(CATALOG, THREAD),
        b"AcroForm" => on(CATALOG, ACROFORM),
        b"Fields" => on(ACROFORM, FIELD),
        b"Kids" => on(FIELD, FIELD | (parent & FT_BITS)),
        b"Parent" => on(WIDGET, FIELD),
        b"Lock" => on(FIELD, SIG_LOCK),
        b"SV" => on(FIELD, SEED_VALUE),
        b"V" => on(FT_SIG, SIGNATURE),
        b"BE" => on(ANNOT, BORDER_EFFECT),
        b"StructTreeRoot" => on(CATALOG, STRUCT_ROOT),
        b"K" => on(STRUCT_ROOT | STRUCT_ELEM, STRUCT_ELEM),
        b"Collection" => on(CATALOG, COLLECTION),
        b"Folders" | b"Child" => on(COLLECTION | FOLDER, FOLDER),
        b"Requirements" => on(CATALOG, REQUIREMENT),
        b"Extensions" => on(CATALOG, EXTENSIONS),
        b"CharProcs" => on(TYPE3_FONT, CHAR_PROCS),
        b"3DD" | b"3DA" => on(ANNOT_3D, THREE_D),
        b"3DV" => on(ANNOT_3D, THREE_D | VIEW_3D),
        b"VA" | b"DV" => on(THREE_D, THREE_D | VIEW_3D),
        b"NA" => on(VIEW_3D, THREE_D | NODE_3D),
        b"BG" | b"SA" | b"RM" | b"LS" => on(THREE_D, THREE_D),
        _ => 0,
    };
    if key == b"A" && parent & STRUCT_ELEM != 0 {
        bits |= ATTR_OBJ;
    }
    bits | map_value(parent)
}

/// Every value of a map dictionary is the same kind of dictionary.
fn map_value(parent: u64) -> u64 {
    [(GS_MAP, EXTGSTATE), (PROPS_MAP, MC_PROPS), (EXTENSIONS, DEV_EXT), (CHAR_PROCS, CONTENT)]
        .into_iter()
        .filter(|(map, _)| parent & map != 0)
        .fold(0, |bits, (_, value)| bits | value)
}

/// `bits` as the rule tables see them: `UNKNOWN` when nothing but being a stream or content
/// classified the dictionary.
pub(super) fn settled(bits: u64) -> u64 {
    if bits & !(STREAM | CONTENT | PIECE) == 0 { bits | UNKNOWN } else { bits }
}
