//! Vendored lopdf patch (8): a `/FlateDecode` object stream is decoded as it is read, its
//! members parsed in batches, instead of being decoded whole first.
//!
//! The reference is lopdf's whole-stream path, reached with the same decoded bytes unfiltered:
//! an unfiltered stream is parsed from its own bytes and refused when they exceed the limit, as a
//! Flate stream decoding past it was. Every case loads both and wants the same members or the
//! same error, and the same node budget left.

use flate2::Compression;
use flate2::write::{DeflateEncoder, ZlibEncoder};
use lopdf::{DecompressError, Dictionary, Document, Error, LoadOptions, NodeBudget, ObjectStream, Stream};
use std::io::Write;

/// lopdf's budget for one load (`MAX_LOAD_NODES`).
const LOAD_NODES: usize = 1 << 23;

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Decoded object-stream content: an index of `(number, offset)` pairs, then `members`. Returns
/// the content and `/First`.
fn decoded(pairs: &[(u32, usize)], members: &[u8]) -> (Vec<u8>, usize) {
    let mut content = Vec::new();
    for &(id, offset) in pairs {
        content.extend_from_slice(format!("{id} {offset} ").as_bytes());
    }
    let first = content.len();
    content.extend_from_slice(members);
    (content, first)
}

fn object_stream(content: Vec<u8>, first: usize, n: usize, flate: bool) -> Stream {
    let mut dict = Dictionary::new();
    dict.set("Type", "ObjStm");
    dict.set("N", n as i64);
    dict.set("First", first as i64);
    if flate {
        dict.set("Filter", "FlateDecode");
    }
    Stream::new(dict, content)
}

fn flate(pairs: &[(u32, usize)], members: &[u8]) -> (Stream, usize) {
    let (content, first) = decoded(pairs, members);
    let size = content.len();
    (object_stream(zlib(&content), first, pairs.len(), true), size)
}

/// What a Flate object stream decodes to: the deflate data after the two-byte zlib header,
/// without the Adler-32 checksum, decoded in 64 KiB windows up to its end, or up to an error with
/// every byte decoded before it. For intact data that is what `Stream::decompressed_content`
/// gives.
fn inflated(encoded: &[u8]) -> Vec<u8> {
    let mut input = encoded.get(2..).unwrap_or_default();
    let mut decompress = flate2::Decompress::new(false);
    let mut window = vec![0; 64 << 10];
    let mut out = Vec::new();
    loop {
        let (before_in, before_out) = (decompress.total_in(), decompress.total_out());
        let status = decompress.decompress(input, &mut window, flate2::FlushDecompress::None);
        let consumed = (decompress.total_in() - before_in) as usize;
        let written = (decompress.total_out() - before_out) as usize;
        input = &input[consumed..];
        out.extend_from_slice(&window[..written]);
        match status {
            Ok(flate2::Status::StreamEnd) | Err(_) => return out,
            Ok(_) if consumed == 0 && written == 0 => return out,
            Ok(_) => {}
        }
    }
}

/// The same stream with its decoded bytes and no filter, which lopdf parses whole.
fn unfiltered(stream: &Stream) -> Stream {
    let mut plain = stream.clone();
    plain.dict.remove(b"Filter");
    plain.content = inflated(&stream.content);
    plain
}

type Loaded = lopdf::Result<ObjectStream>;

/// Load a Flate `stream` and its unfiltered twin from budgets of `nodes`, assert that they agree,
/// and return the Flate result.
fn same(stream: &Stream, limit: Option<usize>, nodes: usize) -> Loaded {
    let plain_budget = NodeBudget::new(nodes);
    let expected = ObjectStream::new_within(&unfiltered(stream), limit, &plain_budget);
    let budget = NodeBudget::new(nodes);
    let loaded = ObjectStream::new_within(stream, limit, &budget);
    match (&loaded, &expected) {
        (Ok(loaded), Ok(expected)) => assert_eq!(loaded.objects, expected.objects, "limit {limit:?}, nodes {nodes}"),
        (Err(loaded), Err(expected)) => {
            assert_eq!(format!("{loaded:?}"), format!("{expected:?}"), "limit {limit:?}, nodes {nodes}")
        }
        _ => panic!("limit {limit:?}, nodes {nodes}: {loaded:?} but the whole stream gives {expected:?}"),
    }
    assert_eq!(budget.left(), plain_budget.left(), "nodes left, limit {limit:?}, nodes {nodes}");
    loaded
}

