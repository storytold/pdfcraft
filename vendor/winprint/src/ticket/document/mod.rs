mod print_schema;
/// Reader for parse XML bytes as Print Schema.
pub mod reader;
/// Writer for serialize Print Schema as XML bytes.
pub mod writer;

pub use print_schema::*;

/// The namespace URI for the Print Schema Framework.
pub const NS_PSF: &str =
    "http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework";
/// The namespace URI for the Print Schema Keywords.
pub const NS_PSK: &str =
    "http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords";
/// The namespace URI for the XML Schema.
pub const NS_XSD: &str = "http://www.w3.org/2001/XMLSchema";
/// The namespace URI for the XML Schema Instance.
pub const NS_XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// (Re-)export the `OwnedName` type from the `xml` crate.
pub use xml::name::OwnedName;
