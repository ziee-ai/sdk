/**
 * Smoke test for @ziee/config: the pure configs resolve, the syncpack helper
 * composes, and a parameterized lint runs against an arbitrary `--root` dir
 * (the parameterization proof) — pass on clean input, fail on a violation.
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import os from 'node:os'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const PKG = path.resolve(HERE, '..')
const node = process.execPath

test('biome.base.json is valid JSON with the generic linter preset', () => {
  const j = JSON.parse(fs.readFileSync(path.join(PKG, 'biome.base.json'), 'utf-8'))
  assert.equal(j.linter.rules.recommended, false)
  assert.ok(j.formatter.enabled)
  // The app-specific antd noRestrictedImports must NOT be in the base preset.
  const str = JSON.stringify(j)
  assert.ok(!str.includes('noRestrictedImports'), 'base must not hard-code app import bans')
})

test('tsconfig.base.json exposes strict generic compilerOptions (no app paths)', () => {
  const raw = fs.readFileSync(path.join(PKG, 'tsconfig.base.json'), 'utf-8')
  assert.ok(raw.includes('"strict": true'))
  assert.ok(!raw.includes('"paths"'), 'base must not hard-code app path mappings')
})

test('defineSyncpack composes semver + version groups (catch-all last)', async () => {
  const { defineSyncpack, semverGroups } = await import(path.join(PKG, 'syncpack.base.mjs'))
  const cfg = defineSyncpack({ source: ['package.json'], versionGroups: [{ label: 'x', dependencies: ['a'], packages: ['**'] }] })
  assert.equal(cfg.semverGroups.length, 2)
  assert.equal(semverGroups[0].dependencies[0], 'typescript')
  assert.equal(cfg.versionGroups.at(-1).policy, 'sameRange')
  assert.equal(cfg.versionGroups.length, 2)
})

test('a parameterized lint scans an arbitrary --root: clean passes, violation fails', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ziee-config-lint-'))
  const colors = path.join(PKG, 'src/lint/hardcoded-colors.mjs')
  // clean file — semantic token class only
  fs.writeFileSync(path.join(dir, 'ok.tsx'), `export const A = () => <div className="bg-primary" />\n`)
  let r = spawnSync(node, [colors, `--root=${dir}`], { encoding: 'utf-8' })
  assert.equal(r.status, 0, r.stdout + r.stderr)
  // violation — raw Tailwind hue
  fs.writeFileSync(path.join(dir, 'bad.tsx'), `export const B = () => <div className="bg-blue-500" />\n`)
  r = spawnSync(node, [colors, `--root=${dir}`], { encoding: 'utf-8' })
  assert.equal(r.status, 1)
  assert.ok((r.stdout + r.stderr).includes('bg-blue-500'))
  fs.rmSync(dir, { recursive: true, force: true })
})

// ── design-spec: an app that LAYERS its token sheet over a package's ───────────
//
// The gate assumed ziee's layout — ONE app CSS carrying `@theme inline` + `:root`
// + `.dark`. An app that imports a package token sheet and then overrides it
// (COMIZY's `@ziee/kit/styles/tokens.css` + `src/styles/nocturne.css`, whose dark
// block is the selector LIST `.dark, .nc-force-dark`) could not be pointed at its
// own tokens at all: a single `--css` saw only half the palette, and the sheet
// carrying the overrides has no `@theme inline`, so the script threw
// `Error: block not found: @theme inline` — an uncaught stack trace, not a
// message an app author could act on.
const designSpec = path.join(PKG, 'src', 'lint', 'design-spec.mjs')

/** A throwaway app root with the given files; returns its path. */
function cssFixture(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'design-spec-'))
  for (const [name, body] of Object.entries(files)) fs.writeFileSync(path.join(dir, name), body)
  return dir
}

const BASE_SHEET = `
/* a comment carrying braces { } and a semicolon ; and parens ( ) */
@theme inline {
  --color-primary: var(--primary);
  --radius-lg: var(--radius);
}
:root { --primary: #base; --radius: 10px; }
.dark { --primary: #basedark; }
`

