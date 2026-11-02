# AcroForm script workflows

This contribution connects the existing AcroForm object model to the native viewer and the
shared automation tools. The regression fixture is generated in Rust; it contains no private
document, employee, signature image or real PIN.

## Runtime and lifecycle

Each open document owns a `ScriptSession`. On native targets its Boa context lives on one
worker thread. Document scripts initialize once with `Doc/Open`, before the requested event;
button, console, format, validate and calculate scripts then share that context. Field objects
read the latest snapshot supplied by the engine, including objects retained by earlier scripts.
The first 4,096 field objects are cached by name. Separate documents cannot share globals or
icon objects. `eval` and dynamic Function construction remain disabled.

Changing the document-script sources recreates the context. Closing, reopening, reloading,
Revert and disabling JavaScript release/reset the context. PDF Undo/Redo restores PDF state;
it does not serialize or rewind arbitrary JavaScript variables. Globals are not saved into the
PDF. A worker fault resets its context; the existing cooperative source/loop/recursion bounds
remain in force. These bounds do not establish a hard JavaScript heap quota.

`calculateNow()` evaluates the current calculation list synchronously, in `/CO` order, using
pending writes. Later statements in the same invocation see the new values. The caller's
event is restored after calculation, including failure. Reentrancy is capped at eight levels.
Field writes and their dependent recalculation use the existing single undo transaction.
Pure caption, geometry, style and visibility updates do not run the calculation chain.

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
Only the worker waits for a reply; egui continues drawing/input. A reply resumes the same
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

Replies are consumed in prompt order. Null/missing replies cancel a menu/response; alert
defaults choose acknowledgement, cancellation or No according to the button set. No direct
file, process or network API is exposed by a prompt. Root confinement and permission-checked
document edits continue through the existing automation/engine boundary.

## Related contributions

The synchronous calculation bridge is based on PR #5304, page boxes on #5501, partial colour
writeback on #5499, and normal captions on #5505. The bounded menu parser was adapted from
#2081. The combined per-document runtime, property/icon bridge and native button interaction
integration are developed here, with related state/icon work also tracked in #344 and #4751.
Those contributors' branches are not modified by this contribution. See the PR description
for the exact imported heads and validation actually performed on the combined source.
