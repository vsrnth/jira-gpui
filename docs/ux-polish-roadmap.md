# UX polish roadmap

## Goal

Make the existing Jira workflows calmer, clearer, and more dependable through
small, coherent UX improvements. This is a roadmap/status record, not a
speculative redesign.

## Restrained native principles

- Use semantic tokens and explicit interaction states rather than one-off
  styling.
- Prefer gpui-component primitives for application structure and interaction;
  keep custom layout code only where the component contract does not cover the
  behavior.
- Use component size variants and rem-scaled helpers for ordinary layout.
  Retain pixel values only for native window/capture bounds, component APIs
  that explicitly require `Pixels`, dynamically clamped viewport geometry,
  and deliberately physical indicators.
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

## Current status

### Completed and committed

- **Issue description image caching (`1ae5f58`)** — valid description images
  are served from a bounded, identity-keyed local cache before Jira fetches;
  invalid or unavailable cache entries fall back safely to Jira.

### Completed and validated

- **Issue-type icon semantics (`ff8c2cf`, placement in `70dfb79`)** — dedicated Lucide assets map Story to
  `book-open-text`, Task/standard task to `list-checks`, and Bug/defect to `bug`; other
  types retain generic embedded icons. Icons are attached to type labels across issue rows,
  issue detail metadata, and update cards; issue keys remain text-only and common Jira/JSM
  aliases are covered.
- **Issue-type semantic colors (`32ca4b6`)** — Story, Task, Bug/defect, and Epic
  use blue, green, red, and purple tones respectively; unknown and less common
  types remain neutral. Deterministic local-only macOS coverage is recorded with
  `a9410ed`, `290ea0e`, `ac22576`, and `d5d76bb`.
- **ADF rule and table support (`a8a051b`, `cf0884b`)** — bounded semantic
  tables and standalone horizontal rules are parsed, rendered, and projected
  to plain text, including stretched uneven-height rows and native table,
  row, and cell geometry.
- **ADF smart cards (`8320412`)** — canonical Atlassian browse cards render
  as bounded issue keys while noncanonical and mixed-attribute cards remain
  safe unsupported content.
- **ADF status lozenges (`141da0e`)** — status text is byte-bounded and
  restricted to the six canonical Jira colors. Valid statuses render as
  semantic lozenges in paragraphs and tables; malformed, marked, and
  content-bearing nodes use the bounded unsupported-content fallback.
- **Common ADF content (`f30d407`)** — task and decision lists, expandable
  content, emoji, dates, and canonical Jira Cloud Confluence smart cards now
  have bounded domain representations, safe plain-text projections, and native
  rich-text rendering. Malformed or unsupported structures retain the safe
  fallback instead of leaking raw ADF attributes.
- **Issue details Accordion (`48a62a5`)** — the existing DescriptionList now
  lives inside the gpui-component Accordion, expanded by default and
  collapsible without losing the selected issue or metadata.
- **Comment composer action sizing** — idle, error, and confirmation actions
  use compact buttons in trailing desktop action rows; local GPUI coverage
  bounds the idle `Post comment` control inside the detail pane. The
  authoritative local macOS regression passed on 2026-08-31 at
  `target/ui-automation/comment-action-final-20260831/rich-content/TestResults.xcresult`
  with result `Passed, 1 passed, 0 failed`.
- **Empty ADF table cells (`c24a73f`, `319ddec`)** — omitted paragraph
  content, which official ADF permits and Jira uses for visually blank table
  cells, is preserved as blank content. Malformed non-array paragraph content
  and structurally invalid empty table-cell content retain the safe fallback.
  Local rich-content coverage verifies blank accessibility values and aligned
  table-cell geometry within 2 points.
- **Sidebar workspace identity (`70dfb79`)** — the expanded native Sidebar is 15 rem
  (240 px), and workspace identity is reduced to a safe organization label.
- **Update-card unread alignment (`70dfb79`)** — the unread dot is aligned with the first
  key/type metadata line and has deterministic local layout assertions.
