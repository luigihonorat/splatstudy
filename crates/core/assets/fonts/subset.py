# SPDX-License-Identifier: GPL-3.0-or-later WITH AdditionRef-SplatStudy-app-store-exception
"""Cut the embedded UI fonts down to the characters SplatStudy shows.

Usage: python subset.py <folder with the original Google Fonts files>

Needs fontTools (pip install fonttools). Reads every string literal in
src/ui/i18n.rs so new Japanese text is always covered, then writes:
  Barlow-Light.ttf, Barlow-Regular.ttf   Latin (en, fr) and punctuation
  NotoSansJP-UI.ttf                      kana, CJK punctuation, used kanji;
                                         variable, wght axis limited to 300-500
"""

import re
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

HERE = Path(__file__).resolve().parent
I18N = HERE.parent.parent / "src" / "ui" / "i18n.rs"

LATIN = [
    (0x0020, 0x007E),  # ASCII
    (0x00A0, 0x017F),  # Latin-1 supplement, Latin Extended-A (French)
    (0x2010, 0x2027),  # dashes, quotes, ellipsis
    (0x2030, 0x203A),  # per mille, guillemets
    (0x20AC, 0x20AC),  # euro
    (0x2190, 0x2193),  # arrows
    (0x2212, 0x2212),  # minus
]
JAPANESE = [
    (0x3000, 0x303F),  # CJK symbols and punctuation
    (0x3041, 0x309F),  # hiragana
    (0x30A0, 0x30FF),  # katakana
    (0xFF01, 0xFF5E),  # full-width forms
    (0x2460, 0x2473),  # circled numbers
    (0x25A0, 0x25CF),  # shapes
]


def codepoints(ranges):
    return {c for lo, hi in ranges for c in range(lo, hi + 1)}


def ui_text():
    source = I18N.read_text(encoding="utf-8")
    return "".join(re.findall(r'"((?:[^"\\]|\\.)*)"', source))


def cut(src, dst, unicodes, limit_wght=None):
    font = TTFont(src)
    options = subset.Options()
    options.layout_features = ["kern", "liga", "palt", "vert"]
    options.name_IDs = ["*"]
    options.notdef_outline = True
    options.drop_tables += ["DSIG"]
    sub = subset.Subsetter(options)
    sub.populate(unicodes=sorted(unicodes))
    sub.subset(font)
    # Limit the weight axis after subsetting: instancing the full font first
    # trips a fontTools bug on Noto Sans JP's gvar table.
    if limit_wght is not None:
        font = instancer.instantiateVariableFont(font, {"wght": limit_wght})
    font.save(dst)
    print(f"{dst.name}: {dst.stat().st_size // 1024} KiB, {len(unicodes)} code points")


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    src = Path(sys.argv[1])
    used = {ord(c) for c in ui_text()}
    latin = codepoints(LATIN)
    japanese = codepoints(JAPANESE) | {c for c in used if c >= 0x2E80}
    cut(src / "Barlow-Light.ttf", HERE / "Barlow-Light.ttf", latin)
    cut(src / "Barlow-Regular.ttf", HERE / "Barlow-Regular.ttf", latin)
    cut(src / "NotoSansJP[wght].ttf", HERE / "NotoSansJP-UI.ttf", japanese | latin, (300, 500))


if __name__ == "__main__":
    main()
