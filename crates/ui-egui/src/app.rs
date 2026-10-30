use super::hooks::OsEvent;
use super::state::{
    Dialog, DocumentCopyJob, GridFile, LeftPanel, LinkOrigin, Mode, NumberDraft, PasswordPrompt, PendingLink, PropsTab, QuickTool, RecentFile,
    RightPanel, UnlockPrompt,
};
#[cfg(not(target_arch = "wasm32"))]
use super::state::{OpenOutcome, OpenRequest, PendingRemoteLink};
use super::*;

mod clipboard;
mod commands;
mod control;
mod files;
mod frame;
mod input;
mod keys;
mod links;
mod prefs;
mod window;

pub(crate) use clipboard::{copy_refused, print_refused};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use files::same_file;
use frame::migrate_panel_state;
pub(crate) use prefs::{MAX_AUTHOR_CHARS, date_text_for_pdf};

impl Default for PdfCraftApp {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfCraftApp {
    /// The window's title and, on the web, the page's, from the active document (#3640). Done in
    /// `logic` too, which also runs while the window or browser tab is hidden: a document opened
    /// then (a `?file=` download finishing) still names the page an embedding host reads.
    fn sync_titles(&mut self, ctx: &egui::Context) {
        // The window shows the active document's name (or title, if it asks for that), with •
        // while it has unsaved changes, as the other Craft apps write it (#3640). Unsaved means
        // what closing would ask about, so a draft not yet applied counts too.
        let dirty = self.active.is_some_and(|i| self.has_unsaved_work(i));
        let mark = if dirty { " •" } else { "" };
        let titled = |name: Option<String>| {
            name.or_else(|| self.combine_showing().then(|| tl!("Combine files").to_owned()))
                .map_or_else(|| "PdfCraft".to_owned(), |name| format!("{name}{mark} — PdfCraft"))
        };
        let title = titled(self.active.and_then(|i| crate::canvas::tab_label(self, i)));
        // The web page's title is what an embedding host reads to tell which file opened, so it
        // names the file even when the document asks to show its own title.
        let page_title = titled(self.active.and_then(|i| self.views.get(i)).and_then(|v| self.session.get(v.id)).map(|d| d.name.clone()));
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        if page_title != self.page_title {
            // eframe's web runner ignores Title: set the page's own.
            #[cfg(target_arch = "wasm32")]
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                document.set_title(&page_title);
            }
            self.page_title = page_title;
        }
    }

