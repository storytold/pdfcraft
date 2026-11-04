use super::as_docx::rgb8;
use super::as_html::allowed;
use super::layout::{Block, Item, Page, count_lookup_steps, items};
use std::collections::HashMap;

fn rtf_text(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '\\' | '{' | '}' => {
                o.push('\\');
                o.push(c);
            }
            c if (c as u32) < 128 => o.push(c),
            c => {
                // \uN with a ? fallback; UTF-16 units as signed 16-bit numbers.
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    o.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    o
}

/// Rich Text Format: paragraphs with their sizes and bold/italic, real table rows, page breaks
/// between pages (images are left out).
pub fn rtf(pages: &[Page]) -> String {
    let items = items(pages);
    // The colour table: entry 0 is the default (auto) colour, then each non-black colour once.
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut slot: HashMap<[u8; 3], usize> = HashMap::new();
    let mut note = |c: [f64; 3]| {
        let c = rgb8(c);
        count_lookup_steps(1);
        if c != [0, 0, 0] && !slot.contains_key(&c) {
            slot.insert(c, colors.len());
            colors.push(c);
        }
    };
    for it in &items {
        match it {
            Item::Para(runs, _) => runs.iter().for_each(|r| note(r.block.color)),
            Item::Table(t) => t.rows.iter().flatten().for_each(|c| note(c.color)),
            Item::Img(_) | Item::PageBreak => {}
        }
    }
    // `\cfN` for a colour (nothing for black, which keeps the default).
    let cf = |c: [f64; 3]| {
        let c = rgb8(c);
        count_lookup_steps(1);
        slot.get(&c).map_or(String::new(), |i| format!("\\cf{}", i + 1))
    };
    let mut s = String::from("{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Helvetica;}}\n");
    if !colors.is_empty() {
        s.push_str("{\\colortbl;");
        for [r, g, b] in &colors {
            s.push_str(&format!("\\red{r}\\green{g}\\blue{b};"));
        }
        s.push_str("}\n");
    }
    for it in items {
        match it {
            Item::Para(runs, _) => {
                // The first run's formatting is the paragraph's; each later run is a group that
                // turns on or off what differs from it.
                let fmt = |b: &Block, later: bool| {
                    let mut fmt = format!("\\fs{}", (b.size * 2.0).round() as i64);
                    if b.bold {
                        fmt.push_str("\\b");
                    } else if later {
                        fmt.push_str("\\b0");
                    }
                    if b.italic {
                        fmt.push_str("\\i");
                    } else if later {
                        fmt.push_str("\\i0");
                    }
                    let colour = cf(b.color);
                    fmt.push_str(if colour.is_empty() && later { "\\cf0" } else { &colour });
                    fmt
                };
                let mut runs = runs.iter();
                let Some(first) = runs.next() else { continue };
                s.push_str(&format!("{{\\pard{} {}", fmt(first.block, false), rtf_text(&first.text)));
                for r in runs {
                    s.push_str(&format!("{{{} {}}}", fmt(r.block, true), rtf_text(&r.text)));
                }
                s.push_str("\\par}\n");
            }
            Item::Table(t) => {
                let mut edges = t.cols.clone();
                edges.push(t.rect[2]);
                let cellx: Vec<i64> = edges.iter().skip(1).map(|e| (e * 20.0).round() as i64).collect();
                for row in &t.rows {
                    s.push_str("\\trowd\\trgaph108");
                    // The row's cell boundaries come first: each cell ends at the right edge of
                    // the last grid column it spans.
                    let mut column = 0;
                    for c in row {
                        column += c.span.max(1);
                        if let Some(x) = cellx.get(column.min(cellx.len()).saturating_sub(1)) {
                            s.push_str(&format!("\\cellx{x}"));
                        }
                    }
                    for c in row {
                        let mut fmt = format!("\\intbl\\fs{}", (c.size * 2.0).round() as i64);
                        if c.bold {
                            fmt.push_str("\\b");
                        }
                        if c.italic {
                            fmt.push_str("\\i");
                        }
                        fmt.push_str(&cf(c.color));
                        s.push_str(&format!("{{{fmt} {}}}\\cell", rtf_text(&c.text)));
                    }
                    s.push_str("\\row\n");
                }
            }
            Item::Img(_) => {}
            Item::PageBreak => s.push_str("\\page\n"),
        }
    }
    s.push('}');
    s
}
