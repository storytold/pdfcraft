use super::error::{Result, ToolError, failed};
use super::json::{Args, nest_rows, security, summary};
use super::paths::{child, read_input, write_atomic};
use super::*;

/// How far a watermark may be moved from the page centre, in points (a page is at most 14,400).
const WATERMARK_MAX_OFFSET: f64 = 20_000.0;

/// Bookmarks in one page of the bookmark list, by default and at most.
const BOOKMARK_PAGE: usize = 100;

const MAX_BOOKMARK_PAGE: usize = 1000;

impl Automation {
    // ---- documents ---------------------------------------------------------------------------

    /// One page of a document's bookmarks: `offset` (from 0) and `limit` (1 to `MAX_BOOKMARK_PAGE`).
    pub(crate) fn bookmark_listing(&self, a: &Args) -> Result<Value> {
        let id = a.int("doc")?;
        let id = DocId(u64::try_from(id).map_err(|_| ToolError::InvalidArgs("doc must be positive".into()))?);
        let offset = a.opt_int("offset")?.unwrap_or(0);
        let offset = usize::try_from(offset).map_err(|_| ToolError::InvalidArgs("offset must be 0 or more".into()))?;
        let limit = a.opt_int("limit")?.unwrap_or(BOOKMARK_PAGE as i64);
        let limit = usize::try_from(limit)
            .ok()
            .filter(|l| (1..=MAX_BOOKMARK_PAGE).contains(l))
            .ok_or_else(|| ToolError::InvalidArgs(format!("limit must be from 1 to {MAX_BOOKMARK_PAGE}")))?;
        let page = self
            .session
            .bookmark_page(id, offset, limit)
            .ok_or_else(|| ToolError::Failed(format!("no open document with id {} (see doc_list)", id.0)))?;
        let pages = &self.doc(a)?.info.pages;
        Ok(json!({ "bookmarks": nest_rows(&page.rows, pages), "next": page.next, "truncated": page.truncated }))
    }

    pub(crate) fn doc(&self, a: &Args) -> Result<&Document> {
        let id = a.int("doc")?;
        let id = u64::try_from(id).map_err(|_| ToolError::InvalidArgs("doc must be positive".into()))?;
        self.session.get(DocId(id)).ok_or_else(|| ToolError::Failed(format!("no open document with id {id} (see doc_list)")))
    }

    /// Most pages whose paragraph listing is remembered.
    const MAX_LISTINGS: usize = 256;

    /// Remember the paragraphs `text_paragraphs` just listed for `page`, so their numbers can be
    /// resolved against that listing.
    pub(crate) fn record_listing(&mut self, doc: DocId, page: usize, blocks: &[TextBlock]) {
        if self.listed.len() >= Self::MAX_LISTINGS && !self.listed.contains_key(&(doc, page)) {
            self.evicted.extend(self.listed.keys().map(|(d, _)| *d));
            self.listed.clear();
        }
        self.listed.insert((doc, page), blocks.iter().map(BlockKey::of).collect());
    }

    /// The index in `blocks` (the page's paragraphs now) of paragraph number `k` (1-based).
    ///
    /// After `text_paragraphs` a number names the paragraph that listing showed, wherever it is
    /// now. Without a listing it is a position in `blocks`. A paragraph an edit merged into a
    /// neighbour (or changed) is refused: the number would otherwise rewrite another paragraph
    /// and lose its text (#2516). So is any number on a page of a document whose listings were
    /// dropped at the cap, until the page is listed again.
    pub(crate) fn paragraph_index(&self, doc: DocId, page: usize, k: i64, blocks: &[TextBlock]) -> Result<usize> {
        let keys = self.listed.get(&(doc, page));
        if keys.is_none() && self.evicted.contains(&doc) {
            return Err(ToolError::InvalidArgs(format!(
                "page {} has no paragraph listing any more: too many pages were listed and the oldest listings were dropped. List the page again with text_paragraphs",
                page + 1
            )));
        }
        let n = keys.map_or(blocks.len(), Vec::len);
        let i = usize::try_from(k)
            .ok()
            .and_then(|k| k.checked_sub(1))
            .filter(|i| *i < n)
            .ok_or_else(|| ToolError::InvalidArgs(format!("paragraph {k} is out of range: page {} has {n} paragraphs", page + 1)))?;
        let Some(keys) = keys else { return Ok(i) };
        keys.get(i).and_then(|key| key.find(blocks, i)).ok_or_else(|| {
            ToolError::InvalidArgs(format!(
                "paragraph {k} of page {} is no longer a paragraph of its own: an edit changed it or how the page is grouped. List the page again with text_paragraphs",
                page + 1
            ))
        })
    }

