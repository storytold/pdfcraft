#[cfg(not(target_arch = "wasm32"))]
use super::MAX_FOLDER_FILES;
use super::files::{AssessError, Assessed, CombineFile, Incoming, Secret, Snapshot, SortKey, UnlockPrompt, assess};
#[cfg(not(target_arch = "wasm32"))]
use super::parts::combinable_in;
use super::parts::{Part, items_of, moved_order, natural, parts_of, range_of, title_of};
use super::{Arc, BTreeSet, CombineTab, HISTORY, Modifiers, PdfCraftApp};

impl PdfCraftApp {
    /// Show the Combine files tab (File ▸ Combine files…), opening it if needed.
    pub fn open_combine_tab(&mut self) {
        self.combine_tab.open = true;
        self.combine_tab.focused = true;
        self.active = None;
    }

    /// Whether the Combine files tab is the one showing.
    pub fn combine_showing(&self) -> bool {
        self.active.is_none() && self.combine_tab.open && self.combine_tab.focused
    }

    /// The selected rows, top to bottom (tests and automation).
    pub fn combine_selection(&self) -> Vec<usize> {
        self.combine_draft.iter().enumerate().filter(|(_, f)| self.combine_tab.selected.contains(&f.id)).map(|(i, _)| i).collect()
    }

    /// Select these rows (tests and automation).
    pub fn select_combine_rows(&mut self, rows: &[usize]) {
        // Files picked: no page stays picked (the trash and Delete act on these files).
        self.combine_tab.selected_pages.clear();
        self.combine_tab.page_anchor = None;
        self.combine_tab.selected = rows.iter().filter_map(|i| self.combine_draft.get(*i)).map(|f| f.id).collect();
        let last = rows.last().and_then(|i| self.combine_draft.get(*i)).map(|f| f.id);
        (self.combine_tab.anchor, self.combine_tab.cursor) = (last, last);
    }

    pub(super) fn combine_snapshot(&self) -> Snapshot {
        Snapshot { files: self.combine_draft.clone(), selected: self.combine_tab.selected.clone(), sort: self.combine_tab.sort }
    }

    fn combine_restore(&mut self, s: Snapshot) {
        self.combine_draft = s.files;
        self.combine_tab.selected = s.selected;
        self.combine_tab.selected_pages.clear();
        self.combine_tab.page_anchor = None;
        self.combine_tab.sort = s.sort;
        self.combine_tab.range_edit = None;
        self.combine_tab.revision = self.combine_tab.revision.wrapping_add(1);
        // A Pages field with the keyboard would otherwise keep typing into the restored list
        // without an undo step.
        if let Some(ctx) = &self.ctx {
            ctx.memory_mut(|m| m.stop_text_input());
        }
    }

    pub(super) fn combine_push_undo(&mut self, before: Snapshot) {
        self.combine_tab.revision = self.combine_tab.revision.wrapping_add(1);
        let undo = &mut self.combine_tab.undo;
        undo.push(before);
        if undo.len() > HISTORY {
            undo.remove(0);
        }
        self.combine_tab.redo.clear();
    }

    /// Remember the list before a change, for Undo.
    pub(super) fn combine_record(&mut self) {
        let now = self.combine_snapshot();
        self.combine_push_undo(now);
        self.combine_tab.range_edit = None;
    }

    pub(crate) fn combine_can_undo(&self, undo: bool) -> bool {
        if undo { !self.combine_tab.undo.is_empty() } else { !self.combine_tab.redo.is_empty() }
    }

    /// Edit ▸ Undo / Redo (⌘Z, ⇧⌘Z) while the Combine files tab shows.
    pub(crate) fn combine_history_step(&mut self, undo: bool) {
        // ⌘Z in the password box is not about the list.
        if self.combine_tab.unlock.is_some() {
            return;
        }
        let (from, to) =
            if undo { (&mut self.combine_tab.undo, &mut self.combine_tab.redo) } else { (&mut self.combine_tab.redo, &mut self.combine_tab.undo) };
        let Some(state) = from.pop() else { return };
        let now = Snapshot { files: self.combine_draft.clone(), selected: self.combine_tab.selected.clone(), sort: self.combine_tab.sort };
        to.push(now);
        self.combine_restore(state);
    }

