//! pdfcraft-xfa — XFA forms (L3). See the README.
//!
//! A dynamic XFA form is a PDF shell around an XML template; viewers without an XFA engine
//! show its one placeholder page ("requires Adobe Reader"). This crate reads the template,
//! lays it out and writes ordinary pages and AcroForm fields into the document, so the rest
//! of PdfCraft (rendering, filling, saving, printing, tools) works on it unchanged.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod data;
pub mod fixtures;
pub mod image;
pub mod layout;
pub mod model;
pub mod packets;
pub mod parse;
pub mod pdf;
pub mod script;
pub mod text;

use pdfcraft_cos::Document;

/// The deepest element nesting accepted before an XML document reaches the parser.
///
/// roxmltree parses by recursive descent, so its stack use grows with the nesting depth: ten
/// thousand levels overflow the main thread's stack, and a stack overflow is `abort()`, which no
/// `catch_unwind` can intercept. The bound therefore has to be checked before parsing, not while
/// walking the result. Around three thousand levels still parse on the default stack, so this stays
/// clear of the cliff while leaving far more room than the walks below, which cut off at 64 - and
/// more than the five hundred levels `hostile_templates_fail_without_panicking` keeps parsing.
/// `pdfcraft-xfdf` carries the same check for form data files.
pub(crate) const MAX_XML_DEPTH: usize = 1_024;

/// Element nesting depth of `text`, skipping comments, CDATA, processing instructions, the DOCTYPE
/// and quoted attribute values. A tag that closes itself (`<draw/>`) is one level while it is read
/// and does not stay open, so a long run of styling elements cannot accumulate into the bound. It
/// cannot under-count a real element; a stray `<` in text is invalid XML and only makes it
/// over-count, which errs towards refusing the document.
pub(crate) fn xml_nesting_depth(text: &str) -> usize {
    fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        (from..hay.len()).find(|&p| hay[p..].starts_with(needle))
    }
    let b = text.as_bytes();
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
        } else if b[i..].starts_with(b"<!--") {
            i = find(b, i + 4, b"-->").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<![CDATA[") {
            i = find(b, i + 9, b"]]>").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<?") {
            i = find(b, i + 2, b"?>").map_or(b.len(), |p| p + 2);
        } else if b[i..].starts_with(b"<!") {
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else if b[i..].starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else {
            depth += 1;
            max = max.max(depth);
            // Skip the rest of the start tag, over any quoted attribute value, and remember its last
            // significant byte: a tag that ends in `/` closes itself, so it is one level deep while it
            // is read rather than a level left open.
            i += 1;
            let mut quote = 0u8;
            let mut last = 0u8;
            while i < b.len() {
                let c = b[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    i += 1;
                    break;
                } else if !c.is_ascii_whitespace() {
                    last = c;
                }
                i += 1;
            }
            if last == b'/' {
                depth -= 1;
            }
        }
    }
    max
}

pub use data::{DataImport, ImportNode, export_data, import_data, parse_data_file, write_data_image};
pub use data::{
    DataNode, DataOp, DataPath, DatasetsWrite, FieldData, FieldDatum, add_data_instances, build_data, data_path, iso_to_pattern, parse_datasets,
    pattern_to_iso, read_values, read_values_with, remove_data_instance, som_to_path, write_data_ops, write_data_value, write_datasets,
    write_datasets_reusing, write_datasets_with,
};
pub use layout::{Action, BorderShape, Form, Item, MAX_PAGES, Page, Widget, WidgetKind, layout};
pub use packets::{Encoding, Packets, decode as decode_packet, encode as encode_packet, read_packets};
pub use parse::{measure, parse};
pub use pdf::{CLICK_KEY, LAYOUT_KEY, SOM_KEY, Stripped, XfaShape, existing_layout, strip, xfa_shape};
pub use script::{
    FormNode, LiveForm, MAX_FORM_NODES, NodeKind, OVERRIDES_KEY, Overrides, ScriptEvent, apply_overrides, data_of, field_grows_with_text,
    fields_by_som, form_tree, has_scripts, overrides, refresh_value, rerender, set_overrides, template_of,
};

#[derive(Debug, thiserror::Error)]
pub enum XfaError {
    #[error("the document is not an XFA form")]
    NotXfa,
    #[error("the XFA template could not be read: {0}")]
    Malformed(String),
    #[error("{0}")]
    TooLarge(String),
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// What laying out a form produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub pages: usize,
    pub fields: usize,
    /// Template bytes read.
    pub template_bytes: usize,
    /// What was approximated or left out, for the user.
    pub warnings: Vec<String>,
}

/// Is this a dynamic XFA form still showing its placeholder pages (one PdfCraft has not laid
/// out yet)?
pub fn is_dynamic(doc: &Document) -> bool {
    existing_layout(doc).is_none() && matches!(read_packets(doc), Ok(Some(p)) if p.needs_rendering || !p.has_fields)
}

/// Lay a template packet (or whole XDP, whose datasets then fill the fields) out.
pub fn layout_xml(xml: &str) -> Result<Form, XfaError> {
    let (tpl, warnings) = parse(xml)?;
    let data = parse_datasets(xml);
    let mut form = layout(&tpl, data.as_ref())?;
    form.warnings.splice(0..0, warnings);
    Ok(form)
}

/// Lay a dynamic XFA form out into its own document: its placeholder pages are replaced by the
/// laid-out pages and its fields join the AcroForm, holding the values of the datasets packet.
/// The XFA packets stay, so Adobe's viewers keep rendering it their way. Widget appearances are
/// left for the forms layer to generate.
pub fn render_into(doc: &mut Document) -> Result<Report, XfaError> {
    if !is_dynamic(doc) {
        return Err(XfaError::NotXfa);
    }
    let packets = read_packets(doc)?.ok_or(XfaError::NotXfa)?;
    if packets.xdp.trim().is_empty() {
        return Err(XfaError::Malformed("the XFA packets hold no template".into()));
    }
    let (tpl, parse_warnings) = parse(&packets.xdp)?;
    let data = parse_datasets(&packets.xdp);
    let laid = apply_overrides(&tpl, &overrides(doc));
    let mut form = layout(&laid, data.as_ref())?;
    form.warnings.splice(0..0, parse_warnings);
    if form.pages.is_empty() {
        return Err(XfaError::Malformed("the template laid out to no pages".into()));
    }
    let written = pdf::write_form(doc, &form)?;
    let mut warnings = form.warnings;
    warnings.extend(written.warnings);
    warnings.dedup();
    Ok(Report { pages: written.pages, fields: written.fields, template_bytes: packets.xdp.len(), warnings })
}

#[cfg(test)]
mod tests;
