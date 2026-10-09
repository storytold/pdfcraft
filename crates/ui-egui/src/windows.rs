//! Windows: the per-window state of the app and the machinery to park it.
//!
//! One window shows one set of tabs ([`WindowState`]). The app keeps the state of the window it
//! is working on in its own fields (`views`, `active`, `dialog`, ...); the other windows are
//! parked in [`WindowSlot`]s. [`PdfCraftApp::with_window`] swaps a parked window in for the
//! duration of a call, so all existing code keeps using `self.views` and friends unchanged.

use super::*;

/// A window of the app. The first one (the main window) is [`WindowId::ROOT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u32);

impl WindowId {
    pub const ROOT: Self = Self(0);

    pub fn viewport(self) -> egui::ViewportId {
        if self == Self::ROOT { egui::ViewportId::ROOT } else { egui::ViewportId::from_hash_of(("pdfcraft-window", self.0)) }
    }
}

thread_local! {
    /// The window being drawn, for [`wid`].
    static DRAWING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// An egui id that belongs to the window being drawn. Ids given to panels, areas and modals are
/// absolute and shared by all windows (their state is kept by id), so two windows would share a
/// panel width, a scroll position or the focus of a text field. The main window keeps the plain
/// ids, so its saved layout is unchanged.
pub(crate) fn wid(name: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    match DRAWING.with(std::cell::Cell::get) {
        0 => egui::Id::new(name),
        window => egui::Id::new(("window", window)).with(name),
    }
}

/// Make `id` the window [`wid`] builds ids for.
pub(crate) fn set_drawing(id: WindowId) {
    DRAWING.with(|d| d.set(id.0));
}

/// Most windows open at once (restored sessions are capped to this).
pub const MAX_WINDOWS: usize = 32;

/// Everything that belongs to one window; see the `[window]` fields of [`PdfCraftApp`]. The
/// defaults here are the single source for [`PdfCraftApp::new`].
pub struct WindowState {
    pub(crate) views: Vec<DocView>,
    /// `None` shows the Home tab.
    pub(crate) active: Option<usize>,
    pub(crate) mode: Mode,
    /// Explicit CLI/control mode lasts for this session and is never persisted.
    pub(crate) mode_override: Option<Mode>,
    pub(crate) left: LeftPanel,
    pub(crate) left_open: bool,
    pub(crate) right: Option<RightPanel>,
    pub(crate) quick_tool: QuickTool,
    /// The tool to go back to when Space, held for a temporary Hand, is released.
    pub(crate) space_hand: Option<QuickTool>,
    pub(crate) dialog: Option<Dialog>,
    /// The dialog seen at the last check, and a counter bumped whenever it changes (see
    /// [`Self::dialog_epoch`]).
    pub(crate) dialog_seen: Option<Dialog>,
    pub(crate) dialog_epoch: u64,
    pub(crate) palette_open: bool,
    pub(crate) palette_query: String,
    pub(crate) all_tools_expanded: bool,
    pub(crate) toast: Option<(String, f64)>,
    pub(crate) password_prompt: Option<PasswordPrompt>,
    pub(crate) full_screen: bool,
    /// A pending "save changes?" question (closing a dirty tab or quitting).
    pub(crate) close_request: Option<CloseRequest>,
    /// Files dropped on the page grid, waiting for the pointer to say which gap they go to.
    pub(crate) grid_drop: Option<GridDrop>,
    /// Shortcuts pressed while a text field had the keyboard, run on the next frame (see
    /// `registry_shortcuts`).
    pub(crate) deferred_commands: Vec<(&'static str, Option<DocId>)>,
    /// The window title last sent to the platform.
    pub(crate) window_title: String,
    /// The Combine files tab: whether it is open, shown, its selection and undo history.
    pub(crate) combine_tab: combine_ui::CombineTab,
    /// Combine files: the files staged so far.
    pub(crate) combine_draft: Vec<combine_ui::CombineFile>,
    /// A bookmark being renamed in the Bookmarks panel: (path, text so far).
    pub(crate) bookmark_rename: Option<(Vec<usize>, String)>,
    /// A document asked to open this address; the user hasn't answered yet (#90, #91).
    pub(crate) pending_link: Option<PendingLink>,
    /// A background job's progress card (see [`widgets::progress_notice`]).
    pub(crate) progress_notice: Option<widgets::ProgressNotice>,
    /// Images waiting for the resolution choice (released on cancel).
    pub(crate) image_import: Option<create_ui::ImageImport>,
    /// Prepare a form ▸ Preview: fill the form instead of editing its fields.
    pub(crate) form_preview: bool,
    pub(crate) cert_viewer: Option<sign_ui::CertViewer>,
    /// Document Properties ▸ Description fields being edited: (document, Title/Author/Subject/Keywords).
    pub(crate) props_draft: Option<(DocId, [String; 4])>,
    /// Document Properties ▸ Initial View (and reading options) being edited.
    pub(crate) view_draft: Option<(DocId, pdfcraft_engine::InitialView)>,
    /// Split dialog settings.
    pub(crate) split_draft: SplitDraft,
    pub(crate) extract_draft: ExtractDraft,
    pub(crate) rotate_draft: RotateDraft,
    /// The signing dialogs' state.
    pub(crate) sign_draft: Option<SignDraft>,
    /// Duplicate Field: which field and onto which pages.
    pub(crate) duplicate_draft: Option<DuplicateDraft>,
    /// The Comment Properties dialog's state.
    pub(crate) comment_props: Option<comment_props::PropsDraft>,
    pub(crate) field_props: Option<prepare::FieldDraft>,
    pub(crate) link_draft: Option<LinkDraft>,
    /// The Replace Pages dialog's state.
    pub(crate) replace_draft: Option<files::ReplaceDraft>,
    /// Number pages dialog settings (1-based pages).
    pub(crate) number_draft: NumberDraft,
    /// Protect Using Password dialog state.
    pub(crate) protect_draft: protect::ProtectDraft,
    /// Set Page Boxes dialog state.
    pub(crate) boxes_draft: pageboxes::BoxesDraft,
    /// Header & footer / watermark / background dialog state.
    pub(crate) marks_draft: marks_ui::MarksDraft,
    /// Export dialog settings.
    pub(crate) export_draft: export_ui::ExportDraft,
    pub(crate) print_draft: PrintDraft,
    pub(crate) redact_pages_draft: RedactPagesDraft,
    pub(crate) redact_search: RedactSearchDraft,
    pub(crate) hidden_draft: HiddenDraft,
    /// PDF Optimizer choices.
    pub(crate) optimize_draft: OptimizeDraft,
    /// Scan & OCR: the Recognize Text choices, the running job, and (tests) run it inline.
    pub(crate) ocr_draft: ocr_ui::OcrDraft,
    pub(crate) alt_draft: a11y_ui::AltDraft,
    pub(crate) doc_js: js_ui::DocJsDraft,
    pub(crate) stamp_draft: stamps_ui::StampDraft,
    /// The Create signature / initials dialog, and its typed preview.
    pub(crate) signature_draft: fill_sign::SigDraft,
    pub(crate) signature_preview: Option<(fill_sign::SavedSig, egui::TextureHandle)>,
    pub(crate) wizard: actions_ui::Wizard,
    /// Compare files: the chosen older document and the last result.
    pub(crate) compare_old: Option<DocId>,
    pub(crate) compare: Option<compare_ui::CompareState>,
    /// Standards ▸ PDF/A: level and last result.
    pub(crate) pdfa: standards_ui::PdfaState,
    pub(crate) a11y: a11y_ui::A11yState,
    /// The last space audit.
    pub(crate) space_audit: Vec<pdfcraft_engine::optimize::SpaceUse>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) signature_images: fill_sign::ImageInbox,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            views: Vec::new(),
            active: None,
            mode: Mode::AllTools,
            mode_override: None,
            left: LeftPanel::AllTools,
            left_open: true,
            right: None,
            quick_tool: QuickTool::Select,
            space_hand: None,
            dialog: None,
            dialog_seen: None,
            dialog_epoch: 0,
            palette_open: false,
            palette_query: String::new(),
            all_tools_expanded: false,
            toast: None,
            password_prompt: None,
            full_screen: false,
            close_request: None,
            grid_drop: None,
            deferred_commands: Vec::new(),
            window_title: String::new(),
            combine_tab: Default::default(),
            combine_draft: Vec::new(),
            bookmark_rename: None,
            pending_link: None,
            progress_notice: None,
            image_import: None,
            form_preview: false,
            cert_viewer: None,
            props_draft: None,
            view_draft: None,
            split_draft: SplitDraft::default(),
            extract_draft: ExtractDraft::default(),
            rotate_draft: RotateDraft::default(),
            sign_draft: None,
            duplicate_draft: None,
            comment_props: None,
            field_props: None,
            link_draft: None,
            replace_draft: None,
            number_draft: NumberDraft { from: 1, to: 1, style: pdfcraft_engine::LabelStyle::Decimal, prefix: String::new(), start: 1 },
            protect_draft: Default::default(),
            boxes_draft: Default::default(),
            marks_draft: Default::default(),
            export_draft: Default::default(),
            print_draft: PrintDraft::default(),
            redact_pages_draft: RedactPagesDraft::default(),
            redact_search: RedactSearchDraft::default(),
            hidden_draft: HiddenDraft::default(),
            optimize_draft: OptimizeDraft::default(),
            ocr_draft: ocr_ui::OcrDraft::default(),
            alt_draft: Default::default(),
            doc_js: Default::default(),
            stamp_draft: Default::default(),
            signature_draft: Default::default(),
            signature_preview: None,
            wizard: Default::default(),
            compare_old: None,
            compare: None,
            pdfa: Default::default(),
            a11y: a11y_ui::A11yState::default(),
            space_audit: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            signature_images: Default::default(),
        }
    }
}

