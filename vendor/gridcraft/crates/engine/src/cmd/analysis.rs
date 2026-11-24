//! Analysis ToolPak (Data › Analysis › Data Analysis): statistical tools that compute a report
//! from input ranges and write it as a static table (values, not formulas) to an output range or
//! a new worksheet.
//!
//! Every tool is one `tool` value of `data.analysisToolPak`. Distribution values (t, F) come
//! from the worksheet functions (`T.DIST.2T`, `F.DIST.RT`, `T.INV.2T`…), so the reports agree
//! with the formulas a user would write. Layouts follow Microsoft's public description of the
//! tools (row labels and section titles); exact spacing is GridCraft's reading of it.

use std::collections::{BTreeMap, HashMap};

use gridcraft_core::{CellError, CellRef, DateSystem, MAX_COLS, MAX_ROWS, RangeRef, Value};
use gridcraft_model::{Cell, NumFmt, Sheet};
use serde_json::{Value as Json, json};

use super::whatif::parse_range;
use super::{CommandSpec, EngineError, Result, Session, bad, cmd, edit, has_doc, str_param};

const CMD: &str = "data.analysisToolPak";
/// Most input cells a tool reads.
const MAX_INPUT_CELLS: u64 = 1_000_000;
/// Most cells a report may write.
const MAX_OUTPUT_CELLS: u64 = 4_000_000;
/// Most variables in a correlation or covariance matrix (work grows with its square).
const MAX_MATRIX_VARS: usize = 256;
/// Excel's limit on regression X columns.
const MAX_REGRESSORS: usize = 16;
/// Most histogram bins.
const MAX_BINS: usize = 100_000;
/// Excel's limit on Fourier Analysis input values.
const MAX_FOURIER: usize = 4096;
/// Most numbers Random Number Generation or Sampling writes in one run.
const MAX_RANDOM: usize = 1_000_000;
/// Most trials of a binomial draw, and the farthest Poisson outcome considered.
const MAX_OUTCOMES: f64 = 1_000_000.0;
/// Largest seed: seeds are whole numbers that a JSON number holds exactly.
const MAX_SEED: u64 = (1 << 53) - 1;

/// Part of the error a programmatic call gets when the output range already holds data; the
/// dialog recognises it and asks before running again with `overwrite: true`.
pub const OVERWRITE_ERROR: &str = "already holds data";

/// (id, label) of every tool, in the order of Excel's Analysis Tools list.
pub const TOOLS: &[(&str, &str)] = &[
    ("anovaSingle", "Anova: Single Factor"),
    ("anovaTwoFactorWith", "Anova: Two-Factor With Replication"),
    ("anovaTwoFactorWithout", "Anova: Two-Factor Without Replication"),
    ("correlation", "Correlation"),
    ("covariance", "Covariance"),
    ("descriptive", "Descriptive Statistics"),
    ("exponentialSmoothing", "Exponential Smoothing"),
    ("fTest", "F-Test Two-Sample for Variances"),
    ("fourierAnalysis", "Fourier Analysis"),
    ("histogram", "Histogram"),
    ("movingAverage", "Moving Average"),
    ("randomNumberGeneration", "Random Number Generation"),
    ("rankPercentile", "Rank and Percentile"),
    ("regression", "Regression"),
    ("sampling", "Sampling"),
    ("tTestPaired", "t-Test: Paired Two Sample for Means"),
    ("tTestEqual", "t-Test: Two-Sample Assuming Equal Variances"),
    ("tTestUnequal", "t-Test: Two-Sample Assuming Unequal Variances"),
    ("zTest", "z-Test: Two Sample for Means"),
];

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "data.analysisToolPak",
        "Data Analysis…",
        ["Data", "Analysis"],
        None,
        "{tool: anovaSingle|anovaTwoFactorWith|anovaTwoFactorWithout|correlation|covariance|descriptive|exponentialSmoothing|fTest|fourierAnalysis|histogram|movingAverage|randomNumberGeneration|rankPercentile|regression|sampling|tTestPaired|tTestEqual|tTestUnequal|zTest (or the tool's label), output?: \"E1\" (top-left of the report; default a new sheet), newSheet?: name, outputTo?: range|newSheet (picks between the two), overwrite?: bool (else non-empty output cells are refused), labels?: bool (first row/column holds names), groupedBy?: columns|rows, …tool params} — range tools take `range`; descriptive: summary? (default true), confidence?: 95, kthLargest?, kthSmallest?; histogram: bins?: range (default evenly spaced), pareto?, cumulative?; regression: inputY, inputX, constantZero?, confidence?: 95, residuals?, standardizedResiduals?, normalProbability?; movingAverage: interval? (3), standardErrors?; exponentialSmoothing: damping? (0.3), standardErrors?; t-tests and fTest: range1, range2, alpha? (0.05), hypothesizedDifference? (0); anovaSingle: alpha?; anovaTwoFactorWithout: range, labels? (the first row holds column names and the first column row names), alpha?; anovaTwoFactorWith: range (first row and column hold labels), rowsPerSample, alpha?; zTest: range1, range2, variance1, variance2 (known variances), alpha?, hypothesizedDifference?; fourierAnalysis: range (one row or column of 2^k numbers or complex text, at most 4096), inverse?; randomNumberGeneration: variables? (1), count (per variable; not for patterned), distribution: uniform{from, to} | normal{mean, standardDeviation} | bernoulli{p} | binomial{p, trials} | poisson{lambda} | patterned{from, to, step, repeatEach, repeatSequence} | discrete{valueRange: two columns of value and probability}, seed? (whole number, not for patterned; the same seed repeats the numbers; the result reports the seed used), at most 1,000,000 numbers; sampling: range, method?: periodic|random, period (periodic), samples (random, with replacement), seed?. Writes static values. No `tool` → {tools} (the menu opens the dialog).",
        has_doc,
        run
    )]
}

// ---------------------------------------------------------------- parameters

fn num(p: &Json, key: &str) -> Option<f64> {
    match p.get(key)? {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().trim_end_matches('%').trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|v| v.is_finite())
}

fn flag(p: &Json, key: &str) -> bool {
    match p.get(key) {
        Some(Json::Bool(b)) => *b,
        Some(Json::String(s)) => s.eq_ignore_ascii_case("true") || s == "1",
        Some(Json::Number(n)) => n.as_f64().is_some_and(|v| v != 0.0),
        _ => false,
    }
}

fn text<'a>(p: &'a Json, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| str_param(p, k)).map(str::trim).filter(|t| !t.is_empty())
}

fn norm(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase()
}

/// A tool id from its id or its label (case and punctuation ignored).
fn tool_id(t: &str) -> Option<&'static str> {
    let n = norm(t);
    TOOLS.iter().find(|(id, label)| norm(id) == n || norm(label) == n).map(|(id, _)| *id)
}

/// Confidence level as a percentage in (0, 100): `95`, `"95%"` or `0.95`.
fn confidence(p: &Json, key: &str) -> Result<Option<f64>> {
    match num(p, key) {
        None => Ok(None),
        Some(c) => {
            let c = if c > 0.0 && c < 1.0 { c * 100.0 } else { c };
            if c > 0.0 && c < 100.0 { Ok(Some(c)) } else { Err(bad(CMD, "the confidence level must be between 0 and 100")) }
        }
    }
}

fn alpha(p: &Json) -> Result<f64> {
    match opt_num(p, "alpha")? {
        None => Ok(0.05),
        Some(a) if a > 0.0 && a < 1.0 => Ok(a),
        Some(_) => Err(bad(CMD, "alpha must be between 0 and 1")),
    }
}

// ---------------------------------------------------------------- input

struct Var {
    name: String,
    vals: Vec<Value>,
}

fn non_numeric() -> EngineError {
    EngineError::Other("Input range contains non-numeric data.".into())
}

/// The cells of a range parameter, after checking its size.
fn input_range(s: &Session, p: &Json, keys: &[&str]) -> Result<(usize, RangeRef)> {
    let t = text(p, keys).ok_or_else(|| bad(CMD, format!("missing `{}` (the input range)", keys.first().copied().unwrap_or("range"))))?;
    let (si, r) = parse_range(&s.doc()?.wb, t, CMD)?;
    if r.count() > MAX_INPUT_CELLS {
        return Err(bad(CMD, format!("the input range is too large ({MAX_INPUT_CELLS} cells at most)")));
    }
    Ok((si, r))
}

/// Splits a range into variables (columns, or rows when `by_rows`); with `labels` the first
/// cell of each names it, else `default(i)` does.
fn variables(s: &Session, si: usize, r: RangeRef, by_rows: bool, labels: bool, default: &dyn Fn(usize) -> String) -> Result<Vec<Var>> {
    let outer = if by_rows { r.height() } else { r.width() };
    let mut out = Vec::with_capacity(outer as usize);
    for i in 0..outer {
        out.push(variable(s, si, r, by_rows, labels, default, i)?);
    }
    Ok(out)
}

/// Variable `i` of [`variables`], read alone so a caller that may stop early needn't build them all.
fn variable(s: &Session, si: usize, r: RangeRef, by_rows: bool, labels: bool, default: &dyn Fn(usize) -> String, i: u32) -> Result<Var> {
    let d = s.doc()?;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let inner = if by_rows { r.width() } else { r.height() };
    let cell = |j: u32| if by_rows { CellRef::new(r.start.row + i, r.start.col + j) } else { CellRef::new(r.start.row + j, r.start.col + i) };
    let mut vals: Vec<Value> = (0..inner).map(|j| sh.value(cell(j))).collect();
    let mut name = default(i as usize);
    if labels && !vals.is_empty() {
        let first = vals.remove(0);
        let t = first.display();
        if !t.trim().is_empty() {
            name = t;
        }
    }
    Ok(Var { name, vals })
}

/// The numbers of a variable; blanks are skipped when `skip_blank`, anything else that isn't a
/// number is an error.
fn numbers(v: &[Value], skip_blank: bool) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(v.len());
    for x in v {
        match x {
            Value::Number(n) => out.push(*n),
            Value::Empty if skip_blank => {}
            _ => return Err(non_numeric()),
        }
    }
    Ok(out)
}

fn by_rows(p: &Json) -> bool {
    str_param(p, "groupedBy").is_some_and(|g| g.to_ascii_lowercase().starts_with("row"))
}

/// One variable from a single row or column (`range1`/`range2` of the two-sample tests).
fn single(s: &Session, p: &Json, key: &str, fallback: &str) -> Result<Var> {
    let (si, r) = input_range(s, p, &[key])?;
    if r.width() > 1 && r.height() > 1 {
        return Err(EngineError::Other("Input range must be a single row or column.".into()));
    }
    let rows = r.width() > 1;
    let mut v = variables(s, si, r, rows, flag(p, "labels"), &|_| fallback.to_string())?;
    v.pop().ok_or_else(|| bad(CMD, "empty input range"))
}

// ---------------------------------------------------------------- statistics

fn mean(x: &[f64]) -> f64 {
    if x.is_empty() { f64::NAN } else { x.iter().sum::<f64>() / x.len() as f64 }
}

/// Sum of squared deviations from the mean.
fn ss(x: &[f64]) -> f64 {
    let m = mean(x);
    x.iter().map(|v| (v - m) * (v - m)).sum()
}

/// Sample variance (n − 1).
fn var_s(x: &[f64]) -> f64 {
    if x.len() < 2 { f64::NAN } else { ss(x) / (x.len() - 1) as f64 }
}

fn div0() -> Value {
    Value::Error(CellError::Div0)
}

/// `a / b`, `#DIV/0!` when `b` is zero or the inputs aren't finite.
fn ratio(a: f64, b: f64) -> Value {
    if b == 0.0 || !a.is_finite() || !b.is_finite() { div0() } else { Value::number(a / b) }
}

fn n_or_div0(x: f64) -> Value {
    if x.is_finite() { Value::number(x) } else { div0() }
}

/// Host services for calling worksheet functions directly.
struct FnHost;

impl gridcraft_functions::Ctx for FnHost {
    fn date_system(&self) -> DateSystem {
        DateSystem::D1900
    }
    fn now_serial(&self) -> f64 {
        0.0
    }
    fn random(&mut self) -> f64 {
        0.5
    }
}

/// Calls a worksheet function on numeric arguments (and `cumulative` flags as booleans).
fn call(name: &str, args: &[Value]) -> Value {
    let Some(spec) = gridcraft_functions::lookup(name) else { return Value::Error(CellError::Name) };
    if args.iter().any(|a| a.as_f64().is_some_and(|v| !v.is_finite())) {
        return Value::Error(CellError::Num);
    }
    let a: Vec<gridcraft_functions::Arg> = args.iter().cloned().map(gridcraft_functions::Arg::val).collect();
    gridcraft_functions::call(spec, &a, &mut FnHost)
}

fn callf(name: &str, args: &[f64]) -> Value {
    call(name, &args.iter().map(|v| Value::Number(*v)).collect::<Vec<_>>())
}

