use pdfcraft_ffi::{Engine, FfiPoint, FfiReviewState};

fn synthetic_blank_pdf() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let objs: Vec<String> = vec![
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_string(),
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>\nendobj\n".to_string(),
    ];

    for obj in &objs {
        offsets.push(out.len());
        out.extend_from_slice(obj.as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn test_comments_list_reply_and_review_state() {
    let engine = Engine::new();
    let pdf = synthetic_blank_pdf();
    let doc_id = engine.open_bytes("comments_test.pdf".into(), pdf, None).expect("open pdf");

    // Initially 0 comments
    let initial_comments = engine.all_comments(doc_id).expect("all comments");
    assert!(initial_comments.is_empty());

    // 1. Add sticky note comment
    engine
        .add_note_annotation(
            doc_id,
            0,
            FfiPoint { x: 100.0, y: 200.0 },
            "Initial review note".into(),
            Some("ReviewerAlice".into()),
            Some("Comment".into()),
            Some(0xFFD700),
        )
        .expect("add note");

    let page_comments = engine.page_comments(doc_id, 0).expect("page comments");
    assert_eq!(page_comments.len(), 1);
    let root = &page_comments[0];
    assert_eq!(root.author.as_deref(), Some("ReviewerAlice"));
    assert_eq!(root.contents.as_deref(), Some("Initial review note"));
    assert!(!root.is_marked);
    assert_eq!(root.review_state, None);

    // 2. Add reply to comment
    engine.reply_to_comment(doc_id, 0, root.index, "I have addressed this issue".into(), "AuthorBob".into()).expect("reply to comment");

    let updated_comments = engine.page_comments(doc_id, 0).expect("page comments after reply");
    assert_eq!(updated_comments.len(), 2);
    let reply = updated_comments.iter().find(|c| c.in_reply_to.is_some()).expect("found reply");
    assert_eq!(reply.author.as_deref(), Some("AuthorBob"));
    assert_eq!(reply.contents.as_deref(), Some("I have addressed this issue"));
    assert_eq!(reply.in_reply_to, root.name);

    // 3. Set review state on root comment thread
    engine.set_comment_review_state(doc_id, 0, root.index, FfiReviewState::Accepted, "ReviewerAlice".into()).expect("set review state");

    let after_status = engine.page_comments(doc_id, 0).expect("page comments after status");
    // Acrobat adds a status reply annotation
    let status_reply = after_status.iter().find(|c| c.review_state.is_some()).expect("found status item");
    assert_eq!(status_reply.review_state, Some(FfiReviewState::Accepted));
    assert_eq!(status_reply.author.as_deref(), Some("ReviewerAlice"));

    // 4. Mark comment with checkmark
    engine.set_comment_marked(doc_id, 0, root.index, true, "ReviewerAlice".into()).expect("mark checkmark");

    let after_marked = engine.page_comments(doc_id, 0).expect("page comments after mark");
    let marked_item = after_marked.iter().find(|c| c.is_marked).expect("found marked item");
    assert!(marked_item.is_marked);

    // 5. Update contents of comment
    engine.set_comment_contents(doc_id, 0, root.index, "Updated review note contents".into()).expect("set contents");

    let after_content = engine.page_comments(doc_id, 0).expect("page comments after contents edit");
    let updated_root = &after_content[0];
    assert_eq!(updated_root.contents.as_deref(), Some("Updated review note contents"));
}
