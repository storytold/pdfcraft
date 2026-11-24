use gridcraft_core::{CellError, CellRef, Value};
use gridcraft_model::{Cell, Formula, Workbook};

use crate::Calc;

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

struct T {
    wb: Workbook,
    calc: Calc,
}

impl T {
    fn new() -> T {
        T { wb: Workbook::new(), calc: Calc::new() }
    }
    fn set(&mut self, at: &str, input: &str) {
        let cell = if input.starts_with('=') {
            Cell::formula(Formula::new(input))
        } else {
            Cell::value(gridcraft_core::parse::parse_input(input, self.wb.date_system).value)
        };
        self.wb.sheet_mut(0).unwrap().cells.set(c(at), cell);
        self.calc.cells_changed(&mut self.wb, &[(0, c(at))]);
    }
    fn get(&self, at: &str) -> Value {
        self.wb.sheet(0).unwrap().value(c(at))
    }
    fn num(&self, at: &str) -> f64 {
        self.get(at).as_f64().unwrap_or_else(|| panic!("{at} = {:?}", self.get(at)))
    }
}

#[test]
fn spill_range_operator() {
    let mut t = T::new();
    t.set("A1", "3");
    t.set("B1", "=SEQUENCE(A1)");
    t.set("C1", "=SUM(B1#)");
    t.set("C2", "=ROWS(B1#)");
    assert_eq!((t.num("C1"), t.num("C2")), (6.0, 3.0));
    // Growing and shrinking the spill recalculates its users.
    t.set("A1", "5");
    assert_eq!((t.num("C1"), t.num("C2")), (15.0, 5.0));
    t.set("A1", "2");
    assert_eq!((t.num("C1"), t.num("C2")), (3.0, 2.0));
    // A blocked spill, or a cell that doesn't spill, gives #REF!.
    t.set("B2", "x");
    assert_eq!(t.get("C1"), Value::Error(CellError::Ref));
    t.set("B2", "");
    assert_eq!(t.num("C1"), 3.0);
    t.set("D1", "=SUM(A1#)");
    assert_eq!(t.get("D1"), Value::Error(CellError::Ref));
    // Through a defined name.
    t.wb.names.push(gridcraft_model::DefinedName {
        name: "Seq".into(),
        scope: None,
        formula: "Sheet1!$B$1#".into(),
        comment: String::new(),
        hidden: false,
    });
    t.set("E1", "=SUM(Seq)*10");
    assert_eq!(t.num("E1"), 30.0);
    t.set("A1", "4");
    assert_eq!(t.num("E1"), 100.0);
}

#[test]
fn arrays_that_spill_as_before_do_not_requeue_their_dependents() {
    // Two arrays that each read the other's spill area, and a formula reading both spills.
    let mut t = T::new();
    t.set("A1", "=SEQUENCE(2)+0*B2");
    t.set("B1", "=SEQUENCE(2)+0*A2");
    t.set("C1", "=SUM(A1#)+SUM(B1#)");
    // An edit that spills the same values again: one round, not one per round up to the limit.
    t.set("A1", "=SEQUENCE(2)+0*B2+0");
    assert_eq!((t.num("A2"), t.num("B2"), t.num("C1")), (2.0, 2.0, 6.0));
    assert!(t.calc.last_recalc_cells <= 3, "{} formulas evaluated for one edit", t.calc.last_recalc_cells);
    // Only the anchor's value changes: formulas reading the whole spill still see it.
    t.set("A1", "={7;2}+0*B2");
    assert_eq!((t.num("A1"), t.num("C1")), (7.0, 12.0));
    // A spill that does change still reaches its dependents.
    t.set("A1", "=SEQUENCE(2)*10+0*B2");
    assert_eq!((t.num("A2"), t.num("C1")), (20.0, 33.0));
}

#[test]
fn arithmetic_and_dependencies() {
    let mut t = T::new();
    t.set("A1", "2");
    t.set("A2", "3");
    t.set("A3", "=A1*A2+1");
    assert_eq!(t.num("A3"), 7.0);
    t.set("A1", "10");
    assert_eq!(t.num("A3"), 31.0);
    t.set("B1", "=SUM(A1:A3)");
    assert_eq!(t.num("B1"), 44.0);
    t.set("A2", "0");
    assert_eq!(t.num("A3"), 1.0);
    assert_eq!(t.num("B1"), 11.0);
}

#[test]
fn chains_out_of_order() {
    let mut t = T::new();
    t.set("A5", "=A4+1");
    t.set("A4", "=A3+1");
    t.set("A3", "=A2+1");
    t.set("A2", "=A1+1");
    t.set("A1", "1");
    assert_eq!(t.num("A5"), 5.0);
}

#[test]
fn xlookup_returns_a_reference() {
    let mut t = T::new();
    for (i, (k, v)) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)].iter().enumerate() {
        t.set(&format!("A{}", i + 1), k);
        t.set(&format!("B{}", i + 1), &v.to_string());
    }
    t.set("D1", "=SUM(XLOOKUP(\"b\",A1:A4,B1:B4):XLOOKUP(\"d\",A1:A4,B1:B4))");
    assert_eq!(t.num("D1"), 9.0);
    t.set("D2", "=SUM(B1:XLOOKUP(\"c\",A1:A4,B1:B4))");
    assert_eq!(t.num("D2"), 6.0);
    t.set("D3", "=CELL(\"address\",XLOOKUP(\"c\",A1:A4,B1:B4))");
    assert_eq!(t.get("D3"), Value::from("$B$3"));
    // Not found: if_not_found, else #N/A, as before.
    t.set("D4", "=XLOOKUP(\"z\",A1:A4,B1:B4,\"none\")");
    assert_eq!(t.get("D4"), Value::from("none"));
    t.set("D5", "=XLOOKUP(\"z\",A1:A4,B1:B4)");
    assert_eq!(t.get("D5"), Value::Error(CellError::NA));
    // A match on an empty cell is a blank reference: ISBLANK is TRUE and & adds nothing (#374).
    t.set("A5", "e");
    t.set("D6", "=ISBLANK(XLOOKUP(\"e\",A1:A5,B1:B5))");
    assert_eq!(t.get("D6"), Value::Bool(true));
    t.set("D7", "=\"x\"&XLOOKUP(\"e\",A1:A5,B1:B5)&\"y\"");
    assert_eq!(t.get("D7"), Value::from("xy"));
    // An empty match_mode keeps the search_mode after it: search last to first.
    t.set("A6", "b");
    t.set("B6", "20");
    t.set("D8", "=XLOOKUP(\"b\",A1:A6,B1:B6,,,-1)");
    assert_eq!(t.num("D8"), 20.0);
    t.set("D9", "=XLOOKUP(\"b\",A1:A6,B1:B6)");
    assert_eq!(t.num("D9"), 2.0);
}

#[test]
fn long_chain() {
    let mut t = T::new();
    t.set("A1", "1");
    for r in 2..=3000 {
        t.set(&format!("A{r}"), &format!("=A{}+1", r - 1));
    }
    assert_eq!(t.num("A3000"), 3000.0);
    t.set("A1", "10");
    assert_eq!(t.num("A3000"), 3009.0);
}

#[test]
fn index_with_arrays_of_positions() {
    let mut t = T::new();
    for (i, row) in [[1, 2, 3], [4, 5, 6], [7, 8, 9]].iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            t.set(&format!("{}{}", ["A", "B", "C"][j], i + 1), &v.to_string());
        }
    }
    t.set("E1", "=INDEX(A1:C3,{1;3},{1,3})");
    assert_eq!([t.num("E1"), t.num("F1"), t.num("E2"), t.num("F2")], [1.0, 3.0, 7.0, 9.0]);
    t.set("E4", "=SUM(INDEX(A1:C3,{1,2,3},2))");
    assert_eq!(t.num("E4"), 15.0);
    t.set("E5", "=INDEX({10,20,30},{3,1})");
    assert_eq!([t.num("E5"), t.num("F5")], [30.0, 10.0]);
    t.set("E7", "=INDEX(A1:C3,2,3)");
    assert_eq!(t.num("E7"), 6.0);
}

#[test]
fn index_area_number_overflow() {
    let mut t = T::new();
    t.set("A1", "7");
    t.set("D1", "9");
    for (area, expected) in [("1", 7.0), ("2", 9.0)] {
        t.set("G5", &format!("=INDEX((A1:B2,D1:E2),1,1,{area})"));
        assert_eq!(t.num("G5"), expected, "area {area}");
    }
    for area in ["3", "1E300", "-1", "-1E300"] {
        t.set("G5", &format!("=INDEX((A1:B2,D1:E2),1,1,{area})"));
        assert_eq!(t.get("G5"), Value::Error(CellError::Ref), "area {area}");
    }
}

/// INDEX with one index, as observed in Excel: a 2-D reference needs both indexes (#REF!), a
/// column array gives a 1×1 array (TYPE 64), a row array or 1-D reference gives one value.
#[test]
fn index_with_one_index() {
    let mut t = T::new();
    for (i, v) in [10, 20, 30].iter().enumerate() {
        t.set(&format!("A{}", i + 1), &v.to_string());
    }
    let mut k = 1;
    for r in 5..=7 {
        for col in ["A", "B", "C"] {
            t.set(&format!("{col}{r}"), &k.to_string());
            k += 1;
        }
    }
    for (row, (formula, expected)) in [
        // Column arrays keep Excel's 1×1 array; row arrays and explicit columns give a value.
        ("=INDEX({10;20;30},2)", 20.0),
        ("=TYPE(INDEX({10;20;30},2))", 64.0),
        ("=TYPE(INDEX(SEQUENCE(3),2))", 64.0),
        ("=TYPE(INDEX({10;20;30},2,1))", 1.0),
        ("=TYPE(INDEX({10,20,30},2))", 1.0),
        ("=TYPE(INDEX(INDEX({10;20;30},2),1))", 1.0),
        ("=SUM(INDEX({10;20;30},2))", 20.0),
        // 1-D references with one index are single-cell references.
        ("=TYPE(INDEX(A1:A3,2))", 1.0),
        ("=SUM(INDEX(A1:A3,1):INDEX(A1:A3,3))", 60.0),
        ("=SUM(INDEX((A1:A3,A5:C7),2))", 20.0),
        // A 2-D array with one index is the row.
        ("=SUM(INDEX({1,2;3,4},2))", 7.0),
        ("=SUM(INDEX({1,2;3,4},0))", 10.0),
        // A 2-D reference with an empty (not omitted) column argument is the row.
        ("=SUM(INDEX(A5:C7,2,))", 15.0),
        ("=SUM(INDEX(A5:C7,2,,1))", 15.0),
        ("=COLUMNS(INDEX((A1:A3,A5:C7),2,,2))", 3.0),
        ("=INDEX(A5:C7,2,2)", 5.0),
        // A 2-D reference with one index is #REF!, and the error carries through.
        ("=TYPE(INDEX(A5:C7,2))", 16.0),
        ("=ISREF(INDEX(A5:C7,2))", 0.0),
    ]
    .iter()
    .enumerate()
    {
        let at = format!("J{}", row * 4 + 1);
        t.set(&at, formula);
        let got = match t.get(&at) {
            Value::Bool(b) => f64::from(u8::from(b)),
            v => v.as_f64().unwrap_or_else(|| panic!("{formula} = {v:?}")),
        };
        assert_eq!(got, *expected, "{formula}");
    }
    for formula in [
        "=INDEX(A5:C7,2)",
        "=INDEX(A5:C7,0)",
        "=INDEX(A5:C7,1.9)",
        "=INDEX(A5:C7,)",
        "=INDEX(A5:B6,1)",
        "=INDEX(A5:C7,2)+1",
        "=SUM(INDEX(A5:C7,{1;2}))",
        "=ROWS(INDEX(INDEX(A5:C7,2),3))",
        "=SUM(A5:INDEX(A5:C7,2))",
        "=SUM(INDEX(A5:C7,2):C7)",
    ] {
        t.set("L1", formula);
        assert_eq!(t.get("L1"), Value::Error(CellError::Ref), "{formula}");
    }
    t.set("L1", "=INDEX(A5:C7,{1;2})");
    assert_eq!([t.get("L1"), t.get("L2")], [Value::Error(CellError::Ref), Value::Error(CellError::Ref)]);
    t.set("L1", "=IFERROR(INDEX(A5:C7,2),\"e\")");
    assert_eq!(t.get("L1"), Value::from("e"));
    // A bad position is still #VALUE!.
    t.set("L1", "=INDEX(A5:C7,\"x\")");
    assert_eq!(t.get("L1"), Value::Error(CellError::Value));
}

#[test]
fn errors_and_cycles() {
    let mut t = T::new();
    t.set("A1", "=1/0");
    assert_eq!(t.get("A1"), Value::Error(CellError::Div0));
    t.set("B1", "=B2");
    t.set("B2", "=B1");
    assert!(t.get("B1").is_error());
    t.set("C1", "=NOSUCHFN(1)");
    assert_eq!(t.get("C1"), Value::Error(CellError::Name));
    // `_xludf.` marks a user-defined function, never the built-in of the same name (#726).
    for f in ["=_xludf.CONCAT(\"a\",\"b\")", "=_xludf.SUM(1,2)", "=_xludf.EOMONTH()"] {
        t.set("C3", f);
        assert_eq!(t.get("C3"), Value::Error(CellError::Name), "{f}");
    }
    t.set("C2", "=1+");
    assert_eq!(t.get("C2"), Value::Error(CellError::Name));
}

impl T {
    fn iterative(max_iterations: u32, max_change: f64) -> T {
        let mut t = T::new();
        t.wb.calc.iterative = true;
        t.wb.calc.set_max_iterations(max_iterations);
        t.wb.calc.set_max_change(max_change);
        t
    }
}

#[test]
fn iterative_calculation_stops_at_maximum_iterations() {
    // A1 =A1+1 never settles: every recalculation adds Maximum Iterations.
    let mut t = T::iterative(100, 0.001);
    t.set("A1", "=A1+1");
    assert_eq!(t.num("A1"), 100.0);
    assert!(t.calc.circular.is_empty());
    // An edit outside the loop doesn't recalculate it.
    t.set("B1", "7");
    assert_eq!(t.num("A1"), 100.0);
    // Each recalculation starts from the value the cell shows.
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A1"), 200.0);
    t.wb.calc.set_max_iterations(5);
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A1"), 205.0);
    // Through a range that holds the formula itself.
    t.set("C1", "1");
    t.set("C2", "2");
    t.set("C3", "=SUM(C1:C3)");
    assert_eq!(t.num("C3"), 15.0);
    // Excel's largest Maximum Iterations.
    let mut t = T::iterative(32767, 0.001);
    t.set("A1", "=A1+1");
    assert_eq!(t.num("A1"), 32767.0);
}

#[test]
fn iterative_calculation_converges_within_maximum_change() {
    let mut t = T::iterative(100, 0.001);
    t.set("A1", "=(A2+10)/2");
    t.set("A2", "=A1");
    assert!((t.num("A1") - 10.0).abs() < 0.002, "{}", t.num("A1"));
    assert!((t.num("A2") - 10.0).abs() < 0.002, "{}", t.num("A2"));
    assert!(t.calc.circular.is_empty());
    // A loop fed by a constant, and a formula that reads the loop, follow a change.
    t.set("D1", "10");
    t.set("A1", "=(A2+D1)/2");
    t.set("E1", "=A1*2");
    t.set("D1", "30");
    assert!((t.num("A1") - 30.0).abs() < 0.002, "{}", t.num("A1"));
    assert!((t.num("E1") - 60.0).abs() < 0.004, "{}", t.num("E1"));
    // Already settled: recalculating changes it by less than Maximum Change.
    let a1 = t.num("A1");
    t.calc.recalc_all(&mut t.wb);
    assert!((t.num("A1") - a1).abs() < 0.001);
}

#[test]
fn iterative_calculation_stops_once_changes_are_below_maximum_change() {
    // A1 halves its distance to 10 each pass, from 5: 7.5, 8.75, 9.375, 9.6875, and A2 trails
    // one pass behind. Maximum Change 1 stops once neither moved by 1 or more.
    let mut t = T::iterative(100, 1.0);
    t.set("A1", "=(A2+10)/2");
    assert_eq!(t.num("A1"), 5.0);
    t.set("A2", "=A1");
    assert_eq!((t.num("A1"), t.num("A2")), (9.6875, 9.375));
    // Maximum Iterations 2 stops first.
    let mut t = T::iterative(2, 1.0);
    t.set("A1", "=(A2+10)/2");
    t.set("A2", "=A1");
    assert_eq!((t.num("A1"), t.num("A2")), (8.75, 7.5));
}

#[test]
fn iterative_calculation_of_text_and_errors() {
    let mut t = T::iterative(100, 0.001);
    t.set("A1", "=IF(ISTEXT(A1),A1&\"\",\"go\")");
    assert_eq!(t.get("A1"), Value::from("go"));
    t.set("B1", "=B1+1/0");
    assert_eq!(t.get("B1"), Value::Error(CellError::Div0));
}

/// `&`, CONCAT and TEXTJOIN turn a number into at most 15 significant digits, decimal unless that
/// would need 16 integer digits or more than 21 characters below 1.
#[test]
fn text_joins_convert_numbers_to_15_significant_digits() {
    let mut t = T::new();
    for (formula, text) in [
        ("=(1E15-0.5)&\"\"", "1E+15"),
        ("=(999999999999999)&\"\"", "999999999999999"),
        ("=1234567890123456&\"\"", "1.23456789012346E+15"),
        ("=(9.99999999999999E-9)&\"\"", "9.99999999999999E-09"),
        ("=CONCAT(1/3*1E-5)", "3.33333333333333E-06"),
        ("=TEXTJOIN(\",\",TRUE,1/3*1E-4,0.1+0.2)", "0.0000333333333333333,0.3"),
    ] {
        t.set("A1", formula);
        assert_eq!(t.get("A1"), Value::from(text), "{formula}");
    }
}

#[test]
fn circular_references_without_iterative_calculation() {
    let mut t = T::new();
    t.set("A1", "=A1+1");
    assert_eq!(t.get("A1"), Value::Error(CellError::Circ));
    assert_eq!(t.calc.circular, vec![(0, c("A1"))]);
    // Switched on, the loop is calculated from 0 rather than from the error.
    t.wb.calc.iterative = true;
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A1"), 100.0);
    assert!(t.calc.circular.is_empty());
    t.wb.calc.iterative = false;
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.get("A1"), Value::Error(CellError::Circ));
}

/// The list of circular references keeps the loops that a recalculation of other cells does not
/// reach, and drops a loop once a constant is typed over one of its cells.
#[test]
fn circular_references_stay_listed_until_the_loop_is_removed() {
    let mut t = T::new();
    t.set("A1", "=A1+1");
    t.set("B1", "=B2");
    t.set("B2", "=B1");
    assert!(t.calc.circular.contains(&(0, c("A1"))), "{:?}", t.calc.circular);
    t.set("D4", "7");
    t.set("D5", "=D4*2");
    assert!(t.calc.circular.contains(&(0, c("A1"))), "{:?}", t.calc.circular);
    assert!(t.calc.circular.iter().any(|k| *k == (0, c("B1")) || *k == (0, c("B2"))), "{:?}", t.calc.circular);
    t.set("A1", "5");
    assert!(!t.calc.circular.contains(&(0, c("A1"))), "{:?}", t.calc.circular);
    assert!(t.calc.circular.iter().any(|k| *k == (0, c("B1")) || *k == (0, c("B2"))), "{:?}", t.calc.circular);
    t.set("B2", "3");
    assert!(t.calc.circular.is_empty(), "{:?}", t.calc.circular);
    // In manual calculation only the edited formulas are evaluated; a constant typed over the listed cell removes it.
    t.wb.calc.mode = gridcraft_model::CalcMode::Manual;
    t.set("C1", "=C1+1");
    assert_eq!(t.calc.circular, vec![(0, c("C1"))]);
    t.set("C1", "1");
    assert!(t.calc.circular.is_empty(), "{:?}", t.calc.circular);
}

