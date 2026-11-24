use super::*;
use crate::adjust::{Axis, Edit, adjust, rename_sheet, shift_relative};

fn rt(src: &str) -> String {
    match parse(src) {
        Ok(e) => print(&e),
        Err(e) => format!("{src}: {e}"),
    }
}

#[test]
fn range_end_repeating_sheet_name_round_trips() {
    assert_eq!(rt("'a'!A1:'a'!B2"), "a!A1:a!B2");
    assert_eq!(rt("Sheet1!A1:Sheet1!B2"), "Sheet1!A1:Sheet1!B2");
    assert_eq!(rt("SUM(a!A1:a!B2)"), "SUM(a!A1:a!B2)");
    // Sheet names match without regard to case, as Excel's do.
    assert_eq!(rt("Sheet1!A1:SHEET1!B2"), "Sheet1!A1:SHEET1!B2");
}

#[test]
fn quoted_sheet_qualified_name_keeps_quotes() {
    assert_eq!(rt("'My Sheet'!Total"), "'My Sheet'!Total");
    assert_eq!(rt("'Data'!Total"), "Data!Total");
    assert_eq!(rt("'It''s'!Total+1"), "'It''s'!Total+1");
}

#[test]
fn intersection_with_deleted_operand_reparses() {
    // Deleting the columns of an intersection operand prints `A1:B2 #REF!`; that text must parse back.
    assert_eq!(rt("A1:B2 #REF!"), "A1:B2 #REF!");
    assert_eq!(rt("#REF! A1:B2"), "#REF! A1:B2");
    assert_eq!(parse("A1:B2 #REF!"), parse("A1:B2 (#REF!)").map(|e| strip_paren(&e)));
}

fn strip_paren(e: &Expr) -> Expr {
    match e {
        Expr::Binary(op, a, b) => Expr::Binary(*op, Box::new(strip_paren(a)), Box::new(strip_paren(b))),
        Expr::Paren(x) => strip_paren(x),
        other => other.clone(),
    }
}

#[test]
fn roundtrips() {
    for f in [
        "1+2*3",
        "SUM(A1:B2)",
        "SUM($A$1:B$2,C3)",
        "IF(A1>0,\"yes\",\"no\")",
        "-2^2",
        "A1%",
        "Sheet2!A1+'My Sheet'!$B$3",
        "SUM(Sheet1:Sheet3!A1)",
        "SUM(A:A)",
        "SUM(3:5)",
        "{1,2;3,4}",
        "{-1,\"a\",TRUE,#N/A}",
        "VLOOKUP(A1,Table1[#All],2,FALSE)",
        "Table1[Sales]",
        "Table1[@Sales]",
        "Table1[[#Headers],[A]:[C]]",
        "\"a\"\"b\"&C1",
        "SUM((A1:A3,C1:C3))",
        "A1:C3 B2:B5",
        "LAMBDA(x,x+1)(2)",
        "F(1,,3)",
        "@A1:A10",
        "A1<>B1",
        "A1<=B1",
        "#REF!+1",
        "1E+20",
        "MyName*2",
        "Sheet1!MyName",
        "SUM(A1:B2) ",
    ] {
        let p = rt(f);
        assert_eq!(p, f.trim(), "round trip of {f}");
    }
}

#[test]
fn normalises() {
    assert_eq!(rt("=sum(a1:b2)"), "SUM(A1:B2)");
    assert_eq!(rt("= 1 + 2"), "1+2");
    assert_eq!(rt("_xlfn.XLOOKUP(1,A:A,B:B)"), "XLOOKUP(1,A:A,B:B)");
    assert_eq!(rt("_xlfn.GROUPBY(A1:A3,B1:B3,_xleta.SUM)"), "GROUPBY(A1:A3,B1:B3,SUM)");
    assert_eq!(rt("_XLUDF.concat(\"a\",\"b\")"), "_xludf.CONCAT(\"a\",\"b\")");
    assert_eq!(rt("true"), "TRUE");
    assert_eq!(rt("'Sheet1'!A1"), "Sheet1!A1");
}

