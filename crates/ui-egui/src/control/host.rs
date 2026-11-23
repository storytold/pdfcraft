use serde_json::json;

use super::Value;

/// The side of the Combine thumbnails `ui.state` reports the state of, in points.
const STATE_THUMB_SIDE: f32 = 160.0;

impl crate::PdfCraftApp {
    /// The captured content and the target of synthetic input: active document and open
    /// dialog/prompt, never keyboard focus (the framework withdraws injected input itself when the
    /// focus moves).
    pub(crate) fn capture_epoch(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let doc = self.active.and_then(|i| self.views.get(i)).map(|v| v.id.0);
        doc.hash(&mut hasher);
        format!(
            "{:?}/{:?}/{:?}/{:?}/{:?}/{:?}",
            self.dialogs.open_dialogs().iter().map(|d| (d.id, d.key.as_deref())).collect::<Vec<_>>(),
            (self.preferences.is_some(), self.shortcuts.is_some()),
            self.modal_open(),
            self.palette_open,
            self.close_guard.is_some(),
            self.active.and_then(|i| self.views.get(i)).and_then(|v| v.find.as_ref().map(|_| true)),
        )
        .hash(&mut hasher);
        hasher.finish()
    }

    /// `ui.panel`: the dock's panel request.
    pub(crate) fn panel(&mut self, reset: bool, params: &Value) -> Result<Value, String> {
        crate::side_panels::command(self, reset, params)
    }

