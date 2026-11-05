//! PDF/A-2 6.6.2.3.1: a property of a predefined XMP schema has the value type the XMP
//! Specification gives it.
//!
//! The types come from ISO 16684-1:2011 (XMP Part 1) clause 8 for the Dublin Core, XMP Basic and
//! XMP Rights Management namespaces, and from XMP Specification Part 2 (Additional Properties) for
//! the Media Management, Basic Job Ticket, Paged-Text, Dynamic Media, Adobe PDF, Photoshop and
//! Camera Raw namespaces, with the structures they use. A value is read as RDF/XML serializes it
//! (ISO 16684-1 7.9): a simple value as text or an attribute, an array as `rdf:Seq`, `rdf:Bag` or
//! `rdf:Alt`, a structure as `rdf:parseType="Resource"`, an `rdf:Description` or attributes, and a
//! qualified value through its `rdf:value` (element or attribute). Every metadata stream of the
//! document is read, not only the catalog's, within one decode allowance.
//!
//! The simple types are checked by their lexical form (ISO 16684-1 8.2.1): Boolean is `True` or
//! `False`; Integer an optionally signed run of digits; Real a decimal; Date one of the six forms
//! of 8.2.1.2; Rational two integers around a `/`, as Part 2's `xmpDM:scale` example writes it.
//! The other simple types are text. A field a structure doesn't define, and a property a
//! namespace doesn't define, are left to the clauses about them.

use std::collections::BTreeSet;

use pdfcraft_cos::{DecodeBudget, ObjRef, Object};
use roxmltree::Node;

