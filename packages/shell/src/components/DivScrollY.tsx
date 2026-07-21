import { forwardRef } from 'react'
import {
  OverlayScrollbarsComponent,
  type OverlayScrollbarsComponentProps,
  type OverlayScrollbarsComponentRef,
} from 'overlayscrollbars-react'
import { appLayoutSeam } from '../app-store-seams'

export interface DivScrollYProps
  extends Omit<OverlayScrollbarsComponentProps, 'options'> {
  options?: OverlayScrollbarsComponentProps['options']
  /**
   * Opt this scroller OUT of being an inner scroll box when the page has
   * enabled native document-scroll (mobile). When set AND
   * `AppLayout.nativeScroll` is active, render a plain flow container so the
   * WINDOW scrolls (iOS toolbar collapse + content under the notch) instead of
   * this box capturing the scroll. Off by default, so every other DivScrollY
   * (drawers, desktop panes, nested modals) keeps its inner scroll unchanged.
   */
  nativeFlow?: boolean
}

export const DivScrollY = forwardRef<
  OverlayScrollbarsComponentRef,
  DivScrollYProps
>(({ options, className, children, nativeFlow, ...restProps }, ref) => {
  // SEAM: `AppLayout` is an app-registered module store; in a store-less
  // context (the dev gallery, or any consumer that mounts this before the
  // app-layout module registers) it's undefined. `nativeScroll` is an opt-in
  // mobile flag (off by default), so fall back to an empty object rather than
  // crashing the whole subtree. Read via a typed view so the shell doesn't
  // depend on the app's app-layout store type; runtime is identical to reading
  // `Stores.AppLayout` directly.
  const { nativeScroll } =
    (appLayoutSeam.get() as { nativeScroll?: boolean } | undefined) ??
    {}

  if (nativeFlow && nativeScroll) {
    // Same flex wrapper as the scroller path (below) minus overflow-y-auto, so
    // content flows into the document scroll instead of an inner box.
    const flowClassName = ['flex', className].filter(Boolean).join(' ')
    return (
      <div className={flowClassName}>
        <div className="flex flex-col w-full">{children}</div>
      </div>
    )
  }

  const mergedOptions = {
    scrollbars: { autoHide: 'scroll' as const },
    ...options,
  }

  const mergedClassName = ['overflow-y-auto', 'flex', className]
    .filter(Boolean)
    .join(' ')

  return (
    <OverlayScrollbarsComponent
      ref={ref}
      options={mergedOptions}
      className={mergedClassName}
      {...restProps}
    >
      <div className="flex flex-col w-full">{children}</div>
    </OverlayScrollbarsComponent>
  )
})
