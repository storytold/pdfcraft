use gridcraft_model::ObjKind;

use super::chart::format_series;
use super::{
    Anchor, CellRef, Color, EngineError, Json, Result, Session, Shape, ShapeKind, Sheet, anchor_mode_name, bad, bool_param, edit, json, str_param,
    u32_param,
};

/// The kind of object a `kind` parameter names: `shape` when the parameter is absent; one that
/// is present but not one of the names (`null`, a number, an unknown word) is an error, as for
/// the `object.*` commands, where the kind is required.
fn kind_param(p: &Json, cmd: &str) -> Result<ObjKind> {
    let err = || bad(cmd, "kind must be chart, image, shape, slicer or equation");
    match p.get("kind") {
        None => Ok(ObjKind::Shape),
        Some(Json::String(k)) => ObjKind::parse(k).ok_or_else(err),
        Some(_) => Err(err()),
    }
}

/// Whether `kind` names the shape `sh`: an equation is a shape too, and a plain shape is not an
/// equation, as `Sheet::object_anchor` has it for the `object.*` commands.
fn shape_is(kind: ObjKind, sh: &Shape) -> bool {
    kind != ObjKind::Equation || sh.kind == ShapeKind::Equation
}

pub(super) fn shape_fill(s: &mut Session, p: &Json) -> Result<Json> {
    if str_param(p, "kind") == Some("shape") {
        let color = str_param(p, "color").and_then(Color::from_hex).unwrap_or_default();
        return with_shape(s, p, |sh| sh.fill = color);
    }
    format_series(s, p)
}

pub(super) fn shape_outline(s: &mut Session, p: &Json) -> Result<Json> {
    let color = str_param(p, "color").and_then(Color::from_hex).unwrap_or_default();
    with_shape(s, p, |sh| sh.line = color)
}

fn with_shape(s: &mut Session, p: &Json, f: impl FnOnce(&mut Shape)) -> Result<Json> {
    let si = s.doc()?.wb.active_sheet();
    let id = u32_param(p, "id");
    edit(s, |cx| {
        let shapes = &mut cx.sheet_mut(si)?.shapes;
        let sh = match id {
            Some(id) => shapes.iter_mut().find(|x| x.id == id),
            None => shapes.last_mut(),
        }
        .ok_or_else(|| bad("shape", "no such shape"))?;
        f(sh);
        Ok(Json::Null)
    })
}

pub(super) fn arrange(s: &mut Session, p: &Json, forward: bool) -> Result<Json> {
    let id = u32_param(p, "id").ok_or_else(|| bad("arrange", "missing `id`"))?;
    let kind = kind_param(p, "arrange")?;
    let extreme = bool_param(p, "toFront").or(bool_param(p, "toBack")).unwrap_or(false);
    let si = s.doc()?.wb.active_sheet();
    crate::cmd::controls::check_objects_unlocked(s, si, kind, &[id])?;
    fn reorder<T>(v: &mut Vec<T>, i: usize, forward: bool, extreme: bool) {
        if i >= v.len() {
            return;
        }
        let item = v.remove(i);
        let j = match (forward, extreme) {
            (true, true) => v.len(),
            (false, true) => 0,
            (true, false) => (i + 1).min(v.len()),
            (false, false) => i.saturating_sub(1),
        };
        v.insert(j, item);
    }
    edit(s, |cx| {
        let sh = cx.sheet_mut(si)?;
        // An id that is not an object of this kind is an error, so a stale id, or a shape's id
        // given as an equation, never reorders something else or succeeds silently.
        let found = match kind {
            ObjKind::Chart => sh.charts.iter().position(|c| c.id == id).map(|i| reorder(&mut sh.charts, i, forward, extreme)),
            ObjKind::Image => sh.images.iter().position(|c| c.id == id).map(|i| reorder(&mut sh.images, i, forward, extreme)),
            ObjKind::Slicer => sh.slicers.iter().position(|c| c.id == id).map(|i| reorder(&mut sh.slicers, i, forward, extreme)),
            ObjKind::Shape | ObjKind::Equation => {
                sh.shapes.iter().position(|c| c.id == id && shape_is(kind, c)).map(|i| reorder(&mut sh.shapes, i, forward, extreme))
            }
            ObjKind::Control => sh.controls.iter().position(|c| c.id == id).map(|i| reorder(&mut sh.controls, i, forward, extreme)),
        };
        found.ok_or_else(|| bad("arrange", "no such object"))?;
        Ok(Json::Null)
    })
}

