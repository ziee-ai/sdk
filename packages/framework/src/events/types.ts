/**
 * Base event structure
 * All module events must extend this
 */
export interface BaseEvent {
  type: string
  timestamp: number // Auto-added by EventBus.emit()
}

/**
 * Event handler function type
 */
export type EventHandler<T extends BaseEvent = BaseEvent> = (
  event: T,
) => void | Promise<void>

/**
 * Unsubscribe function returned by on()
 */
export type Unsubscribe = () => void

/**
 * Global event registry
 * Modules augment this via declaration merging
 */
export interface AppEvents {
  // Modules add their events via declaration merging
  // Example:
  // 'group.created': GroupCreatedEvent
  // 'group.updated': GroupUpdatedEvent
}

/**
 * Union type of all registered events
 * Automatically includes all events from AppEvents
 */
export type AppEvent = AppEvents[keyof AppEvents]
