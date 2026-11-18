//! Explicit, read-only word rereading. No automatic selection, correction or ensemble.
use std::sync::Arc;

use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest};

use super::{EngineChoice, OcrJob, PlacedWord, Recognizers, WordCandidate, clamp_region, crop, validate_model_profile};

/// Refuse a partial or degenerate word instead of silently reading different text.
fn view_bounds(info: &PageInfo, word: &PlacedWord) -> Result<[f32; 4], String> {
    if word.origin.iter().chain(&word.across).chain(&word.up).any(|v| !v.is_finite()) {
        return Err("the OCR word has non-finite geometry".into());
    }
    let area = word.across[0] * word.up[1] - word.across[1] * word.up[0];
    if !area.is_finite() || area.abs() <= f64::EPSILON {
        return Err("the OCR word has empty geometry".into());
    }
    let mut bounds = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for (a, u) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let p = info.user_to_view(
            (word.origin[0] + a * word.across[0] + u * word.up[0]) as f32,
            (word.origin[1] + a * word.across[1] + u * word.up[1]) as f32,
        );
        if p.iter().any(|v| !v.is_finite()) {
            return Err("the OCR word has non-finite displayed geometry".into());
        }
        bounds[0] = bounds[0].min(p[0]);
        bounds[1] = bounds[1].min(p[1]);
        bounds[2] = bounds[2].max(p[0]);
        bounds[3] = bounds[3].max(p[1]);
    }
    if bounds[0] < 0.0 || bounds[1] < 0.0 || bounds[2] > info.width || bounds[3] > info.height || bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
        return Err("the full OCR word must be inside the displayed page".into());
    }
    Ok(bounds)
}

