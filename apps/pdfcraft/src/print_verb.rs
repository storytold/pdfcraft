//! `--print <files…>`: File Explorer's Print verb (#1695).
//!
//! Each file is printed on the system default printer without a window, the way Explorer's Print
//! works for other PDF apps: the Print dialog's defaults (all pages, Fit, comments and markups),
//! one copy, and the driver's own two-sided setting. A file that asks for no page scaling
//! (`/PrintScaling /None`) prints at actual size, since that holds without a dialog too. A file that can't be printed unattended — it
//! needs a password, its permissions forbid printing, there is no default printer, the spooler
//! refused the job — is handed back, and the app opens it with the Print dialog showing, so the
//! person sees why rather than nothing happening.

use std::sync::Arc;

use pdfcraft_engine::Session;
use pdfcraft_engine::print::{self, Content, Layout, Orientation, PAPERS, SizeMode, Subset, spool};

/// Print `files` unattended. Returns the ones the app should open with the Print dialog instead.
pub(crate) fn run(files: &[String]) -> Vec<String> {
    let mut leftover = Vec::new();
    for file in files {
        let printed = prepare(file).and_then(|(pdf, job)| spool::submit(&pdf, &job).map_err(|e| e.to_string()));
        match printed {
            Ok(message) => log::info!("printed {file} on the default printer: {message}"),
            Err(e) => {
                log::warn!("couldn't print {file} without the Print dialog: {e}");
                leftover.push(file.clone());
            }
        }
    }
    leftover
}

/// The print-ready PDF of `path` and its job for the default printer, or why there is none.
fn prepare(path: &str) -> Result<(Vec<u8>, spool::Job), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut session = Session::new();
    // A file that needs a password fails here; the app asks for it.
    let id = session.open(name.clone(), Some(path.to_string()), Arc::new(bytes), None).map_err(|e| e.to_string())?;
    let doc = session.get(id).ok_or("the document vanished")?;
    let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let actual_size = doc.prints_without_scaling();
    let settings = settings(&sizes, &labels, actual_size)?;
    // Refuses a document whose permissions forbid printing.
    let pdf = session.print_pdf(id, &settings).map_err(|e| e.to_string())?;
    Ok((pdf, job(&name, actual_size)))
}

/// The Print dialog's defaults for every page, on the paper of the first page (see [`paper`]):
/// Fit, or Actual size when the document asks for no page scaling.
fn settings(sizes: &[(f64, f64)], labels: &[String], actual_size: bool) -> Result<print::Settings, String> {
    let pages = print::select_pages(sizes.len(), None, labels, Subset::All, false).map_err(|e| e.to_string())?;
    let first = sizes.first().copied().ok_or("there are no pages to print")?;
    Ok(print::Settings {
        pages,
        paper: paper(first),
        orientation: Orientation::Auto,
        layout: if actual_size { Layout::Size(SizeMode::Actual) } else { Layout::default() },
        content: Content::DocumentAndMarkups,
        // Not a label job, and printed as vectors (the dialog's defaults).
        ..print::Settings::default()
    })
}

/// The sheet to impose on, portrait. There is no dialog to choose paper, and the dialog's US
/// Letter default is wrong wherever A4 is loaded, so: the standard paper the first page is
/// (A4, Letter…), or else the page's own size. The job asks the spooler to shrink a sheet whose
/// size the printer has no paper for onto the printer's default paper ([`job`]).
fn paper(first: (f64, f64)) -> (f64, f64) {
    if let Some((_, standard)) = print::matching_paper(first).and_then(|i| PAPERS.get(i)) {
        return *standard;
    }
    // A size that isn't a positive number gets the default. Checked before `min`/`max`, which
    // pass a NaN over for the other side.
    if !(first.0.is_finite() && first.1.is_finite() && first.0 > 0.0 && first.1 > 0.0) {
        return print::Settings::default().paper;
    }
    // `layout` takes nothing smaller than an inch.
    (first.0.min(first.1).max(72.0), first.0.max(first.1).max(72.0))
}

