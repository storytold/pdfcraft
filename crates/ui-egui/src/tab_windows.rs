//! Native tab tear-off. The document and view move in memory; no temporary PDF or save is used.
use crate::PdfCraftApp;
use pdfcraft_engine::DocId;

/// A small vertical dead zone keeps tab reordering and edge scrolling easy to perform.
pub(crate) fn outside_strip(strip: egui::Rect, viewport: egui::Rect, pointer: egui::Pos2) -> bool {
    pointer.y < strip.top() - 24.0 || pointer.y > strip.bottom() + 24.0 || pointer.x < viewport.left() - 24.0 || pointer.x > viewport.right() + 24.0
}

impl PdfCraftApp {
    /// NativeWindows is the settings owner. Synchronize before and after each window's
    /// sequential event pass, so a later window starts with every earlier change (including
    /// deletions), rather than merging stale snapshots at exit. Document/view state stays local.
    #[cfg(not(target_arch = "wasm32"))]
    fn copy_shared_settings_from(&mut self, source: &Self) {
        if self.recent != source.recent {
            self.recent.clone_from(&source.recent);
        }
        self.reopen_last_session = source.reopen_last_session;
        self.recent_limit = source.recent_limit;
        self.restore_last_view = source.restore_last_view;
        self.invert_middle_drag = source.invert_middle_drag;
        self.fill_text_color = source.fill_text_color;
        self.pinned_tools.clone_from(&source.pinned_tools);
        self.show_page_edge_shadows = source.show_page_edge_shadows;
        self.show_secured_notice = source.show_secured_notice;
        self.hotbar.tools.clone_from(&source.hotbar.tools);
        self.hotbar.position = source.hotbar.position;
        self.hotbar.vertical = source.hotbar.vertical;
        self.hotbar.button_size = source.hotbar.button_size;
        self.hotbar.dock.clone_from(&source.hotbar.dock);
        self.hotbar.full_side = source.hotbar.full_side;
        self.hotbar.placed = source.hotbar.placed;

        self.reload_changed_files = source.reload_changed_files;
        self.system_title_bar = source.system_title_bar;
        self.scroll_speed = source.scroll_speed;
        self.animation_effects = source.animation_effects;
        self.default_mode = source.default_mode;
        self.view_defaults = source.view_defaults;
        self.always_default_view = source.always_default_view;
        self.organize_zoom = source.organize_zoom;
        self.print_draft.paper = source.print_draft.paper;
        self.units = source.units;
        if self.language != source.language {
            self.language.clone_from(&source.language);
        }
        self.ui_scale = source.ui_scale;
        self.flatten_fill_sign_on_save = source.flatten_fill_sign_on_save;
        self.fill_text_size = source.fill_text_size;
        if self.signature != source.signature {
            self.signature.clone_from(&source.signature);
        }
        if self.initials != source.initials {
            self.initials.clone_from(&source.initials);
        }
        if self.sign_graphic != source.sign_graphic {
            self.sign_graphic.clone_from(&source.sign_graphic);
        }
        if self.digital_ids != source.digital_ids {
            self.digital_ids.clone_from(&source.digital_ids);
        }
        if self.custom_stamps != source.custom_stamps {
            self.custom_stamps.clone_from(&source.custom_stamps);
        }
        if self.custom_actions != source.custom_actions {
            self.custom_actions.clone_from(&source.custom_actions);
        }
        self.comment_prefs.clone_from(&source.comment_prefs);
        self.pinned.folders.clone_from(&source.pinned.folders);
        self.redact_defaults.clone_from(&source.redact_defaults);
        if self.reading_colors != source.reading_colors {
            self.set_reading_colors(source.reading_colors);
        }
        if self.theme_preference != source.theme_preference || self.theme != source.theme {
            self.set_theme_preference(source.theme_preference);
        }
        // These caches describe the shared egui Context, not an individual window.
        self.ui_scale_applied = source.ui_scale_applied;
        self.fonts_cjk = source.fonts_cjk;
        self.fonts_ready = source.fonts_ready;
        self.styled = source.styled;
        if self.session.date_format() != source.session.date_format() {
            let _ = self.session.set_date_format(source.session.date_format());
        }
        if self.session.date_language() != source.session.date_language() {
            let _ = self.session.set_date_language(source.session.date_language());
        }
        if self.session.javascript() != source.session.javascript() {
            self.session.set_javascript(source.session.javascript());
        }
        if self.session.trusted_certificates() != source.session.trusted_certificates() {
            self.apply_trusted_certificates(source.session.trusted_certificates().to_vec());
        }
    }

    /// Native hosts opt in only when they can keep the detached document alive.
    pub fn can_detach_tab(&self, id: DocId) -> bool {
        self.transfer_ready() && self.detach_request.is_none() && self.session.get(id).is_some() && self.views.iter().any(|v| v.id == id)
    }

    /// Refuse transfers while a modal, worker or save operation still owns document state.
    fn transfer_ready(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.pickers.pending() || self.pending_overwrite.is_some() {
            return false;
        }
        self.managed_windows
            && !self.window_close_ready
            && !self.modal_open()
            && self.compare.is_none()
            && self.ocr_run.is_none()
            && self.ocr_batch.is_none()
            && self.action_run.is_none()
            && self.optimize_run.is_none()
            && self.export_status.is_none()
            && self.image_import.is_none()
    }

