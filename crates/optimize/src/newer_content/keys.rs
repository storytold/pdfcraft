//! The version each dictionary key and name value was introduced in, from ISO 32000-2's
//! "PDF 1.x" and "PDF 2.0" markings in its key tables.
//!
//! A rule is a list of `(context mask, version)` pairs; the first whose mask meets the
//! dictionary's context bits applies. Versions are the major number times ten plus the minor
//! (`14` is 1.4, `20` is 2.0). Only constructs of 1.4 and later are listed: nothing older than the
//! lowest compatibility level is told apart.

use super::ctx::{
    AA, ANNOT, ANY, ATTR_OBJ, BORDER_EFFECT, CATALOG, COLLECTION, DEV_EXT, ENCRYPT, EXTGSTATE, FIELD, FILESPEC, FOLDER, FONT_DESC, FORM, FREE_TEXT,
    IMAGE, LINE, LINK, MARKUP, MC_PROPS, MEASURE, NODE_3D, NON_MARKUP, OUTLINE, OUTPUT_INTENT, PAGE, REQUIREMENT, SEED_VALUE, SIG_LOCK, STAMP,
    STREAM, STRUCT_ELEM, SUBMIT, THREAD, THREE_D, TRANSITION, UNKNOWN,
};

pub(super) type Rule = &'static [(u64, u8)];

/// The first rule entry whose mask meets `bits`.
pub(super) fn applies(rule: Rule, bits: u64) -> Option<u8> {
    rule.iter().find(|(mask, _)| bits & mask != 0).map(|&(_, v)| v)
}

/// The rule for dictionary key `key`.
pub(super) fn key(key: &[u8]) -> Option<Rule> {
    structure(key)
        .or_else(|| document(key))
        .or_else(|| graphics(key))
        .or_else(|| annotation(key))
        .or_else(|| interactive(key))
        .or_else(|| tagged(key))
}

const A14: Rule = &[(ANY, 14)];
const A15: Rule = &[(ANY, 15)];
const A16: Rule = &[(ANY, 16)];
const A17: Rule = &[(ANY, 17)];
const A20: Rule = &[(ANY, 20)];

/// §7.3.8 streams, §7.6 encryption, §7.12 extensions.
fn structure(key: &[u8]) -> Option<Rule> {
    Some(match key {
        b"DL" => &[(STREAM, 15)],
        // The encryption dictionary and its crypt filters are never scanned (the optimizer's own
        // encryption check covers them), so these keys don't count anywhere else.
        b"CF" | b"StmF" | b"StrF" | b"EncryptMetadata" | b"CFM" | b"AuthEvent" => &[(ENCRYPT, 15)],
        b"EFF" => &[(ENCRYPT, 16)],
        b"OE" | b"UE" => &[(ENCRYPT, 20)],
        b"ExtensionRevision" => &[(DEV_EXT, 20)],
        b"Perms" => &[(CATALOG, 15), (ENCRYPT | UNKNOWN, 20)],
        b"URL" => &[(DEV_EXT, 20)],
        _ => return None,
    })
}

/// Catalog, page, name-dictionary, file-specification, viewer-preference,
/// outline, transition and collection keys (§7.7, §7.11, §12.2-§12.4).
fn document(key: &[u8]) -> Option<Rule> {
    Some(match key {
        b"AA" => &[(CATALOG, 14)],
        b"OutputIntents" => &[(CATALOG, 14), (PAGE, 20)],
        b"PieceInfo" => &[(CATALOG, 14)],
        b"MarkInfo"
        | b"BoxColorInfo"
        | b"Group"
        | b"EmbeddedFiles"
        | b"AlternatePresentations"
        | b"DisplayDocTitle"
        | b"ViewArea"
        | b"ViewClip"
        | b"PrintArea"
        | b"PrintClip" => A14,
        b"OCProperties" | b"Legal" | b"Tabs" | b"TemplateInstantiated" | b"PresSteps" | b"Renditions" => A15,
        b"SS" => &[(TRANSITION, 15)],
        b"UserUnit" | b"VP" | b"PrintScaling" => A16,
        b"CI" => &[(FILESPEC | UNKNOWN, 17)],
        b"EP" => &[(FILESPEC | UNKNOWN, 20)],
        b"Extensions" | b"Requirements" | b"Collection" | b"NeedsRendering" | b"UF" | b"Duplex" | b"PickTrayByPDFSize" | b"PrintPageRange"
        | b"NumCopies" => A17,
        b"DSS" | b"DPartRoot" | b"AF" | b"DPart" | b"AFRelationship" | b"Enforce" | b"CompressedSize" => A20,
        b"Metadata" => &[(THREAD, 20), (ANY, 14)],
        b"Desc" => &[(FILESPEC, 16), (ATTR_OBJ, 17), (FOLDER, 20), (UNKNOWN, 17)],
        b"Thumb" => &[(FILESPEC | FOLDER, 20)],
        b"C" => &[(OUTLINE, 14)],
        b"F" => &[(OUTLINE, 14)],
        b"B" => &[(TRANSITION, 15)],
        b"Navigator" | b"Colors" | b"Folders" | b"Split" => &[(COLLECTION, 20)],
        _ => return None,
    })
}

