//! Bounded reading of numbers and numeric arrays from the object graph.
//!
//! A [`NumberReader`] reads one number, an exact-length tuple, or a capped sequence whose every
//! original slot keeps its own result. It knows nothing of rectangles, quads or colours (that is
//! `pdfcraft-model`), and it never repairs anything: a value that is not a finite number is an
//! error at its own position, never a zero, a skipped entry or a shorter array.
//!
//! References are followed with [`Document::resolve`]'s bounded chain (an initial reference plus
//! up to 32 more); a chain still ending in a reference (a cycle, or one too long) is
//! [`NumberError::UnresolvedReference`]. A missing or broken target reads as `null`, as
//! [`Document::get`] reads it, so it is [`NumberError::NotNumber`]: the reader can't tell those
//! apart more precisely than the object layer does.
//!
//! Bounds: the outer array's length is checked against the cap before any entry is resolved or
//! anything is allocated, and a direct array is read in place, never copied. This bounds what the
//! reader adds, not what the parser allocates to load an indirect array.

use crate::document::Held;
use crate::{Document, Object};

/// The most entries any numeric tuple or sequence may have (a conservative product allowance,
/// not a limit ISO 32000-2 sets).
pub const MAX_NUMERIC_ENTRIES: usize = 65_536;

/// Why one value is not a usable number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NumberError {
    /// Not an integer or a real (including `null`, and a reference to a missing object).
    #[error("not a number")]
    NotNumber,
    /// A real that is NaN or infinite.
    #[error("not a finite number")]
    NonFinite,
    /// A reference that could not be followed to an object: a cycle, a chain past the bound, or
    /// any reference read by [`NumberReader::direct`].
    #[error("a reference that could not be followed")]
    UnresolvedReference,
}

/// Why a numeric array could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArrayError {
    #[error("not an array")]
    NotArray,
    #[error("a reference to the array could not be followed")]
    UnresolvedReference,
    #[error("{actual} entries where {expected} are required")]
    WrongLength { expected: usize, actual: usize },
    #[error("{len} entries, more than the {cap} allowed")]
    TooLong { len: usize, cap: usize },
    #[error("entry {index}: {error}")]
    InvalidEntry { index: usize, error: NumberError },
    #[error("not enough memory to read the array")]
    Allocation,
}

/// A numeric array read slot by slot: `entries[i]` is the original entry `i`, whatever the
/// entries before it were.
#[derive(Clone, Debug, PartialEq)]
pub struct NumberSequence {
    pub entries: Vec<Result<f64, NumberError>>,
}

/// Reads numbers through a document's references ([`NumberReader::new`]), or only direct
/// values ([`NumberReader::direct`]) for inputs that have no document. Both use the same rules.
#[derive(Clone, Copy, Debug)]
pub struct NumberReader<'doc> {
    doc: Option<&'doc Document>,
}

impl<'doc> NumberReader<'doc> {
    /// A reader that follows references in `doc`.
    pub fn new(doc: &'doc Document) -> Self {
        Self { doc: Some(doc) }
    }

    /// A reader for direct values only: any reference is [`NumberError::UnresolvedReference`]
    /// (or [`ArrayError::UnresolvedReference`] for the array itself). Not a way around
    /// resolution for a value read from a document.
    pub fn direct() -> Self {
        Self { doc: None }
    }

    /// `o` itself, or the object its reference chain ends at; `None` when that is still a
    /// reference. Direct objects are borrowed, never copied.
    fn hold<'o>(&self, o: &'o Object) -> Option<Held<'o>> {
        match o {
            Object::Ref(_) => {
                let doc = self.doc?;
                #[cfg(test)]
                tests::count_resolution();
                let held = doc.held(o);
                (!matches!(&*held, Object::Ref(_))).then_some(held)
            }
            o => Some(Held::Direct(o)),
        }
    }

    /// One finite number: an integer or a real, after following references. Integers convert as
    /// [`Object::as_f64`] converts them; nothing is parsed, clamped or made up.
    pub fn finite(&self, object: &Object) -> Result<f64, NumberError> {
        let held = self.hold(object).ok_or(NumberError::UnresolvedReference)?;
        let value = held.as_f64().ok_or(NumberError::NotNumber)?;
        if value.is_finite() { Ok(value) } else { Err(NumberError::NonFinite) }
    }

    /// Exactly `N` finite numbers. The array's length is checked before any entry is read, and
    /// any bad entry rejects the whole tuple: no entry is skipped and no prefix is taken.
    pub fn tuple<const N: usize>(&self, object: &Object) -> Result<[f64; N], ArrayError> {
        if N > MAX_NUMERIC_ENTRIES {
            return Err(ArrayError::TooLong { len: N, cap: MAX_NUMERIC_ENTRIES });
        }
        let held = self.hold(object).ok_or(ArrayError::UnresolvedReference)?;
        let array = held.as_array().ok_or(ArrayError::NotArray)?;
        if array.len() != N {
            return Err(ArrayError::WrongLength { expected: N, actual: array.len() });
        }
        let mut out = [0.0; N];
        for (index, (slot, entry)) in out.iter_mut().zip(array.iter()).enumerate() {
            *slot = self.finite(entry).map_err(|error| ArrayError::InvalidEntry { index, error })?;
        }
        Ok(out)
    }

    /// Every entry of an array of at most `cap` entries (and never more than
    /// [`MAX_NUMERIC_ENTRIES`]), each slot with its own result. An array past the cap is an error
    /// with no usable prefix; an empty array is an empty sequence.
    pub fn sequence(&self, object: &Object, cap: usize) -> Result<NumberSequence, ArrayError> {
        let cap = cap.min(MAX_NUMERIC_ENTRIES);
        let held = self.hold(object).ok_or(ArrayError::UnresolvedReference)?;
        let array = held.as_array().ok_or(ArrayError::NotArray)?;
        if array.len() > cap {
            return Err(ArrayError::TooLong { len: array.len(), cap });
        }
        let mut entries = Vec::new();
        reserve(&mut entries, array.len())?;
        entries.extend(array.iter().map(|entry| self.finite(entry)));
        Ok(NumberSequence { entries })
    }
}

/// Room for `len` more entries, or [`ArrayError::Allocation`] rather than an allocation panic.
fn reserve<T>(v: &mut Vec<T>, len: usize) -> Result<(), ArrayError> {
    #[cfg(test)]
    if tests::reservation_fails() {
        return Err(ArrayError::Allocation);
    }
    v.try_reserve_exact(len).map_err(|_| ArrayError::Allocation)
}

#[cfg(test)]
mod tests;
