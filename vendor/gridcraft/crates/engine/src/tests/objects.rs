//! Floating objects of every kind: found, moved, listed, stacked and deleted by `{kind, id}`.

use gridcraft_core::CellRef;
use gridcraft_model::{Anchor, AnchorMode, ObjKind, ObjRef, Sheet};
use serde_json::json;

use super::s;
use crate::Session;

fn at(a: &str) -> CellRef {
    CellRef::parse(a).unwrap()
}

fn sheet(s: &Session, i: usize) -> &Sheet {
    s.doc().unwrap().wb.sheet(i).unwrap()
}

fn id_of(v: &serde_json::Value, key: &str) -> u32 {
    v[key].as_u64().unwrap() as u32
}

/// A table with one object of each kind on the first sheet: a chart, a picture, a shape, a
/// slicer, an equation and a form control, in `ObjKind::ALL` order.
fn every_kind(s: &mut Session) -> [ObjRef; 6] {
    s.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Qty"], ["East", 3], ["West", 5], ["East", 2], ["North", 8]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:B5", "name": "Sales"})).unwrap();
    let chart = id_of(&s.execute("insert.chart", json!({"range": "B1:B5", "type": "column", "at": "D2"})).unwrap(), "chart");
    let image = super::add_image(s, "D18", AnchorMode::MoveAndSize);
    let shape = id_of(&s.execute("insert.shape", json!({"kind": "ellipse", "at": "H18"})).unwrap(), "shape");
    s.execute("selection.set", json!({"cell": "A2"})).unwrap();
    let slicer = id_of(&s.execute("insert.slicer", json!({"column": "Region", "at": "L2"})).unwrap(), "slicer");
    let equation = id_of(&s.execute("insert.equation", json!({"latex": "x^2", "cell": "L18"})).unwrap(), "equation");
    let control = id_of(&s.execute("insert.slider", json!({"at": "P18", "link": "$B$7"})).unwrap(), "control");
    [
        ObjRef::new(ObjKind::Chart, chart),
        ObjRef::new(ObjKind::Image, image),
        ObjRef::new(ObjKind::Shape, shape),
        ObjRef::new(ObjKind::Slicer, slicer),
        ObjRef::new(ObjKind::Equation, equation),
        ObjRef::new(ObjKind::Control, control),
    ]
}

#[test]
fn every_kind_is_found_moved_and_deleted_by_kind_and_id() {
    let mut s = s();
    let objs = every_kind(&mut s);
    for o in objs {
        assert_eq!(sheet(&s, 0).object_ref(o.id), Some(o), "the id finds its kind");
        assert_eq!(ObjKind::parse(o.kind.name()), Some(o.kind));
        s.execute("object.move", json!({"kind": o.kind.name(), "id": o.id, "at": "M20", "width": 50, "height": 30})).unwrap();
        let a = sheet(&s, 0).object_anchor(o).unwrap();
        assert_eq!((a.cell, a.width, a.height), (at("M20"), 50.0, 30.0));
        s.execute("object.setAnchorMode", json!({"kind": o.kind.name(), "id": o.id, "mode": "absolute"})).unwrap();
        assert_eq!(sheet(&s, 0).object_anchor(o).unwrap().mode, AnchorMode::Absolute);
    }
    // An equation is a shape too; a plain shape is not an equation.
    assert!(sheet(&s, 0).has_object(ObjRef::new(ObjKind::Shape, objs[4].id)));
    assert!(s.execute("object.delete", json!({"kind": "equation", "id": objs[2].id})).is_err());
    // A kind that does not match the id, an unknown id or kind, and a missing kind are refused.
    assert!(s.execute("object.delete", json!({"kind": "slicer", "id": objs[0].id})).is_err());
    assert!(s.execute("object.delete", json!({"kind": "chart", "id": 9999})).is_err());
    assert!(s.execute("object.delete", json!({"kind": "widget", "id": objs[0].id})).is_err());
    assert!(s.execute("object.delete", json!({"id": objs[0].id})).is_err(), "kind is required");
    for (n, o) in objs.into_iter().enumerate() {
        let out = s.execute("object.delete", json!({"kind": o.kind.name(), "id": o.id})).unwrap();
        assert_eq!(out["deleted"], json!({"kind": o.kind.name(), "id": o.id}));
        assert!(!sheet(&s, 0).has_object(o));
        for later in &objs[n + 1..] {
            assert!(sheet(&s, 0).has_object(*later), "deleting {o:?} leaves {later:?}");
        }
    }
    // One undo step per delete brings the control, deleted last, back.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(sheet(&s, 0).has_object(objs[5]));
}

