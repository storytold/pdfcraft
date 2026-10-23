//! Every failure condition of the Matterhorn Protocol 1.1, in our own words.
//!
//! The Matterhorn Protocol is the PDF Association's public list of the ways a file fails PDF/UA-1
//! (ISO 14289-1): 31 checkpoints, each a handful of failure conditions with the section of the
//! standard it breaks (`UA1:7.18.3-1` is ISO 14289-1 clause 7.18.3, its first requirement) and
//! whether software can decide it or a person must. Ids and sections follow the PDF Association's
//! Tagged PDF School listing of version 1.1, where the Suspects condition is 08-003 (some older
//! lists number it 01-007). Two conditions are left out because they ask for no test of their own:
//! 23-001 (digital signatures: the form-field conditions apply) and 27-001 (navigation).
//!
//! Three entries are not Matterhorn conditions: `tagged-pdf`, the Tagged PDF requirement of ISO
//! 14289-1 clause 7.1 the protocol takes for granted, and the identification and version of PDF/UA-2
//! (ISO 14289-2), which the protocol does not cover.

use super::{Applies, Condition, How};

const fn machine(
    id: &'static str,
    checkpoint: &'static str,
    section: &'static str,
    checked: bool,
    applies: Applies,
    text: &'static str,
) -> Condition {
    Condition { id, matterhorn: true, checkpoint, section, how: How::Machine, checked, applies, text }
}

const fn human(id: &'static str, checkpoint: &'static str, section: &'static str, text: &'static str) -> Condition {
    Condition { id, matterhorn: true, checkpoint, section, how: How::Human, checked: false, applies: Applies::Both, text }
}

/// A machine condition PdfCraft does not check yet: always reported as not checked.
const fn unchecked(id: &'static str, checkpoint: &'static str, section: &'static str, text: &'static str) -> Condition {
    machine(id, checkpoint, section, false, Applies::Ua1, text)
}

const REAL_CONTENT: &str = "Real content tagged";
const ROLE_MAPPING: &str = "Role mapping";
const FLICKERING: &str = "Flickering";
const COLOR: &str = "Color and contrast";
const SOUND: &str = "Sound";
const METADATA: &str = "Metadata";
const DICTIONARY: &str = "Dictionary";
const OCR: &str = "OCR validation";
const TAGS: &str = "Appropriate tags";
const CHARACTERS: &str = "Character mappings";
const LANGUAGE: &str = "Declared natural language";
const STRETCHABLE: &str = "Stretchable characters";
const GRAPHICS: &str = "Graphics";
const HEADINGS: &str = "Headings";
const TABLES: &str = "Tables";
const LISTS: &str = "Lists";
const MATH: &str = "Mathematical expressions";
const HEADERS_FOOTERS: &str = "Page headers and footers";
const NOTES: &str = "Notes and references";
const OPTIONAL_CONTENT: &str = "Optional content";
const EMBEDDED_FILES: &str = "Embedded files";
const ARTICLES: &str = "Article threads";
const FORMS: &str = "Non-interactive forms";
const XFA: &str = "XFA";
const SECURITY: &str = "Security";
const ANNOTATIONS: &str = "Annotations";
const ACTIONS: &str = "Actions";
const XOBJECTS: &str = "XObjects";
const FONTS: &str = "Fonts";

use Applies::{Both, OnlyUa1, OnlyUa2, Ua1};

