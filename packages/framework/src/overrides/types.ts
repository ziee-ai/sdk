/**
 * UI Override Registry — typed seam keys.
 *
 * A "seam" is a point in a core (web) component where the desktop build may
 * substitute a different implementation for a single element, WITHOUT forking
 * the whole enclosing file. Each seam declares its key → props contract here via
 * declaration merging, exactly like the `Slots` / `PanelRendererMap` idiom:
 *
 *   declare module '@ziee/framework/overrides' {
 *     interface UIOverrides {
 *       'hardware.monitor-button': Record<string, never>
 *       'layout.drawer-header': { title: ReactNode; onClose: () => void }
 *     }
 *   }
 *
 * The value type is the props the overridable element receives. An override
 * registered for a key must be a `ComponentType<UIOverrides[key]>`; an unknown
 * key is a compile error. Keys follow `<module>.<element>` in kebab-case.
 *
 * Base interface is intentionally empty — seams augment it. Before any seam is
 * declared `keyof UIOverrides` is `never`, so `registerOverride` / `resolveOverride`
 * simply cannot be called with a bad key (same as `PanelRendererMap`).
 */
export interface UIOverrides {}

export type OverrideKey = keyof UIOverrides
