//! Workbooks.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gridcraft_core::DateSystem;
use gridcraft_formula::Expr;
use serde::{Deserialize, Serialize};

use crate::cell::Formula;
use crate::sheet::Sheet;
use crate::style::{StyleTable, Theme};

/// Most equation source, in bytes, one [`Workbook::refresh_equations`] lays out: about 2,500
/// typical equations, or a few seconds of the slowest 4 KB sources.
pub const MAX_EQUATION_LAYOUT_BYTES: usize = 128 * 1024;

/// Why an equation past [`MAX_EQUATION_LAYOUT_BYTES`] has no layout.
pub const EQUATION_BUDGET_MESSAGE: &str = "This workbook has more equations than are laid out at once; edit this equation to lay it out.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalcMode {
    #[default]
    Automatic,
    AutomaticExceptTables,
    Manual,
}

/// Options › Advanced › "For objects, show" (`<workbookPr showObjects>`, ECMA-376 `ST_Objects`):
/// whether charts, pictures, shapes and slicers are drawn. Ctrl+6 switches between `All` and
/// `None`; `Placeholders` only comes from files and is kept as it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShowObjects {
    #[default]
    All,
    Placeholders,
    None,
}

/// The workbook window's display settings from `<workbookView>` (Excel: File › Options ›
/// Advanced, "Display options for this workbook"): whether the window shows the sheet tabs
/// and the two scroll bars, and how much of the horizontal bar the tabs take (`tabRatio`,
/// thousandths of the bar; the format's default is 600). XLSX I/O normalizes this
/// share to 0–1000, including values supplied directly through the model.
///
/// The values are modelled so an open + save keeps them — a workbook Excel saved with the
/// tabs hidden must not come back with them shown and the settings gone. The app does not
/// act on them yet and there is no Options control for them; that is the other half of
/// storytold/gridcraft#3057.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbookView {
    #[serde(default = "yes")]
    pub show_sheet_tabs: bool,
    #[serde(default = "yes")]
    pub show_horizontal_scroll: bool,
    #[serde(default = "yes")]
    pub show_vertical_scroll: bool,
    #[serde(default = "default_tab_ratio")]
    pub tab_ratio: u32,
}

fn yes() -> bool {
    true
}

fn default_tab_ratio() -> u32 {
    600
}

impl Default for WorkbookView {
    fn default() -> Self {
        WorkbookView { show_sheet_tabs: true, show_horizontal_scroll: true, show_vertical_scroll: true, tab_ratio: default_tab_ratio() }
    }
}

impl WorkbookView {
    /// The entire bar, in thousandths: the upper bound used by XLSX I/O.
    pub const MAX_TAB_RATIO: u32 = 1000;

    fn is_default(&self) -> bool {
        *self == WorkbookView::default()
    }
}

/// File › Options › Formulas › Calculation options, stored per workbook (`<calcPr>`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalcSettings {
    pub mode: CalcMode,
    /// Enable iterative calculation: circular references are calculated repeatedly instead of
    /// reporting an error.
    pub iterative: bool,
    /// Maximum Iterations, 1 to [`CalcSettings::MAX_ITERATIONS`].
    pub max_iterations: u32,
    /// Maximum Change: iteration stops once no value in a circular reference changes by more.
    pub max_change: f64,
    /// Use precision as displayed.
    pub precision_as_displayed: bool,
    /// R1C1 reference style (`<calcPr refMode="R1C1"/>`): kept with the file and written back;
    /// GridCraft itself shows and takes A1 references.
    #[serde(default)]
    pub r1c1: bool,
}

impl Default for CalcSettings {
    fn default() -> Self {
        CalcSettings {
            mode: CalcMode::Automatic,
            iterative: false,
            max_iterations: Self::DEFAULT_ITERATIONS,
            max_change: Self::DEFAULT_CHANGE,
            precision_as_displayed: false,
            r1c1: false,
        }
    }
}

impl CalcSettings {
    pub const DEFAULT_ITERATIONS: u32 = 100;
    pub const DEFAULT_CHANGE: f64 = 0.001;
    /// The largest Maximum Iterations Excel accepts.
    pub const MAX_ITERATIONS: u32 = 32_767;

    /// Sets Maximum Iterations, kept within 1 to [`CalcSettings::MAX_ITERATIONS`].
    pub fn set_max_iterations(&mut self, n: u32) {
        self.max_iterations = n.clamp(1, Self::MAX_ITERATIONS);
    }

