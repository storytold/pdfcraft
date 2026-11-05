//! Markdown and LaTeX rendering for the AI assistant's answers (M13).
//!
//! Models answer in Markdown with `$…$` / `$$…$$` (or `\(…\)` / `\[…\]`) math. The answer is parsed
//! with `pulldown-cmark` into a small block tree and drawn with egui widgets. Formulas are laid out
//! by `latex-rust` (pure Rust, STIX Two Math outlines), turned into SVG and shown through the SVG
//! image loader the shell already installs. A formula that can't be rendered is shown as its source.
//! Nothing here touches the network, and links in an answer are shown as text, never opened.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use egui::{Color32, RichText};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme::{self, Tokens};

/// Most formulas kept rendered at once; the cache is cleared when it grows past this.
const MAX_CACHED_FORMULAS: usize = 512;
/// Longest formula source we try to typeset.
const MAX_FORMULA_CHARS: usize = 4_000;
/// The em size, in pixels, formulas are typeset at (12 pt); they are scaled to the text from there.
const MATH_EM_PX: f32 = 16.0;

// ---------------------------------------------------------------------------------------------
// Parsing

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Fmt {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Inline {
    Text(String, Fmt),
    Math(String),
    SoftBreak,
    HardBreak,
}

type Inlines = Vec<Inline>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Block {
    Heading(u8, Inlines),
    Para(Inlines),
    Code(String),
    Math(String),
    Quote(Vec<Block>),
    List(Option<u64>, Vec<Vec<Block>>),
    Rule,
    Table(Vec<Inlines>, Vec<Vec<Inlines>>),
}

/// Turn `\(…\)` and `\[…\]` into `$…$` and `$$…$$`, leaving code (fenced or inline) alone.
pub(crate) fn normalize_math(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut fence: Option<char> = None;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some('`')
        } else if trimmed.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        match (fence, marker) {
            (None, Some(m)) => {
                fence = Some(m);
                out.push_str(line);
                continue;
            }
            (Some(f), Some(m)) if f == m => {
                fence = None;
                out.push_str(line);
                continue;
            }
            (Some(_), _) => {
                out.push_str(line);
                continue;
            }
            _ => {}
        }
        // Outside inline code spans only: even segments between backticks.
        for (i, seg) in line.split('`').enumerate() {
            if i > 0 {
                out.push('`');
            }
            if i % 2 == 0 {
                out.push_str(&rewrite_delimiters(seg));
            } else {
                out.push_str(seg);
            }
        }
    }
    out
}

/// `\(` `\)` → `$`, `\[` `\]` → `$$`, except where the backslash is itself escaped: the line
/// break `\\[4pt]` in an aligned formula keeps its `\\`.
fn rewrite_delimiters(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    let mut chars = seg.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('\\') => {
                out.push_str("\\\\");
                chars.next();
            }
            Some('[' | ']') => {
                out.push_str("$$");
                chars.next();
            }
            Some('(' | ')') => {
                out.push('$');
                chars.next();
            }
            _ => out.push('\\'),
        }
    }
    out
}

struct Frame {
    kind: FrameKind,
    blocks: Vec<Block>,
}

enum FrameKind {
    Root,
    Quote,
    Item,
    List(Option<u64>, Vec<Vec<Block>>),
}

#[derive(Default)]
struct TableState {
    head: Vec<Inlines>,
    rows: Vec<Vec<Inlines>>,
    row: Vec<Inlines>,
}

