use super::create::{finite, num_array};
use super::lookup::{annot_dict, annots, page_refs};
use super::modify::refuse_foreign_text;
use super::types::NOTE_SIZE;
use super::{AnnotError, Dict, Document, ObjRef, Object, Stream, appearance};

/// (Re)generate `/AP /N` for the annotation `r` from its dictionary, for an edit that keeps its
/// text (a restyle, a new size or colour).
///
/// A comment whose text the standard fonts can't draw, with an appearance another program drew
/// (with a font that has those characters), keeps that appearance: redrawing it would put `?` in
/// their place, so the edit is refused (`AnnotError::Undrawable`). One PdfCraft drew with `?`
/// (marked `pdfcraft_fonts::UNDRAWN_MARK`) is redrawn.
pub fn set_appearance(doc: &mut Document, r: ObjRef) -> Result<(), AnnotError> {
    refuse_foreign_text(doc, &annot_dict(doc, r))?;
    draw_appearance(doc, r)
}

/// Put `before` back when redrawing fails, so a refused edit does not keep the new rectangle or style.
pub(crate) fn redraw_or_restore(doc: &mut Document, r: ObjRef, before: Dict) -> Result<(), AnnotError> {
    if let Err(e) = set_appearance(doc, r) {
        doc.set(r, Object::Dict(before));
        return Err(e);
    }
    Ok(())
}

/// Draw `r`'s normal appearance from its dictionary, whatever its text: characters the standard
/// fonts can't draw come out as `?`, and the annotation is marked (`pdfcraft_fonts::UNDRAWN_MARK`,
/// cleared when all of it can be drawn) so the edit is reported and flattening won't make that
/// `?` permanent. For new text (a new comment, changed contents), as Acrobat keeps text its font
/// can't show, and for display-only copies.
pub(crate) fn draw_appearance(doc: &mut Document, r: ObjRef) -> Result<(), AnnotError> {
    let d = annot_dict(doc, r);
    let direct = with_direct_values(doc, &d);
    let Some((stream, drawn_from)) = build_appearance(doc, &d, &direct) else {
        return Err(AnnotError::Unsupported(String::from_utf8_lossy(direct.name(b"Subtype").unwrap_or_default()).into_owned()));
    };
    let built = Built::new(stream, drawn_from);
    apply_appearance(doc, r, &d, built)
}

/// Whether [`set_appearance`] can draw the annotation `d`: read as the display reads it, with
/// references followed and the same fallback. Edits check this before changing anything.
pub(crate) fn can_draw(doc: &Document, d: &Dict) -> bool {
    build_appearance(doc, d, &with_direct_values(doc, d)).is_some()
}

/// Draw the appearance for an annotation dictionary: the expensive half of [`set_appearance`].
/// `direct` is `d` with references followed ([`with_direct_values`]). Pure from the dictionary and
/// the objects it points at, so [`AppearanceCache`] can reuse it. Returns the stream and the
/// dictionary it was drawn from (`direct`, or `d` for the fallback), which [`Built::new`] reads.
fn build_appearance<'a>(doc: &Document, d: &'a Dict, direct: &'a Dict) -> Option<(Stream, &'a Dict)> {
    // References followed first. Where that turns up something the builder doesn't draw (a
    // cloudy `/BE` on a square), the drawing made without them stays, rather than none.
    let (mut stream, drawn_from) = match appearance::build(doc, direct) {
        Some(stream) => (stream, direct),
        None if !std::ptr::eq(direct, d) => (appearance::build(doc, d)?, d),
        None => return None,
    };
    // Picture-stamp restyling must retain the placement's page-axis correction.
    if d.get(b"PCPicture").is_some() {
        let matrix = d.get(b"AP").and_then(|ap| {
            let ap = doc.resolve(ap);
            let normal = doc.resolve(ap.as_dict()?.get(b"N")?);
            let Object::Stream(normal) = normal.as_ref() else { return None };
            normal.dict.get(b"Matrix").cloned()
        });
        if let Some(matrix) = matrix {
            stream.dict.set(b"Matrix".to_vec(), matrix);
        }
    }
    Some((stream, drawn_from))
}