#[test]
fn selection_pane_lists_and_stacks_slicers() {
    let mut s = s();
    let objs = every_kind(&mut s);
    let second = id_of(&s.execute("insert.slicer", json!({"column": "Qty", "at": "P2"})).unwrap(), "slicer");
    let list = s.execute("arrange.selectionPane", json!({})).unwrap();
    let items = list.as_array().unwrap();
    let kinds: Vec<&str> = items.iter().map(|o| o["kind"].as_str().unwrap()).collect();
    for k in ObjKind::ALL {
        assert!(kinds.contains(&k.name()), "{k:?} listed in {kinds:?}");
    }
    assert_eq!(kinds.last(), Some(&"control"), "form controls stack on top");
    assert_eq!(kinds.get(kinds.len() - 2), Some(&"slicer"), "slicers stack above charts, pictures and shapes");
    let names: Vec<&str> = items.iter().map(|o| o["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Region") && names.contains(&"Qty"), "{names:?}");
    assert!(names.contains(&format!("Slider {}", objs[5].id).as_str()), "{names:?}");
    assert_eq!(sheet(&s, 0).slicers.first().map(|c| c.id), Some(objs[3].id));
    s.execute("arrange.sendBackward", json!({"kind": "slicer", "id": second, "toBack": true})).unwrap();
    assert_eq!(sheet(&s, 0).slicers.first().map(|c| c.id), Some(second));
}

#[test]
fn hostile_object_params_are_refused() {
    let mut s = s();
    let objs = every_kind(&mut s);
    for p in [
        json!({}),
        json!({"kind": "chart"}),
        json!({"kind": "chart", "id": -1}),
        json!({"kind": "chart", "id": u64::MAX}),
        json!({"kind": 7, "id": objs[0].id}),
        json!({"kind": null, "id": objs[0].id}),
    ] {
        for cmd in ["object.delete", "object.move", "object.setAnchorMode"] {
            assert!(s.execute(cmd, p.clone()).is_err(), "{cmd} {p}");
        }
    }
    let o = objs[3];
    s.execute("object.move", json!({"kind": o.kind.name(), "id": o.id, "dx": 1e300, "dy": -1e300, "width": 1e300, "height": -5})).unwrap();
    let a = sheet(&s, 0).object_anchor(o).unwrap();
    assert!(a.dx.is_finite() && a.dy.is_finite() && a.width <= 10000.0 && a.height >= 0.0, "{a:?}");
}

fn cell_of(s: &Session, o: ObjRef) -> String {
    sheet(s, 0).object_anchor(o).unwrap().cell.a1()
}

#[test]
fn every_kind_follows_row_and_column_edits_and_sorts() {
    let mut s = s();
    let objs = every_kind(&mut s);
    // Anchors: chart D2, picture D18, shape H18, slicer L2, equation L18, slider P18.
    s.execute("object.setAnchorMode", json!({"kind": "shape", "id": objs[2].id, "mode": "absolute"})).unwrap();
    let cells = |s: &Session| objs.map(|o| cell_of(s, o));
    assert_eq!(cells(&s), ["D2", "D18", "H18", "L2", "L18", "P18"]);
    s.execute("home.insertRows", json!({"rows": "10:11"})).unwrap();
    assert_eq!(cells(&s), ["D2", "D20", "H18", "L2", "L20", "P20"], "rows inserted above move the objects below them; an absolute shape stays");
    s.execute("home.insertColumns", json!({"cols": "C:C"})).unwrap();
    assert_eq!(cells(&s), ["E2", "E20", "H18", "M2", "M20", "Q20"]);
    s.execute("home.deleteColumns", json!({"cols": "C:C"})).unwrap();
    s.execute("home.deleteRows", json!({"rows": "10:11"})).unwrap();
    assert_eq!(cells(&s), ["D2", "D18", "H18", "L2", "L18", "P18"]);
    // Sorting moves a picture anchored inside the sorted block with its row.
    s.execute("range.setValues", json!({"range": "T1", "values": [["k"], [3], [1], [2]]})).unwrap();
    let pic = ObjRef::new(ObjKind::Image, super::add_image(&mut s, "T2", AnchorMode::MoveAndSize));
    s.execute("selection.set", json!({"range": "T1:T4"})).unwrap();
    s.execute("data.sortAscending", json!({"header": true, "column": "T"})).unwrap();
    assert_eq!(cell_of(&s, pic), "T4", "the row holding 3 sorts last");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(cell_of(&s, pic), "T2");
}

