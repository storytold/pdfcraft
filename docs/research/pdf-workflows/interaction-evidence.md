# Interaction evidence and evaluation reference

Detailed source inspection used PdfCraft revision
`3f044b6263aa6605c3aa44ca6d065e1ab37d13b6`. A later documentation review used
`924c4da4ac2879ac9107c6c82e20ac6be5570f0a` on 2026-10-17. Links retain those
inspected revisions. Neither review was a runtime parity assessment.

This reference summarizes interaction findings from archived comparative research and
relates them to existing PdfCraft mechanisms. It contains no implementation plan,
proposed workspace design, feature commitment or new architecture. The project's
roadmap and machine-readable parity checklist retain their planning and status roles.
Historical design examples and test descriptions are distinguished from current product
evidence below.

## Existing mechanisms and corrected historical descriptions

Several early comparative notes described capabilities that later existed in PdfCraft.
The table records the inspected evidence and its limits rather than preserving those
absence claims.

| Area | Inspected evidence | What that evidence establishes |
|---|---|---|
| Command discovery and dispatch | [`commands.rs`](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/ui-egui/src/commands.rs), [`palette.rs`](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/ui-egui/src/palette.rs) | Menus, shortcuts, palette and other entry points share command metadata/dispatch. The palette is an existing feature, not an absent research concept. |
| Measurement | [`measure_ui.rs`](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/ui-egui/src/measure_ui.rs) | Distance/perimeter/area controls, calibration, snapping and saved-measurement state exist. An older session-only calibration description does not describe every current persisted scale. |
| Comparison | [`compare_ui.rs`](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/ui-egui/src/compare_ui.rs) | Text changes, visual regions, document highlights, report and comment actions exist. This older source link does not establish all later split-view behaviour. |
| OCR | [`ocr_ui.rs`](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/ui-egui/src/ocr_ui.rs) | Background recognition, cancellation request and multiple-file processing exist. Cancellation can retain pages already recognized; it is not synonymous with rollback. |
| Accessibility review | [`accessibility.rs` tests](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/ui-egui/tests/accessibility.rs) | Source includes checks for finding navigation, Tags-panel review, automatic text tagging and per-document skipped-rule state. Tests were inspected, not executed for this handoff. |
| Interface appearance | [`theme.rs`](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/ui-egui/src/theme.rs) | A shared token type and System/Light/Dark preferences exist. Their presence is not a measured contrast result. |
| International text input | [`commands.rs` tests](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/ui-egui/tests/commands.rs) | Palette tests cover idle composition frames, Chinese preedit/commit, cancellation and outside-click behaviour. This does not establish native input-method behaviour in every editable field. |
| Automation conventions | [`automation/README.md`](https://github.com/storytold/pdfcraft/blob/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6/crates/automation/README.md) | The documented contract uses displayed top-left rectangles, 1-based pages/positions, confined paths and a shared tool table. Historical mixed-coordinate criticism is not current evidence of a defect. |

The later [feature checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml)
marks synchronized side-by-side comparison shipped. Its notes describe two document
views when the older version remains open and the window is at least 1000 pixels wide;
synchronization follows offsets rather than matching content. The record identifies
page-size drift and old-tab closure as limits. It also records Action Wizard
creation/editing/running and recursive CLI/MCP folder batches with bounded recipes,
relative paths, collision-safe output names and per-file reports.

The later [roadmap](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/ROADMAP.md)
records changed-on-disk Save confirmation and a shared staging-file writer. It describes
new outputs being reopened/checked before publication. These are existing safety
mechanisms; the archived research is not evidence that atomic saves or conflict
notifications are absent.

Some prose gap tables at the same revision still describe comparison or Tags panels as
missing, while detailed checklist entries and inspected source describe implementations.
This is a documentation inconsistency in the inspected snapshot, not evidence that the
code is absent or that its listed limitations are solved.

## What the archived interaction research distinguished

The research compared workflow structures rather than implementation languages. Its
recurring distinctions are useful when reading feature descriptions and test evidence:

- **Document identity versus current selection.** A tab index, display name and filesystem
  path describe different things. Changing the selected tab does not change which source
  a previously started operation analyzed. Reopening the same path can create a different
  session, and an edit can change the revision within one session.
- **Analysis versus edit versus export.** A finding list describes an input revision; a
  preview can remain uncommitted; an export can describe an invocation snapshot. The
  existence of a preview does not establish that document bytes changed or a file saved.
- **Cancellation request versus terminal cancellation.** A request can be pending while
  a worker reaches a boundary. A completed subset may be intentionally retained. An
  atomic commit and the surrounding multi-file run have different interruption scopes.
- **History availability versus restoration outcome.** A named undo entry establishes
  that history exists. Successful restoration, rendering refresh and save/reopen are
  separate observable outcomes. The research did not reproduce a PdfCraft history failure.
- **Atomic replacement versus destination conflict.** Staging avoids half-written output;
  conflict detection deals with another writer changing the intended destination. They
  address different failure modes. The roadmap records mechanisms in both categories.
- **Automated finding versus conformance.** A checker pass describes supported checks.
  Manual review, unsupported constructs and a report's input identity affect how that
  result can be interpreted. A pass is not independent certification.

These distinctions are an interpretation vocabulary, not a checklist of newly required
features. The underlying product's declared behaviour determines whether retaining a
partial result or refusing an operation is the expected outcome.

## Evaluation observations by workflow

The following table describes what the archived evaluations observed or separated. It
neither reports a fresh PdfCraft test run nor assigns implementation tasks. Example
conditions describe evidence scope rather than newly discovered bugs.

| Workflow | Observable relationship examined | Limits on interpreting the observation |
|---|---|---|
| OCR review | The recognized word, source region, selected row and magnified crop referred to the same token; a saved correction was distinguished from text displayed only in a preview. | Confidence was backend evidence, not a universal accuracy score. Correcting invisible searchable text and visibly editing a scanned phrase were distinct operations. |
| Comparison | Original/revised direction, selected change, displayed pages and exported report referred to the same comparison. Filtering and review disposition were distinguished from the full result set. | Equal text did not establish equal appearance or structure. Offset synchronization and content correspondence were distinct. A missing corresponding page was not page zero or a valid empty match. |
| Batch processing | The input, selected action, current step, saved outputs and per-file failures were separately observable. Completed outputs remained meaningful in a partially failed run. | Per-file atomicity did not imply a whole-run transaction. A retry could repeat a previously committed effect. `command_batch` documents that earlier edits can remain after a later error. |
| Accessibility review | Findings, selected tags and page locations referred to the same reachable objects. Proposed classification and applied structure were distinguished. | An automatic structural result did not establish reading quality. Text-only tagging evidence did not cover arbitrary images, tables or existing structure. |
| Standards checks | The profile, checked revision, findings and saved conversion artifact were separate evidence fields. | Structural checks for a subset of a standard were not a full validation claim. An unavailable external validator was not a passing result. |
| Page organization | Source selection, destination identity, permutation and resulting saved page order were evaluated separately. | Successful page transfer did not by itself establish preservation of every bookmark, form relationship or structure tree. |
| Redaction review | Marked regions, committed removal and checks on the saved output were separate stages. Verification wording was tied to the supported check scope. | A visual rectangle was not removal evidence. Reports and logs could themselves expose removed content. Absence under one extraction strategy did not prove absence under all encodings. |

Archived state-machine notes used terms such as running, review-ready, saving, complete,
partial, failed and stale. Those terms describe distinct user-visible outcomes; they do
not establish that PdfCraft has, or needs, a single global state-machine type. Likewise,
historical multi-pane layouts described a way to observe several related values at once;
they are not adopted interface designs or a request for a replacement shell.

## Keyboard, input and appearance evidence

The research separated semantic labels from complete task access. A headless
accessibility tree can expose a control's name, role and state without establishing that
a person can finish a keyboard-only task or that a native screen reader announces the
result clearly. Focus ownership, return focus after closing a surface, background focus
changes and non-drag alternatives were observed independently from visual arrangement.
No native screen-reader session was performed for this handoff.

[WCAG 2.2](https://www.w3.org/TR/WCAG22/) supplies technology-neutral criteria for
keyboard operation, focus, contrast and target sizes. It is a reference, not evidence
that this native application conforms to a level. The [WAI-ARIA tabs pattern](https://www.w3.org/WAI/ARIA/apg/patterns/tabs/)
describes web tab focus/activation semantics; it does not determine native widget or
platform accessibility implementations.

Input-method research distinguished preedit text, committed text, selection, undo and
cancellation. Enter/Escape can have composition meanings in addition to dialog meanings.
Chinese palette composition tests exist in the inspected source. Their presence does
not establish candidate-window behaviour on Windows, macOS or Linux, or equivalent
coverage in other fields. RTL interface direction and PDF coordinate conventions were
also separate concerns.

Appearance research distinguished interface chrome, page content and reading-colour
transforms. Switching an interface theme did not constitute editing a PDF's colours.
Token tables described intended colours; painted states, alpha composition, focus and
selection established different evidence. Layout evaluation recorded viewport, scaling,
translation length and text direction because a layout observation at one size did not
establish results at other sizes. No archived palette, icon family, font, screenshot or
prototype layout is included or recommended by this reference.

## Evidence boundaries

The material includes archived rationale, source inspection, documented conventions and
proposed-test descriptions from historical research. It does not include fresh native
acceptance, manual assistive-technology checks, independently measured OCR accuracy,
rendering benchmarks or external-reader compatibility results for this handoff.

Names of tests in linked source establish that the source contains those tests. They do
not mean the tests were rerun here, or that an archived implementation's test result
transfers to PdfCraft. The code-inspection and documentation baselines differ and remain
explicit. Current statuses and future work belong to the repository's existing roadmap
and parity records, not this evaluation reference.
