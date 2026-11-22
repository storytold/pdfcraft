//! Formatting inside a comment's body (#3737, #5929): bold, italic, underline, superscript,
//! subscript, text colour and font size on stretches of the text a comment's pop-up and the
//! Comments panel show. The plain text stays in `/Contents`, which every viewer reads; the
//! formatting goes in `/RC`, the rich-text string of ISO 32000-2 §12.7.4.3 (an XHTML subset), and
//! is read back from it, so it survives saving and reopening.
//!
//! Text boxes (`/FreeText`) keep their own `/DS` and `/RC` ([`super::modify`]) and are refused.

use super::lookup::{annot_dict, annot_ref, annots, is_comment_dict, page_ref};
use super::modify::touch;
use super::types::{Meta, Rgb};
use super::{AnnotError, Dict, Document, Object, PdfString};

/// The most runs one body takes.
pub const MAX_RUNS: usize = 2_000;
/// The most characters one body takes.
pub const MAX_CHARS: usize = 100_000;
/// `/RC` read past this many bytes is not read (it bounds the work of a hostile file).
const MAX_RC_BYTES: usize = 1 << 20;
/// The font size range a run may have, in points.
pub const SIZE_RANGE: std::ops::RangeInclusive<f64> = 1.0..=400.0;

/// Raised or lowered text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Script {
    #[default]
    Normal,
    Super,
    Sub,
}

/// A stretch of a comment's text with one formatting. Colour and size are `None` where the
/// comment's own apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub script: Script,
    pub color: Option<Rgb>,
    pub size: Option<f64>,
}

impl Run {
    pub fn plain(text: &str) -> Run {
        Run { text: text.to_string(), ..Run::default() }
    }

    fn same_look(&self, other: &Run) -> bool {
        self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.script == other.script
            && self.color == other.color
            && self.size == other.size
    }
}

/// Give a comment the formatted body `runs`: `/Contents` becomes their text, `/RC` their look.
/// Refused for a text box, and for a body that is too long or has a size outside `SIZE_RANGE`
/// (the comment is left as it is).
pub fn set_rich_contents(doc: &mut Document, page: usize, index: usize, runs: &[Run], meta: &Meta) -> Result<(), AnnotError> {
    let (_, r) = annot_ref(doc, page, index)?;
    if annot_dict(doc, r).name(b"Subtype") == Some(b"FreeText") {
        return Err(AnnotError::Unsupported("text box".into()));
    }
    let rc = rich_string(runs)?;
    let plain: String = runs.iter().map(|run| run.text.as_str()).collect();
    doc.update_dict(r, |d| {
        d.set(b"Contents".to_vec(), PdfString::text(&plain));
        d.set(b"RC".to_vec(), PdfString::text(&rc));
        touch(d, meta);
    })?;
    Ok(())
}

/// The formatted body of the comment at `(page, index)`: the runs of its `/RC`, or `None` when it
/// has no rich text, or its rich text does not give the plain text in `/Contents` (another
/// program changed one of them).
pub fn rich_contents(doc: &Document, page: usize, index: usize) -> Result<Option<Vec<Run>>, AnnotError> {
    let p = page_ref(doc, page)?;
    let entry = annots(doc, p).get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let obj = doc.resolve(&entry);
    let d = obj.as_dict().filter(|d| is_comment_dict(d)).ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    Ok(runs_of(d))
}

