//! Serializable Hot Folder choices for the existing PDF optimizer.
//! The optimizer itself remains shared with desktop and automation.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Off,
    Lossless,
    Reduce,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    Retain,
    Zip,
    #[default]
    Jpeg,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Images {
    pub downsample: bool,
    pub ppi: f64,
    pub above_ppi: f64,
    pub compression: Compression,
    pub quality: u8,
}

impl Default for Images {
    fn default() -> Self {
        Self { downsample: true, ppi: 150.0, above_ppi: 225.0, compression: Compression::Jpeg, quality: 60 }
    }
}

impl Images {
    pub fn validate(&self) -> Result<(), String> {
        if !self.ppi.is_finite()
            || !(9.0..=2400.0).contains(&self.ppi)
            || !self.above_ppi.is_finite()
            || !(9.0..=2400.0).contains(&self.above_ppi)
            || self.above_ppi < self.ppi
            || !(1..=100).contains(&self.quality)
        {
            return Err("PDF optimization needs 9..2400 ppi, a threshold at least as large as the target, and JPEG quality 1..100".into());
        }
        Ok(())
    }

    fn settings(self) -> crate::optimize::ImageSettings {
        crate::optimize::ImageSettings {
            downsample: self.downsample,
            target_ppi: self.ppi,
            above_ppi: self.above_ppi,
            compression: match self.compression {
                Compression::Retain => crate::optimize::Compression::Retain,
                Compression::Zip => crate::optimize::Compression::Flate,
                Compression::Jpeg => crate::optimize::Compression::Jpeg(self.quality),
            },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub mode: Mode,
    pub color: Images,
    pub gray: Images,
}

impl Settings {
    pub fn enabled(&self) -> bool {
        self.mode != Mode::Off
    }

    pub fn validate(&self) -> Result<(), String> {
        self.color.validate()?;
        self.gray.validate()
    }

    fn settings(&self) -> crate::optimize::Settings {
        match self.mode {
            Mode::Reduce => crate::optimize::Settings::default(),
            Mode::Custom => {
                crate::optimize::Settings { color: self.color.settings(), gray: self.gray.settings(), ..crate::optimize::Settings::lossless() }
            }
            Mode::Off | Mode::Lossless => crate::optimize::Settings::lossless(),
        }
    }

    pub(crate) fn apply(
        &self,
        bytes: Arc<Vec<u8>>,
        timestamp: i64,
        should_stop: &dyn Fn() -> bool,
    ) -> Result<(Arc<Vec<u8>>, Option<Report>), String> {
        self.validate()?;
        if should_stop() {
            return Err("PDF optimization stopped; originals retained".into());
        }
        if !self.enabled() {
            return Ok((bytes, None));
        }
        let mut session = crate::Session::new().with_timestamp(timestamp);
        let id = session.open("optimization.pdf", None, bytes.clone(), None).map_err(|error| error.to_string())?;
        let (optimized, report) = session
            .optimize_job(id, &self.settings(), &[])
            .map_err(|error| error.to_string())?
            .run(|_| !should_stop())
            .map_err(|error| error.to_string())?;
        if should_stop() {
            return Err("PDF optimization stopped; originals retained".into());
        }
        let applied = optimized.len() < bytes.len();
        let before = bytes.len() as u64;
        let output = if applied { optimized } else { bytes };
        let report = Report {
            path: PathBuf::new(),
            bytes_before: before,
            bytes_after: output.len() as u64,
            applied,
            images_resampled: if applied { report.optimize.images_resampled } else { 0 },
            images_recompressed: if applied { report.optimize.images_recompressed } else { 0 },
        };
        Ok((output, Some(report)))
    }
}

/// Check before any grouping, rasterization, OCR or page selection can remove signature fields.
pub(crate) fn reject_signed(bytes: Arc<Vec<u8>>) -> Result<(), String> {
    let mut session = crate::Session::new();
    let id = session.open("optimization source.pdf", None, bytes, None).map_err(|error| error.to_string())?;
    if session.get(id).ok_or("optimization source unavailable")?.is_signed() {
        return Err("signed PDFs cannot be optimized; originals retained".into());
    }
    Ok(())
}

/// Actual published sizes, including a no-savings result that retained its original PDF bytes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub path: PathBuf,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub applied: bool,
    pub images_resampled: usize,
    pub images_recompressed: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub documents: u64,
    pub changed: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl Summary {
    pub fn from_reports(reports: &[Report]) -> Option<Self> {
        if reports.is_empty() {
            return None;
        }
        let mut summary = Self::default();
        for report in reports {
            summary.documents = summary.documents.saturating_add(1);
            summary.changed = summary.changed.saturating_add(u64::from(report.applied));
            summary.bytes_before = summary.bytes_before.saturating_add(report.bytes_before);
            summary.bytes_after = summary.bytes_after.saturating_add(report.bytes_after);
        }
        Some(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_full};

    fn padded_document() -> Arc<Vec<u8>> {
        let bytes = crate::Session::new().create_from_text("source", "Searchable words survive optimization").unwrap();
        let mut document = Document::open(bytes).unwrap();
        // An application extension stream is reachable, so cleanup must compress, not discard it.
        let extension = document.add(Object::Stream(Stream::from_raw(Dict::new(), vec![b'x'; 65_536])));
        document.update_dict(document.root().unwrap(), |catalog| catalog.set(b"PdfCraftTestExtension".to_vec(), Object::Ref(extension))).unwrap();
        Arc::new(write_full(&document, &SaveOptions::default()).unwrap())
    }

    #[test]
    fn default_off_is_byte_identical_and_invalid_custom_choices_are_rejected() {
        let bytes = padded_document();
        let settings: Settings = serde_json::from_str("{}").unwrap();
        let (output, report) = settings.apply(bytes.clone(), 1_700_000_000, &|| false).unwrap();
        assert!(Arc::ptr_eq(&output, &bytes));
        assert!(report.is_none());
        assert!(serde_json::from_str::<Settings>(r#"{"mode":"smaller"}"#).is_err());
        assert!(serde_json::from_str::<Settings>(r#"{"lossless":true}"#).is_err());
        for images in [
            Images { ppi: f64::NAN, ..Default::default() },
            Images { above_ppi: 149.0, ..Default::default() },
            Images { quality: 0, ..Default::default() },
            Images { ppi: 2401.0, ..Default::default() },
        ] {
            assert!(Settings { mode: Mode::Custom, color: images, ..Default::default() }.validate().is_err());
        }
    }

    #[test]
    fn lossless_shrinks_without_losing_text_or_reachable_extension_data_and_stabilizes() {
        let source = padded_document();
        let settings = Settings { mode: Mode::Lossless, ..Default::default() };
        let (output, report) = settings.apply(source.clone(), 1_700_000_000, &|| false).unwrap();
        let report = report.unwrap();
        assert!(report.applied && output.len() < source.len());
        assert_eq!((report.bytes_before, report.bytes_after), (source.len() as u64, output.len() as u64));
        assert_eq!((report.images_resampled, report.images_recompressed), (0, 0));
        let mut session = crate::Session::new();
        let id = session.open("result", None, output.clone(), None).unwrap();
        assert!(crate::export::Exporter::new(session.get(id).unwrap()).text(0).unwrap().contains("Searchable words survive"));
        let document = Document::open(output.clone()).unwrap();
        let catalog = document.get(document.root().unwrap());
        let extension = document.resolve(catalog.as_dict().unwrap().get(b"PdfCraftTestExtension").unwrap());
        let Object::Stream(stream) = extension.as_ref() else { panic!("extension stream was lost") };
        assert_eq!(stream.decoded().unwrap(), vec![b'x'; 65_536]);
        // Identical input/settings/time converge: an equal or larger rewrite retains the exact input.
        let (again, second) = settings.apply(output.clone(), 1_700_000_000, &|| false).unwrap();
        assert!(Arc::ptr_eq(&again, &output));
        assert!(!second.unwrap().applied);
    }

    #[test]
    fn optimization_cancels_inside_the_shared_job_without_returning_partial_bytes() {
        let bytes = padded_document();
        let settings = Settings { mode: Mode::Lossless, ..Default::default() };
        assert!(settings.apply(bytes.clone(), 1_700_000_000, &|| true).is_err());
        let calls = std::cell::Cell::new(0);
        let error = settings
            .apply(bytes, 1_700_000_000, &|| {
                calls.set(calls.get() + 1);
                calls.get() >= 3
            })
            .unwrap_err();
        assert!(error.to_lowercase().contains("cancel"), "{error}");
        assert_eq!(calls.get(), 3);
    }
}