    /// Queue a move by document identity, so reordering cannot transfer the wrong tab.
    pub fn request_detach_tab(&mut self, id: DocId, position: Option<egui::Pos2>) -> bool {
        if !self.can_detach_tab(id) {
            return false;
        }
        self.detach_request = Some((id, position.filter(|p| p.x.is_finite() && p.y.is_finite() && p.x.abs() <= 100_000.0 && p.y.abs() <= 100_000.0)));
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
        true
    }

    /// Move a tab into an existing window without reparsing its PDF or losing its editor.
    pub fn transfer_tab_to(&mut self, id: DocId, target: &mut Self, at: usize) -> bool {
        if !self.can_detach_tab(id) || !target.transfer_ready() || at > target.views.len() {
            return false;
        }
        let Some(index) = self.views.iter().position(|v| v.id == id) else { return false };
        let Some(new_id) = self.session.transfer_document(id, &mut target.session) else { return false };
        let mut view = self.views.remove(index);
        view.id = new_id;
        view.restore_transferred_page();
        target.views.insert(at, view);
        target.active = Some(at);
        target.combine_tab.focused = false;
        target.startup_superseded = true;
        if let Some(key) = self.recovery_keys.remove(&id) {
            target.recovery_keys.insert(new_id, key);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.watch.transfer_as(id, &mut target.watch, new_id);
        self.active = match self.active {
            _ if self.views.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active >= self.views.len() => Some(self.views.len() - 1),
            other => other,
        };
        self.deferred_commands.retain(|(_, doc)| *doc != Some(id));
        true
    }

    /// Copy the current PDF pages into another window as one undoable insertion.
    /// Extract uses the unlocked engine document and enforces its page-copy permissions.
    pub fn insert_tab_pages_into(&mut self, id: DocId, target: &mut Self, into: DocId, at: usize) -> bool {
        if !self.can_detach_tab(id) || !target.transfer_ready() || target.active_ids().map(|(_, id)| id) != Some(into) {
            return false;
        }
        if !target.session.get(into).is_some_and(|d| d.allows_assembly() && at <= d.info.pages.len()) {
            return false;
        }
        let Some(index) = self.views.iter().position(|v| v.id == id) else { return false };
        let previous = self.active;
        self.active = Some(index);
        let committed = self.commit_form_typing();
        self.active = previous;
        if !committed || !target.commit_form_typing() {
            return false;
        }
        let Some(doc) = self.session.get(id) else { return false };
        let name = doc.name.clone();
        let count = doc.info.pages.len();
        let bytes = match self.session.extract(id, &(0..count).collect::<Vec<_>>()) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.notify_error(error);
                return false;
            }
        };
        if !target.apply_edit(pdfcraft_engine::Edit::InsertPagesFrom {
            name,
            bytes,
            pages: None,
            at,
            field_conflicts: pdfcraft_engine::FieldConflicts::Rename,
        }) {
            return false;
        }
        if let Some(view) = target.active.and_then(|i| target.views.get_mut(i)) {
            view.select_pages(&(at..at.saturating_add(count)).collect::<Vec<_>>());
            view.go_to_page(at);
        }
        true
    }

    /// Transfer ownership only when a native host is ready to display the new viewport.
    #[cfg(not(target_arch = "wasm32"))]
    fn take_detached_tab(&mut self) -> Option<(Self, Option<egui::Pos2>)> {
        let (id, position) = self.detach_request.take()?;
        if !self.can_detach_tab(id) {
            return None;
        }
        let index = self.views.iter().position(|v| v.id == id)?;
        let mut child = Self::new();
        child.restore(&self.persist());
        child.mode = self.mode;
        child.mode_override = self.mode_override;
        child.motion.reduced = self.motion.reduced;
        child.quick_tool = self.quick_tool;
        child.form_preview = self.form_preview;
        child.system_text_scale = self.system_text_scale;
        child.system_clipboard = self.system_clipboard;
        child.integrated_titlebar = self.integrated_titlebar;
        child.window_controls = self.window_controls.clone();
        child.printer_source = self.printer_source.clone();
        child.options_source = self.options_source.clone();
        child.os_key_store_ids = self.os_key_store_ids;
        child.run_inline = self.run_inline;
        child.recovery = self.recovery.clone();
        child.managed_windows = true;
        child.startup_superseded = true;
        child.styled = self.styled;
        child.fonts_ready = self.fonts_ready;
        child.fonts_cjk = self.fonts_cjk;
        // Every allocation/settings operation precedes removing the original document.
        child.session = self.session.detach_document(id)?;
        let mut view = self.views.remove(index);
        view.restore_transferred_page();
        child.views.push(view);
        child.active = Some(0);
        if let Some(key) = self.recovery_keys.remove(&id) {
            child.recovery_keys.insert(id, key);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.watch.transfer(id, &mut child.watch);
        self.active = match self.active {
            _ if self.views.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active >= self.views.len() => Some(self.views.len() - 1),
            other => other,
        };
        self.deferred_commands.retain(|(_, doc)| *doc != Some(id));
        Some((child, position))
    }
}

