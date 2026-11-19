//! Effective field scopes of PDF signature widgets in XFA templates.

use std::collections::HashMap;

use crate::XfaError;
use crate::data::DataNode;
use crate::model::Template;
use crate::script::{NodeKind, Overrides, form_tree};
use crate::xml::{Document, Node, ParsingOptions};

/// How a signature manifest selects its fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestAction {
    Include,
    Exclude,
    All,
}

/// One XFA signature widget's effective field scope, keyed by its generated SOM path.
/// A PDF signature widget without a manifest covers all fields by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureManifest {
    pub signature_field: String,
    pub action: ManifestAction,
    pub refs: Vec<String>,
    /// A PDF1.6 MDP+ signature has an explicit manifest; PDF1.3 without one covers the form.
    pub explicit: bool,
    /// Certification permissions requested by `<filter><mdp>` for an author signature.
    pub certify: Option<u8>,
    /// The independent document-wide lock seed value, if required by the template.
    pub lock_document: Option<bool>,
}

const MAX_MANIFESTS: usize = 256;
const MAX_REFS: usize = 20_000;
const MAX_SOM_BYTES: usize = 4_096;
const MAX_DEPTH: usize = 64;
const MAX_MATCH_WORK: usize = 8_000_000;
const MAX_MATCH_BYTES: usize = 4 << 20;

