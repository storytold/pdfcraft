//! The CSS a book's layout needs, and no more: style sheets and `style=""` with type, class, id
//! and universal selectors joined by descendant and child combinators, `!important`, specificity
//! and source order, and `@font-face`. Rules whose selectors use anything else (attributes,
//! pseudo-classes, sibling combinators) are skipped, as are `@media` blocks (books use them for
//! particular reading systems) and other at-rules. Values are interpreted in `layout`.

/// One simple selector: `p`, `.note`, `#c1`, `p.note`, `*`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
}

/// A selector: compounds from the outermost, each joined to the next by a descendant (`false`) or
/// child (`true`) combinator.
#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub parts: Vec<(Compound, bool)>,
    pub specificity: (u32, u32, u32),
}

/// A declaration: property (lowercase), value, `!important`.
pub type Declaration = (String, String, bool);

#[derive(Clone, Debug)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
}

/// `@font-face`: a family the book names, and where its file is (container paths).
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    pub family: String,
    pub sources: Vec<String>,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub rules: Vec<Rule>,
    pub faces: Vec<FontFace>,
}

/// An element as selectors see it.
#[derive(Clone, Debug, Default)]
pub struct Element {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = rest[i + 2..].find("*/").map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    out.push_str(rest);
    out
}

/// The text up to the `}` that closes the block opened just before `text`.
fn block_end(text: &str) -> usize {
    let mut depth = 1usize;
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// `a: b; c: d !important` as declarations; unparsable ones are dropped.
pub fn declarations(text: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    for part in text.split(';') {
        let Some((prop, value)) = part.split_once(':') else { continue };
        let prop = prop.trim().to_ascii_lowercase();
        let mut value = value.trim().to_string();
        let important = value.to_ascii_lowercase().ends_with("!important");
        if important {
            value.truncate(value.len() - "!important".len());
            value = value.trim_end().to_string();
        }
        if !prop.is_empty() && !value.is_empty() {
            out.push((prop, value, important));
        }
    }
    out
}

fn compound(text: &str) -> Option<Compound> {
    let mut c = Compound::default();
    let mut rest = text;
    if rest.is_empty() {
        return None;
    }
    let ident_end = |s: &str| s.find(['.', '#', '[', ':']).unwrap_or(s.len());
    if !rest.starts_with(['.', '#']) {
        let end = ident_end(rest);
        let tag = &rest[..end];
        if tag != "*" {
            if tag.is_empty() || !tag.chars().all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_') {
                return None;
            }
            // A namespaced type selector (`epub|type`) is not supported.
            c.tag = Some(tag.to_ascii_lowercase());
        }
        rest = &rest[end..];
    }
    while !rest.is_empty() {
        let kind = rest.chars().next()?;
        let body = &rest[1..];
        let end = ident_end(body);
        let name = &body[..end];
        if name.is_empty() {
            return None;
        }
        match kind {
            '.' => c.classes.push(name.to_string()),
            '#' => c.id = Some(name.to_string()),
            _ => return None,
        }
        rest = &body[end..];
    }
    Some(c)
}

/// One selector of a list, or `None` when it uses anything not supported.
pub fn selector(text: &str) -> Option<Selector> {
    let spaced = text.replace('>', " > ");
    let mut parts: Vec<(Compound, bool)> = Vec::new();
    let mut child = false;
    for token in spaced.split_whitespace() {
        if token == ">" {
            if parts.is_empty() || child {
                return None;
            }
            child = true;
            continue;
        }
        if token.contains(['+', '~', '[', ':', '|']) {
            return None;
        }
        parts.push((compound(token)?, child));
        child = false;
    }
    if parts.is_empty() || child {
        return None;
    }
    let mut spec = (0, 0, 0);
    for (c, _) in &parts {
        spec.0 += u32::from(c.id.is_some());
        spec.1 += c.classes.len() as u32;
        spec.2 += u32::from(c.tag.is_some());
    }
    Some(Selector { parts, specificity: spec })
}

