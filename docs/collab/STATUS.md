# PdfCraft collaboration — status and handoff

- Branch: `feature/realtime-collaboration` (upstream `storytold/pdfcraft`)
- State: implemented behind the `collab` feature (off by default), not pushed,
  no PR opened
- Audience: the next session (human or agent). Everything here was verified in
  the session that wrote it; anything not verified is marked as such.
- Design: `docs/collab/RFC.md` (protocol, ADRs, trade-offs)
- Task and acceptance criteria: `../../TASK.md` in the cabinet-office checkout

## 1. What is built

| Path | Role |
|---|---|
| `crates/collab-proto` | Wire envelope: `Op`, `Applied`, `Welcome`, `Presence`, `Snapshot`, `CommitRequest`, error codes, size limits |
| `crates/collab-server` | Authoritative `Room`: version, epoch, `opId` window, lease, roles, journal tail, participants/ops limits, `replay`, poisoned-read-only and commit-`conflict` state |
| `crates/collab-hub` | In-memory delivery: clients of a document, broadcast, presence fan-out, lease release on disconnect |
| `crates/collab-pdf` | PDF adapter over `engine`: annotations, form values, page rotate/move/delete, restores from server-side copies of deleted comments/pages; durable `Store` (versioned bases, journal, `CABINET` marker) |
| `crates/collab-annot` | Typed wire form of a shared annotation: `shape` (kind, geometry) + `style` (colour, opacity, width, fill) + text, with the v1 subset and its limits |
| `crates/collab-ws` | Blocking WebSocket transport: `hello` + RS256 JWT, `Origin`, frame limits, per-connection outbox/backpressure, `/metrics` snapshot |
| `crates/collab-client` | Client state machine: pending ops (≤8), version/epoch, gap → resync, retryable rejection → resync then resend, reconnect |
| `crates/collab-client-pdf` | Projection: `Op` → engine `Edit` (apply with `Session::apply_remote`), `Edit` → `Op`, inverse operations for undo |
| `crates/collab-session` | Background worker: socket, reconnect with backoff, heartbeat ping, resend of unconfirmed merges after a snapshot |
| `apps/pdfcraft-collab-server` | Daemon (package `pdfcraft-collab-daemon`, binary `pdfcraft-collab-server`): dev mode from a directory, control plane `POST /v1/sessions`, commit callback to the cabinet, `/health`, `/metrics` |
| `crates/ui-egui/src/collab.rs` | App wiring behind `collab`: start from env, apply events, send edits, participants window, presence with cursors and TTL, commit, inverse undo |
| `crates/engine` | `Session::apply_remote` (collaborative edits without the local snapshot undo) |

The feature is native-only for now. `collab-ws` and `collab-session` compile to
an empty crate on wasm; `collab-client`, `collab-client-pdf` and `collab-annot`
are wasm-safe (`cargo check --target wasm32-unknown-unknown` was run for
`collab-client-pdf` after adding the typed payload).

## 2. Decisions that are easy to undo wrongly

1. **`applied` carries the accepted operation.** `Applied.op` is the intent with
   the server's normalisation (author from the token, generated `/NM`, effective
   target). Every replica applies this description; the raw client intent is not
   rebroadcast. The journal stores the same form. Without this a third client
   cannot know what changed.
2. **The author comes from the JWT.** The adapter overwrites `payload.author`
   with the verified `sub` (ADR-0008 §7). A client-supplied author is ignored.
3. **The client proposes `/NM`.** `annotation.add` / `annotation.reply` may carry
   `nm`; the adapter checks uniqueness and uses
   `Edit::AddAnnotationNamed` / `Edit::ReplyToAnnotationNamed`. If absent, the
   engine generates a name and the adapter puts it back into
   `applied.op.payload.nm`. DeckCraft answered that server-generated ids are
   their own choice and had no objection to A for PDF.
4. **The lease is the liveness of one writer, not a permission.** A second
   editor joins with `lease.ttl_secs = 0`, may comment, and gets
   `rejected{code:"lease", retryable:true}` on a page operation while someone
   else holds the lease; `ping` renews the lease (every 30 s), disconnect
   releases it. Roles decide what a client may do (ADR-0008 §2–3); the lease only
   serialises the exclusive structural stream, so a `lease` rejection is retried,
   never reported as a refusal. Same wording as DeckCraft §7 and CADCraft
   ADR-0008. Before this fix the lease was only checked at join and never on
   write.
5. **Retryable rejection waits for the resync.** The client must not resend the
   operation immediately: the worker captures the pending operation, the server
   answers `resync` with the current base/tail, and the operation is re-addressed
   to the new head and resent with its original `opId` (dedup-safe). Sending it
   before the snapshot produces a second stale rejection.
