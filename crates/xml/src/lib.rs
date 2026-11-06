//! pdfcraft-xml — L0 bounded XML parsing for every PdfCraft feature that reads XML: XFA forms,
//! XFDF and XML form data, and XMP metadata. All of it comes from files PdfCraft is handed, so a
//! hostile document must not be able to make a parse take minutes.
//!
//! [`parse`] is the only way into [`roxmltree`] in the workspace: `clippy.toml` disallows calling
//! `roxmltree::Document::parse` and `parse_with_options` anywhere else. Callers still depend on
//! roxmltree (the workspace's one pinned version) for its node types. Before the parser sees the
//! text, a single linear scan ([`scan`]) checks three things:
//! - **Depth**, at most [`Limits::max_depth`] nested elements. roxmltree 0.21.2 no longer recurses
//!   per level, so this bound protects the callers' own walks of the tree, which do recurse.
//! - **DTDs** are refused outright, as every caller wants (`allow_dtd: false`). Refusing on sight means
//!   the scan never has to find where a DOCTYPE's internal subset ends.
//! - **Work**, at most [`Limits::max_work`]. roxmltree 0.21.2 has loops whose cost grows with the
//!   square of the input: duplicate attribute and namespace checks within a start tag, and copying the
//!   inherited namespace bindings into every element that declares one. The scan adds up what those
//!   loops will cost (see [`scan`] for the model) and refuses past the budget. Measured on 0.21.2,
//!   without the budget: 60,000 attributes on one element took 9.8 s, and a root declaring 2,000
//!   namespaces with 20,000 children declaring one each took 156 s, from a 436 KB input.
//!
//! The node count is roxmltree's own `nodes_limit` ([`Limits::max_nodes`]), chosen by each caller.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod scan;
pub mod span;

/// The default bound on element nesting: far beyond any real form or packet, and well within what
/// a recursive walk of the tree can afford on a worker thread's stack.
pub const MAX_DEPTH: usize = 1_024;

/// The default bound on the parser's superlinear work, in the units [`scan`] counts (a short string
/// comparison each). A unit measured at most about 6 ns, so this is at most about 0.4 s of
/// roxmltree's time on the machine where it was measured. Real documents use far less: an XFA
/// template of fifty thousand fields, five attributes each, costs about 1.3·10⁶; a PDF/A XMP packet
/// with extension schemas about 2·10⁴.
pub const MAX_WORK: u64 = 1 << 26;

/// What a parse may cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The deepest element nesting accepted.
    pub max_depth: usize,
    /// The most nodes (elements, text, comments, processing instructions) the tree may hold.
    pub max_nodes: u32,
    /// The most superlinear parser work accepted (see [`scan`]).
    pub max_work: u64,
}

impl Limits {
    /// The default depth and work bounds, with `max_nodes` nodes at most.
    pub const fn new(max_nodes: u32) -> Self {
        Limits { max_depth: MAX_DEPTH, max_nodes, max_work: MAX_WORK }
    }

    /// These limits with a different depth bound.
    pub const fn with_max_depth(self, max_depth: usize) -> Self {
        Limits { max_depth, ..self }
    }
}

/// Why a text was not parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XmlError {
    /// The text declares a DTD (`<!DOCTYPE`), which PdfCraft never reads.
    Doctype,
    /// Elements nest deeper than the limit.
    TooDeep { limit: usize },
    /// Parsing would take more work than the limit (too many attributes or namespace declarations).
    TooComplex { limit: u64 },
    /// The text is not well-formed XML, or holds more nodes than the limit.
    Malformed(roxmltree::Error),
}

impl XmlError {
    /// Whether the text was refused for its size or shape rather than for not being XML.
    pub fn is_limit(&self) -> bool {
        matches!(
            self,
            XmlError::TooDeep { .. }
                | XmlError::TooComplex { .. }
                | XmlError::Malformed(
                    roxmltree::Error::NodesLimitReached | roxmltree::Error::AttributesLimitReached | roxmltree::Error::NamespacesLimitReached
                )
        )
    }
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XmlError::Doctype => f.write_str("XML with a DTD is not read"),
            XmlError::TooDeep { limit } => write!(f, "XML elements nest more than {limit} deep"),
            XmlError::TooComplex { limit } => {
                write!(f, "the XML's attributes and namespace declarations would take the parser more than {limit} units of work")
            }
            XmlError::Malformed(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for XmlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            XmlError::Malformed(e) => Some(e),
            _ => None,
        }
    }
}

/// `text` as an XML tree, once [`scan::check`] has found it within `limits`.
pub fn parse(text: &str, limits: Limits) -> Result<roxmltree::Document<'_>, XmlError> {
    scan::check(text, limits)?;
    let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: limits.max_nodes, ..Default::default() };
    // The workspace's one call into the parser, behind the scan above.
    #[allow(clippy::disallowed_methods)]
    let parsed = roxmltree::Document::parse_with_options(text, options);
    parsed.map_err(XmlError::Malformed)
}

#[cfg(test)]
mod tests;
