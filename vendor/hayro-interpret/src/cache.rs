use crate::util::hash128;
use hayro_syntax::object::{Array, Dict, MaybeRef, Name, Null, ObjRef, Object, Stream};
use kurbo::Affine;
use std::any::Any;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

type CacheMap = HashMap<u128, Option<Box<dyn Any + Send + Sync>>>;
/// The objects read from a document, by key. PdfCraft patch (48): the second field counts the
/// ICC profiles read through it ([`Cache::icc_profile_reads`]).
#[derive(Clone)]
pub(crate) struct Cache(Arc<Mutex<CacheMap>>, Arc<AtomicUsize>);
impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

impl Cache {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())), Arc::default())
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
}

#[derive(Default)]
struct DecodedImageCacheInner {
    map: HashMap<DecodedImageKey, Option<Box<dyn Any + Send + Sync>>>,
    bytes: usize,
    /// PdfCraft patch (32): how many keyed decodes ran (cache misses).
    decodes: usize,
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

    /// The same cache, for a preview (`true`) or a final render.
    pub(crate) fn for_render(&self, preview: bool) -> Self {
        Self {
            inner: self.inner.clone(),
            preview,
        }
    }

    /// The decoded image for `key`, from the cache or from `f`. `None` as the key decodes
    /// without the cache.
    pub(crate) fn get_or_insert_with<T: Clone + Send + Sync + 'static>(
        &self,
        key: Option<DecodedImageKey>,
        budget: usize,
        size_of: impl FnOnce(&T) -> usize,
        f: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let Some(key) = key else { return f() };
        // Fast path: a hit clones the pixels without decoding.
        let hit = match self.inner.lock() {
            Ok(locked) => locked
                .map
                .get(&key)
                .and_then(|val| val.as_ref())
                .and_then(|val| val.downcast_ref::<T>().cloned()),
            // A thread panicked while holding the lock: decode without the cache.
            Err(_) => return f(),
        };
        if let Some(hit) = hit {
            return Some(hit);
        }
        // Miss: decode without holding the lock (decoding must never deadlock the cache).
        let val = f();
        let Ok(mut locked) = self.inner.lock() else {
            return val;
        };
        locked.decodes = locked.decodes.saturating_add(1);
        // Another thread may have decoded it meanwhile: keep the first copy.
        if let Some(existing) = locked.map.get(&key)
            && let Some(existing) = existing.as_ref().and_then(|val| val.downcast_ref::<T>())
        {
            return Some(existing.clone());
        }
        let size = match &val {
            Some(v) => size_of(v),
            None => 0,
        };
        if size > budget {
            return val;
        }
        if locked.bytes.saturating_add(size) > budget {
            locked.map.clear();
            locked.bytes = 0;
        }
        locked.bytes = locked.bytes.saturating_add(size);
        locked.map.insert(
            key,
            val.clone()
                .map(|val| Box::new(val) as Box<dyn Any + Send + Sync>),
        );
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
