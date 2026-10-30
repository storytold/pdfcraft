//! Scan & OCR ▸ Enhance ▸ Scanned document: clean up the pictures scanned pages are made of.
//!
//! Unlike Enhance ▸ Camera image, the page is not rendered and replaced: each scan image is
//! decoded, cleaned ([`crate::camera_clean::clean_samples`]) and written back **into the same
//! image object**, at the same size and in the same colour space. So everything else on the
//! page stays as it was: an invisible OCR text layer, comments, links, form fields, other
//! content, and the image's own soft mask, `/Interpolate` and other entries. A JPEG stays a JPEG
//! (at quality 90), any other grey or RGB image is stored losslessly (Flate), and a black and
//! white (1-bit) image stays 1-bit. An image several pages draw is cleaned once.
//!
//! A page is a scan when its pictures cover at least half of it (as for Recognize text); the
//! pictures each covering at least a tenth of the page are its scan. What is not cleaned is
//! reported, never skipped silently: pages that are no scan, and scan images in a form PdfCraft
//! can't rewrite yet (CMYK, indexed, JPEG 2000, JBIG2, CCITT fax, a stencil mask, a remapping
//! decode array, a scan drawn inside grouped Form artwork) or whose data is broken.
//!
//! Like Optimize, the work is a job: [`Session::scan_clean_job`] captures the document,
//! [`ScanCleanJob::run`] cleans anywhere (a worker thread), and [`Session::apply_scan_clean`]
//! makes the result one undoable step, "Enhance scanned file" (the name the Scan & OCR tool has).

use pdfcraft_cos::{ObjRef, Object, Stream};

use crate::camera_clean::{CleanUp, MAX_LEVEL, clean_samples};
use crate::edit::check_permission;
use crate::{DocId, Edit, EditError, Session, guard, optimize};

pub use crate::camera_clean::DESPECKLE_FROM;

/// The JPEG quality a cleaned JPEG scan is written back at (as Enhance camera image).
const QUALITY: u8 = 90;
/// On a scanned page, an image covering at least this share of the page is part of the scan;
/// smaller ones (a logo or a signature pasted on) are left alone.
const PART_OF_SCAN: f64 = 0.1;
/// Why a scan drawn through grouped Form artwork stays as it is.
const IN_FORM: &str = "it is drawn inside grouped artwork (a form XObject), which can't be cleaned up yet";

/// How scanned pages are cleaned up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanCleanSettings {
    /// Background removal, 0 (off) … 100 (strongest), as Enhance camera image's clean-up level:
    /// the paper's illumination is evened out (fully from 50), so a grey, yellowed or unevenly lit
    /// background turns white; then the contrast is raised so faint strokes darken. Colour ink
    /// stays colour.
    pub level: u8,
    /// Edge shadow removal: shadows along the scan's edges (a book's gutter, a copier's dark
    /// border) are lightened to the paper next to them, line by line, before the level applies.
    pub edge_shadows: bool,
    /// Despeckle: isolated dark specks (dust, scanner noise), dark pixels with at most one dark
    /// neighbour, turn white. The only change a black and white (1-bit) scan takes.
    pub despeckle: bool,
}

impl ScanCleanSettings {
    /// The level `level` (capped at 100) with edge shadow removal (unless the level is 0), and
    /// despeckling from [`DESPECKLE_FROM`] as the camera tool does.
    pub fn at(level: u8) -> Self {
        let level = level.min(MAX_LEVEL);
        Self { level, edge_shadows: level > 0, despeckle: level >= DESPECKLE_FROM }
    }

    /// Whether these settings change anything at all.
    pub fn changes_something(self) -> bool {
        self.level > 0 || self.edge_shadows || self.despeckle
    }
}

impl Default for ScanCleanSettings {
    fn default() -> Self {
        Self::at(50)
    }
}

/// What a clean-up did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanCleanReport {
    /// Pages (0-based) whose scan was cleaned, in order.
    pub pages: Vec<usize>,
    /// Images rewritten. One drawn on several pages counts once.
    pub images: usize,
    /// Pages left alone because no picture covers most of them.
    pub not_scanned: Vec<usize>,
    /// Scan images left as they were, and why.
    pub skipped: Vec<SkippedImage>,
}

/// A scan image that was not cleaned.
#[derive(Clone, Debug, PartialEq)]
pub struct SkippedImage {
    /// The pages (0-based) that draw it, among those asked for.
    pub pages: Vec<usize>,
    /// Its resource name on the first of them.
    pub name: String,
    /// Why, in a sentence that follows "skipped because".
    pub reason: String,
}

