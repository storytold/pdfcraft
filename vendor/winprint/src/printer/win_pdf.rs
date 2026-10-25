use super::DxgiPrintContext;
use super::DxgiPrintContextError;
use crate::printer::FilePrinter;
use crate::printer::PrinterDevice;
use crate::ticket::PrintTicket;
use std::path::Path;
use thiserror::Error;
use windows::core::HSTRING;
use windows::Data::Pdf::PdfDocument;
use windows::Storage::StorageFile;
use windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_F;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::System::WinRT::Pdf::IPdfRendererNative;
use windows::Win32::System::WinRT::Pdf::PDF_RENDER_PARAMS;
use windows::core::{Interface, HRESULT};

/// PdfCraft patch (Windows 7): `PdfCreateRenderer` is looked up in System32's
/// Windows.Data.Pdf.dll when printing, instead of being imported, because that DLL exists only on
/// Windows 8.1 and later and a static import stops the program loading on Windows 7. Where it is
/// missing this returns an error, and printing reports it. See PdfCraft's vendor/README.md.
unsafe fn pdf_create_renderer(device: &IDXGIDevice) -> windows::core::Result<IPdfRendererNative> {
    type CreateRenderer =
        unsafe extern "system" fn(*mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe {
        let module = LoadLibraryExW(
            windows::core::w!("Windows.Data.Pdf.dll"),
            None,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )?;
        let Some(address) = GetProcAddress(module, windows::core::s!("PdfCreateRenderer")) else {
            return Err(windows::core::Error::from_thread());
        };
        let create: CreateRenderer = core::mem::transmute(address);
        let mut renderer = core::ptr::null_mut();
        create(device.as_raw(), &mut renderer).ok()?;
        if renderer.is_null() {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_POINTER));
        }
        Ok(IPdfRendererNative::from_raw(renderer))
    }
}

#[derive(Error, Debug)]
/// Represents an error from [`WinPdfPrinter`].
pub enum WinPdfPrinterError {
    /// DXGI print context error.
    #[error("DXGI print context error")]
    DxgiPrintContextError(#[from] DxgiPrintContextError),
    /// Invalid path.
    #[error("Invalid path")]
    InvalidPath(#[source] std::io::Error),
    /// Failed to open the document.
    #[error("Failed to open the document")]
    FailedToOpenDocument(#[source] windows::core::Error),
    /// Render error.
    #[error("Render error")]
    RenderError(#[source] windows::core::Error),
}

/// A printer that uses [`Windows.Data.Pdf`](https://learn.microsoft.com/en-us/uwp/api/windows.data.pdf?view=winrt-26100) to to print PDF documents.
pub struct WinPdfPrinter {
    printer: PrinterDevice,
}

impl WinPdfPrinter {
    /// Create a new [`WinPdfPrinter`] for the given printer device.
    pub fn new(printer: PrinterDevice) -> Self {
        Self { printer }
    }
}

impl FilePrinter for WinPdfPrinter {
    type Options = PrintTicket;
    type Error = WinPdfPrinterError;
    fn print(&self, path: &Path, options: PrintTicket) -> Result<(), WinPdfPrinterError> {
        self.print_with_progress(path, options, |_, _| {})
    }
}

impl WinPdfPrinter {
    /// Print one PDF as one job, reporting each page after Direct2D accepts it.
    ///
    /// Counts describe pages processed, not physical output or terminal spooler success.
    /// The final result still reports cancellation/failure even after every page was accepted.
    /// The callback runs synchronously on the calling thread; it does not require `Send`.
    pub fn print_with_progress(
        &self,
        path: &Path,
        options: PrintTicket,
        mut progress: impl FnMut(u32, u32),
    ) -> Result<(), WinPdfPrinterError> {
        let context = DxgiPrintContext::new(
            &self.printer,
            &options,
            path.file_name().unwrap_or(path.as_ref()),
        )?;
        let dxgi_device = &context.dxgi_device;
        let print_control = &context.print_control;
        let d2d_context = &context.d2d_context;
        unsafe {
            let absolute_path =
                std::path::absolute(path).map_err(WinPdfPrinterError::InvalidPath)?;
            let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(absolute_path.as_path()))
                .map_err(WinPdfPrinterError::FailedToOpenDocument)?
                .join()
                .map_err(WinPdfPrinterError::FailedToOpenDocument)?;
            let pdf_document = PdfDocument::LoadFromFileAsync(&file)
                .map_err(WinPdfPrinterError::FailedToOpenDocument)?
                .join()
                .map_err(WinPdfPrinterError::FailedToOpenDocument)?;

            let pdf_renderer =
                pdf_create_renderer(dxgi_device).map_err(WinPdfPrinterError::RenderError)?;
            let pdf_render_options = PDF_RENDER_PARAMS {
                IgnoreHighContrast: true.into(),
                ..Default::default()
            };
            let page_count = pdf_document
                .PageCount()
                .map_err(WinPdfPrinterError::RenderError)?;
            progress(0, page_count);
            for i in 0..page_count {
                let pdf_page = pdf_document
                    .GetPage(i)
                    .map_err(WinPdfPrinterError::RenderError)?;

                let command_list = d2d_context
                    .CreateCommandList()
                    .map_err(WinPdfPrinterError::RenderError)?;
                d2d_context.SetTarget(&command_list);
                d2d_context.BeginDraw();
                pdf_renderer
                    .RenderPageToDeviceContext(&pdf_page, d2d_context, Some(&pdf_render_options))
                    .map_err(WinPdfPrinterError::RenderError)?;
                d2d_context
                    .EndDraw(None, None)
                    .map_err(WinPdfPrinterError::RenderError)?;
                command_list
                    .Close()
                    .map_err(WinPdfPrinterError::RenderError)?;

                let pdf_size = pdf_page.Size().map_err(WinPdfPrinterError::RenderError)?;
                let dx_size = D2D_SIZE_F {
                    width: pdf_size.Width as f32,
                    height: pdf_size.Height as f32,
                };
                print_control
                    .AddPage(&command_list, dx_size, None, None, None)
                    .map_err(WinPdfPrinterError::RenderError)?;
                progress(i + 1, page_count);
            }
        }
        context.close_and_wait()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::WinPdfPrinter;
    use crate::{printer::FilePrinter, test_utils::null_device};
    use std::path::Path;

    #[test]
    fn print_simple_pdf_document() {
        let device = null_device::thread_local();
        let pdf = WinPdfPrinter::new(device);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/test_document.pdf");
        pdf.print(path.as_path(), Default::default()).unwrap();
    }
}
