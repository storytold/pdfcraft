//! Flatten tiled pages: a page made of many image tiles becomes one picture, its text kept.
//!
//! Slides exported from design tools (InDesign, Illustrator, Canva, Keynote) often carry what
//! looks like one picture as a background image many times the screen's size plus hundreds of
//! clipped patches drawn over it (`re W n … /Im Do`), because the tool flattened transparency by
//! cutting the page into tiles. The optimizer measures each tile on its own: every one is small,
//! every one is drawn at about the density the page asks for, and nothing is resampled, although
//! the page as a whole is many times sharper than any screen shows.
//!
//! [`detect_tiled_pages`] finds such pages: a page that draws at least `min_images` images, at
//! least half of them smaller than a quarter of the page. For each, flattening
//!
//! 1. splits the page content into its text objects (`BT … ET`) and everything else,
//! 2. draws the page *without* its text, at `max_dimension` pixels along its longer side, with
//!    the renderer (so clipping, layering, masks and blending come out as they are shown),
//! 3. stores that as one JPEG and draws it, then the text objects again over it, with the
//!    painting operators of everything else turned into `n` so the text keeps the graphics
//!    state (position, colour, clip, font) it had.
//!
//! The text stays text: searchable, selectable, sharp at any zoom. What it can't keep: the
//! stacking of text under later drawing (text is drawn last), so a page where the artwork covers
//! some text looks different, and the clip that text in a clipping render mode (4 to 7) puts on
//! what is drawn after it. Such a text object stays in the picture (so the clip still shapes what
//! the renderer draws) and in the overlay as `3 Tr` text, which paints nothing but stays
//! searchable and selectable. A page is left as it is when it:
//!
//! - is rotated or has a `/UserUnit`, has a comment or other markup annotation (the renderer
//!   draws annotations, which would then show twice), or an unreadable content stream;
//! - draws a form XObject that holds text (the text isn't in the page's own content);
//! - would not get smaller (the picture is not smaller than the images it replaces);
//! - is in an encrypted document (the renderer is given the document as written).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_content::{Op, parse, serialize_ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_model::Page;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, effective_scale};

use crate::images::{PageDraws, content_bytes, jpeg, page_draws};
use crate::{FlattenSettings, OptimizeError, Report, Stage};

/// Most tiles whose neighbours are counted when describing a page (the count is quadratic).
const MAX_TOUCH_TILES: usize = 600;

/// Largest longest-side dimension a flattened slide picture requests from the renderer.
const MAX_FLATTEN_DIMENSION: u32 = 8192;

/// Deepest nested Type 3 font or form XObject chain followed when checking glyph resources.
const MAX_TYPE3_DEPTH: usize = 12;

/// Most page `/XObject` entries followed while walking Type 3 glyph resources (a backstop against a
/// hostile resource dictionary). What a font's own glyph programs do is bounded by the page's
/// `DecodeBudget`, [`MAX_TYPE3_DEPTH`] and one walk per program per font, so a font with tens of
/// thousands of glyphs is walked like any other instead of being left unflattened.
const MAX_TYPE3_STEPS: usize = 10_000;

/// Why a tiled page is left as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The page is displayed rotated.
    Rotated,
    /// The page has a `/UserUnit` other than 1.
    UserUnit,
    /// The page has a markup annotation (not a link).
    Annotations,
    /// A form XObject the page draws holds text.
    FormText,
    /// The content stream, or a form's, can't be read back exactly.
    UnreadableContent,
    /// The picture would not be smaller than the images it replaces.
    NoGain,
    /// The renderer could not draw the page.
    RenderFailed,
    /// The document is encrypted.
    Encrypted,
}

impl Skip {
    /// A short phrase for reports.
    pub fn reason(self) -> &'static str {
        match self {
            Skip::Rotated => "rotated page",
            Skip::UserUnit => "page has a UserUnit",
            Skip::Annotations => "page has markup annotations",
            Skip::FormText => "a form XObject holds text",
            Skip::UnreadableContent => "content stream not readable",
            Skip::NoGain => "would not get smaller",
            Skip::RenderFailed => "page could not be rendered",
            Skip::Encrypted => "encrypted document",
        }
    }
}

/// A page made of many image tiles.
#[derive(Clone, Debug, PartialEq)]
pub struct TiledPage {
    /// Position in page order, from 0.
    pub index: usize,
    pub page: ObjRef,
    /// Images the page draws (every drawing counts).
    pub images: usize,
    /// Of those, the ones drawn smaller than a quarter of the page.
    pub tiles: usize,
    /// Pairs of tiles that share an edge: the grid the tiles form (0 when there are over
    /// [`MAX_TOUCH_TILES`] tiles).
    pub touching: usize,
    /// The drawn images' total area over the page's: about 1 for a picture cut in pieces, more
    /// when pieces are drawn over a background.
    pub coverage: f64,
    /// Why flattening leaves the page as it is, if it does.
    pub skip: Option<Skip>,
}

struct Scanned {
    info: TiledPage,
    page: Page,
    /// The page's operators and any page XObjects its Type 3 glyphs require, when flattenable.
    ops: Result<(Vec<Op>, Dict), Skip>,
    /// Encoded bytes of the distinct images the page draws (excluding any kept for Type 3 glyphs).
    image_bytes: usize,
}

/// The rectangular area of `[llx, lly, urx, ury]`, clamped to zero for inverted coordinates.
fn area(r: &[f64; 4]) -> f64 {
    ((r[2] - r[0]).max(0.0)) * ((r[3] - r[1]).max(0.0))
}

/// Whether the page has an annotation that is drawn and isn't a Link or Popup (such as markup, stamps, or form widgets).
fn has_markup_annots(doc: &Document, page: &Page) -> bool {
    let Some(annots) = page.dict.get(b"Annots").map(|a| doc.resolve(a)) else { return false };
    let Some(list) = annots.as_array() else { return false };
    list.iter().any(|a| {
        let a = doc.resolve(a);
        a.as_dict().is_some_and(|d| !matches!(d.name(b"Subtype"), Some(b"Link" | b"Popup")))
    })
}

/// Walks Type 3 fonts reachable from a page's resources so that any page `/XObject` entries
/// referenced by Type 3 glyph procedures (when not provided by the font's or glyph's own
/// `/Resources`) are preserved when replacing the page's `/XObject` dictionary.
struct Type3Walk<'a> {
    doc: &'a Document,
    page_xobjects: Dict,
    budget: DecodeBudget,
    seen_fonts: HashSet<ObjRef>,
    kept: Dict,
    kept_refs: HashSet<ObjRef>,
    steps: usize,
}

