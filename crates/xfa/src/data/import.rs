use super::build::{escape, fresh_datasets, xml_name};
use super::merge::{Op, WRAP_CLOSE, WRAP_OPEN, qname, start_tag_end};
use super::values::Want;
use super::write::{DatasetsWrite, rewrite};
use super::{DataPath, Document, MAX_DEPTH, MAX_NODES, ObjRef, Object, XFA_DATA_NS, XfaError};

/// Data read from a file, held as a tree so it can be written into any packet with the
/// prefix that packet uses, never as the file's own markup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportNode {
    pub name: String,
    /// The element's namespace (`None`: none).
    pub ns: Option<String>,
    /// `(namespace, name, value)`.
    pub attrs: Vec<(Option<String>, String, String)>,
    pub text: String,
    pub children: Vec<ImportNode>,
}

/// Deepest data an import keeps.
const MAX_IMPORT_DEPTH: usize = MAX_DEPTH;

/// Most bytes of names, namespaces, attributes and text an import copies out of its file: a
/// namespace declared once is copied into every element and attribute that inherits it.
pub(crate) const MAX_IMPORT_BYTES: usize = 4 * MAX_IMPORT;

/// What an import may still copy: nodes and bytes.
struct ImportBudget {
    nodes: usize,
    bytes: usize,
}

impl ImportBudget {
    fn new() -> ImportBudget {
        ImportBudget { nodes: MAX_NODES as usize, bytes: MAX_IMPORT_BYTES }
    }

    fn take_bytes(&mut self, n: usize) -> Result<(), XfaError> {
        self.bytes = self
            .bytes
            .checked_sub(n)
            .ok_or_else(|| XfaError::TooLarge(format!("the data file expands to more than {} MB", MAX_IMPORT_BYTES >> 20)))?;
        Ok(())
    }
}

/// `n` as an [`ImportNode`], counting nodes and copied bytes against `budget`; `TooLarge` past
/// the depth or either budget.
fn import_node(n: roxmltree::Node, depth: usize, budget: &mut ImportBudget) -> Result<ImportNode, XfaError> {
    if depth > MAX_IMPORT_DEPTH {
        return Err(XfaError::TooLarge(format!("the data file nests more than {MAX_IMPORT_DEPTH} levels deep")));
    }
    budget.nodes = budget.nodes.checked_sub(1).ok_or_else(|| XfaError::TooLarge("the data file holds too many nodes".into()))?;
    let name = n.tag_name().name();
    let ns = n.tag_name().namespace();
    budget.take_bytes(name.len().saturating_add(ns.map_or(0, str::len)))?;
    let mut attrs = Vec::new();
    for a in n.attributes().filter(|a| a.name() != "xmlns" && a.namespace() != Some("http://www.w3.org/2000/xmlns/")) {
        budget.take_bytes(a.namespace().map_or(0, str::len).saturating_add(a.name().len()).saturating_add(a.value().len()))?;
        attrs.push((a.namespace().map(str::to_string), a.name().to_string(), a.value().to_string()));
    }
    let mut children = Vec::new();
    let mut text = String::new();
    for c in n.children() {
        if c.is_element() {
            children.push(import_node(c, depth + 1, budget)?);
        } else if let Some(t) = c.text() {
            budget.take_bytes(t.len())?;
            text.push_str(t);
        }
    }
    // A group's own text is layout whitespace.
    if !children.is_empty() && text.trim().is_empty() {
        text.clear();
    }
    Ok(ImportNode { name: name.to_string(), ns: ns.map(str::to_string), attrs, text, children })
}

/// `nodes` as markup for a packet whose XFA data namespace prefix is `xfa` (declared where
/// they go); other namespaces are declared on the elements and attributes that use them.
pub(super) fn serialize_nodes(nodes: &[ImportNode], xfa: &str) -> String {
    let mut out = String::new();
    for n in nodes {
        serialize_node(n, xfa, &mut out, 0);
    }
    out
}