6. **Operation ids and comment names carry a per-launch nonce.** The counter
   resets on restart, and the server's `opId` window lives for the whole room;
   without the nonce a restarted app could reuse an id the server has already
   seen and lose the edit to de-duplication.
7. **Remote and optimistic edits share one path.** The app does not apply edits
   locally: it sends the operation and applies the broadcast result with
   `apply_remote`. There is no rollback path for a rejected optimistic edit.
   `apply_remote` clears the local snapshot undo stack: replaying a snapshot
   taken before someone else's edit would revert their work.
8. **Undo is an inverse operation.** `inverse_for` is computed from the document
   *before* the edit and sent as a normal operation; a conflict with a later edit
   is reported rather than forced. Deletions and page removal are not reversible.
9. **A dirty room commits when its last participant leaves.** `Room::commit_now`
   has no actor/version check; it is the server-initiated commit of RFC §8.
   `commit_if_dirty` is called by the transport after the last sink is removed.
10. **The control plane is the production path; a directory is dev mode.**
    `POST /v1/sessions` (`--control ADDR`, off by default) creates or resumes a
    room from inline base64 bytes or an `http://` URL after verifying the JWT
    and the etag. `--documents` remains for development: one room per
    `<fileId>.pdf`, no cabinet, and the source file is never written.
11. **An operation is acknowledged only after its journal line is synced.**
    `Adapter::record` runs after the adapter changed the document and before
    `applied` leaves the server; a failure poisons the room (writes get
    `error_code:"internal"`, `/health` says `ok:false`, `/metrics` marks it
    `read_only`) because retrying on top of the changed document would diverge.
    Commit stays allowed: it writes the head as the new base, which is how an
    operator rescues the edit the journal lost.
12. **`replay` bypasses the acceptance checks on purpose.** Membership, role,
    lease and `opId` de-duplication were checked when the operation was
    accepted; replaying them again would drop history (the lease is long gone
    after a restart). The version must be contiguous, the document must match
    and the epoch rules still hold, so a corrupted journal stops startup rather
    than half-applying. `epoch` is persisted in `CURRENT`, otherwise merged
    comments addressed to a post-rotation page would be replayed into the
    wrong epoch.
13. **An annotation travels as a typed shape, not as tool-specific fields.**
    `collab-annot` owns the JSON (`shape.kind` + `style` + `contents`), and both
    the server adapter and the client projection go through it. Adding a tool
    means one variant in that crate, not a new branch in two adapters. Shapes
    that reference data outside the operation (drawn/typed signatures, custom
    stamps, redaction, attachments) are refused with `unsupported`; the server
    never silently turns them into something else, and the client does not apply
    them locally. A payload without `shape` is still read as a sticky note for
    the unwritten v1 clients.
14. **A commit is saved locally first, published second.** `CabinetAdapter`
    writes the base through `PdfAdapter`, then POSTs
    `{fileId, bytes, sha256, version, expectedVersion}` to `--callback-url`. Only
    a `2xx` advances `expectedVersion` and the `CABINET` marker; a `409` refuses
    the commit (the room is flagged `conflict` in `/health`/`/metrics`, edits
    keep being accepted), any other failure leaves the room dirty so the next
    commit or the last-client-leave commit retries the publication.
15. **The cabinet marker is separate from the base marker.** `--data/<id>/CABINET`
    (`{"version","etag"}`) records what the cabinet holds; it may lag behind
    `CURRENT` exactly by the unpublished commits. Keeping it in `CURRENT` would
    lose the difference and a restart would not know the room is dirty.
16. **The etag is `sha256:<hex>` of the file bytes, computed by the daemon.**
    The cabinet sends the etag it believes it has (or the daemon derives it from
    the bytes); `POST /v1/sessions` refuses `400` when the etag does not match
    the bytes and `409` when it does not match the room. In dev mode the etag
    tracks the file in `--documents`, so replacing that file refuses commits;
    the marker's version still advances, because a local commit is all the
    publication dev mode has.
17. **`409` does not stop the session.** The cabinet's copy changing under a
    session refuses commits, not edits: the journal keeps the work, `/health`
    says `conflict:["file-N"]`, and the cabinet decides (a new session over its
    version, or "save a copy"). Blocking writes would lose the work anyway and
    make the editor look broken.

## 3. Running it

Server:

```bash
# production: rooms from the cabinet control plane
cargo run -p pdfcraft-collab-daemon --release -- \
  --data /srv/collab/data \
  --jwt-key /srv/collab/cabinet-public.pem --jwt-audience lzmkm \
  --listen 127.0.0.1:8787 --health 127.0.0.1:8788 \
  --control 127.0.0.1:8789 \
  --callback-url http://cabinet.internal/v1/collab/commit \
  [--max-source-bytes N] [--origin https://cabinet.example] [--app pdfcraft]

# dev: rooms from a directory, no cabinet
cargo run -p pdfcraft-collab-daemon --release -- \
  --documents /srv/collab/documents --data /srv/collab/data [...]
```

