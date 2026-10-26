//! The session: the open documents and the operations on them.

use std::sync::{Arc, OnceLock};

use pdfcraft_cos::{SaveOptions, write_full, write_incremental};
use pdfcraft_render::{LayerOp, OpenError, ReadingColors, RenderConfig, RenderPool, inspect};

#[cfg(test)]
use crate::check_refresh_panic;
use crate::edit::{EditCtx, check_permission, run_edit};
use crate::history::{Keys, MAX_UNDO, Scope, keys_after, scope_of, uses_scripts};
use crate::{
    BookmarkPage, BookmarkRow, CombineSource, Combined, DataFormat, DocId, Document, Edit, EditError, Hidden, ImageResolution,
    LOW_RESOLUTION_PRINT_DPI, Label, ObjectKind, OptimizeReport, RecoverySnapshot, SignOptions, SourceKind, SplitPart, SummarySort, TrustStore,
    XFA_REMOVED_PREFIX, XFA_SCRIPTS_OFF, apply_layer_state, comment_list, comment_summary, decode_text, display_bytes, guard, info_fields, js,
    local_utc_offset, note_warnings, open_source, open_source_with, optimize, outline_rows, parse_ymd, print, print_raster, render_threads,
    scan_colours, set_default_layer_state, shown_under_redactions, signatures_of, source_kind, sync_xfa_state, undrawn_notices, use_layer_choices,
    writable, xfa, xfa_sync_datasets,
};

#[derive(Default)]
pub struct Session {
    pub(crate) docs: Vec<Document>,
    pub(crate) next_id: u64,
    /// Seconds since the Unix epoch, injected so saves are deterministic in tests.
    clock: Option<fn() -> i64>,
    /// Seconds to add to UTC for local time, read from whoever knows the zone (a browser, say).
    /// Separate from `clock` on purpose: `clock` alone is the test shortcut that takes the
    /// injected time AS local time and zeroes the zone, so injecting only it leaves either the
    /// local dates or the UTC ones wrong by the zone's offset.
    utc_offset: Option<fn() -> i64>,
    /// Certificates trusted for signing (Acrobat: Trusted Certificates).
    pub(crate) trust: Arc<TrustStore>,
    /// Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript, inverted (on by default).
    pub(crate) js_off: bool,
    /// Preferences ▸ Date format, when not the default (see [`dates`]).
    pub(crate) date_format: Option<String>,
    /// Preferences ▸ Date format ▸ Language, when not following the interface language.
    pub(crate) date_language: Option<String>,
    /// What the XFA scripts' time limits are multiplied by (`None`: 1); see
    /// [`Session::with_xfa_time_scale`].
    pub(crate) xfa_time_scale: Option<u32>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a fixed clock (tests) instead of the system time.
    pub fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Multiply the time an XFA form's scripts may take by `factor` (tests): the 3 s the
    /// scripts of an opening or a change share, the 5 s of a click's, the 10 s the script
    /// engine has to start, and each script's own 1 s or 3 s. A test that checks what the
    /// scripts do passes a large factor, so that a busy machine does not stop them. On native
    /// builds, 0 stops every script before it starts; the wasm build has no wall clock, so
    /// there the factor has no effect.
    pub fn with_xfa_time_scale(mut self, factor: u32) -> Self {
        self.xfa_time_scale = Some(factor);
        self
    }

    /// A real clock for a host that has neither a clock nor a time zone of its own (a
    /// `wasm32-unknown-unknown` build: `now()` would be empty and `local_utc_offset()` is 0, so
    /// Fill & Sign stamps 1 Jan 1970 and `/ModDate` is never written). `clock` is the UTC time in
    /// seconds since the epoch; `utc_offset` is the local zone's seconds east of UTC, asked on every
    /// use because the zone changes with the date (daylight saving). Unlike
    /// [`Session::with_clock`], which is a test shortcut, nothing here is taken as local time.
    pub fn with_wall_clock(mut self, clock: fn() -> i64, utc_offset: fn() -> i64) -> Self {
        self.clock = Some(clock);
        self.utc_offset = Some(utc_offset);
        self
    }

    /// Seconds to add to UTC for the local time of this session: the injected zone, else 0 under
    /// the test clock (which is already local), else the system's.
    fn local_offset(&self) -> i64 {
        match (self.utc_offset, self.clock) {
            (Some(zone), _) => zone(),
            (None, Some(_)) => 0,
            (None, None) => local_utc_offset(),
        }
    }

    /// Seconds since the Unix epoch from the session clock (0 when unknown).
    pub fn now_secs(&self) -> i64 {
        self.now().unwrap_or(0)
    }

