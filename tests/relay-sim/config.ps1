# relay-sim: the left half's Kconfig as the dev firmware has it (shared by build.ps1; run.sh keeps
# the same list), and the scenarios CI runs.
$RelaySimDefines = @(
    "CONFIG_NICE_VIEW_RGB_IDLE=1",
    "CONFIG_NICE_VIEW_RGB_BATTERY_DIM=1",
    "CONFIG_FOXGLOVE_SPLIT_GUARD=1",
    "CONFIG_FOXGLOVE_HOST_FRAMES=1",
    "CONFIG_FOXGLOVE_HOST_EFFECTS=1",
    "CONFIG_NICE_VIEW_ART_RELAY_MAX_SIZE=16384",
    "CONFIG_ZMK_SPLIT=1",
    "CONFIG_ZMK_SPLIT_ROLE_CENTRAL=1",
    "CONFIG_FOXGLOVE_RIGHT_LAMPS=21",
    "CONFIG_ZMK_LOG_LEVEL=0"
)
# Gating: each must pass on this tree.
$RelaySimScenarios = @("wake", "wake-app", "stall", "stall-app", "relink-shown")
# Diagnostic only (bisect.ps1 runs them too): a request answered late, or never.
$RelaySimInfo = @("late", "lost")
