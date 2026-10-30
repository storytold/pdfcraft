# Open a PDF at a requested destination

The desktop launcher accepts a physical page or a named PDF destination after the filename.
This lets another application open its manual at the requested help topic.
Quote the complete argument when its path contains spaces:

```sh
pdfcraft "manual with spaces.pdf#nameddest=chapterTwo"
pdfcraft "manual.pdf#page=2"
pdfcraft "manual.pdf#page=2" "appendix.pdf#nameddest=index"
```

`page` is a physical page number starting at 1, independent of printed page labels.
`nameddest` identifies a destination stored in the PDF, including its supported position and zoom.
These parameters follow [RFC 8118 section 3](https://www.rfc-editor.org/rfc/rfc8118.html#section-3).
Several parameters can be separated by `&` or `#` and are applied from left to right.
An unresolved later destination leaves the last successful view in place and reports a notice.
Other parameters in a recognized launch fragment are ignored; fragment parameters do not run scripts, import form data or open another URL.

## Paths and escaping

Plain filesystem paths retain literal percent and hash characters.
Only the filesystem path is made absolute; navigation names are never normalized as paths.
An existing complete filename containing `#` opens as a literal filename.
When a navigation suffix follows such a filename, the longest existing file prefix wins.
A plain path suffix must contain `page=` or `nameddest=` to be recognized as navigation; other hash suffixes remain literal path characters.

Local `file://` URIs separate the fragment before decoding the filename.
Use `%23` for a literal hash in a URI filename:

```sh
pdfcraft "file:///home/me/manual%23draft.pdf#page=2"
pdfcraft "file:///C:/Docs/manual%20with%20spaces.pdf#nameddest=chapterTwo"
```

Percent-escape fragment delimiters inside a destination name: `%26` for `&`, `%23` for `#`, and `%25` for `%`.
Values are decoded once, and `+` remains a literal plus character.
URI-encoded text names use UTF-8; lookup also recognizes the PDF's supported text-string encodings.
An exact raw key takes precedence over an alternative text encoding.

On Windows, Linux and BSD, a second launch can hand the request to the running app.
Each file keeps its own destination and the sending process's working directory.
Navigation survives background preparation, password retries and focusing an already-open tab.
The recent-files list and saved document path contain the actual filename, without the navigation suffix.
Opening or navigating does not edit the PDF.

File pickers, Apple-event file paths, session paths, `pdfcraft-cli` document tools and the UI control channel's `ui.open` continue to accept literal filesystem paths.
They do not interpret launcher suffixes.
Use the desktop launch command for this workflow; the opt-in UI control channel can inspect its result with `state` and `screenshot`.

## Limits

A launch argument is limited to 1 MiB and its decoded absolute path to 128 KiB.
The decoded parameter values are limited to 64 KiB in total, with allowance for their percent-encoded representation, and the parameter count to 64.
An ambiguous plain argument is limited to 64 hash boundaries; a local file URI can escape literal filename hashes.
A destination name is limited to 1,024 UTF-8 bytes.
Malformed page numbers and invalid or oversized names are refused with a readable error.
Missing or out-of-range destinations leave the document open and report a notice.
On-demand named lookup has depth, traversal, comparison and decoding budgets; an incomplete search does not choose a different fallback destination.
Ordinary document inspection retains readable destinations after malformed tree entries and reports a warning.
The named-destinations editing panel and other PDF opening parameters remain separate work.
