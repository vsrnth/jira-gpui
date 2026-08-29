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

Use `--help` for all options and `--list` for the supported semantic scenarios:
`onboarding`, `onboarding-dialog`, `issues`, `updates`, `team`, and `settings`.
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

### Scenarios and artifacts

The fixture host accepts exactly `onboarding`, `issues`, `updates`, `team`, and
`settings`. The XCUITest target performs bounded semantic waits and read-only actions:

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
  bounded-waits for the `issue-detail` accessible title/label to become exactly
  `Issue detail for DESK-179`.
- `settings` starts on the fixture's Settings/Appearance screen, activates the
  nested `Use Dark appearance` CheckBox, and bounded-waits for its selected/value
  state. The stable `appearance-dark` wrapper is also required. The navigation
  container is only required to exist because its
  expanded children make a direct container click ambiguous.
- `updates` and `team` navigate to their read-only surfaces and verify the
  `update-list` and `team-table` containers. Content existence is the stable
  assertion; the AX bridge's immediate selected state is not used.

The tests deliberately do not activate status transitions, assignee changes,
comments, saved-login deletion, notification tests, attachment downloads, or
any other Jira write. Site/email are fixed synthetic test data that may appear
only in local XCTest diagnostics; no API token is supplied or read.

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
