import { useEffect } from 'react'
import { useTheme } from './useTheme'

/**
 * Flatten ANY CSS color (oklch/oklab/color()/hsl/rgb/named) to a plain `#rrggbb`
 * via a 1px canvas. iOS Safari's <meta theme-color> parser rejects the modern
 * color functions our tokens use (oklch), so the value MUST be hex or the
 * status/nav bars ignore it and fall back to Safari's default (near-black).
 */
export function toRgbHex(color: string): string | null {
  try {
    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 1
    const ctx = canvas.getContext('2d')
    if (!ctx) return null
    ctx.fillStyle = '#000'
    ctx.fillStyle = color // invalid colors leave the previous fillStyle intact
    ctx.fillRect(0, 0, 1, 1)
    const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data
    const hex = (n: number) => n.toString(16).padStart(2, '0')
    return `#${hex(r)}${hex(g)}${hex(b)}`
  } catch {
    return null
  }
}

/** Point <meta name="theme-color"> at the resolved value of a theme CSS var
 *  (e.g. `--card` / `--background`), flattened to hex for iOS Safari. */
export function setMetaThemeColorFromVar(cssVar: string): void {
  const val = getComputedStyle(document.documentElement)
    .getPropertyValue(cssVar)
    .trim()
  if (!val) return
  const hex = toRgbHex(val)
  if (!hex) return
  let meta = document.querySelector('meta[name="theme-color"]')
  if (!meta) {
    meta = document.createElement('meta')
    meta.setAttribute('name', 'theme-color')
    document.head.appendChild(meta)
  }
  meta.setAttribute('content', hex)
}

/**
 * Sync the iOS Safari status/nav bar color to a theme surface for as long as the
 * caller is mounted. Layouts pass the CSS var of the surface at the screen edges
 * — `--card` for the app shell, `--background` for the blank/login layout — so
 * the bars match the page. Re-runs on light/dark change.
 */
export function useMetaThemeColor(cssVar: string): void {
  const { isDarkMode } = useTheme()
  useEffect(() => {
    // Defer the CSS-var read to after the browser applies the new theme.
    // The `.dark`/`.light` class that drives `--card`/`--background` is toggled
    // by ThemeProvider's effect (the parent), which — because child effects run
    // before parent effects in a commit — fires AFTER this hook's effect. Reading
    // synchronously here would resolve the PREVIOUS theme's value (a one-step-
    // behind meta theme-color). A rAF read runs after the class + style recalc.
    const id = requestAnimationFrame(() => setMetaThemeColorFromVar(cssVar))
    return () => cancelAnimationFrame(id)
  }, [cssVar, isDarkMode])
}
