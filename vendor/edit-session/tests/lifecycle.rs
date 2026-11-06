use edit_session::{Change, EditSession, FinishError};

#[test]
fn many_previews_produce_one_final_change() {
    let mut draft = EditSession::new(100, ("photo", "layer-1", 9));
    for value in 0..1000 {
        assert_eq!(draft.propose::<()>(Ok(value)), Ok(()));
        assert_eq!(*draft.original(), 100);
    }
    assert_eq!(
        draft.finish(&100, &("photo", "layer-1", 9)),
        Ok(Some(Change {
            before: 100,
            after: 999
        }))
    );
}

#[test]
fn invalid_input_retains_preview_but_blocks_commit_until_corrected() {
    let mut draft = EditSession::new(10, 0);
    assert_eq!(draft.propose::<&str>(Ok(12)), Ok(()));
    assert_eq!(
        draft.propose(Err("incomplete number")),
        Err("incomplete number")
    );
    assert_eq!(*draft.preview(), 12);
    assert!(!draft.is_valid());
    assert_eq!(draft.check(&10, &0), Err(FinishError::InvalidProposal));
    assert_eq!(draft.propose::<&str>(Ok(14)), Ok(()));
    assert_eq!(
        draft.finish(&10, &0),
        Ok(Some(Change {
            before: 10,
            after: 14
        }))
    );
}

#[test]
fn unchanged_and_return_to_original_have_no_commit() {
    assert_eq!(EditSession::new(20, 0).finish(&20, &0), Ok(None));
    let mut draft = EditSession::new(20, 0);
    assert_eq!(draft.propose::<()>(Ok(50)), Ok(()));
    assert_eq!(draft.propose::<()>(Ok(20)), Ok(()));
    assert_eq!(draft.finish(&20, &0), Ok(None));
}

#[test]
fn stale_document_target_or_revision_cannot_commit() {
    for context in [(2, 5, 8), (1, 6, 8), (1, 5, 9)] {
        let draft = EditSession::new(100, (1, 5, 8));
        assert_eq!(
            draft.finish(&100, &context),
            Err(FinishError::ContextChanged)
        );
    }
}

#[test]
fn unversioned_value_change_is_also_rejected() {
    assert_eq!(
        EditSession::new(100, 1).finish(&50, &1),
        Err(FinishError::ValueChanged)
    );
}

#[test]
fn undo_back_to_equal_value_is_stale_when_revision_has_advanced() {
    assert_eq!(
        EditSession::new(100, 1).finish(&100, &3),
        Err(FinishError::ContextChanged)
    );
}

#[test]
fn invalid_final_input_does_not_silently_commit_last_valid_value() {
    let mut draft = EditSession::new(10, 0);
    assert_eq!(draft.propose::<&str>(Ok(12)), Ok(()));
    assert!(draft.propose(Err("not finite")).is_err());
    assert_eq!(draft.finish(&10, &0), Err(FinishError::InvalidProposal));
}

#[test]
fn optional_values_preserve_inheritance_instead_of_flattening_it() {
    let mut draft = EditSession::new(None::<u16>, 0);
    assert_eq!(draft.propose::<()>(Ok(Some(20))), Ok(()));
    assert_eq!(draft.propose::<()>(Ok(None)), Ok(()));
    assert_eq!(draft.finish(&None, &0), Ok(None));
}

#[test]
fn cancel_does_not_touch_host_value() {
    let live = String::from("original");
    let mut draft = EditSession::new(live.clone(), 1);
    assert_eq!(draft.propose::<()>(Ok("preview".into())), Ok(()));
    draft.cancel();
    assert_eq!(live, "original");
}

#[test]
fn invalid_input_can_remain_editable_while_live_context_is_current() {
    let mut draft = EditSession::new(10, 7);
    assert!(draft.propose(Err("incomplete number")).is_err());
    assert_eq!(draft.check_live(&10, &7), Ok(()));
    assert_eq!(draft.check(&10, &7), Err(FinishError::InvalidProposal));
    for (live, context, error) in [
        (10, 8, FinishError::ContextChanged),
        (11, 8, FinishError::ContextChanged),
        (11, 7, FinishError::ValueChanged),
    ] {
        assert_eq!(draft.check_live(&live, &context), Err(error));
        assert_eq!(draft.check(&live, &context), Err(error));
    }
    assert!(!draft.is_valid());
}

#[test]
fn reset_clears_invalid_input_and_confirming_it_is_a_noop() {
    // Reset only needs the value to be Clone, not the host's context.
    #[derive(Debug, PartialEq)]
    struct Context(u64);
    let mut draft = EditSession::new(String::from("original"), Context(7));
    assert_eq!(draft.propose::<()>(Ok(String::from("preview"))), Ok(()));
    assert!(draft.propose(Err("invalid")).is_err());
    draft.reset();
    assert!(draft.is_valid());
    assert_eq!(draft.preview(), "original");
    assert_eq!(draft.original(), "original");
    assert_eq!(draft.context(), &Context(7));
    assert_eq!(
        draft.finish(&String::from("original"), &Context(7)),
        Ok(None)
    );
}

#[test]
fn reset_allows_further_editing_without_changing_the_original() {
    let mut draft = EditSession::new(10, 7);
    assert_eq!(draft.propose::<()>(Ok(12)), Ok(()));
    draft.reset();
    draft.reset();
    assert_eq!(draft.propose::<()>(Ok(15)), Ok(()));
    assert_eq!(
        draft.finish(&10, &7),
        Ok(Some(Change {
            before: 10,
            after: 15
        }))
    );
}

#[test]
fn reset_does_not_rebase_a_stale_draft_or_hide_a_live_value_conflict() {
    for (live, context, error) in [
        (10, 9, FinishError::ContextChanged), // Edit then Undo back to the original.
        (12, 7, FinishError::ValueChanged),
    ] {
        let mut draft = EditSession::new(10, 7);
        assert_eq!(draft.propose::<()>(Ok(12)), Ok(()));
        draft.reset();
        assert_eq!(draft.finish(&live, &context), Err(error));
    }
}

#[test]
fn finish_errors_support_standard_error_reporting() {
    for (error, message) in [
        (
            FinishError::InvalidProposal,
            "the latest proposal is invalid",
        ),
        (FinishError::ContextChanged, "the edit context has changed"),
        (FinishError::ValueChanged, "the live value has changed"),
    ] {
        let reported: &dyn core::error::Error = &error;
        assert_eq!(reported.to_string(), message);
        assert!(reported.source().is_none());
    }
}
