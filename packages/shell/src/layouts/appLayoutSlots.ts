import type {
  ReactNode,
  ReactElement,
  ComponentType,
  LazyExoticComponent,
} from 'react'
import type { PermissionExpr } from '@ziee/framework/permissions'

// The AppLayout SIDEBAR SLOT contract — the 7 slot definitions the shell's
// generic `AppLayout` renders, expressed as TYPE declarations only. An app
// contributes concrete entries through the module system; the shell renders
// whatever registered (permission-filtering is the app's / slot renderer's
// responsibility via the optional `permission` field).
//
// This file carries the `Slots` augmentation, so any compilation that pulls it
// in (the app re-exports it from its app-layout `types` shim) sees the slot
// keys as valid.

/**
 * Sidebar navigation item
 */
export interface SidebarNavItem {
  id: string
  icon: ReactNode
  label: string
  path: string
  order?: number
  /**
   * Optional permission expression. When set, the entry is hidden
   * from the sidebar for users who don't satisfy it. See
   * `.claude/PERMISSION_GATING.md`.
   */
  permission?: PermissionExpr
}

/**
 * Sidebar tool item (appears in tools section)
 */
export interface SidebarToolItem {
  id: string
  icon: ReactNode
  label: string
  path: string
  order?: number
  /**
   * Optional permission expression. When set, the entry is hidden
   * from the sidebar tools section for users who don't satisfy it.
   * See `.claude/PERMISSION_GATING.md`.
   */
  permission?: PermissionExpr
}

/**
 * Sidebar action button (appears at the top)
 */
export interface SidebarActionItem {
  id: string
  icon: ReactNode
  label: string
  onClick?: () => void
  to?: string
  order?: number
}

/**
 * Sidebar widget item (used for components in recent, bottom, footer sections)
 */
export interface SidebarWidgetItem {
  id: string
  component:
    | ComponentType<any>
    | ReactElement
    | LazyExoticComponent<ComponentType<any>>
    | (() => Promise<{ default: ComponentType<any> }>)
  order: number
  /**
   * Optional permission expression. When set, the widget is filtered
   * out of its slot (content / bottom / footer) for users who don't
   * satisfy it — same semantics as `SidebarNavItem.permission`. Use it
   * for widgets that render permission-restricted data (e.g. the recent-
   * conversations list → `conversations::read`, the download indicator →
   * `llm_models::downloads_read`). Widgets that render mixed content
   * (some always-visible) should instead self-gate internally with
   * `<Can>` / `usePermission`. See `.claude/PERMISSION_GATING.md`.
   */
  permission?: PermissionExpr
}

/**
 * Register AppLayout sidebar slots
 */
declare module '@ziee/framework/module-system/types' {
  interface Slots {
    sidebarNavigation: SidebarNavItem[]
    sidebarTools: SidebarToolItem[]
    sidebarPrimaryActions: SidebarActionItem[]
    sidebarContent: SidebarWidgetItem[]
    sidebarBottom: SidebarWidgetItem[]
    sidebarFooter: SidebarWidgetItem[]
    /** App-wide banners rendered at the top of the content area (above the
     *  routed page). Used for the admin "update available" notice. Modules not
     *  loaded in a given bundle (e.g. server-update on desktop) contribute
     *  nothing here. */
    appBanners: SidebarWidgetItem[]
  }
}
