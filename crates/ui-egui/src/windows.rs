//! Several windows (M14): a document tab dragged out of the strip, or sent there from its
//! context menu or the Window menu, shows in a window of its own, and a tab dragged onto another
//! PdfCraft window joins that window's strip.
//!
//! Every extra window is an egui viewport drawn inside the main window's pass
//! (`show_viewport_immediate`), so one `PdfCraftApp` serves every window. The fields that are a
//! window's own (its tabs, panels, mode, dialog and the dialogs' drafts) make up a
//! [`WindowState`]. The app's fields hold the state of the window being drawn
//! (`current_window`); every other window's state is parked in its [`WindowSlot`] (the main
//! window's in `root_state`), and [`PdfCraftApp::with_window`] swaps a window's state in for the
//! duration of a closure. A document shows in one window at a time, so documents, undo and the
//! Combine files tab (main window only) need no synchronising between windows.
//!
//! Changes to the set of windows are queued as [`WindowOp`]s while windows are drawn and applied
//! once per frame, after every window's pass, so no window's state changes under its own pass.
//! Where the desktop tells the app where its windows are (X11, Windows, macOS, and the tests'
//! embedded windows), a tab dragged over another window joins it; where it doesn't (Wayland),
//! the tab opens its own window and the context menu's Move to window moves it instead.

use crate::canvas::DocView;
use crate::{CloseRequest, Dialog, LeftPanel, Mode, PdfCraftApp, QuickTool, RightPanel};
use egui::{Pos2, Rect, Vec2, vec2};
use pdfcraft_engine::DocId;

/// A window: the main window is [`WindowId::ROOT`]; the others count up from 1 for the life of
/// the app (an id is never reused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u32);

impl WindowId {
    pub const ROOT: WindowId = WindowId(0);

    /// The egui viewport the window is drawn in.
    pub fn viewport(self) -> egui::ViewportId {
        if self == Self::ROOT { egui::ViewportId::ROOT } else { egui::ViewportId::from_hash_of(("pdfcraft-window", self.0)) }
    }
}

/// Freedesktop app id of every window: the `.desktop` file name and the hicolor icon name
/// (`packaging/linux/ai.storyteller.pdfcraft.desktop`).
pub const APP_ID: &str = "ai.storyteller.pdfcraft";

/// How many windows may be open at once (the main window included).
pub const MAX_WINDOWS: usize = 16;

/// A new window's size when its source window's size is unknown, in points.
const DEFAULT_SIZE: Vec2 = vec2(1440.0, 920.0);
/// The smallest a new window is made, in points (`apps/pdfcraft` gives the main window the same floor).
const MIN_SIZE: Vec2 = vec2(820.0, 520.0);
/// Where a window opened from a menu lands relative to the window it came from.
const CASCADE: Vec2 = vec2(40.0, 40.0);
/// Where the pointer holds a tab dropped into empty space: the new window's top-left is this
/// far up and left of it, so the tab lands under the pointer.
const TAB_GRIP: Vec2 = vec2(90.0, 18.0);
/// How large a window embedded in the main one is, relative to the size a window would get.
const EMBEDDED_SCALE: f32 = 0.6;

/// The fields of `PdfCraftApp` that are one window's own.
pub struct WindowState {
    pub(crate) views: Vec<DocView>,
    pub(crate) active: Option<usize>,
    pub(crate) mode: Mode,
    pub(crate) mode_override: Option<Mode>,
    pub(crate) left: LeftPanel,
    pub(crate) left_open: bool,
    pub(crate) right: Option<RightPanel>,
    pub(crate) right_before_read: Option<RightPanel>,
    pub(crate) quick_tool: QuickTool,
    pub(crate) space_hand: Option<QuickTool>,
    pub(crate) date_format_draft: Option<String>,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) dialog_seen: Option<Dialog>,
    pub(crate) dialog_epoch: u64,
    pub(crate) palette_open: bool,
    pub(crate) palette_query: String,
    pub(crate) all_tools_expanded: bool,
    pub(crate) toast: Option<(String, f64)>,
    pub(crate) password_prompt: Option<crate::PasswordPrompt>,
    pub(crate) unlock_prompt: Option<crate::UnlockPrompt>,
    pub(crate) full_screen: bool,
    pub(crate) close_request: Option<CloseRequest>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) pending_overwrite: Option<crate::editing::PendingOverwrite>,
    pub(crate) props_draft: Option<(DocId, [String; 4])>,
    pub(crate) view_draft: Option<(DocId, pdfcraft_engine::InitialView)>,
    pub(crate) split_draft: crate::SplitDraft,
    pub(crate) extract_draft: crate::ExtractDraft,
    pub(crate) rotate_draft: crate::RotateDraft,
    pub(crate) sign_draft: Option<crate::SignDraft>,
    pub(crate) ocr_draft: crate::ocr_ui::OcrDraft,
    pub(crate) wizard: crate::actions_ui::Wizard,
    pub(crate) compare_old: Option<DocId>,
    pub(crate) compare: Option<crate::compare_ui::CompareState>,
    pub(crate) doc_js: crate::js_ui::DocJsDraft,
    pub(crate) a11y: crate::a11y_ui::A11yState,
    pub(crate) tags_state: crate::tags_ui::State,
    pub(crate) alt_draft: crate::a11y_ui::AltDraft,
    pub(crate) cert_viewer: Option<crate::sign_ui::CertViewer>,
    pub(crate) space_audit: Vec<pdfcraft_engine::optimize::SpaceUse>,
    pub(crate) image_import: Option<crate::create_ui::ImageImport>,
    pub(crate) stamp_draft: crate::stamps_ui::StampDraft,
    pub(crate) optimize_draft: crate::OptimizeDraft,
    pub(crate) progress_notice: Option<crate::widgets::ProgressNotice>,
    pub(crate) grid_drop: Option<crate::GridDrop>,
    pub(crate) duplicate_draft: Option<crate::DuplicateDraft>,
    pub(crate) form_preview: bool,
    pub(crate) window_title: String,
    pub(crate) deferred_commands: Vec<(&'static str, Option<DocId>)>,
    pub(crate) bookmark_rename: Option<(Vec<usize>, String)>,
    pub(crate) number_draft: crate::NumberDraft,
    pub(crate) word_count_draft: crate::word_count_ui::Draft,
    pub(crate) protect_draft: crate::protect::ProtectDraft,
    pub(crate) boxes_draft: crate::pageboxes::BoxesDraft,
    pub(crate) marks_draft: crate::marks_ui::MarksDraft,
    pub(crate) export_draft: crate::export_ui::ExportDraft,
    pub(crate) signature_draft: crate::fill_sign::SigDraft,
    pub(crate) signature_preview: Option<(crate::fill_sign::SavedSig, egui::TextureHandle)>,
    pub(crate) comment_props: Option<crate::comment_props::PropsDraft>,
    pub(crate) field_props: Option<crate::prepare::FieldDraft>,
    pub(crate) bulk_field_props: Option<crate::bulk_fields::Draft>,
    pub(crate) redact_pages_draft: crate::RedactPagesDraft,
    pub(crate) redact_search: crate::RedactSearchDraft,
    pub(crate) hidden_draft: crate::HiddenDraft,
    pub(crate) print_draft: crate::PrintDraft,
    pub(crate) link_draft: Option<crate::LinkDraft>,
    pub(crate) replace_draft: Option<crate::files::ReplaceDraft>,
    pub(crate) pending_link: Option<crate::PendingLink>,
    pub(crate) pending_full_screen: Option<DocId>,
    pub(crate) pending_remote_link: Option<crate::PendingRemoteLink>,
    pub(crate) pdfa: crate::standards_ui::PdfaState,
    pub(crate) pending_panel_reveals: Vec<crate::docking::Panel>,
}

