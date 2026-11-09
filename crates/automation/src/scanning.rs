//! Scanner automation tools: list scanners, and scan documents into PDF.

use serde_json::{Value, json};
use pdfcraft_platform::scanner::{self, ScanColorMode, ScanOptions, ScanResolution};
use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

impl Automation {
    /// List available scanner hardware (physical and network scanners).
    pub(crate) fn scanners(&self) -> Result<Value> {
        let list = scanner::list_scanners().map_err(|e| ToolError::Failed(e.to_string()))?;
        let devices = list
            .into_iter()
            .map(|d| json!({ "id": d.id, "name": d.name }))
            .collect::<Vec<_>>();
        Ok(json!({ "scanners": devices }))
    }

    /// Scan a document from a scanner into a new or active PDF document.
    pub(crate) fn doc_scan(&mut self, a: &Args) -> Result<Value> {
        let device_id = a.opt_str("device")?.map(str::to_owned);
        let color_mode = match a.opt_str("color")? {
            Some("gray" | "grayscale") => ScanColorMode::Grayscale,
            Some("bw" | "black_and_white" | "lineart") => ScanColorMode::BlackAndWhite,
            _ => ScanColorMode::Color,
        };
        let resolution = match a.opt_int("dpi")? {
            Some(75) => ScanResolution::Dpi75,
            Some(150) => ScanResolution::Dpi150,
            Some(600) => ScanResolution::Dpi600,
            _ => ScanResolution::Dpi300,
        };
        let use_native_ui = a.opt_bool("ui")?.unwrap_or(false);

        let options = ScanOptions {
            device_id,
            color_mode,
            resolution,
            use_native_ui,
        };

        let image_bytes = scanner::acquire_scan(&options)
            .map_err(|e| ToolError::Failed(format!("Scanner acquisition failed: {e}")))?;

        let dpi_f64 = resolution.as_dpi() as f64;
        let images = vec![("Scan.png".to_string(), image_bytes)];
        let image_res = pdfcraft_engine::ImageResolution::Dpi(dpi_f64);

        if let Some(target_doc_id) = a.opt_int("doc")? {
            if target_doc_id < 0 {
                return Err(ToolError::InvalidArgs("doc must be a positive document id".into()));
            }
            let doc_id = pdfcraft_engine::DocId::from_u64(target_doc_id as u64);
            let new_doc_id = self
                .session
                .create_from_images_with_resolution(&images, image_res)
                .map_err(|e| ToolError::Failed(e.to_string()))?;

            let page_count = self.session.get(doc_id).map(|d| d.info.pages.len()).unwrap_or(0);
            let _ = self.session.edit(
                doc_id,
                pdfcraft_engine::Edit::InsertPages {
                    at: page_count,
                    from_doc: new_doc_id,
                    pages: vec![0],
                },
            );
            let _ = self.session.close(new_doc_id);
            return Ok(json!({ "doc": target_doc_id, "page_inserted": page_count + 1 }));
        }

        let new_doc_id = self
            .session
            .create_from_images_with_resolution(&images, image_res)
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        if let Some(out_str) = a.opt_str("out")? {
            let out_path = self.resolve(out_str, true)?;
            let bytes = self
                .session
                .save_to_bytes(new_doc_id)
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            write_atomic(&out_path, &bytes)?;
            let _ = self.session.close(new_doc_id);
            Ok(json!({ "saved": out_path.to_string_lossy(), "pages": 1 }))
        } else {
            Ok(json!({ "doc": new_doc_id.as_u64(), "pages": 1 }))
        }
    }
}

