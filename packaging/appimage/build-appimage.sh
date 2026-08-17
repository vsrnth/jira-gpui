#!/bin/sh
set -eu

usage() {
    printf '%s\n' "Usage: LINUXDEPLOY=/path/to/linuxdeploy APPIMAGETOOL=/path/to/appimagetool [VERSION=0.1.0] $0 [--skip-build]" >&2
}

skip_build=0
if [ "$#" -gt 1 ]; then
    usage
    exit 2
elif [ "$#" -eq 1 ]; then
    if [ "$1" = "--skip-build" ]; then
        skip_build=1
    else
        usage
        exit 2
    fi
fi

case "$(uname -s)" in
    Linux) ;;
    *) printf '%s\n' "AppImage packaging requires a Linux host" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    x86_64|amd64) ;;
    *) printf '%s\n' "AppImage packaging requires Linux x86_64" >&2; exit 1 ;;
esac

: "${LINUXDEPLOY:?Set LINUXDEPLOY to a pinned linuxdeploy executable}"
: "${APPIMAGETOOL:?Set APPIMAGETOOL to a pinned appimagetool executable}"
for tool in "$LINUXDEPLOY" "$APPIMAGETOOL"; do
    if [ ! -f "$tool" ] || [ ! -x "$tool" ]; then
        printf '%s\n' "Tool must be an executable file: $tool" >&2
        exit 1
    fi
done

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
version=${VERSION:-0.1.0}
case "$version" in
    ''|*[!A-Za-z0-9._-]*) printf '%s\n' "VERSION contains unsupported characters" >&2; exit 1 ;;
esac

binary="$project_root/target/release/jira-gpui"
icon_source="$project_root/assets/app-icon/dev.jiradesk.JiraDesk.svg"
apprun="$project_root/packaging/appimage/AppRun"
desktop="$project_root/packaging/appimage/dev.jiradesk.JiraDesk.desktop"
metainfo="$project_root/packaging/appimage/dev.jiradesk.JiraDesk.metainfo.xml"
license="$project_root/LICENSE"
output_dir="$project_root/dist"
output="$output_dir/Jira_Desk-${version}-x86_64.AppImage"

for input in "$desktop" "$icon_source" "$apprun" "$metainfo" "$license"; do
    [ -f "$input" ] || { printf '%s\n' "Missing packaging input: $input" >&2; exit 1; }
done

if [ "$skip_build" -eq 0 ]; then
    (cd "$project_root" && cargo build --release --locked -p jira-gpui)
fi
[ -f "$binary" ] && [ -x "$binary" ] || {
    printf '%s\n' "Missing executable release binary: $binary" >&2
    exit 1
}
ldd_output=$(ldd "$binary") || {
    printf '%s\n' "Could not inspect shared libraries for release binary: $binary" >&2
    exit 1
}
host_libxkbcommon=$(printf '%s\n' "$ldd_output" | awk '
    $1 == "libxkbcommon.so.0" && $3 ~ /^\// { print $3; exit }
    $1 ~ /\/libxkbcommon\.so\.0$/ { print $1; exit }
')
case "$host_libxkbcommon" in
    /*) ;;
    *)
        printf '%s\n' "Could not resolve an absolute libxkbcommon.so.0 path from: $binary" >&2
        exit 1
        ;;
esac
[ -f "$host_libxkbcommon" ] && [ -r "$host_libxkbcommon" ] || {
    printf '%s\n' "Resolved libxkbcommon.so.0 is not a readable file: $host_libxkbcommon" >&2
    exit 1
}

if [ -L "$output_dir" ] || { [ -e "$output_dir" ] && [ ! -d "$output_dir" ]; }; then
    printf '%s\n' "Refusing to use a non-directory dist path: $output_dir" >&2
    exit 1
fi
mkdir -p "$output_dir"
if [ -L "$output" ] || [ -L "$output.sha256" ] || [ -e "$output" ] || [ -e "$output.sha256" ]; then
    printf '%s\n' "Refusing to overwrite an existing AppImage or checksum: $output" >&2
    exit 1
fi
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/jira-desk-appimage.XXXXXXXX")
cleanup() { rm -rf -- "$work_dir"; }
trap cleanup EXIT HUP INT TERM
appdir="$work_dir/JiraDesk.AppDir"
icon="$work_dir/dev.jiradesk.JiraDesk.png"
if ! command -v magick >/dev/null 2>&1; then
    printf '%s\n' "ImageMagick's magick command is required to render the AppImage root PNG icon" >&2
    exit 1
fi
magick "$icon_source" "$icon"
icon_format=$(magick identify -format '%m' "$icon") || {
    printf '%s\n' "Could not inspect rendered icon: $icon" >&2
    exit 1
}
[ "$icon_format" = "PNG" ] || {
    printf '%s\n' "Icon renderer did not produce a PNG file: $icon" >&2
    exit 1
}
mkdir -p "$appdir/usr/share/metainfo" "$appdir/usr/share/licenses/jira-gpui"
cp -- "$metainfo" "$appdir/usr/share/metainfo/"
cp -- "$license" "$appdir/usr/share/licenses/jira-gpui/LICENSE"

APPIMAGE_EXTRACT_AND_RUN=1 "$LINUXDEPLOY" \
    --appdir "$appdir" \
    --executable "$binary" \
    --desktop-file "$desktop" \
    --icon-file "$icon" \
    --custom-apprun "$apprun" \
    --exclude-library libxkbcommon.so.0

mkdir -p "$appdir/usr/lib"
cp -L -- "$host_libxkbcommon" "$appdir/usr/lib/libxkbcommon.so.0"
[ -f "$appdir/usr/lib/libxkbcommon.so.0" ] && [ -r "$appdir/usr/lib/libxkbcommon.so.0" ] || {
    printf '%s\n' "Failed to copy libxkbcommon.so.0 into AppDir" >&2
    exit 1
}

if [ -n "${APPIMAGE_RUNTIME:-}" ]; then
    [ -f "$APPIMAGE_RUNTIME" ] || { printf '%s\n' "APPIMAGE_RUNTIME is not a file" >&2; exit 1; }
    APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --runtime-file "$APPIMAGE_RUNTIME" "$appdir" "$output"
else
    APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" "$appdir" "$output"
fi

output_name=$(basename -- "$output")
(cd "$output_dir" && sha256sum -- "$output_name" > "$output_name.sha256")
printf 'Created %s\nChecksum: %s\n' "$output" "$output.sha256"
