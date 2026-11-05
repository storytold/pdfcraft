//! One compressed binary text cache per PDF. Page layers are independent sections, so adding
//! or replacing a page appends only that page and a small index snapshot.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use sha2::{Digest, Sha256};

use crate::{PageText, RenderConfig, TextGlyph, text::normalized_search_text};

const FILE_MAGIC: &[u8; 8] = b"PCFTEXT\0";
const INDEX_MAGIC: &[u8; 8] = b"PCFIDX\0\x02";
const GRAMS_MAGIC: &[u8; 8] = b"PCFGRAM\x01";
const HEADER_LEN: u64 = 8 + 2 * 32;
const SLOT_LEN: usize = 32;
const MAX_CACHE_BYTES: usize = 512 * 1024 * 1024;
const MAX_PAGES: usize = 1_000_000;
const MAX_GLYPHS: usize = 20_000_000;
const MAX_PAGE_GRAMS: usize = 1_000_000;
const MAX_TOTAL_GRAM_POSTINGS: usize = 2_000_000;

#[derive(Clone, Copy)]
struct Section {
    offset: u64,
    len: u64,
}

#[derive(Clone, Copy)]
struct PageSections {
    text: Section,
    grams: Option<Section>,
}

#[derive(Default)]
struct PageIndex {
    generation: u64,
    slot: Option<usize>,
    pages: BTreeMap<usize, PageSections>,
    page_grams: HashMap<usize, Vec<[u8; 3]>>,
    postings: HashMap<[u8; 3], BTreeSet<usize>>,
    gram_posting_count: usize,
}

pub(crate) struct TextDiskCache {
    path: PathBuf,
    index: Mutex<PageIndex>,
}

impl TextDiskCache {
    pub(crate) fn new(bytes: &[u8], config: &RenderConfig) -> Option<Self> {
        let base = cache_directory()?;
        let mut hash = Sha256::new();
        hash.update(b"pdfcraft-text-cache-v2\0");
        hash.update(env!("CARGO_PKG_VERSION").as_bytes());
        hash.update(bytes);
        hash.update([u8::from(config.hide_comments)]);
        if let Some(password) = &config.password {
            hash.update((password.len() as u64).to_le_bytes());
            hash.update(password.as_bytes());
        } else {
            hash.update(0u64.to_le_bytes());
        }
        hash.update((config.layers.len() as u64).to_le_bytes());
        for (object, generation, visible) in config.layers.iter() {
            hash.update(object.to_le_bytes());
            hash.update(generation.to_le_bytes());
            hash.update([u8::from(*visible)]);
        }
        let path = base.join(format!("{}.bin", hex(&hash.finalize())));
        let index = read_index(&path).unwrap_or_default();
        Some(Self { path, index: Mutex::new(index) })
    }

    #[cfg(test)]
    fn from_path(path: PathBuf) -> Self {
        let index = read_index(&path).unwrap_or_default();
        Self { path, index: Mutex::new(index) }
    }

    pub(crate) fn load(&self, page: usize) -> Option<Arc<PageText>> {
        let section = lock(&self.index).pages.get(&page).map(|sections| sections.text)?;
        if section.len > MAX_CACHE_BYTES as u64 {
            return None;
        }
        let mut file = File::open(&self.path).ok()?;
        file.seek(SeekFrom::Start(section.offset)).ok()?;
        let mut compressed = vec![0; usize::try_from(section.len).ok()?];
        file.read_exact(&mut compressed).ok()?;
        let decoder = ZlibDecoder::new(compressed.as_slice());
        let mut raw = Vec::with_capacity(compressed.len().saturating_mul(2).min(MAX_CACHE_BYTES));
        decoder.take(MAX_CACHE_BYTES as u64 + 1).read_to_end(&mut raw).ok()?;
        if raw.len() > MAX_CACHE_BYTES {
            return None;
        }
        decode(&raw).map(Arc::new)
    }

