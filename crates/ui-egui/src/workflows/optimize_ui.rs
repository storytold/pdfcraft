//! Optimize PDF ▸ Advanced optimization (Acrobat's PDF Optimizer): Images, Discard Objects,
//! Discard User Data and Clean Up panels. The result is saved as a copy, like Reduce File Size.
//!
//! The optimization (this dialog's, or Reduce File Size's fixed choices) runs on the host's job
//! queue and shows its progress in a notice with a bar and a Cancel button; the copy is saved once
//! it is done.

use std::sync::{Arc, Mutex, PoisonError, RwLock};

use craft::behavior::{ColumnKey, ColumnWidth, DisplayOrder, ItemSource};
use craft::host::queue::{JobQueue, Priority};
use craft::ui::contract::{Bind, ColumnSpec, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _, Sizing};
use craft::ui::dialog::{DialogResult, dialog_id};
use craft::ui::display::Text;
use craft::ui::layout_components::Stack;
use craft::ui::list::ListRow;
use craft::ui::patterns::form_dialogs::{DialogFields, FieldValue, HostedParamDialog, Param, ParamDialog, ParamKind, ParamTab, SharedRows};
use craft::ui::table::Table;
use pdfcraft_engine::optimize::{
    Compatibility, Compression, ImageSettings, MonoCompression, MonoSettings, Preset, QUALITIES, Settings, SpaceCategory, SpaceUse,
};
use pdfcraft_engine::optimizer::{OptimizeStage, Optimized};
use pdfcraft_engine::{DocId, EditError, Hidden, OptimizeReport};

// PLAN: assumed `crate::JobCount` (pdf-print-ocr-w6), `crate::notices::{ProgressKey, ProgressNotice, set_progress, end_progress}` (pdf-chrome-w19),
// `crate::dialogs::host::{open, open_over}(app, dialog[, parent])` (pdf-hub-w2) and the facade paths above (`craft::ui::patterns::form_dialogs`, `craft::ui::table`,
// `craft::ui::display::Text::show`, `Table::show_items(..).response`).
use super::job_count::JobCount;
use crate::PdfCraftApp;
use crate::notices::{self, ProgressKey, ProgressNotice};

/// The dialog's key; its answer is read back by [`pass`].
pub(crate) const OPTIMIZE_KEY: &str = "optimize";
/// The key of the Space Audit stand-alone dialog.
pub(crate) const SPACE_AUDIT_KEY: &str = "space-audit";
/// The key of the Space Audit opened from the Optimizer.
pub(crate) const OPTIMIZER_AUDIT_KEY: &str = "optimize.audit";
/// The instance that keeps the Optimizer's audit apart from the Optimizer, which has the same dialog type.
const AUDIT_INSTANCE: &str = "audit";
/// The footer button that opens the audit.
const AUDIT_ACTION: &str = "audit";

/// Field keys: the image blocks and their fields join as `color.downsample`.
pub(crate) const COLOR: &str = "color";
pub(crate) const GRAY: &str = "gray";
pub(crate) const MONO: &str = "mono";
pub(crate) const DOWNSAMPLE: &str = "downsample";
pub(crate) const TARGET_PPI: &str = "target_ppi";
pub(crate) const ABOVE_PPI: &str = "above_ppi";
pub(crate) const COMPRESSION: &str = "compression";
pub(crate) const QUALITY: &str = "quality";
pub(crate) const MERGE_FRAGMENTS: &str = "merge_fragments";
pub(crate) const COMPATIBILITY: &str = "compatibility";
pub(crate) const PRESET: &str = "preset";
pub(crate) const OBJECT_STREAMS: &str = "object_streams";
/// The preset the settings match none of.
const CUSTOM: &str = "custom";
/// Remove unused objects: always on, shown disabled.
const UNUSED_OBJECTS: &str = "unused_objects";

/// The key of `field` in the image block `block`.
pub(crate) fn key(block: &str, field: &str) -> String {
    format!("{block}.{field}")
}

/// What a toggle on the Discard and Clean Up tabs sets.
enum Effect {
    /// A flag of the settings.
    Setting(fn(&Settings) -> bool, fn(&mut Settings, bool)),
    /// A Remove Hidden Information category.
    Discard(Hidden),
}

/// A check box of the Discard Objects (1), Discard User Data (2) or Clean Up (3) tab.
struct Toggle {
    tab: usize,
    key: &'static str,
    label: &'static str,
    effect: Effect,
}

