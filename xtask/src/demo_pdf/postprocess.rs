//! Post-processing of Chrome's PDF: everything a browser cannot emit.

use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use super::annots::{Annotator, Kind, Rect, embedded_file};
use super::form::{self, FormInputs};
use super::pdf::{Content, Font, Fonts, Stamp, colors, name, text};

/// Links to this prefix in showcase.html mark where annotations go.
const MARKER_PREFIX: &str = "https://mark.pdfcraft.invalid/";

const TITLE: &str = "PdfCraft Showcase";
const AUTHOR: &str = "PdfCraft";
const SUBJECT: &str = "A specimen document exercising typography, world scripts, vector graphics, \
                       MathML, forms, annotations, layers and attachments.";
const KEYWORDS: &[&str] = &["PDF", "typography", "OpenType", "AcroForm", "annotations", "optional content", "PdfCraft"];
const CREATOR: &str = "pdfcraft xtask demo-pdf";

/// Chrome-printed page indices (0-based) that the post-processor refers to.
const PAGE_CONTENTS: usize = 1;
const PAGE_SETTING_TEXT: usize = 3;
const PAGE_REVIEW: usize = 11;
const CHROME_PAGES: usize = 12;

pub struct Summary {
    pub pages: usize,
    pub annotations: usize,
    pub fields: usize,
    pub outline: String,
}