    pub(crate) fn candidate_pages(&self, query: &str, page_count: usize) -> Option<Vec<usize>> {
        let query = crate::text::fold_case(query).split_whitespace().collect::<Vec<_>>().join(" ");
        if query.len() < 3 || page_count == 0 {
            return None;
        }
        let grams: HashSet<[u8; 3]> = query.as_bytes().windows(3).filter_map(|bytes| <[u8; 3]>::try_from(bytes).ok()).collect();
        let index = lock(&self.index);
        if index.pages.len() != page_count
            || index.page_grams.len() != page_count
            || !(0..page_count).all(|page| index.page_grams.contains_key(&page))
        {
            return None;
        }
        let Some((_, rarest)) =
            grams.iter().filter_map(|gram| index.postings.get(gram).map(|pages| (gram, pages))).min_by_key(|(_, pages)| pages.len())
        else {
            return Some(Vec::new());
        };
        Some(
            rarest
                .iter()
                .copied()
                .filter(|page| grams.iter().all(|gram| index.postings.get(gram).is_some_and(|pages| pages.contains(page))) && *page < page_count)
                .collect(),
        )
    }

    pub(crate) fn store(&self, page: usize, text: &PageText) {
        let Some(raw) = encode(text) else { return };
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        if encoder.write_all(&raw).is_err() {
            return;
        }
        let Ok(compressed) = encoder.finish() else { return };
        if compressed.len() > MAX_CACHE_BYTES || page > u32::MAX as usize {
            return;
        }
        let grams = unique_grams(text);
        let compressed_grams = grams.as_deref().and_then(compress_grams);

        let mut index = lock(&self.index);
        let Some(parent) = self.path.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(mut file) = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&self.path) else { return };
        if file.lock().is_err() {
            return;
        }
        let disk_generation = read_generation(&mut file).unwrap_or_default();
        if disk_generation > index.generation {
            *index = read_index_from(&mut file).unwrap_or_default();
        }
        if !ensure_header(&mut file) {
            return;
        }
        let Ok(section_offset) = file.seek(SeekFrom::End(0)) else { return };
        if file.write_all(&compressed).is_err() {
            return;
        }
        let grams_section = if let Some(compressed_grams) = &compressed_grams {
            let Ok(offset) = file.seek(SeekFrom::End(0)) else { return };
            if file.write_all(compressed_grams).is_err() {
                return;
            }
            Some(Section { offset, len: compressed_grams.len() as u64 })
        } else {
            None
        };
        let mut pages = index.pages.clone();
        pages.insert(page, PageSections { text: Section { offset: section_offset, len: compressed.len() as u64 }, grams: grams_section });
        let Some(index_bytes) = encode_index(&pages) else { return };
        let Ok(index_offset) = file.seek(SeekFrom::End(0)) else { return };
        if file.write_all(&index_bytes).is_err() || file.flush().is_err() {
            return;
        }
        let generation = index.generation.saturating_add(1).max(1);
        let slot = index.slot.map_or(0, |active| 1 - active);
        let slot_bytes = encode_slot(generation, index_offset, index_bytes.len() as u64, &index_bytes);
        if file.seek(SeekFrom::Start(8 + (slot * SLOT_LEN) as u64)).is_err() || file.write_all(&slot_bytes).is_err() || file.flush().is_err() {
            return;
        }
        index.generation = generation;
        index.slot = Some(slot);
        index.pages = pages;
        remove_page_grams(&mut index, page);
        if grams_section.is_some()
            && let Some(grams) = grams
            && index.gram_posting_count.saturating_add(grams.len()) <= MAX_TOTAL_GRAM_POSTINGS
        {
            for gram in &grams {
                index.postings.entry(*gram).or_default().insert(page);
            }
            index.gram_posting_count = index.gram_posting_count.saturating_add(grams.len());
            index.page_grams.insert(page, grams);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_index(path: &PathBuf) -> Option<PageIndex> {
    let mut file = File::open(path).ok()?;
    read_index_from(&mut file)
}

fn read_index_from(file: &mut File) -> Option<PageIndex> {
    let file_len = file.metadata().ok()?.len();
    if file_len < HEADER_LEN {
        return None;
    }
    let mut header = [0u8; HEADER_LEN as usize];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut header).ok()?;
    if header.get(..8)? != FILE_MAGIC {
        return None;
    }
    let mut best: Option<PageIndex> = None;
    for slot in 0..2 {
        let start = 8 + slot * SLOT_LEN;
        let data = header.get(start..start + SLOT_LEN)?;
        let generation = u64_at(data, 0)?;
        let offset = u64_at(data, 8)?;
        let len = u64_at(data, 16)?;
        let checksum = u64_at(data, 24)?;
        let Some(index_end) = offset.checked_add(len) else { continue };
        if generation == 0 || len > MAX_CACHE_BYTES as u64 || offset < HEADER_LEN || index_end > file_len {
            continue;
        }
        let mut bytes = vec![0; usize::try_from(len).ok()?];
        if file.seek(SeekFrom::Start(offset)).is_err() || file.read_exact(&mut bytes).is_err() || checksum != index_checksum(&bytes) {
            continue;
        }
        let Some(pages) = decode_index(&bytes) else { continue };
        if pages.values().any(|sections| {
            sections.text.offset < HEADER_LEN
                || sections.text.offset.checked_add(sections.text.len).is_none_or(|end| end > offset)
                || sections.grams.is_some_and(|grams| grams.offset < HEADER_LEN || grams.offset.checked_add(grams.len).is_none_or(|end| end > offset))
        }) {
            continue;
        }
        if best.as_ref().is_none_or(|current| generation > current.generation) {
            best = Some(PageIndex { generation, slot: Some(slot), pages, ..PageIndex::default() });
        }
    }
    let mut best = best?;
    load_page_grams(file, &mut best);
    Some(best)
}

fn read_generation(file: &mut File) -> Option<u64> {
    let file_len = file.metadata().ok()?.len();
    if file_len < HEADER_LEN {
        return None;
    }
    let mut header = [0u8; HEADER_LEN as usize];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut header).ok()?;
    if header.get(..8)? != FILE_MAGIC {
        return None;
    }
    (0..2)
        .filter_map(|slot| {
            let start = 8 + slot * SLOT_LEN;
            let data = header.get(start..start + SLOT_LEN)?;
            let generation = u64_at(data, 0)?;
            let offset = u64_at(data, 8)?;
            let len = u64_at(data, 16)?;
            let end = offset.checked_add(len)?;
            (generation > 0 && len <= MAX_CACHE_BYTES as u64 && offset >= HEADER_LEN && end <= file_len).then_some(generation)
        })
        .max()
}

