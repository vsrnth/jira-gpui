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
notification layer. Verify display-name-only identity labels, and confirm that
rich-text placeholders and inert links remain safe. Comments/details are remote
and memory-only, so a restart test should expect cached issue snapshots but not
cached comment bodies.