impl Default for WindowState {
    /// What a fresh window starts with: the same values `PdfCraftApp::new` gives these fields.
    fn default() -> Self {
        Self {
            views: Vec::new(),
            active: None,
            mode: Mode::AllTools,
            mode_override: None,
            left: LeftPanel::AllTools,
            left_open: true,
            right: None,
            right_before_read: None,
            quick_tool: QuickTool::Select,
            space_hand: None,
            date_format_draft: None,
            dialog: None,
            dialog_seen: None,
            dialog_epoch: 0,
            palette_open: false,
            palette_query: String::new(),
            all_tools_expanded: false,
            toast: None,
            password_prompt: None,
            unlock_prompt: None,
            full_screen: false,
            close_request: None,
            #[cfg(not(target_arch = "wasm32"))]
            pending_overwrite: None,
            props_draft: None,
            view_draft: None,
            split_draft: Default::default(),
            extract_draft: Default::default(),
            rotate_draft: Default::default(),
            sign_draft: None,
            ocr_draft: Default::default(),
            wizard: Default::default(),
            compare_old: None,
            compare: None,
            doc_js: Default::default(),
            a11y: Default::default(),
            tags_state: Default::default(),
            alt_draft: Default::default(),
            cert_viewer: None,
            space_audit: Vec::new(),
            image_import: None,
            stamp_draft: Default::default(),
            optimize_draft: Default::default(),
            progress_notice: None,
            grid_drop: None,
            duplicate_draft: None,
            form_preview: false,
            window_title: String::new(),
            deferred_commands: Vec::new(),
            bookmark_rename: None,
            number_draft: crate::NumberDraft { from: 1, to: 1, style: pdfcraft_engine::LabelStyle::Decimal, prefix: String::new(), start: 1 },
            word_count_draft: Default::default(),
            protect_draft: Default::default(),
            boxes_draft: Default::default(),
            marks_draft: Default::default(),
            export_draft: Default::default(),
            signature_draft: Default::default(),
            signature_preview: None,
            comment_props: None,
            field_props: None,
            bulk_field_props: None,
            redact_pages_draft: Default::default(),
            redact_search: Default::default(),
            hidden_draft: Default::default(),
            print_draft: Default::default(),
            link_draft: None,
            replace_draft: None,
            pending_link: None,
            pending_full_screen: None,
            pending_remote_link: None,
            pdfa: Default::default(),
            pending_panel_reveals: Vec::new(),
        }
    }
}

/// A window besides the main one.
pub struct WindowSlot {
    pub id: WindowId,
    /// The window's state while it is not the current one (`None` while it is).
    pub(crate) state: Option<WindowState>,
    /// Where and how large the window opens, in screen points; taken when it first shows.
    wanted: Option<(Option<Pos2>, Vec2)>,
}

/// Where a window is on screen, as last seen, for dragging tabs between windows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WindowGeometry {
    /// The desktop said where the window is (or the window is embedded in the main one): the
    /// rectangles below mean something. Without it (Wayland) a tab can't be dropped on the window.
    pub known: bool,
    /// The screen position of the window's (0, 0), in points.
    pub origin: Pos2,
    /// The window's content on screen, in points.
    pub rect: Rect,
    /// The tab strip, in the window's coordinates.
    pub strip: Option<Rect>,
    /// Each document tab's rect, in the window's coordinates, in strip order.
    pub tabs: Vec<(DocId, Rect)>,
    /// The window has the keyboard focus.
    pub focused: bool,
    /// Whether the app drew the window's frame when it was last synced (see `sync_window_frame`).
    pub app_frame: Option<bool>,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        Self { known: false, origin: Pos2::ZERO, rect: Rect::NOTHING, strip: None, tabs: Vec::new(), focused: false, app_frame: None }
    }
}

