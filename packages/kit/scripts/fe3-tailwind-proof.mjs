// FE-3 functional proof: the documented one-line wiring
// (`@import "@ziee/kit/styles/kit.css"`) makes Tailwind v4 scan the kit's own
// component sources under node_modules and emit their utility classes — so kit
// components render STYLED in a fresh app. Uses the same engine
// (`@tailwindcss/node` compile + `@tailwindcss/oxide` Scanner) that
// `@tailwindcss/vite` drives.
import { compile } from '@tailwindcss/node'
import { Scanner } from '@tailwindcss/oxide'
import { readFileSync } from 'node:fs'
import { dirname, resolve as presolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const KIT_STYLES = presolve(HERE, '../src/styles')

// Simulate the app's main CSS: Tailwind itself + the kit's one-line wiring.
// We inline kit.css's directives against the kit's real base dir so `@source`
// and `@import "./tokens.css"` resolve exactly as they would from
// node_modules/@ziee/kit/src/styles.
const appCss = `@import "tailwindcss";\n${readFileSync(presolve(KIT_STYLES, 'kit.css'), 'utf8')}`

const { build, sources } = await compile(appCss, {
  base: KIT_STYLES, // kit.css's own location — where its @source is anchored
  onDependency: () => {},
})

// The @source directive in kit.css must register a source rooted at the kit's
// own src tree (so Tailwind scans node_modules/@ziee/kit/src from a real app).
const kitSrc = presolve(KIT_STYLES, '..')
if (!sources.some(s => presolve(s.base, s.pattern).startsWith(kitSrc))) {
  console.error('FAIL: kit.css @source did not register the kit src glob:', sources)
  process.exit(1)
}

// Scan the registered sources (the kit's components) for candidate classes.
const scanner = new Scanner({ sources })
const candidates = scanner.scan()

for (const need of ['bg-primary', 'text-muted-foreground', 'rounded-md']) {
  if (!candidates.includes(need)) {
    console.error(`FAIL: expected kit component class "${need}" not found by @source scan`)
    process.exit(1)
  }
}

// Build the CSS from the scanned candidates and assert real rules were emitted.
const css = build(candidates)
// `.bg-primary` proves the kit's own component class was scanned + emitted;
// `--primary` proves the token layer (tokens.css, imported by kit.css) is
// present so that utility actually resolves to a color.
if (!css.includes('.bg-primary') || !css.includes('--primary')) {
  console.error('FAIL: compiled CSS missing kit utility rules / token variables')
  process.exit(1)
}

console.log(
  `PASS FE-3: kit.css scanned ${candidates.length} candidates from the kit src, ` +
    `emitted ${css.length} bytes of CSS incl. .bg-primary + the --primary token.`,
)
