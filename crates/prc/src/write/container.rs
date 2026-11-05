//! The PRC container writer: the file header, file-structure descriptions,
//! each structure's header and its five zlib sections, then the model file
//! [WD 6.1-6.2, 10.2]. The inverse of [`crate::PrcFile::parse`].

use std::io::Write as _;

use crate::{PRC_READER_VERSION, UniqueId};

/// One file structure: its id, its five inflated sections in
/// [`crate::SectionKind`] order, and the uncompressed files its header
/// carries (texture pictures, WD 6.2.2).
pub(crate) struct Structure<'a> {
    pub(crate) id: UniqueId,
    pub(crate) sections: [&'a [u8]; 5],
    pub(crate) files: Vec<&'a [u8]>,
}

/// The ids a container carries besides its structures'.
pub(crate) struct Ids {
    pub(crate) file: UniqueId,
    pub(crate) application: UniqueId,
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing into a Vec cannot fail.
    drop(e.write_all(data));
    e.finish().unwrap_or_default()
}

fn le(out: &mut Vec<u8>, vs: &[u32]) {
    vs.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
}

/// Offsets past `u32::MAX` are unrepresentable; `None` then.
fn offset(at: usize) -> Option<u32> {
    u32::try_from(at).ok()
}

/// The PRC stream: version 8137 for reading and authoring (Acrobat refuses
/// 8138 and 10001), no file-header uncompressed files. `None` past 4 GiB.
pub(crate) fn container(structures: &[Structure<'_>], model: &[u8], ids: &Ids) -> Option<Vec<u8>> {
    let v = PRC_READER_VERSION;
    // "PRC", 2 versions, 2 ids, count; 48 bytes per description; model
    // extent; uncompressed-file count.
    let header_len = 3 + 4 * 11 + 48 * structures.len() + 8 + 4;
    let mut body = Vec::new();
    let mut descriptions = Vec::new();
    for s in structures {
        let start = offset(header_len + body.len())?;
        body.extend_from_slice(b"PRC");
        le(&mut body, &[v, v]);
        le(&mut body, &s.id.0);
        le(&mut body, &ids.application.0);
        le(&mut body, &[u32::try_from(s.files.len()).ok()?]);
        for f in &s.files {
            le(&mut body, &[u32::try_from(f.len()).ok()?]);
            body.extend_from_slice(f);
        }
        le(&mut descriptions, &s.id.0);
        le(&mut descriptions, &[0, 6, start]);
        for section in s.sections {
            le(&mut descriptions, &[offset(header_len + body.len())?]);
            body.extend(zlib(section));
        }
    }
    let mf_start = offset(header_len + body.len())?;
    body.extend(zlib(model));
    let mf_end = offset(header_len + body.len())?;

    let mut out = b"PRC".to_vec();
    le(&mut out, &[v, v]);
    le(&mut out, &ids.file.0);
    le(&mut out, &ids.application.0);
    le(&mut out, &[u32::try_from(structures.len()).ok()?]);
    out.extend(descriptions);
    le(&mut out, &[mf_start, mf_end, 0]);
    debug_assert_eq!(out.len(), header_len);
    out.extend(body);
    Some(out)
}
