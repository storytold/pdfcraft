use super::DxgiPrintContext;
use super::DxgiPrintContextError;
use crate::printer::FilePrinter;
use crate::printer::PrinterDevice;
use crate::ticket::PrintTicket;
use crate::utils::wchar;
use std::path::Path;
use thiserror::Error;
use windows::core::PCWSTR;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_F;
use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC;
use windows::Win32::Graphics::Imaging::GUID_WICPixelFormat32bppPBGRA;
use windows::Win32::Graphics::Imaging::WICBitmapDitherTypeNone;
use windows::Win32::Graphics::Imaging::WICBitmapPaletteTypeMedianCut;
use windows::Win32::Graphics::Imaging::WICDecodeMetadataCacheOnDemand;

#[derive(Error, Debug)]
/// Represents an error from [`ImagePrinter`].
pub enum ImagePrinterError {
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

/// A printer that prints images. Multiple frames in a single image file will be printed as separate pages.
pub struct ImagePrinter {
    printer: PrinterDevice,
}

impl ImagePrinter {
    /// Create a new [`ImagePrinter`] for the given printer device.
    pub fn new(printer: PrinterDevice) -> Self {
        Self { printer }
    }
}

impl FilePrinter for ImagePrinter {
    type Options = PrintTicket;
    type Error = ImagePrinterError;
    fn print(
        &self,
        path: &Path,
        options: PrintTicket,
    ) -> std::result::Result<(), ImagePrinterError> {
        let context = DxgiPrintContext::new(
            &self.printer,
            &options,
            path.file_name().unwrap_or(path.as_ref()),
        )?;
        let wic_factory = &context.wic_factory;
        let print_control = &context.print_control;
        let d2d_context = &context.d2d_context;
        unsafe {
            let absolute_path =
                std::path::absolute(path).map_err(ImagePrinterError::InvalidPath)?;
            let image_decoder = wic_factory
                .CreateDecoderFromFilename(
                    PCWSTR(wchar::to_wide_chars(absolute_path.as_os_str()).as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(ImagePrinterError::FailedToOpenDocument)?;

            let page_count = image_decoder
                .GetFrameCount()
                .map_err(ImagePrinterError::RenderError)?;
            for i in 0..page_count {
                let frame = image_decoder
                    .GetFrame(i)
                    .map_err(ImagePrinterError::RenderError)?;

                let mut image_width = 0;
                let mut image_height = 0;
                frame
                    .GetSize(&mut image_width, &mut image_height)
                    .map_err(ImagePrinterError::RenderError)?;
                let mut image_dpi_x = 0.0;
                let mut image_dpi_y = 0.0;
                frame
                    .GetResolution(&mut image_dpi_x, &mut image_dpi_y)
                    .map_err(ImagePrinterError::RenderError)?;

                let page_size = D2D_SIZE_F {
                    width: (image_width as f64 * 96.0 / image_dpi_x) as f32,
                    height: (image_height as f64 * 96.0 / image_dpi_y) as f32,
                };

                let format_converter = wic_factory
                    .CreateFormatConverter()
                    .map_err(ImagePrinterError::RenderError)?;
                format_converter
                    .Initialize(
                        &frame,
                        &GUID_WICPixelFormat32bppPBGRA,
                        WICBitmapDitherTypeNone,
                        None,
                        0.0,
                        WICBitmapPaletteTypeMedianCut,
                    )
                    .map_err(ImagePrinterError::RenderError)?;
                let bitmap = d2d_context
                    .CreateBitmapFromWicBitmap(&format_converter, None)
                    .map_err(ImagePrinterError::RenderError)?;

                let command_list = d2d_context
                    .CreateCommandList()
                    .map_err(ImagePrinterError::RenderError)?;
                d2d_context.SetTarget(&command_list);

                d2d_context.BeginDraw();
                d2d_context.DrawBitmap(
                    &bitmap,
                    Some(&D2D_RECT_F {
                        left: 0.0,
                        top: 0.0,
                        right: page_size.width,
                        bottom: page_size.height,
                    }),
                    1.0,
                    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                    None,
                    None,
                );
                d2d_context
                    .EndDraw(None, None)
                    .map_err(ImagePrinterError::RenderError)?;
                command_list
                    .Close()
                    .map_err(ImagePrinterError::RenderError)?;
                print_control
                    .AddPage(&command_list, page_size, None, None, None)
                    .map_err(ImagePrinterError::RenderError)?;
            }
        }
        context.close_and_wait()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ImagePrinter;
    use crate::{printer::FilePrinter, test_utils::null_device};
    use std::path::Path;

    #[test]
    fn print_simple_tiff_document() {
        let device = null_device::thread_local();
        let image = ImagePrinter::new(device);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/test_document.tiff");
        image.print(path.as_path(), Default::default()).unwrap();
    }
}
