//! Formula text conversion between the model (canonical text, no `=`) and files (`_xlfn.`
//! prefixes for newer functions).

use gridcraft_formula::Expr;
use gridcraft_model::Formula;

/// Formula text for a file.
pub fn to_file(f: &Formula) -> String {
    match f.expr_arc() {
        // Most formulas have nothing the file spells differently: print them without copying.
        Some(e) if !needs_file_spelling(&e) => gridcraft_formula::print(&e),
        Some(e) => expr_to_file(Expr::clone(&e)),
        None => f.text.clone(),
    }
}

/// Whether [`expr_to_file`] would change anything in `e`: a LET or LAMBDA (parameter prefixes),
/// a function with a file prefix, a spill, `@` or trim operator, or a structured reference.
fn needs_file_spelling(e: &Expr) -> bool {
    let mut found = false;
    e.walk(&mut |x| {
        found |= match x {
            Expr::Call(name, _) => name == "LET" || name == "LAMBDA" || crate::tables::file_function_name(name).is_some(),
            Expr::Unary(gridcraft_formula::UnOp::Spill | gridcraft_formula::UnOp::At | gridcraft_formula::UnOp::Trim(_), _) => true,
            Expr::Struct(_) => true,
            _ => false,
        }
    });
    found
}

/// Arbitrary formula text (names, CF, validation) for a file. Unparseable text is kept.
pub fn text_to_file(text: &str) -> String {
    let body = text.strip_prefix('=').unwrap_or(text);
    match gridcraft_formula::parse(body) {
        Ok(e) => expr_to_file(e),
        Err(_) => body.to_string(),
    }
}

const PARAM_PREFIX: &str = "_xlpm.";

/// Adds Excel's `_xlpm.` prefix to LET/LAMBDA parameter names, at the declaration and at every use in scope.
fn prefix_params(e: Expr, scope: &[String]) -> Expr {
    let sub = |x: Box<Expr>, sc: &[String]| Box::new(prefix_params(*x, sc));
    match e {
        Expr::Name(n) if scope.iter().any(|s| s.eq_ignore_ascii_case(&n)) => Expr::Name(format!("{PARAM_PREFIX}{n}")),
        Expr::Unary(op, x) => Expr::Unary(op, sub(x, scope)),
        Expr::Paren(x) => Expr::Paren(sub(x, scope)),
        Expr::Binary(op, a, b) if !matches!(*a, Expr::Binary(..)) => Expr::Binary(op, sub(a, scope), sub(b, scope)),
        Expr::Binary(op, a, b) => {
            // A long chain such as A1+A2+… nests on its left: follow it in a loop, not one
            // level of recursion per operand.
            let mut spine = vec![(op, b)];
            let mut left = *a;
            while let Expr::Binary(op, a, b) = left {
                spine.push((op, b));
                left = *a;
            }
            let mut acc = prefix_params(left, scope);
            while let Some((op, b)) = spine.pop() {
                acc = Expr::Binary(op, Box::new(acc), sub(b, scope));
            }
            acc
        }
        Expr::Invoke(c, args) => Expr::Invoke(sub(c, scope), args.into_iter().map(|a| prefix_params(a, scope)).collect()),
        Expr::Call(name, args) if name == "LET" || name == "LAMBDA" => {
            let last = args.len().saturating_sub(1);
            let is_let = name == "LET";
            let mut sc = scope.to_vec();
            let mut pending: Option<String> = None;
            let mut out = Vec::with_capacity(args.len());
            for (i, a) in args.into_iter().enumerate() {
                let declares = i < last && (!is_let || i % 2 == 0);
                match a {
                    Expr::Name(n) if declares => {
                        out.push(Expr::Name(format!("{PARAM_PREFIX}{n}")));
                        if is_let {
                            pending = Some(n);
                        } else {
                            sc.push(n);
                        }
                    }
                    other => {
                        out.push(prefix_params(other, &sc));
                        // A LET name is in scope only after its value expression.
                        if let Some(n) = pending.take() {
                            sc.push(n);
                        }
                    }
                }
            }
            Expr::Call(name, out)
        }
        // A LET-bound LAMBDA called by name, e.g. `f(3)`.
        Expr::Call(name, args) if scope.iter().any(|s| s.eq_ignore_ascii_case(&name)) => {
            Expr::Call(format!("{PARAM_PREFIX}{name}"), args.into_iter().map(|a| prefix_params(a, scope)).collect())
        }
        Expr::Call(name, args) => Expr::Call(name, args.into_iter().map(|a| prefix_params(a, scope)).collect()),
        other => other,
    }
}

