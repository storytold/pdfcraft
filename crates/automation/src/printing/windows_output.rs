//! Explicit single-sheet GDI output and validated publication shared with DXGI file output.
//! Neither route is a fallback for the other.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pdfcraft_engine::print::spool::rgb;
use pdfcraft_engine::print_raster::native_output::render_sheet;
use serde_json::{Value, json};

use super::{Args, Automation, Result, bad, failed};

const PRINTER: &str = "Microsoft Print to PDF";
const OUTPUT_LIMIT: u64 = 128 * 1024 * 1024;
const PAPER_TOLERANCE: f64 = 0.05;

/// A validated imposed-sheet count, carried from layout through native output publication.
/// The private field prevents publication from accepting an unchecked caller-supplied count.
#[derive(Clone, Copy, Debug)]
pub(super) struct ExpectedSheets(usize);

impl ExpectedSheets {
    pub(super) fn new(sheets: usize, as_image: bool) -> Result<Self> {
        if as_image && sheets != 1 {
            return Err(bad("GDI printer_output supports exactly one imposed sheet with explicit as_image true"));
        }
        if !as_image && !(1..=64).contains(&sheets) {
            return Err(bad("DXGI printer_output supports 1..64 imposed sheets with explicit as_image false"));
        }
        Ok(Self(sheets))
    }

    fn count(self) -> usize {
        self.0
    }
}

pub(super) struct Destination<'a> {
    pub(super) path: &'a str,
    pub(super) printer: &'a str,
}

pub(super) fn request<'a>(a: &'a Args<'_>) -> Result<Option<Destination<'a>>> {
    if a.0.get("printer_output").is_none() {
        return Ok(None);
    }
    let path = a.str("printer_output")?;
    let printer = a.str("printer")?;
    if path.is_empty() || a.get("path").is_some() {
        return Err(bad("printer_output must name a new PDF file and cannot be combined with path"));
    }
    if !printer.eq_ignore_ascii_case(PRINTER) || a.opt_bool("as_image")?.is_none() {
        return Err(bad(
            "printer_output requires the explicitly named Microsoft Print to PDF printer and explicit as_image true (GDI) or false (DXGI)",
        ));
    }
    if a.opt_int("copies")?.unwrap_or(1) != 1 || a.opt_str("duplex")?.unwrap_or("off") != "off" {
        return Err(bad("printer_output supports copies 1 and duplex off only"));
    }
    // Collation cannot change one copy, but a malformed value still must not be ignored.
    let _ = a.opt_bool("collate")?;
    match a.get("options") {
        None => {}
        Some(Value::Object(options)) if options.is_empty() => {}
        Some(_) => return Err(bad("printer_output does not support driver options; omit options or use an empty object")),
    }
    Ok(Some(Destination { path, printer }))
}

fn paper_matches(actual: (f64, f64), expected: (f64, f64)) -> bool {
    [actual.0, actual.1, expected.0, expected.1].iter().all(|v| v.is_finite() && *v > 0.0)
        && (actual.0 - expected.0).abs() <= PAPER_TOLERANCE
        && (actual.1 - expected.1).abs() <= PAPER_TOLERANCE
}

/// Refuse a driver's two-sided default too: this API has no safe setting override for it.
fn check_simplex(settings: &rgb::DevMode) -> Result<()> {
    let bytes = settings.as_bytes();
    let fields = bytes
        .get(72..76)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| failed("native printer settings have no dmFields"))?;
    let duplex = bytes
        .get(94..96)
        .and_then(|b| <[u8; 2]>::try_from(b).ok())
        .map(i16::from_le_bytes)
        .ok_or_else(|| failed("native printer settings have no dmDuplex"))?;
    if fields & 0x1000 != 0 && duplex != 1 {
        // DM_DUPLEX and DMDUP_SIMPLEX, from DEVMODEW.
        return Err(failed("native printer default is two-sided; choose single-sided in its preferences before printer_output"));
    }
    Ok(())
}