pub(super) fn selection_pane(s: &mut Session, _: &Json) -> Result<Json> {
    let d = s.doc()?;
    let sh = d.wb.active().ok_or(EngineError::NoDocument)?;
    let mut v = Vec::new();
    for c in &sh.shapes {
        let kind = if c.kind == ShapeKind::Equation { "equation" } else { "shape" };
        v.push(json!({"kind": kind, "id": c.id, "name": format!("{:?} {}", c.kind, c.id), "at": c.anchor.cell.a1(), "anchorMode": anchor_mode_name(c.anchor.mode)}));
    }
    for c in &sh.images {
        v.push(json!({"kind": "image", "id": c.id, "name": format!("Picture {}", c.id), "at": c.anchor.cell.a1(), "anchorMode": anchor_mode_name(c.anchor.mode)}));
    }
    for c in &sh.charts {
        v.push(json!({"kind": "chart", "id": c.id, "name": format!("Chart {}", c.id), "title": c.title, "at": c.anchor.cell.a1(), "anchorMode": anchor_mode_name(c.anchor.mode)}));
    }
    // Slicers paint above the other objects.
    for c in &sh.slicers {
        let name = if c.caption.is_empty() { &c.column } else { &c.caption };
        v.push(json!({"kind": "slicer", "id": c.id, "name": name, "at": c.anchor.cell.a1(), "anchorMode": anchor_mode_name(c.anchor.mode)}));
    }
    // Form controls paint above everything else.
    for c in &sh.controls {
        v.push(
            json!({"kind": "control", "id": c.id, "name": c.display_name(), "at": c.anchor.cell.a1(), "anchorMode": anchor_mode_name(c.anchor.mode)}),
        );
    }
    Ok(Json::Array(v))
}

/// The anchors of every object of `kind` on the sheet; under `equation`, only the equations.
fn anchors_of(sh: &mut Sheet, kind: ObjKind) -> Vec<(u32, &mut Anchor)> {
    match kind {
        ObjKind::Chart => sh.charts.iter_mut().map(|c| (c.id, &mut c.anchor)).collect(),
        ObjKind::Image => sh.images.iter_mut().map(|c| (c.id, &mut c.anchor)).collect(),
        ObjKind::Slicer => sh.slicers.iter_mut().map(|c| (c.id, &mut c.anchor)).collect(),
        ObjKind::Control => sh.controls.iter_mut().map(|c| (c.id, &mut c.anchor)).collect(),
        ObjKind::Shape | ObjKind::Equation => sh.shapes.iter_mut().filter(|c| shape_is(kind, c)).map(|c| (c.id, &mut c.anchor)).collect(),
    }
}

pub(super) fn align(s: &mut Session, p: &Json) -> Result<Json> {
    let kind = kind_param(p, "arrange.align")?;
    // `ids` may be absent (nothing to align) but, present, must be an array whose every entry is
    // an object id naming an object of the kind: a stale or mismatched id, or a value of another
    // shape, is an error, as for `arrange` and the `object.*` commands, not something silently
    // left out.
    let bad_ids = || bad("arrange.align", "ids must be an array of object ids");
    let ids: Vec<u32> = match p.get("ids") {
        None => Vec::new(),
        Some(Json::Array(a)) => a.iter().map(|v| v.as_u64().and_then(|x| u32::try_from(x).ok()).ok_or_else(bad_ids)).collect::<Result<_>>()?,
        Some(_) => return Err(bad_ids()),
    };
    let how = str_param(p, "align").unwrap_or("left").to_string();
    let si = s.doc()?.wb.active_sheet();
    crate::cmd::controls::check_objects_unlocked(s, si, kind, &ids)?;
    edit(s, |cx| {
        let sheet = cx.sheet_mut(si)?;
        let geo: Vec<(u32, f64, f64, f32, f32)> = {
            let shc = sheet.clone();
            anchors_of(sheet, kind)
                .into_iter()
                .filter(|(id, _)| ids.contains(id))
                .map(|(id, a)| (id, shc.col_left(a.cell.col) + a.dx as f64, shc.row_top(a.cell.row) + a.dy as f64, a.width, a.height))
                .collect()
        };
        if let Some(missing) = ids.iter().find(|id| !geo.iter().any(|g| g.0 == **id)) {
            return Err(bad("arrange.align", format!("no such object: {missing}")));
        }
        if geo.len() < 2 {
            return Ok(Json::Null);
        }
        let left = geo.iter().map(|g| g.1).fold(f64::INFINITY, f64::min);
        let right = geo.iter().map(|g| g.1 + g.3 as f64).fold(f64::NEG_INFINITY, f64::max);
        let top = geo.iter().map(|g| g.2).fold(f64::INFINITY, f64::min);
        let bottom = geo.iter().map(|g| g.2 + g.4 as f64).fold(f64::NEG_INFINITY, f64::max);
        let shc = sheet.clone();
        for (id, a) in anchors_of(sheet, kind) {
            let Some(g) = geo.iter().find(|g| g.0 == id) else { continue };
            let (mut x, mut y) = (g.1, g.2);
            match how.as_str() {
                "left" => x = left,
                "right" => x = right - g.3 as f64,
                "center" => x = (left + right) / 2.0 - g.3 as f64 / 2.0,
                "top" => y = top,
                "bottom" => y = bottom - g.4 as f64,
                _ => y = (top + bottom) / 2.0 - g.4 as f64 / 2.0,
            }
            let c = CellRef::new(shc.row_at(y), shc.col_at(x));
            a.cell = c;
            a.dx = (x - shc.col_left(c.col)) as f32;
            a.dy = (y - shc.row_top(c.row)) as f32;
        }
        Ok(Json::Null)
    })
}

pub(super) fn rotate(s: &mut Session, p: &Json) -> Result<Json> {
    if p.get("flip").is_some_and(|f| !f.is_null()) {
        return Err(bad("arrange.rotate", "`flip` is not supported: shapes have no flip state"));
    }
    with_shape(s, p, |sh| std::mem::swap(&mut sh.anchor.width, &mut sh.anchor.height))
}
