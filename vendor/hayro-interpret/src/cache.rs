use crate::util::hash128;
use crate::{InterpreterWarning, WarningSinkFn};
use hayro_syntax::object::{Array, Dict, MaybeRef, Name, Null, ObjRef, Object, Stream};
use kurbo::Affine;
use std::any::Any;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

type CacheMap = HashMap<u128, Option<Box<dyn Any + Send + Sync>>>;
/// The objects read from a document, by key. PdfCraft patch (48): the second field counts the
/// ICC profiles read through it ([`Cache::icc_profile_reads`]). PdfCraft patch (80): the third
/// counts the `/Indexed` lookup tables read ([`Cache::indexed_lookup_reads`]).
#[derive(Clone)]
pub(crate) struct Cache(Arc<Mutex<CacheMap>>, Arc<AtomicUsize>, Arc<AtomicUsize>);
impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

impl Cache {
    pub(crate) fn new() -> Self {
        Self(
            Arc::new(Mutex::new(HashMap::new())),
            Arc::default(),
            Arc::default(),
        )
    }

    /// PdfCraft patch (48): count one ICC profile read, the colour space of an `/ICCBased`
    /// profile no entry held yet being built from it.
    pub(crate) fn count_icc_profile_read(&self) {
        self.1.fetch_add(1, Ordering::Relaxed);
    }

    /// PdfCraft patch (48): how many ICC profiles were read through this cache.
    pub(crate) fn icc_profile_reads(&self) -> usize {
        self.1.load(Ordering::Relaxed)
    }

    /// PdfCraft patch (80): count one `/Indexed` lookup table read (a stream decoded or a
    /// string copied).
    pub(crate) fn count_indexed_lookup_read(&self) {
        self.2.fetch_add(1, Ordering::Relaxed);
    }

    /// PdfCraft patch (80): how many `/Indexed` lookup tables were read through this cache.
    pub(crate) fn indexed_lookup_reads(&self) -> usize {
        self.2.load(Ordering::Relaxed)
    }

    pub(crate) fn get_or_insert_with<T: Clone + Send + Sync + 'static>(
        &self,
        id: u128,
        f: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let mut locked = self.0.lock().unwrap();

        // We can't use `get_or_insert_with` here, because if the closure makes another access to the
        // cache, we end up with a deadlock.
        match locked.entry(id) {
            Entry::Occupied(o) => o
                .get()
                .as_ref()
                .and_then(|val| val.downcast_ref::<T>().cloned()),
            Entry::Vacant(_) => {
                drop(locked);
                let val = f();
                self.0.lock().unwrap().insert(
                    id,
                    val.clone()
                        .map(|val| Box::new(val) as Box<dyn Any + Send + Sync>),
                );

                val
            }
        }
    }
}

/// PdfCraft patch: a byte-budgeted cache of decoded images, keyed by everything a decode reads
/// ([`DecodedImageKey`]). Image-heavy pages decode thousands of images; without this every tile
/// and every scale decodes them all again. Entries hold owned pixels, so a hit never changes
/// them; when an insert would pass the budget the whole map is dropped (a fresh epoch), which
/// only costs re-decoding, and an image larger than the whole budget is never kept. The budget
/// caps memory per interpreter cache. A poisoned lock decodes without the cache.
///
/// PdfCraft patch (91): a decode that fails is kept too, as the warnings it reported, so an
/// image drawn again and again that doesn't decode (damaged data, a JPX or JBIG2 stream its
/// decoder refuses) isn't decoded again at every draw: a later draw reports the same warnings
/// and gets nothing, as the decode did. A failure counts [`FAILED_DECODE_BYTES`] towards the
/// budget, so failures can't grow the map without bound either. A failure that may come from
/// what the page had left (its JBIG2 allowance, `ContentTruncated`) isn't kept: another page,
/// or the same page drawn again, starts with a fresh allowance. One that left out a JBIG2 mask
/// for a limit its stream reaches by itself is kept, with the image it takes along (patch 73,
/// [`note_stream_limited`]): decoding them again would only refuse them again, and an image of
/// any size under such a mask was decoded again at every draw.
#[derive(Clone)]
pub(crate) struct DecodedImageCache {
    inner: Arc<Mutex<DecodedImageCacheInner>>,
    /// Whether the render this handle serves is a preview (`InterpreterSettings::preview`).
    /// Part of every key, so a preview's entries are never handed to a final render.
    pub(crate) preview: bool,
}

