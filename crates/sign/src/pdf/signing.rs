use super::listing::{mk_rotation, nums, sig_fields};
use super::{Certificate, Dict, DigestAlg, DigitalId, Document, HashSet, Object, PdfString, SaveOptions, SignError, SignedData, Stream};

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
        self.fields().into_iter().find_map(|(field, text)| pdfcraft_fonts::first_undrawable(text).map(|c| (field, c)))
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

/// Sign `doc` with `id`; returns the signed file (an incremental update when possible).
pub fn sign(doc: &Document, id: &DigitalId, opts: &SignOptions) -> Result<Vec<u8>, SignError> {
    sign_inner(doc, id, opts, None)
}

/// Sign and embed an RFC 3161 signature timestamp from `tsa` (PAdES B-T). The transport is the
/// caller's: this crate never opens a socket, and a rejected or malformed token fails the
/// signing before any file is produced.
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

/// The visible signature: the signer's name large on the left, or the graphic in its place, and
/// the details on the right (Acrobat's standard layout), in Helvetica, or in the bundled Inter
/// font when a detail needs characters beyond Western European. A graphic with no details beside
/// it fills the box. An error, before anything is drawn, when a detail it writes (the signer's
/// name, the certificate's subject, the reason or the location) has a character neither font can
/// draw: signing invisibly, or leaving that part out, still works. A graphic doesn't change this:
/// the name it replaces on the left is still among the details. A label that is drawn
/// ([`AppearanceLabels`], the caller's words) must be drawable in Helvetica, and is refused otherwise.
///
/// `turn` is the widget's `/MK /R` (0, 90, 180 or 270; anything else counts as 0): the layout is
/// drawn upright in the box as turned by it, and a `/Matrix` maps that box onto `rect`.
fn appearance(doc: &mut Document, rect: [f64; 4], turn: i64, name: &str, cert: &Certificate, opts: &SignOptions) -> Result<Stream, SignError> {
    use pdfcraft_fonts::{first_undrawable, helvetica_width, inter_width, literal, undrawable_message, win_ansi, wrap, wrap_with};
    let turn = if matches!(turn, 90 | 180 | 270) { turn } else { 0 };
    let (rw, rh) = ((rect[2] - rect[0]).max(0.0), (rect[3] - rect[1]).max(0.0));
    // The upright layout box: a quarter turn swaps its sides.
    let (w, h) = if turn % 180 == 0 { (rw, rh) } else { (rh, rw) };
    let a = &opts.appearance;
    let dn = cert.subject.display();
    let mut needs_unicode = false;
    if w > 1.0 && h > 1.0 {
        let shown = [
            (a.name, "the signer's name", Some(name)),
            (a.distinguished_name, "the certificate's subject", Some(dn.as_str())),
            (a.reason, "the reason", opts.reason.as_deref()),
            (a.location, "the location", opts.location.as_deref()),
        ];
        for (on, what, text) in shown {
            if on
                && let Some(s) = text
                && first_undrawable(s).is_some()
            {
                for c in s.chars() {
                    if matches!(c, '\n' | '\r') {
                        continue;
                    }
                    if !pdfcraft_fonts::embedding::covers(pdfcraft_fonts::INTER_REGULAR, &c.to_string()) {
                        return Err(SignError::Unsupported(format!(
                            "{}; sign without a visible appearance, or leave {what} out of it",
                            undrawable_message(&format!("{what} of the visible signature"), c)
                        )));
                    }
                }
                needs_unicode = true;
            }
        }
        // The labels are the caller's words (a translation), checked as the details are: only
        // those that are drawn, as each is drawn only beside its detail.
        let has = |v: Option<&str>| v.is_some_and(|v| !v.is_empty());
        let drawn = [a.name, a.distinguished_name, a.reason && has(opts.reason.as_deref()), a.location && has(opts.location.as_deref()), a.date];
        let mut labels = opts.appearance_labels.fields().into_iter().zip(drawn).filter(|&(_, on)| a.labels && on);
        if let Some((field, c)) = labels.find_map(|((field, text), _)| first_undrawable(text).map(|c| (field, c))) {
            return Err(SignError::Unsupported(format!(
                "{}; pass that label in Western European characters, or leave the labels out of the visible signature",
                undrawable_message(&format!("the {field} label of the visible signature"), c)
            )));
        }
    }
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
    let mut out = Vec::new();
    let mut image = None;
    let mut fonts = Dict::new();
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
    let measure = |s: &str, sz: f64| -> f64 { if needs_unicode { inter_width(s, sz) } else { helvetica_width(s, sz) } };
    let wrap_text =
        |s: &str, sz: f64, max_w: f64| -> Vec<String> { if needs_unicode { wrap_with(s, sz, max_w, inter_width) } else { wrap(s, sz, max_w) } };
    let encode_line =
        |l: &str| -> Vec<u8> { if let Some(m) = &metrics { literal(&m.encode(l).unwrap_or_else(|| win_ansi(l))) } else { literal(&win_ansi(l)) } };
    if w > 1.0 && h > 1.0 {
        let pad = (h * 0.06).clamp(1.0, 6.0);
        let (left_w, right_x) = match (&opts.graphic, a.name) {
            (Some(_), _) if lines.is_empty() => (w, pad),
            (Some(_), _) | (None, true) => (w * 0.5, w * 0.5 + pad),
            (None, false) => (0.0, pad),
        };
        let right_w = (w - right_x - pad).max(1.0);
        if let Some(g) = &opts.graphic {
            let (ops, im) = g.draw(doc, [pad, pad, (left_w - 2.0 * pad).max(0.0), (h - 2.0 * pad).max(0.0)])?;
            out.extend(ops.bytes());
            image = im;
        }
        out.extend_from_slice(b"BT\n0 g\n");
        if a.name && opts.graphic.is_none() {
            // The name fills the left half.
            let mut size = (h * 0.4).min(36.0);
            let fit = (left_w - 2.0 * pad).max(1.0);
            let longest = measure(name, 1.0).max(0.01);
            size = size.min(fit / longest).max(4.0);
            let words = wrap_text(name, size, fit);
            let total = words.len() as f64 * size * 1.1;
            let mut y = (h + total) / 2.0 - size * 0.85;
            for l in &words {
                out.extend(format!("/{font_tag} {} Tf 1 0 0 1 {} {} Tm ", fmt(size), fmt(pad), fmt(y)).bytes());
                out.extend(encode_line(l));
                out.extend_from_slice(b" Tj\n");
                y -= size * 1.1;
            }
        }
        // The details, shrunk to fit the right side.
        let mut size = (h / (lines.len().max(1) as f64 * 1.15)).min(12.0);
        let wrapped = loop {
            let all: Vec<String> = lines.iter().flat_map(|l| wrap_text(l, size, right_w)).collect();
            if all.len() as f64 * size * 1.15 <= h - 2.0 * pad || size <= 3.0 {
                break all;
            }
            size *= 0.9;
        };
        let mut y = h - pad - size * 0.9;
        for l in &wrapped {
            out.extend(format!("/{font_tag} {} Tf 1 0 0 1 {} {} Tm ", fmt(size), fmt(right_x), fmt(y)).bytes());
            out.extend(encode_line(l));
            out.extend_from_slice(b" Tj\n");
            y -= size * 1.15;
        }
        out.extend_from_slice(b"ET\n");
    }
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
