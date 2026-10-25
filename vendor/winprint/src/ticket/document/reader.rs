use super::{
    ParameterDef, ParameterInit, PrintCapabilitiesDocument, PrintFeature, PrintFeatureOption,
    PrintSchemaDocument, PrintTicketDocument, Property, PropertyValue, ScoredProperty, NS_PSF,
    NS_XSD, NS_XSI,
};
use std::{fmt::Debug, io::Cursor};
use thiserror::Error;
use xml::{
    common::{Position, TextPosition},
    name::OwnedName,
    namespace::Namespace,
    reader::XmlEvent,
    EventReader,
};

#[derive(Error, Debug)]
/// Represents an error occurred while parsing print schema.
pub enum ParsePrintSchemaError {
    /// Invalid XML.
    #[error("Invalid xml")]
    InvalidXml(#[source] xml::reader::Error),
    /// Invalid print schema.
    #[error("Invalid print schema: (at {pos}) {reason}")]
    InvalidPrintSchema {
        /// Position in the document.
        pos: TextPosition,
        /// Reason of the error.
        reason: String,
    },
    /// Wrong document type.
    #[error("Wrong document type: expected {expected} but found {found}")]
    WrongDocumentType {
        /// Expected document type.
        expected: &'static str,
        /// Found document type.
        found: &'static str,
    },
}

/// Represents a root element which can be parsed from XML.
pub trait ParsableXmlDocument: Sized {
    /// The error type that can be returned when parsing fails.
    type Error;

    /// Parse the XML document from the given XML reader.
    fn parse<R>(reader: &mut EventReader<R>) -> Result<Self, Self::Error>
    where
        R: std::io::Read;

