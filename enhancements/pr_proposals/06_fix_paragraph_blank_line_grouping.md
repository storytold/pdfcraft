# Pull Request: fix(edit): separate paragraphs across blank lines and headings (#3577)

## Summary
In `crates/edit/src/text_regions.rs`, lines were grouped into the same paragraph block whenever their baseline gap was up to `2.5×` the font size. In typical typography, standard line leading is `1.15×`–`1.35×`, so a blank line between two paragraphs at `1.2×` leading produces a `2.4×` gap.

Because `2.4 < 2.5`, two distinct paragraphs separated by a blank line (and headings preceding a paragraph) were merged into a single `TextBlock`. Rewriting or editing that block then rewrapped the heading or closed the blank line.

## Changes
- `crates/edit/src/text_regions.rs`: Tightened row-to-block gap threshold from `2.5` to `1.85`.
- `crates/edit/src/tests.rs`: Added regression test `blank_line_separates_paragraphs_instead_of_merging_into_one`.
