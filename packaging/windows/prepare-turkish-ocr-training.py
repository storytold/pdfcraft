#!/usr/bin/env python3
"""Offline contributor-original Turkish LSTM line fixtures; never runs OCR or training.

Only Pillow from an existing external oracle environment is needed. Images, labels and
WordStr boxes stay under a caller-selected private evidence root outside the repository.
Original text/pixels: MIT OR Apache-2.0. Inter is read in place under OFL-1.1, never copied.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import io
import json
import math
from pathlib import Path
import platform
import random
import struct
import sys
import tempfile
import tomllib
import unicodedata
import unittest

from PIL import Image, ImageDraw, ImageFilter, ImageFont, ImageOps, __version__ as pillow_version

FONT_SHA = "40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82"
MODEL_SHA = "cf7216998bf7f363f18e3def04cc6b28ae954d2a5a3bdad09f743c9d3538ef35"
CHARSET_SHA = "0ce04ab5919d3ae2a48769b4f8a17b25062a619495ccf00f3aa0f10384077994"
SEED = 13502132
MAX_FILE = 4 * 1024 * 1024
MAX_TOTAL = 256 * 1024 * 1024
MAX_WIDTH, MAX_HEIGHT, MAX_PIXELS = 4096, 256, 1_048_576
SIZES = (28, 34, 40, 46)
RECIPES = ("clean", "blur", "skew", "noise")
TARGETS = "îÎûÛ"
PRIMARY_DOCS = [
    "https://tesseract-ocr.github.io/tessdoc/tess4/Make-Box-Files.html",
    "https://github.com/tesseract-ocr/tesstrain/blob/main/generate_wordstr_box.py",
    "https://tesseract-ocr.github.io/tessdoc/tess5/TrainingTesseract-5.html",
]
# New short compositions for this tool, not imported from an evaluation corpus.
BANKS = {
    "training": {
        "lead": ["Arşivde", "Atölyede", "Salonda", "Depoda", "Masada", "Durakta", "Raflarda", "Koridorda"],
        "tail": ["dosya sıraya alındı", "kayıt yeniden sayıldı", "not dikkatle okundu", "etiket sağa yazıldı", "liste sola kondu", "rapor ayrı tutuldu", "satır açık bırakıldı", "başlık tekrar görüldü"],
        "i_hat": ["resmî", "şahsî", "edebî", "askerî", "aklî", "naklî", "fiilî", "harfî"],
        "u_hat": ["mahkûm", "meskûn", "sükût", "hükûmet", "mûsiki", "mûris", "mâkûl", "nüfûz"],
        "pairs": ["IRMAK ıslak İPEK ince", "ISLAK ılık İZİN iki", "IŞIK ırgat İSKELE ileri", "ILGAZ ıssız İPLİK iri"],
        "mix": ["Yeni file hazır; open sonra", "Kısa test bitti; save şimdi", "Raf için blue label seç", "Düz text var; print bekler"],
        "plain": ["Çekmece küçük, gölge yumuşak", "Üç öğrenci köşeyi ölçer", "Güçlü rüzgâr şeridi çözer", "Açık zarf çatıya götürüldü"],
    },
    "internal-val": {
        "lead": ["Bahçede", "Müzede", "Peronda", "Avluda", "Pencerede", "Kulübede", "Kıyıda", "Sergide"],
        "tail": ["belge özenle saklandı", "çizelge baştan izlendi", "fiş usulca çevrildi", "zarf kutuya bırakıldı", "özet önceden istendi", "taslak kapıda incelendi", "sayfa renkli seçildi", "çizgi kenarda bitirildi"],
        "i_hat": ["ilmî", "medenî", "siyasî", "fikrî"],
        "u_hat": ["usûl", "mûteber", "sûret", "malûm"],
        "pairs": ["ISSIZ ırmak İNCİ ilmek", "ITIR ısırık İSİM izlek", "ILIK ışın İĞDE işlek", "IZGARA ıhlamur İMZA izci"],
        "mix": ["Yeşil folder kapalı; next yok", "Yeni batch tamam; stop burada", "Özet için short note bırak", "Son line seç; review açık"],
        "plain": ["Şeffaf şişe öğleden sonra düştü", "Ördek güneşte çamuru eşeler", "Yağlı çörek küçük tabakta", "Gözlükçü dükkânın ışığını kıstı"],
    },
}


class DataError(ValueError):
    pass


def digest(path: Path, limit: int) -> str:
    if not path.is_file() or path.stat().st_size > limit:
        raise DataError(f"input must be a regular file within {limit} bytes: {path}")
    h = hashlib.sha256()
    with path.open("rb") as stream:
        total = 0
        while chunk := stream.read(1024 * 1024):
            total += len(chunk)
            if total > limit:
                raise DataError("input grew beyond its byte limit")
            h.update(chunk)
    return h.hexdigest()


def pinned(path: Path, expected: str, limit: int) -> Path:
    path = path.resolve(strict=True)
    if digest(path, limit) != expected:
        raise DataError(f"pinned SHA-256 mismatch: {path}")
    return path


def font_covers(data: bytes, required: set[str]) -> set[str]:
    """Read only Unicode cmap4/12 mappings from the already hash-verified pinned TTF.

    Checking glyph id!=0 proves no .notdef/system fallback, including the dotted I and hats.
    Every read is bounded; this is not a general-purpose font loader.
    """
    def number(offset: int, fmt: str) -> int:
        length = struct.calcsize(fmt)
        if offset < 0 or offset + length > len(data):
            raise DataError("font cmap points outside the pinned file")
        return struct.unpack_from(fmt, data, offset)[0]
    tables = number(4, ">H")
    if tables > 128:
        raise DataError("too many font tables")
    cm = None
    for i in range(tables):
        pos = 12 + i * 16
        if data[pos:pos + 4] == b"cmap":
            cm = number(pos + 8, ">I")
    if cm is None:
        raise DataError("pinned font has no cmap")
    count = number(cm + 2, ">H")
    if count > 64:
        raise DataError("too many cmap subtables")
    found = set()
    for index in range(count):
        row = cm + 4 + index * 8
        plat, enc = number(row, ">H"), number(row + 2, ">H")
        if plat != 0 and not (plat == 3 and enc in (1, 10)):
            continue
        start = cm + number(row + 4, ">I")
        fmt = number(start, ">H")
        if fmt == 4:
            segments = number(start + 6, ">H") // 2
            if segments > 4096:
                raise DataError("cmap segment cap exceeded")
            end_base = start + 14
            begin_base = end_base + 2 * segments + 2
            delta_base = begin_base + 2 * segments
            range_base = delta_base + 2 * segments
            for char in required:
                cp = ord(char)
                if cp > 0xFFFF:
                    continue
                for j in range(segments):
                    lo, hi = number(begin_base + j * 2, ">H"), number(end_base + j * 2, ">H")
                    if not lo <= cp <= hi:
                        continue
                    delta, offset = number(delta_base + j * 2, ">h"), number(range_base + j * 2, ">H")
                    gid = number(range_base + j * 2 + offset + 2 * (cp - lo), ">H") if offset else cp
                    if gid and ((gid + delta) & 0xFFFF):
                        found.add(char)
                    break
        elif fmt == 12:
            groups = number(start + 12, ">I")
            if groups > 65536:
                raise DataError("cmap group cap exceeded")
            for j in range(groups):
                pos = start + 16 + j * 12
                lo, hi, gid = number(pos, ">I"), number(pos + 4, ">I"), number(pos + 8, ">I")
                for char in required:
                    if lo <= ord(char) <= hi and gid + ord(char) - lo:
                        found.add(char)
    return required - found


def read_alphabet(path: Path) -> set[str]:
    text = path.read_text(encoding="utf-8")
    rows = text.splitlines()
    if not rows or int(rows[0]) != len(rows) - 1:
        raise DataError("unicharset entry count does not match its header")
    return {row.split(" ", 1)[0] for row in rows[1:] if row and not row.startswith(("NULL ", "Joined ", "|Broken|"))} | {" "}


def label_ok(text: str) -> None:
    if not 1 <= len(text) <= 128 or text != text.strip() or unicodedata.normalize("NFC", text) != text:
        raise DataError("labels must be one nonempty NFC line of at most128 characters")
    if any(unicodedata.category(c).startswith("C") or c.isspace() and c != " " for c in text):
        raise DataError("labels cannot contain control or multiline whitespace")


def upper_tr(word: str) -> str:
    return word.translate(str.maketrans({"i": "İ", "ı": "I"})).upper()


def labels(split: str, count: int, seed: int) -> list[dict]:
    if split not in BANKS or not 16 <= count <= (512 if split == "training" else 128) or count % 16:
        raise DataError("line counts must be multiples of16: training16..512; internal-val16..128")
    bank = BANKS[split]
    result = []
    rng = random.Random(seed + (0 if split == "training" else 104729))
    for i in range(count):
        group, n = i % 8, i // 8
        lead = bank["lead"][n % len(bank["lead"])]
        tail = bank["tail"][(n * 3 + group) % len(bank["tail"])]
        if group < 4:
            word = bank["i_hat" if group < 2 else "u_hat"][n % len(bank["i_hat" if group < 2 else "u_hat"])]
            middle = upper_tr(word) if group % 2 else word
        elif group == 4:
            middle = bank["pairs"][n % len(bank["pairs"])]
        elif group == 5:
            middle = f"tutar {17 + n * 3},25; oran %8 ({40 + n}) / 2 = {20 + n}"
        else:
            key = "mix" if group == 6 else "plain"
            middle = bank[key][n % len(bank[key])]
        text = unicodedata.normalize("NFC", f"{lead} {middle}; {tail}. No:{1000 + i:04d}")
        label_ok(text)
        result.append({"id": ("train" if split == "training" else "ival") + f"-{i:04d}", "text": text,
                       "group": ("lower_i_hat", "upper_i_hat", "lower_u_hat", "upper_u_hat", "turkish_i", "numbers_punctuation", "tr_en", "turkish_context")[group],
                       "font_pixels": SIZES[n % len(SIZES)], "recipe": RECIPES[(n // 4 if count >= 128 else n) % len(RECIPES)],
                       "raster_seed": rng.randrange(2**32)})
    if len({row["text"] for row in result}) != count:
        raise DataError("duplicate generated line")
    return result


def wordstr(text: str, width: int, height: int) -> str:
    label_ok(text)
    if not 1 <= width <= MAX_WIDTH or not 1 <= height <= MAX_HEIGHT or width * height > MAX_PIXELS:
        raise DataError("line image dimensions exceed bounds")
    return f"WordStr 0 0 {width} {height} 0 #{text}\n\t 0 0 {width} {height} 0\n"


def render(row: dict, font_path: Path) -> Image.Image:
    font = ImageFont.truetype(str(font_path), row["font_pixels"], layout_engine=ImageFont.Layout.BASIC)
    box = font.getbbox(row["text"], anchor="lt")
    width, height = math.ceil(box[2] - box[0]) + 40, math.ceil(box[3] - box[1]) + 28
    wordstr(row["text"], width, height)
    image = Image.new("L", (width, height), 255)
    ImageDraw.Draw(image).text((20 - box[0], 14 - box[1]), row["text"], font=font, anchor="lt", fill=0)
    rng = random.Random(row["raster_seed"])
    if row["recipe"] == "blur":
        image = image.filter(ImageFilter.GaussianBlur(0.35))
    elif row["recipe"] == "skew":
        image = image.rotate(rng.choice((-0.8, 0.8)), resample=Image.Resampling.BICUBIC, expand=True, fillcolor=255)
    elif row["recipe"] == "noise":
        # Sparse seeded grey specks; avoid replacing the image with a system renderer.
        for _ in range(round(width * height * 0.001)):
            image.putpixel((rng.randrange(2, width - 2), rng.randrange(2, height - 2)), rng.choice((80, 160, 220)))
    elif row["recipe"] != "clean":
        raise DataError("unknown fixed image recipe")
    wordstr(row["text"], *image.size)
    ink = ImageOps.invert(image).getbbox()
    if ink is None or ink[0] <= 0 or ink[1] <= 0 or ink[2] >= image.width or ink[3] >= image.height:
        raise DataError("rendered ink is empty or clipped at the image edge")
    return image


def output_path(evidence_root: Path, output: Path, repo: Path, *, new: bool) -> Path:
    root = evidence_root.resolve(strict=True)
    if not root.is_dir() or root.is_relative_to(repo):
        raise DataError("evidence-root must be an existing directory outside the repository")
    # Parent must already exist; a new output directory is reserved once, never reused.
    parent = output.absolute().parent.resolve(strict=True)
    target = parent / output.name
    if not target.is_relative_to(root) or target == root or target.is_relative_to(repo):
        raise DataError("output must be a child of evidence-root, outside the repository")
    if target.is_symlink() or new and target.exists():
        raise DataError("output already exists or is a link; choose a new directory")
    if not new and (not target.is_dir() or target.resolve(strict=True) != target):
        raise DataError("verification needs an existing real dataset directory")
    return target


class Writer:
    def __init__(self, root: Path):
        self.root = root
        self.total = 0
        self.records: dict[str, dict] = {}

    def put(self, relative: str, data: bytes) -> dict:
        path = self.root / relative
        if len(data) > MAX_FILE or self.total + len(data) > MAX_TOTAL:
            raise DataError("output byte budget exceeded (4MiB/file,256MiB/dataset)")
        if path.parent.resolve(strict=True) != path.parent or not path.parent.is_relative_to(self.root):
            raise DataError("output parent changed or escaped the dataset")
        with path.open("xb") as stream:
            stream.write(data)
        row = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
        self.records[relative] = row
        self.total += len(data)
        return row

    def json(self, relative: str, data: dict) -> dict:
        return self.put(relative, (json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8"))


def make_dataset(args: argparse.Namespace) -> dict:
    repo = Path(__file__).resolve().parents[2]
    if not 0 <= args.seed < 2**32:
        raise DataError("seed must be an integer in0..4294967295")
    out = output_path(args.evidence_root, args.output, repo, new=True)
    font = pinned(args.font, FONT_SHA, MAX_FILE)
    model = pinned(args.base_model, MODEL_SHA, 128 * 1024 * 1024)
    alphabet_file = pinned(args.base_unicharset, CHARSET_SHA, 1024 * 1024)
    splits = {"training": labels("training", args.train_lines, args.seed),
              "internal-val": labels("internal-val", args.val_lines, args.seed)}
    if {r["text"] for r in splits["training"]} & {r["text"] for r in splits["internal-val"]}:
        raise DataError("training and internal validation labels overlap")
    all_chars = set("".join(row["text"] for rows in splits.values() for row in rows))
    missing_font = font_covers(font.read_bytes(), all_chars)
    missing_model = all_chars - read_alphabet(alphabet_file)
    if missing_font or missing_model:
        raise DataError(f"missing font glyphs={sorted(missing_font)!r}; missing base-model characters={sorted(missing_model)!r}")
    # Check every label geometry before reserving the dataset; actual rasters stay bounded.
    for rows in splits.values():
        for row in rows:
            f = ImageFont.truetype(str(font), row["font_pixels"], layout_engine=ImageFont.Layout.BASIC)
            b = f.getbbox(row["text"], anchor="lt")
            wordstr(row["text"], math.ceil(b[2] - b[0]) + 40, math.ceil(b[3] - b[1]) + 28)
    out.mkdir(exist_ok=False)
    writer = Writer(out)
    for split in splits:
        (out / split).mkdir(exist_ok=False)
    records = []
    for split, rows in splits.items():
        for row in rows:
            image = render(row, font)
            stem = f"{split}/{row['id']}"
            files = {}
            for suffix, fmt, options in [("png", "PNG", {"compress_level": 9}), ("tif", "TIFF", {"compression": "raw"})]:
                buffer = io.BytesIO()
                image.save(buffer, format=fmt, dpi=(300, 300), **options)
                files[suffix] = {"path": f"{stem}.{suffix}", **writer.put(f"{stem}.{suffix}", buffer.getvalue())}
            for suffix, data in [("gt.txt", row["text"] + "\n"), ("box", wordstr(row["text"], *image.size))]:
                files[suffix] = {"path": f"{stem}.{suffix}", **writer.put(f"{stem}.{suffix}", data.encode("utf-8"))}
            records.append({**row, "split": split, "width": image.width, "height": image.height,
                            "box": [0, 0, image.width, image.height, 0], "files": files})
    for split, name in [("training", "training-files.txt"), ("internal-val", "internal-val-files.txt")]:
        writer.put(name, ("\n".join(r["files"]["tif"]["path"] for r in records if r["split"] == split) + "\n").encode("utf-8"))
    statistics = {}
    for split, rows in splits.items():
        counter = Counter("".join(r["text"] for r in rows))
        statistics[split] = {"rows": len(rows), "groups": dict(Counter(r["group"] for r in rows)),
                             "target_character_counts": {c: counter[c] for c in TARGETS}, "unicode_character_counts": dict(sorted(counter.items()))}
    manifest = {"schema": "pdfcraft-original-turkish-lines-v1", "state": "INPUTS_FROZEN_BEFORE_TRAINING",
                "seed": args.seed, "counts": {k: len(v) for k, v in splits.items()}, "statistics": statistics,
                "label_policy": "contributor-original fixed banks; NFC; single line; UTF-8 LF; separate banks and no identical train/val label",
                "license": "MIT OR Apache-2.0 original labels/pixels; no third-party prose or copied evaluation labels",
                "font": {"path": str(font), "sha256": FONT_SHA, "license": "OFL-1.1", "fallback": False, "missing_characters": sorted(missing_font)},
                "base_model": {"path": str(model), "sha256": MODEL_SHA, "pack": "script/Latin", "source_commit": "e12c65a915945e4c28e237a9b52bc4a8f39a0cec", "license": "Apache-2.0"},
                "base_alphabet": {"path": str(alphabet_file), "sha256": CHARSET_SHA, "missing_label_characters": sorted(missing_model), "coverage": "exact single-character entries, space as separator; not an accuracy claim"},
                "label_alphabet": [{"character": c, "codepoint": f"U+{ord(c):04X}"} for c in sorted(all_chars)],
                "rendering": {"font_pixels": list(SIZES), "dpi": 300, "mode": "L", "layout": "one line; horizontal margin20px, vertical14px; font loaded by absolute path BASIC layout",
                              "recipes": {"clean": "unchanged", "blur": "Gaussian0.35px", "skew": "seeded +/-0.8deg bicubic expand white", "noise": "seeded0.1% grey specks"},
                              "limits": {"width": MAX_WIDTH, "height": MAX_HEIGHT, "pixels": MAX_PIXELS, "file_bytes": MAX_FILE, "dataset_bytes": MAX_TOTAL}},
                "producer": {"source_sha256": digest(Path(__file__).resolve(), MAX_FILE), "python": platform.python_version(), "pillow": pillow_version},
                "primary_format_references": PRIMARY_DOCS, "files": writer.records.copy(), "samples": records,
                "not_run": ["OCR", "lstmf generation", "training", "holdout evaluation"], "independent_holdout": "not loaded or used"}
    frozen = writer.json("manifest.json", manifest)
    writer.json("FROZEN.json", {"state": "FROZEN_DATA_READY", "manifest": "manifest.json", "manifest_sha256": frozen["sha256"],
                               "manifest_bytes": frozen["bytes"], "training_count": args.train_lines, "internal_val_count": args.val_lines})
    return {"status": "FROZEN_DATA_READY", "output": str(out), "manifest_sha256": frozen["sha256"], "files": len(writer.records), "bytes": writer.total}


def verify_dataset(root: Path) -> dict:
    seal = json.loads((root / "FROZEN.json").read_text(encoding="utf-8"))
    if digest(root / "manifest.json", MAX_FILE) != seal["manifest_sha256"]:
        raise DataError("dataset manifest hash mismatch")
    manifest = json.loads((root / "manifest.json").read_text(encoding="utf-8"))
    for rel, expected in manifest["files"].items():
        path = root / rel
        if not path.resolve(strict=True).is_relative_to(root) or path.is_symlink():
            raise DataError("dataset contains an escaping link")
        if digest(path, MAX_FILE) != expected["sha256"] or path.stat().st_size != expected["bytes"]:
            raise DataError(f"dataset file hash mismatch: {rel}")
    labels_by_split: dict[str, set] = {"training": set(), "internal-val": set()}
    for row in manifest["samples"]:
        stem = root / row["files"]["tif"]["path"]
        text = (root / row["files"]["gt.txt"]["path"]).read_text(encoding="utf-8")
        if text != row["text"] + "\n":
            raise DataError("GT text does not match manifest")
        label_ok(row["text"])
        if (root / row["files"]["box"]["path"]).read_text(encoding="utf-8") != wordstr(row["text"], row["width"], row["height"]):
            raise DataError("WordStr line/terminator mismatch")
        with Image.open(stem) as tif, Image.open(root / row["files"]["png"]["path"]) as png:
            if tif.size != (row["width"], row["height"]) or tif.size != png.size:
                raise DataError("image header geometry differs from frozen manifest")
            tif.load(); png.load()
            if tif.size != (row["width"], row["height"]) or tif.mode != "L" or tif.size != png.size or tif.tobytes() != png.tobytes():
                raise DataError("PNG/TIFF geometry or pixels differ")
        labels_by_split[row["split"]].add(row["text"])
    if labels_by_split["training"] & labels_by_split["internal-val"]:
        raise DataError("training/validation labels overlap")
    extra = verify_specialization_pixels(root, manifest) if manifest.get("data_mode") == "specialization-v2" else verify_counterbalanced(root, manifest) if manifest.get("data_mode") == "counterbalanced-v3" else {}
    return {"status": "VERIFIED_FROZEN_INPUTS_ONLY", "files": len(manifest["files"]), "samples": len(manifest["samples"]),
            "manifest_sha256": seal["manifest_sha256"], "ocr_or_training_run": False, **extra}


def self_test(evidence: Path, font: Path) -> bool:
    font = pinned(font, FONT_SHA, MAX_FILE)
    class Fixtures(unittest.TestCase):
        def test_deterministic_separate_balanced_labels(self):
            train, val = labels("training", 128, SEED), labels("internal-val", 32, SEED)
            self.assertEqual(train, labels("training", 128, SEED))
            self.assertFalse({r["text"] for r in train} & {r["text"] for r in val})
            for rows, expected in [(train, 16), (val, 4)]:
                counts = Counter("".join(r["text"] for r in rows))
                self.assertEqual([counts[c] for c in TARGETS], [expected] * 4)
                self.assertEqual(set(Counter(r["group"] for r in rows).values()), {expected})
        def test_invalid_labels_and_counts(self):
            for text in ["", "two\nlines", " a", "a\tword", "i\u0302", "x" * 129]:
                with self.assertRaises(DataError): label_ok(text)
            for count in [0, 15, 17, 1024]:
                with self.assertRaises(DataError): labels("training", count, SEED)
        def test_wordstr_whole_line_and_tab_terminator(self):
            self.assertEqual(wordstr("î Î û Û", 300, 80), "WordStr 0 0 300 80 0 #î Î û Û\n\t 0 0 300 80 0\n")
            with self.assertRaises(DataError): wordstr("x", MAX_WIDTH + 1, 80)
        def test_pinned_cmap_and_repeatable_render(self):
            self.assertEqual(font_covers(font.read_bytes(), set("îÎûÛıIİiçÇğĞöÖşŞüÜ")), set())
            self.assertEqual(font_covers(font.read_bytes(), {chr(0x10FFFF)}), {chr(0x10FFFF)})
            row = labels("training", 128, SEED)[0]
            self.assertEqual(render(row, font).tobytes(), render(row, font).tobytes())
        def test_no_clobber_escape_and_budgets(self):
            with tempfile.TemporaryDirectory(prefix="fixtures-", dir=evidence) as temp:
                root = Path(temp).resolve(); existing = root / "existing"; existing.mkdir()
                with self.assertRaises(DataError): output_path(root, existing, Path(__file__).resolve().parents[2], new=True)
                with self.assertRaises(DataError): output_path(root, root.parent / "escape", Path(__file__).resolve().parents[2], new=True)
                writer = Writer(root); writer.put("keep.txt", b"original")
                with self.assertRaises(FileExistsError): writer.put("keep.txt", b"changed")
                with self.assertRaises(DataError): writer.put("large.txt", b"x" * (MAX_FILE + 1))
                self.assertEqual((root / "keep.txt").read_bytes(), b"original")
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(Fixtures))
    return result.wasSuccessful()



# V2 is deliberately independent of BANKS/labels(): no old validation/reference text input.
V2_SEED = 14302236
V2_SIZES = (28, 34, 40, 46)
V2_SPACES = (0.8, 1.0, 1.4, 1.9)
FONT_PINS = {
    "Regular": ("Inter-Regular.ttf", FONT_SHA),
    "Medium": ("Inter-Medium.ttf", "97ad806f526e41546d46365bb3a393145f75b7b1568913db74549ad8b8dba872"),
    "SemiBold": ("Inter-SemiBold.ttf", "78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3"),
}
V2_BANKS = {
    "training": {
        "i": ("aslî", "talî", "fennî", "zihnî", "bedenî", "ruhî", "lafzî", "şeklî"),
        "u": ("mezkûr", "menkûl", "matbû", "huzûr", "mağrûr", "meşrû", "fütûr", "mecnûn"),
        "tr": ("Sarı sepet devrildi", "Beyaz kapak gevşek", "Sessiz gölet durgun", "Dikenli dal eğildi", "Bakır levha düzdür", "Çömlek fırına girdi", "Turna sürüsü yükseldi", "Çınar tohumu yeşerdi"),
        "en": ("quiet window", "silver handle", "small notebook", "amber pencil", "dry ribbon", "wide doorway", "stable counter", "paper basket"),
        "mix": ("Düğme ready kaldı", "Kilit için check seç", "Sepet empty göründü", "Taraf için left oku", "Kutuya red işaret", "Fırça soft seçildi", "Bölme clean olsun", "Görev için start ver"),
        "i_control": ("I ı İ i kıvılcım", "I ı İ i ısınma", "I ı İ i kıraç", "I ı İ i ıskarta", "I ı İ i kıtlık", "I ı İ i ısrar", "I ı İ i kırpık", "I ı İ i kıymet"),
    },
    "internal-val": {
        "i": ("tarihî", "coğrafî", "nihaî", "farazî", "sathî", "nisbî", "dâhilî", "haricî"),
        "u": ("şümûl", "mahlûk", "maktûl", "mahzûn", "mahdûm", "vukûf", "sûra", "dûr"),
        "tr": ("Bordo perde toplandı", "Yamaçta kekik yetişir", "Çavdar demeti bağlandı", "Örtülü vagon durdu", "Gümüş toka parladı", "Öbekli sis çözüldü", "Serçe çitleği dolaştı", "Zeytin dalı büyüdü"),
        "en": ("bright ladder", "plain vessel", "green cushion", "short cable", "open bracket", "heavy parcel", "soft fabric", "fresh ledger"),
        "mix": ("Duvar light göründü", "Ölçü için size yaz", "Kemer loose kaldı", "Yastık blue seçildi", "Geçiş için pass ver", "Topak için round çiz", "Sınır clear olmalı", "Sıra için wait denir"),
        "i_control": ("İ i I ı kıpırtı", "İ i I ı ıstakoz", "İ i I ı kılçık", "İ i I ı ıstırap", "İ i I ı kırıntı", "İ i I ı ısırgan", "İ i I ı kısrak", "İ i I ı ısıl"),
    },
}
V2_GROUPS = ("isolated_spaced_hats", "spaced_hat_runs", "compact_hat_runs", "short_lower_i_hat", "short_upper_i_hat",
             "short_lower_u_hat", "short_upper_u_hat", "short_hat_context", "uppercase_hat_context", "spaced_word_pairs",
             "turkish_i_control", "ordinary_tr_control", "ascii_en_control", "number_punctuation_control", "tr_en_control", "hat_plain_contrast")


def attributed_fonts(repo: Path) -> dict:
    attribution = repo / "ATTRIBUTION.toml"
    digest(attribution, MAX_FILE)
    with attribution.open("rb") as stream:
        entries = tomllib.load(stream).get("asset", [])
    result = {}
    for name, (filename, expected) in FONT_PINS.items():
        relative = "assets/fonts/" + filename
        candidates = [entry for entry in entries if entry.get("path") == relative]
        if len(candidates) != 1:
            raise DataError(f"font needs exactly one attribution entry: {relative}")
        entry = candidates[0]
        if entry.get("sha256") != expected or entry.get("licence") != "OFL-1.1" or entry.get("kind") != "font":
            raise DataError(f"font attribution pin/license mismatch: {relative}")
        license_file = (repo / entry.get("licence_file", "")).resolve(strict=True)
        if not license_file.is_relative_to(repo):
            raise DataError("font license path escaped repository")
        path = pinned(repo / relative, expected, MAX_FILE)
        result[name] = {"path": str(path), "sha256": expected, "license": "OFL-1.1", "source": entry.get("source"),
                        "attribution_path": relative, "license_file": str(license_file), "license_sha256": digest(license_file, 128 * 1024)}
    return result


def hat_pattern(n: int, length: int, spaced: bool) -> str:
    # Each four-row family rotates every target equally; distinct canonical families cannot
    # collide because the first target encodes the rotation. No label identifiers are drawn.
    family, rotation = divmod(n, 4)
    digits = [0] + [(family // (4 ** pos) + pos + 1) % 4 for pos in range(length - 1)]
    tokens = [TARGETS[(digit + rotation) % 4] for digit in digits]
    return (" " if spaced else "").join(tokens)


def specialization_labels(split: str, count: int, seed: int) -> list[dict]:
    if split not in V2_BANKS or count != (512 if split == "training" else 128):
        raise DataError("specialization-v2 requires exactly512 training and128 internal-val lines")
    bank = V2_BANKS[split]
    result = []
    rng = random.Random(seed + (0 if split == "training" else 104729))
    for index in range(count):
        group, n = index % 16, index // 16
        slot, variant = n % 8, n // 8
        wi, wu = bank["i"][slot], bank["u"][slot]
        if group == 0:
            text = TARGETS[n % 4] if split == "training" and n < 16 else hat_pattern(n - 16 if split == "training" else n, 2 if split == "training" else 5, True)
        elif group in (1, 2):
            text = hat_pattern(n, (4 if group == 1 else 3) if split == "training" else (3 if group == 1 else 8), group == 1)
        else:
            forms = {3: wi, 4: upper_tr(wi), 5: wu, 6: upper_tr(wu), 7: f"{wi} ve {wu}", 8: upper_tr(f"{wi} ve {wu}"),
                     9: f"{wu} ile {wi}", 10: bank["i_control"][slot], 11: bank["tr"][slot], 12: bank["en"][slot],
                     13: (f"{71 + n}: {19 + n},50 / 5 = {4 + n}; %8 (B)" if split == "training" else f"[{350 + n} + 22] = {372 + n}; %9"),
                     14: bank["mix"][slot], 15: f"{wi} / {wi.replace('î', 'i')} - {wu} / {wu.replace('û', 'u')}"}
            text = forms[group]
            if variant and group != 13:
                prefix = ("", "draft ", "sample ", "entry ")[variant] if group == 12 else ("", "not: ", "kısa ", "son ")[variant]
                text = prefix + text
        text = unicodedata.normalize("NFC", text)
        label_ok(text)
        result.append({"id": ("train" if split == "training" else "ival") + f"-{index:04d}", "text": text, "group": V2_GROUPS[group],
                       "font_name": tuple(FONT_PINS)[(n + group) % 3], "font_pixels": V2_SIZES[(n // 3 + group) % 4],
                       "space_scale": V2_SPACES[(n + group * 3) % 4], "recipe": RECIPES[(n // 4 + group) % 4], "raster_seed": rng.randrange(2**32)})
    if len({row["id"] for row in result}) != count:
        raise DataError("specialization-v2 ids must be unique")
    if split == "internal-val" and len({row["text"] for row in result}) != count:
        raise DataError("specialization-v2 validation labels must be unique")
    return result


def spaced_layout(row: dict, path: Path) -> tuple:
    label_ok(row["text"])
    if row["font_pixels"] not in V2_SIZES or row["space_scale"] not in V2_SPACES:
        raise DataError("v2 font size or real-space scale is outside the fixed recipe")
    font = ImageFont.truetype(str(path), row["font_pixels"], layout_engine=ImageFont.Layout.BASIC)
    space = font.getlength(" ") * row["space_scale"]
    words = row["text"].split(" ")
    positions = []
    x = 0.0
    boxes = []
    for word in words:
        positions.append((word, x))
        box = font.getbbox(word, anchor="ls")
        boxes.append((x + box[0], box[1], x + box[2], box[3]))
        x += font.getlength(word) + space
    left = min(box[0] for box in boxes)
    top = min(box[1] for box in boxes)
    right = max(box[2] for box in boxes)
    bottom = max(box[3] for box in boxes)
    width, height = math.ceil(right - left) + 40, math.ceil(bottom - top) + 28
    wordstr(row["text"], width, height)
    return font, positions, (width, height), (20 - left, 14 - top), space


def render_specialization(row: dict, path: Path) -> tuple[Image.Image, dict]:
    font, positions, size, origin, space = spaced_layout(row, path)
    image = Image.new("L", size, 255)
    draw = ImageDraw.Draw(image)
    for word, x in positions:
        draw.text((origin[0] + x, origin[1]), word, font=font, anchor="ls", fill=0)
    rng = random.Random(row["raster_seed"])
    if row["recipe"] == "blur":
        image = image.filter(ImageFilter.GaussianBlur(0.35))
    elif row["recipe"] == "skew":
        image = image.rotate(rng.choice((-0.8, 0.8)), resample=Image.Resampling.BICUBIC, expand=True, fillcolor=255)
    elif row["recipe"] == "noise":
        for _ in range(round(image.width * image.height * 0.001)):
            image.putpixel((rng.randrange(2, image.width - 2), rng.randrange(2, image.height - 2)), rng.choice((80, 160, 220)))
    elif row["recipe"] != "clean":
        raise DataError("unknown v2 recipe")
    wordstr(row["text"], *image.size)
    ink = ImageOps.invert(image).getbbox()
    if ink is None or ink[0] <= 0 or ink[1] <= 0 or ink[2] >= image.width or ink[3] >= image.height:
        raise DataError("v2 ink is empty or clipped")
    return image, {"space_advance_px": space, "spacing_method": "ASCII-space advance scaled between separately drawn words on one shared baseline; GT unchanged"}


def pixel_hash(image: Image.Image) -> str:
    return hashlib.sha256(struct.pack(">II", image.width, image.height) + image.tobytes()).hexdigest()


def split_disjoint(records: list[dict]) -> dict:
    sets = {}
    for split in ("training", "internal-val"):
        selected = [row for row in records if row["split"] == split]
        texts = {row["text"] for row in selected}
        if len({row["id"] for row in selected}) != len(selected):
            raise DataError("duplicate v2 id within split")
        if split == "internal-val" and len(texts) != len(selected):
            raise DataError("duplicate v2 validation text")
        pixels = {row["pixel_sha256"] for row in selected}
        if len(pixels) != len(selected):
            raise DataError("duplicate v2 decoded pixels within split")
        sets[split] = (texts, pixels)
    text_overlap = sets["training"][0] & sets["internal-val"][0]
    pixel_overlap = sets["training"][1] & sets["internal-val"][1]
    if text_overlap or pixel_overlap:
        raise DataError("v2 train/internal-val text or decoded-pixel overlap")
    return {"train_val_text_overlap": 0, "train_val_decoded_pixel_overlap": 0}


def make_specialization_dataset(args: argparse.Namespace) -> dict:
    repo = Path(__file__).resolve().parents[2]
    if not 0 <= args.seed < 2**32:
        raise DataError("seed must be in0..4294967295")
    out = output_path(args.evidence_root, args.output, repo, new=True)
    if args.font.resolve(strict=True) != (repo / "assets/fonts/Inter-Regular.ttf").resolve(strict=True):
        raise DataError("v2 loads only its three attributed Inter paths; do not override --font")
    fonts = attributed_fonts(repo)
    model = pinned(args.base_model, MODEL_SHA, 128 * 1024 * 1024)
    alphabet_file = pinned(args.base_unicharset, CHARSET_SHA, 1024 * 1024)
    splits = {"training": specialization_labels("training", args.train_lines, args.seed),
              "internal-val": specialization_labels("internal-val", args.val_lines, args.seed)}
    if {row["text"] for row in splits["training"]} & {row["text"] for row in splits["internal-val"]}:
        raise DataError("v2 cross-split labels overlap")
    chars = set("".join(row["text"] for rows in splits.values() for row in rows))
    missing_model = chars - read_alphabet(alphabet_file)
    for metadata in fonts.values():
        missing = font_covers(Path(metadata["path"]).read_bytes(), chars)
        metadata["missing_characters"] = sorted(missing)
        if missing:
            raise DataError(f"v2 font lacks required glyphs: {missing!r}")
    if missing_model:
        raise DataError(f"v2 base model alphabet lacks label characters: {missing_model!r}")
    for rows in splits.values():
        for row in rows:
            spaced_layout(row, Path(fonts[row["font_name"]]["path"]))
    out.mkdir(exist_ok=False)
    writer = Writer(out)
    for split in splits:
        (out / split).mkdir(exist_ok=False)
    records = []
    for split, rows in splits.items():
        for row in rows:
            image, geometry = render_specialization(row, Path(fonts[row["font_name"]]["path"]))
            stem = f"{split}/{row['id']}"
            files = {}
            for suffix, fmt, options in [("png", "PNG", {"compress_level": 9}), ("tif", "TIFF", {"compression": "raw"})]:
                buffer = io.BytesIO(); image.save(buffer, format=fmt, dpi=(300, 300), **options)
                files[suffix] = {"path": f"{stem}.{suffix}", **writer.put(f"{stem}.{suffix}", buffer.getvalue())}
            for suffix, data in [("gt.txt", row["text"] + "\n"), ("box", wordstr(row["text"], *image.size))]:
                files[suffix] = {"path": f"{stem}.{suffix}", **writer.put(f"{stem}.{suffix}", data.encode("utf-8"))}
            records.append({**row, **geometry, "split": split, "width": image.width, "height": image.height,
                            "box": [0, 0, image.width, image.height, 0], "pixel_sha256": pixel_hash(image), "files": files})
    disjoint = split_disjoint(records)
    for split, name in [("training", "training-files.txt"), ("internal-val", "internal-val-files.txt")]:
        writer.put(name, ("\n".join(row["files"]["tif"]["path"] for row in records if row["split"] == split) + "\n").encode("utf-8"))
    statistics = {}
    for split in splits:
        rows = [row for row in records if row["split"] == split]
        chars_count = Counter("".join(row["text"] for row in rows))
        statistics[split] = {"rows": len(rows), "groups": dict(Counter(row["group"] for row in rows)),
                             "target_character_counts": {c: chars_count[c] for c in TARGETS}, "unique_texts": len({row["text"] for row in rows}), "unique_decoded_pixels": len({row["pixel_sha256"] for row in rows}),
                             "fonts": dict(Counter(row["font_name"] for row in rows)), "recipes": dict(Counter(row["recipe"] for row in rows)),
                             "space_scales": dict(Counter(str(row["space_scale"]) for row in rows)),
                             "unicode_character_counts": dict(sorted(chars_count.items()))}
    manifest = {"schema": "pdfcraft-original-turkish-specialization-v2", "data_mode": "specialization-v2", "state": "INPUTS_FROZEN_BEFORE_TRAINING",
                "seed": args.seed, "counts": {split: len(rows) for split, rows in splits.items()}, "statistics": statistics, "split_independence": disjoint,
                "text_origin": "new V2_BANKS only; no BANKS/legacy labels/evaluation/holdout file input; separate lexical banks and pattern lengths for train/val",
                "label_policy": "single NFC UTF-8 LF line; literal ASCII-space GT; real raster-space widths vary without changing GT",
                "label_repetition_policy": "Train labels may repeat across distinct pixels/fonts/sizes; no drawn serial identifier; single-character hats only in training. Validation labels unique. All ids and decoded pixels unique per split; cross-split text/pixel overlap zero.",
                "focus_share": {"training": {"focus": 352, "control": 160}, "internal-val": {"focus": 88, "control": 40}, "focus_groups": list(V2_GROUPS[:10]) + [V2_GROUPS[15]]},
                "license": "MIT OR Apache-2.0 original labels/pixels", "fonts": fonts, "font_fallback": False,
                "attribution_sha256": digest(repo / "ATTRIBUTION.toml", MAX_FILE),
                "base_model": {"path": str(model), "sha256": MODEL_SHA, "pack": "script/Latin", "license": "Apache-2.0"},
                "base_alphabet": {"path": str(alphabet_file), "sha256": CHARSET_SHA, "missing_label_characters": sorted(missing_model)},
                "label_alphabet": [{"character": c, "codepoint": f"U+{ord(c):04X}"} for c in sorted(chars)],
                "rendering": {"font_pixels": list(V2_SIZES), "space_scales": list(V2_SPACES), "mode": "L", "dpi": 300, "layout": "one shared baseline, per-word BASIC direct-font rendering;20px horizontal14px vertical margins",
                              "recipes": {"clean": "unchanged", "blur": "Gaussian0.35px", "skew": "seeded +/-0.8deg bicubic expand", "noise": "seeded0.1% grey specks"},
                              "limits": {"width": MAX_WIDTH, "height": MAX_HEIGHT, "pixels": MAX_PIXELS, "file_bytes": MAX_FILE, "dataset_bytes": MAX_TOTAL}},
                "producer": {"source_sha256": digest(Path(__file__).resolve(), MAX_FILE), "python": platform.python_version(), "pillow": pillow_version},
                "primary_format_references": PRIMARY_DOCS, "files": writer.records.copy(), "samples": records,
                "not_run": ["OCR", "lstmf conversion", "model training", "independent holdout"], "independent_holdout": "not read"}
    frozen = writer.json("manifest.json", manifest)
    writer.json("FROZEN.json", {"state": "FROZEN_DATA_READY", "manifest": "manifest.json", "manifest_sha256": frozen["sha256"],
                               "training_count": 512, "internal_val_count": 128})
    return {"status": "FROZEN_DATA_READY", "data_mode": "specialization-v2", "output": str(out), "manifest_sha256": frozen["sha256"],
            "files": len(writer.records), "bytes": writer.total, **disjoint}


def verify_specialization_pixels(root: Path, manifest: dict) -> dict:
    records = manifest["samples"]
    for row in records:
        with Image.open(root / row["files"]["tif"]["path"]) as image:
            wordstr(row["text"], *image.size)
            image.load()
            if pixel_hash(image) != row["pixel_sha256"]:
                raise DataError("v2 decoded pixel hash mismatch")
    return split_disjoint(records)


def specialization_self_test(evidence: Path) -> bool:
    repo = Path(__file__).resolve().parents[2]
    fonts = attributed_fonts(repo)
    class SpecializationFixtures(unittest.TestCase):
        def test_new_mode_unique_separate_and_focus_majority(self):
            train, val = specialization_labels("training", 512, V2_SEED), specialization_labels("internal-val", 128, V2_SEED)
            self.assertEqual(train, specialization_labels("training", 512, V2_SEED))
            self.assertEqual(len({row["text"] for row in train}), 500)
            self.assertEqual({row["text"] for row in train if len(row["text"]) == 1}, set(TARGETS))
            self.assertFalse(any(len(row["text"]) == 1 for row in val))
            self.assertTrue(any(row["text"] == "î Î" for row in train))
            self.assertEqual(len({row["text"] for row in val}), 128)
            self.assertFalse({row["text"] for row in train} & {row["text"] for row in val})
            self.assertEqual(set(Counter(row["group"] for row in train).values()), {32})
            self.assertEqual(set(Counter(row["group"] for row in val).values()), {8})
            self.assertEqual(sum(row["group"] in V2_GROUPS[:10] + (V2_GROUPS[15],) for row in train), 352)
        def test_v2_does_not_access_legacy_banks(self):
            previous = globals()["BANKS"]
            globals()["BANKS"] = None
            try:
                self.assertEqual(len(specialization_labels("training", 512, V2_SEED)), 512)
                self.assertEqual(len(specialization_labels("internal-val", 128, V2_SEED)), 128)
            finally:
                globals()["BANKS"] = previous
        def test_three_pinned_fonts_cover_every_label_character(self):
            chars = set("".join(row["text"] for split, count in [("training", 512), ("internal-val", 128)] for row in specialization_labels(split, count, V2_SEED)))
            self.assertEqual(set(fonts), {"Regular", "Medium", "SemiBold"})
            for metadata in fonts.values():
                self.assertEqual(font_covers(Path(metadata["path"]).read_bytes(), chars), set())
                self.assertEqual(metadata["license"], "OFL-1.1")
        def test_real_space_geometry_changes_without_changing_gt(self):
            row = specialization_labels("training", 512, V2_SEED)[16]
            row.update(text="î Î û Û", recipe="clean", font_pixels=40, space_scale=0.8)
            small, a = render_specialization(row, Path(fonts["Regular"]["path"]))
            row["space_scale"] = 1.9
            wide, b = render_specialization(row, Path(fonts["Regular"]["path"]))
            self.assertGreater(wide.width, small.width)
            self.assertGreater(b["space_advance_px"], a["space_advance_px"])
            self.assertEqual(row["text"], "î Î û Û")
        def test_wrong_counts_and_raster_recipe_are_rejected(self):
            for count in (0, 128, 511, 513, 1024):
                with self.assertRaises(DataError): specialization_labels("training", count, V2_SEED)
            row = specialization_labels("training", 512, V2_SEED)[0]
            for key, value in [("font_pixels", 9999), ("space_scale", 100000)]:
                bad = dict(row); bad[key] = value
                with self.assertRaises(DataError): spaced_layout(bad, Path(fonts["Regular"]["path"]))
        def test_text_and_raw_pixel_overlap_are_both_refused(self):
            with self.assertRaises(DataError): split_disjoint([{"id": "a", "split": "training", "text": "î", "pixel_sha256": "a"}, {"id": "b", "split": "internal-val", "text": "î", "pixel_sha256": "b"}])
            with self.assertRaises(DataError): split_disjoint([{"id": "a", "split": "training", "text": "î", "pixel_sha256": "a"}, {"id": "b", "split": "internal-val", "text": "Î", "pixel_sha256": "a"}])
        def test_repeated_train_label_requires_different_ids_and_pixels(self):
            base = {"id": "a", "split": "training", "text": "î", "pixel_sha256": "a"}
            alternate = {"id": "b", "split": "training", "text": "î", "pixel_sha256": "b"}
            self.assertEqual(split_disjoint([base, alternate])["train_val_decoded_pixel_overlap"], 0)
            with self.assertRaises(DataError): split_disjoint([base, {**alternate, "pixel_sha256": "a"}])
            with self.assertRaises(DataError): split_disjoint([base, {**alternate, "id": "a"}])
        def test_wrong_attribution_is_rejected_before_loading_a_font(self):
            with tempfile.TemporaryDirectory(prefix="v2-attribution-", dir=evidence) as temp:
                root = Path(temp)
                (root / "ATTRIBUTION.toml").write_text('[[asset]]\npath="assets/fonts/Inter-Regular.ttf"\nkind="font"\nlicence="OFL-1.1"\nsha256="wrong"\n', encoding="utf-8")
                with self.assertRaisesRegex(DataError, "attribution pin/license"): attributed_fonts(root)
    return unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(SpecializationFixtures)).wasSuccessful()



# V3 consumes an explicitly hash-pinned design frozen before any new raster is produced.
V3_SEED = 15302355
V3_FAMILIES = {
    "I_turkish": ("i", "ı", "I", "İ", "î", "Î"),
    "O_turkish": ("O", "Ö", "o", "ö"),
    "U_turkish": ("U", "Ü", "Û", "u", "ü", "û"),
    "A_turkish": ("A", "Â", "a", "â"),
    "E_latin": ("E", "Ê", "e", "ê"),
}
V3_E_POLICY = {
    "role": "latin_non_turkish_control", "positive_rows": 48, "positive_training_share": 48 / 512,
    "purpose": "Visual E/Ê/e/ê contrast only. Ê/ê are not Turkish letters or positive Turkish-word examples.",
    "required_metric": "latin_control_falsepositives(reference,hypothesis)",
    "metric_fields": ["plain_E_to_circumflex", "inserted_circumflex", "other_to_circumflex", "unexpected_total", "positive_control_case_errors"],
    "metric_status": "NOT_EVALUATED: no OCR/training run by producer; future model owner must report unexpected Ê/ê on plain Turkish E/e contexts before qualification",
    "no_postcorrection": True,
}


def v3_matrix_audit(rows: list[dict]) -> dict:
    if len(rows) != 240 or len({r["id"] for r in rows}) != 240:
        raise DataError("v3 matrix needs240 unique ids")
    cells = Counter()
    report = {}
    for family, glyphs in V3_FAMILIES.items():
        selected = [r for r in rows if r["matrix_family"] == family]
        if len(selected) != 48:
            raise DataError("v3 family must contain48 rows")
        for row in selected:
            role = "latin_non_turkish_control" if family == "E_latin" else "turkish_control"
            if row["language_role"] != role or row["font_name"] not in FONT_PINS or row["font_pixels"] not in V2_SIZES or row["space_scale"] not in V2_SPACES or row["recipe"] != "clean":
                raise DataError("v3 family role/font/size/gap/clean recipe mismatch")
            tokens = row["glyph_permutation"]
            if len(tokens) != len(glyphs) or set(tokens) != set(glyphs) or row["text"] != " ".join(tokens):
                raise DataError("v3 matrix GT must contain each frozen family glyph once with single ASCII spaces")
            label_ok(row["text"])
            cells[(family, row["font_name"], row["font_pixels"], row["space_scale"])] += 1
        if set(Counter(r["font_name"] for r in selected).values()) != {16} or set(Counter(r["font_pixels"] for r in selected).values()) != {12} or set(Counter(r["space_scale"] for r in selected).values()) != {12}:
            raise DataError("v3 font/size/gap margins are unbalanced")
        glyph_report = {}
        for glyph in glyphs:
            positions = Counter(r["glyph_permutation"].index(glyph) for r in selected)
            if len(positions) != len(glyphs) or set(positions.values()) != {48 // len(glyphs)}:
                raise DataError("v3 case/glyph positions are unbalanced")
            by_font = {font: sum(r["text"].count(glyph) for r in selected if r["font_name"] == font) for font in FONT_PINS}
            by_size = {str(size): sum(r["text"].count(glyph) for r in selected if r["font_pixels"] == size) for size in V2_SIZES}
            by_gap = {str(gap): sum(r["text"].count(glyph) for r in selected if r["space_scale"] == gap) for gap in V2_SPACES}
            if set(by_font.values()) != {16} or set(by_size.values()) != {12} or set(by_gap.values()) != {12}:
                raise DataError("v3 conditional glyph/case/font/size/gap counts failed")
            glyph_report[glyph] = {"total": 48, "by_font": by_font, "by_size": by_size, "by_gap": by_gap, "positions": dict(positions)}
        report[family] = {"rows": 48, "glyphs": glyph_report}
    expected_cells = {(family, font, size, gap) for family in V3_FAMILIES for font in FONT_PINS for size in V2_SIZES for gap in V2_SPACES}
    if set(cells) != expected_cells or set(cells.values()) != {1}:
        raise DataError("v3 full crossed matrix has a missing or duplicated cell")
    return {"matrix_rows": 240, "unique_cells": len(cells), "families": report}


def latin_control_falsepositives(reference: str, hypothesis: str) -> dict:
    """Read-only raw-codepoint counter; never changes output or runs recognition.

    Correctly expected Ê/ê in the separate Latin block is not a Turkish false positive.
    Exact case mistakes on those controls are separately visible rather than hidden.
    """
    if len(reference) > 8192 or len(hypothesis) > 8192 or (len(reference) + 1) * (len(hypothesis) + 1) > 2_000_000:
        raise DataError("Ê false-positive alignment exceeds bounded diagnostic size")
    grid = [list(range(len(hypothesis) + 1))]
    for i, expected in enumerate(reference, 1):
        row = [i]
        for j, actual in enumerate(hypothesis, 1):
            row.append(min(row[-1] + 1, grid[-1][j] + 1, grid[-1][j - 1] + (expected != actual)))
        grid.append(row)
    pairs = []
    i, j = len(reference), len(hypothesis)
    while i or j:
        if i and j and grid[i][j] == grid[i - 1][j - 1] + (reference[i - 1] != hypothesis[j - 1]):
            pairs.append((i - 1, j - 1)); i -= 1; j -= 1
        elif i and grid[i][j] == grid[i - 1][j] + 1:
            pairs.append((i - 1, None)); i -= 1
        else:
            pairs.append((None, j - 1)); j -= 1
    counts = {"plain_E_to_circumflex": 0, "inserted_circumflex": 0, "other_to_circumflex": 0, "positive_control_case_errors": 0}
    details = []
    for ri, oi in reversed(pairs):
        expected = reference[ri] if ri is not None else ""
        actual = hypothesis[oi] if oi is not None else ""
        if actual not in ("Ê", "ê"):
            continue
        if expected in ("Ê", "ê"):
            if expected != actual:
                counts["positive_control_case_errors"] += 1
            continue
        kind = "plain_E_to_circumflex" if expected in ("E", "e") else "inserted_circumflex" if ri is None else "other_to_circumflex"
        counts[kind] += 1
        details.append({"reference_offset": ri, "output_offset": oi, "expected": expected, "actual": actual, "kind": kind})
    return {**counts, "unexpected_total": sum(counts[k] for k in ("plain_E_to_circumflex", "inserted_circumflex", "other_to_circumflex")),
            "plain_E_e_denominator": reference.count("E") + reference.count("e"), "expected_positive_control_count": reference.count("Ê") + reference.count("ê"),
            "details": details, "postcorrection": False}


def load_v3_design(args: argparse.Namespace) -> tuple:
    if args.v3_design is None or args.v3_design_sha256 is None or len(args.v3_design_sha256) != 64 or any(c not in "0123456789abcdef" for c in args.v3_design_sha256):
        raise DataError("v3 requires --v3-design and its lowercase64hex --v3-design-sha256")
    root = args.evidence_root.resolve(strict=True)
    path = args.v3_design.resolve(strict=True)
    if not path.is_relative_to(root) or path.is_symlink():
        raise DataError("v3 design must stay inside selected evidence-root")
    pinned(path, args.v3_design_sha256, MAX_FILE)
    plan = json.loads(path.read_text(encoding="utf-8"))
    marker_path = path.parent / "DESIGN-FROZEN.json"
    digest(marker_path, MAX_FILE)
    marker = json.loads(marker_path.read_text(encoding="utf-8"))
    if marker.get("status") != "DESIGN_FROZEN_BEFORE_DATA" or marker.get("design_sha256") != args.v3_design_sha256 or marker.get("rendered_v3_data_before_freeze") != 0:
        raise DataError("v3 design must be frozen before rendering")
    if plan.get("status") != "DESIGN_FROZEN_BEFORE_DATA" or not 0 <= plan["seed"] < 2**32 or plan.get("rendered_v3_data_before_freeze") != 0:
        raise DataError("invalid v3 design status/seed")
    if args.seed != plan["seed"] or args.train_lines != 512 or args.val_lines != 128:
        raise DataError("v3 seed/counts must equal the pre-frozen512/128 design")
    source_manifest = Path(plan["v2_manifest"]).resolve(strict=True)
    if not source_manifest.is_relative_to(root):
        raise DataError("V2 dataset input must stay under selected evidence-root")
    pinned(source_manifest, plan["v2_manifest_sha256"], MAX_FILE)
    source = json.loads(source_manifest.read_text(encoding="utf-8"))
    if source.get("data_mode") != "specialization-v2" or source["counts"] != {"training": 512, "internal-val": 128}:
        raise DataError("v3 requires the frozen512/128 V2 source dataset")
    source_rows = {r["id"]: r for r in source["samples"] if r["split"] == "training"}
    ids = plan["retained272_ids"]
    if len(ids) != 272 or len(set(ids)) != 272 or set(ids) - source_rows.keys():
        raise DataError("v3 needs272 distinct frozen V2 training ids")
    group_order = list(source["statistics"]["training"]["groups"])
    selected = []
    for gi, group in enumerate(group_order):
        rows = [r for r in source_rows.values() if r["group"] == group]
        if len(rows) != 32:
            raise DataError("unexpected V2 stratum size")
        selected.extend(r["id"] for r in random.Random(plan["seed"] + gi * 1009).sample(rows, 17))
    if sorted(selected, key=lambda name: int(name.rsplit("-", 1)[1])) != ids:
        raise DataError("v3 retained ids differ from frozen stratified seed rule")
    for expected in plan["retained272"]:
        row = source_rows[expected["source_id"]]
        if expected["files"] != row["files"] or expected["pixel_sha256"] != row["pixel_sha256"] or expected["text_sha256"] != hashlib.sha256((row["text"] + "\n").encode()).hexdigest():
            raise DataError("v3 retained source pins drifted")
    matrix_report = v3_matrix_audit(plan["matrix240"])
    order = plan["training512_order"]
    if len(order) != 512 or len(set(order)) != 512 or set(order) != set(ids) | {r["id"] for r in plan["matrix240"]}:
        raise DataError("v3 frozen training order/id coverage failed")
    return plan, source_manifest.parent, source, matrix_report


def copy_pinned_sample(source: Path, original: dict, writer: Writer, origin: str) -> dict:
    row = dict(original)
    copied = {}
    for suffix, metadata in original["files"].items():
        relative = metadata["path"]
        path = source / relative
        resolved = path.resolve(strict=True)
        if path.is_symlink() or not resolved.is_relative_to(source) or not relative.startswith(("training/", "internal-val/")):
            raise DataError("v3 copied sample escaped frozen source")
        if digest(path, MAX_FILE) != metadata["sha256"] or path.stat().st_size != metadata["bytes"]:
            raise DataError("v3 copied source file hash/size mismatch")
        data = path.read_bytes()
        if len(data) > MAX_FILE or hashlib.sha256(data).hexdigest() != metadata["sha256"]:
            raise DataError("v3 source changed while copying")
        copied[suffix] = {"path": relative, **writer.put(relative, data)}
    row.update(files=copied, origin=origin, v2_source_id=original["id"])
    return row


def v3_role_guard(records: list[dict]) -> dict:
    positives = [r for r in records if any(c in r["text"] for c in "Êê")]
    if len(positives) != 48 or any(r.get("language_role") != "latin_non_turkish_control" or r.get("matrix_family") != "E_latin" or r.get("origin") != "control_matrix" for r in positives):
        raise DataError("Ê/ê positives must stay inside48 isolated Latin control rows")
    return {"positive_E_latin_rows": 48, "positive_E_latin_share": 48 / 512, "Ê_outside_Latin_control": 0,
            "ê_outside_Latin_control": 0, "prediction_false_positive_metric": dict(V3_E_POLICY)}


def make_counterbalanced_dataset(args: argparse.Namespace) -> dict:
    repo = Path(__file__).resolve().parents[2]
    out = output_path(args.evidence_root, args.output, repo, new=True)
    plan, source_dir, source, matrix_report = load_v3_design(args)
    if args.font.resolve(strict=True) != (repo / "assets/fonts/Inter-Regular.ttf").resolve(strict=True):
        raise DataError("v3 uses only the three attributed Inter paths; do not override --font")
    fonts = attributed_fonts(repo)
    model = pinned(args.base_model, MODEL_SHA, 128 * 1024 * 1024)
    alphabet_file = pinned(args.base_unicharset, CHARSET_SHA, 1024 * 1024)
    source_train = {r["id"]: r for r in source["samples"] if r["split"] == "training"}
    val = [r for r in source["samples"] if r["split"] == "internal-val"]
    matrix = {r["id"]: r for r in plan["matrix240"]}
    chars = set("".join(r["text"] for r in source["samples"])) | set("".join(r["text"] for r in matrix.values()))
    missing_model = chars - read_alphabet(alphabet_file)
    for metadata in fonts.values():
        missing = font_covers(Path(metadata["path"]).read_bytes(), chars)
        metadata["missing_characters"] = sorted(missing)
        if missing:
            raise DataError(f"v3 font coverage gap: {missing!r}")
    if missing_model:
        raise DataError(f"v3 model alphabet gap: {missing_model!r}")
    for row in matrix.values():
        spaced_layout(row, Path(fonts[row["font_name"]]["path"]))
    # Geometry/design pins precede exclusive reservation; frozen input bytes are checked again
    # during copying. Any later error leaves a diagnostic partial tree without a FROZEN seal.
    out.mkdir(exist_ok=False)
    writer = Writer(out)
    (out / "training").mkdir(); (out / "internal-val").mkdir()
    records = []
    for sid in plan["training512_order"]:
        if sid in source_train:
            records.append(copy_pinned_sample(source_dir, source_train[sid], writer, "retained_v2_training"))
            continue
        row = dict(matrix[sid])
        row.update(split="training", group="matrix_" + row["matrix_family"], origin="control_matrix")
        image, geometry = render_specialization(row, Path(fonts[row["font_name"]]["path"]))
        stem = "training/" + row["id"]
        files = {}
        for suffix, fmt, options in [("png", "PNG", {"compress_level": 9}), ("tif", "TIFF", {"compression": "raw"})]:
            buffer = io.BytesIO(); image.save(buffer, format=fmt, dpi=(300, 300), **options)
            files[suffix] = {"path": f"{stem}.{suffix}", **writer.put(f"{stem}.{suffix}", buffer.getvalue())}
        for suffix, text in [("gt.txt", row["text"] + "\n"), ("box", wordstr(row["text"], *image.size))]:
            files[suffix] = {"path": f"{stem}.{suffix}", **writer.put(f"{stem}.{suffix}", text.encode("utf-8"))}
        records.append({**row, **geometry, "files": files, "width": image.width, "height": image.height,
                        "box": [0, 0, image.width, image.height, 0], "pixel_sha256": pixel_hash(image)})
    for row in val:
        records.append(copy_pinned_sample(source_dir, row, writer, "retained_v2_internal_val"))
    role_report = v3_role_guard(records)
    disjoint = split_disjoint(records)
    writer.put("training-files.txt", ("\n".join(r["files"]["tif"]["path"] for r in records if r["split"] == "training") + "\n").encode())
    expected_val_list = source["files"]["internal-val-files.txt"]
    val_list = (source_dir / "internal-val-files.txt").read_bytes()
    if hashlib.sha256(val_list).hexdigest() != expected_val_list["sha256"]:
        raise DataError("V2 internal-val list pin drifted")
    writer.put("internal-val-files.txt", val_list)
    statistics = {}
    for split in ("training", "internal-val"):
        rows = [r for r in records if r["split"] == split]
        counter = Counter("".join(r["text"] for r in rows))
        statistics[split] = {"rows": len(rows), "unique_texts": len({r["text"] for r in rows}), "unique_pixels": len({r["pixel_sha256"] for r in rows}),
                             "fonts": dict(Counter(r["font_name"] for r in rows)), "recipes": dict(Counter(r["recipe"] for r in rows)),
                             "space_scales": dict(Counter(str(r["space_scale"]) for r in rows)), "unicode_character_counts": dict(sorted(counter.items()))}
    source_val_pins = {meta["path"]: {"sha256": meta["sha256"], "bytes": meta["bytes"]} for r in val for meta in r["files"].values()}
    source_retained_pins = {meta["path"]: {"sha256": meta["sha256"], "bytes": meta["bytes"]} for sid in plan["retained272_ids"] for meta in source_train[sid]["files"].values()}
    manifest = {"schema": "pdfcraft-counterbalanced-lines-v3", "data_mode": "counterbalanced-v3", "state": "INPUTS_FROZEN_BEFORE_TRAINING",
                "seed": plan["seed"], "design_sha256": args.v3_design_sha256, "design_freeze": "DESIGN_FROZEN_BEFORE_DATA; no v3 raster existed",
                "counts": {"training": 512, "internal-val": 128}, "composition": {"matrix": 240, "retained_v2_training": 272, "retained_v2_val": 128},
                "retained272_ids": plan["retained272_ids"], "training512_order": plan["training512_order"], "matrix_conditional_counts": matrix_report,
                "source_v2_manifest_sha256": plan["v2_manifest_sha256"], "source_v2_manifest_path": plan["v2_manifest"],
                "internal_val_exact_byte_pins": source_val_pins, "retained_training_exact_byte_pins": source_retained_pins,
                "internal_val_list_sha256": expected_val_list["sha256"], "internal_val_policy": "same512 V2 sidecar bytes/hashes; same128 list; no re-render/relabel",
                "E_control_policy": role_report, "false_positive_metric": dict(V3_E_POLICY), "statistics": statistics, "split_independence": disjoint,
                "text_origin": "fixed glyph-control permutations plus272 pre-frozen stratified V2 training rows; no development reference or holdout3 input",
                "label_policy": "NFC UTF-8 LF, one literal ASCII space in matrix; training labels may repeat across unique ids/pixels; no drawn identifiers",
                "fonts": fonts, "font_fallback": False, "attribution_sha256": digest(repo / "ATTRIBUTION.toml", MAX_FILE),
                "base_model": {"path": str(model), "sha256": MODEL_SHA, "pack": "script/Latin", "license": "Apache-2.0"},
                "base_alphabet": {"path": str(alphabet_file), "sha256": CHARSET_SHA, "missing_label_characters": sorted(missing_model)},
                "label_alphabet": [{"character": ch, "codepoint": f"U+{ord(ch):04X}"} for ch in sorted(chars)],
                "rendering": {"matrix_recipe": "clean", "matrix_space_scales": list(V2_SPACES), "matrix_sizes": list(V2_SIZES), "dpi": 300, "mode": "L",
                              "retained": "byte-identical existing V2 rasters/recipes", "limits": {"width": MAX_WIDTH, "height": MAX_HEIGHT, "pixels": MAX_PIXELS, "file_bytes": MAX_FILE, "dataset_bytes": MAX_TOTAL},
                              "causal_boundary": "This fixed composite control block also changes recipe/context mixture; do not attribute a future result solely to one glyph frequency, weight, spacing or clean-image factor."},
                "producer": {"source_sha256": digest(Path(__file__).resolve(), MAX_FILE), "python": platform.python_version(), "pillow": pillow_version},
                "license": "MIT OR Apache-2.0 original labels/pixels; Inter OFL-1.1; base model Apache-2.0", "primary_format_references": PRIMARY_DOCS,
                "files": writer.records.copy(), "samples": records, "not_run": ["OCR", "training", "LSTMF conversion", "model selection", "holdout3"], "prediction_metrics": None}
    frozen = writer.json("manifest.json", manifest)
    writer.json("FROZEN.json", {"state": "FROZEN_DATA_READY", "manifest": "manifest.json", "manifest_sha256": frozen["sha256"],
                               "design_sha256": args.v3_design_sha256, "training_count": 512, "internal_val_count": 128})
    return {"status": "FROZEN_DATA_READY", "data_mode": "counterbalanced-v3", "manifest_sha256": frozen["sha256"], "files": len(writer.records), "bytes": writer.total, **disjoint}


def verify_counterbalanced(root: Path, manifest: dict) -> dict:
    records = manifest["samples"]
    train = [r for r in records if r["split"] == "training"]
    val = [r for r in records if r["split"] == "internal-val"]
    matrix = [r for r in train if r.get("origin") == "control_matrix"]
    retained = [r for r in train if r.get("origin") == "retained_v2_training"]
    if len(train) != 512 or len(val) != 128 or len(retained) != 272 or [r["id"] for r in train] != manifest["training512_order"] or {r["id"] for r in retained} != set(manifest["retained272_ids"]):
        raise DataError("v3 240/272/128 or frozen order guard failed")
    audit = v3_matrix_audit(matrix)
    if audit != manifest["matrix_conditional_counts"]:
        # JSON serializes integer position keys to strings; compare canonical round trips.
        if json.loads(json.dumps(audit)) != manifest["matrix_conditional_counts"]:
            raise DataError("v3 conditional counts differ from frozen manifest")
    v3_role_guard(records)
    for collection, pins in [(val, manifest["internal_val_exact_byte_pins"]), (retained, manifest["retained_training_exact_byte_pins"])]:
        observed = {meta["path"]: {"sha256": meta["sha256"], "bytes": meta["bytes"]} for r in collection for meta in r["files"].values()}
        if observed != pins:
            raise DataError("v3 retained/V2 validation pins differ")
        for relative, expected in pins.items():
            if digest(root / relative, MAX_FILE) != expected["sha256"]:
                raise DataError("v3 retained/V2 validation bytes drifted")
    if digest(root / "internal-val-files.txt", MAX_FILE) != manifest["internal_val_list_sha256"]:
        raise DataError("v3 internal-val list is not byte-identical")
    extra = verify_specialization_pixels(root, manifest)
    return {**extra, "matrix_cells": 240, "retained_training_rows": 272, "unchanged_validation_rows": 128, "Ê_control_role_guard": True}


def counterbalanced_self_test(args: argparse.Namespace) -> bool:
    plan, source_dir, source, audit = load_v3_design(args)
    fonts = attributed_fonts(Path(__file__).resolve().parents[2])
    class CounterbalancedFixtures(unittest.TestCase):
        def test_design_was_frozen_before_render_and_strata_are272(self):
            self.assertEqual(len(plan["retained272_ids"]), 272)
            self.assertEqual(set(Counter(r["group"] for r in plan["retained272"]).values()), {17})
            self.assertEqual(plan["rendered_v3_data_before_freeze"], 0)
            self.assertEqual(audit["unique_cells"], 240)
        def test_conditional_counts_and_permutations_are_exact(self):
            for family in audit["families"].values():
                self.assertEqual(family["rows"], 48)
                for glyph in family["glyphs"].values():
                    self.assertEqual(glyph["total"], 48)
                    self.assertEqual(set(glyph["by_font"].values()), {16})
                    self.assertEqual(set(glyph["by_gap"].values()), {12})
                    self.assertEqual(set(glyph["by_size"].values()), {12})
        def test_wrong_cell_and_case_permutation_fail(self):
            bad = json.loads(json.dumps(plan["matrix240"]))
            bad[0]["font_name"] = "Regular" if bad[0]["font_name"] != "Regular" else "Medium"
            with self.assertRaises(DataError): v3_matrix_audit(bad)
            bad = json.loads(json.dumps(plan["matrix240"]))
            bad[0]["text"] = bad[0]["text"].lower()
            with self.assertRaises(DataError): v3_matrix_audit(bad)
        def test_plain_E_and_insertions_are_false_positives_but_expected_controls_are_not(self):
            self.assertEqual(latin_control_falsepositives("Ev eve", "Êv êve")["plain_E_to_circumflex"], 2)
            self.assertEqual(latin_control_falsepositives("Ev", "Eêv")["inserted_circumflex"], 1)
            self.assertEqual(latin_control_falsepositives("E Ê e ê", "E Ê e ê")["unexpected_total"], 0)
            self.assertEqual(latin_control_falsepositives("Ê", "ê")["positive_control_case_errors"], 1)
            self.assertEqual(latin_control_falsepositives("a", "ê")["other_to_circumflex"], 1)
        def test_metric_caps_reject_without_large_allocation(self):
            with self.assertRaises(DataError): latin_control_falsepositives("E" * 8193, "")
            with self.assertRaises(DataError): latin_control_falsepositives("E" * 2000, "e" * 2000)
        def test_all_matrix_glyphs_have_pinned_font_and_model_coverage(self):
            chars = set("".join(r["text"] for r in plan["matrix240"]))
            alphabet = read_alphabet(pinned(Path(source["base_alphabet"]["path"]), CHARSET_SHA, 1024 * 1024))
            self.assertFalse(chars - alphabet)
            for meta in fonts.values(): self.assertEqual(font_covers(Path(meta["path"]).read_bytes(), chars), set())
        def test_matrix_decoded_pixels_are_unique(self):
            pixels = set()
            for row in plan["matrix240"]:
                image, _ = render_specialization(row, Path(fonts[row["font_name"]]["path"]))
                pixels.add(pixel_hash(image))
            self.assertEqual(len(pixels), 240)
        def test_E_positives_cannot_be_relabelled_as_Turkish(self):
            records = [{**r, "origin": "control_matrix"} for r in plan["matrix240"]]
            self.assertEqual(v3_role_guard(records)["positive_E_latin_rows"], 48)
            positive = next(r for r in records if r["matrix_family"] == "E_latin")
            positive["language_role"] = "turkish_control"
            with self.assertRaises(DataError): v3_role_guard(records)
        def test_copy_keeps_frozen_validation_bytes_and_refuses_existing_file(self):
            original = next(r for r in source["samples"] if r["split"] == "internal-val")
            with tempfile.TemporaryDirectory(prefix="v3-copy-", dir=args.evidence_root) as temp:
                root = Path(temp).resolve(); (root / "internal-val").mkdir()
                writer = Writer(root)
                copied = copy_pinned_sample(source_dir, original, writer, "retained_v2_internal_val")
                for suffix in original["files"]: self.assertEqual(copied["files"][suffix], original["files"][suffix])
                with self.assertRaises(FileExistsError): copy_pinned_sample(source_dir, original, writer, "retained_v2_internal_val")
    return unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(CounterbalancedFixtures)).wasSuccessful()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-root", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--font", type=Path, default=Path(__file__).resolve().parents[2] / "assets/fonts/Inter-Regular.ttf")
    parser.add_argument("--base-model", type=Path)
    parser.add_argument("--base-unicharset", type=Path)
    parser.add_argument("--mode", choices=("original-v1", "specialization-v2", "counterbalanced-v3"), default="original-v1")
    parser.add_argument("--seed", type=int)
    parser.add_argument("--train-lines", type=int)
    parser.add_argument("--val-lines", type=int)
    parser.add_argument("--v3-design", type=Path)
    parser.add_argument("--v3-design-sha256")
    parser.add_argument("--verify", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.seed is None:
        args.seed = SEED if args.mode == "original-v1" else V2_SEED if args.mode == "specialization-v2" else V3_SEED
    if args.train_lines is None:
        args.train_lines = 128 if args.mode == "original-v1" else 512
    if args.val_lines is None:
        args.val_lines = 32 if args.mode == "original-v1" else 128
    try:
        repo = Path(__file__).resolve().parents[2]
        if args.self_test:
            root = args.evidence_root.resolve(strict=True)
            if not root.is_dir() or root.is_relative_to(repo):
                raise DataError("self-test fixtures must stay in evidence outside the repository")
            passed = self_test(root, args.font) if args.mode == "original-v1" else specialization_self_test(root) if args.mode == "specialization-v2" else counterbalanced_self_test(args)
            return 0 if passed else 1
        if args.verify:
            result = verify_dataset(output_path(args.evidence_root, args.verify, repo, new=False))
        else:
            if any(x is None for x in [args.output, args.base_model, args.base_unicharset]):
                raise DataError("generation requires --output, --base-model and --base-unicharset")
            result = make_dataset(args) if args.mode == "original-v1" else make_specialization_dataset(args) if args.mode == "specialization-v2" else make_counterbalanced_dataset(args)
        print(json.dumps(result, ensure_ascii=False, sort_keys=True))
        return 0
    except (DataError, OSError, ValueError, KeyError, struct.error, Image.DecompressionBombError) as error:
        print(f"training-data error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
