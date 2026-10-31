//! The data model (`datasets` packet, XFA 3.3 part 1, "Data Binding"): reading the values a
//! form was saved with, and writing the values of its fields back, so Adobe's viewers (which
//! lay the form out from the XFA packets) show what was filled in here.
//!
//! Binding is the default "normal" one: a field's data node is named after the field and nested
//! under nodes named after its named ancestor subforms, repeated instances as repeated sibling
//! elements. Areas are named in SOM expressions but are not data scopes, so their contents bind
//! under the area's parent ([`data_path`]). Explicit `bind ref` expressions are not followed.

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

use crate::XfaError;
use crate::model::{Field, Node, SomIndices, Template, Ui};
use crate::packets::{Encoding, encode};

mod build;
mod dates;
mod import;
mod merge;
mod passwords;
mod path;
mod values;
mod write;

use build::MAX_INDEX;
pub use build::build_data;
#[cfg(test)]
pub(crate) use dates::iso_to_data;
pub(crate) use dates::{data_date_letters, data_to_iso, date_letters, date_tokens};
pub use dates::{iso_to_pattern, pattern_to_iso};
#[cfg(test)]
pub(crate) use import::MAX_IMPORT_BYTES;
pub use import::{DataImport, ImportNode, export_data, import_data, parse_data_file, write_data_image};
pub use passwords::{clear_password_values, has_password_values};
pub(crate) use path::template_field;
pub use path::{data_path, parse_datasets, som_to_path};
pub use values::{FieldData, FieldDatum, is_on, read_values, read_values_with};
#[cfg(test)]
pub(crate) use write::ENSURED;
pub use write::{
    DatasetsWrite, add_data_instances, remove_data_instance, write_data_ops, write_data_value, write_datasets, write_datasets_reusing,
    write_datasets_with,
};

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

    /// The value at `path`: its text, or, for a container whose children are all `value`
    /// elements (one way a multi-select list's picks are stored), those on separate lines.
    pub fn value_at(&self, path: &[(String, usize)]) -> Option<String> {
        let n = self.get(path)?;
        if n.children.is_empty() || !n.children.iter().all(|c| c.name == "value") {
            return Some(n.text.clone());
        }
        Some(n.children.iter().map(|c| c.text.trim()).filter(|t| !t.is_empty()).take(1000).collect::<Vec<_>>().join("\n"))
    }
}

/// One change to the data, for [`write_data_ops`].
#[derive(Clone, Debug, PartialEq)]
pub enum DataOp {
    /// Put `text` at the node `path`, adding the node (and the ones above it) as needed.
    Value { path: DataPath, text: String },
    /// Make sure the node at `parent` has at least `count` children named `name` (a script
    /// added rows: the layout repeats the subform once per data instance).
    Instances { parent: DataPath, name: String, count: usize },
    /// Take the node at `path` out (a script removed a row).
    Remove(DataPath),
}

impl DataNode {
    /// The node at `path`, made (with empty siblings before it) where missing; `None` past
    /// the index cap.
    fn ensure(&mut self, path: &[(String, usize)]) -> Option<&mut DataNode> {
        let mut node = self;
        for (name, idx) in path {
            if *idx >= MAX_INDEX {
                return None;
            }
            let have = node.children.iter().filter(|c| &c.name == name).count();
            for _ in have..=*idx {
                node.children.push(DataNode { name: name.clone(), ..Default::default() });
            }
            node = node.children.iter_mut().filter(|c| &c.name == name).nth(*idx)?;
        }
        Some(node)
    }

    /// Apply `op` to this tree (the root is the `xfa:data` element), as [`write_data_ops`]
    /// will to the packet, so scripts that run before it is written see the change.
    pub fn apply(&mut self, op: &DataOp) {
        match op {
            DataOp::Value { path, text } => {
                if let Some(n) = self.ensure(path)
                    && n.children.is_empty()
                {
                    n.text = text.clone();
                }
            }
            DataOp::Instances { parent, name, count } => {
                if let Some(&last) = count.checked_sub(1).as_ref() {
                    let mut path = parent.clone();
                    path.push((name.clone(), last.min(MAX_INDEX.saturating_sub(1))));
                    let _ = self.ensure(&path);
                }
            }
            DataOp::Remove(path) => {
                let Some(((name, idx), parent)) = path.split_last() else { return };
                let mut node = self;
                for (n, i) in parent {
                    let Some(next) = node.children.iter_mut().filter(|c| &c.name == n).nth(*i) else { return };
                    node = next;
                }
                if let Some(at) = node.children.iter().enumerate().filter(|(_, c)| &c.name == name).nth(*idx).map(|(at, _)| at) {
                    node.children.remove(at);
                }
            }
        }
    }
}