impl OcrJob {
    /// Propose alternatives for one explicitly selected original word. The caller checks
    /// this job's guard; no word, confidence, geometry, settings or review state is changed.
    pub fn word_candidates(&self, page: usize, word: &PlacedWord, recognizers: &Recognizers) -> Result<Vec<WordCandidate>, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.word_candidates_inner(page, word, recognizers)))
            .unwrap_or_else(|_| Err("word rereading failed; the original review is unchanged".into()))
    }

    fn word_candidates_inner(&self, page: usize, word: &PlacedWord, recognizers: &Recognizers) -> Result<Vec<WordCandidate>, String> {
        if !self.pages.contains(&page) {
            return Err("page is not in this OCR job".into());
        }
        if self.settings.engine == EngineChoice::Ocrs {
            return Err("the selected OCR engine does not support word candidates".into());
        }
        validate_model_profile(&self.settings)?;
        if !self.settings.dpi.is_finite() {
            return Err("OCR resolution must be finite".into());
        }
        let info = self.infos.get(page).ok_or("page is not in this OCR job")?;
        if !info.width.is_finite() || !info.height.is_finite() || info.width <= 0.0 || info.height <= 0.0 {
            return Err("the page has invalid displayed dimensions".into());
        }
        let bounds = view_bounds(info, word)?;
        let config = RenderConfig {
            password: self.password.as_deref().map(Arc::from),
            hide_comments: self.settings.native_text_only,
            // Export-style rasterization excludes runtime system fonts.
            system_fonts: false,
            reject_oversize: true,
            ..Default::default()
        };
        let mut renderer = PageRenderer::new(self.bytes.clone(), config);
        let shot = renderer.render(RenderRequest { page, scale: self.render_scale(page), ..Default::default() });
        if let Some(error) = shot.error {
            return Err(error);
        }
        let sx = shot.width as f32 / info.width;
        let sy = shot.height as f32 / info.height;
        let pixel = [bounds[0] * sx, bounds[1] * sy, bounds[2] * sx, bounds[3] * sy];
        let allowed = match self.settings.region {
            Some(region) => clamp_region(region, shot.width, shot.height).ok_or("the selected region is outside the page")?,
            None => [0, 0, shot.width, shot.height],
        };
        if pixel[0] < allowed[0] as f32 || pixel[1] < allowed[1] as f32 || pixel[2] > allowed[2] as f32 || pixel[3] > allowed[3] as f32 {
            return Err("the full OCR word must be inside the original recognition region".into());
        }
        // Actual raster-pixel margins, restricted to the captured region so a region review
        // cannot read neighbouring content. No preprocess rerun or model override.
        let [x0, y0, x1, y1] = clamp_region(
            [
                (pixel[0] - 8.0).max(allowed[0] as f32),
                (pixel[1] - 10.0).max(allowed[1] as f32),
                (pixel[2] + 8.0).min(allowed[2] as f32),
                (pixel[3] + 10.0).min(allowed[3] as f32),
            ],
            shot.width,
            shot.height,
        )
        .ok_or("the OCR word crop is empty")?;
        if x1 - x0 > 2000 || y1 - y0 > 2000 || u64::from(x1 - x0) * u64::from(y1 - y0) > 4_000_000 {
            return Err("the OCR word crop exceeds 2000 pixels per side or 4 million pixels".into());
        }
        let full = pdfcraft_ocr::OcrImage::new(shot.width, shot.height, shot.rgba.to_vec()).map_err(|e| e.to_string())?;
        let image = crop(&full, x0, y0, x1, y1).ok_or("the OCR word crop is invalid")?;
        let options = pdfcraft_ocr::RecognizeOptions { language: self.settings.language.clone(), model_profile: self.settings.model_profile };
        recognizers.primary.word_candidates(&image, &options).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::super::OcrSettings;
    use super::*;
    use crate::{Edit, Session};
    use pdfcraft_ocr::{ModelProfile, OcrError, Recognizer};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Reader {
        calls: AtomicUsize,
        expected: (u32, u32),
    }

    impl Recognizer for Reader {
        fn id(&self) -> &'static str {
            "fixture"
        }
        fn available(&self) -> bool {
            true
        }
        fn languages(&self) -> Vec<(String, String)> {
            vec![("en".into(), "English".into())]
        }
        fn recognize(&self, _: &pdfcraft_ocr::OcrImage, _: &pdfcraft_ocr::RecognizeOptions) -> Result<Vec<pdfcraft_ocr::Line>, OcrError> {
            panic!("manual candidates must not call normal recognition")
        }
        fn word_candidates(&self, image: &pdfcraft_ocr::OcrImage, options: &pdfcraft_ocr::RecognizeOptions) -> Result<Vec<WordCandidate>, OcrError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!((image.width(), image.height()), self.expected);
            assert_eq!(options.language, "en");
            assert_eq!(options.model_profile, ModelProfile::Standard);
            Ok(vec![WordCandidate { text: "ÜÛ".into(), confidence: Some(94.0), padding: 8, source: "fixture".into() }])
        }
    }

    fn word(info: &PageInfo, bounds: [f32; 4]) -> PlacedWord {
        let [x0, y0, x1, y1] = bounds;
        let o = info.view_to_user(x0, y1);
        let br = info.view_to_user(x1, y1);
        let tl = info.view_to_user(x0, y0);
        PlacedWord {
            text: "ÜÛ".into(),
            confidence: Some(32.5),
            source: "fixture".into(),
            origin: o.map(f64::from),
            across: [f64::from(br[0] - o[0]), f64::from(br[1] - o[1])],
            up: [f64::from(tl[0] - o[0]), f64::from(tl[1] - o[1])],
        }
    }

    #[test]
    fn manual_word_candidates_bounds_include_rotated_cropbox_origin() {
        for rotation in [0, 90, 180, 270] {
            let info = PageInfo { width: 256.0, height: 256.0, label: "1".into(), crop: [100.0, 200.0, 356.0, 456.0], rotation };
            let selected = word(&info, [40.0, 50.0, 80.0, 70.0]);
            assert_eq!(view_bounds(&info, &selected).unwrap(), [40.0, 50.0, 80.0, 70.0]);
            assert!(view_bounds(&info, &word(&info, [-1.0, 50.0, 80.0, 70.0])).is_err());
            let mut empty = selected.clone();
            empty.up = [0.0, 0.0];
            assert!(view_bounds(&info, &empty).is_err());
            let mut enormous = selected;
            enormous.across = [f64::MAX, f64::MAX];
            assert!(view_bounds(&info, &enormous).is_err());
        }
    }

    #[test]
    fn manual_word_candidates_use_rotated_view_crop_and_leave_original_untouched() {
        for rotation in [0, 90, 180, 270] {
            let mut session = Session::new();
            let bytes = session.create_blank(256.0, 256.0, 1).unwrap();
            let id = session.open("fixture.pdf", None, bytes, None).unwrap();
            if rotation != 0 {
                session.apply(id, Edit::RotatePages { pages: vec![0], degrees: rotation }).unwrap();
            }
            let settings = OcrSettings { engine: EngineChoice::Tesseract, dpi: 72.0, ..Default::default() };
            let job = session.ocr_job(id, &[0], settings.clone()).unwrap();
            let selected = word(&job.infos[0], [40.0, 50.0, 80.0, 70.0]);
            let original = selected.clone();
            let reader = Arc::new(Reader { calls: AtomicUsize::new(0), expected: (56, 40) });
            let secondary = Arc::new(Reader { calls: AtomicUsize::new(0), expected: (1, 1) });
            let readers = Recognizers { primary: reader.clone(), secondary: Some(secondary.clone()) };
            let proposals = job.word_candidates(0, &selected, &readers).unwrap();
            assert_eq!(proposals[0].text, "ÜÛ");
            assert_eq!(selected, original);
            assert_eq!(job.settings, settings);
            assert_eq!(reader.calls.load(Ordering::SeqCst), 1);
            assert_eq!(secondary.calls.load(Ordering::SeqCst), 0);
            assert!(session.stale_reason(&job.guard).is_none());
        }
    }

    #[test]
    fn manual_word_candidates_honor_region_and_reject_partial_invalid_words() {
        let mut session = Session::new();
        let id = session.open("fixture.pdf", None, session.create_blank(256.0, 256.0, 1).unwrap(), None).unwrap();
        let settings = OcrSettings { engine: EngineChoice::Tesseract, dpi: 72.0, region: Some([40.0, 50.0, 80.0, 70.0]), ..Default::default() };
        let job = session.ocr_job(id, &[0], settings).unwrap();
        let selected = word(&job.infos[0], [40.0, 50.0, 80.0, 70.0]);
        let reader = Arc::new(Reader { calls: AtomicUsize::new(0), expected: (40, 20) });
        let readers = Recognizers { primary: reader.clone(), secondary: None };
        assert!(job.word_candidates(0, &selected, &readers).is_ok());
        assert!(job.word_candidates(1, &selected, &readers).is_err());
        assert!(job.word_candidates(0, &word(&job.infos[0], [39.0, 50.0, 80.0, 70.0]), &readers).is_err());
        let mut invalid = selected.clone();
        invalid.origin[0] = f64::NAN;
        assert!(job.word_candidates(0, &invalid, &readers).is_err());
        assert_eq!(reader.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn manual_word_candidates_do_not_use_an_unsupported_engine_or_fallback() {
        let mut session = Session::new();
        let id = session.open("fixture.pdf", None, session.create_blank(256.0, 256.0, 1).unwrap(), None).unwrap();
        let job = session.ocr_job(id, &[0], OcrSettings { engine: EngineChoice::Ocrs, ..Default::default() }).unwrap();
        let selected = word(&job.infos[0], [40.0, 50.0, 80.0, 70.0]);
        let reader = Arc::new(Reader { calls: AtomicUsize::new(0), expected: (1, 1) });
        let readers = Recognizers { primary: reader.clone(), secondary: Some(reader.clone()) };
        assert!(job.word_candidates(0, &selected, &readers).unwrap_err().contains("does not support"));
        assert_eq!(reader.calls.load(Ordering::SeqCst), 0);
    }
}
