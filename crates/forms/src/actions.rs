//! Field Properties ▸ Actions: what a field does on each trigger (§12.6.3, Table 197: the
//! widget's `/A` for Mouse Up and its `/AA` D, E, X, Fo, Bl for the others).

use pdfcraft_cos::{Dict, Document, Object, PdfString};

use crate::{FormError, fields};

/// The Actions tab's triggers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trigger {
    MouseUp,
    MouseDown,
    MouseEnter,
    MouseExit,
    OnFocus,
    OnBlur,
}

impl Trigger {
    pub const ALL: [Trigger; 6] = [Trigger::MouseUp, Trigger::MouseDown, Trigger::MouseEnter, Trigger::MouseExit, Trigger::OnFocus, Trigger::OnBlur];

    pub fn label(self) -> &'static str {
        match self {
            Trigger::MouseUp => "Mouse Up",
            Trigger::MouseDown => "Mouse Down",
            Trigger::MouseEnter => "Mouse Enter",
            Trigger::MouseExit => "Mouse Exit",
            Trigger::OnFocus => "On Focus",
            Trigger::OnBlur => "On Blur",
        }
    }

    /// Snake-case id (agents).
    pub fn id(self) -> &'static str {
        match self {
            Trigger::MouseUp => "mouse_up",
            Trigger::MouseDown => "mouse_down",
            Trigger::MouseEnter => "mouse_enter",
            Trigger::MouseExit => "mouse_exit",
            Trigger::OnFocus => "on_focus",
            Trigger::OnBlur => "on_blur",
        }
    }

    pub fn from_id(id: &str) -> Option<Trigger> {
        Trigger::ALL.into_iter().find(|t| t.id() == id)
    }

    /// The `/AA` key (`None`: Mouse Up lives in `/A`).
    fn key(self) -> Option<&'static [u8]> {
        match self {
            Trigger::MouseUp => None,
            Trigger::MouseDown => Some(b"D"),
            Trigger::MouseEnter => Some(b"E"),
            Trigger::MouseExit => Some(b"X"),
            Trigger::OnFocus => Some(b"Fo"),
            Trigger::OnBlur => Some(b"Bl"),
        }
    }
}

/// The Actions tab's "Select Action" choices.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldAction {
    /// Run a JavaScript.
    JavaScript(String),
    /// Open a web link.
    Uri(String),
    /// Reset a form: the listed fields, or every field when empty.
    Reset(Vec<String>),
    /// Execute a menu item (a named action: Print, NextPage, PrevPage, FirstPage, LastPage).
    Named(String),
    /// Go to a page view (0-based page).
    GoTo(usize),
    /// Show/hide a field.
    ShowHide { fields: Vec<String>, hide: bool },
    /// Submit a form.
    Submit(String),
    /// An action of another type, kept as it is.
    Other(String),
}

impl FieldAction {
    /// How the Actions tab lists it.
    pub fn describe(&self) -> String {
        match self {
            FieldAction::JavaScript(_) => "Run a JavaScript".into(),
            FieldAction::Uri(u) => format!("Open a web link: {u}"),
            FieldAction::Reset(f) if f.is_empty() => "Reset a form".into(),
            FieldAction::Reset(f) => format!("Reset a form: {}", f.join(", ")),
            FieldAction::Named(n) => format!("Execute a menu item: {n}"),
            FieldAction::GoTo(p) => format!("Go to a page view: page {}", p + 1),
            FieldAction::ShowHide { fields, hide } => format!("{} a field: {}", if *hide { "Hide" } else { "Show" }, fields.join(", ")),
            FieldAction::Submit(u) => format!("Submit a form: {u}"),
            FieldAction::Other(s) => s.clone(),
        }
    }
}

fn read(doc: &Document, o: &Object, pages: &[pdfcraft_cos::ObjRef], scripts: &mut pdfcraft_cos::DecodeBudget) -> Option<FieldAction> {
    let a = doc.resolve(o);
    let d = a.as_dict()?;
    Some(match d.name(b"S")? {
        b"JavaScript" => FieldAction::JavaScript(crate::script(doc, o, scripts).unwrap_or_default()),
        b"URI" => FieldAction::Uri(d.get(b"URI").and_then(|u| doc.resolve(u).as_string().map(|s| s.to_text())).unwrap_or_default()),
        b"ResetForm" => FieldAction::Reset(d.get(b"Fields").map(|f| crate::read::action_fields(doc, f)).unwrap_or_default()),
        b"Named" => FieldAction::Named(d.name(b"N").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default()),
        b"GoTo" => {
            let dest = d.get(b"D").map(|x| doc.resolve(x))?;
            let target = dest.as_array()?.first()?.as_ref()?;
            FieldAction::GoTo(pages.iter().position(|p| *p == target)?)
        }
        b"Hide" => FieldAction::ShowHide {
            fields: d.get(b"T").map(|t| crate::read::action_fields(doc, t)).unwrap_or_default(),
            hide: !d.get(b"H").is_some_and(|h| matches!(&*doc.resolve(h), Object::Bool(false))),
        },
        b"SubmitForm" => FieldAction::Submit(
            d.get(b"F")
                .map(|f| doc.resolve(f))
                .and_then(|f| {
                    f.as_string()
                        .map(|s| s.to_text())
                        .or_else(|| f.as_dict().and_then(|fd| fd.get(b"F")).and_then(|x| doc.resolve(x).as_string().map(|s| s.to_text())))
                })
                .unwrap_or_default(),
        ),
        other => FieldAction::Other(String::from_utf8_lossy(other).into_owned()),
    })
}

