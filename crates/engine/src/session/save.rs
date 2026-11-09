use super::{
    Arc, DocId, Document, EditError, Keys, OpenError, RecoverySnapshot, RenderPool, SaveOptions, Session, SignOptions, StatusCaptions, TrustStore,
    display_bytes, display_config, guard, render_threads, signatures_of, writable, write_full, write_incremental, xfa_sync_datasets,
};

impl Session {
    /// The bytes to write for Save: an incremental update of the file as opened/last saved,
    /// with `/ModDate` stamped. Call `mark_saved` after writing them successfully.
    ///
    /// A file whose cross-reference data had to be rebuilt is written in full even when nothing
    /// was edited: its bytes as opened are the damaged file, and this save is how the repair the
    /// user was told about reaches the disk (#1937). Not a signed one: a full rewrite would
    /// break its signatures, so with no edits it keeps its bytes, damage and all.
    pub fn save_bytes(&self, id: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let Some(editor) = doc.editor.as_ref() else { return Ok(doc.bytes.clone()) };
        let on_saved = Arc::ptr_eq(editor.cos.bytes(), &editor.saved);
        if !editor.cos.is_modified() && on_saved && (!editor.cos.reconstructed() || doc.is_signed()) {
            return Ok(editor.saved.clone());
        }
        // What the user removed again before saving (a cleared redaction mark) stays out of the file.
        let opts = SaveOptions { mod_date: self.now().map(pdfcraft_cos::pdf_date), drop_unlinked: true, ..SaveOptions::default() };
        // A restored snapshot can be based on an earlier revision. Append to the last saved
        // file so that revision stays in the chain. A full rewrite (redaction, new encryption,
        // a reconstructed file) still writes the current graph on its own.
        let rewrite = editor.cos.full_save_required() || editor.cos.encryption_changed() || editor.cos.revisions().is_empty();
        let cos = writable(&editor.cos)?;
        guard(|| {
            if on_saved || rewrite {
                write_incremental(&cos, &opts)
            } else {
                let mut base = pdfcraft_cos::Document::open_with_password(editor.saved.clone(), editor.saved_keys.reopen.as_deref())?;
                base.overlay_changes_from(&cos)?;
                write_incremental(&base, &opts)
            }
        })
        .map_err(EditError::Write)?
        .map(Arc::new)
        .map_err(|e| EditError::Write(e.to_string()))
    }

    /// A compact, garbage-collected rewrite (Save As ▸ "Optimized" / Reduce File Size groundwork).
    pub fn save_full_bytes(&self, id: DocId) -> Result<Arc<Vec<u8>>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if doc.is_signed() {
            return Err(EditError::Signed);
        }
        let editor = doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        let opts = SaveOptions { mod_date: self.now().map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
        let cos = writable(&editor.cos)?;
        guard(|| write_full(&cos, &opts)).map_err(EditError::Write)?.map(Arc::new).map_err(|e| EditError::Write(e.to_string()))
    }

