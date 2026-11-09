//! End-to-end: three clients over a real socket, one authoritative PDF.
//!
//! The clients do not share memory with the server: each receives `applied`,
//! projects the accepted operation with `pdfcraft-collab-client-pdf` and edits
//! its own document. The test checks that all three documents end up equal, that
//! a viewer cannot write, that an old epoch is refused, and that a late joiner
//! catches up from a snapshot tail.

use pdfcraft_collab_client_pdf::{
    add_annotation_payload, add_note_payload, apply, blank_page_payload, combine_pages_payload, duplicate_page_payload, insert_page_payload,
    merge_op, replace_page_payload, reply_payload, rotate_payload, set_contents_payload, split_pages_payload, structural_op,
};
use pdfcraft_collab_hub::Hub;
use pdfcraft_collab_pdf::PdfAdapter;
use pdfcraft_collab_proto::{
    ActorId, ClientMessage, DocKey, ErrorCode, Op, OpId, PageRef, Presence, Role, Selection, ServerMessage, Target, Viewport,
};
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
    actor: ActorId,
    role: Role,
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
        let mut peer = Self { ws, session, id, actor: ActorId::new("pending")?, role: Role::Viewer, version: 0, epoch: 0 };
        match peer.next()? {
            ServerMessage::Welcome(welcome) => {
                // A late joiner is welcomed at the head; a `resync` brings its copy up.
                peer.actor = welcome.actor_id;
                peer.role = welcome.role;
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

    fn send_presence(&mut self, selection: Option<Selection>) -> Result<(), Box<dyn std::error::Error>> {
        let message = ClientMessage::Presence(Presence {
            actor_id: self.actor.clone(),
            role: self.role,
            page: PageRef { index: 0, epoch: self.epoch },
            viewport: Viewport { x: 0.0, y: 0.0, zoom: 1.0 },
            cursor: Some([10, 20]),
            selection,
            color: "#3366cc".into(),
        });
        self.ws.send(Message::text(serde_json::to_string(&message)?))?;
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

    fn page_count(&self) -> usize {
        self.session.get(self.id).expect("the peer document is open").info.pages.len()
    }

    fn saved(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(self.session.save_bytes(self.id)?.as_ref().clone())
    }
}

/// The text of one page of a saved PDF, as a reader sees it.
fn page_text(bytes: Vec<u8>, page: usize) -> Result<String, Box<dyn std::error::Error>> {
    let mut session = Session::new();
    let id = session.open("check.pdf", None, Arc::new(bytes), None)?;
    let document = session.get(id).ok_or("the check document is open")?;
    let mut exporter = pdfcraft_engine::export::Exporter::new(document);
    Ok(exporter.text(page)?.trim().to_owned())
}

/// One page of `bytes` as a PDF of its own: what an insert or a duplicate
/// carries on the wire.
fn one_page_copy(bytes: &[u8], index: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut scratch = Session::new();
    let id = scratch.open("copy.pdf", None, Arc::new(bytes.to_vec()), None)?;
    let count = scratch.get(id).ok_or("the copy document is open")?.info.pages.len();
    if index >= count {
        return Err("the page is not in the source".into());
    }
    if count > 1 {
        let others: Vec<usize> = (0..count).filter(|i| *i != index).collect();
        scratch.apply(id, pdfcraft_engine::Edit::DeletePages { pages: others })?;
    }
    Ok(scratch.save_bytes(id)?.as_ref().clone())
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

/// Holds the publication until the test opens it, and says when it started.
#[derive(Clone)]
struct Gate {
    started: std::sync::mpsc::Sender<()>,
    release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

struct GatePublication(Gate);

impl Publication for GatePublication {
    fn run(&self) -> PublishOutcome {
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

/// RFC §16, limitation 9: the cabinet call of a commit runs outside the hub
/// lock. A cabinet that does not answer for one document must not freeze
/// presence in another.
#[test]
fn a_commit_does_not_block_presence_in_another_room() -> Result<(), Box<dyn std::error::Error>> {
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

/// RFC §4.6, limitation 10: a live room compacts its journal on its own once
/// the threshold is crossed, and the cabinet call of that compaction runs
/// outside the hub lock. A room whose cabinet does not answer must not freeze
/// another room, and the author still sees its edit acknowledged first.
#[test]
fn a_forced_commit_does_not_block_another_room_and_is_counted() -> Result<(), Box<dyn std::error::Error>> {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let gate = Gate { started: started_tx, release: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())) };
    let doc_a = DocKey::new("file-1")?;
    let doc_b = DocKey::new("file-2")?;
    // One operation fills the journal of either room.
    let mut hub = Hub::new(RoomConfig { forced_commit_ops: 1, forced_commit_bytes: 0, ..RoomConfig::default() });
    hub.open_room(doc_a.clone(), Slow { inner: PdfAdapter::open("a.pdf", fixture(1).into())?, gate: gate.clone() })?;
    hub.open_room(doc_b.clone(), Slow { inner: PdfAdapter::open("b.pdf", fixture(1).into())?, gate: gate.clone() })?;
    let server = Arc::new(WsServer::new(hub, Arc::new(TestVerifier), Config::default()));
    let handle = server.clone().bind_arc("127.0.0.1:0")?;
    let addr = handle.local_addr();

    let mut ada = Peer::join_doc(addr, "editor:ada:file-1", "ada", "file-1")?;
    let mut bo = Peer::join_doc(addr, "editor:bo:file-2", "bo", "file-2")?;

    // The server compacts room A on its own after this edit; the cabinet call
    // blocks until this test opens the gate.
    let add = merge_op(doc_a.clone(), OpId::new("op-forced")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "kept", "ada"));
    ada.send(add)?;
    assert!(matches!(ada.settle()?, ServerMessage::Applied(_)), "the edit is acknowledged before the cabinet answers");
    started_rx.recv_timeout(Duration::from_secs(5))?;

    let metrics = server.metrics()?;
    assert_eq!(metrics.forced_commits, 1, "the compaction is counted as a forced commit");

    // Room B is not behind room A's cabinet call: it accepts an edit, not just
    // presence.
    let note = merge_op(doc_b.clone(), OpId::new("op-b")?, 0, 0, page(0, 0), add_note_payload("bo-1", [30.0, 240.0], "b", "bo"));
    bo.send(note)?;
    assert!(matches!(bo.settle()?, ServerMessage::Applied(_)), "room B was blocked by room A's forced commit");

    // The publication is reported; room A keeps working and crosses the
    // threshold again with its next edit.
    let (lock, condvar) = &*gate.release;
    *lock.lock().expect("the gate is not poisoned") = true;
    condvar.notify_all();
    let second =
        merge_op(doc_a.clone(), OpId::new("op-after")?, ada.version, 0, page(0, 0), add_note_payload("ada-2", [40.0, 230.0], "after", "ada"));
    ada.send(second)?;
    assert!(matches!(ada.settle()?, ServerMessage::Applied(_)));
    Ok(())
}

/// The test's wall clock, for the sweep that the server itself calls with
/// `std::time::SystemTime`.
fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)).unwrap_or(0)
}

/// RFC §4.6, limitation 10, the time threshold: a room whose journal is small
/// and that sees no further edit compacts itself. No message arrives from any
/// client — no edit, no join, no leave — so the background sweep is the only
/// thing that can notice, and it counts the compaction like any other forced
/// commit.
#[test]
fn an_idle_room_compacts_its_journal_without_another_edit() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counted {
        inner: PdfAdapter,
        runs: Arc<AtomicUsize>,
    }

    struct CountedPublication(Arc<AtomicUsize>);

    impl Publication for CountedPublication {
        fn run(&self) -> PublishOutcome {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }
    }

    impl Adapter for Counted {
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
            Some(Box::new(CountedPublication(self.runs.clone())))
        }
        fn finish_publish(&mut self, _version: u64, _outcome: PublishOutcome) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    let runs = Arc::new(AtomicUsize::new(0));
    let doc = DocKey::new("file-1")?;
    // One comment never reaches the operation or byte threshold: only the idle
    // time can make this room compact itself.
    let config = RoomConfig { forced_commit_ops: 1_000, forced_commit_bytes: 0, forced_commit_idle_secs: 1, ..RoomConfig::default() };
    let mut hub = Hub::new(config);
    hub.open_room(doc.clone(), Counted { inner: PdfAdapter::open("idle.pdf", fixture(1).into())?, runs: runs.clone() })?;
    let server = Arc::new(WsServer::new(hub, Arc::new(TestVerifier), Config::default()));
    let handle = server.clone().bind_arc("127.0.0.1:0")?;

    let mut ada = Peer::join(handle.local_addr(), "editor:ada", "ada")?;
    let add = merge_op(doc.clone(), OpId::new("op-idle")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "idle", "ada"));
    ada.send(add)?;
    assert!(matches!(ada.settle()?, ServerMessage::Applied(_)));
    assert_eq!(server.metrics()?.forced_commits, 0, "one edit is below the count and byte thresholds");
    assert_eq!(runs.load(Ordering::Relaxed), 0);

    // No client says anything after the edit: a later clock is all the sweep
    // needs. (The server's own clock is the wall clock; the test moves the
    // sweep's clock past the one-second idle window.)
    assert_eq!(server.sweep_forced_commits(wall_ms() + 10_000), 1);
    assert_eq!(runs.load(Ordering::Relaxed), 1, "the idle room published the base its journal held");
    assert_eq!(server.metrics()?.forced_commits, 1, "an idle compaction is counted like any forced commit");

    // The base now holds the operation, so the journal is empty: another sweep
    // queues nothing, however long the room stays quiet.
    assert_eq!(server.sweep_forced_commits(wall_ms() + 60_000), 0);
    assert_eq!(server.metrics()?.forced_commits, 1);
    Ok(())
}

