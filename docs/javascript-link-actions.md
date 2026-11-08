# JavaScript link actions

PdfCraft inspects `/Subtype /Link /A /S /JavaScript` without rewriting the action dictionary.
`/A` and `/JS` may be indirect objects. `/JS` may be a PDF string or a decoded stream;
the inspection reader caps decoded bytes at 256 KiB and uses the common PDF text decoder.
Malformed, cyclic and oversized operands remain unsupported actions, never executable source.
Rejected stream decodes are cached by resolved stream identity within one inspection. The FIFO
cache retains at most 4,096 identities and no decoded data; eviction permits retrying an older
failure. Failed decodes spend none of the source budget, so later valid sources remain available.
The retained link sources have a shared 4 MiB UTF-8 budget across the document, so repeated
references cannot multiply a small action into unbounded memory. Reaching that budget records
one inspection warning and leaves subsequent JavaScript actions unsupported; ordinary links
and the original PDF action dictionaries are preserved.

## Execution path

The renderer exposes `LinkTarget::JavaScript(source)`. The canvas keeps the link's page and
raw annotation index and asks the engine to prepare immutable `LinkScript` input. The engine
captures the source document ID and generation, document information, form fields and named
document scripts. Reading a document does not require an editor; field changes still require
the existing editing permissions.

The native script worker owns its Boa context throughout one action. Document scripts initialize
that context under `Doc/Open`; the action then runs under `Link/Mouse Up`, with `event.target`
equal to the document, `event.targetName` empty, and `pageNum` initially the link's page.
The link event does not use `event.rc` as a cancellation result.

Initialization is part of the explicit Link transaction. If the whole action succeeds, its
field changes and eligible requests are retained with the action's effects, subject to the same
permissions, generation check and single Undo step. Fresh contexts rerun initialization on every
activation, so an initializer guarded by `event.name === "Open"` can replace saved field values
when a link is clicked.

An initializer that is refused or throws stops the Link action; its edits and external requests
are not committed. Ordinary field, button and console initialization retains upstream's separate
policy: document-script errors are reported while later initializers and the requested script
can continue. Rebuilding definitions for those ordinary events disables initializer field
writes, page changes, menus and viewer requests at the host boundary, and uses a separate
event object. Reads see the current saved/edited state throughout bootstrap, not temporary
defaults that would later be rolled back. Functions and globals are still evaluated; calling
those functions from the requested event can perform legitimate field edits and requests.
Console diagnostics and alerts remain available under the existing reporting policy.
An explicitly requested Doc/Open retains initializer effects; no automatic Open is added.
All paths retain the aggregate initialization bound. When XFA layout owns the form,
the upstream runner omits PDF fallback scripts from initialization; it keeps them in the file.

`app.popUpMenuEx` sends a plain menu tree to the frontend and waits only on the worker thread.
The frontend polls between frames and returns a selection or cancellation over a reply channel.
The same Boa invocation resumes: no replay of the action, no source transformation and no
special-case handling of document generators. `Function.prototype.apply` works normally.

## Desktop context menus

The canvas captures the activation position in egui viewport points before starting the worker.
The desktop host uses egui 0.37 `PopupKind::Menu`, anchored below/right with automatic
flipping and viewport constraints. Root menus and submenus have independently measured widths,
capped at 340 points; their scroll areas are capped at 420 points and the available height.
The menu has no dialog title or modal overlay. Labels wrap, disabled entries cannot activate,
marked entries show a check, and separators and nested submenus retain their PDF values.
Arrow keys navigate enabled entries, Right/Enter enter submenus, Left leaves, Enter/Space select,
and Escape cancels. Enter/Space require an existing keyboard selection, painted as a
selected row; neither key chooses an invisible first entry, and hover alone is insufficient.
While the menu owns these keys, Space is consumed and page pan is suspended; arrows first
establish the selection. Mouse clicks still activate their own row. Menu-owned focus is
released when the interaction ends.

JavaScript annotation hover tips disappear in the activation frame and stay hidden while
that interaction is running or queued, including between popup replies. Ordinary link and
comment tips remain available. Canvas hover tips are pointer-transparent, so their current
or previous-frame rectangles cannot intercept clicks intended for a menu or the document.
Signature validation badge tips also remain available during a JavaScript menu interaction.

An outside press cancels a menu still awaiting a choice without consuming the pointer event:
the canvas, another annotation or an interface control receives that same press/release.
Before the first menu request, a running action also survives outside interaction, loss of
focus, Escape, dialogs and view changes. These events have no menu to dismiss; normal
document shortcuts remain available. A later menu starts a new wait using the current view
and can still be cancelled normally, including while the window is unfocused or blocked.
After a choice is accepted, the action keeps running through outside interaction, loss of focus,
Escape and view changes. A new link activation waits for that accepted action; only the latest
queued activation is retained, and it must still match its captured document generation to start.
A new explicit activation may still supersede an unchosen action, including one still running
before its first menu. This policy is separate from incidental view or focus changes.

