//! A shared allowance for operations that decode many streams.

use std::collections::HashSet;

use pdfcraft_filters::FilterError;

use crate::{CosError, Document, ObjRef, Object, Stream};

/// The steps of JBIG2 decoding ([`hayro_jbig2::Image::decode_within`]) an operation over a
/// document's images may take whatever the file's size. A step is about a pixel, and takes 6 to 16
/// ns to decode, so this is 2 to 5 minutes, and about as long again for what an operation does with
/// the pixels. A page scanned at 300 dpi takes about 17 million, 8.4 for its pixels and that again
/// to decode them, so this alone reads about 1,000 of them.
const IMAGE_WORK: u64 = 1 << 34;
/// The steps each byte of the file adds to [`IMAGE_WORK`], so that a long scan is read whole: such a
/// page pays for itself with about 2 KB of the file, about the smallest a page of text is stored
/// in. So what a hostile file can ask for grows with its length, by about 14 minutes for 10 MB.
const IMAGE_WORK_PER_BYTE: u64 = 1 << 13;

/// Decoded bytes one page's content walk may produce in all: its `/Contents` streams and every
/// form XObject it enters, each occurrence counted again.
pub const MAX_PAGE_DECODED: usize = 256 << 20;
/// Form XObjects one page's content walk may enter, each entry counted again.
pub const MAX_PAGE_FORMS: usize = 50_000;

/// A stream decode within a byte bound, with the bytes its filter stages decoded: the data or
/// the error, each with that count.
type Decoder = fn(&Stream, usize) -> Result<(Vec<u8>, usize), (FilterError, usize)>;

/// What one operation over a document's images may still spend decoding them: exporting them,
/// optimizing them, clearing them for redaction.
///
/// An image has its own limit, which doesn't bound a file of thousands of images made to reach it.
/// JBIG2 is what such a file would use: a few hundred bytes can ask for seconds of decoding, and an
/// image that shares `/JBIG2Globals` decodes them again. So those images share this allowance, and
/// past it they aren't decoded. A caller then treats them as it does an image it can't read, and
/// [`ImageBudget::refused`] says how many there were.
#[derive(Debug)]
pub struct ImageBudget {
    jbig2_work: u64,
    refused: usize,
}

impl ImageBudget {
    /// The allowance for an operation on `doc`, which a longer file makes larger.
    pub fn new(doc: &Document) -> Self {
        let bytes = u64::try_from(doc.bytes().len()).unwrap_or(u64::MAX);
        Self::with_work(IMAGE_WORK.saturating_add(bytes.saturating_mul(IMAGE_WORK_PER_BYTE)))
    }

    /// An allowance of `jbig2_work` steps of JBIG2 decoding.
    pub fn with_work(jbig2_work: u64) -> Self {
        Self { jbig2_work, refused: 0 }
    }

    /// Image `s` decoded like [`Document::decoded_image_within`], charged to the allowance.
    pub fn decode(&mut self, doc: &Document, s: &Stream, max: usize) -> Result<Vec<u8>, CosError> {
        let decoded = doc.decoded_image_spending(s, max, &mut self.jbig2_work);
        if decoded.is_err() && self.jbig2_work == 0 && matches!(s.filters().last(), Some((pdfcraft_filters::Filter::Jbig2, _))) {
            self.refused += 1;
        }
        decoded
    }

    /// The JBIG2 images refused for lack of allowance.
    pub fn refused(&self) -> usize {
        self.refused
    }

    /// The steps of JBIG2 decoding still allowed.
    pub fn work_left(&self) -> u64 {
        self.jbig2_work
    }
}

/// What one operation may still decode and enter across all the streams it reads.
///
/// [`MAX_DECODED`](crate::MAX_DECODED) bounds one decode. It does not bound a page whose
/// `/Contents` names the same compressed stream thousands of times, or a form drawn thousands of
/// times from other forms: each occurrence would get a fresh allowance. A `DecodeBudget` is
/// shared by every decode of one operation, so the total decoded bytes and form entries stay
/// under fixed limits however the file repeats its references. A decode that runs past the
/// remaining allowance spends all of it, so a bomb referenced many times is inflated once.
#[derive(Debug)]
pub struct DecodeBudget {
    /// The whole allowance, for errors.
    limit: usize,
    bytes: usize,
    forms: usize,
    seen: HashSet<ObjRef>,
}

