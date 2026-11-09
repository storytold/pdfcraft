use super::{document::NS_PSK, PredefinedName};
use std::str::FromStr;
use strum::EnumString;
use xml::name::OwnedName;

#[derive(EnumString, Debug, PartialEq, Eq, Hash, Clone, Copy)]
/// Represents a predefined duplex type.
pub enum PredefinedDuplexType {
    /// One sided printing
    OneSided,
    /// Two sided printing such that the page is flipped parallel to the width direction
    TwoSidedShortEdge,
    /// Two sided printing such that the page is flipped parallel to the height direction
    TwoSidedLongEdge,
}

impl PredefinedName for PredefinedDuplexType {
    /// Get predefined media name from the given name.
    fn from_name(name: &OwnedName) -> Option<Self> {
        if name.namespace_ref() == Some(NS_PSK) {
            Self::from_str(name.local_name.as_str()).ok()
        } else {
            None
        }
    }
}
