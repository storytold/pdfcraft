//! The command registry. Ids follow Excel's ribbon (`home.bold`, `insert.chart`,
//! `data.sortAscending`) plus primitive editing commands (`cell.set`, `selection.set`).

pub mod analysis;
pub mod controls;
pub mod data;
pub mod draw;
pub mod edit;
pub mod equation;
pub mod extra;
pub mod file;
pub mod forecast;
pub mod format;
pub mod formulas;
pub mod insert;
pub mod inspect;
pub mod macros;
pub mod pivot;
pub mod print;
pub mod python;
pub mod review;
pub mod sheet;
pub mod slicer;
pub mod solver;
pub mod spelling;
pub mod stash;
pub mod view;
pub mod whatif;
pub mod wordart;

use gridcraft_calc::Key;
use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::Workbook;
use serde::Serialize;
use serde_json::Value as Json;

use crate::{DocState, EngineError, Result, Selection, Session};

pub type Run = fn(&mut Session, &Json) -> Result<Json>;
pub type Enabled = fn(&Session) -> std::result::Result<(), String>;

pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Ribbon/menu placement, e.g. `["Home", "Font"]`. Empty = not in menus.
    pub menu: &'static [&'static str],
    /// Portable notation: `Cmd` is ⌘ on macOS and Ctrl elsewhere.
    pub shortcut: Option<&'static str>,
    /// macOS override when its key differs. Equal to `shortcut` marks a Mac-only shortcut
    /// (Align Left/Center): other platforms show none. See [`CommandInfo::shortcut_for_platform`].
    pub shortcut_mac: Option<&'static str>,
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    pub journal: bool,
    pub undoable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: Vec<&'static str>,
    pub shortcut: Option<&'static str>,
    pub shortcut_mac: Option<&'static str>,
    pub params: &'static str,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

impl CommandInfo {
    /// Format the portable shortcut notation for the runtime operating system.
    pub fn shortcut_for_platform(&self, mac: bool) -> Option<String> {
        let keys = if mac {
            self.shortcut_mac.or(self.shortcut)
        } else if self.shortcut_mac.is_some() && self.shortcut_mac == self.shortcut {
            // An identical Mac override marks a Mac-only display shortcut.
            // The portable field remains unchanged for legacy CLI consumers.
            None
        } else {
            self.shortcut
        }?;
        if mac {
            return Some(keys.to_owned());
        }
        Some(keys.split('+').map(|part| if part == "Cmd" { "Ctrl" } else { part }).collect::<Vec<_>>().join("+"))
    }
}

impl CommandSpec {
    pub fn info(&self, s: &Session) -> CommandInfo {
        let e = (self.enabled)(s);
        CommandInfo {
            id: self.id,
            label: self.label,
            menu: self.menu.to_vec(),
            shortcut: self.shortcut,
            shortcut_mac: self.shortcut_mac,
            params: self.params,
            enabled: e.is_ok(),
            disabled_reason: e.err(),
        }
    }
}

/// Params that hold a secret: `password`, or a name ending in `Password`.
pub fn is_secret_param(key: &str) -> bool {
    key == "password" || key.ends_with("Password")
}

/// The params as the journal records them: secret params are left out, so a password never
/// reaches the journal dialog, `engine.journal` or a recorded script (saved in the workbook).
/// Replaying such an entry runs the command without that param.
pub fn journal_params(params: &Json) -> Json {
    match params {
        Json::Object(o) => Json::Object(o.iter().filter(|(k, _)| !is_secret_param(k)).map(|(k, v)| (k.clone(), v.clone())).collect()),
        other => other.clone(),
    }
}

pub fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}
pub fn has_doc(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no workbook open".into())
}
pub fn can_undo(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    not_grouping(s)?;
    if s.active().is_some_and(|d| !d.undo.is_empty()) { Ok(()) } else { Err("nothing to undo".into()) }
}
pub fn can_redo(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    not_grouping(s)?;
    if s.active().is_some_and(|d| !d.redo.is_empty()) { Ok(()) } else { Err("nothing to redo".into()) }
}
/// Undo and Redo wait while an undo group is open on the active document (an action that spans
/// several calls is still running).
fn not_grouping(s: &Session) -> std::result::Result<(), String> {
    if s.active().is_some_and(|d| s.in_group(d.uid)) { Err("finish or stop the running action first (automate.stopAction)".into()) } else { Ok(()) }
}
pub fn has_clipboard(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    if s.clipboard.is_some() { Ok(()) } else { Err("the clipboard is empty".into()) }
}

macro_rules! cmd {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::cmd::CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, shortcut_mac: None, params: $params, enabled: $en, run: $run, journal: true, undoable: true }
    };
    (query $id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::cmd::CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, shortcut_mac: None, params: $params, enabled: $en, run: $run, journal: false, undoable: false }
    };
    (noundo $id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::cmd::CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, shortcut_mac: None, params: $params, enabled: $en, run: $run, journal: true, undoable: false }
    };
}
pub(crate) use cmd;

