use crate::bindings::pdfium::*;
use crate::printer::FilePrinter;
use crate::printer::PrinterDevice;
use crate::ticket::PrintTicket;
use crate::ticket::ToDevModeError;
use crate::utils::emf::Emf;
use crate::utils::pdfium::PdfiumCustomDocument;
use crate::utils::pdfium::PdfiumGuard;
use crate::utils::wchar;
use scopeguard::defer;
use std::cell::Cell;
use std::path::Path;
use std::ptr;
use std::{fs::File, mem};
use thiserror::Error;
use windows::Win32::Foundation::RECT;
use windows::{
    core::PCWSTR,
    Win32::{
        Graphics::Gdi::{
            CreateDCW, DeleteDC, GetDeviceCaps, SetBrushOrgEx, SetGraphicsMode, SetStretchBltMode,
            GET_DEVICE_CAPS_INDEX, GM_ADVANCED, HALFTONE, LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT,
            PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH,
        },
        Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW},
    },
};

#[derive(Error, Debug)]
/// Represents an error from [`PdfiumPrinter`].
pub enum PdfiumPrinterError {
    /// Failed to open printer.
    #[error("Failed to open printer")]
    FailedToOpenPrinter,
    /// File I/O error.
    #[error("File I/O error")]
    FileIOError(#[source] std::io::Error),
    /// Print ticket error.
    #[error("Print Ticker Error")]
    PrintTicketError(#[source] ToDevModeError),
    /// StartDoc failed.
    #[error("StartDocW failed (returned {0})")]
    StartDocFailed(i32),
    /// StartPage failed.
    #[error("StartPage failed for page {0} (returned {1})")]
    StartPageFailed(i32, i32),
    /// EndPage failed.
    #[error("EndPage failed for page {0} (returned {1})")]
    EndPageFailed(i32, i32),
    /// EndDoc failed.
    #[error("EndDoc failed (returned {0})")]
    EndDocFailed(i32),
    /// PDFium failed to load the document.
    #[error("PDFium failed to load the document (error {0})")]
    PdfiumLoadFailed(u32),
    /// EMF creation failed for a page.
    #[error("Failed to create EMF for page {0}")]
    EmfCreateFailed(i32),
    /// EMF playback failed for a page.
    #[error("Failed to playback EMF for page {0}")]
    EmfPlaybackFailed(i32),
}

/// A printer that uses Pdfium to print PDF documents.
pub struct PdfiumPrinter {
    printer: PrinterDevice,
}

impl PdfiumPrinter {
    /// Create a new [`PdfiumPrinter`] for the given printer device.
    pub fn new(printer: PrinterDevice) -> Self {
        Self { printer }
    }
}

const PRINT_DRIVER: PCWSTR = PCWSTR(
    [
        'W' as u16, 'I' as u16, 'N' as u16, 'S' as u16, 'P' as u16, 'O' as u16, 'O' as u16,
        'L' as u16, 0,
    ]
    .as_ptr(),
);

impl FilePrinter for PdfiumPrinter {
    type Options = PrintTicket;
    type Error = PdfiumPrinterError;

    fn print(
        &self,
        path: &Path,
        options: PrintTicket,
    ) -> std::result::Result<(), PdfiumPrinterError> {
        unsafe {
            let dev_mode = options
                .to_dev_mode(&self.printer)
                .map_err(PdfiumPrinterError::PrintTicketError)?;
            // According to https://learn.microsoft.com/en-us/windows/win32/printdocs/retrieving-a-printer-device-context:
            // > To render to a specific printer, you must specify "WINSPOOL" as the device.
            //
            // However, according to https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-createdcw
            // > For printing, we recommend that you pass NULL to lpszDriver because GDI ignores lpszDriver for printer devices.
            //
            // We check the Chromium source code and it seems that they are using "WINSPOOL" as the driver name, so we will do the same.
            // https://github.com/chromium/chromium/blob/749ad837ac3e74e3988f4a079979e3ea7e926f25/printing/printing_context_win.cc#L489
            let hdc_print = CreateDCW(
                PRINT_DRIVER,
                PCWSTR(wchar::to_wide_chars(self.printer.os_name()).as_ptr()),
                None,
                Some(dev_mode.as_ptr() as *const _),
            );
            if hdc_print.is_invalid() {
                return Err(PdfiumPrinterError::FailedToOpenPrinter);
            }
            defer! {
                let _ = DeleteDC(hdc_print);
            }

            SetGraphicsMode(hdc_print, GM_ADVANCED);
            SetStretchBltMode(hdc_print, HALFTONE);
            // After setting the HALFTONE stretching mode,
            // an application must call the SetBrushOrgEx function to set the brush origin.
            // If it fails to do so, brush misalignment occurs.
            let _ = SetBrushOrgEx(hdc_print, 0, 0, None);

            let mut doc_name = wchar::to_wide_chars(path.file_name().unwrap_or(path.as_ref()));
            let doc_info = DOCINFOW {
                cbSize: mem::size_of::<DOCINFOW>() as i32,
                fwType: 0,
                lpszDocName: PCWSTR(doc_name.as_mut_ptr()),
                lpszOutput: PCWSTR::null(),
                lpszDatatype: PCWSTR::null(),
            };

            let start_doc_ret = StartDocW(hdc_print, &doc_info);
            if start_doc_ret <= 0 {
                return Err(PdfiumPrinterError::StartDocFailed(start_doc_ret));
            }

            let document_completed = Cell::new(false);
            defer! {
                if !document_completed.get() {
                    let _ = AbortDoc(hdc_print);
                }
            }

            let _pdfium_guard = PdfiumGuard::guard();
            let mut file = File::open(path).map_err(PdfiumPrinterError::FileIOError)?;
            let mut file_delegation =
                PdfiumCustomDocument::new(&mut file).map_err(PdfiumPrinterError::FileIOError)?;
            let document = FPDF_LoadCustomDocument(file_delegation.as_mut(), ptr::null());
            if document.is_null() {
                return Err(PdfiumPrinterError::PdfiumLoadFailed(FPDF_GetLastError()));
            }
            defer! {
                FPDF_CloseDocument(document);
            }

            let get_attr =
                |kind: GET_DEVICE_CAPS_INDEX| -> i32 { GetDeviceCaps(Some(hdc_print), kind) };
            let page_count = FPDF_GetPageCount(document);
            for page_index in 0..page_count {
                let page = FPDF_LoadPage(document, page_index);
                defer! {
                    FPDF_ClosePage(page);
                }

                let start_page_ret = StartPage(hdc_print);
                if start_page_ret <= 0 {
                    return Err(PdfiumPrinterError::StartPageFailed(
                        page_index,
                        start_page_ret,
                    ));
                }
                let dpi_x = get_attr(LOGPIXELSX);
                let dpi_y = get_attr(LOGPIXELSY);
                let page_std_width = FPDF_GetPageWidth(page);
                let page_std_height = FPDF_GetPageHeight(page);
                let page_width = (page_std_width * dpi_x as f64 / 72.0).round() as i32;
                let page_height = (page_std_height * dpi_y as f64 / 72.0).round() as i32;
                let emf = Emf::new(
                    hdc_print,
                    (page_std_width * 2540.0 / 72.0).round() as i32,
                    (page_std_height * 2540.0 / 72.0).round() as i32,
                    |hdc_emf| {
                        SetGraphicsMode(hdc_emf, GM_ADVANCED);
                        FPDF_RenderPage(
                            hdc_emf,
                            page,
                            0,
                            0,
                            page_width,
                            page_height,
                            0,
                            FPDF_PRINTING,
                        );
                        true
                    },
                )
                .map_err(|_| PdfiumPrinterError::EmfCreateFailed(page_index))?;

                let paper_width = get_attr(PHYSICALWIDTH);
                let paper_height = get_attr(PHYSICALHEIGHT);
                let scale = f64::min(
                    paper_width as f64 / page_width as f64,
                    paper_height as f64 / page_height as f64,
                );
                let actual_width = (page_width as f64 * scale).round() as i32;
                let actual_height = (page_height as f64 * scale).round() as i32;
                let left = -get_attr(PHYSICALOFFSETX) + (paper_width - actual_width) / 2;
                let top = -get_attr(PHYSICALOFFSETY) + (paper_height - actual_height) / 2;
                let target_rect = RECT {
                    left,
                    top,
                    right: actual_width + left,
                    bottom: actual_height + top,
                };
                let page_result = emf
                    .playback(hdc_print, target_rect)
                    .map_err(|_| PdfiumPrinterError::EmfPlaybackFailed(page_index));
                let end_page_ret = EndPage(hdc_print);
                page_result?;
                if end_page_ret <= 0 {
                    return Err(PdfiumPrinterError::EndPageFailed(page_index, end_page_ret));
                }
            }

            let end_doc_ret = EndDoc(hdc_print);
            if end_doc_ret <= 0 {
                return Err(PdfiumPrinterError::EndDocFailed(end_doc_ret));
            }
            document_completed.set(true);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PdfiumPrinter;
    use crate::{printer::FilePrinter, test_utils::null_device};
    use std::path::Path;

    #[test]
    fn print_simple_pdf_document() {
        let device = null_device::thread_local();
        let pdf = PdfiumPrinter::new(device);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/test_document.pdf");
        pdf.print(path.as_path(), Default::default()).unwrap();
    }
}