fn f64_of(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

// ---------------------------------------------------------------- report grid

/// A report: values by (row, col) relative to its top-left corner.
#[derive(Default)]
struct Grid {
    cells: BTreeMap<(u32, u32), Value>,
    /// Cells shown as percentages.
    pct: Vec<(u32, u32)>,
    /// Rows and columns spanned by `cells`.
    rows: u32,
    cols: u32,
    /// A cell was dropped because the report outgrew [`MAX_OUTPUT_CELLS`]: the report is refused.
    too_large: bool,
}

impl Grid {
    /// Stores a value. Past [`MAX_OUTPUT_CELLS`] (the most a report writes) nothing more is kept,
    /// so a builder that would produce millions of entries stops allocating them, and
    /// [`write_report`] refuses the report.
    fn put(&mut self, r: u32, c: u32, v: Value) {
        if !v.is_empty() {
            let (rows, cols) = (self.rows.max(r.saturating_add(1)), self.cols.max(c.saturating_add(1)));
            if self.too_large || u64::from(rows) * u64::from(cols) > MAX_OUTPUT_CELLS {
                self.too_large = true;
                return;
            }
            (self.rows, self.cols) = (rows, cols);
            self.cells.insert((r, c), v);
        }
    }
    fn txt(&mut self, r: u32, c: u32, t: impl AsRef<str>) {
        self.put(r, c, Value::text(t.as_ref()));
    }
    fn num(&mut self, r: u32, c: u32, x: f64) {
        self.put(r, c, Value::number(x));
    }
    /// Writes a row of labels starting at column `c`.
    fn labels(&mut self, r: u32, c: u32, items: &[&str]) {
        for (k, t) in items.iter().enumerate() {
            if !t.is_empty() {
                self.txt(r, c + k as u32, t);
            }
        }
    }
    /// (rows, cols) spanned.
    fn extent(&self) -> (u32, u32) {
        (self.rows, self.cols)
    }
}

fn idx(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

// ---------------------------------------------------------------- tools

fn descriptive(s: &Session, p: &Json) -> Result<Grid> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let (by_rows, labels) = (by_rows(p), flag(p, "labels"));
    let summary = p.get("summary").map(|_| flag(p, "summary")).unwrap_or(true);
    let conf = confidence(p, "confidence")?;
    let kl = num(p, "kthLargest").filter(|k| *k >= 1.0).map(|k| k as usize);
    let ks = num(p, "kthSmallest").filter(|k| *k >= 1.0).map(|k| k as usize);
    if !summary && conf.is_none() && kl.is_none() && ks.is_none() {
        return Err(bad(CMD, "choose at least one of summary, confidence, kthLargest or kthSmallest"));
    }
    let mut g = Grid::default();
    // Variables are read one at a time: a million-row range grouped by rows is a million variables.
    for i in 0..(if by_rows { r.height() } else { r.width() }) {
        // The report is refused once the grid is too large: the rest would only build discarded labels.
        if g.too_large {
            break;
        }
        let v = variable(s, si, r, by_rows, labels, &|i| format!("Column{}", i + 1), i)?;
        let i = i as usize;
        let x = numbers(&v.vals, true)?;
        let c = idx(i).saturating_mul(2);
        g.txt(0, c, &v.name);
        let mut row = 2u32;
        let mut line = |g: &mut Grid, label: &str, val: Value| {
            g.txt(row, c, label);
            g.put(row, c + 1, val);
            row += 1;
        };
        let n = x.len() as f64;
        let mut sorted = x.clone();
        sorted.sort_by(f64::total_cmp);
        if summary {
            let m = mean(&x);
            let sd = var_s(&x).sqrt();
            line(&mut g, "Mean", n_or_div0(m));
            line(&mut g, "Standard Error", if x.len() < 2 { div0() } else { n_or_div0(sd / n.sqrt()) });
            line(&mut g, "Median", median(&sorted));
            line(&mut g, "Mode", mode(&x));
            line(&mut g, "Standard Deviation", n_or_div0(sd));
            line(&mut g, "Sample Variance", n_or_div0(var_s(&x)));
            line(&mut g, "Kurtosis", kurtosis(&x));
            line(&mut g, "Skewness", skewness(&x));
            let (lo, hi) = (sorted.first().copied(), sorted.last().copied());
            line(&mut g, "Range", lo.zip(hi).map(|(a, b)| Value::number(b - a)).unwrap_or(Value::Number(0.0)));
            line(&mut g, "Minimum", lo.map(Value::number).unwrap_or(Value::Number(0.0)));
            line(&mut g, "Maximum", hi.map(Value::number).unwrap_or(Value::Number(0.0)));
            line(&mut g, "Sum", Value::number(x.iter().sum()));
            line(&mut g, "Count", Value::number(n));
        }
        if let Some(k) = kl {
            let v = if k <= sorted.len() { sorted.get(sorted.len() - k).copied().map(Value::number) } else { None };
            line(&mut g, &format!("Largest({k})"), v.unwrap_or(Value::Error(CellError::Num)));
        }
        if let Some(k) = ks {
            let v = sorted.get(k - 1).copied().map(Value::number);
            line(&mut g, &format!("Smallest({k})"), v.unwrap_or(Value::Error(CellError::Num)));
        }
        if let Some(cl) = conf {
            let sd = var_s(&x).sqrt();
            let val = if x.len() < 2 {
                div0()
            } else {
                let t = f64_of(&callf("T.INV.2T", &[1.0 - cl / 100.0, n - 1.0]));
                n_or_div0(t * sd / n.sqrt())
            };
            line(&mut g, &format!("Confidence Level({cl:.1}%)"), val);
        }
    }
    Ok(g)
}

fn median(sorted: &[f64]) -> Value {
    let n = sorted.len();
    if n == 0 {
        return Value::Error(CellError::Num);
    }
    let v = if n % 2 == 1 { sorted.get(n / 2).copied() } else { sorted.get(n / 2 - 1).zip(sorted.get(n / 2)).map(|(a, b)| (a + b) / 2.0) };
    v.map(Value::number).unwrap_or(Value::Error(CellError::Num))
}

/// The most frequent value (the first one to reach the top count), `#N/A` when no value repeats.
fn mode(x: &[f64]) -> Value {
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for v in x {
        *counts.entry(v.to_bits()).or_default() += 1;
    }
    let top = counts.values().copied().max().unwrap_or(0);
    if top < 2 {
        return Value::Error(CellError::NA);
    }
    x.iter().find(|v| counts.get(&v.to_bits()).copied() == Some(top)).map(|v| Value::number(*v)).unwrap_or(Value::Error(CellError::NA))
}

/// Sample excess kurtosis (Excel's KURT).
fn kurtosis(x: &[f64]) -> Value {
    let n = x.len() as f64;
    let sd = var_s(x).sqrt();
    if x.len() < 4 || sd.is_nan() || sd <= 0.0 {
        return div0();
    }
    let m = mean(x);
    let s4: f64 = x.iter().map(|v| ((v - m) / sd).powi(4)).sum();
    Value::number(n * (n + 1.0) / ((n - 1.0) * (n - 2.0) * (n - 3.0)) * s4 - 3.0 * (n - 1.0) * (n - 1.0) / ((n - 2.0) * (n - 3.0)))
}

/// Sample skewness (Excel's SKEW).
fn skewness(x: &[f64]) -> Value {
    let n = x.len() as f64;
    let sd = var_s(x).sqrt();
    if x.len() < 3 || sd.is_nan() || sd <= 0.0 {
        return div0();
    }
    let m = mean(x);
    let s3: f64 = x.iter().map(|v| ((v - m) / sd).powi(3)).sum();
    Value::number(n / ((n - 1.0) * (n - 2.0)) * s3)
}

fn histogram(s: &Session, p: &Json) -> Result<Grid> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let labels = flag(p, "labels");
    let d = s.doc()?;
    let sh = d.wb.sheet(si).ok_or(EngineError::NoDocument)?;
    let mut cells: Vec<Value> = r.iter().map(|c| sh.value(c)).collect();
    if labels && !cells.is_empty() {
        cells.remove(0);
    }
    let data = numbers(&cells, true)?;
    let mut bins: Vec<f64> = match text(p, &["bins", "binRange"]) {
        Some(_) => {
            let (bsi, br) = input_range(s, p, &["bins", "binRange"])?;
            let bsh = d.wb.sheet(bsi).ok_or(EngineError::NoDocument)?;
            let mut b: Vec<Value> = br.iter().map(|c| bsh.value(c)).collect();
            if labels && !b.is_empty() {
                b.remove(0);
            }
            numbers(&b, true)?
        }
        None => {
            // Evenly spaced bins from the minimum to the maximum, √n intervals.
            let lo = data.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = data.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if !lo.is_finite() || !hi.is_finite() {
                return Err(EngineError::Other("The input range holds no numbers.".into()));
            }
            let k = ((data.len() as f64).sqrt().floor() as usize).clamp(1, 1000);
            if hi == lo { vec![lo] } else { (0..=k).map(|i| lo + (hi - lo) * i as f64 / k as f64).collect() }
        }
    };
    if bins.is_empty() {
        return Err(EngineError::Other("The bin range holds no numbers.".into()));
    }
    if bins.len() > MAX_BINS {
        return Err(bad(CMD, format!("too many bins ({MAX_BINS} at most)")));
    }
    bins.sort_by(f64::total_cmp);
    let mut freq = vec![0u64; bins.len() + 1];
    for x in &data {
        let i = bins.partition_point(|b| b < x);
        if let Some(f) = freq.get_mut(i) {
            *f += 1;
        }
    }
    let rows: Vec<(Value, u64)> = bins.iter().map(|b| Value::number(*b)).chain(std::iter::once(Value::text("More"))).zip(freq).collect();
    let total = data.len() as f64;
    let cumulative = flag(p, "cumulative");
    let mut g = Grid::default();
    let write = |g: &mut Grid, c0: u32, rows: &[(Value, u64)]| {
        g.labels(0, c0, &["Bin", "Frequency"]);
        if cumulative {
            g.txt(0, c0 + 2, "Cumulative %");
        }
        let mut acc = 0u64;
        for (i, (b, f)) in rows.iter().enumerate() {
            let r = idx(i) + 1;
            g.put(r, c0, b.clone());
            g.num(r, c0 + 1, *f as f64);
            if cumulative {
                acc += f;
                g.put(r, c0 + 2, ratio(acc as f64, total));
                g.pct.push((r, c0 + 2));
            }
        }
    };
    write(&mut g, 0, &rows);
    if flag(p, "pareto") {
        let mut sorted = rows.clone();
        sorted.sort_by_key(|r| std::cmp::Reverse(r.1));
        write(&mut g, if cumulative { 3 } else { 2 }, &sorted);
    }
    Ok(g)
}

/// Variables of a range whose columns (rows) must all be numbers, for the matrix tools.
fn matrix_vars(s: &Session, p: &Json) -> Result<Vec<(String, Vec<f64>)>> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let vars = variables(s, si, r, by_rows(p), flag(p, "labels"), &|i| format!("Column {}", i + 1))?;
    if vars.len() > MAX_MATRIX_VARS {
        return Err(bad(CMD, format!("too many variables ({MAX_MATRIX_VARS} at most)")));
    }
    vars.into_iter().map(|v| Ok((v.name, numbers(&v.vals, false)?))).collect()
}

fn matrix(s: &Session, p: &Json, f: impl Fn(&[f64], &[f64]) -> Value, diag: impl Fn(&[f64]) -> Value) -> Result<Grid> {
    let vars = matrix_vars(s, p)?;
    let mut g = Grid::default();
    for (i, (name, x)) in vars.iter().enumerate() {
        let k = idx(i) + 1;
        g.txt(0, k, name);
        g.txt(k, 0, name);
        for (j, (_, y)) in vars.iter().enumerate().take(i + 1) {
            g.put(k, idx(j) + 1, if i == j { diag(x) } else { f(x, y) });
        }
    }
    Ok(g)
}

/// Sum of cross products of deviations.
fn sxy(x: &[f64], y: &[f64]) -> f64 {
    let (mx, my) = (mean(x), mean(y));
    x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum()
}

fn correlation(x: &[f64], y: &[f64]) -> Value {
    let d = (ss(x) * ss(y)).sqrt();
    if x.is_empty() || d.is_nan() || d <= 0.0 { div0() } else { Value::number(sxy(x, y) / d) }
}

fn covariance_p(x: &[f64], y: &[f64]) -> Value {
    if x.is_empty() { div0() } else { Value::number(sxy(x, y) / x.len() as f64) }
}

// ---------------------------------------------------------------- regression

/// Inverts a small symmetric matrix by Gauss–Jordan elimination with partial pivoting; `None`
/// when it is singular (collinear inputs).
fn invert(mut a: Vec<Vec<f64>>) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let scale = (0..n).filter_map(|i| a.get(i).and_then(|r| r.get(i))).fold(0.0f64, |m, v| m.max(v.abs()));
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    for col in 0..n {
        let piv = (col..n).max_by(|&x, &y| {
            let ax = a.get(x).and_then(|r| r.get(col)).map_or(0.0, |v| v.abs());
            let ay = a.get(y).and_then(|r| r.get(col)).map_or(0.0, |v| v.abs());
            ax.total_cmp(&ay)
        })?;
        let pv = *a.get(piv)?.get(col)?;
        if pv.is_nan() || pv.abs() <= 1e-12 * scale.max(f64::MIN_POSITIVE) {
            return None;
        }
        a.swap(col, piv);
        inv.swap(col, piv);
        for j in 0..n {
            *a.get_mut(col)?.get_mut(j)? /= pv;
            *inv.get_mut(col)?.get_mut(j)? /= pv;
        }
        let prow = a.get(col)?.clone();
        let pinv = inv.get(col)?.clone();
        for i in 0..n {
            if i == col {
                continue;
            }
            let f = *a.get(i)?.get(col)?;
            if f == 0.0 {
                continue;
            }
            for j in 0..n {
                *a.get_mut(i)?.get_mut(j)? -= f * prow.get(j)?;
                *inv.get_mut(i)?.get_mut(j)? -= f * pinv.get(j)?;
            }
        }
    }
    Some(inv)
}

struct Fit {
    /// Intercept first, then one coefficient per X column.
    coef: Vec<f64>,
    /// Standard errors in the same order (NaN for a forced-zero intercept).
    se: Vec<f64>,
    predicted: Vec<f64>,
    sse: f64,
    ssr: f64,
    dfr: f64,
    dfe: f64,
    /// Total degrees of freedom (n − 1, or n through the origin).
    dft: f64,
}

/// Ordinary least squares of `y` on the columns `xs`, with an intercept unless `zero`.
fn ols(y: &[f64], xs: &[Vec<f64>], zero: bool) -> Option<Fit> {
    let n = y.len();
    let k = xs.len();
    let nf = n as f64;
    let (ym, xm): (f64, Vec<f64>) = if zero { (0.0, vec![0.0; k]) } else { (mean(y), xs.iter().map(|x| mean(x)).collect()) };
    let dev = |j: usize, i: usize| xs.get(j).and_then(|x| x.get(i)).copied().unwrap_or(0.0) - xm.get(j).copied().unwrap_or(0.0);
    let mut sxx = vec![vec![0.0; k]; k];
    let mut sxy = vec![0.0; k];
    for i in 0..n {
        let yi = y.get(i).copied().unwrap_or(0.0) - ym;
        for a in 0..k {
            let da = dev(a, i);
            if let Some(v) = sxy.get_mut(a) {
                *v += da * yi;
            }
            for b in 0..k {
                if let Some(v) = sxx.get_mut(a).and_then(|r| r.get_mut(b)) {
                    *v += da * dev(b, i);
                }
            }
        }
    }
    let inv = invert(sxx)?;
    let b: Vec<f64> = (0..k)
        .map(|a| (0..k).map(|c| inv.get(a).and_then(|r| r.get(c)).copied().unwrap_or(0.0) * sxy.get(c).copied().unwrap_or(0.0)).sum())
        .collect();
    let b0 = if zero { 0.0 } else { ym - b.iter().zip(&xm).map(|(bj, mj)| bj * mj).sum::<f64>() };
    let predicted: Vec<f64> = (0..n)
        .map(|i| b0 + (0..k).map(|j| b.get(j).copied().unwrap_or(0.0) * xs.get(j).and_then(|x| x.get(i)).copied().unwrap_or(0.0)).sum::<f64>())
        .collect();
    let sse: f64 = y.iter().zip(&predicted).map(|(a, p)| (a - p) * (a - p)).sum();
    let ssr: f64 = predicted.iter().map(|p| (p - ym) * (p - ym)).sum();
    let dfr = k as f64;
    let dfe = if zero { nf - dfr } else { nf - dfr - 1.0 };
    let dft = if zero { nf } else { nf - 1.0 };
    let s2 = sse / dfe;
    let mut se = Vec::with_capacity(k + 1);
    if zero {
        se.push(f64::NAN);
    } else {
        let q: f64 = (0..k)
            .map(|a| {
                (0..k)
                    .map(|c| {
                        xm.get(a).copied().unwrap_or(0.0)
                            * inv.get(a).and_then(|r| r.get(c)).copied().unwrap_or(0.0)
                            * xm.get(c).copied().unwrap_or(0.0)
                    })
                    .sum::<f64>()
            })
            .sum();
        se.push((s2 * (1.0 / nf + q)).sqrt());
    }
    for a in 0..k {
        se.push((s2 * inv.get(a).and_then(|r| r.get(a)).copied().unwrap_or(f64::NAN)).sqrt());
    }
    let mut coef = vec![b0];
    coef.extend(b);
    Some(Fit { coef, se, predicted, sse, ssr, dfr, dfe, dft })
}

fn regression(s: &Session, p: &Json) -> Result<Grid> {
    let labels = flag(p, "labels");
    let (ysi, yr) = input_range(s, p, &["inputY", "y"])?;
    let (xsi, xr) = input_range(s, p, &["inputX", "x"])?;
    if yr.width() != 1 {
        return Err(EngineError::Other("Input Y range must be a single column.".into()));
    }
    if xr.height() != yr.height() {
        return Err(EngineError::Other("Input Y range and input X range must have the same number of rows.".into()));
    }
    if xr.width() as usize > MAX_REGRESSORS {
        return Err(EngineError::Other(format!("Input X range must have {MAX_REGRESSORS} columns or fewer.")));
    }
    let yv = variables(s, ysi, yr, false, labels, &|_| "Y".into())?;
    let xv = variables(s, xsi, xr, false, labels, &|i| format!("X Variable {}", i + 1))?;
    let yvar = yv.first().ok_or_else(|| bad(CMD, "empty input Y range"))?;
    let y = numbers(&yvar.vals, false)?;
    let xs: Vec<Vec<f64>> = xv.iter().map(|v| numbers(&v.vals, false)).collect::<Result<_>>()?;
    let zero = flag(p, "constantZero");
    let k = xs.len();
    let n = y.len();
    let params = if zero { k } else { k + 1 };
    if n <= params {
        return Err(EngineError::Other("Too few observations for this many X variables.".into()));
    }
    let fit = ols(&y, &xs, zero).ok_or_else(|| EngineError::Other("The X variables are collinear; the regression can't be computed.".into()))?;
    let conf = confidence(p, "confidence")?.unwrap_or(95.0);
    let sst = fit.ssr + fit.sse;
    let r2 = if sst > 0.0 { fit.ssr / sst } else { f64::NAN };
    let mut g = Grid::default();
    g.txt(0, 0, "SUMMARY OUTPUT");
    g.txt(2, 0, "Regression Statistics");
    let stats: [(&str, Value); 5] = [
        ("Multiple R", n_or_div0(r2.sqrt())),
        ("R Square", n_or_div0(r2)),
        ("Adjusted R Square", n_or_div0(1.0 - (fit.sse / fit.dfe) / (sst / fit.dft))),
        ("Standard Error", n_or_div0((fit.sse / fit.dfe).sqrt())),
        ("Observations", Value::number(n as f64)),
    ];
    for (i, (label, v)) in stats.into_iter().enumerate() {
        g.txt(3 + idx(i), 0, label);
        g.put(3 + idx(i), 1, v);
    }
    g.txt(9, 0, "ANOVA");
    g.labels(10, 1, &["df", "SS", "MS", "F", "Significance F"]);
    let (msr, mse) = (fit.ssr / fit.dfr, fit.sse / fit.dfe);
    let f = msr / mse;
    g.txt(11, 0, "Regression");
    g.num(11, 1, fit.dfr);
    g.num(11, 2, fit.ssr);
    g.num(11, 3, msr);
    g.put(11, 4, n_or_div0(f));
    g.put(11, 5, if f.is_finite() { callf("F.DIST.RT", &[f, fit.dfr, fit.dfe]) } else { div0() });
    g.txt(12, 0, "Residual");
    g.num(12, 1, fit.dfe);
    g.num(12, 2, fit.sse);
    g.num(12, 3, mse);
    g.txt(13, 0, "Total");
    g.num(13, 1, fit.dft);
    g.num(13, 2, sst);
    let lo2 = format!("Lower {conf:.1}%");
    let hi2 = format!("Upper {conf:.1}%");
    g.labels(15, 1, &["Coefficients", "Standard Error", "t Stat", "P-value", "Lower 95%", "Upper 95%", &lo2, &hi2]);
    let t95 = f64_of(&callf("T.INV.2T", &[0.05, fit.dfe]));
    let tc = f64_of(&callf("T.INV.2T", &[1.0 - conf / 100.0, fit.dfe]));
    for (j, (b, se)) in fit.coef.iter().zip(&fit.se).enumerate() {
        let row = 16 + idx(j);
        g.txt(row, 0, if j == 0 { "Intercept" } else { xv.get(j - 1).map_or("", |v| v.name.as_str()) });
        g.num(row, 1, *b);
        if j == 0 && zero {
            for c in 2..=8 {
                g.put(row, c, Value::Error(CellError::NA));
            }
            continue;
        }
        let t = b / se;
        g.put(row, 2, n_or_div0(*se));
        g.put(row, 3, n_or_div0(t));
        g.put(row, 4, if t.is_finite() { callf("T.DIST.2T", &[t.abs(), fit.dfe]) } else { div0() });
        g.num(row, 5, b - t95 * se);
        g.num(row, 6, b + t95 * se);
        g.num(row, 7, b - tc * se);
        g.num(row, 8, b + tc * se);
    }
    let mut row = 16 + idx(fit.coef.len()) + 3;
    let residuals = flag(p, "residuals");
    let standardized = flag(p, "standardizedResiduals");
    if residuals || standardized {
        g.txt(row, 0, "RESIDUAL OUTPUT");
        let pred = format!("Predicted {}", yvar.name);
        let mut head = vec!["Observation", pred.as_str(), "Residuals"];
        if standardized {
            head.push("Standard Residuals");
        }
        g.labels(row + 2, 0, &head);
        // Residuals scaled by their standard deviation (n − 1).
        let sd = (fit.sse / (n as f64 - 1.0)).sqrt();
        for (i, (yi, pi)) in y.iter().zip(&fit.predicted).enumerate() {
            let r = row + 3 + idx(i);
            g.num(r, 0, (i + 1) as f64);
            g.num(r, 1, *pi);
            g.num(r, 2, yi - pi);
            if standardized {
                g.put(r, 3, ratio(yi - pi, sd));
            }
        }
    }
    if flag(p, "normalProbability") {
        let c = if residuals || standardized { if standardized { 5 } else { 4 } } else { 0 };
        if !(residuals || standardized) {
            row = 16 + idx(fit.coef.len()) + 3;
        }
        g.txt(row, c, "PROBABILITY OUTPUT");
        g.labels(row + 2, c, &["Percentile", &yvar.name]);
        let mut sorted = y.clone();
        sorted.sort_by(f64::total_cmp);
        for (i, v) in sorted.iter().enumerate() {
            let r = row + 3 + idx(i);
            g.num(r, c, 100.0 * (i as f64 + 0.5) / n as f64);
            g.num(r, c + 1, *v);
        }
    }
    Ok(g)
}

