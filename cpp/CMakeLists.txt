# SmartLife Tuya Desktop — Windows C++ frontend (MSVC, windows-latest).
cmake_minimum_required(VERSION 3.20)
project(SmartLifeTuyaDesktop CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# WIN32 = GUI subsystem (no console window). The .rc embeds the app
# icon (ID 1) so the taskbar shows the real icon, never a black square.
add_executable(smartlife_tuya_desktop WIN32 main.cpp core.h app.rc)
target_link_libraries(smartlife_tuya_desktop PRIVATE user32 gdi32 shell32)

# The Rust core is loaded at runtime (LoadLibrary), never linked, so
# no import library is needed from either toolchain.

# Source files are UTF-8 (emoji, triangles, curly quotes in login
# text). Without /utf-8, MSVC reads them in the ANSI codepage and
# the login header renders as mojibake (the 2026-10-27 screenshots).
# NOTE: this must be target_compile_options, not add_compile_options
# after the target — directory options added after add_executable
# do not reach the target, which is exactly how the first /utf-8
# attempt silently failed to fix the mojibake. The literals are
# also written as \u escapes now, so rendering no longer depends
# on the flag alone.
if(MSVC)
  target_compile_options(smartlife_tuya_desktop PRIVATE /utf-8)
endif()
