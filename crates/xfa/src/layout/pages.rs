use super::items::{Item, Page, UNDERLINE_DROP};
use super::*;

impl Layouter<'_> {
    pub(super) fn warn_once(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        if self.warned.insert(m.clone()) && self.warnings.len() < 100 {
            self.warnings.push(m);
        }
    }

    fn page_no(&self) -> usize {
        self.pages.len()
    }

    /// The text an embedded field `id` shows, at most `max` bytes of a field value (the page
    /// number and count are a few bytes; the text layout cuts them).
    pub(super) fn embed(&self, id: &str, max: usize) -> String {
        let v = match self.tpl.page_roles.iter().find(|(i, _)| i == id).map(|(_, r)| *r) {
            Some(PageRole::Number) => self.page_no().to_string(),
            Some(PageRole::Count) => PAGE_COUNT_MARK.to_string(),
            None => self.values.get(id).map(|v| text::prefix(v, max).to_string()).unwrap_or_default(),
        };
        #[cfg(test)]
        EMBED_COPIED.with(|c| c.set(c.get().saturating_add(v.len() as u64)));
        v
    }

    /// `TooLarge` once the measurement budget is spent (sizes measured after that are guesses).
    pub(super) fn check_measures(&self) -> Result<(), XfaError> {
        if self.measures.get() > MAX_MEASURES {
            return Err(XfaError::TooLarge("the form is nested too deeply to lay out".into()));
        }
        Ok(())
    }

    pub(super) fn push(&mut self, item: Item) -> Result<(), XfaError> {
        self.check_measures()?;
        self.items += 1;
        if self.items > MAX_ITEMS {
            return Err(XfaError::TooLarge("the form has too many objects to lay out".into()));
        }
        if let Some(p) = self.pages.last_mut() {
            p.items.push(item);
        }
        Ok(())
    }

    pub(super) fn push_block(&mut self, b: Block) -> Result<(), XfaError> {
        for s in b.spans {
            if s.face.underline {
                let w = s.face.width(&s.text);
                let y = s.baseline + s.face.size * UNDERLINE_DROP;
                self.push(Item::Line { from: (s.x, y), to: (s.x + w, y), width: (s.face.size * 0.06).max(0.3), color: s.face.color, dashed: false })?;
            }
            if !s.text.trim().is_empty() {
                self.push(Item::Text(s))?;
            }
        }
        Ok(())
    }

    // ───────────────────────────────────────────────────────────────────────── pages

    pub(super) fn new_page(&mut self) -> Result<(), XfaError> {
        let idx =
            (0..self.areas.len()).find(|&i| self.areas[i].occur_max.is_none_or(|m| self.uses[i] < m)).unwrap_or(self.areas.len().saturating_sub(1));
        self.new_page_of(idx)
    }

    /// Once the body has been laid out, give every page area that has been used fewer times than
    /// its `occur min` the pages it still needs, in template order, after the body's pages.
    pub(super) fn satisfy_page_area_minimums(&mut self) -> Result<(), XfaError> {
        let areas = Rc::clone(&self.areas);
        for (idx, area) in areas.iter().enumerate() {
            while self.uses.get(idx).is_some_and(|u| *u < area.occur_min) {
                self.new_page_of(idx)?;
            }
        }
        Ok(())
    }

    /// Start a page from page area `idx`.
    fn new_page_of(&mut self, idx: usize) -> Result<(), XfaError> {
        if self.pages.len() >= MAX_PAGES {
            return Err(XfaError::TooLarge(format!("the form lays out to more than {MAX_PAGES} pages")));
        }
        let areas = Rc::clone(&self.areas);
        let Some(area) = areas.get(idx) else {
            return Err(XfaError::Malformed("the template has no page area".into()));
        };
        if let Some(u) = self.uses.get_mut(idx) {
            *u += 1;
        }
        self.pages.push(Page { width: area.width, height: area.height, items: Vec::new() });
        let content = area.content.first().copied().unwrap_or(Rect::new(0.0, 0.0, area.width, area.height));
        self.cur = Some(Cursor { area: idx, content: 0, rect: content, y: content.y, placed: false });
        // Master page content: positioned draws and fields.
        let ctx = Ctx { font: Font::default(), para: Para::default(), som: area.name.clone().map(|n| format!("{n}[0]")).unwrap_or_default() };
        let page_rect = Rect::new(0.0, 0.0, area.width, area.height);
        let sibs = SomIndices::of(&area.items);
        for (i, item) in area.items.iter().enumerate() {
            self.draw_positioned_child(item, page_rect, &ctx, 1, sibs.at(i))?;
        }
        Ok(())
    }

    /// Move to the next content area, or the next page.
    pub(super) fn next_area(&mut self) -> Result<(), XfaError> {
        if let Some(c) = &self.cur
            && let Some(area) = self.areas.get(c.area)
            && let Some(next) = area.content.get(c.content + 1).copied()
        {
            let i = c.content + 1;
            self.cur = Some(Cursor { area: c.area, content: i, rect: next, y: next.y, placed: false });
            return Ok(());
        }
        self.new_page()
    }

    /// The current content area, how far down it the flow is, and whether anything has been
    /// placed in it; before the first page, an empty letter-size page from its top.
    pub(super) fn cursor(&self) -> (Rect, f64, bool) {
        match &self.cur {
            Some(c) => (c.rect, c.y, c.placed),
            None => (Rect::new(0.0, 0.0, 612.0, 792.0), 0.0, false),
        }
    }

    /// Where the flow is now ([`FlowMark`]); `None` before the first page.
    pub(super) fn flow_mark(&self) -> Option<FlowMark> {
        let c = self.cur.as_ref()?;
        Some(FlowMark { page: self.pages.len(), area: c.area, content: c.content, y: c.y, items: self.pages.last().map_or(0, |p| p.items.len()) })
    }

    /// Whether the flow is still in the content area `m` was taken in.
    fn on_area(&self, m: &FlowMark) -> bool {
        self.pages.len() == m.page && self.cur.as_ref().is_some_and(|c| c.area == m.area && c.content == m.content)
    }

    /// Whether page `page` (an index) got anything from item `at` on.
    fn has_items_from(&self, page: usize, at: usize) -> bool {
        self.pages.get(page).is_some_and(|p| p.items.len() > at)
    }

    /// Break the flowed subforms being laid out ([`Self::frames`]) across content areas, as
    /// another viewer breaks such a section on a synthetic form: each one's part here ends after
    /// its content and its bottom margin (the innermost first, each inside the next), the flow
    /// moves to the next content area, and each starts a new part there after its top margin
    /// (the outermost first). The top margins take only what the `need` points of what comes next
    /// leave of the new area: a child that fits a fresh area but not after the margins would
    /// otherwise run past its bottom, out of its border. `to_page` moves to a new page instead (a
    /// child's own page break), which breaks the subforms around it the same way.
    pub(super) fn break_frames(&mut self, need: f64, to_page: bool) -> Result<(), XfaError> {
        let (rect, mut y, _) = self.cursor();
        let page = self.pages.len().saturating_sub(1);
        let here: Vec<bool> = self.frames.iter().map(|f| self.on_area(&f.part)).collect();
        let filled: Vec<bool> = self.frames.iter().map(|f| self.has_items_from(page, f.part.items)).collect();
        for ((f, here), filled) in self.frames.iter_mut().zip(here).zip(filled).rev() {
            y = (y + f.margin.bottom).min(rect.bottom());
            if !here {
                f.lost = true;
            } else if filled && y - f.part.y >= 0.01 {
                f.parts.push((page, f.part.items, Rect::new(f.x, f.part.y, f.w, y - f.part.y)));
            }
        }
        if to_page {
            self.new_page()?;
        } else {
            self.next_area()?;
        }
        let (rect, y, _) = self.cursor();
        let mut room = (rect.bottom() - y - need).max(0.0);
        for i in 0..self.frames.len() {
            let Some(mark) = self.flow_mark() else { break };
            let Some(f) = self.frames.get_mut(i) else { break };
            f.part = mark;
            // `max` before `min`: a NaN margin counts as none and leaves `room` a number.
            let top = f.margin.top.max(0.0).min(room);
            room = (room - top).max(0.0);
            self.advance(top);
        }
        Ok(())
    }

    /// Close flowed subform `f` once its content is placed: its last part ends where the flow
    /// is, and its border (edges and fill) is drawn round each part, closed on every side, under
    /// that part's content. One holding something that didn't fit its content area ([`Frame::lost`])
    /// is drawn without its border, and the layout says so.
    pub(super) fn finish_frame(&mut self, mut f: Frame) -> Result<(), XfaError> {
        let Some(b) = f.border.take() else { return Ok(()) };
        if b.fill().is_none() && b.visible_edges() == [false; 4] {
            return Ok(());
        }
        let (area, end, _) = self.cursor();
        // The bottom margin stays in the content area, as at a break; content that itself ran
        // past it has already cost the section its border ([`Self::overflowed`]).
        let end = end.min(area.bottom()).max(end - f.margin.bottom);
        let page = self.pages.len().saturating_sub(1);
        if !self.on_area(&f.part) {
            f.lost = true;
        } else if self.has_items_from(page, f.part.items) && end - f.part.y >= 0.01 {
            f.parts.push((page, f.part.items, Rect::new(f.x, f.part.y, f.w, end - f.part.y)));
        }
        if f.lost {
            self.warn_once("a bordered section holding an item that doesn't fit on its page is drawn without its border");
            return Ok(());
        }
        // The last part first: a part's place on its page stays where it was found.
        for (page, at, rect) in f.parts.into_iter().rev() {
            self.insert_border(&b, rect, page, at)?;
        }
        Ok(())
    }

    /// Draw border `b` round `rect` on page `page` (an index), under what that page got from item
    /// `at` on.
    fn insert_border(&mut self, b: &Border, rect: Rect, page: usize, at: usize) -> Result<(), XfaError> {
        let before = self.pages.last().map_or(0, |p| p.items.len());
        self.draw_border(b, rect)?;
        let drawn: Vec<Item> = match self.pages.last_mut() {
            Some(p) => {
                let from = before.min(p.items.len());
                p.items.drain(from..).collect()
            }
            None => return Ok(()),
        };
        if let Some(target) = self.pages.get_mut(page) {
            let at = at.min(target.items.len());
            target.items.splice(at..at, drawn);
        }
        Ok(())
    }

    /// Move the cursor down (margins and spacing: the area still counts as empty).
    pub(super) fn advance(&mut self, dy: f64) {
        if let Some(c) = &mut self.cur {
            c.y += dy.max(0.0);
        }
    }

    /// Move past something that was placed.
    pub(super) fn place(&mut self, dy: f64) {
        if let Some(c) = &mut self.cur {
            c.y += dy.max(0.0);
            c.placed = true;
        }
        let (_, y, _) = self.cursor();
        self.overflowed(y);
    }

    /// Something was placed down to `bottom`: past the content area's bottom, the flowed subforms
    /// it is inside can't hold it in a part, and are drawn without their border.
    pub(super) fn overflowed(&mut self, bottom: f64) {
        let (rect, _, _) = self.cursor();
        if bottom > rect.bottom() + 0.01 {
            for f in &mut self.frames {
                f.lost = true;
            }
        }
    }

    /// Make room for `h`, with `after` more drawn before it should it move (a table's overflow
    /// leader): stay, or move to the next content area / page. Returns whether a break happened.
    /// Something that fits a fresh area breaks the flowed subforms it is in with it
    /// ([`Self::break_frames`]), a table's rows included; one taller than an area just moves on,
    /// and those subforms are drawn without their border ([`Self::finish_frame`]).
    pub(super) fn make_room(&mut self, h: f64, after: f64) -> Result<bool, XfaError> {
        let (rect, y, placed) = self.cursor();
        if y + h <= rect.bottom() + 0.01 || !placed {
            return Ok(false);
        }
        if h <= rect.h + 0.01 {
            self.break_frames(h + after, false)?;
        } else {
            self.next_area()?;
        }
        Ok(true)
    }
}
