/**
 * TEMPLATE — shared helpers for the generic gallery visual specs.
 *
 * Copy the `playwright/templates/*.template.ts` files into your app's visual test
 * dir (drop the `.template`), then extend them with app-specific baselines. They
 * are backend-free: they drive the standalone gallery entry and enumerate every
 * surface class from the runtime `window.__GALLERY_LIST_ALL_SURFACES__()` the
 * gallery exposes — so no spec hardcodes the app's surface list.
 */
import type { Page } from '@playwright/test'

/** Standalone gallery URL — override via GALLERY_URL if your app serves elsewhere. */
export const STANDALONE_PATH = process.env.GALLERY_URL || '/gallery.html'

export const THEMES = ['light', 'dark'] as const
export type Theme = (typeof THEMES)[number]

/** Layer B (pixel screenshots) needs blessed, env-specific baselines → opt-in. */
export const SNAPSHOTS_ENABLED = !!process.env.VISUAL_SNAPSHOTS

export const VIEWPORTS = [
  { name: 'mobile', width: 390, height: 844 },
  { name: 'tablet', width: 768, height: 1024 },
  { name: 'desktop', width: 1280, height: 900 },
] as const

export interface SurfaceClasses {
  pages: string[]
  overlays: string[]
  deep: string[]
  seeded: string[]
}

/** Build a `?surface=&state=&theme=` gallery URL. */
export function galleryUrl(opts: { surface?: string; state?: string; theme?: Theme } = {}): string {
  const q = new URLSearchParams()
  if (opts.surface) q.set('surface', opts.surface)
  if (opts.state) q.set('state', opts.state)
  if (opts.theme) q.set('theme', opts.theme)
  const s = q.toString()
  return s ? `${STANDALONE_PATH}?${s}` : STANDALONE_PATH
}

/** Navigate to the browse canvas + let it settle, then enumerate every surface class. */
export async function listSurfaces(page: Page): Promise<SurfaceClasses> {
  await page.goto(STANDALONE_PATH, { waitUntil: 'domcontentloaded' })
  await page.waitForTimeout(5000)
  return page.evaluate(() => {
    const w = window as unknown as {
      __GALLERY_LIST_ALL_SURFACES__?: () => Partial<SurfaceClasses>
    }
    const r = (typeof w.__GALLERY_LIST_ALL_SURFACES__ === 'function'
      ? w.__GALLERY_LIST_ALL_SURFACES__()
      : {}) as Partial<SurfaceClasses>
    return {
      pages: r.pages || [],
      overlays: r.overlays || [],
      deep: r.deep || [],
      seeded: r.seeded || [],
    }
  })
}

/** Open one surface + wait for it to mount/settle. */
export async function openGallery(
  page: Page,
  opts: { surface?: string; state?: string; theme?: Theme } = {},
): Promise<void> {
  await page.goto(galleryUrl(opts), { waitUntil: 'networkidle' })
  await page.waitForTimeout(1500)
}
