# PdfCraft integration candidate, 2026-10-30

Base: 1d1cbc4ec92012767f5b48c92b451e15eb401c42

Validation: combined GitHub CI passed on bdf56209ce6a22980ef488eec92a55073f5a4e18 (Linux and Windows, run 41038860617). No fresh manual GUI pass has been performed. The subsequent architectural-guard additions require a fresh run.
All included source heads are preserved as merge parents, retaining original authorship. No upstream PR was merged. Conflicts were resolved after the user requested inclusion despite merge conflicts.

## Review follow-up and withheld behavior

Review 5712795161 identified combined-behavior gaps after the green baseline at 673178b.
The follow-up is pending fresh hosted validation; earlier results below are historical evidence.

- Backdrop detection uses a shared strict 64 KiB decoding budget, including intermediate filter outputs, and caps content-stream entries. A fill stream executed more than once on the page is treated as ambiguous and is not replaced; source artwork stays untouched.
- Monochrome downsampling no longer guesses Indexed/custom color-space polarity. Those images may be recompressed losslessly but retain their resolution. A local 2-by-2 patch of mixed output samples conservatively retains the entire source image, even when white margins dominate it.
- **Withheld from #4180:** selected-row alignment of existing PDF paragraphs. The UI disables it for a text selection and the editing API explicitly refuses per-row alignment intent before mutation. Whole-paragraph alignment and the independently persisted alignments of added text remain available. Prior immediate-row tests are replaced by safe-refusal tests; no cross-session alignment guarantee is claimed for the withheld feature.
- The custom JBIG2 encoder still needs separate representative independent-decoder qualification. No codec corruption defect has been established by this review. No fresh native manual GUI acceptance pass is claimed.