pub(super) fn print(
    a: &Automation,
    destination: Destination<'_>,
    expected: ExpectedSheets,
    bytes: Vec<u8>,
    raster: pdfcraft_engine::print::Raster,
    title: &str,
    mut out: Value,
) -> Result<Value> {
    if !cfg!(windows) {
        return Err(failed("printer_output is available only on Windows"));
    }
    // Snapshot once and use that same settings object for geometry and the native job.
    let settings = rgb::default_settings(destination.printer).map_err(|e| failed(format!("native printer settings: {e}")))?;
    check_simplex(&settings)?;
    let page = rgb::page(destination.printer, Some(&settings)).map_err(|e| failed(format!("native printer geometry: {e}")))?;
    let sheet = render_sheet(bytes, raster.dpi, raster.grayscale).map_err(failed)?;
    if !paper_matches(page.paper, sheet.paper) {
        return Err(failed(format!(
            "native printer paper/orientation {:?} pt differs from imposed sheet {:?} pt; select matching driver preferences or paper (tolerance 0.05 pt)",
            page.paper, sheet.paper
        )));
    }
    // A file printer must reach the whole sheet. Refuse margins that would clip the raster.
    if !paper_matches((page.printable.0 + page.printable.2, page.printable.1 + page.printable.3), sheet.paper)
        || page.printable.0.abs() > PAPER_TOLERANCE
        || page.printable.1.abs() > PAPER_TOLERANCE
    {
        return Err(failed("native file printer cannot cover the whole imposed sheet without clipping"));
    }
    let mut printed = None;
    let (path, length) = stage_and_publish(a, destination.path, expected, Duration::from_secs(5), |stage| {
        let output = stage.to_str().ok_or_else(|| failed("native output path is not valid Unicode"))?;
        let job = rgb::Job {
            printer: destination.printer,
            settings: Some(&settings),
            title,
            output_file: Some(output),
            copies: 1,
            color_policy: rgb::ColorPolicy::DriverManaged,
            primitives: &[],
        };
        printed = Some(
            rgb::print(
                &job,
                &rgb::RgbImage { width: sheet.width, height: sheet.height, data: &sheet.data },
                (0.0, 0.0, sheet.paper.0, sheet.paper.1),
            )
            .map_err(|e| failed(format!("native GDI output: {e}")))?,
        );
        Ok(())
    })?;
    out["printer_output"] = json!(path.to_string_lossy());
    out["output_bytes"] = json!(length);
    out["backend"] = json!("windows-gdi-rgb");
    out["raster"] =
        json!({ "dpi": raster.dpi, "width": sheet.width, "height": sheet.height, "grayscale": raster.grayscale, "searchable_text": false });
    out["physical_print_verified"] = json!(false);
    if let Some(printed) = printed {
        out["device_dpi"] = json!([printed.dpi.0, printed.dpi.1]);
        out["device_rect"] = json!(printed.device_rect);
    }
    Ok(out)
}

