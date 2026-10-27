use super::*;

impl Session {
    /// Apply an edit. On success the previous state is undoable and the view data is refreshed.
    pub fn apply(&mut self, id: DocId, edit: Edit) -> Result<(), EditError> {
        let now = self.now();
        let today = self.today();
        let js_off = self.js_off;
        let xfa_time_scale = self.xfa_time_scale.unwrap_or(1);
        let doc = self.doc_mut(id)?;
        doc.edit_notices.clear();
        let name = doc.name.clone();
        let is_xfa = doc.info.xfa.is_some();
        let mut scope = scope_of(&edit);
        let mut cx = EditCtx::new(now, doc.generation ^ (id.0 << 48));
        if !js_off {
            cx.xfa = doc.xfa_template.clone();
        }
        cx.xfa_time_scale = xfa_time_scale;
        cx.pages = doc.info.pages.len();
        cx.today = today;
        let reason = doc.read_only_reason.clone().unwrap_or_default();
        let signed = doc.is_signed();
        let editor = doc.editor.as_mut().ok_or(EditError::ReadOnly(reason))?;
        if let Some(p) = editor.cos.permissions() {
            check_permission(&edit, &p)?;
        }
        // Rewriting an OCR word (invisible text over a scan) also hides the scanned word: cover it
        // in the paper's colour and draw the new text in the ink's.
        let edit = match edit {
            Edit::EditTextBlock { page, block, text, mut style } => {
                // Only invisible text is sampled; the blocks are usually cached from opening it.
                if style.cover.is_none()
                    && let Some(info) = doc.info.pages.get(page)
                    && let Some(b) = editor.text_blocks(page).ok().and_then(|b| b.get(block).filter(|b| b.invisible).cloned())
                    && let Some((paper, ink)) = guard(|| scan_colours::sample(&editor.cos, info, page, &b)).ok().flatten()
                {
                    style.cover = Some(paper);
                    style.color = style.color.or(Some(ink));
                }
                Edit::EditTextBlock { page, block, text, style }
            }
            other => other,
        };
        // Nothing to bake: leave undo history and the dirty flag alone.
        if matches!(&edit, Edit::FlattenFillSign) && !pdfcraft_annot::has_visible_fill_sign(&editor.cos) {
            return Ok(());
        }
        let mut next = editor.cos.clone();
        if !js_off && uses_scripts(&edit) {
            cx.js = Some(js::JsRunner::new(&next, &name));
        }
        cx.renamed_fields.clear();
        cx.redaction_overlays.clear();
        // `next` is a copy: if the edit fails or crashes, the document is unchanged.
        guard(|| run_edit(&mut next, &edit, &mut cx))
            .unwrap_or_else(|m| Err(EditError::Invalid(format!("{} failed unexpectedly ({m}); the document was not changed", edit.label()))))?;
        // Redaction removed what its own interpreter places under the marks. The marks were drawn
        // over the page as the renderer shows it, so the renderer has the last word.
        if let Edit::ApplyRedactions { pages } = &edit {
            let marks: Vec<pdfcraft_redact::Mark> =
                pdfcraft_redact::marks(&editor.cos).into_iter().filter(|m| pages.as_ref().is_none_or(|p| p.contains(&m.page))).collect();
            guard(|| shown_under_redactions(&next, &marks, &cx.redaction_overlays, &doc.info.pages, doc.password.as_deref()))
                .unwrap_or_else(|m| Err(EditError::Invalid(format!("{} failed unexpectedly ({m}); the document was not changed", edit.label()))))?;
        }
        // Renamed fields are said, not slipped in: a repair-log note (with the caveat that
        // scripts were left alone) and a count in the undo step's label.
        let renamed_note = |renamed: &[(String, String)]| {
            (!renamed.is_empty()).then(|| {
                let shown: Vec<String> = renamed.iter().take(5).map(|(old, new)| format!("{old} → {new}")).collect();
                let more = renamed.len().saturating_sub(shown.len());
                let more = if more > 0 { format!(", and {more} more") } else { String::new() };
                format!(
                    "inserted pages renamed {} form field{} to keep their names separate ({}{more}); scripts that refer to the old names were not rewritten",
                    renamed.len(),
                    if renamed.len() == 1 { "" } else { "s" },
                    shown.join(", ")
                )
            })
        };
        if let Some(note) = renamed_note(&cx.renamed_fields) {
            next.note_repair(note);
        }
        // Only Insert from File renames fields; its label says so in a wording the interface
        // translates (the count lives in the repair note).
        let label = match (&edit, cx.renamed_fields.len()) {
            (_, 0) => edit.label(),
            (Edit::InsertPagesFrom { name, .. }, n) => Label::with(
                if n == 1 { "Insert pages from {name} — 1 field renamed" } else { "Insert pages from {name} — fields renamed" },
                name.as_str(),
            ),
            _ => edit.label(),
        };
        // An XFA form's scripts answer the change: exit and validate scripts of the field, then
        // every calculation (and a reset recalculates). They read the data, so it is brought up
        // to date first.
        if let Some(tpl) = cx.xfa.clone() {
            if is_xfa && matches!(scope_of(&edit), Scope::Form | Scope::Full | Scope::Content) {
                let datasets = &mut cx.xfa_datasets;
                let notes = guard(|| xfa_sync_datasets(&mut next, Some(&tpl), datasets))
                    .unwrap_or_else(|m| Err(format!("writing the XFA data failed unexpectedly ({m})")))
                    .map_err(EditError::Write)?;
                cx.xfa_out.errors.extend(notes);
            }
            fn collect(e: &Edit, changed: &mut Vec<String>, reset: &mut bool) {
                match e {
                    Edit::SetFieldValue { name, .. } => changed.push(name.clone()),
                    Edit::ResetForm { .. } => *reset = true,
                    Edit::Batch { edits, .. } => edits.iter().for_each(|e| collect(e, changed, reset)),
                    _ => {}
                }
            }
            let mut changed: Vec<String> = Vec::new();
            let mut reset = false;
            collect(&edit, &mut changed, &mut reset);
            if !changed.is_empty() || reset {
                let mut run = xfa::XfaRun {
                    page_count: cx.pages,
                    out: &mut cx.xfa_out,
                    datasets: cx.xfa_datasets,
                    ran_away: false,
                    time_scale: cx.xfa_time_scale,
                };
                // Every changed field's scripts, then the calculations once (after a reset,
                // only those).
                let ran = guard(|| xfa::on_changes(&mut next, &tpl, &changed, &mut run))
                    .unwrap_or_else(|m| Err(format!("the form's scripts failed unexpectedly ({m})")));
                cx.xfa_datasets = run.datasets;
                cx.xfa_ran_away |= run.ran_away;
                if let Err(e) = ran {
                    cx.xfa_out.errors.push(e);
                }
            }
        }
        // XFA forms keep their values in the datasets packet too, for Adobe's viewers.
        let mut xfa_notes = Vec::new();
        if is_xfa && matches!(scope_of(&edit), Scope::Form | Scope::Full | Scope::Content) {
            let (tpl, datasets) = (cx.xfa.as_deref(), &mut cx.xfa_datasets);
            xfa_notes = guard(|| xfa_sync_datasets(&mut next, tpl, datasets))
                .unwrap_or_else(|m| Err(format!("writing the XFA data failed unexpectedly ({m})")))
                .map_err(EditError::Write)?;
            // Rebuilding from stale data would discard accepted widget values.
            if xfa_notes.is_empty() {
                let reflow = guard(|| xfa::relayout_changed_text(&mut next, tpl, &doc.form))
                    .unwrap_or_else(|m| Err(format!("laying out the XFA text failed unexpectedly ({m})")))
                    .map_err(EditError::Write)?;
                if let Some(warnings) = reflow {
                    xfa_notes.extend(warnings);
                    // Layout replaces pages and widgets, even if the page count is unchanged.
                    // Undo/redo must rebuild these too instead of refreshing only field values.
                    scope = Scope::Full;
                }
            }
        }
        // Signed documents are only ever saved incrementally: an edit that needs a full rewrite
        // (applying redactions, changing security, sanitizing) would invalidate the signatures.
        // So does any edit to a file rebuilt from object headers: it has no chain to append to.
        let rewrites = |c: &pdfcraft_cos::Document| c.full_save_required() || c.encryption_changed();
        if signed && ((rewrites(&next) && !rewrites(&editor.cos)) || (next.is_modified() && next.revisions().is_empty())) {
            return Err(EditError::SignedRewrite(edit.label().to_string()));
        }
        // What this edit drew with `?` (a value or comment text its font can't draw, stored as
        // typed): reported, never silent (AGENTS.md §4).
        let notices = if matches!(scope_of(&edit), Scope::Form | Scope::Comments | Scope::Full) {
            guard(|| undrawn_notices(&editor.cos, &next)).unwrap_or_default()
        } else {
            Vec::new()
        };
        let previous = std::mem::replace(&mut editor.cos, next);
        let keys = keys_after(&edit).unwrap_or_else(|| editor.keys.clone());
        let previous_keys = std::mem::replace(&mut editor.keys, keys);
        editor.undo.push((label, previous, previous_keys, scope));
        if editor.undo.len() > MAX_UNDO {
            editor.undo.remove(0);
        }
        editor.redo.clear();
        Self::adopt_keys(doc);
        let refreshed = guard(|| if scope == Scope::Content { Self::refresh_content(doc, Some(&edit)) } else { Self::refresh_scoped(doc, scope) })
            .unwrap_or_else(|m| Err(EditError::Reopen(m)));
        if let Err(e) = refreshed {
            // Roll back: the edit produced something we cannot display.
            if let Some(ed) = doc.editor.as_mut()
                && let Some((_, prev, keys, _)) = ed.undo.pop()
            {
                ed.cos = prev;
                ed.keys = keys;
            }
            Self::adopt_keys(doc);
            if !matches!(guard(|| Self::refresh(doc)), Ok(Ok(()))) {
                Self::show_released(doc);
            }
            return Err(e);
        }
        doc.dirty = true;
        doc.generation += 1;
        doc.edit_notices = notices;
        if !cx.xfa_removed.is_empty() {
            let removed: Vec<String> = cx.xfa_removed.iter().map(|n| format!("{XFA_REMOVED_PREFIX}{n}")).collect();
            doc.xfa_warnings.retain(|w| w.starts_with(XFA_REMOVED_PREFIX));
            note_warnings(&mut doc.xfa_warnings, &removed);
            note_warnings(&mut doc.info.warnings, &removed);
        }
        note_warnings(&mut doc.xfa_warnings, &xfa_notes);
        let notes = doc.xfa_warnings.clone();
        note_warnings(&mut doc.info.warnings, &notes);
        if let Some(js) = cx.js {
            doc.js_output.append(js.output);
        }
        doc.js_output.append(cx.xfa_out);
        if cx.xfa_ran_away {
            doc.xfa_template = None;
            doc.xfa_scripts_off = true;
            doc.js_output.errors.push(XFA_SCRIPTS_OFF.into());
        }
        Ok(())
    }