#[test]
fn spill_operator() -> Result<(), Box<dyn std::error::Error>> {
    for f in ["A1#", "Sheet1!$D$1#", "SUM(B2#)", "'My Sheet'!A1#*2", "ROWS(A1#)+1", "IF(A1,#N/A,#REF!)"] {
        assert_eq!(rt(f), f);
    }
    assert!(matches!(parse("A1#")?, Expr::Unary(UnOp::Spill, _)));
    // Files spell it ANCHORARRAY.
    assert_eq!(rt("_xlfn.ANCHORARRAY(Sheet1!$D$1)"), "Sheet1!$D$1#");
    assert_eq!(rt("SUM(ANCHORARRAY(B2))"), "SUM(B2#)");
    // Only a cell can spill.
    assert!(parse("A1:B2#").is_err());
    assert!(parse("Name#").is_err());
    // Deleting the anchor's sheet leaves #REF!.
    assert_eq!(print(&crate::adjust::delete_sheet(parse("SUM(Data!A1#)")?, "Data", &[])), "SUM(#REF!)");
    Ok(())
}

/// Files spell `@x` as `_xlfn.SINGLE(x)`; an operation inside keeps its parentheses.
#[test]
fn single_is_the_implicit_intersection_operator() {
    assert_eq!(rt("_xlfn.SINGLE(A1:A3)*2"), "@A1:A3*2");
    assert_eq!(rt("_xlfn.SINGLE(A1:A3*2)"), "@(A1:A3*2)");
    assert_eq!(rt("_xlfn.SINGLE(-A1:A3)"), "@(-A1:A3)");
    assert_eq!(rt("SQRT(_xlfn.SINGLE(Data[Qty]))"), "SQRT(@Data[Qty])");
}

#[test]
fn delete_sheet_shrinks_3d_span() -> Result<(), Box<dyn std::error::Error>> {
    let order: Vec<String> = ["Sheet1", "Sheet2", "Sheet3", "Results"].iter().map(|s| s.to_string()).collect();
    let del = |f: &str, n: &str| parse(f).map(|e| print(&crate::adjust::delete_sheet(e, n, &order)));
    assert_eq!(del("SUM(Sheet1:Sheet3!A1)", "Sheet1")?, "SUM(Sheet2:Sheet3!A1)");
    assert_eq!(del("SUM(Sheet1:Sheet3!A1)", "Sheet3")?, "SUM(Sheet1:Sheet2!A1)");
    assert_eq!(del("SUM(Sheet3:Sheet1!A1)", "Sheet1")?, "SUM(Sheet3:Sheet2!A1)");
    assert_eq!(del("SUM(Sheet1:Sheet3!A1)", "Sheet2")?, "SUM(Sheet1:Sheet3!A1)");
    assert_eq!(del("SUM(Sheet1:Sheet2!A1)", "Sheet1")?, "SUM(Sheet2!A1)");
    assert_eq!(del("Sheet1!A1+Sheet3!A1", "Sheet1")?, "#REF!+Sheet3!A1");
    Ok(())
}

#[test]
fn precedence() -> Result<(), Box<dyn std::error::Error>> {
    let e = parse("1+2*3")?;
    assert!(matches!(e, Expr::Binary(BinOp::Add, _, _)));
    let e = parse("-2^2")?;
    assert!(matches!(e, Expr::Binary(BinOp::Pow, _, _)));
    let e = parse("1&2=\"12\"")?;
    assert!(matches!(e, Expr::Binary(BinOp::Eq, _, _)));
    let e = parse("2^3^2")?; // left associative: (2^3)^2
    let Expr::Binary(BinOp::Pow, l, _) = e else { return Err("not a power".into()) };
    assert!(matches!(*l, Expr::Binary(BinOp::Pow, _, _)));
    Ok(())
}

#[test]
fn errors() {
    for bad in ["1+", "SUM(1,2", "(1", "{1,2;3}", "\"abc", "A1:", "1 2", "{A1}", ")", "Sheet1!", "#FOO!"] {
        assert!(parse(bad).is_err(), "{bad} should fail");
    }
}

#[test]
fn no_panic_on_garbage() {
    let samples =
        ["(((((", "[[[", "'''", "!!!", "$$$", "=@@@", "A1:B2:C3:D4", "{,}", "{;}", "F(,,,,)", ":::", "1e", "1e+", ".", "..", "$A$", "XFD1048577"];
    for s in samples {
        let _ = parse(s);
    }
    let deep = "(".repeat(5000) + "1" + &")".repeat(5000);
    assert!(parse(&deep).is_err());
}

#[test]
fn references() -> Result<(), Box<dyn std::error::Error>> {
    let e = parse("SUM(A1:B2)+Sheet2!C3*D4")?;
    let refs: Vec<String> = e.references().iter().map(|r| printer::reference_a1(r)).collect();
    assert_eq!(refs, ["A1:B2", "Sheet2!C3", "D4"]);
    Ok(())
}