/// PdfCraft patch: what a decoded image depends on. Two images share an entry only when all of
/// it is equal:
/// - where the stream's dictionary lies in the document's data (`dict_at`, `dict_len`): a
///   stream's data follows its dictionary, so this tells apart streams whose dictionaries are
///   byte for byte the same (two uncompressed images of one size), which `dict_hash` alone
///   would not; the data stays put for as long as the document (and so this cache) lives;
/// - the dictionary's bytes (`dict_hash`: `/Decode`, `/ColorSpace`, `/SMask`, `/Mask`,
///   `/BitsPerComponent`, filters, …) and its object (`obj`: the decryption key depends on it);
/// - whether it is decoded as a mask, a stencil mask, raster or mask output, interpolation,
///   the decode target size (only for an image a JPX stream decodes, which is the only
///   decode that reads it: PdfCraft patch 32), and preview or final quality;
/// - the default colour space drawn in place of the image's device space, if any (the cache
///   key of its object, PdfCraft patch 38): the same image can be drawn by resources with a
///   `/DefaultRGB` and by resources without one.
///
/// A transfer function (graphics state `/TR`) also changes the pixels and has no cheap key, so
/// an image drawn under one is not cached; neither is an inline image, whose dictionary lives in
/// a content stream and whose colour space can be a name resolved through the resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DecodedImageKey {
    pub(crate) mask_output: bool,
    pub(crate) dict_at: usize,
    pub(crate) dict_len: usize,
    pub(crate) dict_hash: u128,
    pub(crate) obj: Option<(i32, i32)>,
    pub(crate) is_mask: bool,
    pub(crate) is_stencil_mask: bool,
    pub(crate) interpolate: bool,
    pub(crate) default_space: Option<u128>,
    pub(crate) target: Option<(u32, u32)>,
    pub(crate) preview: bool,
    /// PdfCraft patch (82): the size a soft mask far larger than where it is drawn was averaged
    /// down to (`ImageXObject::decoded_soft_mask`), `None` for a mask kept at its own size.
    pub(crate) shrunk_to: Option<(u32, u32)>,
}

/// PdfCraft patch (91): what the budget charges for a failed decode, about what its key and
/// entry take in the map.
const FAILED_DECODE_BYTES: usize = 128;

/// PdfCraft patch (91): with the warnings of a failure kept, that it left out a JBIG2 mask for a
/// limit its stream reaches by itself ([`note_stream_limited`]); no warning's flag.
const STREAM_LIMITED: u8 = 128;

