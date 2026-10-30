use super::common::{clamp_opt, divide, is_row, moved, solid_extent, splittable};
use super::items::Item;
use super::{Ctx, Frame, Layout, Layouter, MAX_DEPTH, MAX_ITEMS, MAX_MEASURES, Node, Rect, SomIndices, Subform, XfaError, data_path, instance_count};

impl Layouter<'_> {
    /// Place `node` at the cursor, splitting an intrinsic horizontal flow between its leaves.
    /// Other blocks move to the next area when they fit there; oversized blocks are split.
    fn flow_block(&mut self, node: &Node, x: f64, (w, h): (f64, f64), ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        let (rect, y, _) = self.cursor();
        let x_offset = x - rect.x;
        // Removing the final optional row can shrink a flowing section from taller than a
        // page to shorter than one. That must not suddenly make the section indivisible and
        // discard the space left on this page. For compatibility with reference-viewer
        // pagination, keep/intact hints do not make these growable groups indivisible.
        // split_block still keeps individual fields and text whole.
        let horizontal_flow = matches!(node, Node::Subform(s)
            if matches!(s.layout, Layout::LrTb | Layout::RlTb) && s.common.h.is_none());
        if !horizontal_flow && y + h > rect.bottom() + 0.01 && h <= rect.h + 0.01 {
            self.make_room(h, 0.0)?;
        }
        let (rect, y, _) = self.cursor();
        let x = rect.x + x_offset;
        let first = self.pages.last().map_or(0, |p| p.items.len());
        self.leaves.clear();
        self.draw_node(node, Rect::new(x, y, w, h), ctx, depth, sib)?;
        let page_h = self.pages.last().map_or(f64::INFINITY, |p| p.height);
        // What was drawn, when it is lower than the block's own height says (content that
        // outgrows a fixed height) and ends off the page.
        let drawn = self
            .pages
            .last()
            .and_then(|p| p.items.get(first..))
            .unwrap_or_default()
            .iter()
            .filter_map(solid_extent)
            .map(|e| e.1)
            .fold(y + h, f64::max);
        if y + h <= rect.bottom() + 0.01 && drawn <= page_h + 0.01 {
            self.leaves.clear();
            self.place(h);
            return Ok(());
        }
        self.split_block(first, y, drawn)
    }

    /// Remember that the items the last page got since `start` are one field or draw at
    /// `rect`, to be kept together by a page break.
    pub(super) fn keep_whole(&mut self, start: usize, rect: Rect) {
        let end = self.pages.last().map_or(0, |p| p.items.len());
        if end > start {
            self.leaves.push((start, end, rect.y, rect.bottom()));
        }
    }

    pub(super) fn item_count(&self) -> usize {
        self.pages.last().map_or(0, |p| p.items.len())
    }

    /// Spread the items the last page holds from index `first` on (a block drawn from `top` to
    /// `end`, past the bottom of its content area) over this and the following content areas.
    /// Breaks fall between text, pictures and fields, never through one that fits in an area;
    /// fills and lines are cut at the break. A break between whole leaves segments the
    /// enclosing flowed frames too; splitting an oversized leaf still loses their borders.
    fn split_block(&mut self, first: usize, top: f64, end: f64) -> Result<(), XfaError> {
        let items: Vec<Item> = match self.pages.last_mut() {
            Some(p) if first <= p.items.len() => p.items.split_off(first),
            _ => return Ok(()),
        };
        // Each item with the extent of the field or draw it belongs to: such a leaf moves
        // whole (its border with its text), unless it is taller than a content area.
        let area_h = self.cursor().0.h;
        let mut extents: Vec<Option<(f64, f64)>> = vec![None; items.len()];
        let leaves = std::mem::take(&mut self.leaves);
        for &(start, end, top, bottom) in &leaves {
            if bottom - top <= area_h + 0.01
                && let Some(slots) = start.checked_sub(first).and_then(|s| extents.get_mut(s..end.saturating_sub(first)))
            {
                slots.fill(Some((top, bottom)));
            }
        }
        let mut rest: Vec<(Item, Option<(f64, f64)>)> = items.into_iter().zip(extents).collect();
        // Solid items move whole, so their original coordinates can be sorted once and
        // discarded as each part is placed, rather than sorted again for every area.
        let mut solid: Vec<(f64, f64)> = rest.iter().filter_map(|(item, kept)| kept.or_else(|| solid_extent(item))).collect();
        solid.sort_by(|a, b| b.0.total_cmp(&a.0));
        let origin_x = self.cursor().0.x;
        // An item's place on the current page is its drawn place moved by (dx, -shift).
        let (mut dx, mut shift) = (0.0, 0.0);
        let mut seg_top = top;
        let mut first_segment = true;
        loop {
            let (rect, _, placed) = self.cursor();
            let limit = rect.bottom() + shift;
            let mut cut = limit;
            if rect.h < 1.0 {
                // No room to split into: leave the rest here.
                cut = f64::INFINITY;
            } else {
                // The lowest break at or above the bottom that crosses no solid item: going
                // through them from the lowest top up, a break moved to an item's top can't
                // cross one already passed.
                for &(t, b) in solid.iter().filter(|e| e.0 < limit - 0.01) {
                    if t < cut - 0.01 && b > cut + 0.01 {
                        cut = t;
                    }
                }
                if cut <= seg_top + 0.01 {
                    // An item taller than what is left. Start it in a fresh area if this one
                    // already holds something; otherwise cut through it.
                    cut = if first_segment && placed && seg_top > rect.y + 0.01 { seg_top } else { limit };
                }
            }
            let mut here = Vec::new();
            let mut below = Vec::new();
            for (item, kept) in rest {
                if let Some((top, _)) = kept {
                    if top < cut - 0.01 {
                        here.push(item)
                    } else {
                        below.push((item, kept))
                    }
                    continue;
                }
                match divide(item, cut) {
                    (Some(a), None) => here.push(a),
                    (None, Some(b)) => below.push((b, None)),
                    (Some(a), Some(b)) => {
                        here.push(a);
                        below.push((b, None));
                    }
                    (None, None) => {}
                }
            }
            if let Some(p) = self.pages.last_mut() {
                p.items.extend(here.into_iter().map(|it| moved(it, dx, -shift)));
            }
            if below.is_empty() {
                if let Some(c) = &mut self.cur {
                    c.y = (end - shift).min(c.rect.bottom()).max(c.y);
                    c.placed = true;
                }
                self.overflowed(end - shift);
                return Ok(());
            }
            rest = below;
            seg_top = cut;
            first_segment = false;
            let cuts_leaf =
                leaves.iter().any(|&(_, _, t, b)| t < cut - 0.01 && b > cut + 0.01) || solid.iter().any(|&(t, b)| t < cut - 0.01 && b > cut + 0.01);
            if cuts_leaf {
                // Preserve the oversized-leaf fallback: its enclosing frames no longer fit
                // their content, so finish_frame omits them and records the warning.
                self.next_area()?;
            } else {
                // Reserve the next band's whole leaves (including overlapping leaves and
                // any leading space), so repeated top margins cannot push them off the area.
                let mut bottom = None::<f64>;
                for &(t, b) in solid.iter().rev().filter(|e| e.0 >= cut - 0.01) {
                    if bottom.is_some_and(|end| t >= end - 0.01) {
                        break;
                    }
                    bottom = Some(bottom.map_or(b, |end| end.max(b)));
                }
                if let Some(c) = &mut self.cur {
                    c.y = cut - shift;
                }
                self.break_frames(bottom.map_or(0.0, |b| (b - cut).max(0.0)), false)?;
            }
            let (rect, y, _) = self.cursor();
            shift = cut - y;
            dx = rect.x - origin_x;
            solid.retain(|e| e.0 >= cut - 0.01);
        }
    }

    fn page_has_flow(&self) -> bool {
        self.cur.as_ref().is_some_and(|c| c.placed || c.content > 0)
    }

    // ───────────────────────────────────────────────────────────────────────── flow

    /// Flow `node` (a subform; anything else is ignored) into the content areas.
    pub(super) fn flow_subform(&mut self, node: &Node, avail: Option<(f64, f64)>, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let Node::Subform(sf) = node else { return Ok(()) };
        self.check_measures()?;
        let instances = self.instances(node, &ctx.som);
        self.warn_truncated(node, &ctx.som, instances);
        if instances == 0 {
            return Ok(());
        }
        // Keep inherited horizontal placement relative to the content area across breaks.
        let avail = avail.map(|(x, w)| (x - self.cursor().0.x, w));
        if sf.break_before_page && self.page_has_flow() {
            self.break_frames(0.0, true)?;
        }
        // Data adds instances of a repeating subform (rows added in another viewer).
        let ctx_parent = ctx;
        for i in 0..instances {
            // Every instance spends budget, so nested repeats that place nothing still stop.
            self.measures.set(self.measures.get().saturating_add(1));
            self.check_measures()?;
            let ctx = ctx_parent.with_som(&sf.common, sib + i);
            let (rect, _, _) = self.cursor();
            let (x, w) = avail.map(|(offset, w)| (rect.x + offset, w)).unwrap_or((rect.x, rect.w));
            let w = clamp_opt(sf.common.w.unwrap_or(w), sf.common.min_w, sf.common.max_w).min(w.max(1.0));
            if !matches!(sf.layout, Layout::Tb | Layout::Table) || sf.common.h.is_some() {
                // Not splittable: one block.
                let (_, h) = self.size_of(node, w, ctx_parent, depth, sib + i);
                self.flow_block(node, x, (w, h), ctx_parent, depth, sib + i)?;
                continue;
            }
            let m = sf.common.margin;
            // Its border and fill go round what it takes from here, a part on each content area.
            let framed = match self.flow_mark() {
                Some(part) => {
                    self.frames.push(Frame { x, w, margin: m, border: sf.common.border.clone(), part, parts: Vec::new(), lost: false });
                    true
                }
                None => false,
            };
            self.advance(m.top);
            let inner_x = x + m.left;
            let inner_w = (w - m.horizontal()).max(1.0);
            match sf.layout {
                Layout::Table => self.flow_table(sf, inner_x, inner_w, &ctx, depth)?,
                _ => {
                    let sibs = SomIndices::of(&sf.children);
                    for (ci, child) in sf.children.iter().enumerate() {
                        if !child.common().presence.occupies() || self.instances(child, &ctx.som) == 0 {
                            continue;
                        }
                        let csib = sibs.at(ci);
                        if let Node::Subform(_) = child
                            && splittable(child)
                        {
                            let child_x = self.cursor().0.x + inner_x - rect.x;
                            self.flow_subform(child, Some((child_x, inner_w)), &ctx, depth + 1, csib)?;
                            continue;
                        }
                        for instance in 0..self.instances(child, &ctx.som) {
                            let (cw, ch) = self.size_of(child, inner_w, &ctx, depth + 1, csib + instance);
                            let child_x = self.cursor().0.x + inner_x - rect.x;
                            self.flow_block(child, child_x, (cw, ch), &ctx, depth + 1, csib + instance)?;
                        }
                    }
                }
            }
            self.advance(m.bottom);
            if framed && let Some(frame) = self.frames.pop() {
                self.finish_frame(frame)?;
            }
        }
        if sf.break_after_page {
            self.break_frames(0.0, true)?;
        }
        Ok(())
    }

    pub(super) fn columns(&self, sf: &Subform, inner_w: f64, ctx: &Ctx) -> Vec<f64> {
        if !sf.column_widths.is_empty() {
            return sf.column_widths.clone();
        }
        // Without declared widths, keep the equal-column fallback, but derive its
        // column count from instantiated cells rather than template declarations.
        let mut cells = if sf.layout == Layout::Row { self.row_column_count(sf, ctx) } else { 1 };
        if sf.layout != Layout::Row {
            let sibs = SomIndices::of(&sf.children);
            for (i, child) in sf.children.iter().enumerate() {
                let Node::Subform(row) = child else { continue };
                if row.layout != Layout::Row || !row.common.presence.occupies() {
                    continue;
                }
                for instance in 0..self.instances(child, &ctx.som) {
                    self.measures.set(self.measures.get().saturating_add(1));
                    if self.measures.get() > MAX_MEASURES {
                        return vec![inner_w]; // check_measures rejects the layout before publication.
                    }
                    cells = cells.max(self.row_column_count(row, &ctx.with_som(&row.common, sibs.at(i) + instance)));
                }
            }
        }
        let cells = cells.max(1);
        vec![inner_w / cells as f64; cells]
    }

    fn row_column_count(&self, row: &Subform, ctx: &Ctx) -> usize {
        let mut columns = 0usize;
        for child in &row.children {
            self.measures.set(self.measures.get().saturating_add(1));
            if self.measures.get() > MAX_MEASURES {
                return 1;
            }
            if child.common().presence.occupies() {
                columns = columns.saturating_add(self.instances(child, &ctx.som).saturating_mul(child.common().col_span.max(1)));
                if columns > MAX_ITEMS {
                    // Do not allocate a column vector amplified by repetitions/spans.
                    self.measures.set(MAX_MEASURES + 1);
                    return 1;
                }
            }
        }
        columns
    }

    /// Flow each data row at its own height, allowing room for a present overflow leader.
    fn flow_table(&mut self, sf: &Subform, x: f64, inner_w: f64, ctx: &Ctx, depth: usize) -> Result<(), XfaError> {
        let x_offset = x - self.cursor().0.x;
        let cols = self.columns(sf, inner_w, ctx);
        let leader =
            sf.overflow_leader.as_ref().and_then(|name| sf.children.iter().position(|c| c.common().name.as_deref() == Some(name) && is_row(c)));
        // The overflow leader starts each part after the first, before the row that moved.
        let sibs = SomIndices::of(&sf.children);
        let lead = leader.and_then(|li| match sf.children.get(li) {
            Some(node @ Node::Subform(l)) if l.common.presence.occupies() && self.instances(node, &ctx.som) > 0 => Some((li, l.as_ref())),
            _ => None,
        });
        let lead_h = lead.map_or(0.0, |(li, l)| self.row_height(l, &cols, inner_w, ctx, depth + 1, sibs.at(li)));
        for (ci, child) in sf.children.iter().enumerate() {
            if !child.common().presence.occupies() || self.instances(child, &ctx.som) == 0 {
                continue;
            }
            let csib = sibs.at(ci);
            if let Node::Subform(row) = child
                && row.layout == Layout::Row
            {
                // Rows added as data (in another viewer) repeat the row.
                let instances = self.instances(child, &ctx.som);
                self.warn_truncated(child, &ctx.som, instances);
                for inst in 0..instances {
                    let h = self.row_height(row, &cols, inner_w, ctx, depth + 1, csib + inst);
                    let led = lead.filter(|(li, _)| *li != ci);
                    if self.make_room(h, led.map_or(0.0, |_| lead_h))?
                        && let Some((li, l)) = led
                    {
                        let (area, y, _) = self.cursor();
                        let x = area.x + x_offset;
                        self.draw_row(l, &cols, Rect::new(x, y, inner_w, lead_h), ctx, depth + 1, sibs.at(li))?;
                        self.place(lead_h);
                    }
                    let (area, y, _) = self.cursor();
                    let x = area.x + x_offset;
                    self.draw_row(row, &cols, Rect::new(x, y, inner_w, h), ctx, depth + 1, csib + inst)?;
                    self.place(h);
                }
                continue;
            }
            if splittable(child)
                && let Node::Subform(_) = child
            {
                let x = self.cursor().0.x + x_offset;
                self.flow_subform(child, Some((x, inner_w)), ctx, depth + 1, csib)?;
                continue;
            }
            for instance in 0..self.instances(child, &ctx.som) {
                let (cw, ch) = self.size_of(child, inner_w, ctx, depth + 1, csib + instance);
                let x = self.cursor().0.x + x_offset;
                self.flow_block(child, x, (cw, ch), ctx, depth + 1, csib + instance)?;
            }
        }
        Ok(())
    }

    // ───────────────────────────────────────────────────────────────────────── sizing

    /// How many instances of `node` to lay out under the container at `parent_som`: a
    /// repeating subform's `occur` and data instances ([`instance_count`]); one for anything else.
    pub(super) fn instances(&self, node: &Node, parent_som: &str) -> usize {
        let Node::Subform(sf) = node else { return 1 };
        let from_data = self
            .data
            .filter(|d| !d.children.is_empty())
            .and_then(|d| sf.common.name.as_ref().map(|name| d.count(&data_path(self.tpl, parent_som), name)));
        instance_count(&sf.occur, from_data)
    }

    /// The data holds more instances of `node` than `laid_out`: say so, since calculations
    /// then see only those.
    pub(super) fn warn_truncated(&mut self, node: &Node, parent_som: &str, laid_out: usize) {
        let Node::Subform(sf) = node else { return };
        let from_data = match (&sf.common.name, self.data) {
            (Some(name), Some(d)) if sf.occur.max != Some(1) => d.count(&data_path(self.tpl, parent_som), name),
            _ => 0,
        };
        if from_data > laid_out {
            self.warn_once(format!(
                "{}: the data holds {from_data} rows; only the first {laid_out} are laid out (calculations see those)",
                sf.common.name.as_deref().unwrap_or("a subform")
            ));
        }
    }
}
