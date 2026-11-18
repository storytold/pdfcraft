use craft::ui::runtime::drag::OsFile; // PLAN: assumed path of `OsFile` in the facade (ui-contract `runtime/drag.rs`)

use super::PdfCraftApp;

/// The name a dropped file is opened under when the OS gave none.
const DROPPED_NAME: &str = "dropped.pdf";

impl PdfCraftApp {
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
