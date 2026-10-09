//! Serialization and saving (ISO 32000-2 §7.5).
//!
//! * `write_incremental` appends the edited objects, a cross-reference section in the same style
//!   as the file's last one (table or stream) and a trailer with `/Prev`. The original bytes are
//!   never modified, which keeps digital signatures over earlier revisions valid.
//! * `write_full` writes only objects reachable from the trailer, renumbered densely, with a
//!   classic cross-reference table — the "Save As" / garbage-collecting path.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;

use crate::CosError;
use crate::document::Document;
use crate::object::{Dict, ObjRef, Object, PdfString, Stream};

/// Serialize one object. Deterministic: same object → same bytes.
pub fn serialize(o: &Object, out: &mut Vec<u8>) {
    match o {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Object::Real(r) => write_real(*r, out),
        Object::String(s) => write_string(s, out),
        Object::Name(n) => write_name(n, out),
        Object::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                serialize(x, out);
            }
            out.push(b']');
        }
        Object::Dict(d) => write_dict(d, out),
        Object::Stream(s) => write_stream(s, out),
        Object::Ref(r) => {
            let _ = write!(out, "{} {} R", r.num, r.generation);
        }
    }
}

fn write_real(r: f64, out: &mut Vec<u8>) {
    if !r.is_finite() {
        out.push(b'0');
        return;
    }
    if r.fract() == 0.0 && r.abs() < 1e15 {
        let _ = write!(out, "{}", r as i64);
        return;
    }
    // Plain decimal notation (§7.3.3 has no exponents). Rust's `Display` for f64 prints the
    // shortest string that parses back to the same value, never in exponent form, so values
    // round-trip exactly (rounding to a fixed number of places visibly shifted shading colours).
    let s = format!("{r}");
    out.extend_from_slice(if s == "-0" { b"0" } else { s.as_bytes() });
}

fn write_name(n: &[u8], out: &mut Vec<u8>) {
    out.push(b'/');
    for &b in n {
        let regular = (b'!'..=b'~').contains(&b) && !matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' | b'#');
        if regular {
            out.push(b);
        } else {
            let _ = write!(out, "#{b:02X}");
        }
    }
}

fn write_string(s: &PdfString, out: &mut Vec<u8>) {
    let binary = s.bytes.iter().filter(|b| !(b' '..=b'~').contains(*b) && !matches!(b, b'\n' | b'\r' | b'\t')).count();
    if s.hex || binary * 4 > s.bytes.len().max(1) {
        out.push(b'<');
        for b in &s.bytes {
            let _ = write!(out, "{b:02X}");
        }
        out.push(b'>');
        return;
    }
    out.push(b'(');
    for &b in &s.bytes {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0C => out.extend_from_slice(b"\\f"),
            b' '..=b'~' => out.push(b),
            _ => {
                let _ = write!(out, "\\{b:03o}");
            }
        }
    }
    out.push(b')');
}

fn write_dict(d: &Dict, out: &mut Vec<u8>) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        write_name(k, out);
        out.push(b' ');
        serialize(v, out);
    }
    out.extend_from_slice(b">>");
}

fn write_stream(s: &Stream, out: &mut Vec<u8>) {
    let mut d = s.dict.clone();
    d.set(b"Length".to_vec(), Object::Int(s.raw.len() as i64));
    write_dict(&d, out);
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(&s.raw);
    out.extend_from_slice(b"\nendstream");
}

/// The object as written: encrypted with its (output) number when the document is encrypted,
/// except the `/Encrypt` dictionary itself (`source_num` is its number in the source document).
fn prepared(doc: &Document, source_num: u32, num: u32, generation: u16, o: &Object) -> Object {
    match doc.output_security() {
        (Some(h), encrypt_num) if Some(source_num) != encrypt_num => crate::security::transform(h, o, num, generation, false),
        _ => o.clone(),
    }
}

fn write_indirect(num: u32, generation: u16, o: &Object, out: &mut Vec<u8>) {
    let _ = writeln!(out, "{num} {generation} obj");
    serialize(o, out);
    out.extend_from_slice(b"\nendobj\n");
}

/// Options for saving.
#[derive(Clone, Debug)]
pub struct SaveOptions {
    /// Replace or add `/Info /ModDate` with this PDF date string (e.g. from `pdf_date`).
    pub mod_date: Option<String>,
    /// Seed for a new file identifier when the document has none (deterministic in tests).
    pub id_seed: u64,
    /// Full saves: pack non-stream objects into compressed object streams with a cross-reference
    /// stream (PDF 1.5+, §7.5.7–7.5.8). Off writes a classic table that any reader accepts.
    pub object_streams: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self { mod_date: None, id_seed: 0x5052_494E_5443_5241, object_streams: true }
    }
}