/// Keep a dropped window's title bar on the monitor, including releases by the taskbar.
#[cfg(target_os = "windows")]
fn visible_position(position: egui::Pos2, scale: f32) -> egui::Pos2 {
    let scale = if scale.is_finite() && (0.01..=100.0).contains(&scale) { scale } else { 1.0 };
    let point = winsafe::POINT { x: (position.x * scale) as i32, y: (position.y * scale) as i32 };
    let monitor = winsafe::HMONITOR::MonitorFromPoint(point, winsafe::co::MONITOR::DEFAULTTONEAREST);
    let Ok(info) = monitor.GetMonitorInfo() else { return position };
    let area = info.rcWork;
    let left = area.left as f32 / scale;
    let top = area.top as f32 / scale;
    egui::pos2(
        position.x.clamp(left, (area.right as f32 / scale - 1100.0).max(left)),
        position.y.clamp(top, (area.bottom as f32 / scale - 800.0).max(top)),
    )
}

/// Other native hosts retain the requested logical position without Windows monitor clamping.
#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn visible_position(position: egui::Pos2, _scale: f32) -> egui::Pos2 {
    position
}

/// One desktop process hosts independent native windows, each with its own document session.
/// Closing the original window hides its root viewport while detached windows remain alive.
#[cfg(not(target_arch = "wasm32"))]
pub struct NativeWindows {
    root: PdfCraftApp,
    windows: Vec<Window>,
    next: u64,
    root_hidden: bool,
    final_session: Option<crate::last_session::LastSession>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Window {
    id: egui::ViewportId,
    app: PdfCraftApp,
    position: Option<egui::Pos2>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeWindows {
    /// Enable tear-off for a native application without changing its existing open documents.
    pub fn new(mut root: PdfCraftApp) -> Self {
        root.managed_windows = true;
        Self { root, windows: Vec::new(), next: 0, root_hidden: false, final_session: None }
    }

    /// Collect bounded, deduplicated reopen entries from every visible managed window.
    fn combined_session(&self) -> crate::last_session::LastSession {
        let mut all = if self.root_hidden { Default::default() } else { self.root.session_to_save() };
        for window in &self.windows {
            let session = window.app.session_to_save();
            if all.active.is_none() {
                all.active = session.active;
            }
            for file in session.files {
                if !all.files.iter().any(|f| f.path == file.path) {
                    all.files.push(file);
                }
            }
        }
        all.files.truncate(crate::last_session::MAX_FILES);
        all
    }

    /// Borrow two distinct managed applications without aliasing their document sessions.
    fn pair_mut(&mut self, source: egui::ViewportId, target: egui::ViewportId) -> Option<(&mut PdfCraftApp, &mut PdfCraftApp)> {
        if source == target {
            return None;
        }
        if source == egui::ViewportId::ROOT {
            return Some((&mut self.root, &mut self.windows.iter_mut().find(|w| w.id == target)?.app));
        }
        if target == egui::ViewportId::ROOT {
            return Some((&mut self.windows.iter_mut().find(|w| w.id == source)?.app, &mut self.root));
        }
        let a = self.windows.iter().position(|w| w.id == source)?;
        let b = self.windows.iter().position(|w| w.id == target)?;
        if a < b {
            let (left, right) = self.windows.split_at_mut(b);
            Some((&mut left.get_mut(a)?.app, &mut right.first_mut()?.app))
        } else {
            let (left, right) = self.windows.split_at_mut(a);
            Some((&mut right.first_mut()?.app, &mut left.get_mut(b)?.app))
        }
    }

    /// Consume queued drags, accepting tab/page destinations before creating a detached window.
    fn collect(&mut self, ctx: &egui::Context) {
        let mut requests = Vec::new();
        if let Some(request) = self.root.detach_request.take() {
            requests.push((egui::ViewportId::ROOT, request));
        }
        for window in &mut self.windows {
            if let Some(request) = window.app.detach_request.take() {
                requests.push((window.id, request));
            }
        }
        let mut arriving = Vec::new();
        for (source, (id, position)) in requests {
            if let Some((target, drop)) = position.and_then(|p| crate::window_drop::destination(ctx, source, p)) {
                if let Some((from, to)) = self.pair_mut(source, target) {
                    let accepted = match drop {
                        crate::window_drop::Target::Tabs(at) => from.transfer_tab_to(id, to, at),
                        crate::window_drop::Target::Pages { doc, at } => from.insert_tab_pages_into(id, to, doc, at),
                        crate::window_drop::Target::Reject => false,
                    };
                    if accepted {
                        ctx.send_viewport_cmd_to(target, egui::ViewportCommand::Focus);
                        ctx.request_repaint_of(source);
                        ctx.request_repaint_of(target);
                    }
                }
                // A rejected drop onto an existing window leaves the source untouched.
                continue;
            }
            let app = if source == egui::ViewportId::ROOT {
                Some(&mut self.root)
            } else {
                self.windows.iter_mut().find(|w| w.id == source).map(|w| &mut w.app)
            };
            if let Some(app) = app {
                app.detach_request = Some((id, position));
                if let Some(tab) = app.take_detached_tab() {
                    arriving.push(tab);
                }
            }
        }
        for (app, position) in arriving {
            self.next = self.next.wrapping_add(1);
            self.windows.push(Window { id: egui::ViewportId::from_hash_of(("pdfcraft-document-window", self.next)), app, position });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl eframe::App for NativeWindows {
    /// Route native input to the application that owns the receiving viewport.
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if ctx.viewport_id() == egui::ViewportId::ROOT {
            self.root.raw_input_hook(ctx, input);
        } else if let Some(window) = self.windows.iter_mut().find(|w| w.id == ctx.viewport_id()) {
            window.app.raw_input_hook(ctx, input);
        }
    }

    /// Persist shared preferences and the combined session even after the root window is hidden.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.root.save(storage);
        if let Ok(mut saved) = serde_json::from_str::<serde_json::Value>(&self.root.persist()) {
            let session = self.final_session.clone().unwrap_or_else(|| self.combined_session());
            saved["last_session"] = serde_json::json!(self.root.reopen_last_session.then_some(session));
            storage.set_string("pdfcraft", saved.to_string());
        }
    }

    /// Keep the root event loop alive and reveal it when new documents arrive.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.root_hidden {
            self.root.window_close_ready = false;
            self.root.allow_quit = false;
        }
        self.root.logic(ctx, frame);
        if self.root_hidden && (!self.root.views.is_empty() || !self.root.pending_opens.is_empty()) {
            self.root_hidden = false;
            self.final_session = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    /// Render managed viewports sequentially, sharing preferences while retaining independent close guards.
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.root_hidden {
            self.root.ui(ui, frame);
        }
        for window in &mut self.windows {
            window.app.copy_shared_settings_from(&self.root);
            window.app.motion.reduced = self.root.motion.reduced;
            let title = window
                .app
                .active
                .and_then(|i| crate::canvas::tab_label(&window.app, i))
                .map_or_else(|| "PdfCraft".to_owned(), |name| format!("{name} — PdfCraft"));
            let mut builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([1100.0, 760.0]).with_min_inner_size([640.0, 420.0]);
            if let Some(position) = window.position.take() {
                builder = builder.with_position(visible_position(position, ctx.pixels_per_point()));
            }
            ctx.show_viewport_immediate(window.id, builder, |ui, _class| {
                let child_ctx = ui.ctx().clone();
                window.app.logic(&child_ctx, frame);
                ui.push_id(window.id, |ui| window.app.ui(ui, frame));
            });
            self.root.copy_shared_settings_from(&window.app);
        }
        self.collect(&ctx);
        if self.final_session.is_none() && (self.root_hidden || self.root.window_close_ready) && self.windows.iter().all(|w| w.app.window_close_ready)
        {
            self.final_session = Some(self.combined_session());
        }
        self.windows.retain(|w| !w.app.window_close_ready);
        let mut live: Vec<_> = self.windows.iter().map(|w| w.id).collect();
        if !self.root_hidden && !self.root.window_close_ready {
            live.push(egui::ViewportId::ROOT);
        }
        crate::window_drop::retain(&ctx, &live);
        if self.windows.is_empty() && (self.root.window_close_ready || self.root_hidden) {
            // eframe handles Close on the next pass. Do not cancel that final root close:
            // every document has already passed its own Save/Discard/Cancel guard.
            self.root.managed_windows = false;
            self.root.allow_quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.root.window_close_ready && !self.root_hidden {
            for i in (0..self.root.views.len()).rev() {
                self.root.close_tab(i);
            }
            self.root_hidden = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    /// Construct a synthetic managed host with three documents and no installed-app state.
    fn app() -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.managed_windows = true;
        app.set_option("language", "en").unwrap();
        let bytes = app.session.create_blank(300.0, 400.0, 3).unwrap();
        for name in ["first.pdf", "middle.pdf", "last.pdf"] {
            app.open_bytes(name, None, bytes.as_ref().clone()).unwrap();
        }
        app
    }

    /// Changes and deletions in a child must survive its closure and the next settings save.
    #[test]
    fn window_settings_changes_and_deletions_survive_child_close() {
        use eframe::App as _;
        #[derive(Default)]
        struct Storage(std::collections::HashMap<String, String>);
        impl eframe::Storage for Storage {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.into(), value);
            }
            fn remove_string(&mut self, key: &str) {
                self.0.remove(key);
            }
            fn flush(&mut self) {}
        }
        let mut host = NativeWindows::new(app());
        let mut child = app();
        child.copy_shared_settings_from(&host.root);
        child.set_option("language", "fr").unwrap();
        child.set_option("theme", "dark").unwrap();
        child.set_option("date-format", "yyyy-mm-dd").unwrap();
        child.session.set_date_language(Some("fr")).unwrap();
        child.ui_scale = Some(1.25);
        child.recent_limit = 7;
        child.hotbar.tools = vec!["edit.edit_text".into()];
        child.hotbar.button_size = 48.0;
        child.restore_last_view = true;
        child.invert_middle_drag = true;
        child.fill_text_color = [0.2, 0.4, 0.6];
        child.pinned_tools = vec!["edit.edit_text".into()];
        child.show_page_edge_shadows = false;
        child.show_secured_notice = false;

        child.always_default_view = true;
        child.organize_zoom = 1.25;
        child.print_draft.paper = 1;
        child.comment_prefs.author = "Synthetic author".into();
        child.signature = Some(crate::fill_sign::SavedSig::Typed("Test".into()));
        child.initials = Some(crate::fill_sign::SavedSig::Typed("T".into()));
        child.recent.push(crate::RecentFile { name: "test.pdf".into(), path: "test.pdf".into(), pages: 1, size: 123 });
        child.session.set_javascript(false);
        host.root.copy_shared_settings_from(&child);
        let mut next = app();
        next.copy_shared_settings_from(&host.root);
        assert_eq!(next.language, "fr");
        assert_eq!(next.recent_limit, 7);
        assert_eq!(next.hotbar.tools, ["edit.edit_text"]);
        assert_eq!(next.hotbar.button_size, 48.0);
        assert!(next.restore_last_view);
        assert!(next.invert_middle_drag);
        assert_eq!(next.fill_text_color, [0.2, 0.4, 0.6]);
        assert_eq!(next.pinned_tools, ["edit.edit_text"]);
        assert!(!next.show_page_edge_shadows);
        assert!(!next.show_secured_notice);

        assert_eq!(next.ui_scale, Some(1.25));
        assert!(next.always_default_view);
        assert_eq!(next.organize_zoom, 1.25);
        assert_eq!(next.print_draft.paper, 1);
        assert_eq!(next.session.date_format(), "yyyy-mm-dd");
        assert_eq!(next.session.date_language(), Some("fr"));
        assert_eq!(next.comment_prefs.author, "Synthetic author");
        assert_eq!(next.signature, child.signature);
        assert!(!next.session.javascript());
        next.signature = None;
        next.initials = None;
        next.recent.clear();
        host.root.copy_shared_settings_from(&next);
        drop(child);
        let mut storage = Storage::default();
        host.save(&mut storage);
        let saved: serde_json::Value = serde_json::from_str(&eframe::Storage::get_string(&storage, "pdfcraft").unwrap()).unwrap();
        assert_eq!(saved["language"], "fr");
        assert_eq!(saved["always_default_view"], true);
        assert_eq!(saved["organize_zoom"], 1.25);
        assert_eq!(saved["print_paper"], pdfcraft_engine::print::PAPERS[1].0);
        assert!(saved["signature_text"].is_null());
        assert!(saved["initials"].is_null());
        assert_eq!(saved["recent"], serde_json::json!([]));
    }

    /// Hiding the root must not omit detached documents from session restoration.
    #[test]
    fn window_last_session_includes_detached_documents_when_root_is_hidden() {
        let mut host = NativeWindows::new(app());
        host.root.reopen_last_session = true;
        let mut child = app();
        let id = child.views[0].id;
        let bytes = child.session.get(id).unwrap().bytes.clone();
        child.session.mark_saved(id, bytes, Some("detached.pdf".into())).unwrap();
        child.views[0].current = 2;
        host.windows.push(Window { id: egui::ViewportId::from_hash_of("child"), app: child, position: None });
        host.root_hidden = true;
        let saved = host.combined_session();
        assert_eq!(saved.files.len(), 1);
        assert!(saved.files[0].path.ends_with("detached.pdf"));
        assert_eq!(saved.files[0].page, 2);
    }

    /// Moving a tab retains its reading position without replacing an already queued navigation.
    #[test]
    fn window_transfers_restore_current_page_without_overwriting_pending_destination() {
        let mut root = app();
        root.views[0].current = 2;
        root.views[0].goto = None;
        let id = root.views[0].id;
        assert!(root.request_detach_tab(id, None));
        let (mut child, _) = root.take_detached_tab().unwrap();
        assert_eq!(child.views[0].goto, Some((2, 0.0)));
        child.views[0].goto = Some((1, 42.0));
        assert!(child.transfer_tab_to(id, &mut root, 0));
        assert_eq!(root.views[0].goto, Some((1, 42.0)));
    }

    /// Detaching transfers unsaved bytes, history, view position and recovery ownership together.
    #[test]
    fn tear_off_preserves_working_bytes_undo_redo_view_and_recovery_identity() {
        let mut root = app();
        root.active = Some(1);
        root.options_source = Some(std::sync::Arc::new(|_| Vec::new()));
        let id = root.views[1].id;
        root.apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "First edit".into() });
        let first = root.session.get(id).unwrap().bytes.clone();
        root.apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Second edit".into() });
        let second = root.session.get(id).unwrap().bytes.clone();
        root.session.undo(id).unwrap();
        let before_transfer = root.session.get(id).unwrap().bytes.clone();
        assert_eq!(before_transfer, first);
        root.views[1].zoom = 1.75;
        root.views[1].current = 2;
        root.recovery_keys.insert(id, "existing-recovery".into());
        let active = root.views[2].id;
        root.active = Some(2);
        assert!(root.request_detach_tab(id, None));
        root.move_tab(1, 0); // A queued request follows identity, not its old index.
        let (mut child, _) = root.take_detached_tab().unwrap();
        assert!(std::sync::Arc::ptr_eq(child.options_source.as_ref().unwrap(), root.options_source.as_ref().unwrap()));
        assert!(root.session.get(id).is_none());
        assert_eq!(root.active_ids().unwrap().1, active);
        assert_eq!(child.active_ids().unwrap().1, id);
        assert_eq!(child.views[0].zoom, 1.75);
        assert_eq!(child.views[0].current, 2);
        assert_eq!(child.recovery_keys.get(&id).unwrap(), "existing-recovery");
        assert!(!root.recovery_keys.contains_key(&id));
        assert!(root.closed_tabs.is_empty());
        assert!(std::sync::Arc::ptr_eq(&child.session.get(id).unwrap().bytes, &before_transfer));
        assert!(child.session.get(id).unwrap().dirty);
        child.session.redo(id).unwrap();
        assert_eq!(child.session.get(id).unwrap().bytes, second);
        child.session.undo(id).unwrap();
        assert_eq!(child.session.get(id).unwrap().bytes, first);
        assert_eq!(root.views.len(), 2);
    }

