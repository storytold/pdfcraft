//! Template XML → [`model`](crate::model). Only what the layout engine uses is read; everything
//! else in the template is ignored, never rejected.

use base64::Engine;
use roxmltree::Node as XmlNode;

use crate::XfaError;
use crate::model::*;

/// Deepest element nesting followed (templates are shallow; hostile ones are not).
const MAX_DEPTH: usize = 64;
/// Most XML nodes a template may have.
const MAX_NODES: u32 = 2_000_000;
/// Largest decoded inline image.
const MAX_IMAGE: usize = 32 << 20;
/// Most items a choice list keeps.
const MAX_ITEMS: usize = 1000;
/// Longest text run kept.
const MAX_TEXT: usize = 1 << 20;

/// Parse a measurement such as `12.7mm`, `72pt`, `1in`, `2.54cm` into points. Bare numbers are
/// points. `None` for anything else (including `em`).
pub fn measure(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let v: f64 = num.trim().parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    let k = match unit.trim() {
        "" | "pt" => 1.0,
        "mm" => 72.0 / 25.4,
        "cm" => 72.0 / 2.54,
        "in" => 72.0,
        "px" => 0.75,
        "pc" => 12.0,
        _ => return None,
    };
    Some((v * k).clamp(-100_000.0, 100_000.0))
}

fn attr_measure(n: XmlNode, name: &str) -> Option<f64> {
    n.attribute(name).and_then(measure)
}

fn child<'a, 'd>(n: XmlNode<'a, 'd>, name: &str) -> Option<XmlNode<'a, 'd>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'd>(n: XmlNode<'a, 'd>, name: &'a str) -> impl Iterator<Item = XmlNode<'a, 'd>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// The text of an image element: base64 is read whole up to the image cap (`text_of` stops at
/// 1 MiB, which would cut a photo short).
fn image_text_of(n: XmlNode) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for d in n.descendants() {
        if d.is_text()
            && let Some(t) = d.text()
        {
            out.extend(t.bytes().filter(|b| !b.is_ascii_whitespace()));
            if out.len() > MAX_IMAGE / 3 * 4 + 4 {
                return None;
            }
        }
    }
    Some(out)
}

fn text_of(n: XmlNode) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        if d.is_text()
            && let Some(t) = d.text()
        {
            s.push_str(t);
            if s.len() > MAX_TEXT {
                s.truncate(s.char_indices().take_while(|(i, _)| *i < MAX_TEXT).last().map_or(0, |(i, c)| i + c.len_utf8()));
                break;
            }
        }
    }
    s
}

fn parse_margin(n: Option<XmlNode>) -> Margin {
    let Some(n) = n else { return Margin::default() };
    Margin {
        top: attr_measure(n, "topInset").unwrap_or(0.0),
        right: attr_measure(n, "rightInset").unwrap_or(0.0),
        bottom: attr_measure(n, "bottomInset").unwrap_or(0.0),
        left: attr_measure(n, "leftInset").unwrap_or(0.0),
    }
}

fn parse_color(n: Option<XmlNode>) -> Option<Color> {
    n.and_then(|c| c.attribute("value")).and_then(Color::parse)
}

fn parse_edge(n: XmlNode) -> Edge {
    Edge {
        presence: Presence::parse(n.attribute("presence")),
        thickness: attr_measure(n, "thickness").unwrap_or(0.5).max(0.0),
        color: parse_color(child(n, "color")).unwrap_or(Color::BLACK),
        stroke: match n.attribute("stroke") {
            Some("dashed" | "dashDot" | "dashDotDot") => Stroke::Dashed,
            Some("dotted") => Stroke::Dotted,
            Some("raised" | "lowered" | "etched" | "embossed") => Stroke::Relief,
            _ => Stroke::Solid,
        },
    }
}

/// `<fill>`: a colour, or white when the element is present without one.
fn parse_fill(n: Option<XmlNode>) -> Option<Color> {
    let n = n?;
    if Presence::parse(n.attribute("presence")) != Presence::Visible {
        return None;
    }
    Some(parse_color(child(n, "color")).unwrap_or(Color::WHITE))
}