fn expr_to_file(e: Expr) -> String {
    let e = prefix_params(e, &[]);
    let e = e.map(&mut |x| match x {
        Expr::Call(name, args) => match crate::tables::file_function_name(&name) {
            Some(n) => Expr::Call(n, args),
            None => Expr::Call(name, args),
        },
        // Files have no `#` operator: Excel writes `A1#` as `_xlfn.ANCHORARRAY(A1)`, and a spill
        // reference that became `#REF!` (its cell was deleted) as plain `#REF!`.
        Expr::Unary(gridcraft_formula::UnOp::Spill, r) if matches!(*r, Expr::Error(_)) => *r,
        Expr::Unary(gridcraft_formula::UnOp::Spill, r) => Expr::Call("_xlfn.ANCHORARRAY".into(), vec![*r]),
        // Nor an `@` operator: Excel writes `@x` as `_xlfn.SINGLE(x)`. The reader puts back the
        // parentheses of `@(A1:A3*2)`, `@(-A1:A3)` and the like, so only those are dropped;
        // others (a union's, redundant ones) are kept so the text reads back the same.
        Expr::Unary(gridcraft_formula::UnOp::At, x) => Expr::Call(
            "_xlfn.SINGLE".into(),
            vec![match *x {
                Expr::Paren(inner) if rewrapped_by_single(&inner) => *inner,
                other => other,
            }],
        ),
        // Nor the `[@Col]` shorthand: files spell it `[[#This Row],[Col]]`, or Excel repairs
        // the file and drops the table. Carried as a name, which prints verbatim.
        Expr::Struct(st) => Expr::Name(gridcraft_formula::printer::struct_ref_file(&st)),
        // Nor trim-reference operators: written as the equivalent `TRIMRANGE(range,n,n)`, which
        // every Excel with trim references reads. (Excel's own file spelling of `A1:.E10` is
        // not publicly documented; the `_xlfn._TRO_*` call forms are accepted on read.)
        Expr::Unary(gridcraft_formula::UnOp::Trim(mode), x) => {
            let code = Expr::Number(f64::from(mode.code()));
            Expr::Call("_xlfn.TRIMRANGE".into(), vec![*x, code.clone(), code])
        }
        other => other,
    });
    gridcraft_formula::print(&e)
}

/// The operands whose parentheses the reader adds back to `_xlfn.SINGLE(x)` (see the formula
/// parser): a binary operation other than a union, and a prefix or postfix operator.
fn rewrapped_by_single(e: &Expr) -> bool {
    use gridcraft_formula::{BinOp, UnOp};
    matches!(e, Expr::Binary(op, ..) if *op != BinOp::Union) || matches!(e, Expr::Unary(UnOp::Neg | UnOp::Plus | UnOp::Percent, _))
}

/// `_xlpm.x` → `x` (any case, as the parser upper-cases function names).
fn strip_param_prefix(n: String) -> String {
    match (n.get(..PARAM_PREFIX.len()), n.get(PARAM_PREFIX.len()..)) {
        (Some(p), Some(rest)) if p.eq_ignore_ascii_case(PARAM_PREFIX) => rest.to_string(),
        _ => n,
    }
}

/// Formula text read from a file → model text (strips prefixes when it parses).
pub fn from_file(text: &str) -> String {
    let body = text.strip_prefix('=').unwrap_or(text);
    match gridcraft_formula::parse(body) {
        Ok(e) => gridcraft_formula::print(&strip_param_prefixes(e)),
        Err(_) => body.to_string(),
    }
}

fn strip_param_prefixes(e: Expr) -> Expr {
    e.map(&mut |x| match x {
        Expr::Name(n) => Expr::Name(strip_param_prefix(n)),
        Expr::Call(n, args) => Expr::Call(strip_param_prefix(n), args),
        other => other,
    })
}

/// The function Google Sheets' XLSX export wraps formulas in when it thinks Excel lacks one of
/// their functions.
const DUMMY_FUNCTION: &str = "__xludf.DUMMYFUNCTION";

