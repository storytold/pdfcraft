# External font embedding exception: decision required

This draft isolates the proposed change removed from Add Text PR #3528 at the
maintainer's request. It is not an approved change to the project's licence policy.

The proposal permits an external document font whose name table declares the GNU
GPL font-embedding exception. TH Sarabun New is the motivating external input.
It does not bundle or commit a font, permit GPL code, change the asset allowlist,
or export a system-interface fallback. Existing format, size, embedding-rights,
excluded-family and authorship checks still apply.

The proposed check only recognizes four phrases in untrusted font metadata.
Any font can claim them. As with other name-table declarations, these phrases
cannot establish licence provenance. This draft must not be merged on the strength
of that heuristic. Maintainers need to decide whether the policy may admit such
external fonts and what independently verifiable provenance is required before
implementing an admission mechanism. The declaration-only implementation is
preserved here for review rather than hidden in the Unicode feature.

PR #3528 retains the existing import policy and tests shaping/embedding with
external font/PDF inputs. Its implementation does not depend on this draft.