    pub fn undo(&mut self, id: DocId) -> Result<Label, EditError> {
        let doc = self.doc_mut(id)?;
        let editor = doc.editor.as_mut().ok_or(EditError::NothingToUndo)?;
        let (label, prev, keys, scope) = editor.undo.pop().ok_or(EditError::NothingToUndo)?;
        let current = std::mem::replace(&mut editor.cos, prev);
        let current_keys = std::mem::replace(&mut editor.keys, keys);
        editor.redo.push((label.clone(), current, current_keys, scope));
        Self::adopt_keys(doc);
        let refreshed = guard(|| Self::refresh_scoped(doc, scope)).unwrap_or_else(|m| Err(EditError::Reopen(m)));
        if let Err(e) = refreshed {
            if let Some(ed) = doc.editor.as_mut()
                && let Some((label, current, current_keys, scope)) = ed.redo.pop()
            {
                let prev = std::mem::replace(&mut ed.cos, current);
                let keys = std::mem::replace(&mut ed.keys, current_keys);
                ed.undo.push((label, prev, keys, scope));
            }
            Self::adopt_keys(doc);
            if !matches!(guard(|| Self::refresh(doc)), Ok(Ok(()))) {
                Self::show_released(doc);
            }
            return Err(e);
        }
        doc.edit_notices.clear();
        Self::sync_dirty(doc);
        doc.generation += 1;
        Ok(label)
    }

