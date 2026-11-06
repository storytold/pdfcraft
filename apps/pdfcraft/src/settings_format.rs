//! Readable native preferences, with a private working copy for eframe's compact serializer.
//!
//! Keep eframe's string map and our JSON schema: old installations and older binaries read the
//! same values. This is presentation, not a preference migration.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead as _, Read as _, Write as _};
use std::path::Path;
use std::rc::Rc;

// Settings can contain a signature image. Skip unusually large files rather than allocating
// from an unchecked file length; indentation also has a separate, bounded output budget.
const MAX_INPUT: u64 = 16 * 1024 * 1024;
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

/// Publishes native saves while eframe writes only to its private working copy.
pub struct Store {
    path: std::path::PathBuf,
    keys: Vec<String>,
    working: std::path::PathBuf,
    _directory: tempfile::TempDir,
    published: RefCell<Option<Published>>,
}

/// The input map and exact target bytes from a successful, bounded publication.
struct Published {
    values: BTreeMap<String, String>,
    bytes: Vec<u8>,
}

impl Store {
    /// Preserve the existing storage map and seed eframe's isolated working file.
    pub fn prepare(path: &Path) -> std::io::Result<Self> {
        make_readable(path)?;
        let bytes = match read_bounded(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => b"{}".to_vec(),
            Err(e) => return Err(e),
        };
        let values: BTreeMap<String, String> = ron::de::from_bytes(&bytes).map_err(std::io::Error::other)?;
        let mut keys: std::collections::BTreeSet<String> = values.into_keys().collect();
        // eframe 0.36 writes these two framework keys before it calls App::save. Keep every
        // existing key too, so legacy and unknown storage values survive.
        keys.extend(["window", "egui", "pdfcraft"].map(str::to_string));
        let directory = tempfile::Builder::new().prefix("pdfcraft-preferences-").tempdir()?;
        let working = directory.path().join("app.ron");
        std::fs::write(&working, bytes)?;
        Ok(Self { path: path.to_path_buf(), keys: keys.into_iter().collect(), working, _directory: directory, published: RefCell::new(None) })
    }

    /// The compact persistence target passed to eframe instead of the user's app.ron.
    pub fn working_path(&self) -> &Path {
        &self.working
    }

    /// Skip an idle save only if the validated target still contains our last publication.
    fn save(&self, storage: &dyn eframe::Storage) -> std::io::Result<()> {
        let values: BTreeMap<String, String> = self.keys.iter().filter_map(|key| storage.get_string(key).map(|value| (key.clone(), value))).collect();
        let before = target_metadata(&self.path)?;
        let mut published = self.published.try_borrow_mut().map_err(std::io::Error::other)?;
        if before.is_some()
            && published.as_ref().is_some_and(|saved| saved.values == values && target_matches(&self.path, &saved.bytes).unwrap_or(false))
        {
            return Ok(());
        }
        let formatted = formatting_budget(&values).and_then(|()| readable_values(values.clone()));
        if let Err(e) = &formatted {
            // A newly saved large signature must still persist, even if it exceeds the
            // prettification budget. The compact representation is the existing format.
            log::warn!("saving compact preferences because formatting failed: {e}");
        }
        let parent = self.path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder.create(parent)?;
        publish(
            &self.path,
            before.as_ref().map(std::fs::Metadata::permissions),
            |file| match &formatted {
                Ok(bytes) => file.write_all(bytes),
                Err(_) => {
                    let mut writer = std::io::BufWriter::new(file);
                    ron::Options::default()
                        .to_io_writer_pretty(&mut writer, &values, ron::ser::PrettyConfig::default())
                        .map_err(std::io::Error::other)?;
                    writer.flush()
                }
            },
            || {
                let after = target_metadata(&self.path)?;
                let stamp = |m: &std::fs::Metadata| (m.len(), m.modified().ok());
                if before.as_ref().map(stamp) != after.as_ref().map(stamp) {
                    return Err(std::io::Error::other("preferences changed during saving"));
                }
                Ok(())
            },
        )?;
        // Retain at most 16 MiB of target bytes. Large snapshots still save normally, but
        // bypass this optimization so their compact fallback cannot grow the cache.
        *published = formatted.ok().filter(|bytes| bytes.len() as u64 <= MAX_INPUT).map(|bytes| Published { values, bytes });
        Ok(())
    }
}

