# Jira Desk AppImage packaging

This directory builds the Phase 1 Linux x86_64 Wayland AppImage. The scaffold follows the [AppImage AppDir specification](https://docs.appimage.org/packaging-guide/manual.html) and uses [linuxdeploy](https://github.com/linuxdeploy/linuxdeploy) plus [appimagetool](https://github.com/AppImage/appimagetool).

## Prerequisites

- A Linux x86_64 host with the Wayland development/runtime libraries required by the GPUI build.
- Rust and Cargo with the repository toolchain available.
- An ImageMagick-compatible `magick` command to render the AppImage root icon as PNG. Local Fedora builds can use ImageMagick 7 directly; Ubuntu 22.04 CI installs ImageMagick 6 and supplies a compatibility launcher.
- Pinned, executable `linuxdeploy` and `appimagetool` paths supplied by the caller.
- Optional pinned AppImage runtime passed with `APPIMAGE_RUNTIME`.

Tools and runtimes should be pinned and SHA-256 verified by CI or by the caller. This scaffold intentionally does not download tools or runtime files and does not bundle credentials, configuration, or the local cache. If `APPIMAGE_RUNTIME` is omitted, `appimagetool` may fetch its default runtime; release CI must always supply a pinned, verified runtime.

## Build

From the repository root:

```sh
LINUXDEPLOY=/opt/tools/linuxdeploy \
APPIMAGETOOL=/opt/tools/appimagetool \
packaging/appimage/build-appimage.sh
```

To reuse an already-built `target/release/jira-gpui`, pass `--skip-build`. To make the runtime explicit and reproducible:

```sh
LINUXDEPLOY=/opt/tools/linuxdeploy \
APPIMAGETOOL=/opt/tools/appimagetool \
APPIMAGE_RUNTIME=/opt/tools/runtime-x86_64 \
packaging/appimage/build-appimage.sh
```

The output is `dist/Jira_Desk-${VERSION}-x86_64.AppImage` with an adjacent SHA-256 checksum. See the [current release and validation snapshot](../../docs/release.md). Checksum, extraction, required-file, and shared-library validation are automated, and a Wayland extract-and-run startup smoke has been exercised. FUSE execution, multi-distribution coverage, real Jira and notification-daemon delivery, and public release remain outstanding. macOS remains Phase 2.

The build renders the source SVG into a 256×256 PNG before calling linuxdeploy. This keeps the root `.DirIcon` compliant with the AppImage specification and avoids generic file icons in file managers that do not load an SVG root icon. The desktop file, icon name, and GPUI Wayland `app_id` are all `dev.jiradesk.JiraDesk`.

Running an AppImage directly does not install its embedded desktop entry or icon into the host desktop's XDG data directories. Therefore, matching `app_id` fixes compositor grouping only; it cannot by itself guarantee a named/iconified taskbar entry on every Wayland desktop. Use a desktop integration tool such as appimaged or AppImageLauncher, or install the desktop file and icon through the distribution, when host-shell integration is required. Jira Desk does not self-register or mutate user desktop state at launch.

The build excludes `libxkbcommon.so.0` from linuxdeploy's ELF rewriting and then copies the exact library linked by the release binary into the AppDir. This avoids a Fedora RELR relocation incompatibility in linuxdeploy's rewriting path. Because the library comes from the build host, AppImages should be built and tested on a compatible Linux distribution/ABI; the script resolves the host path from the binary rather than assuming a distro-specific library directory.
