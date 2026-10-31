#!/usr/bin/env python3
"""check-vendor-ledger.py — verify vendor/README.md against the vendored sources.

The vendor ledger records, per patched crate: version, numbered patches, and the
regression test that proves each patch. The vendored crates are excluded from
the workspace, so nothing else keeps the ledger honest: a test can be renamed,
a patch note rewritten, or a version bumped with the ledger left stale, and CI
would stay green.

Checks (fatal unless noted):

  1. version    — every crate row's Version matches vendor/<crate>/Cargo.toml
  2. cited test — every backticked test citation resolves to a real test fn,
                  test file, or module SOMEWHERE in vendor/* or apps/pdfcraft —
                  a row may cite a proof test that lives in a sibling vendored
                  crate (a hayro-syntax patch is proven by hayro's raster
                  tests) or in the app's own test suite
  3. coverage   — every crate in [patch.crates-io] appears in the ledger
  4. markers    — (warning) the ledger's highest patch number is backed by at
                  least as many `pdfcraft patch` markers in the crate's source

Usage:
  scripts/check-vendor-ledger.py            # exit 1 on fatal findings
  scripts/check-vendor-ledger.py --list     # also print every citation checked
"""
from __future__ import annotations

import re
import sys
from functools import lru_cache
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEDGER = ROOT / "vendor" / "README.md"
PATCHED = re.compile(r'^\s*([A-Za-z0-9_-]+)\s*=\s*\{\s*path\s*=\s*"vendor/([A-Za-z0-9_-]+)"', re.M)
ROW = re.compile(r'^\|\s*([a-z0-9_-]+)\s*\|\s*([^|]*?)\s*\|\s*([^|]*?)\s*\|\s*(.*?)\s*\|\s*(.*?)\s*\|\s*$', re.M)
CITED = re.compile(r'`([^`]+)`')
MARKER = "pdfcraft patch"
SKIP_TOKEN = re.compile(r'\s|cargo |rustc |--|^https?://')

@lru_cache(maxsize=None)
def crate_text(crate: str) -> str:
    """All Rust sources of one vendored crate (or the app), concatenated once."""
    base = ROOT / "vendor" / crate if crate != "pdfcraft" else ROOT / "apps" / "pdfcraft"
    parts = []
    for path in base.rglob("*.rs"):
        try:
            parts.append(path.read_text(errors="replace"))
        except OSError:
            pass
    return "\n".join(parts)

@lru_cache(maxsize=None)
def all_texts() -> tuple[tuple[str, str], ...]:
    """(label, concatenated sources) for every vendored crate plus the app."""
    crates = sorted(p.name for p in (ROOT / "vendor").iterdir() if p.is_dir())
    return tuple((c, crate_text(c)) for c in crates) + (("pdfcraft", crate_text("pdfcraft")),)

@lru_cache(maxsize=1)
def index() -> tuple[frozenset[str], frozenset[str], frozenset[str]]:
    """(test fn names, module names, file basenames) across vendor/*, apps/ and crates/."""
    texts, names = [], []
    bases = [ROOT / "vendor" / c for c in sorted(p.name for p in (ROOT / "vendor").iterdir() if p.is_dir())]
    bases += [ROOT / "apps" / "pdfcraft", ROOT / "crates"]
    for base in bases:
        if not base.is_dir():
            continue
        for path in base.rglob("*.rs"):
            try:
                texts.append(path.read_text(errors="replace"))
                names.append(path.name)
            except OSError:
                pass
    blob = "\n".join(texts)
    fns = frozenset(re.findall(r'\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[(<]', blob))
    mods = frozenset(re.findall(r'\bmod\s+([A-Za-z_][A-Za-z0-9_]*)\b', blob))
    return fns, mods, frozenset(names)

def symbol_exists(symbol: str, idx) -> bool:
    return symbol in idx[0]

def module_exists(name: str, idx) -> bool:
    return name in idx[1]

def file_exists(name: str, idx) -> bool:
    return name.rsplit("/", 1)[-1] in idx[2]