/// The driver only sees a private staging path. Publication uses no-clobber atomic rename,
/// including when a competing process creates the target after our existence check.
pub(super) fn stage_and_publish(
    a: &Automation,
    requested: &str,
    expected: ExpectedSheets,
    timeout: Duration,
    driver: impl FnOnce(&Path) -> Result<()>,
) -> Result<(PathBuf, usize)> {
    let path = std::path::absolute(a.resolve(requested, true)?).map_err(failed)?;
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        return Err(bad("printer_output must have a .pdf extension"));
    }
    match path.symlink_metadata() {
        Ok(_) => return Err(failed(format!("printer_output already exists; refusing to replace it: {}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(failed(format!("cannot inspect printer_output: {error}"))),
    }
    let parent = path.parent().ok_or_else(|| failed("printer_output has no parent directory"))?;
    let stage_dir = tempfile::Builder::new().prefix(".pdfcraft-native-").tempdir_in(parent).map_err(failed)?;
    let stage = stage_dir.path().join("driver.pdf");
    driver(&stage)?;
    let bytes = completed_pdf(a, &stage, expected, timeout)?;
    // Recheck confinement immediately before publication; persist_noclobber owns the final
    // existence race. No driver ever receives the user's final path.
    if std::path::absolute(a.resolve(requested, true)?).map_err(failed)? != path {
        return Err(failed("printer_output destination changed during native printing"));
    }
    crate::bates::publish_new(&path, &bytes)?;
    Ok((path, bytes.len()))
}

// This bounds polling of the generated file after the driver returns. It is not a timeout
// for native submission, rendering or the driver's completion callback.
fn completed_pdf(a: &Automation, path: &Path, expected: ExpectedSheets, timeout: Duration) -> Result<Vec<u8>> {
    let started = Instant::now();
    loop {
        let result = read_completed_pdf(a, path, expected);
        if result.is_ok() || started.elapsed() >= timeout {
            return result;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn read_completed_pdf(a: &Automation, path: &Path, expected: ExpectedSheets) -> Result<Vec<u8>> {
    // This is generated output under the write root, not a caller-supplied read path.
    // Re-resolve it before every read so a staging link cannot escape that authority.
    let requested = path.to_str().ok_or_else(|| failed("native staging path is not valid Unicode"))?;
    let resolved = a.resolve(requested, true)?;
    let file = std::fs::File::open(resolved).map_err(|e| failed(format!("native output is not complete: {e}")))?;
    let metadata = file.metadata().map_err(failed)?;
    if !metadata.is_file() {
        return Err(failed("native output is not a regular PDF file"));
    }
    if metadata.len() > OUTPUT_LIMIT {
        return Err(failed("native output exceeds the 128 MiB limit"));
    }
    let mut bytes = Vec::new();
    file.take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes).map_err(failed)?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err(failed("native output exceeds the 128 MiB limit"));
    }
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(0, |i| i + 1);
    if !bytes.starts_with(b"%PDF-") || !bytes.get(..end).is_some_and(|b| b.ends_with(b"%%EOF")) {
        return Err(failed("native output is not a completed PDF (header or EOF missing)"));
    }
    let shared = Arc::new(bytes);
    let doc = pdfcraft_cos::Document::open(shared.clone()).map_err(failed)?;
    if !doc.repair_log().is_empty() {
        return Err(failed("native output PDF requires repair; refusing to publish it"));
    }
    pdfcraft_render::check_pages(shared.clone(), None, expected.count())
        .map_err(|e| failed(format!("native output must contain exactly {} imposed sheet(s): {e}", expected.count())))?;
    // Retain one bounded byte buffer for publication.
    drop(doc);
    Arc::try_unwrap(shared).map_err(|_| failed("native output is still in use"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(pages: usize) -> Vec<u8> {
        let kids = (0..pages).map(|i| format!("{} 0 R", i + 3)).collect::<Vec<_>>().join(" ");
        let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Count {pages} /Kids [{kids}] >>")];
        objects.extend((0..pages).map(|_| "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>".to_string()));
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        bytes
    }

    #[test]
    fn explicit_output_rejects_ambiguous_or_unsupported_requests() {
        let base = json!({ "printer_output": "out.pdf", "printer": PRINTER, "as_image": true });
        assert!(request(&Args(&base)).unwrap().is_some());
        for (key, value) in [
            ("path", json!("export.pdf")),
            ("printer", json!("default")),
            ("printer", json!("Office")),
            ("as_image", Value::Null),
            ("copies", json!(2)),
            ("duplex", json!("printer")),
            ("options", json!({"media":"A4"})),
            ("collate", json!("yes")),
            ("printer_output", Value::Null),
        ] {
            let mut args = base.clone();
            args[key] = value;
            assert!(request(&Args(&args)).is_err(), "{args}");
        }
        assert!(request(&Args(&json!({"path":"export.pdf"}))).unwrap().is_none());
        let mut dxgi = base.clone();
        dxgi["as_image"] = json!(false);
        assert!(request(&Args(&dxgi)).unwrap().is_some());
        dxgi.as_object_mut().unwrap().remove("as_image");
        assert!(request(&Args(&dxgi)).is_err());
    }

    #[test]
    fn native_geometry_does_not_swap_or_scale_the_sheet() {
        assert!(paper_matches((612.0, 792.0), (612.01, 792.01)));
        assert!(!paper_matches((792.0, 612.0), (612.0, 792.0)));
        assert!(!paper_matches((595.28, 841.89), (612.0, 792.0)));
        assert!(!paper_matches((f64::NAN, 792.0), (612.0, 792.0)));
    }

    #[test]
    fn invalid_sheet_limits_are_rejected_before_the_driver_closure() {
        let dir = tempfile::tempdir().unwrap();
        let a = Automation::new().with_root(dir.path()).unwrap();
        let target = dir.path().join("never-created.pdf");
        for (sheets, as_image) in [(0, false), (65, false), (usize::MAX, false), (0, true), (2, true), (64, true)] {
            let called = std::cell::Cell::new(false);
            let result = ExpectedSheets::new(sheets, as_image).and_then(|expected| {
                stage_and_publish(&a, target.to_str().unwrap(), expected, Duration::ZERO, |_| {
                    called.set(true);
                    Err(failed("the driver must not be reached for an invalid limit"))
                })
            });
            let error = result.unwrap_err();
            assert!(matches!(&error, crate::ToolError::InvalidArgs(_)), "{sheets}, {as_image}: {error}");
            assert!(error.to_string().contains(if as_image { "exactly one" } else { "1..64" }), "the limit must be actionable: {error}");
            assert!(!called.get(), "an invalid limit must be rejected before native submission");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "invalid limits must not create even a staging directory");
        }
        for sheets in 1..=64 {
            assert_eq!(ExpectedSheets::new(sheets, false).unwrap().count(), sheets);
        }
        assert_eq!(ExpectedSheets::new(1, true).unwrap().count(), 1);
    }

    #[test]
    fn driver_failure_and_target_race_preserve_the_final_file() {
        let dir = tempfile::tempdir().unwrap();
        let a = Automation::new().with_root(dir.path()).unwrap();
        let target = dir.path().join("out.pdf");
        let name = target.to_str().unwrap();
        let expected = ExpectedSheets::new(1, true).unwrap();
        let error = stage_and_publish(&a, name, expected, Duration::ZERO, |stage| {
            std::fs::write(stage, b"partial").map_err(failed)?;
            Err(failed("synthetic driver failure"))
        });
        assert!(error.is_err());
        assert!(!target.exists());
        let error = stage_and_publish(&a, name, expected, Duration::ZERO, |stage| {
            std::fs::write(stage, pdf(1)).map_err(failed)?;
            std::fs::write(&target, b"concurrent owner").map_err(failed)?;
            Ok(())
        });
        assert!(error.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"concurrent owner");
        assert!(stage_and_publish(&a, name, expected, Duration::ZERO, |_| panic!("existing-target driver must not run")).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn only_a_completed_single_page_gdi_pdf_is_published() {
        let dir = tempfile::tempdir().unwrap();
        let a = Automation::new().with_root(dir.path()).unwrap();
        let target = dir.path().join("out.pdf");
        let expected = ExpectedSheets::new(1, true).unwrap();
        for bytes in [b"%PDF-1.7\npartial".to_vec(), pdf(2)] {
            let result =
                stage_and_publish(&a, target.to_str().unwrap(), expected, Duration::ZERO, |stage| std::fs::write(stage, &bytes).map_err(failed));
            assert!(result.is_err());
            assert!(!target.exists());
        }
        let bytes = pdf(1);
        stage_and_publish(&a, target.to_str().unwrap(), expected, Duration::ZERO, |stage| std::fs::write(stage, &bytes).map_err(failed)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let outside = dir.path().parent().unwrap().join("outside-native.pdf");
        let result = stage_and_publish(&a, outside.to_str().unwrap(), expected, Duration::ZERO, |_| panic!("outside-root driver must not run"));
        assert!(result.is_err());
    }

    #[test]
    fn dxgi_publication_requires_exactly_the_imposed_sheet_count() {
        for (expected_count, actual_count) in [(2, 0), (2, 1), (2, 3), (64, 63), (64, 65), (1, 2)] {
            let dir = tempfile::tempdir().unwrap();
            let a = Automation::new().with_root(dir.path()).unwrap();
            let target = dir.path().join("missing-or-extra.pdf");
            let expected = ExpectedSheets::new(expected_count, false).unwrap();
            let bytes = pdf(actual_count);
            let result =
                stage_and_publish(&a, target.to_str().unwrap(), expected, Duration::ZERO, |stage| std::fs::write(stage, &bytes).map_err(failed));
            assert!(result.is_err(), "expected {expected_count}, actual {actual_count}: a wrong page count must not publish");
            assert!(!target.exists());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "failure must remove its private stage");
        }
    }

    #[test]
    fn dxgi_publication_accepts_completed_pdfs_at_both_limits_and_multiple_sheets() {
        for sheets in [1, 2, 64] {
            let dir = tempfile::tempdir().unwrap();
            let a = Automation::new().with_root(dir.path()).unwrap();
            let target = dir.path().join("C\u{327}ag\u{306}rı-I\u{307}stanbul.pdf");
            let expected = ExpectedSheets::new(sheets, false).unwrap();
            let bytes = pdf(sheets);
            let (path, length) =
                stage_and_publish(&a, target.to_str().unwrap(), expected, Duration::ZERO, |stage| std::fs::write(stage, &bytes).map_err(failed))
                    .unwrap();
            // Windows canonical paths add a verbatim prefix; compare file identity separately
            // from the exact filename, whose original NFD codepoints must stay unchanged.
            assert_eq!(path.file_name(), target.file_name(), "publication must preserve the original NFD filename");
            assert_eq!(path.canonicalize().unwrap(), target.canonicalize().unwrap(), "publication must use the requested file");
            assert_eq!(length, bytes.len());
            assert_eq!(std::fs::read(&target).unwrap(), bytes);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "only the completed PDF may remain");
        }
    }

    #[test]
    fn a_claimed_page_tree_count_cannot_hide_a_missing_sheet() {
        let dir = tempfile::tempdir().unwrap();
        let a = Automation::new().with_root(dir.path()).unwrap();
        let target = dir.path().join("wrong-tree-count.pdf");
        // Same-length replacement preserves offsets: the declared Count is two, but only
        // one leaf page exists. Publication must count actual pages, not trust the dictionary.
        let bytes = String::from_utf8(pdf(1)).unwrap().replace("/Count 1 /Kids", "/Count 2 /Kids").into_bytes();
        let expected = ExpectedSheets::new(2, false).unwrap();
        let result = stage_and_publish(&a, target.to_str().unwrap(), expected, Duration::ZERO, |stage| std::fs::write(stage, &bytes).map_err(failed));
        assert!(result.is_err());
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
