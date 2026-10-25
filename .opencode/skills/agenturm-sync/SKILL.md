---
name: Agenturm Sync
description: After commits or changes in Agenturm (abhishekash/agenturm) or its website (abhishekash/agenturm-dev), compare the app, marketing site, repository README, and GitHub profile README for current product, installation, privacy, logo, and theme consistency; identify drift and update dependent materials when requested.
---

# Agenturm cross-project sync

Use this workflow after a commit, pull request, or release changes either `abhishekash/agenturm` or `abhishekash/agenturm-dev`. Also use it when changing the Agenturm section or visuals on the `abhishekash/abhishekash` GitHub profile. The profile repository contains other projects and personal content; preserve them.

This is a review-and-sync workflow, not a Git hook. It runs when the skill is invoked during the relevant work; a commit by itself does not execute an OpenCode skill.

## Workflow

1. **Identify the commit and repositories.** Run `git rev-parse --show-toplevel`, inspect `git remote -v`, `git status --short --branch`, and review the new commit (`git show --stat` and relevant parts of `git show`). Identify the repository by its `abhishekash/agenturm` or `abhishekash/agenturm-dev` remote, not by directory name. Never reset, clean, switch away from, or overwrite user changes. Locate the profile repository (`abhishekash/abhishekash`) separately if available.
2. **Read current sources before comparing.** For the app, inspect the changed implementation and authoritative docs (especially usage, integrations, and privacy). For the website, inspect its current page copy, install section, integration section, design tokens, and brand assets. For the profile, inspect `README.md`, `scripts/build_profile_assets.py`, and any generated responsive hero SVGs. Treat current source code and current website source as authoritative; do not copy claims from an older README or from memory.
3. **Check content consistency.** Compare the user-facing headline and product description, supported agents/integrations, install/run commands and prerequisites, feature/view claims, privacy/data-handling claims, documentation links, and any changed behavior. Keep wording appropriate to each destination, but correct stale claims and broken links. Verify shell commands against the current installer and project documentation before publishing them.
4. **Check visual consistency.** The current website design is dark by default with an optional light mode. Compare profile and repository artwork with the website's current dark palette, typography, and brand. Use the website's actual tower mark (currently `public/brand/tower.svg`, served as `/brand/tower.svg`) and its actual wordmark treatment; do not redraw or substitute the older filled tower illustration. If the site's logo asset changes, use the updated source. If the site's theme changes, compare the current tokens in `src/styles/global.css` rather than hard-coding values from this workflow.
5. **Report drift precisely.** Give a short matrix of each destination (app README/docs, website, GitHub profile) and mark it in sync, stale, or not checked. Cite the specific files/claims that differ. Distinguish a visual mismatch from a functional or factual mismatch. If a related checkout is unavailable, say so and provide the exact remote/source files still needing review.
6. **Make only scoped updates.** When asked to synchronize, update only the dependent content/assets needed to reflect the committed change. Preserve unrelated README sections, personal introduction, and other profile projects. Regenerate profile artwork with its checked-in build script when source inputs change; do not hand-edit generated SVGs without updating the generator. Follow each repository's branch/PR policy. Do not commit, push, merge, deploy, or bypass branch protection unless explicitly requested.
7. **Validate.** Run the repository's relevant build, tests, or asset-generation checks; verify internal links and changed commands; render/read updated SVGs at desktop and mobile sizes when visuals changed; and run `git diff --check`. Re-check `git status` so pre-existing user modifications remain untouched. Summarize changes, validation, and anything still blocked.

## Synchronization checklist

- App behavior and documentation agree with the website's feature descriptions.
- Website and profile both reflect the latest supported integrations and installation path.
- Privacy statements match current capture, storage, redaction, and optional-provider behavior.
- Website, repository README, and profile Agenturm visuals use the current logo and dark-default/light-toggle identity.
- Profile hero generator and desktop/mobile generated assets agree; other profile content remains intact.
- Links resolve to current pages and docs, and claims are not stronger than the implementation supports.
