// Minimal ambient declarations for the shell's own `tsc --noEmit`. The
// consuming app supplies the real `vite/client` types at build time.
interface ImportMetaEnv {
  readonly DEV: boolean
  readonly [key: string]: unknown
}
interface ImportMeta {
  readonly env: ImportMetaEnv
}

// Side-effect CSS imports (followed transitively into @ziee/kit source).
declare module '*.css'
