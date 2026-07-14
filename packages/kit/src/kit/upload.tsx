import * as React from 'react'
import { useSurface } from './surface'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

// legacy Upload (controlled-files subset): a drag-and-drop + click dropzone that hands raw
// File objects to the caller. File list rendering/removal is the caller's concern (render
// `children` or a sibling list). No built-in network upload — the app owns transport.
export type UploadProps = {
  onFiles: (files: File[]) => void
  /** accept attribute, e.g. "image/*,.pdf". */
  accept?: string
  multiple?: boolean
  /** Pick a whole folder (sets `webkitdirectory`); the drop path still yields top-level files. */
  directory?: boolean
  disabled?: boolean
  /** Accessible label for the file input (required — i18n). */
  label: string
  /** Dropzone body (instructions, icon, current file list). */
  children: React.ReactNode
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
  className?: string} & KitStyleProps

export const Upload = React.forwardRef<HTMLInputElement, UploadProps>(function Upload(
  { onFiles, accept, multiple, directory, disabled, label, children, className, style, 'data-testid': testid }, ref,
) {
  const s = useSurface({ disabled })
  const inputRef = React.useRef<HTMLInputElement>(null)
  React.useImperativeHandle(ref, () => inputRef.current as HTMLInputElement)
  const [drag, setDrag] = React.useState(false)
  const locked = s.disabled
  const pick = (list: FileList | null) => {
    if (!list || locked) return
    const files = Array.from(list)
    if (files.length) onFiles(multiple ? files : files.slice(0, 1))
  }
  return (
    // The file <input> is a SIBLING of the role="button" dropzone (not a child):
    // a focusable widget nested inside another interactive element is an axe
    // `nested-interactive` violation. `display:contents` keeps the wrapper boxless
    // so layout is unchanged. The data-testid lives on this wrapper so the input
    // stays a DESCENDANT of the testid'd root (selectors that scope an
    // `input[type=file]` under the Upload testid keep working).
    <div className="contents" data-testid={testid}>
    <div
      role="button"
      tabIndex={locked ? -1 : 0}
      aria-label={label}
      aria-disabled={locked || undefined}
      style={style}
      onClick={() => !locked && inputRef.current?.click()}
      onKeyDown={(e) => { if (!locked && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); inputRef.current?.click() } }}
      onDragOver={(e) => { if (!locked) { e.preventDefault(); setDrag(true) } }}
      onDragLeave={() => setDrag(false)}
      onDrop={(e) => { e.preventDefault(); setDrag(false); pick(e.dataTransfer.files) }}
      className={cn(
        'flex cursor-pointer flex-col items-center justify-center gap-2 rounded-md border border-dashed p-6 text-center text-sm transition-colors',
        'focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50',
        drag && 'border-primary bg-primary/5',
        locked && 'cursor-not-allowed opacity-50',
        className,
      )}
    >
      {children}
    </div>
      <input
        ref={inputRef}
        type="file"
        tabIndex={-1}
        accept={accept}
        multiple={multiple}
        disabled={locked}
        aria-label={label}
        // webkitdirectory is non-standard (not in React's input typings) → spread it.
        {...(directory ? ({ webkitdirectory: '' } as Record<string, string>) : {})}
        className="sr-only"
        onChange={(e) => { pick(e.target.files); e.target.value = '' }}
      />
    </div>
  )
})