/// `<border>`: edges in XFA order. One edge applies to all four; two: top/bottom then
/// left/right; three: top, left/right, bottom; four: top, right, bottom, left.
fn parse_border(n: Option<XmlNode>) -> Option<Border> {
    let n = n?;
    let edges: Vec<Edge> = children(n, "edge").map(parse_edge).collect();
    let d = Edge::default();
    let e = |i: usize| edges.get(i).copied().unwrap_or(d);
    let four = match edges.len() {
        0 => [d; 4],
        1 => [e(0); 4],
        2 => [e(0), e(1), e(0), e(1)],
        3 => [e(0), e(1), e(2), e(1)],
        _ => [e(0), e(1), e(2), e(3)],
    };
    Some(Border { presence: Presence::parse(n.attribute("presence")), edges: four, fill: parse_fill(child(n, "fill")) })
}

fn parse_font(n: Option<XmlNode>) -> FontSpec {
    let Some(n) = n else { return FontSpec::default() };
    FontSpec {
        typeface: n.attribute("typeface").map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        size: attr_measure(n, "size").filter(|s| *s > 0.0),
        bold: n.attribute("weight").map(|w| w == "bold"),
        italic: n.attribute("posture").map(|p| p == "italic"),
        underline: n.attribute("underline").map(|u| u != "0"),
        color: parse_color(child(n, "fill").and_then(|f| child(f, "color"))),
    }
}

fn parse_para(n: Option<XmlNode>) -> ParaSpec {
    let Some(n) = n else { return ParaSpec::default() };
    ParaSpec {
        h_align: HAlign::parse(n.attribute("hAlign")),
        v_align: VAlign::parse(n.attribute("vAlign")),
        line_height: attr_measure(n, "lineHeight").filter(|v| *v > 0.0),
        margin_left: attr_measure(n, "marginLeft"),
        margin_right: attr_measure(n, "marginRight"),
        space_above: attr_measure(n, "spaceAbove"),
        space_below: attr_measure(n, "spaceBelow"),
        text_indent: attr_measure(n, "textIndent"),
    }
}

fn parse_common(n: XmlNode) -> Common {
    Common {
        name: n.attribute("name").map(str::to_string).filter(|s| !s.is_empty()),
        id: n.attribute("id").map(str::to_string),
        x: attr_measure(n, "x"),
        y: attr_measure(n, "y"),
        w: attr_measure(n, "w"),
        h: attr_measure(n, "h"),
        min_w: attr_measure(n, "minW"),
        max_w: attr_measure(n, "maxW"),
        min_h: attr_measure(n, "minH"),
        max_h: attr_measure(n, "maxH"),
        presence: Presence::parse(n.attribute("presence")),
        margin: parse_margin(child(n, "margin")),
        border: parse_border(child(n, "border")),
        font: parse_font(child(n, "font")),
        para: parse_para(child(n, "para")),
        col_span: n.attribute("colSpan").and_then(|c| c.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 64),
    }
}