- JWT: RS256, claims `{iss, sub, aud, fileId, app, role, sessionId, iat, exp,
  ver}`; `role ∈ owner|editor|commenter|viewer`; `exp` with 30 s leeway;
  `fileId` must equal `hello.doc`; `app` must equal `--app`.
- `POST /v1/sessions` (`--control`, loopback, JWT required):
  `{"fileId","jwt","etag"?,"bytes"|"initialBytes"|"url"}` → `201`/`200` with
  `{"fileId","version","epoch","etag","created"}`; `401` bad token, `403`
  token for another file, `400` bad body/etag, `409` the room holds another
  etag, `404` no room and no source, `413` file too large, `502` the source URL
  failed. `fileId` is restricted to `[A-Za-z0-9._-]{1,128}`.
- Commit callback: `POST --callback-url` with
  `{fileId, bytes(base64), sha256, version, expectedVersion}` and
  `Authorization: Bearer $PDFCRAFT_COLLAB_CALLBACK_TOKEN` when the env var is
  set; `2xx` accepted, `409` conflict, anything else unpublished.
- `/health` → `{"ok":true,"rooms":N,"clients":M,"poisoned":[],"conflict":[]}`;
  `ok` is false when a room's journal failed. `/metrics` adds
  `pdfcraft_collab_room_read_only{room="…"} 1` and
  `pdfcraft_collab_room_conflict{room="…"} 1`.
- `--data/<fileId>/` is the durable state: `CURRENT`, `versions/<N>/file.pdf`,
  `journal.jsonl`, `CABINET`. Delete the directory to reset the room. The flat
  `--data/<fileId>.pdf` working copy of earlier sessions is not migrated:
  nothing was deployed, so only local scratch data is affected.
- TLS: neither the callback nor a source `url` speaks `https`; both are expected
  to go through the reverse proxy on `http://`. No token, comment text or PDF
  byte is logged.

Client (desktop):

```bash
PDFCRAFT_COLLAB_URL=ws://127.0.0.1:8787/ \
PDFCRAFT_COLLAB_TOKEN="$JWT" \
PDFCRAFT_COLLAB_DOC=file-123 \
PDFCRAFT_COLLAB_NAME="Имя" \
cargo run -p pdfcraft --features collab -- file.pdf
```

The app window "Collaboration" shows role, version/epoch, participants, errors,
commit result, and a commit button for owner/editor.

## 4. Tests and verification

Commands that were run:

```bash
cargo test -p pdfcraft-collab-proto -p pdfcraft-collab-server \
  -p pdfcraft-collab-hub -p pdfcraft-collab-client \
  -p pdfcraft-collab-client-pdf -p pdfcraft-collab-session \
  -p pdfcraft-collab-ws -p pdfcraft-collab-pdf -p pdfcraft-collab-annot \
  -p pdfcraft-collab-daemon
cargo clippy -p pdfcraft-collab-daemon -p pdfcraft-collab-pdf \
  -p pdfcraft-collab-server -p pdfcraft-collab-ws \
  -p pdfcraft-collab-client -p pdfcraft-collab-client-pdf -p pdfcraft-collab-annot \
  --all-targets -- -D warnings
cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings
cargo fmt --all --check
cargo xtask layers
cargo check -p pdfcraft --features collab
cargo test -p pdfcraft-ui-egui --features collab
```

- UI suite after §6.5 (§6.5 touched `ui-egui`), run at `8a62b8fb`: 1492 passed,
  2 failed. One is
  the known font glyph failure from above (`editing::a_custom_property_…`, fails
  without the feature too). The other,
  `fill_sign::fill_sign_enter_adds_lines_outside_click_commits_and_escape_cancels`,
  passes when run alone (`cargo test -p pdfcraft-ui-egui --features collab
  --test it fill_sign::…`), so it is a parallel-run flake, not a regression; the
  test touches neither `collab.rs` nor the canvas presence path. The two later
  commits (`a7f769f3` docs, `0db03ed8` the rotation inverse and the address
  rules in `collab-client`/`collab-client-pdf`) touch no `ui-egui` file; their
  crates' suites and `cargo check -p pdfcraft --features collab` are green.
- The UI suite before §6.5 was last run before the control-plane change, which
  touched neither `ui-egui` nor the client crates (`cargo check -p pdfcraft
  --features collab` was green then, as now).

Counts: proto 12, server 16, hub 4, client 12, client-pdf 13, session 2,
ws 6 unit + 5 e2e, pdf 20, annot 10, daemon 27.

