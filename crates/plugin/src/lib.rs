//! pdfcraft-plugin — the WebAssembly plug-in host for PdfCraft.
//!
//! This follows the plug-in conventions already established by the other
//! Craft apps (PhotoCraft `docs/plugins.md`, VectorCraft `docs/plugins.md`,
//! which itself follows PhotoCraft): a plug-in is **one `.wasm` file** — no
//! package format, no native code — carrying its manifest inside the
//! module, run in the `wasmi` sandbox with no host imports. One file works
//! on macOS, Windows, Linux and the web build, survives PdfCraft updates
//! as long as its ABI version is supported, and is installed by picking
//! the file (Plugins panel / menu) or by dropping it in the plug-ins
//! folder, exactly like the sibling apps' preference folder.
//!
//! ## ABI v1
//!
//! Same shape as the siblings (`pc_*` in PhotoCraft, `vc_*` in
//! VectorCraft), with the `pdf_` prefix:
//!
//! | Export | Signature | Meaning |
//! |---|---|---|
//! | `memory` | memory | The module's linear memory. |
//! | `pdf_abi_version` | `() -> i32` | Must return `1`. |
//! | `pdf_manifest` | `() -> i64` | `(len << 32) \| ptr` of the manifest JSON. |
//! | `pdf_alloc` | `(size) -> ptr` | A block the host may write, or `0`. |
//! | `pdf_run` | `(input, len, params, params_len) -> i64` | Runs one command; packed output or negative error. |
//!
//! The manifest uses the siblings' common fields (`id`, `name`,
//! `version`, `kind`, `author`, `description`, `params`) — PdfCraft's
//! kind in v1 is `"command"`, and its manifest extension is `commands`
//! (the commands the panel and menu offer), just as VectorCraft extended
//! the common schema for its `effect` kind. The input JSON is
//! `{"command": "…", "context": {…}}`; the output JSON is
//! `{"message": "…", "actions": […]}` with host-interpreted actions
//! ([`Action`]): guests never touch the document directly.
//!
//! Plug-in authors usually write against `pdfcraft-plugin-sdk`; the ABI
//! above is the contract, so any language targeting core WebAssembly
//! works.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The ABI version this host speaks (see `pdf_abi_version`).
pub const API_VERSION: u32 = 1;

/// Instruction budget per call: the siblings' base budget, plus a
/// per-input-byte allowance for document-sized contexts.
const FUEL_BASE: u64 = 50_000_000;
const FUEL_PER_BYTE: u64 = 4_000;

/// Largest module accepted (the siblings' limit).
const MAX_MODULE_BYTES: usize = 32 * 1024 * 1024;

/// Largest JSON payload exchanged with a guest, either direction.
const MAX_PAYLOAD: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("{0}")]
    Invalid(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plugin runtime: {0}")]
    Runtime(String),
}

type Result<T> = std::result::Result<T, PluginError>;

fn invalid(message: impl Into<String>) -> PluginError {
    PluginError::Invalid(message.into())
}

/// The manifest a plug-in embeds and returns through `pdf_manifest`.
/// Common fields follow PhotoCraft/VectorCraft; `commands` is PdfCraft's
/// extension for its `command` kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// 1–64 chars of `A-Z a-z 0-9 . _ -` (reverse-DNS recommended).
    pub id: String,
    pub name: String,
    pub version: String,
    /// PdfCraft v1 hosts `"command"` plug-ins.
    pub kind: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Declared commands (PdfCraft extension), in menu order.
    #[serde(default)]
    pub commands: Vec<Command>,
}

/// One command a plug-in offers (panel button, menu item).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

impl Manifest {
    /// Why this plug-in cannot run on this host, if so.
    pub fn compatibility(&self) -> Option<String> {
        if self.id.is_empty() || self.id.len() > 64 || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            return Some("the manifest id must be 1–64 characters of A-Z a-z 0-9 . _ -".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 64 {
            return Some("the manifest needs a name of 1–64 characters".into());
        }
        if self.kind != "command" {
            return Some(format!("plug-in kind {:?} is not hosted by PdfCraft (v1 hosts \"command\")", self.kind));
        }
        None
    }
}

/// What a plug-in asks the host to do after a run. Guests describe
/// effects; the host applies them through its ordinary paths.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Tell the user something (a toast in the app).
    ShowMessage { text: String },
    /// Set one document information key (title, author, …).
    SetDocumentInfo { key: String, value: String },
    /// Open these PDF bytes as a new document.
    OpenDocument { name: String, data_base64: String },
}

/// A guest's answer to a run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

