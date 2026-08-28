/**
 * Generate DESIGN_SYSTEM.md — a concise, machine-readable design contract for the
 * coding agent, so every session has the design tokens + layout conventions in
 * context and STOPS re-inventing them (the documented root cause of UI drift).
 *
 * The token table, status tokens, and radius scale are derived DIRECTLY from the
 * shadcn CSS-variable definitions in `src/index.css` (the single source of truth),
 * so they cannot drift. The convention prose points at the lint scripts that
 * ENFORCE it (lint-hardcoded-colors, biome guardrails) — not a second copy of the
 * rules. Output is intentionally short: agents read it every session.
 *
 * Run:   node scripts/gen-design-spec.mjs           # write DESIGN_SYSTEM.md
 *        node scripts/gen-design-spec.mjs --check    # CI: fail on drift
 */
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'

import { parseMulti, parseOne } from './roots.mjs'

// Parameterized over the app's tree:
//   --css <file>          the token source of truth. REPEATABLE, in cascade order:
//                         an app that LAYERS its own sheet over a package's token
//                         sheet passes both (`--css ../../sdk/packages/kit/src/
//                         styles/tokens.css --css src/styles/nocturne.css`), and a
//                         later file's declarations win, exactly as CSS resolves
//                         them at run time.
//   --dark-selector <sel> the selector carrying the dark palette (default `.dark`).
//                         Matched against each comma-separated part of a block's
//                         selector LIST, so `.dark, .app-force-dark { … }` is found.
//   --out <file>          where to write/check the spec.
//   --app-name <name>     the contract's title.
// All resolved vs CWD; defaults reproduce ziee's single-`src/index.css` layout.
const CSS_FILES = (() => {
  const given = parseMulti('css')
  return (given.length ? given : ['src/index.css']).map(f => path.resolve(process.cwd(), f))
})()
const OUT = path.resolve(process.cwd(), parseOne('out') ?? 'DESIGN_SYSTEM.md')
const APP_NAME = parseOne('app-name') ?? 'Ziee'
const DARK_SELECTOR = parseOne('dark-selector') ?? '.dark'

/** Exit with a message an app author can act on — not an uncaught stack trace. */
function fail(msg) {
  console.error(`[design-spec] ${msg}`)
  process.exit(1)
}

for (const f of CSS_FILES) if (!fs.existsSync(f)) fail(`--css ${f} does not exist`)
// Concatenated in the order given == the order the app @imports them, so the
// merge below reproduces the cascade rather than whichever file was named first.
// Comments are stripped up front: a block comment can contain braces, parens and
// semicolons, which is exactly the punctuation the selector scan below anchors on.
const css = CSS_FILES.map(f => fs.readFileSync(f, 'utf-8'))
  .join('\n')
  .replace(/\/\*[\s\S]*?\*\//g, '')

/**
 * Every top-level block whose selector LIST contains `selector`, in document
 * order. Plural on purpose: a real token sheet declares `:root` more than once
 * (theme-independent tokens, then the palette, then the kit contract), and an app
 * that layers sheets has several files' worth. Taking only the FIRST match read
 * one arbitrary slice of the palette and silently dropped the rest.
 */
function blocks(selector) {
  const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  // The selector may be one part of a comma-separated list: `.dark, .x {`.
  const re = new RegExp(`(?:^|[},;])\\s*((?:[^{}();]|\\([^)]*\\))*?${esc}(?:[^{}();]|\\([^)]*\\))*?)\\{`, 'gm')
  const found = []
  for (const m of css.matchAll(re)) {
    const parts = m[1].split(',').map(p => p.trim()).filter(Boolean)
    if (!parts.includes(selector)) continue
    let depth = 0
    const start = m.index + m[0].length - 1 // at the `{`
    for (let i = start; i < css.length; i++) {
      if (css[i] === '{') depth++
      else if (css[i] === '}' && --depth === 0) {
        found.push(css.slice(start + 1, i))
        break
      }
    }
  }
  return found
}

// `--name: value;` pairs from a block body (comments stripped).
function vars(body) {
  const out = new Map()
  for (const m of body.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/--([\w-]+)\s*:\s*([^;]+);/g)) {
    out.set(m[1].trim(), m[2].trim())
  }
  return out
}

/** Merge every matching block in cascade order — later declarations win. */
function mergedVars(selector, { required = true } = {}) {
  const bodies = blocks(selector)
  if (!bodies.length) {
    if (!required) return new Map()
    fail(
      `no \`${selector}\` block in ${CSS_FILES.map(f => path.relative(process.cwd(), f)).join(' + ')}. ` +
        (selector === '@theme inline'
          ? 'An app that layers its sheet over a package token sheet must pass BOTH with repeatable --css.'
          : `Name the app's selector with --dark-selector.`),
    )
  }
  const out = new Map()
  for (const b of bodies) for (const [k, v] of vars(b)) out.set(k, v)
  return out
}

