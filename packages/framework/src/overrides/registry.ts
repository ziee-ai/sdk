/**
 * UI Override Registry — the runtime store.
 *
 * A module-level `Map` of seam-key → override component, populated ONCE at
 * desktop boot (before first render) and read at render time. Platform is fixed
 * at boot, so there is no reactivity concern and no React context is needed —
 * this mirrors the chat `panelRendererRegistry` (`Chat.store.ts:104-150`)
 * one level more general.
 *
 * The web build registers nothing, so `resolveOverride` always returns
 * `undefined` there and every `<Seam>` renders its fallback → web behavior is
 * byte-identical to before this infrastructure existed.
 *
 * Storage is type-erased on the private edge; the precise `UIOverrides[K]` props
 * type is enforced only on the public `registerOverride` / `resolveOverride`
 * edges, where the caller supplies a concrete `K` (same technique the panel
 * registry uses to avoid collapsing to `never` when zero seams are declared).
 */
import type { ComponentType } from 'react'
import type { UIOverrides } from './types'

type ErasedProps = Record<string, unknown>

const overrideRegistry = new Map<string, ComponentType<ErasedProps>>()

/**
 * Register the desktop implementation for a declared seam. Last-write-wins.
 * Called from a desktop module's `initialize()` (pre-render).
 */
export function registerOverride<K extends keyof UIOverrides>(
  key: K,
  component: ComponentType<UIOverrides[K]>,
): void {
  // Sound: the public <K> signature already proved `component` accepts
  // UIOverrides[K], a subtype of ErasedProps; widen to the erased storage shape.
  overrideRegistry.set(
    key as string,
    component as unknown as ComponentType<ErasedProps>,
  )
}

/**
 * Resolve the override registered for a seam, or `undefined` if none (the web
 * case, and any seam the desktop chose not to override).
 */
export function resolveOverride<K extends keyof UIOverrides>(
  key: K,
): ComponentType<UIOverrides[K]> | undefined {
  return overrideRegistry.get(key as string) as
    | ComponentType<UIOverrides[K]>
    | undefined
}

/** Test/introspection: wipe the registry (used by unit tests between cases). */
export function __clearOverrides(): void {
  overrideRegistry.clear()
}

/** Test/introspection: the set of currently-registered seam keys. */
export function __registeredOverrideKeys(): string[] {
  return [...overrideRegistry.keys()]
}
