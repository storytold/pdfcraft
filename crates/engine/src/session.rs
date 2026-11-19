//! The session: the open documents and the operations on them.

use std::sync::{Arc, OnceLock};

use pdfcraft_cos::{SaveOptions, write_full, write_incremental};
use pdfcraft_render::{LayerOp, OpenError, ReadingColors, RenderConfig, RenderPool, inspect};

#[cfg(test)]
use crate::check_refresh_panic;
use crate::edit::{EditCtx, check_permission, check_purview_permission, run_edit};
use crate::history::{Keys, MAX_UNDO, Scope, keys_after, scope_of, uses_scripts};
use crate::{
    BookmarkPage, BookmarkRow, CombineSource, Combined, DataFormat, DocId, Document, Edit, EditError, Hairlines, HairlinesFixed, Hidden,
    ImageResolution, LOW_RESOLUTION_PRINT_DPI, Label, ObjectKind, OptimizeReport, RecoverySnapshot, SignOptions, SourceKind, SplitPart,
    StatusCaptions, SummarySort, TrustStore, XFA_REMOVED_PREFIX, XFA_SCRIPTS_OFF, apply_layer_state, carry_verdict_marks, comment_list,
    comment_summary, decode_text, display_bytes, display_config, guard, info_fields, js, local_utc_offset, note_warnings, office_to_pdf, open_source,
    open_source_with, optimize, outline_rows, parse_ymd, print, print_layer_states, print_raster, render_threads, scan_colours,
    set_default_layer_state, shown_under_redactions, signatures_of, source_kind, sync_xfa_state, undrawn_notices, use_layer_choices, writable, xfa,
    xfa_sync_datasets,
};

mod editing;
mod files;
mod save;
mod view;

impl Session {
    pub fn set_purview_provider(&mut self, provider: Arc<dyn crate::PurviewProvider>) {
        self.purview = Some(provider);
    }

    pub fn clear_purview_provider(&mut self) {
        self.purview = None;
    }

    pub fn purview_status(&self) -> crate::PurviewStatus {
        self.purview.as_ref().map(|provider| provider.status()).unwrap_or(crate::PurviewStatus::SdkUnavailable)
    }

    pub fn purview_sign_in(&self, username_hint: Option<&str>) -> Result<(), crate::PurviewError> {
        self.purview.as_ref().ok_or(crate::PurviewError::SdkUnavailable)?.sign_in(username_hint)
    }

    pub fn purview_sign_out(&mut self) -> Result<(), crate::PurviewError> {
        self.purview.as_ref().ok_or(crate::PurviewError::SdkUnavailable)?.sign_out()
    }

    pub fn purview_labels(&self) -> Result<Vec<crate::PurviewLabel>, crate::PurviewError> {
        self.purview.as_ref().ok_or(crate::PurviewError::SdkUnavailable)?.labels()
    }

    pub fn purview_apply_label(&mut self, id: DocId, label_id: &str, justification: Option<&str>) -> Result<(), EditError> {
        let document = self.get(id).ok_or(EditError::NoDocument)?;
        if document.editor.as_ref().is_some_and(|editor| editor.cos.output_handler().is_some()) {
            return Err(EditError::Protection(
                "ordinary PDF password protection and Microsoft Purview labels cannot be composed in the same save".into(),
            ));
        }
        let provider = self.purview.clone().ok_or(EditError::Protection("Microsoft Purview support is not configured".into()))?;
        let state = document.purview.clone().unwrap_or(crate::PurviewDocumentState {
            handle: 0,
            original: document.bytes.clone(),
            document: crate::PurviewDocument::default(),
            provider: provider.clone(),
        });
        let working = document.bytes.clone();
        let opened = state
            .provider
            .apply_label(state.handle, &state.original, &working, label_id, justification)
            .map_err(|error| EditError::Protection(error.to_string()))?;
        let cos = pdfcraft_cos::Document::open_with_password(opened.bytes.clone(), None).map_err(|error| EditError::Reopen(error.to_string()))?;
        let document = self.doc_mut(id)?;
        let provider = state.provider.clone();
        if let Some(editor) = document.editor.as_mut() {
            editor.cos = cos;
        } else {
            return Err(EditError::ReadOnly("this Microsoft Purview document could not be edited".into()));
        }
        document.bytes = opened.bytes;
        document.dirty = true;
        document.purview =
            Some(crate::PurviewDocumentState { handle: opened.handle, original: opened.original, document: opened.document, provider });
        document.generation = document.generation.saturating_add(1);
        Self::refresh(document).inspect_err(|_| Self::show_released(document))
    }

