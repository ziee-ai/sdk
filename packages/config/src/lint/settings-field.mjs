/**
 * Guardrail: settings FORMS use the kit Field/FormField — no hand-rolled form rows,
 * and no hand-composed HORIZONTAL field row that starves the label column.
 *
 * The kit ships a Field/FieldGroup/FieldLabel layer (components/ui/shadcn/field.tsx)
 * and a react-hook-form-backed Form/FormField (components/ui/kit/form.tsx). Settings
 * pages MUST route their form controls through it so label placement, row gap, and
 * section spacing stay uniform across every settings surface.
 *
 * TWO rules:
 *
 * RULE 1 — control-outside-a-Field.
 *   In a SETTINGS-scoped file, a kit form control
 *   (Input/InputNumber/Textarea/Select/Switch/Checkbox/RadioGroup/Segmented/DatePicker/
 *   Slider/Upload) MUST be a descendant of a <FormField> or <Field>. Hand-rolling a
 *   form row as a raw flex-col gap-N div wrapping an Input (with its own arbitrary gap)
 *   is exactly the drift this prevents — the gap values across settings pages used to
 *   sprawl gap-1 / gap-2 / gap-3 / gap-4 / gap-6.
 *
 * RULE 2 — starved label column (added after `/settings/scheduler` shipped broken).
 *   Rule 1 only asks "is the control inside a Field?" — which `/settings/scheduler`
 *   satisfied while rendering every label wrapped to ONE WORD PER LINE. It had
 *   hand-composed the horizontal row INSIDE-OUT:
 *
 *       <Field orientation="horizontal">
 *         <FieldContent><FieldTitle>Max active tasks per user</FieldTitle></FieldContent>
 *         <InputNumber />            {/* the kit Input root is `w-full min-w-0` *​/}
 *       </Field>
 *
 *   In shadcn's `fieldVariants`, `orientation: horizontal` reserves the content-sized
 *   (`flex-auto`) slot for a DIRECT-CHILD label and makes `FieldContent` the `flex-1`
 *   (flex-basis 0) CONTROL column. Putting the label in `FieldContent` and a `w-full`
 *   control beside it inverts that: the control claims ~100% and the label column
 *   collapses to min-content.
 *
 *   So: in a settings-scoped file, a STRETCHING control inside a
 *   `<Field orientation="horizontal"|"responsive">` that is NOT inside a `<FormField>`
 *   is a violation. Use `<Form layout="horizontal">` + `<FormField>` (which emits the
 *   label as a fixed-width direct child) — or make the row vertical.
 *
 *   "Stretching" is defined by the kit source, not by guesswork: the controls whose
 *   ROOT element is the native control carrying `w-full` — Input, InputNumber,
 *   InputPassword, Textarea (shadcn/input.tsx, shadcn/textarea.tsx). `Select` renders
 *   into a content-sized `<div className="relative">` wrapper and Switch/Checkbox/
 *   Segmented/RadioGroup are intrinsically sized, so none of them starves the row —
 *   they stay legal in a hand-composed horizontal/responsive Field (that is the
 *   pattern ThemeSettings and McpToolApprovalsTab use correctly).
 *
 * A control that is genuinely NOT a form field — a list-row toggle, a search box, a
 * toolbar filter — opts out of BOTH rules with the `data-standalone-control` flag on
 * the element (mirrors the color linter's `data-allow-custom-color`).
 *
 * kit/ + shadcn/ are excluded — they DEFINE the primitives the rest of the app consumes.
 * Biome's GritQL plugins can't introspect JSX ancestry in this build, so this is a
 * TS-compiler-API AST check, wired into `check` (and CI).
 *
 *   node settings-field.mjs --root=src            # fail on any violation
 */
import { createRequire } from 'node:module'
import path from 'node:path'
import fs from 'node:fs'
import { pathToFileURL } from 'node:url'

import { parseRoots } from './roots.mjs'

const require = createRequire(import.meta.url)
const ts = require('typescript')

const OPT_OUT = 'data-standalone-control'

// Kit form controls that belong in a Field. Names as imported from the kit barrel.
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
// Controls whose ROOT element carries `w-full` — these claim the whole flex row and
// starve a sibling label column. See RULE 2 above.
const STRETCHING = new Set(['Input', 'InputNumber', 'InputPassword', 'Textarea'])
// Elements whose subtree satisfies "control is in a Field".
const FIELD_WRAPPERS = new Set(['FormField', 'Field'])
// Field orientations that put the label BESIDE the control (where starvation happens).
const BESIDE_ORIENTATIONS = new Set(['horizontal', 'responsive'])

