//! The PDF Optimizer (execution plan M11.1; Acrobat: Reduce File Size and Optimize PDF ▸
//! Advanced optimization).
//!
//! - **Merge image fragments:** a page picture some PDF printers draw as thousands of images one
//!   pixel row high is put back together into one image first (see [`fragments`]), so the image
//!   settings can work on it.
//! - **Images:** each image's effective resolution is measured where pages draw it (the
//!   smallest over its uses, through form XObjects); colour and grayscale images above a
//!   threshold are resampled (bicubic) to a target resolution and recompressed as JPEG or Flate.
//!   A new image replaces the old one only if it is smaller. The colour and grey settings leave
//!   alone images they can't decode faithfully (CMYK and other colour spaces, masks, decode arrays
//!   other than the default, JPEG 2000, JBIG2, CCITT, other than 8 bits, JPEGs whose Adobe marker
//!   and `/ColorTransform` disagree). Black-and-white images (1-bit, image masks and CCITT fax,
//!   Group 3 among them; and eight-bit grey and RGB of only black and white, and indexed images
//!   that use only palette entries 0 and 1, packed to one bit) above a threshold are averaged down to a
//!   target resolution, halftones and dithers aside, and are stored as CCITT Group 4 or lossless JBIG2, or 1-bit
//!   Flate once resampled or packed, when that is smaller.
//! - **Discard objects:** page thumbnails, alternate images, document tags (structure tree),
//!   print settings.
//! - **Clean up:** drop images and forms that pages list but never draw (such as a deleted
//!   image); Flate-compress streams that have no filter; remove a redundant outer ASCII85 or
//!   ASCIIHex layer from streams that have one; store LZW-compressed streams with Flate instead.
//!
//! Metadata, attachments, comments, scripts, private data, hidden layers, bookmarks and form
//! fields are discarded by `pdfcraft-redact`'s Remove Hidden Information, which the engine
//! runs alongside.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod audit;
pub mod fragments;
mod images;
mod links;
mod unused;

pub use audit::{SpaceCategory, SpaceUse, audit_space};

use pdfcraft_cos::{Dict, Document, MAX_PAGE_DECODED, ObjRef, Object, Stream, serialize};
use pdfcraft_filters::{Filter, Params};

pub use images::effective_resolutions;

#[derive(Debug, thiserror::Error)]
pub enum OptimizeError {
    #[error("the document has no page tree")]
    NoPages,
    #[error("the document's encryption requires PDF {required} or later (chosen: PDF {chosen})")]
    IncompatibleEncryption { required: &'static str, chosen: &'static str },
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
    #[error("the optimization was cancelled")]
    Cancelled,
}

/// The target header version and object-stream policy for an optimized copy.
///
/// Partial compatibility only: encryption requirements are checked, but content newer than
/// the target is neither detected nor converted. A lower header does not guarantee that
/// older readers can read the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compatibility {
    /// Keep the document's version (upgraded to 1.5 when object streams are written).
    #[default]
    Retain,
    Pdf1_4,
    Pdf1_5,
    Pdf1_6,
    Pdf1_7,
    Pdf2_0,
}

impl Compatibility {
    pub const ALL: [Compatibility; 6] =
        [Compatibility::Retain, Compatibility::Pdf1_4, Compatibility::Pdf1_5, Compatibility::Pdf1_6, Compatibility::Pdf1_7, Compatibility::Pdf2_0];

    /// The name tools use (`"retain"`, `"1.4"`, `"1.5"`, `"1.6"`, `"1.7"`, `"2.0"`).
    pub fn name(self) -> &'static str {
        match self {
            Compatibility::Retain => "retain",
            Compatibility::Pdf1_4 => "1.4",
            Compatibility::Pdf1_5 => "1.5",
            Compatibility::Pdf1_6 => "1.6",
            Compatibility::Pdf1_7 => "1.7",
            Compatibility::Pdf2_0 => "2.0",
        }
    }

    pub fn from_name(name: &str) -> Option<Compatibility> {
        let trimmed = name.trim();
        let stripped = if trimmed.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("pdf ")) { &trimmed[4..] } else { trimmed };
        Compatibility::ALL.into_iter().find(|c| c.name().eq_ignore_ascii_case(stripped))
    }

    /// The three-character `%PDF-` header version, when a target version is chosen.
    pub fn version(self) -> Option<&'static str> {
        match self {
            Compatibility::Retain => None,
            _ => Some(self.name()),
        }
    }
}

