//! Bridging the COS object model and `pdfcraft-crypt`: reading `/Encrypt` dictionaries and
//! decrypting / encrypting the strings and streams of an object (§7.6.2).
//!
//! Not encrypted, per the spec: the `/Encrypt` dictionary itself, cross-reference streams, the
//! trailer `/ID`, objects inside object streams (the object stream is encrypted as a whole), and
//! metadata streams when `/EncryptMetadata` is false. Streams naming a crypt filter (`/Crypt` in
//! `/Filter`) use that filter; embedded files use `/EFF`.

use pdfcraft_crypt::{EncryptDict, Method, SecurityHandler, StreamKind};

use crate::{Dict, Document, Object, PdfString, Stream};

fn bytes_of(doc: &Document, d: &Dict, key: &[u8]) -> Vec<u8> {
    d.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_string().map(|s| s.bytes.clone())).unwrap_or_default()
}

/// Read the handler parameters from an `/Encrypt` dictionary.
pub(crate) fn encrypt_dict(doc: &Document, d: &Dict) -> EncryptDict {
    let int = |k: &[u8], default: i64| d.get(k).map(|v| doc.resolve(v)).and_then(|v| v.as_int()).unwrap_or(default);
    let name = |k: &[u8]| d.get(k).and_then(|v| v.as_name()).map(|n| n.to_vec()).unwrap_or_else(|| b"Identity".to_vec());
    let mut crypt_filters = Vec::new();
    if let Some(cf) = d.get(b"CF").map(|c| doc.resolve(c)).and_then(|c| c.as_dict().cloned()) {
        for (k, v) in cf.iter() {
            let Some(f) = doc.resolve(v).as_dict().cloned() else { continue };
            let method = match f.name(b"CFM") {
                Some(b"V2") => Method::Rc4,
                Some(b"AESV2") => Method::Aes128,
                Some(b"AESV3") => Method::Aes256,
                _ => Method::Identity,
            };
            crypt_filters.push((k.clone(), method));
        }
    }
    let v = int(b"V", 0);
    // V4: the key length comes from the standard crypt filter's /Length (bytes, or bits in
    // some files); /Length on the dictionary is often a stale 40.
    let cf_len = d
        .get(b"CF")
        .map(|c| doc.resolve(c))
        .and_then(|c| c.as_dict().and_then(|c| c.get(b"StdCF").map(|f| doc.resolve(f))))
        .and_then(|f| f.as_dict().and_then(|f| f.int(b"Length")));
    let length_bits = match (v, cf_len) {
        (5.., _) => 256,
        (4, Some(n)) if n <= 32 => n * 8,
        (4, Some(n)) => n,
        (4, None) => 128,
        _ => int(b"Length", 40),
    };
    EncryptDict {
        filter: d.name(b"Filter").map(|n| n.to_vec()).unwrap_or_default(),
        v,
        r: int(b"R", 0),
        length_bits,
        o: bytes_of(doc, d, b"O"),
        u: bytes_of(doc, d, b"U"),
        oe: bytes_of(doc, d, b"OE"),
        ue: bytes_of(doc, d, b"UE"),
        perms: bytes_of(doc, d, b"Perms"),
        p: int(b"P", -1) as i32,
        encrypt_metadata: !matches!(d.get(b"EncryptMetadata"), Some(Object::Bool(false))),
        crypt_filters,
        stm_f: name(b"StmF"),
        str_f: name(b"StrF"),
        ef_f: d.get(b"EFF").and_then(|v| v.as_name()).map(|n| n.to_vec()).unwrap_or_default(),
    }
}

/// The crypt filter a stream names in its `/Filter` chain (`/Crypt` + `/DecodeParms /Name`).
fn named_crypt_filter(dict: &Dict) -> Option<Vec<u8>> {
    let names: Vec<&[u8]> = match dict.get(b"Filter") {
        Some(Object::Name(n)) => vec![n],
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_name).collect(),
        _ => return None,
    };
    let i = names.iter().position(|n| *n == b"Crypt")?;
    let parms = match dict.get(b"DecodeParms") {
        Some(Object::Dict(d)) if i == 0 => Some(d.clone()),
        Some(Object::Array(a)) => a.get(i).and_then(|p| p.as_dict().cloned()),
        _ => None,
    };
    Some(parms.and_then(|p| p.name(b"Name").map(|n| n.to_vec())).unwrap_or_else(|| b"Identity".to_vec()))
}

/// Decrypt (`decrypt = true`) or encrypt every string and stream in an indirect object.
pub(crate) fn transform(h: &SecurityHandler, o: &Object, num: u32, generation: u16, decrypt: bool) -> Object {
    let strings = |s: &PdfString| {
        let bytes = if decrypt { h.decrypt_string(num, generation, &s.bytes) } else { h.encrypt_string(num, generation, &s.bytes) };
        PdfString { bytes, hex: s.hex || !decrypt }
    };
    walk(o, &strings, h, num, generation, decrypt)
}

fn walk(o: &Object, strings: &dyn Fn(&PdfString) -> PdfString, h: &SecurityHandler, num: u32, generation: u16, decrypt: bool) -> Object {
    match o {
        Object::String(s) => Object::String(strings(s)),
        Object::Array(a) => Object::Array(a.iter().map(|x| walk(x, strings, h, num, generation, decrypt)).collect()),
        Object::Dict(d) => Object::Dict(d.iter().map(|(k, v)| (k.clone(), walk(v, strings, h, num, generation, decrypt))).collect()),
        Object::Stream(s) => {
            let dict: Dict = s.dict.iter().map(|(k, v)| (k.clone(), walk(v, strings, h, num, generation, decrypt))).collect();
            let ty = s.dict.name(b"Type");
            if ty == Some(b"XRef") || (ty == Some(b"Metadata") && !h.encrypts_metadata()) {
                return Object::Stream(Stream { dict, raw: s.raw.clone() });
            }
            let named = named_crypt_filter(&s.dict);
            let kind = match (&named, ty) {
                (Some(n), _) => StreamKind::Named(n),
                (None, Some(b"EmbeddedFile")) => StreamKind::EmbeddedFile,
                _ => StreamKind::Normal,
            };
            let raw = if decrypt { h.decrypt_stream(num, generation, &s.raw, kind) } else { h.encrypt_stream(num, generation, &s.raw, kind) };
            Object::Stream(Stream { dict, raw: std::sync::Arc::new(raw) })
        }
        other => other.clone(),
    }
}