Cancelling a waiting menu, including a second popup in the same action, cancels the whole action.
Every retired menu receives null; later menus are cancelled and completion is discarded, including
field changes and external requests. A completion arriving while a menu still awaits a choice
(for example after the reply timeout) is discarded too. If the worker advances to another popup
without a UI reply, the whole action is retired and both menus receive null. Completed jobs are removed; retired
receivers are only drained, never applied. Each invocation retains its immutable engine identity
and generation checks. Frame identity prevents multi-pass layout from starting an activation twice.

At most three live execution channels are retained. If all slots are occupied, the latest
activation waits for a worker to finish. Logical polling does not paint or consume keys;
the UI pass alone draws and handles menu input. Before document shortcuts, a read-only
ownership check reserves the waiting menu's arrows, Enter, Space and Escape, even before any
item gains keyboard focus. Outside interactions retain their normal document shortcuts;
menu cancellation observes raw events even when those shortcuts have consumed them.
Closing/switching documents, editing or disabling
JavaScript invalidates running and accepted results as well as waiting menus. While awaiting a choice, view
changes, another dialog, loss of focus, outside scrolling, Tab and command shortcuts cancel the
interaction. Menu scrolling stays inside the menu; outside events remain available to the reader.

The opt-in control channel reports `javascript_menu` in `ui.state`, with source document/page/
annotation, captured anchor, visible rectangles, execution sequence and live channel count.
It also reports window focus. This adds inspection only; no transport or authentication changes.

## API and limits

Menu entries support `cName`, `cReturn` (including the empty string), `bEnabled`, `bMarked`,
separators (`cName: "-"`), and `oSubMenu` as an object or array. Missing `cReturn` falls back
to `cName`. Cancellation returns JavaScript `null`. Replies are validated against enabled
leaves of the exact menu tree before resuming the interpreter.

Menus are limited to 1,024 entries, 16 submenu levels, 4,096 bytes per label/return value,
and 32 popup calls per action. Each reply wait expires after five minutes. Dropping the host
or its reply channel releases a waiting worker. Existing Boa limits remain: one million
loop iterations, recursion 256, bounded parser nesting, and a dedicated script stack.
These are cooperative limits, not a general heap quota, hard wall-time deadline or ability
to terminate a worker. The public worker API has no admission limit; the three-worker cap
belongs to the desktop host. An already received menu must release its own reply sender
or receive cancellation when its host abandons the action.
Document initialization additionally permits at most 1,024 scripts totalling 1 MiB.
These aggregate limits also apply to fields, buttons and the console. Refusal preserves the
input value and reports a diagnostic to the host; field calculation does not open a modal alert.

