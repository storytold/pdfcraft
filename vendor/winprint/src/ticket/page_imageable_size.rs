use super::{
    document::{
        reader::{ParsableXmlDocument, ParsePrintSchemaError},
        PrintCapabilitiesDocument, WithProperties, NS_PSK,
    },
    FetchPrintCapabilitiesError, MediaSizeTuple, PageMediaSize, PrintCapabilities, PrintTicket,
};
use crate::printer::PrinterDevice;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Represents a media size tuple.
pub struct PageImageableSize {
    /// The size of the page.
    pub size: MediaSizeTuple,
    /// The origin of the imageable area.
    pub origin: MediaSizeTuple,
    /// The extent of the imageable area.
    pub extent: MediaSizeTuple,
}

#[derive(Error, Debug)]
/// Represents an error occurred while fetching imageable size.
pub enum PageImageableSizeError {
    /// Failed to fetch print capabilities.
    #[error("Failed to fetch print capabilities")]
    FetchCapabilitiesError(#[from] FetchPrintCapabilitiesError),
    /// Failed to parse print capabilities.
    #[error("Failed to parse print capabilities")]
    ParseCapabilitiesError(#[from] ParsePrintSchemaError),
    /// Field error.
    #[error("Field error: {field}")]
    FieldError {
        /// Indicates the field name.
        field: &'static str,
    },
}

fn get_u32_property(
    properties: &impl WithProperties,
    name: &'static str,
    ns: Option<&str>,
) -> Result<u32, PageImageableSizeError> {
    properties
        .get_property(name, ns)
        .and_then(|x| x.value.as_ref())
        .and_then(|x| x.integer())
        .and_then(|x| u32::try_from(x).ok())
        .ok_or(PageImageableSizeError::FieldError { field: name })
}

impl PageImageableSize {
    /// Try to fetch the imageable size for the given printer device and media size.
    pub fn try_fetch(
        device: &PrinterDevice,
        media: PageMediaSize,
    ) -> Result<Self, PageImageableSizeError> {
        let ticket = PrintTicket::from(media);
        let caps_xml = PrintCapabilities::fetch_xml_for_ticket(device, Some(&ticket))?;
        let caps = PrintCapabilitiesDocument::parse_from_bytes(&caps_xml)?;
        let imageable_size = caps.get_property("PageImageableSize", Some(NS_PSK)).ok_or(
            PageImageableSizeError::FieldError {
                field: "PageImageableSize",
            },
        )?;
        let size_w = get_u32_property(imageable_size, "ImageableSizeWidth", Some(NS_PSK))?;
        let size_h = get_u32_property(imageable_size, "ImageableSizeHeight", Some(NS_PSK))?;
        let imageable_area = imageable_size
            .get_property("ImageableArea", Some(NS_PSK))
            .ok_or(PageImageableSizeError::FieldError {
                field: "ImageableArea",
            })?;
        let origin_w = get_u32_property(imageable_area, "OriginWidth", Some(NS_PSK))?;
        let origin_h = get_u32_property(imageable_area, "OriginHeight", Some(NS_PSK))?;
        let extent_w = get_u32_property(imageable_area, "ExtentWidth", Some(NS_PSK))?;
        let extent_h = get_u32_property(imageable_area, "ExtentHeight", Some(NS_PSK))?;
        Ok(Self {
            size: MediaSizeTuple::micron(size_w, size_h),
            origin: MediaSizeTuple::micron(origin_w, origin_h),
            extent: MediaSizeTuple::micron(extent_w, extent_h),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PageImageableSize;
    use crate::{test_utils::null_device, ticket::PrintCapabilities};

    #[test]
    fn get_imageable_size() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        for media in capabilities.page_media_sizes() {
            let imageable_size = PageImageableSize::try_fetch(&device, media);
            assert!(imageable_size.is_ok());
            println!("{:#?}", imageable_size);
        }
    }
}