/// The same time threshold, reached by the daemon's background loop rather than
/// by a hand-called sweep: the loop is the only thing running while a quiet
/// room waits, so the wiring, not just the decision, has to be tested.
#[test]
fn the_background_loop_compacts_an_idle_room() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counted {
        inner: PdfAdapter,
        runs: Arc<AtomicUsize>,
    }

    struct CountedPublication(Arc<AtomicUsize>);

    impl Publication for CountedPublication {
        fn run(&self) -> PublishOutcome {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }
    }

    impl Adapter for Counted {
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
            Some(Box::new(CountedPublication(self.runs.clone())))
        }
        fn finish_publish(&mut self, _version: u64, _outcome: PublishOutcome) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    let runs = Arc::new(AtomicUsize::new(0));
    let doc = DocKey::new("file-1")?;
    let config = RoomConfig { forced_commit_ops: 1_000, forced_commit_bytes: 0, forced_commit_idle_secs: 1, ..RoomConfig::default() };
    let mut hub = Hub::new(config);
    hub.open_room(doc.clone(), Counted { inner: PdfAdapter::open("loop.pdf", fixture(1).into())?, runs: runs.clone() })?;
    let server = Arc::new(WsServer::new(hub, Arc::new(TestVerifier), Config::default()));
    let handle = server.clone().bind_arc("127.0.0.1:0")?;
    let queue = server.spawn_publication_retries(Duration::from_millis(20))?;

    let mut ada = Peer::join(handle.local_addr(), "editor:ada", "ada")?;
    let add = merge_op(doc.clone(), OpId::new("op-loop")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "loop", "ada"));
    ada.send(add)?;
    assert!(matches!(ada.settle()?, ServerMessage::Applied(_)));

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while server.metrics()?.forced_commits == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(server.metrics()?.forced_commits, 1, "the background loop compacted the idle journal on its own");
    assert_eq!(runs.load(Ordering::Relaxed), 1, "exactly one publication: the loop does not spin on an empty journal");
    drop(queue);
    Ok(())
}

