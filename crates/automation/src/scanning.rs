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

        let created_bytes = self
            .session
            .create_from_images_with_resolution(&images, image_res)
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        if a.get("doc").is_some() {
            let page_count = self.doc(a)?.info.pages.len();
            self.apply(
                a,
                pdfcraft_engine::Edit::InsertPagesFrom {
                    name: "Scan.pdf".to_string(),
                    bytes: created_bytes,
                    pages: None,
                    at: page_count,
                    field_conflicts: pdfcraft_organize::FieldConflicts::Rename,
                },
            )?;
            return Ok(json!({ "page_inserted": page_count + 1 }));
        }

        if let Some(out_str) = a.opt_str("out")? {
            let out_path = self.resolve(out_str, true)?;
            write_atomic(&out_path, &created_bytes)?;
            Ok(json!({ "saved": out_path.to_string_lossy(), "pages": 1 }))
        } else {
            let name = a.opt_str("name")?.map(str::to_owned).unwrap_or_else(|| "Scan.pdf".to_string());
            let id = self.session.open_new(name, created_bytes).map_err(failed)?;
            Ok(json!({ "doc": id.0, "pages": 1 }))
        }
    }
}

