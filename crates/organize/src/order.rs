//! The order files go in before they are combined: what they can be sorted by and how two of
//! them compare. Everything that sorts files to combine does it here, so they all agree.

use std::cmp::Ordering;
use std::path::Path;
use std::time::SystemTime;

/// What files are sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSort {
    /// File names in [`natural`] order.
    Name,
    Pages,
    /// Size in bytes.
    Size,
    /// When each file was last modified; files with no known time go last.
    Modified,
    /// [`FileFacts::rank`].
    Rank,
    /// The folder each file came from, one part of the path at a time; files from no folder go
    /// last.
    Folder,
}

impl FileSort {
    /// The sort a headless caller asked for by `name`, or what is wrong with `name`, worded to
    /// follow the option's own name. [`FileSort::Rank`] has no name, because a rank is the caller's.
    pub fn from_name(name: &str) -> Result<FileSort, String> {
        match name {
            "name" => Ok(FileSort::Name),
            "pages" => Ok(FileSort::Pages),
            "size" => Ok(FileSort::Size),
            "modified" => Ok(FileSort::Modified),
            "folder" => Ok(FileSort::Folder),
            other => Err(format!("must be \"name\", \"pages\", \"size\", \"modified\" or \"folder\", not {other:?}")),
        }
    }
}

/// What a file is sorted on. A field only needs a real value when its [`FileSort`] is in use.
#[derive(Clone, Copy, Debug)]
pub struct FileFacts<'a> {
    pub name: &'a str,
    pub pages: usize,
    pub size: usize,
    pub modified: Option<SystemTime>,
    pub folder: Option<&'a Path>,
    /// A place the caller gives the file, such as how far it is from being ready to combine.
    pub rank: u8,
}

