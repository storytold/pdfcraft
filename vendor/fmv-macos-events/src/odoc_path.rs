//! Recovery of file paths from the raw bytes of odoc item descriptors.
//!
//! PdfCraft patch (#6324): on macOS 13, an `open`/Finder odoc item arrives as a
//! `'bmrk'` (binary bookmark) descriptor, not a `typeFileURL`. The bookmark stores the
//! path twice: its per-component records keep every name in its exact on-disk bytes,
//! while its embedded full-path string is normalized to NFD. When any on-disk
//! component is NFC-precomposed, `NSAppleEventDescriptor.fileURLValue()` cannot
//! resolve the bookmark and returns nil, and the published crate's `?` silently
//! dropped the item: the app launched showing nothing, without an error.
//!
//! This module decodes the descriptor's data directly, with no Objective-C, so the
//! recovery is unit-testable on every platform. Every parse is total: malformed or
//! truncated data returns `None`, the caller falls back to the NSURL chain, and a
//! rebuilt path is delivered only when it names an existing file. The bookmark
//! component layout (`[length u32 LE][01 01 00 00][utf-8 bytes][NUL padding to a
//! 4-byte boundary]`, records contiguous) was reverse-engineered from Ventura
//! captures; see the pinned evidence on issue #6324. Any anomaly falls back to
//! NSURL, so behaviour can only degrade to the published crate's, never below it.

use std::path::PathBuf;

/// A bookmark blob larger than this is not one of Apple's (they are about a KiB);
/// refuse it rather than scan it. This bounds the chain search, whose worst case is
/// quadratic in the blob's length.
const MAX_BOOKMARK_BYTES: usize = 1 << 18;
/// A path with more components than this is not a file LaunchServices would name.
/// Bounds one chain walk.
const MAX_COMPONENTS: usize = 128;
/// The longest single component name accepted: well past macOS's 255-name limit, so
/// a longer "record" is noise in the blob, not a name.
const MAX_COMPONENT_BYTES: usize = 1024;

/// The path of one odoc item: from its descriptor's raw bytes when they name an
/// existing file, else whatever `nsurl_path` returns (the `fileURLValue` /
/// `NSURL.path` chain of the published crate).
///
/// `'bmrk'` data is walked for its byte-exact component records, joined, and
/// delivered only if the joined path is an existing file; `'furl'` data is
/// percent-decoded. Anything else — unknown descriptor types, malformed or truncated
/// data, a rebuilt path that does not exist — takes the NSURL fallback, which is
/// exactly where the published crate's behaviour already was.
pub(crate) fn item_path(
    descriptor_type: u32,
    data: &[u8],
    nsurl_path: impl FnOnce() -> Option<String>,
) -> Option<PathBuf> {
    let from_raw_bytes = if descriptor_type == u32::from_be_bytes(*b"bmrk") {
        bookmark_chain(data).filter(|path| path.is_file())
    } else if descriptor_type == u32::from_be_bytes(*b"furl") {
        file_url_path(data)
    } else {
        None
    };
    from_raw_bytes.or_else(|| nsurl_path().map(PathBuf::from))
}

/// The `'file://'` bytes of a `'furl'` descriptor, percent-decoded by hand so the
/// exact bytes LaunchServices put in the event survive. The data is historically
/// NUL-terminated; trailing NULs are stripped first.
pub(crate) fn file_url_path(data: &[u8]) -> Option<PathBuf> {
    let mut end = data.len();
    while end > 0 && data[end - 1] == 0 {
        end -= 1;
    }
    let bytes = data.get(..end)?;
    let after_prefix = bytes.strip_prefix(b"file://")?;
    // Skip the authority (empty for local files); the path starts at its first '/'.
    let slash = after_prefix.iter().position(|&byte| byte == b'/')?;
    let decoded = percent_decode(&after_prefix[slash..]);
    let path = String::from_utf8_lossy(&decoded).into_owned();
    if path.starts_with('/') { Some(PathBuf::from(path)) } else { None }
}

/// `%XX` escapes decoded to bytes. A malformed escape passes through untouched, as
/// tolerant URL readers treat it.
pub(crate) fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

/// One 0x0101 component record: its payload bytes and the offset just past them.
struct Record<'data> {
    bytes: &'data [u8],
    end: usize,
}

