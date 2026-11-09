<!-- craft-house-rules:start core source=1315c63e17905daf7dbc6dc6f8c49384e415162b sha256=540e6cea75d12d0710c3dca89658328e81e508ecef102446ca5bb95b7c0b92bb -->
## House rules

These apply in every Craft repository. A later rule may be stricter, not weaker. The source is `standards/house-rules-core.md` in `storytold/craft-contrib`. Propose changes there, not in a copy.

1. **A person is accountable.** Whoever runs an agent answers for what it opens and for its review comments.
2. **Check, then claim.** Before writing code, read the issue comments, search pull requests for `#N` and the keywords, and check recent commits on `main`. Comment on the issue first. Release a claim you will not finish.
3. **Verify, don't guess.** Open a definition before using it. Confirm the cause in the code, even when someone names a location.
4. **Keep fixes narrow and tested.** Fix what the issue describes, where the bug is. Add a test that fails without the fix.
5. **Review the diff before opening a pull request.** Identifiers exist as used, the test fails without the fix, nothing unrelated changed, and the description is true.
6. **Look for a better pull request.** If another fixes the same thing, close yours when it is better or equal, and say why.
7. **Report only what ran.** Say what you tested and what you could not run. Without a local build or test, say so and rely on CI before marking ready.
8. **`main` changes through a pull request.** Do not push to it. Branch protection enforces this. There is no exception.
9. **Stop and ask only about** licensing outside the allowlist, publishing, releases, a new remote, secrets, spending money, destructive git, and anything else hard to undo. Otherwise keep working. `plan/` is a gitignored notebook, not rules. Without it, do not ask for it and do not stop.
10. **Keep secrets and personal details out** of the repository, logs and requests.
11. **CI stays shared.** Workflows call `storytold/craft-common-actions`. Do not change CI in a feature or fix pull request. If CI cannot cover a change, open an issue there.
12. **Guards say why.** A check that looks removable says what it prevents. Read that before changing it.
13. **Instructions must be readable by everyone.** No notes about one machine, and no links to files a reader cannot open. `craftrules` is retired. Do not link to it.
14. **Keep pull requests moving.** Concurrent pull requests are fine; unanswered actionable feedback is not. Respond in 7 days, a reminder at 14, closure eligible at 30, from when an actionable request waits on the author. Suggestions and unconfirmed bot findings do not start the clock. A fix, explanation or agreed plan resets it; unrelated activity does not. Waiting on a reviewer or maintainer pauses it and does not count against the author. A maintainer decides closure; it is never automatic. Reuse search results, do not poll, and stop at a rate limit.
15. **No agent attribution.** A pull request does not say an agent wrote it. A repository rule may not require that.
<!-- craft-house-rules:end -->
<!-- craft-house-rules:start rust source=1315c63e17905daf7dbc6dc6f8c49384e415162b sha256=642f46b4ac38cc9e4d10d1ccde19d68cb6bc03d20e4c6236a7535f3047566262 -->
## Rust Craft repositories

These apply to native Craft apps and to shared Rust libraries. Select them by repository type. A repository that merely contains some Rust does not take this block. Rules later in a repository's file may be stricter, but not weaker. The source is `standards/house-rules-rust.md` in [storytold/craft-contrib](https://github.com/storytold/craft-contrib).