/// The positions in `files`, in sorted order: ascending, or descending if not `ascending`. Files
/// that tie go by name in either direction, then keep their order in `files`. Files missing what
/// `by` reads go last in either direction.
pub fn file_order(files: &[FileFacts<'_>], by: FileSort, ascending: bool) -> Vec<usize> {
    let mut order: Vec<(usize, &FileFacts<'_>)> = files.iter().enumerate().collect();
    order.sort_by(|(_, a), (_, b)| compare(a, b, by, ascending));
    order.into_iter().map(|(at, _)| at).collect()
}

fn compare(a: &FileFacts<'_>, b: &FileFacts<'_>, by: FileSort, ascending: bool) -> Ordering {
    let order = match by {
        FileSort::Name => natural(a.name, b.name),
        FileSort::Pages => a.pages.cmp(&b.pages),
        FileSort::Size => a.size.cmp(&b.size),
        FileSort::Rank => a.rank.cmp(&b.rank),
        FileSort::Modified => match (a.modified, b.modified) {
            (Some(x), Some(y)) => x.cmp(&y),
            (x, y) => return x.is_none().cmp(&y.is_none()).then_with(|| natural(a.name, b.name)),
        },
        FileSort::Folder => match (a.folder, b.folder) {
            (Some(x), Some(y)) => natural_path(x, y),
            (x, y) => return x.is_none().cmp(&y.is_none()).then_with(|| natural(a.name, b.name)),
        },
    };
    let order = if ascending { order } else { order.reverse() };
    order.then_with(|| natural(a.name, b.name))
}

/// File names in the order people expect: case-insensitive, with numbers by value
/// ("scan2" before "scan10").
pub fn natural(a: &str, b: &str) -> Ordering {
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

/// Folders in the order people expect: one part of the path at a time, each in [`natural`]
/// order, so a folder comes before the folders inside it and those before its next sibling.
fn natural_path(a: &Path, b: &Path) -> Ordering {
    let (mut a, mut b) = (a.components(), b.components());
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => match natural(&x.as_os_str().to_string_lossy(), &y.as_os_str().to_string_lossy()) {
                Ordering::Equal => {}
                other => return other,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn named(name: &str) -> FileFacts<'_> {
        FileFacts { name, pages: 0, size: 0, modified: None, folder: None, rank: 0 }
    }

    /// The names of `files` in the order `by` gives.
    fn sorted<'a>(files: &[FileFacts<'a>], by: FileSort, ascending: bool) -> Vec<&'a str> {
        file_order(files, by, ascending).into_iter().map(|at| files[at].name).collect()
    }

    #[test]
    fn names_sort_without_case_and_numbers_by_value() {
        let files = [named("scan10"), named("Scan2"), named("a"), named("scan02b")];
        assert_eq!(sorted(&files, FileSort::Name, true), ["a", "Scan2", "scan02b", "scan10"]);
        assert_eq!(sorted(&files, FileSort::Name, false), ["scan10", "scan02b", "Scan2", "a"]);
    }

    /// `sort_by` may panic on an order that isn't total, and names come from strangers.
    #[test]
    fn natural_is_a_total_order_on_awkward_names() {
        let long = "9".repeat(400);
        let names = ["", "a", "A", "a1", "a01", "a10", "a2", "a 1", "a/1", "0", "00", "1", "ß", "ss", "İ", "i", "٣", "\u{0}", long.as_str()];
        for a in names {
            for b in names {
                assert_eq!(natural(a, b), natural(b, a).reverse(), "{a:?} and {b:?}");
                for c in names {
                    if natural(a, b).is_le() && natural(b, c).is_le() {
                        assert!(natural(a, c).is_le(), "{a:?}, {b:?} and {c:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn folders_sort_one_part_of_the_path_at_a_time() {
        let mut folders = vec!["/x/B", "/x/a b", "/x/a/inner", "/x/a", "/x/a10", "/x/a2"];
        folders.sort_by(|a, b| natural_path(Path::new(a), Path::new(b)));
        assert_eq!(folders, ["/x/a", "/x/a/inner", "/x/a b", "/x/a2", "/x/a10", "/x/B"]);
    }

    #[test]
    fn files_missing_what_is_sorted_on_go_last_either_way() {
        let at = |seconds| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds));
        let files = [
            FileFacts { modified: None, folder: None, ..named("z") },
            FileFacts { modified: at(20), folder: Some(Path::new("/y")), ..named("b") },
            FileFacts { modified: None, folder: None, ..named("c") },
            FileFacts { modified: at(10), folder: Some(Path::new("/x")), ..named("d") },
        ];
        assert_eq!(sorted(&files, FileSort::Modified, true), ["d", "b", "c", "z"]);
        assert_eq!(sorted(&files, FileSort::Modified, false), ["b", "d", "c", "z"]);
        assert_eq!(sorted(&files, FileSort::Folder, true), ["d", "b", "c", "z"]);
        assert_eq!(sorted(&files, FileSort::Folder, false), ["b", "d", "c", "z"]);
    }

    #[test]
    fn ties_go_by_name_in_either_direction_then_stay_as_they_were() {
        let files = [
            FileFacts { pages: 2, size: 9, ..named("b") },
            FileFacts { pages: 1, size: 9, ..named("c") },
            FileFacts { pages: 2, size: 7, ..named("a") },
            FileFacts { pages: 1, size: 7, ..named("c") },
        ];
        assert_eq!(sorted(&files, FileSort::Pages, true), ["c", "c", "a", "b"]);
        assert_eq!(sorted(&files, FileSort::Pages, false), ["a", "b", "c", "c"]);
        assert_eq!(file_order(&files, FileSort::Pages, true), [1, 3, 2, 0], "the two files called c keep their order");
        assert_eq!(sorted(&files, FileSort::Size, false), ["b", "c", "a", "c"]);
    }

    #[test]
    fn ranks_sort_lowest_first() {
        let files = [FileFacts { rank: 2, ..named("fine") }, FileFacts { rank: 0, ..named("stuck") }, FileFacts { rank: 1, ..named("warned") }];
        assert_eq!(sorted(&files, FileSort::Rank, true), ["stuck", "warned", "fine"]);
    }

    #[test]
    fn a_headless_caller_names_every_sort_but_rank() {
        assert_eq!(FileSort::from_name("folder"), Ok(FileSort::Folder));
        for wrong in ["rank", "Name", ""] {
            let why = FileSort::from_name(wrong).unwrap_err();
            assert!(why.contains("\"modified\"") && why.contains(&format!("{wrong:?}")), "{why}");
        }
    }

    #[test]
    fn no_files_sort_to_nothing() {
        assert!(file_order(&[], FileSort::Name, true).is_empty());
    }
}
