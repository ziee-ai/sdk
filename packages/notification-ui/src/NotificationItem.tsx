import { Check, Trash2 } from 'lucide-react'
import type { ReactNode } from 'react'

import {
  type AppNotification,
  getNotificationRenderer,
  type NotificationRendererCtx,
} from '@ziee/framework/notification'
import { Button, Flex, Text } from '@ziee/kit'

import type { NotificationRow } from './types'

/**
 * Dispatch a row's main content through the `@ziee/framework/notification`
 * renderer registry (`getNotificationRenderer(kind).render`), falling back to a
 * generic title / body / timestamp block for any UNREGISTERED kind. This is the
 * per-kind dispatch that lets an app's registered kinds (a study-share invite, a
 * scheduled-task result, …) render their real UI instead of bland title/body.
 *
 * The generated-row (`NotificationRow`) → seam (`AppNotification`) bridge is
 * structurally identical; the single documented cast lives here.
 */
function renderContent(
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

interface NotificationItemProps {
  n: NotificationRow
  /** Renderer context (markRead / remove / close) forwarded to the kind renderer. */
  ctx: NotificationRendererCtx
  /**
   * Whole-row select handler (the app-supplied navigation seam). When omitted
   * the content block is NOT clickable — per-kind `renderer.actions` still work
   * (matching the bespoke-bell reference, which navigates only via actions).
   */
  onSelect?: () => void
  /** Test-id namespace so the bell and the inbox page stay distinguishable. */
  testidPrefix: string
}

/**
 * One notification row, shared by the bell popover AND the full inbox page so
 * per-kind dispatch (render + actions) is IDENTICAL on both surfaces. Layout: an
 * unread dot, the dispatched content (clickable when `onSelect` is supplied),
 * the mark-read + delete controls, and — below — the kind renderer's optional
 * inline `actions` row (e.g. Accept / Decline for an invite).
 *
 * The content uses a role="button" clickable div (NOT a `<button>`) so a kind's
 * `actions` buttons never nest inside another button — invalid HTML.
 */
export function NotificationItem({
  n,
  ctx,
  onSelect,
  testidPrefix,
}: NotificationItemProps) {
  const renderer = getNotificationRenderer(n.kind)
  const content = renderContent(n, ctx)

  return (
    <Flex className="w-full flex-col gap-2">
      <Flex className="w-full items-start gap-2">
        {!n.read_at && (
          <span
            className="mt-1.5 h-2 w-2 shrink-0 rounded-full bg-primary"
            aria-label="Unread"
            role="img"
          />
        )}
        {onSelect ? (
          <div
            role="button"
            tabIndex={0}
            data-testid={`${testidPrefix}-open-${n.id}`}
            onClick={onSelect}
            onKeyDown={e => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault()
                onSelect()
              }
            }}
            className="flex min-w-0 flex-1 cursor-pointer flex-col items-start gap-0.5 text-start"
          >
            {content}
          </div>
        ) : (
          <div className="flex min-w-0 flex-1 flex-col items-start gap-0.5">
            {content}
          </div>
        )}
        <Flex className="shrink-0 gap-1">
          {!n.read_at && (
            <Button
              variant="ghost"
              aria-label="Mark read"
              data-testid={`${testidPrefix}-read-${n.id}`}
              onClick={() => ctx.markRead(n.id)}
            >
              <Check size={16} />
            </Button>
          )}
          <Button
            variant="ghost"
            aria-label="Delete"
            data-testid={`${testidPrefix}-delete-${n.id}`}
            onClick={() => ctx.remove(n.id)}
          >
            <Trash2 size={16} />
          </Button>
        </Flex>
      </Flex>
      {renderer?.actions ? (
        <Flex className="gap-2 pl-4">
          {renderer.actions(n as unknown as AppNotification, ctx)}
        </Flex>
      ) : null}
    </Flex>
  )
}
