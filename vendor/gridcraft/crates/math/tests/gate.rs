//! The RaTeX gate, kept as a regression suite so that a version bump re-runs it: hostile and
//! random input never panics, nesting is bounded below the stack, macro bombs stop, and the
//! syntax an equation editor needs is covered. Every call goes through `catch_unwind`, so a panic
//! is counted, not fatal. `docs/decisions.md` holds the full gate record.

use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Debug, PartialEq)]
enum Outcome {
    Laid(usize),
    Rejected(String),
    Panicked(String),
}

fn run(src: &str) -> Outcome {
    let r = catch_unwind(AssertUnwindSafe(|| match ratex_parser::parse(src) {
        Ok(nodes) => {
            let lb = ratex_layout::layout(&nodes, &ratex_layout::LayoutOptions::default());
            Ok(ratex_layout::to_display_list(&lb).items.len())
        }
        Err(e) => Err(e.message),
    }));
    match r {
        Ok(Ok(n)) => Outcome::Laid(n),
        Ok(Err(m)) => Outcome::Rejected(m),
        Err(p) => {
            Outcome::Panicked(p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default())
        }
    }
}

/// xorshift64: deterministic, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[self.below(v.len())]
    }
}

const TOKENS: &[&str] = &[
    "{",
    "}",
    "[",
    "]",
    "(",
    ")",
    "^",
    "_",
    "&",
    "\\\\",
    "x",
    "1",
    "+",
    "=",
    "\\frac",
    "\\sqrt",
    "\\left",
    "\\right",
    "\\middle",
    "\\sum",
    "\\int",
    "\\begin{matrix}",
    "\\end{matrix}",
    "\\begin{pmatrix}",
    "\\end{pmatrix}",
    "\\begin{cases}",
    "\\end{cases}",
    "\\begin{aligned}",
    "\\end{aligned}",
    "\\begin{array}{cc}",
    "\\end{array}",
    "\\text{",
    "\\mathbb",
    "\\hat",
    "\\overline",
    "\\underbrace",
    "\\overbrace",
    "\\binom",
    "\\color{red}",
    "\\def\\a{\\a}",
    "\\a",
    "\\newcommand",
    "\\boxed",
    "\\xrightarrow",
    "\\stackrel",
    "\\over",
    "\\atop",
    "\\choose",
    "\\displaystyle",
    "\\big",
    "\\Bigg",
    "|",
    "\\|",
    "\\langle",
    ".",
    "\\ce{",
    "\\pu{",
    "H2O",
    "->",
    "\\operatorname",
    "\\mathrm",
    "\\verb|x|",
    "\\char",
    "\\kern",
    "\\hspace{",
    "1em",
    "-1000em",
    "\\rule{",
    "\\raisebox{",
    "\\phantom",
    "\\cancel",
    "\\not",
    "\\limits",
    "\\tag{",
    "$",
    "%",
    "#",
    "~",
    "'",
    "\\vec",
    "\\mathchoice",
    "\\href{",
    "\\includegraphics",
    "\\cr",
    "\\hline",
    "\\multicolumn",
    "\\begin{CD}",
    "\\end{CD}",
    "@>>>",
    "\\infty",
    "\\alpha",
    "\\partial",
    "\u{1F600}",
    "\u{0301}",
    "\u{0000}",
    "\u{FFFF}",
    "\\sideset",
    "\\genfrac",
    "\\substack{",
    "\\smash",
    "\\llap",
    "\\textcolor{",
    "#ff0000",
    "\\colorbox{",
    "\\sqrt[",
    "\\lim",
    "\\overset",
    "\\underset",
    "\\Huge",
    "\\begin{align}",
    "\\end{align}",
    "\\begin{alignat}{2}",
    "\\end{alignat}",
    "\\begin{rcases}",
    "\\expandafter",
    "\\noexpand",
    "\\let",
    "\\gdef",
    "\\global",
    "\\edef",
    "\\boldsymbol",
    "\\mathcal",
    "\\mathfrak",
    "\\char\"41",
    "\\unicode",
    "\\infer",
];

