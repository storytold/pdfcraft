//! Print readiness, verification only.
//!
//! Measures what a press looks at and changes nothing: effective image resolution where each
//! image is drawn, the page boxes (media, crop, trim, bleed), the output intent, fonts that
//! aren't embedded, and the existing PDF/A-2b result. There is no fixup and no generated colour
//! profile. A screen profile such as sRGB is reported as a screen profile, not accepted as a
//! press condition.

use pdfcraft_cos::Document;
use pdfcraft_edit::PageImage;
use pdfcraft_preflight::{Issue, Level};

use crate::Document as OpenDocument;

/// Below this, ordinary sheet printing cannot hold the picture. Large-format work sometimes can.
const FAIL_PPI: f64 = 150.0;
/// Ordinary close-up print. Between this and [`FAIL_PPI`] the picture is usable but soft.
const WARN_PPI: f64 = 300.0;
/// Less than this past the trim, on the tightest side, is not a normal 3 mm bleed.
const MIN_BLEED_MM: f64 = 2.0;

/// One thing a printer should see, without the file having been rewritten.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub id: &'static str,
    pub severity: &'static str,
    pub message: String,
    /// 1-based page, when the finding is about one page.
    pub page: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageBoxes {
    pub page: u32,
    pub media_mm: [f64; 2],
    pub crop_mm: [f64; 2],
    pub trim_mm: [f64; 2],
    pub bleed_mm: [f64; 2],
    pub trim_set: bool,
    pub bleed_set: bool,
    /// Left, bottom, right, top, in millimetres, from the trim out to the bleed.
    pub bleed_margin_mm: [f64; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrawnImage {
    pub page: u32,
    pub name: String,
    pub ppi: f64,
    pub pixels: [u32; 2],
    pub size_mm: [f64; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrintReadiness {
    pub pages: Vec<PageBoxes>,
    pub images: Vec<DrawnImage>,
    pub output_intents: Vec<String>,
    pub fonts_not_embedded: Vec<String>,
    pub pdfa_issues: Vec<Issue>,
    pub findings: Vec<Finding>,
    /// Image placements whose resolution couldn't be measured (a degenerate or non-finite
    /// placement matrix). They are neither passed nor failed.
    pub resolution_unknown: usize,
}

impl PrintReadiness {
    /// A plain-text report. The document is unchanged.
    pub fn text(&self) -> String {
        let mut out = String::from("Print readiness (verification only; the file was not changed)\n");
        if self.output_intents.is_empty() {
            out.push_str("Output intent: none\n");
        } else {
            out.push_str(&format!("Output intent: {}\n", self.output_intents.join(", ")));
        }
        for page in &self.pages {
            if !page.trim_set {
                out.push_str(&format!("Page {}: media {} mm, no TrimBox\n", page.page, size_mm(page.media_mm)));
                continue;
            }
            out.push_str(&format!(
                "Page {}: media {} mm, trim {} mm, bleed margin {} mm{}\n",
                page.page,
                size_mm(page.media_mm),
                size_mm(page.trim_mm),
                min_mm(page.bleed_margin_mm),
                if page.bleed_set { "" } else { " (no BleedBox)" },
            ));
        }
        for image in &self.images {
            out.push_str(&format!(
                "Image {} on page {}: {:.0} ppi, {} × {} px, {} mm\n",
                image.name,
                image.page,
                image.ppi,
                image.pixels[0],
                image.pixels[1],
                size_mm(image.size_mm),
            ));
        }
        if self.resolution_unknown > 0 {
            out.push_str(&format!("Images with unknown resolution: {}\n", self.resolution_unknown));
        }
        if self.fonts_not_embedded.is_empty() {
            out.push_str("Fonts: every referenced font is embedded, or the file has none\n");
        } else {
            out.push_str(&format!("Fonts not embedded: {}\n", self.fonts_not_embedded.join(", ")));
        }
        let level = Level::A2b.label();
        if self.pdfa_issues.is_empty() {
            out.push_str(&format!("{level}: no problems found by the structural check\n"));
        } else {
            out.push_str(&format!("{level}: {} problem{}\n", self.pdfa_issues.len(), if self.pdfa_issues.len() == 1 { "" } else { "s" }));
            for issue in &self.pdfa_issues {
                match issue.page {
                    Some(p) => out.push_str(&format!("- page {}: {}\n", p + 1, issue.message)),
                    None => out.push_str(&format!("- {}\n", issue.message)),
                }
            }
        }
        if self.findings.is_empty() {
            out.push_str("Print checks: no stop for image resolution or bleed\n");
        } else {
            out.push_str("Print checks:\n");
            for finding in &self.findings {
                out.push_str(&format!("- [{}] {}\n", finding.severity, finding.message));
            }
        }
        out.push_str("Not checked: PDF/X, overprint, total area coverage, spot colours, transparency flattening.\n");
        out
    }
}

impl OpenDocument {
    /// A print-readiness report. Reads the file and does not edit it.
    pub fn print_readiness(&self) -> PrintReadiness {
        match &self.editor {
            Some(editor) => inspect(&editor.cos),
            None => PrintReadiness {
                pages: Vec::new(),
                images: Vec::new(),
                output_intents: Vec::new(),
                fonts_not_embedded: Vec::new(),
                pdfa_issues: Vec::new(),
                resolution_unknown: 0,
                findings: vec![Finding {
                    id: "document",
                    severity: "info",
                    message: "The document has no object graph to measure.".into(),
                    page: None,
                }],
            },
        }
    }
}

fn inspect(doc: &Document) -> PrintReadiness {
    let declared = pdfcraft_preflight::declared(doc);
    let pdfa_issues = pdfcraft_preflight::verify(doc, Level::A2b);
    let fonts_not_embedded = pdfa_issues
        .iter()
        .filter(|issue| issue.pattern.contains("isn't embedded") || issue.pattern == "A font isn't embedded")
        .map(|issue| issue.value.clone().unwrap_or_else(|| "unnamed font".into()))
        .collect();
    let model_pages = pdfcraft_model::pages(doc);
    let pages = page_reports(doc, &model_pages);
    let (images, resolution_unknown) = image_reports(doc, &model_pages);
    let mut findings = Vec::new();
    // Without a TrimBox the trim, the bleed and the crop are the same box, so there is no bleed to
    // measure. That is a property of the document, not a fault on every page: report it once.
    let untrimmed = pages.iter().filter(|page| !page.trim_set).count();
    if untrimmed > 0 {
        findings.push(Finding {
            id: "page-trim",
            severity: "info",
            message: if untrimmed == pages.len() {
                "No page has a TrimBox, so bleed can't be measured.".to_string()
            } else {
                format!("{untrimmed} of {} pages have no TrimBox, so their bleed can't be measured.", pages.len())
            },
            page: None,
        });
    }
    for page in pages.iter().filter(|page| page.trim_set) {
        let tight = page.bleed_margin_mm.into_iter().fold(f64::INFINITY, f64::min);
        if tight < MIN_BLEED_MM {
            findings.push(Finding {
                id: "page-bleed",
                severity: "warn",
                message: format!(
                    "Page {} has {:.1} mm of bleed past the trim on its tightest side (a press usually wants about 3 mm). Nothing was added.",
                    page.page,
                    tight.max(0.0)
                ),
                page: Some(page.page),
            });
        }
    }
    for image in &images {
        if image.ppi < FAIL_PPI {
            findings.push(Finding {
                id: "image-ppi",
                severity: "fail",
                message: format!(
                    "Image {} on page {} is drawn at {:.0} ppi, below {:.0}. Enlarging it would not add detail.",
                    image.name, image.page, image.ppi, FAIL_PPI
                ),
                page: Some(image.page),
            });
        } else if image.ppi < WARN_PPI {
            findings.push(Finding {
                id: "image-ppi",
                severity: "warn",
                message: format!(
                    "Image {} on page {} is drawn at {:.0} ppi, below {:.0} for close-up print.",
                    image.name, image.page, image.ppi, WARN_PPI
                ),
                page: Some(image.page),
            });
        }
    }
    if declared.output_intents.iter().any(|intent| is_screen_profile(intent))
        && !declared.output_intents.iter().any(|intent| is_press_profile(intent))
    {
        findings.push(Finding {
            id: "output-intent",
            severity: "warn",
            message: format!(
                "The output intent ({}) is a screen profile. A press condition was not added, because inventing one would retint the file.",
                declared.output_intents.join(", ")
            ),
            page: None,
        });
    }
    if resolution_unknown > 0 {
        findings.push(Finding {
            id: "image-ppi-unknown",
            severity: "info",
            message: format!(
                "The resolution of {resolution_unknown} image placement{} couldn't be measured (its placement matrix is degenerate or not finite).",
                if resolution_unknown == 1 { "" } else { "s" }
            ),
            page: None,
        });
    }
    findings.sort_by(|a, b| a.page.cmp(&b.page).then(a.id.cmp(b.id)).then(a.message.cmp(&b.message)));
    PrintReadiness { pages, images, output_intents: declared.output_intents, fonts_not_embedded, pdfa_issues, findings, resolution_unknown }
}

fn page_reports(doc: &Document, pages: &[pdfcraft_model::Page]) -> Vec<PageBoxes> {
    let boxes = pdfcraft_organize::page_boxes(doc).unwrap_or_default();
    pages
        .iter()
        .enumerate()
        .map(|(i, page)| {
            let row = boxes.get(i).copied().unwrap_or([[0.0; 4]; 5]);
            let trim = row[3];
            let bleed = row[2];
            PageBoxes {
                page: (i + 1) as u32,
                media_mm: wh_mm(row[0]),
                crop_mm: wh_mm(row[1]),
                trim_mm: wh_mm(trim),
                bleed_mm: wh_mm(bleed),
                trim_set: page.dict.contains(b"TrimBox"),
                bleed_set: page.dict.contains(b"BleedBox"),
                bleed_margin_mm: [mm(trim[0] - bleed[0]), mm(trim[1] - bleed[1]), mm(bleed[2] - trim[2]), mm(bleed[3] - trim[3])],
            }
        })
        .collect()
}

/// The images drawn on each page, and how many placements had no measurable resolution.
fn image_reports(doc: &Document, pages: &[pdfcraft_model::Page]) -> (Vec<DrawnImage>, usize) {
    let mut out = Vec::new();
    let mut unknown = 0usize;
    let mut seen = std::collections::HashSet::new();
    for (i, page) in pages.iter().enumerate() {
        let page_no = (i + 1) as u32;
        if let Ok(placed) = pdfcraft_edit::page_images(doc, i) {
            for image in placed {
                if let Some(ppi) = placed_ppi(&image) {
                    if let Some(object) = image.object {
                        seen.insert((page_no, object));
                    }
                    if !ppi.is_finite() {
                        unknown = unknown.saturating_add(1);
                        continue;
                    }
                    let w = (image.rect[2] - image.rect[0]).abs();
                    let h = (image.rect[3] - image.rect[1]).abs();
                    out.push(DrawnImage { page: page_no, name: image.name, ppi, pixels: [image.width, image.height], size_mm: [mm(w), mm(h)] });
                }
            }
        }
        // Images drawn inside a form XObject are not listed above. Their resolution is the
        // smallest use on this page, which is what the optimizer already measures.
        for (object, ppi) in pdfcraft_optimize::effective_resolutions(doc, &[page.obj]) {
            if seen.contains(&(page_no, object)) {
                continue;
            }
            if !ppi.is_finite() {
                unknown = unknown.saturating_add(1);
                continue;
            }
            out.push(DrawnImage { page: page_no, name: format!("object {object}"), ppi, pixels: [0, 0], size_mm: [0.0, 0.0] });
        }
    }
    out.sort_by(|a, b| a.page.cmp(&b.page).then(a.name.cmp(&b.name)).then(a.ppi.total_cmp(&b.ppi)));
    (out, unknown)
}

/// The effective resolution of a raster placement, `None` for Form artwork. Not finite (NaN)
/// when the placement can't be measured: callers report that rather than pass it.
fn placed_ppi(image: &PageImage) -> Option<f64> {
    if image.is_form {
        return None;
    }
    Some(matrix_ppi(image.matrix, image.width, image.height))
}

/// Pixels per inch of a `width` × `height` image drawn through `matrix`, or NaN when the matrix
/// is degenerate or not finite or the image has no pixels.
fn matrix_ppi(matrix: [f64; 6], width: u32, height: u32) -> f64 {
    let [a, b, c, d, ..] = matrix;
    let (sx, sy) = (a.hypot(b), c.hypot(d));
    if width == 0 || height == 0 || !sx.is_finite() || !sy.is_finite() || sx <= 1e-6 || sy <= 1e-6 {
        return f64::NAN;
    }
    let ppi = (f64::from(width) / (sx / 72.0)).min(f64::from(height) / (sy / 72.0));
    if ppi.is_finite() { ppi } else { f64::NAN }
}

fn is_screen_profile(intent: &str) -> bool {
    let id = intent.to_ascii_lowercase();
    id.contains("srgb") || id.contains("adobe rgb") || id.contains("display")
}

fn is_press_profile(intent: &str) -> bool {
    let id = intent.to_ascii_lowercase();
    id.contains("cmyk") || id.contains("fogra") || id.contains("coated") || id.contains("swop") || id.contains("gracol")
}

fn mm(points: f64) -> f64 {
    points * 25.4 / 72.0
}

fn wh_mm(rect: [f64; 4]) -> [f64; 2] {
    [mm((rect[2] - rect[0]).abs()), mm((rect[3] - rect[1]).abs())]
}

fn size_mm(wh: [f64; 2]) -> String {
    format!("{:.1} × {:.1}", wh[0], wh[1])
}

fn min_mm(sides: [f64; 4]) -> String {
    let n = sides.into_iter().fold(f64::INFINITY, f64::min);
    if n.is_finite() { format!("{n:.1}") } else { "0.0".into() }
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Dict, Document, Object, Stream};

    use super::*;

    fn image(doc: &mut Document, w: u32, h: u32) -> pdfcraft_cos::ObjRef {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w as i64));
        d.set(b"Height".to_vec(), Object::Int(h as i64));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        doc.add(Object::Stream(Stream::flate(d, &vec![0; (w as usize) * (h as usize)])))
    }

    fn page(doc: &mut Document, name: &str, object: pdfcraft_cos::ObjRef, content: &str) -> pdfcraft_cos::ObjRef {
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
        let mut x = Dict::new();
        x.set(name.as_bytes().to_vec(), Object::Ref(object));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(x));
        let contents = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.as_bytes().to_vec())));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages));
        p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
        p.set(b"Resources".to_vec(), Object::Dict(res));
        p.set(b"Contents".to_vec(), Object::Ref(contents));
        let r = doc.add(Object::Dict(p));
        doc.update_dict(pages, |d| {
            let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
            kids.push(Object::Ref(r));
            d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
            d.set(b"Kids".to_vec(), Object::Array(kids));
        })
        .unwrap();
        r
    }

    #[test]
    fn a_drawn_image_is_measured_where_it_is_placed() {
        let mut doc = Document::new_empty();
        // 100 px across 144 pt (2 in) is 50 ppi, not "100 px on a letter page".
        let soft = image(&mut doc, 100, 100);
        // 600 px across 72 pt (1 in) is 600 ppi.
        let sharp = image(&mut doc, 600, 600);
        page(&mut doc, "Soft", soft, "q 144 0 0 144 36 600 cm /Soft Do Q");
        let sharp_page = page(&mut doc, "Sharp", sharp, "q 72 0 0 72 36 36 cm /Sharp Do Q");
        doc.update_dict(sharp_page, |d| {
            let inset = |n: i64| Object::Array(vec![n.into(), n.into(), (612 - n).into(), (792 - n).into()]);
            d.set(b"TrimBox".to_vec(), inset(18));
            d.set(b"BleedBox".to_vec(), inset(0));
        })
        .unwrap();

        let report = inspect(&doc);
        let soft = report.images.iter().find(|image| image.name == "Soft").unwrap();
        let sharp = report.images.iter().find(|image| image.name == "Sharp").unwrap();
        assert!((soft.ppi - 50.0).abs() < 0.5, "{}", soft.ppi);
        assert!((sharp.ppi - 600.0).abs() < 0.5, "{}", sharp.ppi);
        assert!(report.findings.iter().any(|f| f.id == "image-ppi" && f.severity == "fail" && f.page == Some(1)));
        assert!(!report.findings.iter().any(|f| f.id == "image-ppi" && f.page == Some(2)));
        // Page 1 has no TrimBox: one document-level note, not a bleed warning per page.
        assert!(!report.findings.iter().any(|f| f.id == "page-bleed"), "{:?}", report.findings);
        let trim: Vec<_> = report.findings.iter().filter(|f| f.id == "page-trim").collect();
        assert_eq!(trim.len(), 1, "{:?}", report.findings);
        assert_eq!((trim[0].severity, trim[0].page), ("info", None));
        assert!(trim[0].message.starts_with("1 of 2 pages"), "{}", trim[0].message);
        assert_eq!(report.resolution_unknown, 0);
        assert!(report.text().contains("50 ppi"));
        assert!(report.text().contains("verification only"));
    }

    #[test]
    fn untrimmed_pages_get_one_note_and_a_tight_trim_gets_a_warning() {
        let mut doc = Document::new_empty();
        let img = image(&mut doc, 600, 600);
        for _ in 0..3 {
            page(&mut doc, "Im", img, "q 72 0 0 72 36 36 cm /Im Do Q");
        }
        let report = inspect(&doc);
        let notes: Vec<_> = report.findings.iter().filter(|f| f.id.starts_with("page-")).collect();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(notes[0].message, "No page has a TrimBox, so bleed can't be measured.");

        // A TrimBox 1 pt inside the crop, with no BleedBox: 0.35 mm of bleed is a real warning.
        let tight = page(&mut doc, "Im", img, "q 72 0 0 72 36 36 cm /Im Do Q");
        doc.update_dict(tight, |d| d.set(b"TrimBox".to_vec(), Object::Array(vec![1.into(), 1.into(), 611.into(), 791.into()]))).unwrap();
        let report = inspect(&doc);
        assert!(report.findings.iter().any(|f| f.id == "page-bleed" && f.severity == "warn" && f.page == Some(4)));
        assert_eq!(report.findings.iter().filter(|f| f.id == "page-bleed").count(), 1);
    }

    #[test]
    fn an_unmeasurable_placement_is_never_passed() {
        assert!(matrix_ppi([f64::NAN, 0.0, 0.0, 72.0, 0.0, 0.0], 600, 600).is_nan());
        assert!(matrix_ppi([f64::INFINITY, 0.0, 0.0, 72.0, 0.0, 0.0], 600, 600).is_nan());
        assert!(matrix_ppi([0.0, 0.0, 0.0, 72.0, 0.0, 0.0], 600, 600).is_nan());
        assert!(matrix_ppi([72.0, 0.0, 0.0, 72.0, 0.0, 0.0], 0, 600).is_nan());
        assert!((matrix_ppi([72.0, 0.0, 0.0, 72.0, 0.0, 0.0], 600, 600) - 600.0).abs() < 1e-9);

        // A placement scaled past f64's range and then multiplied by zero has a NaN matrix. The
        // content readers drop it before it is measured; it must never be reported as passing.
        let mut doc = Document::new_empty();
        let img = image(&mut doc, 100, 100);
        let huge = format!("1{}", "0".repeat(400));
        page(&mut doc, "Im", img, &format!("q {huge} 0 0 {huge} 0 0 cm 0 1 1 0 0 0 cm /Im Do Q"));
        let report = inspect(&doc);
        assert!(report.images.iter().all(|image| image.ppi.is_finite()), "{:?}", report.images);
        assert!(!report.findings.iter().any(|f| f.id == "image-ppi"), "{:?}", report.findings);
    }
}