pub fn command_specs() -> &'static [CommandSpec] {
    static SPECS: std::sync::OnceLock<Vec<CommandSpec>> = std::sync::OnceLock::new();
    SPECS.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(file::specs());
        v.extend(edit::specs());
        v.extend(format::specs());
        v.extend(sheet::specs());
        v.extend(insert::specs());
        v.extend(controls::specs());
        v.extend(equation::specs());
        v.extend(data::specs());
        v.extend(formulas::specs());
        v.extend(review::specs());
        v.extend(view::specs());
        v.extend(inspect::specs());
        v.extend(extra::specs());
        v.extend(macros::specs());
        v.extend(draw::specs());
        v.extend(wordart::specs());
        v.extend(spelling::specs());
        v.extend(solver::specs());
        v.extend(pivot::specs());
        v.extend(python::specs());
        v.extend(slicer::specs());
        v.extend(whatif::specs());
        v.extend(forecast::specs());
        v.extend(analysis::specs());
        v.extend(print::specs());
        v
    })
}

pub fn find_command(id: &str) -> Option<&'static CommandSpec> {
    if let Some(spec) = command_specs().iter().find(|c| c.id == id) {
        return Some(spec);
    }
    // Commands registered by UI integration tests (pending keytips go live
    // the moment their command lands, with no table edit).
    test_commands().iter().find(|c| c.id == id).copied()
}

static TEST_COMMANDS: std::sync::Mutex<Vec<&CommandSpec>> = std::sync::Mutex::new(Vec::new());

/// Test-only: register an extra command so UI integration tests can prove a pending
/// keytip becomes live the moment its command lands (see crates/ui-egui/tests/keytips.rs).
/// Not part of the public API; harmless no-op for production callers who never call it.
#[doc(hidden)]
pub fn register_test_command(id: &'static str, run: Run) {
    // Leaked intentionally: test commands live for the whole process.
    let spec: &'static CommandSpec = Box::leak(Box::new(CommandSpec {
        id,
        label: id,
        menu: &[],
        shortcut: None,
        shortcut_mac: None,
        params: "",
        enabled: |_| Ok(()),
        run,
        journal: true,
        undoable: false,
    }));
    test_commands().push(spec);
}

/// Test-only: an undoable command, to exercise how `execute` records its step.
#[cfg(test)]
pub(crate) fn register_undoable_test_command(id: &'static str, run: Run) {
    let spec: &'static CommandSpec = Box::leak(Box::new(CommandSpec {
        id,
        label: id,
        menu: &[],
        shortcut: None,
        shortcut_mac: None,
        params: "",
        enabled: |_| Ok(()),
        run,
        journal: false,
        undoable: true,
    }));
    test_commands().push(spec);
}

fn test_commands() -> std::sync::MutexGuard<'static, Vec<&'static CommandSpec>> {
    match TEST_COMMANDS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// ---------------------------------------------------------------- param helpers

pub(crate) fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
pub(crate) fn str_param<'a>(p: &'a Json, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Json::as_str)
}
pub(crate) fn f64_param(p: &Json, key: &str) -> Option<f64> {
    p.get(key).and_then(Json::as_f64)
}
pub(crate) fn bool_param(p: &Json, key: &str) -> Option<bool> {
    p.get(key).and_then(Json::as_bool)
}
pub(crate) fn u32_param(p: &Json, key: &str) -> Option<u32> {
    p.get(key).and_then(Json::as_u64).map(|v| v.min(u32::MAX as u64) as u32)
}

/// An object anchor mode from Excel's wording or the model's: `"moveAndSize"`/`"twoCell"`,
/// `"moveOnly"`/`"oneCell"`, `"absolute"`/`"don't move or size"` (case-insensitive).
pub(crate) fn anchor_mode_param(p: &Json) -> Option<gridcraft_model::AnchorMode> {
    parse_anchor_mode(str_param(p, "mode").or_else(|| str_param(p, "anchorMode"))?)
}

pub(crate) fn parse_anchor_mode(s: &str) -> Option<gridcraft_model::AnchorMode> {
    match s.to_ascii_lowercase().replace([' ', '-', '_', '\''], "").as_str() {
        "moveandsize" | "moveandsizeandcells" | "twocell" => Some(gridcraft_model::AnchorMode::MoveAndSize),
        "moveonly" | "movebutdontsize" | "movebutdontsizewithcells" | "onecell" => Some(gridcraft_model::AnchorMode::MoveOnly),
        "absolute" | "dontmoveorsize" | "dontmoveorsizewithcells" | "none" => Some(gridcraft_model::AnchorMode::Absolute),
        _ => None,
    }
}

/// The wire name of an anchor mode (matches the `mode` param of [`anchor_mode_param`]).
pub(crate) fn anchor_mode_name(m: gridcraft_model::AnchorMode) -> &'static str {
    match m {
        gridcraft_model::AnchorMode::MoveAndSize => "moveAndSize",
        gridcraft_model::AnchorMode::MoveOnly => "moveOnly",
        gridcraft_model::AnchorMode::Absolute => "absolute",
    }
}
pub(crate) fn ok() -> Result<Json> {
    Ok(Json::Null)
}

