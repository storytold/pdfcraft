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
//! decoding is unit-testable on every platform; the four tests that create real
//! files are Unix-only. Delivery is best-effort and never silent for each item of
//! the event's direct object (the contract asked for by the review on #6334):
//! bytes that name an existing file are preferred, the NSURL chain of the
//! published crate comes next, and an item nothing could verify is still
//! delivered — its decoded string, unverified — so a failed open reaches the app
//! and its own open error reports it, instead of the launch showing nothing. An
//! item is dropped only when NSURL has nothing either and its bytes decode to no
//! path: a bookmark in which no chain joins (`join_components`: two or more
//! component records, each non-empty and NUL-free), a `'furl'` that is not a
//! `file://` URL or has no `/` after the prefix, or a descriptor of any other
//! type. A direct object that is not a list is decoded as the event's one item
//! ([`event_paths`](crate::odoc_path::event_paths)), where the published crate
//! walked list items only and queued nothing for it. Every parse is total:
//! malformed or truncated data returns `None`. The bookmark component layout
//! (`[length u32 LE][01 01 00 00][utf-8 bytes][NUL padding to a 4-byte boundary]`,
//! records contiguous) was reverse-engineered from Ventura captures; see the
//! pinned evidence on #6324.

use std::path::PathBuf;

/// A bookmark blob larger than this is not one of Apple's (they are about a KiB);
/// refuse it rather than scan it. The scan itself is linear in the blob (each byte
/// is a candidate start at most once, and a walked chain is skipped whole); what
/// the cap bounds is the number of candidate chains, and with it the `is_file()`
/// probes `bookmark_chain` makes on the main thread.
const MAX_BOOKMARK_BYTES: usize = 1 << 18;
/// A path with more components than this is not a file LaunchServices would name.
/// Bounds one chain walk.
const MAX_COMPONENTS: usize = 128;
/// The longest single component name accepted: well past macOS's 255-name limit, so
/// a longer "record" is noise in the blob, not a name.
const MAX_COMPONENT_BYTES: usize = 1024;

/// The paths one odoc event delivers, in order: each item of the direct object
/// decoded by `decode` (the handler's is [`item_path`] with the NSURL chain), an
/// item that decodes to no path left out. `count` is the direct object's
/// `numberOfItems` and `item_at` its 1-based `descriptorAtIndex`. A direct object
/// with no items is decoded as the one item: a descriptor that is not a list
/// counts none (on macOS 13.3 a `'furl'`, `'alis'`, `'bmrk'` or `'utxt'`; AppleScript
/// sends one for `open POSIX file "…"` without braces), and the published crate,
/// walking `1..=count` alone, queued nothing for it. An empty list, taken the same
/// way, decodes to no path, so it still queues nothing. List items are not
/// flattened: a list inside the list is one item, which takes step 2 of
/// [`item_path`] alone.
pub(crate) fn event_paths<D>(
    direct: D,
    count: isize,
    item_at: impl FnMut(isize) -> Option<D>,
    decode: impl FnMut(&D) -> Option<PathBuf>,
) -> Vec<PathBuf> {
    let items = if count < 1 { vec![direct] } else { (1..=count).filter_map(item_at).collect() };
    items.iter().filter_map(decode).collect()
}

