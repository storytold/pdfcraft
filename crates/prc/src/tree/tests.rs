#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::style::{Material, Style};
use super::{
    EntityOverrides, FATHER_HERIT_COLOR, FILE_STRUCTURE_TREE, Globals, Graphics, MAX_PLACEMENTS, NameSource, SHOW, SON_HERIT_COLOR, StyleAlpha,
    resolve_style,
};
use crate::bits::{BitReader, Budget};
use crate::entity::Ctx;
use crate::tess::tests::two_face_square;
use crate::testw::W;
use crate::{PrcError, PrcFile, Rgba, Schema, Tessellation, TriangleMesh, UniqueId};

const RED: Rgba = Rgba { r: 1., g: 0., b: 0., a: 1. };
const GREEN: Rgba = Rgba { r: 0., g: 1., b: 0., a: 1. };
const BLUE: Rgba = Rgba { r: 0., g: 0., b: 1., a: 1. };
/// Visible graphics with no style.
pub(super) const PLAIN: (u32, u16) = (0, SHOW);
/// The biased style indices of [`palette`]: red, green, a blue material.
pub(super) const S_RED: u32 = 1;
const S_GREEN: u32 = 2;
const S_BLUE: u32 = 3;
const RI_POLY_BREP: u32 = 237;

fn fs_id(i: usize) -> UniqueId {
    UniqueId([0x100 + u32::try_from(i).unwrap(), 1, 2, 3])
}

fn len(n: usize) -> u32 {
    u32::try_from(n).unwrap()
}

/// `ContentPRCBase` with no attributes and its own name.
pub(super) fn base(w: &mut W, name: Option<&str>) {
    w.uint(0).bit(false).string(name);
}

pub(super) fn ref_base(w: &mut W, name: Option<&str>, uid: u32) {
    base(w, name);
    w.uint(0).uint(0).uint(uid);
}

/// `PRCBaseWithGraphics` with its own `(style, behaviour)`.
pub(super) fn graphics(w: &mut W, name: Option<&str>, uid: u32, (style, bits): (u32, u16)) {
    ref_base(w, name, uid);
    w.bit(false).uint(0).uint(style).put(u64::from(bits & 0xff), 8).put(u64::from(bits >> 8), 8);
}

fn file_id(w: &mut W, fs: Option<usize>) {
    match fs {
        None => {
            w.bit(true);
        }
        Some(i) => {
            w.bit(false);
            for v in fs_id(i).0 {
                w.uint(v);
            }
        }
    }
}

/// An `RI_PolyBrepModel` drawing biased tessellation `tess` under local system `cs`.
pub(super) fn poly_item(w: &mut W, uid: u32, cs: u32, tess: u32, g: (u32, u16)) {
    w.uint(RI_POLY_BREP);
    graphics(w, None, uid, g);
    w.uint(cs).uint(tess).bit(false).uint(0);
}

/// An `RI_BrepModel` drawing tessellation 1, its exact body stored as `brep`.
fn brep_item(w: &mut W, uid: u32, g: (u32, u16), brep: (u32, u32)) {
    w.uint(232);
    graphics(w, None, uid, g);
    w.uint(0).uint(1).bit(true).uint(brep.0).uint(brep.1).bit(false).uint(0);
}

/// A `PartDefinition` holding the `n` items `items` writes, and no markups or views.
fn part(name: Option<&str>, g: (u32, u16), n: usize, items: impl FnOnce(&mut W)) -> W {
    let mut none = W::default();
    for _ in 0..5 {
        none.uint(0);
    }
    part_with(name, g, n, items, &none)
}

/// [`part`] with `markups` (its markup data and views) written after the items.
pub(super) fn part_with(name: Option<&str>, g: (u32, u16), n: usize, items: impl FnOnce(&mut W), markups: &W) -> W {
    let mut w = W::default();
    w.uint(311);
    graphics(&mut w, name, 0, g);
    for _ in 0..6 {
        w.double(0.);
    }
    w.uint(len(n));
    items(&mut w);
    w.append(markups).uint(0); // user data
    w
}

/// One poly-B-rep item (uid 7) drawing tessellation 1 in style `style`.
pub(super) fn plate(style: u32) -> W {
    part(Some("Plate"), PLAIN, 1, |w| poly_item(w, 7, 0, 1, (style, SHOW)))
}

