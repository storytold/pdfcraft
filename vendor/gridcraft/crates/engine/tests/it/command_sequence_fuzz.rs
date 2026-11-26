//! Command-sequence fuzzing: reproducible random sequences of commands on the sample workbooks,
//! with invariants checked as they run. Per-command tests (`every_command_survives_empty_params`)
//! miss the bugs that need a sequence — insert rows under a spill, undo, sort, paste into a table —
//! and those are the ones that corrupt a workbook.
//!
//! Each seed picks a sample (sales, budget, grades) and runs `GRIDCRAFT_FUZZ_STEPS` commands. Most
//! come from a curated list of state-changing commands with hand-shaped params; the rest are drawn
//! from every registered command, with params generated from its `params` doc string.
//!
//! Invariants:
//! - no command panics (`Session::execute` reports a caught panic as [`EngineError::Internal`]),
//!   and saving, reloading or recalculating never panics either;
//! - a non-undoable command never changes cell content (it would be lost to undo);
//! - the structure stays valid: active sheet in range, sheet and table names valid and unique,
//!   merges and tables on the sheet and not overlapping, legacy arrays anchored at their
//!   top-left cell, the selection on the sheet;
//! - a full recalculation from scratch gives the values the incremental recalculation left
//!   (catches stale dependency graphs);
//! - every few steps and at the end, an XLSX save and reload (with the recalculation opening does)
//!   gives the same sheets, formulas and values;
//! - at the end, undoing everything gives back the starting workbook and redoing everything the
//!   final one (view state aside), and both keep the Watch Window list, which isn't an edit.
//!
//! Knobs (environment variables):
//! - `GRIDCRAFT_FUZZ_SEEDS` (default 6) and `GRIDCRAFT_FUZZ_STEPS` (default 40): the budget. Soak with
//!   `GRIDCRAFT_FUZZ_SEEDS=200 GRIDCRAFT_FUZZ_STEPS=200 cargo test --release -p gridcraft-engine --test it command_sequence_fuzz::`.
//! - `GRIDCRAFT_FUZZ_SEED=17`: replay that one seed (the failure message prints the command line).
//! - `GRIDCRAFT_FUZZ_SHRINK=1`: on failure, shrink the sequence to a minimal list of commands that
//!   still fails the same way, printed as `execute` calls ready for a regression test.
//! - `GRIDCRAFT_FUZZ_KEEP_GOING=1`: report every failing seed instead of stopping at the first.

use std::collections::BTreeSet;
use std::sync::Arc;

use gridcraft_core::{CellError, CellRef, MAX_COLS, MAX_ROWS, RangeRef, Value, col_to_letters};
use gridcraft_engine::calc::Calc;
use gridcraft_engine::model::{CalcMode, Formula, Sheet, SheetViewMode, Workbook};
use gridcraft_engine::{EngineError, HistoryEntry, Session, command_specs, find_command, io};
use serde_json::{Value as Json, json};

const SAMPLES: [&str; 3] = ["sales", "budget", "grades"];
/// Episodes end (undo/redo checks, then a fresh history) before the engine's 100-step undo limit
/// starts dropping the oldest entries.
const EPISODE_UNDO_DEPTH: usize = 90;
/// Save and reload every this many steps (and at the end of every episode).
const ROUNDTRIP_EVERY: usize = 10;
/// The hidden defined name `formulas.watchWindow` keeps its list in.
const WATCH_NAME: &str = "_sc_watch";

/// Commands the fuzzer never runs, and why.
const EXCLUDED: &[(&str, &str)] = &[
    ("file.new", "opens another document"),
    ("file.open", "opens another document"),
    ("file.close", "closes the document under test"),
    ("file.save", "writes files"),
    ("file.saveAs", "writes files"),
    ("file.exportCsv", "writes files"),
    ("file.exportHtml", "writes files"),
    ("file.exportPdf", "renders a PDF: slow, and may write files"),
    ("file.print", "renders a PDF: slow"),
    ("window.activate", "switches documents"),
    ("window.close", "closes the document under test"),
    ("view.newWindow", "opens another window, whose view swaps with the first"),
    ("insert.picture", "reads files"),
    ("pageLayout.background", "reads files"),
    ("formulas.calculationOptions", "manual and iterative calculation differ from a full recalc by design"),
];

// ---------------------------------------------------------------- deterministic randomness

/// SplitMix64: tiny, deterministic and good enough to pick commands.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    /// Inclusive range.
    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % ((hi - lo + 1).max(1) as u64)) as i64
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

// ---------------------------------------------------------------- harness

type Step = (String, Json);

