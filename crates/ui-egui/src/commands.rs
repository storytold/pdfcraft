//! What each registered command does in this frontend (`pdfcraft_engine::commands` says
//! what it is called, where it appears, which key runs it and when it is enabled).
//!
//! Menus, keyboard shortcuts, the palette, the tool panels and automation all reach
//! `PdfCraftApp::execute`: the runtime routes a fired command to [`PdfCraftApp::run_fired`], and
//! one a key fired while it is disabled to [`PdfCraftApp::run_declined`].

pub(crate) mod catalog;
use craft::pages::PageDisplay;
use craft::theme::ModeChoice;
use craft::ui::runtime::commands::Fired;
use pdfcraft_engine::Edit;
use pdfcraft_engine::commands::{self, CommandSpec, Shortcut};

use crate::{Mode, PdfCraftApp, PropsTab, RightPanel, SaveTarget};

/// Chords of the catalog's view commands, for the View menu, tooltips and Help ▸ Keyboard
/// shortcuts. Keep them in step with the catalog's bindings.
pub(crate) const ACTUAL_SIZE: Shortcut = Shortcut::cmd("1");
pub(crate) const PAGE_LEVEL: Shortcut = Shortcut::cmd("0");
pub(crate) const FIT_WIDTH: Shortcut = Shortcut::cmd("2");
pub(crate) const ZOOM_IN: Shortcut = Shortcut::cmd("+");
pub(crate) const ZOOM_OUT: Shortcut = Shortcut::cmd("−");
pub(crate) const ROTATE_CW: Shortcut = Shortcut::cmd_shift("+");
pub(crate) const ROTATE_CCW: Shortcut = Shortcut::cmd_shift("−");
pub(crate) const FIND_NEXT: Shortcut = Shortcut::cmd("G");
pub(crate) const FIND_PREV: Shortcut = Shortcut::cmd_shift("G");
pub(crate) const COPY: Shortcut = Shortcut::cmd("C");
pub(crate) const SELECT_ALL: Shortcut = Shortcut::cmd("A");
pub(crate) const PAGE_PREV: Shortcut = Shortcut::cmd("←");
pub(crate) const PAGE_NEXT: Shortcut = Shortcut::cmd("→");
/// Go to first / last page beside Home and End (`canvas::navigation`).
pub(crate) const FIRST_PAGE: Shortcut = Shortcut::cmd_shift("↑");
pub(crate) const LAST_PAGE: Shortcut = Shortcut::cmd_shift("↓");

/// Whether shortcuts are written the macOS way (`⇧⌘S`) rather than `Ctrl+Shift+S`. egui knows the
/// platform from the build target, and on the web from the browser's user agent. The catalog picks
/// each command's chord by it.
pub(crate) fn mac_shortcuts(ctx: &egui::Context) -> bool {
    ctx.os() == egui::os::OperatingSystem::Mac
}

/// How a command's chord is written in a menu row (`⌘C` or `Ctrl+C`): the registry's shortcut, and the
/// clipboard chords for Copy and Paste, which the framework's clipboard commands own.
pub(crate) fn chord_label(ctx: &egui::Context, id: &str) -> Option<String> {
    let mac = mac_shortcuts(ctx);
    let shortcut = commands::command(id)
        .and_then(|c| c.shortcut_for(mac))
        .or((id == "edit.copy").then_some(COPY))
        .or((id == "edit.paste").then_some(Shortcut::cmd("V")))?;
    Some(shortcut.label(mac))
}