fn limit_error(loaded: Loaded) -> Option<usize> {
    match loaded {
        Err(Error::Decompress(DecompressError::MemoryLimitExceeded { limit })) => Some(limit),
        _ => None,
    }
}

/// Three members, then `tail` decoded bytes of whitespace no member needs, which the whole-stream
/// path holds at once.
fn with_long_tail(tail: usize) -> (Stream, usize) {
    let mut members = b"(first) [1 2 3] << /K /V >>".to_vec();
    members.extend(std::iter::repeat_n(b' ', tail));
    flate(&[(7, 0), (8, 8), (9, 16)], &members)
}

#[test]
fn flate_members_are_the_members_of_the_decoded_bytes() {
    let (stream, size) = with_long_tail(1 << 20);
    let loaded = same(&stream, Some(size), LOAD_NODES).unwrap();
    assert_eq!(loaded.objects.get(&(7, 0)).unwrap().as_str().unwrap(), b"first");
    assert_eq!(loaded.objects.get(&(8, 0)).unwrap().as_array().unwrap().len(), 3);
    assert!(loaded.objects.get(&(9, 0)).unwrap().as_dict().unwrap().has(b"K"));
}

/// Every decoded byte counts towards the limit, including those after the last member: a stream
/// exactly at the limit loads and one byte more is refused, even when the excess is whitespace
/// nothing keeps.
#[test]
fn every_decoded_byte_counts_towards_the_limit() {
    let (stream, size) = with_long_tail(12 << 20);
    assert_eq!(same(&stream, Some(size), LOAD_NODES).unwrap().objects.len(), 3);
    assert_eq!(limit_error(same(&stream, Some(size - 1), LOAD_NODES)), Some(size - 1));
    assert_eq!(limit_error(same(&stream, Some(0), LOAD_NODES)), Some(0));
}

/// More than a batch (1 MiB) of members: the batches build and skip the same members from a
/// shared budget as one parse of the whole stream, whatever budget is left, and a stream refused
/// for its size after some batches were built gives their nodes back.
#[test]
fn members_parsed_in_batches_spend_the_budget_as_one_parse() {
    let mut members = Vec::new();
    let mut pairs = Vec::new();
    for i in 0..600_u32 {
        pairs.push((i + 1, members.len()));
        if i % 3 == 0 {
            members.extend_from_slice(format!("[{}]\n", "1 ".repeat(1500)).as_bytes());
        } else {
            members.extend_from_slice(format!("({})\n", "x".repeat(4000)).as_bytes());
        }
    }
    let (stream, size) = flate(&pairs, &members);
    assert!(size > 2 << 20, "{size} bytes make several batches");
    for nodes in [0, 1, 1_500, 1_501, 3_100, 100_000, 300_000, 301_000, LOAD_NODES] {
        same(&stream, Some(size), nodes).unwrap();
    }
    let all = same(&stream, None, LOAD_NODES).unwrap();
    assert_eq!(all.objects.len(), 600);

    let mut over = members.clone();
    over.extend(std::iter::repeat_n(b' ', 4 << 20));
    let (stream, size) = flate(&pairs, &over);
    let budget = NodeBudget::new(400_000);
    assert_eq!(limit_error(ObjectStream::new_within(&stream, Some(size - 1), &budget)), Some(size - 1));
    assert_eq!(budget.left(), 400_000, "a refused stream spends nothing");
}

/// With a limit, the stream is refused for its size before anything wrong with its dictionary or
/// index is reported, as the whole-stream path decodes before it reads them.
#[test]
fn the_limit_is_checked_before_the_dictionary_and_the_index() {
    let (mut stream, size) = with_long_tail(1 << 20);
    stream.dict.remove(b"First");
    assert_eq!(limit_error(same(&stream, Some(size - 1), LOAD_NODES)), Some(size - 1));
    assert!(matches!(same(&stream, Some(size), LOAD_NODES), Err(Error::DictKey(_))));
    stream.dict.set("First", -1);
    assert!(matches!(same(&stream, Some(size), LOAD_NODES), Err(Error::NumericCast(_))));
    stream.dict.set("First", 7);
    stream.dict.remove(b"N");
    assert_eq!(limit_error(same(&stream, Some(size - 1), LOAD_NODES)), Some(size - 1));
    assert!(matches!(same(&stream, Some(size), LOAD_NODES), Err(Error::DictKey(_))));
}