/// What the editor's gallery and quick-insert buttons produce, plus everyday notation.
const COVERAGE: &[&str] = &[
    "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}",
    "a^2 + b^2 = c^2",
    "(x + a)^n = \\sum_{k=0}^{n} \\binom{n}{k} x^k a^{n-k}",
    "A = \\pi r^2",
    "e^{i\\pi} + 1 = 0",
    "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
    "\\begin{bmatrix}1&0\\\\0&1\\end{bmatrix}",
    "\\begin{vmatrix}a&b\\\\c&d\\end{vmatrix}",
    "\\int_0^\\infty e^{-x^2}\\,dx",
    "\\iint \\oint \\prod_{i} \\coprod",
    "\\lim_{n\\to\\infty}\\left(1+\\frac1n\\right)^n",
    "f(x) = \\begin{cases} x & x \\ge 0 \\\\ -x & x < 0 \\end{cases}",
    "\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}",
    "\\overbrace{a+b}^{n} \\underbrace{c}_{m}",
    "\\vec v \\cdot \\hat n \\times \\overline{z} \\tilde a \\dot x \\ddot y \\bar b",
    "\\mathbb{R}^n \\mathcal{L} \\mathfrak g \\mathrm{d} \\mathbf{v} \\mathit{x} \\mathsf{s} \\mathtt{t}",
    "\\sqrt[3]{x}",
    "\\left\\langle \\psi \\middle| \\phi \\right\\rangle",
    "\\left\\{ x \\right\\} \\left[ y \\right] \\left| z \\right| \\left\\lfloor w \\right\\rceil",
    "\\color{red}{x}\\textcolor{blue}{y}",
    "\\text{if } x \\in A",
    "\\xrightarrow[below]{above} \\xleftarrow{a}",
    "\\begin{array}{c|c} 1 & 2 \\\\ \\hline 3 & 4 \\end{array}",
    "\\alpha\\beta\\gamma\\delta\\epsilon\\varepsilon\\zeta\\eta\\theta\\vartheta\\iota\\kappa\\lambda\\mu\\nu\\xi\\pi\\rho\\sigma\\tau\\upsilon\\phi\\varphi\\chi\\psi\\omega",
    "\\Gamma\\Delta\\Theta\\Lambda\\Xi\\Pi\\Sigma\\Upsilon\\Phi\\Psi\\Omega",
    "\\le\\ge\\ne\\approx\\equiv\\sim\\simeq\\cong\\propto\\ll\\gg\\subset\\supseteq\\in\\notin\\ni",
    "\\pm\\mp\\times\\div\\cdot\\circ\\bullet\\oplus\\otimes\\cup\\cap\\wedge\\vee\\setminus",
    "\\infty\\partial\\nabla\\forall\\exists\\emptyset\\angle\\triangle\\hbar\\ell\\Re\\Im\\aleph",
    "\\to\\leftarrow\\Rightarrow\\Leftrightarrow\\mapsto\\uparrow\\iff\\implies",
    "\\sin\\cos\\tan\\log\\ln\\exp\\max\\min\\det\\gcd\\operatorname{sgn}",
    "\\displaystyle\\sum_{i=1}^n i = \\frac{n(n+1)}{2}",
    "\\binom{n}{k} \\dbinom{a}{b} \\tfrac12 \\dfrac{1}{2} \\cfrac{1}{1+\\cfrac{1}{2}}",
    "x_{i,j}^{(k)} x' x''",
    "\\boxed{E=mc^2}",
    "\\cancel{x} \\bcancel{y}",
    "\\overset{!}{=} \\underset{x}{\\arg\\max} \\stackrel{def}{=}",
    "\\begin{matrix}1&2\\\\3&4\\end{matrix}",
    "\\begin{smallmatrix}1&2\\end{smallmatrix}",
    "\\begin{gather} a \\\\ b \\end{gather}",
    "\\begin{align} a &= b \\end{align}",
    "\\ldots\\cdots\\vdots\\ddots",
    "a\\quad b\\qquad c\\,d\\;e\\!f",
    "\\langle x \\rangle \\lVert v \\rVert",
    "\\frac{\\partial f}{\\partial x}",
    "\\int\\limits_a^b",
    "a\\phantom{x}b\\hphantom{y}c",
    "\\widehat{abc} \\widetilde{xyz} \\overrightarrow{AB}",
    "a \\pmod{n} \\bmod b",
    "\\Big( \\bigg[ \\Bigg\\{",
    "\\not= \\not\\in",
    "1{,}000 \\%\\$\\#\\&\\_\\{\\}",
    "\\textbf{bold} \\textit{it}",
    "\\sum_{\\substack{a\\\\b}}",
];

