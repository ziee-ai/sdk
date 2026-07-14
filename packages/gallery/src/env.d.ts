// Minimal ambient for the Vite `import.meta.env.DEV` flag the mock engine reads
// (the package is Vite-consumed by the app; tsc needs the shape).
interface ImportMetaEnv {
  readonly DEV: boolean
}
interface ImportMeta {
  readonly env: ImportMetaEnv
}

// Side-effect CSS imports (followed transitively into @ziee/kit source).
declare module '*.css'
