#!/bin/sh
set -eu

stop_suite() {
    trap - INT TERM HUP
    exit 130
}
trap stop_suite INT TERM HUP

usage() {
    printf '%s\n' \
        'Usage: tools/macos-ui-automation/run.sh [--scenario NAME | --suite | --list | --self-test] [--artifact-dir ABSOLUTE_DIR]' \
        'Scenarios: onboarding, issues, updates, team, settings' >&2
}

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
if [ "$(uname -s)" != Darwin ]; then
    printf '%s\n' 'macOS UI automation requires Darwin.' >&2
    exit 2
fi
if [ ! -f "$repo_root/Cargo.toml" ] || [ ! -d "$repo_root/.git" ]; then
    printf '%s\n' "could not identify repository root: $repo_root" >&2
    exit 2
fi

scenario=''
suite=0
list=0
self_test=0
artifact_dir=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --scenario)
            [ "$#" -ge 2 ] || { usage; exit 2; }
            scenario=$2
            shift 2
            ;;
        --suite)
            suite=1
            shift
            ;;
        --list)
            list=1
            shift
            ;;
        --self-test)
            self_test=1
            shift
            ;;
        --artifact-dir)
            [ "$#" -ge 2 ] || { usage; exit 2; }
            artifact_dir=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

scenarios='onboarding issues updates team settings'
if [ "$list" -eq 1 ]; then
    if [ -n "$scenario" ] || [ "$suite" -eq 1 ] || [ "$self_test" -eq 1 ] || [ -n "$artifact_dir" ]; then
        usage
        exit 2
    fi
    printf '%s\n' onboarding issues updates team settings
    exit 0
fi
if [ "$self_test" -eq 1 ] && { [ -n "$scenario" ] || [ "$suite" -eq 1 ]; }; then
    usage
    exit 2
fi
if [ "$self_test" -eq 0 ] && [ "$suite" -eq 1 ] && [ -n "$scenario" ]; then
    usage
    exit 2
fi
if [ "$self_test" -eq 0 ] && [ "$suite" -eq 0 ] && [ -z "$scenario" ]; then
    suite=1
fi
if [ -n "$scenario" ]; then
    case " $scenarios " in
        *" $scenario "*) ;;
        *) printf 'unknown scenario: %s\n' "$scenario" >&2; exit 2 ;;
    esac
fi

for tool in cargo codesign plutil xcodebuild; do
    command -v "$tool" >/dev/null 2>&1 || {
        printf 'required tool is not available: %s\n' "$tool" >&2
        exit 2
    }
done
xcodebuild -version >/dev/null

project=$script_dir/JiraDeskUITests.xcodeproj
scheme=JiraDeskUITests
host_path=$repo_root/target/debug/jira-ui-automation-host
plist_template=$script_dir/JiraDeskUITests/JiraDeskUITests-Host-Info.plist

if [ -z "$artifact_dir" ]; then
    run_id=$(date -u '+%Y%m%dT%H%M%SZ')-$$
    artifact_dir=$repo_root/target/ui-automation/$run_id
fi
case "$artifact_dir" in
    /*) ;;
    *) printf '%s\n' '--artifact-dir must be an absolute path' >&2; exit 2 ;;
esac
if [ -L "$artifact_dir" ] || { [ -e "$artifact_dir" ] && [ ! -d "$artifact_dir" ]; }; then
    printf '%s\n' 'artifact directory must not be a symlink or non-directory' >&2
    exit 2
fi
mkdir -p "$artifact_dir"

if [ "$self_test" -eq 1 ]; then
    plutil -lint "$plist_template" >/dev/null
    xcodebuild -list -project "$project" >/dev/null
    printf '%s\n' 'XCUITest project self-test passed.'
    exit 0
fi

if [ ! -f "$plist_template" ]; then
    printf '%s\n' "missing host Info.plist: $plist_template" >&2
    exit 2
fi
printf '%s\n' 'Building deterministic UI automation host…'
cargo build -p jira-gpui --features ui-automation --bin jira-ui-automation-host
if [ ! -f "$host_path" ] || [ ! -x "$host_path" ]; then
    printf '%s\n' "host build did not produce executable: $host_path" >&2
    exit 2
fi

method_for_scenario() {
    case "$1" in
        onboarding) printf '%s\n' testOnboarding ;;
        issues) printf '%s\n' testIssues ;;
        updates) printf '%s\n' testUpdates ;;
        team) printf '%s\n' testTeam ;;
        settings) printf '%s\n' testSettings ;;
    esac
}

run_one() {
    name=$1
    destination=$artifact_dir/$name
    if [ -e "$destination" ] || [ -L "$destination" ]; then
        printf '%s\n' "refusing to reuse existing scenario artifact directory: $destination" >&2
        return 2
    fi
    mkdir "$destination"

    derived=$destination/DerivedData
    result=$destination/TestResults.xcresult
    printf 'Building XCUITest bundle: %s\n' "$name"
    xcodebuild build-for-testing \
        -project "$project" \
        -scheme "$scheme" \
        -destination "platform=macOS,arch=$(uname -m)" \
        -derivedDataPath "$derived" \
        CODE_SIGNING_ALLOWED=YES \
        CODE_SIGNING_IDENTITY=- \
        CODE_SIGNING_REQUIRED=NO

    products="$derived/Build/Products/Debug"
    app="$products/Jira Desk UI Automation.app"
    data_root="$products/Jira Desk UI Automation Data"
    state_root="$products/Jira Desk UI Automation State"
    runner="$products/JiraDeskUITests-Runner.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$data_root" "$state_root"
    cp "$host_path" "$app/Contents/MacOS/jira-ui-automation-host"
    chmod 755 "$app/Contents/MacOS/jira-ui-automation-host"
    cp "$plist_template" "$app/Contents/Info.plist"
    plutil -lint "$app/Contents/Info.plist" >/dev/null
    codesign --force --deep --sign - "$app" >/dev/null
    codesign --verify --deep --strict "$app"
    codesign --verify --deep --strict "$runner"

    method=$(method_for_scenario "$name")
    printf 'Running XCUITest scenario: %s\n' "$name"
    xcodebuild test-without-building \
            -project "$project" \
            -scheme "$scheme" \
            -destination "platform=macOS,arch=$(uname -m)" \
            -derivedDataPath "$derived" \
            -resultBundlePath "$result" \
            -only-testing:"$scheme/JiraDeskUITests/$method" \
            CODE_SIGNING_ALLOWED=YES \
            CODE_SIGNING_IDENTITY=- \
            CODE_SIGNING_REQUIRED=NO
}

if [ "$suite" -eq 1 ]; then
    failures=0
    for name in onboarding issues updates team settings; do
        if ! run_one "$name"; then
            failures=$((failures + 1))
        fi
    done
    if [ "$failures" -ne 0 ]; then
        printf '%s\n' "$failures scenario(s) failed; inspect each TestResults.xcresult and DerivedData." >&2
        exit 1
    fi
    printf '%s\n' 'All local XCUITest scenarios passed.'
    exit 0
fi
run_one "$scenario"
