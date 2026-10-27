use super::values::{FieldDatum, Want, wanted, with_template};
use super::*;

pub(super) fn xml_name(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' }).collect();
    if out.is_empty() || !(out.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')) {
        out.insert(0, '_');
    }
    out
}

/// `s` as XML character data. Characters XML 1.0 cannot hold are left out; the flag says
/// whether any were.
pub(super) fn escape(s: &str) -> (String, bool) {
    let mut out = String::with_capacity(s.len());
    let mut dropped = false;
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Parsers turn a raw carriage return into a line feed; a reference keeps it.
            '\r' => out.push_str("&#xD;"),
            '\u{FFFE}' | '\u{FFFF}' => dropped = true,
            c if c.is_control() && c != '\n' && c != '\t' => dropped = true,
            c => out.push(c),
        }
    }
    (out, dropped)
}

/// Most data nodes one write may create (instances skipped by a SOM index count too).
const MAX_NEW_NODES: usize = 20_000;

/// Highest same-name sibling index a write follows.
pub(super) const MAX_INDEX: usize = 10_000;

/// Most bytes of names the nodes one write creates hold together: every sibling an index
/// skips over gets its own copy of the name.
const MAX_NEW_NAME_BYTES: usize = 4 << 20;

/// What one write may still create: nodes and bytes of their names.
pub(super) struct NewNodes {
    nodes: usize,
    name_bytes: usize,
}

impl NewNodes {
    pub(super) fn new() -> NewNodes {
        NewNodes { nodes: MAX_NEW_NODES, name_bytes: MAX_NEW_NAME_BYTES }
    }
}

/// A data tree being built for writing.
#[derive(Default)]
pub(super) struct Build {
    name: String,
    pub(super) text: String,
    children: Vec<Build>,
    /// Positions in `children` by name, in order.
    by_name: HashMap<String, Vec<usize>>,
}

impl Build {
    /// The node at `path` below this one, created as needed. `None` when the path is deeper
    /// than [`MAX_DEPTH`], an index is above [`MAX_INDEX`], or creating it would spend more
    /// new nodes or name bytes than `budget` holds.
    pub(super) fn at(&mut self, path: &[(String, usize)], budget: &mut NewNodes) -> Option<&mut Build> {
        if path.len() > MAX_DEPTH {
            return None;
        }
        let mut node = self;
        for (name, idx) in path {
            if *idx > MAX_INDEX {
                return None;
            }
            let name = xml_name(name);
            let have = node.by_name.get(&name).map_or(0, Vec::len);
            if *idx >= have {
                let need = idx - have + 1;
                let bytes = need.saturating_mul(name.len());
                if need > budget.nodes || bytes > budget.name_bytes {
                    return None;
                }
                budget.nodes -= need;
                budget.name_bytes -= bytes;
                let start = node.children.len();
                node.children.extend((0..need).map(|_| Build { name: name.clone(), ..Default::default() }));
                node.by_name.entry(name.clone()).or_default().extend(start..start + need);
            }
            let pos = *node.by_name.get(&name)?.get(*idx)?;
            node = node.children.get_mut(pos)?;
        }
        Some(node)
    }

    /// The tree as XML. Depth is bounded by [`Build::at`].
    fn write(&self, out: &mut String) {
        if self.children.is_empty() {
            out.push_str(&format!("<{}>{}</{}>", self.name, escape(&self.text).0, self.name));
            return;
        }
        out.push_str(&format!("<{}>", self.name));
        self.write_children(out);
        out.push_str(&format!("</{}>", self.name));
    }

    pub(super) fn write_children(&self, out: &mut String) {
        for c in &self.children {
            c.write(out);
        }
    }
}

/// Note that a value could not be written (once per kind of reason, naming a few fields).
pub(super) fn skipped(warnings: &mut Vec<String>, reason: &str, som: &str) {
    const MAX_WARNINGS: usize = 20;
    if let Some(w) = warnings.iter_mut().find(|w| w.starts_with(reason)) {
        if w.matches(", ").count() < 4 {
            w.push_str(&format!(", {som}"));
        } else if !w.ends_with('…') {
            w.push_str(", …");
        }
    } else if warnings.len() < MAX_WARNINGS {
        warnings.push(format!("{reason}: {som}"));
    }
}

pub(super) const TOO_MANY: &str = "XFA data not written for fields whose data path is too deep or repeats too often";

pub(super) const STRUCTURED: &str = "XFA data not written for fields whose data node holds structured content";

pub(super) const CONFLICT: &str = "XFA data not written for fields bound to the same data node as another field";

pub(super) const CONTROL: &str = "Characters XML can't hold were left out of the XFA data of";

/// Fresh data for `wants`: the children of an `xfa:data` element.
pub(super) fn build_children(wants: &[Want], warnings: &mut Vec<String>) -> String {
    let mut root = Build::default();
    let mut budget = NewNodes::new();
    for w in wants {
        match root.at(&w.path, &mut budget) {
            Some(node) => node.text = w.text.clone(),
            None => skipped(warnings, TOO_MANY, &w.som),
        }
        if escape(&w.text).1 {
            skipped(warnings, CONTROL, &w.som);
        }
    }
    let mut out = String::new();
    root.write_children(&mut out);
    out
}

/// The `xfa:data` element for `fields`, built afresh.
pub fn build_data(doc: &Document, fields: &[FieldDatum]) -> String {
    let wants = with_template(doc, None, |t| wanted(doc, fields, t));
    format!("<xfa:data>{}</xfa:data>", build_children(&wants, &mut Vec::new()))
}

pub(super) fn fresh_datasets(data_children: &str) -> String {
    format!("<xfa:datasets xmlns:xfa=\"{XFA_DATA_NS}\"><xfa:data>{data_children}</xfa:data></xfa:datasets>")
}
