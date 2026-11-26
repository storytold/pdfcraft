//! Formulas › Recently Used: the functions most recently entered in a formula.
//!
//! The list is [`crate::Prefs::recent_functions`], an application list like Excel's, saved with
//! the other preferences. [`crate::Session::execute`] calls [`note`] for each `cell.*` and
//! `range.*` command it journals, from any source (the interface, the control channel, MCP or a
//! script), so the list never depends on the journal, which drops its oldest entries.

use std::collections::HashSet;
use std::sync::OnceLock;

use crate::cmd::edit::is_formula_input;
use gridcraft_formula::lexer::{Tok, tokenize};
use gridcraft_formula::normalize_function_name;
use serde::Deserialize;
use serde_json::Value as Json;

/// How many functions the list keeps.
pub const MAX: usize = 10;
/// The longest formula read (Excel's formula length limit): a longer one was not typed.
const FORMULA_MAX: usize = 8_192;
/// The most formulas read from one command: a large paste needs only its last few.
const SCAN_BUDGET: usize = 2_000;

/// What the list holds before any function has been used.
pub fn defaults() -> Vec<String> {
    ["SUM", "AVERAGE", "IF", "COUNT", "MAX", "MIN", "SUMIF", "XLOOKUP", "PMT", "TODAY"].map(String::from).to_vec()
}

/// Whether `name` (upper case, without `_xlfn.`) is a function of the catalog.
fn known(name: &str) -> bool {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    NAMES
        .get_or_init(|| crate::cmd::formulas::function_list().iter().filter_map(|f| f["name"].as_str().map(str::to_ascii_uppercase)).collect())
        .contains(name)
}