fn serialize_node(n: &ImportNode, xfa: &str, out: &mut String, depth: usize) {
    if depth > MAX_IMPORT_DEPTH {
        return;
    }
    let mut decls: Vec<(String, String)> = Vec::new();
    let mut qualify = |ns: &Option<String>, name: &str, attr: bool| -> String {
        match ns.as_deref() {
            None => xml_name(name),
            Some(uri) if uri == XFA_DATA_NS => format!("{xfa}:{}", xml_name(name)),
            Some("http://www.w3.org/XML/1998/namespace") => format!("xml:{}", xml_name(name)),
            Some(uri) => {
                let p = match decls.iter().position(|(_, u)| u == uri) {
                    Some(i) => format!("n{i}"),
                    None => {
                        decls.push((format!("n{}", decls.len()), uri.to_string()));
                        format!("n{}", decls.len() - 1)
                    }
                };
                let _ = attr;
                format!("{p}:{}", xml_name(name))
            }
        }
    };
    let tag = qualify(&n.ns, &n.name, false);
    let attrs: Vec<String> = n
        .attrs
        .iter()
        .take(256)
        .map(|(ns, name, value)| {
            let q = qualify(ns, name, true);
            format!(" {q}=\"{}\"", escape(value).0)
        })
        .collect();
    out.push('<');
    out.push_str(&tag);
    for (p, uri) in &decls {
        out.push_str(&format!(" xmlns:{p}=\"{}\"", escape(uri).0));
    }
    for a in &attrs {
        out.push_str(a);
    }
    if n.text.is_empty() && n.children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    out.push_str(&escape(&n.text).0);
    for c in &n.children {
        serialize_node(c, xfa, out, depth + 1);
    }
    out.push_str(&format!("</{tag}>"));
}

/// Largest picture written into the data (its bytes, before base64): with the 64 MB cap on
/// all packets together, several pictures still fit.
const MAX_DATA_IMAGE: usize = 8 << 20;

/// Longest data file imported.
const MAX_IMPORT: usize = 16 << 20;

/// Put a picture into the data node at `path` as Designer saves one a user chose: base64, with
/// its content type (`image/png`, …) as `xfa:contentType` (in the packet's own prefix). The
/// node is made when missing; a data group is refused rather than wiped. `reuse`: the stream
/// this edit already wrote.
pub fn write_data_image(
    doc: &mut Document,
    path: &DataPath,
    content_type: &str,
    bytes: &[u8],
    reuse: Option<ObjRef>,
) -> Result<DatasetsWrite, XfaError> {
    use base64::Engine;
    if path.is_empty() {
        return Err(XfaError::Malformed("a picture needs a data path".into()));
    }
    if bytes.len() > MAX_DATA_IMAGE {
        return Err(XfaError::TooLarge(format!("the picture is larger than {} MB", MAX_DATA_IMAGE >> 20)));
    }
    let ct: String = content_type.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '.' | '+')).take(64).collect();
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let ensure = [Want { som: String::new(), path: path.clone(), text: String::new(), ensure: true }];
    rewrite(doc, &[Op::Values(&ensure), Op::Picture(path, &ct, &b64)], reuse)
}

/// What an import of a data file did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataImport {
    /// Element nodes the new data holds.
    pub nodes: usize,
    pub warnings: Vec<String>,
    /// The datasets stream written.
    pub stream: Option<ObjRef>,
}

