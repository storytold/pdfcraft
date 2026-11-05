//! The crate's error type.

/// Why a PRC stream could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PrcError {
    /// The data does not start with the `PRC` magic [WD 6.1.1].
    #[error("not a PRC stream: missing the \"PRC\" magic")]
    NotPrc,
    /// A value ran past the end of its byte or bit stream.
    #[error("PRC data truncated while reading {0}")]
    Truncated(&'static str),
    /// A structural field holds a value the format forbids.
    #[error("malformed PRC: {0}")]
    Malformed(String),
    /// No `Double` code of up to 22 bits matched [WD 11.17].
    #[error("PRC Double: no code matches the next 22 bits")]
    UnknownDoubleCode,
    /// A section is not a valid zlib stream.
    #[error("PRC {section} section does not inflate: {reason}")]
    Inflate {
        /// The section's name.
        section: &'static str,
        /// The decoder's message.
        reason: String,
    },
    /// The inflated sections and copied blocks together exceed the ceiling
    /// ([`crate::MAX_INFLATED_BYTES`] or the caller's limit).
    #[error("PRC sections inflate past the {limit}-byte ceiling")]
    TooLarge {
        /// The ceiling that was hit.
        limit: usize,
    },
    /// The arrays read from one stream hold more elements than [`crate::MAX_DECODED_ELEMENTS`].
    /// One bit can decode to a 4- or 8-byte element, so the inflation ceiling alone does not
    /// bound the memory the arrays take.
    #[error("PRC arrays decode past the {limit}-element ceiling")]
    TooManyElements {
        /// The ceiling that was hit.
        limit: usize,
    },
    /// A construct this reader does not decode; the part of the file holding it cannot be
    /// read past.
    #[error("PRC {0} is not supported")]
    Unsupported(&'static str),
}
