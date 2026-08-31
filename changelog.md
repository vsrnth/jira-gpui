# Changelog

This changelog records user-visible capabilities and the completed UX work in
Jira Desk. The `Unreleased` section records changes on the current unreleased
branch; it is not a published release.

## Unreleased

### Added

- Jira ADF tables now have a bounded, transport-neutral representation with
  rows, cells, and header-cell semantics. The GPUI rich-text view renders them
  as readable bordered tables and projects their contents to plain text
  (`a8a051b`).
- Standalone ADF rules are represented as semantic horizontal-rule blocks and
  render as bounded dividers.
- Rich-content rendering now keeps horizontal rules and tables semantic through
  GPUI layout, including uneven-height stretched rows and native table, row,
  and cell geometry (`cf0884b`).
- Jira ADF status nodes now accept bounded visible text and six canonical colors
  (neutral, purple, blue, red, yellow, and green), rendering as semantic
  lozenges in paragraphs and table cells. Malformed, marked, or content-bearing
  status nodes fall back to the bounded unsupported-content representation
  (`141da0e`).
- Jira ADF inline and block smart cards for canonical
  `https://<site>.atlassian.net/browse/<KEY>` links render as issue keys without
  retaining or exposing URLs. Noncanonical URLs and mixed smart-card
  attributes are rejected (`8320412`).
- Common Jira ADF content now has bounded representations and native rendering
  for task lists (`taskList`, `taskItem`, and `blockTaskItem`), decision lists,
  expandable sections (`expand` and `nestedExpand`), emoji, and dates. Canonical
  Jira Cloud Confluence cards render as a safe visible label without exposing
  their URL path or query (`f30d407`).
- Issue metadata is now grouped in a native gpui-component Accordion around the
  existing DescriptionList. It is expanded by default and can be collapsed and
  reopened while preserving the selected issue (`48a62a5`).
- Valid empty ADF paragraphs with omitted `content` are preserved as blank
  content. This handles Jira's visually blank table cells while malformed
  non-array paragraph content and structurally invalid empty `tableCell`
  content retain the safe unsupported-content fallback (`c24a73f`).
- Jira issue-type semantics now cover common Jira and JSM labels and aliases,
  including story, task and sub-task variants, bug/defect, epic, initiative,
  spike, improvement/new feature, incident/problem, change, and service
  request. Unknown types keep a neutral fallback icon.
- Common issue types now use app-owned Lucide assets: Story → `book-open-text`,
  Task/standard task → `list-checks`, and Bug/defect → `bug`. The composite
  app `AssetSource` serves these paths while delegating the rest of the icon
  catalog to gpui-component (`ff8c2cf`).
- Jira priorities now use a complete five-level icon vocabulary: double-up for
  Highest, up for High, equal bars for Medium, down for Low, and double-down
  for Lowest. The mapping is exposed in issue rows and detail surfaces with
  stable semantic priority identifiers (`1df6588`).

### Changed

- Issue-type icons are associated with the visible type label in issue rows,
  issue-detail metadata, and update cards. Issue keys remain text-only so the
  task check icon cannot be mistaken for a status indicator (`70dfb79`).
- Issue rows and update cards include the source issue-type label in their
  accessibility names. The detail type surface exposes `Issue type: …`, while
  its parent keeps the stable, key-based label `Issue detail for {key}` used by
  local automation.
- The local update unread indicator is aligned to the first key/type metadata
  line with component spacing; a deterministic layout assertion bounds the
  alignment.
- The expanded native Sidebar allocation is 15 rem (240 px), keeping longer
  navigation labels readable. Workspace identity text is reduced to a safe
  site label rather than exposing URL paths or credentials.
- macOS DMG creation retries only failures whose captured `hdiutil` diagnostic
  contains the exact `Resource busy` text, for at most three attempts. Each
  retry removes only the partial temporary DMG and preserves the command's
  diagnostics (`9d5f44d`).
