//! Windows 64-bit TWAIN 2.x scanner support for PdfCraft.

pub mod dib;

#[cfg(all(target_os = "windows", target_pointer_width = "64"))]
mod win;

/// Color mode requested from the TWAIN scanner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelMode {
    Color,
    Grayscale,
    BlackAndWhite,
}

/// Paper source requested from the TWAIN scanner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaperSource {
    /// Leave the scanner driver's default source unchanged.
    DriverDefault,
    /// Flatbed glass.
    Flatbed,
    /// Automatic Document Feeder (single-sided).
    Feeder,
    /// Automatic Document Feeder (double-sided / duplex).
    FeederDuplex,
}

/// A discovered TWAIN data source (scanner driver).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwainDevice {
    pub name: String,
    pub manufacturer: String,
    pub product_family: String,
}

/// Acquisition request parameters.
#[derive(Clone, Debug)]
pub struct ScanRequest {
    /// Exact TWAIN `ProductName` to open, or `None` to open the default TWAIN source.
    pub device_name: Option<String>,
    /// Requested resolution in DPI (e.g. 150, 200, 300, 400, 600).
    pub dpi: u16,
    /// Requested color mode.
    pub pixel_mode: PixelMode,
    /// Requested paper source.
    pub source: PaperSource,
    /// If true, display the scanner driver's native TWAIN UI dialog.
    pub show_ui: bool,
}

impl Default for ScanRequest {
    fn default() -> Self {
        Self { device_name: None, dpi: 600, pixel_mode: PixelMode::Color, source: PaperSource::DriverDefault, show_ui: true }
    }
}

/// One scanned page returned as a standalone BMP image with its physical DPI.
#[derive(Clone, Debug)]
pub struct ScannedPage {
    pub bmp_bytes: Vec<u8>,
    pub dpi_x: f64,
    pub dpi_y: f64,
}

/// Pages acquired from a TWAIN session, along with an optional incomplete-batch notice.
#[derive(Clone, Debug)]
pub struct ScanBatch {
    pub pages: Vec<ScannedPage>,
    /// Set when acquisition stopped early after one or more pages were already transferred
    /// (for example due to a later transfer error, `MSG_ENDXFER` failure, memory cap, or cancellation).
    pub incomplete_reason: Option<String>,
}

/// Errors produced by the TWAIN backend.
#[derive(Debug, thiserror::Error)]
pub enum TwainError {
    #[error("TWAIN scanning is only available on 64-bit Windows")]
    Unsupported,
    #[error("TWAIN Data Source Manager (TWAINDSM.dll) was not found. Copy 64-bit TWAINDSM.dll next to pdfcraft.exe or into System32.")]
    DsmNotFound,
    #[error("No 64-bit TWAIN scanners were found")]
    NoDevices,
    #[error("Scanning was cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

/// List all installed 64-bit TWAIN scanner drivers on this machine.
pub fn list_devices() -> Result<Vec<TwainDevice>, TwainError> {
    #[cfg(all(target_os = "windows", target_pointer_width = "64"))]
    {
        win::list_devices()
    }
    #[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
    {
        Err(TwainError::Unsupported)
    }
}

/// Acquire one or more pages from the requested 64-bit TWAIN scanner.
///
/// Runs its own Win32 message loop and blocks until scanning completes or `cancel` is set.
/// Cancellation is checked before enabling the source, between event-loop waits, and between pages;
/// setting `cancel` cannot interrupt an in-flight `DAT_IMAGENATIVEXFER` call until the scanner driver returns.
pub fn acquire(request: &ScanRequest, cancel: &std::sync::atomic::AtomicBool) -> Result<ScanBatch, TwainError> {
    #[cfg(all(target_os = "windows", target_pointer_width = "64"))]
    {
        win::acquire(request, cancel)
    }
    #[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
    {
        let _ = (request, cancel);
        Err(TwainError::Unsupported)
    }
}