/// The actions of field `name` (its first widget), by trigger, in the tab's order.
pub fn field_actions(doc: &Document, name: &str) -> Result<Vec<(Trigger, FieldAction)>, FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let pages = crate::page_refs(doc);
    let Some(w) = f.widgets.first() else { return Ok(Vec::new()) };
    let wd = doc.get(w.obj).as_dict().cloned().unwrap_or_default();
    let aa = wd.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
    let mut out = Vec::new();
    let mut scripts = crate::script_budget();
    for t in Trigger::ALL {
        let o = match t.key() {
            None => wd.get(b"A").cloned().or_else(|| aa.get(b"U").cloned()),
            Some(k) => aa.get(k).cloned(),
        };
        // An action chain (/Next) counts as its first action here.
        if let Some(a) = o.and_then(|o| read(doc, &o, &pages, &mut scripts)) {
            out.push((t, a));
        }
    }
    Ok(out)
}

/// Most chained actions [`field_action_chains`] walks per trigger.
const MAX_CHAIN: usize = 1000;

/// How many actions follow `action` through `/Next` (a dictionary or an array of them, each with
/// its own `/Next`), counting each object once.
fn chained(doc: &Document, action: &Object) -> usize {
    let mut seen = std::collections::HashSet::new();
    if let Object::Ref(r) = action {
        seen.insert(*r);
    }
    let mut stack: Vec<Object> = doc.resolve(action).as_dict().and_then(|d| d.get(b"Next").cloned()).into_iter().collect();
    let mut count = 0;
    while let Some(o) = stack.pop() {
        if count >= MAX_CHAIN {
            break;
        }
        if let Object::Ref(r) = &o
            && !seen.insert(*r)
        {
            continue;
        }
        match &*doc.resolve(&o) {
            Object::Array(items) => stack.extend(items.iter().cloned()),
            Object::Dict(d) => {
                count += 1;
                stack.extend(d.get(b"Next").cloned());
            }
            _ => {}
        }
    }
    count
}

/// For each trigger of field `name` (its first widget) whose action has others chained after it,
/// how many there are.
pub fn field_action_chains(doc: &Document, name: &str) -> Result<Vec<(Trigger, usize)>, FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let Some(w) = f.widgets.first() else { return Ok(Vec::new()) };
    let wd = doc.get(w.obj).as_dict().cloned().unwrap_or_default();
    let aa = wd.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
    Ok(Trigger::ALL
        .into_iter()
        .filter_map(|t| {
            let o = match t.key() {
                None => wd.get(b"A").cloned().or_else(|| aa.get(b"U").cloned()),
                Some(k) => aa.get(k).cloned(),
            }?;
            let n = chained(doc, &o);
            (n > 0).then_some((t, n))
        })
        .collect())
}

