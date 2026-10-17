//! Native FFDetr jobs. Detection is separated from editing so frontends can run a job on a
//! worker, cancel it, and apply the complete result atomically if its document is unchanged.

use std::collections::BTreeSet;
use std::sync::Arc;

use pdfcraft_forms::detect::Word;
use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, RequestKind};

pub use pdfcraft_form_detect::{Detector, Error as DetectionError, Kind, MODEL_FILE, model};

use crate::{DocId, Edit, EditError, FieldProps, NewField, Session};

const MAX_FIELDS: usize = 10_000;

#[derive(Clone, Debug)]
pub struct Field {
    pub page: usize,
    pub kind: Kind,
    pub name: String,
    /// PDF user-space points, bottom-left origin (the existing detection tool convention).
    pub rect: [f64; 4],
    pub confidence: f32,
    pub rotation: i64,
}

struct Page {
    index: usize,
    info: PageInfo,
    existing: Vec<[f64; 4]>,
}

pub struct Job {
    doc: DocId,
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    pages: Vec<Page>,
    names: BTreeSet<String>,
}

pub struct Detection {
    doc: DocId,
    bytes: Arc<Vec<u8>>,
    fields: Vec<Field>,
}

impl Detection {
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }
}

impl Job {
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Run without touching the session. Returning false from progress cancels the entire
    /// job; no incomplete result is returned. A progress callback runs between pages.
    pub fn run(&self, detector: &Detector, confidence: f32, mut progress: impl FnMut(usize, usize) -> bool) -> Result<Detection, EditError> {
        crate::guard(|| self.run_inner(confidence, &mut progress, |rgba, w, h| detector.detect(rgba, w, h, confidence)))
            .map_err(|e| EditError::Invalid(format!("form field detection failed: {e}")))?
    }

    fn run_inner(
        &self,
        confidence: f32,
        progress: &mut impl FnMut(usize, usize) -> bool,
        mut detect: impl FnMut(&[u8], u32, u32) -> Result<Vec<pdfcraft_form_detect::Proposal>, DetectionError>,
    ) -> Result<Detection, EditError> {
        pdfcraft_form_detect::validate_confidence(confidence).map_err(|e| EditError::Invalid(e.to_string()))?;
        let config = RenderConfig { password: self.password.as_deref().map(Arc::from), ..Default::default() };
        let mut renderer = PageRenderer::new(self.bytes.clone(), config);
        let mut fields = Vec::new();
        let mut used = self.names.clone();
        for (done, page) in self.pages.iter().enumerate() {
            if !progress(done, self.pages.len()) {
                return Err(EditError::Cancelled);
            }
            let info = &page.info;
            if !info.width.is_finite() || !info.height.is_finite() || info.width <= 0.0 || info.height <= 0.0 {
                return Err(EditError::Invalid(format!("page {} has invalid dimensions", page.index + 1)));
            }
            let scale = 2.0f32.min(3000.0 / info.width.max(info.height));
            let image = renderer.render(RenderRequest { page: page.index, scale, ..Default::default() });
            if let Some(error) = image.error {
                return Err(EditError::Invalid(format!("cannot render page {} for field detection: {error}", page.index + 1)));
            }
            let mut proposals =
                detect(&image.rgba, image.width, image.height).map_err(|e| EditError::Invalid(format!("page {}: {e}", page.index + 1)))?;
            let text = renderer.render(RenderRequest { page: page.index, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            if let Some(error) = text.error {
                return Err(EditError::Invalid(format!("cannot read page {} labels: {error}", page.index + 1)));
            }
            // Name in displayed coordinates: a turned page's labels are still left/above.
            let words: Vec<Word> = text
                .text
                .as_deref()
                .map(|t| crate::js::words_with(t, |x, y| [x, info.height - y]))
                .unwrap_or_default()
                .into_iter()
                .map(|(text, rect)| Word { text, rect: rect.map(f64::from) })
                .collect();
            proposals.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]).then(a.rect[0].total_cmp(&b.rect[0])));
            for proposal in proposals {
                let Some(rect) = user_rect(info, proposal.rect) else {
                    return Err(EditError::Invalid("detector returned an invalid field rectangle".into()));
                };
                if page.existing.iter().any(|other| same_region(*other, rect)) {
                    continue;
                }
                let [left, top, right, bottom] = proposal.rect.map(f64::from);
                let display = [
                    left * f64::from(info.width),
                    (1.0 - bottom) * f64::from(info.height),
                    right * f64::from(info.width),
                    (1.0 - top) * f64::from(info.height),
                ];
                let label = pdfcraft_forms::detect::label_for(&words, display, proposal.kind == Kind::CheckBox);
                let base = if label.is_empty() {
                    match proposal.kind {
                        Kind::Text => "Text",
                        Kind::CheckBox => "Check Box",
                        Kind::Signature => "Signature",
                    }
                    .to_string()
                } else {
                    label.clone()
                };
                let mut n = 1usize;
                let mut name = if label.is_empty() { format!("{base}1") } else { base.clone() };
                while used.contains(&name) {
                    n = n.checked_add(1).ok_or_else(|| EditError::Invalid("too many repeated field names".into()))?;
                    name = format!("{base}{n}");
                }
                used.insert(name.clone());
                if fields.len() >= MAX_FIELDS {
                    return Err(EditError::Invalid("field detection is limited to 10000 fields per operation; select fewer pages".into()));
                }
                fields.push(Field {
                    page: page.index,
                    kind: proposal.kind,
                    name,
                    rect,
                    confidence: proposal.confidence,
                    rotation: i64::from(info.rotation % 360),
                });
            }
        }
        if !progress(self.pages.len(), self.pages.len()) {
            return Err(EditError::Cancelled);
        }
        Ok(Detection { doc: self.doc, bytes: self.bytes.clone(), fields })
    }
}