static TOGGLES: [Toggle; 20] = [
    Toggle {
        tab: 1,
        key: "linksactionsscripts",
        label: "Discard all links, actions and JavaScript",
        effect: Effect::Discard(Hidden::LinksActionsScripts),
    },
    Toggle {
        tab: 1,
        key: "discard_alternate_images",
        label: "Discard alternate images",
        effect: Effect::Setting(|s| s.discard_alternate_images, |s, on| s.discard_alternate_images = on),
    },
    Toggle {
        tab: 1,
        key: "discard_thumbnails",
        label: "Discard embedded page thumbnails",
        effect: Effect::Setting(|s| s.discard_thumbnails, |s, on| s.discard_thumbnails = on),
    },
    Toggle { tab: 1, key: "discard_tags", label: "Discard document tags", effect: Effect::Setting(|s| s.discard_tags, |s, on| s.discard_tags = on) },
    Toggle {
        tab: 1,
        key: "discard_search_index",
        label: "Discard embedded search index",
        effect: Effect::Setting(|s| s.discard_search_index, |s, on| s.discard_search_index = on),
    },
    Toggle {
        tab: 1,
        key: "discard_print_settings",
        label: "Discard embedded print settings",
        effect: Effect::Setting(|s| s.discard_print_settings, |s, on| s.discard_print_settings = on),
    },
    Toggle {
        tab: 1,
        key: "discard_form_actions",
        label: "Discard all form submission, import and reset actions",
        effect: Effect::Setting(|s| s.discard_form_actions, |s, on| s.discard_form_actions = on),
    },
    Toggle { tab: 1, key: "bookmarks", label: "Discard bookmarks", effect: Effect::Discard(Hidden::Bookmarks) },
    Toggle { tab: 1, key: "formfields", label: "Flatten form fields", effect: Effect::Discard(Hidden::FormFields) },
    Toggle { tab: 1, key: "hiddenlayers", label: "Discard hidden layer content", effect: Effect::Discard(Hidden::HiddenLayers) },
    Toggle { tab: 2, key: "comments", label: "Discard all comments, forms and multimedia", effect: Effect::Discard(Hidden::Comments) },
    Toggle { tab: 2, key: "metadata", label: "Discard document information and metadata", effect: Effect::Discard(Hidden::Metadata) },
    Toggle { tab: 2, key: "attachments", label: "Discard all object data (file attachments)", effect: Effect::Discard(Hidden::Attachments) },
    Toggle {
        tab: 2,
        key: "discard_external_refs",
        label: "Discard external cross-references",
        effect: Effect::Setting(|s| s.discard_external_refs, |s, on| s.discard_external_refs = on),
    },
    Toggle { tab: 2, key: "privatedata", label: "Discard private data of other applications", effect: Effect::Discard(Hidden::PrivateData) },
    Toggle { tab: 2, key: "hiddentext", label: "Discard hidden text", effect: Effect::Discard(Hidden::HiddenText) },
    Toggle {
        tab: 3,
        key: "flate_unencoded",
        label: "Use Flate to encode streams that are not encoded",
        effect: Effect::Setting(|s| s.flate_unencoded, |s, on| s.flate_unencoded = on),
    },
    Toggle {
        tab: 3,
        key: "lzw_to_flate",
        label: "In streams that use LZW encoding, use Flate instead",
        effect: Effect::Setting(|s| s.lzw_to_flate, |s, on| s.lzw_to_flate = on),
    },
    Toggle {
        tab: 3,
        key: "remove_invalid_links",
        label: "Remove invalid links and bookmarks",
        effect: Effect::Setting(|s| s.remove_invalid_links, |s, on| s.remove_invalid_links = on),
    },
    Toggle {
        tab: 3,
        key: "remove_unreferenced_dests",
        label: "Remove unreferenced named destinations",
        effect: Effect::Setting(|s| s.remove_unreferenced_dests, |s, on| s.remove_unreferenced_dests = on),
    },
];

/// Compress document structure: its shown value is the effective one, which a PDF 1.4 target forces off.
static OBJECT_STREAMS_TOGGLE: Toggle = Toggle {
    tab: 3,
    key: OBJECT_STREAMS,
    label: "Compress document structure (object streams)",
    effect: Effect::Setting(|s| s.effective_object_streams(), |s, on| s.object_streams = on),
};

fn toggles() -> impl Iterator<Item = &'static Toggle> {
    TOGGLES.iter().chain(std::iter::once(&OBJECT_STREAMS_TOGGLE))
}

/// What the dialog chooses: the settings and the Remove Hidden Information categories to discard.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct OptimizeDraft {
    pub settings: Settings,
    /// Remove Hidden Information categories to discard.
    pub discard: Vec<Hidden>,
}

fn quality_id(compression: Compression) -> String {
    let q = match compression {
        Compression::Jpeg(q) => q,
        _ => 60,
    };
    QUALITIES.iter().min_by_key(|(_, v)| (*v as i32 - q as i32).abs()).map_or(60, |(_, v)| *v).to_string()
}

