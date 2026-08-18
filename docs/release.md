# Release and validation

## Supported Phase 1 target

Phase 1 is Linux x86_64 on native Wayland, packaged as an AppImage. The GPUI
build enables Wayland and does not support X11 as a runtime target. macOS is
Phase 2. The AppImage build instructions and host-library notes live in
[`packaging/appimage/README.md`](../packaging/appimage/README.md).

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
notification lookup, while the generated per-user entry uses an absolute
`dev.jiradesk.JiraDesk-<fingerprint>.png` path. Icon-content changes therefore
change GNOME Shell's cache key; old managed variants are not automatically
deleted, and no desktop-cache updater is assumed.

The icon build invokes ImageMagick's `magick` command. Ubuntu 22.04 ships
ImageMagick 6, so CI must install the compatibility launcher described in
[`packaging/appimage/README.md`](../packaging/appimage/README.md), or run the
packaging job on Ubuntu 24.04 (where the expected command is available). This
is a packaging prerequisite, not an application runtime dependency.

Generated AppImages and checksum files belong in the local/CI `dist/` output
directory and remain untracked source artifacts. Publish them through the
release workflow or an explicitly selected artifact store; do not add them to
the source commit.

## Validation baseline

The current local milestone has 199 workspace tests and a `0.1.4-local`
Wayland smoke run. The exact count is a snapshot, not a compatibility promise;
run the commands below against the current tree:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo clippy --workspace --lib --bins --locked -- -D warnings
git diff --check
```

For a focused GPUI pass:

```bash
RUST_FONTCONFIG_DLOPEN=1 cargo test -p jira-gpui --lib
RUST_FONTCONFIG_DLOPEN=1 cargo check -p jira-gpui --lib
```

## Known limits

The artifact build, extraction checks, checksums, required desktop files, and
library inspection are automated. A local Wayland smoke run has been exercised
for the current `0.1.4-local` milestone. GNOME, KDE, and wlroots compositor
coverage; real Jira permission/account combinations; notification-daemon
delivery; FUSE execution on multiple distributions; public OAuth release; and
long-running offline/reconnect behavior still need broader release testing.

The application is intentionally session-oriented: polling runs only while the
process is alive, and the current API-token flow is not suitable for public
distribution. No X11, Windows, or macOS artifact should be inferred from the
Linux build.

The release smoke should also exercise the responsive dashboard at narrow and
wide window sizes, drag the desktop list/detail divider, select multiple status
categories and clear them back to All, verify the issue-list scrollbar and
refresh spinner, and confirm that in-app notifications appear. Verify that OS
Freedesktop desktop alerts are still delivered independently of the in-app
notification layer. Verify display-name-only identity labels, confirm that
title-bar minimize, maximize, and close controls are clearly discoverable with
the window idle (hover should not be required), and confirm that the registered
component asset bundle renders title-bar and semantic icons. Rich-text
placeholders and inert links must remain safe.

Update-feed smoke must create multiple detected events for one issue, verify
that they render in one ticket group in newest-first order, and verify the
group's Mark as read action persists every contained event locally without a
Jira request. Jira-write smoke must verify assignable-user and available-
transition loading, the separate confirmation step, exactly one assignment or
transition request, safe definite failures, and refresh-required handling for
unknown outcomes.

Media and attachment smoke must cover:

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
- an explicit attachment download showing the XDG portal, cancellation, and
  successful local destination write;
- rejection of oversize or redirected content and verification of downloaded
  contents; and
- isolation between remote Jira/cache state and the selected local file.

No media action should be automatic or mutate Jira. Comments/details and
thumbnails are remote and memory-only, so a restart test should expect cached
issue snapshots but not cached comment bodies or thumbnail bytes. Confirm that
OS Freedesktop alerts remain delivered independently throughout media loads and
local download activity.

Image diagnostics smoke should use a fresh temporary `XDG_STATE_HOME` for the
run, preserving any existing user diagnostics logs, then inspect the JSONL
records after exercising an IX-1873-like unresolved ADF
Media Services UUID. Confirm that the record distinguishes each pre-GPUI
failed or pre-GPUI missing state from the safe `gpui_decode_fallback` category,
without exposing any excluded identifiers, URLs, filenames, text, payloads, or
raw errors. Exercise enough image activity to verify rotation leaves no more
than a 256 KiB active log and one 256 KiB backup. Verify the state directory is
`0700` and both log files are `0600`; a missing or unwritable log must not block
startup. Finally, confirm that the existing OS Freedesktop desktop alerts are
unchanged and still delivered independently of diagnostics and in-app
notifications.
