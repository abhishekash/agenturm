"""Prepare derived media from synthetic captures; never read local agent records."""
import argparse
import json
from pathlib import Path

from PIL import Image


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--captures", type=Path, required=True)
    args = parser.parse_args()
    assets = Path(__file__).parent / "composition" / "assets"
    crops = {
        "sessions": ("agents", (28, 64, 1892, 760)),
        "call": ("mcp-detail", (55, 345, 1040, 1045)),
        "trace": ("traces", (55, 775, 1120, 1140)),
    }
    for name, (source, bounds) in crops.items():
        with Image.open(args.captures / f"{source}.png") as image:
            image.crop(bounds).save(assets / "screens" / f"{name}-crop.png")
    audio = json.loads((assets / "music" / "audio-data.json").read_text())
    # Build-time modulation only: deterministic transition energy, never a loop.
    cues = {}
    for name, time in (("call", 8.74), ("trace", 13.11), ("close", 17.47)):
        frames = audio["frames"][int(time * 30):int((time + 0.3) * 30)]
        energy = max(max(frame["bands"][:2]) for frame in frames)
        cues[name] = round(0.72 + energy * 0.28, 4)
    (assets / "music" / "cue-energy.js").write_text(
        "window.AGENTURM_CUE_ENERGY = " + json.dumps(cues) + ";\n"
    )


if __name__ == "__main__":
    main()