/// RFC §8.2, limitation 6: a publication that fails with a retryable error is
/// tried again in the background after a backoff — not on the next commit, and
/// not on the last client leaving. The room stays `publishing` while the retry
/// is queued, so a second commit is `busy` rather than reaching the cabinet out
/// of order.
#[test]
fn a_retryable_publication_failure_is_retried_in_the_background() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Flaky {
        inner: PdfAdapter,
        runs: Arc<AtomicUsize>,
    }

    struct FailOnce(Arc<AtomicUsize>);

    impl Publication for FailOnce {
        fn run(&self) -> PublishOutcome {
            if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
                Err(AdapterError::new(ErrorCode::Internal, "the cabinet did not answer", true))
            } else {
                Ok(None)
            }
        }
    }

    impl Adapter for Flaky {
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
            Some(Box::new(FailOnce(self.runs.clone())))
        }
    }

    let runs = Arc::new(AtomicUsize::new(0));
    let doc = DocKey::new("file-1")?;
    let mut hub = Hub::new(RoomConfig::default());
    hub.open_room(doc.clone(), Flaky { inner: PdfAdapter::open("f.pdf", fixture(1).into())?, runs: runs.clone() })?;
    let config = Config { publication_backoff: Duration::from_millis(20), publication_backoff_max: Duration::from_millis(50), ..Config::default() };
    let server = Arc::new(WsServer::new(hub, Arc::new(TestVerifier), config));
    let handle = server.clone().bind_arc("127.0.0.1:0")?;
    let queue = server.spawn_publication_retries(Duration::from_millis(5))?;

    let mut ada = Peer::join(handle.local_addr(), "editor:ada", "ada")?;
    let add = merge_op(doc.clone(), OpId::new("op-retry")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "kept", "ada"));
    ada.send(add)?;
    assert!(matches!(ada.settle()?, ServerMessage::Applied(_)));

    ada.ws.send(Message::text(serde_json::to_string(&ClientMessage::Commit(pdfcraft_collab_proto::CommitRequest {
        expected_version: ada.version,
        message: None,
    }))?))?;
    match ada.next()? {
        ServerMessage::Error(e) => assert!(e.retryable, "the commit reports the failed publication as retryable"),
        other => return Err(format!("expected the failed publication, got {other:?}").into()),
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while runs.load(Ordering::Relaxed) < 2 {
        if std::time::Instant::now() > deadline {
            return Err(format!("the queue did not retry the publication ({} attempt(s))", runs.load(Ordering::Relaxed)).into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let metrics = server.metrics()?;
    assert_eq!(metrics.publication_retries, 1, "one background retry");
    assert!(metrics.conflict.is_empty(), "a transport failure is not a conflict");
    let state = server.with_hub(|hub| hub.room(&doc).map(|room| (room.is_dirty(), room.is_publishing())))?;
    assert_eq!(state, Some((false, false)), "the retry published the base and released the room");
    drop(queue);
    Ok(())
}

/// A publication that keeps failing must not become a retry loop in the
/// transport: a non-retryable failure leaves the room dirty, and the next commit
/// (here the last-leave one) retries it — it does not spin on its own.
#[test]
fn a_failed_forced_publication_is_not_retried_in_a_loop() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Failing {
        inner: PdfAdapter,
        runs: Arc<AtomicUsize>,
    }

    struct Fail(Arc<AtomicUsize>);

    impl Publication for Fail {
        fn run(&self) -> PublishOutcome {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(AdapterError::new(ErrorCode::Internal, "the cabinet is down", false))
        }
    }

    impl Adapter for Failing {
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
            Some(Box::new(Fail(self.runs.clone())))
        }
    }

    let runs = Arc::new(AtomicUsize::new(0));
    let doc = DocKey::new("file-1")?;
    let mut hub = Hub::new(RoomConfig { forced_commit_ops: 1, forced_commit_bytes: 0, ..RoomConfig::default() });
    hub.open_room(doc.clone(), Failing { inner: PdfAdapter::open("f.pdf", fixture(1).into())?, runs: runs.clone() })?;
    let handle = WsServer::new(hub, Arc::new(TestVerifier), Config::default()).bind("127.0.0.1:0")?;
    let addr = handle.local_addr();

    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let add = merge_op(doc.clone(), OpId::new("op-fail")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "kept", "ada"));
    ada.send(add)?;
    assert!(matches!(ada.settle()?, ServerMessage::Applied(_)), "the edit is acknowledged even though the cabinet is down");
    // Leaving makes the room's last-leave commit retry the publication once.
    drop(ada);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(runs.load(Ordering::Relaxed), 2, "one forced commit and one last-leave retry; a spin would keep publishing a cabinet that is down");
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

/// Insert and duplicate travel as one-page copies: the replica that never saw
/// the client's file ends with the same pages, a duplicate keeps its address
/// after someone else's edit, the undo returns the page count, and an operation
/// addressed to a passed epoch is retryable rather than applied.
#[test]
fn two_clients_insert_and_duplicate_a_page_and_converge() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    // Ada joins first, so the writer lease is hers: structural operations are
    // serialised by the room and only the lease holder may send them. Bo is the
    // second replica, and the proof that the pages travel on the wire.
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "editor:bo", "bo")?;
    let doc = DocKey::new("file-1")?;
    let original = fixture(2);

    // Bo edits page 0 first (a merge edit: it does not move the epoch). The
    // duplicate below must still find its page after that.
    let comment = merge_op(doc.clone(), OpId::new("op-comment")?, 0, 0, page(0, 0), add_note_payload("bo-1", [40.0, 250.0], "note", "bo"));
    bo.send(comment)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }

    // Ada inserts a copy of page 1 at the top.
    let copy = one_page_copy(&original, 1)?;
    let insert = structural_op(doc.clone(), OpId::new("op-insert")?, ada.version, ada.epoch, page(0, ada.epoch), insert_page_payload(&copy, 0));
    ada.send(insert)?;
    let mut inserted = None;
    for peer in [&mut ada, &mut bo] {
        match peer.settle()? {
            ServerMessage::Applied(applied) => inserted = Some(applied),
            other => return Err(format!("expected an applied insert, got {other:?}").into()),
        }
    }
    let inserted = inserted.ok_or("no applied insert")?;
    assert_eq!(inserted.epoch, 1, "an insert is structural");
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 3, "the page is on every replica");
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 2", "the copy is the page Ada sent");
        assert_eq!(page_text(peer.saved()?, 1)?, "Page 1", "the original first page moved down");
    }

    // The undo of the insert is a delete of the page it put there.
    let at = inserted.op.payload["at"].as_u64().ok_or("the accepted insert names its position")?;
    let undo = structural_op(
        doc.clone(),
        OpId::new("op-insert-undo")?,
        ada.version,
        ada.epoch,
        page(u32::try_from(at)?, ada.epoch),
        json!({"action": "page.delete"}),
    );
    ada.send(undo)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 2, "the undo returns the page count");
    }

    // Duplicate page 0 right after itself.
    let copy = one_page_copy(&original, 0)?;
    let duplicate =
        structural_op(doc.clone(), OpId::new("op-duplicate")?, ada.version, ada.epoch, page(0, ada.epoch), duplicate_page_payload(&copy, 1));
    ada.send(duplicate)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 3);
        assert_eq!(page_text(peer.saved()?, 1)?, "Page 1", "the duplicate follows its source");
    }

    // An insert addressed to the epoch before the last structural operation is
    // stale and retryable: the client resyncs and re-addresses the page.
    let stale = structural_op(doc.clone(), OpId::new("op-stale-insert")?, ada.version, 0, page(0, 0), insert_page_payload(&copy, 0));
    ada.send(stale)?;
    assert!(matches!(ada.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Stale && r.retryable));
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 3, "the stale insert changed nothing");
    }
    Ok(())
}

/// Read until both replicas have applied the same operation.
fn applied_on_both(first: &mut Peer, second: &mut Peer) -> Result<pdfcraft_collab_proto::Applied, Box<dyn std::error::Error>> {
    let mut applied = None;
    for peer in [&mut *first, &mut *second] {
        match peer.settle()? {
            ServerMessage::Applied(accepted) => applied = Some(accepted),
            other => return Err(format!("expected an applied operation, got {other:?}").into()),
        }
    }
    applied.ok_or_else(|| "no applied operation".into())
}