    /// Update the listing after paragraph `k` was edited into `edited`, one of the page's
    /// paragraphs `after`. Returns the numbers of the listed paragraphs that are gone.
    pub(crate) fn follow_edit(&mut self, doc: DocId, page: usize, k: i64, edited: Option<&TextBlock>, after: &[TextBlock]) -> Vec<usize> {
        let Some(keys) = self.listed.get_mut(&(doc, page)) else { return Vec::new() };
        if let (Some(edited), Some(key)) = (edited, usize::try_from(k).ok().and_then(|k| k.checked_sub(1)).and_then(|i| keys.get_mut(i))) {
            *key = BlockKey::of(edited);
        }
        keys.iter().enumerate().filter(|(i, key)| key.find(after, *i).is_none()).map(|(i, _)| i + 1).collect()
    }

    /// The document, if its security allows copying text and images out of it (Table 22, bit 5).
    pub(crate) fn doc_to_copy(&self, a: &Args) -> Result<&Document> {
        let doc = self.doc(a)?;
        doc.check_copying().map_err(failed)?;
        Ok(doc)
    }

    pub(crate) fn doc_open(&mut self, a: &Args) -> Result<Value> {
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = read_input(&path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let id = self.session.open(name, Some(path.to_string_lossy().into_owned()), Arc::new(bytes), a.opt_str("password")?).map_err(failed)?;
        let doc = self.session.get(id).ok_or_else(|| failed("the document vanished"))?;
        Ok(summary(doc))
    }

    pub(crate) fn doc_close(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let discard_changes = a.opt_bool("discard_changes")?.unwrap_or(false);
        if doc.dirty && !discard_changes {
            return Err(failed("the document has unsaved changes: save it with doc_save, or pass discard_changes: true"));
        }
        self.session.close(id);
        self.renderers.remove(&id);
        self.texts.remove(&id);
        self.listed.retain(|(d, _), _| *d != id);
        self.evicted.remove(&id);
        Ok(json!({ "closed": id.0 }))
    }

    pub(crate) fn doc_save(&mut self, a: &Args) -> Result<Value> {
        self.doc_save_with(a, write_atomic)
    }

    pub(crate) fn doc_save_with(&mut self, a: &Args, write: impl FnOnce(&Path, &[u8]) -> Result<()>) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let target = match a.opt_str("path")? {
            Some(p) => self.resolve(p, true)?,
            None => {
                // Saving in place writes where the file was opened from, which the read root
                // allows and the write root may not.
                let own = doc.path.clone().ok_or_else(|| failed("the document has never been saved: pass a path"))?;
                let own_path = PathBuf::from(&own);
                self.check_roots(&own, &own_path, file_access::Access::Write)?;
                own_path
            }
        };
        let same_file = doc.path.as_deref().is_some_and(|p| Path::new(p) == target);
        // Saving to a new file is a full rewrite unless asked otherwise, like Save As.
        let full = a.opt_bool("full")?.unwrap_or(!same_file);
        let flatten_fill_sign = a.opt_bool("flatten_fill_sign")?.unwrap_or(false);
        if flatten_fill_sign {
            self.apply(a, Edit::FlattenFillSign)?;
        }
        let bytes = if full { self.session.save_full_bytes(id) } else { self.session.save_bytes(id) }.map_err(failed)?;
        // A save that adds or removes encryption, or must not keep the old revision (Sanitize,
        // redaction), is rewritten in full even when an incremental save was asked for.
        let incremental = !full && bytes.starts_with(&self.doc(a)?.saved_bytes());
        self.publish_save(id, &target, bytes, incremental, write)
    }

