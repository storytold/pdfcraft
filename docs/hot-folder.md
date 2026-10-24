# PdfCraft Hot Folder

**Status: implemented native workflow, with isolated task workers and durable restart recovery. Platform package acceptance is documented separately. Proprietary ABBYY task/project formats are not compatible.**

**Optimization integration status:** the shared full-save writer's generation-identity correction
from [#1143](https://github.com/storytold/pdfcraft/pull/1143) is backported here from
`da54b14f085c930350d9b23839bd404cf4c3a625` (COS implementation and five synthetic regressions only).
The additional `hot_folder_optimization_never_revives_stale_page_content` automation regression
exercises Off/Lossless/Reduce/Custom publication, preserving the original and live extension data
while preventing stale page references from acquiring replacement content.
The MSVC linker is installed. All seven Hot Folder automation tests pass on the integration
with main `b5467d93`, with the pinned OCR models present. A real CLI run preserves the original
and identical rendered pixels across all four modes: Off retains 66,104 bytes, while each
optimization profile publishes 832 bytes for the synthetic stale-reference fixture. This is a
regression check, not a general compression benchmark. Final package acceptance remains separate.
The strict Windows symlink test cannot create its fixture locally (OS error 1314); GitHub's
Windows job requires this test rather than silently treating an unavailable fixture as coverage.

At integration commit `116cd60c`, local Windows gates pass: `cargo test --workspace --locked -j 2 --no-fail-fast -- --test-threads=2` (3,173 passed, 15 ignored, 0 failed), strict workspace/all-target Clippy, formatting, assets, layering, parity and all 32 WASM package checks. OCR models are present; optional craft-fonts-dependent cases remain unexercised.

The follow-up integration includes main `3f044b62` (native Windows printing) and its resolved dependency lockfile. Targeted Windows tests pass: 22 print tests, 369 editor library tests (one ignored), and all seven Hot Folder automation tests. Strict workspace/all-target Clippy, formatting, assets, layering, parity and all 32 WASM package checks also pass on the follow-up. The full 3,173-test result above belongs to `116cd60c`; it is not a second full-suite run on the printing follow-up.

The 2026-10-14 follow-up integrates main `dc4d86ba`. Its safe server-test harness passes on Windows and Linux after correcting native path separators and case comparison; the previous helper fails the same Linux run. Selected automation, font, content and editor tests pass (1,385 passed, five ignored), plus 271 renderer tests. Print-dialog tests use synthetic printer sources after reproducing a crash during concurrent native printer enumeration; all eleven assertions pass in parallel with those fixtures. This does not certify native printer drivers.

The 2026-10-15 integration incorporates main `d3a91c5c` and version 0.8.0. It retains the Hot Folder D-Bus build input while enabling the upstream qpdf/Poppler requirement in Linux CI. Local targeted tests pass: 164 automation/Hot Folder tests, 374 editor library tests (one ignored), and 83 support-tool tests. External oracle coverage is enforced by Linux CI; optional craft-fonts coverage is not claimed for the local run.

The optional standalone application processes folders without keeping the PdfCraft editor open.
It shares OCR, orientation correction, deskew and paper-edge detection with the engine. Its
interface follows the primary system language when English, German, French, Dutch, Spanish,
Norwegian Bokmål or Swedish is available, otherwise English. The document language defaults
independently from the system language, using the shared OCR language list.

## Current processing contract

- Create named tasks with separate existing input/output folders. Local folders and network
  folders mounted as filesystem paths use the same scanner; credentials are managed by the OS.
- FTP with explicit TLS (or explicitly selected plain FTP), Outlook attachment folders and open
  PDFCraft project manifests use a separate local staging folder. Sources are downloaded atomically;
  output receipts precede source acknowledgement. Interrupted acknowledgement retries do not rerun
  OCR. Project manifests retain their specified file order. No implicit FTPS or SFTP is advertised.
- Choose PDFs, PNG/JPEG/TIFF/BMP images, optional subfolders, start time, once or recurring checks,
  and how long a file must remain unchanged before processing. No recursive symlink traversal.
  Daily, weekly and monthly schedules use local wall-clock time. A missing month day uses that
  month's last day; a DST gap uses the next valid local minute, and repeated times run once.
- Configure resolution, language, existing-text skipping, recognition, rotation, straightening,
  paper-edge cropping and safety padding. Optional blank-page removal is deliberately conservative:
  only completely white rendered pages without extracted text are removed; documents with forms,
  links or annotations are kept. An entirely blank input requires a separate blank-page archive.
- Configure up to sixteen saving steps: processed or image-only PDF, DOCX, RTF, HTML, text,
  PNG, JPEG, TIFF, XLSX, EPUB or a JSON analysis report. Group per file, per containing folder or across a complete settled run.
  File/date/time/task/folder/counter templates control names. Each conversion is staged before
  publishing. Existing names receive numbered suffixes; byte-identical retries reuse results.
  Original move/delete follows success of every saving step and its durable receipt. A failed
  grouped batch requires an explicit retry, so missing inputs cannot silently disappear from it.
  Receipts retain the hashes of the actual staged export bytes. Missing or replaced outputs stop
  source cleanup, including recovery after a crash; older receipts without proofs cannot authorize
  deletion. Unix publication synchronizes containing directories before originals are changed.
- PDF saving steps can optionally optimize the final PDF after OCR, corrections and grouping.
  The default is **Off**. Lossless retains image encoding/quality, text, tags, forms and links;
  Reduce uses the shared 150 ppi/JPEG quality 60 profile; Custom exposes separate color/grayscale
  resolution thresholds, Retain/ZIP/JPEG compression and JPEG quality. Downsampling and JPEG
  re-encoding may lose image detail. Only a strictly smaller result replaces the processed PDF
  bytes; equal/larger candidates are discarded. Existing input files are never overwritten.
  Signed inputs are refused before page selection, rasterization or grouping can remove their
  signatures. Cancellation or optimization failure publishes no partial optimization result and
  never authorizes source cleanup. Explicit Keep/Move/Delete policies still apply after all
  outputs and receipts succeed. Image-only PDF and rasterizing scan settings are separate,
  potentially destructive conversions; choosing Lossless does not undo those earlier choices.
- Select one-based page ranges and save recognition-area templates in task JSON. Regions use
  displayed top-left percentages, include or exclude OCR words, and leave visible content intact.
  Optional grayscale, inversion, brightness, contrast, black/white threshold, median noise reduction
  and splitting spreads rasterize selected pages at the requested resolution (bounded by renderer
  limits). Interactive forms/links/annotations are refused for rasterizing image changes.
- Analyze-only mode writes versioned JSON reports with existing text, geometry, image rectangles and metadata. It bypasses OCR, scan corrections and blank-page removal, and supports the selected page range. An analysis saving step after normal processing describes the resulting PDF; the analyzer itself never runs OCR.
- Optional blank-page archives preserve removed pages as separate PDFs; originals are not removed
  until these files also save successfully. An entirely blank input requires an archive destination.
- Durable receipts suppress duplicate processing across worker restarts. Transient filesystem,
  publication and task-process failures retry automatically with bounded exponential backoff
  (5 seconds up to 15 minutes); one offline task does not block the others. A malformed unchanged
  input is quarantined until its contents change or Retry/Start is requested. A permanently failed
  group stays blocked rather than silently omitting one member. Stop is checked between pages
  and does not save partial OCR output. Editing a running task through automation or its configuration
  file also cancels the previous settings snapshot, including a change from Delete to Keep.
- Import/export task JSON, duplicate tasks, inspect the last 1,000 log events, filter failures and
  open output folders. Importing never enables tasks automatically.

The scanner caps input batches at 512 MiB/128 files, enumeration and journal entries at 10,000, nesting at 32,
configuration/state JSON at 16 MiB and tasks at 32. These bounds produce actionable errors.
Files must be closed by their producer before processing; for slow/scattered writes increase the
settling time or have the producer rename the completed file into the input folder.

## Agent and service interfaces

`pdfcraft-hot-folder --config tasks.json` opens the separate configuration application.
`pdfcraft-hot-folder --worker --config tasks.json` runs the worker without a window.
`pdfcraft-hot-folder --once --config tasks.json` executes one scheduler pass and prints JSON.
`pdfcraft-hot-folder --service --config tasks.json` hosts the installed system service.
`--service-action status|start|stop|enable|disable|install|uninstall` explicitly administers it.
Windows `prepare-account` reapplies service SID/recovery policy without changing the SCM account.
Unix installation additionally requires `--account USER`, a non-root service account.
Unix processing refuses root; elevated execution is reserved for setup and administration.
`--initialize-config --config PATH` atomically creates a stopped configuration without replacing
an existing file. Unix new directories/files are owner-only. Run it as the service account.
The GUI uses an installed matching service rather than launching a competing worker. Windows
installation/removal uses the optional MSI feature; runtime APIs never install it implicitly.
The worker holds an OS lock so only one process owns a configuration, and each engine pass takes
a second lock shared with agent calls. It does not open a network listener or an MCP server.

`hot_folder_configure`, `hot_folder_status`, `hot_folder_poll` and `hot_folder_retry` expose the
same engine through `pdfcraft-cli run` and opt-in MCP. All nested paths honour `--root`, including
inputs, every saving/blank archive folder, configuration and sidecars. Poll processes one stable
file or complete bounded batch per task per due interval; it never silently starts a background worker.
`hot_folder_run_now` overrides the next calendar occurrence. `hot_folder_service` exposes explicit
service administration; mutations are refused in any filesystem-confined session. Hot Folder task, source and project
tools currently refuse separate `--automation-read-root` / `--automation-write-root` settings;
use the supported shared `--root` confinement instead.
`hot_folder_project` writes an ordered open manifest without replacing existing files.
`hot_folder_source` provides credential status/removal, FTP password storage from a named environment
variable, Outlook device sign-in and folder listing. Secrets never appear in tool results or task JSON.

Each PDF `output_plan.steps` entry accepts an `optimization` object. It is omitted or
`{"mode":"off"}` for the original behavior. Modes are `off`, `lossless`, `reduce` and `custom`.
For example, a task can retain an optimized PDF and a separate text export:

```json
{
  "output_plan": {
    "steps": [
      { "format": "pdf", "optimization": { "mode": "lossless" } },
      { "format": "txt" }
    ]
  }
}
```

Custom settings use `color` and `gray`, each accepting `downsample` (boolean), `ppi`,
`above_ppi`, `compression` (`retain`, `zip`, `jpeg`) and `quality` (1–100). Resolution must
be finite, 9–2400 ppi, with `above_ppi >= ppi`. Defaults are 150 ppi above 225 ppi,
JPEG quality 60. Retain keeps the existing image encoding, while generic lossless stream
cleanup remains enabled. With downsampling on, Retain keeps the compression family
(resampled JPEG uses quality 80); choose ZIP for
lossless encoding of resampled pixels or JPEG for an explicit quality setting. Custom otherwise uses the lossless cleanup
policy, retaining optional structures and links. Optimizing non-PDF saving steps is rejected.
Analyze-only mode bypasses optimization as it does the other processing settings.

`hot_folder_poll` and `hot_folder_status` expose `tasks[task_id].last_optimization` for the
last successful publication batch: `documents`, `changed`, `bytes_before`, `bytes_after`.
The standalone UI shows the same actual sizes. Counts exclude steps with optimization off;
unchanged candidates have equal before/after values. Engine `hot_folder_output::save` also
returns per-file `optimizations`, including the final collision-safe path and image counts.
These figures compare against the processed PDF immediately before optimization, not the
original scan or Office/image source. Blank-page archives are kept without this optimization.

## Remote credentials and mail handling

The service uses a separate protected `secrets` directory next to its configuration. Windows
installation restricts this to SYSTEM, Administrators, LocalService and the fixed service SID.
A separately configured SCM service account is validated by its actual OS SID, not a task-JSON name. Unix requires the service
account as owner and permissions 0700 for the directory and 0600 for files. Unsafe permissions,
symlinks and missing provisioning are rejected instead of falling back to an unprotected file.
Set up credentials under the service identity, or use an administrator task-manager session
bound to the existing protected service configuration. Unix administrative writes retain the
non-root service account's UID/GID and private file modes. Importing
tasks transfers only credential IDs, never passwords or tokens. The engine also has an explicit OS-vault
API for callers running in an interactive user session; the boot service does not depend on a login keychain.

Outlook uses Microsoft Graph device authorization with your own public-client application registration.
Keep requests Mail.Read; moving/deleting requests Mail.ReadWrite; offline_access and User.Read retain
the session and verify mailbox identity. Open the displayed Microsoft sign-in URL and enter its code.
The application lists folders after authorization. It processes PDF, PNG, JPEG and TIFF attachments;
cloud-link attachments and attached messages are not treated as documents. Move and Delete apply to
the whole message only after all supported attachments succeed, so enable both PDF and images.
Delete moves messages to Deleted Items, never permanently deletes them. Sources with unsupported or
failed attachments are reported and retained. Server revisions, attachment hashes and mailbox identity
are rechecked before any remote move/delete.

Example task (folders must already exist):

```json
{
  "tasks": [{
    "id": "school-scans",
    "name": "School scans",
    "input": "/scans/in",
    "output": "/scans/out",
    "enabled": false,
    "recursive": true,
    "interval_seconds": 10,
    "settle_seconds": 3,
    "originals": "keep",
    "settings": { "language": "de", "auto_crop": true }
  }]
}
```

`tasks.state.json` keeps the active receipts/observations bounded to 256 entries and 2 MiB
per map; logs keep at most 1,000 events and 1 MiB. Exact historical records spill to sharded
files under `tasks.journal`, so keeping more than 10,000 originals does not stop the queue.
Disk history grows with each distinct source version; provision disk space accordingly.
`tasks.state.pending.json` is the atomic write-ahead snapshot: recovery exposes all grouped
receipts together even if the process died during history spill. Output names and PDF
metadata use the persisted attempt timestamp, making replayed OCR/export byte-identical.

`tasks.lock` and `tasks.worker.lock` are OS locks, released automatically after a crash.
Corrupt JSON is reported without resetting or overwriting it. Back up the configuration,
state, pending state and complete journal together. Do not remove individual history files
to free space: doing so discards deduplication evidence. Configuration, journals and worker
limits belong on local storage; input/output folders may be network shares.

The parent service delegates each due task to a bounded child process. The default is
300 seconds and 4096 MiB per child; see [installation and limits](hot-folder-installation.md).
Windows enforces a Job Object commit-memory limit and kills children if the parent dies.
Unix samples resident memory; it does not promise an equivalent kernel commit limit.
An idle pass does not rewrite unchanged state. Updating one manifest cannot newly overlap
another active task's source or output paths.

## Functional reference and remaining scope

The clean-room reference is ABBYY's [Hot Folder documentation](https://help.abbyy.com/en-us/finereader/16/user_guide/hotfolder/).
No ABBYY assets or code are included. A processing scheduler and task settings are the relevant
functional behaviors; this document does not claim compatibility with ABBYY project/task files.

The functional inventory and its implementation boundaries are:

| Requirement | Current state |
|---|---|
| Windows/macOS/Linux service at system boot | Service host and explicit administration integrated; separate optional installers have platform lifecycle tests |
| Once / recurring interval and local start time, daily/weekly/monthly | Implemented; each due run drains its stable input queue before scheduling the next calendar occurrence |
| Local and OS-mounted network folders | Implemented |
| FTP and Outlook sources | Authenticated adapters, atomic staging, guarded acknowledgements, device sign-in and folder selection implemented |
| Project source | Ordered open PDFCraft manifests implemented; proprietary ABBYY project formats are not supported |
| OCR / scan corrections / exact searchable PDF | Implemented through existing engine |
| Saved region templates / preprocessing | Page selection, include/exclude regions, grayscale/inversion/threshold/brightness/contrast/median filter and spread splitting implemented; bounded read-only JSON analysis reports available |
| Blank-page removal and separate blank-page archive | Implemented with conservative white-only detection and durable archive receipts |
| Multiple save steps/formats | Eleven document/image formats plus versioned JSON analysis, multiple saving steps, XLSX table cells/images and EPUB book structure implemented |
| Group output by file, folder or entire task | Implemented with a 128-file/512-MiB batch bound and staged publication |
| Import/export, duplicate, stop, retry, logs and failure filtering | Implemented for native task JSON |
| Seven requested UI languages | Complete label catalogs and system-language fallback implemented |

Do not run a privileged root/LocalSystem worker against configuration editable by unprivileged
users. System-service installation must establish an appropriately restricted account and
configuration ownership. Installing the optional component must not process any folder until
a user explicitly enables a task.
