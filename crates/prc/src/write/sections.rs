//! The globals (303), tree (304 + 302), empty geometry (306, 307) and
//! model-file (301) entities [WD 7.3.3-7.3.11, 7.4.9-7.4.11, 7.5.3].

use super::validate::Plan;
use super::vec3::{cross, dot};
use super::{BitWriter, MeshScene};
use crate::UniqueId;

const MODEL_FILE: u32 = 301;
const FILE_STRUCTURE: u32 = 302;
const GLOBALS: u32 = 303;
const TREE: u32 = 304;
const PRODUCT_OCCURRENCE: u32 = 310;
const PART_DEFINITION: u32 = 311;
const CARTESIAN_TRANSFORMATION: u32 = 202;
const GENERAL_TRANSFORMATION: u32 = 207;
const RI_POLY_BREP_MODEL: u32 = 237;
const STYLE: u32 = 701;
const LINE_PATTERN: u32 = 721;

/// `PRC_GRAPHICS_Show` [WD 7.2.4.2].
const SHOW: u16 = 0x0001;
/// `EPRCProductLoadStatus` given to every product written.
const LOAD_STATUS: i32 = 4;

/// `ContentPRCBase` with no attributes and the current (section-start:
/// empty) name [WD 7.2.3].
pub(super) fn base(w: &mut BitWriter) {
    w.uint(0).bit(true);
}

/// `ContentPRCBase` naming the entity explicitly; `None` is a null name.
fn named(w: &mut BitWriter, name: Option<&str>) {
    w.uint(0).bit(false).string(name);
}

/// `ContentPRCRefBase`: `ContentPRCBase`, CAD ids 0, then `id`.
pub(super) fn ref_base(w: &mut BitWriter, name: Option<Option<&str>>, id: u32) {
    match name {
        None => base(w),
        Some(n) => named(w, n),
    }
    w.uint(0).uint(0).uint(id);
}

/// `PRCBaseWithGraphics` with explicit graphics: no layer, line style
/// `style` (`None` inheriting), behaviour Show.
fn graphics(w: &mut BitWriter, name: Option<&str>, id: u32, style: Option<u32>) {
    ref_base(w, Some(name), id);
    w.bit(false).uint(0).uint(style.map_or(0, |s| s + 1)).put(u64::from(SHOW & 0xff), 8).put(u64::from(SHOW >> 8), 8);
}

/// Unique ids for referenceable entities, from 1.
struct Ids(u32);

impl Ids {
    fn next(&mut self) -> u32 {
        self.0 += 1;
        self.0
    }
}

/// The globals section: an empty schema, one colour and one RGB style per
/// material, the texture entities and one material style per textured
/// material, and the solid line pattern every style names.
pub(super) fn globals(scene: &MeshScene, plan: &Plan) -> BitWriter {
    let mut w = BitWriter::default();
    w.uint(0).uint(GLOBALS);
    base(&mut w);
    // No referenced structures; chord and angle tolerances; markup font
    // helper empty.
    w.uint(0).double(2000.0).double(40.0).string(Some("")).uint(0);
    w.uint(scene.materials.len() as u32);
    for m in &scene.materials {
        w.doubles(m.rgba[..3].iter().map(|c| c.clamp(0.0, 1.0)));
    }
    let styles = (scene.materials.len() + plan.textured.len()) as u32;
    super::textures::pictures(&mut w, &plan.pictures);
    super::textures::definitions(&mut w, scene, 2 + styles);
    let first = 2 + styles + scene.textures.len() as u32;
    super::textures::materials(&mut w, scene, &plan.textured, first);
    w.uint(1).uint(LINE_PATTERN);
    ref_base(&mut w, None, 1);
    w.uint(2).double(1.0e6).double(0.0).double(0.0).bit(false);
    w.uint(styles);
    for (k, m) in scene.materials.iter().enumerate() {
        w.uint(STYLE);
        ref_base(&mut w, None, 2 + k as u32);
        // Width 1, no vpicture, line pattern 1 (biased), an RGB colour
        // (double-scaled biased index, WD 7.5.3).
        w.double(1.0).bit(false).uint(1).bit(false);
        w.uint(3 * k as u32 + 1);
        let alpha = m.rgba[3].clamp(0.0, 1.0);
        w.bit(alpha < 1.0);
        if alpha < 1.0 {
            w.put((alpha * 255.0).round() as u64, 8);
        }
        w.bit(false).bit(false).bit(false);
    }
    for j in 0..plan.textured.len() as u32 {
        w.uint(STYLE);
        ref_base(&mut w, None, 2 + scene.materials.len() as u32 + j);
        // As above, but a material: the TextureApplication entry (biased),
        // which carries its own alpha.
        w.double(1.0).bit(false).uint(1).bit(true);
        w.uint(2 * j + 2);
        w.bit(false).bit(false).bit(false).bit(false);
    }
    // Fill patterns, reference coordinate systems, user data.
    w.uint(0).uint(0).uint(0);
    w
}

