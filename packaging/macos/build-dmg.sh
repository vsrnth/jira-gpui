#!/bin/sh
set -eu

usage() {
    printf '%s\n' "Usage: VERSION=0.1.0 [BUNDLE_VERSION=0.1.0] $0 [--skip-build]" >&2
}

die() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

hdiutil_bin=${HDIUTIL_BIN:-hdiutil}

create_compressed_dmg() {
    dmg_temp=$1
    staging=$2
    hdiutil_log=$3
    attempt=1
    while [ "$attempt" -le 3 ]; do
        if "$hdiutil_bin" create -srcfolder "$staging" -volname "Jira Desk" -format UDZO \
            -imagekey zlib-level=9 "$dmg_temp" > /dev/null 2>"$hdiutil_log"; then
            status=0
        else
            status=$?
        fi
        if [ -s "$hdiutil_log" ]; then
            cat "$hdiutil_log" >&2
        fi
        if [ "$status" -eq 0 ]; then
            return 0
        fi
        if [ "$attempt" -ge 3 ] || ! grep -Fq 'Resource busy' "$hdiutil_log"; then
            return "$status"
        fi
        if [ -e "$dmg_temp" ] || [ -L "$dmg_temp" ]; then
            rm -f "$dmg_temp"
        fi
        next_attempt=$((attempt + 1))
        printf 'hdiutil create reported Resource busy; retrying attempt %s of 3\n' "$next_attempt" >&2
        sleep 1
        attempt=$((attempt + 1))
    done
    return 1
}

# Keep the retry seam directly executable by deterministic local shell tests without
# running the full macOS packaging pipeline.
if [ "${JIRA_DMG_TEST_HELPER_ONLY:-0}" = 1 ]; then
    [ "$#" -eq 3 ] || exit 2
    create_compressed_dmg "$1" "$2" "$3"
    exit $?
fi

skip_build=0
if [ "$#" -gt 1 ]; then
    usage
    exit 2
elif [ "$#" -eq 1 ]; then
    [ "$1" = "--skip-build" ] || { usage; exit 2; }
    skip_build=1
fi

case "$(uname -s)" in
    Darwin) ;;
    *) die "macOS DMG packaging requires a macOS host" ;;
esac
case "$(uname -m)" in
    arm64) arch=arm64 ;;
    x86_64) arch=x86_64 ;;
    *) die "macOS DMG packaging requires an arm64 or x86_64 host" ;;
esac

version=${VERSION:-0.1.0}
case "$version" in
    ''|*[!A-Za-z0-9._-]*) die "VERSION contains unsupported characters" ;;
esac

# VERSION is retained for artifact naming. By default, remove a release
# suffix such as "-local" for the macOS plist version fields.
bundle_version=${BUNDLE_VERSION:-${version%%-*}}
case "$bundle_version" in
    ''|*[!0-9.]*|.*|*.|*..*|*.*.*.*)
        die "BUNDLE_VERSION must be 1-3 dot-separated numeric components"
        ;;
esac

for tool in cargo sips iconutil codesign plutil shasum; do
    command -v "$tool" >/dev/null 2>&1 || die "required tool is not available: $tool"
done
command -v "$hdiutil_bin" >/dev/null 2>&1 || die "required tool is not available: $hdiutil_bin"

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
binary="$project_root/target/release/jira-gpui"
icon_source="$project_root/assets/app-icon/target-target-svgrepo-com.svg"
plist_template="$project_root/packaging/macos/Info.plist"
output_dir="$project_root/dist"
output="$output_dir/Jira_Desk-${version}-${arch}.dmg"
checksum="$output.sha256"
app_name="Jira Desk.app"
app_id=dev.jiradesk.JiraDesk

for input in "$icon_source" "$plist_template"; do
    [ -f "$input" ] || die "missing packaging input: $input"
done

if [ -L "$output_dir" ] || { [ -e "$output_dir" ] && [ ! -d "$output_dir" ]; }; then
    die "refusing to use a symlink or non-directory dist path: $output_dir"
fi
mkdir -p "$output_dir"
if [ -L "$output" ] || [ -L "$checksum" ] || [ -e "$output" ] || [ -e "$checksum" ]; then
    die "refusing to overwrite an existing DMG or checksum: $output"
fi

if [ "$skip_build" -eq 0 ]; then
    (cd "$project_root" && cargo build --release --locked -p jira-gpui)
fi
[ -f "$binary" ] && [ -x "$binary" ] || die "missing executable release binary: $binary"

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/jira-desk-dmg.XXXXXXXX")
cleanup() {
    if [ -n "${work_dir:-}" ] && [ -d "$work_dir" ]; then
        rm -rf "$work_dir"
    fi
}
trap cleanup EXIT HUP INT TERM

