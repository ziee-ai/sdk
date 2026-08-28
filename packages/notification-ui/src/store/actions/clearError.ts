import type { NotificationsGet, NotificationsSet } from '../state'

export default (set: NotificationsSet, _get: NotificationsGet) => () => {
  set(draft => {
    draft.error = null
  })
}