/// Whether `text` may hold Google Sheets' export wrapper: a cheap test before parsing.
fn mentions_dummy_function(text: &str) -> bool {
    const NEEDLE: &[u8] = b"DUMMYFUNCTION";
    text.as_bytes().windows(NEEDLE.len()).any(|w| w.eq_ignore_ascii_case(NEEDLE))
}

/// The model text of `text` (a formula as read, see [`from_file`]) with its Google Sheets export
/// wrappers replaced (see [`crate::read_xlsx_with`]), or `None` when nothing changes.
pub(crate) fn unwrap_google_sheets(text: &str, known: &dyn Fn(&str) -> bool) -> Option<String> {
    if !mentions_dummy_function(text) {
        return None;
    }
    let e = gridcraft_formula::parse(text.strip_prefix('=').unwrap_or(text)).ok()?;
    let unwrapped = unwrap_dummy_functions(e, known)?;
    Some(gridcraft_formula::print(&strip_param_prefixes(unwrapped)))
}

/// The original formula inside Google Sheets' export wrapper
/// `IFERROR(__xludf.DUMMYFUNCTION("<formula, quotes doubled>"),<cached value>)`, if `e` is that
/// wrapper, the text parses and every function it calls is `known`. The parser has already undone
/// the doubled quotes. The cached value is also the cell's `<v>`, which the reader keeps.
fn dummy_function_inner(e: &Expr, known: &dyn Fn(&str) -> bool) -> Option<Expr> {
    let Expr::Call(name, args) = e else { return None };
    // The parser has already dropped an `_xlfn.` prefix from IFERROR.
    if !name.eq_ignore_ascii_case("IFERROR") {
        return None;
    }
    let [Expr::Call(dummy, inner), _fallback] = args.as_slice() else { return None };
    if !dummy.eq_ignore_ascii_case(DUMMY_FUNCTION) {
        return None;
    }
    let [Expr::Text(src)] = inner.as_slice() else { return None };
    // The text came out of a formula within the length limit, so it is within it too; check
    // anyway rather than rely on that.
    if src.len() > gridcraft_formula::MAX_FORMULA_LEN && src.chars().count() > gridcraft_formula::MAX_FORMULA_LEN {
        return None;
    }
    let parsed = gridcraft_formula::parse(src).ok()?;
    // A function GridCraft can't calculate would show `#NAME?` where Excel shows the cached
    // value; a wrapper inside the text is one too, so only one level is ever unwrapped.
    let mut all_known = true;
    parsed.walk(&mut |x| {
        if let Expr::Call(n, _) = x {
            all_known &= known(n);
        }
    });
    all_known.then_some(parsed)
}

/// `e` with each Google Sheets export wrapper (see [`dummy_function_inner`]) replaced by the
/// formula it holds, for the whole formula or anywhere inside it, or `None` when none is. If the
/// result no longer parses (nesting beyond the parser's limit), nothing is replaced.
fn unwrap_dummy_functions(e: Expr, known: &dyn Fn(&str) -> bool) -> Option<Expr> {
    // The whole formula: no parentheses needed.
    if let Some(inner) = dummy_function_inner(&e, known) {
        return Some(inner);
    }
    let mut found = false;
    let rewritten = e.map(&mut |x| match dummy_function_inner(&x, known) {
        // The printer writes no parentheses of its own, so keep the inner formula's precedence
        // with explicit ones unless it is a single operand.
        Some(
            inner @ (Expr::Call(..)
            | Expr::Ref(_)
            | Expr::Name(_)
            | Expr::Number(_)
            | Expr::Text(_)
            | Expr::Bool(_)
            | Expr::Error(_)
            | Expr::Paren(_)
            | Expr::Array(_)
            | Expr::Struct(_)),
        ) => {
            found = true;
            inner
        }
        Some(inner) => {
            found = true;
            Expr::Paren(Box::new(inner))
        }
        None => x,
    });
    if !found {
        return None;
    }
    gridcraft_formula::parse(&gridcraft_formula::print(&rewritten)).ok()?;
    Some(rewritten)
}

