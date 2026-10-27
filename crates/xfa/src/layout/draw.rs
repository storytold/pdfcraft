use super::common::clean_name;
use super::items::{Item, Rounded};
use super::*;

impl Layouter<'_> {
    // ───────────────────────────────────────────────────────────────────────── drawing

    pub(super) fn draw_row(&mut self, row: &Subform, cols: &[f64], rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        if let Some(b) = &row.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.with_som(&row.common, sib);
        let inner = rect.inset(&row.common.margin);
        let (cells, _) = self.cells(row, cols, inner.w, &ctx, depth);
        let sibs = SomIndices::of(&row.children);
        for (i, r) in cells {
            if let Some(k) = row.children.get(i) {
                // Cells stretch to the row height.
                self.draw_node(k, Rect::new(inner.x + r.x, inner.y, r.w, inner.h), &ctx, depth + 1, sibs.at(i))?;
            }
        }
        Ok(())
    }

    pub(super) fn draw_positioned_child(&mut self, k: &Node, inner: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        let c = k.common();
        if !c.presence.occupies() {
            return Ok(());
        }
        let (w, h) = self.size_of(k, (inner.w - c.x.unwrap_or(0.0)).max(1.0), ctx, depth + 1);
        let (x, y) = c.top_left(w, h);
        self.draw_node(k, Rect::new(inner.x + x, inner.y + y, w, h), ctx, depth + 1, sib)
    }

    pub(super) fn draw_node(&mut self, node: &Node, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let c = node.common();
        if c.presence == Presence::Invisible {
            return Ok(());
        }
        match node {
            Node::Draw(d) => {
                let start = self.item_count();
                self.draw_draw(d, rect, ctx)?;
                // Text and pictures stay with their border; a bare line or rectangle is cut.
                if matches!(d.value, Value::Text(_) | Value::Rich(_) | Value::Image { .. }) {
                    self.keep_whole(start, rect);
                }
                Ok(())
            }
            Node::Field(f) => {
                let start = self.item_count();
                self.draw_field(f, rect, ctx, None, sib)?;
                self.keep_whole(start, rect);
                Ok(())
            }
            Node::ExclGroup(g) => self.draw_group(g, rect, ctx, depth, sib),
            Node::Area(a) => {
                let ctx = ctx.with_som(&a.common, sib);
                let sibs = SomIndices::of(&a.children);
                for (i, k) in a.children.iter().enumerate() {
                    self.draw_positioned_child(k, rect, &ctx, depth + 1, sibs.at(i))?;
                }
                Ok(())
            }
            Node::Subform(s) => {
                // Every drawn instance spends budget, as a flowed one does, so nested repeats that
                // place nothing still stop.
                self.measures.set(self.measures.get().saturating_add(1));
                self.check_measures()?;
                if let Some(b) = &s.common.border {
                    self.draw_border(b, rect)?;
                }
                let parent_ctx = ctx;
                let ctx = ctx.with_som(&s.common, sib);
                let inner = rect.inset(&s.common.margin);
                let sibs = SomIndices::of(&s.children);
                match s.layout {
                    Layout::Positioned => {
                        for (i, k) in s.children.iter().enumerate() {
                            self.draw_positioned_child(k, inner, &ctx, depth + 1, sibs.at(i))?;
                        }
                    }
                    Layout::Tb => {
                        let mut y = inner.y;
                        for (i, k) in s.children.iter().enumerate() {
                            if !k.common().presence.occupies() {
                                continue;
                            }
                            let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                            // A repeating subform here (a table in an area of a positioned page,
                            // say) gets its instances, as in a flowed one, one under another.
                            for instance in 0..self.instances(k, &ctx.som) {
                                self.draw_node(k, Rect::new(inner.x, y, w, h), &ctx, depth + 1, sibs.at(i) + instance)?;
                                y += h;
                            }
                        }
                    }
                    Layout::LrTb | Layout::RlTb => {
                        // A repeating subform here (rows of fields under a heading) gets its
                        // instances, as in the other layouts.
                        let (placed, _) = self.pack_instances(&s.children, inner.w, &ctx, depth, &ctx.som);
                        for (i, instance, r) in placed {
                            if let Some(k) = s.children.get(i) {
                                let x = if s.layout == Layout::RlTb { inner.right() - r.x - r.w } else { inner.x + r.x };
                                self.draw_node(k, Rect::new(x, inner.y + r.y, r.w, r.h), &ctx, depth + 1, sibs.at(i) + instance)?;
                            }
                        }
                    }
                    Layout::Table => {
                        let cols = self.columns(s, inner.w);
                        let mut y = inner.y;
                        for (i, k) in s.children.iter().enumerate() {
                            if !k.common().presence.occupies() {
                                continue;
                            }
                            let csib = sibs.at(i);
                            if let Node::Subform(r) = k
                                && r.layout == Layout::Row
                            {
                                let h = self.row_height(r, &cols, inner.w, &ctx, depth + 1);
                                self.draw_row(r, &cols, Rect::new(inner.x, y, inner.w, h), &ctx, depth + 1, csib)?;
                                y += h;
                            } else {
                                let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                                self.draw_node(k, Rect::new(inner.x, y, w, h), &ctx, depth + 1, csib)?;
                                y += h;
                            }
                        }
                    }
                    Layout::Row => {
                        let cols = self.columns(s, inner.w);
                        self.draw_row(s, &cols, rect, parent_ctx, depth, sib)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// Draw border `b` around `rect`: its fill, then each shown edge as a line inside the box, or
    /// the whole box as one rounded shape when its corners are rounded.
    pub(super) fn draw_border(&mut self, b: &Border, rect: Rect) -> Result<(), XfaError> {
        let vis = b.visible_edges();
        // Rounded corners: the box is one shape, filled and edged along its curves, when its
        // edges are all alike (all shown and the same, or all hidden). Edges that differ keep
        // square corners.
        let radii = b.radii(rect.w, rect.h);
        let alike = b.edges.iter().all(|e| *e == b.edges[0]);
        if radii.iter().any(|r| *r > 0.0) && (vis == [true; 4] && alike || vis == [false; 4]) {
            let e = b.edges[0];
            let edge = vis[0].then(|| {
                let color = if e.stroke == Stroke::Relief { [0.55, 0.55, 0.55] } else { e.color.0 };
                (e.thickness.clamp(0.1, 20.0), color, matches!(e.stroke, Stroke::Dashed | Stroke::Dotted))
            });
            let shape = Rounded {
                rect,
                radii,
                inverted: b.corners.map(|c| c.inverted),
                fill: b.fill().map(|f| f.0),
                edge,
                open_top: false,
                open_bottom: false,
            };
            return self.push(Item::Rounded(Box::new(shape)));
        }
        if let Some(fill) = b.fill() {
            self.push(Item::Fill { rect, color: fill.0 })?;
        }
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
        // Top, right, bottom, left; lines sit inside the box.
        let segs = [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x0, y1), (x1, y1)), ((x0, y0), (x0, y1))];
        for (i, seg) in segs.iter().enumerate() {
            if !vis[i] {
                continue;
            }
            let e = b.edges[i];
            let t = e.thickness.clamp(0.1, 20.0);
            let (dx, dy) = match i {
                0 => (0.0, t / 2.0),
                1 => (-t / 2.0, 0.0),
                2 => (0.0, -t / 2.0),
                _ => (t / 2.0, 0.0),
            };
            let color = if e.stroke == Stroke::Relief { [0.55, 0.55, 0.55] } else { e.color.0 };
            self.push(Item::Line {
                from: (seg.0.0 + dx, seg.0.1 + dy),
                to: (seg.1.0 + dx, seg.1.1 + dy),
                width: t,
                color,
                dashed: matches!(e.stroke, Stroke::Dashed | Stroke::Dotted),
            })?;
        }
        Ok(())
    }

    pub(super) fn draw_value_text(&mut self, v: &Value, rect: Rect, font: &Font, para: &Para) -> Result<(), XfaError> {
        let rich = match v {
            Value::Text(t) => text::plain(t),
            Value::Rich(r) => r.clone(),
            _ => return Ok(()),
        };
        let face = Face::of(font);
        // Text that doesn't fit the height it was given (the template's face is narrower than
        // the standard one drawn in its place, so it wraps to more lines) is drawn smaller,
        // down to 70%, rather than over what lies below.
        let fits = |k: f64| text::measure_height(&rich, rect.w / k, para, &face, &|id, max| self.embed(id, max)) <= rect.h / k + 1.0;
        let scale = if rect.h < 4.0 || fits(1.0) { 1.0 } else { [0.95, 0.9, 0.85, 0.8, 0.75].into_iter().find(|k| fits(*k)).unwrap_or(0.7) };
        let mut block = text::layout(&rich, Rect::new(rect.x, rect.y, rect.w / scale, rect.h / scale), para, &face, &|id, max| self.embed(id, max));
        if scale < 1.0 {
            for s in &mut block.spans {
                s.x = rect.x + (s.x - rect.x) * scale;
                s.baseline = rect.y + (s.baseline - rect.y) * scale;
                s.face.size *= scale;
            }
        }
        self.push_block(block)
    }

    fn draw_draw(&mut self, d: &Draw, rect: Rect, ctx: &Ctx) -> Result<(), XfaError> {
        if let Some(b) = &d.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.child(&d.common);
        let inner = rect.inset(&d.common.margin);
        match &d.value {
            Value::Text(_) | Value::Rich(_) => self.draw_value_text(&d.value, inner, &ctx.font, &ctx.para),
            Value::Image { content_type, data } => self.draw_image(data, content_type, inner),
            Value::Line { slope_up, edge } => {
                if !edge.visible() {
                    return Ok(());
                }
                let (from, to) = if *slope_up {
                    ((inner.x, inner.bottom()), (inner.right(), inner.y))
                } else {
                    ((inner.x, inner.y), (inner.right(), inner.bottom()))
                };
                self.push(Item::Line { from, to, width: edge.thickness.clamp(0.1, 20.0), color: edge.color.0, dashed: edge.stroke == Stroke::Dashed })
            }
            Value::Rectangle(b) => self.draw_border(b, inner),
            Value::Empty => Ok(()),
        }
    }

    fn draw_group(&mut self, g: &ExclGroup, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if let Some(b) = &g.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.with_som(&g.common, sib);
        // The group's data node holds the selected button's on value.
        let selected: Option<String> =
            self.data.and_then(|d| d.text_at(&data_path(self.tpl, &ctx.som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let inner = rect.inset(&g.common.margin);
        let group = self.unique_name(g.common.name.as_deref().unwrap_or("group"));
        let nodes: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
        let placed: Vec<(usize, Rect)> = match g.layout {
            Layout::Tb => {
                let mut y = 0.0;
                let mut out = Vec::new();
                for (i, k) in nodes.iter().enumerate() {
                    if !k.common().presence.occupies() {
                        continue;
                    }
                    let (w, h) = self.size_of(k, inner.w, &ctx, depth + 1);
                    out.push((i, Rect::new(0.0, y, w, h)));
                    y += h;
                }
                out
            }
            Layout::Positioned => nodes
                .iter()
                .enumerate()
                .filter(|(_, k)| k.common().presence.occupies())
                .map(|(i, k)| {
                    let c = k.common();
                    let (w, h) = self.size_of(k, (inner.w - c.x.unwrap_or(0.0)).max(1.0), &ctx, depth + 1);
                    let (x, y) = c.top_left(w, h);
                    (i, Rect::new(x, y, w, h))
                })
                .collect(),
            _ => self.pack(&nodes, inner.w, &ctx, depth).0,
        };
        let sibs = SomIndices::of(&nodes);
        for (i, r) in placed {
            if let Some(f) = g.fields.get(i) {
                let on = f.items.first().cloned().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| (i + 1).to_string());
                let tip = f.tooltip.clone().or_else(|| g.tooltip.clone());
                let chosen = selected.as_deref() == Some(on.as_str());
                self.draw_field(f, Rect::new(inner.x + r.x, inner.y + r.y, r.w, r.h), &ctx, Some((&group, &on, tip, chosen)), sibs.at(i))?;
            }
        }
        Ok(())
    }

    /// A picture scaled to fit `inner`, keeping its shape, from its top left.
    pub(super) fn draw_image(&mut self, data: &[u8], content_type: &str, inner: Rect) -> Result<(), XfaError> {
        self.image_bytes = self.image_bytes.saturating_add(data.len());
        if self.image_bytes > MAX_IMAGE_BYTES {
            return Err(XfaError::TooLarge("the form's pictures are too large together to lay out".into()));
        }
        let fitted = match crate::image::size(data) {
            Some((iw, ih)) if iw > 0 && ih > 0 && inner.w > 0.0 && inner.h > 0.0 => {
                let k = (inner.w / iw as f64).min(inner.h / ih as f64);
                Rect::new(inner.x, inner.y, iw as f64 * k, ih as f64 * k)
            }
            _ => inner,
        };
        self.push(Item::Image { rect: fitted, data: Arc::new(data.to_vec()), content_type: content_type.to_string() })
    }

    /// A field name made from `base` that no earlier field of the form has: the first keeps it,
    /// later ones get `_2`, `_3`, ….
    pub(super) fn unique_name(&mut self, base: &str) -> String {
        let base = clean_name(base);
        let n = self.names.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { base } else { format!("{base}_{n}") }
    }
}
