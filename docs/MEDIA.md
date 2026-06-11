# Media assets (README hero, demo GIF, social preview)

These are the visual assets the public repo needs. They can't be generated from source — they require
capturing the running app. This file is the spec; drop the produced files at the paths noted and they
wire up automatically.

## 1. Hero / demo GIF → `docs/demo.gif`  *(highest impact — the #1 star driver)*

Referenced at the top of the README. Aim for a **30–45s**, **1280×800** (retina), **<5 MB** GIF.

Suggested flow:
1. (0–5s) Launch Furx → the multi-pane grid.
2. (5–15s) Furx Connect wizard → paste one OpenRouter key (auto-saved to keychain).
3. (15–30s) Open Council Mode (⌘J), type one prompt, watch several models answer in parallel, merge.
4. (30–45s) Open the audit log → show the new append-only entry.

Capture with Kap / OBS; compress with `gifski` or `gifsicle --optimize=3 --colors 256`.

## 2. Social preview → `docs/social-preview.png` (1280×640)

Shown when the repo is shared on X/LinkedIn/Slack. Set it in **GitHub → Settings → Social preview**.

- Left: Furx wordmark + tagline "Run any coding agent side-by-side. No proxy."
- Right: a clean screenshot of the 4-pane grid with the Council Mode modal open (legible code, good contrast).
- Footer: "macOS · Linux · Windows" + "Apache-2.0".
- Avoid tiny/illegible text and low-contrast dark-on-dark.

## 3. Optional static screenshots → `docs/screenshots/`

Nice-to-have, referenced from `docs/features.md` if added:
- `connect-wizard.png` — provider setup.
- `council-merge.png` — Council Mode results + merge.
- `audit-log.png` — audit timeline with filters.

> Until `docs/demo.gif` exists, the README shows a broken-image placeholder. This is the top
> pre-launch human task — see the publish checklist.