#[test]
fn copy_shift() -> Result<(), Box<dyn std::error::Error>> {
    let e = parse("A1+$A1+A$1+$A$1+SUM(B2:C3)")?;
    assert_eq!(print(&shift_relative(e, 2, 1)), "B3+$A3+B$1+$A$1+SUM(C4:D5)");
    let e = parse("A1")?;
    assert_eq!(print(&shift_relative(e, -1, 0)), "#REF!");
    let e = parse("SUM(A:A)")?;
    assert_eq!(print(&shift_relative(e, 5, 1)), "SUM(B:B)");
    Ok(())
}

#[test]
fn insert_delete() -> Result<(), Box<dyn std::error::Error>> {
    let ins = Edit::Insert { axis: Axis::Rows, at: 2, count: 2 };
    let e = parse("A1+A3+SUM(A2:A5)+Other!A3")?;
    assert_eq!(print(&adjust(e, "Sheet1", "Sheet1", &ins)), "A1+A5+SUM(A2:A7)+Other!A3");
    let del = Edit::Delete { axis: Axis::Rows, at: 2, count: 2 }; // rows 3-4
    let e = parse("A1+A3+SUM(A2:A5)+A6")?;
    assert_eq!(print(&adjust(e, "Sheet1", "Sheet1", &del)), "A1+#REF!+SUM(A2:A3)+A4");
    let del = Edit::Delete { axis: Axis::Cols, at: 0, count: 1 };
    let e = parse("SUM(A1:C1)+B2")?;
    assert_eq!(print(&adjust(e, "S", "S", &del)), "SUM(A1:B1)+A2");
    let e = parse("Sheet1!B2")?;
    assert_eq!(print(&adjust(e, "Other", "Sheet1", &del)), "Sheet1!A2");
    let e = parse("SUM(A:A)")?;
    assert_eq!(print(&adjust(e, "S", "S", &del)), "SUM(#REF!)");
    Ok(())
}

#[test]
fn insert_delete_cells() -> Result<(), Box<dyn std::error::Error>> {
    let range = |a: &str| gridcraft_core::RangeRef::parse(a);
    let adj = |f: &str, e: &Edit| parse(f).map(|x| print(&adjust(x, "S", "S", e)));
    // Insert A3:A4, shifting down: column A moves, other columns and wider ranges don't.
    let ins = Edit::InsertCells { axis: Axis::Rows, range: range("A3:A4").ok_or("none")? };
    assert_eq!(adj("SUM(A1:A5)+A2+A3+$A$9+B3+SUM(A1:B5)+SUM(A:A)", &ins)?, "SUM(A1:A7)+A2+A5+$A$11+B3+SUM(A1:B5)+SUM(A:A)");
    // Insert B2 shifting right.
    let ins = Edit::InsertCells { axis: Axis::Cols, range: range("B2").ok_or("none")? };
    assert_eq!(adj("A2+B2+C2+B3+SUM(A2:C2)", &ins)?, "A2+C2+D2+B3+SUM(A2:D2)");
    // Delete A3:A4 shifting up: references into it become #REF!, those below move up.
    let del = Edit::DeleteCells { axis: Axis::Rows, range: range("A3:A4").ok_or("none")? };
    assert_eq!(adj("A2+A3+A4+A5+B5+SUM(A1:A10)+SUM(A3:A4)+SUM(A1:B10)", &del)?, "A2+#REF!+#REF!+A3+B5+SUM(A1:A8)+SUM(#REF!)+SUM(A1:B10)");
    // Delete B1:C2 shifting left.
    let del = Edit::DeleteCells { axis: Axis::Cols, range: range("B1:C2").ok_or("none")? };
    assert_eq!(adj("A1+B1+D1+D2+D3+SUM(A1:E1)", &del)?, "A1+#REF!+B1+B2+D3+SUM(A1:C1)");
    Ok(())
}

