#!/usr/bin/env python3
"""Self-tests for check-vendor-ledger.py's citation classifier.

These are the negative cases that keep the resolver honest: a citation whose
FINAL segment is missing must be "missing" even when every parent module on
the path exists (`mod tests` is in almost every crate). Found in review of
the checker: `raster::tests::deleted_fn` resolved "ok" through a parent-module
fallback, so a renamed or deleted regression stayed green.

Run:  python3 scripts/test-check-vendor-ledger.py   # exit 1 on failure
"""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

# The checker's filename is hyphenated, so load it by path.
_spec = importlib.util.spec_from_file_location(
    "check_vendor_ledger", Path(__file__).resolve().parent / "check-vendor-ledger.py"
)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)
classify = _mod.classify

# A fake index: two fns (one under tests), the ubiquitous `tests` module, one
# real submodule, one file — and deliberately NO `deleted_regression`.
IDX = (
    frozenset({"replacement", "offsets_survive_a_roundtrip"}),
    frozenset({"tests", "raster"}),
    frozenset({"lib.rs", "raster.rs"}),
)

CASES = [
    # (expected, citation, why)
    ("ok", "tests::replacement", "the cited test fn exists"),
    ("ok", "raster::tests::replacement", "final fn exists under its module path"),
    ("ok", "raster::tests", "a module the citation itself names"),
    ("ok", "raster.rs", "a file citation"),
    ("ok", "offsets_survive_a_roundtrip", "a bare fn name"),
    ("skip", "cargo test -p foo", "a command, not a citation"),
    ("skip", "https://example.com/a", "a URL"),
    ("skip", "some prose word", "a bare word that is not a symbol"),
    # ── the negative cases the fallback used to accept ──
    ("missing", "tests::deleted_regression", "parent module exists, final fn does not"),
    ("missing", "raster::tests::deleted_fn", "deep path, final fn deleted after a rename"),
    ("missing", "absent_module::deleted_regression", "nothing on the path exists"),
    ("missing", "nope.rs", "a file that does not exist"),
    ("missing", "raster::tests::replacement::typos", "a typo past a real prefix"),
    ("skip", "WgpuError::CreateRenderer", "a CamelCase type path quoted in prose"),
]


def main() -> int:
    failures = []
    for expected, cite, why in CASES:
        got = classify(cite, IDX)
        mark = "ok" if got == expected else "FAIL"
        print(f"[{mark}] {cite!r}: expected {expected!r}, got {got!r} ({why})")
        if got != expected:
            failures.append((cite, expected, got))
    print(f"{len(CASES) - len(failures)}/{len(CASES)} cases pass")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