fn write(doc: &mut Document, a: &FieldAction, pages: &[pdfcraft_cos::ObjRef]) -> Result<Option<Object>, FormError> {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Action"));
    let list = |v: &[String]| Object::Array(v.iter().map(|s| Object::String(PdfString::text(s))).collect());
    match a {
        FieldAction::JavaScript(js) => {
            d.set(b"S".to_vec(), Object::name("JavaScript"));
            d.set(b"JS".to_vec(), PdfString::text(js));
        }
        FieldAction::Uri(u) => {
            d.set(b"S".to_vec(), Object::name("URI"));
            d.set(b"URI".to_vec(), PdfString::literal(u.as_bytes().to_vec()));
        }
        FieldAction::Reset(f) => {
            d.set(b"S".to_vec(), Object::name("ResetForm"));
            if !f.is_empty() {
                d.set(b"Fields".to_vec(), list(f));
            }
        }
        FieldAction::Named(n) => {
            d.set(b"S".to_vec(), Object::name("Named"));
            d.set(b"N".to_vec(), Object::name(n));
        }
        FieldAction::GoTo(p) => {
            let page = *pages.get(*p).ok_or_else(|| FormError::Invalid(format!("the document has no page {}", p + 1)))?;
            d.set(b"S".to_vec(), Object::name("GoTo"));
            d.set(b"D".to_vec(), Object::Array(vec![Object::Ref(page), Object::name("Fit")]));
        }
        FieldAction::ShowHide { fields, hide } => {
            d.set(b"S".to_vec(), Object::name("Hide"));
            d.set(b"T".to_vec(), list(fields));
            d.set(b"H".to_vec(), Object::Bool(*hide));
        }
        FieldAction::Submit(u) => {
            d.set(b"S".to_vec(), Object::name("SubmitForm"));
            let mut fs = Dict::new();
            fs.set(b"FS".to_vec(), Object::name("URL"));
            fs.set(b"F".to_vec(), PdfString::literal(u.as_bytes().to_vec()));
            d.set(b"F".to_vec(), Object::Dict(fs));
            // HTML form format, as Acrobat's default for web servers (ExportFormat, bit 3).
            d.set(b"Flags".to_vec(), Object::Int(4));
        }
        FieldAction::Other(_) => return Ok(None),
    }
    Ok(Some(Object::Ref(doc.add(Object::Dict(d)))))
}

/// Rewrite one trigger of a widget to `new` (`None` clears it), given the `old` value read from
/// the widget. `aa` and `a_entry` are the widget's `/AA` dict and `/A` entry, updated in place.
struct TriggerUpdate<'a> {
    trigger: Trigger,
    replacement: Option<&'a FieldAction>,
    previous: Option<Object>,
}

fn replace_trigger(
    doc: &mut Document,
    pages: &[pdfcraft_cos::ObjRef],
    scripts: &mut pdfcraft_cos::DecodeBudget,
    update: TriggerUpdate<'_>,
    aa: &mut Dict,
    a_entry: &mut Option<Object>,
) -> Result<(), FormError> {
    let TriggerUpdate { trigger: t, replacement: new, previous: old } = update;
    let old_action = old.as_ref().and_then(|o| read(doc, o, pages, scripts));
    if new.is_some() && new == old_action.as_ref() {
        return Ok(());
    }
    // The list shows a trigger's first action: replacing it keeps the ones chained after it.
    let next = new.and(old).and_then(|o| doc.resolve(&o).as_dict().and_then(|d| d.get(b"Next").cloned()));
    let obj = match new {
        Some(a) => write(doc, a, pages)?,
        None => None,
    };
    if let (Some(Object::Ref(r)), Some(next)) = (&obj, next) {
        doc.update_dict(*r, |d| d.set(b"Next".to_vec(), next))?;
    }
    match t.key() {
        None => {
            *a_entry = obj;
            aa.remove(b"U");
        }
        Some(k) => match obj {
            Some(o) => aa.set(k.to_vec(), o),
            None => {
                aa.remove(k);
            }
        },
    }
    Ok(())
}

