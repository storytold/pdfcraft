//! App-owned target identity and conservative inclusion of actual word ink.
use super::Target;
use pdfcraft_engine::{DocId, Session};
use serde_json::{Value, json};

pub(crate) fn current(session: &Session, expected: &Target) -> Option<Target> {
    let id = DocId(expected.document.strip_prefix("pdf:")?.parse().ok()?);
    let document = session.get(id)?;
    Some(Target { document: expected.document.clone(), revision: document.edit_generation() })
}

pub(crate) fn for_doc(session: &Session, id: DocId) -> Option<Target> {
    let document = session.get(id)?;
    Some(Target { document: format!("pdf:{}", id.0), revision: document.edit_generation() })
}

fn numbers<const N: usize>(value: &Value) -> Option<[f64; N]> {
    let values: Vec<_> = value.as_array()?.iter().map(Value::as_f64).collect::<Option<_>>()?;
    let values: [f64; N] = values.try_into().ok()?;
    values.iter().all(|x| x.is_finite()).then_some(values)
}

fn in_quad(point: [f64; 2], q: [f64; 8]) -> bool {
    // PDF QuadPoints are top-left, top-right, bottom-left, bottom-right, not polygon order.
    let points = [[q[0], q[1]], [q[2], q[3]], [q[6], q[7]], [q[4], q[5]]];
    let mut positive = false;
    let mut negative = false;
    let mut area = 0.0;
    for i in 0..4 {
        let a = points[i];
        let b = points[(i + 1) % 4];
        let cross = (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0]);
        positive |= cross > 1e-5;
        negative |= cross < -1e-5;
        area += a[0] * b[1] - b[0] * a[1];
    }
    area.abs() > 1e-5 && !(positive && negative)
}

/// Every changed word must be entirely in a captured quad. A paragraph's large bounding box
/// cannot stand in for an incomplete text selection, including across rotations/CropBoxes.
pub(crate) fn contains(selection: &Value, proof: &Value) -> bool {
    if selection["doc"] != proof["doc"] {
        return false;
    }
    let Some(selected) = selection["pages"].as_array() else { return false };
    let Some(pages) = proof["pages"].as_array().filter(|p| !p.is_empty()) else { return false };
    if !pages.iter().all(|p| selected.contains(p)) {
        return false;
    }
    let Some(rects) = selection["rects"].as_array() else { return false };
    if rects.is_empty() {
        return true;
    }
    let Some(quads) = selection["quads"].as_array() else { return false };
    let Some(regions) = proof["rects"].as_array().filter(|r| !r.is_empty()) else { return false };
    regions.iter().all(|region| {
        let Some(r) = numbers::<4>(&region["rect"]) else { return false };
        if r[0] > r[2] || r[1] > r[3] {
            return false;
        }
        let corners = if let Some(q) = numbers::<8>(&region["quad"]) {
            [[q[0], q[1]], [q[2], q[3]], [q[4], q[5]], [q[6], q[7]]]
        } else {
            [[r[0], r[1]], [r[0], r[3]], [r[2], r[1]], [r[2], r[3]]]
        };
        quads.iter().any(|quad| quad["page"] == region["page"] && numbers::<8>(&quad["quad"]).is_some_and(|q| corners.iter().all(|p| in_quad(*p, q))))
    })
}

/// Rebind a text selection to its edited paragraph's actual new ink. Other mutations keep
/// the original captured scope. No command/format/proposal text is interpreted here.
pub(crate) fn after(selection: &Value, proof: &Value) -> Value {
    if selection["rects"].as_array().is_none_or(Vec::is_empty) || !proof["textTarget"].is_object() {
        return selection.clone();
    }
    let Some(rects) = proof["rects"].as_array() else { return Value::Null };
    let quads: Vec<_> = rects
        .iter()
        .filter_map(|region| {
            let q =
                numbers::<8>(&region["quad"]).or_else(|| numbers::<4>(&region["rect"]).map(|r| [r[0], r[3], r[2], r[3], r[0], r[1], r[2], r[1]]))?;
            Some(json!({"page":region["page"],"quad":q}))
        })
        .collect();
    json!({"doc":proof["doc"],"pages":proof["pages"],"rects":rects,"quads":quads,"text":proof["textTarget"]["text"]})
}
