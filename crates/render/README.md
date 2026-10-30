# pdfcraft-render

Page rendering, inspection, and the shared text layer used by canvas selection, copy,
search, highlighting, and search redaction.

## ActualText selection

An `ActualText` span keeps its logical replacement text through page layout. When its
painted ToUnicode clusters exactly match that text under a verified Unicode bidi
permutation, the final text layer retains each cluster's actual position. Arabic words,
mixed Latin text, and digits can then be selected and searched independently. Repeated
letters and words use their verified visual occurrences rather than an arbitrary match
of equal strings. Source-font zero-advance dots and marks keep their real ink bounds and
belong to a cluster only when its advancing components prove the ownership.

Ligatures remain atomic: the PDF supplies one physical box for a glyph representing
several characters. Invisible formatting characters and hanging spaces retain their
logical text with zero advance. A logically consecutive selection can have disjoint
physical rectangles; these pieces remain separate around unselected text for search
highlighting and redaction.

Unrelated replacement labels, ambiguous auxiliary glyphs, incompatible baselines or
directions, and mappings beyond the bounded work limits retain the original whole-span
replacement. A wrapped row whose opening bidi override or isolate was on a previous row
can also remain atomic when that missing paragraph context prevents an exact proof.
The renderer does not invent character positions or change PDF painting order to make
such a replacement appear precise.
