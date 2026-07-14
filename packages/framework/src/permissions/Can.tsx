import type { ReactNode } from 'react'
import { usePermission } from './usePermission'
import type { PermissionExpr } from './types'

interface CanProps {
  /**
   * Permission expression to gate the children on. Accepts a bare string
   * (`"users::delete"`), an AND group (`{ allOf: ['users::read', 'groups::read'] }`),
   * or an OR group (`{ anyOf: ['users::edit', 'users::reset_password'] }`).
   */
  permission: PermissionExpr

  /** Rendered when the user has permission. */
  children: ReactNode

  /**
   * Rendered when the user does NOT have permission. Defaults to nothing — the
   * right default for action buttons (no disabled fallback, no tooltip — the
   * button is simply absent). Provide a fallback for the narrow set of cases
   * where a "you don't have access" stub should still be visible.
   */
  fallback?: ReactNode
}

/**
 * Declarative permission wrapper. Renders `children` when the current user
 * satisfies `permission`; otherwise renders `fallback` (or nothing).
 *
 * Prefer a declarative slot field on a registered surface over wrapping with
 * `<Can>`. Reach for `<Can>` for per-button gates inside pages, and for
 * `usePermission()` when you need the boolean for conditional logic with
 * multiple branches.
 */
export function Can({ permission, children, fallback = null }: CanProps) {
  const allowed = usePermission(permission)
  return <>{allowed ? children : fallback}</>
}
