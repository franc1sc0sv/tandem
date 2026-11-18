---
paths:
  - "apps/tandem-ui/**"
---
UI: the visual reference is the v0.3 mockups, `mockups/v03/*.dc.html` (which screen is which, and what DECISIONS corrects: `mockups/README.md`). `mockups/main.html` and `next.html` are history (v0.1 and v0.2): not used as a reference. If a component or a screen differs from the mockup, fix the app: the mockup rules.

All UI text is in English, with the terms of `docs/GLOSSARY.md`.

How it is checked:

- `cd apps/tandem-ui && pnpm shots`: captures every demo-mode screen and compares it with its mockup capture in `docs/mockups-png/` (similarity and worst tile, in `docs/screenshots/v03/similarity.json`), and compares the computed styles of each gallery component (`gallery.html`) with the same element of the mockup (`scripts/style-parity-spec.ts`). `pnpm shots --check` fails if a zone with a threshold misses it or if a style property differs; it is what the `ui (screenshots)` job runs.
- Compare a single capture: `cd tools/mockup-render && pnpm mockups:compare app.png ../../docs/mockups-png/V3-Desk.png --diff diff.png`. View a mockup: `pnpm mockups:serve`.

Sober style: muted colors, one soft accent per machine, no neon glow, no flashy gradients or animated backgrounds. Visuals before text. Include a screenshot in the PR (from `docs/screenshots/v03/`, or the gallery `gallery.png` if components change).