const theme = mergedVars('@theme inline') // --color-X -> var(--X), --radius-Y -> calc(...)
const root = mergedVars(':root')
const dark = mergedVars(DARK_SELECTOR)

// Resolve `var(--x)` one hop into the :root / .dark palette.
const deref = (v, palette) => {
  const m = /^var\(--([\w-]+)\)$/.exec(v)
  return m ? (palette.get(m[1]) ?? v) : v
}

// --- semantic color tokens (ordered as declared in @theme inline) ---------------
// name -> one-line "when to reach for it". Missing names fall back to a generic hint.
const HINTS = {
  background: 'page surface',
  foreground: 'default text on the page',
  card: 'raised card surface',
  'card-foreground': 'text on cards',
  popover: 'menus / dropdowns / overlays',
  'popover-foreground': 'text in overlays',
  primary: 'brand accent — buttons, active/selected (theme-driven; NEVER hardcode a blue)',
  'primary-foreground': 'text/icons on a primary fill',
  secondary: 'subtle neutral surface / secondary button',
  'secondary-foreground': 'text on secondary',
  muted: 'muted surface (subtle backgrounds)',
  'muted-foreground': 'secondary / helper text',
  accent: 'hover / active subtle highlight',
  'accent-foreground': 'text on accent',
  destructive: 'errors, destructive/delete actions',
  'destructive-foreground': 'text on destructive',
  success: 'success status',
  'success-foreground': 'text on success fill',
  warning: 'warning status',
  'warning-foreground': 'text on warning fill',
  info: 'informational status (teal — distinct from brand)',
  'info-foreground': 'text on info fill',
  border: 'default borders / dividers',
  input: 'input & control borders',
  ring: 'focus ring (theme-driven)',
  sidebar: 'sidebar chrome surface',
}
const hint = (n) =>
  HINTS[n] ??
  (n.startsWith('chart-')
    ? 'data-viz series color (charts only)'
    : n.startsWith('sidebar')
      ? 'sidebar chrome'
      : '')

const colorTokens = []
for (const [k, v] of theme) {
  if (!k.startsWith('color-')) continue
  const name = k.slice('color-'.length)
  colorTokens.push({
    name,
    light: deref(v, root),
    dark: deref(v, dark),
    hint: hint(name),
  })
}

// --- radius scale ---------------------------------------------------------------
const radiusBase = root.get('radius') ?? '?'
const radii = [...theme].filter(([k]) => k.startsWith('radius-')).map(([k, v]) => [k.replace('radius-', ''), v])

// --- compose the spec -----------------------------------------------------------
const stamp = 'GENERATED by `npm run gen:design-spec` from `src-app/ui/src/index.css` — do not edit by hand.'
const row = (t) =>
  `| \`--${t.name}\` | \`bg-${t.name}\` / \`text-${t.name}\` | ${t.hint} |`

let md = `# ${APP_NAME} Design System (generated)

> ${stamp}
> Regenerate after any token change: \`cd src-app/ui && npm run gen:design-spec\`.
> Drift is caught by \`npm run check:design-spec\` (part of \`npm run check\` + CI).

The UI is a **web-only shadcn (new-york-v4) + Tailwind v4** app. Colors are OKLCH/HSL
CSS variables themed for light **and** dark; the accent (\`--primary\`/\`--ring\`/\`--sidebar-primary\`)
is user-configurable at runtime by \`ThemeProvider.applyAccent\`. **Always use a semantic
token class — never a raw Tailwind hue** (\`bg-blue-500\`), an arbitrary value (\`bg-[#1e90ff]\`),
or an inline \`style\` color: those bypass the accent + dark-mode system and are the root
cause of visual drift.

## Semantic color tokens

Use the class, not the value. Each token is theme-aware (light + dark resolved below).

| token | Tailwind class | when to use |
|---|---|---|
${colorTokens.map(row).join('\n')}

Every \`*-foreground\` is the accessible text/icon color to pair with its fill.
\`chart-1..5\` are for data-viz series ONLY (see the dataviz skill), never as UI accents.

<details><summary>Resolved values (light / dark) — reference only, use the class</summary>

| token | light | dark |
|---|---|---|
${colorTokens.map((t) => `| \`--${t.name}\` | \`${t.light}\` | \`${t.dark}\` |`).join('\n')}

</details>

## Radius scale

