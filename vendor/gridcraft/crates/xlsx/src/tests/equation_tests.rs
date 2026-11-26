//! Equations in xlsx: Office Math with a plain-text fallback, the LaTeX source in `descr`, and
//! Office Math alone (as other writers save it) read back into LaTeX.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::Arc;

use gridcraft_core::CellRef;
use gridcraft_model::*;

use crate::{read_xlsx, write_xlsx};

fn equation(id: u32, cell: &str, latex: &str, ink: Color) -> Shape {
    Shape {
        id,
        kind: ShapeKind::Equation,
        anchor: Anchor { cell: CellRef::parse(cell).unwrap(), width: 200.0, height: 60.0, mode: AnchorMode::MoveOnly, ..Default::default() },
        fill: Color::Auto,
        line: ink,
        text: latex.into(),
        format: Default::default(),
        math: Default::default(),
        rotation: 0.0,
    }
}

fn book(shapes: Vec<Shape>) -> Workbook {
    let mut wb = Workbook::new();
    let mut sh = (*wb.sheets[0]).clone();
    for mut s in shapes {
        s.math.refresh(&s.text.clone());
        sh.shapes.push(s);
    }
    wb.sheets[0] = Arc::new(sh);
    wb
}

fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut m = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        m.insert(f.name().to_string(), b);
    }
    m
}

fn rezip(parts: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in parts {
        out.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
        out.write_all(data).unwrap();
    }
    out.finish().unwrap().into_inner()
}

fn drawing(parts: &BTreeMap<String, Vec<u8>>) -> String {
    String::from_utf8(parts.iter().find(|(k, _)| k.starts_with("xl/drawings/drawing")).unwrap().1.clone()).unwrap()
}

fn shapes(wb: &Workbook) -> Vec<Shape> {
    wb.sheets[0].shapes.clone()
}

/// Whether the drawing `d` describes an equation with `src`: `descr="gridcraft:latex:<16 hex>:src"`.
fn described(d: &str, src: &str) -> bool {
    d.match_indices("descr=\"gridcraft:latex:").any(|(i, m)| {
        let rest = &d[i + m.len()..];
        rest.len() > 17
            && rest.as_bytes()[..16].iter().all(u8::is_ascii_hexdigit)
            && rest[16..].starts_with(':')
            && rest[17..].starts_with(&format!("{src}\""))
    })
}

#[test]
fn equations_write_office_math_with_a_text_fallback_and_read_back() {
    let quad = "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}";
    let cd = "\\begin{CD} A @>f>> B \\end{CD}";
    let wb = book(vec![
        equation(1, "B2", quad, Color::Rgb(0x1F4E79)),
        equation(2, "B8", cd, Color::Auto),
        equation(3, "B12", "a < b \\text{ \"q\" }\nc", Color::Auto),
    ]);
    let bytes = write_xlsx(&wb).unwrap();
    let parts = unzip(&bytes);
    let d = drawing(&parts);
    // The quadratic formula: a choice with Office Math and a fallback with the source as text.
    assert!(d.contains("<mc:Choice xmlns:a14=\"http://schemas.microsoft.com/office/drawing/2010/main\" Requires=\"a14\">"));
    assert!(d.contains("<a14:m><m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\">"));
    assert!(d.contains("<m:f><m:num>") && d.contains("<m:rad>"));
    assert!(described(&d, "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}"), "{d}");
    assert!(d.contains("<a:t>x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}</a:t>"), "the fallback shows the source");
    assert!(d.contains("<a:srgbClr val=\"1F4E79\"/>"), "the ink colour is kept");
    // The colour sits on the Office Math runs themselves, not only on the text fallback (which
    // the reader would take if the Choice were ever filtered out).
    let math = &d[d.find("<a14:m>").unwrap()..d.find("</a14:m>").unwrap()];
    assert!(
        math.contains("<a:solidFill><a:srgbClr val=\"1F4E79\"/></a:solidFill><a:latin typeface=\"Cambria Math\"/></a:rPr>"),
        "the Office Math runs carry the ink colour: {math}"
    );
    // A commutative diagram is outside the Office Math subset: text only, no choice.
    assert_eq!(d.matches("<mc:AlternateContent").count(), 2, "{d}");
    assert!(described(&d, "\\begin{CD} A @&gt;f&gt;&gt; B \\end{CD}"), "{d}");
    // Read back: the source wins, exactly, including quotes and a newline.
    let (back, _) = read_xlsx(&bytes).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 3);
    assert!(s.iter().all(|x| x.kind == ShapeKind::Equation));
    assert_eq!(s[0].text, quad);
    assert_eq!(s[0].line, Color::Rgb(0x1F4E79));
    assert_eq!(s[1].text, cd);
    assert_eq!(s[2].text, "a < b \\text{ \"q\" }\nc");
    assert_eq!(s[0].anchor.mode, AnchorMode::MoveOnly);
    assert!((s[0].anchor.width - 200.0).abs() < 1.0);
}

