// Test-only stub for @/core/events (store-kit's event-bus boundary). Not
// exercised by the proxy/action tests; stubbed so the graph loads under node.
export const useEventBusStore = {
  getState: () => ({
    on: () => () => {},
    removeGroupListeners: () => {},
  }),
}