    pub(super) fn combine_sort(&mut self, key: SortKey, ascending: bool) {
        let list = &mut self.combine_draft;
        let mut keyed: Vec<(u8, CombineFile)> = list.drain(..).map(|mut f| (f.severity(), f)).collect();
        keyed.sort_by(|(sa, a), (sb, b)| {
            let order = match key {
                SortKey::Name => natural(&a.name, &b.name),
                SortKey::Pages => a.pages.cmp(&b.pages),
                SortKey::Size => a.bytes.len().cmp(&b.bytes.len()),
                SortKey::Modified => match (a.modified, b.modified) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    // Unknown times last, whichever the direction.
                    (x, y) => return x.is_none().cmp(&y.is_none()).then_with(|| natural(&a.name, &b.name)),
                },
                SortKey::Warnings => sa.cmp(sb),
            };
            let order = if ascending { order } else { order.reverse() };
            order.then_with(|| natural(&a.name, &b.name))
        });
        *list = keyed.into_iter().map(|(_, f)| f).collect();
        self.combine_tab.sort = Some((key, ascending));
    }

    /// The list as parts, for page edits: each entry's pages, or `None` for one that can't be
    /// split (locked, unreadable, a bad range).
    pub(super) fn combine_parts(&self) -> Vec<Part> {
        self.combine_draft
            .iter()
            .map(|f| Part {
                id: f.id,
                group: f.group,
                // (No page listed, e.g. its pages couldn't be counted: it moves whole, never lost.)
                pages: (f.lock.is_none() && f.problem.is_none()).then(|| f.pages_taken().ok()).flatten().filter(|p| !p.is_empty()),
            })
            .collect()
    }

    /// Make the list `parts`, as one undo step: entries kept by id, new parts made from their
    /// file's other entries, each taking exactly its pages (its range rewritten). The parts of a
    /// file shown as its pages stay shown so; the page selection is let go of.
    pub(super) fn combine_set_parts(&mut self, parts: Vec<Part>) {
        self.combine_record();
        self.combine_put_parts(parts);
        let tab = &mut self.combine_tab;
        tab.selected_pages.clear();
        tab.page_anchor = None;
        // An order made by hand isn't sorted any more.
        tab.sort = None;
    }

    /// Join the parts of a file that have come to be side by side (no undo step of its own: it
    /// belongs to the change that brought them together).
    pub(super) fn combine_join_parts(&mut self) {
        let parts = self.combine_parts();
        // Nothing to join unless some file is in more than one part.
        let groups: BTreeSet<u64> = parts.iter().map(|p| p.group).collect();
        if groups.len() == parts.len() {
            return;
        }
        let next = &mut self.combine_tab.next_id;
        let mut new_id = || {
            let id = *next;
            *next = next.wrapping_add(1);
            id
        };
        let joined = parts_of(&items_of(&parts), &parts, &mut new_id);
        if joined != parts {
            self.combine_put_parts(joined);
        }
    }

    /// Make the list `parts`: entries kept by id, new parts made from their file's other entries,
    /// each taking exactly its pages (its range rewritten only when they differ).
    fn combine_put_parts(&mut self, parts: Vec<Part>) {
        let old = std::mem::take(&mut self.combine_draft);
        let expanded: BTreeSet<u64> = old.iter().filter(|f| self.combine_tab.expanded.contains(&f.id)).map(|f| f.group).collect();
        for p in parts {
            let Some(base) = old.iter().find(|f| f.id == p.id).or_else(|| old.iter().find(|f| f.group == p.group)) else { continue };
            let mut f = base.clone();
            f.id = p.id;
            // (A range that already takes these pages stays as the user typed it.)
            if let Some(pages) = &p.pages
                && f.pages_taken().ok().as_ref() != Some(pages)
            {
                f.range = range_of(pages, f.pages);
                f.checked = None;
            }
            if expanded.contains(&f.group) {
                self.combine_tab.expanded.insert(f.id);
            }
            self.combine_draft.push(f);
        }
        let ids: BTreeSet<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        self.combine_tab.selected.retain(|id| ids.contains(id));
    }

    /// A page card clicked: that page selected (Ctrl/⌘ toggles it, Shift selects every page from
    /// the last one clicked, in the list's order); the files' selection is let go of.
    pub(super) fn combine_click_page(&mut self, id: u64, nth: usize, m: Modifiers) {
        let page = (id, nth);
        let tab = &mut self.combine_tab;
        tab.selected.clear();
        tab.cursor = Some(id);
        if m.shift
            && let Some(from) = tab.page_anchor
        {
            // Every page between the two, as the grid shows them.
            let order: Vec<(u64, usize)> = self
                .combine_draft
                .iter()
                .filter(|f| tab.expanded.contains(&f.id))
                .flat_map(|f| (0..f.pages_taken().map_or(0, |p| p.len())).map(move |n| (f.id, n)))
                .collect();
            let (a, b) = (order.iter().position(|p| *p == from), order.iter().position(|p| *p == page));
            if let (Some(a), Some(b)) = (a, b) {
                if !m.command {
                    tab.selected_pages.clear();
                }
                tab.selected_pages.extend(order.get(a.min(b)..=a.max(b)).unwrap_or_default().iter().copied());
                return;
            }
        }
        if m.command {
            if !tab.selected_pages.remove(&page) {
                tab.selected_pages.insert(page);
            }
        } else {
            tab.selected_pages = BTreeSet::from([page]);
        }
        tab.page_anchor = Some(page);
    }

    /// Move `moving` to `gap`, as one undo step; a move that changes nothing leaves the list,
    /// its sort and the history alone.
    pub(super) fn combine_move_to_gap(&mut self, moving: BTreeSet<u64>, gap: usize) {
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        let Some(order) = moved_order(&ids, &moving, gap) else { return };
        self.combine_record();
        let rank: std::collections::HashMap<u64, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        self.combine_draft.sort_by_key(|f| rank.get(&f.id).copied().unwrap_or(usize::MAX));
        self.combine_tab.selected = moving.into_iter().filter(|id| rank.contains_key(id)).collect();
        self.combine_tab.sort = None;
    }

    /// Close the Combine files tab and forget its list; the last document shows, if any.
    pub fn close_combine_tab(&mut self) {
        let showing = self.combine_showing();
        // (Revisions keep counting: a drag begun before can never match a list made after.)
        let revision = self.combine_tab.revision.wrapping_add(1);
        self.combine_tab = CombineTab { revision, ..CombineTab::default() };
        self.combine_draft.clear();
        self.combine_thumbs.clear();
        if showing {
            self.active = self.views.len().checked_sub(1);
        }
    }

    /// Add picked files to the Combine files list (and show it).
    pub(crate) fn stage_combine(&mut self, files: Vec<(String, Vec<u8>)>) {
        let incoming =
            files.into_iter().map(|(name, bytes)| Incoming { name, bytes: Arc::new(bytes), modified: None, note: None, open_doc: None }).collect();
        self.stage_combine_with(incoming);
    }

    /// Add files to the Combine files list, with what can be known about them up front.
    pub(crate) fn stage_combine_with(&mut self, files: Vec<Incoming>) {
        let mut added = Vec::new();
        for Incoming { name, bytes, modified, note, open_doc } in files {
            // Images and text files become PDFs as they join the list (#569): from then on they
            // are ordinary files, with pages, thumbnails and page ranges.
            // A file that can't be read joins the list all the same, with why (the Warnings
            // column, its card); Combine asks before leaving it out.
            let unreadable = |e: &str| Assessed {
                pages: 0,
                sizes: Arc::from(Vec::new()),
                problem: Some(crate::i18n::fmt(tl!("Can't be read: {e}"), &[("e", e)])),
                lock: None,
                notes: Vec::new(),
            };
            let source = bytes.clone();
            let (bytes, assessed) = match self.session.as_pdf(&name, &bytes) {
                Ok(pdf) => {
                    let assessed = assess(&pdf, None);
                    (pdf, assessed)
                }
                Err(e) => {
                    // (The engine's message names the file, which the row already shows.)
                    let e = match e {
                        pdfcraft_engine::EditError::Source(m) => m,
                        other => other.to_string(),
                    };
                    let why = e.strip_prefix(&format!("{name}: ")).unwrap_or(&e).to_string();
                    (bytes, Ok(unreadable(&why)))
                }
            };
            let assessed = match assessed {
                Ok(a) => a,
                Err(AssessError::Unreadable(e)) => unreadable(&e),
                Err(AssessError::WrongPassword) => {
                    self.notify_fmt("Couldn't add {name}: {e}", &[("name", &name), ("e", "the password is wrong")]);
                    continue;
                }
            };
            let mut f = CombineFile {
                id: 0,
                group: 0,
                name,
                bytes,
                source,
                pages: 0,
                range: String::new(),
                modified,
                problem: None,
                lock: None,
                password: None,
                sizes: Arc::from(Vec::new()),
                auth: 0,
                notes: Vec::new(),
                origin_note: note,
                open_doc,
                checked: None,
            };
            f.take(assessed, None);
            added.push(f);
        }
        if !added.is_empty() {
            self.combine_record();
            for mut f in added {
                f.id = self.combine_tab.next_id;
                f.group = f.id;
                self.combine_tab.next_id = self.combine_tab.next_id.wrapping_add(1);
                self.combine_draft.push(f);
            }
            // A sorted list stays sorted; new files simply join the end otherwise.
            if let Some((key, ascending)) = self.combine_tab.sort {
                self.combine_sort(key, ascending);
            }
        }
        self.open_combine_tab();
    }

    /// Open Unlock…'s password box for `rows`, at `at`.
    pub(super) fn combine_ask_password(&mut self, rows: Vec<u64>, at: egui::Pos2) {
        if rows.is_empty() {
            return;
        }
        self.combine_tab.unlock = Some(UnlockPrompt { rows, input: Secret::default(), show: false, error: None, at, focused: false });
    }

    /// Try `password` on the protected files `rows` (as one undo step). Returns false when it
    /// opened none of them.
    pub fn combine_unlock(&mut self, rows: &[u64], password: &str) -> bool {
        // A file split into parts is unlocked as one: each file once, all its parts alike.
        let groups: BTreeSet<u64> =
            rows.iter().filter_map(|id| self.combine_draft.iter().find(|f| f.id == *id && f.lock.is_some())).map(|f| f.group).collect();
        let mut accepted = Vec::new();
        for group in &groups {
            let Some(f) = self.combine_draft.iter().find(|f| f.group == *group) else { continue };
            if let Ok(a) = assess(&f.bytes, Some(password)) {
                accepted.push((*group, a));
            }
        }
        if accepted.is_empty() {
            return false;
        }
        self.combine_record();
        let tried = groups.len();
        let mut unlocked = 0;
        for (group, a) in accepted {
            let mut auth = None;
            for f in self.combine_draft.iter_mut().filter(|f| f.group == group) {
                f.take(a.clone(), Some(Secret(password.to_owned())));
                // Read once: one way of reading for all its parts (their thumbnails are shared).
                f.auth = *auth.get_or_insert(f.auth);
            }
            unlocked += usize::from(self.combine_draft.iter().any(|f| f.group == group && f.lock.is_none()));
        }
        if tried > 1 {
            self.notify_fmt("Unlocked {n} of {m} files", &[("n", &unlocked.to_string()), ("m", &tried.to_string())]);
        }
        true
    }

    /// Unlock… for the files the user named in tests and automation (by row).
    pub fn combine_unlock_rows(&mut self, rows: &[usize], password: &str) -> bool {
        let ids: Vec<u64> = rows.iter().filter_map(|i| self.combine_draft.get(*i)).map(|f| f.id).collect();
        self.combine_unlock(&ids, password)
    }

    /// Add every PDF, image and text file in a folder the user picks (and its subfolders, if
    /// `recursive`).
    pub fn combine_add_folder(&mut self, recursive: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let title = if recursive { tl!("Add folder and subfolders") } else { tl!("Add folder") };
            let dialog = rfd::AsyncFileDialog::new().set_title(title);
            self.ask_one(crate::pickers::Ask::Folder(dialog), None, move |app, dir| app.add_folder_to_combine(&dir, recursive));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = recursive;
    }

    /// Add the PDFs, images and text files in `dir` to the Combine files list, as one undo step.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn add_folder_to_combine(&mut self, dir: &std::path::Path, recursive: bool) {
        let (paths, more) = combinable_in(dir, recursive);
        let folder = dir.file_name().map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        if paths.is_empty() {
            self.notify_fmt("No PDFs, images or text files found in {folder}", &[("folder", &folder)]);
            self.open_combine_tab();
            return;
        }
        self.use_paths(crate::files::FilePurpose::Combine, &paths);
        if more {
            self.notify_fmt("Added the first {n} files in {folder}", &[("n", &MAX_FOLDER_FILES.to_string()), ("folder", &folder)]);
        }
    }

    /// Add documents open in tabs (by tab index), as they are now; those already in the list
    /// stay out (#1356). The same document, not the same name: two files called alike are two.
    pub(crate) fn add_open_documents(&mut self, tabs: &[usize]) {
        let mut incoming = Vec::new();
        for &i in tabs {
            let Some(doc) = self.views.get(i).and_then(|v| self.session.get(v.id)) else { continue };
            if self.combine_draft.iter().any(|f| f.open_doc == Some(doc.id)) {
                continue;
            }
            #[cfg(not(target_arch = "wasm32"))]
            let modified = if doc.dirty { None } else { doc.path.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()) };
            #[cfg(target_arch = "wasm32")]
            let modified = None;
            let note = doc.dirty.then(|| tl!("Includes unsaved changes").to_string());
            let name = if doc.name.to_ascii_lowercase().ends_with(".pdf") { doc.name.clone() } else { format!("{}.pdf", doc.name) };
            incoming.push(Incoming { name, bytes: doc.bytes.clone(), modified, note, open_doc: Some(doc.id) });
        }
        self.stage_combine_with(incoming);
    }

    /// Combine the listed files into a new, unsaved document; the tab makes way for it.
    /// Files that can't be read are left out (the user was asked).
    pub fn combine_staged(&mut self) {
        let files: Vec<&CombineFile> = self.combine_draft.iter().filter(|f| !f.unreadable()).collect();
        if files.is_empty() {
            return;
        }
        // Each file once, however many parts it is in.
        let count = files.iter().map(|f| f.group).collect::<BTreeSet<u64>>().len();
        let groups: Vec<u64> = files.iter().map(|f| f.group).collect();
        let sources: Vec<(String, Arc<Vec<u8>>, Option<String>)> =
            files.iter().map(|f| (title_of(&f.name).to_string(), f.bytes.clone(), Some(f.range.clone()).filter(|r| !r.trim().is_empty()))).collect();
        let passwords: Vec<Option<&str>> = files.iter().map(|f| f.password.as_ref().map(|p| p.0.as_str())).collect();
        match self.session.combine_grouped(&sources, &groups, &passwords) {
            Ok(bytes) => {
                self.close_combine_tab();
                let message = crate::i18n::fmt(tl!("Combined {n} files"), &[("n", &count.to_string())]);
                self.open_created("Combined.pdf", bytes, &message)
            }
            // The list stays, so the user can fix it.
            Err(e) => self.notify_fmt("Couldn't combine files: {e}", &[("e", &e.to_string())]),
        }
    }
}
