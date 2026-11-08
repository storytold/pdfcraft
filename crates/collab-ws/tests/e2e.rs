//! End-to-end: three clients over a real socket, one authoritative PDF.
//!
//! The clients do not share memory with the server: each receives `applied`,
//! projects the accepted operation with `pdfcraft-collab-client-pdf` and edits
//! its own document. The test checks that all three documents end up equal, that
//! a viewer cannot write, that an old epoch is refused, and that a late joiner
//! catches up from a snapshot tail.

use pdfcraft_collab_client_pdf::{
    add_annotation_payload, add_note_payload, apply, merge_op, reply_payload, rotate_payload, set_contents_payload, structural_op,
};
use pdfcraft_collab_hub::Hub;
use pdfcraft_collab_pdf::PdfAdapter;
use pdfcraft_collab_proto::{ActorId, ClientMessage, DocKey, ErrorCode, Op, OpId, PageRef, Presence, Role, ServerMessage, Target, Viewport};
use pdfcraft_collab_server::{Adapter, AdapterError, ApplyOutcome, CommitOutcome, Publication, PublishOutcome, RoomConfig};
use pdfcraft_collab_ws::{Claims, Config, ServerHandle, TokenError, Verifier, WsServer};
use pdfcraft_engine::Session;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i).into_bytes());
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Tokens are `role:name` (document `file-1`) or `role:name:doc`; the real
/// verifier is unit-tested in the crate.
struct TestVerifier;

impl Verifier for TestVerifier {
    fn verify(&self, token: &str) -> Result<Claims, TokenError> {
        let mut parts = token.split(':');
        let role = parts.next().ok_or(TokenError::Invalid)?;
        let name = parts.next().ok_or(TokenError::Invalid)?;
        let file = parts.next().unwrap_or("file-1");
        let role = match role {
            "editor" => Role::Editor,
            "commenter" => Role::Commenter,
            "viewer" => Role::Viewer,
            _ => return Err(TokenError::Invalid),
        };
        Ok(Claims {
            subject: name.to_owned(),
            audience: "lzmkm".into(),
            file_id: file.to_owned(),
            app: "pdfcraft".into(),
            role,
            session_id: "session-1".into(),
            expires_at: u64::MAX,
        })
    }
}

fn start() -> Result<ServerHandle, Box<dyn std::error::Error>> {
    let bytes = fixture(2);
    let adapter = PdfAdapter::open("e2e.pdf", bytes.clone().into())?;
    let doc = DocKey::new("file-1")?;
    let mut hub = Hub::new(RoomConfig::default());
    hub.open_room(doc, adapter)?;
    Ok(WsServer::new(hub, Arc::new(TestVerifier), Config::default()).bind("127.0.0.1:0")?)
}

struct Peer {
    ws: WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    session: Session,
    id: pdfcraft_engine::DocId,
    version: u64,
    epoch: u64,
}

impl Peer {
    fn join(addr: std::net::SocketAddr, token: &str, name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::join_doc(addr, token, name, "file-1")
    }

    fn join_doc(addr: std::net::SocketAddr, token: &str, name: &str, doc: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let (mut ws, _) = tungstenite::connect(format!("ws://{addr}/"))?;
        if let MaybeTlsStream::Plain(stream) = ws.get_mut() {
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        }
        ws.send(Message::text(
            json!({
                "type": "hello",
                "v": 1,
                "app": "pdfcraft",
                "doc": doc,
                "jwt": token,
                "client": {"name": name, "version": "0"},
            })
            .to_string(),
        ))?;
        let mut session = Session::new();
        let id = session.open_new("e2e.pdf", fixture(2).into())?;
        let mut peer = Self { ws, session, id, version: 0, epoch: 0 };
        match peer.next()? {
            ServerMessage::Welcome(welcome) => {
                // A late joiner is welcomed at the head; a `resync` brings its copy up.
                peer.version = welcome.version;
                peer.epoch = welcome.epoch;
            }
            other => return Err(format!("expected welcome, got {other:?}").into()),
        }
        Ok(peer)
    }