#[derive(Debug)]
struct Failure {
    /// Index of the step that broke an invariant (`steps.len()` for the end-of-run checks).
    step: usize,
    /// Short category, used to keep shrinking on the same bug.
    kind: &'static str,
    what: String,
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

#[test]
fn command_sequences_keep_the_workbook_consistent() {
    let seeds = env_num("GRIDCRAFT_FUZZ_SEEDS", 6);
    let steps = env_num("GRIDCRAFT_FUZZ_STEPS", 40) as usize;
    let keep_going = std::env::var_os("GRIDCRAFT_FUZZ_KEEP_GOING").is_some();
    let list: Vec<u64> = match std::env::var("GRIDCRAFT_FUZZ_SEED").ok().and_then(|v| v.trim().parse().ok()) {
        Some(one) => vec![one],
        None => (0..seeds).collect(),
    };
    let mut reports = Vec::new();
    for seed in list {
        let (sample, sequence, failure) = run_seed(seed, steps);
        if let Some(f) = failure {
            reports.push(report(seed, steps, sample, &sequence, &f));
            if !keep_going {
                break;
            }
        }
    }
    assert!(reports.is_empty(), "{} failing seed(s)\n\n{}", reports.len(), reports.join("\n\n"));
}

fn report(seed: u64, steps: usize, sample: &str, sequence: &[Step], f: &Failure) -> String {
    let at = match sequence.get(f.step) {
        Some((id, p)) => format!("step {} of {}: `{id}` {p}", f.step, sequence.len()),
        None => format!("end-of-run checks after {} steps", sequence.len()),
    };
    let mut out = format!(
        "command-sequence fuzz failure: seed {seed} (sample `{sample}`), {at}\n  invariant [{}]: {}\n  replay: GRIDCRAFT_FUZZ_SEED={seed} GRIDCRAFT_FUZZ_STEPS={steps} cargo test -p gridcraft-engine --test it command_sequence_fuzz::\n",
        f.kind, f.what
    );
    let end = (f.step + 1).min(sequence.len());
    let from = end.saturating_sub(30);
    out.push_str(&format!("  steps {from}..{end}:\n"));
    for (i, (id, p)) in sequence.iter().enumerate().take(end).skip(from) {
        out.push_str(&format!("    {i:4} {id} {p}\n"));
    }
    if std::env::var_os("GRIDCRAFT_FUZZ_SHRINK").is_some() {
        let min = shrink(sample, &sequence[..end], f.kind);
        out.push_str(&format!("  minimal sequence ({} steps) from file.new {{\"sample\": \"{sample}\"}}:\n", min.len()));
        for (id, p) in &min {
            out.push_str(&format!("    s.execute(\"{id}\", json!({p})).ok();\n"));
        }
        if let Some(f) = replay(sample, &min) {
            out.push_str(&format!("  which fails with [{}]: {}\n", f.kind, f.what));
        }
    } else {
        out.push_str("  (set GRIDCRAFT_FUZZ_SHRINK=1 for a minimal sequence)\n");
    }
    out
}

/// Generates and runs one seed's sequence; returns the sample, the steps run and the first failure.
fn run_seed(seed: u64, steps: usize) -> (&'static str, Vec<Step>, Option<Failure>) {
    let mut rng = Rng(seed.wrapping_mul(0xA24B_AED4_963E_E407) ^ 0x5EED);
    let sample = SAMPLES[(seed % SAMPLES.len() as u64) as usize];
    let mut h = Harness::new(sample);
    let mut sequence = Vec::new();
    for i in 0..steps {
        let (id, params) = generate(&mut rng, &h.s);
        sequence.push((id.clone(), params.clone()));
        if let Err(f) = h.step(i, &id, &params) {
            return (sample, sequence, Some(f));
        }
    }
    let failure = h.finish(steps).err();
    (sample, sequence, failure)
}

/// Runs a recorded sequence with the same checks.
fn replay(sample: &str, steps: &[Step]) -> Option<Failure> {
    let mut h = Harness::new(sample);
    for (i, (id, p)) in steps.iter().enumerate() {
        if let Err(f) = h.step(i, id, p) {
            return Some(f);
        }
    }
    h.finish(steps.len()).err()
}

/// Greedy delta debugging: drop chunks of steps while the run still fails the same way.
fn shrink(sample: &str, steps: &[Step], kind: &str) -> Vec<Step> {
    let mut cur = steps.to_vec();
    let mut chunk = cur.len().div_ceil(2).max(1);
    loop {
        let mut i = 0;
        let mut removed = false;
        while i < cur.len() {
            let mut trial = cur.clone();
            trial.drain(i..(i + chunk).min(trial.len()));
            if replay(sample, &trial).is_some_and(|f| f.kind == kind) {
                cur = trial;
                removed = true;
            } else {
                i += chunk;
            }
        }
        if chunk == 1 && !removed {
            return cur;
        }
        if !removed {
            chunk = chunk.div_ceil(2);
        }
    }
}

struct Harness {
    s: Session,
    /// What undoing everything gives back: the workbook at the start of the current episode or,
    /// once the undo history has dropped its oldest steps (it is bounded by count and by the
    /// memory its snapshots hold), the state before the oldest step it kept.
    floor: Arc<Workbook>,
    /// The workbook of the oldest undo step when the history was last looked at, to notice the
    /// history dropping it.
    oldest: Option<Arc<Workbook>>,
}

/// Where redoing the `n` steps just undone ends. The first of them went onto the redo history
/// with the final workbook `end`. The redo history is bounded like undo and drops its oldest
/// entries first, so when that entry is gone every older one is too, and redo stops after fewer
/// than `n` steps at the oldest entry it kept.
fn redo_target(end: &Arc<Workbook>, redo: &[HistoryEntry], n: usize) -> Arc<Workbook> {
    if n == 0 || redo.iter().rev().take(n).any(|e| Arc::ptr_eq(&e.wb, end)) {
        return end.clone();
    }
    redo.first().map_or_else(|| end.clone(), |e| e.wb.clone())
}

fn fail(step: usize, kind: &'static str, what: impl Into<String>) -> Failure {
    Failure { step, kind, what: what.into() }
}

/// Runs `f`, turning a panic into a failure (for checks that run outside `Session::execute`).
fn guarded<T>(step: usize, kind: &'static str, f: impl FnOnce() -> Result<T, String>) -> Result<T, Failure> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(fail(step, kind, e)),
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(ToString::to_string).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
            Err(fail(step, kind, format!("panicked: {msg}")))
        }
    }
}

impl Harness {
    fn new(sample: &str) -> Harness {
        let mut s = Session::new();
        s.execute("file.new", json!({"sample": sample})).expect("sample opens");
        let floor = s.doc().expect("document").wb.clone();
        Harness { s, floor, oldest: None }
    }

    fn wb(&self) -> Arc<Workbook> {
        self.s.doc().expect("document").wb.clone()
    }

    fn step(&mut self, i: usize, id: &str, params: &Json) -> Result<(), Failure> {
        let before = self.wb();
        let r = self.s.execute(id, params.clone());
        self.s.take_ui_requests();
        if let Err(EngineError::Internal(_, msg)) = &r {
            return Err(fail(i, "panic", format!("`{id}` panicked (caught by Session::execute): {msg}")));
        }
        if self.s.documents().len() != 1 {
            return Err(fail(i, "documents", format!("`{id}` changed the number of open documents; exclude it in EXCLUDED")));
        }
        let after = self.wb();
        self.track_history();
        let spec = find_command(id).expect("generated ids are registered");
        if !spec.undoable
            && !matches!(id, "edit.undo" | "edit.redo")
            && let Some(diff) = content_diff(&before, &after)
        {
            return Err(fail(i, "not undoable", format!("non-undoable `{id}` changed cell content: {diff}")));
        }
        self.check_state(i)?;
        if (i + 1).is_multiple_of(ROUNDTRIP_EVERY) {
            guarded(i, "xlsx roundtrip", || roundtrip_diff(&after))?;
        }
        if self.s.doc().expect("document").undo.len() >= EPISODE_UNDO_DEPTH {
            self.end_episode(i)?;
        }
        Ok(())
    }

    /// Structure and recalculation invariants on the current state.
    fn check_state(&self, i: usize) -> Result<(), Failure> {
        let d = self.s.doc().expect("document");
        if let Some(e) = structure_problem(&d.wb, &d.selection) {
            return Err(fail(i, "structure", e));
        }
        let wb = d.wb.clone();
        guarded(i, "recalc", || recalc_diff(&wb))
    }