#[test]
fn date_text_coerces_in_the_workbook_date_system() {
    for (sys, serial) in [(gridcraft_core::DateSystem::D1900, 1462.0), (gridcraft_core::DateSystem::D1904, 0.0)] {
        let mut t = T::new();
        t.wb.date_system = sys;
        t.set("A1", "'1/1/1904");
        t.set("B1", "=A1+0");
        t.set("C1", "=VALUE(A1)");
        t.set("D1", "=-A1");
        t.set("E1", "=DATEVALUE(A1)");
        t.set("F1", "=NUMBERVALUE(A1)");
        assert_eq!(t.get("A1"), Value::from("1/1/1904"));
        assert_eq!(t.num("B1"), serial, "{sys:?}");
        assert_eq!(t.num("C1"), serial, "{sys:?}");
        assert_eq!(t.num("D1"), -serial, "{sys:?}");
        assert_eq!(t.num("E1"), serial, "{sys:?}");
        assert_eq!(t.num("F1"), serial, "{sys:?}");
    }
}

/// Lookups compare numbers exactly while `=` and the criteria functions compare them at 15
/// significant digits. Every expectation was observed in Excel 365 with these fixtures
/// (`=0.1+0.2` is 0.30000000000000004, `=100+1E-14` is 100.00000000000001).
#[test]
fn lookups_compare_numbers_exactly() {
    let mut t = T::new();
    for (at, input) in [
        ("A1", "=0.1+0.2"),
        ("A2", "0.3"),
        ("A3", "=100+1E-14"),
        ("A4", "100"),
        ("A5", "=1E15+0.5"),
        ("A6", "1E15"),
        // Ascending 0.1, 0.3+e, 0.5 and the same descending.
        ("B1", "0.1"),
        ("B2", "=0.1+0.2"),
        ("B3", "0.5"),
        ("C1", "0.5"),
        ("C2", "=0.1+0.2"),
        ("C3", "0.1"),
        // 0.3 next to 0.3+e, ascending and descending.
        ("D1", "0.3"),
        ("D2", "=0.1+0.2"),
        ("D3", "0.7"),
        ("E1", "0.7"),
        ("E2", "=0.1+0.2"),
        ("E3", "0.3"),
        ("G1", "=0.1+0.2"),
        ("H1", "x"),
        ("G2", "row2"),
        // The bisection probes 0.3 first and must not run on into 0.3+e.
        ("J1", "0.1"),
        ("J2", "0.2"),
        ("J3", "0.3"),
        ("J4", "=0.1+0.2"),
        ("J5", "0.4"),
        ("K1", "a"),
        ("K2", "b"),
        ("K3", "c"),
        ("K4", "d"),
        ("K5", "e"),
    ] {
        t.set(at, input);
    }
    let e = 0.1 + 0.2;
    let n = Value::Number;
    let s = |x: &str| Value::from(x);
    let na = Value::Error(CellError::NA);
    let cases: Vec<(&str, Value)> = vec![
        // `=` and friends stay at 15 digits.
        ("=A1=A2", Value::Bool(true)),
        ("=A3=A4", Value::Bool(true)),
        ("=A5=A6", Value::Bool(true)),
        ("=SWITCH(A1,0.3,\"hit\",\"miss\")", s("hit")),
        // Exact match.
        ("=MATCH(A1,A2:A2,0)", na.clone()),
        ("=MATCH(A2,A1:A1,0)", na.clone()),
        ("=VLOOKUP(A1,A2:A2,1,FALSE)", na.clone()),
        ("=HLOOKUP(A1,A2,1,FALSE)", na.clone()),
        ("=HLOOKUP(0.3,G1:H2,2,FALSE)", na.clone()),
        ("=XLOOKUP(A1,A2:A2,A2:A2,\"nf\")", s("nf")),
        ("=XMATCH(A1,A2:A2)", na.clone()),
        ("=MATCH(0.1+0.2,{0.3},0)", na.clone()),
        ("=MATCH(0.3,A1:A2,0)", n(2.0)),
        ("=MATCH(A1,A1:A2,0)", n(1.0)),
        ("=MATCH(0.3,INDEX(A1:A2,0),0)", n(2.0)),
        ("=MATCH(100,A3:A3,0)", na.clone()),
        ("=MATCH(1E15,A5:A5,0)", na.clone()),
        ("=XLOOKUP(0.3,A1:A2,{1;2},\"nf\",0,-1)", n(2.0)),
        ("=XMATCH(A1,A1:A2,0,-1)", n(1.0)),
        // Approximate match against a single entry.
        ("=MATCH(0.3,A1:A1,1)", na.clone()),
        ("=MATCH(0.3,A1:A1,-1)", n(1.0)),
        ("=LOOKUP(0.3,A1:A1)", na.clone()),
        ("=LOOKUP(0.3,A1:A1,G2)", na.clone()),
        ("=VLOOKUP(0.3,A1:A1,1,TRUE)", na.clone()),
        ("=HLOOKUP(0.3,G1:H2,2,TRUE)", na.clone()),
        ("=XLOOKUP(0.3,A1:A1,A1:A1,\"nf\",-1)", s("nf")),
        ("=XLOOKUP(0.3,A1:A1,A1:A1,\"nf\",1)", n(e)),
        ("=XMATCH(0.3,A1:A1,-1)", na.clone()),
        ("=XMATCH(0.3,A1:A1,1)", n(1.0)),
        ("=MATCH(100,A3:A3,1)", na.clone()),
        ("=MATCH(A3,A4:A4,1)", n(1.0)),
        // Approximate match in sorted lists.
        ("=MATCH(0.3,B1:B3,1)", n(1.0)),
        ("=MATCH(A1,B1:B3,1)", n(2.0)),
        ("=MATCH(0.3,C1:C3,-1)", n(2.0)),
        ("=MATCH(0.3,D1:D3,1)", n(1.0)),
        ("=MATCH(A1,D1:D3,1)", n(2.0)),
        ("=MATCH(0.3,E1:E3,-1)", n(3.0)),
        ("=MATCH(A1,E1:E3,-1)", n(2.0)),
        ("=MATCH(0.3,J1:J5,1)", n(3.0)),
        ("=MATCH(A1,J1:J5,1)", n(4.0)),
        ("=MATCH(0.3,J1:J5,0)", n(3.0)),
        ("=MATCH(A1,J1:J5,0)", n(4.0)),
        ("=VLOOKUP(0.3,J1:K5,2,TRUE)", s("c")),
        ("=VLOOKUP(A1,J1:K5,2,TRUE)", s("d")),
        ("=LOOKUP(0.3,J1:J5,K1:K5)", s("c")),
        ("=LOOKUP(A1,J1:J5,K1:K5)", s("d")),
        // XLOOKUP / XMATCH match and search modes.
        ("=XLOOKUP(0.3,B1:B3,B1:B3,\"nf\",0,2)", s("nf")),
        ("=XLOOKUP(0.3,B1:B3,B1:B3,\"nf\",-1,2)", n(0.1)),
        ("=XLOOKUP(0.3,B1:B3,B1:B3,\"nf\",1,2)", n(e)),
        ("=XMATCH(0.3,B1:B3,-1,2)", n(1.0)),
        ("=XMATCH(0.3,B1:B3,1,2)", n(2.0)),
        ("=XMATCH(0.3,C1:C3,1,-2)", n(2.0)),
        ("=XMATCH(0.3,C1:C3,-1,-2)", n(3.0)),
        ("=XMATCH(0.3,C1:C3,0,-2)", na.clone()),
        ("=XMATCH(0.3,J1:J5,0,2)", n(3.0)),
        ("=XMATCH(A1,J1:J5,0,2)", n(4.0)),
        ("=XMATCH(0.3,J1:J5,1,2)", n(3.0)),
        ("=XMATCH(0.3,J1:J5,-1)", n(3.0)),
        ("=XMATCH(0.3,J1:J5,1)", n(3.0)),
        ("=XMATCH(A1,J1:J5,-1)", n(4.0)),
        ("=XMATCH(0.3,B1:B3,1)", n(2.0)),
        ("=XMATCH(0.3,B1:B3,-1)", n(1.0)),
        ("=XMATCH(0.3,B1:B3,0,-1)", na.clone()),
        ("=XMATCH(0.3,D1:D3,1,-1)", n(1.0)),
        // The criteria functions keep 15 digits.
        ("=COUNTIF(A1:A1,0.3)", n(1.0)),
        ("=COUNTIF(A1:A1,\"<0.3\")", n(0.0)),
        ("=COUNTIF(A1:A1,\">0.3\")", n(0.0)),
        ("=COUNTIF(A2:A2,A1)", n(1.0)),
        ("=COUNTIFS(A1:A1,0.3)", n(1.0)),
        ("=SUMIF(A1:A1,0.3)", n(e)),
        ("=SUMIFS(A1:A1,A1:A1,0.3)", n(e)),
        ("=AVERAGEIF(A1:A1,0.3)", n(e)),
        ("=MAXIFS(A1:A1,A1:A1,0.3)", n(e)),
        ("=COUNTIF(A3:A3,\">100\")", n(0.0)),
        ("=COUNTIF(J1:J5,0.3)", n(2.0)),
        ("=ROWS(FILTER(A1:A2,A1:A2=0.3))", n(2.0)),
        ("=MATCH(TRUE,A1:A2=0.3,0)", n(1.0)),
    ];
    let mut wrong = Vec::new();
    for (formula, expected) in cases {
        let got = crate::evaluate(&t.wb, 0, c("Z1"), formula);
        if got != expected {
            wrong.push(format!("{formula}: got {got:?}, Excel {expected:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn lambda_helpers_check_shapes_and_parameters() {
    let mut t = T::new();
    let calc = Value::Error(CellError::Calc);
    // A LAMBDA returning several values can't fill one cell: the whole result is #CALC!.
    t.set("A1", "=MAP({1,2},LAMBDA(x,{1,2}))");
    assert_eq!((t.get("A1"), t.get("B1")), (calc.clone(), Value::Empty));
    t.set("A2", "=BYROW({1,2;3,4},LAMBDA(r,r))");
    assert_eq!((t.get("A2"), t.get("A3")), (calc.clone(), Value::Empty));
    t.set("A4", "=MAKEARRAY(1,2,LAMBDA(r,c,{1,2}))");
    assert_eq!(t.get("A4"), calc.clone());
    t.set("D1", "=SCAN(0,{1,2},LAMBDA(a,b,{1,2}))");
    assert_eq!((t.get("D1"), t.get("E1")), (calc, Value::Empty));
    // One parameter per array (MAP) or exactly one (BYROW/BYCOL).
    t.set("A5", "=MAP({1,2},LAMBDA(x,y,x))");
    assert_eq!(t.get("A5"), Value::Error(CellError::Value));
    t.set("A6", "=BYCOL({1,2},LAMBDA(c,d,SUM(c)))");
    assert_eq!(t.get("A6"), Value::Error(CellError::Value));
    // A single value in an array is fine.
    t.set("A7", "=MAP({1,2},LAMBDA(x,SEQUENCE(1)*x))");
    assert_eq!(t.num("B7"), 2.0);
}

#[test]
fn lambda_calls_spend_one_work_budget_per_cell() {
    // #2382: lambda nesting multiplies invocations, and the budget meters every one of them.
    // Exhaustion itself is tested on a mock host in `eval::fuel_tests` (burning the real 16M
    // budget in a full recalc needs tens of millions of calls, far too slow for a unit test);
    // here the same nested shape runs inside it: 6 × 9 ≈ 59k scalar REDUCE invocations.
    let mut t = T::new();
    t.set(
        "A1",
        "=LET(s,SEQUENCE(9),REDUCE(0,s,LAMBDA(a,v,REDUCE(0,s,LAMBDA(a,v,REDUCE(0,s,LAMBDA(a,v,REDUCE(0,s,LAMBDA(a,v,REDUCE(0,s,LAMBDA(a,v,a+v)))))))))))",
    );
    // The bodies are the same inner reduce, so every level sums to 45 while the invocation
    // count multiplies by 9 per level (9, 81, 729, 6561, 59049 calls).
    assert_eq!(t.num("A1"), 45.0);
    t.set("A2", "=REDUCE(0,SEQUENCE(4000),LAMBDA(a,v,a+v))");
    assert_eq!(t.num("A2"), 4000.0 * 4001.0 / 2.0);
}

#[test]
fn lifted_lookups_stop_past_their_work_budget() {
    // Security: each needle searched the whole lookup vector, so this one cell ran for hours
    // (about 1.1e12 compares) and hung opening the file that held it. It is #NUM! at once now.
    let mut t = T::new();
    t.set("A1", "=ROWS(XMATCH(SEQUENCE(1048576),-SEQUENCE(1048576)))");
    assert_eq!(t.get("A1"), Value::Error(CellError::Num));
    // Ordinary lifted lookups are unaffected.
    for (i, k) in [10, 20, 30, 40, 50].into_iter().enumerate() {
        t.set(&format!("B{}", i + 1), &k.to_string());
        t.set(&format!("C{}", i + 1), &(i + 1).to_string());
    }
    t.set("D1", "=SUM(XMATCH({30;50;10},B1:B5))");
    t.set("D2", "=SUM(XLOOKUP({20;40},B1:B5,C1:C5))");
    t.set("D3", "=SUM(VLOOKUP({30,10},B1:C5,2,FALSE))");
    t.set("D4", "=SUM(MATCH({25,50},B1:B5))");
    assert_eq!([t.num("D1"), t.num("D2"), t.num("D3"), t.num("D4")], [9.0, 6.0, 4.0, 7.0]);
}

#[test]
fn accounting_negative_text_coerces() {
    let mut t = T::new();
    t.set("A1", "'$(1,234)");
    t.set("B1", "=A1+0");
    t.set("C1", "=VALUE(A1)");
    t.set("D1", "=--\"$ (5)\"");
    t.set("E1", "=\"(5)%\"*100");
    t.set("F1", "=\"(-5)\"+0");
    t.set("G1", "$(1,234)");
    assert_eq!((t.num("B1"), t.num("C1"), t.num("D1"), t.num("E1")), (-1234.0, -1234.0, -5.0, -5.0));
    assert_eq!(t.get("F1"), Value::Error(CellError::Value));
    assert_eq!(t.num("G1"), -1234.0);
}

#[test]
fn operators() {
    let mut t = T::new();
    t.set("A1", "=\"a\"&1&TRUE");
    assert_eq!(t.get("A1"), Value::from("a1TRUE"));
    t.set("A2", "=-2^2");
    assert_eq!(t.num("A2"), 4.0);
    t.set("A3", "=50%");
    assert_eq!(t.num("A3"), 0.5);
    t.set("A4", "=\"abc\"=\"ABC\"");
    assert_eq!(t.get("A4"), Value::Bool(true));
    t.set("A5", "=\"5\"+1");
    assert_eq!(t.num("A5"), 6.0);
    t.set("A6", "=\"x\"+1");
    assert_eq!(t.get("A6"), Value::Error(CellError::Value));
    t.set("A7", "=B99");
    assert_eq!(t.num("A7"), 0.0);
}

#[test]
fn blocked_spill_reads_as_spill_error() {
    // Calculated in the same pass, a formula over a blocked spill read the array that couldn't
    // spill.
    let mut t = T::new();
    let s = t.wb.sheet_mut(0).unwrap();
    for a in ["A1", "B1", "C1", "A2", "C2", "A3", "B3", "C3"] {
        s.cells.set(c(a), Cell::value(Value::Number(1.0)));
    }
    s.cells.set(c("B2"), Cell::formula(Formula::new("=SEQUENCE(2,2,9)")));
    s.cells.set(c("E1"), Cell::formula(Formula::new("=SUM(A1:C3)")));
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.get("B2"), Value::Error(CellError::Spill));
    assert_eq!(t.get("E1"), Value::Error(CellError::Spill));
    assert_eq!(t.num("C3"), 1.0);
}

#[test]
fn blocked_spill_read_through_offset() {
    // The reader's reference is only known while evaluating, so it isn't in the dependency graph.
    let mut t = T::new();
    t.set("E4", "5");
    t.set("E8", "=SUM(OFFSET(E2,0,0,2,2))");
    t.set("E3", "=SEQUENCE(2,2,3)");
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.get("E3"), Value::Error(CellError::Spill));
    assert_eq!(t.get("E8"), Value::Error(CellError::Spill));
}

#[test]
fn spill_into_the_old_area_of_a_blocked_neighbour() {
    // C7's array becomes blocked by C8, whose array then takes over C7's old area.
    let mut t = T::new();
    t.set("C7", "=SEQUENCE(2,2)");
    t.set("C8", "=SEQUENCE(2,2)");
    assert_eq!(t.get("C7"), Value::Error(CellError::Spill));
    assert_eq!([t.num("C8"), t.num("D8"), t.num("C9"), t.num("D9")], [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn a_full_recalc_keeps_a_spill_from_an_array_that_grew_into_it() {
    // Both arrays spill; in manual calculation F13 then grows over F14:G14, which C14's array
    // fills. A full recalculation gives what an automatic one would: C14 keeps its cells and F13
    // is #SPILL!, although F13 comes first in reading order.
    let mut t = T::new();
    t.set("J1", "1");
    t.set("C14", "=SEQUENCE(1,7)");
    t.set("F13", "=SEQUENCE(J1,2)");
    assert_eq!([t.num("F13"), t.num("G13")], [1.0, 2.0]);
    t.wb.calc.mode = gridcraft_model::CalcMode::Manual;
    t.set("J1", "5");
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.get("F13"), Value::Error(CellError::Spill));
    assert_eq!([t.num("C14"), t.num("F14"), t.num("G14"), t.num("I14")], [1.0, 4.0, 5.0, 7.0]);
}

#[test]
fn a_full_recalc_lays_out_arrays_as_an_automatic_one_when_one_shrinks_for_another() {
    // C1 grows into C3, which A3's array fills, and A3 grows into E3, which E1's array leaves.
    // Automatic recalculation lays them out in reading order with each former area still filled:
    // C1 is #SPILL! and A3 takes A3:E3. A full recalculation after the same edit in manual
    // calculation gives the same sheet.
    let build = |manual: bool| {
        let mut t = T::new();
        t.set("J1", "2");
        t.set("C1", "=SEQUENCE(J1)");
        t.set("A3", "=SEQUENCE(1,J1+1)");
        t.set("E1", "=SEQUENCE(5-J1)");
        assert_eq!([t.num("C2"), t.num("C3"), t.num("E3")], [2.0, 3.0, 3.0]);
        if manual {
            t.wb.calc.mode = gridcraft_model::CalcMode::Manual;
        }
        t.set("J1", "4");
        if manual {
            t.calc.recalc_all(&mut t.wb);
        }
        t
    };
    let (automatic, full) = (build(false), build(true));
    assert_eq!(automatic.get("C1"), Value::Error(CellError::Spill));
    assert_eq!([automatic.num("A3"), automatic.num("C3"), automatic.num("E3")], [1.0, 3.0, 5.0]);
    for cell in ["A1", "C1", "C2", "C3", "C4", "E1", "E2", "A3", "B3", "C3", "D3", "E3"] {
        assert_eq!(full.get(cell), automatic.get(cell), "{cell}");
    }
}

#[test]
fn formulas_reading_an_array_that_loses_a_spill_conflict_read_spill_errors() {
    // Two arrays of one recalculation want the same empty cell: the one laid out second is
    // #SPILL!, and the formulas that read it in that pass (at its anchor, through `A2#`, or over
    // the cells it would have spilled into) kept its values.
    let mut t = T::new();
    t.set("B1", "=SEQUENCE(3)");
    t.set("A2", "=SEQUENCE(1,3)");
    t.set("D2", "=A2*10");
    t.set("E2", "=C2");
    t.set("F2", "=SUM(A2#)");
    t.set("G2", "=SUM(C2:C3)");
    // Both arrays recalculated together: B1 is laid out first and takes B2.
    t.calc.recalc_all(&mut t.wb);
    let spill = Value::Error(CellError::Spill);
    assert_eq!((t.num("B2"), t.get("A2"), t.get("D2")), (2.0, spill.clone(), spill));
    assert_eq!((t.get("C2"), t.num("E2"), t.get("F2"), t.num("G2")), (Value::Empty, 0.0, Value::Error(CellError::Ref), 0.0));
}

#[test]
fn arrays_do_not_spill_in_tables() {
    // Excel: spilled array formulas aren't supported in tables.
    let mut t = T::new();
    t.wb.sheet_mut(0).unwrap().tables.push(gridcraft_model::Table {
        id: 1,
        name: "Table1".into(),
        range: gridcraft_core::RangeRef::parse("B3:C6").unwrap(),
        header_row: true,
        totals_row: false,
        columns: vec![],
        style: String::new(),
        banded_rows: false,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: false,
        filter: vec![],
        table_type: None,
        connection_id: None,
    });
    t.set("C4", "=SEQUENCE(2)");
    t.set("E1", "=SEQUENCE(4)");
    t.set("H1", "=SUM(C4:C5)");
    assert_eq!(t.get("C4"), Value::Error(CellError::Spill));
    assert_eq!(t.get("C5"), Value::Empty);
    assert_eq!(t.get("H1"), Value::Error(CellError::Spill));
    assert_eq!(t.num("E4"), 4.0);
}

#[test]
fn ifs_and_lazy() {
    let mut t = T::new();
    t.set("A1", "5");
    t.set("B1", "=IF(A1>3,\"big\",1/0)");
    assert_eq!(t.get("B1"), Value::from("big"));
    t.set("B2", "=IFERROR(1/0,\"oops\")");
    assert_eq!(t.get("B2"), Value::from("oops"));
    t.set("B3", "=CHOOSE(2,\"a\",\"b\",\"c\")");
    assert_eq!(t.get("B3"), Value::from("b"));
    t.set("B4", "=SWITCH(A1,1,\"one\",5,\"five\",\"other\")");
    assert_eq!(t.get("B4"), Value::from("five"));
}

#[test]
fn choose_with_an_array_index() {
    let mut t = T::new();
    for (i, (k, v)) in [(1, 10), (2, 20), (3, 30)].iter().enumerate() {
        t.set(&format!("A{}", i + 1), &k.to_string());
        t.set(&format!("B{}", i + 1), &v.to_string());
    }
    t.set("D1", "=CHOOSE({1,2},\"a\",\"b\")");
    assert_eq!(t.get("D1"), Value::from("a"));
    assert_eq!(t.get("E1"), Value::from("b"));
    // The usual "look left" idiom: a two-column array from two ranges.
    t.set("D3", "=VLOOKUP(20,CHOOSE({1,2},B1:B3,A1:A3),2,FALSE)");
    assert_eq!(t.num("D3"), 2.0);
    t.set("D4", "=SUM(CHOOSE({1,2},A1:A3,B1:B3))");
    assert_eq!(t.num("D4"), 66.0);
    t.set("D5", "=CHOOSE(2,A1:A3,B1:B3)");
    assert_eq!(t.num("D7"), 30.0);
}

#[test]
fn references_functions() {
    let mut t = T::new();
    for (i, v) in [10, 20, 30, 40].iter().enumerate() {
        t.set(&format!("A{}", i + 1), &v.to_string());
    }
    t.set("B1", "=ROW(A3)");
    assert_eq!(t.num("B1"), 3.0);
    t.set("B2", "=ROWS(A1:A4)*COLUMNS(A1:C1)");
    assert_eq!(t.num("B2"), 12.0);
    t.set("B3", "=SUM(OFFSET(A1,1,0,2,1))");
    assert_eq!(t.num("B3"), 50.0);
    t.set("B4", "=INDIRECT(\"A\"&4)");
    assert_eq!(t.num("B4"), 40.0);
    // With a1 = FALSE the text must be R1C1: A1-style text is #REF!, not read as A1.
    t.set("C1", "=INDIRECT(\"R3C1\",FALSE)");
    assert_eq!(t.num("C1"), 30.0);
    t.set("C2", "=INDIRECT(\"A3\",FALSE)");
    assert_eq!(t.get("C2"), Value::Error(CellError::Ref));
    // Defined names don't depend on the reference style.
    t.wb.names.push(gridcraft_model::DefinedName {
        name: "Third".into(),
        scope: None,
        formula: "Sheet1!$A$3".into(),
        comment: String::new(),
        hidden: false,
    });
    t.calc.rebuild(&t.wb);
    t.set("C3", "=INDIRECT(\"Third\",FALSE)");
    assert_eq!(t.num("C3"), 30.0);
    t.set("B5", "=SUM(A1:INDEX(A1:A4,3))");
    assert_eq!(t.num("B5"), 60.0);
    t.set("B6", "=SUM(A:A)");
    assert_eq!(t.num("B6"), 100.0);
    t.set("A2", "25");
    assert_eq!(t.num("B3"), 55.0);
    assert_eq!(t.num("B6"), 105.0);
}

#[test]
fn indirect_r1c1_out_of_bounds() {
    for reference in ["R[9223372036854775807]C", "RC[9223372036854775807]", "R[4294967296]C", "R[4294967291]C", "R[2000000]C", "RC[20000]"] {
        let mut t = T::new();
        t.set("D5", &format!("=ROW(INDIRECT(\"{reference}\",FALSE))"));
        assert_eq!(t.get("D5"), Value::Error(CellError::Ref), "{reference}");
    }
}

#[test]
fn indirect_r1c1_past_the_sheet_does_not_fall_back_to_a1() {
    let mut t = T::new();
    t.set("RC16385", "78");
    for reference in ["RC16385", "rc16385", "RC1:RC16385"] {
        t.set("D5", &format!("=INDIRECT(\"{reference}\",FALSE)"));
        assert_eq!(t.get("D5"), Value::Error(CellError::Ref), "{reference}");
    }
}

#[test]
fn indirect_r1c1_valid_references() {
    let mut t = T::new();
    t.set("E6", "11");
    t.set("A1", "22");
    t.set("D4", "33");
    for (reference, expected) in [("R[1]C[1]", 11.0), ("R1C1", 22.0), ("R[-1]C", 33.0)] {
        t.set("D5", &format!("=INDIRECT(\"{reference}\",FALSE)"));
        assert_eq!(t.num("D5"), expected, "{reference}");
    }
}

#[test]
fn relative_names() {
    let mut t = T::new();
    // As files store them: relative to A1. `Left` is the cell to the left, `Above3` the three
    // cells above, `Mixed` column A of the same row.
    for (name, formula) in [("Left", "Sheet1!XFD1"), ("Above3", "Sheet1!A1048574:A1048576"), ("Mixed", "Sheet1!$A1")] {
        t.wb.names.push(gridcraft_model::DefinedName {
            name: name.into(),
            scope: None,
            formula: formula.into(),
            comment: String::new(),
            hidden: false,
        });
    }
    t.calc.rebuild(&t.wb);
    for r in 1..=5 {
        t.set(&format!("A{r}"), &(r * 10).to_string());
    }
    t.set("B2", "=Left");
    assert_eq!(t.num("B2"), 20.0);
    t.set("B4", "=Left+Mixed");
    assert_eq!(t.num("B4"), 80.0);
    t.set("A6", "=SUM(Above3)");
    assert_eq!(t.num("A6"), 120.0);
}

#[test]
fn spills() {
    let mut t = T::new();
    t.set("A1", "=SEQUENCE(3)");
    assert_eq!(t.num("A1"), 1.0);
    assert_eq!(t.num("A3"), 3.0);
    t.set("B1", "=SUM(A1:A3)");
    assert_eq!(t.num("B1"), 6.0);
    t.set("A2", "x");
    assert_eq!(t.get("A1"), Value::Error(CellError::Spill));
    t.set("A2", "");
    assert_eq!(t.num("A3"), 3.0);
    t.set("C1", "={1,2;3,4}*10");
    assert_eq!(t.num("D2"), 40.0);
}

#[test]
fn an_anchor_given_a_formula_that_reads_its_old_spill_area_sees_it_empty() {
    // The new formula was evaluated while the old array was still laid out, and only the
    // anchor's dependents were evaluated again once it was gone.
    let mut t = T::new();
    t.set("A1", "=SEQUENCE(3)");
    assert_eq!(t.num("A3"), 3.0);
    t.set("A1", "=SUM(A2:A3)+1");
    assert_eq!((t.num("A1"), t.get("A3")), (1.0, Value::Empty));
    // Still an array, now spilling elsewhere: it read A2 as 2 and spilled three columns.
    t.set("A1", "=SEQUENCE(3)");
    t.set("A1", "=SEQUENCE(1,A2+1)");
    assert_eq!((t.num("A1"), t.get("B1"), t.get("A2")), (1.0, Value::Empty, Value::Empty));
    // A smaller array inside the old area: it read A3 as 3 and spilled over A1:A2.
    t.set("A1", "=SEQUENCE(3)");
    t.set("A1", "=SEQUENCE(MAX(1,A3-1))");
    assert_eq!((t.num("A1"), t.get("A2"), t.get("A3")), (1.0, Value::Empty, Value::Empty));
}

#[test]
fn references_keep_their_size_past_the_used_range() {
    let mut t = T::new();
    t.set("A1", "5");
    t.set("A2", "6");
    t.set("C1", "=ROWS(A1:A10*1)");
    t.set("C2", "=SUMPRODUCT(--(A1:A10=\"\"))");
    t.set("C3", "=COUNTBLANK(A1:A10)");
    t.set("C4", "=COUNTIF(A1:A10,\"\")");
    t.set("C5", "=COLUMNS(TRANSPOSE(A1:A10))");
    for (at, want) in [("C1", 10.0), ("C2", 8.0), ("C3", 8.0), ("C4", 8.0), ("C5", 10.0)] {
        assert_eq!(t.num(at), want, "{at}");
    }
    // Whole columns are still read up to the used range.
    t.set("D1", "=ROWS(FILTER(A:A,A:A<>\"\"))");
    assert_eq!(t.num("D1"), 2.0);
}

#[test]
fn spilled_blank_cells_show_zero() {
    let mut t = T::new();
    t.set("A1", "5");
    t.set("A3", "7");
    t.set("C1", "=A1:A4");
    assert_eq!(t.get("C2"), Value::Number(0.0));
    assert_eq!(t.get("C4"), Value::Number(0.0));
    t.set("D1", "=COUNT(C1:C4)");
    assert_eq!(t.num("D1"), 4.0);
    t.set("E1", "=A2:A3");
    assert_eq!(t.get("E1"), Value::Number(0.0));
    t.set("F1", "=SORT(A1:A3)");
    assert_eq!(t.get("F3"), Value::Number(0.0));
}

#[test]
fn let_lambda() {
    let mut t = T::new();
    t.set("A1", "=LET(x,2,y,3,x*y)");
    assert_eq!(t.num("A1"), 6.0);
    t.set("A2", "=LAMBDA(a,b,a+b)(2,3)");
    assert_eq!(t.num("A2"), 5.0);
    t.set("A3", "=REDUCE(0,{1,2,3},LAMBDA(acc,v,acc+v))");
    assert_eq!(t.num("A3"), 6.0);
    t.set("A4", "=SUM(MAP({1,2,3},LAMBDA(v,v*v)))");
    assert_eq!(t.num("A4"), 14.0);
}

#[test]
fn let_lambda_names_ignore_case_in_any_script() {
    let mut t = T::new();
    t.set("A1", "=LET(dług,100,DŁUG*2)");
    assert_eq!(t.num("A1"), 200.0);
    t.set("A2", "=LAMBDA(λ,ISOMITTED(Λ))()");
    assert_eq!(t.get("A2"), Value::Bool(true));
    t.set("A3", "=LAMBDA(ćma,ĆMA+1)(1)");
    assert_eq!(t.num("A3"), 2.0);
    t.set("A4", "=LET(ŻAR,LAMBDA(x,x*3),żar(2))");
    assert_eq!(t.num("A4"), 6.0);
    t.set("A5", "=LET(λ,LAMBDA(x,x+1),SUM(MAP({1,2},Λ)))");
    assert_eq!(t.num("A5"), 5.0);
    // `ß` upper-cases to `SS`, yet `ẞ` is its capital.
    t.set("A6", "=LET(ß,100,ẞ*2)");
    assert_eq!(t.num("A6"), 200.0);
    // Dotless ı is a different name from i.
    t.set("A7", "=LET(i,1,ı,2,i)");
    assert_eq!(t.num("A7"), 1.0);
}

#[test]
fn functions_given_by_name() {
    let mut t = T::new();
    for (i, (k, v)) in [("a", 1), ("b", -2), ("a", 3)].iter().enumerate() {
        t.set(&format!("A{}", i + 1), k);
        t.set(&format!("B{}", i + 1), &v.to_string());
    }
    t.set("D1", "=BYROW(B1:B3,SUM)");
    assert_eq!(t.num("D2"), -2.0);
    t.set("E1", "=MAP(B1:B3,ABS)");
    assert_eq!(t.num("E2"), 2.0);
    t.set("F1", "=REDUCE(0,B1:B3,SUM)");
    assert_eq!(t.num("F1"), 2.0);
    t.set("G1", "=GROUPBY(A1:A3,B1:B3,SUM,0,0)");
    assert_eq!(t.get("G1"), Value::from("a"));
    assert_eq!(t.num("H1"), 4.0);
    // A LET variable of that name is still the variable.
    t.set("J1", "=LET(ABS,LAMBDA(x,x*10),SUM(MAP(B1:B3,ABS)))");
    assert_eq!(t.num("J1"), 20.0);
}

#[test]
fn groupby_and_pivotby_take_a_lambda() {
    let mut t = T::new();
    for (i, (k, c, v)) in [("a", "x", 1), ("b", "x", 2), ("a", "y", 3)].iter().enumerate() {
        t.set(&format!("A{}", i + 1), k);
        t.set(&format!("B{}", i + 1), c);
        t.set(&format!("C{}", i + 1), &v.to_string());
    }
    t.set("E1", "=GROUPBY(A1:A3,C1:C3,LAMBDA(x,SUM(x)))");
    assert_eq!(t.get("E1"), Value::from("a"));
    assert_eq!(t.num("F1"), 4.0);
    assert_eq!(t.get("E2"), Value::from("b"));
    assert_eq!(t.num("F2"), 2.0);
    assert_eq!(t.get("E3"), Value::from("Total"));
    assert_eq!(t.num("F3"), 6.0);
    // A LET-bound lambda, called once per group with that group's values.
    t.set("H1", "=LET(f,LAMBDA(v,ROWS(v)*10),GROUPBY(A1:A3,C1:C3,f))");
    assert_eq!(t.num("I1"), 20.0);
    assert_eq!(t.num("I3"), 30.0);
    // PIVOTBY: rows a, b, Total; columns x, y, Total.
    t.set("K1", "=PIVOTBY(A1:A3,B1:B3,C1:C3,LAMBDA(x,SUM(x)))");
    assert_eq!(t.get("L1"), Value::from("x"));
    assert_eq!(t.num("L2"), 1.0);
    assert_eq!(t.num("M2"), 3.0);
    assert_eq!(t.num("N4"), 6.0);
    // Neither a function nor a lambda.
    t.set("P1", "=GROUPBY(A1:A3,C1:C3,TRUE)");
    assert_eq!(t.get("P1"), Value::Error(CellError::Value));
}

#[test]
fn lambda_argument_count() {
    let mut t = T::new();
    t.wb.names.push(gridcraft_model::DefinedName {
        name: "AddPair".into(),
        scope: None,
        formula: "LAMBDA(a,b,a+b)".into(),
        comment: String::new(),
        hidden: false,
    });
    t.calc.rebuild(&t.wb);
    let omit = "LAMBDA(x,y,IF(ISOMITTED(y),\"Missing\",x+y))";
    t.set("A1", "=LAMBDA(a,b,a+b)(2)");
    t.set("A2", "=LAMBDA(a,b,a+b)(2,3,4)");
    t.set("A3", &format!("={omit}(1,)"));
    t.set("A4", &format!("={omit}(1)"));
    t.set("A5", &format!("={omit}(1,2)"));
    t.set("A6", "=LAMBDA(7)()");
    t.set("A7", "=AddPair(2)");
    t.set("A8", "=AddPair(2,3)");
    assert_eq!(t.get("A1"), Value::Error(CellError::Value));
    assert_eq!(t.get("A2"), Value::Error(CellError::Value));
    assert_eq!(t.get("A3"), Value::Text("Missing".into()));
    assert_eq!(t.get("A4"), Value::Error(CellError::Value));
    assert_eq!(t.num("A5"), 3.0);
    assert_eq!(t.num("A6"), 7.0);
    assert_eq!(t.get("A7"), Value::Error(CellError::Value));
    assert_eq!(t.num("A8"), 5.0);
    t.set("A9", "=LAMBDA(x,ISOMITTED(x))()");
    assert_eq!(t.get("A9"), Value::Bool(true));
}

#[test]
fn a_lambda_that_calls_itself_without_end_is_num_on_a_small_stack() {
    // 1 MiB is the main thread's stack on Windows, where commands, recalculation and file open run.
    let handle = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let mut t = T::new();
            t.wb.names.push(gridcraft_model::DefinedName {
                name: "LOOP".into(),
                scope: None,
                formula: "LAMBDA(n,LOOP(n))".into(),
                comment: String::new(),
                hidden: false,
            });
            t.calc.rebuild(&t.wb);
            let endless = "=LET(f,LAMBDA(g,x,g(g,x)),f(f,1))";
            t.set("A1", endless);
            t.set("A2", "=LOOP(1)");
            t.set("A3", "=LET(F,LAMBDA(me,n,IF(n<=0,0,1+SUM(MAP({1},LAMBDA(v,me(me,n-1)))))),F(F,100000))");
            t.set("A4", "=LET(F,LAMBDA(me,n,IF(n<=0,0,1+me(me,n-1))),F(F,131))");
            let evaluated = crate::evaluate(&t.wb, 0, c("B1"), endless);
            [t.get("A1"), t.get("A2"), t.get("A3"), evaluated, t.get("A4")]
        })
        .expect("spawn");
    let [endless, endless_name, through_map, evaluated, countdown] = handle.join().expect("recursion must not overflow the stack");
    assert_eq!(endless, Value::Error(CellError::Num));
    assert_eq!(endless_name, Value::Error(CellError::Num));
    assert_eq!(through_map, Value::Error(CellError::Num));
    assert_eq!(evaluated, Value::Error(CellError::Num));
    assert_eq!(countdown, Value::Number(131.0));
}

#[test]
fn a_recursive_lambda_stops_at_the_same_call_on_any_stack_size() {
    let run = |stack: usize| {
        std::thread::Builder::new()
            .stack_size(stack)
            .spawn(|| {
                let mut wb = Workbook::new();
                wb.names.push(gridcraft_model::DefinedName {
                    name: "CNT".into(),
                    scope: None,
                    formula: "LAMBDA(n,IF(n=0,0,1+CNT(n-1)))".into(),
                    comment: String::new(),
                    hidden: false,
                });
                // F(F,n) has n + 1 calls in progress at its deepest; W adds one more call per step.
                let tail = |n: u32| format!("=LET(F,LAMBDA(me,n,IF(n=0,0,me(me,n-1))),F(F,{n}))");
                let wrapped = |n: u32| format!("=LET(W,LAMBDA(f,k,f(f,k)),F,LAMBDA(me,n,IF(n<=0,0,1+W(me,n-1))),F(F,{n}))");
                [tail(139), tail(140), wrapped(69), wrapped(70), "=CNT(131)".to_string()].map(|f| crate::evaluate(&wb, 0, c("B1"), &f))
            })
            .expect("spawn")
            .join()
            .expect("recursion must not overflow the stack")
    };
    let small = run(1 << 20);
    assert_eq!(small, [Value::Number(0.0), Value::Error(CellError::Num), Value::Number(69.0), Value::Error(CellError::Num), Value::Number(131.0)]);
    assert_eq!(run(64 << 20), small);
}

#[test]
fn a_recursive_lambda_that_reads_another_cells_recursive_lambda_shares_the_stack() {
    let run = |stack: usize| {
        std::thread::Builder::new()
            .stack_size(stack)
            .spawn(|| {
                // Row-major order calculates A1 first, so each cell below is calculated on demand
                // at the deepest call of the one above it.
                let countdown = |then: &str| format!("=LET(F,LAMBDA(me,n,IF(n<=0,{then},1+me(me,n-1))),F(F,131))");
                let mut wb = Workbook::new();
                let sheet = wb.sheet_mut(0).expect("sheet");
                for (at, f) in [
                    ("A1", countdown("A2")),
                    ("A2", countdown("A3")),
                    ("A3", countdown("0")),
                    ("B1", countdown("B2")),
                    ("B2", "=LET(f,LAMBDA(g,x,g(g,x)),f(f,1))".to_string()),
                ] {
                    sheet.cells.set(c(at), Cell::formula(Formula::new(&f)));
                }
                let mut calc = Calc::new();
                calc.recalc_all(&mut wb);
                ["A1", "A2", "A3", "B1", "B2"].map(|at| wb.sheet(0).map(|s| s.value(c(at))).unwrap_or_default())
            })
            .expect("spawn")
            .join()
            .expect("recursion must not overflow the stack")
    };
    let small = run(1 << 20);
    let num = Value::Error(CellError::Num);
    assert_eq!(small, [Value::Number(393.0), Value::Number(262.0), Value::Number(131.0), num.clone(), num]);
    assert_eq!(run(64 << 20), small);
}

#[test]
fn names_and_sheets() {
    let mut t = T::new();
    t.wb.names.push(gridcraft_model::DefinedName { name: "Rate".into(), scope: None, formula: "0.5".into(), comment: String::new(), hidden: false });
    t.wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Data")));
    t.wb.sheet_mut(1).unwrap().set_value(c("A1"), Value::Number(8.0));
    t.calc.rebuild(&t.wb);
    t.set("A1", "=Rate*Data!A1");
    assert_eq!(t.num("A1"), 4.0);
    t.wb.sheet_mut(1).unwrap().set_value(c("A1"), Value::Number(10.0));
    t.calc.cells_changed(&mut t.wb, &[(1, c("A1"))]);
    assert_eq!(t.num("A1"), 5.0);
}

#[test]
fn name_referring_to_several_ranges() {
    let mut t = T::new();
    // As Excel writes it in `<definedName>`: a union with no parentheses around it.
    let union = "Sheet1!$A$1:$A$2,Sheet1!$C$1:$C$2";
    t.wb.names.push(gridcraft_model::DefinedName { name: "u".into(), scope: None, formula: union.into(), comment: String::new(), hidden: false });
    for (at, v) in [("A1", "1"), ("A2", "2"), ("C1", "5"), ("C2", "7")] {
        t.set(at, v);
    }
    t.set("E1", "=SUM(u)");
    t.set("E2", "=AREAS(u)");
    t.set("E3", "=MIN(u)");
    t.set("E4", "=INDEX(u,2,1,2)");
    assert_eq!((t.num("E1"), t.num("E2"), t.num("E3"), t.num("E4")), (15.0, 2.0, 1.0, 7.0));
    t.set("C2", "70");
    assert_eq!(t.num("E1"), 78.0);
}

#[test]
fn legacy_array_formulas_fill_their_range() {
    let mut t = T::new();
    t.set("A1", "1");
    t.set("A2", "2");
    let cse = |t: &mut T, at: &str, formula: &str, range: &str| {
        let mut f = Formula::new(formula);
        f.array = gridcraft_core::RangeRef::parse(range);
        t.wb.sheet_mut(0).unwrap().cells.set(c(at), Cell::formula(f));
        t.calc.cells_changed(&mut t.wb, &[(0, c(at))]);
    };
    // Past the result: #N/A. A single column repeats across the range.
    cse(&mut t, "C1", "=A1:A2*2", "C1:D3");
    cse(&mut t, "F1", "=A1:A2*2", "F1");
    cse(&mut t, "G1", "=5", "G1:G2");
    assert_eq!(t.num("C2"), 4.0);
    assert_eq!(t.num("D2"), 4.0);
    assert_eq!(t.get("C3"), Value::Error(CellError::NA));
    // A smaller range shows only part of the result and never spills.
    assert_eq!(t.num("F1"), 2.0);
    assert_eq!(t.get("F2"), Value::Empty);
    assert_eq!(t.num("G2"), 5.0);
    // A range too large to fill is an error, not an allocation of every cell.
    cse(&mut t, "H1", "=1", "H1:XFD1048576");
    assert_eq!(t.get("H1"), Value::Error(CellError::Num));
}

#[test]
fn array_broadcast_result_is_capped() {
    // A full column against a full row broadcasts to 1048576 x 16384 (17 billion cells, about
    // 400 GB): each operand is within its own cap, so only the product check stops the abort.
    let mut t = T::new();
    let col = "SEQUENCE(1048576)";
    let row = "SEQUENCE(1,16384)";
    for (at, formula) in [
        ("A1", format!("=ROWS({col}*{row})")),
        ("A2", format!("=ROWS({col}&{row})")),
        ("A3", format!("=ROWS({col}>{row})")),
        ("A4", format!("=ROWS(IF({col},{row}))")),
        ("A5", format!("=ROWS(IF({col}>0,1,{row}))")),
        ("A6", format!("=ROWS(TEXT({col},{row}))")),
        ("A7", format!("=ROWS(MAP({col},{row},LAMBDA(a,b,a)))")),
    ] {
        t.set(at, &formula);
        assert_eq!(t.get(at), Value::Error(CellError::Num), "{formula}");
    }
    // Broadcasts within the cap are unchanged.
    t.set("C1", "=SUM(SEQUENCE(3)*SEQUENCE(1,4))");
    assert_eq!(t.num("C1"), 60.0);
    t.set("C2", "=SUM(IF(SEQUENCE(2)>1,SEQUENCE(1,3),0))");
    assert_eq!(t.num("C2"), 6.0);
    t.set("C3", "=COLUMNS(TEXT(SEQUENCE(2),SEQUENCE(1,5)))");
    assert_eq!(t.num("C3"), 5.0);
}

/// MAP refuses a result past `MAX_CELLS` by its checked product, before allocating it (the
/// 2^32-cell wrap that a 32-bit `usize` product would turn into 0 is pinned in
/// `broadcast_len_rejects_a_product_that_wraps_to_zero_on_32_bits`; `SEQUENCE` tops out at
/// 16384 columns, so the widest product it can build is 16384 squared).
#[test]
fn map_caps_its_result_size() {
    let mut t = T::new();
    for (at, formula) in
        [("A1", "=MAP(SEQUENCE(4001),SEQUENCE(1,4000),LAMBDA(a,b,a))"), ("A2", "=MAP(SEQUENCE(16384),SEQUENCE(1,16384),LAMBDA(a,b,a))")]
    {
        t.set(at, formula);
        assert_eq!(t.get(at), Value::Error(CellError::Num), "{formula}");
    }
    // Within the cap the broadcast shape is unchanged.
    t.set("A3", "=ROWS(MAP(SEQUENCE(3),SEQUENCE(1,2),LAMBDA(a,b,a+b)))");
    t.set("A4", "=COLUMNS(MAP(SEQUENCE(3),SEQUENCE(1,2),LAMBDA(a,b,a+b)))");
    assert_eq!((t.num("A3"), t.num("A4")), (3.0, 2.0));
}

#[test]
fn text_function() {
    let mut t = T::new();
    t.set("A1", "=TEXT(1234.5,\"#,##0.00\")");
    assert_eq!(t.get("A1"), Value::from("1,234.50"));
}

#[test]
fn long_operator_chains() {
    let mut t = T::new();
    t.set("A1", "1");
    // 1,000 additions: a formula of 3,000 characters, well within Excel's 8,192.
    t.set("B1", &format!("={}", vec!["A1"; 1000].join("+")));
    assert_eq!(t.num("B1"), 1000.0);
    t.set("B2", &format!("=5{}", "+1-1".repeat(600)));
    assert_eq!(t.num("B2"), 5.0);
}

#[test]
fn flat_chains_as_long_as_excel_allows() {
    // Values observed in Excel (#916): a flat chain has no operand limit below the 8,192-character
    // formula limit, which `=1+1+…` with 4,096 operands meets exactly. On a 1 MiB stack, as on
    // the Windows main thread.
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let chain = |op: &str, item: &str, n: usize| format!("={}", vec![item; n].join(op));
            let mut t = T::new();
            for r in 1..=1500 {
                t.set(&format!("A{r}"), "1");
            }
            t.set("B1", &chain("+", "1", 401));
            t.set("B2", &chain("+", "1", 4096));
            t.set("B3", &chain("-", "1", 2000));
            t.set("B4", &chain("*", "1", 4096));
            t.set("B5", &chain("/", "2", 4096));
            t.set("B6", &chain("^", "1", 4096));
            t.set("B7", &chain("+", "1%", 2000));
            t.set("B8", "=1=1=1");
            t.set("B9", &format!("={}", (1..=1500).map(|r| format!("A{r}")).collect::<Vec<_>>().join("+")));
            t.set("B10", &chain("&", "1", 2000));
            for (at, want) in [("B1", 401.0), ("B2", 4096.0), ("B3", -1998.0), ("B4", 1.0), ("B5", 0.0), ("B6", 1.0), ("B9", 1500.0)] {
                assert_eq!(t.num(at), want, "{at}");
            }
            assert!((t.num("B7") - 20.0).abs() < 1e-9);
            assert_eq!(t.get("B8"), Value::Bool(false));
            assert_eq!(t.get("B10"), Value::text("1".repeat(2000)));
        })
        .expect("spawn")
        .join()
        .expect("long chains must not overflow the stack");
}