    /// Sets Maximum Change (its magnitude); a value that isn't finite is ignored.
    pub fn set_max_change(&mut self, d: f64) {
        if d.is_finite() {
            self.max_change = d.abs();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefinedName {
    pub name: String,
    /// `None` = workbook scope; otherwise the sheet index.
    pub scope: Option<usize>,
    /// Formula text without `=` (`Sheet1!$A$1:$A$10`, `0.07`, `LAMBDA(x,x*2)`).
    pub formula: String,
    pub comment: String,
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocProps {
    pub title: String,
    pub subject: String,
    pub author: String,
    pub company: String,
    pub keywords: String,
    pub description: String,
}

/// One phonetic (furigana) run of a string item (`<rPh>`): the reading `text` and the range of the
/// base text it annotates.
///
/// `start` and `end` are zero-based offsets into the base text (`sb` and `eb` in the file) with
/// `start < end`; reading keeps them within the text's length in UTF-16 code units. Behaviour
/// reference: ISO/IEC 29500-1 §18.4.6 `rPh` and the Microsoft Learn `PhoneticRun` class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneticRun {
    pub start: u32,
    pub end: u32,
    pub text: String,
}

/// The `<phoneticPr>` of a string item: how its phonetic runs are displayed. `kind` (`type`) and
/// `alignment` only ever hold values ISO/IEC 29500-1 §18.18.57 `ST_PhoneticType` / §18.18.56
/// `ST_PhoneticAlignment` define, so damaged files cannot be written back invalid.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneticProps {
    /// `fontId`: a required attribute, an index into the style sheet's fonts. A file without one
    /// gets 0 — the Normal style's font, which an out-of-range index falls back to anyway.
    #[serde(default)]
    pub font_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<String>,
}

/// The phonetic (furigana) data of one shared string: its `<rPh>` runs, in document order, and its
/// `<phoneticPr>` when the file had one.
///
/// `text` is the string's base text — the only handle a cell keeps on its shared string — and is
/// the key the writer uses to re-emit the runs next to the right `<si>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneticString {
    pub text: String,
    pub runs: Vec<PhoneticRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub props: Option<PhoneticProps>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workbook {
    pub sheets: Vec<Arc<Sheet>>,
    pub names: Vec<DefinedName>,
    pub styles: StyleTable,
    pub theme: Theme,
    pub date_system: DateSystem,
    pub calc: CalcSettings,
    #[serde(default, skip_serializing_if = "is_all")]
    pub show_objects: ShowObjects,
    /// Trust Center › Privacy Options › "Remove personal information from file properties on save"
    /// (`<workbookPr filterPrivacy="1"/>`): kept with the file, and while it is on the author and
    /// company are left out of the XLSX files GridCraft writes, as Excel leaves them empty. Other
    /// formats aren't filtered: the workbook's own serialization still holds them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_personal_info: bool,
    /// Read and set with [`Workbook::active_sheet`] and [`Workbook::set_active_sheet`].
    pub active_sheet: ActiveSheet,
    /// The workbook window's display settings (`<workbookView>`), kept across an open + save.
    #[serde(default, skip_serializing_if = "WorkbookView::is_default")]
    pub view: WorkbookView,
    pub props: DocProps,
    /// Workbook structure protection.
    pub protected_structure: bool,
    /// Password of the structure protection (XLSX `workbookProtection`), never the password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure_password: Option<crate::PasswordHash>,
    /// Custom fill lists (Options › Custom Lists) beyond the built-in ones.
    pub custom_lists: Vec<Vec<String>>,
    /// Named cell styles (Cell Styles gallery): name → style.
    pub cell_styles: Vec<(String, crate::style::Style)>,
    /// Phonetic (furigana) data of shared strings (`xl/sharedStrings.xml`), one entry per string
    /// text, so that an open + save keeps a Japanese workbook's readings. Written back next to the
    /// `<si>` holding the same text.
    ///
    /// Keyed by text because that is all a cell keeps of its shared string: the saved table holds
    /// each text once (the format's `uniqueCount`), so two `<si>` items that share a base text can
    /// only keep one reading — the reader keeps the first and warns. Empty for workbooks without
    /// phonetic data; not present in older JSON. Modelled data (unlike [`Workbook::preserved`],
    /// which holds unmodelled parts), so it is saved to JSON as well as to XLSX.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phonetics: Vec<PhoneticString>,
    /// VBA project (`xl/vbaProject.bin`) carried through unchanged. GridCraft does not run
    /// macros; the blob is preserved so that saving a macro-enabled workbook never destroys the
    /// macros it arrived with. base64 in JSON. `None` = the workbook has no macros. Shared, so a
    /// copy of the workbook (every edit's undo step) doesn't copy it.
    #[serde(default, with = "crate::b64::opt")]
    pub vba_project: Option<Arc<Vec<u8>>>,
    /// Field separator of the CSV this workbook was read from, so saving it as CSV writes the same
    /// separator back (a `;` file stays a `;` file). `None` = not read from CSV: CSV saves use `,`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csv_delimiter: Option<u8>,
    /// GridCraft macros (typed command lists, not VBA), in the order the macro list shows them.
    /// The JSON format keeps them as this field; xlsx keeps them in hidden defined names.
    #[serde(default, skip_serializing_if = "crate::macros::Macros::is_empty", skip_deserializing)]
    pub macros: crate::macros::Macros,
    /// The `macros` entries of a JSON file as read, before the engine checks each one on its own
    /// and moves it into `macros`, so one damaged or newer macro doesn't stop the file opening.
    /// Never written.
    #[serde(rename = "macros", default, skip_serializing, deserialize_with = "crate::macros::entries")]
    pub macros_read: Vec<serde_json::Value>,
    /// Macros in a newer format than this build reads, as the file held them. They are never read,
    /// listed or run; a save writes them back unchanged, so the newer build gets them back whole.
    /// Shared between snapshots, as nothing changes them.
    #[serde(skip)]
    pub macros_unread: Arc<Vec<serde_json::Value>>,
    /// Unmodelled file content kept for saving (package root and workbook level). Not in JSON.
    #[serde(skip)]
    pub preserved: Option<Arc<crate::preserved::Preserved>>,
    /// The folder the workbook was read from, which relative paths to linked workbooks in
    /// `preserved` are relative to. Saving into another folder rewrites them from there. Not in JSON.
    #[serde(skip)]
    pub source_dir: Option<std::path::PathBuf>,
    /// The region whose formats this workbook is typed and shown in (Options › Region). A runtime
    /// setting of the application, not of the file: it is not saved, and the engine session sets
    /// it on every document it holds. en-US until then.
    #[serde(skip, default = "gridcraft_core::region::default_region")]
    pub region: &'static gridcraft_core::Region,
    /// The file this workbook reports as its own, which is what `CELL("filename")` returns.
    /// `None` = no file yet, which Excel reports as an empty string until the workbook has been
    /// saved once. Deliberately separate from `source_dir`: that one names the folder relative
    /// links are relative to and must keep naming it after a Save As into another folder, since
    /// the links in memory are still written for the old one. Not in JSON.
    #[serde(skip)]
    pub source_path: Option<std::path::PathBuf>,
}

fn is_all(s: &ShowObjects) -> bool {
    *s == ShowObjects::All
}

impl Default for Workbook {
    fn default() -> Self {
        Workbook::new()
    }
}

/// The active sheet index of a [`Workbook`].
///
/// Every other part of a workbook value is immutable once shared (the engine keeps versions as
/// `Arc<Workbook>` and tells unsaved changes by pointer). The active sheet is the exception: it
/// is the view state of the window showing the workbook, and several windows on one workbook
/// swap their own sheet into it whenever one of them becomes the one commands act on. Keeping it
/// in an atomic lets that swap happen through the shared `Arc` without cloning the workbook.
/// Copies (undo versions, a cloned workbook) take the value at the time of the copy.
#[derive(Default, Serialize, Deserialize)]
#[serde(from = "usize", into = "usize")]
pub struct ActiveSheet(AtomicUsize);

impl ActiveSheet {
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
    pub fn set(&self, i: usize) {
        self.0.store(i, Ordering::Relaxed);
    }
}

impl From<usize> for ActiveSheet {
    fn from(i: usize) -> ActiveSheet {
        ActiveSheet(AtomicUsize::new(i))
    }
}

impl From<ActiveSheet> for usize {
    fn from(a: ActiveSheet) -> usize {
        a.get()
    }
}

impl Clone for ActiveSheet {
    fn clone(&self) -> ActiveSheet {
        ActiveSheet::from(self.get())
    }
}

impl PartialEq for ActiveSheet {
    fn eq(&self, other: &ActiveSheet) -> bool {
        self.get() == other.get()
    }
}

impl std::fmt::Debug for ActiveSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

/// Where a reference is held, for [`Workbook::rewrite_references`].
#[derive(Clone, Copy, Debug)]
pub struct RefHost<'a> {
    /// The sheet holding it, or a defined name's scope (`None`: the workbook).
    pub sheet: Option<usize>,
    /// The name unqualified references in it are on: its sheet's (a sheet-level defined name's
    /// scope sheet, where they are read), or [`NAME_HOST`] for a workbook-level defined name
    /// (read on whichever sheet uses it).
    pub name: &'a str,
}

/// [`RefHost::name`] for a workbook-level defined name: `[` and `]` can't be in a sheet's name, so
/// no sheet's references are taken for a name's.
pub const NAME_HOST: &str = "[]";

/// Formula text with `f` applied to its references; text that isn't a formula, or that `f`
/// doesn't change, stays as it is (a leading `=` is kept). Whether it changed.
fn rewrite_with<E>(t: &mut String, f: &dyn Fn(Expr) -> Expr, parse: fn(&str) -> Result<Expr, E>) -> bool {
    let body = t.trim_start_matches('=');
    let Ok(e) = parse(body) else { return false };
    let before = gridcraft_formula::print(&e);
    let after = gridcraft_formula::print(&f(e));
    if after == before {
        return false;
    }
    *t = if t.starts_with('=') { format!("={after}") } else { after };
    true
}

fn rewrite_text(t: &mut String, f: &dyn Fn(Expr) -> Expr) -> bool {
    rewrite_with(t, f, gridcraft_formula::parse)
}

impl Workbook {
    /// Rewrites every reference the workbook holds with `f`, given where each is held, in one
    /// place: cell formulas (keeping their array and spill kind), defined names, conditional
    /// formats and data validation (written for their range's top-left cell, so they move as
    /// a cell formula there does), chart series (a chart's source block
    /// the edit breaks is dropped; one already broken is kept), sparklines, PivotTable sources
    /// (not one that is a table's name alone, which reads the table first) and form controls'
    /// linked cells. The engine's reference-moving edits (renaming sheets and names, deleting sheets,
    /// renaming tables, inserting and deleting lines and cells, cut-moves) go through it. Not
    /// visited: table calculated-column and totals formulas, a PivotChart's link to its
    /// PivotTable, a slicer's table, and Python cells' `xl()` references (the engine rewrites
    /// the last itself, and a PivotChart's link on a sheet rename). A legacy array's own range is the
    /// caller's to move. Internal
    /// hyperlinks keep the place they name as typed, as Excel's do. A sheet nothing changes on
    /// stays shared.
    pub fn rewrite_references(&mut self, f: &dyn Fn(RefHost<'_>, Expr) -> Expr) {
        let sheet_names: Vec<String> = self.sheets.iter().map(|s| s.name.clone()).collect();
        for n in &mut self.names {
            let name = n.scope.and_then(|i| sheet_names.get(i)).map_or(NAME_HOST, String::as_str);
            let host = RefHost { sheet: n.scope, name };
            rewrite_with(&mut n.formula, &|e| f(host, e), gridcraft_formula::parse_name);
        }
        // A series name is read by the chart as a reference when it starts with `=`, names a
        // sheet (`!`) or is a defined name its sheet sees; anything else (a word, a table's
        // name, `Q1`) is a label and is left as written.
        let names: Vec<(String, Option<usize>)> = self.names.iter().map(|n| (n.name.to_lowercase(), n.scope)).collect();
        // A PivotTable source that is a table's name alone reads the table before any defined
        // name of that name (`pivot::resolve_source`), so a name's edit doesn't reach it.
        let tables: Vec<String> = self.sheets.iter().flat_map(|s| s.tables.iter().map(|t| t.name.to_lowercase())).collect();
        let table_source = |t: &str| tables.contains(&t.trim().trim_start_matches('=').trim().to_lowercase());
        let label = |t: &str, si: usize| {
            let t = t.trim();
            let defined = || names.iter().any(|(n, scope)| scope.is_none_or(|s| s == si) && *n == t.to_lowercase());
            !t.starts_with('=') && !t.contains('!') && !defined()
        };
        for (si, shared) in self.sheets.iter_mut().enumerate() {
            // Worked out on the shared sheet; a list is copied only when something in it
            // changes, and a sheet nothing changes on stays shared with undo snapshots.
            let sh: &Sheet = shared;
            let host = RefHost { sheet: Some(si), name: &sh.name };
            let f = |e: Expr| f(host, e);
            let changes = |t: &str| rewrite_with(&mut t.to_string(), &f, gridcraft_formula::parse);
            let cells: Vec<(gridcraft_core::CellRef, Formula)> = sh
                .cells
                .iter()
                .filter_map(|(c, cell)| {
                    // Trees are compared, not printed text: a formula the edit leaves alone keeps
                    // its stored text, even text that isn't canonical, and isn't printed.
                    let old = cell.formula.as_ref()?;
                    let new = old.with_expr(|e| Some(f(e.clone())).filter(|n| n != e))??;
                    Some((c, Formula { array: old.array, dynamic: old.dynamic, ..Formula::from_expr(new) }))
                })
                .collect();
            let cond_formats = sh.cond_formats.iter().any(|cf| cf.rule.formulas().into_iter().any(|t| changes(t))).then(|| {
                let mut v = sh.cond_formats.clone();
                v.iter_mut().flat_map(|cf| cf.rule.formulas_mut()).for_each(|t| {
                    rewrite_text(t, &f);
                });
                v
            });
            let validations = sh.validations.iter().any(|dv| dv.formulas().into_iter().any(|t| changes(t))).then(|| {
                let mut v = sh.validations.clone();
                v.iter_mut().flat_map(|dv| dv.formulas_mut()).for_each(|t| {
                    rewrite_text(t, &f);
                });
                v
            });
            // A chart's source block that no longer exists is dropped.
            // (Read from the tree: a sheet may be named `#REF!`.)
            let dead = |src: Option<&str>| {
                src.and_then(|s| gridcraft_formula::parse(s.trim_start_matches('=')).ok()).is_some_and(|e| {
                    let mut hit = false;
                    e.walk(&mut |x| hit |= matches!(x, Expr::Error(gridcraft_core::CellError::Ref)));
                    hit
                })
            };
            let charts = sh
                .charts
                .iter()
                .any(|ch| {
                    ch.source.as_deref().is_some_and(changes)
                        || ch.series.iter().any(|se| {
                            changes(&se.values)
                                || [&se.categories, &se.bubble_sizes].into_iter().flatten().any(|t| changes(t))
                                || se.name.as_deref().is_some_and(|t| !label(t, si) && changes(t))
                        })
                })
                .then(|| {
                    let mut v = sh.charts.clone();
                    for ch in &mut v {
                        for se in &mut ch.series {
                            rewrite_text(&mut se.values, &f);
                            for t in [&mut se.categories, &mut se.bubble_sizes].into_iter().flatten() {
                                rewrite_text(t, &f);
                            }
                            if let Some(t) = se.name.as_mut().filter(|t| !label(t, si)) {
                                rewrite_text(t, &f);
                            }
                        }
                        // A source this edit breaks is dropped; one already broken before it
                        // (from a file, or an earlier edit) is kept as it was.
                        let was_dead = dead(ch.source.as_deref());
                        if let Some(src) = &mut ch.source {
                            rewrite_text(src, &f);
                        }
                        if !was_dead && dead(ch.source.as_deref()) {
                            ch.source = None;
                        }
                    }
                    v
                });
            let sparklines = sh.sparklines.iter().any(|sp| changes(&sp.source)).then(|| {
                let mut v = sh.sparklines.clone();
                v.iter_mut().for_each(|sp| {
                    rewrite_text(&mut sp.source, &f);
                });
                v
            });
            let pivots = sh.pivots.iter().any(|pt| !table_source(&pt.source) && changes(&pt.source)).then(|| {
                let mut v = sh.pivots.clone();
                v.iter_mut().filter(|pt| !table_source(&pt.source)).for_each(|pt| {
                    rewrite_text(&mut pt.source, &f);
                });
                v
            });
            // A form control's linked cell.
            let controls = sh.controls.iter().any(|c| c.link.as_deref().is_some_and(changes)).then(|| {
                let mut v = sh.controls.clone();
                v.iter_mut().flat_map(|c| c.link.as_mut()).for_each(|t| {
                    rewrite_text(t, &f);
                });
                v
            });
            if cells.is_empty()
                && cond_formats.is_none()
                && validations.is_none()
                && charts.is_none()
                && sparklines.is_none()
                && pivots.is_none()
                && controls.is_none()
            {
                continue;
            }
            let sh = Arc::make_mut(shared);
            for (c, formula) in cells {
                if let Some(cell) = sh.cells.get_mut(c) {
                    cell.formula = Some(Arc::new(formula));
                }
            }
            if let Some(v) = cond_formats {
                sh.cond_formats = v;
            }
            if let Some(v) = validations {
                sh.validations = v;
            }
            if let Some(v) = charts {
                sh.charts = v;
            }
            if let Some(v) = sparklines {
                sh.sparklines = v;
            }
            if let Some(v) = pivots {
                sh.pivots = v;
            }
            if let Some(v) = controls {
                sh.controls = v;
            }
        }
    }

    /// A new workbook with one sheet, "Sheet1".
    pub fn new() -> Workbook {
        Workbook {
            sheets: vec![Arc::new(Sheet::new("Sheet1"))],
            names: vec![],
            styles: StyleTable::default(),
            theme: Theme::default(),
            date_system: DateSystem::D1900,
            calc: CalcSettings::default(),
            show_objects: ShowObjects::All,
            remove_personal_info: false,
            active_sheet: ActiveSheet::default(),
            view: WorkbookView::default(),
            props: DocProps::default(),
            protected_structure: false,
            structure_password: None,
            custom_lists: vec![],
            cell_styles: vec![],
            phonetics: vec![],
            vba_project: None,
            csv_delimiter: None,
            macros: crate::macros::Macros::default(),
            macros_read: vec![],
            macros_unread: Arc::default(),
            preserved: None,
            source_dir: None,
            region: gridcraft_core::region::default_region(),
            source_path: None,
        }
    }
    pub fn sheet(&self, i: usize) -> Option<&Sheet> {
        self.sheets.get(i).map(|s| s.as_ref())
    }
    pub fn sheet_mut(&mut self, i: usize) -> Option<&mut Sheet> {
        self.sheets.get_mut(i).map(Arc::make_mut)
    }
    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|s| s.name.eq_ignore_ascii_case(name))
    }
    /// The index of the sheet the workbook's window shows (the file's active tab).
    pub fn active_sheet(&self) -> usize {
        self.active_sheet.get()
    }
    /// Changes the active sheet in place, also through a shared `Arc<Workbook>`: it is view state,
    /// so switching sheets or windows neither copies the workbook nor makes it a new version.
    pub fn set_active_sheet(&self, i: usize) {
        self.active_sheet.set(i);
    }
    pub fn active(&self) -> Option<&Sheet> {
        self.sheet(self.active_sheet())
    }
    /// A fresh sheet name `SheetN` not in use.
    pub fn next_sheet_name(&self) -> String {
        (1..).map(|n| format!("Sheet{n}")).find(|n| self.sheet_index(n).is_none()).unwrap_or_else(|| "Sheet".into())
    }
    /// Validates a sheet name like Excel: 1–31 chars, none of `: \ / ? * [ ]`, not starting or
    /// ending with `'`, unique (case-insensitive) unless it is `except`'s own name.
    pub fn check_sheet_name(&self, name: &str, except: Option<usize>) -> Result<(), String> {
        let n = name.chars().count();
        if n == 0 {
            return Err("A sheet name can't be blank.".into());
        }
        if n > 31 {
            return Err("A sheet name can't be longer than 31 characters.".into());
        }
        if name.chars().any(|c| matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']')) {
            return Err("A sheet name can't contain any of these characters: : \\ / ? * [ ]".into());
        }
        if name.starts_with('\'') || name.ends_with('\'') {
            return Err("A sheet name can't begin or end with an apostrophe.".into());
        }
        if name.eq_ignore_ascii_case("History") {
            return Err("\"History\" is a reserved name.".into());
        }
        if let Some(i) = self.sheet_index(name)
            && Some(i) != except
        {
            return Err("That name is already taken. Try a different one.".into());
        }
        Ok(())
    }
    /// Finds a defined name visible from `sheet`: sheet scope first, then workbook scope.
    pub fn name(&self, name: &str, sheet: usize) -> Option<&DefinedName> {
        self.names
            .iter()
            .find(|n| n.scope == Some(sheet) && n.name.eq_ignore_ascii_case(name))
            .or_else(|| self.names.iter().find(|n| n.scope.is_none() && n.name.eq_ignore_ascii_case(name)))
    }
    /// Spells calls of LAMBDA names the way the names are defined (`Double(4)`, where the
    /// parser gives `DOUBLE(4)`: it upper-cases function names, as built-in ones are shown).
    pub fn name_call_case(&self, e: Expr) -> Expr {
        spell_calls(e, &self.lambda_names())
    }
    /// [`Workbook::name_call_case`] for every cell formula and defined name (after reading a
    /// file).
    pub fn apply_name_call_case(&mut self) {
        let lambdas = self.lambda_names();
        if lambdas.is_empty() {
            return;
        }
        for sheet in self.sheets.iter_mut() {
            let fixed: Vec<_> = sheet
                .cells
                .iter()
                .filter_map(|(c, cell)| {
                    let f = cell.formula.as_ref()?;
                    let e = f.expr()?;
                    let ne = spell_calls(e.clone(), &lambdas);
                    (ne != e).then(|| (c, Formula { array: f.array, dynamic: f.dynamic, ..Formula::from_expr(ne) }))
                })
                .collect();
            if fixed.is_empty() {
                continue;
            }
            let sheet = Arc::make_mut(sheet);
            for (c, f) in fixed {
                if let Some(cell) = sheet.cells.get_mut(c) {
                    cell.formula = Some(Arc::new(f));
                }
            }
        }
        for n in self.names.iter_mut() {
            if let Ok(e) = gridcraft_formula::parse_name(&n.formula) {
                let ne = spell_calls(e.clone(), &lambdas);
                if ne != e {
                    n.formula = gridcraft_formula::print(&ne);
                }
            }
        }
    }
    /// Names of the defined names holding a LAMBDA (callable like functions).
    fn lambda_names(&self) -> Vec<String> {
        let is_lambda = |f: &str| f.trim_start_matches('=').trim_start().get(..7).is_some_and(|p| p.eq_ignore_ascii_case("LAMBDA("));
        self.names.iter().filter(|n| is_lambda(&n.formula)).map(|n| n.name.clone()).collect()
    }
    /// Table by name across sheets: (sheet index, table index).
    pub fn table(&self, name: &str) -> Option<(usize, usize)> {
        for (si, s) in self.sheets.iter().enumerate() {
            if let Some(ti) = s.tables.iter().position(|t| t.name.eq_ignore_ascii_case(name)) {
                return Some((si, ti));
            }
        }
        None
    }
    /// Rebuilds the cached layout of every equation whose source changed (or was never laid
    /// out, as after loading a file). One call lays out at most [`MAX_EQUATION_LAYOUT_BYTES`] of
    /// source: one equation can take a tenth of a second to lay out, so a file with thousands of
    /// them would otherwise hold the app for minutes. Equations past the budget get an error
    /// instead (they show their source; editing one lays it out).
    pub fn refresh_equations(&mut self) {
        let stale = |s: &crate::Shape| s.kind == crate::ShapeKind::Equation && s.math.get(&s.text).is_none();
        let mut budget = MAX_EQUATION_LAYOUT_BYTES;
        // Only sheets with a stale equation are copied (sheets are shared with undo snapshots).
        for sh in self.sheets.iter_mut().filter(|sh| sh.shapes.iter().any(stale)) {
            for s in Arc::make_mut(sh).shapes.iter_mut().filter(|s| stale(s)) {
                // A source over the length cap never gets here: the cache answers for it without
                // a lookup. An empty source still costs a call.
                let cost = s.text.len().max(1);
                if cost > budget {
                    s.math.refuse(&s.text, EQUATION_BUDGET_MESSAGE);
                    continue;
                }
                budget -= cost;
                s.math.refresh(&s.text);
            }
        }
    }

    /// A free id for a new chart, picture, shape, table, slicer or form control: one more than the largest in
    /// use, or, when that is `u32::MAX` (a file can carry any id), the lowest free one.
    pub fn next_object_id(&self) -> u32 {
        self.free_object_ids(1).first().copied().unwrap_or(u32::MAX)
    }

    /// `count` distinct ids no object in the workbook uses, sorted: the ones above the largest id
    /// in use while they last, then the lowest free ones. Fewer come back only when the id space
    /// is exhausted.
    pub fn free_object_ids(&self, count: usize) -> Vec<u32> {
        let mut used: Vec<u32> = self
            .sheets
            .iter()
            .flat_map(|s| {
                s.charts
                    .iter()
                    .map(|o| o.id)
                    .chain(s.images.iter().map(|o| o.id))
                    .chain(s.shapes.iter().map(|o| o.id))
                    .chain(s.tables.iter().map(|o| o.id))
                    .chain(s.slicers.iter().map(|o| o.id))
                    .chain(s.controls.iter().map(|o| o.id))
            })
            .collect();
        used.sort_unstable();
        used.dedup();
        let max = used.last().copied().unwrap_or(0);
        let mut out = Vec::with_capacity(count.min(1 << 16));
        let mut next = max.checked_add(1);
        while out.len() < count
            && let Some(id) = next
        {
            out.push(id);
            next = id.checked_add(1);
        }
        // Past the top: the gaps below the largest id, from 1 (0 is never an object's id).
        let mut taken = used.iter().copied().peekable();
        let mut id = 1u32;
        while out.len() < count && id < max {
            while taken.next_if(|&t| t < id).is_some() {}
            if taken.peek() == Some(&id) {
                taken.next();
            } else {
                out.push(id);
            }
            id += 1;
        }
        out.sort_unstable();
        out
    }
}

