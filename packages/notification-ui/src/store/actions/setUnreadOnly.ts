import type { NotificationsGet, NotificationsSet } from '../state'
import loadFactory from './load'

export default (set: NotificationsSet, get: NotificationsGet) => {
  const load = loadFactory(set, get)
  return (unreadOnly: boolean) => {
    set(draft => {
      draft.unreadOnly = unreadOnly
      draft.page = 1
    })
    void load()
  }
}
