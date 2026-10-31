//! Optimize PDF ▸ Advanced optimization as a job that reports its progress.
//!
//! Optimizing a large file takes a while (every image is decoded, resampled and re-encoded), so
//! it is split like OCR: [`Session::optimize_job`] captures a copy of the document and the
//! choices, and [`OptimizeJob::run`] works anywhere (the UI runs it on a worker thread),
//! reporting each [`OptimizeStage`] and stopping when asked to.

use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_full};

use crate::{DocId, EditError, Hidden, OptimizeReport, Session, guard, optimize};

/// Where an optimization is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptimizeStage {
    /// Removing the user data chosen under Discard User Data.
    #[default]
    Discarding,
    /// Merging the fragments of a page picture into one image.
    Fragments,
    /// Flattening tiled presentation slide image grids.
    Flattening { done: usize, total: usize },
    /// About to process image `done` (0-based) of `total`.
    Images { done: usize, total: usize },
    /// Running search trials to meet a target file size.
    Trial { step: usize, total: usize },
    /// Discarding objects and compressing unencoded streams.
    CleaningUp,
    /// Merging identical fonts and images.
    Merging,
    /// Writing the compressed copy.
    Writing,
}

impl OptimizeStage {
    /// How far along the whole run is, from 0 to 1. Images take most of the time; the rest are
    /// small fixed steps.
    pub fn fraction(self) -> f32 {
        match self {
            OptimizeStage::Discarding => 0.0,
            OptimizeStage::Fragments => 0.02,
            OptimizeStage::Flattening { done, total } => 0.02 + 0.06 * if total == 0 { 1.0 } else { done.min(total) as f32 / total as f32 },
            OptimizeStage::Images { done, total } => 0.08 + 0.72 * if total == 0 { 1.0 } else { done.min(total) as f32 / total as f32 },
            // A target search runs the whole optimization per rung, so it is the bulk of the work,
            // not a tail: it spans the image band, and the rungs are its steps.
            OptimizeStage::Trial { step, total } => 0.08 + 0.72 * if total == 0 { 1.0 } else { step.min(total) as f32 / total as f32 },
            OptimizeStage::CleaningUp => 0.85,
            OptimizeStage::Merging => 0.90,
            OptimizeStage::Writing => 0.95,
        }
    }
}

/// The optimized copy's bytes and what was done.
pub type Optimized = (Arc<Vec<u8>>, OptimizeReport);

/// What an optimization needs, independent of the session.
pub struct OptimizeJob {
    cos: pdfcraft_cos::Document,
    settings: optimize::Settings,
    discard: Vec<Hidden>,
    opts: SaveOptions,
}

impl Session {
    /// Capture what optimizing document `id` with `settings` and the Remove Hidden Information
    /// `discard` categories needs. A full rewrite: signed documents are refused.
    pub fn optimize_job(&self, id: DocId, settings: &optimize::Settings, discard: &[Hidden]) -> Result<OptimizeJob, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if doc.is_signed() {
            return Err(EditError::Signed);
        }
        let editor = doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        Ok(OptimizeJob {
            cos: editor.cos.clone(),
            settings: settings.clone(),
            discard: discard.to_vec(),
            opts: SaveOptions {
                mod_date: self.now().map(pdfcraft_cos::pdf_date),
                object_streams: settings.effective_object_streams(),
                ..SaveOptions::default()
            },
        })
    }
}

impl OptimizeJob {
    /// Optimize and write the copy. `progress` is called as each stage starts and before each
    /// image; returning `false` stops with [`EditError::Cancelled`]. A panic is returned as an
    /// error, so a worker thread always reports back.
    pub fn run(self, mut progress: impl FnMut(OptimizeStage) -> bool) -> Result<Optimized, EditError> {
        guard(move || self.work(&mut progress)).map_err(EditError::Optimize)?
    }

    /// Performs the optimization workflow on `self.cos`, reporting progress through `progress`.
    fn work(self, progress: &mut dyn FnMut(OptimizeStage) -> bool) -> Result<Optimized, EditError> {
        let OptimizeJob { mut cos, settings, discard, opts } = self;
        // Like every file written, without the values of Password-flag fields (#3926, #3944).
        pdfcraft_forms::clear_password_values(&mut cos).map_err(|e| EditError::Write(e.to_string()))?;
        pdfcraft_xfa::clear_password_values(&mut cos).map_err(|e| EditError::Write(e.to_string()))?;
        let mut step = |stage| if progress(stage) { Ok(()) } else { Err(EditError::Cancelled) };
        step(OptimizeStage::Discarding)?;
        let discarded = if discard.is_empty() { Vec::new() } else { pdfcraft_redact::sanitize::remove_hidden(&mut cos, &discard)? };
        let mut active_trial: Option<(usize, usize)> = None;
        let report = optimize::optimize_with_progress(&mut cos, &settings, &mut |s| {
            let stage = match (s, active_trial) {
                (optimize::Stage::Trial { n, of }, _) => {
                    active_trial = Some((n, of));
                    OptimizeStage::Trial { step: n, total: of }
                }
                (_, Some((step, total))) => OptimizeStage::Trial { step, total },
                (optimize::Stage::Fragments, None) => OptimizeStage::Fragments,
                (optimize::Stage::Flatten { done, total }, None) => OptimizeStage::Flattening { done, total },
                (optimize::Stage::Images { done, total }, None) => OptimizeStage::Images { done, total },
                (optimize::Stage::CleanUp, None) => OptimizeStage::CleaningUp,
            };
            step(stage).is_ok()
        })
        .map_err(|e| match e {
            optimize::OptimizeError::Cancelled => EditError::Cancelled,
            e => EditError::Optimize(e.to_string()),
        })?;
        step(OptimizeStage::Merging)?;
        let all: Vec<pdfcraft_cos::ObjRef> = cos.object_numbers().into_iter().map(|n| pdfcraft_cos::ObjRef::new(n, cos.generation(n))).collect();
        let merged = pdfcraft_organize::dedupe_resources(&mut cos, &all, false);
        step(OptimizeStage::Writing)?;
        let bytes = write_full(&cos, &opts).map_err(|e| EditError::Write(e.to_string()))?;
        Ok((Arc::new(bytes), OptimizeReport { optimize: report, merged, discarded }))
    }
}