/// Compare bytes in fixed-size buffers; missing or unreadable files are cache misses.
fn target_matches(path: &Path, mut expected: &[u8]) -> std::io::Result<bool> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok(expected.is_empty());
        }
        let Some(rest) = expected.strip_prefix(bytes) else { return Ok(false) };
        expected = rest;
        let read = bytes.len();
        reader.consume(read);
    }
}

/// Delegate the native application callbacks and publish preferences during App::save.
pub fn wrap(app: pdfcraft_ui_egui::PdfCraftApp, store: Rc<Store>) -> Box<dyn eframe::App> {
    Box::new(NativeApp { app, store })
}

struct NativeApp {
    app: pdfcraft_ui_egui::PdfCraftApp,
    store: Rc<Store>,
}

impl eframe::App for NativeApp {
    /// Forward raw input unchanged; the wrapper only changes preference publication.
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.app.raw_input_hook(ctx, input);
    }

    /// Run the inner application's frame logic without changing its lifecycle.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.logic(ctx, frame);
    }

    /// Draw the inner application with the original UI and frame.
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
    }

    /// Save the inner app, then publish synchronously before native termination can bypass the loop.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.app.save(storage);
        // Cocoa's terminate: does not return through run_native. Publish synchronously here,
        // before termination; eframe's later background flush only touches the working copy.
        if let Err(e) = self.store.save(storage) {
            log::warn!("could not save preferences at {}: {e}", self.store.path.display());
        }
    }

    /// Forward shutdown cleanup to the inner application.
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.app.on_exit(gl);
    }

    /// Keep the inner application's autosave schedule.
    fn auto_save_interval(&self) -> std::time::Duration {
        self.app.auto_save_interval()
    }

    /// Keep the inner application's background color.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.app.clear_color(visuals)
    }

    /// Preserve the inner application's decision to retain egui memory.
    fn persist_egui_memory(&self) -> bool {
        self.app.persist_egui_memory()
    }
}

/// Inspect without following symlinks; accept a missing target and reject non-writable file kinds.
fn target_metadata(path: &Path) -> std::io::Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.permissions().readonly() => Ok(Some(metadata)),
        Ok(_) => Err(std::io::Error::other("preferences are not a writable regular file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Atomically reformat an existing writable settings map; leave a missing target untouched.
pub fn make_readable(path: &Path) -> std::io::Result<()> {
    let Some(metadata) = target_metadata(path)? else { return Ok(()) };
    let original = read_bounded(path)?;
    let formatted = readable_text(&original)?;
    if formatted == original {
        return Ok(());
    }
    replace(path, &original, &formatted, metadata.permissions())
}

/// Read at most the input limit plus one byte, rejecting oversized settings before parsing.
fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err(std::io::Error::other("preferences exceed the 16 MiB formatting limit"));
    }
    Ok(bytes)
}

/// Decode the legacy-compatible RON string map and format its known JSON values.
fn readable_text(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let values: BTreeMap<String, String> = ron::de::from_bytes(bytes).map_err(std::io::Error::other)?;
    readable_values(values)
}

/// Pretty-print known JSON objects while preserving unknown storage values and keys.
/// Enforce the formatting budgets and let RON escape embedded raw-string delimiters.
fn readable_values(mut values: BTreeMap<String, String>) -> std::io::Result<Vec<u8>> {
    formatting_budget(&values)?;
    for key in ["pdfcraft", "printcraft"] {
        if let Some(json) = values.get_mut(key) {
            let value: serde_json::Value = serde_json::from_str(json).map_err(std::io::Error::other)?;
            if !value.is_object() {
                return Err(std::io::Error::other("preferences must be a JSON object"));
            }
            let mut pretty = Bounded(Vec::new());
            serde_json::to_writer_pretty(&mut pretty, &value).map_err(std::io::Error::other)?;
            *json = String::from_utf8(pretty.0).map_err(std::io::Error::other)?;
        }
    }
    let mut output = Bounded(Vec::new());
    // JSON whitespace alone would become literal `\n` escapes in eframe's RON string. Raw
    // strings keep physical lines and let RON choose enough hashes for any embedded delimiter.
    ron::Options::default()
        .to_io_writer_pretty(&mut output, &values, ron::ser::PrettyConfig::default().escape_strings(false))
        .map_err(std::io::Error::other)?;
    Ok(output.0)
}

