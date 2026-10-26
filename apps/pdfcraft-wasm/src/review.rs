//! Reading a page and marking it up: find text, select it between two points, highlight it, follow
//! and add links, list annotations and form fields, fill a field. Boxes and points are the displayed
//! page's points from its top-left, y down (the same space as `objects` and `text_blocks`).

use pdfcraft_engine::{Edit, FieldValue, FillMark, LineEnding, LinkAction, LinkStyle, Markup, NewAnnotation, NoteIcon, Shape, StampKind, Style};
use pdfcraft_render::{FieldKind, LinkTarget};
use serde_json::{Value, json};

use crate::core::Document;

const MAX_MATCHES: usize = 1000;
const MAX_RECTS: usize = 2000;

fn finite_box(b: [f64; 4]) -> Result<[f64; 4], String> {
    if b.iter().all(|v| v.is_finite() && v.abs() < 1e7) && b[2] > b[0] && b[3] > b[1] {
        Ok(b)
    } else {
        Err("a box is [x0, y0, x1, y1] with x0 < x1 and y0 < y1".into())
    }
}

fn rect_json(r: [f32; 4]) -> Value {
    json!([r[0], r[1], r[2], r[3]])
}

/// A link target the page may carry: a web address, a mail address or a page of this document.
pub fn parse_target(target: &str) -> Result<LinkAction, String> {
    let t = target.trim();
    if let Some(n) = t.strip_prefix("page:") {
        let page: usize = n.trim().parse().map_err(|_| "a page link is page:NUMBER".to_string())?;
        return page.checked_sub(1).map(LinkAction::Page).ok_or_else(|| "pages start at 1".to_string());
    }
    let lower = t.to_ascii_lowercase();
    if t.len() <= 2048 && !t.chars().any(char::is_control) && ["https://", "http://", "mailto:"].iter().any(|p| lower.starts_with(p)) {
        Ok(LinkAction::Uri(t.to_string()))
    } else {
        Err("a link goes to an http(s) address, a mailto: address, or page:NUMBER".into())
    }
}

impl Document {
    /// Every match of `query` in the text, page by page, as JSON: `page`, the matched `text` and its
    /// `rects` (one per line it spans). At most `limit` matches (and never more than a thousand).
    pub fn find_json(&mut self, query: &str, case_sensitive: bool, whole_words: bool, limit: usize) -> Result<String, String> {
        let mut found: Vec<Value> = Vec::new();
        let limit = limit.clamp(1, MAX_MATCHES);
        if query.trim().is_empty() {
            return Ok("[]".into());
        }
        for page in 0..self.page_count() {
            if found.len() >= limit {
                break;
            }
            let text = self.page_text(page)?;
            for range in text.find_opts_limited(query, case_sensitive, whole_words, limit - found.len()) {
                found.push(json!({
                    "page": page,
                    "text": text.text_of(range.clone()),
                    "rects": text.line_rects(range).into_iter().map(rect_json).collect::<Vec<_>>(),
                }));
            }
        }
        serde_json::to_string(&found).map_err(|e| e.to_string())
    }

    /// The text between two points of a page, read in reading order from the glyph nearest the first
    /// to the one nearest the second: `{ "text": ..., "rects": [...] }`, one rectangle per line.
    pub fn select_text_json(&mut self, page: usize, from: [f64; 2], to: [f64; 2]) -> Result<String, String> {
        if !from.iter().chain(&to).all(|v| v.is_finite()) {
            return Err("the points must be finite".into());
        }
        let text = self.page_text(page)?;
        let (Some(a), Some(b)) = (text.nearest(from[0] as f32, from[1] as f32), text.nearest(to[0] as f32, to[1] as f32)) else {
            return Ok(json!({ "text": "", "rects": [] }).to_string());
        };
        let (first, last) = (a.min(b), a.max(b));
        let range = first..last.saturating_add(1);
        let rects: Vec<Value> = text.line_rects(range.clone()).into_iter().take(MAX_RECTS).map(rect_json).collect();
        Ok(json!({ "text": text.text_of(range), "rects": rects }).to_string())
    }