/// A document tab being dragged, for the strips of the other windows (they draw the drop
/// marker) and for the drop.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabDrag {
    pub doc: DocId,
    pub from: WindowId,
    /// Where the pointer was last seen, in the dragged-from window's coordinates. When the tab
    /// is released over another window, X11 reports the release and the pointer leaving the
    /// window in one frame (the implicit grab ends), so the release frame has no pointer
    /// position of its own.
    pub pointer: Pos2,
    /// The window the pointer is over, and the tab index it would be dropped at (`None`: after
    /// the last tab).
    pub over: Option<(WindowId, Option<usize>)>,
    /// The pointer is outside the strip's detach zone: releasing detaches the tab.
    pub outside: bool,
    /// Escape was pressed: releasing changes nothing.
    pub cancelled: bool,
}

/// A change to the set of windows, queued while windows are drawn and applied after the frame.
#[derive(Clone, Debug, PartialEq)]
pub enum WindowOp {
    /// Move the tab of `doc` out of window `from`: into a new window (`to: None`, placed so
    /// the tab sits at `at`, a screen position, if given), or into window `to` at a tab
    /// index (`None`: after its last tab).
    MoveTab { doc: DocId, from: WindowId, to: Option<(WindowId, Option<usize>)>, at: Option<Pos2> },
    /// Close the window and its tabs (its unsaved-changes questions have been answered).
    Close(WindowId),
    /// Every tab back into the main window.
    MergeAll,
    /// Bring the window to the front.
    Focus(WindowId),
    /// Quit, asking about unsaved changes first (from a window that is not the main one).
    Quit,
}

impl PdfCraftApp {
    /// Exchange the window the app fields hold with `state`.
    ///
    /// Every field of [`WindowState`] is named here, so a field added there without being
    /// swapped does not compile.
    pub(crate) fn swap_window(&mut self, state: &mut WindowState) {
        let WindowState {
            views,
            active,
            mode,
            mode_override,
            left,
            left_open,
            right,
            right_before_read,
            quick_tool,
            space_hand,
            date_format_draft,
            dialog,
            dialog_seen,
            dialog_epoch,
            palette_open,
            palette_query,
            all_tools_expanded,
            toast,
            password_prompt,
            unlock_prompt,
            full_screen,
            close_request,
            #[cfg(not(target_arch = "wasm32"))]
            pending_overwrite,
            props_draft,
            view_draft,
            split_draft,
            extract_draft,
            rotate_draft,
            sign_draft,
            ocr_draft,
            wizard,
            compare_old,
            compare,
            doc_js,
            a11y,
            tags_state,
            alt_draft,
            cert_viewer,
            space_audit,
            image_import,
            stamp_draft,
            optimize_draft,
            progress_notice,
            grid_drop,
            duplicate_draft,
            form_preview,
            window_title,
            deferred_commands,
            bookmark_rename,
            number_draft,
            word_count_draft,
            protect_draft,
            boxes_draft,
            marks_draft,
            export_draft,
            signature_draft,
            signature_preview,
            comment_props,
            field_props,
            bulk_field_props,
            redact_pages_draft,
            redact_search,
            hidden_draft,
            print_draft,
            link_draft,
            replace_draft,
            pending_link,
            pending_full_screen,
            pending_remote_link,
            pdfa,
            pending_panel_reveals,
        } = state;
        std::mem::swap(&mut self.views, views);
        std::mem::swap(&mut self.active, active);
        std::mem::swap(&mut self.mode, mode);
        std::mem::swap(&mut self.mode_override, mode_override);
        std::mem::swap(&mut self.left, left);
        std::mem::swap(&mut self.left_open, left_open);
        std::mem::swap(&mut self.right, right);
        std::mem::swap(&mut self.right_before_read, right_before_read);
        std::mem::swap(&mut self.quick_tool, quick_tool);
        std::mem::swap(&mut self.space_hand, space_hand);
        std::mem::swap(&mut self.date_format_draft, date_format_draft);
        std::mem::swap(&mut self.dialog, dialog);
        std::mem::swap(&mut self.dialog_seen, dialog_seen);
        std::mem::swap(&mut self.dialog_epoch, dialog_epoch);
        std::mem::swap(&mut self.palette_open, palette_open);
        std::mem::swap(&mut self.palette_query, palette_query);
        std::mem::swap(&mut self.all_tools_expanded, all_tools_expanded);
        std::mem::swap(&mut self.toast, toast);
        std::mem::swap(&mut self.password_prompt, password_prompt);
        std::mem::swap(&mut self.unlock_prompt, unlock_prompt);
        std::mem::swap(&mut self.full_screen, full_screen);
        std::mem::swap(&mut self.close_request, close_request);
        #[cfg(not(target_arch = "wasm32"))]
        std::mem::swap(&mut self.pending_overwrite, pending_overwrite);
        std::mem::swap(&mut self.props_draft, props_draft);
        std::mem::swap(&mut self.view_draft, view_draft);
        std::mem::swap(&mut self.split_draft, split_draft);
        std::mem::swap(&mut self.extract_draft, extract_draft);
        std::mem::swap(&mut self.rotate_draft, rotate_draft);
        std::mem::swap(&mut self.sign_draft, sign_draft);
        std::mem::swap(&mut self.ocr_draft, ocr_draft);
        std::mem::swap(&mut self.wizard, wizard);
        std::mem::swap(&mut self.compare_old, compare_old);
        std::mem::swap(&mut self.compare, compare);
        std::mem::swap(&mut self.doc_js, doc_js);
        std::mem::swap(&mut self.a11y, a11y);
        std::mem::swap(&mut self.tags_state, tags_state);
        std::mem::swap(&mut self.alt_draft, alt_draft);
        std::mem::swap(&mut self.cert_viewer, cert_viewer);
        std::mem::swap(&mut self.space_audit, space_audit);
        std::mem::swap(&mut self.image_import, image_import);
        std::mem::swap(&mut self.stamp_draft, stamp_draft);
        std::mem::swap(&mut self.optimize_draft, optimize_draft);
        std::mem::swap(&mut self.progress_notice, progress_notice);
        std::mem::swap(&mut self.grid_drop, grid_drop);
        std::mem::swap(&mut self.duplicate_draft, duplicate_draft);
        std::mem::swap(&mut self.form_preview, form_preview);
        std::mem::swap(&mut self.window_title, window_title);
        std::mem::swap(&mut self.deferred_commands, deferred_commands);
        std::mem::swap(&mut self.bookmark_rename, bookmark_rename);
        std::mem::swap(&mut self.number_draft, number_draft);
        std::mem::swap(&mut self.word_count_draft, word_count_draft);
        std::mem::swap(&mut self.protect_draft, protect_draft);
        std::mem::swap(&mut self.boxes_draft, boxes_draft);
        std::mem::swap(&mut self.marks_draft, marks_draft);
        std::mem::swap(&mut self.export_draft, export_draft);
        std::mem::swap(&mut self.signature_draft, signature_draft);
        std::mem::swap(&mut self.signature_preview, signature_preview);
        std::mem::swap(&mut self.comment_props, comment_props);
        std::mem::swap(&mut self.field_props, field_props);
        std::mem::swap(&mut self.bulk_field_props, bulk_field_props);
        std::mem::swap(&mut self.redact_pages_draft, redact_pages_draft);
        std::mem::swap(&mut self.redact_search, redact_search);
        std::mem::swap(&mut self.hidden_draft, hidden_draft);
        std::mem::swap(&mut self.print_draft, print_draft);
        std::mem::swap(&mut self.link_draft, link_draft);
        std::mem::swap(&mut self.replace_draft, replace_draft);
        std::mem::swap(&mut self.pending_link, pending_link);
        std::mem::swap(&mut self.pending_full_screen, pending_full_screen);
        std::mem::swap(&mut self.pending_remote_link, pending_remote_link);
        std::mem::swap(&mut self.pdfa, pdfa);
        std::mem::swap(&mut self.pending_panel_reveals, pending_panel_reveals);
    }

