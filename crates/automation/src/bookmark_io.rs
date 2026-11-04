//! Bookmark trees in and out of documents: `bookmark_export`, `bookmark_import`, `bookmark_copy`.
//!
//! The portable form is JSON: `{"bookmarks": [node, …]}`, where a node is `title`, `page`
//! (1-based, or `null` for one that goes nowhere), `view` (`{"type", "values"}`: a destination
//! type and its values in PDF user space, as the file stores them), `color`, `bold`, `italic`,
//! `open` and `children`. What export writes, import takes back unchanged. A file also carries
//! `"truncated": true` when the export left out bookmarks nested past the listing limit.
use crate::{Args, Automation, Result, ToolError, failed};
use pdfcraft_engine::{DocId, Edit, OutlineNode};
use serde_json::{Value, json};

/// Deepest nesting a bookmark tree may have (the listing reaches 33 levels).
const MAX_DEPTH: usize = 33;

impl Automation {
    pub(crate) fn bookmark_export(&self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let path = a.opt_path("path")?.unwrap_or_default();
        let tree = self.outline_of(id, &path)?;
        let mut out = json!({ "bookmarks": tree.nodes.iter().map(node_json).collect::<Vec<_>>(), "truncated": tree.truncated });
        if let Some(file) = a.opt_str("out")? {
            let target = self.resolve(file, true)?;
            let mut saved = json!({ "bookmarks": out["bookmarks"] });
            if tree.truncated {
                saved["truncated"] = json!(true);
            }
            let text = serde_json::to_vec_pretty(&saved).map_err(failed)?;
            crate::write_atomic(&target, &text)?;
            out["path"] = json!(target);
        }
        Ok(out)
    }

    pub(crate) fn bookmark_import(&mut self, a: &Args) -> Result<Value> {
        let (nodes, truncated) = match (a.get("bookmarks"), a.opt_str("file")?) {
            (Some(list), None) => (nodes_of(list)?, false),
            (None, Some(file)) => {
                let path = self.resolve(file, false)?;
                let value: Value = serde_json::from_slice(&crate::read_input(&path)?).map_err(|e| failed(format!("not a bookmark file: {e}")))?;
                let list = value.get("bookmarks").ok_or_else(|| failed("not a bookmark file: it has no \"bookmarks\" list"))?;
                (nodes_of(list)?, value.get("truncated") == Some(&Value::Bool(true)))
            }
            _ => return Err(ToolError::InvalidArgs("give either bookmarks or file".into())),
        };
        let mut out = self.write_tree(a, nodes)?;
        out["truncated"] = json!(truncated);
        Ok(out)
    }

    pub(crate) fn bookmark_copy(&mut self, a: &Args) -> Result<Value> {
        let from = a.int("from")?;
        let from = DocId(u64::try_from(from).map_err(|_| ToolError::InvalidArgs("from must be positive".into()))?);
        let path = a.opt_path("path")?.unwrap_or_default();
        let tree = self.outline_of(from, &path)?;
        let mut out = self.write_tree(a, tree.nodes)?;
        out["truncated"] = json!(tree.truncated);
        Ok(out)
    }

    fn outline_of(&self, id: DocId, path: &[usize]) -> Result<pdfcraft_engine::OutlineTree> {
        match self.session.outline_tree(id, path) {
            // The engine names the path 0-based; the caller gave it 1-based.
            Some(tree) => tree.map_err(|e| match e {
                pdfcraft_organize::OutlineError::NoSuchBookmark(_) => failed(format!("there is no bookmark at {:?}", one_based(path))),
                e => failed(e),
            }),
            None => Err(failed(format!("document {} is not open, or its bookmarks can't be read for editing", id.0))),
        }
    }

    /// Write `nodes` into `doc` under `parent` at `position` (or in place of the outline with
    /// `replace`), and say which of them went nowhere because the document lacks their page.
    fn write_tree(&mut self, a: &Args, bookmarks: Vec<OutlineNode>) -> Result<Value> {
        let parent = a.opt_path("parent")?.unwrap_or_default();
        let index = a.opt_int("position")?.map_or(usize::MAX, |p| usize::try_from(p.max(1) - 1).unwrap_or(usize::MAX));
        let replace = a.opt_bool("replace")?.unwrap_or(false);
        let count = self.doc(a)?.info.pages.len();
        let unmapped = pdfcraft_engine::outline_unmapped(&bookmarks, count);
        let mut out = self.apply(a, Edit::ImportBookmarks { parent, index, bookmarks, replace })?;
        out["unmapped"] = json!(unmapped.iter().map(|p| one_based(p)).collect::<Vec<_>>());
        Ok(out)
    }
}

fn one_based(path: &[usize]) -> Vec<usize> {
    path.iter().map(|i| i.saturating_add(1)).collect()
}

/// A node as JSON, leaving out what is at its default (no view, black, regular, no children).
fn node_json(n: &OutlineNode) -> Value {
    let mut v = json!({ "title": n.title, "page": n.page.map(|p| p.saturating_add(1)), "open": n.open });
    if let Some((kind, values)) = &n.view {
        v["view"] = json!({ "type": String::from_utf8_lossy(kind), "values": values });
    }
    if let Some(c) = n.color {
        v["color"] = json!(c);
    }
    if n.bold {
        v["bold"] = json!(true);
    }
    if n.italic {
        v["italic"] = json!(true);
    }
    if !n.children.is_empty() {
        v["children"] = json!(n.children.iter().map(node_json).collect::<Vec<_>>());
    }
    v
}

