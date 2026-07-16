import { useEffect } from 'react'
import { useNavigate } from 'react-router-dom'

import type { NotificationRendererCtx } from '@ziee/framework/notification'
import {
  Button,
  Card,
  Empty,
  ErrorState,
  Flex,
  Segmented,
  Spin,
  Text,
} from '@ziee/kit'
import { SettingsPageContainer } from '@ziee/shell'

import { NotificationItem } from './NotificationItem'
import { notificationsStore } from './storeView'
import type { NotificationRow } from './types'

/**
 * The full notification inbox at the app's inbox route. Generic + app-agnostic:
 * each row dispatches per-kind through the `@ziee/framework/notification`
 * renderer registry (render + inline actions), with a generic title/body
 * fallback, and whole-row selection routes through the app-supplied `onNavigate`
 * seam — the SDK hardcodes ZERO routes.
 */
export function NotificationsPage() {
  const {
    items,
    unread,
    total,
    page,
    perPage,
    unreadOnly,
    loading,
    error,
    onNavigate,
  } = notificationsStore()
  // Guard against a malformed/absent response leaving `items` undefined.
  const list = items ?? []
  const navigate = useNavigate()

  useEffect(() => {
    void notificationsStore().load()
    // On leaving the inbox, restore the store to its default (page 1, all) so the
    // sidebar bell — which shares Stores.Notifications.items — shows the latest
    // notifications, not this page's paginated / unread-only slice.
    return () => {
      notificationsStore().setUnreadOnly(false)
    }
  }, [])

  // Per-kind renderer context (seam). `close` is a no-op on the full page (no
  // popover to dismiss).
  const ctx: NotificationRendererCtx = {
    markRead: (id: string) => void notificationsStore().markRead(id),
    remove: (id: string) => void notificationsStore().remove(id),
    close: () => {},
  }

  // Whole-row select → app-supplied navigation seam (no hardcoded app route).
  const onSelect = onNavigate
    ? (n: NotificationRow) => {
        void notificationsStore().markRead(n.id)
        onNavigate(n, to => navigate(to))
      }
    : undefined

  return (
    <SettingsPageContainer
      title="Notifications"
      subtitle="Background results from your scheduled tasks."
      data-testid="notifications-page"
    >
      <Flex className="mb-3 items-center justify-between">
        <Segmented
          data-standalone-control
          data-testid="notifications-filter"
          value={unreadOnly ? 'unread' : 'all'}
          onChange={v => notificationsStore().setUnreadOnly(v === 'unread')}
          options={[
            { label: 'All', value: 'all' },
            { label: `Unread${unread ? ` (${unread})` : ''}`, value: 'unread' },
          ]}
        />
        <Button
          data-testid="notifications-mark-all"
          variant="outline"
          disabled={unread === 0}
          onClick={() => void notificationsStore().markAllRead()}
        >
          Mark all read
        </Button>
      </Flex>

      {loading && list.length === 0 ? (
        <Flex className="justify-center py-12">
          <Spin size="lg" label="Loading notifications" />
        </Flex>
      ) : error && list.length === 0 ? (
        <ErrorState
          variant="page"
          resource="notifications"
          details={error}
          onRetry={() => void notificationsStore().load()}
          data-testid="notifications-error"
        />
      ) : list.length === 0 ? (
        <Empty
          description="No notifications yet"
          data-testid="notifications-empty"
        />
      ) : (
        <Flex className="flex-col gap-2">
          {list.map(n => (
            <Card key={n.id} data-testid={`notification-card-${n.id}`}>
              <NotificationItem
                n={n}
                ctx={ctx}
                testidPrefix="notification"
                onSelect={onSelect ? () => onSelect(n) : undefined}
              />
            </Card>
          ))}
        </Flex>
      )}

      {total > perPage && (
        <Flex className="justify-center gap-2 pt-4">
          <Button
            data-testid="notifications-prev"
            variant="outline"
            disabled={page <= 1}
            onClick={() => notificationsStore().setPage(page - 1)}
          >
            Previous
          </Button>
          <Text className="self-center text-muted-foreground text-sm">
            Page {page} of {Math.max(1, Math.ceil(total / perPage))}
          </Text>
          <Button
            data-testid="notifications-next"
            variant="outline"
            disabled={page >= Math.ceil(total / perPage)}
            onClick={() => notificationsStore().setPage(page + 1)}
          >
            Next
          </Button>
        </Flex>
      )}
    </SettingsPageContainer>
  )
}
