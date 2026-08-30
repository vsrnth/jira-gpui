#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../../.." && pwd)
build_script="$repo_root/packaging/macos/build-dmg.sh"
test_root=$(mktemp -d "${TMPDIR:-/tmp}/jira-dmg-retry-test.XXXXXXXX")
cleanup() {
    rm -rf "$test_root"
}
trap cleanup EXIT HUP INT TERM

stub="$test_root/hdiutil"
cat > "$stub" <<'EOF'
#!/bin/sh
set -eu

count=$(cat "$HDIUTIL_TEST_STATE")
count=$((count + 1))
printf '%s\n' "$count" > "$HDIUTIL_TEST_STATE"
output=''
for argument do
    output=$argument
done
after_partial() {
    : > "$output"
    exit 1
}
case "$HDIUTIL_TEST_MODE" in
    busy-success)
        if [ "$count" -eq 1 ]; then
            printf '%s\n' 'hdiutil: create failed: Resource busy' >&2
            after_partial
        fi
        if [ -e "$output" ] || [ -L "$output" ]; then
            printf '%s\n' 'test hdiutil: stale partial DMG was not removed before retry' >&2
            exit 2
        fi
        : > "$output"
        printf '%s\n' 'hdiutil: create succeeded' >&2
        ;;
    non-transient)
        printf '%s\n' 'hdiutil: create failed: Permission denied' >&2
        after_partial
        ;;
    busy-exhausted)
        printf '%s\n' 'hdiutil: create failed: Resource busy' >&2
        after_partial
        ;;
    *)
        printf 'unexpected test mode: %s\n' "$HDIUTIL_TEST_MODE" >&2
        exit 2
        ;;
esac
EOF
chmod 755 "$stub"

assert_eq() {
    expected=$1
    actual=$2
    message=$3
    [ "$expected" = "$actual" ] || {
        printf 'assertion failed: %s (expected %s, got %s)\n' "$message" "$expected" "$actual" >&2
        exit 1
    }
}

run_helper() {
    mode=$1
    case_dir="$test_root/$mode"
    mkdir "$case_dir"
    state="$case_dir/state"
    printf '0\n' > "$state"
    staging="$case_dir/staging"
    mkdir "$staging"
    dmg="$case_dir/result.dmg"
    log="$case_dir/hdiutil.log"
    stderr="$case_dir/stderr"
    if HDIUTIL_BIN="$stub" \
        HDIUTIL_TEST_MODE="$mode" \
        HDIUTIL_TEST_STATE="$state" \
        JIRA_DMG_TEST_HELPER_ONLY=1 \
        sh "$build_script" "$dmg" "$staging" "$log" 2>"$stderr"; then
        status=0
    else
        status=$?
    fi
    printf '%s\n' "$status"
}

status=$(run_helper busy-success)
assert_eq 0 "$status" 'Resource busy should retry and then succeed'
assert_eq 2 "$(cat "$test_root/busy-success/state")" 'Resource busy should use two attempts'
[ -f "$test_root/busy-success/result.dmg" ]
grep -Fq 'Resource busy' "$test_root/busy-success/stderr"
grep -Fq 'create succeeded' "$test_root/busy-success/stderr"
grep -Fq 'retrying attempt 2 of 3' "$test_root/busy-success/stderr"

status=$(run_helper non-transient)
assert_eq 1 "$status" 'Non-transient failures should be returned'
assert_eq 1 "$(cat "$test_root/non-transient/state")" 'Non-transient failures should not retry'
grep -Fq 'Permission denied' "$test_root/non-transient/stderr"

status=$(run_helper busy-exhausted)
assert_eq 1 "$status" 'Retry exhaustion should fail'
assert_eq 3 "$(cat "$test_root/busy-exhausted/state")" 'Retry exhaustion should stop at three attempts'
assert_eq 3 "$(grep -c '^hdiutil: create failed: Resource busy$' "$test_root/busy-exhausted/stderr")" \
    'Retry exhaustion should preserve each Resource busy diagnostic'
assert_eq 2 "$(grep -c '^hdiutil create reported Resource busy; retrying attempt' "$test_root/busy-exhausted/stderr")" \
    'Retry exhaustion should report each retry'

printf '%s\n' 'build-dmg hdiutil retry tests passed.'
