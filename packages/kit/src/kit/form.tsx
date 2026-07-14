import * as React from 'react'
import {
  Controller,
  FormProvider,
  useForm as useRhfForm,
  useFormContext,
  useWatch,
  useFieldArray,
  useFormState,
  type ArrayPath,
  type FieldValues,
  type SubmitHandler,
  type UseFormProps,
  type UseFormReturn,
  type UseFieldArrayReturn,
} from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Field, FieldLabel, FieldContent, FieldDescription, FieldError, FieldGroup } from '../shadcn/field'
import { KitSurfaceProvider } from './surface'
import { cn } from '../lib/utils'

// Layout flows from Form → FormField via context (legacy layout / labelCol-width).
//   vertical (default) = label above control
//   horizontal         = label beside control (set `labelWidth` for a fixed label column)
//   inline             = fields flow left-to-right, wrapping
export type FormLayout = 'vertical' | 'horizontal' | 'inline'
interface FormLayoutValue {
  layout: FormLayout
  labelWidth?: number | string
}
const FormLayoutContext = React.createContext<FormLayoutValue>({ layout: 'vertical' })
const px = (w: number | string) => (typeof w === 'number' ? `${w}px` : w)

// react-hook-form is the engine. Validation = a zod schema via zodResolver:
//   const form = useForm<Values>({ resolver: zodResolver(schema), defaultValues })
// Kit default timing = 'onTouched' (validate after first blur, then re-validate on
// change). Override via `mode`/`reValidateMode`.
export { zodResolver }
export function useForm<T extends FieldValues>(props?: UseFormProps<T>): UseFormReturn<T> {
  return useRhfForm<T>({ mode: 'onTouched', ...props })
}

// ---------------------------------------------------------------------------
// react-hook-form escape hatches (typed pass-throughs).
//
// These let a child component reach the surrounding <Form>'s state without
// prop-drilling — the kit equivalents of the legacy form-instance helpers a
// parent form used to share down the tree.
// ---------------------------------------------------------------------------

/**
 * Read the surrounding <Form>'s rhf instance from context (analog of the
 * legacy `Form.useFormInstance()` / the parent-form-context hook). Use inside
 * a component rendered under <Form> to call `setValue`/`getValues`/etc.
 */
export { useFormContext }

/**
 * Subscribe to one or more field values and re-render on change (analog of the
 * legacy `Form.useWatch(name)`). Prefer this over `form.watch` inside deep
 * children — it reads the rhf control from context.
 */
export { useWatch }

/**
 * Manage a dynamic array/list of fields (analog of the legacy `Form.List`):
 * returns `{ fields, append, remove, move, insert, ... }`. For a render-prop
 * surface that mirrors the old list API, prefer the <FormList> helper below.
 */
export { useFieldArray }

/**
 * Subscribe to derived form state (`isDirty`/`errors`/`isSubmitting`/…) from
 * context without re-rendering on every value change (analog of reading the
 * legacy form instance's status flags).
 */
export { useFormState }

/**
 * Bind a single controlled field to the surrounding form (analog of the legacy
 * field wrapper). <FormField> wraps this for the common case; reach for
 * <Controller> directly only when you need full control over the render.
 */
export { Controller }

export type { UseFormReturn, UseFieldArrayReturn }

/**
 * <FormList> — dynamic array-of-rows field (the kit analog of the legacy
 * `Form.List`). Built on rhf `useFieldArray`; renders a render-prop child with
 * the array helpers so callers own the row markup.
 *
 *   <FormList name="items">
 *     {({ fields, append, remove, move }) => (
 *       <>
 *         {fields.map((f, i) => (
 *           <FormField key={f.id} name={`items.${i}.value`} label="Value">
 *             <Input />
 *           </FormField>
 *         ))}
 *         <Button type="button" onClick={() => append({ value: '' })}>Add</Button>
 *       </>
 *     )}
 *   </FormList>
 *
 * `field.id` (NOT the array index) is the stable React key. The control reads
 * the form from context, so <FormList> must live inside a <Form>.
 */
export interface FormListRenderProps<T extends FieldValues> {
  fields: UseFieldArrayReturn<T>['fields']
  append: UseFieldArrayReturn<T>['append']
  remove: UseFieldArrayReturn<T>['remove']
  move: UseFieldArrayReturn<T>['move']
  insert: UseFieldArrayReturn<T>['insert']
  replace: UseFieldArrayReturn<T>['replace']
  update: UseFieldArrayReturn<T>['update']
}
export interface FormListProps<T extends FieldValues> {
  /** Field path of the array in the surrounding form (e.g. "headers"). */
  name: ArrayPath<T>
  children: (helpers: FormListRenderProps<T>) => React.ReactNode
}
export function FormList<T extends FieldValues = FieldValues>({ name, children }: FormListProps<T>) {
  const { control } = useFormContext<T>()
  const { fields, append, remove, move, insert, replace, update } = useFieldArray<T>({ control, name })
  return <>{children({ fields, append, remove, move, insert, replace, update })}</>
}