/// How deep arrays, structures and qualifiers are followed. No type nests nearly this deep, so a
/// value nested deeper doesn't have its type.
const MAX_DEPTH: usize = 32;
/// The decoded bytes all metadata streams of a document may take together.
const MAX_METADATA_BYTES: usize = 64 << 20;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// A value type.
#[derive(Debug, Clone, Copy)]
enum Ty {
    /// Any simple value: Text and the types derived from it (AgentName, Choice of Text, Locale,
    /// MIMEType, ProperName, RenditionClass, URI, URL, XPath, and the like).
    Text,
    /// A GUID: any simple value, but not a URI (ISO 16684-1 8.2.2.3), so not an `rdf:resource`.
    Guid,
    Integer,
    Real,
    Boolean,
    Date,
    Rational,
    /// An alternative array of text items, each with an `xml:lang` (ISO 16684-1 8.2.2.4).
    LangAlt,
    Seq(&'static Ty),
    Bag(&'static Ty),
    Alt(&'static Ty),
    Struct(&'static [Field]),
    /// A structure whose fields aren't given (`xmpMM:Pantry`).
    AnyStruct,
    /// Any of these: where editions of the specification differ.
    OneOf(&'static [Ty]),
}

/// A structure field: namespace, local name, type.
type Field = (&'static str, &'static str, Ty);

/// A predefined namespace: URI, preferred prefix, properties.
struct Schema {
    uri: &'static str,
    prefix: &'static str,
    properties: &'static [(&'static str, Ty)],
}

use Ty::{Boolean, Date, Integer, LangAlt, Rational, Real, Text};

const ST_DIM: &str = "http://ns.adobe.com/xap/1.0/sType/Dimensions#";
// Part 2 1.2.2.3 prints the Font field namespace as "http:ns.adobe.com/xap/1.0/sType/Font#"; the
// other sType namespaces show the intended form.
const ST_FNT: &str = "http://ns.adobe.com/xap/1.0/sType/Font#";
const XMP_G: &str = "http://ns.adobe.com/xap/1.0/g/";
const XMP_G_IMG: &str = "http://ns.adobe.com/xap/1.0/g/img/";
const ST_EVT: &str = "http://ns.adobe.com/xap/1.0/sType/ResourceEvent#";
const ST_REF: &str = "http://ns.adobe.com/xap/1.0/sType/ResourceRef#";
const ST_VER: &str = "http://ns.adobe.com/xap/1.0/sType/Version#";
const ST_JOB: &str = "http://ns.adobe.com/xap/1.0/sType/Job#";
const XMP_DM: &str = "http://ns.adobe.com/xmp/1.0/DynamicMedia/";
const PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";

/// Part 2 Table 3.
const COLORANT: &[Field] = &[
    (XMP_G, "A", Integer),
    (XMP_G, "B", Integer),
    (XMP_G, "L", Real),
    (XMP_G, "black", Real),
    (XMP_G, "cyan", Real),
    (XMP_G, "magenta", Real),
    (XMP_G, "yellow", Real),
    (XMP_G, "blue", Integer),
    (XMP_G, "green", Integer),
    (XMP_G, "red", Integer),
    (XMP_G, "mode", Text),
    (XMP_G, "swatchName", Text),
    (XMP_G, "type", Text),
];
/// Part 2 Table 4.
const DIMENSIONS: &[Field] = &[(ST_DIM, "h", Real), (ST_DIM, "w", Real), (ST_DIM, "unit", Text)];
/// Part 2 Table 5.
const FONT: &[Field] = &[
    (ST_FNT, "childFontFiles", Ty::Seq(&Text)),
    (ST_FNT, "composite", Boolean),
    (ST_FNT, "fontFace", Text),
    (ST_FNT, "fontFamily", Text),
    (ST_FNT, "fontFileName", Text),
    (ST_FNT, "fontName", Text),
    (ST_FNT, "fontType", Text),
    (ST_FNT, "versionString", Text),
];
/// Part 2 Table 6.
const THUMBNAIL: &[Field] = &[(XMP_G_IMG, "format", Text), (XMP_G_IMG, "height", Integer), (XMP_G_IMG, "width", Integer), (XMP_G_IMG, "image", Text)];
/// Part 2 Table 8.
const RESOURCE_EVENT: &[Field] = &[
    (ST_EVT, "action", Text),
    (ST_EVT, "changed", Text),
    (ST_EVT, "instanceID", Ty::Guid),
    (ST_EVT, "parameters", Text),
    (ST_EVT, "softwareAgent", Text),
    (ST_EVT, "when", Date),
];
/// ISO 16684-1 Table 3 and Part 2 Table 9.
const RESOURCE_REF: &[Field] = &[
    (ST_REF, "alternatePaths", Ty::Seq(&Text)),
    (ST_REF, "documentID", Ty::Guid),
    (ST_REF, "filePath", Text),
    (ST_REF, "fromPart", Text),
    (ST_REF, "instanceID", Ty::Guid),
    (ST_REF, "lastModifyDate", Date),
    (ST_REF, "manager", Text),
    (ST_REF, "managerVariant", Text),
    (ST_REF, "manageTo", Text),
    (ST_REF, "manageUI", Text),
    (ST_REF, "maskMarkers", Text),
    (ST_REF, "partMapping", Text),
    (ST_REF, "renditionClass", Text),
    (ST_REF, "renditionParams", Text),
    (ST_REF, "toPart", Text),
    (ST_REF, "versionID", Text),
];
/// Part 2 Table 10.
const VERSION: &[Field] = &[
    (ST_VER, "comments", Text),
    (ST_VER, "event", Ty::Struct(RESOURCE_EVENT)),
    (ST_VER, "modifier", Text),
    (ST_VER, "modifyDate", Date),
    (ST_VER, "version", Text),
];
/// Part 2 Table 11.
const JOB: &[Field] = &[(ST_JOB, "id", Text), (ST_JOB, "name", Text), (ST_JOB, "url", Text)];
/// Part 2 Table 12.
const BEAT_SPLICE_STRETCH: &[Field] =
    &[(XMP_DM, "riseInDecibel", Real), (XMP_DM, "riseInTimeDuration", Ty::Struct(TIME)), (XMP_DM, "useFileBeatsMarker", Boolean)];
/// Part 2 Table 13.
const CUE_POINT_PARAM: &[Field] = &[(XMP_DM, "key", Text), (XMP_DM, "value", Text)];
/// A marker's start and duration: FrameCount (text) in Part 2 Table 16, a Time structure in the
/// September 2005 edition that PDF/A-2 cites. Either is taken.
const FRAME_COUNT_OR_TIME: Ty = Ty::OneOf(&[Text, Ty::Struct(TIME)]);
/// Part 2 Table 16.
const MARKER: &[Field] = &[
    (XMP_DM, "comment", Text),
    (XMP_DM, "cuePointParams", Ty::Seq(&Ty::Struct(CUE_POINT_PARAM))),
    (XMP_DM, "cuePointType", Text),
    (XMP_DM, "duration", FRAME_COUNT_OR_TIME),
    (XMP_DM, "location", Text),
    (XMP_DM, "name", Text),
    (XMP_DM, "probability", Real),
    (XMP_DM, "speaker", Text),
    (XMP_DM, "startTime", FRAME_COUNT_OR_TIME),
    (XMP_DM, "target", Text),
    (XMP_DM, "type", Text),
];
/// Part 2 Table 17.
const MEDIA: &[Field] = &[
    (XMP_DM, "duration", Ty::Struct(TIME)),
    (XMP_DM, "managed", Boolean),
    (XMP_DM, "path", Text),
    (XMP_DM, "startTime", Ty::Struct(TIME)),
    (XMP_DM, "track", Text),
    (XMP_DM, "webStatement", Text),
];
/// Part 2 Table 18.
const PROJECT_LINK: &[Field] = &[(XMP_DM, "path", Text), (XMP_DM, "type", Text)];
/// Part 2 Table 19.
const RESAMPLE_STRETCH: &[Field] = &[(XMP_DM, "quality", Text)];
/// Part 2 Table 20.
const TIME: &[Field] = &[(XMP_DM, "scale", Rational), (XMP_DM, "value", Integer)];
/// Part 2 Table 21.
const TIMECODE: &[Field] = &[(XMP_DM, "timeFormat", Text), (XMP_DM, "timeValue", Text)];
/// Part 2 Table 22.
const TIME_SCALE_STRETCH: &[Field] = &[(XMP_DM, "frameOverlappingPercentage", Real), (XMP_DM, "frameSize", Real), (XMP_DM, "quality", Text)];
/// Part 2 Table 23.
const TRACK: &[Field] =
    &[(XMP_DM, "frameRate", Text), (XMP_DM, "markers", Ty::Seq(&Ty::Struct(MARKER))), (XMP_DM, "trackName", Text), (XMP_DM, "trackType", Text)];
/// Part 2 Table 31.
const ANCESTOR: &[Field] = &[(PHOTOSHOP, "AncestorID", Text)];
/// Part 2 Table 32.
const LAYER: &[Field] = &[(PHOTOSHOP, "LayerName", Text), (PHOTOSHOP, "LayerText", Text)];

const SCHEMAS: &[Schema] = &[
    // ISO 16684-1 Table 4.
    Schema {
        uri: "http://purl.org/dc/elements/1.1/",
        prefix: "dc",
        properties: &[
            ("contributor", Ty::Bag(&Text)),
            ("coverage", Text),
            ("creator", Ty::Seq(&Text)),
            ("date", Ty::Seq(&Date)),
            ("description", LangAlt),
            ("format", Text),
            ("identifier", Text),
            ("language", Ty::Bag(&Text)),
            ("publisher", Ty::Bag(&Text)),
            ("relation", Ty::Bag(&Text)),
            ("rights", LangAlt),
            ("source", Text),
            ("subject", Ty::Bag(&Text)),
            ("title", LangAlt),
            ("type", Ty::Bag(&Text)),
        ],
    },
    // ISO 16684-1 Table 5, with Part 2 Table 24.
    Schema {
        uri: "http://ns.adobe.com/xap/1.0/",
        prefix: "xmp",
        properties: &[
            ("Advisory", Ty::Bag(&Text)),
            ("BaseURL", Text),
            ("CreateDate", Date),
            ("CreatorTool", Text),
            ("Identifier", Ty::Bag(&Text)),
            ("Label", Text),
            ("MetadataDate", Date),
            ("ModifyDate", Date),
            ("Nickname", Text),
            ("Rating", Real),
            ("Thumbnails", Ty::Alt(&Ty::Struct(THUMBNAIL))),
        ],
    },
    // ISO 16684-1 Table 6.
    Schema {
        uri: "http://ns.adobe.com/xap/1.0/rights/",
        prefix: "xmpRights",
        properties: &[("Certificate", Text), ("Marked", Boolean), ("Owner", Ty::Bag(&Text)), ("UsageTerms", LangAlt), ("WebStatement", Text)],
    },
    // ISO 16684-1 Table 7, with Part 2 Tables 25 and 26.
    Schema {
        uri: "http://ns.adobe.com/xap/1.0/mm/",
        prefix: "xmpMM",
        properties: &[
            ("DerivedFrom", Ty::Struct(RESOURCE_REF)),
            ("DocumentID", Ty::Guid),
            ("History", Ty::Seq(&Ty::Struct(RESOURCE_EVENT))),
            ("Ingredients", Ty::Bag(&Ty::Struct(RESOURCE_REF))),
            ("InstanceID", Ty::Guid),
            ("ManagedFrom", Ty::Struct(RESOURCE_REF)),
            ("Manager", Text),
            ("ManageTo", Text),
            ("ManageUI", Text),
            ("ManagerVariant", Text),
            ("OriginalDocumentID", Ty::Guid),
            ("Pantry", Ty::Bag(&Ty::AnyStruct)),
            ("RenditionClass", Text),
            ("RenditionParams", Text),
            ("VersionID", Text),
            ("Versions", Ty::Seq(&Ty::Struct(VERSION))),
            ("LastURL", Text),
            ("RenditionOf", Ty::Struct(RESOURCE_REF)),
            ("SaveID", Integer),
        ],
    },
    // Part 2 Table 27.
    Schema { uri: "http://ns.adobe.com/xap/1.0/bj/", prefix: "xmpBJ", properties: &[("JobRef", Ty::Bag(&Ty::Struct(JOB)))] },
    // Part 2 Table 28.
    Schema {
        uri: "http://ns.adobe.com/xap/1.0/t/pg/",
        prefix: "xmpTPg",
        properties: &[
            ("Colorants", Ty::Seq(&Ty::Struct(COLORANT))),
            ("Fonts", Ty::Bag(&Ty::Struct(FONT))),
            ("MaxPageSize", Ty::Struct(DIMENSIONS)),
            ("NPages", Integer),
            ("PlateNames", Ty::Seq(&Text)),
        ],
    },
    // Part 2 Table 29.
    Schema {
        uri: XMP_DM,
        prefix: "xmpDM",
        properties: &[
            ("absPeakAudioFilePath", Text),
            ("album", Text),
            ("altTapeName", Text),
            ("altTimecode", Ty::Struct(TIMECODE)),
            ("artist", Text),
            ("audioModDate", Date),
            ("audioChannelType", Text),
            ("audioCompressor", Text),
            ("audioSampleRate", Integer),
            ("audioSampleType", Text),
            ("beatSpliceParams", Ty::Struct(BEAT_SPLICE_STRETCH)),
            ("cameraAngle", Text),
            ("cameraLabel", Text),
            ("cameraModel", Text),
            ("cameraMove", Text),
            ("client", Text),
            ("comment", Text),
            ("composer", Text),
            ("contributedMedia", Ty::Bag(&Ty::Struct(MEDIA))),
            ("copyright", Text),
            ("director", Text),
            ("directorPhotography", Text),
            ("duration", Ty::Struct(TIME)),
            ("engineer", Text),
            ("fileDataRate", Rational),
            ("genre", Text),
            ("good", Boolean),
            ("instrument", Text),
            ("introTime", Ty::Struct(TIME)),
            ("key", Text),
            ("logComment", Text),
            ("loop", Boolean),
            ("numberOfBeats", Real),
            ("markers", Ty::Seq(&Ty::Struct(MARKER))),
            ("metadataModDate", Date),
            ("outCue", Ty::Struct(TIME)),
            ("projectName", Text),
            ("projectRef", Ty::Struct(PROJECT_LINK)),
            ("pullDown", Text),
            ("relativePeakAudioFilePath", Text),
            ("relativeTimestamp", Ty::Struct(TIME)),
            ("releaseDate", Date),
            ("resampleParams", Ty::Struct(RESAMPLE_STRETCH)),
            ("scaleType", Text),
            ("scene", Text),
            ("shotDate", Date),
            ("shotDay", Text),
            ("shotLocation", Text),
            ("shotName", Text),
            ("shotNumber", Text),
            ("shotSize", Text),
            ("speakerPlacement", Text),
            ("startTimecode", Ty::Struct(TIMECODE)),
            ("stretchMode", Text),
            ("takeNumber", Integer),
            ("tapeName", Text),
            ("tempo", Real),
            ("timeScaleParams", Ty::Struct(TIME_SCALE_STRETCH)),
            ("timeSignature", Text),
            ("trackNumber", Integer),
            ("Tracks", Ty::Bag(&Ty::Struct(TRACK))),
            ("videoAlphaMode", Text),
            ("videoAlphaPremultipleColor", Ty::Struct(COLORANT)),
            ("videoAlphaUnityIsTransparent", Boolean),
            ("videoColorSpace", Text),
            ("videoCompressor", Text),
            ("videoFieldOrder", Text),
            ("videoFrameRate", Text),
            ("videoFrameSize", Ty::Struct(DIMENSIONS)),
            ("videoModDate", Date),
            ("videoPixelDepth", Text),
            ("videoPixelAspectRatio", Rational),
            ("partOfCompilation", Boolean),
            ("lyrics", Text),
            ("discNumber", Text),
        ],
    },
    // Part 2 Table 30.
    Schema {
        uri: "http://ns.adobe.com/pdf/1.3/",
        prefix: "pdf",
        properties: &[("Keywords", Text), ("PDFVersion", Text), ("Producer", Text), ("Trapped", Boolean)],
    },
    // Part 2 Table 33.
    Schema {
        uri: PHOTOSHOP,
        prefix: "photoshop",
        properties: &[
            ("AuthorsPosition", Text),
            ("CaptionWriter", Text),
            ("Category", Text),
            ("City", Text),
            ("ColorMode", Integer),
            ("Country", Text),
            ("Credit", Text),
            ("DateCreated", Date),
            ("DocumentAncestors", Ty::Bag(&Ty::Struct(ANCESTOR))),
            ("Headline", Text),
            ("History", Text),
            ("ICCProfile", Text),
            ("Instructions", Text),
            ("Source", Text),
            ("State", Text),
            ("SupplementalCategories", Ty::Bag(&Text)),
            ("TextLayers", Ty::Seq(&Ty::Struct(LAYER))),
            ("TransmissionReference", Text),
            ("Urgency", Integer),
        ],
    },
    // Part 2 Table 34.
    Schema {
        uri: "http://ns.adobe.com/camera-raw-settings/1.0/",
        prefix: "crs",
        properties: &[
            ("AutoBrightness", Boolean),
            ("AutoContrast", Boolean),
            ("AutoExposure", Boolean),
            ("AutoShadows", Boolean),
            ("BlueHue", Integer),
            ("BlueSaturation", Integer),
            ("Brightness", Integer),
            ("CameraProfile", Text),
            ("ChromaticAberrationB", Integer),
            ("ChromaticAberrationR", Integer),
            ("ColorNoiseReduction", Integer),
            ("Contrast", Integer),
            ("CropTop", Real),
            ("CropLeft", Real),
            ("CropBottom", Real),
            ("CropRight", Real),
            ("CropAngle", Real),
            ("CropWidth", Real),
            ("CropHeight", Real),
            ("CropUnits", Integer),
            ("Exposure", Real),
            ("GreenHue", Integer),
            ("GreenSaturation", Integer),
            ("HasCrop", Boolean),
            ("HasSettings", Boolean),
            ("LuminanceSmoothing", Integer),
            ("RawFileName", Text),
            ("RedHue", Integer),
            ("RedSaturation", Integer),
            ("Saturation", Integer),
            ("Shadows", Integer),
            ("ShadowTint", Integer),
            ("Sharpness", Integer),
            ("Temperature", Integer),
            ("Tint", Integer),
            ("ToneCurve", Ty::Seq(&Text)),
            ("ToneCurveName", Text),
            ("Version", Text),
            ("VignetteAmount", Integer),
            ("VignetteMidpoint", Integer),
            ("WhiteBalance", Text),
        ],
    },
];

/// The properties of predefined schemas whose values don't have their type, in every metadata
/// stream among the indirect objects `objs` (ISO 32000-1 14.3.2), as `prefix:name` in name order.
pub(crate) fn mistyped_in(objs: &[(ObjRef, std::sync::Arc<Object>)]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut budget = DecodeBudget::new(MAX_METADATA_BYTES, 0);
    for (_, o) in objs {
        let Object::Stream(s) = &**o else { continue };
        if s.dict.name(b"Type") != Some(b"Metadata") {
            continue;
        }
        if budget.remaining() == 0 {
            break;
        }
        let Ok(bytes) = budget.decode_within(s, crate::xmp::MAX_XMP_BYTES) else { continue };
        let text = unicode(&bytes);
        if let Some(parsed) = crate::xmp::parse(&text) {
            out.extend(mistyped(&parsed));
        }
    }
    out
}

/// A packet's text in the Unicode encoding its byte order mark gives: UTF-16 big- or
/// little-endian, or UTF-8 (ISO 16684-1 7.3).
fn unicode(bytes: &[u8]) -> String {
    let utf16 = |data: &[u8], big: bool| -> String {
        let (units, _) = data.as_chunks::<2>();
        let units = units.iter().map(|&[a, b]| if big { u16::from_be_bytes([a, b]) } else { u16::from_le_bytes([a, b]) });
        char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    };
    match bytes {
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The properties of predefined schemas in the packet `doc` whose values don't have their type,
/// as `prefix:name` in name order.
pub(crate) fn mistyped(doc: &roxmltree::Document<'_>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let descriptions = doc.descendants().filter(|n| is(n, RDF, "Description") && n.parent_element().is_some_and(|p| is(&p, RDF, "RDF")));
    for d in descriptions {
        for a in d.attributes() {
            if let Some((schema, ty)) = a.namespace().and_then(|ns| property(ns, a.name()))
                && !simple(ty, a.value())
            {
                out.insert(format!("{}:{}", schema.prefix, a.name()));
            }
        }
        for p in d.children().filter(Node::is_element) {
            let name = p.tag_name();
            if let Some((schema, ty)) = name.namespace().and_then(|ns| property(ns, name.name()))
                && !fits(ty, &p, 0)
            {
                out.insert(format!("{}:{}", schema.prefix, name.name()));
            }
        }
    }
    out
}

fn property(ns: &str, name: &str) -> Option<(&'static Schema, Ty)> {
    let schema = SCHEMAS.iter().find(|s| s.uri == ns)?;
    schema.properties.iter().find(|(n, _)| *n == name).map(|(_, t)| (schema, *t))
}

fn is(n: &Node<'_, '_>, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(ns) && n.tag_name().name() == name
}

/// The attributes of `n` that are fields or qualifiers, not RDF or XML syntax.
fn field_attributes<'a, 'i>(n: &Node<'a, 'i>) -> impl Iterator<Item = roxmltree::Attribute<'a, 'i>> {
    n.attributes().filter(|a| !matches!(a.namespace(), Some(RDF) | Some(XML)))
}

/// Whether the value of the property or array item element `e` has type `ty`.
fn fits(ty: Ty, e: &Node<'_, '_>, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let deeper = depth + 1;
    let children: Vec<Node<'_, '_>> = e.children().filter(Node::is_element).collect();
    // A qualified value: `rdf:value` holds the value itself (ISO 16684-1 7.9.2.5).
    let resource = e.attribute((RDF, "parseType")) == Some("Resource");
    // `parseType="Resource"` is the only parse type XMP has (ISO 16684-1 7.9.2.5); an
    // `rdf:resource` element is empty.
    if e.attribute((RDF, "parseType")).is_some_and(|p| p != "Resource") || e.attribute((RDF, "resource")).is_some() && !children.is_empty() {
        return false;
    }
    let description = match children.as_slice() {
        [d] if is(d, RDF, "Description") => Some(*d),
        _ => None,
    };
    // `rdf:value` as an element of a `parseType="Resource"` node or `rdf:Description`, or as an
    // attribute of the description or of an empty property element. Its qualifiers that are
    // predefined properties keep their types too.
    let node = if resource { Some(*e) } else { description };
    if let Some(n) = node {
        // A qualified value has one `rdf:value`.
        match n.children().filter(|c| is(c, RDF, "value")).collect::<Vec<_>>().as_slice() {
            [] => {}
            [v] => return blank(e) && blank(&n) && fits(ty, v, deeper) && predefined_ok(&n, deeper),
            _ => return false,
        }
    }
    let attribute_node = description.or_else(|| (children.is_empty() && !resource).then_some(*e));
    if let Some(n) = attribute_node
        && let Some(v) = n.attribute((RDF, "value"))
    {
        return blank(e) && blank(&n) && simple(ty, v) && predefined_ok(&n, deeper);
    }
    match ty {
        Ty::Text | Ty::Guid | Ty::Integer | Ty::Real | Ty::Boolean | Ty::Date | Ty::Rational => {
            // `rdf:resource` makes the value a URI, which only a text type takes.
            // The element is then empty.
            if e.attribute((RDF, "resource")).is_some() {
                return matches!(ty, Ty::Text) && children.is_empty() && blank(e);
            }
            children.is_empty() && !resource && field_attributes(e).next().is_none() && simple(ty, &text(e))
        }
        Ty::LangAlt => match array(e, &children, "Alt") {
            Some(items) => items.iter().all(|i| i.attribute((XML, "lang")).is_some() && fits(Ty::Text, i, deeper)),
            None => false,
        },
        Ty::Seq(item) | Ty::Bag(item) | Ty::Alt(item) => {
            let kind = match ty {
                Ty::Seq(_) => "Seq",
                Ty::Bag(_) => "Bag",
                _ => "Alt",
            };
            array(e, &children, kind).is_some_and(|items| items.iter().all(|i| fits(*item, i, deeper)))
        }
        Ty::Struct(fields) => {
            let Some(holder) = structure(e, &children, resource, description) else { return false };
            let attrs_ok = field_attributes(&holder).all(|a| match a.namespace().and_then(|ns| field(fields, ns, a.name())) {
                Some(t) => simple(t, a.value()),
                None => true,
            });
            let elements_ok = holder.children().filter(Node::is_element).all(|f| {
                let n = f.tag_name();
                match n.namespace().and_then(|ns| field(fields, ns, n.name())) {
                    Some(t) => fits(t, &f, deeper),
                    None => true,
                }
            });
            attrs_ok && elements_ok
        }
        // A structure of any fields, such as a Pantry item: predefined properties in it still have
        // their types.
        Ty::AnyStruct => structure(e, &children, resource, description).is_some_and(|holder| predefined_ok(&holder, deeper)),
        Ty::OneOf(types) => types.iter().any(|t| fits(*t, e, depth)),
    }
}

/// Whether the fields of `holder`, as attributes and elements, that are predefined properties have
/// their types.
fn predefined_ok(holder: &Node<'_, '_>, depth: usize) -> bool {
    let attrs_ok = field_attributes(holder).all(|a| a.namespace().and_then(|ns| property(ns, a.name())).is_none_or(|(_, t)| simple(t, a.value())));
    let elements_ok = holder.children().filter(Node::is_element).all(|f| {
        let n = f.tag_name();
        n.namespace().and_then(|ns| property(ns, n.name())).is_none_or(|(_, t)| fits(t, &f, depth))
    });
    attrs_ok && elements_ok
}

fn field(fields: &[Field], ns: &str, name: &str) -> Option<Ty> {
    fields.iter().find(|(n, l, _)| *n == ns && *l == name).map(|(_, _, t)| *t)
}

/// The items of `e`'s value when it is an `rdf:<kind>` array.
fn array<'a, 'i>(e: &Node<'a, 'i>, children: &[Node<'a, 'i>], kind: &str) -> Option<Vec<Node<'a, 'i>>> {
    match children {
        // Items are `rdf:li` (ISO 16684-1 7.9.2.4); any other element, or character data around
        // them, breaks the array, and so does `rdf:parseType` or `rdf:resource` on the property,
        // which makes its value something else.
        [a] if is(a, RDF, kind) && field_attributes(e).next().is_none() && blank(e) && blank(a) && !other_value_form(e) => {
            let items: Vec<Node<'a, 'i>> = a.children().filter(Node::is_element).collect();
            items.iter().all(|i| is(i, RDF, "li")).then_some(items)
        }
        _ => None,
    }
}

/// The node holding the fields of `e`'s value when it is a structure.
fn structure<'a, 'i>(e: &Node<'a, 'i>, children: &[Node<'a, 'i>], resource: bool, description: Option<Node<'a, 'i>>) -> Option<Node<'a, 'i>> {
    if resource {
        // A structure's fields are elements; character data has no place in it.
        return text(e).trim().is_empty().then_some(*e);
    }
    if let Some(d) = description {
        return (blank(e) && blank(&d)).then_some(d);
    }
    // Fields written as attributes of the property element, with no content.
    let empty = text(e).trim().is_empty();
    (children.is_empty() && empty && e.attribute((RDF, "resource")).is_none() && field_attributes(e).next().is_some()).then_some(*e)
}

/// Whether `e` says its value is a resource or a literal of another kind, not the element it holds.
fn other_value_form(e: &Node<'_, '_>) -> bool {
    e.attribute((RDF, "parseType")).is_some() || e.attribute((RDF, "resource")).is_some()
}

/// Whether `e` holds no character data but white space.
fn blank(e: &Node<'_, '_>) -> bool {
    text(e).trim().is_empty()
}

/// All the character data directly in `e`, however comments or other nodes split it.
fn text(e: &Node<'_, '_>) -> String {
    e.children().filter(Node::is_text).filter_map(|t| t.text()).collect()
}

/// Whether the simple value `v` has the simple type `ty`.
fn simple(ty: Ty, v: &str) -> bool {
    match ty {
        Ty::Integer => integer(v),
        Ty::Real => real(v),
        Ty::Boolean => matches!(v, "True" | "False"),
        Ty::Date => date(v),
        Ty::Rational => v.split_once('/').is_some_and(|(n, d)| integer(n) && integer(d)),
        Ty::Text | Ty::Guid => true,
        Ty::OneOf(types) => types.iter().any(|t| simple(*t, v)),
        _ => false,
    }
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// ISO 16684-1 8.2.1.3.
fn integer(v: &str) -> bool {
    digits(v.strip_prefix(['+', '-']).unwrap_or(v))
}

/// ISO 16684-1 8.2.1.4: an integer part, a fraction part, or both.
fn real(v: &str) -> bool {
    let v = v.strip_prefix(['+', '-']).unwrap_or(v);
    match v.split_once('.') {
        Some((i, f)) => (i.is_empty() || digits(i)) && digits(f),
        None => digits(v),
    }
}

/// ISO 16684-1 8.2.1.2: YYYY, YYYY-MM, YYYY-MM-DD, or a date with hh:mm, hh:mm:ss or hh:mm:ss.s
/// and an optional time zone (Z, +hh:mm, -hh:mm).
fn date(v: &str) -> bool {
    let num = |s: &str, len: usize, max: u32| s.len() == len && digits(s) && s.parse::<u32>().is_ok_and(|n| n <= max);
    let in_range = |s: &str, min: u32, max: u32| num(s, 2, max) && s.parse::<u32>().is_ok_and(|n| n >= min);
    let (day, time) = match v.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (v, None),
    };
    let parts: Vec<&str> = day.split('-').collect();
    let day_ok = match parts.as_slice() {
        [y] => num(y, 4, 9999),
        [y, m] => num(y, 4, 9999) && in_range(m, 1, 12),
        [y, m, d] => num(y, 4, 9999) && in_range(m, 1, 12) && in_range(d, 1, 31),
        _ => false,
    };
    let Some(time) = time else { return day_ok };
    if parts.len() != 3 || !day_ok {
        return false;
    }
    let (clock, zone) = match time.find(['Z', '+', '-']) {
        Some(i) => (time.get(..i).unwrap_or_default(), time.get(i..).unwrap_or_default()),
        None => (time, ""),
    };
    let zone_ok = match zone {
        "" | "Z" => true,
        z => z.strip_prefix(['+', '-']).and_then(|z| z.split_once(':')).is_some_and(|(h, m)| num(h, 2, 23) && num(m, 2, 59)),
    };
    let clock_ok = match clock.split(':').collect::<Vec<_>>().as_slice() {
        [h, m] => num(h, 2, 23) && num(m, 2, 59),
        [h, m, s] => {
            let (whole, fraction) = s.split_once('.').map_or((*s, None), |(w, f)| (w, Some(f)));
            num(h, 2, 23) && num(m, 2, 59) && num(whole, 2, 59) && fraction.is_none_or(digits)
        }
        _ => false,
    };
    zone_ok && clock_ok
}
