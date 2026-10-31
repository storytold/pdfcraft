//! A PDF kept on disk: its structure held in memory as one buffer, its large stream bodies read
//! from the file when something wants them. A file of gigabytes is that large because of those
//! bodies, so the structure is a small part of it (`core.lazy-loading`).
//!
//! The held buffer is the file with the bodies cut out, every other byte unchanged and in order.
//! The parsers read it as they read any file: they take a file offset through
//! [`OnDiskFile::held_offset`], and when they reach the start of a body that was cut out,
//! [`OnDiskFile::body_at`] says which one it is.
//!
//! A region is cut only when it is proven to be a stream body. Anything else stays held, so what
//! the walk doesn't understand costs memory and never changes what the parsers read.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::object::Object;
use crate::parser::{Lexer, is_regular, is_whitespace};

/// Stream bodies shorter than this stay held: reading them back would cost more than they do.
const MIN_BODY: u64 = 4096;
/// The most a walk holds, whatever the size of the file: past this it isn't a large file of
/// images, and reading it whole is its reader's decision.
const MAX_HELD: u64 = 1 << 30;
/// How much of the file a walk reads at a time. Most of it is skipped, so this stays small.
const WINDOW: usize = 64 * 1024;
/// An object larger than this stays held without being looked at.
const MAX_WINDOW: usize = 16 * 1024 * 1024;
/// An object that ends this close to the end of a window may have been cut short by it.
const MARGIN: usize = 64;
/// More integer objects than any real file has: the walk keeps them all, so this bounds it.
const MAX_INTEGERS: usize = 1 << 22;
/// How much of the file a search for `endstream` reads at a time.
const SEARCH_CHUNK: usize = 1024 * 1024;

/// Where a file's bytes come from.
pub trait ByteSource: Send + Sync {
    /// The size of the file.
    fn size(&self) -> u64;
    /// Fill `out` from `offset`; anything short of that is an error.
    fn read_at(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()>;
}

/// A stream body left on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskBody {
    /// Where it starts in the file.
    pub start: u64,
    pub len: u64,
}

/// A body cut out of the held buffer, and where it was cut.
#[derive(Clone, Copy, Debug)]
struct Cut {
    body: DiskBody,
    held: usize,
}

/// A PDF whose large stream bodies stay on disk. See the module documentation.
pub struct OnDiskFile {
    held: Arc<Vec<u8>>,
    cuts: Vec<Cut>,
    source: Arc<dyn ByteSource>,
    size: u64,
    failed_reads: AtomicU64,
}

impl std::fmt::Debug for OnDiskFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnDiskFile").field("size", &self.size).field("held", &self.held.len()).field("bodies", &self.cuts.len()).finish()
    }
}

impl OnDiskFile {
    /// Walk `source` and keep its structure. `None` when the file can't stay on disk: it holds
    /// little that can be left there, or a walk would read far more than the file (a file made to
    /// look like endless objects). Reading such a file whole is always right.
    pub fn read(source: Arc<dyn ByteSource>) -> std::io::Result<Option<Self>> {
        let size = source.size();
        let mut walk = Walk::new(source.as_ref(), size);
        if !walk.run()? {
            return Ok(None);
        }
        let Walk { held, cuts, .. } = walk;
        Ok(Some(Self { held: Arc::new(held), cuts, source, size, failed_reads: AtomicU64::new(0) }))
    }

    /// The bytes kept in memory. They are not the file: never write them out.
    pub fn held(&self) -> &Arc<Vec<u8>> {
        &self.held
    }

    /// The size of the file.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// How many stream bodies stayed on disk.
    pub fn body_count(&self) -> usize {
        self.cuts.len()
    }

    /// Where the byte at file offset `offset` is held. `None` for a byte of a body on disk: no
    /// object starts there in a file that reads as it should.
    pub fn held_offset(&self, offset: u64) -> Option<usize> {
        let after = self.cuts.partition_point(|c| c.body.start <= offset);
        let Some(cut) = after.checked_sub(1).and_then(|i| self.cuts.get(i)) else { return usize::try_from(offset).ok() };
        let past = offset.checked_sub(cut.body.start)?.checked_sub(cut.body.len)?;
        cut.held.checked_add(usize::try_from(past).ok()?)
    }

