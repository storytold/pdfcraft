//! The two passes: carry context bits along the keys that reach each object until nothing
//! changes, then apply the key, value and custom rules to every dictionary.

use std::collections::{BTreeSet, HashMap, VecDeque};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use super::ctx::{self, ENCRYPT, FONT_DESC, SIGNATURE, TRAILER};
use super::{checks, keys};

/// How deep direct dictionaries and arrays are followed inside one object.
const MAX_DEPTH: usize = 32;

/// The `(version, construct)` pairs one object needs.
pub(super) type Notes = BTreeSet<(u8, String)>;

/// Every object of `doc`.
pub(super) fn objects(doc: &Document) -> Vec<ObjRef> {
    doc.object_numbers().into_iter().map(|num| ObjRef { num, generation: doc.generation(num) }).collect()
}

/// The context bits every object earns from the keys reaching it, from the trailer down. Bits
/// only grow, and there are 64 of them, so each object is walked at most 65 times.
pub(super) fn reached(doc: &Document, all: &[ObjRef]) -> HashMap<ObjRef, u64> {
    let mut bits: HashMap<ObjRef, u64> = HashMap::new();
    let mut queue: VecDeque<ObjRef> = all.iter().copied().collect();
    let mut found = Vec::new();
    dict_edges(doc.trailer(), TRAILER, 0, &mut found);
    grow(&mut bits, &mut found, &mut queue);
    while let Some(r) = queue.pop_front() {
        let inherited = bits.get(&r).copied().unwrap_or(0);
        match &*doc.get(r) {
            Object::Dict(d) => dict_edges(d, inherited, 0, &mut found),
            Object::Stream(s) => dict_edges(&s.dict, inherited | ctx::STREAM, 0, &mut found),
            // An array of its own passes on what its referrer gave it (`/Contents 10 0 R`).
            array @ Object::Array(_) => child_edges(array, inherited & !ctx::STREAM, 0, &mut found),
            _ => {}
        }
        grow(&mut bits, &mut found, &mut queue);
    }
    bits
}

fn grow(bits: &mut HashMap<ObjRef, u64>, found: &mut Vec<(ObjRef, u64)>, queue: &mut VecDeque<ObjRef>) {
    for (r, b) in found.drain(..) {
        let e = bits.entry(r).or_insert(0);
        if *e | b != *e {
            *e |= b;
            queue.push_back(r);
        }
    }
}

fn dict_edges(d: &Dict, inherited: u64, depth: usize, out: &mut Vec<(ObjRef, u64)>) {
    let bits = ctx::own(d, inherited & ctx::STREAM != 0) | inherited;
    for (k, v) in d.iter() {
        child_edges(v, ctx::edge(bits, k), depth, out);
    }
}