/// A plug-in found in the plug-ins folder: its file and its manifest.
#[derive(Clone, Debug)]
pub struct InstalledPlugin {
    pub manifest: Manifest,
    pub path: PathBuf,
}

impl InstalledPlugin {
    /// Instantiate the guest (fuel-limited, no imports).
    pub fn load(&self) -> Result<LoadedPlugin> {
        let bytes = std::fs::read(&self.path)?;
        LoadedPlugin::from_bytes(bytes)
    }
}

/// Read a module's manifest without keeping it instantiated.
pub fn read_manifest(wasm: &[u8]) -> Result<Manifest> {
    LoadedPlugin::from_bytes(wasm.to_vec())?.manifest()
}

/// Every `*.wasm` plug-in directly in `dir` (not recursive, like the
/// siblings' plug-ins folder). Files whose manifest cannot be read are
/// reported through `problems` and skipped.
pub fn discover(dir: &Path) -> (Vec<InstalledPlugin>, Vec<String>) {
    let mut plugins = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (plugins, problems);
    };
    let mut files: Vec<PathBuf> =
        entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("wasm")).unwrap_or(false)).collect();
    files.sort();
    for path in files {
        match std::fs::read(&path).map_err(PluginError::from).and_then(|bytes| read_manifest(&bytes)) {
            Ok(manifest) => plugins.push(InstalledPlugin { manifest, path }),
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    (plugins, problems)
}

/// Install one `.wasm` module into the plug-ins folder as `<id>.wasm`,
/// replacing any older copy of the same plug-in. The module is fully
/// validated (ABI, manifest) before anything is written.
pub fn install_module(wasm: &[u8], plugins_dir: &Path) -> Result<Manifest> {
    let manifest = read_manifest(wasm)?;
    if let Some(problem) = manifest.compatibility() {
        return Err(invalid(problem));
    }
    std::fs::create_dir_all(plugins_dir)?;
    std::fs::write(plugins_dir.join(format!("{}.wasm", manifest.id)), wasm)?;
    Ok(manifest)
}

/// Remove an installed plug-in file (`<id>.wasm` in the plug-ins folder).
pub fn uninstall(plugins_dir: &Path, id: &str) -> Result<()> {
    let target = plugins_dir.join(format!("{id}.wasm"));
    if !target.is_file() {
        return Err(invalid(format!("{id:?} is not an installed plug-in")));
    }
    std::fs::remove_file(&target)?;
    Ok(())
}

/// Ids the user disabled, persisted as a JSON array next to the plug-ins.
pub fn load_disabled(dir: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(dir.join("disabled.json")).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Persist the disabled set.
pub fn save_disabled(dir: &Path, disabled: &BTreeSet<String>) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_string_pretty(&disabled.iter().collect::<Vec<_>>())?;
    std::fs::write(dir.join("disabled.json"), text)?;
    Ok(())
}

/// A compiled guest module, ready to run commands. Each run uses a fresh
/// instance, like the siblings: no state survives between calls.
pub struct LoadedPlugin {
    engine: wasmi::Engine,
    module: wasmi::Module,
    manifest: Manifest,
}

impl LoadedPlugin {
    /// Compile and validate a module (ABI version, manifest).
    pub fn from_bytes(wasm: Vec<u8>) -> Result<Self> {
        if wasm.len() > MAX_MODULE_BYTES {
            return Err(invalid("the module is larger than 32 MiB"));
        }
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = wasmi::Engine::new(&config);
        let module = wasmi::Module::new(&engine, &wasm[..]).map_err(|e| PluginError::Runtime(e.to_string()))?;
        let mut plugin = Self {
            engine,
            module,
            manifest: Manifest {
                id: String::new(),
                name: String::new(),
                version: String::new(),
                kind: String::new(),
                author: String::new(),
                description: String::new(),
                commands: Vec::new(),
            },
        };
        plugin.manifest = plugin.read_manifest()?;
        Ok(plugin)
    }

    /// The plug-in's embedded manifest.
    pub fn manifest(&self) -> Result<Manifest> {
        Ok(self.manifest.clone())
    }

    fn instantiate(&self) -> Result<(wasmi::Store<()>, wasmi::Instance)> {
        let mut store = wasmi::Store::new(&self.engine, ());
        store.set_fuel(FUEL_BASE).map_err(|e| PluginError::Runtime(e.to_string()))?;
        let linker = wasmi::Linker::new(&self.engine);
        let instance = linker.instantiate_and_start(&mut store, &self.module).map_err(|e| PluginError::Runtime(e.to_string()))?;
        Ok((store, instance))
    }

    fn read_manifest(&self) -> Result<Manifest> {
        let (mut store, instance) = self.instantiate()?;
        let version = instance
            .get_typed_func::<(), i32>(&store, "pdf_abi_version")
            .map_err(|_| invalid("the module does not export pdf_abi_version (build it with pdfcraft-plugin-sdk)"))?
            .call(&mut store, ())
            .map_err(|e| PluginError::Runtime(e.to_string()))?;
        if version != API_VERSION as i32 {
            return Err(invalid(format!("plug-in ABI {version} is not supported by this PdfCraft (ABI {API_VERSION})")));
        }
        let packed = instance
            .get_typed_func::<(), i64>(&store, "pdf_manifest")
            .map_err(|_| invalid("the module does not export pdf_manifest"))?
            .call(&mut store, ())
            .map_err(|e| PluginError::Runtime(e.to_string()))?;
        let bytes = read_packed(&instance, &store, packed)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Run one command against a host-supplied context.
    pub fn invoke(&self, command: &str, context: &serde_json::Value) -> Result<Response> {
        let input = serde_json::json!({ "command": command, "context": context });
        let payload = serde_json::to_vec(&input)?;
        let output = self.run(&payload, b"{}")?;
        Ok(serde_json::from_slice(&output)?)
    }

    fn run(&self, input: &[u8], params: &[u8]) -> Result<Vec<u8>> {
        if input.len() > MAX_PAYLOAD || params.len() > MAX_PAYLOAD {
            return Err(invalid("the request to the plug-in is too large"));
        }
        let (mut store, instance) = self.instantiate()?;
        let extra = FUEL_PER_BYTE.saturating_mul((input.len() + params.len()) as u64);
        store.set_fuel(FUEL_BASE.saturating_add(extra)).map_err(|e| PluginError::Runtime(e.to_string()))?;
        let memory = instance.get_memory(&store, "memory").ok_or_else(|| invalid("the plug-in does not export its memory"))?;
        let alloc = instance.get_typed_func::<u32, u32>(&store, "pdf_alloc").map_err(|_| invalid("the plug-in does not export pdf_alloc"))?;
        let run =
            instance.get_typed_func::<(u32, u32, u32, u32), i64>(&store, "pdf_run").map_err(|_| invalid("the plug-in does not export pdf_run"))?;
        let write = |store: &mut wasmi::Store<()>, bytes: &[u8]| -> Result<u32> {
            let len = u32::try_from(bytes.len()).map_err(|_| invalid("request too large"))?;
            let ptr = alloc.call(&mut *store, len).map_err(|e| PluginError::Runtime(e.to_string()))?;
            if ptr == 0 && len > 0 {
                return Err(invalid("the plug-in could not allocate its input"));
            }
            memory.write(&mut *store, ptr as usize, bytes).map_err(|e| PluginError::Runtime(e.to_string()))?;
            Ok(ptr)
        };
        let input_ptr = write(&mut store, input)?;
        let params_ptr = write(&mut store, params)?;
        let packed = run
            .call(&mut store, (input_ptr, input.len() as u32, params_ptr, params.len() as u32))
            .map_err(|e| PluginError::Runtime(e.to_string()))?;
        if packed < 0 {
            return Err(invalid(format!("the plug-in failed with error code {}", -packed)));
        }
        read_packed(&instance, &store, packed)
    }
}

/// Read `(len << 32) | ptr` packed bytes out of a guest instance.
fn read_packed(instance: &wasmi::Instance, store: &wasmi::Store<()>, packed: i64) -> Result<Vec<u8>> {
    let len = (packed >> 32) as u32;
    let ptr = packed as u32;
    if len as usize > MAX_PAYLOAD {
        return Err(invalid("the plug-in's answer is too large"));
    }
    let memory = instance.get_memory(store, "memory").ok_or_else(|| invalid("the plug-in does not export its memory"))?;
    let mut buf = vec![0u8; len as usize];
    memory.read(store, ptr as usize, &mut buf).map_err(|e| PluginError::Runtime(e.to_string()))?;
    Ok(buf)
}

/// The plug-ins folder for this installation, following the same scheme
/// as the recovery folder: the portable data folder when portable mode
/// is on, otherwise the per-user application data folder.
pub fn default_plugins_dir(portable_data_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = portable_data_dir {
        return Some(dir.join("Plugins"));
    }
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support/PdfCraft/Plugins"))
    } else if cfg!(windows) {
        env("LOCALAPPDATA").map(|d| d.join("PdfCraft").join("Plugins"))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|d| d.join("pdfcraft/plugins"))
    }
}