#[test]
fn subtotal_skips_hidden() {
    let mut t = T::new();
    t.set("A1", "1");
    t.set("A2", "2");
    t.set("A3", "3");
    t.wb.sheet_mut(0).unwrap().rows.insert(1, gridcraft_model::LineInfo { hidden: true, ..Default::default() });
    t.set("B1", "=SUBTOTAL(109,A1:A3)");
    t.set("B2", "=SUBTOTAL(9,A1:A3)");
    assert_eq!(t.num("B1"), 4.0);
    assert_eq!(t.num("B2"), 6.0);
}

#[test]
fn subtotal_109_skips_nested_subtotal() {
    let mut t = T::new();
    t.set("A1", "2");
    t.set("A2", "=SUBTOTAL(9,A1)");
    t.set("C1", "=SUBTOTAL(109,A1:A2)");
    t.set("C2", "=SUBTOTAL(9,A1:A2)");
    t.set("C3", "=SUBTOTAL(109,A1)");
    assert_eq!(t.num("C1"), 2.0);
    assert_eq!(t.num("C2"), 2.0);
    assert_eq!(t.num("C3"), 2.0);
    // Manually hidden rows are still left out by 109 only.
    let mut t = T::new();
    t.set("A1", "1");
    t.set("A2", "2");
    t.set("A3", "=SUBTOTAL(9,A1:A2)");
    t.wb.sheet_mut(0).unwrap().rows.insert(1, gridcraft_model::LineInfo { hidden: true, ..Default::default() });
    t.set("B1", "=SUBTOTAL(109,A1:A3)");
    t.set("B2", "=SUBTOTAL(9,A1:A3)");
    assert_eq!(t.num("B1"), 1.0);
    assert_eq!(t.num("B2"), 3.0);
}

