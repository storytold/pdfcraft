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
use craft_pdf::syntax::object::{Array, Dict, MaybeRef, Name, Object};

const MAX_TYPES: usize = 64;
const MAX_TYPE_BYTES: usize = 64;
/// Cap how many annotation objects a type filter may hide on one page (hostile `/Annots`).
const MAX_HIDDEN_ANNOTS: usize = 10_000;

/// One skip rule: PDF `/Subtype`, optional `/IT`, and optional `/RT /Group`.
#[derive(Clone, Copy)]
struct HideRule<'a> {
    subtype: &'a str,
    intent: Option<&'a str>,
    except_intents: &'a [&'a str],
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
        if !out.iter().any(|existing| existing == label) {
            out.push(label.to_owned());
        }
    }
    out.sort();
    Ok(out)
}

/// Annotation object overrides that hide `names` on the page (`shown = false`). Empty when
/// there is nothing to hide. Existing script overrides for the same object are omitted so the
/// type filter wins.
///
/// `page` scopes the scan to that page index so a hostile `/Annots` list on an earlier page
/// cannot starve later pages of their filter.
pub(crate) fn hidden_type_overrides(pdf: &Pdf, names: &[String], page: usize) -> Vec<(i32, i32, bool)> {
    let rules: Vec<_> = names
        .iter()
        .take(MAX_TYPES)
        .filter_map(|name| canonical_label(name).ok())
        .flat_map(|label| {
            let known = rules_for(label);
            let custom = known.is_empty().then(|| rule(label, None, &[], None));
            known.iter().copied().chain(custom)
        })
        .collect();
    if rules.is_empty() {
        return Vec::new();
    }
    let Some(page_obj) = pdf.pages().get(page) else { return Vec::new() };
    let Some(annots) = page_obj.raw().get::<Array<'_>>(ANNOTS) else { return Vec::new() };
    let mut out = Vec::new();
    let xref = page_obj.xref();
    let mut hidden_on_page = 0;
    for item in annots.raw_iter() {
        if hidden_on_page >= MAX_HIDDEN_ANNOTS {
            break;
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
        hidden_on_page += 1;
    }
    out
}

fn rule_matches(annot: &Dict<'_>, rules: &[HideRule<'_>]) -> bool {
    let subtype = annot.get::<Name<'_>>(SUBTYPE);
    if matches!(subtype.as_deref(), Some(b"Widget" | b"Link")) {
        return false;
    }
    let intent = annot.get::<Name<'_>>(IT);
    let grouped = annot.get::<Name<'_>>(RT).as_deref() == Some(b"Group");
    rules.iter().any(|rule| field_matches(subtype.as_deref(), intent.as_deref(), grouped, rule))
}

fn field_matches(subtype: Option<&[u8]>, intent: Option<&[u8]>, grouped: bool, rule: &HideRule<'_>) -> bool {
    subtype == Some(rule.subtype.as_bytes())
        && match rule.intent {
            Some(it) => intent == Some(it.as_bytes()),
            None => !rule.except_intents.iter().any(|except| intent == Some(except.as_bytes())),
        }
        && rule.grouped.is_none_or(|want| want == grouped)
}

/// The Comments panel's name for one annotation: the same name the page hides.
///
/// `None` for a field, a link, a pop-up, or a subtype that is not one of the panel's names (the
/// caller keeps that subtype). The name comes from the hide rules, so the list and the page
/// agree. An ungrouped caret is Insert text, including one whose `/IT` is `StrikeOutTextEdit`;
/// that intent is Replace text only on a StrikeOut. A grouped caret, and a caret with
/// `/IT /Replace`, are Replace text.
pub fn comment_type_label(subtype: &str, intent: Option<&str>, grouped: bool) -> Option<&'static str> {
    if matches!(subtype, "Widget" | "Link" | "Popup") {
        return None;
    }
    let intent = intent.map(str::as_bytes);
    COMMENT_TYPES
        .iter()
        .find(|(_, rules)| rules.iter().any(|rule| field_matches(Some(subtype.as_bytes()), intent, grouped, rule)))
        .map(|(label, _)| *label)
}

fn canonical_label(name: &str) -> Result<&str, &'static str> {
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
        _ => name,
    };
    Ok(label)
}