    /// Construct an application with the default session, panels, theme and control state.
    pub fn new() -> Self {
        Self {
            session: Session::new(),
            views: Vec::new(),
            closed_tabs: Vec::new(),
            active: None,
            mode: Mode::AllTools,
            default_mode: Mode::AllTools,
            view_defaults: Default::default(),
            units: Default::default(),
            date_format_draft: None,
            mode_override: None,
            left: LeftPanel::AllTools,
            side_panels: Default::default(),
            left_open: true,
            right: None,
            last_right: RightPanel::Comments,
            right_before_read: None,
            comments_panel_closed: false,
            right_panel_closed: false,
            quick_tool: QuickTool::Select,
            space_hand: None,
            full_screen_tool: None,
            comment_prefs: Default::default(),
            theme: ThemeKind::Light,
            // A fresh install follows the desktop's appearance (#1248); saved settings override.
            theme_preference: ThemePreference::System,
            reading_colors: Default::default(),
            #[cfg(target_os = "linux")]
            desktop_theme: None,
            #[cfg(target_os = "linux")]
            desktop_theme_enabled: false,
            scroll_speed: scroll_speed::ScrollSpeed::default(),
            animation_effects: motion::AnimationEffects::default(),
            motion: motion::Motion::default(),
            language: i18n::AUTO.to_string(),
            ui_scale: None,
            system_text_scale: 1.0,
            ui_scale_applied: None,
            flatten_fill_sign_on_save: false,
            fill_text_size: fill_sign::TEXT_SIZE,
            fill_text_color: [0.0; 3],
            dialog: None,
            updates: updates::Updates::default(),
            printer_source: None,
            options_source: None,
            print_tasks: print_ui::PrintTasks::default(),
            palette_open: false,
            palette_query: String::new(),
            all_tools_expanded: false,
            all_tools_full: false,
            all_tools_query: String::new(),
            all_tools_tab: (None, false),
            recent: Vec::new(),
            reopen_last_session: false,
            restore_last_view: true,
            last_views: Vec::new(),
            last_session: Default::default(),
            quit_session: None,
            reload_changed_files: true,
            show_secured_notice: true,
            always_default_view: false,
            #[cfg(not(target_arch = "wasm32"))]
            watch: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            open_worker: None,
            #[cfg(not(target_arch = "wasm32"))]
            pending_opens: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            next_open_token: 0,
            #[cfg(all(test, not(target_arch = "wasm32")))]
            open_prepare_gate: None,
            pinned: Default::default(),
            toast: None,
            toast_script: None,
            script_messages: std::collections::VecDeque::new(),
            script_messages_dropped: 0,
            toast_dropped: 0,
            integrated_titlebar: false,
            window_controls: None,
            system_title_bar: false,
            app_frame_shown: None,
            password_prompt: None,
            document_copy: None,
            unlock_prompt: None,
            full_screen: false,
            inbox: Default::default(),
            startup_inbox: Default::default(),
            startup_superseded: false,
            failed_inbox: Default::default(),
            unsaved_flag: Default::default(),
            os_events: None,
            host_save: None,
            host_dirty: None,
            close_request: None,
            save_prompt_button: Default::default(),
            save_prompt_seen: false,
            save_prompt_attention: false,
            #[cfg(not(target_arch = "wasm32"))]
            pending_overwrite: None,
            save_override: None,
            props_draft: None,
            view_draft: None,
            custom_draft: None,
            requests: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pickers: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pick_override: None,
            export_dir_override: None,
            split_draft: SplitDraft::default(),
            extract_draft: ExtractDraft::default(),
            rotate_draft: RotateDraft::default(),
            summary_sort: Default::default(),
            sign_draft: None,
            digital_ids: Vec::new(),
            sig_expanded: Vec::new(),
            a11y_options: a11y_ui::A11yOptions::default(),
            ocr_draft: ocr_ui::OcrDraft::default(),
            ocr_run: None,
            ocr_batch: None,
            run_inline: false,
            custom_actions: Vec::new(),
            wizard: Default::default(),
            action_run: None,
            action_files_override: None,
            pdfa: Default::default(),
            compare_old: None,
            compare: None,
            js_console: Default::default(),
            doc_js: Default::default(),
            a11y: a11y_ui::A11yState::default(),
            tags_state: Default::default(),
            a11y_skipped: Default::default(),
            alt_draft: Default::default(),
            os_key_store_ids: false,
            cert_viewer: None,
            space_audit: Vec::new(),
            space_audit_of: None,
            combine_draft: Vec::new(),
            combine_conflicts: Default::default(),
            combine_tab: Default::default(),
            combine_columns: Default::default(),
            combine_view: Default::default(),
            combine_zoom: 1.0,
            organize_zoom: 1.0,
            combine_thumbs: Default::default(),
            image_import: None,
            custom_stamps: Vec::new(),
            stamp_draft: Default::default(),
            optimize_draft: OptimizeDraft::default(),
            optimize_run: None,
            progress_notice: None,
            page_clipboard: None,
            grid_drop: None,
            last_snapshot: None,
            last_copied_image: None,
            last_copied_file: None,
            system_clipboard: true,
            clipboard_override: None,
            v_pressed: false,
            attach_override: None,
            duplicate_draft: None,
            page_props_draft: None,
            field_table_draft: None,
            form_preview: false,
            window_title: String::new(),
            page_title: String::new(),
            recovery: None,
            recoverable: Vec::new(),
            recovery_keys: Default::default(),
            last_autosave: None,
            allow_quit: false,
            deferred_commands: Vec::new(),
            native_menu: None,
            menu_input: Vec::new(),
            dialog_seen: None,
            dialog_epoch: 0,
            ctx: None,
            #[cfg(not(target_arch = "wasm32"))]
            window_state: Default::default(),
            styled: false,
            fonts_ready: false,
            fonts_cjk: theme::CjkPreference::Default,
            control: None,
            bookmark_rename: None,
            last_opened_url: None,
            community_card_closed: false,
            pending_link: None,
            pending_full_screen: None,
            pending_remote_link: None,
            protect_draft: Default::default(),
            text_style_draft: Default::default(),
            boxes_draft: Default::default(),
            marks_draft: Default::default(),
            unmarked_page: Default::default(),
            export_draft: Default::default(),
            export_status: None,
            signature: None,
            initials: None,
            signature_draft: Default::default(),
            signature_preview: None,
            signature_photo: None,
            saved_signature_previews: [None, None],
            sign_graphic: sign_ui::SignGraphic::Name,
            sign_graphic_preview: None,
            #[cfg(target_arch = "wasm32")]
            signature_images: Default::default(),
            comment_props: None,
            field_props: None,
            bulk_field_props: None,
            redact_prefs: RedactPrefs::default(),
            redact_defaults: RedactPrefs::default(),
            redact_before: None,
            redact_pages_draft: RedactPagesDraft::default(),
            redact_search: RedactSearchDraft::default(),
            hidden_draft: HiddenDraft::default(),
            print_draft: PrintDraft::default(),
            link_draft: None,
            text_style: content_ui::default_style(),
            replace_draft: None,
            number_draft: NumberDraft { from: 1, to: 1, style: pdfcraft_engine::LabelStyle::Decimal, prefix: String::new(), start: 1 },
            word_count_draft: word_count_ui::Draft::default(),
        }
    }

