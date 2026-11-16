use super::listing::{mk_rotation, nums, sig_fields, text, walk_sig_fields};
use super::{Certificate, Dict, DigestAlg, DigitalId, Document, HashSet, ObjRef, Object, PdfString, SaveOptions, SignError, SignedData, Stream};

// ── signing ─────────────────────────────────────────────────────────────────────────────────

/// What the visible signature shows (Acrobat: Configure Signature Appearance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub name: bool,
    pub date: bool,
    pub reason: bool,
    pub location: bool,
    pub distinguished_name: bool,
    pub labels: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { name: true, date: true, reason: false, location: false, distinguished_name: false, labels: true }
    }
}

/// The words of the labels of a visible signature.
///
/// They are stamped into the document, so the caller chooses them: an interface that speaks
/// another language passes them in that language. The defaults are the English ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppearanceLabels {
    pub signed_by: String,
    pub distinguished_name: String,
    pub reason: String,
    pub location: String,
    pub date: String,
}

impl Default for AppearanceLabels {
    fn default() -> Self {
        Self {
            signed_by: "Digitally signed by".into(),
            distinguished_name: "DN: ".into(),
            reason: "Reason: ".into(),
            location: "Location: ".into(),
            date: "Date: ".into(),
        }
    }
}

impl AppearanceLabels {
    /// Each label with its field name (`signed_by`, `distinguished_name`, `reason`, `location`,
    /// `date`), in the order the visible signature shows them.
    pub fn fields(&self) -> [(&'static str, &str); 5] {
        [
            ("signed_by", self.signed_by.as_str()),
            ("distinguished_name", self.distinguished_name.as_str()),
            ("reason", self.reason.as_str()),
            ("location", self.location.as_str()),
            ("date", self.date.as_str()),
        ]
    }

    /// The first label, with its field name, that has a character the visible signature's font
    /// can't draw, and that character. Signing refuses such a label when it is shown, so an
    /// interface checks its translated labels here and falls back to [`Default`] if one fails.
    pub fn first_undrawable(&self) -> Option<(&'static str, char)> {
        self.fields().into_iter().find_map(|(field, text)| {
            text.chars()
                .find(|c| {
                    !bidi_control(*c)
                        && !c.is_whitespace()
                        && !pdfcraft_fonts::win_ansi_encodable(*c)
                        && !(is_arabic(*c) && pdfcraft_fonts::arabic_has(*c))
                })
                .map(|c| (field, c))
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct SignOptions {
    /// Sign this existing, unsigned signature field.
    pub field: Option<String>,
    /// Or create a field on this page (0-based) with this rectangle (`None`: invisible).
    pub page: usize,
    pub rect: Option<[f64; 4]>,
    /// Name for a new field (default `Signature1`, `Signature2`…).
    pub new_field_name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    /// The signing time as a PDF date (`/M`), from the caller's clock.
    pub date: String,
    /// Certify with these DocMDP permissions (1 no changes, 2 form fill and signing, 3 also comments).
    pub certify: Option<u8>,
    pub appearance: Appearance,
    /// The words of the labels the visible signature shows.
    pub appearance_labels: AppearanceLabels,
    /// A drawing, typed signature or image shown where the large name would be.
    pub graphic: Option<crate::graphic::Graphic>,
}

/// What an encrypted document's permissions (ISO 32000-2 §7.6.4.2, Table 22) allow when it was
/// opened with the user password (the owner may do anything): signing an existing, empty
/// signature field is filling in a form field (bit 9, or bit 6); creating a new signature
/// field needs bits 6 and 4; certifying sets the catalog's `/Perms`, a document change (bit 4).
/// The signature's `/Contents` stays unencrypted (`pdfcraft-cos` exempts it, §7.6.2), so the
/// placeholder can be patched after hashing exactly as in an unencrypted file.
fn check_signing_permissions(doc: &Document, opts: &SignOptions) -> Result<(), SignError> {
    let Some(p) = doc.permissions() else { return Ok(()) };
    let refuse = |what: &str| {
        Err(SignError::Pdf(format!(
            "the document's security settings don't allow {what}; open it with the permissions password, or sign an existing signature field"
        )))
    };
    if opts.field.is_some() && !p.fill_forms() {
        return Err(SignError::Pdf(
            "the document's security settings don't allow filling in form fields or signing; open it with the permissions password to sign it".into(),
        ));
    }
    if opts.field.is_none() && !(p.annotate() && p.modify()) {
        return refuse("adding new signature fields");
    }
    if opts.certify.is_some() && !p.modify() {
        return refuse("certifying it");
    }
    Ok(())
}

/// Placeholder `/ByteRange` values: fixed width, patched after writing.
const BR_MARK: [i64; 3] = [1_111_111_111, 2_222_222_222, 3_333_333_333];

/// Whether `doc` already holds a signature or a document timestamp: a signature dictionary whose
/// `/Contents` holds a nonzero byte (a field whose `/V` has an empty or zero-filled `/Contents`
/// is not signed yet). A signature field without a name (`/T`) counts too: indirect signature
/// dictionaries and document timestamps are found in one pass over the file (by `/ByteRange`,
/// or as timestamps), not through the field tree.
pub fn has_signatures(doc: &Document) -> bool {
    let signed = |d: &Dict| {
        let contents = d.get(b"Contents").map(|c| doc.resolve(c));
        // Hex strings decode to the DER when parsed; a literal string carries hex text.
        let der = contents
            .as_deref()
            .and_then(Object::as_string)
            .and_then(|s| if s.hex { Some(s.bytes.clone()) } else { super::validate::unhex(&s.bytes) });
        der.is_some_and(|der| der.iter().any(|b| *b != 0))
    };
    sig_fields(doc).iter().any(|f| f.dict.get(b"V").map(|v| doc.resolve(v)).as_deref().and_then(Object::as_dict).is_some_and(signed))
        || doc
            .scan_objects()
            .any(|(_, o)| o.as_dict().is_some_and(|d| (d.contains(b"ByteRange") || super::validate::is_doc_timestamp(d)) && signed(d)))
}

/// Sign `doc` with `id`; returns the signed file (an incremental update when possible).
///
/// The document's first signature (no [`has_signatures`]) removes the AcroForm's
/// `/NeedAppearances` flag: Acrobat does not recognise a signature while it is set. Later
/// signatures leave the flag as it is. This function draws no field appearances: once the flag is
/// gone, viewers show each field's stored normal appearance (`/AP /N`), so a filled field whose
/// stored appearance draws no text (an empty placeholder such as `/Tx BMC EMC`, or a look left
/// from an earlier value) shows that instead of its value. A caller that wants the values kept
/// draws the fields first, as `Session::sign` in the engine does.
pub fn sign(doc: &Document, id: &DigitalId, opts: &SignOptions) -> Result<Vec<u8>, SignError> {
    sign_inner(doc, id, opts, None)
}

/// Sign and embed an RFC 3161 signature timestamp from `tsa` (PAdES B-T). The transport is the
/// caller's: this crate never opens a socket, and a rejected or malformed token fails the
/// signing before any file is produced. The form's `/NeedAppearances` flag is handled as in
/// [`sign`].
pub fn sign_with_timestamp(
    doc: &Document,
    id: &DigitalId,
    opts: &SignOptions,
    tsa: &dyn crate::timestamp::TimestampAuthority,
) -> Result<Vec<u8>, SignError> {
    sign_inner(doc, id, opts, Some(tsa))
}

/// Extra `/Contents` headroom for an attached RFC 3161 token (a TSA response with its
/// certificate chain is typically 4–8 KiB; the token is also size-capped on parsing).
const TOKEN_RESERVE: usize = 16 * 1024;

fn sign_inner(
    doc: &Document,
    id: &DigitalId,
    opts: &SignOptions,
    tsa: Option<&dyn crate::timestamp::TimestampAuthority>,
) -> Result<Vec<u8>, SignError> {
    // A pending security change would make the incremental save a full rewrite that applies
    // or removes protection as it signs (and breaks existing signatures): save it first.
    if doc.encryption_changed() {
        return Err(SignError::Pdf("save the document's new security settings before signing it".into()));
    }
    check_signing_permissions(doc, opts)?;
    let mut doc = doc.clone();
    let earlier_signatures = has_signatures(&doc);
    let root = doc.root().ok_or_else(|| SignError::Pdf("the document has no catalog".into()))?;
    let alg = id.key.preferred_digest();
    let reserve =
        8192 + id.certificate.raw.len() + id.chain.iter().map(|c| c.raw.len()).sum::<usize>() + if tsa.is_some() { TOKEN_RESERVE } else { 0 };
    let name = id.certificate.display_name();
    // The signature dictionary.
    let mut v = Dict::new();
    v.set(b"Type".to_vec(), Object::name("Sig"));
    v.set(b"Filter".to_vec(), Object::name("Adobe.PPKLite"));
    v.set(b"SubFilter".to_vec(), Object::name("ETSI.CAdES.detached"));
    v.set(b"ByteRange".to_vec(), Object::Array([0].iter().chain(BR_MARK.iter()).map(|n| Object::Int(*n)).collect()));
    v.set(b"Contents".to_vec(), Object::String(PdfString { bytes: vec![0; reserve], hex: true }));
    v.set(b"M".to_vec(), PdfString::literal(opts.date.as_bytes().to_vec()));
    v.set(b"Name".to_vec(), PdfString::text(&name));
    for (k, val) in [(&b"Reason"[..], &opts.reason), (b"Location", &opts.location), (b"ContactInfo", &opts.contact)] {
        if let Some(s) = val.as_deref().filter(|s| !s.trim().is_empty()) {
            v.set(k.to_vec(), PdfString::text(s));
        }
    }
    let mut app = Dict::new();
    app.set(b"Name".to_vec(), Object::name("PdfCraft"));
    let mut build = Dict::new();
    build.set(b"App".to_vec(), Object::Dict(app));
    v.set(b"Prop_Build".to_vec(), Object::Dict(build));
    if let Some(p) = opts.certify {
        let mut tp = Dict::new();
        tp.set(b"Type".to_vec(), Object::name("TransformParams"));
        tp.set(b"P".to_vec(), Object::Int(p.clamp(1, 3) as i64));
        tp.set(b"V".to_vec(), Object::name("1.2"));
        let mut sr = Dict::new();
        sr.set(b"Type".to_vec(), Object::name("SigRef"));
        sr.set(b"TransformMethod".to_vec(), Object::name("DocMDP"));
        sr.set(b"TransformParams".to_vec(), Object::Dict(tp));
        v.set(b"Reference".to_vec(), Object::Array(vec![Object::Dict(sr)]));
    }
    let sig = doc.add(Object::Dict(v.clone()));
    if opts.certify.is_some() {
        // Catalog /Perms /DocMDP points at the certification signature.
        let mut perms = Dict::new();
        perms.set(b"DocMDP".to_vec(), Object::Ref(sig));
        doc.update_dict(root, |c| c.set(b"Perms".to_vec(), Object::Dict(perms)))?;
    }
    // The field and its widget.
    let pages = pdfcraft_annot::page_refs(&doc).map_err(|e| SignError::Pdf(e.to_string()))?;
    let existing = opts
        .field
        .as_deref()
        .map(|n| sig_fields(&doc).into_iter().find(|f| f.name == n).ok_or_else(|| SignError::Pdf(format!("there is no signature field {n:?}"))));
    // `turn`: how far the appearance is turned to read upright (`/MK /R`, counterclockwise).
    let (widget, rect, turn) = match existing {
        Some(f) => {
            let f = f?;
            if f.signature(&doc).is_some() {
                return Err(SignError::Pdf(format!("the field {:?} is already signed", f.name)));
            }
            if f.has_foreign_value(&doc) {
                return Err(SignError::Pdf(format!("the value of the field {:?} is not a signature", f.name)));
            }
            let fr = f.r.ok_or_else(|| SignError::Pdf("the field is not an indirect object".into()))?;
            doc.update_dict(fr, |d| d.set(b"V".to_vec(), Object::Ref(sig)))?;
            let (wr, wd) = f.widgets.first().cloned().ok_or_else(|| SignError::Pdf("the field has no widget".into()))?;
            let rect = nums(&doc, &wd, b"Rect").filter(|r| r.len() == 4).map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
            // The field's own rotation, as a form author set it: an existing field is not
            // reoriented, so one without `/MK /R` is drawn as before.
            (wr.unwrap_or(fr), rect.unwrap_or([0.0; 4]), mk_rotation(&doc, &wd))
        }
        None => {
            let page = *pages.get(opts.page).ok_or_else(|| SignError::Pdf(format!("page {} does not exist", opts.page + 1)))?;
            let taken: HashSet<String> = sig_fields(&doc).into_iter().map(|f| f.name).collect();
            let fname = opts
                .new_field_name
                .clone()
                .unwrap_or_else(|| (1..).map(|i| format!("Signature{i}")).find(|n| !taken.contains(n)).unwrap_or_default());
            let rect = opts.rect.unwrap_or([0.0; 4]);
            let mut w = Dict::new();
            w.set(b"Type".to_vec(), Object::name("Annot"));
            w.set(b"Subtype".to_vec(), Object::name("Widget"));
            w.set(b"FT".to_vec(), Object::name("Sig"));
            w.set(b"T".to_vec(), PdfString::text(&fname));
            // The walk lists no field without a usable name, and counts a signed one as unread.
            if text(&doc, &w, b"T").is_none() {
                return Err(SignError::Pdf("a signature field needs a name that isn't blank".into()));
            }
            w.set(b"V".to_vec(), Object::Ref(sig));
            w.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|x| Object::Real(*x)).collect()));
            w.set(b"P".to_vec(), Object::Ref(page));
            // Print + Locked.
            w.set(b"F".to_vec(), Object::Int(4 | 128));
            // On a page shown turned, the signature reads upright as the page is displayed: the
            // widget is turned back by the page's rotation (ISO 32000-2 §12.5.6.19, `/MK /R`).
            let turn = pdfcraft_model::pages(&doc).get(opts.page).map_or(0, |p| p.rotation(&doc));
            if turn != 0 {
                let mut mk = Dict::new();
                mk.set(b"R".to_vec(), Object::Int(turn));
                w.set(b"MK".to_vec(), Object::Dict(mk));
            }
            let wr = doc.add(Object::Dict(w));
            // Page /Annots (indirect arrays are updated in place).
            let pd = doc.get(page).as_dict().cloned().unwrap_or_default();
            match pd.get(b"Annots") {
                Some(Object::Ref(ar)) => {
                    let mut a = doc.get(*ar).as_array().cloned().unwrap_or_default();
                    a.push(Object::Ref(wr));
                    doc.set(*ar, Object::Array(a));
                }
                other => {
                    let mut a = other.and_then(|o| o.as_array().cloned()).unwrap_or_default();
                    a.push(Object::Ref(wr));
                    doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(a)))?;
                }
            }
            // AcroForm /Fields and /SigFlags (signatures exist, append only).
            let catalog = doc.get(root).as_dict().cloned().unwrap_or_default();
            let (af_ref, mut af) = match catalog.get(b"AcroForm") {
                Some(Object::Ref(r)) => (Some(*r), doc.get(*r).as_dict().cloned().unwrap_or_default()),
                Some(Object::Dict(d)) => (None, d.clone()),
                _ => (None, Dict::new()),
            };
            let mut fields = match af.get(b"Fields") {
                Some(Object::Ref(r)) => doc.get(*r).as_array().cloned().unwrap_or_default(),
                Some(Object::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            fields.push(Object::Ref(wr));
            af.set(b"Fields".to_vec(), Object::Array(fields));
            af.set(b"SigFlags".to_vec(), Object::Int(3));
            match af_ref {
                Some(r) => doc.set(r, Object::Dict(af)),
                // Inline (or new) AcroForms stay in the catalog.
                None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(af)))?,
            }
            // The walk has to read the whole form, the new field included: past its caps the
            // signature would be listed as unread fields, and its name could repeat an unread one.
            if !walk_sig_fields(&doc).1 {
                return Err(SignError::Pdf(
                    "this form has too many fields, or a field name too long, for PdfCraft to read them all, so it can't add a signature field"
                        .into(),
                ));
            }
            (wr, rect, turn)
        }
    };
    if opts.field.is_some() {
        doc.update_dict(root, |c| {
            if let Some(Object::Dict(af)) = c.get_mut(b"AcroForm") {
                af.set(b"SigFlags".to_vec(), Object::Int(3));
            }
        })?;
        if let Some(Object::Ref(r)) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").cloned()) {
            doc.update_dict(r, |af| af.set(b"SigFlags".to_vec(), Object::Int(3)))?;
        }
    }
    // Acrobat does not recognise a signature in a form that asks viewers to redraw its fields
    // (`/NeedAppearances true`): redrawing would change what the signature covers. The first
    // signature drops the flag; a later one leaves it, as removing it would count as a change
    // to the earlier signatures. Without the flag viewers draw each field's stored `/AP /N`, so
    // a filled field whose stored appearance draws no text (an empty placeholder, or a stale
    // look) shows that instead of its value unless the caller drew the fields first (see `sign`).
    if !earlier_signatures {
        if let Some(Object::Ref(r)) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").cloned()) {
            doc.update_dict(r, |af| {
                af.remove(b"NeedAppearances");
            })?;
        }
        doc.update_dict(root, |c| {
            if let Some(Object::Dict(af)) = c.get_mut(b"AcroForm") {
                af.remove(b"NeedAppearances");
            }
        })?;
    }
    // The appearance.
    let ap = appearance(&mut doc, rect, turn, &name, &id.certificate, opts)?;
    let ap_ref = doc.add(Object::Stream(ap));
    let mut apd = Dict::new();
    apd.set(b"N".to_vec(), Object::Ref(ap_ref));
    doc.update_dict(widget, |w| w.set(b"AP".to_vec(), Object::Dict(apd)))?;
    // Write with the placeholders, then patch them. What the user removed again before signing
    // (a cleared redaction mark) stays out of the file, as in a save; what signing added is
    // reachable (the signature through the field's `/V`), and the placeholders are found in the
    // written bytes.
    let save = SaveOptions { mod_date: Some(opts.date.clone()), object_streams: false, drop_unlinked: true, ..SaveOptions::default() };
    let mut out = pdfcraft_cos::write_incremental(&doc, &save)?;
    let (br_at, gap) = locate(&out, reserve)?;
    let (start, end) = gap;
    let ranges = [0usize, start, end, out.len() - end];
    let mut br_text = format!("0 {} {} {}", ranges[1], ranges[2], ranges[3]).into_bytes();
    let width = format!("0 {} {} {}", BR_MARK[0], BR_MARK[1], BR_MARK[2]).len();
    if br_text.len() > width {
        return Err(SignError::Pdf("the document is too large to sign".into()));
    }
    br_text.resize(width, b' ');
    out[br_at..br_at + width].copy_from_slice(&br_text);
    let digest = alg.digest(&[&out[..start], &out[end..]]);
    let base = crate::cms::sign_detached(&id.key, &id.certificate, &id.chain, alg, &digest)?;
    let cms = match tsa {
        None => base,
        Some(t) => {
            // The imprint covers the signature value, which attaching an unsigned attribute
            // does not change (PAdES B-T).
            let sd = SignedData::parse(&base)?;
            let q = crate::timestamp::TimestampQuery::new(DigestAlg::Sha256, DigestAlg::Sha256.digest(&[&sd.signer.signature]))?;
            let resp = t.timestamp(&q.encode()?)?;
            let token = crate::timestamp::parse_response(&resp, &q)?;
            crate::cms::attach_timestamp_token(&base, &token.raw)?
        }
    };
    if cms.len() > reserve {
        return Err(SignError::Pdf("the signature does not fit its placeholder".into()));
    }
    let hex: Vec<u8> = cms.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    out[start + 1..start + 1 + hex.len()].copy_from_slice(&hex);
    Ok(out)
}

