/**
 * TESTS for the dev-gallery seed completeness gate (ITEM-7, ITEM-14, ITEM-17).
 * Run: node --test scripts/gen-gallery-seed-registry.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  computeSeedDrift,
  hasUserSurface,
  parseSeedExceptions,
} from './gen-gallery-seed-registry.mjs'

// ── TEST-1: computeSeedDrift — MISSING detection ─────────────────────────────
test('TEST-1: surface-bearing module with no gallery.tsx is MISSING', () => {
  const modules = [
    { name: 'foo', hasSurface: true, hasSeed: false },
    { name: 'bar', hasSurface: true, hasSeed: true },
    { name: 'infra', hasSurface: false, hasSeed: false },
  ]
  const { missing } = computeSeedDrift(modules, new Set())
  assert.deepEqual(missing, ['foo'])
})

test('TEST-1: all surface modules seeded → empty MISSING', () => {
  const modules = [
    { name: 'foo', hasSurface: true, hasSeed: true },
    { name: 'bar', hasSurface: true, hasSeed: true },
    { name: 'infra', hasSurface: false, hasSeed: false },
  ]
  const { missing } = computeSeedDrift(modules, new Set())
  assert.deepEqual(missing, [])
})

// ── TEST-15: crawlOnly file still counts as HAS_SEED (caller passes hasSeed) ──
test('TEST-15: a module marked hasSeed (crawlOnly gallery.tsx) is not MISSING', () => {
  const modules = [{ name: 'summarization', hasSurface: true, hasSeed: true }]
  const { missing } = computeSeedDrift(modules, new Set())
  assert.deepEqual(missing, [])
})

// ── TEST-2: STALE_ALLOWLIST — GC of a rotten excuse list ─────────────────────
test('TEST-2: allow-listed module that now HAS seed is STALE', () => {
  const modules = [{ name: 'settings', hasSurface: true, hasSeed: true }]
  const { missing, stale } = computeSeedDrift(modules, new Set(['settings']))
  assert.deepEqual(missing, [])
  assert.deepEqual(stale, ['settings'])
})

test('TEST-2: allow-listed module with NO surface is STALE (allowlist pointless)', () => {
  const modules = [{ name: 'config-client', hasSurface: false, hasSeed: false }]
  const { stale } = computeSeedDrift(modules, new Set(['config-client']))
  assert.deepEqual(stale, ['config-client'])
})

test('TEST-2: a legit allow-list entry (surface, unseeded, listed) is NOT stale and NOT missing', () => {
  const modules = [{ name: 'settings', hasSurface: true, hasSeed: false }]
  const { missing, stale } = computeSeedDrift(modules, new Set(['settings']))
  assert.deepEqual(missing, [])
  assert.deepEqual(stale, [])
})

// ── TEST-3: hasUserSurface — route + slot detection ──────────────────────────
test('TEST-3: a non-skip route path is a user surface', () => {
  assert.equal(hasUserSurface(`routes: [{ path: '/settings/js-tool', element: X }]`), true)
})

test('TEST-3: only skip-path routes → NOT a user surface', () => {
  assert.equal(
    hasUserSurface(`routes: import.meta.env.DEV ? [{ path: '/dev/gallery' }] : []`),
    false,
  )
  assert.equal(hasUserSurface(`routes: [{ path: '/' }, { path: '/auth/callback' }]`), false)
})

test('TEST-3: a user-facing slot registration is a surface even with no route', () => {
  assert.equal(hasUserSurface(`slots: { sidebarBottom: [bellWidget] }`), true)
  assert.equal(hasUserSurface(`slots: { settingsAdminPages: [page] }`), true)
})

test('TEST-3: an infra module (no route, no slot) is NOT a surface', () => {
  assert.equal(hasUserSurface(`export default createModule({ metadata: { name: 'router' } })`), false)
})

test('TEST-3: a COMMENTED-OUT route is NOT a surface (comments stripped)', () => {
  assert.equal(hasUserSurface(`  //   path: '/settings/window',\n  routes: []`), false)
  assert.equal(hasUserSurface(`/* path: '/x' */ routes: []`), false)
})

// ── TEST-4: parseSeedExceptions — reason + sign-off required ─────────────────
test('TEST-4: a well-formed NO-SEED line is parsed', () => {
  const set = parseSeedExceptions(
    '- NO-SEED: settings — redirect shell, hosts settings*Pages slots [approved: user 2026-07-13]',
  )
  assert.ok(set.has('settings'))
})

test('TEST-4: a NO-SEED line missing the [approved:] sign-off is rejected', () => {
  const set = parseSeedExceptions('- NO-SEED: settings — redirect shell')
  assert.equal(set.size, 0)
})

test('TEST-4: a NO-SEED line missing the reason is rejected', () => {
  const set = parseSeedExceptions('- NO-SEED: settings [approved: user 2026-07-13]')
  assert.equal(set.size, 0)
})
