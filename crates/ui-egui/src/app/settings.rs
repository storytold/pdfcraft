//! The typed settings the store keeps in `config.toml`, the import of the last release's
//! `app.ron`, and the portable data folder.
//!
//! Every section is tolerant by construction (`#[serde(default)]`, each value optional); the
//! clamps and refusals a value needs live in `PdfCraftApp::apply_settings`, so a hostile
//! `config.toml` and a hostile legacy file reach the same code.

use super::*;
use craft::settings::{LegacyFile, SectionError, Settings};
use craft::theme::{ModeChoice, MotionChoice};
use craft::ui::contract::ColumnLayout;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// Window chrome, reopening, theme and the grids' zoom.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub reopen_last_session: Option<bool>,
    pub restore_last_view: Option<bool>,
    /// JSON text: the per-file view memory.
    pub last_views: Option<String>,
    pub system_title_bar: Option<bool>,
    pub reload_changed_files: Option<bool>,
    pub update_check_at_start: Option<bool>,
    pub show_secured_notice: Option<bool>,
    pub text_links: Option<bool>,
    /// JSON text: the pinned folders.
    pub pinned_folders: Option<String>,
    /// JSON text: which pinned folders are collapsed.
    pub pinned_folders_collapsed: Option<String>,
    pub pinned_tools: Option<Vec<String>>,
    pub theme: Option<ModeChoice>,
    pub scroll_speed: Option<crate::scroll_speed::ScrollSpeed>,
    pub invert_middle_drag: Option<bool>,
    pub animation_effects: Option<MotionChoice>,
    pub default_mode: Option<crate::Mode>,
    pub default_layout: Option<String>,
    pub combine_columns: Option<ColumnLayout>,
    pub combine_view: Option<String>,
    pub combine_zoom: Option<f64>,
    pub organize_zoom: Option<f64>,
}

impl Layout {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "layout";
}

/// The Print dialog's papers, each printer's setup and the choices it keeps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Print {
    /// JSON text: the custom paper sizes.
    pub print_custom_papers: Option<String>,
    pub print_paper: Option<String>,
    pub print_paper_size: Option<[f64; 2]>,
    pub print_paper_overridden: Option<bool>,
    /// JSON text: each printer's setup.
    pub print_printers: Option<String>,
    pub duplex: Option<String>,
    pub grayscale: Option<bool>,
    pub collate: Option<bool>,
    pub custom_scale: Option<f64>,
    pub size: Option<String>,
    pub comments_forms: Option<String>,
    pub binding: Option<String>,
    pub as_image: Option<bool>,
    pub dpi: Option<f64>,
}

impl Print {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "print";
}

/// Display defaults, units, language, Fill & Sign defaults and panel switches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    pub default_page_gaps: Option<bool>,
    /// Full screen fills the screen with one page at a time (off keeps the page display).
    pub full_screen_fill: Option<bool>,
    /// Esc leaves full screen (off leaves it to ⌘L / Ctrl+L only).
    pub full_screen_escape_exits: Option<bool>,
    pub units: Option<crate::units::Unit>,
    pub always_default_view: Option<bool>,
    pub highlight_fields: Option<bool>,
    pub comments_panel_closed: Option<bool>,
    pub right_panel_closed: Option<bool>,
    /// JSON text: the Scan & OCR choices.
    pub ocr: Option<String>,
    pub default_zoom: Option<String>,
    pub language: Option<String>,
    pub flatten_fill_sign: Option<bool>,
    pub ask_to_lock_signatures: Option<bool>,
    pub fill_text_size: Option<f64>,
    pub fill_text_color: Option<[f64; 3]>,
    pub show_page_edge_shadows: Option<bool>,
    pub fill_text_border: Option<f64>,
    pub fill_text_border_shape: Option<String>,
    /// Absent is "the text's colour": TOML has no null.
    pub fill_text_border_color: Option<[f64; 3]>,
    /// JSON text: the Redact defaults.
    pub redact_prefs: Option<String>,
}

impl Behavior {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "behavior";
}