/// Append a document timestamp (ISO 32000-2 §12.8.2.2): a standalone signature dictionary
/// whose `/Contents` is an RFC 3161 token covering the whole current file (`/ETSI.RFC3161`).
/// The transport is the caller's; a rejected or malformed token produces no file.
pub fn timestamp_document(doc: &Document, tsa: &dyn crate::timestamp::TimestampAuthority, date: &str) -> Result<Vec<u8>, SignError> {
    // A pending security change would make the incremental save a full rewrite that applies
    // or removes protection as it signs (and breaks existing signatures): save it first.
    if doc.encryption_changed() {
        return Err(SignError::Pdf("save the document's new security settings before signing it".into()));
    }
    if doc.permissions().is_some_and(|p| !p.fill_forms()) {
        return Err(SignError::Pdf(
            "the document's security settings don't allow signing; open it with the permissions password to timestamp it".into(),
        ));
    }
    let mut doc = doc.clone();
    // What the user removed again before this (a cleared redaction mark) stays out of the file,
    // as in a save. Nothing refers to the timestamp either, so the leftovers go first and the
    // save keeps everything.
    doc.free_unlinked_additions();
    let mut v = Dict::new();
    v.set(b"Type".to_vec(), Object::name("DocTimeStamp"));
    v.set(b"Filter".to_vec(), Object::name("Adobe.PPKLite"));
    v.set(b"SubFilter".to_vec(), Object::name("ETSI.RFC3161"));
    v.set(b"ByteRange".to_vec(), Object::Array([0].iter().chain(BR_MARK.iter()).map(|n| Object::Int(*n)).collect()));
    v.set(b"Contents".to_vec(), Object::String(PdfString { bytes: vec![0; TOKEN_RESERVE], hex: true }));
    v.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    let mut app = Dict::new();
    app.set(b"Name".to_vec(), Object::name("PdfCraft"));
    let mut build = Dict::new();
    build.set(b"App".to_vec(), Object::Dict(app));
    v.set(b"Prop_Build".to_vec(), Object::Dict(build));
    doc.add(Object::Dict(v));
    let save = SaveOptions { mod_date: Some(date.to_string()), object_streams: false, ..SaveOptions::default() };
    let mut out = pdfcraft_cos::write_incremental(&doc, &save)?;
    let (br_at, gap) = locate(&out, TOKEN_RESERVE)?;
    let (start, end) = gap;
    let ranges = [0usize, start, end, out.len() - end];
    let mut br_text = format!("0 {} {} {}", ranges[1], ranges[2], ranges[3]).into_bytes();
    let width = format!("0 {} {} {}", BR_MARK[0], BR_MARK[1], BR_MARK[2]).len();
    if br_text.len() > width {
        return Err(SignError::Pdf("the document is too large to timestamp".into()));
    }
    br_text.resize(width, b' ');
    out[br_at..br_at + width].copy_from_slice(&br_text);
    let q = crate::timestamp::TimestampQuery::new(DigestAlg::Sha256, DigestAlg::Sha256.digest(&[&out[..start], &out[end..]]))?;
    let resp = tsa.timestamp(&q.encode()?)?;
    let token = crate::timestamp::parse_response(&resp, &q)?;
    let hex: Vec<u8> = token.raw.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    if hex.len() > TOKEN_RESERVE * 2 {
        return Err(SignError::Pdf("the timestamp token does not fit its placeholder".into()));
    }
    out[start + 1..start + 1 + hex.len()].copy_from_slice(&hex);
    Ok(out)
}

