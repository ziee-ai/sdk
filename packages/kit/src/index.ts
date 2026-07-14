// Public UI surface for the app. Import from '@ziee/kit'.
//   shadcn/* = vendored primitives (CLI-generated via `npx shadcn@latest add <c> --overwrite`).
//   kit/*    = our components: shadcn-native types + deliberate additions, a11y-enforced,
//              all reacting to the ambient KitSurface (disabled/loading/readOnly/size).
// See ARCHITECTURE.md.

// cross-cutting
export { KitSurfaceProvider, useSurface, Loading } from './kit/surface'
export type { KitSurface } from './kit/surface'
export { useControllableState } from './kit/use-controllable-state'

// multi-level menu primitives (composable; for nav flyouts / app menubars / right-click menus)
export {
  NavigationMenu, NavigationMenuList, NavigationMenuItem, NavigationMenuContent,
  NavigationMenuTrigger, NavigationMenuLink, NavigationMenuIndicator,
} from './shadcn/navigation-menu'
export {
  Menubar, MenubarMenu, MenubarTrigger, MenubarContent, MenubarItem, MenubarGroup, MenubarLabel,
  MenubarSeparator, MenubarShortcut, MenubarCheckboxItem, MenubarRadioGroup, MenubarRadioItem,
  MenubarSub, MenubarSubTrigger, MenubarSubContent, MenubarPortal,
} from './shadcn/menubar'
export {
  ContextMenu, ContextMenuTrigger, ContextMenuContent, ContextMenuItem, ContextMenuGroup,
  ContextMenuLabel, ContextMenuSeparator, ContextMenuShortcut, ContextMenuCheckboxItem,
  ContextMenuRadioGroup, ContextMenuRadioItem, ContextMenuSub, ContextMenuSubTrigger,
  ContextMenuSubContent, ContextMenuPortal,
} from './shadcn/context-menu'
export { ScrollArea } from './kit/scroll-area'
export type { ScrollAreaProps } from './kit/scroll-area'

// sidebar (composite — provider + collapsible rail + mobile sheet + menu parts)
export {
  Sidebar, SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupAction, SidebarGroupContent,
  SidebarGroupLabel, SidebarHeader, SidebarInput, SidebarInset, SidebarMenu, SidebarMenuAction,
  SidebarMenuBadge, SidebarMenuButton, SidebarMenuItem, SidebarMenuSkeleton, SidebarMenuSub,
  SidebarMenuSubButton, SidebarMenuSubItem, SidebarProvider, SidebarRail, SidebarSeparator,
  SidebarTrigger, useSidebar,
} from './kit/sidebar'
export type { SidebarTriggerProps } from './kit/sidebar'
export { ThemeProvider, useTheme } from './kit/theme'
export type { ThemeProviderProps, ThemeContextValue, ThemePreference, ResolvedTheme } from './kit/theme'

// form engine
export { Form, FormField, useForm, zodResolver } from './kit/form'
export type { FormProps, FormFieldProps } from './kit/form'
// rhf escape hatches + dynamic array fields (parent-form-state access / Form.List analog)
export {
  Controller,
  FormList,
  useFormContext,
  useWatch,
  useFieldArray,
  useFormState,
} from './kit/form'
export type {
  UseFormReturn,
  UseFieldArrayReturn,
  FormListProps,
  FormListRenderProps,
} from './kit/form'

// controls
export { Button } from './kit/button'
export type { ButtonProps } from './kit/button'
export { Input, PasswordInput } from './kit/input'
export type { InputProps, PasswordInputProps } from './kit/input'
export { Textarea } from './kit/textarea'
export type { TextareaProps } from './kit/textarea'
export { Select } from './kit/select'
export type { SelectProps, SelectOption, SelectOptionGroup } from './kit/select'
export {
  SelectTrigger, SelectValue, SelectContent, SelectItem, SelectGroup, SelectLabel, SelectSeparator,
} from './shadcn/select'
export { Switch } from './kit/switch'
export type { SwitchProps } from './kit/switch'
export { Checkbox } from './kit/checkbox'
export type { CheckboxProps } from './kit/checkbox'
export { RadioGroup } from './kit/radio-group'
export type { RadioGroupProps, RadioOption } from './kit/radio-group'
export { Segmented } from './kit/segmented'
export type { SegmentedProps, SegmentedOption } from './kit/segmented'

// layout
export { Flex } from './kit/flex'
export type { FlexProps } from './kit/flex'
export { Space } from './kit/space'
export type { SpaceProps } from './kit/space'
export { Layout } from './kit/layout'
export type { LayoutProps } from './kit/layout'

