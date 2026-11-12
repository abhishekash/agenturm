# Agenturm — Product Hunt launch package

Draft launch materials for Agenturm. This is copy and a checklist only; nothing has been submitted or published.

## Positioning

**For developers running multiple coding agents:** Agenturm is a local, terminal-first monitor that brings session state, recorded activity, tool and MCP calls, and traces into one keyboard-driven interface. It observes agent work; it does not control agents or handle their approvals.

Keep the launch centered on the concrete job: **find what an agent did and where a run got stuck.** Lead with the local, cross-agent workflow. Avoid presenting Agenturm as a general-purpose hosted LLM observability platform.

## Product Hunt submission fields

**Name**  
Agenturm

**Tagline — 56 characters**  
Local observability for coding agents, in your terminal.

**Description — 240 characters**  
> A local, terminal-first monitor for Codex, Claude Code, OpenCode, and Pi. See session state and recorded progress, inspect tool and MCP calls, and follow traces in one keyboard-driven interface. Local monitoring needs no account or API key.

**Primary URL**  
https://www.agenturm.dev/

**Additional link**  
GitHub: https://github.com/abhishekash/agenturm

**Topics**  
Choose the closest available three: Developer Tools, Open Source, Artificial Intelligence.

**Pricing**  
Free. MIT-licensed and usable without an account. Optional AI summaries use a provider the user configures and requests explicitly.

**Makers**  
Add Abhishek from a personal Product Hunt account. Use the actual Product Hunt profile handle and credit any co-makers who want to be listed.

**Shoutouts**  
Optional: Textual, OpenTelemetry, and uv, if those entries are available and you want to credit them.

**Promo**  
None. The product is free and open source; no invented offer or discount is needed.

## Maker comment

Hi Product Hunt 👋 I’m Abhishek, the maker of Agenturm.

When I started running several coding agents, I kept switching between sessions and digging through logs to answer simple questions: Which agent is still working? What did it try? Where did the run get stuck?

I built Agenturm to make that evidence easier to inspect from one keyboard-driven terminal interface. It can discover recent local records from Codex, Claude Code, OpenCode, and Pi. Other instrumented agents can send OpenTelemetry to its local receiver. You can inspect recorded progress, activity, MCP calls, and traces.

Local monitoring stores data in SQLite on your machine and needs no account or API key. Agenturm is a monitor: replies and approvals stay in the agent’s own terminal. The website demo uses synthetic records; it does not read your machine.

To try it, you need Python 3.11+ and uv:

```bash
uvx agenturm
```

Or install the command with:

```bash
uv tool install agenturm
agenturm
```

I’d especially value feedback from people who use more than one coding agent: Which records or events are missing? Is it easy to find the call that caused a run to stop? Tell me what was confusing or open an issue on GitHub. No need to upvote—useful feedback is the goal.

## Launch media

Product Hunt’s current submission guide calls for a square thumbnail, at least two gallery images, and a quick demo video. It currently accepts YouTube links for the video, so upload the final MP4 to YouTube as public or unlisted, then use the full video URL in the draft. Do not set it to private.

### Thumbnail

- Source: `media/brand/icon-512.png`
- Export square, around 240 × 240 pixels, and confirm the upload is under 3 MB.
- Check the small-size crop and contrast before scheduling.

### Gallery order and captions

1. **Across your agents, at a glance** — source `media/screenshots/agents.png`  
   Caption: “See recorded state and recent progress across supported coding agents.”
2. **Inspect the call behind a failure** — source `media/screenshots/mcp-detail.png`  
   Caption: “Open a recorded MCP call to inspect its arguments, response, and error.”
3. **Follow the run into its trace** — source `media/screenshots/traces.png`  
   Caption: “Follow a run from the agent operation to the tool call that failed.”

The screenshots are 1920 × 1234. Export legible gallery versions at Product Hunt’s recommended 1270 × 760 size; preserve enough terminal text to read the relevant evidence. The demo data is synthetic, so keep that label visible in the gallery and video.

### Video upload

**YouTube title**  
Agenturm — see what your coding agents are doing

**YouTube description**  
Agenturm is an open-source, local terminal monitor for coding-agent sessions. This walkthrough uses synthetic records and shows the actual terminal interface. Local session data stays on your machine.

Try it: https://www.agenturm.dev/  
Source: https://github.com/abhishekash/agenturm

**Suggested 29-second narration/caption sequence**

- **0–4s — Agents view:** “Running more than one coding agent? See their recorded state in one terminal.”
- **4–10s — Select a session:** “Open a session to review its task and recent activity.”
- **10–18s — MCP call detail:** “Inspect the arguments, response, and error behind a tool call.”
- **18–25s — Trace view:** “Follow the call in the run and see where it stopped.”
- **25–29s — End card:** “Agenturm. Local observability for coding agents, in your terminal. agenturm.dev”