// ---------------------------------------------------------------- smoothing

fn series(s: &Session, p: &Json) -> Result<Vec<f64>> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    if r.width() > 1 && r.height() > 1 {
        return Err(EngineError::Other("Input range must be a single row or column.".into()));
    }
    let v = variables(s, si, r, r.width() > 1, flag(p, "labels"), &|_| String::new())?;
    numbers(&v.first().map(|v| v.vals.clone()).unwrap_or_default(), false)
}

/// Writes the forecasts (and with `standardErrors` the standard errors over `se_window`
/// points) down two columns.
fn forecast_grid(x: &[f64], f: &[Option<f64>], se_window: usize, p: &Json) -> Grid {
    let mut g = Grid::default();
    let na = || Value::Error(CellError::NA);
    for (i, fi) in f.iter().enumerate() {
        g.put(idx(i), 0, fi.map(Value::number).unwrap_or_else(na));
    }
    if flag(p, "standardErrors") {
        // A running sum of squared residuals over the last `se_window` points: O(n), not
        // O(n·window). A point without a forecast restarts it, so every window touching it is #N/A.
        let sq = |j: usize| Some((x.get(j)? - (*f.get(j)?)?).powi(2));
        let (mut acc, mut run) = (0.0, 0usize);
        for i in 0..f.len() {
            match sq(i) {
                Some(t) => {
                    acc += t;
                    run += 1;
                }
                None => (acc, run) = (0.0, 0),
            }
            if run > se_window {
                acc -= i.checked_sub(se_window).and_then(sq).unwrap_or(0.0);
                run = se_window;
            }
            // Rounding in the subtraction can leave a tiny negative sum.
            let v = (run == se_window).then(|| (acc.max(0.0) / se_window as f64).sqrt());
            g.put(idx(i), 1, v.map(Value::number).unwrap_or_else(na));
        }
    }
    g
}

fn moving_average(s: &Session, p: &Json) -> Result<Grid> {
    let x = series(s, p)?;
    let interval = num(p, "interval").unwrap_or(3.0);
    if interval < 1.0 || interval.fract() != 0.0 || interval > x.len() as f64 {
        return Err(EngineError::Other("The interval must be a whole number from 1 to the number of data points.".into()));
    }
    let k = interval as usize;
    let mut f = Vec::with_capacity(x.len());
    let mut acc = 0.0;
    for (i, v) in x.iter().enumerate() {
        acc += v;
        if let Some(old) = i.checked_sub(k).and_then(|j| x.get(j)) {
            acc -= old;
        }
        f.push(if i + 1 >= k { Some(acc / k as f64) } else { None });
    }
    Ok(forecast_grid(&x, &f, k, p))
}

fn exponential_smoothing(s: &Session, p: &Json) -> Result<Grid> {
    let x = series(s, p)?;
    let damp = num(p, "damping").unwrap_or(0.3);
    if !(0.0..=1.0).contains(&damp) {
        return Err(EngineError::Other("The damping factor must be between 0 and 1.".into()));
    }
    // F(1) = A(0); F(t) = (1 − damping)·A(t−1) + damping·F(t−1).
    let mut f: Vec<Option<f64>> = Vec::with_capacity(x.len());
    for i in 0..x.len() {
        let v = match i {
            0 => None,
            1 => x.first().copied(),
            _ => x.get(i - 1).zip(f.get(i - 1).copied().flatten()).map(|(a, prev)| (1.0 - damp) * a + damp * prev),
        };
        f.push(v);
    }
    Ok(forecast_grid(&x, &f, 3, p))
}

// ---------------------------------------------------------------- rank and percentile

fn rank_percentile(s: &Session, p: &Json) -> Result<Grid> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let vars = variables(s, si, r, by_rows(p), flag(p, "labels"), &|i| format!("Column{}", i + 1))?;
    let mut g = Grid::default();
    for (vi, v) in vars.iter().enumerate() {
        let c = idx(vi).saturating_mul(4);
        g.labels(0, c, &["Point", &v.name, "Rank", "Percent"]);
        let mut pts: Vec<(usize, f64)> = Vec::new();
        for (i, val) in v.vals.iter().enumerate() {
            match val {
                Value::Number(n) => pts.push((i + 1, *n)),
                Value::Empty => {}
                _ => return Err(non_numeric()),
            }
        }
        let mut asc: Vec<f64> = pts.iter().map(|p| p.1).collect();
        asc.sort_by(f64::total_cmp);
        let n = asc.len();
        pts.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        for (k, (point, x)) in pts.iter().enumerate() {
            let row = idx(k) + 1;
            let below = asc.partition_point(|v| v < x);
            let above = n - asc.partition_point(|v| v <= x);
            g.num(row, c, *point as f64);
            g.num(row, c + 1, *x);
            g.num(row, c + 2, (above + 1) as f64);
            // PERCENTRANK.INC: share of values below, truncated to three digits.
            let pr = if n <= 1 { 1.0 } else { (below as f64 / (n - 1) as f64 * 1000.0 + 1e-9).floor() / 1000.0 };
            g.num(row, c + 3, pr);
            g.pct.push((row, c + 3));
        }
    }
    Ok(g)
}

// ---------------------------------------------------------------- tests of means and variances

fn two_samples(s: &Session, p: &Json, skip_blank: bool) -> Result<(Var, Var, Vec<f64>, Vec<f64>)> {
    let a = single(s, p, "range1", "Variable 1")?;
    let b = single(s, p, "range2", "Variable 2")?;
    let x = numbers(&a.vals, skip_blank)?;
    let y = numbers(&b.vals, skip_blank)?;
    if x.len() < 2 || y.len() < 2 {
        return Err(EngineError::Other("Each input range needs at least two numbers.".into()));
    }
    Ok((a, b, x, y))
}

/// Title, blank row and the "Variable 1 / Variable 2" header; returns the first data row.
fn test_head(g: &mut Grid, title: &str, a: &str, b: &str) -> u32 {
    g.txt(0, 0, title);
    g.txt(2, 1, a);
    g.txt(2, 2, b);
    3
}

/// The t statistic rows shared by the three t-tests.
fn t_rows(g: &mut Grid, mut row: u32, t: f64, df: f64, alpha: f64) {
    let ta = t.abs();
    let lines: [(&str, Value); 5] = [
        ("t Stat", n_or_div0(t)),
        ("P(T<=t) one-tail", if t.is_finite() { callf("T.DIST.RT", &[ta, df]) } else { div0() }),
        ("t Critical one-tail", callf("T.INV", &[1.0 - alpha, df])),
        ("P(T<=t) two-tail", if t.is_finite() { callf("T.DIST.2T", &[ta, df]) } else { div0() }),
        ("t Critical two-tail", callf("T.INV.2T", &[alpha, df])),
    ];
    for (label, v) in lines {
        g.txt(row, 0, label);
        g.put(row, 1, v);
        row += 1;
    }
}

fn pair_row(g: &mut Grid, row: u32, label: &str, a: Value, b: Value) {
    g.txt(row, 0, label);
    g.put(row, 1, a);
    g.put(row, 2, b);
}

fn t_test(s: &Session, p: &Json, kind: &str) -> Result<Grid> {
    let alpha = alpha(p)?;
    let hmd = num(p, "hypothesizedDifference").or_else(|| num(p, "difference")).unwrap_or(0.0);
    let paired = kind == "tTestPaired";
    let (a, b, x, y) = two_samples(s, p, !paired)?;
    let (n1, n2) = (x.len() as f64, y.len() as f64);
    let (m1, m2, v1, v2) = (mean(&x), mean(&y), var_s(&x), var_s(&y));
    let mut g = Grid::default();
    let title = match kind {
        "tTestPaired" => "t-Test: Paired Two Sample for Means",
        "tTestEqual" => "t-Test: Two-Sample Assuming Equal Variances",
        _ => "t-Test: Two-Sample Assuming Unequal Variances",
    };
    let mut row = test_head(&mut g, title, &a.name, &b.name);
    pair_row(&mut g, row, "Mean", Value::number(m1), Value::number(m2));
    pair_row(&mut g, row + 1, "Variance", Value::number(v1), Value::number(v2));
    pair_row(&mut g, row + 2, "Observations", Value::number(n1), Value::number(n2));
    row += 3;
    let (t, df) = match kind {
        "tTestPaired" => {
            if x.len() != y.len() {
                return Err(EngineError::Other("Input ranges must have the same number of values.".into()));
            }
            pair_row(&mut g, row, "Pearson Correlation", correlation(&x, &y), Value::Empty);
            row += 1;
            let dif: Vec<f64> = x.iter().zip(&y).map(|(a, b)| a - b).collect();
            let sd = var_s(&dif).sqrt();
            ((mean(&dif) - hmd) / (sd / n1.sqrt()), n1 - 1.0)
        }
        "tTestEqual" => {
            let pooled = ((n1 - 1.0) * v1 + (n2 - 1.0) * v2) / (n1 + n2 - 2.0);
            pair_row(&mut g, row, "Pooled Variance", Value::number(pooled), Value::Empty);
            row += 1;
            ((m1 - m2 - hmd) / (pooled * (1.0 / n1 + 1.0 / n2)).sqrt(), n1 + n2 - 2.0)
        }
        _ => {
            let (q1, q2) = (v1 / n1, v2 / n2);
            // Welch–Satterthwaite degrees of freedom, rounded to a whole number.
            let df = ((q1 + q2).powi(2) / (q1 * q1 / (n1 - 1.0) + q2 * q2 / (n2 - 1.0))).round();
            ((m1 - m2 - hmd) / (q1 + q2).sqrt(), df)
        }
    };
    pair_row(&mut g, row, "Hypothesized Mean Difference", Value::number(hmd), Value::Empty);
    pair_row(&mut g, row + 1, "df", n_or_div0(df), Value::Empty);
    t_rows(&mut g, row + 2, t, df, alpha);
    Ok(g)
}

fn f_test(s: &Session, p: &Json) -> Result<Grid> {
    let alpha = alpha(p)?;
    let (a, b, x, y) = two_samples(s, p, true)?;
    let (v1, v2) = (var_s(&x), var_s(&y));
    let (d1, d2) = (x.len() as f64 - 1.0, y.len() as f64 - 1.0);
    let mut g = Grid::default();
    let row = test_head(&mut g, "F-Test Two-Sample for Variances", &a.name, &b.name);
    pair_row(&mut g, row, "Mean", Value::number(mean(&x)), Value::number(mean(&y)));
    pair_row(&mut g, row + 1, "Variance", Value::number(v1), Value::number(v2));
    pair_row(&mut g, row + 2, "Observations", Value::number(d1 + 1.0), Value::number(d2 + 1.0));
    pair_row(&mut g, row + 3, "df", Value::number(d1), Value::number(d2));
    let f = v1 / v2;
    pair_row(&mut g, row + 4, "F", n_or_div0(f), Value::Empty);
    // The tail on the side of F: left when F < 1, right otherwise.
    let (pv, crit) = if !f.is_finite() {
        (div0(), div0())
    } else if f < 1.0 {
        (call("F.DIST", &[Value::Number(f), Value::Number(d1), Value::Number(d2), Value::Bool(true)]), callf("F.INV", &[alpha, d1, d2]))
    } else {
        (callf("F.DIST.RT", &[f, d1, d2]), callf("F.INV.RT", &[alpha, d1, d2]))
    };
    pair_row(&mut g, row + 5, "P(F<=f) one-tail", pv, Value::Empty);
    pair_row(&mut g, row + 6, "F Critical one-tail", crit, Value::Empty);
    Ok(g)
}

fn anova_single(s: &Session, p: &Json) -> Result<Grid> {
    let alpha = alpha(p)?;
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let vars = variables(s, si, r, by_rows(p), flag(p, "labels"), &|i| format!("Column {}", i + 1))?;
    let groups: Vec<(String, Vec<f64>)> = vars.into_iter().map(|v| Ok((v.name, numbers(&v.vals, true)?))).collect::<Result<_>>()?;
    if groups.len() < 2 || groups.iter().any(|(_, x)| x.is_empty()) {
        return Err(EngineError::Other("Anova needs at least two groups, each with at least one number.".into()));
    }
    let mut g = Grid::default();
    g.txt(0, 0, "Anova: Single Factor");
    g.txt(2, 0, "SUMMARY");
    g.labels(3, 0, &["Groups", "Count", "Sum", "Average", "Variance"]);
    let all: Vec<f64> = groups.iter().flat_map(|(_, x)| x.iter().copied()).collect();
    let grand = mean(&all);
    let (mut ssb, mut ssw) = (0.0, 0.0);
    for (i, (name, x)) in groups.iter().enumerate() {
        let row = 4 + idx(i);
        let m = mean(x);
        g.txt(row, 0, name);
        g.num(row, 1, x.len() as f64);
        g.num(row, 2, x.iter().sum());
        g.num(row, 3, m);
        g.put(row, 4, n_or_div0(var_s(x)));
        ssb += x.len() as f64 * (m - grand) * (m - grand);
        ssw += ss(x);
    }
    let k = groups.len() as f64;
    let n = all.len() as f64;
    let (dfb, dfw) = (k - 1.0, n - k);
    let row = 4 + idx(groups.len()) + 2;
    g.txt(row, 0, "ANOVA");
    g.labels(row + 1, 0, &["Source of Variation", "SS", "df", "MS", "F", "P-value", "F crit"]);
    let (msb, msw) = (ssb / dfb, ratio(ssw, dfw));
    let f = msw.as_f64().map_or(f64::NAN, |w| msb / w);
    g.txt(row + 2, 0, "Between Groups");
    g.num(row + 2, 1, ssb);
    g.num(row + 2, 2, dfb);
    g.num(row + 2, 3, msb);
    g.put(row + 2, 4, n_or_div0(f));
    g.put(row + 2, 5, if f.is_finite() && dfw > 0.0 { callf("F.DIST.RT", &[f, dfb, dfw]) } else { div0() });
    g.put(row + 2, 6, if dfw > 0.0 { callf("F.INV.RT", &[alpha, dfb, dfw]) } else { div0() });
    g.txt(row + 3, 0, "Within Groups");
    g.num(row + 3, 1, ssw);
    g.num(row + 3, 2, dfw);
    g.put(row + 3, 3, msw);
    g.txt(row + 5, 0, "Total");
    g.num(row + 5, 1, ssb + ssw);
    g.num(row + 5, 2, n - 1.0);
    Ok(g)
}

// ---------------------------------------------------------------- two-factor anova and z-test

/// Count, Sum, Average and Variance of a group, as the Anova summaries list them.
fn group_summary(x: &[f64]) -> [Value; 4] {
    [Value::number(x.len() as f64), Value::number(x.iter().sum()), n_or_div0(mean(x)), n_or_div0(var_s(x))]
}

const ANOVA_HEAD: [&str; 7] = ["Source of Variation", "SS", "df", "MS", "F", "P-value", "F crit"];

/// One line of an Anova table. A factor line, tested against the error term `err` (its MS and
/// df), adds F, the P-value and the critical F.
fn anova_line(g: &mut Grid, row: u32, label: &str, ss: f64, df: f64, err: Option<(f64, f64)>, alpha: f64) {
    g.txt(row, 0, label);
    g.num(row, 1, ss);
    g.num(row, 2, df);
    g.put(row, 3, ratio(ss, df));
    if let Some((mse, dfe)) = err {
        let f = ss / df / mse;
        g.put(row, 4, n_or_div0(f));
        g.put(row, 5, if f.is_finite() && dfe > 0.0 { callf("F.DIST.RT", &[f, df, dfe]) } else { div0() });
        g.put(row, 6, if df > 0.0 && dfe > 0.0 { callf("F.INV.RT", &[alpha, df, dfe]) } else { div0() });
    }
}