    /// Undo everything, compare with the start, redo everything, compare with the end, then save
    /// and reload; the next episode starts from here with an empty history.
    fn end_episode(&mut self, i: usize) -> Result<(), Failure> {
        let end = self.wb();
        self.track_history();
        let mut n = 0;
        while !self.s.doc().expect("document").undo.is_empty() {
            if let Err(EngineError::Internal(_, msg)) = self.s.execute("edit.undo", json!({})) {
                return Err(fail(i, "panic", format!("`edit.undo` panicked while undoing everything: {msg}")));
            }
            n += 1;
            if n > 1_000 {
                return Err(fail(i, "undo", "undo never empties its history"));
            }
        }
        let undone = self.wb();
        if let Some(diff) = workbook_diff(&self.floor, &undone) {
            return Err(fail(i, "undo", format!("undoing all {n} steps doesn't give back the starting workbook: {diff}")));
        }
        if watch_list(&end) != watch_list(&undone) {
            return Err(fail(i, "undo", format!("undoing all {n} steps changed the Watch Window list (not an edit)")));
        }
        self.check_state(i)?;
        // Only the steps undone here: an `edit.undo {steps}` in the sequence may have left more.
        let target = redo_target(&end, &self.s.doc().expect("document").redo, n);
        for _ in 0..n {
            if let Err(EngineError::Internal(_, msg)) = self.s.execute("edit.redo", json!({})) {
                return Err(fail(i, "panic", format!("`edit.redo` panicked while redoing everything: {msg}")));
            }
        }
        if let Some(diff) = workbook_diff(&target, &self.wb()) {
            let what = if Arc::ptr_eq(&target, &end) { "the final workbook" } else { "the newest step the redo history kept" };
            return Err(fail(i, "redo", format!("redoing all {n} steps doesn't give back {what}: {diff}")));
        }
        self.check_state(i)?;
        guarded(i, "xlsx roundtrip", || roundtrip_diff(&end))?;
        let d = self.s.active_mut().expect("document");
        d.undo.clear();
        d.redo.clear();
        self.floor = d.wb.clone();
        self.oldest = None;
        Ok(())
    }

    /// Notices the undo history dropping its oldest step: its oldest entry changed without the
    /// history emptying in between. From then on undoing everything stops at the new oldest entry,
    /// including after an `edit.undo` in the sequence has already used up what is left.
    fn track_history(&mut self) {
        let oldest = self.s.doc().expect("document").undo.first().map(|e| e.wb.clone());
        if let (Some(was), Some(now)) = (&self.oldest, &oldest)
            && !Arc::ptr_eq(was, now)
        {
            self.floor = now.clone();
        }
        self.oldest = oldest;
    }

    fn finish(&mut self, steps: usize) -> Result<(), Failure> {
        self.end_episode(steps)
    }
}

// ---------------------------------------------------------------- invariants

fn has_content(c: &gridcraft_engine::model::Cell) -> bool {
    c.formula.is_some() || !c.value.is_empty()
}

/// Cells whose formula or constant differs (computed values of formulas aside).
fn content_diff(a: &Workbook, b: &Workbook) -> Option<String> {
    let names = |wb: &Workbook| wb.sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    if names(a) != names(b) {
        return Some(format!("sheets {:?} became {:?}", names(a), names(b)));
    }
    for (x, y) in a.sheets.iter().zip(&b.sheets) {
        if Arc::ptr_eq(x, y) {
            continue;
        }
        for c in x.cells.diff(&y.cells) {
            let content = |s: &Sheet| {
                s.cell(c).filter(|c| has_content(c)).map(|c| match &c.formula {
                    Some(f) => format!("={}", f.text),
                    None => format!("{:?}", c.value),
                })
            };
            if content(x) != content(y) {
                return Some(format!("{}!{c}: {:?} became {:?}", x.name, content(x), content(y)));
            }
        }
    }
    None
}

fn overlaps(a: &RangeRef, b: &RangeRef) -> bool {
    a.intersection(b).is_some()
}

fn structure_problem(wb: &Workbook, sel: &gridcraft_engine::Selection) -> Option<String> {
    if wb.sheets.is_empty() {
        return Some("no sheets left".into());
    }
    if wb.active_sheet() >= wb.sheets.len() {
        return Some(format!("active sheet {} of {}", wb.active_sheet(), wb.sheets.len()));
    }
    let mut tables = BTreeSet::new();
    for (i, sh) in wb.sheets.iter().enumerate() {
        if let Err(e) = wb.check_sheet_name(&sh.name, Some(i)) {
            return Some(format!("sheet name `{}`: {e}", sh.name));
        }
        let on_sheet = |r: &RangeRef| r.start.is_valid() && r.end.is_valid() && r.start.row <= r.end.row && r.start.col <= r.end.col;
        if let Some(r) = sh.merges.iter().chain(sh.tables.iter().map(|t| &t.range)).find(|r| !on_sheet(r)) {
            return Some(format!("{}: merge or table range {r:?} isn't on the sheet", sh.name));
        }
        // A legacy array formula lives in the top-left cell of its range.
        for (c, cell) in sh.cells.iter() {
            if let Some(a) = cell.formula.as_ref().and_then(|f| f.array)
                && (a.start != c || !on_sheet(&a))
            {
                return Some(format!("{}!{c}: array formula covers {a}", sh.name));
            }
        }
        for (k, m) in sh.merges.iter().enumerate() {
            if let Some(o) = sh.merges.iter().skip(k + 1).find(|o| overlaps(m, o)) {
                return Some(format!("{}: merges {m} and {o} overlap", sh.name));
            }
        }
        for (k, t) in sh.tables.iter().enumerate() {
            if !tables.insert(t.name.to_ascii_lowercase()) {
                return Some(format!("two tables named `{}`", t.name));
            }
            if t.columns.len() as u32 != t.range.end.col - t.range.start.col + 1 {
                return Some(format!("table `{}` at {} has {} columns", t.name, t.range, t.columns.len()));
            }
            if let Some(o) = sh.tables.iter().skip(k + 1).find(|o| overlaps(&t.range, &o.range)) {
                return Some(format!("{}: tables `{}` ({}) and `{}` ({}) overlap", sh.name, t.name, t.range, o.name, o.range));
            }
        }
    }
    if sel.ranges.is_empty() || !sel.active.is_valid() || sel.ranges.iter().any(|r| !r.start.is_valid() || !r.end.is_valid()) {
        return Some(format!("selection {sel:?}"));
    }
    None
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
        _ => a == b,
    }
}

/// The first cell whose displayed value (own value or spilled) differs between two workbooks.
fn value_diff(a: &Workbook, b: &Workbook, what: &str) -> Option<String> {
    for (x, y) in a.sheets.iter().zip(&b.sheets) {
        let mut keys: BTreeSet<CellRef> = BTreeSet::new();
        for s in [x, y] {
            keys.extend(s.cells.iter().filter(|(_, c)| has_content(c)).map(|(c, _)| c));
            keys.extend(s.spill.keys().copied());
        }
        for c in keys {
            let (va, vb) = (x.value(c), y.value(c));
            // A file caches the Python errors as #VALUE! (they aren't error codes of the file
            // format), in a Python cell and in the formulas that read one; reading it back runs
            // nothing.
            let python_error = matches!(va, Value::Error(CellError::Python | CellError::Busy | CellError::Timeout | CellError::Blocked))
                && vb == Value::Error(CellError::Value);
            if !same_value(&va, &vb) && !python_error {
                let f = x.cell(c).and_then(|c| c.formula.as_ref()).map(|f| format!(" (formula ={})", f.text)).unwrap_or_default();
                return Some(format!("{}!{c}{f}: {va:?} in memory, {vb:?} after {what}", x.name));
            }
        }
    }
    None
}

