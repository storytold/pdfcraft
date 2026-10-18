# How to develop and fix a change

This is the procedure for changing PdfCraft. `AGENTS.md` wins if this file ever disagrees with it. `CLAUDE.md` is the day-to-day working note.

A passing test is evidence only for the behaviour that test actually runs. Write the check from the failure you can see, watch that check fail on the code without the fix, then make the same check pass. A test that was written to match the code after the fact does not prove the bug is gone.

## 1. Before writing code

1. Read the issue, the existing tests, and the crate README. Search for an open issue that is the same bug before opening another one.
2. Reproduce the failure with the smallest synthetic PDF or input that shows it. Do not use a personal file.
3. Name the crate. A change in `crates/edit` is not done in `crates/ui-egui` unless the screen is part of the bug.
4. Decide the checks first: one unit test, one tool or crate integration test when a command or file is involved, and a human session when a person clicks or types.

## 2. Rust

Write the smallest function that does one job. The caller should be able to tell what it returns without reading the body.

- Return the crate's error type with `?`. Do not `unwrap`, `expect`, `panic!`, `todo!`, or `unreachable!` outside tests unless the operation cannot fail, and then say why in a comment.
- Use `get` for a value that came from a file, a request, or a user. Slice strings only on character boundaries. Use checked or saturating arithmetic. Guard division and `NaN`.
- Cap a `Vec` or `String` whose size comes from input. Walk a graph with a seen-set or a depth limit.
- Keep one owner of a fact. Do not store the same width, index, or flag in two places and update only one.
- Keep the public function small. A private helper is better than a new `pub` item nobody outside the crate calls.
- Name a function for the result (`paragraph_span`, `rewrite_block`), not for the mechanism (`do_stuff`, `handle_data`).
- A comment states a fact the next reader cannot see from the code. Delete a comment that repeats the code or describes a behaviour the code no longer has.
- No `unsafe`. The workspace forbids it.
- Format with `cargo fmt`. `cargo clippy -p <crate> --all-targets -- -D warnings` must be clean.

## 3. Fixing a bug

1. Add the test that expresses the correct result, using the report's own numbers and text.
2. Run it on the code without the fix and keep the failure. The failure must be the bug (wrong paragraph, deleted neighbour, wrong text), not a missing file or a typo in the test.
3. Make the smallest code change that turns that failure into a pass.
4. Run the older tests in the same area in the same command, so the fix cannot bring a previous bug back.
5. If a neighbouring issue looks related, run that issue's own steps. Link it from the pull request only when those steps pass. A similar title is not enough.

## 4. Tests

Every new behaviour and every bug fix needs a test at the zoom of the change.

| Zoom | Where | When |
|---|---|---|
| Unit | `crates/<crate>/src/tests.rs` or the module's `#[cfg(test)]` | A function's result: empty input, the reported numbers, the boundary just below and just above |
| Crate integration | `crates/<crate>/tests/` | A save, a reopen, or two calls that share a document |
| Tool | `crates/automation/tests/` | A behaviour a person can reach from `pdfcraft-cli` or MCP. The tool and the test ship together |
| Human session | `crates/ui-egui/tests/play/*.json` | A click, a typed edit, a key, or a drag. The check is the text that opens or the paragraph list |

A bug fix is not done until step 2 above has been seen to fail. Say so in the pull request, with the assertion and the actual value.

### Coverage

Before calling the work done, measure the crate you changed:

```text
cargo xtask cover -p <crate>
```

That runs `cargo llvm-cov` and fails when line coverage of that crate is under 80%. Install the runner once with `cargo install cargo-llvm-cov`. The first run builds the crate with coverage instrumentation, so it is slower than `cargo test`.

```text
cargo xtask cover -p <crate> --fail-under-lines 70
```

`--fail-under-lines` replaces the 80% floor for that run. The command measures the whole crate. It does not compare the number with a previous run, and it does not count only the lines the change added. If the crate is already under 80%, record its current percentage with `--fail-under-lines` and cover the lines the change adds by reading the report. Do not lower that percentage. A line that cannot be reached in a test needs a comment that says why.

`cargo xtask cover` is a local check. CI runs `cargo test --workspace` on Linux and Windows. It does not yet fail the job on a coverage percentage.

### Human sessions

`crates/ui-egui/tests/play/` is the in-process equivalent of a browser driver. A JSON file opens a synthetic page and drives the real editor widgets: `command`, `click_page`, `type`, `key`, `drag_edge`, then `expect_editor` or `expect_paragraph`. A `page` field sets the MediaBox in points; omitted, the page is US Letter. Each file runs on more than one window size and zoom, because that changes where a click lands.

```text
cargo test -p pdfcraft-ui-egui --test play -- --nocapture
```

A failure names the session, the window, the zoom, the step, the paragraphs it found, and writes a screenshot under `/tmp/pdfcraft-play/`. Page coordinates in a session are PDF points. The default page is US Letter, 612 by 792.

`expect_editor` reads the paragraph that is open after a click. That is the text a person sees. Do not point it at the first paragraph that happens to contain the same words.

The control channel (`pdfcraft --control FILE` and `pdfcraft-cli ui`) drives a real window. Use it to look at a change. The `play` test is the one that runs unattended, including when the desktop window is covered.

## 5. Before the pull request

On the crates you touched:

```text
cargo fmt -p <crate> --check
cargo clippy -p <crate> --all-targets -- -D warnings
cargo test -p <crate>
cargo xtask cover -p <crate>
```

Add `cargo test -p pdfcraft-ui-egui --test play` when the change is something a person does on screen, and `cargo test -p pdfcraft-automation --test automation` when a tool changed.

For a bug fix, list the command, the failure observed before the fix, and the pass after it. For other changes, list the applicable checks and their results. Name any similar issue you ran and whether its own steps passed.