/// Parse a model answer into blocks.
pub(crate) fn parse(src: &str) -> Vec<Block> {
    let src = normalize_math(src);
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_MATH;
    let mut stack = vec![Frame { kind: FrameKind::Root, blocks: Vec::new() }];
    let mut inl: Inlines = Vec::new();
    let mut fmt = Fmt::default();
    let (mut bold, mut italic, mut strike, mut link) = (0u32, 0u32, 0u32, 0u32);
    let mut code_block: Option<String> = None;
    let mut heading: Option<u8> = None;
    let mut table: Option<TableState> = None;

    // Close the pending inline run as a paragraph (tight list items have no paragraph tag).
    fn flush(stack: &mut [Frame], inl: &mut Inlines) {
        if !inl.is_empty()
            && let Some(top) = stack.last_mut()
        {
            top.blocks.push(Block::Para(std::mem::take(inl)));
        }
    }
    fn push_text(inl: &mut Inlines, s: &str, fmt: Fmt) {
        if let Some(Inline::Text(prev, f)) = inl.last_mut()
            && *f == fmt
        {
            prev.push_str(s);
            return;
        }
        inl.push(Inline::Text(s.to_string(), fmt));
    }

    for ev in Parser::new_ext(&src, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    flush(&mut stack, &mut inl);
                    heading = Some(match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    });
                }
                Tag::BlockQuote(_) => {
                    flush(&mut stack, &mut inl);
                    stack.push(Frame { kind: FrameKind::Quote, blocks: Vec::new() });
                }
                Tag::CodeBlock(_) => {
                    flush(&mut stack, &mut inl);
                    code_block = Some(String::new());
                }
                Tag::List(start) => {
                    flush(&mut stack, &mut inl);
                    stack.push(Frame { kind: FrameKind::List(start, Vec::new()), blocks: Vec::new() });
                }
                Tag::Item => {
                    stack.push(Frame { kind: FrameKind::Item, blocks: Vec::new() });
                }
                Tag::Table(_) => {
                    flush(&mut stack, &mut inl);
                    table = Some(TableState::default());
                }
                Tag::TableCell => inl.clear(),
                Tag::Emphasis => {
                    italic += 1;
                    fmt.italic = true;
                }
                Tag::Strong => {
                    bold += 1;
                    fmt.bold = true;
                }
                Tag::Strikethrough => {
                    strike += 1;
                    fmt.strike = true;
                }
                Tag::Link { .. } => {
                    link += 1;
                    fmt.link = true;
                }
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph => flush(&mut stack, &mut inl),
                TagEnd::Heading(_) => {
                    let level = heading.take().unwrap_or(2);
                    if let Some(top) = stack.last_mut() {
                        top.blocks.push(Block::Heading(level, std::mem::take(&mut inl)));
                    }
                }
                TagEnd::BlockQuote(_) => {
                    flush(&mut stack, &mut inl);
                    if stack.len() > 1
                        && let Some(f) = stack.pop()
                        && let Some(top) = stack.last_mut()
                    {
                        top.blocks.push(Block::Quote(f.blocks));
                    }
                }
                TagEnd::CodeBlock => {
                    if let (Some(code), Some(top)) = (code_block.take(), stack.last_mut()) {
                        top.blocks.push(Block::Code(code.trim_end_matches('\n').to_string()));
                    }
                }
                TagEnd::Item => {
                    flush(&mut stack, &mut inl);
                    if stack.len() > 1
                        && let Some(f) = stack.pop()
                        && let Some(Frame { kind: FrameKind::List(_, items), .. }) = stack.last_mut()
                    {
                        items.push(f.blocks);
                    }
                }
                TagEnd::List(_) => {
                    if stack.len() > 1
                        && let Some(Frame { kind: FrameKind::List(start, items), .. }) = stack.pop()
                        && let Some(top) = stack.last_mut()
                    {
                        top.blocks.push(Block::List(start, items));
                    }
                }
                TagEnd::TableCell => {
                    if let Some(t) = table.as_mut() {
                        t.row.push(std::mem::take(&mut inl));
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = table.as_mut() {
                        t.head = std::mem::take(&mut t.row);
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = table.as_mut() {
                        let row = std::mem::take(&mut t.row);
                        t.rows.push(row);
                    }
                }
                TagEnd::Table => {
                    if let (Some(t), Some(top)) = (table.take(), stack.last_mut()) {
                        top.blocks.push(Block::Table(t.head, t.rows));
                    }
                }
                TagEnd::Emphasis => {
                    italic = italic.saturating_sub(1);
                    fmt.italic = italic > 0;
                }
                TagEnd::Strong => {
                    bold = bold.saturating_sub(1);
                    fmt.bold = bold > 0;
                }
                TagEnd::Strikethrough => {
                    strike = strike.saturating_sub(1);
                    fmt.strike = strike > 0;
                }
                TagEnd::Link => {
                    link = link.saturating_sub(1);
                    fmt.link = link > 0;
                }
                _ => {}
            },
            Event::Text(s) => match code_block.as_mut() {
                Some(code) => code.push_str(&s),
                None => push_text(&mut inl, &s, fmt),
            },
            Event::Code(s) => inl.push(Inline::Text(s.to_string(), Fmt { code: true, ..fmt })),
            Event::InlineMath(s) => inl.push(Inline::Math(s.to_string())),
            Event::DisplayMath(s) => {
                // A display formula ends the text before it and stands on its own line.
                flush(&mut stack, &mut inl);
                if let Some(top) = stack.last_mut() {
                    top.blocks.push(Block::Math(s.to_string()));
                }
            }
            Event::Html(s) | Event::InlineHtml(s) => push_text(&mut inl, &s, fmt),
            Event::SoftBreak => inl.push(Inline::SoftBreak),
            Event::HardBreak => inl.push(Inline::HardBreak),
            Event::Rule => {
                flush(&mut stack, &mut inl);
                if let Some(top) = stack.last_mut() {
                    top.blocks.push(Block::Rule);
                }
            }
            Event::TaskListMarker(done) => push_text(&mut inl, if done { "\u{2611} " } else { "\u{2610} " }, fmt),
            _ => {}
        }
    }
    flush(&mut stack, &mut inl);
    stack.into_iter().next().map(|f| f.blocks).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Formulas

