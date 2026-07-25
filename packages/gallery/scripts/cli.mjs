#!/usr/bin/env node
/**
 * `ziee-gallery <command> [args]` — dispatch to the config-driven gallery scripts.
 * Each script resolves `gallery.config.json` from the current working directory,
 * so run it from the app's `ui/` root.
 */
import { spawn } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const COMMANDS = {
  'gate-ui': 'gate-ui.mjs',
  'runtime-health': 'runtime-health.mjs',
  'gen-state-matrix': 'gen-state-matrix.mjs',
  'gen-overlay-registry': 'gen-overlay-registry.mjs',
  'gen-gallery-coverage': 'gen-gallery-coverage.mjs',
  'gen-gallery-seed-registry': 'gen-gallery-seed-registry.mjs',
  'gallery-coverage': 'gallery-coverage.mjs',
  'check-gallery-prod-exclusion': 'check-gallery-prod-exclusion.mjs',
  'capture-screenshots': 'capture-gallery-screenshots.mjs',
  'capture-states': 'capture-gallery-states.mjs',
}

const [cmd, ...rest] = process.argv.slice(2)
const script = COMMANDS[cmd]
if (!script) {
  console.error(
    `ziee-gallery: unknown command "${cmd ?? ''}". Known: ${Object.keys(COMMANDS).join(', ')}`,
  )
  process.exit(2)
}

const child = spawn('node', [path.join(__dirname, script), ...rest], {
  stdio: 'inherit',
})
child.on('exit', code => process.exit(code ?? 0))