/// The conditions, in the protocol's order, then the ones it leaves out.
pub const CONDITIONS: &[Condition] = &[
    human("01-001", REAL_CONTENT, "UA1:7.1-1", "Content that is an artifact is tagged as real content"),
    human("01-002", REAL_CONTENT, "UA1:7.1-1", "Real content is marked as an artifact"),
    machine("01-003", REAL_CONTENT, "UA1:7.1-1", true, Both, "Content marked as an artifact sits inside tagged content"),
    machine("01-004", REAL_CONTENT, "UA1:7.1-1", true, Both, "Tagged content sits inside content marked as an artifact"),
    machine("01-005", REAL_CONTENT, "UA1:7.1-2", true, Both, "Content is neither tagged nor marked as an artifact"),
    human("01-006", REAL_CONTENT, "UA1:7.1-2", "A structure element's type or attributes don't fit its content"),
    machine("02-001", ROLE_MAPPING, "UA1:7.1-3", true, Ua1, "A non-standard structure type doesn't map to a standard one"),
    human("02-002", ROLE_MAPPING, "UA1:7.1-3", "A non-standard structure type maps to a standard type that doesn't fit it"),
    machine("02-003", ROLE_MAPPING, "UA1:7.1-3", true, Ua1, "The role map is circular"),
    machine("02-004", ROLE_MAPPING, "UA1:7.1-4", true, Ua1, "A standard structure type is remapped"),
    human("03-001", FLICKERING, "UA1:7.1-5", "An action makes the screen flicker"),
    human("03-002", FLICKERING, "UA1:7.1-5", "Multimedia makes the screen flicker"),
    human("03-003", FLICKERING, "UA1:7.1-5", "JavaScript makes the screen flicker"),
    human("04-001", COLOR, "UA1:7.1-6", "Colour, contrast, format or layout carries meaning the tags don't"),
    human("05-001", SOUND, "UA1:7.1-7", "A media annotation's sound isn't available in another form"),
    human("05-002", SOUND, "UA1:7.1-7", "A sound annotation isn't available in another form"),
    human("05-003", SOUND, "UA1:7.1-7", "JavaScript beeps without another way of telling the reader"),
    machine("06-001", METADATA, "UA1:7.1-8", true, Both, "The catalog has no XMP metadata stream"),
    machine("06-002", METADATA, "UA1:5", true, OnlyUa1, "The metadata doesn't identify the file as PDF/UA-1"),
    machine("06-003", METADATA, "UA1:7.1-8", true, Both, "The metadata has no title (dc:title)"),
    human("06-004", METADATA, "UA1:7.1-8", "The title (dc:title) doesn't clearly identify the document"),
    machine("07-001", DICTIONARY, "UA1:7.1-9", true, Both, "The viewer preferences have no DisplayDocTitle entry"),
    machine("07-002", DICTIONARY, "UA1:7.1-9", true, Both, "DisplayDocTitle is false"),
    human("08-001", OCR, "UA1:7.1-10", "Text from OCR has significant errors"),
    human("08-002", OCR, "UA1:7.1-10", "Text from OCR isn't tagged"),
    machine("08-003", OCR, "UA1:7.1-11", true, Ua1, "MarkInfo Suspects is true"),
    human("09-001", TAGS, "UA1:7.2-1", "The tags aren't in a logical reading order"),
    human("09-002", TAGS, "UA1:7.2-1", "Structure elements are nested in a way that doesn't fit their meaning"),
    human("09-003", TAGS, "UA1:7.2-1", "A structure type, after role mapping, doesn't fit its content"),
    machine("09-004", TAGS, "UA1:7.2-1", true, Ua1, "Table elements aren't nested as ISO 32000-1 Table 337 requires"),
    machine("09-005", TAGS, "UA1:7.2-1", true, Ua1, "List elements aren't nested as ISO 32000-1 Table 336 requires"),
    machine("09-006", TAGS, "UA1:7.2-1", true, Ua1, "Table of contents elements aren't nested as ISO 32000-1 Table 333 requires"),
    machine("09-007", TAGS, "UA1:7.2-1", true, Ua1, "Ruby elements aren't nested as ISO 32000-1 Table 338 requires"),
    machine("09-008", TAGS, "UA1:7.2-1", true, Ua1, "Warichu elements aren't nested as ISO 32000-1 Table 338 requires"),
    machine("10-001", CHARACTERS, "UA1:7.2-2", true, Both, "A character code can't be mapped to Unicode"),
    machine("11-001", LANGUAGE, "UA1:7.2-3", true, Both, "The language of text in the page content can't be determined"),
    machine("11-002", LANGUAGE, "UA1:7.2-3", true, Both, "The language of alternate text, actual text or an expansion can't be determined"),
    machine("11-003", LANGUAGE, "UA1:7.2-3", true, Both, "The language of the bookmarks can't be determined"),
    machine("11-004", LANGUAGE, "UA1:7.2-3", true, Both, "The language of an annotation's description (Contents) can't be determined"),
    machine("11-005", LANGUAGE, "UA1:7.2-3", true, Both, "The language of a form field's description (TU) can't be determined"),
    machine("11-006", LANGUAGE, "UA1:7.2-3", true, Both, "The language of the document metadata can't be determined"),
    human("11-007", LANGUAGE, "UA1:7.2-3", "The declared language is wrong for the text"),
    human("12-001", STRETCHABLE, "UA1:7.2-4", "Stretched characters aren't represented appropriately"),
    human("13-001", GRAPHICS, "UA1:7.3-1", "A graphic that isn't an artifact isn't tagged as a Figure"),
    human("13-002", GRAPHICS, "UA1:7.3-1", "A link over a meaningful picture has no alternate text describing both"),
    human("13-003", GRAPHICS, "UA1:7.3-2", "A caption isn't tagged as Caption"),
    machine("13-004", GRAPHICS, "UA1:7.3-3", true, Both, "A Figure has no alternate or actual text"),
    human("13-005", GRAPHICS, "UA1:7.3-4", "A Figure uses actual text where alternate text would fit better"),
    human("13-006", GRAPHICS, "UA1:7.3-5", "A graphic that only means something in a group is tagged on its own"),
    human("13-007", GRAPHICS, "UA1:7.3-6", "A more accessible representation isn't used"),
    human("14-001", HEADINGS, "UA1:7.4-1", "Headings aren't tagged as headings"),
    machine("14-002", HEADINGS, "UA1:7.4.2-1", true, Ua1, "Numbered headings are used, but the first one isn't H1"),
    machine("14-003", HEADINGS, "UA1:7.4-1", true, Ua1, "Numbered heading levels are skipped going down"),
    human("14-004", HEADINGS, "UA1:7.4.3-1", "Numbered heading tags don't use Arabic numerals"),
    human("14-005", HEADINGS, "UA1:7.4.3-1", "A heading deeper than level 6 doesn't use H7 or deeper"),
    machine("14-006", HEADINGS, "UA1:7.4.4-1", true, Ua1, "An element holds more than one H heading"),
    machine("14-007", HEADINGS, "UA1:7.4.4-3", true, Ua1, "The document uses both H and numbered headings"),
    human("15-001", TABLES, "UA1:7.5-1", "A row header cell isn't tagged TH"),
    human("15-002", TABLES, "UA1:7.5-1", "A column header cell isn't tagged TH"),
    machine("15-003", TABLES, "UA1:7.5-2", true, Ua1, "A TH cell has no Scope, in a table that doesn't use Headers and IDs"),
    human("15-004", TABLES, "UA1:7.5-3", "Content that isn't tabular is tagged as a table"),
    human("15-005", TABLES, "UA1:7.5-2", "A cell's headers can't be determined unambiguously"),
    human("16-001", LISTS, "UA1:7.6-1", "An ordered list has no ListNumbering attribute"),
    human("16-002", LISTS, "UA1:7.6-1", "An ordered list's ListNumbering isn't one of the allowed values"),
    human("16-003", LISTS, "UA1:7.6-2", "A list isn't tagged as a list"),
    human("17-001", MATH, "UA1:7.7-1", "A mathematical expression isn't tagged Formula"),
    machine("17-002", MATH, "UA1:7.7-1", true, Ua1, "A Formula has no alternate text"),
    unchecked("17-003", MATH, "UA1:7.7-2", "A formula's characters don't meet the Unicode mapping requirements"),
    human("18-001", HEADERS_FOOTERS, "UA1:7.8-1", "Headers and footers aren't marked as pagination artifacts"),
    human("18-002", HEADERS_FOOTERS, "UA1:7.8-1", "Header and footer artifacts don't have the Header or Footer subtype"),
    human("19-001", NOTES, "UA1:7.9-1", "Footnotes, endnotes or their labels aren't tagged Note"),
    human("19-002", NOTES, "UA1:7.9-1", "References aren't tagged Reference"),
    machine("19-003", NOTES, "UA1:7.9-2", true, Ua1, "A Note has no ID"),
    machine("19-004", NOTES, "UA1:7.9-2", true, Ua1, "A Note's ID isn't unique"),
    machine("20-001", OPTIONAL_CONTENT, "UA1:7.10-1", true, Ua1, "An optional content configuration in Configs has no Name"),
    machine("20-002", OPTIONAL_CONTENT, "UA1:7.10-1", true, Ua1, "The default optional content configuration (D) has no Name"),
    machine("20-003", OPTIONAL_CONTENT, "UA1:7.10-2", true, Ua1, "An optional content configuration has an AS entry"),
    machine("21-001", EMBEDDED_FILES, "UA1:7.11-1", true, Ua1, "An embedded file's file specification lacks F or UF"),
    human("22-001", ARTICLES, "UA1:7.12-1", "Article threads don't follow the logical reading order"),
    human("24-001", FORMS, "UA1:7.14-1", "A non-interactive form isn't tagged with the PrintField attribute"),
    machine("25-001", XFA, "UA1:7.15-1", true, Ua1, "The XFA form requires dynamic rendering"),
    machine("26-001", SECURITY, "UA1:7.16-1", true, Ua1, "The file is encrypted and its encryption dictionary has no P entry"),
    machine("26-002", SECURITY, "UA1:7.16-1", true, Ua1, "The permissions (P) don't let assistive technology read the text"),
    human("28-001", ANNOTATIONS, "UA1:7.18.1-2", "An annotation isn't in the reading order"),
    machine("28-002", ANNOTATIONS, "UA1:7.18.1-2", true, Ua1, "An annotation other than a widget, link or popup isn't in an Annot tag"),
    human("28-003", ANNOTATIONS, "UA1:7.18.1-3", "A formatting annotation isn't tagged for what it means"),
    machine("28-004", ANNOTATIONS, "UA1:7.18.1-4", true, Ua1, "An annotation has no Contents and its tag has no alternate text"),
    machine("28-005", ANNOTATIONS, "UA1:7.18.1-4", true, Ua1, "A form field has no TU and its tag has no alternate text"),
    unchecked("28-006", ANNOTATIONS, "UA1:7.18.2-1", "An annotation of a subtype ISO 32000-1 doesn't define doesn't meet clause 7.18.1"),
    machine("28-007", ANNOTATIONS, "UA1:7.18.2-2", true, Ua1, "There is a trap network annotation (TrapNet)"),
    machine("28-008", ANNOTATIONS, "UA1:7.18.3-1", true, Ua1, "A page with annotations has no Tabs entry"),
    machine("28-009", ANNOTATIONS, "UA1:7.18.3-1", true, Ua1, "A page with annotations has Tabs other than S"),
    machine("28-010", ANNOTATIONS, "UA1:7.18.4-1", true, Both, "A widget annotation isn't in a Form tag"),
    machine("28-011", ANNOTATIONS, "UA1:7.18.5-1", true, Both, "A link annotation isn't in a Link tag"),
    machine("28-012", ANNOTATIONS, "UA1:7.18.5-2", true, Both, "A link annotation has no Contents"),
    human("28-013", ANNOTATIONS, "UA1:7.18.5-3", "An image-map link (IsMap) has no equivalent provided another way"),
    unchecked("28-014", ANNOTATIONS, "UA1:7.18.6.2-1", "A media clip has no content type (CT)"),
    unchecked("28-015", ANNOTATIONS, "UA1:7.18.6.2-1", "A media clip has no alternate description (Alt)"),
    machine("28-016", ANNOTATIONS, "UA1:7.18.7-1", true, Ua1, "A file attachment annotation doesn't meet the embedded-file requirements"),
    machine("28-017", ANNOTATIONS, "UA1:7.18.8-1", true, Ua1, "A printer's mark annotation is in the structure tree"),
    unchecked("28-018", ANNOTATIONS, "UA1:7.18.8-2", "A printer's mark annotation's appearance isn't marked as an artifact"),
    human("29-001", ACTIONS, "UA1:7.19-1", "A script needs keystrokes timed in a particular way"),
    machine("30-001", XOBJECTS, "UA1:7.20-1", true, Ua1, "A reference XObject is present"),
    machine("30-002", XOBJECTS, "UA1:7.20-2", true, Ua1, "A form XObject with MCIDs is drawn more than once"),
    unchecked("31-001", FONTS, "UA1:7.21.3-1", "A Type 0 font's CIDSystemInfo Registry values differ"),
    unchecked("31-002", FONTS, "UA1:7.21.3.1-1", "A Type 0 font's CIDSystemInfo Ordering values differ"),
    unchecked("31-003", FONTS, "UA1:7.21.3.1-1", "A CIDFont's Supplement is lower than its CMap's"),
    unchecked("31-004", FONTS, "UA1:7.21.3.2-1", "A Type 2 CIDFont's CIDToGIDMap is neither a stream nor Identity"),
    unchecked("31-005", FONTS, "UA1:7.21.3.2-1", "A Type 2 CIDFont has no CIDToGIDMap"),
    unchecked("31-006", FONTS, "UA1:7.21.3.3-1", "A CMap is neither predefined nor embedded"),
    unchecked("31-007", FONTS, "UA1:7.21.3.3-1", "A CMap's WMode differs from its embedded stream's"),
    unchecked("31-008", FONTS, "UA1:7.21.3.3-2", "A CMap uses a CMap that isn't predefined"),
    machine("31-009", FONTS, "UA1:7.21.4.1-1", true, Both, "A font used to draw text isn't embedded"),
    human("31-010", FONTS, "UA1:7.21.4.1-2", "An embedded font's licence doesn't allow universal rendering"),
    unchecked("31-011", FONTS, "UA1:7.21.4.1-3", "An embedded font lacks glyphs the text draws"),
    unchecked("31-012", FONTS, "UA1:7.21.4.2-1", "A Type 1 font's CharSet leaves out glyphs the font has"),
    unchecked("31-013", FONTS, "UA1:7.21.4.2-2", "A Type 1 font's CharSet lists glyphs the font lacks"),
    unchecked("31-014", FONTS, "UA1:7.21.4.2-3", "A CIDFont's CIDSet leaves out glyphs the font has"),
    unchecked("31-015", FONTS, "UA1:7.21.4.2-4", "A CIDFont's CIDSet lists glyphs the font lacks"),
    unchecked("31-016", FONTS, "UA1:7.21.5-1", "Glyph widths in the font dictionary and the font program differ"),
    unchecked("31-017", FONTS, "UA1:7.21.6-1", "A non-symbolic TrueType font has no non-symbolic cmap"),
    unchecked("31-018", FONTS, "UA1:7.21.6-2", "A non-symbolic TrueType glyph can't be found through a non-symbolic cmap"),
    unchecked("31-019", FONTS, "UA1:7.21.6-3", "A non-symbolic TrueType font has no Encoding"),
    unchecked("31-020", FONTS, "UA1:7.21.6-4", "A non-symbolic TrueType font's Encoding dictionary has no BaseEncoding"),
    unchecked("31-021", FONTS, "UA1:7.21.6-5", "A non-symbolic TrueType font's encoding is neither MacRoman nor WinAnsi"),
    unchecked("31-022", FONTS, "UA1:7.21.6-6", "A Differences array names glyphs that aren't in the Adobe Glyph List"),
    unchecked("31-023", FONTS, "UA1:7.21.6-7", "A font with Differences has no (3,1) Unicode cmap"),
    unchecked("31-024", FONTS, "UA1:7.21.6-8", "A symbolic TrueType font has an Encoding entry"),
    unchecked("31-025", FONTS, "UA1:7.21.6-9", "A symbolic TrueType font has no cmap"),
    unchecked("31-026", FONTS, "UA1:7.21.6-10", "A symbolic TrueType font with several cmaps has no (3,0) cmap"),
    unchecked("31-027", FONTS, "UA1:7.21.7-1", "A font has no ToUnicode and none of the exceptions applies"),
    unchecked("31-028", FONTS, "UA1:7.21.7-2", "A ToUnicode map maps a code to Unicode 0"),
    unchecked("31-029", FONTS, "UA1:7.21.7-3", "A ToUnicode map maps a code to U+FEFF or U+FFFE"),
    unchecked("31-030", FONTS, "UA1:7.21.8-1", "Text draws the .notdef glyph"),
    Condition {
        id: "tagged-pdf",
        matterhorn: false,
        checkpoint: "Tagged PDF",
        section: "UA1:7.1",
        how: How::Machine,
        checked: true,
        applies: Both,
        text: "The document isn't Tagged PDF: it needs a structure tree and MarkInfo Marked true",
    },
    Condition {
        id: "ua2-identification",
        matterhorn: false,
        checkpoint: METADATA,
        section: "UA2",
        how: How::Machine,
        checked: true,
        applies: OnlyUa2,
        text: "The metadata doesn't identify the file as PDF/UA-2 (part and revision)",
    },
    Condition {
        id: "ua2-pdf-version",
        matterhorn: false,
        checkpoint: "PDF version",
        section: "UA2",
        how: How::Machine,
        checked: true,
        applies: OnlyUa2,
        text: "The file isn't PDF 2.0",
    },
];