/// A full recalculation from scratch must agree with the incrementally maintained values.
fn recalc_diff(wb: &Workbook) -> Result<(), String> {
    if wb.calc.mode != CalcMode::Automatic || wb.calc.iterative {
        return Ok(());
    }
    let mut fresh = wb.clone();
    Calc::new().recalc_all(&mut fresh);
    value_diff(wb, &fresh, "a full recalc").map_or(Ok(()), Err)
}

fn normalized_formula(f: &Formula) -> String {
    let text = Formula::new(&f.text).text;
    if f.array.is_some() { format!("{{={text}}}") } else { format!("={text}") }
}

/// Save to XLSX, reload (recalculating like opening does) and compare sheets, formulas and values.
fn roundtrip_diff(wb: &Workbook) -> Result<(), String> {
    let bytes = io::save_bytes(wb, "fuzz.xlsx").map_err(|e| format!("save failed: {e}"))?;
    let (mut back, _) = io::open_bytes("fuzz.xlsx", &bytes).map_err(|e| format!("reload failed: {e}"))?;
    Calc::new().recalc_all(&mut back);
    let names = |wb: &Workbook| wb.sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    if names(wb) != names(&back) {
        return Err(format!("sheets {:?} reload as {:?}", names(wb), names(&back)));
    }
    for (x, y) in wb.sheets.iter().zip(&back.sheets) {
        let formulas = |s: &Sheet| s.cells.iter().filter_map(|(c, cell)| Some((c, normalized_formula(cell.formula.as_ref()?)))).collect::<Vec<_>>();
        let (fx, fy) = (formulas(x), formulas(y));
        if fx != fy {
            let first = fx.iter().zip(&fy).find(|(a, b)| a != b).map(|(a, b)| format!("{a:?} vs {b:?}"));
            let first = first.unwrap_or_else(|| format!("{} formulas vs {}", fx.len(), fy.len()));
            return Err(format!("{}: formulas differ after reload: {first}", x.name));
        }
    }
    if wb.calc.mode == CalcMode::Automatic && !wb.calc.iterative {
        value_diff(wb, &back, "an XLSX save and reload").map_or(Ok(()), Err)?;
    }
    Ok(())
}

/// The Watch Window list, which is workbook state but not an edit (undo keeps it).
fn watch_list(wb: &Workbook) -> Option<&str> {
    wb.names.iter().find(|n| n.name == WATCH_NAME).map(|n| n.formula.as_str())
}

/// The workbook with view state (active sheet, scroll, active cell, Normal / Page Layout / Page
/// Break Preview) and the Watch Window list reset: undo restores view state as it was, but
/// switching sheets or views isn't an undoable edit.
fn canonical(wb: &Workbook) -> Workbook {
    let mut wb = wb.clone();
    wb.set_active_sheet(0);
    wb.names.retain(|n| n.name != WATCH_NAME);
    for sh in wb.sheets.iter_mut() {
        let sh = Arc::make_mut(sh);
        sh.view_active = CellRef::default();
        sh.view_top_left = CellRef::default();
        sh.view = SheetViewMode::Normal;
    }
    wb
}

fn workbook_diff(a: &Workbook, b: &Workbook) -> Option<String> {
    let (a, b) = (canonical(a), canonical(b));
    if a == b {
        return None;
    }
    if let Some(d) = content_diff(&a, &b) {
        return Some(d);
    }
    for (x, y) in a.sheets.iter().zip(&b.sheets) {
        if x != y {
            if let Some(c) = x.cells.diff(&y.cells).first() {
                return Some(format!("{}!{c}: {:?} vs {:?}", x.name, x.cell(*c), y.cell(*c)));
            }
            let field = [
                ("merges", x.merges != y.merges),
                ("tables", x.tables != y.tables),
                ("charts", x.charts != y.charts),
                ("rows", x.rows != y.rows),
                ("cols", x.cols != y.cols),
                ("autofilter", x.autofilter != y.autofilter),
                ("cond_formats", x.cond_formats != y.cond_formats),
                ("validations", x.validations != y.validations),
                ("spill", x.spill != y.spill || x.spill_ranges != y.spill_ranges),
            ]
            .into_iter()
            .find(|(_, d)| *d)
            .map_or("other sheet properties", |(n, _)| n);
            return Some(format!("{}: {field} differ", x.name));
        }
    }
    Some("workbook-level properties (names, styles, settings) differ".into())
}

// ---------------------------------------------------------------- generation

fn letter(c: u32) -> String {
    col_to_letters(c)
}

/// A row/column index: mostly near the data, sometimes further out, rarely at the sheet's edge.
fn coord(rng: &mut Rng, near: i64, far: i64, max: u32) -> u32 {
    match rng.below(20) {
        0 => max - 1 - rng.int(0, 2) as u32,
        1 | 2 => rng.int(near, far) as u32,
        _ => rng.int(0, near) as u32,
    }
}

fn cell(rng: &mut Rng) -> CellRef {
    CellRef::new(coord(rng, 24, 120, MAX_ROWS), coord(rng, 11, 40, MAX_COLS))
}

/// A cell near the data (formula references stay small).
fn near_cell(rng: &mut Rng) -> CellRef {
    CellRef::new(rng.int(0, 22) as u32, rng.int(0, 10) as u32)
}

fn near_range(rng: &mut Rng) -> RangeRef {
    let a = near_cell(rng);
    let b = CellRef::new(a.row + rng.int(0, 7) as u32, a.col + rng.int(0, 4) as u32);
    RangeRef::new(a, b)
}

fn range(rng: &mut Rng) -> String {
    match rng.below(20) {
        0 | 1 => {
            let c = rng.int(0, 10) as u32;
            format!("{}:{}", letter(c), letter(c + rng.int(0, 2) as u32))
        }
        2 | 3 => {
            let r = rng.int(1, 25);
            format!("{r}:{}", r + rng.int(0, 2))
        }
        4 => cell(rng).a1(),
        5 => {
            let a = cell(rng);
            RangeRef::new(a, a.offset_clamped(rng.int(0, 3), rng.int(0, 3))).a1()
        }
        _ => near_range(rng).a1(),
    }
}