/// A typeset formula: SVG bytes and the size they were laid out at (pixels, at `MATH_EM_PX`).
#[derive(Clone)]
struct Formula {
    uri: String,
    svg: Arc<[u8]>,
    size: egui::Vec2,
}

type FormulaCache = Mutex<HashMap<(String, bool, [u8; 3]), Option<Formula>>>;

fn cache() -> &'static FormulaCache {
    static CACHE: OnceLock<FormulaCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn math_font() -> Option<&'static latex_rust::MathFont> {
    static FONT: OnceLock<Option<latex_rust::MathFont>> = OnceLock::new();
    FONT.get_or_init(|| latex_rust::MathFont::stix_two_math().ok()).as_ref()
}

/// The number in `attr="…"` of the SVG root, in points, as pixels.
fn svg_attr_px(svg: &str, attr: &str) -> Option<f32> {
    let from = svg.find("<svg")?;
    let head = svg.get(from..)?;
    let head = head.get(..head.find('>')?)?;
    let start = head.find(&format!(" {attr}=\""))? + attr.len() + 3;
    let rest = head.get(start..)?;
    let value = rest.get(..rest.find('"')?)?;
    let (num, scale) = match value.strip_suffix("pt") {
        Some(n) => (n, 96.0 / 72.0),
        None => (value.strip_suffix("px").unwrap_or(value), 1.0),
    };
    num.trim().parse::<f32>().ok().map(|v| v * scale)
}

fn typeset(latex: &str, display: bool, color: Color32) -> Option<Formula> {
    let latex = latex.trim();
    if latex.is_empty() || latex.chars().count() > MAX_FORMULA_CHARS {
        return None;
    }
    let key = (latex.to_string(), display, [color.r(), color.g(), color.b()]);
    let mut cache = cache().lock().ok()?;
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    if cache.len() >= MAX_CACHED_FORMULAS {
        cache.clear();
    }
    let formula = typeset_uncached(latex, display, color);
    cache.insert(key, formula.clone());
    formula
}

fn typeset_uncached(latex: &str, display: bool, color: Color32) -> Option<Formula> {
    let font = math_font()?;
    let mut opt = latex_rust::SvgOptions::new();
    opt.font_size_pt = latex_rust::Dim::from_i64(12);
    opt.color = latex_rust::Color::rgb(color.r(), color.g(), color.b());
    opt.display = display;
    // The renderer works on untrusted text; a panic inside it must not take the app down.
    let svg = std::panic::catch_unwind(|| latex_rust::latex_to_svg(latex, font, &opt)).ok()?.ok()?;
    let w = svg_attr_px(&svg, "width")?;
    let h = svg_attr_px(&svg, "height")?;
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 || w > 20_000.0 || h > 20_000.0 {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut hasher);
    Some(Formula { uri: format!("bytes://ai-math/{:016x}.svg", hasher.finish()), svg: Arc::from(svg.into_bytes()), size: egui::vec2(w, h) })
}

/// A formula as an image `scale` times its layout size, or `None` when it can't be typeset.
fn formula_image(latex: &str, display: bool, color: Color32, scale: f32) -> Option<egui::Image<'static>> {
    let f = typeset(latex, display, color)?;
    Some(egui::Image::from_bytes(f.uri, egui::load::Bytes::Shared(f.svg)).fit_to_exact_size(f.size * scale))
}

// ---------------------------------------------------------------------------------------------
// Drawing

