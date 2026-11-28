//! How a Windows print job reaches the printer (#8529). The standard route hands the print-ready
//! PDF to the system's PDF renderer, which some systems draw blurred (Windows Server 2016); the
//! drawn routes have PdfCraft draw every sheet itself and send it through GDI, as vector calls
//! or as pixels at the printer's resolution.

use super::Job;

/// How a print job reaches a Windows printer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Route {
    /// The system draws the print-ready PDF.
    #[default]
    Standard,
    /// PdfCraft draws each sheet as vector calls; a sheet with transparency is drawn as pixels.
    Vector,
    /// PdfCraft draws each sheet as pixels at the printer's resolution.
    Image,
}

impl Route {
    /// Every route, the default first.
    pub const ALL: [Route; 3] = [Route::Standard, Route::Vector, Route::Image];

    /// The route's name in automation and saved settings.
    pub fn id(self) -> &'static str {
        match self {
            Route::Standard => "standard",
            Route::Vector => "vector",
            Route::Image => "image",
        }
    }

    /// The route named `id`.
    pub fn parse(id: &str) -> Option<Route> {
        Route::ALL.into_iter().find(|r| r.id() == id)
    }
}

pub use craft_print::{DrawnSheet, Error as DrawError, SheetPainter, rgb};

/// Prints the print-ready `pdf` with every sheet drawn by `painter` through GDI, told `(sent,
/// pages)` as each page reaches the spooler; to `output_file` instead of the device when given
/// (drivers that write files, and tests). Windows only: elsewhere an error says so and nothing
/// is sent.
pub fn submit_drawn(
    pdf: &[u8],
    job: &Job,
    painter: &mut dyn SheetPainter,
    output_file: Option<&str>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<String, crate::PrintError> {
    if let Some(dll) = super::missing_print_dll() {
        return Err(super::unavailable(dll));
    }
    #[cfg(windows)]
    let _provider = super::print_ticket_provider();
    #[cfg(windows)]
    let resolved = super::windows_duplex::job(job);
    #[cfg(windows)]
    let job = resolved.as_ref();
    craft_print::submit_drawn(pdf, &super::spooled(job), painter, output_file, progress, &craft_print::Cancel::new())
        .map_err(|e| crate::PrintError::Spool(e.to_string()))
}