/// Replace Pages travels as an addressed operation carrying one page: both
/// replicas converge, an over-limit or misaddressed replacement is refused
/// without a change, and a blank insert names the size of the page it adds.
#[test]
fn two_clients_replace_a_page_and_insert_a_blank_one() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    // Ada joins first, so the writer lease is hers: structural operations are
    // serialised and only the lease holder may send them.
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "editor:bo", "bo")?;
    let doc = DocKey::new("file-1")?;
    let original = fixture(2);

    // Ada replaces page 0 with a copy of page 1.
    let copy = one_page_copy(&original, 1)?;
    let replace = structural_op(doc.clone(), OpId::new("op-replace")?, ada.version, ada.epoch, page(0, ada.epoch), replace_page_payload(&copy));
    ada.send(replace)?;
    let replaced = applied_on_both(&mut ada, &mut bo)?;
    assert_eq!(replaced.epoch, 1, "a replacement is structural");
    assert!(replaced.result["copy"].is_string(), "the server kept the page it replaced");
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 2, "a replacement does not change the page count");
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 2", "the page is the copy Ada sent");
    }

    // A replacement over the budget is refused before anything changes, and so
    // is one addressed to a page that is not there.
    let huge = vec![0u8; pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES + 1];
    let oversized = structural_op(doc.clone(), OpId::new("op-oversized")?, ada.version, ada.epoch, page(0, ada.epoch), replace_page_payload(&huge));
    ada.send(oversized)?;
    assert!(matches!(ada.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Limit && !r.retryable));
    let missing = structural_op(doc.clone(), OpId::new("op-missing")?, ada.version, ada.epoch, page(5, ada.epoch), replace_page_payload(&copy));
    ada.send(missing)?;
    assert!(matches!(ada.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Invalid && !r.retryable));
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 2, "a refusal changes nothing");
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 2", "the refused replacement is not applied");
    }

    // A blank page of an explicit size, after page 0.
    let blank = structural_op(doc.clone(), OpId::new("op-blank")?, ada.version, ada.epoch, page(1, ada.epoch), blank_page_payload(300.0, 400.0));
    ada.send(blank)?;
    let inserted = applied_on_both(&mut ada, &mut bo)?;
    assert_eq!(inserted.epoch, 2);
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 3);
        let document = peer.session.get(peer.id).ok_or("the peer document is open")?;
        let crop = document.info.pages[1].crop;
        assert_eq!((crop[2] - crop[0]).abs(), 300.0, "the blank page has the width the operation named");
        assert_eq!((crop[3] - crop[1]).abs(), 400.0);
    }
    Ok(())
}

/// The undo of a replacement is the operation the server's copy names: the old
/// page comes back exactly on both replicas, the redo repeats the replacement,
/// and an operation addressed to a passed epoch changes nothing.
#[test]
fn a_page_replacement_undoes_from_the_copy_and_redoes() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "editor:bo", "bo")?;
    let doc = DocKey::new("file-1")?;
    let copy = one_page_copy(&fixture(2), 1)?;

    let replace = structural_op(doc.clone(), OpId::new("op-replace")?, ada.version, ada.epoch, page(0, ada.epoch), replace_page_payload(&copy));
    ada.send(replace)?;
    let replaced = applied_on_both(&mut ada, &mut bo)?;
    for peer in [&ada, &bo] {
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 2");
    }

    // The undo names the server's copy. Its own target was built in the epoch
    // the replacement left; a raw frame re-addresses it to the head, as the
    // client does when it resends after a resync.
    let undo = pdfcraft_collab_client_pdf::inverse_after(&replaced).ok_or("the replacement kept a copy")?;
    let undo = Op { op_id: OpId::new("op-replace-undo")?, base_version: ada.version, epoch: ada.epoch, target: page(0, ada.epoch), ..undo };
    ada.send(undo)?;
    applied_on_both(&mut ada, &mut bo)?;
    for peer in [&ada, &bo] {
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 1", "the page came back from the copy");
    }

    // The redo repeats the replacement at the head.
    let redo = structural_op(doc.clone(), OpId::new("op-replace-redo")?, ada.version, ada.epoch, page(0, ada.epoch), replace_page_payload(&copy));
    ada.send(redo)?;
    applied_on_both(&mut ada, &mut bo)?;
    for peer in [&ada, &bo] {
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 2", "the redo replaced the page again");
    }

    // An operation addressed to a passed epoch is retryable and changes nothing.
    let stale = structural_op(doc.clone(), OpId::new("op-stale-replace")?, ada.version, 0, page(0, 0), replace_page_payload(&copy));
    ada.send(stale)?;
    assert!(matches!(ada.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Stale && r.retryable));
    for peer in [&ada, &bo] {
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 2", "the stale operation changed nothing");
    }
    Ok(())
}

/// A split is one server-ordered structural operation: the part travels in the
/// accepted operation, both replicas lose the same range and number what stays
/// the same, and the undo joins the part back where the split took it. A
/// combine past the last page is refused without a change.
#[test]
fn two_clients_split_the_document_in_halves_and_join_it_back() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "editor:bo", "bo")?;
    let doc = DocKey::new("file-1")?;

    let split = structural_op(doc.clone(), OpId::new("op-split")?, ada.version, ada.epoch, page(0, ada.epoch), split_pages_payload(1));
    ada.send(split)?;
    let split = applied_on_both(&mut ada, &mut bo)?;
    assert_eq!(split.epoch, 1, "a split is structural");
    assert_eq!(split.result["kept"], 1);
    assert_eq!(split.result["count"], 1);
    assert!(split.result["part"]["sha256"].is_string(), "the accepted operation names the part");
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 1, "both replicas lost the same range");
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 2", "the page that stayed keeps its text and moves down");
    }

    // The undo is built from the part the accepted operation carries.
    let undo = pdfcraft_collab_client_pdf::inverse_after(&split).ok_or("the split carries its part")?;
    let undo = Op { op_id: OpId::new("op-split-undo")?, base_version: ada.version, epoch: ada.epoch, target: page(0, ada.epoch), ..undo };
    ada.send(undo)?;
    let joined = applied_on_both(&mut ada, &mut bo)?;
    assert_eq!(joined.epoch, 2);
    assert_eq!(joined.op.payload["action"], "page.combine");
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 2, "the part came back");
        assert_eq!(page_text(peer.saved()?, 0)?, "Page 1");
        assert_eq!(page_text(peer.saved()?, 1)?, "Page 2", "the part is back at the index the split took it from");
    }

    // A combine past the last page is the document's boundary: refused, and
    // nothing changes.
    let past =
        structural_op(doc.clone(), OpId::new("op-combine-past")?, ada.version, ada.epoch, page(3, ada.epoch), combine_pages_payload(&fixture(1), 1));
    ada.send(past)?;
    assert!(matches!(ada.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Invalid && !r.retryable));
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 2, "a refused combine changes nothing");
    }
    Ok(())
}

