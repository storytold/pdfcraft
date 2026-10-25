use super::{
    document::{
        reader::{ParsableXmlDocument, ParsePrintSchemaError},
        ParameterInit, PrintCapabilitiesDocument, PrintFeatureOption, WithProperties, NS_PSF,
        NS_PSK,
    },
    Copies, FeatureOptionPack, JobDuplex, PageMediaSize, PageOrientation, PageOutputColor,
    PageResolution, PrintTicket,
};
use crate::{
    printer::PrinterDevice,
    utils::{stream::read_com_stream, wchar},
};
use scopeguard::defer;
use std::fmt::Debug;
use thiserror::Error;
use windows::{
    core::{BSTR, PCWSTR},
    Win32::{
        Graphics::Printing::PrintTicket::{
            PTCloseProvider, PTGetPrintCapabilities, PTOpenProvider,
        },
        UI::Shell::SHCreateMemStream,
    },
};
use xml::name::OwnedName;

#[derive(Error, Debug)]
/// Represents an error occurred while fetching print capabilities.
pub enum FetchPrintCapabilitiesError {
    /// Failed to open print ticket provider.
    #[error("Failed to open print ticket provider")]
    OpenProviderFailed(#[source] windows::core::Error),
    /// Stream not allocated.
    #[error("Stream not allocated")]
    StreamNotAllocated,
    /// Cannot get print capabilities.
    #[error("Cannot get print capabilities")]
    CannotGetPrintCapabilities(String, #[source] windows::core::Error),
    /// Failed to read stream.
    #[error("Failed to read stream")]
    ReadStreamFailed(#[source] windows::core::Error),
    /// Failed to parse print capabilities.
    #[error("Failed to parse print capabilities")]
    ParseError(#[source] ParsePrintSchemaError),
}

#[derive(Clone, Debug)]
/// Represents print capabilities.
pub struct PrintCapabilities {
    /// DOM of print capabilities document.
    pub document: PrintCapabilitiesDocument,
}

impl PrintCapabilities {
    /// Fetch print capabilities XML (without parsing it) for the given printer device.
    pub fn fetch_xml(device: &PrinterDevice) -> Result<Vec<u8>, FetchPrintCapabilitiesError> {
        Self::fetch_xml_for_ticket(device, None)
    }

    /// Fetch print capabilities XML (without parsing it) for the given printer device and print ticket.
    pub fn fetch_xml_for_ticket(
        device: &PrinterDevice,
        ticket: Option<&PrintTicket>,
    ) -> Result<Vec<u8>, FetchPrintCapabilitiesError> {
        unsafe {
            let provider =
                PTOpenProvider(PCWSTR(wchar::to_wide_chars(device.os_name()).as_ptr()), 1)
                    .map_err(FetchPrintCapabilitiesError::OpenProviderFailed)?;
            defer! {
                let _ = PTCloseProvider(provider);
            }
            let stream =
                SHCreateMemStream(None).ok_or(FetchPrintCapabilitiesError::StreamNotAllocated)?;
            let ticket_stream = ticket
                .map(|x| {
                    SHCreateMemStream(Some(x.get_xml()))
                        .ok_or(FetchPrintCapabilitiesError::StreamNotAllocated)
                })
                .transpose()?;

            let mut error_message = BSTR::default();
            PTGetPrintCapabilities(
                provider,
                ticket_stream.as_ref(),
                &stream,
                Some(&mut error_message),
            )
            .map_err(|win32_error| {
                FetchPrintCapabilitiesError::CannotGetPrintCapabilities(
                    error_message.to_string(),
                    win32_error,
                )
            })?;

            let data =
                read_com_stream(&stream).map_err(FetchPrintCapabilitiesError::ReadStreamFailed)?;

            Ok(data)
        }
    }

    /// Fetch and parse print capabilities for the given printer device.
    pub fn fetch(device: &PrinterDevice) -> Result<PrintCapabilities, FetchPrintCapabilitiesError> {
        let xml = Self::fetch_xml(device)?;
        let document = PrintCapabilitiesDocument::parse_from_bytes(xml)
            .map_err(FetchPrintCapabilitiesError::ParseError)?;
        Ok(PrintCapabilities { document })
    }

    /// Defines all parameters with default values.
    pub fn default_parameters(&self) -> impl Iterator<Item = ParameterInit> + '_ {
        self.document.parameter_defs.iter().filter_map(|param_def| {
            param_def
                .default_value()
                .map(|default_value| ParameterInit {
                    name: param_def.name.clone(),
                    value: default_value.clone(),
                })
        })
    }

    /// Defines the given parameters with default values.
    pub fn default_parameters_for<'a>(
        &'a self,
        filters: &'a [OwnedName],
    ) -> impl Iterator<Item = ParameterInit> + 'a {
        let mut filters = filters
            .iter()
            .map(|x| (&x.namespace, &x.local_name))
            .collect::<Vec<_>>();
        filters.sort_unstable();
        self.document
            .parameter_defs
            .iter()
            .filter(move |param_def| {
                filters
                    .binary_search(&(&param_def.name.namespace, &param_def.name.local_name))
                    .is_ok()
            })
            .filter_map(move |param_def| {
                param_def
                    .default_value()
                    .map(|default_value| ParameterInit {
                        name: param_def.name.clone(),
                        value: default_value.clone(),
                    })
            })
    }

    /// Get all options for the given feature.
    pub fn options_for_feature(
        &self,
        feature_name: OwnedName,
    ) -> impl Iterator<Item = &PrintFeatureOption> + '_ {
        self.document
            .features
            .iter()
            .filter(move |x| {
                x.name.local_name == feature_name.local_name
                    && x.name.namespace == feature_name.namespace
            })
            .flat_map(|x| x.options.iter())
    }