fn nodes_of(list: &Value) -> Result<Vec<OutlineNode>> {
    let mut count = 0;
    list_of(list, &[], &mut count)
}

/// The nodes of one level; `at` is the 1-based path of their parent, for messages.
fn list_of(list: &Value, at: &[usize], count: &mut usize) -> Result<Vec<OutlineNode>> {
    let items = list.as_array().ok_or_else(|| bad(at, "children must be a list of bookmarks"))?;
    if at.len() >= MAX_DEPTH && !items.is_empty() {
        return Err(bad(at, &format!("bookmarks may nest at most {MAX_DEPTH} levels deep")));
    }
    *count = count.saturating_add(items.len());
    if *count > pdfcraft_engine::MAX_IMPORT_BOOKMARKS {
        return Err(ToolError::InvalidArgs(format!("at most {} bookmarks can be imported at once", pdfcraft_engine::MAX_IMPORT_BOOKMARKS)));
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let mut path = at.to_vec();
        path.push(i.saturating_add(1));
        out.push(node_of(item, &path, count)?);
    }
    Ok(out)
}

/// One node and, through [`list_of`], its children (recursion bounded by `MAX_DEPTH`).
fn node_of(v: &Value, at: &[usize], count: &mut usize) -> Result<OutlineNode> {
    let o = v.as_object().ok_or_else(|| bad(at, "must be an object"))?;
    let field = |k: &str| o.get(k).filter(|x| !x.is_null());
    // An empty title is kept: files have them, and export writes them as "".
    let title = field("title").and_then(Value::as_str).ok_or_else(|| bad(at, "needs a title"))?.to_string();
    let page = match field("page") {
        None => None,
        Some(p) => Some(p.as_u64().filter(|p| *p >= 1).and_then(|p| usize::try_from(p - 1).ok()).ok_or_else(|| bad(at, "page must be 1 or more"))?),
    };
    let flag = |k: &str| field(k).map_or(Ok(None), |x| x.as_bool().map(Some).ok_or_else(|| bad(at, &format!("{k} must be true or false"))));
    let color = match field("color") {
        None => None,
        Some(c) => {
            let rgb: Option<Vec<f32>> = c.as_array().and_then(|c| c.iter().map(|x| x.as_f64().map(|x| x as f32)).collect());
            match rgb.as_deref() {
                Some(&[r, g, b]) if [r, g, b].iter().all(|c| (0.0..=1.0).contains(c)) => Some([r, g, b]),
                _ => return Err(bad(at, "color must be [r, g, b], each from 0 to 1")),
            }
        }
    };
    let children = match field("children") {
        None => Vec::new(),
        Some(list) => list_of(list, at, count)?,
    };
    Ok(OutlineNode {
        title,
        page,
        view: field("view").map(|x| view_of(x, at)).transpose()?,
        color,
        italic: flag("italic")?.unwrap_or(false),
        bold: flag("bold")?.unwrap_or(false),
        open: flag("open")?.unwrap_or(true),
        children,
    })
}

/// `{"type": "XYZ", "values": [left, top, zoom]}`: a destination type with exactly its values.
fn view_of(v: &Value, at: &[usize]) -> Result<(Vec<u8>, Vec<Option<f64>>)> {
    let kind = v.get("type").and_then(Value::as_str).ok_or_else(|| bad(at, "view needs a type such as XYZ or Fit"))?;
    let values = match v.get("values").filter(|x| !x.is_null()) {
        None => Vec::new(),
        Some(list) => {
            let list = list.as_array().filter(|l| l.len() <= 4).ok_or_else(|| bad(at, "view values must be a list of at most 4 numbers or nulls"))?;
            list.iter()
                .map(|x| match x {
                    Value::Null => Ok(None),
                    x => x.as_f64().map(Some).ok_or_else(|| bad(at, "view values must be numbers or null")),
                })
                .collect::<Result<_>>()?
        }
    };
    let arity = pdfcraft_engine::outline_view_arity(kind.as_bytes()).ok_or_else(|| bad(at, VIEW_TYPES))?;
    if values.len() != arity {
        return Err(bad(at, &format!("a {kind} view takes {arity} values (numbers, or null to keep the reader's)")));
    }
    if kind == "FitR" && values.iter().any(Option::is_none) {
        return Err(bad(at, "a FitR view needs all four values: left, bottom, right, top"));
    }
    Ok((kind.as_bytes().to_vec(), values))
}

const VIEW_TYPES: &str = "view type must be XYZ (3 values: left, top, zoom), Fit, FitB (none), FitH, FitV, FitBH, FitBV (1) or FitR (4)";

fn bad(at: &[usize], what: &str) -> ToolError {
    if at.is_empty() {
        return ToolError::InvalidArgs(format!("bookmarks: {what}"));
    }
    ToolError::InvalidArgs(format!("bookmark {at:?}: {what}"))
}