    /// Parse the XML document from the given bytes.
    fn parse_from_bytes(xml: impl AsRef<[u8]>) -> Result<Self, Self::Error> {
        let mut reader = EventReader::new(Cursor::new(xml));
        Self::parse(&mut reader)
    }
}

fn parse_qname(namespace: &Namespace, value: &str) -> OwnedName {
    let prefix_index = value.find(':');
    if let Some(prefix_index) = prefix_index {
        let (prefix, local_name) = value.split_at(prefix_index);
        let local_name = &local_name[1..];
        OwnedName {
            local_name: local_name.to_string(),
            namespace: namespace.get(prefix).map(str::to_string),
            prefix: Some(prefix.to_string()),
        }
    } else {
        OwnedName {
            local_name: value.to_string(),
            namespace: None,
            prefix: None,
        }
    }
}

fn parse_name_attribute(
    attributes: &[xml::attribute::OwnedAttribute],
    namespace: &Namespace,
) -> Option<OwnedName> {
    let attribute_value = attributes
        .iter()
        .find(|x| x.name.local_name == "name" && x.name.namespace.is_none())
        .map(|x| x.value.clone());
    attribute_value.map(|x| parse_qname(namespace, &x))
}

fn parse_type_attribute(
    attributes: &[xml::attribute::OwnedAttribute],
    namespace: &Namespace,
) -> Option<OwnedName> {
    let attribute_value = attributes
        .iter()
        .find(|x| x.name.local_name == "type" && x.name.namespace_ref() == Some(NS_XSI))
        .map(|x| x.value.clone());
    attribute_value.map(|x| parse_qname(namespace, &x))
}

struct PsfValueContext {
    pos: TextPosition,
    value: String,
    value_type: OwnedName,
    namespace: Namespace,
}

impl PsfValueContext {
    fn parse(self) -> Result<PropertyValue, ParsePrintSchemaError> {
        if self.value_type.namespace_ref() == Some(NS_XSD) {
            match self.value_type.local_name.as_str() {
                "string" => return Ok(PropertyValue::String(self.value)),
                "integer" => {
                    return self.value.parse().map(PropertyValue::Integer).map_err(|_| {
                        ParsePrintSchemaError::InvalidPrintSchema {
                            pos: self.pos,
                            reason: "Invalid integer".to_string(),
                        }
                    })
                }
                "QName" => {
                    return Ok(PropertyValue::QName(parse_qname(
                        &self.namespace,
                        &self.value,
                    )))
                }
                _ => {}
            };
        }
        Ok(PropertyValue::Unknown(self.value_type, self.value))
    }
}

impl ParsableXmlDocument for PrintSchemaDocument {
    type Error = ParsePrintSchemaError;
    fn parse<R>(reader: &mut EventReader<R>) -> Result<Self, Self::Error>
    where
        R: std::io::Read,
    {
        let mut depth: usize = 0;

        let mut option_name: Option<OwnedName> = None;
        let mut parameter_ref: Option<OwnedName> = None;

        let mut parameter_def_name: Option<OwnedName> = None;
        let mut parameter_def_container: Option<Vec<ParameterDef>> = None;

        let mut parameter_init_name: Option<OwnedName> = None;
        let mut parameter_init_container: Option<Vec<ParameterInit>> = None;

        let mut feature_name: Vec<OwnedName> = Vec::new();
        let mut feature_containers: Vec<Vec<PrintFeature>> = Vec::new();

        let mut option_containers: Vec<Vec<PrintFeatureOption>> = Vec::new();

        let mut property_name: Vec<OwnedName> = Vec::new();
        let mut property_containers: Vec<Vec<Property>> = Vec::new();

        let mut scored_property_name: Vec<Option<OwnedName>> = Vec::new();
        let mut scored_property_containers: Vec<Vec<ScoredProperty>> = Vec::new();

        let mut value_context: Option<PsfValueContext> = None;
        let mut parsed_value: Option<PropertyValue> = None;

        loop {
            let e = match reader.next() {
                Ok(e) => e,
                Err(e) => return Err(ParsePrintSchemaError::InvalidXml(e)),
            };
            match e {
                XmlEvent::StartElement {
                    name,
                    attributes,
                    namespace,
                } => {
                    depth += 1;

                    if name.namespace_ref() == Some(NS_PSF) {
                        match name.local_name.as_str() {
                            "PrintCapabilities" => {
                                if depth > 1 {
                                    return Err(ParsePrintSchemaError::InvalidPrintSchema {
                                        pos: reader.position(),
                                        reason: "PrintCapabilities should be root element"
                                            .to_string(),
                                    });
                                }
                                // root container
                                feature_containers.push(Vec::new());
                                property_containers.push(Vec::new());
                                parameter_def_container.replace(Vec::new());
                            }
                            "PrintTicket" => {
                                if depth > 1 {
                                    return Err(ParsePrintSchemaError::InvalidPrintSchema {
                                        pos: reader.position(),
                                        reason: "PrintTicket should be root element".to_string(),
                                    });
                                }
                                // root container
                                feature_containers.push(Vec::new());
                                property_containers.push(Vec::new());
                                parameter_init_container.replace(Vec::new());
                            }
                            "ParameterDef" => {
                                parameter_def_name = parse_name_attribute(&attributes, &namespace);
                                property_containers.push(Vec::new());
                            }
                            "ParameterInit" => {
                                parameter_init_name = parse_name_attribute(&attributes, &namespace);
                            }
                            "Feature" => {
                                feature_name.push(
                                    parse_name_attribute(&attributes, &namespace).ok_or_else(
                                        || ParsePrintSchemaError::InvalidPrintSchema {
                                            pos: reader.position(),
                                            reason: "Feature name not found".to_string(),
                                        },
                                    )?,
                                );

                                // for sub-elements
                                feature_containers.push(Vec::new());
                                property_containers.push(Vec::new());
                                option_containers.push(Vec::new());
                            }
                            "Option" => {
                                option_name = parse_name_attribute(&attributes, &namespace);
                                property_containers.push(Vec::new());
                                scored_property_containers.push(Vec::new());
                            }
                            "ParameterRef" => {
                                parameter_ref = parse_name_attribute(&attributes, &namespace);
                            }
                            "ScoredProperty" => {
                                scored_property_name
                                    .push(parse_name_attribute(&attributes, &namespace));

                                // for sub-elements
                                property_containers.push(Vec::new());
                                scored_property_containers.push(Vec::new());

                                // clear previous value
                                parsed_value.take();
                                parameter_ref.take();
                            }
                            "Property" => {
                                property_name.push(
                                    parse_name_attribute(&attributes, &namespace).ok_or_else(
                                        || ParsePrintSchemaError::InvalidPrintSchema {
                                            pos: reader.position(),
                                            reason: "Property name not found".to_string(),
                                        },
                                    )?,
                                );

                                // for sub-elements
                                property_containers.push(Vec::new());

                                // clear previous value
                                parsed_value.take();
                            }
                            "Value" => {
                                if let Some(value_type) =
                                    parse_type_attribute(&attributes, &namespace)
                                {
                                    value_context.replace(PsfValueContext {
                                        pos: reader.position(),
                                        value: String::new(),
                                        value_type,
                                        namespace,
                                    });
                                }
                            }
                            _ => {
                                return Err(ParsePrintSchemaError::InvalidPrintSchema {
                                    pos: reader.position(),
                                    reason: format!("Invalid element: {}", name),
                                })
                            }
                        }
                    }
                }
                XmlEvent::EndElement { name } => {
                    depth -= 1;

                    if name.namespace_ref() == Some(NS_PSF) {
                        match name.local_name.as_str() {
                            "PrintCapabilities" => {
                                return Ok(PrintCapabilitiesDocument {
                                    parameter_defs: parameter_def_container.unwrap(),
                                    features: feature_containers.pop().unwrap(),
                                    properties: property_containers.pop().unwrap(),
                                }
                                .into());
                            }
                            "PrintTicket" => {
                                return Ok(PrintTicketDocument {
                                    parameter_inits: parameter_init_container.unwrap(),
                                    features: feature_containers.pop().unwrap(),
                                    properties: property_containers.pop().unwrap(),
                                }
                                .into());
                            }
                            "ParameterDef" => {
                                // element should be paired, so it's safe to unwrap
                                let parameter_def_name = parameter_def_name.take().unwrap();
                                let properties = property_containers.pop().unwrap();

                                let parent = parameter_def_container.as_mut().ok_or_else(|| {
                                    ParsePrintSchemaError::InvalidPrintSchema {
                                        pos: reader.position(),
                                        reason: "ParameterDef cannot be here".to_string(),
                                    }
                                })?;
                                parent.push(ParameterDef {
                                    name: parameter_def_name,
                                    properties,
                                });
                            }
                            "ParameterInit" => {
                                // element should be paired, so it's safe to unwrap
                                let parameter_init_name = parameter_init_name.take().unwrap();

                                // value may not be found
                                // check it, and if not found, return error
                                let value = parsed_value.take().ok_or_else(|| {
                                    ParsePrintSchemaError::InvalidPrintSchema {
                                        pos: reader.position(),
                                        reason: "ParameterInit value not found".to_string(),
                                    }
                                })?;

                                let parent =
                                    parameter_init_container.as_mut().ok_or_else(|| {
                                        ParsePrintSchemaError::InvalidPrintSchema {
                                            pos: reader.position(),
                                            reason: "ParameterInit cannot be here".to_string(),
                                        }
                                    })?;
                                parent.push(ParameterInit {
                                    name: parameter_init_name,
                                    value,
                                });
                            }
                            "Feature" => {
                                // element should be paired, so it's safe to unwrap
                                let frature_name = feature_name.pop().unwrap();
                                let features = feature_containers.pop().unwrap();
                                let properties = property_containers.pop().unwrap();
                                let options = option_containers.pop().unwrap();

                                let parent = feature_containers.last_mut().ok_or_else(|| {
                                    ParsePrintSchemaError::InvalidPrintSchema {
                                        pos: reader.position(),
                                        reason: "Feature cannot be here".to_string(),
                                    }
                                })?;
                                parent.push(PrintFeature {
                                    name: frature_name,
                                    properties,
                                    options,
                                    features,
                                });
                            }
                            "Option" => {
                                // element should be paired, so it's safe to unwrap
                                let option_name = option_name.take();
                                let properties = property_containers.pop().unwrap();
                                let scored_properties = scored_property_containers.pop().unwrap();

                                let parent = option_containers.last_mut().ok_or_else(|| {
                                    ParsePrintSchemaError::InvalidPrintSchema {
                                        pos: reader.position(),
                                        reason: "Option cannot be here".to_string(),
                                    }
                                })?;
                                parent.push(PrintFeatureOption {
                                    name: option_name,
                                    scored_properties,
                                    properties,
                                });
                            }
                            "ScoredProperty" => {
                                // element should be paired, so it's safe to unwrap
                                let scored_property_name = scored_property_name.pop().unwrap();
                                let properties = property_containers.pop().unwrap();
                                let scored_properties = scored_property_containers.pop().unwrap();

                                let parent =
                                    scored_property_containers.last_mut().ok_or_else(|| {
                                        ParsePrintSchemaError::InvalidPrintSchema {
                                            pos: reader.position(),
                                            reason: "ScoredProperty cannot be here".to_string(),
                                        }
                                    })?;
                                parent.push(ScoredProperty {
                                    name: scored_property_name,
                                    parameter_ref: parameter_ref.take(),
                                    value: parsed_value.take(),
                                    properties,
                                    scored_properties,
                                });
                            }
                            "Property" => {
                                // element should be paired, so it's safe to unwrap
                                let property_name = property_name.pop().unwrap();
                                let properties = property_containers.pop().unwrap();

                                let parent = property_containers.last_mut().ok_or_else(|| {
                                    ParsePrintSchemaError::InvalidPrintSchema {
                                        pos: reader.position(),
                                        reason: "Property cannot be here".to_string(),
                                    }
                                })?;
                                parent.push(Property {
                                    name: property_name,
                                    value: parsed_value.take(),
                                    properties,
                                });
                            }
                            "Value" => {
                                if let Some(value_context) = value_context.take() {
                                    parsed_value.replace(value_context.parse()?);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                XmlEvent::Characters(s) => {
                    if let Some(c) = value_context.as_mut() {
                        c.value.push_str(&s);
                    }
                }
                XmlEvent::Whitespace(_) => {
                    if let Some(c) = value_context.as_mut() {
                        c.value.push(' ');
                    }
                }
                XmlEvent::CData(s) => {
                    if let Some(c) = value_context.as_mut() {
                        c.value.push_str(&s);
                    }
                }
                XmlEvent::EndDocument => break,
                _ => {}
            }
        }

        Err(ParsePrintSchemaError::InvalidPrintSchema {
            pos: reader.position(),
            reason: "No valid root element found".to_string(),
        })
    }
}

impl ParsableXmlDocument for PrintCapabilitiesDocument {
    type Error = ParsePrintSchemaError;
    fn parse<R>(reader: &mut EventReader<R>) -> Result<Self, Self::Error>
    where
        R: std::io::Read,
    {
        PrintSchemaDocument::parse(reader).and_then(|x| match x {
            PrintSchemaDocument::PrintCapabilities(document) => Ok(document),
            PrintSchemaDocument::PrintTicket(_) => Err(ParsePrintSchemaError::WrongDocumentType {
                expected: "PrintCapabilities",
                found: "PrintTicket",
            }),
        })
    }
}

impl ParsableXmlDocument for PrintTicketDocument {
    type Error = ParsePrintSchemaError;
    fn parse<R>(reader: &mut EventReader<R>) -> Result<Self, Self::Error>
    where
        R: std::io::Read,
    {
        PrintSchemaDocument::parse(reader).and_then(|x| match x {
            PrintSchemaDocument::PrintTicket(document) => Ok(document),
            PrintSchemaDocument::PrintCapabilities(_) => {
                Err(ParsePrintSchemaError::WrongDocumentType {
                    expected: "PrintTicket",
                    found: "PrintCapabilities",
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ParsableXmlDocument, ParsePrintSchemaError};
    use crate::ticket::document::{
        PrintCapabilitiesDocument, PrintSchemaDocument, PrintTicketDocument,
    };

    #[test]
    fn wrong_type_should_return_error() {
        let xml = include_bytes!("../../../test_data/print_ticket.xml");
        let result = PrintCapabilitiesDocument::parse_from_bytes(xml);
        assert!(matches!(
            result,
            Err(ParsePrintSchemaError::WrongDocumentType { .. })
        ));

        let xml = include_bytes!("../../../test_data/print_capabilities.xml");
        let result = PrintTicketDocument::parse_from_bytes(xml);
        assert!(matches!(
            result,
            Err(ParsePrintSchemaError::WrongDocumentType { .. })
        ));
    }

    #[test]
    fn parameter_def_should_not_in_print_ticket() {
        let xml = r#"<psf:PrintTicket version="1"
    xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework" 
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema" 
    xmlns:psk="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords">
    <psf:ParameterDef name="psk:JobCopiesAllDocuments">
        <psf:Property name="psf:DataType">
            <psf:Value xsi:type="xsd:QName">xsd:integer</psf:Value>
        </psf:Property>
        <psf:Property name="psf:UnitType">
            <psf:Value xsi:type="xsd:string">copies</psf:Value>
        </psf:Property>
        <psf:Property name="psf:Multiple">
            <psf:Value xsi:type="xsd:integer">1</psf:Value>
        </psf:Property>
        <psf:Property name="psf:MaxValue">
            <psf:Value xsi:type="xsd:integer">9999</psf:Value>
        </psf:Property>
        <psf:Property name="psf:MinValue">
            <psf:Value xsi:type="xsd:integer">1</psf:Value>
        </psf:Property>
        <psf:Property name="psf:DefaultValue">
            <psf:Value xsi:type="xsd:integer">1</psf:Value>
        </psf:Property>
        <psf:Property name="psf:Mandatory">
            <psf:Value xsi:type="xsd:QName">psk:Unconditional</psf:Value>
        </psf:Property>
        <psf:Property name="psk:DisplayName">
            <psf:Value xsi:type="xsd:string">份数</psf:Value>
        </psf:Property>
    </psf:ParameterDef>
</psf:PrintTicket>"#;
        let result = PrintSchemaDocument::parse_from_bytes(xml);
        assert!(matches!(
            result,
            Err(ParsePrintSchemaError::InvalidPrintSchema { .. })
        ));
    }

    #[test]
    fn parameter_init_should_not_in_print_capabilities() {
        let xml = r#"<psf:PrintCapabilities version="1"
    xmlns:psf="http://schemas.microsoft.com/windows/2003/08/printing/printschemaframework" 
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
    xmlns:xsd="http://www.w3.org/2001/XMLSchema" 
    xmlns:psk="http://schemas.microsoft.com/windows/2003/08/printing/printschemakeywords">
    <psf:ParameterInit name="psk:PageMediaSizeMediaSizeWidth">
        <psf:Value xsi:type="xsd:integer">2540</psf:Value>
    </psf:ParameterInit>
</psf:PrintCapabilities>"#;
        let result = PrintSchemaDocument::parse_from_bytes(xml);
        assert!(matches!(
            result,
            Err(ParsePrintSchemaError::InvalidPrintSchema { .. })
        ));
    }

    #[test]
    fn parse_print_ticket() {
        let xml = include_bytes!("../../../test_data/print_ticket.xml");
        let _document = PrintTicketDocument::parse_from_bytes(xml).unwrap();
    }

    #[test]
    fn parse_print_capabilities() {
        let xml = include_bytes!("../../../test_data/print_capabilities.xml");
        let _document = PrintCapabilitiesDocument::parse_from_bytes(xml).unwrap();
    }
}