fn load_page_grams(file: &mut File, index: &mut PageIndex) {
    let mut postings = HashMap::<[u8; 3], BTreeSet<usize>>::new();
    let mut page_grams = HashMap::with_capacity(index.pages.len());
    let mut total = 0usize;
    for (&page, sections) in &index.pages {
        let Some(section) = sections.grams else { continue };
        let Some(grams) = read_grams(file, section) else { continue };
        total = total.saturating_add(grams.len());
        if total > MAX_TOTAL_GRAM_POSTINGS {
            return;
        }
        for gram in &grams {
            postings.entry(*gram).or_default().insert(page);
        }
        page_grams.insert(page, grams);
    }
    index.page_grams = page_grams;
    index.postings = postings;
    index.gram_posting_count = total;
}

fn read_grams(file: &mut File, section: Section) -> Option<Vec<[u8; 3]>> {
    if section.len == 0 || section.len > MAX_CACHE_BYTES as u64 {
        return None;
    }
    let mut compressed = vec![0; usize::try_from(section.len).ok()?];
    file.seek(SeekFrom::Start(section.offset)).ok()?;
    file.read_exact(&mut compressed).ok()?;
    let mut raw = Vec::new();
    ZlibDecoder::new(compressed.as_slice()).take(MAX_CACHE_BYTES as u64 + 1).read_to_end(&mut raw).ok()?;
    if raw.len() > MAX_CACHE_BYTES {
        return None;
    }
    decode_grams(&raw)
}

