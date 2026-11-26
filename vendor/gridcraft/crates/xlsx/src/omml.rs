//! Equations in drawings: Office Math Markup (ECMA-376 Part 1 §22.1) read back into LaTeX for the
//! common subset, the inverse of `gridcraft_math::to_omml`. A construct outside the subset makes
//! the whole equation unreadable as maths (`None`), and the caller keeps it as a text box.

use crate::xml::El;

/// Deepest OMML nesting marked.
const MAX_DEPTH: usize = 64;

/// The LaTeX for an `m:oMath` (or `m:oMathPara`) element, if every part is in the subset and the
/// result lays out.
pub fn to_latex(e: &El) -> Option<String> {
    if e.name != "oMathPara" && e.name != "oMath" {
        return None;
    }
    validate_conversion(e, 0, false, &mut (1 << 20))?;
    let mut xml = String::new();
    write_el(e, &mut xml);
    let math = craft_math::omml::read_str(&xml)?;
    let out = craft_math::math_latex::to_latex(&math.nodes);
    let out = out.trim().to_string();
    (!out.is_empty() && gridcraft_math::check(&out).is_ok()).then_some(out)
}

// The shared reader is tolerant; this importer must fall back instead of losing
// equation content. Bound escaped XML before serializing or descending further.
fn validate_conversion(e: &El, depth: usize, property: bool, remaining: &mut usize) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    let charge = e.name.len().checked_mul(2)?.checked_add(16)?.checked_add(e.text.len().checked_mul(6)?)?;
    *remaining = remaining.checked_sub(charge)?;
    for (key, value) in &e.attrs {
        *remaining = remaining.checked_sub(key.len().checked_add(value.len().checked_mul(6)?)?.checked_add(8)?)?;
    }
    let property = property || e.name.ends_with("Pr");
    if !property
        && !matches!(
            e.name.as_str(),
            "oMath"
                | "oMathPara"
                | "r"
                | "t"
                | "f"
                | "num"
                | "den"
                | "rad"
                | "deg"
                | "e"
                | "sSup"
                | "sSub"
                | "sSubSup"
                | "sPre"
                | "sub"
                | "sup"
                | "nary"
                | "d"
                | "m"
                | "mr"
                | "eqArr"
                | "acc"
                | "bar"
                | "groupChr"
                | "limLow"
                | "limUpp"
                | "lim"
                | "func"
                | "fName"
                | "borderBox"
                | "phant"
                | "box"
        )
    {
        return None;
    }
    // These limits truncate in the shared reader rather than refusing the matrix.
    if (e.name == "m" && e.kids("mr").count() > 1000) || (e.name == "mr" && e.kids("e").count() > 64) {
        return None;
    }
    // The pinned emitter treats a literal caret as syntax. Preserve original text
    // rather than inventing a superscript; structural scripts remain supported.
    if e.name == "t" && e.text.contains('^') {
        return None;
    }
    for child in &e.children {
        validate_conversion(child, depth + 1, property, remaining)?;
    }
    Some(())
}

fn write_el(e: &El, out: &mut String) {
    out.push_str("<m:");
    out.push_str(&e.name);
    for (k, v) in &e.attrs {
        out.push(' ');
        if k == "val" {
            out.push_str("m:val");
        } else {
            out.push_str(k);
        }
        out.push_str("=\"");
        push_xml_escaped(v, out);
        out.push('"');
    }
    if e.children.is_empty() && e.text.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if !e.text.is_empty() {
        push_xml_escaped(&e.text, out);
    }
    for c in &e.children {
        write_el(c, out);
    }
    out.push_str("</m:");
    out.push_str(&e.name);
    out.push('>');
}