    pub fn redo(&mut self, id: DocId) -> Result<Label, EditError> {
        let doc = self.doc_mut(id)?;
        let editor = doc.editor.as_mut().ok_or(EditError::NothingToRedo)?;
        let (label, next, keys, scope) = editor.redo.pop().ok_or(EditError::NothingToRedo)?;
        let current = std::mem::replace(&mut editor.cos, next);
        let current_keys = std::mem::replace(&mut editor.keys, keys);
        editor.undo.push((label.clone(), current, current_keys, scope));
        Self::adopt_keys(doc);
        let refreshed = guard(|| Self::refresh_scoped(doc, scope)).unwrap_or_else(|m| Err(EditError::Reopen(m)));
        if let Err(e) = refreshed {
            if let Some(ed) = doc.editor.as_mut()
                && let Some((label, current, current_keys, scope)) = ed.undo.pop()
            {
                let next = std::mem::replace(&mut ed.cos, current);
                let keys = std::mem::replace(&mut ed.keys, current_keys);
                ed.redo.push((label, next, keys, scope));
            }
            Self::adopt_keys(doc);
            if !matches!(guard(|| Self::refresh(doc)), Ok(Ok(()))) {
                Self::show_released(doc);
            }
            return Err(e);
        }
        doc.edit_notices.clear();
        Self::sync_dirty(doc);
        doc.generation += 1;
        Ok(label)
    }