    fn send(&mut self, op: Op) -> Result<(), Box<dyn std::error::Error>> {
        self.ws.send(Message::text(serde_json::to_string(&ClientMessage::Op(op))?))?;
        Ok(())
    }

    fn next(&mut self) -> Result<ServerMessage, Box<dyn std::error::Error>> {
        loop {
            match self.ws.read()? {
                Message::Text(text) => return Ok(serde_json::from_str(text.as_str())?),
                Message::Close(_) => return Err("closed".into()),
                _ => {}
            }
        }
    }

    /// Read until `applied`/`rejected`, applying accepted operations locally.
    fn settle(&mut self) -> Result<ServerMessage, Box<dyn std::error::Error>> {
        loop {
            match self.next()? {
                ServerMessage::Applied(applied) => {
                    apply(&mut self.session, self.id, &applied.op)?;
                    self.version = applied.version;
                    self.epoch = applied.epoch;
                    return Ok(ServerMessage::Applied(applied));
                }
                other @ ServerMessage::Rejected(_) => return Ok(other),
                ServerMessage::Presence(_) => {}
                other => return Err(format!("unexpected {other:?}").into()),
            }
        }
    }

    fn resync(&mut self, last_version: u64) -> Result<Vec<Op>, Box<dyn std::error::Error>> {
        self.ws.send(Message::text(serde_json::to_string(&ClientMessage::Resync(pdfcraft_collab_proto::Resync { last_version }))?))?;
        loop {
            match self.next()? {
                ServerMessage::Snapshot(snapshot) => {
                    let mut ops = Vec::new();
                    for value in snapshot.tail {
                        ops.push(serde_json::from_value::<Op>(value)?);
                    }
                    return Ok(ops);
                }
                ServerMessage::Presence(_) => {}
                other => return Err(format!("expected a snapshot, got {other:?}").into()),
            }
        }
    }

    fn names(&self) -> Vec<String> {
        let document = self.session.get(self.id).expect("the peer document is open");
        let mut names: Vec<String> = document.info.annotations.iter().filter_map(|a| a.name.clone()).collect();
        names.sort();
        names
    }

    fn rotation(&self) -> u16 {
        self.session.get(self.id).expect("the peer document is open").info.pages[0].rotation
    }
}

fn page(index: u32, epoch: u64) -> Target {
    Target::Page(PageRef { index, epoch })
}

fn annotation(name: &str, epoch: u64) -> Target {
    Target::Annotation { page: PageRef { index: 0, epoch }, nm: name.to_owned() }
}

