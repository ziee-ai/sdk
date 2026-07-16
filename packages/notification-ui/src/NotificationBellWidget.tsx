import { Bell } from 'lucide-react'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'

import { Badge, Button, Empty, Flex, Popover, Text } from '@ziee/kit'

import { notificationsStore } from './storeView'
import type { NotificationRow } from './types'

/**
 * Sidebar (sidebarBottom slot) notification bell: an unread-count badge over a
 * bell icon, opening a popover with the most recent notifications. Generic +
 * app-agnostic — reads the app-registered `Stores.Notifications` through a
 * typed-view seam. Live via that store's `sync:notification` subscription.
 */
export function NotificationBellWidget() {
  const { items, unread } = notificationsStore()
  const [open, setOpen] = useState(false)
  const navigate = useNavigate()

  const recent = items.slice(0, 8)

  const goTo = (n: NotificationRow) => {
    void notificationsStore().markRead(n.id)
    setOpen(false)
    // Kind-specific ids ride the `payload` jsonb column (typed `unknown`).
    const conversationId = (n.payload as { conversation_id?: string } | null)
      ?.conversation_id
    if (conversationId) navigate(`/chat/${conversationId}`)
    else navigate('/notifications')
  }

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
        <Flex className="flex-col gap-1">
          {recent.map(n => (
            <Button
              key={n.id}
              variant="ghost"
              data-testid={`notification-bell-item-${n.id}`}
              onClick={() => goTo(n)}
              className="h-auto w-full flex-col items-start gap-0.5 whitespace-normal px-2 py-2 text-start"
            >
              <Flex className="w-full items-center gap-2">
                {!n.read_at && (
                  <span className="h-2 w-2 shrink-0 rounded-full bg-primary" aria-label="Unread" role="img" />
                )}
                <Text className="flex-1 truncate font-medium">{n.title}</Text>
              </Flex>
              {n.body && (
                <Text className="line-clamp-2 text-muted-foreground text-sm">
                  {n.body}
                </Text>
              )}
              <Text className="text-muted-foreground text-xs">
                {new Date(n.created_at).toLocaleString()}
              </Text>
            </Button>
          ))}
        </Flex>
      )}
      <Flex className="justify-center pt-2">
        <Button
          data-testid="notification-bell-view-all"
          variant="ghost"
          onClick={() => {
            setOpen(false)
            navigate('/notifications')
          }}
        >
          View all
        </Button>
      </Flex>
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