/// Parse a style sheet whose `url()`s are relative to `base` (a container path).
pub fn parse(text: &str, base: &str) -> Sheet {
    let text = strip_comments(text);
    let mut sheet = Sheet::default();
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let head = rest[..open].trim();
        let body_start = open + 1;
        let end = body_start + block_end(&rest[body_start..]);
        let body = &rest[body_start..end.min(rest.len())];
        rest = rest.get(end + 1..).unwrap_or("");
        // Statements without a block before this one (`@import …;`, `@charset …;`).
        let head = head.rsplit(';').next().unwrap_or(head).trim();
        if let Some(at) = head.strip_prefix('@') {
            if at.to_ascii_lowercase().starts_with("font-face") {
                let decls = declarations(body);
                let get = |p: &str| decls.iter().find(|d| d.0 == p).map(|d| d.1.clone());
                let Some(family) = get("font-family").map(|f| unquote(&f)) else { continue };
                let sources = get("src").map(|s| urls(&s, base)).unwrap_or_default();
                let weight = get("font-weight").unwrap_or_default();
                let bold = weight == "bold" || weight.parse::<u32>().is_ok_and(|w| w >= 600);
                let italic = get("font-style").is_some_and(|s| s == "italic" || s == "oblique");
                sheet.faces.push(FontFace { family, sources, bold, italic });
            }
            continue;
        }
        let selectors: Vec<Selector> = head.split(',').filter_map(|s| selector(s.trim())).collect();
        if selectors.is_empty() {
            continue;
        }
        sheet.rules.push(Rule { selectors, declarations: declarations(body) });
    }
    sheet
}

/// `"Name"` or `'Name'` or `Name` → `Name`.
pub fn unquote(s: &str) -> String {
    s.trim().trim_matches(['"', '\'']).trim().to_string()
}

/// The `url(...)`s in a `src` value, resolved; formats other than TrueType and OpenType skipped
/// when a `format()` says so.
fn urls(value: &str, base: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in value.split(',') {
        let lower = part.to_ascii_lowercase();
        if lower.contains("format(") && !(lower.contains("truetype") || lower.contains("opentype")) {
            continue;
        }
        let Some(start) = lower.find("url(") else { continue };
        let inner = &part[start + 4..];
        let Some(end) = inner.find(')') else { continue };
        if let Some(path) = super::package::resolve(base, &unquote(&inner[..end])) {
            out.push(path);
        }
    }
    out
}

fn compound_matches(c: &Compound, e: &Element) -> bool {
    c.tag.as_ref().is_none_or(|t| *t == e.tag)
        && c.id.as_ref().is_none_or(|id| e.id.as_ref() == Some(id))
        && c.classes.iter().all(|k| e.classes.contains(k))
}

/// Whether `sel` matches the last element of `path` (the element and its ancestors, outermost
/// first).
pub fn matches(sel: &Selector, path: &[Element]) -> bool {
    fn go(parts: &[(Compound, bool)], path: &[Element]) -> bool {
        let Some(((last, child), outer)) = parts.split_last() else { return true };
        let Some((element, ancestors)) = path.split_last() else { return false };
        if !compound_matches(last, element) {
            return false;
        }
        if outer.is_empty() {
            return true;
        }
        if *child { go(outer, ancestors) } else { (0..ancestors.len()).rev().any(|i| go(outer, &ancestors[..=i])) }
    }
    go(&sel.parts, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(tag: &str, classes: &[&str]) -> Element {
        Element { tag: tag.into(), id: None, classes: classes.iter().map(|c| c.to_string()).collect() }
    }

    #[test]
    fn rules_selectors_and_font_faces_are_read() {
        let sheet = parse(
            "@charset \"utf-8\";/* c */ p.note, h1 > span { font-size: 1.2em; color: #c00 !important }\n\
             @media amzn-kf8 { p { color: red } }\n\
             a:hover { color: blue } div p { margin: 0 }\n\
             @font-face { font-family: 'Book Serif'; src: url(../fonts/serif.otf) format('opentype'), url(x.woff) format('woff'); font-weight: 700 }",
            "OEBPS/css/style.css",
        );
        assert_eq!(sheet.rules.len(), 2, "@media and the :hover rule are skipped");
        assert_eq!(sheet.rules[0].selectors.len(), 2);
        assert_eq!(sheet.rules[0].declarations[1], ("color".into(), "#c00".into(), true));
        assert_eq!(sheet.faces, [FontFace { family: "Book Serif".into(), sources: vec!["OEBPS/fonts/serif.otf".into()], bold: true, italic: false }]);
    }

    #[test]
    fn descendant_and_child_selectors_match() {
        let path = [el("body", &[]), el("div", &["box"]), el("p", &["note"])];
        let m = |s: &str| matches(&selector(s).unwrap(), &path);
        assert!(m("p") && m(".note") && m("div p") && m("body p") && m("div > p") && m("body .box > .note") && m("*"));
        assert!(!m("body > p") && !m("span") && !m("p.other") && !m("div.other p"));
        assert_eq!(selector("#a .b p").unwrap().specificity, (1, 1, 1));
        assert!(selector("p + p").is_none() && selector("a[href]").is_none() && selector("> p").is_none());
    }
}