    /// Make window `id` the current one: park the current window's state and load `id`'s.
    /// False when there is no such window (nothing changes).
    fn switch_to(&mut self, id: WindowId) -> bool {
        if id == self.current_window {
            return true;
        }
        if !self.has_window(id) {
            return false;
        }
        let mut parked = WindowState::default();
        self.swap_window(&mut parked);
        let current = self.current_window;
        if current == WindowId::ROOT {
            self.root_state = Some(parked);
        } else if let Some(slot) = self.windows.iter_mut().find(|w| w.id == current) {
            slot.state = Some(parked);
        }
        let mut loaded =
            if id == WindowId::ROOT { self.root_state.take() } else { self.windows.iter_mut().find(|w| w.id == id).and_then(|w| w.state.take()) }
                .unwrap_or_default();
        self.swap_window(&mut loaded);
        self.current_window = id;
        true
    }

    /// Run `f` with window `id`'s state in the app fields, then put the current window's state
    /// back (also if `f` panics, so the window being drawn keeps its state). `None` when there
    /// is no such window.
    pub fn with_window<R>(&mut self, id: WindowId, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        let back_to = self.current_window;
        if !self.switch_to(id) {
            return None;
        }
        let mut guard = WindowGuard { app: self, back_to };
        Some(f(&mut guard))
    }

    /// The window whose state the app fields hold: the one being drawn, or the main window
    /// between frames.
    pub fn current_window(&self) -> WindowId {
        self.current_window
    }

    /// Whether `id` is an open window.
    pub fn has_window(&self, id: WindowId) -> bool {
        id == WindowId::ROOT || self.windows.iter().any(|w| w.id == id)
    }

    /// Every window, the main one first, then the others in the order they opened.
    pub fn window_ids(&self) -> Vec<WindowId> {
        std::iter::once(WindowId::ROOT).chain(self.windows.iter().map(|w| w.id)).collect()
    }

    /// How many windows are open.
    pub fn window_count(&self) -> usize {
        self.windows.len() + 1
    }

    /// The window that has the keyboard focus (the main window until another one gets it).
    pub fn focused_window(&self) -> WindowId {
        self.focused_window
    }

    /// Whether a document tab is being dragged (tests).
    pub fn tab_drag_in_progress(&self) -> bool {
        self.tab_drag.is_some()
    }

    /// Every window's id and the name in its title ("PdfCraft" for a window showing Home).
    pub fn window_titles(&self) -> Vec<(WindowId, String)> {
        let name = |title: &str| {
            let name = title.strip_suffix(" — PdfCraft").unwrap_or(title);
            if name.is_empty() { "PdfCraft".to_owned() } else { name.to_owned() }
        };
        self.window_ids()
            .into_iter()
            .map(|id| {
                let title = if id == self.current_window {
                    name(&self.window_title)
                } else if id == WindowId::ROOT {
                    name(self.root_state.as_ref().map_or("", |s| s.window_title.as_str()))
                } else {
                    name(self.windows.iter().find(|w| w.id == id).and_then(|w| w.state.as_ref()).map_or("", |s| s.window_title.as_str()))
                };
                (id, title)
            })
            .collect()
    }

    /// The window the tab of `doc` is in.
    pub fn window_of_doc(&self, doc: DocId) -> Option<WindowId> {
        self.all_views().find(|(_, v)| v.id == doc).map(|(w, _)| w)
    }