pub fn enhance(doc: &mut Document, now: Stamp) -> Result<Summary> {
    doc.version = "1.7".into();
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    if pages.len() != CHROME_PAGES {
        bail!(
            "Chrome produced {} pages but the showcase layout expects {CHROME_PAGES}; \
             a page probably overflowed - check dist/demo/build/chrome.pdf",
            pages.len()
        );
    }
    let fonts = Fonts::add_to(doc);
    let mut annotator = Annotator::new(fonts.clone(), now.clone());
    let mut annotation_count = 0;

    // 1. Marker links -> real annotations.
    for (index, &page) in pages.iter().enumerate() {
        let mut annots = page_annots(doc, page)?;
        let mut seen: Vec<String> = Vec::new();
        let mut keep = Vec::with_capacity(annots.len());
        let mut extra = Vec::new();
        for id in annots.drain(..) {
            let Some((uri, r)) = marker_link(doc, id) else {
                keep.push(id);
                continue;
            };
            if seen.contains(&uri) {
                continue; // a marker that wrapped onto two lines: keep only the first box
            }
            seen.push(uri.clone());
            let kind_str = uri[MARKER_PREFIX.len()..].split('/').next().unwrap_or_default();
            let kind = Kind::from_marker(kind_str).with_context(|| format!("unknown annotation marker {uri} on page {}", index + 1))?;
            let struct_parent = doc.get_dictionary(id)?.get(b"StructParent").ok().cloned();
            let built = annotator.build(doc, page, id, kind, r);
            let mut dict = built.dict;
            if let Some(sp) = struct_parent {
                dict.set("StructParent", sp);
            }
            doc.objects.insert(id, Object::Dictionary(dict));
            keep.push(id);
            annotation_count += 1 + built.extra.len();
            extra.extend(built.extra);
        }
        keep.extend(extra);
        if index == PAGE_SETTING_TEXT {
            keep.push(annotator.simple_note(
                doc,
                page,
                572.0,
                704.0,
                "Drop cap, small-caps first line and old-style figures are all plain glyph placement.",
            ));
            annotation_count += 2;
        }
        set_page_annots(doc, page, keep)?;
    }

    // 2. Optional content: a visible draft watermark and hidden print-only notes.
    let ocg_draft = doc.add_object(dictionary! {
        "Type" => "OCG",
        "Name" => text("Draft watermark"),
        "Intent" => "View",
        "Usage" => dictionary! {
            "CreatorInfo" => dictionary! { "Creator" => text(CREATOR), "Subtype" => "Artwork" },
            "Watermark" => dictionary! { "Subtype" => "Watermark" },
        },
    });
    let ocg_notes = doc.add_object(dictionary! {
        "Type" => "OCG",
        "Name" => text("Print-only notes"),
        "Usage" => dictionary! {
            "Print" => dictionary! { "Subtype" => "Watermark", "PrintState" => "ON" },
            "View" => dictionary! { "ViewState" => "OFF" },
        },
    });
    add_watermark(doc, pages[PAGE_REVIEW], ocg_draft, &fonts)?;

    // 3. Appended AcroForm page.
    let pages_root = doc.catalog()?.get(b"Pages")?.as_reference()?;
    let form_page = form::build(
        doc,
        &mut annotator,
        FormInputs { fonts: &fonts, pages_root, contents_page: pages[PAGE_CONTENTS], print_only_ocg: ocg_notes, folio: "10" },
    );
    {
        let root = doc.get_dictionary_mut(pages_root)?;
        let count = root.get(b"Count")?.as_i64()?;
        root.get_mut(b"Kids")?.as_array_mut()?.push(Object::Reference(form_page.page));
        root.set("Count", count + 1);
    }
    annotation_count += 3; // two links and a note (the rest are widgets)
    let field_count = form_page.fields.len();
    let acroform = doc.add_object(form::acroform(&fonts, &form_page.fields));

    let mut all_pages = pages.clone();
    all_pages.push(form_page.page);

    // 4. Outline.
    let outline = extend_outline(doc, &all_pages, form_page.page)?;

    // 5. Embedded file, metadata and catalog entries.
    let csv = include_str!("../../../assets/demo/showcase-data.csv");
    let csv_spec = embedded_file(doc, "showcase-data.csv", "text/csv", csv.as_bytes(), "Sample data behind the charts on page 7", &now);
    doc.get_dictionary_mut(csv_spec)?.set("AFRelationship", "Data");
    add_embedded_file_name(doc, "showcase-data.csv", csv_spec)?;

    let producer = producer(doc);
    let info = doc.add_object(info_dict(&producer, &now));
    doc.trailer.set("Info", info);
    let xmp_stream = Stream::new(dictionary! { "Type" => "Metadata", "Subtype" => "XML" }, xmp(&producer, &now).into_bytes());
    let xmp = doc.add_object(xmp_stream.with_compression(false));

    let catalog = doc.catalog_mut()?;
    catalog.set("Metadata", xmp);
    catalog.set("AcroForm", acroform);
    catalog.set("PageMode", "UseOutlines");
    catalog.set("ViewerPreferences", dictionary! { "DisplayDocTitle" => true });
    catalog.set("AF", vec![Object::Reference(csv_spec)]);
    if !catalog.has(b"Lang") {
        catalog.set("Lang", Object::string_literal("en-US"));
    }
    catalog.set(
        "PageLabels",
        dictionary! {
            "Nums" => vec![
                0.into(), dictionary! { "P" => text("Cover") }.into(),
                1.into(), dictionary! { "S" => "r" }.into(),
                3.into(), dictionary! { "S" => "D" }.into(),
            ],
        },
    );
    catalog.set(
        "OCProperties",
        dictionary! {
            "OCGs" => vec![Object::Reference(ocg_draft), Object::Reference(ocg_notes)],
            "D" => dictionary! {
                "Name" => text("Showcase layers"),
                "Creator" => text(CREATOR),
                "BaseState" => "ON",
                "ON" => vec![Object::Reference(ocg_draft)],
                "OFF" => vec![Object::Reference(ocg_notes)],
                "Order" => vec![Object::Reference(ocg_draft), Object::Reference(ocg_notes)],
                "AS" => vec![
                    dictionary! { "Event" => "Print", "Category" => vec![name("Print")], "OCGs" => vec![Object::Reference(ocg_notes)] }.into(),
                    dictionary! { "Event" => "View", "Category" => vec![name("View")], "OCGs" => vec![Object::Reference(ocg_notes)] }.into(),
                ],
            },
        },
    );

    Ok(Summary { pages: all_pages.len(), annotations: annotation_count, fields: field_count, outline })
}

/// If `id` is one of Chrome's link annotations pointing at a marker URI, return it with its rect.
fn marker_link(doc: &Document, id: ObjectId) -> Option<(String, Rect)> {
    let dict = doc.get_dictionary(id).ok()?;
    if dict.get(b"Subtype").and_then(Object::as_name).ok()? != b"Link" {
        return None;
    }
    let action = dict.get_deref(b"A", doc).and_then(Object::as_dict).ok()?;
    let uri = action.get_deref(b"URI", doc).and_then(Object::as_str).ok()?;
    let uri = String::from_utf8_lossy(uri).into_owned();
    if !uri.starts_with(MARKER_PREFIX) {
        return None;
    }
    let r = Rect::from_obj(dict.get(b"Rect").ok()?)?;
    Some((uri, r))
}

fn page_annots(doc: &Document, page: ObjectId) -> Result<Vec<ObjectId>> {
    let dict = doc.get_dictionary(page)?;
    let Ok(obj) = dict.get_deref(b"Annots", doc) else {
        return Ok(Vec::new());
    };
    Ok(obj.as_array()?.iter().filter_map(|o| o.as_reference().ok()).collect())
}