/// A range parameter (`"A1:B2"`, `"Sheet2!A1"`, `"B:B"`), defaulting to the current selection's
/// areas.
pub(crate) fn target_ranges(s: &Session, p: &Json) -> Result<Vec<RangeRef>> {
    if let Some(r) = str_param(p, "range").or_else(|| str_param(p, "cell")) {
        let (_, body) = split_sheet(r);
        let mut out = Vec::new();
        for part in body.split(',') {
            out.push(RangeRef::parse(part).ok_or_else(|| bad("range", format!("not a range: `{part}`")))?);
        }
        return Ok(out);
    }
    Ok(s.doc()?.selection.ranges.clone())
}

/// A `rows`/`cols` parameter: `3:5` and `B:C`, or a single `3` or `B`.
pub(crate) fn parse_lines(t: &str) -> Option<RangeRef> {
    RangeRef::parse(t).or_else(|| if t.contains(':') { None } else { RangeRef::parse(&format!("{t}:{t}")) })
}

pub(crate) fn target_range(s: &Session, p: &Json) -> Result<RangeRef> {
    Ok(target_ranges(s, p)?.first().copied().unwrap_or_default())
}

/// The sheet a call names (`sheet` param or a `Sheet!` prefix on `range`), else the active one.
pub(crate) fn target_sheet(s: &Session, p: &Json) -> Result<usize> {
    let d = s.doc()?;
    if let Some(v) = p.get("sheet") {
        if let Some(i) = v.as_u64() {
            return if (i as usize) < d.wb.sheets.len() { Ok(i as usize) } else { Err(bad("sheet", "no such sheet")) };
        }
        if let Some(n) = v.as_str() {
            return d.wb.sheet_index(n).ok_or_else(|| bad("sheet", format!("no sheet named `{n}`")));
        }
    }
    if let Some(r) = str_param(p, "range").or_else(|| str_param(p, "cell"))
        && let (Some(sh), _) = split_sheet(r)
    {
        return d.wb.sheet_index(&sh).ok_or_else(|| bad("range", format!("no sheet named `{sh}`")));
    }
    Ok(d.wb.active_sheet())
}

pub(crate) use gridcraft_core::split_sheet;

/// A single-cell parameter. Absent or null is `Ok(None)` (callers default it); a value that is not
/// a cell reference, or that names a sheet other than the active one, is an error rather than a
/// silent fallback to some other cell.
pub(crate) fn cell_param(s: &Session, p: &Json, key: &str) -> Result<Option<CellRef>> {
    cell_param_on(s, p, key, s.doc()?.wb.active_sheet())
}

/// Like [`cell_param`], for a command that operates on `sheet` (a `Sheet!` prefix must name it).
pub(crate) fn cell_param_on(s: &Session, p: &Json, key: &str, sheet: usize) -> Result<Option<CellRef>> {
    let t = match p.get(key) {
        None | Some(Json::Null) => return Ok(None),
        Some(Json::String(t)) => t,
        Some(v) => return Err(bad("cell", format!("invalid cell reference `{v}` for `{key}`"))),
    };
    let (sh, body) = split_sheet(t);
    let c = CellRef::parse(body).ok_or_else(|| bad("cell", format!("invalid cell reference `{t}` for `{key}`")))?;
    if let Some(name) = sh
        && s.doc()?.wb.sheet_index(&name) != Some(sheet)
    {
        return Err(bad("cell", format!("`{t}` for `{key}` names a sheet other than the one this command works on")));
    }
    Ok(Some(c))
}

/// Applies an edit to a copy of the active workbook, then recalculates and commits it.
/// `changed` collects cells whose content changed; set `structural` for row/column/sheet edits
/// (full recalc).
pub struct Ctx<'a> {
    pub wb: Workbook,
    pub changed: Vec<Key>,
    pub structural: bool,
    /// Recalculate every formula without the other effects of `structural` (table headers are not
    /// resynchronized, array formulas are still checked): for edits that change only what a
    /// function reads, like sheet protection.
    pub recalc_all: bool,
    pub sel: &'a mut Selection,
    /// Rows whose automatic height should be recomputed (sheet, row).
    pub fit_rows: Vec<(usize, u32)>,
    /// The command applied sheet protection itself (row/column inserts and deletes the
    /// protection allows), so cells it shifts aren't refused as edits of locked cells.
    pub protection_checked: bool,
    /// The structural edits the command made, in order (the sheet's name, the edit), as
    /// references were rewritten for them.
    pub edits: Vec<(String, gridcraft_formula::adjust::Edit)>,
}

pub(crate) fn edit<R>(s: &mut Session, f: impl FnOnce(&mut Ctx) -> Result<R>) -> Result<R> {
    let d = s.doc_mut()?;
    commit(d, |cx| {
        let r = f(cx)?;
        crate::tables::sync_headers(cx);
        Ok(r)
    })
}

