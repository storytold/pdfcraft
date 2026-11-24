//! GridCraft worksheet function library.
//!
//! Every function is described by a [`FnSpec`] (name, arity, category, which parameters take
//! scalars, a signature and a one-line description) and implemented as a plain Rust function over
//! already-evaluated arguments. The calculation engine looks functions up by name with
//! [`lookup`] and invokes them through [`call`], which applies Excel's automatic array lifting:
//! passing an array where a scalar is expected evaluates the function once per element.
//!
//! Special forms that need references, laziness or hidden-row information (ROW, OFFSET,
//! INDIRECT, LET, LAMBDA, SUBTOTAL, AGGREGATE, TEXT…) are evaluated by the host; this crate
//! exports [`aggregate_values`] for SUBTOTAL/AGGREGATE.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

use gridcraft_core::{Array, CellError, DateSystem, Value};

/// Builds a [`FnSpec`] whose implementation returns `Result<Value, CellError>`.
///
/// `f!(NAME, min, max, Category, scalar_args, "SIG", "description", path::to::impl)`; prefix the
/// arguments with `volatile;` for volatile functions.
macro_rules! f {
    (volatile; $name:literal, $min:expr, $max:expr, $cat:ident, $scal:expr, $sig:literal, $desc:literal, $imp:expr) => {
        $crate::FnSpec {
            name: $name,
            min_args: $min,
            max_args: $max,
            category: $crate::Category::$cat,
            volatile: true,
            scalar_args: $scal,
            value_args: &[],
            signature: $sig,
            description: $desc,
            imp: {
                fn w(a: &[$crate::Arg], c: &mut dyn $crate::Ctx) -> gridcraft_core::Value {
                    $crate::util::finish(($imp)(a, c))
                }
                w
            },
        }
    };
    ($name:literal, $min:expr, $max:expr, $cat:ident, $scal:expr, $sig:literal, $desc:literal, $imp:expr) => {
        $crate::FnSpec {
            name: $name,
            min_args: $min,
            max_args: $max,
            category: $crate::Category::$cat,
            volatile: false,
            scalar_args: $scal,
            value_args: &[],
            signature: $sig,
            description: $desc,
            imp: {
                fn w(a: &[$crate::Arg], c: &mut dyn $crate::Ctx) -> gridcraft_core::Value {
                    $crate::util::finish(($imp)(a, c))
                }
                w
            },
        }
    };
}

pub(crate) mod criteria;
pub(crate) mod database;
pub(crate) mod date;
pub(crate) mod dynamic;
pub(crate) mod engineering;
pub(crate) mod ets;
pub(crate) mod financial;
pub(crate) mod info;
pub(crate) mod lift;
pub(crate) mod lookup;
pub(crate) mod math;
pub(crate) mod special;
pub(crate) mod stat;
pub(crate) mod text;
pub(crate) mod util;

pub use ets::Timeline;
pub use stat::{aggregate_values, aggregate_values_k};

/// An evaluated argument. Ranges arrive as `Value::Array` (blank cells = `Value::Empty`) with
/// `from_ref = true`.
#[derive(Clone, Debug)]
pub struct Arg {
    pub value: Value,
    pub from_ref: bool,
    /// A written argument whose value came out blank (a lookup of an empty cell): given, not
    /// omitted, although its value is `Value::Empty`.
    pub blank_value: bool,
}

impl Arg {
    /// A literal (non-reference) argument.
    pub fn val(value: impl Into<Value>) -> Arg {
        Arg { value: value.into(), from_ref: false, blank_value: false }
    }
    /// An argument that came from a cell or range reference.
    pub fn reference(value: impl Into<Value>) -> Arg {
        Arg { value: value.into(), from_ref: true, blank_value: false }
    }
}

/// Services a function may need from the host.
pub trait Ctx {
    fn date_system(&self) -> DateSystem;
    /// Current local date-time serial.
    fn now_serial(&self) -> f64;
    /// Uniform in [0, 1).
    fn random(&mut self) -> f64;
    /// For the database functions: whether the formula in the criteria cell at (`row`, `col`) holds
    /// for `record` (a row of `db` below its labels). `None` when that cell holds no formula or the
    /// criteria didn't come from a reference.
    fn computed_criterion(&mut self, _row: usize, _col: usize, _db: &Array, _record: usize) -> Option<bool> {
        None
    }
    /// Whether the cell's work budget has run out: the database functions stop asking for computed
    /// criteria then, as every one would fail at once.
    fn exhausted(&self) -> bool {
        false
    }
    /// For GROUPBY/PIVOTBY: whether their function argument is a LAMBDA (its value is then not used).
    fn has_lambda(&self) -> bool {
        false
    }
    /// Calls that LAMBDA with `arg` (one group's values) and returns its single result; `None` when
    /// there is no lambda.
    fn call_lambda(&mut self, _arg: Value) -> Option<Value> {
        None
    }
}