    /// The body cut out at position `held` of the held buffer.
    pub fn body_at(&self, held: usize) -> Option<DiskBody> {
        let i = self.cuts.partition_point(|c| c.held < held);
        self.cuts.get(i).filter(|c| c.held == held).map(|c| c.body)
    }

    /// Read a body. `None` when the file can no longer give it, and [`Self::failed_reads`] counts
    /// the failure: the caller's callers usually can't report one.
    pub fn read_body(&self, body: DiskBody) -> Option<Vec<u8>> {
        let bytes = self.read_range(body).ok();
        if bytes.is_none() {
            self.failed_reads.fetch_add(1, Ordering::Relaxed);
        }
        bytes
    }

    fn read_range(&self, body: DiskBody) -> std::io::Result<Vec<u8>> {
        let len = usize::try_from(body.len).map_err(|_| std::io::ErrorKind::OutOfMemory)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| std::io::ErrorKind::OutOfMemory)?;
        bytes.resize(len, 0);
        self.source.read_at(body.start, &mut bytes)?;
        Ok(bytes)
    }

    /// How many reads of a body have failed so far.
    pub fn failed_reads(&self) -> u64 {
        self.failed_reads.load(Ordering::Relaxed)
    }

    /// Write the whole file to `out`, a piece at a time.
    pub fn copy_to(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        let mut piece = vec![0; SEARCH_CHUNK];
        let mut at = 0;
        while at < self.size {
            let n = piece.len().min(usize::try_from(self.size - at).unwrap_or(usize::MAX));
            let piece = piece.get_mut(..n).ok_or(std::io::ErrorKind::InvalidInput)?;
            self.source.read_at(at, piece)?;
            out.write_all(piece)?;
            at += n as u64;
        }
        Ok(())
    }
}

/// The state of walking a file, object by object.
struct Walk<'a> {
    source: &'a dyn ByteSource,
    size: u64,
    /// The bytes of the file from `base`.
    window: Vec<u8>,
    base: u64,
    held: Vec<u8>,
    cuts: Vec<Cut>,
    /// Everything before this offset is held or cut.
    done: u64,
    /// Integer objects seen, to check the `/Length` of streams that ended by searching.
    integers: HashMap<u32, i64>,
    /// Cuts whose end came from a search, with the object their `/Length` refers to.
    searched: Vec<(usize, u32)>,
    /// What the walk may still read before giving up.
    budget: u64,
}

/// What looking through one window found.
enum Next {
    /// Go on from this offset.
    At(u64),
    /// The object there didn't fit the window: read more of it.
    Grow,
    /// The walk ran out of budget or held too much.
    GiveUp,
}

impl<'a> Walk<'a> {
    fn new(source: &'a dyn ByteSource, size: u64) -> Self {
        let budget = size.saturating_mul(4).saturating_add(1 << 30);
        Self {
            source,
            size,
            window: Vec::new(),
            base: 0,
            held: Vec::new(),
            cuts: Vec::new(),
            done: 0,
            integers: HashMap::new(),
            searched: Vec::new(),
            budget,
        }
    }

    fn read(&mut self, offset: u64, out: &mut [u8]) -> std::io::Result<bool> {
        let Some(left) = self.budget.checked_sub(out.len() as u64) else { return Ok(false) };
        self.budget = left;
        self.source.read_at(offset, out)?;
        Ok(true)
    }

    fn load(&mut self, at: u64, len: usize) -> std::io::Result<bool> {
        let len = len.min(usize::try_from(self.size.saturating_sub(at)).unwrap_or(usize::MAX));
        let mut window = std::mem::take(&mut self.window);
        window.resize(len, 0);
        let read = self.read(at, &mut window);
        self.window = window;
        self.base = at;
        read
    }

    fn may_hold(&self, more: u64) -> bool {
        may_hold(self.held.len() as u64, more, self.size)
    }

