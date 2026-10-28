# Included by the nice_view_custom and foxglove_link shields. SPDX-License-Identifier: MIT
# Half identification is present with or without a display and survives section GC.
execute_process(COMMAND git rev-parse --short HEAD
  WORKING_DIRECTORY ${CMAKE_CURRENT_LIST_DIR}
  OUTPUT_VARIABLE FOXGLOVE_BUILD_SHA OUTPUT_STRIP_TRAILING_WHITESPACE
  ERROR_QUIET RESULT_VARIABLE FOXGLOVE_GIT_RESULT)
if(NOT FOXGLOVE_GIT_RESULT EQUAL 0 OR NOT FOXGLOVE_BUILD_SHA)
  # CI's container can't run git on the checkout (ownership), but Actions sets GITHUB_SHA.
  if(DEFINED ENV{GITHUB_SHA})
    string(SUBSTRING "$ENV{GITHUB_SHA}" 0 7 FOXGLOVE_BUILD_SHA)
  else()
    set(FOXGLOVE_BUILD_SHA unknown)
  endif()
endif()
# The firmware version (SemVer) lives in FIRMWARE_VERSION; a fw-vX.Y.Z tag must match it (CI).
file(STRINGS ${CMAKE_CURRENT_LIST_DIR}/FIRMWARE_VERSION FOXGLOVE_FW_VERSION LIMIT_COUNT 1)
string(REGEX MATCH "^([0-9]+)\\.([0-9]+)\\.([0-9]+)$" FOXGLOVE_FW_VERSION_OK "${FOXGLOVE_FW_VERSION}")
if(NOT FOXGLOVE_FW_VERSION_OK)
  message(FATAL_ERROR "FIRMWARE_VERSION must be X.Y.Z, not '${FOXGLOVE_FW_VERSION}'")
endif()
zephyr_library_sources(${CMAKE_CURRENT_LIST_DIR}/widgets/half_marker.c)
# Library-wide on purpose: a source property set here wouldn't reach the app library's target
# (it lives in another CMake directory), which is why the SHA used to read 'unknown'.
zephyr_library_compile_definitions(
  FOXGLOVE_BUILD_SHA="${FOXGLOVE_BUILD_SHA}"
  FOXGLOVE_FW_VERSION="${FOXGLOVE_FW_VERSION}"
  FOXGLOVE_FW_MAJOR=${CMAKE_MATCH_1}
  FOXGLOVE_FW_MINOR=${CMAKE_MATCH_2}
  FOXGLOVE_FW_PATCH=${CMAKE_MATCH_3})
zephyr_linker_sources(RODATA ${CMAKE_CURRENT_LIST_DIR}/half_marker.ld)