/// Graphics state, images, colour, optional content and fonts (§8, §9).
fn graphics(key: &[u8]) -> Option<Rule> {
    Some(match key {
        b"SMask" | b"AIS" | b"TK" | b"Matte" => A14,
        b"SMaskInData" | b"OC" | b"FontFamily" | b"FontStretch" | b"FontWeight" => A15,
        b"Process" | b"Colorants" | b"VE" | b"Locked" => A16,
        b"UseBlackPtComp" | b"HTO" | b"PtData" | b"DestOutputProfileRef" | b"SpectralData" => A20,
        b"BM" => &[(EXTGSTATE | UNKNOWN, 14), (ANNOT, 20)],
        b"CA" => &[(EXTGSTATE | MARKUP | UNKNOWN, 14), (NON_MARKUP, 20)],
        b"ca" => &[(EXTGSTATE | UNKNOWN, 14), (ANNOT, 20)],
        b"MixingHints" => &[(OUTPUT_INTENT, 20), (ANY, 16)],
        b"Ref" => &[(FORM, 14), (STRUCT_ELEM | UNKNOWN, 20)],
        b"Lang" => &[(CATALOG | STRUCT_ELEM, 14), (ANNOT, 20), (FONT_DESC | MC_PROPS | UNKNOWN, 15)],
        _ => return None,
    })
}

/// Annotation keys (§12.5), including measurement (§12.9).
fn annotation(key: &[u8]) -> Option<Rule> {
    Some(match key {
        b"NM" | b"MarkStyle" | b"AnnotStates" => A14,
        b"RC" | b"IRT" | b"Subj" | b"State" | b"StateModel" | b"DS" => A15,
        b"RT" => A16,
        b"ExData" | b"LLO" | b"CP" => A17,
        b"CL" | b"LL" | b"LLE" | b"Cap" => A16,
        b"Path" => &[(ANNOT, 20)],
        b"GCS" | b"DCS" | b"PDU" | b"GPTS" | b"LPTS" | b"PCSM" | b"EPSG" | b"WKT" | b"XPTS" | b"Penalty" => A20,
        b"CreationDate" => &[(MARKUP, 15)],
        b"IT" => &[(STAMP, 20), (ANNOT | UNKNOWN, 16)],
        b"QuadPoints" => &[(LINK, 16)],
        b"BS" => &[(LINK | FREE_TEXT, 16)],
        b"BE" | b"RD" => &[(FREE_TEXT, 16), (ANNOT, 15)],
        b"LE" => &[(FREE_TEXT, 16), (ANNOT, 14)],
        b"IC" => &[(ANNOT, 14)],
        b"Measure" => &[(IMAGE | FORM, 20), (ANNOT, 17), (ANY, 16)],
        b"CO" => &[(LINE, 17)],
        b"Q" => &[(FREE_TEXT, 14)],
        b"Bounds" => &[(MEASURE, 20)],
        _ => return None,
    })
}