fn rows_spec(rng: &mut Rng) -> String {
    let r = coord(rng, 24, 120, MAX_ROWS) as i64 + 1;
    format!("{r}:{}", (r + rng.int(0, 2)).min(MAX_ROWS as i64))
}

fn cols_spec(rng: &mut Rng) -> String {
    let c = coord(rng, 11, 40, MAX_COLS);
    format!("{}:{}", letter(c), letter((c + rng.int(0, 1) as u32).min(MAX_COLS - 1)))
}

fn quoted(name: &str) -> String {
    format!("'{}'", name.replace('\'', "''"))
}

fn wb_of(s: &Session) -> &Workbook {
    &s.doc().expect("document").wb
}

fn spill_anchors(wb: &Workbook) -> Vec<(usize, CellRef)> {
    wb.sheets.iter().enumerate().flat_map(|(i, sh)| sh.spill_ranges.keys().map(move |c| (i, *c))).collect()
}

fn tables(wb: &Workbook) -> Vec<(usize, &gridcraft_engine::model::Table)> {
    wb.sheets.iter().enumerate().flat_map(|(i, sh)| sh.tables.iter().map(move |t| (i, t))).collect()
}

fn other_sheet(rng: &mut Rng, wb: &Workbook) -> String {
    quoted(&wb.sheets[rng.below(wb.sheets.len())].name)
}

fn formula(rng: &mut Rng, wb: &Workbook) -> String {
    let c = near_cell(rng).a1();
    let c2 = near_cell(rng).a1();
    let r = near_range(rng).a1();
    let n = rng.int(1, 6);
    let col = letter(rng.int(0, 9) as u32);
    let tabs = tables(wb);
    match rng.below(30) {
        0 => format!("={c}+{n}"),
        1 => format!("=SUM({r})"),
        2 => format!("={c}*{c2}"),
        3 => format!("={}!{c}*2", other_sheet(rng, wb)),
        4 => format!("=SEQUENCE({n},{})", rng.int(1, 3)),
        5 | 6 if !tabs.is_empty() => {
            let (_, t) = rng.pick(&tabs);
            let column = t.columns.get(rng.below(t.columns.len())).map(|c| c.name.clone()).unwrap_or_default();
            match rng.below(4) {
                0 => format!("=SUM({}[{column}])", t.name),
                1 => format!("=ROWS({})", t.name),
                2 => format!("={}[@{column}]", t.name),
                _ => format!("=COUNTA({}[#All])", t.name),
            }
        }
        7 => format!("=INDIRECT(\"{c}\")"),
        8 => format!("=OFFSET({c},1,1)"),
        9 => format!("=IF({c}>{n},\"big\",{c})"),
        10 => {
            let anchors = spill_anchors(wb);
            match anchors.get(rng.below(anchors.len().max(1))) {
                Some((si, a)) => format!("=SUM({}!{a}#)", quoted(&wb.sheets[*si].name)),
                None => format!("={c}#"),
            }
        }
        11 => format!("=SORT({r})"),
        12 => format!("=UNIQUE({r})"),
        13 => format!("=TRANSPOSE({r})"),
        14 => format!("=COUNTIF({col}:{col},\">0\")"),
        15 => format!("=SUM({}:{}!{c})", other_sheet(rng, wb), other_sheet(rng, wb)),
        16 => format!("=LET(x,{c},x*2)"),
        17 => format!("=XLOOKUP({c},{col}1:{col}20,{col}1:{col}20)"),
        18 => format!("=FILTER({col}1:{col}20,{col}1:{col}20>{n})"),
        19 => match wb.names.get(rng.below(wb.names.len().max(1))) {
            Some(nm) => format!("=SUM({})", nm.name),
            None => format!("={c}"),
        },
        20 => "=ROW()+COLUMN()".into(),
        21 => format!("={c}&\"-\"&{c2}"),
        22 => format!("=AVERAGE({r})*{n}"),
        23 => format!("=MAX({r})-MIN({r})"),
        24 => rng.pick(&["=1/0", "=NA()", "=#REF!+1", "=NOSUCHFN(1)", "=SUM(", "=\"text\""]).to_string(),
        25 => format!("={r}"),
        26 => format!("=INDEX({r},{n},1)"),
        27 => format!("=VLOOKUP({c},{r},2,FALSE)"),
        28 => format!("=SEQUENCE({n})*{c}"),
        _ => format!("=TEXT({c},\"0.00\")"),
    }
}

fn literal(rng: &mut Rng) -> String {
    match rng.below(14) {
        0..=3 => rng.int(-50, 500).to_string(),
        4 => format!("{}.{}", rng.int(-9, 99), rng.int(0, 99)),
        5 => rng.pick(&["North", "South", "East", "West", "abc", "Total", "x y"]).to_string(),
        6 => rng.pick(&["TRUE", "FALSE"]).to_string(),
        7 => format!("{}%", rng.int(0, 100)),
        8 => format!("2026-{:02}-{:02}", rng.int(1, 12), rng.int(1, 28)),
        9 => String::new(),
        10 => "'007".into(),
        11 => "$1,234.50".into(),
        12 => "#N/A".into(),
        _ => "1e3".into(),
    }
}

fn input(rng: &mut Rng, wb: &Workbook) -> String {
    if rng.chance(55) { formula(rng, wb) } else { literal(rng) }
}

fn json_input(rng: &mut Rng, wb: &Workbook) -> Json {
    match rng.below(10) {
        0..=1 => json!(rng.int(-20, 200)),
        2 => Json::Null,
        3 => json!(rng.chance(50)),
        _ => json!(input(rng, wb)),
    }
}

fn values_2d(rng: &mut Rng, wb: &Workbook) -> Json {
    let (h, w) = (rng.int(1, 4), rng.int(1, 3));
    Json::Array((0..h).map(|_| Json::Array((0..w).map(|_| json_input(rng, wb)).collect())).collect())
}

fn sheet_param(rng: &mut Rng, wb: &Workbook) -> Json {
    match rng.below(10) {
        0 => json!(wb.sheets.len() + rng.below(2)),
        1..=4 => json!(wb.sheets[rng.below(wb.sheets.len())].name),
        _ => json!(rng.below(wb.sheets.len())),
    }
}

fn new_name(rng: &mut Rng) -> String {
    rng.pick(&["Data", "Q4 2026", "Summary", "Sheet9", "x", "Sales", "Long name of a sheet 31 chars!", "it's", "Grades"]).to_string()
}

fn color(rng: &mut Rng) -> String {
    rng.pick(&["#FF0000", "#00B050", "#FFFF00", "#1F4E79", "none"]).to_string()
}

fn table_param(rng: &mut Rng, wb: &Workbook) -> Json {
    let tabs = tables(wb);
    match tabs.get(rng.below(tabs.len() + 1)) {
        Some((_, t)) => json!(t.name),
        None => json!("NoSuchTable"),
    }
}