fn set_page_annots(doc: &mut Document, page: ObjectId, annots: Vec<ObjectId>) -> Result<()> {
    let dict = doc.get_dictionary_mut(page)?;
    if annots.is_empty() {
        dict.remove(b"Annots");
    } else {
        dict.set("Annots", annots.into_iter().map(Object::Reference).collect::<Vec<_>>());
        dict.set("Tabs", "S");
    }
    Ok(())
}

/// Mutable access to a page's own `/Resources` dictionary (direct or indirect).
fn resources_mut(doc: &mut Document, page: ObjectId) -> Result<&mut Dictionary> {
    match doc.get_dictionary(page)?.get(b"Resources").ok().cloned() {
        Some(Object::Reference(id)) => Ok(doc.get_dictionary_mut(id)?),
        Some(Object::Dictionary(_)) => Ok(doc.get_dictionary_mut(page)?.get_mut(b"Resources")?.as_dict_mut()?),
        _ => {
            let page_dict = doc.get_dictionary_mut(page)?;
            page_dict.set("Resources", Dictionary::new());
            Ok(page_dict.get_mut(b"Resources")?.as_dict_mut()?)
        }
    }
}

/// Insert `key => value` into the `category` sub-dictionary of a page's resources.
fn add_resource(doc: &mut Document, page: ObjectId, category: &str, key: &str, value: Object) -> Result<()> {
    let indirect = match resources_mut(doc, page)?.get(category.as_bytes()) {
        Ok(Object::Reference(id)) => Some(*id),
        _ => None,
    };
    let sub = match indirect {
        Some(id) => doc.get_dictionary_mut(id)?,
        None => {
            let res = resources_mut(doc, page)?;
            if !res.has(category.as_bytes()) {
                res.set(category, Dictionary::new());
            }
            res.get_mut(category.as_bytes())?.as_dict_mut()?
        }
    };
    sub.set(key, value);
    Ok(())
}

/// Wrap the page's existing content in q/Q and append a diagonal "DRAFT" on layer `ocg`.
fn add_watermark(doc: &mut Document, page: ObjectId, ocg: ObjectId, fonts: &Fonts) -> Result<()> {
    add_resource(doc, page, "Properties", "PCDraft", Object::Reference(ocg))?;
    add_resource(doc, page, "Font", "PCDraftFont", Object::Reference(fonts.id(Font::HelvBold)))?;
    add_resource(doc, page, "ExtGState", "PCDraftGS", dictionary! { "Type" => "ExtGState", "ca" => 0.09, "CA" => 0.09 }.into())?;

    let size = 150.0f32;
    let word = "DRAFT";
    let tw = Font::HelvBold.width(word, size);
    let cap = 0.72 * size;
    let (sin, cos) = 52f32.to_radians().sin_cos();
    let (cx, cy) = (306.0, 396.0);
    let x = cx - (cos * tw / 2.0 - sin * cap / 2.0);
    let y = cy - (sin * tw / 2.0 + cos * cap / 2.0);
    let mut c = Content::new();
    c.raw("Q").save();
    c.raw("/OC /PCDraft BDC /Artifact << /Type /Pagination /Subtype /Watermark >> BDC");
    c.gs("PCDraftGS").fill_color(colors::ACCENT);
    c.raw(&format!("BT /PCDraftFont {size} Tf {cos:.4} {sin:.4} {:.4} {cos:.4} {x:.2} {y:.2} Tm (DRAFT) Tj ET", -sin));
    c.raw("EMC EMC").restore();

    let open = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let close = doc.add_object(Stream::new(Dictionary::new(), c.into_bytes()));
    let mut contents = vec![Object::Reference(open)];
    let page_dict = doc.get_dictionary(page)?;
    match page_dict.get(b"Contents")? {
        Object::Reference(id) => match doc.get_object(*id)? {
            Object::Array(items) => contents.extend(items.iter().cloned()),
            _ => contents.push(Object::Reference(*id)),
        },
        Object::Array(items) => contents.extend(items.iter().cloned()),
        other => bail!("unexpected /Contents {other:?}"),
    }
    contents.push(Object::Reference(close));
    doc.get_dictionary_mut(page)?.set("Contents", contents);
    Ok(())
}