/// The path of one odoc item, best-effort and never silent. A `'bmrk'` or
/// `'furl'` item has a raw decode of its own, tried in three steps:
///
/// 1. the raw decode, verified — a `'bmrk'` chain of byte-exact components that
///    names an existing file, or a `'furl'` percent-decode (the URL's path, from
///    the first `/` after `file://`) that names an existing file;
/// 2. whatever `nsurl_path` returns (the `fileURLValue` / `NSURL.path` chain of
///    the published crate), which also resolves what a decode cannot follow: a
///    file reference URL (`file:///.file/id=…`) decodes to a `/.file/id=…`
///    string that does not open (`/.file` is a regular file, so the lookup fails
///    with `ENOTDIR`), while `NSURL.path` resolves it to the file's current path;
/// 3. the raw decode again, now unverified — for a `'bmrk'` item the first
///    plausible chain string in scan order, for a `'furl'` item its decode —
///    delivered rather than dropping the item silently.
///
/// So the path delivered is a raw decode that names an existing file, or the
/// published crate's own answer, or — only where the published crate dropped
/// the item — the raw decode, unverified. Any other descriptor type takes step 2
/// alone. NSURL is consulted only when step 1 finds no file; for a `'furl'` item
/// step 1 is one `is_file()` probe on the main thread (a bookmark's probes are
/// bounded by `MAX_BOOKMARK_BYTES`).
///
/// Step 3 is the never-drop rule (review on #6334): an open in which an item
/// fails every other way still queues a path, and the app's existing open error
/// reports the file it names — a wrong name surfaced beats a silent no-op, and a
/// partial multi-file open reports each failed item the same way. The unverified
/// string has no gate of its own: a chain passes only the one every chain passes
/// to exist at all, [`join_components`] — two or more component records, each
/// non-empty and NUL-free, joined lossily as UTF-8 under a leading `/` — and a
/// `'furl'` decode only [`file_url_path`]'s, a `file://` URL with a `/` after
/// the prefix. With nothing from NSURL, a bookmark in which no chain joins, a
/// `'furl'` that [`file_url_path`] refuses and any other item are dropped, as
/// before.
pub(crate) fn item_path(
    descriptor_type: u32,
    data: &[u8],
    nsurl_path: impl FnOnce() -> Option<String>,
) -> Option<PathBuf> {
    const BOOKMARK: u32 = u32::from_be_bytes(*b"bmrk");
    const FILE_URL: u32 = u32::from_be_bytes(*b"furl");
    // 2. The published crate's chain.
    let from_nsurl = move || nsurl_path().map(PathBuf::from);
    match descriptor_type {
        // 1. A chain that names an existing file; 2; 3. the first chain that
        //    joins, scanned only now that nothing else delivered a path.
        BOOKMARK => bookmark_chain(data)
            .or_else(from_nsurl)
            .or_else(|| bookmark_chain_best_effort(data)),
        // 1. The decode, when it names an existing file; 2; 3. the decode
        //    again, unverified (`None` when `file_url_path` refuses the bytes).
        FILE_URL => file_url_path(data)
            .filter(|path| path.is_file())
            .or_else(from_nsurl)
            .or_else(|| file_url_path(data)),
        _ => from_nsurl(),
    }
}

