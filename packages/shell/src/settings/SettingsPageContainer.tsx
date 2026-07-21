import { Text, Title } from '@ziee/kit'
import { ReactNode, useId } from 'react'
import { DivScrollY } from '../components/DivScrollY'
import { appLayoutSeam } from '../app-store-seams'

interface SettingsPageContainerProps {
  title: string | ReactNode
  subtitle?: string
  children: ReactNode
  'data-testid'?: string
}

export function SettingsPageContainer({
  title,
  subtitle,
  children,
  'data-testid': testid,
}: SettingsPageContainerProps) {
  const titleId = useId()
  // In native document-scroll mode (mobile Settings) the WINDOW scrolls, so the
  // inner DivScrollY must NOT create its own scroller — render the same content
  // in normal flow instead, with a bottom inset so the last card clears the iOS
  // home indicator.
  //
  // SEAM: `AppLayout` is an app-registered module store the shell reads through a
  // typed view (never the app's concrete store type); `nativeScroll` is an opt-in
  // mobile flag (off by default), so fall back to `false` in a store-less context
  // (the dev gallery) rather than crashing. Runtime matches reading
  // `Stores.AppLayout.nativeScroll` directly.
  const nativeScroll =
    (appLayoutSeam.get() as { nativeScroll?: boolean } | undefined)
      ?.nativeScroll ?? false

  // Vertical spacing notes:
  //   pt-3 keeps the title close to the top header bar.
  //   mt-3 on the body section is the gap between title and body.
  const inner = (
    <>
      <div className="w-full flex justify-center pt-3">
        <div className={'max-w-4xl w-full flex flex-col gap-2 px-3'}>
          <Title
            level={4}
            id={titleId}
            data-testid={testid ?? 'settings-page-title'}
            className="!m-0 !leading-tight"
          >
            {title}
          </Title>
          {subtitle && (
            <Text type="secondary" className=" !m-0 !p-0 text-sm !leading-tight">
              {subtitle}
            </Text>
          )}
        </div>
      </div>
      <div className={'flex w-full min-w-0 flex-1 justify-center pb-3 mt-3'}>
        {/* min-w-0 lets this shrink below its content's intrinsic width so long
            unbreakable card text truncates instead of widening the page. */}
        <div
          className={'max-w-4xl w-full min-w-0 h-full flex flex-col gap-3 px-3 self-center'}
        >
          {children}
        </div>
      </div>
    </>
  )

  if (nativeScroll) {
    return (
      <div
        className="flex flex-col w-full"
        role="region"
        aria-labelledby={titleId}
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom, 0px) + 12px)' }}
      >
        {inner}
      </div>
    )
  }

  return (
    <DivScrollY className="h-full" role="region" aria-labelledby={titleId}>
      {inner}
    </DivScrollY>
  )
}
