"""Build the legacy-name test font: DeckCraft Legacy Semibold (contributor-original, CC0-1.0).

Usage: python make_legacy_font.py legacy-semibold.ttf legacy-regular.ttf  (needs fontTools)

legacy-semibold.ttf: typographic family "DeckCraft Legacy", style "Semibold" (name IDs 16/17,
weight 600); Windows family "DeckCraft Legacy Semibold", style "Regular" (name IDs 1/2), as fonts
like Segoe UI Semibold are named. legacy-regular.ttf: the family's Regular ("DeckCraft Legacy",
"Regular", weight 400), named only by IDs 1/2 as Segoe UI's regular file is. Glyphs: .notdef and
a square for "A".
"""
import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen


def square():
    pen = TTGlyphPen(None)
    pen.moveTo((100, 0))
    pen.lineTo((100, 700))
    pen.lineTo((600, 700))
    pen.lineTo((600, 0))
    pen.closePath()
    return pen.glyph()


def build(path, names, weight):
    fb = FontBuilder(1000, isTTF=True)
    fb.setupGlyphOrder([".notdef", "A"])
    fb.setupCharacterMap({ord("A"): "A"})
    fb.setupGlyf({".notdef": TTGlyphPen(None).glyph(), "A": square()})
    fb.setupHorizontalMetrics({".notdef": (500, 0), "A": (700, 100)})
    fb.setupHorizontalHeader(ascent=800, descent=-200)
    fb.setupNameTable(names)
    fb.setupOS2(usWeightClass=weight, sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
    fb.setupPost()
    fb.save(path)


build(sys.argv[1], {
    "familyName": "DeckCraft Legacy Semibold",
    "styleName": "Regular",
    "typographicFamily": "DeckCraft Legacy",
    "typographicSubfamily": "Semibold",
    "uniqueFontIdentifier": "DeckCraft Legacy Semibold",
    "fullName": "DeckCraft Legacy Semibold",
    "psName": "DeckCraftLegacy-Semibold",
}, 600)
build(sys.argv[2], {
    "familyName": "DeckCraft Legacy",
    "styleName": "Regular",
    "uniqueFontIdentifier": "DeckCraft Legacy Regular",
    "fullName": "DeckCraft Legacy",
    "psName": "DeckCraftLegacy-Regular",
}, 400)