#[test]
fn three_clients_converge_and_a_late_joiner_catches_up() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut editor = Peer::join(addr, "editor:ada", "ada")?;
    let mut commenter = Peer::join(addr, "commenter:bo", "bo")?;
    let mut viewer = Peer::join(addr, "viewer:cy", "cy")?;

    // The editor adds a comment; everybody applies the server's /NM.
    let doc = DocKey::new("file-1")?;
    let add = merge_op(doc.clone(), OpId::new("op-add")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "hello", "ada"));
    editor.send(add)?;
    for peer in [&mut editor, &mut commenter, &mut viewer] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&editor, &commenter, &viewer] {
        assert_eq!(peer.names(), vec!["ada-1".to_owned()]);
    }

    // The commenter replies in the same thread.
    let reply = merge_op(doc.clone(), OpId::new("op-reply")?, 1, 0, annotation("ada-1", 0), reply_payload("bo-1", "agreed", "bo"));
    commenter.send(reply)?;
    for peer in [&mut editor, &mut commenter, &mut viewer] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&editor, &commenter, &viewer] {
        assert_eq!(peer.names(), vec!["ada-1".to_owned(), "bo-1".to_owned()]);
    }

    // The viewer may not write, even though the transport accepts the frame.
    let forbidden = merge_op(doc.clone(), OpId::new("op-viewer")?, 2, 0, page(0, 0), add_note_payload("cy-1", [10.0, 10.0], "x", "cy"));
    viewer.send(forbidden)?;
    assert!(matches!(viewer.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Forbidden));

    // The editor rotates the page; the epoch moves and everyone follows.
    let rotate = structural_op(doc.clone(), OpId::new("op-rotate")?, editor.version, 0, page(0, 0), rotate_payload(90));
    editor.send(rotate)?;
    for peer in [&mut editor, &mut commenter, &mut viewer] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&editor, &commenter, &viewer] {
        assert_eq!(peer.rotation(), 90);
    }

    // A merge built on the old epoch is stale, not silently applied.
    let stale = merge_op(doc.clone(), OpId::new("op-stale")?, editor.version, 0, annotation("ada-1", 0), set_contents_payload("late"));
    commenter.send(stale)?;
    assert!(matches!(commenter.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Stale && r.retryable));

    // A late joiner starts from the original file and replays the journal tail.
    let mut late = Peer::join(addr, "viewer:di", "di")?;
    let tail = late.resync(0)?;
    assert_eq!(tail.len(), 3, "add, reply and rotate are journaled; rejections are not");
    for op in tail {
        apply(&mut late.session, late.id, &op)?;
    }
    assert_eq!(late.names(), vec!["ada-1".to_owned(), "bo-1".to_owned()]);
    assert_eq!(late.rotation(), 90);
    let document = late.session.get(late.id).expect("open");
    assert_eq!(document.info.annotations.len(), 2);
    Ok(())
}

/// A typed annotation (a highlight) is not a special case: two clients that
/// never share memory see the same subtype, quads, colour and text.
#[test]
fn two_clients_share_a_typed_highlight() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "commenter:bo", "bo")?;

    let quads = vec![[20.0, 200.0, 120.0, 200.0, 20.0, 190.0, 120.0, 190.0]];
    let shape = pdfcraft_engine::Shape::TextMarkup { kind: pdfcraft_engine::Markup::Highlight, quads: quads.clone() };
    let mut style = pdfcraft_engine::Style::default_for(&shape);
    style.color = [0.1, 0.7, 0.3];
    style.opacity = 0.4;
    let new = pdfcraft_engine::NewAnnotation { page: 0, style: style.clone(), shape, contents: "look here".into(), author: "ada".into() };

    let doc = DocKey::new("file-1")?;
    let payload = add_annotation_payload(&new, "ada-hl-1")?;
    let op = merge_op(doc, OpId::new("op-highlight")?, 0, 0, page(0, 0), payload);
    ada.send(op)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }

    let check = |peer: &Peer| {
        let document = peer.session.get(peer.id).expect("the peer document is open");
        let annotation = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("ada-hl-1")).expect("the highlight reached the replica");
        assert_eq!(annotation.subtype, "Highlight");
        assert_eq!(annotation.contents.as_deref(), Some("look here"));
        assert_eq!(annotation.quads.len(), 1);
        for (got, want) in annotation.quads[0].iter().zip(&quads[0]) {
            assert!((f64::from(*got) - want).abs() < 0.01);
        }
        let color = annotation.color.expect("the highlight carries its colour");
        for (got, want) in color.iter().zip(style.color) {
            assert!((f64::from(*got) - want).abs() < 0.01);
        }
    };
    check(&ada);
    check(&bo);

    // The typed payload is journaled, so a late joiner replays it.
    let mut late = Peer::join(addr, "viewer:di", "di")?;
    let tail = late.resync(0)?;
    assert_eq!(tail.len(), 1, "the highlight is the only accepted operation");
    for op in tail {
        apply(&mut late.session, late.id, &op)?;
    }
    check(&late);
    Ok(())
}

