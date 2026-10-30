#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::tests::{Occ, PLAIN, S_RED, base, graphics, part_with, plate, ref_base, single};
use crate::PrcError;
use crate::bits::BitReader;
use crate::entity::Ctx;
use crate::testw::W;
use crate::{Schema, Tessellation};

/// Read after each entity, to show it was read to its last bit and no further.
const SENTINEL: u32 = 0x5A5A;

/// Runs `read` over `w` then the sentinel, at authoring version `version`.
fn read_at(w: &W, version: u32, read: impl FnOnce(&mut Ctx<'_, '_>) -> Result<(), PrcError>) -> Result<(), PrcError> {
    let mut all = W::default();
    all.append(w).uint(SENTINEL);
    let bytes = all.bytes();
    let schema = Schema::default();
    let mut ctx = Ctx::new(BitReader::new(&bytes), &schema, version);
    read(&mut ctx)?;
    assert_eq!(ctx.r.unsigned_integer()?, SENTINEL, "read past the wrong number of bits");
    Ok(())
}

fn read(w: &W, read: impl FnOnce(&mut Ctx<'_, '_>) -> Result<(), PrcError>) -> Result<(), PrcError> {
    read_at(w, 8137, read)
}

/// Each truncation of `w` is an error, never a panic.
fn truncations_fail(w: &W, read: impl Fn(&mut Ctx<'_, '_>) -> Result<(), PrcError>) {
    let bytes = w.bytes();
    let schema = Schema::default();
    for end in 0..bytes.len().saturating_sub(1) {
        let mut ctx = Ctx::new(BitReader::new(&bytes[..end]), &schema, 8137);
        assert!(read(&mut ctx).is_err(), "cut at {end}");
    }
}

fn reference(w: &mut W, uid: u32) {
    w.uint(205).uint(237).bit(true).uint(uid);
}

fn markup_data(linked: &[W], leaders: &[W], markups: &[W], annotations: &[W]) -> W {
    let mut w = W::default();
    for list in [linked, leaders, markups, annotations] {
        w.uint(u32::try_from(list.len()).unwrap());
        for e in list {
            w.append(e);
        }
    }
    w
}

fn linked_item() -> W {
    let mut w = W::default();
    w.uint(204);
    graphics(&mut w, None, 1, PLAIN);
    w.uint(0).bit(true);
    reference(&mut w, 7);
    w.bit(true).uint(206).uint(149).bit(true).bit(true).uint(1).uint(1).uint(1).uint(0);
    w.bit(true).bit(false).bit(true).bit(false).uint(0);
    w
}

fn leader() -> W {
    let mut w = W::default();
    w.uint(503);
    graphics(&mut w, None, 2, PLAIN);
    w.bit(true);
    reference(&mut w, 3);
    w.bit(false).uint(4).uint(0);
    w
}

fn markup() -> W {
    let mut w = W::default();
    w.uint(502);
    graphics(&mut w, Some("Note"), 3, PLAIN);
    w.uint(1).uint(2).uint(1);
    reference(&mut w, 7);
    w.uint(2);
    reference(&mut w, 2);
    reference(&mut w, 2);
    w.uint(3).uint(0);
    w
}

fn annotation(t: u32, inner: impl FnOnce(&mut W)) -> W {
    let mut w = W::default();
    w.uint(t);
    graphics(&mut w, None, 4, PLAIN);
    inner(&mut w);
    w.uint(0);
    w
}

fn annotation_item() -> W {
    annotation(504, |w| reference(w, 3))
}

fn annotation_set(members: &[W]) -> W {
    annotation(505, |w| {
        w.uint(u32::try_from(members.len()).unwrap());
        for m in members {
            w.append(m);
        }
    })
}

fn annotation_reference() -> W {
    annotation(506, |w| {
        w.uint(1);
        reference(w, 3);
    })
}

fn plane(w: &mut W) {
    w.uint(86).bit(true);
    base(w, Some("Front"));
    w.uint(9).uint(0).put(0x01, 8);
    for v in [1., 2., 3., 0., 0., 1., 1., 1., 1., 0., 0.] {
        w.double(v);
    }
}

fn light(t: u32) -> W {
    let mut w = W::default();
    w.uint(t);
    ref_base(&mut w, None, 5);
    w.uint(1).uint(2).uint(3);
    let doubles = match t {
        732 => 6,
        733 => 4,
        734 => 11,
        _ => 0,
    };
    for _ in 0..doubles {
        w.double(0.5);
    }
    w
}

/// A scene with `lights` and `planes` clipping planes; `is_absolute` is written from 8137.
fn scene(lights: &[W], planes: usize, version: u32) -> W {
    let mut w = W::default();
    w.uint(741);
    ref_base(&mut w, None, 6);
    w.bit(true).uint(u32::try_from(lights.len()).unwrap());
    for l in lights {
        w.append(l);
    }
    w.bit(false).bit(false).uint(u32::try_from(planes).unwrap());
    for _ in 0..planes {
        plane(&mut w);
    }
    w.uint(0).uint(0).uint(0);
    if version >= 8137 {
        w.bit(true);
    }
    w
}