/// A picture at `cell` with anchor `mode`, offset `(dx, dy)` and size `(width, height)`, added
/// straight to the model like `add_image`, so a sort that follows is the only step to undo.
fn picture(s: &mut Session, cell: &str, mode: AnchorMode, dx: f32, dy: f32, width: f32, height: f32) -> ObjRef {
    let id = super::add_image(s, cell, mode);
    let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
    if let Some(img) = wb.sheet_mut(0).unwrap().images.iter_mut().find(|i| i.id == id) {
        img.anchor = Anchor { dx, dy, width, height, ..img.anchor };
    }
    ObjRef::new(ObjKind::Image, id)
}

/// The anchor of `o` as (cell, dx, dy, width, height).
fn anchor_of(s: &Session, o: ObjRef) -> (String, f32, f32, f32, f32) {
    let a = sheet(s, 0).object_anchor(o).unwrap();
    (a.cell.a1(), a.dx, a.dy, a.width, a.height)
}

/// Sorting rows under a picture follows its anchor mode, as Excel's "Move and size with cells",
/// "Move but don't size with cells" and "Don't move or size with cells" do.
#[test]
fn sorting_rows_under_a_picture_follows_its_anchor_mode() {
    let mut s = s();
    // Rows 2..5 hold 3, 1, 4, 2; sorted ascending they read 1, 2, 3, 4, so row 2 moves to row 4,
    // row 3 to row 2, row 4 to row 5 and row 5 to row 3. Default rows are 20 pt tall.
    s.execute("range.setValues", json!({"range": "A1", "values": [["k"], [3], [1], [4], [2]]})).unwrap();
    // Three pictures inside one row each (3 pt down, 12 pt tall), one per anchor mode.
    let sized = picture(&mut s, "A2", AnchorMode::MoveAndSize, 5.0, 3.0, 30.0, 12.0);
    let moved = picture(&mut s, "A3", AnchorMode::MoveOnly, 5.0, 3.0, 30.0, 12.0);
    let pinned = picture(&mut s, "A4", AnchorMode::Absolute, 5.0, 3.0, 30.0, 12.0);
    // A "move and size" picture spanning rows 2 and 3: its top is 10 pt into row 2 (y 30) and
    // its bottom 15 pt into row 3 (y 55). The sort puts row 3 above row 2.
    let tall = picture(&mut s, "A2", AnchorMode::MoveAndSize, 5.0, 10.0, 30.0, 25.0);
    let before: Vec<_> = [sized, moved, pinned, tall].iter().map(|&o| anchor_of(&s, o)).collect();
    s.execute("selection.set", json!({"range": "A1:A5"})).unwrap();
    s.execute("data.sortAscending", json!({"header": true, "column": "A"})).unwrap();
    assert_eq!(
        anchor_of(&s, sized),
        ("A4".into(), 5.0, 3.0, 30.0, 12.0),
        "move and size: travels with row 2 to row 4, re-fitted to the same 20 pt row"
    );
    assert_eq!(anchor_of(&s, moved), ("A2".into(), 5.0, 3.0, 30.0, 12.0), "move only: travels with row 3 to row 2 at its own size");
    assert_eq!(anchor_of(&s, pinned), ("A4".into(), 5.0, 3.0, 30.0, 12.0), "absolute: stays while the cells move under it");
    // The tall picture's bottom row now sorts above its top row, so there is nothing to stretch
    // it between: it travels with its top-left row and keeps its size instead of collapsing.
    assert_eq!(anchor_of(&s, tall), ("A4".into(), 5.0, 10.0, 30.0, 25.0), "two rows swapped: moves with its top row at its old size");
    s.execute("edit.undo", json!({})).unwrap();
    let after: Vec<_> = [sized, moved, pinned, tall].iter().map(|&o| anchor_of(&s, o)).collect();
    assert_eq!(after, before, "undo restores every anchor");
}

