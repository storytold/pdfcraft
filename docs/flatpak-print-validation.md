# Flatpak Print Portal validation (#1905)

> **Last reviewed:** 2026-10-23 · **Last updated:** 2026-10-23 · **Change:** minor (refresh against main; original Linux evidence retained)

## Environment and baseline

Validated on CachyOS Linux, KDE Plasma/Wayland, xdg-desktop-portal 1.22.1 and
xdg-desktop-portal-kde 6.7.5, with Freedesktop Platform 25.08.18. No physical printer or
CUPS queue was required. Flatpak was initially absent; local, unprivileged Flatpak 1.18.4
and the runtime were installed for this validation.

The baseline was upstream/main `924c4da4ac2879ac9107c6c82e20ac6be5570f0a`.
An actual sandbox had neither `lp`, `lpstat`, `lpoptions`, `/etc/cups` nor
`/run/cups/cups.sock`. Native Linux uses craft-print's unchanged CUPS implementation;
inside Flatpak its empty discovery list previously left only Save as PDF.

The regression `flatpak_offers_system_printing_without_cups` was run against an archive
of that unchanged revision, with only the regression test added. It compiled and failed
because the System print dialog option was absent. The same test passes with the fix,
including selecting Save as PDF and writing a valid PDF. Its environment runs in a child
process, so no test mutates environment variables shared with other tests.

## Workflow selection and actual handoff

