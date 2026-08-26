# Release and validation

## Supported Linux artifact

Linux x86_64 on native Wayland is supported and distributed as an AppImage.
The GPUI build enables Wayland and does not support X11 as a runtime target.
The AppImage build instructions and host-library notes live in
[`packaging/appimage/README.md`](../packaging/appimage/README.md). Linux-only
release behavior includes per-user desktop registration, Wayland title-bar
controls, Freedesktop notifications, and XDG portal/runtime smoke checks.

## Supported macOS artifact

Native macOS arm64 and x86_64 are supported and packaged as a DMG. A native
macOS host can build the local DMG with the system packaging tools and the
instructions in [`packaging/macos/README.md`](../packaging/macos/README.md):

```sh
VERSION=0.1.34-local packaging/macos/build-dmg.sh
```

The script selects the host architecture from `uname`, builds and validates a
signed `Jira Desk.app`, and writes a compressed read-only DMG plus adjacent
SHA-256 checksum under `dist/`. Use `--skip-build` only with an existing native
release binary built for that host. This procedure does not cross-compile and
macOS validation is independent of Linux AppImage validation. macOS uses native
GPUI support, the native keyring feature, native file picker behavior, and
`~/Library/Application Support/dev.jiradesk.JiraDesk` plus
`~/Library/Logs/dev.jiradesk.JiraDesk`; the Freedesktop notification adapter
and test are unavailable, while in-app feed and feedback remain available.

## Linux AppImage build and inspection

Build from Linux with the required Wayland development libraries:

```bash
VERSION=0.1.4-local \
  LINUXDEPLOY=/path/to/linuxdeploy \
  APPIMAGETOOL=/path/to/appimagetool \
  APPIMAGE_RUNTIME=/path/to/pinned/runtime-x86_64 \
  packaging/appimage/build-appimage.sh
```

`APPIMAGE_RUNTIME` is optional for local experiments but should be a pinned,
verified runtime for reproducible builds. The script refuses to overwrite an
existing AppImage or checksum. It produces an AppImage and adjacent SHA-256
checksum. Inspecting an extracted AppImage is supported without FUSE; see the
packaging README for the exact commands.

The packaging script rejects drift in the launcher-neutral `Exec=jira-gpui`
template, named icon, AppStream ID, and desktop launchable ID. On a real
AppImage launch, the application installs an idempotent per-user desktop entry
and icon using the absolute current `APPIMAGE` path before creating the GPUI
window; registration failures are best-effort and do not block startup.
Extracted AppDir smoke runs should verify that registration is skipped when
`APPIMAGE` is absent. The embedded desktop template retains the named
`Icon=dev.jiradesk.JiraDesk`; a stable PNG remains available for named
notification lookup and desktop-entry association, while the generated
per-user entry uses an absolute `dev.jiradesk.JiraDesk-<fingerprint>.png`
path. Icon-content changes therefore change GNOME Shell's cache key; old
managed variants are not automatically deleted, and no desktop-cache updater
is assumed.

The icon build invokes ImageMagick's `magick` command. Ubuntu 22.04 ships
ImageMagick 6, so CI must install the compatibility launcher described in
[`packaging/appimage/README.md`](../packaging/appimage/README.md), or run the
packaging job on Ubuntu 24.04 (where the expected command is available). This
is a packaging prerequisite, not an application runtime dependency.

Generated AppImages and checksum files belong in the local/CI `dist/` output
directory and remain untracked source artifacts. Publish them through the
release workflow or an explicitly selected artifact store; do not add them to
the source commit.

## macOS DMG validation

On the native macOS build host, validate the checksum and inspect the
compressed read-only DMG without launching the app:

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

The native macOS build validates the bundle layout, plist values, icon, and
code signature before creating the DMG. Ad-hoc signing is local validation,
not notarization or a public-release readiness claim. Remove the output DMG
and checksum before rebuilding the same version.

## Validation baseline

