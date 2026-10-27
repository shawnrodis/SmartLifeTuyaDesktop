## FFI contract — single-JSON API (matches lib/rust_bridge.dart, 2026-10-27 fix)

Every exported function takes AT MOST ONE string argument, and that
argument is always a single JSON object (or, for no-arg functions,
nothing). Sections below that describe multi-arg signatures are
SUPERSEDED by this contract — do not use them. Dart's bridge uses one typedef for all string functions
(`Pointer<Utf8> Function(Pointer<Utf8>)`), so a multi-argument C ABI
would corrupt the call — the original multi-arg exports were replaced
for exactly that reason. Do NOT reintroduce multi-arg exports.

- `tuya_version()` -> `{"ok":true,"version":"1.5.0"}`
- `tuya_load_config()` -> `{"ok":true,"config":{...}}` where creds/session
  are presence markers only (`user_name`/`country_code`/`platform`/
  `has_password`, `region`/`expires_at_ms`/`has_token`) — tokens and
  passwords NEVER cross into Dart. Dart holds the marker, Rust holds
  the secret in config.json.
- `tuya_save_config(config)` -> `{"ok":true}`. Preserves stored
  creds/session on marker objects, clears them on explicit null
  (logout), replaces them only on a full object with access_token.
- `tuya_login({username,password,region,platform,keep} | {silent:true})`
  -> `{"ok":true,"session":<stripped>,"devices":[parsed Device...]}` or
  `{"ok":false,"error":"no_saved_login"}`. Devices are PARSED Device
  objects (id, name, dev_type, kind, icon_kind, is_scene, online,
  state, has_brightness, has_temp, has_color, brightness_pct,
  temp_slider, hue, saturation, color_mode), NOT raw Tuya payloads.
  Session + cache_devices are persisted by Rust on success; creds
  only when keep=true.
- `tuya_refresh({force})` -> ok/session/devices, or
  `{"ok":true,"rate_limited":true,"session":...}` (DiscFrequent).
- `tuya_control({id,command,value})` -> `{"ok":true,"outcome":"ok"}`.
- `tuya_build_sections({devices,cameras,layout,query})` ->
  `{"ok":true,"sections":[{key,order_key,title,kind,group_id,ids,items}]}`.
  Camera items additionally carry `rtsp_url`, `tile_rtsp_url`,
  `gateway_watch_url` (computed by Rust, which deobfuscates).
- `tuya_camera_start({camera_id,mode})`, mode tile|player|gateway|external.
  Pump keys are `{item_id}:{mode}` (e.g. `cam:{id}:tile`), so tile and
  player pumps for one camera do not collide. Tile = 480x270 Stream 2
  (Tapo), player = 960x540 card quality.
- `tuya_camera_frame({camera_id,pump})` ->
  `{"ok":true,"state":"live|connecting|backoff","width":N,"height":N,
  "rgba_base64":"..."}` with rgba_base64 OMITTED when no frame is ready
  yet; unknown pump -> `{"ok":false,"state":"ended","error":...}`.
- `tuya_camera_stop({camera_id,pump})` -> `{"ok":true}`.
- `tuya_update_check({action:"check"|"download"|"install",...})` ->
  check returns `{"ok":true,"info":{...}}`; download returns
  `{"ok":true,"downloaded_path":"...","asset_name":"..."}` after exact
  asset-name verification; install runs/opens the verified file.
- `tuya_tray_actions({action:"ensure"|""})` -> `{"ok":true,"actions":[...]}`.
  "ensure" best-effort builds the native tray icon (no-op on Linux).


# tuya_core — Rust core for SmartLifeTuyaDesktop (Flutter edition)

Version **1.5.0**. A Rust library crate (`cdylib` + `rlib`, library name
`tuya_core` → `tuya_core.dll` / `libtuya_core.dylib` / `libtuya_core.so`)
extracted from the original Rust+UI app's `src/main.rs` (v1.4.1), stripped
of **all** UI-framework code. There is no `eframe`, `egui`, `Color32`,
`Ui`, texture-handle, or UI-context type anywhere in this crate
(verified by grep). The Flutter shell owns all rendering, window
management, and user interaction; this crate owns config, the Tuya
cloud API, the device/section model, camera frame pumps, and the
platform tray/autostart/single-instance helpers that do not need a UI.

