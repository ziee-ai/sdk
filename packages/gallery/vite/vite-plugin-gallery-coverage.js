/**
 * PART 2 — gallery branch-coverage instrumentation (opt-in via GALLERY_COVERAGE=1).
 *
 * plugin-react v6 transpiles with oxc (no babel hook), so we can't inject
 * babel-plugin-istanbul through it. Instead this standalone `enforce: 'pre'`
 * plugin instruments the ORIGINAL .ts/.tsx source of every component/page under
 * `src/modules` + `src/components/ui` with istanbul-lib-instrument, then lets
 * the normal react/oxc pipeline transpile the instrumented source. Instrumenting
 * the source (not the transpiled JS) means branch locations map straight to
 * source line numbers — no sourcemap juggling — so UNCOVERED_STATES.md points at
 * the real conditional.
 */
import { createInstrumenter } from 'istanbul-lib-instrument'
import path from 'node:path'

const BASE_PARSER_PLUGINS = [
  'typescript',
  'decorators-legacy',
  'classProperties',
  'classPrivateProperties',
  'classPrivateMethods',
  'objectRestSpread',
  'dynamicImport',
  'importAssertions',
  'topLevelAwait',
]

// `.ts` and `.tsx` need different parser configs: enabling `jsx` on a `.ts` file
// misparses generic arrow syntax (`<T>() => …`), so keep one instrumenter each.
function makeInstrumenter(jsx) {
  return createInstrumenter({
    esModules: true,
    compact: false,
    produceSourceMap: true,
    autoWrap: true,
    parserPlugins: jsx ? [...BASE_PARSER_PLUGINS, 'jsx'] : BASE_PARSER_PLUGINS,
  })
}

export function galleryCoveragePlugin(opts = {}) {
  const srcDir = opts.srcDir || 'src'
  const instTs = makeInstrumenter(false)
  const instTsx = makeInstrumenter(true)

  const shouldInstrument = id => {
    const norm = id.split('?')[0].replace(/\\/g, '/')
    if (!/\/src\/(modules|components\/ui)\//.test(norm)) return false
    if (!/\.(ts|tsx)$/.test(norm)) return false
    if (/\.(test|spec|stories)\.(ts|tsx)$/.test(norm)) return false
    if (/\/src\/dev\//.test(norm)) return false
    return true
  }

  return {
    name: 'gallery-coverage-istanbul',
    enforce: 'pre',
    apply: 'serve',
    transform(code, id) {
      if (!shouldInstrument(id)) return null
      const file = id.split('?')[0]
      const inst = /\.tsx$/.test(file) ? instTsx : instTs
      try {
        const instrumented = inst.instrumentSync(code, file)
        return { code: instrumented, map: inst.lastSourceMap() }
      } catch (e) {
        // A file istanbul can't parse (exotic syntax) is skipped, not fatal —
        // it just won't contribute branch coverage.
        this.warn(`[gallery-coverage] skipped ${path.relative(srcDir, file)}: ${e.message}`)
        return null
      }
    },
  }
}