Before uploading, watch the exact final MP4 once for legible text, readable pacing, the synthetic-data label, and a clear end card. The local composition file was not independently reviewed while preparing this package.

## Launch-day promotion drafts

Replace `[Product Hunt link]` after the scheduled page exists. Share where you already have a relevant audience; don’t send unsolicited bulk messages.

### X / Bluesky

I built Agenturm to answer a practical question when several coding agents are running: what is each one doing, and where did a run get stuck?

It brings local Codex, Claude Code, OpenCode, and Pi activity into one terminal view, with recorded tool calls and traces. Open source and local by default.

I’m launching on Product Hunt today: [Product Hunt link]

If you try it, I’d love to hear what’s missing or confusing.

### LinkedIn

I’ve been building Agenturm, a local terminal monitor for coding-agent sessions, and it’s live on Product Hunt today.

Agenturm brings supported Codex, Claude Code, OpenCode, and Pi records into one keyboard-driven interface. You can inspect recorded progress, activity, MCP calls, and traces without sending your session history to a hosted dashboard. Other instrumented agents can send OpenTelemetry to the local receiver.

It’s open source under MIT. Local monitoring needs no account or API key. Replies and approvals stay in the agent’s own terminal.

Try the demo or install it: https://www.agenturm.dev/  
Product Hunt: [Product Hunt link]

I’m looking for feedback on first-run setup, missing agent events, and how quickly you can find a failed call.

### Note to early testers or colleagues

Subject: Agenturm is on Product Hunt today

Hi — I’ve launched Agenturm, my local terminal monitor for coding-agent sessions, on Product Hunt today. If you use Codex, Claude Code, OpenCode, or Pi, I’d value a try and honest feedback on the setup and investigation flow. There’s no need to vote; a note about what worked or where you got stuck is more useful.

Product Hunt: [Product Hunt link]  
Website and install: https://www.agenturm.dev/

## Short reply bank

**Which agents does it support?**  
Agenturm discovers recent local records from Codex, Claude Code, OpenCode, and Pi. Other instrumented agents can send OpenTelemetry; the available detail depends on what the producer emits.

**Does it control agents or approve actions?**  
No. Agenturm monitors recorded work. Reply to agents and approve requests in their own terminals.

**Does my data leave my machine?**  
Supported records are read locally and stored in local SQLite. Captured messages, commands, and tool results can include sensitive project information; credential redaction is best-effort. Optional AI summaries send selected session evidence to your configured provider only when you explicitly request one. See the privacy guide: https://github.com/abhishekash/agenturm/blob/main/docs/privacy.md

**Why is my view empty?**  
Agenturm needs recent records from a supported agent or telemetry from an instrumented producer. Check the “No sessions found?” help on the website or the usage guide: https://github.com/abhishekash/agenturm/blob/main/docs/usage.md

**What do I need to install it?**  
Python 3.11+ and uv. The website covers `uvx`, `uv tool`, the macOS/Linux installer, Windows, and source installs.

**Is it free?**  
Yes. Agenturm is MIT-licensed and local monitoring needs no account or API key. Optional AI summaries use your configured provider.

## Pre-launch checklist

- [ ] Create the Product Hunt draft under a personal maker account.
- [ ] Confirm the site URL, GitHub URL, tagline, and 240-character description.
- [ ] Check the install command and release notes point to the version you intend to launch.
- [ ] Export the square thumbnail and at least two readable gallery images; keep each image under 3 MB.
- [ ] Upload the final video to YouTube as public or unlisted, then add its full URL.
- [ ] Ensure the synthetic-data label is visible in the video and screenshots.
- [ ] Add the maker comment before scheduling.
- [ ] Make the first-run data requirement easy to find on the landing page.
- [ ] Choose a day when you can answer comments, especially during the first few hours. Product Hunt currently schedules launches for 12:01 AM Pacific and allows scheduling up to a month ahead.
- [ ] Set a learning goal: actual first runs from target developers and actionable feedback. Do not make rank the only measure of success.
- [ ] Ask people to try it or share feedback, never to upvote.

## References

- Product Hunt [Launch Guide](https://help.producthunt.com/en/articles/17350772-launch-guide) — current tagline/description limits, submission fields, and launch-day guidance.
- Product Hunt [Preparing for launch](https://www.producthunt.com/launch/preparing-for-launch) — media requirements, YouTube video guidance, scheduling, and maker comment.
- Agenturm [website](https://www.agenturm.dev/), [repository](https://github.com/abhishekash/agenturm), [usage guide](https://github.com/abhishekash/agenturm/blob/main/docs/usage.md), and [privacy guide](https://github.com/abhishekash/agenturm/blob/main/docs/privacy.md).