/// The functions a formula calls, outermost first, known functions only. A formula is what a
/// cell takes as one (`=…`, or `+…`/`-…` as Excel takes `+SUM(1)`).
pub fn functions_in_formula(text: &str) -> Vec<String> {
    if !is_formula_input(text) {
        return vec![];
    }
    let body = text.strip_prefix('=').unwrap_or(text);
    if body.len() > FORMULA_MAX {
        return vec![];
    }
    let mut out: Vec<String> = Vec::new();
    for t in tokenize(body).unwrap_or_default() {
        if let Tok::Func(name) = t.tok {
            let name = normalize_function_name(&name);
            if known(&name) && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// The formula texts a command's params enter, the last first, at most `budget` of them.
fn formulas_newest_first<'a>(id: &str, params: &'a Json, budget: usize) -> Vec<&'a str> {
    fn walk<'a>(v: &'a Json, out: &mut Vec<&'a str>, depth: usize, budget: usize) {
        match v {
            Json::String(s) if is_formula_input(s) && out.len() < budget => out.push(s),
            // A range's values are rows of cells; anything deeper is not a cell value.
            Json::Array(a) if depth < 2 => {
                for x in a.iter().rev() {
                    if out.len() >= budget {
                        return;
                    }
                    walk(x, out, depth + 1, budget);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if id.starts_with("cell.") || id.starts_with("range.") {
        for key in ["formula", "values", "value", "input"] {
            if let Some(v) = params.get(key) {
                walk(v, &mut out, 0, budget);
            }
        }
    }
    out
}

/// Moves `names` (most recent first) to the front of `recent`, keeping at most [`MAX`].
pub fn push(recent: &mut Vec<String>, names: &[String]) {
    let mut next: Vec<String> = Vec::with_capacity(MAX);
    for n in names.iter().chain(recent.iter()) {
        if next.len() == MAX {
            break;
        }
        if !next.contains(n) {
            next.push(n.clone());
        }
    }
    *recent = next;
}

/// Moves the functions that the formulas a command entered call to the front of `recent`. The
/// last formulas are read first (they are the most recent), within [`SCAN_BUDGET`].
pub(crate) fn note(recent: &mut Vec<String>, id: &str, params: &Json) {
    let mut names: Vec<String> = Vec::new();
    for f in formulas_newest_first(id, params, SCAN_BUDGET) {
        for n in functions_in_formula(f) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
        if names.len() >= MAX {
            break;
        }
    }
    if !names.is_empty() {
        push(recent, &names);
    }
}

/// Reads a saved list leniently: known function names only, at most [`MAX`]. A value that is not
/// a list gives the default list, so a hand-edited or hostile `prefs.json` loses this list, never
/// the other preferences.
pub(crate) fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let raw = Json::deserialize(d)?;
    let Some(list) = raw.as_array() else { return Ok(defaults()) };
    let mut out = Vec::new();
    for name in list.iter().filter_map(Json::as_str) {
        if out.len() == MAX {
            break;
        }
        let name = name.to_ascii_uppercase();
        if known(&name) && !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formulas_name_their_functions_outermost_first() {
        assert_eq!(functions_in_formula("=IF(SUM(A1:A3)>2,_xlfn.XLOOKUP(1,B:B,C:C),sum(1))"), ["IF", "SUM", "XLOOKUP"]);
        assert!(functions_in_formula("=MYLAMBDA(1)+A1").is_empty(), "only known functions");
        assert!(functions_in_formula("SUM(1)").is_empty(), "not a formula");
        assert!(functions_in_formula("=\"SUM(\"").is_empty(), "inside a string");
        assert!(functions_in_formula(&format!("=SUM({})", "1+".repeat(9000))).is_empty(), "too long");
        assert!(functions_in_formula("=((((").is_empty());
        assert!(functions_in_formula("=").is_empty());
        // A formula may be typed with + or - instead of =, as input_to_cell takes it.
        assert_eq!(functions_in_formula("+SUM(1)"), ["SUM"]);
        assert_eq!(functions_in_formula("-ABS(A1)"), ["ABS"]);
        assert!(functions_in_formula("-5").is_empty() && functions_in_formula("+1e3").is_empty(), "numbers");
        assert!(functions_in_formula("+").is_empty() && functions_in_formula("- SUM(1)").is_empty(), "not formulas");
    }

    #[test]
    fn commands_give_their_formulas_newest_first() {
        let read = |id: &str, params: Json| formulas_newest_first(id, &params, SCAN_BUDGET).into_iter().map(String::from).collect::<Vec<_>>();
        assert_eq!(read("range.setValues", json!({"values": [["=SUM(1)", 2], ["x", "=PMT(1,2,3)"]]})), ["=PMT(1,2,3)", "=SUM(1)"]);
        assert_eq!(read("cell.set", json!({"input": "=NPV(1,2)"})), ["=NPV(1,2)"]);
        assert_eq!(read("range.setValues", json!({"values": [["+SUM(1)", "-5", "-ABS(2)"]]})), ["-ABS(2)", "+SUM(1)"]);
        assert!(read("home.bold", json!({"input": "=IRR(1)"})).is_empty(), "only cell.* and range.* enter formulas");
        assert!(read("range.setValues", json!({"values": [[["=FV(1,2,3)"]]]})).is_empty(), "deeper than rows of cells");
        // A large paste is read from its end, within the budget.
        let rows: Vec<Json> = (0..5_000).map(|i| json!([format!("=ABS({i})")])).collect();
        let paste = json!({ "values": rows });
        let last = formulas_newest_first("range.setValues", &paste, 3);
        assert_eq!(last, ["=ABS(4999)", "=ABS(4998)", "=ABS(4997)"]);
    }

    #[test]
    fn recent_keeps_ten_distinct_newest_first() {
        let mut r = defaults();
        assert!(r.iter().all(|n| known(n)), "the default list names known functions");
        push(&mut r, &["PMT".into(), "NPV".into()]);
        assert_eq!(r.len(), MAX);
        assert_eq!(&r[..3], ["PMT", "NPV", "SUM"]);
        assert_eq!(r.iter().filter(|n| *n == "PMT").count(), 1);
        let many: Vec<String> =
            ["ABS", "ACOS", "AND", "CHOOSE", "DATE", "EXP", "FIND", "INDEX", "LEFT", "LN", "MOD", "NOW"].map(String::from).to_vec();
        push(&mut r, &many);
        assert_eq!(r, many[..MAX]);
    }

    #[test]
    fn a_saved_list_is_read_leniently() {
        let prefs = |v: Json| serde_json::from_value::<crate::Prefs>(v).unwrap();
        assert_eq!(prefs(json!({"r1c1": true})).recent_functions, defaults(), "a prefs.json from before the list gets the default");
        // A hostile value loses only itself, never the other preferences.
        let bad = prefs(json!({"r1c1": true, "recentFunctions": 5}));
        assert!(bad.r1c1);
        assert_eq!(bad.recent_functions, defaults());
        let mut odd: Vec<Json> = ["npv", "NOT_A_FUNCTION", "SUM", "NPV"].iter().map(|n| json!(n)).chain([json!(3), json!(null)]).collect();
        odd.extend(crate::cmd::formulas::function_list().into_iter().take(40).map(|f| f["name"].clone()));
        let odd = prefs(json!({ "recentFunctions": odd }));
        assert_eq!(&odd.recent_functions[..2], ["NPV", "SUM"]);
        assert_eq!(odd.recent_functions.len(), MAX);
        let back = prefs(serde_json::to_value(&odd).unwrap());
        assert_eq!(back.recent_functions, odd.recent_functions, "the list survives a save and reload");
    }
}