## Included
- [#4304](https://github.com/storytold/pdfcraft/pull/4304): M11: PDF Optimizer stores black-and-white images as CCITT Group 4 (optimize.mono-compression) (1b7164cab1fc187c5385c30f94449542fe284280)
- [#4335](https://github.com/storytold/pdfcraft/pull/4335): M11: black-and-white pictures stay one bit: Create PDF, the optimizer, and CCITT image export (210ba20fe69e701286bbfdbd924cfc01d9aaa9f4)
- [#4384](https://github.com/storytold/pdfcraft/pull/4384): M11: Monochrome Images ▸ Downsample: black-and-white images above 450 ppi are averaged down to 300, Group 4 scans included (1ad951e25bac86f77e75e060dcce34cc32a0f9a3)
- [#4404](https://github.com/storytold/pdfcraft/pull/4404): M11: the optimizer stores Group 3 fax as Group 4, and packs black-and-white RGB and two-entry indexed images to one bit (2d0a7b22411d4b61f130f6175cec918153858593)
- [#4179](https://github.com/storytold/pdfcraft/pull/4179): fix(ui): Ctrl+Z in an open paragraph undoes the document, not stale text (296b9300988fe3ef8d5bcdcb439705e80d6d7321)
- [#5300](https://github.com/storytold/pdfcraft/pull/5300): fix(ui): a press on the page belongs to what its one hit test finds, until the button is up (11bc8c56528fc3a820669d41c0eec29310cdad24)
- [#4719](https://github.com/storytold/pdfcraft/pull/4719): fix(comments): delete or edit one reply from its own menu, keeping the thread (adeb9c1cf2336c10639d8172325e708edf515856)
- [#4541](https://github.com/storytold/pdfcraft/pull/4541): M5: a moved comment is drawn where it goes while it is dragged (f0cd96e90bf696099b02201ac68f9ccb8a00c7d2)
- [#2815](https://github.com/storytold/pdfcraft/pull/2815): fix(edit): show backgrounds on pages that paint their own white backdrop (9aaac56417a2260338c7e7edb41615f1bb42b45e)
- [#4620](https://github.com/storytold/pdfcraft/pull/4620): feat(edit): Esc in an open paragraph selects its box, so Delete removes it (64fba7d177032170939861dc7eb51d8486a43264)
- [#3789](https://github.com/storytold/pdfcraft/pull/3789): Alt+Left / Alt+Right go back and forward through the view history (#3632) (08f3991ab9808b71fa015f22d49dbc4720044c78)
- [#3779](https://github.com/storytold/pdfcraft/pull/3779): Window title: mark unsaved changes, and set it on the web (#3640) (3850c9f52e073e51f8e4a1665b3ae36fae6f8e66)
- [#2960](https://github.com/storytold/pdfcraft/pull/2960): Keep lines with footnote references in paragraph reading order (#2929) (2a7e5bda268b3862b9b2329df16191473966c90e)
- [#5208](https://github.com/storytold/pdfcraft/pull/5208): M10: export a PDF to Markdown (#1595) (20d396d180a33139908a3a1fad6178ab77b90eff)
- [#4596](https://github.com/storytold/pdfcraft/pull/4596): fix(organize,print): join /Contents array pieces with a space (#3502) (d58d121170f2d650196263492c6c54a89aad39b3)
- [#3636](https://github.com/storytold/pdfcraft/pull/3636): Content streams: step over a malformed token instead of ending the page (43337c7fd5f7b5123efcebcc84de35c394726e7a)
- [#4494](https://github.com/storytold/pdfcraft/pull/4494): M2: page_render and render_preview report the render warnings next to the PNG (#4340) (5ada1249e9cdb98a438e26f8fe53c82aba9b8cc1)
- [#4013](https://github.com/storytold/pdfcraft/pull/4013): fix(engine): generate Protect keys and salts with getrandom (#4000) (c580b550555837ad6f2c1aa4152bdb5cb96cf337)
- [#5134](https://github.com/storytold/pdfcraft/pull/5134): fix(ui): keep the All tools rows clear of the scrollbar (#5133) (0c5119791db7de268258b9d25f1286b82b563f8a)
- [#2133](https://github.com/storytold/pdfcraft/pull/2133): Fix Pages sidebar select-all keyboard shortcut (#2052) (7227eb68ab383e47413f1a980f340f5132cf4144)
- [#2635](https://github.com/storytold/pdfcraft/pull/2635): Allow typed font sizes and Fill & Sign text colours (4b319bb3835709626c663b8d41b72d2f9574acc0)

## Added after conflict resolution
- [#4320](https://github.com/storytold/pdfcraft/pull/4320): M11: lossless JBIG2 for black-and-white images (optimize.mono-compression) (e18d1bfed38272a33fad1b79b7452494a9f4dfd6).
- [#4805](https://github.com/storytold/pdfcraft/pull/4805): M11: JBIG2 symbol coding for black-and-white images (optimize.mono-compression) (c7f39a4d9a341dcbdacaf3bc131ea2825f030b13).
- [#4325](https://github.com/storytold/pdfcraft/pull/4325): M6: calculation checks: formulas reading missing fields, scripts building names, shown on the Calculation order page (ebad7f849d9dc04f8aa819efd4cfb06460de3d79).
- [#4180](https://github.com/storytold/pdfcraft/pull/4180): M7: align the selected lines of a paragraph or text box on their own (5a86f35210ca98adb23ba53ff9d0871593b0cfdb).
- [#1952](https://github.com/storytold/pdfcraft/pull/1952): feat(forms): edit checkbox and radio export values (#231) (e19f9646906df51c287f9b049feb8199cd645996).
- [#2529](https://github.com/storytold/pdfcraft/pull/2529): M7: keep a blank line as a paragraph break (#2517) (55a933800b94b89c0caae001ec48609cd5f70256).

Additional previously reviewed parity fixes: #2133, #2635 and #1952.
Excluded held PRs #4648, #2877 and #3762 were not included.
Source reviews and individual hosted CI do not establish combined-build safety; the integration gate must pass.

Integration-specific changes: preserve hard/soft wrap metadata together with per-row alignment; share CCITT/JBIG2 export paths with the 32 MiB globals cap retained; keep downsampling and RGB ownership guards with JBIG2 encoding; retain all translation keys and both sets of regression tests.

## Per-PR regression evidence

Evidence: [Linux job 124006360624](https://github.com/FredApps/pdfcraft/actions/runs/41038860617/job/124006360624) on bdf56209ce6a22980ef488eec92a55073f5a4e18: 5,627 passed, 15 skipped. The Windows leg of the same run also passed.

For each source-head diff above, the added `#[test]` function names were matched against PASS/SKIP entries in the combined Linux log. Every listed source regression was found passing, none skipped or missing. Counts overlap because stacked PRs carry their prerequisites; they must not be summed as distinct tests. This records source regression coverage, not manual feature acceptance.

- #4304: 11/11 added source regression names passed; 0 missing, 0 skipped.
- #4320: 21/21 added source regression names passed; 0 missing, 0 skipped.
- #4325: 18/18 added source regression names passed; 0 missing, 0 skipped.
- #4335: 19/19 added source regression names passed; 0 missing, 0 skipped.
- #4384: 39/39 added source regression names passed; 0 missing, 0 skipped.
- #4404: 50/50 added source regression names passed; 0 missing, 0 skipped.
- #4805: 36/36 added source regression names passed; 0 missing, 0 skipped.
- #5300: 18/18 added source regression names passed; 0 missing, 0 skipped.
- #4719: 6/6 added source regression names passed; 0 missing, 0 skipped.
- #4541: 5/5 added source regression names passed; 0 missing, 0 skipped.
- #2815: 35/35 added source regression names passed; 0 missing, 0 skipped.
- #4620: 1/1 added source regression names passed; 0 missing, 0 skipped.
- #2529: 35/35 added source regression names passed; 0 missing, 0 skipped.
- #3789: 5/5 added source regression names passed; 0 missing, 0 skipped.
- #3779: 4/4 added source regression names passed; 0 missing, 0 skipped.
- #2960: 1/1 added source regression names passed; 0 missing, 0 skipped.
- #5208: 2/2 added source regression names passed; 0 missing, 0 skipped.
- #4596: 1/1 added source regression names passed; 0 missing, 0 skipped.
- #3636: 3/3 added source regression names passed; 0 missing, 0 skipped.
- #4494: 4/4 added source regression names passed; 0 missing, 0 skipped.
- #4013: 4/4 added source regression names passed; 0 missing, 0 skipped.
- #4180: 54/54 added source regression names passed; 0 missing, 0 skipped.
- #4179: 20/20 added source regression names passed; 0 missing, 0 skipped.
- #5134: the source diff adds no test function. Combined gates passed; no new manual scrollbar acceptance is claimed.
- #2133: 1/1 added source regression names passed; 0 missing, 0 skipped.
- #2635: 6/6 added source regression names passed; 0 missing, 0 skipped.
- #1952: 6/6 added source regression names passed; 0 missing, 0 skipped.

The conflict-specific tests `wrapped_rows_keep_alignment_separate_from_hard_break_markers` and `black_and_white_images_become_jbig2` (including the CCITT-to-JBIG2 conversion/render assertions) passed in that same run. Those counts describe that baseline. In the review follow-up, unsupported existing-row alignment success tests are replaced by safe-refusal tests; the corresponding feature is withheld. Paragraph-boundary, field-calculation and button-export coverage is retained.

## Protect dependency check

The integration base main commit 1d1cbc4ec92012767f5b48c92b451e15eb401c42 already locks registry `getrandom` 0.4.3 (checksum 300e883d756b2e4ec94e02791f39b04b522276138852cfc41d9fb7e904106099). #4013 makes it a direct engine dependency rather than introducing an unrecorded package. The combined dependency-policy and asset gates passed. The fail-closed randomness regression also passed.