/// The data a file holds, as the children of `xfa:data`: an XDP or datasets document (Export
/// Data's output, Designer's), a bare data element, or a data document whose root is the
/// form's root data node (named `expect_root`, when the caller knows it). Anything else is
/// refused: a form's data is never replaced by what is not its data.
pub fn parse_data_file(text: &str, expect_root: Option<&str>) -> Result<Vec<ImportNode>, XfaError> {
    if text.len() > MAX_IMPORT {
        return Err(XfaError::TooLarge(format!("the data file is larger than {} MB", MAX_IMPORT >> 20)));
    }
    let text = text.trim_start_matches('\u{feff}');
    if crate::xml_nesting_depth(text) > MAX_IMPORT_DEPTH {
        return Err(XfaError::TooLarge(format!("the data file nests more than {MAX_IMPORT_DEPTH} levels deep")));
    }
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    let parsed = roxmltree::Document::parse_with_options(text, opts).map_err(|e| XfaError::Malformed(format!("the data file is not XML: {e}")))?;
    let root = parsed.root_element();
    let local = root.tag_name().name();
    let in_xfa = |n: &roxmltree::Node| n.tag_name().namespace().is_none_or(|ns| ns == XFA_DATA_NS);
    let is = |n: &roxmltree::Node, name: &str| n.is_element() && n.tag_name().name() == name && in_xfa(n);
    let data = match local {
        "xdp" => {
            let ds = root.children().find(|n| is(n, "datasets")).ok_or_else(|| XfaError::Malformed("the XDP holds no datasets".into()))?;
            ds.children().find(|n| is(n, "data")).ok_or_else(|| XfaError::Malformed("the datasets hold no data element".into()))?
        }
        "datasets" if in_xfa(&root) => {
            root.children().find(|n| is(n, "data")).ok_or_else(|| XfaError::Malformed("the datasets hold no data element".into()))?
        }
        "data" if in_xfa(&root) => root,
        _ => {
            if root.tag_name().namespace().is_some() {
                return Err(XfaError::Malformed(format!("<{local}> is not XFA form data")));
            }
            if let Some(want) = expect_root
                && local != want
            {
                return Err(XfaError::Malformed(format!("the data file's root is <{local}>; this form's data root is <{want}>")));
            }
            let node = import_node(root, 0, &mut ImportBudget::new())?;
            return Ok(vec![node]);
        }
    };
    let mut budget = ImportBudget::new();
    let mut out = Vec::new();
    for c in data.children().filter(|c| c.is_element()) {
        out.push(import_node(c, 1, &mut budget)?);
    }
    Ok(out)
}

/// Replace the form's data with what the file `text` holds (see [`parse_data_file`]), keeping
/// the rest of the datasets (data descriptions, say). `reuse`: the stream this edit already
/// wrote.
pub fn import_data(doc: &mut Document, text: &str, expect_root: Option<&str>, reuse: Option<ObjRef>) -> Result<DataImport, XfaError> {
    let nodes = parse_data_file(text, expect_root)?;
    fn count(n: &[ImportNode]) -> usize {
        n.iter().map(|c| 1 + count(&c.children)).sum()
    }
    let r = rewrite(doc, &[Op::ReplaceData(&nodes)], reuse)?;
    if !r.written {
        return Err(XfaError::Malformed(r.warnings.first().cloned().unwrap_or_else(|| "the data could not be written".into())));
    }
    Ok(DataImport { nodes: count(&nodes), warnings: r.warnings, stream: r.stream })
}

/// The datasets packet's text: from the XFA array's `datasets` item, or the whole XDP.
pub(super) fn datasets_packet(doc: &Document) -> Result<Option<String>, XfaError> {
    let Some(root) = doc.root() else { return Ok(None) };
    let catalog = doc.get(root);
    let Some(acro) = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| doc.resolve(a)) else { return Ok(None) };
    let Some(xfa) = acro.as_dict().and_then(|a| a.get(b"XFA")).map(|x| doc.resolve(x)) else { return Ok(None) };
    let text_of = |o: &Object| -> Result<Option<String>, XfaError> {
        match &*doc.resolve(o) {
            Object::Stream(s) => Ok(crate::packets::decode(&s.decoded_within(64 << 20)?).map(|(t, _)| t)),
            _ => Ok(None),
        }
    };
    match &*xfa {
        Object::Array(items) => {
            let at = (0..items.len())
                .step_by(2)
                .find(|&i| items.get(i).and_then(|n| n.as_string()).is_some_and(|s| s.to_text() == "datasets") && i + 1 < items.len());
            match at.and_then(|i| items.get(i + 1)) {
                Some(o) => text_of(o),
                None => Ok(None),
            }
        }
        Object::Stream(_) => text_of(&xfa),
        _ => Ok(None),
    }
}