/// One value stored in the sheet's last cell makes `A:XFD` trim to ~17 billion cells. SUBTOTAL
/// and AGGREGATE used to read every one of them into a vector (an abort on open, #2175); now
/// every argument is charged against `MAX_CELLS` before any cell is read, and `#NUM!` comes back.
#[test]
fn subtotal_and_aggregate_cap_cells_visited() {
    let mut t = T::new();
    t.set("XFD1048576", "1");
    let formulas = [
        "=SUBTOTAL(9,A:XFD)",
        "=SUBTOTAL(109,1:1048576)",
        "=AGGREGATE(9,6,A:XFD)",
        "=AGGREGATE(14,7,A:XFD,1)",
        // A:J and K:T are about 10.5 million cells each: under the limit alone, over it together,
        // so the second area is refused before the first is read.
        "=SUBTOTAL(9,A:J,K:T)",
        "=AGGREGATE(9,4,(A:J,K:T))",
    ];
    for (i, f) in formulas.iter().enumerate() {
        let at = format!("B{}", i + 1);
        t.set(&at, f);
        assert_eq!(t.get(&at), Value::Error(CellError::Num), "{f}");
    }

    // Whole columns and rows still clip to the used range when it is small.
    let mut t = T::new();
    t.set("A1", "1");
    t.set("B2", "2");
    t.set("C2", "#N/A");
    t.set("E5", "=SUBTOTAL(9,A:B)");
    t.set("E6", "=AGGREGATE(9,6,1:2)");
    t.set("E7", "=AGGREGATE(14,6,A:C,1)");
    assert_eq!((t.num("E5"), t.num("E6"), t.num("E7")), (3.0, 3.0, 2.0));
}

/// SUBTOTAL leaves out rows a filter hides for every function number, while 1-11 keep rows
/// hidden by hand (as in ConditionalFormattingSamples.xlsx, where `SUBTOTAL(9,Table69[Sales])`
/// under a filtered table adds only the visible rows).
#[test]
fn subtotal_skips_filtered_rows() {
    use gridcraft_model::{AutoFilter, FilterCriterion, LineInfo};
    let mut t = T::new();
    for (i, v) in [1, 2, 4, 8, 16, 32].iter().enumerate() {
        t.set(&format!("A{}", i + 2), &v.to_string());
        t.set(&format!("C{}", i + 2), &v.to_string());
    }
    let crit = || vec![(0, FilterCriterion::Custom { a: (">".into(), "1".into()), b: None, and: false })];
    let sh = t.wb.sheet_mut(0).unwrap();
    sh.tables.push(gridcraft_model::Table {
        id: 1,
        name: "Sales".into(),
        range: gridcraft_core::RangeRef::parse("A1:A7").unwrap(),
        header_row: true,
        totals_row: false,
        columns: vec![gridcraft_model::TableColumn {
            name: "Amount".into(),
            totals: Default::default(),
            totals_label: None,
            formula: None,
            unique_name: None,
            xml: None,
        }],
        style: String::new(),
        banded_rows: false,
        banded_cols: false,
        first_col: false,
        last_col: false,
        filter_button: true,
        filter: crit(),
        table_type: None,
        connection_id: None,
    });
    sh.autofilter = Some(AutoFilter { range: gridcraft_core::RangeRef::parse("C1:C7").unwrap(), criteria: crit() });
    // Rows 3 and 5 are filtered out; row 9 (outside both filters) is hidden by hand.
    for row in [2, 4, 8] {
        sh.rows.insert(row, LineInfo { hidden: true, ..Default::default() });
    }
    t.set("A9", "64");
    t.set("E1", "=SUBTOTAL(9,Sales[Amount])");
    t.set("E2", "=SUBTOTAL(109,Sales[Amount])");
    t.set("E3", "=SUBTOTAL(9,C2:C7)");
    t.set("E4", "=SUBTOTAL(9,A2:A9)");
    t.set("E5", "=SUBTOTAL(109,A2:A9)");
    t.set("E6", "=SUBTOTAL(2,A2:A7)");
    t.set("E7", "=AGGREGATE(9,4,A2:A7)");
    for (at, want) in [("E1", 53.0), ("E2", 53.0), ("E3", 53.0), ("E4", 117.0), ("E5", 53.0), ("E6", 4.0), ("E7", 63.0)] {
        assert_eq!(t.num(at), want, "{at}");
    }
}

#[test]
fn aggregate_without_data_argument_is_an_error() {
    let mut t = T::new();
    t.set("A1", "1");
    t.set("A2", "2");
    t.set("D1", "=AGGREGATE(9,0)");
    t.set("D2", "=AGGREGATE(4,0)");
    t.set("D3", "=AGGREGATE(9,0,A1:A2)");
    t.set("D4", "=AGGREGATE(9,0,A3:A4)");
    t.set("D5", "=AGGREGATE(9,0,A1:A2,A1:A2)");
    assert_eq!(t.get("D1"), Value::Error(CellError::Value));
    assert_eq!(t.get("D2"), Value::Error(CellError::Value));
    assert_eq!(t.num("D3"), 3.0);
    assert_eq!(t.num("D4"), 0.0);
    assert_eq!(t.num("D5"), 6.0);
}

#[test]
fn recalc_all_after_load() {
    let mut t = T::new();
    let s = t.wb.sheet_mut(0).unwrap();
    s.cells.set(c("A1"), Cell::value(Value::Number(4.0)));
    s.cells.set(c("A2"), Cell::formula(Formula::new("=A1*2")));
    s.cells.set(c("A3"), Cell::formula(Formula::new("=A2+A1")));
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A3"), 12.0);
}

#[test]
fn sumif_sum_range_takes_the_criteria_size() {
    // Excel adds up the range that starts at sum_range's top-left cell with the criteria range's
    // height and width, whatever size sum_range has.
    let mut t = T::new();
    for (r, (k, v)) in [("a", 1), ("b", 2), ("a", 3)].iter().enumerate() {
        t.set(&format!("A{}", r + 1), k);
        t.set(&format!("B{}", r + 1), &v.to_string());
    }
    t.set("D1", "=SUMIF(A1:A3,\"a\",B1)");
    assert_eq!(t.num("D1"), 4.0);
    t.set("D2", "=AVERAGEIF(A1:A3,\"a\",B1)");
    assert_eq!(t.num("D2"), 2.0);
    t.set("D3", "=SUMIF(A1:A3,\"a\",B1:B2)");
    assert_eq!(t.num("D3"), 4.0);
    t.set("D4", "=SUMIF(A1:A3,\"a\",B1:F1)");
    assert_eq!(t.num("D4"), 4.0);
    t.set("D5", "=SUMIF(A1:A3,\"a\",B1:B99)");
    assert_eq!(t.num("D5"), 4.0);
    t.set("D6", "=SUMIF(A:A,\"a\",B1)");
    assert_eq!(t.num("D6"), 4.0);
    t.set("D7", "=SUMIF(A1:A3,\"a\",$B$2)");
    assert_eq!(t.num("D7"), 2.0); // B2:B4: rows 1 and 3 match → B2 + B4 (empty)
    t.set("D8", "=SUMIF(A1:A3,\"a\")");
    assert_eq!(t.num("D8"), 0.0);
    // A horizontal criteria range resizes across.
    t.set("H1", "1");
    t.set("I1", "0");
    t.set("J1", "1");
    t.set("H2", "10");
    t.set("I2", "20");
    t.set("J2", "30");
    t.set("D9", "=SUMIF(H1:J1,1,H2)");
    assert_eq!(t.num("D9"), 40.0);
    // The resized cells are precedents: editing one recalculates.
    t.set("B3", "5");
    assert_eq!(t.num("D1"), 6.0);
    assert_eq!(t.num("D2"), 3.0);
    assert_eq!(t.num("D3"), 6.0);
    assert_eq!(t.num("D4"), 6.0);
    assert_eq!(t.num("D6"), 6.0);
    // SUMIFS and AVERAGEIFS don't resize: different sizes are #VALUE!.
    t.set("D11", "=SUMIFS(B1,A1:A3,\"a\")");
    assert_eq!(t.get("D11"), Value::Error(CellError::Value));
    t.set("D12", "=AVERAGEIFS(B1:B2,A1:A3,\"a\")");
    assert_eq!(t.get("D12"), Value::Error(CellError::Value));
    t.set("D13", "=SUMIFS(B1:B3,A1:A3,\"a\")");
    assert_eq!(t.num("D13"), 6.0);
}

#[test]
fn sumif_resized_sum_range_stops_at_the_sheet_edge() {
    let mut t = T::new();
    t.set("A1", "a");
    t.set("A2", "a");
    t.set("B1048575", "5");
    t.set("B1048576", "7");
    t.set("C1", "=SUMIF(A1:A2,\"a\",B1048576)");
    assert_eq!(t.num("C1"), 7.0);
    t.set("C2", "=SUMIF(A1:A2,\"a\",B1048575)");
    assert_eq!(t.num("C2"), 12.0);
    t.set("C3", "=SUMIF(A1:B1,\"a\",XFD1)");
    assert_eq!(t.num("C3"), 0.0);
}

