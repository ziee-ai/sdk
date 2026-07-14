/**
 * Drives the gallery's theme + accent from the URL (`?theme=&accent=`) so the
 * WHOLE gallery re-renders per combo — the mechanism Layer B uses to sweep the
 * matrix, and the manual reviewer uses via `/dev/gallery?theme=dark&accent=teal`.
 *
 * It writes through the app's REAL config store (via `GalleryConfig.setThemePref`
 * / `setAccentPref`), so the app `ThemeProvider` applies tokens/theme/accent
 * exactly as in production (no parallel theming path).
 */
import { useCallback, useEffect, useState } from 'react'
import { getGalleryConfig } from './config'
import {
  type GalleryDir,
  type GalleryParams,
  type GalleryTheme,
  parseGalleryParams,
} from './matrix'

function applyToStore(p: GalleryParams) {
  const cfg = getGalleryConfig()
  cfg.setThemePref(p.theme)
  cfg.setAccentPref(p.accent)
  // Direction isn't a config-store concern — apply it straight to the document
  // root (what the kit + Tailwind logical properties read).
  document.documentElement.dir = p.dir
}

export function useGalleryTheme() {
  const cfg = getGalleryConfig()
  const [params, setParams] = useState<GalleryParams>(() =>
    parseGalleryParams(window.location.search, cfg.accents, cfg.defaultAccent),
  )

  // Apply on mount + whenever the URL params change.
  useEffect(() => {
    applyToStore(params)
  }, [params.theme, params.accent, params.dir])

  // Keep in sync with back/forward navigation.
  useEffect(() => {
    const onPop = () =>
      setParams(parseGalleryParams(window.location.search, cfg.accents, cfg.defaultAccent))
    window.addEventListener('popstate', onPop)
    return () => window.removeEventListener('popstate', onPop)
  }, [])

  const setTheme = useCallback((theme: GalleryTheme) => {
    setParams(prev => {
      const next = { ...prev, theme }
      writeUrl(next)
      return next
    })
  }, [])

  const setAccent = useCallback((accent: string) => {
    setParams(prev => {
      const next = { ...prev, accent }
      writeUrl(next)
      return next
    })
  }, [])

  const setDir = useCallback((dir: GalleryDir) => {
    setParams(prev => {
      const next = { ...prev, dir }
      writeUrl(next)
      return next
    })
  }, [])

  return { params, setTheme, setAccent, setDir }
}

function writeUrl(p: GalleryParams) {
  const url = new URL(window.location.href)
  url.searchParams.set('theme', p.theme)
  url.searchParams.set('accent', p.accent)
  url.searchParams.set('dir', p.dir)
  window.history.replaceState(null, '', url.toString())
}
