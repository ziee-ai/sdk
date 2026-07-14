/**
 * Guardrail: settings FORMS use the kit Field/FormField — no hand-rolled form rows.
 *
 * The kit ships a Field/FieldGroup/FieldLabel layer (components/ui/shadcn/field.tsx)
 * and a react-hook-form-backed Form/FormField (components/ui/kit/form.tsx). Settings
 * pages MUST route their form controls through it so label placement, row gap, and
 * section spacing stay uniform across every settings surface. Hand-rolling a form row as
 * a raw flex-col gap-N div wrapping an Input (with its own arbitrary gap) is exactly the
 * drift this rule prevents — the gap values across settings pages used to sprawl
 * gap-1 / gap-2 / gap-3 / gap-4 / gap-6.
 *
 * The rule: in a SETTINGS-scoped file (a module `pages` page, a `Settings` file, or a
 * settings `sections` / `settings` section), a kit form control
 * (Input/InputNumber/Textarea/Select/Switch/Checkbox/RadioGroup/Segmented/DatePicker/
 * Slider/Upload) MUST be a descendant of a <FormField> or <Field>. A control that is
 * genuinely NOT a form field — a list-row toggle, a search box, a toolbar filter — opts
 * out with the `data-standalone-control` flag on the element (mirrors the color linter's
 * `data-allow-custom-color`).
 *
 * kit/ + shadcn/ are excluded — they DEFINE the primitives the rest of the app consumes.
 * Biome's GritQL plugins can't introspect JSX ancestry in this build, so this is a
 * TS-compiler-API AST check, wired into `check` (and CI).
 *
 *   node scripts/lint-settings-field.mjs            # fail on any violation
 */
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'

import { parseRoots } from './roots.mjs'

const require = createRequire(import.meta.url)
const ts = require('typescript')

// Parameterized over the app's src tree(s) via `--root=<dir>` (repeatable).
const ROOTS = parseRoots()
const OPT_OUT = 'data-standalone-control'

// Kit form controls that belong in a Field. Names as imported from '@/components/ui'.
const CONTROLS = new Set([
  'Input',
  'InputNumber',
  'InputPassword',
  'Textarea',
  'Select',
  'Switch',
  'Checkbox',
  'RadioGroup',
  'Segmented',
  'DatePicker',
  'Slider',
  'Upload',
])
// Elements whose subtree satisfies "control is in a Field".
const FIELD_WRAPPERS = new Set(['FormField', 'Field'])

// A settings-scoped file: a module page, a *Settings* file, or a settings section.
function isScoped(file) {
  const p = file.replace(/\\/g, '/')
  const base = path.basename(p)
  if (/\/modules\/[^/]+\/(?:.*\/)?pages\//.test(p)) return true
  if (/Settings.*\.tsx$/.test(base) || /^Settings.*\.tsx$/.test(base))
    return true
  if (/\/components\/(?:sections|settings)\//.test(p)) return true
  return false
}

function findFiles(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc
  for (const e of fs.readdirSync(dir)) {
    const full = path.join(dir, e)
    const st = fs.statSync(full)
    if (st.isDirectory()) {
      if (
        !['node_modules', 'dist', 'build', '.git', 'tests', 'shadcn'].includes(
          e,
        ) &&
        !full.endsWith(path.join('components', 'ui', 'kit'))
      ) {
        findFiles(full, acc)
      }
    } else if (/\.tsx$/.test(e) && !e.endsWith('.generated.ts')) {
      acc.push(full)
    }
  }
  return acc
}

const violations = []

function tagName(node) {
  const el = ts.isJsxElement(node)
    ? node.openingElement
    : ts.isJsxSelfClosingElement(node)
      ? node
      : null
  if (!el) return null
  const name = el.tagName
  return ts.isIdentifier(name) ? name.text : name.getText()
}

function attrNames(node) {
  const el = ts.isJsxElement(node) ? node.openingElement : node
  const props = el.attributes?.properties ?? []
  const names = new Set()
  for (const a of props) {
    if (ts.isJsxAttribute(a) && a.name && ts.isIdentifier(a.name))
      names.add(a.name.text)
  }
  return names
}

function report(node, sf, msg) {
  const el = ts.isJsxElement(node) ? node.openingElement : node
  const { line, character } = sf.getLineAndCharacterOfPosition(el.getStart(sf))
  violations.push({
    file: sf.fileName,
    line: line + 1,
    col: character + 1,
    msg,
  })
}

for (const root of ROOTS) {
  for (const file of findFiles(root)) {
    if (!isScoped(file)) continue
    const sf = ts.createSourceFile(
      file,
      fs.readFileSync(file, 'utf-8'),
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TSX,
    )
    // Walk with a "inside a Field wrapper" depth flag.
    const visit = (node, inField) => {
      let nextInField = inField
      const tag = tagName(node)
      if (tag && FIELD_WRAPPERS.has(tag)) nextInField = true
      if (tag && CONTROLS.has(tag) && !inField) {
        if (!attrNames(node).has(OPT_OUT)) {
          report(node, sf, `<${tag}> outside a FormField/Field`)
        }
      }
      ts.forEachChild(node, c => visit(c, nextInField))
    }
    visit(sf, false)
  }
}

if (violations.length) {
  console.error(
    `\n[settings-field] ✗ ${violations.length} form control(s) outside a Field:\n`,
  )
  for (const v of violations) {
    console.error(
      `  ${path.relative(process.cwd(), v.file)}:${v.line}:${v.col}  ${v.msg}`,
    )
  }
  console.error(
    `\nWrap the control in a kit <FormField> (state-backed forms) or <Field>/<FieldLabel>\n` +
      `(components/ui/shadcn/field.tsx) so label + gap + spacing stay uniform.\n` +
      `If this is a genuinely standalone control (search box, list-row toggle, toolbar\n` +
      `filter — NOT a form field), add the \`${OPT_OUT}\` flag to that element.\n`,
  )
  process.exit(1)
}
console.log('[settings-field] ✓ all settings form controls are inside a Field.')
