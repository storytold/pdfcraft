//! `.xls` data validation: DVal and Dv records ([MS-XLS] 2.4.96, 2.4.95). Every record is
//! assembled here from the spec's structures; no Excel-produced file is used. The expected
//! rules are what the `.xlsx` reader gives for the same rule.

use gridcraft_core::{CellRef, RangeRef};
use gridcraft_model::{CfOperator, ErrorStyle, Validation, ValidationKind, Workbook};

use super::xls_tests::{cat, compound, rec, u16s};
use crate::{ReadReport, read_xls};

fn bof(dt: u16) -> Vec<u8> {
    rec(0x0809, &cat(&[&u16s(&[0x0600, dt, 0x0DBB, 0x07CC]), &[0; 8]]))
}

fn eof() -> Vec<u8> {
    rec(0x000A, &[])
}

/// A workbook of one worksheet, "Sheet1", holding `records`, with defined names (Lbl records,
/// each a name and its formula) in the globals.
fn workbook(names: &[(&str, &[u8])], records: &[Vec<u8>]) -> (Workbook, ReadReport) {
    let lbl = |name: &str, rgce: &[u8]| {
        rec(0x0018, &cat(&[&u16s(&[0]), &[0, name.len() as u8], &u16s(&[rgce.len() as u16, 0, 0]), &[0; 4], &[0], name.as_bytes(), rgce]))
    };
    let lbls: Vec<u8> = names.iter().flat_map(|(n, f)| lbl(n, f)).collect();
    let globals = |pos: u32| cat(&[&bof(0x0005), &rec(0x0085, &cat(&[&pos.to_le_bytes(), &[0, 0, 6, 0], b"Sheet1"])), &lbls, &eof()]);
    let body: Vec<u8> = records.concat();
    let sheet = cat(&[&bof(0x0010), &body, &eof()]);
    let stream = cat(&[&globals(globals(0).len() as u32), &sheet]);
    read_xls(&compound("Workbook", &stream)).unwrap()
}

/// DVal: flags, the input window's position, no drop-down object (-1), then `count` rules.
fn dval(count: u32) -> Vec<u8> {
    rec(0x01B2, &cat(&[&[0, 0], &[0; 8], &(-1i32).to_le_bytes(), &count.to_le_bytes()]))
}

// Dv flag bits ([MS-XLS] 2.4.95).
const STR_LOOKUP: u32 = 1 << 7;
const ALLOW_BLANK: u32 = 1 << 8;
const SUPPRESS_COMBO: u32 = 1 << 9;
const SHOW_INPUT: u32 = 1 << 18;
const SHOW_ERROR: u32 = 1 << 19;
/// What Excel sets on a new rule: blanks allowed, input message and error alert shown.
const DEFAULTS: u32 = ALLOW_BLANK | SHOW_INPUT | SHOW_ERROR;

/// Dv flags: rule type, alert style, operator, plus `bits`.
fn flags(kind: u32, alert: u32, op: u32, bits: u32) -> u32 {
    kind | (alert << 4) | (op << 20) | bits
}

/// XLUnicodeString with compressed characters; "" is stored as Excel stores an empty
/// message, a single NUL character.
fn msg(s: &str) -> Vec<u8> {
    let s = if s.is_empty() { "\0" } else { s };
    cat(&[&(s.len() as u16).to_le_bytes(), &[0], s.as_bytes()])
}

/// XLUnicodeString with UTF-16 characters.
fn wide(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    cat(&[&(units.len() as u16).to_le_bytes(), &[1], &u16s(&units)])
}

/// DVParsedFormula: size, 2 unused bytes (Excel leaves stray data there), the tokens.
fn formula(rgce: &[u8]) -> Vec<u8> {
    cat(&[&(rgce.len() as u16).to_le_bytes(), &[0x6E, 0x74], rgce])
}

/// SqRefU: a count, then each Ref8U as rows (first, last) and columns (first, last).
fn sqref(ranges: &[[u16; 4]]) -> Vec<u8> {
    let list: Vec<u8> = ranges.iter().flat_map(|r| u16s(r)).collect();
    cat(&[&(ranges.len() as u16).to_le_bytes(), &list])
}

/// A Dv record with its four messages (prompt title, error title, prompt, error).
fn dv(flags: u32, msgs: [Vec<u8>; 4], f1: &[u8], f2: &[u8], ranges: &[[u16; 4]]) -> Vec<u8> {
    let [pt, et, p, e] = msgs;
    rec(0x01BE, &cat(&[&flags.to_le_bytes(), &pt, &et, &p, &e, &formula(f1), &formula(f2), &sqref(ranges)]))
}