    /// Get all page media sizes.
    pub fn page_media_sizes(&self) -> impl Iterator<Item = PageMediaSize> + '_ {
        PageMediaSize::list(self)
    }

    /// Get all supported page orientations.
    pub fn page_orientations(&self) -> impl Iterator<Item = PageOrientation> + '_ {
        PageOrientation::list(self)
    }

    /// Get all supported job duplex types.
    ///
    /// # Note
    /// This corresponds to the Print Schema's `JobDuplexAllDocumentsContiguously` keyword, not the `DocumentDuplex` keyword.
    pub fn duplexes(&self) -> impl Iterator<Item = JobDuplex> + '_ {
        JobDuplex::list(self)
    }

    /// Get all supported page output colors.
    pub fn page_output_colors(&self) -> impl Iterator<Item = PageOutputColor> + '_ {
        PageOutputColor::list(self)
    }

    /// Get all supported page resolutions.
    pub fn page_resolutions(&self) -> impl Iterator<Item = PageResolution> + '_ {
        PageResolution::list(self)
    }

    /// Get the maximum number of copies that a printer can print. Return `None` if the device does not report a maximum.
    ///
    /// # Note
    /// This corresponds to the Print Schema's `JobCopiesAllDocuments` keyword, not the `DocumentCopiesAllPages` keyword, or the `PageCopies` keyword. If the printer can print unlimited copies, the property value is 9999.
    pub fn max_copies(&self) -> Option<Copies> {
        println!("{:#?}", self.document.parameter_defs);
        self.document
            .parameter_defs
            .iter()
            .find(|x| {
                x.name.local_name == "JobCopiesAllDocuments"
                    && x.name.namespace_ref() == Some(NS_PSK)
            })
            .and_then(|x| x.get_property("MaxValue", Some(NS_PSF)))
            .and_then(|x| x.value.as_ref())
            .and_then(|x| x.integer())
            .and_then(|x| u16::try_from(x).ok())
            .map(Copies)
    }
}

#[cfg(test)]
mod tests {
    use super::PrintCapabilities;
    use crate::test_utils::null_device;
    #[test]
    fn test_fetch_xml() {
        let device = null_device::thread_local();
        PrintCapabilities::fetch_xml(&device).unwrap();
    }

    #[test]
    fn test_fetch_xml_and_parse() {
        let device = null_device::thread_local();
        PrintCapabilities::fetch(&device).unwrap();
    }
}
