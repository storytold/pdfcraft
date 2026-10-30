use super::common::{MAX_DEPTH, SUBTYPES, rect_strict, text_of};
use super::xfdf::{add_popup, apply_values, find_by_name, place};
use super::*;

fn pdf_string(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    pdfcraft_cos::serialize(&Object::String(PdfString::text(s)), &mut v);
    v
}

// Keep aliases as aliases instead of expanding a shared graph once per use. References are
// queued, not followed recursively, so cycles and long indirect chains cost linear work.
pub(crate) const MAX_FDF_OBJECTS: usize = 100_000;

pub(crate) const MAX_FDF_BYTES: usize = 64 * 1024 * 1024;

/// Direct values stay borrowed: `Document::resolve` would deep-clone them. Only
/// indirect values need an owner, and annotation bodies are never retained in discovery.
enum FdfValue<'a> {
    Borrowed(&'a Object),
    Indirect(std::sync::Arc<Object>),
}

impl<'a> FdfValue<'a> {
    fn resolve(doc: &Document, value: &'a Object) -> Self {
        match value {
            Object::Ref(_) => Self::Indirect(doc.resolve(value)),
            _ => Self::Borrowed(value),
        }
    }

    fn object(&self) -> &Object {
        match self {
            Self::Borrowed(value) => value,
            Self::Indirect(value) => value.as_ref(),
        }
    }
}

struct FdfCopy<'a> {
    doc: &'a Document,
    objects: Vec<Object>,
    numbers: HashMap<ObjRef, u32>,
    pending: Vec<(ObjRef, usize)>,
    nodes: usize,
    bytes: usize,
}

impl<'a> FdfCopy<'a> {
    fn new(doc: &'a Document) -> Self {
        Self { doc, objects: Vec::new(), numbers: HashMap::new(), pending: Vec::new(), nodes: MAX_FDF_OBJECTS, bytes: MAX_FDF_BYTES }
    }

    fn charge(&mut self, bytes: usize) -> Result<(), DataError> {
        self.bytes = self.bytes.checked_sub(bytes).ok_or(DataError::ExportLimit("64 MiB data"))?;
        Ok(())
    }

    fn slot(&mut self) -> Result<(usize, u32), DataError> {
        if self.objects.len() >= MAX_FDF_OBJECTS {
            return Err(DataError::ExportLimit("100,000 objects"));
        }
        // Object 1 is the FDF root; arena indices start at object 2. The arena is capped above.
        let index = self.objects.len();
        let number = index as u32 + 2;
        self.charge(100)?; // indirect-object and xref overhead
        self.objects.push(Object::Null);
        Ok((index, number))
    }

    fn dict(&mut self, dict: &Dict, depth: usize) -> Result<Dict, DataError> {
        if dict.len() > self.nodes {
            return Err(DataError::ExportLimit("100,000 values"));
        }
        let mut out = Dict::new();
        for (key, value) in dict.iter() {
            self.dict_entry(&mut out, key, value, depth + 1)?;
        }
        Ok(out)
    }

    fn dict_entry(&mut self, out: &mut Dict, key: &[u8], value: &Object, depth: usize) -> Result<(), DataError> {
        self.charge(key.len().saturating_mul(3).saturating_add(2))?;
        let value = self.copy(value, depth)?;
        out.set(key.to_vec(), value);
        Ok(())
    }

    /// Skip source-document links and appearances without cloning their values. The
    /// dictionary and every retained entry are charged before any owned copy is made.
    fn annotation_dict(&mut self, dict: &Dict) -> Result<Dict, DataError> {
        self.charge_value(0)?;
        let mut out = Dict::new();
        for (key, value) in dict.iter() {
            if matches!(key.as_slice(), b"P" | b"AP" | b"Popup" | b"IRT" | b"Parent" | b"FS" | b"Sound") {
                continue;
            }
            self.dict_entry(&mut out, key, value, 1)?;
        }
        Ok(out)
    }

    fn charge_value(&mut self, depth: usize) -> Result<(), DataError> {
        if depth > MAX_DEPTH {
            return Err(DataError::ExportLimit("64 direct nesting levels"));
        }
        self.nodes = self.nodes.checked_sub(1).ok_or(DataError::ExportLimit("100,000 values"))?;
        // Includes delimiters, spacing and even the longest finite f64 in plain decimal form.
        self.charge(330)?;
        Ok(())
    }