/// Reject excessive key/value input sizes and arithmetic overflow before formatting.
fn formatting_budget(values: &BTreeMap<String, String>) -> std::io::Result<()> {
    let size = values.iter().try_fold(0usize, |sum, (key, value)| sum.checked_add(key.len())?.checked_add(value.len()));
    if size.is_none_or(|size| size > MAX_INPUT as usize) {
        return Err(std::io::Error::other("preferences exceed the 16 MiB formatting limit"));
    }
    Ok(())
}

/// Recheck the original bytes before atomic replacement, preserving the target's permissions.
fn replace(path: &Path, original: &[u8], formatted: &[u8], permissions: std::fs::Permissions) -> std::io::Result<()> {
    publish(
        path,
        Some(permissions),
        |file| file.write_all(formatted),
        || {
            // Refuse an observed change; eframe itself does not lock concurrent writers.
            if read_bounded(path)? != original {
                return Err(std::io::Error::other("preferences changed during formatting"));
            }
            Ok(())
        },
    )
}

/// Write and sync an exclusive staging file, validate the observed target, then atomically publish.
/// A failed writer or validation leaves the target and staging cleanup to the temporary-file owner.
fn publish(
    path: &Path,
    permissions: Option<std::fs::Permissions>,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
    unchanged: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    // tempfile creates private, unpredictable, exclusive staging files and removes them on
    // failure. Keep the target's permissions before writing any preference data.
    let mut file = tempfile::Builder::new().prefix(".app.ron-format-").tempfile_in(parent)?;
    if let Some(permissions) = permissions {
        file.as_file().set_permissions(permissions)?;
    }
    write(file.as_file_mut())?;
    file.as_file().sync_all()?;
    unchanged()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

struct Bounded(Vec<u8>);
impl std::io::Write for Bounded {
    /// Append a complete chunk only when it fits the bounded formatting output.
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_OUTPUT.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("formatted preferences exceed the 64 MiB limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    /// No-op for the in-memory formatting buffer; publication syncs the staged file separately.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Memory(BTreeMap<String, String>);
    impl eframe::Storage for Memory {
        /// Clone a stored value for the test application, matching eframe's storage interface.
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        /// Replace one value in the in-memory storage used by native-save tests.
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        /// Remove one storage key without touching any files.
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        /// No-op for the test's in-memory storage, which has no pending disk writes.
        fn flush(&mut self) {}
    }

    /// Require App::save to publish current preferences before native shutdown returns from its loop.
    #[test]
    fn app_save_publishes_without_waiting_for_the_native_loop_to_return() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        std::fs::write(&path, saved("printcraft", r#"{"author":"Synthetic contributor","theme":"Dark"}"#)).unwrap();
        let store = Rc::new(Store::prepare(&path).unwrap());
        assert_ne!(store.working_path(), path);
        let mut memory = Memory(ron::de::from_bytes(&std::fs::read(store.working_path()).unwrap()).unwrap());
        memory.0.insert("window".to_string(), "new window size".to_string());
        memory.0.insert("egui".to_string(), "new window memory".to_string());
        let mut app = pdfcraft_ui_egui::PdfCraftApp::new();
        app.restore(&memory.0["printcraft"]);
        let mut native = NativeApp { app, store };
        for theme in ["light", "dark"] {
            native.app.set_option("theme", theme).unwrap();
            eframe::App::save(&mut native, &mut memory);
            let bytes = std::fs::read(&path).unwrap();
            let loaded: BTreeMap<String, String> = ron::de::from_bytes(&bytes).unwrap();
            let current: serde_json::Value = serde_json::from_str(&loaded["pdfcraft"]).unwrap();
            assert_eq!(current["theme"].as_str().unwrap().to_lowercase(), theme);
            assert!(std::str::from_utf8(&bytes).unwrap().lines().any(|line| line.trim() == r#""author": "Synthetic contributor","#));
            for key in ["egui", "window", "future_storage"] {
                assert_eq!(loaded[key], memory.0[key]);
            }
            assert!(loaded.contains_key("printcraft"));
        }
    }

    /// Create the first settings file at the original location with private permissions.
    #[test]
    fn first_save_creates_preferences_without_changing_the_location() {
        let fixture = Fixture::new();
        let path = fixture.0.join("new/config/app.ron");
        let store = Store::prepare(&path).unwrap();
        assert!(!path.exists());
        store.save(&Memory(BTreeMap::from([("pdfcraft".to_string(), r#"{"theme":"Dark"}"#.to_string())]))).unwrap();
        let loaded: BTreeMap<String, String> = ron::de::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&loaded["pdfcraft"]).unwrap()["theme"], "Dark");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    /// An idle save must leave the file alone; changed application settings still publish.
    #[test]
    fn unchanged_autosaves_leave_the_target_untouched() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        let store = Store::prepare(&path).unwrap();
        let mut memory = Memory(BTreeMap::from([("pdfcraft".to_string(), r#"{"theme":"Dark"}"#.to_string())]));
        store.save(&memory).unwrap();
        let original = std::fs::read(&path).unwrap();
        let stamp = mark_saved_file(&path);
        for _ in 0..3 {
            store.save(&memory).unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp, "idle autosave replaced the file");
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
        memory.0.insert("pdfcraft".to_string(), r#"{"theme":"Light"}"#.to_string());
        store.save(&memory).unwrap();
        assert_ne!(std::fs::read(&path).unwrap(), original);
        let loaded: BTreeMap<String, String> = ron::de::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&loaded["pdfcraft"]).unwrap()["theme"], "Light");
    }

    /// Matching size and modification time do not hide a same-length external edit.
    #[test]
    fn externally_edited_preferences_invalidate_the_autosave_cache() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        let store = Store::prepare(&path).unwrap();
        let memory = Memory(BTreeMap::from([("pdfcraft".to_string(), r#"{"theme":"Dark"}"#.to_string())]));
        store.save(&memory).unwrap();
        let original = std::fs::read(&path).unwrap();
        let stamp = mark_saved_file(&path);
        let external = String::from_utf8(original.clone()).unwrap().replace("Dark", "Lite");
        assert_ne!(external.as_bytes(), original);
        assert_eq!(external.len(), original.len());
        std::fs::write(&path, external).unwrap();
        assert_eq!(mark_saved_file(&path), stamp);
        store.save(&memory).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original, "the active session's values must still be saved");
        let stamp = mark_saved_file(&path);
        store.save(&memory).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp, "the successful repair should be cached");
    }

    /// A missing target must be recreated even when the application snapshot is unchanged.
    #[test]
    fn deleted_preferences_are_recreated_before_autosave_can_be_skipped() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        let store = Store::prepare(&path).unwrap();
        let memory = Memory(BTreeMap::from([("pdfcraft".to_string(), r#"{"author":"Synthetic contributor"}"#.to_string())]));
        store.save(&memory).unwrap();
        let original = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        store.save(&memory).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let stamp = mark_saved_file(&path);
        store.save(&memory).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);
    }

    /// A cache hit must still reject a substituted symlink or a newly read-only file.
    #[cfg(unix)]
    #[test]
    fn cached_preferences_still_validate_the_target() {
        use std::os::unix::{fs::PermissionsExt as _, fs::symlink};
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        let store = Store::prepare(&path).unwrap();
        let memory = Memory(BTreeMap::from([("pdfcraft".to_string(), "{}".to_string())]));
        store.save(&memory).unwrap();
        let original = std::fs::read(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(store.save(&memory).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let other = fixture.0.join("other.ron");
        std::fs::rename(&path, &other).unwrap();
        symlink(&other, &path).unwrap();
        assert!(store.save(&memory).is_err());
        assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&other).unwrap(), original);
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(other, &path).unwrap();
        let stamp = mark_saved_file(&path);
        store.save(&memory).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);
    }

    /// A failed save cannot cache an unpublished value or prevent retrying that value.
    #[test]
    fn failed_publication_is_retried_with_the_same_preferences() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        let backup = fixture.0.join("saved.ron");
        let store = Store::prepare(&path).unwrap();
        let mut memory = Memory(BTreeMap::from([("pdfcraft".to_string(), r#"{"theme":"Dark"}"#.to_string())]));
        store.save(&memory).unwrap();
        let original = std::fs::read(&path).unwrap();
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        memory.0.insert("pdfcraft".to_string(), r#"{"theme":"Light"}"#.to_string());
        assert!(store.save(&memory).is_err());
        assert_eq!(std::fs::read(&backup).unwrap(), original);
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        // The failed value must not evict the previous successful publication either.
        memory.0.insert("pdfcraft".to_string(), r#"{"theme":"Dark"}"#.to_string());
        let stamp = mark_saved_file(&path);
        store.save(&memory).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);
        memory.0.insert("pdfcraft".to_string(), r#"{"theme":"Light"}"#.to_string());
        store.save(&memory).unwrap();
        let loaded: BTreeMap<String, String> = ron::de::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&loaded["pdfcraft"]).unwrap()["theme"], "Light");
        let stamp = mark_saved_file(&path);
        store.save(&memory).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1, "no failed staging files may remain");
    }

    /// Set an old, deterministic timestamp so tests detect replacements without sleeps.
    fn mark_saved_file(path: &Path) -> std::time::SystemTime {
        let stamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_234_567_890);
        std::fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(stamp).unwrap();
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    /// Keep an oversized newly saved preference even when readable formatting must fall back.
    #[test]
    fn a_new_large_snapshot_still_persists_in_compact_form() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        let store = Store::prepare(&path).unwrap();
        let json = serde_json::json!({"future_large_setting": "a".repeat(MAX_INPUT as usize)}).to_string();
        let memory = Memory(BTreeMap::from([("pdfcraft".to_string(), json)]));
        store.save(&memory).unwrap();
        let loaded: BTreeMap<String, String> = ron::de::from_reader(std::fs::File::open(path).unwrap()).unwrap();
        assert_eq!(loaded, memory.0, "a formatting limit must not discard a saved preference");
    }

    // Same map and default PrettyConfig as eframe 0.36's FileStorage::save_to_disk.
    /// Build the same outer string map and default RON format as eframe's file storage.
    fn saved(key: &str, json: &str) -> Vec<u8> {
        ron::ser::to_string_pretty(
            &BTreeMap::from([
                (key.to_string(), json.to_string()),
                ("egui".to_string(), "(memory:(),data:{})".to_string()),
                ("window".to_string(), "(inner_size_points:Some((1440.0,920.0)))".to_string()),
                ("future_storage".to_string(), "leave me \\".to_string()),
            ]),
            ron::ser::PrettyConfig::default(),
        )
        .unwrap()
        .into_bytes()
    }

    /// Expose individual JSON settings on physical lines without changing stored values or unknown keys.
    #[test]
    fn preferences_have_physical_lines_for_each_setting() {
        let original = saved("pdfcraft", r#"{"date_format":"d/m/yyyy","date_language":null,"default_zoom":"100%","javascript":false}"#);
        let formatted = readable_text(&original).unwrap();
        let text = std::str::from_utf8(&formatted).unwrap();
        for setting in [r#""date_format": "d/m/yyyy""#, r#""date_language": null"#, r#""default_zoom": "100%""#, r#""javascript": false"#] {
            assert!(text.lines().any(|line| line.trim().trim_end_matches(',') == setting), "no readable line for {setting}: {text}");
        }
        let old: BTreeMap<String, String> = ron::de::from_bytes(&original).unwrap();
        let new: BTreeMap<String, String> = ron::de::from_bytes(&formatted).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&old["pdfcraft"]).unwrap(),
            serde_json::from_str::<serde_json::Value>(&new["pdfcraft"]).unwrap()
        );
        for key in ["window", "egui", "future_storage"] {
            assert_eq!(old[key], new[key], "eframe and unknown storage values must survive");
        }
    }

    /// Preserve legacy keys, Unicode, escapes and raw-string delimiters through repeated formatting.
    #[test]
    fn legacy_printcraft_and_special_strings_keep_their_values() {
        let value = serde_json::json!({
            "author": "Test \"name\" \\ path\nnext line\tПривіт",
            "recent": [{"name": "r###\"closing\"###.pdf", "path": "C:\\PDFs\\demo.pdf"}],
            "future_setting": {"nested": [true, null, u64::MAX]},
        });
        for key in ["pdfcraft", "printcraft"] {
            let formatted = readable_text(&saved(key, &value.to_string())).unwrap();
            let loaded: BTreeMap<String, String> = ron::de::from_bytes(&formatted).unwrap();
            assert_eq!(serde_json::from_str::<serde_json::Value>(&loaded[key]).unwrap(), value);
            let mut app = pdfcraft_ui_egui::PdfCraftApp::new();
            app.restore(&loaded[key]);
            let restored: serde_json::Value = serde_json::from_str(&app.persist()).unwrap();
            assert_eq!(restored["author"], value["author"]);
            assert_eq!(readable_text(&formatted).unwrap(), formatted, "formatting is idempotent");
        }
    }

    /// Restore legacy preferences into the real app and preserve their values in a readable save.
    #[test]
    fn legacy_preferences_restore_then_save_in_the_readable_format() {
        let legacy =
            r#"{"author":"Synthetic contributor","language":"en","theme":"Dark","default_zoom":"150%","default_mode":"edit","javascript":false}"#;
        let loaded: BTreeMap<String, String> = ron::de::from_bytes(&saved("printcraft", legacy)).unwrap();
        let mut app = pdfcraft_ui_egui::PdfCraftApp::new();
        app.restore(&loaded["printcraft"]);
        let compact = app.persist();
        let formatted = readable_text(&saved("pdfcraft", &compact)).unwrap();
        let text = std::str::from_utf8(&formatted).unwrap();
        assert!(text.lines().any(|line| line.trim() == r#""default_zoom": "150%","#), "{text}");
        let loaded: BTreeMap<String, String> = ron::de::from_bytes(&formatted).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&loaded["pdfcraft"]).unwrap(),
            serde_json::from_str::<serde_json::Value>(&compact).unwrap(),
            "formatting keeps every saved setting, including per-tool style arrays"
        );
        let mut reopened = pdfcraft_ui_egui::PdfCraftApp::new();
        reopened.restore(&loaded["pdfcraft"]);
        let restored: serde_json::Value = serde_json::from_str(&reopened.persist()).unwrap();
        let legacy: serde_json::Value = serde_json::from_str(legacy).unwrap();
        for (key, value) in legacy.as_object().unwrap() {
            assert_eq!(&restored[key], value, "legacy preference {key} must restore");
        }
    }

    /// Reformat in place without changing permissions or leaving a staging file behind.
    #[test]
    fn file_formatting_keeps_the_schema_and_permissions() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        let original = saved("pdfcraft", r#"{"default_zoom":"100%","javascript":false}"#);
        std::fs::write(&path, &original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        make_readable(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), readable_text(&original).unwrap());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    /// Reject malformed or oversized preferences without modifying existing files.
    #[test]
    fn malformed_or_large_files_are_left_alone() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        for original in [b"not RON".to_vec(), saved("pdfcraft", "{broken JSON"), vec![b' '; MAX_INPUT as usize + 1]] {
            std::fs::write(&path, &original).unwrap();
            assert!(make_readable(&path).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
        }
        assert!(make_readable(&fixture.0.join("missing.ron")).is_ok());
    }

    /// Preserve an observed external edit and remove failed-publication staging files.
    #[test]
    fn a_changed_target_is_not_replaced_and_staging_is_cleaned_up() {
        let fixture = Fixture::new();
        let path = fixture.0.join("app.ron");
        let original = saved("pdfcraft", "{}");
        let changed = saved("pdfcraft", r#"{"author":"another window"}"#);
        std::fs::write(&path, &changed).unwrap();
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        assert!(replace(&path, &original, b"new", permissions).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), changed);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    /// Refuse symlinks and read-only targets without changing the destination's contents.
    #[cfg(unix)]
    #[test]
    fn symlinks_and_readonly_files_are_left_alone() {
        use std::os::unix::{fs::PermissionsExt as _, fs::symlink};
        let fixture = Fixture::new();
        let target = fixture.0.join("original.ron");
        let link = fixture.0.join("app.ron");
        let original = saved("pdfcraft", "{}");
        std::fs::write(&target, &original).unwrap();
        symlink(&target, &link).unwrap();
        assert!(make_readable(&link).is_err());
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(make_readable(&target).is_err());
        assert_eq!(std::fs::read(target).unwrap(), original);
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        /// Create a distinct process-scoped settings directory owned by this test fixture.
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!("pdfcraft-settings-format-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        /// Remove only this fixture's temporary settings directory on scope exit.
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
