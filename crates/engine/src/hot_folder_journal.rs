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
///
/// The snapshot can be far larger than one journal file may be (a grouped batch gives every
/// source its own copy of the group's outputs), so it is written as parts of at most
/// `PART_BYTES`, named after the snapshot's digest, then committed by a small manifest. A
/// snapshot that was interrupted before its manifest never replaces the previous one.
pub(crate) fn pending_path(target: &Path) -> PathBuf {
    target.with_extension("pending.json")
}

const PART_BYTES: usize = 8 * 1024 * 1024;
/// Bounds what a damaged manifest can make a reader allocate (512 MiB).
const MAX_PARTS: usize = 64;

#[derive(Serialize, Deserialize)]
struct Manifest {
    parts: usize,
    bytes: usize,
    sha256: String,
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `<state>.pending.<digest>.<index>`, beside the manifest `<state>.pending.json`.
fn part_path(target: &Path, digest: &str, index: usize) -> PathBuf {
    target.with_extension(format!("pending.{digest}.{index}"))
}

/// Remove the parts of every snapshot but `keep`. Only names this journal writes are touched.
fn remove_parts(target: &Path, keep: Option<&str>) -> Result<(), String> {
    let (Some(parent), Some(stem)) = (target.parent(), target.file_stem().and_then(|s| s.to_str())) else { return Ok(()) };
    let prefix = format!("{stem}.pending.");
    let manifest = pending_path(target);
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let mut removed = false;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(rest) = name.strip_prefix(&prefix) else { continue };
        let Some((digest, index)) = rest.split_once('.') else { continue };
        if path == manifest || digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) || index.parse::<usize>().is_err() {
            continue;
        }
        if keep != Some(digest) {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
            removed = true;
        }
    }
    if removed { sync_parent(target) } else { Ok(()) }
}

fn read_part(path: &Path, expected: usize) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("pending journal part missing: {e}"))?;
    if is_link(&meta) || !meta.is_file() || meta.len() != expected as u64 {
        return Err("pending journal part is damaged".into());
    }
    let mut bytes = Vec::with_capacity(expected);
    File::open(path).and_then(|file| file.take(expected as u64).read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
    if bytes.len() != expected {
        return Err("pending journal part is damaged".into());
    }
    Ok(bytes)
}

pub(crate) fn pending<T: for<'de> Deserialize<'de>>(target: &Path) -> Result<Option<T>, String> {
    let path = pending_path(target);
    let manifest: Manifest = match fs::symlink_metadata(&path) {
        Ok(meta) if is_link(&meta) || !meta.is_file() => return Err("pending journal must be a regular file".into()),
        Ok(_) => read_json(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let valid_digest = manifest.sha256.len() == 64 && manifest.sha256.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid_digest || manifest.parts == 0 || manifest.parts > MAX_PARTS || manifest.bytes.div_ceil(PART_BYTES) != manifest.parts {
        return Err("pending journal manifest is damaged".into());
    }
    let mut bytes = Vec::with_capacity(manifest.bytes);
    for index in 0..manifest.parts {
        let expected = (manifest.bytes - index * PART_BYTES).min(PART_BYTES);
        bytes.extend(read_part(&part_path(target, &manifest.sha256, index), expected)?);
    }
    if hex_digest(&bytes) != manifest.sha256 {
        return Err("pending journal does not match its manifest".into());
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|e| e.to_string())
}

pub(crate) fn begin<T: Serialize>(target: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let parts = bytes.len().div_ceil(PART_BYTES);
    if parts == 0 || parts > MAX_PARTS {
        return Err("hot folder journal is full; archive the completed task and its configuration".into());
    }
    let digest = hex_digest(&bytes);
    for (index, chunk) in bytes.chunks(PART_BYTES).enumerate() {
        save_bytes(&part_path(target, &digest, index), chunk)?;
    }
    // The manifest is the commit point: until it is replaced, readers keep the previous one.
    save_json(&pending_path(target), &Manifest { parts, bytes: bytes.len(), sha256: digest.clone() })?;
    remove_parts(target, Some(&digest))
}

pub(crate) fn finish(target: &Path) -> Result<(), String> {
    let path = pending_path(target);
    match fs::remove_file(&path) {
        Ok(()) => sync_parent(&path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    remove_parts(target, None)
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

/// The compact JSON length of each `"key":value` member of a map.
pub(super) fn entry_sizes<V: Serialize>(map: &BTreeMap<String, V>) -> Result<BTreeMap<String, usize>, String> {
    map.iter()
        .map(|(key, value)| {
            let key_len = serde_json::to_vec(key).map_err(|e| e.to_string())?.len();
            let value_len = serde_json::to_vec(value).map_err(|e| e.to_string())?.len();
            Ok((key.clone(), key_len.saturating_add(1).saturating_add(value_len)))
        })
        .collect()
}

/// The compact JSON length of a map whose `entries` members total `members` bytes: braces
/// plus a comma between members, exactly what `serde_json::to_vec` writes.
pub(super) fn map_len(members: usize, entries: usize) -> usize {
    members.saturating_add(entries.saturating_sub(1)).saturating_add(2)
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
    // Each entry is serialized once to measure it: re-serializing the whole cache after every
    // spill was quadratic, and minutes long for a large grouped batch.
    let mut receipt_sizes = entry_sizes(&state.receipts)?;
    let mut observation_sizes = entry_sizes(&state.observations)?;
    let mut receipt_bytes: usize = receipt_sizes.values().sum();
    let mut observation_bytes: usize = observation_sizes.values().sum();
    let needs_spill = state.receipts.len() > HOT_ENTRIES
        || state.observations.len() > HOT_ENTRIES
        || map_len(receipt_bytes, state.receipts.len()) > HOT_BYTES
        || map_len(observation_bytes, state.observations.len()) > HOT_BYTES;
    if needs_spill {
        begin(&target, state)?;
    }
    let mut spilled = 0;
    while state.receipts.len() > HOT_ENTRIES || map_len(receipt_bytes, state.receipts.len()) > HOT_BYTES {
        let Some((key, value)) = state.receipts.first_key_value() else { break };
        write(config, "receipts", key, value)?;
        let key = key.clone();
        state.receipts.remove(&key);
        receipt_bytes = receipt_bytes.saturating_sub(receipt_sizes.remove(&key).unwrap_or_default());
        spilled += 1;
        after_spill(spilled)?;
    }
    while state.observations.len() > HOT_ENTRIES || map_len(observation_bytes, state.observations.len()) > HOT_BYTES {
        let Some((key, value)) = state.observations.first_key_value() else { break };
        write(config, "observations", &observation_key(state, key), value)?;
        let key = key.clone();
        state.observations.remove(&key);
        observation_bytes = observation_bytes.saturating_sub(observation_sizes.remove(&key).unwrap_or_default());
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
