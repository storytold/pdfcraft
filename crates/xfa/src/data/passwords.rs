use super::import::datasets_packet;
use super::merge::Op;
use super::path::{data_path, parse_datasets};
use super::values::{Want, path_of, text_of, with_template};
use super::write::{datasets_stream_ref, rewrite};
use super::*;

/// The SOM path and default value of every AcroForm text field with the Password flag (bit 14,
/// `1 << 13`) in `doc`, for static XFA forms whose password fields set the flag in the AcroForm
/// dictionary.
fn acro_password_fields(doc: &Document) -> Vec<(String, String)> {
    const PASSWORD_FLAG: i64 = 1 << 13;
    let Some(root) = doc.root() else { return Vec::new() };
    let catalog = doc.get(root);
    let Some(acro) = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| doc.resolve(a)) else { return Vec::new() };
    let Some(top) = acro.as_dict().and_then(|a| a.get(b"Fields")).map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()) else {
        return Vec::new();
    };
    struct AcroWalk<'a> {
        doc: &'a Document,
        seen: std::collections::HashSet<ObjRef>,
        out: Vec<(String, String)>,
    }
    impl AcroWalk<'_> {
        fn walk(
            &mut self,
            items: &[Object],
            parent_name: &str,
            inherited_ft: Option<&[u8]>,
            inherited_ff: i64,
            inherited_dv: Option<&str>,
            depth: usize,
        ) {
            if depth > 32 {
                return;
            }
            for item in items {
                let Some(r) = item.as_ref() else { continue };
                if !self.seen.insert(r) {
                    continue;
                }
                let obj = self.doc.get(r);
                let Some(d) = obj.as_dict() else { continue };
                let ft = d.name(b"FT").or(inherited_ft);
                let ff = d.int(b"Ff").unwrap_or(inherited_ff);
                let dv_own = text_of(self.doc, d, b"DV");
                let dv = dv_own.as_deref().or(inherited_dv);
                let t = text_of(self.doc, d, b"T");
                let full_name = match (&t, parent_name.is_empty()) {
                    (Some(seg), true) => seg.clone(),
                    (Some(seg), false) => format!("{parent_name}.{seg}"),
                    (None, _) => parent_name.to_string(),
                };
                let kids = d.get(b"Kids").map(|k| self.doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
                let has_named_kid = kids.iter().filter_map(Object::as_ref).any(|kr| self.doc.get(kr).as_dict().is_some_and(|kd| kd.contains(b"T")));
                if !has_named_kid && ft == Some(b"Tx") && (ff & PASSWORD_FLAG) != 0 {
                    let som = text_of(self.doc, d, crate::pdf::SOM_KEY).unwrap_or_else(|| full_name.clone());
                    if !som.is_empty() {
                        self.out.push((som, dv.unwrap_or_default().to_string()));
                    }
                }
                if !kids.is_empty() {
                    self.walk(&kids, &full_name, ft, ff, dv, depth + 1);
                }
            }
        }
    }
    let mut w = AcroWalk { doc, seen: std::collections::HashSet::new(), out: Vec::new() };
    w.walk(&top, "", None, 0, None, 0);
    w.out
}

