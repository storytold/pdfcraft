# Scripted character sheets

MPMB v13.2.3+241220 (Colorful A4) uses a retained JavaScript realm, large rule
libraries, executable rules stored in form fields, custom dialogs and hidden page
templates. The supplied document and its scripts are private verification inputs;
none is included in this repository.

## Runtime support

Document libraries have a 1 MiB individual limit and an 8 MiB combined limit,
with at most 1,024 libraries. Action and field scripts retain their 256 KiB limit.
Retained document sessions admit dynamic `eval` and `Function` source up to 8 MiB,
with 16 MiB of compilation per event. Parser depth, active expression chains,
loop and recursion guards still apply. Stateless runners refuse dynamic code.
These bounds do not establish a JavaScript heap quota.

The lexical guard distinguishes bracket types and known expression starts while
following ambiguous regular-expression/division readings conservatively. Flat
conditional statements no longer consume a lifetime nesting budget. `toSource`
preserves script functions, regular expressions, dates and errors; native
functions, cycles and oversized graphs are refused. Replacement callbacks retain
the legacy empty-string behavior for unmatched regex captures described in
[Mozilla's Firefox 34 release notes](https://developer.mozilla.org/en-US/docs/Mozilla/Firefox/Releases/34).
Custom metadata retains its spelling (`info.SheetVersion`).

`app.execDialog` runs initialize, control callbacks, validation, commit and destroy
on the document worker. The native viewer draws bounded control descriptions,
including lists, hierarchical lists, text, checkboxes, radio controls and ARGB
icon streams. Disabled/hidden controls reject forged events. Missing automation
answers cancel; callbacks are not replayed to collect answers. Document toolbar
buttons, progress and cooperative cancellation share that worker.

Scripts may request a selected UTF-8 text stream with
`util.readFileIntoStream()` and `util.stringFromStream()`. An explicit script
filesystem path is refused. Automation supplies a file only through an explicit
root-confined `answers` entry; the native viewer uses its file picker.

Named `/Pages` and `/Templates` entries retain hidden `/Type /Template` pages.
Scripts can spawn renamed or shared fields, show/hide templates, and remove
spawned pages. New pages and fields are visible within the same script. Changes
replay through the engine as one permission-checked undo transaction. Bookmark
properties, field groups through `getArray()`, and `Field.page`/`submitName` follow
the current PDF graph. Scripted default values persist and are used by `resetForm`. The
`calculate` switch suspends automatic calculation until a script or timer resumes
it; `calculateNow()` remains explicit.

## Performance and qualification

Hidden storage widgets and widgets on hidden pages do not need display
appearances. Skipping those reduced a locally observed no-JavaScript open from
about 51 seconds to under one second, and appearance generation from roughly
61 seconds to 0.2 seconds. Document library sources and template field-name lists
are shared between callbacks; field handles use indexed identity lookup. Repeating
an unchanged field assignment leaves the shared snapshot intact instead of copying
all fields and hidden storage. Large native operations retain a bounded 30-second
shared deadline; small forms retain 10 seconds.

The supplied sheet initialized without document errors or import-failure alerts,
with one imported add-on, 213 races, 21 classes and 806 spells. The final local
run opened it in 8.3 seconds. Timer-result writeback completed in about 1.2 seconds;
previously it exceeded its callback budget after 43 to 61 seconds. Calculated values
are committed before formatting against one final snapshot, avoiding a full form
read for each Format callback. Hidden-value writes invalidate stale appearances
without drawing their potentially large storage strings.

A temporary copy passed Save As, a subsequent incremental save, close and reopen.
It retained Strength 20, modifier 5, the High Elf field text, Champion 3 with the
fighter/champion class state, and an added notes page with its entered notes
(seven pages). A newly selected, contributor-original rule script survived that
round trip and its function executed successfully. Both imported scripts remained
registered; the original add-on retained its 3,816,638 UTF-16 code-unit length.
Removing all notes pages, saving incrementally and reopening retained five pages,
removed the copied notes field, and kept both imported scripts. All these steps
completed without script errors or import-failure alerts. The original document
was never saved over.

The class dialog recognized Fighter and level 3 through successive control
callbacks. Source selection and Layout toolbar controls were exercised through
the same runtime. This qualification covers those workflows, rather than every
class, race, spell combination or arbitrary third-party add-on.

A synthetic desktop document was opened with the opt-in control channel. Its
registered toolbar button opened a custom dialog; editing and confirming it,
saving, and reopening retained the entered value. Only this synthetic document
was used for the UI screenshot.

Synthetic regressions cover library limits, persistent state, serializer and
parser limits, dialog lifecycle, icon streams, selected-file confinement,
calculation suspension/timers, bookmarks, template fields, undo and save/reopen.
See `crates/automation/tests/it/automation.rs` and the JavaScript/forms/organize
unit tests. Every new action is available through the automation tool table.

General Acrobat JavaScript parity remains partial. Overlay template spawning is
not implemented; the observed sheet calls use `bOverlay=false`. Editing branched
template name trees is refused to preserve their child metadata. The web runtime
does not provide the native asynchronous dialog/file-selection host. Arbitrary
third-party add-ons may require additional APIs or exceed the documented bounds.

## Importing from another sheet

The import dialog uses a text field's `browseForFileToSubmit()` to select a
source PDF. The field exposes a temporary selection path to the script. Passing
that path to `app.openDoc` opens the selected source in the same invocation.
The native viewer uses its PDF picker. For `js_run` and `js_tool_button`,
supply `{"pdf":"source.pdf"}` in `answers` at the picker position. The path is
resolved under the automation read root. The source must be a regular AcroForm
PDF up to 64 MiB that permits copying. Cancelling a picker clears any earlier
selection in the same invocation.

The source realm exposes metadata, fields, templates and document functions.
Its field and display changes live in memory for the duration of the script.
The destination reset also uses indexed widget rectangles and lets document
scripts update the appearance of controls that are locked in the form editor.
The engine applies the destination changes in one permission-checked undo
transaction. A cancelled picker or an unhandled transfer error discards staged
destination edits. The source is never saved. Opening a script-supplied path
outside the picker flow still uses the existing viewer request.

The built-in toolbar workflow transferred the character name, Strength,
Dexterity and maximum hit points between temporary copies of the supplied
v13.2.3 Colorful A4 sheet. All four values survived save and reopen. Cancelling
the picker left the destination alone, and the source and original files
remained byte-identical. The successful copies had the pre-existing large
add-on removed. With that add-on present, final initialization exceeded the
existing dynamic compilation budget and the engine discarded the transfer.
The version rejection and permission-denied writeback cases use synthetic forms.

Custom-image import uses cross-document button icons and, for some paths,
temporary source form edits and saves. Those operations are outside this
workflow. Leave the sheet's custom-image option off.

## Importing and exporting form data

The sheet can select an FDF or XFDF file through `importAnFDF()` or
`importAnXFDF()`. Imported values are visible to the rest of the script
immediately. The import and the sheet's follow-up field edits commit together
through the engine's existing permission checks and undo history. Cancelling
the picker discards the invocation's staged form edits.

`exportAsXFDF()` opens a save picker. `exportAsXFDFStr()` returns the same field
encoding directly. Both read the live form values, accept an `aFields` selection
including field groups, and honor `bAllFields` and `bNoPassword`. These calls
exchange form values only. Imports ignore annotations and embedded actions.

For `js_run` and `js_tool_button`, supply `{"form_data":"input.fdf"}` or
`{"form_data":"input.xfdf"}` at the import picker position in `answers`.
Use `{"export_xfdf":"output.xfdf"}` at the save picker position. These selections
are checked against the respective automation read and write roots. The sheet
asks for confirmation before import. Answer `"4"` (Yes) before the picker
answer. A null answer at the picker cancels it. Scripts must omit `cPath`.
A string export can use `cHRef` to set the XML's file reference. A selected input
or exported XFDF may be up to 8 MiB.

Local verification used temporary copies of v13.2.3+241220 Colorful A4. The
sheet exported all fields, equipment, and description after synthetic text
was entered. The equipment and description files contained strict subsets of
the all-fields export. The string API also accepted the settings the sheet
built for each mode. The sheet's FDF and XFDF import workflows computed a
derived modifier that was omitted from the selected input. Answering the
sheet's confirmation and then cancelling either picker preserved the checked
values. The imported values and modifier survived saving and reopening separate
PDF copies. One undo restored the pre-import values after importing the full
XFDF field set. A repeat import into that copy survived saving, closing, and
reopening. The synthetic regression also covers undo, rejected writeback, and
read and write selections outside the permitted root.