#[test]
fn moves_and_renames() -> Result<(), Box<dyn std::error::Error>> {
    let mv = Edit::Move { from: gridcraft_core::RangeRef::parse("A1:B2").ok_or("none")?, to_row: 9, to_col: 3 };
    let e = parse("A1+B2+C3+SUM(A1:B2)")?;
    assert_eq!(print(&adjust(e, "S", "S", &mv)), "D10+E11+C3+SUM(D10:E11)");
    let mv = Edit::Move { from: gridcraft_core::RangeRef::parse("B:C").ok_or("none")?, to_row: 0, to_col: 4 };
    assert_eq!(print(&adjust(parse("SUM(B:B)+SUM($C:$C)+D1")?, "S", "S", &mv)), "SUM(E:E)+SUM($F:$F)+D1");
    let e = parse("Old!A1+'Old'!B2")?;
    assert_eq!(print(&rename_sheet(e, "Old", "New Name")), "'New Name'!A1+'New Name'!B2");
    Ok(())
}

#[test]
fn rename_sheet_updates_qualified_names() -> Result<(), Box<dyn std::error::Error>> {
    let e = parse("Data!MyName+MyName+Other!MyName")?;
    assert_eq!(print(&rename_sheet(e, "data", "Numbers")), "Numbers!MyName+MyName+Other!MyName");
    let e = parse("Data!MyName")?;
    assert_eq!(print(&rename_sheet(e, "Data", "New Name")), "'New Name'!MyName");
    // Stored bare, like a parsed name, so the evaluator finds the sheet and a second rename still matches.
    let e = rename_sheet(parse("Data!MyName")?, "Data", "It's");
    assert_eq!(e, Expr::Name("It's!MyName".into()));
    assert_eq!(print(&rename_sheet(e, "It's", "Totals")), "Totals!MyName");
    // A sheet whose name holds a '!' splits at the last one: renaming "A" leaves "A!B"'s names alone.
    let e = Expr::Name("A!B!MyName".into());
    assert_eq!(rename_sheet(e.clone(), "A", "Z"), e);
    assert_eq!(rename_sheet(e, "a!b", "Z"), Expr::Name("Z!MyName".into()));
    Ok(())
}

#[test]
fn moves_between_sheets() -> Result<(), Box<dyn std::error::Error>> {
    let mv = Edit::MoveToSheet { from: gridcraft_core::RangeRef::parse("A1:B2").ok_or("none")?, to_sheet: "New Name".into(), to_row: 9, to_col: 3 };
    for (host, input, expected) in [
        ("Source", "A1+$B$2+C3+SUM(A1:B2)", "'New Name'!D10+'New Name'!$E$11+C3+SUM('New Name'!D10:E11)"),
        ("Other", "A1+sOuRcE!$B2+Other!B2", "A1+'New Name'!$E11+Other!B2"),
        ("Source", "SUM(A1:C3)+SUM(Source:Other!A1)", "SUM(A1:C3)+SUM(Source:Other!A1)"),
    ] {
        assert_eq!(print(&adjust(parse(input)?, host, "Source", &mv)), expected);
    }
    let edge = Edit::MoveToSheet {
        from: gridcraft_core::RangeRef::parse("A1:B2").ok_or("none")?,
        to_sheet: "New Name".into(),
        to_row: gridcraft_core::MAX_ROWS - 1,
        to_col: 3,
    };
    assert_eq!(print(&adjust(parse("B2")?, "Source", "Source", &edge)), "#REF!");
    for (from, input, row, col, expected) in [("A:A", "$A:$A", 0, 2, "Dest!$C:$C"), ("1:1", "$1:$1", 2, 0, "Dest!$3:$3")] {
        let mv = Edit::MoveToSheet { from: gridcraft_core::RangeRef::parse(from).ok_or("none")?, to_sheet: "Dest".into(), to_row: row, to_col: col };
        assert_eq!(print(&adjust(parse(input)?, "Source", "Source", &mv)), expected);
    }
    Ok(())
}

#[test]
fn r1c1() -> Result<(), Box<dyn std::error::Error>> {
    let e = parse("A1+$B$2+SUM(C:C)")?;
    assert_eq!(print_r1c1(&e, gridcraft_core::CellRef::new(1, 1)), "R[-1]C[-1]+R2C2+SUM(C[1])");
    Ok(())
}

#[test]
fn spans() -> Result<(), Box<dyn std::error::Error>> {
    let toks = lexer::tokenize("SUM(A1, B2)")?;
    let a1 = toks.iter().find(|t| t.tok == lexer::Tok::Word("A1".into())).ok_or("none")?;
    assert_eq!((a1.start, a1.end), (4, 6));
    Ok(())
}