// A settings-scoped file: a module page, a *Settings* file, or a settings section.
export function isScoped(file) {
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

/** Literal value of a JSX string attribute (`orientation="horizontal"`), else null. */
function stringAttr(node, want) {
  const el = ts.isJsxElement(node) ? node.openingElement : node
  for (const a of el.attributes?.properties ?? []) {
    if (!ts.isJsxAttribute(a) || !a.name || !ts.isIdentifier(a.name)) continue
    if (a.name.text !== want) continue
    const init = a.initializer
    if (init && ts.isStringLiteral(init)) return init.text
    if (
      init &&
      ts.isJsxExpression(init) &&
      init.expression &&
      ts.isStringLiteralLike(init.expression)
    )
      return init.expression.text
  }
  return null
}

/**
 * Scan ONE source file. Exported so the rules are unit-testable without a tree.
 * Returns `[{ file, line, col, rule, msg }]`.
 */
export function scanSource(fileName, text) {
  const found = []
  const sf = ts.createSourceFile(
    fileName,
    text,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX,
  )
  const at = node => {
    const el = ts.isJsxElement(node) ? node.openingElement : node
    const { line, character } = sf.getLineAndCharacterOfPosition(el.getStart(sf))
    return { line: line + 1, col: character + 1 }
  }

  // Walk carrying (a) "inside a Field/FormField", (b) "inside a FormField",
  // (c) "inside a label-beside Field".
  const visit = (node, inField, inFormField, inBesideField) => {
    let nextInField = inField
    let nextInFormField = inFormField
    let nextInBeside = inBesideField
    const tag = tagName(node)

    if (tag && FIELD_WRAPPERS.has(tag)) {
      nextInField = true
      if (tag === 'FormField') nextInFormField = true
      if (tag === 'Field') {
        const o = stringAttr(node, 'orientation')
        if (o && BESIDE_ORIENTATIONS.has(o)) nextInBeside = true
      }
    }

    if (tag && CONTROLS.has(tag) && !attrNames(node).has(OPT_OUT)) {
      if (!inField) {
        found.push({
          file: fileName,
          ...at(node),
          rule: 'control-outside-field',
          msg: `<${tag}> outside a FormField/Field`,
        })
      } else if (inBesideField && !inFormField && STRETCHING.has(tag)) {
        found.push({
          file: fileName,
          ...at(node),
          rule: 'starved-label-column',
          msg:
            `<${tag}> (a full-width control) inside a hand-composed ` +
            `<Field orientation="horizontal"|"responsive"> — the label column collapses ` +
            `to min-content and the label wraps one word per line`,
        })
      }
    }

    ts.forEachChild(node, c =>
      visit(c, nextInField, nextInFormField, nextInBeside),
    )
  }
  visit(sf, false, false, false)
  return found
}

/** Scan every settings-scoped `.tsx` under `roots`. */
export function scanRoots(roots) {
  const found = []
  for (const root of roots) {
    for (const file of findFiles(root)) {
      if (!isScoped(file)) continue
      found.push(...scanSource(file, fs.readFileSync(file, 'utf-8')))
    }
  }
  return found
}

function main() {
  const violations = scanRoots(parseRoots())
  if (violations.length) {
    console.error(
      `\n[settings-field] ✗ ${violations.length} settings form-composition violation(s):\n`,
    )
    for (const v of violations) {
      console.error(
        `  ${path.relative(process.cwd(), v.file)}:${v.line}:${v.col}  [${v.rule}] ${v.msg}`,
      )
    }
    console.error(
      `\ncontrol-outside-field → wrap the control in a kit <FormField> (state-backed forms)\n` +
        `  or <Field>/<FieldLabel> (shadcn/field.tsx) so label + gap + spacing stay uniform.\n` +
        `starved-label-column  → a full-width control beside a label inside <FieldContent>\n` +
        `  inverts the kit's horizontal-Field contract. Use <Form layout="horizontal"> +\n` +
        `  <FormField label=…> (the label becomes a fixed-width DIRECT child of the Field),\n` +
        `  or drop the orientation so the row stacks vertically.\n` +
        `If this is a genuinely standalone control (search box, list-row toggle, toolbar\n` +
        `filter — NOT a form field), add the \`${OPT_OUT}\` flag to that element.\n`,
    )
    process.exit(1)
  }
  console.log(
    '[settings-field] ✓ all settings form controls are inside a correctly-composed Field.',
  )
}

// Run only when invoked directly (so the rules stay importable for unit tests).
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main()
}
