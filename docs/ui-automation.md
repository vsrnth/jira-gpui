# GPUI UI capture lab

Jira Desk has a development-only, macOS-only screenshot lab for deterministic
visual iteration. It renders named Jira Desk fixture scenarios through GPUI's
offscreen Metal capture API and writes PNG files; it does not use shell
`screencapture`, coordinate automation, or production startup code.

## Single captures

```bash
cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- \
  --scenario issues --output target/ui-lab/issues-light.png \
  --size 1280x900 --theme light

cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- \
  --scenario settings --output target/ui-lab/settings-dark.png \
  --theme dark

cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- \
  --scenario onboarding-dialog --output target/ui-lab/onboarding-dialog-light.png \
  --size 960x700 --theme light
```

Use `--help` for all options and `--list` for the UI capture scenarios:
`onboarding`, `onboarding-dialog`, `issues`, `updates`, `team`, and `settings`.
The `rich-content` scenario is available to the local macOS XCUITest fixture
host, not to the offscreen UI capture lab, so it is intentionally absent from
the capture list and five-case visual matrix below.
`onboarding-dialog` renders the same disconnected fixture with the production
Connect Jira `Dialog` opened through GPUI. It is a single-capture scenario and
is intentionally excluded from the five-case capture matrix below. `--size` is
a logical window size; the command reports the physical PNG dimensions returned
by the renderer. The output directory is created when needed.

## Capture matrix

The built-in matrix is explicit and ordered. It always uses the existing
semantic fixture and capture safety path:

1. `onboarding-light-960x700.png`
2. `issues-dark-1280x900.png`
3. `updates-light-1095x700.png`
4. `team-dark-1370x900.png`
5. `settings-light-960x700.png`

Capture candidates sequentially into an ignored development directory:

```bash
cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- \
  --matrix --output-dir target/ui-lab/candidate
```

This writes only the known candidate PNGs and `matrix-manifest.json` under the
selected directory. Before replacing the first candidate PNG, capture invalidates
that directory's known manifest; a failed or interrupted capture therefore cannot
leave an old manifest describing a partial generation. It does not remove other
files. Candidate output is not an approved baseline; this milestone intentionally
does not ship baseline PNGs.

## Compare and review diffs

Compare requires every known image in both directories and uses strict
`--pixel-threshold 0` and `--max-diff-percent 0` defaults:

```bash
cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- \
  --compare --actual-dir target/ui-lab/candidate \
  --baseline-dir ui/baselines/macos --diff-dir target/ui-lab/diff \
  --report target/ui-lab/report.json
```

A pixel is changed when the maximum absolute delta across its R, G, B, and A
channels is greater than `--pixel-threshold` (0--255). A case passes when its
changed-pixel percentage is less than or equal to `--max-diff-percent` (0--100);
therefore the tolerance boundary is inclusive. PNGs are decoded as RGBA with bounded dimensions, decoder allocation, and file
size before comparison. Both actual and baseline directories must contain a
valid, ordered `matrix-manifest.json`; its schema, matrix metadata, and declared
dimensions are checked. Missing, malformed, or dimension-mismatched PNGs remain
explicit failing case statuses, while an absent or invalid generation manifest
fails before a report is published. The deterministic JSON report has a schema
version and matrix-order case records containing dimensions, changed pixels,
total pixels, percentage, maximum channel delta, status, and (when valid and
changed) a diff filename. Report and diff destinations are validated before any
cleanup. All five known diff filenames are removed before validating either
generation manifest; an absent or invalid manifest therefore fails without
publishing a report. Unrelated files in the diff directory remain. Diff PNGs are
written only for valid, same-dimension images and highlight changed pixels.

The report and diff destinations must be separate from both the baseline and
actual directory trees, including normalized and symlink aliases. The report
and diff names are derived only from the built-in matrix. Comparison never
modifies baseline PNGs or manifests.

## Explicit baseline acceptance