    /// Unavailable hosts and blocked operations leave the source document owned and intact.
    #[test]
    fn tear_off_rejects_unavailable_host_missing_document_and_modal_without_removing_work() {
        let mut root = app();
        let id = root.views[0].id;
        root.managed_windows = false;
        assert!(!root.request_detach_tab(id, None));
        root.managed_windows = true;
        assert!(!root.request_detach_tab(DocId(u64::MAX), None));
        root.close_request = Some(crate::CloseRequest::Tab(id));
        assert!(!root.request_detach_tab(id, None));
        assert_eq!(root.views.len(), 3);
        root.close_request = None;
        assert!(root.request_detach_tab(id, None));
        root.close_request = Some(crate::CloseRequest::Tab(id));
        assert!(root.take_detached_tab().is_none());
        assert_eq!(root.session.docs().len(), 3);
    }

    /// A detached window applies its own save guard without closing its neighbours.
    #[test]
    fn tear_off_close_uses_save_discard_cancel_and_does_not_close_other_windows() {
        let mut root = app();
        let id = root.views[2].id;
        root.apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Keep me".into() });
        assert!(root.execute("file.detach_tab"));
        let (mut child, _) = root.take_detached_tab().unwrap();
        let ctx = egui::Context::default();
        assert!(!child.request_quit(&ctx));
        assert_eq!(child.close_request, Some(crate::CloseRequest::Quit));
        child.resolve_close(&ctx, None);
        assert!(!child.window_close_ready);
        assert!(child.session.get(id).unwrap().dirty);
        assert!(!child.request_quit(&ctx));
        child.resolve_close(&ctx, Some(false));
        assert!(child.window_close_ready);
        assert_eq!(root.session.docs().len(), 2);
        assert!(!root.window_close_ready);
    }