    /// Validate the prepared bytes before publishing them or marking the document saved.
    pub(crate) fn publish_save(
        &mut self,
        id: DocId,
        target: &Path,
        bytes: Arc<Vec<u8>>,
        incremental: bool,
        write: impl FnOnce(&Path, &[u8]) -> Result<()>,
    ) -> Result<Value> {
        self.session.check_save(id, &bytes).map_err(failed)?;
        write(target, &bytes)?;
        let path = target.to_string_lossy().into_owned();
        self.session.mark_saved(id, bytes.clone(), Some(path.clone())).map_err(failed)?;
        let doc = self.session.get(id).ok_or_else(|| failed("the saved document is no longer open"))?;
        Ok(json!({ "path": path, "bytes": bytes.len(), "incremental": incremental, "document": summary(doc) }))
    }

    pub(crate) fn doc_revisions(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let ends = doc.revision_ends();
        let sigs = doc.signatures.clone();
        let list: Vec<Value> = ends
            .iter()
            .enumerate()
            .map(|(i, end)| {
                let start = if i == 0 { 0 } else { ends[i - 1] };
                // A signature signs the revision its byte range ends in.
                let signed: Vec<&str> =
                    sigs.iter().filter(|s| s.signed && s.signed_len > start && s.signed_len <= *end).map(|s| s.field.as_str()).collect();
                json!({ "revision": i + 1, "end": end, "bytes": end - start, "signed_by": signed })
            })
            .collect();
        Ok(json!({ "revisions": list, "unsaved_changes": doc.dirty }))
    }

    pub(crate) fn doc_initial_view(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{InitialLayout as L, Magnification as M, Navigation as N};
        let bad = |m: String| ToolError::InvalidArgs(m);
        let mut v = self.doc(a)?.initial_view();
        let before = v.clone();
        if let Some(n) = a.opt_str("navigation")? {
            v.navigation = match n {
                "page" => N::PageOnly,
                "bookmarks" => N::Bookmarks,
                "pages" => N::Pages,
                "attachments" => N::Attachments,
                "layers" => N::Layers,
                o => return Err(bad(format!("unknown navigation {o:?}"))),
            };
        }
        if let Some(l) = a.opt_str("layout")? {
            v.layout = match l {
                "default" => L::Default,
                "single" => L::SinglePage,
                "continuous" => L::SinglePageContinuous,
                "two_up" => L::TwoUp,
                "two_up_continuous" => L::TwoUpContinuous,
                "two_up_cover" => L::TwoUpCoverPage,
                "two_up_continuous_cover" => L::TwoUpContinuousCoverPage,
                o => return Err(bad(format!("unknown layout {o:?}"))),
            };
        }
        match a.get("magnification") {
            None => {}
            Some(Value::Number(p)) => v.magnification = M::Percent(p.as_f64().unwrap_or(100.0).clamp(1.0, 6400.0)),
            Some(Value::String(m)) => {
                v.magnification = match m.as_str() {
                    "default" => M::Default,
                    "actual" => M::ActualSize,
                    "fit_page" => M::FitPage,
                    "fit_width" => M::FitWidth,
                    "fit_height" => M::FitHeight,
                    "fit_visible" => M::FitVisible,
                    o => return Err(bad(format!("unknown magnification {o:?}"))),
                }
            }
            Some(_) => return Err(bad("magnification is a name or a percentage".into())),
        }
        if let Some(p) = a.opt_int("page")? {
            v.page = (p.max(1) - 1) as usize;
        }
        for (k, f) in [
            ("fit_window", &mut v.fit_window),
            ("center_window", &mut v.center_window),
            ("full_screen", &mut v.full_screen),
            ("display_title", &mut v.display_title),
            ("hide_menubar", &mut v.hide_menubar),
            ("hide_toolbar", &mut v.hide_toolbar),
            ("hide_window_ui", &mut v.hide_window_ui),
        ] {
            if let Some(b) = a.opt_bool(k)? {
                *f = b;
            }
        }
        if let Some(l) = a.opt_str("language")? {
            v.language = (!l.trim().is_empty()).then(|| l.trim().to_string());
        }
        if let Some(b) = a.opt_str("binding")? {
            v.right_to_left = match b {
                "left" => false,
                "right" => true,
                o => return Err(bad(format!("binding is left or right, not {o:?}"))),
            };
        }
        let mut out = if v != before { self.apply(a, Edit::SetInitialView(Box::new(v.clone())))? } else { json!({}) };
        out["initial_view"] = json!({
            "navigation": format!("{:?}", v.navigation),
            "layout": format!("{:?}", v.layout),
            "magnification": format!("{:?}", v.magnification),
            "page": v.page + 1,
            "fit_window": v.fit_window,
            "center_window": v.center_window,
            "full_screen": v.full_screen,
            "display_title": v.display_title,
            "hide_menubar": v.hide_menubar,
            "hide_toolbar": v.hide_toolbar,
            "hide_window_ui": v.hide_window_ui,
            "language": v.language,
            "binding": if v.right_to_left { "right" } else { "left" },
        });
        Ok(out)
    }