/// A sort by columns follows the anchor mode the same way.
#[test]
fn sorting_columns_under_a_picture_follows_its_anchor_mode() {
    let mut s = s();
    // Row 10 holds a header in A and 3, 1, 2 in B..D; sorted ascending, column B moves to D,
    // C to B and D to C. Default columns are 64 pt wide.
    s.execute("range.setValues", json!({"range": "A10", "values": [["k", 3, 1, 2]]})).unwrap();
    let sized = picture(&mut s, "B10", AnchorMode::MoveAndSize, 5.0, 3.0, 30.0, 12.0);
    let moved = picture(&mut s, "C10", AnchorMode::MoveOnly, 5.0, 3.0, 30.0, 12.0);
    let pinned = picture(&mut s, "D10", AnchorMode::Absolute, 5.0, 3.0, 30.0, 12.0);
    // A "move and size" picture spanning columns B and C: from x 104 (40 pt into B) to x 154
    // (26 pt into C). The sort puts column C before column B.
    let wide = picture(&mut s, "B10", AnchorMode::MoveAndSize, 40.0, 3.0, 50.0, 12.0);
    let before: Vec<_> = [sized, moved, pinned, wide].iter().map(|&o| anchor_of(&s, o)).collect();
    s.execute("data.sort", json!({"range": "A10:D10", "header": true, "orientation": "columns", "keys": [{"row": 10}]})).unwrap();
    assert_eq!(anchor_of(&s, sized), ("D10".into(), 5.0, 3.0, 30.0, 12.0), "move and size: travels with column B to D at the same width");
    assert_eq!(anchor_of(&s, moved), ("B10".into(), 5.0, 3.0, 30.0, 12.0), "move only: travels with column C to B at its own size");
    assert_eq!(anchor_of(&s, pinned), ("D10".into(), 5.0, 3.0, 30.0, 12.0), "absolute: stays");
    assert_eq!(anchor_of(&s, wide), ("D10".into(), 40.0, 3.0, 50.0, 12.0), "two columns swapped: moves with its left column at its old size");
    s.execute("edit.undo", json!({})).unwrap();
    let after: Vec<_> = [sized, moved, pinned, wide].iter().map(|&o| anchor_of(&s, o)).collect();
    assert_eq!(after, before, "undo restores every anchor");
}

#[test]
fn slicers_follow_inserted_rows_like_other_objects() {
    let mut s = s();
    let objs = every_kind(&mut s);
    s.execute("home.insertRows", json!({"rows": "1:2"})).unwrap();
    assert_eq!(cell_of(&s, objs[3]), "L4");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(cell_of(&s, objs[3]), "L2");
}

#[test]
fn chart_series_follow_cut_moves_renames_and_sheet_deletes() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[3], [5], [2], [8]]})).unwrap();
    let chart = id_of(&s.execute("insert.chart", json!({"range": "A1:A4", "type": "column"})).unwrap(), "chart");
    let values = |s: &Session| sheet(s, 0).charts.iter().find(|c| c.id == chart).map(|c| c.series[0].values.clone()).unwrap_or_default();
    s.execute("edit.cut", json!({"range": "A1:A4"})).unwrap();
    s.execute("edit.paste", json!({"at": "F2"})).unwrap();
    assert!(values(&s).contains("$F$2:$F$5"), "a cut-move takes the series with the cells: {}", values(&s));
    s.execute("home.insertSheet", json!({"name": "Other"})).unwrap();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3]]})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().charts[0].series[0].values = "Other!$A$1:$A$3".into();
    s.execute("sheet.rename", json!({"sheet": 1, "name": "Inputs"})).unwrap();
    assert_eq!(values(&s), "Inputs!$A$1:$A$3");
    s.execute("home.deleteSheet", json!({"sheet": 1})).unwrap();
    assert!(values(&s).contains("#REF!"), "a series into a deleted sheet becomes #REF!: {}", values(&s));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(values(&s), "Inputs!$A$1:$A$3");
}