std::thread_local! {
    static LEFT_OUT_BY_STREAM_LIMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// PdfCraft patch (73): note, wherever a decode reports `ContentTruncated` for an image or a kept
/// failure replays it, whether that left out a JBIG2 mask for a limit its stream reaches by itself
/// (`hayro_syntax::jbig2_limited_by_stream`): its failure, and the failure of the image it takes
/// along, is then the stream's to keep, since decoding it again refuses it again.
pub(crate) fn note_stream_limited(limited: bool) {
    LEFT_OUT_BY_STREAM_LIMIT.with(|left_out| left_out.set(limited));
}

fn stream_limited() -> bool {
    LEFT_OUT_BY_STREAM_LIMIT.with(std::cell::Cell::get)
}

/// PdfCraft patch (91): what a [`DecodedImageCache`] keeps for a key.
enum DecodedEntry {
    /// The decoded image.
    Decoded(Box<dyn Any + Send + Sync>),
    /// The decode failed, reporting these warnings ([`warning_bit`] flags).
    Failed(u8),
}

/// PdfCraft patch (91): `warning` as a flag in a set of warnings.
fn warning_bit(warning: InterpreterWarning) -> u8 {
    match warning {
        InterpreterWarning::UnsupportedFont => 1,
        InterpreterWarning::ImageDecodeFailure => 2,
        InterpreterWarning::ContentTruncated => 4,
        InterpreterWarning::ColorFallback => 8,
        InterpreterWarning::Type3WidthsMissing => 16,
        InterpreterWarning::MalformedIndexedImage => 32,
    }
}

/// PdfCraft patch (91): the warnings in a set of [`warning_bit`] flags.
fn warnings_in(bits: u8) -> impl Iterator<Item = InterpreterWarning> {
    [
        InterpreterWarning::UnsupportedFont,
        InterpreterWarning::ImageDecodeFailure,
        InterpreterWarning::ContentTruncated,
        InterpreterWarning::ColorFallback,
        InterpreterWarning::Type3WidthsMissing,
        InterpreterWarning::MalformedIndexedImage,
    ]
    .into_iter()
    .filter(move |warning| bits & warning_bit(*warning) != 0)
}

#[derive(Default)]
struct DecodedImageCacheInner {
    map: HashMap<DecodedImageKey, DecodedEntry>,
    bytes: usize,
    /// PdfCraft patch (32): how many keyed decodes ran (cache misses).
    decodes: usize,
    /// PdfCraft patch (82): how many of `decodes` were of masks (keys with `mask_output`).
    mask_decodes: usize,
}

impl DecodedImageCache {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DecodedImageCacheInner::default())),
            preview: false,
        }
    }

    /// PdfCraft patch (32): how many decodes the cache was asked to keep (its misses), for
    /// diagnostics and tests. 0 when the lock is poisoned.
    pub(crate) fn decodes(&self) -> usize {
        self.inner.lock().map(|locked| locked.decodes).unwrap_or(0)
    }

    /// PdfCraft patch (82): how many of [`Self::decodes`] were of soft masks and `/Mask` images,
    /// for tests. 0 when the lock is poisoned.
    pub(crate) fn mask_decodes(&self) -> usize {
        self.inner
            .lock()
            .map(|locked| locked.mask_decodes)
            .unwrap_or(0)
    }

    /// The same cache, for a preview (`true`) or a final render.
    pub(crate) fn for_render(&self, preview: bool) -> Self {
        Self {
            inner: self.inner.clone(),
            preview,
        }
    }

    /// The decoded image for `key`, from the cache or from `f`, which decodes reporting its
    /// warnings to the sink it is handed. `None` as the key decodes without the cache.
    /// PdfCraft patch (91): `sink` gets the warnings of the decode, or those of the failed
    /// decode kept for `key`.
    pub(crate) fn get_or_insert_with<T: Clone + Send + Sync + 'static>(
        &self,
        key: Option<DecodedImageKey>,
        budget: usize,
        size_of: impl FnOnce(&T) -> usize,
        sink: &WarningSinkFn,
        f: impl FnOnce(&WarningSinkFn) -> Option<T>,
    ) -> Option<T> {
        let Some(key) = key else { return f(sink) };
        // Fast path: a hit clones the pixels without decoding, and a kept failure reports its
        // warnings again without decoding.
        let hit = match self.inner.lock() {
            Ok(locked) => match locked.map.get(&key) {
                Some(DecodedEntry::Decoded(val)) => val.downcast_ref::<T>().cloned().map(Ok),
                Some(DecodedEntry::Failed(bits)) => Some(Err(*bits)),
                None => None,
            },
            // A thread panicked while holding the lock: decode without the cache.
            Err(_) => return f(sink),
        };
        match hit {
            Some(Ok(hit)) => return Some(hit),
            Some(Err(bits)) => {
                note_stream_limited(bits & STREAM_LIMITED != 0);
                warnings_in(bits).for_each(|warning| sink(warning));
                return None;
            }
            None => {}
        }
        // Miss: decode without holding the lock (decoding must never deadlock the cache),
        // noting the warnings it reports.
        let reported = Arc::new(AtomicU8::new(0));
        let recording: WarningSinkFn = {
            let (reported, sink) = (reported.clone(), sink.clone());
            Arc::new(move |warning| {
                reported.fetch_or(warning_bit(warning), Ordering::Relaxed);
                sink(warning);
            })
        };
        let val = f(&recording);
        let reported = reported.load(Ordering::Relaxed);
        let Ok(mut locked) = self.inner.lock() else {
            return val;
        };
        locked.decodes = locked.decodes.saturating_add(1);
        if key.mask_output {
            locked.mask_decodes = locked.mask_decodes.saturating_add(1);
        }
        // Another thread may have decoded it meanwhile: keep the first copy.
        match locked.map.get(&key) {
            Some(DecodedEntry::Decoded(existing)) => {
                if let Some(existing) = existing.downcast_ref::<T>() {
                    return Some(existing.clone());
                }
            }
            Some(DecodedEntry::Failed(_)) if val.is_none() => return None,
            _ => {}
        }
        let truncated = reported & warning_bit(InterpreterWarning::ContentTruncated) != 0;
        let size = match &val {
            Some(v) => size_of(v),
            // A failure the page's JBIG2 allowance may have caused isn't the image's to keep, nor
            // one a JBIG2 decode's work did, which may be the page's too.
            None if hayro_syntax::page_jbig2_work_spent() || (truncated && !stream_limited()) => {
                return None;
            }
            None => FAILED_DECODE_BYTES,
        };
        if size > budget {
            return val;
        }
        if locked.bytes.saturating_add(size) > budget {
            locked.map.clear();
            locked.bytes = 0;
        }
        let entry = match &val {
            Some(val) => DecodedEntry::Decoded(Box::new(val.clone())),
            // Here only when a limit of its stream refused the mask that left it out (above).
            None if truncated => DecodedEntry::Failed(reported | STREAM_LIMITED),
            None => DecodedEntry::Failed(reported),
        };
        // An entry replaced (a failure another thread kept) isn't given back: the budget
        // errs towards a fresh epoch.
        locked.bytes = locked.bytes.saturating_add(size);
        locked.map.insert(key, entry);
        val
    }
}