    /// Check that `bytes`, from [`Self::save_bytes`] or [`Self::save_full_bytes`], reopen as this
    /// document before they are written (#1698): [`Self::mark_saved`] only finds out after the
    /// file has been replaced. Same error as `mark_saved`.
    pub fn check_save(&self, id: DocId, bytes: &Arc<Vec<u8>>) -> Result<(), EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let Some(editor) = doc.editor.as_ref() else { return Ok(()) };
        pdfcraft_cos::Document::open_with_password(bytes.clone(), editor.keys.reopen.as_deref()).map_err(|e| EditError::Reopen(e.to_string()))?;
        pdfcraft_render::check_pages(bytes.clone(), doc.password.as_deref(), doc.info.pages.len()).map_err(|e| EditError::Reopen(e.to_string()))
    }

    /// Record a successful save of `bytes` (to `path`, if any): rebase editing on the saved file
    /// so the next save appends only newer edits. Undo history is kept.
    pub fn mark_saved(&mut self, id: DocId, bytes: Arc<Vec<u8>>, path: Option<String>) -> Result<(), EditError> {
        let doc = self.doc_mut(id)?;
        if let Some(editor) = doc.editor.as_mut() {
            // An encrypted file needs the password it is protected with now (the owner
            // password when known, so saving never downgrades this session's rights).
            let keys = editor.keys.clone();
            // The saved file carries no Password-field values (#3943, #3944); the open document keeps them.
            let typed = pdfcraft_forms::password_values(&editor.cos);
            editor.cos =
                pdfcraft_cos::Document::open_with_password(bytes.clone(), keys.reopen.as_deref()).map_err(|e| EditError::Reopen(e.to_string()))?;
            pdfcraft_forms::restore_password_values(&mut editor.cos, &typed).map_err(|e| EditError::Reopen(e.to_string()))?;
            if !typed.is_empty() && doc.info.xfa.is_some() {
                let mut datasets = None;
                let _ = xfa_sync_datasets(&mut editor.cos, doc.xfa_template.as_deref(), &mut datasets);
            }
            editor.saved = bytes;
            editor.saved_keys = keys;
            editor.unsaved_file = false;
        }
        if let Some(p) = path {
            doc.name = std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
            doc.path = Some(p);
        }
        doc.dirty = false;
        doc.generation += 1;
        Self::refresh(doc).inspect_err(|_| Self::show_released(doc))
    }

    /// Enter the permissions (owner) password of a document opened with its user password,
    /// lifting its restrictions for this session (Acrobat asks for it before changing security).
    /// Not an edit: nothing is marked unsaved and there is no undo step. Undo and redo states
    /// are unlocked as well, and saving keeps the rights. Unencrypted documents and documents
    /// opened by the owner need nothing.
    pub fn unlock(&mut self, id: DocId, password: &str) -> Result<(), EditError> {
        let doc = self.doc_mut(id)?;
        let editor = doc.editor.as_mut().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        if editor.cos.permissions().is_none_or(|p| p.owner) {
            return Ok(());
        }
        editor.cos.unlock(password).map_err(|e| match e {
            pdfcraft_cos::CosError::WrongPassword => EditError::WrongPassword,
            other => EditError::Protection(other.to_string()),
        })?;
        // Saving a state that keeps the security it was opened with reopens it with the same
        // passwords, so it reopens as owner.
        let reopen_as_owner = |cos: &pdfcraft_cos::Document, keys: &mut Keys| {
            if !cos.encryption_changed() {
                keys.reopen = Some(password.to_owned());
            }
        };
        reopen_as_owner(&editor.cos, &mut editor.keys);
        // Other states with the same restricted access are unlocked too; states opened with
        // other passwords (protection changed and saved since) stay as they are.
        for (_, cos, keys, _) in editor.undo.iter_mut().chain(editor.redo.iter_mut()) {
            if cos.permissions().is_some_and(|p| !p.owner) && cos.unlock(password).is_ok() {
                reopen_as_owner(cos, keys);
            }
        }
        Ok(())
    }

    /// File ▸ Revert: back to the last saved version (or the file as opened). Undo history is
    /// cleared, as in Acrobat.
    pub fn revert(&mut self, id: DocId) -> Result<(), EditError> {
        let doc = self.doc_mut(id)?;
        let editor = doc.editor.as_mut().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        let base = editor.saved.clone();
        // The saved file opens with the passwords it was saved with. The current snapshot may
        // be an older revision (undo past that save), so its own base is not the saved file.
        let mut candidates = vec![editor.saved_keys.clone(), editor.keys.clone()];
        candidates.extend(editor.undo.iter().rev().map(|s| s.2.clone()));
        candidates.push(Keys::default());
        let (cos, keys) = candidates
            .into_iter()
            .find_map(|k| pdfcraft_cos::Document::open_with_password(base.clone(), k.reopen.as_deref()).ok().map(|c| (c, k)))
            .ok_or_else(|| EditError::Reopen("the saved file can't be opened".into()))?;
        editor.cos = cos;
        editor.keys = keys;
        editor.undo.clear();
        editor.redo.clear();
        Self::adopt_keys(doc);
        Self::sync_dirty(doc);
        // The changes whose exits ran are gone with the rest: skip no exit.
        doc.xfa_exited.clear();
        doc.generation += 1;
        Self::refresh(doc).inspect_err(|_| Self::show_released(doc))
    }

    /// The document's file changed on disk (a LaTeX or Typst build rewrote it, #431): open
    /// `bytes`, the file as it is now, in its place. Everything opening does runs again (XFA
    /// layout and scripts, signatures, repairs), but the document keeps its id, name and path,
    /// so its tab and view stay with it. Undo history goes, as with Revert. The password it was
    /// opened with is tried; on any failure the document is left as it was.
    pub fn reload(&mut self, id: DocId, bytes: Arc<Vec<u8>>) -> Result<(), OpenError> {
        let old = self.docs.iter().position(|d| d.id == id).ok_or_else(|| OpenError::Invalid("the document is not open".into()))?;
        let doc = &self.docs[old];
        let (name, path, generation) = (doc.name.clone(), doc.path.clone(), doc.generation);
        let password = doc.editor.as_ref().and_then(|e| e.keys.reopen.clone()).or_else(|| doc.password.clone());
        let new_id = self.open(name, path, bytes, password.as_deref())?;
        let at = self.docs.iter().position(|d| d.id == new_id).ok_or_else(|| OpenError::Invalid("the new version was not kept".into()))?;
        let mut doc = self.docs.remove(at);
        doc.id = id;
        // How the document is shown is the reader's, not the file's: reading colours, hidden
        // comments and the inverted view stay as they were (#3443). `open` built the renderer with
        // the defaults.
        if let Some(old) = self.docs.get(old)
            && (doc.config.reading_colors != old.config.reading_colors
                || doc.config.hide_comments != old.config.hide_comments
                || doc.config.invert_colors != old.config.invert_colors
                || doc.config.invert_palette != old.config.invert_palette
                || doc.config.invert_keep_images != old.config.invert_keep_images)
        {
            doc.config.reading_colors = old.config.reading_colors;
            doc.config.hide_comments = old.config.hide_comments;
            doc.config.invert_colors = old.config.invert_colors;
            doc.config.invert_palette = old.config.invert_palette;
            doc.config.invert_keep_images = old.config.invert_keep_images;
            doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), display_config(&doc.config));
        }
        // Caches keyed by the edit generation (edit lines, images, measurements) must see a new
        // one; there is nothing new to autosave.
        doc.generation = generation + 1;
        doc.snapshot_generation = doc.generation;
        // `open` pushed the new version last, so the old one is still at `old`.
        *self.docs.get_mut(old).ok_or_else(|| OpenError::Invalid("the document is not open".into()))? = doc;
        Ok(())
    }

    /// Whether a changed document still needs a recovery snapshot. This does not consume it.
    pub fn has_pending_autosave(&self) -> bool {
        self.docs.iter().any(|d| d.dirty && d.generation != d.snapshot_generation)
    }

    /// Documents with unsaved changes made since the last call: their current working file,
    /// for crash recovery. Encrypted documents stay encrypted in the snapshot.
    pub fn autosave_snapshots(&mut self) -> Vec<RecoverySnapshot> {
        let mut out = Vec::new();
        for d in &mut self.docs {
            if d.dirty && d.generation != d.snapshot_generation {
                // The working bytes hold typed Password-field values (#3926, #3944): a recovery file
                // is written from a copy without them, and none is written if that copy fails.
                let bytes =
                    match d.editor.as_ref().filter(|e| pdfcraft_forms::has_password_values(&e.cos) || pdfcraft_xfa::has_password_values(&e.cos)) {
                        Some(e) => match writable(&e.cos)
                            .and_then(|c| write_incremental(&c, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string())))
                        {
                            Ok(b) => Arc::new(b),
                            Err(_) => continue,
                        },
                        None => d.bytes.clone(),
                    };
                d.snapshot_generation = d.generation;
                out.push(RecoverySnapshot {
                    doc: d.id,
                    name: d.name.clone(),
                    path: d.path.clone(),
                    bytes,
                    encrypted: d.info.encrypted || d.editor.as_ref().is_some_and(|e| e.cos.security().is_some()),
                });
            }
        }
        out
    }

    /// The snapshot [`Self::autosave_snapshots`] handed out for `id` could not be stored: the
    /// document is due again, so the next autosave tries it again.
    pub fn autosave_failed(&mut self, id: DocId) {
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            // Any generation but the current one leaves it due.
            d.snapshot_generation = d.generation.wrapping_sub(1);
        }
    }

    /// Mark a document opened from a recovery file: it has unsaved changes and belongs at
    /// `path` (where Save writes), as when the session ended.
    pub fn mark_recovered(&mut self, id: DocId, path: Option<String>) {
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.path = path;
            d.dirty = true;
            if let Some(e) = d.editor.as_mut() {
                e.unsaved_file = true;
            }
            d.generation += 1;
            d.snapshot_generation = d.generation; // its bytes are already in the recovery store
        }
    }

    /// The certificates trusted for signing.
    pub fn trusted_certificates(&self) -> &[pdfcraft_sign::Certificate] {
        &self.trust.certs
    }

    /// Replace the trusted certificates, revalidate every open document's signatures,
    /// and rebuild its display copy and renderer to match (working file untouched,
    /// generation/dirty unchanged: trust alone is not an edit).
    pub fn set_trusted_certificates(&mut self, certs: Vec<pdfcraft_sign::Certificate>) {
        self.trust = Arc::new(TrustStore { certs });
        for doc in &mut self.docs {
            Self::revalidate(doc, self.trust.clone());
        }
    }

    /// Validate `doc`'s signatures against `trust`, and rebuild its display copy and renderer
    /// with them: the verdicts choose what the copy draws. An unsigned document has nothing to
    /// validate and keeps both. Viewer caches are invalidated by the UI trust path.
    pub(crate) fn revalidate(doc: &mut Document, trust: Arc<TrustStore>) {
        doc.trust = trust;
        if doc.signatures.is_empty() {
            return;
        }
        let Some(editor) = doc.editor.as_mut() else { return };
        let mut signatures = signatures_of(&editor.cos, &doc.bytes, &doc.trust, &doc.sig_cache);
        doc.display = display_bytes(Some(editor), &doc.bytes, &mut signatures, &doc.captions);
        doc.signatures = Arc::new(signatures);
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), display_config(&doc.config));
    }

    /// The words the caption of a named-layer signature appearance is drawn with
    /// ([`pdfcraft_sign::show_status_layers`]), in the interface language. When they change, every
    /// open document's display copy and renderer are rebuilt; like trust, this is not an edit.
    /// Returns whether they changed.
    pub fn set_signature_captions(&mut self, captions: StatusCaptions) -> bool {
        if *self.captions == captions {
            return false;
        }
        self.captions = Arc::new(captions);
        for doc in &mut self.docs {
            Self::draw_captions(doc, self.captions.clone());
        }
        true
    }

    /// Draw `doc`'s signature status layers with `captions`: the display copy and renderer are
    /// rebuilt when it draws a signature's state (working file, generation and dirty unchanged).
    /// Whether a state is drawn does not depend on the words: a caption that can't be drawn in
    /// them is drawn in English.
    pub(crate) fn draw_captions(doc: &mut Document, captions: Arc<StatusCaptions>) {
        doc.captions = captions;
        if !doc.signatures.iter().any(|s| s.verdict_shown) {
            return;
        }
        let Some(editor) = doc.editor.as_mut() else { return };
        let signatures = Arc::make_mut(&mut doc.signatures);
        doc.display = display_bytes(Some(editor), &doc.bytes, signatures, &doc.captions);
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), display_config(&doc.config));
    }

    /// The signing time as a PDF date in local time with its offset (`D:…+02'00'`).
    pub fn signing_date(&self) -> String {
        let offset = self.local_offset();
        let local = pdfcraft_cos::pdf_date(self.now().unwrap_or(0) + offset);
        let stamp = local.trim_end_matches('Z');
        if offset == 0 {
            return format!("{stamp}Z");
        }
        let (sign, m) = if offset < 0 { ('-', -offset / 60) } else { ('+', offset / 60) };
        format!("{stamp}{sign}{:02}'{:02}'", m / 60, m % 60)
    }

    /// Sign the document's current state with `id` (Use a certificate ▸ Digitally sign). Returns
    /// the signed file; the caller saves it and then calls [`Session::mark_signed`]. An empty
    /// `opts.date` takes the session clock.
    pub fn sign(&self, doc: DocId, id: &pdfcraft_sign::DigitalId, mut opts: SignOptions) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        // Encrypted documents are signed under their permissions (the signer checks them).
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        if opts.date.is_empty() {
            opts.date = self.signing_date();
        }
        // A typed Password-field value is not signed into the file (#3926), as it isn't saved.
        let cos = drawn_for_first_signature(writable(&editor.cos)?);
        Ok(Arc::new(pdfcraft_sign::sign(&cos, id, &opts)?))
    }

    /// [`Session::sign`], embedding an RFC 3161 signature timestamp (PAdES B-T) produced by
    /// `tsa`. The transport lives with the caller; the engine never opens a socket.
    pub fn sign_with_timestamp(
        &self,
        doc: DocId,
        id: &pdfcraft_sign::DigitalId,
        mut opts: SignOptions,
        tsa: &dyn pdfcraft_sign::TimestampAuthority,
    ) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        if opts.date.is_empty() {
            opts.date = self.signing_date();
        }
        let cos = drawn_for_first_signature(writable(&editor.cos)?);
        let signed = Arc::new(pdfcraft_sign::sign_with_timestamp(&cos, id, &opts, tsa)?);
        // Checked like a save before anyone writes it (#2166).
        self.check_save(doc, &signed)?;
        Ok(signed)
    }

    /// Append a standalone document timestamp (RFC 3161, `/ETSI.RFC3161`) covering the file's
    /// current state. An empty `date` takes the session clock; the transport is the caller's.
    pub fn timestamp_document(&self, doc: DocId, tsa: &dyn pdfcraft_sign::TimestampAuthority, date: String) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        let date = if date.is_empty() { self.signing_date() } else { date };
        let cos = writable(&editor.cos)?;
        let stamped = Arc::new(pdfcraft_sign::timestamp_document(&cos, tsa, &date)?);
        self.check_save(doc, &stamped)?;
        Ok(stamped)
    }

    /// Embed revocation evidence into the catalog's `/DSS` with `/VRI` entries per signature
    /// (PAdES B-LT): an incremental update that never rewrites signed bytes.
    pub fn embed_ltv(&self, doc: DocId, evidence: &pdfcraft_sign::dss::Evidence) -> Result<Arc<Vec<u8>>, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        let cos = writable(&editor.cos)?;
        let embedded = Arc::new(pdfcraft_sign::dss::embed(&cos, evidence)?);
        self.check_save(doc, &embedded)?;
        Ok(embedded)
    }

    /// Record that the signed file `bytes` was saved (to `path`): like [`Session::mark_saved`],
    /// and the edit history before signing is dropped (signing can't be undone).
    pub fn mark_signed(&mut self, id: DocId, bytes: Arc<Vec<u8>>, path: Option<String>) -> Result<(), EditError> {
        self.mark_saved(id, bytes, path)?;
        let doc = self.doc_mut(id)?;
        if let Some(e) = doc.editor.as_mut() {
            e.undo.clear();
            e.redo.clear();
        }
        Ok(())
    }
}