/// What the engine passes as `known` (`gridcraft_calc::is_known_function`), for tests here: the
/// function library plus the evaluator's own functions the tests use.
#[cfg(test)]
pub(crate) fn test_known_function(n: &str) -> bool {
    gridcraft_functions::lookup(n).is_some() || ["IF", "IFERROR", "ROW", "INDEX", "LET", "LAMBDA"].iter().any(|s| s.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Formulas without a file spelling skip the rewrite; the text must be what it would give.
    #[test]
    fn plain_formulas_print_as_the_rewrite_would() {
        for text in [
            "SUM(A1:B2)*2",
            "A1&\"x\"\"y\"",
            "Sheet2!$A$1+'My Sheet'!C:C-$3:4",
            "IF(A1>0,{1,2;3,4},-B$2%)",
            "Sheet1:Sheet3!A1",
            "INDEX(A:A,1):B5",
            "Total*2",
            "XLOOKUP(1,A:A,B:B)",
            "LET(x,1,x)",
            "LAMBDA(a,a)(1)",
            "A1#",
            "@A1:A2",
            "SUM(A1:.B3)",
            "T[@C]",
        ] {
            let f = Formula::new(text);
            assert_eq!(to_file(&f), expr_to_file(f.expr().unwrap()), "{text}");
        }
    }

    /// The fast path in [`to_file`] skips the rewrite on a list of what triggers it. Run both paths
    /// over every registered function, alone, nested and next to every operator, so a function or
    /// rewrite added later without updating [`needs_file_spelling`] fails here.
    #[test]
    fn every_registered_function_prints_as_the_rewrite_would() {
        let shapes = [
            "{}",
            "1+{}",
            "-{}%",
            "SUM({},A1:B2)",
            "IF(A1,{},Sheet2!$C$3)",
            "LET(x,{},x)",
            "LAMBDA(a,{})(1)",
            "@{}",
            "{}&A1#",
            "{}+SUM(A1:.B3)",
            "{}+T[@C]",
            "{}+T[[#All],[C]]",
            "({})*{{1,2;3,4}}",
            "{}:B5 C1:D2",
            "{}+My Sheet!A:A+Total",
        ];
        let mut checked = 0;
        for spec in gridcraft_functions::all() {
            let n = spec.min_args.max(1).min(spec.max_args).min(4);
            let args = ["A1", "B2:C3", "1", "\"x\""].get(..n).unwrap_or_default().join(",");
            let call = format!("{}({args})", spec.name);
            for shape in shapes {
                let text = shape.replace("{}", &call).replace("{{", "{").replace("}}", "}");
                let f = Formula::new(&text);
                let Some(e) = f.expr() else { continue };
                assert_eq!(to_file(&f), expr_to_file(e), "{text}");
                checked += 1;
            }
        }
        assert!(checked > gridcraft_functions::all().len() * 10, "only {checked} formulas parsed");
    }

    /// NETWORKDAYS.INTL and WORKDAY.INTL are in the file format's own function list: saved without
    /// `_xlfn.` (LibreOffice gives #NAME? for the prefixed spelling), and the prefixed spelling of a
    /// file written by an older version still reads (#3517, #2775).
    #[test]
    fn intl_day_functions_are_saved_without_the_prefix() {
        let f = Formula::new("NETWORKDAYS.INTL(DATE(2026,1,5),DATE(2026,1,9),1)+WORKDAY.INTL(DATE(2026,1,5),3,1)");
        assert_eq!(to_file(&f), "NETWORKDAYS.INTL(DATE(2026,1,5),DATE(2026,1,9),1)+WORKDAY.INTL(DATE(2026,1,5),3,1)");
        assert_eq!(text_to_file("=WORKDAY.INTL(A1,3,1)"), "WORKDAY.INTL(A1,3,1)");
        assert_eq!(from_file("_xlfn.NETWORKDAYS.INTL(A1,B1,1)+_xlfn.WORKDAY.INTL(A1,3,1)"), "NETWORKDAYS.INTL(A1,B1,1)+WORKDAY.INTL(A1,3,1)");
        assert_eq!(from_file("NETWORKDAYS.INTL(A1,B1,1)"), "NETWORKDAYS.INTL(A1,B1,1)");
    }

    #[test]
    fn spill_references() {
        assert_eq!(to_file(&Formula::new("SUM(Sheet1!$D$1#)+B2#")), "SUM(_xlfn.ANCHORARRAY(Sheet1!$D$1))+_xlfn.ANCHORARRAY(B2)");
        assert_eq!(text_to_file("=Sheet1!$D$1#"), "_xlfn.ANCHORARRAY(Sheet1!$D$1)");
        assert_eq!(from_file("SUM(_xlfn.ANCHORARRAY(Sheet1!$D$1))+_xlfn.ANCHORARRAY(B2)"), "SUM(Sheet1!$D$1#)+B2#");
        // Found by the command-sequence fuzzer: deleting the anchor of `H5#` left `=#REF!` in
        // the workbook but `ANCHORARRAY(#REF!)` in the file, which read back differently.
        let deleted = Formula::from_expr(Expr::Unary(gridcraft_formula::UnOp::Spill, Box::new(Expr::Error(gridcraft_core::CellError::Ref))));
        assert_eq!(deleted.text, "#REF!");
        assert_eq!(to_file(&deleted), "#REF!");
    }

    #[test]
    fn implicit_intersection() {
        assert_eq!(to_file(&Formula::new("SUM(@A1:A3*B1:B3)")), "SUM(_xlfn.SINGLE(A1:A3)*B1:B3)");
        assert_eq!(to_file(&Formula::new("@A1:A3*2")), "_xlfn.SINGLE(A1:A3)*2");
        assert_eq!(text_to_file("=@Sheet1!$A$1:$A$3"), "_xlfn.SINGLE(Sheet1!$A$1:$A$3)");
        assert_eq!(from_file("_xlfn.SINGLE(A1:A3)*2"), "@A1:A3*2");
        assert_eq!(from_file("SUM(_xlfn.SINGLE(A1:A3)*B1:B3)"), "SUM(@A1:A3*B1:B3)");
    }

    #[test]
    fn trim_references() {
        assert_eq!(to_file(&Formula::new("SUM(A1:.E10)+SUM(Data!A.:A)")), "SUM(_xlfn.TRIMRANGE(A1:E10,2,2))+SUM(_xlfn.TRIMRANGE(Data!A:A,1,1))");
        assert_eq!(text_to_file("=ROWS(1.:.5)"), "ROWS(_xlfn.TRIMRANGE(1:5,3,3))");
        assert_eq!(from_file("SUM(_xlfn._TRO_TRAILING(A1:E10))+_xlfn._TRO_ALL($A:$B)"), "SUM(A1:.E10)+$A.:.$B");
    }

    #[test]
    fn unbracketed_this_row_column() {
        assert_eq!(from_file("Sales[[#This Row],Units]*[[#This Row],Price]"), "Sales[@Units]*[@Price]");
    }

    #[test]
    fn this_row_long_form() {
        // `[@Col]` is only Excel's display shorthand; a file holding it is repaired by Excel,
        // which drops the table.
        assert_eq!(to_file(&Formula::new("Sales[@Units]*Sales[@[Unit Price]]")), "Sales[[#This Row],[Units]]*Sales[[#This Row],[Unit Price]]");
        assert_eq!(text_to_file("=[@Units]&COUNTA(Sales[@])"), "[[#This Row],[Units]]&COUNTA(Sales[#This Row])");
        assert_eq!(text_to_file("=SUM(Sales[Units])+[#Totals]"), "SUM(Sales[Units])+[#Totals]");
        for f in ["Sales[@Units]*[@[Unit Price]]", "Sales[@]", "[@[Unit Price]]+1"] {
            let file = to_file(&Formula::new(f));
            assert!(!file.contains("[@"), "{file}");
            assert_eq!(from_file(&file), f);
        }
    }

    #[test]
    fn struct_ref_column_with_bang() {
        // A `!` inside the brackets is part of the column name, not a sheet separator.
        assert_eq!(to_file(&Formula::new("SUM(Table1[Done!])")), "SUM(Table1[Done!])");
        assert_eq!(text_to_file("=Table1[[#This Row],[a!b]]"), "Table1[[#This Row],[a!b]]");
        // `Table1[#REF!]` was found by the command-sequence fuzzer (seed 40).
        for text in ["SUM(Table1[Done!])", "Table1[#REF!]", "Table1[@[x!]]", "Sheet2!A1+Table1[[#All],[a!b]]"] {
            let f = Formula::new(text);
            assert_eq!(from_file(&to_file(&f)), f.text);
        }
    }

    #[test]
    fn prefixes() {
        assert_eq!(to_file(&Formula::new("XLOOKUP(1,A:A,B:B)+SUM(A1)")), "_xlfn.XLOOKUP(1,A:A,B:B)+SUM(A1)");
        assert_eq!(to_file(&Formula::new("FILTER(A1:A3,B1:B3)")), "_xlfn._xlws.FILTER(A1:A3,B1:B3)");
        assert_eq!(from_file("_xlfn.XLOOKUP(1,A:A,B:B)"), "XLOOKUP(1,A:A,B:B)");
        assert_eq!(text_to_file("=IFS(A1>0,1)"), "_xlfn.IFS(A1>0,1)");
        assert_eq!(to_file(&Formula::new("REGEXTEST(A1,\"x\")&ENCODEURL(B1)")), "_xlfn.REGEXTEST(A1,\"x\")&_xlfn.ENCODEURL(B1)");
        assert_eq!(to_file(&Formula::new("GROUPBY(A1:A3,B1:B3,\"SUM\")")), "_xlfn.GROUPBY(A1:A3,B1:B3,\"SUM\")");
    }

    /// A formula as the reader leaves it, then with Google Sheets' wrappers unwrapped against
    /// the real function library.
    fn read(file: &str) -> String {
        let text = from_file(file);
        unwrap_google_sheets(&text, &test_known_function).unwrap_or(text)
    }

    /// #2871: Google Sheets' export wraps formulas as
    /// `IFERROR(__xludf.DUMMYFUNCTION("<formula>"),<cached value>)`.
    #[test]
    fn google_sheets_dummy_function_wrapper() {
        // The issue's formula, with its embedded newlines and doubled quotes.
        let file = "IFERROR(__xludf.DUMMYFUNCTION(\"MAX(\n IFERROR(MAX(FILTER(ROW(C:C), C:C=\"\"שולם\"\")), 0),\n IFERROR(MAX(FILTER(ROW(D:D), D:D=\"\"שולם\"\")), 0)\n)\"),40)";
        let want = "MAX(IFERROR(MAX(FILTER(ROW(C:C),C:C=\"שולם\")),0),IFERROR(MAX(FILTER(ROW(D:D),D:D=\"שולם\")),0))";
        assert_eq!(read(file), want);
        assert_eq!(read(&format!("={file}")), want);
        // The reader itself leaves the wrapper: unwrapping is a separate pass.
        assert!(from_file(file).contains("DUMMYFUNCTION"));
        // Any case, and IFERROR with its `_xlfn.` prefix; doubled quotes inside.
        assert_eq!(read("_xlfn.IFERROR(__XLUDF.dummyFunction(\"A1&\"\"x\"\"\"),\"1x\")"), "A1&\"x\"");
        assert_eq!(read("iferror(__xludf.DUMMYFUNCTION(\"SORT(A1:A3*2)\"),2)"), "SORT(A1:A3*2)");
        // Nested: an operator expression keeps its precedence; a call or operand needs no parentheses.
        assert_eq!(read("1+IFERROR(__xludf.DUMMYFUNCTION(\"A1+2\"),5)*3"), "1+(A1+2)*3");
        assert_eq!(read("SUM(IFERROR(__xludf.DUMMYFUNCTION(\"FILTER(A1:A3,B1:B3)\"),0),B1)"), "SUM(FILTER(A1:A3,B1:B3),B1)");
        assert_eq!(read("IFERROR(__xludf.DUMMYFUNCTION(\"A1\"),0)&IFERROR(__xludf.DUMMYFUNCTION(\"-B1\"),0)"), "A1&(-B1)");
        // Each wrapper on its own: a Google-only one stays next to one that is unwrapped.
        assert_eq!(
            read("IFERROR(__xludf.DUMMYFUNCTION(\"A1*2\"),2)+IFERROR(__xludf.DUMMYFUNCTION(\"GOOGLETRANSLATE(A1)\"),0)"),
            "(A1*2)+IFERROR(__XLUDF.DUMMYFUNCTION(\"GOOGLETRANSLATE(A1)\"),0)"
        );
    }

    /// Text that doesn't parse, calls a function GridCraft lacks, or shapes that aren't exactly
    /// the wrapper, are kept as they were, so the cell keeps the cached value as in Excel.
    #[test]
    fn google_sheets_dummy_function_kept_when_not_the_wrapper() {
        for file in [
            "IFERROR(__xludf.DUMMYFUNCTION(\"SUM(\"),5)",
            "IFERROR(__xludf.DUMMYFUNCTION(\"1+\"),5)+1",
            // Google-only functions: GridCraft would show #NAME? instead of the cached value.
            "IFERROR(__xludf.DUMMYFUNCTION(\"GOOGLETRANSLATE(A1,\"\"en\"\",\"\"fr\"\")\"),\"x\")",
            "IFERROR(__xludf.DUMMYFUNCTION(\"ARRAYFORMULA(A1:A3*2)\"),2)",
            "IFERROR(__xludf.DUMMYFUNCTION(\"SUM(QUERY(A1:B3,\"\"select B\"\"))\"),2)",
            // A wrapper inside the text: only one level is ever unwrapped.
            "IFERROR(__xludf.DUMMYFUNCTION(\"IFERROR(__xludf.DUMMYFUNCTION(\"\"A1\"\"),0)\"),0)",
            "IFERROR(__xludf.DUMMYFUNCTION(A1),5)",
            "IFERROR(__xludf.DUMMYFUNCTION(\"A1\",\"B1\"),5)",
            "IFERROR(__xludf.DUMMYFUNCTION(\"A1\"))",
            "IFERROR(__xludf.DUMMYFUNCTION(\"A1\"),5,6)",
            "IFNA(__xludf.DUMMYFUNCTION(\"A1\"),5)",
            "IFERROR(_xludf.DUMMYFUNCTION(\"A1\"),5)",
            "__xludf.DUMMYFUNCTION(\"A1\")",
        ] {
            let kept = gridcraft_formula::print(&gridcraft_formula::parse(file).unwrap());
            assert_eq!(read(file), kept, "{file}");
            assert_eq!(unwrap_google_sheets(&from_file(file), &test_known_function), None, "{file}");
        }
        // Inner text over the formula length limit.
        let long = format!("IFERROR(__xludf.DUMMYFUNCTION(\"{}\"),5)", "1".repeat(gridcraft_formula::MAX_FORMULA_LEN + 1));
        assert!(read(&long).contains("DUMMYFUNCTION"));
        // Nesting that would go past the parser's depth limit once unwrapped is kept as well.
        let deep = |n: usize, mid: &str| format!("{}{mid}{}", "(".repeat(n), ")".repeat(n));
        let inner = deep(200, "1").replace('"', "\"\"");
        let file = deep(200, &format!("1+IFERROR(__xludf.DUMMYFUNCTION(\"{inner}\"),5)"));
        assert!(gridcraft_formula::parse(&file).is_ok());
        let read = read(&file);
        assert!(read.contains("DUMMYFUNCTION"));
        assert!(gridcraft_formula::parse(&read).is_ok());
    }

    #[test]
    fn let_lambda_params() {
        assert_eq!(to_file(&Formula::new("LET(x,1,x+1)")), "_xlfn.LET(_xlpm.x,1,_xlpm.x+1)");
        assert_eq!(to_file(&Formula::new("LET(x,A1*2,y,x+1,x*y)")), "_xlfn.LET(_xlpm.x,A1*2,_xlpm.y,_xlpm.x+1,_xlpm.x*_xlpm.y)");
        assert_eq!(to_file(&Formula::new("LAMBDA(a,b,a^2+b)(3,4)")), "_xlfn.LAMBDA(_xlpm.a,_xlpm.b,_xlpm.a^2+_xlpm.b)(3,4)");
        // A name outside the scope stays a plain defined name.
        assert_eq!(to_file(&Formula::new("LET(x,1,x)+x")), "_xlfn.LET(_xlpm.x,1,_xlpm.x)+x");
        assert_eq!(from_file("_xlfn.LET(_xlpm.x,1,_xlpm.x+1)"), "LET(x,1,x+1)");
        assert_eq!(from_file("_xlfn.LAMBDA(_xlpm.a,_xlpm.a*2)(3)"), "LAMBDA(a,a*2)(3)");
        // A LET-bound LAMBDA called by name.
        assert_eq!(to_file(&Formula::new("LET(f,LAMBDA(x,x*2),f(3))")), "_xlfn.LET(_xlpm.f,_xlfn.LAMBDA(_xlpm.x,_xlpm.x*2),_xlpm.F(3))");
        assert_eq!(from_file("_xlfn.LET(_xlpm.f,_xlfn.LAMBDA(_xlpm.x,_xlpm.x*2),_xlpm.f(3))"), "LET(f,LAMBDA(x,x*2),F(3))");
    }
}