#[derive(Default)]
pub(super) struct Occ<'a> {
    pub(super) name: Option<&'a str>,
    pub(super) g: Option<(u32, u16)>,
    pub(super) part: u32,
    pub(super) prototype: u32,
    pub(super) prototype_fs: Option<usize>,
    pub(super) sons: Vec<u32>,
    pub(super) suppressed: bool,
    pub(super) location: Option<W>,
    pub(super) refs: Vec<W>,
    /// Its markup data and views; none when `None`.
    pub(super) markups: Option<W>,
}

fn occ(w: &mut W, o: &Occ<'_>) {
    w.uint(310);
    graphics(w, o.name, 0, o.g.unwrap_or(PLAIN));
    w.uint(o.part).uint(o.prototype);
    if o.prototype != 0 {
        file_id(w, o.prototype_fs);
    }
    w.uint(0).uint(len(o.sons.len()));
    for &s in &o.sons {
        w.uint(s);
    }
    w.put(u64::from(o.suppressed), 8);
    w.bit(false).double(1.).put(0, 8).int(0);
    match &o.location {
        Some(t) => w.bit(true).append(t),
        None => w.bit(false),
    };
    w.uint(len(o.refs.len()));
    for r in &o.refs {
        w.append(r);
    }
    match &o.markups {
        Some(m) => {
            w.append(m);
        }
        None => {
            for _ in 0..5 {
                w.uint(0);
            }
        }
    }
    w.bit(false).uint(0).uint(0).uint(0);
}

