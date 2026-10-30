//! Which comment types a view hides.
//!
//! The Comments panel names a type (`Oval`, `Callout`, `Replace text`). `comment_list` reports
//! the PDF subtype (`Circle`, `FreeText`). Both are accepted and stored as the panel's name.
//! At most 64 names, each at most 64 bytes: a tool argument is untrusted.
//!
//! Drawing skips those types through hayro's `annotation_overrides` (shown = false), so the
//! shared craft-pdf interpreter needs no type-filter field of its own. Fields and links are
//! never listed.

use craft_pdf::syntax::Pdf;
use craft_pdf::syntax::object::dict::keys::{ANNOTS, IT, RT, SUBTYPE};
use craft_pdf::syntax::object::r#ref::MaybeRef;
use craft_pdf::syntax::object::{Array, Dict, Name, Object};

const MAX_TYPES: usize = 64;
const MAX_TYPE_BYTES: usize = 64;
/// Cap how many annotation objects a type filter may hide in one document (hostile `/Annots`).
const MAX_HIDDEN_ANNOTS: usize = 10_000;

/// One skip rule: PDF `/Subtype`, optional `/IT`, and optional `/RT /Group`.
#[derive(Clone, Debug)]
struct HideRule {
    subtype: String,
    intent: Option<String>,
    except_intents: Vec<String>,
    grouped: Option<bool>,
}

/// The panel's name for `name`, or the name itself when it is a custom subtype.
///
/// `Err` for an empty name, one longer than 64 bytes, or a field, link or pop-up: those are
/// not comments, and hiding them would hide the form or the links.
pub fn normalize_hidden_comment_types<S: AsRef<str>>(names: &[S]) -> Result<Vec<String>, &'static str> {
    if names.len() > MAX_TYPES {
        return Err("at most 64 comment types");
    }
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let label = canonical_label(name.as_ref())?;
        if !out.contains(&label) {
            out.push(label);
        }
    }
    out.sort();
    Ok(out)
}

/// Annotation object overrides that hide `names` on the page (`shown = false`). Empty when
/// there is nothing to hide. Existing script overrides for the same object are omitted so the
/// type filter wins.
pub(crate) fn hidden_type_overrides(pdf: &Pdf, names: &[String]) -> Vec<(i32, i32, bool)> {
    let rules = comment_hide_rules(names);
    if rules.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for page in pdf.pages().iter() {
        let Some(annots) = page.raw().get::<Array<'_>>(ANNOTS) else {
            continue;
        };
        let xref = page.xref();
        for item in annots.raw_iter() {
            if out.len() >= MAX_HIDDEN_ANNOTS {
                return out;
            }
            let (annot, obj) = match item {
                MaybeRef::Ref(r) => match xref.get::<Dict<'_>>(r.into()) {
                    Some(d) => (d, Some(r)),
                    None => continue,
                },
                MaybeRef::NotRef(Object::Dict(_)) => continue,
                MaybeRef::NotRef(_) => continue,
            };
            let Some(r) = obj else { continue };
            if !rule_matches(&annot, &rules) {
                continue;
            }
            out.push((r.obj_number, r.gen_number, false));
        }
    }
    out
}

fn comment_hide_rules(names: &[String]) -> Vec<HideRule> {
    let mut rules = Vec::new();
    for name in names.iter().take(MAX_TYPES) {
        let Ok(label) = canonical_label(name) else { continue };
        rules.extend(rules_for(&label));
    }
    rules
}

fn rule_matches(annot: &Dict<'_>, rules: &[HideRule]) -> bool {
    let subtype = annot.get::<Name<'_>>(SUBTYPE);
    if matches!(subtype.as_deref(), Some(b"Widget" | b"Link")) {
        return false;
    }
    let intent = annot.get::<Name<'_>>(IT);
    let grouped = annot.get::<Name<'_>>(RT).as_deref() == Some(b"Group");
    rules.iter().any(|rule| {
        subtype.as_deref() == Some(rule.subtype.as_bytes())
            && match &rule.intent {
                Some(it) => intent.as_deref() == Some(it.as_bytes()),
                None => !rule.except_intents.iter().any(|except| intent.as_deref() == Some(except.as_bytes())),
            }
            && rule.grouped.is_none_or(|want| want == grouped)
    })
}