- `apps/pdfcraft-collab-server` tests (27) cover the cabinet contract end to
  end over real sockets:
  - `control::tests::a_session_is_created_from_inline_bytes_and_found_again` —
    `POST /v1/sessions` creates a room (`201`), the same call resumes it
    (`200`) with the same etag;
  - `a_changed_copy_is_refused_with_409` and
    `a_live_room_reports_its_etag_and_a_replaced_file_is_refused` — a different
    etag answers `409` with the room's etag and version;
  - `the_token_decides_the_file` — `401` for a bad token, `403` for a token
    issued for another file, `404` with no room and no source;
  - `an_etag_that_does_not_match_the_bytes_is_refused` — `400` (the etag is
    `sha256` of the bytes);
  - `a_source_url_is_fetched_and_a_wrong_content_type_is_not_accepted` — the
    `url` source is fetched over `http`, a failed fetch is `502`, a non-PDF is
    `400`;
  - `a_room_resumed_from_disk_keeps_its_version` — a second process resumes
    from `CURRENT` + journal through the control plane;
  - `rooms::tests::a_commit_posts_the_file_and_the_versions` — the callback
    body is `{fileId, bytes, sha256, version, expectedVersion}`, the bytes
    decode to the committed PDF, and the second commit expects the version the
    cabinet just took;
  - `a_cabinet_that_reports_409_refuses_the_commit` — commit refused with
    `conflict`, room dirty and flagged;
  - `a_changed_source_file_refuses_the_commit` — dev mode: replacing
    `--documents/<id>.pdf` refuses the commit before a base is written;
  - `resuming_with_a_different_etag_is_a_conflict` — `open_room` refuses a
    resume whose seed etag differs;
  - `base64_round_trips_and_refuses_junk`, `a_file_id_cannot_leave_the_data_directory`,
    `http::tests::*` — the encoding, the path-safe `fileId` and the HTTP
    reader/client.
- `crates/collab-server` `a_commit_refused_by_the_adapter_flags_the_room_and_stays_dirty`
  — the `conflict` flag on the room and both commit paths.

- `crates/collab-annot` unit tests cover every supported shape, the shape
  defaults for a partial style, the legacy note payload, unknown/unsupported
  kinds and the size limits.
- `crates/collab-client-pdf` `every_shared_shape_round_trips_between_documents`
  is the engine → operation → engine proof: for each of the 12 shapes it builds
  the operation with `ops_for_edit`, reconstructs the edit on a second document
  and compares shape, style (colour, opacity, width, fill), text; then applies
  it and checks `/Subtype`, `/QuadPoints`, `/C` and `/Contents` in the second
  document. `an_unshared_shape_is_refused_with_a_reason` covers the redaction.
- `crates/collab-pdf` `typed_annotations_apply_and_unshared_ones_are_refused`
  checks the server side: a highlight and a stamp are applied with the client's
  `/NM`, a drawn signature gets `unsupported`.
- `crates/collab-ws/tests/e2e.rs` `two_clients_share_a_typed_highlight` is the
  two-client scenario over a real socket: subtype, quads, colour and text match
  on both replicas.

- `crates/collab-ws/tests/e2e.rs` is the end-to-end proof: three clients over a
  real socket and one PDF; comment + reply converge under the same `/NM`; a
  viewer is refused; a rotation moves the epoch so an old merge is stale; a late
  joiner replays the tail; a commit returns a non-empty file; the last client
  leaving commits the room.
- Live smoke test (not automated): RSA key + RS256 token generated with
  `openssl`, daemon started on loopback, the real
  `target/debug/pdfcraft --features collab` started under `xvfb-run` with the
  `PDFCRAFT_COLLAB_*` variables. `/health` went from `clients:1` to `clients:0`
  when the app closed. This proves connection, JWT and presence wiring in the
  app, not only in the unit/e2e harness.
- Live control-plane smoke test in this session (not automated): RS256 token
  signed with `openssl`, the daemon run with
  `--documents … --control 127.0.0.1:18082 --callback-url http://127.0.0.1:18081/commit`,
  and a one-page PDF in `--documents`. Results: `POST /v1/sessions` answered
  `200` with the file's `sha256` etag for the correct bytes, `400` when the
  etag did not match the bytes and `409` with the room's etag for other bytes;
  a real WebSocket client (`node ws`) joined with the token, added an
  annotation, committed at version 1, and the callback received
  `{"fileId":"file-1","bytes":…,"sha256":"395deb…","version":1,"expectedVersion":0}`;
  after `documents/file-1.pdf` was replaced under the live room, the next
  commit returned `{"type":"error","code":"conflict"}` and `/health` reported
  `"conflict":["file-1"]` with
  `pdfcraft_collab_room_conflict{room="file-1"} 1` in `/metrics`.