/// Reading colours, interface scale, date format and the comment author.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub reading_colors: Option<pdfcraft_render::ReadingColors>,
    /// Absent follows the system: TOML has no null.
    pub ui_scale: Option<f64>,
    pub date_format: Option<String>,
    pub date_language: Option<String>,
    pub author: Option<String>,
    pub hide_empty_highlight_hover: Option<bool>,
    pub comment_tool_pinned: Option<bool>,
}

impl View {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "view";
}

/// The per-tool comment styles and font sizes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Comment {
    /// JSON text: one entry per tool.
    pub comment_styles: Option<String>,
    /// JSON text: one entry per tool.
    pub comment_font_sizes: Option<String>,
}

impl Comment {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "comment";
}

/// Saved signatures, digital IDs, trusted certificates, stamps and actions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Signature {
    pub signatures: Option<craft::review::ui::signature::SignatureSettings>,
    pub sign_graphic: Option<crate::sign_ui::SignGraphic>,
    pub digital_ids: Option<Vec<crate::DigitalIdEntry>>,
    /// PEM text of each trusted certificate.
    pub trusted: Option<Vec<String>>,
    /// JSON text: the custom stamps.
    pub custom_stamps: Option<String>,
    /// JSON text: the custom actions.
    pub actions: Option<String>,
    pub javascript: Option<bool>,
}

impl Signature {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "signature";
}

/// The side panels' workspace and the rail's side.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dock {
    /// JSON text: the dock workspace.
    pub dock: Option<String>,
    pub rail_side: Option<String>,
}

impl Dock {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "dock";
}

/// The recent files, how Home lists them and how many are kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recent {
    /// JSON text: the recent files, newest first.
    pub recent: Option<String>,
    pub recent_limit: Option<u32>,
    pub recent_view: Option<String>,
    pub recent_sort: Option<String>,
}

impl Recent {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "recent";
}

/// The document hotbar's tools.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotbar {
    pub tools: Option<Vec<String>>,
}

impl Hotbar {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "hotbar";
}

/// The windows to reopen at the next launch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// JSON text of a [`craft::platform::session::Session`]; kept only while reopening is on.
    pub last_session: Option<String>,
}

impl Session {
    /// The table's name in `config.toml`.
    pub const KEY: &'static str = "session";
}

/// Everything PdfCraft keeps for the user.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfCraftSettings {
    pub layout: Layout,
    pub print: Print,
    pub behavior: Behavior,
    pub view: View,
    pub comment: Comment,
    pub signature: Signature,
    pub dock: Dock,
    pub recent: Recent,
    pub hotbar: Hotbar,
    pub session: Session,
}

impl Settings for PdfCraftSettings {
    const APP_ID: &'static str = "pdfcraft";
    const FOLDER_NAME: &'static str = "PdfCraft";
    const SCHEMA_VERSION: u32 = 1;
    const PORTABLE_MARKERS: &'static [&'static str] = &["portable.txt", "PdfCraft.portable"];
    const PORTABLE_DIR: &'static str = "PdfCraftData";
}

/// The settings store the app reads its preferences from and writes them to.
pub type Store = craft::settings::SettingsStore<PdfCraftSettings>;

/// How the data folder was chosen (portable marker, environment, platform folder). Resolved once:
/// it is a fact of how the app was installed, and the settings store opens at the same folder.
/// Native only: the web has no folder rules.
#[cfg(not(target_arch = "wasm32"))]
pub fn resolution() -> &'static craft::settings::Resolved {
    use craft::settings::paths::{self, Portable, Probe};
    static RESOLVED: std::sync::OnceLock<craft::settings::Resolved> = std::sync::OnceLock::new();
    RESOLVED.get_or_init(|| {
        let portable = Portable { markers: PdfCraftSettings::PORTABLE_MARKERS, dir: PdfCraftSettings::PORTABLE_DIR };
        paths::resolve_named(PdfCraftSettings::APP_ID, PdfCraftSettings::FOLDER_NAME, &portable, &craft::settings::Location::Auto, &Probe::default())
    })
}

