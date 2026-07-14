import * as React from 'react'
import { Avatar as Base, AvatarImage, AvatarFallback } from '../shadcn/avatar'
import { cn } from '../lib/utils'
import { safeImgSrc } from './safe-href'

const sizes = { sm: 'size-6 text-xs', default: 'size-9 text-sm', lg: 'size-12 text-base' } as const

type Common = { fallback?: React.ReactNode; size?: keyof typeof sizes; className?: string }
// If an image src is provided, `alt` is REQUIRED (a meaningful name for the image).
export type AvatarProps =
  | (Common & { src: string; alt: string })
  | (Common & { src?: undefined; alt?: never })

export function Avatar(props: AvatarProps) {
  const { fallback, size = 'default', className } = props
  // src can be attacker-controlled (LLM/MCP content) — gate it to image-safe schemes (once).
  const safe = props.src != null ? safeImgSrc(props.src) : undefined
  return (
    <Base className={cn(sizes[size], className)}>
      {safe != null && <AvatarImage src={safe} alt={props.alt} />}
      <AvatarFallback>{fallback}</AvatarFallback>
    </Base>
  )
}