/// A window other than the main one.
pub struct WindowSlot {
    pub id: WindowId,
    /// The state while the window is not the current one.
    pub state: WindowState,
    /// Wanted outer geometry when the window first shows (points, screen coordinates).
    pub geometry: Option<egui::Rect>,
    /// The last reported outer geometry (for promotion and the last session).
    pub last_rect: Option<egui::Rect>,
}

/// Changes to the set of windows, queued while windows are drawn and applied after the frame.
#[derive(Clone, Debug, PartialEq)]
pub enum WindowOp {
    NewView {
        doc: DocId,
        from: WindowId,
    },
    /// `to: None` moves the tab into a new window.
    MoveTab {
        doc: DocId,
        from: WindowId,
        to: Option<WindowId>,
    },
    Close(WindowId),
    MergeAll,
    Promote(WindowId),
    Focus(WindowId),
}

impl PdfCraftApp {
    /// Exchange the window the app fields hold with `state`.
    ///
    /// Every field of [`WindowState`] is named here, so a new field that is not swapped does not
    /// compile.
    pub(crate) fn swap_window(&mut self, state: &mut WindowState) {
        let WindowState {
            views,
            active,
            mode,
            mode_override,
            left,
            left_open,
            right,
            quick_tool,
            space_hand,
            dialog,
            dialog_seen,
            dialog_epoch,
            palette_open,
            palette_query,
            all_tools_expanded,
            toast,
            password_prompt,
            full_screen,
            close_request,
            grid_drop,
            deferred_commands,
            window_title,
            combine_tab,
            combine_draft,
            bookmark_rename,
            pending_link,
            progress_notice,
            image_import,
            form_preview,
            cert_viewer,
            props_draft,
            view_draft,
            split_draft,
            extract_draft,
            rotate_draft,
            sign_draft,
            duplicate_draft,
            comment_props,
            field_props,
            link_draft,
            replace_draft,
            number_draft,
            protect_draft,
            boxes_draft,
            marks_draft,
            export_draft,
            print_draft,
            redact_pages_draft,
            redact_search,
            hidden_draft,
            optimize_draft,
            ocr_draft,
            alt_draft,
            doc_js,
            stamp_draft,
            signature_draft,
            signature_preview,
            wizard,
            compare_old,
            compare,
            pdfa,
            a11y,
            space_audit,
            #[cfg(target_arch = "wasm32")]
            signature_images,
        } = state;
        std::mem::swap(&mut self.views, views);
        std::mem::swap(&mut self.active, active);
        std::mem::swap(&mut self.mode, mode);
        std::mem::swap(&mut self.mode_override, mode_override);
        std::mem::swap(&mut self.left, left);
        std::mem::swap(&mut self.left_open, left_open);
        std::mem::swap(&mut self.right, right);
        std::mem::swap(&mut self.quick_tool, quick_tool);
        std::mem::swap(&mut self.space_hand, space_hand);
        std::mem::swap(&mut self.dialog, dialog);
        std::mem::swap(&mut self.dialog_seen, dialog_seen);
        std::mem::swap(&mut self.dialog_epoch, dialog_epoch);
        std::mem::swap(&mut self.palette_open, palette_open);
        std::mem::swap(&mut self.palette_query, palette_query);
        std::mem::swap(&mut self.all_tools_expanded, all_tools_expanded);
        std::mem::swap(&mut self.toast, toast);
        std::mem::swap(&mut self.password_prompt, password_prompt);
        std::mem::swap(&mut self.full_screen, full_screen);
        std::mem::swap(&mut self.close_request, close_request);
        std::mem::swap(&mut self.grid_drop, grid_drop);
        std::mem::swap(&mut self.deferred_commands, deferred_commands);
        std::mem::swap(&mut self.window_title, window_title);
        std::mem::swap(&mut self.combine_tab, combine_tab);
        std::mem::swap(&mut self.combine_draft, combine_draft);
        std::mem::swap(&mut self.bookmark_rename, bookmark_rename);
        std::mem::swap(&mut self.pending_link, pending_link);
        std::mem::swap(&mut self.progress_notice, progress_notice);
        std::mem::swap(&mut self.image_import, image_import);
        std::mem::swap(&mut self.form_preview, form_preview);
        std::mem::swap(&mut self.cert_viewer, cert_viewer);
        std::mem::swap(&mut self.props_draft, props_draft);
        std::mem::swap(&mut self.view_draft, view_draft);
        std::mem::swap(&mut self.split_draft, split_draft);
        std::mem::swap(&mut self.extract_draft, extract_draft);
        std::mem::swap(&mut self.rotate_draft, rotate_draft);
        std::mem::swap(&mut self.sign_draft, sign_draft);
        std::mem::swap(&mut self.duplicate_draft, duplicate_draft);
        std::mem::swap(&mut self.comment_props, comment_props);
        std::mem::swap(&mut self.field_props, field_props);
        std::mem::swap(&mut self.link_draft, link_draft);
        std::mem::swap(&mut self.replace_draft, replace_draft);
        std::mem::swap(&mut self.number_draft, number_draft);
        std::mem::swap(&mut self.protect_draft, protect_draft);
        std::mem::swap(&mut self.boxes_draft, boxes_draft);
        std::mem::swap(&mut self.marks_draft, marks_draft);
        std::mem::swap(&mut self.export_draft, export_draft);
        std::mem::swap(&mut self.print_draft, print_draft);
        std::mem::swap(&mut self.redact_pages_draft, redact_pages_draft);
        std::mem::swap(&mut self.redact_search, redact_search);
        std::mem::swap(&mut self.hidden_draft, hidden_draft);
        std::mem::swap(&mut self.optimize_draft, optimize_draft);
        std::mem::swap(&mut self.ocr_draft, ocr_draft);
        std::mem::swap(&mut self.alt_draft, alt_draft);
        std::mem::swap(&mut self.doc_js, doc_js);
        std::mem::swap(&mut self.stamp_draft, stamp_draft);
        std::mem::swap(&mut self.signature_draft, signature_draft);
        std::mem::swap(&mut self.signature_preview, signature_preview);
        std::mem::swap(&mut self.wizard, wizard);
        std::mem::swap(&mut self.compare_old, compare_old);
        std::mem::swap(&mut self.compare, compare);
        std::mem::swap(&mut self.pdfa, pdfa);
        std::mem::swap(&mut self.a11y, a11y);
        std::mem::swap(&mut self.space_audit, space_audit);
        #[cfg(target_arch = "wasm32")]
        std::mem::swap(&mut self.signature_images, signature_images);
    }