/// Combine Files in one confirmation: one operation per part, sent in the
/// list's order (a structural operation is accepted only at the head), both
/// replicas converge, and Undo takes the parts out in reverse order — the last
/// part leaves first, so the parts before it keep the positions they joined at.
/// A part that goes stale takes the parts already joined out again, so the
/// confirmation is all or nothing.
#[test]
fn two_clients_combine_parts_in_order_and_roll_back_a_stale_one() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "editor:bo", "bo")?;
    let doc = DocKey::new("file-1")?;
    let first = fixture(2);
    let second = fixture(1);

    // The first part appends; the second starts where the first ends.
    let one = structural_op(doc.clone(), OpId::new("op-part-1")?, ada.version, ada.epoch, page(2, ada.epoch), combine_pages_payload(&first, 2));
    ada.send(one)?;
    let one = applied_on_both(&mut ada, &mut bo)?;
    assert_eq!(one.result["at"], json!(2));
    let two = structural_op(doc.clone(), OpId::new("op-part-2")?, ada.version, ada.epoch, page(4, ada.epoch), combine_pages_payload(&second, 1));
    ada.send(two)?;
    let two = applied_on_both(&mut ada, &mut bo)?;
    assert_eq!(two.result["at"], json!(4));
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 5, "both parts joined in order");
    }

    // Undo: the last part leaves first, then the first at its own index.
    let mut undo_two = pdfcraft_collab_client_pdf::inverse_after(&two).ok_or("the combine can be undone")?;
    undo_two.op_id = OpId::new("op-part-2-undo")?;
    undo_two.base_version = ada.version;
    undo_two.epoch = ada.epoch;
    if let Target::Page(page) = &mut undo_two.target {
        page.epoch = ada.epoch;
    }
    ada.send(undo_two)?;
    applied_on_both(&mut ada, &mut bo)?;
    let mut undo_one = pdfcraft_collab_client_pdf::inverse_after(&one).ok_or("the combine can be undone")?;
    undo_one.op_id = OpId::new("op-part-1-undo")?;
    undo_one.base_version = ada.version;
    undo_one.epoch = ada.epoch;
    if let Target::Page(page) = &mut undo_one.target {
        page.epoch = ada.epoch;
    }
    ada.send(undo_one)?;
    applied_on_both(&mut ada, &mut bo)?;
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 2, "the confirmation is fully taken back");
    }

    // A second confirmation: the first part joins, the second is stale (built on
    // a passed epoch) and changes nothing — the joined part is taken out again.
    let joined =
        structural_op(doc.clone(), OpId::new("op-part-retry-1")?, ada.version, ada.epoch, page(2, ada.epoch), combine_pages_payload(&first, 2));
    ada.send(joined)?;
    let joined = applied_on_both(&mut ada, &mut bo)?;
    let stale = structural_op(doc.clone(), OpId::new("op-part-retry-2")?, ada.version, 0, page(4, 0), combine_pages_payload(&second, 1));
    ada.send(stale)?;
    assert!(matches!(ada.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Stale && r.retryable));
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 4, "the stale part changed nothing; the joined one is still there");
    }
    let mut rollback = pdfcraft_collab_client_pdf::inverse_after(&joined).ok_or("the joined part can be taken out")?;
    rollback.op_id = OpId::new("op-part-retry-rollback")?;
    rollback.base_version = ada.version;
    rollback.epoch = ada.epoch;
    if let Target::Page(page) = &mut rollback.target {
        page.epoch = ada.epoch;
    }
    ada.send(rollback)?;
    applied_on_both(&mut ada, &mut bo)?;
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 2, "the confirmation rolled back to the page set before it");
    }
    Ok(())
}

/// A structural operation is serialised by version and addressed by epoch: a
/// split built on a passed epoch is `stale` (retryable) and changes nothing;
/// re-addressed to the head, the same intent applies on both replicas.
#[test]
fn a_split_addressed_to_a_passed_epoch_is_stale_and_retried() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "editor:bo", "bo")?;
    let doc = DocKey::new("file-1")?;

    // A rotation moves the epoch to 1 before the split is retried.
    let turn = structural_op(doc.clone(), OpId::new("op-rotate")?, ada.version, ada.epoch, page(0, ada.epoch), rotate_payload(90));
    ada.send(turn)?;
    applied_on_both(&mut ada, &mut bo)?;

    let stale = structural_op(doc.clone(), OpId::new("op-split-stale")?, ada.version, 0, page(0, 0), split_pages_payload(1));
    ada.send(stale)?;
    assert!(matches!(ada.settle()?, ServerMessage::Rejected(r) if r.code == ErrorCode::Stale && r.retryable));
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 2, "the stale split changed nothing");
        assert_eq!(peer.rotation(), 90, "and did not move the epoch on");
    }

    let retry = structural_op(doc.clone(), OpId::new("op-split-retry")?, ada.version, ada.epoch, page(0, ada.epoch), split_pages_payload(1));
    ada.send(retry)?;
    let split = applied_on_both(&mut ada, &mut bo)?;
    assert_eq!(split.epoch, 2, "the retried split applied");
    for peer in [&ada, &bo] {
        assert_eq!(peer.page_count(), 1);
    }
    Ok(())
}

/// The look of the comment `/NM` on a replica.
fn style_of(peer: &Peer, nm: &str) -> Result<pdfcraft_engine::CommentProps, Box<dyn std::error::Error>> {
    let document = peer.session.get(peer.id).ok_or("the peer document is open")?;
    let comment = document.info.annotations.iter().find(|a| a.name.as_deref() == Some(nm)).ok_or("the comment is not on this replica")?;
    document.comment_props(comment.page, comment.index).ok_or_else(|| "the comment has no properties".into())
}

fn near_color(got: Option<[f64; 3]>, want: [f64; 3]) -> bool {
    got.is_some_and(|got| (0..3).all(|i| (got[i] - want[i]).abs() < 1e-3))
}