/// Keep Chrome's heading-based outline (tidying its titles) and add the form
/// page next to the other chapters; if Chrome produced none, build one.
fn extend_outline(doc: &mut Document, pages: &[ObjectId], form_page: ObjectId) -> Result<String> {
    let dest = |page: ObjectId, top: i64| -> Object { vec![Object::Reference(page), name("XYZ"), 0.into(), top.into(), Object::Null].into() };
    let existing = doc
        .catalog()?
        .get(b"Outlines")
        .and_then(Object::as_reference)
        .ok()
        .filter(|id| doc.get_dictionary(*id).map(|d| d.has(b"First")).unwrap_or(false));

    let (chapter_parent, description) = match existing {
        Some(root) => {
            let mut items = Vec::new();
            collect_outline(doc, root, &mut items);
            let mut parent = root;
            for id in items {
                let dict = doc.get_dictionary_mut(id)?;
                let title = decode_text(dict.get(b"Title").and_then(Object::as_str).unwrap_or_default());
                if title == "Review & Markup" {
                    parent = dict.get(b"Parent").and_then(Object::as_reference).unwrap_or(root);
                }
                let tidy = tidy_title(&title);
                if tidy != title {
                    dict.set("Title", text(&tidy));
                }
            }
            (parent, "Chrome headings (tidied) + form page")
        }
        None => {
            let root = doc.add_object(dictionary! { "Type" => "Outlines", "Count" => 0 });
            let chapters = [
                "Cover",
                "Contents",
                "Foreword",
                "Setting Text",
                "OpenType Features",
                "Expressive Type",
                "Scripts of the World",
                "Mathematics",
                "Vector Graphics",
                "Data & Tables",
                "Code & Images",
                "Review & Markup",
            ];
            for (index, title) in chapters.into_iter().enumerate() {
                append_outline_item(doc, root, title, dest(pages[index], 792), None)?;
            }
            doc.catalog_mut()?.set("Outlines", root);
            (root, "built by xtask (Chrome emitted none)")
        }
    };

    let form_item = append_outline_item(doc, chapter_parent, "Interactive Form", dest(form_page, 792), Some(colors::ACCENT.array()))?;
    for (title, top) in [("Contact", 620), ("Preferences", 426), ("Actions & signature", 240)] {
        append_outline_item(doc, form_item, title, dest(form_page, top), None)?;
    }
    let root = doc.catalog()?.get(b"Outlines")?.as_reference()?;
    let total = count_outline(doc, root);
    Ok(format!("{description}, {total} entries"))
}

/// Every outline item below `node`, depth first.
fn collect_outline(doc: &Document, node: ObjectId, out: &mut Vec<ObjectId>) {
    let next = |id: ObjectId, key: &[u8]| doc.get_dictionary(id).ok().and_then(|d| d.get(key).and_then(Object::as_reference).ok());
    let mut cur = next(node, b"First");
    while let Some(id) = cur {
        out.push(id);
        collect_outline(doc, id, out);
        cur = next(id, b"Next");
    }
}

