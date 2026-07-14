// @ziee/test-e2e — the Layer-B end-to-end SCAFFOLD for ziee-SDK apps. The app
// supplies its own specs + fixtures; this package supplies the reusable scaffold,
// all parameterized over ONE `E2EConfig` seam:
//   - definePlaywrightPreset / defineDesktopPreset — base Playwright configs
//     (full-stack server suite + Tauri/headless desktop variant),
//   - createGlobalSetup / createGlobalTeardown — per-run Postgres-docker bring-up,
//     UI + server warmup, first-run setup hook, cross-session-safe teardown,
//   - the generic Postgres port allocator,
//   - byTestId / makeByTestId + role/label/text — i18n-safe selectors.

export { type E2EConfig, type UiBuildConfig, type ServerWarmupConfig, defineE2EConfig } from './config.ts'
export { createGlobalSetup } from './global-setup.ts'
export { createGlobalTeardown } from './global-teardown.ts'
export {
  definePlaywrightPreset,
  defineDesktopPreset,
  type PresetPaths,
} from './playwright-preset.ts'
export {
  type TestIdLike,
  byTestId,
  makeByTestId,
  byRole,
  byLabel,
  byText,
} from './testid.ts'
export {
  allocatePostgresPort,
  releasePostgresPortLock,
  cleanupStaleLocks,
  cleanupStaleConfigFiles,
  liveRunIds,
} from './port-manager.ts'
