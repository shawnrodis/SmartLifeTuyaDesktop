// Rust core bridge (RustDesk-style: Rust logic, Flutter UI) for
// SmartLifeTuyaDesktop.
//
// The Tuya cloud logic, config file, RTSP/ffmpeg pumps, gateway
// URLs, autostart store, tray actions, monitor enumeration and
// update check live in rust/ (another agent owns that tree) and are
// exposed as a plain C ABI. This file is the Dart side of that
// contract, mirroring the pattern proven in FileNamerAI:
//
//   * Hand-written C-ABI FFI, NOT flutter_rust_bridge codegen:
//     codegen output is version-sensitive to the Flutter/Dart
//     toolchain and cannot be verified on a Linux VM without a
//     Flutter SDK, whereas a C ABI cdylib needs only `cargo build`
//     on the Actions runner.
//   * `Utf8` is imported UNPREFIXED from package:ffi/ffi.dart.
//     NEVER `ffi.Utf8` from dart:ffi — that was a real build
//     failure in FileNamerAI (dart:ffi has no `Utf8` symbol; the
//     type lives in package:ffi).
//   * `Isolate.run` in current Dart takes ONLY a computation —
//     wrap every isolate call as `Isolate.run(() => _fn(args))`.
//     NEVER `Isolate.run(fn, message)` (removed two-arg form).
//   * Every function takes JSON in and returns JSON out (a single
//     UTF-8 string), freed with `tuya_free_string`. A null return
//     is an error, never a silent empty result.
//   * If the native library is missing, these calls throw — they
//     do NOT fall back to Dart re-implementations of the Tuya API,
//     config handling or RTSP decoding, so a missing Rust build
//     can never silently become a pure-Dart app.
//
// CONTRACT — symbol names the Rust core must export (exact):
//   tuya_version, tuya_load_config, tuya_save_config, tuya_login,
//   tuya_refresh, tuya_control, tuya_build_sections,
//   tuya_camera_start, tuya_camera_frame, tuya_camera_stop,
//   tuya_find_ffmpeg, tuya_update_check, tuya_tray_actions,
//   tuya_single_instance, tuya_window_events,
//   tuya_autostart_get, tuya_autostart_set, tuya_monitors,
//   tuya_free_string.
//
// JSON shapes (all responses carry `ok: bool` and, on failure,
// `error: String`):
//   tuya_version() -> {"ok":true,"version":"1.5.0"}
//   tuya_load_config() -> {"ok":true,"config":{...}} where config
//     is the egui Config JSON verbatim: {creds?, session?, prefs,
//     layout, cameras, update, cache_devices?}. `creds`/`session`
//     are present only as presence markers plus `user_name` — the
//     obfuscated password and tokens NEVER cross into Dart.
//   tuya_save_config(configJson) -> {"ok":true} — Dart passes the
//     whole config map back (with creds/session preserved from the
//     load, untouched); Rust writes config.json.
//   tuya_login(req) -> {"ok":true,"session":{...},"devices":[parsed…]}
//     or {"ok":false,"error":"…"}. "devices" are PARSED device
//     objects with the egui Device struct's field names (id, name,
//     dev_type, kind, icon_kind, is_scene, online, state,
//     has_brightness, has_temp, has_color, brightness_pct,
//     temp_slider, hue, saturation, color_mode) — Dart renders
//     them directly and never re-parses raw discovery payloads.
//     req is either
//     {"username","password","region","platform","keep":bool} for
//     an interactive login, or {"silent":true} for a saved-login
//     silent re-login, in which case Rust uses the stored creds
//     itself (Dart never sees the stored password) and returns
//     {"ok":false,"error":"no_saved_login"} when none is stored.
//   tuya_refresh(req) -> {"ok":true,"session":{...},"devices":[parsed…]}
//     or {"ok":true,"rate_limited":true,"session":{...}} (the egui
//     DiscFrequent path: keep last states, stretch the interval)
//     or {"ok":false,"error":"…"}. req: {"force":bool}.
//   tuya_control(req) -> {"ok":true} / {"ok":false,"error":"…"}.
//     req: {"id","command","value"} where command is one of
//     turnOnOff (value 0/1), brightnessSet (device-scale %,
//     11..100), colorTemperatureSet (slider 1000..10000),
//     colorSet ({"hue":0..360,"saturation":0..1,"brightness":%}),
//     and group broadcasts are issued by Dart as one call per
//     member, in turn, exactly like the egui group fan-out.
//   tuya_build_sections(req) -> {"ok":true,"sections":[{key,
//     order_key, title, kind, group_id?, ids:[…]}]} built with the
//     egui rules (default order Cameras → Pinned → Lighting →
//     groups in creation order → Ungrouped → Scenes; Pinned only
//     when non-empty, Scenes only when non-empty; A–Z by display
//     name — alias wins, lowercase — unless a custom order is
//     saved). Camera entries in the devices list additionally
//     carry `rtsp_url` (exact address for Copy URL / Watch),
//     `tile_rtsp_url` (Stream 2 for Tapo templates) and
//     `gateway_watch_url` (null when no gateway is configured),
//     all computed by Rust because only Rust can deobfuscate the
//     stored camera password. req: {"devices":[parsed…],"cameras":[…],
//     "layout":{…},"query":""} — the query is applied by Rust
//     with the egui filter (display name or raw name contains it,
//     case-insensitive); hidden ids are excluded.
//   tuya_camera_start(req) -> {"ok":true,"mode":"tile"|"player"|
//     "external"|"gateway","url"?} . req: {"camera_id","mode"}.
//     "tile" starts the tile pump (Stream 2 for Tapo templates,
//     card quality for Wide tiles); "player" starts the big-player
//     pump at the quality picked on the card; "external" hands the
//     stream to the external player command and returns its url;
//     "gateway" returns the gateway watch-page url to open.
//   tuya_camera_frame(req) -> {"ok":true,"rgba_base64":"…",
//     "width":N,"height":N,"state":"live"|"paused"|"backoff"|
//     "error","reason"?} — the LATEST frame only; Rust drops
//     older frames rather than queueing them. req:
//     {"camera_id","pump":"tile"|"player"}. When no frame is ready
//     yet, state is "starting" and rgba_base64 is absent.
//   tuya_camera_stop(req) -> {"ok":true}. req: {"camera_id",
//     "pump":"tile"|"player"}.
//   tuya_find_ffmpeg(req) -> {"ok":true,"path":"…"} or
//     {"ok":true,"path":null}. req: {"override":""} ("" = bundled
//     copy beside the app exe, then PATH).
//   tuya_update_check(req) -> {"ok":true,"info":{version,
//     page_url, asset_name, asset_url, asset_size}|null} for
//     {"action":"check","manual":bool}; for {"action":"download"}
//     it returns {"ok":true,"downloaded_path":"…"} on success and
//     the Dart side then asks Rust to install via
//     {"action":"install","path":"…"} (Windows: run the Setup .exe
//     and exit; macOS: open the .dmg). Asset names are verified
//     by Rust exactly as the egui build did
//     (SmartLifeTuyaDesktop-Setup-vX.exe / -vX.dmg).
//   tuya_tray_actions(req) -> {"ok":true}. req: {"action":
//     "ensure"|"hide"|"show"|"quit"} — registers/uses the native
//     tray or menu-bar icon. The icon is kept alive by Rust for
//     the session (a previous version dropped it immediately, so
//     clicks could never arrive); the Flutter shell hides/shows
//     its window via window_manager and drains events with
//     tuya_window_events below.
//   tuya_single_instance() -> {"ok":true,"role":
//     "primary"|"secondary"} — MUST be called exactly once at
//     startup, before any window is shown. On secondary the
//     acquire itself already signals the primary (Windows event
//     / Unix show-request file), so the shell just exits.
//   tuya_window_events() -> {"ok":true,"actions":["show"|
//     "quit",…],"show_requested":bool} — tray actions drained
//     from the queue plus the single-instance show request
//     (consumed, not peeked). The shell polls this ~1s.
//   tuya_autostart_get() -> {"ok":true,"enabled":bool,
//     "launch_mode":"normal"|"minimized"|"maximized"} — the OS
//     store is the source of truth, as in egui.
//   tuya_autostart_set(req) -> {"ok":true} / error. req:
//     {"enabled":bool,"launch_mode":"…"}.
//   tuya_monitors() -> {"ok":true,"monitors":[{device, name,
//     rect_px:[x,y,w,h], primary:bool}]} — present only where the
//     Rust core implements enumeration; Dart treats a missing
//     symbol as "no displays found", never as an error to retry.
//     Moving the Flutter window onto a chosen monitor is done by
//     the shell with window_manager (see main.dart): unmaximize,
//     setPosition to the Rust snap target point (monitor.rs
//     snap_target_point_px maths, converted to logical pixels),
//     then maximize — the same move-then-maximize sequence as
//     egui v1.3.1, verified afterwards against the monitor rect.
//
// Library file names (must match the Rust cdylib output, which
// the Actions workflow copies next to the Flutter exe / .app):
//   Windows: tuya_core.dll · macOS: libtuya_core.dylib ·
//   Linux: libtuya_core.so