pub type FnImpl = fn(&[Arg], &mut dyn Ctx) -> Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Financial,
    Logical,
    Text,
    DateTime,
    Lookup,
    MathTrig,
    Statistical,
    Engineering,
    Information,
    Database,
    Compatibility,
    Web,
    Cube,
    /// Excel's "Add-in and Automation" category (EUROCONVERT).
    AddIn,
}

pub struct FnSpec {
    /// Upper-case, without any `_xlfn.` prefix.
    pub name: &'static str,
    pub min_args: usize,
    /// `usize::MAX` for variadic functions (Excel allows 255 arguments).
    pub max_args: usize,
    pub category: Category,
    pub volatile: bool,
    /// For automatic array lifting: true for each parameter position that expects a scalar. When
    /// an argument at a scalar position is an array, the function is applied element-wise
    /// (broadcast like Excel's dynamic arrays). The last entry repeats for variadic tails; an
    /// empty slice means no lifting.
    pub scalar_args: &'static [bool],
    /// Parameter positions that take a single value and reject a reference to more than one
    /// cell with `#VALUE!` (the Analysis ToolPak functions: EOMONTH, WORKDAY, COUPDAYS, BIN2DEC…),
    /// where other scalar parameters lift the range. Arrays that aren't references still lift.
    /// Same layout as `scalar_args`; filled at registration from [`lift::value_args`].
    pub value_args: &'static [bool],
    pub signature: &'static str,
    pub description: &'static str,
    pub imp: FnImpl,
}

impl std::fmt::Debug for FnSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnSpec").field("name", &self.name).field("min_args", &self.min_args).field("max_args", &self.max_args).finish()
    }
}

impl FnSpec {
    /// Whether parameter `i` expects a scalar (and is therefore lifted over arrays).
    pub fn is_scalar_arg(&self, i: usize) -> bool {
        match self.scalar_args.get(i) {
            Some(b) => *b,
            None => self.scalar_args.last().copied().unwrap_or(false),
        }
    }

    /// Whether parameter `i` takes a single value, so a multi-cell reference there is `#VALUE!`.
    pub fn is_value_arg(&self, i: usize) -> bool {
        match self.value_args.get(i) {
            Some(b) => *b,
            None => self.value_args.last().copied().unwrap_or(false),
        }
    }

    /// Whether a call with `n` arguments is valid (Excel allows at most 255).
    pub fn accepts(&self, n: usize) -> bool {
        (self.min_args..=self.max_args).contains(&n) && n <= 255
    }
}

/// Variadic marker for `max_args`.
pub(crate) const VAR: usize = usize::MAX;

fn registry() -> &'static (Vec<FnSpec>, HashMap<&'static str, usize>) {
    static REG: OnceLock<(Vec<FnSpec>, HashMap<&str, usize>)> = OnceLock::new();
    REG.get_or_init(|| {
        let mut v: Vec<FnSpec> = Vec::new();
        for list in [
            math::specs(),
            stat::specs(),
            ets::specs(),
            text::specs(),
            date::specs(),
            lookup::specs(),
            dynamic::specs(),
            financial::specs(),
            info::specs(),
            engineering::specs(),
            database::specs(),
        ] {
            v.extend(list);
        }
        for s in &mut v {
            s.value_args = lift::value_args(s.name);
        }
        let mut map = HashMap::with_capacity(v.len());
        for (i, s) in v.iter().enumerate() {
            map.entry(s.name).or_insert(i);
        }
        (v, map)
    })
}

/// Looks a function up by name (case-insensitive; `_xlfn.`- and `_xlws.`-free names). A
/// `_xludf.` name is user-defined and never found.
pub fn lookup(name: &str) -> Option<&'static FnSpec> {
    let mut n = name.trim().to_ascii_uppercase();
    loop {
        if let Some(r) = n.strip_prefix("_XLFN.") {
            n = r.to_string();
        } else if let Some(r) = n.strip_prefix("_XLWS.") {
            n = r.to_string();
        } else {
            break;
        }
    }
    let (v, map) = registry();
    map.get(n.as_str()).and_then(|&i| v.get(i))
}

/// Every registered function.
pub fn all() -> &'static [FnSpec] {
    &registry().0
}