    /// Fields are being edited (Prepare a form or its Calculation order is open, or a field tool
    /// is picked), unless the form is being previewed.
    pub fn is_preparing(&self) -> bool {
        ((self.left_open && matches!(self.left, LeftPanel::Tool("form") | LeftPanel::CalculationOrder))
            || matches!(self.quick_tool, QuickTool::Field(_)))
            && !self.form_preview
    }

    /// A number that changes whenever the open dialog changes (opened, closed or replaced by
    /// another), checked every frame. A picker started from a dialog's Browse… button answers
    /// only into the same showing of that dialog.
    pub(crate) fn dialog_epoch(&mut self) -> u64 {
        if self.dialog != self.dialog_seen {
            self.dialog_seen = self.dialog;
            self.dialog_epoch = self.dialog_epoch.wrapping_add(1);
        }
        self.dialog_epoch
    }

    pub fn active_ids(&self) -> Option<(usize, DocId)> {
        self.active.and_then(|i| self.views.get(i).map(|v| (i, v.id)))
    }

    /// A dialog or prompt is open over the window: the keyboard is its own, and nothing may act
    /// on the document underneath it.
    pub(crate) fn modal_open(&self) -> bool {
        self.dialog.is_some()
            || self.close_request.is_some()
            || self.password_prompt.is_some()
            || self.unlock_prompt.is_some()
            || self.pending_link.is_some()
            || self.pending_remote_link.is_some()
            || self.updates.open
    }

    /// Show `msg` in the toast now, in place of whatever it showed. Script messages still waiting
    /// show after it.
    pub fn notify(&mut self, msg: impl Into<String>) {
        // A left-out count it replaces is said again once the toast is free.
        self.script_messages_dropped = self.script_messages_dropped.saturating_add(std::mem::take(&mut self.toast_dropped));
        self.toast = Some((msg.into(), 0.0));
        self.toast_script = None;
    }

    /// Notify with a static message in the UI language.
    pub fn notify_tr(&mut self, text: &str) {
        self.notify(tl!(text).to_string());
    }

