# OCR benchmark

> **Last updated:** 2026-10-29 · **Change:** patch (the accented floors of 0 guard nothing yet,
> and when they get real values)

How well Scan & OCR ▸ Recognize text reads, measured two ways: synthetic scans of committed
ground-truth texts (reproducible to the pixel, guarded by a test), and real pages from MDPBench
(scored with MDPBench's own evaluation, so the numbers sit next to published ones).

Everything below runs locally. The models come from `cargo xtask models` (or `PDFCRAFT_MODELS`
pointing at a folder that holds them); without them the floor test skips and the examples stop
with a message.

## Ground truth

`crates/engine/tests/data/ocr/` holds the texts, one file per language and source, as
`doc<TAB>source<TAB>text` rows (`#` starts a comment; consecutive rows of one `doc` form one
page):

- `de.tsv`: an invoice, a letter and a delivery note written for PdfCraft (invented names and
  numbers), with amounts, dates, `§`, `°C` and dashes.
- `<lang>-wikipedia.tsv` for cs, da, de, es, fi, fr, is, it, nl, no, pl, pt and sv: three
  paragraphs each from that language's Wikipedia, CC BY-SA 4.0
  ([LICENSES/CC-BY-SA-4.0.txt](../LICENSES/CC-BY-SA-4.0.txt)), the article revision recorded per
  row and in `ATTRIBUTION.toml`.

`cargo xtask ocr-texts <lang> [--docs N]` fetches new paragraphs from random articles. It keeps
the ones whose letters are most often outside ASCII, so a fixture exercises the letters the
language adds to the Latin alphabet, and leaves out paragraphs with characters outside Latin
Extended-B (IPA, other scripts), which would test the font rather than the recogniser. It prints
the `ATTRIBUTION.toml` entry the file needs. The tests never touch the network.

### Scans

`crates/engine/tests/it/support/ocr_scan.rs` sets each document on an A4 page, 11 pt with
baselines 1.9 × the point size apart, in one of the committed OFL faces (Inter, JetBrains Mono;
`--face FILE` adds another), and scans it under four conditions:

| Condition | Resolution | Degradation |
|---|---|---|
| `300clean` | 300 dpi | none (PNG) |
| `300noisy-skew1` | 300 dpi | sensor noise σ 12, blur 0.8 px, 1° skew, JPEG 85 |
| `200noisy` | 200 dpi | noise σ 12, blur, JPEG 80 |
| `150jpeg65` | 150 dpi | noise σ 10, blur, JPEG 65 |

The noise is seeded per (file, document, face, condition), so every run sees the same pixels.
Each scan becomes a one-page PDF at its resolution and goes through `Session::recognize_text`,
the same path as the menu command.

### Scoring

Words are compared as a multiset: reading order does not count, and surrounding quotes, brackets
and sentence punctuation are trimmed (a hyphen or symbol inside a word stays). Three overlapping
classes are reported: all words, words with a letter outside ASCII ("accented"), and tokens with
a digit or symbol (amounts, dates, `§`, `°C`). A word counts only if it comes back exactly.

### The floor test

`ocr_ground_truth::scanned_ground_truth_reads_back_above_its_floor` reads every fixture in Inter
at `200noisy` and fails if a fixture falls below its recorded floor (the measured share less two
points). Each fixture is read with the recogniser of its own language when PdfCraft has one
(`ocr::LANGUAGES`), otherwise with `en`; floors are kept per fixture and recogniser. On `main`
only `en` exists, so every floor is an `en` floor, and the accented floors are 0: ocrs's alphabet
has no accented letters. When a recogniser for another language lands (#1012, #2520),
`every_fixture_has_a_floor_and_inter_draws_it` fails until that language's fixtures have
measured floors for it. So on `main` the accented floors do not guard accent recognition at all;
they get real, non-zero values, measured for that recogniser, when a Latin recogniser that knows
accented letters lands (#2520, #1012).

The test catches regressions in rendering, recognition or the text layer: a model alphabet
shifted by one letter dropped every fixture below its floor (for example `nl-wikipedia.tsv` from
90 % to 36 % of words, measured while the scans still set the text at about 14.7 pt).

### Running it

```sh
cargo run --release -p pdfcraft-engine --example ocr_bench -- --all --misses
cargo run --release -p pdfcraft-engine --example ocr_bench -- de --cond 200noisy --faces Inter
```

Positional arguments pick fixtures by language, `--ocr en,…` the recognisers to compare (default:
`en` and the fixture's own language; one PdfCraft has no recogniser for is skipped with a
message), `--cond` and `--faces` narrow the matrix, `--save DIR` keeps the scans, `--misses`
lists the words that did not come back and what was read instead.

### Numbers on `main`

Measured 2026-10-28 on ai395 with the `en` recogniser (ocrs), both faces, all four conditions, 24
pages per row:

| Fixture | Words | Accented words | Numbers & symbols |
|---|---|---|---|
| de (invoice, letter, delivery note) | 61.7 % | 0 % | 66.3 % |
| de (Wikipedia) | 69.9 % | 0 % | 71.4 % |
| fr | 68.3 % | 0 % | 100 % |
| es | 72.8 % | 0 % | 95.8 % |
| pt | 69.1 % | 0 % | 87.5 % |
| it | 75.4 % | 0 % | 85.7 % |
| nl | 80.9 % | 0 % | 97.7 % |
| fi | 61.0 % | 0 % | 75.8 % |
| cs | 29.9 % | 0 % | 76.1 % |
| sv | 50.2 % | 0 % | 78.6 % |
| da | 69.7 % | 0 % | 80.2 % |
| is | 36.1 % | 0 % | 62.5 % |
| no | 71.8 % | 0 % | 80.8 % |
| pl | 43.7 % | 0 % | 39.6 % |

The words `en` reads are the unaccented ones; every word with an accent is lost, so the word
share follows how many words of a language carry one.

## Real pages: MDPBench

The fixtures are synthetic. [MDPBench](https://huggingface.co/datasets/Delores-Lin/MDPBench)'s
public split (Apache-2.0) has real pages: 160 per language (academic papers, newspapers, books,
magazines, financial reports, letters), each a digital original with three phone photos of it
(folds, shadows, glare, blur). Its pages are other people's documents: they are never committed,
and no test reads them.

```sh
cargo xtask mdpbench --langs de,es,fr                    # once, into target/mdpbench/
cargo run --release -p pdfcraft-engine --example ocr_mdpbench -- target/mdpbench --langs de,es,fr --out target/mdpbench/predictions
```

`cargo xtask mdpbench` fetches `MDPBench_public.json` and the chosen languages' images from one
pinned dataset revision (`252df132fe1b418dbd5830515ef4b5a06fa7164f`), checks the annotations
against their SHA-256, downloads through `.part` names so an interrupted file is never taken for
a whole one, and keeps files already there. A page that cannot be fetched is reported and the
others still come (the command then exits with an error listing them; run it again to fill the
gaps). Without `--langs` it fetches the Latin-script languages (de, en, es, fr, it, nl, pt).

`examples/ocr_mdpbench` reads every page with each recogniser and prints a quick check of its
own: the three word classes of the ground truth against the annotated text regions, and table
cells (from the annotated HTML) in a column of their own. Results are split into `digital`,
`photo` (taken from above) and `photo-turned` (taken from the left, the right or below: the text
lies sideways or upside down, which recognition cannot read without turning the page first). A
page the recogniser skipped is reported as an error, not scored as empty. Annotated image names
with a directory part are skipped, so the run reads only from `images/` and writes only under
`--out`. The resolution of the images is not recorded, so each page is read as if it were A4
wide. `--kind`, `--limit N` and `--misses` narrow and explain a run.

### The official evaluation

The quick check is not comparable with published results. `--out DIR` writes each page's text to
`DIR/<recogniser>/<image stem>.md`, which is what MDPBench's own evaluation reads. Each line of
the recognised text becomes a paragraph; `main` returns a page's words as one line, so for now a
page is a single paragraph (#5157 keeps the line breaks). The evaluation skips a page that has no prediction, so a page that could not
be read gets an empty one and counts as nothing found, and a prediction that cannot be written
stops the run with an error rather than leaving the page out.

We leave out the formula metric, CDM: it needs Node.js, XeLaTeX and ImageMagick, and PdfCraft
predicts no formulas. The upstream configuration runs it, so take it out before running. With
[Yuliang-Liu/MultimodalOCR](https://github.com/Yuliang-Liu/MultimodalOCR) checked out, in
`MDPBench/`:

1. Copy `configs/end2end.yaml` to `configs/pdfcraft-en.yaml`.
2. In the copy, set `dataset.ground_truth.data_path` to `MDPBench_public.json` (absolute path)
   and `dataset.prediction.data_path` to `DIR/en`, and delete the `- CDM` line under
   `display_formula.metric` (its `Edit_dist` stays).
3. Run `python pdf_validation.py --config configs/pdfcraft-en.yaml`. The results go to
   `result/en_result/` (the prediction folder's name plus `_result`).
4. Run `python tools/calculate_scores.py --result_folder result/en_result`.

It matches predicted and annotated elements (OmniDocBench's procedure), scores text by edit
distance and tables by TEDS, and averages per page.

### Numbers on `main`

Measured 2026-10-28 with the `en` recogniser (ocrs), official evaluation without CDM, on the
German, Spanish and French pages (digital and all photos):

| | DE | ES | FR | Digital | Photo |
|---|---|---|---|---|---|
| PdfCraft `main` (`en`) | 32.6 | 19.8 | 28.6 | 46.9 | 21.2 |
| PP-StructureV3 (paper) | 60.4 | 54.4 | 49.8 | 56.2 | 41.7 |

For scale, from the MDPBench paper: PP-StructureV3 scores 45.4 over all languages, the MinerU-2.5
pipeline 33.5 overall, and Gemini-3-pro 86.4.

Read the comparison with care:

- We score only German, Spanish and French; the paper's overall numbers cover every language,
  including scripts PdfCraft cannot read at all yet. Our "digital" and "photo" columns are over
  DE/ES/FR alone; the paper's are over all languages.
- PdfCraft writes recognised lines, not document structure: no table markup and no formulas, so
  tables score as plain text and any formula on a page is missed.
- Skipping CDM drops a component we would score badly on, which flatters our numbers slightly.
- Photos taken sideways or upside down are in the photo split; `main` does not turn pages before
  reading them, so they come back nearly empty.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-29 | patch | Says that the accented floors of 0 cannot guard accent recognition with `en`, and that they get non-zero values when a Latin recogniser with accented letters lands (#2520, #1012). |
| 2026-10-28 | patch | The scans set their text at 11 pt as documented (they were 4/3 larger, about 14.7 pt); the `en` floors and the synthetic numbers are re-measured. A failed MDPBench page gets an empty prediction, a prediction that cannot be written stops the run, and annotated image names with a directory part are skipped. The official-evaluation steps say how to leave CDM out. |
| 2026-10-28 | new | Ground-truth fixtures, floors for the `en` recogniser, MDPBench with its official evaluation; numbers for `main`. |
