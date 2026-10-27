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