- **Six-scenario local macOS regression matrix (`b9c4f07`)** — passed on 2026-08-29 at
  `target/ui-automation/final-20260829`. It covers `onboarding`, `issues`,
  `rich-content`, `updates`, `team`, and `settings`; the Settings submenu stayed
  within 200x180 bounds, Updates met the native midline tolerance of ≤2 points,
  Issues verified real row type/workspace identity, and rich-content verified a
  ready image plus rule/table without a loading spinner or unsupported fallback.
  The reviewed five-case visual candidates are at
  `target/ui-lab/candidate-20260829-regressions`.
- **Priority icon standardization (`1df6588`)** — all five Jira priority
  levels use the corresponding double/single chevron or equal-bars icon in
  issue rows and detail surfaces, with stable semantic identifiers and exact
  labels for local UI verification.
- **Collapsed sidebar alignment (`66071eb`)** — workspace and toggle controls
  are centered within the collapsed rail and remain bounded away from the
  divider.
- **Persistent empty-detail and comment-image caching (`3b19107`)** — a
  successfully fetched empty description is persisted as loaded, so reopening
  it does not show a spinner; comment ADF images resolve through the exact
  issue attachment catalog and reuse persistent cache-first image bytes.
- **Text-only ADF Markdown descriptions (`b6e8812`)** — conservatively route
  structureless Jira ADF text containing unmistakable block Markdown through
  gpui-component's Markdown TextView, while canonical ADF remains on the
  native renderer. Markdown links remain inert and Markdown/HTML image syntax
  is rejected so all Jira images stay on the authenticated persistent cache
  path. Rust coverage passes. Commit `1cf2f7a` stabilizes the local macOS
  semantic queries against the actual AX contract; the revised rich-content
  scenario passed in the authoritative real-window run at
  `target/ui-automation/markdown-description-verified-20260831/rich-content/TestResults.xcresult`.
  The complete six-scenario committed-tree suite subsequently passed at
  `target/ui-automation/markdown-description-full-suite-20260831`, with each
  saved summary reporting `Passed, 1 passed, 0 failed`.
- **Local regression coverage (`295484c`)** — the consolidated local macOS
  XCUITest suite passed all six scenarios on 2026-08-31 at
  `target/ui-automation/cache-priority-final-20260831`, with each
  `xcresulttool` summary reporting `1 passed, 0 failed`. It stabilizes issue-row
  priority, empty-description cache, and collapsed workspace header
  accessibility semantics, superseding earlier intermediate timeout attempts.
- **Onboarding verification busy state (`92ecc5c`)** — the connection dialog
  presents stable verification/configuration status and keeps its credentials
  fields and actions inert while the asynchronous check is in flight.
  Deterministic local-only macOS coverage was added and stabilized by
  `a9410ed`, `290ea0e`, `11ab2d2`, `ac22576`, and `d5d76bb`.
- **Issue-type colors and onboarding busy validation** — the local-only run at
  `target/ui-automation/onboarding-colors-final-full-20260831` passed
  `onboarding`, `onboarding-busy`, `issues`, `rich-content`, `updates`, and
  `team`; each saved summary reported `1 passed, 0 failed`. Settings initially
  encountered a local testmanager service activation stall and passed after
  per-user service recovery at
  `target/ui-automation/settings-after-testmanager-reset-20260831/settings/TestResults.xcresult`,
  whose saved summary reported `1 passed, 0 failed`. The initial all-in-one
  run was not wholly green because of that Settings stall.
- **Refresh/sidebar polish (`ae330af`, `d1edeca`, `c6176d3`)** — refresh completion uses concise
  “Updated · … issues · … new updates” copy, and the official Lucide
  refresh-cw action is a single icon-only “Refresh Jira” control beside the
  profile. Expanded and collapsed Sidebar layouts keep it bounded,
  non-overlapping, and stacked below the profile in the compact rail.
- **Issue detail and rich links (`0073ea7`)** — change-assignee shares
  the type/status/priority metadata row; detail type and priority groups retain
  stable AX semantics. Safe bounded HTTP(S) links and canonical Jira browse
  links open only in the system browser after explicit clicks, with
  credential-bearing and unsafe schemes rejected.
