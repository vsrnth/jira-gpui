# Jira Desk AppImage packaging

This directory builds the Phase 1 Linux x86_64 Wayland AppImage. The scaffold follows the [AppImage AppDir specification](https://docs.appimage.org/packaging-guide/manual.html) and uses [linuxdeploy](https://github.com/linuxdeploy/linuxdeploy) plus [appimagetool](https://github.com/AppImage/appimagetool).

## Prerequisites

- A Linux x86_64 host with the Wayland development/runtime libraries required by the GPUI build.
- Rust and Cargo with the repository toolchain available.
- An ImageMagick-compatible `magick` command to render the AppImage root icon as PNG. Local Fedora builds can use ImageMagick 7 directly; Ubuntu 22.04 CI installs ImageMagick 6 and supplies a compatibility launcher.
- `curl`, `sha256sum`, `desktop-file-validate`, `xmllint`, `ldd`, `file`, and `magick` for the setup and validation commands below. On Debian/Ubuntu, these are provided by packages such as `curl`, `coreutils`, `desktop-file-utils`, `libxml2-utils`, `libc-bin`, `file`, and `imagemagick`.
- Pinned, executable `linuxdeploy` and `appimagetool` paths supplied by the caller.
- Optional pinned AppImage runtime passed with `APPIMAGE_RUNTIME`.

Tools and runtimes should be pinned and SHA-256 verified by CI or by the caller. The build script itself does not download tools or runtime files and does not bundle credentials, configuration, or the local cache. If `APPIMAGE_RUNTIME` is omitted, `appimagetool` may fetch its default runtime; release CI must always supply a pinned, verified runtime.

## Reproducible local build

Run these commands from the repository root. They put the pinned tools in a
fresh temporary directory and verify each download before making it
executable. The URLs and SHA-256 values are the official pins used by
`.github/workflows/linux-appimage.yml`:

```sh
set -eu

tool_dir="$(mktemp -d "${TMPDIR:-/tmp}/jira-desk-appimage-tools.XXXXXXXX")"
trap 'rm -rf -- "$tool_dir"' EXIT HUP INT TERM

download_and_verify() {
    url="$1"
    destination="$2"
    expected="$3"
    curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
        "$url" --output "$destination"
    printf '%s  %s\n' "$expected" "$destination" | sha256sum --check --status
    chmod 755 "$destination"
}

download_and_verify \
    'https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage' \
    "$tool_dir/linuxdeploy" \
    'c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d'
download_and_verify \
    'https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage' \
    "$tool_dir/appimagetool" \
    'ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0'
download_and_verify \
    'https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64' \
    "$tool_dir/runtime-x86_64" \
    '1cc49bcf1e2ccd593c379adb17c9f85a36d619088296504de95b1d06215aebbf'

# Leave SKIP_BUILD unset for the normal build. Set SKIP_BUILD=1 only when
# target/release/jira-gpui should be rebuilt first and then reused.
build_args=''
if [ "${SKIP_BUILD:-0}" = 1 ]; then
    cargo build --release --locked -p jira-gpui
    build_args='--skip-build'
fi
VERSION=0.1.34-local \
LINUXDEPLOY="$tool_dir/linuxdeploy" \
APPIMAGETOOL="$tool_dir/appimagetool" \
APPIMAGE_RUNTIME="$tool_dir/runtime-x86_64" \
packaging/appimage/build-appimage.sh ${build_args:+$build_args}
```

Keep this shell open while using `$tool_dir`; its cleanup trap removes only
that exact temporary directory when the shell exits. Set `SKIP_BUILD=1` before
running the block only to select the optional `--skip-build` path. That path
first runs `cargo build --release --locked -p jira-gpui` in the same checkout.
Use a fresh version for each build. The script refuses to overwrite either an
existing AppImage or its adjacent checksum.

The output is `dist/Jira_Desk-${VERSION}-x86_64.AppImage` with an adjacent
SHA-256 checksum. The [release and validation snapshot](../../docs/release.md)
links back here for the build and inspection commands.

The local build exercises packaging on the current host; it is not a claim
that the result is identical to the published release or to CI.

## Independent validation without FUSE

Run this block in a fresh shell from the repository root so its cleanup trap
does not replace the tool-download trap or leave the tool directory behind.
It checks the adjacent checksum, extracts the AppImage in a fresh
temporary directory, validates the packaged files and metadata, and inspects
the actual runtime dependencies. It does not execute the AppImage through
FUSE:

```sh
set -eu

repo_root="$(pwd -P)"
VERSION=0.1.34-local
appimage="$repo_root/dist/Jira_Desk-${VERSION}-x86_64.AppImage"
checksum="$appimage.sha256"
test -f "$appimage"
test -f "$checksum"
(cd "$(dirname "$appimage")" && sha256sum --check "$(basename "$checksum")")

extract_dir="$(mktemp -d "${TMPDIR:-/tmp}/jira-desk-appimage-extract.XXXXXXXX")"
trap 'rm -rf -- "$extract_dir"' EXIT HUP INT TERM
cd "$extract_dir"
"$appimage" --appimage-extract >/dev/null
appdir="$extract_dir/squashfs-root"

test -x "$appdir/usr/bin/jira-gpui"
test -f "$appdir/usr/share/applications/dev.jiradesk.JiraDesk.desktop"
test -f "$appdir/usr/share/metainfo/dev.jiradesk.JiraDesk.metainfo.xml"
test -f "$appdir/usr/share/licenses/jira-gpui/LICENSE"
test -f "$appdir/dev.jiradesk.JiraDesk.png"
test -f "$appdir/usr/lib/libxkbcommon.so.0"
test -e "$appdir/.DirIcon"

desktop-file-validate \
    "$appdir/usr/share/applications/dev.jiradesk.JiraDesk.desktop"
xmllint --noout \
    "$appdir/usr/share/metainfo/dev.jiradesk.JiraDesk.metainfo.xml"
file "$appdir/dev.jiradesk.JiraDesk.png"
magick identify -format '%m %wx%h\n' "$appdir/dev.jiradesk.JiraDesk.png"
test "$(magick identify -format '%wx%h' "$appdir/dev.jiradesk.JiraDesk.png")" = '256x256'

ldd_status=0
if ldd_output=$(
    LD_LIBRARY_PATH="$appdir/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
        ldd "$appdir/usr/bin/jira-gpui" 2>&1
); then
    :
else
    ldd_status=$?
fi
printf '%s\n' "$ldd_output"
test "$ldd_status" -eq 0
if ! printf '%s\n' "$ldd_output" | grep -Fq \
    "libxkbcommon.so.0 => $appdir/usr/lib/libxkbcommon.so.0"; then
    echo 'libxkbcommon.so.0 was not resolved from the extracted AppDir' >&2
    exit 1
fi
if printf '%s\n' "$ldd_output" | grep -Fq 'not found'; then
    echo 'Packaged binary has an unresolved shared-library dependency' >&2
    exit 1
fi
if printf '%s\n' "$ldd_output" | grep -Eq 'libX11|libxcb|libXcursor|libXi|libXrandr|libXinerama|libXrender|libXfixes|libXtst|libxkbcommon-x11'; then
    echo 'Packaged binary links an X11 dependency' >&2
    exit 1
fi

cd "$repo_root"
feature_tree="$(cargo tree --target x86_64-unknown-linux-gnu -e features --locked)"
printf '%s\n' "$feature_tree"
if printf '%s\n' "$feature_tree" | grep -Fq 'gpui_linux feature "x11"'; then
    echo 'gpui_linux x11 feature must not be enabled' >&2
    exit 1
fi
```

The cleanup trap only removes the exact temporary directory created by that
snippet. The AppImage and checksum in `dist/` are left in place for review.

The build renders the source SVG into a 256×256 PNG before calling linuxdeploy. This keeps the root `.DirIcon` compliant with the AppImage specification and avoids generic file icons in file managers that do not load an SVG root icon. The desktop file, icon name, and GPUI Wayland `app_id` are all `dev.jiradesk.JiraDesk`.

### Per-user desktop registration

When launched as an AppImage, the application performs best-effort per-user
desktop registration before creating the GPUI window. The AppImage runtime's
`APPIMAGE` variable supplies the current launcher path; it is canonicalized
and written as the desktop entry's `Exec` value. Extracted AppDirs and other
runs without an absolute `APPIMAGE` path skip registration.

The registration is idempotent: each launch atomically refreshes the same
desktop entry, stable named icon, and content-addressed icon for the current
image. With an absolute `XDG_DATA_HOME`, files are written to:

```text
$XDG_DATA_HOME/applications/dev.jiradesk.JiraDesk.desktop
$XDG_DATA_HOME/icons/hicolor/256x256/apps/dev.jiradesk.JiraDesk.png
$XDG_DATA_HOME/icons/hicolor/256x256/apps/dev.jiradesk.JiraDesk-<fingerprint>.png
```

Otherwise the fallback is `$HOME/.local/share` with the same relative paths.
The stable `dev.jiradesk.JiraDesk.png` copy supports named notification-icon
lookup. The generated desktop entry points to the absolute
content-addressed `dev.jiradesk.JiraDesk-<fingerprint>.png` path, so changing
icon content also changes GNOME Shell's cache key. Older fingerprint variants
are not deleted automatically.
The embedded template remains `Exec=jira-gpui`; only the per-user copy gets
the absolute current-AppImage launcher. The embedded template also remains
`Icon=dev.jiradesk.JiraDesk`; the per-user copy uses the absolute installed
content-addressed PNG path so GNOME Shell can show the icon immediately even
when its named-icon cache is stale. Missing permissions, an unavailable
home/data directory, or a missing bundled icon are non-fatal: registration is
skipped and the application still starts. No credentials, Jira data, or other
host state is written. On GNOME, this registration lets the shell resolve the
Wayland app ID to the desktop entry's human-facing `Name=Jira Desk` and
installed icon, so Alt-Tab and taskbar entries do not show the raw
`dev.jiradesk.JiraDesk` identifier.

To remove the integration, delete the desktop entry, the stable
`dev.jiradesk.JiraDesk.png`, and only the managed
`dev.jiradesk.JiraDesk-<fingerprint>.png` variants in that exact icon
directory (under the active `XDG_DATA_HOME`, or under `$HOME/.local/share`
when it is unset), then refresh the desktop shell if it caches application
metadata. Do not broadly delete unrelated icons. This does not remove the
AppImage or Jira Desk's local database.

The build excludes `libxkbcommon.so.0` from linuxdeploy's ELF rewriting and then copies the exact library linked by the release binary into the AppDir. This avoids a Fedora RELR relocation incompatibility in linuxdeploy's rewriting path. Because the library comes from the build host, AppImages should be built and tested on a compatible Linux distribution/ABI; the script resolves the host path from the binary rather than assuming a distro-specific library directory.
