use super::{
    document::{ParameterInit, PrintTicketDocument, PropertyValue, NS_PSK},
    PrintTicket,
};
use xml::name::OwnedName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
/// Represents the number of copies.
///
/// # Example
/// ```rust
/// use winprint::ticket::Copies;
///
/// let copies = Copies(1);
/// assert_eq!(copies.0, 1);
/// ```
pub struct Copies(pub u16);

impl From<Copies> for PrintTicket {
    fn from(copies: Copies) -> Self {
        PrintTicketDocument {
            properties: vec![],
            parameter_inits: vec![ParameterInit {
                name: OwnedName::qualified("JobCopiesAllDocuments", NS_PSK, Some("psk")),
                value: PropertyValue::Integer(copies.0 as i32),
            }],
            features: vec![],
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        test_utils::null_device,
        ticket::{PrintCapabilities, PrintTicketBuilder},
    };

    #[test]
    fn use_max_copies() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        let copies = capabilities.max_copies().unwrap();
        println!("Max copies: {:?}", copies);
        let mut builder = PrintTicketBuilder::new(&device).unwrap();
        builder.merge(copies).unwrap();
    }
}