/// Rich text: XHTML `body > p > span…`. `br` starts a new paragraph; `xfa:embed` spans become
/// embedded fields.
fn parse_rich(body: XmlNode) -> RichText {
    #[derive(Clone, Default)]
    struct Style {
        bold: Option<bool>,
        italic: Option<bool>,
        underline: Option<bool>,
        size: Option<f64>,
    }
    fn styled(base: &Style, css: Option<&str>) -> Style {
        let mut s = base.clone();
        for decl in css.unwrap_or("").split(';') {
            let Some((k, v)) = decl.split_once(':') else { continue };
            let v = v.trim();
            match k.trim() {
                "font-weight" => s.bold = Some(v == "bold" || v.parse::<u32>().is_ok_and(|w| w >= 600)),
                "font-style" => s.italic = Some(v == "italic" || v == "oblique"),
                "text-decoration" => s.underline = Some(v.contains("underline")),
                "font-size" => s.size = measure(v).filter(|x| *x > 0.0),
                _ => {}
            }
        }
        s
    }
    // Paragraph boundaries from `p` are implicit (dropped when empty); `br` makes an explicit
    // line break that stays even when the line is blank.
    fn walk(n: XmlNode, style: &Style, out: &mut RichText, explicit: &mut Vec<bool>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for c in n.children() {
            if c.is_text() {
                if let Some(t) = c.text()
                    && !t.is_empty()
                {
                    push_run(
                        out,
                        explicit,
                        Run {
                            text: t.to_string(),
                            bold: style.bold,
                            italic: style.italic,
                            underline: style.underline,
                            size: style.size,
                            embed: None,
                        },
                    );
                }
                continue;
            }
            if !c.is_element() {
                continue;
            }
            match c.tag_name().name() {
                "p" => {
                    let align = c.attribute("style").and_then(|s| {
                        s.split(';')
                            .find_map(|d| d.split_once(':').filter(|(k, _)| k.trim() == "text-align").and_then(|(_, v)| HAlign::parse(Some(v))))
                    });
                    out.paragraphs.push(Paragraph { runs: Vec::new(), h_align: align });
                    explicit.push(false);
                    walk(c, &styled(style, c.attribute("style")), out, explicit, depth + 1);
                    // The next text starts a new paragraph.
                    out.paragraphs.push(Paragraph::default());
                    explicit.push(false);
                }
                "br" => {
                    out.paragraphs.push(Paragraph::default());
                    explicit.push(true);
                }
                "span" | "b" | "i" | "u" | "a" | "div" | "font" => {
                    let mut st = styled(style, c.attribute("style"));
                    match c.tag_name().name() {
                        "b" => st.bold = Some(true),
                        "i" => st.italic = Some(true),
                        "u" => st.underline = Some(true),
                        _ => {}
                    }
                    let embed = c.attributes().find(|a| a.name() == "embed").map(|a| a.value().trim_start_matches('#').to_string());
                    if let Some(e) = embed {
                        push_run(
                            out,
                            explicit,
                            Run { text: String::new(), bold: st.bold, italic: st.italic, underline: st.underline, size: st.size, embed: Some(e) },
                        );
                    }
                    walk(c, &st, out, explicit, depth + 1);
                }
                _ => walk(c, style, out, explicit, depth + 1),
            }
        }
    }
    fn push_run(out: &mut RichText, explicit: &mut Vec<bool>, run: Run) {
        if out.paragraphs.is_empty() {
            out.paragraphs.push(Paragraph::default());
            explicit.push(false);
        }
        if let Some(p) = out.paragraphs.last_mut() {
            p.runs.push(run);
        }
    }
    let mut out = RichText::default();
    let mut explicit = Vec::new();
    walk(body, &Style::default(), &mut out, &mut explicit, 0);
    // Drop the empty paragraphs `p` boundaries leave behind; blank lines from `br` stay.
    let mut cleaned: Vec<Paragraph> = Vec::new();
    for (i, p) in out.paragraphs.into_iter().enumerate() {
        let blank = p.runs.iter().all(|r| r.text.trim().is_empty() && r.embed.is_none());
        if blank && !explicit.get(i).copied().unwrap_or(false) {
            continue;
        }
        cleaned.push(p);
    }
    while cleaned.last().is_some_and(|p| p.runs.iter().all(|r| r.text.trim().is_empty() && r.embed.is_none())) {
        cleaned.pop();
    }
    RichText { paragraphs: cleaned }
}

