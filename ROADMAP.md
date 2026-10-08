# PdfCraft roadmap

The milestones, current progress and time estimates to Acrobat Pro feature parity. Keep this up to date:
- **Every session:** update the progress column and add a line to the log.
- **Every milestone:** re-estimate.

Detailed task lists and acceptance tests are in `plan/execution-plan.md` (local-only). This file is the public summary.

**What "parity" means here:** the offline feature set of Acrobat Pro, milestones M0–M14. It excludes Adobe's cloud services (Document Cloud storage, Adobe Sign, the Adobe AI Assistant). Those have no clean-room equivalent; PdfCraft's alternatives are local-first, plus opt-in providers (M13).

## Estimate summary

The unit is **wall-clock hours of agent work** (Claude Opus 5.5 coding continuously; human review time not included).

**Where we are (2026-10-07, measured by `cargo xtask parity` over 805 tracked Acrobat Pro features; 23 more are Adobe-cloud-only and out of scope):**

| Tier | Features | Shipped | Partial | Shipped % | Weighted % (partial = ½) |
|---|---|---|---|---|---|
| P0 (must-have for 1.0) | 251 | 222 | 25 | 88.4% | 93.4% |
| P1 | 325 | 172 | 30 | 52.9% | 57.5% |
| P2 | 186 | 10 | 2 | 5.4% | 5.9% |
| P3 | 43 | 0 | 0 | 0% | 0% |
| **All** | **805** | **404** | **57** | **50.2%** | **53.7%** |

