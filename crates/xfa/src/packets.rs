//! The XDP packets inside a PDF (`/AcroForm /XFA`, ISO 32000-1 §12.7.8): either one stream
//! holding the whole XDP, or an array of (name, stream) pairs.

use pdfcraft_cos::{Document, Object};

use crate::XfaError;

/// Most decoded packet bytes read in total.
const MAX_XDP: usize = 64 << 20;

/// What was found in the XFA entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Packets {
    /// The XDP as one XML document (packets concatenated in order), or the template packet
    /// alone when there is no XDP envelope. Empty when the entry holds no template.
    pub xdp: String,
    /// `/NeedsRendering true`: the form expects to be laid out from the template.
    pub needs_rendering: bool,
    /// Whether the AcroForm has any fields of its own.
    pub has_fields: bool,
}

fn to_text(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            char::decode_utf16(rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c))).map(|c| c.unwrap_or('\u{FFFD}')).collect()
        }
        [0xFE, 0xFF, rest @ ..] => {
            char::decode_utf16(rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c))).map(|c| c.unwrap_or('\u{FFFD}')).collect()
        }
        _ => String::from_utf8_lossy(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)).into_owned(),
    }
}

/// Read the XFA packets, if the document has any.
pub fn read_packets(doc: &Document) -> Result<Option<Packets>, XfaError> {
    let Some(root) = doc.root() else { return Ok(None) };
    let catalog = doc.get(root);
    let Some(acro) = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).map(|a| doc.resolve(a)) else { return Ok(None) };
    let Some(acro) = acro.as_dict() else { return Ok(None) };
    let Some(xfa) = acro.get(b"XFA").map(|x| doc.resolve(x)) else { return Ok(None) };
    // `/NeedsRendering` is a catalog entry (ISO 32000-1 Table 28); some producers put it on
    // the form dictionary, so look there too.
    let bool_of = |v: Option<&Object>| match v.map(|v| doc.resolve(v)).as_deref() {
        Some(Object::Bool(b)) => Some(*b),
        _ => None,
    };
    let needs_rendering = bool_of(catalog.as_dict().and_then(|c| c.get(b"NeedsRendering"))).or_else(|| bool_of(acro.get(b"NeedsRendering")));
    let has_fields = acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().map(|a| !a.is_empty())).unwrap_or(false);
    let mut total = 0usize;
    let mut read = |o: &Object| -> Result<Option<String>, XfaError> {
        let s = doc.resolve(o);
        let Object::Stream(s) = &*s else { return Ok(None) };
        let bytes = s.decoded_within(MAX_XDP)?;
        total = total.saturating_add(bytes.len());
        if total > MAX_XDP {
            return Err(XfaError::TooLarge("the XFA packets are larger than 64 MB".into()));
        }
        Ok(Some(to_text(&bytes)))
    };
    let mut xdp = String::new();
    match &*xfa {
        Object::Array(items) => {
            let mut template = None;
            let mut i = 0;
            while i < items.len() {
                let name = items.get(i).and_then(|n| n.as_string()).map(|s| s.to_text());
                let stream = if name.is_some() { items.get(i + 1) } else { items.get(i) };
                if let Some(o) = stream
                    && let Some(text) = read(o)?
                {
                    if name.as_deref() == Some("template") {
                        template = Some(text.clone());
                    }
                    xdp.push_str(&text);
                    xdp.push('\n');
                }
                i += if name.is_some() { 2 } else { 1 };
            }
            if !xdp.contains("<template") {
                xdp = template.unwrap_or_default();
            }
        }
        Object::Stream(_) => {
            xdp = read(&xfa)?.unwrap_or_default();
        }
        // Not a stream or an array: an XFA entry all the same, with nothing to lay out.
        _ => {}
    }
    if !xdp.contains("<template") {
        xdp.clear();
    }
    Ok(Some(Packets { xdp, needs_rendering: needs_rendering.unwrap_or(false), has_fields }))
}
