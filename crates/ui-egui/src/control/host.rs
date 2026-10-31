use super::{Host, Reply, Value, json};

impl Host for crate::PdfCraftApp {
    /// Cancel any in-progress pointer gesture in every open view.
    fn cancel_pointer_gestures(&mut self) {
        for view in &mut self.views {
            view.cancel_pointer_gestures();
        }
    }

    /// The input target: active document, open dialog/prompt, palette and find state.
    ///
    /// Synthetic input admitted for one generation is never replayed into another document or
    /// dialog. Only these identities are hashed, never text or document content.
    fn input_epoch(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.capture_epoch().hash(&mut hasher);
        // The focused widget is part of the input target: keys typed for one field must not land
        // in another after focus moves. Only the id value is hashed, never text.
        let focused = self.ctx.as_ref().and_then(|ctx| ctx.memory(|m| m.focused())).map(|id| id.value());
        focused.hash(&mut hasher);
        hasher.finish()
    }

    /// The captured content: active document and open dialog/prompt, never keyboard focus.
    fn capture_epoch(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let doc = self.active.and_then(|i| self.views.get(i)).map(|v| v.id.0);
        doc.hash(&mut hasher);
        format!(
            "{:?}/{:?}/{:?}/{:?}/{:?}",
            self.dialog,
            self.password_prompt.is_some(),
            self.palette_open,
            self.close_request.is_some(),
            self.active.and_then(|i| self.views.get(i)).and_then(|v| v.find.as_ref().map(|_| true)),
        )
        .hash(&mut hasher);
        hasher.finish()
    }

    /// Control requests wait for the first UI pass that installs fonts; a font-gated early
    /// return cannot consume injected input or issue a screenshot.
    fn accepts_input(&self) -> bool {
        self.fonts_ready
    }

    fn panel(&mut self, reset: bool, params: &Value) -> Reply {
        crate::side_panels::command(self, reset, params)
    }

    fn quit(&mut self, ctx: &egui::Context) -> bool {
        self.request_quit(ctx)
    }

    fn state(&self) -> Value {
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
        json!({
            "side_panels": crate::side_panels::state(self),
            "documents": docs,
            "pending_opens": pending_opens,
            "active": active.map(|v| json!({
                "doc": v.id.0,
                "page": v.current + 1,
                "zoom_percent": (v.zoom * 100.0).round(),
                "fit": format!("{:?}", v.fit),
                "layout": format!("{:?}", v.layout),
                "continuous": v.continuous(),
                "page_gaps": v.show_page_gaps,
                "organize": v.organize,
                "grid_zoom": v.grid_zoom(),
                "thumbnail_zoom": v.thumbnail_zoom(),
                // Pages picked in the organize grid or the Pages panel (empty: the current page).
                "selected_pages": v.selected.iter().map(|p| p + 1).collect::<Vec<_>>(),
                "auto_scrolling": v.auto_scrolling(),
                "raster_memory": v.raster_memory(),
                "viewport": [v.viewport_rect().min.x, v.viewport_rect().min.y, v.viewport_rect().max.x, v.viewport_rect().max.y],
                "find_open": v.find.is_some(),
                "page_errors": v.page_errors().iter().map(|(p, e)| json!({ "page": p + 1, "error": e })).collect::<Vec<_>>(),
                // Where pages are on screen (points), to aim ui.click / ui.drag at page content.
                "pages_on_screen": v.visible_page_rects().iter().map(|(p, r)| json!({ "page": p + 1, "rect": [r.min.x, r.min.y, r.max.x, r.max.y] })).collect::<Vec<_>>(),
                "selected_comment": v.comments.selected.map(|(p, i)| json!({ "page": p + 1, "index": i + 1 })),
                "comment_composer_open": v.comments.composer.is_some(),
            })),
            // The Combine files tab: how its files show, and each grid thumbnail's state.
            "combine": {
                "showing": self.combine_showing(),
                "view": self.combine_view.as_str(),
                "zoom": self.combine_zoom,
                "files": self.combine_draft.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
                "selected": self.combine_selection(),
                "thumbnails": self.combine_thumbnails().iter().map(|t| format!("{t:?}")).collect::<Vec<_>>(),
                "expanded": self.combine_expanded().iter().filter_map(|i| self.combine_draft.get(*i).map(|f| f.name.clone())).collect::<Vec<_>>(),
                "preview": self.combine_preview().map(|(file, page, render)| json!({ "file": file, "page": page + 1, "render": format!("{render:?}") })),
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
            "comment_tool_pinned": self.comment_prefs.pinned,
            "left_panel": if self.left_open { json!(format!("{:?}", self.left)) } else { Value::Null },
            "tools_side": if self.side_panels.tools_on_right() { "right" } else { "left" },
            "right_panel": self.right.map(|r| format!("{r:?}")),
            "dialog": self.dialog.map(|d| format!("{d:?}")),
            "palette_open": self.palette_open,
            "theme": format!("{:?}", self.theme),
            "theme_preference": self.theme_preference,
            "language": self.language,
            "notice": self.toast.as_ref().map(|t| t.0.clone()),
            "progress": self.progress_notice.as_ref().map(|p| json!({ "label": p.label, "fraction": p.fraction })),
            "password_prompt": self.password_prompt.is_some(),
            "close_prompt": self.close_request.is_some(),
        })
    }

    fn command(&mut self, id: &str) -> Reply {
        let spec = pdfcraft_engine::commands::command(id).ok_or_else(|| format!("unknown command {id:?} (see ui.commands)"))?;
        if !self.command_enabled(spec) {
            return Err(format!("{id} is disabled right now"));
        }
        self.execute(id);
        Ok(json!({ "ran": id }))
    }

    fn commands(&self) -> Value {
        let mut list: Vec<Value> = pdfcraft_engine::commands::COMMANDS
            .iter()
            .map(|c| json!({ "id": c.id, "label": c.label, "menu": c.menu, "enabled": self.command_enabled(c), "shortcut": c.shortcut.map(|s| s.label(cfg!(target_os = "macos"))) }))
            .collect();
        list.push(json!({"id": "ui.panel", "label": "Side panel", "enabled": true, "params": "panel: tools or side; open: boolean; width: number (clamped to the panel's limits)"}));
        json!({ "commands": list })
    }

    fn set(&mut self, key: &str, value: &str) -> Reply {
        self.set_option(key, value).map(|()| json!({ "set": key, "value": value }))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open(&mut self, path: &str) -> Reply {
        match self.open_path(path) {
            crate::OpenOutcome::Opened => Ok(json!({ "opened": path, "password_prompt": self.password_prompt.is_some() })),
            crate::OpenOutcome::Focused => Ok(json!({ "opened": path, "focused": true })),
            crate::OpenOutcome::Pending => Ok(json!({ "accepted": path, "pending": true })),
            crate::OpenOutcome::Failed => Err(self.toast.as_ref().map(|t| t.0.clone()).unwrap_or_else(|| format!("couldn't open {path}"))),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn open(&mut self, _path: &str) -> Reply {
        Err("ui.open needs a file system; on the web, drop the file on the page".into())
    }
}