    /// Swap the app fields with the state stored for `id`; false when there is no such window.
    fn swap_stored(&mut self, id: WindowId) -> bool {
        let mut state = if id == WindowId::ROOT {
            std::mem::take(&mut self.root_state)
        } else {
            match self.windows.iter_mut().find(|w| w.id == id) {
                Some(slot) => std::mem::take(&mut slot.state),
                None => return false,
            }
        };
        self.swap_window(&mut state);
        if id == WindowId::ROOT {
            self.root_state = state;
        } else if let Some(slot) = self.windows.iter_mut().find(|w| w.id == id) {
            slot.state = state;
        }
        true
    }

    /// Apply the queued changes to the set of windows.
    pub(crate) fn apply_window_ops(&mut self) {
        for op in std::mem::take(&mut self.pending_window_ops) {
            match op {
                WindowOp::Focus(id) => {
                    if let Some(ctx) = &self.ctx {
                        ctx.send_viewport_cmd_to(id.viewport(), egui::ViewportCommand::Focus);
                    }
                }
                other => log::warn!("window operation not supported yet: {other:?}"),
            }
        }
    }

    /// Whether `id` is a window of the app.
    pub fn has_window(&self, id: WindowId) -> bool {
        id == WindowId::ROOT || self.windows.iter().any(|w| w.id == id)
    }

