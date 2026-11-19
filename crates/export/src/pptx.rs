//! PowerPoint presentation (.pptx, Office Open XML PresentationML): slides per page,
//! largest text as slide title, remaining paragraphs as body text, images and tables between them.

use std::collections::HashMap;
use std::sync::Arc;

use crate::as_docx::{image_mime, room_for};
use crate::{Image, Page, Run, Table, hex};
use craft_pptx::table::{Cell, Table as DmlTable};
use craft_pptx::text::{Paragraph, Run as DmlRun, RunKind, RunProps, TextBody};
use craft_pptx::{
    ColorBase, ColorRef, Fill, Geom, Line, MediaId, MediaItem, Plain, Presentation, Rgba, Shape, ShapeId, ShapeKind, Size, Slide, SlideId, Xfrm,
    style::PictureFill,
};

/// Items on a slide in reading order.
enum SlideBodyItem<'a> {
    /// A paragraph's runs (at least one).
    Para(Vec<Run<'a>>),
    Table(Table),
    Img(&'a Image),
}

/// Clamp slide dimensions in points to PowerPoint's allowed range: 1 inch (72 pt) to 56 inches (4032 pt).
fn slide_pt(pt: f64, default_pt: f64) -> f64 {
    if pt.is_finite() && pt > 0.0 { pt.clamp(72.0, 4032.0) } else { default_pt }
}

/// The largest height one item may take on a slide (about 1,100 inches in points).
const MAX_ITEM_PT: f64 = 78_740.0;

/// Points for a height or gap, clamped to `0..=MAX_ITEM_PT` (NaN reads as 0).
fn height_pt(pt: f64) -> f64 {
    if pt.is_finite() {
        pt.clamp(0.0, MAX_ITEM_PT)
    } else if pt > 0.0 {
        MAX_ITEM_PT
    } else {
        0.0
    }
}

/// Parse a 6-digit hex color string into `Rgba`.
fn rgba_from_hex(h: &str) -> Option<Rgba> {
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(Rgba::rgb(r, g, b))
}

/// A formatted DrawingML text run.
fn dml_run(text: &str, size: f64, bold: bool, italic: bool, color: [f64; 3]) -> DmlRun {
    let sz = if size.is_finite() { size.clamp(1.0, 4000.0) } else { 12.0 };
    let fill =
        hex(color).and_then(|h| rgba_from_hex(&h)).map(|c| Fill::Solid { color: ColorRef { base: ColorBase::Rgb { rgb: c }, mods: Vec::new() } });
    DmlRun {
        text: text.to_string(),
        props: RunProps {
            size: Some(sz),
            bold: if bold { Some(true) } else { None },
            italic: if italic { Some(true) } else { None },
            fill,
            ..Default::default()
        },
        kind: RunKind::Text,
    }
}

/// Generates a DrawingML table shape with explicit cell borders.
fn pptx_table(shape_id: u32, t: &Table, x: f64, y: f64, content_w: f64) -> (Shape, f64) {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let widths_pt: Vec<f64> = edges.windows(2).map(|w| (w[1] - w[0]).max(20.0)).collect();
    let total_pt: f64 = widths_pt.iter().sum();
    let scale = if total_pt > 0.0 { (content_w / total_pt).min(1.0) } else { 1.0 };
    let widths: Vec<f64> = widths_pt.iter().map(|w| (w * scale).max(10.0)).collect();
    let table_w: f64 = widths.iter().sum();
    let row_h: f64 = 24.0;
    let table_h: f64 = (t.rows.len() as f64 * row_h).min(MAX_ITEM_PT);

    let border = Some(Line {
        width: Some(1.0),
        fill: Some(Fill::Solid { color: ColorRef { base: ColorBase::Rgb { rgb: Rgba::rgb(0x99, 0x99, 0x99) }, mods: Vec::new() } }),
        ..Default::default()
    });
    let borders = [border.clone(), border.clone(), border.clone(), border];

    let mut rows = Vec::with_capacity(t.rows.len());
    for row in &t.rows {
        let mut cells = Vec::new();
        for c in row {
            let span = c.span.max(1);
            let run = dml_run(&c.text, c.size, c.bold, c.italic, c.color);
            cells.push(Cell {
                text: TextBody { paragraphs: vec![Paragraph { runs: vec![run], ..Default::default() }], ..Default::default() },
                grid_span: span as u32,
                row_span: 1,
                borders: borders.clone(),
                ..Default::default()
            });
            for _ in 1..span {
                cells.push(Cell { h_merge: true, borders: borders.clone(), ..Default::default() });
            }
        }
        let used: usize = row.iter().map(|c| c.span).sum();
        for _ in used..t.cols.len() {
            cells.push(Cell { borders: borders.clone(), ..Default::default() });
        }
        rows.push(craft_pptx::table::Row { height: row_h, cells });
    }

    let shape = Shape {
        id: ShapeId(shape_id),
        name: format!("Table {shape_id}"),
        xfrm: Some(Xfrm { x, y, w: table_w, h: table_h, ..Default::default() }),
        kind: ShapeKind::Table(DmlTable { cols: widths, rows, ..Default::default() }),
        ..Default::default()
    };
    (shape, table_h)
}

/// A PowerPoint presentation (.pptx, Office Open XML PresentationML): each page becomes a slide,
/// with the largest text as the title, remaining paragraphs as body text, and images between them.
///
/// When `pages` is empty, produces a presentation with a single blank slide so the resulting
/// `.pptx` file is a valid OpenXML package that PowerPoint can open without repair warnings.
pub fn pptx(pages: &[Page], title: &str) -> Vec<u8> {
    let dummy = [Page::default()];
    let pages = if pages.is_empty() { &dummy[..] } else { pages };

    let (sw, sh) = pages.first().map_or((720.0, 540.0), |p| (slide_pt(p.width, 720.0), slide_pt(p.height, 540.0)));
    let mx = (sw / 16.0).clamp(360000.0 / 12700.0, 72.0);
    let my = (sh / 16.0).clamp(360000.0 / 12700.0, 72.0);
    let content_w = sw - 2.0 * mx;

    let mut global_media: Vec<(String, &Image)> = Vec::new();
    // Shared picture bytes remain alive throughout export; only admitted media enter this map.
    let mut media_of: HashMap<*const u8, usize> = HashMap::new();
    let mut slides: Vec<Arc<Slide>> = Vec::new();
    let body = crate::layout::body_size(pages);

    for (slide_idx, p) in pages.iter().enumerate() {
        let (tables, consumed) = crate::tables(&p.blocks, &p.rules);
        let title_idx = p
            .blocks
            .iter()
            .enumerate()
            .filter(|(i, b)| !consumed.get(*i).copied().unwrap_or(false) && !b.text.trim().is_empty())
            .max_by(|(_, a), (_, b)| a.size.total_cmp(&b.size))
            .map(|(i, _)| i);

        let mut parts: Vec<([f64; 4], f64, SlideBodyItem)> = Vec::new();
        // The title is a shape of its own, so it joins no paragraph.
        let skip: Vec<bool> = (0..p.blocks.len()).map(|bi| Some(bi) == title_idx || consumed.get(bi).copied().unwrap_or(false)).collect();
        for runs in crate::flowing_paragraphs(&p.blocks, &skip, body) {
            let Some(first) = runs.first().map(|r| r.block) else { continue };
            let x1 = runs.iter().map(|r| r.block.rect[2]).fold(first.rect[2], f64::max);
            parts.push(([first.rect[0], first.rect[1], x1, first.rect[3]], first.size, SlideBodyItem::Para(runs)));
        }
        for t in tables {
            parts.push((t.rect, 11.0, SlideBodyItem::Table(t)));
        }
        for im in &p.images {
            parts.push((im.rect, (im.rect[3] - im.rect[1]).max(11.0), SlideBodyItem::Img(im)));
        }
        let items = crate::reading_order(p, parts);

        let mut current_y = my;
        let mut shapes: Vec<Shape> = Vec::new();
        let mut shape_id = 2u32;

        if let Some(title_block) = title_idx.and_then(|ti| p.blocks.get(ti)) {
            let title_h = height_pt((title_block.size * 1.5).max(36.0));
            let title_run = dml_run(&title_block.text, title_block.size, title_block.bold, title_block.italic, title_block.color);
            shapes.push(Shape {
                id: ShapeId(shape_id),
                name: format!("Title {shape_id}"),
                xfrm: Some(Xfrm { x: mx, y: current_y, w: content_w, h: title_h, ..Default::default() }),
                kind: ShapeKind::Shape,
                geom: Geom::Preset { name: "rect".into(), adj: Vec::new() },
                fill: Some(Fill::None),
                text_box: true,
                text: Some(TextBody { paragraphs: vec![Paragraph { runs: vec![title_run], ..Default::default() }], ..Default::default() }),
                ..Default::default()
            });
            shape_id += 1;
            current_y = (current_y + title_h + 18.0).min(MAX_ITEM_PT);
        }

        let mut i = 0;
        while let Some(item) = items.get(i) {
            match item {
                SlideBodyItem::Para(_) => {
                    let mut paragraphs = Vec::new();
                    let mut est_h_pt = 0.0;
                    while let Some(SlideBodyItem::Para(runs)) = items.get(i) {
                        let dml_runs = runs.iter().map(|r| dml_run(&r.text, r.block.size, r.block.bold, r.block.italic, r.block.color)).collect();
                        paragraphs.push(Paragraph { runs: dml_runs, ..Default::default() });
                        let len = runs.iter().map(|r| r.text.len()).fold(0usize, usize::saturating_add);
                        let size = runs.iter().map(|r| r.block.size).fold(10.0, f64::max);
                        let lines = (len as f64 / 60.0).ceil().max(1.0);
                        est_h_pt += (size * 1.3) * lines + 6.0;
                        i += 1;
                    }
                    let box_h = height_pt(est_h_pt);
                    shapes.push(Shape {
                        id: ShapeId(shape_id),
                        name: format!("TextBox {shape_id}"),
                        xfrm: Some(Xfrm { x: mx, y: current_y, w: content_w, h: box_h, ..Default::default() }),
                        kind: ShapeKind::Shape,
                        geom: Geom::Preset { name: "rect".into(), adj: Vec::new() },
                        fill: Some(Fill::None),
                        text_box: true,
                        text: Some(TextBody { paragraphs, ..Default::default() }),
                        ..Default::default()
                    });
                    shape_id += 1;
                    current_y = (current_y + box_h + 12.0).min(MAX_ITEM_PT);
                }
                SlideBodyItem::Img(im) => {
                    let global_img_idx = match media_of.get(&im.bytes.as_ptr()) {
                        Some(&idx) => idx,
                        None if room_for(&global_media, im) => {
                            let n = global_media.len() + 1;
                            global_media.push((format!("image{n}.{}", im.ext), *im));
                            media_of.insert(im.bytes.as_ptr(), n);
                            n
                        }
                        None => {
                            i += 1;
                            continue;
                        }
                    };

                    let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
                    let k = (content_w / w).min(1.0);
                    let cx = height_pt(w * k).min(content_w);
                    let cy = height_pt(h * k);
                    let x = mx + (content_w - cx) / 2.0;
                    let y = current_y;

                    shapes.push(Shape {
                        id: ShapeId(shape_id),
                        name: format!("Picture {shape_id}"),
                        xfrm: Some(Xfrm { x, y, w: cx, h: cy, ..Default::default() }),
                        kind: ShapeKind::Picture { fill: PictureFill { media: MediaId(global_img_idx as u32), ..Default::default() } },
                        geom: Geom::Preset { name: "rect".into(), adj: Vec::new() },
                        ..Default::default()
                    });
                    shape_id += 1;
                    current_y = (current_y + cy + 12.0).min(MAX_ITEM_PT);
                    i += 1;
                }
                SlideBodyItem::Table(t) => {
                    let (table_shape, table_h) = pptx_table(shape_id, t, mx, current_y, content_w);
                    shapes.push(table_shape);
                    shape_id += 1;
                    current_y = (current_y + table_h + 12.0).min(MAX_ITEM_PT);
                    i += 1;
                }
            }
        }

        slides.push(Arc::new(Slide { id: SlideId(256 + slide_idx as u32), shapes, ..Default::default() }));
    }

    let media: Vec<MediaItem> = global_media
        .into_iter()
        .enumerate()
        .map(|(idx, (name, im))| MediaItem {
            id: MediaId((idx + 1) as u32),
            name,
            content_type: image_mime(im.ext).to_string(),
            data: Arc::new(im.bytes.to_vec()),
            link: None,
        })
        .collect();

    let mut pres = Presentation { slide_size: Size::new(sw, sh), slides, media, ..Default::default() };
    pres.props.title = title.to_string();

    craft_pptx::export(&pres, &Plain).unwrap_or_default()
}

#[cfg(test)]
mod tests;
