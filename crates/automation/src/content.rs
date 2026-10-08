//! Edit a PDF ▸ Add content tools: add text and images as page content, list what was added,
//! move/resize/retype/restyle it, delete it. Geometry is in points from the top-left of the
//! displayed page (y down), like every other tool.

use pdfcraft_engine::{AddedContent, AddedText, Edit, FontFamily, TextAlign};
use serde_json::{Value, json};

use crate::comments::parse_color;
use crate::{Args, Automation, Result, ToolError, failed};

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// Top-left view rect [x0 y0 x1 y1] ↔ display space [x0 y0 x1 y1] (y up).
fn to_display(r: [f64; 4], ph: f64) -> [f64; 4] {
    [r[0].min(r[2]), ph - r[1].max(r[3]), r[0].max(r[2]), ph - r[1].min(r[3])]
}

fn to_view(r: [f64; 4], ph: f64) -> [f64; 4] {
    [r[0], ph - r[3], r[2], ph - r[1]]
}

fn rect_arg(a: &Args, key: &str) -> Result<Option<[f64; 4]>> {
    a.get(key)
        .map(|v| {
            let r: Vec<f64> = v.as_array().map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            <[f64; 4]>::try_from(r).map_err(|_| bad(format!("{key} must be 4 numbers")))
        })
        .transpose()
}

/// Apply the style arguments to `t`.
fn style(a: &Args, t: &mut AddedText) -> Result<()> {
    if let Some(s) = a.opt_num("size")? {
        t.size = s;
    }
    if let Some(f) = a.opt_str("font")? {
        t.family = match f.to_ascii_lowercase().as_str() {
            "helvetica" | "sans" | "arial" => FontFamily::Helvetica,
            "times" | "serif" => FontFamily::Times,
            "courier" | "mono" | "monospace" => FontFamily::Courier,
            other => return Err(bad(format!("unknown font {other:?} (helvetica, times, courier)"))),
        };
    }
    if let Some(b) = a.opt_bool("bold")? {
        t.bold = b;
    }
    if let Some(i) = a.opt_bool("italic")? {
        t.italic = i;
    }
    if let Some(c) = a.opt_str("color")? {
        t.color = parse_color(c)?;
    }
    if let Some(al) = a.opt_str("align")? {
        t.align = match al {
            "left" => TextAlign::Left,
            "center" | "centre" => TextAlign::Center,
            "right" => TextAlign::Right,
            "justify" => TextAlign::Justify,
            other => return Err(bad(format!("unknown align {other:?}"))),
        };
    }
    Ok(())
}