Base \`--radius: ${radiusBase}\`. Use the Tailwind radius utilities; do not invent \`rounded-[Npx]\`.

| utility | value |
|---|---|
${radii.map(([n, v]) => `| \`rounded-${n}\` | \`${v}\` |`).join('\n')}

## Spacing rhythm

Tailwind v4 **4px base** — every \`gap-N\`/\`p-N\`/\`m-N\` is \`N × 4px\` (\`gap-2\` = 8px).
Stay on the scale; never \`gap-[7px]\`. House conventions (what the codebase actually uses):

- \`gap-2\` (8px) — default gap between controls **within** a row/group.
- \`gap-3\`/\`gap-4\` (12/16px) — between fields, and between stacked sections.
- \`gap-6\` (24px) — between major page blocks. \`gap-1\` — tight icon/label pairs only.
- Prefer flex/grid \`gap-*\` over margins on children for even, direction-agnostic rhythm.
- **Logical direction only (RTL-ready)** — new components use logical direction properties only
  (\`ps/pe\`, \`ms/me\`, \`start/end\`, \`text-start/text-end\`), never the physical \`pl/pr\`, \`ml/mr\`,
  \`left/right\`, \`text-left/text-right\`; directional icons (chevrons, back/forward) must flip under
  RTL. Enforced on new/changed code by \`scripts/lint-logical-direction.mjs\`
  (\`npm run lint:logical-direction\`, taxonomy N1); a genuine physical need opts out with an inline
  \`rtl-ok\` marker.

## Form & settings layout — use \`Field\`, not raw flex-gap

Do **NOT** hand-roll label + control + help text with \`flex flex-col gap-*\`. Compose the kit's
field primitives so spacing, label association, invalid state, and a11y come for free:

- \`Field\` / \`FieldLabel\` / \`FieldDescription\` / \`FieldError\` / \`FieldGroup\` / \`FieldSet\`
  (\`@/components/ui\`) — the label/description/error stack. \`FieldGroup\` owns the inter-field gap.
- \`Form\` + \`FormField\` (\`@/components/ui\`, react-hook-form + zod) — for validated forms.
- Settings pages: wrap the page in \`SettingsPageContainer\` and each section in \`Card\`
  (\`@/components/ui\`); match the existing settings cards rather than free-styling sizes/spacing.

## Component variant selection — quiet variants in dense/narrow containers

Match the variant's visual WEIGHT to the container's density (taxonomy J5):

- **Tabs** — a side panel, drawer, or toolbar is narrow + dense: use \`<Tabs variant="line">\`
  (quiet UNDERLINE tabs). Reserve the default boxed/segmented pill strip for wide, primary
  tabbed surfaces. A boxed strip in a narrow side panel reads as heavy chrome.
- **Section headers** — a title with actions uses \`<SectionHeader title actions>\`
  (\`@/components/ui\`), not \`Card title=/extra=\`: it keeps the title + actions on ONE row
  (title truncates; actions never wrap), avoiding the mobile premature-stack (taxonomy B1).
- **Action groups** — peer icon-only buttons in one group share ONE variant (all \`ghost\` or
  all \`outline\`); don't mix (taxonomy J6). Reserve a distinct variant for a genuine
  primary/secondary split.

## Forbidden patterns (lint-enforced — a build fails on these)

- **Hardcoded colors** — raw hues (\`bg-red-600\`), arbitrary color values (\`text-[#fff]\`,
  \`border-[hsl(...)]\`), or inline \`style\` color props. Enforced by
  \`scripts/lint-hardcoded-colors.mjs\` (\`npm run lint:colors\`). Genuinely-dynamic color
  (e.g. a swatch picker) opts out per element with \`data-allow-custom-color\`.
- **Importing \`antd\`** or using raw \`<button>/<input>/<select>/<textarea>\` — use the kit
  (Biome \`noRestrictedImports\` guardrail, \`npm run lint:guardrails\`). Kit props (incl. the
  required \`data-testid\` / accessible-name props) are documented in
  \`src/components/ui/KIT_MANIFEST.md\`.
- **Editing \`shadcn/\` or \`kit/\`** to restyle app UI — those DEFINE the tokens; consume them.
`

// --- write / check --------------------------------------------------------------
if (process.argv.includes('--check')) {
  const existing = fs.existsSync(OUT) ? fs.readFileSync(OUT, 'utf-8') : ''
  if (existing.trim() !== md.trim()) {
    console.error('DESIGN_SYSTEM.md is stale — run `npm run gen:design-spec` and commit.')
    process.exit(1)
  }
  console.log('DESIGN_SYSTEM.md up to date.')
} else {
  fs.writeFileSync(OUT, md)
  console.log(`Wrote ${OUT} (${colorTokens.length} tokens, ${radii.length} radii).`)
}
