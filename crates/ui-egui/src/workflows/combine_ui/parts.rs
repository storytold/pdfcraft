use std::collections::BTreeSet;
use std::time::SystemTime;

use chrono::{DateTime, Datelike, Timelike, Utc};

#[cfg(not(target_arch = "wasm32"))]
use super::{MAX_FOLDER_DEPTH, MAX_FOLDER_FILES};

/// The PDFs, images and text files in `dir` (and, if `recursive`, the folders inside it, not
/// following links), in natural order, at most [`MAX_FOLDER_FILES`]; and whether there were more.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn combinable_in(dir: &std::path::Path, recursive: bool) -> (Vec<std::path::PathBuf>, bool) {
    let mut found = Vec::new();
    let mut more = false;
    let mut folders = vec![(dir.to_path_buf(), 0usize)];
    'walk: while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                if recursive && !hidden && depth < MAX_FOLDER_DEPTH {
                    folders.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|e| combinable(&e.to_string_lossy())) && path.is_file() {
                if found.len() >= MAX_FOLDER_FILES {
                    more = true;
                    break 'walk;
                }
                found.push(path);
            }
        }
    }
    found.sort_by(|a, b| craft::docpath::names::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    (found, more)
}

/// Whether a file with extension `ext` can join the list: a PDF, or an image or text file that
/// becomes one.
fn combinable(ext: &str) -> bool {
    std::iter::once("pdf").chain(pdfcraft_engine::CONVERTIBLE).any(|c| ext.eq_ignore_ascii_case(c))
}

/// Whether Combine takes a file by what it is: a PDF (by its header or its name) or an image or
/// text file it converts, as `Session::as_pdf_inline` tells them apart. Not an Office document:
/// converting one runs LibreOffice for seconds, which Combine can't do on the frame thread, so
/// it is left out and named rather than listed as a file that can't be read.
pub(super) fn takes(name: &str, bytes: &[u8]) -> bool {
    match pdfcraft_engine::source_kind(name, bytes) {
        Some(pdfcraft_engine::SourceKind::Office) => false,
        Some(_) => true,
        None => name.to_ascii_lowercase().ends_with(".pdf"),
    }
}

/// A file's bookmark in the combined PDF: its name without ".pdf", or without the extension of
/// the image or text file it was made from.
pub(super) fn title_of(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && combinable(ext) => stem,
        _ => name,
    }
}

/// The order after moving the files `moving` (keeping their order) to `gap` in `ids` as it is
/// (0 = before the first, `ids.len()` = after the last); `None` when nothing would change.
pub(crate) fn moved_order(ids: &[u64], moving: &BTreeSet<u64>, gap: usize) -> Option<Vec<u64>> {
    let gap = gap.min(ids.len());
    // The gap counted among the files that stay.
    let before = ids.get(..gap).map_or(0, |s| s.iter().filter(|id| moving.contains(id)).count());
    let (taken, mut rest): (Vec<u64>, Vec<u64>) = ids.iter().partition(|id| moving.contains(id));
    if taken.is_empty() {
        return None;
    }
    let at = gap.saturating_sub(before).min(rest.len());
    rest.splice(at..at, taken);
    (rest != ids).then_some(rest)
}

/// A page range (1-based: "1-3, 6, 9-7") that takes exactly `pages` (0-based, in that order)
/// from a file of `count` pages; empty (every page) when that is all of them in order.
pub(crate) fn range_of(pages: &[usize], count: usize) -> String {
    if pages.len() == count && pages.iter().enumerate().all(|(i, p)| i == *p) {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(&start) = pages.get(i) {
        // The longest run going up (or down) one page at a time from here.
        let step = |a: usize, b: usize| {
            if b == a.wrapping_add(1) {
                1
            } else if a == b.wrapping_add(1) {
                -1
            } else {
                0
            }
        };
        let dir = pages.get(i + 1).map_or(0, |&next| step(start, next));
        let mut end = i;
        while dir != 0 && pages.get(end + 1).is_some_and(|&next| pages.get(end).is_some_and(|&cur| step(cur, next) == dir)) {
            end += 1;
        }
        let last = pages.get(end).copied().unwrap_or(start);
        parts.push(if end == i { format!("{}", start.saturating_add(1)) } else { format!("{}-{}", start.saturating_add(1), last.saturating_add(1)) });
        i = end + 1;
    }
    parts.join(", ")
}

/// One list entry, for moving and removing single pages: its file (`group`: the parts of one
/// file split around share it) and the pages it takes, in order; `None` when they can't be
/// listed (a locked file, a bad range): such an entry only ever moves whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Part {
    pub(crate) id: u64,
    pub(crate) group: u64,
    pub(crate) pages: Option<Vec<usize>>,
}