#[test]
fn random_token_soup_never_panics() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut panics = Vec::new();
    for _ in 0..3000 {
        let mut s = String::new();
        for _ in 0..1 + rng.below(24) {
            if rng.below(10) == 0 {
                s.push(char::from_u32(rng.below(0x3000) as u32).unwrap_or('?'));
            } else {
                s.push_str(rng.pick(TOKENS));
            }
        }
        if let Outcome::Panicked(p) = run(&s) {
            panics.push((s, p));
        }
    }
    assert!(panics.is_empty(), "RaTeX panicked: {panics:?}");
}

#[test]
fn mutated_valid_input_never_panics() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut panics = Vec::new();
    for _ in 0..3000 {
        let mut v: Vec<char> = rng.pick(COVERAGE).chars().collect();
        for _ in 0..1 + rng.below(4) {
            if v.is_empty() {
                break;
            }
            let at = rng.below(v.len() + 1);
            match rng.below(4) {
                0 if at < v.len() => {
                    v.remove(at);
                }
                1 => {
                    for c in rng.pick(TOKENS).chars().rev() {
                        v.insert(at.min(v.len()), c);
                    }
                }
                2 if at < v.len() => v[at] = char::from_u32(rng.below(0x2200) as u32).unwrap_or('?'),
                _ => {
                    let b = rng.below(v.len());
                    let e = (b + rng.below(12)).min(v.len());
                    let seg: Vec<char> = v[b..e].to_vec();
                    for c in seg.into_iter().rev() {
                        v.insert(at.min(v.len()), c);
                    }
                }
            }
        }
        let s: String = v.into_iter().collect();
        if let Outcome::Panicked(p) = run(&s) {
            panics.push((s, p));
        }
    }
    assert!(panics.is_empty(), "RaTeX panicked: {panics:?}");
}

#[test]
fn nesting_is_refused_before_the_stack_runs_out() {
    for (open, close) in [("{", "}"), ("\\frac{", "}{1}"), ("\\sqrt{", "}"), ("\\left(", "\\right)"), ("x^{", "}")] {
        let at = |d: usize| format!("{}x{}", open.repeat(d), close.repeat(d));
        assert!(matches!(run(&at(32)), Outcome::Laid(_)), "{open}: 32 levels lay out");
        for d in [64, 2000] {
            assert_eq!(run(&at(d)), Outcome::Rejected("Recursion limit exceeded".into()), "{open}: {d} levels");
        }
    }
}

#[test]
fn nesting_built_by_macros_is_refused_by_the_parser() {
    // `check_caps` in the wrapper counts the braces, `\left`s and `\begin`s of the source text,
    // so nesting that macro expansion builds reaches the parser; its own recursion guard refuses
    // it, well within the 1,000-expansion limit (64 levels take 128 expansions).
    let built = |d: usize| format!("\\def\\o{{\\left(}}\\def\\c{{\\right)}}{}x{}", "\\o ".repeat(d), "\\c".repeat(d));
    assert!(matches!(run(&built(32)), Outcome::Laid(_)), "32 macro-built levels lay out");
    assert_eq!(run(&built(64)), Outcome::Rejected("Recursion limit exceeded".into()), "64 macro-built levels");
    let frac = |d: usize| format!("\\def\\o#1{{\\frac{{#1}}{{1}}}}{}x{}", "\\o{".repeat(d), "}".repeat(d));
    assert!(matches!(run(&frac(32)), Outcome::Laid(_)), "32 macro-built fractions lay out");
    assert_eq!(run(&frac(64)), Outcome::Rejected("Recursion limit exceeded".into()), "64 macro-built fractions");
    // The wrapper's text count sees no nesting in the first source, so the parser's guard is the
    // one that holds there too.
    assert!(gridcraft_math::layout(&built(64)).is_err());
}

#[test]
fn macro_bombs_and_huge_dimensions_stop() {
    for bomb in [
        "\\def\\a{\\a\\a}\\a",
        "\\def\\a{xxxxxxxxxx}\\def\\b{\\a\\a\\a\\a\\a\\a\\a\\a\\a\\a}\\def\\c{\\b\\b\\b\\b\\b\\b\\b\\b\\b\\b}\\def\\d{\\c\\c\\c\\c\\c\\c\\c\\c\\c\\c}\\d",
    ] {
        assert!(matches!(run(bomb), Outcome::Rejected(m) if m.contains("Too many expansions")));
    }
    // Absurd sizes lay out (the wrapper in this crate caps them); NaN is refused.
    for s in ["\\kern99999999em x", "\\rule{1e308em}{1e308em}", "\\hspace{-99999em}x"] {
        assert!(matches!(run(s), Outcome::Laid(_)), "{s}");
    }
    assert!(matches!(run("\\rule{nanem}{1em}"), Outcome::Rejected(_)));
    assert!(matches!(run(&"x+".repeat(2048)), Outcome::Laid(4096)));
}