def classify(cite: str, idx) -> str:
    """ok | missing | skip — one backticked token from the ledger.

    Resolution is by the citation's FINAL segment only: a test fn or a module
    the citation itself names. The parent path is context, not proof —
    `mod tests` exists in almost every crate, so accepting a parent module
    let `raster::tests::deleted_fn` pass after the test was deleted (found
    in review; see the negative cases in test-check-vendor-ledger.py).
    """
    cite = cite.strip()
    if not cite or SKIP_TOKEN.search(cite):
        return "skip"          # prose, command, URL
    base = cite.split("::", 1)[0]
    if base.endswith(".rs"):
        return "ok" if file_exists(base, idx) else "missing"
    if "::" in cite:
        last = cite.rsplit("::", 1)[-1]
        if last[:1].isupper():
            return "skip"      # a type path from prose (e.g. WgpuError::CreateRenderer)
        if symbol_exists(last, idx) or module_exists(last, idx):
            return "ok"
        return "missing"
    if symbol_exists(cite, idx) or file_exists(cite, idx):
        return "ok"
    return "skip"              # a bare word that is not a test — prose

def main() -> int:
    show_all = "--list" in sys.argv[1:]
    ledger = LEDGER.read_text(errors="replace")
    root_cargo = (ROOT / "Cargo.toml").read_text(errors="replace")
    patched = dict(PATCHED.findall(root_cargo))

    fatal: list[str] = []
    warnings: list[str] = []
    checked = 0
    idx = index()

    for m in ROW.finditer(ledger):
        crate, version, _lic, patches, tests = m.groups()
        version = version.strip().strip("`").strip()
        if crate not in patched:
            continue
        manifest = ROOT / "vendor" / patched[crate] / "Cargo.toml"
        if manifest.is_file():
            mm = re.search(r'^\s*version\s*=\s*"([^"]+)"', manifest.read_text(errors="replace"), re.M)
            got = mm.group(1) if mm else "?"
            if got != version:
                fatal.append(f"version drift: ledger says {crate} {version}, manifest says {got}")
        for cite in CITED.findall(tests):
            state = classify(cite, idx)
            checked += 1
            if show_all:
                print(f"[{state}] {crate}: {cite}")
            if state == "missing":
                fatal.append(f"cited test not found: {crate}: `{cite}`")
        nums = [int(n) for n in re.findall(r'\((\d+)\)', patches)]
        # patch numbering is a sequence (1)(2)(3)…; prose also parenthesises
        # numbers (e.g. a 4 GiB allocation), so take the longest run 1..k that
        # is fully present, and only among plausible indices. A flat cap goes
        # wrong once a crate earns more than 40 patches: items past the cap
        # were dropped and `top` compared against the wrong count.
        present = {n for n in nums if 0 < n <= 500}
        top = 0
        while top + 1 in present:
            top += 1
            text = crate_text(patched[crate]).lower()
            markers = text.count(MARKER)
            if markers < top:
                warnings.append(f"ledger claims {top} patches for {crate} but only {markers} 'pdfcraft patch' markers in source")

    # prose around the table also cites tests; scan every non-table line so
    # notes above the first heading are checked too (table cells were already
    # checked per row above)
    prose = "\n".join(l for l in ledger.splitlines() if not l.lstrip().startswith("|"))
    for cite in CITED.findall(prose):
        state = classify(cite, idx)
        if state == "missing":
            fatal.append(f"cited test not found (prose): `{cite}`")

    for crate in patched:
        if not re.search(rf'^\|\s*{re.escape(crate)}\s*\|', ledger, re.M):
            fatal.append(f"crate {crate} is in [patch.crates-io] but absent from the vendor ledger")

    for w in warnings:
        print(f"warning: {w}")
    for f in fatal:
        print(f"error: {f}")
    print(f"vendor ledger: {checked} citations checked, {len(fatal)} fatal, {len(warnings)} warning(s)")
    return 1 if fatal else 0

if __name__ == "__main__":
    sys.exit(main())