impl<'a> Type3Walk<'a> {
    /// Charges `n` steps against [`MAX_TYPE3_STEPS`], aborting with [`Skip::UnreadableContent`] if exceeded.
    fn charge(&mut self, n: usize) -> Result<(), Skip> {
        self.steps = self.steps.saturating_add(n);
        if self.steps > MAX_TYPE3_STEPS {
            return Err(Skip::UnreadableContent);
        }
        Ok(())
    }

    /// Charges a single step against the traversal budget.
    fn step(&mut self) -> Result<(), Skip> {
        self.charge(1)
    }

    /// Resolves `obj` if present and returns its dictionary, `None` for null or missing, or an error if unreadable.
    fn dict_opt(doc: &Document, obj: Option<&Object>) -> Result<Option<Dict>, Skip> {
        let Some(o) = obj else { return Ok(None) };
        match &*doc.resolve(o) {
            Object::Null => Ok(None),
            Object::Dict(d) => Ok(Some(d.clone())),
            _ => Err(Skip::UnreadableContent),
        }
    }

    /// Walks all font entries in `fonts` within [`MAX_TYPE3_DEPTH`], checking for Type 3 font definitions.
    fn check_fonts(&mut self, fonts: &Dict, local_xobjects: &[Dict], depth: usize) -> Result<(), Skip> {
        if depth > MAX_TYPE3_DEPTH {
            return Err(Skip::UnreadableContent);
        }
        for (_, font_obj) in fonts.iter() {
            self.check_font(font_obj, local_xobjects, depth)?;
        }
        Ok(())
    }

    /// Inspects a font object, traversing its glyph procedures and resources if it is a Type 3 font.
    fn check_font(&mut self, font_obj: &Object, local_xobjects: &[Dict], depth: usize) -> Result<(), Skip> {
        self.step()?;
        let resolved = self.doc.resolve(font_obj);
        let Some(font_dict) = resolved.as_dict() else { return Ok(()) };
        if font_dict.name(b"Subtype") != Some(b"Type3") {
            return Ok(());
        }
        if local_xobjects.is_empty() && font_obj.as_ref().is_some_and(|r| !self.seen_fonts.insert(r)) {
            return Ok(());
        }
        let font_res = Self::dict_opt(self.doc, font_dict.get(b"Resources"))?;
        let font_xobj = Self::dict_opt(self.doc, font_res.as_ref().and_then(|r| r.get(b"XObject")))?;
        self.charge(local_xobjects.len())?;
        let mut font_scopes = local_xobjects.to_vec();
        if let Some(fx) = font_xobj {
            font_scopes.push(fx);
        }
        if let Some(inner_fonts) = Self::dict_opt(self.doc, font_res.as_ref().and_then(|r| r.get(b"Font")))? {
            self.check_fonts(&inner_fonts, &font_scopes, depth + 1)?;
        }
        let char_procs = Self::dict_opt(self.doc, font_dict.get(b"CharProcs"))?.ok_or(Skip::UnreadableContent)?;
        // A glyph program this font shares between several glyphs is walked once.
        let mut walked = HashSet::new();
        for (_, proc_obj) in char_procs.iter() {
            let resolved_proc = self.doc.resolve(proc_obj);
            let Object::Stream(proc_stream) = &*resolved_proc else { return Err(Skip::UnreadableContent) };
            if proc_obj.as_ref().is_some_and(|r| !walked.insert(r)) {
                continue;
            }
            let proc_res = Self::dict_opt(self.doc, proc_stream.dict.get(b"Resources"))?;
            let proc_xobj = Self::dict_opt(self.doc, proc_res.as_ref().and_then(|r| r.get(b"XObject")))?;
            let mut proc_scopes;
            let scopes_ref: &[Dict] = if let Some(px) = proc_xobj {
                self.charge(font_scopes.len())?;
                proc_scopes = font_scopes.clone();
                proc_scopes.push(px);
                &proc_scopes
            } else {
                &font_scopes
            };
            if let Some(inner_fonts) = Self::dict_opt(self.doc, proc_res.as_ref().and_then(|r| r.get(b"Font")))? {
                self.check_fonts(&inner_fonts, scopes_ref, depth + 1)?;
            }
            self.walk_stream(proc_stream, scopes_ref, depth + 1)?;
        }
        Ok(())
    }

    /// Parses a glyph procedure stream and inspects any `Do` invocations up to [`MAX_TYPE3_DEPTH`].
    fn walk_stream(&mut self, stream: &Stream, local_xobjects: &[Dict], depth: usize) -> Result<(), Skip> {
        if depth > MAX_TYPE3_DEPTH {
            return Err(Skip::UnreadableContent);
        }
        let data = self.budget.decode(stream).map_err(|_| Skip::UnreadableContent)?;
        let parsed = parse(&data);
        if parsed.skipped > 0 || parsed.ambiguous_images > 0 {
            return Err(Skip::UnreadableContent);
        }
        for op in &parsed.ops {
            if op.is("Do") {
                let name = op.name(0).ok_or(Skip::UnreadableContent)?;
                self.walk_do(name, local_xobjects, depth)?;
            }
        }
        Ok(())
    }

    /// Checks an XObject invoked via `Do` in a glyph procedure, retaining it if resolved from page-level XObjects.
    fn walk_do(&mut self, name: &[u8], local_xobjects: &[Dict], depth: usize) -> Result<(), Skip> {
        let local_hit = local_xobjects.iter().rev().find_map(|d| d.get(name)).cloned();
        let entry = if let Some(e) = local_hit {
            Some(e)
        } else {
            self.step()?;
            if let Some(e) = self.page_xobjects.get(name).cloned() {
                if let Some(r) = e.as_ref() {
                    self.kept_refs.insert(r);
                }
                self.kept.set(name.to_vec(), e.clone());
                Some(e)
            } else {
                None
            }
        };
        let Some(entry) = entry else { return Ok(()) };
        let resolved_x = self.doc.resolve(&entry);
        let Object::Stream(s) = &*resolved_x else { return Ok(()) };
        if s.dict.name(b"Subtype") != Some(b"Form") {
            return Ok(());
        }
        if !self.budget.enter_form() {
            return Err(Skip::UnreadableContent);
        }
        let form_res = Self::dict_opt(self.doc, s.dict.get(b"Resources"))?;
        let form_xobj = Self::dict_opt(self.doc, form_res.as_ref().and_then(|r| r.get(b"XObject")))?;
        let mut form_scopes;
        let scopes_ref: &[Dict] = if let Some(fx) = form_xobj {
            self.charge(local_xobjects.len())?;
            form_scopes = local_xobjects.to_vec();
            form_scopes.push(fx);
            &form_scopes
        } else {
            local_xobjects
        };
        if let Some(inner_fonts) = Self::dict_opt(self.doc, form_res.as_ref().and_then(|r| r.get(b"Font")))? {
            self.check_fonts(&inner_fonts, scopes_ref, depth + 1)?;
        }
        self.walk_stream(s, scopes_ref, depth + 1)
    }
}