fn no_msgs() -> [Vec<u8>; 4] {
    [msg(""), msg(""), msg(""), msg("")]
}

/// A Dv with Excel's default flags and no messages.
fn rule(kind: u32, alert: u32, op: u32, f1: &[u8], f2: &[u8], ranges: &[[u16; 4]]) -> Vec<u8> {
    dv(flags(kind, alert, op, DEFAULTS), no_msgs(), f1, f2, ranges)
}

fn range(a1: &str) -> RangeRef {
    RangeRef::parse(a1).unwrap()
}

/// The `.xlsx` reader's result for a rule with Excel's default flags and no messages.
fn expected(ranges: &[&str], kind: ValidationKind, op: CfOperator, f1: &str, f2: Option<&str>) -> Validation {
    Validation { ranges: ranges.iter().map(|r| range(r)).collect(), kind, op, f1: f1.into(), f2: f2.map(Into::into), ..Validation::default() }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

const SKIPPED: &str = "Some data validation rules in .xls files could not be read and were skipped.";
const IGNORED: &str = "Some data validation records in .xls files were ignored, as Excel ignores them: they have no DVal record before them, or come after the number of rules it gives.";

/// Each kind of rule, as Excel writes it, gives what the `.xlsx` reader gives for the same rule.
#[test]
fn reads_every_kind_of_rule() {
    use CfOperator::*;
    use ValidationKind::*;
    let rules = [
        // Whole number between 1 and 10, with all four messages and the stop alert.
        dv(
            flags(1, 0, 0, DEFAULTS),
            [msg("Whole"), msg("Oops"), msg("Enter 1 to 10"), msg("Not 1..10")],
            &hex("1e0100"),
            &hex("1e0a00"),
            &[[1, 5, 1, 1]],
        ),
        // Decimal >= 0.5, warning alert.
        rule(2, 1, 6, &hex("1f000000000000e03f"), &[], &[[1, 5, 2, 2]]),
        // Literal list (NUL-separated items), information alert.
        dv(flags(3, 2, 0, DEFAULTS | STR_LOOKUP), no_msgs(), &hex("170e0052656400477265656e00426c7565"), &[], &[[1, 5, 3, 3]]),
        // List from an absolute range ($H$2:$H$5).
        rule(3, 0, 0, &hex("250100040007000700"), &[], &[[1, 5, 4, 4]]),
        // Date between two serials, time < 0.5, text length <= 5, whole not between 5 and 7.
        rule(4, 0, 0, &hex("1e37ab"), &hex("1ea4ac"), &[[1, 5, 5, 5]]),
        rule(5, 0, 5, &hex("1f000000000000e03f"), &[], &[[1, 5, 6, 6]]),
        rule(6, 0, 7, &hex("1e0500"), &[], &[[1, 5, 8, 8]]),
        rule(1, 0, 1, &hex("1e0500"), &hex("1e0700"), &[[1, 5, 9, 9]]),
        // Custom formula K2>A2 on K2:K6 (relative PtgRefN offsets 0 and -10 columns).
        rule(7, 0, 0, &hex("4c000000c04c0000f6c00d"), &[], &[[1, 5, 10, 10]]),
        // Any value, with an input message only.
        dv(flags(0, 0, 0, DEFAULTS), [msg("Note"), msg(""), msg("Anything goes"), msg("")], &[], &[], &[[1, 5, 13, 13]]),
        // A list from a defined name.
        rule(3, 0, 0, &hex("2301000000"), &[], &[[1, 5, 16, 16]]),
        // A dependent list: INDIRECT($S2) on T2:T6.
        rule(3, 0, 0, &hex("190100004c0000128022019400"), &[], &[[1, 5, 19, 19]]),
        // Decimal between two absolute cells.
        rule(2, 0, 0, &hex("4406000700"), &hex("4407000700"), &[[1, 5, 22, 22]]),
    ];
    let mut records = vec![dval(rules.len() as u32)];
    records.extend(rules);
    let (wb, report) = workbook(&[("Colors", &[0x1E, 1, 0])], &records);
    let got = &wb.sheets[0].validations;
    let mut whole = expected(&["B2:B6"], Whole, Between, "1", Some("10"));
    (whole.input_title, whole.error_title, whole.input_message, whole.error_message) =
        ("Whole".into(), "Oops".into(), "Enter 1 to 10".into(), "Not 1..10".into());
    let mut decimal = expected(&["C2:C6"], Decimal, GreaterOrEqual, "0.5", None);
    decimal.error_style = ErrorStyle::Warning;
    let mut literal = expected(&["D2:D6"], List, Between, "\"Red,Green,Blue\"", None);
    literal.error_style = ErrorStyle::Information;
    let mut any = expected(&["N2:N6"], Any, Between, "", None);
    (any.input_title, any.input_message) = ("Note".into(), "Anything goes".into());
    let want = vec![
        whole,
        decimal,
        literal,
        expected(&["E2:E6"], List, Between, "=$H$2:$H$5", None),
        expected(&["F2:F6"], Date, Between, "43831", Some("44196")),
        expected(&["G2:G6"], Time, Less, "0.5", None),
        expected(&["I2:I6"], TextLength, LessOrEqual, "5", None),
        expected(&["J2:J6"], Whole, NotBetween, "5", Some("7")),
        expected(&["K2:K6"], Custom, Between, "K2>A2", None),
        any,
        expected(&["Q2:Q6"], List, Between, "=Colors", None),
        expected(&["T2:T6"], List, Between, "=INDIRECT($S2)", None),
        expected(&["W2:W6"], Decimal, Between, "$H$7", Some("$H$8")),
    ];
    assert_eq!(got.len(), want.len(), "{:?}", report.warnings);
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(g, w);
    }
    assert!(!report.warnings.iter().any(|w| w == SKIPPED), "{:?}", report.warnings);
    assert!(report.warnings[0].contains("data validation were read"), "{:?}", report.warnings);
}