/// Replace field `name`'s actions (on every widget) with `actions`; triggers not listed lose
/// theirs. `Other` actions, and actions that read the same as before, are left as they are (with
/// their chain and the keys this module doesn't read); a replaced action keeps its chain.
pub fn set_field_actions(doc: &mut Document, name: &str, actions: &[(Trigger, FieldAction)]) -> Result<(), FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?.clone();
    let pages = crate::page_refs(doc);
    let mut scripts = crate::script_budget();
    for w in &f.widgets {
        let wd = doc.get(w.obj).as_dict().cloned().unwrap_or_default();
        let mut aa = wd.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
        let mut a_entry = wd.get(b"A").cloned();
        for t in Trigger::ALL {
            let new = actions.iter().find(|(x, _)| *x == t).map(|(_, a)| a);
            if matches!(new, Some(FieldAction::Other(_))) {
                continue;
            }
            let old = match t.key() {
                None => wd.get(b"A").cloned().or_else(|| aa.get(b"U").cloned()),
                Some(k) => aa.get(k).cloned(),
            };
            replace_trigger(doc, &pages, &mut scripts, TriggerUpdate { trigger: t, replacement: new, previous: old }, &mut aa, &mut a_entry)?;
        }
        doc.update_dict(w.obj, |d| {
            match &a_entry {
                Some(a) => d.set(b"A".to_vec(), a.clone()),
                None => {
                    d.remove(b"A");
                }
            }
            if aa.is_empty() {
                d.remove(b"AA");
            } else {
                d.set(b"AA".to_vec(), Object::Dict(aa.clone()));
            }
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_and_hide_actions_resolve_hierarchical_widget_and_indirect_field_targets() {
        let mut doc = Document::new_empty();
        let rect = || Object::Array(vec![0.into(), 0.into(), 50.into(), 20.into()]);

        // 1. Hierarchical child field `address.city` (merged field + widget).
        let address = doc.add(Object::Null);
        let mut city = Dict::new();
        city.set(b"Parent".to_vec(), Object::Ref(address));
        city.set(b"FT".to_vec(), Object::name("Tx"));
        city.set(b"T".to_vec(), PdfString::text("city"));
        city.set(b"Subtype".to_vec(), Object::name("Widget"));
        city.set(b"Rect".to_vec(), rect());
        let city = doc.add(Object::Dict(city));
        let mut addr_dict = Dict::new();
        addr_dict.set(b"T".to_vec(), PdfString::text("address"));
        addr_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(city)]));
        doc.set(address, Object::Dict(addr_dict));

        // 2. Separated field `phone` whose widget dictionary has `/Parent` instead of `/T`.
        let phone = doc.add(Object::Null);
        let mut phone_widget = Dict::new();
        phone_widget.set(b"Subtype".to_vec(), Object::name("Widget"));
        phone_widget.set(b"Parent".to_vec(), Object::Ref(phone));
        phone_widget.set(b"Rect".to_vec(), rect());
        let phone_widget = doc.add(Object::Dict(phone_widget));
        let mut phone_dict = Dict::new();
        phone_dict.set(b"FT".to_vec(), Object::name("Tx"));
        phone_dict.set(b"T".to_vec(), PdfString::text("phone"));
        phone_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(phone_widget)]));
        doc.set(phone, Object::Dict(phone_dict));

        // 3. Indirect string target `"email"`.
        let email_str = doc.add(Object::String(PdfString::text("email")));
        let targets = Object::Array(vec![Object::Ref(city), Object::Ref(phone_widget), Object::Ref(email_str)]);

        let mut reset_action = Dict::new();
        reset_action.set(b"S".to_vec(), Object::name("ResetForm"));
        reset_action.set(b"Fields".to_vec(), targets.clone());
        let reset_action = doc.add(Object::Dict(reset_action));

        let indirect_false = doc.add(Object::Bool(false));
        let mut hide_action = Dict::new();
        hide_action.set(b"S".to_vec(), Object::name("Hide"));
        hide_action.set(b"T".to_vec(), targets);
        hide_action.set(b"H".to_vec(), Object::Ref(indirect_false));
        let hide_action = doc.add(Object::Dict(hide_action));

        let mut aa = Dict::new();
        aa.set(b"D".to_vec(), Object::Ref(hide_action));
        let mut btn = Dict::new();
        btn.set(b"FT".to_vec(), Object::name("Btn"));
        btn.set(b"Ff".to_vec(), Object::Int(1 << 16));
        btn.set(b"T".to_vec(), PdfString::text("btn"));
        btn.set(b"Subtype".to_vec(), Object::name("Widget"));
        btn.set(b"Rect".to_vec(), rect());
        btn.set(b"A".to_vec(), Object::Ref(reset_action));
        btn.set(b"AA".to_vec(), Object::Dict(aa));
        let btn = doc.add(Object::Dict(btn));

        let root = doc.root().unwrap();
        let pages_ref = doc.get(root).as_dict().and_then(|d| d.reference(b"Pages")).unwrap();
        let mut page_dict = Dict::new();
        page_dict.set(b"Type".to_vec(), Object::name("Page"));
        page_dict.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page_dict.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 600.into(), 800.into()]));
        page_dict.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(city), Object::Ref(phone_widget), Object::Ref(btn)]));
        let page = doc.add(Object::Dict(page_dict));
        doc.update_dict(pages_ref, |p| {
            p.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
            p.set(b"Count".to_vec(), Object::Int(1));
        })
        .unwrap();
        let mut form = Dict::new();
        form.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(address), Object::Ref(phone), Object::Ref(btn)]));
        doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(form))).unwrap();

        let expected = vec!["address.city".to_string(), "phone".to_string(), "email".to_string()];
        let btn_field = fields(&doc).into_iter().find(|f| f.name == "btn").unwrap();
        assert_eq!(btn_field.button, Some(crate::af::ButtonAction::Reset { fields: expected.clone(), exclude: false }));

        let acts = field_actions(&doc, "btn").unwrap();
        assert_eq!(
            acts,
            vec![
                (Trigger::MouseUp, FieldAction::Reset(expected.clone())),
                (Trigger::MouseDown, FieldAction::ShowHide { fields: expected, hide: false }),
            ]
        );
    }
}