impl PdfCraftApp {
    /// Runs a command the runtime fired from a key, a menu row, the palette, a component or
    /// automation. The catalog's app-side handlers come first; every other id is a registered
    /// command and goes through [`Self::execute`].
    pub(crate) fn run_fired(&mut self, ctx: &egui::Context, fired: &Fired) {
        if let Some(pasted) = &fired.pasted {
            self.offer_clip_content(pasted);
        }
        let id = fired.command.id.as_str();
        // A Save shortcut pressed on one frame fires at the start of the next frame; if another
        // document became active meanwhile, the shortcut belongs to the first document, not the new one.
        if fired.source == craft::ui::runtime::commands::FireSource::Key
            && matches!(id, "file.save" | "file.save_as")
            && self.last_frame_active.is_some()
            && self.last_frame_active != self.active_ids().map(|(_, doc)| doc)
        {
            return;
        }
        // The OCR Verify screen keeps the window's toolbars and owns F8 and Shift+F8 (the next
        // and previous word to review), so they don't show or hide the interface there.
        if id == crate::viewer_prefs::TOGGLE && self.ocr_verify.open {
            return;
        }
        if self.run_catalog_command(ctx, fired) {
            return;
        }
        // In Organize the clipboard rows and keys are the picked pages' (the catalog enables them as
        // `page.copy`, `page.cut` and `page.paste`); the registry's text Copy itself stays off there.
        let organize = self.active.and_then(|i| self.views.get(i)).is_some_and(|v| v.organize);
        if !organize
            && !self.is_preparing()
            && id == crate::menus::PASTE
            && fired.pasted.is_none()
            && fired.source == craft::ui::runtime::commands::FireSource::Menu
        {
            craft::ui::runtime::overlay::close_popups(ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            return;
        }
        let page_command = match id {
            "edit.copy" => Some("page.copy"),
            crate::menus::CUT => Some("page.cut"),
            crate::menus::PASTE => Some("page.paste"),
            _ => None,
        };
        if !organize && !self.is_preparing() && id == crate::menus::PASTE && fired.pasted.is_some() {
            if let Some(i) = self.active.filter(|&i| i < self.views.len()) {
                self.paste_from_clipboard_at(ctx, i, fired.pasted.as_ref(), fired.paste_at);
            }
            return;
        }
        self.execute(ctx, page_command.filter(|_| organize).unwrap_or(id));
    }

    /// Answers a key press whose command only the app's own state disabled: the user is told
    /// why, as a menu row's disabled reason says. Previous and next view stay silent when there
    /// is no view to visit, as the page view's own keys are.
    pub(crate) fn run_declined(&mut self, fired: &Fired) {
        let id = fired.command.id.as_str();
        if matches!(id, "view.previous_view" | "view.next_view") {
            return;
        }
        if let Some(spec) = commands::command(id) {
            let why = self.disabled_reason(spec);
            self.notify(why);
        }
    }

    /// The handlers of the catalog ids that are not registered engine commands. `false` when
    /// `fired` is not one of them.
    fn run_catalog_command(&mut self, ctx: &egui::Context, fired: &Fired) -> bool {
        let id = fired.command.id.as_str();
        let active = self.active;
        match id {
            crate::menus::QUIT => {
                self.request_quit(ctx);
            }
            // The Combine list takes Select all and Delete while it shows, whatever has the focus
            // there (a text field keeps them).
            "edit.select_all" if self.combine_showing() => {
                let rows: Vec<usize> = (0..self.combine_draft.len()).collect();
                self.select_combine_rows(&rows);
            }
            crate::menus::DELETE if self.combine_showing() => self.combine_apply(crate::combine_ui::RowAction::Remove),
            crate::menus::DELETE if !self.covers_pages() => {
                if let Some(i) = active {
                    let organize = self.views[i].organize;
                    if organize {
                        let _ = self.execute(ctx, "page.delete");
                    } else if let Some(doc) = self.session.get(self.views[i].id)
                        && doc.allows_annotation()
                        && self.views[i].session.comments.primary().is_some()
                    {
                        let info = doc.info.clone();
                        if let Some(edit) = crate::comments::delete_selected(&mut self.views[i].session.comments, &info) {
                            self.apply_edit(ctx, edit);
                        }
                    }
                }
            }
            "edit.select_all" => {
                if let Some(i) = active {
                    self.select_all(i, ctx);
                }
            }
            "form.calculation_order.back" => self.left = crate::LeftPanel::Tool("form"),
            "view.zoom.fit_height" => {
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    v.view.fit = crate::canvas::Fit::Height;
                }
            }
            "view.fit_toggle" => {
                use crate::canvas::Fit;
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    let fit = if v.view.fit == Fit::Page { Fit::Width } else { Fit::Page };
                    v.view.fit = fit;
                }
            }
            "view.rail.left" | "view.rail.right" => {
                use craft::behavior::DockSide;
                let side = if id == "view.rail.left" { DockSide::Start } else { DockSide::End };
                if let Err(e) = crate::side_panels::set_rail_side(self, side) {
                    self.notify(e);
                }
            }
            "home.unpin_tool" => {
                // PLAN: assumed params `{ "tool": <group id> }` and `PdfCraftApp::unpin_tool(&mut self, &str)` (pdf-chrome-w9).
                if let Some(tool) = fired.command.params.get("tool").and_then(|t| t.as_str()) {
                    let tool = tool.to_string();
                    self.unpin_tool(&tool);
                }
            }
            // A row that opens Preferences on one of its pages (the theme button's Appearance settings…).
            "app.preferences" if fired.command.params.get("tab").is_some() => {
                if let Some(page) = fired.command.params.get("tab").and_then(serde_json::Value::as_str) {
                    self.open_preferences_at(page);
                }
            }
            "app.theme" => {
                let next = match self.theme_preference {
                    ModeChoice::System => ModeChoice::Light,
                    ModeChoice::Light => ModeChoice::Dark,
                    ModeChoice::Dark => ModeChoice::System,
                };
                self.set_theme_preference(next);
            }
            "tools.expand" => self.expand_all_tools(ctx),
            "tools.collapse" => self.all_tools_full = false,
            "tools.all" => {
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    v.organize = false;
                }
                self.left = crate::LeftPanel::AllTools;
                self.enter_mode(ctx, Mode::AllTools);
            }
            "bookmark.paste" => self.bookmark_action(ctx, crate::panels::BmAction::Paste { parent: Vec::new(), index: usize::MAX }),
            "bookmark.expand_all" | "bookmark.collapse_all" | "bookmark.expand_level" => {
                let level = match id {
                    "bookmark.expand_all" => usize::MAX,
                    "bookmark.expand_level" => 1,
                    _ => 0,
                };
                if let Some(i) = active
                    && let Some(doc) = self.session.get(self.views[i].id)
                {
                    crate::panels::bookmarks::expand_to(&mut self.views[i], &doc.info.outline, level);
                }
            }
            "layers.reset_visibility" => {
                if let Some(i) = active {
                    let doc = self.views[i].id;
                    if self.session.reset_layers(doc) {
                        self.views[i].invalidate_content();
                    }
                }
            }
            crate::menus::PANEL => self.choose_right_panel(crate::menus::panel(&fired.command)),
            crate::menus::DOCUMENT => {
                if let Some(i) = crate::menus::param_index(&fired.command).filter(|&i| i < self.views.len()) {
                    self.active = Some(i);
                }
            }
            "file.open_recent" if crate::menus::param_index(&fired.command).is_some() => {
                let path = crate::menus::param_index(&fired.command).and_then(|i| self.recent.get(i)).map(|r| r.path.clone());
                if let Some(path) = path {
                    self.open_recent(&path);
                }
            }
            tool if tool.strip_prefix("tools.").and_then(pdfcraft_engine::catalog::group).is_some() => {
                if let Some(g) = tool.strip_prefix("tools.").and_then(pdfcraft_engine::catalog::group) {
                    self.select_tool(ctx, g.id);
                }
            }
            _ => return false,
        }
        true
    }

    /// Switches between light and dark from the mode the interface shows now.
    fn toggle_theme(&mut self, ctx: &egui::Context) {
        let next = if self.root_env(ctx).resolved(ctx).mode == craft::theme::Mode::Light { ModeChoice::Dark } else { ModeChoice::Light };
        self.set_theme_preference(next);
    }

    /// Whether a registered command can run now. The engine judges the document (security,
    /// contents, undo history); view state it can't see is checked here.
    pub(crate) fn command_enabled(&self, spec: &CommandSpec) -> bool {
        // Undo and Redo act on the Combine files list while its tab shows.
        if self.combine_showing() && matches!(spec.needs, commands::Needs::Undo | commands::Needs::Redo) {
            return self.combine_can_undo(spec.needs == commands::Needs::Undo);
        }
        // Undo also takes back typing in a box open on the page, and Redo what Undo took there,
        // with no document step.
        (commands::is_enabled(spec, &self.session, self.active_ids().map(|(_, id)| id))
            || spec.needs == commands::Needs::Undo && self.page_typing_unapplied()
            || spec.needs == commands::Needs::Redo && self.open_box_can_redo())
            && (spec.id != "edit.copy"
                || self.active.and_then(|i| self.views.get(i)).is_some_and(|v| {
                    // Selected text, or a picked picture, which Copy takes in its original format.
                    !v.organize
                        && (v.session.all_text
                            || v.selection_waits_for_text().is_some()
                            || v.selected_text().is_some_and(|t| !t.is_empty())
                            || v.session.picked_image.is_some()
                            || v.session.image_selection.is_some()
                            || v.session.comments.primary().is_some())
                }))
            && (spec.needs != commands::Needs::TwoPageView || self.active.and_then(|i| self.views.get(i)).is_some_and(crate::DocView::cover_applies))
            && match spec.id {
                "comment.copy" => self.active.and_then(|i| self.views.get(i)).is_some_and(|v| v.session.comments.primary().is_some()),
                "comment.paste" => self.comment_clipboard.is_some(),
                "view.previous_view" => self.active.and_then(|i| self.views.get(i)).is_some_and(|v| !v.back.is_empty()),
                "view.next_view" => self.active.and_then(|i| self.views.get(i)).is_some_and(|v| !v.forward.is_empty()),
                _ => true,
            }
    }

    /// Run a registered command by id. Returns `false` when the id is unknown or the command
    /// is disabled right now (the user is told why).
    ///
    /// Last-resort guard (app-agents.md rule 13): a command that panics is reported and the app, with its
    /// open documents, keeps running. Edits are applied to a copy, so the document is unchanged.
    pub fn execute(&mut self, ctx: &egui::Context, id: &str) -> bool {
        let done = match pdfcraft_engine::guard(|| self.execute_unguarded(ctx, id)) {
            Ok(done) => done,
            Err(m) => {
                // A command stopped part way through closing or replacing a tab must not leave
                // the next frame pointing past the last one (#3914).
                self.keep_active_valid();
                self.notify_fmt("That didn't work: an internal error stopped it ({m}). Your documents are unchanged.", &[("m", m.as_str())]);
                true
            }
        };
        crate::dialogs::discard_inactive_password_drafts(self);
        done
    }

    /// Why the command can't run right now, for the user.
    fn disabled_reason(&self, spec: &CommandSpec) -> String {
        match spec.needs {
            commands::Needs::Undo => tl!("Nothing to undo").to_string(),
            commands::Needs::Redo => tl!("Nothing to redo").to_string(),
            commands::Needs::FillForms if self.active.is_some() => tl!("This document has no form fields you can fill in").to_string(),
            commands::Needs::HasComments if self.active.is_some() => tl!("This document has no comments to flatten").to_string(),
            commands::Needs::HasFields if self.active.is_some() => tl!("This document has no form fields to flatten").to_string(),
            commands::Needs::HasRedactions if self.active.is_some() => {
                tl!("There are no redaction marks (mark text, areas or pages first)").to_string()
            }
            commands::Needs::Marks(k) if self.active.is_some() => {
                let kind = match k {
                    pdfcraft_engine::MarkKind::HeaderFooter => tl!("header or footer"),
                    pdfcraft_engine::MarkKind::Watermark => tl!("watermark"),
                    pdfcraft_engine::MarkKind::Background => tl!("background"),
                    pdfcraft_engine::MarkKind::PrinterMarks => tl!("printer marks"),
                };
                crate::i18n::fmt(tl!("This document has no {kind} to change"), &[("kind", kind)])
            }
            // Restricted documents ask for their permissions password instead, so only an
            // unprotected or a read-only document gets here.
            commands::Needs::Security | commands::Needs::ProtectedSecurity if self.active.is_some() => {
                match self.active_ids().and_then(|(_, id)| self.session.get(id)) {
                    Some(d) if !d.allows_security_change() => crate::i18n::fmt(tl!("{name} is read-only"), &[("name", &d.name)]),
                    _ => tl!("This document isn't password-protected").to_string(),
                }
            }
            commands::Needs::Unlock if self.active.is_some() => tl!("This document has no restrictions to lift").to_string(),
            commands::Needs::Assembly | commands::Needs::PageExtraction | commands::Needs::Modification | commands::Needs::Annotate
                if self.active.is_some() =>
            {
                tl!("The document's security settings don't allow this change").to_string()
            }
            commands::Needs::Copying if self.active.is_some() => crate::copy_refused().to_string(),
            commands::Needs::Printing if self.active.is_some() => crate::print_refused().to_string(),
            commands::Needs::TwoPageView if self.active.is_some() => tl!("Switch to two-page view first to show the cover page").to_string(),
            _ => tl!("Open a document first").to_string(),
        }
    }

    fn execute_unguarded(&mut self, ctx: &egui::Context, id: &str) -> bool {
        // Paste is the framework's clipboard command, not a registered one: the pages take what
        // the clipboard holds (objects, a picture or comment text).
        if id == crate::menus::PASTE {
            let Some(i) = self.active.filter(|&i| i < self.views.len()) else { return false };
            if self.views[i].session.forms.focus.is_some() {
                return false;
            }
            self.paste_from_clipboard(ctx, i);
            return true;
        }
        if (id == "edit.copy" || id == crate::menus::CUT)
            && let Some(i) = self.active.filter(|&i| self.views.get(i).is_some_and(|v| !v.organize))
        {
            return if id == "edit.copy" { self.copy_from_canvas(ctx, i) } else { self.cut_from_canvas(ctx, i) };
        }
        let Some(spec) = commands::command(id) else { return false };
        let close_empty_window = id == "file.close" && self.active.is_none() && mac_shortcuts(ctx) && !cfg!(target_arch = "wasm32");
        if !close_empty_window && !self.command_enabled(spec) {
            let why = self.disabled_reason(spec);
            self.notify(why);
            return false;
        }
        let active = self.active;
        let targets = active.map(|i| self.views[i].target_pages()).unwrap_or_default();
        self.run_file_command(ctx, id, active)
            .or_else(|| self.run_view_command(ctx, id, active))
            .or_else(|| self.run_page_command(ctx, id, active, targets))
            .or_else(|| self.run_edit_command(ctx, id, active))
            .or_else(|| self.run_form_command(ctx, id, active))
            .or_else(|| self.run_secure_command(ctx, id, active))
            .unwrap_or(false)
    }

    fn run_file_command(&mut self, ctx: &egui::Context, id: &str, active: Option<usize>) -> Option<bool> {
        match id {
            "file.open" => self.open_dialog(),
            "file.open_recent" => match self.recent.first().map(|r| r.path.clone()) {
                // The palette runs commands without a submenu: open the most recent file.
                Some(p) => self.open_recent(&p),
                None => self.notify_tr("No recent files"),
            },
            "file.clear_recent" if self.recent.is_empty() => self.notify_tr("No recent files"),
            "file.clear_recent" => self.recent.clear(),
            "file.pin_folder" => self.pin_folder_dialog(),
            "file.save" => {
                self.save_active(ctx, SaveTarget::InPlace);
            }
            "file.save_as" => {
                self.save_active(ctx, SaveTarget::As);
            }
            "file.close" => match active {
                Some(i) => self.request_close_tab(ctx, i),
                // macOS ⌘W closes the window when no document is open (#869), through the quit guard.
                None if crate::commands::mac_shortcuts(ctx) && !cfg!(target_arch = "wasm32") => {
                    self.request_quit(ctx);
                }
                None => {}
            },
            "file.close_all" => self.close_all(ctx),
            // PLAN: assumed the dialog openers are `PdfCraftApp` methods named as the sheet gives them (dialogs A-3..A-10), taking no argument unless the sheet names one.
            "file.revert" => self.ask_revert(),
            "file.properties" => self.open_properties(ctx, PropsTab::Description),
            "file.word_count" => self.open_word_count(),
            "protect.properties" => self.open_properties(ctx, PropsTab::Security),
            // A restricted document's security changes once its permissions password is entered.
            "protect.password" | "protect.remove" if self.active_ids().and_then(|(_, id)| self.session.get(id)).is_some_and(|d| d.can_unlock()) => {
                let then = if id == "protect.password" { "protect.password" } else { "protect.remove" };
                self.request_unlock(Some(then));
            }
            "protect.unlock" => self.request_unlock(None),
            "protect.password" => self.open_protect(),
            "prepress.convert_colors" => self.open_convert_colours(ctx),
            "prepress.hairlines" => self.open_fix_hairlines(ctx),
            "protect.remove" => {
                if self.apply_edit(ctx, Edit::RemoveProtection) {
                    self.notify_tr("Security will be removed when you save");
                }
            }
            link if pdfcraft_engine::links::for_command(link).is_some() => {
                if let Some(l) = pdfcraft_engine::links::for_command(link) {
                    self.open_url(l.url);
                }
            }
            "app.preferences" => self.open_preferences(),
            "protect.remove_hidden" => self.open_remove_hidden(),
            "print.dialog" => self.open_print(),
            "optimize.advanced" => crate::optimize_ui::open_optimizer(self, ctx),
            "doc.compare" => crate::compare_ui::open_compare_dialog(self, ctx),
            "standards.pdfa" => self.open_pdfa(),
            "actions.wizard" => crate::actions_ui::open_wizard(self, ctx, None),
            "actions.distribution" => crate::actions_ui::open_wizard(self, ctx, Some("Prepare for Distribution")),
            "actions.optimize_scans" => crate::actions_ui::open_wizard(self, ctx, Some("Optimize Scanned Documents")),
            "create.blank" => self.create_blank(),
            "create.file" => self.open_dialog(),
            "create.multiple" => self.create_multiple_dialog(),
            "create.multiple_separate" => self.create_multiple_separate_dialog(),
            "create.images" => self.create_from_images_dialog(),
            "create.clipboard" => self.create_from_clipboard(),
            "optimize.reduce" => self.reduce_file_size(ctx),
            "help.shortcuts" => self.open_shortcuts(ctx),
            "help.about" => self.open_about(craft::ui::patterns::form_dialogs::AboutTab::default()),
            "help.check_updates" => self.check_for_updates(),
            _ => return None,
        }
        Some(true)
    }

    fn run_view_command(&mut self, ctx: &egui::Context, id: &str, active: Option<usize>) -> Option<bool> {
        match id {
            "ui.dock.reset" => {
                if let Err(e) = crate::side_panels::command(self, true, &serde_json::Value::Null) {
                    self.notify(e);
                }
            }
            "view.select_tool" => self.quick_tool = crate::QuickTool::Select,
            "view.hand_tool" => self.quick_tool = crate::QuickTool::Hand,
            // Find and replace changes page text, so it runs in Edit text mode (as Acrobat's
            // does): the command turns the mode on, and the find bar shows its Replace row there.
            "edit.replace" => {
                if let Some(i) = active {
                    self.quick_tool = crate::QuickTool::EditText;
                    self.left = crate::LeftPanel::Tool("edit");
                    self.reveal_tools();
                    self.views[i].open_replace();
                }
            }
            "view.side_panel" => self.toggle_side_panel(),
            "view.tools_pane" => self.set_left_open(!self.left_open()),
            "view.focus_page_input" => {
                if let Some(view) = active.and_then(|i| self.views.get_mut(i)) {
                    // The page box in the toolbar (chrome.rs); select its label so typing replaces it.
                    let id = view.page_input_id();
                    // Text being typed into the box stays; else it shows the current page's label.
                    if !view.page_input_draft
                        && let Some(doc) = self.session.get(view.id)
                    {
                        // Navigation and this command can arrive before the next rail frame.
                        view.page_input = view.current_page_label(doc.info.pages.get(view.current()).map(|p| p.label.as_str()));
                    }
                    craft::ui::runtime::focus::request(ctx, craft::ui::runtime::focus::FocusTarget::Id(id));
                    view.page_input_reselect = true;
                }
            }
            "view.palette" => self.palette_open = !self.palette_open,
            "view.zoom.actual_size"
            | "view.zoom.page_level"
            | "view.zoom.fit_width"
            | "view.zoom.in"
            | "view.zoom.out"
            | "view.rotate.cw"
            | "view.rotate.ccw" => {
                use crate::canvas::Fit;
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    match id {
                        "view.zoom.actual_size" => {
                            v.view.fit = Fit::None;
                            v.view.zoom = 1.0;
                        }
                        "view.zoom.page_level" => v.view.fit = Fit::Page,
                        "view.zoom.fit_width" => v.view.fit = Fit::Width,
                        "view.zoom.in" => v.view.zoom_step(true),
                        "view.zoom.out" => v.view.zoom_step(false),
                        "view.rotate.cw" => v.view.rotation = (v.view.rotation + 90) % 360,
                        _ => v.view.rotation = (v.view.rotation + 270) % 360,
                    }
                }
            }
            "view.first_page" | "view.last_page" | "view.next_page" | "view.previous_page" => {
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    match id {
                        "view.first_page" => v.go_to_page(0),
                        "view.last_page" => v.go_to_end(),
                        // A step turns a whole spread in two-page view.
                        "view.next_page" => v.step_page(true),
                        _ => v.step_page(false),
                    }
                }
            }
            "view.layout.single" | "view.layout.two_up" => {
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    v.view.display = if id == "view.layout.single" { PageDisplay::Single } else { PageDisplay::TwoUp };
                    v.view.go_to(v.current());
                }
            }
            // The Continuous scrolling checkbox (an independent toggle over the base layout).
            "view.continuous" => {
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    v.view.continuous = !v.view.continuous;
                    v.view.go_to(v.current());
                }
            }
            "view.page_gaps" => {
                if let Some(view) = active.and_then(|i| self.views.get(i)) {
                    self.set_active_page_gaps(!view.view.page_gaps);
                }
            }
            "view.layout.cover" => {
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    v.view.cover = !v.view.cover;
                    v.view.go_to(v.current());
                }
            }
            "view.fit_width_scrolling" | "view.fit_one_page" => {
                use crate::canvas::Fit;
                // Fit one full page shows a single page on its own; fit width scrolls it.
                let (continuous, fit) = if id == "view.fit_one_page" { (false, Fit::Page) } else { (true, Fit::Width) };
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    v.view.display = PageDisplay::Single;
                    v.view.continuous = continuous;
                    v.view.fit = fit;
                    v.view.go_to(v.current());
                }
            }
            "view.full_screen" => {
                let on = !self.full_screen();
                self.set_full_screen(on);
            }
            "view.read_mode" => self.switch_mode(ctx, if self.mode == Mode::Read { Mode::AllTools } else { Mode::Read }, false),
            "view.split" => self.toggle_split(ctx),
            crate::viewer_prefs::TOGGLE => self.toggle_interface(),
            "view.previous_view" | "view.next_view" => {
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    return Some(v.view_history(id == "view.next_view"));
                }
            }
            "view.next_tab" => self.step_tab(true),
            "view.previous_tab" => self.step_tab(false),
            "view.theme" => self.toggle_theme(ctx),
            command if command.starts_with("measure.") => crate::measure_ui::command(self, command),
            "view.theme.system" => self.set_theme_preference(ModeChoice::System),
            "view.theme.light" => self.set_theme_preference(ModeChoice::Light),
            "view.theme.dark" => self.set_theme_preference(ModeChoice::Dark),
            "view.fit_visible" => {
                if let Some(i) = self.active
                    && let Err(e) = self.fit_visible(i)
                {
                    self.notify_fmt("Couldn't fit the visible content: {e}", &[("e", &e.to_string())]);
                }
            }
            "view.marquee_zoom" => self.quick_tool = crate::QuickTool::MarqueeZoom,
            _ => return None,
        }
        Some(true)
    }

    fn run_page_command(&mut self, ctx: &egui::Context, id: &str, active: Option<usize>, targets: Vec<usize>) -> Option<bool> {
        match id {
            "page.combine" => self.open_combine_tab(),
            "page.number" => self.open_number_pages(ctx),
            "page.properties" => {
                if let Some(i) = active
                    && let Some(doc) = self.session.get(self.views[i].id)
                {
                    let pages = self.views[i].target_pages();
                    let orders = doc.tab_orders();
                    let mut theirs = pages.iter().map(|p| orders.get(*p).copied());
                    let first = theirs.next().flatten();
                    let before = if theirs.all(|o| o == first) { first } else { None };
                    let generation = doc.edit_generation();
                    let draft = crate::PagePropsDraft { doc: self.views[i].id, generation, pages, before };
                    self.open_page_props(ctx, draft);
                }
            }
            "bookmark.add" => self.bookmark_action(ctx, crate::panels::BmAction::New),
            "bookmark.from_structure" => self.bookmark_action(ctx, crate::panels::BmAction::FromStructure),
            "bookmark.from_text_styles" => self.bookmark_action(ctx, crate::panels::BmAction::FromTextStyles),
            "page.organize" => {
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    if v.organize {
                        v.organize = false;
                    } else {
                        v.open_organize();
                    }
                }
            }
            "page.rotate" => {
                self.apply_edit(ctx, Edit::RotatePages { pages: targets, degrees: 90 });
            }
            "page.rotate_ccw" => {
                self.apply_edit(ctx, Edit::RotatePages { pages: targets, degrees: -90 });
            }
            "page.delete" => {
                self.apply_edit(ctx, Edit::DeletePages { pages: targets });
            }
            "page.insert_blank" => {
                if let (Some(i), Some(&last)) = (active, targets.last())
                    && let Some(p) = self.session.get(self.views[i].id).and_then(|d| d.info.pages.get(last))
                {
                    let (w, h) = ((p.crop[2] - p.crop[0]).abs().max(1.0) as f64, (p.crop[3] - p.crop[1]).abs().max(1.0) as f64);
                    self.apply_edit(ctx, Edit::InsertBlankPage { at: last + 1, width: w, height: h });
                }
            }
            "page.insert" => self.insert_from_file_dialog(),
            "page.replace" => self.replace_pages_dialog(),
            "page.copy" => self.copy_pages(ctx, false),
            "page.cut" => self.copy_pages(ctx, true),
            "page.paste" => self.paste_pages(ctx),
            "page.duplicate" => {
                self.apply_edit(ctx, Edit::DuplicatePages { pages: targets, at: None });
            }
            // Two or more selected pages reverse among themselves; otherwise the whole document.
            "page.reverse" => {
                let all = active.and_then(|i| self.views.get(i)).map_or(0, |v| v.page_count());
                let pages = if targets.len() >= 2 { targets } else { (0..all).collect() };
                self.apply_edit(ctx, Edit::ReversePages { pages });
            }
            "page.crop" => {
                self.quick_tool = crate::QuickTool::Crop;
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    v.organize = false;
                }
                self.notify_tr("Drag a rectangle on a page to crop it; double-click a page for Set Page Boxes");
            }
            "page.boxes" => self.open_page_boxes(ctx, false),
            "page.extract" => self.open_extract(ctx),
            "page.rotate_dialog" => self.open_rotate(ctx),
            "page.split" => self.open_split(ctx, false),
            _ => return None,
        }
        Some(true)
    }

    fn run_edit_command(&mut self, ctx: &egui::Context, id: &str, active: Option<usize>) -> Option<bool> {
        match id {
            "edit.attach" => self.attach_file_dialog(),
            "edit.undo" => self.undo(ctx),
            "edit.redo" => self.redo(ctx),
            "edit.find" => {
                if let Some(i) = active {
                    self.views[i].open_find();
                }
            }
            "comment.list" => self.choose_right_panel(Some(RightPanel::Comments)),
            tool if crate::comments::CommentTool::from_command(tool).is_some() => {
                let Some(tool) = crate::comments::CommentTool::from_command(tool) else { return Some(false) };
                self.comment_prefs.group_tool[tool.group()] = tool;
                self.quick_tool = crate::QuickTool::Comment(tool);
                // Acrobat opens the Comments panel with the commenting tools, unless the user closed it.
                if self.right().is_none() && !self.comments_panel_closed {
                    self.choose_right_panel(Some(RightPanel::Comments));
                }
                // A text selection made before picking a markup tool is marked right away, on
                // every page it covers, in one undo step.
                if let (Some(kind), Some(i)) = (tool.markup(), active)
                    && let Some(doc) = self.session.get(self.views[i].id)
                {
                    // Pages still to be read are marked when they are, by the tool now picked.
                    let tool_now = self.quick_tool;
                    if let crate::canvas::ToolMarks::Marks(marks, head) = self.views[i].tool_marks(tool_now, true, None, false, &doc.info) {
                        let edits: Vec<Edit> = marks
                            .into_iter()
                            .map(|(page, quads)| {
                                let style = self.comment_prefs.style(tool);
                                let author = self.comment_prefs.author.clone();
                                let shape = pdfcraft_engine::Shape::TextMarkup { kind, quads };
                                Edit::AddAnnotation(pdfcraft_engine::NewAnnotation { page, shape, style, contents: String::new(), author })
                            })
                            .collect();
                        if let Some(edit) = self.views[i].selection_marks_edit(edits, head) {
                            self.views[i].clear_selection();
                            self.apply_edit(ctx, edit);
                        }
                    }
                }
            }
            "comment.flatten" => {
                self.apply_edit(ctx, Edit::Flatten { comments: true, fields: false });
            }
            "edit.header_footer"
            | "edit.watermark"
            | "edit.background"
            | "edit.header_footer.update"
            | "edit.watermark.update"
            | "edit.background.update" => {
                use pdfcraft_engine::MarkKind as K;
                let kind = if id.starts_with("edit.header_footer") {
                    K::HeaderFooter
                } else if id.starts_with("edit.watermark") {
                    K::Watermark
                } else {
                    K::Background
                };
                self.open_marks(ctx, kind, id.ends_with(".update"));
            }
            "edit.bates" => {
                // Bates numbering ▸ Add: the header & footer dialog with a Bates number in the
                // bottom-right box (6 digits from 1), ready to edit.
                if self.marks_draft.hf.text.iter().all(|t| !t.contains("<<Bates")) {
                    self.marks_draft.hf.text[5] = "<<Bates Number#6#1##>>".into();
                }
                self.marks_draft.focused_box = 5;
                self.open_marks(ctx, pdfcraft_engine::MarkKind::HeaderFooter, false);
            }
            "prepress.marks" => self.open_printer_marks(ctx),
            "edit.header_footer.remove" | "edit.watermark.remove" | "edit.background.remove" | "prepress.marks.remove" => {
                use pdfcraft_engine::MarkKind as K;
                let kind = match id {
                    "edit.header_footer.remove" => K::HeaderFooter,
                    "edit.watermark.remove" => K::Watermark,
                    "prepress.marks.remove" => K::PrinterMarks,
                    _ => K::Background,
                };
                self.apply_edit(ctx, Edit::RemoveMarks { kind });
            }
            "export.image" => crate::export_ui::open(self, ctx, crate::export_ui::ExportKind::Image),
            "export.text" => crate::export_ui::open(self, ctx, crate::export_ui::ExportKind::Text),
            "ocr.recognize" => self.open_recognize_text(ctx),
            "ocr.enhance" => self.open_enhance_scan(ctx),
            "ocr.recognize_batch" => self.ocr_files_dialog(),
            "ocr.correct" => self.ocr_verify.open = true,
            "edit.edit_text" => {
                self.quick_tool = crate::QuickTool::EditText;
                self.left = crate::LeftPanel::Tool("edit");
                self.reveal_tools();
                self.notify_tr("Click text or an image to edit it");
            }
            "edit.advanced_search" => {
                if let Some(i) = self.active {
                    let f = self.views[i].find.get_or_insert_with(Default::default);
                    f.in_panel = true;
                    f.bar.open();
                    self.choose_right_panel(Some(crate::RightPanel::Search));
                }
            }
            "export.all_images" => crate::export_ui::open(self, ctx, crate::export_ui::ExportKind::AllImages),
            "edit.text" => {
                self.quick_tool = crate::QuickTool::AddText;
                self.left = crate::LeftPanel::Tool("edit");
                self.reveal_tools();
            }
            "edit.image" => self.add_image_dialog(),
            "edit.link" => {
                self.quick_tool = crate::QuickTool::Link;
                self.notify_tr("Drag a rectangle to create a link; double-click a link to edit it");
            }
            "edit.links_from_urls" => self.links_from_urls(ctx),
            "edit.remove_links" => self.remove_all_links(ctx),
            "comment.import" => self.import_comments_dialog(),
            "form.import_data" => self.import_data_dialog(),
            "comment.stamp" => {
                self.left = crate::LeftPanel::Tool("stamp");
                self.reveal_tools();
            }
            "comment.export" => self.export_data_dialog(ctx, true, false),
            "comment.summarize" => self.open_summarize(ctx),
            "edit.column_select" => self.quick_tool = crate::QuickTool::ColumnSelect,
            "edit.snapshot" => {
                self.quick_tool = crate::QuickTool::Snapshot;
                self.notify_tr("Drag a rectangle around the area to copy");
            }
            "comment.copy" => {
                let Some(i) = active else { return Some(false) };
                return Some(self.copy_selected_comments(i));
            }
            "comment.paste" => {
                let Some(i) = active else { return Some(false) };
                return Some(self.paste_comments(ctx, i, None, None));
            }
            "comment.hide_all" => {
                if let Some(i) = active {
                    let id = self.views[i].id;
                    let hide = !self.session.get(id).is_some_and(|d| d.comments_hidden());
                    if self.session.set_hide_comments(id, hide) {
                        self.views[i].invalidate_content();
                    }
                }
            }
            "export.docx" => self.export_office_dialog(ctx, pdfcraft_engine::compare::OfficeFormat::Docx),
            "export.pptx" => self.export_office_dialog(ctx, pdfcraft_engine::compare::OfficeFormat::Pptx),
            "export.html" => self.export_office_dialog(ctx, pdfcraft_engine::compare::OfficeFormat::Html),
            "export.rtf" => self.export_office_dialog(ctx, pdfcraft_engine::compare::OfficeFormat::Rtf),
            "ocr.enhance_camera" => {
                // The corners start on the document's edges when they can be found, else
                // inset on the current page (camera_ui::page_input).
                self.quick_tool = crate::QuickTool::EnhanceCamera;
                if let Some(i) = self.active
                    && let Some(view) = self.views.get_mut(i)
                    && let Some(page) = self.session.get(view.id).and_then(|d| d.info.pages.get(view.current()).cloned())
                {
                    view.session.camera_done = None;
                    view.session.camera = self.session.detect_camera_corners(view.id, view.current()).map(|found| crate::camera_ui::CameraQuad {
                        page: view.current(),
                        corners: found.map(|[x, y]| page.user_to_view(x as f32, y as f32)),
                    });
                }
                self.notify_tr("Drag the corners onto the document's edges, then choose Enhance page.");
            }
            _ => return None,
        }
        Some(true)
    }

    fn run_form_command(&mut self, ctx: &egui::Context, id: &str, active: Option<usize>) -> Option<bool> {
        match id {
            "form.fields" => self.choose_right_panel(Some(RightPanel::Fields)),
            "form.calculation_order" => {
                self.left = crate::LeftPanel::CalculationOrder;
                // The page edits the fields, so Preview ends; the page grid closes, as for a tool.
                self.form_preview = false;
                if let Some(v) = active.and_then(|i| self.views.get_mut(i)) {
                    v.organize = false;
                }
                self.reveal_tools();
            }
            "form.flatten" => {
                // Flatten what's typed in a field too (#166); a refused value flattens nothing.
                if self.commit_form_typing(ctx) {
                    self.apply_edit(ctx, Edit::Flatten { comments: false, fields: true });
                }
            }
            "form.clear" => {
                if let Some(i) = active {
                    self.views[i].session.forms.focus = None;
                }
                self.apply_edit(ctx, Edit::ResetForm { names: None });
            }
            "form.tab_order.row" | "form.tab_order.column" | "form.tab_order.structure" => {
                let order = match id {
                    "form.tab_order.row" => pdfcraft_engine::TabOrder::Row,
                    "form.tab_order.column" => pdfcraft_engine::TabOrder::Column,
                    _ => pdfcraft_engine::TabOrder::Structure,
                };
                let n = active.and_then(|i| self.session.get(self.views[i].id)).map_or(0, |d| d.info.pages.len());
                self.apply_edit(ctx, Edit::SetTabOrder { pages: (0..n).collect(), order });
            }
            "sign.digital" | "sign.certify" => {
                let certify = id == "sign.certify";
                self.quick_tool = crate::QuickTool::SignArea { certify };
                self.notify_tr("Drag to draw the area where the signature should appear.");
            }
            "sign.certify_invisible" => {
                let page = active.and_then(|i| self.views.get(i)).map_or(0, |v| v.current());
                self.start_signing(ctx, page, None, None, Some(2));
            }
            "sign.validate" => {
                let certs = self.session.trusted_certificates().to_vec();
                self.apply_trusted_certificates(certs);
                self.choose_right_panel(Some(RightPanel::Signatures));
            }
            "sign.panel" => self.choose_right_panel(Some(RightPanel::Signatures)),
            "form.export_data" => self.export_data_dialog(ctx, false, true),
            "form.merge_data" => self.merge_data_dialog(),
            "form.prepare" => {
                self.left = crate::LeftPanel::Tool("form");
                self.reveal_tools();
                self.choose_right_panel(Some(RightPanel::Fields));
                // Like Acrobat, a document without fields gets them detected on the way in.
                if let Some((_, id)) = self.active_ids()
                    && self.session.get(id).is_some_and(|d| d.form.is_empty() && d.editable())
                {
                    self.detect_fields(ctx);
                }
            }
            "form.detect" => self.detect_fields(ctx),
            "form.field.properties" => {
                if let Some((name, w)) = active.and_then(|i| self.views[i].session.prepare.selected.clone()) {
                    crate::prepare::dialog::open(self, &name, w, crate::prepare::FieldTab::General);
                }
            }
            field if crate::prepare::FieldTool::from_command(field).is_some() => {
                let Some(tool) = crate::prepare::FieldTool::from_command(field) else { return Some(false) };
                self.quick_tool = crate::QuickTool::Field(tool);
                self.left = crate::LeftPanel::Tool("form");
                self.reveal_tools();
                if let Some(i) = active {
                    self.views[i].session.forms.focus = None;
                }
                self.notify_fmt(
                    "Click on the page to add a {tool}, or drag to set its size",
                    &[("tool", &crate::i18n::in_sentence(tl!(tool.label())))],
                );
            }
            "sign.fill.signature.remove" => self.signature = None,
            "sign.fill.initials.remove" => self.initials = None,
            "sign.fill.signature.change" | "sign.fill.initials.change" => {
                let initials = id == "sign.fill.initials.change";
                let editing = if initials { self.initials.is_some() } else { self.signature.is_some() };
                crate::dialogs::signature::open(self, initials, editing);
            }
            fill if crate::fill_sign::FillTool::from_command(fill).is_some() => {
                let Some(tool) = crate::fill_sign::FillTool::from_command(fill) else { return Some(false) };
                self.fill_tool = tool;
                self.quick_tool = crate::QuickTool::Fill(tool);
                let initials = tool == crate::fill_sign::FillTool::Initials;
                if (tool == crate::fill_sign::FillTool::Signature && self.signature.is_none()) || (initials && self.initials.is_none()) {
                    crate::dialogs::signature::open(self, initials, false);
                }
            }
            _ => return None,
        }
        Some(true)
    }

    fn run_secure_command(&mut self, ctx: &egui::Context, id: &str, active: Option<usize>) -> Option<bool> {
        match id {
            "a11y.check" => self.start_accessibility_check(ctx),
            "a11y.find_untagged" => self.find_untagged_content(),
            // PLAN: assumed `js_dialogs::{open_console, open_document_scripts}(&mut PdfCraftApp)` at `crate::js_dialogs` (pdf-forms-w17); the latter starts a fresh draft.
            "tools.js_console" => crate::js_dialogs::open_console(self),
            "tools.document_js" => crate::js_dialogs::open_document_scripts(self),
            "a11y.report" => self.show_accessibility_report(),
            "a11y.autotag" => {
                if self.apply_edit(ctx, pdfcraft_engine::Edit::AutoTagText) {
                    self.choose_right_panel(Some(RightPanel::Tags));
                }
            }
            "a11y.alt_text" => crate::a11y_ui::open_alt_text(self, ctx),
            "a11y.autotag_fields" => self.autotag_form_fields(ctx),
            "a11y.tags" => self.choose_right_panel(Some(RightPanel::Tags)),
            "a11y.reading_options" => self.open_properties(ctx, PropsTab::Advanced),
            "redact.mark" => {
                self.quick_tool = crate::QuickTool::Redact;
                self.left = crate::LeftPanel::Tool("redact");
                self.reveal_tools();
                // A text selection made first is marked right away, on every page it covers, in
                // one undo step.
                if let Some(i) = active
                    && let Some(doc) = self.session.get(self.views[i].id)
                {
                    let author = self.comment_prefs.author.clone();
                    // Pages still to be read are marked when they are, by the Redact tool now
                    // picked; a page that could not be read refuses the whole selection.
                    match self.views[i].tool_marks(crate::QuickTool::Redact, true, None, true, &doc.info) {
                        crate::canvas::ToolMarks::Marks(marks, head) => {
                            let edits: Vec<Edit> = marks.into_iter().map(|(page, quads)| self.redact_prefs.mark(page, quads, &author)).collect();
                            if let Some(edit) = self.views[i].selection_marks_edit(edits, head) {
                                self.views[i].clear_selection();
                                self.apply_edit(ctx, edit);
                            }
                        }
                        crate::canvas::ToolMarks::Unreadable => {
                            self.notify_tr("Part of the selected text is on a page whose text could not be read, so nothing was marked for redaction. Mark that page with a box instead.");
                        }
                        crate::canvas::ToolMarks::None => {}
                    }
                }
            }
            "redact.pages" => crate::redact_ui::dialogs::open_pages(self, ctx),
            "redact.search" => {
                self.redact_search.found = None;
                self.redact_search_open = true;
            }
            "redact.properties" => self.redact_props = Some(self.redact_prefs.clone()),
            "redact.apply" => {
                let marks = active.and_then(|i| self.session.get(self.views[i].id)).map_or(0, |d| d.redaction_marks());
                if marks == 0 {
                    self.notify_tr("There are no redaction marks to apply");
                } else {
                    crate::redact_ui::dialogs::open_apply(self);
                }
            }
            "redact.sanitize" => crate::redact_ui::dialogs::open_sanitize(self),
            "redact.clear" => {
                self.apply_edit(ctx, Edit::ClearRedactions);
            }
            _ => return None,
        }
        Some(true)
    }

    /// Shows the next tab (or the previous one) in the strip's order, wrapping around at either
    /// end (#1542). The Combine files tab, when open, comes after the documents. From Home the
    /// next tab is the first and the previous one the last.
    fn step_tab(&mut self, forward: bool) {
        // The tabs of the strip being worked in: split view's second pane steps through its own.
        let place = self.active.and_then(|i| self.place_of(i)).unwrap_or_default();
        let tabs = self.tabs_in(place);
        let count = tabs.len() + usize::from(place == crate::panes::Place::Main && self.combine_tab.open);
        let current = if self.combine_showing() { Some(tabs.len()) } else { self.active.and_then(|a| tabs.iter().position(|&i| i == a)) };
        let Some(next) = tab_after(current, count, forward) else { return };
        match tabs.get(next) {
            Some(&i) => self.active = Some(i),
            None => self.open_combine_tab(),
        }
    }
}

