use super::*;
use pdfcraft_form_detect::Proposal;

fn setup() -> (Session, DocId) {
    let mut session = Session::new();
    let bytes = session.create_from_text("form", "Name: ______________________\n\nEmail: ______________________").unwrap();
    let id = session.open("form.pdf", None, bytes, None).unwrap();
    (session, id)
}

fn proposals() -> Vec<Proposal> {
    vec![
        Proposal { kind: Kind::Text, rect: [0.3, 0.2, 0.8, 0.24], confidence: 0.9 },
        Proposal { kind: Kind::CheckBox, rect: [0.2, 0.4, 0.22, 0.42], confidence: 0.8 },
        Proposal { kind: Kind::Signature, rect: [0.3, 0.7, 0.8, 0.75], confidence: 0.85 },
    ]
}

#[test]
fn proposals_are_added_once_transparent_and_undoable_and_survive_reopen() {
    let (mut session, id) = setup();
    let job = session.form_detection_job(id, &[]).unwrap();
    let found = job.run_inner(0.3, &mut |_, _| true, |_, _, _| Ok(proposals())).unwrap();
    assert_eq!(found.fields().len(), 3);
    assert!(session.get(id).unwrap().form.is_empty(), "preview is read only");
    let names = session.apply_detected_fields(found).unwrap();
    let doc = session.get(id).unwrap();
    assert_eq!(doc.form.len(), 3);
    assert_eq!(doc.can_undo(), Some("Detect form fields"));
    for name in &names {
        let look = doc.field_look(name).unwrap();
        assert_eq!(look.border, None);
        assert_eq!(look.fill, None);
    }
    let bytes = session.save_bytes(id).unwrap();
    let reopened = session.open("saved.pdf", None, bytes, None).unwrap();
    assert_eq!(session.get(reopened).unwrap().form.len(), 3);
    session.undo(id).unwrap();
    assert!(session.get(id).unwrap().form.is_empty());
    session.redo(id).unwrap();
    let again = session.form_detection_job(id, &[]).unwrap().run_inner(0.3, &mut |_, _| true, |_, _, _| Ok(proposals())).unwrap();
    assert!(again.fields().is_empty(), "existing fields are not detected twice");
}

#[test]
fn cancellation_errors_and_stale_results_do_not_modify_the_document() {
    let (mut session, id) = setup();
    let job = session.form_detection_job(id, &[]).unwrap();
    let result = job.run_inner(0.3, &mut |_, _| false, |_, _, _| panic!("cancelled before inference"));
    assert!(matches!(result, Err(EditError::Cancelled)));
    let mut inferred = false;
    let result = job.run_inner(0.3, &mut |done, total| done < total, |_, _, _| {
        inferred = true;
        Ok(proposals())
    });
    assert!(inferred, "the final cancellation check runs after inference");
    assert!(matches!(result, Err(EditError::Cancelled)));
    let result = job.run_inner(0.3, &mut |_, _| true, |_, _, _| Err(DetectionError::Inference("test error".into())));
    assert!(result.is_err());
    assert!(session.get(id).unwrap().form.is_empty());
    let found = job.run_inner(0.3, &mut |_, _| true, |_, _, _| Ok(proposals())).unwrap();
    session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert!(session.apply_detected_fields(found).unwrap_err().to_string().contains("changed"));
    assert!(session.get(id).unwrap().form.is_empty());
    assert_eq!(session.get(id).unwrap().can_undo(), Some("Rotate page"));
}

#[test]
fn native_proposals_share_the_existing_text_layer_naming_rules() {
    let (session, id) = setup();
    let legacy = session.detect_fields(id, &[]);
    assert_eq!(legacy.len(), 2);
    let job = session.form_detection_job(id, &[]).unwrap();
    let info = &job.pages[0].info;
    let proposals: Vec<Proposal> = legacy
        .iter()
        .map(|(_, field)| {
            let a = info.user_to_view(field.rect[0] as f32, field.rect[1] as f32);
            let b = info.user_to_view(field.rect[2] as f32, field.rect[3] as f32);
            Proposal {
                kind: Kind::Text,
                confidence: 0.9,
                rect: [a[0].min(b[0]) / info.width, a[1].min(b[1]) / info.height, a[0].max(b[0]) / info.width, a[1].max(b[1]) / info.height],
            }
        })
        .collect();
    let result = job.run_inner(0.3, &mut |_, _| true, |_, _, _| Ok(proposals.clone())).unwrap();
    assert_eq!(result.fields().iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Name", "Email"]);
}

#[test]
fn normalized_boxes_follow_page_rotation_and_keep_their_displayed_size() {
    let (mut session, id) = setup();
    for angle in [0, 90, 180, 270] {
        let job = session.form_detection_job(id, &[0, 0]).unwrap();
        assert_eq!(job.page_count(), 1, "duplicate page indices do not duplicate fields");
        let info = &job.pages[0].info;
        let normalized = [0.2, 0.3, 0.7, 0.4];
        let rect = user_rect(info, normalized).unwrap();
        let [a, b] = [info.user_to_view(rect[0] as f32, rect[1] as f32), info.user_to_view(rect[2] as f32, rect[3] as f32)];
        let shown = [a[0].min(b[0]) / info.width, a[1].min(b[1]) / info.height, a[0].max(b[0]) / info.width, a[1].max(b[1]) / info.height];
        assert!(shown.iter().zip(normalized).all(|(a, b)| (a - b).abs() < 1e-6), "{angle}: {shown:?}");
        let found = job.run_inner(0.3, &mut |_, _| true, |_, _, _| Ok(proposals())).unwrap();
        assert!(found.fields().iter().all(|f| f.rotation == angle));
        session.apply_detected_fields(found).unwrap();
        let doc = session.get(id).unwrap();
        for field in doc.form.iter() {
            assert_eq!(field.widgets[0].rotation, angle);
        }
        session.undo(id).unwrap();
        session.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    }
    assert!(session.form_detection_job(id, &[usize::MAX]).is_err());
}
