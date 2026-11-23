//! Combine files, in its own tab: the files to combine, in order, each with the pages to take
//! (all, or a range such as "1-3, 6"), its size, when it was modified and anything that stops it
//! being combined. Rows are selected like files in a file manager (click, Ctrl/⌘+click,
//! Shift+click), moved with the toolbar, the keyboard or by dragging, and sorted by clicking a
//! column heading; PDFs dropped on the tab join the list. Every change to the list can be undone.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use craft::behavior::EditHistory;
use craft::behavior::selection::Selection;

use crate::combine_grid::OpenDocuments;

mod actions;
mod files;
mod parts;
mod session;
mod table;
mod view;

use files::CombineList;
pub(crate) use files::Incoming;
pub use files::{CombineFile, CombineView, ConflictPreview, Lock, SortKey};
pub(crate) use files::{CombineKey, Preview, RowAction, UnlockAsk};
pub(crate) use parts::range_of;
#[cfg(test)]
pub(crate) use parts::{Part, moved_order, moved_pages, removed_pages};
#[cfg(test)]
use parts::{title_of, utc_stamp};
pub(crate) use table::columns_from_legacy;
pub(crate) use view::page;

/// The active Combine collection receives navigation left unused by its toolbar.
pub(crate) fn collection_id() -> egui::Id {
    egui::Id::unique("combine_files_collection")
}

/// Files found in a folder are added up to this many at a time.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FOLDER_FILES: usize = 1000;
/// How deep "Add folder and subfolders" looks.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FOLDER_DEPTH: usize = 16;

/// The Combine files tab. It shows in the Home slot (`active == None`) while `focused`.
#[derive(Clone, Debug)]
pub struct CombineTab {
    pub open: bool,
    pub focused: bool,
    /// The column the list is sorted by and whether ascending; cleared when rows are moved.
    pub sort: Option<(SortKey, bool)>,
    /// Undo and redo of the list, its selection and its sort.
    history: EditHistory<CombineList>,
    /// The selected files and pages; the collections of both views bind it.
    pub(crate) selection: Selection<CombineKey>,
    /// The list as it was when a Pages field began an edit, recorded when the edit commits.
    range_edit: Option<CombineList>,
    next_id: u64,
    /// The password dialog of Unlock…, while it shows.
    pub(crate) unlock: Option<UnlockAsk>,
    /// Bumped on every change to the list, so a grid drag that began before one is dropped, and
    /// the duplicate warnings are worked out again. Not part of the history: undo does not rewind it.
    pub(crate) revision: u64,
    /// Exact-content duplicate warnings, shared by both views until the list changes.
    duplicates: Option<(u64, Arc<[bool]>)>,
    /// Files shown as their pages in the grid (a view, not an edit: no undo step).
    pub(crate) expanded: BTreeSet<u64>,
    /// A file shown large (a card's magnifier), while it shows.
    pub(crate) preview: Option<Preview>,
    /// "Leave out files that can't be read?", asked when Combine is clicked with such files in the
    /// list.
    pub(crate) confirm_skip: bool,
    /// The documents the thumbnails keep open for a short while after drawing from them.
    pub(crate) open_documents: Arc<Mutex<OpenDocuments>>,
}

impl Default for CombineTab {
    fn default() -> Self {
        CombineTab {
            open: false,
            focused: false,
            sort: None,
            history: EditHistory::new(&CombineList::default()),
            selection: Selection::default(),
            range_edit: None,
            next_id: 0,
            unlock: None,
            revision: 0,
            duplicates: None,
            expanded: BTreeSet::new(),
            preview: None,
            confirm_skip: false,
            open_documents: Arc::default(),
        }
    }
}

impl CombineTab {
    fn duplicate_warnings(&mut self, files: &[CombineFile]) -> Arc<[bool]> {
        if let Some((revision, flags)) = &self.duplicates
            && *revision == self.revision
        {
            return flags.clone();
        }
        // Only same-named, same-sized files can be repeats. Avoid reading the bytes of
        // unrelated files, and never count a source split into parts as several additions.
        let mut candidates: HashMap<(&str, usize), Vec<usize>> = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            candidates.entry((&f.name, f.source.len())).or_default().push(i);
        }
        let mut twice = vec![false; files.len()];
        for indices in candidates.values() {
            let Some(first) = indices.first().and_then(|i| files.get(*i)) else { continue };
            if indices.iter().all(|i| files.get(*i).is_some_and(|f| f.group == first.group)) {
                continue;
            }
            // Slice keys check exact bytes even when their hashes collide.
            let mut contents: HashMap<&[u8], Vec<usize>> = HashMap::new();
            for &i in indices {
                if let Some(f) = files.get(i) {
                    contents.entry(f.source.as_slice()).or_default().push(i);
                }
            }
            for copies in contents.values() {
                let Some(first) = copies.first().and_then(|i| files.get(*i)) else { continue };
                if copies.iter().any(|i| files.get(*i).is_some_and(|f| f.group != first.group)) {
                    for &i in copies {
                        if let Some(flag) = twice.get_mut(i) {
                            *flag = true;
                        }
                    }
                }
            }
        }
        let flags: Arc<[bool]> = twice.into();
        self.duplicates = Some((self.revision, flags.clone()));
        flags
    }
}

#[cfg(test)]
mod tests;
