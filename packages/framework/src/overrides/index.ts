/**
 * UI Override Registry — public barrel.
 *
 * Element-level desktop overrides: core wraps an overridable element in `<Seam>`
 * (or reads `useOverride`), the desktop build registers a replacement via
 * `registerOverride` at boot. See `types.ts` for the declaration-merging
 * contract and the repo docs "Desktop UI Override" section.
 */
export { registerOverride, resolveOverride } from './registry'
export { useOverride } from './useOverride'
export { Seam } from './Override'
export type { UIOverrides, OverrideKey } from './types'
