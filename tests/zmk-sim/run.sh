#!/usr/bin/env bash
# ZMK v0.3.0 app/run-test.sh mechanics, with retained build logs and pipefail.
set -euo pipefail
suite=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
: "${ZMK_SRC_DIR:?Set ZMK_SRC_DIR to the v0.3.0 zmk/app directory}"
ZMK_SRC_DIR=$(realpath "$ZMK_SRC_DIR")
build_root=${ZMK_BUILD_DIR:-"$suite/build"}
mkdir -p "$build_root"
build_root=$(realpath "$build_root")
failed=0
count=0
for keymap in "$suite"/*/native_posix_64.keymap; do
    [[ -f "$keymap" ]] || continue
    case_dir=$(dirname "$keymap")
    name=$(basename "$case_dir")
    build="$build_root/$name"
    mkdir -p "$build"
    count=$((count + 1))
    echo "Running $name"
    # Log outside the pristine build directory: west -p deletes that directory.
    if ! west build -s "$ZMK_SRC_DIR" -d "$build" -b native_posix_64 -p -- \
        -DCONFIG_ASSERT=y -DZMK_CONFIG="$case_dir" \
        -DEXTRA_CONF_FILE="$suite/common/sim.conf" \
        -DZMK_EXTRA_MODULES="$suite/module" >"$build_root/$name-build.log" 2>&1; then
        echo "FAILED: $name did not build (see $build_root/$name-build.log)"
        failed=1
        continue
    fi
    if ! timeout 60s "$build/zephyr/zmk.exe" 2>"$build/stderr.log" |
        sed -e 's/.*> //' |
        tee "$build/keycode_events_full.log" |
        sed -n -f "$case_dir/events.patterns" >"$build/keycode_events.log"; then
        echo "FAILED: $name simulator/filter error"
        failed=1
        continue
    fi
    # Never accept an empty trace after a logging or boot failure.
    if [[ ! -s "$build/keycode_events.log" ]]; then
        echo "FAILED: $name produced no filtered events"
        failed=1
        continue
    fi
    if ! diff -auZ "$case_dir/keycode_events.snapshot" "$build/keycode_events.log" >"$build/snapshot.diff"; then
        cat "$build/snapshot.diff"
        if [[ ${ZMK_TESTS_AUTO_ACCEPT:-0} == 1 ]]; then
            cp "$build/keycode_events.log" "$case_dir/keycode_events.snapshot"
            echo "ACCEPTED: $name (human review required)"
        else
            echo "FAILED: $name snapshot mismatch"
            failed=1
        fi
    else
        echo "PASS: $name"
    fi
done
[[ $count -gt 0 ]] || { echo 'No simulator cases found' >&2; exit 1; }
exit "$failed"