/// Page `/XObject` entries (and their `ObjRef`s) that retained Type 3 glyph procedures resolve
/// through the page's resources.
fn type3_page_xobjects(doc: &Document, page: ObjRef) -> Result<(Dict, HashSet<ObjRef>), Skip> {
    let Some(res) = crate::images::page_resources(doc, page) else { return Ok((Dict::new(), HashSet::new())) };
    let Some(page_xobjects) = Type3Walk::dict_opt(doc, res.get(b"XObject"))? else { return Ok((Dict::new(), HashSet::new())) };
    if page_xobjects.is_empty() {
        return Ok((Dict::new(), HashSet::new()));
    }
    let Some(fonts) = Type3Walk::dict_opt(doc, res.get(b"Font"))? else { return Ok((Dict::new(), HashSet::new())) };
    let mut walk = Type3Walk {
        doc,
        page_xobjects,
        budget: DecodeBudget::page(),
        seen_fonts: HashSet::new(),
        kept: Dict::new(),
        kept_refs: HashSet::new(),
        steps: 0,
    };
    walk.check_fonts(&fonts, &[], 0)?;
    Ok((walk.kept, walk.kept_refs))
}

/// The page's operators and preserved Type 3 XObjects if it can be flattened, or why not.
fn inspect(doc: &Document, page: &Page, draws: &PageDraws) -> Result<(Vec<Op>, Dict, HashSet<ObjRef>), Skip> {
    if page.rotation(doc) != 0 {
        return Err(Skip::Rotated);
    }
    if (page.user_unit(doc) - 1.0).abs() > 1e-9 {
        return Err(Skip::UserUnit);
    }
    if has_markup_annots(doc, page) {
        return Err(Skip::Annotations);
    }
    if draws.form_text {
        return Err(Skip::FormText);
    }
    if !draws.complete {
        return Err(Skip::UnreadableContent);
    }
    let contents = page.dict.get(b"Contents").ok_or(Skip::UnreadableContent)?;
    let data = content_bytes(doc, contents, &mut DecodeBudget::page()).ok_or(Skip::UnreadableContent)?;
    let parsed = parse(&data);
    if parsed.skipped > 0 || parsed.ambiguous_images > 0 {
        return Err(Skip::UnreadableContent);
    }
    let (kept_xobjects, kept_refs) = type3_page_xobjects(doc, page.obj)?;
    Ok((parsed.ops, kept_xobjects, kept_refs))
}

/// Every page that draws at least `min_images` images, most of them small, with how it is made
/// and whether flattening could handle it.
fn scan(doc: &Document, min_images: usize) -> Vec<Scanned> {
    let pages = pdfcraft_model::pages(doc);
    let mut total_uses: HashMap<ObjRef, usize> = HashMap::new();
    let mut page_data = Vec::with_capacity(pages.len());
    for (index, page) in pages.into_iter().enumerate() {
        let draws = page_draws(doc, page.obj);
        let mut seen = HashSet::new();
        for d in &draws.draws {
            if seen.insert(d.image) {
                *total_uses.entry(d.image).or_default() += 1;
            }
        }
        page_data.push((index, page, draws));
    }

    let mut candidate_pages = Vec::new();
    let mut planned_uses: HashMap<ObjRef, usize> = HashMap::new();
    for (index, page, draws) in page_data {
        if draws.draws.len() < min_images.max(1) {
            continue;
        }
        let crop = page.crop(doc);
        let page_area = area(&crop);
        if !page_area.is_finite() || page_area <= 0.0 {
            continue;
        }
        let tiles: Vec<&[f64; 4]> = draws.draws.iter().map(|d| &d.rect).filter(|r| area(r) < 0.25 * page_area).collect();
        if tiles.len().saturating_mul(2) < draws.draws.len() {
            continue;
        }
        let tol = 0.002 * (crop[2] - crop[0]).max(crop[3] - crop[1]);
        let mut touching = 0usize;
        if tiles.len() <= MAX_TOUCH_TILES {
            for (i, a) in tiles.iter().enumerate() {
                for b in tiles.get(i + 1..).unwrap_or_default() {
                    let overlap_y = a[3].min(b[3]) - a[1].max(b[1]);
                    let overlap_x = a[2].min(b[2]) - a[0].max(b[0]);
                    let side = ((a[2] - b[0]).abs() <= tol || (b[2] - a[0]).abs() <= tol) && overlap_y > tol;
                    let stacked = ((a[3] - b[1]).abs() <= tol || (b[3] - a[1]).abs() <= tol) && overlap_x > tol;
                    touching = touching.saturating_add(usize::from(side || stacked));
                }
            }
        }
        let coverage = draws.draws.iter().map(|d| area(&d.rect)).filter(|a| a.is_finite()).sum::<f64>() / page_area;
        let inspected = inspect(doc, &page, &draws);
        if inspected.is_ok() {
            let kept_refs = inspected.as_ref().map(|(_, _, r)| r);
            let mut seen = HashSet::new();
            for d in &draws.draws {
                if !kept_refs.is_some_and(|k| k.contains(&d.image)) && seen.insert(d.image) {
                    *planned_uses.entry(d.image).or_default() += 1;
                }
            }
        }
        candidate_pages.push((index, page, draws, tiles.len(), touching, coverage, inspected));
    }

    let mut out = Vec::with_capacity(candidate_pages.len());
    for (index, page, draws, tiles_len, touching, coverage, inspected) in candidate_pages {
        let kept_refs = inspected.as_ref().map(|(_, _, r)| r);
        let mut seen = HashSet::new();
        let image_bytes = draws
            .draws
            .iter()
            .filter(|d| !kept_refs.is_some_and(|k| k.contains(&d.image)) && seen.insert(d.image))
            .map(|d| {
                let total = total_uses.get(&d.image).copied().unwrap_or(1);
                let planned = planned_uses.get(&d.image).copied().unwrap_or(0);
                if planned < total {
                    return 0;
                }
                match &*doc.get(d.image) {
                    Object::Stream(s) => s.raw.len() / planned.max(1),
                    _ => 0,
                }
            })
            .fold(0usize, usize::saturating_add);
        let ops = inspected.map(|(ops, kept, _)| (ops, kept));
        let info = TiledPage {
            index,
            page: page.obj,
            images: draws.draws.len(),
            tiles: tiles_len,
            touching,
            coverage,
            skip: ops.as_ref().err().copied(),
        };
        out.push(Scanned { info, page, ops, image_bytes });
    }
    out
}

