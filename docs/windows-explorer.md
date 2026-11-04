# Windows Explorer actions

The MSI registers classic context-menu actions. Windows 10 shows them in the normal
context menu; on Windows 11 use **Show more options**. Portable copies do not alter Explorer.
No file-type default is changed, and uninstall removes the registrations. The actions belong to
the installer's existing optional menus: the PDF ones to the PDF menu (`INSTALLPDFCONTEXTMENU`,
next to "Edit with PdfCraft"), the image and text ones to the image menu
(`INSTALLIMAGECONTEXTMENU`), so leaving a menu out leaves its actions out too.

- **Convert to PDF with PdfCraft** (`.txt`/`.text`) opens each selected text file as a new,
  unsaved PDF. Choose Save to write it. Images keep **Create PDF with PdfCraft…**, which stages
  the selection in one PDF and asks for the page DPI. Supported sources for the actions below
  are PNG, JPEG, TIFF, GIF, BMP, JPEG 2000, and `.txt`/`.text`.
- **Combine files in PdfCraft…** collects selected sources and PDFs in the existing sortable
  Combine tab. Review order, warnings and page ranges before combining. Explorer may launch
  one process per selected file; subsequent requests append to the same review list.
- **Convert to PDF and email…** collects sources or PDFs in an explicit attachment review list.
  **Save email draft…** asks for a parent folder, then creates a uniquely named `PdfCraft-email-…`
  subfolder containing `draft.eml` and the separate PDF attachments. Existing files and source
  documents are never overwritten. Failed conversion or a cancelled chooser leaves the pending
  selection intact; a source arriving during the chooser remains for the next draft.

The email action opens the saved `.eml` with the registered desktop mail handler. The draft uses
MIME attachments and `X-Unsent: 1`; it has no recipient and PdfCraft never sends mail. Support
for editing `.eml` drafts varies by client/version. If a client opens it read-only, create a new
message and attach the PDFs from the saved folder. **Outlook.com requires this manual attachment
step**; PdfCraft does not sign in to Outlook.com or send private files to it automatically.
The draft and PDFs stay on disk after the client closes or fails to open.

Email conversion, MIME encoding and writing run in a background worker. Cancel retains the source
selection and removes incomplete output. Cancellation is cooperative between individual conversions
and write chunks; a currently blocked filesystem call or single conversion cannot be interrupted.
The window remains usable while that call finishes. Only one email worker runs at a time. If the
bundle was already committed when Cancel arrived, it remains available and the mail client is not
opened. The existing Convert/Combine import path still processes each bounded source synchronously.

The current converter does not import Word, Excel or PowerPoint files; those extensions and
Windows `.lnk` shortcuts are not registered. There is no modern Windows 11 `IExplorerCommand`
COM extension in this change. Menu text is a localized executable resource, selected by Windows'
UI language with neutral English fallback, rather than fixed by the installer language.
The labels are built from the craft-i18n checkout the interface embeds (`CRAFT_I18N_DIR`, `locales/<code>/pdfcraft.tsv`): every UI language whose catalog has all three labels gets a table (at the pinned revision, all 34 translated UI languages, including Catalan, Central Kurdish, Korean, Lao, Thai and Vietnamese). A build without the checkout, like its interface, has English labels only.

Explorer requests are limited to 100 paths; IPC is bounded to 1 MiB and file reads to 64 MiB per
source. Explorer conversion/combination checks a 128 MiB aggregate PDF-source budget across the
open documents and pending Combine rows, including later requests. Rejected Combine sources stay
visible as failed rows. This limits retained source buffers, not the renderer's entire memory use.
Email preparation separately limits converted attachments to 128 MiB total. A mail provider may impose
a lower attachment limit. No debounce timer decides that a selection is complete: users confirm
Combine and email lists themselves. A failed IPC handoff starts a separate window, as ordinary
file opening already does. Requests arriving during a dialog or a background document open wait in order until that work
finishes, so an existing Rotate/Redact/Export action keeps its original document and a late open
cannot replace the incoming Combine workspace. The deferred queue
is separately bounded to 100 paths and 1 MiB of path text; additional requests report an error.

For automation, `doc_create_multiple` supplies conversion/combination and `doc_email_draft`
prepares the same durable draft bundle without opening a mail app:

```json
{"tool":"doc_email_draft","args":{"paths":["scan.png","notes.txt"],"out_dir":"."}}
```

Run with `pdfcraft-cli run --root DIR`; source and output paths remain confined to that root.
The result contains `path`, `attachments` and `sent: false`.

Tests use synthetic text/PDF data, real bounded Windows IPC and an injected mail-client callback.
`packaging/windows/test-shell-build.ps1` compiles a tiny metadata-only MSI and reads its registry
table without installing it. CI also requires executable resources and checks all localized
string tables. No test modifies this user's registry, sends mail or launches a mail client.

Public Windows references: [selection models](https://learn.microsoft.com/windows/win32/shell/how-to-employ-the-verb-selection-model),
[localized verbs](https://learn.microsoft.com/windows/win32/shell/fa-verbs),
and [modern Explorer integration](https://learn.microsoft.com/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer).
