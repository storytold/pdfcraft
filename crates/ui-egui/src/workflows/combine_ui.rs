//! Combine files, in its own tab: the files to combine, in order, each with the pages to take
//! (all, or a range such as "1-3, 6"), its size, when it was modified and anything that stops it
//! being combined. Rows are selected like files in a file manager (click, Ctrl/⌘+click,
//! Shift+click), moved with the toolbar, the keyboard or by dragging, and sorted by clicking a
//! column heading; PDFs dropped on the tab join the list. Every change to the list can be undone.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::SystemTime;

use egui::{Align, Color32, CornerRadius, Layout, Modifiers, Rect, Sense, Stroke, vec2};
use egui_extras::{Column, TableBuilder};
use pdfcraft_engine::SourceProblem;

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, widgets};

mod actions;
mod files;
mod parts;
mod session;
mod table;
mod view;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use files::Incoming;
pub use files::{Columns, CombineFile, CombineView, ConflictPreview, Lock, SortKey};
pub(crate) use files::{Preview, RowAction};
use files::{Snapshot, UnlockPrompt};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use parts::ago;
pub(crate) use parts::range_of;
#[cfg(test)]
pub(crate) use parts::{Part, moved_order, moved_pages, removed_pages, step_to};
#[cfg(test)]
use parts::{title_of, utc_stamp};
pub(crate) use view::page;

/// Same reds and ambers as the signature status (`sign_ui`).
pub(crate) const ERROR: Color32 = Color32::from_rgb(0xD7, 0x37, 0x3F);
pub(crate) const WARNING: Color32 = Color32::from_rgb(0xE6, 0x86, 0x19);
/// Undo steps kept for the list (each holds the list itself; the files' bytes are shared).
const HISTORY: usize = 100;

/// Files found in a folder are added up to this many at a time.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FOLDER_FILES: usize = 1000;
/// How deep "Add folder and subfolders" looks.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FOLDER_DEPTH: usize = 16;

/// The Combine files tab. It shows in the Home slot (`active == None`) while `focused`.
#[derive(Clone, Debug, Default)]
pub struct CombineTab {
    pub open: bool,
    pub focused: bool,
    /// The selected rows, by [`CombineFile::id`], so a selection survives sorting and moving.
    pub selected: BTreeSet<u64>,
    /// Where a Shift+click or Shift+arrow range starts, and where it ends.
    anchor: Option<u64>,
    cursor: Option<u64>,
    /// The column the list is sorted by and whether ascending; cleared when rows are moved.
    pub sort: Option<(SortKey, bool)>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The list as it was when a Pages field took the keyboard, recorded on its first change.
    range_edit: Option<Snapshot>,
    next_id: u64,
    /// Bumped to make the table take its column widths afresh (columns moved or reset, or the
    /// window resized while the widths follow it).
    layout: u64,
    /// The width the columns were last fitted to.
    fitted: f32,
    /// The password box of Unlock…, while it shows.
    unlock: Option<UnlockPrompt>,
    /// Bumped on every change to the list, so a grid drag that began before one is dropped.
    pub(crate) revision: u64,
    /// Exact-content duplicate warnings, shared by both views until the list changes.
    duplicates: Option<(u64, Arc<[bool]>)>,
    /// The file the keyboard moved to, for the grid to scroll into view.
    pub(crate) reveal: Option<u64>,
    /// Files per grid row as last drawn (Up/Down move a row in the grid).
    pub(crate) grid_columns: usize,
    /// A grid zoom asked for this frame (toolbar, keys, pinch); the grid applies it once it has
    /// laid out, so it can keep the same cards in view.
    pub(crate) zoom_request: Option<f32>,
    /// The card size slider, which takes the arrow keys while it has the keyboard.
    pub(crate) zoom_slider: Option<egui::Id>,
    /// The card kept in place through a zoom gesture: its file, its place among the file's pages
    /// (0 for the file's own card), how far below the top of the grid it was, and when the grid
    /// last zoomed (seconds).
    pub(crate) zoom_anchor: Option<(u64, usize, f32, f64)>,
    /// Files shown as their pages in the grid (a view, not an edit: no undo step).
    pub(crate) expanded: BTreeSet<u64>,
    /// The pages each expanded file shows, worked out again only when its range changes.
    pub(crate) page_lists: HashMap<u64, crate::combine_grid::PageList>,
    /// The file (by place in the list) at each place of the grid as last drawn: Up and Down step
    /// a row of cards, page cards included.
    pub(crate) grid_places: Vec<usize>,
    /// A file shown large (a card's magnifier), while it shows.
    pub(crate) preview: Option<Preview>,
    /// The card that had the keyboard last frame: Space shows its file large; on any other focused
    /// widget Space is that widget's.
    pub(crate) card_focus: Option<egui::Id>,
    /// That card's file and its place among the file's pages (0 for the file's own card): Space
    /// on a page card shows that page.
    pub(crate) card_focus_at: Option<(u64, usize)>,
    /// Single pages selected in the grid (an expanded file's page cards), by entry id and place
    /// among the entry's pages; the files' selection is empty while any is.
    pub(crate) selected_pages: BTreeSet<(u64, usize)>,
    /// The page a Shift-click selects from.
    pub(crate) page_anchor: Option<(u64, usize)>,
    /// "Leave out files that can't be read?", asked when Combine is clicked with such files in the
    /// list.
    pub(crate) confirm_skip: bool,
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
