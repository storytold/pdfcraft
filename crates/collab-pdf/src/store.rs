//! Durable room state: one committed base per version plus an append-only
//! journal of the operations accepted after it.
//!
//! ```text
//! <dir>/
//!   CURRENT                 {"version":N,"epoch":E} — the base to load
//!   versions/<N>/file.pdf   the document at version N
//!   journal.jsonl           {"version":N,"actor":"…","op":{…}} per accepted operation
//!   CABINET                 {"version":N,"etag":"…"} — what the cabinet holds
//! ```
//!
//! The layout is what makes a crash between "write the base" and "replace
//! `CURRENT`" harmless: the new base is a new file, so until `CURRENT` moves the
//! old base is still the one loaded and the whole journal replays on top of it.
//! After `CURRENT` moves, journal entries at or below that version are skipped,
//! so neither window applies an operation twice.
//!
//! A record is written and synced before the room acknowledges the operation. A
//! journal line that is not valid JSON is only tolerated as the last line of
//! the file: that is a torn write of an operation the client was never told
//! about. A broken line anywhere else stops the room from opening.

use pdfcraft_collab_proto::{ActorId, ErrorCode, Op};
use pdfcraft_collab_server::AdapterError;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

const CURRENT: &str = "CURRENT";
const JOURNAL: &str = "journal.jsonl";
const CABINET: &str = "CABINET";

/// One accepted operation, as it was broadcast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRecord {
    pub version: u64,
    pub actor: String,
    pub op: Op,
}

/// What the cabinet holds for this file: the version it has and the etag of its
/// copy. Kept apart from `CURRENT` because a commit writes the base first and
/// the cabinet accepts it afterwards: between those two steps the base is ahead
/// of the cabinet, and the room must still know that (it is dirty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CabinetCopy {
    pub version: u64,
    pub etag: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Current {
    version: u64,
    epoch: u64,
}

/// A room's directory. Not `Sync`: the room owns its adapter and the transport
/// serialises calls.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
    current: Option<Current>,
    cabinet: Option<CabinetCopy>,
}

