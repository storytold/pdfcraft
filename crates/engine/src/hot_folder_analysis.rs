//! Read-only, versioned analysis of existing PDF contents. Never runs OCR, scripts,
//! rendering or saving; image rectangles are inferred from drawing instructions.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::Arc;

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use serde::Serialize;

const MAX_INPUT: usize = 512 * 1024 * 1024;
const MAX_REPORT: usize = 16 * 1024 * 1024;
const MAX_TEXT: usize = 8 * 1024 * 1024;
const MAX_CONTENT: usize = 8 * 1024 * 1024;
const MAX_STREAM: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 10_000;
const MAX_ITEMS: usize = 50_000;
const MAX_OPERATORS: usize = 100_000;

#[derive(Serialize)]
struct Report {
    format: &'static str,
    version: u32,
    ocr_performed: bool,
    coordinate_space: &'static str,
    document: Metadata,
    pages: Vec<PageReport>,
    limitations: Vec<&'static str>,
}
#[derive(Serialize)]
struct Metadata {
    pdf_version: String,
    bytes: usize,
    page_count: usize,
    encrypted: bool,
    info: BTreeMap<String, String>,
    has_xmp_metadata: bool,
    repair_log: Vec<String>,
}
#[derive(Serialize)]
struct PageReport {
    page_index: usize,
    page_number: usize,
    media_box: [f64; 4],
    crop_box: [f64; 4],
    rotation: i64,
    user_unit: f64,
    display_size_points: [f64; 2],
    content: ContentKind,
    has_text_operators: bool,
    has_vector_paint: bool,
    text_blocks: Vec<TextBlock>,
    images: Vec<Image>,
    warnings: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ContentKind {
    ExistingText,
    ImageWithoutText,
    NoExtractableText,
    EmptyContent,
}
#[derive(Serialize)]
struct TextBlock {
    text: String,
    rect: [f64; 4],
    base_font: String,
    font_size: f64,
    bold: bool,
    italic: bool,
}
#[derive(Serialize)]
struct Image {
    rect: [f64; 4],
    matrix: [f64; 6],
    pixel_width: u32,
    pixel_height: u32,
    resource: Option<String>,
    inline: bool,
}

/// Analyze zero-based pages, preserving their requested order; an empty selection
/// means every page. Duplicate or nonexistent pages are errors. Output is UTF-8 JSON
/// version 1, at most 16 MiB. Input bytes and originals are never modified.
/// This synchronous API belongs on a worker thread; cancellation is checked between
/// content operators, form calls and extraction phases. No OCR model is consulted.
pub fn analyze(bytes: &[u8], pages: &[usize], should_stop: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    crate::guard(|| analyze_inner(bytes, pages, should_stop))?
}

fn analyze_inner(bytes: &[u8], selection: &[usize], stop: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    check(stop)?;
    if bytes.is_empty() || bytes.len() > MAX_INPUT {
        return Err("analysis needs a PDF of at most 512 MiB".into());
    }
    let doc = Document::open_with_stream_limit(Arc::new(bytes.to_vec()), None, MAX_STREAM).map_err(|error| format!("cannot analyze PDF: {error}"))?;
    if doc.permissions().is_some_and(|permissions| !permissions.copy()) {
        return Err("PDF permissions do not allow text extraction".into());
    }
    check(stop)?;
    let all = pdfcraft_model::pages(&doc);
    if all.is_empty() || all.len() > MAX_PAGES || selection.len() > MAX_PAGES {
        return Err("analysis supports 1..10000 pages".into());
    }
    let selected = if selection.is_empty() { (0..all.len()).collect::<Vec<_>>() } else { selection.to_vec() };
    let mut seen = BTreeSet::new();
    if selected.iter().any(|page| *page >= all.len() || !seen.insert(*page)) {
        return Err("analysis selection contains a duplicate or nonexistent page".into());
    }
    let mut budget = Budget::default();
    let mut info = BTreeMap::new();
    for key in ["Title", "Author", "Subject", "Keywords", "Creator", "Producer", "CreationDate", "ModDate", "Trapped"] {
        if let Some(value) = pdfcraft_organize::info(&doc, key) {
            budget.text(&value)?;
            info.insert(key.into(), value);
        }
    }
    let repair_log = doc.repair_log().to_vec();
    for warning in &repair_log {
        budget.text(warning)?;
    }
    let has_xmp_metadata = doc.root().and_then(|root| doc.get(root).as_dict().map(|dict| dict.contains(b"Metadata"))).unwrap_or(false);
    let document = Metadata {
        pdf_version: doc.version().into(),
        bytes: bytes.len(),
        page_count: all.len(),
        encrypted: doc.security().is_some(),
        info,
        has_xmp_metadata,
        repair_log,
    };
    let mut reports = Vec::with_capacity(selected.len());
    for page_index in selected {
        check(stop)?;
        let page = all.get(page_index).ok_or("analysis page is missing")?;
        let crop_box = page.crop(&doc);
        let media_box = rectangle(&doc, page.dict.get(b"MediaBox"))?.unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let (width, height) = page.display_size(&doc);
        if ![width, height].iter().all(|value| value.is_finite() && *value > 0.0) {
            return Err(format!("page {} has invalid display geometry", page_index + 1));
        }
        let mut inventory = Inventory::default();
        let resources = dictionary(&doc, page.dict.get(b"Resources"));
        let mut state = Graphics::default();
        let mut work = Work::default();
        if let Some(contents) = page.dict.get(b"Contents") {
            let resolved = doc.resolve(contents);
            let streams = if let Object::Array(array) = &*resolved { array.clone() } else { vec![contents.clone()] };
            if streams.len() > 4096 {
                return Err("analysis page has too many content streams".into());
            }
            for object in streams {
                let object = doc.resolve(&object);
                let Object::Stream(stream) = &*object else {
                    return Err("analysis page has a malformed content stream".into());
                };
                let data = stream.decoded_within(MAX_STREAM).map_err(|error| format!("analysis content cannot be decoded: {error}"))?;
                walk(&doc, &data, &resources, &mut state, &mut inventory, &mut work, stop)?;
            }
        }
        check(stop)?;
        let blocks =
            pdfcraft_edit::reading_blocks(&doc, page_index).map_err(|error| format!("cannot analyze text on page {}: {error}", page_index + 1))?;
        if blocks.len().saturating_add(inventory.images.len()) > MAX_ITEMS {
            return Err("analysis page exceeds 50000 text/image regions".into());
        }
        let mut text_blocks = Vec::new();
        for block in blocks {
            check(stop)?;
            if block.text.trim().is_empty() {
                continue;
            }
            if !block.rect.iter().chain([&block.size]).all(|value| value.is_finite()) {
                return Err("analysis encountered non-finite text geometry".into());
            }
            budget.text(&block.text)?;
            budget.text(&block.base_font)?;
            text_blocks.push(TextBlock {
                text: block.text,
                rect: block.rect,
                base_font: block.base_font,
                font_size: block.size,
                bold: block.bold,
                italic: block.italic,
            });
        }
        for image in &inventory.images {
            if let Some(resource) = &image.resource {
                budget.text(resource)?;
            }
        }
        budget.items = budget.items.saturating_add(text_blocks.len()).saturating_add(inventory.images.len());
        if budget.items > MAX_ITEMS {
            return Err("analysis report exceeds 50000 text/image regions; select fewer pages".into());
        }
        let content = if !text_blocks.is_empty() {
            ContentKind::ExistingText
        } else if !inventory.images.is_empty() {
            ContentKind::ImageWithoutText
        } else if inventory.has_text || inventory.has_vector || inventory.other_paint {
            ContentKind::NoExtractableText
        } else {
            ContentKind::EmptyContent
        };
        if inventory.has_text && text_blocks.is_empty() {
            inventory.warnings.push("Text drawing operators exist, but their text could not be decoded; OCR was not run.".into());
        }
        if page.dict.contains(b"Annots") {
            inventory.warnings.push("Annotation and form-widget appearances are not part of this content inventory.".into());
        }
        reports.push(PageReport {
            page_index,
            page_number: page_index + 1,
            media_box,
            crop_box,
            rotation: page.rotation(&doc),
            user_unit: page.user_unit(&doc),
            display_size_points: [width, height],
            content,
            has_text_operators: inventory.has_text,
            has_vector_paint: inventory.has_vector,
            text_blocks,
            images: inventory.images,
            warnings: inventory.warnings,
        });
    }
    check(stop)?;
    let report = Report {
        format: "pdfcraft-analysis",
        version: 1,
        ocr_performed: false,
        coordinate_space: "PDF default user space; unrotated, y upward. Multiply by page.user_unit for points. Rectangles are [x0,y0,x1,y1].",
        document,
        pages: reports,
        limitations: vec![
            "This describes existing PDF instructions and decoded text, not visual OCR or semantic layout recognition.",
            "Text blocks retain content order; reading order in complex layouts is not guaranteed.",
            "Image rectangles are transformed placement bounds; clipping, masks, transparency and optional-layer visibility are not applied.",
            "An image without text may be a scan or other artwork; an empty content inventory does not prove the rendered page is blank.",
            "Document Info fields are reported; XMP presence is reported without parsing its separate metadata packet.",
        ],
    };
    let mut output = LimitedOutput(Vec::new());
    serde_json::to_writer_pretty(&mut output, &report).map_err(|_| "analysis report exceeds 16 MiB or could not be encoded")?;
    check(stop)?;
    Ok(output.0)
}

#[derive(Default)]
struct Budget {
    text: usize,
    items: usize,
}
impl Budget {
    fn text(&mut self, text: &str) -> Result<(), String> {
        self.text = self.text.saturating_add(text.len());
        if self.text > MAX_TEXT { Err("analysis text exceeds 8 MiB; select fewer pages".into()) } else { Ok(()) }
    }
}
struct LimitedOutput(Vec<u8>);
impl Write for LimitedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_REPORT {
            return Err(std::io::Error::other("analysis output limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn check(stop: &dyn Fn() -> bool) -> Result<(), String> {
    if stop() { Err("analysis cancelled; originals unchanged".into()) } else { Ok(()) }
}
fn dictionary(doc: &Document, object: Option<&Object>) -> Dict {
    object.map(|value| doc.resolve(value)).and_then(|value| value.as_dict().cloned()).unwrap_or_default()
}
fn rectangle(doc: &Document, object: Option<&Object>) -> Result<Option<[f64; 4]>, String> {
    let Some(object) = object else {
        return Ok(None);
    };
    let object = doc.resolve(object);
    let array = object.as_array().filter(|array| array.len() == 4).ok_or("analysis encountered an invalid rectangle")?;
    let mut values = [0.0_f64; 4];
    for (target, value) in values.iter_mut().zip(array) {
        *target = doc.resolve(value).as_f64().filter(|value| value.is_finite()).ok_or("analysis rectangle is non-finite")?;
    }
    Ok(Some([values[0].min(values[2]), values[1].min(values[3]), values[0].max(values[2]), values[1].max(values[3])]))
}
#[derive(Default)]
struct Inventory {
    images: Vec<Image>,
    has_text: bool,
    has_vector: bool,
    other_paint: bool,
    warnings: Vec<String>,
}
struct Graphics {
    matrix: Matrix,
    stack: Vec<Matrix>,
}
impl Default for Graphics {
    fn default() -> Self {
        Self { matrix: Matrix::IDENTITY, stack: Vec::new() }
    }
}
#[derive(Default)]
struct Work {
    decoded: usize,
    operators: usize,
    forms: usize,
    depth: usize,
    path: Vec<ObjRef>,
}

#[allow(clippy::too_many_arguments)]
fn walk(
    doc: &Document,
    data: &[u8],
    resources: &Dict,
    graphics: &mut Graphics,
    inventory: &mut Inventory,
    work: &mut Work,
    stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    check(stop)?;
    work.decoded = work.decoded.saturating_add(data.len());
    if work.decoded > MAX_CONTENT {
        return Err("analysis page expands beyond 8 MiB of content".into());
    }
    let parsed = pdfcraft_content::parse(data);
    if parsed.skipped > 0 {
        return Err("analysis encountered malformed content instructions; report was not published".into());
    }
    work.operators = work.operators.saturating_add(parsed.ops.len());
    if work.operators > MAX_OPERATORS {
        return Err("analysis page exceeds 100000 drawing instructions".into());
    }
    let xobjects = dictionary(doc, resources.get(b"XObject"));
    for op in parsed.ops {
        check(stop)?;
        match op.op.as_slice() {
            b"q" => {
                if graphics.stack.len() >= 256 {
                    return Err("analysis graphics-state nesting exceeds 256".into());
                }
                graphics.stack.push(graphics.matrix);
            }
            b"Q" => graphics.matrix = graphics.stack.pop().ok_or("analysis encountered an unbalanced graphics state")?,
            b"cm" => {
                let matrix =
                    op.nums::<6>().filter(|matrix| matrix.iter().all(|value| value.is_finite())).ok_or("analysis encountered an invalid matrix")?;
                graphics.matrix = Matrix(matrix).then(&graphics.matrix);
                if !graphics.matrix.0.iter().all(|value| value.is_finite()) {
                    return Err("analysis matrix overflow".into());
                }
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => inventory.has_text = true,
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => inventory.has_vector = true,
            b"sh" => inventory.other_paint = true,
            b"BI" => {
                let (dict, _) = op.inline.as_ref().ok_or("analysis inline image is malformed")?;
                push_image(dict, graphics.matrix, None, true, inventory)?;
            }
            b"Do" => {
                let name = op.name(0).ok_or("analysis XObject call has no name")?;
                let object = xobjects.get(name).ok_or("analysis XObject resource is missing")?;
                let id = object.as_ref();
                let object = doc.resolve(object);
                let Object::Stream(stream) = &*object else {
                    return Err("analysis XObject resource is not a stream".into());
                };
                match stream.dict.name(b"Subtype") {
                    Some(b"Image") => push_image(&stream.dict, graphics.matrix, Some(String::from_utf8_lossy(name).into_owned()), false, inventory)?,
                    Some(b"Form") => {
                        work.forms = work.forms.saturating_add(1);
                        if work.depth >= 12 || work.forms > 256 || id.is_some_and(|id| work.path.contains(&id)) {
                            return Err("analysis form recursion or visit limit reached".into());
                        }
                        let form_matrix = if let Some(matrix) = stream.dict.get(b"Matrix") {
                            let value = doc.resolve(matrix);
                            value.as_array().and_then(|values| Matrix::from_operands(values)).ok_or("analysis form matrix is malformed")?
                        } else {
                            Matrix::IDENTITY
                        };
                        let matrix = form_matrix.then(&graphics.matrix);
                        if !matrix.0.iter().all(|value| value.is_finite()) {
                            return Err("analysis form matrix is non-finite".into());
                        }
                        let form_resources =
                            if stream.dict.contains(b"Resources") { dictionary(doc, stream.dict.get(b"Resources")) } else { resources.clone() };
                        let data = stream.decoded_within(MAX_STREAM).map_err(|error| format!("analysis form cannot be decoded: {error}"))?;
                        if let Some(id) = id {
                            work.path.push(id);
                        }
                        work.depth += 1;
                        let mut inner = Graphics { matrix, stack: Vec::new() };
                        walk(doc, &data, &form_resources, &mut inner, inventory, work, stop)?;
                        work.depth -= 1;
                        if id.is_some() {
                            work.path.pop();
                        }
                    }
                    _ => {
                        inventory.other_paint = true;
                        if inventory.warnings.is_empty() {
                            inventory.warnings.push("An unsupported XObject subtype is present.".into());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn push_image(dict: &Dict, matrix: Matrix, resource: Option<String>, inline: bool, inventory: &mut Inventory) -> Result<(), String> {
    if inventory.images.len() >= MAX_ITEMS {
        return Err("analysis page exceeds 50000 image placements".into());
    }
    let rect = matrix.bbox([0.0, 0.0, 1.0, 1.0]);
    if !rect.iter().all(|value| value.is_finite()) {
        return Err("analysis image bounds are non-finite".into());
    }
    let dimension = |long, short| {
        dict.int(long)
            .or_else(|| dict.int(short))
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| "analysis image dimensions are missing or invalid".to_owned())
    };
    inventory.images.push(Image {
        rect,
        matrix: matrix.0,
        pixel_width: dimension(b"Width", b"W")?,
        pixel_height: dimension(b"Height", b"H")?,
        resource,
        inline,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{SaveOptions, Stream, write_full};
    use serde_json::{Value, json};

    fn nums(values: &[f64]) -> Object {
        Object::Array(values.iter().map(|value| Object::Real(*value)).collect())
    }
    fn fixture() -> Document {
        let mut doc = pdfcraft_create::blank(200.0, 300.0, 3).unwrap();
        let pages = pdfcraft_model::pages(&doc);
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
        let font = doc.add(font);
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Ref(font));
        let mut resources = Dict::new();
        resources.set(b"Font".to_vec(), fonts);
        let text = doc.add(Stream::from_raw(Dict::new(), b"BT /F1 12 Tf 20 230 Td (Existing searchable text) Tj ET".to_vec()));
        doc.update_dict(pages[0].obj, |dict| {
            dict.set(b"Resources".to_vec(), resources);
            dict.set(b"Contents".to_vec(), Object::Ref(text));
            dict.set(b"CropBox".to_vec(), nums(&[10.0, 20.0, 190.0, 290.0]));
            dict.set(b"Rotate".to_vec(), Object::Int(90));
            dict.set(b"UserUnit".to_vec(), Object::Real(2.0));
        })
        .unwrap();
        let mut image = Dict::new();
        image.set(b"Subtype".to_vec(), Object::name("Image"));
        image.set(b"Width".to_vec(), Object::Int(1));
        image.set(b"Height".to_vec(), Object::Int(1));
        image.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
        image.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        let image = doc.add(Stream::from_raw(image, vec![90, 100, 110]));
        let mut objects = Dict::new();
        objects.set(b"Im".to_vec(), Object::Ref(image));
        let mut inner_res = Dict::new();
        inner_res.set(b"XObject".to_vec(), objects);
        let mut form = Dict::new();
        form.set(b"Subtype".to_vec(), Object::name("Form"));
        form.set(b"BBox".to_vec(), nums(&[0.0, 0.0, 200.0, 300.0]));
        form.set(b"Matrix".to_vec(), nums(&[1.0, 0.0, 0.0, 1.0, 5.0, 7.0]));
        form.set(b"Resources".to_vec(), inner_res);
        let form = doc.add(Stream::from_raw(form, b"q 20 0 0 30 1 2 cm /Im Do Q".to_vec()));
        let mut objects = Dict::new();
        objects.set(b"Fm".to_vec(), Object::Ref(form));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), objects);
        let content = doc.add(Stream::from_raw(
            Dict::new(),
            b"q 2 0 0 2 50 60 cm /Fm Do Q q 10 0 0 5 100 100 cm BI /W 1 /H 1 /CS /RGB /BPC 8 ID abc EI Q".to_vec(),
        ));
        doc.update_dict(pages[1].obj, |dict| {
            dict.set(b"Resources".to_vec(), resources);
            dict.set(b"Contents".to_vec(), Object::Ref(content));
        })
        .unwrap();
        pdfcraft_organize::set_info(&mut doc, "Title", "Analyse ä <&> 日本語").unwrap();
        pdfcraft_organize::set_info(&mut doc, "CreationDate", "D:20261011120000+02'00'").unwrap();
        doc
    }
    fn bytes(doc: &Document) -> Vec<u8> {
        write_full(doc, &SaveOptions::default()).unwrap()
    }

    #[test]
    fn analysis_reports_text_scan_empty_metadata_and_nested_image_geometry_without_mutation() {
        let pdf = bytes(&fixture());
        let before = pdf.clone();
        let report = analyze(&pdf, &[], &|| false).unwrap();
        assert_eq!(report, analyze(&pdf, &[], &|| false).unwrap(), "report must be deterministic");
        assert_eq!(pdf, before);
        let report: Value = serde_json::from_slice(&report).unwrap();
        assert_eq!(report["format"], "pdfcraft-analysis");
        assert_eq!(report["version"], 1);
        assert_eq!(report["ocr_performed"], false);
        assert_eq!(report["document"]["info"]["Title"], "Analyse ä <&> 日本語");
        assert_eq!(report["document"]["info"]["CreationDate"], "D:20261011120000+02'00'");
        let pages = report["pages"].as_array().unwrap();
        assert_eq!(pages[0]["content"], "existing_text");
        assert_eq!(pages[0]["text_blocks"][0]["text"], "Existing searchable text");
        assert_eq!(pages[0]["crop_box"], json!([10.0, 20.0, 190.0, 290.0]));
        assert_eq!(pages[0]["display_size_points"], json!([540.0, 360.0]));
        assert_eq!(pages[0]["rotation"], 90);
        assert_eq!(pages[0]["user_unit"], 2.0);
        assert_eq!(pages[1]["content"], "image_without_text");
        assert_eq!(pages[1]["images"][0]["rect"], json!([62.0, 78.0, 102.0, 138.0]));
        assert_eq!(pages[1]["images"][1]["rect"], json!([100.0, 100.0, 110.0, 105.0]));
        assert_eq!(pages[1]["images"][1]["inline"], true);
        assert_eq!(pages[2]["content"], "empty_content");
    }

    #[test]
    fn analysis_preserves_selection_order_and_refuses_bad_selection_malformed_pdf_and_cancel() {
        let pdf = bytes(&fixture());
        let report: Value = serde_json::from_slice(&analyze(&pdf, &[2, 0], &|| false).unwrap()).unwrap();
        assert_eq!(report["pages"][0]["page_number"], 3);
        assert_eq!(report["pages"][1]["page_number"], 1);
        assert!(analyze(&pdf, &[3], &|| false).is_err());
        assert!(analyze(&pdf, &[1, 1], &|| false).is_err());
        assert!(analyze(b"not a PDF", &[], &|| false).is_err());
        assert!(analyze(&pdf, &[], &|| true).unwrap_err().contains("cancelled"));
        let calls = std::cell::Cell::new(0);
        assert!(
            analyze(&pdf, &[], &|| {
                calls.set(calls.get() + 1);
                calls.get() > 10
            })
            .unwrap_err()
            .contains("cancelled")
        );
    }

    #[test]
    fn analysis_rejects_recursive_forms_and_oversized_content_without_truncating_report() {
        let mut doc = pdfcraft_create::blank(200.0, 300.0, 1).unwrap();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let form = doc.add(Object::Null);
        let mut objects = Dict::new();
        objects.set(b"Fm".to_vec(), Object::Ref(form));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), objects);
        let mut form_dict = Dict::new();
        form_dict.set(b"Subtype".to_vec(), Object::name("Form"));
        form_dict.set(b"BBox".to_vec(), nums(&[0.0, 0.0, 200.0, 300.0]));
        form_dict.set(b"Resources".to_vec(), resources.clone());
        doc.set(form, Stream::from_raw(form_dict, b"/Fm Do".to_vec()));
        let content = doc.add(Stream::from_raw(Dict::new(), b"/Fm Do".to_vec()));
        doc.update_dict(page, |dict| {
            dict.set(b"Contents".to_vec(), Object::Ref(content));
            dict.set(b"Resources".to_vec(), resources);
        })
        .unwrap();
        assert!(analyze(&bytes(&doc), &[], &|| false).unwrap_err().contains("recursion"));
        doc.set(content, Stream::from_raw(Dict::new(), vec![b' '; MAX_STREAM + 1]));
        assert!(analyze(&bytes(&doc), &[], &|| false).is_err());
    }

    #[test]
    fn analysis_does_not_claim_vector_art_or_undecodable_text_is_a_blank_scan() {
        let mut doc = pdfcraft_create::blank(200.0, 300.0, 1).unwrap();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let content = doc.add(Stream::from_raw(Dict::new(), b"10 10 50 50 re f".to_vec()));
        doc.update_dict(page, |dict| dict.set(b"Contents".to_vec(), Object::Ref(content))).unwrap();
        let report: Value = serde_json::from_slice(&analyze(&bytes(&doc), &[], &|| false).unwrap()).unwrap();
        assert_eq!(report["pages"][0]["content"], "no_extractable_text");
        assert_eq!(report["pages"][0]["has_vector_paint"], true);
        doc.set(content, Stream::from_raw(Dict::new(), b"BT /MissingFont 12 Tf (text) Tj ET".to_vec()));
        let report: Value = serde_json::from_slice(&analyze(&bytes(&doc), &[], &|| false).unwrap()).unwrap();
        assert_ne!(report["pages"][0]["content"], "empty_content");
        assert_eq!(report["pages"][0]["has_text_operators"], true);
    }
}
