# Foxglove's host link on any ZMK v0.3 board (#75): HELLO, SCREENS, settings commands, build
# info. No screen widgets or art relay of ours; the board's screens table says what's there.
# Use with raw_hid_adapter, e.g. "toucan_left nice_view_gem raw_hid_adapter foxglove_link".
# SPDX-License-Identifier: MIT
set(FOXGLOVE_WIDGETS ${CMAKE_CURRENT_LIST_DIR}/../nice_view_custom/widgets)
if(NOT CONFIG_ZMK_SPLIT OR CONFIG_ZMK_SPLIT_ROLE_CENTRAL)
  zephyr_library_include_directories(${CMAKE_SOURCE_DIR}/include ${FOXGLOVE_WIDGETS})
  zephyr_library_sources(${FOXGLOVE_WIDGETS}/host_link.c)
  if(CONFIG_FOXGLOVE_PANEL)
    zephyr_library_compile_definitions(FOXGLOVE_HAS_PANEL)
    zephyr_library_sources(panel_lvgl.c)
  endif()
  if(CONFIG_SHIELD_TOUCAN_LEFT)
    zephyr_library_sources(screens_toucan2.c)
  else()
    zephyr_library_sources(screens_none.c)
  endif()
endif()
include(${CMAKE_CURRENT_LIST_DIR}/../nice_view_custom/foxglove_common.cmake)
