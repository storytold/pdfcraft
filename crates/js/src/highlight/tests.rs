use boa_engine::Source;
use boa_engine::ast::scope::Scope;
use boa_engine::interner::Interner;
use boa_engine::parser::Parser;
use proptest::prelude::*;

use super::*;

/// Whether the engine's parser takes `source`: what the colouring has to agree with.
fn parses(source: &str) -> bool {
    Parser::new(Source::from_bytes(source)).parse_script(&Scope::new_global(), &mut Interner::default()).is_ok()
}

fn kinds(source: &str) -> Vec<(TokenKind, &str)> {
    highlight(source).into_iter().map(|t| (t.kind, source.get(t.bytes).unwrap_or("<bad range>"))).collect()
}

#[test]
fn tokens_are_coloured_by_kind() {
    use TokenKind::*;
    assert_eq!(
        kinds("var n = getField(\"total\").value; // sum\nif (n > 1.5e+3) event.rc = false; /* x */"),
        vec![
            (Keyword, "var"),
            (Api, "getField"),
            (String, "\"total\""),
            (Comment, "// sum"),
            (Keyword, "if"),
            (Number, "1.5e+3"),
            (Api, "event"),
            (Literal, "false"),
            (Comment, "/* x */")
        ]
    );
}

#[test]
fn a_slash_divides_after_a_value_and_starts_a_regex_elsewhere() {
    use TokenKind::*;
    assert_eq!(kinds("a = b / c / d;"), vec![]);
    assert_eq!(kinds("x = (y) / 2;"), vec![(Number, "2")]);
    assert_eq!(kinds("ok = /[/]\\/x/gi.test(s);"), vec![(Regex, "/[/]\\/x/gi")]);
    assert_eq!(kinds("return /a/;"), vec![(Keyword, "return"), (Regex, "/a/")]);
}

#[test]
fn template_code_is_coloured_as_code() {
    use TokenKind::*;
    assert_eq!(
        kinds("`a ${ {k: 'v'}.k } b ${event.value}`"),
        vec![(Template, "`a ${"), (String, "'v'"), (Template, "} b ${"), (Api, "event"), (Template, "}`")]
    );
}

#[test]
fn names_after_a_dot_are_never_keywords_or_api_names() {
    assert_eq!(kinds("x.default = this.event;"), vec![(TokenKind::Keyword, "this")]);
}

#[test]
fn a_number_stops_at_a_property() {
    use TokenKind::*;
    assert_eq!(kinds("1.5.toFixed(2);"), vec![(Number, "1.5"), (Number, "2")]);
    assert_eq!(kinds("1..toString();"), vec![(Number, "1.")]);
    assert_eq!(kinds(".5.toFixed(2);"), vec![(Number, ".5"), (Number, "2")]);
    assert_eq!(kinds("1.5e3 + 1e-3 + 0x1F + 1_000;"), vec![(Number, "1.5e3"), (Number, "1e-3"), (Number, "0x1F"), (Number, "1_000")]);
}

#[test]
fn a_spread_is_followed_by_a_value() {
    use TokenKind::*;
    assert_eq!(kinds("f(...event);"), vec![(Api, "event")]);
    assert_eq!(kinds("f(...this.a);"), vec![(Keyword, "this")]);
    assert_eq!(kinds("x.event;"), vec![], "a single dot still makes a property");
}

#[test]
fn unclosed_strings_and_comments_stop_where_the_parser_would() {
    use TokenKind::*;
    assert_eq!(kinds("s = 'open\nvar t;"), vec![(String, "'open"), (Keyword, "var")]);
    assert_eq!(kinds("/* never closed\nvar t;"), vec![(Comment, "/* never closed\nvar t;")]);
    assert_eq!(kinds("`never closed ${"), vec![(Template, "`never closed ${")]);
}

proptest! {
    /// Any text, however malformed: tokens are in order, do not overlap and sit on character
    /// boundaries.
    #[test]
    fn any_text_is_handled(source in "[a-z0-9 `'\"/*\\\\${}()\\[\\]\n\r.+?:;éø\u{2028}<!>#-]{0,80}") {
        let mut end = 0;
        for t in highlight(&source) {
            prop_assert!(t.bytes.start >= end && t.bytes.start < t.bytes.end && t.bytes.end <= source.len());
            prop_assert!(source.is_char_boundary(t.bytes.start) && source.is_char_boundary(t.bytes.end));
            end = t.bytes.end;
        }
        // And asking where a literal is, anywhere in it or past its end, gives an answer.
        for at in 0..=source.len().saturating_add(1) {
            let _ = in_literal(&source, at);
        }
    }
}

