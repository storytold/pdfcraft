//! FormCalc (XFA 3.3 part 8), the default scripting language of older Designer forms: a lexer,
//! a parser and a tree-walking evaluator over the same form table and effects as the JavaScript
//! model ([`crate::xfa`]), so the engine treats both languages alike.
//!
//! Covered: expressions with FormCalc's operators and coercions, `var`, assignment to fields
//! and properties, `if`/`elseif`/`else`, `while`, `for … upto/downto … step`, `foreach`,
//! `func … endfunc`, `break`/`continue`/`return`/`exit`, accessors (`$`, `$host`, `$form`,
//! `$record`, `$layout`, `$event`, `xfa.…`, `..name`, `[n]`, `[*]`, `_name` instance managers),
//! and the arithmetic, logical, string, date/time, financial and unit built-in functions.
//! `Get`, `Post`, `Put` (network) refuse.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::xfa::{
    HNode, XHost, XfaDoc, XfaEffect, XfaEvent, XfaKind, XfaNode, XfaOutcome, child_named, clip, clip_result, descendants_named, instances,
    reset_names, resolve_som, run_within,
};
use crate::{Limits, Message, MessageButtons, MessageIcon};

mod access;
mod ast;
mod builtins;
mod dates;
mod eval;
mod lexer;
mod parser;
mod value;
mod words;

use ast::Stmt;
pub(crate) use dates::days_in;
#[cfg(test)]
use dates::{civil_from_days, days_from_civil, format_date, parse_date};
use parser::parse;
use value::Val;
#[cfg(test)]
use value::{fixed, num_text, parse_num};
#[cfg(test)]
use words::word_num;

/// Deepest nesting the parser follows.
const MAX_PARSE_DEPTH: usize = 64;
/// Most statements and expressions evaluated in one run (loops included).
const MAX_STEPS: u64 = 5_000_000;
/// Longest string a script may build.
const MAX_STRING: usize = 1 << 20;
/// Most elements a list may hold.
const MAX_LIST: usize = 100_000;
/// Most operators and accessors in one expression (`a + b + …`, `a.b.c…`, brackets included):
/// the tree is as deep as the chain, and dropping or cloning it recurses.
const MAX_CHAIN: usize = 1000;
// The errors a run stops with when it reaches a limit (rather than a fault in the script).
const RAN_TOO_LONG: &str = "the script ran too long";
const TOO_DEEP: &str = "the script is nested too deeply";
const CALLS_TOO_DEEP: &str = "functions are nested too deeply";
const LOOPED_TOO_MUCH: &str = "the script looped too many times";
const LIST_TOO_LONG: &str = "a list is too long";
const STRING_TOO_LONG: &str = "a string is too long";

/// Whether `e` is a limit the run reached: those stop the script even where a fault is
/// otherwise an answer (`Exists`).
fn is_limit(e: &str) -> bool {
    [RAN_TOO_LONG, TOO_DEEP, CALLS_TOO_DEEP, LOOPED_TOO_MUCH, LIST_TOO_LONG, STRING_TOO_LONG].contains(&e)
}

/// Steps between looks at the clock.
#[cfg(not(target_arch = "wasm32"))]
const CLOCK_EVERY: u64 = 256;

// ── the evaluator ───────────────────────────────────────────────────────────────────────────

enum Flow {
    Next,
    Break,
    Continue,
    Return(Val),
    Exit,
}

#[derive(Debug, PartialEq)]
struct Func {
    params: Vec<String>,
    body: Vec<Stmt>,
}