/// A `/First` far past the data is refused without allocating it: the index grows only with the
/// bytes decoded.
#[test]
fn a_first_offset_past_the_data_is_refused_without_allocating_it() {
    let (mut stream, _) = flate(&[(1, 0)], b"1");
    stream.dict.set("First", i64::from(u32::MAX) << 8);
    assert!(matches!(same(&stream, None, LOAD_NODES), Err(Error::InvalidOffset(_))));
}

#[test]
fn a_corrupt_zlib_header_is_read_past() {
    let (content, first) = decoded(&[(7, 0)], b"(hello)");
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&content).unwrap();
    let mut encoded = vec![0, 0];
    encoded.extend(encoder.finish().unwrap());
    let loaded = same(&object_stream(encoded, first, 1, true), Some(4096), LOAD_NODES).unwrap();
    assert_eq!(loaded.objects.get(&(7, 0)).unwrap().as_str().unwrap(), b"hello");
}

/// Intact data decodes as `Stream::decompressed_content` decodes it. A wrong Adler-32 checksum
/// keeps every member (the whole-stream path can lose the last of the data then), and data cut
/// anywhere keeps the members decoded before the cut.
#[test]
fn a_corrupt_or_truncated_tail_keeps_the_members_before_it() {
    let mut members = b"(hello) ".to_vec();
    members.extend(std::iter::repeat_n(b'x', 1000).chain(*b"\n(bye)"));
    let (content, first) = decoded(&[(7, 0), (8, 1009)], &members);
    let encoded = zlib(&content);
    assert_eq!(inflated(&encoded), content);
    assert_eq!(object_stream(encoded.clone(), first, 2, true).decompressed_content().unwrap(), content);
    let mut checksum = encoded.clone();
    *checksum.last_mut().unwrap() ^= 0x80;
    assert_eq!(same(&object_stream(checksum, first, 2, true), Some(4096), LOAD_NODES).unwrap().objects.len(), 2);
    let mut kept = 0;
    for cut in 0..encoded.len() {
        // Cut inside the index, a stream is refused as `InvalidOffset` either way.
        let loaded = same(&object_stream(encoded[..cut].to_vec(), first, 2, true), Some(4096), LOAD_NODES);
        kept = kept.max(loaded.map_or(0, |loaded| loaded.objects.len()));
    }
    assert_eq!(kept, 2, "without its checksum the data still holds both members");
}

/// Offsets out of order, repeated offsets (the first entry wins), a repeated object number (its
/// last entry wins), members bounded by the next distinct offset, and offsets past the data.
#[test]
fn index_quirks_read_as_in_the_whole_stream_path() {
    let members = b" \n42\n /Name \n[true 7]\n << /Key (value) >>\n (last)";
    let (stream, _) = flate(&[(9, 0), (12, 21), (10, 5), (11, 12), (9, 41)], members);
    let loaded = same(&stream, None, LOAD_NODES).unwrap();
    assert_eq!(loaded.objects.len(), 4);
    assert_eq!(loaded.objects.get(&(9, 0)).unwrap().as_str().unwrap(), b"last");

    let members = b"(a) (b) [(c) (d)] 5 (e)";
    let (stream, size) = flate(&[(1, 0), (2, 0), (3, 4), (1, 8), (4, 18), (5, 20), (6, 400), (7, 400)], members);
    let loaded = same(&stream, Some(size), LOAD_NODES).unwrap();
    assert_eq!(loaded.objects.get(&(1, 0)).unwrap().as_array().unwrap().len(), 2);
    assert!(!loaded.objects.contains_key(&(2, 0)) && !loaded.objects.contains_key(&(6, 0)));
}

/// A small deterministic generator, so a failing case can be found again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const MEMBERS: &[&[u8]] = &[
    b"42",
    b"-3.5",
    b"/Name",
    b"(str)",
    b"(nested (paren) \\) )",
    b"[1 2 [3 /x]]",
    b"<< /K [1 2] /V (x) /D << /E null >> >>",
    b"true",
    b"null",
    b"<48656C6C6F>",
    b"1 0 R",
    b"[1 2",
    b"<< /A",
    b"(unterminated",
    b")",
    b"",
    b"%comment\n7",
];
const SPACES: &[&[u8]] = &[b"", b" ", b"\n", b"\r\n  ", b"\t\x0c", b"\0"];

