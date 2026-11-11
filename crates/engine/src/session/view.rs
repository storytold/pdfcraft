use super::{
    Arc, BookmarkPage, BookmarkRow, DocId, Document, EditError, LayerOp, OpenError, ReadingColors, RenderConfig, RenderPool, Session, SummarySort,
    apply_layer_state, comment_summary, display_config, local_utc_offset, outline_rows, parse_ymd, render_threads, use_layer_choices,
};

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

    /// A dynamic stamp's second line: "By Ada at 2:14 pm, Oct 02, 2026" (local time).
    pub fn stamp_by_line(&self, author: &str) -> String {
        let (date, time) = self.stamp_date_time();
        let who = if author.trim().is_empty() { String::new() } else { format!("By {} ", author.trim()) };
        format!("{who}at {time}, {date}")
    }

    /// A custom stamp's text line (#1741): `template` with `{name}` (the author), `{date}`
    /// ("Oct 02, 2026") and `{time}` ("2:14 pm") filled in now, in local time, as the dynamic
    /// stamps' "By" line has them. Cut to [`MAX_STAMP_LINE`](crate::MAX_STAMP_LINE) characters.
    pub fn stamp_line(&self, template: &str, author: &str) -> String {
        let (date, time) = self.stamp_date_time();
        // One pass over the template, so a placeholder the author's name happens to contain
        // (`Ada {date}`) stays as written rather than being filled in too.
        let mut filled = String::new();
        let mut rest = template.trim();
        while let Some(c) = rest.chars().next() {
            if let Some(after) = rest.strip_prefix("{name}") {
                filled.push_str(author.trim());
                rest = after;
            } else if let Some(after) = rest.strip_prefix("{date}") {
                filled.push_str(&date);
                rest = after;
            } else if let Some(after) = rest.strip_prefix("{time}") {
                filled.push_str(&time);
                rest = after;
            } else {
                filled.push(c);
                rest = rest.get(c.len_utf8()..).unwrap_or_default();
            }
        }
        filled.chars().take(crate::MAX_STAMP_LINE).collect()
    }

    /// Now, as the stamps write it: ("Oct 02, 2026", "2:14 pm"), local time.
    fn stamp_date_time(&self) -> (String, String) {
        let offset = self.local_offset();
        let d = self.now().map(|t| pdfcraft_cos::pdf_date(t + offset)).unwrap_or_default();
        let num = |a: usize, b: usize| d.get(a..b).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let (y, mo, day, hh, mm) = (num(2, 6), num(6, 8), num(8, 10), num(10, 12), num(12, 14));
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let h12 = if hh % 12 == 0 { 12 } else { hh % 12 };
        let month = MONTHS.get(mo.clamp(1, 12) as usize - 1).copied().unwrap_or("Jan");
        (format!("{month} {day:02}, {y}"), format!("{h12}:{mm:02} {}", if hh < 12 { "am" } else { "pm" }))
    }

    /// Today's date in local time: (year, month, day). With an injected clock (tests) the clock
    /// is taken as local time.
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
    /// per line in user space, URI). Text that already has a link over it is skipped, and so is
    /// text on a layer the viewer hides: a link there would sit over nothing the reader can see.
    pub fn find_urls(&self, id: DocId) -> Vec<(usize, Vec<[f64; 4]>, String)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
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
                    .filter_map(|v| line_user_rect(&doc.info.pages[page], v))
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

    /// The addresses written as plain text that the viewer makes clickable
    /// ([`crate::text_links::find_text_links`]), leaving out any a link annotation lies over, from
    /// the 0-based page `from` on. One call reads at most
    /// [`MAX_TEXT_LINK_PAGES_PER_SCAN`](crate::text_links::MAX_TEXT_LINK_PAGES_PER_SCAN) pages
    /// and stops after the page that brings it to
    /// [`MAX_TEXT_LINKS_PER_SCAN`](crate::text_links::MAX_TEXT_LINKS_PER_SCAN) addresses; then
    /// `next_page` says where to go on.
    pub fn text_links(&self, id: DocId, from: usize) -> crate::text_links::TextLinkScan {
        use crate::text_links::{MAX_TEXT_LINK_PAGES_PER_SCAN, MAX_TEXT_LINKS_PER_SCAN, TextLinkScan};
        let Some(doc) = self.get(id) else { return TextLinkScan::default() };
        // The viewer's settings, so text on a layer it hides is not a link.
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), doc.config.clone());
        let mut scan = TextLinkScan::default();
        let pages = doc.info.pages.len();
        for (page, info) in doc.info.pages.iter().enumerate().skip(from).take(MAX_TEXT_LINK_PAGES_PER_SCAN) {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            if let Some(text) = res.text {
                let annotations: Vec<_> = doc.info.links.iter().filter(|l| l.page == page).collect();
                for link in crate::text_links::find_text_links(&text) {
                    if !annotations.iter().any(|l| link.covered_by(info, l.rect)) {
                        scan.links.push((page, link));
                    }
                }
            }
            let next = page.saturating_add(1);
            let read = next.saturating_sub(from);
            if next < pages && (read >= MAX_TEXT_LINK_PAGES_PER_SCAN || scan.links.len() >= MAX_TEXT_LINKS_PER_SCAN) {
                scan.next_page = Some(next);
                break;
            }
        }
        scan
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

    /// The bookmark at `path` with everything under it, or the whole outline for `[]`, as a tree
    /// another document can take ([`crate::Edit::ImportBookmarks`]). `None` when no document has
    /// `id`, or its structure could not be read for editing.
    pub fn outline_tree(&self, id: DocId, path: &[usize]) -> Option<Result<pdfcraft_organize::OutlineTree, pdfcraft_organize::OutlineError>> {
        let editor = self.get(id)?.editor.as_ref()?;
        Some(pdfcraft_organize::outline_tree(&editor.cos, path))
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
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), display_config(&doc.config));
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
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), display_config(&doc.config));
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
        // The summary copies the comments' text out into a new document.
        doc.check_copying()?;
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

