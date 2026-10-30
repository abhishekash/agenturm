# Agenturm launch film

24 seconds · 1920 × 1080 · 30 fps · music and restrained SFX · no narration.

- `brag.mp4`: final video; strongest settled product frame baked as frame zero.
- `brag.jpg`: separate poster for upload or an HTML `poster` attribute.
- `share-copy.txt`: ready-to-post caption with required music attribution.
- `brag-plan.md` / `composition-brief.md`: concept, storyboard, source boundaries.
- `composition/`: editable HyperFrames project with local assets and fonts.
- `CREDITS.md`: media attribution and licensing.

The current app's actual terminal is captured with synthetic records. Crops preserve its Observation palette; the surrounding graphics use the website's dark, amber, and lavender identity. The failure remains a failure; the film does not suggest that Agenturm fixed or controlled the session.

## Preview and render

Requires Node.js 22+, Chrome, FFmpeg and ffprobe on `PATH`.

From `brag-output/composition`:

```bash
npm run check
npx hyperframes@0.8.207 preview --background
npx hyperframes@0.8.207 render --quality delivery --fps 30 --output ../brag.mp4
```

All necessary assets are included locally. No provider account or paid generation is used.

The final export was verified as 720 H.264 frames at 30 fps with a 24-second AAC audio track. All 51 sampled text contrast checks passed; the full HyperFrames gate reported no errors or warnings. Decoded frames from every scene and the baked first-frame poster were visually inspected. Audio peaks at approximately −4.7 dBFS (no clipping). See `validation.json` and `media-verification.json`.

## Refresh the UI assets

From the repository root (requires uv):

```bash
uv run --with cairosvg python scripts/capture_investigation_video.py \
  --size 112x36 --output /tmp/opencode/agenturm-brag-capture \
  --screenshots /tmp/opencode/agenturm-brag-capture/png
uv run --with pillow python brag-output/prepare-assets.py \
  --captures /tmp/opencode/agenturm-brag-capture/png
```

`prepare-assets.py` makes crops and derives transition-presence values from the included audio-analysis JSON. The audio analysis was produced by the installed `media-use/audio/scripts/extract-audio-data.py` helper at 30 fps with eight bands. No analysis happens at playback time.

After a fresh render, extract the settled hero at 1.3 seconds and replace only frame zero (audio and duration remain unchanged):

```bash
ffmpeg -ss 1.3 -i brag-output/brag.mp4 -frames:v 1 -q:v 2 brag-output/brag.jpg
ffmpeg -i brag-output/brag.mp4 -i brag-output/brag.jpg \
  -filter_complex "[0:v][1:v]overlay=0:0:enable='eq(n,0)'[v]" \
  -map '[v]' -map '0:a?' -c:v libx264 -crf 18 -preset slow \
  -pix_fmt yuv420p -c:a copy -movflags +faststart brag-output/brag.poster.mp4
```

Then replace the original video with `brag.poster.mp4`. Keep music attribution when sharing.
