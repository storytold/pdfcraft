//! 3D annotations (ISO 32000-2 §13.6.2): a U3D or PRC model shown in a rectangle on a page,
//! with a poster (`/AP /N`) for printing, for readers without 3D, and until it is activated.

use super::create::{finite, normalize, num_array};
use super::lookup::{annots, page_ref, set_annots};
use super::types::{FLAG_PRINT, Meta};
use super::{AnnotError, Dict, Document, ObjRef, Object, PdfString, Stream, appearance, n};

/// The largest model accepted, in bytes: well past real U3D/PRC files, and bounded because the
/// model is held and compressed in memory.
pub const MAX_MODEL_BYTES: usize = 256 << 20;

/// What the poster says when there is no picture.
const PLACEHOLDER_LABEL: &str = "3D model";

/// A 3D stream's data format (§13.6.3): ECMA-363 U3D or ISO 14739-1 PRC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    U3d,
    Prc,
}

impl ModelFormat {
    /// The format a model file's first bytes declare: U3D files start `U3D\0` (ECMA-363 file
    /// header block type), PRC files start `PRC`.
    ///
    /// # Errors
    ///
    /// [`AnnotError::NotAModel`] for anything else.
    pub fn detect(bytes: &[u8]) -> Result<Self, AnnotError> {
        if bytes.starts_with(b"U3D\0") {
            Ok(Self::U3d)
        } else if bytes.starts_with(b"PRC") {
            Ok(Self::Prc)
        } else {
            Err(AnnotError::NotAModel)
        }
    }

    /// The 3D stream's `/Subtype`.
    pub fn subtype(self) -> &'static str {
        match self {
            Self::U3d => "U3D",
            Self::Prc => "PRC",
        }
    }

    /// The PDF version a document holding it needs (§13.6): a 3D annotation is 1.6, a PRC stream
    /// 1.7 under Adobe's extension, declared by [`add_3d_model`] (PDF 2.0 takes PRC as standard).
    pub fn version(self) -> &'static str {
        match self {
            Self::U3d => "1.6",
            Self::Prc => "1.7",
        }
    }
}

/// When a reader turns the poster into the live model (§13.6.2, the activation dictionary's
/// `/A`). The standard's default is [`Self::Click`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Activation {
    /// `/XA`: when the user clicks it; it prints as its poster.
    #[default]
    Click,
    /// `/PO`: when the page opens.
    PageOpen,
    /// `/PV`: when the page becomes visible.
    PageVisible,
}

impl Activation {
    /// Every activation, with the word a tool argument names it by.
    pub const ALL: [(Self, &'static str); 3] = [(Self::Click, "click"), (Self::PageOpen, "page_open"), (Self::PageVisible, "page_visible")];

    /// The `/A` name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Click => "XA",
            Self::PageOpen => "PO",
            Self::PageVisible => "PV",
        }
    }
}

/// A picture already in the document (an image XObject, or a form XObject whose `/Matrix` maps it
/// to `size` points) drawn into `at`, a box inside the annotation's rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PosterPicture {
    pub xobject: ObjRef,
    pub image: bool,
    pub size: (f64, f64),
    pub at: [f64; 4],
}

/// A 3D model to add on `page` (0-based) in `rect` (user space).
#[derive(Clone, Debug, PartialEq)]
pub struct NewModel3d<'a> {
    pub page: usize,
    pub rect: [f64; 4],
    pub model: &'a [u8],
    pub activation: Activation,
    /// The poster's picture; `None` draws a labelled frame.
    pub poster: Option<PosterPicture>,
    /// `/Contents`: the annotation's alternate description (§12.5.2).
    pub contents: &'a str,
}

/// Check a model before anything is added: its size and its format.
///
/// # Errors
///
/// [`AnnotError::ModelTooLarge`] past [`MAX_MODEL_BYTES`], else as [`ModelFormat::detect`].
pub fn check_model(model: &[u8]) -> Result<ModelFormat, AnnotError> {
    check_model_size(model, MAX_MODEL_BYTES)?;
    ModelFormat::detect(model)
}

fn check_model_size(model: &[u8], max: usize) -> Result<(), AnnotError> {
    if model.len() > max {
        return Err(AnnotError::ModelTooLarge { size: model.len(), max });
    }
    Ok(())
}

