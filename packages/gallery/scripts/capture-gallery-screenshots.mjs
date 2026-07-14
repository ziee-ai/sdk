/**
 * Capture per-surface light+dark screenshots of the seeded gallery for a
 * vision-review pass. Dev-only tooling; output is git-ignored (env-specific).
 *
 * Enumerates ALL FOUR surface classes (pages, overlays, deep, seeded) through the
 * shared single-source `lib/gallery-surfaces.mjs`: browse-canvas pages are shot
 * in-place from the montage; the interaction-only classes (overlay/deep/seeded)
 * are driven one-per-load via `?surface=<slug>` and shot from their section —
 * so the vision pass never silently skips a whole surface class.
 *
 * Usage: node scripts/capture-gallery-screenshots.mjs [--out=DIR] [--url=URL]
 */
import { chromium } from '@playwright/test'
import fs from 'node:fs'
import path from 'node:path'
import { enumerateSurfaces, surfaceCount } from './lib/gallery-surfaces.mjs'

const outArg = process.argv.find(a => a.startsWith('--out='))
const urlArg = process.argv.find(a => a.startsWith('--url='))
const OUT = outArg ? outArg.slice(6) : '/tmp/gallery-shots'
const BASE = urlArg ? urlArg.slice(6) : 'http://localhost:1466/gallery.html'
const THEMES = ['light', 'dark']

const browser = await chromium.launch()

// Enumerate every surface class once (from the browse canvas).
const enumPage = await browser.newPage({ viewport: { width: 1280, height: 1000 } })
const classes = await enumerateSurfaces(enumPage, BASE)
await enumPage.close()
const interactionOnly = [...classes.overlays, ...classes.deep, ...classes.seeded]
console.log(
  `enumerated ${surfaceCount(classes)} surfaces (${classes.pages.length} pages, ` +
    `${interactionOnly.length} interaction-only)`,
)

const summary = {}
for (const theme of THEMES) {
  const dir = path.join(OUT, theme)
  fs.mkdirSync(dir, { recursive: true })

  // 1. Browse-canvas page surfaces — shot in-place from one montage load.
  const page = await browser.newPage({ viewport: { width: 1280, height: 1000 } })
  await page.goto(`${BASE}?theme=${theme}`, { waitUntil: 'networkidle' })
  await page.waitForTimeout(2500)
  let n = 0
  for (const id of classes.pages) {
    const s = page.locator(`[data-testid="gallery-page-${id}"]`).first()
    try {
      await s.scrollIntoViewIfNeeded()
      await s.screenshot({ path: path.join(dir, `${id}.png`) })
      n++
    } catch (e) {
      console.log(`  ! ${theme}/${id}: ${e.message.slice(0, 60)}`)
    }
  }
  // Also a full-page montage.
  await page.screenshot({ path: path.join(OUT, `_full-${theme}.png`), fullPage: true })
  await page.close()

  // 2. Interaction-only surfaces — one page-load each via `?surface=<slug>`.
  for (const slug of interactionOnly) {
    const p = await browser.newPage({ viewport: { width: 1280, height: 1000 } })
    const isOverlay = classes.overlays.includes(slug)
    const url = `${BASE}?surface=${slug}${isOverlay ? '&state=open' : ''}&theme=${theme}`
    try {
      await p.goto(url, { waitUntil: 'networkidle' })
      await p.waitForTimeout(2500)
      const s = p.locator(`[data-testid="gallery-page-${slug}"]`).first()
      await s.screenshot({ path: path.join(dir, `${slug}.png`) })
      n++
    } catch (e) {
      console.log(`  ! ${theme}/${slug}: ${e.message.slice(0, 60)}`)
    }
    await p.close()
  }
  summary[theme] = n
}
await browser.close()
console.log('captured:', JSON.stringify(summary), '→', OUT)
