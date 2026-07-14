/**
 * Accent presets — the user-selectable brand color (Settings → Appearance).
 *
 * The accent IS the primary/brand color: it drives `--primary` (+ `--ring` and the sidebar
 * primary/ring) so buttons, links, focus rings, selected states, and the chat provenance
 * marker all share one hue. Presets only (no free hex) so every option is contrast-tuned in
 * BOTH themes. Values are HSL channels ("H S% L%") to match the token format in index.css.
 *
 * Each preset gives a light + dark variant with a foreground tuned for AA contrast on that
 * fill (white on the mid-dark light variants; near-black on the brighter dark variants).
 */
export interface AccentVariant {
  /** `--primary` / `--ring` fill, HSL channels. */
  primary: string
  /** `--primary-foreground` (text/icon on the fill), HSL channels. */
  fg: string
}
export interface AccentPresetDef {
  label: string
  light: AccentVariant
  dark: AccentVariant
}

export const ACCENT_PRESETS = {
  // Default — a calm, neutral slate-blue (~#3A5BA0). Differentiation rests on type + the
  // provenance margin, not a loud color.
  blue: {
    label: 'Blue',
    light: { primary: '220 47% 43%', fg: '0 0% 100%' },
    dark: { primary: '216 56% 64%', fg: '222 47% 11%' },
  },
  indigo: {
    label: 'Indigo',
    light: { primary: '234 44% 48%', fg: '0 0% 100%' },
    dark: { primary: '232 56% 70%', fg: '222 47% 11%' },
  },
  slate: {
    label: 'Slate',
    light: { primary: '215 25% 35%', fg: '0 0% 100%' },
    dark: { primary: '214 22% 66%', fg: '222 47% 11%' },
  },
  teal: {
    label: 'Teal',
    light: { primary: '188 62% 30%', fg: '0 0% 100%' },
    dark: { primary: '186 52% 58%', fg: '195 60% 9%' },
  },
  green: {
    label: 'Green',
    light: { primary: '152 46% 33%', fg: '0 0% 100%' },
    dark: { primary: '150 44% 56%', fg: '150 40% 9%' },
  },
  violet: {
    label: 'Violet',
    light: { primary: '265 40% 50%', fg: '0 0% 100%' },
    dark: { primary: '263 56% 72%', fg: '265 40% 12%' },
  },
  rose: {
    label: 'Rose',
    light: { primary: '345 55% 46%', fg: '0 0% 100%' },
    dark: { primary: '344 70% 68%', fg: '345 50% 12%' },
  },
  amber: {
    label: 'Amber',
    // light fill darkened to 35% L so white text clears WCAG AA (4.24:1 → ~5.0:1).
    light: { primary: '32 78% 35%', fg: '0 0% 100%' },
    dark: { primary: '38 82% 60%', fg: '32 60% 10%' },
  },
  black: {
    label: 'Black',
    // Monochrome accent — inverts per theme so the fill stays visible and AA-max:
    // near-black fill + white text in light, near-white fill + near-black text in dark.
    light: { primary: '0 0% 9%', fg: '0 0% 100%' },
    dark: { primary: '0 0% 98%', fg: '0 0% 9%' },
  },
} as const satisfies Record<string, AccentPresetDef>

export type AccentPreset = keyof typeof ACCENT_PRESETS
export const DEFAULT_ACCENT: AccentPreset = 'blue'
export const ACCENT_ORDER = Object.keys(ACCENT_PRESETS) as AccentPreset[]

/** Apply an accent preset to the document root for the current theme (idempotent). */
export function applyAccent(root: HTMLElement, preset: AccentPreset, isDark: boolean) {
  const def = ACCENT_PRESETS[preset] ?? ACCENT_PRESETS[DEFAULT_ACCENT]
  const v = isDark ? def.dark : def.light
  // Tokens are full color VALUES now (oklch palette, v4 `@theme inline`), no
  // longer raw channels fed through `hsl(var(--*))`. Emit the preset's HSL
  // channels wrapped as a complete `hsl(...)` color so alpha modifiers
  // (e.g. `ring/50`) resolve via color-mix like every other token.
  root.style.setProperty('--primary', `hsl(${v.primary})`)
  root.style.setProperty('--primary-foreground', `hsl(${v.fg})`)
  root.style.setProperty('--ring', `hsl(${v.primary})`)
  root.style.setProperty('--sidebar-primary', `hsl(${v.primary})`)
  root.style.setProperty('--sidebar-primary-foreground', `hsl(${v.fg})`)
  root.style.setProperty('--sidebar-ring', `hsl(${v.primary})`)
}
