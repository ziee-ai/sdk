import * as React from 'react'

// Decision: the kit accepts an inline `style`, but nudges toward Tailwind `className` at
// COMPILE TIME (not via a runtime console.warn — coding agents and CI read tsc, not the
// browser console). `style` is type-gated: to pass it you MUST also pass `allowStyle: true`,
// so an un-acknowledged `style={{...}}` is a tsc error. The fix is either convert it to
// `className`, or opt in explicitly with `allowStyle`.
//
// Usage: `type FooProps = { ...own } & KitStyleProps`  (intersection — KitStyleProps is a
// union, so it can't be `interface extends`). Destructure `style, allowStyle` out before
// spreading onto a DOM node so `allowStyle` never leaks to the DOM.
export type KitStyleProps =
  | { style?: undefined; allowStyle?: undefined }
  | { style: React.CSSProperties; allowStyle: true }