fn set_image(fields: &mut DialogFields, block: &str, s: &ImageSettings) {
    fields.set(key(block, DOWNSAMPLE), s.downsample);
    fields.set(key(block, TARGET_PPI), s.target_ppi.round() as i64);
    fields.set(key(block, ABOVE_PPI), s.above_ppi.round() as i64);
    fields.set(
        key(block, COMPRESSION),
        FieldValue::Text(
            match s.compression {
                Compression::Jpeg(_) => "jpeg",
                Compression::Flate => "flate",
                Compression::Retain => "retain",
            }
            .into(),
        ),
    );
    fields.set(key(block, QUALITY), FieldValue::Text(quality_id(s.compression)));
}

fn set_mono(fields: &mut DialogFields, m: &MonoSettings) {
    fields.set(key(MONO, DOWNSAMPLE), m.downsample);
    fields.set(key(MONO, TARGET_PPI), m.target_ppi.round() as i64);
    fields.set(key(MONO, ABOVE_PPI), m.above_ppi.round() as i64);
    let id = match m.compression {
        MonoCompression::CcittG4 => "ccitt",
        MonoCompression::Jbig2 => "jbig2",
        MonoCompression::Retain => "retain",
    };
    fields.set(key(MONO, COMPRESSION), FieldValue::Text(id.into()));
}

fn read_image(fields: &DialogFields, block: &str, s: &mut ImageSettings) {
    if let Some(on) = fields.bool(&key(block, DOWNSAMPLE)) {
        s.downsample = on;
    }
    if let Some(v) = fields.int(&key(block, TARGET_PPI)) {
        s.target_ppi = v as f64;
    }
    if let Some(v) = fields.int(&key(block, ABOVE_PPI)) {
        s.above_ppi = v as f64;
    }
    let quality = fields.text(&key(block, QUALITY)).and_then(|q| q.parse::<u8>().ok()).unwrap_or(60);
    match fields.text(&key(block, COMPRESSION)) {
        Some("jpeg") => s.compression = Compression::Jpeg(quality),
        Some("flate") => s.compression = Compression::Flate,
        Some("retain") => s.compression = Compression::Retain,
        _ => {}
    }
}

fn read_mono(fields: &DialogFields, m: &mut MonoSettings) {
    if let Some(on) = fields.bool(&key(MONO, DOWNSAMPLE)) {
        m.downsample = on;
    }
    if let Some(v) = fields.int(&key(MONO, TARGET_PPI)) {
        m.target_ppi = v as f64;
    }
    if let Some(v) = fields.int(&key(MONO, ABOVE_PPI)) {
        m.above_ppi = v as f64;
    }
    match fields.text(&key(MONO, COMPRESSION)) {
        Some("ccitt") => m.compression = MonoCompression::CcittG4,
        Some("jbig2") => m.compression = MonoCompression::Jbig2,
        Some("retain") => m.compression = MonoCompression::Retain,
        _ => {}
    }
}

impl OptimizeDraft {
    /// The dialog's values for this draft, Preset included.
    fn to_fields(&self) -> DialogFields {
        let s = &self.settings;
        let mut fields = DialogFields::new();
        fields.set(PRESET, FieldValue::Text(Preset::of(s).map_or(CUSTOM, Preset::name).into()));
        fields.set(COMPATIBILITY, FieldValue::Text(s.compatibility.name().into()));
        fields.set(MERGE_FRAGMENTS, s.merge_fragments);
        set_image(&mut fields, COLOR, &s.color);
        set_image(&mut fields, GRAY, &s.gray);
        set_mono(&mut fields, &s.mono);
        for toggle in toggles() {
            let on = match &toggle.effect {
                Effect::Setting(get, _) => get(s),
                Effect::Discard(h) => self.discard.contains(h),
            };
            fields.set(toggle.key, on);
        }
        fields
    }

    /// The draft the dialog's values stand for; a value the dialog lacks keeps the default.
    pub(crate) fn from_fields(fields: &DialogFields) -> Self {
        let mut s = Settings::default();
        if let Some(c) = fields.text(COMPATIBILITY).and_then(|n| Compatibility::ALL.into_iter().find(|c| c.name() == n)) {
            s.compatibility = c;
        }
        if let Some(on) = fields.bool(MERGE_FRAGMENTS) {
            s.merge_fragments = on;
        }
        read_image(fields, COLOR, &mut s.color);
        read_image(fields, GRAY, &mut s.gray);
        read_mono(fields, &mut s.mono);
        let mut discard = Vec::new();
        for toggle in toggles() {
            let Some(on) = fields.bool(toggle.key) else { continue };
            match &toggle.effect {
                Effect::Setting(_, set) => set(&mut s, on),
                Effect::Discard(h) if on => discard.push(*h),
                Effect::Discard(_) => {}
            }
        }
        Self { settings: s, discard }
    }
}

/// A resolution to downsample "for images above" is never below the target.
fn raise_above_ppi(s: &mut Settings) {
    s.color.above_ppi = s.color.above_ppi.max(s.color.target_ppi);
    s.gray.above_ppi = s.gray.above_ppi.max(s.gray.target_ppi);
    s.mono.above_ppi = s.mono.above_ppi.max(s.mono.target_ppi);
}