#[test]
fn editor_syntax_is_covered() {
    let missing: Vec<(&str, Outcome)> = COVERAGE.iter().map(|s| (*s, run(s))).filter(|(_, o)| !matches!(o, Outcome::Laid(n) if *n > 0)).collect();
    assert!(missing.is_empty(), "unsupported: {missing:?}");
}

/// Whether every number in `list` is finite and within the caps the wrapper promises.
fn within_caps(list: &gridcraft_math::MathList) -> bool {
    use gridcraft_math::{Item, MAX_EM, MAX_ITEMS, PathCmd};
    let ok = |v: f32| v.is_finite() && v.abs() <= MAX_EM;
    let path_ok = |c: &PathCmd| match *c {
        PathCmd::MoveTo(a, b) | PathCmd::LineTo(a, b) => ok(a) && ok(b),
        PathCmd::QuadTo(a, b, c, d) => [a, b, c, d].into_iter().all(ok),
        PathCmd::CubicTo(a, b, c, d, e, f) => [a, b, c, d, e, f].into_iter().all(ok),
        PathCmd::Close => true,
    };
    list.items.len() <= MAX_ITEMS
        && [list.width, list.height, list.depth].into_iter().all(ok)
        && list.items.iter().all(|i| match i {
            Item::Glyph(g) => [g.x, g.y, g.size, g.width, g.height, g.depth].into_iter().all(ok),
            Item::Rule(r) => [r.x, r.y, r.width, r.height].into_iter().all(ok),
            Item::Path(p) => ok(p.x) && ok(p.y) && p.commands.iter().all(path_ok),
        })
}

