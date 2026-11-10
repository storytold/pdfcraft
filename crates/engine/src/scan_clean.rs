//! Scan & OCR ▸ Enhance ▸ Scanned document: clean up the pictures scanned pages are made of.
//!
//! Unlike Enhance ▸ Camera image, the page is not rendered and replaced: each scan image is
//! decoded, cleaned ([`crate::camera_clean::clean_samples`]) and written back **into the same
//! image object**, at the same size and in the same colour space. So everything else on the
//! page stays as it was: an invisible OCR text layer, comments, links, form fields, other
//! content, and the image's own soft mask, `/Interpolate` and other entries. A JPEG stays a JPEG
//! (at quality 90), a JPEG 2000 image becomes one (it is read, not written back as JPEG 2000;
//! 8-bit grey up to 40 million pixels or RGB up to 20 million, without a palette or an alpha
//! channel),
//! any other grey or RGB image is stored losslessly (Flate), and a black and white (1-bit) image
//! stays 1-bit (a CCITT fax one as Group 4, any other as Flate), unless it is a halftone
//! ([`crate::descreen`]: dots all over, with no crisp ink) and halftone restore is on, when it is
//! restored to 8-bit grey. An image several pages draw is cleaned once.
//!
//! A page is a scan when its pictures cover at least half of it (as for Recognize text); the
//! pictures each covering at least a tenth of the page are its scan. What is not cleaned is
//! reported, never skipped silently: pages that are no scan, and scan images in a form PdfCraft
//! can't rewrite yet (CMYK, indexed, JBIG2, a stencil mask, a remapping decode array, a scan
//! drawn inside grouped Form artwork) or whose data is broken.
//! The ink layer of a layered (mixed-raster) scan, a picture drawn through a stencil mask, is
//! reported and left too: its colours are the text's, and cleaning them as paper lightens the text.
//!
//! Like Optimize, the work is a job: [`Session::scan_clean_job`] captures the document,
//! [`ScanCleanJob::run`] cleans anywhere (a worker thread), and [`Session::apply_scan_clean`]
//! makes the result one undoable step, "Enhance scanned file" (the name the Scan & OCR tool has).

use pdfcraft_cos::{ObjRef, Object, Stream};

use crate::camera_clean::{CleanUp, MAX_LEVEL, clean_samples};
use crate::edit::check_permission;
use crate::{DocId, Edit, EditError, Session, descreen, guard, optimize};

pub use crate::camera_clean::DESPECKLE_FROM;
pub use crate::descreen::Tuning as HalftoneTuning;

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
    /// Halftone restore: a black and white (1-bit) scan that is dots all over, a faint page a
    /// scanner or fax machine dithered, becomes the clean grey picture it was made from
    /// ([`crate::descreen`]). Off, such a scan is treated as any other 1-bit one.
    pub restore_halftones: bool,
    /// How a halftone (dotted) 1-bit scan looks once restored: stroke weight, contrast and paper
    /// clean-up. Nothing else takes them.
    pub halftone: HalftoneTuning,
}

impl ScanCleanSettings {
    /// The level `level` (capped at 100) with edge shadow removal and halftone restore (unless the
    /// level is 0), and despeckling from [`DESPECKLE_FROM`] as the camera tool does.
    pub fn at(level: u8) -> Self {
        let level = level.min(MAX_LEVEL);
        Self { level, edge_shadows: level > 0, despeckle: level >= DESPECKLE_FROM, restore_halftones: level > 0, halftone: HalftoneTuning::default() }
    }

