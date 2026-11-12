use std::collections::BTreeMap;

use super::common::clean_name;
use super::items::{Item, Rounded, Turned};
use super::{
    Arc, Border, Common, Ctx, Draw, ExclGroup, Face, Field, FillKind, Font, Hatch, Layout, Layouter, MAX_DEPTH, MAX_IMAGE_BYTES, Node, Para, Rect,
    Side, SomIndices, Stroke, Subform, Value, XfaError, data_path, text,
};

impl Layouter<'_> {
    // ───────────────────────────────────────────────────────────────────────── drawing

    /// Draw table row `row` at `rect` in columns `cols`, shown where it and what it is inside say
    /// ([`Common::views`]).
    pub(super) fn draw_row(&mut self, row: &Subform, cols: &[f64], rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        self.warn_other_views(&row.common);
        let views = self.views.and(row.common.views());
        if !views.any() {
            return Ok(());
        }
        let outer = std::mem::replace(&mut self.views, views);
        let drawn = self.draw_row_cells(row, cols, rect, ctx, depth, sib);
        self.views = outer;
        drawn
    }

    /// [`Self::draw_row`]'s border and cells, once where the row shows is set.
    pub(super) fn draw_row_cells(&mut self, row: &Subform, cols: &[f64], rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if let Some(b) = self.border_of(&row.common, ctx, sib) {
            self.draw_border(&b, rect)?;
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
        let (w, h) = self.size_at(k, (inner.w - c.x.unwrap_or(0.0)).max(1.0), ctx, depth + 1, sib);
        let (x, y) = c.top_left(w, h);
        let rect = Rect::new(inner.x + x, inner.y + y, w, h);
        let start = self.item_count();
        self.draw_node(k, rect, ctx, depth + 1, sib)?;
        if c.rotate != 0 && matches!(k, Node::Draw(_)) {
            let stands = k.standing_box(w, h);
            let extent = (inner.y + stands.y, inner.y + stands.bottom());
            self.turn(start, (inner.x + c.x.unwrap_or(0.0), inner.y + c.y.unwrap_or(0.0)), c.rotate, extent);
        }
        Ok(())
    }

    /// Turn the items the last page got since `start`, a draw, about `about`.
    fn turn(&mut self, start: usize, about: (f64, f64), quarters: u8, extent: (f64, f64)) {
        let Some(page) = self.pages.last_mut().filter(|p| start < p.items.len()) else { return };
        let items = page.items.split_off(start);
        page.items.push(Item::Turned(Box::new(Turned { about, quarters, extent, items })));
        // One item now, kept whole by what it is: the record of its items, the last one kept,
        // would fall on the next.
        if self.leaves.last().is_some_and(|leaf| leaf.0 == start) {
            self.leaves.pop();
        }
    }

    /// The property changes scripts made to the instance of object `c` at index `sib` under
    /// `ctx` (`default` names an unnamed field or group, as the live form does), if any.
    pub(super) fn changes_of(&self, c: &Common, ctx: &Ctx, sib: usize, default: Option<&str>) -> Option<&BTreeMap<String, String>> {
        if self.props.is_empty() {
            return None;
        }
        let name = c.name.as_deref().or(default)?;
        let som = if ctx.som.is_empty() { format!("{name}[{sib}]") } else { format!("{}.{name}[{sib}]", ctx.som) };
        self.props.get(&som)
    }

    /// The border of container `c` (instance `sib` under `ctx`) as scripts changed it.
    pub(super) fn border_of(&self, c: &Common, ctx: &Ctx, sib: usize) -> Option<Border> {
        match self.changes_of(c, ctx, sib, None) {
            Some(sets) => crate::restyle::restyled_border(c, sets),
            None => c.border.clone(),
        }
    }

    /// Draw `node` at `rect`, shown where it and what it is inside say ([`Common::views`]): an
    /// invisible object that `relevant` prints is drawn for print only, and one shown nowhere
    /// draws nothing.
    pub(super) fn draw_node(&mut self, node: &Node, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        let c = node.common();
        self.warn_other_views(c);
        let views = self.views.and(c.views());
        if !views.any() {
            return Ok(());
        }
        let outer = std::mem::replace(&mut self.views, views);
        let drawn = self.draw_node_in(node, rect, ctx, depth, sib);
        self.views = outer;
        drawn
    }

    /// [`Self::draw_node`] once where the node shows is set.
    pub(super) fn draw_node_in(&mut self, node: &Node, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        match node {
            Node::Draw(d) => {
                let start = self.item_count();
                match self.changes_of(&d.common, ctx, sib, None) {
                    Some(sets) => {
                        let changed = crate::restyle::restyled_draw(d, sets);
                        self.draw_draw(&changed, rect, ctx)?;
                    }
                    None => self.draw_draw(d, rect, ctx)?,
                }
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
            Node::ExclGroup(g) => match self.changes_of(&g.common, ctx, sib, Some("group")) {
                Some(sets) => {
                    let changed = crate::restyle::restyled_group(g, sets);
                    self.draw_group(&changed, rect, ctx, depth, sib)
                }
                None => self.draw_group(g, rect, ctx, depth, sib),
            },
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
                if let Some(b) = self.border_of(&s.common, ctx, sib) {
                    self.draw_border(&b, rect)?;
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
                            // A repeating subform here (a table in an area of a positioned page,
                            // say) gets its instances, as in a flowed one, one under another,
                            // each as tall as its own content, and the rows past them are
                            // reported as there.
                            let instances = self.laid_out_instances(k, &ctx.som);
                            for instance in 0..instances {
                                let sib = sibs.at(i).saturating_add(instance);
                                let (w, h) = self.size_at(k, inner.w, &ctx, depth + 1, sib);
                                self.draw_node(k, Rect::new(inner.x, y, w, h), &ctx, depth + 1, sib)?;
                                y += h;
                            }
                        }
                    }
                    Layout::LrTb | Layout::RlTb => {
                        // A repeating subform here (rows of fields under a heading) gets its
                        // instances, as in the other layouts, and the rows past them are reported.
                        for k in s.children.iter().filter(|k| k.common().presence.occupies()) {
                            self.laid_out_instances(k, &ctx.som);
                        }
                        let (placed, _) = self.pack_instances(&s.children, inner.w, &ctx, depth, &ctx.som);
                        // The sibling indices of each `subformSet` the steps go into, found once per
                        // set (known by where its children are); the subform's own are `sibs`.
                        let mut set_sibs: std::collections::HashMap<usize, SomIndices> = std::collections::HashMap::new();
                        for (at, r) in placed {
                            // Down the steps: a member of a `subformSet` takes the set's SOM path.
                            let (mut nodes, mut node_ctx, mut node_depth) = (s.children.as_slice(), ctx.clone(), depth + 1);
                            let mut target = None;
                            for (step, &(i, instance)) in at.iter().enumerate() {
                                let Some(k) = nodes.get(i) else { break };
                                let index = if step == 0 {
                                    sibs.at(i)
                                } else {
                                    set_sibs.entry(nodes.as_ptr() as usize).or_insert_with(|| SomIndices::of(nodes)).at(i)
                                };
                                let sib = index + instance;
                                match k {
                                    Node::Subform(set) if set.set && step + 1 < at.len() => {
                                        node_ctx = node_ctx.with_som(&set.common, sib);
                                        nodes = &set.children;
                                        node_depth += 1;
                                    }
                                    _ => target = Some((k, sib)),
                                }
                            }
                            if let Some((k, sib)) = target {
                                let x = if s.layout == Layout::RlTb { inner.right() - r.x - r.w } else { inner.x + r.x };
                                self.draw_node(k, Rect::new(x, inner.y + r.y, r.w, r.h), &node_ctx, node_depth, sib)?;
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
                                // A repeating row (a table in a positioned subform, say) gets its
                                // instances one under another, as in a flowed table (XFA 3.3,
                                // "Tables", p. 327), and the rows past them are reported.
                                for instance in 0..self.laid_out_instances(k, &ctx.som) {
                                    let h = self.row_height(r, &cols, inner.w, &ctx, depth + 1, csib.saturating_add(instance));
                                    self.draw_row(r, &cols, Rect::new(inner.x, y, inner.w, h), &ctx, depth + 1, csib + instance)?;
                                    y += h;
                                }
                            } else {
                                let (w, h) = self.size_at(k, inner.w, &ctx, depth + 1, csib);
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
            if b.fill().is_none() || b.fill_kind == FillKind::Solid {
                return self.push(Item::Rounded(Box::new(shape)));
            }
            // The fill, what it paints over its colour, then the edge on top.
            self.push(Item::Rounded(Box::new(Rounded { edge: None, ..shape.clone() })))?;
            self.fill_overlay(b, rect)?;
            return self.push(Item::Rounded(Box::new(Rounded { fill: None, ..shape })));
        }
        if let Some(fill) = b.fill() {
            self.push(Item::Fill { rect, color: fill.0 })?;
            self.fill_overlay(b, rect)?;
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

    /// What border `b`'s fill paints over its colour inside `rect` (see [`FillKind`]): a
    /// hatching pattern's strokes, a gradient as bands from the fill colour to the other, a
    /// stipple as the blend of the two at its rate. On a rounded box the overlay covers the box,
    /// not its curves.
    pub(super) fn fill_overlay(&mut self, b: &Border, rect: Rect) -> Result<(), XfaError> {
        /// Strokes of a hatching pattern, this far apart (the spec leaves the interval to the
        /// application, XFA 3.3 p. 806), this wide.
        const PITCH: f64 = 4.0;
        const STROKE: f64 = 0.5;
        /// Bands a gradient is drawn in.
        const BANDS: usize = 64;
        let Some(base) = b.fill() else { return Ok(()) };
        if !(rect.w > 0.0 && rect.h > 0.0 && rect.w.is_finite() && rect.h.is_finite()) {
            return Ok(());
        }
        let mix = |a: [f64; 3], z: [f64; 3], t: f64| -> [f64; 3] { std::array::from_fn(|i| a[i] + (z[i] - a[i]) * t) };
        let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
        match b.fill_kind {
            FillKind::Solid => {}
            FillKind::Stipple { rate, color } => self.push(Item::Fill { rect, color: mix(base.0, color.0, rate) })?,
            FillKind::Pattern { kind, color } => {
                let line = |from: (f64, f64), to: (f64, f64)| Item::Line { from, to, width: STROKE, color: color.0, dashed: false };
                let (horizontal, vertical, down, up) = match kind {
                    Hatch::CrossHatch => (true, true, false, false),
                    Hatch::Horizontal => (true, false, false, false),
                    Hatch::Vertical => (false, true, false, false),
                    Hatch::CrossDiagonal => (false, false, true, true),
                    Hatch::DiagonalLeft => (false, false, true, false),
                    Hatch::DiagonalRight => (false, false, false, true),
                };
                // Strokes at PITCH from each other: at most a few thousand on a page-sized box.
                let steps = |len: f64| ((len / PITCH).floor() as usize).min(4_000);
                if horizontal {
                    for k in 1..=steps(rect.h) {
                        let y = y0 + k as f64 * PITCH;
                        if y < y1 {
                            self.push(line((x0, y), (x1, y)))?;
                        }
                    }
                }
                if vertical {
                    for k in 1..=steps(rect.w) {
                        let x = x0 + k as f64 * PITCH;
                        if x < x1 {
                            self.push(line((x, y0), (x, y1)))?;
                        }
                    }
                }
                // Diagonals: lines x - y = c (top-left to bottom-right, "diagonalLeft") and
                // x + y = c (bottom-left to top-right), clipped to the box; PITCH apart across.
                let span = rect.w + rect.h;
                let n = steps(span * std::f64::consts::FRAC_1_SQRT_2);
                let gap = PITCH * std::f64::consts::SQRT_2;
                for k in 1..=n {
                    let t = k as f64 * gap;
                    if down {
                        // From the left or top side to the bottom or right side.
                        let start = if t < rect.h { (x0, y1 - t) } else { (x0 + (t - rect.h), y0) };
                        let len = (x1 - start.0).min(y1 - start.1);
                        if len > 0.0 {
                            self.push(line(start, (start.0 + len, start.1 + len)))?;
                        }
                    }
                    if up {
                        let start = if t < rect.h { (x0, y0 + t) } else { (x0 + (t - rect.h), y1) };
                        let len = (x1 - start.0).min(start.1 - y0);
                        if len > 0.0 {
                            self.push(line(start, (start.0 + len, start.1 - len)))?;
                        }
                    }
                }
            }
            FillKind::Linear { to, color } => {
                for k in 0..BANDS {
                    let (a, z) = (k as f64 / BANDS as f64, (k + 1) as f64 / BANDS as f64);
                    let t = (a + z) / 2.0;
                    // Each band reaches into the next, which is drawn over it, so no seam of
                    // the colour below shows where two smoothed edges meet.
                    let reach = ((k + 2).min(BANDS) as f64 / BANDS as f64) - a;
                    let band = match to {
                        Side::Right => Rect::new(x0 + rect.w * a, y0, rect.w * reach, rect.h),
                        Side::Left => Rect::new(x1 - rect.w * (a + reach), y0, rect.w * reach, rect.h),
                        Side::Bottom => Rect::new(x0, y0 + rect.h * a, rect.w, rect.h * reach),
                        Side::Top => Rect::new(x0, y1 - rect.h * (a + reach), rect.w, rect.h * reach),
                    };
                    self.push(Item::Fill { rect: band, color: mix(base.0, color.0, t) })?;
                }
            }
            FillKind::Radial { to_center, color } => {
                // Nested ovals from the edge in, each a band of the gradient (start colour at
                // the centre, `color` at the edge; the other way round for toCenter).
                let (centre, edge) = if to_center { (color.0, base.0) } else { (base.0, color.0) };
                for k in 0..BANDS {
                    let f = 1.0 - k as f64 / BANDS as f64;
                    let r = Rect::new(x0 + rect.w * (1.0 - f) / 2.0, y0 + rect.h * (1.0 - f) / 2.0, rect.w * f, rect.h * f);
                    let most = r.w.min(r.h) / 2.0;
                    let oval = Rounded {
                        rect: r,
                        radii: [most; 4],
                        inverted: [false; 4],
                        fill: Some(mix(centre, edge, f)),
                        edge: None,
                        open_top: false,
                        open_bottom: false,
                    };
                    self.push(Item::Rounded(Box::new(oval)))?;
                }
            }
        }
        Ok(())
    }

    /// Draw `v` in `rect`; `scope` is the SOM of what holds it, for the fields it embeds.
    pub(super) fn draw_value_text(&mut self, v: &Value, rect: Rect, font: &Font, para: &Para, scope: &str) -> Result<(), XfaError> {
        let rich = match v {
            Value::Text(t) => text::plain(t),
            Value::Rich(r) => r.clone(),
            _ => return Ok(()),
        };
        let face = Face::of(font);
        // Each pass over the text — a re-measure that shrinks it, or the final layout — counts
        // its characters against the draw budget, so large text drawn in many instances stops.
        let cost = rich.plain().chars().count().max(1);
        // The values of embedded fields are laid out with it, so each pass counts those too: a
        // short text embedding a long value is as much work as the value written out.
        let embed = |id: &str, max: usize| {
            let v = self.embed_in(id, max, scope);
            self.drawn_chars.set(self.drawn_chars.get().saturating_add(v.chars().count()));
            v
        };
        // Text that doesn't fit the height it was given (the template's face is narrower than
        // the standard one drawn in its place, so it wraps to more lines) is drawn smaller,
        // down to 70%, rather than over what lies below.
        // A pass is not started once the budget is spent, and one that spends it stops the rest.
        let fits = |k: f64| -> Result<bool, XfaError> {
            self.check_drawn_chars()?;
            self.drawn_chars.set(self.drawn_chars.get().saturating_add(cost));
            let fit = text::measure_height(&rich, rect.w / k, para, &face, &embed) <= rect.h / k + 1.0;
            self.check_drawn_chars()?;
            Ok(fit)
        };
        let mut scale = 1.0;
        if rect.h >= 4.0 && !fits(1.0)? {
            scale = 0.7;
            for k in [0.95, 0.9, 0.85, 0.8, 0.75] {
                if fits(k)? {
                    scale = k;
                    break;
                }
            }
        }
        self.drawn_chars.set(self.drawn_chars.get().saturating_add(cost));
        self.check_drawn_chars()?;
        let mut block = text::layout(&rich, Rect::new(rect.x, rect.y, rect.w / scale, rect.h / scale), para, &face, &embed);
        // The layout itself expands the embedded values once more.
        self.check_drawn_chars()?;
        if scale < 1.0 {
            for s in &mut block.spans {
                s.x = rect.x + (s.x - rect.x) * scale;
                s.baseline = rect.y + (s.baseline - rect.y) * scale;
                s.face.size *= scale;
            }
        }
        self.push_block(block)
    }

    /// Draw a `draw` element (static text, an image, a line, a rectangle or an arc) in `rect`,
    /// inside its border and margin.
    fn draw_draw(&mut self, d: &Draw, rect: Rect, ctx: &Ctx) -> Result<(), XfaError> {
        if let Some(b) = &d.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.child(&d.common);
        let inner = rect.inset(&d.common.margin);
        match &d.value {
            Value::Text(_) | Value::Rich(_) => self.draw_value_text(&d.value, inner, &ctx.font, &ctx.para, &ctx.som),
            Value::Image { content_type, data, .. } => self.draw_image(data, content_type, inner),
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

    /// Draw exclusion group `g` at `rect`: its border, then each of its buttons where its layout
    /// puts it, as one radio group, the one its data selects turned on. A group that must be
    /// answered makes each button required. `sib` is the group's index among same-named
    /// siblings, for its SOM path.
    fn draw_group(&mut self, g: &ExclGroup, rect: Rect, ctx: &Ctx, depth: usize, sib: usize) -> Result<(), XfaError> {
        if let Some(b) = &g.common.border {
            self.draw_border(b, rect)?;
        }
        let ctx = ctx.with_som(&g.common, sib);
        // The group's data node holds the selected button's on value.
        let selected: Option<String> =
            self.data.and_then(|d| d.text_at(&data_path(self.tpl, &ctx.som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let inner = rect.inset(&g.common.margin);
        let group = self.unique_name(g.common.name.as_deref().unwrap_or("group"), g.common.name.is_none());
        let nodes: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
        // Each field is measured under the SOM index it is drawn at.
        let sibs = SomIndices::of(&nodes);
        let placed: Vec<(usize, Rect)> = match g.layout {
            Layout::Tb => {
                let mut y = 0.0;
                let mut out = Vec::new();
                for (i, k) in nodes.iter().enumerate() {
                    if !k.common().presence.occupies() {
                        continue;
                    }
                    let (w, h) = self.size_at(k, inner.w, &ctx, depth + 1, sibs.at(i));
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
                    let (w, h) = self.size_at(k, (inner.w - c.x.unwrap_or(0.0)).max(1.0), &ctx, depth + 1, sibs.at(i));
                    let (x, y) = c.top_left(w, h);
                    (i, Rect::new(x, y, w, h))
                })
                .collect(),
            _ => self.pack(&nodes, inner.w, &ctx, depth).0,
        };
        for (i, r) in placed {
            if let Some(f) = g.fields.get(i) {
                // Each button shows where it and the group say ([`Common::views`]), as any field
                // drawn through [`Self::draw_node`] does.
                self.warn_other_views(&f.common);
                let views = self.views.and(f.common.views());
                if !views.any() {
                    continue;
                }
                // The button's on value (its place in the group when it lists none) and tooltip
                // are read from it as its instance's changes leave it ([`Self::draw_field`]); the
                // group's tooltip stands in for a button without one.
                let fallback_on = (i + 1).to_string();
                // A group that must be answered makes each of its buttons required, as the group
                // field they make is.
                let required;
                let f = if g.required && !f.required {
                    required = Field { required: true, ..f.clone() };
                    &required
                } else {
                    f
                };
                let outer = std::mem::replace(&mut self.views, views);
                let radio = Some((group.as_str(), fallback_on.as_str(), g.tooltip.clone(), selected.as_deref()));
                let drawn = self.draw_field(f, Rect::new(inner.x + r.x, inner.y + r.y, r.w, r.h), &ctx, radio, sibs.at(i));
                self.views = outer;
                drawn?;
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

    /// A field name made from `base` that no other field of the form has: the first keeps it,
    /// later ones get `_2`, `_3`, …, skipping a name another field already has or that the
    /// template gives another field (a template field named `Cell4_2` keeps it, and the second
    /// `Cell4` becomes `Cell4_3`). A `fallback` base, the name an unnamed field or group gets
    /// (`field`, `group`), is made up too: it doesn't take a name the template gives another
    /// field either, and a template field named `field` still gets it.
    pub(super) fn unique_name(&mut self, base: &str, fallback: bool) -> String {
        let base = clean_name(base);
        // The template's own name, while no other field has it.
        if !fallback && self.taken.insert(base.clone()) {
            return base;
        }
        loop {
            let n = self.names.entry(base.clone()).or_insert(0);
            *n = n.saturating_add(1);
            let candidate = if *n == 1 { base.clone() } else { format!("{base}_{n}") };
            let reserved = (fallback || *n > 1) && self.template_names.contains(&candidate);
            if !reserved && self.taken.insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}
