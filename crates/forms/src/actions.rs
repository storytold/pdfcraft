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

fn texts(doc: &Document, o: &Object) -> Vec<String> {
    match &*doc.resolve(o) {
        Object::Array(a) => a.iter().filter_map(|x| doc.resolve(x).as_string().map(|s| s.to_text())).collect(),
        Object::String(s) => vec![s.to_text()],
        _ => Vec::new(),
    }
}

fn read(doc: &Document, o: &Object, pages: &[pdfcraft_cos::ObjRef]) -> Option<FieldAction> {
    let a = doc.resolve(o);
    let d = a.as_dict()?;
    Some(match d.name(b"S")? {
        b"JavaScript" => FieldAction::JavaScript(crate::script(doc, o).unwrap_or_default()),
        b"URI" => FieldAction::Uri(d.get(b"URI").and_then(|u| doc.resolve(u).as_string().map(|s| s.to_text())).unwrap_or_default()),
        b"ResetForm" => FieldAction::Reset(d.get(b"Fields").map(|f| texts(doc, f)).unwrap_or_default()),
        b"Named" => FieldAction::Named(d.name(b"N").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default()),
        b"GoTo" => {
            let dest = d.get(b"D").map(|x| doc.resolve(x))?;
            let target = dest.as_array()?.first()?.as_ref()?;
            FieldAction::GoTo(pages.iter().position(|p| *p == target)?)
        }
        b"Hide" => FieldAction::ShowHide {
            fields: d.get(b"T").map(|t| texts(doc, t)).unwrap_or_default(),
            hide: !matches!(d.get(b"H"), Some(Object::Bool(false))),
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
    for t in Trigger::ALL {
        let o = match t.key() {
            None => wd.get(b"A").cloned().or_else(|| aa.get(b"U").cloned()),
            Some(k) => aa.get(k).cloned(),
        };
        // An action chain (/Next) counts as its first action here.
        if let Some(a) = o.and_then(|o| read(doc, &o, &pages)) {
            out.push((t, a));
        }
    }
    Ok(out)
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

/// Replace field `name`'s actions (on every widget) with `actions`; triggers not listed lose
/// theirs. `Other` actions are left as they are.
pub fn set_field_actions(doc: &mut Document, name: &str, actions: &[(Trigger, FieldAction)]) -> Result<(), FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?.clone();
    let pages = crate::page_refs(doc);
    for w in &f.widgets {
        let wd = doc.get(w.obj).as_dict().cloned().unwrap_or_default();
        let mut aa = wd.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
        let mut a_entry = wd.get(b"A").cloned();
        for t in Trigger::ALL {
            let new = actions.iter().find(|(x, _)| *x == t).map(|(_, a)| a);
            if matches!(new, Some(FieldAction::Other(_))) {
                continue;
            }
            let obj = match new {
                Some(a) => write(doc, a, &pages)?,
                None => None,
            };
            match t.key() {
                None => {
                    a_entry = obj;
                    aa.remove(b"U");
                }
                Some(k) => match obj {
                    Some(o) => aa.set(k.to_vec(), o),
                    None => {
                        aa.remove(k);
                    }
                },
            }
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