- UI suite with the feature: 1493 passed, 1 failed (re-run after the typed
  annotation payload: the same numbers). The failure is
  `editing::a_custom_property_whose_name_the_tools_refuse_is_listed_and_left_alone`:
  `No glyph for "�" (U+FFFD)`. It fails **without** the `collab` feature too and
  is an environment/font issue, not a regression.

## 5. Known limitations (the prod gap)

Ordered by the risk they pose to production.

1. **Crash recovery is not crash-tested against the real process.** The layout
   and both crash windows are reasoned about in RFC §8.1 and the pieces are
   tested (`Store`, `Room::replay`, a restart from the journal, a torn last
   line), but nothing kills the daemon mid-commit; a fault-injection test would
   close that gap.
2. **No web client.** The cabinet embeds the web build, but the server transport
   and the app wiring are native-only. The host bridge
   (`pdfcraft:session`, `session-state`, `presence`, `commit`, `committed`) is
   not implemented.
3. **Annotation coverage beyond the typed set.** `annotation.add` shares
   `TextMarkup` (kind/quads/colour), `Rectangle`, `Oval`, `Line`, `Ink`,
   `FreehandHighlight`, `TextBox`, `Typewriter`, palette `Stamp`, `Mark` and
   `Note` with their style (colour, opacity, width, fill, border). Drawn and
   typed signatures, custom stamps, redaction marks and attachments are refused
   with `unsupported` (RFC §3.1.1). Comment replies that carry a status
   (`/State`) and edits that only change an annotation's style are not shared
   yet.
4. **Structural coverage.** Only page rotate, move and delete. Insert,
   duplicate, replace, split/combine, bookmarks, layers, attachments, form
   structure and content edits are not shared.
5. **Presence has cursors, not selections.** The pointer over a page is sent and
   drawn (ADR-0006); a text selection, `/NM` or rect selection is not sent yet.
6. **Undo of a deletion is best effort.** A comment the room added comes back
   exactly; a comment that came from the file has no wire body, so its delete
   carries no copy and cannot be undone. A page comes back while its one-page
   copy fits `MAX_RESTORE_PAGE_BYTES` (160 KiB); a larger page is deleted without
   a copy. Copies are memory-bounded (64 / 64 MiB) and are rebuilt by replay, not
   persisted on their own.
7. **The cabinet contract is implemented, the production loop is not closed.**
   `POST /v1/sessions`, the commit callback and `409` exist (RFC §8.2). What is
   still missing: an idempotent retry/queue for a failed publication (today the
   next commit or the last-client-leave commit retries it), an endpoint to
   resolve a `conflict` room (the cabinet decides "new session over my version"
   or "save a copy" — the daemon only refuses), and `https` without a proxy.
   The shared cabinet contract also lets the cabinet number versions and return
   its own `{version, etag}` from the callback; v1 ignores that body and uses
   the room's numbering, so a cabinet with its own numbering needs an adapter
   (RFC §8.2 says so explicitly).
8. **No TLS in the daemon.** WSS, the control plane and the callback are
   expected to sit behind a reverse proxy; `https://` source URLs and callback
   URLs are refused with a clear error. No per-IP throttling, no session
   TTL/eviction besides the lease TTL.
9. **Large-document budget.** 500 pages are tested functionally; the 100k-page
   inspection cost and batching remote applies are not measured. The control
   plane caps a file at `--max-source-bytes` (256 MiB) but nothing else.
10. **No automation tool/parity entry yet.** The feature is behind a flag; when
    it becomes user-facing, `crates/automation` and `parity/` need an entry.
11. **Journal growth is bounded only by commits.** A room that never commits
    keeps every accepted operation on disk; the last-client-leave commit and the
    manual button are the only compaction. A size-based forced commit is not
    implemented. Bases are pruned to the one `CURRENT` names, so disk is one
    PDF plus the journal, not one PDF per commit.
12. **The callback is synchronous inside the hub lock.** A cabinet that does not
    answer costs every commit up to the 10 s timeout. Commits are rare (button,
    last client leaving) and edits are not blocked, but this is why a queue is
    on the list above.
13. **The legacy annotation payload lives until the flag ships.** A payload
    without `shape` is read as a sticky note; RFC §16 p. 10 sets the deadline —
    the first release with `collab` on by default makes `shape` mandatory.

## 6. Next session: plan

Work in this order unless the owner says otherwise.

### 6.1 Durable journal — done in this branch