#[test]
fn unbracketed_this_row_column() -> Result<(), Box<dyn std::error::Error>> {
    // Writers such as xlsxwriter spell `[@Units]` as `[[#This Row],Units]`.
    let Expr::Binary(BinOp::Mul, a, _) = parse("[[#This Row],Units]*[[#This Row],Price]")? else { return Err("not a product".into()) };
    let Expr::Struct(s) = *a else { return Err("not a struct".into()) };
    assert_eq!((s.specifiers.as_slice(), s.col_start.as_deref()), ([StructItem::ThisRow].as_slice(), Some("Units")));
    assert_eq!(rt("[[#This Row],Units]*[[#This Row],Price]"), "[@Units]*[@Price]");
    assert_eq!(rt("Sales[[#This Row],Units]"), "Sales[@Units]");
    assert_eq!(rt("Sales[[#This Row], Units ]"), "Sales[@Units]");
    assert_eq!(rt("[[#This Row],Units]+[@[Unit Price]]+Sales[[#This Row],[Qty]]"), "[@Units]+[@[Unit Price]]+Sales[@Qty]");
    // Unexpected text between the brackets is an error, never silently dropped.
    for bad in [
        "[[#This Row]x]",
        "[[#This Row],[A]]x",
        "[[#Data],Col]",
        "[[#This Row],A:B]",
        "[x[#This Row]]",
        "[[#This Row],]x",
        "Sales[[#All],[A]]junk",
        "Sales[[A],[B],[C]]",
        "Sales[@[A]x]",
    ] {
        assert!(parse(bad).is_err(), "{bad} should fail");
    }
    Ok(())
}

#[test]
fn struct_column_keeps_edge_spaces() -> Result<(), Box<dyn std::error::Error>> {
    // The spaces are part of the column name: `Table1[Qty ]` names `Qty `, never `Qty`.
    let Expr::Struct(s) = parse("Table1[Qty ]")? else { return Err("unexpected expression".into()) };
    assert_eq!(s.col_start.as_deref(), Some("Qty "));
    let Expr::Struct(s) = parse("Table1[ Qty]")? else { return Err("unexpected expression".into()) };
    assert_eq!(s.col_start.as_deref(), Some(" Qty"));
    // Excel writes such a column in the double-bracket form.
    assert_eq!(rt("Table1[Qty ]"), "Table1[[Qty ]]");
    for f in ["Table1[[Qty ]]", "Table1[[ Sum for Q2 ]]", "Table1[[#Totals],[Qty ]]", "Table1[@[Qty ]]", "Table1[Unit Price]"] {
        assert_eq!(rt(f), f);
    }
    Ok(())
}

#[test]
fn trim_references() -> Result<(), Box<dyn std::error::Error>> {
    for f in [
        "SUM(A1:.E10)",
        "SUM(A1.:E10)",
        "SUM(A1.:.E10)",
        "SUM(A:.A)",
        "SUM($A.:.$C)",
        "ROWS(1:.5)",
        "ROWS($1.:$5)",
        "ROWS(1.:.5)",
        "Sheet2!$A$1:.B9",
        "Q1.2026!A1.:.B2",
        "'Q1 2026'!A1:.B2",
        "SUM(A1:.A9,B1.:B9)",
        "A1:.A9*2",
    ] {
        assert_eq!(rt(f), f, "round trip of {f}");
    }
    let Expr::Call(_, args) = parse("SUM(A1:.E10)")? else { return Err("unexpected expression".into()) };
    assert!(matches!(args.as_slice(), [Expr::Unary(UnOp::Trim(TrimMode::Trailing), r)] if matches!(**r, Expr::Ref(_))));
    // Decimals, row ranges and dotted names keep their meaning.
    assert_eq!(parse("1.5+.5")?, Expr::Binary(BinOp::Add, Box::new(Expr::Number(1.5)), Box::new(Expr::Number(0.5))));
    for f in ["SUM(1:5)", "Q1.2026!A1:B2", "my.name+1", "SUM(A1:B2)*1.5", "1.:5"] {
        assert_eq!(rt(f), f);
    }
    assert_eq!(rt("'Q1.2026'!A1:B2"), "Q1.2026!A1:B2");
    assert!(matches!(parse("Q1.2026!A1:B2")?, Expr::Ref(_)));
    // File call forms.
    assert_eq!(rt("SUM(_xlfn._TRO_TRAILING(A1:E10))"), "SUM(A1:.E10)");
    assert_eq!(rt("_xlfn._TRO_LEADING(A:A)"), "A.:A");
    assert_eq!(rt("_xlfn._TRO_ALL(Sheet1!A1:B2)"), "Sheet1!A1.:.B2");
    // Not a range: printed as the equivalent call.
    assert_eq!(rt("_xlfn._TRO_ALL(B2)"), "TRIMRANGE(B2,3,3)");
    for bad in ["A1.:", ".:A1", "A1:.:B2", "A1.:.:B2"] {
        assert!(parse(bad).is_err(), "{bad} should fail");
    }
    // R1C1 keeps the operator.
    assert_eq!(print_r1c1(&parse("SUM(A1:.B2)+ROWS(2.:2)")?, gridcraft_core::CellRef::new(0, 0)), "SUM(RC:.R[1]C[1])+ROWS(R[1].:R[1])");
    Ok(())
}

