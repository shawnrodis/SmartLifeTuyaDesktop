# SmartLife Tuya Desktop — Windows C++ plan (branch `windows-cpp`)

Decision (Shawn, 2026-10-27): Windows moves from Flutter to C++.
Mac stays egui/Rust on `main`. Flutter work on `flutter-rust` is parked,
not deleted and not published as v1.5.0.

## 1:1 rule (Shawn, 2026-10-27 — applies here in full)

Looks and functionality must match egui 1.4.0 1:1 as far as technically
possible — 1.4.0 is the build that worked great (tag v1.4.0; verified
by diff, v1.4.1 changed only the renderer Glow -> wgpu, no UI or
features). A genuine gap is reported as a blocker, never silently
dropped. That rule is why this is NOT a stock grey Win32 dialog app:
the UI is custom-painted (dark Black theme by default, indigo accent
#6366f1) to match the existing app's look, not the system default.
EXCLUDED from 1:1: 1.4.0's window trailing and moving-to-another-monitor
glitches. Those are egui bugs to fix with the native window, not
behaviour to copy — smooth drag and clean monitor moves are pass/fail
tests for every C++ milestone.

## Architecture: Rust core stays, C++ owns the window

- `rust/` (`tuya_core`, already extracted on `flutter-rust`) stays the
  backend: config, Tuya cloud API, device/section model, camera frame
  pumps, tray/autostart/single-instance helpers. Tokens/passwords never
  leave Rust (same presence-marker contract as `rust/README-API.md`).
- `cpp/` is a native Win32 C++ frontend that loads `tuya_core.dll` at
  runtime (`LoadLibrary` + `GetProcAddress`, single-JSON C ABI — no
  codegen, no import-library toolchain coupling). Same pattern as the
  Flutter bridge, minus Flutter.
- Installer: NSIS, per-user `%LOCALAPPDATA%\SmartLifeTuyaDesktop`,
  same folder/shortcuts/Add-Remove entry as egui/Flutter, so it installs
  seamlessly over egui v1.4.1 and the Flutter test builds.
- Version: stays 1.5.0 for the Windows line (same as the Flutter plan),
  so the update check sees one lineage. Mac egui stays on its own line.

## 1:1 inventory (from egui v1.4.1 / Flutter `lib/main.dart`)

- Login: house icon, title, username/password, region, platform,
  keep-login, silent re-login on launch, Rust-error box if core fails.
- Dashboard sections in order: Cameras → Pinned → Lighting → groups →
  Ungrouped → Scenes. Empty sections dropped except in Edit mode.
- Device cards: on/off, brightness, temperature, colour, online state,
  pin, group assign, hide (Settings → Hidden devices).
- Groups: create/rename/delete, assign devices and cameras.
- Scenes: run, never leave the Scenes section, non-scenes never enter.
- Cameras: tiles 480×270 (Tapo Stream 2), Watch player 960×540,
  gateway watch page, VLC/external fallback via bundled ffmpeg,
  Copy URL, card resize in Edit mode (Normal / Large / Wide),
  Add camera (RTSP, Tapo account, custom path).
- Edit mode: reorder, pin, group assignment, camera resize.
- Settings: launch mode (normal / minimized-to-tray / maximized),
  close-to-tray, tray, autostart, stream gateway & cameras, ffmpeg path,
  external player (VLC default), hidden devices, themes (Black default,
  accent #6366f1), and all 8 Help paragraphs verbatim.
- Window: snap/move/maximize, launch `--minimized`/`--maximized`/saved
  mode, mixed-DPI snap (165 Hz/100% → 60 Hz/175%) as the decisive test.
- Tray: icon (embedded .ico resource — NOT a black square), hide on
  close, click-to-restore, Quit on tray menu, tray balloon.
- Single-instance: second launch signals the primary, then exits.
- Updates: check on start, verified download + delayed installer launch
  (same pattern as Utility Closet / File Namer auto-install).

## Dashboard visual spec (Shawn's screenshot, 2026-10-27, egui 1.4.0)

The screenshot Shawn sent on 2026-10-27 IS the dashboard spec — do not
redesign it, do not simplify it into a generic list. (The screenshot
itself stays OUT of the repo: it shows his own camera view and home
devices. This description is the spec, not the image.)

- Header: house + app title, Updated timestamp, Search devices,
  Autorefresh interval dropdown, Monitor / Refresh / Edit / Settings /
  Log out pill buttons, all on black.
- Camera section: one large card per camera with the live image,
  stream address + quality line, Stream 1 HD / Stream 2 SD pills,
  Watch, Copy URL, Open in external player, pin + close icons.
- Section headers with counts: Lighting (6), Ungrouped (9), Scenes (3).
- Lighting broadcast bar: All toggle, Brightness %, Colour temperature
  slider, full colour-swatch row.
- Light cards: icon, name, On + brightness + colour-temp status, pin,
  close, indigo toggle when on, Brightness slider, Colour temperature
  slider, two swatch rows.
- Plug/switch cards: icon, name, On/Off status, pin, indigo toggle.
- Scene cards: icon, name, "Scene — tap Start to run", indigo
  Start scene button.
- Look: black background, dark cards with thin indigo-tinted borders,
  rounded corners, indigo accent for on-states and primary buttons.
  Stock Win32 controls cannot reproduce this — the dashboard is
  custom-painted, like the login header, or it is not 1:1.

## Milestones

1. Scaffold (this commit): Win32 window shell, Black background,
   app title, embedded icon resource, `tuya_core.dll` loaded at
   runtime, core version shown — proves window + icon + DLL linkage
   on Shawn's machine before any feature work.
2. Login + silent re-login + dashboard shell (sections, device list).
3. Device/group/scene controls, Edit mode.
4. Cameras (tiles, Watch, gateway, VLC fallback).
5. Settings (all items + Help text), tray, launch modes, updater.
6. Shawn's full Windows test list, then merge to `main` and publish
   v1.5.0 for Windows. Mac egui is untouched throughout.

## Mac scope (Shawn, 2026-10-27)

Mac egui is already rock solid and needs no feature work. Its only
change is a version bump to match Windows: when Windows C++ ships as
v1.5.0, Mac gets a v1.5.0 release from the same egui code on `main`
with only the version strings changed. No Mac Flutter/C++ port.
Correction (Shawn, 2026-10-27, verified against GitHub releases):
Mac's latest released version is 1.4.0 — v1.4.1 shipped a Windows
setup exe only, no Mac DMG. So the Mac bump is 1.4.0 → 1.5.0, and
"latest release v1.4.1" must never be read as a Mac version.

## What is NOT happening

- No Flutter Windows release. No Mac feature work, no Mac port.
- No pure-C++ rewrite of the Tuya API logic — the Rust core is proven
  (egui used it, Flutter used it) and rewriting it in C++ would add
  risk without adding reliability where it matters (the window).
- The Flutter black-window diagnosis is parked with the branch: the
  diag log is still welcome if Shawn sends it, but it no longer gates
  the Windows line.
