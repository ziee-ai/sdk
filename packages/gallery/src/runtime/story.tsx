/**
 * Gallery story contract + layout primitives.
 *
 * A "story" describes one kit component rendered across its variants / states /
 * tones / sizes. The gallery composes every story onto one stable canvas so the
 * visual-testing layers run against a single dev-only surface.
 *
 * Testid convention (the assertion + screenshot targets):
 *   - each story → `gallery-section-<storyId>`
 *   - each case  → `gallery-case-<storyId>-<caseKey>`
 * These ids are COMPUTED (template strings), so they never enter the static
 * testid registry nor the duplicate-literal build gate.
 */
import { Component, type ErrorInfo, type ReactNode } from 'react'
import { Text, Title } from '@ziee/kit'
import { MemoryRouter } from 'react-router-dom'

/** One permutation of a component (a variant × state × size cell). */
export interface GalleryCase {
  /** Stable, story-unique slug, e.g. `primary-sm`. Drives the case testid. */
  key: string
  /** Human label shown above the case. */
  label: string
  /** Renders the component permutation. */
  render: () => ReactNode
}

/** All permutations of one component (or one composite scene). */
export interface GalleryStory {
  /** Stable slug, e.g. `button`. Drives the section testid. */
  id: string
  /** Section heading. */
  title: string
  /** Optional one-line note about what the section covers. */
  note?: string
  /** The permutations. */
  cases: GalleryCase[]
}

export const sectionTestId = (storyId: string) => `gallery-section-${storyId}`
export const caseTestId = (storyId: string, caseKey: string) =>
  `gallery-case-${storyId}-${caseKey}`

/**
 * Renders one story as a labeled section: a heading + a wrap grid of cases.
 * Each case sits in its own bordered cell with a computed testid so the layout
 * layers can localize a diff/violation to a single permutation.
 */
/**
 * One story section's own error boundary.
 *
 * Without it a single throwing case blanks the ENTIRE gallery page — every other
 * story and, because `GalleryPages` renders in the same tree, every seeded
 * surface with it. That is the worst possible failure shape for an auditing
 * tool: one broken case and the harness reports nothing about the other 90-odd
 * cells, with no signal distinguishing "clean" from "never rendered".
 *
 * The error is re-thrown to the console as well as rendered, so the runtime
 * health pass still records it as a console-error finding on this surface — the
 * boundary contains the blast radius, it does not swallow the report.
 */
class StoryErrorBoundary extends Component<
  { storyId: string; children: ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null }
  static getDerivedStateFromError(error: Error) {
    return { error }
  }
  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[gallery story ${this.props.storyId}]`, error, info.componentStack)
  }
  render() {
    if (!this.state.error) return this.props.children
    return (
      <div
        data-testid={`gallery-story-error-${this.props.storyId}`}
        className="rounded border border-destructive p-3"
      >
        <Text tone="muted" className="text-xs">
          This story threw while rendering: {this.state.error.message}
        </Text>
      </div>
    )
  }
}

export function StorySection({ story }: { story: GalleryStory }) {
  return (
    <section
      data-testid={sectionTestId(story.id)}
      className="flex flex-col gap-3 border border-border rounded-lg p-4 bg-background"
    >
      <div className="flex flex-col gap-1">
        <Title level={3}>{story.title}</Title>
        {story.note ? (
          <Text tone="muted" className="text-sm">
            {story.note}
          </Text>
        ) : null}
      </div>
      <StoryErrorBoundary storyId={story.id}>
      <div className="flex flex-wrap gap-4 items-start">
        {story.cases.map(c => (
          <div
            key={c.key}
            data-testid={caseTestId(story.id, c.key)}
            className="flex flex-col gap-2 min-w-[8rem] max-w-full"
          >
            <Text tone="muted" className="text-xs uppercase tracking-wide">
              {c.label}
            </Text>
            <div className="flex flex-wrap gap-2 items-center">
              {/* EVERY case gets its own Router. The real app ALWAYS mounts one,
                  so a component reaching for `useNavigate`/`useLocation`/`<Link>`
                  is correct code — and outside a Router it throws
                  `Cannot destructure property 'basename' of useContext(...)`,
                  which React escalates to the nearest boundary. StorySection
                  sits at the TOP of GalleryPage, so ONE such case took the whole
                  page down: every other story AND every seeded surface on the
                  same page went blank behind the error boundary, and the runtime
                  health pass (which walks `?surface=` cells, each with its own
                  MemoryRouter) reported those surfaces clean while the stories
                  lane was on fire.

                  SeededSurfaceFrame and OverlayFrame in pages.tsx already do
                  this, and OverlayFrame's comment records that it was added for
                  exactly this reason. Stories were the third case and were
                  missed. Per CASE, not per section, so one case cannot leak
                  navigation state into its neighbours. */}
              <MemoryRouter>{c.render()}</MemoryRouter>
            </div>
          </div>
        ))}
      </div>
      </StoryErrorBoundary>
    </section>
  )
}
