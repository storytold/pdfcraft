use super::*;

#[test]
fn javascript_escapes_are_decoded() {
    assert_eq!(
        parse_format("AFNumber_Format(2, 0, 0, 0, \"\\x24\", true);"),
        Some(Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true })
    );
    assert_eq!(parse_format("AFDate_FormatEx(\"\\b\\f\\v\\0\\q\");"), Some(Format::Date("\u{8}\u{c}\u{b}\0q".into())));
}

#[test]
fn scripts_are_recognised_and_written_back() {
    assert_eq!(
        parse_format("AFNumber_Format(2, 0, 0, 0, \"$\", true);"),
        Some(Format::Number { decimals: 2, sep: 0, neg: 0, currency: "$".into(), prepend: true })
    );
    assert_eq!(parse_format("AFPercent_Format(1, 0);"), Some(Format::Percent { decimals: 1, sep: 0 }));
    assert_eq!(parse_format("AFDate_FormatEx(\"dd-mmm-yyyy\");"), Some(Format::Date("dd-mmm-yyyy".into())));
    assert_eq!(parse_format("AFDate_Format(2);"), Some(Format::Date("mm/dd/yy".into())));
    assert_eq!(parse_format("AFTime_Format(1);"), Some(Format::Time("h:MM tt".into())));
    assert_eq!(parse_format("AFSpecial_Format(2);"), Some(Format::Special(2)));
    assert_eq!(parse_format("AFSpecial_KeystrokeEx(\"AA-9999\");"), Some(Format::Mask("AA-9999".into())));
    assert_eq!(parse_format("app.alert('hi')"), None);
    assert_eq!(parse_validate("AFRange_Validate(true, 0, true, 100);"), Some(Validate::Range { min: Some(0.0), max: Some(100.0) }));
    assert_eq!(parse_validate("AFRange_Validate(false, 0, true, 10);"), Some(Validate::Range { min: None, max: Some(10.0) }));
    assert_eq!(
        parse_calculate("AFSimple_Calculate(\"SUM\", new Array (\"a\", \"b.c\"));"),
        Some(Calculate::Simple { op: CalcOp::Sum, fields: vec!["a".into(), "b.c".into()] })
    );
    assert_eq!(parse_calculate("AFSimple_Calculate('AVG', ['x']);"), Some(Calculate::Simple { op: CalcOp::Average, fields: vec!["x".into()] }));
    assert_eq!(
        parse_calculate("/** BVCALC Price * Qty EVCALC **/ event.value = AFMakeNumber(getField(\"Price\").value) * …"),
        Some(Calculate::Notation("Price * Qty".into()))
    );
    // Round trips through what the tabs write.
    for f in [
        Format::Number { decimals: 2, sep: 2, neg: 2, currency: "€".into(), prepend: false },
        Format::Percent { decimals: 0, sep: 0 },
        Format::Date("mmmm d, yyyy".into()),
        Format::Special(3),
    ] {
        let (fj, _) = format_js(&f).unwrap();
        assert_eq!(parse_format(&fj), Some(f));
    }
    let euro = format_js(&Format::Number { decimals: 2, sep: 2, neg: 0, currency: "€".into(), prepend: true }).unwrap().0;
    assert!(euro.contains("\\u20AC"), "{euro}");
    assert!(!euro.contains('€'), "{euro}");
    let pound = format_js(&Format::Number { decimals: 2, sep: 0, neg: 0, currency: "£".into(), prepend: true }).unwrap().0;
    assert!(pound.contains("\\u00A3"), "{pound}");
    assert!(!pound.contains('£'), "{pound}");
    let c = Calculate::Simple { op: CalcOp::Product, fields: vec!["q".into(), "p".into()] };
    assert_eq!(parse_calculate(&calculate_js(&c).unwrap()), Some(c));
    let c = Calculate::Notation("a + b".into());
    assert_eq!(parse_calculate(&calculate_js(&c).unwrap()), Some(c));
    // The JavaScript after the comment is Acrobat's translation, so viewers that run it
    // instead of reading the comment compute the same value (not 0).
    assert_eq!(
        calculate_js(&Calculate::Notation("(Unit\\ Price * Qty) / 2".into())).unwrap(),
        "/** BVCALC (Unit\\ Price * Qty) / 2 EVCALC **/ event.value = (AFMakeNumber(this.getField(\"Unit Price\").value) * AFMakeNumber(this.getField(\"Qty\").value)) / 2;"
    );
}