/// `<value>`: text, rich text, an image, a line or a rectangle.
fn parse_value(n: Option<XmlNode>, warnings: &mut Vec<String>) -> Value {
    let Some(n) = n else { return Value::Empty };
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "text" => return Value::Text(text_of(c)),
            "exData" => {
                let html = c.attribute("contentType").is_some_and(|t| t.contains("html")) || child(c, "body").is_some();
                if html {
                    let body = child(c, "body").unwrap_or(c);
                    return Value::Rich(parse_rich(body));
                }
                return Value::Text(text_of(c));
            }
            "image" => {
                let Some(raw) = image_text_of(c) else {
                    warnings.push("an image is larger than 32 MB; it is left out".into());
                    return Value::Empty;
                };
                let data = if c.attribute("transferEncoding").is_none_or(|e| e == "base64") {
                    match base64::engine::general_purpose::STANDARD.decode(&raw) {
                        Ok(d) => d,
                        Err(_) => {
                            warnings.push("an image's base64 data could not be decoded; it is left out".into());
                            return Value::Empty;
                        }
                    }
                } else {
                    raw
                };
                if data.len() > MAX_IMAGE {
                    warnings.push("an image is larger than 32 MB; it is left out".into());
                    return Value::Empty;
                }
                return Value::Image { content_type: c.attribute("contentType").unwrap_or("").to_string(), data };
            }
            "line" => {
                return Value::Line { slope_up: c.attribute("slope") == Some("/"), edge: child(c, "edge").map(parse_edge).unwrap_or_default() };
            }
            "rectangle" => {
                let mut b = parse_border(Some(c)).unwrap_or_default();
                b.fill = parse_fill(child(c, "fill"));
                return Value::Rectangle(b);
            }
            "date" | "time" | "dateTime" | "integer" | "decimal" | "float" | "boolean" => return Value::Text(text_of(c)),
            _ => {}
        }
    }
    Value::Empty
}

fn parse_caption(n: Option<XmlNode>, warnings: &mut Vec<String>) -> Option<Caption> {
    let n = n?;
    Some(Caption {
        placement: match n.attribute("placement") {
            Some("right") => Placement::Right,
            Some("top") => Placement::Top,
            Some("bottom") => Placement::Bottom,
            Some("inline") => Placement::Inline,
            _ => Placement::Left,
        },
        reserve: attr_measure(n, "reserve").filter(|r| *r >= 0.0),
        presence: Presence::parse(n.attribute("presence")),
        value: parse_value(child(n, "value"), warnings),
        font: parse_font(child(n, "font")),
        para: parse_para(child(n, "para")),
    })
}

/// The field's item lists: what is shown (the first `<items>` that is not the saved values,
/// also a check button's on and off values) and, for choice lists, the values saved to the
/// data (`<items save="1">`, usually hidden), empty when the shown text is the value.
fn parse_items(n: XmlNode, warnings: &mut Vec<String>) -> (Vec<String>, Vec<String>) {
    let lists: Vec<XmlNode> = children(n, "items").collect();
    let is_saved = |i: &XmlNode| i.attribute("save") == Some("1") || i.attribute("presence") == Some("hidden");
    let mut over = false;
    let mut texts = |i: &XmlNode| -> Vec<String> {
        let all: Vec<String> = i.children().filter(|c| c.is_element()).map(text_of).take(MAX_ITEMS + 1).collect();
        if all.len() > MAX_ITEMS {
            over = true;
        }
        all.into_iter().take(MAX_ITEMS).collect()
    };
    let shown = lists.iter().find(|i| !is_saved(i)).or(lists.first());
    let saved = lists.iter().find(|i| is_saved(i) && shown.is_none_or(|s| s != *i));
    let shown: Vec<String> = shown.map(&mut texts).unwrap_or_default();
    let mut saved: Vec<String> = saved.map(&mut texts).unwrap_or_default();
    if over && warnings.len() < 100 {
        warnings.push(format!(
            "{} has more than {MAX_ITEMS} items; the rest are left out",
            n.attribute("name").map_or_else(|| "a choice list".to_string(), |name| format!("the choice list {name}"))
        ));
    }
    // A saved list shorter than the shown one is padded with the shown text.
    if !saved.is_empty() && saved.len() < shown.len() {
        saved.extend(shown.iter().skip(saved.len()).cloned());
    }
    (shown, saved)
}