/// The tab after `current` of `count` (before it when `forward` is false), wrapping around. From
/// no tab, forward is the first and back the last. `None` when there are no tabs.
fn tab_after(current: Option<usize>, count: usize, forward: bool) -> Option<usize> {
    let last = count.checked_sub(1)?;
    Some(match (current.filter(|&i| i <= last), forward) {
        (None, true) => 0,
        (None, false) => last,
        (Some(i), true) => {
            if i == last {
                0
            } else {
                i + 1
            }
        }
        (Some(i), false) => i.checked_sub(1).unwrap_or(last),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Next and previous tab step one place and wrap around at either end; from no tab they go
    /// to the first or the last (#1542).
    #[test]
    fn tab_steps_wrap_around_and_start_from_either_end() {
        assert_eq!(tab_after(Some(0), 3, true), Some(1));
        assert_eq!(tab_after(Some(2), 3, true), Some(0), "the last wraps to the first");
        assert_eq!(tab_after(Some(1), 3, false), Some(0));
        assert_eq!(tab_after(Some(0), 3, false), Some(2), "the first wraps to the last");
        assert_eq!(tab_after(None, 3, true), Some(0));
        assert_eq!(tab_after(None, 3, false), Some(2));
        assert_eq!(tab_after(Some(0), 1, true), Some(0), "a single tab stays");
        assert_eq!(tab_after(Some(7), 3, true), Some(0), "a stale index starts over");
        assert_eq!(tab_after(None, 0, true), None);
        assert_eq!(tab_after(Some(0), 0, false), None);
    }
}