#[test]
fn a_line_separator_inside_a_string_does_not_end_it() {
    // Allowed in string literals since ECMAScript 2019, and the parser accepts it.
    let source = "var s = \"a\u{2028}b\"; var t = 'c';";
    assert!(parses(source));
    let strings: Vec<&str> = highlight(source).iter().filter(|t| t.kind == TokenKind::String).filter_map(|t| source.get(t.bytes.clone())).collect();
    assert_eq!(strings, ["\"a\u{2028}b\"", "'c'"]);
}

#[test]
fn every_name_coloured_as_acrobat_s_is_one_the_engine_provides() {
    for name in API {
        let script = format!("if (typeof {name} === 'undefined') throw new Error('missing');");
        let outcome = crate::run(&script, &crate::Event::doc("Open"), &crate::DocInfo::default(), &[], &[], crate::Limits::default());
        assert_eq!(outcome.error, None, "{name}");
    }
}

#[test]
fn a_slash_after_a_keyword_s_brackets_or_a_block_starts_a_regex() {
    let kinds = |s: &str| highlight(s).into_iter().map(|t| (t.kind, s.get(t.bytes).unwrap_or_default().to_string())).collect::<Vec<_>>();
    assert!(kinds("if (x) /'/.test(s);").contains(&(TokenKind::Regex, "/'/".into())), "after a keyword's brackets");
    assert!(kinds("{}\n/a/.test(s);").contains(&(TokenKind::Regex, "/a/".into())), "after a block");
    assert!(!kinds("x = (a + b) / 2 / c;").iter().any(|(k, _)| *k == TokenKind::Regex), "division after a value's bracket");
}

/// What a guess about a `/` hid, and each script is valid once closed: a `/` after an object's `}` or
/// after `of` or `await` as a name divides, though it colours as a regular expression. Enter and a typed
/// `}` changed the white space these scripts hold.
#[test]
fn a_slash_that_may_divide_hides_no_literal() {
    for (script, closer) in [
        ("const n = { valueOf() { return 6; } } / 2; const s = `a\n    ", "`;"),
        ("var o = {a: 1} / 2; var s = `a\n    ", "`;"),
        ("var f = function () {} / 2; var s = `a\n    ", "`;"),
        ("var o = {} / 2; var s = 'abc\\\n    ", "';"),
        ("var of = 6; var x = of / 2; var s = `a\n    ", "`;"),
        ("var await = 4; var x = await / 2; var s = `a\n    ", "`;"),
        ("var yield = 4; var x = yield / 2; var s = `a\n    ", "`;"),
        // An increment leaves the doubt as it was.
        ("var of = 6; var x = of++ / 2; var s = `a\n    ", "`;"),
        // It hides the `}` that ends a substitution, too: past it is the template's text.
        ("var s = `a${ {} / 2 }c\n    ", "`;"),
        // A real regular expression after a block is still seen for what it is.
        ("function f() {}\n/x/.test(a);\nvar s = `a\n    ", "`;"),
    ] {
        assert!(parses(&format!("{script}{closer}")), "{script}");
        assert!(in_literal(script, script.len()), "{script}");
    }
}

/// Where a `/` can't divide, there's no second reading, and code stays code.
#[test]
fn a_slash_that_can_only_start_a_regex_does_not_make_code_a_literal() {
    for script in [
        "if (a) {\n    ",
        "var o = {a: 1};\n    ",
        "var o = {} / 2;\n    ",
        "x = a / 2;\n    ",
        "x = /`/; y = 1;\n    ",
        "f(/`/);\n    ",
        "if (a) /`/.test(b);\n    ",
        "return /`/;\n    ",
        "x = a ? /`/ : b;\n    ",
        "++/`/.lastIndex;\n    ",
    ] {
        assert!(!in_literal(script, script.len()), "{script}");
    }
}

/// The price of erring towards leaving the text alone: a real regular expression holding a `` ` `` after
/// a block reads, the other way, as a template opening. Enter and `}` then add nothing in what follows.
#[test]
fn a_regex_after_a_block_that_could_open_a_template_counts_as_one() {
    assert!(in_literal("function f() {}\n/`/.test(b);\n    ", 33));
    assert_eq!(kinds("function f() {}\n/`/.test(b);").iter().filter(|(k, _)| *k == TokenKind::Template).count(), 0, "its colours stay right");
}

