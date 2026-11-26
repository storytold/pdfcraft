"""Build the COLR test font: DeckCraft Colr Squares (contributor-original, CC0-1.0).

Usage: python make_colr_font.py colr-squares.ttf  (needs fontTools)

U+E000: a 1000x1000 red square (palette 0) with a 400x400 blue square (palette 1) in its middle.
U+E001: a 1000x1000 square in the text colour (palette index 0xFFFF).
Plain outlines of the base glyphs are a single square, as colour fonts carry for renderers
without colour support.
"""
import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.colorLib.builder import buildCOLR, buildCPAL


def square(x0, y0, x1, y1):
    pen = TTGlyphPen(None)
    pen.moveTo((x0, y0))
    pen.lineTo((x0, y1))
    pen.lineTo((x1, y1))
    pen.lineTo((x1, y0))
    pen.closePath()
    return pen.glyph()


def empty():
    return TTGlyphPen(None).glyph()


order = [".notdef", "space", "redblue", "ink", "big", "small"]
fb = FontBuilder(1000, isTTF=True)
fb.setupGlyphOrder(order)
fb.setupCharacterMap({0x20: "space", 0xE000: "redblue", 0xE001: "ink"})
fb.setupGlyf({
    ".notdef": empty(),
    "space": empty(),
    "redblue": square(0, 0, 1000, 1000),
    "ink": square(0, 0, 1000, 1000),
    "big": square(0, 0, 1000, 1000),
    "small": square(300, 300, 700, 700),
})
fb.setupHorizontalMetrics({g: (1000, 0) for g in order})
fb.setupHorizontalHeader(ascent=1000, descent=0)
fb.setupNameTable({"familyName": "DeckCraft Colr Squares", "styleName": "Regular"})
fb.setupOS2(sTypoAscender=1000, sTypoDescender=0, usWinAscent=1000, usWinDescent=0)
fb.setupPost()
fb.font["COLR"] = buildCOLR({"redblue": [("big", 0), ("small", 1)], "ink": [("big", 0xFFFF)]}, version=0)
fb.font["CPAL"] = buildCPAL([[(1.0, 0.0, 0.0, 1.0), (0.0, 0.0, 1.0, 1.0)]])
fb.save(sys.argv[1])
print("saved", sys.argv[1])