/// The flags that switch features off; an operator stored on a list (where it means nothing);
/// messages in UTF-16; literal items with quotes and spaces; a whole column.
#[test]
fn reads_flags_messages_lists_and_whole_columns() {
    let off = flags(3, 0, 0, STR_LOOKUP | SUPPRESS_COMBO);
    let records = [
        dval(4),
        dv(off, no_msgs(), &hex("170300610062"), &[], &[[1, 5, 14, 14]]),
        dv(
            flags(1, 0, 2, DEFAULTS),
            [wide("Café"), wide("Ошибка"), wide("日本語のテスト é ü"), wide("Nur 3 ✓")],
            &hex("1e0300"),
            &[],
            &[[1, 5, 20, 20]],
        ),
        // typOperator 6 on a list, as found in real files: ignored.
        dv(flags(3, 0, 6, DEFAULTS | STR_LOOKUP), no_msgs(), &hex("170c00736179202268692200627965"), &[], &[[1, 5, 23, 23]]),
        // Rows 0-65535: every row of the BIFF8 grid.
        dv(flags(3, 0, 0, DEFAULTS | STR_LOOKUP), no_msgs(), &hex("170500780079007a"), &[], &[[0, 0xFFFF, 21, 21]]),
    ];
    let (wb, report) = workbook(&[], &records);
    let got = &wb.sheets[0].validations;
    assert_eq!(got.len(), 4, "{:?}", report.warnings);
    let mut flags_off = expected(&["O2:O6"], ValidationKind::List, CfOperator::Between, "\"a,b\"", None);
    (flags_off.allow_blank, flags_off.in_cell_dropdown, flags_off.show_input, flags_off.show_error) = (false, false, false, false);
    assert_eq!(got[0], flags_off);
    let mut unicode = expected(&["U2:U6"], ValidationKind::Whole, CfOperator::Equal, "3", None);
    (unicode.input_title, unicode.error_title, unicode.input_message, unicode.error_message) =
        ("Café".into(), "Ошибка".into(), "日本語のテスト é ü".into(), "Nur 3 ✓".into());
    assert_eq!(got[1], unicode);
    // The item `say "hi"` keeps its quotes, doubled as in `.xlsx`; the operator is ignored.
    assert_eq!(got[2], expected(&["X2:X6"], ValidationKind::List, CfOperator::Between, "\"say \"\"hi\"\",bye\"", None));
    let column = &got[3];
    assert_eq!(column.ranges, vec![RangeRef::cols(21, 21)]);
    assert!(column.ranges[0].is_full_cols());
}