test('design-spec: repeatable --css layers sheets in cascade order', () => {
  const dir = cssFixture({
    'base.css': BASE_SHEET,
    'over.css': ':root { --primary: #light; }\n.dark, .force-dark { --primary: #dark; }\n',
  })
  const r = spawnSync(node, [designSpec, '--css', 'base.css', '--css', 'over.css', '--out', 'D.md'], {
    cwd: dir,
    encoding: 'utf8',
  })
  assert.equal(r.status, 0, r.stderr)
  const md = fs.readFileSync(path.join(dir, 'D.md'), 'utf-8')
  // The LAYERED sheet wins in both themes — and the dark block was found even
  // though its selector is a comma LIST, not the bare `.dark` the scan matched.
  assert.match(md, /\| `--primary` \| `#light` \| `#dark` \|/)
  fs.rmSync(dir, { recursive: true, force: true })
})

test('design-spec: --dark-selector names an app-specific dark scope', () => {
  const dir = cssFixture({
    'base.css': BASE_SHEET,
    'over.css': ':root { --primary: #light; }\n[data-theme="night"] { --primary: #night; }\n',
  })
  const r = spawnSync(
    node,
    [designSpec, '--css', 'base.css', '--css', 'over.css', '--dark-selector', '[data-theme="night"]', '--out', 'D.md'],
    { cwd: dir, encoding: 'utf8' },
  )
  assert.equal(r.status, 0, r.stderr)
  assert.match(fs.readFileSync(path.join(dir, 'D.md'), 'utf-8'), /\| `--primary` \| `#light` \| `#night` \|/)
  fs.rmSync(dir, { recursive: true, force: true })
})

test('design-spec: a missing block exits 1 with an actionable message, not a stack trace', () => {
  const dir = cssFixture({ 'only-overrides.css': ':root { --primary: #light; }\n' })
  const r = spawnSync(node, [designSpec, '--css', 'only-overrides.css', '--out', 'D.md'], {
    cwd: dir,
    encoding: 'utf8',
  })
  assert.equal(r.status, 1)
  assert.match(r.stderr, /\[design-spec\] no `@theme inline` block/)
  assert.match(r.stderr, /repeatable --css/)
  assert.ok(!/^\s+at /m.test(r.stderr), `expected no stack trace, got:\n${r.stderr}`)
  fs.rmSync(dir, { recursive: true, force: true })
})

test('design-spec: a --css file that does not exist is NAMED, not an ENOENT throw', () => {
  const dir = cssFixture({})
  const r = spawnSync(node, [designSpec, '--css', 'nope.css', '--out', 'D.md'], {
    cwd: dir,
    encoding: 'utf8',
  })
  assert.equal(r.status, 1)
  assert.match(r.stderr, /\[design-spec\] --css .*nope\.css does not exist/)
  fs.rmSync(dir, { recursive: true, force: true })
})

test('design-spec: every :root block merges — not just the first', () => {
  // A real token sheet declares `:root` several times (theme-independent tokens,
  // then the palette, then the kit contract). Reading only the FIRST match took
  // one arbitrary slice and silently dropped the rest.
  const dir = cssFixture({
    'sheet.css': `
@theme inline { --color-primary: var(--primary); --color-border: var(--border); }
:root { --primary: #p; }
.dark { --primary: #pd; --border: #bd; }
:root { --border: #b; }
`,
  })
  const r = spawnSync(node, [designSpec, '--css', 'sheet.css', '--out', 'D.md'], {
    cwd: dir,
    encoding: 'utf8',
  })
  assert.equal(r.status, 0, r.stderr)
  const md = fs.readFileSync(path.join(dir, 'D.md'), 'utf-8')
  assert.match(md, /\| `--primary` \| `#p` \| `#pd` \|/)
  assert.match(md, /\| `--border` \| `#b` \| `#bd` \|/)
  fs.rmSync(dir, { recursive: true, force: true })
})