    pub(crate) fn doc_optimize(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::optimize::{Compatibility, Compression, ImageSettings, Preset};
        let id = self.doc(a)?.id;
        let before = self.doc(a)?.bytes.len();
        let path = self.resolve(a.str("path")?, true)?;
        let mut settings = match a.opt_str("preset")? {
            None => Preset::Standard.settings(),
            Some(p) => Preset::from_name(p).ok_or_else(|| ToolError::InvalidArgs(format!("unknown preset {p:?} (standard, mobile)")))?.settings(),
        };
        if let Some(c) = a.opt_str("compatibility")? {
            settings.compatibility = Compatibility::from_name(c)
                .ok_or_else(|| ToolError::InvalidArgs(format!("unknown compatibility {c:?} (retain, 1.4, 1.5, 1.6, 1.7, 2.0)")))?;
        }
        let images = |key: &str, base: ImageSettings| -> Result<ImageSettings> {
            let Some(v) = a.get(key) else { return Ok(base) };
            let mut s = base;
            if let Some(d) = v.get("downsample").and_then(Value::as_bool) {
                s.downsample = d;
            }
            if let Some(p) = v.get("ppi").and_then(Value::as_f64) {
                s.target_ppi = p.clamp(9.0, 2400.0);
            }
            if let Some(p) = v.get("above_ppi").and_then(Value::as_f64) {
                s.above_ppi = p.clamp(9.0, 2400.0);
            }
            s.above_ppi = s.above_ppi.max(s.target_ppi);
            if let Some(c) = v.get("compression").and_then(Value::as_str) {
                let q = v.get("quality").and_then(Value::as_u64).unwrap_or(60).clamp(1, 100) as u8;
                s.compression = match c {
                    "jpeg" => Compression::Jpeg(q),
                    "zip" | "flate" => Compression::Flate,
                    "retain" => Compression::Retain,
                    other => return Err(ToolError::InvalidArgs(format!("unknown compression {other:?} (jpeg, zip, retain)"))),
                };
            }
            Ok(s)
        };
        settings.color = images("color", settings.color)?;
        settings.gray = images("gray", settings.gray)?;
        for (key, flag) in [
            ("merge_fragments", &mut settings.merge_fragments),
            ("discard_thumbnails", &mut settings.discard_thumbnails),
            ("discard_alternate_images", &mut settings.discard_alternate_images),
            ("discard_tags", &mut settings.discard_tags),
            ("discard_print_settings", &mut settings.discard_print_settings),
            ("flate_unencoded", &mut settings.flate_unencoded),
            ("object_streams", &mut settings.object_streams),
            ("lzw_to_flate", &mut settings.lzw_to_flate),
            ("remove_invalid_links", &mut settings.remove_invalid_links),
            ("remove_unreferenced_dests", &mut settings.remove_unreferenced_dests),
        ] {
            if let Some(b) = a.opt_bool(key)? {
                *flag = b;
            }
        }
        let discard: Vec<pdfcraft_engine::Hidden> = match a.get("discard") {
            None => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or_else(|| ToolError::InvalidArgs("discard must be an array".into()))?
                .iter()
                .map(|x| {
                    x.as_str()
                        .and_then(pdfcraft_engine::Hidden::from_id)
                        .ok_or_else(|| ToolError::InvalidArgs(format!("unknown discard category {x}")))
                })
                .collect::<Result<_>>()?,
        };
        let (bytes, r) = self.session.optimized_bytes(id, &settings, &discard).map_err(failed)?;
        write_atomic(&path, &bytes)?;
        fn valid_version(v: &[u8]) -> Option<&str> {
            if matches!(v, [b'1'..=b'9', b'.', b'0'..=b'9']) { std::str::from_utf8(v).ok() } else { None }
        }
        let catalog_version = (settings.compatibility == Compatibility::Retain)
            .then(|| {
                let pw = self.doc(a).ok().and_then(|d| d.password.as_deref());
                let doc = pdfcraft_cos::Document::open_with_password(bytes.clone(), pw).ok()?;
                let root = doc.root()?;
                doc.get(root).as_dict()?.name(b"Version").and_then(valid_version).map(str::to_owned)
            })
            .flatten();
        let header_version = bytes.strip_prefix(b"%PDF-").and_then(|b| b.get(..3)).and_then(valid_version);
        // Viewers read the later of the two (object streams raise the header past a lower catalog
        // entry). Both are "d.d", so text order is version order.
        let pdf_version = catalog_version.as_deref().into_iter().chain(header_version).max();
        let o = &r.optimize;
        Ok(json!({
            "path": path.to_string_lossy(),
            "pdf_version": pdf_version,
            "object_streams": settings.effective_object_streams(),
            "bytes_before": before,
            "bytes_after": bytes.len(),
            "merged_images": o.merged_images,
            "fragments_merged": o.fragments_merged,
            "images": o.images,
            "images_resampled": o.images_resampled,
            "images_recompressed": o.images_recompressed,
            "image_bytes_before": o.image_bytes_before,
            "image_bytes_after": o.image_bytes_after,
            "thumbnails": o.thumbnails,
            "alternate_images": o.alternate_images,
            "tags_removed": o.tags_removed,
            "streams_compressed": o.streams_compressed,
            "lzw_streams": o.lzw_streams,
            "invalid_links": o.invalid_links,
            "invalid_bookmarks": o.invalid_bookmarks,
            "unreferenced_dests": o.unreferenced_dests,
            "unused_xobjects": o.unused_xobjects,
            "merged_objects": r.merged,
            "discarded": r.discarded.iter().map(|(h, n)| json!({ "category": h.id(), "count": n })).collect::<Vec<_>>(),
        }))
    }

