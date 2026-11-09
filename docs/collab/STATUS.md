# PdfCraft collaboration — status and handoff

- Branch: `feature/realtime-collaboration` (upstream `storytold/pdfcraft`)
- State: implemented behind the `collab` feature (off by default). **Rebased onto
  `origin/main` `8d5d39ef`** (2026-11-09; all 40 commits moved, no conflicts —
  main's `43f24aa1..8d5d39ef` delta touches 16 files and overlaps the branch
  only in `crates/ui-egui/src/app.rs`, in different hunks). The §6.6 gates were
  re-run at the rebased head (§4). **Pushed:** the force-push was done on
  2026-11-09; `gh pr view 6622` reports head `81035bf2` (equal to the fork head
  and to the local HEAD before this STATUS commit) and `mergeable: MERGEABLE`;
  the only reported check, CodeRabbit, is `SUCCESS` (2026-11-09T00:01:35Z).
  `mergeStateStatus` is `UNSTABLE` with that single green check and no required
  review. The PR body is the text of `pdfcraft/PR-DRAFT.md` from `## What` on
  (§6.6). This STATUS update is local only, not pushed.
  **§6.3 (web host bridge) is done locally in `81a92f32f`** — the transport trait,
  the browser transport and the five host messages (§4, §6.3) — and is not
  pushed either: the fork, PR #6622 and its body still describe `81035bf2`.
  Limitation 1 (crash recovery against the real process) is closed in
  `999da5ec` — a SIGKILL in the journal→base window, replayed on restart (§4);
  limitation 11 (commit publication outside the hub lock) in `a47641fb`. After
  the rebase those commits are `2dfae0fe` and `e043c132`.
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
| `crates/collab-session` | One protocol driver (`driver::Session`: hello/welcome, version and epoch, resync and retry, lease heartbeat, presence) behind a `Transport` trait with two implementations: `native` (a blocking socket on a worker thread, reconnect with backoff) and `web` (the browser's `WebSocket`, whose callbacks queue frames and which the frame loop steps). `Sink` is the object-safe face the UI holds |
| `apps/pdfcraft-collab-server` | Daemon (package `pdfcraft-collab-daemon`, binary `pdfcraft-collab-server`): dev mode from a directory, control plane `POST /v1/sessions`, commit callback to the cabinet, `/health`, `/metrics` |
| `crates/ui-egui/src/collab.rs` | App wiring behind `collab`, native and wasm: start from the environment (desktop) or from the host page, apply events, send edits, participants window, presence with cursors and TTL, commit, inverse undo, host reports. Holds `Box<dyn Sink>`, never a native `Handle` |
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
18. **A commit's cabinet call runs outside the hub lock.** `Room::commit` writes
    the base and returns a `Commit`; the transport runs its publication after
    releasing the lock and reports back with `Room::finish`. A second commit of
    the same document while one is publishing gets `busy` (retryable), so two
    versions cannot reach the cabinet out of order. Moving the call back inside
    `with_hub` makes one slow cabinet freeze every document in the process —
    `a_commit_does_not_block_presence_in_another_room` is the guard.

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

## 5. Known limitations (the prod gap)

Ordered by the risk they pose to production.

1. ~~**Crash recovery is not crash-tested against the real process.**~~ Closed
   by `apps/pdfcraft-collab-server/tests/crash_recovery.rs` at `999da5ec`
   (§4): the daemon accepts a merge and a structural operation over a socket,
   the commit parks between the journal record and the base write, the process
   is SIGKILLed, and the restart comes back at version 2 / epoch 1, returns the
   stored acknowledgement for the retried `opId`, and commits each edit once.
   The layout and the other two windows (base written but `CURRENT` not yet
   renamed, torn last line) stay covered at the `Store` level. A kill during
   the cabinet publication itself is still not crash-tested: it only leaves the
   room dirty and is retried by the next commit (limitation 6).
2. **Annotation coverage beyond the typed set.** `annotation.add` shares
   `TextMarkup` (kind/quads/colour), `Rectangle`, `Oval`, `Line`, `Ink`,
   `FreehandHighlight`, `TextBox`, `Typewriter`, palette `Stamp`, `Mark` and
   `Note` with their style (colour, opacity, width, fill, border). Drawn and
   typed signatures, custom stamps, redaction marks and attachments are refused
   with `unsupported` (RFC §3.1.1). Comment replies that carry a status
   (`/State`) and edits that only change an annotation's style are not shared
   yet.
3. **Structural coverage.** Only page rotate, move and delete. Insert,
   duplicate, replace, split/combine, bookmarks, layers, attachments, form
   structure and content edits are not shared.
4. **Presence has cursors, not selections.** The pointer over a page is sent and
   drawn (ADR-0006); a text selection, `/NM` or rect selection is not sent yet.
5. **Undo of a deletion is best effort.** A comment the room added comes back
   exactly; a comment that came from the file has no wire body, so its delete
   carries no copy and cannot be undone. A page comes back while its one-page
   copy fits `MAX_RESTORE_PAGE_BYTES` (160 KiB); a larger page is deleted without
   a copy. Copies are memory-bounded (64 / 64 MiB) and are rebuilt by replay, not
   persisted on their own.
6. **The cabinet contract is implemented, the production loop is not closed.**
   `POST /v1/sessions`, the commit callback and `409` exist (RFC §8.2). What is
   still missing: an idempotent retry/queue for a failed publication (today the
   next commit or the last-client-leave commit retries it), an endpoint to
   resolve a `conflict` room (the cabinet decides "new session over my version"
   or "save a copy" — the daemon only refuses), and `https` without a proxy.
   The shared cabinet contract also lets the cabinet number versions and return
   its own `{version, etag}` from the callback; v1 ignores that body and uses
   the room's numbering, so a cabinet with its own numbering needs an adapter
   (RFC §8.2 says so explicitly).
7. **No TLS in the daemon.** WSS, the control plane and the callback are
   expected to sit behind a reverse proxy; `https://` source URLs and callback
   URLs are refused with a clear error. No per-IP throttling, no session
   TTL/eviction besides the lease TTL.
8. **Large-document budget.** 500 pages are tested functionally; the 100k-page
   inspection cost and batching remote applies are not measured. The control
   plane caps a file at `--max-source-bytes` (256 MiB) but nothing else.
9. **No automation tool/parity entry yet.** The feature is behind a flag; when
   it becomes user-facing, `crates/automation` and `parity/` need an entry.
10. **Journal growth is bounded only by commits.** A room that never commits
    keeps every accepted operation on disk; the last-client-leave commit and the
    manual button are the only compaction. A size-based forced commit is not
    implemented. Bases are pruned to the one `CURRENT` names, so disk is one
    PDF plus the journal, not one PDF per commit.
11. ~~The callback is synchronous inside the hub lock~~ — the base is written
    under the lock and the cabinet POST runs after it is released (§8.2): a
    cabinet that does not answer delays the committer's connection, not the
    other documents. A second commit of the same document while one is
    publishing gets `busy` (retryable), so two versions cannot reach the
    cabinet out of order. What is still missing is a publication *queue*: a
    failed POST is retried by the next commit or the last-client-leave commit,
    not in the background.
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

Still open here: a size-based forced commit (limitation 10). The fault
injection limitation 1 asked for is done for the real process in the
journal→base window (`apps/pdfcraft-collab-server/tests/crash_recovery.rs`,
`999da5ec`, §4); a kill between the base write and the `CURRENT` rename is still
covered at the `Store` level only.

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
- Still open here: publication retry/queue, an endpoint to resolve a conflict
  room, `https` without a proxy (limitations 6–7).

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
  publication actions for the owner.
- **The PR body was replaced** on 2026-11-09 with the text in
  `pdfcraft/PR-DRAFT.md` (from `## What` on); the body on the PR is byte for
  byte that text plus CodeRabbit's appended summary block. Every claim in it
  describes the feature at the pushed head `81035bf2`; the rebase only changed
  the base, not the features it describes. What it fixes: cursors and their TTL,
  the typed annotation set, undo of deleted comments and pages from server
  copies, the control plane, the commit callback running outside the hub lock,
  and the SIGKILL crash-recovery test.
- Remaining limitations stated in that body: no web client/host bridge
  (§6.3) — **since closed locally in `81a92f32f`, not yet in the pushed head or
  that body** — no publication queue (a failed cabinet POST is retried only by
  the next commit or the last-client-leave commit), no endpoint to resolve a
  `conflict` room, WSS/TLS behind a reverse proxy, page restore capped at
  160 KiB, text selections not shared, and signatures / custom stamps /
  redaction / attachments / page insert-duplicate-split refused as
  `unsupported`.
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
  `busy`.
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