fn user_rect(info: &PageInfo, normalized: [f32; 4]) -> Option<[f64; 4]> {
    let [left, top, right, bottom] = normalized;
    if normalized.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) || left >= right || top >= bottom {
        return None;
    }
    let points = [[left, top], [right, top], [right, bottom], [left, bottom]].map(|[x, y]| info.view_to_user(x * info.width, y * info.height));
    if points.iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    Some([
        points.iter().map(|p| f64::from(p[0])).fold(f64::INFINITY, f64::min),
        points.iter().map(|p| f64::from(p[1])).fold(f64::INFINITY, f64::min),
        points.iter().map(|p| f64::from(p[0])).fold(f64::NEG_INFINITY, f64::max),
        points.iter().map(|p| f64::from(p[1])).fold(f64::NEG_INFINITY, f64::max),
    ])
}

fn same_region(a: [f64; 4], b: [f64; 4]) -> bool {
    let area = |r: [f64; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let intersection = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0) * (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let smaller = area(a).min(area(b));
    smaller > 0.0 && intersection / smaller >= 0.5
}

impl Session {
    pub fn form_detection_job(&self, id: DocId, pages: &[usize]) -> Result<Job, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let all: Vec<usize> = if pages.is_empty() { (0..doc.info.pages.len()).collect() } else { pages.to_vec() };
        let mut seen = BTreeSet::new();
        let mut selected = Vec::new();
        for page in all {
            let info = doc.info.pages.get(page).ok_or_else(|| EditError::Invalid(format!("page {} does not exist", page.saturating_add(1))))?;
            if !seen.insert(page) {
                continue;
            }
            let existing = doc.form.iter().flat_map(|f| &f.widgets).filter(|w| w.page == Some(page)).map(|w| w.rect).collect();
            selected.push(Page { index: page, info: info.clone(), existing });
        }
        Ok(Job {
            doc: id,
            bytes: doc.bytes.clone(),
            password: doc.password.clone(),
            pages: selected,
            names: doc.form.iter().map(|f| f.name.clone()).collect(),
        })
    }

    /// Apply exactly the proposals returned by the job, without running inference again.
    /// A changed/closed document or any failed edit leaves the working document intact.
    pub fn apply_detected_fields(&mut self, found: Detection) -> Result<Vec<String>, EditError> {
        let doc = self.get(found.doc).ok_or(EditError::NoDocument)?;
        if !Arc::ptr_eq(&doc.bytes, &found.bytes) {
            return Err(EditError::Invalid("the document changed during field detection; run detection again".into()));
        }
        if found.fields.is_empty() {
            return Ok(Vec::new());
        }
        let mut edits = Vec::new();
        let mut names = Vec::new();
        for field in found.fields {
            let tall = if field.rotation % 180 == 0 { field.rect[3] - field.rect[1] } else { field.rect[2] - field.rect[0] };
            let kind = match field.kind {
                Kind::Text => NewField::Text { multiline: tall > 30.0 },
                Kind::CheckBox => NewField::CheckBox,
                Kind::Signature => NewField::Signature,
            };
            edits.push(Edit::AddField { page: field.page, rect: field.rect, kind, name: Some(field.name.clone()) });
            edits.push(Edit::SetFieldProps {
                name: field.name.clone(),
                props: Box::new(FieldProps {
                    rotation: (field.rotation != 0).then_some((0, field.rotation)),
                    rect: Some((0, field.rect)),
                    appearance: Some(pdfcraft_forms::LookPatch { border: Some(None), fill: Some(None), ..Default::default() }),
                    ..Default::default()
                }),
            });
            names.push(field.name);
        }
        self.apply(found.doc, Edit::Batch { label: "Detect form fields".into(), edits })?;
        Ok(names)
    }
}

#[cfg(test)]
mod tests;
