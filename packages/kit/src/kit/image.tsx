import * as React from 'react'
import { ZoomIn, ZoomOut, RotateCw, RefreshCw } from 'lucide-react'
import { Dialog as Root, DialogTrigger, DialogContent, DialogTitle } from '../shadcn/dialog'
import { safeImgSrc } from './safe-href'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

// Image: a safe <img> (src gated to image-safe schemes — content can be untrusted) with an
// optional click-to-open preview that supports zoom (wheel + buttons), pan (drag) and rotate.
// `alt` is required; with preview on, `previewLabel` + control labels are required (i18n).
export interface PreviewLabels {
  zoomIn: string
  zoomOut: string
  rotate: string
  reset: string
}
type ImageBase = {
  src: string
  alt: string
  width?: number | string
  height?: number | string
  fallback?: React.ReactNode
  className?: string
} & KitStyleProps
export type ImageProps =
  | (ImageBase & { preview?: false; previewLabel?: never; previewLabels?: never; previewOpen?: never; onPreviewOpenChange?: never })
  | (ImageBase & {
      preview: true
      previewLabel: string
      previewLabels: PreviewLabels
      /** Controlled preview open state (omit for uncontrolled click-to-open). */
      previewOpen?: boolean
      onPreviewOpenChange?: (open: boolean) => void
    })

const clamp = (n: number, lo: number, hi: number) => Math.min(Math.max(n, lo), hi)

// Zoom/pan/rotate viewer — mounted fresh when the dialog opens, so transform state resets per open.
function PreviewViewer({ src, alt, labels }: { src: string; alt: string; labels: PreviewLabels }) {
  const [scale, setScale] = React.useState(1)
  const [rot, setRot] = React.useState(0)
  const [t, setT] = React.useState({ x: 0, y: 0 })
  const [grabbing, setGrabbing] = React.useState(false)
  const drag = React.useRef<{ x: number; y: number } | null>(null)
  const stageRef = React.useRef<HTMLDivElement>(null)
  const reset = () => { setScale(1); setRot(0); setT({ x: 0, y: 0 }) }
  // Native non-passive wheel listener so we can preventDefault (stops the dialog/page co-scrolling).
  React.useEffect(() => {
    const el = stageRef.current
    if (!el) return
    const onWheel = (e: WheelEvent) => { e.preventDefault(); setScale((s) => clamp(s * (e.deltaY < 0 ? 1.1 : 0.9), 0.5, 8)) }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [])
  const endDrag = () => { drag.current = null; setGrabbing(false) }
  return (
    <div className="flex flex-col gap-2">
      <div
        ref={stageRef}
        className="relative flex h-[70vh] items-center justify-center overflow-hidden"
        onPointerDown={(e) => { if (scale <= 1) return; drag.current = { x: e.clientX - t.x, y: e.clientY - t.y }; setGrabbing(true); (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId) }}
        onPointerMove={(e) => { if (drag.current) setT({ x: e.clientX - drag.current.x, y: e.clientY - drag.current.y }) }}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        style={{ cursor: scale > 1 ? (grabbing ? 'grabbing' : 'grab') : 'default' }}
      >
        <img
          src={src}
          alt={alt}
          draggable={false}
          className="max-h-full w-auto select-none"
          style={{ transform: `translate(${t.x}px, ${t.y}px) scale(${scale}) rotate(${rot}deg)`, transition: drag.current ? 'none' : 'transform 120ms' }}
        />
      </div>
      <div className="flex items-center justify-center gap-1">
        <button type="button" aria-label={labels.zoomOut} onClick={() => setScale((s) => clamp(s * 0.8, 0.5, 8))} className="rounded-md p-2 hover:bg-accent"><ZoomOut className="size-4" aria-hidden /></button>
        <button type="button" aria-label={labels.zoomIn} onClick={() => setScale((s) => clamp(s * 1.25, 0.5, 8))} className="rounded-md p-2 hover:bg-accent"><ZoomIn className="size-4" aria-hidden /></button>
        <button type="button" aria-label={labels.rotate} onClick={() => setRot((r) => r + 90)} className="rounded-md p-2 hover:bg-accent"><RotateCw className="size-4" aria-hidden /></button>
        <button type="button" aria-label={labels.reset} onClick={reset} className="rounded-md p-2 hover:bg-accent"><RefreshCw className="size-4" aria-hidden /></button>
      </div>
    </div>
  )
}

export function Image({ src, alt, width, height, fallback, className, style, ...rest }: ImageProps) {
  const [failed, setFailed] = React.useState(false)
  const safe = safeImgSrc(src)
  const preview = (rest as { preview?: boolean }).preview
  const previewLabel = (rest as { previewLabel?: string }).previewLabel
  const previewLabels = (rest as { previewLabels?: PreviewLabels }).previewLabels
  const previewOpen = (rest as { previewOpen?: boolean }).previewOpen
  const onPreviewOpenChange = (rest as { onPreviewOpenChange?: (o: boolean) => void }).onPreviewOpenChange
  if (safe == null || failed) {
    return <span className={cn('inline-flex items-center justify-center bg-muted text-muted-foreground', className)} style={{ width, height, ...style }} role="img" aria-label={alt}>{fallback}</span>
  }
  const img = (
    <img
      src={safe}
      alt={alt}
      width={width}
      height={height}
      onError={() => setFailed(true)}
      className={cn('max-w-full', preview && 'cursor-zoom-in', className)}
      style={style}
    />
  )
  if (!preview) return img
  return (
    <Root open={previewOpen} onOpenChange={onPreviewOpenChange}>
      <DialogTrigger render={<button type="button" aria-label={previewLabel} className="inline-block">{img}</button>} />
      <DialogContent className="max-w-4xl" aria-describedby={undefined}>
        <DialogTitle className="sr-only">{previewLabel}</DialogTitle>
        <PreviewViewer src={safe} alt={alt} labels={previewLabels!} />
      </DialogContent>
    </Root>
  )
}