    /// Unsaved when the working graph is not the last opened or saved file. Undo back to that
    /// file is clean; undo past a later save is not, because the restored snapshot's own base
    /// is an earlier revision.
    pub(super) fn sync_dirty(doc: &mut Document) {
        doc.dirty = doc.editor.as_ref().is_some_and(|e| e.unsaved_file || e.cos.is_modified() || !Arc::ptr_eq(e.cos.bytes(), &e.saved));
    }

    /// Use the current state's password for viewing (protection edits change it).
    pub(super) fn adopt_keys(doc: &mut Document) {
        if let Some(e) = doc.editor.as_ref() {
            doc.password = e.keys.render.clone();
            doc.config.password = e.keys.render.as_deref().map(Arc::from);
        }
    }

    /// Rebuild the working bytes and the view data that `scope` may have changed. Metadata edits
    /// keep the renderer; other scopes also replace it.
    /// Comment and form edits skip the full re-inspection (seconds on very large files): the
    /// comment list and field values are re-read from the object graph instead.
    fn refresh_scoped(doc: &mut Document, scope: Scope) -> Result<(), EditError> {
        if scope == Scope::Full || doc.info.encrypted != doc.editor.as_ref().is_some_and(|e| e.cos.output_handler().is_some()) {
            return Self::refresh(doc);
        }
        if scope == Scope::Content {
            return Self::refresh_content(doc, None);
        }
        if scope == Scope::Links {
            return Self::refresh_links(doc);
        }
        // Metadata edits keep the renderer, which holds the working file anyway.
        if scope != Scope::Metadata {
            Self::release_working(doc);
        }
        let Some(editor) = doc.editor.as_mut() else { return Ok(()) };
        // Comment edits, and form edits (a script may change annotation properties), may change
        // annotation dictionaries, invalidating cached appearances.
        if matches!(scope, Scope::Comments | Scope::Form) {
            editor.appearances.clear();
        }
        let bytes = if editor.cos.is_modified() {
            Arc::new(write_incremental(&editor.cos, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string()))?)
        } else {
            editor.cos.bytes().clone()
        };
        if scope == Scope::Metadata {
            for (field, value) in info_fields(&mut doc.info, &editor.cos) {
                *field = value;
            }
            doc.info.custom = pdfcraft_organize::custom_info(&editor.cos);
            doc.info.file_size = bytes.len();
            #[cfg(test)]
            check_refresh_panic();
            if !doc.signatures.is_empty() {
                let signatures = signatures_of(&editor.cos, &bytes, &doc.trust, &doc.sig_cache);
                doc.signatures = Arc::new(signatures);
            }
            doc.bytes = bytes;
            return Ok(());
        }
        let mut form = pdfcraft_forms::fields(&editor.cos);
        xfa::mark_script_buttons(&editor.cos, &mut form);
        let form = Arc::new(form);
        match scope {
            Scope::Metadata => return Ok(()),
            Scope::Comments => {
                doc.info.annotations = comment_list(&editor.cos);
                // Link indexes are positions in /Annots, so comment insertion/deletion can move them.
                doc.links = OnceLock::new();
            }
            Scope::Form => {
                // PERF: keystroke path, index once. A linear find per field is O(F^2);
                // forms reach 10k fields.
                let mut by_name = std::collections::HashMap::with_capacity(form.len());
                for x in form.iter() {
                    // First match wins (do not simplify to collect: it keeps the last).
                    by_name.entry(x.name.as_str()).or_insert(x);
                }
                for f in &mut doc.info.fields {
                    if let Some(ff) = by_name.get(f.name.as_str()) {
                        f.value = match ff.kind {
                            pdfcraft_forms::FieldKind::CheckBox | pdfcraft_forms::FieldKind::Radio => {
                                Some(ff.value.first().cloned().unwrap_or_else(|| "Off".into()))
                            }
                            _ if ff.value.is_empty() => None,
                            _ => Some(ff.value.join(", ")),
                        };
                    }
                }
            }
            Scope::Links => return Self::refresh_links(doc),
            // Handled above; kept as a full refresh so the match stays total.
            Scope::Full => return Self::refresh(doc),
            // Handled above (content-only refresh); the arm keeps the match total.
            Scope::Content => return Self::refresh_content(doc, None),
        }
        doc.info.file_size = bytes.len();
        doc.form = form;
        let signatures = if doc.signatures.is_empty() { Vec::new() } else { signatures_of(&editor.cos, &bytes, &doc.trust, &doc.sig_cache) };
        doc.display = display_bytes(Some(editor), &bytes);
        doc.signatures = Arc::new(signatures);
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.bytes = bytes;
        Ok(())
    }

    /// Which derived lists a content-only edit can change: added content (`None`, from
    /// undo/redo, re-reads both: always correct, rarely paid).
    pub(crate) fn content_refresh_lists(edit: Option<&Edit>) -> (bool, bool) {
        if let Some(Edit::Batch { edits, .. }) = edit {
            // A batch of content edits (from a script, say) changes what its parts change.
            return edits.iter().fold((false, false), |(added, marks), part| {
                let (a, m) = Self::content_refresh_lists(Some(part));
                (added || a, marks || m)
            });
        }
        let touches_added = edit.is_none_or(|e| {
            matches!(
                e,
                Edit::AddText { .. }
                    | Edit::AddImage { .. }
                    | Edit::UpdateContent { .. }
                    | Edit::DeleteContent { .. }
                    | Edit::ReplaceImage { .. }
                    | Edit::PasteContent { .. }
            ) || matches!(e, Edit::MoveObjects { objects, .. } if objects.iter().any(|o| o.kind == ObjectKind::Added))
        });
        let touches_marks = edit.is_none_or(|e| {
            matches!(e, Edit::AddHeaderFooter { .. } | Edit::AddWatermark { .. } | Edit::AddBackground { .. } | Edit::RemoveMarks { .. })
        });
        (touches_added, touches_marks)
    }

    /// Rebuild working bytes, display bytes and renderer after a content-only edit: page
    /// content streams, resources and added content/marks. The page tree, annotations, form
    /// fields, links, outline and fonts are unchanged, so the full re-inspection (seconds on
    /// very large files) is skipped. `edit` selects which derived lists are re-read (`None`,
    /// from undo/redo, re-reads both: always correct, rarely paid). The cached display
    /// appearances are kept: content edits leave every annotation dictionary alone.
    fn refresh_content(doc: &mut Document, edit: Option<&Edit>) -> Result<(), EditError> {
        Self::release_working(doc);
        sync_xfa_state(doc);
        let Some(editor) = doc.editor.as_mut() else { return Ok(()) };
        let bytes = if editor.cos.is_modified() {
            Arc::new(write_incremental(&editor.cos, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string()))?)
        } else {
            editor.cos.bytes().clone()
        };
        // The edit must still leave a file the viewer can open; anything else rolls back above.
        pdfcraft_render::check_pages(bytes.clone(), doc.password.as_deref(), doc.info.pages.len()).map_err(|e| EditError::Reopen(e.to_string()))?;
        let (touches_added, touches_marks) = Self::content_refresh_lists(edit);
        if touches_added || touches_marks {
            doc.marker_scan = OnceLock::new();
        }
        doc.info.file_size = bytes.len();
        let signatures = if doc.signatures.is_empty() { Vec::new() } else { signatures_of(&editor.cos, &bytes, &doc.trust, &doc.sig_cache) };
        doc.display = display_bytes(Some(editor), &bytes);
        doc.signatures = Arc::new(signatures);
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.bytes = bytes;
        doc.fonts_stale = true;
        Ok(())
    }

    /// Let go of the working file before the next one is written (gap 13). The working file is
    /// the original plus one appended revision, so on a 1 GiB file keeping the old one (in the
    /// working bytes, the display bytes and the renderer) while writing the new one held three
    /// copies at once; this keeps two. Until the refresh finishes, the working and display bytes
    /// are the original's (already held by the editor, so free) and the renderer draws nothing.
    /// A failed edit rolls back and refreshes again, which replaces all three; where a refresh
    /// still fails, [`Self::show_released`] draws the original rather than nothing. Save writes
    /// from the editor, never from these.
    pub(crate) fn release_working(doc: &mut Document) {
        let Some(original) = doc.editor.as_ref().map(|e| e.cos.bytes().clone()) else { return };
        // Dropping the pool stops its workers; each lets go of the file when it exits.
        doc.renderer = RenderPool::new_inline(Arc::new(Vec::new()), doc.config.clone());
        doc.display = original.clone();
        doc.bytes = original;
        #[cfg(test)]
        check_refresh_panic();
    }

    /// After a refresh failed: draw the display bytes it left (the original, once released),
    /// so the page still shows a document.
    pub(super) fn show_released(doc: &mut Document) {
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
    }

    /// Rebuild working bytes, inspection and renderer from the current edit state.
    pub(crate) fn refresh(doc: &mut Document) -> Result<(), EditError> {
        Self::refresh_with_indexes(doc, true, true)
    }

    /// Reinspect a link edit while preserving the page-content marker scan cache.
    fn refresh_links(doc: &mut Document) -> Result<(), EditError> {
        Self::refresh_with_indexes(doc, false, true)
    }

    fn refresh_with_indexes(doc: &mut Document, invalidate_markers: bool, invalidate_links: bool) -> Result<(), EditError> {
        // A full refresh may follow structural or annotation changes: cached appearances
        // may be stale, so they are rebuilt below.
        if let Some(editor) = doc.editor.as_mut() {
            editor.appearances.clear();
        }
        Self::release_working(doc);
        sync_xfa_state(doc);
        let Some(editor) = doc.editor.as_mut() else { return Ok(()) };
        let bytes = if editor.cos.is_modified() {
            Arc::new(write_incremental(&editor.cos, &SaveOptions::default()).map_err(|e| EditError::Write(e.to_string()))?)
        } else {
            editor.cos.bytes().clone()
        };
        let info = inspect(bytes.clone(), doc.password.as_deref()).map_err(|e| EditError::Reopen(e.to_string()))?;
        // Keep the user's layer choices where the layers still exist.
        let mut info = info;
        for l in &mut info.layers {
            if let Some(old) = doc.info.layers.iter().find(|o| o.id == l.id) {
                l.visible = old.visible;
            }
        }
        note_warnings(&mut info.warnings, &doc.xfa_warnings);
        // As in `build_document`: the inspection reads only `/Info`, so fields stated only in XMP
        // are filled in again after every full refresh.
        for (field, value) in info_fields(&mut info, &editor.cos) {
            if field.is_none() {
                *field = value;
            }
        }
        doc.info = info;
        doc.fonts_stale = false;
        let mut form = pdfcraft_forms::fields(&editor.cos);
        xfa::mark_script_buttons(&editor.cos, &mut form);
        doc.form = Arc::new(form);
        if invalidate_markers {
            doc.marker_scan = OnceLock::new();
        }
        if invalidate_links {
            doc.links = OnceLock::new();
        }
        let signatures = signatures_of(&editor.cos, &bytes, &doc.trust, &doc.sig_cache);
        doc.display = display_bytes(Some(editor), &bytes);
        doc.signatures = Arc::new(signatures);
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.bytes = bytes;
        Ok(())
    }
}