/// One image to clean: its object, its name and the pages that draw it.
struct Target {
    object: ObjRef,
    name: String,
    pages: Vec<usize>,
}

/// What cleaning scanned pages needs, independent of the session.
pub struct ScanCleanJob {
    cos: pdfcraft_cos::Document,
    targets: Vec<Target>,
    settings: ScanCleanSettings,
    /// The document's edit generation it was taken at: a result is for that state only.
    generation: u64,
    report: ScanCleanReport,
}

/// The cleaned images, ready for [`Session::apply_scan_clean`], and the report.
pub struct ScanCleaned {
    images: Vec<(ObjRef, Stream)>,
    generation: u64,
    pub report: ScanCleanReport,
}

impl Session {
    /// Capture what cleaning up the scans on `pages` (0-based; empty = all) of document `id`
    /// needs. Errors when the document can't be changed (read-only, or its permissions forbid
    /// changes) or a page doesn't exist.
    pub fn scan_clean_job(&self, id: DocId, pages: &[usize], settings: ScanCleanSettings) -> Result<ScanCleanJob, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if let Some(why) = &doc.read_only_reason {
            return Err(EditError::ReadOnly(why.clone()));
        }
        let editor = doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(String::new()))?;
        if let Some(p) = editor.cos.permissions() {
            check_permission(&Edit::EnhanceScan { images: Vec::new() }, &p)?;
        }
        let all = doc.info.pages.len();
        let mut wanted: Vec<usize> = if pages.is_empty() { (0..all).collect() } else { pages.to_vec() };
        if let Some(p) = wanted.iter().find(|p| **p >= all) {
            return Err(EditError::Invalid(format!("there is no page {}: the document has {all}", p.saturating_add(1))));
        }
        wanted.sort_unstable();
        wanted.dedup();
        let mut report = ScanCleanReport::default();
        let mut targets: Vec<Target> = Vec::new();
        // Form artwork already reported, and where in `report.skipped`.
        let mut forms_reported: Vec<(ObjRef, usize)> = Vec::new();
        for page in wanted {
            let Some(info) = doc.info.pages.get(page) else { continue };
            let Some(images) = crate::scan_fields::scan_images(doc, page, info) else {
                // A scan drawn through a Form XObject is a scan all the same: say why it stays.
                let forms: Vec<_> = crate::scan_fields::scan_forms(doc, page, info).into_iter().filter(|(_, share)| *share >= PART_OF_SCAN).collect();
                if forms.is_empty() {
                    report.not_scanned.push(page);
                }
                for (form, _) in forms {
                    let known = form.object.and_then(|o| forms_reported.iter().find(|(r, _)| *r == o)).and_then(|(_, k)| report.skipped.get_mut(*k));
                    match known {
                        Some(s) if !s.pages.contains(&page) => s.pages.push(page),
                        Some(_) => {}
                        None => {
                            if let Some(o) = form.object {
                                forms_reported.push((o, report.skipped.len()));
                            }
                            report.skipped.push(SkippedImage { pages: vec![page], name: form.name, reason: IN_FORM.into() });
                        }
                    }
                }
                continue;
            };
            for (image, _) in images.into_iter().filter(|(_, share)| *share >= PART_OF_SCAN) {
                let Some(object) = image.object else {
                    report.skipped.push(SkippedImage {
                        pages: vec![page],
                        name: image.name,
                        reason: "it is stored inside the page's resources rather than as an object of its own".into(),
                    });
                    continue;
                };
                match targets.iter_mut().find(|t| t.object == object) {
                    Some(t) if !t.pages.contains(&page) => t.pages.push(page),
                    Some(_) => {}
                    None => targets.push(Target { object, name: image.name, pages: vec![page] }),
                }
            }
        }
        Ok(ScanCleanJob { cos: editor.cos.clone(), targets, settings, generation: doc.edit_generation(), report })
    }

    /// Write the images [`ScanCleanJob::run`] cleaned into document `id` as one undoable step,
    /// "Enhance scanned file" (none when nothing was cleaned), and return the report. Refused
    /// when the document changed since the job was taken.
    pub fn apply_scan_clean(&mut self, id: DocId, cleaned: ScanCleaned) -> Result<ScanCleanReport, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if doc.edit_generation() != cleaned.generation {
            return Err(EditError::Invalid("the document changed while its scans were being cleaned up; nothing was changed, try again".into()));
        }
        if !cleaned.images.is_empty() {
            self.apply(id, Edit::EnhanceScan { images: cleaned.images })?;
        }
        Ok(cleaned.report)
    }

    /// Enhance ▸ Scanned document in one call: [`Self::scan_clean_job`], run, then
    /// [`Self::apply_scan_clean`] (the CLI and agents use this).
    pub fn enhance_scanned_pages(&mut self, id: DocId, pages: &[usize], settings: ScanCleanSettings) -> Result<ScanCleanReport, EditError> {
        let job = self.scan_clean_job(id, pages, settings)?;
        let cleaned = job.run(|_, _| true)?;
        self.apply_scan_clean(id, cleaned)
    }
}