After visual review and explicit user approval, publish a complete candidate
set with the exact confirmation flag:

```bash
cargo run -p jira-gpui --features ui-lab --bin jira-ui-capture -- \
  --accept-baselines --actual-dir target/ui-lab/candidate \
  --baseline-dir ui/baselines/macos --confirm-reviewed
```

Acceptance validates every known candidate PNG and its complete matrix manifest
before touching the baseline directory. It invalidates the known baseline
manifest before replacing any baseline PNG, atomically publishes known PNGs, and
writes the manifest last. Thus the manifest is the validity marker for a complete
baseline generation; this does not claim a transaction across the multiple PNG
files. Existing unrelated files are preserved. If copying or final manifest
publication fails, no valid-looking old baseline manifest remains. Without
`--confirm-reviewed`, acceptance fails without touching baselines. Acceptance is
never inferred from a successful comparison and never silently updates baselines. Git review remains the authority for approving and tracking baseline
PNG changes: no approved baseline ships until a reviewer has inspected and
explicitly accepted the candidates.

The fixture scenarios are explicit, stable constructions in the GPUI adapter.
They reuse the existing sample data and never initialize live Jira workspaces,
credential or keychain loading, network clients, polling, persistence,
downloads, notifications, or Jira write ports.

The lab is intentionally separate from the normal `jira-gpui` binary and macOS
DMG packaging. Build it only during development with `--features ui-lab`.
Linux can type-check the feature, but capture execution reports a clear
macOS-only error.

## Local real-window XCUITest smoke tests

The offscreen capture lab and real-window smoke tests serve different purposes:

1. The capture lab above is the fast, pixel-oriented layer. It uses deterministic
   fixture entities and the offscreen Metal renderer, and does not need a visible
   window or Accessibility permission.
2. The XCUITest layer below launches the development-only fixture host in a real
   macOS window and verifies user-visible workflows through semantic
   accessibility queries. It never uses coordinates, fixed sleeps, keychain
   credentials, Jira, or Jira write controls for pass/fail decisions.

The XCUITest runner is local-only. It is intentionally not a GitHub Actions or
other headless-CI workflow: Xcode, macOS privacy permissions, and the active
desktop session are explicit developer prerequisites.

### Prerequisites

- macOS on a supported arm64 or x86_64 host.
- Rust/Cargo and the repository toolchain.
- Xcode 26.6 (or a compatible newer Xcode) with the macOS SDK and XCTest.
- A logged-in Aqua desktop session. SSH-only, locked, or headless sessions are
  not supported for real-window verification.
- The development-only `ui-automation` feature and
  `jira-ui-automation-host` binary built by the workspace.

XCUITest asks macOS to authorize the test runner through the normal XCTest/Xcode
path. Enable Xcode and the test-running application when macOS lists them in:

`System Settings → Privacy & Security → Accessibility`

Failure screenshots are XCTest attachments and may additionally require:

`System Settings → Privacy & Security → Screen Recording`

Screen Recording is never required for a semantic test to pass. The runner does
not attempt to modify TCC databases or bypass either permission.

### Commands

List the semantic scenarios without building anything:

```bash
tools/macos-ui-automation/run.sh --list
```

Run the complete local smoke suite, or one scenario:

```bash
tools/macos-ui-automation/run.sh --suite
tools/macos-ui-automation/run.sh --scenario onboarding
tools/macos-ui-automation/run.sh --scenario issues
tools/macos-ui-automation/run.sh --scenario rich-content
tools/macos-ui-automation/run.sh --scenario settings
```

Validate the XCUITest project, plist, and scheme without launching the host:

```bash
tools/macos-ui-automation/run.sh --self-test
```

An explicit absolute artifact directory can be supplied when a run needs to be
preserved outside the default ignored location:

```bash
tools/macos-ui-automation/run.sh --suite \
  --artifact-dir /private/tmp/jira-desk-ui-automation/run-001
```

