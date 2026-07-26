/**
 * Unit-test stub for the `@ziee/framework/permissions` BARREL.
 *
 * The barrel re-exports `Can.tsx` (React/JSX), which node's type-STRIPPING
 * runtime — what `node --test` uses — cannot parse, so any spec that transitively
 * imports the barrel dies with ERR_UNKNOWN_FILE_EXTENSION before a single
 * assertion runs. Same rationale (and same mechanism) as the existing
 * `@/core/{module-system,events}` stubs registered in
 * `src-app/ui/scripts/node-test-hooks.mjs`: stub the browser/JSX-coupled
 * boundary, keep everything else real.
 *
 * `hasPermissionNow` is the only member the stubbed boundary is asked for in
 * unit specs; it answers `true` so a permission-gated action under test runs its
 * real body. A spec that needs to exercise the DENY path must import
 * `permissions/evaluatePermission.ts` (a plain `.ts` module) directly, which
 * resolves without the barrel.
 */
export function hasPermissionNow(_expr?: unknown): boolean {
  return true
}

export function setAuthView(_view?: unknown): void {
  /* no-op */
}

export function evaluatePermission(): boolean {
  return true
}