app_dir="$work_dir/$app_name"
contents="$app_dir/Contents"
macos_dir="$contents/MacOS"
resources_dir="$contents/Resources"
plist="$contents/Info.plist"
iconset="$work_dir/JiraDesk.iconset"
icon="$resources_dir/JiraDesk.icns"
staging="$work_dir/dmg-root"
mkdir -p "$macos_dir" "$resources_dir" "$iconset" "$staging"

# The source template is read-only; only the temporary bundle copy receives
# the validated bundle version value.
grep -Fq '@BUNDLE_VERSION@' "$plist_template" ||
    die "Info.plist template is missing the bundle version placeholder"
sed "s/@BUNDLE_VERSION@/$bundle_version/g" "$plist_template" > "$plist"
if grep -Eq '@[A-Za-z0-9_]+@' "$plist"; then
    die "Info.plist version substitution was incomplete"
fi

render_icon() {
    size=$1
    destination=$2
    sips -s format png -z "$size" "$size" "$icon_source" --out "$destination" >/dev/null || \
        die "could not rasterize application icon at ${size}x${size}"
    [ -s "$destination" ] || die "rasterized application icon is empty: $destination"
}

render_icon 16 "$iconset/icon_16x16.png"
render_icon 32 "$iconset/icon_16x16@2x.png"
render_icon 32 "$iconset/icon_32x32.png"
render_icon 64 "$iconset/icon_32x32@2x.png"
render_icon 128 "$iconset/icon_128x128.png"
render_icon 256 "$iconset/icon_128x128@2x.png"
render_icon 256 "$iconset/icon_256x256.png"
render_icon 512 "$iconset/icon_256x256@2x.png"
render_icon 512 "$iconset/icon_512x512.png"
render_icon 1024 "$iconset/icon_512x512@2x.png"
iconutil -c icns "$iconset" -o "$icon" >/dev/null || die "could not create JiraDesk.icns"
[ -s "$icon" ] || die "application icon is empty: $icon"

cp "$binary" "$macos_dir/jira-gpui"
chmod 755 "$macos_dir/jira-gpui"
plutil -lint "$plist" >/dev/null || die "invalid generated Info.plist"

plist_value() {
    plutil -extract "$1" raw -o - "$plist"
}
check_plist_value() {
    key=$1
    expected=$2
    actual=$(plist_value "$key") || die "Info.plist is missing $key"
    [ "$actual" = "$expected" ] || die "Info.plist $key must be $expected (got $actual)"
}
check_plist_value CFBundleIdentifier "$app_id"
check_plist_value CFBundleExecutable jira-gpui
check_plist_value CFBundleName "Jira Desk"
check_plist_value CFBundleDisplayName "Jira Desk"
check_plist_value CFBundleVersion "$bundle_version"
check_plist_value CFBundleShortVersionString "$bundle_version"
[ -x "$macos_dir/jira-gpui" ] || die "bundle executable is not executable"
[ -f "$icon" ] || die "bundle application icon is missing"

signing_identity=${CODESIGN_IDENTITY:--}
codesign --force --deep --sign "$signing_identity" "$app_dir" >/dev/null || \
    die "could not sign application bundle"
codesign --verify --deep --strict "$app_dir" >/dev/null || \
    die "application signature validation failed"

cp -R "$app_dir" "$staging/$app_name"
ln -s /Applications "$staging/Applications"
[ -d "$staging/$app_name" ] || die "DMG staging app is missing"
[ -L "$staging/Applications" ] || die "DMG staging Applications link is missing"

dmg_name="Jira_Desk-${version}-${arch}.dmg"
dmg_temp="$work_dir/$dmg_name"
checksum_temp="$work_dir/$dmg_name.sha256"
create_compressed_dmg "$dmg_temp" "$staging" "$work_dir/hdiutil-create.stderr" ||
    die "could not create compressed DMG"
[ -f "$dmg_temp" ] && [ ! -L "$dmg_temp" ] || die "hdiutil did not create a DMG"
(cd "$work_dir" && shasum -a 256 "$dmg_name" > "$checksum_temp") || \
    die "could not create DMG checksum"

# Move only after hdiutil and checksum creation succeed. The destination was
# checked above and is checked again to avoid replacing a newly-created file.
if [ -L "$output" ] || [ -L "$checksum" ] || [ -e "$output" ] || [ -e "$checksum" ]; then
    die "refusing to overwrite an existing DMG or checksum: $output"
fi
mv "$dmg_temp" "$output"
mv "$checksum_temp" "$checksum"
printf 'Created %s\nChecksum: %s\n' "$output" "$checksum"
