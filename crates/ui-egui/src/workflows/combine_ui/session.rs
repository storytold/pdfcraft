#[cfg(not(target_arch = "wasm32"))]
use super::MAX_FOLDER_FILES;
use super::files::{AssessError, Assessed, CombineFile, CombineKey, CombineList, Incoming, SortKey, UnlockAsk, assess};
#[cfg(not(target_arch = "wasm32"))]
use super::parts::combinable_in;
use super::parts::{Part, items_of, moved_order, parts_of, range_of, takes, title_of};
use super::{Arc, BTreeSet, CombineTab};
use crate::PdfCraftApp;
use craft::behavior::edit_history::Step;
use craft::behavior::selection::Selection;
use craft::behavior::value::{EditEvent, Mixed};
use craft::docpath::names::natural_cmp;
#[cfg(not(target_arch = "wasm32"))]
use craft::host::PickRequest;
use craft::ui::patterns::form_dialogs::Secret;

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
        let selected = self.combine_file_ids();
        self.combine_draft.iter().enumerate().filter(|(_, f)| selected.contains(&f.id)).map(|(i, _)| i).collect()
    }

    /// Select these rows (tests and automation).
    pub fn select_combine_rows(&mut self, rows: &[usize]) {
        // Files picked: no page stays picked (the trash and Delete act on these files).
        let items: Vec<CombineKey> = rows.iter().filter_map(|i| self.combine_draft.get(*i)).map(|f| CombineKey::File(f.id)).collect();
        let last = items.last().copied();
        self.combine_tab.selection = Selection::new(items, last, last);
    }

    /// The selected files, by [`CombineFile::id`].
    pub(crate) fn combine_file_ids(&self) -> BTreeSet<u64> {
        self.combine_tab
            .selection
            .items
            .iter()
            .filter_map(|k| match k {
                CombineKey::File(id) => Some(*id),
                CombineKey::Page(..) => None,
            })
            .collect()
    }

    /// The selected pages, by entry id and place among the entry's pages.
    pub(crate) fn combine_page_keys(&self) -> BTreeSet<(u64, usize)> {
        self.combine_tab
            .selection
            .items
            .iter()
            .filter_map(|k| match k {
                CombineKey::Page(id, nth) => Some((*id, *nth)),
                CombineKey::File(_) => None,
            })
            .collect()
    }

    /// One kind of selection at a time: keeps the keys of the kind of the latest pick (the lead, or
    /// the last key when there is no lead), so the trash never takes pages (or files) the user no
    /// longer means. The collections call it when they report a selection change.
    pub(crate) fn combine_keep_one_kind(&mut self) {
        let is_page = |k: &CombineKey| matches!(k, CombineKey::Page(..));
        let sel = &mut self.combine_tab.selection;
        let Some(kind) = sel.lead.as_ref().or(sel.items.last()).map(is_page) else { return };
        sel.items.retain(|k| is_page(k) == kind);
        if sel.anchor.as_ref().is_some_and(|k| is_page(k) != kind) {
            sel.anchor = None;
        }
    }

    /// The list, its selection and its sort: what one undo step holds.
    pub(super) fn combine_list(&self) -> CombineList {
        CombineList { files: self.combine_draft.clone(), selected: self.combine_tab.selection.items.clone(), sort: self.combine_tab.sort }
    }

    /// Put `list` back (undo, redo).
    fn combine_set_list(&mut self, ctx: &egui::Context, list: CombineList) {
        let ids: BTreeSet<u64> = list.files.iter().map(|f| f.id).collect();
        let lead = list.selected.last().copied();
        self.combine_draft = list.files;
        self.combine_tab.selection = Selection::new(list.selected, lead, lead);
        // Page positions can name different pages after restoring a list.
        self.combine_drop_page_keys(None);
        self.combine_tab.sort = list.sort;
        self.combine_tab.expanded.retain(|id| ids.contains(id));
        self.combine_tab.revision = self.combine_tab.revision.wrapping_add(1);
        // A Pages field with the keyboard would otherwise keep typing into the restored list
        // without an undo step.
        ctx.memory_mut(|m| m.stop_text_input());
    }

    /// Run `change` on the list as one undo step named `label`; a change that leaves the list as it
    /// was records nothing.
    pub(super) fn combine_edit(&mut self, label: &str, change: impl FnOnce(&mut Self)) {
        let before = self.combine_list();
        change(self);
        let after = self.combine_list();
        self.combine_tab.revision = self.combine_tab.revision.wrapping_add(1);
        self.combine_tab.history.record(&EditEvent::set(Mixed::Same(before), after, Arc::from(label), None));
    }

    /// An event of a Pages field: the list as it was when the edit began becomes one undo step when
    /// the edit commits.
    pub(crate) fn combine_range_edit(&mut self, event: &EditEvent<String>) {
        match event {
            EditEvent::Begin { .. } => self.combine_tab.range_edit = Some(self.combine_list()),
            EditEvent::Commit { .. } => {
                if let Some(before) = self.combine_tab.range_edit.take() {
                    let after = self.combine_list();
                    self.combine_tab.revision = self.combine_tab.revision.wrapping_add(1);
                    self.combine_tab.history.record(&EditEvent::set(Mixed::Same(before), after, Arc::from(tl!("Pages")), None));
                }
            }
            EditEvent::Cancel { .. } => self.combine_tab.range_edit = None,
            _ => {}
        }
    }

    pub(crate) fn combine_can_undo(&self, undo: bool) -> bool {
        if undo { self.combine_tab.history.can_undo() } else { self.combine_tab.history.can_redo() }
    }

    /// Edit ▸ Undo / Redo (⌘Z, ⇧⌘Z) while the Combine files tab shows.
    pub(crate) fn combine_history_step(&mut self, ctx: &egui::Context, undo: bool) {
        // ⌘Z in the password box is not about the list, and a Pages field being edited has its own
        // undo; the history never sees that edit's `Begin`, so `is_open` can't say so.
        if self.combine_tab.unlock.is_some() || self.combine_tab.range_edit.is_some() {
            return;
        }
        let mut now = self.combine_list();
        let step = if undo { Step::Undo } else { Step::Redo };
        if self.combine_tab.history.step(&mut now, step) {
            self.combine_set_list(ctx, now);
        }
    }

    pub(super) fn combine_sort(&mut self, key: SortKey, ascending: bool) {
        let list = &mut self.combine_draft;
        let mut keyed: Vec<(u8, CombineFile)> = list.drain(..).map(|mut f| (f.severity(), f)).collect();
        keyed.sort_by(|(sa, a), (sb, b)| {
            let order = match key {
                SortKey::Name => natural_cmp(&a.name, &b.name),
                SortKey::Pages => a.pages.cmp(&b.pages),
                SortKey::Size => a.bytes.len().cmp(&b.bytes.len()),
                SortKey::Modified => match (a.modified, b.modified) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    // Unknown times last, whichever the direction.
                    (x, y) => return x.is_none().cmp(&y.is_none()).then_with(|| natural_cmp(&a.name, &b.name)),
                },
                SortKey::Warnings => sa.cmp(sb),
            };
            let order = if ascending { order } else { order.reverse() };
            order.then_with(|| natural_cmp(&a.name, &b.name))
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

    /// Make the list `parts`, as one undo step named `label`: entries kept by id, new parts made
    /// from their file's other entries, each taking exactly its pages (its range rewritten). The
    /// parts of a file shown as its pages stay shown so; the page selection is let go of.
    pub(super) fn combine_set_parts(&mut self, parts: Vec<Part>, label: &str) {
        self.combine_edit(label, |app| {
            app.combine_put_parts(parts);
            app.combine_drop_page_keys(None);
            // An order made by hand isn't sorted any more.
            app.combine_tab.sort = None;
        });
    }

    /// Let go of the selected pages of the entry `only`, or of every entry (the files stay selected).
    pub(super) fn combine_drop_page_keys(&mut self, only: Option<u64>) {
        let gone = |k: &CombineKey| matches!(k, CombineKey::Page(id, _) if only.is_none_or(|o| o == *id));
        let sel = &mut self.combine_tab.selection;
        sel.items.retain(|k| !gone(k));
        sel.lead = sel.lead.filter(|k| !gone(k));
        sel.anchor = sel.anchor.filter(|k| !gone(k));
    }

    /// Join the parts of a file that have come to be side by side (it belongs to the undo step of
    /// the change that brought them together, so call it inside that change).
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
        let alive = |k: &CombineKey| ids.contains(&k.entry());
        let sel = &mut self.combine_tab.selection;
        sel.items.retain(alive);
        sel.lead = sel.lead.filter(alive);
        sel.anchor = sel.anchor.filter(alive);
    }

    /// Select exactly the files `ids`, in the list's order, keeping the lead and anchor that stay
    /// selected.
    fn combine_select_files(&mut self, ids: &BTreeSet<u64>) {
        let items: Vec<CombineKey> = self.combine_draft.iter().filter(|f| ids.contains(&f.id)).map(|f| CombineKey::File(f.id)).collect();
        let sel = &mut self.combine_tab.selection;
        let (lead, anchor) = (sel.lead.filter(|k| items.contains(k)), sel.anchor.filter(|k| items.contains(k)));
        *sel = Selection::new(items, lead, anchor);
    }

    /// Move `moving` to `gap`, as one undo step; a move that changes nothing leaves the list,
    /// its sort and the history alone.
    pub(super) fn combine_move_to_gap(&mut self, moving: BTreeSet<u64>, gap: usize) {
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        let Some(order) = moved_order(&ids, &moving, gap) else { return };
        self.combine_edit(tl!("Move"), |app| {
            let rank: std::collections::HashMap<u64, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
            app.combine_draft.sort_by_key(|f| rank.get(&f.id).copied().unwrap_or(usize::MAX));
            app.combine_select_files(&moving);
            app.combine_tab.sort = None;
            app.combine_join_parts();
        });
    }

    /// Close the Combine files tab and forget its list; the last document shows, if any.
    pub fn close_combine_tab(&mut self, ctx: &egui::Context) {
        let showing = self.combine_showing();
        crate::combine_grid::release_thumbnails(ctx, &self.combine_tab);
        // (Revisions keep counting: a drag begun before can never match a list made after.)
        let revision = self.combine_tab.revision.wrapping_add(1);
        self.combine_tab = CombineTab { revision, ..CombineTab::default() };
        self.combine_draft.clear();
        if showing {
            self.active = self.views.len().checked_sub(1);
        }
    }

    /// Add picked files to the Combine files list (and show it).
    pub(crate) fn stage_combine(&mut self, files: Vec<(String, Vec<u8>)>) {
        self.stage_combine_with(files.into_iter().map(|(name, bytes)| Incoming::new(name, bytes)).collect());
    }

    /// Files dropped on the Combine files tab join the end of its list in the order dropped, as
    /// one undo step. A drop isn't filtered like a pick, so files of a kind Combine doesn't take
    /// are left out, and named.
    pub(crate) fn add_dropped_to_combine(&mut self, files: Vec<Incoming>) {
        let (taken, refused): (Vec<_>, Vec<_>) = files.into_iter().partition(|f| takes(&f.name, &f.bytes));
        if !refused.is_empty() {
            let names: Vec<&str> = refused.iter().map(|f| f.name.as_str()).collect();
            self.notify_fmt("Not added: {names}. Combine takes PDFs, images and text files.", &[("names", &names.join(", "))]);
        }
        if !taken.is_empty() {
            self.stage_combine_with(taken);
        }
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
            let (bytes, assessed) = match self.session.as_pdf_inline(&name, &bytes) {
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
            self.combine_edit(tl!("Add files…"), |app| {
                for mut f in added {
                    f.id = app.combine_tab.next_id;
                    f.group = f.id;
                    app.combine_tab.next_id = app.combine_tab.next_id.wrapping_add(1);
                    app.combine_draft.push(f);
                }
                // A sorted list stays sorted; new files simply join the end otherwise.
                if let Some((key, ascending)) = app.combine_tab.sort {
                    app.combine_sort(key, ascending);
                }
            });
        }
        self.open_combine_tab();
    }

    /// Open Unlock…'s password dialog for `rows`.
    pub(super) fn combine_ask_password(&mut self, rows: Vec<u64>) {
        if rows.is_empty() {
            return;
        }
        self.combine_tab.unlock = Some(UnlockAsk { rows, error: None });
    }

    /// Try `password` on the protected files `rows` (as one undo step). Returns false when it
    /// opened none of them; the open dialog then shows why.
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
            if let Some(ask) = &mut self.combine_tab.unlock {
                ask.error = Some(tl!("Wrong password").to_string());
            }
            return false;
        }
        let tried = groups.len();
        let mut unlocked = 0;
        self.combine_edit(tl!("Unlock"), |app| {
            for (group, a) in accepted {
                let mut auth = None;
                for f in app.combine_draft.iter_mut().filter(|f| f.group == group) {
                    f.take(a.clone(), Some(Secret::new(password.to_owned())));
                    // Read once: one way of reading for all its parts (their thumbnails are shared).
                    f.auth = *auth.get_or_insert(f.auth);
                }
                unlocked += usize::from(app.combine_draft.iter().any(|f| f.group == group && f.lock.is_none()));
            }
        });
        self.combine_tab.unlock = None;
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

            self.pick_first(PickRequest::folder("combine.folder").title(title), None, move |app, ctx, dir| {
                app.add_folder_to_combine(ctx, &dir, recursive)
            });
        }
        #[cfg(target_arch = "wasm32")]
        let _ = recursive;
    }

    /// Add the files at `paths` to the Combine files list and show it: `pdfcraft --combine` and
    /// File Explorer's "Combine files in PdfCraft" (#585), where each selected file arrives as a
    /// launch of its own and the running PdfCraft collects them here. Nothing to add shows the
    /// tab all the same.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn combine_paths(&mut self, ctx: &egui::Context, paths: &[String]) {
        // A file still opening from before (another launch's Open), or waiting for its password,
        // opens in the background, so finishing doesn't hide the tab shown here; a later Open
        // still comes to the front. An open that something waits on keeps its focus: a chosen
        // tool, a link's destination or a recovery runs on the document once it is in front.
        for pending in &mut self.pending_opens {
            if pending.then.is_none() && pending.after_open.is_none() && pending.recovered.is_none() {
                pending.activate = false;
            }
        }
        if let Some(prompt) = self.password_prompt.as_mut()
            && prompt.then.is_none()
            && prompt.after_open.is_none()
            && prompt.recovered.is_none()
        {
            prompt.activate = false;
        }
        self.open_combine_tab();
        let paths: Vec<std::path::PathBuf> = paths.iter().map(std::path::PathBuf::from).collect();
        self.use_paths(ctx, crate::files::FilePurpose::Combine, &paths);
    }

    /// Add the PDFs, images and text files in `dir` to the Combine files list, as one undo step.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn add_folder_to_combine(&mut self, ctx: &egui::Context, dir: &std::path::Path, recursive: bool) {
        let (paths, more) = combinable_in(dir, recursive);
        let folder = dir.file_name().map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        if paths.is_empty() {
            self.notify_fmt("No PDFs, images or text files found in {folder}", &[("folder", &folder)]);
            self.open_combine_tab();
            return;
        }
        self.use_paths(ctx, crate::files::FilePurpose::Combine, &paths);
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
    pub fn combine_staged(&mut self, ctx: &egui::Context) {
        let files: Vec<&CombineFile> = self.combine_draft.iter().filter(|f| !f.unreadable()).collect();
        if files.is_empty() {
            return;
        }
        // Each file once, however many parts it is in.
        let count = files.iter().map(|f| f.group).collect::<BTreeSet<u64>>().len();
        let groups: Vec<u64> = files.iter().map(|f| f.group).collect();
        let sources: Vec<(String, Arc<Vec<u8>>, Option<String>)> =
            files.iter().map(|f| (title_of(&f.name).to_string(), f.bytes.clone(), Some(f.range.clone()).filter(|r| !r.trim().is_empty()))).collect();
        let passwords: Vec<Option<&str>> = files.iter().map(|f| f.password()).collect();
        match self.session.combine_grouped(&sources, &groups, &passwords) {
            Ok(bytes) => {
                self.close_combine_tab(ctx);
                let message = crate::i18n::fmt(tl!("Combined {n} files"), &[("n", &count.to_string())]);
                self.open_created("Combined.pdf", bytes, &message)
            }
            // The list stays, so the user can fix it.
            Err(e) => self.notify_fmt("Couldn't combine files: {e}", &[("e", &e.to_string())]),
        }
    }
}
