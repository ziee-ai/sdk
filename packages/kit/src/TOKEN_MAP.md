# legacy token → Tailwind class map

Decision: **no `useToken()` shim.** Every legacy `theme.useToken()` read (`token.colorX`) is
converted to a Tailwind class at the call site. shadcn's default palette is kept — legacy tokens
with no shadcn variable map to the nearest existing var/class (noted below).

Mechanics: a legacy `style={{ background: token.colorBgContainer }}` becomes a `className`
(`bg-card`), merged into any existing `className` via `cn(...)`. For a border *color* use the
border-color utility (`border-border`); pair with a width utility (`border`) where the element
isn't already bordered.

| legacy token (observed) | uses | Tailwind class | note |
|---|---|---|---|
| `colorBgContainer` | 23 | `bg-card` | container/surface bg |
| `colorBorderSecondary` | 23 | `border-border` | default border color |
| `colorBgLayout` | 11 | `bg-background` | page/layout bg |
| `colorTextBase`, `colorText` | 10 | `text-foreground` | |
| `colorPrimary` | 8 | `text-primary` / `bg-primary` | by context (fg vs fill) |
| `colorPrimaryBg` | 6 | `bg-accent` | nearest tint (no primary-tint var) |
| `colorPrimaryHover` | 1 | `hover:bg-primary/90` | |
| `colorTextSecondary` | 5 | `text-muted-foreground` | |
| `colorTextTertiary`, `colorTextQuaternary`, `colorTextDescription` | 5 | `text-muted-foreground` | shadcn has one muted fg |
| `colorFillSecondary` | 5 | `bg-muted` | |
| `colorFillTertiary` | 3 | `bg-muted/60` | |
| `colorFillQuaternary` | 2 | `bg-muted/40` | |
| `borderRadiusLG` | 4 | `rounded-lg` | |
| `borderRadius` | — | `rounded-md` | |
| `colorWhite`, `colorTextLightSolid` | 3 | `bg-white` / `text-white` | literal white (theme-invariant) |
| `colorError`, `colorErrorBorder`, `colorErrorBg` | 3 | `text-destructive` / `border-destructive` / `bg-destructive/10` | |
| `colorWarning` | 2 | `text-amber-500` | no warning var (kept shadcn defaults) |
| `colorInfoBorder`, `colorInfoBg` | 2 | `border-blue-500/30` / `bg-blue-500/10` | no info var |
| `motionDurationMid` | 1 | `duration-200` | |
| `fontSize` | 1 | `text-sm` | base 14px == text-sm |

Anything not listed: pick the nearest semantic shadcn class (`bg-popover`, `text-card-foreground`,
`ring-ring`, `rounded-md`) — do NOT introduce new CSS variables (decision: keep shadcn defaults).
