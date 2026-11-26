//! The UI control channel: lets an agent see and drive the running app.
//!
//! **Opt-in only.** Nothing here runs unless the app is started with `--control <file>` (or a
//! test attaches a [`ControlChannel`]). The transport is the framework's `server::listen`: loopback
//! only, a random port, and every connection must first present the random token written to `<file>`.
//!
//! The channel, the injected-input queue, the screenshot queue and the access tree belong to the
//! framework (`craft::automation`). This module is the app's method table over it: the framework's
//! methods first, then the app's own (`requests`).
//!
//! Methods (JSON in, JSON out):
//! - `ui.state`: open documents, active document, page, zoom, page errors, mode, panels, dialog,
//!   notice and observed window dimensions in logical points.
//! - `ui.inspect {query?, role?, focused?, limit?}`: widgets in tree order with `id` (a string), `role`, `label`,
//!   `value`, `rect` (points), `enabled`, `toggled`, `selected`, `clickable`, `focusable`, `focused`, `depth`, and
//!   the `dock` workspace.
//! - `ui.click {id}` | `{label}` | `{x, y, button?, count?, hold?}`: click a widget (through its declared
//!   action) or a point (`button`: primary, secondary, or middle).
//! - `ui.focus {id}` | `{label}`: focus one enabled widget through its action.
//! - `ui.move {x, y}`, `ui.release`, `ui.wheel`, `ui.key`, `ui.type`: pointer and keyboard input. A drag is
//!   `ui.click {hold: true}`, `ui.move`, `ui.release`; `ui.state` reports `pages_on_screen` to aim at.
//! - `ui.dialog.*`: read and answer the open dialogs.
//! - `ui.pick {files | folder | save | cancel}`: answer the next file pick.
//! - `ui.window {maximized}`: maximize or restore the window.
//! - `ui.dock.apply {action}`: apply one dock action; `ui.dock.reset` and `ui.panel` drive the side panels.
//! - `ui.resize {width, height}`: request a native window size (320–8192 logical points per dimension).
//!   The window manager may constrain it; `ui.state.window` reports the observed result.
//! - `ui.quit {}`: normal native exit, or `quitting: false, needs_confirmation: true` and the
//!   existing save prompt when any document has unsaved edits or typing. No discard parameter.
//! - `ui.command {id}`: run a registry command (as the menu would). `ui.commands` lists them.
//! - `ui.set {key, value}`: view options (`--page`, `--zoom`, …), plus persistent preferences.
//!   `select` accepts physical page numbers and ranges such as `1,12,16-18`; blank clears the
//!   explicit selection. `ui.state.active.selected_pages` reports it.
//!   `default-mode=all|read|edit|convert|sign` applies to future PDF opens; `mode` overrides it
//!   for this session without changing the saved preference. `ui.state.default_mode` reports it.
//!   `theme=light|dark|system` changes the saved theme preference; `ui.state.theme_preference`
//!   reports the choice and `ui.state.theme` reports the resolved light/dark colours.
//!   `show_page_edge_shadows=true|false` controls page decoration app-wide.
//!   `comment-tool-pinned=true|false` remembers the annotation quick bar's pin across restarts.
//!   `folder-collapsed=<folder>=on|off` folds a pinned folder on Home down to its heading or
//!   opens it again; `ui.state.pinned_folders` lists each pinned folder and whether it is.
//! - `ui.open {path}`: open a file. A file whose tab is already open focuses it
//!   (`"focused": true`) instead of adding a tab; native opens report `pending: true` until
//!   they complete and are listed in `ui.state.pending_opens`; only a failed open replies `Err`.
//! - `ui.screenshot {region?}`: PNG of the window (base64 in `base64`), optionally cropped to a rect.

use std::sync::atomic::{AtomicU64, Ordering};

use craft::automation::dialogs::dialog_method;
use craft::automation::screenshot::{DocumentId, ScreenshotParams, settle_ms};
use craft::automation::synthetic::input_method;
use craft::automation::{ControlRequest, Reply, access};
use craft::ui::runtime::motion;
use craft::ui::runtime::windows::WindowHost;
use egui::ViewportId;
use serde_json::Value;

