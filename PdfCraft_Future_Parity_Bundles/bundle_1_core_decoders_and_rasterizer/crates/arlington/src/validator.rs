//! Arlington PDF Model Validation Engine.

use std::collections::{HashMap, HashSet};
use std::fmt;
use crate::model::{ArlingtonDictionarySpec, ArlingtonKeySpec, ArlingtonType, PdfVersion};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    MissingRequiredKey { dict: String, key: String },
    TypeMismatch { dict: String, key: String, expected: Vec<ArlingtonType>, actual: ArlingtonType },
    VersionViolation { dict: String, key: String, required_version: PdfVersion, doc_version: PdfVersion },
    DeprecatedKey { dict: String, key: String, deprecated_in: PdfVersion },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequiredKey { dict, key } => {
                write!(f, "Missing required key '{}' in Arlington dictionary '{}'", key, dict)
            }
            Self::TypeMismatch { dict, key, expected, actual } => {
                write!(f, "Type mismatch for '{}/{}': expected {:?}, got {:?}", dict, key, expected, actual)
            }
            Self::VersionViolation { dict, key, required_version, doc_version } => {
                write!(f, "Key '{}/{}' requires PDF {}.{}, but document is {}.{}", dict, key, required_version.major, required_version.minor, doc_version.major, doc_version.minor)
            }
            Self::DeprecatedKey { dict, key, deprecated_in } => {
                write!(f, "Key '{}/{}' was deprecated in PDF {}.{}", dict, key, deprecated_in.major, deprecated_in.minor)
            }
        }
    }
}

pub struct ArlingtonValidator {
    schemas: HashMap<&'static str, ArlingtonDictionarySpec>,
}

impl Default for ArlingtonValidator {
    fn default() -> Self {
        let mut validator = Self {
            schemas: HashMap::new(),
        };
        validator.register_standard_schemas();
        validator
    }
}

impl ArlingtonValidator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_schema(&mut self, spec: ArlingtonDictionarySpec) {
        self.schemas.insert(spec.dict_type, spec);
    }

    /// Validates a dictionary map of key -> (type, value) against an Arlington schema.
    pub fn validate_dict(
        &self,
        dict_name: &str,
        doc_version: PdfVersion,
        entries: &HashMap<String, ArlingtonType>,
    ) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        let schema = match self.schemas.get(dict_name) {
            Some(s) => s,
            None => return errors, // Unknown dictionary type permitted
        };

        // Check required keys
        for key_spec in &schema.keys {
            let key_str = key_spec.key_name;
            let present = entries.get(key_str);

            if key_spec.is_required && present.is_none() {
                // If required in this document version
                if doc_version >= key_spec.since_version {
                    errors.push(ValidationError::MissingRequiredKey {
                        dict: dict_name.into(),
                        key: key_str.into(),
                    });
                }
            }

            if let Some(&actual_type) = present {
                // Check version constraints
                if doc_version < key_spec.since_version {
                    errors.push(ValidationError::VersionViolation {
                        dict: dict_name.into(),
                        key: key_str.into(),
                        required_version: key_spec.since_version,
                        doc_version,
                    });
                }

                if let Some(dep_ver) = key_spec.deprecated_in {
                    if doc_version >= dep_ver {
                        errors.push(ValidationError::DeprecatedKey {
                            dict: dict_name.into(),
                            key: key_str.into(),
                            deprecated_in: dep_ver,
                        });
                    }
                }

                // Check allowed types
                if !key_spec.allowed_types.contains(&actual_type) && actual_type != ArlingtonType::IndirectReference {
                    let mut exp_vec: Vec<ArlingtonType> = key_spec.allowed_types.iter().copied().collect();
                    exp_vec.sort_by_key(|t| format!("{:?}", t));
                    errors.push(ValidationError::TypeMismatch {
                        dict: dict_name.into(),
                        key: key_str.into(),
                        expected: exp_vec,
                        actual: actual_type,
                    });
                }
            }
        }

        errors
    }

    fn register_standard_schemas(&mut self) {
        // Catalog dictionary schema
        let catalog_keys = vec![
            ArlingtonKeySpec {
                key_name: "Type",
                allowed_types: HashSet::from([ArlingtonType::Name]),
                is_required: true,
                since_version: PdfVersion::V1_0,
                deprecated_in: None,
                inherit_from_parent: false,
            },
            ArlingtonKeySpec {
                key_name: "Pages",
                allowed_types: HashSet::from([ArlingtonType::Dictionary, ArlingtonType::IndirectReference]),
                is_required: true,
                since_version: PdfVersion::V1_0,
                deprecated_in: None,
                inherit_from_parent: false,
            },
            ArlingtonKeySpec {
                key_name: "AF",
                allowed_types: HashSet::from([ArlingtonType::Array]),
                is_required: false,
                since_version: PdfVersion::V2_0, // Associated files in PDF 2.0
                deprecated_in: None,
                inherit_from_parent: false,
            },
            ArlingtonKeySpec {
                key_name: "AcroForm",
                allowed_types: HashSet::from([ArlingtonType::Dictionary]),
                is_required: false,
                since_version: PdfVersion::V1_2,
                deprecated_in: None,
                inherit_from_parent: false,
            },
        ];
        self.register_schema(ArlingtonDictionarySpec {
            dict_type: "Catalog",
            keys: catalog_keys,
        });

        // Page dictionary schema
        let page_keys = vec![
            ArlingtonKeySpec {
                key_name: "Type",
                allowed_types: HashSet::from([ArlingtonType::Name]),
                is_required: true,
                since_version: PdfVersion::V1_0,
                deprecated_in: None,
                inherit_from_parent: false,
            },
            ArlingtonKeySpec {
                key_name: "Parent",
                allowed_types: HashSet::from([ArlingtonType::Dictionary, ArlingtonType::IndirectReference]),
                is_required: true,
                since_version: PdfVersion::V1_0,
                deprecated_in: None,
                inherit_from_parent: false,
            },
            ArlingtonKeySpec {
                key_name: "MediaBox",
                allowed_types: HashSet::from([ArlingtonType::Array]),
                is_required: true,
                since_version: PdfVersion::V1_0,
                deprecated_in: None,
                inherit_from_parent: true,
            },
            ArlingtonKeySpec {
                key_name: "Contents",
                allowed_types: HashSet::from([ArlingtonType::Stream, ArlingtonType::Array, ArlingtonType::IndirectReference]),
                is_required: false,
                since_version: PdfVersion::V1_0,
                deprecated_in: None,
                inherit_from_parent: false,
            },
        ];
        self.register_schema(ArlingtonDictionarySpec {
            dict_type: "Page",
            keys: page_keys,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arlington_catalog_validation() {
        let validator = ArlingtonValidator::new();

        // Valid Catalog in PDF 2.0
        let mut entries = HashMap::new();
        entries.insert("Type".into(), ArlingtonType::Name);
        entries.insert("Pages".into(), ArlingtonType::Dictionary);
        entries.insert("AF".into(), ArlingtonType::Array);

        let errors = validator.validate_dict("Catalog", PdfVersion::V2_0, &entries);
        assert!(errors.is_empty());

        // Violating: /AF used in PDF 1.4
        let errs_14 = validator.validate_dict("Catalog", PdfVersion::V1_4, &entries);
        assert_eq!(errs_14.len(), 1);
        match &errs_14[0] {
            ValidationError::VersionViolation { key, .. } => assert_eq!(key, "AF"),
            _ => panic!("Expected version violation"),
        }
    }
}