/// Find the placeholders in the written file: the offset of the ByteRange numbers, and the
/// `<…>` hex string's span. The ByteRange marker is unique; the Contents placeholder is the one
/// in the same object (a full save may renumber objects, so they are not found by number).
fn locate(out: &[u8], reserve: usize) -> Result<(usize, (usize, usize)), SignError> {
    let mark = format!("0 {} {} {}", BR_MARK[0], BR_MARK[1], BR_MARK[2]);
    let hits: Vec<usize> = out.windows(mark.len()).enumerate().filter(|(_, w)| *w == mark.as_bytes()).map(|(i, _)| i).collect();
    let [br] = hits.as_slice() else { return Err(SignError::Pdf("ByteRange placeholder not found".into())) };
    let start = out[..*br].windows(3).rposition(|w| w == b"obj").ok_or_else(|| SignError::Pdf("signature object not found".into()))?;
    let end = br + out[*br..].windows(6).position(|w| w == b"endobj").ok_or_else(|| SignError::Pdf("unterminated signature object".into()))?;
    let region = &out[start..end];
    let mut zeros = vec![b'<'];
    zeros.extend(std::iter::repeat_n(b'0', reserve * 2));
    zeros.push(b'>');
    let c = region.windows(zeros.len()).position(|w| w == zeros.as_slice()).ok_or_else(|| SignError::Pdf("Contents placeholder not found".into()))?;
    Ok((*br, (start + c, start + c + zeros.len())))
}

/// "2026.10.02 14:03:11 +02'00'" from a PDF date.
pub fn display_date(pdf: &str) -> String {
    let s = pdf.strip_prefix("D:").unwrap_or(pdf);
    let g = |r: std::ops::Range<usize>| s.get(r).unwrap_or("00");
    let tz = match s.get(14..).filter(|z| !z.is_empty()) {
        Some(z) if z.starts_with('Z') => " Z".to_string(),
        Some(z) => format!(" {z}"),
        None => String::new(),
    };
    format!("{}.{}.{} {}:{}:{}{tz}", g(0..4), g(4..6), g(6..8), g(8..10), g(10..12), g(12..14))
}

type ClusterKey = (String, Vec<(u32, [u64; 2])>, u64);

/// Identify an Arabic cluster by its text, glyph ids, offsets and advance for font-code reuse.
fn cluster_key(c: &pdfcraft_fonts::ShapedCluster) -> ClusterKey {
    (c.text.clone(), c.glyphs.iter().map(|(id, [x, y])| (*id, [x.to_bits(), y.to_bits()])).collect(), c.advance.to_bits())
}