/// Objects per object stream (Acrobat and qpdf use 100–200).
const OBJSTM_SIZE: usize = 100;

/// Where an object lives, for the cross-reference section.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    /// Free, with the generation the number gets when reused.
    Free(u16),
    /// At a byte offset.
    InFile(u64, u16),
    /// Inside object stream `stream`, at position `index`.
    InStream(u32, u32),
}

/// Append the document's edits to its original bytes (incremental update, §7.5.6).
///
/// A document that had to be reconstructed (no usable cross-reference chain) cannot be updated
/// incrementally — other readers would follow the broken chain — so it is rewritten in full,
/// which also repairs the file (what users expect after "the file was damaged and repaired").
pub fn write_incremental(doc: &Document, opts: &SaveOptions) -> Result<Vec<u8>, CosError> {
    // Reconstructed files have no chain to append to; added or removed encryption must
    // rewrite every object; redaction must not leave the old revision behind. All need a full save.
    if doc.revisions().is_empty() || doc.encryption_changed() || doc.full_save_required() {
        return write_full(doc, opts);
    }
    let mut doc = doc.clone();
    stamp_mod_date(&mut doc, opts);
    let original = doc.bytes().clone();
    // The revision is written on its own and joined to the original at the end, so the result is
    // allocated once at its exact size: growing a buffer that holds a large original past a
    // guessed capacity would copy the whole file again and keep up to as much again unused.
    let newline = !matches!(original.last(), Some(b'\n' | b'\r'));
    let base = (original.len() + usize::from(newline)) as u64;
    let mut out = Vec::new();
    let mut offsets: BTreeMap<u32, (u64, u16, bool)> = BTreeMap::new(); // num → (offset, gen, in use)
    for (num, generation, obj) in doc.overlay_entries().collect::<Vec<_>>() {
        match obj {
            Some(o) => {
                offsets.insert(num, (base + out.len() as u64, generation, true));
                write_indirect(num, generation, &prepared(&doc, num, num, generation, &o), &mut out);
            }
            None => {
                offsets.insert(num, (0, generation, false));
            }
        }
    }
    let prev = doc.revisions().last().map(|r| r.xref_offset);
    let as_stream = doc.revisions().last().is_some_and(|r| r.is_stream);
    let mut trailer = doc.trailer().clone();
    if doc.output_security().0.is_none() {
        ensure_id(&mut trailer, opts, &original); // an encrypted file's ID is part of its key
    }
    if let Some(p) = prev {
        trailer.set(b"Prev".to_vec(), Object::Int(p as i64));
    }
    let size = doc.next_num().max(trailer.int(b"Size").unwrap_or(0) as u32);
    if as_stream {
        let rows = offsets.iter().map(|(n, (o, g, used))| (*n, if *used { Row::InFile(*o, *g) } else { Row::Free(*g) })).collect();
        write_xref_stream(&mut out, base, &mut trailer, rows, size);
    } else {
        trailer.set(b"Size".to_vec(), Object::Int(size as i64));
        let xref_at = base + out.len() as u64;
        write_xref_table(&mut out, &offsets, prev.is_none());
        out.extend_from_slice(b"trailer\n");
        write_dict(&trailer, &mut out);
        let _ = write!(out, "\nstartxref\n{xref_at}\n%%EOF\n");
    }
    let mut file = Vec::with_capacity(original.len() + usize::from(newline) + out.len());
    file.extend_from_slice(&original);
    if newline {
        file.push(b'\n');
    }
    file.extend_from_slice(&out);
    Ok(file)
}

