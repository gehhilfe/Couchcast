cmake_minimum_required(VERSION 3.24)
project(couchcast LANGUAGES CXX)

# Couchcast — modern C++ port.
#
# Fullscreen HDMI-capture viewer with controller input forwarding. The Rust
# winit/wgpu/egui/gilrs/gstreamer-rs/tokio stack is ported to
# SDL3 + Vulkan + Dear ImGui + GStreamer (C API) + ASIO, built with CMake.
# See docs/ARCHITECTURE.md for the load-bearing design decisions (all preserved).

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE RelWithDebInfo CACHE STRING "" FORCE)
endif()

set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# ---------------------------------------------------------------------------
# Dependencies
# ---------------------------------------------------------------------------
find_package(Vulkan REQUIRED)
find_package(SDL3 REQUIRED CONFIG)
find_package(Threads REQUIRED)
find_package(PkgConfig REQUIRED)

pkg_check_modules(GST REQUIRED IMPORTED_TARGET
  gstreamer-1.0
  gstreamer-app-1.0
  gstreamer-video-1.0
  glib-2.0
  gobject-2.0)

# toml++ is used header-only (the system package installs headers under
# /usr/include/toml++). We deliberately do NOT link the tomlplusplus CMake target,
# whose interface defines TOML_HEADER_ONLY=0 and expects a separately-compiled TU.
find_path(TOMLPLUSPLUS_INCLUDE_DIR toml++/toml.hpp)

# glslc, for compiling GLSL -> SPIR-V embedded as C initializer lists.
find_program(GLSLC_EXE NAMES glslc REQUIRED)

# ---------------------------------------------------------------------------
# Dear ImGui (vendored) + SDL3/Vulkan backends
# ---------------------------------------------------------------------------
set(IMGUI_DIR ${CMAKE_CURRENT_SOURCE_DIR}/third_party/imgui)
add_library(imgui STATIC
  ${IMGUI_DIR}/imgui.cpp
  ${IMGUI_DIR}/imgui_draw.cpp
  ${IMGUI_DIR}/imgui_tables.cpp
  ${IMGUI_DIR}/imgui_widgets.cpp
  ${IMGUI_DIR}/backends/imgui_impl_sdl3.cpp
  ${IMGUI_DIR}/backends/imgui_impl_vulkan.cpp)
target_include_directories(imgui PUBLIC ${IMGUI_DIR} ${IMGUI_DIR}/backends)
target_link_libraries(imgui PUBLIC SDL3::SDL3 Vulkan::Vulkan)
target_compile_definitions(imgui PUBLIC IMGUI_IMPL_VULKAN_NO_PROTOTYPES=0)

# ---------------------------------------------------------------------------
# Shader compilation: GLSL -> SPIR-V, embedded as a C header (glslc -mfmt=c)
# ---------------------------------------------------------------------------
set(SHADER_GEN_DIR ${CMAKE_BINARY_DIR}/generated/shaders)
file(MAKE_DIRECTORY ${SHADER_GEN_DIR})

function(embed_shader SRC OUT_VAR)
  get_filename_component(name ${SRC} NAME)
  string(REPLACE "." "_" stem ${name})
  set(out ${SHADER_GEN_DIR}/${stem}.inc)
  add_custom_command(
    OUTPUT ${out}
    COMMAND ${GLSLC_EXE} -O -mfmt=c -o ${out} ${CMAKE_CURRENT_SOURCE_DIR}/${SRC}
    DEPENDS ${CMAKE_CURRENT_SOURCE_DIR}/${SRC}
    COMMENT "Compiling shader ${SRC} -> ${stem}.inc"
    VERBATIM)
  set(${OUT_VAR} ${out} PARENT_SCOPE)
endfunction()

embed_shader(shaders/video.vert VIDEO_VERT_INC)
embed_shader(shaders/video.frag VIDEO_FRAG_INC)