#[test]
fn sumif_resized_sum_range_on_another_sheet_and_through_a_name() {
    let mut t = T::new();
    t.wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Data")));
    t.wb.names.push(gridcraft_model::DefinedName {
        name: "Amounts".into(),
        scope: None,
        formula: "Data!$B$1".into(),
        comment: String::new(),
        hidden: false,
    });
    for (r, k) in ["a", "b", "a"].iter().enumerate() {
        t.set(&format!("A{}", r + 1), k);
        t.wb.sheet_mut(1).unwrap().set_value(CellRef::new(r as u32, 1), Value::Number((r + 1) as f64));
    }
    t.calc.rebuild(&t.wb);
    t.set("C1", "=SUMIF(A1:A3,\"a\",Data!B1)");
    assert_eq!(t.num("C1"), 4.0);
    t.set("C2", "=AVERAGEIF(A1:A3,\"a\",Amounts)");
    assert_eq!(t.num("C2"), 2.0);
    t.wb.sheet_mut(1).unwrap().set_value(c("B3"), Value::Number(9.0));
    t.calc.cells_changed(&mut t.wb, &[(1, c("B3"))]);
    assert_eq!(t.num("C1"), 10.0);
    assert_eq!(t.num("C2"), 5.0);
}

#[test]
fn trimrange() {
    let mut t = T::new();
    t.set("C3", "1");
    t.set("D5", "2");
    let cases = [
        ("J1", "=ROWS(TRIMRANGE(A1:F10))", 3.0),
        ("J2", "=COLUMNS(TRIMRANGE(A1:F10))", 2.0),
        ("J3", "=ROWS(TRIMRANGE(A1:F10,2))", 5.0),
        ("J4", "=ROWS(TRIMRANGE(A1:F10,1,0))", 8.0),
        ("J5", "=COLUMNS(TRIMRANGE(A1:F10,1,0))", 6.0),
        ("J6", "=ROWS(TRIMRANGE(A1:F10,0,0))", 10.0),
        ("J7", "=SUM(TRIMRANGE(C:C))", 1.0),
        ("J8", "=ROWS(TRIMRANGE(C:C))", 1.0),
        ("J9", "=ROWS(TRIMRANGE(C:C,2))", 3.0),
        ("J10", "=MIN(ROW(TRIMRANGE(A1:F10)))", 3.0),
        ("J11", "=COLUMNS(TRIMRANGE({1,2;3,4}))", 2.0),
    ];
    for (at, f, _) in cases {
        t.set(at, f);
    }
    for (at, f, want) in cases {
        assert_eq!(t.num(at), want, "{f}");
    }
    // The result spills as the trimmed block and follows edits.
    t.set("L1", "=TRIMRANGE(A1:F10)");
    assert_eq!((t.num("L1"), t.num("M3")), (1.0, 2.0));
    t.set("E6", "3");
    assert_eq!((t.num("J1"), t.num("J2")), (4.0, 3.0));
    // Errors: a bad trim code, nothing but blanks, several areas.
    t.set("K1", "=TRIMRANGE(A1:F10,4)");
    assert_eq!(t.get("K1"), Value::Error(CellError::Value));
    t.set("K2", "=TRIMRANGE(Q1:Q9)");
    assert_eq!(t.get("K2"), Value::Error(CellError::Calc));
    t.set("K3", "=TRIMRANGE((A1:B2,C3:D4))");
    assert_eq!(t.get("K3"), Value::Error(CellError::Value));
}

/// `formula` in AB1 over a small mixed sheet. The ranges in the cases below are cut from it:
///
/// ```text
///      A      B      C       E
///  1   1.5    10     =A1*4        (C1 is a formula cell inside the ranges)
///  2   2      TRUE           #DIV/0!   (E2 only when `error` is set)
///  3   text   3
///  4   -0.25
/// ```
fn aggregate_of(error: bool, formula: &str) -> Value {
    let mut t = T::new();
    for (at, v) in [("A1", "1.5"), ("A2", "2"), ("A3", "skipped"), ("A4", "-0.25"), ("B1", "10"), ("B2", "TRUE"), ("B3", "3"), ("C1", "=A1*4")] {
        t.set(at, v);
    }
    if error {
        t.set("E2", "#DIV/0!");
    }

    // AB1 is outside every range used, so the formula can't read itself.
    t.set("AB1", formula);
    t.get("AB1")
}

#[test]
fn sum_count_average_over_ranges() {
    // Text, booleans, blanks and a formula cell inside ranges; direct arguments; unions; whole
    // columns; blocks holding an error or nothing numeric. The expected values are what the
    // array path (the ranges copied into an array) gave before the ranges were folded in place,
    // so any difference is a change of behavior.
    let n = Value::Number;
    let e = Value::Error;

    // (formula written with SUM, error in E2?, [SUM, COUNT, AVERAGE] of that formula)
    #[rustfmt::skip]
    let cases: &[(&str, bool, [Value; 3])] = &[
        ("=SUM(A1:A1,B3)", false, [n(4.5), n(2.0), n(2.25)]),
        ("=SUM(A1:A1,B3)", true, [n(4.5), n(2.0), n(2.25)]),
        ("=SUM((A1:A2,B1:B3))", false, [n(16.5), n(4.0), n(4.125)]),
        ("=SUM((A1:A2,B1:B3))", true, [n(16.5), n(4.0), n(4.125)]),
        ("=SUM(A1:A4,B1:B3)", false, [n(16.25), n(5.0), n(3.25)]),
        ("=SUM(A1:A4,B1:B3)", true, [n(16.25), n(5.0), n(3.25)]),
        ("=SUM(A1:A4,\"x\")", false, [e(CellError::Value), n(3.0), e(CellError::Value)]),
        ("=SUM(A1:A4,\"x\")", true, [e(CellError::Value), n(3.0), e(CellError::Value)]),
        ("=SUM(A1:B4,5,\"7\",TRUE)", false, [n(29.25), n(8.0), n(3.65625)]),
        ("=SUM(A1:B4,5,\"7\",TRUE)", true, [n(29.25), n(8.0), n(3.65625)]),
        ("=SUM(A1:C4)+SUM(B1:B3)", false, [n(35.25), n(8.0), n(10.208333333333334)]),
        ("=SUM(A1:C4)+SUM(B1:B3)", true, [n(35.25), n(8.0), n(10.208333333333334)]),
        ("=SUM(A1:C4)", false, [n(22.25), n(6.0), n(3.7083333333333335)]),
        ("=SUM(A1:C4)", true, [n(22.25), n(6.0), n(3.7083333333333335)]),
        ("=SUM(A1:E4)", false, [n(22.25), n(6.0), n(3.7083333333333335)]),
        ("=SUM(A1:E4)", true, [e(CellError::Div0), n(6.0), e(CellError::Div0)]),
        ("=SUM(A1)", false, [n(1.5), n(1.0), n(1.5)]),
        ("=SUM(A1)", true, [n(1.5), n(1.0), n(1.5)]),
        ("=SUM(A3,B2)", false, [n(0.0), n(0.0), e(CellError::Div0)]),
        ("=SUM(A3,B2)", true, [n(0.0), n(0.0), e(CellError::Div0)]),
        ("=SUM(A:C)", false, [n(22.25), n(6.0), n(3.7083333333333335)]),
        ("=SUM(A:C)", true, [n(22.25), n(6.0), n(3.7083333333333335)]),
        ("=SUM(E1:E3)", false, [n(0.0), n(0.0), e(CellError::Div0)]),
        ("=SUM(E1:E3)", true, [e(CellError::Div0), n(0.0), e(CellError::Div0)]),
        ("=SUM(Z1:Z9)", false, [n(0.0), n(0.0), e(CellError::Div0)]),
        ("=SUM(Z1:Z9)", true, [n(0.0), n(0.0), e(CellError::Div0)]),
    ];

    for (formula, error, want) in cases {
        for (func, want) in ["SUM", "COUNT", "AVERAGE"].into_iter().zip(want) {
            // The same formula for each function: `SUM(` becomes `COUNT(` or `AVERAGE(`.
            let f = formula.replace("SUM", func);

            assert_eq!(&aggregate_of(*error, &f), want, "{f} (error in E2: {error})");
        }
    }
}

#[test]
fn sum_count_average_have_no_size_limit() {
    // One stored cell far down stretches the used range to 27 million cells (27 columns by a
    // million rows), past the 16M-cell limit of ranges copied into an array. Only two cells are
    // stored, so the sheet itself is tiny.
    for (func, want) in [("SUM", Value::Number(2.5)), ("COUNT", Value::Number(2.0)), ("AVERAGE", Value::Number(1.25))] {
        for range in ["A1:AA1000000", "A:AA"] {
            let mut t = T::new();
            t.set("A1", "1.5");
            t.set("Z1000000", "1");

            t.set("AB1", &format!("={func}({range})"));

            assert_eq!(t.get("AB1"), want, "{func}({range})");
        }
    }
}

#[test]
fn sum_count_average_over_spilled_values() {
    // A1 spills over A1:A3, so the block holds values that exist only as a spill (they have no
    // stored cell of their own). The host must not fold such a block in place: it falls back
    // to the array path, which sees the spill. Expected: 1 + 2 + 3 + 10.
    let mut t = T::new();
    t.set("A1", "=SEQUENCE(3)");
    t.set("A4", "10");

    for (f, want) in [("SUM", 16.0), ("COUNT", 4.0), ("AVERAGE", 4.0)] {
        t.set("C1", &format!("={f}(A1:A5)"));

        assert_eq!(t.num("C1"), want, "{f}");
    }
}

#[test]
fn aggregate_arguments_are_read_in_order() {
    // Volatile cells draw a random number each time they are calculated, so the order in which
    // the arguments are read decides which draw each one gets. `B1:B2` comes first, so B2 must
    // draw before the `RAND()` argument does. (Reading the plain `RAND()` first once gave
    // 7.9043 here.) The expected values are what the array path gave; they depend on the random
    // generator and its seed, and must be updated if either changes.
    let mut t = T::new();
    t.set("B1", "1");
    t.set("B2", "=RAND()*10");

    t.set("A1", "=SUM(B1:B2,RAND())");

    assert_eq!((t.num("A1"), t.num("B2")), (4.968520165759784, 3.3112028100185356));
}

#[test]
fn sum_count_average_follow_edits_inside_the_range() {
    // A range read in place must still count as a precedent of the formula: if it didn't, an
    // edit inside it would leave the formula showing a stale value. Each step below changes one
    // cell inside the range and checks all three functions; the last one changes a cell outside.
    for range in ["A1:A5", "A:A"] {
        let mut t = T::new();
        t.set("A1", "1");
        t.set("A3", "3");
        t.set("C1", &format!("=SUM({range})"));
        t.set("C2", &format!("=COUNT({range})"));
        t.set("C3", &format!("=AVERAGE({range})"));
        let all = |t: &T| (t.num("C1"), t.num("C2"), t.num("C3"));
        assert_eq!(all(&t), (4.0, 2.0, 2.0), "{range}");

        t.set("A1", "5");
        assert_eq!(all(&t), (8.0, 2.0, 4.0), "{range}: edit");

        t.set("A2", "10");
        assert_eq!(all(&t), (18.0, 3.0, 6.0), "{range}: fill a blank");

        t.set("A3", "text");
        assert_eq!(all(&t), (15.0, 2.0, 7.5), "{range}: number to text");

        t.set("A2", "");
        assert_eq!(all(&t), (5.0, 1.0, 5.0), "{range}: clear");

        t.set("A5", "=A1*2");
        assert_eq!(all(&t), (15.0, 2.0, 7.5), "{range}: formula in the range");

        t.set("A1", "1");
        assert_eq!(all(&t), (3.0, 2.0, 1.5), "{range}: its precedent changes");

        // A7 is inside the whole column A:A but outside A1:A5.
        t.set("A7", "100");
        let outside = if range == "A:A" { (103.0, 3.0, 103.0 / 3.0) } else { (3.0, 2.0, 1.5) };
        assert_eq!(all(&t), outside, "{range}: a cell outside the range");
    }
}

#[test]
fn visit_range_declines_when_a_formula_in_the_block_spills_into_it() {
    // The sheet: D1 = SEQUENCE(3) and D4 = 10, and D1 has not been calculated yet when the
    // block D1:D5 is read.
    let mut wb = Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.cells.set(c("D1"), Cell::formula(Formula::new("=SEQUENCE(3)")));
    sh.cells.set(c("D4"), Cell::value(Value::Number(10.0)));
    let block = gridcraft_core::RangeRef::parse("D1:D5").unwrap();

    // Reading the block calculates D1, which spills over D2:D3. A walk over the stored cells
    // would never see 2 and 3, so the host must decline and leave the block to `range_values`.
    // (`Calc` recalculates a pass that produced new spills, so final results are right either
    // way; only this direct call shows the host's contract.)
    let walked = crate::recalc::tests::probe_visit_range(&wb, &[("D1", "=SEQUENCE(3)")], block);

    assert_eq!(walked, None, "walked a block holding a spill it can't see");
}

/// Trim references evaluate as `TRIMRANGE(range, code, code)`.
#[test]
fn trim_references() {
    let mut t = T::new();
    t.set("C3", "1");
    t.set("D5", "2");
    t.set("A20", "5");
    t.set("A21", "6");
    let cases = [
        ("J1", "=ROWS(A1.:.F10)", "=ROWS(TRIMRANGE(A1:F10,3,3))", 3.0),
        ("J2", "=COLUMNS(A1.:.F10)", "=COLUMNS(TRIMRANGE(A1:F10,3,3))", 2.0),
        ("J3", "=ROWS(A1:.F10)", "=ROWS(TRIMRANGE(A1:F10,2,2))", 5.0),
        ("J4", "=COLUMNS(A1:.F10)", "=COLUMNS(TRIMRANGE(A1:F10,2,2))", 4.0),
        ("J5", "=ROWS(A1.:F10)", "=ROWS(TRIMRANGE(A1:F10,1,1))", 8.0),
        ("J6", "=MIN(ROW(A1.:F10))", "=MIN(ROW(TRIMRANGE(A1:F10,1,1)))", 3.0),
        ("J7", "=SUM(A:.A)", "=SUM(TRIMRANGE(A:A,2,2))", 11.0),
        ("J8", "=ROWS(A:.A)", "=ROWS(TRIMRANGE(A:A,2,2))", 21.0),
        ("J9", "=ROWS(A.:.A)", "=ROWS(TRIMRANGE(A:A,3,3))", 2.0),
        ("J10", "=ROWS(20.:.30)", "=ROWS(TRIMRANGE(20:30,3,3))", 2.0),
    ];
    for (at, f, _, _) in cases {
        t.set(at, f);
    }
    for (i, (at, f, same, want)) in cases.iter().enumerate() {
        assert_eq!(t.num(at), *want, "{f}");
        let k = format!("K{}", i + 1);
        t.set(&k, same);
        assert_eq!(t.num(&k), *want, "{same}");
    }
    // The trimmed block spills and follows edits to the full range.
    t.set("L1", "=C1.:.F10");
    assert_eq!((t.num("L1"), t.num("M3")), (1.0, 2.0));
    t.set("A22", "4");
    assert_eq!((t.num("J7"), t.num("J8")), (15.0, 22.0));
    // Nothing but blanks.
    t.set("K20", "=SUM(Q1:.Q9)");
    assert_eq!(t.get("K20"), Value::Error(CellError::Calc));
}

#[test]
fn lookup_of_an_empty_cell_stays_blank() {
    // #374: B1 (the value for key "k") is empty.
    let mut t = T::new();
    t.set("A1", "k");
    t.set("A2", "j");
    t.set("B2", "v");
    for (at, f, want) in [
        ("D2", "=\"x\"&VLOOKUP(\"k\",A1:B2,2,0)&\"y\"", Value::from("xy")),
        ("D3", "=CONCATENATE(\"x\",VLOOKUP(\"k\",A1:B2,2,0),\"y\")", Value::from("xy")),
        ("D4", "=ISBLANK(XLOOKUP(\"k\",A1:A2,B1:B2))", Value::Bool(true)),
        ("D5", "=\"x\"&XLOOKUP(\"k\",A1:A2,B1:B2)&\"y\"", Value::from("xy")),
        // A bare lookup still shows 0, like a reference to an empty cell.
        ("D8", "=VLOOKUP(\"k\",A1:B2,2,0)", Value::Number(0.0)),
        // Arithmetic still reads the blank as 0.
        ("D9", "=VLOOKUP(\"k\",A1:B2,2,0)+1", Value::Number(1.0)),
        // A blank lookup given as an argument is still given, not omitted.
        ("D10", "=FILTER(1,FALSE,VLOOKUP(\"k\",A1:B2,2,FALSE))", Value::Number(0.0)),
        ("D11", "=XLOOKUP(\"z\",A1:A2,B1:B2,VLOOKUP(\"k\",A1:B2,2,0))", Value::Number(0.0)),
        // The same holds per element when the call is lifted over an array of blank lookups.
        ("D12", "=CEILING.MATH(6.7,VLOOKUP(\"k\",A1:B2,2,0))", Value::Number(0.0)),
        ("D13", "=SUM(CEILING.MATH(6.7,VLOOKUP({\"k\",\"k\"},A1:B2,2,0)))", Value::Number(0.0)),
        ("D14", "=FLOOR.MATH(6.7,VLOOKUP(\"k\",A1:B2,2,0))", Value::Number(0.0)),
        ("D15", "=SUM(FLOOR.MATH(6.7,VLOOKUP({\"k\",\"k\"},A1:B2,2,0)))", Value::Number(0.0)),
    ] {
        t.set(at, f);
        assert_eq!(t.get(at), want, "{f}");
    }
}

#[test]
fn blank_lookup_values_match_excel_windows_observation() {
    // Excel 365 Windows 2609, build 16.0.20430.20032, black-box check 2026-10-13:
    // A1="k", B1 and A2 genuinely empty.
    // All three value lookups preserve blankness inside ISBLANK/concatenation.
    let mut t = T::new();
    t.set("A1", "k");
    for (at, f, want) in [
        ("D1", "=ISBLANK(VLOOKUP(\"k\",A1:B1,2,FALSE))", Value::Bool(true)),
        ("D2", "=ISBLANK(HLOOKUP(\"k\",A1:A2,2,FALSE))", Value::Bool(true)),
        ("D3", "=ISBLANK(LOOKUP(\"k\",A1:A1,B1:B1))", Value::Bool(true)),
        ("D4", "=\"x\"&HLOOKUP(\"k\",A1:A2,2,FALSE)&\"y\"", Value::from("xy")),
    ] {
        t.set(at, f);
        assert_eq!(t.get(at), want, "{f}");
    }
}

#[test]
fn cell_width_is_the_column_width_dialog_value_rounded() {
    // #735: Excel 365 values. Widths in px as `home.columnWidth {"chars"}` sets them
    // (chars * 7 + 5): 10.71 -> 80, 12 -> 89, 10.57 -> 79, 2.86 -> 25; B keeps the default 8.43 (64).
    let mut t = T::new();
    let sh = t.wb.sheet_mut(0).unwrap();
    for (col, px) in [(2, 80.0), (3, 89.0), (4, 79.0), (5, 25.0)] {
        sh.cols.insert(col, gridcraft_model::LineInfo { size: Some(px), ..Default::default() });
    }
    for (at, col) in [("H1", "B"), ("H2", "C"), ("H3", "D"), ("H4", "E"), ("H5", "F")] {
        t.set(at, &format!("=CELL(\"width\",{col}1)"));
    }
    assert_eq!([t.num("H1"), t.num("H2"), t.num("H3"), t.num("H4"), t.num("H5")], [8.0, 11.0, 12.0, 11.0, 3.0]);
}

