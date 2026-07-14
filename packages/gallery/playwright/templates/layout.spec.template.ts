/**
 * TEMPLATE — Layer A: deterministic layout invariants + axe a11y over the gallery.
 *
 * No screenshot baseline: every assertion is a bug-by-definition (horizontal
 * overflow, axe violation). Copy into your visual test dir, then add app-specific
 * baselines (e.g. subtract documented pre-existing kit findings) as they accrue.
 *
 * Needs `@axe-core/playwright` (dev dep). Backend-free via the gallery Vite server.
 */
import AxeBuilder from '@axe-core/playwright'
import { expect, test } from '@playwright/test'
import { THEMES, listSurfaces, openGallery } from './_gallery'

test.describe('gallery layout invariants (Layer A)', () => {
  test('no horizontal overflow on the browse canvas', async ({ page }) => {
    await page.goto('/gallery.html', { waitUntil: 'networkidle' })
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
    )
    expect(overflow, 'page body must not scroll horizontally').toBe(false)
  })

  for (const theme of THEMES) {
    test(`axe: no critical/serious a11y violations (${theme})`, async ({ page }) => {
      await openGallery(page, { theme })
      const results = await new AxeBuilder({ page })
        .withTags(['wcag2a', 'wcag2aa'])
        .analyze()
      const blocking = results.violations.filter(v =>
        ['critical', 'serious'].includes(v.impact ?? ''),
      )
      expect(
        blocking,
        `axe violations (${theme}):\n${blocking.map(v => `  • ${v.id}: ${v.help}`).join('\n')}`,
      ).toHaveLength(0)
    })
  }

  test('every enumerated page renders without horizontal overflow', async ({ page }) => {
    const { pages } = await listSurfaces(page)
    for (const surface of pages) {
      await openGallery(page, { surface })
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
      )
      expect(overflow, `${surface} overflows horizontally`).toBe(false)
    }
  })
})