/// Write only reachable objects, renumbered from 1, with a classic cross-reference table.
pub fn write_full(doc: &Document, opts: &SaveOptions) -> Result<Vec<u8>, CosError> {
    let mut doc = doc.clone();
    stamp_mod_date(&mut doc, opts);
    let trailer_in = doc.trailer().clone();
    // Breadth-first walk from the trailer, assigning new numbers in visit order.
    let mut map: HashMap<ObjRef, u32> = HashMap::new();
    let mut order: Vec<ObjRef> = Vec::new();
    let mut queue: VecDeque<ObjRef> = VecDeque::new();
    let visit_refs = |o: &Object, queue: &mut VecDeque<ObjRef>| collect_refs(o, &mut |r| queue.push_back(r));
    for key in [&b"Root"[..], b"Info", b"Encrypt"] {
        if let Some(r) = trailer_in.reference(key) {
            queue.push_back(r);
        }
    }
    while let Some(r) = queue.pop_front() {
        if map.contains_key(&r) {
            continue;
        }
        let o = doc.get(r);
        if matches!(*o, Object::Null) {
            continue; // dangling reference: written as null by renumbering below
        }
        map.insert(r, (order.len() + 1) as u32);
        order.push(r);
        visit_refs(&o, &mut queue);
    }
    let version = doc.version().chars().take(3).collect::<String>();
    let version = if opts.object_streams && version.as_str() < "1.5" { "1.5".to_string() } else { version };
    let mut out = Vec::new();
    let _ = writeln!(out, "%PDF-{version}");
    // A binary comment (raw high bytes) marks the file as binary for transfer tools.
    out.extend_from_slice(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);
    // The /Encrypt dictionary and everything it references (e.g. indirect /CF crypt-filter
    // dictionaries) are needed before anything can be decrypted: never inside object streams.
    let mut encryption_objects: std::collections::HashSet<ObjRef> = std::collections::HashSet::new();
    let mut pending: Vec<ObjRef> = trailer_in.reference(b"Encrypt").into_iter().collect();
    while let Some(r) = pending.pop() {
        if encryption_objects.insert(r) {
            collect_refs(&doc.get(r), &mut |x| pending.push(x));
        }
    }
    let mut rows: BTreeMap<u32, Row> = BTreeMap::new();
    let mut packed: Vec<(u32, Object)> = Vec::new(); // objects bound for object streams
    for (i, r) in order.iter().enumerate() {
        let num = (i + 1) as u32;
        let o = renumber(&doc.get(*r), &map);
        let in_stream = opts.object_streams && !matches!(o, Object::Stream(_)) && !encryption_objects.contains(r);
        if in_stream {
            packed.push((num, o));
        } else {
            rows.insert(num, Row::InFile(out.len() as u64, 0));
            write_indirect(num, 0, &prepared(&doc, r.num, num, 0, &o), &mut out);
        }
    }
    // Object streams take the numbers after the last object. Their contents are not encrypted
    // individually: the whole stream is (§7.5.7, §7.6.2).
    let mut next = order.len() as u32 + 1;
    for chunk in packed.chunks(OBJSTM_SIZE) {
        let stm_num = next;
        next += 1;
        let mut header = Vec::new();
        let mut body = Vec::new();
        for (index, (num, o)) in chunk.iter().enumerate() {
            let _ = write!(header, "{} {} ", num, body.len());
            serialize(o, &mut body);
            body.push(b'\n');
            rows.insert(*num, Row::InStream(stm_num, index as u32));
        }
        let first = header.len();
        header.extend_from_slice(&body);
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("ObjStm"));
        d.set(b"N".to_vec(), Object::Int(chunk.len() as i64));
        d.set(b"First".to_vec(), Object::Int(first as i64));
        let stm = Object::Stream(Stream::flate(d, &header));
        rows.insert(stm_num, Row::InFile(out.len() as u64, 0));
        write_indirect(stm_num, 0, &prepared(&doc, u32::MAX, stm_num, 0, &stm), &mut out);
    }
    let mut trailer = Dict::new();
    for key in [&b"Root"[..], b"Info", b"Encrypt"] {
        if let Some(r) = trailer_in.reference(key).and_then(|r| map.get(&r)) {
            trailer.set(key.to_vec(), Object::Ref(ObjRef::new(*r, 0)));
        }
    }
    if let Some(Object::Dict(d)) = trailer_in.get(b"Encrypt") {
        trailer.set(b"Encrypt".to_vec(), Object::Dict(d.clone()));
    }
    if let Some(id) = trailer_in.get(b"ID") {
        trailer.set(b"ID".to_vec(), id.clone());
    }
    if doc.output_security().0.is_none() {
        ensure_id(&mut trailer, opts, doc.bytes());
    }
    if opts.object_streams {
        write_xref_stream(&mut out, 0, &mut trailer, rows, next);
    } else {
        trailer.set(b"Size".to_vec(), Object::Int(next as i64));
        let offsets: BTreeMap<u32, (u64, u16, bool)> =
            rows.iter().filter_map(|(n, r)| if let Row::InFile(o, g) = r { Some((*n, (*o, *g, true))) } else { None }).collect();
        let xref_at = out.len();
        write_xref_table(&mut out, &offsets, true);
        out.extend_from_slice(b"trailer\n");
        write_dict(&trailer, &mut out);
        let _ = write!(out, "\nstartxref\n{xref_at}\n%%EOF\n");
    }
    Ok(out)
}