    /// Every window's views: the current window's, then the parked windows'.
    pub(crate) fn all_views(&self) -> impl Iterator<Item = (WindowId, &DocView)> {
        let current = self.current_window;
        let root = self.root_state.iter().flat_map(|s| s.views.iter().map(|v| (WindowId::ROOT, v)));
        let others = self.windows.iter().flat_map(|w| w.state.iter().flat_map(move |s| s.views.iter().map(move |v| (w.id, v))));
        self.views.iter().map(move |v| (current, v)).chain(root).chain(others)
    }

    /// Queue a change to the windows; it is applied after this frame's passes.
    pub fn queue_window_op(&mut self, op: WindowOp) {
        self.window_ops.push(op);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    /// Apply the queued window changes. Called between passes (every window's state is where it
    /// belongs); tests call it in place of a frame.
    pub fn apply_window_ops(&mut self, ctx: &egui::Context) {
        // A drag that ended without the strip seeing its release (the window lost the pointer)
        // must not leave the other strips marking a drop.
        if let Some(drag) = &self.tab_drag
            && !ctx.input_for(drag.from.viewport(), |i| i.pointer.any_down())
        {
            self.tab_drag = None;
        }
        let ops = std::mem::take(&mut self.window_ops);
        if ops.is_empty() {
            // A window whose last tab was closed from its strip still closes.
            self.repair_windows();
            return;
        }
        for op in ops {
            match op {
                WindowOp::MoveTab { doc, from, to, at } => self.move_tab_to_window(doc, from, to, at),
                WindowOp::Close(id) => self.close_window(id),
                WindowOp::MergeAll => self.merge_all_windows(),
                WindowOp::Focus(id) => self.focus_window(id),
                WindowOp::Quit => {
                    self.request_quit(ctx);
                }
            }
        }
        self.repair_windows();
        ctx.request_repaint();
    }

    /// Whether the tab at `index` of the current window may leave it for a new window: any tab
    /// of the main window, and a tab of another window that has more than one (the only tab of
    /// a window is the window; drag the window instead).
    pub(crate) fn can_detach(&self, index: usize) -> bool {
        Self::windows_supported() && index < self.views.len() && (self.current_window == WindowId::ROOT || self.views.len() > 1)
    }

    /// A window for `view`, laid out like window `from` (same mode and panels), at `at` (a screen
    /// position the tab is dropped at) or a little below and to the right of `from`.
    fn open_window_with(&mut self, view: DocView, from: WindowId, at: Option<Pos2>) {
        let layout = self.with_window(from, |a| (a.mode, a.left, a.left_open, a.right, a.all_tools_expanded));
        let (mode, left, left_open, right, all_tools_expanded) =
            layout.unwrap_or((self.mode, self.left, self.left_open, self.right, self.all_tools_expanded));
        let source = self.window_geometry.get(&from).filter(|g| g.known).map(|g| g.rect);
        let size = source.map_or(DEFAULT_SIZE, |r| r.size()).max(MIN_SIZE);
        let pos = at.map(|p| p - TAB_GRIP).or_else(|| source.map(|r| r.min + CASCADE));
        let id = WindowId(self.next_window_id);
        self.next_window_id = self.next_window_id.saturating_add(1);
        let state = WindowState { views: vec![view], active: Some(0), mode, left, left_open, right, all_tools_expanded, ..WindowState::default() };
        self.windows.push(WindowSlot { id, state: Some(state), wanted: Some((pos, size)) });
        self.focused_window = id;
    }

    /// Take the view of `doc` out of window `from` (the document stays open).
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

    /// `WindowOp::MoveTab`: see there. A window other than the main one closes when its last
    /// tab leaves it.
    fn move_tab_to_window(&mut self, doc: DocId, from: WindowId, to: Option<(WindowId, Option<usize>)>, at: Option<Pos2>) {
        if let Some((target, _)) = to
            && (target == from || !self.has_window(target))
        {
            return;
        }
        if to.is_none() {
            // Checked before the view leaves its window, so a refusal changes nothing.
            if self.window_count() >= MAX_WINDOWS {
                self.notify_tr("Too many windows are open");
                return;
            }
            let may_leave = self.with_window(from, |a| a.views.iter().position(|v| v.id == doc).is_some_and(|i| a.can_detach(i)));
            if !may_leave.unwrap_or(false) {
                return;
            }
        }
        let Some(view) = self.take_view(doc, from) else { return };
        match to {
            None => self.open_window_with(view, from, at),
            Some((target, index)) => {
                self.with_window(target, |a| {
                    let i = index.map_or(a.views.len(), |i| i.min(a.views.len()));
                    a.views.insert(i, view);
                    a.active = Some(i);
                });
                self.focus_window(target);
            }
        }
        if from != WindowId::ROOT && from != self.current_window && self.with_window(from, |a| a.views.is_empty()).unwrap_or(false) {
            self.close_window(from);
        }
    }

    /// Close window `id` with its tabs. The main window, and the window being drawn, cannot be
    /// closed this way (the main window closes with the app, through `guard_quit`).
    fn close_window(&mut self, id: WindowId) {
        if id == WindowId::ROOT || id == self.current_window {
            return;
        }
        self.with_window(id, |a| {
            for i in (0..a.views.len()).rev() {
                a.close_tab(i);
            }
        });
        self.windows.retain(|w| w.id != id);
        self.window_geometry.remove(&id);
        if self.focused_window == id {
            self.focused_window = WindowId::ROOT;
        }
    }

    /// Every tab back into the main window, after its own tabs, in window order; the other
    /// windows close. Runs with the main window current (otherwise it is queued).
    pub(crate) fn merge_all_windows(&mut self) {
        if self.current_window != WindowId::ROOT {
            self.queue_window_op(WindowOp::MergeAll);
            return;
        }
        let first_merged = self.views.len();
        for slot in std::mem::take(&mut self.windows) {
            if let Some(mut state) = slot.state {
                self.views.append(&mut state.views);
            }
            self.window_geometry.remove(&slot.id);
        }
        if self.active.is_none() && !self.combine_showing() && first_merged < self.views.len() {
            self.active = Some(first_merged);
        }
        self.focused_window = WindowId::ROOT;
    }

    /// Bring window `id` to the front.
    fn focus_window(&mut self, id: WindowId) {
        if !self.has_window(id) {
            return;
        }
        self.focused_window = id;
        if let Some(ctx) = &self.ctx {
            ctx.send_viewport_cmd_to(id.viewport(), egui::ViewportCommand::Focus);
        }
    }

    /// Window ▸ Close window, from the current window: the main window's close is the app's
    /// (with its quit guard); another window asks about its unsaved changes first.
    pub(crate) fn request_close_window(&mut self) {
        if self.current_window == WindowId::ROOT {
            if let Some(ctx) = &self.ctx {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            }
        } else if self.first_dirty().is_some() {
            self.close_request = Some(CloseRequest::Window);
        } else {
            self.queue_window_op(WindowOp::Close(self.current_window));
        }
    }

    /// The desktop's close button on a window other than the main one: like
    /// [`Self::request_close_window`], but the window stays until the questions are answered.
    pub(crate) fn guard_close_window(&mut self, ctx: &egui::Context) {
        if self.current_window == WindowId::ROOT || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.first_dirty().is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_request = Some(CloseRequest::Window);
        } else {
            self.queue_window_op(WindowOp::Close(self.current_window));
        }
    }