/// Decode a PDF text string (UTF-16BE with BOM, else treated as Latin-1).
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Chrome copies CSS `text-transform: uppercase` into bookmark titles; turn
/// all-caps headings back into sentence case (keeping acronyms).
fn tidy_title(title: &str) -> String {
    const ACRONYMS: &[&str] = &["PDF"];
    // Chrome drops the <br> between the two lines of the cover title.
    if title == "PdfCraftShowcase" {
        return TITLE.to_string();
    }
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().any(char::is_lowercase) || !title.chars().any(char::is_uppercase) {
        return title;
    }
    title
        .split(' ')
        .enumerate()
        .map(|(i, word)| {
            let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
            if ACRONYMS.contains(&bare) {
                return word.to_string();
            }
            let lower = word.to_lowercase();
            if i == 0 {
                let mut chars = lower.chars();
                chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
            } else {
                lower
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Append a child item to outline node `parent`, updating First/Last and the
/// /Count of every open ancestor.
fn append_outline_item(doc: &mut Document, parent: ObjectId, title: &str, dest: Object, color: Option<Object>) -> Result<ObjectId> {
    let last = doc.get_dictionary(parent)?.get(b"Last").and_then(Object::as_reference).ok();
    let mut item = dictionary! { "Title" => text(title), "Parent" => parent, "Dest" => dest };
    if let Some(color) = color {
        item.set("C", color);
        item.set("F", 2); // bold
    }
    if let Some(last) = last {
        item.set("Prev", last);
    }
    let id = doc.add_object(item);
    if let Some(last) = last {
        doc.get_dictionary_mut(last)?.set("Next", id);
    }
    let p = doc.get_dictionary_mut(parent)?;
    if last.is_none() {
        p.set("First", id);
    }
    p.set("Last", id);

    // Open nodes count visible descendants (positive); a closed node's negative
    // count grows in magnitude and hides the change from its ancestors.
    let mut node = Some(parent);
    while let Some(n) = node {
        let d = doc.get_dictionary_mut(n)?;
        let count = d.get(b"Count").and_then(Object::as_i64).unwrap_or(0);
        if count < 0 {
            d.set("Count", count - 1);
            break;
        }
        d.set("Count", count + 1);
        node = d.get(b"Parent").and_then(Object::as_reference).ok();
    }
    Ok(id)
}

fn count_outline(doc: &Document, node: ObjectId) -> usize {
    let mut n = 0;
    let mut cur = doc.get_dictionary(node).ok().and_then(|d| d.get(b"First").and_then(Object::as_reference).ok());
    while let Some(id) = cur {
        n += 1 + count_outline(doc, id);
        cur = doc.get_dictionary(id).ok().and_then(|d| d.get(b"Next").and_then(Object::as_reference).ok());
    }
    n
}

fn add_embedded_file_name(doc: &mut Document, file_name: &str, spec: ObjectId) -> Result<()> {
    let names_ref = doc.catalog()?.get(b"Names").and_then(Object::as_reference).ok();
    let names: &mut Dictionary = match names_ref {
        Some(id) => doc.get_dictionary_mut(id)?,
        None => {
            let catalog = doc.catalog_mut()?;
            if !catalog.has(b"Names") {
                catalog.set("Names", Dictionary::new());
            }
            catalog.get_mut(b"Names")?.as_dict_mut()?
        }
    };
    names.set("EmbeddedFiles", dictionary! { "Names" => vec![Object::string_literal(file_name), Object::Reference(spec)] });
    Ok(())
}

/// Chrome's producer string with this tool appended.
fn producer(doc: &Document) -> String {
    let chrome = doc
        .trailer
        .get_deref(b"Info", doc)
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Producer"))
        .and_then(Object::as_str)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .unwrap_or_else(|_| "Chrome".into());
    format!("{chrome}; lopdf 0.45 via pdfcraft xtask")
}

fn info_dict(producer: &str, now: &Stamp) -> Dictionary {
    dictionary! {
        "Title" => text(TITLE),
        "Author" => text(AUTHOR),
        "Subject" => text(SUBJECT),
        "Keywords" => text(&KEYWORDS.join(", ")),
        "Creator" => text(CREATOR),
        "Producer" => text(producer),
        "CreationDate" => Object::string_literal(now.pdf.clone()),
        "ModDate" => Object::string_literal(now.pdf.clone()),
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn xmp(producer: &str, now: &Stamp) -> String {
    // A stable-looking document id derived from the timestamp (FNV-1a).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in now.iso.bytes().chain(TITLE.bytes()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let uuid = format!("{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}", h >> 32, (h >> 16) & 0xffff, h & 0xfff, (h >> 20) & 0xfff, h & 0xffff_ffff_ffff);
    let keywords: String = KEYWORDS.iter().map(|k| format!("<rdf:li>{}</rdf:li>", xml_escape(k))).collect();
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n\
    xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\">\n\
   <dc:format>application/pdf</dc:format>\n\
   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{title}</rdf:li></rdf:Alt></dc:title>\n\
   <dc:creator><rdf:Seq><rdf:li>{author}</rdf:li></rdf:Seq></dc:creator>\n\
   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{subject}</rdf:li></rdf:Alt></dc:description>\n\
   <dc:subject><rdf:Bag>{keywords}</rdf:Bag></dc:subject>\n\
   <xmp:CreatorTool>{creator}</xmp:CreatorTool>\n\
   <xmp:CreateDate>{date}</xmp:CreateDate>\n\
   <xmp:ModifyDate>{date}</xmp:ModifyDate>\n\
   <xmp:MetadataDate>{date}</xmp:MetadataDate>\n\
   <pdf:Keywords>{kw_flat}</pdf:Keywords>\n\
   <pdf:Producer>{producer}</pdf:Producer>\n\
   <xmpMM:DocumentID>uuid:{uuid}</xmpMM:DocumentID>\n\
   <xmpMM:InstanceID>uuid:{uuid}</xmpMM:InstanceID>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
{padding}\n\
<?xpacket end=\"w\"?>",
        title = xml_escape(TITLE),
        author = xml_escape(AUTHOR),
        subject = xml_escape(SUBJECT),
        creator = xml_escape(CREATOR),
        date = now.iso,
        kw_flat = xml_escape(&KEYWORDS.join(", ")),
        producer = xml_escape(producer),
        padding = " ".repeat(1024),
    )
}
