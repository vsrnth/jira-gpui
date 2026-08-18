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
icon_source="$project_root/assets/app-icon/target-target-svgrepo-com.svg"
apprun="$project_root/packaging/appimage/AppRun"
desktop="$project_root/packaging/appimage/dev.jiradesk.JiraDesk.desktop"
metainfo="$project_root/packaging/appimage/dev.jiradesk.JiraDesk.metainfo.xml"
license="$project_root/LICENSE"
output_dir="$project_root/dist"
output="$output_dir/Jira_Desk-${version}-x86_64.AppImage"
app_id=dev.jiradesk.JiraDesk

for input in "$desktop" "$icon_source" "$apprun" "$metainfo" "$license"; do
    [ -f "$input" ] || { printf '%s\n' "Missing packaging input: $input" >&2; exit 1; }
done

# Keep the source desktop entry launcher-neutral: the application replaces
# Exec with the absolute current AppImage path after a real launch.
desktop_exec_count=$(awk '/^Exec=/{count++} END{print count + 0}' "$desktop")
[ "$desktop_exec_count" -eq 1 ] || {
    printf '%s\n' "Desktop template must contain exactly one Exec entry" >&2
    exit 1
}
grep -Fqx 'Exec=jira-gpui' "$desktop" || {
    printf '%s\n' "Desktop template must contain exactly: Exec=jira-gpui" >&2
    exit 1
}
desktop_name_count=$(awk '/^Name=/{count++} END{print count + 0}' "$desktop")
[ "$desktop_name_count" -eq 1 ] && grep -Fqx 'Name=Jira Desk' "$desktop" || {
    printf '%s\n' "Desktop template must contain exactly one Name=Jira Desk entry" >&2
    exit 1
}
desktop_icon_count=$(awk '/^Icon=/{count++} END{print count + 0}' "$desktop")
[ "$desktop_icon_count" -eq 1 ] && grep -Fqx "Icon=$app_id" "$desktop" || {
    printf '%s\n' "Desktop template must contain exactly one Icon=$app_id entry" >&2
    exit 1
}
grep -Fqx "  <id>$app_id</id>" "$metainfo" || {
    printf '%s\n' "AppStream metadata ID must be $app_id" >&2
    exit 1
}
grep -Fqx "  <launchable type=\"desktop-id\">$app_id.desktop</launchable>" "$metainfo" || {
    printf '%s\n' "AppStream launchable must be $app_id.desktop" >&2
    exit 1
}

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
magick -background none "$icon_source" -resize 256x256! "$icon"
icon_format=$(magick identify -format '%m' "$icon") || {
    printf '%s\n' "Could not inspect rendered icon: $icon" >&2
    exit 1
}
[ "$icon_format" = "PNG" ] || {
    printf '%s\n' "Icon renderer did not produce a PNG file: $icon" >&2
    exit 1
}
icon_dimensions=$(magick identify -format '%wx%h' "$icon") || {
    printf '%s\n' "Could not inspect rendered icon dimensions: $icon" >&2
    exit 1
}
[ "$icon_dimensions" = "256x256" ] || {
    printf '%s\n' "Rendered icon must be exactly 256x256, got $icon_dimensions: $icon" >&2
    exit 1
}
icon_channels=$(magick identify -format '%[channels]' "$icon") || {
    printf '%s\n' "Could not inspect rendered icon alpha channel: $icon" >&2
    exit 1
}
case "$icon_channels" in
    *a*) ;;
    *)
        printf '%s\n' "Rendered icon must contain an alpha channel: $icon" >&2
        exit 1
        ;;
esac
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
