use super::*;

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a fixed clock (tests) instead of the system time.
    pub fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Multiply the time an XFA form's scripts may take by `factor` (tests): the 3 s the
    /// scripts of an opening or a change share, the 5 s of a click's, the 10 s the script
    /// engine has to start, and each script's own 1 s or 3 s. A test that checks what the
    /// scripts do passes a large factor, so that a busy machine does not stop them. On native
    /// builds, 0 stops every script before it starts; the wasm build has no wall clock, so
    /// there the factor has no effect.
    pub fn with_xfa_time_scale(mut self, factor: u32) -> Self {
        self.xfa_time_scale = Some(factor);
        self
    }

    /// A real clock for a host that has neither a clock nor a time zone of its own (a
    /// `wasm32-unknown-unknown` build: `now()` would be empty and `local_utc_offset()` is 0, so
    /// Fill & Sign stamps 1 Jan 1970 and `/ModDate` is never written). `clock` is the UTC time in
    /// seconds since the epoch; `utc_offset` is the local zone's seconds east of UTC, asked on every
    /// use because the zone changes with the date (daylight saving). Unlike
    /// [`Session::with_clock`], which is a test shortcut, nothing here is taken as local time.
    pub fn with_wall_clock(mut self, clock: fn() -> i64, utc_offset: fn() -> i64) -> Self {
        self.clock = Some(clock);
        self.utc_offset = Some(utc_offset);
        self
    }

    /// Seconds to add to UTC for the local time of this session: the injected zone, else 0 under
    /// the test clock (which is already local), else the system's.
    pub(super) fn local_offset(&self) -> i64 {
        match (self.utc_offset, self.clock) {
            (Some(zone), _) => zone(),
            (None, Some(_)) => 0,
            (None, None) => local_utc_offset(),
        }
    }

    /// Seconds since the Unix epoch from the session clock (0 when unknown).
    pub fn now_secs(&self) -> i64 {
        self.now().unwrap_or(0)
    }

    pub(crate) fn now(&self) -> Option<i64> {
        if let Some(c) = self.clock {
            return Some(c());
        }
        #[cfg(not(target_arch = "wasm32"))]
        return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64);
        #[cfg(target_arch = "wasm32")]
        return None;
    }

    /// Today's date in local time: (year, month, day). With an injected clock (tests) the clock
    /// is taken as local time.
    /// A dynamic stamp's second line: "By Ada at 2:14 pm, Oct 02, 2026" (local time).
    pub fn stamp_by_line(&self, author: &str) -> String {
        let offset = self.local_offset();
        let d = self.now().map(|t| pdfcraft_cos::pdf_date(t + offset)).unwrap_or_default();
        let num = |a: usize, b: usize| d.get(a..b).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let (y, mo, day, hh, mm) = (num(2, 6), num(6, 8), num(8, 10), num(10, 12), num(12, 14));
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let h12 = if hh % 12 == 0 { 12 } else { hh % 12 };
        let who = if author.trim().is_empty() { String::new() } else { format!("By {} ", author.trim()) };
        format!("{who}at {h12}:{mm:02} {}, {} {day:02}, {y}", if hh < 12 { "am" } else { "pm" }, MONTHS[(mo.clamp(1, 12) - 1) as usize])
    }

    pub fn today(&self) -> (i64, u32, u32) {
        let offset = self.local_offset();
        self.now().map(|t| pdfcraft_cos::pdf_date(t + offset)).as_deref().and_then(parse_ymd).unwrap_or((1970, 1, 1))
    }

    /// Open a document from bytes. Rendering starts lazily when pages are requested.
    ///
    /// `password` is tried as either the user or owner password when the file is encrypted.
    /// Open saved revision `n` (1 = the oldest) of a document as a new, unsaved document named
    /// "<name> (revision n)".
    pub fn open_revision(&mut self, id: DocId, n: usize) -> Result<DocId, String> {
        let doc = self.get(id).ok_or("no such document")?;
        let ends = doc.revision_ends();
        let end = *n.checked_sub(1).and_then(|i| ends.get(i)).ok_or_else(|| format!("the document has {} revision(s)", ends.len()))?;
        let name = format!("{} (revision {n}).pdf", doc.name.trim_end_matches(".pdf"));
        let (bytes, password) = (Arc::new(doc.bytes[..end.min(doc.bytes.len())].to_vec()), doc.password.clone());
        self.open(name, None, bytes, password.as_deref()).map_err(|e| e.to_string())
    }

    pub(crate) fn doc_mut(&mut self, id: DocId) -> Result<&mut Document, EditError> {
        self.docs.iter_mut().find(|d| d.id == id).ok_or(EditError::NoDocument)
    }

    /// Web addresses in the text of every page (Create links from URLs): (page, one rectangle
    /// per line in user space, URI). Text that already has a link over it is skipped.
    pub fn find_urls(&self, id: DocId) -> Vec<(usize, Vec<[f64; 4]>, String)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut out = Vec::new();
        for page in 0..doc.info.pages.len() {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let Some(text) = res.text else { continue };
            let found: std::cell::RefCell<Vec<(std::ops::Range<usize>, String)>> = Default::default();
            let hits = text.find_with(|chars| {
                let urls = pdfcraft_annot::links::find_urls(chars);
                let ranges = urls.iter().map(|u| u.0.clone()).collect();
                *found.borrow_mut() = urls;
                ranges
            });
            for (glyphs, (_, uri)) in hits.into_iter().zip(found.into_inner()) {
                let rects: Vec<[f64; 4]> = text
                    .line_rects(glyphs)
                    .into_iter()
                    .map(|v| {
                        let q = doc.info.pages[page].view_rect_to_quad(v);
                        let xs = [q[0], q[2], q[4], q[6]];
                        let ys = [q[1], q[3], q[5], q[7]];
                        [
                            xs.iter().copied().fold(f64::MAX, f64::min),
                            ys.iter().copied().fold(f64::MAX, f64::min),
                            xs.iter().copied().fold(f64::MIN, f64::max),
                            ys.iter().copied().fold(f64::MIN, f64::max),
                        ]
                    })
                    .filter(|r| {
                        !doc.links().iter().any(|l| l.page == page && l.rect[0] < r[2] && r[0] < l.rect[2] && l.rect[1] < r[3] && r[1] < l.rect[3])
                    })
                    .collect();
                if !rects.is_empty() {
                    out.push((page, rects, uri));
                }
            }
        }
        out
    }

    /// A page of the bookmark list in outline order: up to `limit` bookmarks from position `offset`.
    /// The document's own structure is read for it, so every bookmark can be reached by paging.
    pub fn bookmark_page(&self, id: DocId, offset: usize, limit: usize) -> Option<BookmarkPage> {
        let doc = self.get(id)?;
        if let Some(editor) = doc.editor.as_ref() {
            let page = pdfcraft_organize::bookmark_page(&editor.cos, offset, limit);
            let rows = page
                .bookmarks
                .into_iter()
                .map(|b| {
                    let view = b.view.map_or_else(Default::default, |(kind, numbers)| {
                        let numbers: Vec<Option<f32>> = numbers.iter().map(|n| n.map(|n| n as f32)).collect();
                        pdfcraft_render::DestView::from_operands(Some(&kind), &numbers)
                    });
                    BookmarkRow { path: b.path, title: b.title, open: b.open, page: b.page, view, color: b.color, italic: b.italic, bold: b.bold }
                })
                .collect();
            return Some(BookmarkPage { rows, next: page.next, truncated: false });
        }
        let all = outline_rows(&doc.info.outline);
        let end = offset.saturating_add(limit);
        let rows = all.iter().skip(offset).take(limit).cloned().collect();
        Some(BookmarkPage { rows, next: (end < all.len()).then_some(end), truncated: doc.info.outline_more })
    }

    /// Top-level bookmarks as split points: (first page of each part, its bookmark's title).
    pub fn bookmark_splits(&self, id: DocId) -> Vec<(usize, String)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let top: Vec<(Option<usize>, String)> = match doc.editor.as_ref() {
            Some(editor) => pdfcraft_organize::top_level_bookmarks(&editor.cos).into_iter().map(|b| (b.page, b.title)).collect(),
            None => doc.info.outline.iter().map(|o| (o.page, o.title.clone())).collect(),
        };
        let mut out: Vec<(usize, String)> = top.into_iter().filter_map(|(page, title)| Some((page?, title))).collect();
        out.sort_by_key(|x| x.0);
        out.dedup_by_key(|x| x.0);
        out
    }

    /// Open freshly created bytes (combine / extract) as a new, unsaved document.
    pub fn open_new(&mut self, name: impl Into<String>, bytes: Arc<Vec<u8>>) -> Result<DocId, OpenError> {
        let id = self.open(name, None, bytes, None)?;
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.dirty = true;
            if let Some(e) = d.editor.as_mut() {
                e.unsaved_file = true;
            }
            // Unsaved work with no edit yet still needs its first recovery snapshot.
            d.generation += 1;
        }
        Ok(id)
    }

    /// Show or hide a layer (optional content group) for viewing. Returns `true` if it changed;
    /// callers must drop cached rasters and text for the document.
    pub fn set_layer_visible(&mut self, id: DocId, layer: usize, visible: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        let Some(l) = doc.info.layers.get_mut(layer) else { return false };
        if l.visible == visible {
            return false;
        }
        l.visible = visible;
        use_layer_choices(doc);
        true
    }

    /// Run a set-layer-visibility action (`SetOCGState`): apply `changes` in order, naming each
    /// layer by its optional content group; groups that aren't layers are skipped. With
    /// `preserve_rb`, a layer turned on turns off the other layers of its radio-button groups.
    /// Returns `true` if any layer changed; callers must drop cached rasters and text for the
    /// document, as for [`Self::set_layer_visible`].
    pub fn set_layer_state(&mut self, id: DocId, changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if !apply_layer_state(&mut doc.info.layers, &doc.info.layer_groups, changes, preserve_rb) {
            return false;
        }
        use_layer_choices(doc);
        true
    }

    /// Show each layer as the file opens (Acrobat's Layers ▸ Reset to Initial Visibility). Returns
    /// `true` if any layer changed; callers must drop cached rasters and text for the document,
    /// as for [`Self::set_layer_visible`].
    pub fn reset_layers(&mut self, id: DocId) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        let mut changed = false;
        for l in &mut doc.info.layers {
            changed |= l.visible != l.initial;
            l.visible = l.initial;
        }
        if changed {
            use_layer_choices(doc);
        }
        changed
    }

    /// Hide or show every comment on the page (Acrobat's Comments ▸ Hide all comments). Form
    /// fields and links still draw. Returns whether anything changed.
    pub fn set_hide_comments(&mut self, id: DocId, hide: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if doc.config.hide_comments == hide {
            return false;
        }
        doc.config.hide_comments = hide;
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.generation += 1;
        true
    }

    /// Set the document's reading colours (Acrobat's Replace Document Colors). Returns whether
    /// anything changed; callers must drop cached rasters and text for the document.
    pub fn set_reading_colors(&mut self, id: DocId, colors: ReadingColors) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if doc.config.reading_colors == colors {
            return false;
        }
        doc.config.reading_colors = colors;
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.generation += 1;
        true
    }

    /// The document's reading-colours setting; `None` when no such document is open.
    pub fn reading_colors(&self, id: DocId) -> Option<ReadingColors> {
        Some(self.get(id)?.config.reading_colors)
    }

    /// How the document's pages are drawn for viewing: its password and view settings (layer
    /// visibility, hidden comments, reading colours). `None` when no such document is open.
    pub fn render_config(&self, id: DocId) -> Option<RenderConfig> {
        Some(self.get(id)?.config.clone())
    }

    /// Acrobat's Summarize Comments ("Comments only" layout): a new PDF listing every comment
    /// with its number, author, type, date, text and replies, grouped by page.
    pub fn summarize_comments(&self, id: DocId, sort: SummarySort) -> Result<Arc<Vec<u8>>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let text = comment_summary(&doc.name, &doc.info.annotations, sort);
        self.create_report(&format!("Summary of Comments on {}", doc.name), &text)
    }

    pub fn close(&mut self, id: DocId) {
        self.docs.retain(|d| d.id != id);
    }

    pub fn get(&self, id: DocId) -> Option<&Document> {
        self.docs.iter().find(|d| d.id == id)
    }

    /// Read document `id`'s font list again if content edits since the last full inspection may
    /// have changed it (Document Properties ▸ Fonts, `doc_info`). A no-op otherwise.
    pub fn refresh_fonts(&mut self, id: DocId) {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id && d.fonts_stale) else { return };
        if let Ok(info) = pdfcraft_render::inspect(doc.bytes.clone(), doc.password.as_deref()) {
            doc.info.fonts = info.fonts;
        }
        doc.fonts_stale = false;
    }

    pub fn docs(&self) -> &[Document] {
        &self.docs
    }
}