/// One copy on the system default printer, with the driver's own two-sided setting. At actual
/// size the spooler may not shrink the sheet either: a printer with no such paper refuses the job,
/// and the file goes to the Print dialog.
fn job(title: &str, actual_size: bool) -> spool::Job {
    spool::Job {
        printer: None,
        copies: 1,
        collate: true,
        duplex: None,
        scaling: if actual_size { spool::Scaling::Exact } else { spool::Scaling::ShrinkToFit },
        grayscale: false,
        title: title.to_string(),
        options: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_engine::{Edit, Printing, Protection};

    const A4: (f64, f64) = (595.28, 841.89);
    const LETTER: (f64, f64) = (612.0, 792.0);

    fn paper_named(name: &str) -> (f64, f64) {
        PAPERS.iter().find(|(n, _)| *n == name).map(|p| p.1).unwrap()
    }

    #[test]
    fn standard_pages_print_on_their_own_paper_in_either_orientation() {
        assert_eq!(paper(A4), paper_named("A4"));
        assert_eq!(paper((A4.1, A4.0)), paper_named("A4"), "a landscape A4 page is still A4 paper; Auto turns the sheet");
        assert_eq!(paper(LETTER), paper_named("US Letter"));
        // Most A4 PDFs round to whole points; they still get the printer's A4, not a custom sheet.
        assert_eq!(paper((595.0, 842.0)), paper_named("A4"));
    }

    #[test]
    fn other_sizes_print_on_a_sheet_of_their_own_size_and_nonsense_on_the_default() {
        assert_eq!(paper((400.0, 300.0)), (300.0, 400.0));
        assert_eq!(paper((10.0, 20.0)), (72.0, 72.0), "nothing smaller than an inch");
        for bad in [(f64::NAN, 500.0), (f64::INFINITY, 500.0), (0.0, 500.0), (-5.0, -5.0)] {
            assert_eq!(paper(bad), print::Settings::default().paper, "{bad:?}");
        }
    }

    #[test]
    fn every_page_prints_with_the_dialog_defaults_on_the_first_pages_paper() {
        let labels: Vec<String> = ["i", "1", "2"].map(String::from).to_vec();
        let s = settings(&[A4, LETTER, A4], &labels, false).unwrap();
        assert_eq!(s.pages, [0, 1, 2]);
        assert_eq!(s.paper, paper_named("A4"));
        assert_eq!(s.layout, Layout::default());
        assert_eq!(s.content, Content::DocumentAndMarkups);
        assert!(settings(&[], &[], false).is_err(), "no pages: an error, not a panic");
        let actual = settings(&[A4], &labels[..1], true).unwrap();
        assert_eq!(actual.layout, Layout::Size(SizeMode::Actual), "/PrintScaling /None: Actual size, not Fit");
        assert_eq!(actual.paper, paper_named("A4"));
    }

    #[test]
    fn the_job_goes_to_the_default_printer_and_shrinks_onto_its_paper() {
        let j = job("report.pdf", false);
        assert_eq!((j.printer, j.copies, j.duplex), (None, 1, None));
        assert_eq!(j.scaling, spool::Scaling::ShrinkToFit);
        assert_eq!(j.title, "report.pdf");
        assert_eq!(job("report.pdf", true).scaling, spool::Scaling::Exact, "no page scaling in the spooler either");
    }

    /// A one-page A4 PDF whose catalog has `/ViewerPreferences << /PrintScaling value >>`.
    fn with_print_scaling(value: &str) -> Vec<u8> {
        let objects = [
            format!("<< /Type /Catalog /Pages 2 0 R /ViewerPreferences << /PrintScaling {value} >> >>"),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_string(),
        ];
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for off in offsets {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
        pdf
    }

    /// ISO 32000-2 Table 147: `/PrintScaling` is honoured when the print dialog is suppressed, as
    /// it is here.
    #[test]
    fn a_file_that_asks_for_no_page_scaling_prints_at_actual_size() {
        let (_d1, none) = file_with(&with_print_scaling("/None"));
        assert_eq!(prepare(&none).unwrap().1.scaling, spool::Scaling::Exact);
        let (_d2, app_default) = file_with(&with_print_scaling("/AppDefault"));
        assert_eq!(prepare(&app_default).unwrap().1.scaling, spool::Scaling::ShrinkToFit);
    }

    /// A file in a fresh temporary folder, removed with the folder.
    fn file_with(bytes: &[u8]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("doc.pdf");
        std::fs::write(&path, bytes).unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

    fn protected(p: Protection) -> Vec<u8> {
        let mut s = Session::new();
        let blank = s.create_blank(A4.0, A4.1, 1).unwrap();
        let id = s.open("blank.pdf", None, blank, None).unwrap();
        s.apply(id, Edit::Protect(p)).unwrap();
        s.save_bytes(id).unwrap().to_vec()
    }

    #[test]
    fn a_printable_file_becomes_a_print_ready_job() {
        let blank = Session::new().create_blank(A4.0, A4.1, 2).unwrap();
        let (_dir, path) = file_with(&blank);
        let (pdf, job) = prepare(&path).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(job.title, "doc.pdf");
    }

    #[test]
    fn files_that_cannot_print_unattended_go_to_the_dialog() {
        // Each of these would run the spooler if `prepare` let it through; none may.
        let missing = std::env::temp_dir().join("pdfcraft-print-verb-missing").join("nothing.pdf");
        assert!(prepare(&missing.to_string_lossy()).is_err(), "a missing file");
        let (_d1, not_pdf) = file_with(b"not a PDF at all");
        assert!(prepare(&not_pdf).is_err(), "not a PDF");
        let password = Protection { open_password: Some("pw".into()), permissions_password: Some("owner".into()), ..Default::default() };
        let (_d2, locked) = file_with(&protected(password));
        assert!(prepare(&locked).unwrap_err().contains("password"), "needs a password");
        let no_print = Protection { permissions_password: Some("owner".into()), printing: Printing::None, ..Default::default() };
        let (_d3, forbidden) = file_with(&protected(no_print));
        assert!(prepare(&forbidden).unwrap_err().contains("printing"), "permissions forbid printing");
    }
}