/// In both readings at once, a template's piece ends where it says, and a position past it isn't in it.
#[test]
fn the_ends_of_a_literal_are_where_they_were() {
    let inside = |marked: &str| {
        let at = marked.find('|').expect("a caret");
        in_literal(&marked.replace('|', ""), at)
    };
    assert!(inside("x = `a|`"));
    assert!(!inside("x = `a`|"));
    assert!(!inside("x = {} / 2; y = `a`|;"));
    assert!(!inside("x = `a${|b}`"));
}

#[test]
fn a_slash_after_an_increment_means_what_it_meant_before_it() {
    use TokenKind::*;
    assert_eq!(kinds("x = i++ / 2 / 3;"), vec![(Number, "2"), (Number, "3")], "after a value: dividing");
    assert_eq!(kinds("x = i-- / 2 / 3;"), vec![(Number, "2"), (Number, "3")]);
    assert_eq!(kinds("x = a[0]++ / 2 / 3;"), vec![(Number, "0"), (Number, "2"), (Number, "3")]);
    assert_eq!(kinds("x = ++/a/.lastIndex;"), vec![(Regex, "/a/")], "before a value: a regular expression");
    assert_eq!(kinds("x = a+++/b/.lastIndex;"), vec![(Regex, "/b/")], "`a++ + /b/`");
    assert_eq!(kinds("x = 1 + +2 / 3;"), vec![(Number, "1"), (Number, "2"), (Number, "3")], "two plus signs, not an increment");
}

/// What the engine reads as comments (ECMAScript Annex B.1.1, and the hashbang grammar) is
/// coloured as comments, so what they hold opens no string or template.
#[test]
fn html_like_comments_and_a_hashbang_are_comments() {
    use TokenKind::*;
    for (source, comment) in [
        ("var a = 1;\n<!-- don't `\nvar b;", "<!-- don't `"),
        ("x<!-- don't\nvar b;", "<!-- don't"),
        ("var a = 1;\n--> don't `\nvar b;", "--> don't `"),
        ("var a = 1\n--> don't\nvar b;", "--> don't"),
        ("\n--> don't\nvar b;", "--> don't"),
        ("var a = 1;\n  \t--> don't\nvar b;", "--> don't"),
        ("var a = 1;\n/* c */ --> don't\nvar b;", "--> don't"),
        ("var a = 1; /* c\n d */ --> don't\nvar b;", "--> don't"),
        ("<!-- one\n--> don't\nvar b;", "--> don't"),
        ("#!/bin/sh don't `\nvar b;", "#!/bin/sh don't `"),
    ] {
        assert!(parses(source), "the engine reads it as a comment: {source:?}");
        let found = kinds(source);
        assert!(found.contains(&(Comment, comment)), "{source:?}: {found:?}");
        assert!(found.contains(&(Keyword, "var")), "the code after it is still code: {source:?}: {found:?}");
        assert!(!found.iter().any(|(k, _)| matches!(k, String | Template)), "{source:?}: {found:?}");
    }
}

#[test]
fn what_only_looks_like_an_html_comment_or_a_hashbang_is_not_one() {
    use TokenKind::*;
    // Decrementing and comparing: a `-->` with a value before it on its line.
    for source in ["x-->0;", "var a;\nx-->0;", "var a;\nx /* c */ --> 0;"] {
        assert!(parses(source), "{source:?}");
        assert!(kinds(source).contains(&(Number, "0")), "{source:?}");
    }
    assert_eq!(kinds("x /* c */ --> 0;"), vec![(Comment, "/* c */"), (Number, "0")]);
    // On the script's first line the parser reads it as `--` and `>`, as it does right after a
    // hashbang, which takes its line end with it.
    for source in ["--> c", "  --> c", "#!x\n--> c"] {
        assert!(!parses(source), "{source:?}");
        assert!(!kinds(source).iter().any(|(_, text)| text.contains("-->")), "{source:?}");
    }
    // In a string, and a hashbang anywhere but the very start.
    assert_eq!(kinds("s = '<!-- x';"), vec![(String, "'<!-- x'")]);
    assert!(!kinds(" #!x").iter().any(|(k, _)| *k == Comment));
    assert!(!kinds("\n#!x").iter().any(|(k, _)| *k == Comment));
}
