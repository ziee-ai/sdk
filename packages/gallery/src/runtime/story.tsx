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
import type { ReactNode } from 'react'
import { Text, Title } from '@ziee/kit'

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
            <div className="flex flex-wrap gap-2 items-center">{c.render()}</div>
          </div>
        ))}
      </div>
    </section>
  )
}
