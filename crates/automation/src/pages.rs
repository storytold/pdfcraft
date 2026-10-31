use super::dispatch::Content;
use super::error::{Result, ToolError, failed};
use super::json::{Args, one_based};
use super::{Arc, Automation, DocId, HashMap, PageRenderer, PageText, RenderConfig, RenderRequest, RequestKind, Value, commands, json, tools};

/// Default and maximum resolution for `page_render`.
pub(crate) const DEFAULT_DPI: f64 = 96.0;

pub(crate) const MAX_DPI: f64 = 600.0;

/// `text_find` extracts this many pages first, doubling each batch up to [`FIND_MAX_BATCH`], so a
/// match near the start returns without touching the rest of a long document (#971).
pub(crate) const FIND_FIRST_BATCH: usize = 8;

const FIND_MAX_BATCH: usize = 128;

type PageTexts = Vec<(usize, std::result::Result<Arc<PageText>, String>)>;

pub(crate) const FIND_MAX_WARNINGS: usize = 100;

pub(crate) const FIND_MAX_WARNING_BYTES: usize = 1024;

impl Automation {
    // ---- pages and text ----------------------------------------------------------------------

    /// 1-based page list → validated 0-based indices.
    pub(crate) fn pages(&self, a: &Args, key: &str) -> Result<Vec<usize>> {
        let n = self.doc(a)?.info.pages.len();
        let pages = one_based(&a.ints(key)?)?;
        if pages.is_empty() {
            return Err(ToolError::InvalidArgs(format!("{key} must list at least one page")));
        }
        if let Some(p) = pages.iter().find(|p| **p >= n) {
            return Err(ToolError::InvalidArgs(format!("page {} is out of range: the document has {n} pages", p + 1)));
        }
        Ok(pages)
    }

    /// A 1-based insertion position (1 = before the first page, n+1 = after the last) → 0-based.
    pub(crate) fn position(&self, a: &Args, key: &str) -> Result<usize> {
        self.opt_position(a, key)?.ok_or_else(|| Args::missing(key))
    }

    /// `position`, but `None` when the key is absent.
    pub(crate) fn opt_position(&self, a: &Args, key: &str) -> Result<Option<usize>> {
        let Some(at) = a.opt_int(key)? else { return Ok(None) };
        let n = self.doc(a)?.info.pages.len();
        if at < 1 || at as usize > n + 1 {
            return Err(ToolError::InvalidArgs(format!("{key} must be between 1 and {} (the document has {n} pages)", n + 1)));
        }
        Ok(Some(at as usize - 1))
    }