    fn is_source_page(&self, dict: &Dict) -> Result<bool, DataError> {
        let Some(value) = dict.get(b"Type") else { return Ok(false) };
        let mut value = FdfValue::Borrowed(value);
        let mut seen = HashSet::new();
        for hops in 0..=MAX_DEPTH {
            match value.object() {
                Object::Name(name) => return Ok(matches!(name.as_slice(), b"Page" | b"Pages")),
                Object::Ref(reference) => {
                    if self.doc.generation(reference.num) != reference.generation || !seen.insert(*reference) {
                        return Ok(false);
                    }
                    if hops == MAX_DEPTH {
                        return Err(DataError::ExportLimit("64 Type reference levels"));
                    }
                    value = FdfValue::Indirect(self.doc.get(*reference));
                }
                _ => return Ok(false),
            }
        }
        Err(DataError::ExportLimit("64 Type reference levels"))
    }

    fn copy(&mut self, value: &Object, depth: usize) -> Result<Object, DataError> {
        self.charge_value(depth)?;
        Ok(match value {
            Object::Ref(source) => {
                let number = if let Some(number) = self.numbers.get(source) {
                    *number
                } else {
                    let (index, number) = self.slot()?;
                    self.numbers.insert(*source, number);
                    self.pending.push((*source, index));
                    number
                };
                Object::Ref(ObjRef::new(number, 0))
            }
            Object::Array(values) => {
                if values.len() > self.nodes {
                    return Err(DataError::ExportLimit("100,000 values"));
                }
                let mut out = Vec::new();
                for value in values {
                    out.push(self.copy(value, depth + 1)?);
                }
                Object::Array(out)
            }
            // Source page links must not drag page contents or the page tree into an FDF.
            Object::Dict(dict) if self.is_source_page(dict)? => Object::Null,
            Object::Dict(dict) => Object::Dict(self.dict(dict, depth)?),
            Object::String(s) => {
                self.charge(s.bytes.len().saturating_mul(4))?;
                value.clone()
            }
            Object::Name(name) => {
                self.charge(name.len().saturating_mul(3))?;
                value.clone()
            }
            // A PDF stream can only be an indirect object, never a dictionary/array value.
            Object::Stream(_) => return Err(DataError::Malformed("FDF export encountered a direct stream".into())),
            other => other.clone(),
        })
    }

    fn finish(&mut self, doc: &Document) -> Result<(), DataError> {
        let mut cursor = 0;
        while let Some((source, index)) = self.pending.get(cursor).copied() {
            // ISO 32000-2 §7.3.10: an absent or stale reference denotes null. Keep reader
            // errors distinct so damaged objects cannot be silently lost during export.
            let value = if doc.generation(source.num) != source.generation {
                std::sync::Arc::new(Object::Null)
            } else {
                match doc.try_get(source.num) {
                    Ok(value) => value,
                    Err(pdfcraft_cos::CosError::MissingObject(_)) => std::sync::Arc::new(Object::Null),
                    Err(error) => return Err(error.into()),
                }
            };
            let copied = match &*value {
                Object::Stream(stream) => {
                    self.charge(stream.raw.len())?;
                    Object::Stream(pdfcraft_cos::Stream { dict: self.dict(&stream.dict, 0)?, raw: stream.raw.clone() })
                }
                other => self.copy(other, 0)?,
            };
            let slot = self.objects.get_mut(index).ok_or_else(|| DataError::Malformed("FDF object slot is missing".into()))?;
            *slot = copied;
            cursor += 1;
        }
        Ok(())
    }
}