use crate::PdfCraftApp;

mod host;
mod requests;

/// The token the next `ui.screenshot` is admitted under; each differs from every pending one.
static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(1);

impl PdfCraftApp {
    /// The document and dialog identity a screenshot is attributed to.
    fn capture_document(&self) -> DocumentId {
        DocumentId::new(self.capture_epoch(), 0)
    }

    /// Answers the control requests of this frame and finishes the screenshots that are due. Runs in the
    /// logic phase; the runtime and fonts are installed at creation (L-227), so every frame can answer.
    pub(crate) fn control_logic(&mut self, ctx: &egui::Context) {
        let Some(mut channel) = self.control.take() else { return };
        let host = WindowHost::of(ctx);
        let document = Some(self.capture_document());
        channel.captures.note_windows(&host, document);
        let finished = channel.captures.drain(motion::now(), document);
        channel.finish_captures(finished, &mut |_path, _png| Err("ui.screenshot writes no files; omit path".to_string()));
        channel.poll_input(&host);
        while let Some(req) = channel.next_request() {
            let reply = self.answer(ctx, &req);
            let settle = settle_ms(ctx.global_style().animation_time);
            channel.settle(req, reply, document, settle);
        }
        self.control = Some(channel);
    }

    /// Counts the pass that just ran and issues the captures admitted for it. Call at the end of every UI pass.
    pub(crate) fn control_ui_pass(&mut self, ctx: &egui::Context) {
        // A discarded pass is layout work, not a frame an agent can observe.
        if ctx.will_discard() {
            return;
        }
        let document = Some(self.capture_document());
        let Some(channel) = self.control.as_mut() else { return };
        let host = WindowHost::of(ctx);
        channel.captures.note_pass();
        let finished = channel.captures.issue(&host, motion::now(), document);
        channel.finish_captures(finished, &mut |_path, _png| Err("ui.screenshot writes no files; omit path".to_string()));
    }

    /// Withdraws the queued synthetic input when its document or dialog changed since the last pass. Runs from
    /// the raw input hook, where the framework takes the withdrawal; a focus move is the framework's own to
    /// withdraw on, and a step that moved the focus keeps the rest of its request.
    pub(crate) fn control_before_input(&mut self, ctx: &egui::Context) {
        if self.control.is_none() {
            return;
        }
        let epoch = self.capture_epoch();
        if epoch != self.control_epoch {
            self.control_epoch = epoch;
            WindowHost::of(ctx).withdraw_injected(ViewportId::ROOT);
        }
    }

    /// The reply to one request: the framework's methods, then the app's.
    fn answer(&mut self, ctx: &egui::Context, req: &ControlRequest) -> Reply {
        let (method, params) = (req.method.as_str(), &req.params);
        let host = WindowHost::of(ctx);
        if let Some(queued) = input_method(method, params, &host, ViewportId::ROOT, &|_, _| None) {
            return match queued {
                Ok(()) => {
                    self.control_epoch = self.capture_epoch();
                    Reply::AfterInput
                }
                Err(e) => Reply::from_result(Err(e)),
            };
        }
        if let Some(result) = dialog_method(&mut self.dialogs, ctx, method, params) {
            return Reply::from_result(result);
        }
        match method {
            "ui.inspect" => Reply::from_result(access::inspect(&host, ViewportId::ROOT, params).map(|mut found| {
                if let Value::Object(fields) = &mut found {
                    fields.insert("dock".into(), crate::side_panels::dock_inspect(self));
                }
                found
            })),
            "ui.screenshot" => match ScreenshotParams::from_params(params) {
                Ok(shot) => {
                    ctx.request_repaint();
                    Reply::screenshot(NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed), shot, ViewportId::ROOT)
                }
                Err(e) => Reply::from_result(Err(e)),
            },
            _ => requests::handle(self, ctx, method, params),
        }
    }
}