/// The data folder beside the executable, when a portable marker sits there and the folder can be
/// written; `None` otherwise, and always on the web.
pub fn portable_dir() -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let resolved = resolution();
        if resolved.rule == craft::settings::Rule::Portable { resolved.dir.clone() } else { None }
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// Opens the settings store at the folder [`resolution`] chose (in memory when it chose none),
/// importing the last release's `app.ron` on first run; on the web, over the browser's storage.
pub fn open() -> Store {
    #[cfg(not(target_arch = "wasm32"))]
    let options = {
        let location = match &resolution().dir {
            Some(dir) => craft::settings::Location::Dir(dir.clone()),
            None => craft::settings::Location::Memory,
        };
        craft::settings::OpenOptions::new(location).legacy(legacy_files())
    };
    #[cfg(target_arch = "wasm32")]
    let options = craft::settings::OpenOptions::new(craft::settings::Location::Dir(PathBuf::from(PdfCraftSettings::APP_ID)))
        .files(std::sync::Arc::new(craft::settings::BrowserFiles));
    Store::open_with(options).0
}

/// The OCR Verify screen's user dictionary folder: the portable data folder's, else the
/// platform's per-user one; `None` where files are not kept (the web).
pub fn dictionary_dir() -> Option<PathBuf> {
    crate::recovery::default_dictionary_dir()
}

/// The folders the last releases kept `app.ron` in, nearest first.
fn legacy_folders() -> Vec<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut folders = Vec::new();
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
            folders.extend(config.map(|c| c.join("pdfcraft")));
        }
        folders.extend(eframe::storage_dir("PdfCraft"));
        folders.extend(eframe::storage_dir("PrintCraft"));
        folders
    }
    #[cfg(target_arch = "wasm32")]
    {
        Vec::new()
    }
}

/// The old files to import on first run; none on the web, which has no `app.ron`.
pub fn legacy_files() -> Vec<LegacyFile<PdfCraftSettings>> {
    legacy_files_in(&legacy_folders())
}

/// The `app.ron` of each folder, a PdfCraft entry then a PrintCraft entry, in folder order.
pub fn legacy_files_in(folders: &[PathBuf]) -> Vec<LegacyFile<PdfCraftSettings>> {
    folders
        .iter()
        .flat_map(|folder| {
            let path = folder.join("app.ron");
            [LegacyFile { path: path.clone(), parse: parse_app_ron_pdfcraft }, LegacyFile { path, parse: parse_app_ron_printcraft }]
        })
        .collect()
}

type Parsed = Result<(PdfCraftSettings, Vec<SectionError>), String>;

/// An `app.ron` written by PdfCraft: the JSON string under `pdfcraft`.
fn parse_app_ron_pdfcraft(bytes: &[u8]) -> Parsed {
    parse_app_ron(bytes, "pdfcraft")
}

/// An `app.ron` written by PrintCraft, PdfCraft's former name: the JSON string under `printcraft`.
fn parse_app_ron_printcraft(bytes: &[u8]) -> Parsed {
    parse_app_ron(bytes, "printcraft")
}

fn parse_app_ron(bytes: &[u8], key: &str) -> Parsed {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let json = craft::window::restore::legacy::eframe_app_ron_entry(text, key).ok_or_else(|| format!("no `{key}` entry"))?;
    Ok(import_json(&json))
}

/// The motion choice a saved name stands for: the last release's `on`, `off` and `system`, and the
/// names the framework writes.
pub fn motion_from_name(name: &str) -> Option<MotionChoice> {
    match name {
        "on" | "full" => Some(MotionChoice::Full),
        "reduced" => Some(MotionChoice::Reduced),
        "off" => Some(MotionChoice::Off),
        "system" => Some(MotionChoice::System),
        _ => None,
    }
}

/// The theme choice a saved name stands for: the last release's `Light`, `Dark` and `System`, and
/// the names the framework writes.
pub fn theme_from_name(name: &str) -> Option<ModeChoice> {
    match name {
        "Light" | "light" => Some(ModeChoice::Light),
        "Dark" | "dark" => Some(ModeChoice::Dark),
        "System" | "system" => Some(ModeChoice::System),
        _ => None,
    }
}

