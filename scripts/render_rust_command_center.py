"""Render real Ratatui TestBackend cells exported by the command center tests.

Example (run color and monochrome in separate processes):
  env -u NO_COLOR AGENTURM_UI_CAPTURE_DIR=/tmp/agenturm-ui-frames cargo test -p agenturm-ui command_center_pinned_frames
  NO_COLOR=1 AGENTURM_UI_CAPTURE_DIR=/tmp/agenturm-ui-frames cargo test -p agenturm-ui command_center_pinned_frames
  python3 scripts/render_rust_command_center.py /tmp/agenturm-ui-frames

Requires Pillow. Generates PNGs and a self-contained, accessible HTML gallery;
these are controlled metadata fixtures, not captures of a user's private data.
"""
from __future__ import annotations

import argparse
import base64
import html
import json
from pathlib import Path
import unicodedata

from PIL import Image, ImageDraw, ImageFont

ANSI = {
    "Black": "#000000", "Red": "#aa0000", "Green": "#00aa00", "Yellow": "#aa5500",
    "Blue": "#0000aa", "Magenta": "#aa00aa", "Cyan": "#00aaaa", "Gray": "#aaaaaa",
    "DarkGray": "#555555", "LightRed": "#ff5555", "LightGreen": "#55ff55", "LightYellow": "#ffff55",
    "LightBlue": "#5555ff", "LightMagenta": "#ff55ff", "LightCyan": "#55ffff", "White": "#ffffff",
}


def color(value, default):
    if value is None:
        return default
    if isinstance(value, list):
        return tuple(value)
    if isinstance(value, dict):
        index = value["indexed"]
        if index < 16:
            return list(ANSI.values())[index]
        if index < 232:
            index -= 16
            levels = [0, 95, 135, 175, 215, 255]
            return (levels[index // 36], levels[index // 6 % 6], levels[index % 6])
        gray = 8 + 10 * (index - 232)
        return (gray, gray, gray)
    return ANSI[value]


def symbol_width(symbol):
    if "\u200d" in symbol:
        return 2
    return max(1, sum(0 if unicodedata.combining(char) or char in "\ufe0e\ufe0f" else 2 if unicodedata.east_asian_width(char) in ("W", "F") else 1 for char in symbol))


def render(path: Path, output: Path, font_path: Path, size: int):
    frame = json.loads(path.read_text())
    regular = ImageFont.truetype(str(font_path), size)
    bold_path = font_path.with_name(font_path.stem + "-Bold" + font_path.suffix)
    bold = ImageFont.truetype(str(bold_path if bold_path.exists() else font_path), size)
    cjk_path = Path("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc")
    cjk = ImageFont.truetype(str(cjk_path), size) if cjk_path.exists() else regular
    emoji_path = Path("/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf")
    emoji = ImageFont.truetype(str(emoji_path), 109) if emoji_path.exists() else None
    cell_width = round(regular.getlength("M"))
    cell_height = size + 6
    margin = 14
    width, height = frame["width"], frame["height"]
    default_bg, default_fg = "#101014", "#efefef"
    image = Image.new("RGB", (width * cell_width + 2 * margin, height * cell_height + 2 * margin), default_bg)
    draw = ImageDraw.Draw(image)
    continuation = 0
    previous_style = None
    for index, cell in enumerate(frame["cells"]):
        x, y = index % width * cell_width + margin, index // width * cell_height + margin
        if index % width == 0:
            continuation = 0
        # Ratatui resets the placeholder cells covered by a wide glyph. A real
        # terminal keeps the leading cell's style across that glyph's width.
        if continuation:
            style = previous_style
            continuation -= 1
        else:
            style = cell
            previous_style = cell
            continuation = symbol_width(cell["symbol"]) - 1
        fg, bg = color(style["fg"], default_fg), color(style["bg"], default_bg)
        if style["reverse"]:
            fg, bg = bg, fg
        draw.rectangle((x, y, x + cell_width - 1, y + cell_height - 1), fill=bg)
    # Draw glyphs after all backgrounds so wide glyphs survive their continuation cell.
    for index, cell in enumerate(frame["cells"]):
        symbol = cell["symbol"]
        if not symbol.strip():
            continue
        x, y = index % width * cell_width + margin, index // width * cell_height + margin
        fg, bg = color(cell["fg"], default_fg), color(cell["bg"], default_bg)
        if cell["reverse"]:
            fg, bg = bg, fg
        wide = symbol_width(symbol) > 1
        if emoji and any(ord(char) >= 0x1F000 for char in symbol):
            glyph = Image.new("RGBA", (160, 140))
            ImageDraw.Draw(glyph).text((0, 0), symbol, font=emoji, embedded_color=True)
            glyph = glyph.crop(glyph.getbbox())
            glyph.thumbnail((cell_width * 2, cell_height - 2), Image.Resampling.LANCZOS)
            image.paste(glyph, (x, y + (cell_height - glyph.height) // 2), glyph)
        else:
            font = cjk if wide else bold if cell["bold"] else regular
            draw.text((x, y + 1), symbol, font=font, fill=fg)
        if cell["underline"]:
            draw.line((x, y + cell_height - 3, x + cell_width - 1, y + cell_height - 3), fill=fg)
    destination = output / (path.stem + ".png")
    image.save(destination)
    return frame, destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("captures", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--font", type=Path, default=Path("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"))
    parser.add_argument("--font-size", type=int, default=16)
    args = parser.parse_args()
    output = args.output or args.captures
    output.mkdir(parents=True, exist_ok=True)
    frames = [render(path, output, args.font, args.font_size) for path in sorted(args.captures.glob("*.json"))]
    if not frames:
        parser.error("No exported frame JSON files found")
    figures = []
    for frame, path in frames:
        caption = f"{frame['name']} · {frame['width']}×{frame['height']} · {frame['profile']} · {frame.get('theme', 'Observation')}"
        encoded = base64.b64encode(path.read_bytes()).decode()
        figures.append(f"<figure><figcaption>{html.escape(caption)}</figcaption><img alt='{html.escape(caption)}' src='data:image/png;base64,{encoded}'><details><summary>Frame text</summary><pre>{html.escape(frame['text'])}</pre></details></figure>")
    gallery = """<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Agenturm command center visual review</title><style>:root{color-scheme:light dark;font-family:system-ui,sans-serif;background:light-dark(#f4f5f7,#101114);color:light-dark(#202633,#ebedf2)}body{margin:0 auto;padding:2rem;max-width:1600px}h1{font-size:1.7rem}p{max-width:75ch;line-height:1.6}figure{margin:2rem 0;padding:1rem;border:1px solid light-dark(#ced2dc,#353943);border-radius:12px;background:light-dark(#fff,#181a1f)}figcaption{font-weight:650;margin-bottom:1rem}img{display:block;max-width:100%;height:auto}details{margin-top:1rem}pre{overflow:auto;line-height:1.3}@media(max-width:600px){body{padding:.8rem}figure{padding:.6rem}}</style><h1>Agenturm command center visual review</h1><p>Actual Ratatui TestBackend cells, rendered with a fixed-width font. These controlled fixtures cover wide and narrow terminals, monochrome, nested agents, Unicode, duplicate project names, partial collection, and empty states. Browser scaling does not change the tested terminal dimensions.</p>""" + "".join(figures) + "</html>"
    (output / "index.html").write_text(gallery)
    print(f"Rendered {len(frames)} pinned frames; gallery: {output / 'index.html'}")


if __name__ == "__main__":
    main()