/// The `'file://'` bytes of a `'furl'` descriptor, percent-decoded by hand so the
/// exact bytes the sender put in the event survive. The data is historically
/// NUL-terminated; trailing NULs are stripped first. The decode is lexical: it
/// does not resolve a file reference URL (`file:///.file/id=…`), which
/// [`item_path`] leaves to NSURL when the decode names no file.
pub(crate) fn file_url_path(data: &[u8]) -> Option<PathBuf> {
    let mut end = data.len();
    while end > 0 && data[end - 1] == 0 {
        end -= 1;
    }
    let bytes = data.get(..end)?;
    let after_prefix = bytes.strip_prefix(b"file://")?;
    // Skip the authority (empty for local files); the path starts at its first '/',
    // and a URL without one has no path.
    let slash = after_prefix.iter().position(|&byte| byte == b'/')?;
    let path_bytes = &after_prefix[slash..];
    // Cut at an unescaped '?' or '#': as NSURL.path does, deliver the URL's path
    // alone, without its query or fragment. The cut happens on the raw bytes,
    // before percent-decoding, so an escaped %3F or %23 still decodes to a literal
    // character — only the sender's own, unescaped delimiters end the path.
    let delimiter =
        path_bytes.iter().position(|&byte| byte == b'?' || byte == b'#').unwrap_or(path_bytes.len());
    let decoded = percent_decode(&path_bytes[..delimiter]);
    let path = String::from_utf8_lossy(&decoded).into_owned();
    // Absolute by construction: it starts with the '/' found above, which decoding keeps.
    Some(PathBuf::from(path))
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
/// Chains that name no file are not delivered here; [`item_path`] may still
/// deliver the first one unverified, per the never-drop rule.
pub(crate) fn bookmark_chain(data: &[u8]) -> Option<PathBuf> {
    chains(data).find(|path| path.is_file())
}

/// The first plausible chain of 0x0101 records joined into a path, without the
/// existence requirement: the best-effort string [`item_path`] delivers rather
/// than drop an item that nothing could verify. Its only gate is the join's
/// ([`join_components`]).
pub(crate) fn bookmark_chain_best_effort(data: &[u8]) -> Option<PathBuf> {
    chains(data).next()
}

/// The plausible paths joined from a bookmark's chains of 0x0101 component
/// records, in scan order: the records starting at each offset in turn, each
/// extended while a further record follows it. Every component name keeps its
/// exact on-disk bytes. A chain whose components cannot all be names (too few,
/// empty, or holding NUL) yields no path, and the scan always resumes behind the
/// whole chain, so its records are not rescanned as candidates of their own.
fn chains(data: &[u8]) -> impl Iterator<Item = PathBuf> + '_ {
    let mut scan = 0;
    std::iter::from_fn(move || {
        // A blob over the size cap is not one of Apple's (they are about a KiB);
        // refuse it rather than scan it. The scan below is linear — `scan` only
        // advances, and a walked chain is skipped whole — so the cap bounds how
        // many candidate chains a blob can yield, and with them the `is_file()`
        // probes of `bookmark_chain`.
        if data.len() > MAX_BOOKMARK_BYTES {
            return None;
        }
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
            let path = join_components(&components);
            // This chain is spent, path or not; resume the scan behind all of it.
            scan = tail + 1;
            if let Some(path) = path {
                return Some(path);
            }
        }
        None
    })
}

