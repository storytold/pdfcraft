use std::fmt;
use xml::name::OwnedName;

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a Print Schema document.
pub enum PrintSchemaDocument {
    /// Documents that typed as [`PrintCapabilitiesDocument`].
    PrintCapabilities(PrintCapabilitiesDocument),
    /// Documents that typed as [`PrintTicketDocument`].
    PrintTicket(PrintTicketDocument),
}

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a PrintCapabilities document.
pub struct PrintCapabilitiesDocument {
    /// Properties of the document
    pub properties: Vec<Property>,
    /// Parameter definitions
    pub parameter_defs: Vec<ParameterDef>,
    /// Features
    pub features: Vec<PrintFeature>,
}

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a PrintTicket document.
pub struct PrintTicketDocument {
    /// Properties of the document
    pub properties: Vec<Property>,
    /// Parameter initializations
    pub parameter_inits: Vec<ParameterInit>,
    /// Features
    pub features: Vec<PrintFeature>,
}

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a Print Feature.
pub struct PrintFeature {
    /// The name of the feature.
    #[fmt("{}", self.name)]
    pub name: OwnedName,
    /// Properties of the feature
    pub properties: Vec<Property>,
    /// Available options
    pub options: Vec<PrintFeatureOption>,
    /// Sub-features of the feature
    pub features: Vec<PrintFeature>,
}

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a parameter initialization used in a [`PrintTicketDocument`].
pub struct ParameterInit {
    /// The name of the parameter.
    #[fmt("{}", self.name)]
    pub name: OwnedName,
    /// The value of the parameter.
    #[fmt("{:?}", self.value)]
    pub value: PropertyValue,
}

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a parameter definition used in a [`PrintCapabilitiesDocument`].
pub struct ParameterDef {
    /// The name of the parameter.
    #[fmt("{}", self.name)]
    pub name: OwnedName,
    /// Properties of the parameter
    pub properties: Vec<Property>,
}

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a possible option for a [`PrintFeature`].
pub struct PrintFeatureOption {
    /// The name of the option.
    #[fmt("{}", self.name.as_ref().map(|x| x.to_string()).unwrap_or("<unnamed>".to_string()))]
    pub name: Option<OwnedName>,
    /// Scored-properties of the option
    pub scored_properties: Vec<ScoredProperty>,
    /// Properties of the option
    pub properties: Vec<Property>,
}

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a scored-property.
/// A [`ScoredProperty`] declares a property that is intrinsic to an [Option](PrintFeatureOption).
/// Such properties should be compared when evaluating how closely a requested Option matches a device-supported Option.
pub struct ScoredProperty {
    /// The name of the scored-property.
    #[fmt("{}", self.name.as_ref().map(|x| x.to_string()).unwrap_or("<unnamed>".to_string()))]
    pub name: Option<OwnedName>,
    /// The parameter that this scored-property depends on.
    #[fmt("{}", self.parameter_ref.as_ref().map(|x| x.to_string()).unwrap_or("<unnamed>".to_string()))]
    pub parameter_ref: Option<OwnedName>,
    /// The value of the scored-property.
    #[fmt("{}", self.value.as_ref().map(|x| format!("{:?}", x)).unwrap_or("<none>".to_string()))]
    pub value: Option<PropertyValue>,
    /// Sub-scored-properties of the scored-property
    pub scored_properties: Vec<ScoredProperty>,
    /// Properties of the scored-property
    pub properties: Vec<Property>,
}

#[derive(Clone, PartialEq, Eq, Hash, fmt_derive::Debug)]
/// Represents a property.
pub struct Property {
    /// The name of the property.
    #[fmt("{}", self.name)]
    pub name: OwnedName,
    /// The value of the property.
    #[fmt("{}", self.value.as_ref().map(|x| format!("{:?}", x)).unwrap_or("<none>".to_string()))]
    pub value: Option<PropertyValue>,
    /// Sub-properties of the property
    pub properties: Vec<Property>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
/// Represents a property value or a scored-property value.
pub enum PropertyValue {
    /// A string value.
    String(String),
    /// An integer value.
    Integer(i32),
    /// A qualified name value.
    QName(OwnedName),
    /// An unknown-typed value.
    Unknown(OwnedName, String),
}

impl fmt::Debug for PropertyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropertyValue::String(s) => write!(f, "String({:?})", s),
            PropertyValue::Integer(i) => write!(f, "Integer({})", i),
            PropertyValue::QName(q) => write!(f, "QName({})", q),
            PropertyValue::Unknown(n, s) => write!(f, "Unknown({}, {:?})", n, s),
        }
    }
}