/// Entries that link the annotation to other objects rather than describe how it looks: left as
/// references by [`with_direct_values`].
const LINK_KEYS: [&[u8]; 5] = [b"P", b"Popup", b"IRT", b"Parent", b"AP"];

/// How deep inside an entry [`with_direct_values`] follows references: `/InkList` is an array of
/// arrays, `/Border` may end in a dash array.
const INLINE_DEPTH: usize = 4;

/// Values [`with_direct_values`] copies out of referenced arrays and dictionaries, or walks
/// before finding one too large to copy, for one annotation, so a file can't have a large object
/// copied or scanned for every entry that points at it.
pub(crate) const INLINE_BUDGET: usize = 1 << 16;

/// Values copied or walked for all the annotations one display pass draws
/// ([`with_missing_appearances_cached`]), so a large object that many annotations point at is
/// not copied, or scanned and found too large, for each of them without bound. Past it,
/// references are left as they are.
pub(crate) const PASS_BUDGET: usize = 1 << 22;

/// `d` with the references among the values [`appearance::build`] reads followed. Any value may be
/// given by reference (ISO 32000-2 §7.3.10): a border style dictionary (`/BS 5 0 R`, Table 168), a
/// colour, `/RD`, `/CL`, a `/W` inside `/BS`. The builder reads only direct values (apart from
/// `/QuadPoints`, which it resolves itself), so it draws from this copy. The entries in
/// [`LINK_KEYS`] and streams stay references; past [`INLINE_DEPTH`] levels or [`INLINE_BUDGET`]
/// copied values a reference is left as it is, which the builder treats as unreadable. `d` itself
/// when nothing is a reference.
pub(crate) fn with_direct_values<'d>(doc: &Document, d: &'d Dict) -> std::borrow::Cow<'d, Dict> {
    follow(doc, d, &mut INLINE_BUDGET.clone()).0
}

/// [`with_direct_values`], copying at most [`INLINE_BUDGET`] values and no more than `pass` has
/// left, which is reduced by what was copied.
#[cfg(test)]
pub(crate) fn with_direct_values_within<'d>(doc: &Document, d: &'d Dict, pass: &mut usize) -> std::borrow::Cow<'d, Dict> {
    follow(doc, d, pass).0
}

/// [`with_direct_values_within`], and whether a reference was left as it is because `pass` had
/// less than [`INLINE_BUDGET`] left: a drawing that a later pass, with room, would draw better.
fn follow<'d>(doc: &Document, d: &'d Dict, pass: &mut usize) -> (std::borrow::Cow<'d, Dict>, bool) {
    let allowed = INLINE_BUDGET.min(*pass);
    let mut budget = Budget { left: allowed, cut: false };
    let mut out: Option<Dict> = None;
    for (key, value) in d.iter() {
        if LINK_KEYS.contains(&key.as_slice()) {
            continue;
        }
        if let Some(direct) = direct_value(doc, value, INLINE_DEPTH, &mut budget) {
            out.get_or_insert_with(|| d.clone()).set(key.clone(), direct);
        }
    }
    *pass = pass.saturating_sub(allowed.saturating_sub(budget.left));
    let starved = budget.cut && allowed < INLINE_BUDGET;
    (out.map_or(std::borrow::Cow::Borrowed(d), std::borrow::Cow::Owned), starved)
}

/// What is left of a budget, and whether a reference was left as it is for lack of it.
struct Budget {
    left: usize,
    cut: bool,
}

/// Bytes of a string or a name that cost one value more in the budgets, about what a value itself
/// takes in memory: a long string costs what it holds, so a large string that many entries point
/// at is not copied for each of them without bound.
const BYTES_PER_VALUE: usize = 64;

