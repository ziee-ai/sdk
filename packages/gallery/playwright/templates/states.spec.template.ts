/**
 * TEMPLATE — render every page across its states (loaded / empty / error) per
 * theme and assert NO console error / uncaught exception / ErrorBoundary crash.
 * Empty + error are where most bugs hide — this is the finding pass.
 *
 * Backend-free via the gallery Vite server. Copy into your visual test dir.
 */
import { expect, test } from '@playwright/test'
import { THEMES, listSurfaces, openGallery } from './_gallery'

const STATES = ['loaded', 'empty', 'error'] as const

test('every page × state × theme renders crash-free', async ({ page }) => {
  const crashes: string[] = []
  page.on('console', m => {
    if (m.type() === 'error' && /\[AppErrorBoundary|Uncaught/.test(m.text()))
      crashes.push(`console: ${m.text().slice(0, 140)}`)
  })
  page.on('pageerror', e => crashes.push(`pageerror: ${e.message.slice(0, 140)}`))

  const { pages } = await listSurfaces(page)
  for (const surface of pages) {
    for (const state of STATES) {
      for (const theme of THEMES) {
        crashes.length = 0
        await openGallery(page, { surface, state, theme })
        expect(
          crashes,
          `${surface} [${state}/${theme}] crashed:\n${crashes.join('\n')}`,
        ).toHaveLength(0)
      }
    }
  }
})
