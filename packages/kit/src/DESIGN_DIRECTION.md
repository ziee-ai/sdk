# ziee — design direction ("bench notebook")

The enforceable identity for the kit. Refined & precise; differentiation rests on typography
and the provenance margin, not a loud color. Subject: a research workbench for life scientists;
the chat surface's job is *think with the literature and your data*.

## Accent (BUILT)
The accent **is** the brand/primary color — it drives `--primary` + `--ring` + the sidebar
primary/ring, so buttons, links, focus, selected states, and the chat provenance marker share
one hue. **User-selectable** in Settings → Appearance (presets only — every preset is
contrast-tuned for WCAG AA in both light & dark). Default = a neutral slate-blue.
- Presets + apply logic: `components/ThemeProvider/accentPresets.ts` (`applyAccent`).
- Persisted via `ConfigClient` (zustand persist, alongside `themePreference`).
- Applied by `ThemeProvider` (sets the CSS vars on the document root per resolved theme).
- First-paint defaults live in `index.css` (`:root` + `html.dark` + `.dark`).
- To add a preset: add to `ACCENT_PRESETS` with light+dark `{primary, fg}` (HSL channels),
  AA-verified; the picker + registry pick it up automatically.

## Typography (PENDING)
- Body/UI: **Atkinson Hyperlegible Next** (legibility-engineered — beauty + a11y in one choice).
- Display (headings, margin numbers): **Space Grotesk**, used with restraint.
- Mono (citations, data, provenance ticks, code): **IBM Plex Mono**.
- Deliver via `@fontsource`; set a deliberate type scale.

## Signature — the provenance margin (PENDING)
A precise left rail down the chat + panels: hairline ticks + monospaced turn numbers, with the
accent marking the turn whose citations are shown in Sources. Numbering encodes real
reasoning/citation order (justified structure, not decoration). Pilot surface: chat.

## Palette field (PENDING)
Move the neutral field from pure white toward a cool-clinical paper (faint blue-grey) — NOT
cream. Keep ink near-black with a slate undertone.

## Quality floor (always)
Responsive to mobile, visible keyboard focus, reduced-motion respected, AA contrast. Motion is
minimal and purposeful (≤150ms reveals, hover micro-states) — nothing ambient.

## Copy voice
Active voice; name things by what the user controls; errors direct ("3 sources couldn't be
reached — retry", never "Error"); one action keeps its name through the whole flow.
