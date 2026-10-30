//! PdfCraft patch: bounded, document-owned final image pixels, independent of placement.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use vello_cpu::Pixmap;

/// Retained pixel bytes per render cache (normally one cache per native worker/document).
pub(crate) const MAX_BYTES: usize = 16 << 20;
/// Also bound bookkeeping for documents with many tiny images.
const MAX_ENTRIES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Key {
    pub(crate) image: u128,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) transparency: bool,
}

/// FIFO eviction: hits do not reorder entries. Active draws can hold their own Arc after
/// eviction; this limits retained cache pixels, not the render context or total process RSS.
#[derive(Default)]
pub(crate) struct DerivedImageCache {
    entries: HashMap<Key, Arc<Pixmap>>,
    order: VecDeque<Key>,
    bytes: usize,
    #[cfg(test)]
    hits: std::cell::Cell<usize>,
    #[cfg(test)]
    disabled: bool,
}

impl DerivedImageCache {
    pub(crate) fn get(&self, key: &Key) -> Option<Arc<Pixmap>> {
        #[cfg(test)]
        if self.disabled {
            return None;
        }
        let hit = self.entries.get(key).cloned();
        #[cfg(test)]
        if hit.is_some() {
            self.hits.set(self.hits.get().saturating_add(1));
        }
        hit
    }

    pub(crate) fn insert(&mut self, key: Key, pixels: Arc<Pixmap>) {
        #[cfg(test)]
        if self.disabled {
            return;
        }
        self.insert_with_budget(key, pixels, MAX_BYTES);
    }

