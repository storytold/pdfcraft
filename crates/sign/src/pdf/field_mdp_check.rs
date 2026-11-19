//! Resolve the effective XFA field scope of a PDF signature for editing and validation. XFA
//! datasets can change without changing the PDF field dictionaries and widgets compared by `locks`.

use std::collections::{HashMap, HashSet};

use pdfcraft_xfa::data::DataNode;
use pdfcraft_xfa::model::Template;
use pdfcraft_xfa::signature::{ManifestAction, canonical_som};
use pdfcraft_xfa::{XfaError, signature};

use super::listing::Field;
use super::{Dict, Document, FieldMdpAction, FieldMdpPolicy};

/// `/Lock` and FieldMDP policies prepared for matching PDF names and their XFA equivalents.
#[derive(Default)]
pub struct FieldMdpRules(Vec<FieldMdpRule>);

struct FieldMdpRule {
    action: FieldMdpAction,
    names: HashSet<String>,
    soms: HashSet<String>,
}

impl FieldMdpRules {
    pub fn new(policies: Vec<FieldMdpPolicy>) -> Self {
        Self(
            policies
                .into_iter()
                .map(|policy| FieldMdpRule {
                    action: policy.action,
                    soms: policy.fields.iter().map(|name| canonical_som(name)).collect(),
                    names: policy.fields.into_iter().collect(),
                })
                .collect(),
        )
    }

    /// Whether any policy explicitly locks all fields.
    pub fn locks_all(&self) -> bool {
        self.0.iter().any(|rule| rule.action == FieldMdpAction::All)
    }

    /// Whether any policy protects the field, identified by its PDF name and optional SOM path.
    pub fn locks(&self, name: &str, som: Option<&str>) -> bool {
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
            let listed = selects(&rule.names, name) || som.is_some_and(|som| selects(&rule.soms, &canonical_som(som)));
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
    if !xdp.contains("<signature") && !xdp.contains(":signature") {
        return Ok(None);
    }
    let manifests = signature::signature_manifests(xdp)?;
    let names = pdfcraft_xfa::script::fields_by_som(revision);
    let canonical_field = canonical_som(signature_field);
    let mut matches = manifests.into_iter().filter(|manifest| {
        names.get(&manifest.signature_field).is_some_and(|name| name == signature_field)
            || canonical_som(&manifest.signature_field) == canonical_field
    });
    let Some(manifest) = matches.next() else { return Ok(None) };
    if matches.next().is_some() {
        return Err(XfaError::Malformed(format!("several XFA widgets match signed field {signature_field:?}")));
    }
    let (template, _) = pdfcraft_xfa::parse(xdp)?;
    let original_data = pdfcraft_xfa::parse_datasets(xdp);
    let mut candidates = signature::field_soms(&template, original_data.as_ref())?;
    let mut seen: HashSet<_> = candidates.iter().cloned().collect();
    candidates.extend(names.keys().filter(|som| seen.insert((**som).clone())).cloned());
    let listed: HashSet<_> = signature::referenced_field_soms(&manifest, &candidates)?.into_iter().collect();
    // Signed PDF policies carry an explicit manifest's scope when present. Without an
    // explicit manifest, the XFA default still applies to the instantiated form.
    let use_manifest = rules.0.is_empty() || !manifest.explicit;
    let fields = candidates
        .into_iter()
        .map(|som| {
            let protected = if use_manifest {
                match manifest.action {
                    ManifestAction::All => true,
                    ManifestAction::Include => listed.contains(&som),
                    ManifestAction::Exclude => !listed.contains(&som),
                }
            } else {
                rules.locks(names.get(&som).unwrap_or(&som), Some(&som))
            };
            (som, protected)
        })
        .collect();
    let manifest_locks_all = use_manifest && manifest.action == ManifestAction::All;
    Ok(Some(SignedXfaScope { template, manifest_locks_all, lock_document: manifest.lock_document == Some(true), original_data, fields, names }))
}

/// `Err` means the signed scope or current XFA data could not be read. The caller leaves the
/// signature's lock verdict unchecked instead of treating an unreadable scope as unlocked.
pub(super) fn xfa_fields_changed(old: &Document, now: &Document, field: &Field, value: &Dict) -> Result<bool, ()> {
    let Some(packets) = pdfcraft_xfa::read_packets(old).map_err(|_| ())? else { return Ok(false) };
    let rules = FieldMdpRules::new(super::locks::policies(old, field, value)?);
    let Some(scope) = signed_xfa_scope(old, &packets.xdp, &field.name, &rules).map_err(|_| ())? else { return Ok(false) };
    let mut protected = scope.fields.iter().filter_map(|(som, protected)| protected.then_some(som.as_str())).peekable();
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
