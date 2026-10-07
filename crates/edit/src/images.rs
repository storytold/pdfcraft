//! Edit a PDF ▸ edit existing images: the images a page draws (image XObjects painted by `Do`
//! in its own content streams), and changing one: move/resize/rotate/flip (the `Do` is wrapped in
//! `q … cm … Q` with the extra transform), replace (it draws another XObject in the same place),
//! or delete. Only the stream that draws it is rewritten, as a new object.

use pdfcraft_content::{Matrix, Op, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::EditError;

/// An image drawn on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageImage {
    /// Its box in user space.
    pub rect: [f64; 4],
    /// The placement: the unit square → user space.
    pub matrix: [f64; 6],
    /// The XObject resource name and object.
    pub name: String,
    pub object: Option<ObjRef>,
    /// Pixel size.
    pub width: u32,
    pub height: u32,
    stream: usize,
    op: usize,
}

fn streams(doc: &Document, page: &Dict) -> Vec<(Object, Vec<u8>)> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    list.into_iter()
        .map(|o| {
            let data = match &*doc.resolve(&o) {
                Object::Stream(s) => s.decoded().unwrap_or_default(),
                _ => Vec::new(),
            };
            (o, data)
        })
        .collect()
}

fn page_of(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn xobjects(doc: &Document, page: &Dict) -> Dict {
    page.get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().and_then(|d| d.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())))
        .unwrap_or_default()
}

/// The images page `page` (0-based) draws, in drawing order.
pub fn page_images(doc: &Document, page: usize) -> Result<Vec<PageImage>, EditError> {
    let p = page_of(doc, page)?;
    let xo = xobjects(doc, &p.dict);
    let mut out = Vec::new();
    // The graphics state carries over from one stream to the next.
    let mut ctm = Matrix::IDENTITY;
    let mut stack = Vec::new();
    for (si, (_, data)) in streams(doc, &p.dict).into_iter().enumerate() {
        for (i, op) in parse(&data).ops.iter().enumerate() {
            match op.op.as_slice() {
                b"q" => stack.push(ctm),
                b"Q" => ctm = stack.pop().unwrap_or(ctm),
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        ctm = Matrix(m).then(&ctm);
                    }
                }
                b"Do" => {
                    let Some(name) = op.name(0) else { continue };
                    let Some(r) = xo.get(name).and_then(Object::as_ref) else { continue };
                    let obj = doc.get(r);
                    let Object::Stream(s) = &*obj else { continue };
                    if s.dict.name(b"Subtype") != Some(b"Image") {
                        continue;
                    }
                    out.push(PageImage {
                        rect: ctm.bbox([0.0, 0.0, 1.0, 1.0]),
                        matrix: ctm.0,
                        name: String::from_utf8_lossy(name).into_owned(),
                        object: Some(r),
                        width: s.dict.int(b"Width").unwrap_or(0).max(0) as u32,
                        height: s.dict.int(b"Height").unwrap_or(0).max(0) as u32,
                        stream: si,
                        op: i,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

/// What to do with an image.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageChange {
    /// Apply a user-space transform after its placement (move, resize, rotate, flip).
    Transform([f64; 6]),
    /// Draw this image XObject instead, in the same place.
    Replace(ObjRef),
    Delete,
}

/// The user-space transform that maps box `from` onto box `to` (move and resize).
pub fn rect_to_rect(from: [f64; 4], to: [f64; 4]) -> [f64; 6] {
    let (fw, fh) = ((from[2] - from[0]).max(1e-6), (from[3] - from[1]).max(1e-6));
    let (sx, sy) = ((to[2] - to[0]) / fw, (to[3] - to[1]) / fh);
    [sx, 0.0, 0.0, sy, to[0] - from[0] * sx, to[1] - from[1] * sy]
}

/// Turn the image a quarter turn clockwise (`quarters` = 1, 2, 3) about its centre, or flip it.
pub fn turn_about_centre(rect: [f64; 4], quarters: i32, flip_h: bool, flip_v: bool) -> [f64; 6] {
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let (a, b, c, d) = match quarters.rem_euclid(4) {
        1 => (0.0, -1.0, 1.0, 0.0),
        2 => (-1.0, 0.0, 0.0, -1.0),
        3 => (0.0, 1.0, -1.0, 0.0),
        _ => (1.0, 0.0, 0.0, 1.0),
    };
    let (fx, fy) = (if flip_h { -1.0 } else { 1.0 }, if flip_v { -1.0 } else { 1.0 });
    let (a, b, c, d) = (a * fx, b * fy, c * fx, d * fy);
    // Translate the centre to the origin, turn, translate back.
    [a, b, c, d, cx - a * cx - c * cy, cy - b * cx - d * cy]
}

/// Change image `index` (from [`page_images`]) on `page`.
pub fn change_image(doc: &mut Document, page: usize, index: usize, change: &ImageChange) -> Result<(), EditError> {
    let images = page_images(doc, page)?;
    let img = images.get(index).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no image {}", page + 1, index + 1)))?;
    let p = page_of(doc, page)?;
    let all = streams(doc, &p.dict);
    let (obj, data) = all[img.stream].clone();
    let mut ops = parse(&data).ops;
    let mut resources = None;
    let replacement: Vec<Op> = match change {
        ImageChange::Delete => Vec::new(),
        ImageChange::Transform(t) => {
            // New CTM = placement × T; as a `cm` before the Do: M = P · T · P⁻¹.
            let pm = Matrix(img.matrix);
            let inv = pm.invert().ok_or_else(|| EditError::Invalid("the image has no area".into()))?;
            let m = pm.then(&Matrix(*t)).then(&inv);
            vec![
                Op::new("q", vec![]),
                Op::new("cm", m.0.iter().map(|v| pdfcraft_content::num(*v)).collect()),
                ops[img.op].clone(),
                Op::new("Q", vec![]),
            ]
        }
        ImageChange::Replace(r) => {
            let name = format!("PCImg{}", r.num);
            let mut res = p.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            let mut xo = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
            xo.set(name.clone().into_bytes(), Object::Ref(*r));
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            resources = Some(res);
            vec![Op::new("Do", vec![Object::name(&name)])]
        }
    };
    ops.splice(img.op..=img.op, replacement);
    let mut dict = match &*doc.resolve(&obj) {
        Object::Stream(s) => s.dict.clone(),
        _ => Dict::new(),
    };
    dict.remove(b"Length");
    let new = doc.add(Object::Stream(Stream::flate(dict, &serialize_ops(&ops))));
    let contents: Vec<Object> = all.iter().enumerate().map(|(i, (o, _))| if i == img.stream { Object::Ref(new) } else { o.clone() }).collect();
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), if contents.len() == 1 { contents[0].clone() } else { Object::Array(contents) });
        if let Some(res) = resources {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(())
}