    /// Notify with an error's own text. Engine and OS messages are not translated: matching their
    /// English wording would break silently whenever it changes.
    pub fn notify_error(&mut self, error: impl std::fmt::Display) {
        self.notify(error.to_string());
    }

    /// Notify with a `{name}`-style template in the UI language (placeholders filled once).
    pub fn notify_fmt(&mut self, template: &str, args: &[(&str, &str)]) {
        self.notify(i18n::fmt(tl!(template), args));
    }

    /// `true` while any open document still waits for page renders (used by headless capture).
    pub fn render_pending(&self) -> bool {
        self.views.iter().any(|v| v.render_pending())
    }
}

impl eframe::App for PdfCraftApp {
    /// While files are dragged over the window the system sends no pointer moves, so egui
    /// would keep the place the pointer entered at: tell it where the pointer really is, so the
    /// page grid can show (and use) the gap under it.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // The button's release a window move or resize kept from egui.
        window_controls::release_after_desktop_grab(ctx, raw);
        self.prepare_raw_input(ctx, raw);
        if raw.hovered_files.is_empty() && raw.dropped_files.is_empty() {
            return;
        }
        if let Some(pos) = drag_pointer::in_window(ctx) {
            raw.events.push(egui::Event::PointerMoved(pos));
        }
        ctx.request_repaint();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // eframe has just stored the window as it is now: not while it is minimized.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let minimized = self.ctx.as_ref().is_some_and(|c| c.input(|i| i.viewport().minimized == Some(true)));
            self.window_state.keep(storage, minimized);
        }
        storage.set_string("pdfcraft", self.persist());
    }

    /// Each frame's work before drawing: opens, operating-system and control-channel requests,
    /// the quit guard, shortcuts, and the background jobs' results.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ctx = Some(ctx.clone());
        #[cfg(not(target_arch = "wasm32"))]
        self.window_state.repair(ctx);
        self.dialog_epoch();
        // Notices raised outside `ui` (opened files, OS events, the control channel) translate too.
        let lang = i18n::Lang::from_pref(&self.language);
        i18n::set_current(lang);
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(worker) = &self.open_worker {
                worker.set_context(ctx);
            }
            self.poll_pending_opens();
        }
        // Chinese wants its own faces before the Japanese ones (one baseline per line).
        let cjk = theme::CjkPreference::for_language(lang.code());
        if !self.styled {
            egui_extras::install_image_loaders(ctx);
            theme::install_fonts_for(ctx, cjk);
            self.fonts_cjk = cjk;
            theme::apply(ctx, self.theme);
            self.styled = true;
        } else {
            self.fonts_ready = true;
            if cjk != self.fonts_cjk {
                // Same family names as before, so named fonts stay valid while the new set loads.
                theme::install_fonts_for(ctx, cjk);
                self.fonts_cjk = cjk;
            }
        }
        self.sync_theme(ctx);
        let native = ctx.native_pixels_per_point().unwrap_or(1.0);
        ui_scale::sync(ctx, ui_scale::zoom(self.ui_scale, self.system_text_scale, native), &mut self.ui_scale_applied);
        // Remember user work even if its tab is subsequently closed or Home is selected.
        self.startup_superseded |= !self.views.is_empty() || self.combine_showing() || self.dialog.is_some() || self.password_prompt.is_some();
        // Also done in `raw_input_hook`, before input; here for hosts that don't call the hook.
        self.scroll_speed.set_line_speed(ctx);
        // Motion reads this frame's wheel input, so it begins in `ui`, in a known UI phase:
        // `logic` also runs alone while hidden, where the input events and time are stale.
        // Before taking this frame's drop: the grid must be drawn once with the pointer where
        // the files were let go before the gap is read.
        self.finish_grid_drop(ctx);
        // Showing a document hides the Combine files tab.
        if self.active.is_some() {
            self.combine_tab.focused = false;
        }
        #[cfg(target_arch = "wasm32")]
        self.process_signature_images();
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        #[cfg(not(target_arch = "wasm32"))]
        let dropped = self.drop_on_signature_pad(dropped);
        #[cfg(not(target_arch = "wasm32"))]
        let dropped = self.drop_on_grid(dropped, ctx);
        for f in dropped {
            // Files dropped on the Combine files tab join its list instead of opening.
            if self.combine_showing() {
                self.drop_into_combine(f, ctx);
            } else {
                self.open_dropped(f, ctx);
            }
        }
        let arrived: Vec<(String, Vec<u8>)> = self.inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (name, bytes) in arrived {
            if let Err(e) = self.open_bytes(&name, None, bytes) {
                self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
        // Don't replace a pending prompt or import. A startup file may need its own password
        // prompt, so its bytes stay queued until the current interaction finishes.
        #[cfg(not(target_arch = "wasm32"))]
        let opening = !self.pending_opens.is_empty();
        #[cfg(target_arch = "wasm32")]
        let opening = false;
        if !opening
            && self.password_prompt.is_none()
            && self.image_import.is_none()
            && self.dialog.is_none()
            && self.close_request.is_none()
            && self.pending_link.is_none()
            && self.pending_remote_link.is_none()
        {
            let startup = self.startup_inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
            for (name, bytes) in startup {
                let activate = !self.startup_superseded;
                if let Err(e) = self.open_bytes_with_focus(&name, None, bytes, activate) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e)]);
                }
            }
        }
        let failed: Vec<(String, String)> = self.failed_inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (name, e) in failed {
            self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e)]);
        }
        let os_events = self.os_events.as_mut().map(|poll| poll()).unwrap_or_default();
        for e in os_events {
            match e {
                #[cfg(not(target_arch = "wasm32"))]
                OsEvent::Open(paths) => {
                    paths.iter().for_each(|p| {
                        self.open_path(p);
                    });
                    // Files handed to the running app (Finder, or another launch): bring its
                    // window to the front, also when minimized or full screen on another Space.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                #[cfg(target_arch = "wasm32")]
                OsEvent::Open(_) => {}
                // Like closing the window: `guard_quit` asks about unsaved changes.
                OsEvent::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        if let Some((_, id)) = self.active_ids() {
            let out = self.session.take_js_output(id);
            self.handle_js(id, out);
        }
        if let Some(mut control) = self.control.take() {
            control.tick(ctx, self);
            self.control = Some(control);
        }
        self.guard_quit(ctx);
        // macOS: quit through `terminate:` instead of dropping the window (Touch Bar crash, #1136).
        #[cfg(target_os = "macos")]
        craft_quit::on_close_request(ctx);
        self.autosave_tick(ctx, web_time::Instant::now());
        // Reload documents whose files a build rewrote (#431).
        #[cfg(not(target_arch = "wasm32"))]
        self.watch_files();
        if self.host_dirty.is_some() {
            let dirty = self.first_dirty().is_some();
            if let Some(report) = self.host_dirty.as_mut() {
                report(dirty);
            }
        }
        self.poll_updates();
        self.poll_print();
        // Shortcuts deferred last frame: the text field has taken that frame's typing since.
        let deferred = std::mem::take(&mut self.deferred_commands);
        self.native_menu_clicks(ctx);
        // The keys a menu click stands for (⌘C, Delete…), before anything reads this frame's keys.
        if !self.menu_input.is_empty() {
            let events = std::mem::take(&mut self.menu_input);
            ctx.input_mut(|i| i.events.extend(events));
        }
        self.shortcuts(ctx);
        // Scrolling and group-drag previews are transient: never resume after changing tabs,
        // opening a modal/palette, or returning to a window that lost focus.
        let blocked = self.modal_open() || self.palette_open || !ctx.input(|i| i.focused);
        for (index, view) in self.views.iter_mut().enumerate() {
            view.objects.set_input_blocked(blocked || self.active != Some(index));
            if blocked || self.active != Some(index) {
                view.auto_scroll.cancel();
            }
        }
        // A field that refused its value this frame keeps the shortcut from saving or printing
        // behind the user's back; so does a save prompt opened since.
        if self.process_pending_edits() && self.close_request.is_none() {
            for (id, doc) in deferred {
                // Only in the document the shortcut was pressed in.
                if self.active_ids().map(|(_, active)| active) == doc {
                    self.execute(id);
                }
            }
        }
        self.poll_export();
        self.auto_ocr_scanned_page();
        self.poll_ocr();
        self.poll_optimize();
        self.poll_action();
        self.process_file_requests();
        #[cfg(not(target_arch = "wasm32"))]
        self.process_picked();
        // Last, after everything that can open or answer the "save changes?" question.
        self.attend_save_prompt(ctx);
        self.sync_native_menu(ctx);
        self.sync_titles(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        i18n::set_current(i18n::Lang::from_pref(&self.language));
        // A language change switches stock chrome between LTR and RTL before anything is drawn.
        theme::sync_direction(&ctx);
        // Fonts registered via set_fonts only take effect next frame; named families would panic now.
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        // Before any widget reads this frame's wheel input or draws.
        self.motion.begin_pass(&ctx, self.animation_effects);
        // One-time 0.36 -> 0.37 panel persistence migration, before any panel stores or reads state.
        migrate_panel_state(ui);
        for view in &mut self.views {
            view.begin_render_frame();
        }
        self.sync_titles(&ctx);
        // The accessible window takes the same name, so a screen reader (AT-SPI on Linux, UIA on
        // Windows) can tell it apart (#4614). egui builds the root node afresh every pass.
        ctx.accesskit_node_builder(egui::accesskit_root_id(), |root| root.set_label(self.window_title.as_str()));
        if self.full_screen && self.active.is_some() {
            // Full screen: the page on black, nothing else, as Acrobat's Full Screen Mode (Esc
            // or ⌘L to leave).
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(egui::Color32::BLACK)).show(ui, |ui| {
                if let Some(i) = self.active {
                    match compare_ui::side_by_side(self, i) {
                        Some((old, new)) => compare_ui::compare_area(self, old, new, ui),
                        None => canvas::document_area(self, i, ui),
                    }
                }
            });
            dialogs::show(self, &ctx);
            // Notices too: a refused field value or a failed save must be seen in full screen.
            self.show_progress(&ctx);
            widgets::toast(self, &ctx);
            self.finish_render_frame(&ctx);
            if let Some(mut control) = self.control.take() {
                control.finish_frame(&ctx, self);
                self.control = Some(control);
            }
            return;
        }
        self.sync_window_frame(&ctx);
        if self.draws_window_frame() {
            window_controls::resize_edges(&ctx);
        }
        chrome::tab_strip(self, ui);
        chrome::mode_bar(self, ui);
        // Whichever way another tab came to the front, it shows instead of the expanded view.
        if self.all_tools_full && self.all_tools_tab != self.shown_tab() {
            self.all_tools_full = false;
        }
        side_panels::show(self, ui);
        let t = theme::Tokens::get(&ctx);
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.pasteboard)).show(ui, |ui| {
            if self.mode == Mode::AllTools && self.all_tools_full {
                all_tools_ui::show(self, ui);
            } else {
                match self.active {
                    None if self.combine_showing() => combine_ui::page(self, ui),
                    None => home::show(self, ui),
                    Some(index) => match compare_ui::side_by_side(self, index) {
                        Some((old, new)) => compare_ui::compare_area(self, old, new, ui),
                        None => canvas::document_area(self, index, ui),
                    },
                }
            }
        });
        self.process_pending_edits();
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        self.show_progress(&ctx);
        widgets::toast(self, &ctx);
        self.finish_render_frame(&ctx);
        // Screenshots are issued only after a settled pass that really drew the window.
        if let Some(mut control) = self.control.take() {
            control.finish_frame(&ctx, self);
            self.control = Some(control);
        }
    }
}