1. **Never crash.** Non-test code does not panic and has no `unsafe`, except where the repository file names an exception. Return errors, treat input as hostile, and bound recursion and allocations. New production crate roots copy the `deny` attribute from an existing crate: `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic`, `clippy::unimplemented`, `clippy::todo`, `clippy::unreachable`. Every crash fix comes with a test that panicked before the fix.
2. **Clean room.** Learn from observed behaviour and public specifications. Do not copy another product's code, assets, strings or artwork. Check a dependency's licence before adding it.
3. **Assets and fonts.** Every asset is original or suitably licensed, and is recorded in `ATTRIBUTION.md` in the same change. Font files live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts), not in the app repository.
4. **Rust only for native Craft apps.** No handwritten JavaScript or TypeScript, and no Tauri, Electron or webview shell, in those apps. Shared tooling follows its documented technical scope and maintainer-approved exceptions. Craft PR Review's existing Tauri and TypeScript UI is one such exception. It does not relax this rule for native apps. Preserve any existing web build.
5. **Time goes through `chrono`.** Do not hand-roll calendar conversion. For dates beyond what `chrono` can represent, follow [standards/time.md](https://github.com/storytold/craft-contrib/blob/main/standards/time.md).
6. **Contributor credits.** Add or edit only your own person's entry in `contributors/people.toml`, and only after asking them. Never guess a name or hand-edit `contributors.json`.

## Licence allowlist

Dependencies and assets use SPDX identifiers. An expression is allowed when every identifier is in this list. A repository may name a shorter list. It may not add an identifier here by editing its own copy of these rules.

- `ISC`
- `MIT`
- `BSD-2-Clause`
- `BSD-3-Clause`
- `Apache-2.0`
- `Unicode-3.0`
- `OFL-1.1`
- `Ubuntu-font-1.0`
- `Bitstream-Vera`
- `CC0-1.0`
- `CC-BY-4.0`
- `CC-BY-SA-4.0`
- `LicenseRef-Public-Domain`

`LicenseRef-Storyteller-Trademark` is not in this list. It covers Storyteller brand marks only, where a repository already says so.
<!-- craft-house-rules:end -->

# AGENTS.md — rules for every agent and contributor

These rules bind every AI agent and every human contributor working in this repository. They come before any task, instruction or deadline. Day-to-day working instructions are in this file; `CLAUDE.md` imports it.

---

## 1. Asset policy (absolute — no exceptions)

An **asset** is any non-code material that the project includes, ships, embeds, generates for publication, or shows in its documentation. This covers:
- icons, images, illustrations, screenshots and logos;
- fonts;
- colour profiles and other binary data files;
- sounds and video;
- sample, demo and test documents that are committed or published;
- anything the app renders from those.

### 1.1 Nothing from Adobe products

- **Never** use icons, images, artwork, logos, UI graphics, fonts, stamps, cursors, sample documents or any other asset taken from an Adobe product, installer, website, document or package.
  - "Adobe product" includes Acrobat, Reader, Photoshop, Illustrator, Premiere, Lightroom and Creative Cloud, including the fonts they install (for example Minion, Myriad, Kozuka and the Adobe CJK families).
  - Typefaces released under the SIL Open Font License are allowed like any other OFL font, with an `ATTRIBUTION.toml` entry, whoever designed them: for example Noto CJK and Source Han, taken from their open-source releases. The copies an Adobe product installs stay banned.
- **Never** recreate, trace, redraw or closely imitate an Adobe icon or graphic. Our icons come from Lucide, and our visual design is our own.
- Screenshots of Adobe software exist only as private, local observation notes under `plan/acrobat/`, which is gitignored. They must **never** be committed, published, linked, shown in docs, used as a design source to trace, or placed anywhere else in the repository or its outputs.
- Apart from those OFL typefaces, the only Adobe-authored material allowed is **non-visual technical data** that is openly licensed and required for PDF compatibility. Each item needs its own entry in `ATTRIBUTION.toml` with `adobe_data = true`, and the list below is closed:
  - The CMap resources (BSD-3-Clause), bundled inside the `hayro-cmap` dependency.
  - The standard-14 font metrics and encoding tables in the PDF specification. They are data, not typefaces. They are embedded as code in hayro, and as our own generated table in `crates/fonts/src/std14_metrics.rs` (approved by the owner, 2026-10-08; that file holds the data alone, so the code reading it can change without re-attesting the table), which is what generated appearances measure with and what survives hayro being retired.

  Adding anything to this list needs the user's explicit approval.

### 1.2 Only open, provable licences

Every asset must be one of:
- **Open source:** ISC, MIT, BSD-2-Clause, BSD-3-Clause, Apache-2.0, OFL-1.1, Ubuntu-Font-1.0, Bitstream-Vera or Unicode-3.0 (Unicode, Inc. data such as ICU4X's line-breaking dictionaries; already allowed for code in `deny.toml`).
- **Creative Commons:** CC0-1.0, CC-BY-4.0 or CC-BY-SA-4.0.
- **Public domain.**
- **Contributor-original:** created, photographed or generated by a contributor who owns it and licenses it under the project licence (`MIT OR Apache-2.0`).

**One narrow exception: the ArtCraft brand.** The ArtCraft name, wordmark and logos in `docs/brand/` are trademarks of the ArtCraft Team and are **not** open source (manifest identifier `LicenseRef-Storyteller-Trademark`, terms in `docs/brand/LICENSE-brand.txt`). They may be used only unmodified, only within PdfCraft, and only as `kind = "logo"` entries from that directory, to identify the project. Forks and modified versions must remove them. Never add other files under this licence, and never modify the logos.

Anything else is forbidden, including assets whose licence you cannot prove from the source. Examples:
- system fonts (Apple, Microsoft, Google Android system UI fonts that are not openly licensed);
- emoji artwork from Apple, Microsoft or Samsung;
- stock photos;
- "free for personal use" material;
- screenshots of other people's software.

**Screenshots of PdfCraft** are contributor-original. Everything visible in them must itself be allowed: our UI, Lucide icons, OFL fonts, and demo documents built only from allowed assets. Never screenshot a document that uses proprietary fonts or images.

### 1.3 Every asset is attributed

- **`ATTRIBUTION.toml`** (repo root) is the single source of truth. It has **one entry per asset file**, whether committed, vendored, fetched at build time or bundled through a Cargo dependency. Each entry records:
  - path or URL, title, author;
  - source (URL, with version or commit);
  - SPDX licence, and the licence file in the repo;
  - SHA-256;
  - how it is used.
- **Translations** live in craft-i18n (`locales/<code>/common.tsv` and `pdfcraft.tsv`), embedded at build time from the checkout pinned in `craft-i18n.rev`; PdfCraft keeps no catalogs. `ATTRIBUTION.toml` records them as one `[[build_input]]` (`kind = "translation"`); craft-i18n's own `ATTRIBUTION.md` credits the translators. Every `[[asset]]` (fonts, images, data) is hashed.
- **`ATTRIBUTION.md`** is generated from it (`cargo xtask assets --write`). Never edit it by hand.
- **`NOTICE`** keeps the licence notices that the licences require.
- **`cargo xtask assets`** runs in `cargo xtask ci`. It fails if:
  - an asset file in the repository has no entry;
  - an entry's hash is missing or does not match;
  - a licence is not on the allowlist;
  - a licence file is missing;
  - an image, icon or font names Adobe as author or source, except a font licensed under OFL-1.1 alone.
- **`cargo xtask demo-pdf`** downloads its fonts only from `ATTRIBUTION.toml` entries, pinned by SHA-256. Afterwards it **fails if the PDF embeds any font not in its allowlist**, which catches the browser silently falling back to a system font.

### 1.4 Fonts live in craft-fonts

- Font assets shared by the Crafting Apps, including every Japanese font, live in [`storytold/craft-fonts`](https://github.com/storytold/craft-fonts), not here. **Never commit a font file to this repository**; adding a font means adding it to craft-fonts. (The Latin UI fonts already in `assets/fonts/` — Inter, JetBrains Mono, Dancing Script — stay.)
- craft-fonts is an **optional build input**, never a Cargo dependency: `CRAFT_FONTS_DIR=<checkout> cargo build` makes `crates/fonts/build.rs` embed its fonts as `pdfcraft_fonts::CRAFT_FONTS`. Without it PdfCraft builds, tests and runs, but has no Japanese face (Japanese UI text shows replacement boxes, and writing Japanese into a PDF returns a clear error). Release builds always set it (`.github/workflows/release.yml`), and `ATTRIBUTION.toml` lists it as a `[[build_input]]`.
- The desktop app may read one font **already installed on the user's machine** at runtime, as the last interface fallback for characters no embedded face has (`crates/ui-egui/src/system_fonts.rs`). That font is never embedded, shipped, committed or copied, and never appears in published screenshots (`PDFCRAFT_SYSTEM_FONTS=0`, which `cargo xtask screenshots` sets). It is not a way around §1.2: nothing from it may enter a PDF, a package or the repository.
- The page viewer may also draw a CID font of an Adobe CJK collection (Japan1, GB1, CNS1, Korea1) that a PDF does **not embed** with one face already installed on the machine (`crates/render/src/system_font.rs`), **on screen only**. `RenderConfig::system_fonts` is off by default and is enabled only for on-screen viewer and preview rasters (`RenderConfig::on_screen`, used by the engine viewer, Combine thumbnails and accessibility figure previews), so exports, printing, OCR and the automation tools never use it. Like the interface font it is read at runtime and never embedded, shipped, committed or copied; Noto CJK and Source Han are never candidates (§1.1), nor is any Adobe family; and `PDFCRAFT_SYSTEM_FONTS=0` turns it off. It is not a way around §1.2 either: nothing drawn with it may be saved or printed.

### 1.5 Procedure for adding or changing an asset

Fonts go to craft-fonts instead (§1.4).

1. Find an asset under an allowed licence from its **original source**: the author's repository or a release, not a random mirror. Record the exact version or commit.
2. Confirm the authorship. It must not be Adobe (§1.1). Keep the licence text.
3. Add the file together with its licence file, plus an `ATTRIBUTION.toml` entry. Run `cargo xtask assets --write`, then `cargo xtask assets`.
4. If an asset's origin or licence is unclear, **do not add it.** Ask the user.

If you find an asset that breaks these rules, stop and remove it from the repository and every output, including docs, screenshots and generated PDFs. Then tell the user at once.

---

## 2. Clean-room rules (summary; `plan/adr/0001` is an optional local note)

- Never read, disassemble or copy anything inside the Acrobat bundle. Only names and listings are allowed, and **never** open `Contents/Resources/JavaScripts/`.
- Behaviour comes only from public specifications (ISO 32000-2, the Arlington model), public documentation and black-box observation.
- Never copy GPL/AGPL code. Copyleft tools run only as external oracle processes.
- Observe Acrobat only with synthetic fixtures. Never capture personal data, account information or recent files.
- Shared real-file test oracles (Photoshop-authored PSDs, etc.) live in [`storytold/photocraft-corpus`](https://github.com/storytold/photocraft-corpus). Never commit large binary fixtures to this repo; fetch them pinned by commit and sha256-verified, as PhotoCraft does with `cargo xtask corpus`.

## 3. Agent control (automation and MCP)

- **Everything is reachable headlessly.** Every user-facing feature must be callable without the GUI: an engine API, then a tool in `crates/automation` (`pdfcraft-cli run` and MCP use the same table). A feature is not done until its tool exists and has an end-to-end test (`crates/automation/README.md`, "Adding a tool").
- **The UI control channel is opt-in too.** It runs only when the app is launched with `--control <file>`. It binds to loopback, uses a fresh random token per launch, and writes it to an owner-only file. Never enable it by default, widen the bind address, or log the token.
- **The MCP server is opt-in.** Never make the app, the CLI or an installer start it, register it with an agent, or listen on a port by default. It runs only when a user (or an agent the user configured) launches `pdfcraft-cli mcp`. Any future network transport must be off by default, bound to loopback and authenticated with a token.
- **Safe by default.** Tools must never discard unsaved work silently, must write atomically, must honour `--root`, and must return errors an agent can act on (say what was wrong and what is allowed).
- **Track parity honestly.** Every user-facing feature has an entry in `parity/acrobat-features.toml`. Mark it `shipped` only when it works end to end and the entry cites a passing test (`evidence`); `cargo xtask parity` (part of `xtask ci`) rejects claims whose commands, tools or tests don't exist.
- **Use it to test.** Agents should drive the engine through `pdfcraft-cli run --script` or the MCP tools, and look at `page_render` output, to check their own changes. For UI changes, launch the app with `--control` and use `pdfcraft-cli ui` (`inspect`, `click`, `screenshot`) to check the real interface.

## 4. Never crash

PdfCraft opens files from strangers, and people trust it with their work. A malformed PDF, a bad tool argument, a corrupt settings file or a full disk must produce an error the user (or agent) can act on, never a crash and never lost work. **This outranks feature work:** don't ship a feature by adding a panic path, and fix a crash before building on top of it. The rules below are the ones that bind this repository.

- **Fail with `Result`.** Non-test code returns errors through the crate's error type and `?`. Where readers are lenient by design, fall back gracefully and record the repair (see Fidelity below).
- **No panicking shortcuts.** No `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!` or `unimplemented!` outside tests, unless the call is provably infallible (e.g. writing to a `Vec`), with a comment saying why. `assert!` only for internal invariants that input cannot reach.
- **No `unsafe`.** The workspace sets `unsafe_code = "forbid"`. If a platform API needs it, use a maintained safe wrapper crate; a real exception needs the owner's sign-off.
- **Treat every input-derived number as hostile.** Use `get()` instead of `[i]` or `[a..b]`. Slice strings only at char boundaries. Use checked/saturating arithmetic for lengths, offsets and counts. Guard against division by zero and NaN/inf casts, and never cast a negative value to `usize`. Cap allocations sized by the file.
- **Bound recursion.** Object graphs can be cyclic or deeply nested. Walk them with seen-sets or depth limits.
- **Don't cascade.** Handle lock poisoning, and keep a panic in a worker thread from taking the app down. Engine entry points that run untrusted-document code keep their last-resort `catch_unwind` guard, and command execution and file import/export must sit behind one that turns an escaped panic into an error message and keeps the user's document. Keep `panic = "unwind"` (the default) so the guard works. It is a safety net, not a substitute for the rules above.
- **Prove it.** Every crash fix comes with a small synthetic regression test that panicked before the fix. Turn fuzzer findings (`cargo xtask fuzz`) into such tests before fixing them.
- **Enforced by clippy.** Root `clippy.toml` lets tests use `unwrap`/`expect`/`panic!`. A crate that is clean carries `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`; new crates start with it.

## 5. Page interaction, fidelity and gates

- **Layering.** Nothing below L7 depends on egui/winit/eframe/rfd. `cos`, `filters`, `crypt` and `arlington` stay standalone.
- **Page input.** Page modules read `PagePointer` (`crates/ui-egui/src/page_pointer.rs`), never egui's raw pointer state. `page_pointer::tests::page_modules_read_the_pointer_only_through_page_pointer` enforces it.
- **Page cursor.** Page modules claim the cursor for what is under the pointer (`view.cursor.claim`, `crates/ui-egui/src/canvas/page_cursor.rs`) and never set it themselves. `page_pointer::tests::page_modules_claim_the_cursor_and_never_set_it` enforces it.
- **Page presses.** A page module decides what is under a point in one hit test (`Hit` in `crates/ui-egui/src/canvas/page_router.rs`) and acts on a press only through that router. Modules never read the router; they are told. `page_pointer::tests::page_modules_act_on_a_press_only_through_the_router` enforces it.
- **Page keys.** Page code (everything under `canvas/` and `tools/`, `app/keys.rs`, `app/clipboard.rs` and the Pages panel's `panels/pages.rs`) reads keys and clipboard events through `PageKeys` (`crates/ui-egui/src/canvas/page_keys.rs`), which has none while something is over the pages (a dialog, the palette, a popup, a menu: `PdfCraftApp::over_pages`), never egui's raw key state. The functions `NOT_PAGE_KEYS` lists, each with why, read raw: a window's or a field's own keys, which `PageKeys` would hide, and the few page reads it can't make (chords that wait for the menu bar instead, Escape ending what the pages hold, any key stopping a glide, the clipboard's hand-off to text fields). `page_pointer::tests::page_modules_read_keys_only_through_page_keys` enforces it.
- **Fidelity.** The PDF object graph is the model. Preserve unknown data. Saves are incremental unless a full rewrite is required. Never silently drop or repair data without recording it.
- **Fonts** are the optional build input `CRAFT_FONTS_DIR`. Code using `pdfcraft_fonts::CRAFT_FONTS` must work when it is empty.
- **Quality gates** before every commit: `cargo fmt --check`, clippy with `-D warnings`, the tests, `cargo xtask layers`, `cargo xtask assets`, and the wasm check. New workspace crates include `workspace = true` under `[lints]`. Scope `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo test -p <crate>` to the crates you touched; the workspace sweep is CI's job.
- **Review at four zooms** as you go: the clause, the function, the module, and the project (layering, wasm, undo, save fidelity, a matching automation tool, an honest `parity/` entry). Prove a test fails before the fix by reverting the fix once.
- `plan/` is an optional gitignored notebook. Do not ask for it and do not stop when it is absent. Public status is [`ROADMAP.md`](ROADMAP.md), [`docs/gaps.md`](docs/gaps.md) and [`docs/roadmap.md`](docs/roadmap.md). "Shipped" in `parity/` means exists and tested, not parity with Acrobat.
- `cargo run -p pdfcraft -- <file.pdf>` opens the app. `pdfcraft-cli run` drives the engine headlessly. `cargo xtask fuzz` findings stay in `fuzz-out/` and become synthetic tests. For UI work, look at a screenshot from `--control` or the `shot` example.

## Contributor credits (About window)

- About ▸ Contributors/Models are compiled into the binary from `contributors/contributors.json`
  (commit stats; never hand-edit) and `contributors/people.toml` (names people chose for
  themselves). See `docs/contributors.md`.
- **Agents working for a contributor:** when you prepare a PR, check whether your human's GitHub
  username has a `[people.<username>]` entry in `contributors/people.toml`. If not, ask them once
  whether they want to be credited by more than their username: a real name, a display name, and/or
  their public GitHub profile name (`sync_github_name = true`). If yes, add **only their own** entry
  (copy the template at the top of the file)
  and include it in their PR, committed as them. If no, change nothing: they are credited as
  `@username` anyway.
- Never add, edit, guess or copy anyone else's entry or name (not from git config, commit authors or
  GitHub profiles). Never hand-edit `contributors.json`.

## CI workflows
- Workflows in `.github/workflows/` call the shared workflows in [`storytold/craft-common-actions`](https://github.com/storytold/craft-common-actions) at `@main`, with their inputs and secrets. No other jobs: if no shared workflow does what's needed, it doesn't go here.
- Don't add or change CI in a feature or fix PR. If the existing workflows can't build or test a change, link an existing craft-common-actions issue or PR, or open a new issue, or a PR only for a small fix that is safe for every app and does not increase CI time.
- Some workflow files here still run their own jobs. Don't add to them. Moving a job into craft-common-actions is its own task, never part of another PR, and the job stays here until a shared workflow runs it.