impl Store {
    /// Open (and create) a room directory. A `CURRENT` file that exists but
    /// cannot be read or parsed is an error: guessing the base would risk
    /// replaying operations on top of the wrong document.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, AdapterError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|e| io_error(&dir, "create", e))?;
        let path = dir.join(CURRENT);
        let current = match fs::read_to_string(&path) {
            Ok(text) => Some(
                serde_json::from_str::<Current>(&text)
                    .map_err(|e| AdapterError::new(ErrorCode::Invalid, format!("{} is not a valid CURRENT file: {e}", path.display()), false))?,
            ),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => return Err(io_error(&path, "read", e)),
        };
        let cabinet = read_cabinet(&dir)?;
        Ok(Self { dir, current, cabinet })
    }

    /// What the cabinet holds, when a session recorded it. A room seeded before
    /// this marker existed has none, and external-change checks are skipped
    /// instead of guessed.
    pub fn cabinet(&self) -> Option<&CabinetCopy> {
        self.cabinet.as_ref()
    }

    /// Record what the cabinet holds. `None` forgets it. The write is atomic: a
    /// half-written marker would make the next commit compare against a value
    /// nobody agreed to.
    pub fn set_cabinet(&mut self, copy: Option<CabinetCopy>) -> Result<(), AdapterError> {
        let path = self.dir.join(CABINET);
        match &copy {
            Some(copy) => {
                let json =
                    serde_json::to_vec(copy).map_err(|e| AdapterError::new(ErrorCode::Invalid, format!("could not encode CABINET: {e}"), false))?;
                write_atomic(&path, &json).map_err(|e| io_error(&path, "write", e))?;
            }
            None => match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(io_error(&path, "remove", e)),
            },
        }
        self.cabinet = copy;
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The version and epoch of the committed base, or `(0, 0)` before the
    /// first base is written.
    pub fn base_state(&self) -> (u64, u64) {
        self.current.map_or((0, 0), |c| (c.version, c.epoch))
    }

    /// The committed base bytes, when there is a committed base.
    pub fn base(&self) -> Result<Option<Vec<u8>>, AdapterError> {
        let Some(current) = self.current else { return Ok(None) };
        let path = self.base_path(current.version);
        let bytes = fs::read(&path).map_err(|e| io_error(&path, "read", e))?;
        Ok(Some(bytes))
    }

    fn base_path(&self, version: u64) -> PathBuf {
        self.dir.join("versions").join(version.to_string()).join("file.pdf")
    }

    /// Write `bytes` as the base for `version` and make it the one to load.
    /// The journal is not touched: the caller truncates it afterwards.
    pub fn write_base(&mut self, version: u64, epoch: u64, bytes: &[u8]) -> Result<(), AdapterError> {
        let path = self.base_path(version);
        let parent = path.parent().ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the base has no directory", false))?;
        fs::create_dir_all(parent).map_err(|e| io_error(parent, "create", e))?;
        write_atomic(&path, bytes).map_err(|e| io_error(&path, "write", e))?;
        let current = Current { version, epoch };
        let json =
            serde_json::to_vec(&current).map_err(|e| AdapterError::new(ErrorCode::Invalid, format!("could not encode CURRENT: {e}"), false))?;
        let marker = self.dir.join(CURRENT);
        write_atomic(&marker, &json).map_err(|e| io_error(&marker, "write", e))?;
        self.current = Some(current);
        // Older bases are unreachable now. Keeping them would grow the disk by
        // a whole PDF per commit; the cabinet keeps versions on its side. A
        // failure here costs space, not correctness, so it is not fatal.
        self.prune_versions(version);
        Ok(())
    }

    /// Remove base directories older than `version` (best effort).
    fn prune_versions(&self, version: u64) {
        let Ok(entries) = fs::read_dir(self.dir.join("versions")) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(older) = name.to_str().and_then(|name| name.parse::<u64>().ok()) else { continue };
            if older < version {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    /// Append one accepted operation and sync it. The room acknowledges the
    /// operation only after this returns.
    pub fn append(&mut self, version: u64, actor: &ActorId, op: &Op) -> Result<(), AdapterError> {
        let record = JournalRecord { version, actor: actor.as_str().to_owned(), op: op.clone() };
        let mut line =
            serde_json::to_vec(&record).map_err(|e| AdapterError::new(ErrorCode::Internal, format!("could not encode an operation: {e}"), false))?;
        line.push(b'\n');
        let path = self.dir.join(JOURNAL);
        let mut file = OpenOptions::new().create(true).append(true).open(&path).map_err(|e| io_error(&path, "open", e))?;
        file.write_all(&line).map_err(|e| io_error(&path, "append", e))?;
        file.sync_data().map_err(|e| io_error(&path, "sync", e))?;
        Ok(())
    }

    /// Every journal record, oldest first. A torn last line is dropped: it was
    /// never acknowledged.
    pub fn journal(&self) -> Result<Vec<JournalRecord>, AdapterError> {
        let path = self.dir.join(JOURNAL);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error(&path, "read", e)),
        };
        let lines: Vec<(usize, &str)> = text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()).collect();
        let last = lines.last().map(|(index, _)| *index);
        let mut records = Vec::with_capacity(lines.len());
        for (index, line) in lines {
            match serde_json::from_str::<JournalRecord>(line) {
                Ok(record) => records.push(record),
                Err(_) if Some(index) == last => break,
                Err(e) => {
                    return Err(AdapterError::new(ErrorCode::Invalid, format!("{} line {} is corrupt: {e}", path.display(), index + 1), false));
                }
            }
        }
        Ok(records)
    }

    /// Drop the records the base at `version` already contains. Records are
    /// re-addressed, not edited: the file is rewritten and replaced atomically.
    pub fn truncate(&mut self, version: u64) -> Result<(), AdapterError> {
        let kept: Vec<JournalRecord> = self.journal()?.into_iter().filter(|record| record.version > version).collect();
        let mut text = Vec::new();
        for record in &kept {
            let mut line = serde_json::to_vec(record)
                .map_err(|e| AdapterError::new(ErrorCode::Internal, format!("could not encode an operation: {e}"), false))?;
            line.push(b'\n');
            text.extend_from_slice(&line);
        }
        let path = self.dir.join(JOURNAL);
        write_atomic(&path, &text).map_err(|e| io_error(&path, "write", e))
    }
}