/// A restored colour: an `[r, g, b]` array of finite numbers, each clamped to 0–1 (a hostile file
/// must not hand the painter NaN or a huge value).
pub(crate) fn rgb_triple(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let mut out = [0.0; 3];
    for (slot, x) in out.iter_mut().zip(a) {
        let x = x.as_f64().filter(|x| x.is_finite())?;
        *slot = x.clamp(0.0, 1.0);
    }
    Some(out)
}

/// Reads one section's keys from a legacy JSON object. A value of the wrong type stays at its
/// default and is listed; an absent or `null` value is simply unset.
struct Reader<'a> {
    map: &'a Map<String, Value>,
    section: &'static str,
    errors: &'a mut Vec<SectionError>,
}

impl<'a> Reader<'a> {
    fn new(map: &'a Map<String, Value>, section: &'static str, errors: &'a mut Vec<SectionError>) -> Self {
        Self { map, section, errors }
    }

    fn value(&self, key: &str) -> Option<&'a Value> {
        self.map.get(key).filter(|v| !v.is_null())
    }

    fn fail(&mut self, key: &str, message: &str) {
        self.errors.push(SectionError { section: format!("{}.{key}", self.section), message: message.to_string() });
    }

    fn bool(&mut self, key: &str) -> Option<bool> {
        let v = self.value(key)?;
        let b = v.as_bool();
        if b.is_none() {
            self.fail(key, "expected a boolean");
        }
        b
    }

    fn f64(&mut self, key: &str) -> Option<f64> {
        let v = self.value(key)?;
        let n = v.as_f64();
        if n.is_none() {
            self.fail(key, "expected a number");
        }
        n
    }

    fn string(&mut self, key: &str) -> Option<String> {
        let v = self.value(key)?;
        let s = v.as_str().map(str::to_string);
        if s.is_none() {
            self.fail(key, "expected a string");
        }
        s
    }

    /// The value as the JSON text a setting keeps; the section that reads it checks its shape.
    fn text(&self, key: &str) -> Option<String> {
        self.value(key).map(Value::to_string)
    }

    fn typed<T: DeserializeOwned>(&mut self, key: &str) -> Option<T> {
        let v = self.value(key)?;
        match serde_json::from_value(v.clone()) {
            Ok(t) => Some(t),
            Err(e) => {
                self.fail(key, &e.to_string());
                None
            }
        }
    }

    /// A list whose entries that fail to read are dropped and listed.
    fn list<T: DeserializeOwned>(&mut self, key: &str) -> Option<Vec<T>> {
        let v = self.value(key)?;
        let Some(entries) = v.as_array() else {
            self.fail(key, "expected a list");
            return None;
        };
        let mut out = Vec::with_capacity(entries.len().min(1024));
        for (i, e) in entries.iter().enumerate() {
            match serde_json::from_value(e.clone()) {
                Ok(t) => out.push(t),
                Err(err) => self.fail(&format!("{key}[{i}]"), &err.to_string()),
            }
        }
        Some(out)
    }
}

/// The last release's visible-signature choice: a unit name or `{"Image": "<base64 png>"}`.
fn legacy_sign_graphic(v: &Value) -> Option<crate::sign_ui::SignGraphic> {
    use crate::sign_ui::SignGraphic;
    match v.as_str() {
        Some("Name") => Some(SignGraphic::Name),
        Some("FillSign") => Some(SignGraphic::FillSign),
        _ => v.get("Image").and_then(crate::fill_sign::legacy_image).map(SignGraphic::Image),
    }
}