    pub fn purview_remove_label(&mut self, id: DocId, justification: Option<&str>) -> Result<(), EditError> {
        let document = self.get(id).ok_or(EditError::NoDocument)?;
        if document.editor.as_ref().is_some_and(|editor| editor.cos.output_handler().is_some()) {
            return Err(EditError::Protection(
                "ordinary PDF password protection and Microsoft Purview labels cannot be composed in the same save".into(),
            ));
        }
        let state =
            document.purview.clone().ok_or_else(|| EditError::Invalid("This document is not Microsoft Purview-labeled or protected".into()))?;
        let working = document.bytes.clone();
        let opened = state
            .provider
            .remove_label(state.handle, &state.original, &working, justification)
            .map_err(|error| EditError::Protection(error.to_string()))?;
        let cos = pdfcraft_cos::Document::open_with_password(opened.bytes.clone(), None).map_err(|error| EditError::Reopen(error.to_string()))?;
        let document = self.doc_mut(id)?;
        let provider = state.provider.clone();
        if let Some(editor) = document.editor.as_mut() {
            editor.cos = cos;
        } else {
            return Err(EditError::ReadOnly("this Microsoft Purview document could not be edited".into()));
        }
        document.bytes = opened.bytes;
        document.dirty = true;
        document.purview =
            Some(crate::PurviewDocumentState { handle: opened.handle, original: opened.original, document: opened.document, provider });
        document.generation = document.generation.saturating_add(1);
        Self::refresh(document).inspect_err(|_| Self::show_released(document))
    }

    pub fn purview_remove_protection(&mut self, id: DocId) -> Result<(), EditError> {
        let document = self.get(id).ok_or(EditError::NoDocument)?;
        if document.editor.as_ref().is_some_and(|editor| editor.cos.output_handler().is_some()) {
            return Err(EditError::Protection(
                "ordinary PDF password protection and Microsoft Purview protection cannot be composed in the same save".into(),
            ));
        }
        let state =
            document.purview.clone().ok_or_else(|| EditError::Invalid("This document is not Microsoft Purview-labeled or protected".into()))?;
        let working = document.bytes.clone();
        let opened =
            state.provider.remove_protection(state.handle, &state.original, &working).map_err(|error| EditError::Protection(error.to_string()))?;
        let cos = pdfcraft_cos::Document::open_with_password(opened.bytes.clone(), None).map_err(|error| EditError::Reopen(error.to_string()))?;
        let document = self.doc_mut(id)?;
        let provider = state.provider.clone();
        if let Some(editor) = document.editor.as_mut() {
            editor.cos = cos;
        } else {
            return Err(EditError::ReadOnly("this Microsoft Purview document could not be edited".into()));
        }
        document.bytes = opened.bytes;
        document.dirty = true;
        document.purview =
            Some(crate::PurviewDocumentState { handle: opened.handle, original: opened.original, document: opened.document, provider });
        document.generation = document.generation.saturating_add(1);
        Self::refresh(document).inspect_err(|_| Self::show_released(document))
    }
}

#[derive(Default)]
pub struct Session {
    pub(crate) docs: Vec<Document>,
    pub(crate) next_id: u64,
    /// Native or test Microsoft Purview provider. None when PdfCraft was built
    /// without MIP SDK support or the app has not configured one.
    pub(crate) purview: Option<Arc<dyn crate::PurviewProvider>>,
    /// Seconds since the Unix epoch, injected so saves are deterministic in tests.
    clock: Option<fn() -> i64>,
    /// Seconds to add to UTC for local time, read from whoever knows the zone (a browser, say).
    /// Separate from `clock` on purpose: `clock` alone is the test shortcut that takes the
    /// injected time AS local time and zeroes the zone, so injecting only it leaves either the
    /// local dates or the UTC ones wrong by the zone's offset.
    utc_offset: Option<fn() -> i64>,
    /// Certificates trusted for signing (Acrobat: Trusted Certificates).
    pub(crate) trust: Arc<TrustStore>,
    /// The words signature status layers are drawn with, in the interface language.
    pub(crate) captions: Arc<StatusCaptions>,
    /// Preferences ▸ JavaScript ▸ Enable Acrobat JavaScript, inverted (on by default).
    pub(crate) js_off: bool,
    /// Preferences ▸ Date format, when not the default (see [`dates`]).
    pub(crate) date_format: Option<String>,
    /// Preferences ▸ Date format ▸ Language, when not following the interface language.
    pub(crate) date_language: Option<String>,
    /// Highlight fields, as the viewer shows it: what forms' scripts read as
    /// `app.runtimeHighlight` (off by default, as in a session without a viewer).
    pub(crate) field_highlight: bool,
    /// What the XFA scripts' time limits are multiplied by (`None`: 1); see
    /// [`Session::with_xfa_time_scale`].
    pub(crate) xfa_time_scale: Option<u32>,
}
