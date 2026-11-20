//! Resolve the effective XFA field scope of a PDF signature for editing and validation. XFA
//! datasets can change without changing the PDF field dictionaries and widgets compared by `locks`.

use std::collections::{HashMap, HashSet};

use pdfcraft_xfa::data::DataNode;
use pdfcraft_xfa::model::Template;
use pdfcraft_xfa::signature::ManifestAction;
use pdfcraft_xfa::{XfaError, signature};

use super::listing::Field;
use super::{Dict, Document, FieldMdpAction, FieldMdpPolicy};

/// `/Lock` and FieldMDP policies prepared for matching PDF names and their XFA equivalents.
#[derive(Default)]
pub struct FieldMdpRules(Vec<FieldMdpRule>);

struct FieldMdpRule {
    action: FieldMdpAction,
    names: HashSet<String>,
}

impl FieldMdpRules {
    pub fn new(policies: Vec<FieldMdpPolicy>) -> Self {
        Self(policies.into_iter().map(|policy| FieldMdpRule { action: policy.action, names: policy.fields.into_iter().collect() }).collect())
    }

    /// Whether any policy explicitly locks all fields.
    pub fn locks_all(&self) -> bool {
        self.0.iter().any(|rule| rule.action == FieldMdpAction::All)
    }

    /// Whether any policy protects the field, identified by its PDF name and optional SOM path.
    pub fn locks(&self, name: &str, som: Option<&str>) -> bool {
        let som = som.filter(|path| {
            path.split('.').all(|step| {
                step.rsplit_once('[').is_some_and(|(name, index)| {
                    !name.is_empty()
                        && index.strip_suffix(']').is_some_and(|index| !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()))
                })
            })
        });
        // Compare complete path components so `address` selects `address.city` but not `addresses.city`.
        let selects = |names: &HashSet<String>, mut path: &str| {
            loop {
                if names.contains(path) {
                    return true;
                }
                let Some((parent, _)) = path.rsplit_once('.') else { return false };
                path = parent;
            }
        };
        self.0.iter().any(|rule| {
            // Signing emits the full SOM for a field without a PDF widget. Compare that
            // path as written too, including any exact dotted parent.
            let listed = selects(&rule.names, name) || som.is_some_and(|som| selects(&rule.names, som));
            match rule.action {
                FieldMdpAction::All => true,
                FieldMdpAction::Include => listed,
                FieldMdpAction::Exclude => !listed,
            }
        })
    }
}

/// Field selection from one signed XFA revision, before any session-only restrictions.
pub struct SignedXfaScope {
    pub template: Template,
    /// The effective manifest scope (including the default) locks all XFA fields.
    pub manifest_locks_all: bool,
    /// The required document lock is independent of the manifest field scope.
    pub lock_document: bool,
    original_data: Option<DataNode>,
    fields: Vec<(String, bool)>,
    names: HashMap<String, String>,
}

impl SignedXfaScope {
    /// Each signed-revision candidate's SOM path, PDF name (or SOM if absent) and whether it is protected.
    pub fn fields(&self) -> impl Iterator<Item = (&str, &str, bool)> {
        self.fields.iter().map(move |(som, protected)| (som.as_str(), self.names.get(som).unwrap_or(som).as_str(), *protected))
    }
}

/// Find the signature's XFA widget and resolve its scope against the signed revision's XDP.
pub fn signed_xfa_scope(revision: &Document, xdp: &str, signature_field: &str, rules: &FieldMdpRules) -> Result<Option<SignedXfaScope>, XfaError> {
    if rules.0.is_empty() && !xdp.contains("<signature") && !xdp.contains(":signature") {
        return Ok(None);
    }
    let (template, _) = pdfcraft_xfa::parse(xdp)?;
    let names = pdfcraft_xfa::script::fields_by_som(revision);
    let manifest = signature::signature_manifest_for_field(xdp, &template, &names, signature_field)?;
    if manifest.is_none() && rules.0.is_empty() {
        return Ok(None);
    }
    let original_data = pdfcraft_xfa::parse_datasets(xdp);
    let mut candidates = signature::field_soms(&template, original_data.as_ref())?;
    let mut seen: HashSet<_> = candidates.iter().cloned().collect();
    candidates.extend(names.keys().filter(|som| seen.insert((**som).clone())).cloned());
    let listed: HashSet<_> = match &manifest {
        Some(manifest) => signature::referenced_field_soms(manifest, &candidates)?.into_iter().collect(),
        None => HashSet::new(),
    };
    // Signed PDF policies carry an explicit manifest's scope when present. Without an
    // explicit manifest, the XFA default still applies to the instantiated form.
    let manifest_action = manifest.as_ref().filter(|m| rules.0.is_empty() || !m.explicit).map(|m| m.action);
    let fields = candidates
        .into_iter()
        .map(|som| {
            let protected = match manifest_action {
                Some(ManifestAction::All) => true,
                Some(ManifestAction::Include) => listed.contains(&som),
                Some(ManifestAction::Exclude) => !listed.contains(&som),
                None => rules.locks(names.get(&som).unwrap_or(&som), Some(&som)),
            };
            (som, protected)
        })
        .collect();
    let manifest_locks_all = manifest_action == Some(ManifestAction::All);
    let lock_document = manifest.is_some_and(|manifest| manifest.lock_document == Some(true));
    Ok(Some(SignedXfaScope { template, manifest_locks_all, lock_document, original_data, fields, names }))
}

