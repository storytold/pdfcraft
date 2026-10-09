//! Native Windows discovery and PDF submission. No shell commands or external PDF viewer.

use std::io::Write;
use std::sync::Arc;

use winprint::printer::{FilePrinter, PrinterDevice, WinPdfPrinter};
use winprint::ticket::document::{NS_PSK, OwnedName, PrintFeature, PrintTicketDocument, reader::ParsableXmlDocument};
use winprint::ticket::{FeatureOptionPack, PrintCapabilities, PrintTicket, PrintTicketBuilder};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

use super::{Duplex, Job, Printer};
use crate::PrintError;

fn spool_error(e: impl std::fmt::Display) -> PrintError {
    PrintError::Spool(e.to_string())
}

fn default_printer() -> Option<String> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows NT\CurrentVersion\Windows").ok()?;
    let value: String = key.get_value("Device").ok()?;
    // The queue name itself can contain commas; the final fields are driver and port.
    let (name, _) = value.rsplit_once(',')?;
    let (name, _) = name.rsplit_once(',')?;
    Some(name.to_string())
}

pub(super) fn printers() -> Result<Vec<Printer>, PrintError> {
    let default = default_printer();
    let mut printers: Vec<_> = PrinterDevice::all()
        .map_err(spool_error)?
        .into_iter()
        .map(|p| Printer { name: p.name().to_string(), default: default.as_deref() == Some(p.name()) })
        .collect();
    printers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(printers)
}

fn select_device(devices: Vec<PrinterDevice>, requested: Option<&str>, default: Option<&str>) -> Result<PrinterDevice, PrintError> {
    let name = requested.or(default).ok_or_else(|| spool_error("No Windows default printer is configured; select a printer by name."))?;
    devices
        .into_iter()
        .find(|p| p.name() == name)
        .ok_or_else(|| spool_error(format!("Windows printer {name:?} was not found. Reopen Print to refresh the printer list.")))
}

/// The PDF is already imposed at final size. Set media and disable further driver scaling.
fn ticket(job: &Job, width: f64, height: f64) -> Result<PrintTicket, PrintError> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 || width.max(height) > 14_400.0 {
        return Err(spool_error("The print sheet has an invalid or unsupported size."));
    }
    let w = (width.min(height) * 25_400.0 / 72.0).round() as u32;
    let h = (width.max(height) * 25_400.0 / 72.0).round() as u32;
    let orientation = if width > height { "Landscape" } else { "Portrait" };
    let duplex = match job.duplex {
        Duplex::Off => "OneSided",
        Duplex::LongEdge => "TwoSidedLongEdge",
        Duplex::ShortEdge => "TwoSidedShortEdge",
    };
    let collate = if job.collate { "Collated" } else { "Uncollated" };
    let color = if job.grayscale { "Monochrome" } else { "Color" };
    let copies = job.copies.clamp(1, 999);
    // Only fixed schema tokens and bounded numbers enter XML. Printer names and document
    // titles are never interpolated into XML, a command line, or an executable script.
    Ok(PrintTicket::from_xml(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<psf:PrintTicket version="1" xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework" xmlns:psk="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
 <psf:ParameterInit name="psk:JobCopiesAllDocuments"><psf:Value xsi:type="xsd:integer">{copies}</psf:Value></psf:ParameterInit>
 <psf:Feature name="psk:DocumentCollate"><psf:Option name="psk:{collate}"/></psf:Feature>
 <psf:Feature name="psk:JobDuplexAllDocumentsContiguously"><psf:Option name="psk:{duplex}"/></psf:Feature>
 <psf:Feature name="psk:PageOutputColor"><psf:Option name="psk:{color}"/></psf:Feature>
 <psf:Feature name="psk:PageScaling"><psf:Option name="psk:None"/></psf:Feature>
 <psf:Feature name="psk:PageOrientation"><psf:Option name="psk:{orientation}"/></psf:Feature>
 <psf:Feature name="psk:PageMediaSize"><psf:Option>
  <psf:ScoredProperty name="psk:MediaSizeWidth"><psf:Value xsi:type="xsd:integer">{w}</psf:Value></psf:ScoredProperty>
  <psf:ScoredProperty name="psk:MediaSizeHeight"><psf:Value xsi:type="xsd:integer">{h}</psf:Value></psf:ScoredProperty>
 </psf:Option></psf:Feature>
</psf:PrintTicket>"#
    )))
}

fn job_filename(title: &str) -> String {
    let safe: String = title.chars().take(100).map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    // A fixed prefix also excludes Windows reserved device names such as CON and NUL.
    format!("PdfCraft-{safe}.pdf")
}