/// The cells of a two-factor input range, row by row.
fn input_rows(s: &Session, p: &Json) -> Result<Vec<Vec<Value>>> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    Ok(variables(s, si, r, true, false, &|_| String::new())?.into_iter().map(|v| v.vals).collect())
}

/// The values of column `j` of a table of numbers.
fn column(data: &[Vec<f64>], j: usize) -> Vec<f64> {
    data.iter().filter_map(|r| r.get(j).copied()).collect()
}

/// Anova: Two-Factor Without Replication: one observation for each pair of levels (rows ×
/// columns), the error term being the residual after both main effects.
fn anova_two_factor_without(s: &Session, p: &Json) -> Result<Grid> {
    let alpha = alpha(p)?;
    let labels = flag(p, "labels");
    let mut rows = input_rows(s, p)?;
    let head = if labels && !rows.is_empty() { Some(rows.remove(0)) } else { None };
    let mut row_names = Vec::with_capacity(rows.len());
    let mut data: Vec<Vec<f64>> = Vec::with_capacity(rows.len());
    for (i, mut row) in rows.into_iter().enumerate() {
        let name = if labels && !row.is_empty() { row.remove(0).display() } else { format!("Row {}", i + 1) };
        row_names.push(name);
        data.push(numbers(&row, false)?);
    }
    let (a, b) = (data.len(), data.first().map_or(0, Vec::len));
    if a < 2 || b < 2 {
        return Err(EngineError::Other("Anova needs at least two rows and two columns of numbers.".into()));
    }
    let col_names: Vec<String> = match head {
        Some(h) => h.iter().skip(1).map(Value::display).collect(),
        None => (1..=b).map(|j| format!("Column {j}")).collect(),
    };
    let cols: Vec<Vec<f64>> = (0..b).map(|j| column(&data, j)).collect();
    let all: Vec<f64> = data.iter().flatten().copied().collect();
    let grand = mean(&all);
    let row_means: Vec<f64> = data.iter().map(|r| mean(r)).collect();
    let col_means: Vec<f64> = cols.iter().map(|c| mean(c)).collect();
    let ss_rows: f64 = row_means.iter().map(|m| b as f64 * (m - grand).powi(2)).sum();
    let ss_cols: f64 = col_means.iter().map(|m| a as f64 * (m - grand).powi(2)).sum();
    // The residual of each observation after removing both main effects.
    let ss_err: f64 =
        data.iter().zip(&row_means).flat_map(|(r, rm)| r.iter().zip(&col_means).map(move |(x, cm)| (x - rm - cm + grand).powi(2))).sum();
    let (df_rows, df_cols) = ((a - 1) as f64, (b - 1) as f64);
    let df_err = df_rows * df_cols;
    let mut g = Grid::default();
    g.txt(0, 0, "Anova: Two-Factor Without Replication");
    g.labels(2, 0, &["SUMMARY", "Count", "Sum", "Average", "Variance"]);
    let summary = |g: &mut Grid, row: u32, name: &str, x: &[f64]| {
        g.txt(row, 0, name);
        for (k, v) in group_summary(x).into_iter().enumerate() {
            g.put(row, 1 + idx(k), v);
        }
    };
    for (i, (name, x)) in row_names.iter().zip(&data).enumerate() {
        summary(&mut g, 3 + idx(i), name, x);
    }
    let first_col = 3 + idx(a) + 1;
    for (j, (name, x)) in col_names.iter().zip(&cols).enumerate() {
        summary(&mut g, first_col + idx(j), name, x);
    }
    let row = first_col + idx(b) + 2;
    g.txt(row, 0, "ANOVA");
    g.labels(row + 1, 0, &ANOVA_HEAD);
    let err = Some((ss_err / df_err, df_err));
    anova_line(&mut g, row + 2, "Rows", ss_rows, df_rows, err, alpha);
    anova_line(&mut g, row + 3, "Columns", ss_cols, df_cols, err, alpha);
    anova_line(&mut g, row + 4, "Error", ss_err, df_err, None, alpha);
    g.txt(row + 6, 0, "Total");
    g.num(row + 6, 1, ss(&all));
    g.num(row + 6, 2, (all.len() - 1) as f64);
    Ok(g)
}

/// The number of rows in each sample of a two-factor Anova with replication.
fn rows_per_sample(p: &Json, data_rows: usize) -> Result<usize> {
    let per = num(p, "rowsPerSample").ok_or_else(|| bad(CMD, "missing `rowsPerSample` (the number of rows in each sample)"))?;
    if per < 2.0 || per.fract() != 0.0 || per > data_rows as f64 || !data_rows.is_multiple_of(per as usize) {
        return Err(EngineError::Other("Rows per sample must be a whole number of at least 2 that divides the number of data rows.".into()));
    }
    Ok(per as usize)
}

/// Anova: Two-Factor With Replication. The input range holds column labels in its first row and
/// sample labels in its first column; each sample (a level of the row factor) spans
/// `rowsPerSample` consecutive rows, so every cell of the design holds that many observations.
fn anova_two_factor_with(s: &Session, p: &Json) -> Result<Grid> {
    let alpha = alpha(p)?;
    let mut rows = input_rows(s, p)?;
    if rows.len() < 3 {
        return Err(EngineError::Other("The input range must hold a row of labels and at least two samples.".into()));
    }
    let head = rows.remove(0);
    let col_names: Vec<String> = head.iter().skip(1).map(Value::display).collect();
    let per = rows_per_sample(p, rows.len())?;
    let mut sample_names = Vec::new();
    let mut data: Vec<Vec<f64>> = Vec::with_capacity(rows.len());
    for (i, mut row) in rows.into_iter().enumerate() {
        let name = if row.is_empty() { Value::Empty } else { row.remove(0) };
        if i.is_multiple_of(per) {
            sample_names.push(name.display());
        }
        data.push(numbers(&row, false)?);
    }
    let (a, b) = (sample_names.len(), col_names.len());
    if a < 2 || b < 2 {
        return Err(EngineError::Other("Anova needs at least two samples and two columns of numbers.".into()));
    }
    // cells[i][j]: the observations of sample i in column j.
    let cells: Vec<Vec<Vec<f64>>> = data.chunks(per).map(|block| (0..b).map(|j| column(block, j)).collect()).collect();
    let all: Vec<f64> = data.iter().flatten().copied().collect();
    let grand = mean(&all);
    let sample_all: Vec<Vec<f64>> = data.chunks(per).map(|block| block.iter().flatten().copied().collect()).collect();
    let cols: Vec<Vec<f64>> = (0..b).map(|j| column(&data, j)).collect();
    let (sm, cm): (Vec<f64>, Vec<f64>) = (sample_all.iter().map(|x| mean(x)).collect(), cols.iter().map(|x| mean(x)).collect());
    let n = per as f64;
    let ss_sample: f64 = sm.iter().map(|m| n * b as f64 * (m - grand).powi(2)).sum();
    let ss_cols: f64 = cm.iter().map(|m| n * a as f64 * (m - grand).powi(2)).sum();
    let mut ss_inter = 0.0;
    let mut ss_within = 0.0;
    for (i, row) in cells.iter().enumerate() {
        for (j, cell) in row.iter().enumerate() {
            let (si, cj) = (sm.get(i).copied().unwrap_or(f64::NAN), cm.get(j).copied().unwrap_or(f64::NAN));
            ss_inter += n * (mean(cell) - si - cj + grand).powi(2);
            ss_within += ss(cell);
        }
    }
    let (df_sample, df_cols) = ((a - 1) as f64, (b - 1) as f64);
    let df_within = (a * b * (per - 1)) as f64;
    let mut g = Grid::default();
    g.txt(0, 0, "Anova: Two-Factor With Replication");
    g.txt(2, 0, "SUMMARY");
    for (j, name) in col_names.iter().enumerate() {
        g.txt(2, 1 + idx(j), name);
    }
    g.txt(2, 1 + idx(b), "Total");
    let stats = ["Count", "Sum", "Average", "Variance"];
    let mut row = 3u32;
    let block = |g: &mut Grid, row: &mut u32, name: &str, groups: Vec<&[f64]>| {
        g.txt(*row, 0, name);
        let sums: Vec<[Value; 4]> = groups.iter().map(|x| group_summary(x)).collect();
        for (k, label) in stats.iter().enumerate() {
            let r = *row + 1 + idx(k);
            g.txt(r, 0, label);
            for (j, sm) in sums.iter().enumerate() {
                g.put(r, 1 + idx(j), sm.get(k).cloned().unwrap_or(Value::Empty));
            }
        }
        *row += 6;
    };
    for ((name, row_cells), whole) in sample_names.iter().zip(&cells).zip(&sample_all) {
        let mut groups: Vec<&[f64]> = row_cells.iter().map(Vec::as_slice).collect();
        groups.push(whole);
        block(&mut g, &mut row, name, groups);
    }
    block(&mut g, &mut row, "Total", cols.iter().map(Vec::as_slice).collect());
    row += 1;
    g.txt(row, 0, "ANOVA");
    g.labels(row + 1, 0, &ANOVA_HEAD);
    let err = Some((ss_within / df_within, df_within));
    anova_line(&mut g, row + 2, "Sample", ss_sample, df_sample, err, alpha);
    anova_line(&mut g, row + 3, "Columns", ss_cols, df_cols, err, alpha);
    anova_line(&mut g, row + 4, "Interaction", ss_inter, df_sample * df_cols, err, alpha);
    anova_line(&mut g, row + 5, "Within", ss_within, df_within, None, alpha);
    g.txt(row + 7, 0, "Total");
    g.num(row + 7, 1, ss(&all));
    g.num(row + 7, 2, (all.len() - 1) as f64);
    Ok(g)
}

/// A known variance parameter: a positive number.
fn known_variance(p: &Json, key: &str) -> Result<f64> {
    match num(p, key) {
        Some(v) if v > 0.0 => Ok(v),
        _ => Err(EngineError::Other(format!("Enter the known variance `{key}` (a positive number)."))),
    }
}

/// z-Test: Two Sample for Means, for two samples whose population variances are known.
fn z_test(s: &Session, p: &Json) -> Result<Grid> {
    let alpha = alpha(p)?;
    let hmd = match opt_num(p, "hypothesizedDifference")? {
        Some(d) => d,
        None => num_or(p, "difference", 0.0)?,
    };
    let (v1, v2) = (known_variance(p, "variance1")?, known_variance(p, "variance2")?);
    let a = single(s, p, "range1", "Variable 1")?;
    let b = single(s, p, "range2", "Variable 2")?;
    let (x, y) = (numbers(&a.vals, true)?, numbers(&b.vals, true)?);
    if x.is_empty() || y.is_empty() {
        return Err(EngineError::Other("Each input range needs at least one number.".into()));
    }
    let (n1, n2) = (x.len() as f64, y.len() as f64);
    let (m1, m2) = (mean(&x), mean(&y));
    let z = (m1 - m2 - hmd) / (v1 / n1 + v2 / n2).sqrt();
    let mut g = Grid::default();
    let row = test_head(&mut g, "z-Test: Two Sample for Means", &a.name, &b.name);
    pair_row(&mut g, row, "Mean", Value::number(m1), Value::number(m2));
    pair_row(&mut g, row + 1, "Known Variance", Value::number(v1), Value::number(v2));
    pair_row(&mut g, row + 2, "Observations", Value::number(n1), Value::number(n2));
    pair_row(&mut g, row + 3, "Hypothesized Mean Difference", Value::number(hmd), Value::Empty);
    pair_row(&mut g, row + 4, "z", n_or_div0(z), Value::Empty);
    // The tail beyond |z|: 1 − Φ(|z|).
    let tail = call("NORM.S.DIST", &[Value::Number(z.abs()), Value::Bool(true)]).as_f64().map(|c| 1.0 - c);
    let lines: [(&str, Value); 4] = [
        ("P(Z<=z) one-tail", tail.map_or_else(div0, Value::number)),
        ("z Critical one-tail", callf("NORM.S.INV", &[1.0 - alpha])),
        ("P(Z<=z) two-tail", tail.map_or_else(div0, |t| Value::number(2.0 * t))),
        ("z Critical two-tail", callf("NORM.S.INV", &[1.0 - alpha / 2.0])),
    ];
    for (k, (label, v)) in lines.into_iter().enumerate() {
        pair_row(&mut g, row + 5 + idx(k), label, v, Value::Empty);
    }
    Ok(g)
}

// ---------------------------------------------------------------- fourier analysis

/// Radix-2 fast Fourier transform (iterative Cooley–Tukey) in place, for a power-of-two number of
/// complex values (re, im). The forward transform is unscaled, X[k] = Σ x[n]·e^(−2πikn/N); the
/// inverse uses e^(+2πikn/N) and divides by N, so it returns the values the forward one took.
fn fft(a: &mut [(f64, f64)], inverse: bool) {
    let n = a.len();
    if n < 2 || !n.is_power_of_two() {
        return;
    }
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if i < j {
            a.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let ang = sign * 2.0 * std::f64::consts::PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                // Each twiddle factor is computed directly, not by repeated multiplication.
                let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (i, j) = (start + k, start + k + len / 2);
                let (Some(&(ur, ui)), Some(&(xr, xi))) = (a.get(i), a.get(j)) else { continue };
                let (vr, vi) = (xr * wr - xi * wi, xr * wi + xi * wr);
                if let Some(u) = a.get_mut(i) {
                    *u = (ur + vr, ui + vi);
                }
                if let Some(x) = a.get_mut(j) {
                    *x = (ur - vr, ui - vi);
                }
            }
        }
        len <<= 1;
    }
    if inverse {
        let nf = n as f64;
        a.iter_mut().for_each(|v| *v = (v.0 / nf, v.1 / nf));
    }
}

/// Fourier Analysis: the discrete Fourier transform (or with `inverse` its inverse) of one row or
/// column of 2^k values, read and written as complex numbers the way IMREAL, IMAGINARY and
/// COMPLEX read and write them.
fn fourier_analysis(s: &Session, p: &Json) -> Result<Grid> {
    let v = single(s, p, "range", "")?;
    let n = v.vals.len();
    if n == 0 || n > MAX_FOURIER || !n.is_power_of_two() {
        return Err(EngineError::Other(format!("Fourier Analysis needs 1, 2, 4, 8 … {MAX_FOURIER} input values (a power of two).")));
    }
    let mut a = Vec::with_capacity(n);
    for x in &v.vals {
        let part = |f: &str| call(f, std::slice::from_ref(x)).as_f64();
        match (x, part("IMREAL"), part("IMAGINARY")) {
            (Value::Empty, ..) => return Err(non_numeric()),
            (_, Some(re), Some(im)) => a.push((re, im)),
            _ => return Err(EngineError::Other("Input range must hold numbers or complex numbers such as 3+4i.".into())),
        }
    }
    let total: f64 = a.iter().map(|(re, im)| re.abs() + im.abs()).sum();
    let inverse = flag(p, "inverse");
    fft(&mut a, inverse);
    // Parts that can only be rounding noise of the transform are written as 0. Each of the
    // log2(N) butterfly stages adds at most a few units of f64::EPSILON times the magnitude of
    // the values it combines, so the computed transform differs from the exact one by at most
    // about log2(N) · ε · Σ|x_n| times a small constant (Higham, Accuracy and Stability of
    // Numerical Algorithms, 2nd ed., chapter 24, The Fast Fourier Transform). The bound comes
    // from the input and N, never from the largest output, so a small part beside a large one
    // (an imaginary part of 1 beside a real part of 1e14, a harmonic of 4e-7 beside a DC term of
    // 8e6) is kept. With N = 1 the transform is the identity and log2(N) = 0: nothing is zeroed.
    // The inverse divides its outputs by N, so its noise is divided by N as well. An input sum
    // that overflows gives no finite bound; then nothing is cleaned up.
    let mut noise = 8.0 * f64::EPSILON * f64::from(n.trailing_zeros()) * total;
    if inverse {
        noise /= n as f64;
    }
    let noise = if noise.is_finite() { noise } else { 0.0 };
    let clean = |x: f64| if x.abs() <= noise { 0.0 } else { x };
    let mut g = Grid::default();
    let first = u32::from(flag(p, "labels"));
    if first == 1 {
        g.txt(0, 0, &v.name);
    }
    for (i, (re, im)) in a.into_iter().enumerate() {
        g.put(first + idx(i), 0, callf("COMPLEX", &[clean(re), clean(im)]));
    }
    Ok(g)
}

// ---------------------------------------------------------------- random numbers and sampling

/// A seeded generator: xoshiro256** (Blackman and Vigna, a public-domain algorithm) with its
/// state filled by SplitMix64. The same seed gives the same numbers in GridCraft; they are not
/// Excel's numbers, whose generator is not documented.
struct Rng([u64; 4]);

impl Rng {
    fn new(seed: u64) -> Rng {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Rng([next(), next(), next(), next()])
    }

    fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = self.0;
        let out = s1.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let (s2, s3) = (s2 ^ s0, s3 ^ s1);
        self.0 = [s0 ^ s3, s1 ^ s2, s2 ^ (s1 << 17), s3.rotate_left(45)];
        out
    }

    /// Uniform in [0, 1), with 53 random bits.
    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform whole number in [0, n), without modulo bias.
    fn below(&mut self, n: u64) -> u64 {
        if n <= 1 {
            return 0;
        }
        let zone = u64::MAX - u64::MAX % n;
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }

    /// Standard normal by the Box–Muller transform.
    fn normal(&mut self) -> f64 {
        let (u1, u2) = (1.0 - self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// The `seed` parameter, or a seed taken from the clock when there is none.
fn seed_of(p: &Json) -> Result<u64> {
    let given = p.get("seed").is_some_and(|v| match v {
        Json::Null => false,
        Json::String(t) => !t.trim().is_empty(),
        _ => true,
    });
    if !given {
        let ms = gridcraft_core::date::unix_now_ms().unwrap_or(0.0);
        return Ok((ms * 1000.0) as u64 & MAX_SEED);
    }
    match num(p, "seed") {
        Some(x) if x >= 0.0 && x.fract() == 0.0 && x <= MAX_SEED as f64 => Ok(x as u64),
        _ => Err(EngineError::Other("The random seed must be a whole number from 0.".into())),
    }
}

/// A number parameter: `None` when absent, null or blank; an error when present but not a
/// finite number (a typed "0,3" or "abc" must not quietly become the default).
fn opt_num(p: &Json, key: &str) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(t)) if t.trim().is_empty() => Ok(None),
        Some(_) => num(p, key).map(Some).ok_or_else(|| EngineError::Other(format!("`{key}` must be a number."))),
    }
}

/// A number parameter, `default` when absent or blank.
fn num_or(p: &Json, key: &str, default: f64) -> Result<f64> {
    Ok(opt_num(p, key)?.unwrap_or(default))
}

/// A whole-number parameter of at least `min`, `default` when absent.
fn whole(p: &Json, key: &str, default: Option<f64>, min: f64) -> Result<f64> {
    match opt_num(p, key)?.or(default) {
        Some(x) if x >= min && x.fract() == 0.0 => Ok(x),
        Some(_) => Err(EngineError::Other(format!("`{key}` must be a whole number of at least {min}."))),
        None => Err(bad(CMD, format!("missing `{key}`"))),
    }
}

/// A probability parameter in [0, 1].
fn probability(p: &Json) -> Result<f64> {
    match num_or(p, "p", 0.5)? {
        x if (0.0..=1.0).contains(&x) => Ok(x),
        _ => Err(EngineError::Other("The p value must be between 0 and 1.".into())),
    }
}

/// The probabilities of a distribution on 0, 1, 2 … `last`, as (first outcome, probabilities of
/// first, first + 1, …), from its probability `at_mode` at `mode`, its most likely outcome, and
/// `ratio`(k) = pmf(k + 1) / pmf(k). Each term is one multiplication of its neighbour towards the
/// mode, so no worksheet function runs per outcome, and the terms only shrink away from the mode,
/// so nothing underflows before the terms that matter. The left tail ends where its terms vanish,
/// the right tail once the cumulative probability reaches 1 within f64 resolution (every later
/// draw would map to the last outcome). The table is as wide as the distribution's effective
/// support, not as `last`: Binomial(1,000,000, 0.5) takes about 23,000 entries, not 1,000,001.
fn pmf_table(last: f64, mode: f64, at_mode: f64, ratio: impl Fn(f64) -> f64) -> Result<(f64, Vec<f64>)> {
    if !at_mode.is_finite() || at_mode <= 0.0 || !(0.0..=last).contains(&mode) || mode.fract() != 0.0 {
        return Err(EngineError::Other("The distribution could not be tabulated.".into()));
    }
    let mut left = Vec::new();
    let (mut k, mut v) = (mode, at_mode);
    while k > 0.0 {
        v /= ratio(k - 1.0);
        if v.is_nan() || v < f64::MIN_POSITIVE {
            break;
        }
        left.push(v);
        k -= 1.0;
    }
    let first = k;
    let mut pmf: Vec<f64> = left.into_iter().rev().collect();
    let mut acc = pmf.iter().sum::<f64>() + at_mode;
    pmf.push(at_mode);
    let (mut k, mut v) = (mode, at_mode);
    while k < last && acc < 1.0 - 1e-15 {
        v *= ratio(k);
        if v.is_nan() || v < f64::MIN_POSITIVE {
            break;
        }
        pmf.push(v);
        acc += v;
        k += 1.0;
    }
    Ok((first, pmf))
}

/// A distribution on whole numbers tabulated for inversion sampling: the cumulative
/// probabilities of `first`, `first + 1`, … up to the last outcome that matters.
struct Table {
    first: f64,
    cdf: Vec<f64>,
}

impl Table {
    /// The distribution on 0, 1, 2 … `last` that `pmf_table` describes.
    fn from_mode(last: f64, mode: f64, at_mode: f64, ratio: impl Fn(f64) -> f64) -> Result<Table> {
        let (first, pmf) = pmf_table(last, mode, at_mode, ratio)?;
        let mut acc = 0.0;
        let cdf = pmf
            .into_iter()
            .map(|p| {
                acc += p;
                acc
            })
            .collect();
        Ok(Table { first, cdf })
    }

    /// All the probability on one outcome.
    fn point(value: f64) -> Table {
        Table { first: value, cdf: vec![1.0] }
    }

    /// A draw: the first outcome whose cumulative probability exceeds a uniform number (inversion).
    fn draw(&self, r: &mut Rng) -> f64 {
        let u = r.uniform() * self.cdf.last().copied().unwrap_or(1.0);
        self.first + self.cdf.partition_point(|c| *c <= u).min(self.cdf.len().saturating_sub(1)) as f64
    }
}

/// The numbers of a patterned sequence: `from` to `to` in steps, each number repeated, the whole
/// sequence repeated.
fn pattern(p: &Json) -> Result<Vec<f64>> {
    let (from, to, step) = (num_or(p, "from", 1.0)?, num_or(p, "to", 1.0)?, num_or(p, "step", 1.0)?);
    let (each, times) = (whole(p, "repeatEach", Some(1.0), 1.0)?, whole(p, "repeatSequence", Some(1.0), 1.0)?);
    if step <= 0.0 || to < from {
        return Err(EngineError::Other("The pattern must run from a smaller number to a larger one in positive steps.".into()));
    }
    let steps = ((to - from) / step + 1e-9).floor() + 1.0;
    if steps * each * times > MAX_RANDOM as f64 {
        return Err(EngineError::Other(format!("Random Number Generation writes at most {MAX_RANDOM} numbers.")));
    }
    let base: Vec<f64> = (0..steps as usize).map(|k| from + step * k as f64).collect();
    let one: Vec<f64> = base.iter().flat_map(|v| std::iter::repeat_n(*v, each as usize)).collect();
    Ok(one.repeat(times as usize))
}

/// Random Number Generation: `count` numbers from a distribution in each of `variables`
/// columns, drawn row by row.
fn random_numbers(s: &Session, p: &Json, seed: u64) -> Result<Grid> {
    let vars = whole(p, "variables", Some(1.0), 1.0)?;
    let dist = text(p, &["distribution"]).map(norm).unwrap_or_else(|| "uniform".into());
    let cells = |rows: f64| {
        if rows * vars > MAX_RANDOM as f64 {
            Err(EngineError::Other(format!("Random Number Generation writes at most {MAX_RANDOM} numbers.")))
        } else {
            Ok(rows as usize)
        }
    };
    let mut g = Grid::default();
    let cols = vars as usize;
    if dist == "patterned" {
        let seq = pattern(p)?;
        cells(seq.len() as f64)?;
        for (i, x) in seq.iter().enumerate() {
            for c in 0..cols {
                g.num(idx(i), idx(c), *x);
            }
        }
        return Ok(g);
    }
    let count = cells(whole(p, "count", None, 1.0)?)?;
    let mut draw: Box<dyn FnMut(&mut Rng) -> f64> = match dist.as_str() {
        "uniform" => {
            let (a, b) = (num_or(p, "from", 0.0)?, num_or(p, "to", 1.0)?);
            Box::new(move |r| a + (b - a) * r.uniform())
        }
        "normal" => {
            let (m, sd) = (num_or(p, "mean", 0.0)?, num_or(p, "standardDeviation", 1.0)?);
            if sd < 0.0 {
                return Err(EngineError::Other("The standard deviation must not be negative.".into()));
            }
            Box::new(move |r| m + sd * r.normal())
        }
        "bernoulli" => {
            let pr = probability(p)?;
            Box::new(move |r| if r.uniform() < pr { 1.0 } else { 0.0 })
        }
        "binomial" => {
            let (pr, n) = (probability(p)?, whole(p, "trials", Some(1.0), 1.0)?);
            if n > MAX_OUTCOMES {
                return Err(EngineError::Other(format!("The number of trials must be at most {MAX_OUTCOMES}.")));
            }
            // p = 0 and p = 1 are exact: every draw is 0 or n. Otherwise one BINOM.DIST value at
            // the mode ⌊(n + 1)p⌋ anchors pmf(k + 1) = pmf(k) · (n − k)/(k + 1) · p/(1 − p).
            let table = if pr == 0.0 {
                Table::point(0.0)
            } else if pr == 1.0 {
                Table::point(n)
            } else {
                let mode = ((n + 1.0) * pr).floor().min(n);
                let at_mode = f64_of(&call("BINOM.DIST", &[Value::Number(mode), Value::Number(n), Value::Number(pr), Value::Bool(false)]));
                let odds = pr / (1.0 - pr);
                Table::from_mode(n, mode, at_mode, |k| (n - k) / (k + 1.0) * odds)?
            };
            Box::new(move |r| table.draw(r))
        }
        "poisson" => {
            let lambda = num_or(p, "lambda", 1.0)?;
            if lambda <= 0.0 || lambda > 100_000.0 {
                return Err(EngineError::Other("Lambda must be greater than 0 and at most 100000.".into()));
            }
            // One POISSON.DIST value at the mode ⌊λ⌋ anchors pmf(k + 1) = pmf(k) · λ/(k + 1).
            let last = (lambda + 40.0 * lambda.sqrt() + 100.0).min(MAX_OUTCOMES);
            let mode = lambda.floor();
            let at_mode = f64_of(&call("POISSON.DIST", &[Value::Number(mode), Value::Number(lambda), Value::Bool(false)]));
            let table = Table::from_mode(last, mode, at_mode, |k| lambda / (k + 1.0))?;
            Box::new(move |r| table.draw(r))
        }
        "discrete" => {
            let (si, r) = input_range(s, p, &["valueRange", "values"])?;
            if r.width() != 2 {
                return Err(EngineError::Other("The value and probability input range must have two columns.".into()));
            }
            let v = variables(s, si, r, false, false, &|_| String::new())?;
            let (values, probs) = match v.as_slice() {
                [a, b] => (numbers(&a.vals, false)?, numbers(&b.vals, false)?),
                _ => return Err(non_numeric()),
            };
            if probs.iter().any(|x| !(0.0..=1.0).contains(x)) || (probs.iter().sum::<f64>() - 1.0).abs() > 1e-9 {
                return Err(EngineError::Other("The probabilities must be between 0 and 1 and add up to 1.".into()));
            }
            let cdf: Vec<f64> = probs
                .iter()
                .scan(0.0, |acc, x| {
                    *acc += x;
                    Some(*acc)
                })
                .collect();
            let table = Table { first: 0.0, cdf };
            Box::new(move |r| values.get(table.draw(r) as usize).copied().unwrap_or(f64::NAN))
        }
        other => return Err(bad(CMD, format!("unknown distribution `{other}`"))),
    };
    let mut rng = Rng::new(seed);
    for i in 0..count {
        for c in 0..cols {
            g.num(idx(i), idx(c), draw(&mut rng));
        }
    }
    Ok(g)
}

/// Whether Random Number Generation writes a patterned sequence (which uses no seed).
fn patterned(p: &Json) -> bool {
    text(p, &["distribution"]).map(norm).is_some_and(|d| d == "patterned")
}

fn random_sampling(p: &Json) -> bool {
    text(p, &["method"]).is_some_and(|m| m.eq_ignore_ascii_case("random"))
}

/// Sampling: every `period`-th value of the input, or `samples` values drawn at random with
/// replacement, down one column. The input is read column by column (row by row for a single
/// row); blanks are skipped.
fn sampling(s: &Session, p: &Json, seed: Option<u64>) -> Result<Grid> {
    let (si, r) = input_range(s, p, &["range", "input"])?;
    let by_rows = r.height() == 1 && r.width() > 1;
    let vars = variables(s, si, r, by_rows, flag(p, "labels"), &|_| String::new())?;
    let mut x = Vec::new();
    for v in &vars {
        x.extend(numbers(&v.vals, true)?);
    }
    if x.is_empty() {
        return Err(EngineError::Other("The input range holds no numbers.".into()));
    }
    let picked: Vec<f64> = match seed {
        Some(seed) => {
            let k = whole(p, "samples", None, 1.0)?;
            if k > MAX_RANDOM as f64 {
                return Err(EngineError::Other(format!("Sampling takes at most {MAX_RANDOM} samples.")));
            }
            let mut rng = Rng::new(seed);
            (0..k as usize).filter_map(|_| x.get(rng.below(x.len() as u64) as usize).copied()).collect()
        }
        None => {
            let period = whole(p, "period", None, 1.0)?;
            if period > x.len() as f64 {
                return Err(EngineError::Other("The period must be at most the number of input values.".into()));
            }
            x.iter().skip(period as usize - 1).step_by(period as usize).copied().collect()
        }
    };
    let mut g = Grid::default();
    for (i, v) in picked.iter().enumerate() {
        g.num(idx(i), 0, *v);
    }
    Ok(g)
}

// ---------------------------------------------------------------- output

/// Writes the report at `output` (refusing to overwrite data unless `overwrite`) or on a new
/// sheet.
fn write_report(s: &mut Session, p: &Json, tool: &str, g: Grid) -> Result<Json> {
    let (rows, cols) = g.extent();
    let (rows, cols) = (rows.max(1), cols.max(1));
    if g.too_large || u64::from(rows) * u64::from(cols) > MAX_OUTPUT_CELLS {
        return Err(bad(CMD, "the report is too large"));
    }
    let d = s.doc()?;
    let target = match str_param(p, "outputTo").map(str::to_ascii_lowercase).as_deref() {
        Some("newsheet") => None,
        Some("range") => Some(text(p, &["output", "outputRange"]).ok_or_else(|| bad(CMD, "missing `output` (the output range)"))?),
        _ => text(p, &["output", "outputRange"]),
    };
    let (si, at, new_sheet) = match target {
        Some(t) => {
            let (si, r) = parse_range(&d.wb, t, CMD)?;
            (si, r.start, None)
        }
        None => {
            if d.wb.protected_structure {
                return Err(EngineError::Other("The workbook is protected and cannot be changed.".into()));
            }
            let name = match p.get("newSheet") {
                Some(Json::String(n)) if !n.trim().is_empty() => n.trim().to_string(),
                _ => d.wb.next_sheet_name(),
            };
            d.wb.check_sheet_name(&name, None).map_err(EngineError::Other)?;
            (d.wb.sheets.len(), CellRef::new(0, 0), Some(name))
        }
    };
    let end_row = at.row.checked_add(rows - 1).filter(|r| *r < MAX_ROWS);
    let end_col = at.col.checked_add(cols - 1).filter(|c| *c < MAX_COLS);
    let (Some(end_row), Some(end_col)) = (end_row, end_col) else {
        return Err(EngineError::Other("The report doesn't fit on the sheet below and right of the output cell.".into()));
    };
    let rect = RangeRef::new(at, CellRef::new(end_row, end_col));
    // Existing content inside the report's rectangle.
    let mut occupied: Vec<CellRef> = Vec::new();
    if new_sheet.is_none()
        && let Some(sh) = d.wb.sheet(si)
    {
        for row in rect.start.row..=rect.end.row {
            for (c, cell) in sh.cells.row(row, rect.start.col, rect.end.col) {
                if cell.has_content() {
                    occupied.push(CellRef::new(row, c));
                }
            }
        }
        if sh.spill.keys().any(|c| rect.contains(*c)) && occupied.is_empty() {
            occupied.push(rect.start);
        }
    }
    if !occupied.is_empty() && !flag(p, "overwrite") {
        return Err(EngineError::Other(format!("The output range {} {OVERWRITE_ERROR}; pass overwrite: true to replace it.", rect.a1())));
    }
    let sheet_name = new_sheet.clone().or_else(|| d.wb.sheet(si).map(|sh| sh.name.clone())).unwrap_or_default();
    let new_index = edit(s, |cx| {
        let pct_fmt = NumFmt::new("0.00%");
        if let Some(name) = &new_sheet {
            cx.wb.sheets.push(std::sync::Arc::new(Sheet::new(name.clone())));
            cx.structural = true;
        }
        let pct_styles: Vec<(CellRef, gridcraft_model::StyleId)> = {
            let sh = cx.wb.sheet(si).ok_or(EngineError::NoDocument)?;
            g.pct.iter().map(|(r, c)| CellRef::new(at.row + r, at.col + c)).map(|c| (c, sh.style_id(c))).collect()
        };
        let pct_styles: HashMap<CellRef, gridcraft_model::StyleId> =
            pct_styles.into_iter().map(|(c, st)| (c, cx.wb.styles.derive(st, |s| s.num_fmt = pct_fmt.clone()))).collect();
        let sh = cx.sheet_mut(si)?;
        for c in &occupied {
            let style = sh.style_id(*c);
            sh.set_cell(*c, Cell { value: Value::Empty, formula: None, style, py: None });
        }
        let mut touched = occupied.clone();
        for ((r, c), v) in &g.cells {
            let cell = CellRef::new(at.row + r, at.col + c);
            let style = pct_styles.get(&cell).copied().unwrap_or_else(|| sh.style_id(cell));
            sh.set_cell(cell, Cell { value: v.clone(), formula: None, style, py: None });
            touched.push(cell);
        }
        for c in touched {
            cx.touch(si, c);
        }
        Ok(new_sheet.as_ref().map(|_| si))
    })?;
    if let Some(i) = new_index {
        s.doc_mut()?.wb_switch_sheet(i);
    }
    Ok(json!({"tool": tool, "sheet": sheet_name, "range": rect.a1(), "rows": rows, "cols": cols}))
}