fn chart_ids(wb: &Workbook) -> Vec<u32> {
    wb.sheets.iter().flat_map(|s| s.charts.iter().map(|c| c.id)).collect()
}

/// Curated state-changing commands and their weights.
const CURATED: &[(&str, usize)] = &[
    ("cell.set", 22),
    ("range.setValues", 5),
    ("range.fill", 2),
    ("selection.set", 8),
    ("edit.undo", 6),
    ("edit.redo", 3),
    ("edit.copy", 3),
    ("edit.cut", 2),
    ("edit.paste", 4),
    ("edit.pasteSpecial", 2),
    ("edit.clearContents", 2),
    ("edit.clearAll", 1),
    ("edit.fillDown", 2),
    ("edit.fillRight", 1),
    ("edit.autoFill", 2),
    ("edit.fillSeries", 1),
    ("home.bold", 1),
    ("home.fillColor", 1),
    ("home.numberFormat", 1),
    ("home.borders", 1),
    ("home.mergeCenter", 2),
    ("home.mergeCells", 1),
    ("home.unmergeCells", 1),
    ("home.insertRows", 4),
    ("home.insertColumns", 3),
    ("home.deleteRows", 4),
    ("home.deleteColumns", 3),
    ("home.insertCells", 2),
    ("home.deleteCells", 2),
    ("home.hideRows", 1),
    ("home.unhideRows", 1),
    ("home.insertSheet", 2),
    ("home.deleteSheet", 2),
    ("sheet.rename", 2),
    ("sheet.move", 2),
    ("sheet.activate", 3),
    ("insert.table", 3),
    ("table.totalRow", 1),
    ("table.resize", 1),
    ("table.convertToRange", 1),
    ("insert.chart", 2),
    ("chart.delete", 1),
    ("data.sortAscending", 2),
    ("data.sort", 2),
    ("data.filter", 2),
    ("data.filterBy", 2),
    ("data.clearFilter", 1),
    ("data.removeDuplicates", 1),
    ("formulas.defineName", 2),
    ("formulas.deleteName", 1),
    ("edit.replace", 1),
    ("python.set", 3),
    ("python.output", 2),
];

fn generate(rng: &mut Rng, s: &Session) -> Step {
    let wb = wb_of(s);
    if rng.chance(82) {
        let total: usize = CURATED.iter().map(|(_, w)| w).sum();
        let mut roll = rng.below(total);
        for (id, w) in CURATED {
            if roll < *w {
                return (id.to_string(), curated_params(rng, wb, id));
            }
            roll -= w;
        }
    }
    let specs: Vec<_> = command_specs().iter().filter(|c| !EXCLUDED.iter().any(|(x, _)| *x == c.id)).collect();
    let spec = specs[rng.below(specs.len())];
    (spec.id.to_string(), doc_params(rng, wb, spec.params))
}

fn curated_params(rng: &mut Rng, wb: &Workbook, id: &str) -> Json {
    let maybe_range = |rng: &mut Rng| if rng.chance(70) { json!({"range": range(rng)}) } else { json!({}) };
    match id {
        "cell.set" => {
            let mut p = json!({"cell": cell(rng).a1(), "input": input(rng, wb)});
            if rng.chance(10) {
                p["sheet"] = sheet_param(rng, wb);
            }
            if rng.chance(4) {
                p["array"] = json!(true);
            }
            p
        }
        "range.setValues" => json!({"range": near_cell(rng).a1(), "values": values_2d(rng, wb)}),
        "python.set" => {
            // Code with xl() references (which follow structural edits), quotes and line breaks;
            // sometimes long enough to meet the formula limit.
            let mut code = match rng.below(5) {
                0 => format!("xl(\"{}\") + 1", near_range(rng).a1()),
                1 => format!("a = xl('{}')\nb = \"q\\\"uote\"\na", near_cell(rng).a1()),
                2 => "[[1, 2], [3, 4]]".to_string(),
                3 => String::new(),
                _ => "\"".repeat(rng.int(1, 5000) as usize),
            };
            if rng.chance(5) {
                code.push_str(&"x".repeat(8_100));
            }
            let mut p = json!({"cell": near_cell(rng).a1(), "code": code});
            if rng.chance(30) {
                p["output"] = json!(if rng.chance(50) { "object" } else { "value" });
            }
            p
        }
        "python.output" => {
            // Mostly the cells python.set writes to; a missing mode switches.
            let mut p = json!({"cell": near_cell(rng).a1()});
            if rng.chance(50) {
                p["mode"] = json!(if rng.chance(50) { "object" } else { "value" });
            }
            p
        }
        "range.fill" => json!({"range": near_range(rng).a1(), "input": input(rng, wb)}),
        "selection.set" => {
            let mut p = json!({"range": range(rng)});
            if rng.chance(15) {
                p["range"] = json!(format!("{},{}", near_range(rng).a1(), near_cell(rng).a1()));
            }
            p
        }
        "edit.undo" | "edit.redo" => json!({"steps": rng.int(1, 3)}),
        "edit.copy" | "edit.cut" => maybe_range(rng),
        "edit.paste" => {
            if rng.chance(15) {
                json!({"at": near_cell(rng).a1(), "text": "1\t2\nx\t=A1"})
            } else {
                json!({"at": cell(rng).a1()})
            }
        }
        "edit.pasteSpecial" => json!({
            "what": rng.pick(&["all", "formulas", "values", "formats", "allExceptBorders", "columnWidths", "valuesAndNumberFormats"]),
            "operation": rng.pick(&["none", "add", "multiply"]),
            "skipBlanks": rng.chance(30),
            "transpose": rng.chance(30),
        }),
        "edit.autoFill" => {
            let src = near_range(rng);
            let tgt = RangeRef::new(src.start, CellRef::new(src.end.row + rng.int(0, 6) as u32, src.end.col + rng.int(0, 1) as u32));
            json!({"source": src.a1(), "target": tgt.a1(), "mode": rng.pick(&["series", "copy", "alternate", "formats", "values"])})
        }
        "edit.fillSeries" => json!({"range": near_range(rng).a1(), "type": rng.pick(&["linear", "growth", "autofill"]), "step": rng.int(-2, 3)}),
        "home.bold" => json!({"range": range(rng)}),
        "home.fillColor" => json!({"range": range(rng), "color": color(rng)}),
        "home.numberFormat" => json!({"range": range(rng), "format": rng.pick(&["Currency", "Percentage", "Short Date", "Text", "General"])}),
        "home.borders" => json!({"range": range(rng), "preset": rng.pick(&["all", "outside", "none", "thickBottom"])}),
        "home.insertRows" | "home.deleteRows" | "home.hideRows" | "home.unhideRows" => {
            let mut p = json!({"rows": rows_spec(rng)});
            if id == "home.insertRows" && rng.chance(20) {
                p["count"] = json!(rng.int(1, 4));
            }
            p
        }
        "home.insertColumns" | "home.deleteColumns" => json!({"cols": cols_spec(rng)}),
        "home.insertCells" => json!({"range": near_range(rng).a1(), "shift": rng.pick(&["right", "down", "row", "column"])}),
        "home.deleteCells" => json!({"range": near_range(rng).a1(), "shift": rng.pick(&["left", "up", "row", "column"])}),
        "home.insertSheet" => {
            let mut p = json!({});
            if rng.chance(40) {
                p["name"] = json!(new_name(rng));
            }
            if rng.chance(40) {
                p["before"] = json!(rng.below(wb.sheets.len() + 1));
            }
            p
        }
        "home.deleteSheet" | "sheet.activate" => json!({"sheet": sheet_param(rng, wb)}),
        "sheet.rename" => json!({"sheet": sheet_param(rng, wb), "name": new_name(rng)}),
        "sheet.move" => json!({"sheet": sheet_param(rng, wb), "to": rng.below(wb.sheets.len() + 1), "copy": rng.chance(35)}),
        "insert.table" => {
            let mut p = json!({"range": near_range(rng).a1(), "header": rng.chance(80)});
            if rng.chance(40) {
                p["name"] = json!(rng.pick(&["T1", "Sales", "Data", "Grades"]));
            }
            p
        }
        "table.totalRow" => json!({"table": table_param(rng, wb), "on": rng.chance(50)}),
        "table.resize" => json!({"table": table_param(rng, wb), "range": near_range(rng).a1()}),
        "table.convertToRange" => json!({"table": table_param(rng, wb)}),
        "insert.chart" => {
            json!({"range": near_range(rng).a1(), "type": rng.pick(&["column", "line", "pie", "scatter", "bar"]), "at": cell(rng).a1()})
        }
        "chart.delete" => {
            let ids = chart_ids(wb);
            match ids.get(rng.below(ids.len() + 1)) {
                Some(id) => json!({"chart": id}),
                None => json!({}),
            }
        }
        "data.sortAscending" => {
            let mut p = maybe_range(rng);
            if rng.chance(50) {
                p["column"] = json!(letter(rng.int(0, 9) as u32));
            }
            p
        }
        "data.sort" => json!({
            "range": near_range(rng).a1(),
            "header": rng.chance(50),
            "keys": [{"column": letter(rng.int(0, 9) as u32), "order": rng.pick(&["asc", "desc"])}],
            "orientation": if rng.chance(15) { "columns" } else { "rows" },
        }),
        "data.filter" => maybe_range(rng),
        "data.filterBy" => {
            let column = letter(rng.int(0, 9) as u32);
            match rng.below(4) {
                0 => json!({"column": column, "values": [literal(rng), literal(rng)]}),
                1 => json!({"column": column, "custom": {"op": ">", "value": rng.int(0, 60000).to_string()}}),
                2 => json!({"column": column, "top": {"count": rng.int(1, 5)}}),
                _ => json!({"column": column, "clear": true}),
            }
        }
        "data.removeDuplicates" => json!({"range": near_range(rng).a1(), "header": rng.chance(50)}),
        "formulas.defineName" => {
            let sheet = other_sheet(rng, wb);
            let r = near_range(rng);
            let abs = format!("${}${}:${}${}", letter(r.start.col), r.start.row + 1, letter(r.end.col), r.end.row + 1);
            json!({"name": rng.pick(&["Rate", "Totals", "my_list", "Q"]), "refersTo": format!("={sheet}!{abs}")})
        }
        "formulas.deleteName" => match wb.names.get(rng.below(wb.names.len() + 1)) {
            Some(n) => json!({"name": n.name}),
            None => json!({"name": "Nope"}),
        },
        "edit.replace" => json!({"what": rng.pick(&["North", "1", "A", "Sales"]), "with": rng.pick(&["South", "2", "B", ""]), "all": true}),
        _ => maybe_range(rng),
    }
}