fn runs_of(d: &Dict) -> Option<Vec<Run>> {
    let text_of = |key: &[u8]| d.get(key).and_then(Object::as_string).map(PdfString::to_text);
    let rc = text_of(b"RC")?;
    let runs = parse_rich(&rc);
    let plain: String = runs.iter().map(|run| run.text.as_str()).collect();
    let contents = text_of(b"Contents").unwrap_or_default();
    let same = |s: &str| s.replace("\r\n", "\n").replace('\r', "\n").trim_end().to_string();
    (!runs.is_empty() && same(&plain) == same(&contents)).then_some(runs)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn hex(c: Rgb) -> String {
    // Clamped to 0..=1 first, so the product is 0..=255 and the cast exact.
    let [r, g, b] = c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn rich_string(runs: &[Run]) -> Result<String, AnnotError> {
    if runs.len() > MAX_RUNS {
        return Err(AnnotError::Invalid(format!("a comment takes at most {MAX_RUNS} formatted runs; {} were given", runs.len())));
    }
    let chars: usize = runs.iter().map(|run| run.text.chars().count()).sum();
    if chars > MAX_CHARS {
        return Err(AnnotError::Invalid(format!("a comment takes at most {MAX_CHARS} characters; {chars} were given")));
    }
    if let Some(size) = runs.iter().filter_map(|run| run.size).find(|s| !s.is_finite() || !SIZE_RANGE.contains(s)) {
        return Err(AnnotError::Invalid(format!("a run's font size must be {}–{} pt, not {size}", SIZE_RANGE.start(), SIZE_RANGE.end())));
    }
    let mut body = String::from("<p dir=\"ltr\">");
    for run in runs {
        let mut style = Vec::new();
        if run.bold {
            style.push("font-weight:bold".to_string());
        }
        if run.italic {
            style.push("font-style:italic".to_string());
        }
        if run.underline {
            style.push("text-decoration:underline".to_string());
        }
        if let Some(color) = run.color {
            style.push(format!("color:{}", hex(color)));
        }
        if let Some(size) = run.size {
            style.push(format!("font-size:{}pt", super::n(size)));
        }
        let (open, close) = match run.script {
            Script::Normal => ("", ""),
            Script::Super => ("<sup>", "</sup>"),
            Script::Sub => ("<sub>", "</sub>"),
        };
        for (i, line) in run.text.replace("\r\n", "\n").replace('\r', "\n").split('\n').enumerate() {
            if i > 0 {
                body.push_str("</p><p dir=\"ltr\">");
            }
            if line.is_empty() {
                continue;
            }
            let text = escape(line);
            if style.is_empty() {
                body.push_str(&format!("{open}{text}{close}"));
            } else {
                body.push_str(&format!("{open}<span style=\"{}\">{text}</span>{close}", style.join(";")));
            }
        }
    }
    body.push_str("</p>");
    let out = format!(
        "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xfa:spec=\"2.0.2\">{body}</body>"
    );
    if out.len() > MAX_RC_BYTES {
        return Err(AnnotError::Invalid(format!("a comment's rich text takes at most {MAX_RC_BYTES} bytes; the formatted body needs {}", out.len())));
    }
    Ok(out)
}

// ── reading ─────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Look {
    bold: bool,
    italic: bool,
    underline: bool,
    script: Script,
    color: Option<Rgb>,
    size: Option<f64>,
}

impl Look {
    fn run(&self, text: String) -> Run {
        Run { text, bold: self.bold, italic: self.italic, underline: self.underline, script: self.script, color: self.color, size: self.size }
    }

    fn apply_tag(&mut self, tag: &str, style: Option<&str>) {
        match tag {
            "b" | "strong" => self.bold = true,
            "i" | "em" => self.italic = true,
            "u" => self.underline = true,
            "sup" => self.script = Script::Super,
            "sub" => self.script = Script::Sub,
            _ => {}
        }
        for decl in style.unwrap_or_default().split(';') {
            let Some((prop, value)) = decl.split_once(':') else { continue };
            let value = value.trim().to_ascii_lowercase();
            match prop.trim().to_ascii_lowercase().as_str() {
                "font-weight" => self.bold = value == "bold" || value.parse::<u32>().is_ok_and(|w| w >= 600),
                "font-style" => self.italic = value == "italic" || value == "oblique",
                "text-decoration" => self.underline = value.contains("underline"),
                "vertical-align" => {
                    self.script = match value.as_str() {
                        "super" => Script::Super,
                        "sub" => Script::Sub,
                        _ => Script::Normal,
                    }
                }
                "color" => {
                    if let Some(c) = css_hex(&value) {
                        self.color = Some(c);
                    }
                }
                "font-size" => {
                    if let Some(size) = value.strip_suffix("pt").and_then(|v| v.trim().parse::<f64>().ok()).filter(|s| SIZE_RANGE.contains(s)) {
                        self.size = Some(size);
                    }
                }
                _ => {}
            }
        }
    }
}

fn css_hex(value: &str) -> Option<Rgb> {
    let digits = value.strip_prefix('#')?;
    let byte = |s: Option<&str>| s.and_then(|s| u8::from_str_radix(s, 16).ok()).map(|v| f64::from(v) / 255.0);
    match digits.len() {
        6 => Some([byte(digits.get(0..2))?, byte(digits.get(2..4))?, byte(digits.get(4..6))?]),
        3 => {
            let one = |at: usize| byte(digits.get(at..at + 1).map(|d| format!("{d}{d}")).as_deref());
            Some([one(0)?, one(1)?, one(2)?])
        }
        _ => None,
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let after = rest.get(at + 1..).unwrap_or_default();
        // Only the supported entity length is searched, so a long run of bare `&` stays linear.
        let prefix = after.get(..after.len().min(11)).unwrap_or_default();
        let entity = prefix.find(';').and_then(|end| after.get(..end).map(|name| (name, end)));
        let decoded = entity.and_then(|(name, _)| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                let number = name.strip_prefix('#')?;
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(h) => u32::from_str_radix(h, 16).ok()?,
                    None => number.parse().ok()?,
                };
                char::from_u32(code)
            }
        });
        match (decoded, entity) {
            (Some(c), Some((_, end))) => {
                out.push(c);
                rest = after.get(end + 1..).unwrap_or_default();
            }
            _ => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The attribute `name` of a tag's text (`span style="…"`).
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let rest = tag.get(at..)?;
    rest.get(..rest.find('"')?)
}