The runner builds the host with
`cargo build -p jira-gpui --features ui-automation --bin jira-ui-automation-host`,
then runs `xcodebuild build-for-testing` for each scenario. It places one
ad-hoc-signed `Jira Desk UI Automation.app` and isolated data/state directories
beside the generated runner in that scenario's
`DerivedData/Build/Products/Debug` directory, then invokes
`xcodebuild test-without-building`. The Swift target derives that products
directory from `Bundle(for:)` and launches the exact app with
`XCUIApplication(url:)`; no environment-variable or scheme-macro forwarding is
needed, and paths containing spaces remain ordinary URL/path values. XCUITest
owns the launched process and terminates it during test teardown.

### Summarize a preserved result bundle

Use `xcresulttool` to inspect one retained scenario result without scraping
`xcodebuild` output. The command returns structured pass/fail JSON:

```bash
/bin/zsh -lc 'xcrun xcresulttool get test-results summary --path target/ui-automation/cache-priority-full-20260831/rich-content/TestResults.xcresult'
```

For another local run, replace the run ID and scenario in the path:

```bash
/bin/zsh -lc 'xcrun xcresulttool get test-results summary --path target/ui-automation/<run-id>/<scenario>/TestResults.xcresult'
```

### Scenarios and artifacts

The fixture host accepts exactly `onboarding`, `issues`, `rich-content`, `updates`,
`team`, and `settings`. The XCUITest target performs bounded semantic waits and
read-only actions:

- `onboarding` opens the connection dialog, selects each field by its exact
  accessibility ID, clicks it for editor focus, and enters the synthetic site
  and email values through the public XCUI application typing API. Each
  character is acknowledged through a bounded value predicate and retried only
  a fixed number of times if necessary. It only requires that the token control
  exists; the token is never typed or read. It then verifies the non-secret
  fields, confirms the fixture Connect control is present without activating
  it, and cancels the dialog. XCTest diagnostics may display the synthetic
  site/email values in local logs or attachments; they are never sent to Jira.
- `issues` selects the deterministic fixture row `issue-row-DESK-179` and
  verifies Story/Task type identity, the normalized `sample` workspace label,
  and bounded-waits for the `issue-detail` accessible title/label to become
  exactly `Issue detail for DESK-179`. It also verifies the semantic
  `issue-detail-details-trigger` control, confirms that the Details accordion
  starts expanded, collapses and reopens it, and checks that its height changes
  and returns within bounded geometry tolerances. The fixture also checks all
  five stable priority identities and labels, and reselects an issue with a
  genuinely empty cached description to ensure it remains ready without a
  loading spinner while background refresh is deferred.
- `rich-content` opens a fixture with a horizontal rule, a valid table, and a
  preloaded PNG image plus canonical Pass/Fail status lozenges. It requires
  the semantic rule/table/image IDs, exact native AX values for the distinct
  status nodes, and asserts that no image spinner or unsupported-content
  sentinel is present. It also includes a comment ADF image whose bytes are
  already cache-ready, proving comment media follows the same persistent,
  cache-first path as description media. The fixture also exercises exact task
  and decision item
  values (`Todo task`, `Done task`, `Decided decision`, and `Undecided
  decision`), expanded and nested-expanded content (`Expanded`, `Details`, and
  `More details`), and the inline emoji/date value `✅ 2026-08-30`. It never
  accesses Jira or persistent storage.
- `settings` starts on the fixture's Settings/Appearance screen and verifies
  that the full `Desktop notifications` label stays within the
  expanded sidebar, activates the nested `Use Dark appearance` CheckBox, and
  then confirms collapsed mode hides expanded labels and keeps the workspace
  icon and sidebar toggle centered in the collapsed rail.
- `updates` navigates to its read-only surface and compares the accessibility
  frames of `update-unread-dot-0` and `update-metadata-0` with a bounded
  vertical tolerance. `team` verifies the `team-table` container.

### Latest validated run

