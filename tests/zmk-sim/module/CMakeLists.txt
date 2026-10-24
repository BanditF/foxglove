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
