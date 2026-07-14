import { forwardRef } from 'react'
import {
  OverlayScrollbarsComponent,
  type OverlayScrollbarsComponentProps,
  type OverlayScrollbarsComponentRef,
} from 'overlayscrollbars-react'

export interface DivScrollYProps
  extends Omit<OverlayScrollbarsComponentProps, 'options'> {
  options?: OverlayScrollbarsComponentProps['options']
  /**
   * Opt this scroller OUT of being an inner scroll box when the page has
   * enabled native document-scroll (mobile). The app-side `DivScrollY` reads
   * the `AppLayout` module store to decide; the kit is domain-agnostic and owns
   * no app store, so here `nativeScroll` is always off and this branch never
   * runs. Kept for prop-shape parity with the app-side component. Off by
   * default, so every scroller keeps its inner scroll unchanged.
   */
  nativeFlow?: boolean
}

/**
 * Kit-local, store-free counterpart of the app's `DivScrollY`
 * (`src-app/ui/src/components/common/DivScrollY.tsx`). The app-side component
 * reads `Stores.AppLayout.nativeScroll` to opt tall mobile scrollers into the
 * document scroll — a domain coupling the kit must not carry. The kit's only
 * consumer (`kit/dialog.tsx`) never passes `nativeFlow`, so the store-driven
 * branch produced no output there; this subset is therefore render-equivalent
 * for the kit's use while keeping the package free of any `@/` app import.
 */
export const DivScrollY = forwardRef<
  OverlayScrollbarsComponentRef,
  DivScrollYProps
>(({ options, className, children, nativeFlow: _nativeFlow, ...restProps }, ref) => {
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
