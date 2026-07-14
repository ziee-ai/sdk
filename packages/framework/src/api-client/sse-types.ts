// SSE event types for reuse across the API layer
export type SSEEventKey<T> = T extends Record<string, any>
  ? T extends any[] | Function
    ? '__init'
    : keyof T extends string
      ? keyof T | '__init'
      : '__init'
  : '__init'

export type SSEEventData<T, K extends SSEEventKey<T>> = K extends '__init'
  ? { abortController: AbortController }
  : T extends Record<string, any>
    ? K extends keyof T
      ? T[K]
      : never
    : never

// Generic SSE event handler with proper type narrowing
// Export the handlers type for explicit use
export type SSEHandlers<T extends Record<string, any>> = {
  [K in keyof T]: (data: T[K]) => void | Promise<void>
} & {
  __init?: (data: { abortController: AbortController }) => void | Promise<void>
  default?: (event: string, data: any) => void | Promise<void>
}

// Unified SSE callback type that supports both function and object formats
export type SSECallback<T> =
  | (<E extends SSEEventKey<T>>(event: E, data: SSEEventData<T, E>) => void | Promise<void>)
  | ((event: string, data: any) => void | Promise<void>)
  | (T extends Record<string, any> ? Partial<SSEHandlers<T>> : never)

export function createSSEHandler<T extends Record<string, any>>(
  handlers: SSEHandlers<T>,
) {
  return (event: SSEEventKey<T>, data: any): void | Promise<void> => {
    if (event === '__init' && handlers.__init) {
      return handlers.__init(data as { abortController: AbortController })
    } else if (event in handlers && event !== '__init') {
      const handler = handlers[event as keyof T]
      if (handler) {
        return handler(data) as void | Promise<void>
      }
    } else if (handlers.default) {
      return handlers.default(event as string, data)
    }
  }
}