Architecture: RustDesk-style, **manual C-ABI FFI — NOT
flutter_rust_bridge codegen** (same pattern as FileNamerAI commit
`ee1db4e`).

## Build & test (Linux)

```sh
cd rust
cargo build        # cdylib (libtuya_core.so) + rlib
cargo test         # 11 ported pure-logic tests, no network
```

Builds lean: the only heavy dependency is `reqwest` (blocking,
`rustls-tls`, `json`), exactly as in the original. `tray-icon` is a
Windows/macOS-only dependency; `windows-sys` is Windows-only; `fs2`
is Unix-only. On Linux the crate has no system-library requirements
beyond a C toolchain.

## FFI contract (all functions)

- Every function returning text returns a NUL-terminated UTF-8 C string
  allocated by Rust. **The caller MUST free it with
  `tuya_free_string`.** Freeing with the system `free()` is wrong.
- Every JSON-returning function returns either the documented success
  shape or `{"error": "..."}`. Error strings never contain passwords,
  tokens, or full RTSP URLs (credentials are never logged anywhere).
- A null input pointer is treated as an empty string.
- `tuya_version()` returns the plain string `1.5.0-rust` (not JSON).

### `tuya_version() -> "1.5.0-rust"`

No arguments, never fails.

### `tuya_free_string(ptr)`

Frees a string previously returned by any `tuya_*` function.
Null-safe (no-op on null).

### `tuya_load_config() -> Config JSON`

Reads the platform config file and returns the whole `Config`
document (see "Config shape" below). Reading also performs the
original v1.2.1 camera-id repair: duplicate or empty camera ids are
reassigned (first record keeps its id) and the repaired config is
persisted immediately, exactly as the original `load_config` did.

Config path: Windows `%APPDATA%\SmartLifeTuyaDesktop\config.json`;
macOS `~/Library/Application Support/SmartLifeTuyaDesktop/config.json`;
else `$XDG_CONFIG_HOME/SmartLifeTuyaDesktop/config.json` (or
`~/.config/...`).

### `tuya_save_config(config_json) -> {"ok":true} | {"error":...}`

`config_json` is a complete `Config` document (missing fields fall
back to defaults, matching the original serde defaults). Persists it
with pretty JSON to the platform config path.

### `tuya_login(creds_json) -> {"session":{...},"devices":[...]} | {"error":...}`

`creds_json` (flat or nested — both accepted):

```json
{
  "user_name": "user@example.com",
  "password": "<plain password for this call>",
  "country_code": "1",
  "platform": "smartlife",
  "prefs": { "...": "optional Prefs overrides, partial OK" }
}
```

or `{"creds": {...}, "prefs": {...}}`. If
`"password_obfuscated": true` is present, `password` is interpreted as
the stored XOR+base64 form and deobfuscated in memory for this call
only. The core does **not** persist credentials on login; the shell
calls `tuya_save_config` when it wants them stored (with the password
already obfuscated, as the original `Creds.password` field stores it).

On success the session is the original `Session` shape
(`access_token`, `refresh_token`, `expires_at_ms`, `region` — region
derived from the token prefix `EU`/`AY`/`AZ`, else from the country
code). `devices` is the raw discovery array after the original
`merge_device_data` against the cached devices from config (so thin
payloads keep previously-known fields). A rate-limited discovery
(`FrequentlyInvoke`) falls back to the cached devices instead of
erroring, as in the original. No network call is made by any other
function in this document except `tuya_login`, `tuya_refresh`,
`tuya_control`, and `tuya_update_check`.

### `tuya_refresh(session_json, prefs_json) -> {"session":{...},"devices":[...]} | {"error":...}`

`session_json` is a `Session` document; `prefs_json` is a (possibly
partial) `Prefs` document, or empty for the stored prefs. Behaviour
mirrors the original `fresh_session`: a session still valid for more
than 60 s is returned as-is; otherwise the refresh token is tried,
and if there is no usable refresh token the call errors
(`"Session expired"` — the shell should then call `tuya_login`;
the core never invents credentials). Discovery then runs as in
`tuya_login`.