/// Read the recorded cabinet marker. A missing file means none; a file that
/// exists but cannot be parsed is an error, not a silent `None`: treating a
/// broken marker as "not recorded" would disable the external-change check.
fn read_cabinet(dir: &Path) -> Result<Option<CabinetCopy>, AdapterError> {
    let path = dir.join(CABINET);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error(&path, "read", e)),
    };
    let copy = serde_json::from_str::<CabinetCopy>(&text)
        .map_err(|e| AdapterError::new(ErrorCode::Invalid, format!("{} is not a valid CABINET file: {e}", path.display()), false))?;
    if copy.etag.trim().is_empty() {
        return Err(AdapterError::new(ErrorCode::Invalid, format!("{} has an empty etag", path.display()), false));
    }
    Ok(Some(copy))
}

/// Park the process at a named crash window so that an outside `SIGKILL` lands
/// exactly there instead of wherever the scheduler happened to be.
///
/// A test arms it with `PDFCRAFT_COLLAB_FAULT=<point>` and
/// `PDFCRAFT_COLLAB_FAULT_MARKER=<path>`: once the marker file exists the
/// process is parked, and the test kills it. Debug builds only, so a
/// deployment cannot set an environment variable that stalls a room. The
/// windows themselves are the ones RFC §8.1 reasons about; `PdfAdapter::commit`
/// parks `commit-before-base` between the journal record and the base write.
#[cfg(debug_assertions)]
pub(crate) fn park_at_fault(point: &str) {
    if std::env::var("PDFCRAFT_COLLAB_FAULT").ok().as_deref() != Some(point) {
        return;
    }
    let Some(marker) = std::env::var_os("PDFCRAFT_COLLAB_FAULT_MARKER") else { return };
    if fs::write(&marker, point.as_bytes()).is_err() {
        // No marker means the test cannot tell the window was reached; parking
        // anyway would only turn a failed marker into a timeout.
        return;
    }
    loop {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(not(debug_assertions))]
pub(crate) fn park_at_fault(_point: &str) {}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    {
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)?;
    if let Some(dir) = path.parent()
        && let Ok(handle) = File::open(dir)
    {
        // The rename itself must survive a crash.
        let _ = handle.sync_all();
    }
    Ok(())
}

fn io_error(path: &Path, action: &str, error: std::io::Error) -> AdapterError {
    AdapterError::new(ErrorCode::Invalid, format!("could not {action} {}: {error}", path.display()), false)
}

#[cfg(test)]
mod tests {
    use super::Store;
    use pdfcraft_collab_proto::{ActorId, ClientInfo, ConflictClass, DocKey, Hello, Op, OpId, OpKind, PageRef, Role, SessionId, Target};
    use pdfcraft_collab_server::{Room, RoomConfig};
    use serde_json::json;

    fn dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("pdfcraft-collab-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn op(id: &str) -> Result<Op, Box<dyn std::error::Error>> {
        Ok(Op {
            op_id: OpId::new(id)?,
            doc: DocKey::new("file-1")?,
            base_version: 0,
            epoch: 0,
            kind: OpKind::Primitive,
            class: ConflictClass::Merge,
            target: Target::Page(PageRef { index: 0, epoch: 0 }),
            payload: json!({"action": "annotation.add", "at": [1.0, 2.0], "contents": id, "nm": id}),
            preview: None,
        })
    }

    fn hello() -> Result<Hello, Box<dyn std::error::Error>> {
        Ok(Hello {
            v: 1,
            app: "pdfcraft".into(),
            doc: DocKey::new("file-1")?,
            jwt: "t".into(),
            client: ClientInfo { name: "test".into(), version: "0".into() },
        })
    }