Implemented as designed (RFC §8.1): per-room `data/<fileId>/` with `CURRENT`
(`{"version","epoch"}`), `versions/<N>/file.pdf` and `journal.jsonl`.
`Store` (in `collab-pdf/src/store.rs`) writes a base with fsync + rename and
replaces `CURRENT` atomically; `Adapter::record` appends and syncs the accepted
operation before the room acknowledges it; `Room::replay` rebuilds version,
epoch, the tail and the `opId` window at startup; a failed `record` poisons the
room and `/health`/`/metrics` say so. The trivia to remember: the epoch must be
persisted with the base, or a replayed comment addressed to a rotated page lands
in the wrong epoch; and `truncate` runs after `CURRENT`, so a crash leaves stale
entries that replay skips by version.

Tests: `store::tests` (reopen, truncate, torn last line, corrupt `CURRENT`,
base pruning, replay after a crash on both sides of the `CURRENT` rename), `collab-server`
(recorded-before-ack, poison and commit-still-works, adapter epoch contract,
replay rebuilds head/epoch/dedup, gap/foreign document), `collab-daemon`
(`a_restart_recovers_the_last_commit` now also covers an operation accepted
*after* the commit).

Still open here: a fault-injection test that kills the *real* daemon between
the base write and the `CURRENT` rename (the windows are covered at the `Store`
level), and a size-based forced commit (limitation 11).

### 6.2 Shared annotation shapes — done in this branch

A new leaf crate `crates/collab-annot` (package `pdfcraft-collab-annot`,
layer L4 in `xtask/src/layers.rs`) owns the wire form:

- `ShapeWire` — `kind`-tagged JSON for `note`, `text_markup` (markup kind +
  quads), `rectangle`, `oval`, `line` (ends, arrowheads), `ink`,
  `freehand_highlight`, `text_box`, `typewriter`, `stamp` (palette kind, `by`),
  `mark` (fill-and-sign). `ShapeWire::try_from(&Shape)` refuses the rest with a
  readable label (`a drawn signature`, `a redaction mark`, …).
- `StyleWire` — colour, opacity, width, fill, border colour/shape, fill and text
  opacity; missing fields fall back to `Style::default_for(shape)`, so a partial
  style still draws identically on every replica.
- `AnnotationInput` — shape + style + contents + author; `to_payload(nm)` builds
  `annotation.add`, `from_payload` parses/validates it (legacy `at` payloads are
  read as a sticky note).
- Limits: ≤4096 quads, ≤1024 strokes, ≤4096 points across strokes (fits the
  256 KiB frame), ≤64 KiB text, ≤256 B author; non-finite geometry and opacity
  outside 0–1 are refused.

Both adapters use it: `collab-pdf::PdfAdapter::add_annotation` builds
`NewAnnotation` from `AnnotationInput` and maps `WireError` to
`unsupported`/`invalid`; `collab-client-pdf` `edit_for` does the same on the
client, and `ops_for_edit(Edit::AddAnnotation)` (no more note-only branch) sends
`add_annotation_payload`. UI code needed no change: it already forwards
`Edit::AddAnnotation` through `ops_for_edit`.

Tests: the round-trip and two-client scenarios are listed in §4. What is still
open here: sharing `annotation.set_style` (recolour/width/opacity of an existing
comment), status replies (`/State`), and the same typed envelope for
`AddCustomStamp` if the cabinet ever needs it (it references an XObject, so it
would need the blob channel).

### 6.3 Web host bridge

- Add a transport trait in `collab-client`-adjacent code with two
  implementations: the existing native worker and a wasm `web-sys` WebSocket (or
  the host `postMessage` bridge).
- `pdfcraft:session` (url, token, doc, role), `session-state`, `presence`,
  `commit`, `committed`; keep `open/save/dirty/failed` unchanged.
- The UI wiring currently assumes a native `Handle`; split it from the
  transport so the participants window compiles on wasm.

### 6.4 Cabinet contract and 409 — done in this branch

- `POST /v1/sessions` on `--control ADDR` (own loopback listener, off by
  default): `{fileId, jwt, etag?, bytes|url}` → `201`/`200` with
  `{fileId, version, epoch, etag, created}`. Validation: JWT (same verifier as
  `hello`), token `fileId` = body `fileId`, path-safe `fileId`, etag =
  `sha256` of the bytes, PDF magic, size limit. `409` when the room holds
  another etag (external change or an unpublished commit).
- Commit callback: `--callback-url` + `PDFCRAFT_COLLAB_CALLBACK_TOKEN` posts
  `{fileId, bytes(base64), sha256, version, expectedVersion}`; `2xx` publishes,
  `409` refuses the commit and flags the room, anything else leaves it dirty.
- External change: the `CABINET` marker (`{version, etag}`) is written
  atomically next to `CURRENT`; dev mode hashes `--documents/<id>.pdf` before
  every commit; the control plane checks the etag on `POST /v1/sessions`, and
  the cabinet checks `expectedVersion` on the callback.
- Decision: **the directory stays a dev mode.** Production is the control
  plane; `--documents` is not removed because it is the way to run the server
  without a cabinet.
