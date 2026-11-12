//! What every section reader shares: the cursor, the schema, the authoring version, and the
//! base fields every PRC entity starts with [WD 7.2.3, 7.4.3, 8.6].

use crate::PrcError;
use crate::bits::BitReader;
use crate::schema::Schema;
use crate::tree::Graphics;

const ATTRIBUTE: u32 = 201;

/// A [`PrcError::Malformed`] naming `what`.
pub(crate) fn malformed(what: String) -> PrcError {
    PrcError::Malformed(what)
}

/// A section reader.
pub(crate) struct Ctx<'a, 's> {
    pub(crate) r: BitReader<'a>,
    pub(crate) schema: &'s Schema,
    /// The file structure's authoring version; gates version-added fields.
    pub(crate) version: u32,
    /// The current name: what a `same_name` entity reuses; every entity's own name replaces
    /// it [WD 7.2.3.4]. Each section starts with none.
    pub(crate) name: Option<String>,
    /// The PRC unique id of the last `ContentPRCRefBase` read [WD 7.2.3.2].
    pub(crate) uid: u32,
    /// The current graphics: what a `same_graphics` entity reuses [WD 7.2.4].
    pub(crate) graphics: Graphics,
}

impl<'a, 's> Ctx<'a, 's> {
    pub(crate) fn new(r: BitReader<'a>, schema: &'s Schema, version: u32) -> Self {
        Ctx { r, schema, version, name: None, uid: 0, graphics: Graphics::default() }
    }

    /// Reads an entity type code; any value but `want` is malformed.
    pub(crate) fn expect_type(&mut self, want: u32) -> Result<(), PrcError> {
        let t = self.r.unsigned_integer()?;
        if t != want {
            return Err(malformed(format!("entity type {t} where {want} belongs")));
        }
        Ok(())
    }

    /// A count that must fit in the remaining data at `min_bits` per item, which bounds any
    /// allocation sized by it.
    pub(crate) fn count(&mut self, min_bits: usize, what: &'static str) -> Result<usize, PrcError> {
        let n = self.r.unsigned_integer()? as usize;
        if n.saturating_mul(min_bits) > self.r.remaining() {
            return Err(PrcError::Truncated(what));
        }
        self.r.take_elements(n)?;
        Ok(n)
    }

    /// A counted array of `UnsignedInteger`s.
    pub(crate) fn uints(&mut self, what: &'static str) -> Result<Vec<u32>, PrcError> {
        let n = self.count(1, what)?;
        (0..n).map(|_| self.r.unsigned_integer()).collect()
    }

    /// A counted array of `Double`s.
    pub(crate) fn doubles(&mut self, what: &'static str) -> Result<Vec<f64>, PrcError> {
        let n = self.count(2, what)?;
        (0..n).map(|_| self.r.double()).collect()
    }

    /// `AttributeEntry` [WD 7.4.3]: a predefined title code or a string.
    fn attribute_entry(&mut self) -> Result<(), PrcError> {
        if self.r.bit()? {
            self.r.unsigned_integer()?;
        } else {
            self.r.string()?;
        }
        Ok(())
    }

    /// One attribute value [WD 7.4.3], read past.
    fn attribute_value(&mut self) -> Result<(), PrcError> {
        match self.r.unsigned_integer()? {
            0 => {}
            1 | 3 => {
                self.r.integer()?;
            }
            2 => {
                self.r.double()?;
            }
            4 => {
                self.r.string()?;
            }
            // prc-rs: a 64-bit integer is its high part, then its low part.
            5 => {
                self.r.integer()?;
                self.r.unsigned_integer()?;
            }
            k => return Err(malformed(format!("attribute value kind {k}"))),
        }
        Ok(())
    }

    /// `ContentPRCBase` [WD 7.2.3]: attributes, then the name; returns the entity's name,
    /// which is also the current name afterwards.
    pub(crate) fn content_prc_base(&mut self) -> Result<Option<String>, PrcError> {
        let n = self.count(1, "attributes")?;
        for _ in 0..n {
            self.expect_type(ATTRIBUTE)?;
            self.attribute_entry()?;
            let pairs = self.count(2, "attribute pairs")?;
            for _ in 0..pairs {
                self.attribute_entry()?;
                self.attribute_value()?;
            }
            self.schema.skip_added_fields(ATTRIBUTE, &mut self.r)?;
        }
        // `same_name` TRUE reuses the current name [WD 7.2.3.4].
        if !self.r.bit()? {
            self.name = self.r.string()?;
        }
        Ok(self.name.clone())
    }

    /// `UserData` [WD 8.6]: a bit count, then that many opaque bits.
    pub(crate) fn user_data(&mut self) -> Result<(), PrcError> {
        let n = self.r.unsigned_integer()? as usize;
        self.r.skip_bits(n)
    }
}
