import { test } from 'node:test'
import assert from 'node:assert/strict'
import { getRouterConfig, setRouterConfig } from './config.ts'

// FE-1 smoke — the router's injectable config DI (pure, no JSX / react-router).
// Proves createRouterModule's knobs (redirect paths, fallback, permission gate)
// flow through the module-level holder RouterComponent reads at render.

test('TEST-FE1-1: sensible defaults', () => {
  setRouterConfig({
    loginPath: '/auth',
    homePath: '/',
    fallback: null,
    permissionGate: null,
  })
  const c = getRouterConfig()
  assert.equal(c.loginPath, '/auth')
  assert.equal(c.homePath, '/')
  assert.equal(c.fallback, null)
  assert.equal(c.permissionGate, null)
})

test('TEST-FE1-2: setRouterConfig merges partial overrides', () => {
  setRouterConfig({ loginPath: '/login' })
  assert.equal(getRouterConfig().loginPath, '/login')
  // untouched keys preserved
  assert.equal(getRouterConfig().homePath, '/')
})

test('TEST-FE1-3: a permission gate can be injected and invoked', () => {
  const gate = ({ permission, children }: { permission: unknown; children: unknown }) =>
    permission === 'admin' ? children : 'DENIED'
  setRouterConfig({ permissionGate: gate })
  const g = getRouterConfig().permissionGate!
  assert.equal(g({ permission: 'admin', children: 'ok' }), 'ok')
  assert.equal(g({ permission: 'user', children: 'ok' }), 'DENIED')
})
