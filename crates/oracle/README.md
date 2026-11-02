# pdfcraft-oracle

The external programs PdfCraft checks its own output against, and one rule: **a missing oracle is
either a declared skip or a failure, never a silent pass.**

- **Layer:** test support (`Class::Testkit` in `xtask/src/layers.rs`). A dev-dependency only, never
  compiled into the product, and so usable even from the standalone crates (`cos`, `geom`,
  `filters`, `crypt`, `arlington`) whose own tests need it.
- No panicking path of its own. The `or_skip!` macro expands its `assert!` into the caller, which
  is test code.

## Why it exists

Every test that wanted an oracle used to probe for it by hand and decide on its own what to do when
it was missing. The common choice — print a line and return — reports as a **pass**, so a suite
with no oracles installed looked exactly like one in which every oracle agreed.

That was not hypothetical. No CI runner installed `qpdf`, so `cos/tests/encryption.rs::qpdf_agrees`
had never executed on a runner, while `README.md` said saved output was "independently checked with
qpdf". For R2–R4 that test is the *only* check of owner-password authentication (Algorithm 7) that
does not go through our own code.

## API

```rust
// In a test: bind the oracle, or return after saying why it was skipped.
let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
let out = qpdf.command().arg("--check").arg(&path).output()?;

// Elsewhere (xtask), handle the absence yourself.
match pdfcraft_oracle::pdftotext() {
    Ok(tool) => { /* tool.command() */ }
    Err(absent) => bail!("{absent}"),
}
```

| Probe | Tool | Used for |
|---|---|---|
| `qpdf()` | qpdf | `--check` structural validation; the independent owner-password oracle |
| `pdftoppm()` | Poppler | page pixels, for comparing against our own raster output |
| `pdftotext()` | Poppler | text extraction (`cargo xtask text-oracle` scores word F1 and reading order against it) |

Each returns `Result<Oracle, Absent>`. `Oracle::command()` hands back a fresh `Command`;
`Absent` renders as an actionable sentence naming the install command.

## The gate

`PDFCRAFT_ORACLES_REQUIRED` (any value, including `0` — the same convention as
`CRAFT_FONTS_REQUIRED`) turns absence from a skip into a failure. CI sets it on the runners that
install the tools, so a skip notice there becomes a red build.

```
$ cargo test -p pdfcraft-cos --test encryption qpdf_agrees
test result: ok. 1 passed            # qpdf absent: skipped, with a notice on stderr

$ PDFCRAFT_ORACLES_REQUIRED=1 cargo test -p pdfcraft-cos --test encryption qpdf_agrees
oracle `qpdf` is required but is not on PATH (program not found): PDFCRAFT_ORACLES_REQUIRED is
set, so this is a failure rather than a skip (apt install qpdf / brew install qpdf)
test result: FAILED. 1 failed
```

`or_skip!` expands to a `return`, so it belongs in a test returning `()`; a test returning `Result`
should match on the probe directly.

## Comparing pixels (`pixels`)

Rendering needs more than "did the tool run". A raw pixel-for-pixel difference is useless here:
two correct renderers disagree on every glyph edge, so a strict comparison calls a perfect page
badly wrong and tells you nothing about whether a shading is missing.

`pixels::compare` counts a pixel as differing only when it differs from **every** pixel in the
other image's 3x3 neighbourhood by more than a tolerance. A one-pixel shift or a differently
weighted edge finds a near match next door and is forgiven; a glyph that is not there, or a region
painted the wrong colour, has no near match and is counted. The test runs in both directions and
reports the worse result, so neither argument order flatters the blurrier renderer.

`decode_pgm` reads what `pdftoppm -gray` writes; `decode_pam_gray` reads what
`pdfcraft-cli render --out x.pam` writes, compositing premultiplied alpha over white first so a
transparent margin is not scored as black.

`cargo xtask render-oracle` drives it over the corpus. It reports; it does not gate.

A high score is a *disagreement*, not a verdict on us. The worst file of the first 250,
`bug1552113.pdf` at 84%, is Poppler painting a link's `/Border [0 0 112]` as a band over the text
the fixture exists to show; ours is the readable rendering. Look at both images before believing
the number. `--hide-annotations` separates annotation differences from page content, though the
default shows them: hiding is slightly worse overall, because it then scores Poppler for every
appearance we do draw.

**Poppler is not a usable oracle for JBIG2.** The 96 `bitmap-*.pdf` corpus fixtures encode one
picture through every corner of the format, so a correct decoder renders them all identically. We
render 1 distinct image across all 96; Poppler renders 8, drawing a blank page for seven of them.
`crates/render/tests/jbig2_invariants.rs` pins that invariant, and needs no oracle to do it.

## Comparing text

`cargo xtask text-oracle` scores two different things against `pdftotext`, because one number
hides the other.

**Word F1** is a bag of words: how much of what each side extracted the other also extracted,
with multiplicity. It catches a character read wrong, a glyph not read at all, a font we cannot
map. It is blind to *order* — a page emitted back to front scores 1.000.

**Reading order** is the longest common subsequence of the two word sequences, as a share of the
longer one. It catches the failures F1 cannot see: a column read into the middle of its
neighbour, a figure's labels threaded through a paragraph, a form's value parted from its label.
Both sides are cut to the first 3,000 words, because the comparison is quadratic and three pages
is plenty of signal; the run says how many files that bit.

The second score exists because of a measurement. Reviewing #2431, a change that moves text and
alters nothing else, word F1 called 26 of the 29 files whose output changed *identical*, while
the order score separated 18 improvements from 6 regressions — the regressions on forms, where
a checkbox's number was parted from its label.

Read it the same way as the pixel score, and then once more: Poppler's reading order is a second
opinion, not the truth, and the same split can score as a win on one file and a loss on another
purely because `pdftotext` happens to make it in one place and not the other.

## Still to do

- Acrobat itself as the reference, rather than other open renderers.
- Oracles that are not external processes (hayro, `lopdf`) are ordinary dev-dependencies and are
  not routed through this crate.
- Two renderers sometimes round the same page box to different pixel sizes, and those files are
  counted as unmeasurable rather than compared.