fn child_edges(v: &Object, bits: u64, depth: usize, out: &mut Vec<(ObjRef, u64)>) {
    if depth > MAX_DEPTH {
        return;
    }
    match v {
        Object::Ref(r) if bits != 0 => out.push((*r, bits)),
        Object::Dict(d) => dict_edges(d, bits & !ctx::STREAM, depth + 1, out),
        Object::Array(a) => {
            // Only a page's `/Contents` hands content to an array: its streams are pieces of one.
            // An array inside it holds no pieces (only direct members are joined), so its streams
            // are read as content on their own.
            let bits = if bits & ctx::PIECE != 0 {
                bits | ctx::CONTENT
            } else if bits & ctx::CONTENT != 0 {
                (bits & !ctx::CONTENT) | ctx::PIECE
            } else {
                bits
            };
            for e in a {
                child_edges(e, bits, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// What the scan of one object found, and the document its references are read in.
pub(super) struct Found<'a> {
    doc: &'a Document,
    /// What the object being scanned needs.
    pub(super) notes: Notes,
    /// Font programs (`/FontFile2`, `/FontFile3`) the stream pass reads.
    pub(super) fonts: Vec<ObjRef>,
    /// What each array given by reference needs under one key and context, so an array shared by
    /// many dictionaries is walked once per key and context. An entry is made before the walk, so
    /// an array reached again inside itself adds nothing. A walk cut short by the depth limit is
    /// reused where the array is reached less deep, which can only leave a construct out.
    arrays: HashMap<(ObjRef, Vec<u8>, Context), Notes>,
}

impl<'a> Found<'a> {
    pub(super) fn new(doc: &'a Document) -> Self {
        Found { doc, notes: Notes::new(), fonts: Vec::new(), arrays: HashMap::new() }
    }
}

/// Apply every rule to dictionary `d` (context `inherited` plus its own entries) and to the
/// direct dictionaries and arrays inside it. The encryption dictionary is left to the optimizer's
/// own check.
pub(super) fn scan_dict(d: &Dict, inherited: u64, depth: usize, found: &mut Found) {
    if depth > MAX_DEPTH || inherited & ENCRYPT != 0 {
        return;
    }
    let raw = ctx::own(d, inherited & ctx::STREAM != 0) | inherited;
    let bits = ctx::settled(raw);
    checks::custom(d, bits, &mut found.notes);
    for (key, v) in d.iter() {
        if let Some(ver) = keys::key(key).and_then(|r| keys::applies(r, bits)) {
            found.notes.insert((ver, format!("/{}", lossy(key))));
        }
        if bits & FONT_DESC != 0 && matches!(key.as_slice(), b"FontFile2" | b"FontFile3") {
            found.fonts.extend(v.as_ref());
        }
        let child = ctx::edge(raw, key);
        scan_value(key, v, Context { bits, child, binary: binary(bits, key) }, depth, found);
    }
}

/// Whether the strings under `key` are binary data: the trailer's `/ID`, and a signature's
/// `/Contents` and `/Cert` (§12.8.1); a signature's `/Reason` or `/Location` is text.
fn binary(bits: u64, key: &[u8]) -> bool {
    bits & TRAILER != 0 || (bits & SIGNATURE != 0 && matches!(key, b"Contents" | b"Cert"))
}

/// The context of a value: its dictionary's bits, the bits a dictionary inside it earns, and
/// whether its strings are binary data rather than text.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Context {
    bits: u64,
    child: u64,
    binary: bool,
}

fn scan_value(key: &[u8], v: &Object, cx: Context, depth: usize, found: &mut Found) {
    match v {
        Object::Name(_) | Object::String(_) => scalar(key, v, cx, &mut found.notes),
        Object::Dict(d) => scan_dict(d, cx.child & !ctx::STREAM, depth + 1, found),
        Object::Array(a) if depth < MAX_DEPTH => {
            for e in a {
                scan_value(key, e, cx, depth + 1, found);
            }
        }
        Object::Ref(r) if depth < MAX_DEPTH => indirect(key, *r, cx, depth, found),
        _ => {}
    }
}

/// A value given by reference, judged under the key and dictionary naming it: a name, a string,
/// or an array, whose entries are judged as a direct array's are (`/BM 9 0 R`, object 9 being
/// `[/Overlay]`). Dictionaries and streams are objects of their own, scanned once in the context
/// they are reached in, and an object that is only a name is no one's value until a key names it.
fn indirect(key: &[u8], r: ObjRef, cx: Context, depth: usize, found: &mut Found) {
    let doc = found.doc;
    let obj = doc.get(r);
    match &*obj {
        v @ (Object::Name(_) | Object::String(_)) => scalar(key, v, cx, &mut found.notes),
        Object::Array(a) => {
            let id = (r, key.to_vec(), cx);
            if let Some(notes) = found.arrays.get(&id) {
                let notes = notes.clone();
                found.notes.extend(notes);
                return;
            }
            found.arrays.insert(id.clone(), Notes::new());
            let outer = std::mem::take(&mut found.notes);
            for e in a {
                scan_value(key, e, cx, depth + 1, found);
            }
            let notes = std::mem::replace(&mut found.notes, outer);
            found.notes.extend(notes.iter().cloned());
            found.arrays.insert(id, notes);
        }
        _ => {}
    }
}

fn scalar(key: &[u8], v: &Object, cx: Context, notes: &mut Notes) {
    match v {
        Object::Name(n) => {
            if let Some(ver) = keys::value(key, n, cx.bits) {
                notes.insert((ver, format!("/{} /{}", lossy(key), lossy(n))));
            }
        }
        Object::String(s) if !cx.binary && s.bytes.starts_with(&[0xEF, 0xBB, 0xBF]) => {
            notes.insert((20, "UTF-8 text string".to_owned()));
        }
        _ => {}
    }
}

/// `b` as text for a construct's name; invalid UTF-8 is replaced, not refused.
pub(super) fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}