/// Draw a model answer (Markdown with LaTeX) into `ui`. `salt` keeps ids of several answers apart.
pub(crate) fn show(ui: &mut egui::Ui, t: &Tokens, src: &str, salt: impl Hash + std::fmt::Debug) {
    let blocks = parse(src);
    ui.push_id(salt, |ui| {
        let mut n = 0usize;
        blocks_ui(ui, t, &blocks, &mut n);
    });
}

fn body_size(ui: &egui::Ui) -> f32 {
    egui::TextStyle::Body.resolve(ui.style()).size
}

fn blocks_ui(ui: &mut egui::Ui, t: &Tokens, blocks: &[Block], n: &mut usize) {
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            ui.add_space(6.0);
        }
        *n += 1;
        block_ui(ui, t, b, n);
    }
}

fn block_ui(ui: &mut egui::Ui, t: &Tokens, b: &Block, n: &mut usize) {
    let body = body_size(ui);
    match b {
        Block::Heading(level, inl) => {
            let size = match level {
                1 => body * 1.5,
                2 => body * 1.3,
                3 => body * 1.15,
                _ => body * 1.05,
            };
            inlines_ui(ui, t, inl, size, true);
        }
        Block::Para(inl) => inlines_ui(ui, t, inl, body, false),
        Block::Code(code) => {
            egui::Frame::new()
                .fill(t.field)
                .stroke(egui::Stroke::new(1.0, t.border))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(RichText::new(code).monospace().size(body * 0.92).color(t.text)).selectable(true).wrap());
                });
        }
        Block::Math(src) => {
            ui.vertical_centered(|ui| match formula_image(src, true, t.text, body * 1.35 / MATH_EM_PX) {
                Some(img) => {
                    let avail = ui.available_width();
                    let natural = img.size().unwrap_or_default();
                    let img = if natural.x > avail && natural.x > 0.0 { img.fit_to_exact_size(natural * (avail / natural.x)) } else { img };
                    ui.add(img);
                }
                None => {
                    ui.add(egui::Label::new(RichText::new(src.trim()).monospace().color(t.text_muted)).selectable(true).wrap());
                }
            });
        }
        Block::Quote(inner) => {
            let r = egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 0, top: 0, bottom: 0 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                blocks_ui(ui, t, inner, n);
            });
            let rect = r.response.rect;
            ui.painter().vline(rect.left() + 3.0, rect.y_range(), egui::Stroke::new(2.0, t.border));
        }
        Block::List(start, items) => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    ui.add_space(3.0);
                }
                ui.horizontal_top(|ui| {
                    ui.add_space(4.0);
                    let marker = match start {
                        Some(s) => format!("{}.", s.saturating_add(i as u64)),
                        None => "\u{2022}".to_string(),
                    };
                    ui.add_sized([body * 1.4, body], egui::Label::new(RichText::new(marker).color(t.text_muted)));
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        blocks_ui(ui, t, item, n);
                    });
                });
            }
        }
        Block::Rule => {
            ui.separator();
        }
        Block::Table(head, rows) => {
            egui::Grid::new(("table", *n)).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                for cell in head {
                    inlines_ui(ui, t, cell, body, true);
                }
                if !head.is_empty() {
                    ui.end_row();
                }
                for row in rows {
                    for cell in row {
                        inlines_ui(ui, t, cell, body, false);
                    }
                    ui.end_row();
                }
            });
        }
    }
}

fn inlines_ui(ui: &mut egui::Ui, t: &Tokens, inl: &[Inline], size: f32, strong: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for i in inl {
            match i {
                Inline::Text(s, f) => {
                    let mut rt = RichText::new(s.as_str()).size(size).color(t.text);
                    if f.bold || strong {
                        rt = rt.font(theme::semibold(size));
                    }
                    if f.italic {
                        rt = rt.italics();
                    }
                    if f.strike {
                        rt = rt.strikethrough();
                    }
                    if f.code {
                        rt = rt.monospace().size(size * 0.92).background_color(t.field);
                    }
                    if f.link {
                        rt = rt.color(t.accent).underline();
                    }
                    ui.add(egui::Label::new(rt).selectable(true));
                }
                Inline::Math(src) => match formula_image(src, false, t.text, size * 1.2 / MATH_EM_PX) {
                    Some(img) => {
                        ui.add_space(2.0);
                        ui.add(img);
                        ui.add_space(2.0);
                    }
                    None => {
                        ui.add(egui::Label::new(RichText::new(src.trim()).monospace().size(size * 0.92).color(t.text_muted)).selectable(true));
                    }
                },
                Inline::SoftBreak => {
                    ui.add(egui::Label::new(RichText::new(" ").size(size)));
                }
                Inline::HardBreak => ui.end_row(),
            }
        }
    });
}