fn collect_refs(o: &Object, f: &mut impl FnMut(ObjRef)) {
    match o {
        Object::Ref(r) => f(*r),
        Object::Array(a) => a.iter().for_each(|x| collect_refs(x, f)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| collect_refs(v, f)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| collect_refs(v, f)),
        _ => {}
    }
}

fn renumber(o: &Object, map: &HashMap<ObjRef, u32>) -> Object {
    match o {
        Object::Ref(r) => map.get(r).map(|n| Object::Ref(ObjRef::new(*n, 0))).unwrap_or(Object::Null),
        Object::Array(a) => Object::Array(a.iter().map(|x| renumber(x, map)).collect()),
        Object::Dict(d) => Object::Dict(d.iter().map(|(k, v)| (k.clone(), renumber(v, map))).collect()),
        Object::Stream(s) => Object::Stream(Stream { dict: s.dict.iter().map(|(k, v)| (k.clone(), renumber(v, map))).collect(), raw: s.raw.clone() }),
        other => other.clone(),
    }
}

/// Classic xref table with one subsection per run of consecutive numbers (§7.5.4).
fn write_xref_table(out: &mut Vec<u8>, offsets: &BTreeMap<u32, (u64, u16, bool)>, include_zero: bool) {
    out.extend_from_slice(b"xref\n");
    let mut all: Vec<(u32, (u64, u16, bool))> = offsets.iter().map(|(k, v)| (*k, *v)).collect();
    if include_zero {
        all.insert(0, (0, (0, 65535, false)));
    }
    let mut i = 0;
    while i < all.len() {
        let start = all[i].0;
        let mut j = i;
        while j + 1 < all.len() && all[j + 1].0 == all[j].0 + 1 {
            j += 1;
        }
        let _ = writeln!(out, "{} {}", start, j - i + 1);
        for (_, (off, generation, used)) in &all[i..=j] {
            let _ = writeln!(out, "{:010} {:05} {} ", off, generation, if *used { 'n' } else { 'f' });
        }
        i = j + 1;
    }
}

/// A cross-reference stream section (§7.5.8) that takes object number `num`, ending the file.
/// `out` holds the file from byte offset `base` on. Rows are Flate-compressed with the PNG Up
/// predictor, and field widths fit the largest value.
fn write_xref_stream(out: &mut Vec<u8>, base: u64, trailer: &mut Dict, mut rows: BTreeMap<u32, Row>, num: u32) {
    let xref_at = base + out.len() as u64;
    rows.insert(num, Row::InFile(xref_at, 0));
    if trailer.get(b"Prev").is_none() {
        rows.entry(0).or_insert(Row::Free(65535));
    }
    let fields = |r: &Row| match *r {
        Row::Free(g) => (0u64, 0u64, u64::from(g)),
        Row::InFile(o, g) => (1, o, u64::from(g)),
        Row::InStream(s, i) => (2, u64::from(s), u64::from(i)),
    };
    let width = |v: u64| ((64 - v.leading_zeros()).div_ceil(8)).max(1) as usize;
    let (w2, w3) = rows.values().map(fields).fold((1, 1), |(a, b), (_, x, y)| (width(x).max(a), width(y).max(b)));
    let cols = 1 + w2 + w3;
    let mut index = Vec::new();
    let nums: Vec<u32> = rows.keys().copied().collect();
    let mut i = 0;
    while i < nums.len() {
        let mut j = i;
        while j + 1 < nums.len() && nums[j + 1] == nums[j] + 1 {
            j += 1;
        }
        index.push(Object::Int(nums[i] as i64));
        index.push(Object::Int((j - i + 1) as i64));
        i = j + 1;
    }
    let mut data = Vec::with_capacity(rows.len() * (cols + 1));
    let mut prev = vec![0u8; cols];
    for r in rows.values() {
        let (t, a, b) = fields(r);
        let mut row = vec![t as u8];
        row.extend_from_slice(&a.to_be_bytes()[8 - w2..]);
        row.extend_from_slice(&b.to_be_bytes()[8 - w3..]);
        data.push(2); // PNG "Up"
        data.extend(row.iter().zip(&prev).map(|(c, p)| c.wrapping_sub(*p)));
        prev = row;
    }
    let mut d = trailer.clone();
    d.set(b"Type".to_vec(), Object::name("XRef"));
    d.set(b"Size".to_vec(), Object::Int(num as i64 + 1));
    d.set(b"W".to_vec(), Object::Array(vec![Object::Int(1), Object::Int(w2 as i64), Object::Int(w3 as i64)]));
    d.set(b"Index".to_vec(), Object::Array(index));
    let mut stream = Stream::flate(d, &data);
    let mut parms = Dict::new();
    parms.set(b"Predictor".to_vec(), Object::Int(12));
    parms.set(b"Columns".to_vec(), Object::Int(cols as i64));
    stream.dict.set(b"DecodeParms".to_vec(), Object::Dict(parms));
    write_indirect(num, 0, &Object::Stream(stream), out);
    let _ = write!(out, "startxref\n{xref_at}\n%%EOF\n");
    trailer.set(b"Size".to_vec(), Object::Int(num as i64 + 1));
}