fn driver_ticket(device: &PrinterDevice, job: &Job, width: f64, height: f64) -> Result<PrintTicket, PrintError> {
    let requested = ticket(job, width, height)?;
    let capabilities = PrintCapabilities::fetch(device).map_err(spool_error)?;
    let w = (width.min(height) * 25_400.0 / 72.0).round() as u32;
    let h = (width.max(height) * 25_400.0 / 72.0).round() as u32;
    let preferred = crate::matching_paper((width, height)).and_then(|i| crate::PAPERS.get(i)).map(|p| p.0.split(" (").next().unwrap_or(p.0));
    let mut candidates: Vec<_> = capabilities
        .page_media_sizes()
        .filter(|m| {
            let s = m.size();
            s.width_in_micron().abs_diff(w) <= 250 && s.height_in_micron().abs_diff(h) <= 250
        })
        .collect();
    candidates.sort_by_key(|m| !preferred.is_some_and(|p| m.display_name().is_some_and(|name| name.starts_with(p))));
    let media = candidates.into_iter().next().ok_or_else(|| {
        spool_error(format!(
            "{} does not advertise a {:.2} × {:.2} inch sheet. Choose a supported paper size or configure that size in the printer driver.",
            device.name(),
            width / 72.0,
            height / 72.0
        ))
    })?;
    // Preserve the driver's exact option name and namespace (including vendor-specific
    // ARCH sizes). Dimension-only options can silently become Letter in Epson drivers.
    let mut document = PrintTicketDocument::parse_from_bytes(requested.get_xml()).map_err(spool_error)?;
    let media_ticket: PrintTicket = media.into();
    let media_document = PrintTicketDocument::parse_from_bytes(media_ticket.get_xml()).map_err(spool_error)?;
    document.features.retain(|f| !(f.name.local_name == "PageMediaSize" && f.name.namespace_ref() == Some(NS_PSK)));
    document.features.extend(media_document.features);
    document.parameter_inits.extend(media_document.parameter_inits);
    let scaling_name = OwnedName::qualified("PageScaling", NS_PSK, Some("psk"));
    if let Some(no_scaling) = capabilities.options_for_feature(scaling_name.clone()).find(|o| o.name.as_ref().is_some_and(|n| n.local_name == "None"))
    {
        document.features.retain(|f| !(f.name.local_name == "PageScaling" && f.name.namespace_ref() == Some(NS_PSK)));
        document.features.push(PrintFeature { name: scaling_name, properties: Vec::new(), options: vec![no_scaling.clone()], features: Vec::new() });
    }
    let mut builder = PrintTicketBuilder::new(device).map_err(spool_error)?;
    builder.merge(PrintTicket::from(document)).map_err(spool_error)?;
    let validated = builder.build().map_err(spool_error)?;
    verify_ticket_size(&validated, w, h, width > height)?;
    Ok(validated)
}

fn verify_ticket_size(ticket: &PrintTicket, width: u32, height: u32, landscape: bool) -> Result<(), PrintError> {
    // Some drivers return empty vendor string parameters. Read only the relevant
    // schema fields instead of rejecting those otherwise valid PrintTickets.
    let xml = std::str::from_utf8(ticket.get_xml()).map_err(spool_error)?;
    let document = roxmltree::Document::parse(xml).map_err(spool_error)?;
    let named = |node: roxmltree::Node<'_, '_>, local: &str| {
        node.attribute("name")
            .and_then(|v| v.split_once(':'))
            .is_some_and(|(prefix, name)| name == local && node.lookup_namespace_uri(Some(prefix)) == Some(NS_PSK))
    };
    let feature =
        |name: &str| document.root_element().children().find(|n| n.has_tag_name((winprint::ticket::document::NS_PSF, "Feature")) && named(*n, name));
    let media = feature("PageMediaSize").ok_or_else(|| spool_error("The printer driver did not confirm the sheet size."))?;
    let dimension = |name: &str| {
        media
            .descendants()
            .find(|n| n.has_tag_name((winprint::ticket::document::NS_PSF, "ScoredProperty")) && named(*n, name))
            .and_then(|n| n.children().find(|v| v.has_tag_name((winprint::ticket::document::NS_PSF, "Value"))))
            .and_then(|n| n.text())
            .and_then(|v| v.trim().parse::<u32>().ok())
    };
    if !dimension("MediaSizeWidth").is_some_and(|w| w.abs_diff(width) <= 250)
        || !dimension("MediaSizeHeight").is_some_and(|h| h.abs_diff(height) <= 250)
    {
        return Err(spool_error("The printer driver substituted a different paper size. No job was sent; select a supported sheet size."));
    }
    let orientation = feature("PageOrientation").and_then(|f| f.children().find(|n| n.has_tag_name((winprint::ticket::document::NS_PSF, "Option"))));
    if !orientation.is_some_and(|n| named(n, if landscape { "Landscape" } else { "Portrait" })) {
        return Err(spool_error("The printer driver changed the sheet orientation. No job was sent."));
    }
    if let Some(scaling) = feature("PageScaling") {
        // Microsoft's IPP driver exposes its advertised None option in a private
        // namespace. Its local name is still None, rather than Fit or Fill.
        if !scaling.children().any(|n| {
            n.has_tag_name((winprint::ticket::document::NS_PSF, "Option")) && n.attribute("name").and_then(|v| v.rsplit(':').next()) == Some("None")
        }) {
            return Err(spool_error("The printer driver enabled additional scaling. No job was sent."));
        }
    }
    Ok(())
}