fn push_xml_escaped(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

/// The first `m:oMath`/`m:oMathPara` under `e` (an `a14:m` element or a paragraph), searched to a
/// bounded depth.
pub fn find_math(e: &El, depth: usize) -> Option<&El> {
    if depth > 8 {
        return None;
    }
    if e.name == "oMathPara" || e.name == "oMath" {
        return Some(e);
    }
    e.children.iter().find_map(|c| find_math(c, depth + 1))
}

/// The LaTeX of a DrawingML text body (`a:txBody`) that holds one equation and nothing else: no
/// text in its runs or fields, and no second `m:oMath`/`m:oMathPara`. A body that mixes text and
/// maths, or holds several equations, is `None`, so the caller keeps it as a text box (or reads
/// the writer's fallback) instead of dropping the rest.
pub fn equation_source(tb: &El) -> Option<String> {
    let has_text = tb
        .kids("p")
        .any(|p| p.children.iter().any(|c| matches!(c.name.as_str(), "r" | "fld") && c.child("t").is_some_and(|t| !t.text.trim().is_empty())));
    if has_text || count_math(tb, 0) != 1 {
        return None;
    }
    find_math(tb, 0).and_then(to_latex)
}

/// How many `m:oMath`/`m:oMathPara` elements are under `e` (not counting those inside one), to
/// the same bounded depth as [`find_math`], stopping at two.
fn count_math(e: &El, depth: usize) -> usize {
    if depth > 8 {
        return 0;
    }
    if e.name == "oMathPara" || e.name == "oMath" {
        return 1;
    }
    let mut n = 0;
    for c in &e.children {
        n += count_math(c, depth + 1);
        if n > 1 {
            break;
        }
    }
    n
}

/// A marker for the equation a text body holds: 16 hex digits of an FNV-1a 64-bit hash over what
/// carries the equation's meaning, the OMML element names in document order, the text of the
/// runs, and the properties the converter reads ([`PROPERTIES`]: a run's `sty`, a delimiter's
/// characters, an operator's limits and so on). The writer puts it in the shape's description
/// beside the LaTeX source; the reader takes the source only while the marker still matches the
/// body, so an equation whose Office Math another application edited (Excel leaves the
/// description alone) is converted from that Office Math instead of coming back as the old
/// source. What a re-serialisation may change without changing the equation is left out: every
/// other property (`ctrlPr`, DrawingML run properties such as size and colour), extension lists,
/// the DrawingML containers (`txBody`, `p`, `r`, `fld`, `lstStyle`), attributes, namespace
/// prefixes and whitespace between elements; adjacent runs without read properties are joined,
/// so splitting or merging them changes nothing, while a run with one is a unit of its own, as
/// the converter converts it (it recognises function names per run). A text change, an added
/// run, a structural change, a change to a property the converter reads or a styled run split in
/// two does.
pub fn body_marker(tb: &El) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut text = String::new();
    mark(tb, &mut h, &mut text, 0);
    mark_text(&mut h, &mut text);
    format!("{h:016x}")
}

fn mark(e: &El, h: &mut u64, text: &mut String, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for c in &e.children {
        if c.name.ends_with("Pr") {
            // Only the properties the converter reads, each as `name=val`, in a fixed order; they
            // close the run text gathered so far, so a property moving between runs is a change.
            let read: Vec<(&str, &str)> =
                read_properties(&c.name).iter().filter_map(|n| c.child(n).map(|p| (*n, p.attr("val").unwrap_or("")))).collect();
            if !read.is_empty() {
                mark_text(h, text);
                mark_bytes(h, b"[");
                mark_bytes(h, c.name.as_bytes());
                for (n, v) in read {
                    mark_bytes(h, b" ");
                    mark_bytes(h, n.as_bytes());
                    mark_bytes(h, b"=");
                    mark_bytes(h, v.as_bytes());
                }
                mark_bytes(h, b"]");
            }
            continue;
        }
        if c.name == "extLst" {
            continue;
        }
        match c.name.as_str() {
            "r" => {
                // A run with a property the converter reads is converted on its own (a function
                // name such as `sin` is recognised per run), so its text is hashed as a unit: the
                // property closes the text before it, and the run's end closes its own. Runs
                // without such properties convert character by character and stay joined.
                let styled = c.children.iter().any(|p| p.name.ends_with("Pr") && read_properties(&p.name).iter().any(|n| p.child(n).is_some()));
                mark(c, h, text, depth + 1);
                if styled {
                    mark_text(h, text);
                }
            }
            "txBody" | "p" | "fld" | "lstStyle" => mark(c, h, text, depth + 1),
            "t" => text.push_str(&c.text),
            "br" => text.push('\n'),
            name => {
                mark_text(h, text);
                mark_bytes(h, b"<");
                mark_bytes(h, name.as_bytes());
                mark(c, h, text, depth + 1);
                mark_text(h, text);
                mark_bytes(h, b">");
            }
        }
    }
}

