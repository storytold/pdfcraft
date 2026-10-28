use super::common::clamp_opt;
use super::field::auto_check_reserve;
use super::{
    Access, Area, Ctx, ExclGroup, Face, Field, Font, Layout, Layouter, MAX_DEPTH, MAX_MEASURES, Measured, Node, Para, Placement, Rect, Subform, Ui,
    Value, data_path, text,
};

impl Layouter<'_> {
    /// The width and height `node` takes in `avail_w`: its own size, else what its content needs
    /// (containers are measured once per width and depth, and measuring stops at its caps).
    pub(super) fn size_of(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> (f64, f64) {
        let c = node.common();
        let spent = self.measures.get().saturating_add(1);
        self.measures.set(spent);
        if depth > MAX_DEPTH || spent > MAX_MEASURES {
            return (c.w.unwrap_or(avail_w), c.h.unwrap_or(0.0));
        }
        // Only containers recurse; their addresses are stable for the whole layout (the
        // template is borrowed, the root and page areas are held by the layouter).
        let key = match node {
            Node::Subform(s) => Some((&**s as *const Subform as usize, avail_w.to_bits(), depth)),
            Node::Area(a) => Some((&**a as *const Area as usize, avail_w.to_bits(), depth)),
            _ => None,
        };
        if let Some(key) = key
            && let Ok(cache) = self.measured.try_borrow()
            && let Some(m) = cache.get(&key)
            && m.font == ctx.font
            && m.para == ctx.para
        {
            return m.size;
        }
        let size = self.measure(node, avail_w, ctx, depth);
        if let Some(key) = key
            && let Ok(mut cache) = self.measured.try_borrow_mut()
        {
            cache.insert(key, Measured { font: ctx.font.clone(), para: ctx.para.clone(), size });
        }
        size
    }

    /// The size (width, height) of `node` within `avail_w`, uncached ([`Self::size_of`] caches it).
    /// Without a width, a container, a text draw or a button is as wide as its content (at most
    /// `avail_w`) and anything else takes `avail_w`; without a height, it is as tall as its content.
    fn measure(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> (f64, f64) {
        let c = node.common();
        // The node's own SOM: its repeating children count their instances under it, and a
        // growing text field finds its value at it. Measurements are cached per template node,
        // so instance 0 stands for all of them.
        let ctx = ctx.with_som(c, 0);
        let som = ctx.som.clone();
        // A container without a width is as wide as its content (a radio group beside its
        // question, say), and so is a text label or a button (as wide as its caption), within
        // what is available; any other field without one takes what is available.
        let w = match c.w {
            Some(w) => w,
            None => match node {
                Node::Subform(_) | Node::ExclGroup(_) | Node::Area(_) => {
                    let inner = (avail_w - c.margin.horizontal()).max(1.0);
                    (self.content_width(node, inner, &ctx, depth) + c.margin.horizontal()).min(avail_w)
                }
                Node::Draw(d) => self.natural_width(&d.value, &ctx.font, &ctx.para).map_or(avail_w, |t| (t + c.margin.horizontal()).min(avail_w)),
                Node::Field(f) if f.ui == Ui::Button => f
                    .caption
                    .as_ref()
                    .and_then(|cap| self.natural_width(&cap.value, &ctx.font.apply(&cap.font), &ctx.para.apply(&cap.para)))
                    .map_or(avail_w, |t| (t + c.margin.horizontal() + f.ui_margin.horizontal()).min(avail_w)),
                Node::Field(_) => avail_w,
            },
        };
        let w = clamp_opt(w, c.min_w, c.max_w);
        let h = match c.h {
            Some(h) => h,
            None => {
                let inner_w = (w - c.margin.horizontal()).max(1.0);
                let content = match node {
                    Node::Draw(d) => self.value_height(&d.value, inner_w, &ctx),
                    Node::Field(f) => self.field_height(f, inner_w, &ctx),
                    Node::ExclGroup(g) => self.group_height(g, inner_w, &ctx, depth),
                    Node::Area(a) => self.positioned_height(&a.children, inner_w, &ctx, depth),
                    Node::Subform(s) => match s.layout {
                        Layout::Positioned => self.positioned_height(&s.children, inner_w, &ctx, depth),
                        // Each instance of a repeating child takes its height again.
                        Layout::Tb => s
                            .children
                            .iter()
                            .filter(|k| k.common().presence.occupies())
                            .map(|k| self.size_of(k, inner_w, &ctx, depth + 1).1 * self.instances(k, &som) as f64)
                            .sum(),
                        Layout::LrTb | Layout::RlTb => self.pack_instances(&s.children, inner_w, &ctx, depth, &som).1,
                        Layout::Table => {
                            let cols = self.columns(s, inner_w);
                            s.children
                                .iter()
                                .filter(|k| k.common().presence.occupies())
                                .map(|k| match k {
                                    Node::Subform(r) if r.layout == Layout::Row => self.row_height(r, &cols, inner_w, &ctx, depth + 1),
                                    _ => self.size_of(k, inner_w, &ctx, depth + 1).1,
                                })
                                .sum()
                        }
                        Layout::Row => self.row_height(s, &self.columns(s, inner_w), inner_w, &ctx, depth + 1),
                    },
                };
                content + c.margin.vertical()
            }
        };
        // A flowed subform is as tall as what it holds: other viewers do not stretch one to
        // the `minH` a designer left on it.
        let min_h = match node {
            Node::Subform(s) if c.h.is_none() && s.layout != Layout::Positioned => None,
            _ => c.min_h,
        };
        (w, clamp_opt(h, min_h, c.max_h))
    }

    /// The natural width of a container's content within `avail_w`.
    fn content_width(&self, node: &Node, avail_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        let group: Vec<Node>;
        let (children, layout): (&[Node], Layout) = match node {
            Node::Subform(s) => {
                if s.layout == Layout::Table && !s.column_widths.is_empty() {
                    return s.column_widths.iter().sum::<f64>().min(avail_w);
                }
                (&s.children, s.layout)
            }
            Node::ExclGroup(g) => {
                // Fields are leaves: measuring these copies caches nothing.
                group = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
                (&group, g.layout)
            }
            Node::Area(a) => (&a.children, Layout::Positioned),
            _ => return avail_w,
        };
        let visible = children.iter().filter(|k| k.common().presence.occupies());
        match layout {
            Layout::LrTb | Layout::RlTb | Layout::Row => {
                let sum: f64 = visible.map(|k| self.size_of(k, avail_w, ctx, depth + 1).0).sum();
                if sum <= avail_w + 0.05 { sum } else { avail_w }
            }
            Layout::Positioned => visible
                .map(|k| {
                    let (w, h) = self.size_of(k, avail_w, ctx, depth + 1);
                    k.common().top_left(w, h).0 + w
                })
                .fold(0.0, f64::max)
                .min(avail_w),
            Layout::Tb | Layout::Table => visible.map(|k| self.size_of(k, avail_w, ctx, depth + 1).0).fold(0.0, f64::max).min(avail_w),
        }
    }

    /// How wide text `v` is unwrapped, in `font` and `para`, with a hair to spare so that laying
    /// it out at that width keeps it on its lines; `None` for what isn't text, or text that draws
    /// nothing.
    fn natural_width(&self, v: &Value, font: &Font, para: &Para) -> Option<f64> {
        let rich = match v {
            Value::Text(t) => text::plain(t),
            Value::Rich(r) => r.clone(),
            _ => return None,
        };
        // No text (a bordered spacer or rule): nothing to size to, so it keeps what it has room for.
        let w = text::natural_width(&rich, para, &Face::of(font), &|id, max| self.embed(id, max));
        (w > 0.0).then_some(w + 0.01)
    }

    /// How tall value `v` is laid out `w` wide: text wrapped in the context's font and paragraph,
    /// an image at its aspect ratio, and nothing for lines, rectangles and empty values.
    fn value_height(&self, v: &Value, w: f64, ctx: &Ctx) -> f64 {
        match v {
            Value::Text(t) => text::measure_height(&text::plain(t), w, &ctx.para, &Face::of(&ctx.font), &|id, max| self.embed(id, max)),
            Value::Rich(r) => text::measure_height(r, w, &ctx.para, &Face::of(&ctx.font), &|id, max| self.embed(id, max)),
            Value::Image { data, .. } => crate::image::size(data).map_or(w, |(iw, ih)| if iw > 0 { w * ih as f64 / iw as f64 } else { w }),
            Value::Line { .. } | Value::Rectangle(_) | Value::Empty => 0.0,
        }
    }

    /// Where field `f`'s caption goes and how much room it takes from the field's box, or `None`
    /// when it has no caption that takes room (a button's is drawn inside its box).
    pub(super) fn caption_reserve(&self, f: &Field, inner_w: f64, ctx: &Ctx) -> Option<(Placement, f64)> {
        let cap = f.caption.as_ref()?;
        if !cap.presence.occupies() || f.ui == Ui::Button {
            return None;
        }
        if cap.value.is_empty() && cap.reserve.is_none() {
            return None;
        }
        // A check box has no text for an inline caption to run with: it is not shown.
        if f.ui == Ui::CheckButton && cap.placement == Placement::Inline {
            return None;
        }
        let placement = if cap.placement == Placement::Inline { Placement::Left } else { cap.placement };
        let font = ctx.font.apply(&cap.font);
        let para = ctx.para.apply(&cap.para);
        let reserve = cap.reserve.unwrap_or_else(|| match placement {
            Placement::Top | Placement::Bottom => {
                let rich = match &cap.value {
                    Value::Rich(r) => r.clone(),
                    other => text::plain(&other.plain().unwrap_or_default()),
                };
                text::measure_height(&rich, inner_w, &para, &Face::of(&font), &|id, max| self.embed(id, max))
            }
            _ => (Face::of(&font).width(&cap.value.plain().unwrap_or_default()) + 2.0).min(inner_w * 0.6),
        });
        Some((placement, reserve))
    }

    /// The height of field `f`'s content within `inner_w` (inside its margins): its widget (a check
    /// box, a button's caption, or one line of text or three when multi-line, more for a text field
    /// that grows) plus a caption above or below it, or the taller of the two for one beside it.
    fn field_height(&self, f: &Field, inner_w: f64, ctx: &Ctx) -> f64 {
        let font = ctx.font.apply(&f.common.font);
        let lines = if f.multiline { 3.0 } else { 1.0 };
        let ui = match f.ui {
            Ui::CheckButton => f.check_size.unwrap_or(10.0).max(text::line_height(font.size)),
            // A button shows its caption: as tall as the caption's text in its own font.
            Ui::Button if f.caption.as_ref().is_some_and(|c| !c.value.is_empty()) => {
                let cap = f.caption.as_ref().map_or(0.0, |c| {
                    let rich = match &c.value {
                        Value::Rich(r) => r.clone(),
                        other => text::plain(&other.plain().unwrap_or_default()),
                    };
                    let w = (inner_w - f.ui_margin.horizontal()).max(1.0);
                    text::measure_height(&rich, w, &ctx.para.apply(&c.para), &Face::of(&ctx.font.apply(&c.font)), &|id, max| self.embed(id, max))
                });
                cap + f.ui_margin.vertical()
            }
            _ => (text::line_height(font.size) * lines).max(self.grown_text_height(f, inner_w, ctx)) + 4.0 + f.ui_margin.vertical(),
        };
        match self.caption_reserve(f, inner_w, ctx) {
            Some((Placement::Top | Placement::Bottom, r)) => ui + r,
            Some((placement, r)) => {
                // A check box's caption without a reserve is drawn in all the box leaves.
                let w = match auto_check_reserve(f, placement, inner_w, None) {
                    Some(auto) => auto,
                    None => inner_w - r,
                };
                let cap = f.caption.as_ref().map(|c| {
                    let rich = match &c.value {
                        Value::Rich(r) => r.clone(),
                        other => text::plain(&other.plain().unwrap_or_default()),
                    };
                    text::measure_height(&rich, w.max(1.0), &ctx.para.apply(&c.para), &Face::of(&ctx.font.apply(&c.font)), &|id, max| {
                        self.embed(id, max)
                    })
                });
                ui.max(cap.unwrap_or(0.0))
            }
            None => ui,
        }
    }

    /// The face a field's text is laid out in: at its horizontal scale when the layout draws
    /// the text itself (a non-interactive field), as designed when the forms layer draws it in
    /// a widget, whose appearances have no horizontal scaling.
    pub(super) fn field_text_face(f: &Field, font: &Font) -> Face {
        let face = Face::of(font);
        if f.access == Access::NonInteractive { face } else { face.unscaled() }
    }

    /// The height the text of a multi-line text field takes (its value in the data, else the
    /// template's), for a field without a fixed height, which grows to hold its text. `ctx`
    /// is the field's own (its SOM ends in the field). Zero for anything else.
    fn grown_text_height(&self, f: &Field, inner_w: f64, ctx: &Ctx) -> f64 {
        if !f.grows_with_text() || f.common.name.is_none() {
            return 0.0;
        }
        let value = self.data.and_then(|d| d.value_at(&data_path(self.tpl, &ctx.som))).filter(|t| !t.trim().is_empty()).or_else(|| f.value.plain());
        let Some(value) = value.filter(|t| !t.trim().is_empty()) else { return 0.0 };
        // Beside a caption the text has what the caption leaves; a widget's text has 2pt of
        // padding on each side, a non-interactive field's (drawn as text) none, so it wraps
        // where it is drawn.
        let beside = match self.caption_reserve(f, inner_w, ctx) {
            Some((Placement::Top | Placement::Bottom, _)) | None => 0.0,
            Some((_, r)) => r,
        };
        let padding = if f.access == Access::NonInteractive { 0.0 } else { 4.0 };
        let w = (inner_w - beside - f.ui_margin.horizontal() - padding).max(1.0);
        let font = ctx.font.apply(&f.common.font);
        text::measure_height(&text::plain(value.trim_end()), w, &ctx.para, &Self::field_text_face(f, &font), &|_, _| String::new())
    }

    /// The height of an exclusion group's fields at `inner_w`, by the group's layout: stacked
    /// top to bottom, at their own positions, or packed in rows.
    fn group_height(&self, g: &ExclGroup, inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        let nodes: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
        match g.layout {
            Layout::Tb => nodes.iter().filter(|k| k.common().presence.occupies()).map(|k| self.size_of(k, inner_w, ctx, depth + 1).1).sum(),
            Layout::Positioned => self.positioned_height(&nodes, inner_w, ctx, depth),
            _ => self.packed_height(&nodes, inner_w, ctx, depth),
        }
    }

    fn positioned_height(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        children
            .iter()
            .filter(|k| k.common().presence.occupies())
            .map(|k| {
                let c = k.common();
                let (w, h) = self.size_of(k, (inner_w - c.x.unwrap_or(0.0)).max(1.0), ctx, depth + 1);
                c.top_left(w, h).1 + h
            })
            .fold(0.0, f64::max)
    }

    /// Left-to-right packing: rows of children, wrapped at the available width, as (child,
    /// instance, place). A repeating subform is packed once per instance it has under the
    /// container at `som`.
    pub(super) fn pack_instances(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize, som: &str) -> (Vec<(usize, usize, Rect)>, f64) {
        let mut out = Vec::new();
        let (mut x, mut y, mut line_h) = (0.0, 0.0, 0.0);
        for (i, k) in children.iter().enumerate() {
            if !k.common().presence.occupies() {
                continue;
            }
            let (w, h) = self.size_of(k, inner_w, ctx, depth + 1);
            for instance in 0..self.instances(k, som) {
                if x > 0.0 && x + w > inner_w + 0.05 {
                    y += line_h;
                    x = 0.0;
                    line_h = 0.0;
                }
                out.push((i, instance, Rect::new(x, y, w, h)));
                x += w;
                line_h = line_h.max(h);
            }
        }
        (out, y + line_h)
    }

    /// [`Self::pack_instances`] for children that don't repeat (the fields of a group).
    pub(super) fn pack(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> (Vec<(usize, Rect)>, f64) {
        let (placed, h) = self.pack_instances(children, inner_w, ctx, depth, "");
        (placed.into_iter().map(|(i, _, r)| (i, r)).collect(), h)
    }

    fn packed_height(&self, children: &[Node], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        self.pack(children, inner_w, ctx, depth).1
    }

    /// Cell rectangles of a row (relative to the row's inner box) and the row height.
    pub(super) fn cells(&self, row: &Subform, cols: &[f64], inner_w: f64, ctx: &Ctx, depth: usize) -> (Vec<(usize, Rect)>, f64) {
        let ctx = ctx.child(&row.common);
        let mut out = Vec::new();
        let mut col = 0usize;
        let mut x = 0.0;
        let mut h: f64 = 0.0;
        for (i, k) in row.children.iter().enumerate() {
            if !k.common().presence.occupies() {
                continue;
            }
            let span = k.common().col_span.max(1);
            let w = if col < cols.len() { cols.iter().skip(col).take(span).sum::<f64>() } else { k.common().w.unwrap_or((inner_w - x).max(1.0)) };
            let (_, ch) = self.size_of(k, w, &ctx, depth + 1);
            out.push((i, Rect::new(x, 0.0, w, ch)));
            h = h.max(ch);
            x += w;
            col += span;
        }
        let h = match row.common.h {
            Some(rh) => rh,
            None => h + row.common.margin.vertical(),
        };
        (out, clamp_opt(h, row.common.min_h, row.common.max_h))
    }

    pub(super) fn row_height(&self, row: &Subform, cols: &[f64], inner_w: f64, ctx: &Ctx, depth: usize) -> f64 {
        self.cells(row, cols, inner_w, ctx, depth).1
    }
}
