use craft::ui::runtime::drag::OsFile; // PLAN: assumed path of `OsFile` in the facade (ui-contract `runtime/drag.rs`)

use super::PdfCraftApp;
use craft::platform::open::{DroppedFile, Intake};

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

fn os_file(file: DroppedFile) -> OsFile {
    OsFile { name: file.name, path: file.path, mime: String::new(), bytes: file.bytes }
}

impl PdfCraftApp {
    /// Opens what the file intake holds, batch by batch in arrival order: another launch's paths
    /// like File ▸ Open, its `combine` files in the Combine files tab, bytes as a new tab, drops
    /// as [`Self::open_dropped`]. A batch from the OS brings the window to the front.
    pub(crate) fn open_intake(&mut self, ctx: &egui::Context) {
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
                            self.combine_paths(&paths);
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
                Intake::Bytes { name, bytes } => {
                    let name = if name.is_empty() { DROPPED_NAME.to_string() } else { name };
                    if let Err(error) = self.open_bytes(&name, None, bytes.to_vec()) {
                        self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
                    }
                }
                Intake::Dropped { files, .. } => self.open_dropped(files.into_iter().map(os_file).collect()),
            }
        }
    }

    /// Opens the files dropped on the window: a file with a path like File ▸ Open, one that
    /// arrived as bytes (the web) as a new tab.
    pub(crate) fn open_dropped(&mut self, files: Vec<OsFile>) {
        for file in files {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(path) = &file.path {
                self.open_path(&path.to_string_lossy());
                continue;
            }
            let name = if file.name.is_empty() { DROPPED_NAME.to_string() } else { file.name.clone() };
            let Some(bytes) = file.bytes else {
                self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", "the file has no contents")]);
                continue;
            };
            if let Err(error) = self.open_bytes(&name, None, bytes.to_vec()) {
                self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &error)]);
            }
        }
    }

    /// Inserts the pages of the dropped `files` into the active document before page `at`, and
    /// selects them. Each file is read here (its bytes, else its path); one that can't be read is
    /// named and skipped by [`Self::insert_each`], the rest are inserted.
    pub(crate) fn insert_dropped(&mut self, files: Vec<OsFile>, at: usize) {
        let read: Vec<Result<(String, Vec<u8>), (String, String)>> = files
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
        self.insert_each(read);
    }
}
