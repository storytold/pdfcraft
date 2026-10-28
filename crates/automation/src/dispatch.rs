use super::error::{Result, ToolError, failed};
use super::json::{Args, document_attachment_keys, info, one_based, parse_style_ranges, security, summary};
use super::paths::{read_input, write_atomic};
use super::{Arc, Automation, Edit, Value, block_overlapping, comments, json, tools};

/// One piece of a tool's result.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// Structured data (MCP: a text block with the JSON plus `structuredContent`).
    Json(Value),
    /// A PNG image.
    Png { data: Vec<u8>, width: u32, height: u32 },
}

impl Automation {
    /// Run the tool `name` with JSON `args` (an object; `null` means no arguments).
    pub fn call(&mut self, name: &str, args: &Value) -> Result<Vec<Content>> {
        let empty = json!({});
        let args = if args.is_null() { &empty } else { args };
        if !args.is_object() {
            return Err(ToolError::InvalidArgs("arguments must be a JSON object".into()));
        }
        let def = tools::find(name).ok_or_else(|| ToolError::UnknownTool(name.into()))?;
        tools::check_args(def, args)?;
        let a = Args(args);
        let out = match name {
            "doc_open" => self.doc_open(&a)?,
            "doc_batch" => self.doc_batch(&a)?,
            "doc_list" => json!({ "documents": self.session.docs().iter().map(summary).collect::<Vec<_>>() }),
            "doc_info" => {
                let id = self.doc(&a)?.id;
                self.session.refresh_fonts(id);
                info(self.doc(&a)?)
            }
            "doc_close" => self.doc_close(&a)?,
            "doc_save" => self.doc_save(&a)?,
            "doc_set_info" => {
                let edit = Edit::SetInfo { key: a.str("key")?.into(), value: a.str("value")?.into() };
                self.apply(&a, edit)?
            }
            "page_render" => return self.page_render(&a).map(|c| vec![c]),
            "comment_image_preview" => return self.comment_image_preview(&a),
            "text_extract" => self.text_extract(&a)?,
            "text_word_count" => self.text_word_count(&a)?,
            "text_find" => self.text_find(&a)?,
            "page_rotate" => {
                let degrees = a.int("degrees")?;
                if degrees % 90 != 0 {
                    return Err(ToolError::InvalidArgs("degrees must be a multiple of 90".into()));
                }
                let doc = self.doc(&a)?;
                let base = match a.opt_ints("pages")? {
                    Some(_) => self.pages(&a, "pages")?,
                    None => (0..doc.info.pages.len()).collect(),
                };
                let parity = match a.opt_str("subset")?.unwrap_or("all") {
                    "all" => pdfcraft_engine::PageParity::Both,
                    "even" => pdfcraft_engine::PageParity::Even,
                    "odd" => pdfcraft_engine::PageParity::Odd,
                    s => return Err(ToolError::InvalidArgs(format!("unknown subset {s:?} (all, even, odd)"))),
                };
                let orientation = match a.opt_str("orientation")?.unwrap_or("all") {
                    "all" => pdfcraft_engine::PageOrientation::Both,
                    "landscape" => pdfcraft_engine::PageOrientation::Landscape,
                    "portrait" => pdfcraft_engine::PageOrientation::Portrait,
                    o => return Err(ToolError::InvalidArgs(format!("unknown orientation {o:?} (all, landscape, portrait)"))),
                };
                let pages = pdfcraft_engine::filter_pages(&self.doc(&a)?.info, &base, parity, orientation);
                if pages.is_empty() {
                    return Err(failed("no pages match the filters"));
                }
                let n = pages.len();
                let mut out = self.apply(&a, Edit::RotatePages { pages, degrees })?;
                out["rotated"] = json!(n);
                out
            }
            "page_enhance_camera" => {
                let page = self.page(&a)?;
                let given = a.nums::<8>("corners")?;
                let doc = self.doc(&a)?;
                let (id, info) = (doc.id, doc.info.pages.get(page).cloned().ok_or_else(|| failed("no such page"))?);
                let corners = match given {
                    Some(c) => [0, 1, 2, 3].map(|i| {
                        let [x, y] = info.view_to_user(c[2 * i] as f32, c[2 * i + 1] as f32);
                        [x as f64, y as f64]
                    }),
                    None => self.session.detect_camera_corners(id, page).ok_or_else(|| {
                        failed("the document's edges could not be found on this page: pass `corners` [x1, y1, x2, y2, x3, y3, x4, y4]")
                    })?,
                };
                let level = match a.opt_int("level")? {
                    None => 0,
                    Some(l @ 0..=100) => l as u8,
                    Some(l) => return Err(ToolError::InvalidArgs(format!("level must be between 0 and 100, not {l}"))),
                };
                let look = match a.opt_str("mode")? {
                    None => pdfcraft_engine::camera_clean::Look::Colour,
                    Some(m) => pdfcraft_engine::camera_clean::Look::from_name(m)
                        .ok_or_else(|| ToolError::InvalidArgs(format!("unknown mode {m:?} (original, colour, greyscale, bw)")))?,
                };
                self.session.enhance_camera_page_at(id, page, corners, look, level).map_err(failed)?;
                let r = |v: f32| (v * 100.0).round() / 100.0;
                let used: Vec<f32> = corners.iter().flat_map(|&[x, y]| info.user_to_view(x as f32, y as f32)).map(r).collect();
                let mut out = summary(self.doc(&a)?);
                out["corners"] = json!(used);
                out["detected"] = json!(given.is_none());
                out
            }
            "page_delete" => {
                let pages = self.pages(&a, "pages")?;
                self.apply(&a, Edit::DeletePages { pages })?
            }
            "page_move" => {
                let pages = self.pages(&a, "pages")?;
                let to = self.position(&a, "to")?;
                self.apply(&a, Edit::MovePages { pages, to })?
            }
            "page_reverse" => {
                let pages = match a.opt_ints("pages")? {
                    Some(_) => self.pages(&a, "pages")?,
                    None => (0..self.doc(&a)?.info.pages.len()).collect(),
                };
                self.apply(&a, Edit::ReversePages { pages })?
            }
            "page_insert_blank" => self.insert_blank(&a)?,
            "page_insert_file" => self.insert_file(&a)?,
            "page_extract" => self.page_extract(&a)?,
            "doc_combine" => self.doc_combine(&a)?,
            "doc_create_multiple" => self.doc_create_multiple(&a)?,
            "doc_split" => self.doc_split(&a)?,
            "edit_undo" => {
                let id = self.doc(&a)?.id;
                let label = self.session.undo(id).map_err(failed)?.to_string();
                json!({ "undone": label, "document": summary(self.doc(&a)?) })
            }
            "edit_redo" => {
                let id = self.doc(&a)?.id;
                let label = self.session.redo(id).map_err(failed)?.to_string();
                json!({ "redone": label, "document": summary(self.doc(&a)?) })
            }
            "command_list" => self.command_list(&a)?,
            "command_run" => return self.command_run(&a),
            "command_batch" => self.command_batch(&a)?,
            "doc_inspect" => self.doc_inspect(&a)?,
            "render_preview" => return self.render_preview(&a).map(|c| vec![c]),
            "page_number" => {
                use pdfcraft_organize::LabelStyle as L;
                let n = self.doc(&a)?.info.pages.len();
                let (from, to) = (a.int("from")?, a.int("to")?);
                if from < 1 || to < from || to as usize > n {
                    return Err(ToolError::InvalidArgs(format!("from and to must satisfy 1 ≤ from ≤ to ≤ {n}")));
                }
                let style = match a.opt_str("style")?.unwrap_or("decimal") {
                    "decimal" => L::Decimal,
                    "upper-roman" => L::UpperRoman,
                    "lower-roman" => L::LowerRoman,
                    "upper-alpha" => L::UpperAlpha,
                    "lower-alpha" => L::LowerAlpha,
                    "none" => L::None,
                    other => return Err(ToolError::InvalidArgs(format!("unknown style {other:?}"))),
                };
                let prefix = a.opt_str("prefix")?.unwrap_or_default().to_string();
                let first = a.opt_int("start")?.unwrap_or(1).clamp(1, u32::MAX as i64) as u32;
                let mut out = self.apply(&a, Edit::NumberPages { from: from as usize - 1, to: to as usize - 1, style, prefix, first })?;
                out["labels"] = json!(self.doc(&a)?.info.pages.iter().map(|p| p.label.clone()).collect::<Vec<_>>());
                out
            }
            "bookmark_list" => self.bookmark_listing(&a)?,
            "bookmark_add" => {
                let page = self.page(&a)?;
                let parent = a.opt_path("parent")?.unwrap_or_default();
                let index = a.opt_int("position")?.map_or(usize::MAX, |p| (p.max(1) - 1) as usize);
                let title = a.str("title")?.to_string();
                let (left, top) = (a.bookmark_coordinate("left")?, a.bookmark_coordinate("top")?);
                let [left, top] = match self.doc(&a)?.info.pages.get(page) {
                    Some(p) => p.xyz_for_view(left, top),
                    None => [None, None],
                };
                let (left, top) = (left.map(f64::from), top.map(f64::from));
                self.apply(&a, Edit::AddBookmark { parent, index, title, page, left, top })?
            }
            "bookmark_rename" => {
                let (path, title) = (a.path("path")?, a.str("title")?.to_string());
                self.apply(&a, Edit::RenameBookmark { path, title })?
            }
            "bookmark_delete" => {
                let path = a.path("path")?;
                self.apply(&a, Edit::DeleteBookmark { path })?
            }
            "bookmark_move" => {
                let from = a.path("path")?;
                let to_parent = a.opt_path("parent")?.unwrap_or_default();
                let index = a.opt_int("position")?.map_or(usize::MAX, |p| (p.max(1) - 1) as usize);
                self.apply(&a, Edit::MoveBookmark { from, to_parent, index })?
            }
            "bookmark_set_page" => {
                let (path, page) = (a.path("path")?, self.page(&a)?);
                self.apply(&a, Edit::SetBookmarkPage { path, page })?
            }
            "bookmark_set_style" => {
                let path = a.path("path")?;
                // Left out keeps the colour; [0, 0, 0] is the default black.
                let color = match a.get("color") {
                    None => None,
                    Some(v) => {
                        let rgb: Option<Vec<f64>> = v.as_array().and_then(|c| c.iter().map(Value::as_f64).collect());
                        match rgb.as_deref() {
                            Some(&[r, g, b]) if [r, g, b].iter().all(|c| (0.0..=1.0).contains(c)) => Some([r, g, b]),
                            _ => return Err(ToolError::InvalidArgs("color must be [r, g, b], each from 0 to 1".into())),
                        }
                    }
                };
                let (bold, italic) = (a.opt_bool("bold")?, a.opt_bool("italic")?);
                if color.is_none() && bold.is_none() && italic.is_none() {
                    return Err(ToolError::InvalidArgs("give color, bold or italic".into()));
                }
                let mut edits = Vec::new();
                if let Some(color) = color {
                    edits.push(Edit::SetBookmarkColor { path: path.clone(), color: Some(color) });
                }
                if bold.is_some() || italic.is_some() {
                    edits.push(Edit::SetBookmarkStyle { path, bold, italic });
                }
                // A colour and a style given together are one step to undo, applied all or nothing.
                match edits.len() {
                    1 => self.apply(&a, edits.remove(0))?,
                    _ => self.apply(&a, Edit::Batch { label: "Style".into(), edits })?,
                }
            }
            "bookmark_from_structure" => {
                let mut out = self.apply(&a, Edit::BookmarksFromStructure)?;
                let listing = self.bookmark_listing(&a)?;
                out["bookmarks"] = listing["bookmarks"].clone();
                out["next"] = listing["next"].clone();
                out
            }
            "doc_protect" => self.doc_protect(&a)?,
            "page_replace" => {
                let pages = self.pages(&a, "pages")?;
                let path = self.resolve(a.str("path")?, false)?;
                let bytes = Arc::new(read_input(&path)?);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let src_pages = match a.opt_ints("from_pages")? {
                    Some(p) => one_based(&p)?,
                    None => (0..pages.len()).collect(),
                };
                self.apply(&a, Edit::ReplacePages { pages, name, bytes, src_pages })?
            }
            "page_duplicate" => {
                let pages = self.pages(&a, "pages")?;
                let at = self.opt_position(&a, "at")?;
                self.apply(&a, Edit::DuplicatePages { pages, at })?
            }
            "page_set_box" => self.page_set_box(&a)?,
            "page_transition_set" => self.page_transition_set(&a)?,
            "doc_create" => self.doc_create(&a)?,
            "xfa_to_static" => self.apply(&a, Edit::XfaToStatic)?,
            "doc_flatten" => {
                let (comments, fields) = (a.opt_bool("comments")?.unwrap_or(true), a.opt_bool("fields")?.unwrap_or(true));
                if !comments && !fields {
                    return Err(ToolError::InvalidArgs("nothing to flatten".into()));
                }
                self.apply(&a, Edit::Flatten { comments, fields })?
            }
            "doc_reduce" => {
                let id = self.doc(&a)?.id;
                let before = self.doc(&a)?.bytes.len();
                let path = self.resolve(a.str("path")?, true)?;
                let (bytes, merged) = self.session.reduced_bytes(id).map_err(failed)?;
                // As in the app, a copy that isn't smaller is not written (#490).
                let written = bytes.len() < before;
                if written {
                    write_atomic(&path, &bytes)?;
                }
                json!({ "path": path.to_string_lossy(), "written": written, "bytes_before": before, "bytes_after": bytes.len(), "merged_objects": merged })
            }
            "doc_optimize" => self.doc_optimize(&a)?,
            "doc_initial_view" => self.doc_initial_view(&a)?,
            "doc_revisions" => self.doc_revisions(&a)?,
            "text_lines" => {
                let page = self.page(&a)?;
                let doc = self.doc_to_copy(&a)?;
                let info = &doc.info.pages[page];
                let r = |x: f32| (x as f64 * 100.0).round() / 100.0;
                let lines: Vec<Value> = doc
                    .text_lines(page)
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let (u, v) = (info.user_to_view(l.rect[0] as f32, l.rect[1] as f32), info.user_to_view(l.rect[2] as f32, l.rect[3] as f32));
                        json!({
                            "line": i + 1,
                            "text": l.text,
                            "rect": [r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))],
                            "font": l.base_font,
                            "size": (l.size * 100.0).round() / 100.0,
                        })
                    })
                    .collect();
                json!({ "page": page + 1, "count": lines.len(), "lines": lines })
            }
            "image_hit" => {
                let page = self.page(&a)?;
                let point = a
                    .get("point")
                    .and_then(Value::as_array)
                    .filter(|p| p.len() == 2)
                    .and_then(|p| Some([p.first()?.as_f64()?, p.get(1)?.as_f64()?]))
                    .filter(|p| p.iter().all(|v| v.is_finite() && v.abs() <= 1e9))
                    .ok_or_else(|| ToolError::InvalidArgs("point must contain two finite coordinates".into()))?;
                let doc = self.doc(&a)?;
                let p = &doc.info.pages[page];
                let user = p.view_to_user(point[0] as f32, point[1] as f32).map(f64::from);
                let images: Vec<_> = doc.page_images(page).iter().enumerate().filter(|(_, im)| im.contains(user)).map(|(i, _)| i + 1).collect();
                json!({"page":page+1,"images":images})
            }
            "page_images" => {
                let page = self.page(&a)?;
                let doc = self.doc(&a)?;
                let info = &doc.info.pages[page];
                let r = |x: f32| (x as f64 * 100.0).round() / 100.0;
                let list: Vec<Value> = doc
                    .page_images(page)
                    .iter()
                    .enumerate()
                    .map(|(i, im)| {
                        let (u, v) = (info.user_to_view(im.rect[0] as f32, im.rect[1] as f32), info.user_to_view(im.rect[2] as f32, im.rect[3] as f32));
                        let visible = im.visible_rect.map(|rect| {
                            let (u, v) = (info.user_to_view(rect[0] as f32, rect[1] as f32), info.user_to_view(rect[2] as f32, rect[3] as f32));
                            [r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))]
                        });
                        json!({ "visible_rect": visible, "image": i + 1, "rect": [r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))], "pixels": [im.width, im.height], "name": im.name, "kind": if im.is_form { "form" } else { "image" } })
                    })
                    .collect();
                json!({ "page": page + 1, "count": list.len(), "images": list })
            }
            "image_edit" | "image_save" => {
                let page = self.page(&a)?;
                let n = self.doc(&a)?.page_images(page).len();
                let k = a.int("image")?;
                if k < 1 || k as usize > n {
                    return Err(ToolError::InvalidArgs(format!("image {k} is out of range: page {} has {n} images", page + 1)));
                }
                let index = k as usize - 1;
                if name == "image_save" {
                    let (ext, bytes) = self.doc(&a)?.page_image_file(page, index).map_err(failed)?;
                    let mut path = self.resolve(a.str("path")?, true)?;
                    if path.is_dir() {
                        // Adding the extension to a folder's name would write beside it: "." is the
                        // root, so that would land outside it.
                        return Err(failed(format!("{}: is a folder, not a file", path.display())));
                    }
                    if path.extension().is_none() {
                        path.set_extension(ext);
                    }
                    write_atomic(&path, &bytes)?;
                    json!({ "path": path.to_string_lossy(), "format": ext, "bytes": bytes.len() })
                } else {
                    use pdfcraft_engine::ImageEdit;
                    let change = match a.str("action")? {
                        "move" => {
                            let r: Vec<f64> =
                                a.get("rect").and_then(Value::as_array).map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                            let r = <[f64; 4]>::try_from(r).map_err(|_| ToolError::InvalidArgs("move needs rect: 4 numbers".into()))?;
                            // Top-left-origin points → user space.
                            let info = &self.doc(&a)?.info.pages[page];
                            let (u0, u1) = (info.view_to_user(r[0] as f32, r[1] as f32), info.view_to_user(r[2] as f32, r[3] as f32));
                            ImageEdit::Move([u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64])
                        }
                        "rotate_by" => ImageEdit::RotateBy(
                            a.get("degrees")
                                .and_then(Value::as_f64)
                                .ok_or_else(|| ToolError::InvalidArgs("rotate_by needs degrees: a number (clockwise)".into()))?,
                        ),
                        "rotate" => ImageEdit::Rotate(a.opt_int("quarters")?.unwrap_or(1) as i32),
                        "crop" => {
                            let r: Vec<f64> =
                                a.get("rect").and_then(Value::as_array).map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                            let r =
                                <[f64; 4]>::try_from(r).map_err(|_| ToolError::InvalidArgs("crop needs rect: 4 numbers, the part to keep".into()))?;
                            let info = &self.doc(&a)?.info.pages[page];
                            let (u0, u1) = (info.view_to_user(r[0] as f32, r[1] as f32), info.view_to_user(r[2] as f32, r[3] as f32));
                            ImageEdit::Crop([u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64])
                        }
                        "flip_horizontal" => ImageEdit::Flip { horizontal: true },
                        "flip_vertical" => ImageEdit::Flip { horizontal: false },
                        "delete" => ImageEdit::Delete,
                        "replace" => {
                            let path = self.resolve(a.str("path")?, false)?;
                            let bytes = read_input(&path)?;
                            ImageEdit::Replace {
                                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                bytes: Arc::new(bytes),
                            }
                        }
                        other => return Err(ToolError::InvalidArgs(format!("unknown action {other:?}"))),
                    };
                    self.apply(&a, Edit::EditPageImage { page, index, change })?
                }
            }
            "text_paragraphs" => {
                let page = self.page(&a)?;
                let doc = self.doc_to_copy(&a)?;
                let info = &doc.info.pages[page];
                let r = |x: f32| (x as f64 * 100.0).round() / 100.0;
                let current = doc.text_blocks(page);
                let id = doc.id;
                let blocks: Vec<Value> = current
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let (u, v) = (info.user_to_view(b.rect[0] as f32, b.rect[1] as f32), info.user_to_view(b.rect[2] as f32, b.rect[3] as f32));
                        json!({
                            "paragraph": i + 1,
                            "text": b.text,
                            "editor_text": b.editor_text,
                            "rows": b.rows.iter().map(|row| row.iter().map(|i| i + 1).collect::<Vec<_>>()).collect::<Vec<_>>(),
                            "lines": b.lines.iter().map(|l| l + 1).collect::<Vec<_>>(),
                            "rect": [r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))],
                            "font": b.base_font,
                            "size": (b.size * 100.0).round() / 100.0,
                        })
                    })
                    .collect();
                self.record_listing(id, page, &current);
                json!({ "page": page + 1, "count": blocks.len(), "paragraphs": blocks })
            }
            "text_font_files" => {
                let roots = if let Some(paths) = a.get("directories") {
                    let paths = paths
                        .as_array()
                        .filter(|p| p.len() <= 16)
                        .ok_or_else(|| ToolError::InvalidArgs("directories must be at most 16 paths".into()))?;
                    paths
                        .iter()
                        .map(|p| self.resolve(p.as_str().ok_or_else(|| ToolError::InvalidArgs("directory must be a path".into()))?, false))
                        .collect::<Result<Vec<_>>>()?
                } else if let Some(roots) = &self.roots {
                    // The installed fonts are outside the roots: look in the read root instead.
                    let read = roots
                        .read_root()
                        .ok_or_else(|| failed("automation filesystem access is not granted: read authority is absent: installed fonts"))?;
                    vec![self.resolve(&read.to_string_lossy(), false)?]
                } else if let Some(root) = &self.root {
                    vec![root.clone()]
                } else {
                    pdfcraft_engine::text_fonts::installed_directories()
                };
                json!({"fonts": pdfcraft_engine::text_fonts::discover(&roots).into_iter().map(|f| json!({"path":f.path,"family":f.family,"font":f.face})).collect::<Vec<_>>()})
            }
            "text_font_import" => {
                let page = self.page(&a)?;
                let path = self.resolve(a.str("path")?, false)?;
                let file = std::fs::File::open(&path).map_err(failed)?;
                use std::io::Read;
                let mut bytes = Vec::new();
                file.take((16 << 20) + 1).read_to_end(&mut bytes).map_err(failed)?;
                if bytes.len() > 16 << 20 {
                    return Err(ToolError::InvalidArgs("font input exceeds 16 MiB".into()));
                }
                let before: Vec<_> = self.doc(&a)?.text_editor_fonts(page).into_iter().map(|f| f.resource).collect();
                let mut out = self.apply(&a, Edit::RegisterTextFont { page, bytes: Arc::new(bytes) })?;
                let added = self
                    .doc(&a)?
                    .text_editor_fonts(page)
                    .into_iter()
                    .find(|f| !before.contains(&f.resource) && !f.resource.starts_with("PCApproved"));
                if let Some(f) = added {
                    out["resource"] = json!(f.resource);
                    out["font"] = json!(f.face);
                }
                out
            }
            "text_fonts" => {
                let page = self.page(&a)?;
                json!({"fonts": self.doc(&a)?.text_editor_fonts(page).into_iter().map(|f| json!({"resource":f.resource,"font":f.face,"full_face":f.resource.starts_with("PCFull"),"preview_only":f.resource.starts_with("PCApproved")})).collect::<Vec<_>>()})
            }
            "text_edit_layout" => {
                let page = self.page(&a)?;
                let doc = self.doc(&a)?;
                let k = a.int("paragraph")?;
                let blocks = doc.text_blocks(page);
                let index = self.paragraph_index(doc.id, page, k, &blocks)?;
                let source = blocks.get(index).ok_or_else(|| ToolError::InvalidArgs("paragraph is out of range".into()))?;
                let text = a.opt_str("text")?.unwrap_or(&source.editor_text);
                let style = pdfcraft_engine::BlockStyle {
                    size: a.opt_num("size")?,
                    width: a.opt_num("width")?,
                    resource: a.opt_str("font_resource")?.map(str::to_owned),
                    ranges: parse_style_ranges(a.get("ranges"))?,
                    ..Default::default()
                };
                let layout = doc.text_edit_layout(page, index, text, &style).map_err(failed)?;
                let info = &doc.info.pages[page];
                let view_rect = |r: [f64; 4]| {
                    let a = info.user_to_view(r[0] as f32, r[1] as f32);
                    let b = info.user_to_view(r[2] as f32, r[3] as f32);
                    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
                };
                let fonts: Vec<_> = doc.text_editor_fonts(page).into_iter().map(|f| json!({"resource": f.resource, "face": f.face})).collect();
                json!({ "page": page + 1, "paragraph": k, "source_rect": view_rect(layout.source_rect),
                    "draft_rect": layout.draft_rect.map(view_rect), "overflow": layout.overflow, "embedded_fonts": fonts })
            }
            "text_edit" if a.get("paragraph").is_some() => {
                let page = self.page(&a)?;
                let k = a.int("paragraph")?;
                let (id, blocks) = {
                    let doc = self.doc(&a)?;
                    (doc.id, doc.text_blocks(page))
                };
                let index = self.paragraph_index(id, page, k, &blocks)?;
                let block = blocks.get(index).cloned().ok_or_else(|| ToolError::InvalidArgs(format!("paragraph {k} is out of range")))?;
                let text = a.opt_str("text")?.map(str::to_owned).unwrap_or(block.text);
                let mut style = pdfcraft_engine::BlockStyle {
                    ranges: parse_style_ranges(a.get("ranges"))?,
                    resource: a.opt_str("font_resource")?.map(str::to_owned),
                    bold: a.opt_bool("bold")?,
                    size: a.opt_num("size")?,
                    underline: a.opt_bool("underline")?,
                    line_spacing: a.opt_num("line_spacing")?,
                    char_spacing: a.opt_num("char_spacing")?,
                    scale: a.opt_num("scale")?,
                    width: a.opt_num("width")?,
                    ..Default::default()
                };
                let (dx, dy) = (a.opt_num("dx")?, a.opt_num("dy")?);
                if dx.is_some() || dy.is_some() {
                    style.offset = Some([dx.unwrap_or(0.0), dy.unwrap_or(0.0)]);
                }
                if let Some(f) = a.opt_str("font")? {
                    let family = match f {
                        "helvetica" => pdfcraft_engine::FontFamily::Helvetica,
                        "times" => pdfcraft_engine::FontFamily::Times,
                        "courier" => pdfcraft_engine::FontFamily::Courier,
                        other => return Err(ToolError::InvalidArgs(format!("unknown font {other:?} (helvetica, times, courier)"))),
                    };
                    style.family = Some((family, a.opt_bool("bold")?.unwrap_or(false), a.opt_bool("italic")?.unwrap_or(false)));
                }
                if let Some(c) = a.opt_str("color")? {
                    style.color = Some(comments::parse_color(c)?);
                }
                if let Some(al) = a.opt_str("align")? {
                    style.align = Some(match al {
                        "left" => pdfcraft_engine::TextAlign::Left,
                        "center" => pdfcraft_engine::TextAlign::Center,
                        "right" => pdfcraft_engine::TextAlign::Right,
                        "justify" => pdfcraft_engine::TextAlign::Justify,
                        other => return Err(ToolError::InvalidArgs(format!("unknown align {other:?}"))),
                    });
                }
                // Where the paragraph is drawn after the edit: moved by dx, dy (up is +dy).
                let [dx, dy] = style.offset.unwrap_or([0.0, 0.0]);
                let moved = [block.rect[0] + dx, block.rect[1] + dy, block.rect[2] + dx, block.rect[3] + dy];
                let wanted: String = text.split_whitespace().collect();
                let mut out = self.apply(&a, Edit::EditTextBlock { page, block: index, text, style })?;
                let doc = self.doc(&a)?;
                let after = doc.text_blocks(page);
                // Without `text` the paragraph keeps the document's own words: not echoed when
                // the document doesn't allow copying them.
                let copy = doc.allows_copying();
                // Only a paragraph that now shows exactly the requested text is the edited one. A
                // neighbour that overlaps the old box (overlaid text, a text layer) must not take
                // its number, and neither must a paragraph the edit merged it into: rewriting that
                // one by this number would drop the neighbour's text.
                let edited = block_overlapping(&after, moved)
                    .filter(|&i| !wanted.is_empty() && after.get(i).is_some_and(|b| b.text.split_whitespace().collect::<String>() == wanted));
                if let Some(b) = edited.and_then(|i| after.get(i)) {
                    out["paragraph"] = json!({ "lines": b.lines.len(), "font": b.base_font });
                    if copy {
                        out["paragraph"]["text"] = json!(b.text);
                    }
                }
                let stale = self.follow_edit(id, page, k, edited.and_then(|i| after.get(i)), &after);
                if !stale.is_empty() {
                    out["stale_paragraphs"] = json!(stale);
                }
                out
            }
            "text_edit" => {
                let page = self.page(&a)?;
                let n = self.doc(&a)?.text_lines(page).len();
                let line = a.int("line")?;
                if line < 1 || line as usize > n {
                    return Err(ToolError::InvalidArgs(format!("line {line} is out of range: page {} has {n} lines", page + 1)));
                }
                let text = a.str("text")?.to_owned();
                let mut out = self.apply(&a, Edit::EditTextLine { page, line: line as usize - 1, text })?;
                let after = self.doc(&a)?.text_lines(page);
                if let Some(l) = after.get(line as usize - 1) {
                    out["line"] = json!({ "text": l.text, "font": l.base_font });
                }
                out
            }
            "doc_audit_space" => {
                let rows: Vec<Value> = self
                    .doc(&a)?
                    .audit_space()
                    .iter()
                    .map(|u| json!({ "category": u.category.label(), "bytes": u.bytes, "percent": (u.percent * 100.0).round() / 100.0 }))
                    .collect();
                json!({ "categories": rows })
            }
            "doc_open_revision" => {
                let id = self.doc(&a)?.id;
                let n = usize::try_from(a.opt_int("revision")?.ok_or_else(|| ToolError::InvalidArgs("revision is required".into()))?).unwrap_or(0);
                let new = self.session.open_revision(id, n).map_err(failed)?;
                summary(self.session.get(new).ok_or_else(|| failed("the document vanished"))?)
            }
            "doc_export_images" | "doc_export_text" | "doc_export_all_images" => self.export(name, &a)?,
            "doc_header_footer" | "doc_watermark" | "doc_background" | "doc_remove_marks" => self.marks(name, &a)?,
            "doc_unprotect" => {
                let mut out = self.apply(&a, Edit::RemoveProtection)?;
                out["security"] = security(self.doc(&a)?);
                out
            }
            "form_fields" => self.form_fields(&a)?,
            "form_fill" => self.form_fill(&a)?,
            "form_set_image" => {
                let path = self.resolve(a.str("path")?, false)?;
                let bytes = read_input(&path)?;
                self.apply(&a, Edit::SetFieldImage { name: a.str("field")?.to_owned(), image: Arc::new(bytes) })?
            }
            "form_reset" => self.form_reset(&a)?,
            "form_add_field" => self.form_add_field(&a)?,
            "form_add_field_table" => self.form_add_field_table(&a)?,
            "form_set_props" => self.form_set_props(&a)?,
            "form_delete_field" => self.form_delete_field(&a)?,
            "form_tab_order" => self.form_tab_order(&a)?,
            "form_calculation_order" => self.form_calculation_order(&a)?,
            "doc_export_data" => self.doc_export_data(&a)?,
            "doc_import_data" => self.doc_import_data(&a)?,
            "redact_mark" => self.redact_mark(&a)?,
            "redact_apply" => self.redact_apply(&a)?,
            "redact_clear" => self.redact_clear(&a)?,
            "doc_hidden_info" => self.doc_hidden_info(&a)?,
            "printers" => self.printers()?,
            "printer_options" => self.printer_options(&a)?,
            "link_list" => self.link_list(&a)?,
            "link_open" => self.link_open(&a)?,
            "link_add" => self.link_add(&a)?,
            "link_edit" => self.link_edit(&a)?,
            "link_delete" => self.link_delete(&a)?,
            "links_from_urls" => self.links_from_urls(&a)?,
            "links_remove" => self.links_remove(&a)?,
            "object_list" => self.object_list(&a)?,
            "object_move" => self.object_move(&a)?,
            "content_list" => self.content_list(&a)?,
            "page_add_text" => self.page_add_text(&a)?,
            "page_add_image" => self.page_add_image(&a)?,
            "content_update" => self.content_update(&a)?,
            "content_delete" => self.content_delete(&a)?,
            "content_copy" => self.content_copy(&a)?,
            "doc_print" => self.doc_print(&a)?,
            "attachment_add" => {
                let path = self.resolve(a.str("path")?, false)?;
                let bytes = read_input(&path)?;
                let name = match a.opt_str("name")? {
                    Some(n) => n.to_string(),
                    None => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                };
                let mime = pdfcraft_engine::mime_for_name(&name).map(str::to_string);
                let description = a.opt_str("description")?.map(str::to_string);
                // Only the document-level keys: an annotation attachment of the same name is not the new one.
                let before = document_attachment_keys(self.doc(&a)?);
                let out = self.apply(&a, Edit::AddAttachment { name, bytes, description, mime })?;
                let key = document_attachment_keys(self.doc(&a)?).into_iter().find(|n| !before.contains(n)).unwrap_or_default();
                json!({ "name": key, "document": out })
            }
            "attachment_delete" => {
                let edit = Edit::DeleteAttachment { key: a.str("name")?.into() };
                self.apply(&a, edit)?
            }
            "doc_remove_hidden" => self.doc_remove_hidden(&a)?,
            "doc_audit_redactions" => self.doc_audit_redactions(&a)?,
            "fill_sign_add" => self.fill_sign_add(&a)?,
            "fill_sign_date_format" => self.fill_sign_date_format(&a)?,
            "measure_distance" => self.measurement_add(&a, pdfcraft_engine::measure::Kind::Distance)?,
            "measure_perimeter" => self.measurement_add(&a, pdfcraft_engine::measure::Kind::Perimeter)?,
            "measure_area" => self.measurement_add(&a, pdfcraft_engine::measure::Kind::Area)?,
            "measure_info" => self.measurement_info(&a)?,
            "measure_list" => self.measurement_list(&a)?,
            "measure_scale" => self.measurement_scale(&a)?,
            "measure_snap" => self.measurement_snap(&a)?,
            "measure_export" => self.measurement_export(&a)?,
            "comment_list" => self.comment_list(&a)?,
            "comment_add" => self.comment_add(&a)?,
            "stamp_custom" => self.stamp_custom(&a)?,
            "comment_reply" => self.comment_reply(&a)?,
            "comment_set_status" => self.comment_set_status(&a)?,
            "sign_list" => self.sign_list(&a)?,
            "accessibility_check" => self.a11y_check(&a)?,
            "accessibility_find_untagged" => self.a11y_find_untagged(&a)?,
            "accessibility_autotag_preview" => self.accessibility_autotag_preview(&a)?,
            "accessibility_autotag_apply" => self.accessibility_autotag_apply(&a)?,
            "accessibility_autotag_form_fields" => self.accessibility_autotag_form_fields(&a)?,
            "ocr_recognize" => self.ocr_recognize(&a)?,
            "js_run" => self.js_run(&a)?,
            "js_document_scripts" => self.js_document_scripts(&a)?,
            "js_set_document_script" => self.js_set_document_script(&a)?,
            "js_enabled" => self.js_enabled(&a)?,
            "form_set_script" => self.form_set_script(&a)?,
            "form_merge_data" => self.form_merge_data(&a)?,
            "form_actions" => self.form_actions(&a)?,
            "form_detect_fields" => self.form_detect_fields(&a)?,
            "doc_compare" => self.doc_compare(&a)?,
            "action_list" => self.action_list()?,
            "pdfa_verify" => self.pdfa_verify(&a)?,
            "print_readiness" => self.print_readiness(&a)?,
            "doc_export_office" => self.doc_export_office(&a)?,
            "pdfa_convert" => self.pdfa_convert(&a)?,
            "standards_remove_id" => {
                let standard = match a.str("standard")? {
                    "pdfa" => pdfcraft_engine::Standard::PdfA,
                    "pdfx" => pdfcraft_engine::Standard::PdfX,
                    other => return Err(ToolError::InvalidArgs(format!("standard must be pdfa or pdfx, not {other}"))),
                };
                self.apply(&a, Edit::RemoveStandardId { standard })?
            }
            "action_run" => self.action_run(&a)?,
            "doc_compare_report" => self.doc_compare_report(&a)?,
            "doc_compare_mark" => self.doc_compare_mark(&a)?,
            "form_set_actions" => self.form_set_actions(&a)?,
            "ocr_status" => self.ocr_status()?,
            "ocr_recognize_files" => self.ocr_recognize_files(&a)?,
            "accessibility_report" => self.a11y_report(&a)?,
            "accessibility_review_record" => self.a11y_review_record(&a)?,
            "accessibility_review_save" => self.a11y_review_save(&a)?,
            "accessibility_review_load" => self.a11y_review_load(&a)?,
            "accessibility_review_html" => self.a11y_review_html(&a)?,
            "accessibility_fix" => self.a11y_fix(&a)?,
            "accessibility_figures" => self.accessibility_figures(&a)?,
            "accessibility_set_alt" => self.accessibility_set_alt(&a)?,
            "accessibility_tags" => self.accessibility_tags(&a)?,
            "accessibility_tag_content" => self.accessibility_tag_content(&a)?,
            "accessibility_set_tag_property" => self.accessibility_set_tag_property(&a)?,
            "accessibility_mark_tag_artifact" => self.accessibility_mark_tag_artifact(&a)?,
            "accessibility_move_tag" => self.accessibility_move_tag(&a)?,
            "sign_id_create" => self.sign_id_create(&a)?,
            "sign_document" => self.sign_document(&a)?,
            "sign_keychain_ids" => {
                #[cfg(target_os = "macos")]
                let ids: Vec<Value> = pdfcraft_engine::sign::keychain::identities(None)
                    .map_err(failed)?
                    .iter()
                    .map(|id| json!({ "id": pdfcraft_engine::sign::keychain::reference(&id.certificate), "certificate": super::signing::cert_json(&id.certificate) }))
                    .collect();
                #[cfg(not(target_os = "macos"))]
                let ids: Vec<Value> = Vec::new();
                json!({ "count": ids.len(), "ids": ids })
            }
            "sign_windows_ids" => {
                #[cfg(target_os = "windows")]
                let (ids, unusable): (Vec<Value>, Vec<Value>) = {
                    let listing = pdfcraft_engine::sign::windows::list().map_err(failed)?;
                    let ids = listing
                        .ids
                        .iter()
                        .map(|id| json!({ "id": pdfcraft_engine::sign::windows::reference(&id.certificate), "certificate": super::signing::cert_json(&id.certificate) }))
                        .collect();
                    let unusable = listing
                        .unusable
                        .iter()
                        .map(|u| json!({ "subject": u.subject, "sha256": u.fingerprint, "reason": u.reason, "no_private_key": u.no_private_key }))
                        .collect();
                    (ids, unusable)
                };
                #[cfg(not(target_os = "windows"))]
                let (ids, unusable): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());
                json!({ "count": ids.len(), "ids": ids, "unusable": unusable })
            }
            "sign_trust" => self.sign_trust(&a)?,
            "comment_mark" => self.comment_mark(&a)?,
            "comment_lock" => self.comment_lock(&a)?,
            "comments_hide" => self.comments_hide(&a)?,
            "layer_set" => self.layer_set(&a)?,
            "view_reading_colors" => self.view_reading_colors(&a)?,
            "comments_summarize" => self.comments_summarize(&a)?,
            "comment_edit" => self.comment_edit(&a)?,
            "comment_delete" => self.comment_delete(&a)?,
            other => return Err(ToolError::UnknownTool(other.into())),
        };
        Ok(vec![Content::Json(out)])
    }
}
