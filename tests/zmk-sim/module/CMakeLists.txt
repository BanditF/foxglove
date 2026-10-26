# Compile the actual upstream behavior even though native_posix has no soft-off PM.
# Only the final zmk_pm_soft_off() call is replaced; hold timing stays upstream.
if(CONFIG_ZMK_PM_SOFT_OFF OR CONFIG_ZMK_BEHAVIOR_SOFT_OFF)
  message(FATAL_ERROR "Simulator supplies the soft-off boundary; keep hardware soft-off disabled")
endif()
zephyr_library()
zephyr_library_include_directories(${APPLICATION_SOURCE_DIR}/include)
zephyr_library_sources(
  ${APPLICATION_SOURCE_DIR}/src/behaviors/behavior_soft_off.c
  stubs.c
)

set(FOXGLOVE_WIDGETS ${CMAKE_CURRENT_LIST_DIR}/../../../boards/shields/nice_view_custom/widgets)

if(CONFIG_FOXGLOVE_SIM_POWEROFF)
  zephyr_library_sources(sleep_sim.c)
endif()

if(CONFIG_FOXGLOVE_SIM_DIM)
  # The levels are the shield's shipped defaults (Kconfig.defconfig), which the simulator doesn't
  # load.
  file(STRINGS ${FOXGLOVE_WIDGETS}/../Kconfig.defconfig FOXGLOVE_DEFCONFIG)
  set(FOXGLOVE_DIM_DEFS "")
  foreach(sym POWER BATTERY LOW LOW_BELOW)
    set(FOXGLOVE_VALUE "")
    set(FOXGLOVE_IN FALSE)
    foreach(line IN LISTS FOXGLOVE_DEFCONFIG)
      if(line MATCHES "^config NICE_VIEW_RGB_DIM_${sym}[^A-Z_]*$")
        set(FOXGLOVE_IN TRUE)
      elseif(FOXGLOVE_IN AND line MATCHES "^[ 	]+default ([0-9]+)")
        set(FOXGLOVE_VALUE ${CMAKE_MATCH_1})
        break()
      endif()
    endforeach()
    if(FOXGLOVE_VALUE STREQUAL "")
      message(FATAL_ERROR "NICE_VIEW_RGB_DIM_${sym}'s default not found in Kconfig.defconfig")
    endif()
    list(APPEND FOXGLOVE_DIM_DEFS "CONFIG_NICE_VIEW_RGB_DIM_${sym}=${FOXGLOVE_VALUE}")
  endforeach()
  set_source_files_properties(${FOXGLOVE_WIDGETS}/battery_dim.c PROPERTIES
    COMPILE_DEFINITIONS "${FOXGLOVE_DIM_DEFS}")
  zephyr_library_include_directories(${FOXGLOVE_WIDGETS})
  zephyr_library_sources(${FOXGLOVE_WIDGETS}/dim_strip.c ${FOXGLOVE_WIDGETS}/battery_dim.c
                         dim_sim.c)
endif()

if(CONFIG_FOXGLOVE_SIM_HOSTLINK)
  file(STRINGS ${FOXGLOVE_WIDGETS}/../FIRMWARE_VERSION FOXGLOVE_FW_VERSION LIMIT_COUNT 1)
  string(REGEX MATCH "^([0-9]+)\.([0-9]+)\.([0-9]+)$" FOXGLOVE_FW_OK "${FOXGLOVE_FW_VERSION}")
  zephyr_library_include_directories(include ${FOXGLOVE_WIDGETS})
  zephyr_library_compile_definitions(
    FOXGLOVE_BUILD_SHA="simtest"
    FOXGLOVE_FW_VERSION="${FOXGLOVE_FW_VERSION}"
    FOXGLOVE_FW_MAJOR=${CMAKE_MATCH_1}
    FOXGLOVE_FW_MINOR=${CMAKE_MATCH_2}
    FOXGLOVE_FW_PATCH=${CMAKE_MATCH_3})
  zephyr_library_sources(
    ${FOXGLOVE_WIDGETS}/host_link.c
    ${FOXGLOVE_WIDGETS}/screens_corne.c
    ${FOXGLOVE_WIDGETS}/half_marker.c
    hostlink_sim.c
  )
  if(CONFIG_FOXGLOVE_SIM_PANEL_STREAM)
    zephyr_library_sources(panel_stream_sim.c)
  endif()
  if(CONFIG_FOXGLOVE_SIM_ART_KEYS)
    # &art_next / &art_prev (#60): the real behaviour; the board-side step prints instead.
    zephyr_library_sources(${FOXGLOVE_WIDGETS}/art_keys.c art_keys_sim.c)
  endif()
  if(CONFIG_FOXGLOVE_TEST_HOOKS)
    zephyr_library_sources(${FOXGLOVE_WIDGETS}/test_hooks.c)
  endif()
endif()