struct Interp<'h> {
    h: &'h mut XHost,
    scopes: Vec<HashMap<String, Val>>,
    /// User functions by lower-case name (FormCalc names are case-insensitive).
    funcs: HashMap<String, Rc<Func>>,
    event: XfaEvent,
    steps: u64,
    loops_left: u64,
    depth: usize,
    max_depth: usize,
    /// `exit` ran: every block ends at once.
    exited: bool,
    /// When the caller stops waiting: the script stops itself soon after (no thread is left
    /// burning a core to the loop limit).
    #[cfg(not(target_arch = "wasm32"))]
    deadline: Option<std::time::Instant>,
    /// `deadline` passed and stopped the script: the run reports as abandoned.
    #[cfg(not(target_arch = "wasm32"))]
    out_of_time: bool,
}

type R<T> = Result<T, String>;

// ── entry points ────────────────────────────────────────────────────────────────────────────

/// Run a FormCalc `script` for `event` on the form `root`, as [`crate::xfa::run_xfa`] runs a
/// JavaScript one.
pub fn run_formcalc(script: &str, event: &XfaEvent, doc: &XfaDoc, root: &XfaNode, limits: Limits) -> XfaOutcome {
    run_formcalc_at(script, event, doc, std::sync::Arc::new(root.clone()), false, limits, None)
}

/// [`run_formcalc`] on an owned form, giving up at `deadline` (the clock is read every
/// [`CLOCK_EVERY`] steps; unused on wasm, which has no clock).
pub(crate) fn run_formcalc_at(
    script: &str,
    event: &XfaEvent,
    doc: &XfaDoc,
    root: std::sync::Arc<XfaNode>,
    reuse: bool,
    limits: Limits,
    deadline: Option<std::time::Duration>,
) -> XfaOutcome {
    // FormCalc has its own nesting limits (the shared `refuse` reads JavaScript syntax).
    if script.len() > crate::MAX_SCRIPT_BYTES {
        return XfaOutcome {
            error: Some(format!("the script is too long ({} KiB; the limit is {} KiB)", script.len() / 1024, crate::MAX_SCRIPT_BYTES / 1024)),
            ..Default::default()
        };
    }
    let body = match parse(script) {
        Ok(b) => b,
        Err(e) => return XfaOutcome { error: Some(e), ..Default::default() },
    };
    let mut host = XHost::new(root, reuse, &event.target, doc);
    #[cfg(target_arch = "wasm32")]
    let _ = deadline;
    let mut interp = Interp {
        h: &mut host,
        scopes: vec![HashMap::new()],
        funcs: HashMap::new(),
        event: event.clone(),
        steps: 0,
        loops_left: limits.loop_iterations,
        depth: 0,
        // The JavaScript recursion limit, kept to 8..=128: statement nesting counts against it
        // too, and the interpreter's frames are large.
        max_depth: limits.recursion.clamp(8, 128),
        exited: false,
        #[cfg(not(target_arch = "wasm32"))]
        deadline: deadline.and_then(|d| std::time::Instant::now().checked_add(d)),
        #[cfg(not(target_arch = "wasm32"))]
        out_of_time: false,
    };
    let mut out = XfaOutcome::default();
    match interp.block(&body) {
        Ok((_, last)) => {
            // As the JavaScript path reports: null is an empty result (a calculate clears the
            // field) with no truth value.
            let last = interp.deref(last);
            out.result = Some(interp.to_str(&last));
            if !matches!(last, Val::Null) {
                out.result_bool = Some(interp.truthy(&last));
            }
        }
        Err(e) => out.error = Some(e),
    }
    // The caller stops waiting at the same time limit, so which of the two notices first is
    // down to the scheduler. The outcome must not depend on it: stopped by the clock, the run
    // reports what an abandoned one does, the error and no changes. Keeping the changes made
    // it a race: a loop adding rows until the clock stopped it left the form unchanged on one
    // run and 1000 rows longer on the next (each laid out again, tens of seconds).
    #[cfg(not(target_arch = "wasm32"))]
    if let (true, Some(d)) = (interp.out_of_time, deadline) {
        return XfaOutcome {
            error: Some(format!("the script ran longer than {:.1} s and was abandoned", d.as_secs_f64())),
            abandoned: true,
            ..Default::default()
        };
    }
    out.notes = host.notes();
    out.effects = host.effects;
    out.console = host.console;
    clip_result(&mut out);
    out
}