    fn insert_with_budget(&mut self, key: Key, pixels: Arc<Pixmap>, budget: usize) {
        let size = pixels.data_as_u8_slice().len();
        if size == 0
            || size > budget
            || key.width != u32::from(pixels.width())
            || key.height != u32::from(pixels.height())
            || self.entries.contains_key(&key)
        {
            return;
        }
        // Reserve fallibly before evicting useful entries. Failure just disables this insert.
        if self.entries.try_reserve(1).is_err() || self.order.try_reserve(1).is_err() {
            return;
        }
        while self.bytes.saturating_add(size) > budget || self.entries.len() >= MAX_ENTRIES {
            let Some(old) = self.order.pop_front() else { return };
            if let Some(old) = self.entries.remove(&old) {
                self.bytes = self.bytes.saturating_sub(old.data_as_u8_slice().len());
            }
        }
        self.bytes = self.bytes.saturating_add(size);
        self.entries.insert(key, pixels);
        self.order.push_back(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hayro_syntax::Pdf;
    use crate::{RenderCache, RenderSettings};
    use hayro_interpret::InterpreterSettings;

    /// Contributor-original image bytes, generated in memory; no fixture/font assets.
    fn document(content: &str, interpolate: bool, extra: &str, variant: u8) -> Pdf {
        let pixels: Vec<u8> = (0_usize..32 * 32).flat_map(|p| [(p % 251) as u8, ((p / 32) % 17 * 13) as u8, variant]).collect();
        let mut bytes = format!("%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 130 70] /Resources << /XObject << /A 5 0 R /B 6 0 R >> /ExtGState << /G << /BM /Multiply /ca .5 >> /H << /SMask << /S /Luminosity /G 7 0 R >> >> >> >> /Contents 4 0 R >> endobj\n4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n", content.len()).into_bytes();
        // Identical dictionaries, different stream locations/data: the cache must distinguish them.
        for (object, blue) in [(5, variant), (6, variant.wrapping_add(97))] {
            let soft_mask = object == 6 && extra == "/SMask 6 0 R";
            let (space, extra, data) = if soft_mask {
                ("DeviceGray", "", pixels.chunks_exact(3).map(|rgb| rgb[0]).collect::<Vec<_>>())
            } else {
                ("DeviceRGB", extra, pixels.chunks_exact(3).flat_map(|rgb| [rgb[0], rgb[1], blue]).collect())
            };
            bytes.extend(format!("{object} 0 obj << /Type /XObject /Subtype /Image /Width 32 /Height 32 /ColorSpace /{space} /BitsPerComponent 8 /Interpolate {interpolate} {extra} /Length {} >> stream\n", data.len()).bytes());
            bytes.extend(data);
            bytes.extend_from_slice(b"\nendstream endobj\n");
        }
        let mask = "0.5 g 0 0 130 70 re f";
        bytes.extend(format!("7 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 130 70] /Group << /S /Transparency /CS /DeviceGray >> /Resources << >> /Length {} >> stream\n{mask}\nendstream endobj\n", mask.len()).bytes());
        bytes.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
        Pdf::new(Arc::new(bytes)).unwrap()
    }

    const REPEATED: &str = "q 8 0 0 8 0 0 cm /A Do Q q 8 0 0 8 12 0 cm /A Do Q";

    #[test]
    fn repeated_draws_and_warm_renders_hit_without_decoding_again() {
        let pdf = document(REPEATED, true, "", 51);
        let cache = RenderCache::new();
        let page = &pdf.pages()[0];
        let first = crate::render(page, &cache, &InterpreterSettings::default(), &RenderSettings::default());
        assert_eq!(cache.derived_images.borrow().hits.get(), 1);
        assert_eq!(cache.derived_images.borrow().entries.len(), 1);
        let warm = crate::render(page, &cache, &InterpreterSettings::default(), &RenderSettings::default());
        assert_eq!(cache.derived_images.borrow().hits.get(), 3);
        assert_eq!(cache.interpreter_cache.decoded_image_decodes(), 1);
        assert_eq!(first.data_as_u8_slice(), warm.data_as_u8_slice());
    }

    #[test]
    fn cached_pixels_match_uncached_across_placement_tiles_and_paint_state() {
        let content = "0.7 g 0 0 130 70 re f q 8.3 0 0 8.7 .35 .4 cm /A Do Q q 8.3 0 0 8.7 16.35 .4 cm /A Do Q q 5 1 -2 7 35 2 cm /A Do Q q 5 1 -2 7 45 2 cm /A Do Q q 12 0 0 11 60 0 cm /B Do Q q /G gs 8.3 0 0 8.7 75 .4 cm /A Do Q q /G gs 8.3 0 0 8.7 90 .4 cm /A Do Q q /H gs 8.3 0 0 8.7 105 30 cm /A Do Q q /H gs 8.3 0 0 8.7 115 30 cm /A Do Q";
        for interpolate in [false, true] {
            for extra in ["", "/Decode [1 0 0 1 1 0]"] {
                let pdf = document(content, interpolate, extra, 41);
                let cached = RenderCache::new();
                let reference = RenderCache::new();
                reference.derived_images.borrow_mut().disabled = true;
                for preview in [false, true, false] {
                    for scale in [1.0, 1.7, 0.75] {
                        for offset in [0.0, 3.25] {
                            let settings = RenderSettings {
                                x_scale: scale,
                                y_scale: scale,
                                x_offset: offset,
                                y_offset: offset,
                                width: Some(130),
                                height: Some(70),
                                ..Default::default()
                            };
                            let interpreter = InterpreterSettings { preview, ..Default::default() };
                            let actual = crate::render(&pdf.pages()[0], &cached, &interpreter, &settings);
                            let expected = crate::render(&pdf.pages()[0], &reference, &interpreter, &settings);
                            assert_eq!(
                                actual.data_as_u8_slice(),
                                expected.data_as_u8_slice(),
                                "interpolate={interpolate}, preview={preview}, scale={scale}, offset={offset}, extra={extra}"
                            );
                        }
                    }
                }
                assert!(cached.derived_images.borrow().hits.get() > 0);
                assert!(cached.derived_images.borrow().entries.len() > 2);
            }
        }
    }

    #[test]
    fn document_replacement_isolated_and_fresh_cache_invalidates() {
        let first = document(REPEATED, false, "", 15);
        let changed = document(REPEATED, false, "", 215);
        let cache = RenderCache::new();
        let settings = RenderSettings::default();
        let interpreter = InterpreterSettings::default();
        let before = crate::render(&first.pages()[0], &cache, &interpreter, &settings);
        let after = crate::render(&changed.pages()[0], &cache, &interpreter, &settings);
        assert_ne!(before.data_as_u8_slice(), after.data_as_u8_slice());
        assert_eq!(cache.derived_images.borrow().entries.len(), 2);
        let fresh = RenderCache::new();
        assert!(fresh.derived_images.borrow().entries.is_empty());
        let reopened = crate::render(&changed.pages()[0], &fresh, &interpreter, &settings);
        assert_eq!(after.data_as_u8_slice(), reopened.data_as_u8_slice());
    }

    #[test]
    fn masked_images_bypass_and_cancelled_pages_do_not_use_warm_entries() {
        for extra in ["/Mask [0 0 0 0 0 0]", "/SMask 6 0 R"] {
            let masked = document(REPEATED, false, extra, 51);
            let cache = RenderCache::new();
            let reference = RenderCache::new();
            reference.derived_images.borrow_mut().disabled = true;
            let actual = crate::render(&masked.pages()[0], &cache, &InterpreterSettings::default(), &RenderSettings::default());
            let expected = crate::render(&masked.pages()[0], &reference, &InterpreterSettings::default(), &RenderSettings::default());
            assert_eq!(actual.data_as_u8_slice(), expected.data_as_u8_slice());
            assert!(cache.derived_images.borrow().entries.is_empty());
        }
        let pdf = document(REPEATED, true, "", 51);
        let cache = RenderCache::new();
        crate::render(&pdf.pages()[0], &cache, &InterpreterSettings::default(), &RenderSettings::default());
        let hits = cache.derived_images.borrow().hits.get();
        let interpreter = InterpreterSettings { cancelled: Some(Arc::new(std::sync::atomic::AtomicBool::new(true))), ..Default::default() };
        let cancelled = crate::render(&pdf.pages()[0], &cache, &interpreter, &RenderSettings::default());
        let reference = crate::render(&pdf.pages()[0], &RenderCache::new(), &interpreter, &RenderSettings::default());
        assert_eq!(cancelled.data_as_u8_slice(), reference.data_as_u8_slice());
        assert_eq!(cache.derived_images.borrow().hits.get(), hits);
    }

    fn key(image: u128) -> Key {
        Key { image, width: 2, height: 2, transparency: false }
    }

    #[test]
    fn hits_share_pixels_and_fifo_eviction_obeys_bytes() {
        let mut cache = DerivedImageCache::default();
        let pixels = Arc::new(Pixmap::new(2, 2));
        cache.insert_with_budget(key(1), pixels.clone(), 32);
        assert!(Arc::ptr_eq(&pixels, &cache.get(&key(1)).unwrap()));
        cache.insert_with_budget(key(2), pixels.clone(), 32);
        assert_eq!(cache.bytes, 32);
        cache.get(&key(1)); // A hit does not change FIFO order.
        cache.insert_with_budget(key(3), pixels, 32);
        assert!(cache.get(&key(1)).is_none());
        assert!(cache.get(&key(2)).is_some());
        assert!(cache.get(&key(3)).is_some());
        assert_eq!(cache.bytes, 32);
    }

    #[test]
    fn oversized_entries_do_not_evict_and_tiny_entries_are_bounded() {
        let mut cache = DerivedImageCache::default();
        cache.insert_with_budget(key(1), Arc::new(Pixmap::new(2, 2)), 16);
        cache.insert_with_budget(Key { width: 3, height: 3, ..key(2) }, Arc::new(Pixmap::new(3, 3)), 16);
        assert_eq!(cache.bytes, 16);
        assert!(cache.get(&key(1)).is_some());
        for image in 2..1026 {
            cache.insert(Key { width: 1, height: 1, ..key(image) }, Arc::new(Pixmap::new(1, 1)));
            assert!(cache.bytes <= MAX_BYTES);
            assert!(cache.entries.len() <= MAX_ENTRIES);
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert_eq!(cache.order.len(), MAX_ENTRIES);
    }

    #[test]
    fn production_byte_limit_is_exact_and_an_oversized_result_is_not_retained() {
        let mut cache = DerivedImageCache::default();
        let full = Key { width: 2048, height: 2048, ..key(1) };
        cache.insert(full, Arc::new(Pixmap::new(2048, 2048)));
        assert_eq!(cache.bytes, MAX_BYTES);
        let too_big = Key { width: 2049, height: 2048, ..key(2) };
        cache.insert(too_big, Arc::new(Pixmap::new(2049, 2048)));
        assert_eq!(cache.bytes, MAX_BYTES);
        assert!(cache.get(&full).is_some());
        assert!(cache.get(&too_big).is_none());
        cache.insert(key(3), Arc::new(Pixmap::new(2, 2)));
        assert_eq!(cache.bytes, 16);
        assert!(cache.get(&full).is_none());
    }
}