### `tuya_control(device_id, command_json) -> {"ok":true,"outcome":"ok"} | {"ok":false,"outcome":"failed"} | {"error":...}`

`command_json`:

```json
{ "action": "turnOnOff", "value": 1 }
```

`action` may also be given as `command` or `name`; `value` defaults
to JSON null. Supported actions are passed through to the Tuya
`skill` control endpoint exactly as in the original (`turnOnOff`,
`brightnessSet`, `colorTemperatureSet`, `colorSet`, scene runs, …);
for `colorSet` the payload uses the `color` key, everything else uses
`value` (original quirk, preserved). Rate-limited commands are
retried silently up to 2 extra times ~1 s apart (original
`net_control`); only a genuine failure (or a third rate-limit)
returns `outcome: "failed"`.

Session/prefs: if `command_json` contains `session` (and optionally
`prefs`), those are used; otherwise the stored config session/prefs
are used. Missing session → `{"error": "no stored session; call
tuya_login first"}`.

### `tuya_build_sections(devices_json, layout_json) -> {"sections":[...]} | {"error":...}`

`devices_json` is an array of raw Tuya discovery device objects
(`{id, name, dev_type, data}` — stringly-typed light readings such as
`"brightness": "800"` are parsed, as in the original v1.3.0 fix) and/or
camera records (a `Camera` JSON object, or `{"camera": {...}}`).
`layout_json` is a (possibly partial) `LayoutCfg` document, or
`{"layout": {...}, "cameras": [...]}` to supply cameras separately.

Output sections are in the original order (stored `section_seq`
wins; missing sections slot in by the built-in default
Cameras → Pinned → Lighting → groups (creation order) → Ungrouped →
Scenes, via `effective_section_seq`):

```json
{ "sections": [
  { "key": "cameras", "order_key": "cameras", "title": "Cameras",
    "kind": "Cameras", "group_id": null,
    "items": [ { "... Camera fields ...",
                 "display_name": "<alias or name>",
                 "item_kind": "camera" } ] },
  { "key": "lighting", "order_key": "lighting", "title": "Lighting",
    "kind": "Lighting", "group_id": null,
    "items": [ { "... parsed Device fields ...",
                 "display_name": "<alias or name>",
                 "item_kind": "device" } ] }
]}
```

Notes: pinned items appear in Pinned **and** in their home section
(original behaviour — `build_sections` does not remove them from
Lighting/groups); hidden items are excluded; empty sections are
dropped (dashboard view, `editing = false`); item `name` is the raw
record name and `display_name` is the alias-aware name. Group
broadcast plans are available to rlib callers via `group_plan`,
`group_master_on`, and `group_average` (not exposed over FFI — the
shell fans a plan out through `tuya_control`).

### `tuya_camera_start(camera_json, ffmpeg_path, w, h) -> {"pump_id":"cam:{id}"} | {"error":...}`

`camera_json` is a `Camera` document (stored form: `password` is the
XOR+base64 value; a `plain_password` field is also accepted and is
obfuscated in memory before URL building). `ffmpeg_path` is the
Settings override ("" = bundled copy beside the exe, then PATH —
same lookup order as the original `find_ffmpeg`). `w`/`h` are the
pump frame size in pixels (0/0 → 480×270 tile size).

URL rule (original tile/player split): requests at tile size
(≤ 480 wide) use the cheap Stream 2 on Tapo templates
(`tile_rtsp_url`); larger requests use the camera's own quality
selection (`build_rtsp_url`). Starting a pump with an existing id
stops the old one first. Error when ffmpeg cannot be found.

### `tuya_camera_frame(pump_id) -> frame JSON | {"state":"ended","error":...}`

```json
{ "state": "live|connecting|backoff", "w": 480, "h": 270,
  "rgba_base64": "<raw RGBA bytes, base64 — latest frame only>",
  "error": "<present only when a mapped ffmpeg failure reason exists>",
  "backoff_secs": 3.2 }
```

Only the **latest** frame is kept — older frames are dropped, never
queued (original behaviour). `"state": "ended"` with an error is
returned for an unknown pump id. Backoff/retry scheduling is the
shell's job: when `state` is `backoff`, wait `backoff_secs` (or the
original 2 s / 4 s / 8 s schedule from `pump_backoff`) and call
`tuya_camera_start` again. The pump itself never re-spawns ffmpeg
on its own.

