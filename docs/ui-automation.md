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

Accessibility (AX) and real-window smoke tests are a later layer. This layer
validates the GPUI-native rendered surface without requiring a visible window
or coordinate-driven interaction. Do not put credentials, keychain data, Jira
URLs, or generated PNGs in the repository.
