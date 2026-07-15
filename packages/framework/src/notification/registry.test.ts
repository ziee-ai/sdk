import { describe, expect, it } from 'vitest'
import {
  getNotificationRenderer,
  registeredNotificationKinds,
  registerNotificationKind,
} from './registry'
import type { AppNotification, NotificationRendererCtx } from './types'

const n = (kind: string): AppNotification => ({
  id: '1',
  kind,
  title: 'T',
  body: 'B',
  interrupt: true,
  payload: {},
  read_at: null,
  created_at: '2026-07-14T00:00:00Z',
})
const ctx: NotificationRendererCtx = {
  markRead: () => {},
  remove: () => {},
  close: () => {},
}

describe('notification kind registry (SDK seam)', () => {
  it('dispatches a registered kind to its renderer, and returns undefined for an unknown kind', () => {
    registerNotificationKind('test_kind', {
      render: notif => `rendered:${notif.title}`,
    })
    const r = getNotificationRenderer('test_kind')
    expect(r).toBeDefined()
    expect(r!.render(n('test_kind'), ctx)).toBe('rendered:T')

    // Unknown kind → no renderer (the inbox uses its generic title/body fallback).
    expect(getNotificationRenderer('never_registered')).toBeUndefined()
  })

  it('lists registered kinds', () => {
    registerNotificationKind('another_kind', { render: () => null })
    expect(registeredNotificationKinds()).toContain('another_kind')
  })
})