const COMMENT_TYPES: &[(&str, &[HideRule<'static>])] = &[
    ("Note", &[rule("Text", None, &[], None)]),
    ("Text box", &[rule("FreeText", None, &["FreeTextCallout", "FreeTextTypeWriter"], None)]),
    ("Callout", &[rule("FreeText", Some("FreeTextCallout"), &[], None)]),
    ("Typewriter", &[rule("FreeText", Some("FreeTextTypeWriter"), &[], None)]),
    ("Strikethrough", &[rule("StrikeOut", None, &["StrikeOutTextEdit"], None)]),
    // Replace text can use a strikeout primary or a caret primary.
    (
        "Replace text",
        &[rule("StrikeOut", Some("StrikeOutTextEdit"), &[], None), rule("Caret", None, &[], Some(true)), rule("Caret", Some("Replace"), &[], None)],
    ),
    ("Insert text", &[rule("Caret", None, &["Replace"], Some(false))]),
    ("Rectangle", &[rule("Square", None, &[], None)]),
    ("Oval", &[rule("Circle", None, &[], None)]),
    ("Drawing", &[rule("Ink", None, &[], None)]),
    ("Polyline", &[rule("PolyLine", None, &[], None)]),
    ("Attachment", &[rule("FileAttachment", None, &[], None)]),
    ("Cloud", &[rule("Polygon", Some("PolygonCloud"), &[], None)]),
    ("Polygon", &[rule("Polygon", None, &["PolygonCloud"], None)]),
    ("Highlight", &[rule("Highlight", None, &[], None)]),
    ("Underline", &[rule("Underline", None, &[], None)]),
    ("Squiggly", &[rule("Squiggly", None, &[], None)]),
    ("Line", &[rule("Line", None, &[], None)]),
    ("Stamp", &[rule("Stamp", None, &[], None)]),
    ("Redact", &[rule("Redact", None, &[], None)]),
    ("Sound", &[rule("Sound", None, &[], None)]),
];

fn rules_for(label: &str) -> &'static [HideRule<'static>] {
    COMMENT_TYPES.iter().find(|(name, _)| *name == label).map_or(&[], |(_, rules)| *rules)
}

const fn rule<'a>(subtype: &'a str, intent: Option<&'a str>, except_intents: &'a [&'a str], grouped: Option<bool>) -> HideRule<'a> {
    HideRule { subtype, intent, except_intents, grouped }
}

#[cfg(test)]
mod tests {
    use super::{comment_type_label, field_matches, normalize_hidden_comment_types, rules_for};

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

    /// The panel name and the hide rules name the same comment. An ungrouped caret with the
    /// strikeout intent is Insert text; that intent is Replace text only on a StrikeOut.
    #[test]
    fn the_panel_name_is_the_type_the_page_hides() {
        let cases = [
            ("Caret", Some("StrikeOutTextEdit"), false, "Insert text"),
            ("Caret", Some("FreeTextCallout"), false, "Insert text"),
            ("Caret", None, false, "Insert text"),
            ("Caret", Some("Replace"), false, "Replace text"),
            ("Caret", Some("Replace"), true, "Replace text"),
            ("Caret", None, true, "Replace text"),
            ("Caret", Some("StrikeOutTextEdit"), true, "Replace text"),
            ("Caret", Some("FreeTextCallout"), true, "Replace text"),
            ("StrikeOut", Some("StrikeOutTextEdit"), false, "Replace text"),
            ("StrikeOut", Some("StrikeOutTextEdit"), true, "Replace text"),
            ("StrikeOut", None, false, "Strikethrough"),
            ("StrikeOut", Some("Replace"), false, "Strikethrough"),
            ("Highlight", Some("StrikeOutTextEdit"), false, "Highlight"),
            ("Square", Some("Replace"), false, "Rectangle"),
            ("FreeText", Some("Replace"), false, "Text box"),
            ("FreeText", Some("FreeTextCallout"), false, "Callout"),
            ("FreeText", Some("FreeTextTypeWriter"), false, "Typewriter"),
            ("FreeText", None, false, "Text box"),
            ("Polygon", Some("PolygonCloud"), false, "Cloud"),
            ("Polygon", None, false, "Polygon"),
            ("Circle", None, false, "Oval"),
            ("Text", None, false, "Note"),
        ];
        let others = [
            "Insert text",
            "Replace text",
            "Strikethrough",
            "Text box",
            "Callout",
            "Typewriter",
            "Highlight",
            "Rectangle",
            "Cloud",
            "Polygon",
            "Oval",
            "Note",
        ];
        for (subtype, intent, grouped, expect) in cases {
            assert_eq!(comment_type_label(subtype, intent, grouped), Some(expect), "{subtype} {intent:?} grouped={grouped}");
            let intent_bytes = intent.map(str::as_bytes);
            let matches = |label| rules_for(label).iter().any(|rule| field_matches(Some(subtype.as_bytes()), intent_bytes, grouped, rule));
            assert!(matches(expect), "{subtype} is not hidden as {expect}");
            for other in others {
                if other == expect {
                    continue;
                }
                assert!(!matches(other), "{subtype} {intent:?} grouped={grouped} is also {other}");
            }
        }
        assert_eq!(comment_type_label("Widget", None, false), None);
        assert_eq!(comment_type_label("Link", None, false), None);
        assert_eq!(comment_type_label("Popup", None, false), None);
        assert_eq!(comment_type_label("MyCustom", Some("Replace"), false), None);
    }
}