    /// A watermark or background picture: `file` (an image or a PDF) and `file_page` (1-based).
    fn mark_file(&self, a: &Args) -> Result<Option<pdfcraft_engine::MarkFile>> {
        let Some(p) = a.opt_str("file")? else { return Ok(None) };
        let path = self.resolve(p, false)?;
        let bytes = read_input(&path)?;
        let page = a.opt_int("file_page")?.unwrap_or(1).max(1) as usize - 1;
        Ok(Some(pdfcraft_engine::MarkFile {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            bytes: Arc::new(bytes),
            page,
        }))
    }

    pub(crate) fn apply(&mut self, a: &Args, edit: Edit) -> Result<Value> {
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        let doc = self.doc(a)?;
        let mut out = summary(doc);
        // Text the edit stored as typed but drew with `?` (its font can't draw it): said, not silent.
        if !doc.edit_notices.is_empty() {
            out["warnings"] = json!(doc.edit_notices.iter().map(ToString::to_string).collect::<Vec<_>>());
        }
        Ok(out)
    }

    pub(crate) fn marks(&mut self, tool: &str, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{Background, HeaderFooter, MarkKind, Watermark};
        let n = self.doc(a)?.info.pages.len();
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..n).collect(),
        };
        let color =
            |key: &str, default: [f64; 3]| -> Result<[f64; 3]> { Ok(a.opt_str(key)?.map(comments::parse_color).transpose()?.unwrap_or(default)) };
        let replace = a.opt_bool("replace")?.unwrap_or(false);
        let edit = match tool {
            "doc_header_footer" => {
                let mut hf = HeaderFooter::default();
                for (k, key) in ["header_left", "header_center", "header_right", "footer_left", "footer_center", "footer_right"].iter().enumerate() {
                    hf.text[k] = a.opt_str(key)?.unwrap_or_default().to_string();
                }
                if let Some(s) = a.opt_num("font_size")? {
                    hf.font_size = s;
                }
                hf.font = match a.opt_str("font")? {
                    None | Some("Helvetica") => pdfcraft_engine::FontFamily::Helvetica,
                    Some("Times") => pdfcraft_engine::FontFamily::Times,
                    Some("Courier") => pdfcraft_engine::FontFamily::Courier,
                    Some(other) => {
                        return Err(ToolError::InvalidArgs(format!("font must be \"Helvetica\", \"Times\" or \"Courier\", not {other:?}")));
                    }
                };
                hf.color = color("color", hf.color)?;
                if let Some(m) = a.get("margins").and_then(Value::as_array) {
                    let m: Vec<f64> = m.iter().filter_map(Value::as_f64).collect();
                    hf.margins = <[f64; 4]>::try_from(m).map_err(|_| ToolError::InvalidArgs("margins must be 4 numbers".into()))?;
                }
                if let Some(s) = a.opt_int("start_number")? {
                    hf.start_number = s.clamp(1, u32::MAX as i64) as u32;
                }
                hf.shrink = a.opt_bool("shrink")?.unwrap_or(false);
                Edit::AddHeaderFooter { pages, settings: hf, replace }
            }
            "doc_watermark" => {
                let d = Watermark::default();
                let file = self.mark_file(a)?;
                let text = match &file {
                    Some(_) => a.opt_str("text")?.unwrap_or_default().to_string(),
                    None => a.str("text")?.to_string(),
                };
                let wm = Watermark {
                    text,
                    source: None,
                    scale: a.opt_num("scale")?.unwrap_or(d.scale).clamp(0.01, 1.0),
                    font_size: a.opt_num("font_size")?.unwrap_or(0.0),
                    color: color("color", d.color)?,
                    opacity: a.opt_num("opacity")?.unwrap_or(d.opacity),
                    rotation: a.opt_num("rotation")?.unwrap_or(d.rotation),
                    behind: a.opt_bool("behind")?.unwrap_or(false),
                    offset: [
                        a.opt_num("offset_x")?.unwrap_or(0.0).clamp(-WATERMARK_MAX_OFFSET, WATERMARK_MAX_OFFSET),
                        a.opt_num("offset_y")?.unwrap_or(0.0).clamp(-WATERMARK_MAX_OFFSET, WATERMARK_MAX_OFFSET),
                    ],
                };
                Edit::AddWatermark { pages, settings: wm, replace, file }
            }
            "doc_background" => Edit::AddBackground {
                pages,
                settings: Background {
                    color: color("color", [1.0; 3])?,
                    opacity: a.opt_num("opacity")?.unwrap_or(1.0),
                    scale: a.opt_num("scale")?.unwrap_or(1.0).clamp(0.01, 1.0),
                    ..Background::default()
                },
                replace,
                file: self.mark_file(a)?,
            },
            _ => {
                let kind = match a.str("kind")? {
                    "header_footer" => MarkKind::HeaderFooter,
                    "watermark" => MarkKind::Watermark,
                    "background" => MarkKind::Background,
                    other => return Err(ToolError::InvalidArgs(format!("unknown kind {other:?}"))),
                };
                Edit::RemoveMarks { kind }
            }
        };
        self.apply(a, edit)
    }

    pub(crate) fn export(&mut self, tool: &str, a: &Args) -> Result<Value> {
        let doc = self.doc_to_copy(a)?;
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..doc.info.pages.len()).collect(),
        };
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let mut ex = pdfcraft_engine::export::Exporter::new(doc);
        if tool == "doc_export_text" {
            let path = self.resolve(a.str("path")?, true)?;
            let text = ex.text_of(&pages).map_err(failed)?;
            write_atomic(&path, text.as_bytes())?;
            return Ok(json!({ "path": path.to_string_lossy(), "pages": pages.len(), "bytes": text.len() }));
        }
        let folder = self.resolve(a.str("folder")?, true)?;
        std::fs::create_dir_all(&folder).map_err(|e| failed(format!("{}: {e}", folder.display())))?;
        if tool == "doc_export_all_images" {
            let min = u32::try_from(a.opt_int("min_size")?.unwrap_or(0).max(0)).unwrap_or(u32::MAX);
            let out = pdfcraft_engine::export::extract_images(&doc.export_source(), &pages, min).map_err(failed)?;
            let mut files = Vec::new();
            for (k, img) in out.images.iter().enumerate() {
                let path = child(&folder, &pdfcraft_engine::export::image_file_name(&stem, img, k + 1, doc.info.pages.len()));
                write_atomic(&path, &img.data)?;
                files.push(json!({ "path": path.to_string_lossy(), "page": img.page + 1, "width": img.width, "height": img.height }));
            }
            let skipped: Vec<Value> = out.skipped.iter().map(|(p, r, why)| json!({ "page": p + 1, "object": r.num, "reason": why })).collect();
            return Ok(json!({ "count": files.len(), "files": files, "skipped": skipped }));
        }
        let dpi = a.opt_num("dpi")?.unwrap_or(150.0);
        let quality = a.opt_int("quality")?.unwrap_or(85).clamp(1, 100) as u8;
        let format = match a.opt_str("format")?.unwrap_or("png") {
            "png" => pdfcraft_engine::export::ImageFormat::Png,
            "jpeg" | "jpg" => pdfcraft_engine::export::ImageFormat::Jpeg { quality },
            "tiff" | "tif" => pdfcraft_engine::export::ImageFormat::Tiff,
            f => return Err(ToolError::InvalidArgs(format!("unknown format {f:?} (png, jpeg, tiff)"))),
        };
        let mut files = Vec::new();
        let mut lowered = Vec::new();
        for p in pages {
            let img = ex.image(p, dpi, format).map_err(failed)?;
            let path = child(&folder, &format!("{stem}_page_{}.{}", p + 1, format.extension()));
            write_atomic(&path, &img)?;
            files.push(path.to_string_lossy().into_owned());
            // A page too large for the renderer at `dpi` is drawn at the most it allows.
            let used = ex.dpi_used(p, dpi);
            if used < dpi.clamp(18.0, 1200.0) - 0.5 {
                lowered.push(json!({ "page": p + 1, "dpi": used.floor() }));
            }
        }
        if lowered.is_empty() {
            Ok(json!({ "count": files.len(), "files": files }))
        } else {
            Ok(json!({ "count": files.len(), "files": files, "lower_dpi": lowered }))
        }
    }

    pub(crate) fn doc_protect(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{Algorithm, Changes, Printing, Protection};
        let d = Protection::default();
        // Restrictions only exist behind a permissions password (ISO 32000-2 §7.6.4.4: /P is
        // enforced against the owner password; without one everything stays allowed). Refuse a
        // restriction that could not take effect instead of writing an unrestricted file (#134).
        let restriction = ["printing", "changes", "copy", "accessibility"].into_iter().find(|k| a.get(k).is_some());
        if let (None, Some(key)) = (a.opt_str("permissions_password")?, restriction) {
            return Err(ToolError::InvalidArgs(format!(
                "`{key}` needs `permissions_password`: with open_password alone the document is encrypted but nothing is restricted"
            )));
        }
        let p = Protection {
            open_password: a.opt_str("open_password")?.map(str::to_owned),
            permissions_password: a.opt_str("permissions_password")?.map(str::to_owned),
            printing: match a.opt_str("printing")? {
                None => d.printing,
                Some("none") => Printing::None,
                Some("low") => Printing::Low,
                Some("high") => Printing::High,
                Some(o) => return Err(ToolError::InvalidArgs(format!("unknown printing {o:?}"))),
            },
            changes: match a.opt_str("changes")? {
                None => d.changes,
                Some("none") => Changes::None,
                Some("pages") => Changes::Pages,
                Some("fill-sign") => Changes::FillSign,
                Some("comment-fill-sign") => Changes::CommentFillSign,
                Some("any-except-extract") => Changes::AnyExceptExtract,
                Some(o) => return Err(ToolError::InvalidArgs(format!("unknown changes {o:?}"))),
            },
            copy: a.opt_bool("copy")?.unwrap_or(d.copy),
            accessibility: a.opt_bool("accessibility")?.unwrap_or(d.accessibility),
            algorithm: match a.opt_str("compatibility")? {
                None | Some("aes-256") => Algorithm::Aes256,
                Some("aes-128") => Algorithm::Aes128,
                Some("rc4-128") => Algorithm::Rc4_128,
                Some("rc4-40") => Algorithm::Rc4_40,
                Some(o) => return Err(ToolError::InvalidArgs(format!("unknown compatibility {o:?}"))),
            },
            encrypt_metadata: a.opt_bool("encrypt_metadata")?.unwrap_or(d.encrypt_metadata),
        };
        let mut out = self.apply(a, Edit::Protect(p))?;
        out["security"] = security(self.doc(a)?);
        Ok(out)
    }
}
