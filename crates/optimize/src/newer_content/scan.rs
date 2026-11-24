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
            for e in a {
                child_edges(e, bits, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// Font programs (`/FontFile2`, `/FontFile3`) the stream pass reads, found while scanning.
pub(super) type Fonts = Vec<ObjRef>;

/// Apply every rule to dictionary `d` (context `inherited` plus its own entries) and to the
/// direct dictionaries and arrays inside it. The encryption dictionary is left to the optimizer's
/// own check.
pub(super) fn scan_dict(d: &Dict, inherited: u64, depth: usize, notes: &mut Notes, fonts: &mut Fonts) {
    if depth > MAX_DEPTH || inherited & ENCRYPT != 0 {
        return;
    }
    let raw = ctx::own(d, inherited & ctx::STREAM != 0) | inherited;
    let bits = ctx::settled(raw);
    checks::custom(d, bits, notes);
    let binary = bits & (SIGNATURE | TRAILER) != 0;
    for (key, v) in d.iter() {
        if let Some(ver) = keys::key(key).and_then(|r| keys::applies(r, bits)) {
            notes.insert((ver, format!("/{}", lossy(key))));
        }
        if bits & FONT_DESC != 0 && matches!(key.as_slice(), b"FontFile2" | b"FontFile3") {
            fonts.extend(v.as_ref());
        }
        let child = ctx::edge(raw, key);
        scan_value(key, v, Context { bits, child, binary }, depth, (notes, fonts));
    }
}

/// The context of a value: its dictionary's bits, the bits a dictionary inside it earns, and
/// whether its strings are binary data rather than text.
#[derive(Clone, Copy)]
struct Context {
    bits: u64,
    child: u64,
    binary: bool,
}

fn scan_value(key: &[u8], v: &Object, cx: Context, depth: usize, (notes, fonts): (&mut Notes, &mut Fonts)) {
    match v {
        Object::Name(n) => {
            if let Some(ver) = keys::value(key, n, cx.bits) {
                notes.insert((ver, format!("/{} /{}", lossy(key), lossy(n))));
            }
        }
        Object::String(s) if !cx.binary && s.bytes.starts_with(&[0xEF, 0xBB, 0xBF]) => {
            notes.insert((20, "UTF-8 text string".to_owned()));
        }
        Object::Dict(d) => scan_dict(d, cx.child & !ctx::STREAM, depth + 1, notes, fonts),
        Object::Array(a) if depth < MAX_DEPTH => {
            for e in a {
                match e {
                    Object::Name(n) if key == b"Filter" => {
                        if let Some(ver) = keys::filter(n) {
                            notes.insert((ver, format!("/Filter /{}", lossy(n))));
                        }
                    }
                    _ => scan_value(key, e, cx, depth + 1, (notes, fonts)),
                }
            }
        }
        _ => {}
    }
}

/// `b` as text for a construct's name; invalid UTF-8 is replaced, not refused.
pub(super) fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}