/// Convert a view-space text line rectangle into a normalized, finite PDF user-space rectangle.
fn line_user_rect(page: &pdfcraft_render::PageInfo, v: [f32; 4]) -> Option<[f64; 4]> {
    if !v.iter().all(|x| x.is_finite()) {
        return None;
    }
    let r = page.view_rect_to_user(v).map(f64::from);
    r.iter().all(|x| x.is_finite()).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::line_user_rect;
    use crate::{Edit, Session};
    use pdfcraft_render::PageInfo;
    use std::sync::Arc;

    /// Build a sample `PageInfo` with a non-origin crop box `[10.0, 20.0, 210.0, 320.0]` (`200 × 300`).
    fn sample_page(rotation: u16) -> PageInfo {
        let (width, height) = if rotation % 180 == 90 { (300.0, 200.0) } else { (200.0, 300.0) };
        PageInfo { width, height, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation }
    }

    #[test]
    fn line_user_rect_rejects_non_finite_coordinates() {
        let page = sample_page(0);
        assert_eq!(line_user_rect(&page, [f32::NAN; 4]), None);
        assert_eq!(line_user_rect(&page, [f32::NAN, 10.0, 50.0, 20.0]), None);
        assert_eq!(line_user_rect(&page, [10.0, 20.0, f32::INFINITY, 30.0]), None);
        assert_eq!(line_user_rect(&page, [10.0, f32::NEG_INFINITY, 50.0, 30.0]), None);

        for bad_crop_box in [[f32::NAN, 0.0, 200.0, 300.0], [0.0, f32::INFINITY, 200.0, 300.0], [0.0, 0.0, f32::NEG_INFINITY, 300.0]] {
            let mut bad_crop = sample_page(0);
            bad_crop.crop = bad_crop_box;
            assert_eq!(line_user_rect(&bad_crop, [10.0, 20.0, 50.0, 40.0]), None);
        }

        // Finite inputs whose crop span overflows f32 during view-to-user mapping must also be rejected.
        let overflow_page = PageInfo { width: 1.0, height: 1.0, label: String::new(), crop: [-f32::MAX, 0.0, f32::MAX, 300.0], rotation: 0 };
        assert_eq!(line_user_rect(&overflow_page, [1.0, 10.0, 2.0, 20.0]), None);
    }

    #[test]
    fn line_user_rect_matches_view_rect_to_user_across_rotations() {
        let v = [12.0, 24.0, 96.0, 48.0];
        let cases = [
            (0, [22.0, 272.0, 106.0, 296.0]),
            (90, [34.0, 32.0, 58.0, 116.0]),
            (180, [114.0, 44.0, 198.0, 68.0]),
            (270, [162.0, 224.0, 186.0, 308.0]),
        ];
        for (rotation, expected) in cases {
            let page = sample_page(rotation);
            let got = line_user_rect(&page, v).expect("finite view rect maps to user space");
            for (g, e) in got.into_iter().zip(expected) {
                assert!((g - e).abs() < 1e-4, "rotation {rotation}: got {got:?}, expected {expected:?}");
            }
        }
    }

    #[test]
    fn find_urls_returns_finite_user_rects_on_rotated_page() {
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
        ];
        let body = "BT /F1 12 Tf 20 150 Td (Visit https://example.com today) Tj ET";
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            pdf.extend_from_slice(o);
            pdf.extend_from_slice(b"\nendobj\n");
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());

        let mut session = Session::new();
        let id = session.open("urls.pdf", None, Arc::new(pdf), None).expect("opens");
        let expected_rect = [46.004, 147.6, 153.38, 159.6];
        let base_urls = session.find_urls(id);
        assert_eq!(base_urls.len(), 1);
        let base_rect = base_urls[0].1[0];
        for (got, exp) in base_rect.into_iter().zip(expected_rect) {
            assert!((got - exp).abs() < 1e-2, "0 deg: got {base_rect:?}, expected {expected_rect:?}");
        }

        for step in [90, 180, 270] {
            session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).expect("rotates");
            let urls = session.find_urls(id);
            assert_eq!(urls.len(), 1);
            assert_eq!(urls[0].0, 0);
            assert_eq!(urls[0].2, "https://example.com");
            assert_eq!(urls[0].1.len(), 1);
            let r = urls[0].1[0];
            for (got, exp) in r.into_iter().zip(expected_rect) {
                assert!((got - exp).abs() < 1e-2, "rotation {step} deg: got {r:?}, expected {expected_rect:?}");
            }
        }
    }
}