# ---------------------------------------------------------------------------
# couchcast_core: the toolkit-agnostic subsystems (transport, config, media,
# input, ui logic). Shared by the executable and the test binary.
# ---------------------------------------------------------------------------
add_library(couchcast_core STATIC
  src/log.cpp
  src/transport/event.cpp
  src/transport/capabilities.cpp
  src/transport/backends/log_transport.cpp
  src/transport/backends/adb.cpp
  src/transport/process.cpp
  src/config/config.cpp
  src/config/mapping.cpp
  src/input/input.cpp
  src/media/device.cpp
  src/media/frame.cpp
  src/media/pipeline.cpp
  src/ui/menu.cpp
  src/ui/debug_overlay.cpp)

target_include_directories(couchcast_core PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/src)
if(TOMLPLUSPLUS_INCLUDE_DIR)
  target_include_directories(couchcast_core PRIVATE ${TOMLPLUSPLUS_INCLUDE_DIR})
endif()
target_link_libraries(couchcast_core PUBLIC
  imgui SDL3::SDL3 Vulkan::Vulkan PkgConfig::GST Threads::Threads)
target_compile_options(couchcast_core PRIVATE
  -Wall -Wextra -Wno-unused-parameter
  -Wno-format-security -Wno-format-truncation -Wno-missing-field-initializers)

# ---------------------------------------------------------------------------
# couchcast executable (window/render/worker/main + the core lib)
# ---------------------------------------------------------------------------
add_executable(couchcast
  src/main.cpp
  src/app.cpp
  src/render/renderer.cpp
  src/worker.cpp
  ${VIDEO_VERT_INC}
  ${VIDEO_FRAG_INC})

target_include_directories(couchcast PRIVATE
  ${CMAKE_CURRENT_SOURCE_DIR}/src ${SHADER_GEN_DIR})
target_link_libraries(couchcast PRIVATE couchcast_core)
# asio (standalone, header-only) is installed system-wide by the `asio` package.
target_compile_definitions(couchcast PRIVATE ASIO_STANDALONE)
target_compile_options(couchcast PRIVATE
  -Wall -Wextra -Wno-unused-parameter
  -Wno-format-security -Wno-format-truncation -Wno-missing-field-initializers)

# ---------------------------------------------------------------------------
# Tests (ports the Rust crates' unit tests; run with `ctest`)
# ---------------------------------------------------------------------------
enable_testing()
add_executable(couchcast_tests tests/tests.cpp)
target_link_libraries(couchcast_tests PRIVATE couchcast_core)
add_test(NAME couchcast_tests COMMAND couchcast_tests)

# ---------------------------------------------------------------------------
# Install: binary + desktop entry + icon into the prefix (default ~/.local), so
# the app can be launched from a menu or added to Steam as a non-Steam game.
# Mirrors the Rust `cargo xtask install`. Configure with
#   -DCMAKE_INSTALL_PREFIX=$HOME/.local
# then `cmake --install build`.
# ---------------------------------------------------------------------------
include(GNUInstallDirs)
set(APP_ID io.github.gehhilfe.Couchcast)

install(TARGETS couchcast RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})

# Rewrite the desktop entry's Exec to the absolute installed path so it launches
# regardless of PATH (matches the Rust installer).
file(READ ${CMAKE_CURRENT_SOURCE_DIR}/data/${APP_ID}.desktop _desktop_contents)
string(REPLACE "Exec=couchcast"
  "Exec=${CMAKE_INSTALL_FULL_BINDIR}/couchcast" _desktop_contents "${_desktop_contents}")
file(GENERATE OUTPUT ${CMAKE_BINARY_DIR}/${APP_ID}.desktop CONTENT "${_desktop_contents}")

install(FILES ${CMAKE_BINARY_DIR}/${APP_ID}.desktop
  DESTINATION ${CMAKE_INSTALL_DATADIR}/applications)
install(FILES ${CMAKE_CURRENT_SOURCE_DIR}/data/icons/${APP_ID}.svg
  DESTINATION ${CMAKE_INSTALL_DATADIR}/icons/hicolor/scalable/apps)

# Refresh the desktop database (best-effort).
install(CODE "execute_process(COMMAND update-desktop-database \
  ${CMAKE_INSTALL_FULL_DATADIR}/applications ERROR_QUIET RESULT_VARIABLE _ignored)")