fn remove_page_grams(index: &mut PageIndex, page: usize) {
    if let Some(previous) = index.page_grams.remove(&page) {
        index.gram_posting_count = index.gram_posting_count.saturating_sub(previous.len());
        for gram in previous {
            let remove = if let Some(pages) = index.postings.get_mut(&gram) {
                pages.remove(&page);
                pages.is_empty()
            } else {
                false
            };
            if remove {
                index.postings.remove(&gram);
            }
        }
    }
}

fn ensure_header(file: &mut File) -> bool {
    let mut magic = [0; 8];
    let valid = file.seek(SeekFrom::Start(0)).is_ok() && file.read_exact(&mut magic).is_ok() && &magic == FILE_MAGIC;
    if valid && file.metadata().is_ok_and(|metadata| metadata.len() >= HEADER_LEN) {
        return true;
    }
    if file.set_len(0).is_err() || file.seek(SeekFrom::Start(0)).is_err() || file.write_all(FILE_MAGIC).is_err() {
        return false;
    }
    file.write_all(&[0; (2 * SLOT_LEN)]).is_ok() && file.flush().is_ok()
}

fn encode_index(pages: &BTreeMap<usize, PageSections>) -> Option<Vec<u8>> {
    if pages.len() > MAX_PAGES {
        return None;
    }
    let mut out = Vec::with_capacity(12 + pages.len() * 36);
    out.extend_from_slice(INDEX_MAGIC);
    put_u32(&mut out, u32::try_from(pages.len()).ok()?);
    for (&page, sections) in pages {
        put_u32(&mut out, u32::try_from(page).ok()?);
        put_u64(&mut out, sections.text.offset);
        put_u64(&mut out, sections.text.len);
        if let Some(grams) = sections.grams {
            put_u64(&mut out, grams.offset);
            put_u64(&mut out, grams.len);
        } else {
            put_u64(&mut out, 0);
            put_u64(&mut out, 0);
        }
    }
    Some(out)
}

fn decode_index(bytes: &[u8]) -> Option<BTreeMap<usize, PageSections>> {
    if bytes.get(..8)? != INDEX_MAGIC {
        return None;
    }
    let mut offset = 8;
    let count = read_u32(bytes, &mut offset)? as usize;
    if count > MAX_PAGES || count.checked_mul(36)?.checked_add(12)? != bytes.len() {
        return None;
    }
    let mut pages = BTreeMap::new();
    for _ in 0..count {
        let page = read_u32(bytes, &mut offset)? as usize;
        let text = Section { offset: read_u64(bytes, &mut offset)?, len: read_u64(bytes, &mut offset)? };
        let grams_offset = read_u64(bytes, &mut offset)?;
        let grams_len = read_u64(bytes, &mut offset)?;
        let grams = if grams_len == 0 { None } else { Some(Section { offset: grams_offset, len: grams_len }) };
        if pages.insert(page, PageSections { text, grams }).is_some() {
            return None;
        }
    }
    Some(pages)
}

fn unique_grams(text: &PageText) -> Option<Vec<[u8; 3]>> {
    let normalized = normalized_search_text(text);
    let mut grams = HashSet::new();
    for bytes in normalized.as_bytes().windows(3) {
        grams.insert(<[u8; 3]>::try_from(bytes).ok()?);
        if grams.len() > MAX_PAGE_GRAMS {
            return None;
        }
    }
    let mut grams: Vec<_> = grams.into_iter().collect();
    grams.sort_unstable();
    Some(grams)
}

fn compress_grams(grams: &[[u8; 3]]) -> Option<Vec<u8>> {
    if grams.len() > MAX_PAGE_GRAMS {
        return None;
    }
    let raw_len = grams.len().checked_mul(3)?.checked_add(12)?;
    if raw_len > MAX_CACHE_BYTES {
        return None;
    }
    let mut raw = Vec::with_capacity(raw_len);
    raw.extend_from_slice(GRAMS_MAGIC);
    put_u32(&mut raw, u32::try_from(grams.len()).ok()?);
    for gram in grams {
        raw.extend_from_slice(gram);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&raw).ok()?;
    encoder.finish().ok()
}