### `tuya_camera_stop(pump_id) -> {"ok":true}`

Stops the pump (flags the worker and kills ffmpeg) and removes it
from the registry. Always ok, even for an unknown id.

### `tuya_find_ffmpeg(override_json) -> {"path":"..."} | {"path":null}`

`override_json` may be a plain string (`"/usr/bin/ffmpeg"`), a JSON
string, or an object with `ffmpeg_path` / `override` / `path`. The
lookup order is the original: bundled copy beside the exe, then the
override (a path, or a bare name resolved against PATH), then PATH.
`null` means "previews off, Watch falls back" in the original's terms.

### `tuya_update_check(current_version) -> ReleaseInfo JSON | {"error":...}`

Fetches the GitHub `releases/latest` endpoint for
`shawnrodis/SmartLifeTuyaDesktop` and selects this platform's asset
(Windows: `SmartLifeTuyaDesktop-Setup-v{version}.exe`; macOS:
`SmartLifeTuyaDesktop-v{version}.dmg`; other platforms: the `.exe`
name, as in the original fallback):

```json
{ "version": "1.5.0", "page_url": "https://github.com/.../releases/latest",
  "asset_name": "...", "asset_url": "...", "asset_size": 123,
  "update_available": true, "current_version": "1.4.1",
  "install": "run_installer_and_exit|open_disk_image" }
```

`update_available` is a numeric version comparison
(`1.10.0 > 1.9.0`). The core only *reports* — downloading, verifying,
running the installer, or opening the DMG is the shell's job (see
Gaps). Network errors and responses without a compatible asset
return `{"error":...}`.

### `tuya_tray_actions() -> {"actions":[...]}`

Drains the static tray-action queue (see "Tray" below).
`actions` is an array of `"show"` / `"quit"` strings, in queue order.
Draining is destructive — a second call returns an empty array until
new actions arrive.

### `tuya_autostart_get() -> {"ok":true,"enabled":bool,"launch_mode":"normal|..."}`
### `tuya_autostart_set(json) -> {"ok":true} | {"error":...}`

`json`: `{"enabled": true, "launch_mode": "minimized"}` (the
`enabled` key may also be `start_with_windows`). The OS autostart
store is the source of truth (Windows HKCU Run key; macOS
LaunchAgent plist), exactly as in the original `reconcile_autostart`
/ `apply_autostart`. On success the prefs are also persisted via
`save_config`. On Linux the core has **no** autostart store, so
enabling returns an error (disabling succeeds trivially, since no
entry exists); the Flutter shell owns autostart on Linux if it wants
one. See Gaps.

### `tuya_monitors() -> {"monitors":[...]}`

```json
{ "monitors": [ { "name": "Dell U2415", "device": "\\\\.\\DISPLAY1",
  "handle": 123, "rect_px": [0, 0, 2560, 1440], "scale": 1.0,
  "primary": true } ] }
```

Live enumeration on Windows (EnumDisplayMonitors + GetMonitorInfoW +
per-monitor DPI + friendly name). On macOS/Linux this returns an
empty array, exactly as the original non-Windows stub did — the snap
feature is Windows-only by design (see Gaps).

## Config shape (field names preserved from `src/main.rs`)

`Config`: `creds` (nullable), `session` (nullable), `prefs`,
`layout`, `cameras`, `update`, `cache_devices` (nullable raw JSON).

`Creds`: `user_name`, `password` (XOR+base64 obfuscated — see below),
`country_code`, `platform`.

`Session`: `access_token`, `refresh_token` (nullable),
`expires_at_ms`, `region`.

`Prefs` defaults (fresh install): `theme: "black"`,
`accent: "#6366f1"`, `density: "comfortable"`, `autorefresh: true`,
`interval: 30`, `proxy_preset: "auto"`, `proxy_custom: ""`,
`gateway_url: ""`, `gateway_type: "go2rtc"`, `player_cmd: "vlc"`,
`previews: true`, `ffmpeg_path: ""`, `start_with_windows: false`,
`launch_mode: "normal"`, `close_to_tray: false`,
`tray_notice_shown: false`.

