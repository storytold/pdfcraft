use super::{document::NS_PSK, PredefinedName};
use std::str::FromStr;
use strum::EnumString;
use xml::name::OwnedName;

#[derive(EnumString, Debug, PartialEq, Eq, Hash, Clone, Copy)]
/// Represents a predefined page output color.
pub enum PredefinedPageOutputColor {
    /// Specifies the output should be in color.
    Color,
    /// Specifies the output should be in grayscale.
    Grayscale,
    /// Specifies the output should be in monochrome (Black).
    Monochrome,
}

impl PredefinedName for PredefinedPageOutputColor {
    /// Get predefined media name from the given name.
    fn from_name(name: &OwnedName) -> Option<Self> {
        if name.namespace_ref() == Some(NS_PSK) {
            Self::from_str(name.local_name.as_str()).ok()
        } else {
            None
        }
    }
}