/// A series name that is a bare word is a label, as a chart reads it: renaming a table of that
/// name leaves it be, while a series named by a defined name follows the name's rename.
#[test]
fn chart_series_names_follow_names_but_not_labels() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [["v"], [5], [2]]})).unwrap();
    s.execute("insert.table", json!({"range": "A1:A3", "name": "Sales"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Title", "refersTo": "=Sheet1!$A$1"})).unwrap();
    let chart = id_of(&s.execute("insert.chart", json!({"range": "A2:A3", "type": "column"})).unwrap(), "chart");
    let set = |s: &mut Session, label: &str, named: &str| {
        let ch = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().charts.iter_mut().find(|c| c.id == chart).unwrap();
        ch.series.truncate(1);
        let mut second = ch.series[0].clone();
        ch.series[0].name = Some(label.into());
        second.name = Some(named.into());
        ch.series.push(second);
    };
    let names = |s: &Session| -> Vec<String> {
        sheet(s, 0).charts.iter().find(|c| c.id == chart).map(|c| c.series.iter().filter_map(|se| se.name.clone()).collect()).unwrap_or_default()
    };
    // A name another sheet holds isn't one this chart sees.
    s.execute("home.insertSheet", json!({"name": "Two"})).unwrap();
    s.execute("formulas.defineName", json!({"name": "Sales", "refersTo": "=Two!$A$1", "scope": "Two"})).unwrap();
    set(&mut s, "Sales", "Title");
    s.execute("table.rename", json!({"table": "Sales", "name": "Revenue"})).unwrap();
    assert_eq!(names(&s), ["Sales", "Title"], "a label isn't the table");
    s.execute("formulas.defineName", json!({"name": "Heading", "refersTo": "=Sheet1!$A$1", "oldName": "Title"})).unwrap();
    assert_eq!(names(&s), ["Sales", "Heading"], "a defined name follows its rename");
    // A label shaped like a cell is a label too.
    set(&mut s, "Q1", "Sheet1!$A$1");
    s.execute("home.insertRows", json!({"rows": "1:1", "sheet": "Sheet1"})).unwrap();
    assert_eq!(names(&s), ["Q1", "Sheet1!$A$2"]);
}

/// A sheet may be named `#REF!`: a chart's source on it is not taken for one that's gone.
#[test]
fn a_chart_source_on_a_sheet_named_ref_error_is_kept() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[3], [5]]})).unwrap();
    s.execute("insert.chart", json!({"range": "A1:A2", "type": "column"})).unwrap();
    s.execute("sheet.rename", json!({"sheet": 0, "name": "#REF!"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Other"})).unwrap();
    s.execute("sheet.rename", json!({"sheet": "Other", "name": "Else"})).unwrap();
    let source = sheet(&s, s.doc().unwrap().wb.sheet_index("#REF!").unwrap()).charts[0].source.clone();
    assert_eq!(source.as_deref(), Some("'#REF!'!A1:A2"));
}

#[test]
fn a_copied_sheet_gives_its_objects_fresh_ids() {
    let mut s = s();
    let objs = every_kind(&mut s);
    s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let ids_of = |sh: &Sheet| -> Vec<u32> {
        sh.charts
            .iter()
            .map(|c| c.id)
            .chain(sh.images.iter().map(|c| c.id))
            .chain(sh.shapes.iter().map(|c| c.id))
            .chain(sh.slicers.iter().map(|c| c.id))
            .chain(sh.controls.iter().map(|c| c.id))
            .collect()
    };
    let mut ids: Vec<u32> = wb.sheets.iter().flat_map(|sh| ids_of(sh)).collect();
    assert_eq!(ids.len(), 2 * objs.len());
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 2 * objs.len(), "every object id stays unique across the workbook");
    let copy = sheet(&s, 1);
    let mut kinds: Vec<&str> = ids_of(copy).into_iter().filter_map(|id| copy.object_ref(id)).map(|o| o.kind.name()).collect();
    let mut want: Vec<&str> = objs.iter().map(|o| o.kind.name()).collect();
    kinds.sort_unstable();
    want.sort_unstable();
    assert_eq!(kinds, want, "the copies keep their kinds");
    assert!(s.doc().unwrap().wb.next_object_id() > *ids.last().unwrap_or(&0));
}

#[test]
fn bubble_sizes_follow_edits_like_the_other_series_references() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1, 3, 10], [2, 5, 20], [3, 2, 30]]})).unwrap();
    let chart = id_of(&s.execute("insert.chart", json!({"range": "A1:B3", "type": "bubble"})).unwrap(), "chart");
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().charts.iter_mut().find(|c| c.id == chart).unwrap().series[0]
        .bubble_sizes = Some("Sheet1!$C$1:$C$3".into());
    let sizes = |s: &Session| sheet(s, 0).charts.iter().find(|c| c.id == chart).and_then(|c| c.series[0].bubble_sizes.clone()).unwrap_or_default();
    s.execute("home.insertRows", json!({"rows": "1:1"})).unwrap();
    assert_eq!(sizes(&s), "Sheet1!$C$2:$C$4", "an inserted row moves the bubble sizes with their cells");
    s.execute("edit.cut", json!({"range": "C2:C4"})).unwrap();
    s.execute("edit.paste", json!({"at": "F2"})).unwrap();
    assert_eq!(sizes(&s), "Sheet1!$F$2:$F$4", "a cut-move takes the bubble sizes with the cells");
    s.execute("sheet.rename", json!({"sheet": 0, "name": "Data"})).unwrap();
    assert_eq!(sizes(&s), "Data!$F$2:$F$4", "a renamed sheet is renamed in the bubble sizes");
}

