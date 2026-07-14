import * as React from 'react'
import {
  Attachment as BaseAttachment,
  AttachmentGroup as BaseAttachmentGroup,
  AttachmentMedia as BaseAttachmentMedia,
  AttachmentContent as BaseAttachmentContent,
  AttachmentTitle as BaseAttachmentTitle,
  AttachmentDescription as BaseAttachmentDescription,
  AttachmentActions as BaseAttachmentActions,
  AttachmentAction as BaseAttachmentAction,
  AttachmentTrigger as BaseAttachmentTrigger,
} from '../shadcn/attachment'

// File/attachment card primitive — the base for FileCard. Composable parts (like Select/Dialog
// parts). Presentational parts pass straight through; the two INTERACTIVE parts force an
// accessible name at the type level (no default — caller owns the string for i18n), matching the
// rest of the kit. State/orientation/size handle upload status, row/square, and density.
export type AttachmentProps = React.ComponentProps<typeof BaseAttachment>
export const Attachment = BaseAttachment

export type AttachmentGroupProps = React.ComponentProps<typeof BaseAttachmentGroup>
export const AttachmentGroup = BaseAttachmentGroup

export type AttachmentMediaProps = React.ComponentProps<typeof BaseAttachmentMedia>
export const AttachmentMedia = BaseAttachmentMedia

export type AttachmentContentProps = React.ComponentProps<typeof BaseAttachmentContent>
export const AttachmentContent = BaseAttachmentContent

export type AttachmentTitleProps = React.ComponentProps<typeof BaseAttachmentTitle>
export const AttachmentTitle = BaseAttachmentTitle

export type AttachmentDescriptionProps = React.ComponentProps<typeof BaseAttachmentDescription>
export const AttachmentDescription = BaseAttachmentDescription

export type AttachmentActionsProps = React.ComponentProps<typeof BaseAttachmentActions>
export const AttachmentActions = BaseAttachmentActions

// Action button has no text → an accessible name is REQUIRED.
export type AttachmentActionProps = React.ComponentProps<typeof BaseAttachmentAction> & {
  'aria-label': string
}
export function AttachmentAction(props: AttachmentActionProps) {
  return <BaseAttachmentAction {...props} />
}

// Full-card overlay button. When it renders its own <button> (not asChild) it has no text, so an
// accessible name is REQUIRED; with asChild the child element supplies the name.
type TriggerBase = Omit<React.ComponentProps<typeof BaseAttachmentTrigger>, 'asChild'>
export type AttachmentTriggerProps =
  | (TriggerBase & { asChild: true; 'aria-label'?: string })
  | (TriggerBase & { asChild?: false; 'aria-label': string })
  // dynamic asChild (e.g. asChild={hasTooltip}) — name always required so there's no nameless hole.
  | (TriggerBase & { asChild: boolean; 'aria-label': string })
export function AttachmentTrigger(props: AttachmentTriggerProps) {
  return <BaseAttachmentTrigger {...props} />
}
