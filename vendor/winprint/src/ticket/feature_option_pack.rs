use xml::name::OwnedName;

use super::{
    document::{
        ParameterInit, PrintFeature, PrintFeatureOption, PrintTicketDocument, WithProperties,
        NS_PSK,
    },
    PrintCapabilities, PrintTicket,
};

/// A trait for the predefined name.
pub trait PredefinedName: Sized {
    /// Get the predefined name from the given name.
    /// If the name is not predefined, `None` is returned.
    fn from_name(name: &OwnedName) -> Option<Self>;
}

/// A trait for the feature option pack.
pub trait FeatureOptionPack: Sized {
    /// Create a new instance.
    fn new(option: PrintFeatureOption, parameters: Vec<ParameterInit>) -> Self;

    /// Get the feature name of the option.
    fn feature_name() -> OwnedName;

    /// Get the DOM of the option.
    fn option(&self) -> &PrintFeatureOption;
    /// Get the mutable reference to the DOM of the option.
    fn option_mut(&mut self) -> &mut PrintFeatureOption;

    /// Get the parameters that is used by the option.
    fn parameters(&self) -> &[ParameterInit];
    /// Get the mutable reference to the parameters that is used by the option.
    fn parameters_mut(&mut self) -> &mut Vec<ParameterInit>;

    /// Convert the feature option pack into the option and the parameters.
    fn into_option_with_parameters(self) -> (PrintFeatureOption, Vec<ParameterInit>);

    /// Get display name of the page orientation.
    fn display_name(&self) -> Option<&str> {
        self.option()
            .get_property("DisplayName", Some(NS_PSK))
            .and_then(|x| x.value.as_ref())
            .and_then(|x| x.string())
    }

    /// List all possible options defined in the capabilities.
    fn list(capabilities: &PrintCapabilities) -> impl Iterator<Item = Self> + '_ {
        capabilities
            .options_for_feature(Self::feature_name())
            .map(move |option| {
                let default_parameters = capabilities
                    .default_parameters_for(option.parameters_dependent().as_slice())
                    .collect();
                Self::new(option.clone(), default_parameters)
            })
    }
}

impl<T> From<T> for PrintTicket
where
    T: FeatureOptionPack,
{
    fn from(value: T) -> Self {
        let (option, parameters) = value.into_option_with_parameters();
        PrintTicketDocument {
            properties: vec![],
            parameter_inits: parameters,
            features: vec![PrintFeature {
                name: T::feature_name(),
                properties: vec![],
                options: vec![option],
                features: vec![],
            }],
        }
        .into()
    }
}

/// A trait for the feature option pack with predefined name.
pub trait FeatureOptionPackWithPredefined: FeatureOptionPack {
    /// The type which represents the predefined name.
    type PredefinedName: PredefinedName;

    /// Get the predefined name of the option.
    /// If the option is not predefined, `None` is returned.
    fn as_predefined_name(&self) -> Option<Self::PredefinedName> {
        self.option()
            .name
            .as_ref()
            .and_then(Self::PredefinedName::from_name)
    }
}

/// Implement the [`FeatureOptionPack`] for the given type.
///
/// # Parameters
/// - `$feature_name:expr`: The feature name of the option.
/// - `$name:ident`: The type to define.
/// - `$predefined_name:ident`: The type of predefined name. If not specified, the type is not predefined.
///
/// # Example
/// ```ignore
/// define_feature_option_pack!(
///     OwnedName::qualified("MyFeature", NS_PSK, Some("psk")),
///     MyPack,
///     MyPredefinedName
/// );
/// ```
macro_rules! define_feature_option_pack {
    ($feature_name:expr, $name:ident) => {
        #[derive(Clone, Debug)]
        #[doc = concat!("Represents a feature option pack as [`", stringify!($name), "`].")]
        pub struct $name {
            /// The option of the feature.
            option: PrintFeatureOption,
            /// The parameters that is used by the option.
            parameters: Vec<ParameterInit>,
        }

        impl crate::ticket::FeatureOptionPack for $name {
            fn new(option: PrintFeatureOption, parameters: Vec<ParameterInit>) -> Self {
                Self { option, parameters }
            }

            fn feature_name() -> OwnedName {
                $feature_name
            }

            fn option(&self) -> &PrintFeatureOption {
                &self.option
            }

            fn option_mut(&mut self) -> &mut PrintFeatureOption {
                &mut self.option
            }

            fn parameters(&self) -> &[ParameterInit] {
                &self.parameters
            }

            fn parameters_mut(&mut self) -> &mut Vec<ParameterInit> {
                &mut self.parameters
            }

            fn into_option_with_parameters(self) -> (PrintFeatureOption, Vec<ParameterInit>) {
                (self.option, self.parameters)
            }
        }
    };
    ($feature_name:expr, $name:ident, $predefined_name:ident) => {
        define_feature_option_pack!($feature_name, $name);

        impl crate::ticket::FeatureOptionPackWithPredefined for $name {
            type PredefinedName = $predefined_name;
        }
    };
}
pub(crate) use define_feature_option_pack;