`pageNum` requests navigate the source document's view. `app.launchURL` uses the existing
document URL allowlist and explicit consent flow. `app.openDoc(cPath, oDoc?)` and
`app.openDoc({cPath: ..., oDoc: ...})` return a file-opening request and JavaScript `null`,
never a live document or navigation proxy. The optional base may be the current document
(`this`, or the Link event's `event.target`); omitted, undefined and null keep that same base.
Other objects, including lookalikes and proxies, other file systems and unsupported positional
options are refused. This does not grant access to another document or remember permission.

On desktop, the UI asks consent even for relative paths. No file is read by Boa. The desktop
resolver normalizes Windows `/C/path` and `/C:/path` forms to `C:/path` and rejects URL and
network paths. Its filesystem validation, PDF extension, regular-file, size and password
checks still apply. Headless requests are untrusted data, not paths validated by this resolver:
their consumers must validate the path, enforce their own root/access policy and obtain
consent before opening anything.
On desktop, if a file-opening or URL consent prompt is already pending, another script document-opening
request is ignored with a translated notification asking the user to respond first and retry.
The existing prompt is preserved; the ignored request is not queued and no file is opened.
Implicit field scripts (Calculate, Format, Validate and Keystroke), including their document
initializers, cannot request opening another document: the engine drops `OpenDoc` requests
and records a console diagnostic, as it already does for SaveAs and side-panel requests.
Explicit Link and Button actions retain their document-opening requests and consent checks.

The engine rechecks JavaScript enablement and the source generation before applying results.
The frontend cancels a pending menu on document close, edits, undo/redo or JavaScript disablement.
Form changes and resets use the existing single undoable batch. A failed link script does not
commit form changes or dispatch its requests; its errors and diagnostic output remain visible.
If applying a successful script is refused (for example by form permissions), its console and
messages are retained, the edit error is shown, and none of its viewer requests is dispatched.

## Headless access

`link_run_javascript {doc, page, index, menu_choices?}` uses one-based `link_list` numbering.
Pass the exact `page` and `index` returned by `link_list`: `index` is the position in that
page's `/Annots` array, not a row number in a filtered list of links; gaps are possible.
It returns `menus`, `alerts`, `console`, `requests`, and `error`.
Following the upstream message contract, `alerts` contains objects with `message`, `title`
(null if absent), `icon` and `buttons`, as `js_run` does, rather than plain strings.
This is an output-shape change for callers of the earlier #2081 branch. Titles and buttons
are retained through the desktop message queue; alert responses still return OK upstream. `menu_choices` is an optional
array of `cReturn` strings or null, consumed in nonempty popup order, maximum 32 entries.
A call with no menu entries returns null without requesting interaction or consuming a choice.
Disabled-only and separator-only menus remain nonempty. The 32-call interactive budget still
counts empty calls; their removal from the interaction stream does not remove that limit. Omitted choices
or explicit null cancel only that popup and return JavaScript `null`. The script then resumes;
it may change fields and issue viewer requests, and a successful completion applies its edits
in the usual Undo batch. This differs from desktop cancellation of a waiting menu, which retires
the whole action and discards its effects. Invalid selections fail closed. External URL/file
requests are returned to the caller and never executed; consequently this tool does not read
paths outside `--root`. Returning a request does not authorize a later read outside that
root: a consumer must validate it and obtain consent before acting on it.

The native tool returns a tool error after ten seconds without a worker event. This timeout
applies separately to each receive, not to the whole action or its synchronous preparation.
No result from the timed-out action is applied. Abandoning the receiver does not terminate
the worker: it may continue running, so callers must handle the error and must not assume
that retrying stopped the earlier worker. Existing cooperative VM limits still apply.
The tool is marked destructive in MCP because scripts can change fields or reset the form.
That hint does not grant permissions; the engine still checks all edits and batches them for Undo.

Engine APIs are `Session::prepare_link_javascript`, `LinkScript::start` (native),
`LinkScript::run` (headless cancellation), `Session::link_javascript_is_current`, and
`Session::finish_link_javascript`. Source identity and execution input are immutable.

## Compatibility boundaries

This extends the existing Acrobat subset rather than implementing the entire Acrobat host.
Named document scripts are initialized once per fresh context; globals are not persistent between
actions. Link initialization uses Doc/Open, then Link/Mouse Up with a document target. Existing
field, button and console calls initialize under their actual event, without a synthetic Open.
Field targets remain fields; legacy events without a target use null. The Link global event
binding is non-enumerable so its document target does not make event serialization cyclic.
Catalog OpenAction JavaScript and document/page JavaScript lifecycle actions, `/Next` action
chains, other annotation triggers, named JavaScript destinations and
all other Acrobat APIs remain outside this implementation. `app.openDoc` returns `null`
immediately, not a live Doc object: opening is deferred to consent, and subsequent script access
to the opened document is unsupported. Hidden/password/options behavior is not emulated.
Native link menus are supported on desktop; the web UI explicitly reports them unavailable.
Existing synchronous console and button execution cancels popup requests instead of displaying
the native link menu. These boundaries must not be represented as complete Acrobat compatibility.

Ordinary PDF opening destinations, named destinations and `/XYZ` coordinate placement retain
the upstream navigation behavior, including #4376. Reading an opening destination does not run
an OpenAction script. Upstream additions such as `util.scand` and `app.execMenuItem` remain
available; headless execution returns viewer requests as data without dispatching external I/O.

## Verification

Synthetic tests cover direct/indirect strings, compressed streams, UTF-16BE, malformed and
oversized operands, reference cycles, menu cancellation, nested/disabled items, forged replies,
continuation without replay, events, read-only documents, stale generations, disablement, and
save/reopen preservation. The optional ignored engine test
`external_pdf_javascript_actions_read_only` takes `PDFCRAFT_JS_READONLY_PDF`, exercises up to
ten links and checks the original file remains byte-identical. It never saves that file or
includes external content in the repository. UI tests live in `js_menu::tests`; they assert
single-click replacement, actual canvas activation at different zoom/DPI settings, placement,
cancellation, stale completions, bounded rapid activation, keyboard/submenus and pending consent
without opening a target file. `PDFCRAFT_SHOT` and `PDFCRAFT_MENU_SHOTS` optionally save private
synthetic test renders. The contributor reports successful Windows testing with a KiCad 10 PDF before the rebase.
Headless renders do not replace native validation of the rebased build.

Running-state regressions hold a real engine completion at the UI channel boundary, rather
than timing a slow script. They check both logic and UI passes, delivery exactly once, one-step
Undo, visible exceptions without edits or requests, first-menu transitions, document invalidation,
channel cleanup and explicit activation supersession. The original tests fail on the uncorrected
Running lifecycle before the phase guards are changed.

The 2026-10-23 rebase passed 2,698 targeted tests. An isolated portable Windows run also
verified annotation activation, the live JavaScript popup, the Next choice, navigation to page 2
and normal exit through the opt-in UI control channel, using a contributor-original synthetic PDF.

Public API behavior source: [Acrobat JavaScript API Reference](https://opensource.adobe.com/dc-acrobat-sdk-docs/library/jsapiref/JS_API_AcroJS.html#popupmenuex).
No implementation code or assets from Adobe products were used.