/// The components joined into an absolute path, if they can all be names: two or
/// more of them, each non-empty and NUL-free. This is the one gate every delivered
/// chain passes, verified or not; the join is lossy UTF-8 under a leading `/`.
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
    use super::{bookmark_chain, event_paths, file_url_path, item_path, percent_decode};
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
    /// Splits on '/', which is why it and the four file-backed tests are Unix-only:
    /// a Windows temp path has no '/' and would yield one unjoinable component.
    #[cfg(unix)]
    fn components_of(path: &std::path::Path) -> Vec<&str> {
        path.to_str().expect("test paths are UTF-8").trim_start_matches('/').split('/').collect()
    }

    /// A chain of byte-exact 0x0101 records delivers the on-disk NFC bytes even
    /// though the blob also carries the NFD-normalized full-path record — the exact
    /// shape of the Ventura failure (#6324).
    #[cfg(unix)]
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
    #[cfg(unix)]
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
        // An unescaped '?' or '#' ends the path, as NSURL.path excludes the query
        // and the fragment; whichever comes first wins.
        assert_eq!(decode(b"file:///tmp/scan.pdf?print=1"), Some("/tmp/scan.pdf".to_owned()));
        assert_eq!(decode(b"file:///tmp/scan.pdf#page=3"), Some("/tmp/scan.pdf".to_owned()));
        assert_eq!(decode(b"file:///tmp/x?a#b"), Some("/tmp/x".to_owned()));
        assert_eq!(decode(b"file:///tmp/x#a?b"), Some("/tmp/x".to_owned()));
        // An escaped %3F or %23 is a literal character of the name, not a delimiter.
        assert_eq!(decode(b"file:///tmp/what%3F.pdf"), Some("/tmp/what?.pdf".to_owned()));
        assert_eq!(decode(b"file:///tmp/c%23.pdf"), Some("/tmp/c#.pdf".to_owned()));
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
    /// intended: raw bytes that name a file — a bookmark's chain or a `'furl'`
    /// decode — win over a differing NSURL path; everything unrecognized keeps
    /// the published crate's NSURL behaviour.
    #[cfg(unix)]
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
        // A 'furl' whose decode names the file wins over a differing NSURL
        // string the same way; a decode that names a directory is no file, so
        // NSURL's answer is delivered. (Every byte but '/' is escaped, so the
        // NFC Á travels as %C3%81.)
        let file_url = |path: &std::path::Path| {
            std::iter::once("file://".to_owned())
                .chain(path.as_os_str().as_encoded_bytes().iter().map(|&byte| {
                    if byte == b'/' { "/".to_owned() } else { format!("%{byte:02X}") }
                }))
                .collect::<String>()
        };
        let furl = u32::from_be_bytes(*b"furl");
        assert_eq!(item_path(furl, file_url(&file).as_bytes(), || Some(nfd_full_path.clone())), Some(file.clone()));
        // NSURL is not consulted at all when the decode names a file.
        assert_eq!(item_path(furl, file_url(&file).as_bytes(), || panic!("NSURL consulted")), Some(file.clone()));
        assert_eq!(
            item_path(furl, file_url(&nfc).as_bytes(), || Some("/from/nsurl".to_owned())),
            Some(PathBuf::from("/from/nsurl"))
        );
        // With nothing to fall back to and nothing path-like in the bytes, the
        // item is dropped, as before.
        assert_eq!(item_path(bookmark, b"garbage", || None), None);

        std::fs::remove_dir_all(&directory).expect("clean up");
    }

    /// The never-drop rule (review on #6334): a bookmark whose chain names no
    /// existing file, with nothing from NSURL either, is still delivered — its
    /// decoded string, unverified — so the app's own open error reports the
    /// failure instead of the launch showing nothing. A NSURL answer still
    /// outranks the unverified chain, and of several unverifiable chains the
    /// first in scan order is the one delivered.
    #[test]
    fn item_path_delivers_an_unverifiable_chain_rather_than_drop_it() {
        let missing = "/definitely/missing/nowhere.pdf";
        let data = blob(&["definitely", "missing", "nowhere.pdf"], missing);
        let bookmark = u32::from_be_bytes(*b"bmrk");
        // Nothing verifies — no chain names a file, NSURL has nothing (the
        // #6324 failure) — and the chain string is still delivered, unverified.
        assert_eq!(item_path(bookmark, &data, || None), Some(PathBuf::from(missing)));
        // A NSURL answer outranks the unverified string.
        assert_eq!(
            item_path(bookmark, &data, || Some("/from/nsurl".to_owned())),
            Some(PathBuf::from("/from/nsurl"))
        );
        // Two separate chains, neither naming a file: the first is delivered. They
        // cannot merge — `blob` ends the first with its 0x0105 full-path record,
        // and `next_record` finds no 0x0101 record there at any of its paddings.
        let mut two = blob(&["definitely", "missing", "first.pdf"], "/definitely/missing/first.pdf");
        for name in ["definitely", "missing", "second.pdf"] {
            push_record(&mut two, 0x0101, name.as_bytes());
        }
        assert_eq!(item_path(bookmark, &two, || None), Some(PathBuf::from("/definitely/missing/first.pdf")));
    }

    /// A `'furl'` decode that names no existing file does not outrank NSURL
    /// (review on #6334): a file reference URL decodes to `/.file/id=…`, which does
    /// not open, while `NSURL.path` resolves it to the file's current path, as the
    /// published crate delivered it; a decode with a trailing '/' names no file
    /// either, and `NSURL.path` drops the slash. With NSURL empty too (a stale
    /// reference, a missing file), the decode is still delivered, unverified.
    #[test]
    fn item_path_takes_nsurl_for_a_file_url_that_names_no_file() {
        let furl = u32::from_be_bytes(*b"furl");
        let reference = b"file:///.file/id=6571367.18547216";
        let resolved = "/Users/someone/\u{00c1}rea fiscal/doc.pdf";
        // NSURL's answer outranks a decode that names no file.
        assert_eq!(item_path(furl, reference, || Some(resolved.to_owned())), Some(PathBuf::from(resolved)));
        // Compared as bytes: `PathBuf` equality ignores a trailing '/'.
        let slashed = b"file:///definitely/missing/nowhere.pdf/";
        assert_eq!(
            item_path(furl, slashed, || Some("/definitely/missing/nowhere.pdf".to_owned())).map(PathBuf::into_os_string),
            Some(std::ffi::OsString::from("/definitely/missing/nowhere.pdf"))
        );
        // Nothing from NSURL: the decode is delivered, unverified (never drop).
        assert_eq!(item_path(furl, reference, || None), Some(PathBuf::from("/.file/id=6571367.18547216")));
        let missing = b"file:///definitely/missing/nowhere.pdf";
        assert_eq!(item_path(furl, missing, || None), Some(PathBuf::from("/definitely/missing/nowhere.pdf")));
    }

    /// A partial multi-file open reports every item: the handler's fold,
    /// [`event_paths`], collects one decode per item, so a mix of a verified file
    /// and an unverifiable one delivers both — the app's open error covers the
    /// failed one instead of it being skipped without notice.
    #[cfg(unix)]
    #[test]
    fn a_multi_item_open_delivers_verified_and_unverified_items_alike() {
        let directory = std::env::temp_dir().join(format!("fmv-multi-{}", std::process::id()));
        let nfc = directory.join("\u{00c1}rea fiscal");
        std::fs::create_dir_all(&nfc).expect("create the NFC-named directory");
        let file = nfc.join("doc.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").expect("create the file");
        let nfd_full_path = file.to_str().expect("UTF-8").replace('\u{00c1}', "A\u{0301}");
        let missing = "/definitely/missing/nowhere.pdf";
        let bookmark = u32::from_be_bytes(*b"bmrk");
        // The handler's own fold over a two-item list: one decode per item,
        // dropped only when it decodes to nothing.
        let items = [
            (bookmark, blob(&components_of(&file), &nfd_full_path)),
            (bookmark, blob(&["definitely", "missing", "nowhere.pdf"], missing)),
        ];
        let list = (u32::from_be_bytes(*b"list"), Vec::new());
        let item_at = |index: isize| items.get(usize::try_from(index - 1).ok()?).cloned();
        let delivered = event_paths(list, 2, item_at, |(t, data)| item_path(*t, data, || None));
        assert_eq!(delivered, [file, PathBuf::from(missing)]);
        std::fs::remove_dir_all(&directory).expect("clean up");
    }

    /// A direct object that is not a list counts no items and is decoded as the
    /// one item (AppleScript `open POSIX file "…"` without braces; the published
    /// crate queued nothing). A list is walked instead — its items, never the list
    /// itself, even when one item is all it holds — and an empty list decodes to
    /// no path, so it still queues nothing.
    #[test]
    fn a_direct_object_that_is_not_a_list_is_decoded_as_the_one_item() {
        let furl = u32::from_be_bytes(*b"furl");
        let bookmark = u32::from_be_bytes(*b"bmrk");
        let list = u32::from_be_bytes(*b"list");
        // (descriptor type, data, what NSURL says of it)
        type Item = (u32, Vec<u8>, Option<&'static str>);
        let decode = |(t, data, nsurl): &Item| item_path(*t, data, || nsurl.map(str::to_owned));
        let missing = "/definitely/missing/nowhere.pdf";
        let direct: Item = (furl, b"file:///definitely/missing/nowhere.pdf".to_vec(), Some(missing));
        assert_eq!(event_paths(direct, 0, |_| None, decode), [PathBuf::from(missing)]);
        // A two-item list: its items, in order; the list itself has no path.
        let two: [Item; 2] = [(furl, b"file:///a/one.pdf".to_vec(), None), (furl, b"file:///a/two.pdf".to_vec(), None)];
        let item_at = |index: isize| two.get(usize::try_from(index - 1).ok()?).cloned();
        assert_eq!(
            event_paths((list, Vec::new(), None), 2, item_at, decode),
            [PathBuf::from("/a/one.pdf"), PathBuf::from("/a/two.pdf")]
        );
        // A one-item list is walked too, and its item delivered once: a list of one
        // 'furl' answers fileURLValue with that file, but the list is not an item.
        let one: [Item; 1] = [(furl, b"file:///a/one.pdf".to_vec(), None)];
        let item_at = |index: isize| one.get(usize::try_from(index - 1).ok()?).cloned();
        assert_eq!(event_paths((list, Vec::new(), Some("/a/one.pdf")), 1, item_at, decode), [PathBuf::from("/a/one.pdf")]);
        // A one-item list of an NFC bookmark (a Finder double-click): the list's own
        // fileURLValue is nil, so only its item's chain delivers the path.
        let finder: [Item; 1] = [(bookmark, blob(&["definitely", "missing", "nowhere.pdf"], missing), None)];
        let item_at = |index: isize| finder.get(usize::try_from(index - 1).ok()?).cloned();
        assert_eq!(event_paths((list, Vec::new(), None), 1, item_at, decode), [PathBuf::from(missing)]);
        // An empty list is its own item and decodes to no path: nothing is queued.
        assert_eq!(event_paths((list, Vec::new(), None), 0, |_| None, decode), Vec::<PathBuf>::new());
    }

    /// With nothing from NSURL, a bookmark in which no chain joins is dropped. The
    /// never-drop step has no gate of its own, so this pins `join_components` — two
    /// or more components, each non-empty and NUL-free — at the `item_path` level,
    /// where no existence check can mask it (`bookmark_chain` alone refuses these
    /// chains for naming no file, gate or not). It also pins that the chain scan
    /// is for bookmarks only: a `'furl'` item's never-drop string is its own URL
    /// decode, never a chain. Garbage still yields `None` and the item is
    /// skipped, as before.
    #[test]
    fn garbage_that_decodes_to_no_path_is_still_dropped() {
        let bookmark = u32::from_be_bytes(*b"bmrk");
        let furl = u32::from_be_bytes(*b"furl");
        // No chain anywhere in the bookmark bytes...
        assert_eq!(item_path(bookmark, b"\xff\x02\xee garbage", || None), None);
        // ...none that joins: a single component is not a path...
        let mut single = b"book\x00\x00\x00\x00".to_vec();
        push_record(&mut single, 0x0101, b"doc.pdf");
        assert_eq!(item_path(bookmark, &single, || None), None);
        // ...nor a chain with a NUL inside a component, which no on-disk name can
        // hold: the join refuses it, so nothing reaches the never-drop step. (An
        // empty component cannot be built: `record_at` refuses a zero length.)
        let mut with_nul = b"book\x00\x00\x00\x00".to_vec();
        push_record(&mut with_nul, 0x0101, b"tmp");
        push_record(&mut with_nul, 0x0101, b"a\x00b");
        push_record(&mut with_nul, 0x0101, b"y.pdf");
        assert_eq!(item_path(bookmark, &with_nul, || None), None);
        // A 'furl' with no '/' after `file://`, or no `file://` at all...
        assert_eq!(item_path(furl, b"file://host", || None), None);
        assert_eq!(item_path(furl, b"http://tmp/x", || None), None);
        assert_eq!(item_path(furl, b"", || None), None);
        // ...and an unknown descriptor type with nothing to fall back to.
        assert_eq!(item_path(u32::from_be_bytes(*b"utf8"), b"some text", || None), None);
        // The chain scan is for bookmarks only: the joinable chain a 'bmrk' item
        // delivers unverified (see the never-drop test) is dropped under any
        // other descriptor type — under 'furl' its bytes are no `file://` URL.
        let chain = blob(&["definitely", "missing", "nowhere.pdf"], "/definitely/missing/nowhere.pdf");
        assert_eq!(item_path(furl, &chain, || None), None);
        assert_eq!(item_path(u32::from_be_bytes(*b"utf8"), &chain, || None), None);
    }
}