    /// Ids of all windows, the main one first.
    pub fn window_ids(&self) -> Vec<WindowId> {
        std::iter::once(WindowId::ROOT).chain(self.windows.iter().map(|w| w.id)).collect()
    }

    /// Number of windows, the main one included.
    pub fn window_count(&self) -> usize {
        1 + self.windows.len()
    }

    /// The window the app fields hold now.
    pub fn current_window(&self) -> WindowId {
        self.current_window
    }

    /// Run `f` with window `id` loaded into the app fields. `None` when there is no such window.
    /// The previous window is back in place afterwards, also when `f` panics.
    pub fn with_window<R>(&mut self, id: WindowId, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        if id == self.current_window {
            return Some(f(self));
        }
        if !self.has_window(id) {
            return None;
        }
        let previous = self.current_window;
        // Park the current window, then load the wanted one.
        self.swap_stored(previous);
        self.swap_stored(id);
        self.current_window = id;
        set_drawing(id);
        let mut guard = WindowGuard { app: self, previous, loaded: id };
        Some(f(&mut guard))
    }

    /// Where `doc` is shown: (window, index in that window's `views`).
    pub fn views_of(&self, doc: DocId) -> Vec<(WindowId, usize)> {
        let mut found = Vec::new();
        self.for_each_state(|id, views| {
            found.extend(views.iter().enumerate().filter(|(_, v)| v.id == doc).map(|(i, _)| (id, i)));
        });
        found
    }