`LayoutCfg`: `groups: [{id, name}]`, `assign: {item_id: group_id}`,
`alias: {item_id: name}`, `hidden: [item_id]`, `pins: [item_id]`,
`section_order: {section_key: [item_id]}`, `collapsed: {section_key:
bool}`, `section_seq: [section order_key]`,
`cam_sizes: {item_id: "large"|"wide"}`. Item ids are device ids, or
`"cam:{camera_id}"` for cameras.

`Camera`: `id`, `name`, `user`, `password` (obfuscated),
`host`, `port` (default 554), `template` (`tapo1` | `tapo2` |
`custom`), `path`.

`UpdateCfg`: `last_check_ms`, `dismissed`.

## Obfuscation (unchanged, do not "improve")

Passwords (`Creds.password`, `Camera.password`) are stored as
XOR-each-byte-with-`0x5A`, then standard base64 — **exactly** the
original scheme, byte-for-byte compatible with existing config files
and with the web edition. This is obfuscation, not encryption, and
the code says so wherever the scheme is mentioned. It is preserved
deliberately so existing configs keep working; do not weaken it
further and do not replace it unilaterally (that would invalidate
every stored credential). The core never logs a password, token, or
full RTSP URL.

## Direct rlib surface (beyond FFI)

The Flutter shell can also use the crate directly. Highlights
(all functions are the originals, minus UI code):

- `config`: `config_path`, `config_dir`, `load_config`,
  `save_config`, `new_camera_id`, `remove_camera_record`,
  `repair_camera_ids`.
- `obfuscate`: `obfuscate`, `deobfuscate`, `b64_encode`,
  `b64_decode`, `now_ms`.
- `region`: `region_base`, `region_key_from_code`,
  `region_from_token`, `apply_proxy`, `url_encode`.
- `convert`: `brightness_to_pct`, `temp_reported_to_slider`,
  `temp_slider_to_reported`, `temp_slider_to_kelvin`,
  `temp_kelvin_to_slider`, `temp_stop_index`, `temp_stop_kelvin`,
  `temp_display_kelvin`, `hex_to_hsv`, `hsv_to_hex`,
  `norm_saturation`, `pct_encode_component`.
- `camera`: `camera_path`, `build_rtsp_url`, `tile_rtsp_url`,
  `tile_feed`, `gateway_watch_url`, `camera_status_text`,
  `cam_size_of`, `CamSize`, `open_external`.
- `player`: `resolve_player_program`, `launch_player`,
  `player_candidates`, `vlc_known_dirs`, `app_paths_lookup`.
- `ffmpeg` / `pump`: `ffmpeg_args`, `ffmpeg_candidates`,
  `find_ffmpeg`, `pump_backoff`, `ffmpeg_failure_reason`,
  `pump_action`, `tile_pump_wanted`, `backoff_wake_delay`,
  `pump_source_key`, `spawn_pump` (no UI context argument),
  `Pump::{stop, note_frame, note_ended}`.
- `registry`: global pump registry (`registry_start/stop/poll`,
  `registry_backoff_secs`, `registry_exists`).
- `device`: `parse_device`, `merge_device_data`, `device_is_light`,
  `device_entry`, `camera_entry`, `build_sections`,
  `order_sections`, `effective_section_seq`, `default_section_seq`,
  `moved_section_seq`, `order_section_items`, `filter_entries`,
  `pack_rows`, `group_plan`, `group_master_on`, `group_average`,
  `brightness_ui_to_device`, `brightness_device_to_ui`,
  `brightness_set_value`, `display_name`, `entry_display_name`,
  `status_text`.
- `net`: `http_client` (blocking reqwest, rustls-tls, json, 30 s
  timeout), `parse_auth_response`, `net_auth`, `net_refresh`,
  `fresh_session`, `skill_body`, `net_skill`, `net_discovery`,
  `is_rate_limit_code`, `control_outcome`, `net_control_once`,
  `net_control`.
- `update`: `expected_update_asset_name_for`,
  `select_update_asset`, `update_install_for`, `parse_version`,
  `version_newer`, `parse_release`, `ReleaseInfo`.