/// Add an outlined Arabic Type 3 font and return its reference and codes in drawing order.
/// Includes ToUnicode text; refuses more than 240 unique clusters or unavailable glyph outlines.
fn type3_arabic(doc: &mut Document, clusters: &[pdfcraft_fonts::ShapedCluster]) -> Result<(ObjRef, Vec<u8>), SignError> {
    let mut charprocs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::with_capacity(clusters.len());
    let mut bbox = [0.0f64; 4];
    let mut to_unicode = Vec::new();
    let mut unique_clusters: Vec<pdfcraft_fonts::ShapedCluster> = Vec::new();
    let mut cluster_to_code = std::collections::HashMap::new();
    let mut codes = Vec::with_capacity(clusters.len());

    for c in clusters {
        let key = cluster_key(c);
        let code = match cluster_to_code.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => *e.get(),
            std::collections::hash_map::Entry::Vacant(e) => {
                if unique_clusters.len() >= 240 {
                    return Err(SignError::Pdf("too many Arabic characters in signature appearance".into()));
                }
                unique_clusters.push(c.clone());
                let code = u8::try_from(unique_clusters.len()).map_err(|_| SignError::Pdf("character code overflow".into()))?;
                e.insert(code);
                code
            }
        };
        codes.push(code);
    }

    for (i, c) in unique_clusters.iter().enumerate() {
        let code = i + 1;
        let mut paths = Vec::new();
        let mut ink: Option<[f64; 4]> = None;
        for (id, [dx, dy]) in &c.glyphs {
            let g = pdfcraft_fonts::arabic_glyph(*id).map_err(|e| SignError::Pdf(format!("missing Arabic glyph {id}: {e:?}")))?;
            for contour in &g.contours {
                for (j, p) in contour.iter().enumerate() {
                    paths.extend(format!("{} {} {}\n", fmt((p[0] + dx) * 1000.0), fmt((p[1] + dy) * 1000.0), if j == 0 { "m" } else { "l" }).bytes());
                }
                paths.extend_from_slice(b"h\n");
                let b = [g.bbox[0] + dx, g.bbox[1] + dy, g.bbox[2] + dx, g.bbox[3] + dy];
                ink = Some(ink.map_or(b, |a| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]));
            }
        }
        let [x0, y0, x1, y1] =
            ink.map_or([0.0; 4], |b| [(b[0] * 1000.0).floor(), (b[1] * 1000.0).floor(), (b[2] * 1000.0).ceil(), (b[3] * 1000.0).ceil()]);
        bbox = [bbox[0].min(x0), bbox[1].min(y0), bbox[2].max(x1), bbox[3].max(y1)];
        let mut proc = format!("{} 0 {} {} {} {} d1\n", fmt(c.advance * 1000.0), fmt(x0), fmt(y0), fmt(x1), fmt(y1)).into_bytes();
        if !paths.is_empty() {
            proc.extend(paths);
            proc.extend_from_slice(b"f\n");
        }
        let glyph = format!("g{code:02X}");
        charprocs.set(glyph.clone().into_bytes(), Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), &proc)))));
        differences.push(Object::name(&glyph));
        widths.push(Object::Real((c.advance * 1000.0).round()));
        if !c.text.is_empty() {
            let hex: String = c.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
            to_unicode.push(format!("<{code:02X}> <{hex}>\n"));
        }
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /PdfCraftSignature def\n/CIDSystemInfo << /Registry (PdfCraft) /Ordering (Signature) /Supplement 0 >> def\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    for block in to_unicode.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n{}endbfchar\n", block.len(), block.concat()));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let cmap_ref = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));

    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name("PCFull0"));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(b"Noto Sans Arabic".to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));

    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));

    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
    font.set(b"FontBBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(unique_clusters.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap_ref));
    let font_ref = doc.add(Object::Dict(font));
    Ok((font_ref, codes))
}

enum SignatureAtom {
    Arabic(pdfcraft_fonts::ShapedCluster),
    Latin(char, f64),
}

impl SignatureAtom {
    /// Return the advance in em units using the same Arabic or Latin fallback face as drawing.
    fn advance(&self) -> f64 {
        match self {
            Self::Arabic(c) => c.advance,
            Self::Latin(_, advance) => *advance,
        }
    }
}

/// Recognize direction controls that affect bidi ordering but must not produce visible glyphs.
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Shape directional runs, not the whole mixed-script row. Latin keeps its own font.
fn signature_atoms(
    text: &str,
    base: unicode_bidi::Level,
    advances: &mut std::collections::HashMap<char, f64>,
) -> Result<Vec<SignatureAtom>, SignError> {
    #[cfg(test)]
    arabic_tests::ATOM_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
    if text.is_empty() {
        // visual_runs requires a nonempty range.
        return Ok(Vec::new());
    }
    let bidi = unicode_bidi::ParagraphBidiInfo::new(text, Some(base));
    let (levels, runs) = bidi.visual_runs(0..text.len());
    let mut atoms = Vec::new();
    for run in runs {
        let rtl = levels.get(run.start).is_some_and(unicode_bidi::Level::is_rtl);
        let Some(text) = text.get(run) else { continue };
        let mut segments: Vec<(bool, String)> = Vec::new();
        for c in text.chars().filter(|c| !bidi_control(*c)) {
            let arabic = is_arabic(c) || (rtl && !c.is_alphanumeric() && pdfcraft_fonts::arabic_has(c));
            match segments.last_mut() {
                Some((kind, s)) if *kind == arabic => s.push(c),
                _ => segments.push((arabic, c.to_string())),
            }
        }
        if rtl {
            segments.reverse();
        }
        for (arabic, text) in segments {
            if arabic {
                atoms.extend(
                    pdfcraft_fonts::shape_arabic(&text, rtl)
                        .map_err(|e| SignError::Pdf(format!("failed to shape Arabic: {e:?}")))?
                        .into_iter()
                        .map(SignatureAtom::Arabic),
                );
            } else {
                atoms.extend(text.chars().map(|c| {
                    let advance = *advances.entry(c).or_insert_with(|| {
                        let mut buf = [0; 4];
                        let text = c.encode_utf8(&mut buf);
                        if pdfcraft_fonts::embedding::covers(pdfcraft_fonts::INTER_REGULAR, text) {
                            pdfcraft_fonts::inter_width(text, 1.0)
                        } else {
                            pdfcraft_fonts::helvetica_width(text, 1.0)
                        }
                    });
                    SignatureAtom::Latin(c, advance)
                }));
            }
        }
    }
    Ok(atoms)
}

struct SignatureLine {
    atoms: Vec<SignatureAtom>,
    rtl: bool,
}

impl SignatureLine {
    /// Measured advance at one point, shared by fitting and drawing.
    fn width(&self) -> f64 {
        self.atoms.iter().map(SignatureAtom::advance).sum()
    }
}

/// Per-appearance shaping cache. Both retained candidates and cumulative shaping work are
/// bounded: adversarial long words must not turn a small field into quadratic memory or CPU.
#[derive(Default)]
struct SignatureLayout {
    candidates: std::collections::HashMap<(String, bool), std::sync::Arc<SignatureLine>>,
    advances: std::collections::HashMap<char, f64>,
    cached_bytes: usize,
    shaped_bytes: usize,
}

/// Tell callers how to recover rather than silently clipping or drawing microscopic text.
fn appearance_fit_error() -> SignError {
    SignError::Unsupported("visible signature text does not fit at 6 pt; enlarge the signature box, shorten the text, or hide a detail".into())
}

impl SignatureLayout {
    /// Reuse a size-independent visual candidate, retaining at most 4 MiB of estimated data.
    fn candidate(&mut self, text: &str, base: unicode_bidi::Level) -> Result<std::sync::Arc<SignatureLine>, SignError> {
        let key = (text.to_owned(), base.is_rtl());
        if let Some(line) = self.candidates.get(&key) {
            return Ok(line.clone());
        }
        self.shaped_bytes = self.shaped_bytes.saturating_add(text.len());
        if self.shaped_bytes > 2_000_000 {
            return Err(SignError::Unsupported("visible signature layout is too complex; shorten the text or hide a detail".into()));
        }
        let line = std::sync::Arc::new(SignatureLine { atoms: signature_atoms(text, base, &mut self.advances)?, rtl: base.is_rtl() });
        let bytes = line.atoms.iter().fold(text.len().saturating_add(128), |total, atom| {
            total.saturating_add(64).saturating_add(match atom {
                SignatureAtom::Arabic(c) => c.text.len().saturating_add(c.glyphs.len().saturating_mul(32)),
                SignatureAtom::Latin(..) => 0,
            })
        });
        if self.cached_bytes.saturating_add(bytes) <= 4 * 1024 * 1024 {
            self.cached_bytes = self.cached_bytes.saturating_add(bytes);
            self.candidates.insert(key, line.clone());
        }
        Ok(line)
    }

