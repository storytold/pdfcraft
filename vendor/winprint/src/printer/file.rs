use std::path::Path;

/// A trait representing a printer that can print files.
pub trait FilePrinter {
    /// The options type for the printer.
    type Options;
    /// The error type for the printer.
    type Error;
    /// Print the file with the given options.
    fn print(&self, path: &Path, options: Self::Options) -> std::result::Result<(), Self::Error>;
}