#[test]
fn trim_references_adjust() -> Result<(), Box<dyn std::error::Error>> {
    let ins = Edit::Insert { axis: Axis::Rows, at: 2, count: 2 };
    let e = parse("SUM(A1:.A5)+SUM(B:.B)+ROWS(2.:.4)")?;
    assert_eq!(print(&adjust(e, "S", "S", &ins)), "SUM(A1:.A7)+SUM(B:.B)+ROWS(2.:.6)");
    let del = Edit::Delete { axis: Axis::Cols, at: 0, count: 1 };
    let e = parse("SUM(A1.:C1)+SUM(A:.A)")?;
    assert_eq!(print(&adjust(e, "S", "S", &del)), "SUM(A1.:B1)+SUM(#REF!)");
    assert_eq!(print(&shift_relative(parse("SUM(A1.:.B2)")?, 1, 1)), "SUM(B2.:.C3)");
    let refs: Vec<String> = parse("SUM(Sheet2!A1:.B5)")?.references().iter().map(|r| printer::reference_a1(r)).collect();
    assert_eq!(refs, ["Sheet2!A1:B5"]);
    Ok(())
}

#[test]
fn prints_long_flat_chain_without_deep_recursion() -> Result<(), Box<dyn std::error::Error>> {
    // Small stack: a recursive printer overflows long before 20000 levels.
    let handle = std::thread::Builder::new().stack_size(256 * 1024).spawn(|| {
        let mut e = Expr::Number(1.0);
        for _ in 0..20000 {
            e = Expr::Binary(BinOp::Add, Box::new(e), Box::new(Expr::Number(1.0)));
        }
        let text = print(&e);
        let r1c1 = print_r1c1(&e, gridcraft_core::CellRef { row: 0, col: 0 });
        // Dropping the deep tree recurses; leak it so only the printer is under test.
        std::mem::forget(e);
        (text, r1c1)
    })?;
    let (text, r1c1) = handle.join().map_err(|_| "thread panicked")?;
    assert_eq!(text, vec!["1"; 20001].join("+"));
    assert_eq!(r1c1, text);
    Ok(())
}

#[test]
fn hostile_formulas_are_errors_not_stack_overflows() -> Result<(), Box<dyn std::error::Error>> {
    // A small stack: parsing must fail cleanly instead of building trees deeper than the nesting
    // limit, which dropping, printing and evaluating walk recursively.
    let handle = std::thread::Builder::new().stack_size(512 * 1024).spawn(|| {
        let percents = format!("1{}", "%".repeat(1_000));
        let invokes = format!("LAMBDA(x,x)(1){}", "(1)".repeat(1_000));
        let long_chain = format!("1{}", "+1".repeat(100_000));
        let huge_percents = format!("1{}", "%".repeat(1_000_000));
        (parse(&percents).is_err(), parse(&invokes).is_err(), parse(&long_chain).is_err(), parse(&huge_percents).is_err())
    })?;
    assert_eq!(handle.join().map_err(|_| "thread panicked")?, (true, true, true, true));
    // Within the limits these still parse.
    assert!(parse(&format!("1{}", "%".repeat(100))).is_ok());
    assert!(parse("LAMBDA(x,x)(1)(2)").is_ok());
    Ok(())
}

#[test]
fn formulas_longer_than_excel_allows_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let at_limit = format!("1{}", "+1".repeat((crate::MAX_FORMULA_LEN - 1) / 2));
    assert_eq!(at_limit.chars().count(), crate::MAX_FORMULA_LEN - 1);
    assert!(parse(&at_limit).is_ok());
    assert!(parse(&format!("={at_limit}")).is_ok(), "the leading = does not count");
    let over = format!("{at_limit}+1");
    let err = parse(&over).err().ok_or("none")?;
    assert!(err.msg.contains("8192"), "{}", err.msg);
    // Characters, not bytes: 8,000 two-byte characters in a text literal are within the limit.
    let text = format!("\"{}\"", "é".repeat(8_000));
    assert!(text.len() > crate::MAX_FORMULA_LEN);
    assert!(parse(&text).is_ok());
    Ok(())
}

