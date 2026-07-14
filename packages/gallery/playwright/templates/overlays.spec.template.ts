/**
 * TEMPLATE — open every overlay-bearing surface (dialogs / drawers / sheets /
 * popovers) in the gallery and assert it renders open without a console error or
 * crash. The overlay analog of the states pass — overlays are the surfaces the
 * seeded gallery historically never rendered open.
 *
 * Backend-free via the gallery Vite server. Copy into your visual test dir.
 */
import { expect, test } from '@playwright/test'
import { THEMES, listSurfaces, openGallery } from './_gallery'

test('every overlay opens crash-free', async ({ page }) => {
  const crashes: string[] = []
  page.on('console', m => {
    if (m.type() === 'error' && /\[AppErrorBoundary|Uncaught/.test(m.text()))
      crashes.push(`console: ${m.text().slice(0, 140)}`)
  })
  page.on('pageerror', e => crashes.push(`pageerror: ${e.message.slice(0, 140)}`))

  const { overlays } = await listSurfaces(page)
  for (const surface of overlays) {
    for (const theme of THEMES) {
      crashes.length = 0
      await openGallery(page, { surface, state: 'open', theme })
      expect(
        crashes,
        `overlay ${surface} [${theme}] crashed:\n${crashes.join('\n')}`,
      ).toHaveLength(0)
    }
  }
})