/// Relative references are offsets from the top-left cell of all the rule's ranges, whatever
/// order the ranges are listed in (AF5:AF9 then AG1:AG3 has its formula written for AF1). An
/// offset Excel couldn't keep inside the grid is stored as PtgRefErr and reads as #REF!.
#[test]
fn relative_references_start_from_the_top_left_of_all_ranges() {
    let records = [
        dval(3),
        // =AE1>0 for AF5:AF9 AG1:AG3 (column offset -1 from AF1).
        rule(7, 0, 0, &hex("4c0000ffc01e00000d"), &[], &[[4, 8, 31, 31], [0, 2, 32, 32]]),
        // =AB1>0 for AC1:AC3 AA5:AA9 (column offset +1 from AA1).
        rule(7, 0, 0, &hex("4c000001c01e00000d"), &[], &[[0, 2, 28, 28], [4, 8, 26, 26]]),
        // PtgRefErr: a reference that fell off the grid.
        rule(7, 0, 0, &hex("2a0000f4c01e00000d"), &[], &[[4, 8, 12, 12], [0, 2, 11, 11]]),
    ];
    let (wb, report) = workbook(&[], &records);
    let got = &wb.sheets[0].validations;
    assert_eq!(got.len(), 3, "{:?}", report.warnings);
    assert_eq!(got[0].ranges, vec![range("AF5:AF9"), range("AG1:AG3")]);
    assert_eq!(got[0].f1, "AE1>0");
    assert_eq!(got[1].f1, "AB1>0");
    assert_eq!(got[2].f1, "#REF!>0");
}

/// As in Excel, a DVal's count is authoritative: Dv records past it, or with no DVal before
/// them, aren't read. That's said once, and not as a rule that failed to read.
#[test]
fn only_the_dvals_count_of_rules_is_read() {
    let a = || rule(1, 0, 4, &hex("1e0000"), &[], &[[0, 0, 0, 0]]);
    let b = || rule(1, 0, 4, &hex("1e0000"), &[], &[[1, 1, 0, 0]]);
    let (wb, report) = workbook(&[], &[dval(1), a(), b(), b()]);
    assert_eq!(wb.sheets[0].validations.iter().map(|v| v.ranges.clone()).collect::<Vec<_>>(), vec![vec![range("A1")]]);
    assert_eq!(report.warnings.iter().filter(|w| *w == IGNORED).count(), 1, "{:?}", report.warnings);
    assert!(!report.warnings.iter().any(|w| w == SKIPPED), "{:?}", report.warnings);
    let (wb, report) = workbook(&[], &[a(), b()]);
    assert!(wb.sheets[0].validations.is_empty());
    assert!(report.warnings.iter().any(|w| w == IGNORED), "{:?}", report.warnings);
    assert!(!report.warnings.iter().any(|w| w == SKIPPED), "{:?}", report.warnings);
}

/// Malformed rules never fail the import. Where Excel opens such a file, the result is what
/// Excel shows; where Excel refuses the whole file, that one rule is skipped with a warning.
#[test]
fn malformed_rules_are_repaired_or_skipped() {
    let ok = |row: u16| rule(1, 0, 4, &hex("1e0000"), &[], &[[row, row, 0, 0]]);
    let cases: Vec<(&str, Vec<u8>, Option<&str>)> = vec![
        // Excel opens an unknown type as "any value".
        ("unknown type", rule(9, 0, 0, &hex("1e0100"), &[], &[[1, 5, 1, 1]]), Some("B2:B6")),
        // Excel refuses an unknown alert style, or an unknown operator on a rule that uses one.
        ("unknown alert", rule(1, 5, 0, &hex("1e0100"), &hex("1e0a00"), &[[1, 5, 1, 1]]), None),
        ("unknown operator", rule(1, 0, 9, &hex("1e0100"), &[], &[[1, 5, 1, 1]]), None),
        // Excel drops a reversed range, and clamps columns past IV.
        ("reversed range", rule(1, 0, 4, &hex("1e0000"), &[], &[[5, 1, 2, 2]]), None),
        ("past column IV", rule(1, 0, 4, &hex("1e0000"), &[], &[[1, 5, 3, 0x100]]), Some("D2:IV6")),
        // Excel refuses a range count or a formula size that runs past the record.
        (
            "too many ranges",
            rec(
                0x01BE,
                &cat(&[
                    &flags(1, 0, 4, DEFAULTS).to_le_bytes(),
                    &no_msgs().concat(),
                    &formula(&hex("1e0000")),
                    &formula(&[]),
                    &u16s(&[2, 1, 5, 4, 4]),
                ]),
            ),
            None,
        ),
        (
            "formula past the record",
            rec(0x01BE, &cat(&[&flags(1, 0, 4, DEFAULTS).to_le_bytes(), &no_msgs().concat(), &u16s(&[200, 0]), &hex("1e0000")])),
            None,
        ),
        ("truncated message", rec(0x01BE, &cat(&[&flags(1, 0, 4, DEFAULTS).to_le_bytes(), &u16s(&[40]), &[0], b"short"])), None),
        ("empty record", rec(0x01BE, &[]), None),
        // A formula GridCraft can't decode (an unknown token).
        ("unknown token", rule(7, 0, 0, &[0xFF], &[], &[[1, 5, 1, 1]]), None),
        // No ranges at all.
        ("no ranges", rule(1, 0, 4, &hex("1e0000"), &[], &[]), None),
    ];
    for (what, bad, want) in cases {
        let (wb, report) = workbook(&[], &[dval(3), ok(10), bad, ok(11)]);
        let got = &wb.sheets[0].validations;
        let ranges: Vec<RangeRef> = got.iter().flat_map(|v| v.ranges.clone()).collect();
        let mut expect = vec![range("A11")];
        expect.extend(want.map(range));
        expect.push(range("A12"));
        assert_eq!(ranges, expect, "{what}: {:?}", report.warnings);
        assert_eq!(report.warnings.iter().any(|w| w == SKIPPED), want.is_none(), "{what}: {:?}", report.warnings);
        if what == "unknown type" {
            assert_eq!((got[1].kind, got[1].f1.as_str(), got[1].f2.as_deref()), (ValidationKind::Any, "", None));
        }
    }
}