impl Automation {
    pub(crate) fn content_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let mut per_page: std::collections::HashMap<usize, usize> = Default::default();
        let items: Vec<Value> = doc
            .added
            .iter()
            .map(|it| {
                let index = per_page.entry(it.page).or_default();
                let ph = doc.info.pages[it.page].height as f64;
                let mut v =
                    json!({ "page": it.page + 1, "index": *index + 1, "rect": to_view(it.content.rect(), ph).map(|x| (x * 100.0).round() / 100.0) });
                *index += 1;
                match &it.content {
                    AddedContent::Text(t) => {
                        v["type"] = json!("text");
                        v["text"] = json!(t.text);
                        v["font"] = json!(t.family.label());
                        v["size"] = json!(t.size);
                        v["bold"] = json!(t.bold);
                        v["italic"] = json!(t.italic);
                    }
                    AddedContent::Image(_) => v["type"] = json!("image"),
                }
                v
            })
            .collect();
        Ok(json!({ "count": items.len(), "items": items }))
    }

    pub(crate) fn page_add_text(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let ph = self.doc(a)?.info.pages[page].height as f64;
        let rect = match (rect_arg(a, "rect")?, a.get("at")) {
            (Some(r), _) => to_display(r, ph),
            (None, Some(v)) => {
                let p: Vec<f64> = v.as_array().map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                let [x, y]: [f64; 2] = p.try_into().map_err(|_| bad("at must be [x, y]"))?;
                let w = a.opt_num("width")?.unwrap_or(200.0);
                [x, ph - y - 20.0, x + w, ph - y]
            }
            (None, None) => return Err(bad("pass at [x, y] (the text's top-left) or rect")),
        };
        let mut t = AddedText { rect, text: a.str("text")?.to_string(), ..AddedText::default() };
        style(a, &mut t)?;
        let mut out = self.apply(a, Edit::AddText { page, text: t })?;
        out["index"] = json!(self.doc(a)?.added.iter().filter(|x| x.page == page).count());
        Ok(out)
    }

    pub(crate) fn page_add_image(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let ph = self.doc(a)?.info.pages[page].height as f64;
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let rect = rect_arg(a, "rect")?.map(|r| to_display(r, ph));
        let mut out = self.apply(a, Edit::AddImage { page, rect, name, bytes: std::sync::Arc::new(bytes) })?;
        let doc = self.doc(a)?;
        let mine: Vec<_> = doc.added.iter().filter(|x| x.page == page).collect();
        out["index"] = json!(mine.len());
        out["rect"] = json!(mine.last().map(|x| to_view(x.content.rect(), ph)));
        Ok(out)
    }

    fn item(&self, a: &Args) -> Result<(usize, usize, AddedContent, f64)> {
        let page = self.page(a)?;
        let doc = self.doc(a)?;
        let index = a.int("index")?;
        let ph = doc.info.pages[page].height as f64;
        let it = doc
            .added
            .iter()
            .filter(|x| x.page == page)
            .nth((index - 1).max(0) as usize)
            .filter(|_| index >= 1)
            .ok_or_else(|| failed(format!("page {} has no added item {index} (see content_list)", page + 1)))?;
        Ok((page, index as usize - 1, it.content.clone(), ph))
    }

    pub(crate) fn content_update(&mut self, a: &Args) -> Result<Value> {
        let (page, index, content, ph) = self.item(a)?;
        let mut content = match rect_arg(a, "rect")? {
            Some(r) => content.with_rect(to_display(r, ph)),
            None => content,
        };
        match &mut content {
            AddedContent::Text(t) => {
                if let Some(s) = a.opt_str("text")? {
                    t.text = s.to_string();
                }
                style(a, t)?;
                if ["rotate", "flip_h", "flip_v", "crop", "image"].iter().any(|k| a.get(k).is_some()) {
                    return Err(bad("rotate, flip, crop and image apply to images"));
                }
            }
            AddedContent::Image(i) => {
                if a.get("text").is_some() {
                    return Err(bad("images have no text"));
                }
                if let Some(deg) = a.opt_int("rotate")? {
                    if deg.rem_euclid(90) != 0 {
                        return Err(bad("rotate must be a multiple of 90"));
                    }
                    i.rotation = ((i64::from(i.rotation) * 90 + deg).rem_euclid(360) / 90) as u8;
                }
                if a.opt_bool("flip_h")? == Some(true) {
                    i.flip_h = !i.flip_h;
                }
                if a.opt_bool("flip_v")? == Some(true) {
                    i.flip_v = !i.flip_v;
                }
                if let Some(c) = rect_arg(a, "crop")? {
                    if !c.iter().all(|v| (0.0..0.5).contains(v)) {
                        return Err(bad("crop takes fractions from 0 to under 0.5 (left, bottom, right, top)"));
                    }
                    i.crop = c;
                }
                if let Some(p) = a.opt_str("image")? {
                    let path = self.resolve(p, false)?;
                    let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    // Box and transforms first, then the new picture.
                    self.apply(a, Edit::UpdateContent { page, index, content: content.clone() })?;
                    return self.apply(a, Edit::ReplaceImage { page, index, name, bytes: std::sync::Arc::new(bytes) });
                }
            }
        }
        self.apply(a, Edit::UpdateContent { page, index, content })
    }

    pub(crate) fn content_delete(&mut self, a: &Args) -> Result<Value> {
        let (page, index, _, _) = self.item(a)?;
        self.apply(a, Edit::DeleteContent { page, index })
    }
}
