"""Build the Inter subsets the crash-protection reconnect screen draws with.

Keeps Basic Latin, Latin-1 and common punctuation, drops hinting and
OpenType layout, and decomposes composite glyphs so the built-in
rasterizer (src/slate/font.rs) only ever sees simple quadratic outlines.

Usage:
    pip install fonttools
    python scripts/subset_slate_fonts.py path/to/Inter-4.1/extras/ttf

Inter is by Rasmus Andersson, SIL Open Font License 1.1
(assets/fonts/OFL.txt). Download: https://github.com/rsms/inter/releases
"""

import sys
from pathlib import Path

from fontTools import subset
from fontTools.pens.recordingPen import DecomposingRecordingPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

WEIGHTS = ("Regular", "SemiBold")
UNICODES = (
    list(range(0x20, 0x7F))
    + list(range(0xA0, 0x100))
    + [0x2013, 0x2014, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2026]
)
OUT_DIR = Path(__file__).resolve().parent.parent / "assets" / "fonts"


def subset_font(source: Path) -> TTFont:
    options = subset.Options()
    options.hinting = False
    options.layout_features = []
    options.name_IDs = [0, 1, 2, 3, 4, 5, 6, 13, 14]
    options.notdef_outline = True
    font = TTFont(source)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=UNICODES)
    subsetter.subset(font)
    return font


def decompose_composites(font: TTFont) -> None:
    glyph_set = font.getGlyphSet()
    glyf = font["glyf"]
    for name in font.getGlyphOrder():
        if not glyf[name].isComposite():
            continue
        recording = DecomposingRecordingPen(glyph_set)
        glyph_set[name].draw(recording)
        pen = TTGlyphPen(None)
        recording.replay(pen)
        glyf[name] = pen.glyph()
        glyf[name].recalcBounds(glyf)


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    source_dir = Path(sys.argv[1])
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for weight in WEIGHTS:
        font = subset_font(source_dir / f"Inter-{weight}.ttf")
        decompose_composites(font)
        target = OUT_DIR / f"Inter-{weight}-slate.ttf"
        font.save(target)
        print(f"{target.name}: {target.stat().st_size} bytes")


if __name__ == "__main__":
    main()