    /// Fit at no less than 6 pt. Unwrapped text uses an analytic scale; wrapped text
    /// uses five bounded refinements, sharing candidates across every attempted size.
    fn fit(
        &mut self,
        text: &[String],
        max_size: f64,
        width: f64,
        height: f64,
        leading: f64,
    ) -> Result<(f64, Vec<std::sync::Arc<SignatureLine>>), SignError> {
        const MIN_SIZE: f64 = 6.0;
        if text.is_empty() {
            return Ok((MIN_SIZE, Vec::new()));
        }
        if !width.is_finite() || width <= 0.0 || !height.is_finite() || height <= 0.0 || !max_size.is_finite() || max_size < MIN_SIZE {
            return Err(appearance_fit_error());
        }
        let unbounded = text.iter().map(|s| self.lines(s, 1.0, f64::MAX)).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect::<Vec<_>>();
        let longest = unbounded.iter().map(|line| line.width()).fold(0.01, f64::max);
        let size = max_size.min(width / longest).min(height / (unbounded.len().max(1) as f64 * leading));
        if size >= MIN_SIZE {
            return Ok((size, unbounded));
        }
        let mut layout = |size| -> Result<Vec<std::sync::Arc<SignatureLine>>, SignError> {
            let lines = text.iter().map(|s| self.lines(s, size, width)).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect::<Vec<_>>();
            if lines.len() as f64 * size * leading > height {
                return Err(appearance_fit_error());
            }
            Ok(lines)
        };
        let mut best = layout(MIN_SIZE)?;
        let mut low = MIN_SIZE;
        let mut high = max_size;
        for _ in 0..5 {
            let mid = (low + high) / 2.0;
            match layout(mid) {
                Ok(lines) => {
                    low = mid;
                    best = lines;
                }
                Err(SignError::Unsupported(message)) if message.starts_with("visible signature text does not fit") => high = mid,
                Err(error) => return Err(error),
            }
        }
        Ok((low, best))
    }
}

/// Break logical text before applying bidi and shaping each final line. In particular the
/// first RTL line must contain the beginning, not the end, of the logical paragraph.
/// `f64::MAX` measures without wrapping, shaping each paragraph once. Finite widths measure
/// complete candidates and split oversized words at grapheme boundaries. Refuses text over
/// 16,384 UTF-8 bytes and propagates shaping errors.
#[cfg(test)]
fn signature_lines(text: &str, size: f64, width: f64) -> Result<Vec<std::sync::Arc<SignatureLine>>, SignError> {
    SignatureLayout::default().lines(text, size, width)
}

impl SignatureLayout {
    /// Wrap logical paragraphs using cached, complete visual candidates. Refuse a single
    /// grapheme wider than the box; tabs and breakable Unicode spaces behave as spaces.
    fn lines(&mut self, text: &str, size: f64, width: f64) -> Result<Vec<std::sync::Arc<SignatureLine>>, SignError> {
        use unicode_segmentation::UnicodeSegmentation;
        // Bound shaping, layout retries, and the generated appearance even for hostile certificate
        // subjects and script arguments. Labels are included by the caller in `text`.
        if text.len() > 16_384 {
            return Err(SignError::Unsupported(
                "visible signature text is limited to 16384 UTF-8 bytes per row; shorten it or hide that detail".into(),
            ));
        }
        if !size.is_finite() || size <= 0.0 || !width.is_finite() || width <= 0.0 {
            return Err(appearance_fit_error());
        }
        let mut lines = Vec::new();
        for para in text.replace("\r\n", "\n").split(['\r', '\n']) {
            let para: String = para.chars().map(|c| if c.is_whitespace() && !matches!(c, '\u{00a0}' | '\u{202f}') { ' ' } else { c }).collect();
            let bidi = unicode_bidi::ParagraphBidiInfo::new(&para, None);
            let base = bidi.paragraph_level;
            if width == f64::MAX {
                // The name-width pass has no wrapping limit. Preserve the empty-line rule that
                // drops leading spaces, but shape the final paragraph only once.
                lines.push(self.candidate(para.trim_start_matches(' '), base)?);
                continue;
            }
            let mut line = String::new();
            let mut measured = None;
            for word in para.split(' ') {
                // Font selection for spaces depends on the complete bidi run, and joining can
                // change Arabic advances. Measure exactly the candidate that would be drawn.
                let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                let shaped = self.candidate(&candidate, base)?;
                if shaped.width() * size <= width {
                    line = candidate;
                    measured = Some(shaped);
                    continue;
                }
                if !line.is_empty() {
                    if let Some(shaped) = measured.take() {
                        lines.push(shaped);
                    }
                    line.clear();
                }
                let shaped = self.candidate(word, base)?;
                if shaped.width() * size <= width {
                    line.push_str(word);
                    measured = Some(shaped);
                } else {
                    // Never detach vowel marks from a letter when breaking a long word.
                    for grapheme in word.graphemes(true) {
                        let candidate = format!("{line}{grapheme}");
                        let mut shaped = self.candidate(&candidate, base)?;
                        if !line.is_empty() && shaped.width() * size > width {
                            if let Some(shaped) = measured.take() {
                                lines.push(shaped);
                            }
                            line.clear();
                            shaped = self.candidate(grapheme, base)?;
                        }
                        if shaped.width() * size > width {
                            return Err(appearance_fit_error());
                        }
                        line.push_str(grapheme);
                        measured = Some(shaped);
                    }
                }
            }
            lines.push(match measured {
                Some(shaped) => shaped,
                None => self.candidate("", base)?,
            });
        }
        Ok(lines)
    }
}

