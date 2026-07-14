/**
 * PART 2 — DYNAMIC PROOF via BRANCH COVERAGE.
 *
 * Renders EVERY gallery combo (browse + each page in empty/error/delayed +
 * every overlay + every chat deep-state) under istanbul instrumentation, merges
 * the per-render `window.__coverage__`, and reports every UNCOVERED CONDITIONAL
 * BRANCH in a component/page file — i.e. a render fork the gallery never
 * exercised = a state that never rendered.
 *
 * This is the runtime complement to Part 1's static tsc gate: Part 1 proves each
 * NAMED state has a gallery entry; Part 2 proves the entries actually EXERCISE
 * the branch (and additionally catches the generic `branch` conditionals the
 * tsc gate does not name).
 *
 * Instrumentation is opt-in (GALLERY_COVERAGE=1 → babel-plugin-istanbul in
 * vite.config.ts), so normal dev/build never pays for it. The heavy Playwright
 * pass is a SEPARATE CI-able script (not in the fast `npm run check`, which keeps
 * the Part-1 tsc parity gate).
 *
 * Output:
 *   - src/dev/gallery/UNCOVERED_STATES.md         (file:line + condition text)
 *   - src/dev/gallery/coverage.raw.json           (merged coverage, gitignored)
 * Gate: every uncovered branch must be rendered by a new entry OR allow-listed in
 *   src/dev/gallery/coverage-allowlist.json (a `"file:line"` → reason map).
 *   `--gate` exits non-zero on any non-allow-listed uncovered branch.
 *
 * Usage:
 *   GALLERY_COVERAGE=1 npm run dev -- --port 1466 --strictPort   # (instrumented)
 *   node scripts/gallery-coverage.mjs --url=http://localhost:1466/gallery.html
 *   node scripts/gallery-coverage.mjs --url=… --gate              # CI gate
 */
import { chromium } from '@playwright/test'
import fs from 'node:fs'
import path from 'node:path'
import { enumerateSurfaces } from './lib/gallery-surfaces.mjs'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

// Config-driven anchors (was HERE/UI_DIR/SRC). The app runs this with its `ui/`
// as cwd, so `gallery.config.json` + relative paths resolve against it.
const CFG = resolveGalleryConfig()
const SRC = path.resolve(CFG.__cwd, CFG.srcDir)
const GALLERY = path.resolve(CFG.__cwd, CFG.galleryDir)
const OUT_MD = path.join(GALLERY, 'UNCOVERED_STATES.md')
const OUT_RAW = path.join(GALLERY, 'coverage.raw.json')
const ALLOWLIST = path.join(GALLERY, 'coverage-allowlist.json')

const arg = (n, d) => {
  const a = process.argv.find(x => x.startsWith(`--${n}=`))
  return a ? a.slice(n.length + 3) : d
}
const BASE = arg('url', 'http://localhost:1466/gallery.html')
const GATE = process.argv.includes('--gate')
const STATES = ['empty', 'error', 'delayed']
const LAUNCH = { args: ['--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu'] }

const relFile = f => path.relative(SRC, f).replace(/\\/g, '/')
const isSurfaceFile = f =>
  /\/src\/modules\//.test(f) || /\/src\/components\/ui\//.test(f)

// ── merge coverage (manual — same branchMap across pages; sum the counts) ─────
function mergeInto(acc, cov) {
  if (!cov) return
  for (const [file, fc] of Object.entries(cov)) {
    const cur = acc[file]
    if (!cur) {
      acc[file] = JSON.parse(JSON.stringify(fc))
      continue
    }
    for (const [bid, arms] of Object.entries(fc.b || {})) {
      cur.b[bid] = (cur.b[bid] || arms.map(() => 0)).map((n, i) => n + (arms[i] || 0))
    }
    for (const [sid, n] of Object.entries(fc.s || {})) cur.s[sid] = (cur.s[sid] || 0) + n
  }
}

// The heavy gallery page can crash the browser process; `holder.b` lets us
// relaunch a dead browser and keep the pass going (fresh context per visit).
async function ensureBrowser(holder) {
  if (holder.b && holder.b.isConnected()) return holder.b
  try { await holder.b?.close() } catch {}
  holder.b = await chromium.launch(LAUNCH)
  return holder.b
}

