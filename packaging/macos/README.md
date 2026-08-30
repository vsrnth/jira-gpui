# Jira Desk macOS DMG packaging

This directory builds the supported native macOS artifact. The script supports
only native `arm64` and `x86_64` hosts; it does not cross-compile or package
Linux builds. It does not copy credentials, preferences, Jira data, or cache
files into the application.

## Prerequisites

- macOS with Rust/Cargo and the repository toolchain.
- The system `sips`, `iconutil`, `codesign`, `hdiutil`, `plutil`, and `shasum`
  commands.
- A release binary can be built with Cargo on the host architecture.

## Local build

From the repository root, the default command builds the release binary,
creates `Jira Desk.app`, renders `assets/app-icon/target-target-svgrepo-com.svg`
into `JiraDesk.icns`, ad-hoc signs the app, and creates a compressed read-only
DMG containing the app and an `Applications` shortcut:

```sh
VERSION=0.1.34-local packaging/macos/build-dmg.sh
```

Use an existing `target/release/jira-gpui` only when it was built for the
current native host:

```sh
VERSION=0.1.34-local packaging/macos/build-dmg.sh --skip-build
```

Ad-hoc signing is the default. An explicit local or Developer ID identity can
be selected with `CODESIGN_IDENTITY`:

```sh
CODESIGN_IDENTITY='Developer ID Application: Example (TEAMID)' \
VERSION=0.1.34-local packaging/macos/build-dmg.sh
```

`VERSION` defaults to `0.1.0`, accepts only letters, numbers, `.`, `_`, and `-`,
and is used for artifact naming. The macOS plist fields
`CFBundleShortVersionString` and `CFBundleVersion` use a separate numeric,
dot-separated version with one to three components. By default, the script
derives that value by removing the suffix beginning at the first `-`, so
`VERSION=0.1.34-local` produces `0.1.34` in both plist fields. Set
`BUNDLE_VERSION` to override the derived value; it must match the same format.
If the portion of `VERSION` before the first `-` is not numeric, an override is
required:

```sh
VERSION=0.1.34-local BUNDLE_VERSION=0.1.34 packaging/macos/build-dmg.sh
```

The script refuses an existing `dist/` symlink or non-directory and never
overwrites an existing DMG or checksum. Outputs are:

```text
dist/Jira_Desk-${VERSION}-${arm64|x86_64}.dmg
dist/Jira_Desk-${VERSION}-${arm64|x86_64}.dmg.sha256
```

## Validation

The build validates the bundle layout, generated property list values, icon,
and codesign signature before invoking `hdiutil`.

The `hdiutil create` step retries only when its captured diagnostic contains
the exact `Resource busy` text, with at most three total attempts. Before each
retry it removes only the partial temporary DMG at the requested output path;
the captured diagnostics from every attempt remain visible, and other
`hdiutil` failures are returned immediately. Run the focused local regression
coverage with:

```sh
sh packaging/macos/tests/test-build-dmg.sh
```

Afterward, verify the checksum and inspect the read-only DMG without launching
the app:

```sh
VERSION=0.1.34-local
arch="$(uname -m)"
dmg="dist/Jira_Desk-${VERSION}-${arch}.dmg"
(cd dist && shasum -a 256 --check "$(basename "$dmg").sha256")
hdiutil imageinfo "$dmg" >/dev/null
mount_point="$(mktemp -d "${TMPDIR:-/tmp}/jira-desk-dmg-check.XXXXXXXX")"
attached=$(hdiutil attach -readonly -nobrowse -mountpoint "$mount_point" "$dmg" \
  | awk 'END { print $NF }')
test -d "$mount_point/Jira Desk.app/Contents/MacOS"
test -x "$mount_point/Jira Desk.app/Contents/MacOS/jira-gpui"
test -L "$mount_point/Applications"
hdiutil detach "$attached" >/dev/null
rmdir "$mount_point"
```

Remove the output DMG and checksum before rebuilding the same version. DMG
artifacts remain local/CI outputs and should not be committed.
