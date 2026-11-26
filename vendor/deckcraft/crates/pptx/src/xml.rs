//! Bounded XML DOM wrapper over [`craft_drawingml::xmlx::parse`] for callers that read kept
//! XML fragments (such as 3-D shape settings).

use crate::PptxError;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct El {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    El(El),
    Text(String),
}

fn local_of(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

impl El {
    pub fn local(&self) -> &str {
        local_of(&self.name)
    }

    pub fn prefix(&self) -> &str {
        self.name.split_once(':').map_or("", |(p, _)| p)
    }

    pub fn is(&self, local: &str) -> bool {
        self.local() == local
    }

    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        self.write_xml(&mut out);
        out
    }

    fn write_xml(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.name);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            craft_xml::push_esc_attr(out, v);
            out.push('"');
        }
        if self.children.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        for c in &self.children {
            match c {
                Node::El(e) => e.write_xml(out),
                Node::Text(t) => craft_xml::push_esc(out, t),
            }
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name || local_of(k) == name).map(|(_, v)| v.as_str())
    }

    pub fn elements(&self) -> impl Iterator<Item = &El> {
        self.children.iter().filter_map(|n| match n {
            Node::El(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn child(&self, local: &str) -> Option<&El> {
        self.elements().find(|e| e.local() == local)
    }

    pub fn children_named<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.elements().filter(move |e| e.local() == local)
    }

    pub fn path(&self, path: &[&str]) -> Option<&El> {
        path.iter().try_fold(self, |cur, name| cur.child(name))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub root: El,
}

fn from_craft_el(el: craft_xml::El) -> El {
    let children = el
        .children
        .into_iter()
        .filter_map(|n| match n {
            craft_xml::Node::El(c) => Some(Node::El(from_craft_el(c))),
            craft_xml::Node::Text(t) | craft_xml::Node::CData(t) => Some(Node::Text(t)),
            craft_xml::Node::Pi { .. } | craft_xml::Node::Comment(_) => None,
        })
        .collect();
    El { name: el.name, attrs: el.attrs, children }
}

pub fn parse(bytes: &[u8]) -> Result<Doc, PptxError> {
    let root = craft_drawingml::xmlx::parse(bytes).map_err(|e| PptxError::Xml(e.to_string()))?;
    Ok(Doc { root: from_craft_el(root) })
}
