//! The last check of [`super::flatten_layers`]: no layer's block is left in any stream a reader may
//! draw, whatever path draws it.

use std::collections::HashSet;

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use super::{appearances, has_name, is_content_stream, opens_layer_block};
use crate::RedactError;
use crate::sanitize::sub;

/// How deep [`Roles::walk`] follows the dictionaries and arrays held directly in one object.
const MAX_DEPTH: usize = 32;

/// Refuses a result in which a stream a full save writes may still hold a layer's block: the
/// layers are gone, so a reader would show it. Each stream is read as far as a reader reads it
/// (a damaged one up to the damage). One that can't be read at all is refused unless what it is
/// says no reader draws it, or the form pass already kept it ([`Role`]); attachments, images, font files and ICC profiles are not
/// read.
pub(super) fn refuse_layer_blocks_left(doc: &Document) -> Result<(), RedactError> {
    let mut kept: Vec<u32> = pdfcraft_cos::reachable_objects(doc)?.into_iter().collect();
    kept.sort_unstable();
    let roles = Roles::read(doc, &kept);
    for n in kept {
        let r = ObjRef::new(n, doc.generation(n));
        let Object::Stream(s) = &*doc.get(r) else { continue };
        let role = roles.of(doc, r, &s.dict);
        if matches!(role, Role::NotDrawn) {
            continue;
        }
        match may_hold_layer_block(doc, s) {
            Some(false) => {}
            None if matches!(role, Role::Judged) => {}
            Some(true) | None => return Err(RedactError::LayerContentUnreadable(n)),
        }
    }
    Ok(())
}

/// Whether `s` may hold a layer's block, read as a reader reads it; `None` if it can't be read.
/// A damaged stream is read up to the damage. Bytes the parser couldn't read, or inline image
/// data whose end is a guess, may hide one: there a `BDC` in the text that no parsed operator
/// accounts for counts as one (it may be in a string or a comment, so this over-refuses).
fn may_hold_layer_block(doc: &Document, s: &Stream) -> Option<bool> {
    let (data, _) = DecodeBudget::page().decode_with_damage(doc, s, usize::MAX).ok()?;
    let parsed = pdfcraft_content::parse(&data);
    if parsed.ops.iter().any(opens_layer_block) {
        return Some(true);
    }
    let unread = parsed.skipped > 0 || parsed.ambiguous_images > 0;
    let in_text = data.windows(3).filter(|w| *w == b"BDC").count();
    Some(unread && in_text > parsed.ops.iter().filter(|op| op.is("BDC")).count())
}

/// What a stream is, as far as the check needs to know.
enum Role {
    /// Content, or drawn some way the check doesn't know: read, and refused if unreadable.
    Read,
    /// Read, but kept if unreadable: metadata, as nothing stops a reader drawing it, and a form or
    /// tiling pattern with `/Resources` of its own, which [`super::unwrap_forms`] has already
    /// refused if those can name a layer.
    Judged,
    /// An attachment a file specification's `/EF` names, an image, a font file or an ICC
    /// profile: not read.
    NotDrawn,
}

/// The streams whose role is told by what names them, not by their own dictionaries.
#[derive(Default)]
struct Roles {
    /// Drawn as content: pages' `/Contents`, Type 3 glyphs (`/CharProcs`) and annotation
    /// appearances, whatever their dictionaries say.
    drawn: HashSet<ObjRef>,
    /// The embedded files a file specification's `/EF` names.
    attached: HashSet<ObjRef>,
}

impl Roles {
    fn read(doc: &Document, kept: &[u32]) -> Self {
        let mut roles = Roles { drawn: appearances(doc), ..Default::default() };
        for &n in kept {
            roles.walk(doc, &doc.get(ObjRef::new(n, doc.generation(n))), 0);
        }
        roles
    }

    /// Notes every dictionary held directly in `o`, to [`MAX_DEPTH`].
    fn walk(&mut self, doc: &Document, o: &Object, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let d = match o {
            Object::Dict(d) => d,
            Object::Stream(s) => &s.dict,
            Object::Array(a) => {
                a.iter().for_each(|v| self.walk(doc, v, depth.saturating_add(1)));
                return;
            }
            _ => return,
        };
        self.note(doc, d);
        for (_, v) in d.iter() {
            if matches!(v, Object::Dict(_) | Object::Array(_)) {
                self.walk(doc, v, depth.saturating_add(1));
            }
        }
    }

    fn note(&mut self, doc: &Document, d: &Dict) {
        let named_in = |key: &[u8]| sub(doc, d, key).map(|e| e.iter().filter_map(|(_, v)| v.as_ref()).collect::<Vec<_>>());
        self.attached.extend(named_in(b"EF").unwrap_or_default());
        if has_name(doc, d, b"Subtype", b"Type3") {
            self.drawn.extend(named_in(b"CharProcs").unwrap_or_default());
        }
        if let Some(c) = d.get(b"Contents").filter(|_| has_name(doc, d, b"Type", b"Page")) {
            self.drawn.extend(c.as_ref());
            if let Some(a) = doc.resolve(c).as_array() {
                self.drawn.extend(a.iter().filter_map(Object::as_ref));
            }
        }
    }

    /// What `r`, a stream with dictionary `d`, is. What [`Roles::drawn`] holds is read first,
    /// so a glyph posing as a form with `/Resources` is still refused when unreadable (§9.6.5).
    fn of(&self, doc: &Document, r: ObjRef, d: &Dict) -> Role {
        let content = is_content_stream(doc, d);
        if self.drawn.contains(&r) {
            Role::Read
        } else if content && d.contains(b"Resources") {
            Role::Judged
        } else if content {
            Role::Read
        } else if self.attached.contains(&r) || is_data(doc, d) {
            Role::NotDrawn
        } else if has_name(doc, d, b"Type", b"Metadata") {
            Role::Judged
        } else {
            Role::Read
        }
    }
}

/// Is this stream, by its dictionary, an image, a font file (ISO 32000-1 §9.9) or an ICC
/// profile (`/N`, §8.6.5.5)?
fn is_data(doc: &Document, d: &Dict) -> bool {
    let font_file = [b"Type1C".as_slice(), b"CIDFontType0C", b"OpenType"];
    has_name(doc, d, b"Subtype", b"Image")
        || font_file.iter().any(|n| has_name(doc, d, b"Subtype", n))
        || [b"Length1".as_slice(), b"Length2", b"Length3", b"N"].iter().any(|k| d.contains(k))
}
