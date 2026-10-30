# Brag Plan: Agenturm

## Planning rubric
1. **App:** Local observability for Codex, Claude Code, OpenCode, Pi, and configured OpenTelemetry producers, in a keyboard-driven terminal.
2. **Strongest claim:** “Know what your agents are doing.” Not an agent controller; approvals remain in the original terminals.
3. **Visual hook:** Website-scale condensed typography beside the actual terminal; amber focus, lavender details, architectural tower mark.
4. **Actual UI:** Fresh synthetic Agents, expanded MCP inspector, and linked execution tree, captured from current `main` using the existing in-memory demo feed.
5. **Shortest satisfying video:** 24 seconds, enough to read the error and then its context.
6. **Tone:** Polished; a quiet, precise instrument for the agent era.
7. **Audio:** A low, steady music bed with three warm keyboard/reveal accents; no narration.
8. **Caption:** Agenturm: follow your coding agents from one terminal. Inspect recorded progress, tool calls, and traces—local by default, open source, no account required. https://www.agenturm.dev/
9. **User flow:** See supported sessions → inspect a recorded failed GitHub call → follow its linked trace. No claim that the failure is fixed.

## Angle / hook
Lead with the site's exact headline, not a generic AI promise. Turn “knowing” into a concrete investigation: the request, its HTTP 403 response, and its place in the run. Close where the viewer can try it.

## Source and privacy
Reference: https://www.agenturm.dev/, fetched and successfully captured with HyperFrames. Product source: README.md, docs/usage.md, docs/privacy.md, docs/branding.md, observatory/themes.py, scripts/capture_investigation_video.py, scripts/render_terminal_demo.py.
All terminal content is fictional, supplied by the existing in-memory demo; no personal sessions, receiver, database, or prompts are opened. Current UI may label the recent synthetic session “Tool running”; do not mislabel that overview as “Waiting.” The recorded MCP response separately demonstrates a real error inspector.

## Tone / format
- Preset: polished.
- Direction: a terminal field guide, precise cuts and generous settled holds.
- Landscape, 1920 × 1080, 30 fps.
- Duration: 24 seconds.

## Visual identity
- Website background: #100f16; panel: #17151f; ink: #f4f1eb.
- Accent: #ffbd69; lavender: #c5adff; muted: #a6a0b2.
- Display: IBM Plex Sans Condensed, 600. Body: IBM Plex Sans, 400–600. Code: JetBrains Mono.
- Preserve the actual captured Observation terminal palette (#151a17, #dceb5a) rather than recoloring its pixels.
- Use the original tower SVG; no decorative flashing logo, gradients, particles, or fake dashboard.

## Audio direction / music cue guidance
- Track: Happy Beats / Business Moves Vol. 12, Sascha Ende, ende.app (CC BY 4.0).
- Steady, clean bed at 0.25 gain, 0.4s fade-in, 1.6s fade-out. Three restrained low-HF accents on the product reveal and keyboard actions.
- Cue preset read: `.agents/skills/brag/assets/music/cues/happy-beats-business-moves-vol-12-by-ende-dot-app.music-cues.json`; approximately 109.96 BPM.
- Major moments at 8.74s (inspect), 13.11s (trace), 17.47s (close). At most three strong cue locks.
- Sequential overview labels at 3.27, 4.39, 5.48 seconds: every-other-beat pacing, all remain visible through 8.5s.
- Audio-reactive: use extracted energy only to shape a slight accent-rule brightness at the three transitions; no ongoing pulse, no title scaling.
- Sound must not imply an error was resolved or an agent was controlled.

## Storyboard

### Scene 1 — Know — 0–3s (3s)
Exact headline: “Know what your agents are doing.” Brand wordmark and original tower. A framed crop of the actual terminal is already present at frame zero; the headline resolves in a restrained whole-line reveal by 0.6s and holds.
Sequential/interaction: headline arrives first, supporting line follows, no typing animation.
Audio: music enters softly; no novelty effects.
Transition: the same terminal presentation becomes the session overview.

### Scene 2 — See the sessions — 3–8.74s (5.74s)
“Every session. One terminal.” Show the actual session list with Codex, OpenCode, and Claude Code; Pi is named as a supported integration in the small agent rail, not invented as a captured fourth row.
Sequential/interaction: three quiet labels “Recorded progress”, “Tool calls”, “Traces” enter at reading-safe intervals; keyboard hint indicates selection.
Audio: one warm reveal accent; music continues.
Transition: move closer to the recorded call on the next strong beat.

### Scene 3 — Read the evidence — 8.74–17.47s (8.73s)
8.74–13.11: “Inspect the call.” Crop the actual expanded inspector to show github / get_pull_request, arguments, status_code 403, and “API rate limit exceeded”. Explicit synthetic-records label remains visible.
13.11–17.47: “Follow the trace.” Show the actual linked execution tree, highlighting the recorded MCP operation in context. Show the `t` keyboard action, never a mouse-driven fake controller.
Sequential/interaction: actual captured states switch on the keyboard action; tree is an image of the real UI, not a recreated diagram.
Audio: two quiet key sounds aligned with those actions; music supplies the major edit rhythm.
Transition: terminal recedes into a brand end card at 17.47.

### Scene 4 — Start here — 17.47–24s (6.53s)
“Start here. Stay in your terminal.” Agenturm wordmark, original tower, `agenturm.dev`, and the exact install command `curl -fsSL https://www.agenturm.dev/install.sh | bash`; separate launch command `agenturm`. Small prerequisites: Python 3.11+, Git, uv. Principles: “Open source · Local by default · No account required”.
Sequential/interaction: command and launch line settle by 18.5s; final card holds still for over five seconds.
Audio: music fades out naturally, no voiceover.

**Total:** 3 + 5.74 + 8.73 + 6.53 = 24 seconds.
