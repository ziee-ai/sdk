// Minimal ambient declarations for this package's own `tsc --noEmit`. The
// consuming app supplies the real `vite/client` types at build time.
interface ImportMetaEnv {
  readonly DEV: boolean
  readonly [key: string]: unknown
}
interface ImportMeta {
  readonly env: ImportMetaEnv
  // The folder-glob store pattern (`actions: import.meta.glob('./actions/*.ts')`).
  // The consuming app supplies the real `vite/client` overloads at build time;
  // this minimal signature is only for this package's own standalone `tsc`.
  glob(
    pattern: string,
    options?: { eager?: boolean; import?: string },
  ): Record<string, () => Promise<unknown>>
}

// Side-effect CSS imports (followed transitively into @ziee/kit source).
declare module '*.css'