#[test]
fn a_rename_rewrites_only_where_a_calculation_reads_the_field() {
    let renamed = |js: &str| rename_in_calculation(js, "a", "qty");
    // The field lists Acrobat and other tools write for AFSimple_Calculate.
    assert_eq!(
        renamed("AFSimple_Calculate(\"SUM\", new Array (\"a\", \"b\"));").as_deref(),
        Some("AFSimple_Calculate(\"SUM\", new Array (\"qty\", \"b\"));")
    );
    assert_eq!(renamed("AFSimple_Calculate('AVG', ['b', 'a']);").as_deref(), Some("AFSimple_Calculate('AVG', ['b', \"qty\"]);"));
    assert_eq!(renamed("AFSimple_Calculate(\"SUM\", \"a, b,a\");").as_deref(), Some("AFSimple_Calculate(\"SUM\", \"qty, b,qty\");"));
    assert_eq!(renamed("AFSimple_Calculate(\"SUM\", new Array(\"ab\", \"b.a\"));"), None, "other names that contain it");
    // Notation: the expression in the comment and Acrobat's translation after it.
    let notation = |expr: &str| calculate_js(&Calculate::Notation(expr.into())).unwrap();
    assert_eq!(rename_in_calculation(&notation("(a + ab) * a / 2"), "a", "Unit Price"), Some(notation("(Unit\\ Price + ab) * Unit\\ Price / 2")));
    assert_eq!(rename_in_calculation(&notation("Unit\\ Price * 2"), "Unit Price", "price"), Some(notation("price * 2")));
    // Scripts: a getField whose argument is one string literal. Comments, strings, template
    // literals, regular expressions and names built at run time are left as written.
    let script = "// getField(\"a\")\nvar s = \"getField('a')\", t = `getField(\"a\")`; /* getField(\"a\") */\n\
                      if (/\"/.test(s)) event.value = this.getField('a').value / 2 * getField(\"a\" + 1).value + getField ( \"ab\" ).value;";
    assert_eq!(renamed(script), Some(script.replacen("this.getField('a')", "this.getField(\"qty\")", 1)));
    assert_eq!(renamed("event.value = getField(\"b\").value;"), None);
    // A new name is escaped as the script needs it.
    assert_eq!(
        rename_in_calculation("event.value = getField(\"a\").value;", "a", "say \"é\"").as_deref(),
        Some("event.value = getField(\"say \\\"\\u00E9\\\"\").value;")
    );
    // A name that would end the notation early is not written in.
    assert_eq!(rename_in_calculation(&notation("a * 2"), "a", "qtyEVCALC"), None);
    // Unfinished scripts end their tokens, not the program.
    for js in ["getField(\"a", "/* getField(\"a\")", "`getField(\"a\")", "x = /[/", "getField('a\\u12"] {
        assert_eq!(renamed(js), None, "{js}");
    }
}

/// The notation is read only from a comment, where Acrobat writes it: a string or code that
/// spells the markers is left alone, by the Calculate tab and by a rename.
#[test]
fn notation_is_read_only_from_a_comment() {
    let string = "var s = \"BVCALC a EVCALC\"; event.value = getField(\"b\").value;";
    assert_eq!(parse_calculate(string), None);
    assert_eq!(rename_in_calculation(string, "a", "qty"), None);
    // A comment after such a string holds the notation.
    let both = "var s = 'BVCALC x EVCALC'; /** BVCALC a + 1 EVCALC **/ event.value = 1;";
    assert_eq!(parse_calculate(both), Some(Calculate::Notation("a + 1".into())));
    assert_eq!(rename_in_calculation(both, "x", "y"), None);
    assert_eq!(rename_in_calculation(both, "a", "qty"), Some(both.replace("a + 1", "qty + 1")));
    // A line comment is a comment too, and an unfinished one still holds it.
    assert_eq!(parse_calculate("// BVCALC a EVCALC\nevent.value = 1;"), Some(Calculate::Notation("a".into())));
    assert_eq!(parse_calculate("/** BVCALC a EVCALC"), Some(Calculate::Notation("a".into())));
}