fn decode_grams(bytes: &[u8]) -> Option<Vec<[u8; 3]>> {
    if bytes.get(..8)? != GRAMS_MAGIC {
        return None;
    }
    let mut offset = 8;
    let count = read_u32(bytes, &mut offset)? as usize;
    if count > MAX_PAGE_GRAMS || count.checked_mul(3)?.checked_add(12)? != bytes.len() {
        return None;
    }
    let mut grams = Vec::with_capacity(count);
    for _ in 0..count {
        grams.push(take(bytes, &mut offset, 3)?.try_into().ok()?);
    }
    Some(grams)
}

fn encode_slot(generation: u64, offset: u64, len: u64, index: &[u8]) -> [u8; SLOT_LEN] {
    let mut out = [0; SLOT_LEN];
    out[0..8].copy_from_slice(&generation.to_le_bytes());
    out[8..16].copy_from_slice(&offset.to_le_bytes());
    out[16..24].copy_from_slice(&len.to_le_bytes());
    out[24..32].copy_from_slice(&index_checksum(index).to_le_bytes());
    out
}

fn index_checksum(bytes: &[u8]) -> u64 {
    let digest = Sha256::digest(bytes);
    u64::from_le_bytes(digest[..8].try_into().unwrap_or_default())
}

fn encode(text: &PageText) -> Option<Vec<u8>> {
    let count = text.glyphs.len();
    if count > MAX_GLYPHS || text.line_of.len() != count || text.space_before.len() != count {
        return None;
    }
    let mut out = Vec::with_capacity(count.saturating_mul(48).saturating_add(12).min(MAX_CACHE_BYTES));
    out.extend_from_slice(b"PCFPAGE\0");
    put_u32(&mut out, u32::try_from(count).ok()?);
    for (i, glyph) in text.glyphs.iter().enumerate() {
        put_u32(&mut out, u32::try_from(glyph.text.len()).ok()?);
        out.extend_from_slice(glyph.text.as_bytes());
        for value in glyph.rect.into_iter().chain(glyph.quad.into_iter().flatten()).chain(glyph.direction) {
            put_f32(&mut out, value);
        }
        put_u32(&mut out, text.line_of[i]);
        out.push(u8::from(text.space_before[i]));
    }
    (out.len() <= MAX_CACHE_BYTES).then_some(out)
}

fn decode(bytes: &[u8]) -> Option<PageText> {
    if bytes.get(..8)? != b"PCFPAGE\0" {
        return None;
    }
    let mut offset = 8;
    let count = read_u32(bytes, &mut offset)? as usize;
    if count > MAX_GLYPHS || count.saturating_mul(48) > bytes.len() {
        return None;
    }
    let mut glyphs = Vec::with_capacity(count);
    let mut line_of = Vec::with_capacity(count);
    let mut space_before = Vec::with_capacity(count);
    for _ in 0..count {
        let len = read_u32(bytes, &mut offset)? as usize;
        let text = String::from_utf8(take(bytes, &mut offset, len)?.to_vec()).ok()?;
        let mut floats = [0.0; 14];
        for value in &mut floats {
            *value = read_f32(bytes, &mut offset)?;
        }
        let rect = [floats[0], floats[1], floats[2], floats[3]];
        let quad = [[floats[4], floats[5]], [floats[6], floats[7]], [floats[8], floats[9]], [floats[10], floats[11]]];
        let direction = [floats[12], floats[13]];
        let line = read_u32(bytes, &mut offset)?;
        let space = *take(bytes, &mut offset, 1)?.first()?;
        if space > 1 {
            return None;
        }
        glyphs.push(TextGlyph { text, rect, quad, direction });
        line_of.push(line);
        space_before.push(space != 0);
    }
    (offset == bytes.len()).then_some(PageText { glyphs, line_of, space_before })
}

