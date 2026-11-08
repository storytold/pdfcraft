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
| `crates/collab-server` | Authoritative `Room`: version, epoch, `opId` window, lease, roles, journal tail, participants/ops limits |
| `crates/collab-hub` | In-memory delivery: clients of a document, broadcast, presence fan-out, lease release on disconnect |
| `crates/collab-pdf` | PDF adapter over `engine`: annotations, form values, page rotate/move/delete; atomic commit to a path |
| `crates/collab-ws` | Blocking WebSocket transport: `hello` + RS256 JWT, `Origin`, frame limits, per-connection outbox/backpressure, `/metrics` snapshot |
| `crates/collab-client` | Client state machine: pending ops (≤8), version/epoch, gap → resync, retryable rejection → resync then resend, reconnect |
| `crates/collab-client-pdf` | Projection: `Op` → engine `Edit` (apply with `Session::apply_remote`), `Edit` → `Op`, inverse operations for undo |
| `crates/collab-session` | Background worker: socket, reconnect with backoff, heartbeat ping, resend of unconfirmed merges after a snapshot |
| `apps/pdfcraft-collab-server` | Daemon (package `pdfcraft-collab-daemon`, binary `pdfcraft-collab-server`): rooms from a directory, JWT verifier, commits, `/health`, `/metrics` |
| `crates/ui-egui/src/collab.rs` | App wiring behind `collab`: start from env, apply events, send edits, participants window, presence, commit, inverse undo |
| `crates/engine` | `Session::apply_remote` (collaborative edits without the local snapshot undo) |

The feature is native-only for now. `collab-ws` and `collab-session` compile to
an empty crate on wasm; `collab-client` and `collab-client-pdf` are wasm-safe.

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
    yet: `<fileId>.pdf` in `--documents` becomes a room, and
    `--data/<fileId>.pdf` is the working copy that commits replace atomically.
    Restart recovery starts from that working copy (tested).

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
- `/health` → `{"ok":true,"rooms":N,"clients":M}`; `/metrics` → Prometheus text.
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
  -p pdfcraft-collab-ws -p pdfcraft-collab-pdf -p pdfcraft-collab-daemon
cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings
cargo fmt --all --check
cargo xtask layers
cargo check -p pdfcraft --features collab
cargo test -p pdfcraft-ui-egui --features collab
```

Counts: proto 12, server 10, hub 4, client 11, client-pdf 7, session 2,
ws 6 unit + 3 e2e, pdf 5 (including a 500-page document), daemon 5.

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

1. **The journal does not survive a restart.** The working copy is written on
   commit, and a restart recovers that copy (tested), but operations accepted
   after the last commit are lost if the process dies. `Room` keeps its journal
   in memory only.
2. **No web client.** The cabinet embeds the web build, but the server transport
   and the app wiring are native-only. The host bridge
   (`pdfcraft:session`, `session-state`, `presence`, `commit`, `committed`) is
   not implemented.
3. **Annotation coverage.** Only sticky notes (`Shape::Note`) are shared through
   `annotation.add`. Highlights, underlines, strikeouts, shapes, lines, ink,
   stamps, text boxes and typewriter edits answer “not shared yet” and are not
   applied locally in a collaborative session.
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

## 6. Next session: plan

Work in this order unless the owner says otherwise.

### 6.1 Durable journal (highest priority)

Goal: a crash loses no acknowledged operation; recovery is tested.

Design that avoids the “commit then crash before truncate” double-apply:

- Layout `data/<fileId>/`:
  - `versions/<N>/file.pdf` — committed bases, one per commit;
  - `CURRENT` — the version of the base to load, replaced atomically
    (`CURRENT.tmp` + `rename`);
  - `journal.jsonl` — accepted operations appended as
    `{"version":N,"actor":"…","op":{…}}`, flushed before the op is acknowledged.
- Commit: write `versions/<N+1>/file.pdf`, fsync, atomically replace `CURRENT`,
  then drop journal entries with `version <= N+1` (rewrite/truncate).
- Startup: read `CURRENT`, open that PDF, replay journal entries with
  `version > CURRENT` through a new `Room::replay(actor, op)` that bypasses
  membership/role/lease/dedup but rebuilds version, epoch, journal and the
  `applied` map.
- Crash windows: before `CURRENT` → old base plus full journal; after `CURRENT`
  → new base plus stale entries that are skipped by version. Neither replays an
  applied operation.
- A journal write failure after the adapter changed the document must poison the
  room (reject further operations, report on `/health`) instead of silently
  diverging; document that the last operation may be lost on restart.
- Tests: append/replay; crash before `CURRENT`; crash after `CURRENT`; duplicate
  `opId` after restart; `xtask layers` and clippy.

Files: `crates/collab-pdf` (journal writer/commit path), `crates/collab-server`
(`replay`), `apps/pdfcraft-collab-server` (layout/startup), new tests.

### 6.2 Shared annotation shapes

- Add a typed shape/style payload (a small `collab-annot` crate depending on
  `pdfcraft-annot`, used by both the adapter and the projection) or an equivalent
  serde representation. Support at least `TextMarkup` (kind, quads, colour),
  `Rectangle`, `Oval`, `Line`, `Ink`, `FreehandHighlight`, `TextBox`,
  `Typewriter`, palette `Stamp`, `Mark`.
- Keep blobs, signatures, custom stamps, redaction and attachments out of v1.
- Round-trip test: engine document → operation → another engine document; compare
  subtype, rect/quads, colour, opacity and text.
- Replace the “only sticky notes” error in `ops_for_edit` and the note-only arm
  in `collab-pdf`.

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
- Mutex poisoning is treated as “hub unavailable” (`TransportError::Hub`); there
  is no panic path in the transport.
- Never use `pkill -f` with a pattern that appears in your own command line on
  this machine (it kills the shell); use `pgrep -x`/explicit PIDs.
- The smoke-test keys and JWT live under `/tmp` and are disposable; never commit
  a token or a private key.
