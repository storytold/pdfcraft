# Arabic text editing milestone: integration map

Updated 2026-11-02. These contributions form one Arabic editing
package with five reviewable logical parts. The proposed delivery is one branch
and one consolidated PR, preserving the original contributions and their evidence.
The combined package is published as [PR #5731](https://github.com/storytold/pdfcraft/pull/5731).
Publication does not imply acceptance, merge or release.

| Part | Contribution | Role in the combined workflow |
| --- | --- | --- |
| [#5602](https://github.com/storytold/pdfcraft/pull/5602) | Arabic decimal/group separators | Keep numeric runs intact during text recovery. |
| [#5603](https://github.com/storytold/pdfcraft/pull/5603) | Combining marks in copy/search | Keep mapped nonspacing marks with their visual base without moving glyph geometry. |
| [#5623](https://github.com/storytold/pdfcraft/pull/5623) | Source-font Arabic editing | Reuse supported complete embedded fonts, contextual shaping, Unicode mappings and source positions through repeated edits/save/reopen. |
| [#5693](https://github.com/storytold/pdfcraft/pull/5693) | Auto/LTR/RTL controls | Expose paragraph base direction independently of physical alignment through UI, engine, storage and automation. |
| Input follow-up in #5731 | [Interactive editing and composition](arabic-input.md) | Protect native composition keys and unfinished drafts, release the guard after native caret interruption, and expose bounded `ui.ime` delivery. |

The input follow-up uses egui's native caret, selection and shaping. It does not
introduce a second visual-order editor. Eight distinct native shell cases have
passing evidence across bounded runs. The user also confirmed physical Windows
Arabic typing, with joined multiline text observed in the native editor. This is
separate from provider-specific IME and browser acceptance.

A standalone input build demonstrated why the dependencies matter: typing worked,
but Apply selected a variable fallback before the document's valid static font.
The already-existing #5623 source-priority fix addresses that failure. Input-only
acceptance cannot stand in for acceptance of the combined source-editing workflow.

Proposed consolidated package:

1. Keep numeric-separator recovery (#5602) as a small logical part with its
   original regression and reference.
2. Keep mapped combining-mark copy/search (#5603), its renderer regressions and
   the existing `unicode-bidi` dependency edge as a second part.
3. Integrate the source-font milestone (#5623), preserving complete-source font
   priority, contextual-span completion, cluster Unicode ownership and its tests.
4. Retain the Auto/LTR/RTL controls (#5693), including engine/tool/storage paths.
   Preserve both direction helpers and #5623's shaping changes at their overlap.
   [Review 5730604263](https://github.com/storytold/pdfcraft/pull/5693#pullrequestreview-5730604263)
   is addressed in the consolidated package: explicit Auto/LTR/RTL
   changes and edits retaining saved non-Auto direction reject turned source
   baselines and legacy visual-order Indic fonts before mutation. The public-API
   regression fails before the guard and passes after it; scoped editor Clippy
   and WASM checks passed. Styled/explicit-direction justification and selected-row
   alignment refusals are documented. This is not a maintainer approval or merge.
5. Add the interactive input follow-up, including the bounded `ui.ime` control
   API and native composition/caret interruption regressions.

Keep the existing PRs open until the maintainer accepts the consolidated PR as
their replacement. Preserve original author metadata and link every source PR;
do not import unrelated old upstream merge, CI or asset changes. A shared branch
does not require collapsing the five parts into an opaque commit.

An earlier integration snapshot included upstream main `d7dd1de647ee` (2026-11-02),
with both roadmap histories retained. Before merging, complete Chrome input and
broader copy/search acceptance, and record precise provider/native-reader limitations. Local checks do not replace exact-head CI
and maintainer review. Historical source-PR test results
remain historical; they must not be represented as one new combined suite run.

At the 2026-11-02 GitHub snapshot, all four published PRs were open. #5602, #5603
and #5623 had merge conflicts; #5693 was mergeable but had the review follow-up
above. The input contribution is now included in #5731. Recheck the source PR
states and exact heads before merging.

A combined desktop build was also exercised with the user's Windows Arabic
keyboard: Arabic text plus `1378` was saved, and the saved PDF was reopened with
text readback and rendering checked. This is a bounded example using a supported font.
Chrome acceptance, broader runtime/provider coverage and independent native-reader
qualification remain pending. The consolidated PR is open for review;
it has not been merged.

The supported-font scope remains important. Arbitrary mixed-style source text,
full Arabic form appearances, missing-font completion, OCR/export interoperability
and universal OS IME qualification are not delivered by this bundle. Overall RTL
interface parity remains planned until its broader workflows have evidence.

Review 5735147265 follow-up: completed native click/drag interruptions release composition ownership at the common frame boundary, covering non-page text widgets while retaining page-specific resets. The component regression failed with the reset disabled and passes for single-line/multiline click/drag cases against pinned egui; this is not live form/provider/browser qualification. README links distinguish the historical core checkpoint from integrated direction controls. Preview and Apply now share paragraph replacement selection. The focused public-tool regression first reproduced different bounds and overflow for implicit width-plus-Auto-direction reflow, then passed after the correction. The synthetic paragraph includes both a soft wrap and a hard break; preview remains read-only and Apply retains the hard break. Forced LTR/RTL across these standard-font source rows still requires a uniform complete face. This closes the specific selector finding, without expanding the qualification claims above.


### Review 5737152311: replacement intent and saved breaks

Explicit replacement text now remains literal through styled rewriting, including
a draft equal to the source's space-joined `text`. Width-plus-direction edits can
therefore rejoin a paragraph whose existing breaks are all soft.

An unchanged direction-only draft carries an explicit source-break preservation
policy from the editor or automation caller. Styled layout captures the original
soft/hard classification and separator counts before modifying source runs, keeps
the visual rows and writes the corresponding PDF break markers. Automatic bidi
base selection still follows the logical paragraph across soft continuations.

Three additional public-tool regressions failed on the prior implementation and
pass after these corrections: all-soft implicit width/direction reflow, explicit
text equal to the flat source text, and direction-only visual-row/break-type
preservation through save/reopen and later widening. The earlier preview/Apply
regression also passes. This fixes data semantics within the supported editing
scope; the font, provider/browser and native-reader limits above remain.


### Current-main integration and review 5737544499

This update incorporates upstream main `e2615187279bd5a916c4fe56abc0a135156e005d`.
The automatic overlaps in resource reading, text interpretation, image handling,
automation telemetry, formatting controls and parity were reviewed. Named direction
properties now use main's in-place dictionary helpers rather than copying the full
property dictionary for each marker. Existing numeric/mark/source-font fixes remain.

The source-break preservation policy is excluded from effective-formatting checks.
A policy-only edit leaves the original stream intact; partial formatting retains
untouched rows' original TJ operators. The core regression tests cover both. This
does not change the engine's pre-existing undo bookkeeping for no-op requests.
Selected-row alignment remains refused as previously documented; it is not enabled
by this correction.

While an inline paragraph owns an unfinished composition, its host raw-input hook
defers primary document clicks outside that editor before egui can move IME focus.
The matching release is consumed too, including across a composition-ending frame.
Caret clicks inside the editor and application controls outside the document stay
available. The page-input path also refuses to take/publish the live editor while
composition is active.

The focused shell regression uses two visible synthetic paragraphs and an inactive
switch positive control. It checks that an outside paragraph click does not publish
the candidate, cancellation retains the accepted draft, and ordinary switching works
after cancellation. It runs the real host hook because kittest otherwise skips it.
ASCII preedit isolates ownership from font support. This is component evidence,
not new Windows/Chrome/provider or independent native-reader qualification.

### Reading surfaces and Find consolidation (#3347 / #3342)

The contributor's older [#5605](https://github.com/storytold/pdfcraft/pull/5605)
source-span encoding increment is superseded by the later #5623 contribution
already included here: complete-source font priority, whole-context span completion
and their later regressions. It does not need a second implementation import.

The #3347 contribution from `@ihsanmokhlisse` is integrated from pinned source head
`e3942c8f3f178ce77d61139925ebcf63d7440251`, preserving its author and history.
It adds opt-in right-to-left two-page reading order, Arabic/mixed-script comment
hover layout and logical-order RTL bookmark titles. Page indices and the PDF's
stored page order stay unchanged. Arrow keys follow the chosen spread order;
Page Up/Down retain document order. Agents can use `ui.command view.layout.rtl`
or `ui.set` with `rtl-pages=on|off`; `ui.state.active.rtl_pages` reports the state.
The extra catalog labels retain their original attribution and native-review limits.

The #3342 contribution from `@ihsanmokhlisse`, source head
`6f9c540a63a9c0249825c38f784e441dfa7af495`, is retained
as contribution history, with its seven public Find regressions. All seven pass
against this branch's native egui 0.37 editor, including paint, undo, deletion and
Search-panel typing. Its older custom visual-order editor is deliberately not
adopted: native egui already provides the tested behavior, and importing a second
editor would add incompatible visual/caret and snapshot-memory ownership. The
source PR's failing custom-editor paint test is not represented as passing here.
The source PRs remain open pending maintainer acceptance of this consolidation.

Review 5738954531 also corrects a pure-Latin regression: explicit LTR paragraphs
can use range formatting, including after save/reopen. RTL-base or RTL-content
mixed-style rows retain the existing uniform-full-face restriction. A focused
public-tool regression reproduced the rejection before the fix and passes with it.
The `d7dd1de647ee` reference above is explicitly an earlier integration snapshot;
that checkpoint used main `e2615187279bd5a916c4fe56abc0a135156e005d`.


### Review 5739601464 follow-up

The next update integrates main `168a3cb91e90333a7069f2bac63835140c6d0f5b`.
Directional rows choose one usable complete source encoding before characters are
resolved. A sparse PDF encoding can therefore accept a new Latin character when
its complete embedded font covers the row. Already encodable rows keep their
resource; explicit resource requests and unsupported mixed-style RTL restrictions
remain authoritative.

Bookmark titles remain in logical order and use egui's automatic direction per
paragraph. An Arabic first paragraph no longer forces a following Latin-leading
paragraph to RTL. A production-layout regression checks actual glyph positions.

Both reported failures were witnessed before correction. The focused public-tool
font case passes preview/Apply, source-font controls and save/reopen; three bookmark
layout cases pass. The remaining validation and exact-head publication record are
reported in the PR description. This does not qualify broader fonts, forms/comments
shaping, native-reader typography, browser input or provider-specific IME behavior.