- Tests: the daemon suite (27) and
  `collab-server::tests::a_commit_refused_by_the_adapter_flags_the_room_and_stays_dirty`.
  A live smoke test is described in §4 (real HTTP control plane, real WebSocket
  commit, callback received, `409` after replacing the source file).
- Still open here: publication retry/queue, an endpoint to resolve a conflict
  room, `https` without a proxy (limitations 7–8, 12).

### 6.5 Presence cursors and undo — done in this branch

Presence:

- `DocView.local_cursor` is filled by `draw_page` from `PagePointer` (the page
  module resolves the pointer to PDF user space with `PageInfo::view_to_user`),
  so the app never reads egui's raw pointer state and the pointer-over-a-dialog
  rules keep working.
- `collab_presence` sends at most 10 frames/s (`PRESENCE_INTERVAL` 100 ms),
  coalesced (only the last state), only when something changed, plus a 1 s
  heartbeat so a reader is not shown as inactive. The server's `presence_hz`
  limit stays as the backstop.
- Remote cursors are drawn by the canvas (`paint_remote_cursors`, arrow + name
  in the actor's colour) on their page only; a presence whose `page.epoch` is
  not the current epoch is not drawn (its index means another page).
- TTL 30 s is client-side (`PRESENCE_TTL`), because presence is never journaled
  or replayed: a participant that goes quiet for 30 s is shown as `inactive` and
  its cursor is dropped; a disconnect or `leave` clears it at once.
- The canvas knows nothing about collaboration: it draws `remote_cursors` and
  reports `local_cursor`, both empty without the feature.

Undo of deletions (RFC §6.1, ADR-0010):

- `PdfAdapter` keeps `known` (the wire body of every comment the room added, by
  `/NM`) and `copies` (bounded: 64 copies / 64 MiB, oldest evicted). A delete
  answers with `copy`, the accepted operation carries it, and the client builds
  the inverse from the accepted operation (`inverse_after`), because a replica
  no longer has the content.
- `annotation.restore`: the copy's body is put back under the same `/NM`
  (`AddAnnotationNamed`); a reply comes back a reply (`ReplyToAnnotationNamed`
  with the parent's `/NM`). The accepted operation carries the body
  (`payload.body`), so every replica and the journal apply the same edit.
- `page.restore`: the delete snapshots the page as a one-page PDF *before* the
  page goes; the restore inserts it with `InsertPagesFrom` at the old index and
  the accepted operation carries the page in `payload.bytes` (base64). Cap:
  `MAX_RESTORE_PAGE_BYTES` = 160 KiB so base64 fits the 256 KiB frame.
- Copies are not on disk: `replay` re-applies the journal in order and rebuilds
  the same tokens (deterministic counter). A comment that came from the file,
  not from the room, has no wire body and is deleted without a copy; its undo is
  refused.
- Undo is compare-and-set (chair's review, point 3): `expect` on
  `annotation.set_contents`, `field.set_value` and `page.rotate`; the adapter
  answers `conflict` when the value changed since, never overwriting someone
  else's edit. Structural inverses are CAS through `baseVersion == head`.
- A rotation's inverse is the *opposite turn*: `RotatePages` adds `degrees` to
  the page's angle, so the old inverse (which sent the previous angle as if it
  were absolute) was a no-op from 0 and a doubled turn otherwise. It now sends
  `(360 - degrees) % 360` and `expect`s the angle the operation left. The e2e
  test undoes a turn and checks both replicas are upright.
- The client re-addresses a merge intent's target to the current epoch when it
  is re-queued after a resync (`with_epoch`); before this a retried comment was
  stale for ever.

Tests added: `collab-pdf` `a_deleted_comment_comes_back_from_the_copy`,
`a_deleted_reply_comes_back_as_a_reply`, `a_deleted_page_comes_back_with_its_content`,
`a_restore_replays_without_its_copy`, `an_expired_or_foreign_copy_is_a_conflict`,
`an_undo_does_not_overwrite_someone_elses_edit`,
`a_turned_page_refuses_an_undo_that_would_overwrite_a_later_turn`;
`collab-client-pdf` `the_accepted_delete_answers_with_the_operation_that_undoes_it`,
`a_restore_adds_the_comment_its_operation_carries`,
`a_page_restore_inserts_the_page_its_operation_carries`,
`a_deletion_is_undone_from_the_server_copy`; `collab-client`
`a_readdressed_merge_intent_takes_the_new_epoch`; `collab-ws` e2e
`two_clients_undo_a_deleted_comment_and_a_deleted_page` (two clients, one
comment delete undone by the other, one page delete undone from the copy, both
replicas equal).

Still open here: drawing remote *selections*, and the blob channel that would
lift the 160 KiB page cap and make a page restore independent of one frame.

### 6.6 Cleanup and submission

- Update `docs/collab/RFC.md` §16 as items close — done for §6.4 and §6.5.
- Add an automation/parity entry once the feature is user-facing.
- Re-run the full gates, open the PR, no CI changes in the feature PR. The PR
  body is not in this tree: the cabinet-office checkout keeps the draft at
  `pdfcraft/PR-DRAFT.md` and the checklist at `PR-CHECKLIST.md`; the body must
  still say what was tested, what was not, and nothing about who wrote it.
- RFC v1 was proposed on the bus (`20261108T115119Z-pdfcraft-9830`). Approvals
  on record: cadcraft (`20261108T124137Z-cadcraft-60aa`), deckcraft
  (`20261108T130802Z-deckcraft-2885`), chair (`20261108T115818Z-craft-05ab`),
  all with non-blocking remarks. Chair's four conditions to close before the PR:
  epoch wording (`RFC §8.1`), lease wording (`RFC §7`), undo CAS (§6.5) — all
  done in this branch; the commit callback still runs under the hub lock
  (limitation 12), which is the one left open. Quorum is 4 of 5 projects
  (`AGENDA.md` §4): wordcraft or gridcraft still has to approve; both were asked
  in `rfc/pdfcraft`. PdfCraft voted on deckcraft's RFC
  (`20261108T135623Z-pdfcraft-2aad`, A with the unified lease wording).

## 7. Gotchas for the next session

- `cargo test -p pdfcraft-ui-egui` needs the full egui/accesskit build and takes
  ~15 minutes on this machine; the UI suite itself is ~12 minutes.
- Building the app with `collab` takes ~20 minutes; `cargo check` is enough for
  wiring changes.
- The `edit` flow is intercepted in `PdfCraftApp::apply_edit` and
  `history_step`; anything that calls `session.apply` directly (OCR, camera,
  some tools) bypasses collaboration and will produce unsupported edits. Gate
  those tools if they become reachable.
- A new annotation shape is a variant in `crates/collab-annot` plus the engine
  type it maps to; do not add a branch in `collab-pdf` and another in
  `collab-client-pdf`. The wire names are asserted in that crate's
  `the_wire_names_are_stable`, so renaming one is a protocol break.
- The frame budget is 256 KiB. `MAX_TOTAL_POINTS` (4096) is chosen so a full ink
  operation still fits; raising it needs the batching/blob channel, not a bigger
  constant alone.
- Mutex poisoning is treated as “hub unavailable” (`TransportError::Hub`); there
  is no panic path in the transport.
- The control plane runs its step under the hub lock (`WsServer::with_hub`).
  Provisioning parses a PDF, so two cabinets creating different rooms wait for
  each other; do not call back into the server from inside that closure (it
  would deadlock).
- The daemon's HTTP client is plain `http://` only and refuses redirects,
  chunked responses and `https`. If the cabinet endpoint needs any of those,
  fix the daemon, do not "just use curl".
- `CABINET` is the source of truth for "what the cabinet has"; if it is
  corrupt, `Store::open` fails rather than guessing, exactly like `CURRENT`.
- The dev-mode mirror check hashes `--documents/<fileId>.pdf` on every commit.
  On a very large file this is a visible pause; the control plane path hashes
  only the bytes it was handed.
- Never use `pkill -f` with a pattern that appears in your own command line on
  this machine (it kills the shell); use `pgrep -x`/explicit PIDs.
- `pgrep -af pdfcraft-collab-server` also matches your own shell command line;
  check the PID before killing.
- The smoke-test keys and JWT live under `/tmp` and are disposable; never commit
  a token or a private key.
- The copy store (`PdfAdapter::copies`) is memory-only on purpose: `replay`
  re-applies the journal in order and rebuilds the same tokens, so nothing is
  written next to `CURRENT`. Changing the order in which copies are made (or
  resetting `next_copy` elsewhere) breaks that; the tokens are the keys the
  client's undo still holds.
- A page restore puts base64 page bytes inside the operation, which is why it is
  capped at `MAX_RESTORE_PAGE_BYTES` (160 KiB). Do not raise the constant: a
  larger page needs the blob channel (the RFC §16 gap), not a bigger frame.
- `inverse_for` returns `None` for the two deletes on purpose; the UI patches the
  inverse from `applied` with `inverse_after`. An inverse for a delete built on
  the client would need the content the replica no longer has.
- Presence TTL is a client-side clock (`PRESENCE_TTL` in `ui-egui/src/collab.rs`):
  the server stores only the last state per participant and does not evict it, so
  "inactive" is drawn where presence is read. A merge intent re-addressed after a
  resync takes the new target epoch (`collab-client::with_epoch`); without it a
  retried comment was stale for ever.