/// `cos` with its form fields drawn, when the signature about to be made is the document's first
/// and the form sets `/NeedAppearances` (#5939). That signature removes the flag, after which
/// viewers show each field's stored appearance; a filled field whose stored appearance draws no
/// text (an empty placeholder such as `/Tx BMC EMC`, or a stale look) would go blank. So every
/// text and choice field is drawn from its value first, as the document is displayed
/// ([`pdfcraft_forms::with_field_appearances`]), and the signature covers that look.
///
/// One exception: where PdfCraft's drawing would show a character as `?` (the field's font can't
/// hold it) and the field's own appearance draws text, the field keeps its own appearance, which
/// the program that filled it drew with a font that has the character. A field whose own
/// appearance draws no text gets PdfCraft's drawing, `?` and all. Later signatures change
/// nothing here: redrawing would count as a change to the earlier signatures.
fn drawn_for_first_signature(cos: std::borrow::Cow<'_, pdfcraft_cos::Document>) -> std::borrow::Cow<'_, pdfcraft_cos::Document> {
    use pdfcraft_forms::FieldKind;
    if !pdfcraft_forms::need_appearances(&cos) || pdfcraft_sign::has_signatures(&cos) {
        return cos;
    }
    let Some(mut drawn) = pdfcraft_forms::with_field_appearances(&cos) else { return cos };
    for f in pdfcraft_forms::fields(&cos) {
        if !matches!(f.kind, FieldKind::Text | FieldKind::Combo | FieldKind::List) {
            continue;
        }
        for w in &f.widgets {
            let own = cos.get(w.obj).as_dict().filter(|d| draws_text(&cos, d)).and_then(|d| d.get(b"AP").cloned());
            // `.1`: the first character the drawing showed as `?`.
            if let Some(ap) = own
                && pdfcraft_forms::appearance::field_appearance_drawn(&cos, &f, w, &f.value, true).1.is_some()
            {
                drawn.update_dict(w.obj, |d| d.set(b"AP".to_vec(), ap)).ok();
            }
        }
    }
    std::borrow::Cow::Owned(drawn)
}

/// Whether the widget's normal appearance stream (`/AP /N`) shows text, itself or through the
/// form XObjects it paints (a background-only one doesn't count).
fn draws_text(cos: &pdfcraft_cos::Document, widget: &pdfcraft_cos::Dict) -> bool {
    let ap = widget.get(b"AP").map(|ap| cos.resolve(ap));
    let normal = ap.as_deref().and_then(|ap| ap.as_dict()).and_then(|ap| ap.get(b"N")).map(|n| cos.resolve(n));
    let Some(pdfcraft_cos::Object::Stream(s)) = normal.as_deref() else { return false };
    stream_draws_text(cos, s, &mut 16)
}

/// Whether the form stream `s` shows text, following `Do` into its form XObjects. `budget` caps
/// the streams opened along the way; running out (deep nesting, a cycle, many XObjects) counts
/// as text, so the field keeps its own appearance as it would without the look inside.
fn stream_draws_text(cos: &pdfcraft_cos::Document, s: &pdfcraft_cos::Stream, budget: &mut u8) -> bool {
    let Ok(data) = s.decoded() else { return false };
    let resources = s.dict.get(b"Resources").map(|r| cos.resolve(r));
    let xobjects = resources.as_deref().and_then(|r| r.as_dict()).and_then(|r| r.get(b"XObject")).map(|x| cos.resolve(x));
    for op in pdfcraft_content::ops(&data) {
        match op.op.as_slice() {
            b"Tj" | b"TJ" | b"'" | b"\"" => return true,
            b"Do" => {
                let name = op.operands.first().and_then(|n| n.as_name());
                let x = name.and_then(|n| xobjects.as_deref().and_then(|x| x.as_dict()).and_then(|x| x.get(n))).map(|x| cos.resolve(x));
                let Some(pdfcraft_cos::Object::Stream(f)) = x.as_deref() else { continue };
                if f.dict.name(b"Subtype") != Some(b"Form") {
                    continue;
                }
                if *budget == 0 {
                    return true;
                }
                *budget -= 1;
                if stream_draws_text(cos, f, budget) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}