/// A file can carry an object with the largest id. New objects and a copied sheet's objects then
/// take free ids instead of the largest one again, so `{kind, id}` still names one object.
#[test]
fn object_ids_stay_unique_when_a_file_used_the_largest_id() {
    let mut s = s();
    let first = id_of(&s.execute("insert.shape", json!({"kind": "rectangle", "at": "B2"})).unwrap(), "shape");
    // The file's shape carries the largest id (set on the model, as a loaded file would).
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().shapes.iter_mut().find(|o| o.id == first).unwrap().id = u32::MAX;
    let second = id_of(&s.execute("insert.shape", json!({"kind": "ellipse", "at": "D2"})).unwrap(), "shape");
    let third = id_of(&s.execute("insert.shape", json!({"kind": "triangle", "at": "F2"})).unwrap(), "shape");
    assert!(second != u32::MAX && third != u32::MAX && second != third, "new shapes take free ids, not the largest again: {second}, {third}");
    s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let mut ids: Vec<u32> = wb.sheets.iter().flat_map(|sh| sh.shapes.iter().map(|o| o.id)).collect();
    assert_eq!(ids.len(), 6);
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 6, "every shape id is distinct across both sheets");
}

/// A copied sheet's form controls take free ids like its other objects: a file's control can
/// carry the largest id, and the ids the copy asks for count the controls.
#[test]
fn a_copied_sheets_form_controls_take_free_ids_too() {
    let mut s = s();
    let slider = id_of(&s.execute("insert.slider", json!({"at": "B2", "link": "$A$1"})).unwrap(), "control");
    s.execute("insert.button", json!({"at": "B6"})).unwrap();
    s.execute("insert.shape", json!({"kind": "rectangle", "at": "D2"})).unwrap();
    // The file's slider carries the largest id (set on the model, as a loaded file would).
    std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb).sheet_mut(0).unwrap().controls.iter_mut().find(|o| o.id == slider).unwrap().id = u32::MAX;
    s.execute("sheet.move", json!({"sheet": 0, "to": 1, "copy": true})).unwrap();
    let wb = &s.doc().unwrap().wb;
    let mut ids: Vec<u32> = wb.sheets.iter().flat_map(|sh| sh.controls.iter().map(|o| o.id).chain(sh.shapes.iter().map(|o| o.id))).collect();
    assert_eq!(ids.len(), 6);
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 6, "every control and shape id is distinct across both sheets: {ids:?}");
    assert_eq!(ids.iter().filter(|&&i| i == u32::MAX).count(), 1, "only the file's slider keeps the largest id");
}

/// A file's anchors are not held to the 10,000 points `object.move` gives a new size. A picture
/// a file made 12,000 wide keeps that width when it is moved, shrinks by what a resize asks
/// (11,990 stays 11,990, no jump to 10,000) and never grows further; a dimension the file kept
/// within the limit is still capped there.
#[test]
fn an_oversized_imported_object_keeps_its_size_when_moved_and_shrinks_only_by_a_resize() {
    let mut s = s();
    let image = super::add_image(&mut s, "B2", AnchorMode::MoveOnly);
    // The file's picture is 12,000 by 300: set on the model, then written and read back as a file.
    {
        let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
        let a = wb.sheet_mut(0).unwrap().object_anchor_mut(ObjRef::new(ObjKind::Image, image)).unwrap();
        a.width = 12_000.0;
        a.height = 300.0;
    }
    let saved = s.execute("file.saveBytes", json!({"format": "xlsx"})).unwrap();
    s.execute("file.open", json!({"name": "wide.xlsx", "base64": saved["base64"]})).unwrap();
    let o = ObjRef::new(ObjKind::Image, sheet(&s, 0).images[0].id);
    let size = |s: &Session| {
        let a = sheet(s, 0).object_anchor(o).unwrap();
        (a.width.round(), a.height.round())
    };
    assert_eq!(size(&s), (12_000.0, 300.0), "the reader keeps the file's size");
    s.execute("object.move", json!({"kind": "image", "id": o.id, "at": "D5", "dx": 3, "dy": 4})).unwrap();
    let a = sheet(&s, 0).object_anchor(o).unwrap();
    assert_eq!((a.cell, a.dx, a.dy), (at("D5"), 3.0, 4.0));
    assert_eq!(size(&s), (12_000.0, 300.0), "a move without a size keeps the file's size");
    s.execute("object.move", json!({"kind": "image", "id": o.id, "width": 12_010})).unwrap();
    assert_eq!(size(&s), (12_000.0, 300.0), "the object does not grow past what the file gave");
    s.execute("object.move", json!({"kind": "image", "id": o.id, "width": 11_990})).unwrap();
    assert_eq!(size(&s), (11_990.0, 300.0), "a resize shrinks by what it asks, with no jump to 10,000");
    s.execute("object.move", json!({"kind": "image", "id": o.id, "width": 50_000, "height": 50_000})).unwrap();
    assert_eq!(size(&s), (11_990.0, 10_000.0), "hostile sizes are capped: at the current width, and at 10,000 for a height the file kept within it");
}