    /// `ui.state`: what an agent needs to aim its next request.
    pub(crate) fn state(&self, ctx: &egui::Context) -> Value {
        let active = self.active.and_then(|i| self.views.get(i));
        let docs: Vec<Value> = self
            .views
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let d = self.session.get(v.id)?;
                Some(json!({ "tab": i, "doc": v.id.0, "name": d.name, "path": d.path, "pages": d.info.pages.len(), "dirty": d.dirty, "active": self.active == Some(i) }))
            })
            .collect();
        #[cfg(not(target_arch = "wasm32"))]
        let pending_opens: Vec<Value> =
            self.pending_opens.iter().map(|pending| json!({ "token": pending.token, "name": pending.name, "path": pending.path })).collect();
        #[cfg(target_arch = "wasm32")]
        let pending_opens: Vec<Value> = Vec::new();
        let cards = craft::ui::runtime::toast::cards(ctx, &craft::ui::runtime::env::WindowKey::main());
        let notice = cards.last().map(|card| card.toast.message.as_str().to_string());
        let progress =
            cards.iter().rev().find_map(|card| card.toast.progress.map(|f| json!({ "label": card.toast.message.as_str(), "fraction": f })));
        let dock = crate::side_panels::state(self);
        let thumb_request = craft::ui::primitives::textures::ThumbRequest::for_cell(STATE_THUMB_SIDE, ctx.pixels_per_point());
        json!({
            "side_panels": dock.clone(),
            "documents": docs,
            "pending_opens": pending_opens,
            "active": active.map(|v| json!({
                "doc": v.id.0,
                "page": v.current() + 1,
                "zoom_percent": (v.view.zoom * 100.0).round(),
                "fit": format!("{:?}", v.view.fit),
                "layout": format!("{:?}", v.view.display),
                "continuous": v.view.continuous,
                "page_gaps": v.view.page_gaps,
                "organize": v.organize,
                "grid_zoom": self.organize_zoom,
                "thumbnail_zoom": v.thumbnail_zoom(),
                // Pages picked in the organize grid or the Pages panel (empty: the current page).
                "selected_pages": v.selected.items.iter().map(|p| p + 1).collect::<Vec<_>>(),
                "auto_scrolling": v.auto_scrolling(),
                // The page store's texture counts as the last pass drew the view.
                "raster_memory": json!({
                    "entries": v.textures.entries,
                    "bytes": v.textures.bytes,
                    "uploads": v.textures.uploads,
                    "upload_bytes": v.textures.upload_bytes,
                    "evicted": v.textures.evicted,
                }),
                "viewport": [v.viewport_rect().min.x, v.viewport_rect().min.y, v.viewport_rect().max.x, v.viewport_rect().max.y],
                "find_open": v.find.is_some(),
                "page_errors": v.page_errors().iter().map(|(p, e)| json!({ "page": p + 1, "error": e })).collect::<Vec<_>>(),
                // Pages cut at the renderer's limits (#623), 1-based.
                "cut_pages": v.cut_pages().iter().map(|p| p + 1).collect::<Vec<_>>(),
                // Where pages are on screen (points), to aim ui.click / ui.drag at page content.
                "pages_on_screen": v.visible_page_rects().iter().map(|(p, r)| json!({ "page": p + 1, "rect": [r.min.x, r.min.y, r.max.x, r.max.y] })).collect::<Vec<_>>(),
                "selected_comment": v.session.comments.primary().map(|(p, i)| json!({ "page": p + 1, "index": i + 1 })),
                // Every selected comment (⌘-click adds), the one with the handles first.
                "selected_comments": v.session.comments.selection().iter().map(|(p, i)| json!({ "page": p + 1, "index": i + 1 })).collect::<Vec<_>>(),
                "comment_composer_open": v.session.comments.composer.is_some(),
            })),
            // The Combine files tab: how its files show, and each grid thumbnail's state.
            "combine": {
                "showing": self.combine_showing(),
                "view": self.combine_view.as_str(),
                "zoom": self.combine_zoom,
                "files": self.combine_draft.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
                "selected": self.combine_selection(),
                "thumbnails": self.combine_thumbnails(ctx, &thumb_request).iter().map(|t| format!("{t:?}")).collect::<Vec<_>>(),
                "expanded": self.combine_expanded().iter().filter_map(|i| self.combine_draft.get(*i).map(|f| f.name.clone())).collect::<Vec<_>>(),
                "preview": self.combine_preview().map(|(file, page)| json!({ "file": file, "page": page + 1 })),
            },
            "quick_tool": match self.quick_tool {
                crate::QuickTool::Measure(t) => format!("measure-{}", t.name()),
                crate::QuickTool::Select => "select".to_string(),
                crate::QuickTool::Hand => "hand".to_string(),
                crate::QuickTool::Crop => "crop".to_string(),
                crate::QuickTool::EnhanceCamera => "enhance-camera".to_string(),
                crate::QuickTool::Redact => "redact".to_string(),
                crate::QuickTool::AddText => "add-text".to_string(),
                crate::QuickTool::EditText => "edit-text".to_string(),
                crate::QuickTool::Link => "link".to_string(),
                crate::QuickTool::SignArea { certify: false } => "sign".to_string(),
                crate::QuickTool::SignArea { certify: true } => "certify".to_string(),
                crate::QuickTool::MarqueeZoom => "marquee-zoom".to_string(),
                crate::QuickTool::Snapshot => "snapshot".to_string(),
                crate::QuickTool::ColumnSelect => "column-select".to_string(),
                crate::QuickTool::Stamp(k) => format!("stamp-{}", k.name().trim_start_matches("PC").to_ascii_lowercase()),
                crate::QuickTool::CustomStamp(i) => format!("custom-stamp-{i}"),
                crate::QuickTool::Fill(f) => format!("fill-{}", f.command().trim_start_matches("sign.fill.")),
                crate::QuickTool::Field(f) => format!("field-{}", f.command().trim_start_matches("form.add.")),
                crate::QuickTool::Comment(t) => t.command().trim_start_matches("comment.").to_string(),
            },
            "home": self.active.is_none(),
            "mode": format!("{:?}", self.mode),
            "default_mode": self.default_mode,
            "hotbar": { "tools": self.hotbar.tools },
            "comment_tool_pinned": self.comment_prefs.pinned,
            // Home's pinned folders, in order, and whether each is folded down to its heading.
            "pinned_folders": self.pinned.folders.iter().map(|f| json!({ "folder": f, "collapsed": self.pinned.collapsed.contains(f) })).collect::<Vec<_>>(),
            "left_panel": if self.left_open() { json!(format!("{:?}", self.left)) } else { Value::Null },
            "tools_side": dock["tools_side"].clone(),
            "rail_side": dock["rail_side"].clone(),
            "right_panel": self.right().map(|r| format!("{r:?}")),
            "hidden_interface": self.hidden_interface().to_json(),
            // The top host dialog's key, else the inline window that is open.
            "dialog": self
                .dialogs
                .open_dialogs()
                .last()
                .and_then(|d| d.key.clone())
                .or_else(|| self.preferences.is_some().then(|| "preferences".to_string()))
                .or_else(|| self.shortcuts.is_some().then(|| "shortcuts".to_string())),
            "palette_open": self.palette_open,
            "theme": format!("{:?}", self.root_env(ctx).resolved(ctx).mode),
            "theme_preference": format!("{:?}", self.theme_preference),
            "show_page_edge_shadows": self.show_page_edge_shadows,
            "language": self.language,
            "notice": notice,
            "progress": progress,
            "password_prompt": self.password_prompt.is_some(),
            "close_prompt": self.close_guard.is_some(),
        })
    }

    /// `ui.command`: runs a registered command, refused while it is disabled.
    pub(crate) fn command(&mut self, ctx: &egui::Context, id: &str) -> Result<Value, String> {
        let spec = pdfcraft_engine::commands::command(id).ok_or_else(|| format!("unknown command {id:?} (see ui.commands)"))?;
        if !self.command_enabled(spec) {
            return Err(format!("{id} is disabled right now"));
        }
        self.execute(ctx, id);
        Ok(json!({ "ran": id }))
    }

    /// `ui.commands`: every registered command, whether it can run now and its shortcut.
    pub(crate) fn commands(&self) -> Value {
        let mut list: Vec<Value> = pdfcraft_engine::commands::COMMANDS
            .iter()
            .map(|c| json!({ "id": c.id, "label": c.label, "menu": c.menu, "enabled": self.command_enabled(c), "shortcut": c.shortcut_for(cfg!(target_os = "macos")).map(|s| s.label(cfg!(target_os = "macos"))) }))
            .collect();
        list.push(json!({"id": "ui.panel", "label": "Side panel", "enabled": true, "params": "panel: tools or side; open: boolean; width: number (clamped to the panel's limits)"}));
        json!({ "commands": list })
    }

    /// `ui.set`: a named view option, as [`Self::set_option`] takes it.
    pub(crate) fn set(&mut self, ctx: &egui::Context, key: &str, value: &str) -> Result<Value, String> {
        self.set_option(ctx, key, value).map(|()| json!({ "set": key, "value": value }))
    }

    /// `ui.open`: opens a file as the user would; a failure replies with its error.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn open(&mut self, path: &str) -> Result<Value, String> {
        match self.open_path(path) {
            crate::OpenOutcome::Opened => Ok(json!({ "opened": path, "password_prompt": self.password_prompt.is_some() })),
            crate::OpenOutcome::Focused => Ok(json!({ "opened": path, "focused": true })),
            crate::OpenOutcome::Pending => Ok(json!({ "accepted": path, "pending": true })),
            crate::OpenOutcome::Failed(error) => Err(error.to_string()),
        }
    }

    /// `ui.open` on the web: there is no file system to open from.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn open(&mut self, _path: &str) -> Result<Value, String> {
        Err("ui.open needs a file system; on the web, drop the file on the page".into())
    }
}