#[test]
fn office_math_without_a_source_is_converted_to_latex() {
    let quad = "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}";
    let wb = book(vec![equation(1, "B2", quad, Color::Auto), equation(2, "B8", "\\sum_{k=0}^{n} \\binom{n}{k} \\le \\infty", Color::Auto)]);
    let mut parts = unzip(&write_xlsx(&wb).unwrap());
    // Strip GridCraft's source, as a file saved by another application would be.
    let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    let mut d = drawing(&parts);
    while let Some(a) = d.find(" descr=\"gridcraft:latex:") {
        let b = d[a + 8..].find('"').unwrap() + a + 9;
        d.replace_range(a..b, "");
    }
    assert!(!d.contains("gridcraft:latex"));
    parts.insert(name, d.into_bytes());
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 2);
    for (x, src) in s.iter().zip([quad, "\\sum_{k=0}^{n} \\binom{n}{k} \\le \\infty"]) {
        assert_eq!(x.kind, ShapeKind::Equation, "{}", x.text);
        // The conversion lays out to the same glyphs as the original.
        let glyphs = |t: &str| -> String {
            gridcraft_math::layout(t)
                .unwrap()
                .items
                .iter()
                .filter_map(|i| if let gridcraft_math::Item::Glyph(g) = i { Some(g.ch) } else { None })
                .collect()
        };
        assert_eq!(glyphs(&x.text), glyphs(src), "{} from {src}", x.text);
    }
}

#[test]
fn a_text_box_stays_a_text_box_and_hostile_sources_survive() {
    let mut wb = book(vec![equation(1, "B2", &"{".repeat(40), Color::Auto), equation(2, "B6", "\u{0}\u{1F600} & ]]> <!-- -->", Color::Auto)]);
    let mut sh = (*wb.sheets[0]).clone();
    sh.shapes.push(Shape {
        id: 3,
        kind: ShapeKind::TextBox,
        anchor: Anchor { cell: CellRef::parse("D2").unwrap(), width: 80.0, height: 30.0, ..Default::default() },
        fill: Color::Auto,
        line: Color::Auto,
        text: "plain".into(),
        format: Default::default(),
        math: Default::default(),
        rotation: 0.0,
    });
    wb.sheets[0] = Arc::new(sh);
    let (back, _) = read_xlsx(&write_xlsx(&wb).unwrap()).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 3);
    assert_eq!(s[0].text, "{".repeat(40));
    assert_eq!(s[0].kind, ShapeKind::Equation);
    assert_eq!(s[1].text, "\u{1F600} & ]]> <!-- -->", "XML-illegal characters are dropped, the rest kept");
    assert_eq!(s[2].kind, ShapeKind::TextBox);
    assert_eq!(s[2].text, "plain");
}

#[test]
fn office_math_edited_elsewhere_reads_back_as_edited() {
    // GridCraft writes the source in the description and Office Math in the body. Another
    // application that edits the Office Math (Excel) leaves the description alone; the marker in
    // the description no longer matches the body, so the Office Math is converted and the stale
    // source dropped, and the next save writes the edited equation.
    let wb = book(vec![equation(1, "B2", "x^2", Color::Rgb(0x1F4E79))]);
    let mut parts = unzip(&write_xlsx(&wb).unwrap());
    let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    let mut d = drawing(&parts);
    assert!(described(&d, "x^2"));
    // Excel's edit: the base `x` becomes `y`, in the Office Math only.
    let (m0, m1) = (d.find("<m:oMath>").unwrap(), d.find("</m:oMath>").unwrap());
    let edited = d[m0..m1].replacen("<m:t>x</m:t>", "<m:t>y</m:t>", 1);
    assert_ne!(edited, d[m0..m1]);
    d.replace_range(m0..m1, &edited);
    parts.insert(name, d.into_bytes());
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, ShapeKind::Equation);
    assert_eq!(s[0].text, "{y}^{2}", "the edited Office Math, converted");
    assert_eq!(s[0].line, Color::Rgb(0x1F4E79));
    // Saving writes the edited equation; the old source is gone.
    let again = drawing(&unzip(&write_xlsx(&back).unwrap()));
    assert!(described(&again, "{y}^{2}") && !again.contains("x^2"), "{again}");
}