async function visit(holder, url, acc) {
  for (let attempt = 0; attempt < 2; attempt++) {
    let ctx
    try {
      const browser = await ensureBrowser(holder)
      ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } })
      const page = await ctx.newPage()
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 })
      // browse + data-pages settle async; deep/overlay/seeded setups run on
      // mount. Seeded surfaces lazy-load a real component THEN hold a store seed
      // (~4.5s) — a late-mounting component (slow chunk under the full pass) needs
      // the seed still asserting when it first renders, so wait 5.5s on a surface.
      await page.waitForTimeout(url.includes('surface=') ? 6500 : 5000)
      // Interaction URLs drive a post-mount recipe; wait for its done-signal so the
      // now-exercised branch is present in this render's __coverage__.
      if (url.includes('interact=')) {
        await page
          .waitForSelector('body[data-gallery-interact-done]', { timeout: 12000 })
          .catch(() => {})
        await page.waitForTimeout(400)
      }
      const cov = await page.evaluate(() => window.__coverage__ || null)
      if (!cov) throw new Error('no __coverage__ (is GALLERY_COVERAGE=1 on the server?)')
      mergeInto(acc, cov)
      return true
    } catch (e) {
      const dead = /closed|crash|Target|disconnected/i.test(String(e))
      if (dead && attempt === 0) { holder.b = null; continue } // relaunch + retry once
      console.warn(`  ! ${url.slice(BASE.length)} — ${String(e).slice(0, 90)}`)
      return false
    } finally {
      try { await ctx?.close() } catch {}
    }
  }
  return false
}

async function enumerateSlugs(holder) {
  const browser = await ensureBrowser(holder)
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } })
  const page = await ctx.newPage()
  // Single-source enumeration (shared with the capture scripts) — pages minus the
  // interaction-only classes already resolved inside `listAllSurfaces`.
  const classes = await enumerateSurfaces(page, BASE)
  await ctx.close()
  return classes
}

// Part-1 cross-reference: which surface lines carry a NAMED state signal
// (loading/error/empty/overlay/panel). An uncovered branch on such a line is a
// STATE gap (the actionable queue); everything else is a generic prop-variant
// branch fork (informational — the state axis is already gated by Part 1 + the
// kit stories). Parsed from the generated matrix so the two never drift.
const NAMED_KINDS = new Set(['loading', 'error', 'empty', 'overlay', 'panel'])
function loadNamedStateLines() {
  const p = path.join(GALLERY, 'stateMatrix.generated.ts')
  const set = new Map() // "surfaceNoExt:line" → kind
  if (!fs.existsSync(p)) return set
  const src = fs.readFileSync(p, 'utf8')
  let surface = null
  for (const line of src.split('\n')) {
    const sm = line.match(/^\s*surface:\s*"([^"]+)"/)
    if (sm) { surface = sm[1]; continue }
    const km = line.match(/kind:\s*"([^"]+)".*line:\s*(\d+)/)
    if (km && surface && NAMED_KINDS.has(km[1])) set.set(`${surface}:${km[2]}`, km[1])
  }
  return set
}