fn wall_clock() -> std::time::Duration {
    std::time::SystemTime::UNIX_EPOCH.elapsed().unwrap_or_default()
}

pub(crate) fn commit<R>(d: &mut DocState, f: impl FnOnce(&mut Ctx) -> Result<R>) -> Result<R> {
    // `GRIDCRAFT_PROFILE` timing, never on wasm where the clock traps. `elapsed` on the epoch
    // reads the wall clock without the (disallowed) `now`.
    let t0 = if cfg!(target_arch = "wasm32") { None } else { std::env::var_os("GRIDCRAFT_PROFILE").map(|_| wall_clock()) };
    let since = |t0: std::time::Duration| wall_clock().saturating_sub(t0);
    let mut sel = d.selection.clone();
    let mut ctx = Ctx {
        wb: (*d.wb).clone(),
        changed: Vec::new(),
        structural: false,
        recalc_all: false,
        sel: &mut sel,
        fit_rows: Vec::new(),
        protection_checked: false,
        edits: Vec::new(),
    };
    let r = f(&mut ctx)?;
    let Ctx { mut wb, mut changed, structural, recalc_all, mut fit_rows, protection_checked, edits, .. } = ctx;
    // Equations whose source changed (or that arrived without a layout, as from a paste) are
    // laid out here, so painting only ever reads the cache.
    wb.refresh_equations();
    if !protection_checked {
        check_protection(&d.wb, &wb, &d.unlocked_ranges)?;
    }
    if !structural {
        check_arrays(&d.wb, &wb)?;
    }
    if wb.calc.precision_as_displayed {
        round_edited_as_displayed(&d.wb, &mut wb, &mut changed, structural);
    }
    if let Some(log) = d.touched.as_mut() {
        for (si, c) in changed.iter().take(TOUCH_LOG_LIMIT.saturating_sub(log.len())) {
            if let Some(sh) = wb.sheet(*si) {
                log.push((sh.name.clone(), *c));
            }
        }
    }
    if let Some(log) = d.edits.as_mut() {
        log.extend(edits);
    }
    if changed.len() <= 200_000 {
        fit_rows.extend(changed.iter().map(|(s, c)| (*s, c.row)));
    }
    if let Some(t0) = t0 {
        eprintln!("commit: edit {:?}, {} changed", since(t0), changed.len());
    }
    if structural || recalc_all {
        d.calc.recalc_all(&mut wb);
    } else if !changed.is_empty() {
        d.calc.cells_changed(&mut wb, &changed);
    }
    controls::sync_linked_values(&mut wb);
    if let Some(t0) = t0 {
        eprintln!("commit: + recalc {:?}", since(t0));
    }
    fit_rows.sort_unstable();
    fit_rows.dedup();
    let mut sweep: Option<(usize, MergeSweep)> = None;
    let mut left = MERGE_FIT_WORK;
    for (si, row) in fit_rows {
        if sweep.as_ref().is_none_or(|(at, _)| *at != si) {
            if let Some((_, old)) = &sweep {
                left = old.left();
            }
            sweep = Some((si, MergeSweep::new(left)));
        }
        if let Some((_, sw)) = sweep.as_mut() {
            fit_row_height(&mut wb, si, row, sw);
        }
    }
    if let Some(t0) = t0 {
        eprintln!("commit: + rows {:?}", since(t0));
    }
    d.wb = std::sync::Arc::new(wb);
    d.selection = sel;
    Ok(r)
}

/// "Set precision as displayed": a number typed, pasted or reformatted is stored as its format
/// shows it (Excel rounds a constant when it is entered and when its cell's format changes,
/// observed through COM). Only cells the edit touched are visited, so this costs nothing for
/// the rest of the workbook; formula cells whose format changed are recalculated, and the
/// recalculation rounds them. A structural edit recalculates everything anyway, so it only
/// rounds its constants (a sheet the edit created, for one, holds written numbers).
fn round_edited_as_displayed(old: &Workbook, wb: &mut Workbook, changed: &mut Vec<gridcraft_calc::Key>, structural: bool) {
    let mut prec = gridcraft_calc::DisplayedPrecision::new();
    for si in 0..wb.sheets.len() {
        let Some(after) = wb.sheets.get(si) else { continue };
        // Sheets are matched by name. A sheet the edit created has no earlier version, so all of
        // its cells are new.
        let before = old.sheet_index(&after.name).and_then(|i| old.sheets.get(i));
        if before.is_some_and(|b| std::sync::Arc::ptr_eq(b, after)) {
            continue;
        }
        let diffs: Vec<CellRef> = match before {
            Some(before) => before.cells.diff(&after.cells),
            None => after.cells.iter().map(|(c, _)| c).collect(),
        };
        let mut anchors = std::collections::HashSet::new();
        for &c in &diffs {
            if !structural && after.cell(c).is_some_and(|cell| cell.formula.is_some()) {
                changed.push((si, c));
                anchors.insert(c);
            }
        }
        let updates = displayed_updates(&mut prec, wb, si, diffs.iter().copied());
        // A spill destination is no stored cell: reformatting it (or its row or column) leaves
        // no change in the store, so its anchor is recalculated to round the new element.
        if let (Some(before), false) = (before, structural) {
            for (&anchor, range) in &after.spill_ranges {
                if !anchors.contains(&anchor)
                    && (diffs.iter().any(|d| range.contains(*d))
                        || line_styles_differ(&before.rows, &after.rows, range.start.row, range.end.row)
                        || line_styles_differ(&before.cols, &after.cols, range.start.col, range.end.col))
                {
                    changed.push((si, anchor));
                }
            }
        }
        if updates.is_empty() {
            continue;
        }
        let Some(sh) = wb.sheet_mut(si) else { continue };
        for (c, r) in updates {
            if let Some(cell) = sh.cells.get_mut(c) {
                cell.value = gridcraft_core::Value::Number(r);
                if !structural {
                    changed.push((si, c));
                }
            }
        }
    }
}