fn canonical_label(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() || name.len() > MAX_TYPE_BYTES || name.chars().any(char::is_control) {
        return Err("a comment type must be 1–64 characters");
    }
    let folded = name.to_ascii_lowercase();
    let label = match folded.as_str() {
        "note" | "text" | "sticky note" => "Note",
        "text box" | "textbox" | "freetext" => "Text box",
        "callout" | "freetextcallout" => "Callout",
        "typewriter" | "freetexttypewriter" => "Typewriter",
        "strikethrough" | "strikeout" => "Strikethrough",
        "replace text" | "strikeouttextedit" => "Replace text",
        "rectangle" | "square" => "Rectangle",
        "oval" | "circle" => "Oval",
        "drawing" | "ink" | "pencil" => "Drawing",
        "polyline" | "polygonal line" => "Polyline",
        "attachment" | "fileattachment" | "file attachment" => "Attachment",
        "insert text" | "caret" | "inserted text" => "Insert text",
        "cloud" | "polygoncloud" => "Cloud",
        "polygon" => "Polygon",
        "highlight" => "Highlight",
        "underline" => "Underline",
        "squiggly" => "Squiggly",
        "line" => "Line",
        "stamp" => "Stamp",
        "redact" | "redaction" => "Redact",
        "sound" => "Sound",
        "widget" | "link" | "popup" => return Err("fields, links and pop-ups are not comment types"),
        _ => return Ok(name.to_string()),
    };
    Ok(label.to_string())
}

fn rules_for(label: &str) -> Vec<HideRule> {
    match label {
        "Note" => vec![rule("Text", None, &[], None)],
        "Text box" => vec![rule("FreeText", None, &["FreeTextCallout", "FreeTextTypeWriter"], None)],
        "Callout" => vec![rule("FreeText", Some("FreeTextCallout"), &[], None)],
        "Typewriter" => vec![rule("FreeText", Some("FreeTextTypeWriter"), &[], None)],
        "Strikethrough" => vec![rule("StrikeOut", None, &["StrikeOutTextEdit"], None)],
        "Replace text" => vec![rule("StrikeOut", Some("StrikeOutTextEdit"), &[], None), rule("Caret", None, &[], Some(true))],
        "Insert text" => vec![rule("Caret", None, &[], Some(false))],
        "Rectangle" => vec![rule("Square", None, &[], None)],
        "Oval" => vec![rule("Circle", None, &[], None)],
        "Drawing" => vec![rule("Ink", None, &[], None)],
        "Polyline" => vec![rule("PolyLine", None, &[], None)],
        "Attachment" => vec![rule("FileAttachment", None, &[], None)],
        "Cloud" => vec![rule("Polygon", Some("PolygonCloud"), &[], None)],
        "Polygon" => vec![rule("Polygon", None, &["PolygonCloud"], None)],
        other => vec![rule(other, None, &[], None)],
    }
}

fn rule(subtype: &str, intent: Option<&str>, except: &[&str], grouped: Option<bool>) -> HideRule {
    HideRule {
        subtype: subtype.to_string(),
        intent: intent.map(str::to_string),
        except_intents: except.iter().map(|s| (*s).to_string()).collect(),
        grouped,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_hidden_comment_types;

    #[test]
    fn aliases_collapse_to_the_panel_name() {
        let names = normalize_hidden_comment_types(&["circle", "Oval", "Square", "FreeTextCallout"]).expect("names");
        assert_eq!(names, vec!["Callout".to_string(), "Oval".to_string(), "Rectangle".to_string()]);
    }

    #[test]
    fn fields_links_and_empty_names_are_refused() {
        assert!(normalize_hidden_comment_types(&["Widget"]).is_err());
        assert!(normalize_hidden_comment_types(&["link"]).is_err());
        assert!(normalize_hidden_comment_types(&["Popup"]).is_err());
        assert!(normalize_hidden_comment_types(&[""]).is_err());
        assert!(normalize_hidden_comment_types(&[" "]).is_err());
        let long = "x".repeat(65);
        assert!(normalize_hidden_comment_types(&[long.as_str()]).is_err());
        let mut many = Vec::new();
        for i in 0..65 {
            many.push(format!("Custom{i}"));
        }
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(normalize_hidden_comment_types(&refs).is_err());
    }
}