/// The last release's settings JSON as typed settings. Values that fail to read stay at their
/// default and are listed; the text itself failing to parse gives the defaults and one entry.
pub fn import_json(text: &str) -> (PdfCraftSettings, Vec<SectionError>) {
    let mut errors = Vec::new();
    let root = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => map,
        Ok(_) => {
            errors.push(SectionError { section: String::new(), message: "expected an object".to_string() });
            return (PdfCraftSettings::default(), errors);
        }
        Err(e) => {
            errors.push(SectionError { section: String::new(), message: e.to_string() });
            return (PdfCraftSettings::default(), errors);
        }
    };

    let layout = {
        let mut r = Reader::new(&root, Layout::KEY, &mut errors);
        let animation_effects = r.string("animation_effects").and_then(|name| {
            let motion = motion_from_name(&name);
            if motion.is_none() {
                r.fail("animation_effects", "unknown motion name");
            }
            motion
        });
        let theme = r.string("theme").and_then(|name| {
            let theme = theme_from_name(&name);
            if theme.is_none() {
                r.fail("theme", "unknown theme name");
            }
            theme
        });
        let combine_columns = r.value("combine_columns").and_then(crate::combine_ui::columns_from_legacy);
        Layout {
            reopen_last_session: r.bool("reopen_last_session"),
            restore_last_view: r.bool("restore_last_view"),
            last_views: r.text("last_views"),
            system_title_bar: r.bool("system_title_bar"),
            reload_changed_files: r.bool("reload_changed_files"),
            update_check_at_start: r.bool("update_check_at_start"),
            show_secured_notice: r.bool("show_secured_notice"),
            text_links: r.bool("text_links"),
            pinned_folders: r.text("pinned_folders"),
            pinned_folders_collapsed: r.text("pinned_folders_collapsed"),
            pinned_tools: r.list("pinned_tools"),
            theme,
            scroll_speed: r.typed("scroll_speed"),
            invert_middle_drag: r.bool("invert_middle_drag"),
            animation_effects,
            default_mode: r.typed("default_mode"),
            default_layout: r.string("default_layout"),
            combine_columns,
            combine_view: r.string("combine_view"),
            combine_zoom: r.f64("combine_zoom"),
            organize_zoom: r.f64("organize_zoom"),
        }
    };

    // The dialog's remembered choices sat in a `print` object, the papers beside it.
    let mut print_keys = root.clone();
    if let Some(Value::Object(inner)) = root.get("print") {
        print_keys.extend(inner.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let print = {
        let mut r = Reader::new(&print_keys, Print::KEY, &mut errors);
        Print {
            print_custom_papers: r.text("print_custom_papers"),
            print_paper: r.string("print_paper"),
            print_paper_size: r.typed("print_paper_size"),
            print_paper_overridden: r.bool("print_paper_overridden"),
            print_printers: r.text("print_printers"),
            duplex: r.string("duplex"),
            grayscale: r.bool("grayscale"),
            collate: r.bool("collate"),
            custom_scale: r.f64("custom_scale"),
            size: r.string("size"),
            comments_forms: r.string("comments_forms"),
            binding: r.string("binding"),
            as_image: r.bool("as_image"),
            dpi: r.f64("dpi"),
        }
    };

    let behavior = {
        let mut r = Reader::new(&root, Behavior::KEY, &mut errors);
        Behavior {
            default_page_gaps: r.bool("default_page_gaps"),
            full_screen_fill: r.bool("full_screen_fill"),
            full_screen_escape_exits: r.bool("full_screen_escape_exits"),
            units: r.typed("units"),
            always_default_view: r.bool("always_default_view"),
            highlight_fields: r.bool("highlight_fields"),
            comments_panel_closed: r.bool("comments_panel_closed"),
            right_panel_closed: r.bool("right_panel_closed"),
            ocr: r.text("ocr"),
            default_zoom: r.string("default_zoom"),
            language: r.string("language"),
            flatten_fill_sign: r.bool("flatten_fill_sign"),
            ask_to_lock_signatures: r.bool("ask_to_lock_signatures"),
            fill_text_size: r.f64("fill_text_size"),
            fill_text_color: r.typed("fill_text_color"),
            show_page_edge_shadows: r.bool("show_page_edge_shadows"),
            fill_text_border: r.f64("fill_text_border"),
            fill_text_border_shape: r.string("fill_text_border_shape"),
            fill_text_border_color: r.typed("fill_text_border_color"),
            redact_prefs: r.text("redact_prefs"),
        }
    };

    let view = {
        let mut r = Reader::new(&root, View::KEY, &mut errors);
        View {
            reading_colors: r.typed("reading_colors"),
            ui_scale: r.f64("ui_scale"),
            date_format: r.string("date_format"),
            date_language: r.string("date_language"),
            author: r.string("author"),
            hide_empty_highlight_hover: r.bool("hide_empty_highlight_hover"),
            comment_tool_pinned: r.bool("comment_tool_pinned"),
        }
    };

    let comment = {
        let r = Reader::new(&root, Comment::KEY, &mut errors);
        Comment { comment_styles: r.text("comment_styles"), comment_font_sizes: r.text("comment_font_sizes") }
    };

    let signature = {
        let mut r = Reader::new(&root, Signature::KEY, &mut errors);
        // The last release kept the two slots under four top-level keys.
        let has_slots = ["signature", "signature_text", "signature_image", "initials"].iter().any(|key| root.contains_key(*key));
        let signatures = has_slots.then(|| crate::fill_sign::legacy_signatures(&Value::Object(root.clone())));
        let sign_graphic = r.value("sign_graphic").and_then(legacy_sign_graphic);
        Signature {
            signatures,
            sign_graphic,
            digital_ids: r.list("digital_ids"),
            trusted: r.list("trusted"),
            custom_stamps: r.text("custom_stamps"),
            actions: r.text("actions"),
            javascript: r.bool("javascript"),
        }
    };

    // The last release's `hotbar` object keeps a tool list and a docked side; the side belongs to the dock workspace now.
    // PLAN: assumed `hotbar::migrate_legacy(&Value) -> (Vec<String>, Option<String>)` (chrome-w18)
    // PLAN: `side_panels::migrate` (chrome-w13) takes no hotbar side, so the legacy docked side is not carried into the workspace
    let hotbar_tools = match root.get("hotbar") {
        Some(legacy) if legacy.is_object() => Some(crate::hotbar::migrate_legacy(legacy).0),
        _ => None,
    };
    let hotbar = Hotbar { tools: hotbar_tools };

    let dock = {
        let r = Reader::new(&root, Dock::KEY, &mut errors);
        let saved_side = r.text("rail_side").and_then(|t| serde_json::from_str::<String>(&t).ok());
        match r.text("dock") {
            Some(workspace) => Dock { dock: Some(workspace), rail_side: saved_side },
            // Settings from before the dock keep their panels in `side_panels` or `docking`.
            None => match crate::side_panels::migrate(&Value::Object(root.clone())) {
                Some(m) => Dock {
                    dock: serde_json::to_string(&m.workspace).ok(),
                    rail_side: serde_json::to_value(m.rail_side).ok().and_then(|v| v.as_str().map(str::to_string)),
                },
                None => Dock { dock: None, rail_side: saved_side },
            },
        }
    };

    let recent = {
        let mut r = Reader::new(&root, Recent::KEY, &mut errors);
        Recent {
            recent: r.text("recent"),
            recent_limit: r.f64("recent_limit").filter(|n| *n >= 0.0 && n.is_finite() && n.fract() == 0.0).map(|n| n as u32),
            recent_view: r.string("recent_view"),
            recent_sort: r.string("recent_sort"),
        }
    };

    let session = Session { last_session: root.get("last_session").and_then(session_text) };

    (PdfCraftSettings { layout, print, behavior, view, comment, signature, dock, recent, hotbar, session }, errors)
}

/// A saved session as the JSON text of the current [`craft::platform::session::Session`]. The last
/// release listed `files` and the front one by index; the current form lists `documents` and the
/// front one by path.
fn session_text(value: &Value) -> Option<String> {
    use craft::platform::session::Session;
    if !value.is_object() {
        return None;
    }
    let session = if value.get("documents").is_some() {
        Session::from_json(value)
    } else {
        let files = value.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
        let active = match &value["active"] {
            Value::Number(n) => {
                n.as_u64().and_then(|i| usize::try_from(i).ok()).and_then(|i| files.get(i)).and_then(|f| f["path"].as_str()).map(str::to_string)
            }
            Value::String(path) => Some(path.clone()),
            _ => None,
        };
        let documents: Vec<Value> =
            files.iter().map(|f| serde_json::json!({ "path": f["path"], "view": f.get("view").cloned().unwrap_or(Value::Null) })).collect();
        Session::from_json(&serde_json::json!({ "documents": documents, "active": active }))
    };
    serde_json::to_string(&session).ok()
}