/// The pages of `doc` made of many image tiles: at least `min_images` images drawn, at least
/// half of them smaller than a quarter of the page. Each says whether flattening can handle it
/// ([`TiledPage::skip`]).
pub fn detect_tiled_pages(doc: &Document, min_images: usize) -> Vec<TiledPage> {
    scan(doc, min_images).into_iter().map(|s| s.info).collect()
}

/// The operators of a page split in two: everything but its text objects (what the renderer
/// draws to make the picture), and its text objects with the state operators around them but no
/// painting (so they come out as they were).
fn split(ops: &[Op]) -> (Vec<Op>, Vec<Op>) {
    let (mut background, mut text) = (Vec::with_capacity(ops.len()), Vec::new());
    let mut text_block = Vec::new();
    let mut in_text = false;
    for op in ops {
        if op.is("BT") {
            in_text = true;
            text_block.clear();
            text_block.push(op.clone());
            continue;
        }
        if in_text {
            text_block.push(op.clone());
            if op.is("ET") {
                in_text = false;
                let has_clip = text_block.iter().any(|o| o.is("Tr") && o.num(0).is_some_and(|m| m >= 4.0));
                if has_clip {
                    // Clipping text defines the boundary of an image or shading in the background.
                    // Keep the clip in the background so the renderer paints it accurately.
                    background.extend(text_block.iter().cloned());
                    // In the foreground text overlay, convert Tr >= 4 to Tr 3 (invisible text)
                    // so it does not draw duplicate pixels, yet stays searchable and selectable.
                    for o in &text_block {
                        if o.is("Tr") && o.num(0).is_some_and(|m| m >= 4.0) {
                            text.push(Op::new("Tr", vec![Object::Int(3)]));
                        } else {
                            text.push(o.clone());
                        }
                    }
                } else {
                    text.extend(text_block.iter().cloned());
                }
            }
            continue;
        }
        background.push(op.clone());
        match op.op.as_slice() {
            // Painting becomes "end the path": a clip set by `W` before it still applies.
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" => text.push(Op::new("n", Vec::new())),
            b"sh" | b"Do" | b"BI" => {}
            _ => text.push(op.clone()),
        }
    }
    (background, text)
}

/// A page being flattened: its original `/Contents`, its text operators and its box.
struct Plan {
    index: usize,
    page: ObjRef,
    crop: [f64; 4],
    original: Option<Object>,
    text: Vec<Op>,
    kept_xobjects: Dict,
    images: usize,
    image_bytes: usize,
}

/// Restores original page content streams if any flattening step fails or is cancelled.
fn restore(doc: &mut Document, plans: &[Plan]) -> Result<(), OptimizeError> {
    for p in plans {
        if let Some(original) = &p.original {
            doc.update_dict(p.page, |d| d.set(b"Contents".to_vec(), original.clone()))?;
        }
    }
    Ok(())
}

/// A name for the picture that the page's image resources don't use yet.
fn fresh_name(doc: &Document, page: ObjRef) -> Vec<u8> {
    let res = crate::images::page_resources(doc, page);
    let xobjects = res.and_then(|r| r.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()));
    let mut k = 0usize;
    loop {
        let name = if k == 0 { b"PCFlat".to_vec() } else { format!("PCFlat{k}").into_bytes() };
        if !xobjects.as_ref().is_some_and(|x| x.contains(&name)) {
            return name;
        }
        k = k.saturating_add(1);
    }
}

/// Replace the page's XObjects with the flattened picture (retaining any `keep` entries required
/// by Type 3 glyph procedures), detaching the hundreds of tiles.
fn set_only_xobject(doc: &mut Document, page: ObjRef, name: &[u8], image: ObjRef, keep: &Dict) -> Result<(), OptimizeError> {
    let mut x = keep.clone();
    x.set(name.to_vec(), Object::Ref(image));
    let x_ref = doc.add(Object::Dict(x));
    let res = doc.get(page).as_dict().and_then(|d| d.get(b"Resources").cloned());
    match res {
        Some(Object::Ref(r)) => {
            let mut d = doc.get(r).as_dict().cloned().unwrap_or_default();
            d.set(b"XObject".to_vec(), Object::Ref(x_ref));
            let new_res = doc.add(Object::Dict(d));
            doc.update_dict(page, |p| p.set(b"Resources".to_vec(), Object::Ref(new_res)))?;
        }
        Some(Object::Dict(mut d)) => {
            d.set(b"XObject".to_vec(), Object::Ref(x_ref));
            doc.update_dict(page, |p| p.set(b"Resources".to_vec(), Object::Dict(d)))?;
        }
        _ => {
            let mut d = crate::images::page_resources(doc, page).unwrap_or_default();
            d.set(b"XObject".to_vec(), Object::Ref(x_ref));
            doc.update_dict(page, |p| p.set(b"Resources".to_vec(), Object::Dict(d)))?;
        }
    }
    Ok(())
}

/// Premultiplied RGBA8 as RGB8 over white.
fn rgb_over_white(rgba: &[u8]) -> Option<Vec<u8>> {
    let cap = (rgba.len() / 4).checked_mul(3)?;
    let mut out = Vec::new();
    out.try_reserve_exact(cap).ok()?;
    for px in rgba.chunks_exact(4) {
        let [r, g, b, a] = px else { continue };
        let hole = 255 - *a;
        out.extend_from_slice(&[r.saturating_add(hole), g.saturating_add(hole), b.saturating_add(hole)]);
    }
    Some(out)
}