export interface FormProps<T extends FieldValues> {
  form: UseFormReturn<T>
  onSubmit: SubmitHandler<T>
  /** Disables every control inside (propagates via KitSurface). */
  disabled?: boolean
  size?: 'sm' | 'default' | 'lg'
  /** Native form name (legacy `name`); also used to namespace field ids if needed. */
  name?: string
  /** Label position for every FormField inside (legacy `layout`). Default 'vertical'. */
  layout?: FormLayout
  /** Fixed label-column width for horizontal layout (legacy labelCol), e.g. 120 or '8rem'. */
  labelWidth?: number | string
  className?: string
  /** Test selector — forwarded onto the <form> root (i18n-safe). */
  'data-testid': string
  children: React.ReactNode
}

export function Form<T extends FieldValues>({ form, onSubmit, disabled, size, name, layout = 'vertical', labelWidth, className, children, 'data-testid': testid }: FormProps<T>) {
  // Responsive: a horizontal form (label BESIDE the control) stacks to vertical
  // (label ABOVE the control) once its own container gets narrow — otherwise the
  // fixed label column leaves the control almost no room on small screens. We
  // observe the form's OWN width (not the viewport), so it also stacks inside a
  // narrow drawer/card.
  const formRef = React.useRef<HTMLFormElement>(null)
  const [narrow, setNarrow] = React.useState(false)
  React.useLayoutEffect(() => {
    const el = formRef.current
    if (!el || layout !== 'horizontal') return
    const update = (w: number) => setNarrow(w < 480)
    update(el.offsetWidth)
    const ro = new ResizeObserver((entries) => update(entries[0].contentRect.width))
    ro.observe(el)
    return () => ro.disconnect()
  }, [layout])
  const effectiveLayout: FormLayout = layout === 'horizontal' && narrow ? 'vertical' : layout
  // Horizontal forms with no explicit `labelWidth` otherwise let the label column
  // grow to ~half the row (huge gap before the control). Default to a consistent
  // fixed column so every settings form aligns the same way. (Dropped when stacked.)
  const effectiveLabelWidth =
    effectiveLayout === 'horizontal' ? (labelWidth == null ? '13rem' : labelWidth) : undefined
  return (
    <FormProvider {...form}>
      <FormLayoutContext.Provider value={React.useMemo(() => ({ layout: effectiveLayout, labelWidth: effectiveLabelWidth }), [effectiveLayout, effectiveLabelWidth])}>
        <KitSurfaceProvider disabled={disabled} size={size}>
          {/* id={name} lets a submit button rendered OUTSIDE the <form> (e.g. in a
              Drawer/Dialog footer) trigger it via the native `form="<name>"` attribute. */}
          <form ref={formRef} id={name} name={name} onSubmit={form.handleSubmit(onSubmit)} className={className} noValidate data-testid={testid}>
            {/* KitSurface disables kit components (+ <a>/custom); <fieldset disabled>
                also disables native + third-party form controls. `contents` keeps layout. */}
            <fieldset disabled={disabled} className="contents">
              {/* inline forms flow horizontally + wrap; the per-field <Field> is w-full by
                  default, so override it to w-auto here or every field claims a full row. */}
              <FieldGroup className={effectiveLayout === 'inline' ? 'flex-row flex-wrap items-end gap-4 [&>[data-slot=field]]:w-auto' : undefined}>
                {children}
              </FieldGroup>
            </fieldset>
          </form>
        </KitSurfaceProvider>
      </FormLayoutContext.Provider>
    </FormProvider>
  )
}

interface FormFieldBase {
  name: string
  description?: React.ReactNode
  className?: string
  /** Marks the field required: adds `aria-required` + a visual `*` on the label. */
  required?: boolean
  /** Prop the value binds to. Default 'value'; use 'checked' for Switch/Checkbox. */
  valuePropName?: string
  /**
   * A single kit control element. value/onChange/onBlur/name/id/ref + invalid +
   * aria-describedby (and aria-label/labelledby when there's no visible label) are
   * injected. The control MUST accept an `onChange(value)` (kit controls do; Select
   * aliases it). A consumer-supplied onChange/onBlur on the child is composed.
   */
  children: React.ReactElement
}
// ACCESSIBLE NAME REQUIRED (a11y): every field must have a `label`, OR — when there
// is no visible label (e.g. a table-cell editor named by its column header) — an
// explicit `aria-label`/`aria-labelledby`. tsc rejects a nameless FormField.
export type FormFieldProps = FormFieldBase & (
  | { label: React.ReactNode; 'aria-label'?: string; 'aria-labelledby'?: string }
  | { label?: undefined; 'aria-label': string; 'aria-labelledby'?: string }
  | { label?: undefined; 'aria-label'?: string; 'aria-labelledby': string }
)

