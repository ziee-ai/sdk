import { Bell } from 'lucide-react'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'

import type { NotificationRendererCtx } from '@ziee/framework/notification'
import {
  Badge,
  Button,
  Empty,
  Flex,
  Popover,
  ScrollArea,
  Separator,
  Text,
} from '@ziee/kit'

import { NotificationItem } from './NotificationItem'
import { notificationsStore } from './storeView'

/**
 * Sidebar (sidebarBottom slot) notification bell: an unread-count badge over a
 * bell icon, opening a popover with the most recent notifications. Generic +
 * app-agnostic — reads the app-registered `Stores.Notifications` through a
 * typed-view seam, dispatches each row per-kind through the
 * `@ziee/framework/notification` renderer registry, and navigates via the
 * app-supplied `onNavigate` seam (the SDK hardcodes ZERO routes). Live via that
 * store's `sync:notification` subscription.
 */
export function NotificationBellWidget() {
  const { items, unread, onNavigate, inboxPath } = notificationsStore()
  const [open, setOpen] = useState(false)
  const navigate = useNavigate()

  const recent = items.slice(0, 8)

  // Renderer context: the bell's `close` dismisses the popover so a kind's
  // action (Accept / navigate) can tear it down after acting.
  const ctx: NotificationRendererCtx = {
    markRead: (id: string) => void notificationsStore().markRead(id),
    remove: (id: string) => void notificationsStore().remove(id),
    close: () => setOpen(false),
  }

  // Whole-row select → app-supplied navigation seam. No seam ⇒ rows aren't
  // whole-row clickable (per-kind actions still work).
  const onSelect = onNavigate
    ? (n: (typeof recent)[number]) => {
        void notificationsStore().markRead(n.id)
        setOpen(false)
        onNavigate(n, to => navigate(to))
      }
    : undefined

  // The panel — NOT a child of it — owns the size, and that size is bounded by
  // the viewport. Previously this wrapper carried a fixed
  // `width: 340, maxHeight: 460` inline while the kit popup box is `w-72`
  // (288px), so 62px of every row (the mark-read + delete controls) painted
  // OUTSIDE the popover's background, and at a 320px viewport the fixed 340px
  // pushed `document.scrollWidth` to 358 and scrolled the page sideways.
  //
  // The popup now carries BOTH bounds (see the `className` on `<Popover>`
  // below): `w-[min(21.25rem,calc(100vw-2rem))]` keeps the old desktop density
  // while making the width viewport-relative, and `max-h-(--available-height)`
  // caps the whole panel at the space base-ui measured between the anchor and
  // the viewport edge. The kit `Popover` forwards `className` onto the popup,
  // where tailwind-merge resolves `w-[…]` over the primitive's `w-72` — so the
  // shared kit popover primitive needs no edit.
  //
  // `min-h-0` here and on the scroller is what makes the height bound reach the
  // list: the popup is `flex flex-col`, so with a bounded popup the list takes
  // the leftover space via `flex-1` and scrolls the rest. That is deliberately
  // NOT a hardcoded "reserve Nrem for header+footer" subtraction — such a
  // constant silently breaks the moment the header or footer changes height.
  const content = (
    <div className="flex min-h-0 w-full min-w-0 flex-col">
      <Flex className="items-center justify-between gap-2 px-1 pb-2">
        <Text className="min-w-0 truncate font-medium">Notifications</Text>
        {unread > 0 && (
          <Button
            data-testid="notification-bell-mark-all"
            variant="ghost"
            className="shrink-0"
            onClick={() => void notificationsStore().markAllRead()}
          >
            Mark all read
          </Button>
        )}
      </Flex>
      {recent.length === 0 ? (
        <Empty
          description="No notifications yet"
          data-testid="notification-bell-empty"
        />
      ) : (
        // Only the LIST scrolls, so the header above and the "View all" footer
        // below stay pinned and reachable however long the list gets (before,
        // the scroll box wrapped all three and "View all" was unreachable
        // without scrolling past 8 items).
        //
        // `min-h-0 flex-1` — not a pixel cap — is what bounds it: the popup is
        // capped at `--available-height` (the custom property base-ui's
        // positioner publishes for the anchor-to-viewport-edge space; the same
        // bound `kit/dropdown.tsx` uses), and the list then takes whatever is
        // left after the pinned header + footer, whatever heights those happen
        // to be. `max-h-[26rem]` is only an aesthetic ceiling so the popover
        // doesn't run the full height of a tall desktop screen.
        <ScrollArea
          axis="y"
          autoHide="leave"
          className="max-h-[26rem] w-full min-h-0 min-w-0 flex-1"
          data-testid="notification-bell-list"
        >
          <Flex className="w-full min-w-0 flex-col">
            {recent.map((n, i) => (
              <div key={n.id} className="w-full min-w-0 px-1 py-1">
                {i > 0 && <Separator className="mb-1" />}
                <NotificationItem
                  n={n}
                  ctx={ctx}
                  testidPrefix="notification-bell"
                  onSelect={onSelect ? () => onSelect(n) : undefined}
                />
              </div>
            ))}
          </Flex>
        </ScrollArea>
      )}
      {inboxPath && (
        <Flex className="justify-center pt-2">
          <Button
            data-testid="notification-bell-view-all"
            variant="ghost"
            onClick={() => {
              setOpen(false)
              navigate(inboxPath)
            }}
          >
            View all
          </Button>
        </Flex>
      )}
    </div>
  )

  return (
    <Popover
      content={content}
      trigger="click"
      side="right"
      align="end"
      open={open}
      onOpenChange={setOpen}
      // WIDTH: `100vw` is the layout viewport INCLUDING a classic vertical
      // scrollbar, while the "no sideways body scroll" invariant is measured
      // against `documentElement.clientWidth`, which excludes it. The 2rem
      // gutter covers that ~15px difference with room to spare — do not shrink
      // it below ~1.25rem or the invariant breaks on a vertically-scrolling
      // page with classic scrollbars.
      // HEIGHT: caps the WHOLE panel, so the pinned header/footer plus the
      // list can never exceed the space base-ui measured. There is deliberately
      // no second `max-w`: an inert bound reads as a safety net that isn't one.
      className="w-[min(21.25rem,calc(100vw-2rem))] max-h-(--available-height)"
    >
      <div className="flex cursor-pointer items-center justify-center px-4 py-3">
        <Badge
          count={unread}
          tone="error"
          offset={[10, 0]}
          aria-label={unread > 0 ? `${unread} unread notifications` : 'Notifications'}
          data-testid="notification-bell-badge"
        >
          <Bell size={20} aria-label="Notifications" />
        </Badge>
      </div>
    </Popover>
  )
}
