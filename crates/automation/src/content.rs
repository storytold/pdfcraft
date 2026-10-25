//! Edit a PDF ▸ Add content tools: add text and images as page content, list what was added,
//! move/resize/retype/restyle it, delete it. Geometry is in points from the top-left of the
//! displayed page (y down), like every other tool.

use pdfcraft_engine::{ADDED_CHAR_SPACING_LIMIT, AddedContent, AddedText, Edit, FontFamily, TextAlign};
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
    if let Some(c) = a.opt_num("char_spacing")? {
        if !(-ADDED_CHAR_SPACING_LIMIT..=ADDED_CHAR_SPACING_LIMIT).contains(&c) {
            return Err(bad(format!("char_spacing must be between -{ADDED_CHAR_SPACING_LIMIT} and {ADDED_CHAR_SPACING_LIMIT} points")));
        }
        t.char_spacing = c;
    }
    Ok(())
}

impl Automation {
    pub(crate) fn content_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let mut per_page: std::collections::HashMap<usize, usize> = Default::default();
        let items: Vec<Value> = doc
            .added()
            .iter()
            .map(|it| {
                let index = per_page.entry(it.page).or_default();
                // Unchanged on a page the viewer could not read (#2660), like `crate::view_rect`.
                let rect = doc.info.pages.get(it.page).map_or(it.content.rect(), |p| to_view(it.content.rect(), p.height as f64));
                let mut v = json!({ "page": it.page + 1, "index": *index + 1, "rect": rect.map(|x| (x * 100.0).round() / 100.0) });
                *index += 1;
                match &it.content {
                    AddedContent::Text(t) => {
                        v["type"] = json!("text");
                        v["text"] = json!(t.text);
                        v["font"] = json!(t.family.label());
                        v["size"] = json!(t.size);
                        v["bold"] = json!(t.bold);
                        v["italic"] = json!(t.italic);
                        v["char_spacing"] = json!(t.char_spacing);
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
        out["index"] = json!(self.doc(a)?.added().iter().filter(|x| x.page == page).count());
        Ok(out)
    }

    pub(crate) fn page_add_image(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let ph = self.doc(a)?.info.pages[page].height as f64;
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = crate::read_input(&path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let rect = rect_arg(a, "rect")?.map(|r| to_display(r, ph));
        let mut out = self.apply(a, Edit::AddImage { page, rect, name, bytes: std::sync::Arc::new(bytes) })?;
        let doc = self.doc(a)?;
        let mine: Vec<_> = doc.added().iter().filter(|x| x.page == page).collect();
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
            .added()
            .iter()
            .filter(|x| x.page == page)
            .nth(usize::try_from(index.saturating_sub(1)).unwrap_or(usize::MAX))
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
                if ["font", "size", "bold", "italic", "color", "align", "char_spacing"].iter().any(|k| a.get(k).is_some()) {
                    return Err(bad("font, size, bold, italic, color, align and char_spacing apply to text"));
                }
                if let Some(deg) = a.opt_int("rotate")? {
                    if deg.rem_euclid(90) != 0 {
                        return Err(bad("rotate must be a multiple of 90"));
                    }
                    // Reduce untrusted degrees before adding the current quarter turns.
                    i.rotation = (i64::from(i.rotation) + deg.rem_euclid(360) / 90).rem_euclid(4) as u8;
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
                    let bytes = crate::read_input(&path)?;
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

    /// Edit ▸ Copy, then Paste: an added item onto `to_page` of `to_doc` (its own page and
    /// document by default), as it is, at `at` (its top-left) or where it was, stepped down and to
    /// the right past copies of it already on that page.
    pub(crate) fn content_copy(&mut self, a: &Args) -> Result<Value> {
        let (page, index, content, _) = self.item(a)?;
        let source = self.doc(a)?;
        source.check_copying()?;
        let from = source.id;
        let file = match &content {
            AddedContent::Text(_) => None,
            AddedContent::Image(_) => {
                let (ext, bytes) = source.added_image_file(page, index).map_err(failed)?;
                Some((format!("Pasted image.{ext}"), std::sync::Arc::new(bytes)))
            }
        };
        let to_doc = match a.opt_int("to_doc")? {
            Some(d) => {
                let d = u64::try_from(d).map_err(|_| bad("to_doc must be positive"))?;
                self.session.get(pdfcraft_engine::DocId(d)).ok_or_else(|| failed(format!("no open document with id {d} (see doc_list)")))?.id
            }
            None => from,
        };
        let doc = self.session.get(to_doc).ok_or_else(|| failed("the document is no longer open"))?;
        let n = doc.info.pages.len();
        let to_page = match a.opt_int("to_page")? {
            Some(p) => usize::try_from(p)
                .ok()
                .and_then(|p| p.checked_sub(1))
                .filter(|p| *p < n)
                .ok_or_else(|| bad(format!("to_page {p} is out of range: the document has {n} pages")))?,
            None if page < n => page,
            None => return Err(bad(format!("page {} is out of range in the target: it has {n} pages; pass to_page", page + 1))),
        };
        let info = doc.info.pages.get(to_page).ok_or_else(|| bad("the page no longer exists"))?;
        let (pw, ph) = (f64::from(info.width), f64::from(info.height));
        let at = match a.get("at") {
            Some(v) => {
                let p: Option<Vec<f64>> = v.as_array().and_then(|x| x.iter().map(Value::as_f64).collect());
                let [x, y]: [f64; 2] = p.and_then(|p| p.try_into().ok()).ok_or_else(|| bad("at must be [x, y]"))?;
                Some([x, ph - y])
            }
            None => None,
        };
        let on_page = || doc.added().iter().filter(|x| x.page == to_page);
        let rect = match at {
            Some(at) => pdfcraft_engine::added_paste_rect(content.rect(), [pw, ph], 0, Some(at)),
            None => pdfcraft_engine::added_paste_place(on_page().map(|x| &x.content), &content, [pw, ph]),
        };
        let before = on_page().count();
        let target = json!({ "doc": to_doc.0 });
        let mut out = self.apply(&Args(&target), Edit::PasteContent { page: to_page, content: content.with_rect(rect), file })?;
        out["page"] = json!(to_page + 1);
        out["index"] = json!(before + 1);
        out["rect"] = json!(to_view(rect, ph).map(|x| (x * 100.0).round() / 100.0));
        Ok(out)
    }
}

impl Automation {
    /// One mixed inventory, with exact source references and the current document generation.
    pub(crate) fn object_list(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let doc = self.doc(a)?;
        let info = doc.info.pages.get(page).ok_or_else(|| bad("the page no longer exists"))?;
        let objects = doc.editable_objects(page).map_err(failed)?;
        let is_path = |o: &pdfcraft_engine::EditableObject| o.target.kind == pdfcraft_engine::ObjectKind::Path;
        let paths = if objects.iter().any(is_path) { doc.page_paths(page).map_err(failed)? } else { Vec::new() };
        let colour = |c: Option<[f64; 3]>| c.map(|c| crate::comments::hex(c.map(|v| v as f32)));
        let items = objects
            .iter()
            .map(|o| {
                let rect = crate::comments::rect_to_view(info, o.rect.map(|v| v as f32));
                let mut row = json!({"kind":o.target.kind.as_str(),"index":o.target.index.saturating_add(1),"type":o.content_type,
                "rect":rect.map(|v|(f64::from(v)*100.0).round()/100.0),"text":o.text});
                if let Some(p) = paths.get(o.target.index).filter(|_| is_path(o)) {
                    row["fill"] = json!(colour(p.fill_rgb));
                    row["stroke"] = json!(colour(p.stroke_rgb));
                    let width = (p.line_width_user * 100.0).round() / 100.0;
                    row["line_width"] = json!(p.paint.stroke.then_some(width));
                }
                row
            })
            .collect::<Vec<_>>();
        Ok(json!({"page":page+1,"generation":doc.edit_generation(),"count":items.len(),"objects":items}))
    }

    pub(crate) fn object_move(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let targets = object_targets(a)?;
        let delta = a
            .get("offset")
            .and_then(Value::as_array)
            .filter(|v| v.len() == 2)
            .and_then(|v| Some([v.first()?.as_f64()?, v.get(1)?.as_f64()?]))
            .filter(|v| v.iter().all(|n| n.is_finite() && n.abs() <= 1e9))
            .ok_or_else(|| bad("offset must contain two finite numbers [dx, dy] in displayed points"))?;
        let doc = self.doc(a)?;
        check_generation(a, doc, "moving")?;
        let offset = doc.object_move_offset(page, delta).map_err(failed)?;
        let count = targets.len();
        let mut result = self.apply(a, Edit::MoveObjects { page, objects: targets, offset })?;
        result["moved"] = json!(count);
        Ok(result)
    }

    pub(crate) fn object_align(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::ObjectAlign;
        let page = self.page(a)?;
        let targets = object_targets(a)?;
        let align = match a.get("action").and_then(Value::as_str) {
            Some("left") => ObjectAlign::Left,
            Some("centre" | "center") => ObjectAlign::CentreHorizontal,
            Some("right") => ObjectAlign::Right,
            Some("top") => ObjectAlign::Top,
            Some("middle") => ObjectAlign::CentreVertical,
            Some("bottom") => ObjectAlign::Bottom,
            Some("distribute_horizontal") => ObjectAlign::DistributeHorizontal,
            Some("distribute_vertical") => ObjectAlign::DistributeVertical,
            _ => return Err(bad("action must be left, centre, right, top, middle, bottom, distribute_horizontal or distribute_vertical")),
        };
        check_generation(a, self.doc(a)?, "aligning")?;
        let count = targets.len();
        let mut result = self.apply(a, Edit::AlignObjects { page, objects: targets, align })?;
        result["aligned"] = json!(count);
        Ok(result)
    }

    pub(crate) fn object_resize(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let target = a.get("object").ok_or_else(|| bad("object must be one path reference from object_list")).and_then(object_target)?;
        if target.kind != pdfcraft_engine::ObjectKind::Path {
            return Err(bad("only a vector path (kind path) can be resized"));
        }
        let r = a
            .nums::<4>("rect")?
            .filter(|r| r.iter().all(|v| v.abs() <= 1e9) && r[2] > r[0] && r[3] > r[1])
            .ok_or_else(|| bad("rect must be [x0, y0, x1, y1] in displayed points, with x1 > x0 and y1 > y0"))?;
        let doc = self.doc(a)?;
        check_generation(a, doc, "resizing")?;
        let info = doc.info.pages.get(page).ok_or_else(|| bad("the page no longer exists"))?;
        let rect = crate::comments::rect_to_user(info, r);
        self.apply(a, Edit::TransformPath { page, index: target.index, rect })
    }

    pub(crate) fn object_recolour(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let targets = object_targets(a)?;
        let fill = a.opt_str("fill")?.map(parse_color).transpose()?;
        let stroke = a.opt_str("stroke")?.map(parse_color).transpose()?;
        if fill.is_none() && stroke.is_none() {
            return Err(bad("give fill, stroke or both as #RRGGBB"));
        }
        check_generation(a, self.doc(a)?, "recolouring")?;
        let count = targets.len();
        let mut result = self.apply(a, Edit::RecolourObjects { page, objects: targets, fill, stroke })?;
        result["recoloured"] = json!(count);
        Ok(result)
    }

    pub(crate) fn object_delete(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let targets = object_targets(a)?;
        check_generation(a, self.doc(a)?, "deleting")?;
        let count = targets.len();
        let mut result = self.apply(a, Edit::DeleteObjects { page, objects: targets })?;
        result["deleted"] = json!(count);
        Ok(result)
    }

    pub(crate) fn object_arrange(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::ZOrder;
        let page = self.page(a)?;
        let object = a.get("object").ok_or_else(|| bad("object must be one path or image reference from object_list")).and_then(object_target)?;
        let order = match a.get("order").and_then(Value::as_str) {
            Some("front") => ZOrder::Front,
            Some("forward") => ZOrder::Forward,
            Some("backward") => ZOrder::Backward,
            Some("back") => ZOrder::Back,
            _ => return Err(bad("order must be front, forward, backward or back")),
        };
        let doc = self.doc(a)?;
        check_generation(a, doc, "arranging")?;
        // Paths and images are listed in painting order, so the reference changes with it.
        let moved = doc.arranged_target(page, object, order).map_err(failed)?;
        let mut result = self.apply(a, Edit::ArrangeObjects { page, object, order })?;
        result["object"] = json!({"kind":moved.kind.as_str(),"index":moved.index.saturating_add(1)});
        Ok(result)
    }
}

/// `objects`: 1–1000 `{kind, index}` references from object_list (1-based indexes).
pub(crate) fn object_targets(a: &Args) -> Result<Vec<pdfcraft_engine::ObjectTarget>> {
    a.get("objects")
        .and_then(Value::as_array)
        .filter(|r| !r.is_empty() && r.len() <= pdfcraft_engine::MAX_MOVE_OBJECTS)
        .ok_or_else(|| bad("objects must contain 1–1000 references from object_list"))?
        .iter()
        .map(object_target)
        .collect()
}

/// One `{kind, index}` reference from object_list (1-based index).
fn object_target(row: &Value) -> Result<pdfcraft_engine::ObjectTarget> {
    use pdfcraft_engine::{ObjectKind, ObjectTarget};
    let row = row.as_object().ok_or_else(|| bad("each object must contain kind and index"))?;
    if row.keys().any(|k| k != "kind" && k != "index") {
        return Err(bad("an object reference accepts only kind and index"));
    }
    let kind = match row.get("kind").and_then(Value::as_str) {
        Some("added") => ObjectKind::Added,
        Some("text") => ObjectKind::Text,
        Some("image") => ObjectKind::Image,
        Some("path") => ObjectKind::Path,
        _ => return Err(bad("object kind must be added, text, image or path (see object_list)")),
    };
    let index = row
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| usize::try_from(i).ok())
        .ok_or_else(|| bad("object index must be a positive integer from object_list"))?;
    Ok(ObjectTarget { kind, index })
}

/// The optional `generation` from object_list: refuse references to an older inventory.
fn check_generation(a: &Args, doc: &pdfcraft_engine::Document, doing: &str) -> Result<()> {
    if let Some(g) = a.get("generation") {
        let g = g.as_u64().ok_or_else(|| bad("generation must be a non-negative integer from object_list"))?;
        if g != doc.edit_generation() {
            return Err(failed(format!("the document changed; call object_list again before {doing}")));
        }
    }
    Ok(())
}
