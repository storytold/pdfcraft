# PdfCraft collaboration — status and handoff

- Branch: `feature/realtime-collaboration` (upstream `storytold/pdfcraft`).
- Merge: **`origin/main` is merged into the branch** six times, all true merges
  — no rebase and no force. `c15cc0f57` (2026-11-10) merged `debb826f2` ("M10:
  preserve annotation placement precision (#4995) (#5002)") when the branch had
  been rebased onto `ba28458ad` (§4 records that rebase and its four conflict
  stops); one file conflicted, `crates/annot/src/tests.rs`, where both sides
  append tests at the end, and both were kept unchanged. `Cargo.lock`
  auto-merged there with `craft-print` on main's `0e4943cb…` and
  `craft-web-host` on the branch's storytold pin `7666e2b1…`. `afd28a2f9`
  (2026-11-10) merged `92b965dba` ("M10: preserve annotation layers in vector
  print output (#5019)") with **no conflicted file** (40 files, +3831/−224):
  main never touched `crates/annot/src/tests.rs` after `debb826f2`, so the
  branch's version — which already keeps both sides — merged as it was.
- The third merge was: `b21b57b55` (2026-11-11) merges `ad3e6d4b8`
  ("fix(optimize): bound each unused-resource decode by the remaining shared
  budget (#6046)"), the tip at the moment of the fetch and 21 commits above
  `92b965dba`. 75 files, +3670/−192: a new `vendor/edit-session` crate, the
  document-properties sessions and their scenarios, main's JavaScript field
  colour work (`#5499`, `#5505`) and a `nightly-macos.yml` workflow. Two
  conflicts. (1) Textual, `crates/engine/src/tests.rs`: both sides append tests
  at the end — the branch's page/comment-reference tests and main's JavaScript
  field-colour test — and **both are kept**. (2) Semantic, merged silently by
  git: `ad3e6d4b8` adds a second `Stream::decoded_strict_within` beside the one
  `03366c3c7` added, so **main's own tip does not compile** (E0592 at
  `crates/cos/src/object.rs:458`; its push run `42918717889` failed in `gate`),
  and nothing could be taken from it as-is. The resolution keeps the newer,
  counted definition — the remaining-budget semantics its doc comment and its
  new test name — and drops the older duplicate; that hunk was a fix to main's
  own tree and was to be removed once upstream fixed the duplicate, which
  `d2916decb` (the next paragraph) did. Nothing else
  in the merge needed an edit; `Cargo.lock` auto-merged and
  `cargo metadata --locked` exits 0. After the merge the branch is **117 commits
  ahead and 0 behind** `ad3e6d4b8`, and 118 after that record.
- The fourth merge was: `84da974d9` (2026-11-11) merges `d2916decb`
  ("packaging: use `[[` for bash tests (SonarCloud S7688) (#6932)"), the tip at
  the fetch and 3 commits above `ad3e6d4b8`. Those three are `9345b5caa`
  (#6933) and `25f3f5429` (#6934) — both "remove the duplicate
  `decoded_strict_within` that broke main after #6046" — and the packaging
  commit, so **main fixed its own duplicate** and the hand-written fix the
  previous merge had to carry is no longer needed. Against its first parent the
  merge is two files, +5/−4: `packaging/freebsd/package.sh` (main's change) and
  `crates/cos/src/object.rs`. That file conflicted once more, because the
  branch's resolution and main's fix differ in shape: the branch's
  `decoded_strict_within` delegates to `decoded_strict_within_counted`, main's
  keeps the `decode` closure. The merge keeps the branch's delegation — one
  decode path, the counted one — and the difference is recorded rather than
  hidden: `git diff origin/main HEAD -- crates/cos/src/object.rs` is 4 lines (a
  doc comment and the body), the branch's only delta from `main` outside the
  feature, and it can be removed by taking main's text verbatim if a reviewer
  wants the smallest diff. After the merge: **119 ahead / 0 behind**
  `d2916decb`, and 120 after this record. `Cargo.lock` did not move and
  `cargo metadata --locked` exits 0.
- The fifth merge is `b7545a7a9` (2026-11-11 09:16Z): it merges `4efcfa6ea`
  ("fix(crypt, engine): refuse page extraction when Protect allows any change
  except extracting pages (#5081)"), the tip **13 commits** above the branch's
  last merge base `d2916decb` — every commit from `41d959d94` to `4efcfa6ea`
  (`#2961`, `#5082`, `#5080`, `#2672`, `#5083`, the comment-anchor fix,
  `#5079`, `#6955`, `#5699`, `#6919`, `#6918`, `#6910`, `#5081`). Three files
  conflicted, and each was resolved by keeping both sides:
  `crates/engine/src/edit.rs` (the `use crate::{…}` list, where main dropped
  `open_source` — its remaining callers moved to `edit/run.rs` — and added
  `Edit::SetFieldImageTarget` to the enum), `crates/engine/src/history.rs`
  (main's `ResetForm` and `SetFieldImageTarget` arms beside the branch's
  `SetPageLinks | SetPagesLinks` one, in both `uses_scripts` and `scope_of`)
  and `crates/engine/src/tests.rs` (both sides append tests at the end; both
  kept, no duplicate names). Against its first parent the merge is **120 files,
  +10863/−1139**; `Cargo.lock` moved 5 lines (main's `craft-print` pin
  `60ede999…` and the two `winprint` revs). After it the branch was **122
  commits** not in `origin/main` and 0 behind `4efcfa6ea`.
- The sixth merge is `95e2c9c97` (2026-11-11 10:26:52Z), made by the parallel
  PR-sweep session while this pass was measuring the fifth: it merges
  `3a117c3c0` ("M10: open Word, Excel and PowerPoint files through
  LibreOffice"), the tip ten minutes later, and it is **30 files, +1297/−126**
  against its first parent. Two files conflicted: `Cargo.lock`, re-resolved by
  `cargo metadata` (5 lines against the fifth head — `rustix` 1.1.6 and the
  `windows-sys` split the new dependency brought), and
  `crates/engine/src/session.rs`. That second resolution left two defects, both
  the resolution's and neither main's nor the branch's: a blank line at the head
  of the `use crate::{…}` list (`session.rs:10`), so `cargo fmt --check` fails,
  and the `BookmarkRow` import (`session.rs:14`), which main's tree no longer
  uses natively, so `cargo clippy --workspace --all-targets -- -D warnings`
  fails with `unused import`. At `b7545a7a9` both steps are green (§4), which is
  the control. The parallel session pushed it at 10:28:56Z, so PR #6622 carries
  that head. **Both defects are fixed in `69b2a6dad`** (§4, "The two red steps
  of `95e2c9c97` fixed"), which was the head until `46d2e203a` re-based the line
  on v0.21.0; the fork and the PR carried `95e2c9c97` only until `28fd4e1b3` was
  pushed at 17:41Z.
- Delivered head: **`eb60bc5ff`** ("docs(collab): the delivered head is
  `6ac4e1130` — the owner pushed it, the runs await approval"). **Superseded on
  2026-11-12**, and again at 16:20Z of that day: `gh pr view 6622` now reads
  `headRefOid` `2a2b0c1571e5635a77f3ac90c83f2c89fdd4fc36` (the merge of
  `origin/main` `d749fd806`), with the fork at the same commit; `e7a056501`
  (the docs commit above `eb60bc5ff`) and `eb60bc5ff` are its ancestors, and
  `fee60000b` is the one local commit above it (header's snapshot bullet). Everything below about
  `eb60bc5ff` describes its own delivery and remains true of the runs measured
  there; the PR head moved by one docs-only commit. Measured
  2026-11-11T23:37Z, before the commit that carries this record: PR #6622's
  `headRefOid`, the fork `copoka/pdfcraft`'s branch tip (`git ls-remote fork
  refs/heads/feature/realtime-collaboration`) and this clone's HEAD all read
  `eb60bc5ff`; `git rev-list --left-right --count
  fork/feature/realtime-collaboration...HEAD` reads `0 0`, and the tree is
  clean.
  This record adds one docs commit (`docs/collab/STATUS.md` alone) on top of it
  and that commit is local — the agent's `git push*` denial still stands, while
  the delivery to the PR is done, by the owner or the PR-sweep pass.
  **`6ac4e1130` is no longer the fork head**: it is the previous delivered head,
  one commit below `eb60bc5ff`, and the earlier version of this record that
  named it the delivered head and itself local was right at 20:08Z and is
  superseded by the push of 21:10Z ("Delivery", below).
  Under `eb60bc5ff`: the code head
  `0d3ea2708` (the cabinet's `expectedVersion` as its etag, §4), then three
  docs-only commits — `4d9a42819`, `6ac4e1130` and `eb60bc5ff` — on
  `28fd4e1b3` (the lock records `web-time` for the collab client) on
  `46d2e203a` (merge `origin/main` `3650f01f2`, v0.21.0) on `95e2c9c97` …
  `69b2a6dad` on the rebased branch.
  **All three commits above the code head are docs-only** (`docs/collab/STATUS.md`
  alone), so the gate measured on `0d3ea2708` is the gate of `eb60bc5ff`
  (§4). Counts, measured 2026-11-11T23:37Z after `git fetch origin main`:
  **135 commits** not in `origin/main` (`git rev-list --count
  origin/main..HEAD`; 136 with this record) and **22 behind** it — `main` has
  moved to `80e26e674`, and `3650f01f2` stays an ancestor of HEAD;
  `git diff --shortstat origin/main HEAD` is **158 files changed, 44152
  insertions(+), 2330 deletions(-)** (the deletions and the extra files are
  main's 22 commits, not the branch's). The counts the earlier briefs named —
  134/12 at `6ac4e1130`, 132/0 at `0d3ea2708`, 125/0 at `69b2a6dad` with 108
  files/+43521, 122/0 at `b7545a7a9`, 119/0 at `d2916decb`, and the pre-merge
  series in the paragraph above — were right when measured, against the
  `origin/main` of their time; `46d2e203a` re-based the line on v0.21.0 and the
  commits on top add the contract fix and its records. Commits named below in
  §4 by their pre-merge hashes are mapped there.
- Delivery, as measured 2026-11-11T23:37Z: PR #6622's `headRefOid` is
  **`eb60bc5ff`**, and so is the fork `copoka/pdfcraft`'s branch tip (`git
  ls-remote fork refs/heads/feature/realtime-collaboration`) and this clone's
  HEAD, with a clean tree. This record is one docs commit on top of
  `eb60bc5ff` and is local; the delivered head stays `eb60bc5ff` until the
  owner or `pr-agent` pushes the branch again. `gh pr view 6622` reads
  `mergeable: MERGEABLE`, `mergeStateStatus: UNSTABLE`, `baseRefOid`
  `3650f01f2` (v0.21.0), not a draft, `reviewDecision` empty, `updatedAt`
  2026-11-11T21:10:56Z, and one check (CodeRabbit, `skipped`); the JSON's
  `commits` array lists 100 entries, which is the API's page cap, not a count.
  **Five runs exist on `eb60bc5ff`**, all created 21:10:52–53Z — CI
  `43100841697`, SonarCloud `43100841668`, Nix `43100841572`, FreeBSD
  `43100841526`, Windows ARM64 `43100841785` — and every one of them is
  `completed` with conclusion **`action_required`**: a fork PR's workflows wait
  for a maintainer's approval, so **none of them has executed** (no job
  started, no step, no log) **and there is still no CI result on the PR head**.
  The five runs the earlier record listed — CI `43085349704`, SonarCloud
  `43085349832`, Nix `43085349544`, FreeBSD `43085349527`, Windows ARM64
  `43085349547`, created 19:14:33Z on `6ac4e1130` — belong to the previous
  head and are superseded, not failed. The push that moved the fork from
  `6ac4e1130` to `eb60bc5ff` **was made by the owner or the PR-sweep pass, not
  by this agent**: the PR's `updatedAt` and the run times both read 21:10Z,
  while this agent's permission still denies `git push*` (unchanged since the
  first pass). Nothing in this file was pushed by the agent. The record further
  down that says the push did not happen was true at 18:20Z, when the fork
  still read `28fd4e1b3`; it is history now — `0d3ea2708` and the three docs
  commits are delivered with `eb60bc5ff`. The gate evidence remains
  the host sweep on `0d3ea2708` in §4 ("The cabinet's `expectedVersion` is its
  etag"), which stands for `eb60bc5ff` because all three commits above the code
  head are docs-only.
  `pdfcraft/PR-DRAFT.md` now describes the same snapshot (PR head
  `2a2b0c157`, local `fee60000b`, the gate in §4); it is a draft whose facts are
  current as of 2026-11-12T16:20Z.
- State: implemented behind the `collab` feature (off by default). **`CI=1
  cargo xtask ci` exits 0 on the head `f107f56c2`** — all 10 steps, with the
  numbers at the end of §4, "Save in a hosted session…" — and on `fee60000b` —
  all 10 steps, **10380 passed, 0
  failed, 22 ignored** (§4, "The cabinet's Save reaches the room") — and on the
  earlier `0d3ea2708` — all 10 steps, 157 targets (111
  `Running` + 46 `Doc-tests`), **10309 passed, 0 failed, 22 ignored** (§4, "The
  cabinet's `expectedVersion` is its etag"). The gate on the previous head
  (`69b2a6dad`, before `46d2e203a` merged v0.21.0) was 10225/0/22; the 84 extra
  tests are the merged release's and the commits on top. On `69b2a6dad` the
  same command was green with 155 targets as the
  log lists them (109 `Running` + 46 `Doc-tests`), **10225 passed, 0 failed,
  22 ignored** (§4, "The two red steps of `95e2c9c97` fixed"). That is the head
  the fix made; the same command on the delivered `95e2c9c97` still stops at the
  first step — `fmt` on the blank line the sixth merge left, `clippy` on the
  unused `BookmarkRow` import — and the other eight steps pass there one by one
  (§4), which is why the two steps and not the other eight were the task. On
  `b7545a7a9`, the control, all ten are green with 10202 passed / 0 / 18. The
  23 tests and the 4 extra ignored ones over that control are `3a117c3c0`'s
  LibreOffice work; on `b7545a7a9` the totals are 175 above `b21b57b55`'s
  10027/156/18, which is main's thirteen commits' own tests. The sweep
  needs `CRAFT_I18N_DIR` (a craft-i18n worktree at `575cfdef`, the revision
  `craft-i18n.rev` pins) and `CRAFT_FONTS_DIR` (`b076417b`): without the
  catalogs the comments test panics at `set_option("language", "de")` — an
  earlier pass ran exactly that and got EXIT=1 — which is the documented cause,
  not a regression. The two `test` targets that stopped an older sweep stay
  closed (§4). The reasons CI run `42720349704` (head `b27fa89c8`) failed stay
  closed: the git source (§4, "The CI git-source gate") and the `deny` advisory
  RUSTSEC-2026-0387 (§4, "The JWT crypto backend") — `cargo deny --log-level
  error check` exits 0 and `crypto-bigint` 0.5.5 is out of the graph. The
  workspace `clippy` dead-code fix (`f48ec4754`) is in too. The earlier gates
  were run at their own heads (`5bad54c40`, `157f4d239`, `22d29d070`,
  `0c57e2271`, `2b61ff80f`, `642dabb5b`, `158ab7fa4`, §4) and were not re-run
  here.
- What is built, all recorded in §4 and pushed to the fork in earlier passes: the web host bridge
  (`81a92f32f`), the forced journal commit (`8fc898bae`), the idle/time
  threshold (`2a3b6e43e`), crash recovery against the real process
  (`999da5ec`), the commit publication outside the hub lock (`a47641fb`),
  restyle and status replies (`ce5c38798`), the publication queue
  (`a639f30c0`) and its recovery after a restart plus the resolve's own
  publication (`21aaf357a`, §4), presence selections (`b4e9179d8`) and their second slice —
several `/NM`, per-line quads, the caret (`5bad54c40`, §4), re-addressed across a turn
  and a catch-up and cleared with a dropped session (§4, 2026-11-12) — the conflict-resolution
  endpoint (`62d48d30a`), line endings/font size/info (`c400147b3`), page
  replace/blank insert (`e9756d138`), page insert/duplicate (`b13933e9b`),
  page split/combine (`362f9c72b`), the Split/Combine dialogs sending them
  inside a session (§4), and the split part kept as a document plus one Combine
  confirmation as ordered parts (`843a8f533`, §4), and the cabinet's HS256
  session tickets next to RS256 (`157f4d239`, §4), and a comment's parts, a
  move, a resize and a reshape as addressable operations (`22d29d070`, §4),
  and its pop-up window as one too (`0c57e2271`, §4), and the outline with its
  named destinations as eight structural operations (`d1229deb9`, §4), a
  bookmark's look in `outline.item.update`/`add`, the named-destination
  interface and the `bookmark_set_style` open state (`f262cb8d5`, `628f6176e`,
  §4), the page labels as two structural operations (`c26867e9c`, §4), links as
  two structural operations over a page's whole link list (`2b61ff80f`, §4),
  and the whole-document link actions as `links.add_many`/`links.remove_many`
  over several pages at once (`b86c4963a`, §4). The fork carries all of it up
  to `95e2c9c97`, including the earlier rebase onto
  `origin/main` `ba28458ad` (§4, first table) and the merges of `debb826f2`,
  `92b965dba`, `ad3e6d4b8`, `d2916decb`, `4efcfa6ea` and `3a117c3c0`. Nothing
  in the feature waits for a push any more: the docs commits on top of the
  earlier merges went out with the branch (`ed8d11115` in an earlier pass,
  `95e2c9c97` before this one), `28fd4e1b3` with the v0.21.0 merge, and
  `0d3ea2708` plus the three docs commits, the last two in the pushes of 19:14Z
  and 21:10Z (owner or `pr-agent`), so PR #6622 carries `eb60bc5ff` — see
  "Delivery". The one
  presence item still outside
  this repository is the host page's `pdfcraft:presence` payload:
  `craft-web-host` must gain optional `page`/`selection` on its `Participant`
  (§5 p.4, §4).
- Known gaps before production are listed in §5, the plan in §6. The pass
  before this one added no feature work: it fixed the two steps the sixth merge
  left red.
  `69b2a6dad` drops the blank line and the unused `BookmarkRow` from the
  `use crate::{…}` block in `crates/engine/src/session.rs`, with rustfmt's
  rewrap of the list, and the whole sweep re-run on that head exits 0 — all ten
  steps, 10225 passed (§4). This pass is the cabinet contract change and its
  record (§4); it
  leaves the encrypted-PDF policy
  (§3, RFC §12), the `https`-without-a-reverse-proxy note, the in-process
  `conflict` flag and the journal tail as they were. A kill between the base
  write and the `CURRENT` rename is covered at the `Store` level only (§5 p.1,
  §6.1); the real-process SIGKILL test covers the journal→base window.
  The two passes after that one are docs-only web-path evidence and this
  selection acceptance pass; both are recorded at the top of §4.
- The pass before this one was docs only: the web path's browser run is recorded
  in §4 ("The web path in a browser: two iframes, one room", 2026-11-12). It
  built this branch's `apps/pdfcraft-web` with trunk on the build host, ran a dev
  room and a cabinet-like page on loopback, and drove two iframes over CDP: both
  joined, the owner highlighted page 1, the editor noted page 2, each side saw
  the other's annotation, and `commit` → `committed` closed on the app's status.
  No code changed; §1's trunk sentence and §6.3's "not exercised" line follow
  from it. Nothing was pushed.
- This pass closes the remote-selection leftover named at §6.6: the range is
  addressed per line, carries the author's caret, and is re-addressed to the new
  epoch after a turn or a catch-up replay. The feature itself was already in
  `5bad54c40` (an ancestor of HEAD); this pass added the acceptance tests the
  brief named and two client rules — keeping the selection across `page.rotate`
  and a snapshot tail (`DocView::document_keeping_selection`), and clearing the
  participant list on `Disconnected`/`Joined` so a dropped socket draws no old
  cursors or selections. §4 records it, §5 p.4 and §6.6 are updated, RFC §5 now
  states how a turn/resync re-addresses a range and that the protocol has no
  `leave` message (a silent peer is the 30 s TTL). The code commit is
  `00c06ed5c`; code and docs are local, the push is the owner's.
- This pass closes the last selection leftover: presence now addresses the form
  field being typed into by its stable name and the caret's offset inside its
  text, not only the field as an object (§4, "Form-field carets in presence",
  2026-11-12). `Selection::Text` gains an optional `field: {name, offset}`; the
  widget's box and the caret's user-space point still travel, so an older peer
  keeps drawing the place. The editor records the offset and point each frame
  from the galley it draws and clears them when the keyboard leaves, a page turn
  re-addresses the frame to the new `epoch`, a field or page the form no longer
  holds is not sent, and a password shares no caret. The code commit is
  `c935ebadb`; docs follow on top. **Measured in this pass:** PR #6622's head is
  still `e7a056501` (docs-only above `eb60bc5ff`), and `git rev-list
  --left-right --count e7a056501...HEAD` reads `0 4` — `ac33dd212`,
  `00c06ed5c`, `c56da220b` and `c935ebadb` are the only commits above it. The
  etag fix `0d3ea2708` named in this pass's brief as unpushed is **already
  delivered**: `git merge-base --is-ancestor 0d3ea2708 e7a056501` succeeds, it
  is an ancestor of the PR head, so only this pass's two commits are new. Nothing
  was pushed by this agent; the push is the owner's.
- Snapshot this file is written to (measured 2026-11-12T17:54Z with `git` and
  `gh pr view 6622`): the **head is `f107f56c2`** — "Save in a hosted session is
  the cabinet's commit, not a copy of the window" (3 files, +94/−1; §4) — with a
  clean tree, and the fork `copoka/pdfcraft` and PR #6622 both still carry
  **`2a2b0c157`**; `git rev-list --left-right --count
  fork/feature/realtime-collaboration...HEAD` reads `0 6`. The six commits above
  the delivered head, oldest first:
  1. `fee60000b` — code: the room-side commit for the cabinet
     (`POST /v1/sessions/{fileId}/commit`) and an etag the room can compare
     (`CABINET.sha256`); 8 files, +571/−45 (§4);
  2. `d1d304231` — docs: the record of that commit and its etag;
  3. `ad4f33787` — docs: the branch's snapshot in this file and in
     `PR-DRAFT.md`, and the one commit the PR was missing;
  4. `24f519a91` — code: a closed socket no longer takes the seat, the role and
     the lease of the same actor that still has a live connection
     (`crates/collab-hub/src/lib.rs`, +34/−1; §4, "A closed socket no longer
     takes the seat");
  5. `826303ace` — docs: that room defect, its WebKit/Chromium reproduction, the
     room binary that carries the fix, and the app's byte Save, which at that
     point still wrote behind the room;
  6. `f107f56c2` — code: that Save trap closed — in a live session a host page
     commits, so `file.save` (toolbar, menu, Ctrl+S) and the page's own Save
     action call `collab_commit` instead of writing this window's bytes
     (`crates/ui-egui/src/{collab,commands,editing}.rs`, +94/−1; §4).
  The PR is `MERGEABLE`/`UNSTABLE`, not a draft, `reviewDecision` empty,
  `baseRefOid` `d749fd806`, `updatedAt` 2026-11-12T06:18:30Z, and every workflow
  run on the fork reads `action_required`, so nothing has executed on GitHub and
  the gate evidence is the host sweep in §4, not a CI run. `origin/main` has
  moved to `120c62fb8` (three commits: `#7069`, `#7034`, `#7031`); the branch is
  **148 ahead and 3 behind** it, `git diff --shortstat origin/main HEAD` is
  **131 files, +46076/−2012**. Merging those three is a separate decision and
  has not been done. The owner's steps are the push (`git push fork
  feature/realtime-collaboration`), the run approvals, the review and the manual
  desktop acceptance; nothing else is owed on the code side.
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
| `crates/collab-pdf` | PDF adapter over `engine`: annotations (add, edit, restyle with `/LE`, font size, author/subject/icon, parts, move, geometry, pop-up, reply with or without `/State`, delete, restore), form values, page rotate/move/delete/insert/duplicate/replace/blank insert/split/combine, the outline (item add, update of its title, destination and look — colour, weight, slant, open state — delete with its whole subtree, restore, move — addressed by the item's object key; the whole outline replaced in one operation, with the prior tree as its undo image), named destinations (add, update, delete — addressed by name), page labels (set, clear — addressed by the page a range starts on) and links (set, clear — a page's whole link list), restores from server-side copies of deleted comments/pages and the copy a replacement keeps for its undo; durable `Store` (versioned bases, journal, `CABINET` marker carrying the room's version and the cabinet's) |
| `crates/collab-outline` | Typed wire form of an outline item's destination (a page and a point, a Table 149 view, a named destination, or nowhere), of the look an update or an add names (`ItemLook`: colour as three numbers from 0 to 1 or `null` for the default black, bold, italic, open state) and of the subtree copy a delete carries, with their bounds: title and name length, depth, node count, copy size, colour channels, view types and their values; the tree the from-structure/from-text-styles generators build in a replica |
| `crates/collab-link` | Typed wire form of a link: its area (a rectangle, or the quads of a text area), its target (a page of this document with a view, a named destination, a page of another file, or a web address) and its look, with the bounds of every number and string; a list of them is a page's whole link list |
| `crates/collab-annot` | Typed wire form of a shared annotation: `shape` (kind, geometry) + `style` (colour, opacity, width, endings, fill) + text, with the v1 subset and its limits; the final style of an existing comment (`StyleUpdate` for `annotation.update`), its font size (`FontSizeUpdate`), its author/subject/icon (`InfoUpdate`), its parts (`PartsUpdate`), its move (`MoveUpdate`) and its geometry (`GeometryUpdate`) and their before/expect CAS; the review-state names of a status reply |
| `crates/collab-ws` | Blocking WebSocket transport: `hello` + RS256/HS256 JWT, `Origin`, frame limits, per-connection outbox/backpressure, `/metrics` snapshot, the control plane's `resolve_conflict` and its counter (which publishes the refused base), the publication queue and its recovery of restored publications |
| `crates/collab-client` | Client state machine: pending ops (≤8), version/epoch, gap → resync, retryable rejection → resync then resend, reconnect |
| `crates/collab-client-pdf` | Projection: `Op` → engine `Edit` (apply with `Session::apply_remote`), `Edit` → `Op` (a bookmark edit — title, destination, colour, style or open state — is one `outline.item.update` addressed by the item key this replica's list gives it; a whole-outline `ImportBookmarks` with `replace` is one `outline.replace`; the structure and text-style generators build their tree here and send the same operation; a link edit is addressed by the page's whole list), inverse operations for undo, including the outline, named destination and link operations, whose inverses are built from the accepted operation |
| `crates/collab-session` | One protocol driver (`driver::Session`: hello/welcome, version and epoch, resync and retry, lease heartbeat, presence) behind a `Transport` trait with two implementations: `native` (a blocking socket on a worker thread, reconnect with backoff) and `web` (the browser's `WebSocket`, whose callbacks queue frames and which the frame loop steps). `Sink` is the object-safe face the UI holds |
| `apps/pdfcraft-collab-server` | Daemon (package `pdfcraft-collab-daemon`, binary `pdfcraft-collab-server`): dev mode from a directory, control plane `POST /v1/sessions`, `POST /v1/sessions/{fileId}/commit` (the host's Save: the room builds the document and publishes it, answering `{version, cabinet:{version,etag}}`) and `POST /v1/sessions/{fileId}/resolve`, commit callback to the cabinet with an idempotency key (its `{version, etag}` answer is adopted), the background publication queue with a backoff, the start-up recovery of unpublished bases from the store, `/health`, `/metrics` (`pdfcraft_collab_forced_commits_total`, `pdfcraft_collab_publication_retries_total`, `pdfcraft_collab_publications_restored_total`, `pdfcraft_collab_publications_after_resolve_total`, `pdfcraft_collab_conflicts_resolved_total`) |
| `crates/ui-egui/src/collab.rs` | App wiring behind `collab`, native and wasm: start from the environment (desktop) or from the host page, apply events, send edits, participants window, presence with cursors, selections and TTL, commit, inverse undo, host reports, Save in a hosted session routed to the cabinet's commit (`commit_instead_of_save`), and the Split/Combine dialogs' `page.split`/`page.combine` inside a session. Holds `Box<dyn Sink>`, never a native `Handle` |
| `crates/ui-egui/src/dialogs/named_dests_ui.rs` | Bookmarks ▸ Named destinations…: the document's names with the page and view each opens, and add/update/delete as engine edits (`SetNamedDestination`/`DeleteNamedDestination`), so a session sends `dest.add`/`dest.update`/`dest.delete` and the dialog never writes the document itself |
| `crates/automation/src/named_dests.rs` | Headless parity for named destinations: `destinations` (name → page and view), `destination_set` (add or move a name, with the Table 149 type and its values checked before the write) and `destination_delete` |
| `apps/pdfcraft-web/src/main.rs` | Web shell: the host bridge for collaboration (`pdfcraft:session` in; `session-state`, `presence`, `commit` out; `committed` back) over `craft-web-host` |
| `crates/engine` | `Session::apply_remote` (collaborative edits without the local snapshot undo); `Edit::AddBookmarkWith`/`UpdateBookmark` (title, destination and look in one step) /`SetNamedDestination`/`DeleteNamedDestination` with `BookmarkDest`, `Document::bookmarks()` (paths with the item's object), `Document::named_destinations()` and `Document::bookmark_color()` (a colour at the file's own precision, for an undo), `Document::outline_tree()`/`structure_bookmarks()`/`text_style_bookmarks()` for the generators a collaboration client runs in its replica; `Edit::SetPageLinks` over a page's whole link list, with `LinkContent` (`LinkBody`), and `LinkAction::Named`/`Remote` with the quads a text link carries |

Both platforms run the same protocol: `collab-session` compiles on wasm32 with a
`web-sys` `WebSocket` transport, and `ui-egui --features collab` compiles for
wasm32 (§4, §6.3). `collab-ws` stays native-only (it is the server's transport);
`collab-client`, `collab-client-pdf`, `collab-annot` and `collab-outline` are wasm-safe. The web
build enables the feature with `trunk build --release`: the page turns `collab`
on for its own asset through `data-cargo-features="collab"` in `index.html`,
because a `--features` flag on the trunk command line reaches the OCR worker
too and that crate has no such feature (§4, D4 item 1).

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
    `--callback-url`, where `expectedVersion` is the **etag** of the cabinet's
    copy — a string, absent on the first commit of a session
    (INTEGRATION.md §5.2, the body DeckCraft's room sends). Only a `2xx`
    advances the `CABINET` marker; its body `{version, etag}` (the shared
    contract) is the copy the room adopts: the `etag` is the next
    `expectedVersion`, the `version` the cabinet's numbering of it. An empty
    body falls back to `sha256:<digest>` of the published bytes and to one more
    than a numeric `expectedVersion` (this cabinet's etag is its version tag),
    else to the room's version. A `409` refuses the commit (the room is flagged
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
24. **A page's links are one list; a link's remote page number is not ours.** A
    link annotation has no name of its own and its index in `/Annots` moves
    under any page operation, so the room addresses a page's links as a list:
    `page.link.set` names the whole list and `page.link.clear` removes one
    entry from it. That is what makes the undo image exact — the list the page
    had, carried in the accepted operation — and a delete of one link does not
    need to guess an index another replica may have moved. A link that did not
    change keeps its annotation object, so a rewrite only touches what it must.
    A remote go-to's `/D` is a page number of *another* file: page operations
    of this document renumber the numbers that are ours and must leave those
    alone (§4). A page that holds a link whose action the model cannot write
    back (JavaScript, Launch) is refused whole with `unsupported`, rather than
    rewritten without it: a silently dropped destination is not an undo.
25. **A dialog that changes the document asks the session first.** Split and
    Combine call `collab_shares`/`collab_document` and route through the same
    session methods as edits; the file workflow is the fallback, never a
    parallel path — otherwise a user in a session would get files the shared
    document never sees, and the room would not know about the change. Combine
    focuses the session's document before it closes the Combine tab, because
    the applied broadcast is delivered to the active view.
26. **The JWT backend is `aws_lc_rs`, and it is not a preference.** jsonwebtoken
    11.1's `rust_crypto` feature drags `p256`/`p384` 0.13 → `elliptic-curve`
    0.13 → `crypto-bigint` 0.5.5, which is RUSTSEC-2026-0387 (timing branches in
    constant-time arithmetic, fixed in 0.7.6); the tickets this verifies are the
    cabinet's session tokens, so an ignore entry would keep the vulnerable code
    in the binary. `aws_lc_rs` is the only provider feature that does not, this
    stack is native-only, and the same binaries already link aws-lc-rs through
    `pdfcraft-sign` (ADR-0009). Switching back, or enabling both features,
    either brings the advisory back or makes jsonwebtoken refuse to pick a
    provider — it panics with "Could not automatically determine the
    process-level CryptoProvider" when both or neither is on. A repin is not
    available either: `jsonwebtoken` 11.1.0 is the newest release and pins
    `p256 = "0.13.2"`.
27. **A Save in the host's page asks the room to publish; a tag that moved onto
    our own bytes is adopted, not refused.** The page's `pdfcraft:commit` is
    answered by `POST /v1/sessions/{fileId}/commit`: the room builds the
    document it holds and publishes it through the same callback, and a clean
    room is answered from its marker instead of publishing again (the same
    assembly, so a repeated Save cannot create a second version). Do not build
    the file in the cabinet for this class of room: the room's `apply_remote`
    state is the document, and a second assembly path would be a second truth.
    The tag the control plane compares is the cabinet's own opaque etag
    (`{etag, version}` in the request, `CABINET.etag` in the room), never a
    digest of the bytes — comparing a digest with a version refused every
    reopen from the first accepted commit on. `CABINET.sha256` (the room's
    digest of the base it published) is what lets a moved tag be recognised as
    our own commit whose answer was lost: then `Room::adopt_cabinet` moves the
    marker and the session continues; foreign bytes stay `409`. An operator's
    `resolve` carries no bytes, so it cannot prove adoption and keeps the
    refusal; a marker written before the field existed does the same.

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
  `{"fileId","jwt","etag"?,"version"?,"bytes"|"initialBytes"|"url"}` →
  `201`/`200` with `{"fileId","version","epoch","etag","created"}`; `401` bad
  token, `403` token for another file, `400` bad body/etag, `409` the room
  holds another etag, `404` no room and no source, `413` file too large, `502`
  the source URL failed. `fileId` is restricted to `[A-Za-z0-9._-]{1,128}`.
  `etag` is the cabinet's own opaque tag of the copy it holds (this cabinet's
  is the file's version tag), not a digest of `bytes`, and `version` is the
  cabinet's numbering of that copy; without them the room falls back to the
  digest of `bytes` and cannot tell a newer cabinet tag from an external
  change. A tag that moved onto bytes this room published is our own commit
  whose answer was lost and is adopted; foreign bytes stay `409`.
- `POST /v1/sessions/{fileId}/commit` (`--control`, loopback, the cabinet's
  session ticket in `Authorization`) — the room-side Save: the room builds the
  document it holds and publishes it through the commit callback, and the
  answer names both versions. `200` `{"fileId","version","cabinet":{"version","etag"}}`
  (a clean room is answered from its marker, so a repeated Save creates no
  second version); `401` no/bad token, `403` another file or a role that cannot
  write, `400` bad `fileId`, `404` no room, `409 busy` (a publication in
  flight), `409 conflict` (the cabinet refused the publication), `409
  unpublished` (the room never published a copy the cabinet holds), `502` saved
  in the room but not published (retryable — the queue carries it on), `503`
  hub unavailable.
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
  `{fileId, bytes(base64), sha256, version, expectedVersion, idempotencyKey}`,
  where `expectedVersion` is the **etag** of the cabinet's copy (a string,
  INTEGRATION.md §5.2) and is absent on the first commit of a session
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
- An encrypted or signed PDF never starts a room: `PdfAdapter::open` refuses it
  with `Unsupported` before the first operation (a password would have to live on
  the server, and a full rewrite would invalidate the signature; RFC §12). The
  control plane sees the open failing, not a half-session.

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

### Save in a hosted session is the cabinet's commit, not a copy of the window (`f107f56c2`, 2026-11-12, local; not pushed)

The trap the previous record left open ("A closed socket no longer takes the
seat…", §4) is closed by `f107f56c2`.

**The defect.** Pressing Ctrl+S, the toolbar's Save or the menu's Save in a live
session ran `file.save`, which saved the bytes of this window's build through the
host page (`pdfcraft:save` **with bytes**). The cabinet stored them as an
ordinary new version behind the room: the room's marker kept naming the previous
copy, and the next commit — and the next ticket — were refused `409 the file
changed outside the session`. Two acceptance runs on the cabinet hit it (file 43
grew to 8504 B and then to 12 233 B, marker left behind) and the session could
only be reopened by reseeding the room. It is not a room defect: from the room's
side the file really did change outside it.

**The fix.** The room owns the stored document, so in a hosted session Save *is*
the room's commit:

- `PdfCraftApp::commit_instead_of_save` (`crates/ui-egui/src/collab.rs`) returns
  `true` and calls `collab_commit` when a session is live **and**
  `host_commit.is_some()`, i.e. only when a host page (the browser) owns the
  commit;
- `file.save` (`commands.rs`) takes that route first and returns without
  `save_active`; the pages' own Save action (`editing.rs`) does the same;
- desktop and single-user modes are untouched: there the session is a plain sync
  server with no host commit, so Save stays the local file. Both paths are
  covered — that is the pair of tests.

Tests, new in `crates/ui-egui/src/collab.rs`:
`save_in_a_hosted_session_asks_the_cabinet_to_commit` counts the host commit,
asserts no file appears at the `save_override` path and that the status line
reads "Commit requested from the cabinet"; on the old code it fails with "the
cabinet is asked to build the file" (0 calls, bytes written).
`save_without_a_hosted_session_writes_the_file` holds the ordinary save for a
session nobody commits to.

**Gates run for this commit** (as its own message records, on host `agents`):
`cargo fmt --check`; `clippy -D warnings` with the `collab` feature; wasm32
`pdfcraft-web` with and without the feature; `cargo test -p pdfcraft-ui-egui
--features collab` — lib 1035 + it 1898 with the pinned craft-i18n (`575cfdef`)
and craft-fonts (`b076417b`); without them the two documented environment tests
fail and are unchanged by this diff; plus the ocr, page_shadows, perf and play
suites.

**What is still owed on this head.** The full ten-step sweep
(`CI=1 cargo xtask ci`) had *not* been run on `f107f56c2` when the commit was
written: the last one was on `fee60000b` (10380 passed / 0 failed / 22 ignored,
§4 above), and two code commits above it — `24f519a91` (collab-hub) and
`f107f56c2` (ui-egui) — plus their three docs records postdate it. `24f519a91`
carries its own unit test
(`a_closed_connection_does_not_take_the_seat_of_the_same_actor`, collab-hub),
but neither commit's crate had been through the workspace sweep at the merged
head. The sweep for this head was run in this pass; it is recorded at the end of
this section.

### The cabinet's Save reaches the room, and the etag compares like with like (`fee60000b`, 2026-11-12, local; not pushed)

Two defects kept the cabinet's PdfCraft integration from finishing a save.

**The save never reached the room.** The page's "Commit to the cabinet" posts
`pdfcraft:commit`; the cabinet's D3 route refused it with `400`, because
PdfCraft's control plane is opened by the cabinet's own session ticket and only
a `session`-mode room had ever been asked to build the file. So the cabinet
never asked the room, never received `{version, etag}` and never posted
`pdfcraft:committed`; the version was only ever stored by the room's own idle
commit, and the app's status line stayed at "Commit requested from the cabinet".

`POST /v1/sessions/{fileId}/commit` (`apps/pdfcraft-collab-server/src/control.rs`):

- `Authorization: Bearer <session ticket>`, verified by the same `Verifier` as
  `hello`; the token's file must equal the path's (`403`), the role must be one
  that writes — `can_structural()`, owner or editor (`403` otherwise) — and a
  `fileId` that is not path-safe is `400`.
- It calls `WsServer::commit_now` (`crates/collab-ws/src/serve.rs`), the seam
  the control plane needed. Under the hub lock a clean room is answered from
  its marker (`CommitNow::Clean`, the same `Published` a publication answers,
  so a repeated Save creates no second version); otherwise
  `Hub::commit_if_dirty` writes the base under the lock, the cabinet call runs
  outside it (`Pending::publish`), `Hub::finish` records the outcome, a
  retryable failure is queued with the usual doubling backoff and the compaction
  the publication made due is queued too. `CommitNow` also carries `Missing`,
  `Busy` (a publication in flight), `Conflict`, `Retryable` and `Failed`; it
  never discards the room's work.
- Answers: `200 {fileId, version, cabinet:{version, etag}}` (GridCraft's
  shape); `401` no or bad token; `403` another file or a read-only role; `404`
  no room; `409 busy`; `409 conflict` when the cabinet refused the publication;
  `409 unpublished` when the room has never published a copy the cabinet holds
  (the cabinet must not be told a version that does not exist); `502` when the
  commit is saved in the room but not published (retryable — the queue carries
  it on); `503` hub unavailable; `500` internal.

**The ticket compared a digest with a version.** `POST /v1/sessions` compared
the tag the cabinet sent with the room's marker; the cabinet sent none, so the
room fell back to the SHA-256 of the bytes it had just received, while the
marker holds the cabinet's own tag (this cabinet's is the file's version tag).
From the first accepted commit on, every ticket compared a digest with a version
and the control plane answered `409` — reproduced on the deployed file 43,
where the session could only be reopened by moving the room's state directory
aside.

- The request now carries the cabinet's own `{etag, version}` explicitly
  (`version` is what the next publication sends as `expectedVersion`), and
  `rooms::Seed.cabinet_version` makes the room compare like with like.
- `CABINET` gains `sha256`, the digest of the published base
  (`crates/collab-pdf/src/store.rs`, `CabinetCopy.sha256`). A tag that moved is
  still a refusal, with one exception: when the bytes the cabinet names are the
  ones this room published — a retry, or a process that died between the
  cabinet's answer and the marker write — the room adopts the tag
  (`Room::adopt_cabinet`, reached through the one new `Room::adapter_mut`)
  instead of blocking the session. Foreign bytes stay `409`; a marker written
  before the field existed, and an operator's `resolve` (which sends no bytes),
  keep the old refusal.
- `apps/pdfcraft-collab-server/src/main.rs` seeds dev mode with
  `cabinet_version: None`; there the etag remains the digest of
  `--documents/<fileId>.pdf`.

Tests, new in `apps/pdfcraft-collab-server/src/control.rs`:
`a_commit_builds_the_file_and_names_both_versions`,
`a_commit_needs_a_token_for_this_file_and_a_writing_role`,
`a_refused_publication_is_a_409_and_a_repeat_is_still_a_409`,
`a_tag_that_moved_to_our_own_bytes_is_adopted_and_a_foreign_one_is_refused`,
`a_double_commit_survives_an_answer_that_never_reached_the_marker` — the route
with a session ticket, a clean repeat that publishes nothing, a viewer's and
another file's ticket refused, a refused publication, a marker written before
the digest, and the double commit whose answer was lost.

**Gate, on `fee60000b`, host `agents` (rustc 1.99.1, warm `target/`).**
`CI=1 cargo xtask ci` with `CRAFT_I18N_DIR` (craft-i18n at `575cfdef`) and
`CRAFT_FONTS_DIR` (`b076417b`) exits **0 — all ten steps**, no retries:
`fmt`; `clippy --workspace --all-targets -- -D warnings`;
`cli-no-default-features`; `test` — **10380 passed, 0 failed, 22 ignored**
(the 71 more than the 10309 at `0d3ea2708` are this line's and the earlier
undelivered ones'); `i18n-report`;
`layers` (52 crates, no violation); `wasm` (44 crates); `license-check` (258
repository assets, 7 bundled, 22 fetched); `deny` (advisories, bans, licences,
sources ok); `parity` (859 features). A previous hand-run of the same crates in
the commit's own message gave 39 daemon tests (5 new), 16 ws unit, 71
collab-pdf, 33 collab-server and 36 e2e; the sweep above supersedes it, and
nothing in it has run on GitHub — every workflow run on the fork is
`action_required`.

### Form-field carets in presence, addressed by name and offset (2026-11-12, local; not pushed)

The leftover `00c06ed5c` closed for text ranges was the field still being only an
object: the caret was the range's active end, never the caret inside a form field
being typed into (§5 p.4). A peer could see that its author was on the page and
had a selection, but not which field the typing went into or where the caret sat
inside it.

**Wire change.** `Selection::Text` gains one optional member,
`field: {name, offset}` (`crates/collab-proto/src/lib.rs`, `c935ebadb`):

- `name` is the field's fully qualified name — its `/T` chain joined by dots, the
  same stable address `Edit::SetFieldValue` and the app's field list use, and the
  field equivalent of an annotation's `/NM`. Not a widget object and not a page
  index: a turn, a move or an insert leaves it alone. Bounds:
  `MAX_FIELD_NAME_BYTES` = 256; a longer name is not sent (the box and caret
  point still are), and an empty one is refused `invalid` by
  `Selection::validate` before the frame leaves the socket.
- `offset` is the caret's position in characters into the draft the author has
  typed so far. The draft's text never travels; presence keeps it out of
  the frame, and a test asserts the encoded frame has no value.
- `rect` is the widget's box in user space and `caret` its point, as for any
  text selection, so a peer that does not know `field` still draws a box over the
  field and a caret where the typing is. Both extensions are additive: an old
  frame without `field` parses, and an old peer ignores the member
  (`the_plain_selection_form_still_parses`).
- The host page's report carries it as `ParticipantSelection::Text::field`
  (`FieldCaretRef`); `craft-web-host` still does not pass page/selection on (a
  foreign crate, §5 p.4), so this is app-side state like the rest.

**Client rules** (`crates/ui-egui/src/tools/forms_ui.rs`, `collab.rs`):

- while a text field (or an editable combo box) holds the keyboard, the editor
  records the caret's character offset and its user-space point from the same
  galley it draws (`Focus::caret`/`caret_at`, `record_caret`); the point goes
  through `PageXform::screen_to_view` and `PageInfo::view_to_user`, so a turned
  field and a rotated page are mapped back correctly;
- `local_selection` puts a field ahead of a comment or a range: that is what the
  person is doing. `field_selection` resolves the focused field's widget in the
  form the session holds, so the page index and box are current after any page
  operation, and refuses a widget whose page is gone or hidden;
- losing the keyboard (Enter, Tab, Escape, a click away, a commit, another
  document) clears both values and the next presence frame sends no field
  selection; a field the form no longer holds (deleted field, deleted page) is
  not sent at all. `overlay` clears the record at the top of each frame, so an
  editor that did not draw this frame (scrolled away, a dialog took the keys)
  cannot leave a stale trace;
- a `page.rotate` and a catch-up replay keep the field address and re-address the
  frame to the new `epoch`, the same `with_epoch` rule as a merge intent; an
  epoch change from a page operation does not need to drop it, because the
  address is the name, not the index;
- a **password** field shares no presence: its caret's offset would be the length
  of a secret. A name the frame cannot carry keeps the box and the caret point
  and drops only `field`; no field's text or value is included.

Tests:

| Crate | Test |
|---|---|
| `collab-proto` | `a_field_caret_carries_its_name_and_offset` (round-trip, no-text frame, empty and over-`MAX_FIELD_NAME_BYTES` names refused, a field without a caret point validates) |
| `collab-ws` e2e | `two_clients_share_a_selection_without_an_operation` (a field frame reaches two peers with its name and offset, box and point; the next frame with no selection takes it away; a late joiner sees it after its welcome; a viewer's presence is delivered while its operation is `forbidden`) |
| `ui-egui` unit | `a_field_caret_is_addressed_by_name_and_offset_and_goes_with_the_focus`, `a_field_caret_survives_a_turn_and_goes_with_the_field_or_page`, `a_password_field_shares_no_caret_and_a_long_name_keeps_the_box`, `the_host_report_carries_a_participants_selection` (extended with `field`) |
| `ui-egui` `it` (egui_kittest) | `forms::typing_in_a_field_records_the_caret_for_presence` — typing into the fixture's `name` field records the offset at the end of the draft and a point inside the widget's box in user space; Escape clears the focus |

Gates at `c935ebadb` (host `agents`, rustc 1.99.1, warm `target/`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy (proto, client, client-pdf, ws, pdf, UI with the feature) | `cargo clippy -p pdfcraft-collab-proto -p pdfcraft-collab-client -p pdfcraft-collab-client-pdf -p pdfcraft-collab-ws -p pdfcraft-collab-pdf -p pdfcraft-ui-egui --features pdfcraft-ui-egui/collab --all-targets -- -D warnings` | pass |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **36/36** |
| Selection tests in the UI | `cargo test -p pdfcraft-ui-egui --features pdfcraft-ui-egui/collab --lib collab::` | pass — **24** (1010 filtered out) |
| Forms in the real shell | `cargo test -p pdfcraft-ui-egui --features pdfcraft-ui-egui/collab --test it -- forms::` | pass — **74** |
| Tests, client / pdf / ws unit | `cargo test -p pdfcraft-collab-client -p pdfcraft-collab-pdf -p pdfcraft-collab-ws` | pass — client **14**, pdf **71**, ws unit **16**; the three doc-test targets run 0 |
| Web shell (wasm) | `cargo xtask wasm` | pass — wasm32: 44 crates ok |

What this does **not** cover: a peer does not resolve the field name against its
own copy, so when *another* client deletes the field (not the page) the box stays
on screen until the author's next frame or the 30 s TTL — a deleted page is
dropped at once by the epoch rule. A same-name twin is disambiguated by the field
object in the author's own list, but only the name travels (the box is what a
peer draws). The caret's geometry is where the author drew it; a peer cannot
recompute the offset's position in its own value, because the draft's text is
deliberately not on the wire. `craft-web-host` still does not forward
`selection`, so the cabinet's markers do not show it yet.

### Remote selections: per line, with the caret, re-addressed across a turn and a catch-up (2026-11-12, local; not pushed)

The brief took the leftover §6.6 named: "per-line range geometry, caret". The
feature is `5bad54c40` (docs `b6244142c`), an ancestor of HEAD: `Selection::Text`
carries `quads` (one rectangle per line piece, reading order, ≤48) and `caret`
(the active end, or the dragged corner of a column select), the canvas paints
each piece and the caret in the actor's colour, and a range addressed to another
epoch is not drawn. This pass does not rewrite it; it adds the acceptance
evidence and the two client rules the brief's scenarios need:

- **A turn and a catch-up keep the range.** `invalidate_content` clears the
  view's text selection on every content change, so a peer's `page.rotate` (or
  the snapshot tail replayed after a reconnect) erased the author's range before
  the new epoch ever reached a peer. `DocView::document_keeping_selection`
  (`crates/ui-egui/src/canvas.rs`) keeps the selection when its page still
  exists; the applied operations and the snapshot's tail replay use it when they
  leave page addresses alone (`keeps_page_addresses`: a merge operation, or
  `page.rotate`), and clear it for a structural page operation that can move or
  remove the page (`crates/ui-egui/src/collab.rs`). The next presence frame
  carries the same lines with the new `epoch` — the `with_epoch` rule for a merge
  intent, applied to presence. The text layer is re-extracted, so a frame or two
  after the turn may carry no selection; it comes back at the new epoch.
- **A dropped session draws no old room.** `Event::Disconnected` and
  `Event::Joined` clear `participants`, so a cursor or a selection from the
  socket that ended does not survive a reconnect; the new session re-learns who
  is there from presence. A peer that simply goes quiet is still the 30 s TTL
  (ADR-0006): the server forgets it at once on the socket close but has no
  `leave` message, so peers expire it; RFC §5 now says exactly that instead of
  claiming an immediate removal.

One line of the brief asked that a viewer not publish a selection. `TASK.md`
(§ roles: "`viewer` — только просмотр и presence"; § presence: "страница,
viewport, курсор, выделенный текст/аннотация") and RFC §5 say the opposite: a
selection is presence, never an operation. The e2e test therefore asserts a
viewer's selection is delivered while its operation is `forbidden`; refusing
viewer presence would take away what TASK.md grants it.

Tests:

| Crate | Test |
|---|---|
| `collab-ws` e2e | `two_clients_share_a_selection_without_an_operation` (a three-line range reaches two peers with three pieces and its caret; a cleared selection takes the caret away; a rotation moves the epoch to 1 and the same range re-addressed to it is delivered; a viewer's selection is presence while its operation is `forbidden`; a late joiner sees only the rotation) |
| `ui-egui` | `a_merge_or_a_turn_keeps_the_page_addresses`, `a_three_line_range_is_one_rect_per_line_and_its_caret_goes_with_it`, `a_rotated_page_re_addresses_the_range_to_the_new_epoch`, `a_range_on_a_deleted_page_is_neither_sent_nor_drawn`, `a_disconnect_takes_the_remote_selections_away` |

Gates at `00c06ed5c` (host `agents`, rustc 1.99.1, warm `target/`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy (collab crates, client-pdf, ws, and the UI with the feature) | `cargo clippy -p pdfcraft-collab-proto -p pdfcraft-collab-client -p pdfcraft-collab-client-pdf -p pdfcraft-collab-ws -p pdfcraft-collab-pdf -p pdfcraft-ui-egui --features pdfcraft-ui-egui/collab --all-targets -- -D warnings` | pass |
| E2E over a real socket | `cargo test -p pdfcraft-collab-ws --test e2e` | pass, **36/36** |
| Selection tests in the UI | `cargo test -p pdfcraft-ui-egui --features pdfcraft-ui-egui/collab --lib collab::` | pass — **21** (1010 filtered out) |
| Tests, client / pdf / ws unit | `cargo test -p pdfcraft-collab-client -p pdfcraft-collab-pdf -p pdfcraft-collab-ws` | pass — client **14**, pdf **71**, ws unit **16**; the three doc-test targets run 0 |
| Web shell (wasm) | `cargo xtask wasm` | pass — wasm32: 44 crates ok |

What this does **not** cover: a line's orientation *inside* a page — the wire
carries an axis-aligned rectangle per line piece, so text rotated within the
page (vertical text) is drawn as its box; the page's own `/Rotate` is carried
because the geometry is in user space and the receiver maps it back with the
page's rotation. The caret here is the range's active end; the caret inside a form
field is its own pass (2026-11-12, §4 above, `c935ebadb`). A
selection spanning several pages still names only the comments on the page in
view. The host page's `pdfcraft:presence` payload still cannot carry
`page`/`selection` until `craft-web-host` gains the two optional fields (a
foreign crate, ADR-0016).

### The web path in a browser: two iframes, one room (2026-11-12, local; not pushed)

§6.3 left one line open: "a browser run against the cabinet's page (two
iframes, `session` in, edits, `commit` out, `committed` back) — the code is
compile-checked and driver-tested, not exercised in a browser here". The
cabinet's acceptance run of 2026-11-11 did the chain against the real page
(§4, "The browser bridge in the cabinet: D4 closed"); this pass repeats it
locally, on the build host, from this branch's own bundle and a dev room, so it
can be repeated without the cabinet. No product code changed: this entry, the
§6.3 paragraph and one sentence in §1 are the whole pass.

**Environment.** The build host `agents` (20 cores, 125 GB RAM, Ubuntu 24.04,
rustc 1.99.1); trunk 0.22.1; the wasm bundle built from this branch's tree;
headless Chromium 141.0.7390.37 (the Playwright headless shell) with
`--use-angle=swiftshader --enable-unsafe-swiftshader`, WebGL 2 through
SwiftShader; Node 22 for the driver. The daemon is the branch's
`cargo build -p pdfcraft-collab-daemon` (binary `pdfcraft-collab-server`) in
dev mode:

```text
pdfcraft-collab-server --documents .../documents --data .../data \
  --jwt-secret-file .../jwt-secret --jwt-audience lzmkm \
  --listen 127.0.0.1:8791 --health 127.0.0.1:8792 \
  --origin http://127.0.0.1:9010 --app pdfcraft
```

One seed, `file-1.pdf`: two pages of plain text (Helvetica 18 pt, four lines
each), generated for the run and normalised with Ghostscript. Tickets: HS256
with a 32-byte development secret, `aud lzmkm`, `sub user-alice`/`user-bob`,
`doc file-1` **and** `fileId file-1`, `app pdfcraft`, `role owner`/`editor`,
`sid session-1`, two hours.

The page is one origin, `http://127.0.0.1:9010/`, serving the `dist/web` bundle
under `/app/` and a cabinet-like host page with two iframes,
`/app/index.html?host=parent&webgl&control&author=alice` and `...=bob`. The
host page answers `pdfcraft:ready` with `host`, `open` (the PDF's bytes as an
`ArrayBuffer`) and `session` (its own ticket and role), records
`session-state`, `presence` and `commit`, and answers `committed`. Input went
through the Chrome DevTools Protocol; since the app draws on one canvas, the
driver used the app's own `?control` channel for the pointer and keyboard
(`ui.click`, `ui.drag`, `ui.type`, `ui.key`) and for state (`ui.state`,
`ui.inspect`), and CDP for navigation, `Runtime.evaluate` and screenshots.

The bundle: `cd apps/pdfcraft-web && trunk build --release` produced
`dist/web` with `pdfcraft-web-fcbf58f0e625b9b3_bg.wasm` (48,221,940 bytes) and
`pdfcraft-web-ea55eafed102e968.js`; the page carries `collab` through
`index.html`'s `data-cargo-features="collab"`. Giving `--features collab` to
the trunk command line is not the way (§4, D4 item 1: trunk applies it to every
rust asset and the OCR worker has no such feature); §1's sentence that named
that command is corrected to `trunk build --release` by this pass.

**Result.**

| Step | What was observed |
|---|---|
| `pdfcraft:ready` | both iframes: `version:"1"`, `appVersion:"0.21.0"`, `file:false`; the host answered each with `host`, `open` and `session` |
| join | `Connected as user-alice · owner` and `Connected as user-bob · editor`; 21 `session-state`/`presence` frames in the window; each side's participant list showed the other (`user-bob:editor`, `user-alice:owner`) |
| owner's edit, page 1 | `comment.highlight`, then a drag across the text line — the room accepted `version 1`, `actor user-alice`, `class merge`, `kind primitive`, `annotation.add` of a `text_markup` `highlight` (two quads, `x 60…478`, `y 732…710`), style yellow, `base_version 0` |
| editor's edit, page 2 | `comment.note`, a click on page 2, the composer, the text "Browser run: editor sticky note on page two", Ctrl+Enter — `version 2`, `actor user-bob`, `annotation.add` of a `note` at `[183.6, 396]`, page index 1, contents in the operation; `base_version 1` |
| presence | the app's Collaboration window read `user-bob · editor · page 2` on the owner's side and `user-alice · owner · page 1` on the editor's; both statuses stayed `Connected as …` through the edits |
| the other side's annotation | the editor's Comments panel listed the owner's highlight (`user-alice`, `Highlight`, `Page 1`); the owner panned to page 2 and selected the editor's note — `ui.state` gave `selected_comment {page:2, index:1}` — and the screenshot shows both annotations in both panes and both panels |
| commit | the owner's "Commit to the cabinet": `pdfcraft:commit {sessionId:"session-1", bytes:null}` (no bytes: the cabinet assembles the file); the host answered `pdfcraft:committed {version:2, etag:"sha256:f5d3b1b1…"}`; the app's status became `Committed version 2` |
| rejects | none: two operations, both accepted (`/health` `ok:true`, the room's journal two records); the console had four warnings, all Chrome's "`integrity` is ignored for this preload" notice, and no error |

**What the run found (harness-side; no product change was made).**

1. **A text-markup gesture only selects text on the pane's current page.** With
   one pane showing page 2 as current, drags and double-clicks on page 1 at
   every offset between 2 % and 50 % of its rect started no selection and
   `edit.copy` stayed disabled, while the same gestures on page 2 selected at
   once; the run makes each edit on the page its pane has as current. A person
   scrolling to the page does this without noticing.
2. **With the left "All tools" panel open, Fit Width is a strip and the same
   gestures miss even on the current page.** There the page measured 100 × 129
   points and no line offset produced a selection; with the panel closed it was
   281 × 364 points with the Comments panel open on the right and 611 × 790
   with both side panels closed, and the first attempt at the PDF's second line
   produced the highlight. The run closes that panel in both panes first
   (`Close panel` in the accessibility tree).
3. **The web shell joins the document from the token's `fileId` claim, not
   `doc`.** `doc_key_from_token` (`crates/ui-egui/src/collab.rs`) reads
   `fileId` and falls back to the host's `sessionId`; a ticket carrying only
   `doc:"file-1"` therefore joined the room `session-1` and the server refused
   it ("the session token is for another file"). The cabinet's ticket carries
   both spellings (INTEGRATION.md §3.2), so the cabinet path is unaffected; a
   hand-made development ticket must carry `fileId`.
4. **The two iframes joined as different clients** (`client-<nanoseconds>`):
   neither replaced the other and both edit streams reached the room — the
   WordCraft `web-1` collision (§5.1 there) did not repeat.
5. **In host mode the commit is the cabinet's, not the room's.** The app posts
   `commit` with no bytes and waits for `committed`; nothing on the page can
   send the room's own `Commit` (that is the desktop path). The test page
   answered with the room's newest accepted version and the SHA-256 of the
   room's committed base, and it stored no file: `CURRENT` stayed at 0 and the
   journal at two records. The numbers in the row above are therefore what the
   page *reported*, not a stored cabinet version.

**What this does not establish.** The real cabinet's page, origin policy and
`wss://`; its own commit (fetch or assemble the file, store it, advance
`CURRENT`, send the real etag); two real browser profiles or machines; a person
at the controls. The cabinet's run of 2026-11-11 covers the first two on
production; this one covers repeatability on loopback.

**The harness.** Not in the repository — it lives on the build host under
`/home/markin/craft-runs/pdfcraft-web/`: `run.sh` (reset the room, start the
daemon and the origin, fresh browser profile, the run), `server.mjs` (static
bundle, tickets, `/room-state.json`), `cabinet.html` + `host.js` (the host
page), `run.mjs` + `cdp.mjs` (the CDP driver, which writes `out/results.json`
and the screenshots), `make-pdf.py` + `document.pdf`, `documents/file-1.pdf`.
`run.sh` ends with a nonzero exit on any failed step. It is described here so it
can be rebuilt, or moved into a test, when the repository wants a browser gate;
nothing was committed from it.

### The cabinet's `expectedVersion` is its etag, not a number (`0d3ea2708`)

The brief: the working tree carried a contract change from a parallel
verification pass — the room published `expectedVersion` as a **string etag** of
the cabinet's copy (INTEGRATION.md §5.2, the body DeckCraft's room sends) instead
of a number, with nothing at all on the first commit — plus the three tests that
compare it. Bring it to a green gate, commit on top of `28fd4e1b3` (PR #6622's
head) and push. The gate is green and the commit is `0d3ea2708`; **the push was
not the agent's**: this agent's permission denies `git push*` (the same guard as
every pass before), so the owner or `pr-agent` pushed one branch (`git push fork
feature/realtime-collaboration`) at 19:14Z, and the fork tip, PR #6622's
`headRefOid` and this clone's HEAD were `6ac4e1130`; a later push by the owner
or the PR-sweep pass at 21:10Z made the same three read `eb60bc5ff`, which is
the delivered head (header, "Delivery") — the code and the gate are unchanged
by either push, since every commit above `0d3ea2708` is docs-only. What the
21:10Z push delivered is that record of `6ac4e1130` — `eb60bc5ff` itself; the
commit this pass adds is another docs commit on top, local until the owner or
`pr-agent` pushes the branch again. An
intermediate reading at 18:20Z still saw
`28fd4e1b3`, which is why an earlier version of this paragraph said the push had
not happened; that reading is superseded.

- **The defect.** The room sent `expectedVersion` as a JSON number and `0` for
  the first commit. The cabinet's commit contract names the etag of the copy it
  holds, a string, and omits it for the first commit: a number is refused by the
  cabinet's decoder (`400`), and a `0` would read as an expectation of a copy
  nobody has. `Publisher::publish` now takes `expected: Option<&str>` and builds
  the body as a map, inserting `expectedVersion` only when the etag is present
  and non-empty; `CabinetAdapter::expected_etag` reads `CABINET.etag` only when
  the marker also has `expected` (a copy the cabinet named by an accepted commit
  or a resolve — a room seeded from bytes has the seed's own etag, which the
  cabinet never issued). A `2xx` body still advances the marker: its `etag` is
  the next `expectedVersion`, its `version` the cabinet's numbering; a body
  without the object falls back to `sha256:<digest>` of the published bytes and
  to one more than a numeric etag (this cabinet's etag is its version tag), else
  to the room's version.
- **The tests follow the contract.** The fake cabinets in
  `tests/conflict_resolve.rs`, `tests/publication_retry.rs` and
  `tests/restore_publication.rs` compare opaque etags (`sha256:<version:064x>`
  in two of them; the dev-mode one also accepts the source file's etag, which is
  the copy a `--documents` room keeps), the first delivery of a session asserts
  that `expectedVersion` is **absent**, and the resolve test asserts that the
  etag the operator named is what the next publications carry.
- **A restored publication is the session's first commit.** A room killed
  between the base write and the POST has no etag the cabinet ever named, so it
  cannot name one: the restored POST is the first commit, the cabinet stores it
  as its next version and the room adopts the etag it answers. The old test
  expected a `409` on that POST (`expectedVersion: 0` against a cabinet at 5) and
  then a resolve; with no expectation that refusal is unreachable, so the test
  became `a_restored_publication_is_the_first_commit_and_the_marker_advances`
  (cabinet at 5 → stores 6, `publications_restored_total 1`, no conflict) and the
  `wait_for_health`/`resolve` helpers it used were dropped — clippy's
  `dead_code` at `-D warnings` refused them (EXIT 101 before the removal).
- **Two defects of the working-tree change fixed here.** `check_mirror`'s doc
  comment had ended up above `expected_etag` (which gained its own), so
  `check_mirror` was undocumented; and the replaced test left the two helpers
  without a caller. Both are in `0d3ea2708`; no behaviour changed beyond the
  contract itself. `cargo fmt --all -- --check` and `cargo clippy -p
  pdfcraft-collab-daemon --all-targets -- -D warnings` exit 0 on the final tree
  (the clippy warning about `egui_*` patches and `hayro-interpret`'s unused
  `Deref` is the host's dependency tree, not this crate).
- **The crate's tests** on the host: `cargo test -p pdfcraft-collab-daemon`
  — 34 unit, `conflict_resolve` 1, `crash_recovery` 1, `publication_retry` 1,
  `restore_publication` 2, **0 failed, 0 ignored, EXIT 0**.
- **The whole sweep** (`CRAFT_I18N_DIR=/home/markin/build/i18n-575`
  (`575cfdef`, the `craft-i18n.rev` pin), `CRAFT_FONTS_DIR=/home/markin/craft-fonts`
  (`b076417b`), `CARGO_BUILD_JOBS=12`, rustc 1.99.1): `CI=1 cargo xtask ci` →
  **EXIT 0, `CI: all 10 steps passed`** — 157 targets as the log lists them (111
  `Running` + 46 `Doc-tests`), **10309 passed, 0 failed, 22 ignored**; `fmt`,
  `clippy`, `cli-no-default-features`, `i18n-report`, `layers`, `wasm`,
  `license-check`, `deny` and `parity` pass. The totals are 84 above the
  `69b2a6dad` sweep's 10225 — the commits merged since (`46d2e203a`'s v0.21.0 and
  the three on top) brought their own tests.
- **Docs say the same as the wire.** RFC §8 and `server.md` §3 now describe
  `expectedVersion` as the etag string and the absent first commit, and the
  `CABINET` marker's `expected` as the cabinet's numbering (not the next
  `expectedVersion`, which is the etag). `pdfcraft/INTEGRATION.md`'s callback
  line says so too. Decision 14 in §2 carries the new wording.
- **What this does not change.** The resolve endpoint's own body
  (`{"expectedVersion": N, "etag": "sha256:…"}`) still names the cabinet's
  **version** and its etag: it is the operator saying which copy the cabinet
  holds, and the room turns that into the string the next publication sends.
  The dev-mode mirror path keeps the source file's etag as the marker (decision
  16), so a `--documents` room's publications name the source file's hash; the
  dev-mode fixtures accept it for that reason.


### The two red steps of `95e2c9c97` fixed in `69b2a6dad`, and the sweep on it (2026-11-11, local; not pushed)

The brief asked for exactly this: remove the blank line after `use crate::{` at
`crates/engine/src/session.rs` and the unused `BookmarkRow` in the same import,
run `CI=1 cargo xtask ci` on the `agents` host until it exits 0, commit on top of
`f7ff56373`, update this file, and push nothing. Done in that order.

- **The defect.** Both were in the hand-written import block of the sixth merge
  (`95e2c9c97`, §4 above): a blank line at the head of the list, which stopped
  `cargo fmt --all -- --check`, and `BookmarkRow`, which main's tree no longer
  uses natively, which stopped `cargo clippy --workspace --all-targets -- -D
  warnings` with `unused import: BookmarkRow --> crates/engine/src/session.rs:14:19`.
  `BookmarkRow` occurs exactly once in the file — that import — so the name was
  dead there and nowhere else; `crates/engine/src/bookmarks.rs` and
  `crates/engine/src/lib.rs` keep the type and are untouched.
- **The fix, `69b2a6dad`** (`fix(engine): repair the session.rs import block the
  3a117c3c0 merge left`), one file, +6/−7: the blank line goes and `BookmarkRow`
  leaves the list. Removing the name shortens the first line, so rustfmt wants
  the list rewrapped; the resolved list is rustfmt's own output, taken from
  `cargo fmt` on the host rather than written by hand, and `cargo fmt --all --
  --check` then exits 0. No other line, file or behaviour changes — the commit's
  diff is that hunk alone.
- **The sweep on `69b2a6dad`,** first run, no retries, on host `agents` (rustc
  1.99.1 `58cf8f9c3`, `CARGO_BUILD_JOBS=12`,
  `CRAFT_I18N_DIR=/home/markin/build/i18n-575` — craft-i18n `575cfdef`, the
  revision `craft-i18n.rev` pins — and
  `CRAFT_FONTS_DIR=/home/markin/craft-fonts` at `b076417b`, both verified on the
  host before the run): the host printed `head=69b2a6dad tree=0` before it, and
  **`CI_EXIT=0`, `CI: all 10 steps passed`**.

  | Step | Result |
  |---|---|
  | `fmt` | passed — `cargo fmt --all -- --check` |
  | `clippy` | passed — `cargo clippy --workspace --all-targets -- -D warnings` |
  | `cli-no-default-features` | passed — `cargo check -p pdfcraft-cli --no-default-features` |
  | `test` | passed — **10225 passed, 0 failed, 22 ignored** |
  | `i18n-report` | passed — 5071 English strings, 0 catalog rows skipped |
  | `layers` | passed — 52 crates, no layering violation |
  | `wasm` | passed — wasm32, 44 crates ok |
  | `license-check` | passed — `assets: ok (258 repository assets, 7 bundled by dependencies, 22 fetched at build time)` |
  | `deny` | passed — `advisories ok, bans ok, licenses ok, sources ok` |
  | `parity` | passed — 858 features (835 counted, 23 na), 56.8% shipped / 64.1% weighted; P0 90.5% shipped / 94.6% weighted |

  The totals are the same as the eight-steps-one-by-one measurement on
  `95e2c9c97` (10225 / 0 / 22), as they must be: the fix touches an import, not
  a test. The target count is the log's own: 109 `Running` blocks and 46
  `Doc-tests` blocks, 155 together — the earlier record's "156 targets" was its
  own split of the same suite (108 + 48) and is not contradicted here, only
  counted differently. `cargo-nextest` is still not installed on the host, so
  `test` is `cargo test`. `cargo metadata --locked` exits 0 and the host's tree
  was clean.
- **Nothing was pushed.** The fork and PR #6622 stay at `95e2c9c97` (§ header,
  "Delivery"); `git ls-remote fork` was checked after the commit and still
  returns `95e2c9c97`. The push of `69b2a6dad` and the docs commits on top of it
  is the owner's step.
- **What this does not change.** The red result recorded in the section below
  for `95e2c9c97` stands as the measurement of *that* head; the two failing
  steps there are the same two this commit fixes. The eight other steps were not
  re-run on `95e2c9c97` here.

### The merges of `origin/main` `4efcfa6ea` and `3a117c3c0`, and the sweeps on `b7545a7a9` and `95e2c9c97` (2026-11-11, local; the second head was pushed by a parallel session)

The brief asked for `CI=1 cargo xtask ci` on the local head `b7545a7a9` — the
merge of `origin/main` `4efcfa6ea` — with the evidence in this file, in
`pdfcraft/PR-DRAFT.md` and in the PR body draft, no code change and no push. The
sweep ran green. While it ran, the head moved: the parallel PR-sweep session
merged the next main tip (`3a117c3c0`) into the branch as `95e2c9c97` and
pushed it. Both heads were measured, and both results are below.

- **The fifth merge, `b7545a7a9`** (2026-11-11 09:16Z). A true merge (`--no-ff`,
  no rebase, no force) of `4efcfa6ea` — 13 commits above the last merge base
  `d2916decb` — with three conflicted files, all resolved by keeping both
  sides: `crates/engine/src/edit.rs` (the `use crate::{…}` list: main dropped
  `open_source`, whose callers moved to `edit/run.rs`, and added
  `Edit::SetFieldImageTarget`), `crates/engine/src/history.rs` (main's
  `ResetForm` and `SetFieldImageTarget` arms beside the branch's
  `SetPageLinks | SetPagesLinks` one, in `uses_scripts` and in `scope_of`) and
  `crates/engine/src/tests.rs` (both sides append tests; both kept, no
  duplicate names). Against its first parent: **120 files, +10863/−1139**.
  `Cargo.lock` moved 5 lines (main's `craft-print` pin `60ede999…` and the two
  `winprint` revs). After it: **122 commits** not in `origin/main`, 0 behind.
- **The sweep on `b7545a7a9`,** first run, no retries, on host `agents` (rustc
  1.99.1 `58cf8f9c3`, `CARGO_BUILD_JOBS=12`, `CRAFT_I18N_DIR=/home/markin/build/i18n-575`
  — craft-i18n `575cfdef`, the revision `craft-i18n.rev` pins — and
  `CRAFT_FONTS_DIR=/home/markin/craft-fonts` at `b076417b`, the ref `ci.yml`
  passes): **EXIT=0, `CI: all 10 steps passed`**. The host printed
  `head=b7545a7a9` before the run, and the totals below are that head's, not the
  sixth head's (10202 against its 10225), so the test binaries were built from
  `b7545a7a9` — the parallel session's merge and push came later in the same
  window.

  | Step | Result |
  |---|---|
  | `fmt` | passed — `cargo fmt --all -- --check` |
  | `clippy` | passed — `cargo clippy --workspace --all-targets -- -D warnings` |
  | `cli-no-default-features` | passed — `cargo check -p pdfcraft-cli --no-default-features` |
  | `test` | passed — 156 targets, **10202 passed, 0 failed, 18 ignored** |
  | `i18n-report` | passed — 5071 English strings, 0 catalog rows skipped |
  | `layers` | passed — 52 crates, no layering violation |
  | `wasm` | passed — wasm32, 44 crates ok |
  | `license-check` | passed — `assets: ok (258 repository assets, 7 bundled by dependencies, 22 fetched at build time)` |
  | `deny` | passed — `advisories ok, bans ok, licenses ok, sources ok` |
  | `parity` | passed — 858 features (835 counted, 23 na), 56.8% shipped / 64.1% weighted; P0 90.5% shipped / 94.6% weighted |

  `cargo-nextest` is still not installed on the host, so `test` is `cargo test`,
  as in earlier passes. The tests are 175 above `b21b57b55`'s 10027 — main's
  thirteen commits brought their own (js, forms, print, crypt, prc, cos,
  render, automation) — and the suite is unchanged in count from the previous
  sweep on top of that.
- **The head moved while the sweep ran.** At 10:26:52Z the parallel PR-sweep
  session committed `95e2c9c97` (a merge of `3a117c3c0`, "M10: open Word, Excel
  and PowerPoint files through LibreOffice"), and pushed it at 10:28:56Z. At
  10:33Z, before that push was visible to this pass, PR #6622 still read
  `headRefOid` `7f25d493b`, `mergeable: CONFLICTING`, `mergeStateStatus: DIRTY`,
  `updatedAt` 07:26:52Z, one check (CodeRabbit, `skipped`) — the state the brief
  named. After the push the same query reads `headRefOid` `95e2c9c97`,
  `MERGEABLE` / `UNSTABLE`, `baseRefOid` `3a117c3c0`, `updatedAt` 10:28:56Z;
  `git ls-remote fork` returns the same hash. The sixth merge is **30 files,
  +1297/−126** against its first parent, with two conflicted files:
  `Cargo.lock`, re-resolved by `cargo metadata` (5 lines against the fifth head
  — `rustix` 1.1.6 and the `windows-sys` split the new dependency brought), and
  `crates/engine/src/session.rs`.
- **The delivered head fails `fmt` and `clippy`, both from that `session.rs`
  resolution.** `CI=1 cargo xtask ci` on `95e2c9c97` (same host and environment,
  the head synced by the same command) exits 1 at the first step:

  ```
  Diff in …/crates/engine/src/session.rs:10:
   use crate::history::{Keys, MAX_UNDO, Scope, keys_after, scope_of, uses_scripts};
   use crate::{
  -
       BookmarkPage, BookmarkRow, CombineSource, …
  CI: 0 passed, FAILED at `fmt`: cargo fmt --all -- --check: exited with exit status: 1
  ```

  and `cargo clippy --workspace --all-targets -- -D warnings` stops on

  ```
  error: unused import: `BookmarkRow`
    --> crates/engine/src/session.rs:14:19
  ```

  `BookmarkRow` was used natively at `b7545a7a9` (the workspace clippy was green
  there); main's tree no longer uses it after `3a117c3c0`, so the import the
  resolution kept is dead. Neither defect is main's or the branch's — both are
  in the hand-written import block of the sixth merge. The fix is two lines in
  that block (drop the blank line and the unused name); this pass did not make
  it, because the brief says the code stays as it is.
- **The other eight steps pass on `95e2c9c97`,** run one by one on the same tree
  and the same environment (the `fmt` step has no selection argument in
  `xtask ci`, so the remaining steps were invoked directly):

  | Step | Result |
  |---|---|
  | `fmt` | **FAILED** — the blank line above, `session.rs:10` |
  | `clippy` | **FAILED** — `unused import: BookmarkRow`, `session.rs:14:19` |
  | `cli-no-default-features` | passed |
  | `test` | passed — 156 targets, **10225 passed, 0 failed, 22 ignored** |
  | `i18n-report` | passed — 5071 English strings, 0 catalog rows skipped |
  | `layers` | passed — 52 crates, no layering violation |
  | `wasm` | passed — wasm32, 44 crates ok |
  | `license-check` | passed — 258 repository assets, 7 bundled, 22 fetched |
  | `deny` | passed — advisories, bans, licenses and sources ok |
  | `parity` | passed — 858 features (835 counted, 23 na), 56.8% / 64.1%; P0 90.5% / 94.6% |

  The 23 tests over the fifth head, and its 4 extra ignored ones, are
  `3a117c3c0`'s LibreOffice work; the `doc` run is inside the 156 targets, as in
  the `xtask` split (108 lib/bins/tests + 48 doc). `cargo metadata --locked`
  exits 0 on this head, and the host's tree was clean.

### (Previous pass, superseded above.) The merge of `origin/main` `d2916decb` and the sweep on the head PR #6622 carried (2026-11-11, local; head published)

The brief asked to re-measure the gates on `84da974d9`, put the facts into this
header, into `pdfcraft/PR-DRAFT.md` and into the PR body, and leave the code
alone. Nothing was pushed in this pass: `84da974d9` is already the PR's head,
and the records written now are the owner's to publish.

- **What `main` did in those three commits.** `9345b5caa` (#6933) and
  `25f3f5429` (#6934) both remove the second `decoded_strict_within` that
  `ad3e6d4b8` (#6046) had introduced — the duplicate the previous merge had to
  resolve by hand — and `d2916decb` (#6932) rewrites `packaging/freebsd/package.sh`
  to `[[`. So `main`'s tree compiles by itself again, and nothing in the merge
  has to repair it.
- **The merge, `84da974d9`.** A true merge (`--no-ff`, no rebase, no force):
  two files against its first parent, +5/−4, `packaging/freebsd/package.sh`
  (main's) and `crates/cos/src/object.rs`. The one conflict is in the latter,
  because the branch's resolution and main's fix differ in shape — the branch's
  `decoded_strict_within` delegates to `decoded_strict_within_counted` and its
  doc comment says so, while main's keeps the `decode` closure. The merge keeps
  the branch's delegation: one decode path, the counted one, and no line of
  main's behaviour is lost. `git diff origin/main HEAD -- crates/cos/src/object.rs`
  is 4 lines (a doc comment and the body) — the branch's only difference from
  `main` outside the feature. It is no longer a fix to main's tree; taking
  main's text verbatim would remove it, if a reviewer prefers the smaller diff.
  After the merge: **119 ahead / 0 behind** `d2916decb` (120 after this record).
  `Cargo.lock` did not move and `cargo metadata --locked` exits 0.
- **The sweep, `CI=1 cargo xtask ci`,** on host `agents` (rustc 1.99.1
  `58cf8f9c3`, `CARGO_BUILD_JOBS=12`, `CRAFT_I18N_DIR=/home/markin/build/i18n-575`
  — craft-i18n `575cfdef`, the revision `craft-i18n.rev` pins — and
  `CRAFT_FONTS_DIR=/home/markin/craft-fonts` at `b076417b`, the ref `ci.yml`
  passes), on `84da974d9`: **EXIT=0, `CI: all 10 steps passed`**, first run, no
  retries. `test`: **156 targets, 10027 passed, 0 failed, 18 ignored**. The
  totals are identical to the `b21b57b55` sweep, which is what main's three
  commits predict: they delete code and edit a script, and add no test.
- **Per step:**

  | Step | Result |
  |---|---|
  | `fmt` | passed — `cargo fmt --all -- --check` |
  | `clippy` | passed — `cargo clippy --workspace --all-targets -- -D warnings` |
  | `cli-no-default-features` | passed — `cargo check -p pdfcraft-cli --no-default-features` |
  | `test` | passed — 156 targets, 10027 passed, 0 failed, 18 ignored |
  | `i18n-report` | passed — 5069 English strings, 0 catalog rows skipped |
  | `layers` | passed — 52 crates, no layering violation |
  | `wasm` | passed — wasm32, 44 crates ok |
  | `license-check` | passed — `assets: ok (258 repository assets, 7 bundled by dependencies, 22 fetched at build time)` |
  | `deny` | passed — `advisories ok, bans ok, licenses ok, sources ok` |
  | `parity` | passed — 858 features (835 counted, 23 na), 56.8% shipped / 64.1% weighted; P0 90.5% shipped / 94.6% weighted |

  `cargo-nextest` is still not installed on the host, so `test` is `cargo test`,
  as in earlier passes.
- **The three fast steps were incremental, and one was proved from scratch.**
  `fmt`, `clippy` and `cli-no-default-features` came back from warm fingerprints
  (the host's `target/` had already built this content), so the log shows no
  compile line for them. To make the merged `crates/cos/src/object.rs` prove
  itself rather than trust the cache, the file was touched on the host and
  `cargo clippy -p pdfcraft-cos --all-targets -- -D warnings` re-ran from
  scratch: 8.47 s, exit 0, and `git status --porcelain` stayed empty (the touch
  changes no content).
- **What this does not cover.** As in the earlier sweeps: the host has no
  `qpdf`/`poppler`, so oracle-backed tests skip there exactly as they do without
  those tools; Windows-only checks stay CI's; the web bridge is compiled for
  wasm32 but never run in a browser. And no GitHub workflow ran on
  `84da974d9` — the only check on the PR is CodeRabbit `skipping` — so
  `MERGEABLE`/`UNSTABLE` is GitHub's calculation of the merge, not a gate run.
- **Delivery, as the API reports it.** `headRefOid` `84da974d9` = the fork's
  branch tip (`git ls-remote fork`); it is the parent of the docs commit that
  carries this record, so the local branch is that one docs commit above the PR
  head. `mergeable` `MERGEABLE`, `mergeStateStatus` `UNSTABLE`; base `main`; not
  a draft; `reviewDecision` empty; `commits` 100 because GitHub pages the list,
  against 119 in `git rev-list --count origin/main..HEAD` at that head.

### (Previous pass, superseded above.) The merge of `origin/main` `ad3e6d4b8`, and the delivery facts

The brief asked to merge the remote `main` tip into the branch, run the gate on
the new head and bring the header to the facts of the delivery. Two of those
three turned out to need evidence first: the tip itself is broken, and the PR's
`CONFLICTING` state is a conflict with that tip, not with the PR's base.

- **The tip did not compile.** `git fetch origin main` moved `origin/main` from
  `85987baef` (what this clone had) to `ad3e6d4b8`, 6 commits further, and 21
  above the branch's last merge base `92b965dba`. `ad3e6d4b8` ("fix(optimize):
  bound each unused-resource decode by the remaining shared budget (#6046)")
  adds `Stream::decoded_strict_within` to `crates/cos/src/object.rs` beside the
  one `03366c3c7` had already added (that commit split `decoded_strict` and put
  the `decode`/`decode_chain` pair in place). The two definitions have the same
  signature, so the tree cannot compile: `error[E0592]: duplicate definitions
  with name 'decoded_strict_within'`, at `object.rs:458` beside `object.rs:451`.
  This is main's own breakage, not the merge's: `85987baef` and `92b965dba` each
  contain **one** definition, `ad3e6d4b8` contains two; main's own push run
  `42918717889` on that commit finished `failure` in `gate / gate (ubuntu-24.04)`
  and `gate / gate (windows-latest)`; the twenty commits below it are fine.
- **The merge, `b21b57b55`.** A true merge (`--no-ff`, no rebase, no force),
  75 files, +3670/−192. One textual conflict, `crates/engine/src/tests.rs`
  (both sides append tests at the end) — the branch's
  `page_and_comment_references_survive_comment_edits` and
  `remote_edits_drop_the_local_snapshot_history` and main's
  `javascript_non_rgb_field_colours_convert_and_keep_the_scripts_other_changes`;
  **both kept**, in that order. The duplicate above arrived as a
  non-conflicting hunk (git had nothing to compare — each side added its own
  copy), so it was resolved in the merge as well: the **older** definition (the
  `decode`-closure one from `03366c3c7`) is dropped and the **newer** counted
  one is kept, because the commit's doc comment, its message and its new test
  (`strict_decoding_is_bounded_by_the_callers_remaining_allowance`) all describe
  the remaining-budget semantics. The utility of the merge commit for upstream is
  exactly this one hunk; it is to be dropped when main fixes its own duplicate.
  `Cargo.lock` auto-merged, `cargo metadata --locked` exits 0, and after the
  merge the branch is **117 ahead / 0 behind** `ad3e6d4b8` (118 after this
  record). `cargo check --workspace --all-targets -j 12` on the host exits 0
  before the sweep.
- **The gate, `CI=1 cargo xtask ci`,** on host `agents` (rustc 1.99.1,
  `CARGO_BUILD_JOBS=12`, `CRAFT_I18N_DIR=/home/markin/build/i18n-575` —
  craft-i18n `575cfdef`, the revision `craft-i18n.rev` pins — and
  `CRAFT_FONTS_DIR=/home/markin/craft-fonts` at `b076417b`, the ref `ci.yml`
  passes), on `b21b57b55`: **EXIT=0, `CI: all 10 steps passed`**, first run, no
  retries.
- **Per step:**

  | Step | Result |
  |---|---|
  | `fmt` | passed — `cargo fmt --all -- --check` |
  | `clippy` | passed — `cargo clippy --workspace --all-targets -- -D warnings` |
  | `cli-no-default-features` | passed |
  | `test` | passed — **156 targets, 10027 passed, 0 failed, 18 ignored** (up from 9986/156 at `afd28a2f9`, the difference is main's new tests plus the branch's two kept ones) |
  | `i18n-report` | passed — 5069 English strings, 0 catalog rows skipped |
  | `layers` | passed — 52 crates, no layering violation |
  | `wasm` | passed — wasm32, 44 crates ok |
  | `license-check` | passed — `assets: ok (258 repository assets, 7 bundled by dependencies, 22 fetched at build time)` |
  | `deny` | passed — `advisories ok, bans ok, licenses ok, sources ok` |
  | `parity` | passed — 858 features (835 counted, 23 na), 56.8% shipped / 64.1% weighted; P0 90.5% shipped / 94.6% weighted |

  `cargo-nextest` is not installed on the host, so the `test` step used
  `cargo test`, as in earlier passes.
- **What this does not cover.** The host has no `qpdf`/`poppler`, so
  oracle-backed tests skip there exactly as they do without those tools;
  Windows-only checks stay CI's; the sweep ran on `b21b57b55`, not on this
  STATUS commit above it.
- **The PR's state, from the API.** `headRefOid` = `80e99a2ed`, the fork's
  branch tip (`git ls-remote fork` agrees), `updatedAt` 2026-11-10 21:43:17Z —
  the push that the earlier header said had not happened did happen.
  `mergeable: false`, `mergeable_state: "dirty"`, `base.sha` `92b965dba` (read
  before this pass's body rewrite; the head did not move); the
  checks on the head are five workflows at `action_required` (CI, Nix,
  SonarCloud, FreeBSD, Windows ARM64) and CodeRabbit `skipped`, so no gate has
  run there. The dirty state is accounted for by `ad3e6d4b8` and the 20 commits
  under it — the merge above removes it locally; GitHub will re-evaluate the
  mergeability after the next push.
- **The PR body.** It described base `b6cbf9bb3`, a head two merges old and a
  test list that no longer matched the tree. It was rewritten this pass from
  `pdfcraft/PR-DRAFT.md`, which now records this head's gates (the numbers in
  the table above) and what was not run, and it keeps to house rule 15 (no
  agent attribution) and to `PR-CHECKLIST.md`'s "what was tested".
- **Still outside the repository.** The push of `b21b57b55` and the maintainer's
  approval of the five runs on the new head. This role denies `git push*`.

### The full CI sweep — merge of `origin/main`, both `test` targets closed (2026-11-10, local; not pushed)

The pass before this one left `cargo xtask ci` red at `test` on two targets, both
`origin/main` tests (the previous subsection records the reading). This pass
checked them against a clean `origin/main` first, merged the remote, fixed the
one real cause and ran the whole sweep.

- **Clean `origin/main` (`debb826f2`) on its own tree.** A `git worktree` in
  `/tmp`, synced to host `agents` as its own directory, built and ran the two
  tests: `pdfcraft-automation --test it
  local_gotor_link_open_applies_destinations_and_honors_root` **passes**;
  `pdfcraft-ui-egui --test it
  comments::the_selection_bar_stays_inside_the_pages_area` **passes** only with
  `CRAFT_I18N_DIR` set to the craft-i18n checkout that `craft-i18n.rev` pins
  (`575cfdef…`, worktree of `~/craft-i18n`) and `CRAFT_FONTS_DIR` set to the
  craft-fonts ref `ci.yml` passes (`b076417b…`). Without the catalogs it panics
  at `set_option("language", "de")` — `language must be one of auto, en` — and
  not at the area-rect expect; the earlier record that called it "no area rect"
  was a mis-reading of that panic.
- **The automation failure was this branch's, not main's.** `2b61ff80f` parses a
  remote go-to into `LinkAction::Remote` (GoToR and named-destination links are
  editable in a room), so `link_list` reported `remote_file`/`remote_page` and
  left `other` empty; the upstream test reads `other` as the action's own `/S`
  name. `c1171d941` reports `GoToR` there beside the remote fields; the upstream
  assert is unchanged. The same test passes on clean `origin/main` and on this
  tree.
- **The merge** (`c15cc0f57`) is described in the header; its one conflict was
  resolved by keeping both sides' appended tests in `crates/annot/src/tests.rs`.
- **The sweep.** `CI=1 cargo xtask ci` on host `agents` (rustc 1.99.1,
  `CARGO_BUILD_JOBS=12`, `CRAFT_I18N_DIR` a craft-i18n worktree at `575cfdef`
  and `CRAFT_FONTS_DIR` a craft-fonts checkout at `b076417b`, the revisions
  `craft-i18n.rev` and `ci.yml` pin): **EXIT=0**, `CI: all 10 steps passed` — fmt,
  clippy, cli-no-default-features, test, i18n-report, layers, wasm,
  license-check, deny, parity. The `test` step: **156 targets, 9930 passed, 0
  failed, 18 ignored**. `layers`: 52 crates, no violation; `wasm32`: 44 crates
  ok; `assets`: 258 repository assets, 7 bundled by dependencies, 22 fetched at
  build time; `parity`: 858 features (835 counted, 23 na); `deny`: advisories,
  bans, licences and sources all ok.
- **What this does not cover.** The host has no `qpdf`/`poppler`, and CI's
  `PDFCRAFT_ORACLES_REQUIRED=1` is not set here, so oracle-backed tests skip the
  way they do without those tools; Windows-only checks stay CI's. The sweep ran
  on the merge plus `c1171d941`, not on the STATUS commit itself.

### The merge of `origin/main` `92b965dba` (the brief's `8559306f7`, moved on) and the gate run at the merged head (2026-11-10, local; not pushed)

The brief asked to merge `origin/main` `8559306f7` (eight printing/forms
commits) into `4f3e21245` — the head of PR #6622 — keeping both sides of the
expected `crates/annot/src/tests.rs` conflict, run `cargo xtask ci` on host
`agents` and push the fork. The upstream tip had moved to `92b965dba` by the
time of the fetch, so the merge takes the tip; `8559306f7` is its ancestor 22
commits down, and its eight commits are part of it.

- **The merge, `afd28a2f9`.** A true merge (`--no-ff`, no rebase, no force) with
  **no conflicted file**: 40 files, +3831/−224. `crates/annot/src/tests.rs`
  did not conflict because main never changed it after `debb826f2` (the blob is
  still `ce5a8c1` at both `8559306f7` and `92b965dba`), so the branch's version,
  which already carries both sides from the `c15cc0f57` resolution, merged as it
  was. `Cargo.lock` auto-merged (main's print/spool dependencies) and
  `cargo metadata --locked` exits 0. After the merge: **115 ahead / 0 behind**
  `92b965dba`, and 116 after this record.
- **The gate needs the i18n and font checkouts.** The first run without them
  (`CI=1 cargo xtask ci`) went **EXIT=1** at `test` on
  `comments::the_selection_bar_stays_inside_the_pages_area`, panicking at
  `crates/ui-egui/tests/it/comments.rs:4374` with
  `language must be one of auto, en` — the documented missing-catalog cause, not
  a merge regression (no `crates/annot` or collab test failed). The second run
  with `CRAFT_I18N_DIR=/home/markin/build/i18n-575` (craft-i18n `575cfdef`, the
  revision `craft-i18n.rev` pins) and
  `CRAFT_FONTS_DIR=/home/markin/craft-fonts` (`b076417b`) went **EXIT=1** at
  `test` on `print::adding_custom_paper_overrides_pending_driver_default`, a new
  test of main's paper-detection work, at
  `crates/ui-egui/tests/it/print.rs:1577`: "No nodes found matching the query:
  By { label: "Custom…" }". The whole `pdfcraft-ui-egui --test it` target then
  passed **3/3 in isolation** (1869 tests, 34–36 s each) and the single test
  passes in 0.11 s, so that is a flake under the workspace-wide run (the test
  starts a paper-discovery worker and clicks as soon as the probe fires).
- **The green sweep**, third run, same environment: **EXIT=0**, `CI: all 10
  steps passed` — fmt, clippy, cli-no-default-features, test, i18n-report,
  layers, wasm, license-check, deny, parity. `test`: 156 targets, **9986
  passed, 0 failed, 18 ignored**. `layers`: 52 crates, no violation; `wasm32`:
  44 crates ok; `assets`: 258 repository assets, 7 bundled by dependencies, 22
  fetched at build time; `deny`: advisories, bans, licences and sources all ok;
  `parity`: 858 features (835 counted, 23 na), 56.8% shipped / 64.1% weighted
  (P0 90.5% shipped, 94.6% weighted); `i18n-report`: 5068 English strings, 0
  skipped catalog rows.
- **The push did not happen.** This agent's role denies `git push*`
  (`.opencode/agents/pdfcraft-envoy.md`); the shell tool refused the push and
  its `--dry-run` with `permission.rejected`. The fork `copoka/pdfcraft`
  `feature/realtime-collaboration` is still at `4f3e21245`, so PR #6622 does not
  yet carry `afd28a2f9` or this record. The owner's command is
  `git -C pdfcraft/upstream push fork HEAD:refs/heads/feature/realtime-collaboration`
  — a fast-forward, no force. `storytold/*` is not pushed to and `main` is
  untouched. (The push went through in a later pass: the fork and the PR's
  `headRefOid` became `80e99a2ed` — see the header and §4, "The merge of
  `origin/main` `ad3e6d4b8`".)
- **What this does not cover.** The host has no `qpdf`/`poppler`, so
  oracle-backed tests skip there; Windows-only checks stay CI's. The sweep ran
  on `afd28a2f9`, not on the STATUS commit above it.

### The JWT crypto backend — `deny` and RUSTSEC-2026-0387 (2026-11-10, local; not pushed)

`fix(collab): verify and sign JWTs with aws-lc-rs, not RustCrypto` (`f31f00449`)
on `f8e21e29d`, with `fix(ui): keep the selection caret behind the collab
feature` (`f48ec4754`) on it, which is the third reason `cargo xtask ci` had
gone red. The `cargo xtask ci` step of run `42720349704` (head `b27fa89c8`)
failed at `deny` on two counts: the fork's git source (§4, "The CI git-source
gate") and **RUSTSEC-2026-0387** — `crypto-bigint 0.5.5`, timing branches in its
constant-time arithmetic, fixed in 0.7.6 — which `deny.toml` does not ignore and
should not. It reached the tree along one chain only: `jsonwebtoken 11.1.0`
with the `rust_crypto` feature → `p256 0.13.2` / `p384 0.13.1` →
`ecdsa 0.16.9` → `elliptic-curve 0.13.8` → `crypto-bigint 0.5.5`; at
`f8e21e29d` `cargo tree -i crypto-bigint@0.5.5` printed exactly that and
nothing else.

- **Both manifests ask for `aws_lc_rs`.** `crates/collab-ws` (the transport's
  dependency) and `apps/pdfcraft-collab-server` (a dev-dependency: the crash
  test signs its own RS256 tickets) now use
  `features = ["aws_lc_rs", "use_pem"]`. jsonwebtoken 11.1 wants exactly one
  provider feature; with both or neither it panics at the first ticket
  ("Could not automatically determine the process-level CryptoProvider").
- **A repin was not available.** `jsonwebtoken` 11.1.0 (2026-09-16) is the
  newest release on crates.io and pins `p256 = "0.13.2"`, so the 0.14 stack
  `pdfcraft-sign` already uses (`bp256 0.14` → `elliptic-curve 0.14.1` →
  `crypto-bigint 0.7.6`) cannot be reached from the JWT side, and forking the
  crate over a timing advisory was not on the table.
- **No new native requirement, no wasm impact.** Both crates are native-only
  (`collab-ws` holds its one native dependency under
  `cfg(not(target_arch = "wasm32"))`, and the web client reaches the server
  through the host bridge), and `aws-lc-rs 1.18.2` is already built for the same
  binaries through `pdfcraft-sign` (ADR-0009). No source changed: `token.rs`
  keeps `DecodingKey::from_secret` for HS256 and `from_rsa_pem` for RS256.
- **The lock loses 17 packages and gains none**: crypto-bigint 0.5.5, p256
  0.13.2, p384 0.13.1, ecdsa 0.16.9, elliptic-curve 0.13.8, primeorder 0.13.6,
  sec1 0.7.3, rfc6979 0.4.0, base16ct 0.2.0, ff 0.13.1, group 0.13.0,
  fiat-crypto 0.2.9, hkdf 0.12.4, hmac 0.12.1, curve25519-dalek 4.1.3, ed25519
  2.2.3, ed25519-dalek 2.2.0. `crypto-bigint 0.7.6` stays, from
  `elliptic-curve` 0.14.1. No other package moved.
- **The workspace `clippy` step was red before this pass, for its own reason.**
  `DocView::selection_caret` has one reader, `collab.rs`, which exists only with
  the `collab` feature; without it the method is dead code and
  `cargo clippy --workspace --all-targets -- -D warnings` fails. It arrived
  with the presence caret (`5bad54c40`), whose slice was checked with
  `-p pdfcraft-ui-egui --features collab`, so the workspace sweep had been red
  since. The method now carries `#[cfg(feature = "collab")]`; the document's
  selection code is untouched and the feature build is still clean.

Gates at `f48ec4754` (host `agents`, rustc 1.99.1, `-j 4`; run by
`../scripts/remote-build.sh`):

| Gate | Command | Result |
|---|---|---|
| Advisories, bans, licences, sources | `cargo deny --log-level error check` (cargo-deny 0.20.2, installed for this pass) | EXIT=0 — `advisories ok, bans ok, licenses ok, sources ok` |
| The package is gone | `cargo tree -i crypto-bigint@0.5.5` | no such package; the only match is `crypto-bigint@0.7.6` |
| The lock | package list before and after | 17 packages removed, 0 added |
| The JWT crates | `cargo test -p pdfcraft-collab-ws -p pdfcraft-collab-daemon -j 8` | EXIT=0 — ws 16 unit + 36 e2e, daemon 34 + 5 integration (conflict resolve, crash recovery, publication retry, restore), 0 failed |
| Workspace clippy | `cargo clippy --workspace --all-targets -- -D warnings` | EXIT=0 |
| UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | EXIT=0 |
| The steps after `test` | `cargo xtask i18n-report --workspace`, `layers`, `wasm`, `license-check`, `parity` | EXIT=0 each — 52 crates with no layering violation, wasm32 44 crates ok, 258 assets ok |
| `cargo xtask ci` | `CI=1 cargo xtask ci` | **failed at `test`** at this head — both targets are closed by the merge and `c1171d941` (§4, "The full CI sweep"); `fmt`, `clippy` and `cli-no-default-features` passed |

The two `test` targets that stopped this run, kept for the record — the newer
subsection above replaces this reading:

- `pdfcraft-automation --test it`:
  `local_gotor_link_open_applies_destinations_and_honors_root` —
  `assert_eq!(listed["links"][0]["other"], "GoToR")` failed with `left: Null,
  right: "GoToR"`. This was **this branch's** `LinkAction::Remote` reporting,
  not a pre-existing `origin/main` failure: with a clean `origin/main` the test
  passes, and `c1171d941` closes it.
- `pdfcraft-ui-egui --test it`:
  `comments::the_selection_bar_stays_inside_the_pages_area` — the panic was at
  `set_option("language", "de")` (`language must be one of auto, en`), because
  the build embedded no craft-i18n catalogs; it was not the area rect. With
  `CRAFT_I18N_DIR` at the pinned `575cfdef` the test passes on a clean
  `origin/main` and here.

Both are `origin/main` tests (`f28406f68` and `952636315`); the automation one
lives in a file this branch edited for bookmarks, the UI one in a file the
branch never touched, and neither exercises the JWT stack.

### The CI git-source gate — one git source, storytold (2026-11-10, local; not pushed)

`fix(web): take craft-web-host from storytold, not the fork` (`158ab7fa4`).
CI run `42720349704` (pull request, head `b27fa89c8`) failed in
`git-sources` ("Git dependencies come from storytold", `ci.yml`): that step
accepts no git source outside `github.com/storytold/`, and the branch's
`Cargo.lock` carried one entry with
`source = "git+https://github.com/copoka/craft-libs?rev=7666e2b1…"`, which
`origin/main` does not have. The pin came from `apps/pdfcraft-web`, the only
manifest of the branch that named the fork: the collaboration host messages
(`session`, `session-state`, `presence`, `commit`, `committed`) are not in
`storytold/main` yet. At the PR head the manifest and the lock both named the
fork, so the lock was self-consistent and the source check is the sub-check
that rejects it (the job's log was not available while the run was still in
progress; the check is a single step). (Observed on 2026-11-10: that job
`failure`, the shared `gate` job still `in_progress`; this pass fixes only the
job named in the brief.)

- **The same commit, taken from storytold.** `storytold/craft-libs#716`
  ("feat(web-host): collaboration session messages in the host contract",
  `main` ← `copoka:feat/collab-session-messages`, open) has the head
  `7666e2b1…`, and `git ls-remote https://github.com/storytold/craft-libs`
  lists it as `refs/pull/716/head`, so the commit is reachable in the upstream
  repository even though no branch there contains it. The manifest now pins
  `https://github.com/storytold/craft-libs` at that rev; `cargo` fetched it by
  SHA from storytold (the log of the lock rebuild says `Updating git repository
  https://github.com/storytold/craft-libs` and `Adding craft-web-host … storytold`).
  No code changes: the rev is the same, so the crate compiled before and after
  is identical.
- **The lock moves in one line.** The rebuilt `Cargo.lock` differs from the
  previous one in the `source` of `craft-web-host` alone (`copoka` →
  `storytold`); no other package moved, and no other pin was touched. The branch
  still has `craft-web-api` at `fd5e2fe…` and the workspace libraries at
  `787a7a0f…`/`7ee35a9f…`/`1b34eba4…`, all storytold.
- **What it does not fix.** The pin is still a commit outside `storytold/main`
  (PR #716 is open). When it merges, the rev can move to the merge commit and
  the comment in `apps/pdfcraft-web/Cargo.toml` goes away. Nothing else in the
  gate is affected: the shared `gate` job (fmt, clippy, tests, wasm, assets)
  keeps running from `craft-common-actions@main`.

Gates at `158ab7fa4` (host `agents`, rustc 1.99.1, `-j 4`; all run by
`../scripts/remote-build.sh`):

| Gate | Command | Result |
|---|---|---|
| CI job `git-sources`, lock step | `cargo metadata --locked --format-version 1` | EXIT=0 |
| CI job `git-sources`, source step | the `ci.yml` python check over `Cargo.lock` and `.cargo/config.toml` | pass — no git source outside `github.com/storytold/` (`storytold/craft-libs`, `storytold/egui`, `storytold/winit`, `storytold/winprint.rs`); no `source` replacement, `patch` or `paths` |
| Formatting | `cargo fmt --all -- --check` | EXIT=0 |
| Clippy, the collab crates | `cargo clippy -p pdfcraft-collab-server -p pdfcraft-collab-ws -p pdfcraft-collab-pdf -p pdfcraft-collab-outline -p pdfcraft-collab-client-pdf --all-targets -- -D warnings` | EXIT=0 (one pre-existing warning in the vendored `hayro-interpret`) |
| Web shell, the crate whose dependency changed | `cargo check -p pdfcraft-web --target wasm32-unknown-unknown --features collab` | EXIT=0 — 22 warnings, main's native-only helpers unused on wasm, none from the bridge |
| Web shell, feature off | `cargo check -p pdfcraft-web --target wasm32-unknown-unknown` | EXIT=0 |
| Tests | `cargo test -p pdfcraft-collab-server -p pdfcraft-collab-ws -p pdfcraft-collab-pdf -p pdfcraft-collab-outline -p pdfcraft-collab-client-pdf` | EXIT=0 — client-pdf 48, outline 9, pdf 71, server 33, ws 16 unit + 36 e2e; **213 passed, 0 failed, 0 ignored** |
| Not a gate here | `cargo clippy -p pdfcraft-web --target wasm32-unknown-unknown --features collab --all-targets -- -D warnings` | EXIT=101 — five `-D warnings` on main's native-only code in `pdfcraft-render` under wasm (unused imports, methods and fields), pre-existing and unrelated to this change; the repository's wasm gate is `cargo check`, as the tables above use |

Nothing was pushed in that pass: PR #6622 kept the fork's head `b27fa89c8`, so its
next run still saw the old lock until the owner force-pushes. The job was red on the
PR head, not on this tree; the branch's head is now green for that job.

### The whole outline replaced — §5 p.3 / §6.6 (2026-11-10, local; not pushed)

`feat(collab): replace the whole outline in one operation` (`642dabb5b`) on
`d2a1bcf64`. This closes the last outline item §6.6 left open: `ImportBookmarks`
with `replace` was `unsupported` in a room, and so were the from-structure and
from-text-styles generators, because each replaces more of the outline than one
addressed item.

- **One operation over the whole tree** (RFC §3.1.7). `outline.replace` is
  addressed to the outline itself (`Target::OutlineItem { key: None }`, not an
  item), and its payload is `items` — the complete outline in the same wire form
  the subtree copy uses (`title`, `page`, `view`, `color`, `italic`, `bold`,
  `open`, `children`). A node may name its destination in the full `dest` form
  instead (including `named`): the name is checked against the document and
  written as that name's page and view, because the accepted operation and the
  journal carry a tree of pages. `epoch` is the compare-and-set, as for
  `dest.*`: an operation built on another epoch is `stale` (retryable) before
  the document is read.
- **The undo image is the whole tree it replaced.** The accepted operation
  carries `items` (as written, normalised), `before` (the document's whole
  outline before the write) and `count`; the undo is one more `outline.replace`
  over `before`, and a redo repeats the original operation. An empty `items`
  clears the outline: the engine's import refuses an empty list, so every
  replica applies a batch of top-level deletions — one edit, one undo step —
  which is also the undo of a document that had no outline.
- **Every refusal happens before the first write**: a target that is not the
  outline root, a missing or non-list `items`, a blank or over-long title
  (512 bytes), a page past the last, a name this document does not define, a
  name that opens no page, a view that is not Table 149 or not of its arity, a
  colour that is not three numbers from 0 to 1, nesting deeper than 32 levels,
  more than 2 000 nodes or more than 128 KiB of JSON (`limit`), an empty tree on
  an empty outline (`invalid`), and a passed `epoch` that is not the current one
  (`stale`). None of them moves `version`, `epoch`, the document or the journal.
  The same bounds are checked on the document's own outline for `before`: an
  outline too large for one operation cannot be replaced, because its undo
  could not travel.
- **The write is one edit**: `Edit::ImportBookmarks` with `replace: true`, or —
  for an empty tree — `Edit::Batch` of `DeleteBookmark { path: [0] }` for each
  top-level item. A replacement rebuilds every bookmark object, so a key a
  replica took before it is `invalid` afterwards (the same honest boundary as
  `outline.item.restore`).
- **Page operations after a replacement keep the targets**: a bookmark is
  written with the page object, so insert/duplicate/blank/combine/move/split/
  delete/replace keep each item on its page and only the number moves; the
  server recomputes key→path before every operation, so a key taken before a
  page operation still addresses its item (the test checks a blank insert and a
  rename by a key taken before it).
- **The client projects both ways**: `Edit::ImportBookmarks` with `replace`
  becomes one `outline.replace` (`outline_replace_payload`); an accepted
  operation becomes the same `ImportBookmarks` (or the batch of deletions for an
  empty tree); `inverse_for` returns `None` and `inverse_after` builds the undo
  from `before`. `collab_apply_edit` already lists `ImportBookmarks` as
  structural, so a commenter never builds it and a viewer never sends it.
- **The generators run in the replica**: `Document::structure_bookmarks()` and
  `Document::text_style_bookmarks()` read the headings (the engine's own
  `pdfcraft_a11y::headings` and `text_style_bookmarks::headings`), and
  `collab-outline::tree_from_entries` nests them the way `add_bookmark_tree`
  does — one expanded "Untitled" parent first, entries nested by level, each at
  the top of its page (`/XYZ`, zoom kept). The operation is one
  `outline.replace` with the new parent followed by the document's own
  top-level items, so a room leaves the list a lone document would have left
  (the engine inserts the parent first too). `TextStyle::matches` became public
  so the client refuses a level list with two equal styles exactly as the
  engine does; an untagged document, no levels, a duplicate style and no text
  in the chosen styles are the engine's own messages. A structure link (`/SE`)
  is not part of the wire, so generated bookmarks carry none.

Tests:

| Crate | Test |
|---|---|
| `collab-outline` | `entries_nest_by_level_under_one_parent` (a parent, nesting by level, each heading's top, the whole tree round-trips through the wire, no entries is an empty parent) |
| `collab-pdf` | `the_whole_outline_is_replaced_as_one_operation` (one epoch step; the accepted `items`/`before`/`count`; a named destination resolved to its page; look and view written; undo over `before`, redo; a blank insert keeps every target and a key taken before it still renames its item; an empty tree clears and its undo restores), `a_bad_outline_replace_changes_nothing` (13 refusals — an item target, no items, items not a list, a blank and a long title, a missing page, a missing name, an unknown view, a `FitR` with a hole, depth 33, 2 001 nodes, over 128 KiB of JSON, a stale epoch — each leaving version, epoch and outline unchanged; a document whose own outline is over the budget is `limit`) |
| `collab-client-pdf` | `bookmark_edits_become_addressed_operations` (`replace` → one `outline.replace` at the outline root with `items` and `epoch`; a `replace` with a parent refused), `accepted_outline_operations_become_edits` (an accepted `outline.replace` → `ImportBookmarks` with `replace`), `a_replacement_undoes_into_one_more_replacement` (the undo op over `before`, the empty tree as a batch of top-level deletions, its own undo restoring the tree), `generator_edits_become_one_outline_replacement` (from structure: one operation, the new parent then the document's list, the tree parses; an untagged document refused), `text_style_generators_become_one_outline_replacement` (one operation, a heading per page; no levels, two equal styles and no text refused) |
| `collab-ws` e2e (socket) | `the_whole_outline_is_replaced_over_a_socket` (two editors and a viewer: a replacement converges on all three replicas, its undo and the redo apply, a stale epoch is `stale` without a version move, a viewer `forbidden`) |

Gates at `642dabb5b` (host `agents`, rustc 1.99.1, `-j 4`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | EXIT=0 |
| Clippy, the crates this pass touched | `cargo clippy -p pdfcraft-collab-outline -p pdfcraft-collab-client-pdf -p pdfcraft-collab-pdf -p pdfcraft-collab-ws -p pdfcraft-engine --all-targets -- -D warnings` | EXIT=0 |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | EXIT=0 |
| App wiring | `cargo check -p pdfcraft --features collab` | EXIT=0 |
| Tests, the collab crates of the slice | `cargo test -p pdfcraft-collab-outline -p pdfcraft-collab-client-pdf -p pdfcraft-collab-pdf -p pdfcraft-collab-ws` | EXIT=0 — client-pdf 48, outline 9, pdf 71, ws 16 unit + 36 e2e; 0 failed, 0 ignored |
| Tests, the engine the reads and edits live in | `cargo test -p pdfcraft-engine` | EXIT=0 — engine 636 + 9 + 46; 2 ignored (pre-existing) |

What this does **not** cover: an outline over one operation's budget (2 000
nodes, 128 KiB JSON) cannot be replaced in a room, since its `before` image
would not fit; a replacement rebuilds every bookmark object, so keys taken
before it are invalid; a generator loses the structure link (`/SE`) a lone
document keeps; and `outline.replace` refuses an empty tree on an empty outline
instead of recording an epoch bump with nothing in it. Nothing was pushed in that pass: PR
#6622 kept the fork's head `b27fa89c8`, and the pass's two commits stayed local.

### Whole-document link operations — §5 p.3 / §6.6 (2026-11-10, local; not pushed)

`feat(collab): share whole-document link operations`. This closes the last item
§6.6 left open: `Edit::AddLinks` and `Edit::RemoveLinks` were `unsupported` in a
room because they touch the whole document, and the protocol had only
per-page link operations. Now they are two document operations over several
pages, so every link action the app and automation offer is shared.

- **One operation over every page it touches** (RFC §3.1.9). `links.add_many`
  carries `pages: [{"page": N, "links": [...]}]` — the links each page gains, in
  the order of the text — and `links.remove_many` carries the same shape, where
  `links` is the list the page keeps (an entry without one, or an empty list,
  removes every link on the page). The address is the first page the operation
  touches (`Target::Page`, its `epoch` checked like every page operation), and
  the payload names the rest; a document that moved is `stale` and the client
  retries against the new head. One operation is one epoch step, one journal
  record and one host-version step, not one per page.
- **Every refusal happens before the first write**: a page twice in one
  operation, a page past the end, an operation that names no page, an
  `add_many` without a link, a page already holding a link the model cannot
  carry (JavaScript, Launch, a malformed area — `unsupported` whole), an area
  that is not four finite numbers or is under 2 pt a side, a target page past
  the last, a blank or over-long address (`MAX_LINK_URI_BYTES`, 2 048), a list
  over `MAX_LINKS_PER_PAGE` (1 024) or a document over `MAX_LINKS` (10 000)
  (`limit`), a `remove_many` that would change nothing (`invalid`), and a
  `before` that no longer matches the pages (`conflict`). None of them moves
  `version`, `epoch`, the document or the journal, and a viewer or a commenter
  gets `forbidden` from the room before the adapter runs.
- **The undo image is every touched page's links, whole.** The accepted
  operation carries `pages` (the lists it leaves) and `before` (the lists it
  replaced), plus `added`/`removed`; the undo is one `links.remove_many` over
  `before` with `pages` as its check, so an add and a removal undo through one
  shape and neither replica guesses. A redo repeats the original operation.
- **The engine writes the lists in one edit**: `Edit::SetPagesLinks` over
  `pdfcraft_annot::links::set_pages_links`, which checks every page (the page,
  every body, and the links the page holds now) before the first write and then
  rewrites each list with the same in-place rules as `page.link.set` — an
  unchanged link keeps its annotation object.
- **The client projects both ways**: `Edit::AddLinks` becomes one
  `links.add_many` (the `(page, rects, uri)` findings grouped by page in the
  text's order), `Edit::RemoveLinks` one `links.remove_many` (`pages: None` is
  every page of the document); `edit_for` applies an accepted operation as
  `Edit::SetPagesLinks`, and `inverse_after` builds the undo from the accepted
  operation. `collab_apply_edit` lists `SetPagesLinks` as structural, and
  `automation`'s `links_from_urls`/`links_remove` reach the room through the
  same edits.
- **Page operations still keep the links**: the added links go through the same
  annotation objects, so insert/duplicate/blank/combine/move/split/delete/
  replace keep each link on its page and renumber a destination written as a
  page number (the e2e test checks a go-to added by `add_many` after a blank
  insert).

Tests:

| Crate | Test |
|---|---|
| `collab-link` | `pages_round_trip_and_are_bounded` (the same pages come back; a page twice, one past the bound and a list over the room's total refused before a document is read) |
| `annot` | `several_pages_are_written_whole_and_a_bad_one_writes_nothing` (a bad area on the second page leaves the first untouched; the same span without it writes both and survives a save) |
| `collab-pdf` | `many_links_are_added_and_removed_as_operations` (three links on two pages in one epoch step; the undo over the lists it replaced; a redo; a plain removal and its undo; a stale check is `conflict` and moves nothing), `a_bad_many_link_operation_changes_nothing` (16 refusals — no pages, a page past the end, no link, a page twice, small and huge areas, a target page past the end, a blank and a long address, an unknown target, too many links, no change, and a stale `before` — each with version, epoch and document unchanged), `a_page_with_an_unshared_link_refuses_a_whole_document_operation`, `a_viewer_and_a_commenter_cannot_remove_many_links` |
| `collab-client-pdf` | `link_edits_become_addressed_operations` (both whole-document edits become one operation addressed at the first page; each page listed once with its links), `accepted_link_operations_become_edits` (an accepted add and removal through `edit_for`; the undo over the lists they replaced with the lists they left as the check) |
| `collab-ws` e2e (socket) | `whole_document_links_are_operations_over_a_socket` (two editors and a viewer: a link on each of two pages in one operation converges on all three replicas; the undo and the redo apply; a plain removal and its undo; a stale check is `conflict` without a version move; a viewer `forbidden`; a blank insert keeps each link on its page and a page-number go-to follows the page it named) |

Gates at `feat(collab): share whole-document link operations` (host `agents`,
rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | EXIT=0 |
| Clippy, the crates this pass touched | `cargo clippy -p pdfcraft-annot -p pdfcraft-engine -p pdfcraft-organize -p pdfcraft-collab-{proto,link,pdf,client-pdf,ws} -p pdfcraft-automation --all-targets -- -D warnings` | EXIT=0 |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | EXIT=0 |
| App wiring | `cargo check -p pdfcraft --features collab` | EXIT=0 |
| Tests, the four crates of the slice | `cargo test -p pdfcraft-collab-link -p pdfcraft-collab-client-pdf -p pdfcraft-collab-pdf -p pdfcraft-collab-ws` | EXIT=0 — client-pdf 45, link 7, pdf 69, ws 16 unit + 35 e2e; 0 failed, 0 ignored |
| Tests, the crates whose edits and links changed | `cargo test -p pdfcraft-annot -p pdfcraft-engine` | EXIT=0 — annot 198, engine 636 + 9 + 46; 2 ignored (pre-existing) |

What this does **not** cover: a link whose action the model cannot carry
(JavaScript, Launch) still refuses its page — and therefore the whole
whole-document operation — whole; a link copied in from another file keeps its
own file's page numbers; and a link to a named destination is not rewritten
when `dest.delete` removes that name (its undo puts the name back). The state
is still local: nothing was pushed, and PR #6622 keeps the fork's head
`b27fa89c8`.

### A bookmark's look and the named-destination interface — §5 p.3 / §6.6 (2026-11-10, local; not pushed)

`f262cb8d5` and `628f6176e` on `aa4f23dc4`. This closes what the outline pass left open: the
panel's colour, bold/italic and open-state edits, and the named destinations the
app had no editor for (`d1229deb9`'s §4 section, last paragraph). Both are
protocol and interface, not new operations: the look travels in the
`outline.item.update`/`add` that already exist.

- **The update carries the look.** `color`, `bold`, `italic` and `open` are
  optional fields of `outline.item.update`, next to `title` and `dest`, and
  `add` takes `color`/`bold`/`italic`. The wire form is
  `collab_outline::ItemLook`: a colour is `null` (the default black, `/C`
  removed) or exactly three numbers from 0 to 1, the other three are booleans.
  An absent field is left alone, so a rename does not touch the look, and the
  accepted operation names only what it changed. On the engine side
  `Edit::UpdateBookmark` gained `color: Option<Option<[f64; 3]>>`, `bold`,
  `italic` and `open`, and one edit writes all of them (`run_edit`'s
  `update_bookmark`), so an operation is one undo record, not five.
- **The undo image is the whole node**: `before {title, dest, color, italic,
  bold, open}`. The interface's listing rounds a colour to an `f32`, so a
  shared session reads the `/C` at the file's own precision through
  `Document::bookmark_color` (new in `pdfcraft-organize` behind the engine):
  an undo writes back `0.1`, not `0.10000000149011612`. The inverse names only
  the fields the operation named, with the values `before` held — a colour that
  was removed comes back as `null`, and an `open` state is only put back when
  the update named one (an item whose children are gone has no state to
  restore, and the room would refuse it).
- **Refusals happen before the first write**: a colour that is not three
  numbers or is outside 0..1, `bold`/`italic`/`open` that are not booleans
  (`invalid`), an `open` on a bookmark with no children (`invalid` — there is no
  `/Count` to change, so the room says so rather than writing nothing), an
  `add` that names `open` (a new item has no children), and an `update` with no
  title, no destination and no look field (`invalid`). The renderer's own write
  of an open state (`set_bookmark_open`) stays a no-op on a childless item; the
  room is the layer that refuses. None of the refusals moves `version`, `epoch`,
  the document or the journal; `collab-pdf`'s refusal test grew ten cases.
- **The panel sends it.** The Bookmarks context menu has *Expanded in the
  document* (disabled without children, ISO 32000-2 Table 151's sign of
  `/Count`); the triangle stays the reader's own view of the file's state, as
  before. `BmAction::SetOpen` becomes `Edit::UpdateBookmark { open, … }`, and
  the existing *Colour* and *Style* menus (`SetBookmarkColor`,
  `SetBookmarkStyle`) project to the same one `outline.item.update`
  (`collab-client-pdf::ops_for_edit`), addressed by the item's object key.
  `collab_apply_edit` already lists `UpdateBookmark` as structural, so a
  commenter never builds the operation and a viewer is refused by the room.
- **Named destinations have an interface**: Bookmarks ▸ Named destinations…
  (`bookmark.destinations`, `crates/ui-egui/src/dialogs/named_dests_ui.rs`)
  lists every `/Names /Dests` entry — name, the page it opens and its view —
  and adds, moves or deletes one. The form takes a name, a 1-based page and a
  Table 149 view type with its values (a blank value is a hole, the reader
  keeps its position or zoom). Every change is `Edit::SetNamedDestination` or
  `Edit::DeleteNamedDestination` through `apply_edit`: in a session that is
  `dest.add`/`dest.update`/`dest.delete` and the dialog never writes the
  document itself; outside one it is the ordinary undoable step. A refusal (a
  `commenter` or a `viewer` in a room, a page the document lacks) is shown in
  the form.
- **Headless parity**: `crates/automation/src/named_dests.rs` adds
  `destinations` (name → page and view), `destination_set` (add or move a name,
  the type's own number of values checked before the write) and
  `destination_delete`, following `bookmark_io.rs`'s shape;
  `bookmark_set_style` (`628f6176e`) gained `open`, so the state the panel now
  writes is reachable without the interface too — one `Edit::UpdateBookmark`,
  refused for a bookmark with no children rather than reporting a write the
  engine does not make. `parity/`'s `view.destinations-panel` moves from
  `planned` to `shipped`, `organize.bookmark-expand-collapse` lists
  `bookmark_set_style` as its tool, and its note no longer says the open state
  is never written back.

Tests:

| Crate | Test |
|---|---|
| `collab-outline` | `a_look_round_trips_and_leaves_out_what_it_did_not_name` (colour, bold, open; `null` clears; an unnamed style is left alone; the empty look), `a_look_that_is_not_what_it_must_be_is_refused` (two and four components, 1.1, −0.1, strings, a colour that is not a list, `bold: 1`, `italic: "yes"`, `open: null` and `open: 1`) |
| `organize` | `opening_a_bookmark_without_children_changes_nothing` (no `/Count` to close or open; a parent's does change), `a_bookmarks_colour_is_read_back_at_the_files_precision` (`0.1` stays `0.1`, black is no colour, a missing bookmark refused) |
| `collab-pdf` | `a_bookmark_look_is_updated_with_the_whole_node_as_its_undo_image` (colour + bold + italic, the accepted fields, `before` as the whole node; colour cleared and the parent closed, the `before` of both; an add that carries a look), `a_bad_outline_operation_changes_nothing` +10 cases (a colour of two or four, 1.5 and a string, `bold: 1`, `italic: "yes"`, `open` on "Two", `open` and a bad colour on an add) |
| `collab-client-pdf` | `bookmark_look_edits_become_one_update_and_its_undo` (`SetBookmarkColor`, `SetBookmarkStyle` and an open-state `UpdateBookmark` each become one `outline.item.update` naming only what changed; `edit_for` applies `color: null` as the default black and leaves an unnamed slant alone; the inverse names the look `before` held, in the version and epoch the update left) |
| `collab-ws` e2e (socket) | `a_bookmarks_look_converges_and_undoes_over_a_socket` (two editors and a viewer; colour/weight/slant converge on all three, the undo from the accepted operation puts the look back, the redo returns it, `open: false` closes the parent everywhere with `before.open` true, a viewer `forbidden` with no change, a vanished key `invalid` with no version move) |
| `automation` | `named_destinations_through_tools` (listed empty, XYZ and Fit added, an existing name moved without a second entry, seven refusals — blank and 129-byte names, a page past the last, an unknown view, two values for XYZ, a `FitR` with a hole, a value for `Fit` — with nothing written, a delete, a name that is gone refused, and the name still there after a save and reopen); `bookmarks_through_tools` (`bookmark_set_style open: false` on a parent, one undo reopens it, a leaf refused) and `bookmark_list_reports_colour_and_style` (a leaf's `open` refused beside the colour and style refusals) |

Gates at `628f6176e` (host `agents`, rustc 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, the crates this pass touched | `cargo clippy -p pdfcraft-engine -p pdfcraft-organize -p pdfcraft-collab-{outline,pdf,client-pdf,ws} -p pdfcraft-automation --all-targets -- -D warnings` | pass (the automation crate again after `628f6176e`) |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, the outline and its look | `cargo test -p pdfcraft-collab-outline -p pdfcraft-collab-pdf -p pdfcraft-collab-client-pdf` | pass — outline 8, pdf 65, client-pdf 45; 0 failed, 0 ignored |
| Tests, the socket | `cargo test -p pdfcraft-collab-ws` | pass — ws 16 unit + 34 e2e; 0 failed |
| Tests, the engines the edits live in | `cargo test -p pdfcraft-organize -p pdfcraft-engine` | pass — organize 232 + 1 + 1 integration, engine 636 + 9 + 46; 2 ignored (pre-existing) |
| Tests, automation | `cargo test -p pdfcraft-automation` | 433 of 434 pass, `named_destinations_through_tools`, `bookmarks_through_tools` and `bookmark_list_reports_colour_and_style` included; the one failure, `local_gotor_link_open_applies_destinations_and_honors_root`, was **this branch's** `Remote` reporting and is fixed in `c1171d941` (§4, "The full CI sweep") — the earlier "pre-existing at `aa4f23dc4`" note compared the pass against the branch with its changes stashed, not against `origin/main`, which passes |
| App wiring | `cargo check -p pdfcraft-ui-egui --features collab --all-targets` | pass |
| Layers | `cargo xtask layers` | OK — 52 crates, no violations |
| Parity | `cargo xtask parity` | exit 0 (its notes list shipped features without a tool, as before) |

What this does **not** cover: replacing the whole outline
(`ImportBookmarks` with `replace`) and the from-structure/from-text-styles
generators stay `unsupported` in a room; a named destination cannot be renamed
in one step (the dialog adds the new name and deletes the old one, two steps in
the history); a destination that is a name of a name, and one that goes nowhere,
still cannot be written through `dest.*`; and the Bookmarks triangle remains
the reader's view toggle rather than a document edit. Nothing was pushed: the
four commits are local and PR #6622 keeps the fork's head `b27fa89c8`.

### Links — §5 p.3, the last item of TASK §3's list (2026-11-10, local; not pushed)

`2b61ff80f` on `04ac8f2ea`. Links were the last thing TASK §3 named that a room
could not share: the Link Properties dialog's edits were refused as
`unsupported`, a remote go-to read as `Other("GoToR")`, and a commit could not
carry a link a page operation had moved.

- **One page's links are the unit of the operation** (RFC §3.1.9, decision 24).
  `page.link.set` makes the page's links exactly the list it names;
  `page.link.clear` removes one entry, named by its position among the page's
  links. Both are addressed by the page (`Target::Page`, epoch checked) and are
  structural: head version + lease, `epoch++`, and a `viewer`/`commenter` gets
  `forbidden` from the room before the adapter runs; the client lists
  `AddLink`/`SetLink`/`DeleteLink`/`RemoveLinks`/`AddLinks`/`SetPageLinks` as
  structural, so a commenter sees ReadOnly and neither `version` nor `epoch`
  moves.
- **A link travels typed** (`crates/collab-link`, `LinkContent` in the engine):
  the area (`rect`, or the `quads` of a text area — the rectangle is their box),
  the target and the look. Targets are a page of this document with a view
  (`fit`, `fit_width`, `fit_height`, `fit_visible`, `inherit`, `percent`), a
  named destination (`LinkAction::Named`), a page of another file
  (`LinkAction::Remote`, `/S /GoToR`, with the path kept as written) and a web
  address. `style` is the visible/invisible rectangle, colour, width,
  dash/underline and highlight; a payload without one gets a new link's look.
- **Every refusal happens before the first write**: a page outside the document
  or in another epoch; an area that is not four finite numbers, smaller than
  2 pt on a side, or past `MAX_LINK_COORD`; empty quads, quads not of eight
  numbers or more than `MAX_LINK_QUADS` (1 024); a target page past the last;
  a blank name, a blank or over-long address (`MAX_LINK_URI_BYTES`, 2 048), a
  blank or over-long remote path (`MAX_LINK_REMOTE_FILE_BYTES`, 4 096), an
  unknown target or zoom; a list over `MAX_LINKS_PER_PAGE` (1 024) or a
  document over `MAX_LINKS` (10 000); a clear index outside the list; and a
  `before` that no longer matches the page (`conflict`, the undo's
  compare-and-set). A page that already holds a link the model cannot write
  back (JavaScript, Launch, a malformed area) is `unsupported` whole. None of
  them moves `version`, `epoch`, the document or the journal.
- **The undo image is the page's links, whole.** The accepted operation carries
  both lists (`links` after, `before` before; a clear also names `removed`), so
  every replica and the journal write the same page, and the undo is an
  ordinary `page.link.set` over `before` with `links` as its check. No
  server-side copy is made, and a redo is the original operation.
- **The engine writes a page's list in place where it can**:
  `Edit::SetPageLinks` keeps the annotation object of a link that did not
  change, rewrites a changed one (`Rect`, `QuadPoints`, `/BS`, `/C`, `/H`,
  `/Dest` or `/A`), deletes the surplus from the last and appends the new ones;
  the annote crate's `bodies` refuses to read a page whose links the model
  cannot carry, before anything is written.
- **Page operations keep a link's page and renumber the numbers it names**
  (`crates/organize/src/page_links.rs`): insert, duplicate, blank insert,
  combine, move, split, delete and replace keep the page object, so an object
  destination follows its page; a destination written as a page *number*
  (`[1 /Fit]`, `/D [1 /Fit]`) is rewritten to the new index, a link to a
  deleted page loses its destination (as a bookmark does), and a remote go-to
  and a named destination are left alone — their numbers and names are not this
  document's. Pages brought in from another file keep their own file's numbers.
- **The client projects both ways**: `Edit::AddLink` appends to the list this
  replica has, `Edit::SetLink` changes the entry the `/Annots` index names
  (area, target and look — whichever is given), `Edit::DeleteLink` becomes
  `page.link.clear` with the link's position; `edit_for` turns both accepted
  operations into `Edit::SetPageLinks`, and `inverse_after` swaps the two lists
  the server carried. `Edit::RemoveLinks` and `Edit::AddLinks` (remove every
  link; make links from the addresses in the text) were `unsupported` in a
  room at this commit — whole-document actions with no operation then; §4's
  whole-document links section closes them.

Tests:

| Crate | Test |
|---|---|
| `collab-link` | `every_target_round_trips`, `a_text_area_is_its_box_and_travels_whole`, `a_bad_area_is_refused` (no area, small, not numbers, not four, past the bound, empty quads, a quad of seven, over the quad bound), `a_bad_target_is_refused` (no/unknown kind, page past the last, empty and long address, empty remote file, a remote name, a zoom of zero, a blank name), `a_look_is_optional_and_bounded` (a colour over one or of two, a negative or wide width, dashed and underlined, an unknown highlight), `a_list_is_bounded` |
| `annot` | `a_remote_go_to_and_a_named_destination_round_trip` (both survive a save; a GoToR writes `/D` as a number, never a page of this document), `a_page_link_list_is_written_whole_and_keeps_what_did_not_change` (a quad area, a changed target, a third link, the surplus removed; the unchanged link keeps its object), `a_link_the_model_cannot_carry_refuses_the_whole_page` (a JavaScript action: reading and rewriting refuse, nothing changes) |
| `organize` | `page_numbers_in_links_follow_the_pages_they_name` (a move, then a blank insert; the remote go-to and the name untouched), `a_link_to_a_deleted_page_loses_its_destination` (the annotation stays, its destination goes) |
| `collab-pdf` | `page_links_are_set_and_cleared_as_operations` (a web address, then a page with a text area; the first link keeps its object; clear, its lists and `removed`; the undo writes the old list), `a_bad_link_operation_changes_nothing` (11 refusals — page past the end, stale `before`, small and huge areas, target page past the end, blank and long address, unknown kind, too many links, clear outside the list, clear with no index — each with version, epoch and document unchanged), `a_page_with_an_unshared_link_is_refused_whole`, `a_viewer_and_a_commenter_cannot_change_links`, `page_operations_keep_the_links_on_their_pages` (a blank insert, then a delete) |
| `collab-client-pdf` | `link_edits_become_addressed_operations` (an add, a change by `/Annots` index, a delete as a position, a missing index refused; the whole-document actions as one operation), `accepted_link_operations_become_edits` (set and clear through `edit_for`, the inverse swap and its epoch) |
| `collab-ws` e2e (socket) | `page_links_are_operations_over_a_socket` (two editors and a viewer; a web address, then a page with a view, a name and another file's page converge on all three replicas; a stale check is `conflict` without a version move; the undo and the redo apply; a clear and its undo; a viewer `forbidden`; a blank insert keeps both links on their page) |

Gates at `2b61ff80f` (host `agents`, rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, the crates this pass touched | `cargo clippy -p pdfcraft-annot -p pdfcraft-engine -p pdfcraft-organize -p pdfcraft-collab-{proto,link,pdf,client-pdf,server,ws} -p pdfcraft-automation --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, the gate list | `cargo test -p pdfcraft-collab-server -p pdfcraft-collab-ws -p pdfcraft-collab-pdf` | pass — server 33, ws 16 unit + 33 e2e, pdf 64; 0 failed, 0 ignored |
| Tests, the rest of the collab set | `cargo test -p pdfcraft-collab-{proto,link,outline,hub,client,client-pdf,session,annot}` | pass — proto 15, link 6, outline 6, hub 5, client 14, client-pdf 44, session 5, annot 197; 0 failed |
| Tests, the crates whose links moved | `cargo test -p pdfcraft-organize -p pdfcraft-engine` | pass — organize 230 + 2 integration, engine 636 + 9 + 46; 2 ignored (pre-existing) |
| Daemon | `cargo test -p pdfcraft-collab-daemon` | pass — 34 + `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1 + `restore_publication` 2 |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| App on the browser | `cargo check -p pdfcraft-ui-egui --features collab --target wasm32-unknown-unknown` | pass |
| Workspace | `cargo check --workspace --all-targets` | pass (one pre-existing warning in vendored `hayro-interpret`) |
| UI, the collab module | `cargo test -p pdfcraft-ui-egui --features collab --lib collab::` | pass — 16 |
| Layers | `cargo xtask layers` | OK — 52 crates, no violations |
| Browser | `cargo xtask wasm` | 44 crates ok |

What this does **not** cover: a link
annotation's subtype is assumed `Link` with a rectangle and an action the model
carries, and anything else refuses its page whole; a page's link list is
rewritten as one operation, so two editors adding a link to the same page at
the same moment are two operations that the server serialises (one is
`stale`, retried against the new head) rather than a merge; a link copied in
from another file keeps its own file's page numbers; and a link whose
destination is a *named destination* is not rewritten when `dest.delete`
removes that name (the undo of the delete puts the name back). A "Number
pages" span that would cross several existing ranges is still refused
(`conflict`) rather than split: the dialog labels one range at a time, exactly
as recorded in the page-labels section below. Nothing was pushed: PR #6622
keeps the fork's head `b27fa89c8`.


### Page labels — §5 p.3 (2026-11-10, local; not pushed)

`c26867e9c` on `b3732c152`. A page label is a `/PageLabels` number-tree range
(ISO 32000-2 §12.4.2): from a page on, every page shows a prefix and a number
in a style, until the next range starts. The room had no operation for one: the
"Number pages" dialog was refused as `unsupported` in a session, and a commit
could not carry labels a page operation had moved.

- **Two structural operations, addressed by the page a range starts on**
  (RFC §3.1.8): `page.label.set` (target `page_label { start }`, payload
  `end`, `style`, `prefix`, `first`) writes `start..=end`; `page.label.clear`
  removes the range that begins at `start`. Styles are the wire's own names:
  `decimal`, `upper_roman`, `lower_roman`, `upper_alpha`, `lower_alpha`,
  `none` (prefix only). Both bump the epoch and are refused for a `viewer` and
  a `commenter` before the adapter runs; the client lists
  `Edit::NumberPages` and the new `Edit::ClearPageLabels` as structural, so a
  commenter sees ReadOnly and the room answers `forbidden` without moving
  `version`.
- **Every refusal happens before the first write**: a span past the document or
  empty, an unknown style, a zero first number and a blank label are `invalid`;
  a label over `MAX_PAGE_LABEL_BYTES` (1 024), a number past 32 bits and a
  result over `MAX_PAGE_LABEL_RANGES` (1 024) are `limit`; a span that would
  cross another range's start is `conflict` (the undo image is one range, so
  swallowing a second one would not be an undo); a `/PageLabels` tree that is
  cyclic, too deep or over its work bound is `invalid` rather than rewritten
  blind. A clear of a page that begins no range is `invalid`. None of them
  moves `version`, `epoch`, the document or the journal.
- **The accepted operation carries the whole range it replaced** (`before`,
  `{start, end, style, prefix, first}`), so the undo is an ordinary
  `page.label.set`. A document without `/PageLabels` shows plain numbers: its
  undo image is the implicit `0..last` decimal range, marked `implicit` (the
  pages before a set are then not covered by a range). A set that lands inside
  a range splits it, so its undo names the ranges the set left over that span
  (a list in `before`) and writes one range over them: a compare-and-set, so a
  label someone changed in between is a `conflict`, not an overwrite. A clear
  does not split, and its undo is a plain set.
- **Page operations keep each page's label.** `pdfcraft-organize` gained a
  label snapshot and a plan: insert (from a file, a restore, a blank or a
  duplicate) and move re-derive `/PageLabels` from the pages' old labels before
  the page tree is rebuilt, so an explicit range follows its pages (the insert
  splits it and writes the continuation with the old number), inserted pages
  carry the labels of their part (or the physical number when the source is
  unlabelled), and an unlabelled page keeps showing its physical number. The
  plan is checked before the pages move, so a label that cannot be written
  refuses the whole edit. `split` and `delete` already move ranges with their
  pages (`drop_deleted_pages`, which now also merges continuations, so undoing
  an insert restores the original tree). `replace` keeps the page object and
  its label. No behaviour changed outside the room apart from the fix itself:
  an insert or a move now keeps labels on their pages in single-user mode too.
- **Engine surface**: `Edit::ClearPageLabels { start }` (the engine had only
  `Edit::NumberPages`), and `Document::page_label_ranges()` reads the working
  file strictly for the adapter.
- **The client projects both ways**: `ops_for_edit` turns `Edit::NumberPages`
  into one `page.label.set` (an empty span and a prefix over the bound are
  refused before sending) and `Edit::ClearPageLabels` into `page.label.clear`;
  `edit_for` turns the accepted operations back into those two edits;
  `inverse_for` returns `None` for both and `inverse_after` builds the undo from
  `before` (a plain set for a clear, the range list for a split set). The
  existing "Number pages" dialog therefore works in a room; a dialog span that
  would cross several existing ranges is refused by the room, since its one
  undo image is one range.

Tests:

| Crate | Test |
|---|---|
| `organize` | `page_operations_keep_each_label_on_its_page` (blank insert splits a range and later pages keep their numbers; delete restores the ranges; duplicate carries the copied page's label; move keeps every label; clearing a range hands its pages back and the set over it is the undo; a page inside a range cannot be cleared), `inserted_pages_bring_their_labels` (a labelled source keeps B-A/B-B around physical numbers, an unlabelled source adds none), `a_label_span_is_checked_before_it_is_written` (blank label, long prefix, empty span, number overflow, valid ranges) |
| `collab-pdf` | `page_labels_are_set_and_cleared_as_operations` (front matter, an appendix that splits the continuation with 5/6 kept, the undo image of each, the split undo naming both ranges and a stale one refused, clear and its undo, clearing the last ranges), `a_bad_label_operation_changes_nothing` (9 refusals — past the end, empty, unknown style, zero first, blank, long, crossing, clear inside, clear missing — each with version, epoch and labels unchanged; viewer and commenter `forbidden`), `page_operations_keep_the_labels_on_their_pages` (insert a copy inside a range, split the inserted page) |
| `collab-client-pdf` | `page_label_edits_become_addressed_operations` (Number pages → one set with the target and span, clear → clear, empty span and long prefix refused), `accepted_label_operations_become_edits` (edit_for both ways, replication of a split set, `inverse_after` naming the two left ranges and writing the old range back, clear's undo as a plain set) |
| `collab-ws` e2e (socket) | `page_labels_are_operations_over_a_socket` (two editors and a viewer; front matter and an appendix converge on all three replicas; a stale undo is `conflict` without a version move; the two undos restore the labels and a redo applies again; clear and its undo; a crossing set is `conflict`; a viewer is `forbidden`) |

Gates at `c26867e9c` (host `agents`, rustc 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, the crates this pass touched | `cargo clippy -p pdfcraft-organize -p pdfcraft-engine -p pdfcraft-collab-{proto,pdf,client-pdf,server,ws} --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, the gate list | `cargo test -p pdfcraft-collab-server -p pdfcraft-collab-ws -p pdfcraft-collab-pdf` | pass — pdf 59, server 33, ws 16 unit + 32 e2e; 0 failed, 0 ignored |
| Tests, the rest of the collab set | `cargo test -p pdfcraft-collab-{proto,outline,hub,client,client-pdf,session,annot}` | pass — proto 15, outline 6, hub 5, client 14, client-pdf 42, session 5, annot 23; 0 failed |
| Tests, the crates whose labels moved | `cargo test -p pdfcraft-organize -p pdfcraft-engine` | pass — organize 228 + 2 integration, engine 636 + 9 + 46; 2 ignored (pre-existing) |
| Daemon | `cargo test -p pdfcraft-collab-daemon` | pass — 34 + `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1 + `restore_publication` 2 |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Workspace | `cargo check --workspace --all-targets` | pass (pre-existing warnings outside this pass's files) |
| UI, the collab module | `cargo test -p pdfcraft-ui-egui --features collab --lib collab::` | pass — 16 |
| Layers | `cargo xtask layers` | OK — 51 crates, no violations |
| Browser | `cargo xtask wasm` | 43 crates ok |

What this does **not** cover: links (`/Annots` link annotations and remote
go-tos) are still not shared — the rest of TASK §3's list. A "Number pages"
span that would cross several existing ranges is refused (`conflict`), not
split into several operations; the dialog still labels one range at a time. The
operation writes whole ranges, not "this range plus the numbers after it": a
set's continuation keeps the old numbering, so the dialog's usual
"relabel these pages, keep the rest" works, but moving a boundary is two
operations. Nothing was pushed: PR #6622 keeps the fork's head `b27fa89c8`.

### The outline and its named destinations — §5 p.3 (2026-11-10, local; not pushed)

`d1229deb9` on `0ab350c6b`. The room had no operation for a bookmark or a named
destination: the panel's edits were refused as `unsupported` in a session, and a
commit could not carry them. They are now eight structural operations (RFC
§3.1.7), and the address is the file's own identity rather than a position.

- **A bookmark is addressed by its object reference** (`OutlineKey { num,
  generation }`, `Target::OutlineItem { key }`, `None` = the outline itself as a
  parent). A path (`[0, 2]`) is a position and moves as soon as an item above it
  is added or removed; the object is what the file keeps, and what a destination
  array names. The adapter re-derives key→path from its document on every
  operation (`Session::bookmark_page`, every item with its object), so an
  operation built before a page operation still finds its item afterwards. A key
  that names nothing is `invalid` — never a guess by position.
- **A named destination is addressed by its name** (`Target::NamedDest { name }`),
  the key the PDF uses in `/Names /Dests`. `pdfcraft-organize` gained the read
  (`named_destinations`, `named_destination`) and the write: `set_named_destination`
  and `delete_named_destination` rewrite `/Names /Dests` as one leaf with its
  keys in ascending byte order, take the name out of the legacy `/Dests`
  dictionary so one name means one place, and leave every other tree in `/Names`
  alone. `MAX_NAMED_DESTINATIONS` (10 000) bounds the list.
- **The destination of a bookmark or a name** is `{"kind":"page", page, left,
  top}` (`/XYZ`), `{"kind":"view", page, view:{kind, values}}` for any other
  Table 149 type, `{"kind":"named", name}` for a bookmark, or
  `{"kind":"nowhere"}` for a container with no `/Dest`. The `view` form is what
  makes an undo exact: a `Fit`/`FitR` destination comes back as it was instead of
  being flattened to `XYZ`. Engine: `BookmarkDest` with those four forms,
  `Edit::AddBookmarkWith`, `Edit::UpdateBookmark` (title and/or destination in
  one step), `Edit::SetNamedDestination`, `Edit::DeleteNamedDestination`;
  `pdfcraft-organize` gained `add_bookmark_with`, `set_bookmark_dest`,
  `dest_object` and `view_at`. `BookmarkRow` now carries the item's object
  (`object: Option<(u32, u16)>`) and `Document` gained `bookmarks()`, so a
  replica can map a path to the key it must send.
- **`outline.item.delete` carries the whole subtree** — `OutlineNode`s with
  titles, pages, views, colours, styles, open state and nesting — plus the key of
  the item, its parent's key and its index. The copy travels in the accepted
  operation (like `page.split`'s `bytes`), so no server-side copy store was
  added, replay rebuilds nothing, and the undo is an ordinary
  `outline.item.restore` through `Edit::ImportBookmarks`. `limit` before the
  first write when the subtree is over 2 000 nodes, over 128 KiB of JSON, or
  deeper than the listing can read (`truncated`); the byte bound is checked
  again on a restore, because the accepted operation is broadcast and journaled
  and has to fit the frame on the way out as well. A restored item gets a new
  object reference (PDF has no way to choose one), and the result names it, so a
  redo addresses the new key — the same shape `page.restore` has.
- **`outline.item.move`** takes the new parent's key (`null` = top level) and an
  index, refuses a move into itself or its own subtree (`invalid`) and one that
  would nest deeper than 32 levels (`limit`), and the accepted operation carries
  `from`, `to_parent`, `index` and `before {parent, index, path}`.
- **Refusals happen before the first write**: an unknown parent or item, a blank
  or over-long title (512 bytes), a page past the last, a name this document does
  not define, a blank name, a cycle, depth, count (10 000 items), copy bounds,
  `dest.add` of a taken name and `dest.update`/`dest.delete` of a missing one
  (`conflict`), `dest.update`/`dest.delete` of a name that opens no page (a name
  of a name — `unsupported`, since the room keeps a page per name), and `dest.*`
  carrying a passed `epoch` (`stale`, retryable). None
  of them moves `version`, `epoch`, the document, the epoch counter or the
  journal. `outline.item.update` with neither title nor destination is `invalid`.
- **Roles.** All eight are `structural`, so the room refuses a `viewer` and a
  `commenter` before the adapter runs (`forbidden`, no version hint) and the
  client shows ReadOnly: `collab_apply_edit` lists every bookmark and named
  destination edit as structural, so a commenter never even builds the
  operation.
- **Page operations keep the targets.** Insert, split, combine and replace hold
  page objects, so a bookmark or a name keeps its page and only the number moves;
  a bookmark whose page is deleted loses `/Dest` and keeps its place, and a name
  on a deleted page is pruned — the engine's existing behaviour — and the undo
  of the delete puts both back. A key taken before the page operation still
  addresses its item afterwards (the server recomputed the path).
- **The client projects both ways.** `ops_for_edit` turns
  `AddBookmark`/`AddBookmarkWith`, `RenameBookmark`, `SetBookmarkPage`,
  `UpdateBookmark`, `DeleteBookmark`, `MoveBookmark`, `SetNamedDestination` and
  `DeleteNamedDestination` into operations (a name the replica already has is
  `dest.update`, one it does not is `dest.add`; `ImportBookmarks` with `replace`
  is `unsupported`); `edit_for` turns every accepted operation back into the
  edit; `inverse_for` returns `None` for all of them and `inverse_after` builds
  the inverse from the accepted operation (`add`→`delete`, `update`→`update`
  with `before`, `delete`→`restore`, `restore`→`delete` of the new key,
  `move`→`move` back, and the three `dest.*`).
- **A new leaf crate** `crates/collab-outline` (package
  `pdfcraft-collab-outline`) holds the wire form of a destination and of a
  subtree copy, with every bound: titles, names, depth, node count, copy size,
  views against Table 149, a `/FitR` that lost a value. It compiles for the
  browser (`collab-annot`'s sibling; no socket, no clock).

Tests:

| Crate | Test |
|---|---|
| `collab-outline` | `every_destination_round_trips`, `a_destination_that_is_not_a_place_is_refused` (no page, negative, `1e400`-class, blank name, unknown kind, unknown view, `FitR` with three values or a hole), `titles_and_names_are_bounded`, `a_subtree_round_trips_whole` (nesting, views, colours, styles, open state; node count and depth), `a_copy_that_breaks_a_bound_is_refused` (title, depth, node count, a colour past 1, children that are not a list, a `Fit` with a value), `an_empty_copy_is_an_empty_list` |
| `organize` | `named_destinations_are_set_listed_moved_and_deleted` (set, read back through the same lookup a reader uses, update in place, order, blank name / missing page / non-finite coordinate refused, delete, a missing name refused, the empty slot removed), `legacy_dests_and_the_other_name_trees_survive_an_edit` (a `/Dests` entry listed, taken over by the tree, the `JavaScript` tree untouched, a legacy-only name deleted), `a_bookmark_goes_to_a_named_destination_and_back` (named / nowhere / view destinations, refusals, `set_bookmark_dest` both ways, a bad `FitR` refused) |
| `engine` | `named_destinations_and_the_shared_outline_edits` (a name, a bookmark to it, an undefined name refused, title+destination in one step and its undo, a container, deleting the name leaves the bookmark going nowhere and its undo brings the place back, `bookmarks()` carries the object, both survive a save) |
| `collab-pdf` | `the_outline_is_added_updated_moved_and_deleted` (add under an addressed parent, `before` of the update, move with `from`/`before`, delete with the whole subtree, restore at its index, adding to the restored parent by its new key), `a_bad_outline_operation_changes_nothing` (13 refusals: unknown parent/item, missing page and name, blank and over-long title, update with nothing to change, delete of nothing, cycle into a child and into itself, a 40-deep copy, an over-long title in a copy, a 300-item copy over the byte budget — each leaves version, epoch and document as they were), `page_operations_keep_the_outline_targets_and_the_keys` (blank insert, a key from before it, a split that drops two pages, a combine that brings its own bookmark, a replacement; the name keeps its page), `named_destinations_are_operations` (add, duplicate `conflict`, update with `before`, delete with `before`, and five refusals), `a_viewer_cannot_change_the_outline` (viewer and commenter, `forbidden`, nothing moves) |
| `collab-client-pdf` | `bookmark_edits_become_addressed_operations` (each UI edit → one structural op addressed by the parent's or the item's key, a path the replica does not have, `dest.add` vs `dest.update`, `replace` unsupported), `accepted_outline_operations_become_edits` (add/update/move/restore/dest through `edit_for`, a named destination that is a name refused) |
| `collab-ws` e2e (socket) | `two_clients_share_the_outline` (two editors and a viewer; add, update with a named destination, move; all three replicas equal), `a_deleted_subtree_comes_back_whole_over_a_socket` (delete, `inverse_after` → restore, `inverse_after` → redo, a stale key `invalid` with no version move, a viewer `forbidden`), `named_destinations_are_operations_over_a_socket` (add, update, delete, a viewer refused, a bookmark that named it going nowhere) |

Gates at `d1229deb9` (host `agents`, rustc 1.99.1, `-j 12`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass |
| Clippy, collab crates and the crates this pass touched | `cargo clippy -p pdfcraft-organize -p pdfcraft-engine -p pdfcraft-collab-{proto,outline,pdf,client-pdf,server,ws} --all-targets -- -D warnings` | pass |
| Clippy, UI with the feature | `cargo clippy -p pdfcraft-ui-egui --features collab --all-targets -- -D warnings` | pass |
| Tests, the gate list | `cargo test -p pdfcraft-collab-server -p pdfcraft-collab-ws -p pdfcraft-collab-pdf` | pass — server 33, ws 16 unit + 31 e2e, pdf 56; 0 failed, 0 ignored |
| Tests, the rest of the collab set | `cargo test -p pdfcraft-collab-{proto,outline,hub,client,client-pdf,session,annot}` | pass — proto 15, outline 6, hub 5, client 14, client-pdf 40, session 5, annot 23; 0 failed |
| Tests, the engine the edits live in | `cargo test -p pdfcraft-organize -p pdfcraft-engine` | pass — organize 225 + 2 integration, engine 636 + 9 + 46; 2 ignored (pre-existing) |
| Daemon | `cargo test -p pdfcraft-collab-daemon` | pass — 34 + `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1 + `restore_publication` 2 |
| App wiring | `cargo check -p pdfcraft --features collab` | pass |
| Workspace | `cargo check --workspace --all-targets` | pass (two pre-existing warnings outside this pass's files) |
| UI, the collab module | `cargo test -p pdfcraft-ui-egui --features collab --lib collab::` | pass — 16 |

What this does **not** cover: links (`/Annots` link annotations and remote
go-tos) and page labels are still not shared — this pass is the outline and
named destinations, as briefed. The app has no named-destination editor of its
own, so the three `dest.*` operations have no interface command and no
`crates/automation` tool: they are protocol plumbing so a document's names
survive a session and a commit, and the `Edit`s they use are engine-level. A
bookmark inside a delete's copy that went to a *named* destination comes back
going to that name's page (the copy is a tree of pages), and a restored item
gets a new object key. `SetBookmarkColor`, `SetBookmarkStyle`,
`SetBookmarkOpen`, `BookmarksFromStructure` and `BookmarksFromTextStyles` are
still `unsupported` in a room. Nothing was pushed: PR #6622 keeps the fork's
head `b27fa89c8`, and this pass's two commits are local.

### Publications that outlive the process — §5 p.6 / limitation 6 (2026-11-10, local; not pushed)

`21aaf357a` on `dc773c609`. Limitation 6 had two open ends: the publication
queue lived only in memory, so a restart fell back to the next commit, join or
leave; and `POST /v1/sessions/{fileId}/resolve` re-armed the room without
publishing the base the cabinet had refused, so a room that then went quiet
waited forever. Both are closed with no new wire verb. The publication *is* the
committed base, its payload is fixed, and its idempotency key is derived from
that payload, so a later process can send exactly what the dead one would have
sent.

- **A restart finds the unpublished base in the store.** `Store` already keeps
  `CURRENT` (the base the room loads, its version and epoch) and `CABINET` (the
  base the cabinet took, with both numberings). A room is unpublished when the
  first is ahead of the second — a commit wrote its base and the POST either
  failed retryably or was in flight when the process went away.
  `rooms::restore_publications` walks `<data>/`, reads only `CURRENT` and
  `CABINET` per room, and opens the unpublished ones from the base plus the
  journal, exactly as `POST /v1/sessions` would, so a client that connects
  afterwards sees the same head. One directory that cannot be opened is skipped
  (its session request reports it) instead of stopping the daemon. The journal
  tail beyond that base is not this path's work: it becomes a base at the next
  commit.
- **The base, not a new commit.** `Adapter::pending_publication` (default
  `None`; `CabinetAdapter` implements it) builds the POST of the committed base:
  `fileId`, the version `CURRENT` names, the bytes read back from
  `versions/<N>/file.pdf` through `PdfAdapter::committed_base`, and
  `expectedVersion` from the marker (the cabinet's numbering, which a resolve
  may have moved). `Room::republish` takes it and makes the room `publishing`,
  so a client commit in the meantime is `busy` rather than a newer base
  overtaking the restored one. `Room::finish_retry` records the outcome against
  `base_version`, the version that was written — the same path a queued retry
  takes.
- **The same key, so one version.** The publication carries
  `fileId:version:sha256` of that base in the body (`idempotencyKey`) and in the
  `Idempotency-Key` header. A cabinet that took the bytes before the process
  died recognises the duplicate instead of creating a second version; the next
  commit's `expectedVersion` proves the marker moved.
- **Queued at once, not after a backoff.** `Shared::queue_restored` puts a
  restored publication due *now* (no attempt of this process failed yet);
  `retry_publications` runs it on the next 100 ms tick. The daemon seeds the
  queue before it binds the WebSocket listener, so nothing can overtake it.
  `publication_retries_total` counts only publications that failed at least
  once, so a restored first attempt is not reported as a retry.
- **The resolve publishes.** `WsServer::resolve_conflict` moves the marker,
  clears the flag, and then calls `Room::republish` and queues the result — no
  commit, join or leave is needed. A resolve with nothing to publish (the marker
  already names the base) queues nothing; a base that cannot be read back leaves
  the resolve standing and the next commit publishes, which is what happened
  before this change. The refresh is counted separately from the resolution.
- **The tail follows, in order.** After a successful restored publication the
  queue applies its existing rule: the room is dirty if it also has a journal
  tail beyond that base, nobody is connected, so it commits the tail and
  publishes it as the next version — the same thing the last-leave commit would
  have done had the process lived. The base still goes first (a newer base never
  overtakes it), and each publication carries its own derived key.
- **A `409` still stops the queue.** A restored or resolve-queued publication
  goes through `Room::finish_retry` like any other: a `409` flags the room,
  releases it and drops the entry (retrying the same `expectedVersion` cannot
  succeed); a retryable failure keeps the room `publishing` and schedules the
  next attempt with the same body and key; a `retryable: false` failure is
  dropped. The `conflict` flag itself is still in-process — a restart
  re-discovers it by publishing, which is exactly the state that produces it.
- `/metrics` gains
  `pdfcraft_collab_publications_restored_total` and
  `pdfcraft_collab_publications_after_resolve_total`
  (`Metrics::publications_restored`, `::publications_after_resolve`); `/health`
  is unchanged. The start-up line names the restored count.

Tests:

| Crate | Test |
|---|---|
| `collab-server` | `a_restored_base_is_published_without_a_commit_and_blocks_newer_ones` (resume at 3 with the cabinet at 1: `republish` returns one attempt, `commit_now`/`republish` are `busy`, `finish_retry` makes the room clean and leaves nothing to publish); `a_restored_base_the_cabinet_refuses_flags_the_room_and_stops` (409 → conflict, not `publishing`, still dirty); `a_restored_base_that_failed_retryably_keeps_the_room_publishing`; `an_adapter_with_nothing_unpublished_republishes_nothing` |
| `collab-ws` | unit `a_restored_publication_is_queued_at_once_and_counted` (queued, not run under the lock; one attempt on the next tick; `publications_restored` 1, `publication_retries` 0; a success is not queued again) and `a_restored_base_is_followed_by_the_journal_tail` (a room resumed at base 1 with an acknowledged operation after it and nobody connected: the base is published first, then the tail as its own commit, and the room ends clean at version 2) |
| daemon | `tests/restore_publication.rs` `a_restart_publishes_the_base_the_previous_process_left`: the real daemon with `--control` (no `--documents`), a cabinet that is down for the first commit, `SIGKILL`; the second process publishes `version 1` with no `expectedVersion` (the session's first commit) and the derived key **with no client connected**, `publications_restored_total 1`, `publication_retries_total 0`, one delivery (not repeated after 1.5 s), and the next commit names `sha256:<…1>` (`version 2`). `a_restored_publication_is_the_first_commit_and_the_marker_advances`: the restored POST carries no `expectedVersion`, the cabinet stores it as its next version (6, from 5), the room adopts the etag it answered, the metrics count it and no room is flagged |
| daemon | `tests/conflict_resolve.rs` now asserts the new loop: a `409` is not queued (no delivery and no retry for 1.5 s), the resolve publishes the refused base once with the same derived key, and the next commits expect the cabinet's answer (8, then 9) |

Gates on the host `agents` (rustc 1.99.1, `-j 8`):

| Gate | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | pass, EXIT 0 |
| Clippy, collab crates + daemon | `cargo clippy -p pdfcraft-collab-server -p pdfcraft-collab-ws --all-targets -- -D warnings` | pass, EXIT 0 (also `-p pdfcraft-collab-daemon -p pdfcraft-collab-pdf`) |
| Tests, protocol crates | `cargo test -p pdfcraft-collab-server` | 33 passed, 0 failed, 0 ignored, EXIT 0 |
| Tests, transport e2e | `cargo test -p pdfcraft-collab-ws --test e2e` | 28 passed, 0 failed, 0 ignored, EXIT 0 |
| Tests, transport unit + pdf | `cargo test -p pdfcraft-collab-ws -p pdfcraft-collab-pdf` | 16 + 51 passed, 0 failed, 0 ignored, EXIT 0 |
| Tests, daemon | `cargo test -p pdfcraft-collab-daemon` | 34 unit + `conflict_resolve` 1 + `crash_recovery` 1 + `publication_retry` 1 + `restore_publication` 2, 0 failed, 0 ignored, EXIT 0 |

What this does **not** cover: the start-up recovery runs only for a room whose
base the cabinet does not have — a room that is clean on disk and whose journal
tail already became a published base is not re-published (there is nothing to
send); `publishing` and `conflict` are still in-process flags, so a restart
recomputes them from `CURRENT`/`CABINET` and, where the base is unpublished, by
publishing; and `https` without a reverse proxy is still open (limitation 7).

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
operation is the copy). Layers, attachments, form structure, links, page labels
and content edits are still not shared; bookmarks and named destinations are
shared since `d1229deb9` (§4, RFC §3.1.7).

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
| daemon rooms | `an_operator_resolution_lets_the_next_commits_publish_again`: a cabinet holding version 7 refuses two commits (both with no `expectedVersion`, queue stopped); resolve →7 (`etag = sha256:<7>`); the next commit sends `version 2` with that etag and the derived key, the cabinet answers `{version: 8}`, and the commit after that sends `sha256:<8>` |
| daemon control | `an_operator_resolves_a_conflict_and_a_second_call_is_refused`: conflict through a real callback `409`; 401/403 (another file, viewer)/400 (bad etag, version 0)/404; then a 200 with `version 1` (unmoved), `cabinetVersion 4`, `conflict:false`; metrics show no conflict and `conflicts_resolved 1`; a second resolve is `409 not_conflict` and is not counted |
| daemon e2e (`tests/conflict_resolve.rs`) | `a_conflict_is_resolved_once_and_the_resolve_publishes_the_base`: the real daemon in dev mode against a fake cabinet holding version 7 — the commit is refused (no `expectedVersion`), `/health` and `/metrics` flag the room, a second commit is refused with the same absent expectation, no token is `401`, resolve is accepted, `/health`/`/metrics` clear and count 1, a second resolve is `409`, the next commit reaches the cabinet with the etag the resolve named and the derived idempotency key, the cabinet stores one version (8), and the commit after that names the copy again (in dev mode the source file's etag) |

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

What this section still said was open — a resolve that only lets the *next*
commit publish — is closed in `21aaf357a` (§4): the resolve publishes the refused
base itself, and a restart republishes it from the store.

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

What this does **not** cover (the first three are closed in the next section,
`5bad54c40`): several selected comments share nothing (only the
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
selection, not the caret inside a form field being typed into (closed later in
`c935ebadb`, §4 above); only comments on the
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
| daemon | `tests/publication_retry.rs` `a_lost_callback_answer_is_retried_with_the_same_key_and_one_version`: the real daemon in dev mode with `--callback-url`; the fake cabinet **takes** the first delivery and closes without an answer (the classic lost response), sees the retry as a duplicate of the same key and answers `2xx`. Asserts two deliveries, same `version`/`sha256`/`idempotencyKey` in both and no `expectedVersion` (the session's first commit), one stored version, `pdfcraft_collab_publication_retries_total 1`, no `conflict`, and that the next commit sends `version 2` with an opaque `sha256:…` etag — the marker advanced without a second version. |
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
- `craft-web-host` is pinned to `7666e2b1` (the commit with the collaboration
  messages) for that crate only — ADR-0016 §3; since the CI gate fix it is
  fetched from `storytold/craft-libs` (PR #716's head), not the fork (§4, "The
  CI git-source gate"), and `craft-web-api` stays on `storytold/craft-libs`
  `fd5e2fe`. Feature `collab` in
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

### The browser bridge in the cabinet: D4 closed (2026-11-11)

§6.3 built the bridge; no run had ever used it. The cabinet's browser acceptance
found that the PDF page never dialled the room (D4 in `docs/CRAFT-ACCEPTANCE.md`
of the cabinet repository). Four defects, all fixed here, and the browser run
that proves the chain end to end.

1. **The page was built without the feature** (`5cb08cb3d`). `apps/pdfcraft-web`
   carries the five messages only under `collab`, and trunk built the page
   without it: the deployed wasm at the pin `4f3e21245` had no collaboration
   code at all (`collaboration session`, `Connecting…`, `Committed version`
   absent; the bare `session-state` string it does hold comes from
   `craft-web-host`, which joins the app id and the kind at run time). Saying
   `--features collab` on the trunk command line cannot be the answer: trunk
   applies it to every rust asset, and the OCR worker has no such feature, so
   the build fails. `index.html` now sets `data-cargo-features="collab"` on the
   app's own asset. The crate feature stays off by default: `cargo build` and
   `cargo test` are single-user, and so is a page that never receives
   `pdfcraft:session`.
2. **The browser transport read a queue nothing filled** (`f80fea041`). The
   socket's callbacks pushed into `WebTransport::queue`, while `read` drained
   `Inner::incoming`, which nothing ever wrote. The socket opened, the hello went
   out, the room answered `welcome`, and the driver waited forever: the status
   stayed `Connecting…` with no error. One queue now.
3. **The open event after the greeting sent a second hello** (`55859443c`). The
   page's socket is open before the first step, so the hello is written at once;
   the `Opened` the callback queued was read afterwards and taken for a new
   connection. The room answered `this connection is already joined`, which the
   app showed under a joined status. A live, already-greeted socket ignores the
   trace of its own open. The new test
   `an_open_reported_after_the_greeting_is_not_a_second_hello` fails without the
   guard (two hellos).
4. **An annotation made in the page panicked the app.** `new_annotation_name`
   (crates/collab-client-pdf) read the clock with `std::time::SystemTime::now`,
   which panics on wasm32 ("time not implemented on this platform"). The first
   highlight or sticky note took the whole page down. It uses `web_time` now, as
   the rest of the wasm-reachable code does. (The change was swept into the
   concurrent merge `46d2e203a`, which brought main's v0.21.0 release in; it is
   recorded here because it belongs to this run.)
5. **The room's commit publication spoke a different contract than the cabinet**
   (`19cf676b1`). The room sent `expectedVersion` as a JSON number, and `0` for
   the first commit. The cabinet's callback contract (INTEGRATION.md §5.2, the
   one DeckCraft's room speaks) names the **etag** of the copy the cabinet holds,
   a string, absent for the first commit: the number was refused by the
   cabinet's decoder (`400`), so every commit that reached the callback was lost.
   The room now sends the etag it adopted from the cabinet's answer, and sends
   nothing until one exists.

Gates: `cargo test -p pdfcraft-collab-daemon` — 34 unit + 5 integration tests,
including the new `an_open_reported_after_the_greeting_is_not_a_second_hello`
(fails without the guard) and the publication tests rewritten around opaque
etags; `cargo test -p pdfcraft-collab-client-pdf` 48; `cargo check --target
wasm32-unknown-unknown -p pdfcraft-web` and `-p pdfcraft-collab-session` pass.
The web bundle is built and checked by the cabinet's
`scripts/craft-build-pdfcraft-web.sh`, which refuses a wasm without the bridge
markers.

The browser run (cabinet `https://mail.lzmkm.ru`, disk file 43
`acceptance.pdf`, owner/editor/viewer accounts, headless Chrome):

| What | Observed |
|---|---|
| Static bundle | `0.21.0-collab.46d2e203abbd`, wasm `sha256:4457c37d…77e36`, `200` from `/office-assets/pdfcraft/…` |
| Socket | `wss://mail.lzmkm.ru/office-sync/pdfcraft/ws`, `hello` → `welcome` (`owner`/`editor`/`viewer`) |
| Bridge messages | `pdfcraft:ready`, `session-state`, `presence`, `dirty`, `commit` |
| Status | `Connected as craft-e2e-owner@lzmkm.ru · owner` (and the editor's and viewer's own) |
| Presence | both panes `Участников: 1` (the other one); the app's Collaboration window lists `craft-e2e-editor@lzmkm.ru · editor · page 1` on the owner's side and the owner on the editor's |
| An edit | a highlight drawn on the owner's page: 1 `op` out, 1 `applied` in on the editor |
| Commit | the Collaboration window's "Commit to the cabinet" → `pdfcraft:commit` → the cabinet's D3 route → the room's callback → a stored version; the file's content grew from 591 to 2949 bytes and the stored document contains the `/Highlight` annotation and the owner's name |
| Viewer | joins as `viewer`; the highlight tool produces no `op` (only presence frames leave) |

What this does **not** establish, and what to look at next:

- The host → app leg `committed` was not observed: the version is stored, but
  the app's status stayed `Commit requested from the cabinet` and no
  `craft-version` marker appeared. The cabinet posts
  `{version, etag}` after the D3 commit; `craft-web-host` parses a whole number
  and a string, which that is. This is the first thing to instrument.
- The room's control plane answers `409` to a new ticket when the room's
  `CABINET` marker and the cabinet's file etag disagree (observed after a
  commit loop clicked the button more than once). The recovery used here was to
  move the room's state directory aside and let the room reseed from the file;
  the daemon should probably treat "the cabinet's etag changed after our own
  commit" as its own publication, not as an external change.
- The sticky note's editor has its own Publish button in the page (a canvas
  target); the acceptance marks text with the highlighter instead. A note made
  through the tool was not part of this run.
- The room binary on the cabinet is now `19cf676b1` (built from this branch,
  v0.21.0); the room's docker image field in `CRAFT_APPS` is inert since M92.
- **A hash note on `19cf676b1`.** It is the parallel pass's first commit of this
  fix (parent `28fd4e1b3`, same subject as `0d3ea2708`) and it is **not an
  ancestor of HEAD**: the delivered form is `0d3ea2708`, the commit in this
  branch and the one the gate in §4 ran on (§4, "The cabinet's `expectedVersion`
  is its etag"). A rebuild of the room should be taken from the delivered head;
  whether the binary currently on the cabinet was built from the final commit is
  a cabinet-side check this pass cannot make from here.

### Both remainders of that run closed: the room-side commit and its etag (2026-11-12)

The browser run above left two things undone, and the cabinet's M95 pass closed
both. They were one defect seen from two sides: the cabinet never asked the room
for the file, and the ticket carried the wrong kind of etag.

The two defects and the fix they became are recorded above ("The cabinet's Save
reaches the room, and the etag compares like with like"): the control plane's
`POST /v1/sessions/{fileId}/commit` and its answers, the ticket's
`{etag, version}` compared with the marker instead of a digest of the bytes, the
`CABINET` digest that lets a moved tag be adopted, and `WsServer::commit_now` as
the seam. What follows is what the cabinet-side pass saw.

Gates on `fee60000b` (8 files, +571/−45): the full sweep is in that section
above — `CI=1 cargo xtask ci` exits 0, all ten steps, **10380 passed, 0 failed,
22 ignored**. The touched crates were also run alone: collab-daemon 39 (5 new:
the commit route with a ticket, a clean repeat that publishes nothing, a viewer
and another file's ticket, a refused publication, and the double commit whose
answer was lost), collab-ws 16 + e2e 36, collab-pdf 71, collab-server 33,
collab-hub 5, collab-client-pdf 48, all passing.

The room binary built from it is `68df6a7b…63238` (glibc 2.38, v0.21.1) and it
is what `craft-sync-pdfcraft` runs on the cabinet since 2026-11-12 18:51 MSK.

**Browser run on the cabinet (2026-11-12, file 43 `acceptance.pdf`, owner and
editor through SSO, headless Chrome).** Recorded in the cabinet's
`docs/CRAFT-ACCEPTANCE.md` §11 with screenshots:

| What | Observed |
|---|---|
| An edit and the app's Commit | `POST /api/v1/files/43/craft/commit` `200`; the cabinet's bar `Версия: 1794499249`; the app's status `Committed version 1794499249`; the page received `pdfcraft:committed {version:1794499249, etag:"1794499249"}` |
| A second Commit | `200` (a new version `1794499397` after the room's next build, and no `409`) |
| A rapid double click | two commits, both `200` |
| A foreign change of the file (an ordinary replace of its bytes) | the next ticket `409 conflict` |
| The original bytes restored | ticket `200`: the tag moved to our own published bytes and the room adopted it |

**What is still open (new, and not this fix):** a page loaded *after* a commit,
while the room's journal is empty because the commit compacted it, is answered
with a snapshot that carries a **base reference** (`{"type":"snapshot","mode":
"base","base":{"sha256":…}}`), and the web client cannot take a base: it says
"The server compacted the journal; reopen the document to catch up." The base
delivery the DeckCraft client has (`GET /v1/sessions/{id}/base/{sha256}` +
`collab-client-pdf` fetching it) is what PdfCraft needs next; until then a
reload right after a commit asks the user to reopen the document. The same
message appears in a clean run whenever the join has to start from a compacted
base.

### A closed socket no longer takes the seat of a reader that still has one (2026-11-12)

The cabinet's WebKit run of the same day reported a state the Chromium run never
reached: the owner's page said `Not allowed: not in this session`, the room's
counters said the editor was in the room, and the ticket and the socket were
fine. It is a room defect, and `24f519a` fixes it.

`Room` registers a participant by its actor — one seat per person, not per
connection — but `Hub::disconnect` removed that seat, the role and the writing
lease for **every** closed connection. The page opens a new socket on a ticket
renewal and again after a reconnect, so the room can see the new `hello` before
the old socket's close reaches it; the actor then had a live connection and no
seat, and every operation, presence frame and resync from it was refused with
`Forbidden: not in this session` while the interface looked connected. WebKit
made the window easy to hit (slower wasm start, a second `hello` per page), a
Chromium tab that is reopened or closed reaches it too.

The rule is now the obvious one: only the last connection of an actor leaving
gives up the seat, the lease and the operation window. The test
`a_closed_connection_does_not_take_the_seat_of_the_same_actor` joins the same
actor twice, closes one connection and expects the other one's operation to be
accepted; on the old code it is refused with `Forbidden: not in this session`.

Reproduced and re-checked in both engines through the cabinet (file 43):

| Run | Before `24f519a` | After |
|---|---|---|
| WebKit, the same account in two tabs, one closed | `Connected … owner: Not allowed: not in this session`, every frame `{"type":"error","code":"forbidden","message":"not in this session"}` | `Connected … owner`, the highlight is applied |
| Chromium, the same two tabs | (the same rule; not measured before the fix) | `applied` with `version 4` |
| WebKit, owner + editor + viewer | — | all three `Connected`, `Участников: 1`/`2`, edits both ways, no `forbidden` |
| WebKit, edit and a double Commit | — | `Committed version 1794502272` in the bar and in the app, two `200`s |

The room binary built from `24f519a` is `bb42c7c5…fa691` and replaced
`68df6a7b…63238` on `craft-sync-pdfcraft` at 2026-11-12 19:39 MSK.

**A separate trap seen while measuring (not fixed here):** the app's own Save
(toolbar, `file.save`, Ctrl+S) still sends `pdfcraft:save` **with bytes**, which
the cabinet stores as an ordinary new version behind the room's back. The room's
marker then names the previous copy, and the next commit — and the next ticket —
answer `409`, until the operator re-seeds the room or resolves it. In a
live session the Save should go through `collab_commit` (the room owns the
document). One version of the deployed file 43 (8504 B, 16:05Z) came from exactly
that path.

### Save in a hosted session is the cabinet's commit (2026-11-12)

The same cabinet pass found the last way a browser session could lock itself: the
app's own Save (toolbar, menu, Ctrl+S) ran `file.save`, which sends the bytes of
this window's build through the host page. The cabinet stored them as an ordinary
version behind the room, so the room's marker kept naming the previous copy and
the next commit — and the next ticket — answered `409 the file changed outside
the session`. It happened twice on the file 43 of the acceptance (16:05Z and
19:05Z), and each time the room had to be reseeded by hand.

`f107f56c2` routes Save to `collab_commit` while a session is live **and** a host
page owns the commit; `file.save` and the pages' own Save action both take that
path. Single-user mode and the desktop are untouched: there the room is a plain
sync server and Save stays the local file.

Tests: `save_in_a_hosted_session_asks_the_cabinet_to_commit` counts the host
commit and asserts that nothing appears at the `save_override` path — on the old
code it fails with zero commits and the bytes written;
`save_without_a_hosted_session_writes_the_file` keeps the ordinary save for a
session nobody commits to. Gates: fmt, clippy `-D warnings` with `collab`, wasm32
`pdfcraft-web` with and without the feature, and `cargo test -p
pdfcraft-ui-egui --features collab` (lib 1035, `it` 1898 with the pinned
craft-i18n `575cfdef` and craft-fonts; without them the two documented
environment tests fail, unchanged by this diff).

The bundle `0.21.1-collab.f107f56c2fe6` is what the cabinet serves since
2026-11-12 20:41 MSK. Two Ctrl+S presses on file 43, in Chromium and in WebKit,
each produced `POST /files/43/craft/commit` `200` and a version
(1794506204 → 1794506220 and 1794506160 → 1794506166), with no `PUT
/files/43/content` at all; the room's marker then named exactly the stored file
(`etag 1794506220`, room version 12, `sha256 a18329c3…` = the downloaded bytes)
and both tickets answered `200`.

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
   ~~Bookmarks~~ and named destinations are shared since `d1229deb9` (§4, RFC
   §3.1.7): an item is addressed by its object key and a name by its name, a
   delete carries its whole subtree for the undo, and every page operation keeps
   the target page because the destination names the page object. Page labels
   are shared since `c26867e9c` (§4, RFC §3.1.8): a range is addressed by the
   page it starts on, `page.label.set`/`page.label.clear` carry the whole range
   they replaced for the undo, and insert/duplicate/blank/combine/move keep
   every page's label on it. Links are shared since `2b61ff80f` (§4, RFC
   §3.1.9): the unit is one page's whole link list, `page.link.set` and
   `page.link.clear` carry the list before and after for the undo, a target is
   a page with a view, a named destination, a page of another file (remote
   go-to) or a web address, and a page operation keeps a link's page while it
   renumbers a destination written as a page number; a whole-document action is
   one `links.add_many`/`links.remove_many` over every page it touches, so
   `Edit::RemoveLinks` and `Edit::AddLinks` are shared too (§4, RFC §3.1.9).
   What a room still cannot do with links: a link whose action
   the model cannot carry (JavaScript, Launch) refuses its page whole; a link
   copied in from another file keeps that file's page numbers; and a link to a
   named destination is not rewritten when the name is deleted (its undo puts
   the name back). The bookmark's *colour*, *style* and *open* state are shared
   since this pass (§4, RFC §3.1.7): they are the same addressed
   `outline.item.update`/`add`, refused for a colour that is not three numbers
   from 0 to 1, a style that is not a boolean and an open state on an item with
   no children, and the undo image is the whole node. Named destinations have
   an editor of their own now (Bookmarks ▸ Named destinations…, §4) and headless
   tools (`destinations`, `destination_set`, `destination_delete`). The whole
   outline is replaced in one operation since `642dabb5b` (§4, RFC §3.1.7):
   `outline.replace` carries the complete tree (a node may name its destination
   by name, which is resolved to its page), the accepted operation carries the
   whole `before` for the undo, an empty tree clears the outline, and
   `ImportBookmarks` with `replace` is shared. The from-structure and
   from-text-styles generators build their tree in the replica and send that
   same operation, so they are shared too — a structure link (`/SE`) is not
   carried, and every outline over one operation's budget (2 000 nodes,
   128 KiB JSON) cannot be replaced, because its undo image would not fit.
   Layers, attachments, form structure and content edits are still not shared.
4. ~~**Presence has cursors, not selections.**~~ Closed on the client in
   `b4e9179d8` (§4): presence now carries the selected comment's `/NM` or a
   rectangle in user space (a text range's bounding box, a column select), under
   the cursor's 30 s TTL, drawn in the actor's colour for every participant
   including viewers, and it never becomes an operation or moves `version`.
   The rest of it is closed in `5bad54c40` (§4): several selected comments travel
   as a list (primary `/NM` plus `also`), a text range as one rectangle per line
   piece with the caret at its active end, and the canvas draws each quad. The
   range is re-addressed, not dropped, after a turn or a catch-up replay (§4,
   2026-11-12): the page and its glyphs are kept, the next frame carries the new
   `epoch`, and a page that is gone takes the range with it (`with_epoch`'s rule
   for a merge intent, applied to presence); a dropped socket clears the
   participant list, and a peer that goes quiet is the 30 s TTL (the protocol has
   no `leave` message).
   The field caret is closed in `c935ebadb` (§4, "Form-field carets in presence",
   2026-11-12): `Selection::Text` carries an optional `field: {name, offset}` —
   the field's fully qualified name and the caret's position in characters into
   the draft, with the widget's box and the caret's user-space point, never the
   text. The editor records it each frame and clears it when the keyboard leaves;
   a turn and a catch-up re-address it to the new `epoch`; a field or page the
   form no longer holds is not sent; a password shares no caret. A peer does not
   resolve the name against its own form, so a field deleted by *another* client
   keeps its box until the author's next frame or the TTL (a deleted page is
   dropped at once by the epoch rule).
   What is still open: a line's orientation inside a page is the
   axis-aligned box of its piece (the page's own `/Rotate` is carried); a
   selection spanning several pages names only the comments on the page in view;
   and the host page's `pdfcraft:presence`
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
6. **The cabinet contract is implemented, the queue survives a restart, `https`
   is still open.** `POST /v1/sessions`, the commit callback, the background
   publication queue and `409` exist (RFC §8.2, §4). A failed POST (network
   error, `429`, `5xx`) is retried by the daemon with an exponential backoff
   (1 s doubling to 60 s) under a derived idempotency key, without waiting for
   the next commit or the last-client-leave commit; a retry publishes the same
   version, never a second one, and a `409` stops the queue until an operator
   re-seeds the room (§4). The queue is no longer in-process-only (`21aaf357a`,
   §4): at start-up the daemon walks the store and queues every base `CURRENT`
   names that `CABINET` does not, so a room left `publishing` by a dead process
   reaches the cabinet with no commit, join or leave — the same base, the same
   `fileId:version:sha256` key, one version. The operator's way out,
   `POST /v1/sessions/{fileId}/resolve` (`62d48d30a`), names the cabinet's
   current version and etag, clears the flag and now publishes the refused base
   at once instead of waiting for the room to speak again; a repeat, a wrong
   version, another file or a role without write access is refused without a
   change. What is still missing: `https` without a proxy.
   The shared cabinet contract also lets the cabinet number versions and return
   its own `{version, etag}` from the callback; the daemon now adopts that body
   as the copy it holds (falling back to `expectedVersion + 1`) and keeps both
   numbers in `CABINET`, so a cabinet with its own numbering works as long as
   that number is per file and monotonic. **Done since (`fee60000b`, §4):** the
   cabinet's Save reaches the room through `POST /v1/sessions/{fileId}/commit`,
   which answers `{version, cabinet:{version, etag}}` and answers a clean room
   from its marker; the ticket's `{etag, version}` is the cabinet's own tag and
   numbering, compared with the marker instead of a digest of the bytes, and
   `CABINET.sha256` lets a moved tag that names the room's own publication be
   adopted. A marker written before that field, and an operator's `resolve`
   (which carries no bytes), still refuse a moved tag.
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
    daemon's queue with a backoff, and a restart recovers the queue from the
    store (`21aaf357a`, §4), so it no longer falls back to the next commit, join
    or leave.
12. **The legacy annotation payload lives until the flag ships.** A payload
    without `shape` is read as a sticky note; RFC §16 p. 10 sets the deadline —
    the first release with `collab` on by default makes `shape` mandatory.

## 6. Next session: plan

Work in this order unless the owner says otherwise.

**Deliver first.** Measured 2026-11-12T16:35Z (`git`, `gh pr view 6622`): the
fork and PR #6622 carry **`2a2b0c157`**; this clone's **code head** is
**`fee60000b`** ("a room-side commit for the cabinet, and an etag it can
recognise", §4), and `git rev-list --left-right --count
fork/feature/realtime-collaboration...HEAD` reads `0 3` — the code commit and
two docs commits (the cabinet-side record `d1d304231` and the one before this
paragraph). Everything the earlier versions of this section
listed as unpushed — `ac33dd212`, `00c06ed5c`, `c56da220b`, `c935ebadb` — is
**delivered**: they are ancestors of `2a2b0c157`, which is the fork tip and the
PR head; `git merge-base --is-ancestor 2a2b0c157 HEAD` exits 0. This agent's
permission still denies `git push*`, so `git push fork
feature/realtime-collaboration` from this clone is the owner's step; after it
`gh pr view 6622` must read this branch's tip — the docs commit above
`fee60000b` — and the fork's five workflows will
wait again for a maintainer's approval (`action_required`, so no check has
executed on the branch). The gate on this head is §4, "The cabinet's Save
reaches the room", and `pdfcraft/PR-DRAFT.md` now carries the same facts.
Everything below is still open feature work; nothing in it is a prerequisite of
that push.

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
commit, join, leave, the queued publication or the start-up recovery
(`21aaf357a`, §4) republishes it; and a kill between
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
  so `apps/pdfcraft-web` pins the PR's head commit `7666e2b1` for that crate
  only (ADR-0016 §3). It is fetched from `storytold/craft-libs`, where that
  commit is `refs/pull/716/head` — the same commit the other Craft apps take
  from the fork — because the CI job "Git dependencies come from storytold"
  rejects a git source outside `github.com/storytold/` (§4, "The CI git-source
  gate"). `Cargo.lock` records the storytold URL; after the craft-libs PR
  merges, the rev moves to the merge commit.
- The web feature is off by default:
  `cd apps/pdfcraft-web && trunk build --release --features collab`.

Tests and gaps: §4. The browser run this section left open is now on record
twice: against the cabinet's real page (§4, "The browser bridge in the
cabinet: D4 closed", 2026-11-11) and locally and reproducibly on the build
host (§4, "The web path in a browser: two iframes, one room", 2026-11-12 —
this branch's trunk bundle, a dev room, CDP). What remains for a person is the
cabinet's own commit (store the file, advance the version, send the real etag),
its origin/CSP with `wss://`, and two real profiles; the page-side wiring
WordCraft documented is the one the local run exercises.

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
  numbering). The publication queue is done in §4 (in-process) and its two open
  ends are closed in `21aaf357a` (§4): the restart recovers it from the store
  and the resolve publishes by itself. Still open here: `https` without a proxy
  (limitation 7).

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
selections are done: several `/NM` and per-line range geometry with the caret
(`5bad54c40`), re-addressed to the new epoch after a turn or a catch-up and
cleared when their page goes (§4, 2026-11-12); what remains is listed in §5 p.4
(a form-field caret, a line's own orientation, multi-page names, the host
payload).

### 6.6 Cleanup and submission

- **This pass (2026-11-12, 17:54Z): the head is `f107f56c2`, six commits above
  the delivered `2a2b0c157`.** The tree is clean and nothing was pushed. The six
  commits are `fee60000b` (room-side commit + etag, code), `d1d304231` (its
  record), `ad4f33787` (the branch's snapshot), `24f519a91` (a closed socket no
  longer takes an actor's seat, code), `826303ace` (that record, and the Save
  trap left open there) and `f107f56c2` (the Save trap closed: in a hosted
  session Save is the room's commit, code) — the header snapshot above lists
  each with its files and lines. `git rev-list --left-right --count
  fork/feature/realtime-collaboration...HEAD` reads `0 6`; PR #6622 is
  `MERGEABLE`/`UNSTABLE`, `reviewDecision` empty, every run
  `action_required`. `origin/main` moved to `120c62fb8`, so the branch is 148
  ahead and 3 behind; merging it is a separate decision. This pass changed **no
  code**: it rewrote the header snapshot, added §4's record of `f107f56c2` and
  its gate state, aligned `pdfcraft/PR-DRAFT.md`, and re-ran the full sweep on
  the head on host `agents` (§4, end of the `f107f56c2` section). The owner's
  steps are the push, the run approvals, the review and the manual desktop
  acceptance.
- **This pass (2026-11-12, 16:35Z): the branch's facts, and the one commit
  owed.** The code head is `fee60000b` on top of the delivered `2a2b0c157`,
  with two docs commits above it (`d1d304231`, the cabinet-side record, and this
  one); the fork and PR #6622 read `2a2b0c157`, `git rev-list
  --left-right --count fork/…...HEAD` is `0 3`, the tree is clean. This pass
  changed **no code**: it rewrote §6's delivery paragraph, this bullet, §7's
  first gotcha and `pdfcraft/PR-DRAFT.md` to that snapshot, added §4's
  record of `fee60000b` with the gate run on it, and folded the duplicate
  records of that commit into one. The earlier record that named
  `e7a056501` as the head and five unpushed commits is history: `c935ebadb`
  and the four commits beside it are ancestors of `2a2b0c157`. Nothing was
  pushed; the push, the run approvals, the review and the manual desktop
  acceptance (§5.2 p.4) are the owner's.
- **Delivery state (2026-11-11T23:37Z; head re-measured 2026-11-12).** The
  delivered head recorded here is `eb60bc5ff` —
  the delivery record of `6ac4e1130`, which is the D4 record on top of
  `0d3ea2708` (the etag fix, §4) on `28fd4e1b3` — and at that moment it was at
  once the local HEAD, the fork's branch tip and PR #6622's `headRefOid`;
  `git rev-list --left-right --count fork/…...HEAD` was `0 0` and the tree was
  clean. **Since then the PR head moved twice, and `gh pr view 6622` on
  2026-11-12 reads `headRefOid e7a0565014a786f431111f70f54ba2e20fb62355`
  (`updatedAt` 2026-11-12T01:12:28Z): `e7a056501` (a docs record) is pushed
  above `eb60bc5ff`, and this clone's HEAD was `ac33dd212` (the web-path record)
  on top of it before this pass. `e7a056501` is docs-only and has no workflow
  run of its own; the five runs below belong to `eb60bc5ff` and stay its
  record.** **`6ac4e1130` is no longer the fork head**: it is an earlier
  delivered head, below `eb60bc5ff`. This record is on top of `ac33dd212` and
  is local; the delivered head stays `e7a056501`
  until the owner or `pr-agent` pushes the branch again. The owner or the
  PR-sweep pass made those pushes, at 19:14Z, 21:10Z and 01:12Z; this agent's
  `git push*` denial was not lifted, so nothing in this file was pushed by the
  agent. `mergeable: MERGEABLE`, `mergeStateStatus: UNSTABLE`,
  `baseRefOid` `3650f01f2`, not a draft, `reviewDecision` empty, one check
  (CodeRabbit, `skipped`), and five workflow runs on `eb60bc5ff` (CI
  `43100841697`, SonarCloud `43100841668`, Nix `43100841572`, FreeBSD
  `43100841526`, Windows ARM64 `43100841785`, created 21:10:52–53Z) all
  `action_required`, i.e. none executed. The five runs on `6ac4e1130`
  (`43085349704`, `43085349832`, `43085349544`, `43085349527`, `43085349547`,
  19:14:33Z) are superseded with it. The
  branch is **135 commits** not in `origin/main` at `eb60bc5ff` (136 with this
  record) and **22 behind** it (`main` moved to `80e26e674`), and
  `git diff --shortstat origin/main HEAD` is 158 files, +44152/−2330.
  **`CI=1 cargo xtask ci` exits 0 on the code head
  `0d3ea2708`**, all ten steps (§4); the three docs commits above it change no
  code, so that gate covers `eb60bc5ff`. The "not pushed" notes in the older
  bullets below describe their own pass — they are history, not the current
  state.
- **The `deny` step's advisory is closed** (`f31f00449`, 2026-11-10, not
  pushed): `crypto-bigint` 0.5.5 (RUSTSEC-2026-0387) is out of the graph —
  `jsonwebtoken` selects `aws_lc_rs` instead of `rust_crypto` in
  `crates/collab-ws` and `apps/pdfcraft-collab-server`, the lock loses 17
  packages and gains none, `cargo deny --log-level error check` exits 0 and
  `cargo tree -i crypto-bigint@0.5.5` finds nothing (§4, "The JWT crypto
  backend"). `cargo xtask ci` passes all 10 steps on `c1171d941` (2026-11-10;
  §4, "The full CI sweep") — `fmt`, `clippy` (with the older `selection_caret`
  break fixed in `f48ec4754`), `cli-no-default-features`, `test` (9930 passed,
  0 failed, 18 ignored), i18n-report, layers, wasm, license-check, deny and
  parity.
- **The CI `git-sources` job passes on this tree** (`158ab7fa4`, 2026-11-10,
  not pushed): `craft-web-host` is fetched from `storytold/craft-libs` at
  PR #716's head instead of the fork, so `Cargo.lock` has no git source outside
  `github.com/storytold/` and `cargo metadata --locked` exits 0. The lock moved
  in one line and no code changed. Gates: §4, "The CI git-source gate". The PR
  head still carries the old lock until the owner force-pushes; the shared
  `gate` job of that run was still in progress when observed.
- **The outline and named destinations are done locally in §4**
  (`d1229deb9`, 2026-11-10, not pushed): `outline.item.add`/`update`/`delete`/
  `restore`/`move` and `dest.add`/`update`/`delete`, addressed by the item's
  object key and by the name, with a whole-subtree copy in the delete and its
  restore; the server recomputes key→path before each operation, and every
  refusal (unknown item, cycle, missing page or name, title/depth/count/copy
  bounds) happens before the first write. Tests: `collab-outline` (6),
  `organize` (3), `engine` (1), `collab-pdf` (5), `collab-client-pdf` (2) and
  `collab-ws` e2e (3, two editors and a viewer: converge, undo/redo of a
  subtree, a viewer refused, a stale key refused without a version move).
  **The look and the named-destination interface are done in §4** (2026-11-10,
  not pushed): `outline.item.update`/`add` carry `color`, `italic`, `bold` and
  `open`, the undo image is the whole node, every refusal (colour not three
  numbers from 0 to 1, a style that is not a boolean, `open` on an item without
  children) happens before the first write, the Bookmarks panel's colour,
  Bold/Italic and Expanded-in-the-document edits go through that one operation,
  and Bookmarks ▸ Named destinations… plus `destinations`/
  `destination_set`/`destination_delete` in `crates/automation` edit the
  document's names, and `bookmark_set_style` writes the open state headlessly.
  Tests: `collab-outline` (2), `organize` (2), `collab-pdf` (1 new, 10 refuse
  cases added), `collab-client-pdf` (1), `collab-ws` e2e (1) and `automation`
  (3: the named destinations and the open state).
- **The whole outline and the generators are done locally in §4**
  (`642dabb5b`, 2026-11-10, not pushed): `outline.replace` replaces the whole
  tree in one structural operation addressed to the outline itself, `epoch` is
  the compare-and-set, the accepted operation carries `items` and the whole
  `before` for its undo, an empty tree clears the outline, and
  `ImportBookmarks` with `replace` is no longer `unsupported` on the client.
  `BookmarksFromStructure` and `BookmarksFromTextStyles` build their tree in the
  replica (`Document::structure_bookmarks`/`text_style_bookmarks`,
  `collab-outline::tree_from_entries`) and send the same operation with the
  document's own list after the new parent. Tests: `collab-outline` (1),
  `collab-pdf` (2), `collab-client-pdf` (3) and `collab-ws` e2e (1, two editors
  and a viewer). (Links, the rest of TASK §3's list, are done in the bullet
  below.)
- **Links are done locally in §4** (`2b61ff80f`, 2026-11-10, not pushed):
  `page.link.set` and `page.link.clear` over a page's whole link list, a typed
  link body (rectangle or text quads; a page with a view, a named destination,
  a page of another file or a web address; the look), every refusal before the
  first write, a page whose link the model cannot carry refused whole, and
  insert/duplicate/blank/combine/move/split/delete/replace keeping a link's page
  and renumbering the destination numbers that are this document's. Tests:
  `collab-link` (6), `annot` (3), `organize` (2), `collab-pdf` (5),
  `collab-client-pdf` (2) and `collab-ws` e2e (1, two editors and a viewer).
  **The whole-document actions are done in §4** (2026-11-10, not pushed):
  `links.add_many` and `links.remove_many` carry several pages' whole lists in
  one structural operation, so `Edit::AddLinks` and `Edit::RemoveLinks` are no
  longer `unsupported`; one operation is one epoch step and one undo over the
  lists the pages had. Tests: `collab-link` (1), `annot` (1), `collab-pdf` (4),
  `collab-client-pdf` (1) and `collab-ws` e2e (1, two editors and a viewer).
  What is left: the link actions the model does not carry (JavaScript, Launch)
  still refuse the page whole.
- **Page labels are done locally in §4** (`c26867e9c`, 2026-11-10, not pushed):
  `page.label.set` and `page.label.clear` addressed by the page a range starts
  on, the whole replaced range in `before` for the undo (a list when the set
  split it, a compare-and-set), every refusal before the first write, and
  insert/split/combine/replace/duplicate keeping each page's label. Tests:
  `organize` (3), `collab-pdf` (3), `collab-client-pdf` (2) and `collab-ws` e2e
  (1, two editors and a viewer).
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
  /v1/sessions/{fileId}/resolve`, §4), and its two open ends are closed in
  `21aaf357a` (§4): a restart recovers the queue from the store, and a resolve
  publishes the refused base at once — no commit, join or leave needed.
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

- **The red head `95e2c9c97` is history; do not re-fix it.** It failed `fmt`
  and `clippy` on the import block the merge of `3a117c3c0` left in
  `crates/engine/src/session.rs` (a blank line at line 10, an unused
  `BookmarkRow` at line 14); `69b2a6dad` fixed both and the delivered
  `2a2b0c157` and the local head `f107f56c2` are above it. Before changing
  anything, re-run the gate on the current head: `CI=1 cargo xtask ci` with
  `CRAFT_I18N_DIR` (craft-i18n at `575cfdef`) and `CRAFT_FONTS_DIR`
  (`b076417b`) — §4, "Save in a hosted session…", has the run for `f107f56c2`
  and "The cabinet's Save reaches the room" the one for `fee60000b`. No GitHub
  workflow has executed on the branch (every run is `action_required`), so a
  local sweep is the only evidence there is.
- The branch's only difference from `origin/main` outside the feature is 4 lines
  in `crates/cos/src/object.rs`: the branch's `decoded_strict_within` delegates
  to `decoded_strict_within_counted`, main's keeps the `decode` closure. Both
  are correct; the merges of `d2916decb`, `4efcfa6ea` and `3a117c3c0` kept the
  delegation on purpose (§4). If
  a reviewer wants the smallest diff, take main's text verbatim instead of
  re-resolving that merge.
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
  the second would spin forever. The queue's entries do not survive a restart,
  but the work does: `rooms::restore_publications` re-derives them from
  `CURRENT` vs `CABINET` and publishes with the same derived `idempotencyKey`
  (§3.1 of `server.md`, §4).
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