/// A line comment ends at any of JavaScript's line terminators, not only `\n`, so with `\r` line
/// endings the code after one is still code; and a `/` read as starting a regular expression
/// divides after all when the line ends first.
#[test]
fn a_line_comment_ends_at_any_line_terminator() {
    for end in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
        let string = format!("// header{end}var s = \"BVCALC a EVCALC\"; event.value = getField(\"b\").value;");
        assert_eq!(parse_calculate(&string), None, "{end:?}");
        assert_eq!(rename_in_calculation(&string, "a", "qty"), None, "{end:?}");
        let script = format!("// note{end}getField(\"a\").value;");
        assert_eq!(rename_in_calculation(&script, "a", "qty"), Some(script.replace("\"a\"", "\"qty\"")), "{end:?}");
        let divides = format!("var n = i++ / 2;{end}event.value = getField(\"a\").value / 4;");
        assert_eq!(rename_in_calculation(&divides, "a", "qty"), Some(divides.replace("\"a\"", "\"qty\"")), "{end:?}");
    }
}

/// U+2028 and U+2029 end a word, a number or a regular expression's flags, though as bytes
/// past ASCII they could be part of one, so a `getField` right after one is still a call.
#[test]
fn a_token_ends_at_a_line_separator() {
    for end in ["\u{2028}", "\u{2029}"] {
        let scripts = ["x", "1", "/r/g"].map(|token| format!("var t = {token}{end}getField(\"a\").value;"));
        let renamed = scripts.clone().map(|js| rename_in_calculation(&js, "a", "qty"));
        assert_eq!(renamed, scripts.map(|js| Some(js.replace("\"a\"", "\"qty\""))), "{end:?}");
    }
}

/// A `/` after the condition of `if`, `while`, `for` or `with` starts a regular expression,
/// as after an operator; after a call's parentheses it still divides.
#[test]
fn a_regex_after_a_condition_is_not_read_as_code() {
    let renamed = |js: &str| rename_in_calculation(js, "a", "qty");
    assert_eq!(renamed("if (flag) /getField(\"a\")/.test(x); event.value = 1;"), None);
    // A quote inside it doesn't start a string that hides the rest of the script.
    let quote = "while (i--) /\"/.test(s); event.value = getField(\"a\").value;";
    assert_eq!(renamed(quote), Some(quote.replace("\"a\"", "\"qty\"")));
    // The same after the `while` that ends a `do` loop.
    let after_do = "do i++; while (i < n) /\"/.test(s); event.value = getField(\"a\").value;";
    assert_eq!(renamed(after_do), Some(after_do.replace("\"a\"", "\"qty\"")));
    // After a call, or a method that happens to be called `if`, the `/` divides.
    let divides = "event.value = Math.abs(getField(\"b\").value) / 2 + getField(\"a\").value / 3 + o.if(x) / 4 + getField(\"a\").value / 5;";
    assert_eq!(renamed(divides), Some(divides.replace("\"a\"", "\"qty\"")));
}

/// A `getField` whose argument is more than one string, or an `AFSimple_Calculate` list that
/// isn't written out, reads a field whose name isn't known until the script runs.
#[test]
fn names_built_at_run_time_are_noticed() {
    for js in [
        "event.value = getField(\"row\" + i).value;",
        "event.value = this.getField(name).value;",
        "AFSimple_Calculate(\"SUM\", list);",
        "AFSimple_Calculate(\"SUM\", new Array(\"a\", b));",
        "AFSimple_Calculate(\"SUM\", [\"a\", prefix + \"b\"]);",
        "AFSimple_Calculate(\"SUM\", new Array(\"a\", \"b\" + c));",
        "AFSimple_Calculate(op, new Array(\"a\"));",
        "AFSimple_Calculate(\"SUM\", \"row\" + suffix);",
        "AFSimple_Calculate(\"SUM\", [\"a\"].concat(more));",
        "AFSimple_Calculate(\"SUM\", [\"row\" + i]);",
    ] {
        assert!(reads_built_names(js), "{js}");
    }
    // A string that is only the start of a name or a list isn't read, or renamed, as one.
    for (js, reads) in [
        ("AFSimple_Calculate(\"SUM\", \"row\" + suffix);", vec![]),
        ("AFSimple_Calculate(\"SUM\", [\"row\" + i]);", vec![]),
        ("AFSimple_Calculate(\"SUM\", [\"a\", \"row\" + i]);", vec!["a"]),
        ("AFSimple_Calculate(\"SUM\", [\"a\", \"b\"].concat(more));", vec!["a", "b"]),
    ] {
        assert_eq!(parse_calculate(js), None, "{js} is a script");
        assert_eq!(calculation_reads(js), reads, "{js}");
        assert_eq!(rename_in_calculation(js, "row", "line"), None, "{js}");
    }
    for js in [
        "event.value = getField(\"a\").value + getField('b').value;",
        "AFSimple_Calculate(\"SUM\", new Array(\"a\", \"b\"));",
        "AFSimple_Calculate(\"SUM\", [\"a\", \"b\",]);",
        "AFSimple_Calculate(\"SUM\", \"a, b\");",
        "// getField(\"row\" + i)\nvar s = 'getField(name)'; event.value = 1;",
        "/** BVCALC a + b EVCALC **/ event.value = getField(\"a\").value + getField(\"b\").value;",
    ] {
        assert!(!reads_built_names(js), "{js}");
    }
    // The names it does know are still read and renamed.
    let js = "event.value = getField(\"a\").value + getField(\"row\" + i).value + AFSimple_Calculate(\"SUM\", new Array(\"a\", b));";
    assert_eq!(calculation_reads(js), ["a"]);
    assert_eq!(rename_in_calculation(js, "a", "qty"), Some(js.replace("\"a\"", "\"qty\"")));
    // So are a list's, string or array, with another argument after it.
    for js in ["AFSimple_Calculate(\"SUM\", \"a, b\", x);", "AFSimple_Calculate(\"SUM\", [\"a\", \"b\"], x);"] {
        assert!(reads_built_names(js), "{js}");
        assert_eq!(calculation_reads(js), ["a", "b"], "{js}");
        assert_eq!(rename_in_calculation(js, "a", "qty"), Some(js.replace("\"a", "\"qty")), "{js}");
    }
}