import 'dart:convert';
import 'dart:ffi' as ffi;
import 'dart:io';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

typedef _TuyaStringNative = ffi.Pointer<Utf8> Function(
    ffi.Pointer<Utf8>);
typedef _TuyaStringDart = ffi.Pointer<Utf8> Function(
    ffi.Pointer<Utf8>);
typedef _TuyaNoArgNative = ffi.Pointer<Utf8> Function();
typedef _TuyaNoArgDart = ffi.Pointer<Utf8> Function();
typedef _TuyaFreeNative = ffi.Void Function(ffi.Pointer<Utf8>);
typedef _TuyaFreeDart = void Function(ffi.Pointer<Utf8>);

class RustBridge {
  RustBridge._();

  static ffi.DynamicLibrary _open() {
    // NOTE: file names must match the Rust cdylib output (see the
    // header comment). If the Rust core has not been built yet,
    // this throws — the UI surfaces that error, it never pretends
    // the core exists.
    if (Platform.isWindows) {
      return ffi.DynamicLibrary.open('tuya_core.dll');
    }
    if (Platform.isMacOS) {
      return ffi.DynamicLibrary.open('libtuya_core.dylib');
    }
    return ffi.DynamicLibrary.open('libtuya_core.so');
  }

  static String _readAndFree(
      ffi.DynamicLibrary lib, ffi.Pointer<Utf8> ptr) {
    if (ptr == ffi.nullptr) {
      throw StateError('Rust core returned a null string');
    }
    final result = ptr.toDartString();
    final free =
        lib.lookupFunction<_TuyaFreeNative, _TuyaFreeDart>(
            'tuya_free_string');
    free(ptr);
    return result;
  }

