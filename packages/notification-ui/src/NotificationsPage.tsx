import { Check, Trash2 } from 'lucide-react'
import { type ReactNode, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'

import {
  type AppNotification,
  getNotificationRenderer,
  type NotificationRendererCtx,
} from '@ziee/framework/notification'
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

import { notificationsStore } from './storeView'
import type { NotificationRow } from './types'

/**
 * Render a notification's inbox content by dispatching on its `kind` through the
 * `@ziee/framework/notification` renderer registry, falling back to the generic
 * title/body/timestamp block for any unregistered kind. The generated-row →
 * seam `AppNotification` shape bridge (structurally identical) is encapsulated
 * here in one documented cast.
 */
function renderNotificationContent(
  n: NotificationRow,
  ctx: NotificationRendererCtx,
): ReactNode {
  const renderer = getNotificationRenderer(n.kind)
  if (renderer) return renderer.render(n as unknown as AppNotification, ctx)
  return (
    <>
      <Text className="font-medium">{n.title}</Text>
      {n.body ? (
        <Text className="text-muted-foreground text-sm">{n.body}</Text>
      ) : null}
      <Text className="text-muted-foreground text-xs">
        {new Date(n.created_at).toLocaleString()}
      </Text>
    </>
  )
}

/** The full notification inbox at /notifications. Generic + app-agnostic. */
export function NotificationsPage() {
  const { items, unread, total, page, perPage, unreadOnly, loading, error } =
    notificationsStore()
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
  const rendererCtx: NotificationRendererCtx = {
    markRead: (id: string) => void notificationsStore().markRead(id),
    remove: (id: string) => void notificationsStore().remove(id),
    close: () => {},
  }

  const open = (n: NotificationRow) => {
    void notificationsStore().markRead(n.id)
    // Kind-specific ids ride the `payload` jsonb column (typed `unknown`).
    const conversationId = (n.payload as { conversation_id?: string } | null)
      ?.conversation_id
    if (conversationId) navigate(`/chat/${conversationId}`)
  }

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
              <Flex className="items-start gap-3">
                {!n.read_at && (
                  <span className="mt-1.5 h-2 w-2 shrink-0 rounded-full bg-primary" aria-label="Unread" role="img" />
                )}
                <Button
                  variant="ghost"
                  className="h-auto min-w-0 flex-1 flex-col items-start gap-0.5 whitespace-normal text-start"
                  onClick={() => open(n)}
                  data-testid={`notification-open-${n.id}`}
                >
                  {renderNotificationContent(n, rendererCtx)}
                </Button>
                <Flex className="gap-1">
                  {!n.read_at && (
                    <Button
                      data-testid={`notification-read-${n.id}`}
                      variant="ghost"
                      aria-label="Mark read"
                      onClick={() => void notificationsStore().markRead(n.id)}
                    >
                      <Check size={16} />
                    </Button>
                  )}
                  <Button
                    data-testid={`notification-delete-${n.id}`}
                    variant="ghost"
                    aria-label="Delete"
                    onClick={() => void notificationsStore().remove(n.id)}
                  >
                    <Trash2 size={16} />
                  </Button>
                </Flex>
              </Flex>
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
