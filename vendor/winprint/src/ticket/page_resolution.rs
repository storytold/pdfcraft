use super::{
    define_feature_option_pack,
    document::{ParameterInit, PrintFeatureOption, WithScoredProperties, NS_PSK},
};
use xml::name::OwnedName;

define_feature_option_pack!(
    OwnedName::qualified("PageResolution", NS_PSK, Some("psk")),
    PageResolution
);

impl PageResolution {
    /// Get the resolution of the page in DPI.
    pub fn dpi(&self) -> (u32, u32) {
        let x = self
            .option
            .get_scored_property("ResolutionX", Some(NS_PSK))
            .and_then(|x| x.value_with(&self.parameters))
            .and_then(|x| x.integer())
            .unwrap_or_default();
        let y = self
            .option
            .get_scored_property("ResolutionY", Some(NS_PSK))
            .and_then(|x| x.value_with(&self.parameters))
            .and_then(|x| x.integer())
            .unwrap_or_default();
        (x as u32, y as u32)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        test_utils::null_device,
        ticket::{PrintCapabilities, PrintTicketBuilder},
    };

    #[test]
    fn use_page_resolution() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        for resolution in capabilities.page_resolutions() {
            let mut builder = PrintTicketBuilder::new(&device).unwrap();
            builder.merge(resolution).unwrap();
        }
    }

    #[test]
    fn get_dpi() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        for resolution in capabilities.page_resolutions() {
            let (x, y) = resolution.dpi();
            assert!(x > 0);
            assert!(y > 0);
        }
    }
}
