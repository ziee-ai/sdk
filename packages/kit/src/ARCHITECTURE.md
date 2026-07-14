# Kit UI architecture (legacy → shadcn)

The app imports UI **only** from `@/components/ui` (the barrel). Behind it:

```
components/ui/
├── index.ts        ← public surface (barrel)
├── shadcn/         ← vendored primitives. CLI-generated:
│                     `npx shadcn@latest add <c> --overwrite --yes` (aliases.ui → here).
│                     NEVER hand-edit; re-run the CLI to update.
└── kit/            ← OUR components, composing the primitives. Shadcn-native prop
                      types (so tsc flags any legacy holdover at the call site) PLUS
                      deliberate additions for real gaps.
```

## Principles

1. **Shadcn-native types are the base.** Kit props mirror shadcn (`variant`, `size`,
   `asChild`, native attrs). Pointing a call site at the barrel makes **`tsc` the
   migration punch-list** — it errors on legacy props *and* value changes
   (`type="primary"`, `size="middle"`). No deprecation layer, no extra lint.
2. **The kit is a superset, not a passthrough.** Add capabilities shadcn leaves to
   hand-composition — using *our* clean names, never legacy's (e.g. Button `loading`,
   `href`). Never reintroduce legacy spellings (keeps tsc honest).
3. **Cross-cutting state flows through ONE channel** — see KitSurface below. Never a
   bespoke context per concern (no `FormDisabledContext`, no `CardLoadingContext`, …).

## Cross-cutting foundations (design these first; components consume them)

### 1. Tokens / theme
`src/index.css` defines the shadcn token vars (`:root` + `html.dark`) the
`tailwind.config.js` references. Dark mode = `html.dark` (same toggle the app uses).
*(TODO: align token values to the current legacy theme during rollout.)*

### 2. KitSurface — ambient state inheritance (`kit/surface.tsx`)
One typed, nestable, merging context for every cross-cutting axis:

```ts
interface KitSurface { disabled?; loading?; readOnly?; size? }
<KitSurfaceProvider loading> … </KitSurfaceProvider>   // nestable; inner merges over outer
const s = useSurface(ownProps)                          // own wins when defined (incl. false)
```

- **Containers set it** (Form, Card, Section, app root) — all the *same* provider.
- **Components read it and own their reaction** — state vs reaction are decoupled:

  | resolved axis | Button | Input* | Card* | Table* |
  |---|---|---|---|---|
  | `loading` (data not ready) | skeleton | skeleton line | skeleton body | skeleton rows |
  | `disabled` | disabled | disabled | dim + block | dim |
  | `size` | density | density | padding | row height |
  | `readOnly` | — | read-only | — | — |

  *(\* not built yet — contract for when they are.)*

- A **loading boundary** is just `<Loading>…</Loading>` (sugar for
  `<KitSurfaceProvider loading>`): every kit component inside, at any depth, renders
  **its own** skeleton. Generic "swap children for `<Skeleton>`" can't shape each one;
  component-owned skeletons can.
- Adding an axis = one field on `KitSurface` + the components that care. No new context.
- Note: a component's *own* action state (Button `loading` → spinner) is distinct from
  ambient region `loading` (→ skeleton). Region loading wins.
- Perf: a single context re-renders all consumers on change. Fine to start; switch to
  `use-context-selector` or split hot/cold fields only if profiling shows churn.

### 3. Form integration (DONE — `kit/form.tsx`)
react-hook-form + zod + shadcn `field`. `Form` renders FormProvider + `<KitSurfaceProvider
disabled size>` + native `<form onSubmit={form.handleSubmit(onSubmit)}>`, so form-level
disabled/size propagate through the SAME KitSurface channel — no form-specific context.
`useForm`/`zodResolver` re-exported. `FormField` wraps the control ELEMENT (legacy-`Form.Item` style) — control comes from
the Form CONTEXT (no `control` prop). It injects value/onChange/onBlur/name/id/ref onto
the child via cloneElement, so kit controls must be form-bindable (value + onChange(value)
+ ref; Select has an onChange alias). Use `valuePropName="checked"` for Switch/Checkbox.
Usage:
```tsx
const form = useForm<Values>({ resolver: zodResolver(schema), defaultValues })
<Form form={form} onSubmit={save} disabled={form.formState.isSubmitting}>
  <FormField name="email" label="Email"><Input placeholder="…" /></FormField>
  <FormField name="theme" label="Theme"><Select options={opts} /></FormField>
  <Button type="submit" loading={form.formState.isSubmitting}>Save</Button>
</Form>
```

## Control contract (MANDATORY — every interactive kit control)
Every control MUST give the surface axes full parity — as **own props AND ambient**,
own winning — by funnelling them through one `useSurface` call:
```ts
const s = useSurface({ disabled, size /*, readOnly where meaningful */ })  // NOT own `loading`
if (s.loading) return <Skeleton …shaped like this control… />   // region loading only
// then apply s.disabled, s.size, s.readOnly; own `loading` → in-place spinner
```
- **Two distinct loading meanings (uniform across all controls):**
  - **Region loading** = ambient `surface.loading` (a `<KitSurfaceProvider loading>` /
    `<Loading>` wrapping an area whose data isn't ready) → **skeleton**, component-shaped.
  - **Own `loading` prop** = *this* control is busy → **spinner** (in-place: button glyph,
    select trigger, input suffix) + disabled. NEVER a skeleton.
  - ⇒ own `loading` must NOT be passed into `useSurface` (only ambient drives the skeleton).
- A control must obey BOTH a parent provider (ambient) and its own props (no provider needed).

## Adding a component (recipe)
1. `npx shadcn@latest add <c> --overwrite --yes` → lands in `shadcn/`.
2. Write `kit/<c>.tsx`: shadcn-native props + needed superset extras; consume
   `useSurface()` for the axes it reacts to.
3. Export from `index.ts`.
4. `tsc` against real usage (the punch-list).