/// Applies the dialog's own rules to its values: a Preset pick replaces every setting, any other edit moves Preset to the
/// preset the settings now match (Custom when none), and the resolutions and object streams stay consistent.
fn follow_edits(dialog: &mut HostedParamDialog) {
    let edited = dialog.take_edited();
    let fields = dialog.fields();
    let mut draft = OptimizeDraft::from_fields(fields);
    if edited.iter().any(|k| k == PRESET)
        && let Some(preset) = fields.text(PRESET).and_then(Preset::from_name)
    {
        draft.settings = preset.settings();
    }
    raise_above_ppi(&mut draft.settings);
    let next = draft.to_fields();
    let changes: Vec<(String, FieldValue)> =
        next.iter().filter(|(k, v)| dialog.fields().get(k) != Some(*v)).map(|(k, v)| (k.to_string(), v.clone())).collect();
    for (k, v) in changes {
        dialog.set_field(k, v);
    }
}

fn bool_row(key: impl Into<String>, label: &str, default: bool) -> Param {
    Param::new(key, tl!(label).to_string(), ParamKind::bool(default))
}

fn ppi_row(block: &str, field: &str, label: &str, default: f64) -> Param {
    Param::new(key(block, field), tl!(label).to_string(), ParamKind::int(Some(9), Some(2400), default.round() as i64))
        .enabled_when(key(block, DOWNSAMPLE), true)
}

fn choice(options: Vec<(String, String)>, default: &str) -> ParamKind {
    ParamKind::choice(options.into_iter().map(|(id, label)| (id, label.into())).collect(), Some(default.to_string()))
}

/// The Color or Grayscale Images block.
fn image_rows(block: &str, title: &str, defaults: &ImageSettings) -> Vec<Param> {
    let jpeg = FieldValue::Text("jpeg".into());
    let options = [("jpeg", "JPEG"), ("flate", tl!("ZIP")), ("retain", tl!("Retain existing"))];
    vec![
        Param::section(tl!(title).to_string()),
        bool_row(key(block, DOWNSAMPLE), "Bicubic downsampling to", defaults.downsample),
        ppi_row(block, TARGET_PPI, "Resolution (ppi)", defaults.target_ppi),
        ppi_row(block, ABOVE_PPI, "For images above (ppi)", defaults.above_ppi),
        Param::new(
            key(block, COMPRESSION),
            tl!("Compression"),
            choice(options.iter().map(|(id, l)| (id.to_string(), l.to_string())).collect(), "jpeg"),
        ),
        Param::new(
            key(block, QUALITY),
            tl!("Quality"),
            choice(QUALITIES.iter().map(|(n, v)| (v.to_string(), tl!(n).to_string())).collect(), &quality_id(defaults.compression)),
        )
        .visible_when(key(block, COMPRESSION), jpeg),
    ]
}

/// The Monochrome Images block.
fn mono_rows(defaults: &MonoSettings) -> Vec<Param> {
    let options = [("ccitt", tl!("CCITT Group 4")), ("jbig2", tl!("JBIG2 (lossless)")), ("retain", tl!("Retain existing"))];
    vec![
        Param::section(tl!("Monochrome Images")),
        bool_row(key(MONO, DOWNSAMPLE), "Downsample to", defaults.downsample),
        ppi_row(MONO, TARGET_PPI, "Resolution (ppi)", defaults.target_ppi),
        ppi_row(MONO, ABOVE_PPI, "For images above (ppi)", defaults.above_ppi),
        Param::new(
            key(MONO, COMPRESSION),
            tl!("Compression"),
            choice(options.iter().map(|(id, l)| (id.to_string(), l.to_string())).collect(), "ccitt"),
        ),
    ]
}

fn toggle_rows(tab: usize) -> Vec<Param> {
    let defaults = Settings::default();
    let mut rows: Vec<Param> = toggles()
        .filter(|t| t.tab == tab)
        .map(|t| {
            let on = match &t.effect {
                Effect::Setting(get, _) => get(&defaults),
                Effect::Discard(_) => false,
            };
            let row = bool_row(t.key, t.label, on);
            if t.key == OBJECT_STREAMS {
                let allowed = Compatibility::ALL.into_iter().filter(|c| *c != Compatibility::Pdf1_4).map(|c| FieldValue::Text(c.name().into()));
                row.enabled_when_in(COMPATIBILITY, allowed)
            } else {
                row
            }
        })
        .collect();
    if tab == 3 {
        rows.push(bool_row(UNUSED_OBJECTS, "Remove unused objects and merge identical ones", true).disabled());
    }
    rows
}

