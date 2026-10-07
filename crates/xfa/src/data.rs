//! The data model (`datasets` packet, XFA 3.3 part 1, "Data Binding"): reading the values a
//! form was saved with, and writing the values of its fields back, so Adobe's viewers (which
//! lay the form out from the XFA packets) show what was filled in here.
//!
//! Binding is the default "normal" one: a field's data node is named after the field and nested
//! under nodes named after its named ancestor subforms, repeated instances as repeated sibling
//! elements. Explicit `bind ref` expressions are not followed.

use printcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

use crate::XfaError;

const XFA_DATA_NS: &str = "http://www.xfa.org/schema/xfa-data/1.0/";
/// Most data nodes read.
const MAX_NODES: u32 = 1_000_000;
const MAX_DEPTH: usize = 64;

/// One element of the data document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataNode {
    pub name: String,
    pub text: String,
    pub children: Vec<DataNode>,
}

/// A path into the data: `(name, index among same-named siblings)` per level.
pub type DataPath = Vec<(String, usize)>;

impl DataNode {
    /// The node at `path` below this one.
    pub fn get(&self, path: &[(String, usize)]) -> Option<&DataNode> {
        let mut node = self;
        for (name, idx) in path {
            node = node.children.iter().filter(|c| &c.name == name).nth(*idx)?;
        }
        Some(node)
    }

    /// How many children of the node at `parent` are named `name`.
    pub fn count(&self, parent: &[(String, usize)], name: &str) -> usize {
        self.get(parent).map_or(0, |n| n.children.iter().filter(|c| c.name == name).count())
    }

    pub fn text_at(&self, path: &[(String, usize)]) -> Option<&str> {
        self.get(path).map(|n| n.text.as_str())
    }
}

fn to_node(n: roxmltree::Node, depth: usize) -> DataNode {
    let name = n.tag_name().name().to_string();
    let children: Vec<DataNode> =
        if depth < MAX_DEPTH { n.children().filter(|c| c.is_element()).map(|c| to_node(c, depth + 1)).collect() } else { Vec::new() };
    // The text of a leaf is its value; a container's own text is whitespace.
    let text = if children.is_empty() { n.children().filter_map(|c| c.text()).collect::<String>() } else { String::new() };
    DataNode { name, text, children }
}

/// The `xfa:data` element of the datasets packet in `xdp` (or a bare datasets document), as a
/// tree whose root is that element.
pub fn parse_datasets(xdp: &str) -> Option<DataNode> {
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    let doc = roxmltree::Document::parse_with_options(xdp, opts).ok()?;
    let datasets = doc.descendants().find(|n| n.is_element() && n.tag_name().name() == "datasets")?;
    let data = datasets.children().find(|n| n.is_element() && n.tag_name().name() == "data")?;
    Some(to_node(data, 0))
}

/// The data path a SOM expression names: `form[0].#subform[0].name[1]` → `[(form, 0), (name, 1)]`.
/// Unnamed containers (`#subform`) have no data node.
pub fn som_to_path(som: &str) -> DataPath {
    som.split('.')
        .filter_map(|seg| {
            let seg = seg.trim();
            if seg.is_empty() || seg.starts_with('#') || seg.starts_with('$') {
                return None;
            }
            let (name, idx) = match seg.split_once('[') {
                Some((n, rest)) => (n, rest.trim_end_matches(']').parse::<usize>().unwrap_or(0)),
                None => (seg, 0),
            };
            Some((name.to_string(), idx.min(100_000)))
        })
        .collect()
}

/// What a field holds, for writing the datasets.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldData {
    Text(String),
    /// Checked or not.
    Check(bool),
    /// The selected button's on value, if any.
    Radio(Option<String>),
    /// Nothing to write (buttons, signatures).
    None,
}

/// One field of the AcroForm, as the engine sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDatum {
    /// The terminal field dictionary (where `/PCSom` and `/PCItems` live).
    pub obj: ObjRef,
    /// The fully qualified field name (a static form's SOM path).
    pub name: String,
    pub data: FieldData,
}

fn text_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text()))
}

/// The SOM path of a field: its `/PCSom` when PrintCraft generated it, else its name.
fn som_of(doc: &Document, f: &FieldDatum) -> String {
    doc.get(f.obj).as_dict().and_then(|d| text_of(doc, d, crate::pdf::SOM_KEY)).unwrap_or_else(|| f.name.clone())
}