    pub(crate) fn now(&self) -> Option<i64> {
        if let Some(c) = self.clock {
            return Some(c());
        }
        #[cfg(not(target_arch = "wasm32"))]
        return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64);
        #[cfg(target_arch = "wasm32")]
        return None;
    }

    /// Today's date in local time: (year, month, day). With an injected clock (tests) the clock
    /// is taken as local time.
    /// A dynamic stamp's second line: "By Ada at 2:14 pm, Oct 02, 2026" (local time).
    pub fn stamp_by_line(&self, author: &str) -> String {
        let offset = self.local_offset();
        let d = self.now().map(|t| pdfcraft_cos::pdf_date(t + offset)).unwrap_or_default();
        let num = |a: usize, b: usize| d.get(a..b).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let (y, mo, day, hh, mm) = (num(2, 6), num(6, 8), num(8, 10), num(10, 12), num(12, 14));
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let h12 = if hh % 12 == 0 { 12 } else { hh % 12 };
        let who = if author.trim().is_empty() { String::new() } else { format!("By {} ", author.trim()) };
        format!("{who}at {h12}:{mm:02} {}, {} {day:02}, {y}", if hh < 12 { "am" } else { "pm" }, MONTHS[(mo.clamp(1, 12) - 1) as usize])
    }

    pub fn today(&self) -> (i64, u32, u32) {
        let offset = self.local_offset();
        self.now().map(|t| pdfcraft_cos::pdf_date(t + offset)).as_deref().and_then(parse_ymd).unwrap_or((1970, 1, 1))
    }

    /// Open a document from bytes. Rendering starts lazily when pages are requested.
    ///
    /// `password` is tried as either the user or owner password when the file is encrypted.
    /// Open saved revision `n` (1 = the oldest) of a document as a new, unsaved document named
    /// "<name> (revision n)".
    pub fn open_revision(&mut self, id: DocId, n: usize) -> Result<DocId, String> {
        let doc = self.get(id).ok_or("no such document")?;
        let ends = doc.revision_ends();
        let end = *n.checked_sub(1).and_then(|i| ends.get(i)).ok_or_else(|| format!("the document has {} revision(s)", ends.len()))?;
        let name = format!("{} (revision {n}).pdf", doc.name.trim_end_matches(".pdf"));
        let (bytes, password) = (Arc::new(doc.bytes[..end.min(doc.bytes.len())].to_vec()), doc.password.clone());
        self.open(name, None, bytes, password.as_deref()).map_err(|e| e.to_string())
    }

    pub(crate) fn doc_mut(&mut self, id: DocId) -> Result<&mut Document, EditError> {
        self.docs.iter_mut().find(|d| d.id == id).ok_or(EditError::NoDocument)
    }

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
    fn sync_dirty(doc: &mut Document) {
        doc.dirty = doc.editor.as_ref().is_some_and(|e| e.unsaved_file || e.cos.is_modified() || !Arc::ptr_eq(e.cos.bytes(), &e.saved));
    }

    /// Use the current state's password for viewing (protection edits change it).
    fn adopt_keys(doc: &mut Document) {
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
    fn show_released(doc: &mut Document) {
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
        // How the document is shown is the reader's, not the file's: reading colours and hidden
        // comments stay as they were (#3443). `open` built the renderer with the defaults.
        if let Some(old) = self.docs.get(old)
            && (doc.config.reading_colors != old.config.reading_colors || doc.config.hide_comments != old.config.hide_comments)
        {
            doc.config.reading_colors = old.config.reading_colors;
            doc.config.hide_comments = old.config.hide_comments;
            doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        }
        // Caches keyed by the edit generation (edit lines, images, measurements) must see a new
        // one; there is nothing new to autosave.
        doc.generation = generation + 1;
        doc.snapshot_generation = doc.generation;
        // `open` pushed the new version last, so the old one is still at `old`.
        *self.docs.get_mut(old).ok_or_else(|| OpenError::Invalid("the document is not open".into()))? = doc;
        Ok(())
    }

    /// The page count of another PDF (Replace Pages, Insert Pages dialogs).
    pub fn page_count_of(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<usize, EditError> {
        Ok(pdfcraft_organize::page_count(&open_source(name, bytes)?)?)
    }

    /// A new blank document (Create ▸ Blank page).
    pub fn create_blank(&self, width: f64, height: f64, pages: usize) -> Result<Arc<Vec<u8>>, EditError> {
        self.write_new(&pdfcraft_create::blank(width, height, pages)?)
    }

    /// A new document with one page per image (PNG, JPEG).
    pub fn create_from_images(&self, images: &[(String, Vec<u8>)]) -> Result<Arc<Vec<u8>>, EditError> {
        self.write_new(&pdfcraft_create::from_images(images)?)
    }

    /// Create image pages at embedded resolution or a fixed dpi, without resampling.
    pub fn create_from_images_with_resolution(&self, images: &[(String, Vec<u8>)], resolution: ImageResolution) -> Result<Arc<Vec<u8>>, EditError> {
        self.write_new(&pdfcraft_create::from_images_with_resolution(images, resolution)?)
    }

    /// A new document from plain text (US Letter, 11 pt Helvetica). Text Helvetica can't draw is
    /// refused, naming the line and the character (never set as `?`).
    pub fn create_from_text(&self, title: &str, text: &str) -> Result<Arc<Vec<u8>>, EditError> {
        self.write_new(&pdfcraft_create::from_text(title, text, pdfcraft_create::LETTER, 11.0)?)
    }

    /// [`Self::create_from_text`] for a report PdfCraft writes from documents (comment summaries,
    /// compare reports), which can quote any language: characters Helvetica can't draw are written
    /// as `[U+XXXX]`, and the report's first line says so, rather than the report being refused.
    pub(crate) fn create_report(&self, title: &str, text: &str) -> Result<Arc<Vec<u8>>, EditError> {
        let (doc, _escaped) = pdfcraft_create::from_text_escaped(title, text, pdfcraft_create::LETTER, 11.0)?;
        self.write_new(&doc)
    }

    /// Convert a file Create understands (an image or plain text) to PDF bytes; a PDF is checked
    /// (it must open and allow copying pages) and returned as it is.
    pub fn convert_to_pdf(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<(SourceKind, Arc<Vec<u8>>), EditError> {
        let Some(kind) = source_kind(name, bytes) else {
            return Err(EditError::Source(format!("{name}: this file type can't be converted; use a PDF, an image or a .txt file")));
        };
        let title = name.rsplit_once('.').map_or(name, |(s, _)| s);
        // Image decoders read untrusted bytes: a panic in one must not take the app down.
        let created = guard(|| match kind {
            SourceKind::Pdf => open_source(name, bytes).map(|_| bytes.clone()),
            SourceKind::Image => self.create_from_images(&[(name.to_string(), bytes.to_vec())]),
            SourceKind::Text => self.create_from_text(title, &decode_text(bytes)),
        })
        .map_err(|_| EditError::Source(format!("{name}: the file could not be read")))?;
        Ok((kind, created?))
    }

    /// A file for Combine as PDF bytes: an image or text file is converted; a PDF is returned as
    /// it is, unchecked (Combine reads it itself, with its password), as is a file named .pdf
    /// without a `%PDF-` header, which the reader may still open.
    pub fn as_pdf(&self, name: &str, bytes: &Arc<Vec<u8>>) -> Result<Arc<Vec<u8>>, EditError> {
        let pdf = match source_kind(name, bytes) {
            Some(SourceKind::Pdf) => true,
            Some(SourceKind::Image | SourceKind::Text) => false,
            None => name.to_ascii_lowercase().ends_with(".pdf"),
        };
        if pdf { Ok(bytes.clone()) } else { self.convert_to_pdf(name, bytes).map(|(_, pdf)| pdf) }
    }

    /// Reduce File Size: Acrobat's defaults (images above 225 ppi to 150 ppi, JPEG medium
    /// quality; thumbnails dropped), identical resources merged, unused objects dropped, objects
    /// packed into compressed object streams. Returns the bytes and how many objects were
    /// merged. The open document is not changed (Acrobat saves the reduced copy as a new file).
    pub fn reduced_bytes(&self, id: DocId) -> Result<(Arc<Vec<u8>>, usize), EditError> {
        let (bytes, report) = self.optimized_bytes(id, &optimize::Settings::default(), &[])?;
        Ok((bytes, report.merged))
    }

    /// Optimize PDF ▸ Advanced optimization: `settings` for images and objects, plus Remove
    /// Hidden Information's `discard` categories (user data). A full rewrite: signed documents
    /// are refused. The open document is not changed. This is [`Self::optimize_job`] run in
    /// place, without progress.
    pub fn optimized_bytes(&self, id: DocId, settings: &optimize::Settings, discard: &[Hidden]) -> Result<(Arc<Vec<u8>>, OptimizeReport), EditError> {
        self.optimize_job(id, settings, discard)?.run(|_| true)
    }

    /// Export comments and/or form data: XFDF and FDF carry either or both; XML, CSV and text
    /// are form data only.
    pub fn export_data(&self, id: DocId, format: DataFormat, comments: bool, fields: bool) -> Result<Vec<u8>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = match doc.editor.as_ref() {
            Some(e) => e.cos.clone(),
            None => {
                pdfcraft_cos::Document::open_with_password(doc.bytes.clone(), doc.password.as_deref()).map_err(|e| EditError::Write(e.to_string()))?
            }
        };
        // An XFA form's data is its datasets, as Acrobat exports it: XML or an XDP.
        let xfa = doc.info.xfa.is_some();
        if format == DataFormat::Xdp && !xfa {
            return Err(EditError::Invalid("XDP holds XFA form data; this document is not an XFA form (use .xml, .csv or .txt)".into()));
        }
        guard(|| match format {
            DataFormat::Xfdf => Ok(pdfcraft_xfdf::export_xfdf(&cos, comments, fields, &doc.name).into_bytes()),
            DataFormat::Fdf => pdfcraft_xfdf::export_fdf(&cos, comments, fields, &doc.name).map_err(|e| e.to_string()),
            DataFormat::Xml | DataFormat::Xdp if xfa => {
                pdfcraft_xfa::export_data(&cos, format == DataFormat::Xdp, &doc.name).map(String::into_bytes).map_err(|e| e.to_string())
            }
            other => Ok(pdfcraft_xfdf::export_data(&cos, other).into_bytes()),
        })
        .map_err(EditError::Write)?
        .map_err(EditError::Write)
    }

    /// The print-ready PDF for `settings` (sheets laid out for the paper; see `pdfcraft-print`).
    pub fn print_pdf(&self, id: DocId, settings: &print::Settings) -> Result<Vec<u8>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if !doc.allows_printing() {
            return Err(EditError::NotPermitted("printing"));
        }
        // Table 22, bit 12 clear: only a low-level representation may be printed, so the pages
        // always go as pictures, at no more than the low resolution.
        let low_res = doc.permissions().is_some_and(|p| !p.print_high_quality()).then(|| print::Raster {
            dpi: settings.raster.map_or(LOW_RESOLUTION_PRINT_DPI, |r| r.dpi.min(LOW_RESOLUTION_PRINT_DPI)),
            grayscale: settings.raster.is_some_and(|r| r.grayscale),
        });
        // Print as image: the pages used become pictures first, then the picture document is
        // imposed in their place (its page k is the k-th distinct source page used).
        if let Some(raster) = low_res.as_ref().or(settings.raster.as_ref()) {
            let mut distinct: Vec<usize> = Vec::new();
            for &p in &settings.pages {
                if !distinct.contains(&p) {
                    distinct.push(p);
                }
            }
            let bytes = print_raster::rasterize(doc, &distinct, raster, settings.content).map_err(EditError::Print)?;
            let pictures = pdfcraft_cos::Document::open(Arc::new(bytes)).map_err(|e| EditError::Print(e.to_string()))?;
            let remapped = print::Settings {
                pages: settings.pages.iter().map(|p| distinct.iter().position(|d| d == p).unwrap_or(0)).collect(),
                content: print::Content::Document,
                raster: None,
                ..settings.clone()
            };
            return pdfcraft_print::impose(&pictures, &remapped).map_err(|e| EditError::Print(e.to_string()));
        }
        let mut cos = match doc.editor.as_ref() {
            Some(e) => e.cos.clone(),
            None => {
                pdfcraft_cos::Document::open_with_password(doc.bytes.clone(), doc.password.as_deref()).map_err(|e| EditError::Write(e.to_string()))?
            }
        };
        // #3046: the layers print as they are shown. Once the Layers panel (or a set-layer
        // action) has changed one, the print copy's default configuration says which are on, so a
        // hidden layer stays off on paper too; untouched, the document's own configuration prints.
        if !doc.config.layers.is_empty() {
            set_default_layer_state(&mut cos, &doc.info.layers);
        }
        pdfcraft_print::impose(&cos, settings).map_err(|e| EditError::Print(e.to_string()))
    }

    /// For each page, whether its text mostly runs bottom-to-top ([`pdfcraft_print::text_reads_up`]);
    /// only `pages` are read, the others are `false`.
    pub fn print_text_up(&self, id: DocId, pages: &[usize]) -> Vec<bool> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let cos = match doc.editor.as_ref() {
            Some(e) => e.cos.clone(),
            None => match pdfcraft_cos::Document::open_with_password(doc.bytes.clone(), doc.password.as_deref()) {
                Ok(c) => c,
                Err(_) => return Vec::new(),
            },
        };
        (0..doc.info.pages.len()).map(|i| pages.contains(&i) && pdfcraft_print::text_reads_up(&cos, i)).collect()
    }

    /// Combine whole files, in order, into new PDF bytes (one bookmark per file).
    pub fn combine(&self, sources: &[(String, Arc<Vec<u8>>)]) -> Result<Arc<Vec<u8>>, EditError> {
        let docs = sources.iter().map(|(n, b)| open_source(n, b)).collect::<Result<Vec<_>, _>>()?;
        let named: Vec<(&str, &pdfcraft_cos::Document)> = sources.iter().map(|(n, _)| n.as_str()).zip(docs.iter()).collect();
        let out = pdfcraft_organize::combine(&named)?;
        self.write_new(&out)
    }

    /// Combine Files with a page range per file ("1-3, 6"; `None` or empty for all pages).
    pub fn combine_ranges(&self, sources: &[CombineSource]) -> Result<Arc<Vec<u8>>, EditError> {
        self.combine_unlocked(sources, &[])
    }

    /// Combine sources one at a time, in order (Create ▸ Multiple files), to the same result as
    /// [`Self::combine_ranges`]. Each item is a source's bookmark title, its PDF bytes (see
    /// [`Self::convert_to_pdf`]) and its page range (`None` for all pages). Items are pulled one at a
    /// time: a source is opened, its pages are copied into the result, and its bytes are released
    /// before the next item is read. So the number of sources is not limited, and one source's
    /// document is held at a time. An item's error stops the combine and is returned as it is.
    /// `Ok(None)` when there are no sources.
    pub fn combine_each<I, E>(&self, sources: I) -> Result<Option<Arc<Vec<u8>>>, E>
    where
        I: IntoIterator<Item = Result<CombineSource, E>>,
        E: From<EditError>,
    {
        let mut combiner = pdfcraft_organize::Combiner::new();
        let mut added = false;
        for source in sources {
            let (title, bytes, range) = source?;
            let doc = open_source(&title, &bytes)?;
            let range = range.as_deref().map(str::trim).filter(|r| !r.is_empty());
            let pages = match range {
                Some(r) => {
                    let n = pdfcraft_organize::page_count(&doc).map_err(EditError::from)?;
                    let p = pdfcraft_print::select_pages(n, Some(r), &[], pdfcraft_print::Subset::All, false)
                        .map_err(|e| EditError::Print(format!("{title}: {e}")))?;
                    Some(p)
                }
                None => None,
            };
            combiner.add(&title, &doc, pages.as_deref()).map_err(EditError::from)?;
            added = true;
        }
        if !added {
            return Ok(None);
        }
        let out = combiner.finish().map_err(EditError::from)?;
        Ok(Some(self.write_new(&out)?))
    }

    /// [`Self::combine_ranges`] with the password each encrypted source is opened with, by
    /// position (missing or `None`: no password). The result is not encrypted.
    pub fn combine_unlocked(&self, sources: &[CombineSource], passwords: &[Option<&str>]) -> Result<Arc<Vec<u8>>, EditError> {
        Ok(self.combine_unlocked_reporting(sources, passwords, pdfcraft_organize::FieldConflicts::Rename)?.bytes)
    }

    /// The pre-flight over byte sources: what combining them, in order, would rename in each one
    /// (whole files; page ranges are the combine's to apply). A source that cannot be opened
    /// previews as nothing — its row already says why it cannot be combined.
    pub fn combine_field_conflicts(&self, sources: &[CombineSource], passwords: &[Option<&str>]) -> Vec<Vec<(String, String)>> {
        let docs: Vec<Option<pdfcraft_cos::Document>> =
            sources.iter().enumerate().map(|(i, (n, b, _))| open_source_with(n, b, passwords.get(i).copied().flatten()).ok()).collect();
        let refs: Vec<&pdfcraft_cos::Document> = docs.iter().filter_map(Option::as_ref).collect();
        let mut opened = pdfcraft_organize::combine_field_conflicts(&refs).into_iter();
        // One preview per source, in step with it: a file that cannot be opened has none.
        docs.iter().map(|d| if d.is_some() { opened.next().unwrap_or_default() } else { Default::default() }).collect()
    }

    /// [`Self::combine_unlocked`] reporting what the conflict resolution renamed.
    pub fn combine_unlocked_reporting(
        &self,
        sources: &[CombineSource],
        passwords: &[Option<&str>],
        field_conflicts: pdfcraft_organize::FieldConflicts,
    ) -> Result<Combined, EditError> {
        let docs = sources
            .iter()
            .enumerate()
            .map(|(i, (n, b, _))| open_source_with(n, b, passwords.get(i).copied().flatten()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut pages = Vec::with_capacity(docs.len());
        for ((name, _, range), d) in sources.iter().zip(&docs) {
            let range = range.as_deref().map(str::trim).filter(|r| !r.is_empty());
            pages.push(match range {
                Some(r) => {
                    let n = pdfcraft_organize::page_count(d)?;
                    let p = pdfcraft_print::select_pages(n, Some(r), &[], pdfcraft_print::Subset::All, false)
                        .map_err(|e| EditError::Print(format!("{name}: {e}")))?;
                    Some(p)
                }
                None => None,
            });
        }
        let named: Vec<(&str, &pdfcraft_cos::Document, Option<&[usize]>)> =
            sources.iter().zip(docs.iter()).zip(&pages).map(|(((n, _, _), d), p)| (n.as_str(), d, p.as_deref())).collect();
        let out = pdfcraft_organize::combine_selected_with(&named, field_conflicts)?;
        let bytes = self.write_new(&out.doc)?;
        Ok(Combined { bytes, renamed_fields: out.renamed_fields })
    }

    /// [`Self::combine_unlocked`] where sources with the same `groups` key (by position) are one
    /// file split into several runs placed apart: each file is opened and copied once, so it keeps
    /// one bookmark, its links between its own pages, its fields and its attachments whole (see
    /// `pdfcraft_organize::combine_grouped`). A group's later sources use its first source's bytes
    /// and password; a source without a key is a file of its own.
    pub fn combine_grouped(&self, sources: &[CombineSource], groups: &[u64], passwords: &[Option<&str>]) -> Result<Arc<Vec<u8>>, EditError> {
        Ok(self.combine_grouped_reporting(sources, groups, passwords, pdfcraft_organize::FieldConflicts::Rename)?.bytes)
    }

    /// [`Self::combine_grouped`] reporting what the conflict resolution renamed.
    pub fn combine_grouped_reporting(
        &self,
        sources: &[CombineSource],
        groups: &[u64],
        passwords: &[Option<&str>],
        field_conflicts: pdfcraft_organize::FieldConflicts,
    ) -> Result<Combined, EditError> {
        let (mut keys, mut docs, mut file): (Vec<Option<u64>>, Vec<pdfcraft_cos::Document>, Vec<usize>) = (Vec::new(), Vec::new(), Vec::new());
        for (i, (name, bytes, _)) in sources.iter().enumerate() {
            let key = groups.get(i).copied();
            let known = key.and_then(|k| keys.iter().position(|g| *g == Some(k)));
            let at = match known {
                Some(at) => at,
                None => {
                    docs.push(open_source_with(name, bytes, passwords.get(i).copied().flatten())?);
                    keys.push(key);
                    docs.len() - 1
                }
            };
            file.push(at);
        }
        let mut pages = Vec::with_capacity(sources.len());
        for ((name, _, range), at) in sources.iter().zip(&file) {
            let range = range.as_deref().map(str::trim).filter(|r| !r.is_empty());
            let Some(d) = docs.get(*at) else { return Err(EditError::Write(format!("{name}: not opened"))) };
            pages.push(match range {
                Some(r) => {
                    let n = pdfcraft_organize::page_count(d)?;
                    let p = pdfcraft_print::select_pages(n, Some(r), &[], pdfcraft_print::Subset::All, false)
                        .map_err(|e| EditError::Print(format!("{name}: {e}")))?;
                    Some(p)
                }
                None => None,
            });
        }
        let mut runs: Vec<pdfcraft_organize::Run<'_>> = Vec::with_capacity(sources.len());
        for (((name, _, _), at), p) in sources.iter().zip(&file).zip(&pages) {
            let Some(d) = docs.get(*at) else { return Err(EditError::Write(format!("{name}: not opened"))) };
            runs.push((*at, name.as_str(), d, p.as_deref()));
        }
        let out = pdfcraft_organize::combine_grouped_with(&runs, field_conflicts)?;
        let bytes = self.write_new(&out.doc)?;
        Ok(Combined { bytes, renamed_fields: out.renamed_fields })
    }

    /// New PDF bytes containing copies of `pages` of the document (Extract Pages).
    pub fn extract(&self, id: DocId, pages: &[usize]) -> Result<Arc<Vec<u8>>, EditError> {
        let src = self.cos(id)?;
        if src.permissions().is_some_and(|p| !p.assemble()) {
            return Err(EditError::NotPermitted("extracting pages"));
        }
        let out = pdfcraft_organize::extract_pages(src, pages)?;
        self.write_new(&out)
    }

    /// Split the document into several new PDFs.
    pub fn split(&self, id: DocId, by: &pdfcraft_organize::SplitBy) -> Result<Vec<SplitPart>, EditError> {
        let src = self.cos(id)?;
        if src.permissions().is_some_and(|p| !p.assemble()) {
            return Err(EditError::NotPermitted("splitting the document"));
        }
        let n = pdfcraft_organize::page_count(src)?;
        pdfcraft_organize::split_ranges(n, by)
            .into_iter()
            .map(|r| {
                let doc = pdfcraft_organize::extract_pages(src, &r.clone().collect::<Vec<_>>())?;
                Ok((r.start + 1, r.end, self.write_new(&doc)?))
            })
            .collect()
    }

    /// Split into parts of at most `max_bytes` each (Acrobat's "File size"): pages are taken in
    /// order while their size (as single-page files, an over-estimate because shared fonts and
    /// images count once per part) fits; a page larger than the limit is a part of its own.
    pub fn split_by_size(&self, id: DocId, max_bytes: usize) -> Result<Vec<SplitPart>, EditError> {
        let src = self.cos(id)?;
        if src.permissions().is_some_and(|p| !p.assemble()) {
            return Err(EditError::NotPermitted("splitting the document"));
        }
        let n = pdfcraft_organize::page_count(src)?;
        let mut cuts = Vec::new();
        let mut used = 0usize;
        for p in 0..n {
            let size = self.write_unchecked(&pdfcraft_organize::extract_pages(src, &[p])?)?.len();
            if used > 0 && used + size > max_bytes {
                cuts.push(p);
                used = 0;
            }
            used += size;
        }
        self.split(id, &pdfcraft_organize::SplitBy::Before(cuts))
    }

    /// Web addresses in the text of every page (Create links from URLs): (page, one rectangle
    /// per line in user space, URI). Text that already has a link over it is skipped.
    pub fn find_urls(&self, id: DocId) -> Vec<(usize, Vec<[f64; 4]>, String)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let config = RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut out = Vec::new();
        for page in 0..doc.info.pages.len() {
            let res = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let Some(text) = res.text else { continue };
            let found: std::cell::RefCell<Vec<(std::ops::Range<usize>, String)>> = Default::default();
            let hits = text.find_with(|chars| {
                let urls = pdfcraft_annot::links::find_urls(chars);
                let ranges = urls.iter().map(|u| u.0.clone()).collect();
                *found.borrow_mut() = urls;
                ranges
            });
            for (glyphs, (_, uri)) in hits.into_iter().zip(found.into_inner()) {
                let rects: Vec<[f64; 4]> = text
                    .line_rects(glyphs)
                    .into_iter()
                    .map(|v| {
                        let q = doc.info.pages[page].view_rect_to_quad(v);
                        let xs = [q[0], q[2], q[4], q[6]];
                        let ys = [q[1], q[3], q[5], q[7]];
                        [
                            xs.iter().copied().fold(f64::MAX, f64::min),
                            ys.iter().copied().fold(f64::MAX, f64::min),
                            xs.iter().copied().fold(f64::MIN, f64::max),
                            ys.iter().copied().fold(f64::MIN, f64::max),
                        ]
                    })
                    .filter(|r| {
                        !doc.links().iter().any(|l| l.page == page && l.rect[0] < r[2] && r[0] < l.rect[2] && l.rect[1] < r[3] && r[1] < l.rect[3])
                    })
                    .collect();
                if !rects.is_empty() {
                    out.push((page, rects, uri));
                }
            }
        }
        out
    }

    /// A page of the bookmark list in outline order: up to `limit` bookmarks from position `offset`.
    /// The document's own structure is read for it, so every bookmark can be reached by paging.
    pub fn bookmark_page(&self, id: DocId, offset: usize, limit: usize) -> Option<BookmarkPage> {
        let doc = self.get(id)?;
        if let Some(editor) = doc.editor.as_ref() {
            let page = pdfcraft_organize::bookmark_page(&editor.cos, offset, limit);
            let rows = page
                .bookmarks
                .into_iter()
                .map(|b| {
                    let view = b.view.map_or_else(Default::default, |(kind, numbers)| {
                        let numbers: Vec<Option<f32>> = numbers.iter().map(|n| n.map(|n| n as f32)).collect();
                        pdfcraft_render::DestView::from_operands(Some(&kind), &numbers)
                    });
                    BookmarkRow { path: b.path, title: b.title, open: b.open, page: b.page, view, color: b.color, italic: b.italic, bold: b.bold }
                })
                .collect();
            return Some(BookmarkPage { rows, next: page.next, truncated: false });
        }
        let all = outline_rows(&doc.info.outline);
        let end = offset.saturating_add(limit);
        let rows = all.iter().skip(offset).take(limit).cloned().collect();
        Some(BookmarkPage { rows, next: (end < all.len()).then_some(end), truncated: doc.info.outline_more })
    }

    /// Top-level bookmarks as split points: (first page of each part, its bookmark's title).
    pub fn bookmark_splits(&self, id: DocId) -> Vec<(usize, String)> {
        let Some(doc) = self.get(id) else { return Vec::new() };
        let top: Vec<(Option<usize>, String)> = match doc.editor.as_ref() {
            Some(editor) => pdfcraft_organize::top_level_bookmarks(&editor.cos).into_iter().map(|b| (b.page, b.title)).collect(),
            None => doc.info.outline.iter().map(|o| (o.page, o.title.clone())).collect(),
        };
        let mut out: Vec<(usize, String)> = top.into_iter().filter_map(|(page, title)| Some((page?, title))).collect();
        out.sort_by_key(|x| x.0);
        out.dedup_by_key(|x| x.0);
        out
    }

    /// Open freshly created bytes (combine / extract) as a new, unsaved document.
    pub fn open_new(&mut self, name: impl Into<String>, bytes: Arc<Vec<u8>>) -> Result<DocId, OpenError> {
        let id = self.open(name, None, bytes, None)?;
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.dirty = true;
            if let Some(e) = d.editor.as_mut() {
                e.unsaved_file = true;
            }
            // Unsaved work with no edit yet still needs its first recovery snapshot.
            d.generation += 1;
        }
        Ok(id)
    }

    fn cos(&self, id: DocId) -> Result<&pdfcraft_cos::Document, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        doc.editor.as_ref().map(|e| &e.cos).ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))
    }

    /// A new file (Combine, Extract, Split, Create) as bytes, checked like a save before anyone
    /// writes it (#1698, #2166): it reopens, and shows the pages it was built from.
    fn write_new(&self, doc: &pdfcraft_cos::Document) -> Result<Arc<Vec<u8>>, EditError> {
        let bytes = self.write_unchecked(doc)?;
        let expected = pdfcraft_organize::page_count(doc)?;
        pdfcraft_cos::Document::open(bytes.clone()).map_err(|e| EditError::Reopen(e.to_string()))?;
        pdfcraft_render::check_pages(bytes.clone(), None, expected).map_err(|e| EditError::Reopen(e.to_string()))?;
        Ok(bytes)
    }

    /// [`Self::write_new`] without the check, for bytes nobody writes: measuring a part's size.
    /// Like a save, a new file never carries a typed Password-field value (#3926): Extract and
    /// Split copy the fields of the pages they take.
    fn write_unchecked(&self, doc: &pdfcraft_cos::Document) -> Result<Arc<Vec<u8>>, EditError> {
        let opts = SaveOptions { mod_date: self.now().map(pdfcraft_cos::pdf_date), ..SaveOptions::default() };
        let doc = writable(doc)?;
        write_full(&doc, &opts).map(Arc::new).map_err(|e| EditError::Write(e.to_string()))
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

    /// Show or hide a layer (optional content group) for viewing. Returns `true` if it changed;
    /// callers must drop cached rasters and text for the document.
    pub fn set_layer_visible(&mut self, id: DocId, layer: usize, visible: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        let Some(l) = doc.info.layers.get_mut(layer) else { return false };
        if l.visible == visible {
            return false;
        }
        l.visible = visible;
        use_layer_choices(doc);
        true
    }

    /// Run a set-layer-visibility action (`SetOCGState`): apply `changes` in order, naming each
    /// layer by its optional content group; groups that aren't layers are skipped. With
    /// `preserve_rb`, a layer turned on turns off the other layers of its radio-button groups.
    /// Returns `true` if any layer changed; callers must drop cached rasters and text for the
    /// document, as for [`Self::set_layer_visible`].
    pub fn set_layer_state(&mut self, id: DocId, changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if !apply_layer_state(&mut doc.info.layers, &doc.info.layer_groups, changes, preserve_rb) {
            return false;
        }
        use_layer_choices(doc);
        true
    }

    /// Show each layer as the file opens (Acrobat's Layers ▸ Reset to Initial Visibility). Returns
    /// `true` if any layer changed; callers must drop cached rasters and text for the document,
    /// as for [`Self::set_layer_visible`].
    pub fn reset_layers(&mut self, id: DocId) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        let mut changed = false;
        for l in &mut doc.info.layers {
            changed |= l.visible != l.initial;
            l.visible = l.initial;
        }
        if changed {
            use_layer_choices(doc);
        }
        changed
    }

    /// Hide or show every comment on the page (Acrobat's Comments ▸ Hide all comments). Form
    /// fields and links still draw. Returns whether anything changed.
    pub fn set_hide_comments(&mut self, id: DocId, hide: bool) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if doc.config.hide_comments == hide {
            return false;
        }
        doc.config.hide_comments = hide;
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.generation += 1;
        true
    }

    /// Set the document's reading colours (Acrobat's Replace Document Colors). Returns whether
    /// anything changed; callers must drop cached rasters and text for the document.
    pub fn set_reading_colors(&mut self, id: DocId, colors: ReadingColors) -> bool {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return false };
        if doc.config.reading_colors == colors {
            return false;
        }
        doc.config.reading_colors = colors;
        doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        doc.generation += 1;
        true
    }

    /// The document's reading-colours setting; `None` when no such document is open.
    pub fn reading_colors(&self, id: DocId) -> Option<ReadingColors> {
        Some(self.get(id)?.config.reading_colors)
    }

    /// How the document's pages are drawn for viewing: its password and view settings (layer
    /// visibility, hidden comments, reading colours). `None` when no such document is open.
    pub fn render_config(&self, id: DocId) -> Option<RenderConfig> {
        Some(self.get(id)?.config.clone())
    }

    /// Acrobat's Summarize Comments ("Comments only" layout): a new PDF listing every comment
    /// with its number, author, type, date, text and replies, grouped by page.
    pub fn summarize_comments(&self, id: DocId, sort: SummarySort) -> Result<Arc<Vec<u8>>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let text = comment_summary(&doc.name, &doc.info.annotations, sort);
        self.create_report(&format!("Summary of Comments on {}", doc.name), &text)
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
            doc.trust = self.trust.clone();
            let Some(editor) = doc.editor.as_mut() else { continue };
            // Keep verdict refresh and renderer refresh together. Viewer caches are
            // invalidated by the UI trust path; trust alone changes no document artwork.
            let signatures = if doc.signatures.is_empty() { Vec::new() } else { signatures_of(&editor.cos, &doc.bytes, &doc.trust, &doc.sig_cache) };
            doc.display = display_bytes(Some(editor), &doc.bytes);
            doc.signatures = Arc::new(signatures);
            doc.renderer = RenderPool::new(doc.display.clone(), render_threads(), doc.config.clone());
        }
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
        let cos = writable(&editor.cos)?;
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
        let cos = writable(&editor.cos)?;
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

    pub fn close(&mut self, id: DocId) {
        self.docs.retain(|d| d.id != id);
    }

    pub fn get(&self, id: DocId) -> Option<&Document> {
        self.docs.iter().find(|d| d.id == id)
    }

    /// Read document `id`'s font list again if content edits since the last full inspection may
    /// have changed it (Document Properties ▸ Fonts, `doc_info`). A no-op otherwise.
    pub fn refresh_fonts(&mut self, id: DocId) {
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id && d.fonts_stale) else { return };
        if let Ok(info) = pdfcraft_render::inspect(doc.bytes.clone(), doc.password.as_deref()) {
            doc.info.fonts = info.fonts;
        }
        doc.fonts_stale = false;
    }

    pub fn docs(&self) -> &[Document] {
        &self.docs
    }
}