Validation is command-based rather than tied to a fixed test-count snapshot.
Run the commands below against the current tree to establish the workspace test
total and validation status; test totals and smoke-artifact versions are expected
to change as the project evolves. The `0.1.4-local` Wayland smoke run noted under
Known limits is retained as historical milestone evidence:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --lib --bins --locked -- -D warnings
git diff --check
```

For a focused GPUI pass on either supported native host:

```bash
RUST_FONTCONFIG_DLOPEN=1 cargo test -p jira-gpui --lib
RUST_FONTCONFIG_DLOPEN=1 cargo check -p jira-gpui --lib
```

## Known limits

The Linux artifact build, extraction checks, checksums, required desktop files,
and library inspection are automated. A local Wayland smoke run was exercised
for the historical `0.1.4-local` milestone. Native macOS build, bundle, code
signature, DMG layout, checksum, and read-only mount checks are documented in
the macOS packaging README but still need broader host coverage. GNOME, KDE,
and wlroots compositor coverage; real Jira permission/account combinations;
Linux notification-daemon delivery; FUSE execution on multiple distributions;
macOS runtime behavior; public OAuth release; and long-running
offline/reconnect behavior still need broader release testing.

The application is intentionally session-oriented: polling runs only while the
process is alive, and the current API-token flow is not suitable for public
distribution. X11 and Windows remain unsupported. Linux AppImage and macOS DMG
artifacts must be built and validated natively on their respective hosts; one
platform's artifact does not validate the other.

### Linux Wayland runtime smoke

The Linux Wayland release smoke should exercise the responsive dashboard at
narrow and wide window sizes, drag the desktop list/detail divider, select
multiple status categories and clear them back to All, verify the issue-list
scrollbar and refresh spinner, and confirm that every manual refresh raises one
in-app summary even when there are no new updates. Verify update timestamps use the
system local timezone with an explicit offset, exercise the Unread and All
feed filters, and confirm generic activity uses compact fallback wording with
progressive disclosure. The refresh summary's desktop counts must be checked
as notifications accepted by the desktop service, not as a guarantee that the
shell displayed them. Verify that OS Freedesktop desktop alerts are still
delivered independently of the in-app notification layer. Verify
display-name-only identity labels, confirm that title-bar minimize, maximize,
and close controls are clearly discoverable with the window idle (hover should
not be required), and confirm that the registered component asset bundle
renders title-bar and semantic icons. Rich-text placeholders and inert links
must remain safe.

Linux update-feed smoke must create multiple detected events for one issue, verify
that they render in one ticket group in newest-first order, and verify the
group's Mark as read action persists every contained event locally without a
Jira request. Jira-edit smoke must verify that the first assignable-user read
uses one bounded empty query, later searches filter the persisted candidate
set locally, and fresh transition choices are reused for 24 hours. It must
also verify that a successful transition invalidates those choices before the
next read, while the separate confirmation step dispatches exactly one
assignment or transition request. Safe definite failures and
refresh-required handling for unknown outcomes remain required.

Linux update-feed smoke should also verify that changed cached/incoming snapshots
use the bounded bulk-changelog read, that history timestamps are filtered to
the snapshot window, and that safe field changes render directly as
`Field: old → new`. A changelog failure or unsupported gateway must leave the
sync successful and show only the generic fallback for the affected issue;
pagination, cancellation, and the eight-page safety cap remain bounded.

Linux media and attachment smoke must cover:

- an unambiguous description image loading as an authenticated Jira thumbnail;
- an IX-1873-like ADF media node containing a Media Services UUID that cannot
  be converted through documented Jira REST, showing the labeled bounded
  allowlisted-attachment gallery without claiming ADF placement, and recording
  the safe unavailable-media diagnostic;
- an ambiguous image reference falling back safely without an arbitrary remote
  Media Services read;
- a thumbnail 404, and separately a bounded unknown-MIME response whose bytes do
  not match an image signature, each rejected as thumbnail bytes and then
  falling back at most once to bounded authenticated original content; the
  original response must still pass strict attachment ID, MIME, nonempty, and
  size preflight. Authentication, transport, non-404 status, malformed-MIME,
  empty, and oversize thumbnail errors do not fall back;
- cached image metadata remaining allowlisted while authenticated thumbnail
  responses using `application/octet-stream` or `image/jpg` are accepted only
  with valid image byte signatures; unsupported MIME and bad signatures remain
  rejected as thumbnail responses, while arbitrary origins, redirects, and
  oversize content remain rejected;
- cancellation before and during thumbnail loading, including stale-selection
  protection;
- per-image 8 MiB, 16-reference, and 32 MiB aggregate rejection boundaries;
- an explicit attachment download showing the Linux XDG portal, cancellation,
  and successful local destination write;
- rejection of oversize or redirected content and verification of downloaded
  contents; and
- isolation between remote Jira/cache state and the selected local file.

No media action should be automatic or mutate Jira. Comments/details and
thumbnails are remote and memory-only, so a restart test should expect cached
issue snapshots but not cached comment bodies or thumbnail bytes. Confirm that
Linux OS Freedesktop alerts remain delivered independently throughout media
loads and local download activity.

Linux image diagnostics smoke should use a fresh temporary `XDG_STATE_HOME`
for the run, preserving any existing user diagnostics logs, then inspect the
JSONL records after exercising an IX-1873-like unresolved ADF Media Services
UUID. Confirm that the record distinguishes each pre-GPUI
failed or pre-GPUI missing state from the safe `gpui_decode_fallback` category,
without exposing any excluded identifiers, URLs, filenames, text, payloads, or
raw errors. Exercise enough image activity to verify rotation leaves no more
than a 256 KiB active log and one 256 KiB backup. Verify the state directory is
`0700` and both log files are `0600`; a missing or unwritable log must not block
startup. Finally, confirm that the existing Linux OS Freedesktop desktop alerts are
unchanged and still delivered independently of diagnostics and in-app
notifications.

### macOS runtime smoke

On a native macOS host, smoke the same core dashboard, sync, detail, confirmed
write, cancellation, and in-app feedback paths. Verify native GPUI window
behavior, native keyring save/load, the native file picker for an explicit
attachment download, and application data/log locations under
`~/Library/Application Support/dev.jiradesk.JiraDesk` and
`~/Library/Logs/dev.jiradesk.JiraDesk`. Do not require per-user Linux desktop
registration, Wayland controls, XDG portals, Freedesktop notifications, or the
Freedesktop notification test: that adapter/test is unavailable on macOS.
