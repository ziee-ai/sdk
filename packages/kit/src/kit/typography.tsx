import * as React from 'react'
import { Check, Copy } from 'lucide-react'
import { cn } from '../lib/utils'
import { safeHref } from './safe-href'
import { type KitStyleProps } from './style-guard'

const textTone = {
  default: '',
  muted: 'text-muted-foreground',
  destructive: 'text-destructive',
  // legacy `type` aliases:
  secondary: 'text-muted-foreground',
  // Dark-aware AA status tokens, not raw palette hues (which failed AA in dark).
  success: 'text-success',
  warning: 'text-warning',
  danger: 'text-destructive',
} as const

// A copy affordance needs an accessible name → `label` is required (no default, for i18n).
export interface Copyable {
  /** The exact text to copy (required — the kit does not scrape rendered text). */
  text: string
  /** Accessible name for the copy button. */
  label: string
  /** Optional unique data-testid for the copy button (i18n-safe selection). */
  testId?: string
}

function CopyButton({ copyable }: { copyable: Copyable }) {
  const [done, setDone] = React.useState(false)
  const timer = React.useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  React.useEffect(() => () => clearTimeout(timer.current), [])
  return (
    <button
      type="button"
      aria-label={copyable.label}
      data-testid={copyable.testId}
      onClick={() => {
        void navigator.clipboard?.writeText(copyable.text).then(() => {
          setDone(true)
          clearTimeout(timer.current)
          timer.current = setTimeout(() => setDone(false), 1500)
        })
      }}
      className="relative ms-1 inline-flex align-middle opacity-60 hover:opacity-100 focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
    >
      {done ? <Check className="size-3.5" aria-hidden /> : <Copy className="size-3.5" aria-hidden />}
    </button>
  )
}

export type TextProps = Omit<React.ComponentProps<'span'>, 'style'> & {
  /** Kit tones + legacy `type` aliases (secondary/success/warning/danger). */
  tone?: keyof typeof textTone
  /** legacy `type` — alias of `tone`. */
  type?: 'secondary' | 'success' | 'warning' | 'danger'
  /** Bold text (legacy `strong`). */
  strong?: boolean
  /** Inline monospace code styling (legacy `code`). */
  code?: boolean
  /** Single-line truncation with ellipsis (legacy `ellipsis`). */
  ellipsis?: boolean
  /** Adds a copy button (legacy `copyable`). */
  copyable?: Copyable
} & KitStyleProps
// inline-code look (legacy Typography `code`).
const codeCls = 'rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]'
// forwardRef: these are natural children for Tooltip/Popover/Dropdown triggers (asChild),
// which clone the child and attach a ref — a plain function component would drop it.
export const Text = React.forwardRef<HTMLSpanElement, TextProps>(function Text(
  { tone, type, strong, code, ellipsis, copyable, style, allowStyle: _a, className, children, ...props }, ref,
) {
  const t = tone ?? type ?? 'default'
  // With a copy button, truncation must wrap the CONTENT only (else the button is clipped).
  if (copyable != null) {
    return (
      <span ref={ref} style={style} className={cn('inline-flex max-w-full items-center text-sm', textTone[t], strong && 'font-semibold', code && codeCls, className)} {...props}>
        <span className={cn(ellipsis && 'truncate')}>{children}</span>
        <CopyButton copyable={copyable} />
      </span>
    )
  }
  return (
    <span
      ref={ref}
      style={style}
      className={cn('text-sm', textTone[t], strong && 'font-semibold', code && codeCls, ellipsis && 'inline-block max-w-full truncate align-bottom', className)}
      {...props}
    >
      {children}
    </span>
  )
})

export type TitleProps = Omit<React.ComponentProps<'h1'>, 'style'> & {
  level?: 1 | 2 | 3 | 4 | 5
} & KitStyleProps
// Matches shadcn's documented Typography scale (ui.shadcn.com/docs/components/typography):
// h1 text-4xl extrabold · h2 text-3xl · h3 text-2xl · h4 text-xl, all semibold/tracking-tight.
// Level 5 is a kit-only extra step below h4.
const titleCls = {
  1: 'text-4xl font-extrabold tracking-tight text-balance',
  2: 'text-3xl font-semibold tracking-tight',
  3: 'text-2xl font-semibold tracking-tight',
  4: 'text-xl font-semibold tracking-tight',
  5: 'text-lg font-semibold tracking-tight',
} as const
export const Title = React.forwardRef<HTMLHeadingElement, TitleProps>(function Title(
  { level = 1, style, allowStyle: _a, className, ...props }, ref,
) {
  const Tag = `h${level}` as 'h1'
  return <Tag ref={ref} style={style} className={cn(titleCls[level], className)} {...props} />
})

export type ParagraphProps = Omit<React.ComponentProps<'p'>, 'style'> & {
  tone?: keyof typeof textTone
  /** legacy `type` — alias of `tone`. */
  type?: 'secondary' | 'success' | 'warning' | 'danger'
  strong?: boolean
  code?: boolean
  ellipsis?: boolean
  copyable?: Copyable
} & KitStyleProps
export const Paragraph = React.forwardRef<HTMLParagraphElement, ParagraphProps>(
  function Paragraph({ tone, type, strong, code, ellipsis, copyable, style, allowStyle: _a, className, children, ...props }, ref) {
    const t = tone ?? type ?? 'default'
    return (
      <p ref={ref} style={style} className={cn('text-sm leading-relaxed', textTone[t], strong && 'font-semibold', code && codeCls, ellipsis && 'truncate', className)} {...props}>
        {children}
        {copyable != null && <CopyButton copyable={copyable} />}
      </p>
    )
  },
)

export type LinkProps = Omit<React.ComponentProps<'a'>, 'href' | 'style'> & {
  href: string
} & KitStyleProps
export const Link = React.forwardRef<HTMLAnchorElement, LinkProps>(function Link(
  { href, target, rel, style, allowStyle: _a, className, children, ...props }, ref,
) {
  const safe = safeHref(href)
  // A rejected (unsafe) href would otherwise render a styled-but-inoperable anchor —
  // render inert text instead so it isn't announced/styled as a working link.
  if (safe == null) {
    return <span ref={ref as React.Ref<HTMLSpanElement>} style={style} className={className} {...(props as object)}>{children}</span>
  }
  return (
    <a
      ref={ref}
      {...props}
      style={style}
      href={safe}
      target={target}
      // _blank always gets noopener noreferrer — appended to (not replaced by) any caller rel.
      rel={target === '_blank' ? cn('noopener noreferrer', rel) : rel}
      className={cn('text-primary underline-offset-4 hover:underline', className)}
    >
      {children}
    </a>
  )
})