/// Renders a page candidate to an image and replaces its content stream if the flattened JPEG is smaller.
fn finish_plan(doc: &mut Document, renderer: &mut PageRenderer, plan: &Plan, settings: &FlattenSettings) -> Result<bool, OptimizeError> {
    let (w, h) = (plan.crop[2] - plan.crop[0], plan.crop[3] - plan.crop[1]);
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return Ok(false);
    }
    let max_dim = settings.max_dimension.clamp(16, MAX_FLATTEN_DIMENSION);
    let scale = effective_scale(w as f32, h as f32, max_dim as f32 / w.max(h) as f32);
    let page = renderer.render(RenderRequest { page: plan.index, scale, ..RenderRequest::default() });
    let pixels = (page.width as usize).checked_mul(page.height as usize);
    if page.error.is_some() || page.width == 0 || page.height == 0 || pixels.and_then(|n| n.checked_mul(4)) != Some(page.rgba.len()) {
        return Ok(false);
    }
    let Some(rgb) = rgb_over_white(&page.rgba) else { return Ok(false) };
    let Some(data) = jpeg(&rgb, 3, page.width, page.height, settings.quality) else { return Ok(false) };
    if data.len() >= plan.image_bytes {
        return Ok(false);
    }
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("XObject"));
    dict.set(b"Subtype".to_vec(), Object::name("Image"));
    dict.set(b"Width".to_vec(), Object::Int(i64::from(page.width)));
    dict.set(b"Height".to_vec(), Object::Int(i64::from(page.height)));
    dict.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
    dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    dict.set(b"Filter".to_vec(), Object::name("DCTDecode"));
    let image = doc.add(Object::Stream(Stream::from_raw(dict, data)));
    let name = fresh_name(doc, plan.page);
    set_only_xobject(doc, plan.page, &name, image, &plan.kept_xobjects)?;
    let mut ops = vec![
        Op::new("q", Vec::new()),
        Op::new("cm", vec![Object::Real(w), Object::Int(0), Object::Int(0), Object::Real(h), Object::Real(plan.crop[0]), Object::Real(plan.crop[1])]),
        Op::new("Do", vec![Object::Name(name)]),
        Op::new("Q", Vec::new()),
    ];
    ops.extend(plan.text.iter().cloned());
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), &serialize_ops(&ops))));
    doc.update_dict(plan.page, |d| d.set(b"Contents".to_vec(), Object::Ref(contents)))?;
    Ok(true)
}