fn run(s: &mut Session, p: &Json) -> Result<Json> {
    let Some(t) = text(p, &["tool"]) else {
        s.ui_requests.push(crate::UiRequest::Dialog("analysisToolPak".into(), json!({})));
        return Ok(json!({"tools": TOOLS.iter().map(|(id, label)| json!({"id": id, "label": label})).collect::<Vec<_>>()}));
    };
    let tool = tool_id(t).ok_or_else(|| bad(CMD, format!("unknown tool `{t}`")))?;
    let mut seed = None;
    let g = match tool {
        "descriptive" => descriptive(s, p)?,
        "histogram" => histogram(s, p)?,
        "correlation" => matrix(s, p, correlation, |_| Value::Number(1.0))?,
        "covariance" => matrix(s, p, covariance_p, |x| covariance_p(x, x))?,
        "regression" => regression(s, p)?,
        "movingAverage" => moving_average(s, p)?,
        "exponentialSmoothing" => exponential_smoothing(s, p)?,
        "rankPercentile" => rank_percentile(s, p)?,
        "tTestPaired" | "tTestEqual" | "tTestUnequal" => t_test(s, p, tool)?,
        "fTest" => f_test(s, p)?,
        "anovaSingle" => anova_single(s, p)?,
        "anovaTwoFactorWithout" => anova_two_factor_without(s, p)?,
        "anovaTwoFactorWith" => anova_two_factor_with(s, p)?,
        "zTest" => z_test(s, p)?,
        "fourierAnalysis" => fourier_analysis(s, p)?,
        "randomNumberGeneration" => {
            // A patterned sequence draws nothing: its seed is neither read nor reported.
            if !patterned(p) {
                seed = Some(seed_of(p)?);
            }
            random_numbers(s, p, seed.unwrap_or(0))?
        }
        "sampling" => {
            if random_sampling(p) {
                seed = Some(seed_of(p)?);
            }
            sampling(s, p, seed)?
        }
        _ => return Err(bad(CMD, format!("unknown tool `{t}`"))),
    };
    let mut out = write_report(s, p, tool, g)?;
    if let Some(k) = seed {
        out["seed"] = json!(k);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use gridcraft_core::{CellRef, Value};
    use serde_json::{Value as Json, json};

    use super::{MAX_OUTCOMES, call, f64_of, pmf_table};
    use crate::Session;

    fn s() -> Session {
        let mut s = Session::new();
        s.new_workbook();
        s
    }
    fn v(s: &Session, a: &str) -> Value {
        let d = s.doc().unwrap();
        let (sh, body) = crate::cmd::split_sheet(a);
        let si = sh.map(|n| d.wb.sheet_index(&n).unwrap()).unwrap_or(d.wb.active_sheet());
        d.wb.sheet(si).unwrap().value(CellRef::parse(body).unwrap())
    }
    fn n(s: &Session, a: &str) -> f64 {
        v(s, a).as_f64().unwrap_or(f64::NAN)
    }
    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }
    /// Evaluates a formula in a scratch cell.
    fn eval(s: &mut Session, f: &str) -> f64 {
        s.execute("cell.set", json!({"cell": "Z1000", "input": f})).unwrap();
        let x = n(s, "Z1000");
        s.execute("cell.set", json!({"cell": "Z1000", "input": ""})).unwrap();
        x
    }

    #[test]
    fn a_report_stops_growing_past_the_output_limit() {
        // Entries past the limit are not kept, and the report is refused.
        let mut g = super::Grid::default();
        g.num(1999, 1999, 1.0);
        assert!(!g.too_large);
        g.num(0, 2000, 1.0);
        assert!(g.too_large);
        assert_eq!(g.cells.len(), 1);
        assert_eq!(g.extent(), (2000, 2000));
        // Row-grouped statistics of a tall, almost empty column: one variable per row would make
        // millions of report entries; the report is refused instead of built.
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1]]})).unwrap();
        s.execute("range.setValues", json!({"range": "A1000000", "values": [[2]]})).unwrap();
        let err = s
            .execute("data.analysisToolPak", json!({"tool": "descriptive", "range": "A1:A1000000", "groupedBy": "rows", "output": "C1"}))
            .expect_err("report too large");
        assert!(err.to_string().contains("too large"), "{err}");
        assert_eq!(v(&s, "C1"), Value::Empty);
    }

    #[test]
    fn forecast_standard_errors_match_the_full_window_rescan() {
        // #1043: the running sum must give what re-adding every window gave, gaps included.
        let x: [f64; 11] = [2.0, 4.0, 5.0, 4.0, 5.0, 7.0, 1.0, 3.0, 8.0, 6.0, 2.0];
        let f = [None, Some(3.0), Some(4.5), None, Some(4.0), Some(6.5), Some(2.0), Some(2.5), Some(7.0), Some(5.0), Some(4.0)];
        let rescan = |i: usize, w: usize| -> Value {
            let v = (i + 1).checked_sub(w).and_then(|s0| {
                let mut acc = 0.0;
                for j in s0..=i {
                    acc += (x.get(j)? - (*f.get(j)?)?).powi(2);
                }
                Some((acc / w as f64).sqrt())
            });
            v.map(Value::number).unwrap_or(Value::Error(gridcraft_core::CellError::NA))
        };
        for w in 1..=5 {
            let g = super::forecast_grid(&x, &f, w, &json!({"standardErrors": true}));
            for i in 0..f.len() {
                let (got, want) = (g.cells.get(&(i as u32, 1)).cloned().unwrap_or(Value::Empty), rescan(i, w));
                match (got.as_f64(), want.as_f64()) {
                    (Some(a), Some(b)) => assert!(close(a, b, 1e-12), "window {w}, row {i}: {a} vs {b}"),
                    _ => assert_eq!(got, want, "window {w}, row {i}"),
                }
            }
        }
        // Row 3 has no forecast: windows through it are #N/A, the next full window after it is not.
        let g = super::forecast_grid(&x, &f, 3, &json!({"standardErrors": true}));
        assert_eq!(g.cells.get(&(5, 1)), Some(&Value::Error(gridcraft_core::CellError::NA)));
        assert!(g.cells.get(&(6, 1)).and_then(Value::as_f64).is_some());
    }

    #[test]
    fn descriptive_statistics_of_one_to_five() {
        // 1..5: mean 3, s = √2.5, kurtosis −1.2, skewness 0 (textbook values).
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [["Score"], [1], [2], [3], [4], [5]]})).unwrap();
        let r = s
            .execute("data.analysisToolPak", json!({"tool": "Descriptive Statistics", "range": "A1:A6", "labels": true, "output": "C1", "confidence": 95, "kthLargest": 2, "kthSmallest": 1}))
            .unwrap();
        assert_eq!(r["range"], "C1:D18");
        assert_eq!(v(&s, "C1"), Value::text("Score"));
        assert_eq!(v(&s, "C3"), Value::text("Mean"));
        assert_eq!(n(&s, "D3"), 3.0);
        assert!(close(n(&s, "D4"), (2.5f64 / 5.0).sqrt(), 1e-12));
        assert_eq!(n(&s, "D5"), 3.0);
        assert_eq!(v(&s, "D6"), Value::Error(gridcraft_core::CellError::NA));
        assert!(close(n(&s, "D7"), 2.5f64.sqrt(), 1e-12));
        assert_eq!(n(&s, "D8"), 2.5);
        assert!(close(n(&s, "D9"), -1.2, 1e-12));
        assert!(close(n(&s, "D10"), 0.0, 1e-12));
        assert_eq!((n(&s, "D11"), n(&s, "D12"), n(&s, "D13"), n(&s, "D14"), n(&s, "D15")), (4.0, 1.0, 5.0, 15.0, 5.0));
        assert_eq!(v(&s, "C16"), Value::text("Largest(2)"));
        assert_eq!(n(&s, "D16"), 4.0);
        assert_eq!(n(&s, "D17"), 1.0);
        assert_eq!(v(&s, "C18"), Value::text("Confidence Level(95.0%)"));
        let want = eval(&mut s, "=CONFIDENCE.T(0.05,STDEV.S(A2:A6),5)");
        assert!(close(n(&s, "D18"), want, 1e-9), "{} vs {want}", n(&s, "D18"));
        // The output range now holds data: a second run is refused unless it may overwrite.
        let again = json!({"tool": "descriptive", "range": "A2:A6", "output": "C1"});
        let e = s.execute("data.analysisToolPak", again.clone()).unwrap_err().to_string();
        assert!(e.contains(super::OVERWRITE_ERROR), "{e}");
        let mut again = again;
        again["overwrite"] = json!(true);
        s.execute("data.analysisToolPak", again).unwrap();
        assert_eq!(v(&s, "C1"), Value::text("Column1"));
    }

    #[test]
    fn regression_matches_textbook_and_linest() {
        // x = 1..5, y = 2,4,5,4,5: b = 0.6, a = 2.2, R² = 0.6, F = 4.5 (textbook example).
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [["y", "x"], [2, 1], [4, 2], [5, 3], [4, 4], [5, 5]]})).unwrap();
        s.execute(
            "data.analysisToolPak",
            json!({"tool": "regression", "inputY": "A1:A6", "inputX": "B1:B6", "labels": true, "newSheet": "Fit", "residuals": true, "standardizedResiduals": true}),
        )
        .unwrap();
        assert!(close(n(&s, "Fit!B5"), 0.6, 1e-12), "R²");
        assert!(close(n(&s, "Fit!B6"), 1.0 - 0.4 * 4.0 / 3.0, 1e-12), "adjusted R²");
        assert!(close(n(&s, "Fit!B7"), 0.8f64.sqrt(), 1e-12), "standard error");
        assert!(close(n(&s, "Fit!E12"), 4.5, 1e-12), "F");
        assert!(close(n(&s, "Fit!B17"), 2.2, 1e-12), "intercept");
        assert!(close(n(&s, "Fit!B18"), 0.6, 1e-12), "slope");
        assert_eq!(v(&s, "Fit!A18"), Value::text("x"));
        assert!(close(n(&s, "Fit!C18"), (0.8f64 / 10.0).sqrt(), 1e-12), "SE of slope");
        // Simple regression: Significance F equals the slope's p-value.
        assert!(close(n(&s, "Fit!F12"), n(&s, "Fit!E18"), 1e-9));
        assert!(n(&s, "Fit!E18") > 0.12 && n(&s, "Fit!E18") < 0.13);
        assert_eq!(v(&s, "Fit!A22"), Value::text("RESIDUAL OUTPUT"));
        assert!(close(n(&s, "Fit!B25"), 2.8, 1e-12), "first prediction");
        assert!(close(n(&s, "Fit!C25"), -0.8, 1e-12), "first residual");
        // Multiple regression agrees with LINEST.
        s.execute(
            "range.setValues",
            json!({"range": "Sheet1!D1", "values": [[1, 2, 3], [3, 1, 5], [4, 5, 2], [7, 3, 4], [6, 6, 1], [9, 4, 6], [8, 7, 3]]}),
        )
        .unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "regression", "inputY": "Sheet1!D1:D7", "inputX": "Sheet1!E1:F7", "newSheet": "Multi"}))
            .unwrap();
        let slope2 = eval(&mut s, "=INDEX(LINEST(Sheet1!D1:D7,Sheet1!E1:F7,TRUE,TRUE),1,1)");
        let slope1 = eval(&mut s, "=INDEX(LINEST(Sheet1!D1:D7,Sheet1!E1:F7,TRUE,TRUE),1,2)");
        let se1 = eval(&mut s, "=INDEX(LINEST(Sheet1!D1:D7,Sheet1!E1:F7,TRUE,TRUE),2,2)");
        let icpt = eval(&mut s, "=INDEX(LINEST(Sheet1!D1:D7,Sheet1!E1:F7,TRUE,TRUE),1,3)");
        assert!(close(n(&s, "Multi!B17"), icpt, 1e-9));
        assert!(close(n(&s, "Multi!B18"), slope1, 1e-9));
        assert!(close(n(&s, "Multi!B19"), slope2, 1e-9));
        assert!(close(n(&s, "Multi!C18"), se1, 1e-9));
        assert_eq!(v(&s, "Multi!A19"), Value::text("X Variable 2"));
    }

    #[test]
    fn tests_of_means_and_variances_match_worksheet_functions() {
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [[12, 10], [15, 14], [11, 9], [18, 15], [14, 13], [16, 12]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "tTestEqual", "range1": "A1:A6", "range2": "B1:B6", "output": "D1"})).unwrap();
        assert_eq!(v(&s, "D1"), Value::text("t-Test: Two-Sample Assuming Equal Variances"));
        assert_eq!(n(&s, "E9"), 10.0, "df");
        let p2 = eval(&mut s, "=T.TEST(A1:A6,B1:B6,2,2)");
        assert!(close(n(&s, "E13"), p2, 1e-9), "two-tail p");
        s.execute("data.analysisToolPak", json!({"tool": "tTestPaired", "range1": "A1:A6", "range2": "B1:B6", "output": "H1"})).unwrap();
        let pp = eval(&mut s, "=T.TEST(A1:A6,B1:B6,2,1)");
        assert_eq!(v(&s, "H13"), Value::text("P(T<=t) two-tail"));
        assert!(close(n(&s, "I13"), pp, 1e-9), "paired two-tail p");
        s.execute("data.analysisToolPak", json!({"tool": "fTest", "range1": "A1:A6", "range2": "B1:B6", "output": "L1"})).unwrap();
        let fp = eval(&mut s, "=F.TEST(A1:A6,B1:B6)");
        assert!(close(n(&s, "M9") * 2.0, fp, 1e-9), "one-tail p is half of F.TEST");
        // Anova on {1,2,3}, {4,5,6}, {7,8,9}: SSB 54, SSW 6, F 27.
        s.execute("range.setValues", json!({"range": "A20", "values": [[1, 4, 7], [2, 5, 8], [3, 6, 9]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "anovaSingle", "range": "A20:C22", "output": "E20"})).unwrap();
        assert_eq!(v(&s, "E31"), Value::text("Between Groups"));
        assert_eq!((n(&s, "F31"), n(&s, "F32"), n(&s, "I31")), (54.0, 6.0, 27.0));
        let p = eval(&mut s, "=F.DIST.RT(27,2,6)");
        assert!(close(n(&s, "J31"), p, 1e-12));
    }

    #[test]
    fn histogram_matrices_smoothing_and_ranks() {
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1], [2], [2], [3], [5], [7]]})).unwrap();
        s.execute("range.setValues", json!({"range": "B1", "values": [[2], [4], [6]]})).unwrap();
        s.execute(
            "data.analysisToolPak",
            json!({"tool": "histogram", "range": "A1:A6", "bins": "B1:B3", "output": "D1", "cumulative": true, "pareto": true}),
        )
        .unwrap();
        assert_eq!((n(&s, "E2"), n(&s, "E3"), n(&s, "E4"), n(&s, "E5")), (3.0, 1.0, 1.0, 1.0));
        assert_eq!(v(&s, "D5"), Value::text("More"));
        assert_eq!(n(&s, "F5"), 1.0);
        assert_eq!((n(&s, "G2"), n(&s, "H2")), (2.0, 3.0), "Pareto puts the largest bin first");
        // Correlation and covariance against CORREL and COVARIANCE.P.
        s.execute("range.setValues", json!({"range": "J1", "values": [[1, 2], [2, 4], [3, 5], [4, 4], [5, 5]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "correlation", "range": "J1:K5", "output": "M1"})).unwrap();
        assert_eq!(n(&s, "N2"), 1.0);
        assert!(close(n(&s, "N3"), eval(&mut s, "=CORREL(J1:J5,K1:K5)"), 1e-12));
        assert!(v(&s, "O2").is_empty(), "upper triangle is blank");
        s.execute("data.analysisToolPak", json!({"tool": "covariance", "range": "J1:K5", "output": "M10"})).unwrap();
        assert!(close(n(&s, "N12"), eval(&mut s, "=COVARIANCE.P(J1:J5,K1:K5)"), 1e-12));
        assert!(close(n(&s, "N11"), eval(&mut s, "=VAR.P(J1:J5)"), 1e-12));
        // Moving average of 3 and exponential smoothing with damping 0.3.
        s.execute("data.analysisToolPak", json!({"tool": "movingAverage", "range": "K1:K5", "output": "Q1", "standardErrors": true})).unwrap();
        assert_eq!(v(&s, "Q2"), Value::Error(gridcraft_core::CellError::NA));
        assert!(close(n(&s, "Q3"), 11.0 / 3.0, 1e-12));
        assert!(close(n(&s, "Q5"), 14.0 / 3.0, 1e-12));
        assert_eq!(v(&s, "R4"), Value::Error(gridcraft_core::CellError::NA));
        assert!(n(&s, "R5").is_finite());
        s.execute("data.analysisToolPak", json!({"tool": "exponentialSmoothing", "range": "K1:K5", "output": "S1"})).unwrap();
        assert_eq!(n(&s, "S2"), 2.0);
        assert!(close(n(&s, "S3"), 0.7 * 4.0 + 0.3 * 2.0, 1e-12));
        // Rank and Percentile of 2,4,5,4,5: the two 5s share rank 1 (3 of 4 others below them).
        s.execute("data.analysisToolPak", json!({"tool": "rankPercentile", "range": "K1:K5", "output": "U1"})).unwrap();
        assert_eq!((n(&s, "U2"), n(&s, "V2"), n(&s, "W2"), n(&s, "X2")), (3.0, 5.0, 1.0, 0.75));
        assert_eq!((n(&s, "U3"), n(&s, "W3")), (5.0, 1.0));
        assert_eq!((n(&s, "V6"), n(&s, "X6")), (2.0, 0.0));
        assert_eq!(n(&s, "X4"), 0.25, "4 has one value below it among five");
    }

    #[test]
    fn two_factor_anova_without_replication_matches_a_textbook_example() {
        // Hardness testing, a randomized complete block design (Montgomery, Design and Analysis
        // of Experiments, Example 4.1): four tips (rows) by four coupons (columns). Published:
        // SS tips 0.385, SS coupons 0.825, SS error 0.080, SS total 1.290, F tips 14.44.
        let mut s = s();
        s.execute(
            "range.setValues",
            json!({"range": "A1", "values": [
                ["", "Coupon 1", "Coupon 2", "Coupon 3", "Coupon 4"],
                ["Tip 1", 9.3, 9.4, 9.6, 10.0],
                ["Tip 2", 9.4, 9.3, 9.8, 9.9],
                ["Tip 3", 9.2, 9.4, 9.5, 9.7],
                ["Tip 4", 9.7, 9.6, 10.0, 10.2],
            ]}),
        )
        .unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "Anova: Two-Factor Without Replication", "range": "A1:E5", "labels": true, "output": "H1"}))
            .unwrap();
        assert_eq!(v(&s, "H1"), Value::text("Anova: Two-Factor Without Replication"));
        assert_eq!((v(&s, "H4"), n(&s, "I4")), (Value::text("Tip 1"), 4.0));
        assert!(close(n(&s, "J4"), 38.3, 1e-12), "sum of tip 1");
        assert_eq!(v(&s, "H9"), Value::text("Coupon 1"));
        assert!(close(n(&s, "K12"), 9.95, 1e-12), "average of coupon 4");
        assert_eq!(v(&s, "H17"), Value::text("Rows"));
        assert!(close(n(&s, "I17"), 0.385, 1e-12));
        assert!(close(n(&s, "I18"), 0.825, 1e-12));
        assert!(close(n(&s, "I19"), 0.080, 1e-12));
        assert!(close(n(&s, "I21"), 1.290, 1e-12));
        assert_eq!((n(&s, "J17"), n(&s, "J19"), n(&s, "J21")), (3.0, 9.0, 15.0));
        assert!(close(n(&s, "L17"), 14.4375, 1e-9), "F for tips");
        assert!(close(n(&s, "M17"), eval(&mut s, "=F.DIST.RT(14.4375,3,9)"), 1e-9));
        assert!(close(n(&s, "N17"), eval(&mut s, "=F.INV.RT(0.05,3,9)"), 1e-9));
        assert!(v(&s, "L19").is_empty(), "the error line has no F");
    }

    #[test]
    fn two_factor_anova_with_replication_matches_a_textbook_example() {
        let mut s = s();
        // Battery life (Montgomery, Example 5.1): three materials (samples of four rows) by three
        // temperatures (columns). Published: SS material 10683.72, SS temperature 39118.72,
        // SS interaction 9613.78, SS error 18230.75 on 27 df, SS total 77646.97.
        s.execute(
            "range.setValues",
            json!({"range": "A20", "values": [
                ["Material", "15 F", "70 F", "125 F"],
                ["Type 1", 130, 34, 20], ["", 155, 40, 70], ["", 74, 80, 82], ["", 180, 75, 58],
                ["Type 2", 150, 136, 25], ["", 188, 122, 70], ["", 159, 106, 58], ["", 126, 115, 45],
                ["Type 3", 138, 174, 96], ["", 110, 120, 104], ["", 168, 150, 82], ["", 160, 139, 60],
            ]}),
        )
        .unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "anovaTwoFactorWith", "range": "A20:D32", "rowsPerSample": 4, "newSheet": "Battery"}))
            .unwrap();
        assert_eq!(v(&s, "Battery!A3"), Value::text("SUMMARY"));
        assert_eq!((v(&s, "Battery!B3"), v(&s, "Battery!E3")), (Value::text("15 F"), Value::text("Total")));
        assert_eq!(v(&s, "Battery!A4"), Value::text("Type 1"));
        assert_eq!((n(&s, "Battery!B5"), n(&s, "Battery!B6"), n(&s, "Battery!E5")), (4.0, 539.0, 12.0));
        assert_eq!(v(&s, "Battery!A22"), Value::text("Total"));
        assert_eq!(n(&s, "Battery!B24"), 1738.0, "sum of the 15 F column");
        assert_eq!(v(&s, "Battery!A29"), Value::text("ANOVA"));
        assert_eq!(
            (v(&s, "Battery!A31"), v(&s, "Battery!A34"), v(&s, "Battery!A36")),
            (Value::text("Sample"), Value::text("Within"), Value::text("Total"))
        );
        assert!(close(n(&s, "Battery!B31"), 10683.72, 0.01));
        assert!(close(n(&s, "Battery!B32"), 39118.72, 0.01));
        assert!(close(n(&s, "Battery!B33"), 9613.78, 0.01));
        assert!(close(n(&s, "Battery!B34"), 18230.75, 0.01));
        assert!(close(n(&s, "Battery!B36"), 77646.97, 0.01));
        assert_eq!((n(&s, "Battery!C33"), n(&s, "Battery!C34"), n(&s, "Battery!C36")), (4.0, 27.0, 35.0));
        assert!(close(n(&s, "Battery!E31"), 7.91, 0.005) && close(n(&s, "Battery!E32"), 28.97, 0.005) && close(n(&s, "Battery!E33"), 3.56, 0.005));
        assert!(close(n(&s, "Battery!F33"), 0.0186, 0.00005), "interaction P-value");
        // Rows per sample must divide the data rows.
        let bad = json!({"tool": "anovaTwoFactorWith", "range": "Sheet1!A20:D32", "rowsPerSample": 5, "output": "F60"});
        assert!(s.execute("data.analysisToolPak", bad).is_err());
    }

    #[test]
    fn z_test_with_known_variances() {
        // x̄ = 3 (n = 5, σ² = 2), ȳ = 4 (n = 3, σ² = 6): z = −1/√2.4. Critical values from the
        // standard normal table: 1.644854 (one tail) and 1.959964 (two tails) at α = 0.05.
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [["x", "y"], [1, 2], [2, 4], [3, 6], [4, null], [5, null]]})).unwrap();
        s.execute(
            "data.analysisToolPak",
            json!({"tool": "zTest", "range1": "A1:A6", "range2": "B1:B6", "labels": true, "variance1": 2, "variance2": 6, "output": "D1"}),
        )
        .unwrap();
        assert_eq!(v(&s, "D1"), Value::text("z-Test: Two Sample for Means"));
        assert_eq!((v(&s, "E3"), v(&s, "F3")), (Value::text("x"), Value::text("y")));
        assert_eq!((n(&s, "E4"), n(&s, "F4"), n(&s, "E5"), n(&s, "F6")), (3.0, 4.0, 2.0, 3.0));
        let z = -1.0 / 2.4f64.sqrt();
        assert!(close(n(&s, "E8"), z, 1e-12));
        let tail = 1.0 - eval(&mut s, &format!("=NORM.S.DIST({},TRUE)", -z));
        assert!(close(n(&s, "E9"), tail, 1e-12) && close(n(&s, "E9"), 0.2593, 1e-4));
        assert!(close(n(&s, "E10"), 1.644_853_6, 1e-6));
        assert!(close(n(&s, "E11"), 2.0 * tail, 1e-12));
        assert!(close(n(&s, "E12"), 1.959_964_0, 1e-6));
        // Unknown or non-positive variances are refused.
        assert!(s.execute("data.analysisToolPak", json!({"tool": "zTest", "range1": "A2:A6", "range2": "B2:B4", "output": "H1"})).is_err());
        let zero = json!({"tool": "zTest", "range1": "A2:A6", "range2": "B2:B4", "variance1": 0, "variance2": 1, "output": "H1"});
        assert!(s.execute("data.analysisToolPak", zero).is_err());
    }

    #[test]
    fn fourier_analysis_matches_known_transforms() {
        let mut s = s();
        // DFT of 1, 2, 3, 4: 10, −2+2i, −2, −2−2i.
        s.execute("range.setValues", json!({"range": "A1", "values": [["Signal"], [1], [2], [3], [4]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "Fourier Analysis", "range": "A1:A5", "labels": true, "output": "C1"})).unwrap();
        let col = |s: &Session, c: &str, rows: std::ops::RangeInclusive<u32>| rows.map(|r| v(s, &format!("{c}{r}"))).collect::<Vec<_>>();
        assert_eq!(col(&s, "C", 1..=5), ["Signal", "10", "-2+2i", "-2", "-2-2i"].map(Value::text).to_vec());
        // The inverse transform of that output returns the signal.
        s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "C2:C5", "inverse": true, "output": "E1"})).unwrap();
        assert_eq!(col(&s, "E", 1..=4), ["1", "2", "3", "4"].map(Value::text).to_vec());
        // cos(2πn/8) over eight points: N/2 = 4 at the first and last frequencies, 0 elsewhere.
        let wave: Vec<Json> = (0..8).map(|k| json!([(std::f64::consts::PI * k as f64 / 4.0).cos()])).collect();
        s.execute("range.setValues", json!({"range": "G1", "values": wave})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "G1:G8", "output": "H1"})).unwrap();
        assert_eq!(col(&s, "H", 1..=8), ["0", "4", "0", "0", "0", "0", "0", "4"].map(Value::text).to_vec());
        // Complex text input; a single value is its own transform.
        s.execute("range.setValues", json!({"range": "J1", "values": [["3+4i"], ["1-i"]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "J1:J2", "output": "K1"})).unwrap();
        assert_eq!(col(&s, "K", 1..=2), ["4+3i", "2+5i"].map(Value::text).to_vec());
        s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "J1", "output": "L1"})).unwrap();
        assert_eq!(v(&s, "L1"), Value::text("3+4i"));
        // Three values are not a power of two; a word is not a number.
        assert!(s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "A2:A4", "output": "N1"})).is_err());
        assert!(s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "A1:A4", "output": "N1"})).is_err());
        assert!(s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "A1:A8192", "output": "N1"})).is_err());
    }

    #[test]
    fn fourier_analysis_keeps_a_small_imaginary_part_beside_a_large_real_one() {
        let mut s = s();
        // With one value the transform is the identity and performs no arithmetic, so no part of
        // the output is rounding noise: 1e14 + 1i comes back as 1e14 + 1i, forward and inverse.
        s.execute("range.setValues", json!({"range": "A1", "values": [["100000000000000+1i"]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "A1", "output": "C1"})).unwrap();
        assert_eq!(eval(&mut s, "=IMREAL(C1)"), 1e14);
        assert_eq!(eval(&mut s, "=IMAGINARY(C1)"), 1.0);
        s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "C1", "inverse": true, "output": "E1"})).unwrap();
        assert_eq!(eval(&mut s, "=IMREAL(E1)"), 1e14);
        assert_eq!(eval(&mut s, "=IMAGINARY(E1)"), 1.0);
    }

    #[test]
    fn fourier_analysis_round_trip_recovers_small_components_beside_a_large_dc() {
        let mut s = s();
        // x_n = 1e6 + 1e-7·cos(2πn/8) for n = 0 … 7. The transform of a constant c over N points is
        // N·c at k = 0 and 0 elsewhere; the transform of a·cos(2πn/N) is a·N/2 at k = 1 and
        // k = N − 1. So X_0 = 8·1e6 = 8e6, X_1 = X_7 = 1e-7·8/2 = 4e-7 and X_2 … X_6 = 0. The inputs
        // are stored to within ulp(1e6)/2 ≈ 6e-11 and the first butterfly stage subtracts nearby
        // numbers exactly, so the harmonics carry at most about 5e-10 of noise. The cleanup bound
        // 8·ε·log2(8)·Σ|x_n| ≈ 8 · 2.2e-16 · 3 · 8e6 ≈ 4.3e-8 zeroes noise of that size and keeps
        // 4e-7; a cutoff relative to the largest output (8e6 · 1e-13 = 8e-7) would zero the
        // harmonics.
        let x = |k: usize| 1e6 + 1e-7 * (std::f64::consts::PI * k as f64 / 4.0).cos();
        let wave: Vec<Json> = (0..8).map(|k| json!([x(k)])).collect();
        s.execute("range.setValues", json!({"range": "A1", "values": wave})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "A1:A8", "output": "C1"})).unwrap();
        assert!(close(eval(&mut s, "=IMREAL(C1)"), 8e6, 1e-6));
        assert_eq!(eval(&mut s, "=IMAGINARY(C1)"), 0.0);
        for cell in ["C2", "C8"] {
            assert!(close(eval(&mut s, &format!("=IMREAL({cell})")), 4e-7, 5e-9), "{cell}");
            assert_eq!(eval(&mut s, &format!("=IMAGINARY({cell})")), 0.0, "{cell}");
        }
        for r in 3..=7 {
            assert_eq!(v(&s, &format!("C{r}")), Value::text("0"), "C{r}");
        }
        // The inverse of that output returns each x_n to within 2e-9 (2e-15 relative); without the
        // harmonics every value would come back as 1e6, off by up to 1e-7.
        s.execute("data.analysisToolPak", json!({"tool": "fourierAnalysis", "range": "C1:C8", "inverse": true, "output": "E1"})).unwrap();
        for k in 0..8 {
            let cell = format!("E{}", k + 1);
            assert!(close(eval(&mut s, &format!("=IMREAL({cell})")), x(k), 2e-9), "{cell}");
            assert_eq!(eval(&mut s, &format!("=IMAGINARY({cell})")), 0.0, "{cell}");
        }
    }

    /// Column `col` of sheet `sheet`, rows 1 to `rows`, as numbers.
    fn numbers_of(s: &Session, sheet: &str, col: &str, rows: u32) -> Vec<f64> {
        (1..=rows).map(|r| n(s, &format!("{sheet}!{col}{r}"))).collect()
    }
    fn mean_var(x: &[f64]) -> (f64, f64) {
        let m = x.iter().sum::<f64>() / x.len() as f64;
        (m, x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (x.len() - 1) as f64)
    }

    #[test]
    fn random_numbers_are_seeded_and_follow_their_distributions() {
        let mut s = s();
        let rng = |s: &mut Session, sheet: &str, p: Json| {
            let mut p = p;
            p["tool"] = json!("randomNumberGeneration");
            p["newSheet"] = json!(sheet);
            s.execute("data.analysisToolPak", p).unwrap()
        };
        // The same seed repeats the numbers; the result reports the seed.
        let r = rng(&mut s, "U1", json!({"variables": 2, "count": 2000, "distribution": "uniform", "from": 2, "to": 4, "seed": 42}));
        assert_eq!((r["seed"].as_u64(), r["range"].as_str()), (Some(42), Some("A1:B2000")));
        rng(&mut s, "U2", json!({"variables": 2, "count": 2000, "distribution": "uniform", "from": 2, "to": 4, "seed": 42}));
        let (a, b) = (numbers_of(&s, "U1", "B", 2000), numbers_of(&s, "U2", "B", 2000));
        assert_eq!(a, b);
        assert!(a.iter().all(|x| (2.0..4.0).contains(x)));
        // Uniform on [2, 4]: mean 3, variance 1/3.
        let (m, var) = mean_var(&a);
        assert!((m - 3.0).abs() < 0.05 && (var - 1.0 / 3.0).abs() < 0.03, "{m} {var}");
        let unseeded = rng(&mut s, "U3", json!({"count": 1}));
        assert!(unseeded["seed"].as_u64().is_some(), "a seed is chosen and reported");
        // Normal(10, 2): mean 10, variance 4.
        rng(&mut s, "N", json!({"count": 20000, "distribution": "normal", "mean": 10, "standardDeviation": 2, "seed": 7}));
        let (m, var) = mean_var(&numbers_of(&s, "N", "A", 20000));
        assert!((m - 10.0).abs() < 0.05 && (var - 4.0).abs() < 0.15, "{m} {var}");
        // Binomial(20, 0.3): mean np = 6, variance np(1 − p) = 4.2.
        rng(&mut s, "B", json!({"count": 20000, "distribution": "binomial", "p": 0.3, "trials": 20, "seed": 8}));
        let x = numbers_of(&s, "B", "A", 20000);
        assert!(x.iter().all(|v| v.fract() == 0.0 && (0.0..=20.0).contains(v)));
        let (m, var) = mean_var(&x);
        assert!((m - 6.0).abs() < 0.08 && (var - 4.2).abs() < 0.2, "{m} {var}");
        // Poisson(3.5): mean and variance 3.5.
        rng(&mut s, "P", json!({"count": 20000, "distribution": "poisson", "lambda": 3.5, "seed": 9}));
        let (m, var) = mean_var(&numbers_of(&s, "P", "A", 20000));
        assert!((m - 3.5).abs() < 0.08 && (var - 3.5).abs() < 0.2, "{m} {var}");
        // Bernoulli(0.25) and a discrete distribution on {1, 2} with P(2) = 0.75.
        rng(&mut s, "Ber", json!({"count": 4000, "distribution": "bernoulli", "p": 0.25, "seed": 10}));
        let ones = numbers_of(&s, "Ber", "A", 4000).iter().filter(|v| **v == 1.0).count();
        assert!((900..1100).contains(&ones), "{ones}");
        s.execute("range.setValues", json!({"range": "Sheet1!A1", "values": [[1, 0.25], [2, 0.75]]})).unwrap();
        rng(&mut s, "D", json!({"count": 4000, "distribution": "discrete", "valueRange": "Sheet1!A1:B2", "seed": 11}));
        let x = numbers_of(&s, "D", "A", 4000);
        assert!(x.iter().all(|v| *v == 1.0 || *v == 2.0));
        let twos = x.iter().filter(|v| **v == 2.0).count();
        assert!((2900..3100).contains(&twos), "{twos}");
        // Patterned: 1 to 3, each number twice, the sequence twice.
        rng(&mut s, "Pat", json!({"distribution": "patterned", "from": 1, "to": 3, "step": 1, "repeatEach": 2, "repeatSequence": 2}));
        assert_eq!(numbers_of(&s, "Pat", "A", 12), [1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0]);
        assert!(v(&s, "Pat!A13").is_empty());
        // Output is bounded; parameters are checked.
        for p in [
            json!({"count": 2_000_000}),
            json!({"count": 1000, "variables": 2000}),
            json!({"count": 0}),
            json!({"count": 1.5}),
            json!({"distribution": "uniform"}),
            json!({"count": 1, "distribution": "bernoulli", "p": 2}),
            json!({"count": 1, "distribution": "binomial", "trials": 1e9}),
            json!({"count": 1, "distribution": "poisson", "lambda": -1}),
            json!({"count": 1, "distribution": "normal", "standardDeviation": -1}),
            json!({"count": 1, "distribution": "discrete", "valueRange": "Sheet1!A1:A2"}),
            json!({"count": 1, "distribution": "patterned", "from": 1, "to": 1e9, "step": 1}),
            json!({"count": 1, "distribution": "patterned", "from": 3, "to": 1}),
            json!({"count": 1, "distribution": "weird"}),
            json!({"count": 1, "seed": -1}),
            json!({"count": 1, "seed": "abc"}),
        ] {
            let mut q = p.clone();
            q["tool"] = json!("randomNumberGeneration");
            q["output"] = json!("Sheet1!H1");
            assert!(s.execute("data.analysisToolPak", q).is_err(), "{p}");
        }
        s.execute("range.setValues", json!({"range": "Sheet1!A1", "values": [[1, 0.5], [2, 0.6]]})).unwrap();
        let p =
            json!({"tool": "randomNumberGeneration", "count": 1, "distribution": "discrete", "valueRange": "Sheet1!A1:B2", "output": "Sheet1!H1"});
        assert!(s.execute("data.analysisToolPak", p).is_err(), "probabilities must add up to 1");
    }

    #[test]
    fn binomial_and_poisson_tables_agree_with_the_worksheet_functions() {
        // The recurrence outward from the mode reproduces BINOM.DIST and POISSON.DIST at every
        // tabulated outcome, the table carries the whole distribution, and it is only as wide as
        // the outcomes that matter. The worksheet functions themselves are accurate to about
        // n · ε for n trials (their logarithms are of that size), hence the moderate n and λ.
        let binom = |k: f64, n: f64, p: f64| f64_of(&call("BINOM.DIST", &[Value::Number(k), Value::Number(n), Value::Number(p), Value::Bool(false)]));
        for (n, p) in [(20.0f64, 0.3f64), (100.0, 0.5), (50.0, 0.9), (30.0, 0.01), (7.0, 0.999), (1.0, 0.5)] {
            let mode = ((n + 1.0) * p).floor().min(n);
            let odds = p / (1.0 - p);
            let (first, pmf) = pmf_table(n, mode, binom(mode, n, p), |k| (n - k) / (k + 1.0) * odds).unwrap();
            assert!(first >= 0.0 && first + pmf.len() as f64 - 1.0 <= n, "n={n} p={p}: {first} + {}", pmf.len());
            for (i, got) in pmf.iter().enumerate() {
                let k = first + i as f64;
                let want = binom(k, n, p);
                assert!(((got - want) / want).abs() < 1e-12, "n={n} p={p} k={k}: {got} vs {want}");
            }
            assert!((pmf.iter().sum::<f64>() - 1.0).abs() < 1e-12, "n={n} p={p}");
        }
        let poisson = |k: f64, lambda: f64| f64_of(&call("POISSON.DIST", &[Value::Number(k), Value::Number(lambda), Value::Bool(false)]));
        for lambda in [0.2f64, 3.5, 50.0] {
            let mode = lambda.floor();
            let (first, pmf) = pmf_table(MAX_OUTCOMES, mode, poisson(mode, lambda), |k| lambda / (k + 1.0)).unwrap();
            assert_eq!(first, 0.0);
            for (i, got) in pmf.iter().enumerate() {
                let want = poisson(i as f64, lambda);
                assert!(((got - want) / want).abs() < 1e-12, "lambda={lambda} k={i}: {got} vs {want}");
            }
            assert!((pmf.iter().sum::<f64>() - 1.0).abs() < 1e-12, "lambda={lambda}");
        }
        // Binomial(1e6, 0.5): standard deviation 500, so the terms vanish within about 38
        // standard deviations below the mode and the sum reaches 1 within about 8 above it:
        // roughly 23,000 entries starting near 481,000, not 1,000,001 starting at 0.
        let (n, p) = (1_000_000.0, 0.5);
        let (first, pmf) = pmf_table(n, 500_000.0, binom(500_000.0, n, p), |k| (n - k) / (k + 1.0)).unwrap();
        assert!((470_000.0..=490_000.0).contains(&first) && pmf.len() < 40_000, "{first} + {}", pmf.len());
        // The anchor BINOM.DIST(500000, 1e6, 0.5) carries that function's error of about n · ε
        // (its logarithms are near 7e5), which scales the whole table; draws divide by the total.
        assert!((pmf.iter().sum::<f64>() - 1.0).abs() < 1e-8);
        // A bad anchor or mode is refused rather than tabulated.
        assert!(pmf_table(10.0, 5.0, f64::NAN, |_| 1.0).is_err());
        assert!(pmf_table(10.0, 5.0, 0.0, |_| 1.0).is_err());
        assert!(pmf_table(10.0, 11.0, 0.5, |_| 1.0).is_err());
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // a native-only timing test: the clock is read off wasm
    fn binomial_and_poisson_draws_at_the_limits_are_tabulated_quickly() {
        let mut s = s();
        // The tables cover the outcomes around the mode, not every one of the 1,000,001 for a
        // million trials, so the largest inputs cost milliseconds. Binomial(1e6, 0.5) has mean
        // 500000 and standard deviation 500: 1000 draws put the sample mean within 500/√1000 ≈ 16
        // of it, so 100 is six standard errors. Poisson(1e5) has mean 1e5 and standard deviation
        // 316: 1000 draws put the mean within 10, so 60 is six standard errors. With p = 1 every
        // draw is exactly 1e6, with p = 0 exactly 0.
        let mut draw = |sheet: &str, dist: Json| {
            let mut dist = dist;
            dist["tool"] = json!("randomNumberGeneration");
            dist["count"] = json!(1000);
            dist["seed"] = json!(3);
            dist["newSheet"] = json!(sheet);
            s.execute("data.analysisToolPak", dist).unwrap();
        };
        let start = std::time::Instant::now();
        draw("Half", json!({"distribution": "binomial", "p": 0.5, "trials": 1_000_000}));
        draw("Sure", json!({"distribution": "binomial", "p": 1, "trials": 1_000_000}));
        draw("Never", json!({"distribution": "binomial", "p": 0, "trials": 1_000_000}));
        draw("Pois", json!({"distribution": "poisson", "lambda": 100_000}));
        let elapsed = start.elapsed();
        let (m, _) = mean_var(&numbers_of(&s, "Half", "A", 1000));
        assert!((m - 500_000.0).abs() < 100.0, "{m}");
        assert!(numbers_of(&s, "Sure", "A", 1000).iter().all(|v| *v == 1_000_000.0));
        assert!(numbers_of(&s, "Never", "A", 1000).iter().all(|v| *v == 0.0));
        let (m, _) = mean_var(&numbers_of(&s, "Pois", "A", 1000));
        assert!((m - 100_000.0).abs() < 60.0, "{m}");
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    fn sampling_periodic_and_random() {
        let mut s = s();
        let values: Vec<Json> = (1..=10).map(|k| json!([k])).collect();
        s.execute("range.setValues", json!({"range": "A1", "values": values})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "Sampling", "range": "A1:A10", "period": 3, "output": "C1"})).unwrap();
        assert_eq!(numbers_of(&s, "Sheet1", "C", 3), [3.0, 6.0, 9.0]);
        assert!(v(&s, "C4").is_empty());
        let r = s
            .execute(
                "data.analysisToolPak",
                json!({"tool": "sampling", "range": "A1:A10", "method": "random", "samples": 20, "seed": 5, "output": "D1"}),
            )
            .unwrap();
        assert_eq!(r["seed"], 5);
        s.execute(
            "data.analysisToolPak",
            json!({"tool": "sampling", "range": "A1:A10", "method": "random", "samples": 20, "seed": 5, "output": "E1"}),
        )
        .unwrap();
        let (a, b) = (numbers_of(&s, "Sheet1", "D", 20), numbers_of(&s, "Sheet1", "E", 20));
        assert_eq!(a, b, "the same seed repeats the sample");
        assert!(a.iter().all(|x| x.fract() == 0.0 && (1.0..=10.0).contains(x)));
        for p in [
            json!({"range": "A1:A10", "period": 11}),
            json!({"range": "A1:A10", "period": 0}),
            json!({"range": "A1:A10"}),
            json!({"range": "A1:A10", "method": "random", "samples": 2_000_000}),
            json!({"range": "B1:B10", "period": 1}),
        ] {
            let mut q = p.clone();
            q["tool"] = json!("sampling");
            q["output"] = json!("G1");
            assert!(s.execute("data.analysisToolPak", q).is_err(), "{p}");
        }
    }

    #[test]
    fn options_typed_as_text_that_is_not_a_number_are_refused() {
        // A form sends its options as text; one that doesn't read as a number (a decimal comma,
        // a typo) is an error, not the default, while a blank option takes the default.
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, 2], [2, 4], [3, 6]]})).unwrap();
        let rng = |dist: &str| json!({"tool": "randomNumberGeneration", "count": 3, "distribution": dist, "seed": 1, "outputTo": "newSheet"});
        let z = json!({"tool": "zTest", "range1": "Sheet1!A1:A3", "range2": "Sheet1!B1:B3", "variance1": 1, "variance2": 4, "outputTo": "newSheet"});
        let with = |base: &Json, key: &str, val: &str| {
            let mut p = base.clone();
            p[key] = json!(val);
            p
        };
        let cases = [
            (rng("uniform"), "from"),
            (rng("uniform"), "to"),
            (rng("normal"), "mean"),
            (rng("normal"), "standardDeviation"),
            (rng("bernoulli"), "p"),
            (rng("binomial"), "p"),
            (rng("poisson"), "lambda"),
            (rng("patterned"), "step"),
            (rng("uniform"), "variables"),
            (z.clone(), "hypothesizedDifference"),
        ];
        for (base, key) in cases {
            let e = s.execute("data.analysisToolPak", with(&base, key, "0,3")).unwrap_err().to_string();
            assert!(e.contains(key), "{key}: {e}");
            s.execute("data.analysisToolPak", with(&base, key, " ")).unwrap_or_else(|e| panic!("{key} blank: {e}"));
        }
    }

    #[test]
    fn an_alpha_that_is_not_a_number_is_refused() {
        // A decimal comma or a typo in Alpha must not run the test at 0.05 without a word.
        let mut s = s();
        s.execute("range.setValues", json!({"range": "A1", "values": [["", "c1", "c2"], ["r1", 1, 2], ["r1", 2, 4], ["r2", 3, 6], ["r2", 5, 9]]}))
            .unwrap();
        for base in [
            json!({"tool": "zTest", "range1": "Sheet1!B2:B5", "range2": "Sheet1!C2:C5", "variance1": 1, "variance2": 4}),
            json!({"tool": "anovaTwoFactorWithout", "range": "Sheet1!B2:C5"}),
            json!({"tool": "anovaTwoFactorWith", "range": "Sheet1!A1:C5", "rowsPerSample": 2}),
            json!({"tool": "tTestEqual", "range1": "Sheet1!B2:B5", "range2": "Sheet1!C2:C5"}),
        ] {
            let mut p = base.clone();
            p["alpha"] = json!("0,01");
            let e = s.execute("data.analysisToolPak", p).unwrap_err().to_string();
            assert!(e.contains("alpha"), "{base}: {e}");
            let mut p = base.clone();
            p["alpha"] = json!("");
            s.execute("data.analysisToolPak", p).unwrap_or_else(|e| panic!("{base} blank alpha: {e}"));
        }
    }

    #[test]
    fn a_patterned_sequence_ignores_the_seed() {
        // Patterned generation draws nothing, so a seed left in the form (even one that isn't a
        // number) neither blocks the run nor shows up in the result.
        let mut s = s();
        for seed in [json!("abc"), json!(7)] {
            let r = s
                .execute(
                    "data.analysisToolPak",
                    json!({"tool": "randomNumberGeneration", "distribution": "patterned", "from": 1, "to": 3, "seed": seed, "outputTo": "newSheet"}),
                )
                .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
            assert!(r.get("seed").is_none(), "seed {seed}: {r}");
        }
        // A random distribution still refuses the bad seed.
        let bad = json!({"tool": "randomNumberGeneration", "distribution": "uniform", "count": 2, "seed": "abc", "outputTo": "newSheet"});
        assert!(s.execute("data.analysisToolPak", bad).is_err());
    }

    #[test]
    fn hostile_inputs_are_refused_without_panicking() {
        let mut s = s();
        let r = s.execute("data.analysisToolPak", json!({})).unwrap();
        assert!(r["tools"].as_array().is_some_and(|t| t.len() == super::TOOLS.len()));
        assert!(s.take_ui_requests().iter().any(|q| matches!(q, crate::UiRequest::Dialog(n, _) if n == "analysisToolPak")));
        s.execute("range.setValues", json!({"range": "A1", "values": [[1, "x"], [2, 3]]})).unwrap();
        for p in [
            json!({"tool": "nope"}),
            json!({"tool": "descriptive"}),
            json!({"tool": "descriptive", "range": "A1:XFD1048576"}),
            json!({"tool": "descriptive", "range": "A1:B2"}),
            json!({"tool": "descriptive", "range": "A1:A2", "output": "XFD1048576"}),
            json!({"tool": "descriptive", "range": "A1:A2", "newSheet": "bad[name]"}),
            json!({"tool": "regression", "inputY": "A1:A2", "inputX": "A1:A2"}),
            json!({"tool": "regression", "inputY": "A1:A2", "inputX": "A1:Q2"}),
            json!({"tool": "movingAverage", "range": "A1:A2", "interval": 1e300}),
            json!({"tool": "movingAverage", "range": "A1:A2", "interval": -3}),
            json!({"tool": "exponentialSmoothing", "range": "A1:A2", "damping": 7}),
            json!({"tool": "histogram", "range": "C1:C3"}),
            json!({"tool": "tTestPaired", "range1": "A1", "range2": "A2"}),
            json!({"tool": "anovaSingle", "range": "A1:A2"}),
            json!({"tool": "correlation", "range": "A1:B2", "alpha": "x", "groupedBy": 5}),
            json!({"tool": "descriptive", "range": "A1:A2", "confidence": 250}),
            json!({"tool": "anovaTwoFactorWithout", "range": "A1:B2"}),
            json!({"tool": "anovaTwoFactorWithout", "range": "A1:A2"}),
            json!({"tool": "anovaTwoFactorWith", "range": "A1:B2", "rowsPerSample": 2}),
            json!({"tool": "anovaTwoFactorWith", "range": "A1:B2", "rowsPerSample": 1e300}),
            json!({"tool": "zTest", "range1": "A1", "range2": "A2", "variance1": "x", "variance2": -1}),
        ] {
            assert!(s.execute("data.analysisToolPak", p.clone()).is_err(), "{p}");
        }
        // Collinear X columns are an error, not a NaN report.
        s.execute("range.setValues", json!({"range": "D1", "values": [[1, 1, 2], [2, 2, 4], [3, 3, 6], [5, 4, 8]]})).unwrap();
        assert!(s.execute("data.analysisToolPak", json!({"tool": "regression", "inputY": "D1:D4", "inputX": "E1:F4"})).is_err());
        // Constant data gives #DIV/0! cells, not a crash.
        s.execute("range.setValues", json!({"range": "H1", "values": [[4], [4], [4]]})).unwrap();
        s.execute("data.analysisToolPak", json!({"tool": "descriptive", "range": "H1:H3", "output": "J1"})).unwrap();
        assert_eq!(v(&s, "K9"), Value::Error(gridcraft_core::CellError::Div0));
    }
}
