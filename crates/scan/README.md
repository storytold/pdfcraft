# pdfcraft-scan

Layer L4: find scanners and scan pages, for **File ▸ Create ▸ PDF from Scanner** (issue #312).
It has no UI and no PDF code: the engine turns the scanned images into pages
(`Session::create_from_scan`), the shell shows the dialog, and the automation crate offers the
`scanners` tool and `doc_create` with `from: "scanner"`.

```rust
let found = pdfcraft_scan::scanners(Duration::from_secs(3));   // never fails; maybe empty
let settings = Preset::GrayscaleDocument.settings();            // colour mode + dpi
let pages = pdfcraft_scan::scan(&found[0].id, &settings, &cancel)?;   // Vec<ScannedPage>
```

A scanner's `id` names the backend that drives it:

| id | Backend | Platforms | Finds scanners with |
|---|---|---|---|
| `escl:http://host:port/eSCL` | **eSCL** (AirScan / Mopria): HTTP + XML | all but the web build | mDNS `_uscan._tcp`, or typed: `escl:192.168.1.20` |
| `sane:<device>` | **SANE** through the `scanimage` program | Linux, FreeBSD, other Unix | `scanimage -L` |

No `unsafe` and no C bindings: eSCL is plain HTTP (`ureq`) and XML (`roxmltree`), mDNS is
`mdns-sd`, and SANE drives `scanimage`. Windows supports eSCL only; WIA is deferred until a
suitable Rust binding exists, and no PowerShell scanner helper is launched.

Typed addresses and mDNS results must use numeric loopback, link-local or private addresses
(RFC 1918 / IPv6 ULA). Hostnames, public addresses, credentials, queries and fragments are
refused. HTTP redirects are never followed, proxies are disabled, and scan-job `Location`
headers must remain on the same origin (scheme, address and port). Numeric addresses avoid
DNS rebinding; mDNS names are shown as labels, while connections use the advertised local IP.

The automation `scanners` and `doc_create` tools advertise `openWorldHint: true`.
`doc_create` with `from: "scanner"` requires `user_confirmed: true` only after explicit user
consent; omitted or false consent is rejected before any scanner request. This is a caller
attestation, not a way for the application to verify a human interaction.

Network access is limited to the scanner workflow: local-address HTTP and mDNS discovery.
The restrictions above implement the PR #359 review requirements; they do not authorize
general outbound requests elsewhere in the app.

- **Settings**: colour mode (black and white, gray, colour), resolution (the device's closest
  supported one is used), source (flatbed, document feeder, feeder both sides) and paper
  (Letter, Legal, A4, A5, or the scanner's whole bed). The four presets (Black & White Document,
  Grayscale Document, Color Document, Color Photo) set the colour mode and resolution.
- **Pages** come back as the image file the device sent (PNG, JPEG, TIFF) with their resolution;
  `pdfcraft-create` embeds them without resampling, sized from that resolution.
- **Errors** are `ScanError`s an agent or person can act on: feeder empty, jam, busy, cover open,
  no such source, not found, cancelled.
- **Cancel**: setting the `AtomicBool` stops a scan (the eSCL job is deleted, the program is killed).
- **Limits**: at most 500 pages per scan and 512 MB per page; every answer from a device or a
  program is untrusted and size-capped (AGENTS.md §4).

## Testing without hardware

- `fake::FakeEscl` (feature `fake-escl`) is an eSCL scanner on loopback with a flatbed and a duplex
  feeder; the automation and UI tests scan from it.
- SANE's `test` device (`sane-utils`) runs the real `scanimage` path, including the 10-sheet feeder
  (skipped when `scanimage` is missing).
- Address/Location restrictions and redirect refusal are covered by loopback tests; automation
  tests verify that missing consent sends no requests. No real scanner hardware was available.