#[test]
fn a_marker_over_garbage_office_math_falls_back_without_panicking() {
    // Office Math that is not the marker's and not in the subset either: the Choice is left
    // alone and the fallback text box, which carries the description, is read as the equation.
    let wb = book(vec![equation(1, "B2", "x^2", Color::Auto)]);
    let mut parts = unzip(&write_xlsx(&wb).unwrap());
    let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    let mut d = drawing(&parts);
    let (m0, m1) = (d.find("<m:oMath>").unwrap(), d.find("</m:oMath>").unwrap());
    d.replace_range(m0..m1 + "</m:oMath>".len(), "<m:oMath><m:unknown/><m:r/><m:t>junk</m:t><m:oMath/><m:e/></m:oMath>");
    parts.insert(name.clone(), d.into_bytes());
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, ShapeKind::Equation);
    assert_eq!(s[0].text, "x^2");
    // A description whose marker is malformed is not GridCraft's source: with Office Math the
    // maths is converted; without it the box is text.
    let d2 = drawing(&unzip(&write_xlsx(&wb).unwrap())).replace("descr=\"gridcraft:latex:", "descr=\"gridcraft:latex:zz:");
    parts.insert(name, d2.into_bytes());
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 1);
    assert_eq!((s[0].kind, s[0].text.as_str()), (ShapeKind::Equation, "{x}^{2}"));
}

#[test]
fn office_math_outside_the_subset_reads_the_fallback() {
    let wb = book(vec![equation(1, "B2", "x^2", Color::Auto)]);
    let mut parts = unzip(&write_xlsx(&wb).unwrap());
    let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    let mut d = drawing(&parts);
    // Another writer: no GridCraft source, Office Math the reader does not convert,
    // and a fallback text box with its own text.
    while let Some(a) = d.find(" descr=\"gridcraft:latex:") {
        let b = d[a + 8..].find('"').unwrap() + a + 9;
        d.replace_range(a..b, "");
    }
    let (m0, m1) = (d.find("<m:oMath>").unwrap(), d.find("</m:oMath>").unwrap());
    d.replace_range(m0..m1 + "</m:oMath>".len(), "<m:oMath><m:unknown><m:e><m:r><m:t>y</m:t></m:r></m:e></m:unknown></m:oMath>");
    let fb = d.find("<mc:Fallback>").unwrap();
    let t = d[fb..].find("<a:t>x^2</a:t>").unwrap() + fb;
    d.replace_range(t..t + "<a:t>x^2</a:t>".len(), "<a:t>the writer's fallback</a:t>");
    parts.insert(name, d.into_bytes());
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, ShapeKind::TextBox, "not an equation");
    assert_eq!(s[0].text, "the writer's fallback", "the fallback is read, not the empty Office Math box");
}

#[test]
fn hostile_sources_round_trip_exactly() {
    // Sources built from the characters XML and LaTeX treat specially: each is saved (as Office
    // Math with a fallback, or as text) and read back as the same equation source.
    const PARTS: &[&str] = &[
        "x",
        "\\frac{a}{b}",
        "\"",
        "'",
        "<",
        ">",
        "&",
        "&amp;",
        "]]>",
        "<!--",
        "\n",
        "\t",
        "\r",
        "\r\n",
        " ",
        "\u{e9}",
        "\u{1F600}",
        "\u{4E2D}",
        "{",
        "}",
        "^",
        "_",
        "\\\\",
        "%",
    ];
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let mut sources = Vec::new();
    for _ in 0..200 {
        let mut s = String::from("x");
        for _ in 0..1 + next(10) {
            s.push_str(PARTS[next(PARTS.len())]);
        }
        sources.push(s);
    }
    let shapes_in: Vec<Shape> = sources.iter().enumerate().map(|(i, s)| equation(i as u32 + 1, &format!("B{}", i * 4 + 2), s, Color::Auto)).collect();
    let (back, _) = read_xlsx(&write_xlsx(&book(shapes_in)).unwrap()).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), sources.len());
    for (x, src) in s.iter().zip(&sources) {
        assert_eq!(x.kind, ShapeKind::Equation, "{src:?}");
        assert_eq!(&x.text, src, "the source survives exactly");
    }
}