/// Add a 3D annotation: the model in a Flate-compressed 3D stream referenced from `/3DD` (so the
/// annotation has its own instance, §13.6.2), `/3DA` stating only `/A`, and the poster as `/AP /N`.
/// No view (`/3DV`, `/VA`) is written: readers open on the model's own default view. A document
/// older than [`ModelFormat::version`] is raised to it. Returns its index in the page's `/Annots`.
///
/// # Errors
///
/// As [`check_model`]; [`AnnotError::NoSuchPage`]; [`AnnotError::Invalid`] for a rectangle with no
/// area or a poster box outside it.
pub fn add_3d_model(doc: &mut Document, new: &NewModel3d<'_>, meta: &Meta) -> Result<usize, AnnotError> {
    let format = check_model(new.model)?;
    let page = page_ref(doc, new.page)?;
    let rect = normalize(new.rect);
    if !finite(&new.rect) || rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
        return Err(AnnotError::Invalid("invalid rectangle for the 3D model (each side at least 1 pt)".into()));
    }
    let poster = match &new.poster {
        Some(p) => picture_poster(rect, p)?,
        None => placeholder_poster(rect),
    };
    let mut stream = Dict::new();
    stream.set(b"Type".to_vec(), Object::name("3D"));
    stream.set(b"Subtype".to_vec(), Object::name(format.subtype()));
    let stream = doc.add(Object::Stream(Stream::flate(stream, new.model)));
    let poster = doc.add(Object::Stream(poster));
    let d = annotation_dict(new, rect, page, stream, poster, meta);
    let r = doc.add(Object::Dict(d));
    let mut list = annots(doc, page);
    let index = list.len();
    list.push(Object::Ref(r));
    set_annots(doc, page, list)?;
    at_least_version(doc, format.version())?;
    if format == ModelFormat::Prc && !at_least(doc, "2.0") {
        declare_prc_extension(doc)?;
    }
    Ok(index)
}

/// Whether the header or the catalog `/Version` says `min` or later (§7.7.2).
fn at_least(doc: &Document, min: &str) -> bool {
    let reached = |v: &[u8]| v.len() >= 3 && v >= min.as_bytes();
    let catalog = doc.root().and_then(|root| doc.get(root).as_dict().and_then(|c| c.get(b"Version").cloned()));
    let catalog = catalog.and_then(|v| doc.resolve(&v).as_name().map(<[u8]>::to_vec));
    reached(doc.version().as_bytes()) || catalog.as_deref().is_some_and(reached)
}

/// PRC came before PDF 2.0 as an Adobe extension to 1.7: such a file declares
/// `/Extensions /ADBE` with ExtensionLevel 1 or later (§7.12). Other extensions and an ADBE entry
/// that already has a level are kept; one that is not a dictionary (an array of them) is left alone.
fn declare_prc_extension(doc: &mut Document) -> Result<(), AnnotError> {
    let Some(root) = doc.root() else { return Ok(()) };
    let catalog = doc.get(root);
    // `None` when present but not a dictionary; an empty one when absent.
    let dict_or_new = |o: Option<&Object>| match o.map(|o| doc.resolve(o)) {
        None => Some(Dict::new()),
        Some(o) => o.as_dict().cloned(),
    };
    let Some(mut extensions) = dict_or_new(catalog.as_dict().and_then(|c| c.get(b"Extensions"))) else { return Ok(()) };
    let Some(mut adbe) = dict_or_new(extensions.get(b"ADBE")) else { return Ok(()) };
    if adbe.get(b"ExtensionLevel").map(|l| doc.resolve(l)).and_then(|l| l.as_int()).is_some_and(|l| l >= 1) {
        return Ok(());
    }
    adbe.set(b"BaseVersion".to_vec(), Object::name("1.7"));
    adbe.set(b"ExtensionLevel".to_vec(), Object::Int(1));
    extensions.set(b"ADBE".to_vec(), Object::Dict(adbe));
    doc.update_dict(root, |c| c.set(b"Extensions".to_vec(), Object::Dict(extensions)))?;
    Ok(())
}

/// A document whose header and catalog `/Version` both say less than `min` gets `/Version min` in
/// its catalog (§7.7.2: the later of the two applies).
fn at_least_version(doc: &mut Document, min: &str) -> Result<(), AnnotError> {
    let Some(root) = doc.root() else { return Ok(()) };
    if !at_least(doc, min) {
        doc.update_dict(root, |c| c.set(b"Version".to_vec(), Object::name(min)))?;
    }
    Ok(())
}

fn annotation_dict(new: &NewModel3d<'_>, rect: [f64; 4], page: ObjRef, stream: ObjRef, poster: ObjRef, meta: &Meta) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("3D"));
    d.set(b"Rect".to_vec(), num_array(&rect));
    d.set(b"P".to_vec(), Object::Ref(page));
    d.set(b"F".to_vec(), Object::Int(FLAG_PRINT));
    if !new.contents.is_empty() {
        d.set(b"Contents".to_vec(), PdfString::text(new.contents));
    }
    if !meta.id.is_empty() {
        d.set(b"NM".to_vec(), PdfString::text(&meta.id));
    }
    if let Some(date) = &meta.date {
        d.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    }
    d.set(b"3DD".to_vec(), Object::Ref(stream));
    let mut activation = Dict::new();
    activation.set(b"A".to_vec(), Object::name(new.activation.name()));
    d.set(b"3DA".to_vec(), Object::Dict(activation));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Ref(poster));
    d.set(b"AP".to_vec(), Object::Dict(ap));
    d
}