    /// Number of views (tabs, across all windows) showing `doc`.
    pub fn view_count(&self, doc: DocId) -> usize {
        self.views_of(doc).len()
    }

    /// Visit the views of every window: the loaded one and the parked ones.
    fn for_each_state(&self, mut f: impl FnMut(WindowId, &[DocView])) {
        f(self.current_window, &self.views);
        if self.current_window != WindowId::ROOT {
            f(WindowId::ROOT, &self.root_state.views);
        }
        for slot in &self.windows {
            if slot.id != self.current_window {
                f(slot.id, &slot.state.views);
            }
        }
    }

    /// Visit every view of every window with the session (to read its document).
    pub(crate) fn for_each_view(&mut self, mut f: impl FnMut(WindowId, &mut DocView, &Session)) {
        let session = &self.session;
        let current = self.current_window;
        self.views.iter_mut().for_each(|v| f(current, v, session));
        if current != WindowId::ROOT {
            self.root_state.views.iter_mut().for_each(|v| f(WindowId::ROOT, v, session));
        }
        for slot in self.windows.iter_mut().filter(|s| s.id != current) {
            let id = slot.id;
            slot.state.views.iter_mut().for_each(|v| f(id, v, session));
        }
    }

    /// Check the window invariants; the list of violations is empty when all hold.
    pub fn debug_check_windows(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for slot in &self.windows {
            if slot.id == WindowId::ROOT {
                problems.push("a child window has the main window's id".to_owned());
            }
            if !ids.insert(slot.id) {
                problems.push(format!("window {} exists twice", slot.id.0));
            }
        }
        let mut shown = std::collections::HashSet::new();
        let mut empty_children = Vec::new();
        self.for_each_state(|id, views| {
            let mut here = std::collections::HashSet::new();
            for v in views {
                if self.session.get(v.id).is_none() {
                    problems.push(format!("window {} shows a document that is not open", id.0));
                }
                if !here.insert(v.id) {
                    problems.push(format!("window {} shows a document twice", id.0));
                }
                shown.insert(v.id);
            }
            if id != WindowId::ROOT && views.is_empty() {
                empty_children.push(id);
            }
        });
        for id in empty_children {
            let open = if id == self.current_window {
                self.combine_tab.open
            } else {
                self.windows.iter().find(|w| w.id == id).is_some_and(|w| w.state.combine_tab.open)
            };
            if !open {
                problems.push(format!("window {} has no tabs", id.0));
            }
        }
        for doc in self.session.docs() {
            if !shown.contains(&doc.id) {
                problems.push(format!("a document is open without a view ({:?})", doc.id));
            }
        }
        problems
    }
}