#[test]
fn office_math_beside_text_or_other_math_is_not_an_equation() {
    // Another writer's text box: no GridCraft source, and Office Math the reader converts, but
    // sharing the box with ordinary text or with a second equation. Taking it as one equation
    // would drop the rest, so it stays a text box (or the writer's fallback is read).
    let saved = || {
        let wb = book(vec![equation(1, "B2", "x^2", Color::Auto)]);
        let parts = unzip(&write_xlsx(&wb).unwrap());
        let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
        let mut d = drawing(&parts);
        while let Some(a) = d.find(" descr=\"gridcraft:latex:") {
            let b = d[a + 8..].find('"').unwrap() + a + 9;
            d.replace_range(a..b, "");
        }
        (parts, name, d)
    };
    let read = |mut parts: BTreeMap<String, Vec<u8>>, name: String, d: String| {
        parts.insert(name, d.into_bytes());
        shapes(&read_xlsx(&rezip(&parts)).unwrap().0)
    };
    let math_para = |d: &str| {
        let a = d.find("<a:p><a:pPr algn=\"ctr\"/><a14:m>").unwrap();
        let b = d[a..].find("</a:p>").unwrap() + a + "</a:p>".len();
        d[a..b].to_string()
    };

    // 1. In the a14 choice, a text run before the maths: the fallback is read instead.
    let (parts, name, mut d) = saved();
    let at = d.find("<a14:m>").unwrap();
    d.insert_str(at, "<a:r><a:rPr lang=\"en-US\"/><a:t>Solve: </a:t></a:r>");
    let fb = d.find("<mc:Fallback>").unwrap();
    let t = d[fb..].find("<a:t>x^2</a:t>").unwrap() + fb;
    d.replace_range(t..t + "<a:t>x^2</a:t>".len(), "<a:t>Solve: x squared</a:t>");
    let s = read(parts, name, d);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, ShapeKind::TextBox, "{:?}", s[0].text);
    assert_eq!(s[0].text, "Solve: x squared", "the writer's fallback keeps the text");

    // 2. A plain text box (no alternate content) with the same paragraph keeps its text.
    let (parts, name, d) = saved();
    let (c0, c1) = (d.find("<mc:Choice").unwrap(), d.find("</mc:Choice>").unwrap());
    let choice = &d[c0..c1];
    let sp = choice[choice.find("<xdr:sp ").unwrap()..].to_string();
    let sp = sp.replacen("<a14:m>", "<a:r><a:rPr lang=\"en-US\"/><a:t>Solve: </a:t></a:r><a14:m>", 1);
    let (a0, a1) = (d.find("<mc:AlternateContent").unwrap(), d.find("</mc:AlternateContent>").unwrap() + "</mc:AlternateContent>".len());
    let mut d2 = d.clone();
    d2.replace_range(a0..a1, &sp);
    let s = read(parts, name, d2);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, ShapeKind::TextBox, "{:?}", s[0].text);
    assert!(s[0].text.contains("Solve:"), "the text is kept: {:?}", s[0].text);

    // 3. Two equations in one box: not read as the first one alone.
    let (parts, name, d) = saved();
    let para = math_para(&d);
    let at = d.find(&para).unwrap() + para.len();
    let mut d3 = d.clone();
    d3.insert_str(at, &para);
    let s = read(parts, name, d3);
    assert_eq!(s.len(), 1);
    assert_ne!(s[0].kind, ShapeKind::Equation, "two equations are not one");
}

#[test]
fn a_brace_with_scripts_on_both_sides_round_trips() {
    // `\underbrace{x}_{n}^{2}`: the label is the brace's limit and the exponent a superscript
    // around the whole. With GridCraft's source in the description, the marker matches the body
    // and the source comes back as written; with the source stripped (a file another application
    // saved) the Office Math is converted and the exponent is still there.
    let src = "\\underbrace{x}_{n}^{2}";
    let wb = book(vec![equation(1, "B2", src, Color::Auto)]);
    let bytes = write_xlsx(&wb).unwrap();
    let mut parts = unzip(&bytes);
    let mut d = drawing(&parts);
    assert!(d.contains("<m:sSup><m:e><m:limLow><m:e><m:groupChr>") && d.contains("</m:limLow></m:e><m:sup><m:r>"), "{d}");
    assert!(described(&d, src), "{d}");
    let (back, _) = read_xlsx(&bytes).unwrap();
    assert_eq!(shapes(&back)[0].text, src, "the marker matches its own Office Math, so the source is taken");
    let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    while let Some(a) = d.find(" descr=\"gridcraft:latex:") {
        let b = d[a + 8..].find('"').unwrap() + a + 9;
        d.replace_range(a..b, "");
    }
    parts.insert(name, d.into_bytes());
    let (converted, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&converted);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, ShapeKind::Equation);
    assert_eq!(s[0].text, "{\\underbrace{x}_{n}}^{2}", "converted from the Office Math, with both scripts");
}

