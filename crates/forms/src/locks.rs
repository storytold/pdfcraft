//! Fields a signature locks (ISO 32000-1 §12.7.4.5, Table 233 `/Lock`, and the FieldMDP
//! transform of §12.8.2.4): once a signature field is signed, changing a field its lock names
//! invalidates that signature. Acrobat marks those fields read-only when it signs; files from
//! other signers often don't, so filling checks the lock itself.

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document};

use crate::{Field, FieldKind, text_of};

/// What a lock covers (`/Action`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    All,
    Include,
    Exclude,
}

/// A lock dictionary (`/Lock`) or FieldMDP transform parameters: its action and field names.
fn rule(doc: &Document, d: &Dict) -> Option<(Action, Vec<String>)> {
    let action = match d.name(b"Action")? {
        b"All" => Action::All,
        b"Include" => Action::Include,
        b"Exclude" => Action::Exclude,
        _ => return None,
    };
    let names = d
        .get(b"Fields")
        .map(|f| doc.resolve(f))
        .and_then(|f| f.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|n| text_of(&doc.resolve(n)))
        .collect();
    Some((action, names))
}

/// The locks of signature field `sig`, if it is signed: its `/Lock`, and the FieldMDP transform
/// in its signature's `/Reference` (Acrobat writes both; other signers one of them).
fn rules_of(doc: &Document, sig: &Field) -> Vec<(Action, Vec<String>)> {
    let field = doc.get(sig.obj);
    let Some(d) = field.as_dict() else { return Vec::new() };
    let value = d.get(b"V").map(|v| doc.resolve(v));
    let Some(value) = value.as_deref().and_then(|v| v.as_dict()) else {
        return Vec::new(); // not signed: the lock takes effect only once it is
    };
    let mut rules: Vec<_> = d.get(b"Lock").map(|l| doc.resolve(l)).and_then(|l| l.as_dict().and_then(|l| rule(doc, l))).into_iter().collect();
    let refs = value.get(b"Reference").map(|r| doc.resolve(r));
    for r in refs.as_deref().and_then(|r| r.as_array()).into_iter().flatten() {
        let r = doc.resolve(r);
        let Some(rd) = r.as_dict() else { continue };
        if rd.name(b"TransformMethod") != Some(b"FieldMDP") {
            continue;
        }
        if let Some(params) = rd.get(b"TransformParams").map(|p| doc.resolve(p))
            && let Some(rule) = params.as_dict().and_then(|p| rule(doc, p))
        {
            rules.push(rule);
        }
    }
    rules
}

/// The fields of `all` that signed signature fields lock: locked field name → the name of the
/// signature field that locks it (the first one, in field order). A listed name also locks the
/// fields below it (`address` locks `address.city`).
pub fn signature_locks(doc: &Document, all: &[Field]) -> HashMap<String, String> {
    let mut locked = HashMap::new();
    for sig in all.iter().filter(|f| f.kind == FieldKind::Signature) {
        for (action, names) in rules_of(doc, sig) {
            let listed = |n: &str| names.iter().any(|x| n == x || n.strip_prefix(x.as_str()).is_some_and(|r| r.starts_with('.')));
            for f in all.iter().filter(|f| f.name != sig.name) {
                let hit = match action {
                    Action::All => true,
                    Action::Include => listed(&f.name),
                    Action::Exclude => !listed(&f.name),
                };
                if hit {
                    locked.entry(f.name.clone()).or_insert_with(|| sig.name.clone());
                }
            }
        }
    }
    locked
}