/// Calls a function with array lifting applied. Wrong arity gives `#VALUE!`.
pub fn call(spec: &FnSpec, args: &[Arg], ctx: &mut dyn Ctx) -> Value {
    if !spec.accepts(args.len()) {
        return Value::Error(CellError::Value);
    }
    lift::call_lifted(spec, args, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for s in all() {
            assert!(seen.insert(s.name), "duplicate function {}", s.name);
            assert_eq!(s.name, s.name.to_ascii_uppercase(), "{} not upper-case", s.name);
            assert!(s.min_args <= s.max_args, "{} arity", s.name);
            assert!(!s.description.is_empty() && !s.signature.is_empty(), "{} docs", s.name);
        }
        println!("registered functions: {}", all().len());
        assert!(all().len() >= 400);
        assert!(lookup("_xlfn._xlws.sort").is_some_and(|s| s.name == "SORT"));
        assert!(lookup("nosuchfn").is_none());
        for s in all() {
            assert!(lookup(&format!("_xlfn.{}", s.name.to_ascii_lowercase())).is_some_and(|x| x.name == s.name));
        }
    }

    /// #2163: a NaN column index is refused (it used to become 0 and underflow to `usize::MAX`).
    #[test]
    fn nan_index_is_a_value_error() {
        use crate::util::testutil::*;
        use gridcraft_core::CellError;
        let table = rf(arr(vec![vec![tv("Field"), tv("Other")], vec![nv(1.0), nv(2.0)]]));
        let nan = || av(Value::Number(f64::NAN));
        assert_eq!(ev("VLOOKUP", vec![n(1.0), table.clone(), nan()]), Value::Error(CellError::Value));
        assert_eq!(ev("HLOOKUP", vec![t("Field"), table.clone(), nan()]), Value::Error(CellError::Value));
        assert_eq!(ev("DSUM", vec![table.clone(), nan(), table.clone()]), Value::Error(CellError::Value));
        assert_eq!(ev("VLOOKUP", vec![n(1.0), table, n(2.0)]), Value::Number(2.0));
    }

    /// #2162: REGEXREPLACE gives up on a too-long result while building it, and otherwise replaces
    /// as before (every match, `$n` groups).
    #[test]
    fn regexreplace_bounds_its_result() {
        use crate::util::testutil::*;
        use gridcraft_core::CellError;
        let many = "a".repeat(32_767);
        assert_eq!(ev("REGEXREPLACE", vec![t(&many), t("."), t(&"b".repeat(32_767))]), Value::Error(CellError::Value));
        assert_eq!(ev("REGEXREPLACE", vec![t(&many), t("a"), t("b")]), Value::text("b".repeat(32_767)));
        assert_eq!(ev("REGEXREPLACE", vec![t("a1b22c"), t("[0-9]+"), t("#")]), Value::text("a#b#c"));
        assert_eq!(ev("REGEXREPLACE", vec![t("2026-10-22"), t("(\\d+)-(\\d+)-(\\d+)"), t("$3/$2/$1")]), Value::text("22/10/2026"));
        assert_eq!(ev("REGEXREPLACE", vec![t("abc"), t("x*"), t("-")]), Value::text("-a-b-c-"));
        assert_eq!(ev("REGEXREPLACE", vec![t("a1b2"), t("[0-9]"), t("#"), n(2.0)]), Value::text("a1b#"));
        assert_eq!(ev("REGEXREPLACE", vec![t("a1b2"), t("[0-9]"), t("#"), n(-2.0)]), Value::text("a#b2"));
        assert_eq!(ev("REGEXREPLACE", vec![t("a1b2"), t("[0-9]"), t("#"), n(-3.0)]), Value::text("a1b2"));
        // 2^32 is no occurrence the text has (and on 32-bit targets it must not wrap to 0).
        for occurrence in [4_294_967_296.0, -4_294_967_296.0] {
            assert_eq!(ev("REGEXREPLACE", vec![t("a1b2"), t("[0-9]"), t("#"), n(occurrence)]), Value::text("a1b2"));
        }
        // One match whose replacement repeats it 10,000 times: refused before it is built.
        assert_eq!(ev("REGEXREPLACE", vec![t(&many), t("^.*$"), t(&"$0".repeat(10_000))]), Value::Error(CellError::Value));
        assert_eq!(ev("REGEXREPLACE", vec![t(&many), t("^.*$"), t(&"$0".repeat(10_000)), n(1.0)]), Value::Error(CellError::Value));
    }

    #[test]
    fn hostile_inputs_never_panic() {
        use crate::util::testutil::*;
        use gridcraft_core::{Array, CellError};
        let mixed = Value::from(
            Array::new(2, 3, vec![Value::Number(1.0), Value::from("x"), Value::Empty, Value::Bool(true), Value::Number(-1e300), Value::Number(0.5)])
                .unwrap(),
        );
        let patterns: Vec<Arg> = vec![
            n(0.0),
            n(-1.0),
            n(1e308),
            n(-1e308),
            n(1e10),
            n(2.5),
            // Numbers no formula should make, but code can (#2163).
            av(Value::Number(f64::NAN)),
            av(Value::Number(f64::INFINITY)),
            t(""),
            t("abc"),
            t("*?~"),
            b(true),
            e(CellError::NA),
            empty(),
            av(mixed.clone()),
            rf(mixed),
            rf(col(&[3.0, 1.0, 2.0])),
        ];
        for spec in all() {
            let max = spec.max_args.min(spec.min_args + 3);
            for count in spec.min_args..=max {
                for p in &patterns {
                    let args = vec![p.clone(); count];
                    let _ = call(spec, &args, &mut TestCtx::default());
                }
                // Mixed: cycle through patterns.
                let args: Vec<Arg> = (0..count).map(|i| patterns[(i * 7 + count) % patterns.len()].clone()).collect();
                let _ = call(spec, &args, &mut TestCtx::default());
            }
        }
    }
}