#[test]
fn a_text_edit_to_an_equation_without_office_math_reads_back_as_the_edit() {
    // An equation outside the subset (`\begin{CD}`) is written as a text box with the source as
    // its text and the marker of that box in its description. Another application that edits the
    // text leaves the description alone, as Excel does; the marker then no longer matches, so the
    // box is read as a text box with the edited text, not as the stale source, which the next
    // save would have written over the edit.
    let cd = "\\begin{CD} A @>f>> B \\end{CD}";
    let wb = book(vec![equation(1, "B2", cd, Color::Auto)]);
    let mut parts = unzip(&write_xlsx(&wb).unwrap());
    let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    let mut d = drawing(&parts);
    assert!(!d.contains("<m:oMath>"), "{d}");
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    assert_eq!((shapes(&back)[0].kind, shapes(&back)[0].text.as_str()), (ShapeKind::Equation, cd), "unedited, the source is taken");
    // Excel's edit: the text of the box, not the description.
    let t0 = d.find("<a:t>").unwrap();
    let t1 = d[t0..].find("</a:t>").unwrap() + t0 + "</a:t>".len();
    d.replace_range(t0..t1, "<a:t>edited in Excel</a:t>");
    parts.insert(name, d.into_bytes());
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 1);
    assert_eq!((s[0].kind, s[0].text.as_str()), (ShapeKind::TextBox, "edited in Excel"), "the edit, not the stale source");
    // The fallback box of an equation with Office Math carries the marker of its own text, not
    // the Office Math's, so a reader left with the fallback alone (the Choice gone) still takes
    // the source.
    let mut parts = unzip(&write_xlsx(&book(vec![equation(1, "B2", "x^2", Color::Auto)])).unwrap());
    let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    let mut d = drawing(&parts);
    let markers: Vec<String> = d.match_indices("descr=\"gridcraft:latex:").map(|(i, m)| d[i + m.len()..i + m.len() + 16].to_string()).collect();
    assert_eq!(markers.len(), 2, "{d}");
    assert_ne!(markers[0], markers[1], "the Choice and the Fallback each carry their own body's marker");
    let (c0, c1) = (d.find("<mc:Choice").unwrap(), d.find("</mc:Choice>").unwrap() + "</mc:Choice>".len());
    d.replace_range(c0..c1, "");
    parts.insert(name, d.into_bytes());
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&back);
    assert_eq!((s.len(), s[0].kind, s[0].text.as_str()), (1, ShapeKind::Equation, "x^2"), "the fallback alone still reads as the equation");
}

#[test]
fn a_paragraph_split_in_an_equation_without_office_math_reads_back_as_the_edit() {
    // The marker joins run text across paragraphs, so splitting the one paragraph of a plain-text
    // equation box into two without changing a character leaves it equal to the description's.
    // The box's text, paragraphs included, is compared with what the writer wrote as well, so
    // the split reads back as a text box with two paragraphs instead of the stale source.
    let cd = "\\begin{CD} A @>f>> B \\end{CD}";
    let wb = book(vec![equation(1, "B2", cd, Color::Auto)]);
    let mut parts = unzip(&write_xlsx(&wb).unwrap());
    let name = parts.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    let mut d = drawing(&parts);
    let (p0, p1) = (d.find("<a:p>").unwrap(), d.find("</a:p>").unwrap() + "</a:p>".len());
    let para = d[p0..p1].to_string();
    let (t0, t1) = (para.find("<a:t>").unwrap() + "<a:t>".len(), para.find("</a:t>").unwrap());
    let cut = t0 + para[t0..t1].find(' ').unwrap() + 1;
    let split = format!("{}{}{}{}", &para[..cut], &para[t1..], &para[..t0], &para[cut..]);
    assert_eq!(split.matches("<a:p>").count(), 2);
    d.replace_range(p0..p1, &split);
    parts.insert(name, d.into_bytes());
    let (back, _) = read_xlsx(&rezip(&parts)).unwrap();
    let s = shapes(&back);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, ShapeKind::TextBox, "the split is an edit: {}", s[0].text);
    // The cut falls after the first space, which stays in the first paragraph, so the text read
    // back is the source with a newline after that space, and nothing else moved or dropped.
    assert_eq!(s[0].text, cd.replacen(' ', " \n", 1), "the split text, exactly");
}