fn parse_field(n: XmlNode, warnings: &mut Vec<String>) -> Field {
    let ui = child(n, "ui");
    let ui_el = ui.and_then(|u| u.children().find(|c| c.is_element() && c.tag_name().name() != "extras" && c.tag_name().name() != "picture"));
    let kind = match ui_el.map(|e| e.tag_name().name()) {
        None | Some("textEdit") => Ui::TextEdit,
        Some("numericEdit") => Ui::NumericEdit,
        Some("dateTimeEdit") => Ui::DateTimeEdit,
        Some("checkButton") => Ui::CheckButton,
        Some("choiceList") => Ui::ChoiceList,
        Some("button") => Ui::Button,
        Some("signature") => Ui::Signature,
        Some("imageEdit") => Ui::ImageEdit,
        Some("barcode") => Ui::Barcode,
        Some("passwordEdit") => Ui::PasswordEdit,
        Some(_) => Ui::Unknown,
    };
    let value_el = child(n, "value");
    let max_chars =
        value_el.and_then(|v| child(v, "text")).and_then(|t| t.attribute("maxChars")).and_then(|m| m.trim().parse::<usize>().ok()).filter(|m| *m > 0);
    let scripts = parse_events(n);
    let numeric = matches!(kind, Ui::NumericEdit)
        || value_el.is_some_and(|v| v.children().any(|c| c.is_element() && matches!(c.tag_name().name(), "decimal" | "integer" | "float")));
    let (items, item_values) = parse_items(n, warnings);
    let choice = match (kind == Ui::ChoiceList, ui_el) {
        (true, Some(e)) => {
            let open = e.attribute("open").unwrap_or("onEntry");
            ChoiceList {
                list_box: matches!(open, "always" | "multiSelect"),
                multi: open == "multiSelect",
                editable: e.attribute("textEntry").is_some_and(|t| t == "1" || t == "true"),
            }
        }
        _ => ChoiceList::default(),
    };
    let validate_el = child(n, "validate");
    let validate_message = validate_el
        .and_then(|v| child(v, "message"))
        .and_then(|m| children(m, "text").find(|t| t.attribute("name") == Some("scriptTest")).or_else(|| child(m, "text")))
        .map(text_of)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    Field {
        common: parse_common(n),
        ui: kind,
        ui_border: ui_el.and_then(|e| parse_border(child(e, "border"))),
        ui_margin: parse_margin(ui_el.and_then(|e| child(e, "margin"))),
        check_size: ui_el.and_then(|e| attr_measure(e, "size")).filter(|s| *s > 0.0),
        check_round: ui_el.is_some_and(|e| e.attribute("shape") == Some("round")),
        multiline: ui_el.is_some_and(|e| e.attribute("multiLine").is_some_and(|m| m == "1" || m == "true")),
        caption: parse_caption(child(n, "caption"), warnings),
        value: parse_value(value_el, warnings),
        items,
        item_values,
        choice,
        max_chars,
        tooltip: child(n, "assist").and_then(|a| child(a, "toolTip")).map(text_of).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        access: match n.attribute("access") {
            Some("readOnly") => Access::ReadOnly,
            Some("protected") => Access::Protected,
            Some("nonInteractive") => Access::NonInteractive,
            _ => Access::Open,
        },
        picture: ui.and_then(|u| child(u, "picture")).map(text_of).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
        scripts,
        calculate: child(n, "calculate").and_then(|c| child(c, "script")).map(|s| parse_script(s, "calculate")),
        validate: validate_el.and_then(|v| child(v, "script")).map(|s| parse_script(s, "validate")),
        validate_message,
        numeric,
    }
}

fn parse_script(s: XmlNode, activity: &str) -> Script {
    let ct = s.attribute("contentType").unwrap_or("");
    Script { activity: activity.to_string(), text: text_of(s).trim().to_string(), formcalc: !ct.to_ascii_lowercase().contains("javascript") }
}

/// `<event activity="…"><script>…</script></event>` children.
fn parse_events(n: XmlNode) -> Vec<Script> {
    let mut scripts = Vec::new();
    for ev in children(n, "event").take(64) {
        if let Some(s) = child(ev, "script") {
            let activity = ev.attribute("activity").unwrap_or("click");
            scripts.push(parse_script(s, activity));
        }
    }
    scripts
}