/// Actions, forms, signatures, requirements and 3D (§12.6-§12.10, §13.6).
fn interactive(key: &[u8]) -> Option<Rule> {
    Some(match key {
        b"PO" | b"PC" | b"PV" | b"PI" => &[(AA, 15)],
        b"WC" | b"WS" | b"WP" | b"DP" => &[(AA, 14)],
        b"SD"
        | b"LockDocument"
        | b"AppearanceFilter"
        | b"SignaturePolicyOID"
        | b"SignaturePolicyHashValue"
        | b"SignaturePolicyHashAlgorithm"
        | b"SignaturePolicyCommitmentType"
        | b"3DU"
        | b"GEO" => A20,
        b"CharSet" => &[(SUBMIT, 20)],
        b"XFA" | b"RV" => A15,
        b"Lock" | b"SV" => &[(FIELD, 15)],
        b"P" => &[(SIG_LOCK, 20)],
        b"MDP" | b"TimeStamp" => &[(SEED_VALUE, 16)],
        b"LegalAttestation" => A16,
        b"AddRevInfo" | b"SubjectDN" | b"KeyUsage" | b"URLType" | b"NP" => A17,
        b"DigestMethod" => &[(SEED_VALUE, 17)],
        b"V" => &[(REQUIREMENT, 20)],
        b"Style" | b"Window" | b"Transparent" | b"ColorSpace" | b"MA" | b"ST" | b"SC" => &[(THREE_D, 20)],
        b"PRC" | b"Instance" | b"Data" => &[(NODE_3D, 20)],
        b"RM" => &[(NODE_3D, 20), (THREE_D, 17)],
        b"TB" | b"AN" | b"LS" | b"SA" | b"NA" | b"NR" | b"OB" => &[(THREE_D, 17)],
        _ => return None,
    })
}

/// Logical structure and tagged-PDF attributes (§14.7, §14.8, §14.9).
fn tagged(key: &[u8]) -> Option<Rule> {
    const ATTR15: Rule = &[(ATTR_OBJ | UNKNOWN, 15)];
    const ATTR16: Rule = &[(ATTR_OBJ | UNKNOWN, 16)];
    const ATTR17: Rule = &[(ATTR_OBJ | UNKNOWN, 17)];
    const ATTR20: Rule = &[(ATTR_OBJ | UNKNOWN, 20)];
    Some(match key {
        b"UserProperties" | b"Suspects" => A16,
        b"Namespaces" | b"PronunciationLexicon" | b"NS" | b"PhoneticAlphabet" | b"Phoneme" => A20,
        b"E" => &[(STRUCT_ELEM | MC_PROPS | UNKNOWN, 15)],
        b"ActualText" => &[(MC_PROPS, 15), (ANY, 14)],
        b"Alt" => &[(MC_PROPS, 15)],
        b"Artifact" => &[(MC_PROPS, 20)],
        b"BackgroundColor"
        | b"BorderColor"
        | b"BorderStyle"
        | b"BorderThickness"
        | b"Padding"
        | b"Color"
        | b"TBorderStyle"
        | b"TPadding"
        | b"TextDecorationColor"
        | b"TextDecorationThickness"
        | b"RubyAlign"
        | b"RubyPosition"
        | b"GlyphOrientationVertical"
        | b"Scope"
        | b"Headers" => ATTR15,
        b"ColumnCount" | b"ColumnGap" | b"ColumnWidths" => ATTR16,
        b"Role" | b"Checked" | b"Summary" => ATTR17,
        b"TextPosition" | b"ContinuedList" | b"ContinuedFrom" | b"Short" => ATTR20,
        _ => return None,
    })
}

/// The version a name `value` of key `key` needs, in a dictionary with
/// context `bits` (filters §7.4, crypt filters §7.6.6, signatures §12.8,
/// and the `/Type`, `/S`, `/Subtype` and enumerated names of §7-§14).
pub(super) fn value(key: &[u8], value: &[u8], bits: u64) -> Option<u8> {
    match key {
        b"Filter" => filter(value),
        b"CFM" => match value {
            b"V2" => Some(15),
            b"AESV2" => Some(16),
            b"AESV3" => Some(20),
            _ => None,
        },
        b"SubFilter" if value == b"adbe.pkcs7.sha1" => Some(14),
        b"SubFilter" if value.starts_with(b"ETSI.") => Some(20),
        b"Type" => type_value(value),
        b"S" => s_value(value, bits),
        b"Subtype" => subtype_value(value, bits),
        b"Encoding" | b"UseCMap" => cmap(value),
        _ => enumerated(key, value, bits),
    }
}