/// Expected results are Excel's, from Apache POI's `DGet.xls` (rows 46-49): a criteria cell holding
/// a formula under a label that names no field is evaluated for every record.
#[test]
fn database_functions_with_computed_criteria() {
    let mut t = T::new();
    let db = [
        ["name", "age", "size", "headgear", "glasses"],
        ["Huey", "6", "45", "cap~", "FALSE"],
        ["Dewey", "7", "fourty-six", "cap?", "FALSE"],
        ["Louie", "8", "47", "cap*", "FALSE"],
        ["Donald", "32", "70", "sailor's hat", "FALSE"],
        ["Scrooge", "62", "65", "top hat", "TRUE"],
    ];
    for (r, row) in db.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            t.set(&format!("{}{}", ["F", "G", "H", "I", "J"][c], r + 2), v);
        }
    }
    t.set("G45", "7");
    let cases = [
        // Relative references are to the first record's row and move to each record's row;
        // `$G$45` stays put.
        ("G", "=G2<$G$45", "Dewey"),
        // Column labels stand for the record's fields.
        ("H", "=glasses=TRUE", "Scrooge"),
        ("I", "=age<7", "Huey"),
        ("J", "=size/7=10", "Donald"),
        // An absolute reference to a cell that is not the first record's.
        ("K", "=H3=$L$1", "Donald"),
    ];
    t.set("L1", "70");
    for (i, (col, formula, _)) in cases.iter().enumerate() {
        t.set(&format!("{col}46"), "Formula");
        t.set(&format!("{col}47"), formula);
        t.set(&format!("B{}", 46 + i), &format!("=DGET($F$2:$J$7,\"name\",{col}46:{col}47)"));
    }
    for (i, (_, formula, want)) in cases.iter().enumerate() {
        assert_eq!(t.get(&format!("B{}", 46 + i)), Value::text(*want), "{formula}");
    }
    // Every database function takes them, ANDed with the other criteria of their row.
    // Text sorts after numbers, so Dewey's "fourty-six" is > 40.
    t.set("L47", "=size>40");
    t.set("M46", "age");
    t.set("M47", ">6");
    t.set("B51", "=DCOUNT($F$2:$J$7,\"age\",I46:I47)");
    t.set("B52", "=DSUM($F$2:$J$7,\"age\",G46:G47)");
    t.set("B53", "=DCOUNTA($F$2:$J$7,\"name\",L46:M47)");
    assert_eq!(t.num("B51"), 1.0);
    assert_eq!(t.num("B52"), 7.0);
    assert_eq!(t.num("B53"), 4.0);
}

#[test]
fn offset_extreme_arguments_return_ref() {
    let wb = Workbook::new();
    for formula in [
        "=OFFSET(B2,1E300,0)",
        "=OFFSET(B2,0,1E300)",
        "=OFFSET(B2,0,0,1E300,1)",
        "=OFFSET(B2,0,0,1,1E300)",
        "=OFFSET(B2,-1E300,0)",
        "=OFFSET(B2,0,-1E300)",
        "=OFFSET(B2,0,0,-1E300,1)",
        "=OFFSET(B2,0,0,1,-1E300)",
        "=OFFSET(B2,1E300,0,-1E300,1)",
        "=OFFSET(B2,0,1E300,1,-1E300)",
        "=OFFSET(B2,-1E300,0,-1E300,1)",
    ] {
        assert_eq!(crate::evaluate(&wb, 0, c("D1"), formula), Value::Error(CellError::Ref), "{formula}");
    }
    for (formula, expected) in [
        ("=ROW(OFFSET(B2,1.9,0))", 3.0),
        ("=COLUMN(OFFSET(B2,0,-1.9))", 1.0),
        ("=ROWS(OFFSET(B2,0,0,-2,1))", 2.0),
        ("=COLUMNS(OFFSET(B2,0,0,1,-2))", 2.0),
        ("=ROWS(OFFSET(A1,0,0,1048576,1))", 1048576.0),
    ] {
        assert_eq!(crate::evaluate(&wb, 0, c("D1"), formula), Value::Number(expected), "{formula}");
    }
}

/// `@` intersects a reference with the formula's row (a column range) or column (a row range);
/// a 2D range both (circular when it includes the formula's cell); a range the formula does not
/// line up with gives #VALUE!. An array gives its top-left value. Legacy formulas read from files use it where Excel intersects implicitly.
#[test]
fn implicit_intersection_operator() {
    let mut t = T::new();
    for (at, v) in [("A1", "1"), ("A2", "2"), ("A3", "3"), ("A5", "4"), ("B5", "5"), ("C5", "6")] {
        t.set(at, v);
    }
    t.set("B2", "=TANH(@A1:A3)");
    t.set("B7", "=TANH(@A5:C5)");
    t.set("B3", "=@A1:C3");
    t.set("B9", "=@A1:A3");
    t.set("E9", "=@A5:C5");
    t.set("D1", "=@{7,8;9,10}");
    t.set("D2", "=@A2");
    t.set("D3", "=SUM(@A1:A3)");
    assert_eq!(t.num("B2"), 2f64.tanh());
    assert_eq!(t.num("B7"), 5f64.tanh());
    for at in ["B9", "E9"] {
        assert_eq!(t.get(at), Value::Error(CellError::Value), "{at}");
    }
    // A 2D range that includes the formula's own cell is a circular reference (Excel shows 0).
    assert_eq!(t.get("B3"), Value::Error(CellError::Circ));
    // From another sheet a 2D range intersects on both row and column; outside it is #VALUE!.
    t.wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Other")));
    for (at, input) in [("B2", "=@Sheet1!A1:C3"), ("C1", "=@Sheet1!A1:C3"), ("B9", "=@Sheet1!A1:C3"), ("E2", "=@Sheet1!A1:C3")] {
        t.wb.sheet_mut(1).unwrap().cells.set(c(at), Cell::formula(Formula::new(input)));
    }
    t.calc.recalc_all(&mut t.wb);
    let other = |at: &str| t.wb.sheet(1).unwrap().value(c(at));
    assert_eq!(other("B2"), Value::Number(2f64.tanh())); // Sheet1!B2
    assert_eq!(other("C1"), Value::Number(0.0));
    assert_eq!(other("B9"), Value::Error(CellError::Value));
    assert_eq!(other("E2"), Value::Error(CellError::Value));
    assert_eq!(t.num("D1"), 7.0);
    assert_eq!(t.num("D2"), 2.0);
    assert_eq!(t.num("D3"), 3.0);
    // An array result shows its top-left value (T of a reference reads its top-left cell).
    t.set("A11", "x");
    t.set("C11", "y");
    t.set("E1", "=@T(A11:C11)");
    assert_eq!(t.get("E1"), Value::text("x"));
    // Without `@` (a dynamic-array formula) the same formula spills.
    t.set("F1", "=TANH(A1:A3)");
    assert_eq!((t.num("F1"), t.num("F2"), t.num("F3")), (1f64.tanh(), 2f64.tanh(), 3f64.tanh()));
}

/// "Set precision as displayed": results are rounded to their cell's format before they are
/// stored and before other formulas read them. Values observed in Excel (COM,
/// `Workbook.PrecisionAsDisplayed = True`) with the same synthetic formulas.
#[test]
fn precision_as_displayed_rounds_results_by_cell_format() {
    let fmt = |t: &mut T, at: &str, code: &str| {
        let id = t.wb.styles.intern(gridcraft_model::Style { num_fmt: gridcraft_model::NumFmt::new(code), ..Default::default() });
        t.wb.sheet_mut(0).unwrap().set_style(c(at), id);
    };
    let mut t = T::new();
    t.set("A1", "=-130+657.91");
    t.set("A2", "=A1*10");
    t.set("A3", "=A1/7");
    // Cross-cell: E2 rounds what it reads from E1; E1 itself (General) keeps full precision.
    t.set("E1", "=10/3");
    t.set("E2", "=E1");
    t.set("E3", "=E2*10");
    t.set("E4", "=E1*10");
    // A spill: each element takes the format of the cell it lands in.
    t.set("F1", "=SEQUENCE(3,1,0.123,0.111)");
    t.set("G4", "=SUM(F1:F3)");
    t.set("H1", "=TRUE");
    t.set("H2", "=\"1.5\"");
    // A conditional format rounds by the sign's section, not the one displayed: `[<1]0.00;0`
    // shows 2+1/3 as `2` but stores 2.33, and stores -10/3 as -3.
    t.set("J1", "=2+1/3");
    t.set("J2", "=-10/3");
    t.set("K1", "=J1*3");
    t.set("K2", "=J2*3");
    fmt(&mut t, "J1", "[<1]0.00;0");
    fmt(&mut t, "J2", "[<1]0.00;0");
    fmt(&mut t, "A1", "0");
    fmt(&mut t, "A3", "0.0000");
    fmt(&mut t, "E2", "0");
    fmt(&mut t, "F1", "0.0");
    fmt(&mut t, "F2", "0.0");
    fmt(&mut t, "H1", "0");
    fmt(&mut t, "H2", "0");
    // Off: formats don't change values.
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A1"), -130.0 + 657.91);
    assert_eq!(t.num("E2"), 10.0 / 3.0);
    t.wb.calc.precision_as_displayed = true;
    t.calc.recalc_all(&mut t.wb);
    assert_eq!((t.num("A1"), t.num("A2"), t.num("A3")), (528.0, 5280.0, 75.4286));
    assert_eq!((t.num("E1"), t.num("E2"), t.num("E3"), t.num("E4")), (10.0 / 3.0, 3.0, 30.0, 10.0 / 3.0 * 10.0));
    let sh = t.wb.sheet(0).unwrap();
    assert_eq!(
        (sh.value(c("F1")), sh.value(c("F2")), sh.value(c("F3"))),
        (Value::Number(0.1), Value::Number(0.2), Value::Number(0.123 + 0.111 * 2.0))
    );
    assert_eq!(t.num("G4"), 0.1 + 0.2 + (0.123 + 0.111 * 2.0));
    assert_eq!((t.num("J1"), t.num("J2"), t.num("K1"), t.num("K2")), (2.33, -3.0, 2.33 * 3.0, -9.0));
    // Booleans and text are left alone.
    assert_eq!((t.get("H1"), t.get("H2")), (Value::Bool(true), Value::from("1.5")));
    // An edit recalculates through the rounded values too.
    t.set("A1", "=-130+657.41");
    fmt(&mut t, "A1", "0");
    t.calc.cells_changed(&mut t.wb, &[(0, c("A1"))]);
    assert_eq!((t.num("A1"), t.num("A2")), (527.0, 5270.0));
    // Off again: a recalculation brings back full precision.
    t.wb.calc.precision_as_displayed = false;
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A2"), (-130.0 + 657.41) * 10.0);
}
#[test]
fn analysis_toolpak_value_params_reject_multi_cell_references() {
    // Excel 365 (observed): EOMONTH, WORKDAY, COUPDAYS, BIN2DEC... give #VALUE! for a reference to more
    // than one cell, however the reference is formed, and lift array constants and computed arrays.
    let mut t = T::new();
    for (a, v) in [
        ("A1", "45000"),
        ("A2", "45031"),
        ("A3", "45061"),
        ("B1", "1"),
        ("B2", "2"),
        ("B3", "3"),
        ("C2", "7"),
        ("F1", "1"),
        ("F2", "2"),
        ("F3", "11"),
    ] {
        t.set(a, v);
    }
    t.wb.names.push(gridcraft_model::DefinedName {
        name: "Dates".into(),
        scope: None,
        formula: "Sheet1!$A$1:$A$3".into(),
        comment: String::new(),
        hidden: false,
    });
    let value = Value::Error(CellError::Value);
    for f in [
        "=EOMONTH(A1:A3,1)",
        "=EOMONTH(A1,B1:B3)",
        "=EDATE(A1:A3,1)",
        "=SUMPRODUCT(EOMONTH(A1:A3,0))",
        "=EOMONTH(INDEX(A1:A3,0,1),0)",
        "=EOMONTH(OFFSET(A1,0,0,3,1),0)",
        "=EOMONTH(IF(TRUE,A1:A3),0)",
        "=EOMONTH(CHOOSE(1,A1:A3),0)",
        "=LET(x,A1:A3,EOMONTH(x,0))",
        "=EOMONTH(Dates,0)",
        "=EOMONTH((A1,A2),0)",
        "=EOMONTH(Z:Z,0)",
        "=EOMONTH(A1:INDEX(A1:A3,3),0)",
        "=WORKDAY(A1:A3,1)",
        "=NETWORKDAYS(A1,A3:A4)",
        "=YEARFRAC(A1,A3,B1:B2)",
        "=WEEKNUM(A1,B1:B3)",
        "=COUPDAYS(A1:A3,46000,2,0)",
        "=DISC(A1,46000,98,100,B1:B3)",
        "=BIN2DEC(B1:B3)",
        "=ISEVEN(B1:B3)",
    ] {
        t.set("H5", f);
        assert_eq!((t.get("H5"), t.get("H6")), (value.clone(), Value::Empty), "{f}");
        t.set("H5", "");
    }
    // One-cell references, intersections, @ and the range parameters are fine.
    for (f, want) in [
        ("=EOMONTH(A1:A1,0)", 45016.0),
        ("=EOMONTH(INDEX(A1:A3,2),0)", 45046.0),
        ("=EOMONTH(OFFSET(A1,0,0,1,1),0)", 45016.0),
        ("=EOMONTH(A1:A3 A2:C2,0)", 45046.0),
        ("=EOMONTH(@A1:A3,0)", 45046.0),
        ("=WORKDAY(A1,1,A1:A3)", 45001.0),
        ("=XNPV(0,B1:B3,A1:A3)", 6.0),
    ] {
        t.set("H2", f);
        assert_eq!(t.num("H2"), want, "{f}");
        t.set("H2", "");
    }
    // Arrays that aren't references lift; so does the WORKDAY.INTL/NETWORKDAYS.INTL weekend.
    for (f, below, right) in [
        ("=EOMONTH(A1:A3+0,0)", Value::Number(45046.0), Value::Empty),
        ("=EOMONTH(A1,{1,2})", Value::Empty, Value::Number(45077.0)),
        ("=EOMONTH(TRANSPOSE(A1:A3),0)", Value::Empty, Value::Number(45046.0)),
        ("=MAP(A1:A3,LAMBDA(x,EOMONTH(x,0)))", Value::Number(45046.0), Value::Empty),
        ("=NETWORKDAYS.INTL(A1,A3+30,F1:F3)", Value::Number(66.0), Value::Empty),
        ("=ISOWEEKNUM(A1:A3)", Value::Number(15.0), Value::Empty),
        // The rejected reference adds no shape: a 2x1 array of #VALUE!.
        ("=EOMONTH(A1:A3,{1;2})", value.clone(), Value::Empty),
    ] {
        t.set("H5", f);
        assert_eq!((t.get("H6"), t.get("I5")), (below, right), "{f}");
        t.set("H5", "");
    }
}
/// A formula reading one further down (or on a later sheet) is calculated on demand, inside the
/// formula that reads it. Chains of thousands of such formulas, common in financial models, used
/// to overflow the stack (or give `#CIRC!` past 2,000 levels); they now calculate in full.
#[test]
fn long_dependency_chains_against_row_order_calculate() {
    const N: u32 = 20_000;
    let mut t = T::new();
    {
        let sh = t.wb.sheet_mut(0).unwrap();
        for r in 1..N {
            sh.cells.set(c(&format!("A{r}")), Cell::formula(Formula::new(&format!("=A{}+1", r + 1))));
        }
        sh.cells.set(c(&format!("A{N}")), Cell::value(Value::Number(1.0)));
    }
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A1"), N as f64);
    assert_eq!(t.num("A2"), (N - 1) as f64);
    assert!(t.calc.circular.is_empty());
    // An edit at the far end recalculates the whole chain.
    t.set(&format!("A{N}"), "10");
    assert_eq!(t.num("A1"), (N + 9) as f64);
    assert!(t.calc.circular.is_empty());

    // Hopping between two sheets at every step, through a range read (SUM) as well.
    let mut t = T::new();
    t.wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Data")));
    for r in 1..N {
        t.wb.sheet_mut(0).unwrap().cells.set(c(&format!("A{r}")), Cell::formula(Formula::new(&format!("=Data!A{r}+1"))));
        let next = format!("=SUM(Sheet1!A{}:A{})", r + 1, r + 1);
        t.wb.sheet_mut(1).unwrap().cells.set(c(&format!("A{r}")), Cell::formula(Formula::new(&next)));
    }
    t.wb.sheet_mut(0).unwrap().cells.set(c(&format!("A{N}")), Cell::value(Value::Number(0.0)));
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A1"), (N - 1) as f64);
    assert_eq!(t.wb.sheet(1).unwrap().value(c("A1")), Value::Number((N - 2) as f64));
    assert!(t.calc.circular.is_empty());
}

/// A loop longer than the on-demand nesting limit is still a circular reference (and still ends).
#[test]
fn long_circular_chain_is_circular() {
    const N: u32 = 5_000;
    let mut t = T::new();
    {
        let sh = t.wb.sheet_mut(0).unwrap();
        for r in 1..N {
            sh.cells.set(c(&format!("A{r}")), Cell::formula(Formula::new(&format!("=A{}+1", r + 1))));
        }
        sh.cells.set(c(&format!("A{N}")), Cell::formula(Formula::new("=A1+1")));
        sh.cells.set(c("B1"), Cell::formula(Formula::new("=1+2")));
    }
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.get("A1"), Value::Error(CellError::Circ));
    assert_eq!(t.get(&format!("A{N}")), Value::Error(CellError::Circ));
    assert_eq!(t.num("B1"), 3.0);
    assert!(!t.calc.circular.is_empty());
    // Each formula is listed once at most.
    let mut seen = t.calc.circular.clone();
    seen.sort_by_key(|(s, c)| (*s, c.row, c.col));
    seen.dedup();
    assert_eq!(seen.len(), t.calc.circular.len());
    // With iterative calculation the loop settles to numbers instead (no hang, no crash).
    t.wb.calc.iterative = true;
    t.wb.calc.max_iterations = 3;
    t.calc.recalc_all(&mut t.wb);
    assert!(t.get("A1").as_f64().is_some());
}

/// A loop closing back into the middle of a chain deeper than the on-demand nesting limit is found
/// where calculating it in one go finds it: at the formula read while it was being calculated.
#[test]
fn deep_loop_is_found_where_it_closes() {
    let mut t = T::new();
    {
        let sh = t.wb.sheet_mut(0).unwrap();
        for r in 1..300 {
            sh.cells.set(c(&format!("A{r}")), Cell::formula(Formula::new(&format!("=A{}+1", r + 1))));
        }
        sh.cells.set(c("A300"), Cell::formula(Formula::new("=A30+1")));
        // Reads the loop through IFERROR, so its value shows where the loop was cut.
        sh.cells.set(c("B1"), Cell::formula(Formula::new("=IFERROR(A1,-1)")));
    }
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.calc.circular, vec![(0, c("A30"))]);
    assert_eq!(t.get("A1"), Value::Error(CellError::Circ));
    assert_eq!(t.num("B1"), -1.0);
}