    pub(crate) fn renderer(&mut self, id: DocId) -> Result<&mut PageRenderer> {
        let doc = self.session.get(id).ok_or_else(|| failed("no such document"))?;
        // The document's own view settings: layer visibility, hidden comments, reading colours
        // (tools that change one drop this cache so the next render reads it again).
        let config =
            self.session.render_config(id).unwrap_or_else(|| RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() });
        let fresh = || (doc.display.clone(), PageRenderer::new(doc.display.clone(), config.clone()));
        let entry = self.renderers.entry(id).or_insert_with(fresh);
        if !Arc::ptr_eq(&entry.0, &doc.display) {
            *entry = fresh();
        }
        Ok(&mut entry.1)
    }

    /// The page as a PNG, followed by a JSON part listing the render warnings when there are any,
    /// so a caller can tell that the picture is incomplete, as `pdfcraft-cli render` does (#4340).
    pub(crate) fn page_render(&mut self, a: &Args) -> Result<Vec<Content>> {
        let id = self.doc(a)?.id;
        let page = self.page(a)?;
        let dpi = a.opt_num("dpi")?.unwrap_or(DEFAULT_DPI);
        if !(1.0..=MAX_DPI).contains(&dpi) {
            return Err(ToolError::InvalidArgs(format!("dpi must be between 1 and {MAX_DPI}")));
        }
        let scale = (dpi / 72.0) as f32;
        let block = a.opt_int("hide_text_block")?;
        let added = a.opt_int("hide_added_text")?;
        if block.is_some() && added.is_some() {
            return Err(ToolError::InvalidArgs("choose only one of hide_text_block and hide_added_text".into()));
        }
        let out = if let Some(block) = block {
            if block < 1 {
                return Err(ToolError::InvalidArgs("hide_text_block must be a positive paragraph index from text_paragraphs".into()));
            }
            // The number names the paragraph text_paragraphs listed, as it does for text_edit.
            let doc = self.doc(a)?;
            let index = self.paragraph_index(doc.id, page, block, &doc.text_blocks(page))?;
            doc.text_edit_preview(page, index).map_err(failed)?.render_background(scale).map_err(failed)?
        } else if let Some(added) = added {
            let index = usize::try_from(added)
                .ok()
                .and_then(|n| n.checked_sub(1))
                .ok_or_else(|| ToolError::InvalidArgs("hide_added_text must be a positive added text index from object_list".into()))?;
            self.doc(a)?.added_text_preview(page, index).map_err(failed)?.render_background(scale).map_err(failed)?
        } else {
            self.renderer(id)?.render(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag: 0, preview: false })
        };
        if let Some(e) = out.error {
            return Err(failed(format!("page {} could not be rendered: {e}", page + 1)));
        }
        rendered_content(&out, page)
    }

    /// A page as RGBA pixels, without encoding a PNG. `page` is 1-based and `scale` is pixels
    /// per point (1.0 = 72 dpi). For an embedder that paints the result straight onto a canvas:
    /// the `page_render` tool only returns PNG, and encoding one to decode it again is wasted
    /// work on every scroll. It uses the same renderer, and the same per-document cache, as
    /// `page_render`.
    pub fn render_pixels(&mut self, doc: u64, page: usize, scale: f32) -> Result<(u32, u32, pdfcraft_render::Pixels)> {
        if !(scale.is_finite() && scale > 0.0 && scale <= (MAX_DPI / 72.0) as f32) {
            return Err(ToolError::InvalidArgs(format!("scale must be between 0 and {}", MAX_DPI / 72.0)));
        }
        let id = DocId(doc);
        let n = self.session.get(id).ok_or_else(|| failed(format!("no open document with id {doc} (see doc_list)")))?.info.pages.len();
        if page < 1 || page > n {
            return Err(ToolError::InvalidArgs(format!("page {page} is out of range: the document has {n} pages")));
        }
        let out = self.renderer(id)?.render(RenderRequest { page: page - 1, kind: RequestKind::Pixels, tile: None, scale, tag: 0, preview: false });
        if let Some(e) = out.error {
            return Err(failed(format!("page {page} could not be rendered: {e}")));
        }
        Ok((out.width, out.height, out.rgba))
    }

    pub(crate) fn page(&self, a: &Args) -> Result<usize> {
        let n = self.doc(a)?.info.pages.len();
        let p = a.int("page")?;
        if p < 1 || p as usize > n {
            return Err(ToolError::InvalidArgs(format!("page {p} is out of range: the document has {n} pages")));
        }
        Ok(p as usize - 1)
    }

    /// The text of `pages` (0-based), from the cache or extracted in parallel.
    pub(crate) fn page_texts(&mut self, id: DocId, pages: &[usize]) -> Result<Vec<Arc<PageText>>> {
        self.page_text_results(id, pages, &mut extract_parallel)?
            .into_iter()
            .map(|(p, text)| text.map_err(|e| failed(format!("page {}: {e}", p + 1))))
            .collect()
    }

    /// Keep per-page failures until the caller chooses strict or partial-result semantics.
    pub(crate) fn page_text_results(
        &mut self,
        id: DocId,
        pages: &[usize],
        extract: &mut impl FnMut(&Arc<Vec<u8>>, Option<Arc<str>>, &[usize]) -> PageTexts,
    ) -> Result<PageTexts> {
        let doc = self.session.get(id).ok_or_else(|| failed("no such document"))?;
        let (bytes, n) = (doc.bytes.clone(), doc.info.pages.len());
        let password: Option<Arc<str>> = doc.password.as_deref().map(Arc::from);
        self.texts.prepare(id, &bytes);
        let mut results = HashMap::new();
        let mut missing = Vec::new();
        for &p in pages {
            if p >= n {
                results.insert(p, Err("page is out of range".to_owned()));
            } else if let Some(text) = self.texts.get(id, p) {
                results.insert(p, Ok(text));
            } else {
                missing.push(p);
            }
        }
        missing.sort_unstable();
        missing.dedup();
        if !missing.is_empty() {
            for (p, text) in extract(&bytes, password, &missing) {
                if let Ok(text) = &text {
                    self.texts.insert(id, p, &bytes, text.clone());
                }
                results.insert(p, text);
            }
        }
        // The batch owns its Arcs: evicting a cache entry cannot lose a requested result. A
        // worker that fails to return a page is an explicit failure, never an empty success.
        Ok(pages.iter().map(|p| (*p, results.get(p).cloned().unwrap_or_else(|| Err("text extraction returned no result".into())))).collect())
    }

    pub(crate) fn text_word_count(&self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::word_count::{self, Counts, Options};
        let doc = self.doc_to_copy(a)?;
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..doc.info.pages.len()).collect(),
        };
        let options = Options {
            exclude_tables: a.opt_bool("exclude_tables")?.unwrap_or(false),
            exclude_code: a.opt_bool("exclude_code")?.unwrap_or(false),
            exclude_images: a.opt_bool("exclude_images")?.unwrap_or(false),
        };
        let report = word_count::count(doc.export_source(), &pages, options, |_| true).map_err(failed)?;
        let counts = |c: Counts| json!({ "words": c.words, "characters": c.characters, "characters_without_spaces": c.characters_without_spaces });
        Ok(json!({
            "total": counts(report.total),
            "pages": report.pages.into_iter().map(|(p, c)| json!({ "page": p + 1, "counts": counts(c) })).collect::<Vec<_>>(),
            "pages_without_text": report.pages_without_text,
            "unclassified_structure": report.unclassified_structure,
        }))
    }

    pub(crate) fn text_extract(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc_to_copy(a)?;
        let id = doc.id;
        let pages = match a.opt_ints("pages")? {
            Some(_) => self.pages(a, "pages")?,
            None => (0..doc.info.pages.len()).collect(),
        };
        // Column select (#740): only the text inside the rectangle, row by row.
        let rect = a.nums::<4>("rect")?.map(|r| r.map(|v| v as f32));
        let texts = self.page_texts(id, &pages)?;
        let text = |t: &PageText| match rect {
            Some(r) => t.column_text(&t.glyphs_in(r)),
            None => t.plain_text(),
        };
        let out: Vec<Value> = pages.iter().zip(texts).map(|(p, t)| json!({ "page": p + 1, "text": text(&t) })).collect();
        Ok(json!({ "pages": out }))
    }

    /// Matches in document order, extracting page text in growing batches and stopping as soon as
    /// `limit` is reached, so a bounded search doesn't extract (and cache) the whole document
    /// (#971). One match past the limit supplies `next`. Reached extraction failures are reported
    /// as bounded warnings; `complete` requires both no continuation and no failed pages.
    pub(crate) fn text_find(&mut self, a: &Args) -> Result<Value> {
        self.text_find_with(a, &mut extract_parallel)
    }

    pub(crate) fn text_find_with(
        &mut self,
        a: &Args,
        extract: &mut impl FnMut(&Arc<Vec<u8>>, Option<Arc<str>>, &[usize]) -> PageTexts,
    ) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, n) = (doc.id, doc.info.pages.len());
        let query = a.str("query")?;
        let limit = usize::try_from(a.opt_int("limit")?.unwrap_or(500).max(1)).unwrap_or(usize::MAX);
        let start = match a.opt_int("start_page")? {
            None => 0,
            Some(p) if p >= 1 && usize::try_from(p).is_ok_and(|p| p <= n) => usize::try_from(p - 1).unwrap_or(0),
            Some(p) => return Err(ToolError::InvalidArgs(format!("start_page {p} is out of range: the document has {n} pages"))),
        };
        let skip = usize::try_from(a.opt_int("skip")?.unwrap_or(0).max(0)).unwrap_or(usize::MAX);
        let mut matches = Vec::new();
        let mut next = None;
        let mut warnings = Vec::new();
        let mut warning_count = 0usize;
        let (mut first, mut batch) = (start, FIND_FIRST_BATCH);
        'search: while first < n {
            let end = first.saturating_add(batch).min(n);
            let pages: Vec<usize> = (first..end).collect();
            let texts = self.page_text_results(id, &pages, extract)?;
            for (p, text) in texts {
                let text = match text {
                    Ok(text) => text,
                    Err(mut error) => {
                        warning_count += 1;
                        if warnings.len() < FIND_MAX_WARNINGS {
                            let end = error.floor_char_boundary(FIND_MAX_WARNING_BYTES.min(error.len()));
                            let truncated = end < error.len();
                            error.truncate(end);
                            warnings.push(json!({ "page": p + 1, "error": error, "error_truncated": truncated }));
                        }
                        continue;
                    }
                };
                let skip_here = if p == start { skip } else { 0 };
                // One extra match proves that the caller needs a continuation. Include skipped
                // matches on the starting page when bounding the temporary per-page result.
                let needed = limit.saturating_sub(matches.len()).saturating_add(skip_here).saturating_add(1);
                for (k, r) in text.find_opts_limited(query, false, false, needed).into_iter().enumerate().skip(skip_here) {
                    if matches.len() == limit {
                        next = Some(json!({ "start_page": p + 1, "skip": k }));
                        break 'search;
                    }
                    matches.push(json!({ "page": p + 1, "text": text.text_of(r.clone()), "rects": text.line_rects(r) }));
                }
            }
            first = end;
            batch = batch.saturating_mul(2).min(FIND_MAX_BATCH);
        }
        let mut out = json!({ "query": query, "count": matches.len(), "complete": next.is_none() && warning_count == 0, "matches": matches,
            "warnings_truncated": warning_count > warnings.len(), "warning_count": warning_count, "warnings": warnings });
        if let (Some(next), Some(o)) = (next, out.as_object_mut()) {
            o.insert("next".into(), next);
        }
        Ok(out)
    }

    pub(crate) fn command_list(&self, a: &Args) -> Result<Value> {
        let active = match a.opt_int("doc")? {
            Some(_) => Some(self.doc(a)?.id),
            None => None,
        };
        let filter = a.opt_str("filter")?.unwrap_or("").to_lowercase();
        let enabled_only = a.opt_bool("enabled_only")?.unwrap_or(false);
        let list: Vec<Value> = commands::COMMANDS
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "label": commands::current_label(c, &self.session, active),
                    "menu": c.menu,
                    "shortcut": c.shortcut.map(|s| s.label(cfg!(target_os = "macos"))),
                    "enabled": commands::is_enabled(c, &self.session, active),
                    "tool": tools::tool_for_command(c.id),
                    "params": tools::tool_for_command(c.id).and_then(tools::find).map(|t| &t.input_schema),
                })
            })
            .collect();
        let list: Vec<Value> = list
            .into_iter()
            .filter(|c| {
                (!enabled_only || c.get("enabled").and_then(Value::as_bool) == Some(true))
                    && (filter.is_empty()
                        || ["id", "label", "menu"].iter().any(|k| c.get(k).is_some_and(|v| v.to_string().to_lowercase().contains(&filter))))
            })
            .collect();
        Ok(json!({ "commands": list }))
    }
}

