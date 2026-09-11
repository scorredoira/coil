#!/usr/bin/env python3
"""Draws a terminal screen captured by `tmux capture-pane -p -e` as a PNG.

Usage: render.py <capture.ansi> <out.png>

Only the SGR sequences tmux writes are understood: 24-bit, 256 and the 16 basic
colours, bold, italic, underline and reverse. Block and box-drawing characters are
drawn as shapes rather than glyphs, so rows join without gaps the way a terminal
shows them.
"""

import re
import sys

from PIL import Image, ImageDraw, ImageFont

FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
FONT_BOLD = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf"
FONT_ITALIC = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf"
SIZE = 26
LINE = 1.3
PAD = 24

DEFAULT_FG = (201, 209, 217)
DEFAULT_BG = (13, 17, 23)
BASIC = [
    (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16),
    (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
    (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67),
    (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
]

SGR = re.compile(r"\x1b\[([0-9;:]*)m")


def xterm_256(n):
    if n < 16:
        return BASIC[n]
    if n < 232:
        n -= 16
        steps = [0, 95, 135, 175, 215, 255]
        return (steps[n // 36], steps[(n // 6) % 6], steps[n % 6])
    level = 8 + (n - 232) * 10
    return (level, level, level)


class Style:
    def __init__(self):
        self.reset()

    def reset(self):
        self.fg = None
        self.bg = None
        self.bold = False
        self.italic = False
        self.underline = False
        self.reverse = False

    def copy(self):
        other = Style()
        other.__dict__.update(self.__dict__)
        return other

    def apply(self, params):
        codes = [int(p) if p else 0 for p in re.split("[;:]", params)] if params else [0]
        i = 0
        while i < len(codes):
            code = codes[i]
            if code == 0:
                self.reset()
            elif code == 1:
                self.bold = True
            elif code == 3:
                self.italic = True
            elif code == 4:
                self.underline = True
            elif code == 7:
                self.reverse = True
            elif code == 22:
                self.bold = False
            elif code == 23:
                self.italic = False
            elif code == 24:
                self.underline = False
            elif code == 27:
                self.reverse = False
            elif code in (38, 48, 58):
                if codes[i + 1] == 2:
                    colour = tuple(codes[i + 2:i + 5])
                    i += 4
                else:
                    colour = xterm_256(codes[i + 2])
                    i += 2
                if code == 38:
                    self.fg = colour
                elif code == 48:
                    self.bg = colour
            elif code == 39:
                self.fg = None
            elif code == 49:
                self.bg = None
            elif 30 <= code <= 37:
                self.fg = BASIC[code - 30]
            elif 40 <= code <= 47:
                self.bg = BASIC[code - 40]
            elif 90 <= code <= 97:
                self.fg = BASIC[code - 90 + 8]
            elif 100 <= code <= 107:
                self.bg = BASIC[code - 100 + 8]
            i += 1

    def colours(self):
        fg = self.fg or DEFAULT_FG
        bg = self.bg or DEFAULT_BG
        if self.reverse:
            return bg, fg
        return fg, bg


def parse(text):
    """The screen as rows of (character, style) cells."""
    rows = []
    style = Style()
    for line in text.split("\n"):
        cells = []
        position = 0
        for match in SGR.finditer(line):
            for char in line[position:match.start()]:
                cells.append((char, style.copy()))
            style.apply(match.group(1))
            position = match.end()
        for char in line[position:]:
            cells.append((char, style.copy()))
        rows.append(cells)
    while rows and not rows[-1]:
        rows.pop()
    return rows


def draw_shape(draw, char, x, y, width, height, fg):
    """Draws the block and box characters that must touch their neighbours; returns
    whether the character was one of them."""
    middle_x = x + width // 2
    middle_y = y + height // 2
    thin = max(2, width // 8)
    if char == "▄":
        draw.rectangle([x, middle_y, x + width - 1, y + height - 1], fill=fg)
    elif char == "▀":
        draw.rectangle([x, y, x + width - 1, middle_y - 1], fill=fg)
    elif char == "█":
        draw.rectangle([x, y, x + width - 1, y + height - 1], fill=fg)
    elif char == "│":
        draw.rectangle([middle_x - thin // 2, y, middle_x + thin // 2, y + height - 1], fill=fg)
    elif char == "─":
        draw.rectangle([x, middle_y - thin // 2, x + width - 1, middle_y + thin // 2], fill=fg)
    else:
        return False
    return True


def render(rows, out):
    regular = ImageFont.truetype(FONT, SIZE)
    fonts = {
        (False, False): regular,
        (True, False): ImageFont.truetype(FONT_BOLD, SIZE),
        (False, True): ImageFont.truetype(FONT_ITALIC, SIZE),
        (True, True): ImageFont.truetype(FONT_BOLD, SIZE),
    }
    width = round(regular.getlength("M"))
    height = round(SIZE * LINE)
    columns = max(len(row) for row in rows)
    image = Image.new(
        "RGB",
        (columns * width + 2 * PAD, len(rows) * height + 2 * PAD),
        DEFAULT_BG,
    )
    draw = ImageDraw.Draw(image)
    ascent, _ = regular.getmetrics()
    baseline = (height - SIZE) // 2
    for row_index, row in enumerate(rows):
        y = PAD + row_index * height
        for column, (char, style) in enumerate(row):
            x = PAD + column * width
            fg, bg = style.colours()
            draw.rectangle([x, y, x + width - 1, y + height - 1], fill=bg)
            if char == " " or draw_shape(draw, char, x, y, width, height, fg):
                continue
            font = fonts[(style.bold, style.italic)]
            draw.text((x, y + baseline), char, font=font, fill=fg)
            if style.underline:
                underline_y = y + baseline + ascent + 2
                draw.line([x, underline_y, x + width - 1, underline_y], fill=fg, width=2)
    image.save(out, optimize=True)


def main():
    if len(sys.argv) != 3:
        sys.exit("usage: render.py <capture.ansi> <out.png>")
    with open(sys.argv[1], encoding="utf-8") as capture:
        rows = parse(capture.read())
    if not rows:
        sys.exit(f"{sys.argv[1]}: the capture is empty")
    render(rows, sys.argv[2])


if __name__ == "__main__":
    main()
