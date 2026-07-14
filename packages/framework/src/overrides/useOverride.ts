/**
 * `useOverride` — resolve a seam to the winning component (override or fallback)
 * for logic-heavy call sites that want the component reference rather than the
 * `<Seam>` wrapper. It calls no React hooks (the registry is fixed at boot), but
 * is `use`-prefixed because it is intended to be read during render.
 *
 *   const MonitorButton = useOverride('hardware.monitor-button', DefaultMonitorButton)
 *   return <MonitorButton {...props} />
 */
import type { ComponentType } from 'react'
import type { UIOverrides } from './types'
import { resolveOverride } from './registry.ts'

export function useOverride<K extends keyof UIOverrides>(
  key: K,
  fallback: ComponentType<UIOverrides[K]>,
): ComponentType<UIOverrides[K]> {
  return resolveOverride(key) ?? fallback
}