/// The data to export, as Acrobat's Export Data writes it: `xdp` false gives an XML document
/// holding the data alone (the form's root data node at the root, or the data element when
/// there are several); true gives an XDP holding the whole datasets and a reference to the
/// PDF, named `pdf_name`. A document whose packets can't be read is an error, not empty data.
pub fn export_data(doc: &Document, xdp: bool, pdf_name: &str) -> Result<String, XfaError> {
    if crate::read_packets(doc)?.is_none() {
        return Err(XfaError::NotXfa);
    }
    let packet = datasets_packet(doc)?;
    let (datasets, data) = match &packet {
        Some(text) => match datasets_markup(text) {
            Ok(Some(m)) => m,
            Ok(None) => (fresh_datasets(""), String::new()),
            Err(()) => return Err(XfaError::Malformed("the XFA data could not be read".into())),
        },
        None => (fresh_datasets(""), String::new()),
    };
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    if xdp {
        let (name, _) = escape(pdf_name);
        out.push_str("<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\">\n");
        out.push_str(&datasets);
        out.push_str(&format!("\n<pdf href=\"{name}\" xmlns=\"http://ns.adobe.com/xdp/pdf/\"/>\n</xdp:xdp>\n"));
    } else {
        out.push_str(if data.is_empty() { &datasets } else { &data });
        out.push('\n');
    }
    Ok(out)
}

/// The `xfa:datasets` element of `text` as written, and the data alone (its one child, or the
/// data element), each with the namespaces it uses declared on it. `Ok(None)`: no datasets
/// element; `Err`: the text can't be read.
fn datasets_markup(text: &str) -> Result<Option<(String, String)>, ()> {
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    if crate::xml_nesting_depth(text) > crate::MAX_XML_DEPTH {
        return Err(());
    }
    let wrapped = format!("{WRAP_OPEN}{text}{WRAP_CLOSE}");
    let (src, parsed) = match roxmltree::Document::parse_with_options(text, opts) {
        Ok(d) => (text, d),
        Err(_) => (wrapped.as_str(), roxmltree::Document::parse_with_options(&wrapped, opts).map_err(|_| ())?),
    };
    let Some(ds) = parsed.descendants().find(|n| n.is_element() && n.tag_name().name() == "datasets") else { return Ok(None) };
    let data = ds.children().find(|n| n.is_element() && n.tag_name().name() == "data");
    let data_alone = match data {
        Some(d) => {
            let kids: Vec<roxmltree::Node> = d.children().filter(|c| c.is_element()).collect();
            match kids.as_slice() {
                [one] => element_markup(src, *one).ok_or(())?,
                _ => element_markup(src, d).ok_or(())?,
            }
        }
        None => String::new(),
    };
    Ok(Some((element_markup(src, ds).ok_or(())?, data_alone)))
}

/// `n` as written in `src`, with the namespace declarations in scope that its start tag lacks
/// added to it, so it stands alone.
fn element_markup(src: &str, n: roxmltree::Node) -> Option<String> {
    let range = n.range();
    let text = src.get(range.clone())?;
    let tag_end = start_tag_end(src, &range)?;
    let start_tag = src.get(range.start..tag_end)?;
    let mut decls = String::new();
    for ns in n.namespaces() {
        let attr = match ns.name() {
            Some(p) => format!("xmlns:{p}="),
            None => "xmlns=".to_string(),
        };
        if !start_tag.contains(&attr) {
            decls.push_str(&format!(" {attr}\"{}\"", escape(ns.uri()).0));
        }
    }
    if decls.is_empty() {
        return Some(text.to_string());
    }
    let name_end = qname(src, &range)?.len() + 1;
    Some(format!("{}{decls}{}", text.get(..name_end)?, text.get(name_end..)?))
}