    /// Hold everything from `done` to `end`.
    fn hold_to(&mut self, end: u64) -> std::io::Result<bool> {
        let Some(more) = end.checked_sub(self.done).filter(|n| *n > 0) else { return Ok(true) };
        if !self.may_hold(more) {
            return Ok(false);
        }
        let (start, len) = (self.done, usize::try_from(more).map_err(|_| std::io::ErrorKind::OutOfMemory)?);
        self.held.try_reserve(len).map_err(|_| std::io::ErrorKind::OutOfMemory)?;
        let from = start.checked_sub(self.base).and_then(|at| usize::try_from(at).ok());
        match from.and_then(|at| self.window.get(at..at.checked_add(len)?)) {
            Some(bytes) => self.held.extend_from_slice(bytes),
            None => {
                let at = self.held.len();
                self.held.resize(at + len, 0);
                let mut held = std::mem::take(&mut self.held);
                let read = self.read(start, held.get_mut(at..).ok_or(std::io::ErrorKind::InvalidInput)?);
                self.held = held;
                if !read? {
                    return Ok(false);
                }
            }
        }
        self.done = end;
        Ok(true)
    }

    /// Walk the file. `false` when it can't stay on disk.
    fn run(&mut self) -> std::io::Result<bool> {
        let mut at = 0;
        let mut size = WINDOW;
        while at < self.size {
            if !self.load(at, size)? {
                return Ok(false);
            }
            match self.look_through()? {
                Next::At(next) => {
                    at = next;
                    size = WINDOW;
                }
                Next::Grow if size < MAX_WINDOW => size = size.saturating_mul(2),
                // Too large to look at: it stays held, and the walk goes on inside it.
                Next::Grow => {
                    at = at.saturating_add((size / 2) as u64);
                    size = WINDOW;
                }
                Next::GiveUp => return Ok(false),
            }
        }
        Ok(self.hold_to(self.size)? && self.searches_agree())
    }

    /// A body whose end came from a search must be what its `/Length` says: a stream can hold the
    /// word `endstream` (a PDF attached uncompressed), which ends a search early. A `/Length` the
    /// walk didn't see is in an object stream, and the search stands.
    fn searches_agree(&self) -> bool {
        self.searched.iter().all(|(cut, num)| {
            let length = self.integers.get(num).and_then(|n| u64::try_from(*n).ok());
            length.is_none_or(|length| self.cuts.get(*cut).is_some_and(|c| c.body.len == length))
        })
    }

