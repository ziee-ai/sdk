import * as React from 'react'
import { SidebarTrigger as BaseSidebarTrigger } from '../shadcn/sidebar'

// Sidebar = a large composite primitive (provider + collapsible rail + mobile sheet + menu parts),
// composed at the call site like Select/Dialog parts. Structural parts pass through; the one
// interactive icon control (SidebarTrigger) forces a caller-owned accessible name — the vendored
// primitive hardcodes an English sr-only "Toggle Sidebar", which a required aria-label overrides.
export {
  Sidebar, SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupAction, SidebarGroupContent,
  SidebarGroupLabel, SidebarHeader, SidebarInput, SidebarInset, SidebarMenu, SidebarMenuAction,
  SidebarMenuBadge, SidebarMenuButton, SidebarMenuItem, SidebarMenuSkeleton, SidebarMenuSub,
  SidebarMenuSubButton, SidebarMenuSubItem, SidebarProvider, SidebarRail, SidebarSeparator,
  useSidebar,
} from '../shadcn/sidebar'

// SidebarTrigger is an icon-only button → a caller-owned accessible name is REQUIRED (no default;
// overrides the primitive's hardcoded sr-only text so it's translatable).
export type SidebarTriggerProps = React.ComponentProps<typeof BaseSidebarTrigger> & {
  'aria-label': string
  /** Test selector — REQUIRED, forwarded onto the trigger via {...props} (i18n-safe). */
  'data-testid': string
}
// forwardRef so it can still be used as a Tooltip/Popover `asChild` trigger.
export const SidebarTrigger = React.forwardRef<
  React.ElementRef<typeof BaseSidebarTrigger>,
  SidebarTriggerProps
>(function SidebarTrigger(props, ref) {
  return <BaseSidebarTrigger ref={ref} {...props} />
})
