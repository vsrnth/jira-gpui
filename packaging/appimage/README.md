# Jira Desk AppImage packaging

This directory builds the Phase 1 Linux x86_64 Wayland AppImage. The scaffold follows the [AppImage AppDir specification](https://docs.appimage.org/packaging-guide/manual.html) and uses [linuxdeploy](https://github.com/linuxdeploy/linuxdeploy) plus [appimagetool](https://github.com/AppImage/appimagetool).

## Prerequisites

- A Linux x86_64 host with the Wayland development/runtime libraries required by the GPUI build.
- Rust and Cargo with the repository toolchain available.
- Pinned, executable `linuxdeploy` and `appimagetool` paths supplied by the caller.
- Optional pinned AppImage runtime passed with `APPIMAGE_RUNTIME`.

Tools and runtimes should be pinned and SHA-256 verified by CI or by the caller. This scaffold intentionally does not download tools or runtime files and does not bundle credentials, configuration, or the local cache. If `APPIMAGE_RUNTIME` is omitted, `appimagetool` may fetch its default runtime; release CI must always supply a pinned, verified runtime.

## Build

From the repository root:

```sh
LINUXDEPLOY=/opt/tools/linuxdeploy \
APPIMAGETOOL=/opt/tools/appimagetool \
VERSION=0.1.0 \
packaging/appimage/build-appimage.sh
```

To reuse an already-built `target/release/jira-gpui`, pass `--skip-build`. To make the runtime explicit and reproducible:

```sh
LINUXDEPLOY=/opt/tools/linuxdeploy \
APPIMAGETOOL=/opt/tools/appimagetool \
APPIMAGE_RUNTIME=/opt/tools/runtime-x86_64 \
packaging/appimage/build-appimage.sh
```

The output is `dist/Jira_Desk-${VERSION}-x86_64.AppImage` with an adjacent SHA-256 checksum. Linux host-library compatibility is intentionally deferred to later Linux CI and release validation. Packaging execution cannot be tested on the current macOS development host. macOS remains Phase 2.