/// The Optimizer's dialog over its values: Preset and Compatibility above four tabs.
fn build_optimizer(fields: &mut DialogFields) -> ParamDialog<'_> {
    let defaults = Settings::default();
    let preset = [("standard", tl!("Standard")), ("mobile", tl!("Mobile")), (CUSTOM, tl!("Custom"))];
    let compatibility: Vec<(String, String)> = Compatibility::ALL
        .into_iter()
        .map(|c| (c.name().to_string(), if c == Compatibility::Retain { tl!("Retain existing").to_string() } else { format!("PDF {}", c.name()) }))
        .collect();
    let shared = vec![
        Param::new(PRESET, tl!("Settings"), choice(preset.iter().map(|(id, l)| (id.to_string(), l.to_string())).collect(), "standard")),
        Param::new(COMPATIBILITY, tl!("Compatibility"), choice(compatibility, Compatibility::Retain.name())),
    ];
    let mut images = vec![bool_row(MERGE_FRAGMENTS, "Merge image fragments", defaults.merge_fragments)];
    images.extend(image_rows(COLOR, "Color Images", &defaults.color));
    images.extend(image_rows(GRAY, "Grayscale Images", &defaults.gray));
    images.extend(mono_rows(&defaults.mono));
    let tabs = vec![
        ParamTab::new(tl!("Images"), images),
        ParamTab::new(tl!("Discard Objects"), toggle_rows(1)),
        ParamTab::new(tl!("Discard User Data"), toggle_rows(2)),
        ParamTab::new(tl!("Clean Up"), toggle_rows(3)),
    ];
    ParamDialog::new(OPTIMIZE_KEY, tl!("PDF Optimizer"), shared)
        .tabs(tabs)
        .shared_rows(SharedRows::Above)
        .extra_buttons(vec![(AUDIT_ACTION.to_string(), tl!("Audit space usage…").into())])
        .footer_note(tl!("Each image is measured where pages draw it; an image is replaced only if the result is smaller."))
        .values(Bind::bound(fields))
}

/// Opens PDF Optimizer over the defaults.
pub(crate) fn open_optimizer(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let dialog = HostedParamDialog::new(ctx, OPTIMIZE_KEY, OptimizeDraft::default().to_fields(), build_optimizer);
    crate::dialogs::host::open(app, dialog);
}

const DESCRIPTION: ColumnKey = ColumnKey::new("description");
const BYTES: ColumnKey = ColumnKey::new("bytes");
const PERCENTAGE: ColumnKey = ColumnKey::new("percentage");

/// One row of the audit table.
struct AuditLine {
    description: String,
    bytes: String,
    percentage: String,
}

/// The audit's rows: each kind that takes bytes, then the total.
fn audit_lines(rows: &[SpaceUse]) -> Vec<AuditLine> {
    let mut lines: Vec<AuditLine> = rows
        .iter()
        .filter(|r| r.bytes > 0)
        .map(|r| {
            // "Patterns" also names the redaction search patterns ("模式"); the audit's PDF graphics objects need their own key.
            let description = if r.category == SpaceCategory::Patterns {
                tl!("Patterns (graphics objects)").to_string()
            } else {
                tl!(r.category.label()).to_string()
            };
            AuditLine { description, bytes: r.bytes.to_string(), percentage: format!("{:.2}%", r.percent) }
        })
        .collect();
    let total: u64 = rows.iter().map(|r| r.bytes).sum();
    lines.push(AuditLine { description: tl!("Total").to_string(), bytes: total.to_string(), percentage: "100.00%".to_string() });
    lines
}

struct AuditRows<'a>(&'a [AuditLine]);

impl DisplayOrder<usize> for AuditRows<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0.len()).then_some(*key)
    }

    fn text(&self, at: usize) -> Option<std::borrow::Cow<'_, str>> {
        self.0.get(at).map(|line| std::borrow::Cow::Borrowed(line.description.as_str()))
    }
}

impl ItemSource<usize> for AuditRows<'_> {}

/// The audit's explanation over its table, read from `rows` each pass. No rows show until the measure result arrives.
fn audit_row(rows: Arc<RwLock<Vec<SpaceUse>>>) -> Param {
    Param::custom("space-audit", tl!("Space Audit"), move |ui, _fields, _cx| {
        let lines = audit_lines(&rows.read().unwrap_or_else(PoisonError::into_inner));
        let shown = Stack::new().fill_width().show(ui, |ui, _| {
            Text::new(tl!("What takes up the space in this file, by kind of content.")).show(ui);
            let columns = vec![
                ColumnSpec::new(DESCRIPTION, tl!("Description"), ColumnWidth::Content),
                ColumnSpec::new(BYTES, tl!("Bytes"), ColumnWidth::Content).numeric(),
                ColumnSpec::new(PERCENTAGE, tl!("Percentage"), ColumnWidth::Content).numeric(),
            ];
            Table::<usize>::new(tl!("Space Audit"), columns)
                .select(SelectMode::None)
                .sizing(Sizing::Content)
                .natural_height(true)
                .striped(true)
                .show_items(ui, &AuditRows(&lines), &mut |at, icx| {
                    let Some(line) = lines.get(*at) else { return ListRow::new("") };
                    ListRow::new(line.description.clone())
                        .cells(icx.cells())
                        .striped(icx.row.striped)
                        .cell(BYTES, line.bytes.clone())
                        .cell(PERCENTAGE, line.percentage.clone())
                })
                .response
        });
        shown.inner.unwrap_or(shown.output)
    })
    .full_width()
}