/// Restyling an existing comment is one operation: both replicas see the same
/// final look, the accepted operation carries the look it replaced (the undo),
/// and an undo whose look someone else replaced is a conflict, not an overwrite.
#[test]
fn two_clients_restyle_a_comment_and_undo_it() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "commenter:bo", "bo")?;

    let doc = DocKey::new("file-1")?;
    let shape = pdfcraft_engine::Shape::Rectangle { rect: [10.0, 10.0, 80.0, 40.0] };
    let mut style = pdfcraft_engine::Style::default_for(&shape);
    style.color = [1.0, 0.0, 0.0];
    style.width = 1.0;
    let new = pdfcraft_engine::NewAnnotation { page: 0, shape, style, contents: String::new(), author: "ada".into() };
    let add = merge_op(doc.clone(), OpId::new("op-add")?, 0, 0, page(0, 0), add_annotation_payload(&new, "ada-rect")?);
    ada.send(add)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        assert!(near_color(style_of(peer, "ada-rect")?.color, [1.0, 0.0, 0.0]));
    }

    // Bo restyles the comment: colour, opacity, width and fill in one operation.
    let update = merge_op(
        doc.clone(),
        OpId::new("op-style")?,
        1,
        0,
        annotation("ada-rect", 0),
        json!({"action": "annotation.update",
               "style": {"color": [0.0, 0.0, 1.0], "opacity": 0.5, "width": 3.0, "fill": [0.0, 1.0, 0.0]}}),
    );
    bo.send(update)?;
    let mut applied = None;
    for peer in [&mut ada, &mut bo] {
        match peer.settle()? {
            ServerMessage::Applied(applied_op) => applied = Some(applied_op),
            other => return Err(format!("expected an applied restyle, got {other:?}").into()),
        }
    }
    let applied = applied.ok_or("no applied restyle")?;
    for peer in [&ada, &bo] {
        let style = style_of(peer, "ada-rect")?;
        assert!(near_color(style.color, [0.0, 0.0, 1.0]), "the colour is on every replica");
        assert!((style.opacity - 0.5).abs() < 1e-6);
        assert!((style.width.unwrap_or_default() - 3.0).abs() < 1e-6);
        assert!(near_color(style.fill, [0.0, 1.0, 0.0]), "the fill is on every replica");
    }

    // The undo is built from the accepted operation: the look the restyle replaced.
    let undo = pdfcraft_collab_client_pdf::inverse_after(&applied).ok_or("the restyle has no inverse")?;
    assert_eq!(undo.payload["action"], "annotation.update");
    assert_eq!(undo.payload["style"]["color"], json!([1.0, 0.0, 0.0]));
    ada.send(Op { op_id: OpId::new("op-undo")?, ..undo.clone() })?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        let style = style_of(peer, "ada-rect")?;
        assert!(near_color(style.color, [1.0, 0.0, 0.0]), "the old colour is back on every replica");
        assert!((style.width.unwrap_or_default() - 1.0).abs() < 1e-6);
        assert_eq!(style.fill, None, "the fill the restyle added is gone");
    }

    // Someone restyles the same comment again: the undo names the look it left
    // (blue) and is refused, so their look stays.
    let theirs = merge_op(
        doc.clone(),
        OpId::new("op-style-2")?,
        3,
        0,
        annotation("ada-rect", 0),
        json!({"action": "annotation.update", "style": {"color": [0.0, 1.0, 0.0]}}),
    );
    bo.send(theirs)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    let mut late = undo;
    late.base_version = 4;
    late.op_id = OpId::new("op-undo-2")?;
    ada.send(late)?;
    // A rejection answers its sender only; nothing is broadcast.
    match ada.settle()? {
        ServerMessage::Rejected(rejected) => {
            assert_eq!(rejected.code, ErrorCode::Conflict);
            assert!(!rejected.retryable);
        }
        other => return Err(format!("expected a rejected undo, got {other:?}").into()),
    }
    for peer in [&ada, &bo] {
        assert!(near_color(style_of(peer, "ada-rect")?.color, [0.0, 1.0, 0.0]), "the other restyle is kept");
    }
    Ok(())
}

fn add_shape_on_both(
    ada: &mut Peer,
    bo: &mut Peer,
    doc: &DocKey,
    shape: pdfcraft_engine::Shape,
    name: &str,
    op_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let style = pdfcraft_engine::Style::default_for(&shape);
    let new = pdfcraft_engine::NewAnnotation { page: 0, shape, style, contents: "text".into(), author: "ada".into() };
    let add = merge_op(doc.clone(), OpId::new(op_id)?, ada.version, 0, page(0, 0), add_annotation_payload(&new, name)?);
    ada.send(add)?;
    for peer in [&mut *ada, &mut *bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    Ok(())
}

/// Send `op` from `sender`; both replicas apply the accepted operation, which is
/// returned (it carries what the undo needs).
fn apply_on_both(sender: &mut Peer, other: &mut Peer, op: Op) -> Result<pdfcraft_collab_proto::Applied, Box<dyn std::error::Error>> {
    sender.send(op)?;
    let applied = match sender.settle()? {
        ServerMessage::Applied(applied) => applied,
        other => return Err(format!("expected an applied operation, got {other:?}").into()),
    };
    match other.settle()? {
        ServerMessage::Applied(_) => {}
        other => return Err(format!("expected the accepted operation on the second replica, got {other:?}").into()),
    }
    Ok(applied)
}

/// A line's endings are part of its look: both replicas see the new pair, and the
/// undo from the accepted operation puts the old one back — one operation each.
#[test]
fn two_clients_change_line_endings_and_undo_them() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "commenter:bo", "bo")?;
    let doc = DocKey::new("file-1")?;
    let line = pdfcraft_engine::Shape::Line {
        from: [20.0, 20.0],
        to: [150.0, 100.0],
        start: pdfcraft_engine::LineEnding::None,
        end: pdfcraft_engine::LineEnding::OpenArrow,
    };
    add_shape_on_both(&mut ada, &mut bo, &doc, line, "ada-line", "op-add-line")?;

    let base = bo.version;
    let applied = apply_on_both(
        &mut bo,
        &mut ada,
        merge_op(
            doc.clone(),
            OpId::new("op-endings")?,
            base,
            0,
            annotation("ada-line", 0),
            json!({"action": "annotation.update", "style": {"endings": ["square", "closed_arrow"]}}),
        ),
    )?;
    for peer in [&ada, &bo] {
        assert_eq!(
            style_of(peer, "ada-line")?.endings,
            Some(vec![pdfcraft_engine::LineEnding::Square, pdfcraft_engine::LineEnding::ClosedArrow]),
            "the endings are on every replica"
        );
    }
    let undo = pdfcraft_collab_client_pdf::inverse_after(&applied).ok_or("the endings have no inverse")?;
    assert_eq!(undo.payload["action"], "annotation.update");
    assert_eq!(undo.payload["style"]["endings"], json!(["none", "open_arrow"]));
    apply_on_both(&mut ada, &mut bo, Op { op_id: OpId::new("op-undo-endings")?, ..undo })?;
    for peer in [&ada, &bo] {
        assert_eq!(
            style_of(peer, "ada-line")?.endings,
            Some(vec![pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::OpenArrow]),
            "the old endings are back on every replica"
        );
    }
    Ok(())
}