The cache/priority regression run was prepared locally on 2026-08-31. The
deterministic offscreen five-case GPUI matrix captured successfully at
`target/ui-lab/cache-priority-20260831`, including Issues and Settings
candidate images. This does not imply that the separate real-window XCUITest
run passed: its test bodies were blocked by the host timeout described below.
The XCUITest bundle and self-test compiled successfully, but real UI execution was
blocked before test bodies ran by repeated `Timed out while enabling automation
mode` failures. Consequently these scenarios are not claimed as passed. The
preserved diagnostic roots are `target/ui-automation/cache-priority-20260831`
and `target/ui-automation/cache-priority-retry-20260831`.

The complete six-scenario local suite passed on 2026-08-29. Its artifacts are
retained under `target/ui-automation/final-20260829`:

- `onboarding` verified the disconnected credential form without entering or
  reading a token.
- `issues` verified real issue-row type identity and the normalized workspace
  identity, plus the stable `Issue detail for DESK-179` label.
- `rich-content` verified a ready image, horizontal rule, and table with no
  loading spinner or unsupported-content fallback.
- `updates` verified the unread dot against the native metadata midline with a
  tolerance of at most 2 points.
- `team` verified the Team Tracker surface identity.
- `settings` verified the expanded Sidebar and a submenu bounded to 200x180
  points, then verified collapsed navigation hides expanded labels.

The corresponding five-case offscreen visual candidates were reviewed and are
retained under `target/ui-lab/candidate-20260829-regressions`.

### ADF status verification

The `rich-content` scenario was rerun separately on 2026-08-30 for the ADF
status follow-up. It passed with bounded status text and the fixture's green
`Pass` and red `Fail` nodes. All six canonical colors are covered by the
parser/domain tests. XCTest asserted the exact native AX values `Pass` and
`Fail` on distinct status nodes. The existing horizontal-rule,
uneven-table geometry, ready-image, and no-unsupported-fallback assertions
remained active. This was a rich-content-only verification, not a complete
six-scenario suite rerun.

The result bundle and retained screenshot are under
`target/ui-automation/adf-status-final-20260830`. The exported candidate
image is:

`target/ui-automation/adf-status-final-20260830/candidate-export/B8721B09-B686-4AA6-81A9-5B2E48F0B291.png`.

The tests deliberately do not activate status transitions, assignee changes,
comments, saved-login deletion, notification tests, attachment downloads, or
any other Jira write. Site/email are fixed synthetic test data that may appear
only in local XCTest diagnostics; no API token is supplied or read.

### ADF and accordion verification

The rich-content fixture covers the common ADF nodes that previously rendered
as unsupported placeholders: `taskList`/`taskItem` and `blockTaskItem`,
`decisionList`/`decisionItem`, `expand`/`nestedExpand`, `emoji`, and `date`. It
also covers the safe visible label for canonical Jira Cloud Confluence smart
cards. Parser limits, malformed-node handling, and unsupported-node fallback
remain exercised by domain and adapter tests; UI assertions accept only the
bounded semantic values exposed by the fixture.

The issue fixture covers the native gpui-component Accordion around the
existing DescriptionList. The Details section is expanded by default and can
be collapsed and reopened without changing the selected issue or metadata.
The trigger and group use stable semantic identifiers so XCUITest checks
behavior and bounded geometry rather than implementation-specific coordinates.

The latest scenario-specific verification passed on 2026-08-30:

- Issues/Details Accordion: `target/ui-automation/accordion-ax-fix-3/issues/TestResults.xcresult`
- Rich ADF content: `target/ui-automation/adf-accordion-pass2/rich-content/TestResults.xcresult`

The retained rich-content candidate image is
`target/ui-automation/adf-accordion-20260830-pass2-rich-content/rich-content-candidate.png`.
These are local development artifacts; they are not baselines and this was not
a fresh rerun of the complete six-scenario suite.

### Empty ADF cell verification

