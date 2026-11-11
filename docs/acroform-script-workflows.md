# AcroForm script workflows

This contribution connects the existing AcroForm object model to the native viewer and the
shared automation tools. The regression fixture is generated in Rust; it contains no private
document, employee, signature image or real PIN.

## Runtime and lifecycle

Each open document owns a `ScriptSession`. On native targets its Boa context lives on one
worker thread with a dedicated 64 MiB stack reservation (the transient stateless runner retains
its separate stack policy). Document scripts initialize once with `Doc/Open`, before the requested event;
button, console, format, validate and calculate scripts then share that context. Field objects
read the latest snapshot supplied by the engine, including objects retained by earlier scripts.
Exposed Field objects are cached by PDF object identity; synthetic fields use name keys. Separate documents cannot share globals or
icon objects. `eval` and dynamic Function construction remain disabled.

Changing the document-script sources recreates the context. Closing, reopening, reloading,
Revert, disabling JavaScript, and saving a full rewrite release/reset the context. Incremental saves retain it. A rewrite can reuse PDF object numbers, so old Field and icon objects must not be carried across it. PDF Undo/Redo restores PDF state;
it does not serialize or rewind arbitrary JavaScript variables. Globals are not saved into the
PDF. A worker fault resets its context; the existing cooperative source/loop/recursion bounds
remain in force. The source preflight additionally caps active arrow chains at 256 and right-associative
assignment/exponentiation chains at 4,096 across open bracket levels. Independent statements
and comma-separated sibling expressions reset only their own level; comparisons do not count.
Keyword unary/new chains share the prefix
depth bound of 64. These conservative limits protect recursive parser/compiler paths before
execution on the smaller worker stack. Boundary and refusal cases run in subprocess regressions.
These bounds do not establish a hard JavaScript heap quota.

`calculateNow()` evaluates the current calculation list synchronously, in `/CO` order, using
pending writes. Later statements in the same invocation see the new values. The caller's
event is restored after calculation, including failure. Reentrancy is capped at eight levels.
Field writes and their dependent recalculation use the existing single undo transaction.
Pure caption, geometry, style and visibility updates do not run the calculation chain.
Page geometry is read once per edit runner. Calculation sources/order are analyzed once on its
first callback and reused; field values and appearance properties still refresh on every callback.
Structural/document changes between batch children refresh the snapshot, and editing a field's
calculation script or calculation order invalidates the cached plan before recalculating.
`numPages`, `documentFileName` and `info` read the current event snapshot, including page/info edits,
Undo/Redo and completed Save As, without reinitializing script globals.

## Fields, appearances and icons

- `buttonGetCaption()` reads the first widget. An unindexed `buttonSetCaption()` updates every
  child widget, including when the first already has the requested text. An unchanged uniform
  caption does not create an edit. Only normal face `0` is implemented.
- `rect` reads/writes rotated user-space `[left, top, right, bottom]`. Writes affect widget 0,
  as specified for an unindexed Field. Conversion uses its page rotation and effective Crop
  box. Small/zero-area rectangles remain representable, rather than applying the interactive
  field-authoring minimum size to a script's overlay.
- `textColor`, `fillColor`, `strokeColor`, `textSize`, `lineWidth`, `borderStyle`, `userName`,
  `readonly` and `display` flow to the form model and saved appearances. Colour input supports
  `T`, `G`, `RGB` and `CMYK`; text cannot be transparent. The existing appearance editor writes
  its canonical RGB representation. Partial changes preserve unrelated fonts and properties.
- `buttonGetIcon()` returns a host-minted object for a normal Form XObject icon. `buttonSetIcon`
  accepts only a handle minted in the same context, or null to clear it. It reuses the form and
  its resources, including transparency, instead of rasterizing a signature/background. The
  existing button appearance generator draws it. Other icon faces are refused.
- `getPageBox` reads effective/inherited page boxes in rotated user space. `zoom`,
  `app.runtimeHighlight` and `setFocus()` communicate with the native viewer. Automation
  reports viewer requests and does not open an external application or change a real viewer.

## Native menus and input

`app.popUpMenuEx` supports nested menus, separators, marked/disabled items, explicit return
values and cancellation. `app.response` supports named and positional parameters, defaults,
password masking and null cancellation. Alert replies use the documented numeric button ids.
Only the worker waits for a reply; egui continues drawing/input. The central modal gate blocks
document shortcuts and page input while a prompt is visible. Idle prompt polling uses a
one-second repaint interval; replying wakes the UI immediately and worker polling remains 16 ms. A reply resumes the same
invocation, without replaying the script or its side effects.

An explicit button captures document identity, generation and view geometry. Closing/switching
the document, editing it, disabling scripts or changing the relevant view discards a stale
completion and invalidates the old runtime. A reply is checked against an enabled menu leaf
and its enabled ancestors; invented/disabled return values fail with a script error. Prompts
have at most 1,024 entries, 16 submenu levels, 4,096-byte strings and 32 calls per invocation.
The native desktop admits one pending button interaction. A document worker refuses a second
run while the first is active, including while waiting for a prompt; it never queues a
synchronous field callback behind an unanswered dialog.

