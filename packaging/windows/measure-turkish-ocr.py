#!/usr/bin/env python3
"""External synthetic OCR measurement, not an application/runtime dependency.

Requires Pillow, reportlab and PyMuPDF in an external oracle environment, plus
Poppler's pdftoppm on PATH. Generated fixtures and reports must stay outside the
repository. PyMuPDF is used only by this external verification process.

Example (with OCR models/Tesseract configured in the caller's environment):
    python packaging/windows/measure-turkish-ocr.py --cli /path/pdfcraft-cli.exe \
        --output /path/private-evidence/ocr-run

The output directory must be new. Re-run against a different CLI with a new
output directory; sample definitions, random seed and image recipes are fixed.
Original reference prose and generated fixture pixels are MIT OR Apache-2.0.
No reference text is silently corrected after recognition.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import random
import shutil
import subprocess
import sys
import time
import unicodedata

from PIL import Image, ImageDraw, ImageFilter, ImageFont, __version__ as pillow_version
import pymupdf
import reportlab
from reportlab.pdfgen.canvas import Canvas


INTER_SHA256 = "40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82"
TEXTS = [
    {
        "id": "gundelik",
        "language_mix": "tr",
        "anchor": "Çağrı",
        "text": (
            "Çağrı bugün Şişli'deki küçük kütüphaneye uğradı. Öğrenciler, yağmur "
            "dinince bahçede yürüyüş yapacak. Çocuklar üzüm, şeftali ve böğürtlen "
            "toplarken öğretmen ışığı kapattı. Güvenli bir belge için önce doğru "
            "dosyayı seçin, ardından kaydedip yeniden açın. Iğdır ile İstanbul "
            "arasında düzenli görüşmeler sürüyor; hiçbir başvuru gözden kaçmıyor."
        ),
    },
    {
        "id": "sayilar",
        "language_mix": "tr",
        "anchor": "Kayıt",
        "text": (
            "Kayıt tarihi: 08.11.2026. Sipariş A-27 için toplam 1.234,50 TL, "
            "indirim %18 ve teslim süresi 3 iş günüdür. Ölçüler 12,5 x 24 cm; "
            "kutu sayısı 48, sıra numarası 007'dir. İlgili kişi Ayşe, saat 09:30'da "
            "yeniden arayacak. Soru: ödeme alındı mı? Yanıt: evet! Belge no: "
            "TR/2026-114; açıklama (ön ödeme) ve son tutar ayrıca doğrulanır."
        ),
    },
    {
        "id": "harfler",
        "language_mix": "tr",
        "anchor": "Iğdır",
        "text": (
            "Iğdır, İzmir, Çanakkale, Şırnak ve Gümüşhane aynı çizelgede yer alıyor. "
            "IŞIK ve İŞARET başlıkları doğru yazılmalıdır. Küçük harfler: ç ğ ı i ö ş ü; "
            "büyük harfler: Ç Ğ I İ Ö Ş Ü. Kâğıt, hikâye, millî ve sükûn sözcükleri "
            "dikkatle okunur. Â, Î ve Û işaretleri de denemeye dahildir. Bu belge "
            "herhangi bir kişisel bilgi içermez; bütün cümleler özgün test metnidir."
        ),
    },
    {
        "id": "karma",
        "language_mix": "tr-en",
        "anchor": "Türkçe",
        "text": (
            "Türkçe ve English birlikte: Please review the attached invoice before "
            "Friday. Çağrı will send the updated report at 14:45. Dosya adı "
            "Project-2026, durum ready ve toplam 250,75 EUR olarak kaydedildi. "
            "Check the page count, search for İstanbul, then save and reopen the "
            "document. Öğrenci yorumları için not: quality, ölçüm, güven ve erişim."
        ),
    },
]
CONDITIONS = ("clean300", "skew300", "low90")
SIZE = (2480, 1150)
SOURCE_DPI = 300


def normalized(text: str) -> str:
    """Canonical-equivalent text and collapsed whitespace; preserve case/punctuation."""
    return " ".join(unicodedata.normalize("NFC", text).split())


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def edit_counts(expected: str, actual: str) -> dict:
    """Unit-cost Levenshtein, including spaces and punctuation, with a traceback."""
    rows = [list(range(len(actual) + 1))]
    for i, a in enumerate(expected, 1):
        row = [i]
        for j, b in enumerate(actual, 1):
            row.append(min(rows[-1][j] + 1, row[-1] + 1, rows[-1][j - 1] + (a != b)))
        rows.append(row)
    i, j = len(expected), len(actual)
    edits = Counter()
    differences = []
    while i or j:
        if i and j and rows[i][j] == rows[i - 1][j - 1] + (expected[i - 1] != actual[j - 1]):
            if expected[i - 1] != actual[j - 1]:
                edits["substitutions"] += 1
                differences.append({"offset": i - 1, "expected": expected[i - 1], "actual": actual[j - 1]})
            i, j = i - 1, j - 1
        elif i and rows[i][j] == rows[i - 1][j] + 1:
            edits["deletions"] += 1
            differences.append({"offset": i - 1, "expected": expected[i - 1], "actual": ""})
            i -= 1
        else:
            edits["insertions"] += 1
            differences.append({"offset": i, "expected": "", "actual": actual[j - 1]})
            j -= 1
    count = rows[-1][-1]
    return {
        "edits": count,
        "reference_characters": len(expected),
        "cer": count / len(expected) if expected else None,
        **{kind: edits[kind] for kind in ("substitutions", "deletions", "insertions")},
        "differences": list(reversed(differences)),
    }


def make_scan(text: str, font: ImageFont.FreeTypeFont, condition: str) -> Image.Image:
    image = Image.new("RGB", SIZE, "white")
    draw = ImageDraw.Draw(image)
    lines, current = [], ""
    for word in text.split():
        candidate = f"{current} {word}".strip()
        if draw.textlength(candidate, font=font) > SIZE[0] - 360:
            lines.append(current)
            current = word
        else:
            current = candidate
    lines.append(current)
    if 160 + len(lines) * 78 > SIZE[1] - 120:
        raise ValueError("The fixed fixture layout would clip the reference prose")
    for index, line in enumerate(lines):
        draw.text((180, 150 + index * 78), line, font=font, fill="black")
    if condition == "skew300":
        return image.rotate(3.0, resample=Image.Resampling.BICUBIC, expand=False, fillcolor="white")
    if condition == "low90":
        # Genuine lower native resolution, not a 300-DPI image labelled as 90 DPI.
        image = image.resize(tuple(round(n * 90 / 300) for n in SIZE), Image.Resampling.LANCZOS).convert("L")
        image = image.filter(ImageFilter.GaussianBlur(0.55))
        image = image.point(lambda value: round(60 + value * 0.70))
        noise = random.Random(20261108)
        pixels = image.load()
        for _ in range(round(image.width * image.height * 0.003)):
            pixels[noise.randrange(image.width), noise.randrange(image.height)] = noise.choice((70, 235))
        buffer = io.BytesIO()
        image.save(buffer, format="JPEG", quality=40)
        buffer.seek(0)
        return Image.open(buffer).convert("RGB")
    return image


def run_cli(cli: Path, folder: Path, name: str, steps: list) -> dict:
    script = folder / f"{name}.script.json"
    write_json(script, steps)
    args = [str(cli), "run", "--script", str(script), "--root", str(folder), "--report"]
    start = time.monotonic()
    result = subprocess.run(args, capture_output=True, timeout=180)
    (folder / f"{name}.stdout.json").write_bytes(result.stdout)
    (folder / f"{name}.stderr.txt").write_bytes(result.stderr)
    try:
        report = json.loads(result.stdout.decode("utf-8-sig"))
    except (ValueError, UnicodeError) as error:
        raise RuntimeError(f"CLI returned no valid JSON report: {script}") from error
    write_json(folder / f"{name}.invocation.json", {"args": args, "exit_code": result.returncode, "seconds": time.monotonic() - start})
    if result.returncode or not report.get("ok"):
        raise RuntimeError(f"CLI failed; inspect {folder / (name + '.stdout.json')}")
    return report


def result_of(report: dict, tool: str, index: int = 0) -> dict:
    matches = [step["result"][0] for step in report["steps"] if step["tool"] == tool]
    return matches[index]


def page_text(result: dict) -> str:
    return "\n".join(page["text"] for page in result["pages"])


def inspect_saved(source: Path, saved: Path, recognized: str, query: str) -> tuple[dict, str]:
    checks = {"reader": f"PyMuPDF {pymupdf.__version__}"}
    with pymupdf.open(source) as before, pymupdf.open(saved) as after:
        checks["source_image_only"] = not before[0].get_text().strip()
        checks["one_page_preserved"] = len(before) == len(after) == 1
        saved_text = after[0].get_text()
        checks["saved_text_equals_recognition"] = normalized(saved_text) == normalized(recognized)
        checks["recognized_word_searchable"] = bool(query) and bool(after[0].search_for(query))
        for dpi in (96, 300):
            matrix = pymupdf.Matrix(dpi / 72, dpi / 72)
            a, b = (doc[0].get_pixmap(matrix=matrix, alpha=False) for doc in (before, after))
            checks[f"pixels_unchanged_{dpi}dpi"] = a.width == b.width and a.height == b.height and a.samples == b.samples
            checks[f"source_pixels_sha256_{dpi}dpi"] = hashlib.sha256(a.samples).hexdigest()
            checks[f"saved_pixels_sha256_{dpi}dpi"] = hashlib.sha256(b.samples).hexdigest()
    return checks, saved_text


def measure_sample(cli: Path, folder: Path, entry: dict, condition: str, font: ImageFont.FreeTypeFont, poppler: str) -> dict:
    folder.mkdir()
    scan = make_scan(entry["text"], font, condition)
    scan.save(folder / "scan.png")
    source, saved = folder / "source.pdf", folder / "searchable.pdf"
    width, height = (n * 72 / SOURCE_DPI for n in SIZE)
    canvas = Canvas(str(source), pagesize=(width, height), invariant=1)
    canvas.setAuthor("PdfCraft synthetic OCR measurement")
    canvas.setTitle(f"Original Turkish OCR test: {entry['id']}, {condition}")
    canvas.drawImage(str(folder / "scan.png"), 0, 0, width=width, height=height)
    canvas.showPage()
    canvas.save()
    subprocess.run([poppler, "-f", "1", "-singlefile", "-scale-to", "1200", "-png", str(source), str(folder / "source-poppler")], check=True, capture_output=True, timeout=30)
    ocr = {"doc": 1, "language": "tr", "engine": "auto", "dpi": 300, "output_mode": "searchable", "deskew": False, "denoise": False, "binarize": False, "auto_rotate": False}
    steps = [
        {"tool": "ocr_status", "args": {}},
        {"tool": "doc_open", "args": {"path": source.name}},
        {"tool": "text_extract", "args": {"doc": 1}},
    ]
    # One extra read per run makes automatic engine provenance observable.
    if entry["id"] == TEXTS[0]["id"] and condition == "clean300":
        steps.append({"tool": "ocr_words", "args": {key: value for key, value in ocr.items() if key != "output_mode"}})
    steps.extend([
        {"tool": "ocr_recognize", "args": ocr},
        {"tool": "text_extract", "args": {"doc": 1}},
        {"tool": "doc_save", "args": {"doc": 1, "path": saved.name}},
        {"tool": "page_render", "args": {"doc": 1, "page": 1, "dpi": 96}, "out": "saved-cli.png"},
        {"tool": "doc_close", "args": {"doc": 1}},
        {"tool": "doc_open", "args": {"path": saved.name}},
        {"tool": "text_extract", "args": {"doc": 2}},
    ])
    report = run_cli(cli, folder, "recognize", steps)
    recognized = page_text(result_of(report, "ocr_recognize"))
    expected, actual = normalized(entry["text"]), normalized(recognized)
    query = next((word.strip(".,;:!?()'") for word in actual.split() if len(word.strip(".,;:!?()'")) > 2), "")
    search_report = run_cli(cli, folder, "reopen-search", [
        {"tool": "doc_open", "args": {"path": saved.name}},
        {"tool": "text_find", "args": {"doc": 1, "query": query or entry["anchor"]}},
        {"tool": "text_find", "args": {"doc": 1, "query": entry["anchor"]}},
    ])
    checks, independent_text = inspect_saved(source, saved, recognized, query)
    native_before = page_text(result_of(report, "text_extract", 1))
    native_after = page_text(result_of(report, "text_extract", 2))
    checks.update({
        "source_cli_image_only": not page_text(result_of(report, "text_extract", 0)).strip(),
        "cli_extract_equals_recognition": normalized(native_before) == actual,
        "cli_reopened_extract_equals_recognition": normalized(native_after) == actual,
        "cli_save_reopen_preserves_native_extract": normalized(native_before) == normalized(native_after),
        "cli_recognized_word_searchable": result_of(search_report, "text_find", 0)["count"] > 0,
    })
    verification_keys = [key for key, value in checks.items() if isinstance(value, bool)]
    result = {
        "id": f"{entry['id']}-{condition}", "condition": condition, "language_mix": entry["language_mix"],
        "native_dpi": 90 if condition == "low90" else 300, "scan_pixels": list(scan.size),
        "ocr_settings": ocr, "expected": entry["text"], "recognized": recognized,
        "normalized_expected": expected, "normalized_recognized": actual, **edit_counts(expected, actual),
        "text_stages": {
            "ocr_recognition": recognized,
            "native_before_save": native_before,
            "native_after_reopen": native_after,
            "independent_saved_extract": independent_text,
        },
        "stage_metrics": {
            "reference_to_ocr": edit_counts(expected, actual),
            "reference_to_native_before_save": edit_counts(expected, normalized(native_before)),
            "reference_to_native_after_reopen": edit_counts(expected, normalized(native_after)),
            "reference_to_independent": edit_counts(expected, normalized(independent_text)),
            "ocr_to_native_before_save": edit_counts(actual, normalized(native_before)),
            "native_before_to_after_save": edit_counts(normalized(native_before), normalized(native_after)),
            "ocr_to_independent": edit_counts(actual, normalized(independent_text)),
        },
        "recognized_words": result_of(report, "ocr_recognize")["words"],
        "expected_anchor": entry["anchor"], "expected_anchor_found": result_of(search_report, "text_find", 1)["count"] > 0,
        "roundtrip_search_query": query, "checks": checks,
        "persistence_and_pixels_pass": all(checks[key] for key in verification_keys),
        "source_sha256": sha256(source), "saved_sha256": sha256(saved),
    }
    if any(step["tool"] == "ocr_words" for step in report["steps"]):
        words = result_of(report, "ocr_words")
        result["automatic_reader_sources"] = sorted({word["source"] for page in words["pages"] for word in page["words"]})
    write_json(folder / "measurement.json", result)
    print(f"{result['id']}: CER {result['edits']}/{result['reference_characters']} = {result['cer']:.4%}; roundtrip {result['persistence_and_pixels_pass']}", flush=True)
    return result


def aggregate(samples: list) -> dict:
    edits = sum(sample["edits"] for sample in samples)
    characters = sum(sample["reference_characters"] for sample in samples)
    return {"samples": len(samples), "edits": edits, "reference_characters": characters, "cer": edits / characters,
            "persistence_and_pixels_pass": all(sample["persistence_and_pixels_pass"] for sample in samples)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--font", type=Path, default=Path(__file__).resolve().parents[2] / "assets/fonts/Inter-Regular.ttf")
    parser.add_argument("--pdftoppm", default=shutil.which("pdftoppm"))
    args = parser.parse_args()
    cli, output, font_path = args.cli.resolve(), args.output.resolve(), args.font.resolve()
    repository = Path(__file__).resolve().parents[2]
    if output.is_relative_to(repository):
        parser.error("--output must be outside the source repository; generated assets are private test evidence")
    if output.exists():
        parser.error("--output must be a new directory; existing evidence is never overwritten")
    if not cli.is_file() or not args.pdftoppm:
        parser.error("the CLI and Poppler pdftoppm must exist")
    if sha256(font_path) != INTER_SHA256:
        parser.error("--font must be the repository's attributed Inter Regular font")
    output.mkdir(parents=True)
    font = ImageFont.truetype(str(font_path), round(12 * SOURCE_DPI / 72))
    manifest = {
        "schema": 1, "corpus": "turkish-original-12-v1", "reference_licence": "MIT OR Apache-2.0",
        "scope": "Synthetic single-paragraph scans; not real scanners, handwriting, multi-column layouts or documents in the wild",
        "normalization": "Unicode NFC and collapse all whitespace to one ASCII space; no case-folding, spelling/punctuation correction or character deletion",
        "metric": "Micro-averaged unit-cost Levenshtein on normalized Unicode code points, including spaces; edits/reference characters",
        "clean_target": 0.01, "page_points": [n * 72 / SOURCE_DPI for n in SIZE], "font_points": 12,
        "recipes": {"clean300": "black Inter Regular on white, 300 native DPI", "skew300": "same raster rotated +3 degrees bicubic, no deskew", "low90": "90 native DPI, Gaussian blur radius 0.55 px, levels 60..238.5, 0.3% seeded speckle, JPEG quality 40; embedded losslessly after JPEG decode"},
        "font": {"path": str(font_path), "sha256": sha256(font_path), "title": "Inter Regular", "author": "The Inter Project Authors (Rasmus Andersson)", "source": "https://github.com/rsms/inter", "licence": "OFL-1.1", "licence_file": str(font_path.parent / "OFL-Inter.txt")},
        "cli": {"path": str(cli), "sha256": sha256(cli)}, "texts": TEXTS,
        "driver": {"path": str(Path(__file__).resolve()), "sha256": sha256(Path(__file__))},
        "environment": {name: os.environ.get(name) for name in ("PDFCRAFT_TESSERACT", "TESSDATA_PREFIX", "PDFCRAFT_MODELS", "PDFCRAFT_SYSTEM_FONTS")},
        "tools": {"python": sys.version, "platform": platform.platform(), "pillow": pillow_version, "reportlab": reportlab.Version, "pymupdf": pymupdf.__version__, "pdftoppm": args.pdftoppm},
    }
    model_dir = Path(manifest["environment"]["TESSDATA_PREFIX"] or ".")
    manifest["models"] = [{"file": str(path), "sha256": sha256(path)} for path in sorted(model_dir.glob("*.traineddata"))]
    write_json(output / "manifest.json", manifest)
    samples = []
    for entry in TEXTS:
        for condition in CONDITIONS:
            samples.append(measure_sample(cli, output / f"{entry['id']}-{condition}", entry, condition, font, args.pdftoppm))
    groups = {condition: aggregate([sample for sample in samples if sample["condition"] == condition]) for condition in CONDITIONS}
    groups["clean300_tr_only"] = aggregate([sample for sample in samples if sample["condition"] == "clean300" and sample["language_mix"] == "tr"])
    groups["clean300_mixed"] = aggregate([sample for sample in samples if sample["condition"] == "clean300" and sample["language_mix"] == "tr-en"])
    summary = {"corpus": manifest["corpus"], "cli_sha256": manifest["cli"]["sha256"], "groups": groups, "samples": samples,
               "clean_target_pass": groups["clean300"]["cer"] <= 0.01,
               "all_persistence_and_pixels_pass": all(sample["persistence_and_pixels_pass"] for sample in samples)}
    summary["validation_counts"] = {
        key: sum(sample["checks"][key] for sample in samples)
        for key, value in samples[0]["checks"].items() if isinstance(value, bool)
    }
    write_json(output / "summary.json", summary)
    lines = ["# Türkçe OCR sentetik ölçüm kümesi", "", "12 örnek: dört özgün paragraf ve her biri için üç görüntü koşulu. Tek paragraf, Inter Regular 12 pt; çok sütunlu veya gerçek tarayıcı belgelerini temsil etmez.", "", "CER: Unicode NFC + boşluk birleştirme sonrası, büyük/küçük harf ve noktalama korunarak Levenshtein düzenleme sayısı / referans karakter sayısı. Sayılar mikro toplamdır. Hiçbir OCR metni düzeltilmedi.", "", "| Grup | Örnek | Düzenleme / karakter | CER |", "|---|---:|---:|---:|"]
    for name, group in groups.items():
        lines.append(f"| {name} | {group['samples']} | {group['edits']} / {group['reference_characters']} | {group['cer']:.3%} |")
    lines.extend(["", "| Örnek | Düzenleme / karakter | CER | Çıkarım/arama/piksel |", "|---|---:|---:|---|"])
    for sample in samples:
        lines.append(f"| {sample['id']} | {sample['edits']} / {sample['reference_characters']} | {sample['cer']:.3%} | {'GEÇTİ' if sample['persistence_and_pixels_pass'] else 'GEÇMEDİ'} |")
    lines.extend(["", "| Doğrulama | Geçen örnek / toplam |", "|---|---:|"])
    for check, count in summary["validation_counts"].items():
        lines.append(f"| {check} | {count} / {len(samples)} |")
    lines.extend(["", "Dört metin aşaması ve her aşamanın CER/diff sayısı örneklerin measurement.json içindeki text_stages ve stage_metrics alanlarında ayrı tutulur. Kaydetme/yeniden açma ile tanımadan sonra metin okuma sırası aynı kontrol değildir."])
    lines.extend(["", f"Temiz kümede CER ≤ %1 hedefi: {'GEÇTİ' if summary['clean_target_pass'] else 'GEÇMEDİ'}.", f"12 örnekte kaydet/yeniden aç/çıkar/ara ve bağımsız 96/300 DPI piksel koruma: {'GEÇTİ' if summary['all_persistence_and_pixels_pass'] else 'GEÇMEDİ'}.", "", "Düşük kalite 90 DPI kaynak, bulanıklık ve JPEG bozulumuyla sabitlenmiştir. Eğik örneklerde düzeltme seçeneği kapalıdır. Karma örnek de yalnız Türkçe model ve tr/auto isteği ile okunur; ikili dil tanıma iddiası değildir.", "", "Bütün ayrıntılar summary.json; ham CLI betik/raporları ve kaynak/çıktı PDF'leri örnek klasörlerindedir. Font lisansı ve hash'i manifest.json dosyasındadır. Bu sonuç sürüm kapısı veya genel OCR doğruluk garantisi değildir.", ""])
    (output / "report.md").write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps({"groups": groups, "clean_target_pass": summary["clean_target_pass"], "all_persistence_and_pixels_pass": summary["all_persistence_and_pixels_pass"]}, ensure_ascii=False, indent=2), flush=True)
    return 0 if summary["all_persistence_and_pixels_pass"] and summary["clean_target_pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
