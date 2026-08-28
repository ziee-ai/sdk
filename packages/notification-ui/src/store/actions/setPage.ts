import type { NotificationsGet, NotificationsSet } from '../state'
import loadFactory from './load'

export default (set: NotificationsSet, get: NotificationsGet) => {
  const load = loadFactory(set, get)
  return (page: number) => {
    set(draft => {
      draft.page = page
    })
    void load()
  }
}