    /// Whether these settings change anything at all.
    pub fn changes_something(self) -> bool {
        self.level > 0 || self.edge_shadows || self.despeckle || self.restore_halftones
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
    /// Halftone scans restored: what they are stored as, and the pages (0-based) that draw them.
    pub halftone: Vec<HalftoneImage>,
}

/// A halftone scan that was restored.
#[derive(Clone, Debug, PartialEq)]
pub struct HalftoneImage {
    /// The image.
    pub object: ObjRef,
    /// How the image is stored, for example "CCITT fax 1-bit".
    pub format: String,
    pub pages: Vec<usize>,
}

/// The images an earlier run of a clean-up cleaned, as they are in the document, for a run after
/// it that changes only some pages ([`ScanCleanJob::keeping`]).
pub struct ScanCleanKept {
    cos: pdfcraft_cos::Document,
    halftone: Vec<HalftoneImage>,
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
    /// Halftone tunings of single pages (0-based), which take the place of the settings' for the
    /// image drawn on that page.
    page_tunings: Vec<(usize, HalftoneTuning)>,
    /// An earlier run's images, and the pages (0-based) whose images are cleaned again all the same.
    kept: Option<(ScanCleanKept, Vec<usize>)>,
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
            // Ordinary images and scan-holding Forms are classified together, so a page that
            // mixes them gets its images cleaned and its Forms reported.
            let Some((images, forms, own_scan)) = crate::scan_fields::scan_parts(doc, page, info) else {
                report.not_scanned.push(page);
                continue;
            };
            let forms: Vec<_> = forms.into_iter().filter(|(_, share)| *share >= PART_OF_SCAN).collect();
            if !own_scan && forms.is_empty() {
                report.not_scanned.push(page);
                continue;
            }
            // A scan drawn through a Form XObject is a scan all the same: say why it stays.
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
        Ok(ScanCleanJob {
            cos: editor.cos.clone(),
            targets,
            settings,
            generation: doc.edit_generation(),
            report,
            page_tunings: Vec::new(),
            kept: None,
        })
    }

