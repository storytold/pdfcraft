use super::{
    define_feature_option_pack,
    document::{ParameterInit, PrintFeatureOption, WithScoredProperties, NS_PSK},
    MediaSizeTuple, PredefinedMediaName,
};
use xml::name::OwnedName;

define_feature_option_pack!(
    OwnedName::qualified("PageMediaSize", NS_PSK, Some("psk")),
    PageMediaSize,
    PredefinedMediaName
);

impl PageMediaSize {
    /// Get the size of the media.
    pub fn size(&self) -> MediaSizeTuple {
        let width = self
            .option
            .get_scored_property("MediaSizeWidth", Some(NS_PSK))
            .and_then(|x| x.value_with(&self.parameters))
            .and_then(|x| x.integer())
            .unwrap_or_default();
        let height = self
            .option
            .get_scored_property("MediaSizeHeight", Some(NS_PSK))
            .and_then(|x| x.value_with(&self.parameters))
            .and_then(|x| x.integer())
            .unwrap_or_default();
        MediaSizeTuple::micron(width as u32, height as u32)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        test_utils::null_device,
        ticket::{
            FeatureOptionPack, FeatureOptionPackWithPredefined, PredefinedMediaName,
            PrintCapabilities, PrintTicketBuilder,
        },
    };

    #[test]
    fn use_page_media_size() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        for media in capabilities.page_media_sizes() {
            let mut builder = PrintTicketBuilder::new(&device).unwrap();
            builder.merge(media).unwrap();
        }
    }

    #[test]
    fn get_size() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        for media in capabilities.page_media_sizes() {
            let size = media.size();
            assert!(
                size.width_in_micron() | size.height_in_micron() > 0,
                "Size is zero for {:#?}",
                media
            );
        }
    }

    #[test]
    fn get_display_name() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        for media in capabilities.page_media_sizes() {
            let display_name = media.display_name();
            assert!(
                display_name.is_some(),
                "Display name is not found for {:#?}",
                media
            );
        }
    }

    #[test]
    fn a4_should_be_found() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        capabilities
            .page_media_sizes()
            .find(|x| x.as_predefined_name() == Some(PredefinedMediaName::ISOA4))
            .unwrap();
    }
}