fn child<'a>(node: Node<'a>, name: &str) -> Option<Node<'a>> {
    node.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn ref_text(node: Node<'_>) -> Option<String> {
    let mut value = String::new();
    for text in node.children().filter_map(|c| c.text()) {
        if value.len().saturating_add(text.len()) > MAX_SOM_BYTES {
            return None;
        }
        value.push_str(text);
    }
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The SOM path used by generated PdfCraft fields. XFA references may start at the XDP/template
/// root and pass through unnamed class nodes; those nodes do not appear in `/PCSom`.
pub fn canonical_som(path: &str) -> String {
    let path = path.trim();
    let path = path.strip_prefix("xfa[0].template[0].").or_else(|| path.strip_prefix("$template.")).unwrap_or(path);
    let mut result = String::new();
    let mut dots = 0usize;
    for step in path.split('.') {
        if step.is_empty() {
            dots += 1;
            continue;
        }
        let class = step.split_once('[').map_or(step, |(name, _)| name);
        if matches!(class, "#subform" | "#subformSet" | "#area" | "#pageSet" | "#pageArea") {
            continue;
        }
        if dots > 0 {
            result.push_str(if dots > 1 { ".." } else { "." });
        }
        result.push_str(step);
        dots = 1;
    }
    result
}

fn manifest_of(field: Node<'_>, som: &str) -> Result<Option<SignatureManifest>, XfaError> {
    let Some(signature) = child(field, "ui").and_then(|ui| child(ui, "signature")) else { return Ok(None) };
    let pdf16 = match signature.attribute("type").unwrap_or("PDF1.3") {
        "PDF1.3" => false,
        "PDF1.6" => true,
        other => return Err(XfaError::Malformed(format!("unsupported PDF signature type {other:?} at {som}"))),
    };
    let filter = child(signature, "filter");
    if !pdf16 && (filter.is_some() || child(signature, "manifest").is_some()) {
        return Err(XfaError::Malformed(format!("a PDF1.3 signature cannot carry a manifest or filter at {som}")));
    }
    let mdp = filter.and_then(|filter| child(filter, "mdp"));
    let certify = match mdp.and_then(|mdp| mdp.attribute("signatureType")) {
        Some("author") => {
            let permission = mdp.and_then(|mdp| mdp.attribute("permissions")).unwrap_or("2");
            Some(
                permission
                    .parse::<u8>()
                    .ok()
                    .filter(|p| (1..=3).contains(p))
                    .ok_or_else(|| XfaError::Malformed(format!("invalid MDP permissions at {som}")))?,
            )
        }
        Some("filler") | None => None,
        Some(other) => return Err(XfaError::Malformed(format!("invalid MDP signatureType {other:?} at {som}"))),
    };
    let lock_document = match filter.and_then(|filter| child(filter, "lockDocument")) {
        Some(lock) if lock.attribute("type") == Some("required") => match lock.text().map(str::trim) {
            Some("0") => Some(true),
            Some("1") => Some(false),
            Some("auto" | "") | None => None,
            _ => return Err(XfaError::Malformed(format!("invalid required lockDocument value at {som}"))),
        },
        _ => None,
    };
    let Some(manifest) = child(signature, "manifest") else {
        return Ok(Some(SignatureManifest {
            signature_field: som.to_string(),
            action: ManifestAction::All,
            refs: Vec::new(),
            explicit: false,
            certify,
            lock_document,
        }));
    };
    let action = match manifest.attribute("action").unwrap_or("include") {
        "include" => ManifestAction::Include,
        "exclude" => ManifestAction::Exclude,
        "all" => ManifestAction::All,
        other => return Err(XfaError::Malformed(format!("invalid manifest action {other:?} at {som}"))),
    };
    let mut refs = Vec::new();
    for entry in manifest.children().filter(|c| c.is_element() && c.tag_name().name() == "ref") {
        if refs.len() == MAX_REFS {
            return Err(XfaError::TooLarge(format!("the manifest at {som} has more than {MAX_REFS} references")));
        }
        refs.push(ref_text(entry).ok_or_else(|| XfaError::Malformed(format!("an empty or oversized manifest reference at {som}")))?);
    }
    Ok(Some(SignatureManifest { signature_field: som.to_string(), action, refs, explicit: true, certify, lock_document }))
}

fn walk(node: Node<'_>, prefix: &str, depth: usize, out: &mut Vec<SignatureManifest>) -> Result<(), XfaError> {
    if depth > MAX_DEPTH {
        return Err(XfaError::TooLarge("the signature manifest tree exceeds its limit".into()));
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for c in node.children().filter(Node::is_element) {
        let tag = c.tag_name().name();
        if tag == "proto" || tag == "variables" {
            continue;
        }
        let container = matches!(tag, "subform" | "subformSet" | "area" | "exclGroup" | "field" | "draw" | "pageArea" | "pageSet");
        if !container {
            continue;
        }
        let name = c.attribute("name");
        let index = name.map_or(0, |name| {
            let n = seen.entry(name).or_insert(0);
            let i = *n;
            *n += 1;
            i
        });
        let som = match name {
            Some(name) if !prefix.is_empty() => format!("{prefix}.{name}[{index}]"),
            Some(name) => format!("{name}[{index}]"),
            None => prefix.to_string(),
        };
        if tag == "field" {
            let som = if name.is_some() {
                som
            } else if prefix.is_empty() {
                "field[0]".into()
            } else {
                format!("{prefix}.field[0]")
            };
            if let Some(manifest) = manifest_of(c, &som)? {
                if out.len() >= MAX_MANIFESTS {
                    return Err(XfaError::TooLarge("the signature manifest tree exceeds its limit".into()));
                }
                out.push(manifest);
            }
        } else {
            walk(c, &som, depth + 1, out)?;
        }
    }
    Ok(())
}

/// Parse the effective scopes of PDF signature widgets from an XDP or template packet.
/// A widget without a manifest covers all fields. Prototypes are expanded first, so
/// `<manifest use="#id">` inherits the refs of a manifest under `<variables>`.
pub fn signature_manifests(xml: &str) -> Result<Vec<SignatureManifest>, XfaError> {
    if xml.len() > crate::packets::MAX_TEMPLATE_TEXT {
        return Err(XfaError::TooLarge("the XFA template is too large to read its signature manifests".into()));
    }
    if crate::xml_nesting_depth(xml) > crate::MAX_XML_DEPTH {
        return Err(XfaError::Malformed("the XFA template is too deeply nested to read its signature manifests".into()));
    }
    let opts = || ParsingOptions { allow_dtd: false, nodes_limit: 2_000_000 };
    let parsed = Document::parse_with_options(xml, opts()).map_err(XfaError::Malformed)?;
    let root = parsed.root_element();
    let is_template = |node: &Node<'_>| {
        node.is_element()
            && node.tag_name().name() == "template"
            && (node.tag_name().namespace().is_some_and(|ns| ns.contains("xfa-template")) || child(*node, "subform").is_some())
    };
    let template = if is_template(&root) { root } else { root.children().find(is_template).ok_or(XfaError::NotXfa)? };
    let (expanded, warnings) = crate::proto::expand(template);
    if !warnings.is_empty()
        && template.descendants().any(|node| {
            node.is_element()
                && matches!(node.tag_name().name(), "signature" | "manifest" | "mdp" | "filter")
                && (node.attribute("use").is_some() || node.attribute("usehref").is_some())
        })
    {
        return Err(XfaError::Malformed(format!("signature prototype could not be resolved: {}", warnings.join("; "))));
    }
    let expanded_doc;
    let template = match expanded.as_deref().map(|s| Document::parse_with_options(s, opts())) {
        Some(Ok(doc)) => {
            expanded_doc = doc;
            expanded_doc.root_element()
        }
        Some(Err(e)) => return Err(XfaError::Malformed(format!("expanded signature template: {e}"))),
        None => template,
    };
    let mut out = Vec::new();
    walk(template, "", 0, &mut out)?;
    Ok(out)
}

/// Every instantiated XFA field, including fields without a visible PDF widget. The live form
/// already applies repeat counts and data binding; a truncated walk cannot define a safe scope.
pub fn field_soms(template: &Template, data: Option<&DataNode>) -> Result<Vec<String>, XfaError> {
    let live = form_tree(template, data, &Overrides::default());
    if live.truncated {
        return Err(XfaError::TooLarge("the live XFA form is too large to resolve signature fields".into()));
    }
    let mut out = Vec::new();
    let mut pending = vec![&live.root];
    while let Some(node) = pending.pop() {
        if node.kind == Some(NodeKind::Field) {
            out.push(node.som.clone());
        }
        pending.extend(node.children.iter().rev());
    }
    Ok(out)
}

fn step_matches(pattern: &str, value: &str) -> bool {
    let (name, index) = pattern.split_once('[').map_or((pattern, None), |(name, rest)| (name, rest.strip_suffix(']')));
    let (actual, actual_index) = value.split_once('[').map_or((value, None), |(name, rest)| (name, rest.strip_suffix(']')));
    (name == "*" || name == actual) && index.is_none_or(|index| index == "*" || Some(index) == actual_index)
}

fn supported_pattern(path: &str) -> bool {
    let path = path.strip_prefix("..").unwrap_or(path);
    if path.is_empty() {
        return false;
    }
    for step in path.split('.') {
        if step.is_empty() {
            continue; // the separator is a descendant search (`..`)
        }
        let (name, index) = match step.split_once('[') {
            Some((name, rest)) => {
                let Some(index) = rest.strip_suffix(']') else { return false };
                if index != "*" && (index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit())) {
                    return false;
                }
                (name, Some(index))
            }
            None => (step, None),
        };
        if name.is_empty()
            || name.chars().any(|c| matches!(c, '$' | '#' | '/' | '@' | '(' | ')' | '[' | ']'))
            || index.is_none() && step.contains(']')
        {
            return false;
        }
    }
    !path.starts_with('.') && !path.ends_with('.') && !path.contains("...")
}

