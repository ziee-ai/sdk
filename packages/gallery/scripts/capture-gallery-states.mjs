/**
 * Multi-state screenshots + empty/error bug collector.
 *
 * For every gallery surface — ACROSS ALL FOUR CLASSES (pages, overlays, deep
 * conversation states, seeded components) — captures a screenshot per theme and
 * records any console error / ErrorBoundary crash. Pages get the data-state set
 * (loaded / empty / error, driven by `?surface=&state=`); the interaction-only
 * classes render once via `?surface=<slug>`. Empty + error are where most bugs
 * hide — this is the finding pass.
 *
 * Enumeration goes through the shared `lib/gallery-surfaces.mjs` (single source),
 * so a capture pass can never again silently skip a whole surface class.
 *
 * Screenshot id: surface__state__theme.png
 * Usage: node scripts/capture-gallery-states.mjs [--out=DIR] [--url=BASE] [--states=loaded,empty,error]
 */
import { chromium } from '@playwright/test'
import fs from 'node:fs'
import path from 'node:path'
import { enumerateSurfaces, captureCells, cellUrl, surfaceCount } from './lib/gallery-surfaces.mjs'

const arg = (n, d) => (process.argv.find(a => a.startsWith(`--${n}=`)) || `--${n}=${d}`).split('=').slice(1).join('=')
const OUT = arg('out', '/tmp/gallery-states')
const BASE = arg('url', 'http://localhost:1466/gallery.html')
const STATES = arg('states', 'loaded,empty,error').split(',')
const THEMES = ['light', 'dark']

const browser = await chromium.launch()

// 1. Enumerate EVERY surface class from the single source (browse canvas).
const enumPage = await browser.newPage({ viewport: { width: 1280, height: 900 } })
const classes = await enumerateSurfaces(enumPage, BASE)
await enumPage.close()
const cells = captureCells(classes, { states: STATES })
console.log(
  `enumerated ${surfaceCount(classes)} surfaces (${classes.pages.length} pages, ` +
    `${classes.overlays.length} overlays, ${classes.deep.length} deep, ${classes.seeded.length} seeded) → ${cells.length} cells × ${THEMES.length} themes`,
)

// 2. Capture each (cell, theme) + collect crashes.
const findings = []
let shots = 0
for (const cell of cells) {
  const { slug, cls, state } = cell
  for (const theme of THEMES) {
    const p = await browser.newPage({ viewport: { width: 1280, height: 900 } })
    const errs = new Set()
    p.on('console', m => {
      if (m.type() !== 'error') return
      const t = m.text()
      if (/\[AppErrorBoundary \[(page|overlay|deep|seeded)-/.test(t)) errs.add('CRASH: ' + t.replace(/\s+/g, ' ').slice(0, 120))
    })
    p.on('pageerror', e => errs.add('CRASH: ' + e.message.slice(0, 120)))
    const url = cellUrl(BASE, cell, { theme })
    try {
      await p.goto(url, { waitUntil: 'networkidle' })
      if (cls === 'interaction') {
        // The frame drives the recipe after mount + a settle window, then stamps
        // <body data-gallery-interact-done>. Wait for that signal (bounded), so the
        // shot captures the DRIVEN state, not the pre-interaction surface.
        await p
          .waitForSelector('body[data-gallery-interact-done]', { timeout: 12000 })
          .catch(() => {})
        await p.waitForTimeout(400)
      } else {
        // Seeded/deep surfaces run a mount-time store seed (~a few s); pages settle faster.
        await p.waitForTimeout(cls === 'page' ? (state === 'error' ? 1500 : 1200) : 2500)
      }
      const dir = path.join(OUT, theme)
      fs.mkdirSync(dir, { recursive: true })
      // Interaction states can portal outside the frame (drawers/dialogs) — shoot
      // the full page; the base classes shoot their bounded frame section.
      if (cls === 'interaction') {
        await p.screenshot({ path: path.join(dir, `${slug}__${state}.png`), fullPage: true })
      } else {
        const sec = p.locator(`[data-testid="gallery-page-${slug}"]`)
        await sec.screenshot({ path: path.join(dir, `${slug}__${state}.png`) })
      }
      shots++
      // Only report crashes; a page rendering empty/error UI cleanly is fine.
      if (theme === 'light') {
        const crashes = [...errs].filter(e => e.startsWith('CRASH'))
        if (crashes.length) findings.push({ slug, cls, state, crashes })
      }
    } catch (e) {
      findings.push({ slug, cls, state, crashes: ['NAV: ' + e.message.slice(0, 80)] })
    }
    await p.close()
  }
}
await browser.close()

console.log(`captured ${shots} state screenshots → ${OUT}`)
console.log(`\n=== crashes across all surface classes (${findings.length}) ===`)
for (const f of findings) console.log(`  ${f.slug} [${f.cls}/${f.state}]: ${f.crashes.join(' | ')}`)
fs.writeFileSync(path.join(OUT, 'findings.json'), JSON.stringify({ surfaceCount: surfaceCount(classes), classes, findings }, null, 2))
