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

/// The Wayland app id (matches packaging/linux/ai.storyteller.pdfcraft.desktop).
pub const APP_ID: &str = "ai.storyteller.pdfcraft";

/// A window's outer settings: size, minimum size, drag and drop, app id and icon, and the title
/// bar drawn by us on macOS. Used for the main window and for every other one.
pub fn window_builder(title: &str, integrated_titlebar: bool, icon: Option<std::sync::Arc<egui::IconData>>) -> egui::ViewportBuilder {
    let mut builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size([1440.0, 920.0])
        .with_min_inner_size([820.0, 520.0])
        .with_drag_and_drop(true)
        .with_app_id(APP_ID);
    if let Some(icon) = icon {
        builder = builder.with_icon(icon);
    }
    if integrated_titlebar {
        builder = builder.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
    }
    builder
}

/// Most views one document can have (the render pool serves this many clients).
pub const MAX_VIEWS_PER_DOCUMENT: usize = pdfcraft_render::MAX_CLIENTS;

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
        self.stamp_handled();
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

    /// Bring every view up to date with what happened to its document: edits made through
    /// another view (or window) re-render only what they changed.
    pub(crate) fn sync_views(&mut self) {
        self.assign_view_numbers();
        self.for_each_view(|_, view, session| {
            if let Some(doc) = session.get(view.id) {
                Self::catch_up(view, doc);
            }
        });
    }

    /// Bring the loaded window's views up to date (before it acts, so what it changes next is
    /// told apart from what happened elsewhere).
    pub(crate) fn sync_loaded_views(&mut self) {
        let session = &self.session;
        for view in self.views.iter_mut() {
            if let Some(doc) = session.get(view.id) {
                Self::catch_up(view, doc);
            }
        }
    }

    fn catch_up(view: &mut DocView, doc: &pdfcraft_engine::Document) {
        use pdfcraft_engine::Change;
        Self::stamp_view(view, doc);
        // What the view did itself counts as seen; anything newer (another window's edit in
        // the same frame) still reaches it.
        let from = view.seen_generation.max(view.handled_generation);
        let change = doc.changes_since(from);
        match &change {
            None => {}
            Some(Change::Pages(pages)) => pages.iter().for_each(|p| view.page_changed(*p)),
            Some(Change::Remap(map)) => view.document_changed_with(&doc.info, Some(map)),
            Some(Change::All) => view.document_changed(&doc.info),
        }
        if let Some(change) = &change {
            view.foreign_change(doc, change);
        }
        if view.seen_display_generation.max(view.handled_display_generation) != doc.display_generation() {
            view.invalidate_content();
        }
        // An active search runs again once the texts of the new page list arrive; the query
        // stays, only the old hits (which point at old pages) go.
        if view.seen_generation.max(view.handled_generation) != doc.edit_generation() {
            view.rerun_find();
        }
        view.change_handled = false;
        view.seen_generation = doc.edit_generation();
        view.seen_display_generation = doc.display_generation();
    }

    /// A view that took a change into account itself has caught up with the document as it is
    /// now: remember that generation.
    fn stamp_view(view: &mut DocView, doc: &pdfcraft_engine::Document) {
        if std::mem::take(&mut view.change_handled) {
            view.handled_generation = view.handled_generation.max(doc.edit_generation());
            view.handled_display_generation = doc.display_generation();
        }
    }

    /// Stamp the generation on the loaded window's views that handled a change themselves. Runs
    /// before the window is parked, so a change another window makes next is not mistaken for
    /// one this window already handled.
    pub(crate) fn stamp_handled(&mut self) {
        let session = &self.session;
        for view in self.views.iter_mut() {
            if let Some(doc) = session.get(view.id) {
                Self::stamp_view(view, doc);
            }
        }
    }

    /// Add a view of `doc` in a parked window of its own (tests).
    #[doc(hidden)]
    pub fn test_add_parked_view(&mut self, doc: DocId) -> Option<WindowId> {
        let d = self.session.get(doc)?;
        let mut view = DocView::new(doc, &d.info, self.view_defaults);
        view.seen_generation = d.edit_generation();
        view.seen_display_generation = d.display_generation();
        view.view_no = 0;
        let state = WindowState { views: vec![view], active: Some(0), ..Default::default() };
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        self.windows.push(WindowSlot { id, state, geometry: None, last_rect: None });
        self.assign_view_numbers();
        Some(id)
    }

    /// The window `id` was asked to close, as by its close button (tests).
    #[doc(hidden)]
    pub fn test_close_window(&mut self, id: WindowId, ctx: &egui::Context) {
        self.with_window(id, |a| a.guard_close_window(ctx, id));
    }

    /// Ask to quit as the operating system does (tests, automation).
    #[doc(hidden)]
    pub fn request_quit(&mut self, ctx: &egui::Context) {
        self.quit_requested = true;
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    /// Queue a change to the windows, to be applied after this frame (tests).
    #[doc(hidden)]
    pub fn queue_window_op(&mut self, op: WindowOp) {
        self.pending_window_ops.push(op);
    }

    /// Apply the queued window changes now, in the main window (tests).
    #[doc(hidden)]
    pub fn pending_window_ops_for_test(&mut self) {
        self.apply_window_ops();
    }

    /// Apply the queued changes to the set of windows.
    pub(crate) fn apply_window_ops(&mut self) {
        if self.current_window != WindowId::ROOT {
            return; // only from the main window's pass
        }
        for op in std::mem::take(&mut self.pending_window_ops) {
            match op {
                WindowOp::Focus(id) => self.focus_window(id),
                WindowOp::NewView { doc, from } => self.new_view_window(doc, from),
                WindowOp::MoveTab { doc, from, to } => self.move_tab(doc, from, to),
                WindowOp::Close(id) => self.close_window(id),
                WindowOp::MergeAll => self.merge_all_windows(),
                WindowOp::Promote(id) => self.promote_window(id),
            }
        }
        self.repair_windows();
    }

    /// The main window was closed while others are open: the window `id` takes its place (the
    /// main window's own tabs close; documents another window shows stay open) and the main
    /// window takes over its size and position.
    fn promote_window(&mut self, id: WindowId) {
        if id == WindowId::ROOT || self.current_window != WindowId::ROOT {
            return;
        }
        let Some(at) = self.windows.iter().position(|w| w.id == id) else { return };
        while !self.views.is_empty() {
            self.close_tab(self.views.len() - 1);
        }
        let slot = self.windows.remove(at);
        let rect = slot.last_rect.or(slot.geometry);
        let mut state = slot.state;
        self.swap_window(&mut state);
        self.window_had_focus.remove(&id);
        self.focused_window = WindowId::ROOT;
        if let (Some(ctx), Some(rect)) = (self.ctx.clone(), rect.filter(|r| r.is_finite() && r.width() > 100.0 && r.height() > 100.0)) {
            // Wayland does not let a window place itself: only its size carries over.
            let wayland = cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some();
            if !wayland {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::OuterPosition(rect.min));
            }
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::InnerSize(rect.size()));
        }
    }

    fn focus_window(&mut self, id: WindowId) {
        if !self.has_window(id) {
            return;
        }
        self.focused_window = id;
        if let Some(ctx) = &self.ctx {
            ctx.send_viewport_cmd_to(id.viewport(), egui::ViewportCommand::Focus);
        }
    }

    /// The rectangle window `id` last had on screen.
    fn rect_of(&self, id: WindowId) -> Option<egui::Rect> {
        if id == WindowId::ROOT { self.root_rect } else { self.windows.iter().find(|w| w.id == id).and_then(|w| w.last_rect) }
    }

    /// A window for `view`, laid out like window `from` (same panels and workspace, a little
    /// below and to the right of it).
    fn open_window_with(&mut self, view: DocView, from: WindowId) {
        if self.windows.len() + 1 >= MAX_WINDOWS {
            self.notify_tr("Too many windows are open");
            return;
        }
        let (mode, left, left_open, right) =
            self.with_window(from, |a| (a.mode, a.left, a.left_open, a.right)).unwrap_or((self.mode, self.left, self.left_open, self.right));
        let geometry = self.rect_of(from).map(|r| r.translate(egui::vec2(30.0, 30.0)));
        let state = WindowState { views: vec![view], active: Some(0), mode, left, left_open, right, ..Default::default() };
        let id = WindowId(self.next_window_id);
        self.next_window_id = self.next_window_id.saturating_add(1);
        self.windows.push(WindowSlot { id, state, geometry, last_rect: None });
        self.assign_view_numbers();
        self.focus_window(id);
    }

    /// A new view of `doc` showing what the one in window `from` shows.
    fn new_view_window(&mut self, doc: DocId, from: WindowId) {
        if !self.has_window(from) {
            return;
        }
        if self.view_count(doc) >= MAX_VIEWS_PER_DOCUMENT {
            return self.notify_tr("Too many windows for this document");
        }
        let Some(d) = self.session.get(doc) else { return };
        let mut view = DocView::new(doc, &d.info, self.view_defaults);
        view.seen_generation = d.edit_generation();
        view.seen_display_generation = d.display_generation();
        let pages = d.info.pages.len();
        let shown = self
            .with_window(from, |a| a.views.iter().find(|v| v.id == doc).map(|v| (v.zoom, v.fit, v.layout, v.rotation, v.cover, v.current)))
            .flatten();
        if let Some((zoom, fit, layout, rotation, cover, current)) = shown {
            view.zoom = zoom;
            view.fit = fit;
            view.layout = layout;
            view.rotation = rotation;
            view.cover = cover;
            if current < pages {
                view.go_to_page(current);
            }
        }
        self.open_window_with(view, from);
    }

    /// Take the view of `doc` out of window `from` (without closing the document).
    fn take_view(&mut self, doc: DocId, from: WindowId) -> Option<DocView> {
        self.with_window(from, |a| {
            let at = a.views.iter().position(|v| v.id == doc)?;
            let view = a.views.remove(at);
            a.active = match a.active {
                _ if a.views.is_empty() => None,
                Some(i) if i > at => Some(i - 1),
                Some(i) if i >= a.views.len() => Some(a.views.len() - 1),
                other => other,
            };
            Some(view)
        })
        .flatten()
    }

    fn move_tab(&mut self, doc: DocId, from: WindowId, to: Option<WindowId>) {
        if to == Some(from) {
            return;
        }
        if let Some(target) = to
            && !self.has_window(target)
        {
            return;
        }
        let Some(view) = self.take_view(doc, from) else { return };
        match to {
            None => self.open_window_with(view, from),
            Some(target) => {
                let mut leftover = None;
                self.with_window(target, |a| match a.views.iter().position(|v| v.id == doc) {
                    // That window shows the document already: show it, and let this view go.
                    Some(i) => {
                        a.active = Some(i);
                        leftover = Some(view);
                    }
                    None => {
                        a.views.push(view);
                        a.active = Some(a.views.len() - 1);
                    }
                });
                if let Some(mut extra) = leftover
                    && let Some(d) = self.session.get(doc)
                {
                    extra.release_render_client(&d.renderer);
                }
                self.focus_window(target);
            }
        }
        // A window left without tabs closes with them.
        if from != WindowId::ROOT && self.with_window(from, |a| a.views.is_empty() && !a.combine_tab.open).unwrap_or(false) {
            self.close_window(from);
        }
    }

    /// Close window `id` and its tabs. Questions about unsaved work come first (`guard_close_window`).
    fn close_window(&mut self, id: WindowId) {
        if id == WindowId::ROOT {
            log::warn!("the main window cannot be closed this way");
            return;
        }
        if id == self.current_window || !self.has_window(id) {
            return;
        }
        self.with_window(id, |a| {
            while !a.views.is_empty() {
                a.close_tab(a.views.len() - 1);
            }
        });
        self.windows.retain(|w| w.id != id);
        if self.focused_window == id {
            self.focused_window = WindowId::ROOT;
        }
    }

    /// Bring every tab of every other window into the main window; a document the main window
    /// shows already is not added twice.
    fn merge_all_windows(&mut self) {
        let slots = std::mem::take(&mut self.windows);
        for mut slot in slots {
            let views = std::mem::take(&mut slot.state.views);
            for mut view in views {
                if self.views.iter().any(|v| v.id == view.id) {
                    if let Some(d) = self.session.get(view.id) {
                        view.release_render_client(&d.renderer);
                    }
                } else {
                    self.views.push(view);
                    if self.active.is_none() {
                        self.active = Some(self.views.len() - 1);
                    }
                }
            }
        }
        self.focused_window = WindowId::ROOT;
    }

    /// Put the windows back in order if they are not (see `debug_check_windows`): nothing is
    /// ever closed that holds a document.
    fn repair_windows(&mut self) {
        // A document without a view gets one in the main window.
        let mut shown = std::collections::HashSet::new();
        self.for_each_state(|_, views| views.iter().for_each(|v| _ = shown.insert(v.id)));
        let orphans: Vec<DocId> = self.session.docs().iter().map(|d| d.id).filter(|id| !shown.contains(id)).collect();
        for id in orphans {
            log::error!("document {id:?} had no view; showing it in the main window");
            if let Some(d) = self.session.get(id) {
                let view = DocView::new(id, &d.info, self.view_defaults);
                self.views.push(view);
            }
        }
        // A view of a document the session no longer has goes; a second view of a document in
        // one window goes too.
        let open: std::collections::HashSet<DocId> = self.session.docs().iter().map(|d| d.id).collect();
        let fix = |views: &mut Vec<DocView>, active: &mut Option<usize>| {
            let mut seen = std::collections::HashSet::new();
            let before = views.len();
            views.retain(|v| open.contains(&v.id) && seen.insert(v.id));
            if views.len() != before {
                log::error!("removed {} stray views", before - views.len());
                *active = if views.is_empty() { None } else { Some(active.unwrap_or(0).min(views.len() - 1)) };
            }
        };
        fix(&mut self.views, &mut self.active);
        for slot in &mut self.windows {
            fix(&mut slot.state.views, &mut slot.state.active);
        }
        // A window with nothing to show closes.
        let empty: Vec<WindowId> = self.windows.iter().filter(|w| w.state.views.is_empty() && !w.state.combine_tab.open).map(|w| w.id).collect();
        self.windows.retain(|w| !empty.contains(&w.id));
        if !self.has_window(self.focused_window) {
            self.focused_window = WindowId::ROOT;
        }
        for problem in self.debug_check_windows() {
            log::error!("window check: {problem}");
        }
    }

    /// Note where window `id` is and whether it has the focus. Leaving a window takes over what
    /// is half typed in it, so the other windows see the document as it is.
    pub(crate) fn track_window(&mut self, id: WindowId, ctx: &egui::Context) {
        if let Some(rect) = ctx.input(|i| i.viewport().outer_rect).filter(|r| r.is_finite()) {
            if id == WindowId::ROOT {
                self.root_rect = Some(rect);
            } else if let Some(slot) = self.windows.iter_mut().find(|w| w.id == id) {
                slot.last_rect = Some(rect);
            }
        }
        if self.window_count() < 2 && !self.window_had_focus.is_empty() {
            self.window_had_focus.clear();
        }
        if self.window_count() < 2 {
            return;
        }
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        let had = self.window_had_focus.insert(id, focused).unwrap_or(false);
        if had && !focused {
            if let Some(i) = self.active {
                self.commit_view_inputs(i);
            }
        } else if focused && !had {
            self.focused_window = id;
            self.prioritise_focused_window();
        }
    }

    /// Pages for the focused window are rendered first.
    fn prioritise_focused_window(&mut self) {
        let focused = self.focused_window;
        self.for_each_view(|window, view, session| {
            if let (Some((pool_id, client)), Some(doc)) = (view.render_client, session.get(view.id))
                && pool_id == doc.renderer.pool_id()
            {
                doc.renderer.set_priority(client, u8::from(window == focused));
            }
        });
    }

    /// Draw the windows besides the main one. Each is an egui viewport of its own, with its
    /// state loaded for the duration of its pass.
    pub(crate) fn show_child_windows(&mut self) {
        let Some(ctx) = self.ctx.clone() else { return };
        let ids: Vec<WindowId> = self.windows.iter().map(|w| w.id).collect();
        for id in ids {
            let Some(slot) = self.windows.iter().find(|w| w.id == id) else { continue };
            let geometry = slot.geometry;
            let title = self.with_window(id, |a| a.window_title.clone()).unwrap_or_default();
            let title = if title.is_empty() { "PdfCraft".to_owned() } else { title };
            let mut builder = window_builder(&title, self.integrated_titlebar, self.window_icon.clone());
            if let Some(rect) = geometry.filter(|r| r.is_finite() && r.width() > 100.0 && r.height() > 100.0) {
                builder = builder.with_position(rect.min).with_inner_size(rect.size());
            }
            self.with_window(id, |app| {
                ctx.show_viewport_immediate(id.viewport(), builder, |ui, class| app.window_pass(id, ui, class));
            });
        }
    }

    /// What a view is called in its tab and window title: the document's name, and `:n` once
    /// the document is shown more than once.
    pub fn display_label(&self, view: &DocView) -> Option<String> {
        let name = self.session.get(view.id)?.display_name();
        Some(if self.view_count(view.id) > 1 { format!("{name}:{}", view.view_no) } else { name })
    }

    /// Number the views that have none yet.
    pub(crate) fn assign_view_numbers(&mut self) {
        let mut next = std::mem::take(&mut self.next_view_no);
        self.for_each_view(|_, view, _| {
            if view.view_no == 0 {
                let n = next.entry(view.id).or_insert(0);
                *n = n.saturating_add(1);
                view.view_no = *n;
            }
        });
        self.next_view_no = next;
    }

    /// Remember which window started each background job (and forget finished ones), so its
    /// progress and its result show where the user asked for it.
    pub(crate) fn note_job_origins(&mut self) {
        let here = self.current_window;
        let running = [
            ("export", self.export_status.is_some()),
            ("ocr", self.ocr_run.is_some() || self.ocr_batch.is_some()),
            ("optimize", self.optimize_run.is_some()),
            ("action", self.action_run.is_some()),
        ];
        for (kind, on) in running {
            if on {
                self.job_origin.entry(kind).or_insert(here);
            } else {
                self.job_origin.remove(kind);
            }
        }
    }

    /// Run `f` (a job's poll) in the window that started the job; in the current one when that
    /// window is gone.
    pub(crate) fn poll_in_origin(&mut self, kind: &'static str, f: impl FnOnce(&mut Self)) {
        let origin = self.job_origin.get(kind).copied().unwrap_or(self.current_window);
        let mut f = Some(f);
        self.with_window(origin, |a| {
            if let Some(f) = f.take() {
                f(a);
            }
        });
        if let Some(f) = f {
            f(self);
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
        guard.sync_loaded_views();
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
    pub(crate) fn for_each_state(&self, mut f: impl FnMut(WindowId, &[DocView])) {
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
        pages_pdf(1)
    }

    /// An `n`-page PDF.
    pub(crate) fn pages_pdf(n: usize) -> Vec<u8> {
        let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")),
        ];
        objs.extend((0..n).map(|_| "<< /Type /Page /Parent 2 0 R >>".to_owned()));
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
    fn a_job_reports_in_the_window_that_started_it() {
        let (mut app, child, _) = two_windows();
        app.job_origin.insert("ocr", child);
        app.poll_in_origin("ocr", |a| a.notify("done"));
        assert!(app.toast.is_none(), "not in the main window");
        assert_eq!(app.with_window(child, |a| a.toast.as_ref().map(|t| t.0.clone())).unwrap().as_deref(), Some("done"));
        // The window is gone: the current one shows it.
        app.job_origin.insert("ocr", WindowId(99));
        app.poll_in_origin("ocr", |a| a.notify("later"));
        assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("later"));
    }

    #[test]
    fn job_origins_follow_the_running_jobs() {
        let (mut app, child, _) = two_windows();
        app.run_inline = true;
        app.with_window(child, |a| {
            a.export_status = Some(Default::default());
            a.note_job_origins();
        });
        assert_eq!(app.job_origin.get("export"), Some(&child));
        app.export_status = None;
        app.note_job_origins();
        assert!(app.job_origin.is_empty());
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod sync_tests {
    use super::tests::pages_pdf;
    use super::*;
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};

    /// A four-page document shown in the main window and in a parked one; both have pictures of
    /// every page.
    fn two_views() -> (PdfCraftApp, WindowId, DocId, egui::Context) {
        let ctx = egui::Context::default();
        let mut app = PdfCraftApp::new();
        app.open_bytes("a.pdf", None, pages_pdf(4)).unwrap();
        let doc = app.views[0].id;
        let other = app.test_add_parked_view(doc).unwrap();
        for page in 0..4 {
            app.views[0].test_set_page_texture(&ctx, page);
            app.with_window(other, |a| a.views[0].test_set_page_texture(&ctx, page));
        }
        (app, other, doc, ctx)
    }

    fn comment(page: usize) -> Edit {
        Edit::AddAnnotation(NewAnnotation {
            page,
            shape: Shape::Rectangle { rect: [10.0, 10.0, 60.0, 60.0] },
            style: Style { color: [1.0, 0.0, 0.0], opacity: 1.0, width: 2.0, fill: None },
            contents: "x".into(),
            author: "t".into(),
        })
    }

    #[test]
    fn a_comment_in_one_view_renders_only_that_page_in_the_other() {
        let (mut app, other, _, _ctx) = two_views();
        assert!(app.apply_edit(comment(2)));
        app.sync_views();
        assert!(app.views[0].stale_pages() == [2], "the acting view handled it itself: {:?}", app.views[0].stale_pages());
        assert_eq!(app.with_window(other, |a| a.views[0].stale_pages()).unwrap(), vec![2], "the other view re-renders just that page");
        // Nothing left to catch up on.
        app.with_window(other, |a| a.views[0].test_set_page_texture(&_ctx, 2));
        app.sync_views();
        assert_eq!(app.with_window(other, |a| a.views[0].stale_pages()).unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn deleting_a_page_moves_the_other_views_place() {
        let (mut app, other, doc, _ctx) = two_views();
        app.with_window(other, |a| a.views[0].go_to_page(3));
        app.active = Some(0);
        app.views[0].select_pages(&[1]);
        assert!(app.apply_edit(Edit::DeletePages { pages: vec![1] }));
        app.sync_views();
        let (current, pages) = app.with_window(other, |a| (a.views[0].current, a.session.get(doc).map(|d| d.info.pages.len()))).unwrap();
        assert_eq!((current, pages), (2, Some(3)), "it showed the fourth page and still does");
        assert!(app.debug_check_windows().is_empty());
    }

    #[test]
    fn undo_in_one_view_refreshes_the_other() {
        let (mut app, other, doc, _ctx) = two_views();
        assert!(app.apply_edit(comment(0)));
        app.sync_views();
        app.with_window(other, |a| a.views[0].test_set_page_texture(&_ctx, 0));
        app.session.undo(doc).unwrap();
        app.sync_views();
        assert_eq!(app.with_window(other, |a| a.views[0].stale_pages()).unwrap(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn two_windows_changing_one_document_in_one_frame_reach_each_other() {
        let (mut app, other, _, _ctx) = two_views();
        // Window A comments on page 0, window B on page 2, before anything syncs.
        assert!(app.apply_edit(comment(0)));
        assert!(app.with_window(other, |a| a.apply_edit(comment(2))).unwrap());
        app.sync_views();
        let a_stale = app.views[0].stale_pages();
        let b_stale = app.with_window(other, |a| a.views[0].stale_pages()).unwrap();
        assert_eq!(a_stale, vec![0, 2], "A must learn about B's edit");
        assert_eq!(b_stale, vec![0, 2], "B must learn about A's edit");
    }

    #[test]
    fn a_search_in_one_view_survives_a_page_deleted_in_another() {
        let (mut app, other, _doc, _ctx) = two_views();
        app.with_window(other, |a| {
            a.views[0].open_find();
            if let Some(f) = a.views[0].find.as_mut() {
                f.query = "needle".into();
            }
            a.views[0].rerun_find();
            a.views[0].test_deliver_text(3, "a needle here");
            assert_eq!(a.views[0].find_match_pages(), vec![3]);
        });
        app.views[0].select_pages(&[1]);
        assert!(app.apply_edit(Edit::DeletePages { pages: vec![1] }));
        app.sync_views();
        app.with_window(other, |a| {
            let v = &mut a.views[0];
            assert!(v.find_match_pages().is_empty(), "old hits are gone until the texts are read again");
            assert_eq!(v.find.as_ref().map(|f| f.case_query.as_str()), Some("needle"), "the query stays");
            // The text of the old page 3 (now page 2) arrives again.
            v.test_deliver_text(2, "a needle here");
            assert_eq!(v.find_match_pages(), vec![2], "hit on the page's new number");
        });
    }

    /// Delete the last page through the main window (the other window's view does not know yet).
    fn delete_last_page(app: &mut PdfCraftApp) {
        app.views[0].select_pages(&[3]);
        assert!(app.apply_edit(Edit::DeletePages { pages: vec![3] }));
        app.sync_views();
    }

    #[test]
    fn selections_on_a_page_deleted_elsewhere_are_dropped() {
        let (mut app, other, _doc, _ctx) = two_views();
        app.with_window(other, |a| {
            let v = &mut a.views[0];
            v.comments.selected = Some((3, 0));
            v.links.selected = Some((3, 0));
            v.content.selected = Some((3, 0));
            v.image_selection = Some(crate::edit_text_ui::ImageSelection::test_new(3, 0));
            v.prepare.selected = Some(("nothing".into(), 0));
        });
        delete_last_page(&mut app);
        app.with_window(other, |a| {
            let v = &a.views[0];
            assert!(v.comments.selected.is_none(), "comment selection");
            assert!(v.links.selected.is_none(), "link selection");
            assert!(v.content.selected.is_none(), "content selection");
            assert!(v.image_selection.is_none(), "image selection");
            assert!(v.prepare.selected.is_none(), "prepare selection");
        });
    }

    #[test]
    fn a_comment_selection_follows_its_page_and_survives_when_it_still_exists() {
        let (mut app, other, doc, _ctx) = two_views();
        assert!(app.apply_edit(comment(3)));
        let index = app.session.get(doc).unwrap().info.annotations[0].index;
        app.sync_views();
        app.with_window(other, |a| a.views[0].comments.selected = Some((3, index)));
        // Page 1 goes: the comment's page is page 2 now, and the selection moved with it.
        app.views[0].select_pages(&[1]);
        assert!(app.apply_edit(Edit::DeletePages { pages: vec![1] }));
        app.sync_views();
        assert_eq!(app.with_window(other, |a| a.views[0].comments.selected).unwrap(), Some((2, index)));
        // The comment is deleted by the main window: the selection goes.
        assert!(app.apply_edit(Edit::DeleteAnnotation { page: 2, index }));
        app.sync_views();
        assert_eq!(app.with_window(other, |a| a.views[0].comments.selected).unwrap(), None);
    }

    #[test]
    fn a_paragraph_being_edited_when_its_page_is_deleted_elsewhere_stays_open_with_its_text() {
        let (mut app, other, doc, _ctx) = two_views();
        app.with_window(other, |a| a.views[0].line_editor = Some(crate::edit_text_ui::LineEditor::test_new(3, 0, "my text")));
        delete_last_page(&mut app);
        app.with_window(other, |a| {
            let info = a.session.get(doc).unwrap().info.clone();
            let d = a.session.get(doc).unwrap();
            let ed = a.views[0].line_editor.as_mut().expect("still open");
            assert_eq!(ed.text, "my text");
            assert!(ed.must_check_first(&info, d), "the first Apply is refused");
            assert!(ed.must_check_first(&info, d), "and so is every later one: the page is gone");
        });
    }

    #[test]
    fn a_paragraph_edit_after_a_change_elsewhere_is_confirmed_once() {
        let (mut app, other, doc, _ctx) = two_views();
        app.with_window(other, |a| a.views[0].line_editor = Some(crate::edit_text_ui::LineEditor::test_new(0, 0, "changed")));
        assert!(app.apply_edit(Edit::RotatePages { pages: vec![2], degrees: 90 }));
        app.sync_views();
        app.with_window(other, |a| {
            let d = a.session.get(doc).unwrap();
            let info = d.info.clone();
            let ed = a.views[0].line_editor.as_mut().unwrap();
            assert!(ed.stale);
            // The page has no paragraphs at all: still refused (the target is gone).
            assert!(ed.must_check_first(&info, d));
            assert!(!ed.stale, "asked once");
        });
    }

    #[test]
    fn a_comment_draft_on_a_deleted_page_is_kept_and_not_posted() {
        let (mut app, other, _doc, _ctx) = two_views();
        app.with_window(other, |a| a.views[0].comments.test_open_composer(3, "my note"));
        delete_last_page(&mut app);
        app.with_window(other, |a| {
            let cv = &mut a.views[0].comments;
            assert!(cv.composer.as_ref().is_some_and(|c| c.text == "my note"));
            assert!(cv.post_must_wait(true));
            // An empty draft has nothing to lose.
            if let Some(c) = cv.composer.as_mut() {
                c.text.clear();
            }
            assert!(!cv.post_must_wait(true));
        });
    }

    #[test]
    fn a_comment_draft_is_confirmed_after_a_rebuild_elsewhere_then_posts() {
        let (mut app, other, _doc, _ctx) = two_views();
        app.with_window(other, |a| a.views[0].comments.test_open_composer(0, "my note"));
        assert!(app.apply_edit(Edit::RotatePages { pages: vec![2], degrees: 90 }));
        app.sync_views();
        app.with_window(other, |a| {
            let cv = &mut a.views[0].comments;
            assert!(cv.post_must_wait(false), "first Post: look first");
            assert!(!cv.post_must_wait(false), "second Post goes through");
        });
    }
}