/// Scripts are read in one pass: a line of unfinished regular expressions, or calls nested in
/// one another's field lists, don't make a rename read the rest of the script again for each.
#[test]
fn hostile_scripts_are_read_in_one_pass() {
    let regexes = format!("x = {} getField(\"a\")", "/[".repeat(500_000));
    assert!(rename_in_calculation(&regexes, "a", "b").is_some_and(|js| js.ends_with("getField(\"b\")")));
    let nested = format!("{}\"a\"", "AFSimple_Calculate(\"SUM\", new Array(".repeat(100_000));
    assert_eq!(rename_in_calculation(&nested, "a", "b"), Some(format!("{}\"b\"", "AFSimple_Calculate(\"SUM\", new Array(".repeat(100_000))));
}

/// A notation is stored only when it reads back from the script's comment as itself.
#[test]
fn a_notation_is_stored_only_when_it_reads_back() {
    assert!(notation_storable("Price * Qty"));
    assert!(notation_storable("  a + b  "), "the spaces around it are left out when it is read");
    // A last name that ends in a space keeps it, escaped, when the notation is read back.
    let total = "Qty * Total\\ ";
    assert!(notation_storable(total));
    let read = parse_calculate(&calculate_js(&Calculate::Notation(total.into())).unwrap());
    assert_eq!(read.as_ref().map(Calculate::reads), Some(vec!["Qty".to_owned(), "Total ".to_owned()]));
    for expr in ["", "  ", "a EVCALC b", "a */ x /*", "a */ b", "a\\"] {
        assert!(!notation_storable(expr), "{expr:?}");
    }
}

/// Notation is read through one lexer: field names with escapes, numbers and operators.
#[test]
fn notation_is_read_by_one_lexer() {
    let tokens: Vec<Notation> = notation_tokens("(Unit\\ Price+1.5)*qty_2\\").into_iter().map(|(_, t)| t).collect();
    assert_eq!(
        tokens,
        [
            Notation::Op('('),
            Notation::Name("Unit Price".into()),
            Notation::Op('+'),
            Notation::Number,
            Notation::Op(')'),
            Notation::Op('*'),
            Notation::Name("qty_2\\".into())
        ]
    );
    assert_eq!(notation_js("2 * a\\ b"), "2 * AFMakeNumber(this.getField(\"a b\").value)");
    let values = |name: &str| if name == "Unit Price" { 4.0 } else { 0.5 };
    assert_eq!(eval_notation("(Unit\\ Price + 1.5) * qty", &values), Some(2.75));
    assert_eq!(eval_notation("1.2.3 + a", &values), None, "a number that isn't one");
    assert_eq!(notation_name("2nd + (x)\\"), "\\2nd\\ \\+\\ \\(x\\)\\\\");
}

