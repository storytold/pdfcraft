# PdfCraft user guide

[Deutsch](usage-de.md) · [All languages / translate](README.md) · [Project README](../../README.md)

English is the source text; German is the initial example translation. Further languages can
follow the same structure (see the language index).

Start here for everyday PDF workflows, keyboard shortcuts and links to detailed instructions.
English UI labels are retained so commands can be found in the menus or command palette, even if
your interface uses another language. Checked against source revision
[1dd2737](https://github.com/storytold/pdfcraft/tree/1dd2737b47a9c1c9562f0af563053d1bf510e4bf)
on 2026-10-27. A downloaded release may differ.

## Contents

- [Open, save and find tools](#open-save-and-find-tools)
- [Read, search and copy](#read-search-and-copy)
- [Organize, combine and split](#organize-combine-and-split)
- [Comments, forms and editing](#comments-forms-and-editing)
- [More tools and output](#more-tools-and-output)
- [Keyboard reference](#keyboard-reference)
- [View and context controls](#view-and-context-controls)
- [Settings, limitations and further help](#settings-limitations-and-further-help)

## Open, save and find tools

1. Use **File › Open…** to select a PDF, or choose a recent document on Home. Enter the opening
   password when requested. Author-imposed restrictions can disable copying, printing or editing;
   **File › Security properties…** explains the document's permissions.
2. Choose a task in **All tools**, or press **Ctrl+K** (**Cmd+K** on macOS) and search for a command.
   The palette accepts English names, translated names and command IDs. A tool marked as still in
   development is not a completed workflow.
3. Save changes with **File › Save**. Use **Save as…** for a separate output file. Unsaved tabs have
   a dot; closing asks what to do with unsaved changes. Recovery is offered after an unexpected exit.

Installation, available downloads and building from source are in the [README](../../README.md).

## Read, search and copy

Use the page-number field, page thumbnails or Bookmarks panel to navigate. Search bookmark titles
to find nested entries. Under **View › Page display**, choose single-page/two-page layout and
continuous scrolling. For a continuous strip, choose Single page, enable Continuous scrolling and
turn off Show gaps between pages. This removes the viewer's spacing, not white margins in the PDF.
See [page display](../page-display.md) for examples and defaults.

Press **Ctrl+F / Cmd+F**, enter text and step through matches with **Ctrl+G / Cmd+G** or the
previous-match shortcut. **Advanced search…** opens the broader search interface.
Choose the Select tool (**V**), drag across text, then **Ctrl+C / Cmd+C** to copy extracted text.
Double-click selects a word. Copying depends on PDF permissions and available text; a page that is
only an image needs **Scan & OCR** before its words can be searched or copied.
**Edit › Column select** (or Alt/Option-drag with the Select tool) restricts selection to a rectangle.

Use the Hand tool (**H**) to pan, or hold Space temporarily while dragging. **File › Word count…**
counts the document or current page; the [word-count guide](../word-count.md) explains exclusions
and scanned pages. Copying PDF text does not imply preservation of a word processor's rich formatting.

## Organize, combine and split

1. Open **Pages › Organize pages** to see the page grid.
2. Select a page, Ctrl-click/Cmd-click to add individual pages, or Shift-click a range.
3. Use the page controls to rotate, delete, insert or move the selection. **Delete** removes the
   selected pages; **Ctrl+A / Cmd+A** selects all pages while the grid owns the keyboard.
4. Review the resulting order and save. **Undo** reverses an edit; **Redo** restores it.

**File › Combine files…** joins PDFs into one document. **Extract** copies selected pages into
another document; **Split** divides a document by page count or split points. Use the resulting
file/output options to choose where the new documents are written. Existing command-line recipes
for these tasks are in [CLI batch operations](../cli-batch.md).

## Comments, forms and editing

| Task | Steps / entry point | Result and limits |
|---|---|---|
| Comment on a PDF | Find **Add a sticky note** or a markup tool in the palette, then click/drag on the page; inspect **Comments panel** | Adds annotations; it does not rewrite the original text |
| Fill an existing form | Click its text fields, checkboxes or lists; use **Form fields panel** to inspect values, then save | Requires fillable fields and the document's fill permission |
| Create form fields | Choose **Prepare a form**, then a field tool or **Detect form fields**; inspect field properties and tab order | Creates interactive fields; inspect detected fields before distributing the PDF |
| Add text to a flat form | Find **Fill & Sign: add text** and place the text on the page | Adds a visible annotation rather than an interactive field |
| Edit existing text | **Edit a PDF › Edit text & images**, then click a text region and edit it | Font/glyph availability and PDF structure can limit edits; follow [editing existing text](../editing-existing-text.md) |
| Move or resize a figure | In **Edit text & images**, select the figure, drag it or a corner; use its context menu for rotation | Some figures are grouped artwork; see [image editing and restrictions](../editing-images.md) |
| Move several objects | In **Edit text & images**, modifier-click objects or draw a selection rectangle, then drag the group | One page at a time; see [multiple-object editing](../editing-multiple-objects.md) |

Save after editing. For a copy that leaves the source file intact, choose **Save as…** first.

## More tools and output

The palette provides these entry points. Follow each dialog's options and review the output;
this overview does not promise full compatibility with every PDF.

| Task | Entry point and next action |
|---|---|
| Recognize a scan | **Recognize text…** in Scan & OCR; choose the pages/options, run recognition and inspect the searchable text |
| Remove sensitive page content | Mark with **Redact text and images**, then **Apply redactions…**; marks alone do not remove content. Review the saved result |
| Remove hidden information | **Remove hidden information…** or **Sanitize document…**; inspect what will be removed before applying |
| Password protection | **Protect using password…**; set the dialog's protection options and save. Inspect **Security properties…** afterward |
| Visible signature/initials | **Fill & Sign: sign** or **Fill & Sign: initials**; place the appearance. These are distinct from a certificate-based digital signature |
| Digital signatures | **Digitally sign**, **Signatures panel**, **Validate all signatures**; use the certificate workflow and inspect reported validation status |
| Accessibility | **Check for accessibility…**, then inspect the report and relevant tagging/alternate-text tools |
| Export | **File** provides image, text, Word, PowerPoint, HTML and RTF export commands; choose a destination and inspect fidelity |
| Print | **File › Print…**; choose pages and printer settings, then print (desktop) |
| Reduce size | **File › Reduce file size…** or **Optimize PDF…**; choose options and inspect the resulting copy |

For implemented scope and limitations, use the [file-format reference](../file-format-parity.md)
and [known gaps](../gaps.md). Office conversions, advanced signatures and standards workflows
have incomplete coverage; their presence in a menu is not a guarantee of professional parity.

## Keyboard reference

The table lists all **25 default shortcut entries** from the
[command registry](../../crates/engine/src/commands.rs). Additional view/tool keys follow below.
**Ctrl** means Control, **Cmd** means Command (⌘), and **Shift** is ⇧; Alt is Option (⌥) on macOS.
Slashes separate alternatives. Function keys may require Fn. Native platform columns are shown;
browsers and the operating system may intercept keys, and punctuation depends on keyboard layout.

Most command shortcuts work outside text editing; Undo/Redo act on a focused text editor instead
of the document. Modal dialogs own their keys. Disabled commands still require their document,
selection or permissions. Open **Help › Keyboard shortcuts** for the in-app reference; it is a
reference dialog, not a shortcut-assignment editor.

| Command | Windows / Linux / BSD | macOS |
|---|---|---|
| Open… | `Ctrl+O` | `Cmd+O` |
| Save | `Ctrl+S` | `Cmd+S` |
| Save as… | `Ctrl+Shift+S` | `Cmd+Shift+S` |
| Close file | `Ctrl+W` | `Cmd+W` |
| Close all | `Ctrl+Shift+W` | `Cmd+Shift+W` |
| Print… | `Ctrl+P` | `Cmd+P` |
| Document properties… | `Ctrl+D` | `Cmd+D` |
| Undo | `Ctrl+Z` | `Cmd+Z` |
| Redo | `Ctrl+Shift+Z` | `Cmd+Shift+Z` |
| Find… | `Ctrl+F` | `Cmd+F` |
| Advanced search… | `Ctrl+Shift+F` | `Cmd+Shift+F` |
| Find tools and commands… | `Ctrl+K` | `Cmd+K` |
| Fit visible | `Ctrl+3` | `Cmd+3` |
| Full screen mode | `Ctrl+L` | `Cmd+L` |
| Read mode | `Ctrl+H` | `Cmd+Ctrl+H` |
| Next tab | `Ctrl+Tab` | `Ctrl+Tab` |
| Previous tab | `Ctrl+Shift+Tab` | `Ctrl+Shift+Tab` |
| Previous view | `Ctrl+[` | `Cmd+[` |
| Next view | `Ctrl+]` | `Cmd+]` |
| Go to page… | `Ctrl+Shift+N` | `Cmd+Shift+N` |
| Show or hide the side panel | `F4` | `F4` |
| Show or hide the Tools panel | `Shift+F4` | `Shift+F4` |
| Preferences… | `Ctrl+,` | `Cmd+,` |
| JavaScript console… | `Ctrl+J` | `Cmd+J` |
| New bookmark | `Ctrl+B` | `Cmd+B` |

## View and context controls

In this section **Mod** means Ctrl on Windows/Linux/BSD and Cmd on macOS. Click the document
before using view keys; an active text field, form field, palette or dialog can take precedence.

| Context | Keys / gesture | Effect |
|---|---|---|
| Search | Mod+G / Mod+Shift+G | Next/previous match, including while the Find input has focus |
| Text selection | Mod+C; double-click | Copies selected text; selects a word |
| Selection | Mod+A | Selects document text, pages in Organize, or fields in Prepare a form; a focused text editor selects its own text |
| Selected picture | Mod+C | Copies the picture as a file if permitted; web users need Save Image As |
| Desktop document, clipboard contains an image | Mod+V | Adds the image to the current page; text fields, Organize and Prepare a form have their own paste behavior |
| View | Mod+1 / Mod+0 / Mod+2 | Actual size / fit page / fit width |
| View | Mod+Plus or Mod+Equals / Mod+Minus | Zoom in/out; in Organize this changes thumbnail size instead |
| Organize pages | Mod+0 | Resets thumbnail zoom |
| View | Mod+Shift+Plus or Mod+Shift+Equals / Mod+Shift+Minus | Rotates the view clockwise/counterclockwise; this is distinct from rotating document pages |
| View | Pinch; Ctrl-scroll | Changes zoom |
| Navigation | Home / End | First/last page |
| Navigation | Left / Right; Mod+PageUp / Mod+PageDown | Previous/next page |
| Navigation | PageUp / PageDown | Scrolls by a screen in continuous view; changes page in noncontinuous view |
| Navigation | Up / Down | Scrolls by a line; can turn a single page at its edge |
| Full-screen presentation | Space or Enter / Backspace | Next/previous page |
| Free document canvas | V / H; hold Space | Select / Hand tool; temporarily pan until Space is released |
| Selected pages in Organize | Delete | Deletes the selected pages |
| Selected objects / annotations / prepared fields | Arrow keys | Nudges the selection when that tool owns the keys, rather than navigating pages |
| Edit text & images | Shift/Ctrl/Cmd-click; drag empty space | Toggles objects in the selection; draws a selection rectangle |
| Selection / search / full screen | Escape | Clears/cancels the nearest active context; can close Find or leave full screen |

Sources: [view navigation](../../crates/ui-egui/src/canvas/navigation.rs),
[app key routing](../../crates/ui-egui/src/app/keys.rs),
[in-app shortcut reference](../../crates/ui-egui/src/dialogs.rs),
[clipboard](../../crates/ui-egui/src/app/clipboard.rs),
[image copying](../../crates/ui-egui/src/canvas/image_pick.rs),
[image paste](../../crates/ui-egui/src/tools/content_ui.rs),
[multiple-object guide](../editing-multiple-objects.md).

## Settings, limitations and further help

**Edit › Preferences…** (Ctrl+, / Cmd+,) controls application settings, including interface
language and default document/view options. The Tools panel can sit on either side; drag a panel's
inner edge to resize it. **View › Reset side panels** restores the default arrangement.
F4 toggles the side panel, Shift+F4 the Tools panel.

Desktop and web differ: the checked web build does not provide printing, reopening a closed
file-backed tab or revealing its file in the system file manager. It cannot put an image file
on the system clipboard; use **Save Image As**. Browser shortcut interception and clipboard
permissions can also affect operation. Consult the existing references for detailed support:

| Topic | Documentation |
|---|---|
| Page layouts and spacing | [Page display](../page-display.md) |
| Text, images and groups | [Text editing](../editing-existing-text.md), [images](../editing-images.md), [multiple objects](../editing-multiple-objects.md) |
| Counting text | [Word count](../word-count.md) |
| Interface language | [Localization](../localization.md) |
| Startup problems | [Windows renderer startup](../windows-renderer-startup.md), [development/logs](../development.md) |
| Automation and batch work | [CLI batch](../cli-batch.md), [MCP](../mcp.md) |
| Compatibility and unfinished work | [File formats](../file-format-parity.md), [UI coverage](../ui-parity.md), [gaps](../gaps.md), [roadmap](../roadmap.md) |

This initial handbook covers core workflows, all default registry shortcuts and the view/context
controls listed above. It is not yet an exhaustive tutorial for every tool or dialog. Detailed
walkthroughs for signatures, accessibility remediation, OCR options, optimization, advanced forms
and the remaining dialog/tool gestures still need to be expanded and checked. Existing linked
pages remain authoritative for their details and are currently mostly English. Roadmap entries
are plans, not instructions for features already available.

Contributors: update the English guide and existing translations with user-facing changes and verify the relevant
keys and links; see [AGENTS.md](../../AGENTS.md#user-documentation).