- **Targeted local verification** — Issues and Rich Content each passed
  (`1 passed, 0 failed`) at
  `target/ui-automation/detail-footer-links-issues10-20260831/issues/TestResults.xcresult`
  and
  `target/ui-automation/detail-footer-links-rich2-20260831/rich-content/TestResults.xcresult`.
  This targeted Issues+Rich Content run was not a full-suite rerun and used
  no network, Jira credentials, Jira writes, or CI; links, refresh, and
  assignee controls were not clicked.

The ADF status follow-up was verified separately through the local
`rich-content` scenario on 2026-08-30. It asserted exact native AX values for
distinct `Pass` and `Fail` status nodes while retaining horizontal-rule and
uneven-table geometry, ready-image, and no-unsupported-fallback checks. This
was not a rerun of the complete six-scenario suite. The result bundle and
retained screenshot are at
`target/ui-automation/adf-status-final-20260830`; the exported candidate is
`target/ui-automation/adf-status-final-20260830/candidate-export/B8721B09-B686-4AA6-81A9-5B2E48F0B291.png`.

The common-ADF and Details Accordion follow-up was verified separately through
the local `issues` and `rich-content` scenarios on 2026-08-30. The issues
scenario asserted the `issue-detail-details-trigger` semantics and bounded
collapse/reopen geometry. The rich-content scenario asserted task, decision,
expand/nested-expand, emoji/date values while retaining the existing
status/table/rule/ready-image/no-spinner/no-unsupported checks. This was not a
rerun of the complete six-scenario suite. Results are retained at
`target/ui-automation/accordion-ax-fix-3/issues/TestResults.xcresult` and
`target/ui-automation/adf-accordion-pass2/rich-content/TestResults.xcresult`;
the candidate image is
`target/ui-automation/adf-accordion-20260830-pass2-rich-content/rich-content-candidate.png`.

The text-only ADF Markdown revision (`b6e8812`, stabilized by `1cf2f7a`) has
deterministic Rust coverage and local-only XCUITest assertions for rendered
headings, delimiter absence, and bounded description geometry. The authoritative
real-window `rich-content` run passed on 2026-08-31 at
`target/ui-automation/markdown-description-verified-20260831/rich-content/TestResults.xcresult`;
its saved `xcresulttool` summary reported `Passed, 1 passed, 0 failed`, and it
retained the `rich-content-final` candidate screenshot. Earlier attempts stopped
before test execution with `Timed out while enabling automation mode`; their
retained roots are
`target/ui-automation/markdown-description-final-20260831`,
`target/ui-automation/markdown-description-retry-20260831`, and
`target/ui-automation/markdown-description-after-testmanager-reset-20260831`.
These earlier roots are diagnostic history only. The verified Markdown/canonical
ADF scenario now passes and does not supersede the separate earlier six-scenario
passing run. The final consolidated six-scenario committed-tree suite then
passed on 2026-08-31 at
`target/ui-automation/markdown-description-full-suite-20260831`; onboarding,
issues, rich-content, updates, team, and settings each reported
`Passed, 1 passed, 0 failed`. This supersedes the targeted-only Markdown
validation as the final evidence for these two commits.

The Team Tracker detail-cache regression is covered by focused GPUI tests and
the local-only `team` fixture. Team-only cached tickets now use the same
cache-first Refreshing state and background-refresh safeguards as Issues.
The authoritative Team run passed on 2026-08-31 after resetting the stalled
per-user testmanager services at
`target/ui-automation/team-detail-cache-20260831/team/TestResults-after-testmanager-reset.xcresult`;
its `xcresulttool` summary reported `Passed, 1 passed, 0 failed`. The first two
attempts timed out before test execution while enabling automation mode; this
passing bundle supersedes them.

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
- `c4be503` — adopt gpui-component's native Sidebar for application navigation.
- `44d3d37` — review and accept the native Sidebar capture baseline.
- `f8ab865` — adopt gpui-component's native Settings surface.
- `a41a588` — review and accept the native Settings capture baseline.
- `fe914d0` — stabilize compact issue-list header geometry.
- `326fab4` — bound and simplify the Sidebar refresh footer.
- `f6ddcf4` — remove the duplicate unread count from the change ledger.
- `6d44f18` — persist bounded issue-detail snapshots so selection paints
  cached content immediately while a guarded background refresh checks Jira.