    /// Go through the window's objects, cutting the bodies it proves.
    fn look_through(&mut self) -> std::io::Result<Next> {
        let eof = self.base.saturating_add(self.window.len() as u64) >= self.size;
        let mut from = 0;
        loop {
            let Some(keyword) = find(&self.window, b"obj", from) else {
                // A header cut by the end of the window comes back at the start of the next one.
                let rest = if eof { self.window.len() } else { self.window.len().saturating_sub(MARGIN).max(from.min(self.window.len())) };
                return Ok(Next::At(if rest == 0 { self.size } else { self.base + rest as u64 }));
            };
            let Some((header, num)) = object_header(&self.window, keyword) else {
                from = keyword + 3;
                continue;
            };
            let mut lexer = Lexer::new(&self.window, keyword + 3);
            let value = lexer.object();
            let end = lexer.pos;
            if !eof && end.saturating_add(MARGIN) >= self.window.len() {
                return Ok(if header > 0 { Next::At(self.base + header as u64) } else { Next::Grow });
            }
            let Ok(value) = value else {
                from = keyword + 3;
                continue;
            };
            from = end.max(keyword + 3);
            match value {
                Object::Int(n) => {
                    if self.integers.len() >= MAX_INTEGERS {
                        return Ok(Next::GiveUp);
                    }
                    self.integers.insert(num, n);
                }
                Object::Dict(dict) => {
                    let Some(body) = body_start(&self.window, end) else { continue };
                    let start = self.base + body as u64;
                    let declared = match dict.get(b"Length") {
                        Some(Object::Int(n)) => u64::try_from(*n).ok(),
                        _ => None,
                    };
                    let refers_to = match dict.get(b"Length") {
                        Some(Object::Ref(r)) => Some(r.num),
                        _ => None,
                    };
                    let proven = match declared {
                        Some(n) => self.endstream_at(start.saturating_add(n))?.then_some(n),
                        None => None,
                    };
                    let Some(len) = (match proven {
                        Some(n) => Some(n),
                        None => self.search_end(start)?,
                    }) else {
                        continue;
                    };
                    let end = start.saturating_add(len);
                    let structural = matches!(dict.name(b"Type"), Some(b"ObjStm" | b"XRef"));
                    let keep = len < MIN_BODY || structural;
                    if !self.hold_to(if keep { end } else { start })? {
                        return Ok(Next::GiveUp);
                    }
                    if !keep {
                        if let (None, Some(num)) = (proven, refers_to) {
                            self.searched.push((self.cuts.len(), num));
                        }
                        self.cuts.push(Cut { body: DiskBody { start, len }, held: self.held.len() });
                        self.done = end;
                    }
                    // Still in this window: loading it again per small stream would use up the budget.
                    match end.checked_sub(self.base).and_then(|at| usize::try_from(at).ok()).filter(|at| *at < self.window.len()) {
                        Some(at) => from = at,
                        None => return Ok(Next::At(end)),
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether `endstream` comes next at `at`, as a parser checks a `/Length`.
    fn endstream_at(&mut self, at: u64) -> std::io::Result<bool> {
        if at >= self.size {
            return Ok(false);
        }
        let mut word = [0; 32];
        let n = word.len().min(usize::try_from(self.size - at).unwrap_or(usize::MAX));
        let Some(word) = word.get_mut(..n) else { return Ok(false) };
        Ok(self.read(at, word)? && Lexer::new(word, 0).eat_keyword(b"endstream"))
    }

    /// The length of the stream body at `start`, as a parser finds it when `/Length` won't do.
    fn search_end(&mut self, start: u64) -> std::io::Result<Option<u64>> {
        const WORD: &[u8] = b"endstream";
        let mut chunk = vec![0; SEARCH_CHUNK];
        let mut at = start;
        while at < self.size {
            let n = chunk.len().min(usize::try_from(self.size - at).unwrap_or(usize::MAX));
            let Some(piece) = chunk.get_mut(..n) else { return Ok(None) };
            if !self.read(at, piece)? {
                return Ok(None);
            }
            if let Some(found) = piece.windows(WORD.len()).position(|w| w == WORD) {
                // The end-of-line before `endstream` is not data.
                let mut end = at + found as u64;
                for eol in b"\n\r" {
                    if end > start && self.byte_at(end - 1)? == Some(*eol) {
                        end -= 1;
                    }
                }
                return Ok(Some(end - start));
            }
            if n < chunk.len() {
                break;
            }
            at += (n - WORD.len() + 1) as u64;
        }
        Ok(None)
    }

    fn byte_at(&mut self, at: u64) -> std::io::Result<Option<u8>> {
        let mut byte = [0];
        Ok(self.read(at, &mut byte)?.then_some(byte[0]))
    }
}

/// Whether `more` bytes may join the `held` of a file of `size`: past half the file, keeping it on
/// disk saves too little to be worth being read-only for.
fn may_hold(held: u64, more: u64, size: u64) -> bool {
    held.saturating_add(more) <= (size / 2).min(MAX_HELD)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let found = hay.get(from..)?.windows(needle.len()).position(|w| w == needle)?;
    from.checked_add(found)
}

/// The `N G` before the keyword `obj` at `keyword`: where it starts, and `N`.
fn object_header(window: &[u8], keyword: usize) -> Option<(usize, u32)> {
    if window.get(keyword + 3).copied().is_some_and(is_regular) {
        return None;
    }
    let back_over = |mut at: usize, wanted: fn(u8) -> bool| {
        while at > 0 && window.get(at - 1).copied().is_some_and(wanted) {
            at -= 1;
        }
        at
    };
    let gen_end = back_over(keyword, is_whitespace);
    let gen_start = back_over(gen_end, |b| b.is_ascii_digit());
    let num_end = back_over(gen_start, is_whitespace);
    let num_start = back_over(num_end, |b| b.is_ascii_digit());
    if gen_end == keyword || gen_start == gen_end || num_end == gen_start || num_start == num_end {
        return None;
    }
    let number = std::str::from_utf8(window.get(num_start..num_end)?).ok()?.parse().ok()?;
    Some((num_start, number))
}

/// Where the body starts when the `stream` keyword comes next at `at`. The keyword must end its
/// line, or the parsers disagree on where the data starts: such a stream stays held.
fn body_start(window: &[u8], at: usize) -> Option<usize> {
    let mut lexer = Lexer::new(window, at);
    if !lexer.eat_keyword(b"stream") {
        return None;
    }
    let mut eol = lexer.pos;
    while matches!(window.get(eol), Some(b' ' | b'\t')) {
        eol += 1;
    }
    match (window.get(eol), window.get(eol + 1)) {
        (Some(b'\r'), Some(b'\n')) => Some(eol + 2),
        (Some(b'\n' | b'\r'), _) => Some(eol + 1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, SaveOptions, write_full, write_incremental};
    use pdfcraft_testkit::Pdf;
    use proptest::prelude::*;

    struct Memory(Vec<u8>);

    impl ByteSource for Memory {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
            let bytes = usize::try_from(offset).ok().and_then(|at| self.0.get(at..at.checked_add(out.len())?));
            out.copy_from_slice(bytes.ok_or(std::io::ErrorKind::UnexpectedEof)?);
            Ok(())
        }
    }

    fn walked(bytes: &[u8]) -> OnDiskFile {
        OnDiskFile::read(Arc::new(Memory(bytes.to_vec()))).expect("readable").expect("stays on disk")
    }

    /// The held bytes and the bodies put back together.
    fn joined(file: &OnDiskFile) -> Vec<u8> {
        let (mut out, mut held) = (Vec::new(), 0);
        for cut in &file.cuts {
            out.extend_from_slice(&file.held[held..cut.held]);
            out.extend_from_slice(&file.read_body(cut.body).expect("readable"));
            held = cut.held;
        }
        out.extend_from_slice(&file.held[held..]);
        out
    }

    fn noise(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) ^ (i >> 8) as u8).collect()
    }

    /// One page with `images` image streams of `len` bytes.
    fn with_images(images: usize, len: usize) -> Vec<u8> {
        let refs: Vec<String> = (0..images).map(|i| format!("/I{i} {} 0 R", 5 + i)).collect();
        let content: String = (0..images).map(|i| format!("q 10 0 0 10 {i} 0 cm /I{i} Do Q\n")).collect();
        let mut pdf = Pdf::new()
            .obj("<</Type/Catalog/Pages 2 0 R>>")
            .obj("<</Type/Pages/Kids[3 0 R]/Count 1>>")
            .obj(format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 100 100]/Contents 4 0 R/Resources<</XObject<<{}>>>>>>", refs.join(" ")))
            .stream("", content);
        for i in 0..images {
            pdf = pdf.stream("/Type/XObject/Subtype/Image/Width 100/Height 100/BitsPerComponent 8/ColorSpace/DeviceGray", noise(i as u8, len));
        }
        pdf.build()
    }

    #[test]
    fn large_bodies_stay_on_disk_and_the_rest_is_held_in_order() {
        let pdf = with_images(4, 20_000);
        let file = walked(&pdf);
        assert_eq!(file.body_count(), 4);
        assert!(file.held().len() < 2_000, "held {} bytes", file.held().len());
        assert_eq!(joined(&file), pdf);
        assert_eq!(file.size(), pdf.len() as u64);
    }

    #[test]
    fn file_offsets_map_around_the_bodies() {
        let pdf = with_images(2, 20_000);
        let file = walked(&pdf);
        let (first, second) = (file.cuts[0], file.cuts[1]);
        assert_eq!(file.held_offset(0), Some(0));
        assert_eq!(file.held_offset(first.body.start - 1), Some(first.held - 1));
        assert_eq!(file.held_offset(first.body.start), None, "inside a body");
        assert_eq!(file.held_offset(first.body.start + first.body.len - 1), None);
        assert_eq!(file.held_offset(first.body.start + first.body.len), Some(first.held));
        assert_eq!(file.held_offset(second.body.start + second.body.len), Some(second.held));
        assert_eq!(file.body_at(first.held), Some(first.body));
        assert_eq!(file.body_at(first.held + 1), None);
        let tail = pdf.len() as u64 - 1;
        assert_eq!(file.held()[file.held_offset(tail).unwrap()], pdf[pdf.len() - 1]);
    }

    /// Every object must read the same through the held bytes as through the whole file.
    fn assert_reads_the_same(pdf: &[u8], file: Arc<OnDiskFile>) {
        let whole = Document::open(Arc::new(pdf.to_vec())).expect("opens");
        let on_disk = Document::open_on_disk(file, usize::MAX).expect("stays on disk");
        assert_eq!(whole.object_numbers(), on_disk.object_numbers());
        assert_eq!(whole.trailer(), on_disk.trailer());
        for num in whole.object_numbers() {
            assert_eq!(whole.try_get(num).unwrap(), on_disk.try_get(num).unwrap(), "object {num}");
        }
    }

    #[test]
    fn objects_read_the_same_as_from_the_whole_file() {
        let pdf = with_images(3, 30_000);
        assert_reads_the_same(&pdf, Arc::new(walked(&pdf)));
    }

    #[test]
    fn an_update_on_top_of_the_file_reads_through_the_cuts() {
        let pdf = with_images(2, 30_000);
        let mut doc = Document::open(Arc::new(pdf)).unwrap();
        let info = doc.add(crate::Dict::new());
        let mut trailer_info = crate::Dict::new();
        trailer_info.set(b"Producer".to_vec(), Object::Ref(info));
        doc.add(trailer_info);
        let updated = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let file = walked(&updated);
        assert_eq!(file.body_count(), 2);
        assert_eq!(joined(&file), updated);
        assert_reads_the_same(&updated, Arc::new(file));
    }

    #[test]
    fn object_streams_and_cross_reference_streams_stay_held() {
        let doc = Document::open(Arc::new(with_images(2, 30_000))).unwrap();
        let packed = write_full(&doc, &SaveOptions::default()).unwrap();
        let reparsed = Document::open(Arc::new(packed.clone())).unwrap();
        assert!(reparsed.revisions().iter().any(|r| r.is_stream), "a file with a cross-reference stream");
        let file = walked(&packed);
        assert_eq!(file.body_count(), 2);
        assert_eq!(joined(&file), packed);
        assert_reads_the_same(&packed, Arc::new(file));
    }

    #[test]
    fn an_indirect_length_is_searched_and_checked() {
        let pdf = Pdf::new()
            .obj("<</Type/Catalog/Pages 2 0 R>>")
            .obj("<</Type/Pages/Kids[]/Count 0>>")
            .obj(format!("<</Length 4 0 R>>\nstream\n{}\nendstream", "x".repeat(9_000)))
            .obj("9000")
            .build();
        let file = walked(&pdf);
        assert_eq!(file.body_count(), 1);
        assert_eq!(file.cuts[0].body.len, 9_000);
        assert_reads_the_same(&pdf, Arc::new(file));
    }

    #[test]
    fn a_body_holding_the_word_endstream_with_an_indirect_length_is_not_cut_short() {
        // The search stops at the word inside the body; its /Length says otherwise.
        let mut body = vec![b'x'; 5_000];
        body.extend_from_slice(b"\nendstream\nendobj\n99 0 obj\n<</Length 6000>>\nstream\n");
        body.extend_from_slice(&vec![b'y'; 6_000]);
        let length = body.len();
        let pdf = Pdf::new()
            .obj("<</Type/Catalog/Pages 2 0 R>>")
            .obj("<</Type/Pages/Kids[]/Count 0>>")
            .obj([b"<</Length 4 0 R>>\nstream\n".as_slice(), &body, b"\nendstream"].concat())
            .obj(length.to_string())
            .build();
        assert!(OnDiskFile::read(Arc::new(Memory(pdf))).unwrap().is_none(), "read whole instead");
    }

    #[test]
    fn a_stream_keyword_inside_a_string_is_not_a_stream() {
        let fake = format!("<</S(>> stream\n{}\nendstream)>>", "z".repeat(8_000));
        let pdf = Pdf::new().obj("<</Type/Catalog/Pages 2 0 R>>").obj("<</Type/Pages/Kids[]/Count 0>>").obj(fake).build();
        let walk = OnDiskFile::read(Arc::new(Memory(pdf))).unwrap();
        assert!(walk.is_none_or(|f| f.body_count() == 0));
    }

    #[test]
    fn a_file_with_little_to_leave_on_disk_is_read_whole() {
        let pdf = with_images(4, 100);
        assert!(OnDiskFile::read(Arc::new(Memory(pdf))).unwrap().is_none());
    }

    #[test]
    fn what_a_walk_holds_has_a_limit_whatever_the_size_of_the_file() {
        assert!(may_hold(0, 50, 100));
        assert!(!may_hold(0, 51, 100));
        assert!(may_hold(MAX_HELD - 1, 1, u64::MAX));
        assert!(!may_hold(MAX_HELD, 1, u64::MAX), "a hostile file of any size holds no more than this");
        assert!(!may_hold(u64::MAX, u64::MAX, u64::MAX));
    }

    #[test]
    fn a_wrong_length_is_found_by_its_endstream() {
        let body = "w".repeat(10_000);
        let pdf = Pdf::new()
            .obj("<</Type/Catalog/Pages 2 0 R>>")
            .obj("<</Type/Pages/Kids[]/Count 0>>")
            .obj(format!("<</Length 12>>\nstream\n{body}\nendstream"))
            .build();
        let file = walked(&pdf);
        assert_eq!(file.cuts[0].body.len, 10_000);
    }

    #[test]
    fn windows_never_cut_a_header_or_a_dictionary_short() {
        // Objects long enough to straddle every window boundary.
        for pad in [WINDOW - 40, WINDOW - 8, WINDOW + 3, 2 * WINDOW + 100] {
            let filler = format!("<</Pad({})>>", "p".repeat(pad));
            let pdf = Pdf::new()
                .obj("<</Type/Catalog/Pages 2 0 R>>")
                .obj("<</Type/Pages/Kids[]/Count 0>>")
                .obj(filler)
                .stream("/Filter/None", noise(3, 2_000_000))
                .build();
            let file = walked(&pdf);
            assert_eq!(file.body_count(), 1, "padding {pad}");
            assert_eq!(joined(&file), pdf);
        }
    }

    #[test]
    fn a_read_that_fails_is_counted_and_gives_nothing() {
        struct Shrinks(Vec<u8>, std::sync::atomic::AtomicBool);
        impl ByteSource for Shrinks {
            fn size(&self) -> u64 {
                self.0.len() as u64
            }
            fn read_at(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
                if self.1.load(Ordering::Relaxed) {
                    return Err(std::io::ErrorKind::NotFound.into());
                }
                Memory(self.0.clone()).read_at(offset, out)
            }
        }
        let source = Arc::new(Shrinks(with_images(1, 20_000), false.into()));
        let file = OnDiskFile::read(source.clone()).unwrap().unwrap();
        let body = file.cuts[0].body;
        assert!(file.read_body(body).is_some());
        assert_eq!(file.failed_reads(), 0);
        source.1.store(true, Ordering::Relaxed);
        assert_eq!(file.read_body(body), None);
        assert_eq!(file.failed_reads(), 1);
    }

    #[test]
    fn copy_to_writes_the_file_itself() {
        let pdf = with_images(2, 3_000_000);
        let file = walked(&pdf);
        let mut out = Vec::new();
        file.copy_to(&mut out).unwrap();
        assert_eq!(out, pdf);
    }

    #[test]
    fn hostile_files_read_without_panicking() {
        for pdf in [
            b"%PDF-1.7\n1 0 obj\n<</Length 99999999999999999999>>\nstream\nabc\nendstream\nendobj\n".to_vec(),
            b"%PDF-1.7\n1 0 obj\n<</Length -5>>\nstream\nabc\nendstream\nendobj\n".to_vec(),
            b"%PDF-1.7\n1 0 obj\n<</Length 9223372036854775807>>\nstream\n".to_vec(),
            b"1 0 obj".to_vec(),
            b"obj".to_vec(),
            Vec::new(),
            [b"%PDF-1.7\n".as_slice(), &b"4294967296 0 obj ".repeat(50_000)].concat(),
            [b"%PDF-1.7\n1 0 obj\n".as_slice(), &b"[".repeat(1 << 18)].concat(),
        ] {
            let _ = OnDiskFile::read(Arc::new(Memory(pdf))).unwrap();
        }
    }

    #[test]
    fn a_cross_reference_entry_inside_a_body_reads_as_in_the_whole_file() {
        let mut pdf = with_images(1, 20_000);
        let inside = walked(&pdf).cuts[0].body.start + 100;
        // Point the entry of object 1, the catalog, into the image data: both find it by searching.
        let table = find(&pdf, b"\nxref\n", 0).unwrap() + 6;
        let entry = find(&pdf, b"\n", table).unwrap() + 1 + 20;
        pdf.splice(entry..entry + 10, format!("{inside:010}").into_bytes());
        assert_reads_the_same(&pdf, Arc::new(walked(&pdf)));
    }

    #[test]
    fn small_streams_do_not_read_a_window_each() {
        struct Counted(Vec<u8>, AtomicU64);
        impl ByteSource for Counted {
            fn size(&self) -> u64 {
                self.0.len() as u64
            }
            fn read_at(&self, offset: u64, out: &mut [u8]) -> std::io::Result<()> {
                self.1.fetch_add(out.len() as u64, Ordering::Relaxed);
                let at = offset as usize;
                out.copy_from_slice(self.0.get(at..at + out.len()).ok_or(std::io::ErrorKind::UnexpectedEof)?);
                Ok(())
            }
        }
        let mut pdf = Pdf::new().obj("<</Type/Catalog/Pages 2 0 R>>").obj("<</Type/Pages/Kids[]/Count 0>>");
        for i in 0..2_000 {
            pdf = pdf.stream("", noise(i as u8, 100));
        }
        let pdf = pdf.stream("", noise(0, 2_000_000)).build();
        let size = pdf.len() as u64;
        let source = Arc::new(Counted(pdf, AtomicU64::new(0)));
        assert!(OnDiskFile::read(source.clone()).unwrap().is_some());
        let read = source.1.load(Ordering::Relaxed);
        assert!(read < size * 2, "read {read} bytes of a file of {size}");
    }

    #[test]
    fn a_file_that_needs_repair_or_a_password_does_not_stay_on_disk() {
        let mut broken = with_images(1, 20_000);
        broken.truncate(find(&broken, b"startxref", 0).unwrap());
        assert!(Document::open_on_disk(Arc::new(walked(&broken)), usize::MAX).is_err());
    }

    #[test]
    fn writers_refuse_a_document_on_disk() {
        let doc = Document::open_on_disk(Arc::new(walked(&with_images(1, 20_000))), usize::MAX).unwrap();
        assert_eq!(write_incremental(&doc, &SaveOptions::default()), Err(crate::CosError::ReadOnlyLimit));
        assert_eq!(write_full(&doc, &SaveOptions::default()), Err(crate::CosError::ReadOnlyLimit));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Any mix of stream sizes and end-of-line styles: the walk gives back the file exactly.
        #[test]
        fn the_held_bytes_and_the_bodies_are_the_file(
            sizes in proptest::collection::vec(0usize..12_000, 1..8),
            eols in proptest::collection::vec(0usize..3, 8),
            padding in 0usize..200,
        ) {
            let mut pdf = b"%PDF-1.7\n".to_vec();
            pdf.extend_from_slice(&vec![b'%'; padding]);
            pdf.push(b'\n');
            for (i, size) in sizes.iter().enumerate() {
                let eol: &[u8] = [b"\n".as_slice(), b"\r\n", b" \r\n"][eols[i]];
                pdf.extend_from_slice(format!("{} 0 obj\n<</Length {size}>>\nstream", i + 1).as_bytes());
                pdf.extend_from_slice(eol);
                pdf.extend_from_slice(&noise(i as u8, *size));
                pdf.extend_from_slice(b"\nendstream\nendobj\n");
            }
            if let Some(file) = OnDiskFile::read(Arc::new(Memory(pdf.clone()))).unwrap() {
                prop_assert_eq!(joined(&file), pdf);
            }
        }

        /// Bytes that are not a PDF at all never panic the walk.
        #[test]
        fn the_walk_survives_noise(bytes in proptest::collection::vec(any::<u8>(), 0..4_000)) {
            let _ = OnDiskFile::read(Arc::new(Memory(bytes))).unwrap();
        }
    }
}