/// Recognize the Arabic Unicode blocks supported by the signature appearance dispatcher.
fn is_arabic(c: char) -> bool {
    matches!(u32::from(c), 0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// Whether visible text contains a character from the Arabic blocks requiring Arabic rendering.
fn contains_arabic(text: &str) -> bool {
    text.chars().any(is_arabic)
}

/// Validate displayed fields and labels, returning Arabic and Latin-Extended font requirements.
/// Invisible boxes skip validation; missing fonts, unsupported characters and oversized Arabic
/// appearance fields return actionable errors before drawing.
fn check_appearance_text(name: &str, dn: &str, opts: &SignOptions, w: f64, h: f64) -> Result<(bool, bool), SignError> {
    use pdfcraft_fonts::{first_undrawable, undrawable_message};
    if w <= 1.0 || h <= 1.0 {
        return Ok((false, false));
    }
    let a = &opts.appearance;
    let mut shown = vec![
        (a.name, "the signer's name", Some(name)),
        (a.distinguished_name, "the certificate's subject", Some(dn)),
        (a.reason, "the reason", opts.reason.as_deref()),
        (a.location, "the location", opts.location.as_deref()),
    ];
    let has = |v: Option<&str>| v.is_some_and(|v| !v.is_empty());
    let drawn = [a.name, a.distinguished_name, a.reason && has(opts.reason.as_deref()), a.location && has(opts.location.as_deref()), a.date];
    let labels: Vec<_> = opts
        .appearance_labels
        .fields()
        .into_iter()
        .zip(drawn)
        .filter(|&(_, on)| a.labels && on)
        .map(|((field, text), _)| (true, field, Some(text)))
        .collect();
    let mut needs_arabic = false;
    let mut needs_unicode = false;
    for &(on, _, text) in shown.iter().chain(&labels) {
        if on && let Some(s) = text {
            if contains_arabic(s) {
                needs_arabic = true;
            } else if first_undrawable(s).is_some() {
                needs_unicode = true;
            }
        }
    }
    if needs_arabic {
        shown.extend(labels.iter().copied());
        let arabic_face = pdfcraft_fonts::document_arabic_font().ok_or_else(|| {
            SignError::Unsupported(
                "this text needs PdfCraft's Arabic font (Noto Sans Arabic), which this build doesn't include; use a release that includes the Arabic font, or sign without a visible appearance".into(),
            )
        })?;
        let mut coverage = std::collections::HashMap::new();
        for &(on, what, text) in &shown {
            if on && let Some(s) = text {
                if s.len() > 16_384 {
                    return Err(SignError::Unsupported(format!(
                        "{what} of the visible signature exceeds 16384 UTF-8 bytes; shorten it or leave it out of the appearance"
                    )));
                }
                for c in s.chars() {
                    if c.is_whitespace() || bidi_control(c) {
                        continue;
                    }
                    let covered = *coverage.entry(c).or_insert_with(|| {
                        let mut buf = [0; 4];
                        let text = c.encode_utf8(&mut buf);
                        pdfcraft_fonts::embedding::covers(arabic_face.bytes, text)
                            || pdfcraft_fonts::embedding::covers(pdfcraft_fonts::INTER_REGULAR, text)
                            || pdfcraft_fonts::win_ansi_encodable(c)
                    });
                    if !covered {
                        return Err(SignError::Unsupported(format!(
                            "{}; sign without a visible appearance, or leave {what} out of it",
                            undrawable_message(&format!("{what} of the visible signature"), c)
                        )));
                    }
                }
            }
        }
    } else if needs_unicode {
        let mut coverage = std::collections::HashMap::new();
        for &(on, what, text) in &shown {
            if on
                && let Some(s) = text
                && first_undrawable(s).is_some()
            {
                for c in s.chars() {
                    if matches!(c, '\n' | '\r') {
                        continue;
                    }
                    if !*coverage.entry(c).or_insert_with(|| {
                        let mut buf = [0; 4];
                        pdfcraft_fonts::embedding::covers(pdfcraft_fonts::INTER_REGULAR, c.encode_utf8(&mut buf))
                    }) {
                        return Err(SignError::Unsupported(format!(
                            "{}; sign without a visible appearance, or leave {what} out of it",
                            undrawable_message(&format!("{what} of the visible signature"), c)
                        )));
                    }
                }
            }
        }
    }
    if !needs_arabic && let Some((field, c)) = labels.iter().find_map(|(_, field, text)| text.and_then(first_undrawable).map(|c| (field, c))) {
        return Err(SignError::Unsupported(format!(
            "{}; pass that label in Western European characters, or leave the labels out of the visible signature",
            undrawable_message(&format!("the {field} label of the visible signature"), c)
        )));
    }
    Ok((needs_arabic, needs_unicode))
}

struct AppearanceBox {
    h: f64,
    pad: f64,
    left_w: f64,
    right_x: f64,
    right_w: f64,
}

struct SignatureFonts {
    latin_tag: String,
    latin: pdfcraft_fonts::pdf::Metrics,
}

impl SignatureFonts {
    /// Append one visual line at baseline `y`, aligned within `[left, width, padding]`.
    /// Consumes Arabic codes in drawing order or uses Latin fallback fonts; refuses overflow.
    fn draw_line(
        &self,
        out: &mut Vec<u8>,
        line: &SignatureLine,
        codes: &mut std::slice::Iter<'_, u8>,
        size: f64,
        rect: [f64; 3],
        y: f64,
    ) -> Result<(), SignError> {
        let [left, width, pad] = rect;
        let line_w = line.width() * size;
        if !line_w.is_finite() || line_w > width + 0.000_001 {
            return Err(appearance_fit_error());
        }
        let mut pen = if line.rtl { left + width - line_w } else { left } + pad;
        for atom in &line.atoms {
            let (tag, encoded) = match atom {
                SignatureAtom::Arabic(_) => {
                    let code = codes.next().ok_or_else(|| SignError::Pdf("signature glyph is not in its font".into()))?;
                    ("PCFull0", format!("<{code:02X}>"))
                }
                SignatureAtom::Latin(c, _) => {
                    let text = c.to_string();
                    let (tag, bytes) = if let Some(bytes) = self.latin.encode(&text) {
                        (self.latin_tag.as_str(), bytes)
                    } else if let Some(byte) = pdfcraft_fonts::win_ansi_byte(*c) {
                        ("Helv", vec![byte])
                    } else {
                        return Err(SignError::Unsupported(format!("the visible signature font cannot draw {c:?}")));
                    };
                    (tag, format!("<{}>", bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()))
                }
            };
            out.extend(format!("/{tag} {} Tf 1 0 0 1 {} {} Tm {encoded} Tj\n", fmt(size), fmt(pen), fmt(y)).bytes());
            pen += atom.advance() * size;
        }
        Ok(())
    }
}

/// Lay out and draw the visible name/graphic and details with bidi-aware Arabic and Latin fonts.
/// Returns PDF drawing operators, font resources and an optional image reference, propagating
/// layout, font and graphic errors without publishing a partially signed document.
fn draw_arabic_appearance(
    doc: &mut Document,
    b: &AppearanceBox,
    name: &str,
    lines: &[String],
    opts: &SignOptions,
) -> Result<(Vec<u8>, Dict, Option<ObjRef>), SignError> {
    let a = &opts.appearance;
    let mut out = Vec::new();
    let mut fonts = Dict::new();
    let mut image = None;

    let mut layout = SignatureLayout::default();
    let fit = b.left_w - 2.0 * b.pad;
    let height = b.h - 2.0 * b.pad;
    let shown_name = if a.name && opts.graphic.is_none() { name } else { "" };
    let (name_size, name_lines) =
        if shown_name.is_empty() { (6.0, Vec::new()) } else { layout.fit(&[shown_name.to_owned()], (b.h * 0.4).min(36.0), fit, height, 1.1)? };
    // right_w already excludes the outer right padding; unlike the name column it
    // starts at right_x, so inset its left edge as well without extending the right.
    let detail_width = b.right_w - b.pad;
    let (detail_size, wrapped_detail_lines) = layout.fit(lines, 12.0, detail_width, height, 1.15)?;

    let mut all_clusters = Vec::new();
    for line in name_lines.iter().chain(&wrapped_detail_lines) {
        all_clusters.extend(line.atoms.iter().filter_map(|atom| match atom {
            SignatureAtom::Arabic(c) => Some(c.clone()),
            SignatureAtom::Latin(..) => None,
        }));
    }

    let (font_ref, codes) = type3_arabic(doc, &all_clusters)?;
    let mut codes = codes.iter();
    fonts.set(b"PCFull0".to_vec(), Object::Ref(font_ref));
    let latin_tag = pdfcraft_fonts::embedding::embed(doc, &mut fonts, pdfcraft_fonts::INTER_REGULAR, "Inter")
        .map_err(|e| SignError::Pdf(format!("failed to embed signature font: {e}")))?;
    let latin_dict = fonts
        .get(latin_tag.as_bytes())
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().cloned())
        .ok_or_else(|| SignError::Pdf("embedded signature font dictionary not found".into()))?;
    let font_data = SignatureFonts { latin_tag, latin: pdfcraft_fonts::pdf::Metrics::from_dict(doc, &latin_dict) };
    let mut helvetica = Dict::new();
    helvetica.set(b"Type".to_vec(), Object::name("Font"));
    helvetica.set(b"Subtype".to_vec(), Object::name("Type1"));
    helvetica.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    helvetica.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    fonts.set(b"Helv".to_vec(), Object::Dict(helvetica));

    if let Some(g) = &opts.graphic {
        let (ops, im) = g.draw(doc, [b.pad, b.pad, (b.left_w - 2.0 * b.pad).max(0.0), (b.h - 2.0 * b.pad).max(0.0)])?;
        out.extend(ops.bytes());
        image = im;
    }
    out.extend_from_slice(b"BT\n0 g\n");
    if a.name && opts.graphic.is_none() {
        let total = name_lines.len() as f64 * name_size * 1.1;
        let mut y = (b.h + total) / 2.0 - name_size * 0.85;
        for line in &name_lines {
            font_data.draw_line(&mut out, line, &mut codes, name_size, [0.0, fit, b.pad], y)?;
            y -= name_size * 1.1;
        }
    }
    let mut y = b.h - b.pad - detail_size * 0.9;
    for line in &wrapped_detail_lines {
        font_data.draw_line(&mut out, line, &mut codes, detail_size, [b.right_x, detail_width, b.pad], y)?;
        y -= detail_size * 1.15;
    }
    out.extend_from_slice(b"ET\n");
    Ok((out, fonts, image))
}

/// Draw a non-Arabic signature's name/graphic and details with Helvetica or embedded Inter.
/// Returns drawing operators, font resources and an optional image reference.
fn draw_standard_appearance(
    doc: &mut Document,
    b: &AppearanceBox,
    name: &str,
    lines: &[String],
    needs_unicode: bool,
    opts: &SignOptions,
) -> Result<(Vec<u8>, Dict, Option<ObjRef>), SignError> {
    use pdfcraft_fonts::{helvetica_width, inter_width, literal, win_ansi, wrap, wrap_with};
    let a = &opts.appearance;
    let mut out = Vec::new();
    let mut fonts = Dict::new();
    let mut image = None;

    let wrap_text =
        |s: &str, sz: f64, max_w: f64| -> Vec<String> { if needs_unicode { wrap_with(s, sz, max_w, inter_width) } else { wrap(s, sz, max_w) } };

    let (font_tag, metrics) = if needs_unicode {
        let tag = pdfcraft_fonts::embedding::embed(doc, &mut fonts, pdfcraft_fonts::INTER_REGULAR, "Inter")
            .map_err(|e| SignError::Pdf(format!("failed to embed signature font: {e}")))?;
        let font_dict = fonts
            .get(tag.as_bytes())
            .map(|o| doc.resolve(o))
            .and_then(|o| o.as_dict().cloned())
            .ok_or_else(|| SignError::Pdf("embedded signature font dictionary not found".into()))?;
        let m = pdfcraft_fonts::pdf::Metrics::from_dict(doc, &font_dict);
        (tag, Some(m))
    } else {
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type1"));
        font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
        font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        fonts.set(b"Helv".to_vec(), Object::Dict(font));
        ("Helv".to_string(), None)
    };
    let encode_line =
        |l: &str| -> Vec<u8> { if let Some(m) = &metrics { literal(&m.encode(l).unwrap_or_else(|| win_ansi(l))) } else { literal(&win_ansi(l)) } };

    if let Some(g) = &opts.graphic {
        let (ops, im) = g.draw(doc, [b.pad, b.pad, (b.left_w - 2.0 * b.pad).max(0.0), (b.h - 2.0 * b.pad).max(0.0)])?;
        out.extend(ops.bytes());
        image = im;
    }
    out.extend_from_slice(b"BT\n0 g\n");
    if a.name && opts.graphic.is_none() {
        let mut size = (b.h * 0.4).min(36.0);
        let fit = (b.left_w - 2.0 * b.pad).max(1.0);
        let longest = (if needs_unicode { inter_width(name, 1.0) } else { helvetica_width(name, 1.0) }).max(0.01);
        size = size.min(fit / longest).max(4.0);
        let words = wrap_text(name, size, fit);
        let total = words.len() as f64 * size * 1.1;
        let mut y = (b.h + total) / 2.0 - size * 0.85;
        for l in &words {
            out.extend(format!("/{font_tag} {} Tf 1 0 0 1 {} {} Tm ", fmt(size), fmt(b.pad), fmt(y)).bytes());
            out.extend(encode_line(l));
            out.extend_from_slice(b" Tj\n");
            y -= size * 1.1;
        }
    }
    let mut size = (b.h / (lines.len().max(1) as f64 * 1.15)).min(12.0);
    let wrapped = loop {
        let all: Vec<String> = lines.iter().flat_map(|l| wrap_text(l, size, b.right_w)).collect();
        if all.len() as f64 * size * 1.15 <= b.h - 2.0 * b.pad || size <= 3.0 {
            break all;
        }
        size *= 0.9;
    };
    let mut y = b.h - b.pad - size * 0.9;
    for l in &wrapped {
        out.extend(format!("/{font_tag} {} Tf 1 0 0 1 {} {} Tm ", fmt(size), fmt(b.right_x), fmt(y)).bytes());
        out.extend(encode_line(l));
        out.extend_from_slice(b" Tj\n");
        y -= size * 1.15;
    }
    out.extend_from_slice(b"ET\n");
    Ok((out, fonts, image))
}

/// The visible signature: the signer's name large on the left, or the graphic in its place, and
/// the details on the right (Acrobat's standard layout), in Helvetica, or in the bundled Inter
/// font when a detail needs characters beyond Western European, or in Noto Sans Arabic from
/// craft-fonts when Arabic characters are present. A graphic with no details beside it fills the
/// box. An error, before anything is drawn, when a detail it writes (the signer's name, the
/// certificate's subject, the reason or the location) has a character none of the fonts can draw:
/// signing invisibly, or leaving that part out, still works. A graphic doesn't change this: the
/// name it replaces on the left is still among the details. A label that is drawn
/// ([`AppearanceLabels`], the caller's words) must be drawable in Helvetica, and is refused otherwise.
///
/// `turn` is the widget's `/MK /R` (0, 90, 180 or 270; anything else counts as 0): the layout is
/// drawn upright in the box as turned by it, and a `/Matrix` maps that box onto `rect`.
fn appearance(doc: &mut Document, rect: [f64; 4], turn: i64, name: &str, cert: &Certificate, opts: &SignOptions) -> Result<Stream, SignError> {
    let turn = if matches!(turn, 90 | 180 | 270) { turn } else { 0 };
    let (rw, rh) = ((rect[2] - rect[0]).max(0.0), (rect[3] - rect[1]).max(0.0));
    // The upright layout box: a quarter turn swaps its sides.
    let (w, h) = if turn % 180 == 0 { (rw, rh) } else { (rh, rw) };
    let a = &opts.appearance;
    let dn = cert.subject.display();
    let (needs_arabic, needs_unicode) = check_appearance_text(name, &dn, opts, w, h)?;

    let mut lines: Vec<String> = Vec::new();
    let label = |l: &str, v: &str| if a.labels { format!("{l}{v}") } else { v.to_string() };
    // The labels are the caller's words, so a signature signed in another language reads in it.
    let wording = &opts.appearance_labels;
    if a.name {
        lines.push(if a.labels { wording.signed_by.clone() } else { String::new() });
        lines.push(name.to_string());
    }
    if a.distinguished_name {
        lines.push(label(&wording.distinguished_name, &dn));
    }
    if a.reason
        && let Some(r) = opts.reason.as_deref().filter(|r| !r.is_empty())
    {
        lines.push(label(&wording.reason, r));
    }
    if a.location
        && let Some(l) = opts.location.as_deref().filter(|l| !l.is_empty())
    {
        lines.push(label(&wording.location, l));
    }
    if a.date {
        lines.push(label(&wording.date, &display_date(&opts.date)));
    }
    lines.retain(|l| !l.is_empty());

    let (out, fonts, image) = if w > 1.0 && h > 1.0 {
        let pad = (h * 0.06).clamp(1.0, 6.0);
        let (left_w, right_x) = match (&opts.graphic, a.name) {
            (Some(_), _) if lines.is_empty() => (w, pad),
            (Some(_), _) | (None, true) => (w * 0.5, w * 0.5 + pad),
            (None, false) => (0.0, pad),
        };
        let right_w = (w - right_x - pad).max(1.0);
        let ab = AppearanceBox { h, pad, left_w, right_x, right_w };

        if needs_arabic {
            draw_arabic_appearance(doc, &ab, name, &lines, opts)?
        } else {
            draw_standard_appearance(doc, &ab, name, &lines, needs_unicode, opts)?
        }
    } else {
        (Vec::new(), Dict::new(), None)
    };

    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array([0.0, 0.0, w, h].iter().map(|x| Object::Real(*x)).collect()));
    if turn != 0 {
        // Viewers fit the box, transformed by `/Matrix`, to the widget's `/Rect` (ISO 32000-2
        // §12.5.5); this one lands on `rect` exactly, turned as `/MK /R` says.
        let mut m = pdfcraft_model::view_matrix_for(turn, [rect[0], rect[1], rect[0] + rw, rect[1] + rh]);
        if !m.iter().all(|x| x.is_finite()) {
            // A caller's non-finite corner: the turn alone, which viewers fit to `/Rect` the same.
            m = pdfcraft_model::view_matrix_for(turn, [0.0; 4]);
        }
        d.set(b"Matrix".to_vec(), Object::Array(m.iter().map(|x| Object::Real(*x)).collect()));
    }
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    if let Some(im) = image {
        let mut xobjects = Dict::new();
        xobjects.set(b"Im1".to_vec(), Object::Ref(im));
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
    }
    d.set(b"Resources".to_vec(), Object::Dict(res));
    Ok(Stream::flate(d, &out))
}

