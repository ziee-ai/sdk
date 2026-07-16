/**
 * Generic UI extension slot registry (dogfood gap G8, frontend half).
 *
 * The FE counterpart of `ziee_framework::entity_extension`: a domain-agnostic
 * primitive for the "modules register ordered React components into named UI
 * injection points, and a `<Slot>` component renders whatever is registered"
 * pattern. ziee's chat extension registry grew this inline (its `slotRegistry`
 * Map + `renderSlot` + hand-written `<ExtensionSlot>`); CytoAnalyst re-grew the
 * same shape. Only the slot-NAME union and the wrapper `data-*` attribute
 * differ — the mechanics (register-by-owner, order-sort, enabled-gate,
 * unregister-by-owner, keyed render) are identical and live here.
 *
 * An app parameterizes the primitive with its own slot-name union:
 *
 * ```ts
 * type ChatSlotName = keyof typeof CHAT_SLOTS
 * const slots = createSlotRegistry<ChatSlotName>({ isEnabled })
 * export const ExtensionSlot = createExtensionSlot(hostRegistry, {
 *   slotAttr: 'data-chat-extension-slot',
 * })
 * ```
 */

import { Fragment } from 'react'
import type { ComponentType, FC, ReactNode } from 'react'

/** A single slot registration: a component + an optional render order. */
export interface SlotRegistrationInput {
  /** React component to render in the slot. */
  component: ComponentType
  /** Render order (lower renders first; falls back to `defaultOrder`). */
  order?: number
}

/** Options for {@link createSlotRegistry}. */
export interface CreateSlotRegistryOptions {
  /** Order applied when a registration omits `order` (default `100`). */
  defaultOrder?: number
  /**
   * Optional gate consulted at render time: return `false` to hide an owner's
   * slot components (e.g. a disabled extension). Lets the host keep enabled
   * state as its single source of truth. Default: everything enabled.
   */
  isEnabled?: (ownerName: string) => boolean
  /** Optional label for `console.error` diagnostics on a failed render. */
  label?: string
}

/** The generic slot registry, keyed by the app's slot-name union. */
export interface SlotRegistry<SlotName extends string> {
  /**
   * Register an owner's slot components. Re-registering the same owner is
   * HMR-safe — its previous entries are dropped first.
   */
  register(
    ownerName: string,
    slots: Partial<Record<SlotName, SlotRegistrationInput>>,
  ): void
  /** Remove every entry contributed by `ownerName`. */
  unregister(ownerName: string): void
  /** Render every enabled component for `slot`, order-sorted, keyed by owner. */
  renderSlot(slot: SlotName): ReactNode[]
  /** Whether `slot` has at least one (enabled) renderer. */
  hasRenderers(slot: SlotName): boolean
}

interface SlotEntry {
  ownerName: string
  Component: ComponentType
  order: number
}

/**
 * Create a slot registry parameterized by the app's slot-name union. Domain
 * agnostic: the registry never imports app types — the union is a pure
 * type-parameter and slot names are plain strings at runtime.
 */
