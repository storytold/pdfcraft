use super::{DocId, LeftPanel, Mode, PdfCraftApp, QuickTool, RightPanel, edit_text_ui};

impl PdfCraftApp {
    /// Run a catalogue command. Commands that aren't implemented yet say which milestone ships them.
    pub fn run_command(&mut self, ctx: &egui::Context, command: &str) {
        if let Some(spec) = pdfcraft_engine::commands::command(command) {
            use pdfcraft_engine::commands::Needs;
            // A tool that needs a document, chosen with none open, asks for the file and then
            // runs on it, instead of only saying to open one first.
            let needs_document = !matches!(spec.needs, Needs::Nothing | Needs::Undo | Needs::Redo);
            let is_tool = pdfcraft_engine::catalog::TOOL_GROUPS
                .iter()
                .flat_map(|g| g.sections.iter().flat_map(|s| s.items.iter()))
                .any(|i| i.command == spec.id);
            if self.active.is_none() && needs_document && is_tool && !self.command_enabled(spec) {
                self.open_then_run(spec);
                return;
            }
            self.execute(ctx, command);
            return;
        }
        // Not implemented yet: say which milestone ships it.
        let when = pdfcraft_engine::catalog::TOOL_GROUPS
            .iter()
            .flat_map(|g| g.sections.iter().flat_map(|s| s.items.iter()))
            .find(|i| i.command == command)
            .map(|i| match i.availability {
                pdfcraft_engine::catalog::Availability::Planned(m) => crate::i18n::fmt(tl!("ships in milestone {m}"), &[("m", m)]),
                pdfcraft_engine::catalog::Availability::Provider => tl!("needs an AI provider (off by default)").to_string(),
                pdfcraft_engine::catalog::Availability::Ready => tl!("is available").to_string(),
            })
            .unwrap_or_else(|| tl!("is not available yet").to_string());
        self.notify_fmt("`{command}` {when}", &[("command", command), ("when", &when)]);
    }

    /// Ask for a file to open, then run `spec` on it once it is open (or unlocked). Nothing runs
    /// when the picker is cancelled or the file doesn't open.
    fn open_then_run(&mut self, spec: &'static pdfcraft_engine::commands::CommandSpec) {
        self.startup_superseded = true;
        let request = craft::host::PickRequest::open("open").title(tl!(spec.label).trim_end_matches('…'));
        let command = spec.id;
        self.pick_file(request, None, move |app, ctx, picked| app.open_picked_then(ctx, picked, command));
    }

    pub(crate) fn reveal_tools(&mut self) {
        self.set_left_open(true);
    }