    #[test]
    fn a_base_and_the_journal_survive_reopening() -> Result<(), Box<dyn std::error::Error>> {
        let path = dir("reopen");
        let actor = ActorId::new("editor")?;
        {
            let mut store = Store::open(&path)?;
            assert_eq!(store.base_state(), (0, 0));
            assert!(store.base()?.is_none());
            store.write_base(4, 2, b"%PDF-base")?;
            store.append(5, &actor, &op("a5")?)?;
            store.append(6, &actor, &op("a6")?)?;
        }
        let store = Store::open(&path)?;
        assert_eq!(store.base_state(), (4, 2));
        assert_eq!(store.base()?.as_deref(), Some(b"%PDF-base".as_slice()));
        let records = store.journal()?;
        assert_eq!(records.len(), 2);
        assert_eq!((records[0].version, records[1].version), (5, 6));
        assert_eq!(records[0].actor, "editor");
        assert_eq!(records[0].op.op_id.as_str(), "a5");
        let _ = std::fs::remove_dir_all(&path);
        Ok(())
    }

    #[test]
    fn truncation_keeps_only_the_operations_the_base_lacks() -> Result<(), Box<dyn std::error::Error>> {
        let path = dir("truncate");
        let actor = ActorId::new("editor")?;
        let mut store = Store::open(&path)?;
        store.write_base(0, 0, b"%PDF-base")?;
        for version in 1..=3 {
            store.append(version, &actor, &op(&format!("a{version}"))?)?;
        }
        store.truncate(2)?;
        let kept = store.journal()?;
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].version, 3);
        // A later base drops the rest.
        store.truncate(3)?;
        assert!(store.journal()?.is_empty());
        let _ = std::fs::remove_dir_all(&path);
        Ok(())
    }

    #[test]
    fn only_the_base_current_points_at_is_kept() -> Result<(), Box<dyn std::error::Error>> {
        let path = dir("prune");
        let mut store = Store::open(&path)?;
        store.write_base(0, 0, b"%PDF-0")?;
        store.write_base(1, 0, b"%PDF-1")?;
        store.write_base(2, 2, b"%PDF-2")?;
        assert!(!path.join("versions/0").exists());
        assert!(!path.join("versions/1").exists());
        assert_eq!(store.base()?.as_deref(), Some(b"%PDF-2".as_slice()));
        assert_eq!(store.base_state(), (2, 2));
        // A new Store reads the marker, not a directory listing.
        assert_eq!(Store::open(&path)?.base()?.as_deref(), Some(b"%PDF-2".as_slice()));
        let _ = std::fs::remove_dir_all(&path);
        Ok(())
    }

    #[test]
    fn a_crash_after_the_marker_does_not_apply_an_operation_twice() -> Result<(), Box<dyn std::error::Error>> {
        use crate::{PdfAdapter, fixture};
        use pdfcraft_collab_server::Room;
        let path = dir("crash-after-marker");
        let doc = DocKey::new("file-1")?;
        let editor = ActorId::new("editor")?;
        {
            let mut store = Store::open(&path)?;
            store.write_base(0, 0, &fixture(1))?;
            let mut adapter = PdfAdapter::open("fixture.pdf", fixture(1).into())?;
            adapter.attach_store(store);
            let mut room = Room::new(SessionId::new("s1")?, doc.clone(), adapter, RoomConfig::default());
            room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
            room.apply(&editor, &op("a1")?, 0)?;
            let commit = room.commit(&editor, 1)?;
            let outcome = commit.publish();
            room.finish(commit, outcome)?;
        }
        // Simulate the crash window: the base and CURRENT are at version 1, but
        // the journal still holds the entry the commit was about to truncate.
        let mut store = Store::open(&path)?;
        assert_eq!(store.base_state(), (1, 0));
        store.append(1, &editor, &op("a1")?)?;
        let records = store.journal()?;
        assert_eq!(records.len(), 1, "the stale entry is on disk");
        let bytes = store.base()?.ok_or("no base")?;
        let mut adapter = PdfAdapter::open("fixture.pdf", bytes.into())?;
        adapter.attach_store(store);
        let mut room = Room::resume(SessionId::new("s2")?, doc, adapter, 1, 0, RoomConfig::default());
        for record in records.into_iter().filter(|record| record.version > 1) {
            let actor = ActorId::new(record.actor)?;
            room.replay(actor, record.op, record.version)?;
        }
        assert_eq!(room.version(), 1);
        let document = room.adapter().document().ok_or("no document")?;
        assert_eq!(document.info.annotations.iter().filter(|a| a.name.as_deref() == Some("a1")).count(), 1);
        let _ = std::fs::remove_dir_all(&path);
        Ok(())
    }

    #[test]
    fn a_torn_last_line_is_dropped_but_a_broken_middle_line_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
        let path = dir("torn");
        let actor = ActorId::new("editor")?;
        let mut store = Store::open(&path)?;
        store.append(1, &actor, &op("a1")?)?;
        let journal = path.join("journal.jsonl");
        let mut text = std::fs::read_to_string(&journal)?;
        text.push_str("{\"version\":2,\"actor\":\"edi");
        std::fs::write(&journal, text)?;
        assert_eq!(store.journal()?.len(), 1, "the partial line was never acknowledged");
        let mut text = std::fs::read_to_string(&journal)?;
        text.insert_str(0, "not json\n");
        std::fs::write(&journal, text)?;
        assert!(store.journal().is_err(), "a broken line before the end hides acknowledged operations");
        let _ = std::fs::remove_dir_all(&path);
        Ok(())
    }

    #[test]
    fn a_corrupt_current_file_is_refused_instead_of_guessed() -> Result<(), Box<dyn std::error::Error>> {
        let path = dir("current");
        std::fs::create_dir_all(&path)?;
        std::fs::write(path.join("CURRENT"), "not json")?;
        assert!(Store::open(&path).is_err());
        let _ = std::fs::remove_dir_all(&path);
        Ok(())
    }

    #[test]
    fn a_room_replays_its_journal_after_a_restart() -> Result<(), Box<dyn std::error::Error>> {
        use crate::{PdfAdapter, fixture};
        let path = dir("replay");
        let doc = DocKey::new("file-1")?;
        let editor = ActorId::new("editor")?;
        {
            let mut store = Store::open(&path)?;
            store.write_base(0, 0, &fixture(1))?;
            let mut adapter = PdfAdapter::open("fixture.pdf", fixture(1).into())?;
            adapter.attach_store(store);
            let mut room = Room::new(SessionId::new("s1")?, doc.clone(), adapter, RoomConfig::default());
            room.hello(editor.clone(), Role::Editor, &hello()?, 0)?;
            room.apply(&editor, &op("a1")?, 0)?;
            // No commit: the room "crashes" with one accepted operation.
        }
        let store = Store::open(&path)?;
        let (version, epoch) = store.base_state();
        assert_eq!((version, epoch), (0, 0));
        // The crash window before `CURRENT` moves: a base for the next version
        // exists on disk but the marker still names the old one, so the whole
        // journal has to be replayed on the old base.
        std::fs::create_dir_all(path.join("versions/1"))?;
        std::fs::write(path.join("versions/1/file.pdf"), fixture(2))?;
        let records = store.journal()?;
        let bytes = store.base()?.ok_or("no base")?;
        let mut adapter = PdfAdapter::open("fixture.pdf", bytes.into())?;
        adapter.attach_store(store);
        let mut room = Room::resume(SessionId::new("s2")?, doc, adapter, version, epoch, RoomConfig::default());
        for record in records {
            let actor = ActorId::new(record.actor)?;
            room.replay(actor, record.op, record.version)?;
        }
        assert_eq!(room.version(), 1);
        let document = room.adapter().document().ok_or("no document")?;
        assert!(document.info.annotations.iter().any(|a| a.name.as_deref() == Some("a1")), "the acknowledged operation survived");
        let _ = std::fs::remove_dir_all(&path);
        Ok(())
    }
}
