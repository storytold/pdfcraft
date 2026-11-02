use super::dates::{data_date_pattern, data_to_iso, iso_to_data, iso_to_pattern, pattern_to_iso};
use super::path::{data_path, parse_datasets, som_to_path};
use super::{DataPath, Dict, Document, ObjRef, Object, Template};

/// What a field holds, for writing the datasets.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldData {
    Text(String),
    /// Checked or not.
    Check(bool),
    /// The selected button's on value, if any.
    Radio(Option<String>),
    /// Nothing to write (buttons, signatures).
    None,
}

/// One field of the AcroForm, as the engine sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDatum {
    /// The terminal field dictionary (where `/PCSom` and `/PCItems` live).
    pub obj: ObjRef,
    /// The fully qualified field name (a static form's SOM path).
    pub name: String,
    pub data: FieldData,
}

pub(super) fn text_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text()))
}

/// The SOM path of a field: its `/PCSom` when PdfCraft generated it, else its name.
fn som_of(doc: &Document, f: &FieldDatum) -> String {
    doc.get(f.obj).as_dict().and_then(|d| text_of(doc, d, crate::pdf::SOM_KEY)).unwrap_or_else(|| f.name.clone())
}

/// The datasets text for a radio group. The AcroForm value is an appearance-state name
/// (`New York` is stored as `New_York`); `/PCOn` on the selected widget is the original item.
fn radio_dataset_value(doc: &Document, f: &FieldDatum, sel: &Option<String>) -> String {
    let Some(selected) = sel.as_deref().filter(|s| !s.is_empty() && *s != "Off") else {
        return String::new();
    };
    let Some(group) = doc.get(f.obj).as_dict().cloned() else { return selected.to_string() };
    let kids = group.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_else(|| vec![Object::Ref(f.obj)]);
    for kid in kids {
        let Some(widget) = doc.resolve(&kid).as_dict().cloned() else { continue };
        let original = text_of(doc, &widget, b"PCOn");
        let state = widget
            .get(b"AP")
            .map(|ap| doc.resolve(ap))
            .and_then(|ap| ap.as_dict().and_then(|a| a.get(b"N").cloned()))
            .and_then(|n| doc.resolve(&n).as_dict().cloned())
            .and_then(|n| n.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).find(|k| k != "Off"));
        if state.as_deref() == Some(selected) || original.as_deref() == Some(selected) {
            return original.unwrap_or_else(|| selected.to_string());
        }
    }
    selected.to_string()
}

/// The on and off item values of a check box (`/PCItems`), or the widget's on state.
fn items_of(doc: &Document, f: &FieldDatum) -> (String, String) {
    let d = doc.get(f.obj);
    let items: Vec<String> = d
        .as_dict()
        .and_then(|d| d.get(b"PCItems").map(|o| doc.resolve(o)))
        .and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| x.as_string().map(|s| s.to_text())).collect()))
        .unwrap_or_default();
    let on = items.first().cloned().unwrap_or_else(|| "1".into());
    let off = items.get(1).cloned().unwrap_or_default();
    (on, off)
}

/// The Acrobat date pattern a field's format action names (`AFDate_FormatEx("yyyy-mm-dd")`).
fn date_pattern_of(doc: &Document, f: &FieldDatum) -> Option<String> {
    let d = doc.get(f.obj);
    let aa = doc.resolve(d.as_dict()?.get(b"AA")?);
    let fa = doc.resolve(aa.as_dict()?.get(b"F")?);
    let js = text_of(doc, fa.as_dict()?, b"JS")?;
    let i = js.find("AFDate_FormatEx(")?;
    let rest = &js[i + "AFDate_FormatEx(".len()..];
    let q = rest.find('"')?;
    let body = &rest[q + 1..];
    Some(body[..body.find('"')?].to_string())
}

/// A value to write: where (the field's SOM path, and the data path it binds to) and what.
pub(super) struct Want {
    pub(super) som: String,
    pub(super) path: DataPath,
    pub(super) text: String,
    /// Create the node even for an empty value (an instance of a repeating subform).
    pub(super) ensure: bool,
}