- `f1d1878` — group Settings content with native setting sections.
- `19b1c03` — present Jira metadata with the native DescriptionList.
- `b03c22c` — place Settings navigation directly in the application Sidebar.
- `7c16fd5` — add privacy-safe macOS Notification Center delivery.
- `92b20ec` — enable explicit macOS notification testing from Settings.
- `6728afd`, `9417221`, `9e11e5d`, `b849eeb` — migrate ordinary shell,
  issue, update, rich-content, onboarding, Settings, and Team geometry to
  component sizing and rem-scaled helpers.
- `14155f9` — replace the status Select with a compact native Popover and List
  that stages a transition for explicit confirmation without writing to Jira.
- `48089a6` — replace the Team tracker's custom drag rail and width math with
  native Resizable panels and session-persistent component state.
- `7dddcc2`, `2a4120c`, `abc94ed`, `e357bcc`, `9a9688e` — complete the
  accessibility hooks and deterministic, no-side-effect fixture host for local
  UI verification.
- `ac8a618` — add the local-only real-window XCUITest harness and five semantic
  smoke scenarios.
- `1ae5f58` — persist bounded issue-description images with cache validation,
  eviction, and migration coverage.
- `a8a051b` — render bounded ADF rules and tables.
- `70dfb79` — clarify issue and Sidebar identity, including type icons,
  workspace labeling, and update-card alignment.
- `ff8c2cf` — add dedicated Lucide issue-type assets, composite asset loading,
  and exact-path mapping coverage.
- `b9c4f07` — extend local macOS regression coverage to six deterministic
  scenarios and validate the rich-content and alignment assertions.
- `8320412` — render canonical Jira ADF smart cards as issue keys with
  strict host/path/attribute validation.
- `cf0884b` — preserve semantic table geometry through stretched uneven-height
  rows and native table, row, and cell accessibility bounds.
- `141da0e` — add bounded canonical ADF status lozenges and safe malformed
  node fallback coverage.
- `f30d407` — render common bounded ADF task, decision, expand, emoji, date,
  and safe Confluence smart-card content.
- `48a62a5` — present issue metadata inside a native, default-expanded
  gpui-component Details Accordion.
- `ac43742` — expose the Accordion trigger with stable macOS accessibility
  semantics.
- `852ce4a` — add local macOS assertions for common ADF values and Accordion
  collapse/reopen geometry.
- `c24a73f` — preserve valid empty ADF paragraphs used by blank Jira table
  cells while retaining safe malformed-content fallback behavior.
- `319ddec` — verify blank-cell semantics and aligned table geometry in the
  local macOS rich-content fixture.
- `1cf2f7a` — stabilize Markdown/canonical ADF rich-content UI assertions
  against the actual macOS accessibility contract.
- `1df6588` — standardize the five Jira priority icons and semantic labels.
- `66071eb` — center collapsed-sidebar workspace and toggle controls.
- `3b19107` — persist loaded empty details and cache comment ADF image bytes.
- `6379508` — add local semantic regression coverage for cache, media,
  priorities, and collapsed-sidebar alignment.
- `92ecc5c` — show onboarding verification progress and keep the connection
  form inert while credentials are verified and configured.
- `32ca4b6` — apply semantic colors to common Jira issue types.
- `a9410ed`, `290ea0e`, `11ab2d2`, `ac22576`, `d5d76bb` — add and stabilize
  deterministic local macOS coverage for onboarding busy behavior and issue
  type-color surfaces.

## Ordered plan

Each slice is independently reviewed and validated using the gate below, then
committed separately. The original ordered slices and the follow-up slices
recorded in **Current status** above are complete and validated.

1. **Completed — Issues/detail:** Make exact-key Jira loading and failure visible
   on mobile; align page hierarchy and selected, empty, loading, and error
   states.
2. **Completed — Updates:** Establish consistent heading, filter, and card
   hierarchy; keep state distinctions readable and responsive without
   decorative motion.
3. **Completed — Team Tracker:** Keep table selection/detail identity consistent
   through sort, filter, and rebuild; align sort and selection semantics, use
   semantic colors, and bound detail geometry.