/// The on and off item values of a check box (`/PCItems`), or the widget's on state.
fn items_of(doc: &Document, f: &FieldDatum) -> (String, String) {
    let d = doc.get(f.obj);
    let items: Vec<String> = d
        .as_dict()
        .and_then(|d| d.get(b"PCItems").map(|o| doc.resolve(o)))
        .and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| x.as_string().map(|s| s.to_text())).collect()))
        .unwrap_or_default();
    let on = items.first().cloned().unwrap_or_else(|| "1".into());
    let off = items.get(1).cloned().unwrap_or_default();
    (on, off)
}

/// The Acrobat date pattern a field's format action names (`AFDate_FormatEx("yyyy-mm-dd")`).
fn date_pattern_of(doc: &Document, f: &FieldDatum) -> Option<String> {
    let d = doc.get(f.obj);
    let aa = doc.resolve(d.as_dict()?.get(b"AA")?);
    let fa = doc.resolve(aa.as_dict()?.get(b"F")?);
    let js = text_of(doc, fa.as_dict()?, b"JS")?;
    let i = js.find("AFDate_FormatEx(")?;
    let rest = &js[i + "AFDate_FormatEx(".len()..];
    let q = rest.find('"')?;
    let body = &rest[q + 1..];
    Some(body[..body.find('"')?].to_string())
}

/// The tokens of a date pattern: `yyyy`, `yy`, `mm`, `m`, `dd`, `d`, or a literal character.
fn date_tokens(pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].to_ascii_lowercase();
        if "ymd".contains(c) {
            let mut j = i;
            while j < chars.len() && chars[j].to_ascii_lowercase() == c {
                j += 1;
            }
            out.push(std::iter::repeat_n(c, j - i).collect());
            i = j;
        } else {
            out.push(chars[i].to_string());
            i += 1;
        }
    }
    out
}

/// `2001-02-03` shown in `pattern` (`mm/dd/yyyy` → `02/03/2001`). Unknown patterns and
/// non-dates come back as they are.
pub fn iso_to_pattern(iso: &str, pattern: &str) -> String {
    let parts: Vec<&str> = iso.trim().splitn(3, '-').collect();
    let [y, m, d] = parts[..] else { return iso.to_string() };
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<u32>(), m.parse::<u32>(), d.parse::<u32>()) else { return iso.to_string() };
    let mut out = String::new();
    for t in date_tokens(pattern) {
        match t.as_str() {
            "yyyy" => out.push_str(&format!("{y:04}")),
            "yy" => out.push_str(&format!("{:02}", y % 100)),
            "mm" => out.push_str(&format!("{m:02}")),
            "m" => out.push_str(&m.to_string()),
            "dd" => out.push_str(&format!("{d:02}")),
            "d" => out.push_str(&d.to_string()),
            other => out.push_str(other),
        }
    }
    out
}

/// A date typed as `pattern` back to `yyyy-mm-dd`. `None` when it doesn't match.
pub fn pattern_to_iso(text: &str, pattern: &str) -> Option<String> {
    let text = text.trim();
    let (mut y, mut m, mut d) = (None, None, None);
    let mut rest = text;
    let tokens = date_tokens(pattern);
    for (i, t) in tokens.iter().enumerate() {
        match t.as_str() {
            "yyyy" | "yy" | "mm" | "m" | "dd" | "d" => {
                // Digits up to the next literal (or a fixed width).
                let fixed = matches!(t.as_str(), "yyyy" | "yy" | "mm" | "dd");
                let width = if fixed {
                    t.len()
                } else {
                    rest.chars().take_while(|c| c.is_ascii_digit()).count().min(if t == "d" || t == "m" { 2 } else { 4 })
                };
                let digits: String = rest.chars().take(width).collect();
                if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                let v: u32 = digits.parse().ok()?;
                rest = &rest[digits.len()..];
                match t.as_str() {
                    "yyyy" => y = Some(v),
                    "yy" => y = Some(if v < 50 { 2000 + v } else { 1900 + v }),
                    "mm" | "m" => m = Some(v),
                    _ => d = Some(v),
                }
            }
            lit => {
                rest = rest.strip_prefix(lit)?;
            }
        }
        let _ = i;
    }
    if !rest.is_empty() {
        return None;
    }
    let (y, m, d) = (y?, m?, d?);
    (1..=12).contains(&m).then(|| format!("{y:04}-{m:02}-{d:02}")).filter(|_| (1..=31).contains(&d))
}

