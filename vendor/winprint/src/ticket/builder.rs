use super::{PrintTicket, DEFAULT_PRINT_TICKET_XML};
use crate::{
    printer::PrinterDevice,
    utils::{stream::copy_com_stream_to_vec, wchar},
};
use thiserror::Error;
use windows::{
    core::{BSTR, PCWSTR},
    Win32::{
        Graphics::Printing::PrintTicket::{
            kPTJobScope, PTCloseProvider, PTMergeAndValidatePrintTicket, PTOpenProvider,
            HPTPROVIDER,
        },
        UI::Shell::SHCreateMemStream,
    },
};

/// Represents a print ticket builder, which can be used to build a print ticket.
pub struct PrintTicketBuilder {
    xml: Vec<u8>,
    provider: HPTPROVIDER,
}

#[derive(Error, Debug)]
/// Represents an error occurred while building a print ticket.
pub enum PrintTicketBuilderError {
    /// Failed to open print ticket provider.
    #[error("Failed to open print ticket provider")]
    OpenProviderFailed(#[source] windows::core::Error),
    /// Stream not allocated.
    #[error("Stream not allocated")]
    StreamNotAllocated,
    /// Failed to merge print tickets.
    #[error("Failed to merge print tickets")]
    MergePrintTicketsFailed(String, #[source] windows::core::Error),
    /// Failed to decode print ticket.
    #[error("Failed to decode print ticket")]
    DecodePrintTicketFailed(#[source] windows::core::Error),
}

impl PrintTicketBuilder {
    /// Create a new print ticket builder for the given printer device.
    pub fn new(device: &PrinterDevice) -> Result<Self, PrintTicketBuilderError> {
        let provider = unsafe {
            PTOpenProvider(PCWSTR(wchar::to_wide_chars(device.os_name()).as_ptr()), 1)
                .map_err(PrintTicketBuilderError::OpenProviderFailed)?
        };
        Ok(Self {
            xml: DEFAULT_PRINT_TICKET_XML.into(),
            provider,
        })
    }

    /// Merge the given print ticket into the current print ticket.
    pub fn merge(&mut self, delta: impl Into<PrintTicket>) -> Result<(), PrintTicketBuilderError> {
        unsafe {
            let base = SHCreateMemStream(Some(self.xml.as_ref()))
                .ok_or(PrintTicketBuilderError::StreamNotAllocated)?;
            let delta = SHCreateMemStream(Some(delta.into().get_xml()))
                .ok_or(PrintTicketBuilderError::StreamNotAllocated)?;
            let result =
                SHCreateMemStream(None).ok_or(PrintTicketBuilderError::StreamNotAllocated)?;
            let mut error_message = BSTR::default();
            PTMergeAndValidatePrintTicket(
                self.provider,
                &base,
                &delta,
                kPTJobScope,
                Some(&result),
                Some(&mut error_message),
            )
            .map_err(|win32_error| {
                PrintTicketBuilderError::MergePrintTicketsFailed(
                    error_message.to_string(),
                    win32_error,
                )
            })?;
            copy_com_stream_to_vec(&mut self.xml, &result)
                .map_err(PrintTicketBuilderError::DecodePrintTicketFailed)?;
        }
        Ok(())
    }

    /// Build the print ticket.
    pub fn build(mut self) -> Result<PrintTicket, PrintTicketBuilderError> {
        let xml = std::mem::take(&mut self.xml);
        Ok(PrintTicket { xml })
    }
}

impl Drop for PrintTicketBuilder {
    fn drop(&mut self) {
        unsafe {
            let _ = PTCloseProvider(self.provider);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PrintTicketBuilder;
    use crate::{
        test_utils::null_device,
        ticket::{PrintTicket, PrintTicketBuilderError},
    };

    #[test]
    fn merge_simple_ticket() {
        let device = null_device::thread_local();
        let mut builder = PrintTicketBuilder::new(&device).unwrap();
        let delta = r#"<psf:PrintTicket xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema" version="1" xmlns:psk="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords">
    <psf:Feature name="psk:PageMediaSize">
		<psf:Option name="psk:NorthAmericaTabloid">
			<psf:ScoredProperty name="psk:MediaSizeWidth">
				<psf:Value xsi:type="xsd:integer">279400</psf:Value>
			</psf:ScoredProperty>
			<psf:ScoredProperty name="psk:MediaSizeHeight">
				<psf:Value xsi:type="xsd:integer">431800</psf:Value>
			</psf:ScoredProperty>
			<psf:Property name="psk:DisplayName">
				<psf:Value xsi:type="xsd:string">Tabloid</psf:Value>
			</psf:Property>
		</psf:Option>
	</psf:Feature>
</psf:PrintTicket>"#;
        builder.merge(PrintTicket::from_xml(delta)).unwrap();
        let _ticket = builder.build().unwrap();
    }

    #[test]
    fn merge_invalid_ticket() {
        let device = null_device::thread_local();
        let mut builder = PrintTicketBuilder::new(&device).unwrap();
        let delta = r#"This is not a valid print ticket"#;
        let result = builder.merge(PrintTicket::from_xml(delta));
        assert!(matches!(
            result,
            Err(PrintTicketBuilderError::MergePrintTicketsFailed(..))
        ));
    }
}