  static Map<String, dynamic> _decode(String s) {
    final decoded = jsonDecode(s);
    if (decoded is! Map<String, dynamic>) {
      throw StateError('Rust core returned non-object JSON');
    }
    return decoded;
  }

  static String _callString(
      ffi.DynamicLibrary lib, String symbol, String inputJson) {
    final fn = lib.lookupFunction<_TuyaStringNative, _TuyaStringDart>(
        symbol);
    final ptr = inputJson.toNativeUtf8();
    try {
      return _readAndFree(lib, fn(ptr));
    } finally {
      malloc.free(ptr);
    }
  }

  static String _callNoArg(ffi.DynamicLibrary lib, String symbol) {
    final fn =
        lib.lookupFunction<_TuyaNoArgNative, _TuyaNoArgDart>(symbol);
    return _readAndFree(lib, fn());
  }

  /// Synchronous version string (fast, runs directly).
  static Map<String, dynamic> version() {
    return _decode(_callNoArg(_open(), 'tuya_version'));
  }

  /// Synchronous config load (fast, local file read).
  static Map<String, dynamic> loadConfig() {
    return _decode(_callNoArg(_open(), 'tuya_load_config'));
  }

  /// Synchronous config save (fast, local file write).
  static Map<String, dynamic> saveConfig(
      Map<String, dynamic> config) {
    final lib = _open();
    return _decode(
        _callString(lib, 'tuya_save_config', jsonEncode(config)));
  }

  // ---- isolate entry points (top-level: only simple data crosses
  // ---- the isolate boundary; each opens the library itself) ----