// style opt-in (inline style is type-gated: passing `style` requires `allowStyle`)
export type { KitStyleProps } from './kit/style-guard'

// searchable / multi select
export { Combobox } from './kit/combobox'
export type { ComboboxProps, ComboboxOption } from './kit/combobox'
export { MultiSelect } from './kit/multi-select'
export type { MultiSelectProps, MultiSelectOption } from './kit/multi-select'

// attachment (file/attachment card primitive — base for FileCard).
// Composable parts (like Select/Dialog parts); compose them at the call site.
export {
  Attachment, AttachmentGroup, AttachmentMedia, AttachmentContent, AttachmentTitle,
  AttachmentDescription, AttachmentActions, AttachmentAction, AttachmentTrigger,
} from './kit/attachment'
export type {
  AttachmentProps, AttachmentGroupProps, AttachmentMediaProps, AttachmentContentProps,
  AttachmentTitleProps, AttachmentDescriptionProps, AttachmentActionsProps,
  AttachmentActionProps, AttachmentTriggerProps,
} from './kit/attachment'

// data display (added)
export { Descriptions } from './kit/descriptions'
export type { DescriptionsProps, DescriptionsItem } from './kit/descriptions'
export { Statistic } from './kit/statistic'
export type { StatisticProps } from './kit/statistic'
export { Image } from './kit/image'
export type { ImageProps, PreviewLabels } from './kit/image'
export { Tree } from './kit/tree'
export type { TreeProps, TreeNode } from './kit/tree'
export { Menu, menuRowClasses, MenuRowButton } from './kit/menu'
export type { MenuProps, MenuItem } from './kit/menu'
export { Upload } from './kit/upload'
export type { UploadProps } from './kit/upload'

// display
export { Badge } from './kit/badge'
export type { BadgeProps, BadgeTone } from './kit/badge'
export { Tag } from './kit/tag'
export type { TagProps, TagTone, TagVariant } from './kit/tag'
export { Skeleton } from './kit/skeleton'
export { List } from './kit/list'
export type { ListProps } from './kit/list'
export { Table } from './kit/table'
export type { TableProps, TableColumn } from './kit/table'
export { Result } from './kit/result'
export type { ResultProps, ResultStatus } from './kit/result'
export { Pagination } from './kit/pagination'
export type { PaginationProps } from './kit/pagination'
export { InputNumber } from './kit/input-number'
export type { InputNumberProps } from './kit/input-number'
// imperative toast + dialog (shadcn-aligned names). Mount <Toaster/> + <DialogHost/> once at root.
export { message, toast, Toaster } from './kit/toast'
export { dialog, DialogHost } from './kit/dialog-host'
export type { ConfirmOptions, AlertOptions } from './kit/dialog-host'
export { Alert } from './kit/alert'
export type { AlertProps, AlertTone } from './kit/alert'
export { Empty } from './kit/empty'
export type { EmptyProps } from './kit/empty'
export { ErrorState } from './kit/error-state'
export type { ErrorStateProps } from './kit/error-state'
export { Avatar } from './kit/avatar'
export type { AvatarProps } from './kit/avatar'
export { Progress } from './kit/progress'
export type { ProgressProps, ProgressTone } from './kit/progress'
export { Separator } from './kit/separator'
export type { SeparatorProps } from './kit/separator'
export { Spinner, Spin } from './kit/spinner'
export type { SpinnerProps, SpinProps } from './kit/spinner'
export { Card } from './kit/card'
export type { CardProps } from './kit/card'
export { SectionHeader } from './kit/section-header'
export type { SectionHeaderProps } from './kit/section-header'
export { Text, Title, Paragraph, Link } from './kit/typography'
export type { TextProps, TitleProps, ParagraphProps, LinkProps } from './kit/typography'

// overlays & nav
export { Tooltip } from './kit/tooltip'
export type { TooltipProps } from './kit/tooltip'
export { Dialog } from './kit/dialog'
export type { DialogProps } from './kit/dialog'
export { Sheet } from './kit/sheet'
export type { SheetProps } from './kit/sheet'
export { Popover } from './kit/popover'
export type { PopoverProps } from './kit/popover'
export { Confirm } from './kit/confirm'
export type { ConfirmProps } from './kit/confirm'
export { Dropdown } from './kit/dropdown'
export type { DropdownProps, DropdownItem } from './kit/dropdown'
export { Tabs } from './kit/tabs'
export type { TabsProps, TabItem } from './kit/tabs'
export { Accordion } from './kit/accordion'
export type { AccordionProps, AccordionItemDef } from './kit/accordion'

// date
export { DatePicker } from './kit/date-picker'
export type { DatePickerProps } from './kit/date-picker'