/// Where an optimization is, reported to [`optimize_with_progress`]'s callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// About to process image `done` (0-based) of `total`; `done == total` once all are done.
    Images { done: usize, total: usize },
    /// Merging the fragments of a page picture into one image (only when a page has some).
    Fragments,
    /// Discarding objects, cleaning up links and compressing unencoded streams.
    CleanUp,
}

/// How resampled (or recompressed) images are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    /// JPEG with a quality from 1 to 100.
    Jpeg(u8),
    /// Lossless (zlib).
    Flate,
    /// Keep the image's own compression (resampled images become Flate if they weren't JPEG).
    Retain,
}

/// How black-and-white (1-bit) images are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonoCompression {
    /// CCITT Group 4 (lossless), when smaller than the image as it is.
    CcittG4,
    /// JBIG2, lossless (a generic region, or symbols when the image's marks repeat), when smaller
    /// than the image as it is. JBIG2Decode is PDF 1.4: an older document gets `/Version /1.4` in
    /// its catalog.
    Jbig2,
    /// Keep each image's own compression.
    Retain,
}

/// How black-and-white images (1-bit, CCITT fax among them; and eight-bit grey and RGB of only black
/// and white, and indexed images that use only palette entries 0 and 1, packed to one bit), and
/// image masks, are resampled and stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonoSettings {
    /// Average images whose effective resolution is above `above_ppi` down to `target_ppi`.
    pub downsample: bool,
    pub target_ppi: f64,
    pub above_ppi: f64,
    pub compression: MonoCompression,
}

impl MonoSettings {
    /// Reduce File Size and the presets: to 300 ppi above 450 ppi, CCITT Group 4. Text in a scan
    /// stays sharp at 300 ppi.
    pub const REDUCE: MonoSettings = MonoSettings { downsample: true, target_ppi: 300.0, above_ppi: 450.0, compression: MonoCompression::CcittG4 };
}

/// Acrobat's JPEG quality steps.
pub const QUALITIES: [(&str, u8); 5] = [("Minimum", 25), ("Low", 45), ("Medium", 60), ("High", 80), ("Maximum", 95)];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageSettings {
    /// Resample images whose effective resolution is above `above_ppi` down to `target_ppi`.
    pub downsample: bool,
    pub target_ppi: f64,
    pub above_ppi: f64,
    pub compression: Compression,
}

impl ImageSettings {
    /// Reduce File Size and the Standard preset: bicubic to 150 ppi above 225 ppi, JPEG medium
    /// quality.
    pub const REDUCE: ImageSettings = ImageSettings { downsample: true, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Jpeg(60) };
    /// The Mobile preset: bicubic to 96 ppi above 144 ppi, JPEG high quality. Pages read on a
    /// screen; text in scans stays legible.
    pub const MOBILE: ImageSettings = ImageSettings { downsample: true, target_ppi: 96.0, above_ppi: 144.0, compression: Compression::Jpeg(80) };
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Target PDF version (`Retain` by default).
    pub compatibility: Compatibility,
    /// Put page pictures drawn as many stacked fragments back together first ([`fragments`]).
    pub merge_fragments: bool,
    pub color: ImageSettings,
    pub gray: ImageSettings,
    /// Black-and-white images, and image masks.
    pub mono: MonoSettings,
    pub discard_thumbnails: bool,
    pub discard_alternate_images: bool,
    pub discard_tags: bool,
    pub discard_print_settings: bool,
    /// Flate-compress streams stored without a filter, and strip a redundant outer ASCII85 or
    /// ASCIIHex layer from streams that have one.
    pub flate_unencoded: bool,
    /// Clean Up: pack non-stream objects into compressed object streams with a cross-reference
    /// stream (PDF 1.5+, §7.5.7–7.5.8). Always off when `compatibility` is [`Compatibility::Pdf1_4`].
    pub object_streams: bool,
    /// Clean Up: decode LZW-compressed streams and store them with Flate instead.
    pub lzw_to_flate: bool,
    /// Clean Up: remove links (and bookmark destinations) that point nowhere.
    pub remove_invalid_links: bool,
    /// Clean Up: remove named destinations nothing refers to.
    pub remove_unreferenced_dests: bool,
}