- `monitor` / `snap` / `win32win`: `MonitorInfo`,
  `enumerate_monitors`, `monitor_rect_points`,
  `monitor_orientation`, `snap_target_point_px`, `snap_axis`,
  `monitor_index_at`, `SnapPhase`, `SnapAction`, `SnapFacts`,
  `snap_next`, `SnapJob`, and the Win32 window ops
  (`win32_needs_restore`, `win32_restore_window`,
  `win32_maximize_window`, `win32_move_window`,
  `win32_window_monitor`, `win32_window_size_px`) — real on
  Windows, inert stubs elsewhere.
- `launch`: `LaunchMode`, `launch_mode_from_args`,
  `launch_mode_from_command`, `resolve_launch_mode`,
  `autostart_command`, `desired_autostart`.
- `system`: tray queue (`push_tray_action`,
  `drain_tray_actions`, `tray_action_for_icon_parts`,
  `tray_action_for_menu_id`), `close_action`, `build_tray`,
  `spawn_tray_thread` (Windows; see Gaps), single-instance guard
  (`acquire_single_instance`, `InstanceRole`, `SHOW_REQUESTED`,
  `show_requested_take`), autostart store (`reg_run_value`,
  `reg_set_run_value`, `reconcile_autostart`, `apply_autostart`,
  `autostart_get`, `launch_agent_plist` / `launch_agent_command` /
  `parse_autostart_command`, `parse_reg_named_value`).

## Tray (in the core, with one shell-owned piece)

- In the core: the static action queue, the pure event→action
  mappings, `close_action` (Quit vs HideToTray), `build_tray`
  (Windows/macOS tray icon + Open/Quit menu via `tray-icon`), and
  the Windows dedicated tray thread (`spawn_tray_thread(open_id,
  quit_id)`), which owns both tray event receivers and pushes
  `Show`/`Quit` onto the queue even when no UI frame runs (the
  original v1.2.3 fix).
- In the Flutter shell: acting on drained actions. The original
  tray thread also poked a UI-framework repaint and restored the
  window directly in Win32 (`ShowWindowAsync` on a stored HWND) —
  that part cannot live in a UI-free core, because the core has no
  window handle. The shell must restore/show its own window on
  `"show"` and run its real quit path on `"quit"`.
- Tray **hide-on-close** itself: the *decision* is in the core
  (`close_action`); the *hiding* is in Flutter (the shell keeps
  running with its window hidden while close-to-tray is on, and the
  tray icon stays alive for the session). Not a gap — split
  deliberately, decision in core, window in Flutter.