/// Whether any row or column style in `lo..=hi` differs between two sheets' line maps.
fn line_styles_differ(
    before: &std::collections::BTreeMap<u32, gridcraft_model::LineInfo>,
    after: &std::collections::BTreeMap<u32, gridcraft_model::LineInfo>,
    lo: u32,
    hi: u32,
) -> bool {
    let style = |m: &std::collections::BTreeMap<u32, gridcraft_model::LineInfo>, k: u32| m.get(&k).and_then(|l| l.style);
    after.range(lo..=hi).any(|(k, l)| l.style != style(before, *k)) || before.range(lo..=hi).any(|(k, l)| l.style != style(after, *k))
}

/// Turning "Set precision as displayed" on rounds every number constant in the workbook to its
/// format, permanently (Excel warns "Data will permanently lose accuracy"). Formulas are rounded
/// by the recalculation that follows.
pub(crate) fn round_all_constants_as_displayed(wb: &mut Workbook) {
    let mut prec = gridcraft_calc::DisplayedPrecision::new();
    for si in 0..wb.sheets.len() {
        let Some(sh) = wb.sheets.get(si) else { continue };
        let updates = displayed_updates(&mut prec, wb, si, sh.cells.iter().map(|(c, _)| c));
        if updates.is_empty() {
            continue;
        }
        let Some(sh) = wb.sheet_mut(si) else { continue };
        for (c, r) in updates {
            if let Some(cell) = sh.cells.get_mut(c) {
                cell.value = gridcraft_core::Value::Number(r);
            }
        }
    }
}

/// The constants among `cells` on sheet `si` whose value the option changes, with their rounded
/// values. A locked constant on a protected sheet keeps its value: protection keeps its content,
/// and rounding it by its format would change the content that the sheet protects.
fn displayed_updates(
    prec: &mut gridcraft_calc::DisplayedPrecision,
    wb: &Workbook,
    si: usize,
    cells: impl Iterator<Item = CellRef>,
) -> Vec<(CellRef, f64)> {
    let Some(sh) = wb.sheet(si) else { return Vec::new() };
    cells
        .filter(|&c| !(sh.is_protected() && wb.styles.get(sh.style_id(c)).protection.locked))
        .filter_map(|c| {
            let cell = sh.cell(c)?;
            match cell.value {
                gridcraft_core::Value::Number(n) if cell.formula.is_none() => {
                    let r = prec.round(wb, si, c, n);
                    (r.to_bits() != n.to_bits()).then_some((c, r))
                }
                _ => None,
            }
        })
        .collect()
}
/// The most cells a dry run's touch log keeps; a footprint whose log is full may be missing writes
/// that kept a cell's value, so a shift is refused then.
pub(crate) const TOUCH_LOG_LIMIT: usize = 1_000_000;

/// Excel's message for an edit of a locked cell on a protected sheet.
pub(crate) const PROTECTED: &str = "The cell or chart you're trying to change is on a protected sheet.";

/// Refuses an edit that changed what a locked cell on a protected sheet holds (its value or
/// formula), or that unlocked it, whichever command made it. Sheets are matched by name, so
/// moving, renaming or deleting a sheet isn't an edit of its cells. A sort or duplicate removal
/// that moves an unlocked cell with equal content onto a locked one changes no content but would
/// leave the cell editable, so the effective lock (row and column formats included) is compared
/// as well.
fn check_protection(old: &Workbook, new: &Workbook, unlocked: &[(String, String)]) -> Result<()> {
    for sh in &old.sheets {
        if !sh.is_protected() {
            continue;
        }
        let Some(after) = new.sheet_index(&sh.name).and_then(|i| new.sheets.get(i)) else { continue };
        if std::sync::Arc::ptr_eq(sh, after) || !after.is_protected() {
            continue;
        }
        for c in sh.cells.diff(&after.cells) {
            let unlocked_now = !new.styles.get(after.style_id(c)).protection.locked;
            if (unlocked_now || !same_content(sh.cell(c), after.cell(c)))
                && old.styles.get(sh.style_id(c)).protection.locked
                && let Some(m) = locked_cell_error(sh, c, unlocked)
            {
                return Err(EngineError::Other(m));
            }
        }
    }
    Ok(())
}

