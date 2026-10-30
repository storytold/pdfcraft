use super::{BTreeSet, Ordering, SystemTime};
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
    found.sort_by(|a, b| natural(&a.to_string_lossy(), &b.to_string_lossy()));
    (found, more)
}

/// Whether a file with extension `ext` can join the list: a PDF, or an image or text file that
/// becomes one.
fn combinable(ext: &str) -> bool {
    std::iter::once("pdf").chain(pdfcraft_engine::CONVERTIBLE).any(|c| ext.eq_ignore_ascii_case(c))
}

/// A file's bookmark in the combined PDF: its name without ".pdf", or without the extension of
/// the image or text file it was made from.
pub(super) fn title_of(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && combinable(ext) => stem,
        _ => name,
    }
}

/// File names in the order people expect: case-insensitive, with numbers by value
/// ("scan2" before "scan10").
pub(super) fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits.trim_start_matches('0').to_owned()
                };
                let (x, y) = (take(&mut a), take(&mut b));
                // Without leading zeros, a longer run of digits is a larger number.
                match x.len().cmp(&y.len()).then_with(|| x.cmp(&y)) {
                    Ordering::Equal => {}
                    other => return other,
                }
            }
            (Some(x), Some(y)) => {
                match x.to_lowercase().cmp(y.to_lowercase()) {
                    Ordering::Equal => {}
                    other => return other,
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Where an arrow key goes from `at` among `n` files, `by` at a time (1 in the list; a grid
/// row's length for Up/Down in the grid). It stays put on the first row going up and on the last
/// row going down; a short last row is reached from the row above at its last file.
pub(crate) fn step_to(at: usize, n: usize, by: usize, down: bool) -> usize {
    let by = by.max(1);
    let last = n.saturating_sub(1);
    let at = at.min(last);
    if down {
        if at >= last / by * by { at } else { at.saturating_add(by).min(last) }
    } else if at < by {
        at
    } else {
        at - by
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

/// How long ago `time` was ("5 min ago"); a date after a week. Relative times need no time zone.
pub(crate) fn ago(time: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(time).map(|d| d.as_secs()).unwrap_or(0);
    let (mins, hours, days) = (secs / 60, secs / 3600, secs / 86_400);
    match () {
        _ if mins == 0 => tl!("just now").to_string(),
        _ if hours == 0 => crate::i18n::fmt(tl!("{n} min ago"), &[("n", &mins.to_string())]),
        _ if days == 0 => crate::i18n::fmt(tl!("{n} h ago"), &[("n", &hours.to_string())]),
        _ if days == 1 => tl!("yesterday").to_string(),
        _ if days < 7 => crate::i18n::fmt(tl!("{n} days ago"), &[("n", &days.to_string())]),
        _ => utc_stamp(time).get(..10).unwrap_or_default().to_string(),
    }
}

/// `time` as "2026-10-08 14:05 UTC". A clock before 1970 reads as the epoch; past chrono's year
/// 262143 range the proleptic Gregorian date is kept via the 400-year cycle, not the epoch.
pub(super) fn utc_stamp(time: SystemTime) -> String {
    let secs = time.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days);
    // `{y:04}` rather than `%Y`, which writes a `+` before years past 9999.
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

/// Days since 1970-01-01 → proleptic Gregorian (year, month, day).
///
/// A pre-1970 time already clamps to the epoch (`unwrap_or(0)`), so `days` is an unsigned count
/// that can run past `NaiveDate`'s ±262143-year range, where the old hand-rolled converter still
/// produced a date. The Gregorian calendar repeats every 400 years (146_097 days), so chrono
/// converts the day within one cycle and whole cycles go back into the year (an `i128`, then
/// saturated into `i64`), keeping chrono as the only calendar logic and reproducing the old
/// result exactly. 1970-01-01 is `num_days_from_ce` 719_163.
fn civil_from_days(days: u64) -> (i64, u32, u32) {
    use chrono::Datelike;
    let (cycles, rest) = (days / 146_097, days % 146_097);
    // `rest + 719_163` is 719_163..865_259, always a valid CE day.
    match i32::try_from(rest + 719_163).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt) {
        Some(d) => {
            let y = i128::from(d.year()) + i128::from(cycles) * 400;
            (i64::try_from(y).unwrap_or(i64::MAX), d.month(), d.day())
        }
        None => (1970, 1, 1), // unreachable
    }
}