/// The room is only as good as its adapter: a commit through the wire produces
/// a readable PDF with the comments in it.
/// Last participant out: the transport commits a dirty room by itself.
#[test]
fn leaving_the_last_client_commits_the_room() -> Result<(), Box<dyn std::error::Error>> {
    struct Counting {
        inner: PdfAdapter,
        commits: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Adapter for Counting {
        fn apply(&mut self, actor: &pdfcraft_collab_proto::ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            self.inner.apply(actor, op)
        }
        fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError> {
            let outcome = self.inner.commit(version, epoch);
            if outcome.is_ok() {
                self.commits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            outcome
        }
        fn base(&self) -> Option<pdfcraft_collab_proto::BlobRef> {
            self.inner.base()
        }
    }

    let commits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let adapter = Counting { inner: PdfAdapter::open("e2e.pdf", fixture(1).into())?, commits: commits.clone() };
    let doc = DocKey::new("file-1")?;
    let mut hub = Hub::new(RoomConfig::default());
    hub.open_room(doc.clone(), adapter)?;
    let handle = WsServer::new(hub, Arc::new(TestVerifier), Config::default()).bind("127.0.0.1:0")?;
    let mut editor = Peer::join(handle.local_addr(), "editor:ada", "ada")?;
    let add = merge_op(doc, OpId::new("op-add")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "kept", "ada"));
    editor.send(add)?;
    let _ = editor.settle()?;
    drop(editor);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while commits.load(std::sync::atomic::Ordering::Relaxed) == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(commits.load(std::sync::atomic::Ordering::Relaxed), 1, "the last client leaving committed the session");
    Ok(())
}

#[test]
fn a_commit_over_the_wire_produces_a_readable_pdf() -> Result<(), Box<dyn std::error::Error>> {
    let adapter = PdfAdapter::open("e2e.pdf", fixture(1).into())?;
    let doc = DocKey::new("file-1")?;
    let mut hub = Hub::new(RoomConfig::default());
    hub.open_room(doc.clone(), adapter)?;
    let handle = WsServer::new(hub, Arc::new(TestVerifier), Config::default()).bind("127.0.0.1:0")?;

    let mut editor = Peer::join(handle.local_addr(), "editor:ada", "ada")?;
    let add = merge_op(doc.clone(), OpId::new("op-add")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "kept", "ada"));
    editor.send(add)?;
    let _ = editor.settle()?;

    editor.ws.send(Message::text(serde_json::to_string(&ClientMessage::Commit(pdfcraft_collab_proto::CommitRequest {
        expected_version: editor.version,
        message: None,
    }))?))?;
    let result = loop {
        match editor.next()? {
            ServerMessage::CommitResult(result) => break result,
            ServerMessage::Presence(_) => {}
            other => return Err(format!("expected a commit result, got {other:?}").into()),
        }
    };
    assert!(!result.conflict);
    assert_eq!(result.version, editor.version);
    assert!(!result.sha256.is_empty());
    assert!(result.bytes > 0);
    Ok(())
}