/// An empty exact-geometry (306) or extra-geometry (307) section: no
/// topological contexts [WD 7.3.8, 7.3.9]. Viewers reject a zero-byte one.
pub(super) fn empty_geometry(kind: u32) -> BitWriter {
    let mut w = BitWriter::default();
    w.uint(kind);
    base(&mut w);
    w.uint(0).uint(0);
    w
}

/// The tree section: parts, occurrences, then the file structure's
/// internal data.
pub(super) fn tree(scene: &MeshScene, plan: &Plan) -> BitWriter {
    let mut ids = Ids(plan.global_ids(scene));
    let mut w = BitWriter::default();
    w.uint(TREE);
    base(&mut w);
    w.uint(plan.parts.len() as u32);
    let mut tess = 0u32;
    for &m in &plan.parts {
        part(&mut w, scene, plan, m, &mut tess, &mut ids);
    }
    w.uint(plan.occurrences.len() as u32);
    for o in &plan.occurrences {
        occurrence(&mut w, scene, plan, o, &mut ids);
    }
    w.uint(FILE_STRUCTURE);
    base(&mut w);
    // The root occurrence is the last (biased index = count).
    let next = ids.next();
    w.uint(next).uint(plan.occurrences.len() as u32).uint(0);
    w
}

/// Mesh `m`'s part: its bounding box and one poly-B-rep item per
/// tessellated primitive, numbered on from `tess`.
fn part(w: &mut BitWriter, scene: &MeshScene, plan: &Plan, m: usize, tess: &mut u32, ids: &mut Ids) {
    let mesh = scene.meshes.get(m);
    w.uint(PART_DEFINITION);
    graphics(w, mesh.and_then(|m| m.name.as_deref()), ids.next(), None);
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let prims: Vec<_> = plan.tessellations.iter().filter(|t| t.0 == m).filter_map(|&(_, p)| mesh?.primitives.get(p)).collect();
    for v in prims.iter().flat_map(|p| &p.positions) {
        for ((l, h), c) in lo.iter_mut().zip(hi.iter_mut()).zip(v) {
            *l = l.min(*c);
            *h = h.max(*c);
        }
    }
    if prims.is_empty() {
        (lo, hi) = ([1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]);
    }
    w.doubles(lo).doubles(hi);
    w.uint(prims.len() as u32);
    for p in prims {
        w.uint(RI_POLY_BREP_MODEL);
        graphics(w, None, ids.next(), plan.style_of(scene, p));
        *tess += 1;
        // No local coordinate system; tessellation (biased); not closed;
        // no user data.
        w.uint(0).uint(*tess).bit(false).uint(0);
    }
    // Markups (4 lists), views, user data.
    w.uint(0).uint(0).uint(0).uint(0).uint(0).uint(0);
}