impl ParameterDef {
    /// Get the default value of this parameter.
    pub fn default_value(&self) -> Option<&PropertyValue> {
        self.properties
            .iter()
            .find(|x| {
                x.name.local_name == "DefaultValue" && x.name.namespace_ref() == Some(super::NS_PSF)
            })
            .and_then(|x| x.value.as_ref())
    }
}

impl PropertyValue {
    /// Get the `xsi:type` of this value.
    pub fn xsi_type(&self) -> OwnedName {
        match self {
            PropertyValue::String(_) => OwnedName::qualified("string", super::NS_XSD, Some("xsd")),
            PropertyValue::Integer(_) => {
                OwnedName::qualified("integer", super::NS_XSD, Some("xsd"))
            }
            PropertyValue::QName(_) => OwnedName::qualified("QName", super::NS_XSD, Some("xsd")),
            PropertyValue::Unknown(n, _) => n.clone(),
        }
    }
    /// Try as [`PropertyValue::String`] value.
    pub fn string(&self) -> Option<&str> {
        match self {
            PropertyValue::String(s) => Some(s),
            _ => None,
        }
    }
    /// Try as [`PropertyValue::Integer`] value.
    pub fn integer(&self) -> Option<i32> {
        match self {
            PropertyValue::Integer(i) => Some(*i),
            _ => None,
        }
    }
    /// Try as [`PropertyValue::QName`] value.
    pub fn qualified_name(&self) -> Option<&OwnedName> {
        match self {
            PropertyValue::QName(q) => Some(q),
            _ => None,
        }
    }
}

impl PrintFeatureOption {
    /// Collect all parameters that this option depends on.
    pub fn parameters_dependent(&self) -> Vec<OwnedName> {
        let mut result = vec![];
        for scored_property in &self.scored_properties {
            result.extend(scored_property.parameters_dependent());
        }
        result
    }
}

impl ScoredProperty {
    /// Collect all parameters that this scored-property depends on.
    pub fn parameters_dependent(&self) -> Vec<OwnedName> {
        let mut result = vec![];
        if let Some(ref parameter_ref) = self.parameter_ref {
            result.push(parameter_ref.clone());
        }
        for scored_property in &self.scored_properties {
            result.extend(scored_property.parameters_dependent());
        }
        result
    }

    /// Get the value of this scored-property, or the value of the parameter it references.
    pub fn value_with<'a>(&'a self, parameters: &'a [ParameterInit]) -> Option<&'a PropertyValue> {
        if let Some(ref parameter_ref) = self.parameter_ref {
            parameters
                .iter()
                .find(|x| x.name == *parameter_ref)
                .map(|x| &x.value)
        } else {
            self.value.as_ref()
        }
    }
}

impl From<PrintTicketDocument> for PrintSchemaDocument {
    fn from(value: PrintTicketDocument) -> Self {
        PrintSchemaDocument::PrintTicket(value)
    }
}

impl From<PrintCapabilitiesDocument> for PrintSchemaDocument {
    fn from(value: PrintCapabilitiesDocument) -> Self {
        PrintSchemaDocument::PrintCapabilities(value)
    }
}

/// A trait for types that have scored-properties.
pub trait WithScoredProperties {
    /// Get the scored properties.
    fn scored_properties(&self) -> &[ScoredProperty];

    /// Get the scored-property with the given name and namespace.
    fn get_scored_property(&self, name: &str, namespace: Option<&str>) -> Option<&ScoredProperty> {
        self.scored_properties().iter().find(|x| {
            x.name.as_ref().map_or(false, |x| {
                x.local_name == name && x.namespace_ref() == namespace
            })
        })
    }
}

impl WithScoredProperties for PrintFeatureOption {
    fn scored_properties(&self) -> &[ScoredProperty] {
        &self.scored_properties
    }
}

impl WithScoredProperties for ScoredProperty {
    fn scored_properties(&self) -> &[ScoredProperty] {
        &self.scored_properties
    }
}

/// A trait for types that have properties.
pub trait WithProperties {
    /// Get the properties.
    fn properties(&self) -> &[Property];

    /// Get the property with the given name and namespace.
    fn get_property(&self, name: &str, namespace: Option<&str>) -> Option<&Property> {
        self.properties()
            .iter()
            .find(|x| x.name.local_name == name && x.name.namespace_ref() == namespace)
    }
}