#[test]
fn sheet_names_excel_quotes_stay_quoted() {
    // Excel quotes R1C1-looking names, a leading `.`, cells within the sheet and booleans (#1458).
    for name in ["R", "C", "RC", "R1C", "RC1", "R1C1", "r2c3", "R1", "C1", "A1", "XFD1", "AB1048576", ".x", "TRUE", "false"] {
        assert_eq!(rt(&format!("'{name}'!A1")), format!("'{name}'!A1"), "{name}");
    }
    // Names Excel leaves bare.
    for name in ["XFE1", "AB1048577", "Sheet.1", "_x", "A1A", "\u{3a3}"] {
        assert_eq!(rt(&format!("'{name}'!A1")), format!("{name}!A1"), "{name}");
    }
}

#[test]
fn longest_flat_chains_clone_compare_walk_and_adjust_without_deep_recursion() -> Result<(), Box<dyn std::error::Error>> {
    // `=1+1+…` with 4,096 operands is 8,192 characters, Excel's formula limit, and nests 4,095
    // levels on its left. On a 1 MiB stack (the Windows main thread) the derived, recursive
    // Clone overflowed when the engine stored such a formula (#916).
    let handle = std::thread::Builder::new().stack_size(1 << 20).spawn(|| {
        let plus = vec!["1"; 4096].join("+");
        let e = parse(&plus)?;
        let copy = e.clone();
        assert!(copy == e);
        let other = parse(&format!("{}-1", vec!["1"; 4095].join("+")))?;
        assert!(other != e);
        let mut nodes = 0;
        e.walk(&mut |_| nodes += 1);
        assert_eq!(nodes, 2 * 4096 - 1);
        assert_eq!(print(&copy.map(&mut |x| x)), plus);
        // A chain of references: walk finds them in order, adjust rewrites every one.
        let refs = (1..=1000).map(|r| format!("A{r}")).collect::<Vec<_>>().join("+");
        let e = parse(&refs)?;
        let found: Vec<String> = e.references().iter().map(|r| printer::reference_a1(r)).collect();
        assert_eq!(found.first().map(String::as_str), Some("A1"));
        assert_eq!(found.last().map(String::as_str), Some("A1000"));
        assert_eq!(found.len(), 1000);
        let ins = Edit::Insert { axis: Axis::Rows, at: 0, count: 1 };
        let moved = print(&adjust(e.clone(), "S", "S", &ins));
        assert_eq!(moved, (2..=1001).map(|r| format!("A{r}")).collect::<Vec<_>>().join("+"));
        assert_eq!(print(&shift_relative(e, 0, 1)), (1..=1000).map(|r| format!("B{r}")).collect::<Vec<_>>().join("+"));
        Ok::<(), crate::ParseError>(())
    })?;
    handle.join().map_err(|_| "thread panicked")??;
    Ok(())
}

#[test]
fn spanish_input_counts_left_operand_of_a_chain_at_the_same_depth() -> Result<(), Box<dyn std::error::Error>> {
    // The leftmost operand of a flat chain is not nested, so `(((…1)))+1` is accepted whenever
    // `(((…1)))` is, in both the pre-check and `normalize`.
    let wrap = |n: usize| {
        let mut e = Expr::Number(1.0);
        for _ in 0..n {
            e = Expr::Paren(Box::new(e));
        }
        e
    };
    let es = |e: &Expr| crate::print_input(e, FormulaLocale::Es, |_| false);
    let deepest = (0..400).take_while(|&n| es(&wrap(n)).is_ok()).last().ok_or("none")?;
    assert!(deepest > 1 && deepest < 400);
    let chain = Expr::Binary(BinOp::Add, Box::new(wrap(deepest)), Box::new(Expr::Number(1.0)));
    assert!(es(&chain).is_ok());
    Ok(())
}