fn occurrence(w: &mut BitWriter, scene: &MeshScene, plan: &Plan, o: &super::validate::Occ, ids: &mut Ids) {
    let node = o.node.and_then(|n| scene.nodes.get(n));
    w.uint(PRODUCT_OCCURRENCE);
    graphics(w, node.and_then(|n| n.name.as_deref()), ids.next(), None);
    let part = node.and_then(|n| n.mesh).and_then(|m| *plan.part_of_mesh.get(m)?).map_or(0, |p| p + 1);
    // Part (biased), no prototype, no external data, sons (raw).
    w.uint(part).uint(0).uint(0).uint(o.sons.len() as u32);
    for &s in &o.sons {
        w.uint(s);
    }
    // Not suppressed; ProductInformation: units from the scene, no flags.
    w.put(0, 8).bit(true).double(scene.units_in_mm).put(0, 8).int(LOAD_STATUS);
    let matrix = node.and_then(|n| n.matrix).filter(|m| *m != IDENTITY);
    w.bit(matrix.is_some());
    if let Some(m) = matrix {
        transformation(w, &m);
    }
    // No entity references, markups (4 lists), views, no entity filter,
    // display filters, scene display parameters, user data.
    w.uint(0).uint(0).uint(0).uint(0).uint(0).uint(0);
    w.bit(false).uint(0).uint(0).uint(0);
}

const IDENTITY: [f64; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// A rotation (possibly mirrored) times a uniform scale as a
/// `CartesianTransformation` [WD 7.4.11]; anything else (shear, non-uniform
/// scale) as a column-major `GeneralTransformation` [WD 7.4.9].
fn transformation(w: &mut BitWriter, m: &[f64; 16]) {
    let mut cols = [[0.0; 3]; 4];
    for (dst, src) in cols.iter_mut().zip(m.as_chunks::<4>().0) {
        for (d, s) in dst.iter_mut().zip(src) {
            *d = *s;
        }
    }
    let [x, y, z, t] = cols;
    let det = dot(x, cross(y, z));
    let s = det.abs().cbrt();
    let unit = |v: [f64; 3]| v.map(|c| c / s);
    let (rx, ry, rz) = (unit(x), unit(y), unit(z));
    let orthonormal = s > 1e-12
        && [(rx, rx, 1.0), (ry, ry, 1.0), (rz, rz, 1.0), (rx, ry, 0.0), (rx, rz, 0.0), (ry, rz, 0.0)]
            .iter()
            .all(|&(a, b, e)| (dot(a, b) - e).abs() < 1e-9);
    if !orthonormal {
        w.uint(GENERAL_TRANSFORMATION).doubles(m.iter().copied());
        return;
    }
    let rotated = det < 0.0 || rx != [1.0, 0.0, 0.0] || ry != [0.0, 1.0, 0.0];
    let scaled = (s - 1.0).abs() > 1e-12;
    let mut b = 0u8;
    if t != [0.0; 3] {
        b |= 0x01;
    }
    if rotated {
        b |= 0x02;
    }
    if det < 0.0 {
        b |= 0x04;
    }
    if scaled {
        b |= 0x08;
    }
    w.uint(CARTESIAN_TRANSFORMATION).put(u64::from(b), 8);
    if b & 0x01 != 0 {
        w.doubles(t);
    }
    if rotated {
        w.doubles(rx).doubles(ry);
    }
    if scaled {
        w.double(s);
    }
}

/// The model file: an empty schema, the units, the one root.
pub(super) fn model_file(scene: &MeshScene, plan: &Plan, structure: UniqueId) -> BitWriter {
    let mut w = BitWriter::default();
    w.uint(0).uint(MODEL_FILE);
    base(&mut w);
    w.bit(true).double(scene.units_in_mm).uint(1);
    for c in structure.0 {
        w.uint(c);
    }
    // Root occurrence, the last (biased), active; file structure 0's index
    // in the model file; no user data.
    w.uint(plan.occurrences.len() as u32).bit(true).uint(0).uint(0);
    w
}
