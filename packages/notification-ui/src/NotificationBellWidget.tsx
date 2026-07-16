import { Bell } from 'lucide-react'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'

import type { NotificationRendererCtx } from '@ziee/framework/notification'
import { Badge, Button, Empty, Flex, Popover, Separator, Text } from '@ziee/kit'

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

  const content = (
    <div style={{ width: 340, maxHeight: 460, overflowY: 'auto' }}>
      <Flex className="items-center justify-between px-1 pb-2">
        <Text className="font-medium">Notifications</Text>
        {unread > 0 && (
          <Button
            data-testid="notification-bell-mark-all"
            variant="ghost"
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
        <Flex className="flex-col">
          {recent.map((n, i) => (
            <div key={n.id} className="w-full px-1 py-1">
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