fn parse_occur(n: Option<XmlNode>) -> Occur {
    let Some(n) = n else { return Occur::default() };
    let int = |a: &str| n.attribute(a).and_then(|v| v.trim().parse::<i64>().ok());
    let min = int("min").unwrap_or(1).clamp(0, 1000) as usize;
    let max = match int("max") {
        Some(m) if m < 0 => None,
        Some(m) => Some((m.clamp(0, 1000) as usize).max(min)),
        None => Some(min.max(1)),
    };
    let initial = int("initial").map_or(min, |i| i.clamp(0, 1000) as usize).max(min);
    Occur { min, max, initial: max.map_or(initial, |m| initial.min(m)) }
}

fn parse_page_set(n: XmlNode, warnings: &mut Vec<String>, depth: usize) -> PageSet {
    let mut areas = Vec::new();
    for pa in children(n, "pageArea") {
        let medium = child(pa, "medium");
        let short = medium.and_then(|m| attr_measure(m, "short")).unwrap_or(612.0).clamp(1.0, 14_400.0);
        let long = medium.and_then(|m| attr_measure(m, "long")).unwrap_or(792.0).clamp(1.0, 14_400.0);
        let (width, height) = if medium.and_then(|m| m.attribute("orientation")) == Some("landscape") { (long, short) } else { (short, long) };
        let mut content: Vec<Rect> = children(pa, "contentArea")
            .map(|c| {
                Rect::new(
                    attr_measure(c, "x").unwrap_or(0.0),
                    attr_measure(c, "y").unwrap_or(0.0),
                    attr_measure(c, "w").unwrap_or(width).clamp(1.0, width),
                    attr_measure(c, "h").unwrap_or(height).clamp(1.0, height),
                )
            })
            .collect();
        if content.is_empty() {
            content.push(Rect::new(0.0, 0.0, width, height));
        }
        let items = pa.children().filter(|c| c.is_element()).filter_map(|c| parse_node(c, warnings, depth + 1)).collect();
        let occur = parse_occur(child(pa, "occur"));
        areas.push(PageArea { name: pa.attribute("name").map(str::to_string), width, height, content, items, occur_max: occur.max });
    }
    // Nested page sets: take their areas too.
    for ps in children(n, "pageSet") {
        if depth < MAX_DEPTH {
            areas.extend(parse_page_set(ps, warnings, depth + 1).areas);
        }
    }
    PageSet { areas }
}

fn parse_subform(n: XmlNode, warnings: &mut Vec<String>, depth: usize) -> Subform {
    let common = parse_common(n);
    let mut children_out = Vec::new();
    let mut page_set = None;
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "pageSet" => page_set = Some(parse_page_set(c, warnings, depth + 1)),
            _ => {
                if let Some(node) = parse_node(c, warnings, depth + 1) {
                    children_out.push(node);
                }
            }
        }
    }
    let column_widths: Vec<f64> =
        n.attribute("columnWidths").map(|s| s.split_whitespace().filter_map(measure).map(|w| w.max(0.0)).take(256).collect()).unwrap_or_default();
    let break_to_page = |name: &str| children(n, name).any(|b| b.attribute("targetType").is_none_or(|t| t == "pageArea" || t == "contentArea"));
    Subform {
        common,
        layout: Layout::parse(n.attribute("layout")),
        children: children_out,
        scripts: parse_events(n),
        page_set,
        occur: parse_occur(child(n, "occur")),
        break_before_page: break_to_page("breakBefore")
            || child(n, "break").is_some_and(|b| b.attribute("before").is_some_and(|v| v == "pageArea" || v == "contentArea")),
        break_after_page: break_to_page("breakAfter"),
        column_widths,
        overflow_leader: child(n, "overflow").and_then(|o| o.attribute("leader")).map(str::to_string).filter(|s| !s.is_empty()),
    }
}