/// The poster drawing `p` into its box, which must lie within `rect`.
fn picture_poster(rect: [f64; 4], p: &PosterPicture) -> Result<Stream, AnnotError> {
    let at = normalize(p.at);
    // A box fitted to `rect` in floating point can end a few ulps outside it: allow that, then clamp.
    let slack = rect.iter().fold(1.0_f64, |m, v| m.max(v.abs())) * 1e-9;
    let near = finite(&p.at) && at[0] >= rect[0] - slack && at[1] >= rect[1] - slack && at[2] <= rect[2] + slack && at[3] <= rect[3] + slack;
    let at = [at[0].max(rect[0]), at[1].max(rect[1]), at[2].min(rect[2]), at[3].min(rect[3])];
    if !(near && at[2] > at[0] && at[3] > at[1]) {
        return Err(AnnotError::Invalid("the poster must fit inside the 3D model's rectangle".into()));
    }
    let place = appearance::picture_place(at, p.image, Some(p.size)).ok_or_else(|| AnnotError::Invalid("the poster picture has no size".into()))?;
    let mut xobjects = Dict::new();
    xobjects.set(b"Pic".to_vec(), Object::Ref(p.xobject));
    let mut resources = Dict::new();
    resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
    Ok(appearance::form(rect, format!("q {place} /Pic Do Q\n").as_bytes(), resources))
}

/// A light frame with "3D model" centred in it, shrunk to fit (left out when even 2 pt won't).
fn placeholder_poster(rect: [f64; 4]) -> Stream {
    let [x0, y0, x1, y1] = rect;
    let (w, h) = (x1 - x0, y1 - y0);
    let mut c =
        format!("0.94 g {} {} {} {} re f\n0.45 G 1 w {} {} {} {} re S\n", n(x0), n(y0), n(w), n(h), n(x0 + 0.5), n(y0 + 0.5), n(w - 1.0), n(h - 1.0));
    let mut resources = Dict::new();
    let unit = appearance::text_width(PLACEHOLDER_LABEL, 1.0);
    let size = if unit > 0.0 { (w * 0.8 / unit).min(h * 0.4).min(14.0) } else { 0.0 };
    if size >= 2.0 {
        let (tx, ty) = (x0 + (w - unit * size) / 2.0, y0 + (h - size * 0.7) / 2.0);
        c.push_str(&format!("BT 0.3 g /Helv {} Tf {} {} Td ({PLACEHOLDER_LABEL}) Tj ET\n", n(size), n(tx), n(ty)));
        resources.set(b"Font".to_vec(), appearance::helvetica_resource());
    }
    appearance::form(rect, c.as_bytes(), resources)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_size_is_capped() {
        assert_eq!(check_model_size(&[0; 8], 8), Ok(()));
        assert_eq!(check_model_size(&[0; 9], 8), Err(AnnotError::ModelTooLarge { size: 9, max: 8 }));
    }

    #[test]
    fn the_format_is_read_from_the_header() {
        assert_eq!(ModelFormat::detect(b"U3D\0rest"), Ok(ModelFormat::U3d));
        assert_eq!(ModelFormat::detect(b"PRC8800"), Ok(ModelFormat::Prc));
        for junk in [&b""[..], b"U3D", b"U3DX", b"PR", b"%PDF-1.7", b"prc"] {
            assert_eq!(ModelFormat::detect(junk), Err(AnnotError::NotAModel), "{junk:?}");
        }
    }

    #[test]
    fn the_placeholder_label_shrinks_to_fit_and_is_dropped_when_it_cannot() {
        let wide = String::from_utf8(placeholder_poster([0.0, 0.0, 300.0, 200.0]).decoded().unwrap().to_vec()).unwrap();
        assert!(wide.contains("/Helv 14 Tf") && wide.contains("(3D model) Tj"), "{wide}");
        let tiny = placeholder_poster([0.0, 0.0, 4.0, 4.0]);
        assert!(!String::from_utf8(tiny.decoded().unwrap().to_vec()).unwrap().contains("Tj"));
        assert!(tiny.dict.get(b"Resources").and_then(Object::as_dict).is_none_or(|r| r.get(b"Font").is_none()));
    }
}