/// A trait for objects that can generate a unique cache key.
pub trait CacheKey {
    /// Returns the cache key for this object.
    fn cache_key(&self) -> u128;
}

impl<T: CacheKey, U: CacheKey> CacheKey for (T, U) {
    fn cache_key(&self) -> u128 {
        hash128(&(self.0.cache_key(), self.1.cache_key()))
    }
}

impl CacheKey for Dict<'_> {
    fn cache_key(&self) -> u128 {
        hash128(self.data())
    }
}

impl CacheKey for Stream<'_> {
    fn cache_key(&self) -> u128 {
        self.dict().cache_key()
    }
}

impl CacheKey for Null {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}

impl CacheKey for bool {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}

impl CacheKey for hayro_syntax::object::Number {
    fn cache_key(&self) -> u128 {
        hash128(&self.as_f64().to_bits())
    }
}

impl CacheKey for hayro_syntax::object::String<'_> {
    fn cache_key(&self) -> u128 {
        hash128(self.as_ref())
    }
}

impl CacheKey for Name<'_> {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}

impl CacheKey for Array<'_> {
    fn cache_key(&self) -> u128 {
        hash128(self.data())
    }
}

impl CacheKey for Object<'_> {
    fn cache_key(&self) -> u128 {
        match self {
            Object::Null(n) => n.cache_key(),
            Object::Boolean(b) => b.cache_key(),
            Object::Number(n) => n.cache_key(),
            Object::String(s) => s.cache_key(),
            Object::Name(n) => n.cache_key(),
            Object::Dict(d) => d.cache_key(),
            Object::Array(a) => a.cache_key(),
            Object::Stream(s) => s.cache_key(),
        }
    }
}

impl CacheKey for ObjRef {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}

impl<T: CacheKey> CacheKey for MaybeRef<T> {
    fn cache_key(&self) -> u128 {
        match self {
            Self::Ref(r) => r.cache_key(),
            Self::NotRef(o) => o.cache_key(),
        }
    }
}

impl CacheKey for Affine {
    fn cache_key(&self) -> u128 {
        let c = self.as_coeffs();
        hash128(&[
            c[0].to_bits(),
            c[1].to_bits(),
            c[2].to_bits(),
            c[3].to_bits(),
            c[4].to_bits(),
            c[5].to_bits(),
        ])
    }
}

impl CacheKey for u128 {
    fn cache_key(&self) -> u128 {
        hash128(self)
    }
}