pub(crate) fn fmt(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod arabic_tests {
    use super::*;

    thread_local! {
        pub(super) static ATOM_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// Recover the logical text of each atom in visual order for layout assertions.
    fn visual_text(line: &SignatureLine) -> String {
        line.atoms
            .iter()
            .map(|a| match a {
                SignatureAtom::Arabic(c) => c.text.clone(),
                SignatureAtom::Latin(c, _) => c.to_string(),
            })
            .collect()
    }

    /// A grapheme that cannot fit is an error, not permission to draw outside the box.
    #[test]
    fn overwide_graphemes_are_refused() {
        assert!(signature_lines("W", 12.0, 1.0).is_err());
    }

    /// Tabs and Unicode spacing are wrapping opportunities, not visible fallback glyphs.
    #[test]
    fn whitespace_wraps_without_drawing_controls() {
        let lines = signature_lines("a\tb\u{2003}c", 12.0, 8.0).unwrap();
        assert_eq!(lines.iter().map(|line| visual_text(line)).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    /// Completed lines reuse their measured atoms rather than shaping them again.
    #[test]
    fn finite_line_reuses_candidate_shaping() {
        ATOM_CALLS.with(|calls| calls.set(0));
        assert_eq!(signature_lines("Smith", 12.0, 1000.0).unwrap().len(), 1);
        assert_eq!(ATOM_CALLS.with(std::cell::Cell::get), 1);
    }

    /// The same candidate at another size reuses atoms and cached Latin advances.
    #[test]
    fn size_retries_share_bounded_candidates() {
        let mut layout = SignatureLayout::default();
        ATOM_CALLS.with(|calls| calls.set(0));
        let first = layout.lines("Smith Smith", 12.0, 1000.0).unwrap();
        let calls = ATOM_CALLS.with(std::cell::Cell::get);
        let second = layout.lines("Smith Smith", 6.0, 1000.0).unwrap();
        assert_eq!(ATOM_CALLS.with(std::cell::Cell::get), calls);
        assert!(std::sync::Arc::ptr_eq(&first[0], &second[0]));
        assert_eq!(layout.advances.len(), 6);
        assert!(layout.cached_bytes <= 4 * 1024 * 1024);
        layout.shaped_bytes = 2_000_000;
        assert!(layout.lines("uncached text", 6.0, 1000.0).is_err());
    }

    /// The drawer refuses over-wide lines in either direction before emitting operators.
    #[test]
    fn drawing_rejects_rtl_and_ltr_overflow() {
        let fonts = SignatureFonts { latin_tag: "Helv".into(), latin: pdfcraft_fonts::pdf::Metrics::fallback() };
        for rtl in [false, true] {
            let line = SignatureLine { atoms: vec![SignatureAtom::Latin('W', 1.0)], rtl };
            let mut out = Vec::new();
            assert!(fonts.draw_line(&mut out, &line, &mut [].iter(), 12.0, [0.0, 1.0, 0.0], 10.0).is_err());
            assert!(out.is_empty());
        }
    }

    /// Both writing directions respect the padded left and right advance boundaries.
    #[test]
    fn drawing_insets_both_directions() {
        let fonts = SignatureFonts { latin_tag: "Helv".into(), latin: pdfcraft_fonts::pdf::Metrics::fallback() };
        for (rtl, pen) in [(false, 12), (true, 26)] {
            let line = SignatureLine { atoms: vec![SignatureAtom::Latin('W', 1.0)], rtl };
            let mut out = Vec::new();
            fonts.draw_line(&mut out, &line, &mut [].iter(), 6.0, [10.0, 20.0, 2.0], 10.0).unwrap();
            assert!(String::from_utf8(out).unwrap().contains(&format!("1 0 0 1 {pen} 10 Tm")));
        }
    }

    /// Fitting cannot shrink below 6 pt or draw an oversized name through the box.
    #[test]
    fn fitting_enforces_minimum_size_and_height() {
        let mut layout = SignatureLayout::default();
        assert!(layout.fit(&["W".into()], 36.0, 1.0, 100.0, 1.1).is_err());
        assert!(layout.fit(&["a\nb\nc".into()], 12.0, 100.0, 10.0, 1.15).is_err());
        ATOM_CALLS.with(|calls| calls.set(0));
        let (size, lines) = layout.fit(&["Smith".into()], 12.0, 100.0, 30.0, 1.15).unwrap();
        assert!((6.0..=12.0).contains(&size));
        assert_eq!(lines.len(), 1);
        assert_eq!(ATOM_CALLS.with(std::cell::Cell::get), 1, "unwrapped name fitting must shape only once");
    }

    /// Check that Arabic ordering does not reverse adjacent Latin text or numbers.
    #[test]
    fn mixed_script_runs_keep_latin_and_numbers_in_reading_order() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            return;
        }
        for (logical, visual) in [("فراس Smith 123", "Smith 123 سارف"), ("Reason: موافق", "Reason: قفاوم")] {
            let lines = signature_lines(logical, 12.0, 1000.0).unwrap();
            assert_eq!(lines.len(), 1);
            assert_eq!(visual_text(&lines[0]), visual);
        }
        let lines = signature_lines("\u{2067}فراس\u{2069} Smith", 12.0, 1000.0).unwrap();
        assert!(!visual_text(&lines[0]).chars().any(bidi_control));
    }

    /// Check that wrapping consumes the logical beginning of an RTL paragraph first.
    #[test]
    fn rtl_wrap_starts_with_the_beginning_of_the_logical_paragraph() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            return;
        }
        let first = signature_lines("موافق", 12.0, 1000.0).unwrap();
        let width = first[0].atoms.iter().map(SignatureAtom::advance).sum::<f64>() * 12.0 + 0.01;
        let lines = signature_lines("موافق موافق موافق", 12.0, width).unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.rtl && visual_text(l) == visual_text(&first[0])));
        let lines = signature_lines("موافق دمشق", 12.0, width).unwrap();
        assert_eq!(visual_text(&lines[0]), visual_text(&first[0]));
    }

    /// Pin one shaping call per unbounded paragraph while preserving spacing and explicit breaks.
    #[test]
    fn unbounded_name_shapes_each_paragraph_once_without_changing_spacing() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            return;
        }
        for text in ["  موافق Smith  Smith  \r\n   \n  Damascus ", "", "   "] {
            let expected = signature_lines(text, 1.0, 1000.0).unwrap();
            ATOM_CALLS.with(|calls| calls.set(0));
            let actual = signature_lines(text, 1.0, f64::MAX).unwrap();
            assert_eq!(ATOM_CALLS.with(std::cell::Cell::get), expected.len(), "{text:?}");
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(&expected) {
                assert_eq!(actual.rtl, expected.rtl);
                assert_eq!(visual_text(actual), visual_text(expected));
                assert_eq!(
                    actual.atoms.iter().map(SignatureAtom::advance).collect::<Vec<_>>(),
                    expected.atoms.iter().map(SignatureAtom::advance).collect::<Vec<_>>()
                );
            }
        }
        let word = "موافق ";
        let long_name = word.repeat(16_384 / word.len());
        ATOM_CALLS.with(|calls| calls.set(0));
        let lines = signature_lines(&long_name, 1.0, f64::MAX).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(ATOM_CALLS.with(std::cell::Cell::get), 1);
    }

    /// Check tight mixed-script lines against their fully shaped width, not summed word widths.
    #[test]
    fn wrapping_measures_complete_bidi_candidates() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            return;
        }
        let text = "موافق Smith Smith";
        let full = signature_lines(text, 12.0, 1000.0).unwrap();
        let exact_width = full[0].atoms.iter().map(SignatureAtom::advance).sum::<f64>() * 12.0;
        assert_eq!(signature_lines(text, 12.0, exact_width + 0.001).unwrap().len(), 1);
        let width = exact_width - 0.001;
        let wrapped = signature_lines(text, 12.0, width).unwrap();
        assert!(wrapped.len() > 1, "the complete candidate does not fit");
        for line in wrapped {
            let drawn_width = line.atoms.iter().map(SignatureAtom::advance).sum::<f64>() * 12.0;
            assert!(drawn_width <= width, "{drawn_width} > {width}");
        }
    }

    /// Check that joined Arabic graphemes fit together without unnecessary line breaks.
    #[test]
    fn oversized_arabic_words_measure_joined_grapheme_candidates() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            return;
        }
        let pair = signature_lines("بَبَ", 12.0, 1000.0).unwrap();
        let width = pair[0].atoms.iter().map(SignatureAtom::advance).sum::<f64>() * 12.0 + 0.001;
        let lines = signature_lines("بَبَبَبَ", 12.0, width).unwrap();
        assert_eq!(lines.len(), 2, "joined letters should fit in pairs");
        assert!(lines.iter().all(|line| visual_text(line) == visual_text(&pair[0])));
    }

    /// Preserve CR/LF breaks and base-letter/mark groups, and enforce the text-size bound.
    #[test]
    fn explicit_breaks_and_long_words_preserve_graphemes() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            return;
        }
        let lines = signature_lines("موافق\r\n\nSmith\r123", 12.0, 1000.0).unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines[1].atoms.is_empty());
        assert_eq!(visual_text(&lines[2]), "Smith");
        assert_eq!(visual_text(&lines[3]), "123");
        let single = signature_lines("بَ", 12.0, f64::MAX).unwrap();
        let width = single[0].width() * 12.0 + 0.001;
        let lines = signature_lines("بَبَبَ", 12.0, width).unwrap();
        assert_eq!(lines.len(), 3);
        for line in lines {
            let text = visual_text(&line);
            assert_eq!(text.chars().filter(|c| *c == 'ب').count(), 1);
            assert_eq!(text.chars().filter(|c| *c == '\u{064e}').count(), 1);
        }
        assert!(signature_lines(&"a".repeat(16_385), 12.0, 100.0).is_err());
    }
}