/// One page in the list's order, or an entry that moves whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Item {
    Page { group: u64, from: u64, nth: usize, page: usize },
    Whole(u64),
}

/// Every page in the list's order (and the entries that can't be split, whole).
pub(super) fn items_of(parts: &[Part]) -> Vec<Item> {
    let mut items = Vec::new();
    for p in parts {
        match &p.pages {
            Some(pages) => items.extend(pages.iter().enumerate().map(|(nth, &page)| Item::Page { group: p.group, from: p.id, nth, page })),
            None => items.push(Item::Whole(p.id)),
        }
    }
    items
}

/// Entries again from `items`: a file's pages side by side are one entry (its first unused id
/// among them, else a new one from `new_id`); entries that move whole keep theirs.
pub(super) fn parts_of(items: &[Item], parts: &[Part], new_id: &mut dyn FnMut() -> u64) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    let mut used: BTreeSet<u64> = BTreeSet::new();
    let mut run: Vec<Item> = Vec::new();
    let mut close = |run: &mut Vec<Item>, out: &mut Vec<Part>, used: &mut BTreeSet<u64>| {
        let Some(Item::Page { group, .. }) = run.first().copied() else { return };
        let id = run.iter().find_map(|i| match i {
            Item::Page { from, .. } if !used.contains(from) => Some(*from),
            _ => None,
        });
        let id = id.unwrap_or_else(&mut *new_id);
        used.insert(id);
        let pages = run.iter().filter_map(|i| if let Item::Page { page, .. } = i { Some(*page) } else { None }).collect();
        out.push(Part { id, group, pages: Some(pages) });
        run.clear();
    };
    for item in items {
        match item {
            Item::Whole(id) => {
                close(&mut run, &mut out, &mut used);
                if let Some(p) = parts.iter().find(|p| p.id == *id) {
                    used.insert(*id);
                    out.push(p.clone());
                }
            }
            Item::Page { group, .. } => {
                if run.first().is_some_and(|f| !matches!(f, Item::Page { group: g, .. } if g == group)) {
                    close(&mut run, &mut out, &mut used);
                }
                run.push(*item);
            }
        }
    }
    close(&mut run, &mut out, &mut used);
    out
}

/// The entries after moving the pages `moving` (by entry id and place among its pages), in
/// their order, to before page `gap` of the list (counted over every page, an entry that can't be
/// split counting as one; 0 = before the first): a page moved between another file's pages
/// splits its file into parts, and parts of one file brought side by side join. `None` when
/// nothing would change.
pub(crate) fn moved_pages(parts: &[Part], moving: &BTreeSet<(u64, usize)>, gap: usize, new_id: &mut dyn FnMut() -> u64) -> Option<Vec<Part>> {
    let items = items_of(parts);
    let gap = gap.min(items.len());
    let picked = |i: &Item| matches!(i, Item::Page { from, nth, .. } if moving.contains(&(*from, *nth)));
    let before = items.get(..gap).map_or(0, |s| s.iter().filter(|i| picked(i)).count());
    let (taken, mut rest): (Vec<Item>, Vec<Item>) = items.iter().partition(|i| picked(i));
    if taken.is_empty() {
        return None;
    }
    let at = gap.saturating_sub(before).min(rest.len());
    rest.splice(at..at, taken);
    let out = parts_of(&rest, parts, new_id);
    (out != parts).then_some(out)
}

/// The entries after taking out the pages `removing` (by entry id and place among its pages);
/// an entry left with none goes, and parts of one file left side by side join. `None` when
/// nothing would change.
pub(crate) fn removed_pages(parts: &[Part], removing: &BTreeSet<(u64, usize)>, new_id: &mut dyn FnMut() -> u64) -> Option<Vec<Part>> {
    let items: Vec<Item> =
        items_of(parts).into_iter().filter(|i| !matches!(i, Item::Page { from, nth, .. } if removing.contains(&(*from, *nth)))).collect();
    let out = parts_of(&items, parts, new_id);
    (out != parts).then_some(out)
}

/// `time` as "2026-10-08 14:05 UTC". A file time is hostile input (Windows counts from 1601,
/// some filesystems hold far-future times): one before 1970 reads as the epoch, and one past
/// chrono's range as its last moment, on every platform.
pub(super) fn utc_stamp(time: SystemTime) -> String {
    let secs = time.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let at = DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or(DateTime::<Utc>::MAX_UTC);
    // Built from parts rather than `%Y`, which writes a `+` before years past 9999.
    format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", at.year(), at.month(), at.day(), at.hour(), at.minute())
}