fn xml_name(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' }).collect();
    if out.is_empty() || !(out.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')) {
        out.insert(0, '_');
    }
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if c.is_control() && c != '\n' && c != '\t' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// A data tree being built for writing.
#[derive(Default)]
struct Build {
    name: String,
    text: String,
    children: Vec<Build>,
}

impl Build {
    fn at(&mut self, path: &[(String, usize)]) -> &mut Build {
        let mut node = self;
        for (name, idx) in path {
            let name = xml_name(name);
            while node.children.iter().filter(|c| c.name == name).count() <= *idx {
                node.children.push(Build { name: name.clone(), ..Default::default() });
            }
            let pos = node.children.iter().enumerate().filter(|(_, c)| c.name == name).nth(*idx).map(|(i, _)| i).unwrap_or(0);
            node = &mut node.children[pos];
        }
        node
    }

    fn write(&self, out: &mut String, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        if self.children.is_empty() {
            out.push_str(&format!("<{}>{}</{}>", self.name, escape(&self.text), self.name));
            return;
        }
        out.push_str(&format!("<{}>", self.name));
        for c in &self.children {
            c.write(out, depth + 1);
        }
        out.push_str(&format!("</{}>", self.name));
    }
}

/// The `xfa:data` element for `fields`.
pub fn build_data(doc: &Document, fields: &[FieldDatum]) -> String {
    let mut root = Build::default();
    for f in fields {
        let path = som_to_path(&som_of(doc, f));
        if path.is_empty() {
            continue;
        }
        let text = match &f.data {
            FieldData::Text(t) => match date_pattern_of(doc, f) {
                Some(p) => pattern_to_iso(t, &p).unwrap_or_else(|| t.clone()),
                None => t.clone(),
            },
            FieldData::Check(on) => {
                let (on_item, off_item) = items_of(doc, f);
                if *on { on_item } else { off_item }
            }
            FieldData::Radio(sel) => sel.clone().unwrap_or_default(),
            FieldData::None => continue,
        };
        root.at(&path).text = text;
    }
    let mut out = String::from("<xfa:data>");
    for c in &root.children {
        c.write(&mut out, 0);
    }
    out.push_str("</xfa:data>");
    out
}

/// The datasets packet text with its `xfa:data` replaced by `data` (other children, such as
/// a data description, stay). `None` gets a fresh packet.
fn replace_data(existing: Option<&str>, data: &str) -> String {
    let fresh = || format!("<xfa:datasets xmlns:xfa=\"{XFA_DATA_NS}\">{data}</xfa:datasets>");
    let Some(text) = existing else { return fresh() };
    let Some(start) = text.find("<xfa:data>").or_else(|| text.find("<xfa:data ")).or_else(|| text.find("<xfa:data/>")) else {
        // A datasets element without data: put ours in front of its end tag.
        return match text.rfind("</xfa:datasets>") {
            Some(end) => format!("{}{data}{}", &text[..end], &text[end..]),
            None => fresh(),
        };
    };
    let after = &text[start..];
    let end = if after.starts_with("<xfa:data/>") {
        start + "<xfa:data/>".len()
    } else {
        match after.find("</xfa:data>") {
            Some(e) => start + e + "</xfa:data>".len(),
            None => return fresh(),
        }
    };
    format!("{}{data}{}", &text[..start], &text[end..])
}

fn stream_text(doc: &Document, o: &Object) -> Option<String> {
    let s = doc.resolve(o);
    let Object::Stream(s) = &*s else { return None };
    s.decoded_within(64 << 20).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn text_stream(text: &str) -> Stream {
    Stream::flate(Dict::new(), text.as_bytes())
}

/// Write the fields' values into the datasets packet. Returns `false` when the document has no
/// XFA entry to write into.
pub fn write_datasets(doc: &mut Document, fields: &[FieldDatum]) -> Result<bool, XfaError> {
    let Some(root) = doc.root() else { return Ok(false) };
    let catalog = doc.get(root);
    let Some(acro_obj) = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).cloned() else { return Ok(false) };
    let acro_ref = acro_obj.as_ref();
    let Some(mut acro) = doc.resolve(&acro_obj).as_dict().cloned() else { return Ok(false) };
    let Some(xfa) = acro.get(b"XFA").cloned() else { return Ok(false) };
    let data = build_data(doc, fields);
    match &*doc.resolve(&xfa) {
        Object::Array(items) => {
            let mut items = items.clone();
            let mut i = 0;
            while i + 1 < items.len() {
                if items.get(i).and_then(|n| n.as_string()).is_some_and(|s| s.to_text() == "datasets") {
                    let existing = items.get(i + 1).and_then(|o| stream_text(doc, o));
                    let packet = replace_data(existing.as_deref(), &data);
                    let r = doc.add(Object::Stream(text_stream(&packet)));
                    items[i + 1] = Object::Ref(r);
                    return finish(doc, acro_ref, &mut acro, root, items);
                }
                i += 2;
            }
            // No datasets packet yet: before the postamble, or last.
            let r = doc.add(Object::Stream(text_stream(&replace_data(None, &data))));
            let pos = items.iter().position(|n| n.as_string().is_some_and(|s| s.to_text() == "postamble")).unwrap_or(items.len());
            items.insert(pos, Object::Ref(r));
            items.insert(pos, Object::String(PdfString::text("datasets")));
            finish(doc, acro_ref, &mut acro, root, items)
        }
        Object::Stream(_) => {
            let Some(text) = stream_text(doc, &xfa) else { return Ok(false) };
            let new = match (text.find("<xfa:datasets"), text.find("</xfa:datasets>")) {
                (Some(a), Some(b)) if b > a => {
                    let end = b + "</xfa:datasets>".len();
                    format!("{}{}{}", &text[..a], replace_data(Some(&text[a..end]), &data), &text[end..])
                }
                _ => match text.rfind("</xdp:xdp>") {
                    Some(e) => format!("{}{}{}", &text[..e], replace_data(None, &data), &text[e..]),
                    None => return Ok(false),
                },
            };
            let r = doc.add(Object::Stream(text_stream(&new)));
            acro.set(b"XFA".to_vec(), Object::Ref(r));
            match acro_ref {
                Some(ar) => doc.set(ar, Object::Dict(acro)),
                None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?,
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn finish(doc: &mut Document, acro_ref: Option<ObjRef>, acro: &mut Dict, root: ObjRef, items: Vec<Object>) -> Result<bool, XfaError> {
    acro.set(b"XFA".to_vec(), Object::Array(items));
    match acro_ref {
        Some(ar) => doc.set(ar, Object::Dict(acro.clone())),
        None => {
            let a = acro.clone();
            doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(a)))?;
        }
    }
    Ok(true)
}

/// The values the datasets hold for `fields`: `(field name, what it should hold)`, only for
/// fields the data mentions.
pub fn read_values(doc: &Document, fields: &[FieldDatum]) -> Vec<(String, FieldData)> {
    let Ok(Some(p)) = crate::read_packets(doc) else { return Vec::new() };
    let Some(data) = parse_datasets(&p.xdp) else { return Vec::new() };
    let mut out = Vec::new();
    for f in fields {
        let path = som_to_path(&som_of(doc, f));
        let Some(text) = data.text_at(&path) else { continue };
        let value = match &f.data {
            FieldData::Text(_) => FieldData::Text(match date_pattern_of(doc, f) {
                Some(pat) => iso_to_pattern(text, &pat),
                None => text.to_string(),
            }),
            FieldData::Check(_) => {
                let (on, _) = items_of(doc, f);
                FieldData::Check(is_on(text, &on))
            }
            FieldData::Radio(_) => FieldData::Radio((!text.trim().is_empty()).then(|| text.trim().to_string())),
            FieldData::None => continue,
        };
        out.push((f.name.clone(), value));
    }
    out
}

/// Does a data value mean "checked" for a button whose on value is `on`?
pub fn is_on(text: &str, on: &str) -> bool {
    let t = text.trim();
    if t == on {
        return true;
    }
    !matches!(t.to_ascii_lowercase().as_str(), "" | "0" | "off" | "false" | "no") && on.trim().is_empty()
}
