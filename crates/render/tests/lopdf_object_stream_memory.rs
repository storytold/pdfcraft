//! Vendored lopdf patch (8): reading a `/FlateDecode` object stream no longer holds its whole
//! decoded content. Alone in its test binary, so no other test moves the process's peak memory.

#![cfg(target_os = "linux")]

use flate2::Compression;
use flate2::write::ZlibEncoder;
use lopdf::{Document, NodeBudget, ObjectStream, Stream};
use std::io::Write;

/// The process's peak resident memory in bytes (`VmHWM`).
fn peak_resident() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|line| line.starts_with("VmHWM:")).unwrap();
    let kib: usize = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kib << 10
}

/// Compress the decoded object-stream content `index` then `members`, without holding `members`
/// whole: each call of `members` writes one piece.
fn compress(index: &[u8], members: impl Iterator<Item = Vec<u8>>) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(index).unwrap();
    for member in members {
        encoder.write_all(&member).unwrap();
    }
    encoder.finish().unwrap()
}

fn object_stream(index: &[u8], n: usize, encoded: Vec<u8>) -> Stream {
    let mut dict = lopdf::Dictionary::new();
    dict.set("Type", "ObjStm");
    dict.set("Filter", "FlateDecode");
    dict.set("N", n as i64);
    dict.set("First", index.len() as i64);
    Stream::new(dict, encoded)
}

/// Run `load` and return how much it raised the process's peak memory above its memory before,
/// in MiB. The peak is reset first (`clear_refs`, Linux 4.0), so an earlier peak hides nothing.
/// Where a sandbox refuses the reset, an earlier peak can only make the growth read smaller, so
/// the check may pass when it should not, but never fails falsely.
fn peak_growth(load: impl FnOnce()) -> usize {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    let before = peak_resident();
    load();
    peak_resident().saturating_sub(before) >> 20
}

/// A full load of 128 MiB of members, each an array of two integers padded with spaces: the
/// members are decoded and parsed a batch (1 MiB) at a time. Decoding the stream whole first held
/// all 128 MiB, and up to twice that while its buffer grew.
fn full_load() {
    const MEMBER: usize = 4 << 10;
    const COUNT: usize = 32 << 10;
    let member = |i: usize| {
        let mut bytes = format!("[{i}").into_bytes();
        bytes.resize(MEMBER - 2, b' ');
        bytes.extend_from_slice(b"7]");
        bytes
    };
    let index: String = (0..COUNT).map(|i| format!("{} {} ", i + 1, i * MEMBER)).collect();
    let stream = object_stream(index.as_bytes(), COUNT, compress(index.as_bytes(), (0..COUNT).map(member)));
    let mut loaded = None;
    let grew = peak_growth(|| {
        loaded = Some(ObjectStream::new_within(&stream, Some(256 << 20), &NodeBudget::new(1 << 23)).unwrap());
    });
    let loaded = loaded.unwrap();
    assert_eq!(loaded.objects.len(), COUNT);
    assert_eq!(loaded.objects.get(&(COUNT as u32, 0)).unwrap().as_array().unwrap().len(), 2);
    assert!(grew < 64, "a full load of {} MiB of members raised peak memory by {grew} MiB", (COUNT * MEMBER) >> 20);
}

/// A PDF whose Info dictionary (object 4) is the first member of an object stream, before a
/// 192 MiB string (object 5) that a lookup of the Info dictionary skips.
fn info_before_a_large_member_pdf() -> Vec<u8> {
    let info = b"<< /Title (Kept) >> ".to_vec();
    let index = format!("4 0 5 {} ", info.len());
    let string = (0..192).map(|_| vec![b'x'; 1 << 20]);
    let members = std::iter::once(info).chain(std::iter::once(b"(".to_vec())).chain(string).chain([b")".to_vec()]);
    let data = compress(index.as_bytes(), members);

    let mut pdf = b"%PDF-1.5\n".to_vec();
    let mut offsets = [0_u32; 7];
    for (id, body) in [(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()), (2, b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec())] {
        offsets[id] = pdf.len() as u32;
        pdf.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        pdf.extend_from_slice(&body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    offsets[3] = pdf.len() as u32;
    let head = format!("3 0 obj\n<< /Type /ObjStm /Filter /FlateDecode /N 2 /First {} /Length {} >>\nstream\n", index.len(), data.len());
    pdf.extend_from_slice(head.as_bytes());
    pdf.extend_from_slice(&data);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    let xref_offset = pdf.len() as u32;
    offsets[6] = xref_offset;
    let mut records = Vec::new();
    for (id, &offset) in offsets.iter().enumerate() {
        let (kind, field, index) = match id {
            0 => (0_u8, 0_u32, 0xffff_u16),
            4 => (2, 3, 0),
            5 => (2, 3, 1),
            _ => (1, offset, 0),
        };
        records.push(kind);
        records.extend_from_slice(&field.to_be_bytes());
        records.extend_from_slice(&index.to_be_bytes());
    }
    let head = format!("6 0 obj\n<< /Type /XRef /Size 7 /Root 1 0 R /Info 4 0 R /W [1 4 2] /Length {} >>\nstream\n", records.len());
    pdf.extend_from_slice(head.as_bytes());
    pdf.extend_from_slice(&records);
    pdf.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes());
    pdf
}

/// A lookup of one member (here the Info dictionary that lopdf's metadata reader wants) keeps
/// only that member's bytes, and decodes nothing after it when no limit asks for the rest.
fn lookup() {
    let pdf = info_before_a_large_member_pdf();
    let mut title = None;
    let grew = peak_growth(|| title = Document::load_metadata_mem(&pdf).unwrap().title);
    assert_eq!(title.as_deref(), Some("Kept"));
    assert!(grew < 32, "looking up a 20-byte member raised peak memory by {grew} MiB");
}

/// Both cases in one test, one after the other: each measures the growth of a peak the earlier
/// one may already have raised, so the lookup, the smaller, runs first.
#[test]
fn reading_flate_object_streams_does_not_hold_their_decoded_bytes() {
    lookup();
    full_load();
}
