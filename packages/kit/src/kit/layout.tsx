import * as React from 'react'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

// legacy Layout: structural flex regions. Layout + Header/Sider/Content/Footer subcomponents.
type Div = {
  children?: React.ReactNode
  className?: string
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
} & KitStyleProps

function make(tag: 'header' | 'footer' | 'aside' | 'main' | 'div', base: string, display = 'Layout') {
  const C = ({ children, className, style, 'data-testid': testid }: Div) =>
    React.createElement(tag, { className: cn(base, className), style, 'data-testid': testid }, children)
  C.displayName = display
  return C
}

export type LayoutProps = Div & {
  /** Lay children out in a row (e.g. Sider + Content) instead of a column. */
  hasSider?: boolean
}
function LayoutRoot({ children, className, style, hasSider, 'data-testid': testid }: LayoutProps) {
  return <div className={cn('flex min-h-0 flex-1', hasSider ? 'flex-row' : 'flex-col', className)} style={style} data-testid={testid}>{children}</div>
}
LayoutRoot.displayName = 'Layout'

const Header = make('header', 'flex items-center px-4 h-14 border-b shrink-0', 'Layout.Header')
const Footer = make('footer', 'px-4 py-3 border-t shrink-0', 'Layout.Footer')
const Sider = make('aside', 'shrink-0 border-r overflow-y-auto', 'Layout.Sider')
const Content = make('main', 'flex-1 min-h-0 overflow-auto', 'Layout.Content')

export const Layout = Object.assign(LayoutRoot, { Header, Footer, Sider, Content })