pub(super) fn tree(parts: &[W], occs: &[Occ<'_>]) -> W {
    let mut w = W::default();
    w.uint(304);
    base(&mut w, None);
    w.uint(len(parts.len()));
    for p in parts {
        w.append(p);
    }
    w.uint(len(occs.len()));
    for o in occs {
        occ(&mut w, o);
    }
    w.uint(302);
    base(&mut w, None);
    w.uint(0).uint(0).uint(0);
    w
}

fn style(index: u32, is_material: bool, transparency: Option<u8>) -> W {
    let mut w = W::default();
    w.uint(701);
    ref_base(&mut w, None, 0);
    w.double(0.).bit(false).uint(0).bit(is_material).uint(index);
    match transparency {
        Some(t) => w.bit(true).put(u64::from(t), 8),
        None => w.bit(false),
    };
    w.bit(false).bit(false).bit(false);
    w
}

fn material(diffuse: u32, alpha: f64) -> W {
    let mut w = W::default();
    w.uint(702);
    ref_base(&mut w, None, 0);
    w.uint(0).uint(diffuse).uint(0).uint(0);
    w.double(0.).double(1.).double(alpha).double(1.).double(1.);
    w
}

/// A globals section (schema included) with `colours`, `materials`, `styles` and reference
/// coordinate systems `systems` (transformations).
fn globals(colours: &[Rgba], materials: &[W], styles: &[W], systems: &[W]) -> W {
    let mut schema = W::default();
    schema.uint(0);
    globals_after(schema, colours, materials, styles, systems)
}

/// [`globals`] after the already-written `schema`.
fn globals_after(mut w: W, colours: &[Rgba], materials: &[W], styles: &[W], systems: &[W]) -> W {
    w.uint(303);
    base(&mut w, None);
    w.uint(0).double(0.).double(0.).string(None).uint(0);
    w.uint(len(colours.len()));
    for c in colours {
        w.double(c.r).double(c.g).double(c.b);
    }
    w.uint(0).uint(0).uint(len(materials.len()));
    for m in materials {
        w.append(m);
    }
    w.uint(0).uint(len(styles.len()));
    for s in styles {
        w.append(s);
    }
    w.uint(0).uint(len(systems.len()));
    for t in systems {
        w.uint(240);
        graphics(&mut w, None, 0, PLAIN);
        w.uint(0).uint(0).append(t).uint(0);
    }
    w.uint(0);
    w
}

/// Red, green and blue colours, and styles [`S_RED`], [`S_GREEN`] and [`S_BLUE`] (a
/// material with diffuse alpha 0).
fn palette() -> W {
    globals(&[RED, GREEN, BLUE], &[material(7, 0.)], &[style(1, false, None), style(4, false, None), style(1, true, None)], &[])
}

fn translate(t: [f64; 3]) -> W {
    let mut w = W::default();
    w.uint(202).put(0x01, 8).double(t[0]).double(t[1]).double(t[2]);
    w
}

/// A translation by `t` after a uniform scale by `s`.
fn translate_scale(t: [f64; 3], s: f64) -> W {
    let mut w = W::default();
    w.uint(202).put(0x01 | 0x08, 8).double(t[0]).double(t[1]).double(t[2]).double(s);
    w
}

/// An entity reference giving the item with PRC unique id `uid` the graphics `g`.
fn ref_entity(uid: u32, g: (u32, u16)) -> W {
    ref_entity_at(uid, g, 0)
}

/// [`ref_entity`], also placing its target by biased coordinate system `local`.
fn ref_entity_at(uid: u32, g: (u32, u16), local: u32) -> W {
    let mut w = W::default();
    w.uint(203);
    graphics(&mut w, None, 0, g);
    w.uint(local).bit(true).uint(205).uint(RI_POLY_BREP).bit(true).uint(uid).uint(0);
    w
}

/// An entity reference giving faces `faces` of B-rep `brep` the graphics `g`.
fn ref_faces(brep: (u32, u32), faces: &[u32], g: (u32, u16)) -> W {
    let mut w = W::default();
    w.uint(203);
    graphics(&mut w, None, 0, g);
    w.uint(0).bit(true).uint(206).uint(149).bit(true).bit(true).uint(brep.0).uint(brep.1).uint(len(faces.len()));
    for &f in faces {
        w.uint(f);
    }
    w.uint(0);
    w
}

/// A model file (schema included) whose roots are `(file structure, biased occurrence)`.
fn model(roots: &[(usize, u32)]) -> W {
    let mut w = W::default();
    w.uint(0).uint(301);
    base(&mut w, None);
    w.bit(false).double(1.).uint(len(roots.len()));
    for &(fs, index) in roots {
        for v in fs_id(fs).0 {
            w.uint(v);
        }
        w.uint(index).bit(true);
    }
    w
}

/// One file structure's globals, tree and tessellation sections.
struct Fs {
    globals: W,
    tree: W,
    tess: W,
}

fn zlib(data: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn u32s(out: &mut Vec<u8>, vs: &[u32]) {
    vs.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
}

/// A PRC stream of `structures` and `model`.
fn prc(structures: &[Fs], model: &W) -> Vec<u8> {
    let header_len = 59 + 48 * structures.len();
    let at = |body: &Vec<u8>| len(header_len + body.len());
    let mut body = Vec::new();
    let mut descriptions = Vec::new();
    for (i, fs) in structures.iter().enumerate() {
        let mut offs = vec![at(&body)];
        body.extend(b"PRC");
        u32s(&mut body, &[8137, 8137]);
        u32s(&mut body, &fs_id(i).0);
        u32s(&mut body, &[0, 0, 0, 0, 0]);
        for s in [fs.globals.bytes(), fs.tree.bytes(), fs.tess.bytes(), Vec::new(), Vec::new()] {
            offs.push(at(&body));
            body.extend(zlib(&s));
        }
        descriptions.push((fs_id(i), offs));
    }
    let mf_start = at(&body);
    body.extend(zlib(&model.bytes()));
    let mf_end = at(&body);
    let mut out = b"PRC".to_vec();
    u32s(&mut out, &[8137, 8137, 1, 2, 3, 4, 0, 0, 0, 0, len(structures.len())]);
    for (id, offs) in descriptions {
        u32s(&mut out, &id.0);
        u32s(&mut out, &[0, 6]);
        u32s(&mut out, &offs);
    }
    u32s(&mut out, &[mf_start, mf_end, 0]);
    out.extend(body);
    assert_eq!(out.len(), usize::try_from(mf_end).unwrap());
    out
}

/// One file structure with [`palette`], `parts` and `occs`, whose root is occurrence 0.
pub(super) fn single(parts: &[W], occs: &[Occ<'_>]) -> PrcFile {
    PrcFile::parse(&prc(&[Fs { globals: palette(), tree: tree(parts, occs), tess: two_face_square() }], &model(&[(0, 1)]))).unwrap()
}

fn mesh(f: &PrcFile, fs: usize) -> TriangleMesh {
    match f.file_structures[fs].tessellations().unwrap().into_iter().next() {
        Some(Tessellation::Mesh(m)) => m,
        other => panic!("not a mesh: {other:?}"),
    }
}

fn colours(f: &PrcFile, scope: EntityOverrides) -> Vec<Option<Rgba>> {
    f.placements_with(StyleAlpha::default(), scope).unwrap().iter().map(|p| p.colour).collect()
}

fn malformed(r: Result<impl std::fmt::Debug, PrcError>) -> String {
    match r {
        Err(PrcError::Malformed(m)) => m,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn a_parsed_assembly_places_names_and_colours_each_occurrence() {
    let occs = [
        Occ { name: Some("Asm"), sons: vec![1, 2], location: Some(translate([0., 0., 100.])), ..Occ::default() },
        Occ { part: 1, location: Some(translate([10., 0., 0.])), ..Occ::default() },
        Occ { name: Some("B"), part: 1, location: Some(translate([0., 5., 0.])), ..Occ::default() },
    ];
    let f = single(&[plate(S_RED)], &occs);
    assert!(f.stores_tree());
    let p = f.placements().unwrap();
    assert_eq!(p.len(), 2);
    let m = mesh(&f, 0);
    let moved: Vec<_> = p.iter().map(|p| (p.file_structure, p.tessellation, m.transformed(&p.matrix).positions[2])).collect();
    assert_eq!(moved, [(0, 0, [11., 1., 100.]), (0, 0, [1., 6., 100.])]);
    assert_eq!(p.iter().map(|p| p.colour).collect::<Vec<_>>(), [Some(RED); 2]);
    // A material with diffuse alpha 0 and no style transparency reads as opaque.
    assert_eq!(colours(&single(&[plate(S_BLUE)], &occs), EntityOverrides::default()), [Some(BLUE); 2]);
    let nodes = f.model_tree().unwrap();
    let summary: Vec<_> = nodes.iter().map(|n| (n.name.as_deref(), n.name_from, n.parent, n.depth, n.placements.clone())).collect();
    assert_eq!(
        summary,
        [
            (Some("Asm"), NameSource::Occurrence, None, 0, 0..2),
            (Some("Plate"), NameSource::Part, Some(0), 1, 0..1),
            (Some("B"), NameSource::Occurrence, Some(0), 1, 1..2),
        ]
    );
}

#[test]
fn a_file_without_a_model_file_entity_has_no_tree() {
    let fs = Fs { globals: palette(), tree: tree(&[], &[]), tess: two_face_square() };
    let bytes = prc(&[fs], &W::default());
    let f = PrcFile::parse(&bytes).unwrap();
    assert!(!f.stores_tree());
    assert!(f.placements().unwrap().is_empty());
}

#[test]
fn an_occurrence_that_contains_itself_is_an_error() {
    let occs = [Occ { sons: vec![1], ..Occ::default() }, Occ { part: 1, sons: vec![0, 0], ..Occ::default() }];
    assert!(malformed(single(&[plate(S_RED)], &occs).placements()).contains("contains itself"));
}

#[test]
fn a_prototype_cycle_is_an_error() {
    let occs = [Occ { prototype: 2, ..Occ::default() }, Occ { prototype: 1, ..Occ::default() }];
    assert!(malformed(single(&[], &occs).model_tree()).contains("own prototype"));
}

/// One element allowance covers the whole walk: an allowance that fits one file structure does
/// not fit two, and running short of it is reported as such.
#[test]
fn one_element_allowance_covers_the_whole_walk() {
    // A part of 64 items, so each file structure's tree section, not its globals or the model
    // file, decodes most of the elements.
    let wide = || part(None, PLAIN, 64, |w| (0..64).for_each(|_| poly_item(w, 0, 0, 1, PLAIN)));
    let fs = || Fs { globals: palette(), tree: tree(&[wide()], &[Occ { part: 1, ..Occ::default() }]), tess: two_face_square() };
    let one = PrcFile::parse(&prc(&[fs()], &model(&[(0, 1)]))).unwrap();
    let two = PrcFile::parse(&prc(&[fs(), fs()], &model(&[(0, 1), (1, 1)]))).unwrap();
    let walk = |f: &PrcFile, n| f.walk_within(StyleAlpha::default(), EntityOverrides::default(), Budget { elements: n, ..Budget::FULL });
    let short = |f: &PrcFile, n| matches!(walk(f, n), Err(PrcError::TooManyElements { .. }));
    // One structure decodes about 70 elements, two about 140.
    assert!(short(&one, 32));
    assert_eq!(walk(&one, 100).map(|w| w.0.len()), Ok(64));
    assert!(short(&two, 100), "each structure fits in 100, both do not");
    assert_eq!(walk(&two, 200).map(|w| w.0.len()), Ok(128));
}

/// One schema-step allowance covers the whole walk: each structure's tree runs a 2,000-step
/// program, which a 3,000-step allowance fits once but not twice.
#[test]
fn one_step_allowance_covers_the_whole_walk() {
    let fs = || {
        let mut schema = W::default();
        // For (1000) { empty block } on the file structure tree.
        schema.uint(1).uint(FILE_STRUCTURE_TREE).uint(5);
        for t in [15, 26, 1000, 19, 21] {
            schema.uint(t);
        }
        let globals = globals_after(schema, &[RED], &[], &[], &[]);
        Fs { globals, tree: tree(&[plate(S_RED)], &[Occ { part: 1, ..Occ::default() }]), tess: two_face_square() }
    };
    let one = PrcFile::parse(&prc(&[fs()], &model(&[(0, 1)]))).unwrap();
    let two = PrcFile::parse(&prc(&[fs(), fs()], &model(&[(0, 1), (1, 1)]))).unwrap();
    let walk = |f: &PrcFile| f.walk_within(StyleAlpha::default(), EntityOverrides::default(), Budget { steps: 3000, ..Budget::FULL });
    assert_eq!(walk(&one).map(|w| w.0.len()), Ok(1));
    assert!(matches!(walk(&two), Err(PrcError::Malformed(m)) if m.contains("step ceiling")));
    assert_eq!(two.placements().map(|p| p.len()), Ok(2));
}

/// Each structure's schema is read once, charged to the walk's allowance: a schema too large
/// for what is left is an error, not decoded outside the allowance and then skipped.
#[test]
fn the_globals_schema_is_charged_to_the_walk() {
    let mut schema = W::default();
    // 200 empty entries cost 200 elements.
    schema.uint(200);
    for t in 0..200 {
        schema.uint(50_000 + t).uint(0);
    }
    let fs = Fs {
        globals: globals_after(schema, &[RED], &[], &[], &[]),
        tree: tree(&[plate(S_RED)], &[Occ { part: 1, ..Occ::default() }]),
        tess: two_face_square(),
    };
    let f = PrcFile::parse(&prc(&[fs], &model(&[(0, 1)]))).unwrap();
    let walk = |n| f.walk_within(StyleAlpha::default(), EntityOverrides::default(), Budget { elements: n, ..Budget::FULL });
    assert!(matches!(walk(100), Err(PrcError::TooManyElements { .. })));
    assert_eq!(walk(1000).map(|w| w.0.len()), Ok(1));
}

/// Globals that cannot be read are reported, not replaced by empty ones, when an entity
/// reference names one of their coordinate systems.
#[test]
fn unreadable_globals_are_an_error_when_a_reference_names_a_coordinate_system() {
    let file = |local| {
        let mut fonts = W::default();
        fonts.uint(0).uint(303);
        base(&mut fonts, None);
        fonts.uint(0).double(0.).double(0.).string(None).uint(1);
        let occs = [Occ { part: 1, refs: vec![ref_entity_at(7, PLAIN, local)], ..Occ::default() }];
        let fs = Fs { globals: fonts, tree: tree(&[plate(S_RED)], &occs), tess: two_face_square() };
        PrcFile::parse(&prc(&[fs], &model(&[(0, 1)]))).unwrap()
    };
    assert_eq!(file(0).placements().map(|p| p.len()), Ok(1));
    assert!(matches!(file(1).placements(), Err(PrcError::Unsupported("PRC global fonts"))));
}

#[test]
fn an_exponential_walk_is_cut_off() {
    // Occurrence k has occurrence k + 1 twice as sons: 2^21 visits.
    let occs: Vec<_> = (0..21).map(|k| Occ { sons: if k < 20 { vec![k + 1, k + 1] } else { Vec::new() }, ..Occ::default() }).collect();
    assert!(malformed(single(&[], &occs).model_tree()).contains("too large"));
}

#[test]
fn placements_past_the_ceiling_are_an_error() {
    // 1,025 placements of a part with 1,024 items.
    let items = 1024;
    let wide = part(None, PLAIN, items, |w| (0..items).for_each(|_| poly_item(w, 0, 0, 1, PLAIN)));
    let occs = [Occ { sons: vec![1; 1025], ..Occ::default() }, Occ { part: 1, ..Occ::default() }];
    assert!(1025 * items > MAX_PLACEMENTS);
    assert!(malformed(single(&[wide], &occs).placements()).contains("placements"));
}

#[test]
fn hidden_and_suppressed_occurrences_are_listed_but_not_placed() {
    let occs = [
        Occ { sons: vec![1, 2, 3], ..Occ::default() },
        Occ { part: 1, ..Occ::default() },
        Occ { part: 1, suppressed: true, ..Occ::default() },
        Occ { part: 1, sons: vec![1], g: Some((0, 0)), ..Occ::default() },
    ];
    let f = single(&[plate(S_RED)], &occs);
    assert_eq!(f.placements().unwrap().len(), 1);
    let nodes = f.model_tree().unwrap();
    let flags: Vec<_> = nodes.iter().map(|n| (n.occurrence, n.hidden, n.suppressed, n.drawn)).collect();
    assert_eq!(flags, [(0, false, false, true), (1, false, false, true), (2, false, true, false), (3, true, false, false), (1, false, false, false)]);
}

#[test]
fn a_father_herit_colour_beats_the_part_and_a_son_herit_colour_beats_that() {
    let forced = (S_GREEN, SHOW | FATHER_HERIT_COLOR);
    let occs = [Occ { sons: vec![1], g: Some(forced), ..Occ::default() }, Occ { part: 1, ..Occ::default() }];
    assert_eq!(colours(&single(&[plate(S_RED)], &occs), EntityOverrides::default()), [Some(GREEN)]);
    let son = part(None, PLAIN, 1, |w| poly_item(w, 7, 0, 1, (S_RED, SHOW | SON_HERIT_COLOR)));
    assert_eq!(colours(&single(&[son], &occs), EntityOverrides::default()), [Some(RED)]);
    // Unforced, the son's own style wins over its father's.
    let occs = [Occ { sons: vec![1], g: Some((S_GREEN, SHOW)), ..Occ::default() }, Occ { part: 1, ..Occ::default() }];
    assert_eq!(colours(&single(&[plate(S_RED)], &occs), EntityOverrides::default()), [Some(RED)]);
}

#[test]
fn resolve_style_takes_the_oldest_forcing_ancestor() {
    let g = |style, bits| Graphics { style, bits, fs: 0 };
    let chain = [g(1, FATHER_HERIT_COLOR), g(2, FATHER_HERIT_COLOR), g(0, 0), g(3, 0)];
    assert_eq!(resolve_style(&chain).style, 1);
    assert_eq!(resolve_style(&[g(1, 0), g(0, 0)]).style, 1);
    assert_eq!(resolve_style(&[]).style, 0);
}

#[test]
fn a_prototypes_part_is_coloured_by_its_own_structures_styles() {
    // Structure 1 is a part library whose style 1 is green; structure 0's style 1 is red.
    let library = Fs {
        globals: globals(&[GREEN], &[], &[style(1, false, None)], &[]),
        tree: tree(&[plate(1)], &[Occ { name: Some("Bolt"), part: 1, ..Occ::default() }]),
        tess: two_face_square(),
    };
    let assembly = Fs {
        globals: globals(&[RED], &[], &[style(1, false, None)], &[]),
        tree: tree(&[], &[Occ { prototype: 1, prototype_fs: Some(1), location: Some(translate([1., 2., 3.])), ..Occ::default() }]),
        tess: W::default(),
    };
    let f = PrcFile::parse(&prc(&[assembly, library], &model(&[(0, 1)]))).unwrap();
    let p = f.placements().unwrap();
    assert_eq!(p.iter().map(|p| (p.file_structure, p.colour, p.matrix.at(1, 3))).collect::<Vec<_>>(), [(1, Some(GREEN), 2.)]);
    let n = f.model_tree().unwrap();
    assert_eq!((n[0].name.as_deref(), n[0].name_from), (Some("Bolt"), NameSource::Prototype));
}

/// A root placing the red plate twice, the first occurrence carrying `refs`.
fn shared_plate(refs: Vec<W>) -> PrcFile {
    let occs = [Occ { sons: vec![1, 2], ..Occ::default() }, Occ { part: 1, refs, ..Occ::default() }, Occ { part: 1, ..Occ::default() }];
    single(&[plate(S_RED)], &occs)
}

#[test]
fn an_entity_reference_reaches_its_scope() {
    let f = shared_plate(vec![ref_entity(7, (S_GREEN, SHOW))]);
    assert_eq!(colours(&f, EntityOverrides::Subtree), [Some(GREEN), Some(RED)]);
    assert_eq!(colours(&f, EntityOverrides::Everywhere), [Some(GREEN), Some(GREEN)]);
    assert_eq!(colours(&f, EntityOverrides::Ignore), [Some(RED), Some(RED)]);
    // A reference to another uid changes nothing; one without Show hides its target.
    assert_eq!(colours(&shared_plate(vec![ref_entity(8, (S_GREEN, SHOW))]), EntityOverrides::Subtree), [Some(RED); 2]);
    assert_eq!(colours(&shared_plate(vec![ref_entity(7, (0, 0))]), EntityOverrides::Subtree), [Some(RED)]);
}

#[test]
fn a_topology_reference_colours_its_faces() {
    let brep = part(None, PLAIN, 1, |w| brep_item(w, 7, (S_RED, SHOW), (4, 9)));
    let occs = [Occ { part: 1, refs: vec![ref_faces((4, 9), &[1], (S_GREEN, SHOW))], ..Occ::default() }];
    let f = single(&[brep], &occs);
    let p = f.placements().unwrap();
    assert_eq!(p[0].colour, Some(RED));
    assert_eq!(p[0].triangle_colours(&mesh(&f, 0)), Some(vec![Some(RED), Some(GREEN)]));
    // Another body's faces are left alone.
    let occs = [Occ { part: 1, refs: vec![ref_faces((4, 8), &[1], (S_GREEN, SHOW))], ..Occ::default() }];
    let f = single(&[part(None, PLAIN, 1, |w| brep_item(w, 7, (S_RED, SHOW), (4, 9)))], &occs);
    assert_eq!(f.placements().unwrap()[0].triangle_colours(&mesh(&f, 0)), None);
}

#[test]
fn an_items_coordinate_system_applies_after_the_occurrence_location() {
    let fs = Fs {
        globals: globals(&[RED], &[], &[], &[translate([0., 0., 7.])]),
        tree: tree(
            &[part(None, PLAIN, 1, |w| poly_item(w, 7, 1, 1, PLAIN))],
            &[Occ { part: 1, location: Some(translate_scale([1., 0., 0.], 2.)), ..Occ::default() }],
        ),
        tess: two_face_square(),
    };
    let f = PrcFile::parse(&prc(&[fs], &model(&[(0, 1)]))).unwrap();
    let p = f.placements().unwrap();
    assert_eq!((p[0].matrix.at(0, 3), p[0].matrix.at(2, 3), p[0].colour), (1., 14., None));
}

#[test]
fn a_colour_index_counts_doubles() {
    // pdf-issues #816: colour i is stored as 3i + 1.
    let g =
        |index| Globals { colours: vec![RED, GREEN], styles: vec![Style { is_material: false, index, transparency: None }], ..Globals::default() };
    let paint = |index| g(index).style_paint(1, StyleAlpha::default()).map(|p| p.rgba);
    assert_eq!(paint(1), Some(RED));
    assert_eq!(paint(4), Some(GREEN));
    assert_eq!(paint(2), None);
    assert_eq!(paint(7), None);
}

#[test]
fn style_alpha_rules_combine_transparency_with_material_alpha() {
    let g = |alpha, transparency| Globals {
        colours: vec![BLUE],
        materials: vec![Material::Plain { diffuse: 1, emissive: 0, alpha }],
        styles: vec![Style { is_material: true, index: 1, transparency }],
        ..Globals::default()
    };
    let a = |alpha, t, rule| g(alpha, t).style_paint(1, rule).unwrap().rgba.a;
    assert_eq!(a(0., None, StyleAlpha::ZeroUnset), 1.);
    assert_eq!(a(0., None, StyleAlpha::StyleWins), 0.);
    assert_eq!(a(0.5, Some(51), StyleAlpha::StyleWins), 0.2);
    assert_eq!(a(0.5, Some(51), StyleAlpha::Multiply), 0.1);
    assert_eq!(a(f64::NAN, None, StyleAlpha::Multiply), 1.);
    // A textured material takes its base's colour.
    let mut t = g(1., None);
    t.materials.push(Material::Textured { base: 1 });
    t.styles[0].index = 2;
    assert_eq!(t.style_paint(1, StyleAlpha::default()).unwrap().rgba, BLUE);
}

fn transformation(behaviour: u8, fields: &[f64]) -> super::Matrix {
    let mut w = W::default();
    w.uint(202).put(u64::from(behaviour), 8);
    for &d in fields {
        w.double(d);
    }
    let bytes = w.bytes();
    Ctx::new(BitReader::new(&bytes), &Schema::default(), 8137).transformation().unwrap()
}

#[test]
fn a_mirrored_rotation_takes_y_cross_x_and_both_scales_multiply() {
    let x_y = [1., 0., 0., 0., 1., 0.];
    assert_eq!(transformation(0x02, &x_y).at(2, 2), 1.);
    assert_eq!(transformation(0x02 | 0x04, &x_y).at(2, 2), -1.);
    let m = transformation(0x10 | 0x08, &[1., 2., 3., 2.]);
    assert_eq!([m.at(0, 0), m.at(1, 1), m.at(2, 2)], [2., 4., 6.]);
    let m = transformation(0x40, &[0., 0., 0.5, 2.]);
    assert_eq!(m.0[3], [0., 0., 0.5, 2.]);
}

#[test]
fn every_truncation_of_a_tree_or_globals_is_an_error_not_a_panic() {
    let occs = [
        Occ { name: Some("Asm"), sons: vec![1], location: Some(translate([0., 0., 1.])), ..Occ::default() },
        Occ { part: 1, refs: vec![ref_entity(7, (S_GREEN, SHOW)), ref_faces((1, 1), &[0], PLAIN)], ..Occ::default() },
    ];
    let t = tree(&[plate(S_RED)], &occs).bytes();
    let schema = Schema::default();
    for end in 0..t.len().saturating_sub(1) {
        assert!(Ctx::new(BitReader::new(&t[..end]), &schema, 8137).file_structure_tree().is_err(), "tree cut at {end}");
    }
    let g = palette().bytes();
    for end in 1..g.len().saturating_sub(1) {
        let mut r = BitReader::new(&g[..end]);
        if Schema::read(&mut r).is_ok() {
            assert!(Ctx::new(r, &schema, 8137).globals().is_err(), "globals cut at {end}");
        }
    }
}

#[test]
fn a_prototype_chain_past_the_ceiling_is_an_error() {
    // Occurrence k (1-based) has occurrence k + 1 as its prototype; none has a part.
    let n = u32::try_from(super::MAX_DEPTH).unwrap() + 3;
    let occs: Vec<_> = (1..=n).map(|k| Occ { prototype: if k < n { k + 1 } else { 0 }, ..Occ::default() }).collect();
    assert!(malformed(single(&[], &occs).model_tree()).contains("prototype chain too long"));
}

/// `depth` representation sets, each holding the next, around one poly-B-rep item.
fn nested_sets(w: &mut W, depth: usize) {
    if depth == 0 {
        return poly_item(w, 0, 0, 1, PLAIN);
    }
    w.uint(super::RI_SET);
    graphics(w, None, 0, PLAIN);
    w.uint(0).uint(0).uint(1);
    nested_sets(w, depth - 1);
    w.uint(0); // user data
}

#[test]
fn representation_sets_nested_past_the_ceiling_are_an_error() {
    let deep = part(None, PLAIN, 1, |w| nested_sets(w, super::MAX_DEPTH + 1));
    let occs = [Occ { part: 1, ..Occ::default() }];
    assert!(malformed(single(&[deep], &occs).placements()).contains("representation sets nest too deep"));
    let ok = part(None, PLAIN, 1, |w| nested_sets(w, super::MAX_DEPTH));
    assert_eq!(single(&[ok], &occs).placements().unwrap().len(), 1);
}
