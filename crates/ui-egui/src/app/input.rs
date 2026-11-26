use craft::ui::runtime::drag::OsFile; // PLAN: assumed path of `OsFile` in the facade (ui-contract `runtime/drag.rs`)

use super::PdfCraftApp;
use craft::platform::open::Intake;

/// The name a dropped file is opened under when the OS gave none.
const DROPPED_NAME: &str = "dropped.pdf";

/// The action a `--combine` launch sends with its files (`single_instance::deliver`).
#[cfg(not(target_arch = "wasm32"))]
const COMBINE_ACTION: &str = "combine";

/// Raises the window for an open request from the OS.
fn bring_to_front(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}

impl PdfCraftApp {
    /// Opens what the file intake holds, batch by batch in arrival order: another launch's paths
    /// like File ▸ Open, its `combine` files in the Combine files tab, bytes as a new tab (see
    /// [`Self::open_startup_files`]), drops as [`Self::open_dropped`]. A batch from the OS brings
    /// the window to the front.
    pub(crate) fn open_intake(&mut self, ctx: &egui::Context) {
        // Work the user has started holds even after its tab closes or Home is selected.
        self.startup_superseded |= !self.views.is_empty() || self.combine_showing() || self.modal_open();
        for batch in self.intake.take() {
            match batch {
                Intake::Open(paths) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    for path in &paths {
                        self.open_path(&path.to_string_lossy());
                    }
                    #[cfg(target_arch = "wasm32")]
                    drop(paths);
                    bring_to_front(ctx);
                }
                Intake::OpenFor { paths, action } => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        let paths: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                        if action == COMBINE_ACTION {
                            self.combine_paths(ctx, &paths);
                        } else {
                            for path in &paths {
                                self.open_path(path);
                            }
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    drop((paths, action));
                    bring_to_front(ctx);
                }
                Intake::Quit => {
                    self.request_quit(ctx);
                }
                Intake::Bytes { name, bytes } => {
                    let name = if name.is_empty() { DROPPED_NAME.to_string() } else { name };
                    self.startup_files.push((name, bytes));
                }
                Intake::Dropped { files, .. } => {
                    for file in files {
                        self.open_dropped_file(ctx, file.name, file.path, file.bytes);
                    }
                }
            }
        }
        self.open_startup_files(ctx);
    }

    /// Opens the files that arrived as bytes, once no prompt, dialog, link question or open is
    /// under way: such a file may ask for its own password, and must not take another's place. It
    /// opens in front only while the user has started nothing; after that it waits in a background
    /// tab (#171).
    fn open_startup_files(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        let opening = !self.pending_opens.is_empty();
        #[cfg(target_arch = "wasm32")]
        let opening = false;
        if self.startup_files.is_empty() || opening || self.image_import.is_some() || self.modal_open() {
            return;
        }
        for (name, bytes) in std::mem::take(&mut self.startup_files) {
            let activate = !self.startup_superseded;
            if let Err(error) = self.open_bytes_with_focus(ctx, &name, None, bytes.to_vec(), activate) {
                self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
            }
        }
    }

    /// Opens the files dropped on the window: a file with a path like File ▸ Open, one that
    /// arrived as bytes (the web) as a new tab.
    pub(crate) fn open_dropped(&mut self, ctx: &egui::Context, files: Vec<OsFile>) {
        for file in files {
            self.open_dropped_file(ctx, file.name, file.path, file.bytes);
        }
    }

    /// Opens one dropped file, from the window or from the OS's file intake: by its path when it has one, else from
    /// its bytes under `name`.
    fn open_dropped_file(&mut self, ctx: &egui::Context, name: String, path: Option<std::path::PathBuf>, bytes: Option<std::sync::Arc<[u8]>>) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = &path {
            self.open_path(&path.to_string_lossy());
            return;
        }
        #[cfg(target_arch = "wasm32")]
        drop(path);
        let name = if name.is_empty() { DROPPED_NAME.to_string() } else { name };
        let Some(bytes) = bytes else {
            self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", "the file has no contents")]);
            return;
        };
        if let Err(error) = self.open_bytes(ctx, &name, None, bytes.to_vec()) {
            self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
        }
    }

    /// Inserts the pages of the dropped `files` into the active document before page `at`, and
    /// selects them. Each file is read here (its bytes, else its path); one that can't be read is
    /// named and skipped by [`Self::insert_each`], the rest are inserted.
    pub(crate) fn insert_dropped(&mut self, ctx: &egui::Context, files: Vec<OsFile>, at: usize) {
        let read: Vec<DroppedRead> = files
            .into_iter()
            .map(|file| {
                let name = if file.name.is_empty() { DROPPED_NAME.to_string() } else { file.name.clone() };
                let bytes = match (&file.bytes, &file.path) {
                    (Some(bytes), _) => Ok(bytes.to_vec()),
                    (None, Some(path)) => std::fs::read(path).map_err(|e| e.to_string()),
                    (None, None) => Err("the file has no contents".to_string()),
                };
                bytes.map(|bytes| (name.clone(), bytes)).map_err(|error| (name, error))
            })
            .collect();
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
        view.insert_at = Some(at);
        self.insert_each(ctx, read);
    }
}

/// A dropped file read: its name and bytes, or its name and why it couldn't be read.
type DroppedRead = Result<(String, Vec<u8>), (String, String)>;