    /// The final managed window permits process exit only after its document close guard succeeds.
    #[test]
    fn tear_off_final_window_allows_the_native_exit_after_its_save_guard() {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| NativeWindows::new(app()));
        h.run_steps(2);
        assert!(h.state_mut().root.request_quit(&egui::Context::default()));
        h.run_steps(2);
        assert!(!h.state().root.managed_windows, "the final Close must not be canceled on the next native pass");
        assert!(h.state().root.allow_quit);
        assert!(h.state().windows.is_empty());
    }

    /// Drive a synthetic pointer drag with an optional Escape cancellation before release.
    fn drag(h: &mut Harness<'static, PdfCraftApp>, to: egui::Pos2, escape: bool) {
        let from = h.get_by_role_and_label(egui::accesskit::Role::Tab, "first.pdf").rect().center();
        h.event(egui::Event::PointerMoved(from));
        h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
        for step in 1..=5 {
            h.event(egui::Event::PointerMoved(from.lerp(to, step as f32 / 5.0)));
            h.run_steps(1);
        }
        if escape {
            h.key_press(egui::Key::Escape);
            h.run_steps(1);
        }
        h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.run_steps(2);
    }

    /// Only a completed drag beyond the strip queues tear-off; Escape leaves the tab in place.
    #[test]
    fn tear_off_raw_drag_queues_only_outside_strip_and_escape_cancels() {
        for escape in [false, true] {
            let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app());
            h.run_steps(4);
            let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
            drag(&mut h, egui::pos2(400.0, 180.0), escape);
            assert_eq!(h.state().detach_request.map(|r| r.0), if escape { None } else { Some(ids[0]) });
            assert_eq!(h.state().views.iter().map(|v| v.id).collect::<Vec<_>>(), ids);
        }
    }

    /// Horizontal dragging retains reorder and edge scrolling instead of detaching the tab.
    #[test]
    fn tear_off_keeps_horizontal_reordering_and_strip_edge_scrolling() {
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app());
        h.run_steps(4);
        let first = h.state().views[0].id;
        let end = h.get_by_role_and_label(egui::accesskit::Role::Tab, "last.pdf").rect().right_center() + egui::vec2(12.0, 0.0);
        drag(&mut h, end, false);
        assert_eq!(h.state().views[2].id, first);
        assert!(h.state().detach_request.is_none());
        let strip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 38.0));
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0));
        assert!(!outside_strip(strip, viewport, egui::pos2(1399.0, 20.0)));
        assert!(outside_strip(strip, viewport, egui::pos2(1430.0, 20.0)));
    }
    /// A transferred document keeps the protection against overwriting externally changed files.
    #[test]
    fn tear_off_preserves_external_file_change_protection() {
        let mut root = app();
        let id = root.views[0].id;
        let path = std::env::temp_dir().join(format!("pdfcraft-tear-off-watch-{}.pdf", std::process::id()));
        std::fs::write(&path, b"old").unwrap();
        let path_text = path.to_str().unwrap();
        root.watch.known_now(id, path_text);
        std::fs::write(&path, b"external rewrite").unwrap();
        assert!(root.request_detach_tab(id, None));
        let (child, _) = root.take_detached_tab().unwrap();
        assert!(child.watch.disk_differs(id, path_text, child.session.get(id).map(|d| d.saved_bytes())));
        std::fs::remove_file(path).unwrap();
    }

    /// Documents created in different windows never share recovery identifiers.
    #[test]
    fn tear_off_new_documents_have_distinct_autosaves_across_windows() {
        let mut root = app();
        let dir = std::env::temp_dir().join(format!("pdfcraft-tear-off-recovery-{}", std::process::id()));
        root.enable_recovery(crate::RecoveryStore::new(&dir));
        let id = root.views[0].id;
        assert!(root.request_detach_tab(id, None));
        let (mut child, _) = root.take_detached_tab().unwrap();
        let bytes = root.session.create_blank(100.0, 100.0, 1).unwrap();
        for app in [&mut root, &mut child] {
            app.open_bytes("new.pdf", None, bytes.as_ref().clone()).unwrap();
            assert!(app.apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "unsaved".into() }));
            app.autosave_now();
        }
        let a = root.active_ids().unwrap().1;
        let b = child.active_ids().unwrap().1;
        assert_ne!(a, b, "all windows share one document identity namespace");
        assert_ne!(root.recovery_keys.get(&a), child.recovery_keys.get(&b));
        let child_path = dir.join(format!("{}.pdf", child.recovery_keys.get(&b).unwrap()));
        for i in (0..root.views.len()).rev() {
            root.close_tab(i);
        }
        root.shutdown_recovery();
        assert!(child_path.is_file(), "closing the source must not remove the detached autosave");
        for i in (0..child.views.len()).rev() {
            child.close_tab(i);
        }
        drop(child);
        drop(root);
        std::fs::remove_dir(&dir).unwrap();
    }

    /// Moving a tab into another window preserves state and inserts it at the chosen tab gap.
    #[test]
    fn window_drop_tab_move_keeps_identity_view_recovery_and_target_order() {
        let mut source = app();
        let mut target = app();
        source.active = Some(1);
        let id = source.views[1].id;
        source.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        source.views[1].zoom = 2.25;
        source.recovery_keys.insert(id, "moving-recovery".into());
        let original: Vec<_> = target.views.iter().map(|v| v.id).collect();
        let bytes = source.session.get(id).unwrap().bytes.clone();
        assert!(source.transfer_tab_to(id, &mut target, 1));
        let moved = target.views[1].id;
        assert_eq!(moved, id);
        assert!(!original.contains(&moved));
        assert_eq!(target.views[0].id, original[0]);
        assert_eq!(target.views[2].id, original[1]);
        assert_eq!(target.active, Some(1));
        assert_eq!(target.views[1].zoom, 2.25);
        assert_eq!(target.recovery_keys.get(&moved).map(String::as_str), Some("moving-recovery"));
        assert!(std::sync::Arc::ptr_eq(&bytes, &target.session.get(moved).unwrap().bytes));
        assert!(!source.recovery_keys.contains_key(&id));
        target.session.undo(moved).unwrap();
        assert_eq!(target.session.get(moved).unwrap().info.pages[0].rotation, 0);
        target.session.redo(moved).unwrap();
        assert!(target.transfer_tab_to(moved, &mut source, 0));
        assert_eq!(source.views[0].zoom, 2.25);
        assert_eq!(source.session.get(source.views[0].id).unwrap().bytes, bytes);
    }

    /// Organize receives the current edited pages as one undoable copy, leaving the source open.
    #[test]
    fn window_drop_organize_copies_current_pages_at_gap_and_undo_keeps_source() {
        let mut source = app();
        let mut target = app();
        let id = source.active_ids().unwrap().1;
        let into = target.active_ids().unwrap().1;
        source.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![1], degrees: 90 });
        let source_bytes = source.session.get(id).unwrap().bytes.clone();
        let target_bytes = target.session.get(into).unwrap().bytes.clone();
        target.views[2].open_organize();
        assert!(source.insert_tab_pages_into(id, &mut target, into, 1));
        assert_eq!(target.session.get(into).unwrap().info.pages.len(), 6);
        assert_eq!(target.session.get(into).unwrap().info.pages[2].rotation, 90);
        assert_eq!(target.views[2].target_pages(), vec![1, 2, 3]);
        assert_eq!(source.views.len(), 3);
        assert!(std::sync::Arc::ptr_eq(&source.session.get(id).unwrap().bytes, &source_bytes));
        target.session.undo(into).unwrap();
        assert_eq!(target.session.get(into).unwrap().bytes, target_bytes);
        target.session.redo(into).unwrap();
        assert_eq!(target.session.get(into).unwrap().info.pages.len(), 6);
        assert_eq!(source.session.get(id).unwrap().bytes, source_bytes);
    }

    /// Rejected destinations must not mutate either participating document.
    #[test]
    fn window_drop_rejection_keeps_both_documents_unchanged() {
        let mut source = app();
        let mut target = app();
        let id = source.active_ids().unwrap().1;
        let into = target.active_ids().unwrap().1;
        let before = target.session.get(into).unwrap().bytes.clone();
        assert!(!source.transfer_tab_to(id, &mut target, 99));
        assert!(!source.insert_tab_pages_into(id, &mut target, into, 99));
        target.close_request = Some(crate::CloseRequest::Quit);
        assert!(!source.transfer_tab_to(id, &mut target, 0));
        assert!(!source.insert_tab_pages_into(id, &mut target, into, 0));
        assert_eq!(source.views.len(), 3);
        assert_eq!(target.views.len(), 3);
        assert_eq!(target.session.get(into).unwrap().bytes, before);
        target.close_request = None;
        target.active = Some(0);
        assert!(!source.insert_tab_pages_into(id, &mut target, into, 0));
    }

    /// Transfers address the correct source and destination across root and child viewports.
    #[test]
    fn window_drop_child_to_child_and_back_to_root_use_correct_sessions() {
        let mut host = NativeWindows::new(app());
        let a = egui::ViewportId::from_hash_of("a");
        let b = egui::ViewportId::from_hash_of("b");
        host.windows.push(Window { id: a, app: app(), position: None });
        host.windows.push(Window { id: b, app: app(), position: None });
        let (source, target) = host.pair_mut(a, b).unwrap();
        let id = source.views[0].id;
        assert!(source.transfer_tab_to(id, target, 0));
        let moved = target.views[0].id;
        let (source, target) = host.pair_mut(b, egui::ViewportId::ROOT).unwrap();
        assert!(source.transfer_tab_to(moved, target, 3));
        assert_eq!(host.root.views.len(), 4);
        assert_eq!(host.windows[0].app.views.len(), 2);
        assert_eq!(host.windows[1].app.views.len(), 3);
        assert!(host.pair_mut(a, a).is_none());
    }

    /// Page copying refuses restricted sources and accepts a source with sufficient permissions.
    #[test]
    fn window_drop_organize_honors_encryption_permissions_and_accepts_unlocked_source() {
        for permitted in [false, true] {
            let mut source = app();
            let mut target = app();
            let raw = source.session.create_blank(200.0, 300.0, 2).unwrap();
            let mut pdf = pdfcraft_cos::Document::open(raw).unwrap();
            pdf.set_encryption(&pdfcraft_cos::NewEncryption {
                algorithm: pdfcraft_cos::Algorithm::Aes256,
                user_password: "",
                owner_password: "owner",
                permissions: if permitted { -1 } else { 4 },
                encrypt_metadata: true,
                seed: [3; 32],
            })
            .unwrap();
            let encrypted = pdfcraft_cos::write_full(&pdf, &Default::default()).unwrap();
            source.open_bytes("protected.pdf", None, encrypted).unwrap();
            let id = source.active_ids().unwrap().1;
            let into = target.active_ids().unwrap().1;
            let before = target.session.get(into).unwrap().bytes.clone();
            assert_eq!(source.insert_tab_pages_into(id, &mut target, into, 0), permitted);
            assert!(source.session.get(id).unwrap().info.encrypted);
            if permitted {
                assert_eq!(target.session.get(into).unwrap().info.pages.len(), 5);
            } else {
                assert_eq!(target.session.get(into).unwrap().bytes, before);
            }
        }
    }

    /// Transferred file watches continue detecting external modifications under the retained identity.
    #[test]
    fn window_drop_remapped_file_watch_still_detects_external_changes() {
        let mut source = app();
        let mut target = app();
        let id = source.views[0].id;
        let path = std::env::temp_dir().join(format!("pdfcraft-window-drop-watch-{}.pdf", std::process::id()));
        std::fs::write(&path, b"before").unwrap();
        source.watch.known_now(id, path.to_str().unwrap());
        std::fs::write(&path, b"external rewrite").unwrap();
        assert!(source.transfer_tab_to(id, &mut target, 0));
        assert!(target.watch.disk_differs(
            target.views[0].id,
            path.to_str().unwrap(),
            target.session.get(target.views[0].id).map(|d| d.saved_bytes())
        ));
        std::fs::remove_file(&path).unwrap();
    }
}
