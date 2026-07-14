import { create } from 'zustand'
import { subscribeWithSelector } from 'zustand/middleware'
import { immer } from 'zustand/middleware/immer'
import { enableMapSet } from 'immer'
import type { EventHandler, Unsubscribe, AppEvents, BaseEvent } from './types'

// Enable immer support for Map/Set drafts — the store's `handlers` and
// `groupHandlers` are Maps mutated inside immer set() callbacks. Without
// this, immer's auto-freeze turns those mutations into silent no-ops in
// dev and throws in prod.
enableMapSet()

interface EventBusState {
  // Map of event type to set of handlers (use any for storage, type-safe at call site)
  handlers: Map<string, Set<EventHandler<any>>>

  // Map of group name + event type to handler (for deduplication)
  groupHandlers: Map<string, EventHandler<any>>

  // Subscribe to specific event type (type-safe)
  // If groupName provided, automatically replaces any existing handler for same group+event
  on: <K extends keyof AppEvents>(
    eventType: K,
    handler: EventHandler<AppEvents[K]>,
    groupName?: string,
  ) => Unsubscribe

  // Emit event to all subscribed handlers (type-safe)
  emit: <K extends keyof AppEvents>(
    event: Omit<AppEvents[K], 'timestamp'>,
  ) => Promise<void>

  // Remove specific handler
  off: (eventType: string, handler: EventHandler<any>) => void

  // Remove all handlers for a specific group
  removeGroupListeners: (groupName: string) => void

  // Clear all handlers (for testing)
  clear: () => void

  // Get handler count for debugging
  getHandlerCount: (eventType?: string) => number
}

export const useEventBusStore = create<EventBusState>()(
  subscribeWithSelector(
    immer(
      (set, get): EventBusState => ({
        handlers: new Map(),
        groupHandlers: new Map(),

        on: (eventType, handler, groupName) => {
          set(state => {
            // If group name provided, implement deduplication
            if (groupName) {
              const groupKey = `${groupName}::${eventType}`

              // Remove old handler for this group+event if it exists
              const oldHandler = state.groupHandlers.get(groupKey)
              if (oldHandler) {
                const eventHandlers = state.handlers.get(eventType)
                if (eventHandlers) {
                  eventHandlers.delete(oldHandler)
                }
              }

              // Track new handler for this group+event
              state.groupHandlers.set(groupKey, handler)
            }

            // Add handler to main handlers map
            if (!state.handlers.has(eventType)) {
              state.handlers.set(eventType, new Set())
            }
            state.handlers.get(eventType)!.add(handler)
          })

          // Return unsubscribe function
          return () => {
            get().off(eventType, handler)
          }
        },

        emit: async <K extends keyof AppEvents>(
          event: Omit<AppEvents[K], 'timestamp'>,
        ) => {
          // Add timestamp automatically
          const fullEvent = {
            ...event,
            timestamp: Date.now(),
          } as AppEvents[K] & BaseEvent

          // Extract event type - safe because all events must have 'type' property
          const eventType = (event as any).type as string

          const { handlers } = get()
          const eventHandlers = handlers.get(eventType)

          if (!eventHandlers || eventHandlers.size === 0) {
            return
          }

          // Execute all handlers (non-blocking). Each handler is isolated:
          // a sync throw OR an async rejection from one handler is logged
          // and swallowed so it can't bubble up through Promise.all and
          // reject the emit() caller (which is usually a mutation that
          // shouldn't fail because a listener has a bug).
          const promises = Array.from(eventHandlers).map(handler =>
            Promise.resolve()
              .then(() => handler(fullEvent))
              .catch(error => {
                console.error(
                  `[EventBus] Handler error for ${eventType}:`,
                  error,
                )
              }),
          )

          await Promise.all(promises)
        },

        off: (eventType, handler) => {
          set(state => {
            const eventHandlers = state.handlers.get(eventType)
            if (eventHandlers) {
              eventHandlers.delete(handler)
              if (eventHandlers.size === 0) {
                state.handlers.delete(eventType)
              }
            }

            // Also remove from groupHandlers if present
            const keysToDelete: string[] = []
            state.groupHandlers.forEach((groupHandler, key) => {
              if (groupHandler === handler) {
                keysToDelete.push(key)
              }
            })
            keysToDelete.forEach(key => state.groupHandlers.delete(key))
          })
        },

        removeGroupListeners: (groupName: string) => {
          set(state => {
            const prefix = `${groupName}::`
            const keysToDelete: string[] = []

            // Find all group handlers for this group
            state.groupHandlers.forEach((handler, key) => {
              if (key.startsWith(prefix)) {
                keysToDelete.push(key)

                // Extract event type from key (format: "groupName::eventType")
                const eventType = key.substring(prefix.length)

                // Remove handler from main handlers map
                const eventHandlers = state.handlers.get(eventType)
                if (eventHandlers) {
                  eventHandlers.delete(handler)
                  if (eventHandlers.size === 0) {
                    state.handlers.delete(eventType)
                  }
                }
              }
            })

            // Remove all group handler entries
            keysToDelete.forEach(key => state.groupHandlers.delete(key))

            console.debug(
              `[EventBus] Removed ${keysToDelete.length} listeners for group: ${groupName}`,
            )
          })
        },

        clear: () => {
          set(state => {
            state.handlers.clear()
            state.groupHandlers.clear()
          })
        },

        getHandlerCount: (eventType?: string) => {
          const { handlers } = get()
          if (eventType) {
            return handlers.get(eventType)?.size || 0
          }
          return Array.from(handlers.values()).reduce(
            (sum, set) => sum + set.size,
            0,
          )
        },
      }),
    ),
  ),
)