export function createSlotRegistry<SlotName extends string>(
  options: CreateSlotRegistryOptions = {},
): SlotRegistry<SlotName> {
  const defaultOrder = options.defaultOrder ?? 100
  const isEnabled = options.isEnabled ?? (() => true)
  const registry = new Map<SlotName, SlotEntry[]>()

  const unregister = (ownerName: string): void => {
    for (const [slot, entries] of registry.entries()) {
      const filtered = entries.filter((e) => e.ownerName !== ownerName)
      if (filtered.length === 0) {
        registry.delete(slot)
      } else {
        registry.set(slot, filtered)
      }
    }
  }

  return {
    register(ownerName, slots) {
      // HMR-safe: drop this owner's prior entries before re-adding.
      unregister(ownerName)
      for (const [slotName, registration] of Object.entries(slots) as [
        SlotName,
        SlotRegistrationInput | undefined,
      ][]) {
        if (!registration) continue
        if (!registry.has(slotName)) registry.set(slotName, [])
        registry.get(slotName)!.push({
          ownerName,
          Component: registration.component,
          order: registration.order ?? defaultOrder,
        })
      }
    },

    unregister,

    renderSlot(slot) {
      const entries = registry.get(slot)
      if (!entries || entries.length === 0) return []

      const enabled = entries
        .filter((e) => isEnabled(e.ownerName))
        .sort((a, b) => a.order - b.order)

      const nodes: ReactNode[] = []
      for (const { ownerName, Component } of enabled) {
        try {
          nodes.push(<Component key={ownerName} />)
        } catch (error) {
          console.error(
            `[${options.label ?? 'Slots'}] Error rendering slot '${slot}' in ${ownerName}:`,
            error,
          )
        }
      }
      return nodes
    },

    hasRenderers(slot) {
      const entries = registry.get(slot)
      if (!entries) return false
      return entries.some((e) => isEnabled(e.ownerName))
    },
  }
}

/** Anything that can render a slot (a {@link SlotRegistry} or a host wrapping one). */
export interface ExtensionSlotSource<SlotName extends string> {
  renderSlot(slot: SlotName): ReactNode[]
}

/** Props of the `<ExtensionSlot>` component produced by {@link createExtensionSlot}. */
export interface ExtensionSlotProps<SlotName extends string> {
  /** Name of the slot to render. */
  name: SlotName
  /** Optional wrapper className. */
  className?: string
  /** Optional fallback rendered when no extension contributes to the slot. */
  fallback?: ReactNode
  /** Optional stable test selector forwarded onto the wrapper div. */
  'data-testid'?: string
}

/** Options for {@link createExtensionSlot}. */
export interface CreateExtensionSlotOptions {
  /**
   * `data-*` attribute stamped on the wrapper div (default
   * `'data-extension-slot'`). Apps set their existing attribute so DOM +
   * E2E selectors stay byte-identical (ziee chat uses
   * `'data-chat-extension-slot'`).
   */
  slotAttr?: string
}

/**
 * Build an `<ExtensionSlot>` component bound to a slot source. Renders every
 * registered component for `name` inside a wrapper div carrying the configured
 * `data-*` attribute; renders `fallback` (or nothing) when the slot is empty.
 */
export function createExtensionSlot<SlotName extends string>(
  source: ExtensionSlotSource<SlotName>,
  options: CreateExtensionSlotOptions = {},
): FC<ExtensionSlotProps<SlotName>> {
  const slotAttr = options.slotAttr ?? 'data-extension-slot'

  return function ExtensionSlot({
    name,
    className,
    fallback,
    'data-testid': dataTestid,
  }: ExtensionSlotProps<SlotName>) {
    const renderers = source.renderSlot(name)

    if (renderers.length === 0) {
      return fallback ? <>{fallback}</> : null
    }

    return (
      <div className={className} {...{ [slotAttr]: name }} data-testid={dataTestid}>
        {renderers.map((renderer, index) => (
          <Fragment key={`${name}-${index}`}>{renderer}</Fragment>
        ))}
      </div>
    )
  }
}

/** One discovered default export + the glob path it came from. */
export interface DiscoveredModule<T> {
  path: string
  value: T
}

/**
 * Merge one or more `import.meta.glob(..., { eager: true })` result maps and
 * extract each module's default export — the auto-discovery helper behind the
 * "drop a file at the conventional path and it registers itself" convention.
 * Modules without a default export are skipped. The app sorts + registers the
 * returned values (its ordering policy stays app-side).
 */
export function collectGlobDefaults<T>(
  ...globResults: Record<string, { default?: T }>[]
): DiscoveredModule<T>[] {
  const out: DiscoveredModule<T>[] = []
  for (const glob of globResults) {
    for (const [path, mod] of Object.entries(glob)) {
      const value = mod?.default
      if (value !== undefined) out.push({ path, value })
    }
  }
  return out
}