/// What copying `o` itself costs in the budgets, not counting what is inside an array or a
/// dictionary: one, and one more for every [`BYTES_PER_VALUE`] bytes of a string or a name.
fn cost(o: &Object) -> usize {
    let bytes = match o {
        Object::String(s) => s.bytes.len(),
        Object::Name(n) => n.len(),
        _ => 0,
    };
    1usize.saturating_add(bytes / BYTES_PER_VALUE)
}

/// What copying `o` costs: the [`cost`] of every value in it, counting into its arrays and
/// dictionaries (not through references, which are charged when followed), or, past `limit`,
/// `Err` with the values it visited or queued before finding that out (at least one), so a
/// rejected walk is charged too. Walks at most `limit` values and holds no more than `limit` waiting.
fn weight(o: &Object, limit: usize) -> Result<usize, usize> {
    let mut count = 0usize;
    let mut stack = vec![o];
    while let Some(o) = stack.pop() {
        let spent = count.saturating_add(stack.len()).max(1);
        count = match count.checked_add(cost(o)).filter(|c| *c <= limit) {
            Some(c) => c,
            None => return Err(spent),
        };
        let inner = match o {
            Object::Array(items) => items.len(),
            Object::Dict(entries) => entries.len(),
            _ => continue,
        };
        // Every value waiting is counted later, so more than `limit` in all is over it already.
        if count.saturating_add(stack.len()).saturating_add(inner) > limit {
            return Err(spent.saturating_add(1));
        }
        match o {
            Object::Array(items) => stack.extend(items.iter()),
            Object::Dict(entries) => stack.extend(entries.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
    Ok(count)
}

/// `o` with references followed `depth` levels into arrays and dictionaries, or `None` when there
/// is nothing to follow (or it may not be: a stream, an over-budget object, a chain of references).
fn direct_value(doc: &Document, o: &Object, depth: usize, budget: &mut Budget) -> Option<Object> {
    let held;
    let (base, followed) = match o {
        Object::Ref(_) => {
            // A spent budget follows nothing more, not even to find out what is there.
            if budget.left == 0 {
                budget.cut = true;
                return None;
            }
            held = doc.resolve(o);
            if matches!(&*held, Object::Stream(_) | Object::Ref(_)) {
                return None;
            }
            // Everything copied counts, inner arrays and dictionaries too, and so does the walk
            // that finds an object too large: many references to one would otherwise each walk
            // it again for free.
            match weight(&held, budget.left) {
                Ok(n) => budget.left = budget.left.checked_sub(n)?,
                Err(spent) => {
                    budget.left = budget.left.saturating_sub(spent);
                    budget.cut = true;
                    return None;
                }
            }
            (&*held, true)
        }
        other => (other, false),
    };
    let Some(next) = depth.checked_sub(1) else { return followed.then(|| base.clone()) };
    match base {
        Object::Array(items) => {
            let mut changed: Option<Vec<Object>> = None;
            for (i, item) in items.iter().enumerate() {
                if let Some(direct) = direct_value(doc, item, next, budget) {
                    let copy = changed.get_or_insert_with(|| items.clone());
                    if let Some(slot) = copy.get_mut(i) {
                        *slot = direct;
                    }
                }
            }
            changed.map(Object::Array).or_else(|| followed.then(|| base.clone()))
        }
        Object::Dict(entries) => {
            let mut changed: Option<Dict> = None;
            for (key, value) in entries.iter() {
                if let Some(direct) = direct_value(doc, value, next, budget) {
                    changed.get_or_insert_with(|| entries.clone()).set(key.clone(), direct);
                }
            }
            changed.map(Object::Dict).or_else(|| followed.then(|| base.clone()))
        }
        _ => followed.then(|| base.clone()),
    }
}

/// A built appearance and what storing it needs from the dictionary it was drawn from (with
/// references followed, or not for the fallback drawing), so a cached one is stored without
/// following them again, and its marker matches what it draws.
#[derive(Clone, Debug)]
struct Built {
    stream: Stream,
    /// The first character the standard fonts can't draw (it came out as `?`).
    missing: Option<char>,
    /// A note's or attachment's `/Rect`, as written.
    rect: Option<Vec<f64>>,
}

impl Built {
    fn new(stream: Stream, drawn_from: &Dict) -> Self {
        let missing = appearance::undrawable_text(drawn_from).map(|(_, c)| c);
        let rect = matches!(drawn_from.name(b"Subtype"), Some(b"Text" | b"FileAttachment"))
            .then(|| drawn_from.get(b"Rect")?.as_array()?.iter().map(|o| o.as_f64()).collect::<Option<Vec<f64>>>())
            .flatten();
        Built { stream, missing, rect }
    }
}

/// Store a built appearance on the annotation: the cheap half of [`set_appearance`].
fn apply_appearance(doc: &mut Document, r: ObjRef, d: &Dict, built: Built) -> Result<(), AnnotError> {
    // `missing`: characters the standard fonts can't draw came out as `?`; the annotation is
    // marked with the first.
    let Built { mut stream, missing, rect } = built;
    // A text box's Thai glyphs are streams of their own.
    pdfcraft_fonts::shaped_text::store_streams(doc, &mut stream.dict);
    let ap = doc.add(Object::Stream(stream));
    // A copy (a shared /AP is left alone) that keeps unknown entries; the old down and rollover
    // appearances would show the previous look on press or hover, so they go with the old /N.
    let mut apd = d.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).unwrap_or_default();
    apd.remove(b"D");
    apd.remove(b"R");
    apd.set(b"N".to_vec(), Object::Ref(ap));
    // A note icon has a fixed size (`NOTE_SIZE`); viewers draw it at the upper-left corner of `/Rect`.
    // The appearance's box is mapped onto `/Rect`, so any other `/Rect` (a large one, or one with
    // no width, as LaTeX `pdfcomment` writes) would stretch the icon or shrink it to nothing.
    let icon_rect = rect.filter(|q| q.len() == 4 && finite(q)).map(|q| {
        let (x, top) = (q[0].min(q[2]), q[1].max(q[3]));
        [x, top - NOTE_SIZE, x + NOTE_SIZE, top]
    });
    doc.update_dict(r, |d| {
        d.set(b"AP".to_vec(), Object::Dict(apd));
        if let Some(rect) = icon_rect {
            d.set(b"Rect".to_vec(), num_array(&rect));
        }
        d.remove(b"AS");
        pdfcraft_fonts::set_undrawn_mark(d, missing);
    })?;
    Ok(())
}

/// Display appearances built for annotations without any, reused while the annotation
/// dictionaries they were drawn from are unchanged.
///
/// A content edit (moving a paragraph, retyping text, …) leaves every annotation dictionary
/// alone, but the display copy is rebuilt from the edited document on every refresh: without
/// this cache each refresh redrew every appearance (27k on one CAD file, ~0.7 s per edit
/// commit). Entries are keyed by annotation reference; object numbers are never reused within
/// a document lineage, so an entry stays valid until an annotation edit runs. Callers clear
/// the cache on any refresh that may change annotations (comments, structural and full
/// refreshes) and keep it for content-only ones.
#[derive(Clone, Debug, Default)]
pub struct AppearanceCache {
    entries: std::collections::HashMap<ObjRef, Built>,
    bytes: usize,
    hits: u64,
    builds: u64,
}

impl AppearanceCache {
    /// Cached appearance data restarts past this instead of growing without bound.
    const MAX_BYTES: usize = 64 << 20;

    /// Forget every entry (an annotation edit may have changed the dictionaries).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn get(&mut self, r: ObjRef) -> Option<Built> {
        let built = self.entries.get(&r)?.clone();
        self.hits += 1;
        Some(built)
    }

    fn insert(&mut self, r: ObjRef, built: &Built) {
        let stream = &built.stream;
        self.builds += 1;
        if self.entries.len() >= 1_000_000 {
            self.clear();
        }
        if self.bytes.saturating_add(stream.raw.len()) <= Self::MAX_BYTES {
            self.bytes += stream.raw.len();
            self.entries.insert(r, built.clone());
        }
        // Else over budget: keep serving the entries already cached instead of growing.
    }

    /// Cache hits so far (reused appearances); for tests.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Appearances built so far; for tests.
    pub fn builds(&self) -> u64 {
        self.builds
    }
}

/// A copy of `doc` in which every annotation without a normal appearance (`/AP /N`) has the one
/// [`appearance::build`] draws from its dictionary, for displaying the document (`None` when no
/// annotation needs one). Many files carry comments without appearances (FreeText, Ink, notes,
/// stamps, …) that viewers draw from the dictionary. `doc` is not changed: saving writes the file
/// as it was. Links, form fields and pop-ups keep their own handling.
pub fn with_missing_appearances(doc: &Document) -> Option<Document> {
    with_missing_appearances_cached(doc, &mut AppearanceCache::default())
}

/// A copy of `doc` with [`AppearanceCache`] reuse across refreshes: annotations whose
/// dictionary is unchanged since the cached build keep their stored appearance instead of
/// redrawing it. See [`AppearanceCache`] for when entries stay valid.
pub fn with_missing_appearances_cached(doc: &Document, cache: &mut AppearanceCache) -> Option<Document> {
    let mut copy: Option<Document> = None;
    // An annotation listed more than once, on one page or several, is drawn once. `has_normal`
    // reads `doc`, which never gets the appearance, so every repeat used to add another stream to
    // the copy, and the display bytes grew with the number of listings.
    let mut seen = std::collections::HashSet::new();
    let mut budget = PASS_BUDGET;
    for page in page_refs(doc).ok()? {
        for r in annots(doc, page).iter().filter_map(Object::as_ref) {
            if !seen.insert(r) {
                continue;
            }
            let d = annot_dict(doc, r);
            // `/Subtype` may be given by reference too (ISO 32000-2 §7.3.10).
            let subtype = d.get(b"Subtype").map(|o| doc.resolve(o));
            if matches!(subtype.as_deref().and_then(Object::as_name), None | Some(b"Link" | b"Widget" | b"Popup")) {
                continue;
            }
            let has_normal = d.get(b"AP").is_some_and(|ap| doc.resolve(ap).as_dict().is_some_and(|ap| ap.contains(b"N")));
            if has_normal {
                continue;
            }
            if let Some(built) = cache.get(r) {
                // Fails only when `r` isn't a dictionary, which the cached build ruled out.
                apply_appearance(copy.get_or_insert_with(|| doc.clone()), r, &d, built).ok();
                continue;
            }
            // Another program's stamp gets a drawing for display only (ISO 32000-2 Table 184). Its
            // `/AP`, if any, has no `/N` to keep here.
            let other_stamp = || {
                let mut bare = d.clone();
                bare.remove(b"AP");
                appearance::display_stamp(&bare)
            };
            let (direct, starved) = follow(doc, &d, &mut budget);
            let built = match build_appearance(doc, &d, &direct) {
                Some((stream, drawn_from)) => Built::new(stream, drawn_from),
                None => match other_stamp() {
                    Some(stream) => Built::new(stream, &d),
                    None => continue,
                },
            };
            // Drawn with references left as they are because earlier annotations spent the pass's
            // budget: not kept, so a later pass, where those hit the cache, draws it with them.
            if !starved {
                cache.insert(r, &built);
            }
            // Fails only when `r` isn't a dictionary, which the build just ruled out.
            apply_appearance(copy.get_or_insert_with(|| doc.clone()), r, &d, built).ok();
        }
    }
    copy
}