    /// Windows whose last tab went (closed from its strip) close; a window that has gone no
    /// longer has the focus.
    fn repair_windows(&mut self) {
        let current = self.current_window;
        let empty: Vec<WindowId> =
            self.windows.iter().filter(|w| w.id != current && w.state.as_ref().is_some_and(|s| s.views.is_empty())).map(|w| w.id).collect();
        for id in empty {
            self.close_window(id);
        }
        if !self.has_window(self.focused_window) {
            self.focused_window = WindowId::ROOT;
        }
    }

    /// Note where window `id` is and whether it has the focus. `embedded` is the window's
    /// rect in the main window when it is drawn inside it (tests, and backends without
    /// several windows).
    pub(crate) fn track_window(&mut self, id: WindowId, ctx: &egui::Context, embedded: Option<Rect>) {
        let (inner, focused) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().focused));
        let root_origin = self.window_geometry.get(&WindowId::ROOT).map_or(Pos2::ZERO, |g| g.origin);
        let geometry = self.window_geometry.entry(id).or_default();
        match (embedded, inner) {
            (Some(rect), _) => {
                geometry.known = true;
                geometry.origin = root_origin;
                geometry.rect = rect.translate(root_origin.to_vec2());
            }
            (None, Some(rect)) if rect.is_finite() => {
                geometry.known = true;
                geometry.origin = rect.min;
                geometry.rect = rect;
            }
            // Only the main window's place may be unknown and still count: embedded windows
            // are placed in its coordinates. (On Wayland the desktop keeps every window's
            // place to itself: the main window counts, its coordinates are its own, and the
            // other windows stay unknown, so no tab is dropped on the wrong one.)
            (None, None) if id == WindowId::ROOT => {
                geometry.known = true;
                geometry.origin = Pos2::ZERO;
                geometry.rect = ctx.content_rect();
            }
            _ => geometry.known = false,
        }
        if let Some(focused) = focused {
            geometry.focused = focused;
            if focused {
                self.focused_window = id;
            }
        }
    }

    /// The tab strip's place and its tabs' places in the current window, from the strip.
    pub(crate) fn note_strip_geometry(&mut self, strip: Rect, tabs: Vec<(DocId, Rect)>) {
        let geometry = self.window_geometry.entry(self.current_window).or_default();
        geometry.strip = Some(strip);
        geometry.tabs = tabs;
    }

    /// `local`, a position in the current window, on screen; `None` where the desktop keeps
    /// the window's place to itself.
    pub(crate) fn screen_pos(&self, local: Pos2) -> Option<Pos2> {
        let geometry = self.window_geometry.get(&self.current_window).filter(|g| g.known)?;
        Some(geometry.origin + local.to_vec2())
    }

    /// The window under the screen position `screen` other than `except`: the focused one if
    /// it is there (windows may overlap), otherwise any.
    pub(crate) fn window_under(&self, screen: Pos2, except: WindowId) -> Option<WindowId> {
        let candidates = || self.window_geometry.iter().filter(|(id, g)| **id != except && g.known && g.rect.contains(screen));
        candidates().find(|(_, g)| g.focused).or_else(|| candidates().next()).map(|(id, _)| *id)
    }

    /// Where a tab dropped at the screen position `screen` lands in window `target`: before the
    /// first tab whose centre is right of the pointer when the pointer is in the strip, after
    /// the last tab when it is elsewhere in the window.
    pub(crate) fn drop_index_in(&self, target: WindowId, screen: Pos2) -> Option<usize> {
        let geometry = self.window_geometry.get(&target)?;
        let local = screen - geometry.origin.to_vec2();
        let strip = geometry.strip?;
        if !strip.contains(local) {
            return None;
        }
        Some(geometry.tabs.iter().filter(|(_, r)| r.center().x < local.x).count())
    }

    /// The settings a window other than the main one opens with (`wanted` on its first show).
    fn window_builder(&self, title: &str, wanted: Option<(Option<Pos2>, Vec2)>) -> egui::ViewportBuilder {
        let mut builder =
            egui::ViewportBuilder::default().with_title(title).with_app_id(APP_ID).with_min_inner_size(MIN_SIZE).with_drag_and_drop(true);
        if let Some(icon) = &self.window_icon {
            builder = builder.with_icon(icon.clone());
        }
        if let Some((pos, size)) = wanted {
            builder = builder.with_inner_size(size).with_active(true);
            if let Some(pos) = pos {
                builder = builder.with_position(pos);
            }
        }
        if cfg!(target_os = "macos") && self.integrated_titlebar {
            builder = builder.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
        }
        if self.draws_window_frame() {
            // The tab strip is the title bar (window_controls), as in the main window.
            builder = builder.with_decorations(false);
        }
        builder
    }

    /// Draw the windows besides the main one, each as an egui viewport with its state loaded
    /// for the duration of its pass. Called from the main window's pass.
    pub(crate) fn show_child_windows(&mut self, ctx: &egui::Context) {
        let embedded = ctx.embed_viewports();
        let ids: Vec<WindowId> = self.windows.iter().map(|w| w.id).collect();
        for id in ids {
            let Some(slot) = self.windows.iter_mut().find(|w| w.id == id) else { continue };
            let title = slot.state.as_ref().map(|s| s.window_title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "PdfCraft".to_owned());
            let viewport = id.viewport();
            if embedded {
                // No windows here (tests; a backend without them): an egui window inside the main
                // one, at the place and a fraction of the size a window would get, so the main
                // window's strip stays reachable. (egui's own fallback ignores the position.)
                let (pos, size) = slot.wanted.unwrap_or((None, DEFAULT_SIZE));
                let mut window = egui::Window::new(title).id(egui::Id::new(viewport)).default_size(size * EMBEDDED_SCALE).collapsible(false);
                if let Some(pos) = pos {
                    window = window.default_pos(pos);
                }
                self.with_window(id, |app| {
                    window.show(ctx, |ui| app.window_pass(id, ui, egui::ViewportClass::EmbeddedWindow));
                });
            } else {
                let wanted = slot.wanted.take();
                let builder = self.window_builder(&title, wanted);
                self.with_window(id, |app| {
                    ctx.show_viewport_immediate(viewport, builder, |ui, class| app.window_pass(id, ui, class));
                });
            }
        }
    }

    /// One window's pass: where it is, its share of the frame's logic, and its contents.
    fn window_pass(&mut self, id: WindowId, ui: &mut egui::Ui, class: egui::ViewportClass) {
        let ctx = ui.ctx().clone();
        let embedded = matches!(class, egui::ViewportClass::EmbeddedWindow).then(|| ui.max_rect());
        self.track_window(id, &ctx, embedded);
        self.window_logic(&ctx, embedded.is_some());
        if embedded.is_some() { self.draw_embedded_window(ui) } else { self.draw_window(ui) }
    }

    /// What a window drawn inside the main one shows (tests; egui's fallback where a backend
    /// has no windows): its tab strip, so tabs can be dragged to and from it, and the active
    /// document's name. The full contents would repeat the main window's fixed widget ids in
    /// one viewport, which egui refuses.
    fn draw_embedded_window(&mut self, ui: &mut egui::Ui) {
        let name = self.active.and_then(|i| crate::canvas::tab_label(self, i));
        self.window_title = name.as_deref().map_or_else(|| "PdfCraft".to_owned(), |name| format!("{name} — PdfCraft"));
        crate::chrome::tab_strip(self, ui);
        egui::CentralPanel::default().show(ui, |ui| {
            ui.centered_and_justified(|ui| ui.label(name.unwrap_or_default()));
        });
    }

    /// Whether this build has several windows: the desktop app (the web build has one window).
    pub fn windows_supported() -> bool {
        cfg!(not(target_arch = "wasm32"))
    }
}

