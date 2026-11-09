use super::{
    define_feature_option_pack,
    document::{ParameterInit, PrintFeatureOption, NS_PSK},
    PredefinedDuplexType,
};
use xml::name::OwnedName;

define_feature_option_pack!(
    OwnedName::qualified("JobDuplexAllDocumentsContiguously", NS_PSK, Some("psk")),
    JobDuplex,
    PredefinedDuplexType
);