/// `Err` means the signed scope or current XFA data could not be read. The caller leaves the
/// signature's lock verdict unchecked instead of treating an unreadable scope as unlocked.
pub(super) fn xfa_fields_changed(old: &Document, now: &Document, field: &Field, value: &Dict) -> Result<bool, ()> {
    let Some(packets) = pdfcraft_xfa::read_packets(old).map_err(|_| ())? else { return Ok(false) };
    let rules = FieldMdpRules::new(super::locks::policies(old, field, value)?);
    let Some(scope) = signed_xfa_scope(old, &packets.xdp, &field.name, &rules).map_err(|_| ())? else { return Ok(false) };
    let mut protected = scope.fields.iter().filter_map(|(som, protected)| (*protected || scope.lock_document).then_some(som.as_str())).peekable();
    if protected.peek().is_none() {
        return Ok(false);
    }
    let current_packets = pdfcraft_xfa::read_packets(now).map_err(|_| ())?.ok_or(())?;
    let current_data = pdfcraft_xfa::parse_datasets(&current_packets.xdp);
    for som in protected {
        let path = pdfcraft_xfa::data_path(&scope.template, som);
        let before = scope.original_data.as_ref().and_then(|data| data.value_at(&path));
        let after = current_data.as_ref().and_then(|data| data.value_at(&path));
        if before != after {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_mdp_preserves_pdf_names_with_trailing_spaces() {
        let rules = FieldMdpRules::new(vec![FieldMdpPolicy { action: FieldMdpAction::Exclude, fields: vec!["A ".into(), "form[0].B[0]".into()] }]);
        assert!(rules.locks("A", Some("A")));
        assert!(!rules.locks("A ", Some("A")));
        assert!(!rules.locks("B", Some("form[0].B[0]")));
        assert!(rules.locks("B", Some("form[0].B[1]")));
    }

    #[test]
    fn field_mdp_and_document_lock_detect_xfa_dataset_only_changes() {
        let xdp = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
            <template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form">
            <field name="A"><ui><textEdit/></ui></field><field name="B"><ui><textEdit/></ui></field>
            SIGNATURE
            </subform></template><xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/">
            <xfa:data><form><A>one</A><B>two</B></form></xfa:data></xfa:datasets></xdp:xdp>"#;
        let xfa_signature = r#"<field name="sign"><ui><signature type="PDF1.6"><filter><lockDocument type="required">0</lockDocument></filter>
            <manifest action="include"><ref>A</ref></manifest></signature></ui></field>"#;
        for (signature, pc_som, locks_b) in [("", "", false), (xfa_signature, "/PCSom (form[0].sign[0])", true)] {
            let bytes = pdfcraft_testkit::Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R 4 0 R 5 0 R] /XFA 6 0 R >> >>")
                .obj("<< /Type /Pages /Kids [] /Count 0 >>")
                .obj(format!("<< /FT /Sig /T (Approval) {pc_som} /Lock << /Action /Include /Fields [(A)] >> /V << >> >>"))
                .obj("<< /FT /Tx /T (A) /PCSom (form[0].A[0]) /V (one) >>")
                .obj("<< /FT /Tx /T (B) /PCSom (form[0].B[0]) /V (two) >>")
                .stream("", xdp.replace("SIGNATURE", signature))
                .build();
            let old = Document::open(std::sync::Arc::new(bytes)).unwrap();
            let fields = super::super::listing::sig_fields(&old);
            let field = &fields[0];
            let value = field.signature(&old).unwrap();
            for (name, expected) in [("A", true), ("B", locks_b)] {
                let mut now = old.clone();
                let path = pdfcraft_xfa::som_to_path(&format!("form[0].{name}[0]"));
                pdfcraft_xfa::data::write_data_value(&mut now, &path, "changed").unwrap();
                assert_eq!(xfa_fields_changed(&old, &now, field, &value), Ok(expected), "{signature}: {name}");
            }
        }
    }
}