impl DecodeBudget {
    /// An allowance of `bytes` decoded bytes and `forms` form entries.
    pub fn new(bytes: usize, forms: usize) -> Self {
        Self { limit: bytes, bytes, forms, seen: HashSet::new() }
    }

    /// The allowance for one page: [`MAX_PAGE_DECODED`] bytes and [`MAX_PAGE_FORMS`] forms.
    pub fn page() -> Self {
        Self::new(MAX_PAGE_DECODED, MAX_PAGE_FORMS)
    }

    /// Decoded bytes still allowed.
    pub fn remaining(&self) -> usize {
        self.bytes
    }

    /// Whether a decode was refused for lack of allowance or a form entry for lack of visits.
    pub fn exhausted(&self) -> bool {
        self.bytes == 0 || self.forms == 0
    }

    /// `stream` decoded like [`Stream::decoded`], charged to the allowance. Fails, without
    /// decoding, once the allowance is spent.
    pub fn decode(&mut self, stream: &Stream) -> Result<Vec<u8>, CosError> {
        self.charge(stream, usize::MAX, Stream::decoded_within_counted)
    }

    /// Like [`DecodeBudget::decode`], for a stream with a tighter bound of its own: decoding
    /// past `max` bytes is an error that charges `max`, the work it took to find out.
    pub fn decode_within(&mut self, stream: &Stream, max: usize) -> Result<Vec<u8>, CosError> {
        self.charge(stream, max, Stream::decoded_within_counted)
    }

    /// `stream` decoded like [`Stream::decoded_strict`] (any corruption is an error), charged to
    /// the allowance.
    pub fn decode_strict(&mut self, stream: &Stream) -> Result<Vec<u8>, CosError> {
        self.charge(stream, usize::MAX, Stream::decoded_strict_within_counted)
    }

    /// Decodes `stream` within `max` and the allowance, charging what every filter stage
    /// decoded: a stage's output counts even when a later stage shrinks it, and a corrupt stream
    /// pays for the bytes decoded before the error, so referencing a stream again and again
    /// spends the allowance by the work each decode does.
    fn charge(&mut self, stream: &Stream, max: usize, decode: Decoder) -> Result<Vec<u8>, CosError> {
        if self.bytes == 0 && !stream.raw.is_empty() {
            return Err(CosError::Filter(FilterError::LimitExceeded(self.limit).to_string()));
        }
        if max < self.bytes {
            let result = decode(stream, max);
            let spent = match &result {
                Ok((_, spent)) => *spent,
                Err((FilterError::LimitExceeded(_), _)) => max,
                Err((_, spent)) => *spent,
            };
            self.bytes = self.bytes.saturating_sub(spent);
            return result.map(|(data, _)| data).map_err(|(e, _)| CosError::Filter(e.to_string()));
        }
        match decode(stream, self.bytes) {
            Ok((data, spent)) => {
                self.bytes = self.bytes.saturating_sub(spent);
                Ok(data)
            }
            Err((FilterError::LimitExceeded(_), _)) => {
                self.bytes = 0;
                Err(CosError::Filter(FilterError::LimitExceeded(self.limit).to_string()))
            }
            Err((e, spent)) => {
                self.bytes = self.bytes.saturating_sub(spent);
                Err(CosError::Filter(e.to_string()))
            }
        }
    }

    /// Counts one form entry. `false` once [`DecodeBudget::new`]'s `forms` entries are spent:
    /// the caller leaves that form out.
    pub fn enter_form(&mut self) -> bool {
        match self.forms.checked_sub(1) {
            Some(left) => {
                self.forms = left;
                true
            }
            None => false,
        }
    }

    /// `true` the first time `r` is passed to this budget, for callers that need each
    /// referenced stream only once.
    pub fn first_visit(&mut self, r: ObjRef) -> bool {
        self.seen.insert(r)
    }