/// A random Flate object stream, often malformed, and whether its compressed data is intact.
fn random_stream(rng: &mut Rng) -> (Stream, bool) {
    let mut members = Vec::new();
    let mut pairs = Vec::new();
    for _ in 0..rng.below(12) {
        members.extend_from_slice(rng.pick(SPACES));
        let offset = members.len();
        if rng.one_in(8) {
            // A member a tight budget cannot build.
            members.push(b'[');
            members.extend(std::iter::repeat_n(b"0 ".as_slice(), rng.below(40)).flatten());
            members.push(b']');
        } else {
            members.extend_from_slice(rng.pick(MEMBERS));
        }
        pairs.push((rng.below(20) as u32, offset));
        if rng.one_in(10) {
            pairs.push((rng.below(20) as u32, offset));
        }
        if rng.one_in(10) {
            members.extend(std::iter::repeat_n(b' ', rng.below(64)));
        }
    }
    if rng.one_in(10) {
        pairs.push((99, members.len() + rng.below(10)));
    }
    if rng.one_in(12) && pairs.len() > 1 {
        let (a, b) = (rng.below(pairs.len()), rng.below(pairs.len()));
        pairs.swap(a, b);
    }
    let mut index = String::new();
    for &(id, offset) in &pairs {
        index.push_str(&format!("{id} {offset} "));
        if rng.one_in(30) {
            index.push_str("x ");
        }
    }
    if rng.one_in(20) {
        index.push_str("17");
    }
    let first = index.len();
    let mut content = index.into_bytes();
    content.extend_from_slice(&members);
    if rng.one_in(10) {
        content.extend(std::iter::repeat_n(b' ', rng.below(5_000)));
    }
    let mut encoded = zlib(&content);
    let intact = rng.below(14);
    match intact {
        0 => encoded.truncate(rng.below(encoded.len() + 1)),
        1 => {
            let at = rng.below(encoded.len());
            encoded[at] ^= 1 << rng.below(8);
        }
        2 => encoded[..2].fill(0),
        3 => encoded = content.clone(),
        _ => {}
    }
    let mut stream = object_stream(encoded, first, pairs.len(), true);
    match rng.below(20) {
        0 => drop(stream.dict.remove(b"First")),
        1 => stream.dict.set("First", (first + rng.below(8)) as i64 - 4),
        2 => drop(stream.dict.remove(b"N")),
        3 => stream.dict.set("N", rng.below(20) as i64),
        4 => stream.dict.set("First", (content.len() + rng.below(4)) as i64),
        _ => {}
    }
    (stream, intact > 3)
}

/// Random streams, malformed in their members, index, dictionary or data, under every limit
/// around their decoded size and budgets that run out part-way.
#[test]
fn random_flate_streams_load_as_their_decoded_bytes_do() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut loaded, mut refused) = (0, 0);
    let cases = 3_000;
    for case in 0..cases {
        let (stream, intact) = random_stream(&mut rng);
        let size = stream.decompressed_content().map_or(0, |content| content.len());
        if intact {
            // The reference decodes intact data as the whole-stream path's decoder does.
            assert_eq!(inflated(&stream.content), stream.decompressed_content().unwrap(), "case {case}");
        }
        let limit = match rng.below(6) {
            0 => None,
            1 => Some(size),
            2 => Some(size.saturating_sub(1)),
            3 => Some(rng.below(size + 2)),
            4 => Some(0),
            _ => Some(256 << 20),
        };
        let nodes = if rng.one_in(3) { rng.below(40) } else { LOAD_NODES };
        match std::panic::catch_unwind(|| same(&stream, limit, nodes)) {
            Ok(Ok(_)) => loaded += 1,
            Ok(Err(_)) => refused += 1,
            Err(panic) => panic!("case {case}: {:?}", panic.downcast_ref::<String>()),
        }
    }
    // The generator reaches both outcomes often, so the comparison is not vacuous.
    assert!(loaded > cases / 2, "{loaded} of {cases} loaded");
    assert!(refused > cases / 20, "{refused} of {cases} refused");
}

