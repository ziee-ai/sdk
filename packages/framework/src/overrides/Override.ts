/**
 * `<Seam>` — the wrap-in-place element-override primitive.
 *
 * Wrap the overridable element; its children ARE the fallback (the original web
 * markup), so declaring a seam needs no `DefaultFoo` extraction — just a wrap:
 *
 *   <Seam id="hardware.monitor-button">
 *     <Button onClick={openWeb}>Monitor</Button>   // fallback (web)
 *   </Seam>
 *
 * When the desktop build has registered an override for `id`, the override is
 * rendered with `props`; otherwise the children render unchanged. `props` is
 * REQUIRED when the seam's `UIOverrides[id]` declares any prop, and optional
 * when the override takes none (`Record<string, never>`), so a no-prop button
 * seam stays terse while a prop-carrying seam is type-checked.
 */
import type { ReactNode } from 'react'
import { createElement, Fragment } from 'react'
import type { UIOverrides } from './types'
import { resolveOverride } from './registry.ts'

type SeamPropsBase<K extends keyof UIOverrides> = {
  id: K
  children: ReactNode
}

// props optional only when the override takes an empty prop bag.
type SeamProps<K extends keyof UIOverrides> = SeamPropsBase<K> &
  (UIOverrides[K] extends Record<string, never>
    ? { props?: UIOverrides[K] }
    : { props: UIOverrides[K] })

export function Seam<K extends keyof UIOverrides>({
  id,
  props,
  children,
}: SeamProps<K>) {
  const Override = resolveOverride(id)
  if (Override) {
    return createElement(Override, (props ?? {}) as UIOverrides[K])
  }
  // JSX-free `<>{children}</>` so the primitive is importable under `node --test`
  // (the core unit runner strips types but does not transform JSX).
  return createElement(Fragment, null, children)
}
