use super::{
    define_feature_option_pack,
    document::{ParameterInit, PrintFeatureOption, NS_PSK},
    PredefinedPageOutputColor,
};
use xml::name::OwnedName;

define_feature_option_pack!(
    OwnedName::qualified("PageOutputColor", NS_PSK, Some("psk")),
    PageOutputColor,
    PredefinedPageOutputColor
);