/// Calls of the names in `lambdas` spelled as the names are.
fn spell_calls(e: Expr, lambdas: &[String]) -> Expr {
    if lambdas.is_empty() {
        return e;
    }
    e.map(&mut |x| match x {
        Expr::Call(n, args) => match lambdas.iter().find(|l| l.eq_ignore_ascii_case(&n)) {
            Some(l) => Expr::Call(l.clone(), args),
            None => Expr::Call(n, args),
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phonetics_are_additive_in_json() -> Result<(), Box<dyn std::error::Error>> {
        let mut wb = Workbook::new();
        assert!(wb.phonetics.is_empty(), "new workbooks have no phonetic data");
        wb.phonetics.push(PhoneticString {
            text: "東".into(),
            runs: vec![PhoneticRun { start: 0, end: 1, text: "ヒガシ".into() }],
            props: Some(PhoneticProps { font_id: 3, kind: Some("Hiragana".into()), alignment: None }),
        });
        let json = serde_json::to_string(&wb)?;
        assert_eq!(serde_json::from_str::<Workbook>(&json)?.phonetics, wb.phonetics);
        // JSON written before the field existed still loads.
        let mut value: serde_json::Value = serde_json::from_str(&json)?;
        value.as_object_mut().ok_or("none")?.remove("phonetics");
        assert!(serde_json::from_value::<Workbook>(value)?.phonetics.is_empty());
        Ok(())
    }

    /// A rewrite copies only the sheets it changes: the others stay shared (with undo snapshots).
    #[test]
    fn rewrite_references_leaves_untouched_sheets_shared() -> Result<(), Box<dyn std::error::Error>> {
        let mut wb = Workbook::new();
        let mut other = Sheet::new("Other");
        other.set_cell(gridcraft_core::CellRef::new(0, 0), crate::Cell::formula(Formula::new("Sheet1!A1+1")));
        wb.sheets.push(Arc::new(other));
        let mut third = Sheet::new("Third");
        third.set_cell(gridcraft_core::CellRef::new(0, 0), crate::Cell::formula(Formula::new("A1+1")));
        // A rule that doesn't name the sheet leaves its sheet shared; one that does is rewritten.
        third.validations.push(crate::Validation { f1: "=A1>0".into(), ..Default::default() });
        wb.sheets.push(Arc::new(third));
        let mut fourth = Sheet::new("Fourth");
        fourth.validations.push(crate::Validation { f1: "=Sheet1!A1>0".into(), ..Default::default() });
        wb.sheets.push(Arc::new(fourth));
        let snapshot = wb.clone();
        wb.rewrite_references(&|_, e| gridcraft_formula::adjust::rename_sheet(e, "Sheet1", "Data"));
        let shared = |i: usize| -> Result<bool, Box<dyn std::error::Error>> {
            Ok(Arc::ptr_eq(wb.sheets.get(i).ok_or("sheet")?, snapshot.sheets.get(i).ok_or("sheet")?))
        };
        assert!(shared(0)?, "nothing on Sheet1 names it");
        assert!(!shared(1)?, "Other's formula names Sheet1");
        assert!(shared(2)?, "Third's formula and rule are unqualified");
        assert!(!shared(3)?, "Fourth's rule names Sheet1");
        assert_eq!(wb.sheets.get(3).and_then(|s| s.validations.first()).map(|v| v.f1.as_str()), Some("=Data!A1>0"));
        let f = wb.sheets.get(1).and_then(|s| s.cell(gridcraft_core::CellRef::new(0, 0))).and_then(|c| c.formula.clone()).ok_or("formula")?;
        assert_eq!(f.text, "Data!A1+1");
        Ok(())
    }

    #[test]
    fn sheet_names() {
        let wb = Workbook::new();
        assert_eq!(wb.next_sheet_name(), "Sheet2");
        assert!(wb.check_sheet_name("sheet1", None).is_err());
        assert!(wb.check_sheet_name("sheet1", Some(0)).is_ok());
        assert!(wb.check_sheet_name("a/b", None).is_err());
        assert!(wb.check_sheet_name(&"x".repeat(32), None).is_err());
        assert!(wb.check_sheet_name("Budget 2026", None).is_ok());
    }

    #[test]
    fn object_ids_never_overflow_or_repeat() {
        let mut wb = Workbook::new();
        let image = |id| crate::Image { id, anchor: Default::default(), data: Default::default(), mime: String::new(), alt: String::new() };
        assert_eq!(wb.next_object_id(), 1);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(7));
        }
        assert_eq!(wb.next_object_id(), 8);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.controls.push(crate::FormControl { id: 12, ..Default::default() });
        }
        assert_eq!(wb.next_object_id(), 13, "form controls take ids from the same sequence");
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(u32::MAX));
        }
        // An id read from a file at the maximum: the lowest free id, not the maximum again.
        assert_eq!(wb.next_object_id(), 1);
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(1));
            sh.images.push(image(3));
        }
        assert_eq!(wb.next_object_id(), 2);
        assert_eq!(wb.free_object_ids(4), vec![2, 4, 5, 6], "the gaps, skipping 1, 3 and 7");
        let mut wb = Workbook::new();
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(u32::MAX - 1));
        }
        assert_eq!(wb.free_object_ids(3), vec![1, 2, u32::MAX], "the top first, then from 1");
    }

    /// A form control's id is in use like any object's: the free ids skip it, also when it is the
    /// largest id a file could carry.
    #[test]
    fn free_object_ids_skip_a_form_controls_id() {
        let mut wb = Workbook::new();
        if let Some(sh) = wb.sheet_mut(0) {
            sh.controls.push(crate::FormControl { id: 3, ..Default::default() });
            sh.controls.push(crate::FormControl { id: u32::MAX, ..Default::default() });
        }
        assert_eq!(wb.free_object_ids(4), vec![1, 2, 4, 5], "3 and the largest id are taken");
        assert_eq!(wb.next_object_id(), 1);
    }

    /// A file can use the largest id and the one below it: nothing is free above them, so the free
    /// ids are the lowest unused ones from 1 (0 is never an object's id), and a large request is
    /// served in time linear in the count and the ids in use.
    #[test]
    fn free_object_ids_come_from_below_when_nothing_is_free_above() {
        let mut wb = Workbook::new();
        let image = |id| crate::Image { id, anchor: Default::default(), data: Default::default(), mime: String::new(), alt: String::new() };
        if let Some(sh) = wb.sheet_mut(0) {
            sh.images.push(image(u32::MAX));
            sh.images.push(image(u32::MAX - 1));
            sh.images.push(image(0));
        }
        assert_eq!(wb.free_object_ids(3), vec![1, 2, 3]);
        assert_eq!(wb.next_object_id(), 1);
        // The 100,000 lowest free ids: distinct, ascending, none of them in use.
        let many = wb.free_object_ids(100_000);
        assert_eq!(many.len(), 100_000);
        assert!(many.windows(2).all(|w| w[0] < w[1]), "ascending without repeats");
        assert!(!many.contains(&0) && !many.contains(&(u32::MAX - 1)) && !many.contains(&u32::MAX));
        assert_eq!(wb.free_object_ids(0), Vec::<u32>::new());
    }
}