Official ADF permits a paragraph node with its `content` property omitted. Jira
uses that form for visually blank table cells, so the parser preserves it as a
blank paragraph. Malformed non-array paragraph content and structurally invalid
empty `tableCell` content continue to use the safe unsupported-content
fallback.

The local fixture adds an exact blank accessibility label/value assertion,
positive table-cell geometry with aligned edges within 2 points, and a
no-unsupported-content assertion. This is fixture-based, local-only coverage:
it uses no network, Jira credentials, Jira writes, or CI execution. The passing
rich-content result is retained at
`target/ui-automation/adf-empty-cells-20260831-pass2/rich-content/TestResults.xcresult`;
the candidate image is
`target/ui-automation/adf-empty-cells-20260831-pass2/blank-cells-candidate.png`.
This rich-content-only run does not replace or claim a fresh full-suite run.

### Cache, comment media, priority, and collapsed-rail verification

The 2026-08-31 fixture additions are local-only and deterministic. They use
synthetic issue data, no Jira credentials, no network, no Jira writes, and no
CI execution. Assertions cover the persistent `detail_loaded` marker for empty
descriptions, cache-first comment image bytes, exact five-level priority labels
and identifiers, and collapsed-sidebar control geometry. The XCUITest project
and its self-test passed compilation, while both attempted real-window runs
stopped before the test bodies because macOS repeatedly timed out enabling
automation mode. Keep the two diagnostic roots above when investigating that
host-level failure; they do not constitute passing results.

Each run is written below `target/ui-automation/<run-id>` by default (the
directory is ignored by Git). Each scenario contains:

- `TestResults.xcresult`: XCTest results, activities, and failure attachments;
- `DerivedData`: the disposable UI-testing build products;
- `DerivedData/Build/Products/Debug/Jira Desk UI Automation.app`: the
  ad-hoc-signed fixture host;
- sibling `Jira Desk UI Automation Data` and `Jira Desk UI Automation State`
  directories: isolated XDG roots supplied to the host.

On failure, XCTest keeps a screenshot and a sanitized accessibility debug
description as attachments. They are diagnostic only and are not needed for
pass/fail. The site/email values are synthetic dummy constants; they are never
sent to Jira, and the token field is never populated.

### Accessibility identifiers

GPUI's `.accessibility_id(...)` is the external identifier mapped by AccessKit
to macOS accessibility identifiers. GPUI `.id(...)` and development-only
`.debug_selector(...)` values are not interchangeable with external AX
identifiers. Stable interactive controls used by this runner therefore carry an
explicit accessibility ID and an appropriate role. The Swift tests query those
IDs through `XCUIElement`. Onboarding clicks the matching field and uses
`XCUIApplication.typeKey(_:modifierFlags:)` one character at a time so GPUI's
editor receives each event without burst loss. This uses XCUI's synchronization
between calls and needs no coordinates, pasteboard, tab navigation, private
XCTest APIs, or arbitrary sleeps. All waits are bounded XCTest waits.

### Troubleshooting

- `xcodebuild` reports an authorization failure: enable Xcode and the
  XCUITest/test-running application under Privacy & Security → Accessibility,
  then rerun. Do not edit TCC databases.
- `Jira Desk UI Automation` window not found: use an active desktop session,
  close a stale fixture host, and inspect the corresponding
  `TestResults.xcresult` attachments.
- A screenshot attachment is missing while assertions pass: grant Screen
  Recording if a diagnostic image is needed; this is not a test failure.
- A node is missing: inspect the AX tree, then add or stabilize the corresponding
  `.accessibility_id(...)` in the GPUI surface. Do not replace the query with a
  coordinate or a fixed delay.
- A run is interrupted: XCTest teardown terminates the fixture app it launched.
  Per-scenario artifacts remain available for diagnosis.

Do not put credentials, keychain data, Jira URLs, or generated PNGs in the
repository. The XCUITest runner's temporary data roots and all generated
artifacts remain outside tracked source files. This workflow is deliberately
local-only; it is not run in CI.
