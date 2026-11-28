use super::import::datasets_packet;
use super::merge::Op;
use super::path::{data_path, parse_datasets_packet};
use super::values::{Want, path_of, text_of, with_template};
use super::write::{datasets_stream_ref, rewrite};
use super::{DataNode, DataPath, Document, MAX_DEPTH, MAX_SOM_INDEX, Node, ObjRef, Object, SomIndices, Template, Ui, XfaError};

/// How deep the AcroForm field tree is walked for password fields: as deep as the form's own
/// field walk (`pdfcraft_forms::fields`), which is what editing and dataset synchronization see.
const MAX_ACRO_DEPTH: usize = 64;

/// The SOM path and default value of every AcroForm text field with the Password flag (bit 14,
/// `1 << 13`) in `doc`, for static XFA forms whose password fields set the flag in the AcroForm
/// dictionary. `None` when the walk was cut short at [`MAX_ACRO_DEPTH`]: which branch is cut
/// depends on the order the tree is visited in, so a field the form's own walk reaches may have
/// been missed, and the caller can't tell that no password field is left.
fn acro_password_fields(doc: &Document) -> Option<Vec<(String, String)>> {
    const PASSWORD_FLAG: i64 = 1 << 13;
    let Some(root) = doc.root() else { return Some(Vec::new()) };
    let catalog = doc.get(root);
    let Some(acro) = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| doc.resolve(a)) else { return Some(Vec::new()) };
    let Some(top) = acro.as_dict().and_then(|a| a.get(b"Fields")).map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()) else {
        return Some(Vec::new());
    };
    struct AcroWalk<'a> {
        doc: &'a Document,
        /// Each field reached, with what it was reached with: its parent's name, type, flags and
        /// default, which decide what it classifies as.
        seen: std::collections::HashMap<ObjRef, Reach>,
        /// Fields past the depth limit, with what they were reached with. One reached again from
        /// another root in the same way was classified there; any other is not fully read.
        cut: Vec<(ObjRef, Reach)>,
        out: Vec<(String, String)>,
    }
    type Reach = (String, Option<Vec<u8>>, i64, Option<String>);
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
            if depth > MAX_ACRO_DEPTH {
                let reach: Reach = (parent_name.to_string(), inherited_ft.map(<[u8]>::to_vec), inherited_ff, inherited_dv.map(str::to_string));
                self.cut.extend(items.iter().filter_map(Object::as_ref).map(|r| (r, reach.clone())));
                return;
            }
            for item in items {
                let Some(r) = item.as_ref() else { continue };
                let reach: Reach = (parent_name.to_string(), inherited_ft.map(<[u8]>::to_vec), inherited_ff, inherited_dv.map(str::to_string));
                // The reach a field was classified with stays: a later visit that skips it classified
                // nothing, so it must not stand in for the cut-off one.
                if let std::collections::hash_map::Entry::Vacant(slot) = self.seen.entry(r) {
                    slot.insert(reach);
                } else {
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
    let mut w = AcroWalk { doc, seen: std::collections::HashMap::new(), cut: Vec::new(), out: Vec::new() };
    w.walk(&top, "", None, 0, None, 0);
    // Decided once every root has been visited: a branch cut deep down may be reached shallower
    // from a later root, and is then read in full.
    let incomplete = w.cut.iter().any(|(r, reach)| w.seen.get(r) != Some(reach));
    (!incomplete).then_some(w.out)
}

/// The `Want` entries needed to restore password fields in `doc`'s datasets packet to their
/// defaults (or empty when they have no default).
///
/// The packet is read as the datasets writer reads it ([`parse_datasets_packet`]): a packet the
/// writer can't read holds no value it wrote, so there is nothing typed in it to clear. An error
/// when the password fields can't all be found (a field tree deeper than the field walk reaches):
/// the save is refused rather than written with a typed password left in the data.
fn password_dataset_wants(doc: &Document) -> Result<Vec<Want>, XfaError> {
    if datasets_stream_ref(doc).is_none() {
        return Ok(Vec::new());
    }
    // Checked before the packet is read: a packet that can't be parsed leaves nothing to clear, but
    // a form whose fields can't all be found is refused whatever the packet holds.
    let Some(acro_fields) = acro_password_fields(doc) else {
        return Err(XfaError::Malformed(
            "the form's fields nest too deeply to find every password field, so typed passwords could not be removed from its XFA data".into(),
        ));
    };
    let Ok(Some(packet)) = datasets_packet(doc) else { return Ok(Vec::new()) };
    let Some((data, data_complete)) = parse_datasets_packet(&packet) else { return Ok(Vec::new()) };
    if !data_complete {
        return Err(XfaError::Malformed("the XFA password data scan was incomplete".into()));
    }
    let (wants, scan_complete) = with_template(doc, None, |tpl| collect_password_wants(Some(acro_fields), &data, tpl));
    if !scan_complete {
        return Err(XfaError::Malformed("the XFA password field scan was incomplete".into()));
    }
    Ok(wants)
}

/// Password values to clear in an already parsed data tree. Export passes the exact tree
/// it will emit rather than treating an unreadable document packet as an empty scan.
pub(super) fn password_data_wants(doc: &Document, data: &DataNode, tpl: Option<&Template>) -> (Vec<Want>, bool) {
    collect_password_wants(acro_password_fields(doc), data, tpl)
}

/// Shared scan over template `<passwordEdit/>` fields and pre-scanned AcroForm password fields.
fn collect_password_wants(acro_fields: Option<Vec<(String, String)>>, data: &DataNode, tpl: Option<&Template>) -> (Vec<Want>, bool) {
    let mut complete = true;
    let mut candidates: Vec<(String, DataPath, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(t) = tpl {
        struct Walk<'a> {
            tpl: &'a Template,
            data: &'a DataNode,
            nodes: usize,
            seen: &'a mut std::collections::HashSet<DataPath>,
            out: &'a mut Vec<(String, DataPath, String)>,
            complete: &'a mut bool,
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
                if children.is_empty() {
                    return;
                }
                if depth > MAX_DEPTH || self.nodes >= crate::script::MAX_FORM_NODES || !password_som_is_complete(parent_som) {
                    *self.complete = false;
                    return;
                }
                let sibs = SomIndices::of(children);
                for (i, n) in children.iter().enumerate() {
                    if self.nodes >= crate::script::MAX_FORM_NODES {
                        *self.complete = false;
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
                                        *self.complete = false;
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
                            if !password_som_is_complete(&som) {
                                *self.complete = false;
                                continue;
                            }
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
        let mut w = Walk { tpl: t, data, nodes: 0, seen: &mut seen, out: &mut candidates, complete: &mut complete };
        w.walk(std::slice::from_ref(&root), "", 0);
        if let Some(ps) = &t.root.page_set {
            for (i, area) in ps.areas.iter().enumerate() {
                let som = area.name.clone().map(|n| format!("{n}[0]")).unwrap_or_else(|| format!("#pageArea[{i}]"));
                w.walk(&area.items, &som, 1);
            }
        }
    }
    match acro_fields {
        Some(acro) => {
            for (som, default) in acro {
                if !password_som_is_complete(&som) {
                    complete = false;
                    continue;
                }
                let path = path_of(tpl, &som);
                if !path.is_empty() && seen.insert(path.clone()) {
                    candidates.push((som, path, default));
                }
            }
        }
        None => complete = false,
    }
    let wants = candidates
        .into_iter()
        .filter_map(|(som, path, default)| {
            let node = data.get(&path)?;
            // A structured password node is not a scalar the writer can safely clear.
            // Include it even when value_at would hide its contents as empty text.
            (!node.children.is_empty() || (!node.text.is_empty() && node.text != default)).then_some(Want { som, path, text: default, ensure: false })
        })
        .collect();
    (wants, complete)
}

/// The general SOM reader clamps large indices; a security scan must not alias another node.
fn password_som_is_complete(som: &str) -> bool {
    som.split('.').all(|segment| match segment.split_once('[') {
        Some((_, index)) => index.trim_end_matches(']').parse::<usize>().is_ok_and(|index| index <= MAX_SOM_INDEX),
        None => true,
    })
}

/// Whether any XFA password field (`<passwordEdit/>` or an AcroForm text field with the Password
/// flag) holds a value in `doc`'s datasets packet beyond its default (#3944). A form whose
/// password fields can't all be found counts as holding some, so that clearing them
/// ([`clear_password_values`]) reports why the document can't be written without them.
pub fn has_password_values(doc: &Document) -> bool {
    !password_dataset_wants(doc).is_ok_and(|w| w.is_empty())
}

/// Clear typed password values from `doc`'s XFA datasets packet (restoring a template default
/// where the field has one, else emptying the element), replacing the datasets stream in place
/// (#3944). Returns how many password fields were cleared in the datasets.
pub fn clear_password_values(doc: &mut Document) -> Result<usize, XfaError> {
    let wants = password_dataset_wants(doc)?;
    if wants.is_empty() {
        return Ok(0);
    }
    let count = wants.len();
    let reuse = datasets_stream_ref(doc);
    let report = rewrite(doc, &[Op::Values(&wants)], reuse)?;
    if !report.written || !password_dataset_wants(doc)?.is_empty() {
        let reason = report.warnings.first().map_or("the XFA datasets password values could not be cleared", String::as_str);
        return Err(XfaError::Malformed(reason.to_string()));
    }
    Ok(count)
}