impl Settings {
    /// Whether `write_full` should pack non-stream objects into compressed object streams:
    /// enabled when `object_streams` is set and the target version is not PDF 1.4 (§7.5.7).
    pub fn effective_object_streams(&self) -> bool {
        self.object_streams && self.compatibility != Compatibility::Pdf1_4
    }
}

impl Default for Settings {
    /// Acrobat's Reduce File Size choices.
    fn default() -> Self {
        Self {
            compatibility: Compatibility::Retain,
            merge_fragments: true,
            color: ImageSettings::REDUCE,
            gray: ImageSettings::REDUCE,
            mono: MonoSettings::REDUCE,
            discard_thumbnails: true,
            discard_alternate_images: true,
            discard_tags: false,
            discard_print_settings: false,
            flate_unencoded: true,
            object_streams: true,
            lzw_to_flate: true,
            remove_invalid_links: true,
            remove_unreferenced_dests: true,
        }
    }
}

/// The optimizer's ready-made settings, as in Acrobat's Settings menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Reduce File Size's choices ([`Settings::default`]).
    Standard,
    /// Smaller files for reading on a phone or screen: images to 96 ppi above 144, JPEG high.
    Mobile,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::Standard, Preset::Mobile];

    /// The name tools use.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Standard => "standard",
            Preset::Mobile => "mobile",
        }
    }

    pub fn from_name(name: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.name() == name)
    }

    pub fn settings(self) -> Settings {
        match self {
            Preset::Standard => Settings::default(),
            Preset::Mobile => Settings { color: ImageSettings::MOBILE, gray: ImageSettings::MOBILE, ..Settings::default() },
        }
    }

    /// The preset `settings` are, if they are one.
    pub fn of(settings: &Settings) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.settings() == *settings)
    }
}

/// What an optimization did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Images merged from fragments, and how many fragments went into them.
    pub merged_images: usize,
    pub fragments_merged: usize,
    pub images: usize,
    pub images_resampled: usize,
    pub images_recompressed: usize,
    /// Encoded image bytes before and after (images that changed).
    pub image_bytes_before: usize,
    pub image_bytes_after: usize,
    pub thumbnails: usize,
    pub alternate_images: usize,
    pub tags_removed: bool,
    pub print_settings: usize,
    pub streams_compressed: usize,
    /// Streams whose LZW compression was replaced with Flate (Clean Up).
    pub lzw_streams: usize,
    /// Links removed and bookmarks whose destination was cleared (Clean Up).
    pub invalid_links: usize,
    pub invalid_bookmarks: usize,
    pub unreferenced_dests: usize,
    /// Images and forms the pages listed but never drew, dropped (Clean Up).
    pub unused_xobjects: usize,
}

/// The minimum PDF version required by `doc`'s active security handler, when it uses features
/// introduced after PDF 1.4 (crypt filters in 1.5, AES-128 in 1.6, AES-256 in 1.7).
fn required_encryption_version(doc: &Document) -> Option<&'static str> {
    let h = doc.output_handler()?;
    let d = h.dict();
    if d.v >= 5 || d.crypt_filters.iter().any(|(_, m)| *m == pdfcraft_cos::CryptMethod::Aes256) {
        Some("1.7")
    } else if d.v == 4 {
        if d.crypt_filters.iter().any(|(_, m)| *m == pdfcraft_cos::CryptMethod::Aes128) { Some("1.6") } else { Some("1.5") }
    } else {
        None
    }
}

/// Optimize `doc` in place.
pub fn optimize(doc: &mut Document, settings: &Settings) -> Result<Report, OptimizeError> {
    optimize_with_progress(doc, settings, &mut |_| true)
}