/// Match a reference's steps against one field path. A double dot permits descendant search;
/// a single dot requires the next direct child. The field itself must be the final matched node.
fn matches_steps(steps: &[(bool, &str)], path: &[&str], work: &mut usize) -> Result<bool, XfaError> {
    if steps.len() > path.len() {
        return Ok(false);
    }
    // Each step visits the path once. Tracking the reachable prefixes avoids backtracking
    // through every combination when the reference contains several descendant searches.
    *work = work.saturating_add(steps.len().saturating_mul(path.len()));
    if *work > MAX_MATCH_WORK {
        return Err(XfaError::TooLarge("the signature manifest needs too many field comparisons".into()));
    }
    let mut reachable = vec![false; path.len() + 1];
    reachable[0] = true;
    let mut next = vec![false; reachable.len()];
    for &(descendant, step) in steps {
        let mut earlier = false;
        for ((&previous, part), matched) in reachable.iter().zip(path).zip(next.iter_mut().skip(1)) {
            earlier = previous || (descendant && earlier);
            *matched = earlier && step_matches(step, part);
        }
        std::mem::swap(&mut reachable, &mut next);
        if let Some(first) = reachable.first_mut() {
            *first = false;
        }
    }
    Ok(reachable.last().copied().unwrap_or(false))
}