  static String _loginIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_login', requestJson);
  }

  static String _refreshIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_refresh', requestJson);
  }

  static String _controlIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_control', requestJson);
  }

  static String _buildSectionsIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_build_sections', requestJson);
  }

  static String _cameraStartIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_camera_start', requestJson);
  }

  static String _cameraFrameIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_camera_frame', requestJson);
  }

  static String _cameraStopIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_camera_stop', requestJson);
  }

  static String _findFfmpegIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_find_ffmpeg', requestJson);
  }

  static String _updateCheckIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_update_check', requestJson);
  }

  static String _trayActionsIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_tray_actions', requestJson);
  }

  static String _autostartGetIsolate() {
    final lib = _open();
    return _callNoArg(lib, 'tuya_autostart_get');
  }

  static String _autostartSetIsolate(String requestJson) {
    final lib = _open();
    return _callString(lib, 'tuya_autostart_set', requestJson);
  }

  static String _monitorsIsolate() {
    final lib = _open();
    return _callNoArg(lib, 'tuya_monitors');
  }

  /// Interactive or silent login. Network-bound: runs in an
  /// isolate so the UI thread is never blocked by Tuya.
  static Future<Map<String, dynamic>> login(
      Map<String, dynamic> request) async {
    final json =
        await Isolate.run(() => _loginIsolate(jsonEncode(request)));
    return _decode(json);
  }

  /// Device-list refresh. Network-bound: isolate.
  static Future<Map<String, dynamic>> refresh(
      {bool force = false}) async {
    final json = await Isolate.run(
        () => _refreshIsolate(jsonEncode({'force': force})));
    return _decode(json);
  }

  /// One device command. Isolate (network-bound, and control
  /// retries happen Rust-side on rate-limit, as in egui).
  static Future<Map<String, dynamic>> control(
      String id, String command, dynamic value) async {
    final json = await Isolate.run(() => _controlIsolate(
        jsonEncode({'id': id, 'command': command, 'value': value})));
    return _decode(json);
  }

  /// Section list for the dashboard. Fast: runs directly.
  static Map<String, dynamic> buildSections(
      Map<String, dynamic> request) {
    final lib = _open();
    return _decode(_callString(
        lib, 'tuya_build_sections', jsonEncode(request)));
  }

  /// Start a camera pump. Isolate (spawns ffmpeg Rust-side).
  static Future<Map<String, dynamic>> cameraStart(
      String cameraId, String mode) async {
    final json = await Isolate.run(() => _cameraStartIsolate(
        jsonEncode({'camera_id': cameraId, 'mode': mode})));
    return _decode(json);
  }

  /// Latest frame only for a camera pump. Isolate; called on the
  /// tile/player poll timer, never awaited on the UI thread.
  static Future<Map<String, dynamic>> cameraFrame(
      String cameraId, String pump) async {
    final json = await Isolate.run(() => _cameraFrameIsolate(
        jsonEncode({'camera_id': cameraId, 'pump': pump})));
    return _decode(json);
  }

  /// Stop a camera pump. Isolate.
  static Future<void> cameraStop(String cameraId, String pump) async {
    await Isolate.run(() => _cameraStopIsolate(
        jsonEncode({'camera_id': cameraId, 'pump': pump})));
  }

  /// ffmpeg lookup. Isolate (stats several paths).
  static Future<Map<String, dynamic>> findFfmpeg(
      String override) async {
    final json = await Isolate.run(
        () => _findFfmpegIsolate(jsonEncode({'override': override})));
    return _decode(json);
  }

  /// Update check / download / install. Network-bound: isolate.
  static Future<Map<String, dynamic>> updateCheck(
      Map<String, dynamic> request) async {
    final json = await Isolate.run(
        () => _updateCheckIsolate(jsonEncode(request)));
    return _decode(json);
  }

  /// Single-instance role. Runs directly (fast, local only).
  /// Call exactly once at startup; see the contract above.
  static Map<String, dynamic> singleInstance() {
    return _decode(_callNoArg(_open(), 'tuya_single_instance'));
  }

  /// Window events (tray actions + single-instance show).
  /// Runs directly (fast, local only); polled ~1s by the shell.
  static Map<String, dynamic> windowEvents() {
    return _decode(_callNoArg(_open(), 'tuya_window_events'));
  }

  /// Tray / menu-bar icon actions. Runs directly (fast).
  static Map<String, dynamic> trayActions(String action) {
    final lib = _open();
    return _decode(_callString(
        lib, 'tuya_tray_actions', jsonEncode({'action': action})));
  }

  /// Autostart store read. Isolate (registry / LaunchAgent read).
  static Future<Map<String, dynamic>> autostartGet() async {
    final json = await Isolate.run(() => _autostartGetIsolate());
    return _decode(json);
  }

  /// Autostart store write. Isolate.
  static Future<Map<String, dynamic>> autostartSet(
      bool enabled, String launchMode) async {
    final json = await Isolate.run(() => _autostartSetIsolate(
        jsonEncode({'enabled': enabled, 'launch_mode': launchMode})));
    return _decode(json);
  }

  /// Monitor enumeration. Returns {"ok":false,"error":"…"} when
  /// the symbol is absent — Dart treats that as "no displays
  /// found", never as a retryable error.
  static Future<Map<String, dynamic>> monitors() async {
    try {
      final json = await Isolate.run(() => _monitorsIsolate());
      return _decode(json);
    } catch (_) {
      return {
        'ok': false,
        'error': 'tuya_monitors not present in this Rust core build',
      };
    }
  }
}