/// RFC §16, limitation 9: the cabinet call of a commit runs outside the hub
/// lock. A cabinet that does not answer for one document must not freeze
/// presence in another.
#[test]
fn a_commit_does_not_block_presence_in_another_room() -> Result<(), Box<dyn std::error::Error>> {
    /// Holds the publication until the test opens it, and says when it started.
    #[derive(Clone)]
    struct Gate {
        started: std::sync::mpsc::Sender<()>,
        release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }

    struct GatePublication(Gate);

    impl Publication for GatePublication {
        fn run(self: Box<Self>) -> PublishOutcome {
            let _ = self.0.started.send(());
            let (lock, condvar) = &*self.0.release;
            let mut released = lock.lock().expect("the gate is not poisoned");
            while !*released {
                released = condvar.wait(released).expect("the gate is not poisoned");
            }
            Ok(None)
        }
    }

    struct Slow {
        inner: PdfAdapter,
        gate: Gate,
    }

    impl Adapter for Slow {
        fn apply(&mut self, actor: &ActorId, op: &Op) -> Result<ApplyOutcome, AdapterError> {
            self.inner.apply(actor, op)
        }
        fn commit(&mut self, version: u64, epoch: u64) -> Result<CommitOutcome, AdapterError> {
            self.inner.commit(version, epoch)
        }
        fn base(&self) -> Option<pdfcraft_collab_proto::BlobRef> {
            self.inner.base()
        }
        fn take_publication(&mut self) -> Option<Box<dyn Publication>> {
            Some(Box::new(GatePublication(self.gate.clone())))
        }
        fn finish_publish(&mut self, _version: u64, _outcome: PublishOutcome) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let gate = Gate { started: started_tx, release: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())) };
    let doc_a = DocKey::new("file-1")?;
    let doc_b = DocKey::new("file-2")?;
    let mut hub = Hub::new(RoomConfig::default());
    hub.open_room(doc_a.clone(), Slow { inner: PdfAdapter::open("a.pdf", fixture(1).into())?, gate: gate.clone() })?;
    hub.open_room(doc_b.clone(), Slow { inner: PdfAdapter::open("b.pdf", fixture(1).into())?, gate: gate.clone() })?;
    let handle = WsServer::new(hub, Arc::new(TestVerifier), Config::default()).bind("127.0.0.1:0")?;
    let addr = handle.local_addr();

    let mut ada = Peer::join_doc(addr, "editor:ada:file-1", "ada", "file-1")?;
    let mut bo = Peer::join_doc(addr, "editor:bo:file-2", "bo", "file-2")?;
    let mut cy = Peer::join_doc(addr, "editor:cy:file-2", "cy", "file-2")?;

    let add = merge_op(doc_a.clone(), OpId::new("op-add")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "kept", "ada"));
    ada.send(add)?;
    assert!(matches!(ada.settle()?, ServerMessage::Applied(_)));

    // Ada commits; the cabinet call blocks until this test opens the gate.
    ada.ws.send(Message::text(serde_json::to_string(&ClientMessage::Commit(pdfcraft_collab_proto::CommitRequest {
        expected_version: ada.version,
        message: None,
    }))?))?;
    started_rx.recv_timeout(Duration::from_secs(5))?;

    // While room A is inside its cabinet call, room B still exchanges presence.
    let presence = Presence {
        actor_id: ActorId::new("bo")?,
        role: Role::Editor,
        page: PageRef { index: 0, epoch: 0 },
        viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
        cursor: Some([5, 6]),
        selection: None,
        color: "#112233".into(),
    };
    bo.ws.send(Message::text(serde_json::to_string(&ClientMessage::Presence(presence))?))?;
    match cy.next()? {
        ServerMessage::Presence(presence) => assert_eq!(presence.actor_id.as_str(), "bo"),
        other => return Err(format!("presence in the other room was blocked by the commit: {other:?}").into()),
    }

    // The commit still answers its author once the cabinet did.
    let (lock, condvar) = &*gate.release;
    *lock.lock().expect("the gate is not poisoned") = true;
    condvar.notify_all();
    let result = loop {
        match ada.next()? {
            ServerMessage::CommitResult(result) => break result,
            ServerMessage::Presence(_) => {}
            other => return Err(format!("expected a commit result, got {other:?}").into()),
        }
    };
    assert_eq!(result.version, 1);
    assert!(!result.conflict);
    Ok(())
}