**Effort-weighted parity: ≈ 30–35%.** Feature counts overstate progress: the remaining features include the hardest ones (our own renderer and font engine, editing existing text and reflow, OCR beyond Latin, XFA, PDF/A/X/UA preflight, Office export, long-term signature validation). Weighting each milestone by its estimated size gives about a third of the total work done. See **[Honest assessment](#honest-assessment-2026-10-05)** for what the numbers don't show.

**Observed throughput:** about 72k lines of kept, tested Rust (plus 97 agent tools and ≈ 600 tests) in roughly 65–75 agent-hours since 2026-09-30, about 1.0k lines per agent-hour, with corpus, oracle and visual checks. That is faster than the original plan assumed, so the hour estimates below are revised down from the first plan (2,000–4,000 h).

| Scenario | Remaining hours to full parity (M0–M14) | Calendar time |
|---|---|---|
| One Opus 5.5 agent, continuous | **≈ 600–1,100 h** | ≈ 4–7 weeks |
| 3–5 agents in parallel on separate crates | **≈ 250–450 h wall clock** | ≈ 2–3 weeks |
| With human review, integration and pauses | — | ≈ 2–4 months |

**How the estimate is built:** each milestone's remaining fraction (table below) times its size, re-based on the measured rate. The long poles are M2 (own renderer, ≈ 150–250 h), M7 (editing existing text and images, ≈ 150–250 h), M10 OCR and Office export (≈ 100–150 h), M6 JavaScript engine (≈ 60–100 h), M11 PDF/A/X/UA preflight (≈ 80–140 h) and M12 XFA/compare (≈ 80–140 h). The last 5–10% (odd real-world files, pixel-level polish against Acrobat) costs about as much as a mid-sized milestone.

## Honest assessment (2026-10-05)

Read this before choosing work. The feature table above counts what exists; this section says how solid it is.

| Dimension | State | In one line |
|---|---|---|
| Feature count | 50.2% shipped (P0 88%, P1 53%, P2 5%, P3 0%) | A typical viewer, annotator, form-filler or page organizer is mostly covered |
| Effort | ≈ 30–35% | The remaining work is the hardest: M2 15%, M7 17%, M11 18%, M12 20%, M14 0% |
| Foundations | Weakest | Rendering is still the bootstrap `hayro`; the inspector is `lopdf`; 18 vendored patches carried |
| Robustness | Early beta | Each 15-minute fuzz run found new out-of-memory crashes or hangs until 2026-10-05 |
| Acrobat fidelity | Unmeasured | No systematic side-by-side comparison with Acrobat, visual or behavioural |
| Product readiness | 0.2.1 | No code signing, performance budgets or keyboard-only operation yet; localization has its catalog system (English, partial Japanese) but few strings go through it |

**By area** (shipped share of each area's features):

| Area | Shipped | What's strong / what's missing |
|---|---|---|
| A Core | 75% | Parser, repair, encryption, incremental saves. Missing: lazy loading (`ByteSource`), own image codecs, PDF 2.0 extras, Arlington validation |
| F Forms | 71% | Filling, authoring, AF scripts, sandboxed JavaScript, data exchange. Missing: XFA, wider JavaScript object model |
| E Comments | 63% | All markup types with appearances, XFDF/FDF, summaries. Missing: replace-text proposals, summary layouts |
| M Accessibility | 62% | Checker (all 32 rules). Missing: autotag, Tags/Order/Content panels, Reading Order tool, keyboard-only operation |
| B View | 55% | Shell, find, panels, tiles, web build. **Rendering is borrowed (`hayro`)**, so the 7 rendering P0s are only partial |
| D Organize | 55% | Pages, combine, split, bookmarks, labels. Missing: replace pages, transitions |
| G Protect | 54% | Passwords, permissions, redaction, sanitize. Missing: certificate security, redaction codes |
| H Sign | 48% | PAdES B-B signing and validation. Missing: timestamps (B-T), LTV (DSS/OCSP/CRL), FieldMDP, Windows store, PKCS #11 |
| C Edit | 46% | Added text and images stay editable; header/footer/watermark. Missing: robust editing of existing text and images (fonts, subsets, reflow) |
| L Print | 36% | Acrobat-style sizing, n-up, booklet, CUPS. Missing: Windows and web printing, production options |
| J Create | 32% | From images, text, clipboard; Word/HTML/RTF export. Missing: Office import, Excel/PowerPoint export |
| N Misc | 27% | CLI, MCP, UI control channel, Action Wizard. Missing: AI providers, performance budgets |
| K Optimize | 26% | Reduce File Size, Optimizer. Missing: preflight, PDF/X/UA, transparency/fonts panels |
| I OCR | 19% | Searchable image for Latin script. Missing: other scripts and accents, editable-text output, deskew |

**What "shipped" means, and doesn't.** A feature is shipped when it exists and at least one specific test covers it. 159 of 398 shipped features rest on exactly one test, and only about 5 cite an external oracle (`pdftotext`, `pdfsig`, OpenSSL, a corpus). Shipped does not mean "as good as Acrobat".

**Robustness, measured.** `main`'s CI was red for 30+ runs until 2026-10-04 (a clippy lint, Windows line endings, flaky perf tests). Once the nightly fuzz job could finish, three 15-minute runs found 7 out-of-memory crashes (one aborted on every platform) and 11 hangs, 7 of them in our own `cos` parser rather than `hayro`. All are fixed or in review, but expect more: keep the nightly fuzz job green and turn every finding into a synthetic test.

## Where we're lacking and where we're going

The gaps that matter most, in priority order. Agents: prefer these over adding P2/P3 features, and check `parity/acrobat-features.toml` notes for the specifics of each.

1. **Our own renderer (M2).** Replace `hayro` with the `model` crate, our font engine and the DisplayList devices (ADR-0004). It turns the 7 rendering P0 partials into shipped features and removes most vendored patches. The largest single lever.
2. **Hardening.** Keep the nightly fuzz job green; fix every crash and hang with a synthetic regression test; finish lazy loading so large files aren't read whole; set performance budgets (`misc.performance-budgets`).
3. **Measure fidelity against Acrobat.** Build a side-by-side harness (render, text, form behaviour) over synthetic fixtures (`plan/acrobat/`, clean-room rules apply). Until then, parity numbers count features, not quality.
4. **Editing existing content (M7).** Fonts, subsets, reflow and CJK. The most visible gap for Pro users.
5. **Pro workflows.** Signatures with timestamps and LTV (M9), OCR beyond Latin (M10), Office import/export (M10), preflight and PDF/A/X/UA (M11), XFA (M12).
6. **1.0 polish (M14).** Code-signed installers, localization (the catalog system is in place; route every dialog and panel string through `tl!`, then add languages), keyboard-only operation, a full screen-reader audit.

Decided against for now (owner, 2026-10-05): self-installing updates and an update check at start. Help ▸ Check for updates is manual only.

## Milestones

Hours are for a single agent (low–high). "Done" is the estimated fraction of that milestone's *acceptance criteria* that are met. It is not a count of lines of code.

| M | Milestone | Est. hours | Done | Remaining (h) | Notes |
|---|---|---|---|---|---|
| M0 | Skeleton: workspace, xtask gates, CI | 15–30 | 92% | 2–4 | GitHub workflow, `deny.toml` (licence audit of every dependency), parity checklist (826 features, `xtask parity`) done. Missing: remaining crate stubs, testkit/oracle crates |
| M1 | COS: filters, crypt, parser, xref, writer | 120–200 | 78% | 25–45 | Done:<br>- filters and crypt: every standard-security revision R2–R6 (RC4, AES-128/256), SASLprep, permissions, creating encryption;<br>- cos: parse and repair (including invalid xref object-number ranges), decrypt on load, re-encrypt on save, incremental and full writing.<br>Corpus: open/edit/save passes on 958 files, and all 7 password-protected files open.<br>Full saves now pack objects into compressed object streams. Fuzzing runs nightly (`xtask fuzz`). Missing: ≥ 250 tests, own image codecs |
| M2 | Model, render, text | 200–350 | 15% | 170–300 | hayro bootstrap renderer (vendored patches). Text extraction reaches word-F1 0.98 against pdftotext. Missing: model crate, fonts, DisplayList, renderer independent of hayro |
| M3 | Viewer app (native + web) | 80–150 | 88% | 10–19 | Acrobat-style shell, find, select, panels, tiles, web build, UI control channel for agents (opt-in). Missing: 60 fps test on a 500-page document, snapshot tests of every panel Done since: fit visible, document title in the window; Linux middle-button scrolling, covered by headless UI regressions. |
| M4 | Engine, history, save, organize | 100–180 | 95% | 9–17 | Done:<br>- command registry (menus, shortcuts and palette all use it);<br>- undo/redo; incremental, atomic and encrypted saves;<br>- autosave and crash recovery;<br>- organize, combine, extract, split and insert-from-file, with identical fonts and images stored once;<br>- CLI `edit/combine/extract/split`.<br>Done since: bookmark editing, page labels (Number pages), CLI `run`, Set Page Boxes, Crop tool, Duplicate pages, Ctrl+A / Cmd+A to select all pages in Organize. Missing: Replace pages, page transitions, recovery on the web |
| M5 | Comments (all annotation types, XFDF) | 120–200 | 88% | 15–25 | Done: notes, highlight/underline/strikeout/squiggly, text boxes, ink, lines, arrows, rectangles, ovals, stamps (dynamic, Sign Here, Standard Business), with appearance streams; replies, status, checkmarks, locking, move/resize/restyle/delete; properties dialog; panel filter and sort; hide all; summaries (comments only); XFDF/FDF import and export; flatten; Fill & Sign; agent tools. Done since: polygons, connected lines, clouds, callouts, inserted text, colour and checkmark filters. Missing: custom stamps, replace-text proposals, summary layouts with the page |
| M6 | Forms + JavaScript | 160–320 | 45% | 95–185 | Done: filling all field types with regenerated appearances; Prepare a form (every field type, move/resize/delete, Field Properties General/Appearance/Position/Options/Format/Validate/Calculate); Acrobat's AF format/keystroke/validate/calculate functions and simplified field notation run natively in Acrobat's event order; tab order; push-button actions; flatten; agent tools. Form data exchange (FDF, XFDF, XML, CSV, text) done. Done since: JavaScript engine (boa, sandboxed) running custom keystroke/validate/calculate/format and button scripts, document JavaScripts, console. Missing: Actions tab, wider object model (annotations, layers, dialogs), XFA |
| M7 | Content editing (text, images, header/footer, watermark) | 250–500 | 17% | 210–420 | Done: header & footer, watermarks, backgrounds, Bates; added text and images that stay editable (move, resize, format, rotate, flip, crop, replace); links (Link tool, Link Properties, create from URLs, remove all). Missing: editing existing text and images in place (the longest pole), image/PDF watermarks |
| M8 | Security + redaction | 100–180 | 66% | 35–65 | Done: opening protected documents, permissions, Protect Using Password, Remove security; redaction (mark text/areas/pages, Search & Redact with patterns, apply removing glyphs, image pixels, vectors, XObject content, annotations and fields, verification, full rewrite on save); Remove hidden information and Sanitize. Missing: certificate security, redaction codes and pattern locales, DCT re-encoding |
| M9 | Signatures (PAdES, validation) | 160–280 | 55% | 70–125 | Done: new `sign` crate (DER, X.509, CMS, PKCS #12 on RustCrypto; aws-lc-rs for RSA private keys); PAdES B-B signing (visible, invisible, existing fields, certification with DocMDP); validation with trust store and changes-after-signing classification; self-signed digital IDs; Signatures panel, message bar, sign dialogs; agent tools. Checked with pdfsig and OpenSSL. Done since: macOS Keychain signing, certificate viewer, signed documents protected from rewrites. Missing: timestamps (B-T), LTV (DSS, OCSP, CRL), FieldMDP, Windows store, PKCS #11 |
| M10 | OCR, create, export, print | 200–350 | 33% | 130–235 | Done: create from blank/text/PNG/JPEG/TIFF (multi-page)/GIF/BMP; export PNG/JPEG/TIFF and text; Print (Acrobat's sizing, n-up, booklet, poster, comments & forms, preview, CUPS spooler, print-ready PDF). Done since: export all images; OCR (searchable image for pages, ranges and multiple files); single-sided cut-and-stack imposition with cut marks, through Print and doc_print. Missing: OCR languages beyond Latin, editable-text OCR output, Office export/import, Windows/web printing |
| M11 | Optimize, preflight, PDF/A/X/UA, print production | 200–350 | 18% | 165–290 | Done: new `optimize` crate: Reduce File Size and the PDF Optimizer (images measured where drawn, bicubic downsampling, JPEG/ZIP recompression only when smaller, discard objects and user data, Flate clean-up, resource merging, object streams). Missing: fonts and transparency panels, space audit, preflight, PDF/A/X/UA |
| M12 | Accessibility, compare, measure, search, XFA | 200–380 | 20% | 160–305 | Done: new `a11y` crate with the Accessibility Checker (all 32 rules, report, Fix/Skip/Explain, options dialog and results panel, agent tools). Missing: autotag, Tags/Order/Content panels, Reading Order tool, alt-text workflow, compare, measure, search index, XFA |
| M13 | Automation (MCP, Action Wizard, CLI) + AI providers | 60–120 | 45% | 33–66 | Done: MCP resources (document info, text, page images); headless tool table (123 tools incl. signing, optimizing, initial view, links, stamps, data exchange, comment review, forms authoring and scripts, redaction, sanitize, print, add content), opt-in MCP server over stdio, CLI `run`/`tools`, UI control channel with drag. Missing: Action Wizard, AI providers |
| M14 | 1.0 polish: performance, localization, installers | 120–250 | 5% | 115–240 | Done: PhotoCraft's translation system (`i18n/`: TSV catalogs, `tl!`, command-id and plural entries, system-language detection, strict catalog tests); every dialog, panel and notice goes through `tl!`; Japanese and Traditional Chinese complete for them (≈1,800 entries each), Simplified Chinese ≈1,600, Czech and Brazilian Portuguese for menus. Missing: more catalogs filled in, Windows language detection, installers, performance budgets |
| | **Total (original plan sizing)** | **2,085–3,840** | **≈ 35%** | **≈ 1,350–2,500 at the planned rate; ≈ 600–1,100 at the measured rate** | |

**Overall progress: about 30–35% of the effort (49.5% of features shipped).** The viewer and the core are far ahead of the editing features, because the viewer was built first so progress could be seen; rendering itself is still borrowed from `hayro`. See [Honest assessment](#honest-assessment-2026-10-05).

## Critical path

M0 → M1 → M2 → M3 → M4 must happen in order. After M4, M5–M12 can run in parallel across crates. The long poles are M7 (content editing), M9 (signatures) and M12 (XFA).

## Risks most likely to push estimates up

- Fidelity of text editing: fonts, subsets, reflow.
- XFA dynamic layout.
- Real-world signature chains and revocation checks.
- The rendering long tail: Type3 fonts, broken fonts, shadings.
- Correctness of PDF/A and PDF/UA conversion.
- CPU rendering performance on the web.
- Hostile input: every fuzz run so far has found new crashes or hangs.
- Unmeasured fidelity: without an Acrobat comparison harness, quality gaps surface as user reports.

## Log

Newest first. One line per session: the date, what moved, and the new overall percentage.

- **2026-10-08 (community):** Cut and stack in Print > Multiple: single-sided n-up sheets can be cut into cell piles and restacked in selected page order, with aligned gutter marks and blanks kept in their cells. Available through doc_print; duplex is refused. Grid-size overflow returns an error. No change to the parity count or the overall estimate (about 30–35%).
- **2026-10-07 (community, #143):** Windows MSI gets a full-UI welcome dialog with a "Create a desktop shortcut" checkbox (on by default; `INSTALLDESKTOPSHORTCUT=0` for unattended installs), progress, files in use, repair/remove, and a "Setup completed successfully" screen with Finish (plus distinct failed/cancelled screens). Start Menu and desktop shortcuts are now plain links to `pdfcraft.exe` instead of advertised MSI shortcuts. Packaging checks the compiled MSI's shortcut, checkbox and dialog tables; the ARM64 smoke test checks both links' targets, their removal on uninstall, and the opt-out. Installer polish only; feature counts unchanged. ≈ 30–35%.
- **2026-10-07 (localization):** The interface translates through the same system as PhotoCraft: one TSV catalog per language (`crates/ui-egui/src/i18n/ja.tsv`), the `tl!` macro, entries keyed by command id or with plural forms, and catalog tests (format, duplicates, placeholders, command ids). Catalogs are validated strictly (unknown escapes, reserved contexts, placeholders, ellipses, plural forms), a bad line is skipped and logged at run time, `fmt` substitutes in one pass, the drawing language is per thread, and each `.tsv` is an attributed asset (`kind = "translation"`, checked by `cargo xtask assets`); validation, attribution and the language-switch control test folded in from #175. The 49 existing Japanese strings moved over unchanged, plus `Auto`. The `language` preference now defaults to `auto` (follow the system language; English when it has no catalog); `en`/`ja` still persist, and unknown values are rejected. Not covered: dialogs and panels, Windows system-language detection. ≈ 30–35% (unchanged).
- **2026-10-07 (session 16):** PrintCraft is renamed PdfCraft: repo `storytold/pdfcraft`, crates and binaries `pdfcraft*`, bundle id `ai.storyteller.pdfcraft`. On first launch the app moves the old PrintCraft settings and crash-recovery folders to the new name, so upgrades keep recent files, preferences and unsaved work. No feature change. ≈ 30–35%.
- **2026-10-07 (community, #163):** Fill & Sign shows saved signature/initials previews with add, change and remove controls in the quick picker and left panel. The editor loads the saved typed text or drawn strokes; Apply replaces future placements and Cancel preserves the saved value. Existing annotations stay unchanged. Long typed names shrink to the placement width and include glyph overhangs in preview/PDF bounds. Regression coverage includes persistence, removal/recreation, long names and the opt-in UI control channel. Parity counts and effort estimate unchanged (50.2% shipped, ≈ 30–35% effort).

- **2026-10-07 (fork, default-workspace-mode):** Preferences ▸ Documents and view remembers the workspace used for newly opened PDFs. All Tools remains the default; explicit CLI/control mode overrides last for the session and PDF Initial View metadata stays independent. Overall estimate unchanged (≈ 30–35%).

- **2026-10-07 (community, M14, #112):** Simplified Chinese (简体中文, `zh-hans`) as a catalog of about 1,600 entries; `zh`, `zh_CN` and `zh-Hans` locales follow it. In Simplified Chinese the craft-fonts `Hans` faces come before the Japanese ones, so Simplified-only characters don't sit on a different baseline.
- **2026-10-07 (community, M14, #140):** Traditional Chinese (繁體中文, `zh-hant`, Taiwan vocabulary) as a catalog of about 1,800 entries; `zh_TW`, `zh_HK`, `zh_MO` and `zh-Hant` locales follow it. Drawn with the craft-fonts CJK faces (missing characters show replacement boxes).
- **2026-10-07 (community, M14, #141):** Every dialog, panel, notice, shortcut list and native-picker title goes through the catalog (`tl!`, `i18n::fmt`), and Japanese grows from 49 to about 1,800 entries (`ja.tsv`); worker threads (export, OCR, actions) write their summaries in the UI language; history labels keep captured file and field names; the command palette searches translated and English labels and ids. Engine and OS error text is no longer matched against English templates: it is shown as is, inside a translated frame. Localization remains partial for other languages.
- **2026-10-07 (session 15):** Five community issues, each with a regression test: in two-page view, Next page moves a whole spread (#77, #70); Export to Word opens in Word again, with control characters dropped and receipt-length pages kept within Word's 22 in, checked in Microsoft Word (#78, #72); macOS opens PDFs from Finder, Open With and the Dock (Apple events through the audited `fmv-macos-events`, PDF declared in the Info.plist), and the packaging no longer describes the image editor it was copied from (#82, #73); Add text keeps the box being typed when you click elsewhere, with Done and Discard beside it, reproduced and checked with real OS input (#86, #74); sharper, higher-contrast text, most visible on Windows, with every text colour at WCAG AA in both themes (#84, #76; the console window was already gone in 0.2.1). Still open: #79 (layered "not verified" signature appearances), #80 (native macOS menu bar), #81 (Homebrew and Scoop). P0 88%, P1 53%; 805 features 50.2% shipped (53.7% weighted). ≈ 30–35%.
- **2026-10-06 (community navigation):** Linux-only middle-button vertical scrolling in the document and Organize grid, toggled by clicking the wheel, with cancellation that protects page tools and selection. Scrolling stays on after releasing the wheel, even if the pointer moved before release. Manual feedback led to a Chromium-style distance^2.2 speed curve, a 15-screen-point dead zone, and continuous redraws that use elapsed frame time; the former linear speed cap is removed. Headless UI/control regressions cover fractional movement, consistent travel at 30/60/120/144 fps, pause/resume and cancellation. Other platforms retain their existing middle-button input behavior, with a regression that checks the custom scroller neither blocks input nor changes the cursor or consumes Escape. No new dependency. Automatic scrolling remains partial because keyboard-started timed scrolling is absent. Overall effort estimate remains ≈ 30–35%.
- **2026-10-06 (session 14):** Issues: move and resize existing text boxes in Edit text (#59, closes #40); Windows release builds open no console window and the Start Menu shortcut has its icon (#58, #57; rolled out to every Craft app); native Windows ARM64 installers, installed and run on an ARM64 runner in CI (#66, #71; the intermittent ARM64 test crash was a WARP shader-JIT race between parallel GPU-rendering tests, not our code); XFA forms are detected and explained instead of failing silently (#65, #60 stays open for real XFA). Also: squiggly underline tool, comment opacity, Preferences ▸ Identity, drag-and-drop/hover tests; community table export to Word/HTML/RTF (#62, with linear-time detection and a column cap). #63 (timestamps/LTV with outbound network) held for review fixes and an owner decision on network access. ≈ 30–35% (unchanged).
- **2026-10-06 (community, #68):** Interface language: Czech (Čeština, `ui.set language=cs`) joins English and Japanese. It translates every registered File/Edit/Pages/View/Help command and every label routed through the translation table, with tests that fail when a menu label lacks Czech; the interface fonts are tested to cover Czech diacritics. Dialogs and panels remain English (misc.prefs-language: partial).
- **2026-10-06:** M1: invalid object-number ranges in xref tables and streams take the existing reconstruction path, with the reason recorded, instead of overflowing or aliasing another object. Synthetic open/repair regression added. Overall estimate unchanged (≈ 30–35%).

- **2026-10-05 (session 13, later):** FreeBSD: CI builds and tests the workspace in a FreeBSD 14.3 VM (#45) and the release ships a FreeBSD x86_64 tarball, checked in CI (#42); README lists FreeBSD. Fuzzing: the nightly job's 11 hangs fixed (#41: our parser's exponential retries, inline images, Type 3 fan-out), plus a JBIG2 hang (#43, vendored hayro-jbig2) and a Type 1 font stack overflow (#46, skrifa 0.47); a full run now finds 0 crashes. Landed community PRs #36 (text editing, Japanese replacements), #47 (English/Japanese interface) and #48 (XFDF colour crash). Windows launch crash with Intel's Vulkan driver fixed by using Direct3D 12 (#49, issue #37); one copy of the Japanese font instead of two (#51, −8.3 MB); full screen and About get real tests (#50). P0 88%, P1 52%. ≈ 30–35%.
- **2026-10-05 (community, #47):** Interface language: Preferences ▸ Interface language switches English/Japanese and persists (`ui.set language=ja|en`, reported by `ui.state`). Core menu labels are translated; dialogs, panels and more languages remain open (misc.prefs-language: partial).
- **2026-10-05 (session 13):** Landed community PRs #3–#7 and the never-crash rollout (#11–#25); CI green on every job for the first time in 30+ runs (clippy 1.99, Windows line endings, perf-test noise); nightly fuzz job made to finish (child memory cap) and its findings fixed: 7 out-of-memory crashes (#27, #29, #30) and 11 hangs (in review); issue #8 (integrated GPU, keyboard save prompt); Help ▸ Check for updates (manual only). Added the honest assessment and gap list above. P0 88%, P1 52%; 804 features 49.5% shipped (53.3% weighted). ≈ 30–35%.
- **2026-10-03 (session 12, later):** Forms (merge data into a spreadsheet, Actions tab, automatic field detection and naming), Compare files (new `compare` crate: text and visual differences, panel, report, comments), Action Wizard (built-in and custom actions over files), PDF/A verify and Save as PDF/A (new `preflight` crate), Export to Word/HTML/RTF (new `export` crate). P0 88%, P1 52%; 803 features 49.6% shipped (53.3% weighted). ≈ 39%.
- **2026-10-03 (session 12):** Edit text and images in place (lines, paragraphs, formatting, existing images), Scan & OCR (new `ocr` crate on ocrs with CC-BY-SA models fetched by `cargo xtask models`: searchable image, page ranges, multiple files), Acrobat JavaScript (new `js` crate on boa: custom field scripts in Acrobat's event order, button scripts, document JavaScripts, console, Enable JavaScript preference, sandbox limits). 110+ tools; P0 88%, P1 48%; 803 features 47.9% shipped (51.1% weighted). ≈ 37%.
- **2026-10-02 (session 11, later):** Sign with macOS Keychain identities, Add alternate text, Advanced Search panel, space audit and link clean-up in the Optimizer, hybrid-reference files, signed documents protected from rewrites, redaction cleans the tags, custom stamps, image fields, Combine files with page choice. 97 tools; P0 82%, P1 45%. ≈ 30%.
- **2026-10-02 (session 11):** Accessibility Checker (new `a11y` crate: all 32 rules, report, fixes, options dialog and results panel), MCP resources for page images and text, revisions (list, open an earlier one), export all images, fit visible, form Preview and locked fields, document title in the window; plus attachment comments, eraser, date picker, duplicate/align/distribute fields. 91 tools; P0 80%, P1 43%. ≈ 29%.

- **2026-10-02 (session 10, last):** Manual tab order, backgrounds/watermarks from files (PDF pages as forms, images), Initial View and reading options (honoured on open), bookmarks expand/collapse, make-default comment properties, required-field borders, cut/copy/paste pages, marquee zoom, snapshot. 85 tools; P0 80%, P1 ≈ 27%.
- **2026-10-02 (session 10, latest):** PDF Optimizer and real Reduce File Size (new `optimize` crate: effective-resolution image downsampling, recompression, discards), Field Properties ▸ Options for every field type, Redaction Properties overlay styling, drag-to-reorder pages, repair notice and log; signature revalidation made incremental (comment edit on a 120 MB signed file 84 → 6 ms). 84 tools; P0 parity ≈ 80%. ≈ 27%.
- **2026-10-02 (session 10, later):** Digital signatures (new `sign` crate: PKCS #12, X.509, CMS; PAdES B-B signing incl. certification; validation with trust store and DocMDP-aware change detection; Signatures panel, message bar, sign and digital ID dialogs; signed documents keep incremental saves), polygon/cloud/connected-lines/callout/caret comments, Create PDF from clipboard, comment filters by colour and checkmark. Verified against pdfsig and OpenSSL. 83 tools; P0 parity 78%, P1 22.5%. ≈ 26%.
- **2026-10-02 (session 10):** Viewer conveniences (fit height, view history, cover page, close all, revert, system theme, find options), XFDF/FDF and form data exchange (new `xfdf` crate), stamps, organize extras (split by bookmarks/size, Extract and Rotate Pages dialogs), links (Link tool and properties, create from URLs), comment review (checkmarks, locking, copy text, hide all, summaries). Fixed the Comments panel's "…" menu, which chased the pointer. 79 tools; P0 parity ≈ 73%, P1 ≈ 19%. ≈ 23%.
- **2026-10-02 (session 9, later):** Print (n-up, booklet, poster, preview, CUPS), export JPEG/TIFF and create from TIFF/GIF/BMP, add text and images that stay editable, form formats/validation/calculation run natively (Acrobat's AF functions), field appearance, tab order, push buttons, 500-page frame budget. 67 tools; P0 parity ≈ 73%. ≈ 21%.
- **2026-10-02 (session 9):** Prepare a form (field authoring, Field Properties), Redaction (new `content` and `redact` crates: glyph/image/vector/XObject removal with verification, Search & Redact patterns, full-rewrite saves), Remove hidden information and Sanitize. 59 agent tools; P0 parity ≈ 63%. ≈ 18%.
- **2026-10-01 (session 8):**
  - Commenting: sticky notes, highlights, underline, strikethrough, text boxes, freehand, lines, arrows, rectangles and ovals, with replies, status, colours, moving and resizing; Acrobat-style quick bar and Comments panel; six agent tools (an agent can highlight a phrase by naming it).
  - Protect Using Password, with Acrobat's permission levels and compatibility options; Remove security.
  - Filling in forms: text, check boxes, radio buttons, combo and list boxes, Tab between fields, Clear form; three agent tools.
  - Performance: opening a 190 MB manual went from 172 s to under 2 s; comment and form edits on it are 6× faster.
  - Fixed: saving a password-protected document failed; encrypted documents were not reported as encrypted.
  - Set Page Boxes, a Crop tool and Duplicate pages.
  - Header & footer (page numbers, dates, Bates numbers), watermarks and backgrounds, with Update and Remove.
  - Export to PNG and text; create PDFs from images, text or nothing; Reduce file size.
  - Fill & Sign: typed text, marks, the date and a drawn signature.
  - Comment properties, filtering and sorting; flattening comments and form fields.
  - Fixed: the Hand tool did not pan.
  - 50 agent tools.
  - Overall ≈ 16%.

- **2026-10-01 (session 7):**
  - Bookmark editing and page numbering (Number pages), with undo, agent tools and UI.
  - A render watchdog: pathological pages are skipped after 20 s instead of spinning forever.
  - The agent control channel no longer reports success when the window is hidden.
  - Community links: Discord button in the title bar, plus Home, About, Help menu, CLI and README.
  - Overall ≈ 10%.

- **2026-10-01 (session 6):**
  - UI control channel: agents can inspect the widget tree, click, type, press keys, run commands and take screenshots of the running app. Off unless the app is started with `--control`; loopback only, token-authenticated.
  - Combine and insert store identical fonts, images and other resources once. 40 copies of the showcase: 130 MB → 3.9 MB, 3× faster, pixel-identical.
  - Full saves use compressed object streams (showcase 3.4 → 2.8 MB; the 40× combine is now 2.3 MB). Fixed opening encrypted files whose catalog is compressed, and a crash on looped page trees.
  - Fuzzing (`cargo xtask fuzz`, nightly in CI): about 150,000 mutated files tried. Seven crash and hang bugs found and fixed, each with a regression test: two in our code, five in the temporary renderer.
  - Parity checklist: 826 Acrobat Pro features tracked in `parity/acrobat-features.toml`; `cargo xtask parity` checks every claim against code and tests. 11.8% shipped overall, 31.6% of P0.
  - Overall ≈ 9.5%.

- **2026-09-30 (session 5, after a machine crash; no work lost):**
  - Agent control: new `pdfcraft-automation` crate with 20 JSON-Schema tools, an opt-in MCP server (`pdfcraft-cli mcp`, stdio only, can be compiled out), and `pdfcraft-cli run`/`tools`.
  - Text on rotated pages now reads along its lines; the pdf.js oracle is unchanged at median 0.980.
  - Finding text in 520 pages: 17.9 s → 3.7 s (parallel), about 20 ms when repeated (cached).
  - CI: dependency licence audit (`deny.toml`, `xtask deny`) and a GitHub workflow for macOS, Windows, Linux and wasm.
  - Overall ≈ 8%.

- **2026-09-30 (session 4):**
  - Combine, extract and split, with links, named destinations, fields, layers, attachments and nested bookmarks all carried over. Combined pages render pixel-identical to their sources.
  - Fixed a precision bug in how reals were written.
  - Encryption: a new `crypt` crate covering R2–R6. Encrypted documents can be opened, edited and saved; permissions are honoured; the Security tab is real. Checked against hayro and qpdf.
  - Autosave and crash recovery.
  - Command registry.
  - Asset policy (`AGENTS.md`, `ATTRIBUTION.toml`, `xtask assets`) and a README with 13 reproducible screenshots (`xtask screenshots`).
  - Overall ≈ 7%.
- **2026-09-30 (session 3):**
  - New crates: `filters` (every non-image filter, 57 tests) and `cos` (object parser, xref reader, repair, incremental and full writer).
  - New `organize` crate: page operations and info edits.
  - Engine: editing with undo/redo and save. CLI: `edit`.
  - UI: organize toolbar with multi-select, ⌘Z/⇧⌘Z/⌘S/⇧⌘S, Edit menu, editable Description properties, a dot on tabs with unsaved changes, and a save prompt on close and quit. 13 new kittest tests.
  - Corpus: open, edit and save round-trip on 946 of 951 files. 182 of 182 sampled saved outputs pass `qpdf --check`.
  - Overall ≈ 5%.
- **2026-09-30 (session 2):**
  - Robustness sweep (963 of 983 files open, 0 crashes), text layer, find and select, tiles, web build, polish.
  - Overall ≈ 3–4%.
- **2026-09-30 (session 1):** planning complete; viewer vertical slice.
