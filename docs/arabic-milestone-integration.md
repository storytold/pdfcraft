# Arabic text editing milestone: integration map

Updated 2026-11-02. These contributions form one Arabic editing
package with five reviewable logical parts. The proposed delivery is one branch
and one consolidated PR, preserving the original contributions and their evidence.
This map is a local integration proposal, not a claim that the work has been
published, accepted, merged or released.

| Part | Contribution | Role in the combined workflow |
| --- | --- | --- |
| [#5602](https://github.com/storytold/pdfcraft/pull/5602) | Arabic decimal/group separators | Keep numeric runs intact during text recovery. |
| [#5603](https://github.com/storytold/pdfcraft/pull/5603) | Combining marks in copy/search | Keep mapped nonspacing marks with their visual base without moving glyph geometry. |
| [#5623](https://github.com/storytold/pdfcraft/pull/5623) | Source-font Arabic editing | Reuse supported complete embedded fonts, contextual shaping, Unicode mappings and source positions through repeated edits/save/reopen. |
| [#5693](https://github.com/storytold/pdfcraft/pull/5693) | Auto/LTR/RTL controls | Expose paragraph base direction independently of physical alignment through UI, engine, storage and automation. |
| Input follow-up, PR pending | [Interactive editing and composition](arabic-input.md) | Protect native composition keys and unfinished drafts, release the guard after native caret interruption, and expose bounded `ui.ime` delivery. |

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
   remains to be applied and qualified: explicit direction must not bypass
   turned-text/legacy Indic exclusions, and documentation must state the existing
   justification restriction. The prepared package does not claim this review is fixed.
5. Add the interactive input follow-up, including the bounded `ui.ime` control
   API and native composition/caret interruption regressions.

Keep the existing PRs open until the maintainer accepts the consolidated PR as
their replacement. Preserve original author metadata and link every source PR;
do not import unrelated old upstream merge, CI or asset changes. A shared branch
does not require collapsing the five parts into an opaque commit.

Before publishing, refresh the package against current main, qualify the combined
source Apply/Save/reopen and copy/search workflow, complete Chrome input checks,
and record precise provider/native-reader limitations. Passing canonical-path
local builds do not by themselves qualify this uncommitted package snapshot or
replace exact-head CI and maintainer review. Historical source-PR test results
remain historical; they must not be represented as one new combined suite run.

At the 2026-11-02 GitHub snapshot, all four published PRs were open. #5602, #5603
and #5623 had merge conflicts; #5693 was mergeable but had the review follow-up
above. The new input contribution had not been pushed. Recheck those states and
the exact heads before merging. This note has not itself notified a maintainer.

The supported-font scope remains important. Arbitrary mixed-style source text,
full Arabic form appearances, missing-font completion, OCR/export interoperability
and universal OS IME qualification are not delivered by this bundle. Overall RTL
interface parity remains planned until its broader workflows have evidence.
