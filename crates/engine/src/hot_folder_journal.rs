//! Small active journal plus durable, sharded history. Historical receipts are never
//! discarded to make room: an evicted source version must remain deduplicated after restart.
//! Individual reads/writes and the hot cache are bounded; disk history grows with unique work.
use super::*;

const HOT_ENTRIES: usize = 256;
const HOT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Entry<T> {
    key: String,
    value: T,
}

fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    meta.file_type().is_symlink()
}

fn directory(path: &Path, create: bool) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !is_link(&meta) => Ok(true),
        Ok(_) => Err("hot folder history directory must not be a link or file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            #[cfg(not(unix))]
            let _ = &mut builder;
            builder.create(path).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            if rustix::process::geteuid().as_raw() == 0 {
                use std::os::unix::fs::MetadataExt;
                let owner = fs::metadata(path.parent().ok_or("history needs a parent")?).map_err(|e| e.to_string())?;
                std::os::unix::fs::chown(path, Some(owner.uid()), Some(owner.gid())).map_err(|e| e.to_string())?;
            }
            sync_parent(path)?;
            Ok(true)
        }
        Err(e) => Err(e.to_string()),
    }
}

fn location(config: &Path, kind: &str, key: &str, create: bool) -> Result<Option<PathBuf>, String> {
    let digest: String = Sha256::digest(key.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    let base = config.with_extension("journal");
    let group = base.join(kind);
    let shard = group.join(digest.get(..2).ok_or("invalid history key")?);
    for path in [&base, &group, &shard] {
        if !directory(path, create)? {
            return Ok(None);
        }
    }
    let path = shard.join(format!("{digest}.json"));
    match fs::symlink_metadata(&path) {
        Ok(meta) if is_link(&meta) || !meta.is_file() => Err("hot folder history must be a regular file".into()),
        Ok(_) => Ok(Some(path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(create.then_some(path)),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn read<T: for<'de> Deserialize<'de>>(config: &Path, kind: &str, key: &str) -> Result<Option<T>, String> {
    let Some(path) = location(config, kind, key, false)? else { return Ok(None) };
    let entry: Entry<T> = read_json(&path)?;
    if entry.key != key {
        return Err("hot folder history key does not match its durable receipt".into());
    }
    Ok(Some(entry.value))
}

pub(crate) fn write<T: Serialize>(config: &Path, kind: &str, key: &str, value: &T) -> Result<(), String> {
    let path = location(config, kind, key, true)?.ok_or("history path unavailable")?;
    save_json(&path, &Entry { key: key.to_owned(), value })
}

/// A full write-ahead snapshot makes every group receipt visible together even if
/// the process is killed halfway through sharding its history.
pub(crate) fn pending_path(target: &Path) -> PathBuf {
    target.with_extension("pending.json")
}

pub(crate) fn pending<T: for<'de> Deserialize<'de>>(target: &Path) -> Result<Option<T>, String> {
    let path = pending_path(target);
    match fs::symlink_metadata(&path) {
        Ok(meta) if is_link(&meta) || !meta.is_file() => Err("pending journal must be a regular file".into()),
        Ok(_) => read_json(&path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn begin<T: Serialize>(target: &Path, value: &T) -> Result<(), String> {
    save_json(&pending_path(target), value)
}

pub(crate) fn finish(target: &Path) -> Result<(), String> {
    let path = pending_path(target);
    match fs::remove_file(&path) {
        Ok(()) => sync_parent(&path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn receipt(config: &Path, state: &State, key: &str) -> Result<Option<Receipt>, String> {
    match state.receipts.get(key) {
        Some(value) => Ok(Some(value.clone())),
        None => read(config, "receipts", key),
    }
}

fn observation_key(state: &State, key: &str) -> String {
    let task = key.split(':').next().unwrap_or_default();
    format!("{}:{key}", state.observation_epochs.get(task).copied().unwrap_or_default())
}

pub(super) fn observation(config: &Path, state: &State, key: &str) -> Result<Option<Observation>, String> {
    match state.observations.get(key) {
        Some(value) => Ok(Some(value.clone())),
        None => read(config, "observations", &observation_key(state, key)),
    }
}

/// Publication order is intentional: archive entries first, then the compact active state.
/// A killed process sees either the old complete cache or the new cache plus durable history.
pub(super) fn save(config: &Path, state: &mut State) -> Result<(), String> {
    save_with(config, state, &|_| Ok(()))
}

pub(super) fn save_with(config: &Path, state: &mut State, after_spill: &dyn Fn(usize) -> Result<(), String>) -> Result<(), String> {
    let links: BTreeMap<_, _> = state
        .receipts
        .iter()
        .filter_map(|(key, receipt)| key.rsplit_once(':').map(|(source, _)| ((source.to_owned(), receipt.source_stamp.clone()), key.clone())))
        .collect();
    for (key, observation) in &mut state.observations {
        if observation.receipt_key.is_none() {
            observation.receipt_key = links.get(&(key.clone(), observation.fingerprint.clone())).cloned();
        }
    }
    let target = state_path(config);
    let needs_spill = state.receipts.len() > HOT_ENTRIES
        || state.observations.len() > HOT_ENTRIES
        || serde_json::to_vec(&state.receipts).map_err(|e| e.to_string())?.len() > HOT_BYTES
        || serde_json::to_vec(&state.observations).map_err(|e| e.to_string())?.len() > HOT_BYTES;
    if needs_spill {
        begin(&target, state)?;
    }
    let mut spilled = 0;
    while state.receipts.len() > HOT_ENTRIES || serde_json::to_vec(&state.receipts).map_err(|e| e.to_string())?.len() > HOT_BYTES {
        let Some((key, value)) = state.receipts.first_key_value() else { break };
        write(config, "receipts", key, value)?;
        let key = key.clone();
        state.receipts.remove(&key);
        spilled += 1;
        after_spill(spilled)?;
    }
    while state.observations.len() > HOT_ENTRIES || serde_json::to_vec(&state.observations).map_err(|e| e.to_string())?.len() > HOT_BYTES {
        let Some((key, value)) = state.observations.first_key_value() else { break };
        write(config, "observations", &observation_key(state, key), value)?;
        let key = key.clone();
        state.observations.remove(&key);
        spilled += 1;
        after_spill(spilled)?;
    }
    if state.log.len() > 1000 {
        state.log.drain(..state.log.len().saturating_sub(1000));
    }
    while !state.log.is_empty() && serde_json::to_vec(&state.log).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        state.log.drain(..state.log.len().div_ceil(2));
    }
    save_json(&target, state)?;
    finish(&target)
}