The first implementation evaluated direct `Print`. The KDE backend returned response 2
immediately without opening a dialog; this was observed on D-Bus, not inferred from a
missing window. The implementation therefore uses `PreparePrint` and then `Print` with
its token. Only PreparePrint opens a host dialog. The response token/settings are checked
before Print receives the already-composed PDF. This also needed KDE's observed `reverse:
"no"` spelling of false, covered by the mock service tests. A method reply or a successful
PreparePrint alone never becomes a printing-success notice.

The modified desktop app and CLI were packaged, installed and launched in a real Flatpak
sandbox with exactly the manifests' existing finish-args:

```
shared=ipc
sockets=fallback-x11;wayland
devices=dri
filesystems=xdg-documents;xdg-download
```

No network, CUPS socket, host filesystem or host-command permissions were added. Both
manifests parse identically to the base; their changes are comments only. The host's print
portal is reached through Flatpak's normal session-bus proxy.

The native Linux app opened the synthetic PDF and displayed its existing CUPS/Save as PDF
dialog over the opt-in control channel. The Flatpak opened the same four-page synthetic
PDF, offered System print dialog despite an empty printer list, and opened KDE's native
Stampa dialog. Copies, duplex, collation and Properties were disabled/delegated in PdfCraft;
PDF layout/preview controls remained usable. Control inspection remained responsive while
the host dialog was pending.

The synthetic source contains four original coloured rectangles, without fonts or images.
PdfCraft selected pages 2–4, imposed two pages per sheet and chose landscape Letter. KDE's
built-in **Stampa su file (PDF)** received the resulting document. A second successful GUI
submission after host cancellation, and a headless `doc_print` submission through the same
installed Flatpak, produced identical output. The tool returned `status: "accepted"`.

`pdfinfo` reported two unencrypted PDF 1.7 pages, each **792 × 612 pt**, 1,374 bytes.
The compared portal outputs were **byte-for-byte identical** to the print-ready PDF generated
by the sandboxed CLI's Save as PDF path for those settings:

```
SHA-256: 83213200b1525b8f30e8498e09aaa41a90e22537c6677ae2bffba1170197512e
```

The output destination was outside the sandbox's visible filesystem; a sandbox check
confirmed the output pathname was inaccessible while the host file existed. This proves the
handoff used the portal, without gaining access to that host directory or to CUPS.

## Cancellation and resources

- Cancel in the KDE dialog: no output file, no success/error notice, progress removed and
  the GUI still responsive. The next request printed the expected document successfully.
- Cancel in PdfCraft's progress card: the pending KDE dialog closed, progress cleared and
  no success notice appeared.
- Quit PdfCraft with a pending dialog: the host dialog closed and no file was created.
- During a pending request, the application held one descriptor to an unlinked temporary PDF.
  After the response, no temporary PDF descriptor remained. The PDF was already
  anonymous before the host dialog opened; creation uses an anonymous file before writing.
- The private D-Bus tests additionally verify read-only descriptors, early responses,
  malformed responses/tokens/settings, incompatible layout/output format, failed handoff,
  missing services, service disappearance, timeouts/Request.Close and consecutive requests.

## Checks

Original functional checks on 2026-10-17 (all passed on that revision):

| Command/scope | Result |
|---|---:|
| `cargo test -p pdfcraft-print -j 2` | 36 tests |
| `cargo test -p pdfcraft-engine -j 2 --lib print` | 11 tests, including permission checks and a snapshot after source closure |
| `cargo test -p pdfcraft-ui-egui -j 2 --test print --test control` | 14 print + 59 control tests |
| `cargo test -p pdfcraft-ui-egui -j 2 --lib print_ui` | 8 tests, including progress cancellation and acceptance wording |
| `cargo test -p pdfcraft-automation -j 2 --test automation printing` | 3 tests |
| `cargo test -p pdfcraft-automation -j 2 --test automation_roots` | 8 tests |
| Installed Flatpak GUI/CLI → KDE Print to File | identical PDF, repeated submission and cancellation verified |

Other checks: `cargo fmt --check`; scoped Clippy with `-D warnings --all-targets` on print,
engine, automation, ui-egui and the desktop app; `cargo xtask layers`, `assets` and `parity`;
WASM compilation of pdfcraft-web with Rust 1.95; manifest parsing/consistency and unchanged
permissions; desktop/AppStream metadata validation; Bash syntax; Flatpak export, bundle,
installation and execution. WASM emits existing dead-code warnings in render/UI modules.
The expected regression failure on the base is the only intentional failing check.

## Packaging and limits

The local test package used debug binaries with symbols stripped. CachyOS's libm exports
newer GLIBC symbol versions than runtime 25.08, so the local binaries were linked against the
runtime's libm before packaging; no linker setting or bundled glibc is added to the repository.
The release pipeline already builds Linux binaries on Ubuntu 22.04. The test bundle includes
the existing desktop metadata, icons and licence notices; the full offline Flathub source
build and optional release OCR/Japanese-font inputs were not exercised by this printing test.

Physical printing, duplex behaviour, paper calibration, collation and printer-driver
compatibility are not hardware-validated. Native CUPS selection and the native GUI were
verified, but this host has no native CUPS queues. Windows/macOS runtime printing was not
rerun; their backends are unchanged. X11 parent identifiers are supported; Wayland uses an
unparented, nonmodal host dialog because eframe cannot safely export an xdg-foreign handle.
Cancellation closes the portal request; recalling a physical job already handed to the host
is not guaranteed. The host's physical paper/scale choices still require attention; conflicting known page/N-up/
reverse/scale settings are refused before handoff; PdfCraft supplies only PDF input to the host, whose dialog handles physical printer output.

Private validation artifacts (logs, synthetic PDFs and local screenshots) were kept outside
the repository. No third-party desktop screenshots, binary fixtures or fonts are committed.

## Revision history

| Date | Change |
|---|---|
| 2026-10-17 | Added implementation, baseline regression, real sandbox/PDF output and cancellation evidence. |

## Refresh against main (2026-10-23, PR #3066)

The branch integrates upstream `main` at `0dc77fe4` without changing the original
portal transport. Conflict resolution retains upstream low-resolution permission
checks, print-content filtering, hidden-layer state, localized driver options,
optional duplex settings, paper persistence and the remembered print destination.
The Flatpak regressions now live in the consolidated `it` integration targets;
their child invocations use the full module-qualified test names and assert that
one test actually passed. An explicitly selected Save as PDF remains selected
when the Flatpak print dialog reopens.

This refresh is checked on Windows. The Linux/KDE/Flatpak runtime evidence above
belongs to the original 2026-10-17 revision and has not been rerun on this host.

Fresh native checks: **155 tests passed**, using `--profile low-disk -j 2 --locked`:

| Scope | Passing tests |
|---|---:|
| `pdfcraft-print` | 40 |
| engine `--lib print` | 16 |
| UI `--lib print_ui` | 8 |
| UI `--test it print::` | 25 |
| UI `--test it control::` | 55 |
| automation `printing` | 3 |
| automation `--test it automation_roots::` | 8 |

`cargo fmt --all --check`, scoped Clippy on print/engine/automation/ui-egui/desktop
with `--all-targets -- -D warnings`, and the `layers`, `assets` and `parity` xtasks
passed. `pdfcraft-web` passed the `wasm32-unknown-unknown` check, with existing
render/UI dead-code warnings. Linux runtime tests are excluded on Windows.

The Linux-only portal and its test code also pass cross-compilation with
`cargo check -p pdfcraft-print --target x86_64-unknown-linux-gnu --tests`
(using the same profile/job/lock flags). This is compilation coverage, not a
Linux runtime or physical-printer result.

## CI follow-up (2026-10-23)

The branch now integrates upstream `main` at `57e531cc`, including its existing
`ctutils` 0.4.5 update for RUSTSEC-2026-0385. No advisory exception was added.
The eight new system-print UI strings have translations in all 33 catalogs,
including the catalogs whose coverage is still partial. Translations retain the
distinction between service acceptance and physical completion, host-owned
printer settings, and the page/layout/scale instructions for the prepared PDF.
The automation tool description retains both upstream's page-number subset
clarification and the portal-specific handoff instructions after the merge.

Fresh tests on Windows, Rust 1.99, `--profile low-disk -j 2 --locked`:

| Scope | Passing tests |
|---|---:|
| UI `--lib` (including all i18n coverage and catalog validation tests) | 540 |
| UI `--test it print::` | 25 |
| `pdfcraft-print` | 41 |

`cargo fmt --check`, UI Clippy with `-j 8 --all-targets -- -D warnings`, and
the `layers`, `assets` and `parity` xtasks pass. `pdfcraft-web` passes the
`wasm32-unknown-unknown` check (existing render/UI dead-code warnings).
`cargo-deny` 0.20.2, `--all-features check`, reports advisories, bans, licenses
and sources all OK. The failed CI runs on the preceding revision provide the
before-fix evidence for the i18n failures and the vulnerable lockfile entry.

Linux/FreeBSD runtime tests and physical printing were not rerun locally in
this follow-up; the original KDE/Flatpak evidence remains scoped as above.

## MCP responsiveness follow-up (2026-10-24)

MCP system-dialog printing now returns a pending job and polls/cancels it through
`doc_print_job`; CLI/direct automation retains the synchronous receipts used by the physical
Flatpak validation above. Composition and portal waits run on a worker for MCP. The synthetic
MCP regression holds an injected portal open while serving two pings and a cancellation,
without contacting a real desktop portal. The old synchronous dispatch failed its deadline.
Additional regressions cover bounded receipts, terminal errors, overlap refusal, cancellation
and session-drop cleanup. Unsupported portal arguments now fail before layout/composition.
No additional physical-printer or host-dialog validation is claimed for this API follow-up.