/// The Space Audit row, for a dialog that shows the audit among other rows (Properties ▸ Space Audit): reads the app's shared
/// result each pass. It starts no measure, so a dialog may build it every pass; `properties.rs` runs `measure_space_audit`
/// when Properties ▸ Space Audit opens and again after an edit.
pub(crate) fn space_audit_param(app: &PdfCraftApp) -> Param {
    audit_row(app.space_audit.clone())
}

/// Opens the audit dialog: over the Optimizer when `from_optimizer`, so its values survive, else alone.
pub(crate) fn open_space_audit(app: &mut PdfCraftApp, ctx: &egui::Context, from_optimizer: bool) {
    app.measure_space_audit();
    let key = if from_optimizer { OPTIMIZER_AUDIT_KEY } else { SPACE_AUDIT_KEY };
    let rows = app.space_audit.clone();
    let dialog = HostedParamDialog::new(ctx, key, DialogFields::new(), move |fields| {
        ParamDialog::new(key, tl!("Space Audit"), vec![audit_row(rows.clone())]).read_only(true).values(Bind::bound(fields))
    });
    if from_optimizer {
        crate::dialogs::host::open_over(app, dialog.instance(AUDIT_INSTANCE));
    } else {
        crate::dialogs::host::open(app, dialog);
    }
}

/// The dialogs' side of a frame: applies the Optimizer's rules to its edits, opens the audit it asks for, runs the optimization it
/// answers with, and takes in the measure's result.
pub(crate) fn pass(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let optimizer = dialog_id(OPTIMIZE_KEY);
    let mut audit = false;
    if let Some(dialog) = app.dialogs.get_mut_by_id::<HostedParamDialog>(optimizer) {
        follow_edits(dialog);
        audit = dialog.take_action().as_deref() == Some(AUDIT_ACTION);
    }
    if audit {
        open_space_audit(app, ctx, true);
    }
    if let Some(DialogResult::Ok(result)) = app.dialogs.take_by_id::<HostedParamDialog>(optimizer) {
        app.optimize_with_draft(ctx, &OptimizeDraft::from_fields(&result.fields));
    }
    // The audit only reads: its answer, OK or Cancel, closes it and nothing else.
    // The Optimizer's audit is an instance, whose id differs from its key's.
    for id in [dialog_id((OPTIMIZER_AUDIT_KEY, AUDIT_INSTANCE)), dialog_id(SPACE_AUDIT_KEY)] {
        let _ = app.dialogs.take_by_id::<HostedParamDialog>(id);
    }
    #[cfg(target_arch = "wasm32")]
    app.space_audit_jobs.run_inline(1);
    while let Some(done) = app.space_audit_jobs.try_recv() {
        *app.space_audit.write().unwrap_or_else(PoisonError::into_inner) = done.result;
        app.space_audit_of = Some(done.key);
    }
}

/// Which command started an optimization: they save under different names, and only the
/// Optimizer's notice lists what it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizeKind {
    /// Reduce File Size (Acrobat's defaults).
    Reduce,
    /// Optimize PDF ▸ Advanced optimization (the dialog's choices).
    Advanced,
}

impl OptimizeKind {
    /// The suffix of the saved copy's name ("notes (reduced).pdf").
    pub(crate) fn suffix(self) -> &'static str {
        match self {
            OptimizeKind::Reduce => "reduced",
            OptimizeKind::Advanced => "optimized",
        }
    }
}

/// A running optimization: what the job reports while it runs.
pub struct OptimizeRun {
    pub doc: DocId,
    pub kind: OptimizeKind,
    /// Where the copy goes, if the command asked first. Otherwise it asks when the run is done.
    pub destination: Option<String>,
    /// The page or image count the job has reached; its changes move the progress card.
    pub progress: Arc<JobCount>,
    /// The stage the job is in, which names the card's text (`JobCount` carries a count only).
    pub stage: Arc<Mutex<OptimizeStage>>,
}

/// The queue of optimization runs, keyed by document: one run per document, and one at a time by the app's own rule.
pub(crate) type OptimizeJobs = JobQueue<DocId, Result<Optimized, EditError>>;

/// The queue of space audits, keyed by document and edit generation (a newer edit supersedes an older audit).
pub(crate) type SpaceAuditJobs = JobQueue<(DocId, u64), Vec<pdfcraft_engine::optimize::SpaceUse>>;

