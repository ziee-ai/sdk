// Test-only stub for @/core/module-system. createStoreProxy only imports this
// for the top-level `Stores` runtime proxy; the proxy factory under test never
// touches it. Stubbing it keeps the node --test graph free of browser-only code.
export const useModuleSystemStore = {
  getState: () => ({ stores: {} as Record<string, unknown> }),
}