fn parse_node(c: XmlNode, warnings: &mut Vec<String>, depth: usize) -> Option<Node> {
    if depth > MAX_DEPTH {
        return None;
    }
    Some(match c.tag_name().name() {
        "subform" => Node::Subform(Box::new(parse_subform(c, warnings, depth))),
        "subformSet" => {
            // A set of alternative subforms: lay out its members in order.
            let mut sf = parse_subform(c, warnings, depth);
            sf.layout = Layout::Tb;
            Node::Subform(Box::new(sf))
        }
        "area" => Node::Area(Box::new(Area {
            common: parse_common(c),
            children: c.children().filter(|k| k.is_element()).filter_map(|k| parse_node(k, warnings, depth + 1)).collect(),
        })),
        "field" => Node::Field(Box::new(parse_field(c, warnings))),
        "draw" => Node::Draw(Box::new(Draw { common: parse_common(c), value: parse_value(child(c, "value"), warnings) })),
        "exclGroup" => Node::ExclGroup(Box::new(ExclGroup {
            common: parse_common(c),
            layout: Layout::parse(c.attribute("layout")),
            fields: children(c, "field").map(|f| parse_field(f, warnings)).collect(),
            tooltip: child(c, "assist").and_then(|a| child(a, "toolTip")).map(text_of).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
            scripts: parse_events(c),
        })),
        _ => return None,
    })
}

/// Which page-number role a field with a layout:ready script plays, if any.
fn page_role(f: &Field) -> Option<PageRole> {
    let script = f.scripts.iter().find(|s| s.activity == "ready" || s.activity == "layoutReady")?;
    if script.text.contains("pageCount") {
        Some(PageRole::Count)
    } else if script.text.contains("layout.page(") || script.text.contains("layout.absPage(") {
        Some(PageRole::Number)
    } else {
        None
    }
}

fn collect_page_roles(nodes: &[Node], out: &mut Vec<(String, PageRole)>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for n in nodes {
        match n {
            Node::Field(f) => {
                if let (Some(id), Some(role)) = (&f.common.id, page_role(f)) {
                    out.push((id.clone(), role));
                }
            }
            Node::Subform(s) => {
                collect_page_roles(&s.children, out, depth + 1);
                if let Some(ps) = &s.page_set {
                    for a in &ps.areas {
                        collect_page_roles(&a.items, out, depth + 1);
                    }
                }
            }
            Node::Area(a) => collect_page_roles(&a.children, out, depth + 1),
            Node::ExclGroup(_) | Node::Draw(_) => {}
        }
    }
}

/// Parse a template packet. Returns the template and the warnings met on the way.
pub fn parse(xml: &str) -> Result<(Template, Vec<String>), XfaError> {
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    let doc = roxmltree::Document::parse_with_options(xml, opts).map_err(|e| XfaError::Malformed(format!("template XML: {e}")))?;
    let root = doc.root_element();
    // The XDP's config packet has a <template> element too: the one we want is in the
    // xfa-template namespace, or at least has a subform in it.
    let is_template = |n: &XmlNode| {
        n.is_element()
            && n.tag_name().name() == "template"
            && (n.tag_name().namespace().is_some_and(|ns| ns.contains("xfa-template")) || children(*n, "subform").next().is_some())
    };
    let template = if is_template(&root) {
        root
    } else {
        root.descendants().find(is_template).ok_or_else(|| XfaError::Malformed("no <template> element".into()))?
    };
    let mut warnings = Vec::new();
    let sf = children(template, "subform").next().ok_or_else(|| XfaError::Malformed("the template has no root subform".into()))?;
    let mut root_sf = parse_subform(sf, &mut warnings, 0);
    if root_sf.page_set.is_none() {
        // A page set may sit beside the root subform.
        root_sf.page_set = child(template, "pageSet").map(|ps| parse_page_set(ps, &mut warnings, 0));
    }
    let mut page_roles = Vec::new();
    collect_page_roles(&[Node::Subform(Box::new(root_sf.clone()))], &mut page_roles, 0);
    Ok((Template { root: root_sf, page_roles }, warnings))
}
