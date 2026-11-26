//! OPC package access through `craft-opc`: zip parts read against GridCraft's limits,
//! relationships with GridCraft's `Rel`, part-name resolution, and a deterministic zip writer.
//! Parts are parsed with this crate's XML reader.

use crate::IoError;
use crate::xml::{self, El};

pub use craft_opc::{normalize, resolve, split_dir};

/// Largest decompressed part accepted.
pub const MAX_PART: u64 = 512 * 1024 * 1024;
/// Largest total decompressed size read from one package.
pub const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
/// Most entries accepted in the zip directory.
pub const MAX_ENTRIES: usize = 200_000;

const LIMITS: craft_opc::Limits = craft_opc::Limits::new(MAX_ENTRIES, MAX_PART, MAX_TOTAL);

pub struct Package<'a> {
    opc: craft_opc::LazyPackage<'a>,
    craft: craft_xlsx::Package<'a>,
}

/// A relationship.
#[derive(Clone, Debug)]
pub struct Rel {
    pub id: String,
    /// Last path segment of the relationship type (`worksheet`, `styles`, …): the same for the
    /// transitional and strict namespaces.
    pub kind: String,
    /// Resolved part name (no leading `/`), or the raw target for external relationships.
    pub target: String,
    pub external: bool,
}

/// GridCraft's wording of a package error.
fn error(e: craft_opc::Error) -> IoError {
    use craft_opc::Error as E;
    match e {
        E::Zip(m) | E::Write(m) => IoError::Zip(m),
        E::TooManyEntries { declared, .. } => IoError::TooLarge(format!("{declared} zip entries")),
        E::PartTooLarge { part, limit } => IoError::TooLarge(format!("{part} expands to more than {limit} bytes")),
        E::PackageTooLarge { .. } => IoError::TooLarge("package expands to more than 1 GB".into()),
        E::Xml { part, error } => IoError::Xml(format!("{part}: {error}")),
        e if e.is_limit() => IoError::TooLarge(e.to_string()),
        e => IoError::Zip(e.to_string()),
    }
}

/// GridCraft's wording of a `craft-xlsx` error.
pub(crate) fn craft_error(e: craft_xlsx::Error) -> IoError {
    match e {
        craft_xlsx::Error::Zip(m) => IoError::Zip(m),
        craft_xlsx::Error::Xml(m) => IoError::Xml(m),
        craft_xlsx::Error::Invalid(m) => IoError::Format(m),
        craft_xlsx::Error::TooLarge(m) => IoError::TooLarge(m),
    }
}

impl<'a> Package<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Package<'a>, IoError> {
        let opc = craft_opc::LazyPackage::open(bytes, &LIMITS).map_err(error)?;
        let craft = craft_xlsx::Package::open(bytes, &craft_xlsx::Limits::default()).map_err(craft_error)?;
        Ok(Package { opc, craft })
    }

    pub(crate) fn craft_mut(&mut self) -> &mut craft_xlsx::Package<'a> {
        &mut self.craft
    }

    /// Every part's (normalized) name, sorted.
    #[cfg(test)]
    pub fn names(&self) -> Vec<String> {
        self.opc.names().into_iter().map(str::to_string).collect()
    }

    pub fn has(&self, part: &str) -> bool {
        self.opc.has(part)
    }

    /// Reads a part's bytes; `Ok(None)` when it doesn't exist.
    pub fn read(&mut self, part: &str) -> Result<Option<Vec<u8>>, IoError> {
        self.opc.read(part).map_err(error)
    }

    /// Reads a part the reader can do without (the caller warns and goes on when it fails). A read
    /// that fails partway is charged this part's whole allowance instead of ending the package's.
    pub fn read_optional_limited(&mut self, part: &str, maximum: u64) -> Result<Option<Vec<u8>>, IoError> {
        self.opc.read_optional_limited(part, maximum).map_err(error)
    }

    /// Reads a small metadata part without allowing it to expand to the general part limit.
    pub fn read_limited(&mut self, part: &str, maximum: u64) -> Result<Option<Vec<u8>>, IoError> {
        self.opc.read_limited(part, maximum).map_err(error)
    }

    pub fn read_xml(&mut self, part: &str) -> Result<Option<El>, IoError> {
        match self.read(part)? {
            Some(b) => xml::parse(&b).map(Some).map_err(|e| prefix(part, e)),
            None => Ok(None),
        }
    }

    /// Relationships of a part (`xl/workbook.xml` → `xl/_rels/workbook.xml.rels`). Missing or
    /// broken rels parts give an empty list.
    pub fn rels(&mut self, part: &str) -> Result<Vec<Rel>, IoError> {
        let rels = self.opc.rels(part).map_err(error)?;
        Ok(rels
            .list
            .into_iter()
            .map(|r| {
                let kind = r.kind().to_string();
                Rel { id: r.id, kind, target: r.target, external: r.external }
            })
            .collect())
    }
}

/// The first part whose declared unpacked size is over [`MAX_PART`], with that size.
pub fn oversized_part(bytes: &[u8]) -> Option<(String, u64)> {
    craft_opc::declared_oversized(bytes, &LIMITS)
}

fn prefix(part: &str, e: IoError) -> IoError {
    match e {
        IoError::Xml(m) => IoError::Xml(format!("{part}: {m}")),
        o => o,
    }
}

// ---------------------------------------------------------------- writing

/// A zip package being written: every entry deflated, with one fixed timestamp.
pub struct ZipOut {
    zip: craft_opc::ZipOut,
}

impl ZipOut {
    pub fn new() -> ZipOut {
        ZipOut { zip: craft_opc::ZipOut::new() }
    }
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), IoError> {
        self.zip.add(name, data, craft_opc::Compression::Deflate).map_err(error)
    }
    pub fn finish(self) -> Result<Vec<u8>, IoError> {
        self.zip.finish().map_err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolving() {
        assert_eq!(resolve("xl", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("xl/worksheets", "../drawings/d1.xml"), "xl/drawings/d1.xml");
        assert_eq!(resolve("xl/worksheets", "/xl/tables/t.xml"), "xl/tables/t.xml");
        assert_eq!(resolve("", "xl/workbook.xml"), "xl/workbook.xml");
        assert_eq!(resolve("a", "../../../x"), "x");
        assert_eq!(split_dir("xl/workbook.xml"), ("xl", "workbook.xml"));
        assert_eq!(split_dir("x.xml"), ("", "x.xml"));
    }
}
