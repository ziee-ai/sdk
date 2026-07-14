/**
 * The visual-test matrix — the theme × accent × direction × viewport
 * combinations the gallery is rendered under. Imported by both the gallery
 * (control bar + URL handling) and the Playwright Layer-B spec so the two never
 * drift. Accent values are app tokens, injected via `GalleryConfig.accents`.
 */

export type GalleryTheme = 'light' | 'dark'
export const GALLERY_THEMES: GalleryTheme[] = ['light', 'dark']

export interface GalleryViewport {
  name: 'mobile' | 'tablet' | 'desktop'
  width: number
  height: number
}

export const GALLERY_VIEWPORTS: GalleryViewport[] = [
  { name: 'mobile', width: 390, height: 844 },
  { name: 'tablet', width: 768, height: 1024 },
  { name: 'desktop', width: 1280, height: 900 },
]

export const GALLERY_PATH = '/dev/gallery'
/** Standalone (backend-free) Vite entry served in dev. */
export const GALLERY_STANDALONE_PATH = '/gallery.html'

export type GalleryDir = 'ltr' | 'rtl'
export const GALLERY_DIRS: GalleryDir[] = ['ltr', 'rtl']

export interface GalleryParams {
  theme: GalleryTheme
  /** The selected accent token (app-defined). */
  accent: string
  /** Text direction — RTL surfaces mirroring/alignment/overflow bugs cheaply. */
  dir: GalleryDir
}

/**
 * Parse `?theme=&accent=&dir=` into validated params (with defaults). The accent
 * allow-list + default are app tokens (from `GalleryConfig.accents`).
 */
export function parseGalleryParams(
  search: string,
  accents: readonly string[],
  defaultAccent: string,
): GalleryParams {
  const q = new URLSearchParams(search)
  const theme = q.get('theme')
  const accent = q.get('accent')
  const dir = q.get('dir')
  return {
    theme: theme === 'dark' ? 'dark' : 'light',
    accent: accents.includes(accent ?? '') ? (accent as string) : defaultAccent,
    dir: dir === 'rtl' ? 'rtl' : 'ltr',
  }
}