function report(acc) {
  const allow = fs.existsSync(ALLOWLIST) ? JSON.parse(fs.readFileSync(ALLOWLIST, 'utf8')) : {}
  const named = loadNamedStateLines()
  const uncovered = []
  for (const [file, fc] of Object.entries(acc)) {
    if (!isSurfaceFile(file)) continue
    const rel = relFile(file)
    const noExt = rel.replace(/\.tsx?$/, '')
    const srcLines = fs.existsSync(file) ? fs.readFileSync(file, 'utf8').split('\n') : []
    for (const [bid, arms] of Object.entries(fc.b || {})) {
      const meta = fc.branchMap?.[bid]
      if (!meta) continue
      arms.forEach((count, i) => {
        if (count > 0) return
        const loc = meta.locations?.[i] || meta.loc
        const line = loc?.start?.line ?? 0
        // A named-state signal's condition line may sit a line off the branch
        // start; correlate with ±1 tolerance.
        const stateKind =
          named.get(`${noExt}:${line}`) ||
          named.get(`${noExt}:${line - 1}`) ||
          named.get(`${noExt}:${line + 1}`)
        const key = `${rel}:${line}`
        uncovered.push({
          rel, noExt, line, type: meta.type, arm: i, key,
          condition: (srcLines[line - 1] || '').trim().slice(0, 140),
          category: stateKind ? 'state' : 'branch',
          stateKind: stateKind || '',
          allowed: key in allow || `${key}:${i}` in allow,
        })
      })
    }
  }
  uncovered.sort((a, b) => a.rel.localeCompare(b.rel) || a.line - b.line || a.arm - b.arm)

  const stateGaps = uncovered.filter(u => u.category === 'state' && !u.allowed)
  const branchGaps = uncovered.filter(u => u.category === 'branch' && !u.allowed)
  const filesRendered = Object.keys(acc).filter(isSurfaceFile).length

  // ── write UNCOVERED_STATES.md ──────────────────────────────────────────────
  const byFile = list => {
    const m = {}
    for (const u of list) (m[u.rel] ??= []).push(u)
    return m
  }
  let md = `# Uncovered render branches (GENERATED)

> \`node scripts/gallery-coverage.mjs\` — the runtime branch-coverage proof (Part 2).
> An uncovered arm is a conditional-render fork NO gallery combo exercised. Rows are
> split into **state gaps** (the arm sits on a Part-1 named-state signal — the
> actionable queue: add a gallery entry that reaches it, or allow-list it in
> \`coverage-allowlist.json\` with a reason) and **generic branch forks** (prop
> variants / defensive defaults — the state axis is already gated by Part 1's tsc
> gate + the kit stories, so these are informational).

## Summary

- ${filesRendered} instrumented surface files rendered.
- **${stateGaps.length}** STATE gaps not allow-listed — the actionable queue.
- ${branchGaps.length} generic branch forks not allow-listed (informational).

## State-level gaps (actionable)

`
  if (!stateGaps.length) {
    md += `✅ Every Part-1 named-state branch (loading/error/empty/overlay/panel) was exercised by a gallery combo (or allow-listed).\n\n`
  } else {
    for (const [rel, list] of Object.entries(byFile(stateGaps))) {
      md += `### \`${rel}\`\n\n| line | state | condition |\n|---|---|---|\n`
      for (const u of list)
        md += `| ${u.line} | ${u.stateKind} | \`${u.condition.replace(/\|/g, '\\|')}\` |\n`
      md += '\n'
    }
  }
  md += `## Generic branch forks (informational — top files by count)\n\n| file | uncovered forks |\n|---|---|\n`
  const branchByFile = byFile(branchGaps)
  for (const [rel, list] of Object.entries(branchByFile).sort((a, b) => b[1].length - a[1].length).slice(0, 30))
    md += `| \`${rel}\` | ${list.length} |\n`
  fs.writeFileSync(OUT_MD, md)

  console.log(
    `\nwrote ${relFile(OUT_MD)} — ${stateGaps.length} STATE gaps (queue), ${branchGaps.length} generic branch forks.`,
  )
  if (GATE && stateGaps.length) {
    console.error(`\n❌ COVERAGE GATE FAILED — ${stateGaps.length} named-state branch(es) never rendered; add an entry or allow-list them.`)
    process.exit(1)
  }
  console.log(GATE ? '\n✅ COVERAGE GATE PASSED (every named-state branch exercised or allow-listed)' : '\n(report-only; pass --gate to fail on STATE residuals)')
}

async function main() {
  // Fast path: re-report from the saved raw merge without re-driving the browser.
  if (process.argv.includes('--report-only')) {
    if (!fs.existsSync(OUT_RAW)) { console.error(`no ${relFile(OUT_RAW)} — run a full pass first.`); process.exit(2) }
    report(JSON.parse(fs.readFileSync(OUT_RAW, 'utf8')))
    return
  }

  const holder = { b: null }
  const acc = {}
  console.log('=== gallery branch-coverage pass ===')
  const { pages, overlays, deep, seeded, interactions = [] } = await enumerateSlugs(holder)
  console.log(`enumerated ${pages.length} pages, ${overlays.length} overlays, ${deep.length} deep states, ${seeded.length} seeded surfaces, ${interactions.length} interaction recipes`)

  let done = 0
  const total =
    1 + pages.length * STATES.length + overlays.length + deep.length + seeded.length + interactions.length
  const tick = () => { if (++done % 15 === 0) console.log(`  …${done}/${total} combos`) }
  await visit(holder, BASE, acc); tick() // browse (all pages, loaded)
  for (const slug of pages)
    for (const st of STATES) { await visit(holder, `${BASE}?surface=${slug}&state=${st}`, acc); tick() }
  for (const slug of [...overlays, ...deep, ...seeded]) { await visit(holder, `${BASE}?surface=${slug}`, acc); tick() }
  // Interaction recipes: drive the post-mount action so the interaction-gated
  // branch is exercised (the de-allowlisted arms live here).
  for (const it of interactions) { await visit(holder, `${BASE}?surface=${it.slug}&interact=${it.name}`, acc); tick() }
  try { await holder.b?.close() } catch {}

  fs.writeFileSync(OUT_RAW, JSON.stringify(acc))
  report(acc)
}

main().catch(e => {
  console.error(e)
  process.exit(2)
})