struct ParseOut {
    runs: Vec<Run>,
    pending_breaks: usize,
    chars: usize,
    over: bool,
}

impl ParseOut {
    fn take(&mut self, n: usize) -> bool {
        if self.over || self.chars.saturating_add(n) > MAX_CHARS {
            self.over = true;
            false
        } else {
            self.chars += n;
            true
        }
    }

    fn newline(&mut self) {
        if self.runs.is_empty() {
            self.pending_breaks = self.pending_breaks.saturating_add(1);
        } else if self.take(1)
            && let Some(last) = self.runs.last_mut()
        {
            last.text.push('\n');
        }
    }

    fn push(&mut self, look: &Look, text: String) {
        if self.over || text.is_empty() {
            return;
        }
        if self.runs.len() >= MAX_RUNS {
            self.over = true;
            return;
        }
        let lead = if self.runs.is_empty() { self.pending_breaks } else { 0 };
        let add = text.chars().count();
        if !self.take(lead.saturating_add(add)) {
            return;
        }
        let text = if lead > 0 {
            self.pending_breaks = 0;
            format!("{}{text}", "\n".repeat(lead))
        } else {
            text
        };
        let run = look.run(text);
        match self.runs.last_mut() {
            Some(last) if last.same_look(&run) => last.text.push_str(&run.text),
            _ => self.runs.push(run),
        }
    }
}

/// The runs of an XHTML rich-text string. Unknown tags are skipped, their text kept.
/// Over-budget input yields no runs, so a matching `/Contents` still refuses the body.
fn parse_rich(rc: &str) -> Vec<Run> {
    let end = rc.char_indices().map(|(i, _)| i).find(|i| *i >= MAX_RC_BYTES).unwrap_or(rc.len());
    let mut rest = rc.get(..end).unwrap_or_default();
    let mut stack = vec![Look::default()];
    let mut out = ParseOut { runs: Vec::new(), pending_breaks: 0, chars: 0, over: false };
    let mut break_before = false;
    while !rest.is_empty() {
        if out.over {
            break;
        }
        let look = stack.last().cloned().unwrap_or_default();
        let Some(open) = rest.find('<') else {
            out.push(&look, unescape(rest));
            break;
        };
        let text = unescape(rest.get(..open).unwrap_or_default());
        // Indentation between tags is not text.
        if !(text.trim().is_empty() && text.contains('\n')) {
            out.push(&look, text);
        }
        let after = rest.get(open + 1..).unwrap_or_default();
        let Some(close) = after.find('>') else { break };
        let tag = after.get(..close).unwrap_or_default();
        rest = after.get(close + 1..).unwrap_or_default();
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default().to_ascii_lowercase();
        let name = name.rsplit(':').next().unwrap_or_default().to_string();
        if tag.starts_with('/') {
            if name == "p" {
                break_before = true;
            }
            if stack.len() > 1 {
                stack.pop();
            }
            continue;
        }
        if name == "br" {
            out.newline();
            continue;
        }
        if name == "p" && break_before {
            break_before = false;
            out.newline();
        }
        if tag.ends_with('/') {
            continue;
        }
        let mut next = look;
        next.apply_tag(&name, attribute(tag, "style"));
        stack.push(next);
    }
    if out.over {
        return Vec::new();
    }
    out.runs
}
