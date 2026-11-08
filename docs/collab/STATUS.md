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
| `crates/collab-server` | Authoritative `Room`: version, epoch, `opId` window, lease, roles, journal tail, participants/ops limits, `replay`, poisoned-read-only state |
| `crates/collab-hub` | In-memory delivery: clients of a document, broadcast, presence fan-out, lease release on disconnect |
| `crates/collab-pdf` | PDF adapter over `engine`: annotations, form values, page rotate/move/delete; durable `Store` (versioned bases + journal) |
| `crates/collab-annot` | Typed wire form of a shared annotation: `shape` (kind, geometry) + `style` (colour, opacity, width, fill) + text, with the v1 subset and its limits |
| `crates/collab-ws` | Blocking WebSocket transport: `hello` + RS256 JWT, `Origin`, frame limits, per-connection outbox/backpressure, `/metrics` snapshot |
| `crates/collab-client` | Client state machine: pending ops (≤8), version/epoch, gap → resync, retryable rejection → resync then resend, reconnect |
| `crates/collab-client-pdf` | Projection: `Op` → engine `Edit` (apply with `Session::apply_remote`), `Edit` → `Op`, inverse operations for undo |
| `crates/collab-session` | Background worker: socket, reconnect with backoff, heartbeat ping, resend of unconfirmed merges after a snapshot |
| `apps/pdfcraft-collab-server` | Daemon (package `pdfcraft-collab-daemon`, binary `pdfcraft-collab-server`): rooms from a directory, startup replay, JWT verifier, commits, `/health`, `/metrics` |
| `crates/ui-egui/src/collab.rs` | App wiring behind `collab`: start from env, apply events, send edits, participants window, presence, commit, inverse undo |
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
4. **Lease gates structural writes, not joining.** A second editor joins with
   `lease.ttl_secs = 0`, may comment, and gets `rejected{code:"lease",
   retryable:true}` on a page operation while someone else holds the lease.
   `ping` renews the lease; disconnect releases it. Before this fix the lease was
   only checked at join and never on write.
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
10. **The daemon provisions rooms from a directory.** There is no control plane
    yet: `<fileId>.pdf` in `--documents` becomes a room, and `--data/<fileId>/`
    holds its durable state: `CURRENT` (base version and epoch),
    `versions/<N>/file.pdf` and `journal.jsonl`.
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

## 3. Running it

Server:

```bash
cargo run -p pdfcraft-collab-daemon --release -- \
  --documents /srv/collab/documents --data /srv/collab/data \
  --jwt-key /srv/collab/cabinet-public.pem --jwt-audience lzmkm \
  --listen 127.0.0.1:8787 --health 127.0.0.1:8788 \
  [--origin https://cabinet.example] [--app pdfcraft]
```

- JWT: RS256, claims `{iss, sub, aud, fileId, app, role, sessionId, iat, exp,
  ver}`; `role ∈ owner|editor|commenter|viewer`; `exp` with 30 s leeway;
  `fileId` must equal `hello.doc`; `app` must equal `--app`.
- `/health` → `{"ok":true,"rooms":N,"clients":M,"poisoned":[]}`; `ok` is false
  when a room's journal failed and it refuses writes. `/metrics` adds
  `pdfcraft_collab_room_read_only{room="…"} 1` for those rooms.
- `--data/<fileId>/` is the durable state: `CURRENT`,
  `versions/<N>/file.pdf`, `journal.jsonl`. Delete the directory to reset the
  room to the copy in `--documents`. The flat `--data/<fileId>.pdf` working copy
  of earlier sessions is not migrated: nothing was deployed, so only local
  scratch data is affected.
- No token, comment text or PDF byte is logged.

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
cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings
cargo fmt --all --check
cargo xtask layers
cargo check -p pdfcraft --features collab
cargo test -p pdfcraft-ui-egui --features collab
```

Counts: proto 12, server 15, hub 4, client 11, client-pdf 9, session 2,
ws 6 unit + 4 e2e, pdf 13, annot 10, daemon 5.

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
- UI suite with the feature: 1493 passed, 1 failed. The failure is
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
5. **Presence is a list.** Page and zoom are sent; other participants' cursors
   and selections are not drawn.
6. **Undo is partial.** Comments, text/field values, rotation and move are
   reversible; deleting a comment or a page is not. Redo is basic.
7. **No cabinet control plane.** Rooms are provisioned from a directory; there is
   no `POST /v1/sessions`, no commit callback, no check of an external file
   change (409) and no `base_etag`/file watch.
8. **No TLS in the daemon.** WSS is expected from a reverse proxy; no per-IP
   throttling, no session TTL/eviction besides the lease TTL.
9. **Large-document budget.** 500 pages are tested functionally; the 100k-page
   inspection cost and batching remote applies are not measured.
10. **No automation tool/parity entry yet.** The feature is behind a flag; when
    it becomes user-facing, `crates/automation` and `parity/` need an entry.
11. **Journal growth is bounded only by commits.** A room that never commits
    keeps every accepted operation on disk; the last-client-leave commit and the
    manual button are the only compaction. A size-based forced commit is not
    implemented. Bases are pruned to the one `CURRENT` names, so disk is one
    PDF plus the journal, not one PDF per commit.

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

### 6.4 Cabinet contract and 409

- `POST /v1/sessions` (file id, bytes or URL, JWT) → room; commit callback to the
  cabinet with `{fileId, bytes, sha256, version, expectedVersion}`; `409` when the
  file changed outside the session (`base_etag`/file watch).
- Decide whether the daemon keeps directory provisioning as a dev mode.

### 6.5 Presence cursors and undo

- Send the cursor/selection from the page pointer (≤10 Hz); draw remote cursors
  with the stored colour. The protocol fields already exist.
- Keep server-side copies of deleted annotations/pages for inverse operations
  (ADR-0010) so delete can be undone.

### 6.6 Cleanup and submission

- Update `docs/collab/RFC.md` §16 as items close.
- Add an automation/parity entry once the feature is user-facing.
- Re-run the full gates, open the PR (see the draft body in
  `../PR-DRAFT.md`), no CI changes in the feature PR.

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
- Never use `pkill -f` with a pattern that appears in your own command line on
  this machine (it kills the shell); use `pgrep -x`/explicit PIDs.
- The smoke-test keys and JWT live under `/tmp` and are disposable; never commit
  a token or a private key.