impl WithProperties for PrintSchemaDocument {
    fn properties(&self) -> &[Property] {
        match self {
            PrintSchemaDocument::PrintCapabilities(x) => x.properties(),
            PrintSchemaDocument::PrintTicket(x) => x.properties(),
        }
    }
}

impl WithProperties for PrintCapabilitiesDocument {
    fn properties(&self) -> &[Property] {
        &self.properties
    }
}

impl WithProperties for PrintTicketDocument {
    fn properties(&self) -> &[Property] {
        &self.properties
    }
}

impl WithProperties for ParameterDef {
    fn properties(&self) -> &[Property] {
        &self.properties
    }
}

impl WithProperties for PrintFeature {
    fn properties(&self) -> &[Property] {
        &self.properties
    }
}

impl WithProperties for PrintFeatureOption {
    fn properties(&self) -> &[Property] {
        &self.properties
    }
}

impl WithProperties for ScoredProperty {
    fn properties(&self) -> &[Property] {
        &self.properties
    }
}

impl WithProperties for Property {
    fn properties(&self) -> &[Property] {
        &self.properties
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PrintCapabilitiesDocument, PrintSchemaDocument, PrintTicketDocument, Property,
        PropertyValue, WithProperties,
    };
    use xml::name::OwnedName;

    fn new_test_properties() -> Vec<Property> {
        vec![
            Property {
                name: OwnedName::local("Property1"),
                value: Some(PropertyValue::String("Value1".to_string())),
                properties: vec![],
            },
            Property {
                name: OwnedName::qualified("Property2", "http://test.namespace/", Some("test")),
                value: Some(PropertyValue::Integer(2)),
                properties: vec![],
            },
        ]
    }

    fn check_test_properties(w: &impl WithProperties) {
        assert_eq!(w.properties().len(), 2);

        // ensure we can get properties
        assert_eq!(
            w.get_property("Property1", None)
                .and_then(|p| p.value.as_ref())
                .and_then(|v| v.string()),
            Some("Value1")
        );
        assert_eq!(
            w.get_property("Property2", Some("http://test.namespace/"))
                .and_then(|p| p.value.as_ref())
                .and_then(|v| v.integer()),
            Some(2)
        );

        // namespace is handled
        assert!(w
            .get_property("Property1", Some("http://wrong.namespace/"))
            .is_none());
        assert!(w
            .get_property("Property2", Some("http://wrong.namespace/"))
            .is_none());
        assert!(w.get_property("Property2", None).is_none());

        // ensure we can't get properties that don't exist
        assert!(w.get_property("PROPERTY_NOT_EXIST", None).is_none());
    }

    #[test]
    fn get_properties_from_ticket() {
        let document1: PrintSchemaDocument = PrintTicketDocument {
            properties: new_test_properties(),
            parameter_inits: vec![],
            features: vec![],
        }
        .into();
        check_test_properties(&document1);
    }

    #[test]
    fn get_properties_from_capabilities() {
        let document1: PrintSchemaDocument = PrintCapabilitiesDocument {
            properties: new_test_properties(),
            parameter_defs: vec![],
            features: vec![],
        }
        .into();
        check_test_properties(&document1);
    }

    #[test]
    fn get_properties_from_parameter_def() {
        let parameter_def = super::ParameterDef {
            name: OwnedName::local("Test"),
            properties: new_test_properties(),
        };
        check_test_properties(&parameter_def);
    }

    #[test]
    fn get_properties_from_option() {
        let option = super::PrintFeatureOption {
            name: None,
            scored_properties: vec![],
            properties: new_test_properties(),
        };
        check_test_properties(&option);
    }

    #[test]
    fn get_properties_from_feature() {
        let feature = super::PrintFeature {
            name: OwnedName::local("Test"),
            properties: new_test_properties(),
            options: vec![],
            features: vec![],
        };
        check_test_properties(&feature);
    }

    #[test]
    fn get_properties_from_scored_property() {
        let scored_property = super::ScoredProperty {
            name: None,
            parameter_ref: None,
            value: None,
            scored_properties: vec![],
            properties: new_test_properties(),
        };
        check_test_properties(&scored_property);
    }

    #[test]
    fn get_properties_from_property() {
        let property = Property {
            name: OwnedName::local("Test"),
            value: None,
            properties: new_test_properties(),
        };
        check_test_properties(&property);
    }
}
