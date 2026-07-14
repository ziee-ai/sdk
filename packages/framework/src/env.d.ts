// Minimal ambient declaration for the Vite `import.meta.env` surface the
// runtime reads (`import.meta.env.DEV`). The framework has no direct Vite
// dependency, so this stands in for `vite/client` under the package's own
// `tsc --noEmit`. The consuming app supplies the real `vite/client` types.
interface ImportMetaEnv {
  readonly DEV: boolean
  readonly [key: string]: unknown
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