/// A PDF with a cross-reference stream whose Info dictionary (object 4) is the second member of
/// object stream 3, after `before` (object 5) and followed by `tail` bytes of whitespace.
fn info_in_object_stream_pdf(before: &[u8], tail: usize, flate: bool) -> (Vec<u8>, usize) {
    let mut members = before.to_vec();
    members.push(b' ');
    let info_at = members.len();
    members.extend_from_slice(b"<< /Title (Kept) >>");
    members.extend(std::iter::repeat_n(b' ', tail));
    let (content, first) = decoded(&[(5, 0), (4, info_at)], &members);
    let size = content.len();
    let data = if flate { zlib(&content) } else { content };
    let filter = if flate { "/Filter /FlateDecode " } else { "" };

    let mut pdf = b"%PDF-1.5\n".to_vec();
    let mut offsets = [0_u32; 7];
    let mut append = |pdf: &mut Vec<u8>, id: usize, head: &[u8], body: &[u8], tail: &[u8]| {
        offsets[id] = pdf.len() as u32;
        pdf.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        pdf.extend_from_slice(head);
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(tail);
        pdf.extend_from_slice(b"\nendobj\n");
    };
    append(&mut pdf, 1, b"<< /Type /Catalog /Pages 2 0 R >>", b"", b"");
    append(&mut pdf, 2, b"<< /Type /Pages /Kids [] /Count 0 >>", b"", b"");
    let head = format!("<< /Type /ObjStm {filter}/N 2 /First {first} /Length {} >>\nstream\n", data.len());
    append(&mut pdf, 3, head.as_bytes(), &data, b"\nendstream");
    let xref_offset = pdf.len() as u32;
    offsets[6] = xref_offset;
    let mut records = Vec::new();
    for (id, &offset) in offsets.iter().enumerate() {
        let (kind, field, index) = match id {
            0 => (0_u8, 0_u32, 0xffff_u16),
            4 => (2, 3, 1),
            5 => (2, 3, 0),
            _ => (1, offset, 0),
        };
        records.push(kind);
        records.extend_from_slice(&field.to_be_bytes());
        records.extend_from_slice(&index.to_be_bytes());
    }
    let head = format!("<< /Type /XRef /Size 7 /Root 1 0 R /Info 4 0 R /W [1 4 2] /Length {} >>\nstream\n", records.len());
    pdf.extend_from_slice(format!("6 0 obj\n{head}").as_bytes());
    pdf.extend_from_slice(&records);
    pdf.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes());
    (pdf, size)
}

fn limited(limit: usize) -> LoadOptions {
    LoadOptions { max_decompressed_size: Some(limit), ..LoadOptions::default() }
}

fn info_title(doc: &Document) -> Option<Vec<u8>> {
    let info = doc.get_object((4, 0)).ok()?.as_dict().ok()?;
    Some(info.get(b"Title").ok()?.as_str().ok()?.to_vec())
}

/// End to end: a document load and a metadata lookup read a Flate object stream's members as they
/// read the same stream unfiltered, and the load refuses (drops) the stream when its decoded
/// bytes, most of them whitespace after the last member, exceed the limit by one.
#[test]
fn documents_read_flate_object_streams_as_unfiltered_ones() {
    for (before, tail) in [(b"[1 2 3]".as_slice(), 0), (b"(x)".as_slice(), 3 << 20)] {
        let (flated, size) = info_in_object_stream_pdf(before, tail, true);
        let (plain, _) = info_in_object_stream_pdf(before, tail, false);
        for pdf in [&flated, &plain] {
            let doc = Document::load_mem_with_options(pdf, limited(size)).unwrap();
            assert_eq!(info_title(&doc).as_deref(), Some(b"Kept".as_slice()));
            assert!(doc.get_object((5, 0)).is_ok());
            let metadata = Document::load_metadata_mem(pdf).unwrap();
            assert_eq!(metadata.title.as_deref(), Some("Kept"));
            let doc = Document::load_mem_with_options(pdf, limited(size - 1)).unwrap();
            assert_eq!(info_title(&doc), None, "a stream over the limit is dropped");
            assert!(doc.get_object((5, 0)).is_err());
        }
    }
}
