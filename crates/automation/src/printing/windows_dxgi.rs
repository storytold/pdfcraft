//! Explicit Windows.Data.Pdf/DXGI output; no application-side rasterization or GDI fallback.

use std::time::Duration;

use pdfcraft_engine::print::spool;
use serde_json::{Value, json};

use super::{Automation, Result, bad, failed, windows_output};

/// A headless caller cannot answer PORTPROMPT's Save dialog. Require its output filename
/// before entering the blocking package-target call, including for a resolved default queue.
pub(super) fn check_headless_destination(printer: &str, default: Option<&str>) -> Result<()> {
    let selected = if printer == "default" {
        default.ok_or_else(|| bad("the default printer could not be resolved; choose an explicit name from printers"))?
    } else {
        printer
    };
    if selected.eq_ignore_ascii_case("Microsoft Print to PDF") {
        return Err(bad(
            "headless Microsoft Print to PDF needs printer_output naming a new .pdf file and explicit as_image false (DXGI) or true (GDI)",
        ));
    }
    Ok(())
}

pub(super) fn print(
    a: &Automation,
    requested: &str,
    expected: windows_output::ExpectedSheets,
    bytes: Vec<u8>,
    job: &spool::Job,
    mut out: Value,
) -> Result<Value> {
    if !cfg!(windows) {
        return Err(failed("DXGI file output is available only on Windows"));
    }
    let mut submitted = None;
    let (path, length) = windows_output::stage_and_publish(a, requested, expected, Duration::from_secs(5), |stage| {
        submitted = Some(spool::submit_dxgi_to_file(&bytes, job, stage).map_err(|e| failed(format!("native DXGI output: {e}")))?);
        Ok(())
    })?;
    out["printer_output"] = json!(path.to_string_lossy());
    out["output_bytes"] = json!(length);
    out["backend"] = json!("windows-dxgi");
    out["as_image"] = json!(false);
    out["physical_print_verified"] = json!(false);
    // Windows and the selected driver decide which individual PDF elements stay vector or
    // become images. This route does not promise searchable text or a wholly vector output.
    if let Some(message) = submitted {
        out["job"] = json!(message);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_headless_pdf_queue_requires_an_explicit_output_even_when_default() {
        assert!(check_headless_destination("Microsoft Print to PDF", None).is_err());
        assert!(check_headless_destination("microsoft print to pdf", None).is_err());
        assert!(check_headless_destination("default", Some("Microsoft Print to PDF")).is_err());
        assert!(check_headless_destination("default", None).is_err());
        assert!(check_headless_destination("Office", None).is_ok());
        assert!(check_headless_destination("default", Some("Office")).is_ok());
    }
}