The native prompt UI is not implemented for the web build. The existing headless/unsupported
host path cancels unanswered popups and response dialogs; no web-interaction parity is claimed.
This work does not add timer execution, PDF annotations/layers to the JS object model, or the
remaining document action triggers.

## Automation

The existing `js_run` accepts `answers`, an optional array of up to 32 strings or null values:

```json
{
  "doc": 1,
  "script": "var t=app.popUpMenuEx({cName:'Manual',cReturn:'manual'}); if(t==='manual') getField('time').value=app.response({cQuestion:'Time'});",
  "answers": ["manual", "07:45"]
}
```

The schema advertises a 4,096-character maximum per answer; the handler additionally enforces
a 4,096-byte UTF-8 budget, so multi-byte strings can reach the byte limit earlier.
Replies are consumed in prompt order. Null/missing replies cancel a menu/response; alert
defaults choose acknowledgement, cancellation or No according to the button set. No direct
file, process or network API is exposed by a prompt. Root confinement and permission-checked
document edits continue through the existing automation/engine boundary.

## Related contributions

The persistent runtime uses the upstream work from #5304 for synchronous calculations,
#5501 for page boxes, #5499 for colour parsing and writeback, and #5505 for normal captions.
The integration supplies the live document state, exact field identity, and grouped field
selection needed by the persistent context.

The bounded `popUpMenuEx` parser in `crates/js/src/interaction.rs` was adapted from #2081.
The prompt channel and default menu renderer support interactive form scripts. #2081 can
supply an anchored renderer through the UI menu callback when it adds link actions. Once
the shared parser lands upstream, this integration can use it directly.