- Collapsed-sidebar workspace and toggle controls are centered within the
  physical rail, with bounded geometry coverage so the controls do not drift
  toward the divider (`66071eb`).
- Empty issue descriptions are treated as a loaded, cacheable detail state.
  Cached detail snapshots paint immediately while a guarded background refresh
  checks Jira; comment ADF image references resolve against the exact issue
  attachment catalog and image bytes reuse the persistent cache-first media
  path (`3b19107`).

### Security

- ADF table parsing is bounded by row, cell, and depth limits, and malformed
  structures remain represented as safe placeholders.
- ADF status text and color values are allowlisted and bounded; malformed,
  marked, or content-bearing nodes never expose raw attributes or content.
- Smart-card parsing accepts only canonical Atlassian browse links with
  matching attributes; rejected links never become rendered URL content.
- Common ADF parsing is bounded by depth, item, text, and nesting limits. Invalid
  task, decision, expand, emoji, date, and smart-card nodes retain the safe
  unsupported-content fallback; raw ADF attributes are not rendered.
- Empty paragraph and table-cell handling distinguishes official valid ADF from
  malformed structures: omitted paragraph content is blank, while non-array
  paragraph content and structurally invalid empty cells never become rendered
  raw data.
- Workspace labels only expose a validated site slug or hostname fallback;
  credentials, paths, queries, and fragments are not displayed.

### Testing

- Added pure mapping coverage for issue-type aliases and neutral fallbacks.
- Added exact-path mapping tests for the three dedicated Lucide icons and
  asset-source tests covering loading and built-in asset delegation.
- Added rich-text table projection and horizontal-rule rendering coverage.
- Added local semantic verification for canonical ADF status lozenges,
  including exact native AX values for distinct `Pass` and `Fail` nodes.
- Added local semantic verification for task/decision item values,
  expand/nested-expand labels, and the exact `✅ 2026-08-30` emoji/date flow.
  The same fixture retains rule, table, status, ready-image, no-spinner, and
  no-unsupported-content assertions.
- Added local macOS verification for the `issue-detail-details-trigger`
  Accordion: default-expanded state, collapse/reopen behavior, and bounded
  group-height geometry. The focused result bundles passed on 2026-08-30 at
  `target/ui-automation/accordion-ax-fix-3/issues/TestResults.xcresult` and
  `target/ui-automation/adf-accordion-pass2/rich-content/TestResults.xcresult`.
  This was not a rerun of the complete six-scenario suite.
- Added fixture-based local macOS rich-content coverage for blank ADF table
  cells: exact blank accessibility label/value, positive and aligned cell
  geometry within 2 points, and no unsupported-content fallback. The passing
  result is `target/ui-automation/adf-empty-cells-20260831-pass2/rich-content/TestResults.xcresult`
  with candidate image
  `target/ui-automation/adf-empty-cells-20260831-pass2/blank-cells-candidate.png`
  (`319ddec`). This run used no network, Jira credentials, Jira writes, or CI
  and was rich-content-only rather than a full-suite rerun.
- Added local semantic verification for stretched uneven-height ADF tables,
  table/row/cell geometry, and canonical smart-card issue-key rendering. The
  fixture-only macOS artifact is
  `target/ui-automation/adf-cards-value-escalated-20260829`.
- The status verification retained table geometry, ready-image, and
  unsupported-fallback assertions. Its local rich-content result and screenshot
  are retained at
  `target/ui-automation/adf-status-final-20260830`, with the exported
  candidate at
  `target/ui-automation/adf-status-final-20260830/candidate-export/B8721B09-B686-4AA6-81A9-5B2E48F0B291.png`.
- Added local GPUI geometry coverage for update-card unread-dot alignment.
- Added local GPUI and macOS semantic coverage for all five priority labels,
  collapsed-sidebar centering, cached empty descriptions, comment image
  readiness, and the absence of a detail spinner on repeated issue selection
  (`6379508`).