/// Replaces eligible tiled pages with flattened pictures according to `settings`, updating `report` and `progress`.
pub(crate) fn run(
    doc: &mut Document,
    settings: &FlattenSettings,
    report: &mut Report,
    progress: &mut dyn FnMut(Stage) -> bool,
) -> Result<(), OptimizeError> {
    let scanned = scan(doc, settings.min_images);
    report.tiled_pages += scanned.len();
    if scanned.is_empty() {
        return Ok(());
    }
    if doc.output_handler().is_some() {
        report.pages_not_flattened += scanned.len();
        return Ok(());
    }
    // Stage 1: each page that can be flattened gets, for now, its content without text, so one
    // serialization of the document lets the renderer draw every page as the picture will be.
    let mut plans = Vec::new();
    for s in scanned {
        let Ok((ops, kept_xobjects)) = s.ops else {
            report.pages_not_flattened += 1;
            continue;
        };
        let (background, text) = split(&ops);
        let original = doc.get(s.page.obj).as_dict().and_then(|d| d.get(b"Contents").cloned());
        let stripped = doc.add(Object::Stream(Stream::flate(Dict::new(), &serialize_ops(&background))));
        doc.update_dict(s.page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(stripped)))?;
        plans.push(Plan {
            index: s.info.index,
            page: s.page.obj,
            crop: s.page.crop(doc),
            original,
            text,
            kept_xobjects,
            images: s.info.images,
            image_bytes: s.image_bytes,
        });
    }
    if plans.is_empty() {
        return Ok(());
    }
    let bytes = match write_full(doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }) {
        Ok(b) => b,
        Err(e) => {
            restore(doc, &plans)?;
            return Err(e.into());
        }
    };
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig { hide_comments: true, ..RenderConfig::default() });
    // Stage 2: draw each page, store the picture, put the text back.
    let total = plans.len();
    for (done, plan) in plans.iter().enumerate() {
        if !progress(Stage::Flatten { done, total }) {
            restore(doc, plans.get(done..).unwrap_or_default())?;
            return Err(OptimizeError::Cancelled);
        }
        if finish_plan(doc, &mut renderer, plan, settings)? {
            report.pages_flattened += 1;
            report.tiles_flattened += plan.images;
        } else {
            restore(doc, std::slice::from_ref(plan))?;
            report.pages_not_flattened += 1;
        }
    }
    if !progress(Stage::Flatten { done: total, total }) {
        return Err(OptimizeError::Cancelled);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(s: &str, operands: Vec<Object>) -> Op {
        Op::new(s, operands)
    }

    #[test]
    fn text_keeps_its_state_and_loses_the_painting() {
        let ops = vec![
            op("q", vec![]),
            op("re", vec![0.into(), 0.into(), 10.into(), 10.into()]),
            op("W", vec![]),
            op("n", vec![]),
            op("rg", vec![1.into(), 0.into(), 0.into()]),
            op("re", vec![0.into(), 0.into(), 5.into(), 5.into()]),
            op("f", vec![]),
            op("Do", vec![Object::name("Im")]),
            op("BT", vec![]),
            op("Tj", vec![]),
            op("ET", vec![]),
            op("Q", vec![]),
        ];
        let (background, text) = split(&ops);
        assert!(background.iter().all(|o| !o.is("BT") && !o.is("Tj") && !o.is("ET")));
        assert!(background.iter().any(|o| o.is("Do")) && background.iter().any(|o| o.is("f")));
        // The text side keeps the clip and colour, has no Do, and the fill became a no-paint `n`.
        assert!(text.iter().all(|o| !o.is("Do") && !o.is("f")));
        assert!(text.iter().any(|o| o.is("W")) && text.iter().any(|o| o.is("rg")) && text.iter().any(|o| o.is("Tj")));
        assert_eq!(text.iter().filter(|o| o.is("q")).count(), text.iter().filter(|o| o.is("Q")).count());
    }

    #[test]
    fn premultiplied_pixels_go_over_white() {
        assert_eq!(rgb_over_white(&[10, 20, 30, 255, 0, 0, 0, 0, 50, 50, 50, 128]), Some(vec![10, 20, 30, 255, 255, 255, 177, 177, 177]));
    }

    #[test]
    fn inherited_resources_are_kept_when_page_has_no_own_resources() {
        let mut doc = Document::new_empty();
        let font_ref = doc.add(Object::Dict(Dict::new()));
        let mut font_dict = Dict::new();
        font_dict.set(b"F1".to_vec(), Object::Ref(font_ref));
        let mut pages_res = Dict::new();
        pages_res.set(b"Font".to_vec(), Object::Dict(font_dict));

        let mut pages_dict = Dict::new();
        pages_dict.set(b"Type".to_vec(), Object::name("Pages"));
        pages_dict.set(b"Resources".to_vec(), Object::Dict(pages_res));
        let pages_ref = doc.add(Object::Dict(pages_dict));

        let mut page_dict = Dict::new();
        page_dict.set(b"Type".to_vec(), Object::name("Page"));
        page_dict.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        let page_ref = doc.add(Object::Dict(page_dict));

        let img_ref = doc.add(Object::Dict(Dict::new()));
        set_only_xobject(&mut doc, page_ref, b"PCFlat", img_ref, &Dict::new()).unwrap();

        let page_obj = doc.get(page_ref);
        let res = page_obj.as_dict().unwrap().get(b"Resources").unwrap();
        let resolved = doc.resolve(res);
        let res_dict = resolved.as_dict().unwrap();
        assert!(res_dict.get(b"Font").is_some(), "inherited Font must survive");
        assert!(res_dict.get(b"XObject").is_some(), "XObject must be set");
    }

    fn add_rgb_image(doc: &mut Document, w: i64, h: i64, fill: u8) -> ObjRef {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(w));
        d.set(b"Height".to_vec(), Object::Int(h));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        doc.add(Object::Stream(Stream::from_raw(d, vec![fill; (w * h * 3) as usize])))
    }

    #[test]
    fn type3_glyph_xobjects_resolved_through_page_are_preserved() {
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        let mut content_ops = String::new();
        for i in 0..8 {
            let tile = add_rgb_image(&mut doc, 40, 40, (i * 25) as u8);
            let name = format!("T{i}");
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(tile));
            let x = (i % 4) * 20;
            let y = (i / 4) * 20;
            content_ops.push_str(&format!("q 20 0 0 20 {x} {y} cm /{name} Do Q\n"));
        }
        let glyph_img = add_rgb_image(&mut doc, 4, 4, 200);
        xobjects.set(b"ImGlyph".to_vec(), Object::Ref(glyph_img));
        content_ops.push_str("BT /T3 12 Tf 10 50 Td (A) Tj ET\n");

        let glyph_proc = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1 0 d0 q 1 0 0 1 0 0 cm /ImGlyph Do Q".to_vec())));
        let mut char_procs = Dict::new();
        char_procs.set(b"A".to_vec(), Object::Ref(glyph_proc));

        let mut enc = Dict::new();
        enc.set(b"Type".to_vec(), Object::name("Encoding"));
        enc.set(b"Differences".to_vec(), Object::Array(vec![Object::Int(65), Object::name("A")]));

        let mut t3 = Dict::new();
        t3.set(b"Type".to_vec(), Object::name("Font"));
        t3.set(b"Subtype".to_vec(), Object::name("Type3"));
        t3.set(b"FontBBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()]));
        t3.set(b"FontMatrix".to_vec(), Object::Array(vec![1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 0.into()]));
        t3.set(b"CharProcs".to_vec(), Object::Dict(char_procs));
        t3.set(b"Encoding".to_vec(), Object::Dict(enc));
        t3.set(b"FirstChar".to_vec(), Object::Int(65));
        t3.set(b"LastChar".to_vec(), Object::Int(65));
        t3.set(b"Widths".to_vec(), Object::Array(vec![Object::Int(1)]));
        let t3_ref = doc.add(Object::Dict(t3));

        let mut fonts = Dict::new();
        fonts.set(b"T3".to_vec(), Object::Ref(t3_ref));

        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        res.set(b"Font".to_vec(), Object::Dict(fonts));

        let contents_ref = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.into_bytes())));
        let pages_ref = doc.add(Object::Dict(Dict::new()));

        let mut page_dict = Dict::new();
        page_dict.set(b"Type".to_vec(), Object::name("Page"));
        page_dict.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page_dict.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        page_dict.set(b"Resources".to_vec(), Object::Dict(res));
        page_dict.set(b"Contents".to_vec(), Object::Ref(contents_ref));
        let page_ref = doc.add(Object::Dict(page_dict));

        let mut pages_dict = Dict::new();
        pages_dict.set(b"Type".to_vec(), Object::name("Pages"));
        pages_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
        pages_dict.set(b"Count".to_vec(), Object::Int(1));
        doc.set(pages_ref, Object::Dict(pages_dict));

        let mut cat = Dict::new();
        cat.set(b"Type".to_vec(), Object::name("Catalog"));
        cat.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        let cat_ref = doc.add(Object::Dict(cat));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));

        let mut report = Report::default();
        run(&mut doc, &FlattenSettings { enabled: true, min_images: 8, max_dimension: 200, quality: 70 }, &mut report, &mut |_| true).unwrap();
        assert_eq!(report.pages_flattened, 1);

        let page_obj = doc.get(page_ref);
        let res_obj = doc.resolve(page_obj.as_dict().unwrap().get(b"Resources").unwrap());
        let xobj_obj = doc.resolve(res_obj.as_dict().unwrap().get(b"XObject").unwrap());
        let xobj_dict = xobj_obj.as_dict().unwrap();
        assert!(xobj_dict.contains(b"PCFlat"), "flattened picture must be installed");
        assert_eq!(xobj_dict.reference(b"ImGlyph"), Some(glyph_img), "Type 3 glyph XObject resolved through page must survive");
        assert!(!xobj_dict.contains(b"T0"), "tile XObjects must be detached");
    }

    #[test]
    fn type3_glyph_with_unreadable_stream_skips_flattening() {
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        let mut content_ops = String::new();
        for i in 0..8 {
            let tile = add_rgb_image(&mut doc, 40, 40, (i * 25) as u8);
            let name = format!("T{i}");
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(tile));
            content_ops.push_str(&format!("q 10 0 0 10 0 0 cm /{name} Do Q\n"));
        }
        let mut bad_dict = Dict::new();
        bad_dict.set(b"Filter".to_vec(), Object::name("NoSuchFilter"));
        let bad_proc = doc.add(Object::Stream(Stream::from_raw(bad_dict, b"/ImGlyph Do".to_vec())));
        let mut char_procs = Dict::new();
        char_procs.set(b"A".to_vec(), Object::Ref(bad_proc));

        let mut t3 = Dict::new();
        t3.set(b"Type".to_vec(), Object::name("Font"));
        t3.set(b"Subtype".to_vec(), Object::name("Type3"));
        t3.set(b"CharProcs".to_vec(), Object::Dict(char_procs));
        let mut fonts = Dict::new();
        fonts.set(b"T3".to_vec(), Object::Dict(t3));

        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        res.set(b"Font".to_vec(), Object::Dict(fonts));

        let contents_ref = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.into_bytes())));
        let pages_ref = doc.add(Object::Dict(Dict::new()));
        let mut page_dict = Dict::new();
        page_dict.set(b"Type".to_vec(), Object::name("Page"));
        page_dict.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page_dict.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        page_dict.set(b"Resources".to_vec(), Object::Dict(res));
        page_dict.set(b"Contents".to_vec(), Object::Ref(contents_ref));
        let page_ref = doc.add(Object::Dict(page_dict));

        let mut pages_dict = Dict::new();
        pages_dict.set(b"Type".to_vec(), Object::name("Pages"));
        pages_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
        pages_dict.set(b"Count".to_vec(), Object::Int(1));
        doc.set(pages_ref, Object::Dict(pages_dict));

        let mut cat = Dict::new();
        cat.set(b"Type".to_vec(), Object::name("Catalog"));
        cat.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        let cat_ref = doc.add(Object::Dict(cat));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));

        let detected = detect_tiled_pages(&doc, 8);
        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].skip, Some(Skip::UnreadableContent));
    }

    #[test]
    fn type3_glyph_traversal_step_limit_aborts() {
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        let mut content_ops = String::new();
        for i in 0..8 {
            let tile = add_rgb_image(&mut doc, 40, 40, (i * 25) as u8);
            let name = format!("T{i}");
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(tile));
            content_ops.push_str(&format!("q 10 0 0 10 0 0 cm /{name} Do Q\n"));
        }
        let mut proc_ops = Vec::new();
        for _ in 0..=MAX_TYPE3_STEPS {
            proc_ops.extend_from_slice(b"/X Do\n");
        }
        let proc = doc.add(Object::Stream(Stream::from_raw(Dict::new(), proc_ops)));
        let mut char_procs = Dict::new();
        char_procs.set(b"A".to_vec(), Object::Ref(proc));

        let mut t3 = Dict::new();
        t3.set(b"Type".to_vec(), Object::name("Font"));
        t3.set(b"Subtype".to_vec(), Object::name("Type3"));
        t3.set(b"CharProcs".to_vec(), Object::Dict(char_procs));
        let mut fonts = Dict::new();
        fonts.set(b"T3".to_vec(), Object::Dict(t3));

        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        res.set(b"Font".to_vec(), Object::Dict(fonts));

        let contents_ref = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.into_bytes())));
        let pages_ref = doc.add(Object::Dict(Dict::new()));
        let mut page_dict = Dict::new();
        page_dict.set(b"Type".to_vec(), Object::name("Page"));
        page_dict.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page_dict.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        page_dict.set(b"Resources".to_vec(), Object::Dict(res));
        page_dict.set(b"Contents".to_vec(), Object::Ref(contents_ref));
        let page_ref = doc.add(Object::Dict(page_dict));

        let mut pages_dict = Dict::new();
        pages_dict.set(b"Type".to_vec(), Object::name("Pages"));
        pages_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
        pages_dict.set(b"Count".to_vec(), Object::Int(1));
        doc.set(pages_ref, Object::Dict(pages_dict));

        let mut cat = Dict::new();
        cat.set(b"Type".to_vec(), Object::name("Catalog"));
        cat.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        let cat_ref = doc.add(Object::Dict(cat));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));

        let detected = detect_tiled_pages(&doc, 8);
        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].skip, Some(Skip::UnreadableContent));
    }

    #[test]
    fn a_bitmap_type3_font_with_many_glyph_programs_still_flattens() {
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        let mut content_ops = String::new();
        for i in 0..8 {
            let tile = add_rgb_image(&mut doc, 40, 40, (i * 25) as u8);
            let name = format!("T{i}");
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(tile));
            let x = (i % 4) * 20;
            let y = (i / 4) * 20;
            content_ops.push_str(&format!("q 20 0 0 20 {x} {y} cm /{name} Do Q\n"));
        }
        content_ops.push_str("BT /T3 4 Tf 10 50 Td (AAAA) Tj ET\n");

        // One image per glyph program, in the font's own resources: the shape a scanned or exported
        // bitmap Type 3 font has, with more glyph programs than the page-level step backstop.
        let glyph_img = add_rgb_image(&mut doc, 2, 2, 90);
        let mut glyph_xobjects = Dict::new();
        glyph_xobjects.set(b"Im".to_vec(), Object::Ref(glyph_img));
        let mut procs = Dict::new();
        for i in 0..MAX_TYPE3_STEPS + 16 {
            let proc = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q 1 0 0 1 0 0 cm /Im Do Q".to_vec())));
            procs.set(format!("g{i}").into_bytes(), Object::Ref(proc));
        }
        let mut font_res = Dict::new();
        font_res.set(b"XObject".to_vec(), Object::Dict(glyph_xobjects));

        let mut t3 = Dict::new();
        t3.set(b"Type".to_vec(), Object::name("Font"));
        t3.set(b"Subtype".to_vec(), Object::name("Type3"));
        t3.set(b"CharProcs".to_vec(), Object::Dict(procs));
        t3.set(b"Resources".to_vec(), Object::Dict(font_res));
        let mut fonts = Dict::new();
        fonts.set(b"T3".to_vec(), Object::Dict(t3));

        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        res.set(b"Font".to_vec(), Object::Dict(fonts));

        let contents_ref = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.into_bytes())));
        let pages_ref = doc.add(Object::Dict(Dict::new()));
        let mut page_dict = Dict::new();
        page_dict.set(b"Type".to_vec(), Object::name("Page"));
        page_dict.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page_dict.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        page_dict.set(b"Resources".to_vec(), Object::Dict(res));
        page_dict.set(b"Contents".to_vec(), Object::Ref(contents_ref));
        let page_ref = doc.add(Object::Dict(page_dict));

        let mut pages_dict = Dict::new();
        pages_dict.set(b"Type".to_vec(), Object::name("Pages"));
        pages_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
        pages_dict.set(b"Count".to_vec(), Object::Int(1));
        doc.set(pages_ref, Object::Dict(pages_dict));

        let mut cat = Dict::new();
        cat.set(b"Type".to_vec(), Object::name("Catalog"));
        cat.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        let cat_ref = doc.add(Object::Dict(cat));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));

        let detected = detect_tiled_pages(&doc, 8);
        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].skip, None, "glyph programs of the font's own must not count against the page-level step backstop");
    }

    #[test]
    fn page_with_widget_annotation_skips_flattening() {
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        let mut content_ops = String::new();
        for i in 0..8 {
            let tile = add_rgb_image(&mut doc, 40, 40, (i * 25) as u8);
            let name = format!("T{i}");
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(tile));
            let x = (i % 4) * 20;
            let y = (i / 4) * 20;
            content_ops.push_str(&format!("q 20 0 0 20 {x} {y} cm /{name} Do Q\n"));
        }

        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));

        let mut widget = Dict::new();
        widget.set(b"Type".to_vec(), Object::name("Annot"));
        widget.set(b"Subtype".to_vec(), Object::name("Widget"));
        widget.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 50.into(), 20.into()]));
        let widget_ref = doc.add(Object::Dict(widget));

        let contents_ref = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.into_bytes())));
        let pages_ref = doc.add(Object::Dict(Dict::new()));
        let mut page_dict = Dict::new();
        page_dict.set(b"Type".to_vec(), Object::name("Page"));
        page_dict.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        page_dict.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        page_dict.set(b"Resources".to_vec(), Object::Dict(res));
        page_dict.set(b"Contents".to_vec(), Object::Ref(contents_ref));
        page_dict.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(widget_ref)]));
        let page_ref = doc.add(Object::Dict(page_dict));

        let mut pages_dict = Dict::new();
        pages_dict.set(b"Type".to_vec(), Object::name("Pages"));
        pages_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
        pages_dict.set(b"Count".to_vec(), Object::Int(1));
        doc.set(pages_ref, Object::Dict(pages_dict));

        let mut cat = Dict::new();
        cat.set(b"Type".to_vec(), Object::name("Catalog"));
        cat.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        let cat_ref = doc.add(Object::Dict(cat));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));

        let detected = detect_tiled_pages(&doc, 8);
        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].skip, Some(Skip::Annotations), "form widget annotation must cause page flattening to be skipped");
    }

    #[test]
    fn shared_images_across_flattened_pages_attribute_savings_fairly() {
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        let mut content_ops = String::new();
        let mut total_bytes = 0usize;
        for i in 0..8 {
            let tile = add_rgb_image(&mut doc, 40, 40, (i * 25) as u8);
            let name = format!("T{i}");
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(tile));
            let x = (i % 4) * 20;
            let y = (i / 4) * 20;
            content_ops.push_str(&format!("q 20 0 0 20 {x} {y} cm /{name} Do Q\n"));
            if let Object::Stream(s) = &*doc.get(tile) {
                total_bytes += s.raw.len();
            }
        }

        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        let res_ref = doc.add(Object::Dict(res));

        let contents1 = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.clone().into_bytes())));
        let contents2 = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.into_bytes())));

        let pages_ref = doc.add(Object::Dict(Dict::new()));

        let mut p1 = Dict::new();
        p1.set(b"Type".to_vec(), Object::name("Page"));
        p1.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        p1.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        p1.set(b"Resources".to_vec(), Object::Ref(res_ref));
        p1.set(b"Contents".to_vec(), Object::Ref(contents1));
        let page1 = doc.add(Object::Dict(p1));

        let mut p2 = Dict::new();
        p2.set(b"Type".to_vec(), Object::name("Page"));
        p2.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        p2.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        p2.set(b"Resources".to_vec(), Object::Ref(res_ref));
        p2.set(b"Contents".to_vec(), Object::Ref(contents2));
        let page2 = doc.add(Object::Dict(p2));

        let mut pages_dict = Dict::new();
        pages_dict.set(b"Type".to_vec(), Object::name("Pages"));
        pages_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page1), Object::Ref(page2)]));
        pages_dict.set(b"Count".to_vec(), Object::Int(2));
        doc.set(pages_ref, Object::Dict(pages_dict));

        let mut cat = Dict::new();
        cat.set(b"Type".to_vec(), Object::name("Catalog"));
        cat.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        let cat_ref = doc.add(Object::Dict(cat));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));

        let scanned = scan(&doc, 8);
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned[0].image_bytes, total_bytes / 2);
        assert_eq!(scanned[1].image_bytes, total_bytes / 2);
    }

    #[test]
    fn shared_images_with_unflattened_page_yield_zero_savings() {
        let mut doc = Document::new_empty();
        let mut xobjects = Dict::new();
        let mut content_ops = String::new();
        for i in 0..8 {
            let tile = add_rgb_image(&mut doc, 40, 40, (i * 25) as u8);
            let name = format!("T{i}");
            xobjects.set(name.as_bytes().to_vec(), Object::Ref(tile));
            let x = (i % 4) * 20;
            let y = (i / 4) * 20;
            content_ops.push_str(&format!("q 20 0 0 20 {x} {y} cm /{name} Do Q\n"));
        }

        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        let res_ref = doc.add(Object::Dict(res));

        let contents1 = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.clone().into_bytes())));
        let contents2 = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content_ops.into_bytes())));

        let mut widget = Dict::new();
        widget.set(b"Type".to_vec(), Object::name("Annot"));
        widget.set(b"Subtype".to_vec(), Object::name("Widget"));
        widget.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 50.into(), 20.into()]));
        let widget_ref = doc.add(Object::Dict(widget));

        let pages_ref = doc.add(Object::Dict(Dict::new()));

        let mut p1 = Dict::new();
        p1.set(b"Type".to_vec(), Object::name("Page"));
        p1.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        p1.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        p1.set(b"Resources".to_vec(), Object::Ref(res_ref));
        p1.set(b"Contents".to_vec(), Object::Ref(contents1));
        let page1 = doc.add(Object::Dict(p1));

        let mut p2 = Dict::new();
        p2.set(b"Type".to_vec(), Object::name("Page"));
        p2.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        p2.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 200.into(), 200.into()]));
        p2.set(b"Resources".to_vec(), Object::Ref(res_ref));
        p2.set(b"Contents".to_vec(), Object::Ref(contents2));
        p2.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(widget_ref)]));
        let page2 = doc.add(Object::Dict(p2));

        let mut pages_dict = Dict::new();
        pages_dict.set(b"Type".to_vec(), Object::name("Pages"));
        pages_dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page1), Object::Ref(page2)]));
        pages_dict.set(b"Count".to_vec(), Object::Int(2));
        doc.set(pages_ref, Object::Dict(pages_dict));

        let mut cat = Dict::new();
        cat.set(b"Type".to_vec(), Object::name("Catalog"));
        cat.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        let cat_ref = doc.add(Object::Dict(cat));
        doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));

        let scanned = scan(&doc, 8);
        assert_eq!(scanned.len(), 2);
        // Page 1 is a candidate, but its tiles are shared with Page 2 which cannot be flattened (Skip::Annotations).
        // Therefore, flattening Page 1 cannot free the tiles from the document, so image_bytes must be 0.
        assert_eq!(scanned[0].image_bytes, 0);

        let mut report = Report::default();
        run(&mut doc, &FlattenSettings { enabled: true, min_images: 8, max_dimension: 200, quality: 70 }, &mut report, &mut |_| true).unwrap();
        assert_eq!(report.pages_flattened, 0, "must not flatten when shared tiles cannot be deleted from the document");
    }
}