/// The plain text of some inlines, for tests.
#[cfg(test)]
fn plain(inl: &[Inline]) -> String {
    inl.iter()
        .map(|i| match i {
            Inline::Text(s, _) => s.clone(),
            Inline::Math(m) => format!("${m}$"),
            Inline::SoftBreak | Inline::HardBreak => " ".into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_latex_delimiters_but_not_code() {
        assert_eq!(normalize_math(r"so \(x^2\) and \[y\]"), "so $x^2$ and $$y$$");
        assert_eq!(normalize_math("`\\(x\\)` ok"), "`\\(x\\)` ok");
        // A spaced line break inside a formula is not a delimiter.
        assert_eq!(normalize_math(r"\[a \\[4pt] b\]"), r"$$a \\[4pt] b$$");
        assert_eq!(normalize_math(r"\(a\)\\(b\)"), r"$a$\\(b$");
        assert_eq!(normalize_math("```\n\\(x\\)\n```\n\\(y\\)"), "```\n\\(x\\)\n```\n$y$");
    }

    #[test]
    fn parses_headings_lists_and_emphasis() {
        let b = parse("## Title\n\nSome **bold** text.\n\n- one\n- two\n\n1. a\n2. b\n");
        assert!(matches!(&b[0], Block::Heading(2, i) if plain(i) == "Title"));
        assert!(matches!(&b[1], Block::Para(i) if plain(i) == "Some bold text."));
        assert!(matches!(&b[2], Block::List(None, items) if items.len() == 2));
        assert!(matches!(&b[3], Block::List(Some(1), items) if items.len() == 2));
        let Block::Para(i) = &b[1] else { panic!() };
        assert!(i.iter().any(|x| matches!(x, Inline::Text(s, f) if s == "bold" && f.bold)));
    }

    #[test]
    fn parses_inline_and_display_math() {
        let b = parse("Energy is $E=mc^2$ here.\n\n$$\\int_0^1 x\\,dx = \\frac12$$\n");
        let Block::Para(i) = &b[0] else { panic!("{b:?}") };
        assert!(i.contains(&Inline::Math("E=mc^2".into())));
        assert!(matches!(&b[1], Block::Math(m) if m.contains("\\int")));
        let b = parse(r"Also \(a+b\) and \[c\]");
        assert!(matches!(&b[0], Block::Para(i) if i.contains(&Inline::Math("a+b".into()))));
        assert!(b.iter().any(|x| matches!(x, Block::Math(m) if m == "c")));
    }

    #[test]
    fn dollar_amounts_are_not_math() {
        let b = parse("It costs $5 and then $6 more.");
        let Block::Para(i) = &b[0] else { panic!() };
        assert!(!i.iter().any(|x| matches!(x, Inline::Math(_))), "{i:?}");
    }

    #[test]
    fn parses_tables_code_and_quotes() {
        let b = parse("| a | b |\n|---|---|\n| 1 | 2 |\n\n```\nlet x = 1;\n```\n\n> quoted\n");
        assert!(matches!(&b[0], Block::Table(h, r) if h.len() == 2 && r.len() == 1));
        assert!(matches!(&b[1], Block::Code(c) if c == "let x = 1;"));
        assert!(matches!(&b[2], Block::Quote(q) if q.len() == 1));
    }

    #[test]
    fn typesets_formulas_and_rejects_garbage() {
        let f = typeset(r"\frac{a}{b} + \sqrt{x^2+1}", false, Color32::BLACK).expect("typeset");
        assert!(f.size.x > 0.0 && f.size.y > 0.0);
        assert!(std::str::from_utf8(&f.svg).unwrap().starts_with("<svg") || std::str::from_utf8(&f.svg).unwrap().contains("<svg"));
        assert!(typeset(r"\notacommand{", false, Color32::BLACK).is_none());
        assert!(typeset("   ", true, Color32::BLACK).is_none());
        let deep = format!("{}x{}", "{".repeat(5_000), "}".repeat(5_000));
        let _ = typeset(&deep, true, Color32::BLACK); // must not crash
    }
}
