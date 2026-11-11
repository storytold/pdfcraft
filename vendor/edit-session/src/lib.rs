//! A local draft of a value, independent of rendering and the application's undo engine.
//!
//! The host validates proposals and owns all document mutations. Dropping a session cancels
//! only its local draft. Store small parameters here, not a full document per pointer event.
//! A context must include stable document/target identity and a revision that changes on
//! every relevant edit (including undo/redo). Comparing values alone cannot detect ABA edits.
#![no_std]
#![forbid(unsafe_code)]

/// Why a draft cannot be committed to the current host state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishError {
    /// The most recent proposal was rejected. Correct it before finishing.
    InvalidProposal,
    /// Document, target, selection or relevant revision has changed.
    ContextChanged,
    /// The live value changed without the context being updated.
    ValueChanged,
}

impl core::fmt::Display for FinishError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidProposal => "the latest proposal is invalid",
            Self::ContextChanged => "the edit context has changed",
            Self::ValueChanged => "the live value has changed",
        })
    }
}

impl core::error::Error for FinishError {}

/// A proposed change. The caller must apply it through its existing command engine.
#[derive(Debug, PartialEq, Eq)]
pub struct Change<T> {
    pub before: T,
    pub after: T,
}

/// A single-use edit. It never writes to a document or records history itself.
///
/// This type intentionally does not implement `Clone`. `finish` consumes it, so the same
/// session cannot accidentally emit two commits. A failed finish also consumes the draft;
/// call `check` first if invalid input should remain editable in the UI.
#[must_use = "a draft must be retained, finished, or deliberately cancelled"]
#[derive(Debug)]
pub struct EditSession<T, C> {
    original: T,
    preview: T,
    context: C,
    valid: bool,
}

impl<T: Clone, C> EditSession<T, C> {
    /// Begin from a value already validated by the application.
    pub fn new(original: T, context: C) -> Self {
        Self {
            preview: original.clone(),
            original,
            context,
            valid: true,
        }
    }

    /// Restore the original preview and clear invalid input, keeping the draft open.
    ///
    /// The original value and context are retained: this does not rebase a stale draft
    /// or change the live document. The host must refresh its displayed preview itself.
    pub fn reset(&mut self) {
        self.preview = self.original.clone();
        self.valid = true;
    }
}

impl<T, C> EditSession<T, C> {
    /// The validated value captured when editing began.
    pub fn original(&self) -> &T {
        &self.original
    }

    /// Last valid preview, even when a later incomplete/invalid input is present.
    pub fn preview(&self) -> &T {
        &self.preview
    }

    /// The host context captured when editing began; never changed by this session.
    pub fn context(&self) -> &C {
        &self.context
    }

    /// Whether the latest proposal was accepted. This does not check live host state.
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// Accept a host-validated proposal. Errors invalidate commit without replacing the
    /// last valid preview. Units, mixed values, finite checks and bounds belong to the host.
    pub fn propose<E>(&mut self, proposal: Result<T, E>) -> Result<(), E> {
        match proposal {
            Ok(value) => {
                self.preview = value;
                self.valid = true;
                Ok(())
            }
            Err(error) => {
                self.valid = false;
                Err(error)
            }
        }
    }

    /// Discard the local draft. No rollback commands or history entries are generated.
    pub fn cancel(self) {}
}

impl<T: PartialEq, C: PartialEq> EditSession<T, C> {
    /// Check whether the draft still belongs to the current host state, independently
    /// of invalid input. Useful when deciding whether an open editor must be dismissed.
    ///
    /// This never returns `InvalidProposal` and does not authorize a commit. Use `check`
    /// or `finish` immediately before applying on the host's serialized command path.
    /// Context conflicts take precedence over value conflicts. No lock is acquired.
    pub fn check_live(&self, live: &T, context: &C) -> Result<(), FinishError> {
        if *context != self.context {
            Err(FinishError::ContextChanged)
        } else if *live != self.original {
            Err(FinishError::ValueChanged)
        } else {
            Ok(())
        }
    }

    /// Check live host state and proposal validity immediately before applying on the
    /// host's serialized command path. Live conflicts take precedence over invalid input.
    /// This comparison is not a lock and is not a distributed transaction.
    pub fn check(&self, live: &T, context: &C) -> Result<(), FinishError> {
        self.check_live(live, context)?;
        if !self.valid {
            Err(FinishError::InvalidProposal)
        } else {
            Ok(())
        }
    }

    /// Emit at most one change; a no-op emits none, preserving the host's redo stack.
    /// The host still validates and applies the command and handles execution failure.
    pub fn finish(self, live: &T, context: &C) -> Result<Option<Change<T>>, FinishError> {
        self.check(live, context)?;
        if self.original == self.preview {
            Ok(None)
        } else {
            Ok(Some(Change {
                before: self.original,
                after: self.preview,
            }))
        }
    }
}

/// A session cannot be finished twice.
///
/// ```compile_fail
/// use edit_session::EditSession;
/// let draft = EditSession::new(100, ("document", 7));
/// let _ = draft.finish(&100, &("document", 7));
/// let _ = draft.finish(&100, &("document", 7));
/// ```
const _: () = ();