#[test]
fn the_guarded_entry_points_survive_hostile_input() {
    // What every caller (commands, the editor's preview, the xlsx writer) reaches: `layout`,
    // `check` and `to_omml`, with their caps. Random token soup, mutations of valid input, and
    // inputs at and past the caps; none may panic, a layout stays within its caps, and an error's
    // span stays on char boundaries inside the source.
    let mut rng = Rng(0x6a09_e667_f3bc_c909);
    let mut inputs: Vec<String> = Vec::new();
    for _ in 0..1500 {
        let mut s = String::new();
        for _ in 0..1 + rng.below(32) {
            match rng.below(12) {
                0 => s.push(char::from_u32(rng.below(0x11_0000) as u32).unwrap_or('\u{FFFD}')),
                1 => s.push_str(rng.pick(COVERAGE)),
                _ => s.push_str(rng.pick(TOKENS)),
            }
        }
        inputs.push(s);
    }
    for (open, close) in [("{", "}"), ("\\left(", "\\right)"), ("\\begin{matrix}", "\\end{matrix}"), ("\\sqrt{", "}")] {
        for d in [31, 32, 33, 500] {
            inputs.push(format!("{}x{}", open.repeat(d), close.repeat(d)));
        }
    }
    let cap = gridcraft_math::MAX_SOURCE_BYTES;
    inputs.push("x".repeat(cap));
    inputs.push("x".repeat(cap + 1));
    inputs.push(format!("{}\u{e9}", "x".repeat(cap - 1)));
    inputs.push("\\rule{1e308em}{1e308em}".into());
    inputs.push("\\def\\a{\\a\\a}\\a".into());
    let mut failures = Vec::new();
    for s in &inputs {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let laid = gridcraft_math::layout(s);
            let checked = gridcraft_math::check(s);
            let omml = gridcraft_math::to_omml(s, 11.0, None);
            (laid, checked, omml)
        }));
        let Ok((laid, checked, omml)) = r else {
            failures.push(format!("panicked: {s:?}"));
            continue;
        };
        if let Ok(list) = &laid
            && !within_caps(list)
        {
            failures.push(format!("outside the caps: {s:?}"));
        }
        for e in [laid.as_ref().err(), checked.as_ref().err(), omml.as_ref().err()].into_iter().flatten() {
            if let Some(span) = &e.span
                && !(span.start <= span.end && span.end <= s.len() && s.is_char_boundary(span.start) && s.is_char_boundary(span.end))
            {
                failures.push(format!("span {span:?} off the source: {s:?}"));
            }
        }
        if laid.is_ok() && checked.is_err() {
            failures.push(format!("lays out but does not check: {s:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
#[allow(clippy::disallowed_methods)] // tests run only off wasm, where the clock exists
fn pathological_source_at_the_cap_finishes_in_bounded_time() {
    // Sources of exactly `MAX_SOURCE_BYTES` built to make a parser or a regular expression work
    // hardest: groups and delimiters nested to the depth cap, long runs of backslashes, braces and
    // scripts, a `(a+)+b` shape, wide and tall environments, and chemistry. `\ce` and `\pu` are
    // the one place RaTeX runs regular expressions over the source (its mhchem port uses
    // `fancy-regex`, which stops a match after a million backtracking steps); the rest of the
    // parser is hand written. Each source lays out, checks and converts, or is refused, well
    // inside the bound (the slowest, `\ce{+-+-...}`, takes 0.8 s in a debug build on an Apple M
    // series machine; catastrophic backtracking would take minutes), so a regression in RaTeX or
    // its dependencies shows up here without making a slow CI runner fail.
    use gridcraft_math::{MAX_DEPTH, MAX_SOURCE_BYTES, check, layout, to_omml};
    use std::time::{Duration, Instant};
    let fill = |head: &str, body: &str, tail: &str| {
        let room = MAX_SOURCE_BYTES.saturating_sub(head.len() + tail.len());
        let mut s = String::from(head);
        while s.len() + body.len() <= head.len() + room {
            s.push_str(body);
        }
        while s.len() < MAX_SOURCE_BYTES - tail.len() {
            s.push(' ');
        }
        s.push_str(tail);
        assert_eq!(s.len(), MAX_SOURCE_BYTES);
        s
    };
    let deep = |open: &str, close: &str| fill(&open.repeat(MAX_DEPTH), "x", &close.repeat(MAX_DEPTH));
    let sources = [
        deep("{", "}"),
        deep("\\left(", "\\right)"),
        deep("\\frac{", "}{1}"),
        deep("\\sqrt{", "}"),
        deep("x^{", "}"),
        deep("\\hat{", "}"),
        deep("\\text{", "}"),
        deep("\\begin{matrix}", "\\end{matrix}"),
        fill("", "\\\\", ""),
        fill("", "\\{", ""),
        fill("", "{}", ""),
        fill("", "(a+)+", "b"),
        fill("", "x^", "x"),
        fill("", "'", ""),
        fill("", "\\,", ""),
        fill("", "&", ""),
        fill("\\begin{matrix}", "&", "\\end{matrix}"),
        fill("\\begin{aligned}", "\\\\", "\\end{aligned}"),
        fill("\\begin{cases}", "x&\\\\", "\\end{cases}"),
        fill("", "\\def\\a{x}", ""),
        fill("", "\\color{red}", ""),
        fill("", "\u{0301}", ""),
        fill("", "\\mathbb{R}", ""),
        fill("", "\\operatorname{f}", ""),
        fill("\\ce{", "1", "}"),
        fill("\\ce{", "0.", "}"),
        fill("\\ce{", "(a", "}"),
        fill("\\ce{", "->", "}"),
        fill("\\ce{", "^_", "}"),
        fill("\\ce{", "$", "}"),
        fill("\\ce{", "H2O ", "}"),
        fill("\\ce{", "\\ce{", "}"),
        fill("\\ce{", "+-", "}"),
        fill("\\ce{", "1/2", "}"),
        fill("\\pu{", "+-", "}"),
        fill("\\pu{", "1e", "}"),
        fill("\\pu{", "10^", "}"),
        fill("\\pu{", "kJ//", "}"),
    ];
    // A CI regression threshold with headroom for shared-runner variance, not a runtime deadline.
    let bound = Duration::from_secs(15);
    let mut slow = Vec::new();
    for s in &sources {
        let start = Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| (layout(s).is_ok(), check(s).is_ok(), to_omml(s, 11.0, None).is_ok())));
        let took = start.elapsed();
        let head: String = s.chars().take(24).collect();
        match r {
            Ok(_) if took <= bound => {}
            Ok(_) => slow.push(format!("{head:?}: {took:?}")),
            Err(_) => slow.push(format!("{head:?}: panicked")),
        }
    }
    assert!(slow.is_empty(), "over {bound:?}: {slow:#?}");
}