    /// The scan images of document `id` as a clean-up left them (`halftone` is what its report
    /// says it restored), taken before that clean-up is undone to be run again with other
    /// settings for some pages: see [`ScanCleanJob::keeping`].
    pub fn scan_clean_kept(&self, id: DocId, halftone: Vec<HalftoneImage>) -> Result<ScanCleanKept, EditError> {
        let editor = self.get(id).and_then(|d| d.editor.as_ref()).ok_or(EditError::NoDocument)?;
        Ok(ScanCleanKept { cos: editor.cos.clone(), halftone })
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
    /// Give single pages their own halftone tuning: an image takes that of the first of its pages
    /// that has one, else the settings'.
    pub fn with_page_tunings(mut self, tunings: Vec<(usize, HalftoneTuning)>) -> Self {
        self.page_tunings = tunings;
        self
    }

    /// Clean again only the images drawn on `again` (0-based pages): every other image the earlier
    /// run `kept` changed is taken from it as it is. The result is what a full run would give when
    /// nothing but the tuning of those pages differs, without the time: a restore of one page
    /// takes about a second, and a book has hundreds.
    pub fn keeping(mut self, kept: ScanCleanKept, again: &[usize]) -> Self {
        self.kept = Some((kept, again.to_vec()));
        self
    }

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
        let ScanCleanJob { cos, targets, settings, generation, mut report, page_tunings, kept } = self;
        let total = targets.len();
        let mut images = Vec::new();
        let mut changed_pages: Vec<usize> = Vec::new();
        // One decoding allowance for the whole run, sized by the file.
        let mut budget = pdfcraft_cos::ImageBudget::new(&cos);
        for (done, target) in targets.into_iter().enumerate() {
            if !progress(done, total) {
                return Err(EditError::Cancelled);
            }
            // What the earlier run made of an image none of whose pages is cleaned again.
            if let Some((earlier, again)) = &kept
                && !target.pages.iter().any(|p| again.contains(p))
                && let Some(stream) = changed_stream(&cos, &earlier.cos, target.object)
            {
                report.halftone.extend(earlier.halftone.iter().find(|h| h.object == target.object).cloned());
                images.push((target.object, stream));
                changed_pages.extend(target.pages.iter().copied());
                continue;
            }
            let mut skip = |reason: String| report.skipped.push(SkippedImage { pages: target.pages.clone(), name: target.name.clone(), reason });
            let halftone = target.pages.iter().find_map(|p| page_tunings.iter().find(|(q, _)| q == p)).map_or(settings.halftone, |(_, t)| *t);
            // One image a decoder panics on is that image's bad data: the others are still cleaned.
            let cleaned = guard(|| clean_image(&cos, target.object, ScanCleanSettings { halftone, ..settings }, &mut budget))
                .unwrap_or_else(|m| Err(format!("its data could not be decoded (the decoder stopped: {m})")));
            match cleaned {
                Ok(Some((stream, halftone))) => {
                    if let Some(format) = halftone {
                        report.halftone.push(HalftoneImage { object: target.object, format, pages: target.pages.clone() });
                    }
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

/// The stream `object` is in `later`, when that is not the one it is in `now`.
fn changed_stream(now: &pdfcraft_cos::Document, later: &pdfcraft_cos::Document, object: ObjRef) -> Option<Stream> {
    let (before, after) = (now.get(object), later.get(object));
    match &*after {
        Object::Stream(s) if !std::sync::Arc::ptr_eq(&before, &after) && *before != *after => Some(s.clone()),
        _ => None,
    }
}

/// Image `object` cleaned with `settings`: the new stream (and, for a halftone scan, how it was
/// stored), `None` when nothing changes, or why it can't be cleaned.
fn clean_image(
    cos: &pdfcraft_cos::Document,
    object: ObjRef,
    settings: ScanCleanSettings,
    budget: &mut pdfcraft_cos::ImageBudget,
) -> Result<Option<(Stream, Option<String>)>, String> {
    let obj = cos.get(object);
    let Object::Stream(s) = &*obj else { return Err("it is not an image".into()) };
    if s.dict.name(b"Subtype") != Some(b"Image") {
        return Err("it is not an image".into());
    }
    // A colour-key mask names sample values to leave unpainted: cleaned samples would match others.
    if matches!(s.dict.get(b"Mask").map(|m| cos.resolve(m)).as_deref(), Some(Object::Array(_))) {
        return Err("it has a colour-key mask, which cleaning would change".into());
    }
    // A layered (mixed-raster) scan draws its ink as a picture of the ink's colours through a mask
    // of the text's shapes: a stencil `/Mask`, or a one-bit `/SMask`. Those colours are no paper,
    // and background removal would lighten the text.
    let mask = |key: &[u8]| s.dict.get(key).map(|m| cos.resolve(m));
    let one_bit = |m: &Object| matches!(m, Object::Stream(m) if m.dict.int(b"BitsPerComponent") == Some(1));
    if matches!(mask(b"Mask").as_deref(), Some(Object::Stream(_))) || mask(b"SMask").as_deref().is_some_and(one_bit) {
        return Err("it is drawn through a stencil mask: it holds the ink colours of a layered scan, not its paper".into());
    }
    let samples = optimize::decode_samples(cos, s, budget).map_err(|e| e.to_string())?;
    if samples.bilevel && settings.restore_halftones && descreen::is_halftone(&samples.data, samples.width, samples.height) {
        let format = storage(s);
        return descreened(s, samples, settings.halftone).map(|stream| Some((stream, Some(format))));
    }
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
    optimize::encode_samples(s, &cleaned, QUALITY).map(|stream| Some((stream, None))).ok_or_else(|| "it could not be written back".into())
}

/// How image `s` is stored, in a few words, for the report.
fn storage(s: &Stream) -> String {
    let codec = match s.filters().last().map(|(f, _)| f.name()) {
        Some("CCITTFaxDecode") => "CCITT fax",
        Some("JBIG2Decode") => "JBIG2",
        Some("FlateDecode") => "Flate",
        Some(_) => "compressed",
        None => "uncompressed",
    };
    format!("{codec} 1-bit halftone")
}

/// A halftone (dithered) 1-bit scan turned into the clean grey picture it was made from
/// ([`descreen::restore`]: smoothed tones, then bolder, smoother text), stored as 8-bit grey.
fn descreened(old: &Stream, samples: optimize::Samples, tuning: HalftoneTuning) -> Result<Stream, String> {
    let data = descreen::restore(&samples.data, samples.width, samples.height, tuning)
        .ok_or("its samples don't match its size, or it is too large to restore")?;
    let grey = optimize::Samples { data, components: 1, jpeg: false, bilevel: false, inverted: false, ..samples };
    let mut stream = optimize::encode_samples(old, &grey, QUALITY).ok_or("it could not be written back")?;
    // The grey picture is drawn as it is: a decode array meant for the 1-bit data would invert it.
    stream.dict.remove(b"Decode");
    Ok(stream)
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
