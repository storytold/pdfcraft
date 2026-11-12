//! The non-text content under the regions: images, paths and shadings, and the placement of the
//! glyphs still shown, checked independently of the interpreter.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::sync::Arc;

use pdfcraft_content::{Matrix, contains, overlaps, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::extract::{appearance_matrix, bbox_of, matrix_of, page_content, res_dict};
use super::util::{Ctx, image_codec};
use crate::verify::limits::{MAX_CLIP_SUBPATHS, MAX_DEPTH, MAX_GS_STACK, MAX_HOLES, MAX_PIXELS, MAX_RESEARCHES};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Finding {
    /// This many glyphs are placed inside the region.
    Text(usize),
    Image,
    Path,
    Shading,
    Missing,
    Unverifiable(&'static str),
}

#[derive(Clone)]
pub(super) struct Gs {
    pub(super) ctm: Matrix,
    /// Device-space boxes cut out of the clip (even-odd clips with a hole).
    pub(super) holes: Vec<[f64; 4]>,
}

pub(super) struct Geo<'a> {
    pub(super) doc: &'a Document,
    pub(super) ctx: &'a Ctx,
    pub(super) regions: &'a [[f64; 4]],
    pub(super) found: Vec<(usize, Finding)>,
    /// What `found` already holds: each finding is recorded once per region, so a page that
    /// paints thousands of times cannot grow the report (or the memory) with repeats.
    reported: HashSet<(usize, Finding)>,
    /// Hidden content already searched for images ([`Geo::has_image`]), by object: in any scope
    /// (`None`), where its search read no names below its own, or in each of these scopes, with the
    /// lowest level of names its search there read (`usize::MAX` while it is being searched).
    searched: HashMap<ObjRef, Option<HashMap<u64, usize>>>,
    /// Searches of hidden content in a scope other than the first it was searched in.
    researched: usize,
    /// What tells scopes apart ([`Geo::scope_of`]): keyed afresh for each check, so a file can't
    /// be built for two scopes to look alike.
    hasher: RandomState,
    /// Scopes given to content that isn't an object of the file, each its own.
    fresh: u64,
    /// Which `/XObject` names the content being checked looks `Do` up in: the page's
    /// ([`PAGE_SCOPE`]), none ([`NO_NAMES`]), or a form's with resources of its own, added to its
    /// caller's ([`Geo::scope_of`]).
    scope: u64,
    /// The resource names the forms on the way to this point are read with
    /// ([`crate::interp::MAX_HELD_BYTES`]).
    held: usize,
    /// What the objects forms give in their resources are ([`crate::interp::kind`]).
    kinds: crate::interp::Kinds,
}

/// The scope of the page's own content: the `/XObject` names its resources give.
const PAGE_SCOPE: u64 = 0;
/// The scope of content whose resources give no `/XObject` names, and look none up elsewhere.
const NO_NAMES: u64 = 1;

/// One level of the names hidden content looks an XObject up in ([`Geo::has_image`]): an
/// `/XObject` dictionary, the object whose resources give it, and the scope it makes with the
/// levels below it.
#[derive(Clone, Copy)]
struct Level<'d> {
    xobjects: &'d Dict,
    owner: Option<ObjRef>,
    scope: u64,
}

/// An object read in place where it is direct, or from the file where it is a reference, so a
/// dictionary a stream gives directly isn't copied to be read.
enum Read<'o> {
    Here(&'o Object),
    File(Arc<Object>),
}

impl Read<'_> {
    fn object(&self) -> &Object {
        match self {
            Read::Here(o) => o,
            Read::File(o) => o,
        }
    }
}

fn read<'o>(doc: &Document, o: &'o Object) -> Read<'o> {
    if matches!(o, Object::Ref(_)) { Read::File(doc.resolve(o)) } else { Read::Here(o) }
}