/// The `Want` entries needed to restore password fields in `doc`'s datasets packet to their
/// defaults (or empty when they have no default).
fn password_dataset_wants(doc: &Document) -> Vec<Want> {
    if datasets_stream_ref(doc).is_none() {
        return Vec::new();
    }
    let Ok(Some(packet)) = datasets_packet(doc) else { return Vec::new() };
    let Some(data) = parse_datasets(&packet) else { return Vec::new() };
    with_template(doc, None, |tpl| {
        let mut candidates: Vec<(String, DataPath, String)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if let Some(t) = tpl {
            struct Walk<'a> {
                tpl: &'a Template,
                data: &'a DataNode,
                nodes: usize,
                seen: &'a mut std::collections::HashSet<DataPath>,
                out: &'a mut Vec<(String, DataPath, String)>,
            }
            impl Walk<'_> {
                fn child_som(parent: &str, name: Option<&str>, index: usize) -> String {
                    match name {
                        Some(n) if !parent.is_empty() => format!("{parent}.{n}[{index}]"),
                        Some(n) => format!("{n}[{index}]"),
                        None => parent.to_string(),
                    }
                }
                fn walk(&mut self, children: &[Node], parent_som: &str, depth: usize) {
                    if depth > MAX_DEPTH || self.nodes >= crate::script::MAX_FORM_NODES {
                        return;
                    }
                    let sibs = SomIndices::of(children);
                    for (i, n) in children.iter().enumerate() {
                        if self.nodes >= crate::script::MAX_FORM_NODES {
                            break;
                        }
                        self.nodes += 1;
                        let sib = sibs.at(i);
                        match n {
                            Node::Subform(sf) => {
                                let from_data = match &sf.common.name {
                                    Some(name) if sf.occur.max != Some(1) => self.data.count(&data_path(self.tpl, parent_som), name),
                                    _ => 0,
                                };
                                let instances = crate::layout::instance_count(&sf.occur, from_data).max(from_data);
                                for k in 0..instances {
                                    if k > 0 {
                                        if self.nodes >= crate::script::MAX_FORM_NODES {
                                            break;
                                        }
                                        self.nodes += 1;
                                    }
                                    let som = Self::child_som(parent_som, sf.common.name.as_deref(), sib + k);
                                    self.walk(&sf.children, &som, depth + 1);
                                }
                            }
                            Node::Area(a) => {
                                let som = Self::child_som(parent_som, a.common.name.as_deref(), sib);
                                self.walk(&a.children, &som, depth + 1);
                            }
                            Node::Field(f) if f.ui == Ui::PasswordEdit => {
                                let som = Self::child_som(parent_som, f.common.name.as_deref().or(Some("field")), sib);
                                let path = data_path(self.tpl, &som);
                                if !path.is_empty() && self.seen.insert(path.clone()) {
                                    let default = f.value.plain().map(|v| f.saved_value(v.trim())).unwrap_or_default();
                                    self.out.push((som, path, default));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            let root = Node::Subform(Box::new(t.root.clone()));
            let mut w = Walk { tpl: t, data: &data, nodes: 0, seen: &mut seen, out: &mut candidates };
            w.walk(std::slice::from_ref(&root), "", 0);
            if let Some(ps) = &t.root.page_set {
                for (i, area) in ps.areas.iter().enumerate() {
                    let som = area.name.clone().map(|n| format!("{n}[0]")).unwrap_or_else(|| format!("#pageArea[{i}]"));
                    w.walk(&area.items, &som, 1);
                }
            }
        }
        for (som, default) in acro_password_fields(doc) {
            let path = path_of(tpl, &som);
            if !path.is_empty() && seen.insert(path.clone()) {
                candidates.push((som, path, default));
            }
        }
        candidates
            .into_iter()
            .filter_map(|(som, path, default)| {
                let cur = data.value_at(&path)?;
                (!cur.is_empty() && cur != default).then_some(Want { som, path, text: default, ensure: false })
            })
            .collect()
    })
}

/// Whether any XFA password field (`<passwordEdit/>` or an AcroForm text field with the Password
/// flag) holds a value in `doc`'s datasets packet beyond its default (#3944).
pub fn has_password_values(doc: &Document) -> bool {
    !password_dataset_wants(doc).is_empty()
}

/// Clear typed password values from `doc`'s XFA datasets packet (restoring a template default
/// where the field has one, else emptying the element), replacing the datasets stream in place
/// (#3944). Returns how many password fields were cleared in the datasets.
pub fn clear_password_values(doc: &mut Document) -> Result<usize, XfaError> {
    let wants = password_dataset_wants(doc);
    if wants.is_empty() {
        return Ok(0);
    }
    let count = wants.len();
    let reuse = datasets_stream_ref(doc);
    let report = rewrite(doc, &[Op::Values(&wants)], reuse)?;
    if !report.written || !password_dataset_wants(doc).is_empty() {
        let reason = report.warnings.first().map_or("the XFA datasets password values could not be cleared", String::as_str);
        return Err(XfaError::Malformed(reason.to_string()));
    }
    Ok(count)
}
