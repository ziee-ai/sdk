/**
 * Failure-proof suite for the logical-direction lint.
 *
 * The point of this file is the EXIT STATUS. A prior audit of the consuming app
 * found that every one of its lints could be disarmed by flipping `exit(1)` to
 * `exit(0)` while the lints' own self-tests still passed 13/13 and 24/24 —
 * because those suites asserted the printed findings and never the code the
 * process returned. Every assertion below is on `status`, and each RED case has
 * a GREEN control so "always refuse" cannot score either.
 *
 * Each case builds a throwaway git repo with a `main` and a feature branch, so
 * the diff scoping under test is the real one.
 *
 *   node --test sdk/packages/config/src/lint/logical-direction.test.mjs
 */
import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const LINT = fileURLToPath(new URL('./logical-direction.mjs', import.meta.url))
const made = []
process.on('exit', () => {
  for (const d of made) rmSync(d, { recursive: true, force: true })
})

function git(repo, ...args) {
  execFileSync('git', args, { cwd: repo, stdio: ['ignore', 'ignore', 'pipe'] })
}

/** A repo with `main` holding one clean file, then a `feature` branch checked out. */
function newRepo({ trunk = 'main' } = {}) {
  const repo = mkdtempSync(path.join(tmpdir(), 'ld-lint-'))
  made.push(repo)
  git(repo, 'init', '-q', '-b', trunk)
  git(repo, 'config', 'user.email', 't@t.t')
  git(repo, 'config', 'user.name', 't')
  mkdirSync(path.join(repo, 'src-app/ui/src/modules/x'), { recursive: true })
  write(repo, 'src-app/ui/src/modules/x/Base.tsx', 'export const B = () => <div className="ps-4" />\n')
  git(repo, 'add', '-A')
  git(repo, 'commit', '-qm', 'base')
  git(repo, 'checkout', '-q', '-b', 'feature')
  return repo
}
function write(repo, rel, body) {
  mkdirSync(path.dirname(path.join(repo, rel)), { recursive: true })
  writeFileSync(path.join(repo, rel), body)
}
function run(repo, ...args) {
  const r = spawnSync(process.execPath, [LINT, ...args], { cwd: repo, encoding: 'utf8' })
  return { status: r.status, out: `${r.stdout}${r.stderr}` }
}

test('a clean changed file passes — exit 0 (the control that stops "always refuse")', () => {
  const repo = newRepo()
  write(repo, 'src-app/ui/src/modules/x/New.tsx', 'export const N = () => <div className="ps-4 me-2 text-start" />\n')
  git(repo, 'add', '-A')
  const r = run(repo)
  assert.equal(r.status, 0, r.out)
  assert.match(r.out, /✓ 1 changed file/)
})

test('a physical utility on an added line is REFUSED — exit 1', () => {
  const repo = newRepo()
  write(repo, 'src-app/ui/src/modules/x/New.tsx', 'export const N = () => <div className="pl-4 text-left" />\n')
  git(repo, 'add', '-A')
  const r = run(repo)
  assert.equal(r.status, 1, r.out)
  assert.match(r.out, /pl-/)
  assert.match(r.out, /text-left/)
})

test('BLIND SPOT 1: no resolvable base ref is a FAILURE, not a silent pass', () => {
  // The trunk is `trunk`, so neither `origin/main` nor `main` resolves — exactly
  // the state `actions/checkout` with the default fetch-depth: 1 produces.
  const repo = newRepo({ trunk: 'trunk' })
  write(repo, 'src-app/ui/src/modules/x/New.tsx', 'export const N = () => <div className="pl-4" />\n')
  git(repo, 'add', '-A')
  const r = run(repo)
  assert.notEqual(r.status, 0, `expected a refusal, got exit 0:\n${r.out}`)
  assert.match(r.out, /no base ref/)
  // …and the declared escape still works, but it has to be typed.
  const declared = run(repo, '--allow-no-base')
  assert.equal(declared.status, 0, declared.out)
  assert.match(declared.out, /--allow-no-base was passed/)
  // …and naming the base explicitly makes the lint see the finding again.
  const named = run(repo, '--base=trunk')
  assert.equal(named.status, 1, named.out)
  assert.match(named.out, /pl-/)
})

test('BLIND SPOT 2: an UNTRACKED new file is new code', () => {
  const repo = newRepo()
  // never `git add`ed — `git diff <base>` does not mention it at all
  write(repo, 'src-app/ui/src/modules/x/Probe.tsx', 'export const P = () => <div className="pl-4 text-left" />\n')
  const r = run(repo)
  assert.equal(r.status, 1, `an untracked file with pl-4 must fail:\n${r.out}`)
  assert.match(r.out, /Probe\.tsx/)
})

test('BLIND SPOT 3: .css is scanned — utilities and longhand properties', () => {
  const repo = newRepo()
  write(
    repo,
    'src-app/ui/src/styles/probe.css',
    '.a { padding-left: 8px; }\n.b { @apply pl-4; }\n.c { text-align: right; }\n',
  )
  git(repo, 'add', '-A')
  const r = run(repo)
  assert.equal(r.status, 1, r.out)
  assert.match(r.out, /padding-left/)
  assert.match(r.out, /text-align: right/)
})

test('.css opt-out and comments do not fire', () => {
  const repo = newRepo()
  write(
    repo,
    'src-app/ui/src/styles/ok.css',
    '/* pl-4 mentioned in prose */\n.a { padding-left: 8px; } /* rtl-ok: LTR-locked scrubber */\n',
  )
  git(repo, 'add', '-A')
  const r = run(repo)
  assert.equal(r.status, 0, r.out)
})

test('BLIND SPOT 4: a scope that matches nothing is a FAILURE, not a clean run', () => {
  const repo = newRepo()
  write(repo, 'src-app/ui/src/modules/x/New.tsx', 'export const N = () => <div className="pl-4" />\n')
  git(repo, 'add', '-A')
  const r = run(repo, '--path-include=does/not/exist/')
  assert.notEqual(r.status, 0, `a zero-file scan over a non-empty diff must refuse:\n${r.out}`)
  assert.match(r.out, /scanned 0 files/)
})

test('--root is honoured, not silently dropped', () => {
  const repo = newRepo()
  // A UI tree that is NOT ziee's hard-coded default path.
  write(repo, 'web/src/Comp.tsx', 'export const C = () => <div className="pl-4" />\n')
  git(repo, 'add', '-A')
  // Without a scope the default trees match nothing → the zero-scan floor fires.
  assert.notEqual(run(repo).status, 0)
  // With --root pointed at the real tree the finding is found.
  const r = run(repo, '--root=web/src')
  assert.equal(r.status, 1, r.out)
  assert.match(r.out, /Comp\.tsx/)
})

test('a Rust-only branch legitimately scans zero files and passes', () => {
  const repo = newRepo()
  write(repo, 'src-app/server/src/lib.rs', 'pub fn f() {}\n')
  git(repo, 'add', '-A')
  const r = run(repo)
  assert.equal(r.status, 0, r.out)
})