// Wrap the control element; control comes from the Form context
// (no `control` prop). Bindings are injected via cloneElement onto the kit control.
export function FormField(props: FormFieldProps) {
  const {
    name, label, description, className, required, valuePropName = 'value', children,
    'aria-label': ariaLabel, 'aria-labelledby': ariaLabelledby,
  } = props
  const { control } = useFormContext()
  const { layout, labelWidth } = React.useContext(FormLayoutContext)
  const beside = layout === 'horizontal' || layout === 'inline'
  const uid = React.useId()
  const fieldId = `${uid}-field`
  const labelId = `${uid}-label`
  const descId = `${uid}-desc`
  const errId = `${uid}-err`
  const childProps = children.props as {
    onChange?: (...a: unknown[]) => void
    onBlur?: (...a: unknown[]) => void
  }
  return (
    <Controller
      control={control}
      name={name}
      render={({ field, fieldState, formState }) => {
        // The zod resolver validates the WHOLE form each run, so errors[name] may be
        // populated for fields the user hasn't reached. Only SHOW a field's error once
        // it's been touched (or the form was submitted). `formState` here is the
        // SUBSCRIBED one from the render prop (the context copy doesn't re-render us).
        const showError = (fieldState.isTouched || formState.isSubmitted) && !!fieldState.error?.message
        const describedBy =
          [description ? descId : null, showError ? errId : null].filter(Boolean).join(' ') || undefined
        const injected: Record<string, unknown> = {
          [valuePropName]: field.value,
          // compose: run the form binding, then any consumer handler on the child.
          onChange: (...a: unknown[]) => {
            ;(field.onChange as (...x: unknown[]) => void)(...a)
            childProps.onChange?.(...a)
          },
          onBlur: (...a: unknown[]) => {
            field.onBlur()
            childProps.onBlur?.(...a)
          },
          name: field.name,
          id: fieldId,
          ref: field.ref,
          'aria-describedby': describedBy,
        }
        if (showError) injected.invalid = true
        if (required) injected['aria-required'] = true
        // Accessible name. With a visible label, point the control at it via
        // aria-labelledby (in ADDITION to the label's htmlFor) — a bare
        // htmlFor/id pair names <input> for AT but some controls/checkers only
        // resolve a name via aria-label(ledby); aria-labelledby to the visible
        // label is the gold-standard association and names textarea/select too.
        if (label != null) injected['aria-labelledby'] = ariaLabelledby ?? labelId
        // No visible label → the explicit name goes on the control itself.
        if (label == null && ariaLabel) injected['aria-label'] = ariaLabel
        if (label == null && ariaLabelledby) injected['aria-labelledby'] = ariaLabelledby
        const labelEl = label != null && (
          <FieldLabel
            id={labelId}
            htmlFor={fieldId}
            // fixed label column for horizontal layout (legacy labelCol). Internal style
            // is allowed inside the kit; the style-guard only gates CONSUMER style props.
            style={beside && labelWidth != null ? { width: px(labelWidth), flex: 'none' } : undefined}
          >
            {label}
            {required && <span aria-hidden className="ml-0.5 text-destructive">*</span>}
          </FieldLabel>
        )
        const control = React.cloneElement(children, injected)
        const descEl = description != null && <FieldDescription id={descId} data-testid={`field-desc-${name}`}>{description}</FieldDescription>
        // FieldError already carries role="alert" → announced when it appears
        const errEl = showError && <FieldError id={errId} data-testid={`field-error-${name}`}>{fieldState.error?.message}</FieldError>
        // horizontal/inline: label beside a FieldContent column (control + desc + error).
        if (beside) {
          return (
            <Field orientation="horizontal" data-invalid={showError || undefined} className={cn(className)}>
              {labelEl}
              <FieldContent>
                {control}
                {descEl}
                {errEl}
              </FieldContent>
            </Field>
          )
        }
        return (
          <Field data-invalid={showError || undefined} className={cn(className)}>
            {labelEl}
            {control}
            {descEl}
            {errEl}
          </Field>
        )
      }}
    />
  )
}
