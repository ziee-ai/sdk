import * as React from 'react'
import {
  OverlayScrollbarsComponent,
  type OverlayScrollbarsComponentProps,
  type OverlayScrollbarsComponentRef,
} from 'overlayscrollbars-react'
import 'overlayscrollbars/overlayscrollbars.css'

// ScrollArea built on overlayscrollbars (NOT Radix) — overlay scrollbars that don't take
// layout space and theme via the global .os-theme-* CSS. Supports BOTH axes.
// The host must be size-constrained by the caller (e.g. `h-full` / `max-h-*` / `flex-1` via
// className) — overlayscrollbars scrolls whatever overflows that box. No inner wrapper is
// added (a w-full/flex-col wrapper would suppress horizontal scrolling).
// MIGRATION NOTE (replacing the legacy y-only DivScrollY): pass `axis="y"` and supply your own
// `flex flex-col w-full` content wrapper — DivScrollY baked both in, ScrollArea does not.
type Axis = 'both' | 'y' | 'x'

const overflowFor = (axis: Axis): { x: 'scroll' | 'hidden'; y: 'scroll' | 'hidden' } =>
  axis === 'y' ? { x: 'hidden', y: 'scroll' }
    : axis === 'x' ? { x: 'scroll', y: 'hidden' }
      : { x: 'scroll', y: 'scroll' }

export interface ScrollAreaProps extends Omit<OverlayScrollbarsComponentProps, 'options'> {
  /** Which axes scroll. Default 'both'. */
  axis?: Axis
  /** Scrollbar auto-hide behavior. Default 'scroll' (visible only while scrolling). */
  autoHide?: 'never' | 'scroll' | 'leave' | 'move'
  /** Escape hatch: merged over the derived options (overflow/scrollbars deep-merge by key). */
  options?: OverlayScrollbarsComponentProps['options']
}

export const ScrollArea = React.forwardRef<OverlayScrollbarsComponentRef, ScrollAreaProps>(
  function ScrollArea({ axis = 'both', autoHide = 'scroll', options, children, ...rest }, ref) {
    // deep-merge overflow + scrollbars so a caller `options` adds to (not replaces) the derived
    // axis/autoHide; other option keys come straight from `options`. (`options` may be `false`.)
    const opts = options && typeof options === 'object' ? options : undefined
    const merged: OverlayScrollbarsComponentProps['options'] = {
      ...opts,
      overflow: { ...overflowFor(axis), ...opts?.overflow },
      scrollbars: { autoHide, ...opts?.scrollbars },
    }
    return (
      <OverlayScrollbarsComponent ref={ref} options={merged} {...rest}>
        {children}
      </OverlayScrollbarsComponent>
    )
  },
)