/// The component record starting exactly at `offset`, if one does:
/// `[length u32 LE][01 01 00 00][payload]`, a length in `1..=MAX_COMPONENT_BYTES`
/// with the payload inside `data`. The NUL padding after the payload (up to a 4-byte
/// boundary) is not part of the record.
fn record_at(data: &[u8], offset: usize) -> Option<Record<'_>> {
    let header_end = offset.checked_add(8)?;
    let header = data.get(offset..header_end)?;
    let (length_bytes, magic) = header.split_at(4);
    let length = u32::from_le_bytes(<[u8; 4]>::try_from(length_bytes).ok()?) as usize;
    if magic != [0x01, 0x01, 0x00, 0x00] || length == 0 || length > MAX_COMPONENT_BYTES {
        return None;
    }
    let end = header_end.checked_add(length)?;
    Some(Record { bytes: data.get(header_end..end)?, end })
}

/// The next record in a chain: at `from` plus up to 3 NUL padding bytes.
fn next_record(data: &[u8], from: usize) -> Option<Record<'_>> {
    (0..4).find_map(|padding| record_at(data, from.checked_add(padding)?))
}

/// The path joined from a bookmark's contiguous chain of 0x0101 component records —
/// every name in its exact on-disk bytes — but only when that path is an existing
/// file. A deliverable chain has two or more non-empty components without NULs; an
/// on-disk name cannot contain NUL, so a record that does is padding or noise.
pub(crate) fn bookmark_chain(data: &[u8]) -> Option<PathBuf> {
    if data.len() > MAX_BOOKMARK_BYTES {
        return None;
    }
    let mut scan = 0;
    while scan < data.len() {
        let Some(first) = record_at(data, scan) else {
            scan += 1;
            continue;
        };
        // Walk the maximal chain of records starting at `first`.
        let mut components = vec![first.bytes];
        let mut tail = first.end;
        while components.len() < MAX_COMPONENTS {
            match next_record(data, tail) {
                Some(record) => {
                    components.push(record.bytes);
                    tail = record.end;
                }
                None => break,
            }
        }
        if let Some(path) = join_components(&components).filter(|path| path.is_file()) {
            return Some(path);
        }
        // This chain does not name a file; resume the scan behind the whole chain.
        scan = tail + 1;
    }
    None
}