pub(super) fn submit(pdf: &[u8], job: &Job) -> Result<String, PrintError> {
    let device = select_device(PrinterDevice::all().map_err(spool_error)?, job.printer.as_deref(), default_printer().as_deref())?;
    let doc = pdfcraft_cos::Document::open(Arc::new(pdf.to_vec()))?;
    let pages = pdfcraft_model::pages(&doc);
    let first = pages.first().ok_or(PrintError::NoPages)?.display_size(&doc);
    // This backend has one ticket per job. Never silently rotate/scale mixed sheets.
    if pages.iter().any(|p| {
        let s = p.display_size(&doc);
        (s.0 - first.0).abs() > 0.1 || (s.1 - first.1).abs() > 0.1
    }) {
        return Err(spool_error(
            "Windows printing requires one sheet size and orientation per job. Choose Portrait or Landscape instead of Auto, or print the differing sheets separately.",
        ));
    }
    let dir = tempfile::Builder::new().prefix("pdfcraft-job-").tempdir().map_err(spool_error)?;
    let path = dir.path().join(job_filename(&job.title));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(spool_error)?;
    file.write_all(pdf).map_err(spool_error)?;
    drop(file);
    let job = job.clone();
    // A dedicated thread gives WinRT its own COM apartment, independent of egui or CLI callers.
    // Join before dropping the private temporary directory so the spooler can read the PDF.
    let result = std::thread::Builder::new()
        .name("pdfcraft-windows-print".into())
        .spawn(move || {
            let options = driver_ticket(&device, &job, first.0, first.1)?;
            WinPdfPrinter::new(device).print(&path, options).map_err(|e| {
                let mut message = e.to_string();
                let mut cause = std::error::Error::source(&e);
                while let Some(source) = cause {
                    message.push_str(&format!(": {source}"));
                    cause = source.source();
                }
                spool_error(message)
            })
        })
        .map_err(spool_error)?
        .join()
        .map_err(|_| spool_error("The Windows print worker failed. Check the queue before retrying to avoid duplicate output."))?;
    result?;
    Ok("Accepted by the Windows print spooler".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_ticket_preserves_job_options_and_sheet_size() {
        let job = Job { copies: 3, collate: false, duplex: Duplex::ShortEdge, grayscale: true, title: "<&>".into(), ..Job::default() };
        let xml = String::from_utf8(ticket(&job, 1224.0, 792.0).unwrap().into_xml()).unwrap();
        for value in [">3<", "Uncollated", "TwoSidedShortEdge", "Monochrome", "Landscape", ">279400<", ">431800<", "psk:None"] {
            assert!(xml.contains(value), "missing {value}");
        }
        assert!(!xml.contains("<&>"));
        assert!(ticket(&job, f64::NAN, 100.0).is_err());
        assert!(ticket(&job, -1.0, 100.0).is_err());
    }

    #[test]
    fn missing_printer_never_falls_back_to_another_queue() {
        assert!(select_device(Vec::new(), Some("missing"), Some("default")).is_err());
        assert!(select_device(Vec::new(), None, None).is_err());
    }

    #[test]
    fn job_title_cannot_escape_temporary_directory() {
        let name = job_filename(r"..\..\CON:/unsafe?name");
        assert_eq!(std::path::Path::new(&name).components().count(), 1);
        assert!(name.starts_with("PdfCraft-"));
    }

    #[test]
    fn architectural_ticket_requests_24_by_36_without_driver_scaling() {
        let xml = String::from_utf8(ticket(&Job::default(), 2592.0, 1728.0).unwrap().into_xml()).unwrap();
        for value in [">609600<", ">914400<", "psk:Landscape", "psk:None"] {
            assert!(xml.contains(value), "missing {value}");
        }
    }

    #[test]
    fn driver_substitution_is_rejected_before_submission() {
        let letter = ticket(&Job::default(), 792.0, 612.0).unwrap();
        assert!(verify_ticket_size(&letter, 609600, 914400, true).is_err());
        let arch = ticket(&Job::default(), 2592.0, 1728.0).unwrap();
        verify_ticket_size(&arch, 609600, 914400, true).unwrap();
        assert!(verify_ticket_size(&arch, 609600, 914400, false).is_err());
    }

    #[test]
    #[ignore = "read-only installed driver check; requires PDFCRAFT_TEST_PRINTER; sends no print job"]
    fn installed_driver_accepts_arch_d_without_substitution() {
        let name = std::env::var("PDFCRAFT_TEST_PRINTER").unwrap();
        let device = select_device(PrinterDevice::all().unwrap(), Some(&name), None).unwrap();
        let ticket = driver_ticket(&device, &Job::default(), 2592.0, 1728.0).unwrap();
        verify_ticket_size(&ticket, 609600, 914400, true).unwrap();
        if let Ok(path) = std::env::var("PDFCRAFT_TEST_TICKET_OUTPUT") {
            std::fs::write(path, ticket.get_xml()).unwrap();
        }
    }
}