/// Hashes the run text gathered so far, if any, and clears it.
fn mark_text(h: &mut u64, text: &mut String) {
    if !text.is_empty() {
        mark_bytes(h, b"\"");
        mark_bytes(h, text.as_bytes());
        mark_bytes(h, b"\"");
        text.clear();
    }
}

fn mark_bytes(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Children in order, skipping property elements.
const PROPERTIES: &[(&str, &[&str])] = &[
    ("rPr", &["nor", "sty", "scr", "lit"]),
    ("fPr", &["type"]),
    ("radPr", &["degHide"]),
    ("naryPr", &["chr", "limLoc", "subHide", "supHide"]),
    ("dPr", &["begChr", "sepChr", "endChr"]),
    ("accPr", &["chr"]),
    ("barPr", &["pos"]),
    ("groupChrPr", &["chr", "pos"]),
];

/// The properties of the property element `pr` that the converter reads.
fn read_properties(pr: &str) -> &'static [&'static str] {
    PROPERTIES.iter().find(|(name, _)| *name == pr).map_or(&[], |(_, names)| names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(xml: &str) -> El {
        crate::xml::parse(format!("<m:oMath xmlns:m=\"m\" xmlns:a=\"a\">{xml}</m:oMath>").as_bytes()).unwrap()
    }

    fn latex(xml: &str) -> Option<String> {
        to_latex(&el(xml))
    }

    #[test]
    fn conversion_does_not_accept_a_partial_equation() {
        assert_eq!(latex("<m:r><m:t>x</m:t></m:r><m:unknown/>"), None);
        assert_eq!(
            latex(
                "<m:r><m:t>x</m:t></m:r><mc:AlternateContent xmlns:mc=\"mc\"><mc:Fallback><m:r><m:t>y</m:t></m:r></mc:Fallback></mc:AlternateContent>"
            ),
            None
        );
        assert_eq!(latex("<m:r><m:t>x^2</m:t></m:r>"), None);
        assert_eq!(latex("<m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>").as_deref(), Some("{x}^{2}"));
        let cells = "<m:e><m:r><m:t>x</m:t></m:r></m:e>".repeat(65);
        assert_eq!(latex(&format!("<m:m><m:mr>{cells}</m:mr></m:m>")), None);
    }

    #[test]
    fn literal_run_property_invalidates_the_saved_source_marker() {
        let plain = el("<m:r><m:t>x</m:t></m:r>");
        let literal = el("<m:r><m:rPr><m:lit/></m:rPr><m:t>x</m:t></m:r>");
        assert_ne!(body_marker(&plain), body_marker(&literal));
        assert_eq!(body_marker(&plain), body_marker(&plain.clone()));
    }

    #[test]
    fn the_subset_reads_back() {
        assert_eq!(latex("<m:r><m:t>x+1</m:t></m:r>").as_deref(), Some("x+1"));
        assert_eq!(latex("<m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f>").as_deref(), Some("\\frac{a}{b}"));
        assert_eq!(latex("<m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>").as_deref(), Some("{x}^{2}"));
        assert_eq!(
            latex("<m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/><m:e><m:r><m:t>π</m:t></m:r></m:e></m:rad>").as_deref(),
            Some("\\sqrt{\\pi }")
        );
        let sum = latex(
            "<m:nary><m:naryPr><m:chr m:val=\"∑\"/><m:limLoc m:val=\"undOvr\"/></m:naryPr><m:sub><m:r><m:t>i=1</m:t></m:r></m:sub><m:sup><m:r><m:t>n</m:t></m:r></m:sup><m:e><m:r><m:t>i</m:t></m:r></m:e></m:nary>",
        );
        assert_eq!(sum.as_deref(), Some("\\sum\\limits_{i=1}^{n} {i}"));
        // An integral without a character, with hidden limits.
        assert_eq!(latex("<m:nary><m:naryPr><m:subHide m:val=\"1\"/><m:supHide m:val=\"1\"/></m:naryPr><m:sub/><m:sup/><m:e><m:r><m:t>f</m:t></m:r></m:e></m:nary>").as_deref(), Some("\\int {f}"));
        let m = latex(
            "<m:d><m:dPr><m:begChr m:val=\"[\"/><m:endChr m:val=\"]\"/></m:dPr><m:e><m:m><m:mr><m:e><m:r><m:t>1</m:t></m:r></m:e><m:e><m:r><m:t>0</m:t></m:r></m:e></m:mr><m:mr><m:e><m:r><m:t>0</m:t></m:r></m:e><m:e><m:r><m:t>1</m:t></m:r></m:e></m:mr></m:m></m:e></m:d>",
        );
        assert_eq!(m.as_deref(), Some("\\left[\\begin{matrix}1&0\\\\0&1\\end{matrix}\\right]"));
        assert_eq!(latex("<m:r><m:rPr><m:sty m:val=\"p\"/></m:rPr><m:t>sin</m:t></m:r><m:r><m:t>x</m:t></m:r>").as_deref(), Some("\\sin x"));
        assert_eq!(latex("<m:r><m:rPr><m:scr m:val=\"double-struck\"/></m:rPr><m:t>R</m:t></m:r>").as_deref(), Some("\\mathbb{R}"));
        assert_eq!(latex("<m:r><m:rPr><m:nor/></m:rPr><m:t>if {x}</m:t></m:r>").as_deref(), Some("\\text{if \\{x\\}}"));
        assert_eq!(
            latex("<m:acc><m:accPr><m:chr m:val=\"\u{0302}\"/></m:accPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:acc>").as_deref(),
            Some("\\hat{x}")
        );
    }

    #[test]
    fn what_is_outside_the_subset_is_none() {
        assert_eq!(latex("<m:sPre><m:sub/><m:sup/><m:e/></m:sPre>").as_deref(), Some("{}_{}^{}{}"));
        assert_eq!(latex(""), None, "an empty equation is not maths");
        assert_eq!(latex("<m:nary><m:naryPr><m:chr m:val=\"☃\"/></m:naryPr><m:e/></m:nary>").as_deref(), Some("☃ {}"));
        // Deep nesting stops instead of recursing without bound.
        let deep = format!("{}<m:r><m:t>x</m:t></m:r>{}", "<m:box><m:e>".repeat(100), "</m:e></m:box>".repeat(100));
        assert_eq!(latex(&deep), None);
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // tests run only off wasm, where the clock exists
    fn latex_past_the_source_cap_stops_converting_and_falls_back() {
        // 100,000 sibling runs: the conversion stops once its LaTeX exceeds the cap that
        // `gridcraft_math::check` enforces, instead of building the whole string and then
        // refusing it, so the caller falls back to the writer's text in bounded time.
        let wide = el(&"<m:r><m:t>x</m:t></m:r>".repeat(100_000));
        let start = std::time::Instant::now();
        assert_eq!(to_latex(&wide), None);
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
        // One run whose text alone is past the cap is refused before it is escaped (each
        // backslash would otherwise grow to `\textbackslash `).
        let long = el(&format!("<m:r><m:rPr><m:nor/></m:rPr><m:t>{}</m:t></m:r>", "\\".repeat(gridcraft_math::MAX_SOURCE_BYTES + 1)));
        assert_eq!(to_latex(&long), None);
        // Groups count the braces they add even when no run adds text.
        let groups = el(&"<m:box><m:e/></m:box>".repeat(gridcraft_math::MAX_SOURCE_BYTES / 2 + 1));
        assert_eq!(to_latex(&groups), None);
        // At the cap the equation still reads back, and the cap counts the LaTeX produced, not
        // the run's bytes: a minus sign (three bytes of UTF-8) becomes a one-byte `-`.
        let at_cap = el(&"<m:r><m:t>x</m:t></m:r>".repeat(gridcraft_math::MAX_SOURCE_BYTES));
        assert_eq!(to_latex(&at_cap).map(|s| s.len()), Some(gridcraft_math::MAX_SOURCE_BYTES));
        let minus = el(&format!("<m:r><m:t>{}</m:t></m:r>", "\u{2212}".repeat(gridcraft_math::MAX_SOURCE_BYTES)));
        assert_eq!(to_latex(&minus), Some("-".repeat(gridcraft_math::MAX_SOURCE_BYTES)));
    }

    #[test]
    fn a_blank_middle_row_comes_back_and_a_trailing_one_does_not() {
        // The writer keeps an author's blank row and drops only the empty row a trailing `\\`
        // leaves; the reader joins rows with `\\`, so the count of `\\` is the row count less one.
        let back = |src: &str| {
            let omml = gridcraft_math::to_omml(src, 11.0, None).unwrap();
            to_latex(&crate::xml::parse(format!("<x xmlns:m=\"m\" xmlns:a=\"a\">{omml}</x>").as_bytes()).unwrap().children[0].clone()).unwrap()
        };
        let cases = back("\\begin{cases} x \\\\ \\\\ y \\end{cases}");
        assert_eq!(cases.matches("\\\\").count(), 2, "three rows: {cases}");
        let trailing = back("\\begin{matrix} a \\\\ b \\\\ \\end{matrix}");
        assert_eq!(trailing.matches("\\\\").count(), 1, "two rows: {trailing}");
    }

    #[test]
    fn a_space_in_text_mode_survives_the_round_trip() {
        let omml = gridcraft_math::to_omml("\\text{if } x", 11.0, None).unwrap();
        let back =
            to_latex(&crate::xml::parse(format!("<x xmlns:m=\"m\" xmlns:a=\"a\">{omml}</x>").as_bytes()).unwrap().children[0].clone()).unwrap();
        assert!(back.contains("\\text{ }") || back.contains("if }"), "the space after `if` is still there: {back}");
        let spaces = |l: &gridcraft_math::MathList| l.items.iter().filter(|i| matches!(i, gridcraft_math::Item::Glyph(g) if g.ch == ' ')).count();
        let (a, b) = (gridcraft_math::layout("\\text{if } x").unwrap(), gridcraft_math::layout(&back).unwrap());
        assert_eq!(spaces(&a), spaces(&b), "{back}");
    }

    #[test]
    fn a_limit_on_the_opposite_side_of_a_brace_keeps_its_placement() {
        for (properties, brace, opposite, same) in [
            (r#"<m:chr m:val="&#x23DF;"/><m:pos m:val="top"/>"#, "overbrace", "limUpp", "limLow"),
            (r#"<m:chr m:val="&#x23DE;"/><m:pos m:val="bot"/>"#, "underbrace", "limLow", "limUpp"),
            (r#"<m:pos m:val="top"/>"#, "overbrace", "limUpp", "limLow"),
            (r#"<m:chr m:val="?"/><m:pos m:val="top"/>"#, "overbrace", "limLow", "limUpp"),
        ] {
            let base = format!("<m:groupChr><m:groupChrPr>{properties}</m:groupChrPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:groupChr>");
            let limit = |kind: &str| format!("<m:{kind}><m:e>{base}</m:e><m:lim><m:r><m:t>n</m:t></m:r></m:lim></m:{kind}>");
            let script = |kind: &str| if kind == "limUpp" { '^' } else { '_' };
            assert_eq!(latex(&limit(opposite)), Some(format!("\\{brace}{{x}}{}{{n}}", script(opposite))), "{properties}");
            assert_eq!(latex(&limit(same)), Some(format!("\\{brace}{{x}}{}{{n}}", script(same))), "{properties}");
        }
    }

    #[test]
    fn a_labelled_brace_reads_back_with_a_script_on_its_other_side() {
        // A limit over a brace alone is the brace's label, which LaTeX writes as the brace's own
        // script: `\underbrace{x}_{n}` and `\overbrace{x}^{n}`, as the writer writes them. A
        // script around the whole (`m:sSup` around `m:limLow`) then comes back as well, so
        // `\underbrace{x}_{n}^{2}` keeps its exponent through the round trip. A limit over any
        // other base is still \underset or \overset.
        let brace = |chr: &str, pos: &str| {
            format!(
                "<m:groupChr><m:groupChrPr><m:chr m:val=\"{chr}\"/><m:pos m:val=\"{pos}\"/></m:groupChrPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:groupChr>"
            )
        };
        let lim = "<m:lim><m:r><m:t>n</m:t></m:r></m:lim>";
        let under = format!("<m:limLow><m:e>{}</m:e>{lim}</m:limLow>", brace("\u{23DF}", "bot"));
        assert_eq!(latex(&under).as_deref(), Some("\\underbrace{x}_{n}"));
        assert_eq!(
            latex(&format!("<m:sSup><m:e>{under}</m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>")).as_deref(),
            Some("{\\underbrace{x}_{n}}^{2}")
        );
        assert_eq!(latex(&format!("<m:limUpp><m:e>{}</m:e>{lim}</m:limUpp>", brace("\u{23DE}", "top"))).as_deref(), Some("\\overbrace{x}^{n}"));
        // Control properties beside the brace do not make it a second child.
        assert_eq!(
            latex(&format!("<m:limLow><m:e><m:ctrlPr/>{}</m:e>{lim}</m:limLow>", brace("\u{23DF}", "bot"))).as_deref(),
            Some("\\underbrace{x}_{n}")
        );
        assert_eq!(latex(&format!("<m:limLow><m:e><m:r><m:t>a</m:t></m:r></m:e>{lim}</m:limLow>")).as_deref(), Some("\\underset{n}{a}"));
        assert_eq!(
            latex(&format!("<m:limLow><m:e>{}<m:r><m:t>+1</m:t></m:r></m:e>{lim}</m:limLow>", brace("\u{23DF}", "bot"))).as_deref(),
            Some("\\underset{n}{\\underbrace{x}+1}")
        );
        // Written by GridCraft and read back, both scripts are there and the glyphs are the same.
        let back = |src: &str| {
            let omml = gridcraft_math::to_omml(src, 11.0, None).unwrap();
            to_latex(&crate::xml::parse(format!("<x xmlns:m=\"m\" xmlns:a=\"a\">{omml}</x>").as_bytes()).unwrap().children[0].clone()).unwrap()
        };
        let glyphs = |l: &str| -> Vec<char> {
            let mut g: Vec<char> = gridcraft_math::layout(l)
                .unwrap()
                .items
                .iter()
                .filter_map(|i| if let gridcraft_math::Item::Glyph(g) = i { Some(g.ch) } else { None })
                .filter(|c| !c.is_whitespace())
                .collect();
            g.sort_unstable();
            g
        };
        for (src, expected) in [
            ("\\underbrace{x}_{n}^{2}", "{\\underbrace{x}_{n}}^{2}"),
            ("\\overbrace{x}^{n}_{m}", "{\\overbrace{x}^{n}}_{m}"),
            ("\\underbrace{x}^{2}", "{\\underbrace{x}}^{2}"),
            ("\\underbrace{x}_{n}", "\\underbrace{x}_{n}"),
        ] {
            let b = back(src);
            assert_eq!(b, expected);
            assert_eq!(glyphs(&b), glyphs(src), "{src} came back as {b}");
        }
    }

    #[test]
    fn the_body_marker_follows_the_equation_not_its_serialisation() {
        let tb = |inner: &str| {
            crate::xml::parse(
                format!(
                    "<xdr:txBody xmlns:xdr=\"x\" xmlns:a=\"a\" xmlns:m=\"m\" xmlns:a14=\"a14\"><a:bodyPr/><a:lstStyle/><a:p><a:pPr algn=\"ctr\"/><a14:m><m:oMathPara><m:oMathParaPr/><m:oMath>{inner}</m:oMath></m:oMathPara></a14:m><a:endParaRPr/></a:p></xdr:txBody>"
                )
                .as_bytes(),
            )
            .unwrap()
        };
        let base = body_marker(&tb("<m:r><m:t>x+1</m:t></m:r>"));
        assert_eq!(base.len(), 16);
        assert!(base.bytes().all(|b| b.is_ascii_hexdigit()));
        // Run boundaries, whitespace between elements, attributes, and properties the converter
        // does not read (`ctrlPr`, an extension list, DrawingML size, colour and font) are not
        // the equation: what Excel re-serialises differently leaves the marker alone.
        assert_eq!(body_marker(&tb("<m:r><m:t>x</m:t></m:r><m:r><m:t>+1</m:t></m:r>")), base);
        assert_eq!(body_marker(&tb("\n  <m:r>\n <a:rPr lang=\"en-US\" sz=\"1100\"/><m:t xml:space=\"preserve\">x+1</m:t></m:r>\n")), base);
        assert_eq!(body_marker(&tb("<m:r><m:t>x+1</m:t></m:r><m:ctrlPr><a:rPr lang=\"en-US\" i=\"1\"/></m:ctrlPr>")), base);
        assert_eq!(body_marker(&tb("<m:r><m:t>x+1</m:t></m:r><a:extLst><a:ext uri=\"{x}\"><a:z/></a:ext></a:extLst>")), base);
        assert_eq!(
            body_marker(&tb(
                "<m:r><a:rPr sz=\"2400\"><a:solidFill><a:srgbClr val=\"FF0000\"/></a:solidFill><a:latin typeface=\"Cambria Math\"/></a:rPr><m:t>x+1</m:t></m:r>"
            )),
            base
        );
        assert_eq!(
            body_marker(&tb("<m:r><m:rPr><m:rFonts m:ascii=\"Cambria\"/></m:rPr><m:t>x+1</m:t></m:r>")),
            base,
            "a property the converter never reads"
        );
        // Properties the converter reads are: a run's style, an accent's character, a delimiter's
        // brackets, an operator's limits; the same property on another run is a change too.
        let styled = |sty: &str| body_marker(&tb(&format!("<m:r><m:rPr><m:sty m:val=\"{sty}\"/></m:rPr><m:t>x</m:t></m:r><m:r><m:t>+1</m:t></m:r>")));
        assert_ne!(styled("p"), base);
        assert_ne!(styled("p"), styled("b"));
        assert_ne!(styled("p"), body_marker(&tb("<m:r><m:t>x</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\"/></m:rPr><m:t>+1</m:t></m:r>")));
        // The converter converts a run with a read property on its own and recognises `sin` per
        // run, so a styled `sin` split into `s` and `in` (the second styled or not) is a change,
        // while plain runs convert character by character and may be split or joined freely.
        let upright = |runs: &[(&str, bool)]| {
            let xml: String = runs
                .iter()
                .map(|(t, p)| format!("<m:r>{}<m:t>{t}</m:t></m:r>", if *p { "<m:rPr><m:sty m:val=\"p\"/></m:rPr>" } else { "" }))
                .collect();
            body_marker(&tb(&xml))
        };
        assert_ne!(upright(&[("sin", true)]), upright(&[("s", true), ("in", false)]));
        assert_ne!(upright(&[("sin", true)]), upright(&[("s", true), ("in", true)]));
        assert_ne!(upright(&[("sin", true), ("x", false)]), upright(&[("s", true), ("inx", false)]));
        assert_eq!(upright(&[("ab", false)]), upright(&[("a", false), ("b", false)]));
        assert_eq!(upright(&[("sin", true), ("xy", false)]), upright(&[("sin", true), ("x", false), ("y", false)]));
        let accented =
            |chr: &str| body_marker(&tb(&format!("<m:acc><m:accPr><m:chr m:val=\"{chr}\"/></m:accPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:acc>")));
        assert_ne!(accented("\u{0302}"), accented("\u{0303}"));
        let bracketed = |b: &str, e: &str| {
            body_marker(&tb(&format!(
                "<m:d><m:dPr><m:begChr m:val=\"{b}\"/><m:endChr m:val=\"{e}\"/></m:dPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:d>"
            )))
        };
        assert_ne!(bracketed("(", ")"), bracketed("[", "]"));
        assert_ne!(
            body_marker(&tb("<m:nary><m:naryPr><m:chr m:val=\"∑\"/></m:naryPr><m:sub/><m:sup/><m:e><m:r><m:t>i</m:t></m:r></m:e></m:nary>")),
            body_marker(&tb(
                "<m:nary><m:naryPr><m:chr m:val=\"∑\"/><m:subHide m:val=\"1\"/></m:naryPr><m:sub/><m:sup/><m:e><m:r><m:t>i</m:t></m:r></m:e></m:nary>"
            ))
        );
        // Text and structure are the equation.
        assert_ne!(body_marker(&tb("<m:r><m:t>x+2</m:t></m:r>")), base);
        assert_ne!(body_marker(&tb("<m:r><m:t>x+1</m:t></m:r><m:r><m:t> </m:t></m:r>")), base, "a space added is a change");
        assert_ne!(body_marker(&tb("<m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>+1</m:t></m:r></m:sup></m:sSup>")), base);
        assert_ne!(body_marker(&tb("<m:f><m:num><m:r><m:t>x</m:t></m:r></m:num><m:den><m:r><m:t>+1</m:t></m:r></m:den></m:f>")), base);
        assert_ne!(body_marker(&tb("<m:r><m:t>x+1</m:t></m:r>")), body_marker(&tb("")));
    }

    #[test]
    fn writing_then_reading_keeps_the_maths() {
        for (_, src) in gridcraft_math::GALLERY {
            let omml = gridcraft_math::to_omml(src, 11.0, None).unwrap();
            let back =
                to_latex(&crate::xml::parse(format!("<x xmlns:m=\"m\" xmlns:a=\"a\">{omml}</x>").as_bytes()).unwrap().children[0].clone()).unwrap();
            let (a, b) = (gridcraft_math::layout(src).unwrap(), gridcraft_math::layout(&back).unwrap());
            let glyphs = |l: &gridcraft_math::MathList| -> String {
                l.items
                    .iter()
                    .filter_map(|i| if let gridcraft_math::Item::Glyph(g) = i { Some(g.ch) } else { None })
                    .filter(|c| !c.is_whitespace())
                    .collect()
            };
            assert_eq!(glyphs(&a), glyphs(&b), "{src} came back as {back}");
        }
    }
}
