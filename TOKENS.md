# Content-column tokens

These are the typography and spacing tokens for the note column
(`:root` R15 block and the content-colour rows on `body` in `ui/style.css`).
opensidian picks every value here from three rules of its own:

- **Spacing unit.** `--space-unit: 0.25rem` (4px at the default 16px root).
  Every margin, indent, radius, rule thickness and width below is a whole or
  half multiple of it.
- **Type scale.** A modular scale with ratio r = 1.12. Headings run from
  h6 = r¹ to h1 = r⁶, so every heading is larger than body text and each
  level is one step above the next. Values are rounded to 3 decimals.
- **Heading rhythm.** Line height falls by 0.06 and tracking tightens by
  0.003em per level going up from h6, so larger headings set tighter.
  h6 starts at line height 1.45 and tracking -0.003em.

Colours are wired to opensidian's own layer-2/layer-3 names, so a theme that
reaches the base palette still reaches them.

## Per-token derivation

Body text
- `--font-smaller: 0.9em` (secondary text, one tenth below body)
- `--line-height-normal: 1.55` (body leading chosen for a 45rem column)
- `--line-height-tight: 1.25` (UI and compact rows, 0.3 under body)
- `--bold-modifier: 250` (regular 400 + 250 = 650: semibold-plus on variable fonts)
- `--bold-weight: min(900, calc(var(--font-weight) + var(--bold-modifier)))` (clamped to the CSS weight range)

Heading sizes (r = 1.12, hₙ = r^(7−n))
- `--h1-size: 1.974em` (r⁶)
- `--h2-size: 1.762em` (r⁵)
- `--h3-size: 1.574em` (r⁴)
- `--h4-size: 1.405em` (r³)
- `--h5-size: 1.254em` (r²)
- `--h6-size: 1.12em` (r¹)

Heading line heights (1.45 − 0.06 × (6 − n))
- `--h1-line-height: 1.15`
- `--h2-line-height: 1.21`
- `--h3-line-height: 1.27`
- `--h4-line-height: 1.33`
- `--h5-line-height: 1.39`
- `--h6-line-height: 1.45`

Heading tracking (−0.003em × (7 − n))
- `--h1-letter-spacing: -0.018em`
- `--h2-letter-spacing: -0.015em`
- `--h3-letter-spacing: -0.012em`
- `--h4-letter-spacing: -0.009em`
- `--h5-letter-spacing: -0.006em`
- `--h6-letter-spacing: -0.003em`

Vertical spacing (u = `--space-unit`)
- `--p-spacing: calc(u × 5)` (1.25rem between blocks)
- `--p-spacing-empty: u` (an empty paragraph keeps one unit)
- `--heading-spacing: calc(u × 8)` (2rem above a heading that follows a block)

Lists
- `--list-indent: calc(u × 7)` (1.75rem per nesting level)
- `--list-indent-editing: calc(u × 2)` (extra hang in the live editor)
- `--list-spacing: calc(u / 2)` (half a unit between items)
- `--list-bullet-size: calc(u × 1.5)` (6px dot)
- `--list-bullet-radius: calc(var(--list-bullet-size) / 2)` (half the size: a circle)
- `--list-marker-color: var(--text-chrome-muted)`

Checkboxes
- `--checkbox-size: calc(u × 3.5)` (14px, sits inside the cap height of body text)
- `--checkbox-radius: calc(u × 0.75)` (3px)
- `--checkbox-margin-inline-start: calc(u × 3)` (12px)
- `--checkbox-border-color: var(--text-chrome-muted)`
- `--checkbox-color: var(--bg-button-primary)`
- `--checkbox-marker-color: var(--bg-base)`
- `--checklist-done-decoration: line-through 0.06em` (strike with an explicit thickness)
- `--checklist-done-color: var(--text-chrome-faint)`

Code and quotes
- `--code-size: 0.92em` (monospace runs wide; 0.92 matches its x-height to body text)
- `--code-radius: calc(u × 1.5)` (6px)
- `--code-background: var(--bg-surface)`
- `--blockquote-border-thickness: calc(u × 0.75)` (3px rule)
- `--blockquote-border-color: var(--text-accent)`

Column
- `--file-line-width: calc(u × 180)` (45rem reading measure)
