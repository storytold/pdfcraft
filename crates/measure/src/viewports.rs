//! Viewport management preserves ordering, unknown entries and indirect containers.
use crate::{Result, Scale, check_points, invalid, nums, page, text};
use pdfcraft_cos::{Dict, Document, Object};
use serde::Serialize;

pub const MAX_VIEWPORTS: usize = 1024;
#[derive(Clone, Debug, Serialize)]
pub struct Viewport {
    pub index: usize,
    pub name: String,
    pub bbox: [f64; 4],
    pub scale: std::result::Result<Scale, String>,
}
fn rectangle(doc: &Document, d: &Dict) -> Result<[f64; 4]> {
    let object = doc.resolve(d.get(b"BBox").ok_or_else(|| invalid("viewport has no bounding box"))?);
    let values = object.as_array().filter(|v| v.len() == 4).ok_or_else(|| invalid("invalid viewport bounding box"))?;
    let mut bbox = [0.0; 4];
    for (target, value) in bbox.iter_mut().zip(values) {
        *target = doc.resolve(value).as_f64().ok_or_else(|| invalid("invalid viewport coordinate"))?;
    }
    validate(bbox, "")?;
    Ok(bbox)
}
fn validate(bbox: [f64; 4], name: &str) -> Result<()> {
    check_points(&[[bbox[0], bbox[1]], [bbox[2], bbox[3]]])?;
    if bbox[2] <= bbox[0] || bbox[3] <= bbox[1] || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(invalid("viewport needs a positive rectangle and a printable name of at most 256 bytes"));
    }
    Ok(())
}
pub(crate) fn array(doc: &Document, index: usize) -> Result<Vec<Object>> {
    let p = page(doc, index)?;
    match p.dict.get(b"VP") {
        None => Ok(Vec::new()),
        Some(o) => {
            let object = doc.resolve(o);
            let a = object.as_array().ok_or_else(|| invalid("invalid page viewports"))?;
            if a.len() > MAX_VIEWPORTS {
                return Err(invalid("page has too many measurement viewports"));
            }
            Ok(a.clone())
        }
    }
}
pub(crate) fn write(doc: &mut Document, index: usize, values: Vec<Object>) -> Result<()> {
    let p = page(doc, index)?;
    match p.dict.get(b"VP").and_then(Object::as_ref) {
        Some(_) => {
            // Detach before editing: another page or extension may reference this array.
            let r = doc.add(Object::Array(values));
            doc.update_dict(p.obj, |d| d.set(b"VP".to_vec(), Object::Ref(r))).map_err(|e| invalid(&e.to_string()))?;
        }
        None => doc.update_dict(p.obj, |d| d.set(b"VP".to_vec(), Object::Array(values))).map_err(|e| invalid(&e.to_string()))?,
    }
    Ok(())
}
pub fn list(doc: &Document, page_index: usize) -> Result<Vec<Viewport>> {
    array(doc, page_index)?
        .iter()
        .enumerate()
        .map(|(index, o)| {
            let object = doc.resolve(o);
            let d = object.as_dict().ok_or_else(|| invalid("invalid viewport dictionary"))?;
            let bbox = rectangle(doc, d)?;
            let scale = d
                .get(b"Measure")
                .ok_or_else(|| "viewport has no measurement scale".to_string())
                .and_then(|o| Scale::read_in_viewport(doc, o, bbox).map_err(|e| e.to_string()));
            Ok(Viewport { index, name: text(d, b"Name"), bbox, scale })
        })
        .collect()
}
/// Replace one viewport without changing the order in which scales take precedence.
pub fn update(doc: &mut Document, page_index: usize, index: usize, bbox: [f64; 4], name: &str, scale: &Scale) -> Result<()> {
    validate(bbox, name)?;
    let measure = scale.dictionary()?;
    let mut values = array(doc, page_index)?;
    let old = values.get(index).ok_or_else(|| invalid("measurement viewport does not exist"))?;
    let object = doc.resolve(old);
    let mut d = object.as_dict().cloned().ok_or_else(|| invalid("invalid viewport dictionary"))?;
    d.set(b"BBox".to_vec(), nums(bbox));
    d.set(b"Name".to_vec(), pdfcraft_cos::PdfString::text(name));
    d.set(b"Measure".to_vec(), Object::Dict(measure));
    match old.as_ref() {
        Some(_) => {
            // Keep this viewport indirect without changing other owners of the old object.
            let r = doc.add(Object::Dict(d));
            if let Some(target) = values.get_mut(index) {
                *target = Object::Ref(r);
            }
            write(doc, page_index, values)?;
        }
        None => {
            if let Some(target) = values.get_mut(index) {
                *target = Object::Dict(d);
            }
            write(doc, page_index, values)?;
        }
    }
    Ok(())
}
/// Removing a drawing scale never changes the scales captured by saved annotations.
pub fn remove(doc: &mut Document, page_index: usize, index: usize) -> Result<()> {
    let mut values = array(doc, page_index)?;
    if index >= values.len() {
        return Err(invalid("measurement viewport does not exist"));
    }
    values.remove(index);
    write(doc, page_index, values)
}
