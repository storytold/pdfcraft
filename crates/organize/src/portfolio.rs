//! PDF portfolios (Create a PDF ▸ Portfolio): a document whose own pages are a cover sheet and
//! whose documents are its document-level attachments, presented as a file list through the
//! catalog's `/Collection` dictionary (ISO 32000-2 §7.11.6, Tables 153–157).
//!
//! The attachments are added first with [`crate::add_attachment`]; [`set_portfolio`] then
//! describes them as a collection: the columns of the file list (`/Schema`), each file's values
//! for them (`/CI`), the order they are listed in (`/Sort`), how the list first shows (`/View`)
//! and the file presented first (`/D`). Built for case files and exhibit packets, the columns are
//! an exhibit label, the file name, a description, the document's own date, and the file's
//! modification date and size.

use pdfcraft_cos::{Dict, Document, Object, PdfString};

use crate::OrganizeError;
use crate::import::name_tree_entries;

/// How a portfolio's file list first shows (`/View`, Table 153).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PortfolioLayout {
    /// A table with a column per field (`/D`).
    #[default]
    Details,
    /// Large icons (`/T`).
    Tile,
    /// The list starts hidden (`/H`); the initial file fills the window.
    Hidden,
}

impl PortfolioLayout {
    /// The `/View` name.
    fn name(self) -> &'static str {
        match self {
            Self::Details => "D",
            Self::Tile => "T",
            Self::Hidden => "H",
        }
    }

    /// The layout for `details`, `tile` or `hidden`; `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "details" | "" => Some(Self::Details),
            "tile" | "tiles" => Some(Self::Tile),
            "hidden" => Some(Self::Hidden),
            _ => None,
        }
    }
}

/// How the files are labelled as exhibits, in list order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExhibitStyle {
    /// No exhibit column.
    None,
    /// A, B, … Z, AA, AB, … (as in "Exhibit A").
    #[default]
    Letters,
    /// 1, 2, 3, …
    Numbers,
}

impl ExhibitStyle {
    /// The style for `letters`, `numbers` or `none`; `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "letters" | "letter" | "alpha" | "" => Some(Self::Letters),
            "numbers" | "number" | "numeric" => Some(Self::Numbers),
            "none" | "off" => Some(Self::None),
            _ => None,
        }
    }
}

/// The exhibit label of the file at `index` (0-based) in `style`, after `prefix` ("Exhibit" gives
/// "Exhibit A"; an empty prefix gives "A"). `None` for [`ExhibitStyle::None`].
pub fn exhibit_label(style: ExhibitStyle, prefix: &str, index: usize) -> Option<String> {
    let mark = match style {
        ExhibitStyle::None => return None,
        ExhibitStyle::Numbers => index.saturating_add(1).to_string(),
        ExhibitStyle::Letters => {
            // Bijective base 26: 0 → A, 25 → Z, 26 → AA.
            let mut n = index;
            let mut letters = Vec::new();
            loop {
                letters.push(b'A'.saturating_add((n % 26) as u8));
                if n < 26 {
                    break;
                }
                n = n / 26 - 1;
            }
            letters.reverse();
            String::from_utf8_lossy(&letters).into_owned()
        }
    };
    let prefix = prefix.trim();
    Some(if prefix.is_empty() { mark } else { format!("{prefix} {mark}") })
}

/// One file of a portfolio: the `/EmbeddedFiles` key [`crate::add_attachment`] returned for it,
/// and its values for the portfolio's own columns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortfolioItem {
    /// The attachment's name-tree key.
    pub key: String,
    /// "Exhibit A", "Tab 3"…; `None` leaves the cell empty.
    pub exhibit: Option<String>,
    /// The document's own date (when it was signed, filed or served), as a PDF date string
    /// (`D:20261116`); `None` leaves the cell empty.
    pub document_date: Option<String>,
}

/// A portfolio's file list: its files in the order they are listed, how the list first shows,
/// and the file presented first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortfolioSpec {
    pub items: Vec<PortfolioItem>,
    pub layout: PortfolioLayout,
    /// The key of the file presented first; `None` presents the cover sheet.
    pub initial: Option<String>,
}

/// The most files one portfolio lists: far beyond any exhibit packet, and a bound on the work.
pub const MAX_PORTFOLIO_FILES: usize = 10_000;