/// [`run_formcalc`] on its own thread, abandoned after `timeout` (and told to stop itself then).
pub fn run_formcalc_within(script: &str, event: &XfaEvent, doc: &XfaDoc, root: XfaNode, limits: Limits, timeout: std::time::Duration) -> XfaOutcome {
    let (script, event, doc) = (script.to_string(), event.clone(), doc.clone());
    // FormCalc has no engine to start: its time (and its own deadline) begins at once.
    run_within("pdfcraft-formcalc", timeout, move |started| {
        started();
        run_formcalc_at(&script, &event, &doc, std::sync::Arc::new(root), false, limits, Some(timeout))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_does_not_print_negative_zero() {
        assert_eq!(fixed(-0.4, 0), "0");
        assert_eq!(fixed(-0.004, 2), "0.00");
        assert_eq!(fixed(-0.0, 0), "0");
        assert_eq!(fixed(-1.5, 0), "-2");
    }

    #[test]
    fn dates_round_trip_through_formcalc_day_numbers() {
        assert_eq!(days_from_civil(1900, 1, 1), 1);
        assert_eq!(days_from_civil(1970, 1, 1), 25_568);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        // Past chrono's ±262143 the 400-year cycles carry the year; values from the pre-chrono
        // formula, both directions, both sides of the epoch.
        for (n, date) in [
            (99_000_000, (272_952, 10, 16)),
            (99_999_999, (275_690, 9, 11)),
            (-99_000_000, (-269_153, 3, 16)),
            (-99_999_999, (-271_891, 4, 20)),
            (72_354_905, (200_000, 12, 31)),
            (-73_742_460, (-200_000, 1, 1)),
        ] {
            assert_eq!(civil_from_days(n), date, "{n}");
            assert_eq!(days_from_civil(date.0, date.1, date.2), n, "{date:?}");
        }
        assert_eq!(days_in(272_952, 2), 29, "272952 is a leap year");
        assert_eq!(days_in(-269_153, 2), 28);
        assert_eq!(days_in(i64::MAX, 12), 31);
        let _ = (civil_from_days(i64::MAX), civil_from_days(i64::MIN), days_from_civil(i64::MAX, 12, 31), days_from_civil(i64::MIN, 1, 1));
        assert_eq!(parse_date("2001-02-03", "YYYY-MM-DD"), Some((2001, 2, 3)));
        assert_eq!(parse_date("Feb 3, 2001", "MMM D, YYYY"), Some((2001, 2, 3)));
        assert_eq!(parse_date("2/3/01", "M/D/YY"), Some((2001, 2, 3)));
        assert_eq!(format_date(2001, 2, 3, "MMMM D, YYYY"), "February 3, 2001");
        assert_eq!(word_num(1234.5, true), "One Thousand Two Hundred Thirty-Four Dollars And 50 Cents");
        assert_eq!(word_num(999_999_999_999.999, true), "");
        assert_eq!(num_text(3.0), "3");
        assert_eq!(num_text(2.5), "2.5");
        assert_eq!(num_text(0.1 + 0.2), "0.3");
        assert_eq!(parse_num("12abc"), Some(12.0));
        assert_eq!(parse_num("abc"), None);
    }

    /// A number prints to at most fifteen significant digits, so the binary noise of a value
    /// with six digits before the point (279893.88 is 279893.880000000004656… as a double) stays
    /// hidden, while small values keep their twelve decimals.
    #[test]
    fn num_text_prints_at_most_fifteen_significant_digits() {
        assert_eq!(num_text(279_893.88), "279893.88");
        assert_eq!(num_text(-279_893.88), "-279893.88");
        assert_eq!(num_text(279_893.88 + 291_774.12 + 304_171.06 + 317_107.44 + 273_970.85), "1466917.35");
        assert_eq!(num_text(1_000_000.0 / 3.0), "333333.333333333");
        assert_eq!(num_text(1.0 / 3.0), "0.333333333333");
        assert_eq!(num_text(0.3 - 0.1 - 0.2), "0", "noise below twelve decimals is zero");
        // Past fifteen digits before the point no decimals are left, and the integer keeps its zeros.
        assert_eq!(num_text(1e15 + 0.5), "1000000000000000");
        assert_eq!(num_text(1e20), "100000000000000000000");
        assert_eq!(num_text(-1e20), "-100000000000000000000");
        assert_eq!(num_text(f64::MAX).len(), 309);
    }

    #[test]
    fn decode_xml_and_html_entities() {
        let eval = |script: &str| {
            let root = XfaNode::default();
            let out = run_formcalc(script, &XfaEvent::default(), &XfaDoc::default(), &root, Limits::default());
            assert!(out.error.is_none(), "{:?}", out.error);
            out.result.unwrap_or_default()
        };

        // `&apos;` in both xml and html
        assert_eq!(eval(r#"Decode("It&apos;s &lt;ok&gt; &amp; &quot;fine&quot;", "xml")"#), "It's <ok> & \"fine\"");
        assert_eq!(eval(r#"Decode("It&apos;s", "html")"#), "It's");

        // Decimal and hex numeric character references (case-insensitive #x/#X and hex digits, leading zeros)
        assert_eq!(eval(r#"Decode("&#65;&#x42;&#X43;&#00068;&#x00065;&#x20AC;&#x1F600;", "xml")"#), "ABCDe€😀");
        assert_eq!(eval(r#"Decode("&#65;&#x42;&#X43;&#x00E9;", "html")"#), "ABCé");

        // Invalid / surrogate / out-of-range / zero / malformed numeric references stay literal without panicking
        assert_eq!(
            eval(r#"Decode("&#0; &#xD800; &#xDFFF; &#x110000; &#99999999999999999999; &#; &#x; &#65 &#x4G; &;", "xml")"#),
            "&#0; &#xD800; &#xDFFF; &#x110000; &#99999999999999999999; &#; &#x; &#65 &#x4G; &;"
        );

        // Single-pass decoding: `&amp;lt;` and `&#38;lt;` must decode to `&lt;`, never `<`
        assert_eq!(eval(r#"Decode("&amp;lt; &amp;#65; &#38;lt; &#x26;apos;", "xml")"#), "&lt; &#65; &lt; &apos;");
        assert_eq!(eval(r#"Decode("&amp;nbsp; &#38;eacute;", "html")"#), "&nbsp; &eacute;");

        // HTML named entities decode in "html" mode but remain literal in "xml" mode
        assert_eq!(eval(r#"Decode("caf&eacute;&nbsp;&copy;&euro;&mdash;&alpha;&Omega;&hearts;", "html")"#), "café\u{00A0}©€—αΩ♥");
        assert_eq!(eval(r#"Decode("caf&eacute;&nbsp;&copy;&euro;", "xml")"#), "caf&eacute;&nbsp;&copy;&euro;");
        // Case sensitivity of HTML named entities (`&Eacute;` vs `&eacute;`, `&Prime;` vs `&prime;`, `&Dagger;` vs `&dagger;`)
        assert_eq!(eval(r#"Decode("&Eacute;&eacute;&Prime;&prime;&Dagger;&dagger;&bogus;", "html")"#), "Éé″′‡†&bogus;");
        // Non-ASCII characters after `&` or `&#` and long runs of `&` without `;`
        assert_eq!(eval(r#"Decode("&é; &#é; &#xé; &amp;é", "html")"#), "&é; &#é; &#xé; &é");
        let amps = "&".repeat(5_000);
        assert_eq!(eval(&format!(r#"Decode("{amps}&apos;", "xml")"#)), format!("{amps}'"));
    }
}