4. **Completed — Native shell and settings:** Use the native Sidebar, Settings,
   TitleBar integration, Toggle-based appearance controls, and component asset
   provider while keeping Settings navigation in the application Sidebar.
5. **Completed — Issue detail interactions:** Paint persisted, bounded detail
   snapshots immediately; refresh in the background; use DescriptionList for
   metadata and a Popover/List for explicitly confirmed status transitions.
6. **Completed — Desktop integration:** Deliver privacy-safe macOS
   notifications, expose explicit notification testing, and use native
   Resizable panels for Team table/detail geometry.
7. **Completed — Design-guide sizing audit:** Replace ordinary fixed pixel
   spacing and dimensions with component sizes or rem-scaled helpers across the
   GPUI adapter. The remaining pixel values are limited to:
   - native Resizable and Settings pane size/range APIs;
   - DataTable column measurement APIs;
   - native window, viewport-test, and offscreen-capture bounds;
   - dynamically clamped mobile/navigation geometry; and
   - the deliberately physical 3px selected-issue rail.
8. **Completed — Local semantic macOS UI automation:** Verify deterministic
   real-window XCUITest coverage for onboarding, issue/detail identity, updates,
   team, and settings without Jira, keychain, notification, or write side
   effects.
9. **Completed and validated — Six-scenario local macOS regression coverage:**
   The complete `onboarding`, `issues`, `rich-content`, `updates`, `team`, and
   `settings` suite passed on 2026-08-29. Results are retained at
   `target/ui-automation/final-20260829`.
10. **Completed — Common ADF rendering and issue metadata disclosure:**
    Parse and render bounded task/decision/expand/emoji/date content, retain a
    safe Confluence smart-card label, and place the existing issue DescriptionList
    in a default-expanded Accordion. Focused local macOS verification passed on
    2026-08-30; the full six-scenario suite remains represented by the
    2026-08-29 run above and was not rerun for this slice.
11. **Completed — Valid empty ADF paragraphs:** Preserve official ADF
    paragraphs with omitted content as blank Jira table cells, while keeping
    malformed paragraph and structurally invalid empty-cell input on the safe
    fallback path. Rich-content-only local macOS verification passed on
    2026-08-31; it did not rerun the complete six-scenario suite.
12. **Completed — Detail/media cache and Jira priority polish:** Persist the
    loaded state of genuinely empty details, refresh cached detail and comment
    media in the background, standardize all five priority icons, and align
    collapsed-sidebar controls. The consolidated local macOS suite passed all
    six scenarios on 2026-08-31 in
    `target/ui-automation/cache-priority-final-20260831` (`295484c`).

## Latest UI verification

The deterministic offscreen five-case GPUI matrix was captured successfully on
2026-08-31 at `target/ui-lab/cache-priority-20260831`, including Issues and
Settings candidate images. The final consolidated local macOS XCUITest suite
also passed all six scenarios on 2026-08-31 at
`target/ui-automation/cache-priority-final-20260831`; each scenario reported
`1 passed, 0 failed` in its `xcresulttool` summary. This supersedes earlier
intermediate timeout attempts.

The previously accepted five-case macOS visual matrix was regenerated after
the status Popover, sizing audit, and Team Resizable migration by following
`docs/ui-automation.md`. Issues, Team, Settings, onboarding, and local updates
were visually reviewed at their canonical matrix sizes. The final five-scenario
real-window XCUITest suite also passed for onboarding, issues/detail identity,
updates, team, and settings. No clipping, overlap, pane-boundary, or semantic
interaction regressions were found. Candidate PNGs and XCUITest results remain
ignored local development artifacts and were not published as new baselines.

The six-scenario local macOS regression extension passed on 2026-08-29 at
`target/ui-automation/final-20260829`. It covered real issue-row type/workspace
identity, a ready image plus horizontal rule/table with no loading or
unsupported fallback, Updates native midline alignment within 2 points, Team
Tracker identity, and Settings submenu bounds of 200x180. The reviewed visual
candidate matrix is retained at
`target/ui-lab/candidate-20260829-regressions`.

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