/// A text box's font size is one addressable operation with a compare-and-set
/// undo, and a note's author/subject/icon is another; both replicas converge on
/// each and the undo restores the previous properties.
#[test]
fn two_clients_resize_text_and_rename_a_note_and_undo_them() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "commenter:bo", "bo")?;
    let doc = DocKey::new("file-1")?;
    let box_ = pdfcraft_engine::Shape::TextBox { rect: [20.0, 120.0, 180.0, 170.0], font_size: 10.0 };
    let note = pdfcraft_engine::Shape::Note { at: [60.0, 240.0], icon: pdfcraft_engine::NoteIcon::Comment };
    add_shape_on_both(&mut ada, &mut bo, &doc, box_, "ada-box", "op-add-box")?;
    add_shape_on_both(&mut ada, &mut bo, &doc, note, "ada-note", "op-add-note")?;

    // The font size.
    let base = bo.version;
    let applied = apply_on_both(
        &mut bo,
        &mut ada,
        merge_op(doc.clone(), OpId::new("op-size")?, base, 0, annotation("ada-box", 0), json!({"action": "annotation.font_size", "size": 16.0})),
    )?;
    for peer in [&ada, &bo] {
        assert_eq!(style_of(peer, "ada-box")?.font_size, Some(16.0), "the size is on every replica");
    }
    let undo = pdfcraft_collab_client_pdf::inverse_after(&applied).ok_or("the font size has no inverse")?;
    assert_eq!(undo.payload["action"], "annotation.font_size");
    assert_eq!(undo.payload["size"], json!(10.0));
    assert_eq!(undo.payload["expect"], json!(16.0));
    apply_on_both(&mut ada, &mut bo, Op { op_id: OpId::new("op-undo-size")?, ..undo })?;
    for peer in [&ada, &bo] {
        assert_eq!(style_of(peer, "ada-box")?.font_size, Some(10.0), "the old size is back on every replica");
    }

    // The note's author, subject and icon, in one operation.
    let base = bo.version;
    let applied = apply_on_both(
        &mut bo,
        &mut ada,
        merge_op(
            doc.clone(),
            OpId::new("op-info")?,
            base,
            0,
            annotation("ada-note", 0),
            json!({"action": "annotation.info", "info": {"author": "bo", "subject": "Review", "icon": "help"}}),
        ),
    )?;
    for peer in [&ada, &bo] {
        let props = style_of(peer, "ada-note")?;
        assert_eq!(props.author, "bo", "the author is on every replica");
        assert_eq!(props.subject, "Review");
        assert_eq!(props.icon, Some(pdfcraft_engine::NoteIcon::Help));
    }
    assert_eq!(applied.op.payload["before"]["author"], json!("ada"));
    assert_eq!(applied.op.payload["before"]["subject"], json!("Sticky Note"), "the note's own subject");
    assert_eq!(applied.op.payload["before"]["icon"], json!("comment"));
    let undo = pdfcraft_collab_client_pdf::inverse_after(&applied).ok_or("the info change has no inverse")?;
    assert_eq!(undo.payload["action"], "annotation.info");
    apply_on_both(&mut ada, &mut bo, Op { op_id: OpId::new("op-undo-info")?, ..undo })?;
    for peer in [&ada, &bo] {
        let props = style_of(peer, "ada-note")?;
        assert_eq!(props.author, "ada", "the old author is back on every replica");
        assert_eq!(props.subject, "Sticky Note");
        assert_eq!(props.icon, Some(pdfcraft_engine::NoteIcon::Comment));
    }
    Ok(())
}

/// A style detail the comment cannot carry is refused before the document
/// changes, and an undo whose value someone else replaced is a conflict: neither
/// moves the version and both replicas keep the other user's change.
#[test]
fn a_refused_style_detail_and_a_stale_undo_change_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "commenter:bo", "bo")?;
    let doc = DocKey::new("file-1")?;
    let note = pdfcraft_engine::Shape::Note { at: [60.0, 240.0], icon: pdfcraft_engine::NoteIcon::Comment };
    let rect = pdfcraft_engine::Shape::Rectangle { rect: [10.0, 10.0, 80.0, 40.0] };
    let box_ = pdfcraft_engine::Shape::TextBox { rect: [20.0, 120.0, 180.0, 170.0], font_size: 10.0 };
    add_shape_on_both(&mut ada, &mut bo, &doc, note, "ada-note", "op-add-note")?;
    add_shape_on_both(&mut ada, &mut bo, &doc, rect, "ada-rect", "op-add-rect")?;
    add_shape_on_both(&mut ada, &mut bo, &doc, box_, "ada-box", "op-add-box")?;

    // Each refusal is answered to its sender only; nothing is broadcast and the
    // version stays where it was.
    let version = ada.version;
    for (id, target, payload, why) in [
        (
            "op-bad-endings",
            annotation("ada-note", 0),
            json!({"action": "annotation.update", "style": {"endings": ["slash", "butt"]}}),
            "a note has no endings",
        ),
        ("op-bad-size", annotation("ada-rect", 0), json!({"action": "annotation.font_size", "size": 14.0}), "a rectangle has no font size"),
        ("op-bad-icon", annotation("ada-rect", 0), json!({"action": "annotation.info", "info": {"icon": "star"}}), "a rectangle has no icon"),
        ("op-bad-subject", annotation("ada-note", 0), json!({"action": "annotation.info", "info": {"subject": ""}}), "an empty subject is refused"),
    ] {
        ada.send(merge_op(doc.clone(), OpId::new(id)?, version, 0, target, payload))?;
        match ada.settle()? {
            ServerMessage::Rejected(rejected) => {
                assert_eq!(rejected.code, ErrorCode::Invalid, "{why}: {}", rejected.message);
                assert!(!rejected.retryable);
            }
            other => return Err(format!("expected a rejection for {id}, got {other:?}").into()),
        }
        assert_eq!(ada.version, version, "a refusal does not move the version");
    }

    // Bo resizes the text; Ada's undo names the size it left (16) and is refused
    // after Bo's second resize: his size stays on both replicas.
    let base = bo.version;
    let applied = apply_on_both(
        &mut bo,
        &mut ada,
        merge_op(doc.clone(), OpId::new("op-size-1")?, base, 0, annotation("ada-box", 0), json!({"action": "annotation.font_size", "size": 16.0})),
    )?;
    let undo = pdfcraft_collab_client_pdf::inverse_after(&applied).ok_or("the font size has no inverse")?;
    let base = bo.version;
    apply_on_both(
        &mut bo,
        &mut ada,
        merge_op(doc.clone(), OpId::new("op-size-2")?, base, 0, annotation("ada-box", 0), json!({"action": "annotation.font_size", "size": 22.0})),
    )?;
    let version = ada.version;
    ada.send(Op { op_id: OpId::new("op-undo-size-late")?, ..undo })?;
    match ada.settle()? {
        ServerMessage::Rejected(rejected) => {
            assert_eq!(rejected.code, ErrorCode::Conflict);
            assert!(!rejected.retryable);
        }
        other => return Err(format!("expected a refused undo, got {other:?}").into()),
    }
    assert_eq!(ada.version, version, "a refused undo does not move the version");
    for peer in [&ada, &bo] {
        assert_eq!(style_of(peer, "ada-box")?.font_size, Some(22.0), "the other user's size is kept");
    }
    Ok(())
}