/// Puts the previous window back when [`PdfCraftApp::with_window`] is left, also by a panic.
struct WindowGuard<'a> {
    app: &'a mut PdfCraftApp,
    previous: WindowId,
    loaded: WindowId,
}

impl std::ops::Deref for WindowGuard<'_> {
    type Target = PdfCraftApp;
    fn deref(&self) -> &PdfCraftApp {
        self.app
    }
}

impl std::ops::DerefMut for WindowGuard<'_> {
    fn deref_mut(&mut self) -> &mut PdfCraftApp {
        self.app
    }
}

impl Drop for WindowGuard<'_> {
    fn drop(&mut self) {
        self.app.swap_stored(self.loaded);
        self.app.swap_stored(self.previous);
        self.app.current_window = self.previous;
        set_drawing(self.previous);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// A one-page PDF.
    pub(crate) fn tiny_pdf() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R >>".to_owned(),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// An app with one open document and a second window showing a view of the same document.
    fn two_windows() -> (PdfCraftApp, WindowId, DocId) {
        let mut app = PdfCraftApp::new();
        app.open_bytes("a.pdf", None, tiny_pdf()).unwrap();
        let doc = app.views[0].id;
        let info = app.session.get(doc).unwrap().info.clone();
        let mut state = WindowState::default();
        state.views.push(DocView::new(doc, &info, app.view_defaults));
        state.active = Some(0);
        state.palette_query = "child".into();
        let id = WindowId(app.next_window_id);
        app.next_window_id += 1;
        app.windows.push(WindowSlot { id, state, geometry: None, last_rect: None });
        (app, id, doc)
    }

    fn summary(app: &PdfCraftApp) -> (usize, Option<usize>, Mode, bool, String, bool) {
        (app.views.len(), app.active, app.mode, app.dialog.is_some(), app.palette_query.clone(), app.left_open)
    }

    #[test]
    fn swap_twice_is_identity() {
        let (mut app, _, _) = two_windows();
        app.palette_query = "root".into();
        app.left_open = false;
        let before = summary(&app);
        let mut other = WindowState { palette_query: "other".into(), ..Default::default() };
        app.swap_window(&mut other);
        assert_eq!(app.palette_query, "other");
        assert!(app.views.is_empty());
        assert!(app.left_open);
        app.swap_window(&mut other);
        assert_eq!(summary(&app), before);
    }

    #[test]
    fn with_window_unknown_id_is_none() {
        let (mut app, _, _) = two_windows();
        assert!(app.with_window(WindowId(99), |_| ()).is_none());
        assert!(app.with_window(WindowId(u32::MAX), |_| ()).is_none());
        assert_eq!(app.current_window(), WindowId::ROOT);
    }

    #[test]
    fn with_window_loads_and_restores() {
        let (mut app, child, doc) = two_windows();
        app.palette_query = "root".into();
        let seen = app.with_window(child, |a| (a.palette_query.clone(), a.views.len(), a.current_window(), a.views_of(doc).len())).unwrap();
        assert_eq!(seen, ("child".to_owned(), 1, child, 2));
        assert_eq!(app.palette_query, "root");
        assert_eq!(app.current_window(), WindowId::ROOT);
        assert_eq!(app.views.len(), 1);
        // Changes made inside stay with that window.
        app.with_window(child, |a| a.palette_query = "edited".into());
        assert_eq!(app.palette_query, "root");
        assert_eq!(app.with_window(child, |a| a.palette_query.clone()).unwrap(), "edited");
    }

    #[test]
    fn with_window_restores_after_panic() {
        let (mut app, child, _) = two_windows();
        app.palette_query = "root".into();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.with_window(child, |_| panic!("boom"));
        }));
        assert!(result.is_err());
        assert_eq!(app.current_window(), WindowId::ROOT);
        assert_eq!(app.palette_query, "root");
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.windows.len(), 1);
        assert_eq!(app.windows[0].state.palette_query, "child");
        assert!(app.debug_check_windows().is_empty());
    }

    #[test]
    fn nested_with_window_restores_every_level() {
        let (mut app, child, _) = two_windows();
        app.palette_query = "root".into();
        let inner = app
            .with_window(child, |a| {
                let root = a.with_window(WindowId::ROOT, |r| r.palette_query.clone());
                (root, a.palette_query.clone(), a.current_window())
            })
            .unwrap();
        assert_eq!(inner, (Some("root".to_owned()), "child".to_owned(), child));
        assert_eq!(app.palette_query, "root");
        assert_eq!(app.windows[0].state.palette_query, "child");
        assert!(app.debug_check_windows().is_empty());
    }

    #[test]
    fn views_of_and_for_each_view_reach_parked_windows() {
        let (mut app, child, doc) = two_windows();
        assert_eq!(app.views_of(doc), vec![(WindowId::ROOT, 0), (child, 0)]);
        let mut seen = Vec::new();
        app.for_each_view(|w, v, session| seen.push((w, v.id, session.get(v.id).is_some())));
        assert_eq!(seen, vec![(WindowId::ROOT, doc, true), (child, doc, true)]);
        // The same from inside the child window.
        app.with_window(child, |a| assert_eq!(a.views_of(doc), vec![(child, 0), (WindowId::ROOT, 0)]));
    }

    #[test]
    fn check_windows_reports_duplicate_view() {
        let (mut app, _, doc) = two_windows();
        assert!(app.debug_check_windows().is_empty());
        let info = app.session.get(doc).unwrap().info.clone();
        let again = DocView::new(doc, &info, app.view_defaults);
        app.views.push(again);
        let problems = app.debug_check_windows();
        assert!(problems.iter().any(|p| p.contains("twice")), "{problems:?}");
    }

    #[test]
    fn check_windows_reports_empty_child_and_orphan_document() {
        let (mut app, child, _) = two_windows();
        app.windows[0].state.views.clear();
        let problems = app.debug_check_windows();
        assert!(problems.iter().any(|p| p.contains(&format!("window {} has no tabs", child.0))), "{problems:?}");
        app.views.clear();
        let problems = app.debug_check_windows();
        assert!(problems.iter().any(|p| p.contains("without a view")), "{problems:?}");
    }
}
