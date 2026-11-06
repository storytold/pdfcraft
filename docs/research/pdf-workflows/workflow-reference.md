# Workflow research and verification reference

This reference connects archived workflow research with PdfCraft's existing feature catalogue. It is not a roadmap, a set of implementation assignments, or an approval to introduce features, storage formats, dependencies or architecture. Planning remains in the repository's existing `ROADMAP.md` and `parity/` records.
The catalogue and roadmap snapshot is [`924c4da4`](https://github.com/storytold/pdfcraft/tree/924c4da4ac2879ac9107c6c82e20ac6be5570f0a), dated 2026-10-16. Engine and automation names were inspected separately at [`3f044b62`](https://github.com/storytold/pdfcraft/tree/3f044b6263aa6605c3aa44ca6d065e1ab37d13b6), dated 2026-10-13. Catalogue status means the status recorded at that revision; it is not an independent verification verdict. No implementation tests, benchmarks, competitor sessions or conformance checks were executed for this reference.
The [competitive research digest](competitive-research.md) provides the archived context. The [source register](source-register.csv) records source identifiers, URLs, archive dates and verification limits. Links below are locators for the retained research, not fresh confirmations of vendor behavior. The [feature mapping](feature-mapping.csv) contains the corresponding catalogue records. B01–B20 are reference section identifiers, not a second task or priority system.
Evaluation dimensions name what an observation describes; they do not prescribe targets or new acceptance gates. Availability, structural validity, visual fidelity, interoperability and human usability are distinct evidence. Negative or incomplete observations remain distinguishable from successful checks.

## B01 — Rendering and output fidelity

**Recorded baseline.** The checklist records six mapped rendering features as partial. Its knockout entry is marked shipped, but cites signing tests and signed-save notes; the transparency entry describes knockout rendering as incomplete. Those are inconsistent catalogue references, not evidence from a fresh rendering test. The current progress log and gap 12 document existing `cargo xtask render-oracle` and `cargo xtask text-oracle` tools, including pixel disagreements and reading-order measurements. These tools report differences; their output is not a fidelity certification.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `view.render-pages` | partial | M2 |
| `view.render-shadings-patterns` | partial | M2 |
| `view.render-transparency` | partial | M2 |
| `view.render-knockout` | shipped | M11 |
| `view.render-type3` | partial | M2 |
| `view.render-cid-fonts` | partial | M2 |
| `view.render-image-codecs` | partial | M2 |

**Archived research question.** The archived viewing and conversion studies distinguish whether a file opens, whether its text can be extracted, and whether the visible page matches a reference. These observations concern different kinds of fidelity.

**Evaluation dimensions.** Pixel differences and their locations; glyph identity and reading order; clipping and transparency; independent-reader outcomes; oracle version and tolerance; unsupported or incomplete analysis.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/render/src/raster.rs::renders_a_blue_rectangle`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/render/src/raster.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf); [SR007: Adobe Acrobat — Acrobat product help index](https://helpx.adobe.com/acrobat/user-guide.html). Verification state and date are in the source register.

## B02 — Existing-text editing

**Recorded baseline.** Text editing and RTL/CJK editing are partial. The mapped rotated-text and tag-preservation entries remain planned. Inspected source exposes text_edit, text_edit_layout, text_fonts and text_font_import; this does not establish general support for every font or script.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `edit.text-edit` | partial | M7 |
| `edit.rtl-cjk-editing` | partial | M7 |
| `edit.rotated-text-editing` | planned | M7 |
| `edit.keep-tags-on-edit` | planned | M7 |

**Archived research question.** The archived editing studies ask whether correcting text preserves its font, baseline, writing order, nearby content and document structure. A visually acceptable edit and a semantically faithful edit are separate observations.

**Evaluation dimensions.** Extracted Unicode and logical order; shaping and missing-glyph behavior; retained font/style; neighbouring content; selection/caret behavior; saved/reopened layout; unsupported transformations.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/edit/src/tests.rs::paragraphs_are_found_and_rewrapped`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/edit/src/tests.rs); [`crates/edit/src/bidi.rs::hebrew_sentences_numbers_latin_and_brackets_round_trip`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/edit/src/bidi.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf); [SR007: Adobe Acrobat — Acrobat product help index](https://helpx.adobe.com/acrobat/user-guide.html). Verification state and date are in the source register.

## B03 — Document comparison

**Recorded baseline.** Text comparison, visual comparison, PDF reports and side-by-side views are shipped in the current checklist. The side-by-side notes describe an old-tab requirement, a minimum window width of 1000 px and offset-based synchronization. They identify content-aligned scrolling and retaining comparison after the old tab closes as limitations. Filters, selected ranges and scanned comparison remain planned.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `misc.compare-text` | shipped | M12 |
| `misc.compare-visual` | shipped | M12 |
| `misc.compare-report` | shipped | M12 |
| `misc.compare-side-by-side` | shipped | M12 |
| `misc.compare-filters` | planned | M12 |
| `misc.compare-page-ranges` | planned | M12 |
| `misc.compare-scanned` | planned | M12 |

**Archived research question.** Archived comparison studies distinguish a textual change list, raster differences, overlays and synchronized old/new context. The research question is how a reviewer associates a reported difference with both document locations.

**Evaluation dimensions.** Difference type and location; page alignment with insertions or unequal sizes; zoom/scroll correspondence; navigation counts; report destinations; identical-input outcomes; input immutability.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/compare/src/lib.rs::insertions_deletions_and_replacements`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/compare/src/lib.rs); [`crates/engine/src/tests.rs::comparing_two_versions_of_a_document`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/engine/src/tests.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR018: Bluebeam Revu — Documentation locator: compare documents vs overlay pages](https://support.bluebeam.com/revu/features/compare-documents-vs-overlay-pages.html); [SR030: Bluebeam Revu — Documentation locator: compare documents](https://support.bluebeam.com/user-manual/menus/document/compare-documents.html); [SR002: ABBYY FineReader — User guide index: verification, comparison and automation](https://help.abbyy.com/en-us/finereader/16/). Verification state and date are in the source register.

## B04 — Recognition proofing and correction

**Recorded baseline.** Recognition of pages, ranges and multiple files is shipped; editable-text output is partial; correction of low-confidence suspects is planned. Inspected source includes ocr_recognize, ocr_recognize_files and ocr_status. These operations alone do not establish a proofing workflow.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `ocr.correct-recognized-text` | planned | M10 |
| `ocr.editable-text-output` | partial | M10 |
| `ocr.recognize-text` | shipped | M10 |
| `ocr.recognize-range` | shipped | M10 |
| `ocr.recognize-multiple` | shipped | M10 |

**Archived research question.** The archived recognition study separates image/text/zoom synchronization, navigation among uncertain material, correction, skipping and human-reviewed state. Confidence from an engine and a human review decision are distinct evidence.

**Evaluation dimensions.** Word-to-image correspondence; confidence provenance or absence; correction in extracted text after save/reopen; retained scan appearance; skipped versus reviewed state; stale recognition results; keyboard reachability.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/engine/tests/ocr_edit.rs::editing_an_ocr_word_covers_the_scan_and_shows_the_new_text`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/engine/tests/ocr_edit.rs); [`crates/engine/src/tests.rs::recognize_text_makes_a_scanned_page_searchable`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/engine/src/tests.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR002: ABBYY FineReader — User guide index: verification, comparison and automation](https://help.abbyy.com/en-us/finereader/16/); [SR031: Bluebeam Revu — Documentation locator: ocr](https://support.bluebeam.com/user-manual/menus/document/ocr.html). Verification state and date are in the source register.

## B05 — Recognition languages and scan preparation

**Recorded baseline.** Language support is partial; orientation detection, deskew and pluggable engines are planned. Camera-image cleanup provides partial despeckle, background-removal and perspective capabilities. The notes do not establish equivalent preprocessing of arbitrary scanned pages.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `ocr.ocr-languages` | partial | M10 |
| `ocr.rotation-detection` | planned | M10 |
| `ocr.deskew` | planned | M10 |
| `ocr.ocr-engines` | planned | M10 |
| `ocr.despeckle` | partial | M10 |
| `ocr.background-removal` | partial | M10 |
| `ocr.camera-perspective` | partial | M10 |

**Archived research question.** The archived scan studies distinguish selected language from the language actually recognized and distinguish camera perspective correction from text orientation, deskew and image cleanup.

**Evaluation dimensions.** Actual backend/language; model availability and provenance; rotation/skew geometry; text accuracy by script; annotation/search coordinate agreement; pages with existing text; duplicate-layer behavior; explicit unsupported options.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/ui-egui/tests/ocr.rs::recognize_text_dialog_adds_searchable_text`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/ocr.rs); [`crates/engine/src/camera_clean.rs::a_shadow_and_a_yellow_cast_become_white_paper`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/engine/src/camera_clean.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR002: ABBYY FineReader — User guide index: verification, comparison and automation](https://help.abbyy.com/en-us/finereader/16/); [SR120: PDF-XChange — Documentation locator: pdf xchange ocr](https://www.pdf-xchange.com/pdf-xchange-ocr); [SR133: PDF24 — Documentation locator: ocr pdf](https://tools.pdf24.org/en/ocr-pdf). Verification state and date are in the source register.

## B06 — Reusable batch processing

**Recorded baseline.** Action creation and execution are shipped; management/exchange and prompt-user steps remain planned. The inspected 2026-10-13 actions.rs has 12 step kinds, and action_list/action_run are present. This is an older source observation, not a new inspection of the current runner.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `misc.action-wizard-run` | shipped | M13 |
| `misc.action-wizard-create` | shipped | M13 |
| `misc.action-wizard-manage` | planned | M13 |
| `misc.action-prompt-steps` | planned | M13 |

**Archived research question.** Archived workflow studies describe named sequences, ordered steps, portable definitions and per-file outcomes. They distinguish a saved process definition from the outcomes of a particular execution.

**Evaluation dimensions.** Definition/version round-trip; step order and arguments; input/output identity; occupied output names; individual failure reporting; partial batch outcomes; cancellation; path confinement; secret handling in reports.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/ui-egui/tests/actions.rs::desktop_actions_preserve_duplicate_names_and_results_from_earlier_runs`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/actions.rs); [`crates/automation/tests/automation.rs::actions_through_tools`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/automation/tests/automation.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR056: Foxit PDF Editor — Documentation locator: add custom commands to custom actions](https://www.foxit.com/resource-hub/user-guide/foxit-pdf-editor-for-windows/add-custom-commands-to-custom-actions/); [SR172: Stirling-PDF — Automation pipeline documentation](https://docs.stirlingpdf.com/Configuration/Automation/Pipeline/). Verification state and date are in the source register.

## B07 — Document structure and accessibility authoring

**Recorded baseline.** Text-only autotag, Tags/property editing, sibling reading-order edits and artifact marking are partial. The current artifact entry covers reachable indirect leaf tags owning direct MCIDs through the panel and accessibility_mark_tag_artifact. Arbitrary painted selection, direct dictionaries and whole tagged subtrees remain outside that entry. accessibility_tags is described as paginated and filterable.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `a11y.autotag` | partial | M12 |
| `a11y.tags-panel` | partial | M12 |
| `a11y.tag-properties` | partial | M12 |
| `a11y.reading-order-tool` | partial | M12 |
| `a11y.order-panel` | partial | M12 |
| `a11y.table-editor` | planned | M12 |
| `a11y.create-tag-from-selection` | planned | M12 |
| `a11y.artifact-marking` | partial | M12 |

**Archived research question.** Archived accessibility studies distinguish automatic classification, actual structure-tree changes, reading order and human interpretation. A visual panel, a structurally valid parent tree and an accessible reading experience are different observations.

**Evaluation dimensions.** MCID/parent-tree consistency; content ownership; tag and reading order; alternate text; direct/indirect dictionary coverage; malformed or ambiguous structure; appearance after artifact marking; saved/reopened structure; manual-check findings.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/automation/tests/automation.rs::autotag_preview_proposes_text_without_changing_the_pdf`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/automation/tests/automation.rs); [`crates/ui-egui/tests/accessibility.rs::tags_panel_edits_a_tag_property_and_undoes_it`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/accessibility.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR108: PDF Association — Accessible PDF standards and reference material](https://pdfa.org/iso-14289-2-pdf-ua-2-the-gold-standard-for-accessibility-in-pdf-2-0-has-arrived/); [SR110: PDF Association — Accessible PDF standards and reference material](https://pdfa.org/wtpdf/); [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf). Verification state and date are in the source register.

## B08 — Preflight and archival validation

**Recorded baseline.** PDF/A verification and conversion are partial. Preflight analysis, result presentation and reports remain planned. Inspected source exposes pdfa_verify, pdfa_convert and print_readiness; their availability is not proof of full conformance validation.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `optimize.pdfa-validate` | partial | M11 |
| `optimize.pdfa-convert` | partial | M11 |
| `optimize.preflight` | planned | M11 |
| `optimize.preflight-results` | planned | M11 |
| `optimize.preflight-reports` | planned | M11 |

**Archived research question.** The archived standards studies distinguish declared conformance, implemented structural checks, conversion changes and independent validation. A repairable finding is different from a condition the software cannot evaluate.

**Evaluation dimensions.** Profile and rule coverage; page/object identification; unsupported checks; fonts and output intents; changes made by conversion; remaining findings after save/reopen; agreement and disagreement with an identified independent validator.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/preflight/src/tests.rs::verify_reports_what_breaks_pdfa`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/preflight/src/tests.rs); [`crates/automation/tests/automation.rs::pdfa_through_tools`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/automation/tests/automation.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf); [SR076: LibreOffice Draw — PDF export and security help index](https://help.libreoffice.org/latest/en-US/text/shared/01/ref_pdf_export.html); [SR105: PDF Association — Product or project documentation entry](https://pdfa.org). Verification state and date are in the source register.

## B09 — Redaction and assurance evidence

**Recorded baseline.** The mapped excision and verification features are shipped. Redaction properties are also shipped, including overlay/mark styling, persistent defaults, session-only changes and cancel behavior. Pattern locales, partial words and folder redaction remain planned; code sets are partial.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `protect.redaction-verification` | shipped | M8 |
| `protect.redact-glyph-removal` | shipped | M8 |
| `protect.redact-image-pixels` | shipped | M8 |
| `protect.redact-vectors` | shipped | M8 |
| `protect.redact-annots-fields` | shipped | M8 |
| `protect.redact-struct-tree` | shipped | M8 |
| `protect.redact-pattern-locales` | planned | M8 |
| `protect.redact-partial-words` | planned | M8 |
| `protect.redact-folder` | planned | M13 |
| `protect.redaction-properties` | shipped | M8 |
| `protect.redaction-codes` | partial | M8 |

**Archived research question.** Archived redaction studies distinguish a visible cover from removal of underlying content and distinguish a scoped verification result from an unlimited security guarantee. A report containing removed text can itself disclose sensitive content.

**Evaluation dimensions.** Search/extraction after save/reopen; image/vector/annotation/structure paths covered; hidden-information treatment; input/output hashes; unsupported constructs; false success on a cover-only control; refusal/cancellation behavior; sensitive data in exported records.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/redact/src/tests.rs::unreadable_content_fails_closed`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/redact/src/tests.rs); [`crates/ui-egui/tests/ui_group/redact.rs::marking_applying_and_clearing`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/ui_group/redact.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR034: Bluebeam Revu — Documentation locator: redaction](https://support.bluebeam.com/user-manual/menus/edit/redaction.html); [SR097: Nitro PDF Pro — Documentation locator: nitros complete guide to secure automated pdf redaction](https://www.gonitro.com/resources/nitros-complete-guide-to-secure-automated-pdf-redaction); [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf). Verification state and date are in the source register.

## B10 — Digital signatures and validation evidence

**Recorded baseline.** The mapped B-B, PKCS #12, native Windows/macOS identity and certification capabilities are shipped. Timestamp preferences, PKCS #11 and FieldMDP are planned; B-T/LTV/B-LTA and online revocation support are partial. Inspected source exposes sign_document, sign_list and sign_trust.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `sign.cades-pades-bb` | shipped | M9 |
| `sign.pkcs12` | shipped | M9 |
| `sign.windows-cert-store` | shipped | M9 |
| `sign.macos-keychain` | shipped | M9 |
| `sign.certify` | shipped | M9 |
| `sign.timestamp-servers` | planned | M9 |
| `sign.pkcs11` | planned | M9 |
| `sign.field-mdp` | planned | M9 |
| `sign.pades-bt` | partial | M9 |
| `sign.ltv` | partial | M9 |
| `sign.pades-blta` | partial | M9 |
| `sign.ocsp-crl` | partial | M9 |

**Archived research question.** Archived signing studies distinguish document integrity, signer trust, permitted changes, timestamp evidence and long-term validation material. A local signing workflow is different from a hosted signature-request service.

**Evaluation dimensions.** Covered revision; permitted/disallowed changes; intact versus trusted status; timestamp presence and trust; revocation-evidence freshness; requested versus attained level; network-consent state; private-key custody; reopened independent validation.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/sign/tests/crypto.rs::signatures_round_trip_for_every_key_type`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/sign/tests/crypto.rs); [`crates/sign/tests/pdf.rs::signing_then_validating_with_and_without_trust`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/sign/tests/pdf.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR028: Bluebeam Revu — Documentation locator: sign seal](https://support.bluebeam.com/user-manual/menus/batch/sign-seal.html); [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf); [SR007: Adobe Acrobat — Acrobat product help index](https://helpx.adobe.com/acrobat/user-guide.html). Verification state and date are in the source register.

## B11 — Forms, events and scripting

**Recorded baseline.** Custom scripts, event order and calculation order are shipped. Form submission and the mapped general JavaScript action are planned; push-button actions are partial. Inspected source includes js_run, js_document_scripts, form_set_script, form_actions, form_set_actions and js_enabled.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `form.custom-scripts` | shipped | M6 |
| `form.event-order` | shipped | M6 |
| `form.calculation-order` | shipped | M6 |
| `form.submit-form` | planned | M6 |
| `edit.action-javascript` | planned | M6 |
| `form.push-buttons` | partial | M6 |

**Archived research question.** Archived form studies separate compatibility with installed forms from authoring tools and separate field-event order from document-level actions. Scripting availability does not imply filesystem or network privileges.

**Evaluation dimensions.** Trigger timing; validation/calculation/format order; committed values after save/reopen; enabled/disabled script state; resource bounds; malformed scripts; unwanted external effects; cancellation and refusal outcomes.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/engine/src/tests.rs::form_javascript_validates_calculates_and_formats`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/engine/src/tests.rs); [`crates/forms/src/tests.rs::formats_validation_and_calculations_run_like_acrobat`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/forms/src/tests.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR012: Adobe Acrobat — Acrobat JavaScript API reference](https://opensource.adobe.com/dc-acrobat-sdk-docs/library/jsapiref/index.html); [SR013: Adobe Acrobat — Acrobat JavaScript developer guide](https://opensource.adobe.com/dc-acrobat-sdk-docs/library/jsdevguide/index.html); [SR043: Firefox / pdf.js — Form scripting API implementation reference](https://github.com/mozilla/pdf.js/blob/master/src/scripting_api/aform.js). Verification state and date are in the source register.

## B12 — Measurement and scale interpretation

**Recorded baseline.** Distance and scale are partial; perimeter, area, snapping, information display and CSV export are shipped. Current mapped measurement entries are assigned to M12. Inspected source includes the measure_* tool family.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `comment.measure-distance` | partial | M12 |
| `comment.measure-scale` | partial | M12 |
| `comment.measure-perimeter` | shipped | M12 |
| `comment.measure-area` | shipped | M12 |
| `comment.measure-snap` | shipped | M12 |
| `comment.measure-info-panel` | shipped | M12 |
| `comment.measure-export` | shipped | M12 |

**Archived research question.** Archived technical-review studies distinguish measurement geometry from unit formatting, calibration and interchange. Reading a saved measurement is different from assuming every imported scale is supported.

**Evaluation dimensions.** Known dimensions and numeric tolerance; selected viewport and scale; compound/fractional formats; rotated/cropped pages; imported unsupported formats; self-intersecting boundaries; saved/reopened values; CSV escaping and spreadsheet safety.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/automation/tests/automation.rs::measurements_calibrate_draw_save_reopen_and_export`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/automation/tests/automation.rs); [`crates/measure/src/tests.rs::viewport_precedence_first_point_user_unit_and_roundtrip`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/measure/src/tests.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf); [SR107: PDF Association — Foxit PDF Editor 2026.2 release announcement](https://pdfa.org/foxit-pdf-editor-20262-release-notes/). Verification state and date are in the source register.

## B13 — Page assembly and document composition

**Recorded baseline.** Combine, selected page ranges, sorting/grid previews, every-N/bookmark splitting and field-name conflict handling are shipped. Structure-tree merging, multiple-file split and the mapped naming refinements remain planned. Existing combine and page tools were present in the inspected source.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `organize.combine-files` | shipped | M4 |
| `organize.combine-choose-pages` | shipped | M4 |
| `organize.combine-sort` | shipped | M4 |
| `organize.combine-thumbnails` | shipped | M4 |
| `organize.field-name-conflicts` | shipped | M4 |
| `organize.split-every-n` | shipped | M4 |
| `organize.split-by-bookmarks` | shipped | M4 |
| `organize.struct-tree-merge` | planned | M12 |
| `organize.split-multiple-files` | planned | M13 |
| `organize.split-naming` | planned | M4 |

**Archived research question.** Archived composition studies examine how users review source pages, order and destination before a transfer. Visible selection, the captured source revision and the resulting document are separate pieces of evidence.

**Evaluation dimensions.** Page order; source/target identity; unsaved-source handling; links, fields and bookmarks; size mismatch; field-name conflicts; structure preservation; source immutability; undo; saved/reopened destinations.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/organize/src/tests.rs::combine_concatenates_with_a_bookmark_per_file`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/organize/src/tests.rs); [`crates/ui-egui/tests/editing.rs::combine_files_takes_chosen_pages_in_the_order_listed`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/editing.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf); [SR007: Adobe Acrobat — Acrobat product help index](https://helpx.adobe.com/acrobat/user-guide.html). Verification state and date are in the source register.

## B14 — Creation and conversion fidelity

**Recorded baseline.** Office import and spreadsheet export are planned; DOCX and presentation export are partial. DOCX notes record package validation and DrawingML picture anchors on positioned pages, with that positioned output still unverified in Word. Package validity does not establish visual fidelity.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `create.from-office` | planned | M10 |
| `create.export-docx` | partial | M10 |
| `create.export-xlsx` | planned | M10 |
| `create.export-pptx` | partial | M10 |

**Archived research question.** Archived conversion studies distinguish reflowed editable output, positioned output, raster output and native PDF edits. Their suitability depends on what information the user needs to retain.

**Evaluation dimensions.** Text and table semantics; headers/footers; page dimensions; images and vector rules; paint order; exact font matching; package validity; reopened rendering; independent application/version; explicit unsupported input and output.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/export/src/lib.rs::word_does_not_invent_borders_for_aligned_invoice_fields`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/export/src/lib.rs); [`crates/export/src/pptx.rs::pptx_is_a_presentationml_package`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/export/src/pptx.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR076: LibreOffice Draw — PDF export and security help index](https://help.libreoffice.org/latest/en-US/text/shared/01/ref_pdf_export.html); [SR007: Adobe Acrobat — Acrobat product help index](https://helpx.adobe.com/acrobat/user-guide.html). Verification state and date are in the source register.

## B15 — Document performance and resource use

**Recorded baseline.** Smooth 500-page scrolling is shipped. Performance budgets and lazy loading of very large files are planned. Existing notes describe whole-file input and remaining save-memory peaks; they do not establish bounded performance for every document.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `view.large-document-performance` | shipped | M3 |
| `misc.performance-budgets` | planned | M14 |
| `core.lazy-loading` | planned | M1 |

**Archived research question.** Archived performance studies distinguish open time, first useful page, interaction latency and job completion. Input byte size and page count alone do not describe rendering complexity.

**Evaluation dimensions.** Release build and hardware; corpus generation; fresh-process runs and variance; peak resident memory; open/edit/save timings; UI-thread stalls; cache state; cancellation latency; bounds on hostile dimensions; output validity after reopen.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/ui-egui/tests/perf.rs::scrolling_a_500_page_document_stays_within_the_frame_budget`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/perf.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf); [SR099: Okular — Project source and technical documentation reference](https://github.com/KDE/okular). Verification state and date are in the source register.

## B16 — Search across document collections

**Recorded baseline.** Advanced current-document search is partial; folder, Boolean and indexed search are planned. Inspected source includes text_find and extraction. Mapped search entries belong to M12.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `view.advanced-search` | partial | M12 |
| `view.search-folder` | planned | M12 |
| `view.search-boolean` | planned | M12 |
| `view.search-index` | planned | M12 |

**Archived research question.** Archived search studies distinguish current-file matching, collection traversal, persistent indexing and navigation back to a location. A result count is incomplete without the set of files actually searched.

**Evaluation dimensions.** Query semantics; searched/excluded/failed files; path aliases and cycles; root confinement; result and cache bounds; stale locations after edits; location after reopen; cancellation; truncation and incomplete-result reporting.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/ui-egui/tests/viewer.rs::advanced_search_lists_results_with_context`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/viewer.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR177: SumatraPDF — Command palette and navigation prefixes](https://www.sumatrapdfreader.org/docs/Command-Palette); [SR007: Adobe Acrobat — Acrobat product help index](https://helpx.adobe.com/acrobat/user-guide.html). Verification state and date are in the source register.

## B17 — Keyboard and assistive-technology interaction

**Recorded baseline.** App screen-reader support is partial and keyboard-only operation is planned. Reading colors are shipped; current notes cover annotation appearances and highlight legibility. Existing labels and display-color behavior do not establish completion of every task with assistive technology.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `a11y.app-screen-reader` | partial | M3 |
| `a11y.keyboard-only` | planned | M14 |
| `a11y.replace-page-colors` | shipped | M12 |

**Archived research question.** Archived interaction studies distinguish the accessibility tree, keyboard focus and what a platform screen reader announces. Successful automated tree inspection and a successful human task are different evidence.

**Evaluation dimensions.** Names, roles, values and states; complete focus path; focus restoration; error/completion announcements; disabled-state explanations; selection after page changes; non-color information; platform/screen-reader version; manual versus automated findings.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/ui-egui/tests/control.rs::inspect_and_click_by_label_and_id`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/control.rs); [`crates/automation/tests/automation.rs::reading_colors_through_tools`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/automation/tests/automation.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR099: Okular — Project source and technical documentation reference](https://github.com/KDE/okular); [SR104: Okular — Project source and technical documentation reference](https://raw.githubusercontent.com/KDE/okular/master/doc/index.docbook); [SR108: PDF Association — Accessible PDF standards and reference material](https://pdfa.org/iso-14289-2-pdf-ua-2-the-gold-standard-for-accessibility-in-pdf-2-0-has-arrived/). Verification state and date are in the source register.

## B18 — Preferences and capability disclosure

**Recorded baseline.** The preferences dialog is partial; settings search, admin defaults and RTL UI are planned. Existing categories include language, document/view, identity, Fill & Sign and JavaScript. Current notes describe XDG migration that preserves configured preferences; they do not establish a complete signature-preferences category.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `misc.preferences-dialog` | partial | M14 |
| `misc.prefs-search` | planned | M14 |
| `misc.admin-preferences` | planned | M14 |
| `misc.rtl-ui` | planned | M14 |

**Archived research question.** Archived shell and deployment studies distinguish application preferences, document overrides, session state and managed policy. A visible option is different from an available runtime capability.

**Evaluation dimensions.** Scope and precedence; default versus override; persistence across restart/reopen; malformed stored values; migration preservation; policy-locked state; cancellation of drafts; consistency among supported entry points.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/ui-egui/tests/javascript.rs::document_scripts_and_preferences`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/javascript.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR080: Master PDF Editor — Workspace navigation and object inspection](https://code-industry.net/masterpdfeditor-help/workspace-master-pdf-editor/); [SR177: SumatraPDF — Command palette and navigation prefixes](https://www.sumatrapdfreader.org/docs/Command-Palette). Verification state and date are in the source register.

## B19 — Print preparation and color behavior

**Recorded baseline.** Printing, n-up/booklet and previews exist. Saved print presets and the mapped output-preview/color-production features are planned. Inspected source includes print_readiness; that inspection path is not a full production Preflight.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `organize.print-presets` | planned | M10 |
| `print.output-preview` | planned | M11 |
| `print.color-warnings` | planned | M11 |
| `print.convert-colors` | planned | M11 |
| `print.color-management` | planned | M11 |

**Archived research question.** Archived print studies distinguish display-only reading colors, print-preview appearance, color conversion and the output intent. An ICC profile or a printer selection has meaning only with an identified processing path.

**Evaluation dimensions.** Page order and dimensions; persistent print options; preview/output correspondence; fonts and color-space findings; profile provenance and rendering intent; unsupported separations; device/platform differences; reopened print-ready output.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites the per-feature evidence recorded in its entries. These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR076: LibreOffice Draw — PDF export and security help index](https://help.libreoffice.org/latest/en-US/text/shared/01/ref_pdf_export.html); [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf). Verification state and date are in the source register.

## B20 — Document identity, history and failure outcomes

**Recorded baseline.** The mapped undo, atomic batch-edit, autosave, crash-recovery and unsaved-change capabilities are shipped. Current recovery notes cover instance locks that prevent another running instance from offering/discarding live autosaves. Browser recovery and multiple windows remain planned.

| Existing parity ID | Recorded status | Existing milestone |
|---|---|---|
| `core.undo-redo` | shipped | M4 |
| `core.undo-after-save` | shipped | M4 |
| `core.atomic-batch-edits` | shipped | M4 |
| `core.autosave` | shipped | M4 |
| `core.crash-recovery` | shipped | M4 |
| `core.unsaved-changes-prompt` | shipped | M4 |
| `core.web-recovery` | planned | M4 |
| `view.multiple-windows` | planned | M14 |

**Archived research question.** Archived document-ownership studies distinguish the active tab from the document captured by a long-running operation and distinguish a staged candidate from an accepted save. Those studies are not evidence of a defect in current PdfCraft.

**Evaluation dimensions.** Captured document/revision; destination identity; close-during-save outcome; external file replacement; write failure; coherent undo/redo; unrelated-document preservation; recovery binding; saved/reopened accepted state; cancellation and reported non-interruptible stages.

**Existing implementation evidence.** The [pinned checklist](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/parity/acrobat-features.toml) cites [`crates/engine/src/tests.rs::undo_and_redo_restore_exact_states`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/engine/src/tests.rs); [`crates/ui-egui/tests/recovery.rs::saving_or_closing_clears_the_entry_and_nothing_is_written_for_clean_documents`](https://github.com/storytold/pdfcraft/blob/924c4da4ac2879ac9107c6c82e20ac6be5570f0a/crates/ui-egui/tests/recovery.rs). These are existing evidence references, not checks rerun for this document.

**Archived source locators.** [SR014: Adobe Acrobat — ISO 32000-1:2008 PDF specification](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf); [SR099: Okular — Project source and technical documentation reference](https://github.com/KDE/okular). Verification state and date are in the source register.