/// A predefined CMap name first listed in PDF 1.4 or 1.5 (1.7 §9.7.5.2
/// Table 119); an older reader raises an error on a name it does not
/// predefine. The names are unique to CMaps, so no context is needed.
fn cmap(name: &[u8]) -> Option<u8> {
    match name {
        b"GBKp-EUC-H" | b"GBKp-EUC-V" | b"GBK2K-H" | b"GBK2K-V" | b"HKscs-B5-H" | b"HKscs-B5-V" => Some(14),
        b"UniGB-UTF16-H" | b"UniGB-UTF16-V" | b"UniCNS-UTF16-H" | b"UniCNS-UTF16-V" | b"UniJIS-UTF16-H" | b"UniJIS-UTF16-V" | b"UniKS-UTF16-H"
        | b"UniKS-UTF16-V" => Some(15),
        _ => None,
    }
}

/// The version a stream filter name needs.
pub(super) fn filter(name: &[u8]) -> Option<u8> {
    match name {
        b"JBIG2Decode" => Some(14),
        b"JPXDecode" | b"Crypt" => Some(15),
        b"BrotliDecode" => Some(20),
        _ => None,
    }
}

fn type_value(value: &[u8]) -> Option<u8> {
    match value {
        b"OutputIntent" | b"Metadata" | b"SlideShow" | b"Group" => Some(14),
        b"OCG" | b"OCMD" => Some(15),
        b"Collection" | b"CollectionItem" | b"CollectionSchema" | b"CollectionField" | b"CollectionSort" | b"Requirement" => Some(17),
        b"CollectionColors" | b"CollectionSplit" | b"Folder" | b"Navigator" | b"DSS" | b"VRI" | b"DPartRoot" | b"DPart" | b"Namespace"
        | b"DocTimeStamp" | b"Inline" | b"3DMeasure" | b"3DUnits" => Some(20),
        v if v.starts_with(b"RichMedia") => Some(20),
        _ => None,
    }
}

fn s_value(value: &[u8], bits: u64) -> Option<u8> {
    match value {
        b"Transparency" | b"Luminosity" | b"Alpha" => Some(14),
        b"Fly" | b"Push" | b"Cover" | b"Uncover" | b"Fade" | b"SetOCGState" | b"Rendition" | b"Trans" => Some(15),
        b"GoToE" | b"GoTo3DView" => Some(16),
        b"GoToDp" | b"RichMediaExecute" => Some(20),
        b"C" if bits & BORDER_EFFECT != 0 => Some(15),
        _ => None,
    }
}

fn subtype_value(value: &[u8], bits: u64) -> Option<u8> {
    let annot = bits & (ANNOT | UNKNOWN) != 0;
    match value {
        b"Squiggly" | b"PrinterMark" if annot => Some(14),
        b"Polygon" | b"PolyLine" | b"Caret" | b"Screen" if annot => Some(15),
        b"Watermark" | b"3D" if annot => Some(16),
        b"Redact" if annot => Some(17),
        b"Projection" | b"RichMedia" if annot => Some(20),
        b"OpenType" | b"NChannel" | b"RL" | b"U3D" => Some(16),
        b"Background" | b"Markup3D" => Some(17),
        b"PageNum" | b"LineNum" | b"Redaction" | b"Bates" | b"GEO" | b"Cloud" | b"PRC" | b"3DM" | b"MarkupGeo" => Some(20),
        _ => None,
    }
}

fn enumerated(key: &[u8], value: &[u8], bits: u64) -> Option<u8> {
    match (key, value) {
        (b"RT", b"Group" | b"R") => Some(15),
        (b"LE", b"Butt" | b"ROpenArrow" | b"RClosedArrow") => Some(15),
        (b"LE", b"Slash") => Some(16),
        (b"IT", b"PolyLineDimension" | b"PolygonDimension") => Some(17),
        (b"IT", _) if bits & STAMP == 0 => Some(16),
        (b"PageLayout", b"TwoPageLeft" | b"TwoPageRight") => Some(15),
        (b"PageMode", b"UseOC") => Some(15),
        (b"PageMode", b"UseAttachments") => Some(16),
        (b"NonFullScreenPageMode", b"UseAttachments") => Some(20),
        (b"Tabs", b"A" | b"W") => Some(20),
        (b"View", b"C") if bits & COLLECTION != 0 => Some(20),
        _ => None,
    }
}