/// What still paints inside `regions` on a page of the output (the overlay boxes excepted):
/// images with pixels there, paths and shadings not clipped away from there.
pub(super) fn check_geometry(doc: &Document, page: &Dict, regions: &[[f64; 4]], ctx: &Ctx) -> Vec<(usize, Finding)> {
    let unverifiable = |why: &'static str| (0..regions.len()).map(|i| (i, Finding::Unverifiable(why))).collect();
    // The redaction boxes are painted on purpose.
    let Ok(data) = page_content(doc, page, ctx, true) else { return unverifiable("the page content can't be read in full") };
    let mut geo = Geo {
        doc,
        ctx,
        regions,
        found: Vec::new(),
        reported: HashSet::new(),
        searched: HashMap::new(),
        researched: 0,
        hasher: RandomState::new(),
        fresh: 0,
        scope: PAGE_SCOPE,
        held: 0,
        kinds: crate::interp::Kinds::new(),
    };
    let res = page.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    geo.run(&data, &res, Some(&res), Matrix::IDENTITY, Vec::new(), 0);
    geo.appearances(page, &res);
    geo.found
}

impl Geo<'_> {
    /// Record a finding for region `i`, once: a page that paints over a region a thousand
    /// times is one finding, not a thousand.
    fn report(&mut self, i: usize, f: Finding) {
        if self.reported.insert((i, f.clone())) {
            self.found.push((i, f));
        }
    }

    /// The appearances of the annotations still on the page, checked like the page itself: their
    /// content is placed with the appearance matrix (ISO 32000-2 §12.5.5) and swept for images,
    /// paths and shadings under the regions. An appearance that has content but can't be placed
    /// makes every region unverifiable: the redaction refuses such documents, so one in the
    /// output means what it paints can't be vouched for.
    fn appearances(&mut self, page: &Dict, res: &Dict) {
        let doc = self.doc;
        for a in crate::annots_of(doc, page) {
            // A direct dictionary has no reference of its own; it is checked like an indirect
            // one (what its appearance paints does not depend on how it is stored).
            let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
            // A redaction mark's appearance outlines the marked area on purpose (and applying
            // removes the mark whole): it is exempt like the boxes drawn over the regions.
            if d.name(b"Subtype") == Some(b"Redact") {
                continue;
            }
            let Some(ap) = d.get(b"AP").and_then(|x| doc.resolve(x).as_dict().cloned()) else { continue };
            let rect = crate::rect_of(doc, d.get(b"Rect"));
            for (_, state) in ap.iter() {
                let s = doc.resolve(state);
                let streams: Vec<(Option<ObjRef>, Stream)> = match &*s {
                    Object::Stream(st) => vec![(state.as_ref(), st.clone())],
                    Object::Dict(sub) => sub
                        .iter()
                        .filter_map(|(_, v)| match &*doc.resolve(v) {
                            Object::Stream(st) => Some((v.as_ref(), st.clone())),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                for (id, st) in streams {
                    // A stream shared by several annotations is checked at each annotation's
                    // own placement: where it lands comes from each `/Rect`, so the first
                    // placement tells nothing about the others. Repeated decodes spend the
                    // same bounded budget as everything else.
                    let Ok(data) = self.ctx.decode(self.doc, &st) else {
                        self.unverifiable("an annotation appearance can't be read in full");
                        continue;
                    };
                    if data.is_empty() {
                        continue;
                    }
                    let Some(ctm) = rect.as_ref().and_then(|r| appearance_matrix(doc, &st, r)) else {
                        self.unverifiable("an annotation appearance can't be placed, so what it paints can't be checked");
                        continue;
                    };
                    let own = st.dict.get(b"Resources").and_then(|x| doc.resolve(x).as_dict().cloned());
                    // An appearance with resources of its own is read with those alone.
                    self.scope = match &own {
                        None => PAGE_SCOPE,
                        Some(own) if own.get(b"XObject").is_none() => NO_NAMES,
                        Some(_) => self.scope_of(NO_NAMES, id),
                    };
                    let res = own.as_ref().unwrap_or(res);
                    self.run(&data, res, Some(res), ctm, Vec::new(), 1);
                    self.scope = PAGE_SCOPE;
                }
            }
        }
    }

    fn touching(&self, b: [f64; 4]) -> Vec<usize> {
        (0..self.regions.len()).filter(|&i| self.regions.get(i).is_some_and(|r| overlaps(*r, b, 0.0))).collect()
    }

    /// Does something painting over `within` reach region `i` through the clip? Only the part of
    /// the region the painting could touch has to be cut out.
    fn unclipped(&self, i: usize, within: [f64; 4], gs: &Gs) -> bool {
        let Some(r) = self.regions.get(i) else { return false };
        let reach = [r[0].max(within[0]), r[1].max(within[1]), r[2].min(within[2]), r[3].min(within[3])];
        !gs.holes.iter().any(|h| contains(*h, reach, 0.01))
    }

    /// Check content read with the names `res`, searching the patterns, soft masks and Type 3 fonts
    /// `searched` gives for images: the resources this content gives itself, if any. Its caller's
    /// were searched where they were given, and a copy of them in `res` would search them again on
    /// every draw.
    fn run(&mut self, data: &[u8], res: &Dict, searched: Option<&Dict>, ctm: Matrix, holes: Vec<[f64; 4]>, depth: usize) {
        if depth > MAX_DEPTH {
            for i in 0..self.regions.len() {
                self.report(i, Finding::Unverifiable("forms are nested too deep to check"));
            }
            return;
        }
        let xobjects = res_dict(self.doc, res, b"XObject");
        if let Some(searched) = searched {
            self.hidden_images(searched, &xobjects, depth);
        }
        let mut gs = Gs { ctm, holes };
        let mut stack: Vec<Gs> = Vec::new();
        // The current path in device space: its subpaths, and the clip operator it carries.
        let mut sub: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut clip: Option<bool> = None;
        for op in parse(data).ops {
            let o = op.op.as_slice();
            if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
                extend_path(&op, &gs.ctm, &mut sub, &mut clip);
                continue;
            }
            if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                self.paint_path(o, &mut gs, &sub, clip);
                sub.clear();
                clip = None;
                continue;
            }
            sub.clear();
            clip = None;
            match o {
                b"q" => {
                    // The saved state carries the holes with it, so an unbounded stack of them
                    // is unbounded memory: past the cap the walk says so instead of growing.
                    if stack.len() >= MAX_GS_STACK {
                        return self.unverifiable("the content stacks graphics states too deeply to check");
                    }
                    stack.push(gs.clone());
                }
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        gs.ctm = Matrix(m).then(&gs.ctm);
                    }
                }
                b"sh" => {
                    for i in 0..self.regions.len() {
                        if self.unclipped(i, [f64::MIN, f64::MIN, f64::MAX, f64::MAX], &gs) {
                            self.report(i, Finding::Shading);
                        }
                    }
                }
                b"BI" => {
                    for i in self.touching(gs.ctm.bbox([0.0, 0.0, 1.0, 1.0])) {
                        self.report(i, Finding::Image);
                    }
                }
                b"Do" => {
                    let Some(entry) = op.last_name().and_then(|n| xobjects.get(n)) else { continue };
                    let obj = self.doc.resolve(entry);
                    let Object::Stream(s) = &*obj else { continue };
                    match s.dict.name(b"Subtype") {
                        Some(b"Image") => {
                            for i in self.touching(gs.ctm.bbox([0.0, 0.0, 1.0, 1.0])) {
                                let Some(region) = self.regions.get(i) else { continue };
                                match image_cleared(self.doc, self.ctx, s, &gs.ctm, *region) {
                                    Ok(true) => {}
                                    Ok(false) => self.report(i, Finding::Image),
                                    Err(()) => self.report(i, Finding::Unverifiable("an image under the region can't be inspected")),
                                }
                            }
                        }
                        Some(b"Form") => self.form(s, entry.as_ref(), &gs, res, depth),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// A path-painting operator `o` ends the current path: report what it paints over a region,
    /// and let an even-odd clip add its holes to `gs`.
    fn paint_path(&mut self, o: &[u8], gs: &mut Gs, sub: &[Vec<(f64, f64)>], clip: Option<bool>) {
        let pts: Vec<(f64, f64)> = sub.iter().flatten().copied().collect();
        let bbox = pts.iter().fold(None, |b: Option<[f64; 4]>, &(u, v)| {
            Some(match b {
                None => [u, v, u, v],
                Some(b) => [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)],
            })
        });
        if o != b"n"
            && let Some(b) = bbox
        {
            for i in self.touching(b) {
                if self.unclipped(i, b, gs) {
                    self.report(i, Finding::Path);
                }
            }
        }
        // An even-odd clip whose subpaths are all axis-aligned rectangles lets through what an
        // odd number of the inner rectangles covers. Where a region lies under the outer box
        // and an odd number of inner ones, it is cut out of the clip; under an even number it
        // is painted and must be reported. Anything that is not a stock of disjoint rectangles
        // (a triangle, a rotated box, overlapping inners, too many of them) cannot be told
        // cheaply, so no hole is cut: what the clip lets through is then reported as paint
        // (fail closed). This is how redaction itself clips regions out of what it leaves.
        if clip != Some(true) {
            return;
        }
        let Some(rects) = sub.iter().map(|s| axis_aligned_rect(s)).collect::<Option<Vec<[f64; 4]>>>() else { return };
        if rects.len() < 2 || rects.len() > MAX_CLIP_SUBPATHS {
            return;
        }
        let (outer, inners) = (rects[0], &rects[1..]);
        // Inner rectangles must be disjoint, or their overlaps would change the parity the
        // reader draws with.
        let disjoint = inners.iter().enumerate().all(|(x, a)| inners.iter().skip(x + 1).all(|b| !overlaps(*a, *b, 0.0)));
        let regions: Vec<[f64; 4]> = self.regions.to_vec();
        let mut cut = Vec::new();
        for r in regions {
            if contains(outer, r, 0.0) && disjoint && inners.iter().filter(|b| contains(**b, r, 0.0)).count() % 2 == 1 {
                cut.push(r);
            }
        }
        if gs.holes.len() + cut.len() > MAX_HOLES {
            return self.unverifiable("the content cuts clips with too many pieces to check");
        }
        gs.holes.extend(cut);
    }

    fn unverifiable(&mut self, why: &'static str) {
        for i in 0..self.regions.len() {
            self.report(i, Finding::Unverifiable(why));
        }
    }

    fn form(&mut self, s: &Stream, id: Option<ObjRef>, gs: &Gs, parent: &Dict, depth: usize) {
        let (Ok(m), Ok(bbox)) = (matrix_of(self.doc, &s.dict), bbox_of(self.doc, &s.dict)) else {
            self.unverifiable("a form under the region has a malformed /Matrix or /BBox");
            return;
        };
        let fctm = m.then(&gs.ctm);
        if let Some(bb) = bbox
            && self.touching(fctm.bbox(bb)).is_empty()
        {
            return;
        }
        let Ok(data) = self.ctx.decode(self.doc, s) else {
            self.unverifiable("a form under the region can't be decoded in full");
            return;
        };
        // A name its own resources lack is looked up in its caller's, as the renderer does.
        let own = s.dict.get(b"Resources").and_then(|r| self.doc.resolve(r).as_dict().cloned());
        let held = match &own {
            Some(own) => crate::interp::held_bytes(self.doc, own).saturating_add(crate::interp::held_bytes(self.doc, parent)),
            None => crate::interp::held_bytes(self.doc, parent),
        };
        if self.held.saturating_add(held) > crate::interp::MAX_HELD_BYTES {
            self.unverifiable("the forms under the region hold too many resources to check");
            return;
        }
        let names = own.as_ref().map(|own| crate::interp::form_resources(self.doc, own, parent, &mut self.kinds));
        let res = names.as_ref().unwrap_or(parent);
        // Its own `/XObject` names come before its caller's; without them, it looks names up as its
        // caller does.
        let outer = self.scope;
        if own.as_ref().is_some_and(|own| own.get(b"XObject").is_some()) {
            self.scope = self.scope_of(outer, id);
        }
        self.held += held;
        self.run(&data, res, own.as_ref(), fctm, gs.holes.clone(), depth + 1);
        self.held = self.held.saturating_sub(held);
        self.scope = outer;
    }

    /// The scope of the `/XObject` names that object `id`'s resources give, looked up before those
    /// of scope `below`: one for each object over each scope (content that isn't an object of the
    /// file gets one of its own).
    fn scope_of(&mut self, below: u64, id: Option<ObjRef>) -> u64 {
        match id {
            Some(r) => self.hasher.hash_one((below, r)),
            None => {
                self.fresh += 1;
                self.hasher.hash_one((below, u64::MAX, self.fresh))
            }
        }
    }

    /// The scope of `names`, the `/XObject` names a font written directly in a `/Font` dictionary
    /// gives, looked up before those of scope `below`: made of what it names by reference, so the
    /// same names make the same scope however often the font is read. Another value given directly
    /// names nothing a lookup takes ([`Geo::look_up`]), but a stream, which gives it a scope of its
    /// own.
    fn scope_of_names(&mut self, below: u64, names: &Dict) -> u64 {
        let mut h = self.hasher.build_hasher();
        (below, u64::MAX - 1).hash(&mut h);
        for (name, value) in names.iter() {
            match value {
                Object::Ref(r) => (name, r).hash(&mut h),
                Object::Stream(_) => return self.scope_of(below, None),
                _ => {}
            }
        }
        h.finish()
    }

    /// Images drawn from tiling patterns, soft-mask groups and Type3 glyph procedures can't be
    /// placed or cleared by this check: if those `searched` gives reach any, every region is
    /// unverifiable. Each looks the XObjects it draws up in its own resources, then, for a glyph,
    /// in its font's `/Resources`, then in `xobjects`, those of the content they are given to, as
    /// the renderer does where that content paints with them.
    fn hidden_images(&mut self, searched: &Dict, xobjects: &Dict, depth: usize) {
        let doc = self.doc;
        // Each stream, with the Type 3 font it is a glyph of, where that font's resources give
        // `/XObject` names: its place in `fonts`.
        let mut streams: Vec<(Option<ObjRef>, Stream, Option<usize>)> = Vec::new();
        let mut fonts: Vec<(Option<ObjRef>, Dict)> = Vec::new();
        let take = |o: &Object, font: Option<usize>, streams: &mut Vec<(Option<ObjRef>, Stream, Option<usize>)>| {
            #[cfg(test)]
            HIDDEN_LOOKS.with(|c| c.set(c.get() + 1));
            if let Object::Stream(s) = &*doc.resolve(o) {
                streams.push((o.as_ref(), s.clone(), font));
            }
        };
        for (_, p) in res_dict(doc, searched, b"Pattern").iter() {
            take(p, None, &mut streams);
        }
        for (_, g) in res_dict(doc, searched, b"ExtGState").iter() {
            if let Some(Object::Dict(sm)) = doc.resolve(g).as_dict().and_then(|g| g.get(b"SMask")).map(|m| (*doc.resolve(m)).clone()).as_ref()
                && let Some(group) = sm.get(b"G")
            {
                take(group, None, &mut streams);
            }
        }
        for (_, f) in res_dict(doc, searched, b"Font").iter() {
            let Some(font) = doc.resolve(f).as_dict().cloned() else { continue };
            if font.name(b"Subtype") != Some(b"Type3") {
                continue;
            }
            let given = font.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).filter(|r| r.get(b"XObject").is_some());
            let place = given.map(|r| {
                fonts.push((f.as_ref(), res_dict(doc, &r, b"XObject")));
                fonts.len() - 1
            });
            if let Some(procs) = font.get(b"CharProcs").and_then(|c| doc.resolve(c).as_dict().cloned()) {
                for (_, p) in procs.iter() {
                    take(p, place, &mut streams);
                }
            }
        }
        let root = Level { xobjects, owner: None, scope: self.scope };
        let scopes: Vec<u64> = fonts
            .iter()
            .map(|(id, names)| if id.is_some() { self.scope_of(root.scope, *id) } else { self.scope_of_names(root.scope, names) })
            .collect();
        let font_levels: Vec<Level<'_>> = fonts.iter().zip(scopes).map(|((id, x), scope)| Level { xobjects: x, owner: *id, scope }).collect();
        for (id, s, font) in streams {
            let levels: Vec<Level<'_>> = std::iter::once(root).chain(font.and_then(|i| font_levels.get(i).copied())).collect();
            let below = levels.last().map_or(root.scope, |l| l.scope);
            let found = match id.map(|r| self.start(r, below)) {
                Some(Ok(Some(_))) => continue,
                Some(Err(())) => Err(()),
                Some(Ok(None)) | None => self.has_image(&s, id, &levels, depth + 1),
            };
            if let (Some(r), Ok((_, lowest))) = (id, &found) {
                self.finish(r, below, *lowest, levels.len());
            }
            match found {
                Ok((false, _)) => {}
                Ok((true, _)) => self.unverifiable("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"),
                Err(()) => self.unverifiable("a pattern, soft mask or Type3 glyph can't be read in full"),
            }
        }
    }

    /// Does the content of `s`, object `id`, draw an image, forms it draws included? It looks each
    /// XObject it draws up in its own resources, then through `levels` (innermost last), the names
    /// of what draws it, as the renderer does, without copying any. With the answer, the lowest of
    /// `levels` the search read (`levels.len()` when it read none of them): an answer that read
    /// none holds wherever `s` is drawn.
    fn has_image(&mut self, s: &Stream, id: Option<ObjRef>, levels: &[Level<'_>], depth: usize) -> Result<(bool, usize), ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        #[cfg(test)]
        HIDDEN_SEARCHES.with(|c| c.set(c.get() + 1));
        let data = self.ctx.decode(self.doc, s)?;
        let doc = self.doc;
        let resources = s.dict.get(b"Resources").map(|r| read(doc, r));
        let own = resources.as_ref().and_then(|r| r.object().as_dict()).and_then(|r| r.get(b"XObject")).map(|x| read(doc, x));
        let mut inner = levels.to_vec();
        if let Some(xobjects) = own.as_ref().and_then(|x| x.object().as_dict()) {
            // Drawn inside itself, it is read with the names it is already read with.
            let below = levels.last().map_or(NO_NAMES, |l| l.scope);
            if id.is_none() || levels.last().is_none_or(|l| l.owner != id) {
                let scope = self.scope_of(below, id);
                inner.push(Level { xobjects, owner: id, scope });
            }
        }
        let scope = inner.last().map_or(NO_NAMES, |l| l.scope);
        let mut lowest = levels.len();
        for op in parse(&data).ops {
            match op.op.as_slice() {
                b"BI" => return Ok((true, lowest)),
                b"Do" => {
                    let Some(name) = op.last_name() else { continue };
                    let (entry, read) = self.look_up(&inner, name);
                    lowest = lowest.min(read);
                    let Some(entry) = entry else { continue };
                    let Object::Stream(x) = &*doc.resolve(&entry) else { continue };
                    match x.dict.name(b"Subtype") {
                        Some(b"Image") => return Ok((true, lowest)),
                        Some(b"Form") => {
                            let id = entry.as_ref();
                            if let Some(r) = id
                                && let Some(read) = self.start(r, scope)?
                            {
                                lowest = lowest.min(read);
                                continue;
                            }
                            let (found, read) = self.has_image(x, id, &inner, depth + 1)?;
                            lowest = lowest.min(read);
                            if let Some(r) = id {
                                self.finish(r, scope, read, inner.len());
                            }
                            if found {
                                return Ok((true, lowest));
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok((false, lowest))
    }

    /// The XObject `name` names where hidden content draws it, through `levels` innermost first,
    /// as the renderer looks it up: in the first level giving it as a stream, an entry of another
    /// kind passed over ([`crate::interp::form_resources`]). With it, the lowest level read.
    fn look_up(&mut self, levels: &[Level<'_>], name: &[u8]) -> (Option<Object>, usize) {
        let mut read = levels.len();
        for (i, level) in levels.iter().enumerate().rev() {
            read = i;
            if let Some(entry) = level.xobjects.get(name)
                && crate::interp::kind(self.doc, entry, &mut self.kinds).stream
            {
                return (Some(entry.clone()), i);
            }
        }
        (None, read)
    }

    /// Whether to search object `r` in `scope` (`Ok(None)`), or, with the lowest level of names a
    /// search of it there read, to skip it: it was searched in that scope, or is being searched
    /// there (drawn inside itself), or was searched with an answer that holds in all. Searching an
    /// object again in another scope is counted, and past [`MAX_RESEARCHES`] it is an error.
    fn start(&mut self, r: ObjRef, scope: u64) -> Result<Option<usize>, ()> {
        match self.searched.get_mut(&r) {
            None => {
                self.searched.insert(r, Some(HashMap::from([(scope, usize::MAX)])));
                Ok(None)
            }
            Some(None) => Ok(Some(usize::MAX)),
            Some(Some(scopes)) => {
                if let Some(read) = scopes.get(&scope) {
                    return Ok(Some(*read));
                }
                if self.researched >= MAX_RESEARCHES {
                    return Err(());
                }
                self.researched += 1;
                scopes.insert(scope, usize::MAX);
                Ok(None)
            }
        }
    }

    /// Record a search of object `r` in `scope`, done, that read down to level `read` of names: an
    /// answer that read none of the `below` levels under its own holds in every scope.
    fn finish(&mut self, r: ObjRef, scope: u64, read: usize, below: usize) {
        if read >= below {
            self.searched.insert(r, None);
        } else if let Some(Some(scopes)) = self.searched.get_mut(&r) {
            scopes.insert(scope, read);
        }
    }
}

/// Extend the current path (`sub`: its subpaths in device space; `clip`: the clip operator it
/// carries) by one construction operator.
fn extend_path(op: &pdfcraft_content::Op, ctm: &Matrix, sub: &mut Vec<Vec<(f64, f64)>>, clip: &mut Option<bool>) {
    let o = op.op.as_slice();
    let at = |x: f64, y: f64| ctm.apply(x, y);
    // Each takes its numbers from the end, as the renderer reads them: a surplus before them is
    // ignored, not read as a point.
    let points: Option<Vec<(f64, f64)>> = match o {
        b"l" => op.nums::<2>().map(|[x, y]| vec![at(x, y)]),
        b"c" => op.nums::<6>().map(|[x1, y1, x2, y2, x3, y3]| vec![at(x1, y1), at(x2, y2), at(x3, y3)]),
        b"v" | b"y" => op.nums::<4>().map(|[x1, y1, x2, y2]| vec![at(x1, y1), at(x2, y2)]),
        _ => None,
    };
    match o {
        b"m" => {
            if let Some([x, y]) = op.nums::<2>() {
                sub.push(vec![at(x, y)]);
            }
        }
        b"l" | b"c" | b"v" | b"y" => {
            if let (Some(cur), Some(points)) = (sub.last_mut(), points) {
                cur.extend(points);
            }
        }
        b"re" => {
            if let Some([x, y, w, h]) = op.nums::<4>() {
                sub.push(vec![at(x, y), at(x + w, y), at(x + w, y + h), at(x, y + h)]);
            }
        }
        b"W" => *clip = Some(false),
        b"W*" => *clip = Some(true),
        _ => {}
    }
}

/// The axis-aligned rectangle a four-corner subpath draws, in device space: `None` for anything
/// else (open paths, more or fewer corners, skewed or rotated boxes, non-finite coordinates).
fn axis_aligned_rect(pts: &[(f64, f64)]) -> Option<[f64; 4]> {
    // `re` leaves its rectangle open: the four corners are the whole subpath. A rectangle drawn
    // by hand closes by repeating the first point at the end. Both spell the same four corners.
    let pts = match pts {
        [a, _, _, _, e] if a == e => &pts[..4],
        _ => pts,
    };
    let [p0, p1, p2, p3] = pts else { return None };
    let finite = |p: &(f64, f64)| p.0.is_finite() && p.1.is_finite();
    if !finite(p0) || !finite(p1) || !finite(p2) || !finite(p3) {
        return None;
    }
    // The edges alternate between horizontal and vertical, whichever corner they start from and
    // whichever way the rectangle is wound.
    let flat = |a: &(f64, f64), b: &(f64, f64)| a.1 == b.1 && a.0 != b.0;
    let steep = |a: &(f64, f64), b: &(f64, f64)| a.0 == b.0 && a.1 != b.1;
    let even = flat(p0, p1) && steep(p1, p2) && flat(p2, p3) && steep(p3, p0);
    let odd = steep(p0, p1) && flat(p1, p2) && steep(p2, p3) && flat(p3, p0);
    if !even && !odd {
        return None;
    }
    let (x0, x1) = (p0.0.min(p2.0), p0.0.max(p2.0));
    let (y0, y1) = (p0.1.min(p2.1), p0.1.max(p2.1));
    (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
}

/// Are the samples of image `s` whose centres fall in `region` all clear (zero; for image masks,
/// the sample that does not paint)? `Err` when the image can't be inspected.
#[allow(clippy::result_unit_err)]
pub(super) fn image_cleared(doc: &Document, ctx: &Ctx, s: &Stream, ctm: &Matrix, region: [f64; 4]) -> Result<bool, ()> {
    if image_codec(&s.dict) {
        // Still encoded pixels: nothing was cleared.
        return Ok(false);
    }
    let (w, h) = (s.dict.int(b"Width").ok_or(())?, s.dict.int(b"Height").ok_or(())?);
    if w <= 0 || h <= 0 || (w as u64).saturating_mul(h as u64) > MAX_PIXELS {
        return Err(());
    }
    let (w, h) = (usize::try_from(w).map_err(|_| ())?, usize::try_from(h).map_err(|_| ())?);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let (ncomp, bpc) = if mask {
        (1, 1)
    } else {
        let cs = doc.resolve(s.dict.get(b"ColorSpace").ok_or(())?);
        let name = match &*cs {
            Object::Name(n) => n.clone(),
            Object::Array(a) => a.first().and_then(Object::as_name).ok_or(())?.to_vec(),
            _ => return Err(()),
        };
        let n = match name.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
            b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
            b"DeviceCMYK" | b"CMYK" => 4,
            // One component per colorant name.
            b"DeviceN" => doc.resolve(cs.as_array().and_then(|a| a.get(1)).ok_or(())?).as_array().ok_or(())?.len(),
            b"ICCBased" => match &*doc.resolve(cs.as_array().and_then(|a| a.get(1)).ok_or(())?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N").ok_or(())?).map_err(|_| ())?,
                _ => return Err(()),
            },
            _ => return Err(()),
        };
        (n, usize::try_from(s.dict.int(b"BitsPerComponent").ok_or(())?).map_err(|_| ())?)
    };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) || ncomp == 0 || ncomp > 32 {
        return Err(());
    }
    let data = ctx.decode(doc, s)?;
    let bits = ncomp.checked_mul(bpc).ok_or(())?;
    let row = w.checked_mul(bits).ok_or(())?.div_ceil(8);
    if data.len() < row.checked_mul(h).ok_or(())? {
        return Err(());
    }
    let inverted = s.dict.get(b"Decode").is_some_and(|d| {
        let d = doc.resolve(d);
        d.as_array().and_then(|a| a.first()).is_some_and(|first| doc.resolve(first).as_f64() == Some(1.0))
    });
    let want = u8::from(mask && !inverted);
    for j in 0..h {
        let v = 1.0 - (j as f64 + 0.5) / h as f64;
        // Rows that don't reach the region hold nothing to check.
        let ((ax, ay), (bx, by)) = (ctm.apply(0.0, v), ctm.apply(1.0, v));
        if !overlaps(region, [ax.min(bx), ay.min(by) - 0.01, ax.max(bx), ay.max(by) + 0.01], 0.0) {
            continue;
        }
        for i in 0..w {
            let (x, y) = ctm.apply((i as f64 + 0.5) / w as f64, v);
            if x < region[0] || x > region[2] || y < region[1] || y > region[3] {
                continue;
            }
            let start = j.checked_mul(row).and_then(|v| v.checked_mul(8)).and_then(|v| v.checked_add(i.checked_mul(bits)?)).ok_or(())?;
            for bit in start..start.checked_add(bits).ok_or(())? {
                let byte = data.get(bit / 8).ok_or(())?;
                if (byte >> (7 - bit % 8)) & 1 != want {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
thread_local! {
    /// How many pattern cells, soft-mask groups and glyph procedures the image search has looked at.
    pub(crate) static HIDDEN_LOOKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many searches of hidden content, and of forms inside it, the image search has made.
    pub(crate) static HIDDEN_SEARCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
