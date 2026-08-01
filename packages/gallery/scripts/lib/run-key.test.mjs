/**
 * Unit tests for the unified run-key (run node --test on this file).
 * Covers TESTS.md TEST-2/3/4/5/6/7/14.
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  fnv1a8,
  worktreeKey,
  worktreeRoot,
  portBase,
  PORT_FLOORS,
  isPortBindable,
  pickBindablePort,
  resolveGalleryPort,
  serverIsThisWorktree,
  sentinelPayload,
} from './run-key.mjs'
import { createServer } from 'node:net'

// TEST-3: CROSS-LANGUAGE parity — the SAME literals the Rust #[cfg(test)]
// `cross_language_parity_fixture` pins. A drift in either language fails one.
test('TEST-3 cross-language FNV parity fixture', () => {
  assert.equal(fnv1a8('/data/pbya/ziee/tmp/xwt-a'), '12080097')
  assert.equal(fnv1a8('/data/pbya/ziee/tmp/xwt-b'), '1208014a')
  // worktreeKey strips /src-app → same key from a manifest-style path.
  assert.equal(worktreeKey('/data/pbya/ziee/tmp/xwt-a/src-app/server'), '12080097')
  assert.equal(worktreeKey('/data/pbya/ziee/tmp/xwt-a'), '12080097')
})

// TEST-2: key shape + portBase math + pickBindablePort.
test('TEST-2 worktreeKey is 8 hex, portBase deterministic + in range', () => {
  const k = worktreeKey('/data/pbya/ziee/tmp/xwt-a')
  assert.match(k, /^[0-9a-f]{8}$/)
  const a = portBase(k, 20000, 200)
  const b = portBase(k, 20000, 200)
  assert.equal(a, b)
  assert.ok(a >= 20000 && a < 20200)
  // Matches the Rust port_base test value for the same key + range.
  assert.equal(portBase('12080097', 20000, 200), 20000 + (0x12080097 % 200))
  // Malformed key → floor, never NaN.
  assert.equal(portBase('zzzzzzzz', 9000, 200), 9000)
})

test('TEST-2 pickBindablePort returns a bindable port and skips a held one', async () => {
  // Hold a port, assert pickBindablePort steps past it.
  const held = await new Promise((res) => {
    const s = createServer()
    s.listen(0, '0.0.0.0', () => res(s))
  })
  const heldPort = held.address().port
  assert.equal(await isPortBindable(heldPort), false)
  const picked = await pickBindablePort(heldPort, 50)
  assert.notEqual(picked, heldPort)
  assert.equal(await isPortBindable(picked), true)
  held.close()
})

// TEST-4: no-foreign-reuse predicate.
test('TEST-4 serverIsThisWorktree only matches identical roots', () => {
  assert.equal(serverIsThisWorktree('/wt/a', '/wt/a'), true)
  assert.equal(serverIsThisWorktree('/wt/a', '/wt/b'), false)
  assert.equal(serverIsThisWorktree('', '/wt/a'), false)
  assert.equal(serverIsThisWorktree(null, '/wt/a'), false)
  assert.equal(serverIsThisWorktree(undefined, '/wt/a'), false)
})

// TEST-5: sentinel payload carries the worktree root.
test('TEST-5 sentinelPayload emits the worktree root', () => {
  const p = sentinelPayload(process.cwd())
  assert.equal(typeof p.worktreeRoot, 'string')
  assert.ok(p.worktreeRoot.length > 0)
  assert.equal(typeof p.pid, 'number')
})

// TEST-6: resolveGalleryPort precedence env > cfg > key-derived; null cfg → derived.
test('TEST-6 resolveGalleryPort precedence + null-cfg derives (never NaN)', () => {
  // explicit env wins.
  assert.equal(resolveGalleryPort({ env: '31234', cfgPort: 1420 }), 31234)
  // no env, explicit cfg wins.
  assert.equal(resolveGalleryPort({ env: undefined, cfgPort: 1420 }), 1420)
  // no env, null cfg → key-derived within the web-gallery range (never NaN).
  const derived = resolveGalleryPort({ env: undefined, cfgPort: null, which: 'webGallery' })
  assert.ok(Number.isFinite(derived))
  assert.ok(
    derived >= PORT_FLOORS.webGallery.floor &&
      derived < PORT_FLOORS.webGallery.floor + PORT_FLOORS.webGallery.span,
  )
})

// TEST-7: gate-ui and its runtime-health child agree on the port (same inputs → same port).
test('TEST-7 resolveGalleryPort is stable for the same env+cwd', () => {
  const a = resolveGalleryPort({ env: undefined, cfgPort: null, which: 'webGallery' })
  const b = resolveGalleryPort({ env: undefined, cfgPort: null, which: 'webGallery' })
  assert.equal(a, b)
  // an explicit GALLERY_PORT is honored identically by both.
  assert.equal(
    resolveGalleryPort({ env: '41999', cfgPort: null }),
    resolveGalleryPort({ env: '41999', cfgPort: 1420 }),
  )
})

// TEST-14: desktop floor is disjoint from web floor (no cross-workspace 1420 collision).
test('TEST-14 desktop gallery port range is disjoint from web', () => {
  const web = PORT_FLOORS.webGallery
  const desk = PORT_FLOORS.desktopGallery
  const webHi = web.floor + web.span
  const deskHi = desk.floor + desk.span
  assert.ok(webHi <= desk.floor || deskHi <= web.floor, 'web and desktop gallery ranges must not overlap')
  // desktop-e2e backend is OFF the web-e2e 9100 overlap.
  assert.notEqual(PORT_FLOORS.desktopE2eBackend.floor, PORT_FLOORS.webE2eBackend.floor)
  assert.ok(
    PORT_FLOORS.desktopE2eBackend.floor >= PORT_FLOORS.webE2eBackend.floor + PORT_FLOORS.webE2eBackend.span,
  )
})