impl ScanCleanJob {
    /// The images to clean (one drawn on several pages counts once).
    pub fn total(&self) -> usize {
        self.targets.len()
    }

    /// Clean every scan image. `progress(done, total)` is called before each image; returning
    /// `false` stops with [`EditError::Cancelled`]. A panic is returned as an error, so a worker
    /// thread always reports back.
    pub fn run(self, mut progress: impl FnMut(usize, usize) -> bool) -> Result<ScanCleaned, EditError> {
        guard(move || self.work(&mut progress)).map_err(|m| EditError::Invalid(format!("cleaning up the scans failed unexpectedly ({m})")))?
    }

    fn work(self, progress: &mut dyn FnMut(usize, usize) -> bool) -> Result<ScanCleaned, EditError> {
        let ScanCleanJob { cos, targets, settings, generation, mut report } = self;
        let total = targets.len();
        let mut images = Vec::new();
        let mut changed_pages: Vec<usize> = Vec::new();
        for (done, target) in targets.into_iter().enumerate() {
            if !progress(done, total) {
                return Err(EditError::Cancelled);
            }
            let mut skip = |reason: String| report.skipped.push(SkippedImage { pages: target.pages.clone(), name: target.name.clone(), reason });
            match clean_image(&cos, target.object, settings) {
                Ok(Some(stream)) => {
                    images.push((target.object, stream));
                    changed_pages.extend(target.pages.iter().copied());
                }
                Ok(None) => skip("it is already clean: nothing changes at these settings".into()),
                Err(reason) => skip(reason),
            }
        }
        changed_pages.sort_unstable();
        changed_pages.dedup();
        report.images = images.len();
        report.pages = changed_pages;
        report.skipped.sort_by_key(|s| s.pages.first().copied());
        Ok(ScanCleaned { images, generation, report })
    }
}

/// Image `object` cleaned with `settings`: the new stream, `None` when nothing changes, or why it
/// can't be cleaned.
fn clean_image(cos: &pdfcraft_cos::Document, object: ObjRef, settings: ScanCleanSettings) -> Result<Option<Stream>, String> {
    let obj = cos.get(object);
    let Object::Stream(s) = &*obj else { return Err("it is not an image".into()) };
    if s.dict.name(b"Subtype") != Some(b"Image") {
        return Err("it is not an image".into());
    }
    // A colour-key mask names sample values to leave unpainted: cleaned samples would match others.
    if matches!(s.dict.get(b"Mask").map(|m| cos.resolve(m)).as_deref(), Some(Object::Array(_))) {
        return Err("it has a colour-key mask, which cleaning would change".into());
    }
    let samples = optimize::decode_samples(cos, s).map_err(|e| e.to_string())?;
    let cleaned = if samples.bilevel {
        if !settings.despeckle {
            return Err("it is black and white, which only Despeckle changes, and Despeckle is off".into());
        }
        clean_samples(&samples.data, 1, samples.width, samples.height, CleanUp { level: 0, edge_shadows: false, despeckle: true })
    } else {
        if !settings.changes_something() {
            return Ok(None);
        }
        let how = CleanUp { level: settings.level, edge_shadows: settings.edge_shadows, despeckle: settings.despeckle };
        clean_samples(&samples.data, samples.components, samples.width, samples.height, how)
    }
    .ok_or("its samples don't match its size")?;
    if cleaned == samples.data {
        return Ok(None);
    }
    let cleaned = optimize::Samples { data: cleaned, ..samples };
    optimize::encode_samples(s, &cleaned, QUALITY).map(Some).ok_or_else(|| "it could not be written back".into())
}

/// [`Edit::EnhanceScan`]: put the cleaned images in place. Each object must still be an image.
pub(crate) fn write_back(doc: &mut pdfcraft_cos::Document, images: &[(ObjRef, Stream)]) -> Result<(), EditError> {
    for (object, stream) in images {
        let is_image = matches!(&*doc.get(*object), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image"));
        if !is_image {
            return Err(EditError::Invalid(format!("object {} is no longer an image; the scan was not changed", object.num)));
        }
        doc.set(*object, Object::Stream(stream.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
