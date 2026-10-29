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
# Gating: each must pass on this tree. stall and stall-app were known gaps until the left let
# go of unanswered requests and dropped a link whose request never got an answer (#618).
$RelaySimScenarios = @("wake", "wake-app", "relink-shown", "push-relink-tap", "stall", "stall-app", "late", "late-ignored")
# Known gaps: each must still FAIL on this tree (README.md, "Known gaps"); one that passes means
# the gap closed and the lists and README need updating. None at the moment.
$RelaySimKnownGaps = @()
# Diagnostic only (bisect.ps1 runs it too): a request never answered while a SELECT waits.
$RelaySimInfo = @("lost")
