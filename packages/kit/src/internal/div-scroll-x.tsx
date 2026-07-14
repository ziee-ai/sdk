import { forwardRef } from 'react'
import {
  OverlayScrollbarsComponent,
  type OverlayScrollbarsComponentProps,
  type OverlayScrollbarsComponentRef,
} from 'overlayscrollbars-react'

export interface DivScrollXProps
  extends Omit<OverlayScrollbarsComponentProps, 'options'> {
  options?: OverlayScrollbarsComponentProps['options']
  /** Classes on the inner content row (the flex holding the scrollable items). */
  contentClassName?: string
}

/**
 * Horizontal counterpart of {@link DivScrollY}: an OverlayScrollbars viewport
 * that scrolls on the X axis (Y hidden) with the app's auto-hide scrollbar
 * styling, instead of a native `overflow-x-auto` box. The inner row is
 * `min-w-full` so `justify-content` on `contentClassName` can center the items
 * when they fit and fall back to scroll when they overflow. The viewport
 * carries `data-overlayscrollbars-viewport`, which the Drawer swipe-to-close
 * guard already treats as a horizontal scroller (so swiping the row doesn't
 * close the drawer).
 */
export const DivScrollX = forwardRef<
  OverlayScrollbarsComponentRef,
  DivScrollXProps
>(({ options, className, contentClassName, children, ...restProps }, ref) => {
  const mergedOptions = {
    scrollbars: { autoHide: 'scroll' as const },
    overflow: { y: 'hidden' as const },
    ...options,
  }

  const mergedClassName = ['overflow-x-auto', className]
    .filter(Boolean)
    .join(' ')

  const innerClassName = ['flex flex-row items-center min-w-full', contentClassName]
    .filter(Boolean)
    .join(' ')

  return (
    <OverlayScrollbarsComponent
      ref={ref}
      options={mergedOptions}
      className={mergedClassName}
      {...restProps}
    >
      <div className={innerClassName}>{children}</div>
    </OverlayScrollbarsComponent>
  )
})