/// `tpl`, or the document's own template when none is given (parsed once).
pub(super) fn with_template<T>(doc: &Document, tpl: Option<&Template>, f: impl FnOnce(Option<&Template>) -> T) -> T {
    match tpl {
        Some(t) => f(Some(t)),
        None => {
            let parsed = crate::script::template_of(doc).ok();
            f(parsed.as_ref())
        }
    }
}

/// [`data_path`] when the template is known, else [`som_to_path`].
pub(super) fn path_of(tpl: Option<&Template>, som: &str) -> DataPath {
    match tpl {
        Some(t) => data_path(t, som),
        None => som_to_path(som),
    }
}

/// The values the datasets should hold for `fields`, by data path: dates in each field's data
/// picture (or ISO), check and radio states, choice values as saved.
pub(super) fn wanted(doc: &Document, fields: &[FieldDatum], tpl: Option<&Template>) -> Vec<Want> {
    let mut out = Vec::new();
    for f in fields {
        let som = som_of(doc, f);
        let path = path_of(tpl, &som);
        if path.is_empty() {
            continue;
        }
        let text = match &f.data {
            FieldData::Text(t) => match date_pattern_of(doc, f) {
                Some(p) => match pattern_to_iso(t, &p) {
                    // The date goes into the data in the field's data picture, or as an ISO date.
                    Some(iso) => iso_to_data(iso, data_date_pattern(tpl, &som).as_deref()),
                    None => t.clone(),
                },
                None => t.clone(),
            },
            FieldData::Check(on) => {
                let (on_item, off_item) = items_of(doc, f);
                if *on { on_item } else { off_item }
            }
            FieldData::Radio(sel) => radio_dataset_value(doc, f, sel),
            FieldData::None => continue,
        };
        out.push(Want { som, path, text, ensure: false });
    }
    out
}

/// The values the datasets hold for `fields`: `(field name, what it should hold)`, only for
/// fields the data mentions.
pub fn read_values(doc: &Document, fields: &[FieldDatum]) -> Vec<(String, FieldData)> {
    read_values_with(doc, None, fields)
}

/// [`read_values`] with the form's parsed template when the caller has it; `None` parses the
/// document's.
pub fn read_values_with(doc: &Document, tpl: Option<&Template>, fields: &[FieldDatum]) -> Vec<(String, FieldData)> {
    with_template(doc, tpl, |t| values_at(doc, t, fields))
}

/// The values the document's datasets hold for `fields`, read in each field's data picture and
/// shown in its own pattern; only fields the data mentions.
fn values_at(doc: &Document, tpl: Option<&Template>, fields: &[FieldDatum]) -> Vec<(String, FieldData)> {
    let Ok(Some(p)) = crate::read_packets(doc) else { return Vec::new() };
    let Some(data) = parse_datasets(&p.xdp) else { return Vec::new() };
    let mut out = Vec::new();
    for f in fields {
        let som = som_of(doc, f);
        let path = path_of(tpl, &som);
        let Some(text) = data.value_at(&path) else { continue };
        let text = text.as_str();
        let value = match &f.data {
            FieldData::Text(_) => FieldData::Text(match date_pattern_of(doc, f) {
                Some(pat) => iso_to_pattern(&data_to_iso(text, data_date_pattern(tpl, &som).as_deref()), &pat),
                None => text.to_string(),
            }),
            FieldData::Check(_) => {
                let (on, _) = items_of(doc, f);
                FieldData::Check(is_on(text, &on))
            }
            FieldData::Radio(_) => FieldData::Radio((!text.trim().is_empty()).then(|| text.trim().to_string())),
            FieldData::None => continue,
        };
        out.push((f.name.clone(), value));
    }
    out
}

/// Does a data value mean "checked" for a button whose on value is `on`?
pub fn is_on(text: &str, on: &str) -> bool {
    let t = text.trim();
    if t == on {
        return true;
    }
    !matches!(t.to_ascii_lowercase().as_str(), "" | "0" | "off" | "false" | "no") && on.trim().is_empty()
}