    /// Enter a tool from the catalogue, Home, palette or control channel. Organize is a
    /// workspace, so opening its panel also opens the page grid (without toggling it off).
    pub(crate) fn select_tool(&mut self, ctx: &egui::Context, id: &'static str) {
        // The tool's panel shows, not the expanded All tools view over it.
        self.all_tools_full = false;
        // Edit a PDF turns on Edit text & images at once, as the Edit tab does (`enter_mode`), so
        // the page is editable without a second click. Leaving it for another tool from All tools
        // puts editing down again; under the Edit tab the workspace keeps it, as before. Read mode
        // hides the quick toolbar and keeps every tool down (`enter_mode`), so the control
        // channel's `tool edit` there leaves editing off.
        if id == "edit" {
            if self.mode != Mode::Read {
                self.quick_tool = QuickTool::EditText;
            }
        } else if self.mode != Mode::Edit
            && self.left == LeftPanel::Tool("edit")
            && matches!(self.quick_tool, QuickTool::EditText | QuickTool::AddText)
        {
            self.put_down_edit_text(ctx);
        }
        self.left = LeftPanel::Tool(id);
        self.reveal_tools();
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) {
            if id == "organize" {
                view.open_organize();
            } else {
                view.organize = false;
            }
        }
    }

    /// Select the workspace and its matching tool panel, just like the mode bar. The tool goes
    /// with the workspace as [`Self::enter_mode`] sets it.
    pub(crate) fn select_mode(&mut self, ctx: &egui::Context, mode: Mode) {
        self.enter_mode(ctx, mode);
        self.reveal_tools();
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) {
            view.organize = false;
        }
        self.left = match mode {
            Mode::Edit => LeftPanel::Tool("edit"),
            Mode::Convert => LeftPanel::Tool("export"),
            Mode::Sign => LeftPanel::Tool("fill_sign"),
            _ => LeftPanel::AllTools,
        };
    }

    /// Changes the workspace and the tool that goes with it. The mode bar (through
    /// [`Self::select_mode`]), the Read mode command and the tool panels' back arrows all come
    /// here, so they leave Edit alike. Edit turns on Edit text & images at once, as Acrobat does:
    /// every paragraph and image on the page gets its box and is editable with one click. Leaving
    /// Edit puts the editing tools down again and applies a paragraph still open in the editor.
    pub(crate) fn enter_mode(&mut self, ctx: &egui::Context, mode: Mode) {
        self.mode = mode;
        // Another workspace puts the expanded All tools view away.
        if mode != Mode::AllTools {
            self.all_tools_full = false;
        }
        if mode == Mode::Edit {
            self.quick_tool = QuickTool::EditText;
        } else if matches!(self.quick_tool, QuickTool::EditText | QuickTool::AddText) {
            self.put_down_edit_text(ctx);
        }
        // Read mode hides the quick toolbar, so no other tool may stay active there. Opening a
        // document with Read as the default mode comes here without `switch_mode`.
        if mode == Mode::Read {
            self.quick_tool = QuickTool::Select;
        }
    }

    /// Fill the workspace with the expanded All tools view, entering All tools the way the mode
    /// bar does: leaving Read restores its right panel and leaving Edit puts the editing tool down.
    pub(crate) fn expand_all_tools(&mut self, ctx: &egui::Context) {
        if self.mode != Mode::AllTools {
            self.switch_mode(ctx, Mode::AllTools, true);
        }
        self.all_tools_full = true;
        self.all_tools_tab = self.shown_tab();
    }

    /// The tab at the front: the active document, or else whether Combine files shows over Home.
    pub(super) fn shown_tab(&self) -> (Option<DocId>, bool) {
        (self.active.and_then(|i| self.views.get(i)).map(|v| v.id), self.combine_showing())
    }

    /// Back to the Select tool from an editing tool. A paragraph still open in the editor is
    /// applied, as clicking away from it does; left open, its page would keep showing the
    /// paragraph's text removed with no editor over it.
    pub(crate) fn put_down_edit_text(&mut self, ctx: &egui::Context) {
        self.quick_tool = QuickTool::Select;
        if let Some(ed) = self.active.and_then(|i| self.views.get_mut(i)).and_then(|v| v.session.line_editor.take())
            && let Some(edit) = edit_text_ui::finished_edit(ed)
        {
            self.apply_edit(ctx, edit);
        }
    }

    /// Switches workspace because the user chose to (mode bar, Read mode command). Entering Read
    /// mode closes the right panel, as Acrobat's Read mode hides its panes, and leaving it reopens
    /// that panel. The rail stays in Read mode, and its buttons open panels there too (#932).
    /// Read mode hides the quick toolbar, so entering it also puts the current tool down.
    pub(crate) fn switch_mode(&mut self, ctx: &egui::Context, mode: Mode, select_tools: bool) {
        let was_read = self.mode == Mode::Read;
        if select_tools {
            self.select_mode(ctx, mode);
        } else {
            self.enter_mode(ctx, mode);
        }
        if mode == Mode::Read && !was_read {
            self.right_before_read = self.right();
            self.set_right(None);
            // An editing tool is put down so its open paragraph is applied, as `select_mode` does.
            if matches!(self.quick_tool, QuickTool::EditText | QuickTool::AddText) {
                self.put_down_edit_text(ctx);
            } else {
                self.quick_tool = QuickTool::Select;
            }
        } else if mode != Mode::Read && was_read {
            let before = self.right_before_read.take();
            if self.right().is_none() {
                self.set_right(before);
            }
        }
    }

    /// Opens a right panel, or closes it with `None`, because the user chose to. Closing Comments
    /// keeps comment tools from reopening it; opening it again lets them (#225). A choice made in
    /// Read mode replaces the pane saved on the way in, so leaving Read mode doesn't undo it.
    pub fn choose_right_panel(&mut self, panel: Option<RightPanel>) {
        self.right_panel_chosen(self.right(), panel);
        self.set_right(panel);
        if let Some(panel) = panel {
            self.last_right = panel;
        }
    }

    /// What the user's choice of the right panel, from `before` to `panel`, is remembered as: here
    /// or in the dock (its tabs and Close button).
    pub(crate) fn right_panel_chosen(&mut self, before: Option<RightPanel>, panel: Option<RightPanel>) {
        if self.mode == Mode::Read {
            self.right_before_read = None;
        }
        if panel == Some(RightPanel::Comments) {
            self.comments_panel_closed = false;
        } else if panel.is_none() && before == Some(RightPanel::Comments) {
            self.comments_panel_closed = true;
        }
        if panel.is_some() {
            self.right_panel_closed = false;
        } else if before.is_some() {
            self.right_panel_closed = true;
        }
    }

    /// F4: closes the side panel, or reopens the one open last.
    pub(crate) fn toggle_side_panel(&mut self) {
        self.choose_right_panel(if self.right().is_some() { None } else { Some(self.last_right) });
    }
}