proptest::proptest! {
    /// Any field name written in notation reads back as that one name.
    #[test]
    fn any_name_written_in_notation_reads_back(name in "\\PC{1,12}") {
        let written = notation_name(&name);
        proptest::prop_assert_eq!(notation_tokens(&written), vec![(0..written.len(), Notation::Name(name))]);
    }

    /// Renaming a field in a formula and back gives the formula as it was.
    #[test]
    fn a_rename_round_trips(old in "\\PC{1,8}", new in "\\PC{1,8}", others in proptest::collection::vec("\\PC{1,8}", 0..4), sum in proptest::bool::ANY) {
        proptest::prop_assume!(old != new && !others.contains(&new) && ![&old, &new].into_iter().chain(&others).any(|n| n.contains("VCALC")));
        let names: Vec<String> = std::iter::once(old.clone()).chain(others).collect();
        let js = if sum {
            calculate_js(&Calculate::Simple { op: CalcOp::Sum, fields: names }).unwrap()
        } else {
            calculate_js(&Calculate::Notation(names.iter().map(|n| notation_name(n)).collect::<Vec<_>>().join(" * "))).unwrap()
        };
        let renamed = rename_in_calculation(&js, &old, &new);
        proptest::prop_assert!(renamed.is_some(), "{js}");
        let back = renamed.as_deref().and_then(|r| rename_in_calculation(r, &new, &old));
        proptest::prop_assert_eq!(back, Some(js));
    }

    /// A notation of any names, written into a script, reads back from it as written.
    #[test]
    fn a_notation_reads_back_as_written(names in proptest::collection::vec("\\PC{1,8}", 1..5)) {
        proptest::prop_assume!(!names.iter().any(|n| n.contains("VCALC")));
        let expr = names.iter().map(|n| notation_name(n)).collect::<Vec<_>>().join(" + ");
        let js = calculate_js(&Calculate::Notation(expr.clone())).unwrap();
        proptest::prop_assert_eq!(parse_calculate(&js), Some(Calculate::Notation(expr.clone())));
        proptest::prop_assert!(notation_storable(&expr), "{expr:?}");
    }

    /// What a formula reads is the same whether it is read from the tab's settings or from
    /// the script they are written as.
    #[test]
    fn a_formula_reads_the_same_fields_as_its_script(names in proptest::collection::vec("\\PC{1,8}", 1..5), sum in proptest::bool::ANY) {
        proptest::prop_assume!(!names.iter().any(|n| n.contains("VCALC")));
        let c = if sum {
            Calculate::Simple { op: CalcOp::Product, fields: names.clone() }
        } else {
            Calculate::Notation(names.iter().map(|n| notation_name(n)).collect::<Vec<_>>().join(" + "))
        };
        let js = calculate_js(&c).unwrap();
        proptest::prop_assert_eq!(calculation_reads(&js), c.reads());
        proptest::prop_assert_eq!(c.reads(), unique(names));
    }
}

#[test]
fn a_calculation_reads_each_named_field_once() {
    assert_eq!(Calculate::Simple { op: CalcOp::Sum, fields: vec!["a".into(), "b".into(), "a".into()] }.reads(), ["a", "b"]);
    assert_eq!(Calculate::Notation("(Unit\\ Price + 2) * qty / qty".into()).reads(), ["Unit Price", "qty"]);
    assert_eq!(Calculate::None.reads(), Vec::<String>::new());
    let script = "var t = getField(\"sub\").value; // getField(\"x\")\nevent.value = t + getField(\"tax\").value + getField(\"row\" + 1).value;";
    assert_eq!(calculation_reads(script), ["sub", "tax"], "names built at run time aren't known");
    assert_eq!(calculation_reads("AFSimple_Calculate(\"SUM\", \"a, b\");"), ["a", "b"]);
}

#[test]
fn numbers_format_like_acrobat() {
    let n = |d, s, ng, c: &str, p| Format::Number { decimals: d, sep: s, neg: ng, currency: c.into(), prepend: p };
    assert_eq!(format_value(&n(2, 0, 0, "$", true), "1234.5"), "$1,234.50");
    assert_eq!(format_value(&n(0, 1, 0, "", true), "1234567"), "1234567");
    assert_eq!(format_value(&n(2, 2, 0, " €", false), "1234.5"), "1.234,50 €");
    assert_eq!(format_value(&n(2, 0, 2, "", true), "-12"), "(12.00)");
    assert_eq!(format_value(&n(1, 0, 0, "", true), "-0.25"), "-0.3");
    assert_eq!(format_value(&Format::Percent { decimals: 1, sep: 0 }, "0.125"), "12.5%");
    assert_eq!(make_number("$1,234.50"), Some(1234.5));
    assert_eq!(make_number("1.234,5"), Some(1234.5));
    assert_eq!(make_number("(7)"), Some(-7.0));
    assert_eq!(make_number("abc"), None);
    assert_eq!(keystroke(&n(2, 0, 0, "$", true), "Total", "$1,000"), Ok("1000".into()));
    assert_eq!(keystroke(&n(2, 0, 0, "$", true), "Total", "ten"), Err("The value entered does not match the format of the field [ Total ]".into()));
    // Blank input clears a formatted field (#1323); without a format it is text like any other.
    assert_eq!(keystroke(&n(2, 0, 0, "$", true), "Total", " \t"), Ok(String::new()));
    assert_eq!(keystroke(&Format::None, "Name", " \t"), Ok(" \t".into()));
}