/// [`optimize`], calling `progress` before each image and before the clean-up (and now and then
/// during a long one). Returning `false` stops with [`OptimizeError::Cancelled`]; `doc` is then
/// partly optimized and should be dropped.
pub fn optimize_with_progress(doc: &mut Document, settings: &Settings, progress: &mut dyn FnMut(Stage) -> bool) -> Result<Report, OptimizeError> {
    // Lexical ordering is valid for these fixed versions with single-digit components.
    if let Some(chosen) = settings.compatibility.version()
        && let Some(required) = required_encryption_version(doc)
        && chosen < required
    {
        return Err(OptimizeError::IncompatibleEncryption { required, chosen });
    }
    let mut report = Report::default();
    let pages = pdfcraft_annot::page_refs(doc).map_err(|_| OptimizeError::NoPages)?;
    if settings.merge_fragments {
        fragments::run(doc, &pages, &mut report, progress)?;
    }
    images::run(doc, &pages, settings, &mut report, progress)?;
    if !progress(Stage::CleanUp) {
        return Err(OptimizeError::Cancelled);
    }
    report.unused_xobjects = unused::remove_unused(doc, &pages, progress)?;
    if settings.discard_thumbnails {
        for p in &pages {
            if doc.get(*p).as_dict().is_some_and(|d| d.contains(b"Thumb")) {
                doc.update_dict(*p, |d| {
                    d.remove(b"Thumb");
                })?;
                report.thumbnails += 1;
            }
        }
    }
    if settings.discard_alternate_images {
        for num in doc.object_numbers() {
            let r = ObjRef { num, generation: doc.generation(num) };
            let alt = matches!(&*doc.get(r), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") && s.dict.contains(b"Alternates"));
            if alt {
                if let Object::Stream(s) = &*doc.get(r) {
                    let mut s = s.clone();
                    s.dict.remove(b"Alternates");
                    doc.set(r, Object::Stream(s));
                }
                report.alternate_images += 1;
            }
        }
    }
    let root = doc.root();
    if let Some(chosen) = settings.compatibility.version() {
        doc.set_version(chosen);
        if let Some(root) = root
            && doc.get(root).as_dict().is_some_and(|c| c.contains(b"Version"))
        {
            doc.update_dict(root, |c| {
                c.remove(b"Version");
            })?;
        }
    }
    if settings.discard_tags
        && let Some(root) = root
        && doc.get(root).as_dict().is_some_and(|c| c.contains(b"StructTreeRoot"))
    {
        doc.update_dict(root, |c| {
            c.remove(b"StructTreeRoot");
            c.remove(b"MarkInfo");
        })?;
        for p in &pages {
            if doc.get(*p).as_dict().is_some_and(|d| d.contains(b"StructParents")) {
                doc.update_dict(*p, |d| {
                    d.remove(b"StructParents");
                })?;
            }
        }
        report.tags_removed = true;
    }
    if settings.discard_print_settings
        && let Some(root) = root
    {
        const PRINT: [&[u8]; 7] = [b"PrintScaling", b"Duplex", b"PickTrayByPDFSize", b"PrintPageRange", b"NumCopies", b"PrintArea", b"PrintClip"];
        let vp = doc.get(root).as_dict().and_then(|c| c.get(b"ViewerPreferences").cloned());
        let edit = |d: &mut pdfcraft_cos::Dict| PRINT.iter().filter(|k| d.remove(k).is_some()).count();
        match vp {
            Some(Object::Ref(r)) => {
                let mut n = 0;
                doc.update_dict(r, |d| n = edit(d))?;
                report.print_settings += n;
            }
            Some(Object::Dict(_)) => {
                let mut n = 0;
                doc.update_dict(root, |c| {
                    if let Some(Object::Dict(d)) = c.get_mut(b"ViewerPreferences") {
                        n = edit(d);
                    }
                })?;
                report.print_settings += n;
            }
            _ => {}
        }
    }
    if settings.remove_invalid_links {
        let (l, b) = links::remove_invalid(doc, &pages)?;
        report.invalid_links = l;
        report.invalid_bookmarks = b;
    }
    if settings.remove_unreferenced_dests {
        report.unreferenced_dests = links::remove_unreferenced_dests(doc)?;
    }
    // Before the unencoded-stream pass, which would otherwise take the ASCII layer off an
    // `[/ASCII85Decode /LZWDecode]` stream first and count it twice.
    if settings.lzw_to_flate {
        for num in doc.object_numbers() {
            let r = ObjRef { num, generation: doc.generation(num) };
            let obj = doc.get(r);
            let Object::Stream(s) = &*obj else { continue };
            if let Some(replacement) = lzw_to_flate(s, MAX_LZW_DECODED) {
                doc.set(r, Object::Stream(replacement));
                report.lzw_streams += 1;
            }
        }
    }
    if settings.flate_unencoded {
        for num in doc.object_numbers() {
            let r = ObjRef { num, generation: doc.generation(num) };
            let obj = doc.get(r);
            let Object::Stream(s) = &*obj else { continue };
            // XMP stays readable as plain text (PDF/A requires it); tiny streams don't gain.
            if s.dict.name(b"Type") == Some(b"Metadata") || s.dict.name(b"Type") == Some(b"XRef") || s.raw.len() < 64 {
                continue;
            }
            let replacement = if s.dict.contains(b"Filter") {
                strip_ascii_wrapper(s)
            } else {
                let mut d = s.dict.clone();
                d.remove(b"Length");
                let packed = Stream::flate(d, &s.raw);
                (packed.raw.len() < s.raw.len()).then_some(packed)
            };
            if let Some(replacement) = replacement {
                doc.set(r, Object::Stream(replacement));
                report.streams_compressed += 1;
            }
        }
    }
    Ok(report)
}

/// Largest decoded LZW stream converted, as for a page's content: the data and its check are
/// held together, so a few hundred kilobytes of hostile LZW can't ask for gigabytes.
const MAX_LZW_DECODED: usize = MAX_PAGE_DECODED;

/// `s` with its LZW compression replaced by Flate (Acrobat's "In streams that use LZW encoding,
/// use Flate instead"). The filters up to the last LZW one are decoded, strictly and with its
/// predictor; the result is Flate-encoded under the same predictor, without `EarlyChange`, and
/// the filters after it keep their encoded bytes. A stream that doesn't decode cleanly, or
/// whose filters or parameters can't be read for certain, or that decodes past `max` bytes, is
/// left as it is.
fn lzw_to_flate(s: &Stream, max: usize) -> Option<Stream> {
    // An external file's data isn't in `raw`; cross-reference and object streams are rewritten
    // on save, so converting them would change nothing in the file.
    if s.dict.contains(b"F") || matches!(s.dict.name(b"Type"), Some(b"XRef" | b"ObjStm")) {
        return None;
    }
    let names: Vec<&[u8]> = match s.dict.get(b"Filter")? {
        Object::Name(n) => vec![n.as_slice()],
        Object::Array(a) => a.iter().map(Object::as_name).collect::<Option<_>>()?,
        _ => return None,
    };
    // `/Crypt` was applied when the object was loaded, and dropping it would change how the
    // stream is encrypted.
    if names.contains(&&b"Crypt"[..]) {
        return None;
    }
    let lzw = names.iter().rposition(|n| matches!(*n, b"LZWDecode" | b"LZW"))?;
    // One entry per filter, each a dictionary or null; anything else (an indirect reference, an
    // array of the wrong length) would be read as defaults, so the data might not decode as
    // written.
    let parms: Vec<Option<&Dict>> = match s.dict.get(b"DecodeParms") {
        None | Some(Object::Null) => vec![None; names.len()],
        Some(Object::Dict(d)) if names.len() == 1 => vec![Some(d)],
        Some(Object::Array(a)) if a.len() == names.len() => a
            .iter()
            .map(|p| match p {
                Object::Dict(d) => Some(Some(d)),
                Object::Null => Some(None),
                _ => None,
            })
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let chain = s.filters();
    let stages = chain.get(..=lzw)?;
    let (_, params) = stages.last()?;
    // The decoded stages' parameters must be direct integers: `filters` reads anything else (a
    // reference, 12.5, a name) as the default, and the dictionary carried over would then
    // describe the new data differently from how it was encoded.
    let direct = |p: &Option<&Dict>| p.is_none_or(|d| d.iter().all(|(_, v)| matches!(v, Object::Int(_))));
    if !parms.get(..=lzw)?.iter().all(direct) {
        return None;
    }
    let data = pdfcraft_filters::decode(stages, &s.raw, max).ok()?;
    let flate = Params { early_change: 1, ..params.clone() };
    let raw = pdfcraft_filters::encode(&Filter::Flate, &flate, &data).ok()?;
    // The predictor's encoder must give back exactly what its decoder read (a short last row,
    // unusual parameters), or the stream would change. The limit counts a PNG predictor's tag
    // bytes too, at most one per row of at least one byte.
    if pdfcraft_filters::decode(&[(Filter::Flate, flate)], &raw, data.len().saturating_mul(2).saturating_add(1)).ok()? != data {
        return None;
    }
    // Without a predictor the other parameters say nothing to Flate.
    let first = parms.get(lzw).copied().flatten().filter(|_| params.predictor > 1).map(|d| {
        let mut d = d.clone();
        d.remove(b"EarlyChange");
        Object::Dict(d)
    });
    let rest_names = names.get(lzw + 1..)?;
    let rest_parms = parms.get(lzw + 1..)?;
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    if rest_names.is_empty() {
        dict.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        match first {
            Some(p) => dict.set(b"DecodeParms".to_vec(), p),
            None => {
                dict.remove(b"DecodeParms");
            }
        }
    } else {
        let filters = std::iter::once(Object::name("FlateDecode")).chain(rest_names.iter().map(|n| Object::Name(n.to_vec())));
        dict.set(b"Filter".to_vec(), Object::Array(filters.collect()));
        let parms: Vec<Object> = std::iter::once(first.unwrap_or(Object::Null))
            .chain(rest_parms.iter().map(|p| p.map_or(Object::Null, |d| Object::Dict(d.clone()))))
            .collect();
        if parms.iter().all(|p| matches!(p, Object::Null)) {
            dict.remove(b"DecodeParms");
        } else {
            dict.set(b"DecodeParms".to_vec(), Object::Array(parms));
        }
    }
    Some(Stream::from_raw(dict, raw))
}

/// `s` without its outer ASCII85 or ASCIIHex layer (`[/ASCII85Decode /FlateDecode]` becomes
/// `[/FlateDecode]`), when that is smaller. Only the outer layer is decoded: strictly, and within
/// the stored size, which ASCII text never falls below, so the inner encoded bytes stay exact.
fn strip_ascii_wrapper(s: &Stream) -> Option<Stream> {
    if s.dict.contains(b"DecodeParms") || s.dict.name(b"Type") == Some(b"ObjStm") || s.dict.name(b"Subtype") == Some(b"Image") {
        return None;
    }
    let Some(Object::Array(filters)) = s.dict.get(b"Filter") else { return None };
    let names = filters.iter().map(Object::as_name).collect::<Option<Vec<_>>>()?;
    let (outer, inner) = names.split_first()?;
    if !matches!(*outer, b"ASCII85Decode" | b"ASCIIHexDecode") || inner.is_empty() || inner.contains(&&b"Crypt"[..]) {
        return None;
    }
    let data = s.decoded_first_filter_strict(s.raw.len()).ok()?;
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    dict.set(b"Filter".to_vec(), Object::Array(inner.iter().map(|n| Object::Name(n.to_vec())).collect()));
    let candidate = Stream::from_raw(dict, data);
    (stored_len(&candidate)? < stored_len(s)?).then_some(candidate)
}

/// Bytes the stream object takes in the file: its dictionary with `/Length` set as the writer
/// sets it, then its data.
pub(crate) fn stored_len(s: &Stream) -> Option<usize> {
    let mut dict = s.dict.clone();
    dict.set(b"Length".to_vec(), Object::Int(i64::try_from(s.raw.len()).ok()?));
    let mut out = Vec::new();
    serialize(&Object::Dict(dict), &mut out);
    out.len().checked_add(s.raw.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_lzw_stream_that_decodes_past_the_limit_is_left_as_it_is() {
        let data = b"0 0 m 612 792 l S\n".repeat(64);
        let raw = pdfcraft_filters::encode(&Filter::Lzw, &Params::default(), &data).unwrap();
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::name("LZWDecode"));
        let s = Stream::from_raw(dict, raw);
        assert!(lzw_to_flate(&s, data.len() - 1).is_none());
        let converted = lzw_to_flate(&s, data.len()).unwrap();
        assert_eq!(converted.decoded_strict().unwrap(), data);
    }
}