/// Refuses an action a protected sheet does not allow, by the tick box for it in Review ▸ Protect
/// Sheet. `what` names the action in the message. An unprotected sheet allows everything.
///
/// Cell edits are guarded separately, by whether the cell is locked (`locked_cell_error`): these
/// permissions are the ones that are not about one cell's contents.
pub(crate) fn refuse_unless_allowed(s: &Session, sheet: usize, what: &str, pick: fn(&gridcraft_model::SheetProtection) -> bool) -> Result<()> {
    if s.doc()?.wb.sheet(sheet).is_some_and(|sh| sh.protection.as_ref().is_some_and(|pr| !pick(pr))) {
        return Err(EngineError::Other(format!("{what} isn't allowed on this protected sheet (Review › Unprotect Sheet).")));
    }
    Ok(())
}

/// Whether a locked cell on a protected sheet stays locked: `None` when it's in an Allow Edit
/// Range without a password or one unlocked in this session, else the message to show.
pub(crate) fn locked_cell_error(sh: &gridcraft_model::Sheet, c: CellRef, unlocked: &[(String, String)]) -> Option<String> {
    let mut needs_password = None;
    for r in sh.edit_ranges.iter().filter(|r| r.contains(c)) {
        // A range limited to Windows users or groups (a security descriptor) can't be checked
        // here, so without a password it stays locked rather than opening to every editor.
        if (r.password.is_none() && r.security_descriptor.is_none()) || unlocked.iter().any(|(s, t)| *s == sh.name && *t == r.title) {
            return None;
        }
        if r.password.is_some() {
            needs_password.get_or_insert(r.title.as_str());
        }
    }
    Some(match needs_password {
        Some(t) => format!("The range \"{t}\" needs its password before it can be changed (Review › Allow Edit Ranges)."),
        None => PROTECTED.into(),
    })
}

/// Refuses an edit that put something into a cell of a multi-cell legacy array other than its
/// formula cell, while the array stays (Excel: "You can't change part of an array."). The array
/// would show its value there while the file kept the new content. Cells shifted by row and
/// column edits aren't edits of their content, so structural commits aren't checked.
fn check_arrays(old: &Workbook, new: &Workbook) -> Result<()> {
    for sh in &old.sheets {
        if sh.spill.is_empty() {
            continue;
        }
        let Some(after) = new.sheet_index(&sh.name).and_then(|i| new.sheets.get(i)) else { continue };
        if std::sync::Arc::ptr_eq(sh, after) {
            continue;
        }
        for c in sh.cells.diff(&after.cells) {
            let filled = after.cell(c).is_some_and(gridcraft_model::Cell::has_content);
            if !filled || !sh.spill.contains_key(&c) || same_content(sh.cell(c), after.cell(c)) {
                continue;
            }
            let in_array = sh.spill_ranges.keys().any(|&anchor| {
                anchor != c
                    && after.cell(anchor).and_then(|x| x.formula.as_ref()).and_then(|f| f.array).is_some_and(|a| a.start == anchor && a.contains(c))
            });
            if in_array {
                return Err(EngineError::Other("You can't change part of an array.".into()));
            }
        }
    }
    Ok(())
}

/// Two cells hold the same value, formula or Python code (formats aside; a Python cell's result
/// is not something the person edits).
fn same_content(a: Option<&gridcraft_model::Cell>, b: Option<&gridcraft_model::Cell>) -> bool {
    let empty = |c: Option<&gridcraft_model::Cell>| c.is_none_or(|c| !c.has_content());
    match (a, b) {
        (Some(a), Some(b)) => match (&a.formula, &b.formula) {
            (Some(x), Some(y)) => x.text == y.text && x.array == y.array,
            (None, None) => match (&a.py, &b.py) {
                (Some(x), Some(y)) => x.code == y.code && x.object == y.object,
                (None, None) => a.value == b.value,
                _ => false,
            },
            _ => false,
        },
        _ => empty(a) && empty(b),
    }
}

/// Lines a single text line needs when word-wrapped into `width` points. The grid wraps whole
/// words (egui's LayoutJob), so dividing the total width by the column width undercounts: each
/// drawn line ends early at the last word that fits. A word wider than the column breaks
/// mid-word, the way egui does. (#1969)
fn wrapped_line_count(line: &str, width: f32, size: f32, bold: bool) -> f32 {
    let word_w = |w: &str| crate::cmd::format::approx_text_width(w, size, bold);
    let space_w = word_w(" ");
    let mut lines = 1.0f32;
    let mut used = 0.0f32;
    for word in line.split_whitespace() {
        let w = word_w(word);
        if w > width {
            // wider than the column: egui fills the current line, then breaks it across lines
            if used > 0.0 {
                lines += 1.0;
            }
            let chunks = (w / width).ceil();
            lines += chunks - 1.0;
            used = w - (chunks - 1.0) * width;
            continue;
        }
        let gap = if used > 0.0 { space_w } else { 0.0 };
        if used + gap + w > width {
            lines += 1.0;
            used = w;
        } else {
            used += gap + w;
        }
    }
    lines
}

