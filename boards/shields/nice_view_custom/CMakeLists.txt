if(CONFIG_ZMK_DISPLAY AND CONFIG_NICE_VIEW_WIDGET_STATUS)
  # host_link.c leaves the panel redraw and screen readback to status.c and screen_snap.c.
  zephyr_library_compile_definitions(FOXGLOVE_HAS_WIDGETS)
  zephyr_library_include_directories(${CMAKE_SOURCE_DIR}/include)
  zephyr_library_sources(custom_status_screen.c)
  zephyr_library_sources(widgets/icons.c)
  zephyr_library_sources(widgets/util.c)
  zephyr_library_sources(widgets/screen_snap.c)
  if(CONFIG_FOXGLOVE_PANEL_POLARITY)
    # Both halves: the panels' dark or light, from the PC and the left (#666).
    zephyr_library_sources(widgets/panel_polarity.c)
  endif()

  if(NOT CONFIG_ZMK_SPLIT OR CONFIG_ZMK_SPLIT_ROLE_CENTRAL)
    zephyr_library_sources(widgets/status.c)
    if(CONFIG_NICE_VIEW_HOST_LINK)
      zephyr_library_sources(widgets/host_link.c)
      # The knob (#719) asks the host link whether the PC holds it.
      zephyr_library_compile_definitions(FOXGLOVE_HAS_HOST_LINK)
      if(CONFIG_FOXGLOVE_HOST_FRAMES)
        zephyr_library_sources(widgets/host_frames.c)
      endif()
      if(CONFIG_FOXGLOVE_HOST_EFFECTS)
        zephyr_library_sources(widgets/host_effects.c)
      endif()
      if(CONFIG_FOXGLOVE_LAMPARRAY)
        # The HID LampArray (#88): the core, its USB interface, and Bluetooth when asked.
        zephyr_library_sources(widgets/lamparray.c widgets/lamparray_usb.c)
        if(CONFIG_FOXGLOVE_LAMPARRAY_BLE)
          zephyr_library_sources(widgets/lamparray_hog.c)
        endif()
        if(NOT CONFIG_FOXGLOVE_HOST_EFFECTS)
          # Its lamp positions are SET_EFFECT's built-in table.
          zephyr_library_sources(widgets/lamp_effect.c)
        endif()
      endif()
      zephyr_library_sources(widgets/screens_corne.c)
      if(CONFIG_FOXGLOVE_TEST_HOOKS)
        zephyr_library_sources(widgets/test_hooks.c)
      endif()
      if(CONFIG_NICE_VIEW_ART_RELAY)
        zephyr_library_sources(widgets/art_relay_central.c)
        zephyr_library_sources(widgets/art_keys.c)
        # The left's activity goes to the right, whose art follows it (#603).
        zephyr_library_sources(widgets/activity_relay.c)
      endif()
    endif()
  else()
    zephyr_library_sources(widgets/custom_art.c)
    zephyr_library_sources(widgets/peripheral_status.c)
    # The art's pace (#842) comes from the board's screens table: its art target on this half.
    zephyr_library_sources(widgets/art_pace.c widgets/screens_corne.c)
    if(CONFIG_NICE_VIEW_ART_RELAY)
      # The relay's far end (art_relay_peripheral.c) in front of the art store (#834).
      zephyr_library_sources(widgets/art_relay_peripheral.c widgets/art_store.c)
      zephyr_library_sources(widgets/art_slots.c)
      if(CONFIG_FOXGLOVE_TEST_HOOKS)
        zephyr_library_sources(widgets/test_hooks.c)
      endif()
    endif()
  endif()
endif()

# Not a display feature: the underglow's idle handling is built whenever it's on.
if(CONFIG_NICE_VIEW_RGB_IDLE)
  zephyr_library_include_directories(${CMAKE_SOURCE_DIR}/include)
  zephyr_library_sources(widgets/rgb_idle.c)
  if(CONFIG_FOXGLOVE_HOST_FRAMES)
    zephyr_library_sources(widgets/lamp_strip.c)
  endif()
  if(CONFIG_FOXGLOVE_HOST_EFFECTS)
    # Both halves draw SET_EFFECT from their own lamp positions (#490).
    zephyr_library_sources(widgets/effect_run.c widgets/lamp_effect.c)
  endif()
endif()
if(CONFIG_NICE_VIEW_RGB_BATTERY_DIM)
  # colour_effects.c: Swirl in the chosen colour (#626), and the lighting health counters.
  zephyr_library_sources(widgets/dim_strip.c widgets/battery_dim.c widgets/colour_effects.c)
endif()
# Not a display feature either: a cable or a host arriving counts as activity (#432). Here, not
# in foxglove_common.cmake: the foxglove_link shield (Toucan2) builds that into Zephyr's own
# library, without ZMK's headers.
if(CONFIG_USB_DEVICE_STACK)
  zephyr_library_sources(widgets/usb_wake.c)
endif()
# The PC's state (#609, #604): both halves, display or not. Its users (rgb_idle.c,
# peripheral_status.c, activity_relay.c, the backlight below) read the effective activity.
if(CONFIG_FOXGLOVE_PC_STATE)
  zephyr_library_include_directories(${CMAKE_SOURCE_DIR}/include)
  zephyr_library_sources(widgets/pc_state.c)
  if(CONFIG_FOXGLOVE_BACKLIGHT_IDLE)
    zephyr_library_sources(widgets/backlight_idle.c)
  endif()
endif()

# The keyboard's self-report (#705): both halves, display or not.
if(CONFIG_FOXGLOVE_HEALTH)
  zephyr_library_include_directories(${CMAKE_SOURCE_DIR}/include)
  zephyr_library_sources(widgets/health.c)
endif()

# &knob / &knob_press (#719): on the central (or a one-piece board), display or not. Without the
# host link they are their fallback keys only.
if(CONFIG_FOXGLOVE_KNOB AND (NOT CONFIG_ZMK_SPLIT OR CONFIG_ZMK_SPLIT_ROLE_CENTRAL))
  zephyr_library_include_directories(${CMAKE_SOURCE_DIR}/include)
  zephyr_library_sources(widgets/knob.c)
endif()

# Version, SHA and half marker, shared with the foxglove_link shield.
include(${CMAKE_CURRENT_LIST_DIR}/foxglove_common.cmake)