/// The notice's text for a stage.
fn stage_label(stage: OptimizeStage) -> String {
    match stage {
        OptimizeStage::Discarding => tl!("Optimizing… removing user data").to_string(),
        OptimizeStage::Reading { done: 0, .. } => tl!("Optimizing… reading the pages").to_string(),
        OptimizeStage::Reading { done, total } => crate::i18n::fmt(
            tl!("Optimizing… reading page {d} of {t}"),
            &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
        ),
        OptimizeStage::Fragments => tl!("Optimizing… merging image fragments").to_string(),
        OptimizeStage::Images { done, total } => crate::i18n::fmt(
            tl!("Optimizing… image {d} of {t}"),
            &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
        ),
        OptimizeStage::CleaningUp => tl!("Optimizing… cleaning up").to_string(),
        OptimizeStage::Merging => tl!("Optimizing… merging identical objects").to_string(),
        OptimizeStage::Writing => tl!("Optimizing… writing the copy").to_string(),
    }
}

/// The count a stage has reached, for the run's `JobCount`.
fn stage_counts(stage: OptimizeStage) -> (usize, usize) {
    match stage {
        OptimizeStage::Reading { done, total } | OptimizeStage::Images { done, total } => (done, total),
        _ => (0, 0),
    }
}

/// What the saved notice adds after the size: images optimized and items discarded.
fn summary(r: &OptimizeReport) -> String {
    let o = &r.optimize;
    let lang = crate::i18n::current();
    let mut parts = Vec::new();
    let images = o.images_resampled + o.images_recompressed;
    if images > 0 {
        parts.push(crate::i18n::trn(lang, images as u64, "{n} image optimized", "{n} images optimized"));
    }
    let discarded: usize = r.discarded.iter().map(|(_, n)| n).sum();
    if discarded > 0 {
        parts.push(crate::i18n::trn(lang, discarded as u64, "{n} item discarded", "{n} items discarded"));
    }
    if parts.is_empty() { String::new() } else { format!("; {}", parts.join(", ")) }
}

impl PdfCraftApp {
    /// Measure the active document's space use in the background. With no document the audit is empty.
    pub(crate) fn measure_space_audit(&mut self) {
        let Some((_, id)) = self.active_ids() else {
            self.space_audit.write().unwrap_or_else(PoisonError::into_inner).clear();
            self.space_audit_of = None;
            return;
        };
        let Some(doc) = self.session.get(id) else { return };
        let key = (id, doc.edit_generation());
        let job = doc.space_audit_job();
        self.space_audit_jobs.submit(key, Priority::BACKGROUND, move |_| job.run());
    }

    /// Optimize PDF with the dialog's choices in the background; the copy is saved when done.
    pub(crate) fn optimize_with_draft(&mut self, ctx: &egui::Context, draft: &OptimizeDraft) {
        self.start_optimize(ctx, OptimizeKind::Advanced, &draft.settings, &draft.discard);
    }

    /// Run an optimization of the active document in the background, showing its progress; the
    /// copy is saved when it is done.
    pub(crate) fn start_optimize(&mut self, ctx: &egui::Context, kind: OptimizeKind, settings: &Settings, discard: &[Hidden]) {
        let Some((_, id)) = self.active_ids() else { return };
        self.run_optimize(ctx, id, kind, settings, discard, None);
    }