/// `chart.set` follows the same rule: the Format Chart pane's Size fields send the unchanged
/// dimension back with the edited one, so a chart a file made 12,000 wide keeps that width when
/// its height is edited, shrinks by what is asked and never grows further.
#[test]
fn chart_set_keeps_a_chart_size_a_file_made_larger_than_the_limit() {
    let mut s = s();
    s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [3]]})).unwrap();
    let chart = id_of(&s.execute("insert.chart", json!({"range": "A1:A3", "type": "column", "at": "C2"})).unwrap(), "chart");
    // The file's chart is 12,000 wide (set on the model, as a loaded file would).
    let wb = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().wb);
    wb.sheet_mut(0).unwrap().charts.iter_mut().find(|c| c.id == chart).unwrap().anchor.width = 12_000.0;
    let size = |s: &Session| {
        let c = sheet(s, 0).charts.iter().find(|c| c.id == chart).unwrap();
        (c.anchor.width, c.anchor.height)
    };
    s.execute("chart.set", json!({"chart": chart, "width": 12_000, "height": 250})).unwrap();
    assert_eq!(size(&s), (12_000.0, 250.0), "the unchanged width sent back with a new height stays");
    s.execute("chart.set", json!({"chart": chart, "width": 11_990})).unwrap();
    assert_eq!(size(&s), (11_990.0, 250.0), "a resize shrinks by what it asks");
    s.execute("chart.set", json!({"chart": chart, "width": 50_000, "height": 50_000})).unwrap();
    assert_eq!(size(&s), (11_990.0, 10_000.0), "hostile sizes are capped at the current width and at 10,000");
}

#[test]
fn arrange_and_align_match_a_shape_id_to_its_kind() {
    // `arrange.*` and `arrange.align` under `kind: "equation"` act on equations only: an
    // ordinary shape's id given as an equation is refused by `arrange` and left in place by
    // `align`, as `object.delete` refuses it. An equation is a shape too, so its id works under
    // `shape`, as for the other object commands; an unknown kind, and an id of no object of the
    // kind, are refused instead of acting on whatever shape has the id.
    let mut s = s();
    let objs = every_kind(&mut s);
    let (shape, equation) = (objs[2], objs[4]);
    let order = |s: &Session| -> Vec<u32> { sheet(s, 0).shapes.iter().map(|c| c.id).collect() };
    assert_eq!(order(&s), vec![shape.id, equation.id]);
    assert!(s.execute("arrange.bringForward", json!({"kind": "equation", "id": shape.id, "toFront": true})).is_err());
    assert_eq!(order(&s), vec![shape.id, equation.id], "the shape is not reordered as an equation");
    s.execute("arrange.sendBackward", json!({"kind": "equation", "id": equation.id, "toBack": true})).unwrap();
    assert_eq!(order(&s), vec![equation.id, shape.id]);
    s.execute("arrange.bringForward", json!({"kind": "shape", "id": equation.id, "toFront": true})).unwrap();
    assert_eq!(order(&s), vec![shape.id, equation.id], "an equation is a shape too");
    assert!(s.execute("arrange.bringForward", json!({"kind": "widget", "id": shape.id})).is_err());
    assert!(s.execute("arrange.bringForward", json!({"kind": "chart", "id": 9999})).is_err());
    assert!(s.execute("arrange.bringForward", json!({"kind": "shape", "id": objs[0].id})).is_err(), "a chart's id is not a shape");
    // Align: the shape's id given as an equation is refused and nothing moves; the two equations
    // line up on their own.
    let second = ObjRef::new(ObjKind::Equation, id_of(&s.execute("insert.equation", json!({"latex": "y", "cell": "B30"})).unwrap(), "equation"));
    let before = (anchor_of(&s, shape), anchor_of(&s, equation));
    assert!(s.execute("arrange.align", json!({"kind": "equation", "ids": [shape.id, equation.id, second.id], "align": "left"})).is_err());
    assert_eq!((anchor_of(&s, shape), anchor_of(&s, equation)), before, "nothing is aligned with a refused id");
    s.execute("arrange.align", json!({"kind": "equation", "ids": [equation.id, second.id], "align": "left"})).unwrap();
    let (a, b) = (sheet(&s, 0).object_anchor(equation).unwrap(), sheet(&s, 0).object_anchor(second).unwrap());
    assert_eq!((a.cell.col, a.dx), (b.cell.col, b.dx), "the equations line up");
    assert!(s.execute("arrange.align", json!({"kind": "widget", "ids": [shape.id, equation.id], "align": "left"})).is_err());
}

