/**
 * Shared "controlled value requires a change handler" type for kit value-controls.
 *
 * A controlled input (`value` set) with NO change handler is silently frozen — a common
 * footgun. These types make that a COMPILE error while leaving uncontrolled usage (and the
 * FormField pattern, which injects value+onChange at runtime so callers write neither) valid.
 *
 *   export type SelectProps = SelectBase & ValueBinding<string>
 *   export type MultiSelectProps = MultiSelectBase & ValueBinding<string[]>
 *
 * `value` is typed `T | undefined` in the controlled arms so the ubiquitous
 * `value={x ?? undefined}` pattern still type-checks (it just still requires a handler).
 */
export type ValueBinding<T> =
  // Uncontrolled: no `value` (or `defaultValue`); handlers optional.
  | { value?: undefined; defaultValue?: T; onValueChange?: (value: T) => void; onChange?: (value: T) => void }
  // Controlled: `value` present → REQUIRE a handler (onValueChange OR its onChange alias).
  | { value: T | undefined; defaultValue?: undefined; onValueChange: (value: T) => void; onChange?: (value: T) => void }
  | { value: T | undefined; defaultValue?: undefined; onChange: (value: T) => void; onValueChange?: (value: T) => void }

/**
 * Same idea for toggle controls (Switch/Checkbox), which bind on `checked` (with a `value`
 * alias for FormField) and emit via `onCheckedChange` (with an `onChange` alias). Controlled
 * (`checked` set) requires one of the two handlers.
 */
export type CheckedBinding =
  | {
      checked?: undefined
      value?: undefined
      defaultChecked?: boolean
      onCheckedChange?: (checked: boolean) => void
      onChange?: (checked: boolean) => void
    }
  | {
      checked: boolean | undefined
      value?: boolean
      defaultChecked?: undefined
      onCheckedChange: (checked: boolean) => void
      onChange?: (checked: boolean) => void
    }
  | {
      checked: boolean | undefined
      value?: boolean
      defaultChecked?: undefined
      onChange: (checked: boolean) => void
      onCheckedChange?: (checked: boolean) => void
    }