#[test]
fn dates_times_and_masks() {
    let d = Format::Date("mm/dd/yyyy".into());
    assert_eq!(format_value(&d, "1/2/24"), "01/02/2024");
    assert_eq!(format_value(&Format::Date("mmmm d, yyyy".into()), "10/1/2026"), "October 1, 2026");
    assert_eq!(format_value(&Format::Date("dddd, mmm d, yyyy".into()), "10/1/2026"), "Thursday, Oct 1, 2026");
    assert_eq!(format_value(&Format::Date("dd-mmm-yy".into()), "5-Mar-21"), "05-Mar-21", "month names are read");
    assert!(keystroke(&d, "Due", "2/30/2024").is_err(), "no 30 February");
    assert!(keystroke(&d, "Due", "13/1/2024").is_err());
    // Literal digits in the format (a leave form fixing the year as "dd.mm.26").
    let fixed = Format::Date("dd.mm.26".into());
    assert_eq!(keystroke(&fixed, "Von", "15.05.26"), Ok("15.05.26".into()));
    assert_eq!(format_value(&fixed, "15.05.26"), "15.05.26");
    assert_eq!(format_value(&fixed, "3.1"), "03.01.26", "the literal may be left out");
    assert!(keystroke(&fixed, "Von", "15.05.27").is_err(), "a different number there is not the literal");
    assert!(keystroke(&fixed, "Von", "32.05.26").is_err());
    assert_eq!(format_value(&Format::Time("h:MM tt".into()), "14:05"), "2:05 pm");
    assert_eq!(format_value(&Format::Time("HH:MM".into()), "2:05 pm"), "14:05");
    assert_eq!(format_value(&Format::Special(2), "5551234567"), "(555) 123-4567");
    assert_eq!(format_value(&Format::Special(3), "123-45-6789"), "123-45-6789");
    assert!(keystroke(&Format::Special(0), "Zip", "1234").is_err());
    assert_eq!(format_value(&Format::Mask("AA-9999".into()), "ab1234"), "ab-1234");
    assert!(keystroke(&Format::Mask("AA-9999".into()), "Code", "a11234").is_err());
}

#[test]
fn validation_and_calculation() {
    let r = Validate::Range { min: Some(0.0), max: Some(100.0) };
    assert!(validate(&r, "50").is_ok() && validate(&r, "").is_ok());
    assert_eq!(validate(&r, "120"), Err("Invalid value: must be greater than or equal to 0 and less than or equal to 100.".into()));
    let vals = |name: &str| -> Vec<String> {
        match name {
            "a" => vec!["2".into()],
            "b" => vec!["$3.50".into()],
            "row" => vec!["1".into(), "2".into(), "3".into()],
            "Unit Price" => vec!["4".into()],
            _ => vec![],
        }
    };
    let simple = |op, f: &[&str]| calculate(&Calculate::Simple { op, fields: f.iter().map(|s| s.to_string()).collect() }, &vals);
    assert_eq!(simple(CalcOp::Sum, &["a", "b"]), Some("5.5".into()));
    assert_eq!(simple(CalcOp::Product, &["a", "b"]), Some("7".into()));
    assert_eq!(simple(CalcOp::Average, &["row"]), Some("2".into()), "groups count every child");
    assert_eq!(simple(CalcOp::Maximum, &["a", "row"]), Some("3".into()));
    assert_eq!(simple(CalcOp::Sum, &["missing"]), Some("0".into()));
    assert_eq!(calculate(&Calculate::Notation("(a + b) * 2 - Unit\\ Price / 4".into()), &vals), Some("10".into()));
    assert_eq!(calculate(&Calculate::Notation("a +".into()), &vals), None);
}