#[test]
fn large_bounded_references_keep_their_shape() {
    // D3:AZ400 is 19,502 cells, past the size where a block used to be cut to the used range
    // (which ends at column F here). Sliced that way it no longer lined up with the full-size
    // operands next to it. Expected values observed in Excel 365 on the same layout.
    let mut t = T::new();
    for (at, v) in [
        ("D1", "1"),
        ("E1", "2"),
        ("F1", "3"),
        ("A3", "1"),
        ("A4", "2"),
        ("A5", "1"),
        ("B3", "2"),
        ("B4", "3"),
        ("B5", "4"),
        ("D3", "10"),
        ("E3", "20"),
        ("F3", "30"),
        ("D4", "40"),
        ("E4", "50"),
        ("F4", "60"),
        ("D5", "70"),
        ("E5", "80"),
        ("F5", "90"),
    ] {
        t.set(at, v);
    }
    let cases = [
        ("=SUMPRODUCT(D3:AZ400*B3:B400,(A3:A400=1)*(D1:AZ1=2))", Value::Number(360.0)),
        ("=ROWS(D3:AZ400*1)", Value::Number(398.0)),
        ("=COLUMNS(D3:AZ400*1)", Value::Number(49.0)),
        ("=SUMPRODUCT(D3:AZ400,(A3:A400=1)*(D1:AZ1=2))", Value::Number(100.0)),
        ("=SUMPRODUCT(D3:AZ400+0,D3:AZ401)", Value::Error(CellError::Value)),
        ("=COUNTBLANK(D3:AZ400)", Value::Number(19493.0)),
        ("=SUMPRODUCT(--(D3:AZ400=\"\"))", Value::Number(19493.0)),
        ("=SUM(D3:AZ400*B3:B400)", Value::Number(1530.0)),
        ("=COLUMNS(TRANSPOSE(D3:AZ400))", Value::Number(398.0)),
        // Whole columns are still read up to the used range.
        ("=SUMPRODUCT((A:A=1)*B:B)", Value::Number(6.0)),
    ];
    for (i, (formula, want)) in cases.iter().enumerate() {
        let at = format!("C{}", 1001 + i);
        t.set(&at, formula);
        assert_eq!(&t.get(&at), want, "{formula}");
    }
}

#[test]
fn median_large_values_recalculate_dependents() {
    let mut t = T::new();
    t.set("A1", "1e308");
    t.set("A2", "1.6e308");
    t.set("B1", "=MEDIAN(A1:A2)");
    t.set("B2", "=AGGREGATE(12,0,A1:A2)");
    t.set("C1", "=B1/1e308");
    t.set("C2", "=B2/1e308");
    for cell in ["B1", "B2"] {
        assert!((t.num(cell) / 1e308 - 1.3).abs() < 1e-14);
    }
    for cell in ["C1", "C2"] {
        assert!((t.num(cell) - 1.3).abs() < 1e-14);
    }
    t.set("A2", "-1e308");
    for cell in ["B1", "B2", "C1", "C2"] {
        assert_eq!(t.num(cell), 0.0);
    }
    t.set("A1", "-1.6e308");
    for cell in ["B1", "B2"] {
        assert!((t.num(cell) / 1e308 + 1.3).abs() < 1e-14);
    }
    for cell in ["C1", "C2"] {
        assert!((t.num(cell) + 1.3).abs() < 1e-14);
    }
}

/// #2183: `INDEX(…):INDEX(…):…` used to re-walk each operand of the left-associative range chain,
/// costing 2^k for k operands (28 took 40 s; 60 would take millennia), on open and on every edit.
#[test]
fn range_operator_chain_precedents_are_linear() {
    let operands: Vec<String> = (1..=60).map(|i| format!("INDEX(A{i},1)")).collect();
    let src = format!("=SUM({})", operands.join(":"));
    let e = gridcraft_formula::parse(&src).unwrap();
    let wb = Workbook::new();
    // With the old 2^k walk this call would never return; #2183 asks for under 100 ms.
    #[allow(clippy::disallowed_methods)] // a native test: the clock is there
    let t0 = std::time::Instant::now();
    let (areas, dynamic) = crate::eval::precedents(&wb, 0, &e);
    let took = t0.elapsed();
    assert!(took < std::time::Duration::from_millis(100), "precedents took {took:?}");
    assert!(!dynamic);
    // The chain covers A1:A60, not only the 60 cells named in it.
    let whole = gridcraft_core::RangeRef::new(c("A1"), c("A60"));
    assert!(areas.iter().any(|a| a.sheet == 0 && a.range == whole), "{areas:?}");
    // Each node is visited once: 60 references plus one box per range operator.
    assert!(areas.len() < 200, "{} areas", areas.len());

    // Through the recalculation engine: setting it finishes and editing a cell in the box recalculates.
    let mut t = T::new();
    for i in 1..=60 {
        t.set(&format!("A{i}"), "1");
    }
    t.set("B1", &src);
    assert_eq!(t.num("B1"), 60.0);
    t.set("A30", "5");
    assert_eq!(t.num("B1"), 64.0);
}

/// A chained range of plain references (`A1:B2:C3`, which the parser can't fold into one
/// reference) covers their bounding box, as the evaluator expands it: an edit inside the box but
/// outside each operand still recalculates.
#[test]
fn chained_plain_range_precedents_cover_the_bounding_box() {
    let e = gridcraft_formula::parse("=SUM(A1:B2:C3)").unwrap();
    let (areas, dynamic) = crate::eval::precedents(&Workbook::new(), 0, &e);
    assert!(!dynamic);
    let whole = gridcraft_core::RangeRef::new(c("A1"), c("C3"));
    assert!(areas.iter().any(|a| a.sheet == 0 && a.range == whole), "{areas:?}");

    let mut t = T::new();
    t.set("A1", "1");
    t.set("E1", "=SUM(A1:B2:C3)");
    assert_eq!(t.num("E1"), 1.0);
    t.set("C1", "4");
    assert_eq!(t.num("E1"), 5.0);
}

/// `precedents` walks with an explicit stack: an expression nested deeper than any formula
/// within the length limit can build still finds its references, on a small stack.
#[test]
fn precedents_of_deep_nesting_do_not_recurse() {
    let (areas, dynamic) = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let mut e = gridcraft_formula::parse("=A1").unwrap();
            for _ in 0..10_000 {
                e = gridcraft_formula::Expr::Paren(Box::new(e));
            }
            let found = crate::eval::precedents(&Workbook::new(), 0, &e);
            // Dropping the deep tree recurses; leak it so only `precedents` is under test.
            std::mem::forget(e);
            found
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(!dynamic);
    assert_eq!(areas.len(), 1);
    // Shallow nesting stays static.
    let e = gridcraft_formula::parse("=((((A1))))").unwrap();
    let (areas, dynamic) = crate::eval::precedents(&Workbook::new(), 0, &e);
    assert!(!dynamic);
    assert_eq!(areas.len(), 1);
}

#[test]
fn cell_filename_names_the_workbook_file() {
    // Excel returns `<folder>/[<file>]<sheet>`, and the classic sheet-name formula splits it on
    // the `]`; an empty string broke that formula in every German workbook that used it (#1489).
    let dir = std::path::Path::new("books");
    let mut t = T::new();
    t.wb.sheet_mut(0).unwrap().name = "Info".into();

    // No file yet: empty, as in a workbook Excel has never saved.
    t.set("A1", "1");
    t.set("B1", "=CELL(\"filename\",A1)");
    assert_eq!(t.get("B1"), Value::from(""));

    t.wb.source_path = Some(dir.join("gc_cell_info_de.xlsx"));
    t.calc = Calc::new();
    let expect = dir.join("[gc_cell_info_de.xlsx]Info").to_string_lossy().into_owned();

    t.set("B2", "=CELL(\"filename\",A1)");
    assert_eq!(t.get("B2"), Value::from(expect.as_str()));

    // The two formulas from the report: the sheet name, and the test that the `]` is there.
    t.set("B3", "=MID(CELL(\"filename\",A1),FIND(\"]\",CELL(\"filename\",A1))+1,31)");
    assert_eq!(t.get("B3"), Value::from("Info"), "the sheet-name formula");
    t.set("B4", "=ISNUMBER(FIND(\"]\",CELL(\"filename\",A1)))");
    assert_eq!(t.get("B4"), Value::Bool(true));
    // The sheet is the one the reference is on, not the active one.
    t.wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Second")));
    t.set("B6", "=CELL(\"filename\",Second!A1)");
    assert_eq!(t.get("B6"), Value::from(dir.join("[gc_cell_info_de.xlsx]Second").to_string_lossy().into_owned().as_str()));

    // A workbook that arrived as bytes knows its name but no folder: still splittable.
    t.wb.source_path = Some(std::path::PathBuf::from("Book.xlsx"));
    t.calc = Calc::new();
    t.set("B7", "=CELL(\"filename\",A1)");
    assert_eq!(t.get("B7"), Value::from("[Book.xlsx]Info"));
}

#[test]
fn n_lifts_over_a_computed_array_but_reads_the_first_cell_of_a_range() {
    let mut t = T::new();
    t.set("B1", "0");
    t.set("B2", "23850");
    t.set("D2", "10%");
    t.set("D3", "12%");
    t.set("F1", "50000");
    t.set("H1", "=SUMPRODUCT(D2:D3-D1:D2,F1-B1:B2,N(F1>B1:B2))");
    assert!((t.num("H1") - 5523.0).abs() < 1e-9, "{:?}", t.get("H1"));
    t.set("H2", "=SUMPRODUCT(N(B1:B2>0))");
    assert_eq!(t.num("H2"), 1.0);
    // A spill of N(D2:D3) would also show 10% in H3; summing it tells the two apart.
    t.set("H3", "=SUM(N(D2:D3))");
    assert!((t.num("H3") - 0.1).abs() < 1e-12, "{:?}", t.get("H3"));
}

#[test]
fn a_dynamic_array_of_one_value_still_anchors_a_spill() {
    // In Excel a dynamic-array formula anchors a spill range even while its result is a single
    // value, so `C1#` is the 1 x 1 range and ROWS/SUM/COUNT of it work. GridCraft gave #REF!
    // (and COUNT 0), so every formula counting a filtered list broke the moment the list had
    // exactly one entry (#1491). Values and cells follow the reported fixture.
    let mut t = T::new();
    for i in 1..=5 {
        t.set(&format!("A{i}"), &i.to_string());
        t.set(&format!("B{i}"), "7");
    }
    t.set("C1", "=FILTER(A1:A5,A1:A5>4)");
    t.set("C3", "=SORT(FILTER(A1:A5,A1:A5=3))");
    t.set("C5", "=UNIQUE(B1:B5)");
    // Control: the same FILTER matching two rows, which worked before.
    t.set("C9", "=FILTER(A1:A5,A1:A5>3)");

    t.set("E1", "=ROWS(C1#)");
    t.set("E2", "=SUM(C1#)");
    t.set("E3", "=ROWS(C3#)");
    t.set("E5", "=COUNT(C5#)");
    t.set("E9", "=ROWS(C9#)");
    assert_eq!(t.get("E1"), Value::from(1.0));
    assert_eq!(t.get("E2"), Value::from(5.0));
    assert_eq!(t.get("E3"), Value::from(1.0));
    assert_eq!(t.get("E5"), Value::from(1.0));
    assert_eq!(t.get("E9"), Value::from(2.0), "a spilling array is unchanged");

    // The anchors still show their first value, as they did.
    assert_eq!(t.get("C1"), Value::from(5.0));
    assert_eq!(t.get("C5"), Value::from(7.0));

    // A formula whose result is a scalar never becomes an anchor: `#` on it is still #REF!.
    t.set("D1", "=1+1");
    t.set("E11", "=ROWS(D1#)");
    assert_eq!(t.get("E11"), Value::Error(CellError::Ref));
}

#[test]
fn text_before_after_empty_instance_defaults_to_one() {
    let mut t = T::new();
    t.set("A1", "=TEXTBEFORE(\"Little Red Riding Hood\",\"red\",,1)");
    assert_eq!(t.get("A1"), Value::Text("Little ".into()));
    t.set("A2", "=TEXTAFTER(\"a-b\",\"-\",,,,\"nf\")");
    assert_eq!(t.get("A2"), Value::Text("b".into()));
    t.set("A3", "=TEXTAFTER(\"a-b\",\"x\",,,,\"nf\")");
    assert_eq!(t.get("A3"), Value::Text("nf".into()));
    t.set("A4", "=TEXTBEFORE(\"a-b\",\"-\",0)");
    assert_eq!(t.get("A4"), Value::Error(CellError::Value));
}

#[test]
fn loan_schedule_residue_keeps_excels_sign() {
    // A synthetic monthly loan schedule: interest on the opening balance, principal = PMT - interest,
    // each later period guarded by IF(p - SUM(principal so far) > 0, ...). After the 12th payment
    // Excel is left with 2^-30 > 0, so it takes a 13th, tiny-interest period. That last bit comes
    // from PMT; the old (1 + rate)^nper form left -2.8e-8 and returned "" instead.
    let mut t = T::new();
    t.set("B1", "4653300");
    t.set("B2", "1");
    t.set("B3", "0.141");
    let col = |i: u8| char::from(b'A' + i).to_string();
    t.set("C8", "=$B$1*($B$3/12)");
    t.set("C7", "=PMT($B$3/12,$B$2*12,-$B$1)-C8");
    t.set("D8", "=($B1-C7)*($B$3/12)");
    t.set("D7", "=PMT($B$3/12,$B$2*12,-$B$1)-D8");
    for i in 4..16 {
        let (c, p) = (col(i), col(i - 1));
        t.set(&format!("{c}8"), &format!("=IF($B1-SUM($C7:{p}7)>0,($B1-SUM($C7:{p}7))*($B$3/12),\"\")"));
        t.set(&format!("{c}7"), &format!("=IF($B1-SUM($C7:{p}7)>0,PMT($B$3/12,$B$2*12,-$B$1)-{c}8,\"\")"));
    }
    t.set("N4", "=IF($B$1-SUM($C7:N7)>0,$B$1-SUM($C7:N7),\"\")");
    // Excel's values (Value2) for the same synthetic inputs.
    // The leftover (2^-30 in Excel) is 12 payments' worth of rounding, so a one-ULP libm difference
    // moves it by as much as its own size: only its sign is checked, and the 13th period must exist.
    let positive = |cell: &str| match t.get(cell) {
        Value::Number(x) => assert!(x > 0.0 && x < 1e-6, "{cell}: {x}"),
        other => panic!("{cell}: expected a small positive number, got {other:?}"),
    };
    positive("N4");
    positive("O8");
    match t.get("O7") {
        Value::Number(x) => assert!((x - 418025.37230211904).abs() < 1e-3, "O7: {x}"),
        other => panic!("O7: expected a number, got {other:?}"),
    }
    assert_eq!(t.get("P7"), Value::Text("".into()));
}

#[test]
fn clearing_a_cell_unblocks_the_arrays_above_and_left_of_it() {
    let mut t = T::new();
    for r in 1..=50 {
        t.set(&format!("D{r}"), &format!("={r}*2"));
    }
    t.set("B2", "7");
    t.set("B1", "=SEQUENCE(3)");
    t.set("C1", "=SEQUENCE(1,3)");
    t.set("A20", "=B1+1");
    assert_eq!(t.get("B1"), Value::Error(CellError::Spill));
    assert_eq!(t.get("A20"), Value::Error(CellError::Spill));
    // Emptied: the array above it spills.
    t.wb.sheet_mut(0).unwrap().cells.remove(c("B2"));
    t.calc.cells_changed(&mut t.wb, &[(0, c("B2"))]);
    assert_eq!(t.num("B3"), 3.0);
    assert_eq!(t.num("A20"), 2.0);
    // Typing over the area blocks it again; far from it, nothing is disturbed.
    t.set("B3", "9");
    assert_eq!(t.get("B1"), Value::Error(CellError::Spill));
    t.set("D50", "1");
    assert_eq!(t.get("B1"), Value::Error(CellError::Spill));
    assert_eq!(t.num("D49"), 98.0);
    // Clearing it again frees the array.
    t.set("B3", "");
    assert_eq!(t.num("B3"), 3.0);
}

/// A sheet with `#SPILL!` cells in rows 0..64, columns 100..16384 (as blocked arrays leave), none
/// near column A.
fn spill_error_field() -> T {
    let mut t = T::new();
    let sh = t.wb.sheet_mut(0).unwrap();
    for r in 0..64 {
        for col in 100..16_384 {
            sh.cells.set(CellRef::new(r, col), Cell::value(Value::Error(CellError::Spill)));
        }
    }
    t.calc.recalc_all(&mut t.wb);
    t
}

#[test]
fn an_edit_far_left_of_many_spill_errors_does_not_disturb_them() {
    let mut t = spill_error_field();
    t.set("A65", "5");
    assert_eq!(t.num("A65"), 5.0);
    assert_eq!(t.get("CW1"), Value::Error(CellError::Spill));
    assert_eq!(t.get("XFD64"), Value::Error(CellError::Spill));
    // A blocked formula inside the window is still found when a cell to its right is cleared.
    t.set("DA66", "7");
    t.set("CZ66", "=SEQUENCE(1,3)");
    assert_eq!(t.get("CZ66"), Value::Error(CellError::Spill));
    t.wb.sheet_mut(0).unwrap().cells.remove(c("DA66"));
    t.calc.cells_changed(&mut t.wb, &[(0, c("DA66"))]);
    assert_eq!(t.num("DA66"), 2.0);
}

/// Probe: single-cell edits in column A below a block of `#SPILL!` cells to its right.
/// `cargo test -p gridcraft-calc --release -- --ignored --nocapture spill_error_field_probe`
#[test]
#[ignore]
#[allow(clippy::disallowed_methods)]
fn spill_error_field_probe() {
    let mut t = spill_error_field();
    let mut best = std::time::Duration::MAX;
    for i in 0..9 {
        let start = std::time::Instant::now();
        for _ in 0..100 {
            t.set("A65", &format!("{i}"));
        }
        best = best.min(start.elapsed());
    }
    eprintln!("100 edits of A65 beside 1,040,000 spill errors: best of 9 = {best:?}");
}

/// A full-height column spill (inside the 10,000,000-cell cap) and a paste of 17 cells elsewhere:
/// the paste must not register the spill's 65,536 blocks, and a paste over the spill must still
/// block it.
fn tall_spill_book() -> T {
    let mut t = T::new();
    t.set("A1", "=SEQUENCE(1048576)");
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A1048576"), 1_048_576.0);
    t
}

fn paste_into(t: &mut T, cells: &[(u32, u32)]) {
    let changed: Vec<(usize, CellRef)> = cells.iter().map(|&(r, c)| (0, CellRef::new(r, c))).collect();
    for k in &changed {
        t.wb.sheet_mut(0).unwrap().cells.set(k.1, Cell::value(Value::Number(1.0)));
    }
    t.calc.cells_changed(&mut t.wb, &changed);
}

#[test]
fn a_paste_beside_a_tall_spill_leaves_it_alone_and_one_over_it_blocks_it() {
    let mut t = tall_spill_book();
    let beside: Vec<(u32, u32)> = (0..17).map(|i| (i, 5)).collect();
    paste_into(&mut t, &beside);
    assert_eq!(t.num("A1048576"), 1_048_576.0);
    let over: Vec<(u32, u32)> = (0..17).map(|i| (500_000 + i, if i == 9 { 0 } else { 5 })).collect();
    paste_into(&mut t, &over);
    assert_eq!(t.get("A1"), Value::Error(CellError::Spill));
}

/// Probe: `cells_changed` for a 17-cell paste in column F beside the tall spill.
/// `cargo test -p gridcraft-calc --release -- --ignored --nocapture tall_spill_paste_probe`
#[test]
#[ignore]
#[allow(clippy::disallowed_methods)]
fn tall_spill_paste_probe() {
    let mut t = tall_spill_book();
    let mut best = std::time::Duration::MAX;
    for round in 0..9u32 {
        let beside: Vec<(u32, u32)> = (0..17).map(|i| (round * 20 + i, 5)).collect();
        let start = std::time::Instant::now();
        paste_into(&mut t, &beside);
        best = best.min(start.elapsed());
    }
    eprintln!("tall spill, 17-cell paste beside it: best of 9 = {best:?}");
}