#[test]
fn arrange_refuses_a_kind_that_is_present_but_not_a_name() {
    // `kind` is `shape` only when the parameter is absent: `null`, a number or another non-name
    // is refused, so a shape's id is not reordered or aligned under it. The `object.*` commands
    // refuse the same values, their kind being required.
    let mut s = s();
    let objs = every_kind(&mut s);
    let (shape, equation) = (objs[2], objs[4]);
    let order = |s: &Session| -> Vec<u32> { sheet(s, 0).shapes.iter().map(|c| c.id).collect() };
    let before = (order(&s), anchor_of(&s, shape));
    for kind in [json!(null), json!(7), json!(["shape"]), json!({"kind": "shape"}), json!(true)] {
        assert!(s.execute("arrange.bringForward", json!({"kind": kind, "id": shape.id, "toFront": true})).is_err(), "{kind}");
        assert!(s.execute("arrange.align", json!({"kind": kind, "ids": [shape.id, equation.id], "align": "left"})).is_err(), "{kind}");
        assert!(s.execute("object.move", json!({"kind": kind, "id": shape.id, "at": "A1"})).is_err(), "{kind}");
    }
    assert_eq!((order(&s), anchor_of(&s, shape)), before, "nothing moved");
    // Absent, the kind is `shape`, as before.
    s.execute("arrange.bringForward", json!({"id": shape.id, "toFront": true})).unwrap();
    assert_eq!(order(&s).last(), Some(&shape.id));
}

#[test]
fn align_refuses_an_id_that_is_not_an_object_of_the_kind() {
    // `arrange.align` and `arrange.group` refuse an id in `ids` that is not an object of the
    // kind, or not a number, as `arrange` and the `object.*` commands refuse it, instead of
    // aligning the rest and reporting success; nothing moves. With fewer than two ids they
    // change nothing and succeed.
    let mut s = s();
    let objs = every_kind(&mut s);
    let (shape, equation) = (objs[2], objs[4]);
    let second = ObjRef::new(ObjKind::Equation, id_of(&s.execute("insert.equation", json!({"latex": "y", "cell": "B30"})).unwrap(), "equation"));
    let before = (anchor_of(&s, shape), anchor_of(&s, equation), anchor_of(&s, second));
    for ids in [
        json!([shape.id, equation.id, second.id]),
        json!([equation.id, second.id, 9999]),
        json!([equation.id, second.id, -1]),
        json!([equation.id, second.id, "x"]),
        json!([equation.id, second.id, 1e12]),
        json!([equation.id, second.id, null]),
    ] {
        assert!(s.execute("arrange.align", json!({"kind": "equation", "ids": ids, "align": "left"})).is_err(), "{ids}");
        assert!(s.execute("arrange.group", json!({"kind": "equation", "ids": ids})).is_err(), "{ids}");
    }
    assert_eq!((anchor_of(&s, shape), anchor_of(&s, equation), anchor_of(&s, second)), before, "nothing moved");
    s.execute("arrange.align", json!({"kind": "equation", "ids": [equation.id], "align": "left"})).unwrap();
    s.execute("arrange.align", json!({"kind": "equation", "align": "left"})).unwrap();
    assert_eq!(anchor_of(&s, equation), before.1, "fewer than two ids change nothing");
    s.execute("arrange.align", json!({"kind": "equation", "ids": [equation.id, second.id], "align": "left"})).unwrap();
    let (a, b) = (sheet(&s, 0).object_anchor(equation).unwrap(), sheet(&s, 0).object_anchor(second).unwrap());
    assert_eq!((a.cell.col, a.dx), (b.cell.col, b.dx), "the equations line up");
}

#[test]
fn align_refuses_ids_that_are_not_an_array() {
    // `ids` may be absent (nothing to align) but, present, must be an array of object ids: `null`,
    // a number, a string or an object is refused by `arrange.align` and `arrange.group`, and
    // nothing moves.
    let mut s = s();
    let objs = every_kind(&mut s);
    let (shape, equation) = (objs[2], objs[4]);
    let before = (anchor_of(&s, shape), anchor_of(&s, equation));
    for ids in [json!(null), json!(7), json!("x"), json!({"ids": [1]})] {
        assert!(s.execute("arrange.align", json!({"kind": "shape", "ids": ids, "align": "left"})).is_err(), "{ids}");
        assert!(s.execute("arrange.group", json!({"kind": "shape", "ids": ids})).is_err(), "{ids}");
    }
    s.execute("arrange.align", json!({"kind": "shape", "align": "left"})).unwrap();
    s.execute("arrange.group", json!({"kind": "shape"})).unwrap();
    assert_eq!((anchor_of(&s, shape), anchor_of(&s, equation)), before, "nothing moved");
}
