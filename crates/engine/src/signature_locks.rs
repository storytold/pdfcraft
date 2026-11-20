//! Derived editing restrictions from signed `/Lock`, FieldMDP, and XFA manifests.
//!
//! These are session state, not PDF edits: generated XFA fields must not gain new `/Ff` bits
//! merely because a signature was read. The final edit check also catches calculations that
//! change a protected field through an otherwise editable input.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_forms::{Field, flags};
use pdfcraft_sign::pdf::{FieldMdpAction, FieldMdpPolicy, FieldMdpRules, SignatureInfo, signed_field_policies, signed_xfa_scope};
use pdfcraft_xfa::data::{DataPath, data_path, som_to_path};
use pdfcraft_xfa::model::Template;
use pdfcraft_xfa::signature::{ManifestAction, canonical_som};

use crate::Edit;

/// Resolve the selected XFA widget before signing. The PDF signer receives only PDF field names
/// and has no knowledge of XFA's template or SOM grammar.
pub(crate) struct SigningPolicy {
    pub field_mdp: Option<FieldMdpPolicy>,
    pub certify: Option<u8>,
}

pub(crate) fn signing_policy(doc: &Document, field_name: &str) -> Result<Option<SigningPolicy>, pdfcraft_xfa::XfaError> {
    let Some(packets) = pdfcraft_xfa::read_packets(doc)? else { return Ok(None) };
    if !packets.xdp.contains("<signature") && !packets.xdp.contains(":signature") {
        return Ok(None);
    }
    let (template, _) = pdfcraft_xfa::parse(&packets.xdp)?;
    let by_som = pdfcraft_xfa::script::fields_by_som(doc);
    let Some(manifest) = pdfcraft_xfa::signature::signature_manifest_for_field(&packets.xdp, &template, &by_som, field_name)? else {
        return Ok(None);
    };
    let field_mdp = if manifest.explicit && manifest.action == ManifestAction::All {
        Some(FieldMdpPolicy { action: FieldMdpAction::All, fields: Vec::new() })
    } else if manifest.explicit {
        let data = pdfcraft_xfa::parse_datasets(&packets.xdp);
        let mut candidates = pdfcraft_xfa::signature::field_soms(&template, data.as_ref())?;
        let mut seen: HashSet<_> = candidates.iter().cloned().collect();
        candidates.extend(by_som.keys().filter(|som| seen.insert((**som).clone())).cloned());
        let listed = pdfcraft_xfa::signature::referenced_field_soms(&manifest, &candidates)?;
        let mut fields: Vec<String> = listed.into_iter().map(|som| by_som.get(&som).cloned().unwrap_or(som)).collect();
        fields.sort();
        fields.dedup();
        let action = match manifest.action {
            ManifestAction::Include => FieldMdpAction::Include,
            ManifestAction::Exclude => FieldMdpAction::Exclude,
            ManifestAction::All => FieldMdpAction::All,
        };
        Some(FieldMdpPolicy { action, fields })
    } else {
        None
    };
    Ok(Some(SigningPolicy { field_mdp, certify: manifest.certify }))
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SignatureLocks {
    names: HashSet<String>,
    data_paths: Vec<(String, DataPath)>,
    data_path_names: HashSet<String>,
    /// Calculated fields whose signature lock suppresses the calculation's write.
    calculated_soms: HashMap<String, String>,
    warning: Option<String>,
    block_new_signatures: bool,
    unknown_scope: bool,
}

impl SignatureLocks {
    fn add(&mut self, name: &str, som: Option<&str>, template: Option<&Template>) {
        self.names.insert(name.to_string());
        if let Some(som) = som {
            if self.data_path_names.insert(name.to_string()) {
                let path = template.map_or_else(|| som_to_path(som), |tpl| data_path(tpl, som));
                self.data_paths.push((name.to_string(), path));
            }
            if template.is_some_and(|tpl| pdfcraft_xfa::field_is_calculated(tpl, som)) {
                self.calculated_soms.insert(canonical_som(som), name.to_string());
            }
        }
    }

    pub(crate) fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub(crate) fn is_locked(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    pub(crate) fn blocks_new_signature(&self) -> bool {
        self.block_new_signatures
    }

    /// Structural edits to a protected field are refused even when they leave `/V` unchanged.
    pub(crate) fn targeted<'a>(&self, edit: &'a Edit) -> Option<&'a str> {
        match edit {
            Edit::SetFieldValue { name, .. }
            | Edit::SetFieldProps { name, .. }
            | Edit::SetFieldImage { name, .. }
            | Edit::SetFieldImageTarget { name, .. }
            | Edit::DuplicateField { name, .. }
            | Edit::DeleteField { name }
            | Edit::DeleteWidget { name, .. }
            | Edit::SetFieldScript { name, .. }
            | Edit::MoveInTabOrder { name, .. } => self.names.contains(name).then_some(name.as_str()),
            Edit::Batch { edits, .. } => edits.iter().find_map(|part| self.targeted(part)),
            _ => None,
        }
    }

    pub(crate) fn read(doc: &Document, bytes: &[u8], form: &[Field], signatures: &[SignatureInfo], template: Option<&Template>) -> Self {
        let mut locks = Self::default();
        if !signatures.iter().any(|signature| signature.signed) {
            return locks;
        }
        let current_soms = pdfcraft_xfa::script::field_objects_by_som(doc);
        let by_obj: HashMap<_, _> = current_soms.iter().map(|(som, (_, obj))| (*obj, som.as_str())).collect();
        let mut signed_fields: Vec<_> = signatures.iter().filter(|signature| signature.signed && !signature.doc_timestamp).collect();
        signed_fields.sort_by_key(|signature| signature.signed_len);
        // Adjacent fields may share one signed revision. Release each PDF prefix before reading
        // the next so memory does not grow with the number of signatures in the file.
        let mut current_revision = None;
        let mut whole_document = false;
        for signature in signed_fields {
            let name = signature.field.as_str();
            let Some(signed_bytes) = bytes.get(..signature.signed_len).filter(|prefix| !prefix.is_empty()) else {
                locks.warning.get_or_insert_with(|| "a signed revision could not be read to determine its field locks".into());
                locks.unknown_scope = true;
                continue;
            };
            if current_revision.as_ref().is_none_or(|(len, _)| *len != signature.signed_len) {
                // Drop the previous decoded revision before allocating a new copy of its bytes.
                drop(current_revision.take());
                let revision = doc.open_with_same_credentials(Arc::new(signed_bytes.to_vec())).ok();
                current_revision = Some((signature.signed_len, revision));
            }
            let Some((_, Some(revision))) = &current_revision else {
                locks.warning.get_or_insert_with(|| "a signed revision could not be read to determine its field locks".into());
                locks.unknown_scope = true;
                continue;
            };
            let pdf_rules = match signed_field_policies(revision, name) {
                Ok(policies) => FieldMdpRules::new(policies),
                Err(error) => {
                    locks.warning.get_or_insert_with(|| format!("the signed field's lock scope could not be read: {error}"));
                    // Its scope is unknown, so no form field can be assumed safe to edit.
                    locks.unknown_scope = true;
                    FieldMdpRules::default()
                }
            };
            locks.block_new_signatures |= pdf_rules.locks_all();
            let packets = match pdfcraft_xfa::read_packets(revision) {
                Ok(packets) => packets,
                Err(error) => {
                    locks.warning.get_or_insert_with(|| format!("the signed XFA data could not be read: {error}"));
                    locks.unknown_scope = true;
                    None
                }
            };
            let xfa = packets.and_then(|packets| match signed_xfa_scope(revision, &packets.xdp, name, &pdf_rules) {
                Ok(scope) => scope,
                Err(error) => {
                    locks.warning.get_or_insert_with(|| format!("the signed XFA manifest could not be resolved: {error}"));
                    locks.unknown_scope = true;
                    None
                }
            });
            if let Some(scope) = &xfa {
                whole_document |= scope.lock_document;
                locks.block_new_signatures |= scope.manifest_locks_all || scope.lock_document;
                for (som, pdf_name, protected) in scope.fields() {
                    if protected || scope.lock_document {
                        locks.add(pdf_name, Some(som), Some(&scope.template));
                    }
                }
            }
            let field_template = xfa.as_ref().map(|scope| &scope.template).or(template);
            for field in form {
                let som = by_obj.get(&field.obj).copied();
                if pdf_rules.locks(&field.name, som) || field.name == name {
                    locks.add(&field.name, som, field_template);
                }
            }
        }
        locks.block_new_signatures |= locks.unknown_scope;
        if whole_document || locks.unknown_scope {
            for field in form {
                locks.add(&field.name, by_obj.get(&field.obj).copied(), template);
            }
        }
        let signed_names: HashSet<_> = signatures.iter().filter(|sig| sig.signed).map(|sig| sig.field.as_str()).collect();
        for field in form.iter().filter(|field| signed_names.contains(field.name.as_str())) {
            locks.add(&field.name, by_obj.get(&field.obj).copied(), template);
        }
        locks
    }

    pub(crate) fn apply(&self, form: &mut [Field]) {
        for field in form {
            if self.names.contains(&field.name) {
                field.flags |= flags::READ_ONLY;
                for widget in &mut field.widgets {
                    widget.read_only = true;
                }
            }
        }
    }

    /// A protected XFA field may still run its calculation, but the resulting write is ignored.
    pub(crate) fn calculated_name(&self, som: &str) -> Option<&str> {
        self.calculated_soms.get(&canonical_som(som)).map(String::as_str)
    }

    pub(crate) fn has_calculations(&self) -> bool {
        !self.calculated_soms.is_empty()
    }

    /// Name the first protected field whose value or widgets changed. Called after XFA calculations and
    /// dataset sync, while the candidate edit is still an isolated copy.
    pub(crate) fn changed(&self, before: &Document, after: &Document) -> Option<String> {
        if self.names.is_empty() && !self.unknown_scope {
            return None;
        }
        let fields = |doc: &Document| {
            pdfcraft_forms::fields(doc)
                .into_iter()
                .filter(|f| self.unknown_scope || self.names.contains(&f.name))
                .map(|f| (f.name.clone(), f))
                .collect::<HashMap<_, _>>()
        };
        let before_fields = fields(before);
        let after_fields = fields(after);
        if self.unknown_scope
            && let Some(name) = after_fields.keys().find(|name| !before_fields.contains_key(*name))
        {
            return Some(name.clone());
        }
        for name in &self.names {
            let changed = match (before_fields.get(name), after_fields.get(name)) {
                (Some(old), Some(new)) if self.unknown_scope => old != new,
                (Some(old), Some(new)) => {
                    old.value != new.value
                        || old.selected != new.selected
                        || old.widgets.iter().map(|widget| widget.rect).ne(new.widgets.iter().map(|widget| widget.rect))
                }
                (old, new) => old.is_some() != new.is_some(),
            };
            if changed {
                return Some(name.clone());
            }
        }
        if !self.data_paths.is_empty() {
            let before_data = pdfcraft_xfa::script::data_of(before);
            let after_data = pdfcraft_xfa::script::data_of(after);
            for (name, path) in &self.data_paths {
                let old = before_data.as_ref().and_then(|data| data.get(path));
                let new = after_data.as_ref().and_then(|data| data.get(path));
                if old != new {
                    return Some(name.clone());
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_edits_to_protected_fields_are_targeted() {
        let locks = SignatureLocks { names: HashSet::from(["C".into()]), ..SignatureLocks::default() };
        assert_eq!(locks.targeted(&Edit::DeleteField { name: "C".into() }), Some("C"));
        assert_eq!(locks.targeted(&Edit::DeleteField { name: "A".into() }), None);
    }

    #[test]
    fn field_mdp_parent_names_select_descendants_only() {
        let rules = FieldMdpRules::new(vec![FieldMdpPolicy { action: FieldMdpAction::Include, fields: vec!["address".into()] }]);
        assert!(rules.locks("address", None));
        assert!(rules.locks("address.city", None));
        assert!(!rules.locks("addresses.city", None));
    }
}