/// Expand the manifest's references to actual field SOM paths, retaining duplicate matches as
/// the XFA node collection does. Only field nodes supplied by `candidates` can be returned.
pub fn referenced_field_soms(manifest: &SignatureManifest, candidates: &[String]) -> Result<Vec<String>, XfaError> {
    if manifest.action == ManifestAction::All {
        return Ok(Vec::new());
    }
    let owner = canonical_som(&manifest.signature_field);
    let parent: Vec<&str> = owner.rsplit_once('.').map(|(path, _)| path.split('.').collect()).unwrap_or_default();
    let mut out = Vec::new();
    let mut work = 0usize;
    let mut matched_bytes = 0usize;
    for reference in &manifest.refs {
        let raw = reference.trim();
        let absolute = raw.starts_with("xfa[0].template[0].") || raw.starts_with("$template.") || raw.starts_with("$form.");
        let normalized = if let Some(rest) = raw.strip_prefix("$form.") { rest.to_string() } else { canonical_som(raw) };
        if normalized.len() > MAX_SOM_BYTES || !supported_pattern(&normalized) {
            return Err(XfaError::Malformed(format!("unsupported signature manifest reference {reference:?}")));
        }
        let mut steps = Vec::new();
        let mut descendant = false;
        for step in normalized.split('.') {
            if step.is_empty() {
                descendant = true;
            } else {
                steps.push((descendant, step));
                descendant = false;
            }
        }
        if steps.len() > MAX_DEPTH {
            return Err(XfaError::TooLarge("the signature manifest reference is too deeply nested".into()));
        }
        let depths: Vec<usize> = if absolute { vec![0] } else { (0..=parent.len()).rev().collect() };
        let previous_matches = out.len();
        for depth in depths {
            for candidate in candidates {
                work += 1;
                if work > MAX_MATCH_WORK {
                    return Err(XfaError::TooLarge("the signature manifest needs too many field comparisons".into()));
                }
                if candidate.len() > MAX_SOM_BYTES {
                    return Err(XfaError::TooLarge("a field path is too long to resolve the signature manifest".into()));
                }
                let parts: Vec<&str> = candidate.split('.').collect();
                if let Some(suffix) = parts.get(depth..)
                    && parts.iter().take(depth).copied().eq(parent.iter().take(depth).copied())
                    && matches_steps(&steps, suffix, &mut work)?
                {
                    matched_bytes = matched_bytes.saturating_add(candidate.len().saturating_add(std::mem::size_of::<String>()));
                    if matched_bytes > MAX_MATCH_BYTES {
                        return Err(XfaError::TooLarge("the signature manifest selects too many field names".into()));
                    }
                    out.push(candidate.clone());
                }
            }
            if out.len() > previous_matches {
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_inherits_refs_from_variables() {
        let xml = r##"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
 <subform name="form"><subform name="body">
  <field name="amount"><ui><numericEdit/></ui></field>
  <field name="sign"><ui><signature type="PDF1.6"><manifest action="include" use="#locked"/></signature></ui></field>
 </subform><variables><manifest id="locked"><ref>xfa[0].template[0].form[0].#subform[0].body[0].amount[0]</ref></manifest></variables></subform>
</template></xdp:xdp>"##;
        let manifests = signature_manifests(xml).unwrap();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].signature_field, "form[0].body[0].sign[0]");
        assert_eq!(manifests[0].action, ManifestAction::Include);
        assert_eq!(canonical_som(&manifests[0].refs[0]), "form[0].body[0].amount[0]");
    }

    #[test]
    fn a_manifest_requires_pdf16() {
        let xml = r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form">
            <field name="sign"><ui><signature><manifest action="all"/></signature></ui></field>
            </subform></template>"#;
        assert!(signature_manifests(xml).is_err());
    }

    #[test]
    fn relative_wildcards_and_descendants_keep_duplicate_references() {
        let manifest = SignatureManifest {
            signature_field: "form[0].body[0].sign[0]".into(),
            action: ManifestAction::Include,
            refs: vec!["rows[*]..amount[*]".into(), "rows[*]..amount[*]".into()],
            explicit: true,
            certify: None,
            lock_document: None,
        };
        let candidates = [
            "form[0].body[0].rows[0].amount[0]".to_string(),
            "form[0].body[0].rows[1].nested[0].amount[0]".to_string(),
            "form[0].elsewhere[0].rows[0].amount[0]".to_string(),
        ];
        let selected = referenced_field_soms(&manifest, &candidates).unwrap();
        assert_eq!(selected, [candidates[0].clone(), candidates[1].clone(), candidates[0].clone(), candidates[1].clone()]);
    }

    #[test]
    fn malformed_som_index_is_not_silently_treated_as_a_wildcard() {
        let manifest = SignatureManifest {
            signature_field: "form[0].sign[0]".into(),
            action: ManifestAction::Include,
            refs: vec!["form[0].amount[broken".into()],
            explicit: true,
            certify: None,
            lock_document: None,
        };
        assert!(referenced_field_soms(&manifest, &["form[0].amount[0]".into()]).is_err());
    }

    #[test]
    fn hidden_fields_are_manifest_candidates() {
        let xml = r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form">
            <field name="visible"><ui><textEdit/></ui></field>
            <field name="hidden" presence="hidden"><ui><textEdit/></ui></field>
            </subform></template>"#;
        let (template, _) = crate::parse(xml).unwrap();
        let fields = field_soms(&template, None).unwrap();
        assert!(fields.contains(&"form[0].hidden[0]".to_string()));
    }

    #[test]
    fn author_mdp_and_required_document_lock_are_separate_from_manifest() {
        let xml = r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form">
            <field name="sign"><ui><signature type="PDF1.6"><filter><mdp signatureType="author" permissions="2"/>
            <lockDocument type="required">0</lockDocument></filter><manifest action="include"><ref>form[0].sign[0]</ref></manifest>
            </signature></ui></field></subform></template>"#;
        let manifest = signature_manifests(xml).unwrap().remove(0);
        assert_eq!(manifest.certify, Some(2));
        assert_eq!(manifest.lock_document, Some(true));
        assert_eq!(manifest.action, ManifestAction::Include);
    }
}
