# UX polish roadmap

## Goal

Make the existing Jira workflows calmer, clearer, and more dependable through
small, coherent UX improvements. This is a roadmap/status record, not a
speculative redesign.

## Restrained native principles

- Use semantic tokens and explicit interaction states rather than one-off
  styling.
- Maintain a clear visual hierarchy and consistency across surfaces.
- Treat accessibility and responsive behavior as correctness requirements.
- Use purposeful animation only when it communicates state or supports an
  interaction; avoid decorative motion.

## Inspiration resources

- [Beautiful UI](https://beautifului.dev/)
- [beui](https://beui.dev/)
- [Rare UI](https://rareui.com/)
- [Transitions](https://transitions.dev/)
- [shadcn/ui](https://ui.shadcn.com/)
- [UI Skills](https://ui-skills.com/)
- [coss.com/ui](https://coss.com/ui)
- [Design System Checklist](https://designsystemchecklist.com/)
- [reui components](https://reui.io/components)
- Emil Kowalski, [“You Don’t Need Animations”](https://emilkowal.ski/ui/you-dont-need-animations)

These are references for restraint, hierarchy, state communication, and
interaction quality—not a mandate to copy visual treatments.

## Invariant safety constraints

- Never expose credentials in UI, diagnostics, logs, errors, or captures;
  redact them at every presentation boundary.
- Jira writes are limited to an explicitly user-confirmed comment, assignment
  change, or status transition through the dedicated write ports.
- Dispatch each confirmed Jira write once: no automatic retries, deletes,
  other issue mutations, or attachment mutations.
- Local cache, preferences, notification state, and sync cursors may be
  written locally; this does not expand Jira write permissions.

## Completed coherent commits

- `780a9e5` — remove redundant title headings.
- `af7baaf` — move Refresh and sync status to the sidebar/mobile status row.
- `4400dc8` — add the selected-issue accent rail.
- `19a9c4b` — improve onboarding validation and accessibility.
- `a5c4cb2` — tailor settings feedback to the host platform.
- `1c6760a` — complete responsive shell and mobile navigation polish.
- `16e980d` — complete Issues/detail loading, error, and responsive hierarchy polish.
- `ff534fd` — complete Updates heading, filter, state, and responsive hierarchy polish.
- `377d7e3` — complete Team Tracker selection, sorting, semantic color, and responsive geometry polish.
- `df3a6bf` — simplify the dashboard shell by removing redundant workspace branding, bounding concise live sync status, and preserving refresh/site context.
- `4632e3a` — make issue-row mouse selection consistent with a focus-visible-only keyboard ring and verify compact transition chooser bounds.
- `8df4758` — distinguish typed Team Tracker refresh failures from successful zero-results empty states while preserving truthful context, accessibility, and cached rows.
- `c279eb2` — complete explicit review and publication of the macOS UX baseline matrix.

## Ordered plan

Each slice is independently reviewed and validated using the gate below, then
committed separately. **All currently ordered slices are complete.**

1. **Completed — Issues/detail:** Make exact-key Jira loading and failure visible
   on mobile; align page hierarchy and selected, empty, loading, and error
   states.
2. **Completed — Updates:** Establish consistent heading, filter, and card
   hierarchy; keep state distinctions readable and responsive without
   decorative motion.
3. **Completed — Team Tracker:** Keep table selection/detail identity consistent
   through sort, filter, and rebuild; align sort and selection semantics, use
   semantic colors, and bound detail geometry.

## Per-slice validation gate

Before each slice is committed:

1. Run formatting (`cargo fmt --all -- --check`).
2. Run focused tests for the changed behavior.
3. Run the full `jira-gpui` library test suite.
4. Run Clippy with `-D warnings` for the relevant library/workspace targets.
5. Check the diff (`git diff --check`) and confirm only intended files changed.
6. Capture relevant macOS UI states where platform behavior or layout is in
   scope.
7. Obtain a fresh P1/P2 review covering behavior, accessibility, responsiveness,
   and the safety constraints above.
8. Commit the slice only after the gate passes.