fn filter(w: &mut W) {
    w.uint(320);
    ref_base(w, None, 8);
    w.bit(true).bit(true).uint(0).bit(true).uint(0).uint(0);
}

fn view(version: u32) -> W {
    let mut w = W::default();
    w.uint(501);
    graphics(&mut w, Some("Top"), 7, PLAIN);
    w.uint(1);
    reference(&mut w, 3);
    plane(&mut w);
    w.bit(true).append(&scene(&[light(731)], 1, version));
    if version >= 8137 {
        w.bit(true).bit(false).bit(true).uint(1);
        reference(&mut w, 7);
        w.uint(1);
        filter(&mut w);
    }
    w.uint(0);
    w
}

fn views(vs: &[W]) -> W {
    let mut w = W::default();
    w.uint(u32::try_from(vs.len()).unwrap());
    for v in vs {
        w.append(v);
    }
    w
}

#[test]
fn a_markup_linked_item_is_read_past() {
    let w = markup_data(&[linked_item()], &[], &[], &[]);
    read(&w, |c| c.markup_data()).unwrap();
    truncations_fail(&w, |c| c.markup_data());
}

#[test]
fn a_leader_is_read_past() {
    let w = markup_data(&[], &[leader()], &[], &[]);
    read(&w, |c| c.markup_data()).unwrap();
    truncations_fail(&w, |c| c.markup_data());
}

#[test]
fn a_markup_is_read_past() {
    let w = markup_data(&[], &[], &[markup()], &[]);
    read(&w, |c| c.markup_data()).unwrap();
    truncations_fail(&w, |c| c.markup_data());
}

#[test]
fn annotation_items_sets_and_references_are_read_past() {
    let set = annotation_set(&[annotation_item(), annotation_set(&[annotation_reference()])]);
    let w = markup_data(&[], &[], &[], &[annotation_item(), set, annotation_reference()]);
    read(&w, |c| c.markup_data()).unwrap();
    truncations_fail(&w, |c| c.markup_data());
}

#[test]
fn annotation_sets_nested_past_the_ceiling_are_an_error() {
    let mut e = annotation_item();
    for _ in 0..=super::MAX_DEPTH {
        e = annotation_set(&[e]);
    }
    let w = markup_data(&[], &[], &[], &[e]);
    assert!(matches!(read(&w, |c| c.markup_data()), Err(PrcError::Malformed(_))));
}

#[test]
fn an_annotation_of_another_type_is_an_error() {
    let w = markup_data(&[], &[], &[], &[annotation(502, |_| {})]);
    assert!(matches!(read(&w, |c| c.markup_data()), Err(PrcError::Malformed(_))));
}

#[test]
fn a_view_is_read_past_with_its_flags_only_from_8137() {
    let w = views(&[view(8137)]);
    read(&w, |c| c.views()).unwrap();
    truncations_fail(&w, |c| c.views());
    read_at(&views(&[view(8030)]), 8030, |c| c.views()).unwrap();
}

#[test]
fn scene_lights_of_each_kind_are_read_past() {
    let w = scene(&[light(731), light(732), light(733), light(734)], 0, 8137);
    read(&w, |c| c.scene_display_parameters()).unwrap();
    truncations_fail(&w, |c| c.scene_display_parameters());
    let mut old = W::default();
    old.uint(733);
    ref_base(&mut old, None, 5);
    old.uint(1).uint(2).uint(3).double(0.).double(0.).double(1.);
    read_at(&old, 8020, |c| c.light()).unwrap();
}

#[test]
fn scene_clipping_planes_are_read_past() {
    let w = scene(&[], 2, 8137);
    read(&w, |c| c.scene_display_parameters()).unwrap();
    truncations_fail(&w, |c| c.scene_display_parameters());
}

#[test]
fn a_tree_with_markups_and_views_still_places_its_parts() {
    let mut tail = markup_data(&[linked_item()], &[leader()], &[markup()], &[annotation_set(&[annotation_item()])]);
    tail.append(&views(&[view(8137)]));
    let part = part_with(Some("Plate"), PLAIN, 1, |w| super::tests::poly_item(w, 7, 0, 1, (S_RED, super::SHOW)), &tail);
    let occs = [Occ { name: Some("Asm"), sons: vec![1], markups: Some(tail.clone()), ..Occ::default() }, Occ { part: 1, ..Occ::default() }];
    let f = single(&[part, plate(S_RED)], &occs);
    let nodes = f.model_tree().unwrap();
    assert_eq!(nodes.len(), 2);
    let p = f.placements().unwrap();
    assert_eq!(p.len(), 1);
    assert!(matches!(f.file_structures[0].tessellations().unwrap().first(), Some(Tessellation::Mesh(_))));
}
