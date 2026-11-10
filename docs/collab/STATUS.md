# PdfCraft collaboration — status and handoff

- Branch: `feature/realtime-collaboration` (upstream `storytold/pdfcraft`)
- Base: `origin/main` `ba28458ad` ("M3: a password prompt keeps a place in the
  open queue for its retry (#6731)", 2026-11-09), the tip the branch was rebased
  onto. All **76 branch commits** were
  replayed with **four conflict stops in three files**
  (`crates/ui-egui/src/app.rs` twice, `crates/ui-egui/src/canvas.rs`,
  `crates/engine/src/edit.rs`), each resolved by keeping both sides; §4 records
  them. `origin/main` has moved since, faster than any pass followed it: at the
  presence-selection pass its tip was `a71b5124b` (11 behind), and at this
  pass's start it was `274b7a042` ("Merge pull request #6800 from
  storytold/merge/fonts-budget-6239"), so the branch is now **48 behind**
  `274b7a042`. `git merge-tree --write-tree origin/main HEAD` exited 0 at
  `0c57e2271`; it was not re-run at this pass's head, and no rebase was done in
  this pass, so the base stays `ba28458ad`.
- Local head before this STATUS commit: `5bad54c40` (feat: share several
  selections, per-line quads and the caret) on `38d0bf56d` (docs(collab):
  record the pop-up window operation and reconcile the STATUS head) on
  `0c57e2271` (feat: open, close and move
  a comment's pop-up window as one operation) on `199076765` (docs(collab):
  record the parts, move and geometry operations) on `22d29d070` (feat: share a
  comment's parts, a move and its geometry) on `be5ac3570` (docs(collab):
  reconcile STATUS with PR #6622 and the cabinet HS256 image) on `3f3bbedb0`
  (docs: record the cabinet HS256 auth pass and its gates) on `157f4d239` (feat:
  accept the cabinet's HS256 session tickets next to RS256) on `b27fa89c8`
  (docs: record the split part and the ordered Combine confirmation) on
  `843a8f533` (feat: keep a split's part and join Combine as ordered parts) on
  `a007c4256` (docs: record the rebase onto `ba28458ad` and the gate run) on
  `5f3c6f3c2` (style: wrap the engine export list after the rebase merge) on
  `2020fce3f` (docs: record the Split/Combine dialogs inside a session) on
  `d975ebb72` (feat: send Split and Combine from a session as page operations)
  on `8f71c37db` (feat: split/combine pages as server-ordered operations) on
  `32e94bc78` (docs: the previous rebase record) on `ebce0fb6b` (refactor: the
  `apply_edit` dispatcher, which the merged `ui-egui` needed to stay under the
  clippy complexity limit) — **88 commits** not in `origin/main`
  (`git rev-list --count origin/main..HEAD` at `5bad54c40`), **48 behind** and
  **8 ahead** of the PR head; this STATUS commit makes 89. The numbers the
  earlier briefs named — 85 commits, 9 behind, 5 ahead for `199076765`, then
  86/11/6 at `0c57e2271` — were right when they were measured; `origin/main`
  kept moving (`ba28458ad` → `a71b5124b` → `274b7a042`) and each slice added
  its commits. Commits named below and
  in §4 by their pre-rebase hashes are mapped in the rebase section of §4.
- PR #6622 has the fork's head `b27fa89c8` (`copoka/pdfcraft`,
  `feature/realtime-collaboration`, 80 commits) and reports
  `mergeable: MERGEABLE` / `mergeStateStatus: UNSTABLE` — `updatedAt`
  2026-11-09 23:25:38Z, observed 2026-11-10. So the PR describes the branch
  through the Split/Combine record; the eight local commits above it are in it
  only after the owner's force-push: the cabinet's HS256 tickets (`157f4d239`)
  and their record (`3f3bbedb0`), the parts/move/geometry operation
  (`22d29d070`) and its record (`199076765`), the pop-up operation
  (`0c57e2271`) and its record (`38d0bf56d`), and this pass's selection
  operation (`5bad54c40`) and its record (this STATUS commit). `UNSTABLE` is
  the CI rollup, not a
  conflict: `git merge-tree --write-tree origin/main HEAD` exits 0. Nothing was
  pushed from these sessions: only the owner's force-push moves the PR, and
  mergeability follows it. The PR body still describes an older push; it is not
  re-edited in this pass (nothing is pushed).
- State: implemented behind the `collab` feature (off by default). The gates in
  §4 are green at the local head `5bad54c40` for the crates this pass touched
  (proto, client, ws, server, hub, `ui-egui` with `collab`, and the wasm shell);
  the whole-suite gates named earlier were run at their own heads (`157f4d239`,
  `22d29d070`, `0c57e2271`, §4) and were not re-run here.
- What is done locally, all recorded in §4 and none pushed: the web host bridge
  (`81a92f32f`), the forced journal commit (`8fc898bae`), the idle/time
  threshold (`2a3b6e43e`), crash recovery against the real process
  (`999da5ec`), the commit publication outside the hub lock (`a47641fb`),
  restyle and status replies (`ce5c38798`), the publication queue
  (`a639f30c0`), presence selections (`b4e9179d8`) and their second slice —
several `/NM`, per-line quads, the caret (`5bad54c40`, §4) — the conflict-resolution
  endpoint (`62d48d30a`), line endings/font size/info (`c400147b3`), page
  replace/blank insert (`e9756d138`), page insert/duplicate (`b13933e9b`),
  page split/combine (`362f9c72b`), the Split/Combine dialogs sending them
  inside a session (this session, §4), and the split part kept as a document
  plus one Combine confirmation as ordered parts (`843a8f533`, §4), and the
  cabinet's HS256 session tickets next to RS256 (`157f4d239`, §4), and a
  comment's parts, a move, a resize and a reshape as addressable operations
  (`22d29d070`, §4), and its pop-up window as one too (`0c57e2271`, §4). The
  whole
  branch is rebased onto `origin/main` `ba28458ad` with an empty `merge-tree`
  (§4, first table). The one presence item still outside this repository is the
  host page's `pdfcraft:presence` payload: `craft-web-host` must gain optional
  `page`/`selection` on its `Participant` (§5 p.4, §4).
- Known gaps before production are listed in §5, the plan in §6. The one this
  pass leaves explicitly open: a kill between the base write and the `CURRENT`
  rename is covered at the `Store` level only (§5 p.1, §6.1); the real-process
  SIGKILL test covers the journal→base window.
- Audience: the next session (human or agent). Everything here was verified in
  the session that wrote it; anything not verified is marked as such.
- Design: `docs/collab/RFC.md` (protocol, ADRs, trade-offs); operation of the
  daemon: `docs/collab/server.md`
- Task and acceptance criteria: `../../TASK.md` in the cabinet-office checkout

## 1. What is built

| Path | Role |
|---|---|
| `crates/collab-proto` | Wire envelope: `Op`, `Applied`, `Welcome`, `Presence`, `Snapshot`, `CommitRequest`, error codes, size limits |
| `crates/collab-server` | Authoritative `Room`: version, epoch, `opId` window, lease, roles, journal tail, participants/ops limits, `replay`, poisoned-read-only and commit-`conflict` state, the journal operation/byte threshold for a forced commit (`forced_commit_due`), the operator's resolution of a `conflict` room (`resolve_conflict`) |
| `crates/collab-hub` | In-memory delivery: clients of a document, broadcast, presence fan-out, lease release on disconnect, and the server-initiated commits (last leave, forced compaction) whose publication runs outside the lock |
| `crates/collab-pdf` | PDF adapter over `engine`: annotations (add, edit, restyle with `/LE`, font size, author/subject/icon, parts, move, geometry, reply with or without `/State`, delete, restore), form values, page rotate/move/delete/insert/duplicate/replace/blank insert, restores from server-side copies of deleted comments/pages and the copy a replacement keeps for its undo; durable `Store` (versioned bases, journal, `CABINET` marker carrying the room's version and the cabinet's) |
| `crates/collab-annot` | Typed wire form of a shared annotation: `shape` (kind, geometry) + `style` (colour, opacity, width, endings, fill) + text, with the v1 subset and its limits; the final style of an existing comment (`StyleUpdate` for `annotation.update`), its font size (`FontSizeUpdate`), its author/subject/icon (`InfoUpdate`), its parts (`PartsUpdate`), its move (`MoveUpdate`) and its geometry (`GeometryUpdate`) and their before/expect CAS; the review-state names of a status reply |
| `crates/collab-ws` | Blocking WebSocket transport: `hello` + RS256 JWT, `Origin`, frame limits, per-connection outbox/backpressure, `/metrics` snapshot, the control plane's `resolve_conflict` and its counter |
| `crates/collab-client` | Client state machine: pending ops (≤8), version/epoch, gap → resync, retryable rejection → resync then resend, reconnect |
| `crates/collab-client-pdf` | Projection: `Op` → engine `Edit` (apply with `Session::apply_remote`), `Edit` → `Op`, inverse operations for undo |
| `crates/collab-session` | One protocol driver (`driver::Session`: hello/welcome, version and epoch, resync and retry, lease heartbeat, presence) behind a `Transport` trait with two implementations: `native` (a blocking socket on a worker thread, reconnect with backoff) and `web` (the browser's `WebSocket`, whose callbacks queue frames and which the frame loop steps). `Sink` is the object-safe face the UI holds |
| `apps/pdfcraft-collab-server` | Daemon (package `pdfcraft-collab-daemon`, binary `pdfcraft-collab-server`): dev mode from a directory, control plane `POST /v1/sessions` and `POST /v1/sessions/{fileId}/resolve`, commit callback to the cabinet with an idempotency key (its `{version, etag}` answer is adopted), the background publication queue with a backoff, `/health`, `/metrics` (`pdfcraft_collab_forced_commits_total`, `pdfcraft_collab_publication_retries_total`, `pdfcraft_collab_conflicts_resolved_total`) |
| `crates/ui-egui/src/collab.rs` | App wiring behind `collab`, native and wasm: start from the environment (desktop) or from the host page, apply events, send edits, participants window, presence with cursors, selections and TTL, commit, inverse undo, host reports, and the Split/Combine dialogs' `page.split`/`page.combine` inside a session. Holds `Box<dyn Sink>`, never a native `Handle` |
| `apps/pdfcraft-web/src/main.rs` | Web shell: the host bridge for collaboration (`pdfcraft:session` in; `session-state`, `presence`, `commit` out; `committed` back) over `craft-web-host` |
| `crates/engine` | `Session::apply_remote` (collaborative edits without the local snapshot undo) |

Both platforms run the same protocol: `collab-session` compiles on wasm32 with a
`web-sys` `WebSocket` transport, and `ui-egui --features collab` compiles for
wasm32 (§4, §6.3). `collab-ws` stays native-only (it is the server's transport);
`collab-client`, `collab-client-pdf` and `collab-annot` are wasm-safe. The web
build enables the feature with `trunk build --release --features collab`.

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
9. **A dirty room commits when its last participant leaves — and compacts
   itself while they stay.** `Room::commit_now` has no actor/version check; it
   is the server-initiated commit of RFC §8. `commit_if_dirty` is called by the
   transport after the last sink is removed. A room that never empties used to
   keep every operation on disk; now `Room::forced_commit_due` (operations or
   journal bytes since the last base, `RoomConfig::forced_commit_ops` /
   `forced_commit_bytes`) makes the transport call `Hub::forced_commit` after
   each client message and on a join. The same `commit_now` writes the base
   under the hub lock; the publication still runs outside it. Do not turn this
   into a timer without deciding what "idle" means: the threshold is state, so
   it survives a reconnect and a restart (replayed entries count).
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
14. **A commit is saved locally first, published second — and a retryable
    failure is queued.** `CabinetAdapter` writes the base through `PdfAdapter`,
    then POSTs
    `{fileId, bytes, sha256, version, expectedVersion, idempotencyKey}` to
    `--callback-url`. Only a `2xx` advances the `CABINET` marker; its body
    `{version, etag}` (the shared contract) is the copy the room adopts and the
    next `expectedVersion`, and an empty body falls back to
    `expectedVersion + 1`. A `409` refuses the commit (the room is flagged
    `conflict` in `/health`/`/metrics`, edits keep being accepted) and stops the
    queue. A network error, `429` or `5xx` is retryable: the room keeps
    `publishing`, and the transport's queue re-runs the same publication after
    a doubling backoff, with the same version and the same key
    (`fileId:version:sha256`), so a lost answer becomes a duplicate to the
    cabinet, not a second version (§4). A non-retryable `4xx` leaves the room
    dirty so the next commit retries it, as before.
15. **The cabinet marker is separate from the base marker.** `--data/<id>/CABINET`
    (`{"version","etag","expected"}`, where `expected` is optional and defaults
    to `version`) records what the cabinet holds; it may lag behind
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
    make the editor look broken. The first option is the operator's
    `POST /v1/sessions/{fileId}/resolve` (decision 19).
18. **A commit's cabinet call runs outside the hub lock.** `Room::commit` writes
    the base and returns a `Commit`; the transport runs its publication after
    releasing the lock and reports back with `Room::finish`. A second commit of
    the same document while one is publishing gets `busy` (retryable), so two
    versions cannot reach the cabinet out of order; a retryable failure keeps
    the room `publishing` for the same reason, and the queue's retry is what
    releases it (`Room::finish_retry`). Moving the call back inside `with_hub`
    makes one slow cabinet freeze every document in the process —
    `a_commit_does_not_block_presence_in_another_room` is the guard.
19. **A conflict is resolved by naming the cabinet's copy, once.** `409` stops
    the queue because retrying the same `expectedVersion` cannot succeed, so
    the way out is explicit: `POST /v1/sessions/{fileId}/resolve` with
    `{expectedVersion, etag}` and a service token (the same RS256 JWT,
    `owner`/`editor`). `Room::resolve_conflict` checks that the room *is* in
    conflict and is not publishing (a publication in flight was addressed to
    the old marker), that the version is newer than the one the session knows
    (a repeat would fail the same way), then writes the marker and clears the
    flag. It does not publish by itself and does not move `version`: the work
    the cabinet never took is still in the journal, the room stays dirty, and
    the next commit (button, last leave, forced compaction) publishes it with
    the adopted `expectedVersion`. The marker therefore needs two numbers —
    `version` (the room's, for the dirty check after a restart) and `expected`
    (the cabinet's, for the next publication) — kept in `CABINET`; they are
    equal until a resolution. The `{version, etag}` answer to that next
    publication moves `expected` on, so a resolution lasts for the session
    rather than one commit. A second resolve is `409 not_conflict`; a wrong
    version `400`; another file's token, a viewer's role or no room is
    refused; none of them changes the room. `/metrics` gains
    `pdfcraft_collab_conflicts_resolved_total`.
20. **A comment property is its own addressable operation, not a hidden part of
    a restyle.** Line endings stay inside `annotation.update` because they are
    part of the look the comment bar sets in one choice; font size
    (`annotation.font_size`) and author/subject/icon (`annotation.info`) are
    separate actions because the engine edits are separate (`Edit::FontSizeAnnotation`,
    `Edit::SetAnnotationInfo`) and each can fail on its own. Making them one big
    edit would need a new engine edit that duplicates three validations; making
    them fields of the restyle would make a font-size change and a colour change
    one undo step even when the interface did not ask for that. A Comment
    Properties batch is several operations in one history group, so it still
    undoes as one step.
21. **The wire's `null` removes a name; an empty string is a mistake.** A
    comment may have no `/T` or `/Subj`, and the undo of setting one must put
    "no author" back, which an empty string cannot express without changing the
    file. So `null` removes the entry (the engine deletes the key), an empty
    string is `invalid`, and `Props` reads "no author" as `""` for the
    compare-and-set. Without this the undo of a name on a file-originated
    comment would silently leave the new name in place.
22. **A replacement keeps what it replaced, and no copy means a refused undo —
    not a lost page.** `page.replace` is the only structural operation whose
    payload *has* to be checked as a whole before the edit (base64, one page,
    the size budget), because the page it carries is the document's new
    content. The old page is copied first, and the accepted operation carries
    the copy token next to `bytes`: a replica and the journal re-apply `bytes`,
    while the undo names the token. A page over the budget is still
    replaceable; it just has no copy, and the undo (`"copy": null`) is a
    `conflict` from the room rather than a silent "cannot undo" on one machine,
    so every client tells the same story and nothing is changed by the refusal.
    Do not "fix" this into a client-side copy made from the replica: a replica
    can be behind the head in a way the room's serialisation hides, and the
    copy the server made is the page the room actually had.
23. **A room's Split keeps the head and takes the tail out; the dialog says so
    before it is confirmed.** One `page.split` carries one contiguous part, so
    the dialog's first cut is the only shape a room can take: the pages before
    it keep their indices (comments, selections and presence on them do not
    move) and the rest leaves as one part, whose undo is the `page.combine` of
    the accepted operation. The dialog's text changes in a session — it names
    the cut and the pages that leave, instead of promising N files — because a
    silent "Split" that removes pages and writes nothing would be a surprise.
    A plan with one part has no cut and is refused with a message. The part is
    not written to a file from the session, but it is not lost either: the
    accepted operation carries it, and the app opens it as a document of its
    own in the background (RFC §3.1.5 leaves the file to the client). Do not
    make the dialog take the first part out to match "the first file": that
    renumbers every remaining page and moves everybody's comments.
24. **A dialog that changes the document asks the session first.** Split and
    Combine call `collab_shares`/`collab_document` and route through the same
    session methods as edits; the file workflow is the fallback, never a
    parallel path — otherwise a user in a session would get files the shared
    document never sees, and the room would not know about the change. Combine
    focuses the session's document before it closes the Combine tab, because
    the applied broadcast is delivered to the active view.

## 3. Running it

Server:

```bash
# production: rooms from the cabinet control plane
cargo run -p pdfcraft-collab-daemon --release -- \
  --data /srv/collab/data \
  --jwt-secret-file /srv/collab/cabinet-jwt-secret --jwt-audience lzmkm \
  --listen 127.0.0.1:8787 --health 127.0.0.1:8788 \
  --control 127.0.0.1:8789 \
  --callback-url http://cabinet.internal/v1/collab/commit \
  [--max-source-bytes N] [--origin https://cabinet.example] [--app pdfcraft]

# dev: rooms from a directory, no cabinet
cargo run -p pdfcraft-collab-daemon --release -- \
  --documents /srv/collab/documents --data /srv/collab/data [...]
```

- JWT: HS256 with the cabinet's secret (`--jwt-secret-file FILE` or
  `PDFCRAFT_COLLAB_JWT_SECRET`, at least 16 bytes) — the scheme the cabinet
  signs with today; RS256 (`--jwt-key FILE`) stays an option. Exactly one is
  required: a key and a secret, two secrets, or neither are start-up errors,
  and the secret is never logged (RFC §7.1).
- Claims: `aud` (must equal `--jwt-audience`), `sub`, `exp` (30 s leeway),
  `role ∈ owner|editor|commenter|viewer`; the file is `doc` or `fileId`
  (string or number), the session `sid` or `sessionId`; an empty or
  contradictory pair is refused. The file name must equal `hello.doc`; `app`,
  when present, must equal `--app`.
- `POST /v1/sessions` (`--control`, loopback, JWT required):
  `{"fileId","jwt","etag"?,"bytes"|"initialBytes"|"url"}` → `201`/`200` with
  `{"fileId","version","epoch","etag","created"}`; `401` bad token, `403`
  token for another file, `400` bad body/etag, `409` the room holds another
  etag, `404` no room and no source, `413` file too large, `502` the source URL
  failed. `fileId` is restricted to `[A-Za-z0-9._-]{1,128}`.
- `POST /v1/sessions/{fileId}/resolve` (`--control`, loopback,
  `Authorization: Bearer <service token>` — the same RS256 JWT, `owner`/`editor`,
  `fileId` must match the path): `{"expectedVersion": N, "etag": "sha256:<64 hex>"}`
  → `200` `{"fileId","version","epoch","etag","cabinetVersion","conflict":false}`;
  `400` bad body/etag or a version not newer than the room's, `401` no token,
  `403` another file or a role without write access, `404` no room,
  `409 not_conflict` (nothing to resolve), `409 busy` (a publication in flight),
  `503` hub unavailable. Every refusal leaves the room unchanged. On success the
  `conflict` flag is gone (`/health`, `/metrics`), the marker adopts
  `{expectedVersion, etag}`, and the next commit publishes with it.
- Commit callback: `POST --callback-url` with
  `{fileId, bytes(base64), sha256, version, expectedVersion, idempotencyKey}`
  and `Authorization: Bearer $PDFCRAFT_COLLAB_CALLBACK_TOKEN` when the env var
  is set, plus the same value in the `Idempotency-Key` header; `2xx` accepted,
  `409` conflict (and the queue stops), a network error / `429` / `5xx` is
  retried in the background with a backoff, any other `4xx` is unpublished
  until the next commit.
- `/health` → `{"ok":true,"rooms":N,"clients":M,"poisoned":[],"conflict":[]}`;
  `ok` is false when a room's journal failed. `/metrics` adds
  `pdfcraft_collab_room_read_only{room="…"} 1`,
  `pdfcraft_collab_room_conflict{room="…"} 1`,
  `pdfcraft_collab_forced_commits_total`,
  `pdfcraft_collab_publication_retries_total` and
  `pdfcraft_collab_conflicts_resolved_total`.
- `--data/<fileId>/` is the durable state: `CURRENT`, `versions/<N>/file.pdf`,
  `journal.jsonl`, `CABINET` (`{"version","etag","expected"}`: the room's
  version of the published base, the etag, and the cabinet's numbering of it). Delete the directory to reset the room. The flat
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

### Cabinet HS256 tickets — auth/lease (2026-11-10, local; not pushed)

The cabinet signs its session tickets with HS256 only
(`mail-frontend/internal/application/craft/service.go`), while the daemon
verified RS256 only (`--jwt-key cabinet-public.pem`), so a PDF did not open in
the cabinet. The transport now accepts both, one scheme per deployment, and
refuses an ambiguous configuration instead of guessing.

- `crates/collab-ws/src/token.rs`: `Hs256Verifier` next to `JwtVerifier`. The
  signature/expiry/audience decode is shared; the algorithm is fixed by the
  verifier, so a ticket signed with the other scheme is `invalid` before any
  claim is read. The cabinet's names are accepted next to the RFC's: `doc`/`sid`
  as well as `fileId`/`sessionId`, and `fileId` as a number as well as a string.
  Empty or contradictory names, a missing `sub`, and a secret under 16 bytes are
  refused. `app` is checked when present; the audience binds the application.
- `apps/pdfcraft-collab-server/src/main.rs`: `--jwt-secret-file FILE` or
  `PDFCRAFT_COLLAB_JWT_SECRET` selects HS256; `--jwt-key FILE` selects RS256.
  Key + secret, two secrets, or neither are start-up errors that name the flags;
  the secret is never logged (errors name the path only). The startup line
  reports `auth hs256|rs256`. `--jwt-key` no longer defaults to
  `cabinet-public.pem`: a deployment must say which scheme it uses.
- Tests: `token.rs` unit tests (valid cabinet ticket, RFC spelling, numeric
  `fileId`, bad signature, expired, wrong `aud`, wrong `app`, missing names,
  contradictory names, scheme confusion, short secret); `tests/e2e.rs` opens a
  real socket with `Hs256Verifier` and checks `welcome` for a valid ticket and
  `error{code:"auth"}` for one signed with another secret; `main.rs` tests the
  configuration matrix and the secret file's trailing newline.

Gates on `agents` (`../scripts/remote-build.sh`), all green at `157f4d239`:

```text
$ cargo clippy -p pdfcraft-collab-ws -p pdfcraft-collab-server -p pdfcraft-collab-daemon --all-targets -- -D warnings
    Finished `dev` profile [optimized + debuginfo] target(s) in 1.72s
$ cargo fmt --all -- --check            # exit 0
$ cargo test -p pdfcraft-collab-ws
test result: ok. 24 passed; 0 failed      (e2e, incl. the HS256 socket test)
test result: ok. 14 passed; 0 failed      (lib, incl. 8 token tests)
$ cargo test -p pdfcraft-collab-server
test result: ok. 29 passed; 0 failed
$ cargo test -p pdfcraft-collab-daemon --bin pdfcraft-collab-server
test result: ok. 34 passed; 0 failed      (incl. the auth configuration tests)
$ cargo test -p pdfcraft-collab-daemon --test conflict_resolve --test crash_recovery --test publication_retry
test result: ok. 1 passed; 0 failed       (each; the real binary still starts with --jwt-key/RS256)
```

`cargo xtask ci` (the workspace sweep) is CI's job and was not run here.

**The cabinet's container was built from this head.** `agents` produced
`pdfcraft-collab-server:0.18.1-collab.3f3bbedb038e` (binary `ac37e4d581ff…`,
image `3085e9710bfb…`) — the tag carries the 12-character prefix of
`3f3bbedb0`, so the image is this HEAD and not `157f4d239`. Its entrypoint
reads `craft-sync-jwt` and selects `--jwt-secret-file`, the room starts
`auth hs256`, and `cabinet-public.pem` is gone from the compose file. A real
cabinet HS256 ticket was then accepted: the room answered
`invalid/no such document session`, not `auth` — the ticket verified and only
the dev-mode room was missing (mail-frontend, thread `meta/mail-frontend`,
2026-11-10 00:12Z).

Not verified, and the reason this is an auth gate rather than an end-to-end
one: **no two-client WSS run for PdfCraft through the cabinet's `nginx` proxy
was made.** GridCraft and DeckCraft were run that way in the same pass; for
PdfCraft only the auth/lease path above is proven, so proxied upgrade,
multiplexing, presence and the commit callback through the proxy remain
unproven at this head. The second client and the owner role are exercised only
over a direct socket (`tests/e2e.rs`, §4 above).

### Rebase onto `origin/main` `ba28458ad` (the brief's `d2139e58a`, moved on) and the gate run at the rebased head (2026-11-09)

The pass was asked to rebase onto `origin/main` `d2139e58a` — the base the
previous pass recorded — because PR #6622 reports `CONFLICTING`. By the time the
tree was fetched, `origin/main` was `ba28458ad` ("M3: a password prompt keeps a
place in the open queue for its retry (#6731)"), 204 commits past `d2139e58a`; a
branch rebased onto `d2139e58a` would not have an empty `merge-tree` against
`origin/main`, so the rebase targeted the current tip. `git merge-tree
--write-tree origin/main HEAD` at the old head named exactly three paths;
nothing else conflicted.

`git rebase --onto origin/main d2139e58a` replayed all **76 branch commits**
with **four conflict stops in three files**:

| Commit | File | Merge |
|---|---|---|
| `a837473b3` (desktop wiring) | `crates/ui-egui/src/app.rs` | **conflict**. Main added `self.window_preferences_frame(&ctx)` at the end of the frame ("FitWindow and CenterWindow, once the document just opened is laid out", the viewer-preferences merge); the branch adds `self.show_collab(&ctx)` under `#[cfg(all(feature = "collab", not(target_arch = "wasm32")))]`. Kept main's line first, then the collab call. |
| `9580b6b9d` (presence cursors) | `crates/ui-egui/src/canvas.rs` | **conflict** in the `DocView` fields. Main added `pub interface: crate::viewer_prefs::TabInterface` (the interface the document hid in this tab, ISO 32000-2 Table 147); the branch adds `remote_cursors` and `local_cursor`. Kept all three, `interface` in main's position. |
| `017e5c395` (browser session) | `crates/ui-egui/src/app.rs` | **conflict** on the same lines as the first one, because the branch widens the guard to `#[cfg(feature = "collab")]` for every platform. Kept main's `window_preferences_frame` line and the widened guard. |
| `e58dfb09a` (restyle) | `crates/engine/src/edit.rs` | **conflict** in the `use crate::{…}` list. Main's list gained `PrinterMarks` (printer marks) and was re-wrapped; the branch adds `Restyle`. Union of both names. |

Every commit subject of the old branch is present in the new one (multiset
equality of `git log --format=%s`, `backup/realtime-collab-pre-ba28458ad` vs
`HEAD`), so nothing was dropped or squashed: every addressable operation and the
idle compaction are still there — `8f71c37db` (page split/combine),
`d975ebb72` (the Split/Combine dialogs sending them inside a session), page
replace/blank insert/insert/duplicate, and `forced_commit_due` /
`forced_commit_idle_secs` in `collab-server`. The only new commit is the style
fix below.

`main` changed 139 files in `d2139e58a..ba28458ad` (11 470 insertions, 374
deletions); 14 of them are also touched by the branch, so 11 merged
automatically. Every file the branch changes that main does not is
byte-identical to the old tip (56 files), and every file main changes that the
branch does not is byte-identical to `origin/main` (125 files). For the three
hand-resolved files the changed (`+`/`-`) lines of `git diff origin/main..HEAD`
are exactly the branch's own changed lines from `git diff d2139e58a..<old tip>`:
for `app.rs` and `canvas.rs` the two lists are identical, and for `edit.rs` they
differ only in the import line, which carries main's `PrinterMarks` beside the
branch's `Restyle`. The rebase moved the base and changed no feature content.

Rustfmt re-wrapped the merged import list differently from the local wrap, so the
style fix is its own commit at the tip, `5f3c6f3c2` "style(collab): wrap the
engine export list after the rebase merge".

Backup of the pre-rebase head: `backup/realtime-collab-pre-ba28458ad`. Pre-rebase
hashes used elsewhere in this file map to the rebased commits as follows:
`a837473b3`→`6aa311bb3`, `9580b6b9d`→`a59e69832`, `017e5c395`→`d0af73f35`,
`e58dfb09a`→`a1eca73d3`, `e097b8e39`→`d975ebb72`, `eb40e16d0`→`8ba680689`,
`362f9c72b`→`8f71c37db`, `a6577074a`→`32e94bc78`, `5b76f7e99`→`ebce0fb6b`,
`4a691c53a`→`1a796930d`, `270e1c11e`→`363b5ada1`, `19f49f812`→`5643e3634`.

Gates at `5f3c6f3c2` (host `agents`, rustc stable, `-j 4`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass (after `5f3c6f3c2`; the first run named only the merged import list) |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **225**, 0 failed: proto 12, server 60 (29 unit + 31 bin), hub 5, client 13, client-pdf 32, session 5, ws 8 unit + 22 e2e, pdf 47, annot 18, daemon `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1 |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **22/22** (inside the run above) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Web build | `cargo check -p pdfcraft-ui-egui --features collab --target wasm32-unknown-unknown` | pass — the 10 warnings are main's native-only helpers unused on wasm (`render`, `ui-egui` save/panels/persist), none in a collab file |
| Layering | `cargo xtask layers` | pass — 50 crates, no layering violations |

Nothing was pushed from that rebase session, so at the time of that run PR #6622
still described `31bd4a7a2` and stayed `CONFLICTING` / `DIRTY`; it has since
moved to the fork's `b27fa89c8` (`MERGEABLE` / `UNSTABLE` — see the header and
the HS256 block above, which is the current state). The full `ui-egui` test
suite was not re-run for this rebase: clippy with `--features collab` compiles
and lints the merged `app.rs`/`canvas.rs`, and the last full run is §6.6 below.

### Split and Combine dialogs inside a room — §5 p.3 / §6.6 (2026-11-09, local; not pushed)

`e097b8e39` on `eb40e16d0`. Limitation 3 left the Split/Combine dialogs exporting
files locally, so in a session they changed nothing on the shared document. They
now go through the page operations of §3.1.5, and the file workflow runs only
when no session owns the document.

- **The session side** (`crates/ui-egui/src/collab.rs`):
  `collab_shares(doc)` / `collab_document()` tell the dialogs whether a session
  owns the open document (false without the feature, so the single-user path is
  untouched); `collab_split_pages(doc, from, count)` and
  `collab_combine_pages(doc, at, part)` build the intent with
  `split_pages_op`/`combine_pages_op` in one place, with `collab_apply_edit`'s
  refusals in the same order — not connected, role without page access, a
  projection error (boundaries, a part over 160 KiB) — all before anything is
  queued. A combine's inverse (the `page.split` of the same range) is computed
  from this replica's document; a split's inverse cannot be (the part is cut by
  the server), so it is `None` until the accepted operation arrives and
  `inverse_after` patches the history group (`Event::Applied`, unchanged).
  History, redo and Undo are the same groups edits use.
- **Split** (`split_active` → `split_shared`): one `page.split`. The plan's
  first cut is where the part starts (`split_cut`): the pages before it keep
  their indices — nobody's comments or presence move — and the tail leaves as
  one part. The dialog says so in a session ("takes the pages from page {p} out
  … {n} pages leave; Undo brings them back") instead of promising N files; a
  plan whose cut leaves a single part is refused with a message. Measured per
  mode: `PageCount`/`Before` from `split_ranges`, `Size` from
  `split_by_size`'s second part. The part is not written to a file in a session
  — RFC §3.1.5 leaves exporting it to the client, and the operation is the only
  copy the room keeps (the limitations below say what that means).
- **Combine** (`combine_staged` → `combine_shared`): the part is built exactly
  as before (`combine_grouped`), then `page.combine` at the session document's
  end (one version for the whole part) instead of a new unsaved document. On
  acceptance the document tab is focused and the Combine tab closed, so the
  broadcast applies to the view; a refusal keeps the list as it is (the error is
  the Collaboration window's, as for other operations) and nothing is exported
  or opened. The Combine view adds one muted line when a session's document is
  open: "In a shared session the files join the open document in one operation."
- **Nothing else changes:** the single-user Split (parts written to a chosen
  folder or downloaded), Combine (a new unsaved document) and their tests are
  the same code path as before, reached when `collab_shares` is false.

Tests:

| Crate | Test |
|---|---|
| `collab-client-pdf` | `two_replicas_split_a_range_out_and_join_it_back`: two `Session`s open the same 4-page file; the intent comes from `split_pages_op` (what the dialog sends); the accepted split (range, both sides, the server-cut part) applies to both replicas, which both hold 2 pages; `inverse_after`'s `page.combine` applies to both and both are back at 4; the combine's own `inverse_for` (re-addressed to the head and epoch it left) is a `page.split` that takes the range out on both again |
| `ui-egui` (`collab::tests`) | `a_shared_split_sends_one_operation_and_undo_joins_the_part` (a real `PdfCraftApp` with a 4-page document and a recorded sink: the dialog path queues exactly one `page.split` at page 1 for 3 pages in epoch 0; applying the accepted operation leaves 1 page and patches the group's inverse to the `page.combine` at the same index in epoch 1; Undo queues that combine, and its accepted form brings the 4 pages back), `a_shared_split_stops_before_the_server` (a split that would take the whole document is refused before anything is queued and sets the session error; another document's id returns `None`, so the caller keeps the file workflow) |
| `ui-egui` (`workflows::files::shared_split_tests`) | `the_shared_cut_is_the_start_of_the_second_part`: `PageCount(1|2|3)` cut at 1|2|3 of a 4-page file, `PageCount(4)` and `Before([0])` have no cut, `Before([2])` cuts at 2 |

Gates (host `agents`, rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, projection | `cargo clippy -p pdfcraft-collab-client-pdf --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, projection | `cargo test -p pdfcraft-collab-client-pdf` | pass — **32** (the 31 of §4 below plus the two-replica test) |
| Tests, UI with the feature | `cargo test -p pdfcraft-ui-egui --features collab --lib` | pass — **927** passed, 1 ignored |
| Tests, UI shell and Combine/Split | `cargo test -p pdfcraft-ui-egui --features collab --test it -- combine`, `-- split` | pass — 83 and 23 |
| UI without the feature | `cargo check -p pdfcraft-ui-egui --all-targets` | pass |
| Web build | `cargo check -p pdfcraft-ui-egui --features collab --target wasm32-unknown-unknown` | pass |

What this did **not** cover before the next section: the part a split takes
out was not written to a file from the session and was not kept (the accepted
operation carried it; the next section opens it as a document); a plan with
more than one cut still removed one contiguous tail; a part over 160 KiB was
refused until the blob channel (RFC §4.1), for a combine as for a replica's
insert; and nothing was run against a real cabinet or in a browser. The first
two of those are addressed below; the multi-cut plan, the 160 KiB budget and a
real-cabinet/browser run remain.

### The split part and several parts per Combine — §6.6 leftovers (2026-11-09, local; not pushed)

`843a8f533` on `a007c4256`. The two items §6.6 left open are closed: a split in a
room no longer loses the part it takes out, and one Combine confirmation joins
the whole list as several parts, all or nothing.

- **The part is kept.** The accepted `page.split` already carries the bytes
  (RFC §3.1.5); `collab-client-pdf::split_part(&Applied)` now hands them to the
  app with the range (`SplitPart { from, to, bytes }`). In a room the app opens
  the part as a document of its own in the background (`open_created_quiet`),
  named as the single-user Split names its files (`<stem> (pages a-b).pdf`,
  `(page a)`), so the pages can be read, printed or saved with Save a copy; the
  shared document keeps the focus, so Undo still belongs to the room. Only a
  split that is the user's Split opens a tab: one sent as an undo or a rollback
  is an inverse and opens nothing (`SPLIT_LABEL`).
- **One confirmation, several parts.** In a room `combine_staged` builds one
  part per combine-list entry (its own range and password, in the list's order)
  and `collab-client-pdf::combine_parts_ops` builds one `page.combine` per part,
  each addressed where the parts before it end. The app sends them one at a
  time (`CombineBatch`): a structural operation is accepted only at the head of
  the version, so the next part goes out when the answer to the previous one is
  in. The confirmation is one history group, so Undo takes it back in one step,
  and undo sends a group's inverses in reverse order — the last part leaves
  first, so the parts below it keep the positions they joined at — also one at
  a time, each re-addressed to the head the previous answer left
  (`OpSequence`).
- **All or nothing.** A part refused `stale` (retryable) stops the queue: the
  parts already joined are taken out again with the `page.split` of each, built
  in reverse on the head the retry's snapshot leaves (`combine_mark_rollback`,
  then `collab_combine_rollback`), the history group goes away, and the error
  says the combine was taken back so the list can be sent again. A
  non-retryable refusal (`limit`, `invalid`, `forbidden`) rolls back the same
  way. A part over `MAX_RESTORE_PAGE_BYTES` (160 KiB) is still refused before
  the first operation is sent and waits for the blob channel (RFC §4.1); the
  budget was not raised.
- **Nothing changes outside a room:** without a session the Combine tab builds
  the merged document as before (`combine_grouped` on the whole list) and opens
  it unsaved, and the Split dialog writes files as before.

Tests:

| Crate | Test |
|---|---|
| `collab-client-pdf` | `a_combine_of_several_parts_is_an_ordered_sequence` (one operation per part, each at the position the previous parts leave; a part over the budget or a position past the last page refuses the whole confirmation), `two_replicas_combine_several_parts_and_take_them_back` (both replicas converge after two parts, and the splits in reverse order restore the 4-page document), `the_accepted_split_part_opens_as_its_own_document` (`split_part` gives the range and the bytes, which open as a 2-page document) |
| `ui-egui` (`collab::tests`) | `a_shared_split_sends_one_operation_and_undo_joins_the_part` (now also: the accepted part opens in a tab named `t (pages 2-4).pdf` with 3 pages, and the shared document keeps the focus), `a_shared_combine_joins_its_parts_one_at_a_time_and_undo_takes_them_back` (one `page.combine` sent, the second follows the first answer at index 6, both parts are one history group, and Undo sends the splits at 6 then 4), `a_stale_part_rolls_the_shared_combine_back` (a retried part waits for the snapshot, then the split at the joined index takes the part out; the history group is gone) |
| `collab-ws` e2e | `two_clients_combine_parts_in_order_and_roll_back_a_stale_one` (two parts join in list order on both replicas, reverse-order undo restores 2 pages, a stale second part changes nothing and the joined first is taken out again) |

Gates at `843a8f533` (host `agents`, rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, projection | `cargo clippy -p pdfcraft-collab-client-pdf --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, adapter | `cargo test -p pdfcraft-collab-pdf` | pass — 47 |
| Tests, projection | `cargo test -p pdfcraft-collab-client-pdf` | pass — **35** (32 of §4 plus the three above) |
| Tests, session and e2e | `cargo test -p pdfcraft-collab-ws` | pass — 8 unit + **23** e2e (22 plus the new combine test) |
| Tests, UI with the feature | `cargo test -p pdfcraft-ui-egui --features collab --lib` | pass — **964** passed, 1 ignored (927 of §4 plus the new collab tests) |
| UI without the feature | `cargo check -p pdfcraft-ui-egui --all-targets` | pass |
| Web build | `cargo check -p pdfcraft-ui-egui --features collab --target wasm32-unknown-unknown` | pass |

What this does **not** cover: the part opens as an unsaved document, not in a
file — saving it is the user's Save a copy; a plan with more than one cut still
removes one contiguous tail (the other parts are not separate documents); the
combine list is sent as one part per list entry, so a single entry whose PDF is
over 160 KiB still waits for the blob channel; and nothing was run against a
real cabinet or in a browser.

### Splitting and combining pages — §5 p.3 / limitation 3 (2026-11-09, local; not pushed)

`362f9c72b` on `a6577074a`. Limitation 3 said split/combine were not shared:
the app's Split and Combine are file workflows (`Session::split`/`combine`
produce a new document, `study.md` §6), so a session had nothing to offer. They
are server-ordered structural operations now, with the §3.1.4–3.1.5 contract: the
operation carries the pages that move, both sides of the page set are named, the
epoch moves, and the undo is the operation that puts the page set back.

- **`page.split`** (`{"action":"page.split","count":c}`, target `page` = the
  first page of the part) takes `count` pages out of the document. The part is
  cut by the server from the authoritative document (`Session::extract`) and
  travels in the accepted operation as `bytes` (base64, ≤
  `MAX_RESTORE_PAGE_BYTES` = 160 KiB), so every replica and the journal can join
  it back without a second channel. A client-supplied `bytes` is ignored: the
  server always cuts the part itself, or an undo could put content into the
  document through a split. The accepted payload names both sides — `from`,
  `to`, `count`, `kept` (the pages left in the document) and `bytes` — and the
  result adds `part: {sha256, len}`.
- **Numbering is deterministic.** The pages before `from` keep their index, the
  pages from `to` on move down by `count`, and the part is numbered from its own
  first page. A combine inserts `count` pages at `at` (the page count appends)
  and every page from `at` on moves up by `count`.
- **The boundaries are the document's.** A split with `count == 0`, a range that
  runs past the last page, an address that is not a page, and a split that would
  leave no page in the document (`count == page count`) are `invalid` before
  anything changes; a part over the budget is `limit`. A combine with a position
  past the last page, no or zero `count`, a part that is not a readable PDF, or
  a `count` that is not the part's own page count is `invalid`, and a part over
  the budget `limit`. None of the refusals moves `version`/`epoch`, writes a
  journal line or is applied locally.
- **`page.combine`** (`{"action":"page.combine","count":c,"bytes":"<part>"}`,
  target = insert position) is the in-room form of Combine Files and the undo of
  a split: a whole part joins the document in one version and one epoch step,
  where an insert from file is one operation per page (`page.insert`).
- **Undo is the other operation.** The inverse of a split is `page.combine` at
  `from` with the part the accepted operation carries (`inverse_after`); the
  inverse of a combine is `page.split` of the same range at the same index
  (`inverse_for`, re-addressed by `inverse_after` in the epoch the combine
  left). Both are ordinary operations, so a later structural change makes the
  undo `stale` and it is retried, never forced.
- **Replay needs no client file.** A journaled split is cut again from the
  document in replay order; a combine inserts the bytes it recorded. The extract
  writes a `ModDate` from the session clock, so a replayed split's part is
  byte-different from the accepted one, but the journal keeps the accepted bytes
  and a replica's undo comes from its own `applied` frame — the page set is the
  same either way.
- **Client projection.** `edit_for` turns an accepted split into
  `Edit::DeletePages` of the range and a combine into `Edit::InsertPagesFrom`
  (`pages: None`, the whole part); `split_pages_op`/`combine_pages_op` build the
  intents, with the same boundary and budget checks on this side so nothing the
  room would refuse is sent. The app's Split/Combine dialogs are wired to these
  operations in the section above (§4, this session); they export files locally
  only when no session owns the document.

Tests:

| Crate | Test |
|---|---|
| `collab-pdf` | `a_split_takes_a_range_out_and_carries_the_part` (the range leaves, both sides are numbered, the part is the page that left, the join puts it back at the index), `a_split_at_the_documents_edge_is_refused` (empty, whole-document, past-the-end, missing page, no count → `invalid`; a part over the budget → `limit`; nothing changes), `a_split_addressed_to_another_epoch_is_retryable` (stale + retryable, then applied re-addressed), `a_combine_inserts_the_part_it_carries_in_one_step` (whole part in one version, deterministic numbering, a split of the range undoes it), `a_combine_refuses_a_part_it_cannot_carry` (past the end, count mismatch, bad base64, not a PDF, no count, no bytes → `invalid`; over the budget → `limit`), `a_split_and_a_combine_replay_from_their_operations` (a fresh room reaches the same page set from the journal alone) |
| `collab-client-pdf` | `a_split_removes_its_range_and_the_accepted_operation_joins_it_back` (the replica deletes the range, the accepted part becomes one `page.combine` at the old index), `a_combine_inserts_its_part_and_splits_it_back` (whole-part insert, `inverse_for` and `inverse_after` name the same range), `split_and_combine_boundaries_are_refused_before_sending` |
| `collab-ws` e2e | `two_clients_split_the_document_in_halves_and_join_it_back` (both replicas lose the same range and number the rest the same, the undo joins the part back, a combine past the last page is refused), `a_split_addressed_to_a_passed_epoch_is_stale_and_retried` (after a rotation the old-epoch split is stale/retryable and changes nothing; the re-addressed intent applies on both replicas) |

Gates at `362f9c72b` (host `agents`, rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **224**: proto 12, server 29, hub 5, client 13, client-pdf 31, session 5, ws 8 unit, pdf 47, annot 18, daemon 31 + `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1, e2e 22/22 |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **22/22** (inside the run above) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |

What this does **not** cover: a part travels inside one operation, so a split or
a combine whose part is larger than 160 KiB is refused with `limit` until the
blob channel lands (RFC §4.1); the dialogs route to these operations only when a
session owns the document, and the part a split takes out is not written to a
file from the session (§4 above); and a split's undo depends on
the part the accepted operation carried, so a client that lost its history
before the part was applied cannot rebuild it (the room never kept a copy — the
operation is the copy). Bookmarks, layers, attachments, form structure and
content edits are still not shared.

### Conflict resolution — §5 p.6 / limitation 6 (2026-11-09, local; not pushed)

`62d48d30a` on `fa89dff5b`. A `409` from the cabinet stopped the publication
queue for good: nothing in the daemon could change the room's expectation, so
the only ways out were a new session over the cabinet's version or deleting the
room (limitation 6). The operator's way out is now an endpoint:

- **`POST /v1/sessions/{fileId}/resolve`** on the control plane, `Authorization:
  Bearer <service token>` — the same RS256 JWT the session uses, verified by the
  same `Verifier`; `claims.file_id` must equal the path and the role must be
  `owner`/`editor` (a commenter/viewer cannot save the file, so it cannot resolve
  it). Body `{"expectedVersion": N, "etag": "sha256:<64 hex>"}`; etags are
  normalised to lower-case hex. `200` answers the room's `version`/`epoch`, the
  adopted `etag`, `cabinetVersion` and `conflict: false`. Refusals: `401` no/bad
  token, `403` another file or a read-only role, `404` no room, `400` bad body or
  a version not newer than the room's, `409 not_conflict` (also a repeat),
  `409 busy` (a publication in flight), `503` hub unavailable. None changes the
  room.
- **`Room::resolve_conflict`** checks, in order: the room is in `conflict`; no
  publication is in flight (it was addressed to the old marker); the version is
  newer than the one the session already holds (a repeat or a roll-back would
  fail the same way); then `Adapter::set_published` writes the marker and the
  flag is cleared. It does not publish and does not move `version`: the room is
  still `dirty`, and the next commit (button, last leave, forced compaction)
  publishes the journal's work with the adopted `expectedVersion`.
- **The marker keeps two numberings.** `CABINET` is now
  `{"version","etag","expected"}`: `version` is the room's version of the
  published base (what `startup`/`is_dirty` use after a restart), `expected` the
  cabinet's numbering of the same copy (the next `expectedVersion`). They are
  equal until a resolution; `expected` is `#[serde(default)]`, so an older
  marker is read as `expected = version`. Without this, resolving against an
  external version would either make the next commit fail again or make a
  restart believe unpublished work was saved.
- **The callback answer is used.** `Publisher::publish` now parses the `2xx`
  body `{version, etag}` (INTEGRATION.md §5.2) and that copy is what the room
  adopts; an empty or unreadable body falls back to `expectedVersion + 1` and
  `sha256:<hex>` of the published bytes. This is what makes a resolution last for
  the session, not one commit.
- **`WsServer::resolve_conflict`** moves the marker under the hub lock (so no
  publication can run with the old one) and counts successes in
  `Shared::conflicts_resolved`, exposed as
  `pdfcraft_collab_conflicts_resolved_total`. `/health`'s `conflict` list and
  `/metrics`'s `pdfcraft_collab_room_conflict` are derived from the room, so they
  clear as soon as the resolve succeeds.
- Operation of the daemon (endpoints, codes, conflict lifecycle, metrics) is
  written down in `docs/collab/server.md`, and RFC §8.2/§9/§16 p.6 carry the
  contract.

Tests:

| Crate | Test |
|---|---|
| `collab-server` | `an_operator_resolution_adopts_the_cabinets_marker_and_clears_the_conflict` (conflict → resolve → flag gone, marker adopted, still dirty; a repeat is refused and changes nothing), `a_resolution_must_name_a_version_the_cabinet_moved_to` (5/4/0 refused as `invalid` against a known 5; 6 accepted), `a_resolution_waits_for_a_publication_in_flight` (a conflicted room committed again → resolve is `busy`, nothing moves, the publication's own answer decides) |
| `collab-ws` | `a_resolution_needs_a_room_and_a_conflict` (`Missing`/`NotInConflict`, no counter increment) |
| daemon rooms | `an_operator_resolution_lets_the_next_commits_publish_again`: a cabinet holding version 7 refuses two commits (both `expectedVersion 0`, queue stopped); resolve →7; the next commit sends `version 2, expectedVersion 7` with the derived key, the cabinet answers `{version: 8}`, and the commit after that sends `expectedVersion 8` |
| daemon control | `an_operator_resolves_a_conflict_and_a_second_call_is_refused`: conflict through a real callback `409`; 401/403 (another file, viewer)/400 (bad etag, version 0)/404; then a 200 with `version 1` (unmoved), `cabinetVersion 4`, `conflict:false`; metrics show no conflict and `conflicts_resolved 1`; a second resolve is `409 not_conflict` and is not counted |
| daemon e2e (`tests/conflict_resolve.rs`) | `a_conflict_is_resolved_once_and_the_next_commits_publish_again`: the real daemon in dev mode against a fake cabinet holding version 7 — the commit is refused, `/health` and `/metrics` flag the room, a second commit is refused with the same expectation, no token is `401`, resolve is accepted, `/health`/`/metrics` clear and count 1, a second resolve is `409`, the next commit reaches the cabinet with `expectedVersion 7` and the derived idempotency key, the cabinet stores one version (8), the commit after that expects 8 |

Gates at `62d48d30a` (host `agents`, rustc 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **182**: proto 12, server 27, hub 5, client 13, client-pdf 20, session 5, ws 8 unit, pdf 30, annot 15, daemon 31 + `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1, e2e 13/13 |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, 13/13 (inside the run above) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |

What this does **not** cover: the resolve does not publish by itself, so a room
with no further commit, no join and no last leave waits for one of those (the
queue is in-process, as before); the cabinet's own numbering is taken from the
callback body, and a cabinet that reuses numbers would need an adapter; a
conflict caused by the dev-mode mirror changing needs the operator to name the
mirror file's etag (the source file is the cabinet there); and nothing was run
against a real cabinet.

### Presence selections — §5 p.4 / limitation 4 (2026-11-09, local; not pushed)

`b4e9179d8` on `f13bc634e`. The wire already had `Presence.selection`
(`Selection::{Annotation,Text}`, proto `f13bc634e` and earlier), and the server
and hub already forwarded it with the size/rate limits; what was missing was the
client: the UI sent `selection: None` and drew nothing but pointers. Limitation 4
is closed on the client side:

- **What travels.** The selected comment's `/NM` when it is on the page in view
  (`Selection::Annotation`); otherwise a rectangle in PDF user space
  (`Selection::Text { page, rect }`) — the selected comment's markup quads, the
  bounding box of the text selection's quads (a range is one box), or the
  rectangle of a column select. No text, no comment `Contents`, no author and no
  token is ever put in the frame; the client test pins the frame's keys.
- **Sending.** `collab_presence` computes the selection from `DocView` state
  (`comments.selected`, `DocView::selection_quads`) and `DocInfo` (the `/NM` and
  geometry of a comment). The coalescing rule now includes it: a change of
  selection sends within the 100 ms interval, the 1 s heartbeat still keeps a
  reader current, and presence is sent by every role, viewers included (ADR-0006
  §5). A selection addressed to page `p` travels with `PageRef { p, epoch }`, not
  with the page in view, so scrolling does not move it.
- **Receiving and drawing.** `CollabUi::selections` applies the pointer's rules —
  the 30 s TTL and the sender's page-set epoch — and the canvas paints a
  translucent fill (alpha 48) with a one-pixel outline in the actor's colour,
  over the page and its comments, after the cursors (`paint_remote_selections`). An
  `Annotation` selection is resolved to the comment's quads, else its rectangle;
  a name that is no longer on the page draws nothing. The replica's own presence
  never arrives back (the hub broadcasts to everyone but the sender), so the
  local selection is never drawn as a remote one; the editor's own highlight
  stays the editor's.
- **Never an operation.** Presence still goes through `Room::presence`, which
  only checks the actor and the rate, and through the hub's `broadcast_except`;
  it is not journaled and does not touch `version`/`epoch`. A viewer may share a
  selection and cannot write; the room accepts the next real operation as before.

Tests:

| Crate | Test |
|---|---|
| `collab-ws` e2e | `two_clients_share_a_selection_without_an_operation`: editor, commenter and viewer over a socket; the editor's `Text` selection reaches both others, the viewer's `Annotation` selection reaches the editor, versions stay 0, a joiner's resync tail is empty, and after a rotation the tail holds exactly that one operation |
| `collab-client` | `a_presence_frame_carries_a_selection_and_no_document_content`: a `text` selection round-trips, the frame has exactly the presence keys, and `contents`/`text`/`jwt`/`token`/`bytes`/`password`/`author` are absent; the frame stays under `MAX_PRESENCE_BYTES` |
| `ui-egui` | `a_remote_selection_expires_with_its_presence` (fresh draws, another epoch does not, stale after 30 s does not), `a_selected_comment_travels_by_its_name`, `a_text_range_travels_as_its_bounding_rect` |

Gates at `b4e9179d8` (host `agents`, rustc 1.99.1):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, client + projection + transport | `cargo clippy -p pdfcraft-collab-client -p pdfcraft-collab-client-pdf -p pdfcraft-collab-ws --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, client + projection | `cargo test -p pdfcraft-collab-client -p pdfcraft-collab-client-pdf` | pass — client **13**, client-pdf 20 |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **13/13** |
| UI, the collab module | `cargo test -p pdfcraft-ui-egui --features collab --lib collab::` | pass — 6 (the three new ones and the three token/role ones) |

What this does **not** cover: several selected comments share nothing (only the
primary selection is sent); a multi-line range is one bounding box, not one quad
per line (so the box may cover unselected text between lines); a caret/typing
position is not shared; and selection rendering was not checked in a browser
(the host page still gets only the participant list's page, not a selection).
The full `ui-egui` suite was not re-run: clippy with the feature compiles it and
the collab module's tests pass, but the page-painting path has no new test that
drives a real frame.

### Presence selections, second slice — several `/NM`, per-line quads and a caret (2026-11-10, local; not pushed)

`5bad54c40` on `38d0bf56d`. The first slice (`b4e9179d8`, above) sent one comment
name or one bounding box. This closes the three gaps it named and keeps the wire
compatible:

- **Several selected comments are one list.** `Selection::Annotation` gained an
  optional `also: Vec<String>`: `nm` is the comment the sender acts on (the one
  the handles belong to) and `also` the others selected with it, both taken from
  `CommentView::selection()` and only for the page in view (a presence frame
  names one page). A peer that predates the field still draws the primary one.
- **A text range is one rectangle per line piece.** `Selection::Text` gained
  `quads: Vec<[i64; 4]>` (reading order, empty when the sender only knows the
  box) and `caret: Option<[i64; 2]>` (`DocView::selection_caret`: the active end
  of the run, or the dragged corner of a column select, in user space). `rect`
  stays the bounding box, so an old peer draws the range as before. The canvas
  fills each quad separately; the box between lines no longer covers text the
  sender did not select, and the caret is a thin line in the actor's colour.
- **Bounds instead of a broken frame.** `MAX_SELECTION_NAMES = 32`,
  `MAX_SELECTION_QUADS = 48` (`collab-proto`). Over 48 pieces the client sends the
  box alone; the protocol refuses more names/pieces (`TooMany`) and any frame
  over `MAX_PRESENCE_BYTES` (2 KiB) as before. The server validates the selection
  in `Presence::validate`, so a refused frame reaches nobody and the connection
  stays (e2e `an_oversized_selection_is_refused_and_the_room_keeps_going`).
- **The app's state carries it to the host page — as far as the pinned contract
  allows.** `CollabParticipant` now has `selection: Option<ParticipantSelection>`
  (`Annotations(Vec<String>)` or `Text { page, rects, caret }`, `page` 1-based)
  next to `page`, `active`; the presence report is coalesced on that change.
  **The wire does not carry it yet:** `craft-web-host` at
  `7666e2b100d8cf12998f9b57d3b036964cc3189c` describes a `Participant` as
  `clientId`/`role`/`color` only, so `apps/pdfcraft-web`'s `participants()` cannot
  pass `page`/`selection`; its contract says new optional fields keep version 1,
  so adding them there is the one change left (a foreign crate, ADR-0016 — not
  edited here). The shell's `presence_fn` says so in place.
- **Never an operation.** Unchanged: presence is not journaled, does not bump
  `version`, and a viewer may share any selection.

Tests:

| Crate | Test |
|---|---|
| `collab-proto` | `the_plain_selection_form_still_parses` (a frame with only `nm`/`rect` parses and re-encodes unchanged), `a_selection_lists_names_quads_and_a_caret`, `selection_limits_are_enforced` (33 names, 49 quads, empty name, off-page), `presence_over_the_limit_is_refused` (within the counts, large coordinates still overflow 2 KiB) |
| `collab-client` | `a_presence_frame_carries_a_selection_and_no_document_content` (keys at every depth, `quads` and `caret` present, `contents`/`text`/`jwt`/`token`/`bytes`/`password`/`author` absent), `a_presence_frame_lists_selected_comments` |
| `collab-ws` e2e | `two_clients_share_a_selection_without_an_operation` (now a two-line range with a caret and a viewer's three-name list; versions stay 0, the tail stays empty), `an_oversized_selection_is_refused_and_the_room_keeps_going` (49 quads → `invalid`, then a valid selection and a rotation are accepted) |
| `ui-egui` | `several_selected_comments_travel_as_a_list`, `a_text_range_travels_as_one_rect_per_line` (the caret too), `a_text_selection_from_another_epoch_is_not_drawn`, `a_text_selection_is_sent_as_quads_and_a_caret` (and the box fallback over 48), `the_local_selection_lists_comments_and_per_line_quads` (a real `DocView`: two selected comments, an off-page comment and a two-line range), `the_host_report_carries_a_participants_selection` (page/selection, and neither past the TTL) |

Gates at `5bad54c40` (host `agents`, rustc 1.99.1, warm `target/`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, proto + client + transport + server + hub + UI | `cargo clippy -p pdfcraft-collab-proto -p pdfcraft-collab-client -p pdfcraft-collab-ws -p pdfcraft-collab-server -p pdfcraft-collab-hub -p pdfcraft-ui-egui --features pdfcraft-ui-egui/collab --all-targets -- -D warnings` | pass |
| Tests, proto / client / server / hub | `cargo test -p <crate>` | pass — proto **15**, client **14**, server **29**, hub **5**; 0 failed, 0 ignored each |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **28/28** |
| UI, the collab module | `cargo test -p pdfcraft-ui-egui --features pdfcraft-ui-egui/collab --lib collab::` | pass — **16** (955 filtered out) |
| Web shell (wasm) | `cargo check -p pdfcraft-web --target wasm32-unknown-unknown --features collab` | pass |

What this does **not** cover: the caret is the active end of the current text
selection, not the caret inside a form field being typed into (`forms_ui` owns
that editor and has no page-space caret to hand over yet); only comments on the
page in view are named, so a multi-page selection falls back to the primary
comment's rectangle; and the host page still receives no selection until
`craft-web-host` gains the two optional fields above. Selection painting has no
test that drives a real frame: the collab module's tests cover the projection
into `RemoteSelection` and the state, and clippy compiles the canvas path.


### The publication queue — §5 p.6 / limitation 6 (2026-11-09, local; not pushed)

A commit writes its base locally and then POSTs the file to the cabinet. Until
now a failed POST was only retried by the next commit or by the last-client-leave
commit (limitation 6), so a room whose cabinet was down for a minute stayed dirty
until somebody acted.

- **A retryable failure is queued, not dropped.** `Publication::run` now takes
  `&self` (it used to consume the box), so the same publication object can be run
  again with exactly the same payload. `Room::finish` marks a failure as
  retryable when the adapter says so and the code is not `Conflict`, keeps the
  room `publishing`, and hands the publication back through
  `Room::take_retry`; `Hub::finish` returns it to the transport.
- **The queue lives in the transport, the thread in the daemon.**
  `collab-ws` keeps one `Queued` entry per document (`publication`, `attempts`,
  `due_ms`), `WsServer::retry_publications(now_ms)` runs the entries whose
  backoff elapsed (outside the hub lock, `Room::finish_retry` records the
  outcome) and `WsServer::spawn_publication_retries(interval)` is the loop the
  daemon starts with 100 ms. Backoff is `publication_backoff` (default 1 s)
  doubling per attempt up to `publication_backoff_max` (default 60 s); both are
  `collab-ws::Config` fields, so a test can use 20 ms.
- **A retry cannot become a second version.** The queue re-runs the publication
  of the same base: same `version`, same `expectedVersion`, same `sha256`. The
  callback also carries an idempotency key, `fileId:version:sha256`, in the body
  (`idempotencyKey`) and in the `Idempotency-Key` header, so a cabinet that took
  the bytes but lost the answer recognises the duplicate instead of creating
  another version. The key is derived, not generated: a retry after a restart
  sends the same one.
- **A queued retry keeps the room `publishing`,** so a client commit in the
  meantime gets `busy` (retryable) rather than a newer base overtaking the
  queued one (the rule of limitation 11). A successful retry advances
  `committed_version` and releases the room; if edits arrived while the
  publication was queued and nobody is connected, the transport publishes that
  newer base too, so nothing waits for a join.
- **A `409` stops the queue until a re-seed.** `finish_retry` flags the room
  `conflict`, releases it, and the entry is dropped: retrying the same
  `expectedVersion` cannot succeed (RFC §8.2). A `4xx` that a retry would get
  again (a contract error) is not queued either — only `TransportError`s, `429`
  and `5xx` are, which is what `Publisher` reports as `PublishError::Retryable`.
- `/metrics` gains `pdfcraft_collab_publication_retries_total`
  (`Metrics::publication_retries`, a process counter of background attempts).
  `/health` is unchanged.

Tests:

| Crate | Test |
|---|---|
| `collab-server` | `a_retryable_publication_failure_is_queued_and_keeps_the_room_busy` (failure → queued → `busy` commit → retry succeeds → room free), `a_failure_no_retry_would_fix_is_not_queued`, and the `409` test now asserts the queue stays empty and the room is released |
| `collab-ws` | unit `the_publication_backoff_doubles_and_caps`; e2e `a_retryable_publication_failure_is_retried_in_the_background` (a publication that fails once; the queue retries it, `metrics().publication_retries` is 1, the room is clean and not `publishing` when the test ends) |
| daemon | `tests/publication_retry.rs` `a_lost_callback_answer_is_retried_with_the_same_key_and_one_version`: the real daemon in dev mode with `--callback-url`; the fake cabinet **takes** the first delivery and closes without an answer (the classic lost response), sees the retry as a duplicate of the same key and answers `2xx`. Asserts two deliveries, same `version`/`expectedVersion`/`sha256`/`idempotencyKey` in both, one stored version, `pdfcraft_collab_publication_retries_total 1`, no `conflict`, and that the next commit sends `version 2, expectedVersion 1` — the marker advanced without a second version. |
| daemon (rooms) | `a_transport_failure_is_retryable_and_a_refused_body_is_not` (503/network → `Retryable`, 400 → `Failed`), and `a_commit_posts_the_file_and_the_versions` now checks `idempotencyKey` |

The existing `a_failed_forced_publication_is_not_retried_in_a_loop` still holds
and is the guard against the opposite mistake: its publication fails with
`retryable: false`, so the queue never picks it up and exactly two publications
run (the forced commit and the last leave).

Gates (host `agents`, rustc 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **173**: proto 12, server 24, hub 5, client 12, client-pdf 20, session 5, ws 7 unit, pdf 30, annot 15, daemon 29 + `crash_recovery` 1 + `publication_retry` 1, e2e 12/12 |

What this does **not** cover: the queue is in-process, so a restart loses it —
an unpublished base is then republished by the next commit, the next join or the
last leave (the existing paths), and the derived idempotency key still makes
that repeat a duplicate to the cabinet. A time-based republish for a room that
goes quiet after a restart is still open. A cabinet that accepts the bytes and
then answers `4xx` forever leaves the room dirty with the queue empty (the next
commit retries) — that is deliberate: a `4xx` is the contract being wrong, not a
transport failure.

### Restyling a comment and status replies — §5 p.2 / limitation 2 (2026-11-09, local; not pushed)

`ce5c38798` on `b7f66cde8`. Limitation 2 said that "edits that only change an
annotation's style" and "comment replies that carry a status (`/State`)" were
not shared. Both are now ordinary merge operations:

- `annotation.update` (target `Target::Annotation`, payload `action`,
  `style`, optional `expect`) sets the **final** look of the comment `/NM`:
  `color`, `opacity`, `width`, `fill`, `border_color`, `border_shape`, each
  optional, `null` for `fill`/`border_color`/`border_shape` meaning "take it
  away". It is one operation where the interface sends up to four edits
  (`StyleAnnotation`, `FillAnnotation`, `BorderColorAnnotation`,
  `BorderShapeAnnotation`, usually inside `Edit::Batch`), so a replica applies
  one version and undo takes back one choice.
- Why one engine edit: `Edit::RestyleAnnotation` (`pdfcraft_annot::restyle`)
  applies the whole look all-or-nothing — every property is checked before the
  first mutation (locked, `can_draw`, foreign text, does the comment have a
  width/fill/border at all), the annotation and its Replace Text partners are
  snapshotted, and a failure part-way puts the dictionaries back. A sequence of
  the interactive edits (`Edit::Batch`) has no rollback, and a half-applied
  operation that was then rejected would leave the server and every replica
  disagreeing.
- The address is checked **before** any change: a `/NM` that is not on the page
  (`annotation_index` → `conflict` "the annotation was deleted"), a locked
  comment (`unsupported`), or a property the comment does not have — `width` on
  a sticky note, `fill` on a line, `border_*` outside text boxes/typewriter
  text, `border_shape` on a callout — is `invalid`/`unsupported`, and the
  document is untouched (test `an_update_that_cannot_be_applied_changes_nothing`).
- Undo is compare-and-set (ADR-0010 §3): the accepted operation carries `style`
  (what it set, `expect` removed) and `before` (the previous values of exactly
  those fields), and the undo is `annotation.update` with `style = before` and
  `expect = style`. `StyleUpdate::still_matches` compares only the named fields
  with a `5e-4` tolerance (the engine writes three decimals); a mismatch is
  `conflict`, not retryable, and the other user's look stays.
- `annotation.reply` gained an optional `state`
  (`none`/`accepted`/`rejected`/`cancelled`/`completed`). A state reply needs
  the client's `nm`; the adapter applies `Edit::SetAnnotationStatusNamed`
  (new, the named twin of `SetAnnotationStatus`, like `ReplyToAnnotationNamed`),
  so the engine writes `/State` + `/StateModel /Review` and the status text
  ("Accepted set by bo") under the server's `/NM` on every replica. The
  accepted operation carries the canonical `text` and `state`. `restore` of a
  state reply rebuilds a state reply (`SetAnnotationStatusNamed`), not a plain
  comment — the delete's copy keeps the body with `state`.
- A reply resolves its parent first (`annotation_index`), so a reply addressed
  to a deleted comment is `conflict` before anything is added; deleting a
  comment takes its replies with it (`delete_annotation`), which is what the
  test then checks.
- Client projection: `style_update_for_edits` folds a group of interactive
  style edits on one comment into one `StyleUpdate` (a single `StyleAnnotation`
  is the one-element case), `edit_for` rebuilds `Edit::RestyleAnnotation`, and
  `inverse_for`/`inverse_after` build the CAS undo. `Edit::SetAnnotationStatus`
  from the comment panel/menu becomes a named state reply. `annotation.restore`
  on a replica now understands `parent` and `state` (before this a restored
  reply fell through to `AddAnnotationNamed` and a reply body that is not a
  shape was unreadable). Line endings (`/LE`) do not travel yet: a style edit
  carrying them is refused whole (`unsupported`), never partly applied.

Tests added (collab):

| Crate | Test |
|---|---|
| `collab-annot` | `a_style_update_round_trips_and_says_what_it_clears`, `a_style_update_needs_a_property_and_names_no_unknown_one`, `a_style_update_refuses_a_property_the_comment_does_not_have`, `before_records_only_what_the_update_names_and_the_undo_checks_it`, `the_review_state_names_are_stable` |
| `collab-pdf` | `an_update_restyles_a_comment_and_carries_the_look_it_replaced` (incl. replay on a fresh replica), `an_update_that_cannot_be_applied_changes_nothing`, `an_undo_of_a_restyle_puts_the_old_look_back_unless_someone_restyled_since`, `a_state_reply_sets_the_status_and_a_reply_to_a_deleted_comment_is_refused` |
| `collab-client-pdf` | `a_style_edit_becomes_one_update_and_its_inverse`, `a_batch_of_style_edits_is_one_update_and_round_trips`, `a_style_edit_the_wire_cannot_carry_is_refused`, `a_state_reply_keeps_its_status_through_the_wire_and_a_restore`, `the_accepted_update_answers_with_the_operation_that_undoes_it` |
| `collab-ws` e2e | `two_clients_restyle_a_comment_and_undo_it` (two clients: both replicas see colour/opacity/width/fill, undo from the accepted operation, a later restyle makes the undo a `conflict`), `two_clients_reply_with_a_status_and_a_deleted_parent_refuses_a_reply` (both replicas get `/State` + "Accepted set by bo", the status survives delete/restore from the copy, a reply to a deleted parent is refused and adds nothing) |

Gates at `ce5c38798` (host `agents`, rustc 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **167**: proto 12, server 22, hub 5, client 12, client-pdf 20, session 5, ws 6 (unit), pdf 30, annot 15, daemon 28 + 1 (`tests/crash_recovery.rs`) + the 11 e2e |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **11/11** (run inside the `cargo test` above) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| wasm, client crate | `cargo check --target wasm32-unknown-unknown -p pdfcraft-collab-client-pdf` | pass |
| Layering | `cargo xtask layers` | pass — 50 crates, no violations |

Not re-run for this slice: the full UI suite (the only `ui-egui` change is one
arm in `comment_page` for `Edit::RestyleAnnotation`; clippy with `--features
collab` is green on it). The branch is 53 commits ahead of `origin/main`
`ca9e3c2c3` and 0 behind; nothing was pushed, so PR #6622 still describes
`81035bf2`.

### Line endings, font size and comment properties — §5 p.2 / limitation 2 (2026-11-09, local; not pushed)

`c400147b3` on `44ff6d3be`. The restyle slice (`ce5c38798`, above) left three
gaps: a style edit that carried line endings was refused whole, and the font
size and the author/subject/note-icon changes (`Edit::FontSizeAnnotation`,
`Edit::SetAnnotationInfo`) were not shared at all, so Comment Properties ▸ OK
was refused whenever it touched them. All three are ordinary merge operations
now:

- **`annotation.update` carries `/LE`.** `StyleUpdate::endings` (two names for a
  line or polyline, one for a callout — `"none"`, `"square"`, `"circle"`,
  `"diamond"`, `"open_arrow"`, `"closed_arrow"`, `"butt"`, `"r_open_arrow"`,
  `"r_closed_arrow"`, `"slash"`) travels in the same `style` object, and the
  engine's `Restyle` gained the same field, so `pdfcraft_annot::restyle` still
  applies the whole look all-or-nothing (it validates the pair before the first
  mutation and restores the dictionary on a later failure). `expect`/`before`
  compare the pair; a note, or the wrong number of endings, is `invalid` before
  anything changes.
- **`annotation.font_size`** (`{"action":"annotation.font_size","size":14.0,
  "expect":10.0}`) addresses a text box, typewriter text or callout by `/NM` and
  applies `Edit::FontSizeAnnotation`: 1–400 pt, the box regrows for the wrapped
  text, the accepted operation carries `size` and `before` (the previous
  number), and the undo is `size = before`, `expect = size`. A comment without
  `/DA`, a non-numeric or out-of-range size and a locked comment are
  `invalid`/`unsupported` before the edit.
- **`annotation.info`** (`{"action":"annotation.info","info":{"author":"ada",
  "subject":"Review","icon":"help"}}`) applies `Edit::SetAnnotationInfo` with the
  same all-or-nothing discipline: all 14 note icons by name, `author`/`subject`
  for any comment, an icon only for a sticky note. A string sets the property,
  `null` removes it (`/T`, `/Subj`), an empty string is refused so a request
  never wipes a name by accident; the accepted operation carries `info` and
  `before` (previous values of exactly the named fields, `null` for one the
  comment did not have), and the undo is `info = before`, `expect = info`.
- **The engine has the same guarantees.** `pdfcraft_annot::set_info` now puts the
  dictionary back when a note's appearance cannot be redrawn (it used to leave
  `/T`, `/Subj` or `/Name` changed), and an empty author or subject removes the
  entry instead of writing an empty string, so the undo of a name the comment
  did not have is expressible on the wire.
- **Comment Properties ▸ OK still undoes as one step.** A batch of
  `StyleAnnotation`/`FillAnnotation`/`BorderColorAnnotation`/
  `BorderShapeAnnotation`/`FontSizeAnnotation`/`SetAnnotationInfo` on one comment
  becomes one operation per kind (`annotation.update` →
  `annotation.font_size` → `annotation.info`), in one history group; a batch
  with anything else is still refused whole. A single style edit and an unfilled
  `SetAnnotationInfo` are refused on the client too, so nothing the room would
  reject is sent.
- Refusals do not move the version, write no journal line and are answered only
  to the sender; a stranger's later change to the same property makes the undo
  `conflict` (`retryable: false`) and leaves their value in place.

Tests:

| Crate | Test |
|---|---|
| `collab-annot` | `a_style_update_round_trips_and_says_what_it_clears` (endings in the payload and the engine restyle), `line_endings_are_named_and_counted`, `a_style_update_refuses_a_property_the_comment_does_not_have` (one ending for a callout, two for a line, none for a note), `before_records_only_what_the_update_names_and_the_undo_checks_it` (endings CAS), `a_font_size_update_round_trips_and_is_bounded`, `an_info_update_round_trips_and_refuses_an_empty_name` (null clears, empty is refused, icon is a note's) |
| `collab-pdf` | `an_update_sets_line_endings_with_the_rest_of_the_look` (before, CAS conflict, replay on a fresh replica), `endings_on_a_comment_that_has_none_change_nothing` (version unmoved, colour unchanged), `a_font_size_update_resizes_text_and_undoes_as_one_operation` (before, replay, stale undo refused without a change), `an_info_update_sets_the_note_icon_and_the_names_or_changes_nothing` (before with `null`, empty subject and an icon on a rectangle refused, undo removes the subject again, replay) |
| `collab-client-pdf` | `a_style_edit_the_comment_cannot_carry_is_refused_on_this_side_too`, `line_endings_travel_with_the_style_and_their_undo_names_the_pair`, `a_font_size_and_an_info_change_become_operations_with_cas_inverses`, `a_mixed_properties_batch_becomes_one_operation_per_kind` (order, shared target, mixed batch refused) |
| `collab-ws` e2e | `two_clients_change_line_endings_and_undo_them`, `two_clients_resize_text_and_rename_a_note_and_undo_them` (font size and info, each with the undo from the accepted operation on both replicas), `a_refused_style_detail_and_a_stale_undo_change_nothing` (four refusals, version unmoved; a stale size undo is `conflict` and the other user's 22 pt stays) |

Gates at `c400147b3` (host `agents`, rustc 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **195**: proto 12, server 27 + bin 31, hub 5, client 13, client-pdf 23, session 5, ws 8 unit, pdf 34, annot 18, daemon 3 (`conflict_resolve` 1, `crash_recovery` 1, `publication_retry` 1) + **e2e 16** |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **16/16** (two new) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass — 50 crates, no violations |

What this does **not** cover: the full `ui-egui` suite was not re-run (the only
UI change is none — the new operations are reached through the existing
`Edit::FontSizeAnnotation`/`Edit::SetAnnotationInfo` arms of `ops_for_edit`;
clippy with the feature compiles it); and nothing was run against a real cabinet
or in a browser.

### Part opacity, a move and a comment's geometry — §5 p.2 / limitation 2 (2026-11-10, local; not pushed)

`22d29d070` on `be5ac3570`. This closes the rest of limitation 2: the three
Comment Properties edits the engine applies with its own `Edit`s are now
addressable operations with a compare-and-set undo, so a batch that carries
them is no longer refused whole.

- **`annotation.parts`** (`{"action":"annotation.parts","parts":{"fill":0.4,
  "text":0.8},"expect":{"fill":1.0}}`) sets a shape's or text box's fill
  opacity and a text box's, typewriter text's or callout's text opacity — the
  final numbers, 0–1, in one operation over `Edit::PartOpacity`
  (`pdfcraft_annot::set_part_opacity`). `annotation.update` still sets the
  *whole* comment's opacity; a part's inherits it, and an undo writes the
  numbers back (the engine drops the key again when the value equals the
  parent's, so the drawn look is the same). A note or line with a fill opacity,
  a shape with a text opacity, a locked comment or an appearance that can't be
  redrawn is refused before anything changes. The accepted operation carries
  `parts` and `before`; undo is `parts = before`, `expect = parts`.
- **`annotation.move`** (`{"action":"annotation.move","dx":12,"dy":-4,
  "expect":[10,10,80,40]}`) moves a comment, its pop-up and its points by an
  offset over `Edit::MoveAnnotation`; the accepted operation carries `before`
  and `after` (the `/Rect` the engine read and wrote), because three decimals
  and a sum are not the same number. `expect` is the undo's compare-and-set on
  `/Rect`. Text markup (`Highlight`/`Underline`/`StrikeOut`/`Squiggly`) and
  the caret of a Replace Text proposal stay on their text, and a locked
  comment stays put — both `unsupported` before mutation.
- **`annotation.geometry`** (`{"geometry":{"rect":[…]}}` or
  `{"geometry":{"points":[[x,y],…]}}`) is a resize (`Edit::ResizeAnnotation`,
  the frame Comment Properties names: a callout's text box, else `/Rect`) or a
  reshape (`Edit::ReshapeAnnotation`: a line's ends, a polygon's corners, a
  callout's arrow point and knee). A rectangle is normalised at parse time and
  must be at least 1 pt on a side; points are bounded and counted by kind (a
  line two, a polyline/polygon at least two, a callout one or two). Only
  `Square`, `Circle`, `FreeText`, `Stamp` and `Ink` resize (a callout without
  a readable `/RD` does not), and a measurement (`/IT … Dimension`) keeps its
  points. The accepted operation carries `geometry`, `before` and `after` (what
  the engine drew around the edit), so the undo of a circle's or a callout's
  resize names the frame the engine actually produced, not the client's
  intent.
- **A batch from the interface is one group per comment.** `Comment Properties
  ▸ OK` and a group drag go through `annotation_property_ops`, which now folds
  style, parts, font size, info, moves and geometry into one operation per
  kind and one group per addressed comment, in that order. Every group is
  checked before any operation is sent, so an unsupported edit still refuses
  the whole batch; a batch over several selected comments is no longer refused
  (it used to require one comment). The history group still undoes the batch as
  one step.
- Refusals do not move the version, write no journal line and are answered only
  to the sender; a stranger's later change to the same parts, place or
  geometry makes the undo `conflict` (`retryable: false`) and leaves their
  value in place. The three new kinds are `merge`-class and never move `epoch`.
- The engine side needed no new edit: `Props` now reports the comment's
  `/Rect`, the frame a resize names and its points (`crates/annot/src/info.rs`),
  and `collab-annot` gained `PartsUpdate`, `MoveUpdate`, `GeometryUpdate` with
  the usual `check_for`/`before`/`still_matches`; the adapter and the client
  projection map them to the engine's edits and build the undo from the
  accepted operation (`inverse_after`), like a restyle.

Tests:

| Crate | Test |
|---|---|
| `collab-annot` | `a_parts_update_round_trips_and_refuses_a_part_the_comment_has_not` (0–1, unknown part, a rectangle's text, the before and the CAS), `a_geometry_update_round_trips_a_rect_and_refuses_a_bad_one` (corner order, under 1 pt, non-finite, a note and a callout without a text box), `a_geometry_update_round_trips_points_and_a_measurement_keeps_its_own` (line/polygon/callout counts, a Dimension, the CAS), `a_move_round_trips_and_keeps_text_markup_in_place` (dx/dy, the `/Rect` before and after, markup and a Replace caret refused) |
| `collab-pdf` | `a_parts_update_sets_the_fill_and_text_opacities_and_undoes_as_one_operation` (before, replay on a fresh replica, a rectangle's text refused with the version unmoved, a stale undo a conflict), `a_move_and_a_resize_are_addressable_and_undo_with_a_compare_and_set` (before/after, replay, a stranger's drag blocks the undo, the resize undone from `before`/`after`), `a_reshape_sets_the_points_of_a_line_and_refuses_a_comment_without_them` (points, a rectangle refused, a stale undo, replay) |
| `collab-client-pdf` | `a_mixed_properties_batch_becomes_one_operation_per_kind` (style → parts → font size → info → move → geometry; a two-comment batch is one group per comment; a style + pop-up group; a batch with `LockAnnotation` refused whole), `a_parts_update_and_a_move_round_trip_two_replicas` (accepted op applied on both, undo applied on both, a note's resize builds no operation), `a_resize_and_a_reshape_round_trip_two_replicas` |
| `collab-ws` e2e | `two_clients_change_parts_and_move_a_comment_and_undo_them` (both replicas, the undo's `expect` from the accepted op, a note's resize and a text box's points refused with the version unmoved), `two_clients_resize_and_reshape_a_comment_and_undo_them` |

Gates at `22d29d070` (host `agents`, rustc 1.99.1, `-j 4`/`-j 8`/`-j 12` per
step):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **250**: proto 12, server 29 + bin 34, hub 5, client 13, client-pdf 37, session 5, ws 14 unit, pdf 50, annot 22, daemon 3 (`conflict_resolve` 1, `crash_recovery` 1, `publication_retry` 1) + **e2e 26** |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **26/26** (two new) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass — 50 crates, no violations |

What this does **not** cover: the full `ui-egui` suite was not re-run (no UI
source changed — the dialog's `PartOpacity` arm and the drag's
`MoveAnnotation`/`ResizeAnnotation`/`ReshapeAnnotation` were already in
`ops_for_edit`'s reach; clippy with the feature compiles them); a callout's
resize is the frame the interface names, and a comment whose `/RD` or
appearance cannot be read is refused rather than guessed; signatures, custom
stamps, redaction marks and
attachments are still refused (the pop-up window was shared afterwards,
`0c57e2271`, below); and nothing was run against a real cabinet or in
a browser.

### A comment's pop-up window — §5 p.2 / limitation 2 (2026-11-10, local; not pushed)

`0c57e2271` on `199076765`. This closes the last edit inside a comment: the
note window's place and open state are an addressable operation now, so the
room refuses no edit of an existing comment as a whole any more.

- **`annotation.popup`** (`{"action":"annotation.popup","popup":{"rect":[80,
  130,280,250],"open":true},"expect":{"open":false}}`) sets the final place
  (`rect`, the `/Rect` of the comment's `/Popup` annotation, corners ordered, at
  least 1 pt on a side) and the open state (`open`, `/Open`) of its own window —
  one addressable `merge`-class operation over `Edit::SetAnnotationPopup`
  (`pdfcraft_annot::set_popup`). A field left out stays as it is. A comment
  without a `/Popup` gets one beside it, so opening a highlight's or a
  rectangle's note window is this operation, not client-only state.
- A text box (it draws no window), a Fill & Sign mark and a reply (`/IRT`,
  shown in its parent's window) are refused `invalid` **before** anything
  changes: no journal line, no version step, and the request is answered only
  to its sender. The comment's lock does not apply — the window's place and
  state are not the comment, as in the engine.
- The accepted operation carries `popup`, `before` (the state the update
  replaced, for exactly the fields it names; `null` when the comment had no
  window) and `after` (what the document reads after the edit: the place a new
  window gets is the engine's). The undo is a normal operation built from the
  accepted one (`inverse_after`), `popup = before`, `expect = after`; a
  stranger's move or open since the edit is `conflict` (`retryable: false`) and
  their place stays. A window the open itself created has no state to restore:
  its undo closes it where it was created, because the engine has no edit that
  removes a `/Popup` — the one approximation of this operation, stated in the
  RFC (§3.1.6).
- `Props` now reports the comment's own `/Popup` (`popup`), a Fill & Sign mark
  (`fill_sign`) and a reply (`reply`), so `check_for` can refuse before mutation
  (`crates/annot/src/info.rs`); `collab-annot` gained `PopupUpdate` (the
  intent) and `PopupState` (the accepted operation's `before`/`after`), with the
  usual `check_for`/`before`/`still_matches`. The client projection maps
  `Edit::SetAnnotationPopup` both alone and inside a Comment Properties group
  (style → parts → font size → info → move → geometry → window) and builds the
  undo from the accepted operation. UI code needed no change: the comment
  tool's close, open and title-bar drag already send `Edit::SetAnnotationPopup`
  (and `one_page_changed` already skips its raster), so the operation is reached
  through the existing edit.

Tests:

| Crate | Test |
|---|---|
| `collab-annot` | `a_popup_update_round_trips_and_refuses_a_comment_without_a_window` (parse/serialize, corner order, under 1 pt, unknown keys, an empty update; a note, a text box, a reply and a Fill & Sign mark; the `before` of only the named fields, and the CAS on both fields, including no window at all) |
| `collab-pdf` | `a_popup_window_is_opened_moved_and_undone_with_a_compare_and_set` (an open with `before`, a move with `before`/`after`, replay on a fresh replica, a window created on a rectangle with `before: null`, a text box refused with the version unmoved and nothing changed, a stranger's drag blocking the undo, the undo of the creation closing it) |
| `collab-client-pdf` | `a_popup_window_round_trips_two_replicas` (the accepted open and move applied on both replicas, the undo from `inverse_after` on both, the creation closed on both, a text box building no operation and leaving the other replica untouched), `a_mixed_properties_batch_becomes_one_operation_per_kind` (a style + open group on one comment; the refused batch now uses `LockAnnotation`, which still has no operation) |
| `collab-ws` e2e | `two_clients_open_and_move_a_comment_window_and_undo_it` (both replicas converge on the open and on the move, the undo's `expect` and `popup` from the accepted operation, a text box's window refused with the version unmoved) |

Gates at `0c57e2271` (host `agents`, rustc 1.99.1, `-j 12` for the compile
steps):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list, plus `-p pdfcraft-annot` (the crate whose `Props` grew) | pass — **448** (annot 194, proto 12, server 29 + bin 34, hub 5, client 13, client-pdf 38, session 5, ws 14 unit, pdf 51, collab-annot 23, daemon 3 + e2e 27) |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **27/27** (one new) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass — 50 crates, no violations |

What this does **not** cover: the full `ui-egui` suite was not re-run (no UI
source changed; clippy with the feature compiles it and the edit was already on
the comment tool's path); `cargo xtask assets` and the wasm check were not run
in this pass (no asset and no new dependency); a window's rectangle travels as
`f32` in `Props`, so an undo can differ from the original `/Rect` by well under
0.001 pt; and nothing was run against a real cabinet or in a browser.

### Page insert and duplicate — §5 p.3 (2026-11-09, local; not pushed)

`b13933e9b` on `d01adcad4`. Structural coverage was rotate, move and delete
(§5 p.3). Now:

- `page.insert` (target `Target::Page` at the insertion position in the current
  epoch, `index ≤ page count`) and `page.duplicate` (target is the source page;
  the copy goes at the payload's `at`, or right after the source when it names
  none) carry the page as a one-page PDF inside the operation: `payload.bytes`
  (base64) and `payload.at`. Both apply through `Edit::InsertPagesFrom`, bump
  the epoch and are journaled with the copy, so every replica and a restart
  insert the same bytes without a second channel.
- The copy is bounded by `MAX_RESTORE_PAGE_BYTES` (160 KiB, now
  `pdfcraft_collab_proto::MAX_PAGE_COPY_BYTES` so the client uses the same
  number). A page over it is refused with `limit` **before** `apply_edit`: no
  partial insert, no epoch bump, no journal entry. The client refuses the same
  page in `ops_for_edit` (`ProjectionError::TooLarge`) and in `edit_for`, so the
  edit is never applied locally either.
- A foreign epoch on the address is `stale` and retryable (`insert_position`
  and `page_index`), so the client resyncs and re-addresses the page; a merge
  edit someone else made to the source page does not move the address.
- Undo: `inverse_for` turns an insert or a duplicate into a `page.delete` of the
  page it put there, so the page count returns to what it was; the redo is the
  original operation.
- Client projection: `edit_for` applies both actions; `ops_for_edit` builds them
  from `Edit::InsertPagesFrom` (one operation per page, each cut with the
  engine's own one-page copy) and `Edit::DuplicatePages` (copies cut from the
  document as it stands); `insert_page_payload` / `duplicate_page_payload` are
  the wire helpers.

Gates at `b13933e9b` (host `agents`, rustc 1.99.1, `-j 8`/`-j 10`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, whole workspace | `cargo clippy --all-targets -- -D warnings` | pass |
| Tests, the three named packages | `cargo test -p pdfcraft-collab-pdf -p pdfcraft-collab-ws -p pdfcraft-collab-session` | pass — pdf 26, ws 6 unit + 9 e2e, session 5 |
| Tests, client projection | `cargo test -p pdfcraft-collab-client-pdf` | pass — 15 |
| Tests, the collab list | proto 12, server 22, hub 5, client 12, client-pdf 15, session 5, ws 6 + 9 e2e, pdf 26, annot 10, daemon 28 + 1 crash | pass — **151** |
| wasm, client projection | `cargo check --target wasm32-unknown-unknown -p pdfcraft-collab-client-pdf` | pass |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |

New tests:

- `collab-pdf` — `an_inserted_page_lands_at_its_address` (the copy lands at 0,
  the original moves to 1, the committed PDF reads "Page 3" then "Page 1"),
  `a_duplicate_lands_at_the_position_it_names`,
  `a_page_over_the_limit_is_refused_before_the_document_changes` (`limit`; page
  count, epoch and version unchanged),
  `an_insert_addressed_to_another_epoch_is_retryable` (`stale`, retryable),
  `the_undo_of_an_insert_returns_the_original_page_count`, and
  `an_insert_replays_from_its_operation` (a fresh adapter replays the journal).
- `collab-client-pdf` —
  `an_insert_and_a_duplicate_become_addressable_operations` (payload, address,
  the inverse is a `page.delete`, and the replica applies both) and
  `a_page_over_the_limit_is_refused_on_this_side_too`.
- `collab-ws` e2e — `two_clients_insert_and_duplicate_a_page_and_converge`: two
  clients over a socket; Bo comments on page 0, Ada inserts a copy of page 1 at
  0 (both replicas read "Page 2" at 0 and "Page 1" at 1), the undo returns the
  page count, Ada duplicates page 0 (both read "Page 1" at 1), and an insert
  addressed to epoch 0 after the epoch moved is `stale` and retryable with
  nothing changed.

What this does **not** cover: split/combine, and duplicating a page larger than
the budget (refused, not shared). Replace and blank insert were closed later by
`e9756d138` (§4, RFC §3.1.4). Nothing was pushed: the fork and PR #6622 still
describe `81035bf2`.

### Forced journal commit — §6.1 limitation 10 (2026-11-09, local; not pushed)

`8fc898bae` on `a4c852fc6`. A room with connected clients used to grow its
journal until the last one left; the last-leave commit and the manual button
were the only compaction (limitation 10). Now:

- `Room` keeps `base_version` (the base the journal sits on) and
  `journal_bytes` (the serialised size of the records `Adapter::record` wrote,
  the same fields the durable `JournalRecord` has, so the sum matches the file)
  and exposes `Room::forced_commit_due`. `RoomConfig` gains
  `forced_commit_ops` (default 2000) and `forced_commit_bytes` (default 8 MiB);
  `0` disables either threshold. `replay` counts the entries it restores, so a
  restart with a large journal is due immediately; a refreshed base resets both
  counters in `write_commit`.
- The transport (`collab-ws`) calls `Shared::queue_forced` after every client
  message and after a join. It calls `Hub::forced_commit`, which writes the base
  with `Room::commit_now` **under the hub lock** and returns a `Pending`; the
  cabinet POST runs in `run_pending` **outside** the lock, like any commit. A
  second commit while one publishes is `busy` (retryable) and queues nothing:
  the threshold stays crossed, so the next message or the end of that
  publication (`Hub::finish` → `queue_forced`) retries it. If the last client
  left while a publication ran, `finish` now runs the last-leave
  `commit_if_dirty`, so that commit is not lost either.
- A forced commit refused by the cabinet as `409` sets the room's `conflict`
  flag through `Room::finish`, exactly like a manual commit; `forced_commit_due`
  is then false and the room keeps the journal for an operator. Retrying the
  same `expectedVersion` cannot succeed, so this is not a retry loop.
- The connection loop flushes the outbox before it runs a publication: the
  author sees its operation acknowledged before the cabinet call the operation
  triggered, and only that connection waits for the cabinet. Other rooms and
  other clients in the same room are unaffected.
- `/metrics` gains `pdfcraft_collab_forced_commits_total`
  (`Metrics::forced_commits`, a process counter incremented when a compaction
  writes its base). `/health` is unchanged.
- A publication that fails retryably used to leave the room dirty until the
  next commit (limitation 6). It now stays `publishing` and is retried by the
  daemon's queue (§4); a non-retryable failure still leaves the room dirty for
  the next commit.

What is not covered: a time-based threshold for the *journal*. A room that
becomes dirty when a non-retryable publication fails and then sees no further
edit waits for the next edit, a join, or the last-client-leave commit. The base
write itself (serialising the PDF) is under the hub lock, like a client commit;
only the cabinet POST is outside.

Gates at `8fc898bae` (host `agents`, toolchain 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **142** (134 without the e2e suite): proto 12, **server 22**, **hub 5**, client 12, client-pdf 13, session 5, ws 6 unit, pdf 20, annot 10, **daemon 28** + 1 (`tests/crash_recovery.rs`) |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **8/8** |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| wasm, session + typed client | `cargo check --target wasm32-unknown-unknown -p pdfcraft-collab-session -p pdfcraft-collab-client-pdf` | pass |

New tests:

- `collab-server` — `the_journal_threshold_turns_into_a_server_commit_and_resets`
  (two operations cross the threshold, `commit_now` clears it, the next two
  cross it again), `the_byte_threshold_counts_what_the_adapter_writes`,
  `a_room_in_conflict_does_not_force_commits_but_keeps_the_journal`,
  `a_commit_in_flight_makes_the_threshold_wait_for_the_next_call` (the `busy`
  answer is retried and succeeds after the publication is reported).
- `collab-hub` — `an_accepted_operation_compacts_the_journal_at_the_threshold`
  (the pending commit names the document; a second `forced_commit` while it is
  publishing queues nothing; after `finish` the room is clean).
- `collab-ws` e2e — `a_forced_commit_does_not_block_another_room_and_is_counted`:
  room A crosses the threshold on one edit, its cabinet POST blocks on a gate,
  the author's `applied` still arrives, room B accepts an edit, and
  `metrics().forced_commits` is 1; after the gate opens room A keeps editing.
- `collab-ws` e2e — `a_failed_forced_publication_is_not_retried_in_a_loop`: a
  cabinet that never answers fails the forced commit and the last-leave retry
  exactly once each (two publications), and the transport does not spin.
- daemon — `a_forced_commit_stops_the_journal_from_growing`: a seeded room with
  `forced_commit_ops: 3` accepts ten operations; the journal is truncated after
  each forced commit, never grows past three records (compared with the size of
  one remaining line), and one record (version 10) is left after the base at
  version 9. The production thresholds are larger; the test uses three.

### Idle forced commit — §6.1 limitation 10, the time threshold (2026-11-09, local; not pushed)

`2a3b6e43e` on `171cbd0fb`. The operation and byte thresholds
(`8fc898bae`, above) only fire when an operation or a join arrives. A comment
written in a quiet room stays in the journal until somebody acts, so the time
threshold closes that gap:

- **`RoomConfig::forced_commit_idle_secs`** (default 300, `0` disables it) sits
  next to `forced_commit_ops` / `forced_commit_bytes`. **Idle** is defined as
  the time since the newest journaled operation: `version > base_version` (the
  journal holds work the base does not) and
  `now - last_activity_ms ≥ forced_commit_idle_secs`. It is room state, not
  connection state: a reconnect or a new join does not reset the clock, and a
  base written by any commit empties the journal and disarms it. A dirty room
  whose base is already written but unpublished is *not* idle-eligible: the
  journal is empty, and republishing that base is the queued publication's path,
  not a compaction.
- **It survives a restart.** `JournalRecord` gained `ms` (the acceptance time;
  `#[serde(default)]`, so an older journal reads `0`), `Adapter::record` takes
  that clock from `Room::apply`, and `Room::replay` re-arms `last_activity_ms`
  from the newest record. An old record without a timestamp arms nothing — the
  next edit starts the clock — rather than guessing a time.
- **The transport sweeps.** `Hub::documents` + `WsServer::sweep_forced_commits(now_ms)`
  walks every room under the hub lock, queues the compactions that are due and
  runs their publications outside the lock. The daemon's background loop
  (`WsServer::spawn_publication_retries`, 100 ms) calls it after the publication
  queue, because a quiet room sends no message the transport could react to.
- **The same commit path and the same counter.** A sweep compaction is
  `Room::commit_now` under the hub lock plus a publication outside it, exactly
  like the operation/byte threshold; it increments
  `pdfcraft_collab_forced_commits_total`, and a `409` flags the room `conflict`
  and stops further forced commits until the operator resolves it (§4, `62d48d30a`).
  A `busy` room (a publication in flight) queues nothing; the threshold stays
  crossed and the next sweep retries.

Tests:

| Crate | Test |
|---|---|
| `collab-server` | `an_idle_journal_compacts_without_a_new_edit_and_a_reconnect_keeps_its_clock` (one operation below both counters; not due at 119 999 ms, not due after a disconnect + rejoin at 60 s, due at 120 s; the commit disarms it; `0` disables the clock), `a_replayed_journal_carries_its_idle_clock_across_a_restart` (an entry recorded at 5 s is due at 125 s but not at 124 999 ms; a record with `ms = 0` arms nothing) |
| `collab-pdf` | `a_base_and_the_journal_survive_reopening` now asserts the stored `ms` of both records |
| `collab-ws` e2e | `an_idle_room_compacts_its_journal_without_another_edit`: one edit, far below the counters, then no message at all; a sweep with a later clock compacts it, the publication runs once, `metrics().forced_commits` is 1 and a second sweep queues nothing. `the_background_loop_compacts_an_idle_room`: the daemon's loop, started at 20 ms, compacts the room on its own with exactly one publication (1 s idle) |

Gates at `2a3b6e43e` (host `agents`, rustc 1.99.1; no `-j` flag, so the host's
`[build] jobs = 4` applied):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates touched + daemon | `cargo clippy -p pdfcraft-collab-{server,hub,ws,pdf} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the nine-crate list | pass — **213**: proto 12, server 29, hub 5, client 13, client-pdf 28, session 5, ws 8 (unit) + 20 e2e, pdf 41, annot 18, daemon 31 + `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1 |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **20/20** |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass — 50 crates, no violations |

What this does **not** cover: the idle clock starts from the journal, so a
room that is dirty only because its base was written and never published is
still republished by the next commit, join or leave (the queued publication,
§4), not by this threshold; a kill between the base write and the `CURRENT`
rename stays covered at the `Store` level only (`Store::tests`, §5 p.1) — the
real-process SIGKILL test (`tests/crash_recovery.rs`, `999da5ec`) parks in the
journal→base window, and that window remains the only one with process-level
coverage. The idle threshold was not exercised against a real cabinet.

### Blob channel — decision (2026-11-09, no code)

RFC §16 and §4.1 leave the blob channel open: every page copy (insert,
duplicate, replace, restore) travels as base64 inside the operation, capped at
`MAX_RESTORE_PAGE_BYTES` = 160 KiB, inside a 256 KiB frame. **Decision: C — the
blob channel is a separate PR after this one merges.** The channel is a protocol
addition (a blob message, a server-side store with quota and GC, a client upload
path) that needs its own design and ADR; it would enlarge an already wide PR and
cannot be reviewed against the same base. Until then the 160 KiB cap is an
accepted, tested limit: a larger page is refused with `limit` before the
document changes, and a larger replacement is applied with no copy, so its undo
is a `conflict` (§5 p.3, RFC §3.1.4). Nothing was raised and no constant changed.

### Web host bridge — §6.3 (2026-11-09, local; not pushed)

Code: `81a92f32f` (the slice) on top of `018c9840`. The branch is local-only
after that commit: nothing was pushed, so PR #6622 still describes `81035bf2`.

What changed:

- `crates/collab-session` is split into `driver` (the protocol half: hello and
  welcome, version/epoch, resync and retry, the 30 s lease heartbeat, the
  re-address of a retry under a snapshot), `transport` (the `Transport` trait and
  `ReadOutcome`), `native` (the blocking `tungstenite` socket on its worker
  thread) and `web` (the browser's `WebSocket`, callbacks into a queue, redial
  with a doubling backoff capped at 10 s, no timers and no cycle).
- `Sink` is the object-safe facade (`enqueue`, `presence`, `commit`, `resync`,
  `try_event`, `close`); `open(config)` returns the platform's one. `ui-egui`
  holds `Box<dyn Sink>`, so the participants window and the edit path compile on
  wasm.
- The desktop keeps its behaviour: the worker thread, the 20 ms read timeout,
  reconnect delay, merge-only resend. A command made while the socket is down
  waits in the driver (`retry`); a frame the socket has not taken yet (still
  dialing) waits in the driver's outbox instead of being dropped.
- `apps/pdfcraft-web` carries the messages of INTEGRATION.md §6:
  `pdfcraft:session` (queued for the frame that has the document open),
  `pdfcraft:session-state` on a change of status/participants,
  `pdfcraft:presence` coalesced at ≤10 Hz, `pdfcraft:commit` when the cabinet
  owns the commit (no bytes), `pdfcraft:committed` back. The document a session
  joins is the token's `fileId` claim (what the server checks against
  `hello.doc`), falling back to the host's session id.
- `craft-web-host` is pinned to `copoka/craft-libs` `7666e2b1` (the branch with
  the collaboration messages) for that crate only — ADR-0016 §3; `craft-web-api`
  stays on `storytold/craft-libs` `fd5e2fe`. Feature `collab` in
  `pdfcraft-web` is off by default: `trunk build --release --features collab`.
- §5 loses the "no web client" item and the rest is renumbered: old 3–13 are
  now 2–12. Every cross-reference moved with it — the publication-queue note in
  item 1 and §6.1 (old 7 → 6), the journal-growth note in §6.1 (old 11 → 10),
  the TLS note in §6.4 (old 7–8 → 6–7), and the header and §6.6 (old 12 → 11).
  RFC §16 item 2 says the same as this slice.

Gates at `81a92f32f` (toolchain 1.99.1, host `agents`, `-j 8`/`-j 10`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Clippy, wasm session | `cargo clippy --target wasm32-unknown-unknown -p pdfcraft-collab-session --all-targets -- -D warnings` | pass (the `web` transport is only compiled there) |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — 128 (134 with the e2e suite): proto 12, server 18, hub 4, client 12, client-pdf 13, **session 5**, ws 6 (unit), pdf 20, annot 10, daemon 27 + 1 (`tests/crash_recovery.rs`) |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, 6/6 |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass (50 crates) |
| wasm, every crate below L8 | `cargo xtask wasm` | pass, 42 crates |
| wasm, session + typed client | `cargo check --target wasm32-unknown-unknown -p pdfcraft-collab-session -p pdfcraft-collab-client-pdf` | pass |
| wasm, UI with the feature | `cargo check --target wasm32-unknown-unknown -p pdfcraft-ui-egui --features collab` | pass (11 pre-existing dead-code warnings from the wasm build of `ui-egui`, none from `collab.rs`) |
| wasm, web shell with the bridge | `cargo check --target wasm32-unknown-unknown -p pdfcraft-web --features collab` | pass — the pinned `craft-web-host` and the five messages resolve |

New tests in this slice: `collab-session` `the_driver_waits_for_the_socket_to_open_and_greets_once`
(a socket that is still dialing gets nothing and loses nothing; exactly one
hello per connection), `an_unreadable_frame_is_a_notice_not_a_disconnect`,
`an_operation_queued_offline_goes_out_after_the_welcome` (the offline queue and
the welcome path), plus the two worker tests moved unchanged; `ui-egui`
`the_document_comes_from_the_tokens_file_claim`,
`an_unreadable_token_falls_back_to_the_session_id`, `a_role_name_round_trips`.
The full `ui-egui` suite was not re-run for this slice: the only UI change is
the transport type and the host reports, and clippy plus the three new tests are
green on it; the last full run is §6.6 below.

What this does **not** cover: nothing here was run in a browser. The host bridge
is compile-checked against the pinned `craft-lib` and exercised only in the
native driver tests; a two-iframe run (the cabinet sends `session`, both panes
edit, `commit` out and `committed` back) needs the cabinet's page and the
`craft-web-host` API from `craft-libs`, and stays open. The live desktop smoke
test (daemon + `xvfb-run pdfcraft --features collab`) was not repeated either:
the worker path is unchanged and its unit tests cover the driver it now uses.

### Rebase onto `origin/main` `8d5d39ef` and the gate run at the rebased head (2026-11-09)

The branch was 40 commits ahead of `43f24aa1` (the `origin/main` it had been
rebased onto on 2026-11-08) and the fork head `333072e8` made PR #6622
`CONFLICTING`. `git rebase origin/main` moved all 40 commits onto `8d5d39ef`
("Combine files: a drop is one undo step, in order, and names what it can't
take", #6645) with **no conflict**.

Main changed 16 files in `43f24aa1..8d5d39ef`; the only one also touched by the
branch is `crates/ui-egui/src/app.rs`. Main rewrote the drop loop there
(`drop_into_combine(dropped, ctx)` takes the whole batch, `#6645`); the branch
adds the `collab`/`collab_off` fields in `new()` and the
`poll_collab`/`show_collab` calls in `update`/`ui` (commit `397c539c`). The
hunks do not overlap, so git kept both. Same check as CADCraft's stack rebuild
(§4.1.1 of its STATUS): `git diff backup/pre-rebase-7628b605..HEAD` is byte for
byte `git diff 43f24aa1..origin/main`, i.e. the rebase moved the base and
changed no feature content. PdfCraft has no `i18n/*.tsv` catalog (UI strings
live in `ui-egui` code, with `status_captions` for translated captions), so the
i18n half of CADCraft's conflict pattern has no counterpart here.

Backup of the pre-rebase head: `backup/pre-rebase-7628b605` (= `7628b605`).
Rebased head before the STATUS commit: `faad6b79`.

Gates re-run at that head (toolchain 1.99.1; the machine was under heavy load —
load average 25–30 on 4 cores from the other projects' builds, hence the
17 minutes for `cargo xtask layers`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass on re-run; the first attempt was killed by SIGTERM to its `clippy-driver` children after 4 s (machine memory/build pressure from the parallel builds), not by a diagnostic — no error line was emitted |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — 125 tests: proto 12, server 18, hub 4, client 12, client-pdf 13, session 2, ws 6 (unit), pdf 20, annot 10, daemon 27 (unit) + 1 (`tests/crash_recovery.rs`) |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, 6/6 |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass |
| UI suite | `cargo test -p pdfcraft-ui-egui --features collab --no-fail-fast` | 2440 passed, 1 failed, 5 ignored: `--lib` 840 passed/1 ignored; `it` 1582 passed and the known font failure `editing::a_custom_property_whose_name_the_tools_refuse_is_listed_and_left_alone` (`No glyph for "�" (U+FFFD)`); `ocr` 6, `page_shadows` 7, `perf` 4, `play` 1 |
| wasm, client crate | `cargo check --target wasm32-unknown-unknown -p pdfcraft-collab-client-pdf` | pass |

The single `it` failure is environment, not this branch: the test comes from
`origin/main` (last touched by `ae7a21d2`, not by the branch, which only edits
`crates/ui-egui/src/editing.rs` for remote edits), it panics in egui's font
code because the machine's font set lacks a glyph for U+FFFD, and it failed the
same way before the rebase and without the `collab` feature (the
`5eb6bea8` run below). The parallel-load flake
`control::transport::transport_tests::only_so_many_connections_may_wait_to_authenticate`
passed in this run.

Mergeability is now observed, not expected: after the force-push `gh pr view
6622` reports head `81035bf2` (the pushed head; the local branch is that head
plus this STATUS commit, which is not pushed), `mergeable: MERGEABLE`,
`mergeStateStatus: UNSTABLE` with the single reported check CodeRabbit
`SUCCESS` at 2026-11-09T00:01:35Z and no required review;
`git ls-remote fork` names the same `81035bf2`, `git rev-list --count
origin/main..HEAD` is 0 behind, and
`git merge-tree --write-tree origin/main HEAD` exits 0.

### Rebase onto `origin/main` `ca9e3c2c3` (the requested `cde331fb7`) and the gate run at the rebased head (2026-11-09)

The pushed head `81035bf2` had been rebased onto `8d5d39ef`; `main` then moved
on, and the target named for this session, `cde331fb7`, was already behind it.
`origin/main` was fetched and was `ca9e3c2c3` ("Merge pull request #6666 from
storytold/merge/redact-forms-misc", 2026-11-09 11:50 +0800), with `cde331fb7`
an ancestor. `git rebase origin/main` moved all 47 commits — the 40 from the
`8d5d39ef` rebase plus the seven web-bridge, forced-commit and docs commits —
onto the current `origin/main` with **one conflict**.

`main` changed 371 files in `8d5d39ef..ca9e3c2c3` (37 181 insertions,
2 382 deletions). Ten of them are also touched by the branch:

| File | Merge |
|---|---|
| `Cargo.lock` | automatic |
| `crates/annot/src/modify.rs` | automatic |
| `crates/engine/src/document.rs` | automatic |
| `crates/engine/src/edit/run.rs` | automatic |
| `crates/engine/src/history.rs` | automatic |
| `crates/engine/src/session/editing.rs` | automatic |
| `crates/engine/src/tests.rs` | **conflict**, resolved by hand |
| `crates/ui-egui/src/app.rs` | automatic |
| `crates/ui-egui/src/canvas.rs` | automatic |
| `crates/ui-egui/src/lib.rs` | automatic |

The conflict is in `78d7b7d5c` "feat(engine): expose page and annotation
references for collaboration". Both sides appended tests at the end of
`crates/engine/src/tests.rs`: main added
`a_document_encrypting_only_its_embedded_files_opens_and_saves_without_a_password`,
the branch added `page_and_comment_references_survive_comment_edits` and
`remote_edits_drop_the_local_snapshot_history`. The resolution keeps main's test
first and both branch tests after it;
`git diff origin/main..HEAD -- crates/engine/src/tests.rs` is exactly those two
test functions and nothing else.

The rebase moved the base and changed no feature content:
`git diff --name-only backup/pre-rebase-2529c8c2..HEAD` names the same 371 files
as `git diff --name-only 8d5d39ef..origin/main`, with the same 37 181/2 382 line
counts; each of the nine auto-merged files is byte-identical to a plain
`git merge-file` of base/old-branch/`origin/main`; every file the branch changes
that main does not is byte-identical to the old tip, and every file main changes
that the branch does not is byte-identical to `origin/main`. The pre-rebase head
is kept as `backup/pre-rebase-2529c8c2` (`2529c8c2`); the rebased head before
this STATUS commit is `0e839a4b1`. The hashes used in the header and below are
pre-rebase; the current commits are `81a92f32f`→`0d3d9e11e`,
`8fc898bae`→`d8ade8648`, `999da5ec`→`d3e62ecf6`, `a47641fb`→`b5a67b098`,
`2529c8c2f`→`0e839a4b1` (and `81035bf2` stays the pushed fork head).

Gates at the rebased head (host `agents`, rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **142**: proto 12, server 22, hub 5, client 12, client-pdf 13, session 5, ws 6 (unit), pdf 20, annot 10, daemon 28 + 1 (`tests/crash_recovery.rs`) + the 8 e2e below |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **8/8** (run inside the `cargo test` above) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass — 50 crates, no violations |
| Mergeability | `git merge-tree --write-tree origin/main HEAD` | exit 0, no conflicted paths |

The branch is 47 commits ahead of `ca9e3c2c3` and 0 behind. Nothing was pushed:
the fork, PR #6622 and its body still describe `81035bf2`, so the PR stays
`CONFLICTING` until the owner force-pushes the rebased branch. The UI suite was
not re-run for this rebase; clippy with `--features collab` and
`cargo check -p pdfcraft --features collab` are green on the merged
`ui-egui` files, and the last full UI run is §6.6 below.

### Rebase onto `origin/main` `b6cbf9bb3` and the gate run at the rebased head (2026-11-09)

The pushed head `4eea573be` (PR #6622) had been rebased onto `ca9e3c2c3`; `main`
then moved on by **307 commits** to `b6cbf9bb3` ("Merge pull request #6710 from
storytold/merge/forms-links-text2", 2026-11-09 19:45 +0800), and the PR reported
`CONFLICTING`. `git rebase origin/main` moved all **64 branch commits** onto it
with **four conflicts**, each resolved by hand:

| File | Merge |
|---|---|
| `Cargo.lock` | **conflict**: main upgraded `zip` 9.1.0 → 9.1.3, which dropped `zopfli`; the branch's lock still had 9.1.0 + `zopfli`. Kept main's 9.1.3 block, dropped the `zopfli` package, kept the branch's new package entries |
| `crates/annot/src/modify.rs` | **conflict**: both sides added before `set_review_state` — main reworded its doc comment, the branch added `add_reply_named`. Kept main's comment and the branch's function |
| `crates/ui-egui/src/app.rs` | **conflict**: in `App::new()` main replaced the `session` field with `panes: Default::default()`, the branch adds the two `collab` fields. Kept both, collab fields first |
| `crates/engine/src/edit.rs` | **conflict**: the `use crate::{…}` list — main added `Hairlines`, the branch `Restyle` (and their rustfmt wrapping differed). Kept main's list with `Restyle` in alphabetical place |

`main` changed 325 files in `ca9e3c2c3..b6cbf9bb3` (27 562 insertions, 1 950
deletions); 19 of them are also touched by the branch, so 15 merged
automatically and each is byte-identical to a plain `git merge-file` of
base/old-branch/`origin/main`. Every file the branch changes that main does not
is byte-identical to the old tip, except the two the field fix touches
(`crates/collab-client-pdf/src/lib.rs`, `crates/collab-pdf/src/lib.rs`), and
every file main changes that the branch does not is byte-identical to
`origin/main`. The session's brief expected conflicts in
`crates/engine/src/session/editing.rs` (the
`apply`/`prepare_text`/`apply_remote`/`apply_reporting(id, edit, record_undo)`
split) and `crates/engine/src/tests.rs` merged **without** conflicts: main's
changes there sit in different hunks, and both sides' tests survive —
`git diff origin/main..HEAD -- crates/engine/src/tests.rs` is the two branch
tests and nothing else.

One compile break did come out of main's `forms` work and is fixed at the tip
in `e0d200af5` "fix(collab): carry a button's on state and a list pick on the
wire": `FieldValue` gained `Widget(usize)` (pick one button of a group) and
`ChoiceIndices(Vec<usize>)` (pick an option by position), and a check box's
kids each got their own on state. The projection now sends a `Widget` pick as
the widget's on state and a single `ChoiceIndices` pick as that option's export
value; a pick of several options (or of none) is refused with `Unsupported`,
because the v1 wire carries one string and the replicas would otherwise
diverge. The wire's check-box string is now read back as
`FieldValue::Text(text)` on **both** the client (`collab-client-pdf`) and the
adapter (`collab-pdf`), which the engine accepts for `Yes`/`true`/custom
states alike (the old `Check(bool)` mapping read a capitalised or custom
on-state as off), and the inverse of `field.set_value` now names the field's
stored value instead of normalising a check box to `true`/`false`, so the
`expect` compare sees the state the file holds.

The fix sits at the tip rather than inside `281aaaed2`, the commit that
introduced the projection: folding it in with `--autosquash` conflicted,
because later commits rewrote `field_text`; as a separate commit the tree at
`e0d200af5` is what the gates below ran on, while the intermediate commits
before it do not build against the new `main` on their own.

Backup of the pre-rebase head: `backup/pre-rebase-4eea573b` (= `4eea573b`).
Rebased head before the STATUS commit: `e0d200af5` (the 64 replayed commits
plus the field fix). The branch is 65 commits ahead of `b6cbf9bb3` and 0
behind; `git merge-tree --write-tree origin/main HEAD` exits 0 with no
conflicted paths. Nothing was pushed, so the fork, PR #6622 and its body still
describe `81035bf2` and the PR stayed `CONFLICTING`/`DIRTY` until the owner
force-pushed. The body was later replaced with the text of
`pdfcraft/PR-DRAFT.md` at this base (see the re-verification below).

Gates at `e0d200af5` (host `agents`, rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **209**: proto 12, server 27 (unit), hub 5, client 13, client-pdf 28, session 5, ws 8 (unit) + 18 e2e, pdf 41, annot 18, daemon 31 (unit) + `tests/conflict_resolve.rs` 1 + `tests/crash_recovery.rs` 1 + `tests/publication_retry.rs` 1 |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass — 50 crates, no layering violations |
| Mergeability | `git merge-tree --write-tree origin/main HEAD` | exit 0, no conflicted paths |

The test numbers this session was asked to expect (142, e2e 8) are the
`ca9e3c2c3` figures from the previous rebase; the commits added since then
(page insert/duplicate, restyle/status reply, publication queue, presence
selections, conflict resolution, line endings/font size/info, page
replace/blank insert) added their tests. The counts above match the `#[test]`
functions in the tree (proto 12, server 27, hub 5, client 13, client-pdf 28,
session 5, ws 26, pdf 41, annot 18, daemon 34), so no test is duplicated by
the rebase. The UI suite was not re-run for this rebase; the last full run is
§6.6 below, and clippy with `--features collab` plus
`cargo check -p pdfcraft --features collab` are green on the merged files.

Re-verified at `4bcb5b85b` — the docs commit that records the `b6cbf9bb3`
rebase, one docs commit on top of `e0d200af5` — in a later pass on 2026-11-09.
The tree was clean, `git merge-tree --write-tree origin/main HEAD` exited 0 and
wrote tree `0653eecff`, and the §6.6 gates were re-run green on the host
(`agents`, rustc 1.99.1, `-j 12`): `cargo fmt --all -- --check`; clippy for the
nine collab crates plus the daemon and for `pdfcraft-ui-egui --features collab`
with `--all-targets -- -D warnings`; the collaboration suites — **209** tests
in 8 s on the warm target (proto 12, server 27, hub 5, client 13,
client-pdf 28, session 5, ws 8 unit, pdf 41, annot 18, daemon 31 + the
conflict-resolve, crash-recovery and publication-retry tests) — and the 18 e2e
on their own; `cargo check -p pdfcraft --features collab`; `cargo xtask layers`
(50 crates, no violations). The code at `4bcb5b85b` is `e0d200af5`'s plus
nothing, so this is the same tree the earlier table measured, timed again on a
warm target rather than the earlier cold one.

The PR body was then replaced (a body edit, not a push) with the text of
`pdfcraft/PR-DRAFT.md`, updated to the current base: the web host bridge,
presence selections, page insert/duplicate/replace/blank insert, the
publication queue and the conflict-resolution endpoint moved out of the
"Limitations" list, the test section names the 209 tests and the e2e run, and
the body now names `main` `b6cbf9bb3` as the base. Nothing was pushed: PR
#6622 still has the pushed head `4eea573be` and reads `CONFLICTING` / `DIRTY`
until the owner or chair force-pushes this branch.

### Rebase onto `origin/main` `d2139e58a` (the brief's `fe3879bc8`, moved on) and the gate run at the rebased head (2026-11-09)

The pass was asked to rebase onto `origin/main` `fe3879bc8` (#6729) and predicted
conflicts in `crates/annot/src/lib.rs` and `crates/engine/src/lib.rs`. By the
time the tree was fetched, `origin/main` was `d2139e58a` ("Merge pull request
#6716 from storytold/merge/xfa2"), 277 commits past `fe3879bc8`; a branch
rebased onto `fe3879bc8` would not have an empty `merge-tree` against
`origin/main`, so the rebase targeted the current tip. `git merge-tree
--write-tree origin/main HEAD` at the old head named exactly the two predicted
paths; nothing else conflicted.

`git rebase origin/main` moved all **69 branch commits** from `b6cbf9bb3` with
**two conflicts**:

| File | Merge |
|---|---|
| `crates/annot/src/lib.rs` | **conflict** in the `pub use` lists. Main added the callout and custom-stamp names (`CALLOUT_KNEE_LINE`, `CUSTOM_LINE_BAND`, `CUSTOM_LINE_TEXT`, `Side`, `callout_knee`, `callout_knee_on`, `callout_side`, `custom_stamp_size`), `PopupInfo`, `is_group_subordinate`, `release_members` and `set_popup` (the comment-popup, callout-reshape and custom-stamp merges); the branch adds `add_annotation_named`, `add_reply_named`, `set_review_state_named` and `Restyle`. Kept main's lists with the branch's four names in alphabetical place. The same conflict fires twice, in `ebc8d7d55` (the named reply) and `115ccc87c` (the restyle), because each appends to the same lists. |
| `crates/engine/src/lib.rs` | **conflict** in the `pdfcraft_annot` re-export: main added the callout names, the branch adds `Restyle`. Kept main's list with `Restyle` after `Props as CommentProps`. |

`main` changed 126 files in `b6cbf9bb3..d2139e58a` (18 178 insertions, 1 029
deletions); 17 of them are also touched by the branch, so 15 merged
automatically and each is byte-identical to a plain `git merge-file` of
base/old-branch/`origin/main`. Every file the branch changes that main does not
is byte-identical to the old tip (49 files), and every file main changes that
the branch does not is byte-identical to `origin/main` (109 files). The two
hand-resolved files differ from a plain merge exactly by the branch's four and
one names: `git diff origin/main..HEAD -- crates/annot/src/lib.rs
crates/engine/src/lib.rs` is those names and nothing else. The rebase moved the
base and changed no feature content.

Rustfmt re-wrapped the two merged export lists differently from the local
wrapping, so the style fix is its own commit at the tip, `4a691c53a`
"style(collab): wrap the merged annot and engine export lists".

The merged `ui-egui` then failed clippy under the `collab` feature: main's
comment-popup growth made `PdfCraftApp::apply_edit` exactly
`cognitive-complexity-threshold = 30`, and the branch's four-line
pre-dispatch (the collab feature returns before the local path) took it to
31/30. The fix keeps the dispatch and splits the function: `apply_edit` is now
a thin dispatcher that asks the room (`collab_apply_edit`) and otherwise calls
`apply_local_edit`, which is main's body unchanged. Commit `5b76f7e99`
"refactor(collab): keep the edit path within the complexity limit". Nothing else
in `apply_local_edit` changed; the single-document behaviour is the same.

Backup of the pre-rebase head: `backup/pre-rebase-31bd4a7a` (= `31bd4a7a2`).
Pre-rebase hashes used elsewhere in this file map to the rebased commits as
follows: `2a3b6e43e`→`270e1c11e`, `31bd4a7a2`→`19f49f812`,
`171cbd0fb`→`7ba1cd435`, `4bcb5b85b`→`2671b9dea`, `e0d200af5`→`526898a50`,
`acae90c06`→`49f1cc41e`, `55a0c5101`→`def2723f9`, `91126fdb7`→`0336fd858`,
`458ec5406`→`bf7947bec`, `bd058c8fe`→`5feeb883f`, `b4e9179d8`→`8c44a1918`,
`ce5c38798`→`e58dfb09a`, `a639f30c0`→`34aaf7d10`,
`115ccc87c`→`e58dfb09a`, `ebc8d7d55`→`cc32d2dff`.

Gates at `5b76f7e99` (host `agents`, rustc 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass at `5b76f7e99`; the first run, at `4a691c53a`, failed `apply_edit` at 31/30 (the refactor commit above) |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **213**: proto 12, server 29 (unit) + 31 (bin), hub 5, client 13, client-pdf 28, session 5, ws 8 (unit) + 20 e2e, pdf 41, annot 18, daemon `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1 |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **20/20** (inside the run above) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass — 50 crates, no layering violations |

Nothing was pushed, so the fork, PR #6622 and its body still describe
`81035bf2` / `4eea573be` and the PR stays `CONFLICTING` / `DIRTY`. The full
`ui-egui` suite was not re-run for this rebase; clippy with `--features collab`
and `cargo check -p pdfcraft --features collab` are green on the merged files,
and the last full run is §6.6 below.

### SIGKILL crash recovery — `999da5ec` (limitation 1, closed)

`apps/pdfcraft-collab-server/tests/crash_recovery.rs`
`a_sigkill_between_the_journal_and_the_base_loses_nothing_and_duplicates_nothing`
runs the real `pdfcraft-collab-server` binary (built by cargo for the test),
signs its own RS256 token as the cabinet would, and drives it over a socket:
`annotation.add` (merge, version 1), `page.rotate` (structural, version 2,
epoch 1), then `commit`. The window between the acknowledged journal record and
the base write is made deterministic by a fault point in
`PdfAdapter::commit` (`crates/collab-pdf/src/store.rs::park_at_fault`,
`PDFCRAFT_COLLAB_FAULT=commit-before-base`): the daemon writes a marker file and
parks, the test asserts what is on disk (two journal lines for versions 1 and 2,
no `versions/2`, `CURRENT` still `{"version":0,"epoch":0}`) and sends `SIGKILL`
(`Child::kill`, asserted as signal 9). The point is compiled out of a release
build, where the test prints that it was skipped instead of passing silently.

The restart is the evidence: the welcome carries version 2 and epoch 1 rebuilt
from the journal; the retried `opId crash-a1` returns its stored acknowledgement
(version 1, epoch 0) instead of applying again; a new merge operation addressed
to epoch 1 lands on version 3 (a duplicate would have made it 4); a commit then
writes `versions/3/file.pdf` with `CURRENT {"version":3,"epoch":1}`, an empty
journal, both comments once and the page still rotated 90°.

```
cargo test -p pdfcraft-collab-daemon --test crash_recovery
running 1 test
test a_sigkill_between_the_journal_and_the_base_loses_nothing_and_duplicates_nothing ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.85s
```

Run three times in a row, same result (0.85–1.6 s). The same-session gates:
`cargo test` on the collab package list — proto 12, server 18, hub 4, client 12,
client-pdf 13, session 2, ws 6 unit + 6 e2e, pdf 20, annot 10, daemon 27 unit
**+ 1 crash test** (125 without e2e), all pass; `cargo clippy -p
pdfcraft-collab-pdf -p pdfcraft-collab-daemon --all-targets -- -D warnings` and
`cargo fmt --all --check` pass.

What this does *not* cover: a kill between the base write and the `CURRENT`
rename, and a torn journal line, stay `Store`-level tests; a kill during the
cabinet publication itself is still only reasoned about (the room restarts
dirty and the next commit retries — limitation 6).

### Gate run at `5eb6bea8` — the §6.6 acceptance pass (2026-11-08; pre-rebase hashes, see the 2026-11-09 section above)

Working tree clean; `feature/realtime-collaboration` at `5eb6bea8`
(`a47641fb` plus the preceding docs commit) on top of the pushed head
`0f286ed9`.
Toolchain: cargo/rustc 1.99.1 (no `rust-toolchain.toml`; the workspace declares
`rust-version = "1.95"`, edition 2024).

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all --check` | pass (exit 0) |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — 124 tests: proto 12, server 18, hub 4, client 12, client-pdf 13, session 2, ws 6 (unit), pdf 20, annot 10, daemon 27 |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, 6/6, including `a_commit_does_not_block_presence_in_another_room` (two rooms) and `a_commit_over_the_wire_produces_a_readable_pdf` |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Layering | `cargo xtask layers` | pass |
| UI suite, first pass | `cargo test -p pdfcraft-ui-egui --features collab` | `--lib` **1 failed** of 799 (`only_so_many_connections_may_wait_to_authenticate`); cargo stopped before the integration targets |
| UI suite, full re-run | `cargo test -p pdfcraft-ui-egui --features collab --no-fail-fast` | `--lib` 799 passed, 1 ignored; `it` 1493 passed, **1 failed**; `ocr` 6, `page_shadows` 7, `perf` 4, `play` 1 pass; 5 ignored in total |

The two failures are environment, not regressions:

- `control::transport::transport_tests::only_so_many_connections_may_wait_to_authenticate`
  (`control/transport.rs:573`) expected the fourth connection's `auth` reply to
  carry `{"ok":true}` and got `Null`. It passes alone in 0.52 s and in the full
  re-run; the file's last change is `2d27306a`, outside this branch; the test
  depends on a 100 ms admission window while 800 tests run in parallel on four
  cores already busy with another build. Parallel-load flake.
- `editing::a_custom_property_whose_name_the_tools_refuse_is_listed_and_left_alone`
  — the known font failure (`No glyph for "�" (U+FFFD)`), reproduced without
  the `collab` feature in earlier sessions; environment.

The acceptance brief also names `collab-doc`; this workspace has no such crate —
the PDF document layer is `pdfcraft-collab-pdf` and it is in the list above.

Commands that were run in earlier sessions:

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

Counts: proto 12, server 18, hub 4, client 12, client-pdf 13, session 2,
ws 6 unit + 6 e2e, pdf 20, annot 10, daemon 27.

- `crates/collab-server` `a_commit_publishes_after_the_room_is_released` —
  `commit` writes the base but does not run the publication and the room stays
  dirty; a second commit is `busy` until `finish`; `finish` advances what the
  cabinet holds. `a_failed_publication_leaves_the_room_dirty_and_flags_it` —
  a `409` from the publication leaves the room dirty and `conflict`.
- `crates/collab-hub` `resync_and_commit_answer_the_requester` now asserts the
  commit answers only after `publish` + `finish`, never from `send` alone.
- `crates/collab-ws/tests/e2e.rs` `a_commit_does_not_block_presence_in_another_room`
  is the lock proof: two rooms, room A's cabinet call held open on a gate, a
  presence in room B still reaches its member; with the call inside the lock
  the test fails on the 5 s read timeout.
  `serve::tests::a_second_editor_joins_and_gets_a_zero_lease` was made
  deterministic (the lease goes to the first *processed* join, so the second
  client now connects after the first welcome) — it was a parallel-run flake,
  not a regression.
- The `ui-egui` suite was **not** re-run for this slice: the only UI change is
  one arm in `code_name` for the new `busy` code, and
  `cargo check -p pdfcraft --features collab` plus
  `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -D warnings`
  are green on it. The last full run (1493 passed, 1 known font failure) is
  recorded above.

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

### Replacing a page and inserting a blank one — §5 p.3 / limitation 3 (2026-11-09, local; not pushed)

`e9756d138` on `617178084`. Limitation 3 said structural coverage stopped after
rotate/move/delete/restore/insert/duplicate: Replace Pages and Insert Blank Page
were `unsupported` in a session, so two of the page menu's own commands failed
there. Both are addressable structural operations now (RFC §3.1.4):

- **`page.replace`** addresses the page it replaces (`Target::Page`, current
  epoch) and carries the new page as a one-page PDF (`payload.bytes`, base64).
  The payload is checked as a whole before the document changes: base64 decodes,
  exactly one page (a two-page file is `invalid`, not a page inserted by
  surprise), and the copy is within `MAX_RESTORE_PAGE_BYTES` (160 KiB, `limit`).
  A missing page is `invalid`, a foreign epoch `stale` (retryable). None of the
  refusals moves `version`/`epoch`, writes a journal line or is applied locally.
  The page count and every other page's index stay as they were.
- **The undo is the copy the server kept.** The adapter copies the page before
  the edit (`page.copy`) and answers its token in `applied.result.copy`; the
  accepted operation carries `bytes` (so a replica and the journal need no copy)
  and the token. The undo is `page.replace` with `payload.copy` and the same page
  addressed in the new epoch: the old page comes back exactly, and that undo
  keeps its own copy (so it is itself undoable). A page over the budget is still
  replaceable; no copy is kept, and an undo that names none (`"copy": null`) is
  `conflict` without a change. The redo repeats the original operation, which
  carries `bytes`, so it never depends on a copy. Replay rebuilds the copies in
  journal order, so the token is deterministic after a restart.
- **`page.insert_blank`** names the size of the page it adds (`width`/`height`
  in points, `> 0`, finite, ≤ 14400 — Annex C's page-size limit) and the insert
  position as the target (`index` may equal the page count). Replicas build the
  same page with `Edit::InsertBlankPage`; anything else is `invalid` before the
  edit. Its undo is `page.delete` at `at`.
- Both are `structural`: the room serialises them under the lease and requires
  the head version, and each bumps the epoch, so an operation built on a passed
  epoch is `stale` and retryable.
- **Client projection.** `Edit::ReplacePages` becomes one `page.replace` per
  page (each page travels whole; a source/target count mismatch is refused as a
  whole), `Edit::InsertBlankPage` one `page.insert_blank`. `inverse_for` returns
  no inverse for a replacement (the replica no longer has the page); the app
  patches the group from `applied` through `inverse_after`, which builds the
  copy-named undo — or the `copy: null` one when the server kept none.
  `collab_apply_edit` now counts `Edit::ReplacePages` as structural, so a
  commenter is refused before an operation is built.

Tests:

| Crate | Test |
|---|---|
| `collab-pdf` | `a_replaced_page_comes_back_from_the_copy` (content replaced, count unchanged, the accepted op carries the page and its token, the undo restores the exact page on a commit), `a_replacement_that_is_not_one_page_is_refused_before_the_document_changes` (bad base64, two pages, empty, no page at all), `a_replacement_over_the_limit_is_refused_before_the_document_changes`, `the_undo_of_a_replacement_without_a_copy_is_a_conflict` (a page padded over the budget: no token, the undo moves nothing), `a_replacement_addressed_to_another_epoch_is_retryable`, `a_blank_page_is_inserted_with_the_size_it_names` (crop 300×400, undo restores the count, zero/negative/huge/non-numeric sizes refused), `a_replacement_replays_from_its_operation` |
| `collab-client-pdf` | `a_page_replacement_becomes_one_addressed_operation_and_its_undo`, `a_multi_page_replacement_is_one_operation_per_page` (count mismatch refused), `the_accepted_replacement_answers_with_the_operation_that_undoes_it` (with a copy and without), `a_blank_page_becomes_an_addressed_operation_with_its_size`, `a_replacement_over_the_limit_is_refused_on_this_side_too` |
| `collab-ws` e2e | `two_clients_replace_a_page_and_insert_a_blank_one` (both replicas converge on the replaced content and the blank page's crop, an over-limit and a missing-page replacement are refused without a change), `a_page_replacement_undoes_from_the_copy_and_redoes` (undo restores the old page on both replicas, redo replaces it again, a passed epoch is `stale`) |

Gates at `e9756d138` (host `agents`, rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-{proto,server,hub,client,client-pdf,session,ws,pdf,annot} -p pdfcraft-collab-daemon --all-targets -- -D warnings` | pass |
| Tests, collab crates + daemon | `cargo test` on the same package list | pass — **209**: proto 12, server 27, hub 5, client 13, client-pdf 28, session 5, ws 8 unit, pdf 41, annot 18, daemon 31 + `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1, e2e 18/18 |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **18/18** (inside the run above) |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| UI, the collab module | `cargo test -p pdfcraft-ui-egui --features collab --lib collab::` | pass — 6 |

What this does **not** cover: the replacement carries only the page's content
and the boxes `Edit::ReplacePages` copies (MediaBox/CropBox/…), while the target
page's links, comments, bookmarks and form fields stay the page's own — an undo
therefore returns the content exactly but not annotations someone added to the
page in between (those are their own merge operations and stay). Split/combine,
bookmarks, layers, attachments, form structure and content edits are still not
shared. The full `ui-egui` suite was not re-run: clippy with the feature
compiles it and the collab module's tests pass.

## 5. Known limitations (the prod gap)

Ordered by the risk they pose to production.
1. ~~**Crash recovery is not crash-tested against the real process.**~~ Closed
   by `apps/pdfcraft-collab-server/tests/crash_recovery.rs` at `999da5ec`
   (§4): the daemon accepts a merge and a structural operation over a socket,
   the commit parks between the journal record and the base write, the process
   is SIGKILLed, and the restart comes back at version 2 / epoch 1, returns the
   stored acknowledgement for the retried `opId`, and commits each edit once.
   The layout and the other two windows (base written but `CURRENT` not yet
   renamed, torn last line) stay covered at the `Store` level: a kill between
   the base write and the `CURRENT` rename is a `Store::tests` case only, not a
   real-process one. A kill during
   the cabinet publication itself is still not crash-tested: it only leaves the
   room dirty and is retried by the next commit (limitation 6).
2. **Annotation coverage beyond the typed set.** `annotation.add` shares
   `TextMarkup` (kind/quads/colour), `Rectangle`, `Oval`, `Line`, `Ink`,
   `FreehandHighlight`, `TextBox`, `Typewriter`, palette `Stamp`, `Mark` and
   `Note` with their style (colour, opacity, width, fill, border).
   `annotation.update` (`ce5c38798`, §4) shares the restyle of an existing
   comment — colour, opacity, line width, fill, border colour and shape, and
   line endings (`/LE`, `c400147b3`) — as one operation with a
   compare-and-set undo; `annotation.font_size` shares the font size of a text
   box, typewriter text or callout, and `annotation.info` the author, subject
   and note icon, each as its own addressable operation with the same CAS undo
   (`c400147b3`, §4); `annotation.reply` carries a review state (`/State`)
   whose reply is named by the client and survives delete/restore; and
   `annotation.parts`, `annotation.move` and `annotation.geometry` share a
   comment's fill and text opacities, its move, its resize and the points of a
   line or polygon (`22d29d070`, §4), each as its own operation with the same
   compare-and-set undo; and `annotation.popup` (`0c57e2271`, §4) opens, closes
   or moves the comment's own pop-up window, with the same undo — the last edit
   inside a comment the room had no operation for. A Comment Properties batch or
   a group drag that
   carries them is no longer refused whole: it becomes one group of operations
   per comment. Drawn and
   typed signatures, custom stamps, redaction marks and attachments are still
   refused with `unsupported` (RFC §3.1.1). Nothing an edit inside a comment
   can change is left unshared: what remains outside the wire is the shape and
   style of other comment types, not an edit of an existing one. A window the
   open itself created has no state to restore (`before` is `null`), so its undo
   closes it where the open put it — the engine has no edit that removes a
   `/Popup`, and the window's place is view state, not document content (RFC
   §3.1.6).
3. **Structural coverage.** Page rotate, move, delete and restore, insert and
   duplicate (§4, `b13933e9b`), replace and blank insert
   (`e9756d138`, §4, RFC §3.1.4), and now split and combine
   (`362f9c72b`, §4, RFC §3.1.5), whose dialogs send them inside a room (§4
   above). An insert, a duplicate or a replacement
   carries the page as a one-page PDF inside the operation (base64, ≤
   `MAX_RESTORE_PAGE_BYTES` = 160 KiB); a page over that is refused with
   `limit` before the document changes and is not applied locally — a
   replacement over the budget still happens, but its undo is a `conflict`
   (no copy). A blank insert names its size in points. A split carries the
   part it cuts out in the same budget (a bigger part is `limit`) and a combine
   inserts a whole part in one operation; both name both sides of the page set,
   so the numbering is deterministic, and their undo is the other operation.
   All of them bump the epoch, so an operation addressed to a passed epoch is
   `stale` (retryable). What a room's Split cannot yet do: write the part it
   takes out to a file, or produce more than one part from one confirmation
   (one `page.split` carries one contiguous tail; the dialog names the cut).
   Bookmarks, layers, attachments, form structure and
   content edits are still not shared.
4. ~~**Presence has cursors, not selections.**~~ Closed on the client in
   `b4e9179d8` (§4): presence now carries the selected comment's `/NM` or a
   rectangle in user space (a text range's bounding box, a column select), under
   the cursor's 30 s TTL, drawn in the actor's colour for every participant
   including viewers, and it never becomes an operation or moves `version`.
   The rest of it is closed in `5bad54c40` (§4): several selected comments travel
   as a list (primary `/NM` plus `also`), a text range as one rectangle per line
   piece with the caret at its active end, and the canvas draws each quad.
   What is still open: the caret is the range's active end, not the caret inside
   a form field being typed into; a selection spanning several pages names only
   the comments on the page in view; and the host page's `pdfcraft:presence`
   report still cannot carry the participants' page/selection, because
   `craft-web-host` (`7666e2b`, a foreign crate) describes a `Participant` as
   `clientId`/`role`/`color` only — its contract allows the two optional fields,
   but adding them is a change outside this repository. The app-side state
   (`CollabState.participants[].page`/`.selection`) already carries them.
5. **Undo of a deletion is best effort.** A comment the room added comes back
   exactly; a comment that came from the file has no wire body, so its delete
   carries no copy and cannot be undone. A page comes back while its one-page
   copy fits `MAX_RESTORE_PAGE_BYTES` (160 KiB); a larger page is deleted without
   a copy. Copies are memory-bounded (64 / 64 MiB) and are rebuilt by replay, not
   persisted on their own.
6. **The cabinet contract is implemented, the retry queue is in place, the
   production loop is still open.** `POST /v1/sessions`, the commit callback,
   the background publication queue and `409` exist (RFC §8.2, §4). A failed
   POST (network error, `429`, `5xx`) is retried by the daemon with an
   exponential backoff (1 s doubling to 60 s) under a derived idempotency key,
   without waiting for the next commit or the last-client-leave commit; a
   retry publishes the same version, never a second one, and a `409` stops the
   queue until an operator re-seeds the room (§4). The operator's way out is
   `POST /v1/sessions/{fileId}/resolve` (`62d48d30a`, §4): it names the
   cabinet's current version and etag, clears the flag and lets the next commit
   publish again; a repeat, a wrong version, another file or a role without
   write access is refused without a change. What is still missing: the queue is
   in-process, so a restart falls back to the next commit/join/leave; the
   resolve does not republish by itself (a room that then goes quiet waits for a
   commit, a join or the last leave); and `https` without a proxy. The shared
   cabinet contract also lets the cabinet number versions and return its own
   `{version, etag}` from the callback; the daemon now adopts that body as the
   copy it holds (falling back to `expectedVersion + 1`) and keeps both numbers
   in `CABINET`, so a cabinet with its own numbering works as long as that
   number is per file and monotonic.
7. **No TLS in the daemon.** WSS, the control plane and the callback are
   expected to sit behind a reverse proxy; `https://` source URLs and callback
   URLs are refused with a clear error. No per-IP throttling, no session
   TTL/eviction besides the lease TTL.
8. **Large-document budget.** 500 pages are tested functionally; the 100k-page
   inspection cost and batching remote applies are not measured. The control
   plane caps a file at `--max-source-bytes` (256 MiB) but nothing else.
9. **No automation tool/parity entry yet.** The feature is behind a flag; when
   it becomes user-facing, `crates/automation` and `parity/` need an entry.
10. ~~**Journal growth is bounded only by commits.**~~ Closed in `8fc898bae`
    (§4, §6.1): a live room compacts its journal after
    `RoomConfig::forced_commit_ops` (default 2000) accepted operations or
    `forced_commit_bytes` (default 8 MiB) of records, with the base written
    under the hub lock and the cabinet POST outside it; a `busy` room is retried
    by the next message or by the end of the publication, and
    `pdfcraft_collab_forced_commits_total` counts the compactions. The time
    threshold is closed in `2a3b6e43e` (§4, §6.1):
    `RoomConfig::forced_commit_idle_secs` (default 300) compacts a journal that
    has seen no accepted operation for that long, through the daemon's
    background sweep; idle is room state — a reconnect does not reset it — and
    the journal's recorded acceptance time re-arms it after a restart. Still
    open: a `409` from the cabinet stops forced commits (the room is flagged
    `conflict` until the operator's `POST /v1/sessions/{fileId}/resolve`, §4) —
    retrying the same `expectedVersion` cannot succeed. Bases are pruned to the
    one `CURRENT` names, so disk is one PDF plus the journal, not one PDF per
    commit.
11. ~~The callback is synchronous inside the hub lock~~ — the base is written
    under the lock and the cabinet POST runs after it is released (§8.2): a
    cabinet that does not answer delays the committer's connection, not the
    other documents. A second commit of the same document while one is
    publishing gets `busy` (retryable), so two versions cannot reach the
    cabinet out of order. ~~What is still missing is a publication *queue*.~~
    Closed in §4: a retryable failure stays `publishing` and is retried by the
    daemon's queue with a backoff; the queue is in-process, so a restart falls
    back to the next commit, join or leave.
12. **The legacy annotation payload lives until the flag ships.** A payload
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
*after* the commit; `tests/crash_recovery.rs` SIGKILLs the real process between
the journal record and the base write and replays it, §4).

The journal is also bounded while clients stay connected (limitation 10,
`8fc898bae`, §4). `Room` counts the operations and the bytes of journal records
since the last base (`journal_bytes`, mirrored from
`collab_pdf::store::JournalRecord`; `replay` counts what it restores) and
`RoomConfig::forced_commit_ops` / `forced_commit_bytes` (defaults 2000 / 8 MiB,
`0` disables each) declare the threshold. The transport calls
`Hub::forced_commit` after every client message and on a join, so a live room
compacts itself: `Room::commit_now` writes the base under the hub lock, the
`Pending` publication runs after the lock is released, and `Hub::finish` queues
another compaction if edits arrived while it ran. A `busy` answer (a publication
in flight) is retried by the next message or by that publication's completion; a
`409` sets the room's `conflict` flag and stops further forced commits, the same
rule as a manual commit; the flag clears through the operator's resolve (§4),
after which the threshold is due again on the next call. `/metrics` gains
`pdfcraft_collab_forced_commits_total`.

The time threshold (`2a3b6e43e`, §4) is `RoomConfig::forced_commit_idle_secs`
(default 300, `0` disables it): the newest journaled operation is that old and
the journal has not grown since. "Idle" is room state — a reconnect or a join
does not reset it — and it survives a restart because each journal record
carries its acceptance time (`ms`, `#[serde(default)]`, read back by
`Room::replay`); a record written before the field existed arms no clock. The
daemon's background loop (`WsServer::spawn_publication_retries`, 100 ms) sweeps
every room with `WsServer::sweep_forced_commits`, so a room that sees no message
still compacts itself, and the compaction counts in the same metric.

What is still open here: a room that is dirty only because its base was written
and never published is not idle-eligible (its journal is empty) — the next
commit, join, leave or the queued publication republishes it; and a kill between
the base write and the `CURRENT` rename is still covered at the `Store` level
only (`Store::tests`; §5 p.1). The fault injection limitation 1 asked for is done
for the real process in the journal→base window
(`apps/pdfcraft-collab-server/tests/crash_recovery.rs`, `999da5ec`, §4).

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

Tests: the round-trip and two-client scenarios are listed in §4. **Done since
(`ce5c38798`, §4):** the restyle of an existing comment (`annotation.update`:
colour, opacity, width, fill, border colour/shape, one all-or-nothing edit with
a CAS undo) and status replies (`annotation.reply` with `state` → `/State` +
`/StateModel /Review`, named by the client, restored as a status reply).
**Done since (`c400147b3`, §4):** line endings (`/LE`) in the same style
object, the font size (`annotation.font_size`) and a comment's author/subject/
note icon (`annotation.info`), each its own addressable operation with a CAS
undo. **Done since (`22d29d070`, §4):** a comment's fill and text opacities
(`annotation.parts`), its move (`annotation.move`), its resize and the points
of a line or polygon (`annotation.geometry`), each with a compare-and-set undo,
and a Comment Properties batch or a group drag folded into one group of
operations per comment. **Done since (`0c57e2271`, §4):** the comment's pop-up
window (`annotation.popup`) — its place and its open state, with the same
compare-and-set undo; a comment without a window of its own gets one beside it,
and the undo of that creation closes it where the open put it (RFC §3.1.6).
What is still open here: the same typed envelope for `AddCustomStamp` if the
cabinet ever needs it (it references an XObject, so it would need the blob
channel).

### 6.3 Web host bridge — done in this branch (local, not pushed)

The transport is a trait with two implementations and one protocol driver in
front of both (commit `81a92f32f`, §4):

- `collab-session::transport::Transport` — `write(ClientMessage)` and
  `read() -> ReadOutcome` (`Opened` / `Message` / `Unreadable` / `Nothing` /
  `Closed`). `driver::Session` owns everything the protocol does: hello, the
  welcome, version and epoch, resync and the re-addressed retry, the 30 s lease
  heartbeat, presence, the outbox. It is the code that used to live inside the
  socket loop.
- `native::Handle` — the worker thread and blocking socket, unchanged in
  behaviour (20 ms read timeout, 1 s redial, merge-only resend).
- `web::WebHandle` — the browser's `WebSocket`: callbacks push into a queue,
  `try_event` steps the driver once per call, a close redials with a backoff
  that doubles to 10 s; no timers, no callback cycle, no threads.
- `Sink` — the object-safe face the UI holds (`Box<dyn Sink>`). `Handle` is no
  longer named outside the crate, and `ui-egui --features collab` compiles for
  wasm32 with the participants window, remote cursors and the commit button.
- Messages (INTEGRATION.md §6): `pdfcraft:session` in (the app connects; the
  host's role is advisory, the token decides — ADR-0008 §7);
  `pdfcraft:session-state` on a change of status/error/participants;
  `pdfcraft:presence` with the same participant list, coalesced at ≤10 Hz;
  `pdfcraft:commit` when the cabinet owns the commit (no bytes; the cabinet
  assembles the file) and `pdfcraft:committed {version, etag}` back. The basic
  `open/save/dirty/failed` messages are unchanged.
- The document key comes from the token's `fileId` (the server refuses a `hello`
  whose `doc` is not the token's file), with the host's `sessionId` as the
  fallback for a token that cannot be read.
- A session handed over before the file is open waits for the next frame instead
  of being refused: the host sends `open` and `session` together, and the file
  opens after `poll_collab` in the same frame.
- `craft-web-host` (the crate that owns the message contract and both
  transports) does not carry the collaboration messages in `storytold/main` yet,
  so `apps/pdfcraft-web` pins `copoka/craft-libs` `7666e2b1` for that crate only
  (ADR-0016 §3), the same pin the other Craft apps use. `Cargo.lock` records it;
  after the craft-libs PR merges, the URL goes back and the merge commit is
  pinned.
- The web feature is off by default:
  `cd apps/pdfcraft-web && trunk build --release --features collab`.

Tests and gaps: §4. What is still open: a browser run against the cabinet's page
(two iframes, `session` in, edits, `commit` out, `committed` back) — the code is
compile-checked and driver-tested, not exercised in a browser here; the cabinet's
own commit path is the one WordCraft documented, and the same page-side wiring
has to be repeated for PdfCraft's origin/CSP.

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
- **The publication runs outside the hub lock** (the chair's fourth condition to
  close before the PR): `Room::commit` writes the base under the lock and
  returns a `Commit`; `WsServer::run_pending` releases the lock, runs
  `Commit::publish` (the cabinet POST) and then calls `Room::finish` under the
  lock again, which advances the `CABINET` marker or leaves the room dirty.
  `Hub::send` hands a commit out as `Pending`, so the transport cannot run it by
  accident under the lock; the last-client-leave commit takes the same path.
  A second commit of the same document while one is publishing is refused with
  the new `busy` error code (`retryable`): two versions must not reach the
  cabinet out of order. Test: `collab-ws` e2e
  `a_commit_does_not_block_presence_in_another_room` (two rooms, the cabinet
  call held open on a gate, presence in the other room still arrives; fails on
  the 5 s read timeout when the call is put back under the lock).
- **Done since:** the `conflict`-resolution endpoint
  (`POST /v1/sessions/{fileId}/resolve`, §4, `62d48d30a`) and the callback's
  `{version, etag}` answer (the marker keeps the room's and the cabinet's
  numbering). Still open here: `https` without a proxy (limitation 7). The
  publication queue is done in §4 (in-process).

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
- Selections followed (`b4e9179d8`, §4): `collab_presence` sends the selected
  comment's `/NM` (on the page in view) or a rectangle in user space — the text
  range's bounding box, the selected comment's quads, a column select — and the
  canvas draws `remote_selections` as a translucent fill in the actor's colour.
  The same TTL and epoch rules apply; viewers receive and draw them too.
- TTL 30 s is client-side (`PRESENCE_TTL`), because presence is never journaled
  or replayed: a participant that goes quiet for 30 s is shown as `inactive` and
  its cursor and selection are dropped; a disconnect or `leave` clears them at
  once.
- The canvas knows nothing about collaboration: it draws `remote_cursors` and
  `remote_selections` and reports `local_cursor`, all empty without the feature.

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

Still open here: the blob channel that would lift the 160 KiB page cap and make
a page restore independent of one frame — decided as a separate PR after this
one merges (§4 "Blob channel — decision"), so the cap stands until then. Remote
selections are done (§4, `b4e9179d8`); what remains of them is listed in §5 p.4
(several selected comments, per-line range geometry, caret).

### 6.6 Cleanup and submission

- **Split and combine are done locally in §4** (`362f9c72b`, 2026-11-09, not
  pushed): `page.split` takes a range out and carries the part in the accepted
  operation, `page.combine` joins a whole part in one operation, both are
  structural (head version + lease, epoch bump, `stale` on a passed epoch),
  refuse the document's boundaries and the 160 KiB frame budget before any
  change, and undo each other. Tests: `collab-pdf` (6), `collab-client-pdf`
  (3) and `collab-ws` e2e (2, two clients: converge, stale-retry, undo).
  **The dialogs are wired in §4 (this session, not pushed):** in a session,
  Split sends one `page.split` (the plan's cut, the tail leaves, the dialog says
  so) and the part opens as a document of its own; Combine sends one
  `page.combine` per list entry into the open document instead of a new file,
  sent one at a time as one confirmation, and a stale part takes the joined
  parts out again (§4, "The split part and several parts per Combine"); the
  local export runs only outside a session. Tests: the two-replica projection
  tests and the UI and e2e tests in §4. What is left: a part over the budget
  still waits for the blob channel (RFC §4.1); the part opens as an unsaved
  document, not in a file.
- **The publication queue (limitation 6 / §5 p.6) is done locally in §4**
  (2026-11-09, not pushed): a retryable cabinet failure is retried by the
  daemon's background queue with a doubling backoff (1 s → 60 s) and a derived
  idempotency key, the room stays `publishing` while it is queued (so a new
  commit is `busy` and no second version is created), a `409` stops the queue,
  and `/metrics` exposes `pdfcraft_collab_publication_retries_total`. Tests:
  `collab-server` queue state machine, `collab-ws` e2e retry and backoff,
  `apps/pdfcraft-collab-server/tests/publication_retry.rs` (lost answer →
  retry → the cabinet sees a duplicate and stores one version). **The
  `conflict`-resolution endpoint is done in `62d48d30a`** (`POST
  /v1/sessions/{fileId}/resolve`, §4). Still open: the queue is in-process (a
  restart falls back to the next commit/join/leave), and a resolved room waits
  for the next commit/join/leave to republish.
- **Rebase onto `origin/main`.** The branch had already been rebased onto
  `43f24aa1` (2026-11-08; that rebase had one conflict in
  `crates/engine/src/session/editing.rs` — main's
  `apply_reporting`/`prepare_text` split against this branch's `apply_remote`
  with a `record_undo` flag; the resolution makes `apply`, `prepare_text` and
  `apply_remote` all call `apply_reporting(id, edit, record_undo)`, so the
  remote path still clears the local snapshot history). `main` then moved to
  `8d5d39ef` and the branch was rebased onto it on 2026-11-09 with **no
  conflicts** (§4). The rebased tip before the docs commits is `faad6b79`; the
  pre-rebase head is kept as `backup/pre-rebase-7628b605`.
- **Rebase onto `origin/main` `ca9e3c2c3` (the requested `cde331fb7`), done in
  the 2026-11-09 rebase session.** `git rebase origin/main` moved all 47 commits
  onto the current `origin/main`; one conflict in `crates/engine/src/tests.rs`
  (both sides appended tests at the end of the file) — main's test kept first,
  both branch tests after it (§4). The rebased head before this STATUS commit is
  `0e839a4b1`; the pre-rebase head is kept as
  `backup/pre-rebase-2529c8c2`. Nothing was pushed, so the PR still describes
  `81035bf2`.
- **Rebase onto `origin/main` `b6cbf9bb3`, done in the 2026-11-09 session.**
  `git rebase origin/main` moved all 64 commits; four conflicts (`Cargo.lock` —
  main's `zip` 9.1.3 without `zopfli`; `annot/src/modify.rs`; `ui-egui/src/app.rs`;
  `engine/src/edit.rs`) were resolved by hand, and one compile fix at the tip
  `e0d200af5` covers main's new `FieldValue::Widget`/`ChoiceIndices` and the
  per-kid check-box on states (§4). The pre-rebase head is kept as
  `backup/pre-rebase-4eea573b`; the rebased head before this STATUS commit is
  `e0d200af5`, 65 ahead of `origin/main` and 0 behind. Nothing was pushed, so
  the fork and the PR head still describe `81035bf2`; the PR body was replaced
  later that day, at `4bcb5b85b` (§4).
- **Rebase onto `origin/main` `ba28458ad` (the requested `d2139e58a`), done in
  this 2026-11-09 session.** `git rebase --onto origin/main d2139e58a` moved all
  76 commits; three files conflicted, four stops (`ui-egui/src/app.rs` twice —
  the frame's `window_preferences_frame` line and the collab `show_collab`
  call, then the widened `#[cfg(feature = "collab")]` guard;
  `ui-egui/src/canvas.rs` — `TabInterface` against the presence cursor fields;
  `engine/src/edit.rs` — `PrinterMarks` against `Restyle` in the import list),
  each resolved by keeping both sides (§4). The style fix for the merged import
  list is `5f3c6f3c2`; the head before this STATUS commit is `5f3c6f3c2`, 77
  ahead of `origin/main` and 0 behind, `git merge-tree --write-tree origin/main
  HEAD` exits 0. The pre-rebase head is kept as
  `backup/realtime-collab-pre-ba28458ad`. Nothing was pushed, so PR #6622 still
  describes the owner's force-push `31bd4a7a2` and stays `CONFLICTING`.
- **Gates at `e0d200af5` are green** (§4): `cargo fmt --all -- --check`; clippy
  for the nine collab crates plus the daemon and for `pdfcraft-ui-egui
  --features collab`, `--all-targets -- -D warnings`; 209 collab/e2e tests
  (proto 12, server 27, hub 5, client 13, client-pdf 28, session 5, ws 8 unit +
  18 e2e, pdf 41, annot 18, daemon 31 + the conflict-resolve, crash-recovery
  and publication-retry tests); `cargo check -p pdfcraft --features collab`;
  `cargo xtask layers` (50 crates, no violations);
  `git merge-tree --write-tree origin/main HEAD` exits 0.
- **Gates at `0e839a4b1` were green** (§4; the historical `ca9e3c2c3` figure
  set, superseded by the `b6cbf9bb3` rebase above): `cargo fmt --all --check`; clippy for
  the nine collab crates plus the daemon and for `pdfcraft-ui-egui --features
  collab`, `--all-targets -- -D warnings`; 142 collab/e2e tests (proto 12,
  server 22, hub 5, client 12, client-pdf 13, session 5, ws 6 unit, pdf 20,
  annot 10, daemon 28 + the crash-recovery test, e2e 8/8);
  `cargo check -p pdfcraft --features collab`; `cargo xtask layers` (50 crates,
  no violations); `git merge-tree --write-tree origin/main HEAD` exits 0.
- **Gates at the rebased tip `faad6b79` are green** (§4): `cargo fmt --all
  --check`; clippy for the nine collab crates plus the daemon and for
  `pdfcraft-ui-egui --features collab`, `--all-targets -- -D warnings` (this
  one had to be re-run: the first attempt's `clippy-driver` children were
  SIGTERMed under machine memory pressure); 125 collab tests (proto 12,
  server 18, hub 4, client 12, client-pdf 13, session 2, ws 6 unit, pdf 20,
  annot 10, daemon 27 + the crash-recovery test), 6/6 e2e (the two-room commit
  test included); `cargo check -p pdfcraft --features collab`;
  `cargo xtask layers` (50 crates, no violations); the wasm check for
  `collab-client-pdf`; and the UI suite — 2440 passed across all targets
  (`--lib` 840 + 1 ignored, `it` 1582 + 4 ignored, ocr 6, page_shadows 7, perf
  4, play 1) with the one known `U+FFFD` font failure
  (`editing::a_custom_property_whose_name_the_tools_refuse_is_listed_and_left_alone`),
  which reproduces without the `collab` feature. The parallel-load flake in
  `control::transport` did not recur.
- **Pushed on 2026-11-09.** The force-push of the rebased branch was done (a
  publication action by the owner); `git ls-remote fork
  refs/heads/feature/realtime-collaboration` and `gh pr view 6622` both name
  head `81035bf2` — the local HEAD before this STATUS commit — and the PR reads
  `mergeable: MERGEABLE` (previously `333072e8` / `CONFLICTING`). The local
  branch is 43 commits ahead of `origin/main` and 0 behind: the 40 rebased
  commits, `e04ed03b` and `81035bf2` (both pushed), and this STATUS commit
  (local only). `git merge-tree --write-tree origin/main HEAD` exits 0 with no
  conflicted paths. Pushing this docs commit and editing the PR text remain
  publication actions for the owner. **This paragraph describes the state before
  the third rebase**: after it the local branch is 47 commits ahead of
  `ca9e3c2c3` and 0 behind, and the PR is `CONFLICTING` again until the owner
  force-pushes the rebased branch (the fork head stays `81035bf2`).
- **The PR body was replaced** on 2026-11-09 with the text in
  `pdfcraft/PR-DRAFT.md` (from `## What` on); the body on the PR is byte for
  byte that text plus CodeRabbit's appended summary block. Every claim in it
  described the feature at the then-pushed head `81035bf2`; the rebase only
  changed the base, not the features it describes. (Superseded on 2026-11-09:
  the body was replaced again with the text updated to `b6cbf9bb3`, §4.) What it fixes: cursors and their TTL,
  the typed annotation set, undo of deleted comments and pages from server
  copies, the control plane, the commit callback running outside the hub lock,
  and the SIGKILL crash-recovery test.
- Remaining limitations stated in that body: no web client/host bridge
  (§6.3) — **since closed locally in `81a92f32f`, not yet in the pushed head or
  that body** — no publication queue (a failed cabinet POST is retried only by
  the next commit or the last-client-leave commit) — **since closed locally in
  §4 (the queue is in-process)** — no endpoint to resolve a `conflict` room —
  **since closed locally in `62d48d30a`, §4** — WSS/TLS behind a reverse proxy,
  page restore capped at 160 KiB, text selections not shared, and signatures /
  custom stamps / redaction / attachments / page insert-duplicate-split refused
  as `unsupported` (insert and duplicate are shared since `b13933e9b`, §4; the
  body and the pushed head still say the old list).
- Update `docs/collab/RFC.md` §16 as items close — done for §6.4, §6.5 and
  limitation 11 (§8.2, §9, §16 p. 9).
- Add an automation/parity entry once the feature is user-facing.
- No CI changes in the feature PR. The body is not in this tree: the
  cabinet-office checkout keeps the draft at `pdfcraft/PR-DRAFT.md` and the
  checklist at `PR-CHECKLIST.md`; the body must say what was tested, what was
  not, and nothing about who wrote it.
- RFC v1 was proposed on the bus (`20261108T115119Z-pdfcraft-9830`). Approvals
  on record: cadcraft (`20261108T124137Z-cadcraft-60aa`), deckcraft
  (`20261108T130802Z-deckcraft-2885`), chair (`20261108T115818Z-craft-05ab`),
  wordcraft (`20261108T142201Z-wordcraft-5893`, vote A), all with non-blocking
  remarks; chair recorded the decision as `20261108T143416Z-craft-2403`
  (ADR-0015). **Quorum is 5 of 5** (`AGENDA.md` §4).
- Chair's four conditions to close before the PR: epoch wording (`RFC §8.1`),
  lease wording (`RFC §7`), undo CAS (§6.5), commit callback outside the hub
  lock (limitation 11) — all four are closed in this branch; the last one in
  §6.4. Nothing blocks the PR from this list.
- PdfCraft voted on deckcraft's RFC (`20261108T135623Z-pdfcraft-2aad`, A with
  the unified lease wording).

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
- A commit's publication runs on the committer's connection thread **after** the
  hub lock is released (`WsServer::run_pending`). Do not run `Commit::publish`
  inside `with_hub` or under `lock(&self.shared)`: one cabinet that does not
  answer would then freeze every document, and
  `a_commit_does_not_block_presence_in_another_room` fails on its 5 s timeout.
  `Hub::send` returns a `Pending` commit; a caller that drops it without
  `Hub::finish` leaves the room `publishing` and every later commit answers
  `busy`. The same holds for a queued retry: take it with `Room::take_retry`,
  run it outside the lock, and always report back with `Room::finish_retry` —
  a dropped retry leaves the room `publishing` for ever.
- The publication queue is one entry per document (`Shared::retries`) and one
  background thread (`WsServer::spawn_publication_retries`, started by the
  daemon with a 100 ms tick). The backoff is `Config::publication_backoff` /
  `publication_backoff_max` (1 s / 60 s by default; tests pass 20 ms). A
  `retryable: false` failure is never queued: that is the difference between
  "the cabinet did not answer" and "the cabinet refused the body", and queueing
  the second would spin forever. The queue does not survive a restart: the
  room comes back dirty and the next commit, join or leave republishes it with
  the same derived `idempotencyKey`.
- The idempotency key is `fileId:version:sha256`, in the body
  (`idempotencyKey`) and the `Idempotency-Key` header. It names the payload, so
  a retry of the same version carries the same key and the cabinet can answer
  "already have it"; do not make it random per attempt — that would turn a lost
  answer into a second version.
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
- Auth is decided once at start-up: `--jwt-secret-file` /
  `PDFCRAFT_COLLAB_JWT_SECRET` (HS256) or `--jwt-key` (RS256), never both.
  `--jwt-key` has no default any more, so a deployment must name its scheme or
  the daemon exits with a message naming both flags. Do not add a fallback: a
  daemon that guesses would accept a ticket it must refuse. The secret is never
  logged, so diagnose a refused ticket from its claims and the configured
  audience, not from the key.
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
- A restyle is one operation on purpose: do not map a `StyleAnnotation`,
  `FillAnnotation`, `BorderColorAnnotation` or `BorderShapeAnnotation` to its own
  wire op. `annotation.update` carries the final look plus `before` (the look it
  replaced); `expect` is the undo's compare-and-set and is stripped from the
  accepted operation. `Edit::RestyleAnnotation` is all-or-nothing — use it, not
  `Edit::Batch`, for anything that reaches the server, because a `Batch` has no
  rollback and a failed operation must leave the server unchanged.
- A state reply must be named by the client (`Edit::SetAnnotationStatusNamed`):
  the unnamed `Edit::SetAnnotationStatus` generates a random `/NM` per replica,
  so the replicas would disagree on the reply's address. The status text
  ("Accepted set by …") is the engine's; the wire `text` is only a plain reply's.
- Presence TTL is a client-side clock (`PRESENCE_TTL` in `ui-egui/src/collab.rs`):
  the server stores only the last state per participant and does not evict it, so
  "inactive" is drawn where presence is read. A merge intent re-addressed after a
  resync takes the new target epoch (`collab-client::with_epoch`); without it a
  retried comment was stale for ever.