/// Hostile counts and sizes never panic or allocate from the file's numbers.
#[test]
fn hostile_validation_records_never_panic() {
    let f = flags(1, 0, 4, DEFAULTS).to_le_bytes();
    let big = [
        rec(0x01B2, &[0xFF; 18]),
        rec(0x01BE, &cat(&[&f, &u16s(&[0xFFFF]), &[1]])),
        rec(0x01BE, &cat(&[&f, &no_msgs().concat(), &u16s(&[0xFFFF, 0xFFFF])])),
        rec(0x01BE, &cat(&[&f, &no_msgs().concat(), &formula(&[]), &formula(&[]), &u16s(&[0xFFFF])])),
        // A literal list whose PtgStr claims 255 UTF-16 characters and holds none.
        rec(
            0x01BE,
            &cat(&[
                &flags(3, 0, 0, DEFAULTS | STR_LOOKUP).to_le_bytes(),
                &no_msgs().concat(),
                &formula(&[0x17, 0xFF, 1]),
                &formula(&[]),
                &sqref(&[[0, 0, 0, 0]]),
            ]),
        ),
        // Every flag bit set: type 15, alert 7, operator 15.
        rec(0x01BE, &cat(&[&[0xFF; 4], &no_msgs().concat(), &formula(&[]), &formula(&[]), &sqref(&[[0, 0, 0, 0]])])),
    ];
    let (wb, _) = workbook(&[], &big);
    assert!(wb.sheets[0].validations.is_empty());
    let mut many = vec![dval(u32::MAX)];
    many.extend((0..200u16).map(|r| rule(1, 0, 4, &hex("1e0000"), &[], &[[r, r, 0, 0]])));
    let (wb, _) = workbook(&[], &many);
    assert_eq!(wb.sheets[0].validations.len(), 200);
    assert_eq!(wb.sheets[0].validations[199].ranges, vec![RangeRef::cell(CellRef::new(199, 0))]);
}

/// Rules with no messages cost no text, so a crafted file could otherwise keep millions of them:
/// the workbook keeps 65,536 and says it skipped the rest.
#[test]
fn rules_past_the_workbook_limit_are_skipped() {
    let one = rule(0, 0, 0, &[], &[], &[[0, 0, 0, 0]]);
    let mut records = vec![dval(65_537)];
    records.extend(std::iter::repeat_n(one, 65_537));
    let (wb, report) = workbook(&[], &records);
    assert_eq!(wb.sheets[0].validations.len(), 65_536);
    // craft-xls counts the rules past its cap with the ones it could not read.
    assert_eq!(report.warnings.iter().filter(|w| *w == SKIPPED).count(), 1, "{:?}", report.warnings);
    assert!(!report.warnings.iter().any(|w| w == IGNORED), "{:?}", report.warnings);
}