    /// Marks text up: `kind` is `highlight`, `underline` or `strikeout`; `rects` are displayed boxes
    /// (what [`Self::select_text_json`] gave); `color` is RGB from 0 to 1, or none for the kind's usual.
    pub fn markup(&mut self, page: usize, kind: &str, rects: &[[f64; 4]], color: Option<[f64; 3]>) -> Result<(), String> {
        let kind = match kind {
            "highlight" => Markup::Highlight,
            "underline" => Markup::Underline,
            "strikeout" => Markup::StrikeOut,
            other => return Err(format!("unknown markup {other:?}")),
        };
        if rects.is_empty() || rects.len() > MAX_RECTS {
            return Err("mark up between 1 and 2000 lines".into());
        }
        let info = self.page_info_ref(page)?;
        let mut quads = Vec::with_capacity(rects.len());
        for r in rects {
            let r = finite_box(*r)?;
            quads.push(info.view_rect_to_quad([r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]));
        }
        let shape = Shape::TextMarkup { kind, quads };
        let mut style = Style::default_for(&shape);
        if let Some(c) = color {
            if !c.iter().all(|v| (0.0..=1.0).contains(v)) {
                return Err("a color is [r, g, b], each from 0 to 1".into());
            }
            style.color = c;
        }
        self.apply(Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: String::new(), author: String::new() }))
    }

    /// Adds a link over a displayed box: `target` is an http(s) or mailto: address, or `page:N` (1-based).
    pub fn add_link(&mut self, page: usize, view: [f64; 4], target: &str) -> Result<(), String> {
        let action = parse_target(target)?;
        let v = finite_box(view)?;
        let user = self.page_info_ref(page)?.view_rect_to_user([v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
        let rect = user.map(f64::from);
        self.apply(Edit::AddLink { page, rect, action, style: LinkStyle::default() })
    }

    /// The links of a page as JSON: `index` (what `delete_link` takes), `view`, and `uri` or `page` (0-based).
    pub fn links_json(&self, page: usize) -> Result<String, String> {
        let info = self.page_info_ref(page)?;
        let list: Vec<Value> = self
            .doc()?
            .info
            .links
            .iter()
            .filter(|l| l.page == page)
            .filter_map(|l| {
                let a = info.user_to_view(l.rect[0], l.rect[1]);
                let b = info.user_to_view(l.rect[2], l.rect[3]);
                let view = json!([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]);
                match &l.target {
                    LinkTarget::Uri(u) => Some(json!({ "index": l.index, "view": view, "uri": u })),
                    LinkTarget::Page(p, _) => Some(json!({ "index": l.index, "view": view, "page": p })),
                    _ => None,
                }
            })
            .collect();
        serde_json::to_string(&list).map_err(|e| e.to_string())
    }

    pub fn delete_link(&mut self, page: usize, index: usize) -> Result<(), String> {
        self.apply(Edit::DeleteLink { page, index })
    }

    /// The page's comments and marks (highlights, notes, shapes) as JSON: `index`, `subtype`, `view`, `contents`.
    pub fn annotations_json(&self, page: usize) -> Result<String, String> {
        let info = self.page_info_ref(page)?;
        let list: Vec<Value> = self
            .doc()?
            .info
            .annotations
            .iter()
            .filter(|a| a.page == page)
            .map(|a| {
                let p = info.user_to_view(a.rect[0], a.rect[1]);
                let q = info.user_to_view(a.rect[2], a.rect[3]);
                json!({
                    "index": a.index,
                    "subtype": a.subtype,
                    "contents": a.contents,
                    "view": [p[0].min(q[0]), p[1].min(q[1]), p[0].max(q[0]), p[1].max(q[1])],
                })
            })
            .collect();
        serde_json::to_string(&list).map_err(|e| e.to_string())
    }

    /// Adds a comment from a description (boxes and points are displayed points from the page's top-left):
    /// `{"type": "note"|"rect"|"oval"|"line"|"arrow"|"ink"|"textbox"|"stamp", ...}` with `at`, `box`, `from`,
    /// `to`, `strokes`, `stamp`, `fontSize`, and optionally `contents`, `author`, `color` and `fill` (RGB 0 to 1),
    /// `width` and `opacity`.
    pub fn add_annotation(&mut self, page: usize, spec: &str) -> Result<(), String> {
        let v: Value = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        let info = self.page_info_ref(page)?;
        let point = |p: &Value| -> Result<[f64; 2], String> {
            let a = p.as_array().filter(|a| a.len() == 2).ok_or("a point is [x, y]")?;
            let (x, y) = (a[0].as_f64().ok_or("a point is numbers")?, a[1].as_f64().ok_or("a point is numbers")?);
            if !(x.is_finite() && y.is_finite()) {
                return Err("a point must be finite".into());
            }
            let u = info.view_to_user(x as f32, y as f32);
            Ok([f64::from(u[0]), f64::from(u[1])])
        };
        let rect = |key: &str| -> Result<[f64; 4], String> {
            let a = v[key].as_array().filter(|a| a.len() == 4).ok_or_else(|| format!("{key} is [x0, y0, x1, y1]"))?;
            let mut b = [0.0; 4];
            for (slot, n) in b.iter_mut().zip(a) {
                *slot = n.as_f64().filter(|n| n.is_finite()).ok_or("a box is finite numbers")?;
            }
            let u = info.view_rect_to_user([b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32]);
            Ok(u.map(f64::from))
        };
        let colour = |key: &str| -> Result<Option<[f64; 3]>, String> {
            match v.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(c) => {
                    let a = c.as_array().filter(|a| a.len() == 3).ok_or_else(|| format!("{key} is [r, g, b]"))?;
                    let mut out = [0.0; 3];
                    for (slot, n) in out.iter_mut().zip(a) {
                        *slot = n.as_f64().filter(|n| (0.0..=1.0).contains(n)).ok_or_else(|| format!("{key} is three numbers from 0 to 1"))?;
                    }
                    Ok(Some(out))
                }
            }
        };
        let kind = v["type"].as_str().ok_or("a comment has a type")?;
        let shape = match kind {
            "note" => Shape::Note { at: point(&v["at"])?, icon: NoteIcon::Comment },
            "rect" => Shape::Rectangle { rect: rect("box")? },
            "oval" => Shape::Oval { rect: rect("box")? },
            "line" => Shape::Line { from: point(&v["from"])?, to: point(&v["to"])?, start: LineEnding::None, end: LineEnding::None },
            "arrow" => Shape::Line { from: point(&v["from"])?, to: point(&v["to"])?, start: LineEnding::None, end: LineEnding::OpenArrow },
            "ink" => {
                let strokes = v["strokes"].as_array().filter(|s| !s.is_empty() && s.len() <= 500).ok_or("ink has strokes")?;
                let mut out = Vec::with_capacity(strokes.len());
                for stroke in strokes {
                    let pts = stroke.as_array().filter(|p| !p.is_empty() && p.len() <= 20_000).ok_or("a stroke has points")?;
                    out.push(pts.iter().map(&point).collect::<Result<Vec<_>, _>>()?);
                }
                Shape::Ink { strokes: out }
            }
            "textbox" => {
                Shape::TextBox { rect: rect("box")?, font_size: v["fontSize"].as_f64().filter(|n| (4.0..=200.0).contains(n)).unwrap_or(12.0) }
            }
            "typewriter" => {
                Shape::Typewriter { rect: rect("box")?, font_size: v["fontSize"].as_f64().filter(|n| (4.0..=200.0).contains(n)).unwrap_or(12.0) }
            }
            "mark" => {
                let mark = match v["mark"].as_str().unwrap_or("check") {
                    "check" => FillMark::Check,
                    "cross" => FillMark::Cross,
                    "dot" => FillMark::Dot,
                    "line" => FillMark::Line,
                    other => return Err(format!("unknown mark {other:?}")),
                };
                Shape::Mark { rect: rect("box")?, mark }
            }
            "signature" => {
                let strokes = v["strokes"].as_array().filter(|s| !s.is_empty() && s.len() <= 500).ok_or("a signature has strokes")?;
                let mut out = Vec::with_capacity(strokes.len());
                for stroke in strokes {
                    let pts = stroke.as_array().filter(|p| !p.is_empty() && p.len() <= 20_000).ok_or("a stroke has points")?;
                    out.push(pts.iter().map(&point).collect::<Result<Vec<_>, _>>()?);
                }
                Shape::Signature { strokes: out }
            }
            "typedSignature" => {
                let text = v["text"].as_str().filter(|t| !t.trim().is_empty() && t.len() <= 200).ok_or("a typed signature has text")?;
                let at = point(&v["at"])?;
                let height = v["height"].as_f64().filter(|h| h.is_finite() && (4.0..=300.0).contains(h)).unwrap_or(36.0);
                pdfcraft_engine::typed_signature_shape(at, text, height, i64::from(info.rotation))
                    .ok_or("that text cannot be drawn as a signature")?
            }
            "stamp" => {
                let name = v["stamp"].as_str().ok_or("a stamp has a name")?;
                let stamp = StampKind::from_name(name.as_bytes()).ok_or_else(|| format!("unknown stamp {name:?}"))?;
                Shape::Stamp { rect: rect("box")?, stamp, by: v["by"].as_str().map(str::to_string) }
            }
            other => return Err(format!("unknown comment type {other:?}")),
        };
        let mut style = Style::default_for(&shape);
        if let Some(c) = colour("color")? {
            style.color = c;
        }
        if let Some(c) = colour("fill")? {
            style.fill = Some(c);
        }
        if let Some(w) = v["width"].as_f64().filter(|w| w.is_finite() && (0.0..=100.0).contains(w)) {
            style.width = w;
        }
        if let Some(o) = v["opacity"].as_f64().filter(|o| (0.0..=1.0).contains(o)) {
            style.opacity = o;
        }
        let contents = v["contents"].as_str().unwrap_or_default().to_string();
        let author = v["author"].as_str().unwrap_or_default().to_string();
        self.apply(Edit::AddAnnotation(NewAnnotation { page, shape, style, contents, author }))
    }

    /// The stamps that can be added: `name` (what `add_annotation` takes), the `label` on it, its `group`
    /// and its natural `size` in points.
    pub fn stamps_json() -> String {
        let list: Vec<Value> = StampKind::ALL
            .iter()
            .map(|k| {
                let (w, h) = k.size();
                let group = match k.group() {
                    pdfcraft_engine::StampGroup::Dynamic => "dynamic",
                    pdfcraft_engine::StampGroup::SignHere => "signHere",
                    pdfcraft_engine::StampGroup::StandardBusiness => "business",
                };
                json!({ "name": k.name(), "label": k.label(), "group": group, "size": [w, h], "color": k.color() })
            })
            .collect();
        serde_json::to_string(&list).unwrap_or_default()
    }

    /// Changes the text of the comment at `index` (what `annotations_json` lists).
    pub fn set_annotation_contents(&mut self, page: usize, index: usize, text: &str) -> Result<(), String> {
        self.apply(Edit::SetAnnotationContents { page, index, text: text.to_string() })
    }

    pub fn delete_annotation(&mut self, page: usize, index: usize) -> Result<(), String> {
        self.apply(Edit::DeleteAnnotation { page, index })
    }

    /// The form fields on a page as JSON: `name`, `kind` (`text`, `check`, `radio`, `button`, `choice`,
    /// `signature`), `value`, `view`.
    pub fn fields_json(&self, page: usize) -> Result<String, String> {
        let info = self.page_info_ref(page)?;
        let list: Vec<Value> = self
            .doc()?
            .info
            .fields
            .iter()
            .filter(|f| f.page == Some(page))
            .filter_map(|f| {
                let r = f.rect?;
                let (a, b) = (info.user_to_view(r[0], r[1]), info.user_to_view(r[2], r[3]));
                let kind = match f.kind {
                    FieldKind::Text => "text",
                    FieldKind::CheckBox => "check",
                    FieldKind::Radio => "radio",
                    FieldKind::PushButton => "button",
                    FieldKind::Combo | FieldKind::List => "choice",
                    FieldKind::Signature => "signature",
                    FieldKind::Unknown => return None,
                };
                Some(json!({ "name": f.name, "kind": kind, "value": f.value, "view": [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])] }))
            })
            .collect();
        serde_json::to_string(&list).map_err(|e| e.to_string())
    }

    /// Fills a field: `value` is JSON, `{"text": "..."}`, `{"check": true}` or `{"choice": ["a"]}`.
    pub fn set_field(&mut self, name: &str, value: &str) -> Result<(), String> {
        let v: Value = serde_json::from_str(value).map_err(|e| e.to_string())?;
        let value = if let Some(t) = v["text"].as_str() {
            FieldValue::Text(t.to_string())
        } else if let Some(c) = v["check"].as_bool() {
            FieldValue::Check(c)
        } else if let Some(list) = v["choice"].as_array() {
            FieldValue::Choice(list.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        } else {
            return Err("a value is {\"text\": ...}, {\"check\": ...} or {\"choice\": [...]}".into());
        };
        self.apply(Edit::SetFieldValue { name: name.to_string(), value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(body: &str, extra_objs: &[&str], page_extra: &str, catalog_extra: &str) -> Vec<u8> {
        let mut objs = vec![
            format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
            "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> {page_extra} >>"),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        ];
        objs.extend(extra_objs.iter().map(|s| s.to_string()));
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn plain() -> Document {
        let body = "BT /F1 18 Tf 20 250 Td (Alpha beta gamma) Tj 0 -30 Td (second line alpha) Tj ET";
        Document::open(&pdf(body, &[], "", ""), None, || 1_700_000_000).expect("opens")
    }

    #[test]
    fn finds_text_with_the_boxes_to_show_it() {
        let mut doc = plain();
        let found: Vec<Value> = serde_json::from_str(&doc.find_json("alpha", false, false, 10).expect("finds")).expect("json");
        assert_eq!(found.len(), 2, "both lines: {found:?}");
        assert_eq!(found[0]["page"], 0);
        assert_eq!(found[0]["text"], "Alpha");
        let r = &found[0]["rects"][0];
        let (x0, y0, x1, y1) =
            (r[0].as_f64().unwrap_or(-1.0), r[1].as_f64().unwrap_or(-1.0), r[2].as_f64().unwrap_or(-1.0), r[3].as_f64().unwrap_or(-1.0));
        assert!(x0 >= 15.0 && x1 < 100.0 && y0 > 20.0 && y1 < 70.0 && x0 < x1 && y0 < y1, "near the top left, drawn at y 250 up: {r}");
        let exact: Vec<Value> = serde_json::from_str(&doc.find_json("alpha", true, false, 10).expect("finds")).expect("json");
        assert_eq!(exact.len(), 1, "case sensitive: only the lower-case one");
        let words: Vec<Value> = serde_json::from_str(&doc.find_json("alph", false, true, 10).expect("finds")).expect("json");
        assert!(words.is_empty(), "whole words only");
        assert_eq!(doc.find_json("  ", false, false, 10).expect("empty"), "[]");
        let one: Vec<Value> = serde_json::from_str(&doc.find_json("a", false, false, 1).expect("finds")).expect("json");
        assert_eq!(one.len(), 1, "limited");
    }

    #[test]
    fn selects_between_two_points_and_marks_it_up() {
        let mut doc = plain();
        let found: Vec<Value> = serde_json::from_str(&doc.find_json("beta", false, false, 5).expect("finds")).expect("json");
        let r: Vec<f64> = found[0]["rects"][0].as_array().expect("rect").iter().map(|v| v.as_f64().expect("n")).collect();
        let selection: Value =
            serde_json::from_str(&doc.select_text_json(0, [r[0] + 1.0, (r[1] + r[3]) / 2.0], [r[2] - 1.0, (r[1] + r[3]) / 2.0]).expect("selects"))
                .expect("json");
        assert!(selection["text"].as_str().expect("text").contains("beta"), "{selection}");
        let rects: Vec<[f64; 4]> = selection["rects"]
            .as_array()
            .expect("rects")
            .iter()
            .map(|x| [x[0].as_f64().expect("n"), x[1].as_f64().expect("n"), x[2].as_f64().expect("n"), x[3].as_f64().expect("n")])
            .collect();
        doc.markup(0, "highlight", &rects, Some([1.0, 0.9, 0.0])).expect("highlights");
        let marks: Vec<Value> = serde_json::from_str(&doc.annotations_json(0).expect("annotations")).expect("json");
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0]["subtype"], "Highlight");
        doc.undo().expect("undoes");
        assert_eq!(doc.annotations_json(0).expect("annotations"), "[]");
        assert!(doc.markup(0, "squiggle", &rects, None).is_err());
        assert!(doc.markup(0, "highlight", &[], None).is_err());
        assert!(doc.markup(0, "highlight", &rects, Some([2.0, 0.0, 0.0])).is_err());
        let nothing: Value =
            serde_json::from_str(&doc.select_text_json(5, [0.0, 0.0], [1.0, 1.0]).unwrap_or_else(|_| "{\"text\":\"err\"}".into())).expect("json");
        assert_eq!(nothing["text"], "err", "no such page");
    }

    #[test]
    fn adds_comments_stamps_and_edits_their_text() {
        let mut doc = plain();
        let specs = [
            r#"{"type":"note","at":[30,40],"contents":"Check this"}"#,
            r#"{"type":"rect","box":[20,60,120,100],"color":[1,0,0],"width":2}"#,
            r#"{"type":"oval","box":[20,110,120,150],"fill":[0.9,0.9,0.1]}"#,
            r#"{"type":"line","from":[10,160],"to":[100,170]}"#,
            r#"{"type":"arrow","from":[10,180],"to":[100,190]}"#,
            r#"{"type":"ink","strokes":[[[10,200],[20,210],[40,205]]],"color":[0,0,1]}"#,
            r#"{"type":"textbox","box":[20,220,150,250],"fontSize":14,"contents":"Hello"}"#,
            r#"{"type":"stamp","box":[20,260,140,290],"stamp":"Approved"}"#,
        ];
        for spec in specs {
            doc.add_annotation(0, spec).unwrap_or_else(|e| panic!("{spec}: {e}"));
        }
        let marks: Vec<Value> = serde_json::from_str(&doc.annotations_json(0).expect("annotations")).expect("json");
        assert_eq!(marks.len(), specs.len());
        let note = marks.iter().find(|m| m["subtype"] == "Text").expect("the note");
        assert_eq!(note["contents"], "Check this");
        let index = usize::try_from(note["index"].as_u64().expect("index")).expect("fits");
        doc.set_annotation_contents(0, index, "Checked").expect("changes the text");
        let marks: Vec<Value> = serde_json::from_str(&doc.annotations_json(0).expect("annotations")).expect("json");
        assert!(marks.iter().any(|m| m["contents"] == "Checked"));
        for bad in [
            "not json",
            r#"{"type":"squiggle"}"#,
            r#"{"type":"rect"}"#,
            r#"{"type":"rect","box":[0,0,1]}"#,
            r#"{"type":"rect","box":[0,0,10,10],"color":[2,0,0]}"#,
            r#"{"type":"ink","strokes":[]}"#,
            r#"{"type":"stamp","box":[0,0,10,10],"stamp":"Nope"}"#,
            r#"{"type":"note","at":[1e999,2]}"#,
        ] {
            assert!(doc.add_annotation(0, bad).is_err(), "{bad} should be refused");
        }
        assert!(Document::stamps_json().contains("Approved"));
    }

    #[test]
    fn adds_lists_and_deletes_links() {
        let mut doc = plain();
        doc.add_link(0, [20.0, 40.0, 120.0, 60.0], "https://example.com/a").expect("adds a web link");
        doc.add_link(0, [20.0, 80.0, 120.0, 100.0], "page:1").expect("adds a page link");
        let links: Vec<Value> = serde_json::from_str(&doc.links_json(0).expect("links")).expect("json");
        assert_eq!(links.len(), 2);
        let web = links.iter().find(|l| l["uri"] == "https://example.com/a").expect("the web link");
        let v: Vec<f64> = web["view"].as_array().expect("view").iter().map(|x| x.as_f64().expect("n")).collect();
        assert!((v[1] - 40.0).abs() < 1.0 && (v[3] - 60.0).abs() < 1.0, "where it was put: {v:?}");
        assert!(links.iter().any(|l| l["page"] == 0));
        let index = usize::try_from(web["index"].as_u64().expect("index")).expect("fits");
        doc.delete_link(0, index).expect("deletes");
        assert_eq!(serde_json::from_str::<Vec<Value>>(&doc.links_json(0).expect("links")).expect("json").len(), 1);
        for bad in ["javascript:alert(1)", "ftp://x", "page:0", "page:x", "", "https://exa\nmple.com"] {
            assert!(doc.add_link(0, [1.0, 1.0, 9.0, 9.0], bad).is_err(), "{bad:?}");
        }
        assert!(doc.add_link(0, [9.0, 9.0, 1.0, 1.0], "https://example.com").is_err());
    }

    #[test]
    fn lists_and_fills_form_fields() {
        let form = pdf(
            "BT /F1 12 Tf 20 280 Td (Form) Tj ET",
            &[
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 200 120 220] /P 4 0 R /DA (/F1 12 Tf 0 g) >>",
                "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /Rect [20 150 40 170] /P 4 0 R /AS /Off /AP << /N << /Yes 5 0 R /Off 5 0 R >> >> >>",
                "<< /Fields [6 0 R 7 0 R] /DA (/F1 12 Tf 0 g) /DR << /Font << /F1 3 0 R >> >> /NeedAppearances true >>",
            ],
            "/Annots [6 0 R 7 0 R]",
            "/AcroForm 8 0 R",
        );
        let mut doc = Document::open(&form, None, || 1_700_000_000).expect("opens");
        let fields: Vec<Value> = serde_json::from_str(&doc.fields_json(0).expect("fields")).expect("json");
        let names: Vec<&str> = fields.iter().filter_map(|f| f["name"].as_str()).collect();
        assert!(names.contains(&"name") && names.contains(&"agree"), "{fields:?}");
        let name = fields.iter().find(|f| f["name"] == "name").expect("the text field");
        assert_eq!(name["kind"], "text");
        let v: Vec<f64> = name["view"].as_array().expect("view").iter().map(|x| x.as_f64().expect("n")).collect();
        assert!((v[1] - 80.0).abs() < 1.0 && (v[3] - 100.0).abs() < 1.0, "200 to 220 up is 80 to 100 down: {v:?}");
        doc.set_field("name", r#"{"text": "Ada"}"#).expect("fills the text");
        doc.set_field("agree", r#"{"check": true}"#).expect("checks");
        let after: Vec<Value> = serde_json::from_str(&doc.fields_json(0).expect("fields")).expect("json");
        assert_eq!(after.iter().find(|f| f["name"] == "name").expect("field")["value"], "Ada");
        assert!(doc.set_field("nothing", r#"{"text": "x"}"#).is_err());
        assert!(doc.set_field("name", r#"{"other": 1}"#).is_err());
        assert!(doc.set_field("name", "not json").is_err());
    }
}