/// A reply that sets the review status is a named reply with `/State`; it
/// survives the delete/restore copy, and a reply addressed to a comment that is
/// gone is refused before anything is added.
#[test]
fn two_clients_reply_with_a_status_and_a_deleted_parent_refuses_a_reply() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "commenter:bo", "bo")?;

    let doc = DocKey::new("file-1")?;
    let add = merge_op(doc.clone(), OpId::new("op-add")?, 0, 0, page(0, 0), add_note_payload("ada-1", [40.0, 250.0], "please review", "ada"));
    ada.send(add)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }

    let reply = merge_op(
        doc.clone(),
        OpId::new("op-status")?,
        1,
        0,
        annotation("ada-1", 0),
        json!({"action": "annotation.reply", "nm": "bo-status", "author": "bo", "state": "accepted"}),
    );
    bo.send(reply)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        let document = peer.session.get(peer.id).ok_or("open")?;
        let status = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("bo-status")).ok_or("the status reply is missing")?;
        assert_eq!(status.state.as_deref(), Some("Accepted"), "the reply sets /State");
        assert_eq!(status.in_reply_to.as_deref(), Some("ada-1"), "it answers the comment it names");
        assert_eq!(status.contents.as_deref(), Some("Accepted set by bo"));
    }

    // The status survives the server's copy: delete the reply, restore it.
    let delete = merge_op(doc.clone(), OpId::new("op-del-status")?, 2, 0, annotation("bo-status", 0), json!({"action": "annotation.delete"}));
    ada.send(delete)?;
    let mut deleted = None;
    for peer in [&mut ada, &mut bo] {
        match peer.settle()? {
            ServerMessage::Applied(applied) => deleted = Some(applied),
            other => return Err(format!("expected an applied delete, got {other:?}").into()),
        }
    }
    let deleted = deleted.ok_or("no applied delete")?;
    for peer in [&ada, &bo] {
        assert_eq!(peer.names(), vec!["ada-1".to_owned()], "the status reply is gone");
    }
    let restore = pdfcraft_collab_client_pdf::inverse_after(&deleted).ok_or("the delete kept no copy")?;
    bo.send(restore)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        let document = peer.session.get(peer.id).ok_or("open")?;
        let status = document.info.annotations.iter().find(|a| a.name.as_deref() == Some("bo-status")).ok_or("the status reply did not come back")?;
        assert_eq!(status.state.as_deref(), Some("Accepted"), "the restored reply is a status reply");
        assert_eq!(status.in_reply_to.as_deref(), Some("ada-1"));
    }

    // The parent goes (with its replies): a reply addressed to it is refused and
    // nothing is added.
    let delete = merge_op(doc.clone(), OpId::new("op-del-parent")?, 4, 0, annotation("ada-1", 0), json!({"action": "annotation.delete"}));
    ada.send(delete)?;
    for peer in [&mut ada, &mut bo] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    for peer in [&ada, &bo] {
        assert!(peer.names().is_empty(), "the comment and its reply are gone");
    }
    let late = merge_op(
        doc.clone(),
        OpId::new("op-late-reply")?,
        5,
        0,
        annotation("ada-1", 0),
        json!({"action": "annotation.reply", "nm": "bo-late", "text": "too late", "author": "bo"}),
    );
    bo.send(late)?;
    // A rejection answers its sender only; Ada's replica is the proof that
    // nothing was added anywhere.
    match bo.settle()? {
        ServerMessage::Rejected(rejected) => assert_eq!(rejected.code, ErrorCode::Conflict, "{}", rejected.message),
        other => return Err(format!("expected a rejected reply, got {other:?}").into()),
    }
    for peer in [&ada, &bo] {
        assert!(peer.names().is_empty(), "the refused reply added nothing");
    }
    Ok(())
}

/// A selection is presence, not an operation: another replica and a viewer see it, no
/// version or journal entry follows, and the room still accepts edits afterwards.
#[test]
fn two_clients_share_a_selection_without_an_operation() -> Result<(), Box<dyn std::error::Error>> {
    let handle = start()?;
    let addr = handle.local_addr();
    let mut ada = Peer::join(addr, "editor:ada", "ada")?;
    let mut bo = Peer::join(addr, "commenter:bo", "bo")?;
    let mut di = Peer::join(addr, "viewer:di", "di")?;

    let text = Selection::Text { page: PageRef { index: 0, epoch: 0 }, rect: [20, 300, 220, 320] };
    ada.send_presence(Some(text.clone()))?;
    for peer in [&mut bo, &mut di] {
        match peer.next()? {
            ServerMessage::Presence(presence) => {
                assert_eq!(presence.actor_id, ada.actor);
                assert_eq!(presence.selection, Some(text.clone()), "the selection travels as presence");
            }
            other => return Err(format!("expected presence, got {other:?}").into()),
        }
    }

    // A viewer's own selection is presence too; it writes no operation.
    let named = Selection::Annotation { nm: "ada-hl-1".into() };
    di.send_presence(Some(named.clone()))?;
    match ada.next()? {
        ServerMessage::Presence(presence) => {
            assert_eq!(presence.actor_id, di.actor);
            assert_eq!(presence.selection, Some(named), "a viewer's selection is seen by others");
        }
        other => return Err(format!("expected presence, got {other:?}").into()),
    }

    assert_eq!((ada.version, bo.version, di.version), (0, 0, 0), "presence never bumps the version");
    let mut late = Peer::join(addr, "editor:eve", "eve")?;
    assert!(late.resync(0)?.is_empty(), "a selection is not journaled and is not an operation");

    // The room still accepts an operation afterwards: presence left nothing behind.
    let doc = DocKey::new("file-1")?;
    let rotate = structural_op(doc, OpId::new("op-rotate")?, ada.version, ada.epoch, page(0, ada.epoch), rotate_payload(90));
    ada.send(rotate)?;
    for peer in [&mut ada, &mut bo, &mut di] {
        assert!(matches!(peer.settle()?, ServerMessage::Applied(_)));
    }
    assert_eq!(ada.version, 1);
    // A replica that joins after the edit sees the rotation alone: the selections are not
    // in the journal it catches up from.
    let mut last = Peer::join(addr, "viewer:frank", "frank")?;
    assert_eq!(last.resync(0)?.len(), 1, "only the rotation is in the journal");
    Ok(())
}