- Captured the deterministic offscreen five-case GPUI matrix successfully on
  2026-08-31 at `target/ui-lab/cache-priority-20260831`, including the Issues
  and Settings candidate images. This capture result is separate from the
  real-window XCUITest run, which was blocked by the host automation timeout.
- Added deterministic local shell regression coverage for the bounded DMG
  retry behavior, including transient success, non-transient failure, retry
  exhaustion, partial-DMG cleanup, and diagnostic preservation:
  `packaging/macos/tests/test-build-dmg.sh` (`9d5f44d`).
- Extended the local-only macOS fixture suite to six deterministic scenarios
  (`onboarding`, `issues`, `rich-content`, `updates`, `team`, and `settings`);
  the complete run passed on 2026-08-29. Artifacts are retained under
  `target/ui-automation/final-20260829` (`b9c4f07`).

The validated six-scenario run covered real issue-row type and workspace
identity, ready rich content (image, rule, and table with no loading or
unsupported fallback), Updates unread-dot/native-metadata midline alignment
within 2 points, and Team Tracker identity. Settings also kept its submenu
within bounded 200x180 geometry. The reviewed five-case visual candidates are
under `target/ui-lab/candidate-20260829-regressions`.

## Image caching milestone (`1ae5f58`)

### Added

- Persist bounded Jira description images in the local cache, keyed by site,
  issue, and attachment identity.
- Serve valid cached images before fetching Jira, validate cached media before
  use, and fall back to Jira when the cache is unavailable or invalid.
- Bound cached image count, per-image bytes, and total bytes with eviction and
  migration coverage.

### Security

- Cached media is restricted by attachment identity, supported image signature,
  MIME type, and configured size limits. Jira writes remain limited to the
  explicitly confirmed write ports.

## Local macOS automation milestone (`ac8a618`, with semantic hooks in `7dddcc2`, `e357bcc`, `9a9688e`)

### Added

- A local-only, fixture-based real-window XCUITest host and runner.
- Five deterministic semantic smoke scenarios covering onboarding,
  issue/detail identity, updates, Team Tracker, and Settings.
- Stable accessibility identifiers and bounded visual assertions for local UI
  verification without Jira credentials, network access, or Jira writes.

The six-scenario extension and its UI follow-up assertions are validated locally
on 2026-08-29 (`b9c4f07`); see the Unreleased testing notes and
`target/ui-automation/final-20260829`.

## Completed UX milestones

### Responsive shell and navigation (`1c6760a`, `af7baaf`, `780a9e5`)

- Completed responsive desktop/mobile shell behavior and mobile navigation.
- Moved refresh and sync status into the Sidebar/mobile status row.
- Removed redundant title headings and preserved clear workspace context.

### Issues, Updates, and Team Tracker (`16e980d`, `ff534fd`, `377d7e3`)

- Made issue loading, refreshing, empty, and failure states explicit and
  responsive.
- Established a readable Updates heading, filter, card, and state hierarchy.
- Stabilized Team Tracker selection, sorting, semantic colors, and detail
  geometry.

### Native shell and settings (`c4be503`, `f8ab865`, `48089a6`)

- Adopted the native gpui-component Sidebar and Settings surfaces.
- Kept Settings navigation in the application Sidebar and integrated native
  TitleBar behavior.
- Replaced custom Team Tracker pane sizing with native Resizable panels and
  session-persistent component state.

### Cached detail and guarded interactions (`6d44f18`, `19b1c03`, `14155f9`)

- Persisted bounded issue-detail snapshots so cached content paints immediately
  while Jira refreshes in the background.
- Presented issue metadata with the native DescriptionList.
- Replaced status selection with a compact Popover/List that stages a status
  transition for explicit confirmation.

### Desktop integration and onboarding (`7c16fd5`, `92b20ec`, `19a9c4b`, `a5c4cb2`)

- Added privacy-safe macOS Notification Center delivery and explicit local
  notification testing.
- Added credential validation before connection and host-appropriate Settings
  feedback.
