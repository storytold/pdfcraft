//! Arlington PDF Model Definitions per DARPA SafeDocs Arlington Schema.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArlingtonType {
    Boolean,
    Integer,
    Number,
    String,
    Name,
    Array,
    Dictionary,
    Stream,
    Null,
    IndirectReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PdfVersion {
    pub major: u8,
    pub minor: u8,
}

impl PdfVersion {
    pub const V1_0: Self = Self { major: 1, minor: 0 };
    pub const V1_1: Self = Self { major: 1, minor: 1 };
    pub const V1_2: Self = Self { major: 1, minor: 2 };
    pub const V1_3: Self = Self { major: 1, minor: 3 };
    pub const V1_4: Self = Self { major: 1, minor: 4 };
    pub const V1_5: Self = Self { major: 1, minor: 5 };
    pub const V1_6: Self = Self { major: 1, minor: 6 };
    pub const V1_7: Self = Self { major: 1, minor: 7 };
    pub const V2_0: Self = Self { major: 2, minor: 0 };
}

/// A single key specification in an Arlington dictionary schema.
#[derive(Debug, Clone)]
pub struct ArlingtonKeySpec {
    pub key_name: &'static str,
    pub allowed_types: HashSet<ArlingtonType>,
    pub is_required: bool,
    pub since_version: PdfVersion,
    pub deprecated_in: Option<PdfVersion>,
    pub inherit_from_parent: bool,
}

/// Dictionary specification schema (e.g. "Catalog", "Page", "Annotation").
#[derive(Debug, Clone)]
pub struct ArlingtonDictionarySpec {
    pub dict_type: &'static str,
    pub keys: Vec<ArlingtonKeySpec>,
}