/// Top-level keys of a params doc (`{range?, shift: right|down, count?}`): (key, optional, hint).
fn doc_keys(doc: &str) -> Vec<(String, bool, String)> {
    let Some(start) = doc.find('{') else { return vec![] };
    let mut parts = Vec::new();
    let (mut depth, mut in_str, mut cur) = (0i32, false, String::new());
    for ch in doc[start + 1..].chars() {
        match ch {
            '"' => {
                in_str = !in_str;
                cur.push(ch);
            }
            _ if in_str => cur.push(ch),
            '{' | '[' | '(' => {
                depth += 1;
                cur.push(ch);
            }
            '}' | ']' | ')' if depth == 0 => break,
            '}' | ']' | ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    parts.push(cur);
    let mut out = Vec::new();
    for part in parts {
        let part = part.trim();
        let key: String = part.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if key.is_empty() {
            continue;
        }
        let rest = &part[key.len()..];
        let optional = rest.starts_with('?');
        let hint = rest.trim_start_matches('?').trim_start();
        let hint = hint.strip_prefix(':').unwrap_or(hint).trim().to_string();
        out.push((key, optional, hint));
    }
    out
}

/// Bare-word alternatives in a hint (`asc|desc`, `"Good"|"Bad"`), if it is an enumeration.
fn alternatives(hint: &str) -> Option<Vec<String>> {
    if !hint.contains('|') {
        return None;
    }
    let alts: Vec<String> = hint
        .split('|')
        .map(|a| a.split(" (").next().unwrap_or(a).trim().trim_matches('"').trim_end_matches('…').to_string())
        .filter(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || " .".contains(c)))
        .collect();
    (!alts.is_empty()).then_some(alts)
}

/// Params from a command's params doc: values chosen by key name, then by the hint's shape.
fn doc_params(rng: &mut Rng, wb: &Workbook, doc: &str) -> Json {
    let mut p = serde_json::Map::new();
    for (key, optional, hint) in doc_keys(doc) {
        if optional && rng.chance(40) {
            continue;
        }
        if let Some(v) = doc_value(rng, wb, &key, &hint) {
            p.insert(key, v);
        }
    }
    Json::Object(p)
}

fn doc_value(rng: &mut Rng, wb: &Workbook, key: &str, hint: &str) -> Option<Json> {
    let lower = key.to_ascii_lowercase();
    let v = match lower.as_str() {
        "path" | "base64" | "password" | "html" => return None,
        "range" | "source" | "target" | "destination" | "criteria" | "copyto" | "printarea" | "location" | "resultcells" | "changing"
        | "rowinput" | "columninput" | "add" | "remove" | "apply" => json!(range(rng)),
        "refersto" => json!(format!("={}!$A$1:$B$4", other_sheet(rng, wb))),
        "cell" | "at" | "active" | "fromcell" | "set" | "reference" => json!(cell(rng).a1()),
        "rows" | "titlerows" if !hint.starts_with('[') => json!(rows_spec(rng)),
        "cols" | "titlecols" if !hint.starts_with('[') => json!(cols_spec(rng)),
        "sheet" => sheet_param(rng, wb),
        "input" | "formula" | "formula1" | "formula2" | "value" | "value2" | "with" => json!(input(rng, wb)),
        "table" => table_param(rng, wb),
        "chart" | "id" => {
            let ids = chart_ids(wb);
            json!(ids.get(rng.below(ids.len() + 1)).copied().unwrap_or(rng.int(0, 5) as u32))
        }
        "ids" => json!(chart_ids(wb)),
        "color" => json!(color(rng)),
        "values" if hint.starts_with("[[") => values_2d(rng, wb),
        "keys" => json!([{"column": letter(rng.int(0, 9) as u32), "order": rng.pick(&["asc", "desc"])}]),
        "column" | "groupby" => json!(letter(rng.int(0, 9) as u32)),
        "columns" | "sources" if !hint.contains('|') => json!([letter(rng.int(0, 9) as u32)]),
        "points" => json!([[10, 10], [40, 25], [70, 5]]),
        "commands" => json!([{"command": "cell.set", "params": {"cell": near_cell(rng).a1(), "input": input(rng, wb)}}]),
        "char" => json!("€"),
        "name" | "oldname" if alternatives(hint).is_none() => json!(new_name(rng)),
        "text" | "title" | "comment" | "author" | "word" | "header" | "footer" | "what" if alternatives(hint).is_none() => {
            json!(rng.pick(&["North", "note", "&CPage &P", "x"]))
        }
        _ => return Some(hint_value(rng, wb, hint)),
    };
    Some(v)
}

fn hint_value(rng: &mut Rng, wb: &Workbook, hint: &str) -> Json {
    if let Some(alts) = alternatives(hint) {
        return json!(rng.pick(&alts));
    }
    if hint.contains("bool") || hint == "true" || hint == "false" {
        return json!(rng.chance(50));
    }
    if let Some((a, b)) = hint.split_once("..")
        && let (Ok(a), Ok(b)) = (a.trim().parse::<i64>(), b.trim().split(' ').next().unwrap_or("").parse::<i64>())
    {
        return json!(rng.int(a, b));
    }
    if let Ok(n) = hint.split(' ').next().unwrap_or("").parse::<f64>() {
        return json!((n * rng.int(0, 3) as f64).round());
    }
    if hint.starts_with('[') {
        let heads: Vec<String> = tables(wb).iter().flat_map(|(_, t)| t.columns.iter().map(|c| c.name.clone())).collect();
        return match heads.get(rng.below(heads.len() + 1)) {
            Some(h) => json!([h]),
            None => json!([]),
        };
    }
    if let Some(ex) = hint.strip_prefix('"').and_then(|h| h.split('"').next())
        && rng.chance(60)
    {
        return json!(ex);
    }
    match rng.below(4) {
        0 => json!(rng.chance(50)),
        1 => json!(rng.int(0, 5)),
        2 => json!(near_range(rng).a1()),
        _ => json!(literal(rng)),
    }
}

#[test]
fn params_docs_parse_into_keys() {
    let keys = doc_keys("{range?, shift: right|down|row|column, input: \"text, number or =formula\"}");
    let names: Vec<_> = keys.iter().map(|(k, o, _)| (k.as_str(), *o)).collect();
    assert_eq!(names, [("range", true), ("shift", false), ("input", false)]);
    assert_eq!(alternatives(&keys[1].2), Some(vec!["right".into(), "down".into(), "row".into(), "column".into()]));
    // Every registered command's doc parses, and the fuzzer's exclusions name real commands.
    for spec in command_specs() {
        let _ = doc_keys(spec.params);
    }
    for (id, _) in EXCLUDED {
        assert!(find_command(id).is_some(), "EXCLUDED names unknown command `{id}`");
    }
    for (id, _) in CURATED {
        assert!(find_command(id).is_some(), "CURATED names unknown command `{id}`");
    }
}

/// Seed 177: switching a sheet to Page Break Preview isn't an undo step (as in Excel), so undoing
/// everything leaves the view mode alone and the undo check must not count it as an edit.
#[test]
fn view_mode_switch_is_not_an_undo_difference() {
    for id in ["view.pageBreakPreview", "view.pageLayout"] {
        let failure = replay("sales", &[(id.to_string(), json!({"sheet": 0}))]);
        assert!(failure.is_none(), "`{id}`: {failure:?}");
    }
}

/// Review of #2332: once the undo history has dropped its oldest steps, an `edit.undo` in the
/// sequence can use up everything it kept before the episode ends. Undoing everything then has
/// nothing left to do, and the workbook is the state before the oldest step kept, not the
/// episode's start.
#[test]
fn a_trimmed_history_undone_in_the_sequence_stops_at_its_oldest_step() {
    let mut h = Harness::new("sales");
    // Past the engine's 100-step undo limit, so it drops the oldest 5 steps.
    for i in 0..105 {
        h.s.execute("cell.set", json!({"cell": format!("Z{}", i + 1), "input": i.to_string()})).expect("cell.set");
        h.track_history();
    }
    assert_eq!(h.s.doc().expect("document").undo.len(), 100, "the history dropped its oldest steps");
    if let Err(f) = h.step(0, "edit.undo", &json!({"steps": 100})) {
        panic!("{f:?}");
    }
    assert!(h.s.doc().expect("document").undo.is_empty());
    if let Err(f) = h.end_episode(0) {
        panic!("{f:?}");
    }
}

/// Review of #2332: the redo history is bounded too, so undoing `n` steps may leave fewer than
/// `n` to redo, and redo then stops at the oldest entry it kept rather than at the final workbook.
#[test]
fn redo_stops_at_the_oldest_step_its_history_kept() {
    let h = Harness::new("sales");
    let selection = h.s.doc().expect("document").selection.clone();
    let entry = |wb: &Arc<Workbook>| HistoryEntry { label: String::new(), wb: wb.clone(), selection: selection.clone(), retained_bytes: 0 };
    let end = Arc::new((*h.wb()).clone());
    let kept = Arc::new((*h.wb()).clone());
    // Both steps kept: the first one undone went onto the redo history with the final workbook.
    assert!(Arc::ptr_eq(&redo_target(&end, &[entry(&end), entry(&kept)], 2), &end));
    // Its entry dropped: redo stops at the oldest entry left.
    assert!(Arc::ptr_eq(&redo_target(&end, &[entry(&kept)], 2), &kept));
    // Nothing undone, so nothing to redo, whatever an earlier `edit.undo` left there.
    assert!(Arc::ptr_eq(&redo_target(&end, &[entry(&kept)], 0), &end));
}