- Tray icon asset: the original embedded `assets/tray_icon.rgba`
  from the app crate. The core's `build_tray` reads the same RGBA
  bytes from the `TUYA_TRAY_ICON_RGBA` environment path when set,
  else uses a blank 64×64 icon so construction never fails. The
  Flutter shell should supply the real asset (or use its own tray
  implementation and only use the core's queue).
- The one-time "still running in the tray" balloon
  (`show_tray_balloon`, a transient Win32 notify icon against the
  main window's HWND) is **dropped** — see Gaps.

## Explicit gaps — features dropped from the core, with reason

These were in the original app but cannot live in a UI-free core
without faking a UI framework. Nothing below is silently missing:
each states whether it belongs in Flutter, or is a genuine gap.

1. **Window move / maximize (snap execution) — Flutter.** The pure
   snap maths *is* in the core (`snap_next` state machine,
   `snap_target_point_px`, monitor enumeration on Windows, and the
   Win32 ops `win32_restore_window` / `win32_move_window` /
   `win32_maximize_window` / `win32_window_monitor`). What was
   dropped is the per-frame *driver* (`App::advance_snap`): it
   advanced the state machine once per UI frame, read fullscreen /
   zoomed state from the UI framework, and issued UI-framework
   viewport commands (e.g. leaving fullscreen). The Flutter shell
   must drive `snap_next` itself and call the Win32 ops with its own
   window handle. The core deliberately has no `main_hwnd`
   (extracting an HWND from a UI-framework `Frame` is UI code).
2. **Snap-to-monitor menu — Flutter.** The menu itself (listing
   monitors, marking the current one) is UI. The core supplies
   `tuya_monitors` / `enumerate_monitors`,
   `monitor_index_at`, and the snap maths above.
3. **Drag-reorder geometry — Flutter.** Dragging cards/sections to
   reorder them (press position, drag threshold, drop targets,
   `pack_rows` aside) is pointer + layout geometry owned by the
   UI. The core keeps the *results* of a reorder: `order_sections`,
   `effective_section_seq`, `moved_section_seq`,
   `order_section_items`, `pack_rows`, and the layout fields
   (`section_order`, `section_seq`, `pins`) that persist it. The
   shell writes the new order into layout and calls
   `tuya_save_config`; it must not invent geometry in the core.
4. **Hide-on-close window handling — Flutter (decision in core).**
   See "Tray" above. `close_action` is in the core; actually hiding
   / restoring the window is the shell's.
5. **One-time tray balloon — gap (dropped, no replacement).**
   `show_tray_balloon` needed a main-window HWND to attach a
   transient Win32 notify icon to. The Flutter shell can show its
   own notification if it wants one; the core does not attempt it.
   Cosmetic only (a single informational balloon).
6. **Auto-update download/install/exit — Flutter.** The core
   checks (`tuya_update_check`) and classifies the install kind
   (`run_installer_and_exit` vs `open_disk_image`). Downloading
   and verifying the asset, running the Windows Setup exe and
   exiting, or opening the macOS DMG, are shell responsibilities —
   the core must never exit the host process or replace its own
   binary.
7. **Single-instance "show window" delivery — split.** The guard
   itself is in the core (Windows named mutex + event; Unix
   file lock via `fs2`, extended to Linux where the original had
   no guard). The *delivery* differs: on Windows the waiter thread
   pushes `TrayAction::Show` onto the tray queue (the shell acts
   on it); on macOS the original primary polled a `show-request`
   file from its frame loop — the shell must poll
   `show_request_path` (config dir + `show-request`) itself. The
   `SHOW_REQUESTED` flag is also exposed for shells that prefer it.
8. **macOS menu-bar tray polling — Flutter.** On macOS the
   original drained tray receivers from the frame loop; there is
   no frame loop in the core, so the shell drains the queue via
   `tuya_tray_actions` (or wires its own tray listener to
   `push_tray_action`). The Windows dedicated thread exists in
   the core; a macOS equivalent thread is not provided because
   the original deliberately did not use one there.
9. **Linux autostart store — gap (shell-owned).** The original had
   no Linux build (Windows Run key / macOS LaunchAgent only).
   `tuya_autostart_set(enabled=true)` on Linux returns an error
   rather than pretending to work. If the Flutter shell ships on
   Linux, it should implement XDG autostart itself and keep
   `Prefs.start_with_windows` in sync.
10. **Linux/macOS monitor enumeration — faithful stub, not a gap
    in behaviour.** `tuya_monitors` returns `[]` off Windows,
    exactly as the original's non-Windows `enumerate_monitors`
    stub did. A Flutter shell that wants real multi-monitor data
    on macOS/Linux must enumerate displays itself (e.g. via
    Flutter plugins) — the core will not fake it.
11. **All rendering, dialogs, settings forms, login/dashboard UI,
    toasts, sliders, colour pickers, camera tiles/players, and
    font/icon handling — Flutter, by design.** Nothing from the
    original `App` struct, its `update` loop, or its widget
    helpers (`toggle_switch`, `slim_slider`, `swatch`,
    `play_pause_chip`, header budgets) is in the core; those are
    presentation, not logic. The one exception callers should
    know about: `status_text` (the card status line, e.g.
    `"On · 80% · 4000K"`) *is* in the core because it is pure
    device-state formatting, and the shell may reuse it verbatim.

## What was NOT changed

- Field names, JSON shapes, defaults, conversion scales
  (brightness UI 1..100 ↔ device 11..100; colour-temp slider
  1000..10000 ↔ reported scale; five Kelvin stops
  2700/3000/4000/5000/6500), RTSP URL rules, proxy rules, backoff
  schedule (2 s / 4 s / 8 s), rate-limit handling, and the
  XOR+base64 obfuscation are all byte-for-byte the original's.
- The blocking reqwest client (rustls-tls, json, 30 s timeout)
  is unchanged.
- No secrets are logged, and no credential value appears in any
  error string or FFI output (sessions are returned to the shell
  because the shell needs the token to call the API — that is the
  documented FFI contract, not logging).
