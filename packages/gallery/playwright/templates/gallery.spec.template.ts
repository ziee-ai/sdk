/**
 * TEMPLATE — Layer B: pixel-regression screenshots of every surface × theme.
 *
 * OPT-IN: pixel baselines are environment-specific, so this runs only with
 * VISUAL_SNAPSHOTS=1 (e.g. when blessing in a pinned container). Bless with
 * `--update-snapshots`. Copy into your visual test dir.
 *
 * Backend-free via the gallery Vite server.
 */
import { expect, test } from '@playwright/test'
import { SNAPSHOTS_ENABLED, THEMES, listSurfaces, openGallery } from './_gallery'

test.skip(!SNAPSHOTS_ENABLED, 'Layer B is opt-in — set VISUAL_SNAPSHOTS=1 to run')

test('surface screenshots match blessed baselines', async ({ page }) => {
  const { pages } = await listSurfaces(page)
  for (const surface of pages) {
    for (const theme of THEMES) {
      await openGallery(page, { surface, theme })
      const section = page.getByTestId(`gallery-page-${surface}`).first()
      await expect(section).toHaveScreenshot(`${surface.replace(/\//g, '__')}__${theme}.png`)
    }
  }
})
