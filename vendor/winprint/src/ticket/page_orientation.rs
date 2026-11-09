use super::{
    define_feature_option_pack,
    document::{ParameterInit, PrintFeatureOption, NS_PSK},
    PredefinedPageOrientation,
};
use xml::name::OwnedName;

define_feature_option_pack!(
    OwnedName::qualified("PageOrientation", NS_PSK, Some("psk")),
    PageOrientation,
    PredefinedPageOrientation
);

#[cfg(test)]
mod tests {
    use crate::{
        test_utils::null_device,
        ticket::{
            FeatureOptionPack, FeatureOptionPackWithPredefined, PredefinedPageOrientation,
            PrintCapabilities, PrintTicketBuilder,
        },
    };
    #[test]
    fn use_landscape() {
        let device = null_device::thread_local();
        let capabilities = PrintCapabilities::fetch(&device).unwrap();
        let option = capabilities
            .page_orientations()
            .find(|x| x.as_predefined_name() == Some(PredefinedPageOrientation::Landscape))
            .unwrap();
        println!("Using {:?}", option.display_name());
        let mut builder = PrintTicketBuilder::new(&device).unwrap();
        builder.merge(option).unwrap();
    }
}
