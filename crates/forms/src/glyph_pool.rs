//! One object per Type 3 glyph procedure across a form's appearances (#6867).
//!
//! Each widget appearance that draws characters the standard fonts lack builds its own Type 3
//! fonts (`pdfcraft_fonts::shaped_text`), and a cluster's glyph procedure is the same bytes in
//! every one of them. Storing each appearance's streams on their own put the same procedure into
//! the file once per widget. [`LazyPool`] stores them through a
//! `pdfcraft_fonts::shaped_text::GlyphPool` seeded from the procedures the form's appearances
//! already hold, so an equal one is referred to instead. Fonts, codes and content stay per
//! appearance: what is drawn doesn't change.

use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_fonts::shaped_text::{GlyphPool, store_streams, store_streams_pooled};

use crate::read::fields;

#[cfg(test)]
thread_local! {
    /// How many times [`glyph_pool`] read the form's widgets, for tests.
    pub(crate) static SEEDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A glyph pool read from the document the first time an appearance with a Type 3 font is stored,
/// and kept for the rest of one public call (a fill, a batch, a reset), so a batch reads the form
/// once. It holds references into the document as it was during that call, so it is created anew
/// for each call and never kept across one: an undo replaces the document, and a pool from before
/// it could point at objects the restored document doesn't have.
#[derive(Default)]
pub(crate) struct LazyPool {
    pool: Option<GlyphPool>,
}

impl LazyPool {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Move the streams in `d` (a generated appearance's dictionary, not yet in `doc`) into `doc`,
    /// as `store_streams` does, with equal Type 3 glyph procedures and ToUnicode maps stored once.
    /// An appearance without a Type 3 font is stored as before, without reading the form: Latin
    /// text costs nothing more.
    pub(crate) fn store(&mut self, doc: &mut Document, d: &mut Dict) {
        if self.pool.is_none() && !has_type3_font(d) {
            store_streams(doc, d);
            return;
        }
        let pool = self.pool.get_or_insert_with(|| glyph_pool(doc));
        store_streams_pooled(doc, d, pool);
    }
}

/// Whether the appearance dictionary `d`, as `appearance` builds it (its resources direct), names
/// a Type 3 font.
fn has_type3_font(d: &Dict) -> bool {
    let fonts = d.get(b"Resources").and_then(Object::as_dict).and_then(|r| r.get(b"Font")).and_then(Object::as_dict);
    fonts.is_some_and(|fonts| {
        fonts.iter().any(|(_, f)| f.as_dict().and_then(|f| f.get(b"Subtype")).and_then(Object::as_name) == Some(b"Type3".as_slice()))
    })
}

/// The glyph procedures and ToUnicode maps of the Type 3 fonts in the normal appearances of the
/// form's widgets. Each step reads one level (widget, `/AP`, `/N`, `/Resources`, `/Font`, font),
/// never a chain, so a cyclic or deep file can't make it loop; what isn't what it should be is
/// passed over, and `GlyphPool::remember_font` bounds how much it reads.
fn glyph_pool(doc: &Document) -> GlyphPool {
    #[cfg(test)]
    SEEDS.with(|c| c.set(c.get() + 1));
    let mut pool = GlyphPool::new();
    for f in fields(doc) {
        for w in &f.widgets {
            let widget = doc.get(w.obj);
            let Some(ap) = widget.as_dict().and_then(|d| d.get(b"AP")).map(|a| doc.resolve(a)) else { continue };
            let Some(normal) = ap.as_dict().and_then(|a| a.get(b"N")).map(|n| doc.resolve(n)) else { continue };
            let Object::Stream(s) = &*normal else { continue };
            let Some(resources) = s.dict.get(b"Resources").map(|r| doc.resolve(r)) else { continue };
            let Some(fonts) = resources.as_dict().and_then(|r| r.get(b"Font")).map(|f| doc.resolve(f)) else { continue };
            let Some(fonts) = fonts.as_dict() else { continue };
            for (_, font) in fonts.iter() {
                let font = doc.resolve(font);
                if let Some(font) = font.as_dict() {
                    pool.remember_font(doc, font);
                }
            }
        }
    }
    pool
}