    /// [`Self::start_optimize`] for document `id`, saving to `destination` if that's known already.
    pub(crate) fn run_optimize(
        &mut self,
        ctx: &egui::Context,
        id: DocId,
        kind: OptimizeKind,
        settings: &Settings,
        discard: &[Hidden],
        destination: Option<String>,
    ) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing(ctx) {
            return;
        }
        if self.optimize_run.is_some() {
            self.notify_tr("Optimization is already running");
            return;
        }
        // Both jobs share the progress card and capture the same document state.
        if self.scan_clean_run.is_some() {
            self.notify_tr("Scanned pages are already being enhanced");
            return;
        }
        let job = match self.session.optimize_job(id, settings, discard) {
            Ok(job) => job,
            Err(e) => return self.save_optimized(id, kind, Err(e), destination),
        };
        let progress = Arc::new(JobCount::default());
        let stage = Arc::new(Mutex::new(OptimizeStage::default()));
        let (count, shown) = (progress.clone(), stage.clone());
        self.optimize_jobs.submit(id, Priority::BACKGROUND, move |cancel| {
            #[cfg(not(target_arch = "wasm32"))]
            pdfcraft_engine::optimize::give_way_to_other_programs();
            job.run(|now| {
                *shown.lock().unwrap_or_else(PoisonError::into_inner) = now;
                let (done, total) = stage_counts(now);
                count.set(done, total);
                !cancel.is_cancelled() && !count.cancelled()
            })
        });
        self.optimize_run = Some(OptimizeRun { doc: id, kind, destination, progress, stage });
        self.show_optimize_stage();
    }

    /// Puts the run's stage on the progress card.
    fn show_optimize_stage(&mut self) {
        let Some(run) = self.optimize_run.as_ref() else { return };
        let stage = *run.stage.lock().unwrap_or_else(PoisonError::into_inner);
        let cancelling = run.progress.cancelled();
        let notice = ProgressNotice {
            label: if cancelling { tl!("Cancelling…").to_string() } else { stage_label(stage) },
            fraction: stage.fraction(),
            cancellable: !cancelling,
        };
        notices::set_progress(self, ProgressKey::Optimize, notice);
    }

    /// Stop a running optimization: its result is discarded and nothing is saved.
    pub fn cancel_optimize(&mut self) {
        let Some(run) = &self.optimize_run else { return };
        run.progress.cancel();
        self.show_optimize_stage();
    }

    /// Show progress; save the copy once the job is done.
    pub(crate) fn poll_optimize(&mut self) {
        // Without lanes (wasm32) the owner runs one job per pass.
        #[cfg(target_arch = "wasm32")]
        self.optimize_jobs.run_inline(1);
        while let Some(done) = self.optimize_jobs.try_recv() {
            let Some(run) = self.optimize_run.as_ref().filter(|run| run.doc == done.key) else { continue };
            let (doc, kind, destination) = (run.doc, run.kind, run.destination.clone());
            self.optimize_run = None;
            notices::end_progress(self, ProgressKey::Optimize);
            match done.result {
                Err(EditError::Cancelled) => self.notify_tr("Optimization cancelled"),
                result => {
                    let result = result.map(|(b, r)| {
                        let detail = if kind == OptimizeKind::Advanced { summary(&r) } else { String::new() };
                        (b, detail)
                    });
                    self.save_optimized(doc, kind, result, destination);
                }
            }
            return;
        }
        let Some(run) = self.optimize_run.as_ref() else { return };
        if run.progress.take_change().is_some() {
            self.show_optimize_stage();
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Cancelling while a queued optimization is waiting must reach its worker before it saves a copy.
    #[test]
    fn cancelling_a_queued_optimization_stops_the_worker() {
        let jobs = Arc::new(craft::host::ManualJobs::new());
        let host = craft::host::Host::builder(craft::host::AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" }).jobs(jobs.clone()).build();
        let mut app = PdfCraftApp::for_unit_tests_on(Arc::new(host));
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "notes.txt", None, b"hello".to_vec()).unwrap();
        let (_, id) = app.active_ids().unwrap();
        app.run_optimize(&ctx, id, OptimizeKind::Advanced, &Settings::default(), &[], None);
        app.cancel_optimize();
        jobs.run_all();
        let done = app.optimize_jobs.try_recv().expect("the optimization reported its outcome");
        assert!(matches!(done.result, Err(EditError::Cancelled)), "the worker must observe the cancellation");
    }

    #[test]
    fn the_progress_card_shows_the_stage_and_cancels() {
        let progress = Arc::new(JobCount::default());
        progress.set(3, 10);
        let stage = Arc::new(Mutex::new(OptimizeStage::Images { done: 3, total: 10 }));
        let (p, s) = (progress.clone(), stage.clone());
        let jobs = Arc::new(craft::host::ManualJobs::new());
        let host_jobs = jobs.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let host = craft::host::Host::builder(craft::host::AppInfo { id: "pdfcraft", name: "PdfCraft", version: "0" })
                    .jobs(host_jobs.clone())
                    .build();
                let mut app = PdfCraftApp::for_unit_tests_on(Arc::new(host));
                app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                app.open_bytes(&egui::Context::default(), "notes.txt", None, b"hello".to_vec()).unwrap();
                let (_, doc) = app.active_ids().unwrap();
                app.optimize_jobs.submit(doc, Priority::BACKGROUND, |_| Err(EditError::Cancelled));
                app.optimize_run = Some(OptimizeRun { doc, kind: OptimizeKind::Advanced, destination: None, progress: p, stage: s });
                app
            }
        });
        // 1.5 s (kittest steps a quarter second): the bar has eased in.
        h.run_steps(6);
        if let Ok(path) = std::env::var("PDFCRAFT_OPTIMIZE_PROGRESS_SHOT") {
            h.render().unwrap().save(path).unwrap();
        }
        h.get_by_label_contains("Optimizing… image 4 of 10");

        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(progress.cancelled(), "the worker was asked to stop");
        assert!(h.query_all_by_label("Optimizing, Cancelling…").next().is_some());
        assert!(h.query_by_label("Cancel").is_none(), "Cancel is not offered twice");
        jobs.run_all();
        h.run_steps(2);
        assert!(h.state().optimize_run.is_none(), "the run ended");
        assert!(h.query_by_label("Cancel").is_none(), "the card and its Cancel are gone");
        assert!(h.query_by_label_contains("Optimizing… image 4 of 10").is_none());
        assert!(h.query_all_by_label("Optimization cancelled").next().is_some());
    }
}