fn take<'a>(bytes: &'a [u8], offset: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = offset.checked_add(len)?;
    let part = bytes.get(*offset..end)?;
    *offset = end;
    Some(part)
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    Some(u32::from_le_bytes(take(bytes, offset, 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    Some(u64::from_le_bytes(take(bytes, offset, 8)?.try_into().ok()?))
}

fn read_f32(bytes: &[u8], offset: &mut usize) -> Option<f32> {
    Some(f32::from_le_bytes(take(bytes, offset, 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(offset..offset + 8)?.try_into().ok()?))
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn cache_directory() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("PdfCraft/text-v2"))
    }
    #[cfg(target_os = "macos")]
    {
        Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Caches/PdfCraft/text-v2"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        Some(base.join("PdfCraft/text-v2"))
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PageText {
        PageText {
            glyphs: vec![TextGlyph {
                text: "βeta".into(),
                rect: [1.0, 2.0, 3.0, 4.0],
                quad: [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]],
                direction: [1.0, 0.0],
            }],
            line_of: vec![7],
            space_before: vec![true],
        }
    }

    #[test]
    fn a_single_bin_retains_and_replaces_individual_page_sections() {
        let path = std::env::temp_dir().join(format!("pdfcraft-text-cache-test-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let cache = TextDiskCache::from_path(path.clone());
        let first = sample();
        let second = PageText::default();
        cache.store(0, &first);
        cache.store(1, &second);
        assert_eq!(cache.load(0).as_deref(), Some(&first));
        assert_eq!(cache.load(1).as_deref(), Some(&second));
        assert_eq!(cache.candidate_pages("eta", 2), Some(vec![0]));
        assert_eq!(cache.candidate_pages("ΒET", 2), Some(vec![0]), "index and search share Unicode case folding");
        assert_eq!(cache.candidate_pages("et", 2), None, "short queries use the exact SIMD scan");
        let reopened = TextDiskCache::from_path(path.clone());
        assert_eq!(reopened.candidate_pages("eta", 2), Some(vec![0]), "page grams survive process restart");
        let replacement = PageText { glyphs: Vec::new(), line_of: Vec::new(), space_before: Vec::new() };
        cache.store(0, &replacement);
        assert_eq!(cache.load(0).as_deref(), Some(&replacement));
        assert_eq!(cache.candidate_pages("eta", 2), Some(Vec::new()), "replaced page grams are removed");
        let reopened = TextDiskCache::from_path(path.clone());
        assert_eq!(reopened.load(0).as_deref(), Some(&replacement));
        assert_eq!(reopened.load(1).as_deref(), Some(&second));
        assert_eq!(reopened.candidate_pages("eta", 2), Some(Vec::new()), "page replacement survives process restart");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn separate_cache_writers_merge_page_sections_before_appending() {
        let path = std::env::temp_dir().join(format!("pdfcraft-text-cache-writers-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let first_writer = TextDiskCache::from_path(path.clone());
        let second_writer = TextDiskCache::from_path(path.clone());
        let first = sample();
        let second = PageText::default();
        first_writer.store(0, &first);
        second_writer.store(1, &second);
        first_writer.store(2, &first);
        let reopened = TextDiskCache::from_path(path.clone());
        assert_eq!(reopened.load(0).as_deref(), Some(&first));
        assert_eq!(reopened.load(1).as_deref(), Some(&second));
        assert_eq!(reopened.load(2).as_deref(), Some(&first));
        assert_eq!(reopened.candidate_pages("eta", 3), Some(vec![0, 2]));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cache_key_changes_with_pdf_bytes_and_text_settings() {
        let config = RenderConfig::default();
        let Some(original) = TextDiskCache::new(b"one", &config) else { return };
        let changed_pdf = TextDiskCache::new(b"two", &config).unwrap();
        let changed_settings = TextDiskCache::new(b"one", &RenderConfig { hide_comments: true, ..config }).unwrap();
        assert_ne!(original.path, changed_pdf.path);
        assert_ne!(original.path, changed_settings.path);
    }
}
