import type { AriaAttributes } from 'react'

/**
 * The kit's closed-prop controls DECLARE the `aria-*` attributes they do not forward.
 *
 * ── THE DEFECT THIS EXISTS FOR ────────────────────────────────────────────────
 *
 * `ClosedSetSelect` (the declarative-form closed-set control) passed `aria-invalid` to the kit
 * `Select`, which names that state `invalid`. The attribute was DROPPED — `Select` destructures a
 * fixed prop list and never spreads a rest — and it type-checked, so nothing anywhere said so.
 * Every closed set in a declarative form therefore rendered its validation message with no
 * programmatic invalid state and none of the kit's invalid styling: visible to a sighted user
 * reading carefully, invisible to assistive tech.
 *
 * ── WHY IT TYPE-CHECKED (not the reason you would guess) ──────────────────────
 *
 * It is tempting to blame the union: `SelectProps` is `SelectBase & ValueBinding<string> &
 * (allowClear union)`, and TypeScript is known to weaken excess-property checking against unions.
 * That is NOT the mechanism here, and it matters, because it means neither a discriminated union
 * nor a key-pinning wrapper generic would have caught it.
 *
 * The mechanism is a JSX rule: **an attribute whose name is not a valid JS identifier — i.e. any
 * HYPHENATED name — is exempt from excess-property checking entirely**, on every component, union
 * or not. That exemption exists so `data-*` and `aria-*` can be put on anything. Measured on this
 * tree with TS 6.0: `<Select … nonHyphenBogus={1} />` is an error today, on the same union, while
 * `<Select … aria-invalid />` and `<Select … totally-bogus-hyphen="x" />` are both accepted. So
 * NON-hyphenated excess props were never the hole; hyphenated ones are the whole hole.
 *
 * An exemption from EXCESS-property checking is not an exemption from type checking. A property
 * that is DECLARED is checked against its declared type. So the only way to make a hyphenated
 * attribute fail is to declare it — with a type nothing can be assigned to. That is what this is.
 *
 * ── WHY THE BAN IS DERIVED, NOT HAND-LISTED ───────────────────────────────────
 *
 * A hand-written list of "the aria attributes this control shadows" is a second encoding of the
 * control's own props, and the two drift: the next control to grow an `invalid` prop gets the
 * shadowing and not the ban. {@link NoUndeclaredAria} instead subtracts the control's DECLARED
 * aria keys from the full ARIA vocabulary (`keyof AriaAttributes`, ~50 finite names), so adding
 * `'aria-foo'?: string` to a base interface un-bans `aria-foo` and removing it re-bans it, with no
 * second place to edit.
 *
 * A blanket `[k: \`aria-${string}\`]: never` index signature would be stronger still and is not
 * expressible: within one object type an index signature must be compatible with every named
 * property, so it would reject the very `aria-label` the control declares. Enumerating React's
 * finite `AriaAttributes` is the exhaustive form that TypeScript can actually hold.
 *
 * ── SCOPE ─────────────────────────────────────────────────────────────────────
 *
 * Only the CLOSED-prop controls (Select, MultiSelect, Combobox, Switch, Checkbox, RadioGroup,
 * Segmented, DatePicker) — the ones whose props type is a hand-written interface and whose
 * implementation destructures a fixed list. `Input`, `Textarea`, `InputNumber` and `Button` build
 * on `React.ComponentProps<…>` and spread the rest onto a real DOM element, so an `aria-*` on one
 * of those genuinely arrives and must stay legal.
 */
export interface UnforwardedAria {
  /**
   * Uninhabited on purpose: nothing can be assigned to `never`, so any value passed for a banned
   * attribute is a compile error naming this type. Reading the error means reading this file.
   *
   * Pass the control's NAMED prop instead — `invalid` (→ `aria-invalid`), `loading` (→ `aria-busy`),
   * `disabled` (→ `aria-disabled`) — or one of the `aria-*` props the control declares.
   */
  readonly __thisAriaAttributeIsNotForwardedByThisKitControl: never
}

/** Every attribute name in React's ARIA vocabulary. Finite, which is what makes the ban below
 *  expressible at all (see the header). */
export type AriaAttributeName = keyof AriaAttributes

/**
 * Ban every `aria-*` that `Declared` does not declare (plus `Also`, for the names a props type
 * carries in a trailing union rather than in its base interface — `MultiSelect`'s required
 * `aria-label` / `aria-labelledby` pair is the one case).
 *
 * Optional (`?:`) rather than required: `aria-x={undefined}` stays legal, so a caller that
 * conditionally spreads an absent attribute is unaffected. Passing a VALUE is the error.
 */
export type NoUndeclaredAria<Declared, Also extends AriaAttributeName = never> = {
  [K in Exclude<AriaAttributeName, Extract<keyof Declared, AriaAttributeName> | Also>]?: UnforwardedAria
}