/// Recomputes an automatic row height from its content: the largest font, and wrapped text.
/// Rows with a user-set height keep it.
///
/// Line breaks in a cell's text count only when the cell wraps, as in Excel: with Wrap Text off
/// a multi-line value shows on one line and AutoFit gives a one-line row.
pub fn auto_row_height(wb: &mut Workbook, si: usize, row: u32) {
    let mut sweep = MergeSweep::new(MERGE_FIT_WORK);
    fit_row_height(wb, si, row, &mut sweep);
}

/// Most merge comparisons one auto-fit pass (a commit, or opening a workbook) makes. Past it the
/// remaining rows keep the heights they have rather than stall on a sheet whose merges overlap
/// the fitted rows many times over.
pub(crate) const MERGE_FIT_WORK: usize = 50_000_000;

/// The merged areas of one sheet that touch a row, for rows visited in ascending order: the
/// merges are sorted by first row once and swept, so fitting many rows never rescans every merge
/// per row. All comparisons count against [`MERGE_FIT_WORK`].
pub(crate) struct MergeSweep {
    /// Built on the first row that needs merge checks, so rows that return early never pay for it.
    by_start: Option<Vec<gridcraft_core::RangeRef>>,
    next: usize,
    active: Vec<gridcraft_core::RangeRef>,
    budget: usize,
    /// The first row was answered by a plain scan: a pass that fits one row never pays for a sort.
    scanned: bool,
}

impl MergeSweep {
    /// A sweep for one sheet's merges with `budget` comparisons left for the pass (carry
    /// [`MergeSweep::left`] of the previous sheet's sweep into the next). Every call to
    /// [`MergeSweep::at`] must pass that same sheet.
    pub(crate) fn new(budget: usize) -> Self {
        Self { by_start: None, next: 0, active: Vec::new(), budget, scanned: false }
    }

    /// Comparisons left for the rest of the pass.
    pub(crate) fn left(&self) -> usize {
        self.budget
    }

    /// Takes `n` comparisons from the budget; `false` once it is spent.
    fn charge(&mut self, n: usize) -> bool {
        match self.budget.checked_sub(n) {
            Some(rest) => {
                self.budget = rest;
                true
            }
            None => {
                self.budget = 0;
                false
            }
        }
    }

    /// The merges covering `row`, or `None` when the budget is spent. Rows must not decrease
    /// between calls (skipping rows is fine).
    fn at(&mut self, sh: &gridcraft_model::Sheet, row: u32) -> Option<Vec<gridcraft_core::RangeRef>> {
        if self.by_start.is_none() && !self.scanned {
            self.scanned = true;
            if !self.charge(sh.merges.len()) {
                return None;
            }
            return Some(sh.merges.iter().copied().filter(|m| !m.is_single() && m.start.row <= row && row <= m.end.row).collect());
        }
        if self.by_start.is_none() {
            // Sorting is about n log2 n comparisons, charged before it starts.
            let n = sh.merges.len();
            if !self.charge(n.saturating_mul((usize::BITS - n.leading_zeros()) as usize)) {
                return None;
            }
            let mut v: Vec<gridcraft_core::RangeRef> = sh.merges.iter().copied().filter(|m| !m.is_single()).collect();
            v.sort_unstable_by_key(|m| m.start.row);
            self.by_start = Some(v);
        }
        let by_start = self.by_start.as_ref()?;
        while let Some(m) = by_start.get(self.next).filter(|m| m.start.row <= row) {
            self.active.push(*m);
            self.next += 1;
        }
        if !self.charge(self.active.len()) {
            return None;
        }
        self.active.retain(|m| m.end.row >= row);
        Some(self.active.clone())
    }
}