#[test]
fn many_spills_are_read_and_blocked_by_a_paste() {
    let mut t = T::new();
    // 40 arrays of 3 rows side by side in rows 1-3, and a reader of a range over two of them.
    for i in 0..40u32 {
        let anchor = CellRef::new(0, 2 * i);
        t.wb.sheet_mut(0).unwrap().cells.set(anchor, Cell::formula(Formula::new(&format!("=SEQUENCE(3,1,{})", 10 * i))));
    }
    t.set("A10", "=SUM(C1:F3)");
    t.set("A12", "=COUNT(C1:F3)");
    t.set("A11", "=C2*1");
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.num("A10"), (10 + 11 + 12 + 20 + 21 + 22) as f64);
    assert_eq!(t.num("A11"), 11.0);
    assert_eq!(t.num("A12"), 6.0);
    // Pasting 20 cells (more than one spill's width worth, so the changed set is indexed) over
    // the second row of every other array blocks those arrays; the rest are unchanged.
    let changed: Vec<(usize, CellRef)> = (0..20u32).map(|i| (0, CellRef::new(1, 4 * i))).collect();
    for k in &changed {
        t.wb.sheet_mut(0).unwrap().cells.set(k.1, Cell::value(Value::Number(1.0)));
    }
    t.calc.cells_changed(&mut t.wb, &changed);
    assert_eq!(t.get("E1"), Value::Error(CellError::Spill));
    assert_eq!(t.get("C1"), Value::Number(10.0));
    assert_eq!(t.get("G2"), Value::Number(31.0));
    assert_eq!(t.get("A10"), Value::Error(CellError::Spill));
    assert_eq!(t.num("A12"), 4.0);
}

#[test]
fn a_paste_over_small_and_tall_spills_blocks_those_it_touches() {
    let mut t = T::new();
    for i in 0..40u32 {
        t.wb.sheet_mut(0).unwrap().cells.set(CellRef::new(0, 2 * i), Cell::formula(Formula::new("=SEQUENCE(3)")));
    }
    t.wb.sheet_mut(0).unwrap().cells.set(CellRef::new(0, 200), Cell::formula(Formula::new("=SEQUENCE(1048576)")));
    t.calc.recalc_all(&mut t.wb);
    // 20 cells: the middle row of the small arrays in columns 0, 8, 16 ... 72 (columns 80 and up hit
    // nothing), and one deep inside the tall one.
    let mut cells: Vec<(u32, u32)> = (0..19u32).map(|i| (1, 8 * i)).collect();
    cells.push((700_000, 200));
    paste_into(&mut t, &cells);
    assert_eq!(t.get("A1"), Value::Error(CellError::Spill));
    assert_eq!(t.get("GS1"), Value::Error(CellError::Spill));
    assert_eq!(t.num("C1"), 1.0);
    assert_eq!(t.num("C3"), 3.0);
    assert_eq!(t.get("I1"), Value::Error(CellError::Spill));
    assert_eq!(t.num("E3"), 3.0);
}

#[test]
fn a_big_paste_on_one_sheet_and_one_cell_over_many_spills_on_another() {
    let mut t = T::new();
    t.wb.sheets.push(std::sync::Arc::new(gridcraft_model::Sheet::new("Data")));
    for i in 0..1_500u32 {
        t.wb.sheet_mut(1).unwrap().cells.set(CellRef::new(4 * i, 0), Cell::formula(Formula::new(&format!("=SEQUENCE(3,1,{})", 10 * i))));
    }
    t.calc.recalc_all(&mut t.wb);
    assert_eq!(t.wb.sheet(1).unwrap().value(CellRef::new(4 * 700 + 2, 0)), Value::Number(7002.0));
    // 5000 cells pasted on the first sheet, one over the middle of the 701st array on the second.
    let mut changed: Vec<(usize, CellRef)> = (0..5_000u32).map(|i| (0, CellRef::new(i / 50, i % 50))).collect();
    changed.push((1, CellRef::new(4 * 700 + 1, 0)));
    for k in &changed {
        t.wb.sheet_mut(k.0).unwrap().cells.set(k.1, Cell::value(Value::Number(1.0)));
    }
    t.calc.cells_changed(&mut t.wb, &changed);
    let data = t.wb.sheet(1).unwrap();
    assert_eq!(data.value(CellRef::new(4 * 700, 0)), Value::Error(CellError::Spill));
    assert_eq!(data.value(CellRef::new(4 * 699 + 2, 0)), Value::Number(6992.0));
    assert_eq!(data.value(CellRef::new(4 * 701 + 2, 0)), Value::Number(7012.0));
}

/// `SUM`, `COUNT` and `AVERAGE` over blocks of constants share one pass-wide answer per block, and a
/// running total extends the shorter block's. Every result must equal the left-to-right sum of the
/// cells, bit for bit, with text, blanks, errors, formulas and a nonzero start in the mix.
#[test]
fn aggregates_over_ranges_match_a_plain_fold_bit_for_bit() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }
    let nums = [0.1, 0.2, 0.3, 1e16, -1e16, 1.0, 3.7, -0.0, 12345.678, 1e-9, 7.0];
    let rows = 40u32;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut wb = Workbook::new();
    let fresh = |rng: &mut Rng, plain: bool| match rng.next(if plain { 8 } else { 14 }) {
        0 => Cell::value(Value::text("t")),
        1 => Cell::default(),
        2 => Cell::value(Value::Bool(true)),
        3 if !plain => Cell::value(Value::Error(CellError::NA)),
        4 if !plain => Cell::formula(Formula::new("=A1*2")),
        5 if !plain => Cell::formula(Formula::new("=\"x\"&B2")),
        _ => Cell::value(Value::Number(nums[rng.next(nums.len() as u64) as usize])),
    };
    // Columns A:C mostly numbers; E holds a formula column and some errors only in a few rows.
    for r in 0..rows {
        for col in 0..3 {
            let plain = !(r % 13 == 7 && col == 1);
            wb.sheet_mut(0).unwrap().cells.set(CellRef::new(r, col), fresh(&mut rng, plain));
        }
    }
    // Formulas in column H and up: (function, optional leading number, blocks).
    type Oracle<'a> = (CellRef, &'a str, Option<f64>, Vec<(u32, u32, u32, u32)>);
    let mut oracle: Vec<Oracle> = vec![];
    let funcs = ["SUM", "COUNT", "AVERAGE"];
    let mut at = 0;
    let mut add = |wb: &mut Workbook, f: &'static str, lead: Option<f64>, blocks: Vec<(u32, u32, u32, u32)>| {
        let args: Vec<String> = blocks
            .iter()
            .map(|&(r0, c0, r1, c1)| format!("{}{}:{}{}", (b'A' + c0 as u8) as char, r0 + 1, (b'A' + c1 as u8) as char, r1 + 1))
            .collect();
        let lead_s = lead.map(|l| format!("{l:e},")).unwrap_or_default();
        let cell = CellRef::new(at % rows, 7 + at / rows);
        wb.sheet_mut(0).unwrap().cells.set(cell, Cell::formula(Formula::new(&format!("={f}({lead_s}{})", args.join(",")))));
        oracle.push((cell, f, lead, blocks));
        at += 1;
    };
    // Running totals over a clean column and over a column with an error further down.
    for r in 0..rows {
        add(&mut wb, "SUM", None, vec![(0, 0, r, 0)]);
        add(&mut wb, "SUM", None, vec![(0, 1, r, 1)]);
        add(&mut wb, "COUNT", None, vec![(0, 0, r, 2)]);
    }
    for _ in 0..120 {
        let (r0, r1) = (rng.next(rows as u64) as u32, rng.next(rows as u64) as u32);
        let (c0, c1) = (rng.next(3) as u32, rng.next(3) as u32);
        let n = 1 + rng.next(2) as usize;
        let blocks = (0..n).map(|_| (r0.min(r1), c0.min(c1), r0.max(r1), c0.max(c1))).collect();
        let lead = [None, Some(1e16), Some(0.5), Some(2.0)][rng.next(4) as usize];
        let f = funcs[rng.next(3) as usize];
        add(&mut wb, f, lead, blocks);
    }
    let check = |wb: &Workbook, when: &str| {
        let sh = wb.sheet(0).unwrap();
        for (cell, f, lead, blocks) in &oracle {
            let (mut sum, mut count, mut error) = (lead.unwrap_or(0.0), u64::from(lead.is_some()), None);
            for &(r0, c0, r1, c1) in blocks {
                for r in r0..=r1 {
                    for col in c0..=c1 {
                        match sh.value(CellRef::new(r, col)) {
                            Value::Number(n) => {
                                sum += n;
                                count += 1;
                            }
                            Value::Error(e) if *f != "COUNT" => error = error.or(Some(e)),
                            _ => {}
                        }
                    }
                }
            }
            let want = match (error, *f) {
                (Some(e), _) => Value::Error(e),
                (_, "SUM") => Value::number(sum),
                (_, "COUNT") => Value::number(count as f64),
                (_, _) if count == 0 => Value::Error(CellError::Div0),
                _ => Value::number(sum / count as f64),
            };
            let got = sh.value(*cell);
            let same = match (&got, &want) {
                (Value::Number(a), Value::Number(b)) => a.to_bits() == b.to_bits(),
                (a, b) => a == b,
            };
            assert!(same, "{when}: {:?} = {:?}, plain fold {:?}", cell, got, want);
        }
    };
    let mut calc = Calc::new();
    calc.rebuild(&wb);
    calc.recalc_all(&mut wb);
    check(&wb, "full recalc");
    for round in 0..30 {
        let mut changed = vec![];
        for _ in 0..1 + rng.next(4) {
            let at = CellRef::new(rng.next(rows as u64) as u32, rng.next(3) as u32);
            wb.sheet_mut(0).unwrap().cells.set(at, fresh(&mut rng, round % 2 == 0));
            changed.push((0, at));
        }
        calc.cells_changed(&mut wb, &changed);
        check(&wb, &format!("edit round {round}"));
    }
}

/// `rebuild` and `cells_changed` read formulas through `Formula::with_expr`, which borrows the
/// cached tree or parses on the fly. The dependency graph must come out the same for formulas that
/// carry their tree and for formulas that only carry their text (as after loading a file).
#[test]
fn graph_built_from_text_only_formulas_matches_cached() {
    fn run(cached: bool) -> Vec<Value> {
        let mut t = T::new();
        t.set("A1", "2");
        for (at, f) in [("B1", "=A1*10"), ("C1", "=B1+A1"), ("D1", "=SUM(A1:C1)"), ("E1", "=NOPE(")] {
            // E1 does not parse, so it has no tree either way and only covers the `None` branch.
            let mut formula = Formula::new(f);
            if !cached {
                formula.expr = None;
            }
            t.wb.sheet_mut(0).unwrap().cells.set(c(at), Cell::formula(formula));
        }
        t.calc.rebuild(&t.wb);
        t.calc.recalc_all(&mut t.wb);
        let mut out: Vec<Value> = ["B1", "C1", "D1", "E1"].iter().map(|a| t.get(a)).collect();
        // Editing the input must reach the dependents through the rebuilt graph.
        t.set("A1", "5");
        out.extend(["B1", "C1", "D1"].iter().map(|a| t.get(a)));
        // A text-only formula that is rewritten through `cells_changed` joins the graph too.
        let mut formula = Formula::new("=A1+100");
        if !cached {
            formula.expr = None;
        }
        t.wb.sheet_mut(0).unwrap().cells.set(c("F1"), Cell::formula(formula));
        t.calc.cells_changed(&mut t.wb, &[(0, c("F1"))]);
        t.set("A1", "7");
        out.push(t.get("F1"));
        out
    }
    let cached = run(true);
    assert_eq!(cached, run(false));
    assert_eq!(cached[4], Value::Number(50.0));
    assert_eq!(cached[7], Value::Number(107.0));
}

/// Makes `at` a Python cell showing `cached` (its value before this calculation), with `result`
/// as its kept result, and returns the cell's key for `Calc::cells_changed`.
fn plant_python(t: &mut T, at: &str, cached: Value, result: Value) -> (usize, CellRef) {
    use gridcraft_model::python::{PyCode, PyOutput};
    let output = PyOutput { value: result, type_name: "list".into(), preview: String::new(), convertible: true };
    let py = PyCode { output: Some(output), ..PyCode::new("[5, 6]") };
    let cell = Cell { value: cached, formula: None, style: gridcraft_model::StyleId::default(), py: Some(std::sync::Arc::new(py)) };
    t.wb.sheet_mut(0).unwrap().cells.set(c(at), cell);
    (0, c(at))
}

fn column(values: &[f64]) -> Value {
    let data = values.iter().map(|v| Value::Number(*v)).collect();
    Value::Array(std::sync::Arc::new(gridcraft_core::Array::new(values.len(), 1, data).unwrap()))
}

#[test]
fn formulas_read_a_dirty_python_cell_after_it_is_laid_out() {
    // B1's result can't spill (B2 holds a value), so it is #SPILL!. A1, A2 and A3 come before
    // it in the pass; each must lay it out first rather than read its old value, 1.
    let build = |t: &mut T| {
        t.set("B2", "10");
        t.set("B3", "100");
        t.set("A1", "=B1");
        t.set("A2", "=MAX(B1:B3)");
        t.set("A3", "=SUM(B1:B3)");
        t.set("A4", "=COUNTA(B1:B3)");
        t.set("A5", "=SUMPRODUCT(--ISERROR(B1:B3))");
    };
    let check = |t: &T, how: &str| {
        assert_eq!(t.get("B1"), Value::Error(CellError::Spill), "{how}");
        assert_eq!(t.get("A1"), Value::Error(CellError::Spill), "{how}: a reference");
        assert_eq!(t.get("A2"), Value::Error(CellError::Spill), "{how}: a range read");
        assert_eq!(t.get("A3"), Value::Error(CellError::Spill), "{how}: a range walked cell by cell");
        assert_eq!(t.num("A4"), 3.0, "{how}");
        assert_eq!(t.num("A5"), 1.0, "{how}: the error is seen in the range");
    };

    // A full recalculation.
    let mut t = T::new();
    build(&mut t);
    plant_python(&mut t, "B1", Value::Number(1.0), column(&[5.0, 6.0]));
    t.calc.recalc_all(&mut t.wb);
    check(&t, "full");

    // An incremental one: B1 becomes a Python cell after its readers.
    let mut t = T::new();
    t.set("B1", "1");
    build(&mut t);
    assert_eq!((t.num("A1"), t.num("A3")), (1.0, 111.0));
    let k = plant_python(&mut t, "B1", Value::Number(1.0), column(&[5.0, 6.0]));
    t.calc.cells_changed(&mut t.wb, &[k]);
    check(&t, "incremental");

    // Unblocked, the readers see the new array.
    t.set("B2", "");
    assert_eq!((t.num("B1"), t.num("B2")), (5.0, 6.0));
    assert_eq!((t.num("A1"), t.num("A2"), t.num("A3")), (5.0, 100.0, 111.0));
}

#[test]
fn disjoint_spills_do_not_scan_every_spill_owner() {
    // Many independent two-cell arrays: write-back must not walk all earlier owners for each one.
    let mut t = T::new();
    for i in 1..=2000 {
        t.wb.sheet_mut(0).unwrap().cells.set(c(&format!("A{}", i * 2 - 1)), Cell::formula(Formula::new("=SEQUENCE(2)")));
    }
    crate::recalc::OWNER_SCANS.with(|n| n.set(0));
    t.calc.recalc_all(&mut t.wb);
    assert_eq!((t.num("A2"), t.num("A4000")), (2.0, 2.0));
    let scans = crate::recalc::OWNER_SCANS.with(|n| n.get());
    assert!(scans < 10, "{scans} owner scans for 2000 disjoint spills");
}

#[test]
fn overlapping_spill_edit_sequences_match_a_full_recalc() {
    // Random short edit sequences on a small grid of arrays that run into each other (sizes from
    // shared cells, so one edit resizes several at once), with constants and clears in their way.
    // After every edit the incremental result must equal a full recalc of the same workbook.
    let seeds: u64 = std::env::var("GRIDCRAFT_SPILL_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    let steps: usize = std::env::var("GRIDCRAFT_SPILL_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let only: Option<u64> = std::env::var("GRIDCRAFT_SPILL_SEED").ok().and_then(|v| v.parse().ok());
    let next = |state: &mut u64| {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let seeds: Vec<u64> = match only {
        Some(s) => vec![s],
        None => (0..seeds).collect(),
    };
    for seed in seeds {
        let mut state = seed;
        let mut t = T::new();
        // Sizes in J1:J4.
        for (i, n) in [2, 3, 1, 4].iter().enumerate() {
            t.set(&format!("J{}", i + 1), &n.to_string());
        }
        let mut log: Vec<(String, String)> = Vec::new();
        for step in 0..steps {
            let r = next(&mut state);
            let at = format!("{}{}", (b'A' + (r % 6) as u8) as char, 1 + (r >> 8) % 6);
            let kind = (r >> 16) % 14;
            let (at, input) = match kind {
                0..=3 => {
                    let a = 1 + (r >> 24) % 4;
                    let b = 1 + (r >> 28) % 4;
                    (at, format!("=SEQUENCE($J${a},$J${b})"))
                }
                4 => (at, format!("=SEQUENCE({},{})", 1 + (r >> 24) % 3, 1 + (r >> 28) % 3)),
                5 => (format!("J{}", 1 + (r >> 24) % 4), (1 + (r >> 28) % 4).to_string()),
                6 => (at, "x".to_string()),
                7 | 8 => (at, String::new()),
                9 => (format!("K{}", 1 + (r >> 24) % 6), "=SUM(A1:F6)".to_string()),
                10 => {
                    // An earlier edit again (often putting back what a cell held before, as undo does).
                    match log.len() {
                        0 => (at, String::new()),
                        n => log[(r >> 24) as usize % n].clone(),
                    }
                }
                11 => (format!("{}{}", (b'A' + ((r >> 24) % 6) as u8) as char, 1 + (r >> 32) % 6), (1 + (r >> 40) % 3).to_string()),
                // Several cells in one edit (a fill, or clearing a range): a 2x2 block from `at`.
                12 => (format!("{at}+"), String::new()),
                _ => (format!("{at}+"), format!("=SEQUENCE($J${},$J${})", 1 + (r >> 24) % 4, 1 + (r >> 28) % 4)),
            };
            log.push((at.clone(), input.clone()));
            if let Some(from) = at.strip_suffix('+') {
                let from = c(from);
                let keys: Vec<(usize, CellRef)> =
                    (0..2).flat_map(|dr| (0..2).map(move |dc| (0, CellRef { row: from.row + dr, col: from.col + dc }))).collect();
                for k in &keys {
                    let cell = if input.is_empty() { Cell::value(Value::Empty) } else { Cell::formula(Formula::new(&input)) };
                    t.wb.sheet_mut(0).unwrap().cells.set(k.1, cell);
                }
                t.calc.cells_changed(&mut t.wb, &keys);
            } else {
                t.set(&at, &input);
            }
            let mut fresh = t.wb.clone();
            Calc::new().recalc_all(&mut fresh);
            let (a, b) = (t.wb.sheet(0).unwrap(), fresh.sheet(0).unwrap());
            for row in 0..12 {
                for col in 0..12 {
                    let cell = CellRef { row, col };
                    assert_eq!(a.value(cell), b.value(cell), "seed {seed} step {step}: {cell} incremental vs full recalc after {log:?}");
                }
            }
            let mut ka: Vec<_> = a.spill_ranges.iter().map(|(k, v)| (*k, *v)).collect();
            let mut kb: Vec<_> = b.spill_ranges.iter().map(|(k, v)| (*k, *v)).collect();
            ka.sort_by_key(|(k, _)| (k.row, k.col));
            kb.sort_by_key(|(k, _)| (k.row, k.col));
            assert_eq!(ka, kb, "seed {seed} step {step}: spill layout after {log:?}");
        }
    }
}

/// A computed criterion whose formula runs a database function over the same criteria counts as
/// not met inside itself: before, each record's check ran the function again for every record, at
/// every level, so the work grew exponentially with the nesting allowed.
#[test]
fn computed_criterion_that_reenters_itself_is_not_met() {
    let mut t = T::new();
    for (r, (name, age)) in [("name", "age"), ("a", "1"), ("b", "2"), ("c", "3"), ("d", "4"), ("e", "5")].iter().enumerate() {
        t.set(&format!("F{}", r + 2), name);
        t.set(&format!("G{}", r + 2), age);
    }
    t.set("N46", "Formula");
    t.set("N47", "=DCOUNT($F$2:$G$7,\"age\",$N$46:$N$47)>0");
    t.set("B60", "=DCOUNT($F$2:$G$7,\"age\",N46:N47)");
    assert_eq!(t.num("B60"), 0.0);
}