/// The components joined into an absolute path, if they can all be names.
fn join_components(components: &[&[u8]]) -> Option<PathBuf> {
    let usable = components.len() >= 2
        && components.iter().all(|name| !name.is_empty() && !name.contains(&0));
    if !usable {
        return None;
    }
    let mut path = String::from("/");
    for (index, name) in components.iter().enumerate() {
        if index > 0 {
            path.push('/');
        }
        path.push_str(&String::from_utf8_lossy(name));
    }
    Some(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::{bookmark_chain, file_url_path, item_path, percent_decode};
    use std::path::PathBuf;

    /// Append a bookmark-style record: `[length u32 LE][magic u16 LE + 00 00][bytes]`,
    /// NUL-padded to a 4-byte boundary, as the Ventura captures pack them.
    fn push_record(data: &mut Vec<u8>, magic: u16, bytes: &[u8]) {
        data.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        data.extend_from_slice(&magic.to_le_bytes());
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(bytes);
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
    }

    /// A bookmark-style blob whose 0x0101 component records spell `names` in their
    /// exact bytes, preceded by a `book` header and followed by an NFD-normalized
    /// 0x0105 full-path record, as the captures carried both encodings.
    fn blob(names: &[&str], full_path: &str) -> Vec<u8> {
        let mut data = b"book\x00\x00\x00\x00".to_vec();
        for name in names {
            push_record(&mut data, 0x0101, name.as_bytes());
        }
        push_record(&mut data, 0x0105, full_path.as_bytes());
        data
    }

    /// The path components of `path` (its bytes are all valid UTF-8 in these tests).
    fn components_of(path: &std::path::Path) -> Vec<&str> {
        path.to_str().expect("test paths are UTF-8").trim_start_matches('/').split('/').collect()
    }

    /// A chain of byte-exact 0x0101 records delivers the on-disk NFC bytes even
    /// though the blob also carries the NFD-normalized full-path record — the exact
    /// shape of the Ventura failure (#6324).
    #[test]
    fn bookmark_chain_delivers_the_on_disk_nfc_bytes_beside_an_nfd_full_path() {
        let directory = std::env::temp_dir().join(format!("fmv-bmrk-{}", std::process::id()));
        let nfc = directory.join("\u{00c1}rea fiscal"); // NFC: UTF-8 c3 81
        std::fs::create_dir_all(&nfc).expect("create the NFC-named directory");
        let file = nfc.join("doc.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").expect("create the file");
        let names = components_of(&file);
        let nfd_full_path = file.to_str().expect("UTF-8").replace('\u{00c1}', "A\u{0301}");
        let data = blob(&names, &nfd_full_path);

        let delivered = bookmark_chain(&data).expect("the chain names an existing file");

        assert_eq!(delivered, file);
        assert!(
            delivered.as_os_str().as_encoded_bytes().windows(2).any(|pair| pair == [0xc3, 0x81]),
            "the delivered path keeps the NFC bytes, not the NFD full path"
        );
        std::fs::remove_dir_all(&directory).expect("clean up");
    }

    /// Existence gating: a well-formed chain that names no file is not delivered.
    #[test]
    fn bookmark_chain_rejects_a_chain_that_names_no_file() {
        let data = blob(&["definitely", "missing", "nowhere.pdf"], "/definitely/missing/nowhere.pdf");
        assert_eq!(bookmark_chain(&data), None);
    }

    /// A single record is not a path, whatever it names.
    #[test]
    fn bookmark_chain_needs_two_components() {
        let mut single = b"book\x00\x00\x00\x00".to_vec();
        push_record(&mut single, 0x0101, b"doc.pdf");
        assert_eq!(bookmark_chain(&single), None);
    }

    /// Malformed and adversarial blobs are rejected without panicking.
    #[test]
    fn malformed_bookmarks_are_rejected_without_panicking() {
        assert_eq!(bookmark_chain(&[]), None);
        assert_eq!(bookmark_chain(b"book"), None);
        // A header that stops mid-way.
        assert_eq!(bookmark_chain(&[0, 0, 0, 4, 0x01, 0x01]), None);
        // A length that runs past the data.
        assert_eq!(bookmark_chain(&[4, 0, 0, 0, 0x01, 0x01, 0, 0, b'a']), None);
        // The largest length, far past any blob.
        assert_eq!(bookmark_chain(&[0xff, 0xff, 0xff, 0xff, 0x01, 0x01, 0, 0, b'a', b'b']), None);
        // A zero length is not a component.
        assert_eq!(bookmark_chain(&[0, 0, 0, 0, 0x01, 0x01, 0, 0]), None);
        // The component magic is 0x0101; records with another magic never chain and
        // never decode, so the blob as a whole is refused.
        let mut other_magic = b"book\x00\x00\x00\x00".to_vec();
        push_record(&mut other_magic, 0x0102, b"tmp");
        push_record(&mut other_magic, 0x0102, b"x");
        push_record(&mut other_magic, 0x0102, b"y.pdf");
        assert_eq!(bookmark_chain(&other_magic), None);
        // A component containing a NUL cannot be an on-disk name.
        let mut with_nul = b"book\x00\x00\x00\x00".to_vec();
        push_record(&mut with_nul, 0x0101, b"tmp");
        push_record(&mut with_nul, 0x0101, b"a\x00b");
        push_record(&mut with_nul, 0x0101, b"y.pdf");
        assert_eq!(bookmark_chain(&with_nul), None);
        // A blob over the size cap is refused before any scanning.
        let huge = vec![0u8; super::MAX_BOOKMARK_BYTES + 1];
        assert_eq!(bookmark_chain(&huge), None);
    }

    /// An NFD full-path record ahead of the component chain does not disturb the
    /// chain: only 0x0101 records chain, and the padding between them is stepped over.
    #[test]
    fn bookmark_chain_finds_components_behind_other_records() {
        let directory = std::env::temp_dir().join(format!("fmv-pad-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("create the directory");
        let file = directory.join("doc.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").expect("create the file");
        let names = components_of(&file);
        let mut data = b"book\x00\x00\x00\x00".to_vec();
        push_record(&mut data, 0x0105, b"/an/nfd/full/path/doc.pdf");
        for name in &names {
            push_record(&mut data, 0x0101, name.as_bytes());
        }
        assert_eq!(bookmark_chain(&data), Some(file));
        std::fs::remove_dir_all(&directory).expect("clean up");
    }

    /// `'furl'` decoding: escapes, NUL padding, authorities, raw non-ASCII bytes.
    #[test]
    fn file_url_decoding() {
        let decode = |data: &[u8]| {
            file_url_path(data).and_then(|path| path.into_os_string().into_string().ok())
        };
        // Percent escapes, including an escaped NFC Á (%C3%81) and an escaped NFD Á
        // (%CC%81), both preserved as the bytes the sender put in.
        assert_eq!(decode(b"file:///tmp/a%20b/%C3%81.pdf\0"), Some("/tmp/a b/\u{00c1}.pdf".to_owned()));
        assert_eq!(decode(b"file:///tmp/A%CC%81rea fiscal/doc.pdf"), Some("/tmp/A\u{0301}rea fiscal/doc.pdf".to_owned()));
        // Raw non-ASCII bytes without escapes, and NUL padding.
        assert_eq!(decode(b"file:///tmp/\xc3\x81rea fiscal/doc.pdf\0\0"), Some("/tmp/\u{00c1}rea fiscal/doc.pdf".to_owned()));
        // Bytes that are not UTF-8 decode lossily rather than fail.
        assert_eq!(decode(b"file:///tmp/\xff.pdf"), Some("/tmp/\u{fffd}.pdf".to_owned()));
        // The authority is skipped; the path starts at its first '/'.
        assert_eq!(decode(b"file://localhost/tmp/x"), Some("/tmp/x".to_owned()));
        // Not a file URL, no path, nothing at all.
        assert_eq!(decode(b"http://tmp/x"), None);
        assert_eq!(decode(b"file://host"), None);
        assert_eq!(decode(b""), None);
        assert_eq!(decode(b"\0\0"), None);
    }

    /// Percent decoding leaves malformed escapes untouched.
    #[test]
    fn percent_decoding_passes_malformed_escapes_through() {
        assert_eq!(percent_decode(b"100% and %zz"), b"100% and %zz");
        assert_eq!(percent_decode(b"%41%2f"), b"A/");
        assert_eq!(percent_decode(b"trailing %"), b"trailing %");
        assert_eq!(percent_decode(b"short %4"), b"short %4");
        assert_eq!(percent_decode(b""), b"");
    }

    /// The item decode prefers the raw bytes and falls back to NSURL exactly as
    /// intended: raw bookmark bytes that name a file win over a differing NSURL
    /// path; everything unrecognized keeps the published crate's NSURL behaviour.
    #[test]
    fn item_path_prefers_existing_raw_bytes_and_falls_back_to_nsurl() {
        let directory = std::env::temp_dir().join(format!("fmv-item-{}", std::process::id()));
        let nfc = directory.join("\u{00c1}rea fiscal");
        std::fs::create_dir_all(&nfc).expect("create the NFC-named directory");
        let file = nfc.join("doc.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").expect("create the file");
        let names = components_of(&file);
        let nfd_full_path = file.to_str().expect("UTF-8").replace('\u{00c1}', "A\u{0301}");
        let data = blob(&names, &nfd_full_path);
        let bookmark = u32::from_be_bytes(*b"bmrk");

        // The raw bytes win even though NSURL would deliver the NFD string.
        let delivered = item_path(bookmark, &data, || Some(nfd_full_path.clone())).expect("delivered");
        assert_eq!(delivered, file);

        // A bookmark that cannot be decoded falls back to NSURL, byte-faithfully.
        let nfd = "/private/tmp/A\u{0301}rea fiscal/doc.pdf";
        assert_eq!(item_path(bookmark, b"garbage", || Some(nfd.to_owned())), Some(PathBuf::from(nfd)));
        // So does an unknown descriptor type...
        assert_eq!(
            item_path(u32::from_be_bytes(*b"utf8"), b"some text", || Some("/from/nsurl".to_owned())),
            Some(PathBuf::from("/from/nsurl"))
        );
        // ...and a 'furl' item decodes directly, with NSURL empty.
        assert_eq!(
            item_path(u32::from_be_bytes(*b"furl"), b"file:///tmp/x", || None),
            Some(PathBuf::from("/tmp/x"))
        );
        // With nothing to fall back to, the item is dropped, as before.
        assert_eq!(item_path(bookmark, b"garbage", || None), None);

        std::fs::remove_dir_all(&directory).expect("clean up");
    }
}