/// [`auto_row_height`] with the sheet's merges taken from `sweep`.
pub(crate) fn fit_row_height(wb: &mut Workbook, si: usize, row: u32, sweep: &mut MergeSweep) {
    // Out of budget: the row keeps the height it has.
    if sweep.left() == 0 {
        return;
    }
    let Some(sh) = wb.sheet(si) else { return };
    if sh.rows.get(&row).is_some_and(|i| i.custom) {
        return;
    }
    let default = sh.default_row_height;
    // Fast path: rows of default-size, unwrapped content keep the default height.
    let plain = sh.cells.row(row, 0, gridcraft_core::MAX_COLS - 1).all(|(_, cell)| {
        let st = wb.styles.get(cell.style);
        st.font.size <= gridcraft_model::DEFAULT_FONT_SIZE && !st.align.wrap
    });
    if plain {
        if sh.rows.get(&row).is_some_and(|i| i.size.is_some())
            && let Some(shm) = wb.sheet_mut(si)
            && let Some(e) = shm.rows.get_mut(&row)
        {
            {
                e.size = None;
                if *e == gridcraft_model::LineInfo::default() {
                    shm.rows.remove(&row);
                }
            }
        }
        return;
    }
    let mut need: f32 = default;
    // The cell loop must not rescan every merge per cell: the merges touching this row are
    // picked out once, and per cell the candidates are re-checked (normally none or a couple;
    // a sheet with many merges and many saved columns would otherwise scan all of them per
    // cell, rows × merges × columns).
    // Rows are fitted in ascending order, so the merges touching this one come from a sweep over
    // the sheet's merges sorted once, not from a scan of all of them. Out of budget: the row keeps
    // the height it has.
    let Some(row_merges) = sweep.at(sh, row) else { return };
    for (c, cell) in sh.cells.row(row, 0, gridcraft_core::MAX_COLS - 1) {
        if !sweep.charge(row_merges.len()) {
            return;
        }
        // Merged cells never auto-fit their rows, like Excel (column autofit skips them too).
        if row_merges.iter().any(|m| m.contains(CellRef::new(row, c))) {
            continue;
        }
        let st = wb.styles.get(cell.style);
        let size = st.font.size;
        // Scaled from an 11pt row's height, not the sheet's default: a sheet whose default
        // already fits a bigger font (21pt rows for 16pt text) would otherwise scale it twice.
        let line = (size * 1.8).round().max(gridcraft_model::DEFAULT_ROW_HEIGHT * size / gridcraft_model::DEFAULT_FONT_SIZE);
        let mut lines = 1.0f32;
        if !cell.value.is_empty() || cell.formula.is_some() {
            // Only a wrapping cell is laid out over several lines: each explicit line break
            // starts one, and each line folds at the column width.
            if st.align.wrap {
                let text = crate::display::cell_text(wb, sh, CellRef::new(row, c));
                let w = sh.col_width(c).max(1.0) - 6.0;
                lines = text.lines().map(|l| wrapped_line_count(l, w.max(1.0), size, st.font.bold)).sum::<f32>().max(1.0);
            }
        } else if st.font.size <= gridcraft_model::DEFAULT_FONT_SIZE {
            continue;
        }
        let h = if lines > 1.0 { (size * 96.0 / 72.0 * 1.22 * lines + 5.0).ceil() } else { line };
        need = need.max(h.min(546.0));
    }
    let Some(shm) = wb.sheet_mut(si) else { return };
    let e = shm.rows.entry(row).or_default();
    e.size = if (need - default).abs() < 0.5 { None } else { Some(need) };
    if *e == gridcraft_model::LineInfo::default() {
        shm.rows.remove(&row);
    }
}

impl Ctx<'_> {
    pub fn sheet_index(&self) -> usize {
        self.wb.active_sheet()
    }
    pub fn sheet_mut(&mut self, i: usize) -> Result<&mut gridcraft_model::Sheet> {
        self.wb.sheet_mut(i).ok_or_else(|| EngineError::Other("no such sheet".into()))
    }
    pub fn touch(&mut self, sheet: usize, c: CellRef) {
        self.changed.push((sheet, c));
    }
}

#[cfg(test)]
mod merge_sweep_tests {
    use super::*;

    #[test]
    fn the_budget_carries_from_one_sheets_sweep_to_the_next() {
        let mut sheet = gridcraft_model::Sheet::new("S");
        for r in ["A1:B2", "C1:D2", "E1:F2"] {
            sheet.merges.push(gridcraft_core::RangeRef::parse(r).unwrap());
        }
        let mut first = MergeSweep::new(5);
        assert!(first.at(&sheet, 0).is_some(), "the first row is a plain scan: three comparisons");
        assert!(first.at(&sheet, 1).is_none(), "the second row needs the sort: six comparisons, two are left");
        assert_eq!(first.left(), 0);
        // The next sheet starts with what is left: nothing, so it fits nothing.
        let mut second = MergeSweep::new(first.left());
        assert!(second.at(&sheet, 0).is_none());
        assert!(MergeSweep::new(MERGE_FIT_WORK).at(&sheet, 0).is_some());
    }

    #[test]
    fn sorting_the_merges_is_charged() {
        let mut sheet = gridcraft_model::Sheet::new("S");
        for r in ["A1:B2", "C1:D2", "E1:F2", "G1:H2"] {
            sheet.merges.push(gridcraft_core::RangeRef::parse(r).unwrap());
        }
        // The first row costs a plain scan (4); from the second on the merges are sorted, about
        // 4 * 3 comparisons: 7 left is not enough, 12 plus the row's 4 is.
        let mut sweep = MergeSweep::new(11);
        assert!(sweep.at(&sheet, 0).is_some());
        assert!(sweep.at(&sheet, 1).is_none());
        assert_eq!(sweep.left(), 0);
        let mut sweep = MergeSweep::new(20);
        assert!(sweep.at(&sheet, 0).is_some() && sweep.at(&sheet, 1).is_some());
    }
}
