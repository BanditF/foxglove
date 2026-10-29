#!/usr/bin/env bash
# relay-sim with gcc (CI: .github/workflows/zmk-sim.yml, job relay-sim). Builds the harness
# against the widgets directory (default: this checkout's) and runs the gating scenarios.
#   bash tests/relay-sim/run.sh [widgets_dir] [scenario...]
# Keep DEFINES and SCENARIOS in step with config.ps1 (build.ps1, Windows).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
widgets="${1:-$root/boards/shields/nice_view_custom/widgets}"
shift || true
out="${RELAY_SIM_OUT:-$root/target/relay-sim}"
mkdir -p "$out"

DEFINES=(
    -DCONFIG_NICE_VIEW_RGB_IDLE=1
    -DCONFIG_NICE_VIEW_RGB_BATTERY_DIM=1
    -DCONFIG_FOXGLOVE_SPLIT_GUARD=1
    -DCONFIG_FOXGLOVE_HOST_FRAMES=1
    -DCONFIG_FOXGLOVE_HOST_EFFECTS=1
    -DCONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE=16384
    -DCONFIG_ZMK_SPLIT=1
    -DCONFIG_ZMK_SPLIT_ROLE_CENTRAL=1
    -DCONFIG_FOXGLOVE_RIGHT_LAMPS=21
    -DCONFIG_ZMK_LOG_LEVEL=0
)
SCENARIOS=(wake wake-app relink-shown push-relink-tap stall stall-app late late-ignored select-snap snap-latency)
# Known gaps: each must still FAIL (README.md, "Known gaps"). None at the moment.
KNOWN_GAPS=()
if [ "$#" -gt 0 ]; then
    SCENARIOS=("$@")
    KNOWN_GAPS=()
fi

cc="${CC:-gcc}"
flags=(-std=c11 -O1 -g -Wall -Wno-unused-function -Wno-unused-variable -Wno-missing-braces)
"$cc" "${flags[@]}" -c "$here/sim_coro.c" -o "$out/sim_coro.o"
"$cc" "${flags[@]}" "${DEFINES[@]}" -I"$here/include" -I"$widgets" -I"$here" \
    "$here/sim_kernel.c" "$here/sim_bt.c" "$here/sim_stubs.c" "$here/main.c" \
    "$widgets/art_relay_central.c" "$out/sim_coro.o" -o "$out/relay-sim"

fail=0
for s in "${SCENARIOS[@]}"; do
    if ! "$out/relay-sim" "$s" 2; then
        fail=$((fail + 1))
    fi
done
for s in ${KNOWN_GAPS[@]+"${KNOWN_GAPS[@]}"}; do
    if "$out/relay-sim" "$s" 1 | grep "^RESULT "; then
        echo "relay-sim: known gap $s passed: update run.sh, config.ps1 and README.md"
        fail=$((fail + 1))
    else
        echo "relay-sim: known gap $s fails as documented"
    fi
done
if [ "$fail" -ne 0 ]; then
    echo "relay-sim: $fail scenario(s) failed"
    exit 1
fi
echo "relay-sim: all scenarios passed"