/// Puts the window that was current back when dropped (see [`PdfCraftApp::with_window`]).
struct WindowGuard<'a> {
    app: &'a mut PdfCraftApp,
    back_to: WindowId,
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
        self.app.switch_to(self.back_to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDF: &[u8] = include_bytes!("../tests/data/form.pdf");

    fn app_with(names: &[&str]) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        for name in names {
            app.open_bytes(name, None, PDF.to_vec()).unwrap();
        }
        app
    }

    fn names(app: &PdfCraftApp) -> Vec<String> {
        app.views.iter().filter_map(|v| app.session.get(v.id)).map(|d| d.name.clone()).collect()
    }

    #[test]
    fn swapping_a_state_twice_changes_nothing() {
        let mut app = app_with(&["a.pdf", "b.pdf"]);
        app.mode = Mode::Read;
        app.palette_query = "find".into();
        let mut other = WindowState::default();
        app.swap_window(&mut other);
        assert!(app.views.is_empty());
        assert_eq!(app.mode, Mode::AllTools);
        assert_eq!(other.views.len(), 2);
        assert_eq!(other.palette_query, "find");
        app.swap_window(&mut other);
        assert_eq!(app.views.len(), 2);
        assert_eq!(app.mode, Mode::Read);
        assert_eq!(app.palette_query, "find");
    }

    #[test]
    fn a_tab_moves_into_a_new_window_and_back() {
        let mut app = app_with(&["a.pdf", "b.pdf", "c.pdf"]);
        let ctx = egui::Context::default();
        let b = app.views[1].id;
        app.active = Some(1);
        app.queue_window_op(WindowOp::MoveTab { doc: b, from: WindowId::ROOT, to: None, at: None });
        app.apply_window_ops(&ctx);
        assert_eq!(app.window_count(), 2);
        assert_eq!(names(&app), ["a.pdf", "c.pdf"]);
        assert_eq!(app.active, Some(1), "the tab after the one that left is active");
        let second = app.windows[0].id;
        assert_eq!(app.window_of_doc(b), Some(second));
        assert_eq!(app.with_window(second, |a| (names(a), a.active)), Some((vec!["b.pdf".to_owned()], Some(0))));
        assert_eq!(app.current_window(), WindowId::ROOT, "the main window is current again");
        assert_eq!(app.session.docs().len(), 3, "the document is still open");
        // Back to the main window, in front of a.pdf.
        app.queue_window_op(WindowOp::MoveTab { doc: b, from: second, to: Some((WindowId::ROOT, Some(0))), at: None });
        app.apply_window_ops(&ctx);
        assert_eq!(app.window_count(), 1, "the window left without tabs closed");
        assert_eq!(names(&app), ["b.pdf", "a.pdf", "c.pdf"]);
        assert_eq!(app.active, Some(0));
        assert_eq!(app.session.docs().len(), 3);
    }

    #[test]
    fn the_only_tab_of_a_second_window_stays_but_the_main_windows_last_tab_may_leave() {
        let mut app = app_with(&["a.pdf"]);
        let ctx = egui::Context::default();
        let a = app.views[0].id;
        assert!(app.can_detach(0));
        app.queue_window_op(WindowOp::MoveTab { doc: a, from: WindowId::ROOT, to: None, at: None });
        app.apply_window_ops(&ctx);
        assert_eq!(app.window_count(), 2);
        assert!(app.views.is_empty() && app.active.is_none(), "the main window shows Home");
        let second = app.windows[0].id;
        assert_eq!(app.with_window(second, |a| a.can_detach(0)), Some(false));
        app.queue_window_op(WindowOp::MoveTab { doc: a, from: second, to: None, at: None });
        app.apply_window_ops(&ctx);
        assert_eq!(app.window_count(), 2, "nothing happened");
        assert_eq!(app.window_of_doc(a), Some(second));
    }

    #[test]
    fn closing_a_window_closes_its_documents_and_merging_brings_every_tab_home() {
        let mut app = app_with(&["a.pdf", "b.pdf", "c.pdf"]);
        let ctx = egui::Context::default();
        let (b, c) = (app.views[1].id, app.views[2].id);
        app.queue_window_op(WindowOp::MoveTab { doc: b, from: WindowId::ROOT, to: None, at: None });
        app.queue_window_op(WindowOp::MoveTab { doc: c, from: WindowId::ROOT, to: None, at: None });
        app.apply_window_ops(&ctx);
        assert_eq!(app.window_count(), 3);
        let ids = app.window_ids();
        app.queue_window_op(WindowOp::Close(ids[1]));
        app.apply_window_ops(&ctx);
        assert_eq!(app.window_count(), 2);
        assert!(app.session.get(b).is_none(), "b.pdf closed with its window");
        assert_eq!(app.closed_tabs.len(), 0, "a document without a path leaves no reopen entry");
        app.queue_window_op(WindowOp::MergeAll);
        app.apply_window_ops(&ctx);
        assert_eq!(app.window_count(), 1);
        assert_eq!(names(&app), ["a.pdf", "c.pdf"]);
        assert_eq!(app.active, Some(0), "the main window's own active tab stays active");
        assert_eq!(app.session.docs().len(), 2);
    }

    #[test]
    fn with_window_restores_the_current_window_after_a_panic() {
        let mut app = app_with(&["a.pdf", "b.pdf"]);
        let ctx = egui::Context::default();
        let b = app.views[1].id;
        app.queue_window_op(WindowOp::MoveTab { doc: b, from: WindowId::ROOT, to: None, at: None });
        app.apply_window_ops(&ctx);
        let second = app.windows[0].id;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.with_window(second, |a| {
                assert_eq!(a.current_window(), second);
                panic!("inside");
            })
        }));
        assert!(result.is_err());
        assert_eq!(app.current_window(), WindowId::ROOT);
        assert_eq!(names(&app), ["a.pdf"]);
        assert_eq!(app.with_window(second, |a| names(a)), Some(vec!["b.pdf".to_owned()]));
        assert_eq!(app.with_window(WindowId(99), |_| ()), None, "no such window");
    }

    #[test]
    fn a_window_under_a_point_is_found_by_its_last_known_place() {
        let mut app = app_with(&["a.pdf"]);
        app.window_geometry.insert(
            WindowId::ROOT,
            WindowGeometry { known: true, origin: Pos2::ZERO, rect: Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0)), ..Default::default() },
        );
        let second = WindowId(1);
        app.windows.push(WindowSlot { id: second, state: Some(WindowState::default()), wanted: None });
        app.window_geometry.insert(
            second,
            WindowGeometry {
                known: true,
                origin: Pos2::new(1200.0, 100.0),
                rect: Rect::from_min_size(Pos2::new(1200.0, 100.0), vec2(800.0, 600.0)),
                strip: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 38.0))),
                tabs: vec![(DocId(7), Rect::from_min_size(Pos2::new(50.0, 4.0), vec2(180.0, 30.0)))],
                ..Default::default()
            },
        );
        assert_eq!(app.screen_pos(Pos2::new(10.0, 20.0)), Some(Pos2::new(10.0, 20.0)));
        assert_eq!(app.window_under(Pos2::new(1300.0, 120.0), WindowId::ROOT), Some(second));
        assert_eq!(app.window_under(Pos2::new(1300.0, 120.0), second), None, "the dragged-from window never counts");
        assert_eq!(app.window_under(Pos2::new(3000.0, 120.0), WindowId::ROOT), None);
        // In the strip: before the tab whose centre is right of the pointer; past it: after.
        assert_eq!(app.drop_index_in(second, Pos2::new(1260.0, 120.0)), Some(0));
        assert_eq!(app.drop_index_in(second, Pos2::new(1400.0, 120.0)), Some(1));
        assert_eq!(app.drop_index_in(second, Pos2::new(1400.0, 400.0)), None, "below the strip: after the last tab");
        let unknown = WindowGeometry { rect: Rect::from_min_size(Pos2::ZERO, vec2(5000.0, 5000.0)), ..Default::default() };
        app.window_geometry.insert(WindowId(2), unknown);
        assert_eq!(app.window_under(Pos2::new(3000.0, 120.0), WindowId::ROOT), None, "a window of unknown place never matches");
    }
}