/// FDF with field values and/or comments (appearances are not carried). Each comment is an
/// indirect object of the FDF, so a reply's `/IRT` can point at its parent's object there
/// (§12.7.8.3.1). Nested values are copied and renumbered, including shared or cyclic graphs.
/// Source `/Page` and `/Pages` dictionaries become null at every copy depth.
/// Refuses exports above the object, value, direct-depth or data budget instead of losing data.
pub fn export_fdf(doc: &Document, comments: bool, fields: bool, file: &str) -> Result<Vec<u8>, DataError> {
    let mut copy = FdfCopy::new(doc);
    copy.charge(file.len().saturating_mul(8))?;
    let mut body: Vec<u8> = b"<< /FDF << ".to_vec();
    // Comment dictionaries, written after the root as objects 2, 3, …
    if fields {
        body.extend_from_slice(b"/Fields [");
        for f in pdfcraft_forms::fields(doc) {
            if matches!(f.kind, FieldKind::PushButton | FieldKind::Signature) {
                continue;
            }
            // Field dictionary, /T string and /V scalar/array, plus every emitted list
            // entry. Other field kinds emit only their first value (or a default).
            let field_nodes = if matches!(f.kind, FieldKind::List) && f.value.len() > 1 { f.value.len().saturating_add(3) } else { 3 };
            copy.nodes = copy.nodes.checked_sub(field_nodes).ok_or(DataError::ExportLimit("100,000 values"))?;
            copy.charge(f.name.len().saturating_mul(8).saturating_add(100))?;
            for value in &f.value {
                copy.charge(value.len().saturating_mul(8).saturating_add(100))?;
            }
            body.extend_from_slice(b"<< /T ");
            body.extend(pdf_string(&f.name));
            body.extend_from_slice(b" /V ");
            match f.kind {
                FieldKind::CheckBox | FieldKind::Radio => {
                    let v = f.value.first().cloned().unwrap_or_else(|| "Off".into());
                    pdfcraft_cos::serialize(&Object::Name(pdfcraft_forms::name_bytes(&v)), &mut body);
                }
                FieldKind::List if f.value.len() > 1 => {
                    pdfcraft_cos::serialize(&Object::Array(f.value.iter().map(|v| Object::String(PdfString::text(v))).collect()), &mut body);
                }
                _ => body.extend(pdf_string(f.value.first().map(String::as_str).unwrap_or(""))),
            }
            body.extend_from_slice(b" >> ");
        }
        body.extend_from_slice(b"] ");
    }
    if comments {
        // Keep the page/array owners once. Handles retain positions, never annotation
        // bodies: the document cache can evict and reparse even a repeated source reference.
        let pages = pdfcraft_model::pages(doc);
        let mut arrays = Vec::new();
        let mut picked: Vec<(Option<ObjRef>, usize, usize)> = Vec::new();
        for (pi, page) in pages.iter().enumerate() {
            let owner = page.dict.get(b"Annots").map(|value| FdfValue::resolve(doc, value));
            let before = picked.len();
            if let Some(values) = owner.as_ref().and_then(|owner| owner.object().as_array()) {
                for (ai, a) in values.iter().enumerate() {
                    let value = FdfValue::resolve(doc, a);
                    let Some(d) = value.object().as_dict() else { continue };
                    if d.name(b"Subtype").is_some_and(|sub| SUBTYPES.iter().any(|(s, _)| s.as_bytes() == sub)) {
                        if picked.len() >= MAX_FDF_OBJECTS {
                            return Err(DataError::ExportLimit("100,000 comments"));
                        }
                        picked.push((a.as_ref(), pi, ai));
                    }
                }
            }
            // Empty/unselected arrays need no owner. Resolve subsequent pages only after
            // this page passes the cap, rather than loading them all before discovery.
            arrays.push(if picked.len() > before { owner } else { None });
        }
        let mut numbers = HashMap::new();
        for (source, _, _) in &picked {
            let (_, number) = copy.slot()?;
            if let Some(source) = source {
                numbers.insert(*source, number);
            }
        }
        copy.numbers = numbers.clone();
        let count = picked.len();
        for (index, (_, pi, ai)) in picked.into_iter().enumerate() {
            let annotation = arrays
                .get(pi)
                .and_then(Option::as_ref)
                .and_then(|owner| owner.object().as_array())
                .and_then(|values| values.get(ai))
                .ok_or_else(|| DataError::Malformed("FDF annotation slot is missing".into()))?;
            let value = FdfValue::resolve(doc, annotation);
            let d = value.object().as_dict().ok_or_else(|| DataError::Malformed("FDF comment is not a dictionary".into()))?;
            // A reply points at its parent's object in this FDF; a parent that isn't exported
            // leaves the reply unthreaded.
            let irt = d.get(b"IRT").and_then(Object::as_ref).and_then(|r| numbers.get(&r)).copied();
            let mut d = copy.annotation_dict(d)?;
            if let Some(num) = irt {
                d.set(b"IRT".to_vec(), Object::Ref(ObjRef::new(num, 0)));
            }
            d.set(b"Page".to_vec(), Object::Int(pi as i64));
            let slot = copy.objects.get_mut(index).ok_or_else(|| DataError::Malformed("FDF comment slot is missing".into()))?;
            *slot = Object::Dict(d);
        }
        body.extend_from_slice(b"/Annots [");
        for i in 0..count {
            let _ = write!(body, "{} 0 R ", i + 2);
        }
        body.extend_from_slice(b"] ");
    }
    body.extend_from_slice(b"/F ");
    body.extend(pdf_string(file));
    body.extend_from_slice(b" >> >>");
    copy.finish(doc)?;
    let mut out = b"%FDF-1.2\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(copy.objects.len() + 1);
    offsets.push(out.len());
    out.extend_from_slice(b"1 0 obj\n");
    out.extend(body);
    out.extend_from_slice(b"\nendobj\n");
    for (i, object) in copy.objects.into_iter().enumerate() {
        offsets.push(out.len());
        let _ = writeln!(out, "{} 0 obj", i + 2);
        pdfcraft_cos::serialize(&object, &mut out);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for o in offsets {
        let _ = writeln!(out, "{o:010} 00000 n ");
    }
    let _ = write!(out, "trailer\n<< /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n");
    Ok(out)
}

/// What one FDF import may copy out of the file. A field tree or comment that repeats a reference
/// level after level would otherwise expand a small file into millions of values.
const MAX_FDF_IMPORT_VALUES: usize = 1_000_000;

pub(crate) struct ImportBudget {
    values: usize,
    bytes: usize,
}

impl ImportBudget {
    pub(crate) fn new() -> Self {
        Self { values: MAX_FDF_IMPORT_VALUES, bytes: MAX_FDF_BYTES }
    }

    fn spend(&mut self, values: usize, bytes: usize) -> Result<(), DataError> {
        match (self.values.checked_sub(values), self.bytes.checked_sub(bytes)) {
            (Some(v), Some(b)) => {
                (self.values, self.bytes) = (v, b);
                Ok(())
            }
            _ => Err(DataError::Malformed(format!(
                "the file expands to more than {MAX_FDF_IMPORT_VALUES} values or {} MiB of text",
                MAX_FDF_BYTES >> 20
            ))),
        }
    }
}

/// Copy an object out of another document, resolving references. A reference back to an object
/// being copied becomes null.
fn deep(src: &Document, o: &Object, depth: usize, path: &mut Vec<ObjRef>, budget: &mut ImportBudget) -> Result<Object, DataError> {
    budget.spend(1, 0)?;
    if depth > 16 {
        return Ok(Object::Null);
    }
    Ok(match o {
        Object::Ref(r) => {
            if path.contains(r) {
                return Ok(Object::Null);
            }
            path.push(*r);
            let copy = deep(src, &src.get(*r), depth + 1, path, budget);
            path.pop();
            copy?
        }
        Object::Array(a) => Object::Array(a.iter().map(|x| deep(src, x, depth + 1, path, budget)).collect::<Result<_, _>>()?),
        Object::Dict(d) => {
            Object::Dict(d.iter().map(|(k, v)| Ok((k.clone(), deep(src, v, depth + 1, path, budget)?))).collect::<Result<_, DataError>>()?)
        }
        Object::Stream(_) => Object::Null,
        Object::String(s) => {
            budget.spend(0, s.bytes.len())?;
            o.clone()
        }
        Object::Name(n) => {
            budget.spend(0, n.len())?;
            o.clone()
        }
        other => other.clone(),
    })
}

/// Read an FDF field tree. A field listed again beneath itself is skipped.
pub(crate) fn walk_fdf(
    fdf: &Document,
    list: &[Object],
    prefix: &str,
    out: &mut Vec<(String, Vec<String>)>,
    depth: usize,
    path: &mut Vec<ObjRef>,
    budget: &mut ImportBudget,
) -> Result<(), DataError> {
    if depth > 32 {
        return Ok(());
    }
    for f in list {
        budget.spend(1, 0)?;
        let held;
        let field = match f {
            Object::Ref(r) if path.contains(r) => continue,
            Object::Ref(r) => {
                held = fdf.get(*r);
                &*held
            }
            direct => direct,
        };
        let Some(d) = field.as_dict() else { continue };
        let Some(t) = d.get(b"T").and_then(|t| fdf.resolve(t).as_string().map(|s| s.to_text())) else { continue };
        let full = if prefix.is_empty() { t } else { format!("{prefix}.{t}") };
        budget.spend(0, full.len())?;
        if let Some(v) = d.get(b"V").map(|v| fdf.resolve(v)) {
            let vals: Vec<String> = match &*v {
                Object::String(s) => vec![s.to_text()],
                Object::Name(n) => vec![pdfcraft_forms::name_text(n)],
                Object::Array(a) => {
                    budget.spend(a.len(), 0)?;
                    a.iter().filter_map(|x| fdf.resolve(x).as_string().map(|s| s.to_text())).collect()
                }
                _ => Vec::new(),
            };
            budget.spend(vals.len(), vals.iter().map(String::len).fold(0, usize::saturating_add))?;
            out.push((full.clone(), vals));
        }
        if let Some(kids) = d.get(b"Kids").map(|k| fdf.resolve(k))
            && let Some(kids) = kids.as_array()
        {
            let entered = f.as_ref().map(|r| path.push(r)).is_some();
            let walked = walk_fdf(fdf, kids, &full, out, depth + 1, path, budget);
            if entered {
                path.pop();
            }
            walked?;
        }
    }
    Ok(())
}

pub(crate) fn fdf_body(fdf: &Document) -> Result<Dict, DataError> {
    let root = fdf.root().ok_or_else(|| DataError::Malformed("no /Root".into()))?;
    fdf.get(root)
        .as_dict()
        .and_then(|d| d.get(b"FDF").cloned())
        .map(|f| fdf.resolve(&f))
        .and_then(|f| f.as_dict().cloned())
        .ok_or(DataError::UnknownFormat)
}

pub(crate) fn import_fdf(doc: &mut Document, bytes: &[u8]) -> Result<Report, DataError> {
    let fdf = Document::open(std::sync::Arc::new(bytes.to_vec())).map_err(|e| DataError::Malformed(e.to_string()))?;
    let body = fdf_body(&fdf)?;
    let mut report = Report::default();
    let mut budget = ImportBudget::new();
    // Fields (with /Kids for hierarchical names).
    let mut values = Vec::new();
    if let Some(list) = body.get(b"Fields").map(|f| fdf.resolve(f)).and_then(|f| f.as_array().cloned()) {
        walk_fdf(&fdf, &list, "", &mut values, 0, &mut Vec::new(), &mut budget)?;
    }
    // Comments: parents first, then replies (whose /IRT needs the parent's new reference).
    let pages: Vec<ObjRef> = pdfcraft_model::pages(doc).iter().map(|p| p.obj).collect();
    let mut placed: HashMap<ObjRef, ObjRef> = HashMap::new();
    let mut later: HashMap<ObjRef, (Option<ObjRef>, Option<String>)> = HashMap::new();
    for a in body.get(b"Annots").map(|a| fdf.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
        let Some(mut src) = fdf.resolve(&a).as_dict().cloned() else { continue };
        // The parent is another comment: link it below rather than copying it in.
        let irt = src.get(b"IRT").cloned();
        src.remove(b"IRT");
        // The pop-up is an annotation of its own (/Popup shall be an indirect reference, ISO
        // 32000-2 Table 172): copied in, it would become a direct dictionary. A new one is
        // placed on the page below, as the XFDF import does.
        let popup = src.get(b"Popup").map(|p| fdf.resolve(p)).and_then(|p| p.as_dict().cloned());
        src.remove(b"Popup");
        let mut path: Vec<ObjRef> = a.as_ref().into_iter().collect();
        let Object::Dict(mut d) = deep(&fdf, &Object::Dict(src), 0, &mut path, &mut budget)? else { continue };
        let Some(page) = d.get(b"Page").and_then(Object::as_int).and_then(|p| usize::try_from(p).ok()).and_then(|p| pages.get(p).copied()) else {
            continue;
        };
        if !d.name(b"Subtype").is_some_and(|s| SUBTYPES.iter().any(|(x, _)| x.as_bytes() == s)) {
            continue;
        }
        d.remove(b"Page");
        d.set(b"P".to_vec(), Object::Ref(page));
        let r = place(doc, page, d)?;
        later.remove(&r);
        if let Some(rect) = popup.as_ref().and_then(|p| rect_strict(&fdf, p.get(b"Rect"))) {
            let open = popup.as_ref().and_then(|p| p.get(b"Open")).is_some_and(|o| matches!(&*fdf.resolve(o), Object::Bool(true)));
            add_popup(doc, page, r, &rect, open)?;
        }
        if let Some(fr) = a.as_ref() {
            placed.insert(fr, r);
        }
        if let Some(irt) = irt {
            let parent_name = fdf.resolve(&irt).as_dict().and_then(|p| text_of(&fdf, p, b"NM"));
            later.insert(r, (irt.as_ref(), parent_name));
        }
        report.comments += 1;
    }
    for (r, (parent_obj, parent_name)) in later {
        // The parent imported from this file, else one already in the document with its name.
        let parent = parent_obj.and_then(|p| placed.get(&p).copied()).or_else(|| parent_name.and_then(|nm| find_by_name(doc, &nm)));
        if let Some(parent) = parent.filter(|p| *p != r) {
            doc.update_dict(r, |d| d.set(b"IRT".to_vec(), Object::Ref(parent)))?;
        }
    }
    apply_values(doc, &values, &mut report);
    Ok(report)
}