    /// Each stream `contents` names, decoded against the allowance, in order. `contents` is a
    /// page's `/Contents`: one stream, or an array whose non-stream entries are left out.
    /// A stream that fails to decode, or arrives after the allowance is spent, is an `Err` in its
    /// place, so the caller chooses between skipping it and giving up.
    pub fn decode_contents(&mut self, doc: &Document, contents: &Object) -> Vec<Result<Vec<u8>, CosError>> {
        match &*doc.resolve(contents) {
            Object::Stream(s) => vec![self.decode(s)],
            Object::Array(list) => list
                .iter()
                .filter_map(|entry| match &*doc.resolve(entry) {
                    Object::Stream(s) => Some(self.decode(s)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use crate::Dict;

    fn jbig2(parms: Option<Object>) -> Stream {
        let mut dict = Dict::new();
        dict.set(b"Width".to_vec(), Object::Int(16));
        dict.set(b"Height".to_vec(), Object::Int(2));
        dict.set(b"Filter".to_vec(), Object::name("JBIG2Decode"));
        if let Some(parms) = parms {
            dict.set(b"DecodeParms".to_vec(), parms);
        }
        Stream::from_raw(dict, pdfcraft_filters::encode_jbig2_generic(&[0xFF; 4], 16, 2).unwrap())
    }

    /// Each JBIG2 image spends what it took from one allowance, and those past it are refused and
    /// counted. Other images aren't charged, and a failure of theirs isn't counted as one of these.
    #[test]
    fn the_images_of_an_operation_share_its_allowance() {
        let doc = Document::new_empty();
        let image = jbig2(None);
        let mut budget = ImageBudget::with_work(u64::MAX);
        let samples = budget.decode(&doc, &image, 1 << 20).unwrap();
        let cost = u64::MAX - budget.work_left();
        assert!(cost > 0);
        let mut budget = ImageBudget::with_work(3 * cost);
        for left in [2, 1, 0] {
            assert_eq!(budget.decode(&doc, &image, 1 << 20).unwrap(), samples);
            assert_eq!(budget.work_left(), left * cost);
        }
        assert_eq!(budget.refused(), 0, "the allowance held exactly three");
        for refused in [1, 2] {
            assert!(budget.decode(&doc, &image, 1 << 20).is_err());
            assert_eq!(budget.refused(), refused);
        }
        let flate = Stream::flate(Dict::new(), b"samples");
        assert_eq!(budget.decode(&doc, &flate, 1 << 20).unwrap(), b"samples", "not JBIG2: no charge");
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::name("NoSuchFilter"));
        assert!(budget.decode(&doc, &Stream::from_raw(dict, b"data".to_vec()), 1 << 20).is_err());
        assert_eq!(budget.refused(), 2, "not JBIG2: not one of these");
    }

    /// The images of a file share its globals, so an image refused doesn't read them: this one's
    /// can't be read, which is how to tell.
    #[test]
    fn an_image_past_the_allowance_does_not_read_its_globals() {
        let mut doc = Document::new_empty();
        let mut unreadable = Dict::new();
        unreadable.set(b"Filter".to_vec(), Object::name("NoSuchFilter"));
        let globals = doc.add(Object::Stream(Stream::from_raw(unreadable, b"data".to_vec())));
        let mut parms = Dict::new();
        parms.set(b"JBIG2Globals".to_vec(), Object::Ref(globals));
        let image = jbig2(Some(Object::Dict(parms)));
        let read = ImageBudget::with_work(u64::MAX).decode(&doc, &image, 1 << 20).unwrap_err().to_string();
        assert!(read.contains("globals"), "{read}");
        let refused = ImageBudget::with_work(0).decode(&doc, &image, 1 << 20).unwrap_err().to_string();
        assert!(refused.contains("more JBIG2 decoding than"), "{refused}");
    }

    /// A longer file gets more, by its length, and an empty one the fixed part.
    #[test]
    fn a_longer_file_allows_more_decoding() {
        assert_eq!(ImageBudget::new(&Document::new_empty()).work_left(), IMAGE_WORK);
        let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF".to_vec();
        let len = pdf.len() as u64;
        let doc = Document::open(std::sync::Arc::new(pdf)).unwrap();
        assert_eq!(ImageBudget::new(&doc).work_left(), IMAGE_WORK + len * IMAGE_WORK_PER_BYTE);
    }

    // What the constants' comments promise, for a page scanned at 300 dpi: about 1,000 of them
    // whatever the file's size, and a page paying for itself with 2 KB of the file.
    const PAGE: u64 = 17_000_000;
    const _: () = assert!(IMAGE_WORK >= 1_000 * PAGE);
    const _: () = assert!(IMAGE_WORK_PER_BYTE * 2_100 >= PAGE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dict;

    #[test]
    fn repeated_references_share_one_allowance() {
        let mut doc = Document::new_empty();
        let bomb = doc.add(Object::Stream(Stream::flate(Dict::new(), &vec![b' '; 1 << 20])));
        let contents = Object::Array(vec![Object::Ref(bomb); 64]);
        let mut budget = DecodeBudget::new(10 << 20, 1);
        let decoded = budget.decode_contents(&doc, &contents);
        assert_eq!(decoded.len(), 64);
        assert_eq!(decoded.iter().filter(|d| d.is_ok()).count(), 10);
        let total: usize = decoded.iter().flatten().map(Vec::len).sum();
        assert_eq!(total, 10 << 20);
        assert!(budget.exhausted());
        // A decode past the allowance spends it, so a bomb is inflated at most once more.
        let mut budget = DecodeBudget::new((1 << 20) - 1, 1);
        assert!(budget.decode_contents(&doc, &contents).iter().all(Result::is_err));
        assert_eq!(budget.remaining(), 0);
        let mut budget = DecodeBudget::new((1 << 20) - 1, 1);
        let Object::Stream(stream) = &*doc.get(bomb) else { panic!("not a stream") };
        assert!(budget.decode_strict(stream).is_err());
        assert_eq!(budget.remaining(), 0);

        let mut budget = DecodeBudget::new(10 << 20, 1);
        assert!(budget.decode_within(stream, 1000).is_err());
        assert_eq!(budget.remaining(), (10 << 20) - 1000);
        assert_eq!(budget.decode_within(stream, 2 << 20).map(|d| d.len()).ok(), Some(1 << 20));
        assert_eq!(budget.remaining(), (9 << 20) - 1000);

        assert!(budget.enter_form());
        assert!(!budget.enter_form());
        assert!(budget.first_visit(bomb));
        assert!(!budget.first_visit(bomb));
    }

    /// A corrupt stream fails a strict decode after decoding part of it: each reference pays
    /// for that part, so references to it can't decode without limit.
    #[test]
    fn failed_strict_decodes_charge_what_they_decoded() {
        let whole = Stream::flate(Dict::new(), &vec![b' '; 1 << 20]);
        let cut = whole.raw.len() / 2;
        let corrupt = Stream::from_raw(whole.dict.clone(), whole.raw.get(..cut).unwrap_or_default().to_vec());
        let partial = corrupt.decoded().map(|d| d.len()).unwrap_or(0);
        assert!(partial > 0, "a cut flate stream decodes in part");
        let mut budget = DecodeBudget::new(10 << 20, 1);
        assert!(budget.decode_strict(&corrupt).is_err());
        assert_eq!(budget.remaining(), (10 << 20) - partial);
        let mut budget = DecodeBudget::new(10 << 20, 1);
        assert!(budget.decode_within(&corrupt, 1 << 30).is_ok());
        assert_eq!(budget.remaining(), (10 << 20) - partial);
    }

    /// A chain whose later stage shrinks the data pays for every stage's output: a Flate stage
    /// that inflates to hex text, which ASCIIHex then halves, charges both.
    #[test]
    fn decodes_charge_every_stage_of_a_chain() {
        let hex: Vec<u8> = std::iter::repeat_n(b"ab".as_slice(), 1 << 19).flatten().copied().collect();
        let flate = Stream::flate(Dict::new(), &hex);
        let mut dict = Dict::new();
        dict.set(b"Filter".to_vec(), Object::Array(vec![Object::name("FlateDecode"), Object::name("ASCIIHexDecode")]));
        let chained = Stream::from_raw(dict, flate.raw.to_vec());
        assert_eq!(chained.decoded().map(|d| d.len()).ok(), Some(1 << 19));
        let charged = (1 << 20) + (1 << 19);
        let mut budget = DecodeBudget::new(10 << 20, 1);
        assert_eq!(budget.decode(&chained).map(|d| d.len()).ok(), Some(1 << 19));
        assert_eq!(budget.remaining(), (10 << 20) - charged);
        assert!(budget.decode_within(&chained, 2 << 20).is_ok());
        assert_eq!(budget.remaining(), (10 << 20) - 2 * charged);
        assert!(budget.decode_strict(&chained).is_ok());
        assert_eq!(budget.remaining(), (10 << 20) - 3 * charged);
    }
}
