use super::{GridDrop, GridFile, PdfCraftApp};
#[cfg(not(target_arch = "wasm32"))]
use super::{OpenRequest, open_worker};

/// Read a file dropped on a page grid (see [`GridFile`]).
fn read_grid_file((name, source): GridFile) -> Result<(String, Vec<u8>), (String, String)> {
    let bytes = match source {
        Ok(bytes) => bytes,
        Err(path) => std::fs::read(&path).map_err(|e| (name.clone(), e.to_string()))?,
    };
    Ok((name, bytes))
}

impl PdfCraftApp {
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn open_dropped(&mut self, f: egui::DroppedFileHandle, _ctx: &egui::Context) {
        let p = f.path().to_string_lossy().into_owned();
        if !p.is_empty() && f.path().is_absolute() {
            self.open_path(&p);
            return;
        }
        let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
        match f.bytes() {
            Ok(bytes) => {
                if self.ctx.is_some() && !self.run_inline {
                    let result = self.schedule_open(OpenRequest {
                        name: name.clone(),
                        path: None,
                        stamp: None,
                        input: open_worker::Input::Bytes(std::sync::Arc::new(bytes)),
                        password: None,
                        activate: true,
                        after_open: None,
                        recovered: None,
                        then: None,
                    });
                    if let Err(error) = result {
                        self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                    }
                } else if let Err(error) = self.open_bytes(&name, None, bytes) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                }
            }
            Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
        }
    }

    /// The document whose page grid is showing and may take pages, if any.
    fn grid_target(&self) -> Option<pdfcraft_engine::DocId> {
        let view = self.active.and_then(|i| self.views.get(i)).filter(|v| v.organize)?;
        (self.dialog.is_none() && self.session.get(view.id)?.allows_assembly()).then_some(view.id)
    }

    /// The document whose Pages panel is under the pointer and may take pages, if any.
    #[cfg(not(target_arch = "wasm32"))]
    fn panel_target(&self, ctx: &egui::Context) -> Option<pdfcraft_engine::DocId> {
        let view = self.active.and_then(|i| self.views.get(i))?;
        let pointer = ctx.input(|i| i.pointer.hover_pos())?;
        let over = view.pages_panel.is_some_and(|panel| panel.contains(pointer));
        (over && self.dialog.is_none() && self.session.get(view.id)?.allows_assembly()).then_some(view.id)
    }

    /// Files dropped on the Pages panel, or while the page grid shows, are inserted into the
    /// document rather than opened: they are kept until [`Self::finish_grid_drop`] knows the
    /// gap. Returns the files not taken.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn drop_on_grid(&mut self, dropped: Vec<egui::DroppedFileHandle>, ctx: &egui::Context) -> Vec<egui::DroppedFileHandle> {
        if dropped.is_empty() {
            return dropped;
        }
        // The panel first: the grid takes files dropped anywhere in the window.
        let (id, panel) = match (self.panel_target(ctx), self.grid_target()) {
            (Some(id), _) => (id, true),
            (None, Some(id)) => (id, false),
            (None, None) => return dropped,
        };
        let mut files = Vec::new();
        for f in &dropped {
            let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
            if f.path().is_absolute() {
                files.push((name, Err(f.path().to_path_buf())));
                continue;
            }
            match f.bytes() {
                Ok(b) => files.push((name, Ok(b))),
                Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e)]),
            }
        }
        if !files.is_empty() {
            // Where there is no telling where the pointer is during the drag, its place arrives
            // with the first move after the drop.
            ctx.request_repaint();
            self.grid_drop = Some(GridDrop { doc: id, panel, files, deadline: web_time::Instant::now() + std::time::Duration::from_secs(1) });
        }
        Vec::new()
    }

    /// Insert files dropped on the page grid or the Pages panel at the gap under the pointer,
    /// or at the end when the pointer hasn't shown up in time.
    pub(super) fn finish_grid_drop(&mut self, ctx: &egui::Context) {
        let Some(GridDrop { doc: id, panel, deadline, .. }) = &self.grid_drop else { return };
        let (id, panel, deadline) = (*id, *panel, *deadline);
        let still = if panel {
            self.dialog.is_none()
                && self.active_ids().map(|(_, active)| active) == Some(id)
                && self.session.get(id).is_some_and(|d| d.allows_assembly())
        } else {
            self.grid_target() == Some(id)
        };
        if !still {
            self.grid_drop = None;
            return self.notify_tr("The document changed while you were choosing a file, so nothing was changed.");
        }
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
        let gap = match if panel { view.panel_gap } else { view.grid_gap } {
            Some(gap) => gap,
            None if web_time::Instant::now() >= deadline => usize::MAX,
            None => return ctx.request_repaint(),
        };
        view.insert_at = Some(gap);
        if let Some(drop) = self.grid_drop.take() {
            self.insert_each(drop.files.into_iter().map(read_grid_file));
        }
    }

    /// Browsers read dropped files asynchronously; the bytes land in `inbox` and open next frame.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn open_dropped(&mut self, f: egui::DroppedFileHandle, ctx: &egui::Context) {
        self.startup_superseded = true;
        let inbox = self.inbox.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
            if let Ok(bytes) = f.bytes_async().await
                && let Ok(mut q) = inbox.lock()
            {
                q.push((name, bytes));
                ctx.request_repaint();
            }
        });
    }
}