/// Make `doc` a portfolio of the attachments `spec` lists: write the catalog's `/Collection`,
/// give each listed attachment its `/CI` values, and open the document on its attachments
/// (`/PageMode /UseAttachments`), so a viewer without portfolio support still shows the files.
/// Every listed key must name a document-level attachment, each once; attachments `spec` does
/// not list stay attachments and are listed after the others.
pub fn set_portfolio(doc: &mut Document, spec: &PortfolioSpec) -> Result<(), OrganizeError> {
    if spec.items.is_empty() {
        return Err(OrganizeError::Invalid("a portfolio needs at least one file".into()));
    }
    if spec.items.len() > MAX_PORTFOLIO_FILES {
        return Err(OrganizeError::Invalid(format!("a portfolio holds at most {MAX_PORTFOLIO_FILES} files")));
    }
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    let specs = file_specs(doc);
    // Check everything before changing anything, so a refusal leaves the document as it was.
    let mut targets = Vec::with_capacity(spec.items.len());
    for (i, item) in spec.items.iter().enumerate() {
        if spec.items[..i].iter().any(|other| other.key == item.key) {
            return Err(OrganizeError::Invalid(format!("{} is listed twice", item.key)));
        }
        let Some((_, value)) = specs.iter().find(|(k, _)| *k == item.key) else {
            return Err(OrganizeError::Invalid(format!("there is no attachment named {}", item.key)));
        };
        let Object::Ref(reference) = value else {
            return Err(OrganizeError::Invalid(format!("attachment {} has an invalid file specification", item.key)));
        };
        targets.push(*reference);
    }
    if let Some(initial) = &spec.initial
        && !specs.iter().any(|(k, _)| k == initial)
    {
        return Err(OrganizeError::Invalid(format!("there is no attachment named {initial} to open first")));
    }

    let exhibits = spec.items.iter().any(|i| i.exhibit.as_deref().is_some_and(|e| !e.trim().is_empty()));
    let dates = spec.items.iter().any(|i| i.document_date.as_deref().is_some_and(|d| !d.trim().is_empty()));

    for (n, (item, reference)) in spec.items.iter().zip(&targets).enumerate() {
        let mut ci = Dict::new();
        ci.set(b"Type".to_vec(), Object::name("CollectionItem"));
        ci.set(b"Order".to_vec(), Object::Int(i64::try_from(n).unwrap_or(i64::MAX).saturating_add(1)));
        if let Some(exhibit) = item.exhibit.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            ci.set(b"Exhibit".to_vec(), Object::String(PdfString::text(exhibit)));
        }
        if let Some(date) = item.document_date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            ci.set(b"DocDate".to_vec(), Object::String(PdfString::literal(date.as_bytes().to_vec())));
        }
        let ci = doc.add(Object::Dict(ci));
        doc.update_dict(*reference, |fs| fs.set(b"CI".to_vec(), Object::Ref(ci)))?;
    }

    let mut collection = Dict::new();
    collection.set(b"Type".to_vec(), Object::name("Collection"));
    collection.set(b"Schema".to_vec(), Object::Dict(schema(exhibits, dates)));
    let mut sort = Dict::new();
    sort.set(b"Type".to_vec(), Object::name("CollectionSort"));
    sort.set(b"S".to_vec(), Object::name("Order"));
    sort.set(b"A".to_vec(), Object::Bool(true));
    collection.set(b"Sort".to_vec(), Object::Dict(sort));
    collection.set(b"View".to_vec(), Object::name(spec.layout.name()));
    if let Some(initial) = &spec.initial {
        collection.set(b"D".to_vec(), Object::String(PdfString::text(initial)));
    }
    let collection = doc.add(Object::Dict(collection));
    doc.update_dict(root, |catalog| {
        catalog.set(b"Collection".to_vec(), Object::Ref(collection));
        catalog.set(b"PageMode".to_vec(), Object::name("UseAttachments"));
    })?;
    Ok(())
}

/// The file list's columns (Table 155): the hidden `Order` that sorts it, then Exhibit (when any
/// file has one), Name, Description, Document date (when any file has one), Modified and Size.
fn schema(exhibits: bool, dates: bool) -> Dict {
    let mut schema = Dict::new();
    schema.set(b"Type".to_vec(), Object::name("CollectionSchema"));
    let mut column = 0i64;
    let mut add = |key: &str, subtype: &str, label: &str, visible: bool, editable: bool| {
        let mut field = Dict::new();
        field.set(b"Type".to_vec(), Object::name("CollectionField"));
        field.set(b"Subtype".to_vec(), Object::name(subtype));
        field.set(b"N".to_vec(), Object::String(PdfString::text(label)));
        field.set(b"O".to_vec(), Object::Int(column));
        field.set(b"V".to_vec(), Object::Bool(visible));
        field.set(b"E".to_vec(), Object::Bool(editable));
        schema.set(key.as_bytes().to_vec(), Object::Dict(field));
        column = column.saturating_add(1);
    };
    add("Order", "N", "Order", false, false);
    if exhibits {
        add("Exhibit", "S", "Exhibit", true, true);
    }
    add("FileName", "F", "Name", true, false);
    add("Description", "Desc", "Description", true, true);
    if dates {
        add("DocDate", "D", "Document date", true, true);
    }
    add("Modified", "ModDate", "Modified", true, false);
    add("Size", "Size", "Size", true, false);
    schema
}

/// The `/EmbeddedFiles` entries as (decoded key, file specification), in tree order.
fn file_specs(doc: &Document) -> Vec<(String, Object)> {
    let Some(tree) = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"Names").map(|n| doc.resolve(n)))
        .and_then(|n| n.as_dict().and_then(|d| d.get(b"EmbeddedFiles").cloned()))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    name_tree_entries(doc, &tree, 0, &mut std::collections::HashSet::new(), &mut out);
    out.into_iter().map(|(k, v)| (PdfString::literal(k).to_text(), v)).collect()
}