#[test]
fn the_plain_text_stand_in_for_an_equation_is_drawn_at_a_normal_size() {
    // A source Office Math cannot hold is saved as the text box alone; a source it can hold has
    // the same box as its fallback. Neither takes the (large) size the equation is drawn at.
    let wb = book(vec![equation(1, "B2", "\\begin{CD} A @>f>> B \\end{CD}", Color::Auto), equation(2, "B8", "x^2", Color::Auto)]);
    let d = drawing(&unzip(&write_xlsx(&wb).unwrap()));
    let text_runs: Vec<&str> =
        d.match_indices("<a:r><a:rPr").map(|(i, _)| &d[i..i + d[i..].find("</a:rPr>").or_else(|| d[i..].find("/>")).unwrap()]).collect();
    assert_eq!(text_runs.len(), 2, "{d}");
    for r in text_runs {
        assert!(r.contains(" sz=\"1100\""), "{r}");
    }
    // The Office Math box keeps the size of the equation.
    assert!(!d.contains("<a:endParaRPr lang=\"en-US\" sz=\"1100\"/>"), "{d}");
}

#[test]
fn rejected_office_math_without_a_saved_source_keeps_visible_text() {
    let wb = book(vec![equation(1, "B2", "x^2", Color::Auto)]);
    let original = unzip(&write_xlsx(&wb).unwrap());
    let name = original.keys().find(|k| k.starts_with("xl/drawings/drawing")).unwrap().clone();
    let d = drawing(&original);
    let c0 = d.find("<mc:Choice").unwrap();
    let c1 = d.find("</mc:Choice>").unwrap();
    let choice = &d[c0..c1];
    let mut sp = choice[choice.find("<xdr:sp ").unwrap()..].to_string();
    while let Some(a) = sp.find(" descr=\"gridcraft:latex:") {
        let b = sp[a + 8..].find('"').unwrap() + a + 9;
        sp.replace_range(a..b, "");
    }
    let a0 = d.find("<mc:AlternateContent").unwrap();
    let a1 = d.find("</mc:AlternateContent>").unwrap() + "</mc:AlternateContent>".len();
    for (body, expected) in [
        ("<a:p><a14:m><m:oMath><m:r><m:t>x^2</m:t></m:r></m:oMath></a14:m></a:p>", "x^2"),
        (
            "<a:p><a:r><a:t>Solve: </a:t></a:r><a14:m><m:oMath><m:r><m:t>x^2</m:t></m:r></m:oMath></a14:m><a:r><a:t> please</a:t></a:r></a:p><a:p><a:r><a:t>next</a:t></a:r></a:p>",
            "Solve: x^2 please\nnext",
        ),
        ("<a:p><a14:m><m:oMath><m:unknown><m:r><m:t>λ^2</m:t></m:r></m:unknown></m:oMath></a14:m></a:p>", "λ^2"),
    ] {
        let mut shape = sp.clone();
        let start = shape.find("<xdr:txBody>").unwrap();
        let end = shape.find("</xdr:txBody>").unwrap() + "</xdr:txBody>".len();
        shape.replace_range(start..end, &format!("<xdr:txBody>{body}</xdr:txBody>"));
        let mut xml = d.clone();
        xml.replace_range(a0..a1, &shape);
        let mut parts = original.clone();
        parts.insert(name.clone(), xml.into_bytes());
        let back = shapes(&read_xlsx(&rezip(&parts)).unwrap().0);
        assert_eq!(back.len(), 1);
        assert_ne!(back[0].kind, ShapeKind::Equation);
        assert_eq!(back[0].text, expected);
        let again = shapes(&read_xlsx(&write_xlsx(&book(back)).unwrap()).unwrap().0);
        assert_eq!(again[0].text, expected);
    }
}
