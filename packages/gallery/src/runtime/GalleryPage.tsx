/**
 * The component gallery — a dev-only canvas rendering every real page/overlay/
 * seeded surface + the kit-component stories, under a URL-driven theme × accent
 * combo. The linchpin of the visual-testing system: Layers A/B/C all run against
 * this one stable surface.
 */
import { Flex, Select, Text, Title } from '@ziee/kit'
import { getGalleryConfig } from './config'
import { GALLERY_DIRS, GALLERY_THEMES } from './matrix'
import { StorySection } from './story'
import { GalleryPages } from './pages'
import { useGalleryTheme } from './useGalleryTheme'

const ctl = (name: string) => `gallery-control-${name}`

function ControlBar() {
  const { accents, accentLabels } = getGalleryConfig()
  const { params, setTheme, setAccent, setDir } = useGalleryTheme()
  return (
    <div
      data-testid="gallery-controls"
      className="sticky top-0 z-10 flex flex-wrap items-end gap-4 border-b border-border bg-background/95 px-6 py-3 backdrop-blur"
    >
      <Title level={2} className="mr-auto">
        Component gallery
      </Title>
      <Flex direction="column" gap="xs">
        <Text tone="muted" className="text-xs uppercase tracking-wide">
          Theme
        </Text>
        <Select
          data-testid={ctl('theme')}
          aria-label="Theme"
          value={params.theme}
          onChange={v => setTheme(v as (typeof GALLERY_THEMES)[number])}
          options={GALLERY_THEMES.map(t => ({ value: t, label: t }))}
        />
      </Flex>
      <Flex direction="column" gap="xs">
        <Text tone="muted" className="text-xs uppercase tracking-wide">
          Accent
        </Text>
        <Select
          data-testid={ctl('accent')}
          aria-label="Accent"
          value={params.accent}
          onChange={v => setAccent(v as string)}
          options={accents.map(a => ({
            value: a,
            label: accentLabels[a] ?? a,
          }))}
        />
      </Flex>
      <Flex direction="column" gap="xs">
        <Text tone="muted" className="text-xs uppercase tracking-wide">
          Direction
        </Text>
        <Select
          data-testid={ctl('dir')}
          aria-label="Direction"
          value={params.dir}
          onChange={v => setDir(v as (typeof GALLERY_DIRS)[number])}
          options={GALLERY_DIRS.map(d => ({ value: d, label: d.toUpperCase() }))}
        />
      </Flex>
    </div>
  )
}

export function GalleryPage({
  surface,
  state,
}: {
  surface?: string
  state?: string
} = {}) {
  const { stories } = getGalleryConfig()
  return (
    <div
      data-testid="gallery-root"
      // A RUNTIME string marker (not a comment — minifiers strip comments) that
      // `check-gallery-prod-exclusion.mjs` greps for in the prod bundle. Reachable
      // only via the DEV-gated dev-gallery lazy import, so it is tree-shaken out
      // of prod; if the gallery ever leaked, this attribute value (a used JSX
      // string literal) survives minification and the check catches it.
      data-gallery-build-marker="ZIEE_GALLERY_SEED_MARKER"
      className="min-h-full w-full overflow-x-hidden bg-background text-foreground"
    >
      <ControlBar />
      <div className="flex flex-col gap-6 p-6">
        {surface ? (
          <GalleryPages only={surface} state={state} />
        ) : (
          <>
            <GalleryPages />
            {(stories ?? []).map(story => (
              <StorySection key={story.id} story={story} />
            ))}
          </>
        )}
      </div>
    </div>
  )
}

export default GalleryPage