/// RFC §6 over the wire: a comment and a page are deleted by one client and
/// undone by the other, and both replicas end up equal. The undo comes from the
/// server's copy — neither replica has the deleted content any more.
#[test]
fn two_clients_undo_a_deleted_comment_and_a_deleted_page() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "editor:bo", "bo")?;

    let doc = DocKey::new("file-1")?;
    let add = merge_op(doc.clone(), OpId::new("op-add")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "kept", "ada"));
    ada.send(add)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }

    // Bo deletes Ada's comment; both replicas lose it.
    let delete = merge_op(doc.clone(), OpId::new("op-delete")?, 1, 0, annotation("ada-1", 0), json!({"action": "annotation.delete"}));
    bo.send(delete)?;
    let mut deleted = None;
    for peer in [&mut ada, &mut bo] {
        match peer.settle()? {
            ServerMessage::Applied(applied) => deleted = Some(applied),
            other => return Err(format!("expected an applied delete, got {other:?}").into()),
        }
    }
    let deleted = deleted.ok_or("no applied delete")?;
    for peer in [&ada, &bo] {
        assert!(peer.names().is_empty(), "the comment is gone on every replica");
    }

    // The undo is built from the accepted operation: it names the server's copy.
    let undo = pdfcraft_collab_client_pdf::inverse_after(&deleted).ok_or("the delete kept no copy")?;
    assert_eq!(undo.payload["action"], "annotation.restore");
    ada.send(undo)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        assert_eq!(peer.names(), vec!["ada-1".to_owned()], "the comment came back from the copy");
        let document = peer.session.get(peer.id).expect("open");
        let back = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("ada-1")).expect("the comment");
        assert_eq!(back.contents.as_deref(), Some("kept"));
    }

    // The same for a page. The writer lease is Ada's (she joined first), and a
    // structural operation needs it, so Ada both deletes and undoes here; Bo's
    // replica is the proof that the page comes back from the server's copy.
    let delete = structural_op(doc.clone(), OpId::new("op-page-delete")?, ada.version, 0, page(1, 0), json!({"action": "page.delete"}));
    ada.send(delete)?;
    let mut deleted = None;
    for peer in [&mut ada, &mut bo] {
        match peer.settle()? {
            ServerMessage::Applied(applied) => deleted = Some(applied),
            other => return Err(format!("expected an applied page delete, got {other:?}").into()),
        }
    }
    let deleted = deleted.ok_or("no applied page delete")?;
    for peer in [&ada, &bo] {
        assert_eq!(peer.session.get(peer.id).expect("open").info.pages.len(), 1);
    }

    let undo = pdfcraft_collab_client_pdf::inverse_after(&deleted).ok_or("the page delete kept no copy")?;
    assert_eq!(undo.payload["action"], "page.restore");
    ada.send(pdfcraft_collab_proto::Op { op_id: OpId::new("op-page-undo")?, ..undo })?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        let document = peer.session.get(peer.id).expect("open");
        assert_eq!(document.info.pages.len(), 2, "the page is back on every replica");
    }

    // A rotation undoes through the same path, and its target follows the epoch
    // the head is on (the inverse is built on the epoch before the turn).
    let rotate = structural_op(doc.clone(), OpId::new("op-rotate")?, ada.version, ada.epoch, page(0, ada.epoch), rotate_payload(90));
    // An inverse is built from the document as it was *before* the operation, as
    // the app does when it queues one.
    let undo = pdfcraft_collab_client_pdf::inverse_for(ada.session.get(ada.id).ok_or("open")?, &rotate, OpId::new("op-rotate-undo")?)?
        .ok_or("a rotation can be undone")?;
    ada.send(rotate)?;
    let mut turned = None;
    for peer in [&mut ada, &mut bo] {
        match peer.settle()? {
            ServerMessage::Applied(applied) => turned = Some(applied),
            other => return Err(format!("expected an applied rotate, got {other:?}").into()),
        }
    }
    let turned = turned.ok_or("no applied rotate")?;
    assert_eq!(turned.epoch, ada.epoch);
    // The inverse is built on the epoch before the turn; the client re-addresses
    // it to the head when it queues it (`ClientSession::enqueue`), and this peer
    // sends raw frames.
    let undo = Op { base_version: ada.version, epoch: ada.epoch, target: Target::Page(PageRef { index: 0, epoch: ada.epoch }), ..undo };
    ada.send(undo)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)), "the undo is accepted, not stale");
    }
    for peer in [&ada, &bo] {
        assert_eq!(peer.rotation(), 0, "the page is upright on every replica");
    }
    Ok(())
}