`ScriptSession::run_host_task` gives future timer and page actions (#4751) access to the
retained realm on its own worker.

Document actions and widget triggers (ported from #344 by @DWAA1660) run through the same
realm. On opening, the document scripts run, then the catalog `/OpenAction`'s JavaScript
and its `/Next` chain (event `Doc/Open`); Revert runs them again in a fresh realm.
`Session::run_document_actions` runs `/AA /WS` before a save and `/AA /WP` before printing:
the app's Save and Print and the `doc_save` and `doc_print` tools call it first. A failing
action is reported and never stops the save or print. `Session::field_trigger` runs a
widget's Mouse Enter, Mouse Exit, Mouse Down, Mouse Up, Focus or Blur JavaScript; the app
queues them from the pointer and the focus, and `js_event` runs them headlessly. A check
box's or radio button's Mouse Up runs after its click has stored the new value (#247).
Scripts that run unasked (open, Will Save, Will Print, pointer over, focus) may only ask for
highlighting, zoom, focus or a page.

The lexical preflight recognizes unambiguous top-level and block-body ASI boundaries before identifiers, including declarations and assignments separated by line breaks or comments. It preserves counters after operators and member access, and across binary keyword operators. It distinguishes control-statement parentheses for regexp literals; inside expression parentheses, array/object literals and template interpolations it remains conservative; this is not a complete ECMAScript parser or ASI implementation.

Native persistent workers are capped process-wide at four on 32-bit targets and sixteen on 64-bit targets. Each reserves 64 MiB of stack; closing a document releases its slot after the worker exits. Capacity errors ask the caller to close a scripted document and retry; other documents are never silently evicted. The transient stateless runner has its separate existing stack policy.

Synchronous runs have one ten-second host deadline covering startup and every prompt round trip. A timeout replaces the document realm; late globals and custom Field properties cannot reach later runs. This is isolation, not forcible termination of an OS thread: cooperative engine limits still govern the detached worker, whose capacity slot stays charged until it exits. No hard CPU or heap quota is claimed.

An asynchronous completion stays reserved until the engine accepts it. An intervening edit forks a fresh realm before any field calculation, including when the edit later fails. Pending results carry runtime identity as well as generation, so discarding an old result never clears a newer realm. Script prompts render last and explicitly raise their retained egui layer, including when another app dialog opens later. Null prompt answers cancel popup/response input; alert answers instead select the button set's default (OK, Cancel or No).

`resetForm()` updates the live field shadow synchronously, including retained Field references and named parent groups. Reset requests still reach the PDF engine for default-value and appearance semantics; the atomic writeback performs resets before applying the final script delta. Delta comparison uses the reset baseline, so a later script assignment restoring the original pre-reset value is preserved. Doc Open and the first button/console event share this ordering, including after JavaScript is disabled and re-enabled. PDF Undo/Redo and save/reopen preserve the final values.

The reset comparison baseline is obtained by running the existing forms reset path on an isolated document copy. It therefore includes unnamed built-in Calculate text/combo dependents, without executing custom Calculate scripts during the preview. Normal automatic calculation after script value writes remains in force; this does not establish a policy that explicit formula-field writes override their formulas. Tests cover saved defaults left untouched by the event and a non-default typed native response as well as existing default/cancellation paths.

Reset writeback is admitted before request copies, preview cloning and PDF mutation only when it fits 64 reset calls, 4,096 aggregate selectors, and 2^24 estimated work units. Named resets pay for the bounded selector scan and the fields actually matched. The estimate charges both preview/commit passes, field/calculation/dependency scans, calculation source lengths and selector scans/bytes. Excess work returns an explicit error without committing PDF edits or an undo entry. Under-budget requests retain their order; duplicate resets are not silently merged because repeated recalculation can be observable. This is an operation/amplification bound, not a hard UI wall-clock deadline. Native start/completion tests cover the 64-call boundary, 65/10,000-call refusal, selector overflow, aggregate large-form refusal, reset/restore, Undo/Redo and save/reopen.

The reset admission estimate also charges field-name copies, selector comparison bytes, and the worst-case expansion of each named reset to all fields (including both validation and membership scans). This conservative bound can refuse a named reset even if its eventual selection would be small; full-form resets avoid that named-selection cost. Refusal tests compare saved bytes as well as generation and undo state.

Reset admission includes the total widget count for preview and commit. The shared JavaScript string-array converter also enforces a 32 MiB aggregate escaped UTF-8 text budget before copying beyond it (including scalar input), independently of later request clipping. Exact Unicode and unpaired-surrogate output bytes are charged; the existing array-item limit remains. JavaScript ToString/getters may still execute script code before conversion; this is not a general JavaScript heap limit.

Unknown PDF defaults (failed stream decoding or a spent shared value budget) are marked in the JavaScript snapshot. A reset preserves their live values, matching the forms reset policy; explicit assignments before/after reset remain effective. The policy follows inherited defaults too. Menu and response properties now use the bounded escaped UTF-8 conversion at 4096 bytes per property before copying past that limit; user conversion exceptions are preserved. Existing item/depth/prompt/reply limits remain separate. Immediate dependent reads can use resetForm(...); calculateNow(); the reset callback timing itself has not been changed without an Acrobat parity oracle.

The 64-reset call budget is now enforced in the worker before argument conversion and field scanning, shared by document initialization and its first event, and reset for later jobs. A sticky outcome flag rejects PDF writeback even if JavaScript catches the exception. The existing engine admission/work guard remains. Calculations are resolved by PDF object reference to the matching snapshot position; event targets and duplicate-name value and property deltas retain object identity through the host, authoring helpers and redraw. Public getField(name) retains first-match behavior. Tests cover both calculation orders, target reads/writes, native/console completion, Undo/Redo and save/reopen.

AFSimple_Calculate expands exact/group/duplicate field matches without materializing their cross-product; numeric conversion and empty-result fallbacks remain unchanged. Selector comparison work is admitted up to 2^24 estimated units. The Field cache uses canonical PDF identities (name-only keys for synthetic callers), prunes removed fields between jobs and covers every exposed field, including more than 4096; renames preserve retained references/custom properties. Script color changes detach outside widgets that share the target group's indirect MK, including bounded reference aliases, while keeping target widgets' indirect object and unrelated MK entries. Native button clicks carry exact identities through hit testing, action dispatch and event construction. Legacy name-only engine/UI entry points remain wrappers. Headless form_fields returns field_id; js_run accepts that optional 48-bit identity with a matching field name, validates it before execution, and retains deterministic prompt replies. IDs are document-local and must be rediscovered after structural changes.

Field instance tokens live outside PDF undo snapshots. Successful edits and undo/redo retire
removed field instances immediately, even when no script job observes the gap. Reusing the
same PDF object number does not revive an old Field accessor; document globals survive history.
A refused synchronous/native writeback invalidates its document realm before returning an error.
The host-backed event accessor remains usable when globalThis is frozen. AFSimple_Calculate
shares a 2^24 selector-work budget across all calculation sources in an event job; later jobs
receive a fresh budget. Native simple calculations include every selected list export value,
and calculateNow uses the regular value setter's checkbox/radio normalization.

Native UI feedback distinguishes a refused busy click, a stale interaction and a timeout.
Timeouts report discarded results without claiming worker termination. Visible comparison halves
may complete; switching away, changing the view or editing still invalidates stale interactions.
Focus reveals a laid-out widget's page, and image import carries exact clicked identity through
the picker and SetFieldImageTarget. The headless form_set_image tool accepts the same optional
field_id and validates exact name/identity pairs; omitted IDs preserve legacy lookup. Mixed widget icons emit field-wide changes only when an
icon setter runs. UI waiting helpers use elapsed deadlines rather than machine-speed retry counts.

Comparison script document requests use their source view. Save As captures the source document without changing the comparison tab. Print activates and retains the source tab because the existing print dialog reads the active document throughout its lifetime. Missing source views never fall back to another active document. Regression coverage distinguishes a two-page source from a one-page comparison file and checks saved bytes, print-dialog page state and untouched comparison content.

Explicit calculation logs ordinary field script errors and continues in calculation order. Each job can evaluate at most 4,096 calculation scripts, including nested calls and document initialization on the first event. Exhaustion remains in effect after a JavaScript catch and prevents PDF writeback.
