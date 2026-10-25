//! Filled rectangular panel boundaries used by editable text grouping.

use pdfcraft_content::{Matrix, Op};
use pdfcraft_cos::{Dict, Document, ObjRef};

struct Container {
    inverse: Matrix,
    rect: [f64; 4],
    /// Where it is painted in the page's drawing order (see [`Containers::membership`]).
    at: Vec<usize>,
    /// Its bounding box on the page, whatever matrix drew it.
    page: [f64; 4],
    /// The first container with the same box on the page: a panel painted again behind each of
    /// its rows, under any matrix, is still one panel.
    id: usize,
}

#[derive(Default)]
pub(super) struct Containers {
    filled: Vec<Container>,
    pending: Option<Container>,
    complex: bool,
}

impl Containers {
    pub(super) fn collect(doc: &Document, ops: &[Op], resources: &Dict) -> Self {
        let mut collector =
            Collector { doc, containers: Self::default(), path: Vec::new(), budget: super::FormBudget::new(pdfcraft_cos::DecodeBudget::page()) };
        collector.walk(ops, resources, Matrix::IDENTITY, &[]);
        let mut first = std::collections::HashMap::new();
        for (index, container) in collector.containers.filled.iter_mut().enumerate() {
            let key = container.page.map(|v| (v * 1000.0).round() as i64);
            container.id = *first.entry(key).or_insert(index);
        }
        collector.containers
    }

    fn observe(&mut self, op: &Op, ctm: Matrix, at: &[usize]) {
        match op.op.as_slice() {
            b"re" => {
                if self.pending.is_some() {
                    self.complex = true;
                }
                self.pending = op.nums::<4>().zip(ctm.invert()).and_then(|([x, y, width, height], inverse)| {
                    let rect = [x.min(x + width), y.min(y + height), x.max(x + width), y.max(y + height)];
                    (rect.iter().all(|v| v.is_finite()) && rect[2] > rect[0] && rect[3] > rect[1]).then_some(Container {
                        inverse,
                        rect,
                        at: Vec::new(),
                        page: page_box(rect, ctm),
                        id: 0,
                    })
                });
            }
            b"m" | b"l" | b"c" | b"v" | b"y" => self.complex = true,
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if !self.complex
                    && self.filled.len() < 4096
                    && let Some(mut rect) = self.pending.take()
                {
                    rect.at = at.to_vec();
                    self.filled.push(rect);
                }
                self.pending = None;
                self.complex = false;
            }
            b"n" | b"S" | b"s" => {
                self.pending = None;
                self.complex = false;
            }
            _ => {}
        }
    }

    /// The panels that hold `rect`'s centre and are taller than it. A highlight or inline shading
    /// drawn to the height of its glyphs is not a panel, and neither is a fill painted after the
    /// text (a redaction cover): `at` is the text's place in the drawing order, the operator
    /// indices from the page down through the forms that draw it.
    pub(super) fn membership(&self, rect: [f64; 4], at: &[usize]) -> Vec<usize> {
        let (x, y) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
        let mut ids: Vec<usize> = self
            .filled
            .iter()
            .filter_map(|container| {
                let ys =
                    [(rect[0], rect[1]), (rect[2], rect[1]), (rect[0], rect[3]), (rect[2], rect[3])].map(|(x, y)| container.inverse.apply(x, y).1);
                let height = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max) - ys.iter().copied().fold(f64::INFINITY, f64::min);
                let (x, y) = container.inverse.apply(x, y);
                let [left, bottom, right, top] = container.rect;
                (container.at.as_slice() < at && x >= left && x <= right && y >= bottom && y <= top && top - bottom >= height * 1.15)
                    .then_some(container.id)
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

fn page_box([left, bottom, right, top]: [f64; 4], ctm: Matrix) -> [f64; 4] {
    let corners = [(left, bottom), (right, bottom), (left, top), (right, top)].map(|(x, y)| ctm.apply(x, y));
    corners.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |[x0, y0, x1, y1], &(x, y)| {
        [x0.min(x), y0.min(y), x1.max(x), y1.max(y)]
    })
}

impl Containers {
    /// Whether panel `id` (from [`Containers::membership`]) is no more than a padded highlight
    /// around `rect`: under 1.75 times its height on the page.
    pub(super) fn hugs(&self, id: usize, rect: [f64; 4]) -> bool {
        self.filled.get(id).is_some_and(|panel| panel.page[3] - panel.page[1] < (rect[3] - rect[1]) * 1.75)
    }

    /// Whether panel `id` reaches across `rect` from its left edge to its right one on the page.
    pub(super) fn spans(&self, id: usize, rect: [f64; 4]) -> bool {
        let tolerance = (rect[3] - rect[1]) * 0.25;
        self.filled.get(id).is_some_and(|panel| panel.page[0] <= rect[0] + tolerance && panel.page[2] >= rect[2] - tolerance)
    }
}

struct Collector<'a> {
    doc: &'a Document,
    containers: Containers,
    path: Vec<ObjRef>,
    budget: super::FormBudget,
}

impl Collector<'_> {
    fn walk(&mut self, ops: &[Op], resources: &Dict, initial: Matrix, prefix: &[usize]) {
        let mut local = Containers::default();
        let mut ctm = initial;
        let mut stack = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            let at = [prefix, &[i]].concat();
            local.observe(op, ctm, &at);
            match op.op.as_slice() {
                b"q" => stack.push(ctm),
                b"Q" => {
                    if let Some(saved) = stack.pop() {
                        ctm = saved;
                    }
                }
                b"cm" => {
                    if let Some(matrix) = op.nums::<6>() {
                        ctm = Matrix(matrix).then(&ctm);
                    }
                }
                b"Do" => {
                    if let Some(name) = op.name(0)
                        && let Some(form) = super::form_call(self.doc, resources, name, ctm, &self.path, &mut self.budget)
                    {
                        if let Some(obj) = form.obj {
                            self.path.push(obj);
                        }
                        self.walk(&pdfcraft_content::parse(&form.data).ops, &form.resources, form.ctm, &at);
                        if form.obj.is_some() {
                            self.path.pop();
                        }
                    }
                }
                _ => {}
            }
        }
        let remaining = 4096_usize.saturating_sub(self.containers.filled.len());
        self.containers.filled.extend(local.filled.into_iter().take(remaining));
    }
}