/// A rendered raster as PNG, through the engine's encoder (Export ▸ Image writes the same).
pub(crate) fn encode_png(width: u32, height: u32, premultiplied: &[u8]) -> Result<Vec<u8>> {
    pdfcraft_engine::export::encode_png(width, height, premultiplied).map_err(failed)
}

/// The output of a page-rendering tool: the PNG of the 0-based `page`, followed by a JSON part
/// listing its render warnings when there are any. Without warnings it is the PNG alone, as it
/// always was.
pub(crate) fn rendered_content(out: &pdfcraft_render::RenderedPage, page: usize) -> Result<Vec<Content>> {
    let data = encode_png(out.width, out.height, &out.rgba)?;
    let mut content = vec![Content::Png { data, width: out.width, height: out.height }];
    if !out.warnings.is_empty() {
        // Sentences, like the other tools' `warnings`, so command_run can add its own to the list.
        let warnings: Vec<&str> = out.warnings.iter().map(|&w| render_warning_text(w)).collect();
        content.push(Content::Json(json!({ "page": page.saturating_add(1), "warnings": warnings })));
    }
    Ok(content)
}

/// What a render warning means, in the words `pdfcraft-cli render` prints for it.
fn render_warning_text(warning: pdfcraft_render::RenderWarning) -> &'static str {
    use pdfcraft_render::RenderWarning;
    match warning {
        RenderWarning::ContentTruncated => "content past the page's safety budget was skipped",
        RenderWarning::ColourFallback => "a colour that could not be computed was painted in a fallback colour",
    }
}

/// Extract the text of `pages` (0-based) with one renderer per worker thread.
pub(crate) fn extract_parallel(bytes: &Arc<Vec<u8>>, password: Option<Arc<str>>, pages: &[usize]) -> PageTexts {
    let extract = |r: &mut PageRenderer, p: usize| {
        let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0, preview: false });
        match out.error {
            Some(e) => Err(e),
            None => Ok(out.text.unwrap_or_default()),
        }
    };
    let config = RenderConfig { password, ..Default::default() };
    let workers = if cfg!(target_arch = "wasm32") { 1 } else { std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get).min(8) };
    // Small jobs aren't worth a second parse of the document.
    if workers == 1 || pages.len() < 8 {
        let mut r = PageRenderer::new(bytes.clone(), config);
        return pages.iter().map(|p| (*p, extract(&mut r, *p))).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: PageTexts = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut r = PageRenderer::new(bytes.clone(), config.clone());
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(p) = pages.get(i) else { break };
                        done.push((*p, extract(&mut r, *p)));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    out.sort_by_key(|(p, _)| *p);
    out
}