fn stamp_mod_date(doc: &mut Document, opts: &SaveOptions) {
    let Some(date) = &opts.mod_date else { return };
    let date = Object::String(PdfString::literal(date.as_bytes().to_vec()));
    match doc.trailer().get(b"Info").cloned() {
        Some(Object::Ref(r)) => {
            let _ = doc.update_dict(r, |d| d.set(b"ModDate".to_vec(), date.clone()));
        }
        _ => {
            let mut info = Dict::new();
            info.set(b"ModDate".to_vec(), date);
            let r = doc.add(info);
            doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
        }
    }
}

/// Keep an existing `/ID`, or create one (two equal 16-byte strings, §14.4).
fn ensure_id(trailer: &mut Dict, opts: &SaveOptions, content: &[u8]) {
    if matches!(trailer.get(b"ID"), Some(Object::Array(a)) if a.len() == 2) {
        return;
    }
    // FNV-1a over a sample of the content and the seed: stable, dependency-free.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ opts.id_seed;
    for b in content.iter().step_by((content.len() / 4096).max(1)).chain(content.iter().rev().take(64)) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let mut id = Vec::with_capacity(16);
    id.extend_from_slice(&h.to_be_bytes());
    id.extend_from_slice(&h.rotate_left(29).wrapping_mul(0x9E37_79B9_7F4A_7C15).to_be_bytes());
    let s = Object::String(PdfString { bytes: id, hex: true });
    trailer.set(b"ID".to_vec(), Object::Array(vec![s.clone(), s]));
}

/// A PDF date string for "now" given seconds since the Unix epoch (UTC).
pub fn pdf_date(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::Lexer;

    fn roundtrip(o: &Object) -> Object {
        let mut out = Vec::new();
        serialize(o, &mut out);
        Lexer::new(&out, 0).object().expect("parses")
    }

    #[test]
    fn reals_keep_full_precision_without_exponents() {
        for r in [0.1, -0.000_000_123_456_789, 1e-12, 123_456.789_012_345, 1.0 / 3.0, -2.5e7 + 0.25] {
            let mut out = Vec::new();
            serialize(&Object::Real(r), &mut out);
            assert!(!out.contains(&b'e') && !out.contains(&b'E'), "{}", String::from_utf8_lossy(&out));
            assert_eq!(roundtrip(&Object::Real(r)), Object::Real(r));
        }
    }

    proptest! {
        #[test]
        fn any_finite_real_round_trips(r in proptest::num::f64::NORMAL | proptest::num::f64::SUBNORMAL | proptest::num::f64::ZERO) {
            prop_assume!(r.abs() < 1e15 || r.fract() == 0.0);
            let back = roundtrip(&Object::Real(r));
            match back {
                Object::Real(b) => prop_assert_eq!(b, r),
                Object::Int(i) => prop_assert_eq!(i as f64, r),
                other => prop_assert!(false, "unexpected {other:?}"),
            }
        }

        #[test]
        fn strings_and_names_round_trip(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let s = Object::String(PdfString { bytes: bytes.clone(), hex: false });
            prop_assert_eq!(roundtrip(&s).as_string().map(|s| s.bytes.clone()), Some(bytes.clone()));
            if !bytes.is_empty() && !bytes.contains(&0) {
                prop_assert_eq!(roundtrip(&Object::Name(bytes.clone())), Object::Name(bytes));
            }
        }
    }
}