#[test]
fn walk_is_preorder_and_map_is_postorder_left_to_right() -> Result<(), Box<dyn std::error::Error>> {
    // `(A1+B1)+SUM(C1,D1)-E1`: a chain with a call on its right. walk visits parents before
    // children, map rewrites children (left to right) before their parent.
    let e = parse("(A1+B1)+SUM(C1,D1)-E1")?;
    let mut walked = Vec::new();
    e.walk(&mut |n| walked.push(print(n)));
    assert_eq!(walked, ["(A1+B1)+SUM(C1,D1)-E1", "(A1+B1)+SUM(C1,D1)", "(A1+B1)", "A1+B1", "A1", "B1", "SUM(C1,D1)", "C1", "D1", "E1"]);
    let mut mapped = Vec::new();
    let out = e.map(&mut |n| {
        mapped.push(print(&n));
        n
    });
    assert_eq!(mapped, ["A1", "B1", "A1+B1", "(A1+B1)", "C1", "D1", "SUM(C1,D1)", "(A1+B1)+SUM(C1,D1)", "E1", "(A1+B1)+SUM(C1,D1)-E1"]);
    assert_eq!(print(&out), "(A1+B1)+SUM(C1,D1)-E1");
    Ok(())
}

#[test]
fn missing_closing_parentheses_are_closed_and_errors_describe_the_typed_text() -> Result<(), Box<dyn std::error::Error>> {
    // #2030: `1+` reported "unexpected `)`", the error of the last retry with `)` appended.
    let entry = |src: &str| parse_closing_parens(src, parse);
    assert_eq!(entry("SUM(1,2").map(|e| print(&e)), Ok("SUM(1,2)".to_string()));
    assert_eq!(entry("((1+2").map(|e| print(&e)), Ok("((1+2))".to_string()));
    assert_eq!(entry("1+2").map(|e| print(&e)), Ok("1+2".to_string()));
    for src in ["1+", "--", "SUM(1,+", "1+*2", &format!("{}1", "(".repeat(9))] {
        assert_eq!(entry(src).err().ok_or("none")?, parse(src).err().ok_or("none")?, "{src}");
    }
    assert_eq!(entry("1+").err().ok_or("none")?.msg, "unexpected end of formula");
    Ok(())
}

#[test]
fn number_literal_too_large_for_f64_is_a_parse_error() {
    for bad in ["1E+400", "-1E+400", "1E+400*A1", "SUM(1E309,1)", "9".repeat(400).as_str()] {
        assert!(parse(bad).is_err(), "{bad} should fail");
    }
    assert!(parse("1E+308").is_ok());
}

#[test]
fn three_d_span_and_cell_start_still_lex_as_span() {
    assert_eq!(rt("SUM(Sheet1:Sheet3!A1)"), "SUM(Sheet1:Sheet3!A1)");
    assert_eq!(rt("SUM(Q1:Sheet3!A1)"), "SUM('Q1:Sheet3'!A1)");
}

/// References print the same through the appending printer for every reference shape.
#[test]
fn reference_shapes_print_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    for text in [
        "A1",
        "XFD1048576",
        "$AA$10:AB$11",
        "$A:XFD",
        "B:$C",
        "1:$1048576",
        "$3:4",
        "'My Sheet'!A1",
        "'It''s'!$B$2:C3",
        "Sheet1:Sheet3!B2",
        "'Sheet 1:Sheet3'!B2",
        "SUM(A1:.B3)+SUM(A.:.B)",
    ] {
        let e = parse(text)?;
        assert_eq!(print(&e), text);
        if let Expr::Ref(r) = &e {
            assert_eq!(printer::reference_a1(r), text);
        }
    }
    Ok(())
}

#[test]
fn column_starting_with_at_round_trips() {
    for name in ["@a", "a@b"] {
        let s = StructRef { table: "T".into(), specifiers: vec![], col_start: Some(name.into()), col_end: None };
        let text = format!("={}", printer::struct_ref(&s));
        assert_eq!(parse(&text).ok(), Some(Expr::Struct(s)), "printed as {text}");
    }
}

#[test]
fn wrappers_stacked_across_levels_are_bounded_as_a_whole() {
    // Each level adds 40 percents inside the parser's per-level bound, so no level is too deep alone.
    let stacked = |levels: usize| {
        let mut f = "1".to_string();
        for _ in 0..levels {
            f = format!("({f}){}", "%".repeat(40));
        }
        f
    };
    assert!(parse(&stacked(5)).is_ok());
    assert!(parse(&stacked(7)).is_err(), "7 x 41 wrappers on one path");
    assert!(parse(&format!("{}1{}", "(".repeat(200), ")".repeat(200))).is_ok());
    assert!(parse(&format!("={}", vec!["A1%"; 100].join("+"))).is_ok(), "siblings are not stacked");
}
