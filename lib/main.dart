// SmartLifeTuyaDesktop — Flutter UI (Rust core + Flutter UI,
// RustDesk-style), 1:1 with the egui build (src/main.rs, v1.4.1)
// as technically possible in Flutter. Every egui screen, section
// and control is replicated here with the same labels and effects;
// anything that genuinely cannot be replicated is marked with a
// TODO(GAP) comment in code and listed in the conversion report —
// nothing is silently dropped.
//
// Layout of this file: app + state, config helpers, section
// ordering (egui build_sections mirror), toasts, login view,
// dashboard header, sections, device cards, camera cards, group
// controls, camera form, watch player, settings, update flow.
//
// GAP NOTES (explicit, not pretended):
//   TODO(GAP) tray hide-on-close: Flutter cannot intercept the
//     window close button or hide the window without the
//     window_manager plugin (not in this app's dependency set).
//     The setting is saved and the Rust tray icon is registered
//     via tuya_tray_actions, but closing the window currently
//     quits the app. The Settings panel says so in plain text.
//   TODO(GAP) snap-to-monitor / window move / maximize: moving
//     the Flutter window onto a chosen monitor, and the
//     launch-as Maximized/Minimized behaviours, need
//     window_manager. The 🖥 Monitor menu lists live displays
//     from tuya_monitors (when present) but selecting one only
//     explains the gap — it does not move the window.
//   TODO(GAP) native window drag: Flutter windows are dragged by
//     the OS title bar, not by app code; there is no Dart API to
//     reproduce egui/winit's drag handling. Not a UI feature gap:
//     the standard title bar drags the window normally.
//   TODO(GAP) grid drag geometry: egui dragged cards by pointer
//     position over measured cell rects with an 8px promote
//     threshold. Flutter exposes no drag-start threshold on
//     Draggable, so cards use LongPressDraggable (long-press to
//     start) + DragTarget, with the ↑/↓ buttons as the exact
//     secondary method. The reorder effects are identical.
//   TODO(GAP) single-instance / tray-icon click-to-restore: the
//     egui Win32 mutex + tray menu live in the native shell, not
//     in the Flutter window. Rust tray_actions covers the icon;
//     click-to-restore needs window_manager and is not wired.
//   TODO(GAP) colour picker: egui used its native sRGB colour
//     editor. Flutter has no built-in colour editor, so "More
//     colours…" opens RGB sliders + a hex field + Apply colour
//     with the identical effect (colorSet with the chosen HSV).

import 'dart:async';
import 'dart:convert';
import 'dart:io' show Platform, Process, exit;
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'rust_bridge.dart';
import 'theme.dart';

void main() {
  runApp(const TuyaApp());
}

const String appVersion = '1.5.0';
const String releasesPage =
    'https://github.com/shawnrodis/SmartLifeTuyaDesktop/releases/latest';
const String releasesApi =
    'https://api.github.com/repos/shawnrodis/SmartLifeTuyaDesktop/releases/latest';

class TuyaApp extends StatelessWidget {
  const TuyaApp({super.key});

  @override
  Widget build(BuildContext context) {
    return const MaterialApp(
      title: 'SmartLife & Tuya Desktop',
      debugShowCheckedModeBanner: false,
      home: HomePage(),
    );
  }
}

enum AppView { login, dashboard, settings }

class _Toast {
  final String text;
  final bool isError;
  final int id;
  _Toast(this.text, this.isError, this.id);
}

/// One dashboard entry: a Tuya device or a user camera. Field
/// names match the egui Device struct / Camera struct (parsed
/// device maps from the Rust core, camera maps from config).
class _Entry {
  final String id;
  final String name;
  final bool isCamera;
  final bool isScene;
  final bool light;
  final Map<String, dynamic> raw;
  _Entry({
    required this.id,
    required this.name,
    required this.isCamera,
    required this.isScene,
    required this.light,
    required this.raw,
  });
}

class _Section {
  final String key;
  final String orderKey;
  final String title;
  final String kind; // cameras|pinned|lighting|group|ungrouped|scenes
  final String? groupId;
  final List<String> ids;
  _Section({
    required this.key,
    required this.orderKey,
    required this.title,
    required this.kind,
    required this.groupId,
    required this.ids,
  });
}

class HomePage extends StatefulWidget {
  const HomePage({super.key});

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  // ---- config (whole egui Config JSON, Rust is the file owner) --
  Map<String, dynamic> _config = {};
  bool _configLoaded = false;
  String? _bridgeError;

  Map<String, dynamic> get _prefs =>
      (_config['prefs'] as Map<String, dynamic>?) ?? {};
  Map<String, dynamic> get _layout =>
      (_config['layout'] as Map<String, dynamic>?) ?? {};

  String get _theme => _prefs['theme'] as String? ?? 'black';
  String get _accentHex => _prefs['accent'] as String? ?? defaultAccentHex;
  bool get _compact => _prefs['density'] == 'compact';
  bool get _autorefresh => _prefs['autorefresh'] != false;
  int get _interval =>
      (_prefs['interval'] is num) ? (_prefs['interval'] as num).toInt() : 30;

  AppPalette get _pal =>
      paletteFor(_theme, MediaQuery.platformBrightnessOf(context));
  Color get _accent => hexToColor(_accentHex);

  // ---- view / session ----
  AppView _view = AppView.login;
  bool _loggedIn = false;
  List<Map<String, dynamic>> _devices = [];
  String _lastUpdated = '';
  DateTime? _lastFetch;
  bool _discBusy = false;
  bool _refreshBusy = false;
  DateTime? _backoffUntil;
  int _freqStreak = 0;

  // ---- login form ----
  final _liUser = TextEditingController();
  final _liPass = TextEditingController();
  String _liRegion = '1';
  String _liPlatform = 'smart_life';
  bool _liKeep = true;
  bool _loginBusy = false;
  String? _loginError;
  bool _showProxy = false;

  // ---- dashboard state ----
  final _query = TextEditingController();
  final _searchFocus = FocusNode();
  String _queryText = '';
  bool _editing = false;
  bool _sliderActive = false;
  final List<_Toast> _toasts = [];
  int _toastSeq = 0;

  // ---- update flow ----
  Map<String, dynamic>? _updateInfo;
  bool _updateChecking = false;
  String? _updateError;
  List<int>? _updateDl; // [got, total]

  // ---- cameras ----
  final Set<String> _pausedCams = {};
  final Set<String> _inPlayer = {};
  final Map<String, ui.Image?> _frames = {};
  final Map<String, String> _frameState = {}; // live|starting|backoff|error
  final Map<String, String> _frameReason = {};
  bool _framePollBusy = false;
  String? _ffmpegPath;
  bool _ffmpegChecked = false;

  // ---- autostart (OS store is the source of truth) ----
  bool _autostartEnabled = false;
  String _launchMode = 'normal';

  // ---- editing buffers ----
  final Map<String, TextEditingController> _aliasControllers = {};
  final Map<String, TextEditingController> _groupControllers = {};
  final Map<String, Color> _colorPick = {};
  String? _openColorFor;

  Timer? _tickTimer;
  Timer? _frameTimer;

  // Section cache: rebuilt when devices/layout/query/editing change.
  List<_Section>? _sectionsCache;
  String _sectionsCacheKey = '';

  @override
  void initState() {
    super.initState();
    _query.addListener(() {
      if (_queryText != _query.text) {
        setState(() => _queryText = _query.text);
      }
    });
    _init();
    _tickTimer = Timer.periodic(const Duration(seconds: 1), (_) => _tick());
    _frameTimer =
        Timer.periodic(const Duration(milliseconds: 500), (_) => _pollFrames());
  }

  @override
  void dispose() {
    _tickTimer?.cancel();
    _frameTimer?.cancel();
    _liUser.dispose();
    _liPass.dispose();
    _query.dispose();
    _searchFocus.dispose();
    for (final c in _aliasControllers.values) {
      c.dispose();
    }
    for (final c in _groupControllers.values) {
      c.dispose();
    }
    super.dispose();
  }

  // ================= config =================

  Map<String, dynamic> _defaultConfig() {
    return {
      'creds': null,
      'session': null,
      'prefs': {
        'theme': 'black',
        'accent': defaultAccentHex,
        'density': 'comfortable',
        'autorefresh': true,
        'interval': 30,
        'proxy_preset': 'auto',
        'proxy_custom': '',
        'gateway_url': '',
        'gateway_type': 'go2rtc',
        'player_cmd': 'vlc',
        'previews': true,
        'ffmpeg_path': '',
        'start_with_windows': false,
        'launch_mode': 'normal',
        'close_to_tray': false,
        'tray_notice_shown': false,
      },
      'layout': {
        'groups': [],
        'assign': {},
        'alias': {},
        'hidden': [],
        'pins': [],
        'section_order': {},
        'collapsed': {},
        'section_seq': [],
        'cam_sizes': {},
      },
      'cameras': [],
      'update': {'last_check_ms': 0, 'dismissed': ''},
    };
  }

  Future<void> _init() async {
    try {
      final res = RustBridge.loadConfig();
      if (res['ok'] == true && res['config'] is Map<String, dynamic>) {
        _config = res['config'] as Map<String, dynamic>;
      } else {
        _config = _defaultConfig();
      }
    } catch (e) {
      // The Rust core is not built yet (or the library is absent).
      // The UI still renders so the gap is visible, never hidden.
      _bridgeError = 'Rust core unavailable: $e';
      _config = _defaultConfig();
    }
    _configLoaded = true;
    // Prefill the login form from a saved login, if any.
    final creds = _config['creds'];
    if (creds is Map<String, dynamic>) {
      _liUser.text = creds['user_name'] as String? ?? '';
      final region = creds['country_code'] as String? ?? '1';
      _liRegion =
          regions.any((e) => e.$1 == region) ? region : '1';
      final platform = creds['platform'] as String? ?? 'smart_life';
      _liPlatform =
          platforms.any((e) => e.$1 == platform) ? platform : 'smart_life';
    }
    if (mounted) setState(() {});
    // Silent re-login when a login is saved (egui: session token
    // refresh / saved-login sign-in on startup). Dart never sees
    // the stored password: Rust uses its own stored creds.
    if (creds != null || _config['session'] != null) {
      await _silentLogin();
    }
    _maybeUpdateCheck(manual: false);
    _refreshAutostart();
    _refreshFfmpeg();
  }

  Future<void> _saveConfig() async {
    try {
      RustBridge.saveConfig(_config);
    } catch (_) {
      // egui's save is silent; a failed save must not spam toasts.
    }
    _sectionsCache = null;
  }

  void _mutPrefs(String key, dynamic value) {
    (_config['prefs'] as Map<String, dynamic>?)?.putIfAbsent(key, () => null);
    _prefs[key] = value;
    _saveConfig();
    setState(() {});
  }

  void _mutLayout(String key, dynamic value) {
    _layout[key] = value;
    _saveConfig();
    setState(() {});
  }

  List<Map<String, dynamic>> get _cameras {
    final list = _config['cameras'];
    if (list is List) {
      return list.whereType<Map<String, dynamic>>().toList();
    }
    return [];
  }

  List<Map<String, dynamic>> get _groups {
    final list = (_layout['groups'] as List?) ?? [];
    return list.whereType<Map<String, dynamic>>().toList();
  }

  Map<String, String> get _assign {
    final m = _layout['assign'];
    if (m is Map) {
      return m.map((k, v) => MapEntry(k.toString(), v.toString()));
    }
    return {};
  }

  Map<String, String> get _alias {
    final m = _layout['alias'];
    if (m is Map) {
      return m.map((k, v) => MapEntry(k.toString(), v.toString()));
    }
    return {};
  }

  List<String> get _hidden {
    final l = _layout['hidden'];
    if (l is List) return l.map((e) => e.toString()).toList();
    return [];
  }

  List<String> get _pins {
    final l = _layout['pins'];
    if (l is List) return l.map((e) => e.toString()).toList();
    return [];
  }

  Map<String, List<String>> get _sectionOrder {
    final m = _layout['section_order'];
    final out = <String, List<String>>{};
    if (m is Map) {
      m.forEach((k, v) {
        if (v is List) out[k.toString()] = v.map((e) => e.toString()).toList();
      });
    }
    return out;
  }

  Map<String, bool> get _collapsed {
    final m = _layout['collapsed'];
    final out = <String, bool>{};
    if (m is Map) {
      m.forEach((k, v) => out[k.toString()] = v == true);
    }
    return out;
  }

  Map<String, String> get _camSizes {
    final m = _layout['cam_sizes'];
    final out = <String, String>{};
    if (m is Map) {
      m.forEach((k, v) => out[k.toString()] = v.toString());
    }
    return out;
  }

  String _displayName(String id, String name) => _alias[id] ?? name;

  // ================= entries & sections =================
  // Mirror of egui build_sections / order_section_items /
  // filter_entries / effective_section_seq. The Rust core exposes
  // the same computation as tuya_build_sections; Dart recomputes
  // it locally so the dashboard renders even when the core's
  // section export is absent, and the two must agree (same rules
  // documented in rust_bridge.dart).

  List<_Entry> _allEntries() {
    final entries = <_Entry>[];
    for (final d in _devices) {
      final id = d['id']?.toString() ?? '';
      final kind = d['kind'] as String? ?? '';
      entries.add(_Entry(
        id: id,
        name: d['name']?.toString() ?? id,
        isCamera: false,
        isScene: d['is_scene'] == true,
        light: kind == 'light',
        raw: d,
      ));
    }
    for (final c in _cameras) {
      final id = 'cam:${c['id']}';
      entries.add(_Entry(
        id: id,
        name: c['name']?.toString() ?? id,
        isCamera: true,
        isScene: false,
        light: false,
        raw: c,
      ));
    }
    return entries;
  }

  List<_Entry> _filterEntries(List<_Entry> entries) {
    final q = _queryText.trim().toLowerCase();
    return entries.where((e) {
      if (_hidden.contains(e.id)) return false;
      if (q.isEmpty) return true;
      return _displayName(e.id, e.name).toLowerCase().contains(q) ||
          e.name.toLowerCase().contains(q);
    }).toList();
  }

  List<String> _orderSectionItems(
      List<_Entry> items, List<String>? customOrder) {
    final byId = {for (final e in items) e.id: e};
    final used = <String>{};
    final out = <String>[];
    if (customOrder != null) {
      for (final id in customOrder) {
        if (byId.containsKey(id) && used.add(id)) out.add(id);
      }
    }
    final rest =
        items.where((e) => !used.contains(e.id)).toList()
          ..sort((a, b) {
            final na = _displayName(a.id, a.name).toLowerCase();
            final nb = _displayName(b.id, b.name).toLowerCase();
            final c = na.compareTo(nb);
            return c != 0 ? c : a.id.compareTo(b.id);
          });
    out.addAll(rest.map((e) => e.id));
    return out;
  }

  int _sectionSlot(String kind) {
    switch (kind) {
      case 'cameras':
        return 0;
      case 'pinned':
        return 1;
      case 'lighting':
        return 2;
      case 'group':
        return 3;
      case 'ungrouped':
        return 4;
      case 'scenes':
        return 5;
      default:
        return 3;
    }
  }

  List<_Section> _buildSections() {
    final key =
        '${_devices.length}:${_cameras.length}:${_layout.hashCode}:$_queryText:$_editing';
    if (_sectionsCache != null && _sectionsCacheKey == key) {
      return _sectionsCache!;
    }
    final visible = _filterEntries(_allEntries());
    final scenes = visible.where((e) => e.isScene).toList();
    final normal = visible.where((e) => !e.isScene).toList();
    final pinSet = _pins.toSet();
    final groupIds = _groups.map((g) => g['id'].toString()).toSet();
    bool assigned(_Entry e) {
      final g = _assign[e.id];
      return g != null && groupIds.contains(g);
    }

    final order = _sectionOrder;
    final sections = <_Section>[];
    sections.add(_Section(
      key: 'cameras',
      orderKey: 'cameras',
      title: 'Cameras',
      kind: 'cameras',
      groupId: null,
      ids: _orderSectionItems(
          normal.where((e) => !assigned(e) && e.isCamera).toList(),
          order['cameras']),
    ));
    final pinned =
        normal.where((e) => pinSet.contains(e.id)).toList();
    if (pinned.isNotEmpty) {
      sections.add(_Section(
        key: 'pinned',
        orderKey: 'pinned',
        title: 'Pinned',
        kind: 'pinned',
        groupId: null,
        ids: _orderSectionItems(pinned, order['pinned']),
      ));
    }
    sections.add(_Section(
      key: 'lighting',
      orderKey: 'lighting',
      title: 'Lighting',
      kind: 'lighting',
      groupId: null,
      ids: _orderSectionItems(
          normal.where((e) => !assigned(e) && e.light).toList(),
          order['lighting']),
    ));
    for (final g in _groups) {
      final gid = g['id'].toString();
      sections.add(_Section(
        key: gid,
        orderKey: 'group:$gid',
        title: g['name']?.toString() ?? gid,
        kind: 'group',
        groupId: gid,
        ids: _orderSectionItems(
            normal.where((e) => _assign[e.id] == gid).toList(),
            order['group:$gid']),
      ));
    }
    sections.add(_Section(
      key: 'ungrouped',
      orderKey: 'ungrouped',
      title: 'Ungrouped',
      kind: 'ungrouped',
      groupId: null,
      ids: _orderSectionItems(
          normal
              .where((e) => !assigned(e) && !e.light && !e.isCamera)
              .toList(),
          order['ungrouped']),
    ));
    if (scenes.isNotEmpty) {
      sections.add(_Section(
        key: 'scenes',
        orderKey: 'scenes',
        title: 'Scenes',
        kind: 'scenes',
        groupId: null,
        ids: _orderSectionItems(scenes, order['scenes']),
      ));
    }
    // Effective section sequence: stored order wins for sections
    // that exist; a missing section slots in after its nearest
    // already-placed default-order predecessor (or at the front).
    final stored = (_layout['section_seq'] as List?)
            ?.map((e) => e.toString())
            .toList() ??
        [];
    final present = sections.map((s) => s.orderKey).toSet();
    final eff = stored.where((k) => present.contains(k)).toList();
    final defaults = List<_Section>.from(sections)
      ..sort((a, b) =>
          _sectionSlot(a.kind).compareTo(_sectionSlot(b.kind)));
    for (var i = 0; i < defaults.length; i++) {
      final k = defaults[i].orderKey;
      if (eff.contains(k)) continue;
      var at = 0;
      for (var j = i - 1; j >= 0; j--) {
        final p = eff.indexOf(defaults[j].orderKey);
        if (p >= 0) {
          at = p + 1;
          break;
        }
      }
      eff.insert(at, k);
    }
    final rank = {for (var i = 0; i < eff.length; i++) eff[i]: i};
    sections.sort((a, b) => (rank[a.orderKey] ?? 99)
        .compareTo(rank[b.orderKey] ?? 99));
    // Empty sections are dropped, except in Edit mode where group,
    // lighting and cameras sections stay so items can be dropped
    // into them.
    sections.retainWhere((s) =>
        s.ids.isNotEmpty ||
        (_editing &&
            (s.kind == 'group' ||
                s.kind == 'lighting' ||
                s.kind == 'cameras')));
    _sectionsCache = sections;
    _sectionsCacheKey = key;
    return sections;
  }

  _Entry? _entryById(String id) {
    for (final e in _allEntries()) {
      if (e.id == id) return e;
    }
    return null;
  }

  Map<String, dynamic>? _deviceById(String id) {
    for (final d in _devices) {
      if (d['id']?.toString() == id) return d;
    }
    return null;
  }

  Map<String, dynamic>? _cameraByItemId(String itemId) {
    for (final c in _cameras) {
      if ('cam:${c['id']}' == itemId) return c;
    }
    return null;
  }

  // ================= toasts =================

  void _toast(String text, {bool isError = false}) {
    final id = ++_toastSeq;
    setState(() => _toasts.add(_Toast(text, isError, id)));
    Timer(const Duration(milliseconds: 3600), () {
      if (!mounted) return;
      setState(() => _toasts.removeWhere((t) => t.id == id));
    });
  }

  Widget _toastLayer() {
    if (_toasts.isEmpty) return const SizedBox.shrink();
    return Align(
      alignment: Alignment.bottomCenter,
      child: Padding(
        padding: const EdgeInsets.only(bottom: 24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: _toasts.map((t) {
            final isLight = _theme == 'light';
            final bg = t.isError
                ? const Color(0xFF4C1120)
                : (_theme == 'black'
                    ? const Color(0xFF14161C)
                    : (_theme == 'light'
                        ? const Color(0xFF17202E)
                        : const Color(0xFF1E2534)));
            final border = t.isError
                ? const Color(0xFF881337)
                : const Color(0xFF333F58);
            return Container(
              margin: const EdgeInsets.only(top: 8),
              padding:
                  const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
              decoration: BoxDecoration(
                color: bg,
                border: Border.all(color: border),
                borderRadius: BorderRadius.circular(11),
              ),
              child: Text(
                t.text,
                style: TextStyle(
                  color: t.isError
                      ? const Color(0xFFFB7185)
                      : (isLight ? Colors.white : _pal.text),
                  fontSize: 13.5,
                ),
              ),
            );
          }).toList(),
        ),
      ),
    );
  }

  // ================= auth / refresh =================

  Future<void> _silentLogin() async {
    try {
      final res = await RustBridge.login({'silent': true});
      if (res['ok'] == true) {
        _applySession(res);
      } else {
        final err = res['error']?.toString() ?? '';
        if (err != 'no_saved_login' && err.isNotEmpty) {
          setState(() => _loginError = err);
        }
      }
    } catch (_) {
      // Stay on the login screen; the bridge error is shown there.
    }
    if (mounted) setState(() {});
  }

  void _applySession(Map<String, dynamic> res) {
    if (res['session'] is Map<String, dynamic>) {
      _config['session'] = res['session'];
    }
    final devices = res['devices'];
    if (devices is List) {
      _devices = devices.whereType<Map<String, dynamic>>().toList();
      _lastFetch = DateTime.now();
      _lastUpdated = 'Updated ${_timeNow()}';
      _freqStreak = 0;
      _backoffUntil = null;
      _saveConfig();
    }
    _loggedIn = true;
    _view = AppView.dashboard;
    _sectionsCache = null;
  }

  String _timeNow() {
    final now = DateTime.now();
    return '${now.hour.toString().padLeft(2, '0')}:'
        '${now.minute.toString().padLeft(2, '0')}';
  }

  Future<void> _submitLogin() async {
    setState(() {
      _loginBusy = true;
      _loginError = null;
    });
    try {
      final res = await RustBridge.login({
        'username': _liUser.text.trim(),
        'password': _liPass.text,
        'region': _liRegion,
        'platform': _liPlatform,
        'keep': _liKeep,
      });
      if (res['ok'] == true) {
        if (_liKeep) {
          _config['creds'] = {
            'user_name': _liUser.text.trim(),
            'country_code': _liRegion,
            'platform': _liPlatform,
          };
        } else {
          _config['creds'] = null;
        }
        _applySession(res);
        _toast('Logged in', isError: false);
      } else {
        setState(() => _loginError =
            res['error']?.toString() ?? 'Login failed — check username, password and region.');
      }
    } catch (e) {
      setState(() => _loginError = 'Login failed: $e');
    }
    setState(() => _loginBusy = false);
  }

  void _logout() {
    _loggedIn = false;
    _devices = [];
    _config['creds'] = null;
    _config['session'] = null;
    _editing = false;
    _saveConfig();
    setState(() => _view = AppView.login);
    _toast('Logged out');
  }

  Future<void> _refresh({bool manual = false}) async {
    if (_discBusy) return;
    if (manual) _backoffUntil = null;
    setState(() {
      _discBusy = true;
      _refreshBusy = manual;
    });
    try {
      final res = await RustBridge.refresh(force: manual);
      if (res['ok'] == true && res['rate_limited'] == true) {
        // egui DiscFrequent: silent. Keep last states, stretch the
        // effective interval (×2, ×4, ×8; at least 60s, at most
        // 5min), no toast, no error.
        _freqStreak++;
        final shift = _freqStreak > 3 ? 3 : _freqStreak;
        var secs = _interval * (1 << shift);
        if (secs < 60) secs = 60;
        if (secs > 300) secs = 300;
        _backoffUntil = DateTime.now().add(Duration(seconds: secs));
        if (_devices.isNotEmpty) _lastFetch = DateTime.now();
        if (res['session'] is Map<String, dynamic>) {
          _config['session'] = res['session'];
          _saveConfig();
        }
      } else if (res['ok'] == true) {
        _applySession(res);
      } else {
        _toast(res['error']?.toString() ?? 'Refresh failed',
            isError: true);
      }
    } catch (e) {
      _toast('Refresh failed: $e', isError: true);
    }
    setState(() {
      _discBusy = false;
      _refreshBusy = false;
    });
  }

  void _tick() {
    if (!mounted) return;
    if (_view != AppView.dashboard ||
        !_loggedIn ||
        !_autorefresh ||
        _editing ||
        _sliderActive ||
        _searchFocus.hasFocus) {
      return;
    }
    final backedOff =
        _backoffUntil != null && DateTime.now().isBefore(_backoffUntil!);
    final due = _lastFetch == null ||
        DateTime.now().difference(_lastFetch!).inSeconds >= _interval;
    if (due && !_discBusy && !backedOff) {
      _refresh();
    }
  }

  // ================= controls =================

  Future<void> _control(
      String id, String command, dynamic value) async {
    try {
      final res = await RustBridge.control(id, command, value);
      if (res['ok'] != true) {
        _toast(res['error']?.toString() ?? 'Command failed',
            isError: true);
      }
    } catch (e) {
      _toast('Command failed: $e', isError: true);
    }
  }

  Future<void> _toggleDevice(Map<String, dynamic> dev) async {
    final id = dev['id'].toString();
    final wasOn = dev['state'] == true;
    setState(() => dev['state'] = !wasOn);
    try {
      final res =
          await RustBridge.control(id, 'turnOnOff', wasOn ? 0 : 1);
      if (res['ok'] != true) {
        setState(() => dev['state'] = wasOn);
        _toast(res['error']?.toString() ?? 'Command failed',
            isError: true);
      }
    } catch (e) {
      setState(() => dev['state'] = wasOn);
      _toast('Command failed: $e', isError: true);
    }
  }

  Future<void> _doColor(String id, String hex) async {
    final dev = _deviceById(id);
    final brightness =
        (dev?['brightness_pct'] is num) ? (dev!['brightness_pct'] as num).toInt() : 100;
    final (h, s, _) = hexToHsv(hex);
    setState(() {
      if (dev != null) {
        dev['hue'] = h;
        dev['saturation'] = s;
        dev['color_mode'] = 'colour';
      }
    });
    await _control(id, 'colorSet', {
      'hue': h,
      'saturation': s,
      'brightness': brightness,
    });
  }

  Future<void> _startScene(String id) async {
    await _control(id, 'turnOnOff', 1);
    _toast('Scene started');
  }

  void _togglePin(String id) {
    final pins = _pins;
    if (pins.contains(id)) {
      pins.remove(id);
    } else {
      pins.add(id);
    }
    _mutLayout('pins', pins);
    _toast(pins.contains(id)
        ? 'Pinned to the top'
        : 'Unpinned');
  }

  void _hideItem(String id) {
    final hidden = _hidden;
    if (!hidden.contains(id)) hidden.add(id);
    _mutLayout('hidden', hidden);
    _toast('Hidden — bring it back in Settings → Hidden devices');
  }

  void _moveInSection(_Section sec, String id, int dir) {
    final order = List<String>.from(sec.ids);
    final i = order.indexOf(id);
    final j = i + dir;
    if (i < 0 || j < 0 || j >= order.length) return;
    final tmp = order[i];
    order[i] = order[j];
    order[j] = tmp;
    final all = _sectionOrder;
    all[sec.orderKey] = order;
    _mutLayout('section_order', all);
  }

  void _moveSection(String orderKey, int dir) {
    final rendered = _buildSections();
    final stored = (_layout['section_seq'] as List?)
            ?.map((e) => e.toString())
            .toList() ??
        [];
    final present = rendered.map((s) => s.orderKey).toSet();
    final eff = stored.where((k) => present.contains(k)).toList();
    final defaults = List<_Section>.from(rendered)
      ..sort((a, b) =>
          _sectionSlot(a.kind).compareTo(_sectionSlot(b.kind)));
    for (var i = 0; i < defaults.length; i++) {
      final k = defaults[i].orderKey;
      if (eff.contains(k)) continue;
      var at = 0;
      for (var j = i - 1; j >= 0; j--) {
        final p = eff.indexOf(defaults[j].orderKey);
        if (p >= 0) {
          at = p + 1;
          break;
        }
      }
      eff.insert(at, k);
    }
    final i = eff.indexOf(orderKey);
    final j = i + dir;
    if (i < 0 || j < 0 || j >= eff.length) return;
    final tmp = eff[i];
    eff[i] = eff[j];
    eff[j] = tmp;
    for (final k in stored) {
      if (!eff.contains(k)) eff.add(k);
    }
    _mutLayout('section_seq', eff);
  }

  /// Drag-and-drop move: drop [draggedId] onto [targetSection]
  /// (before [beforeId], or at the end when null). Mirrors egui:
  /// dragging onto another section moves the item there — group
  /// assignment for groups, pin for Pinned, automatic (assign
  /// removed) for Cameras/Lighting/Ungrouped. Scenes never leave
  /// the Scenes section and non-scenes never enter it.
  void _moveItem(
      String draggedId, _Section targetSection, String? beforeId) {
    final entry = _entryById(draggedId);
    if (entry == null) return;
    if (entry.isScene && targetSection.kind != 'scenes') return;
    if (!entry.isScene && targetSection.kind == 'scenes') return;
    if (targetSection.kind == 'cameras' && !entry.isCamera) return;
    if (targetSection.kind == 'lighting' &&
        !(entry.light || entry.isCamera)) {
      // Lighting is automatic for lights only; a camera dropped
      // here would vanish from its automatic section, so reject.
      if (!entry.light) return;
    }
    final assign = _assign;
    if (targetSection.kind == 'group') {
      assign[draggedId] = targetSection.groupId!;
    } else if (targetSection.kind == 'pinned') {
      final pins = _pins;
      if (!pins.contains(draggedId)) pins.add(draggedId);
      _layout['pins'] = pins;
    } else if (targetSection.kind == 'cameras' ||
        targetSection.kind == 'lighting' ||
        targetSection.kind == 'ungrouped') {
      assign.remove(draggedId);
    }
    _layout['assign'] = assign;
    // Reorder within the target section (cache must be cleared
    // first: the layout map was mutated in place).
    _sectionsCache = null;
    final sections = _buildSections();
    final target = sections.firstWhere(
        (s) => s.orderKey == targetSection.orderKey,
        orElse: () => targetSection);
    final order = List<String>.from(target.ids);
    order.remove(draggedId);
    if (beforeId != null && order.contains(beforeId)) {
      order.insert(order.indexOf(beforeId), draggedId);
    } else {
      order.add(draggedId);
    }
    final all = _sectionOrder;
    all[targetSection.orderKey] = order;
    _layout['section_order'] = all;
    _saveConfig();
    setState(() {});
  }

  void _setEditing(bool on) {
    if (on) {
      for (final d in _devices) {
        final id = d['id'].toString();
        _aliasControllers.putIfAbsent(id, () => TextEditingController())
          ..text = _alias[id] ?? '';
      }
      for (final c in _cameras) {
        final id = 'cam:${c['id']}';
        _aliasControllers.putIfAbsent(id, () => TextEditingController())
          ..text = _alias[id] ?? '';
      }
      for (final g in _groups) {
        final gid = g['id'].toString();
        _groupControllers.putIfAbsent(gid, () => TextEditingController())
          ..text = g['name']?.toString() ?? '';
      }
    }
    setState(() => _editing = on);
  }

  // ================= scaffold =================

  @override
  Widget build(BuildContext context) {
    final pal = _pal;
    Widget body;
    if (!_configLoaded) {
      body = const Center(child: CircularProgressIndicator());
    } else if (_view == AppView.login || !_loggedIn) {
      body = _loginView();
    } else if (_view == AppView.settings) {
      body = _settingsView();
    } else {
      body = _dashboardView();
    }
    return Scaffold(
      backgroundColor: pal.bg,
      body: Stack(
        children: [
          Positioned.fill(child: body),
          _toastLayer(),
        ],
      ),
    );
  }

  Widget _fieldLabel(String text) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: Align(
        alignment: Alignment.centerLeft,
        child: Text(
          text,
          style: TextStyle(
              fontSize: 12.5, color: _pal.dim, fontWeight: FontWeight.bold),
        ),
      ),
    );
  }

  InputDecoration _inputDecoration(String hint) {
    return InputDecoration(
      hintText: hint,
      hintStyle: TextStyle(color: _pal.faint),
      filled: true,
      fillColor: _pal.card,
      contentPadding:
          const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(buttonRadius),
        borderSide: BorderSide(color: _pal.border),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(buttonRadius),
        borderSide: BorderSide(color: _pal.border),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(buttonRadius),
        borderSide: BorderSide(color: _accent),
      ),
    );
  }

  Widget _dropdown<T>({
    required T? value,
    required List<(T?, String)> items,
    required ValueChanged<T?> onChanged,
    double? width,
  }) {
    // Guard against a stored value that is not in the list (e.g. a
    // region/platform/theme from an older config): fall back to
    // the first item instead of throwing in DropdownButton.
    final effectiveValue =
        items.any((e) => e.$1 == value) ? value : items.first.$1;
    return Container(
      width: width,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: BoxDecoration(
        color: _pal.card,
        border: Border.all(color: _pal.border),
        borderRadius: BorderRadius.circular(buttonRadius),
      ),
      child: DropdownButton<T>(
        value: effectiveValue,
        isExpanded: true,
        underline: const SizedBox.shrink(),
        dropdownColor: _pal.card,
        style: TextStyle(color: _pal.text, fontSize: 14),
        items: items
            .map((e) => DropdownMenuItem<T>(value: e.$1, child: Text(e.$2)))
            .toList(),
        onChanged: onChanged,
      ),
    );
  }

  // ================= login =================

  Widget _loginView() {
    final pal = _pal;
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.symmetric(vertical: 36),
        child: Container(
          width: 400,
          padding: const EdgeInsets.fromLTRB(28, 30, 28, 24),
          decoration: BoxDecoration(
            color: pal.card,
            border: Border.all(color: pal.border),
            borderRadius: BorderRadius.circular(loginRadius),
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Text('🏠', style: TextStyle(fontSize: 40)),
              Text('SmartLife & Tuya Desktop',
                  style: TextStyle(
                      fontSize: 23,
                      fontWeight: FontWeight.bold,
                      color: pal.text)),
              const SizedBox(height: 4),
              Text(
                'Your Smart Life / Tuya devices, in one clean dashboard',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 13.5, color: pal.dim),
              ),
              const SizedBox(height: 18),
              if (_bridgeError != null)
                Container(
                  width: double.infinity,
                  padding: const EdgeInsets.symmetric(
                      horizontal: 12, vertical: 9),
                  decoration: BoxDecoration(
                    color: dangerColor.withOpacity(0.12),
                    border: Border.all(
                        color: dangerColor.withOpacity(0.4)),
                    borderRadius: BorderRadius.circular(11),
                  ),
                  child: Text(_bridgeError!,
                      style: TextStyle(
                          fontSize: 13,
                          color: _theme == 'light'
                              ? const Color(0xFFBE123C)
                              : const Color(0xFFFB7185))),
                ),
              if (_bridgeError != null) const SizedBox(height: 12),
              if (_loginError != null)
                Container(
                  width: double.infinity,
                  padding: const EdgeInsets.symmetric(
                      horizontal: 12, vertical: 9),
                  decoration: BoxDecoration(
                    color: dangerColor.withOpacity(0.12),
                    border: Border.all(
                        color: dangerColor.withOpacity(0.4)),
                    borderRadius: BorderRadius.circular(11),
                  ),
                  child: Text(_loginError!,
                      style: TextStyle(
                          fontSize: 13,
                          color: _theme == 'light'
                              ? const Color(0xFFBE123C)
                              : const Color(0xFFFB7185))),
                ),
              if (_loginError != null) const SizedBox(height: 12),
              // "Sign in to Tuya" (egui: the Log in button's hover
              // text) heads the form.
              Align(
                alignment: Alignment.centerLeft,
                child: Text('Sign in to Tuya',
                    style: TextStyle(
                        fontSize: 15,
                        fontWeight: FontWeight.bold,
                        color: pal.text)),
              ),
              const SizedBox(height: 12),
              _fieldLabel('Smart Life / Tuya username'),
              TextField(
                controller: _liUser,
                style: TextStyle(color: pal.text),
                decoration: _inputDecoration('email or phone'),
              ),
              const SizedBox(height: 10),
              _fieldLabel('Password'),
              TextField(
                controller: _liPass,
                obscureText: true,
                style: TextStyle(color: pal.text),
                decoration:
                    _inputDecoration('Your Smart Life password'),
                onSubmitted: (_) => _submitLogin(),
              ),
              const SizedBox(height: 10),
              _fieldLabel('Region (where your account was registered)'),
              _dropdown<String>(
                value: _liRegion,
                items: regions,
                onChanged: (v) {
                  if (v != null) setState(() => _liRegion = v);
                },
              ),
              const SizedBox(height: 10),
              _fieldLabel('App platform'),
              _dropdown<String>(
                value: _liPlatform,
                items: platforms,
                onChanged: (v) {
                  if (v != null) setState(() => _liPlatform = v);
                },
              ),
              const SizedBox(height: 8),
              CheckboxListTile(
                value: _liKeep,
                onChanged: (v) =>
                    setState(() => _liKeep = v ?? true),
                title: Text('Keep me logged in on this device',
                    style: TextStyle(color: pal.text, fontSize: 14)),
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                activeColor: _accent,
              ),
              const SizedBox(height: 12),
              SizedBox(
                width: double.infinity,
                child: ElevatedButton(
                  onPressed: _loginBusy ? null : _submitLogin,
                  style: ElevatedButton.styleFrom(
                    backgroundColor: _accent,
                    foregroundColor: Colors.white,
                    padding: const EdgeInsets.symmetric(vertical: 14),
                    shape: RoundedRectangleBorder(
                        borderRadius:
                            BorderRadius.circular(buttonRadius)),
                  ),
                  child: const Text('Log in', style: TextStyle(fontSize: 15)),
                ),
              ),
              if (_loginBusy) ...[
                const SizedBox(height: 8),
                const CircularProgressIndicator(),
              ],
              const SizedBox(height: 14),
              const Divider(),
              ExpansionTile(
                title: Text('Connection / proxy settings',
                    style: TextStyle(color: pal.text, fontSize: 14)),
                children: [
                  const SizedBox(height: 6),
                  _proxyFields(),
                  const SizedBox(height: 6),
                  Text(
                    "Most regions connect directly. A proxy is only needed where Tuya's servers don't accept direct browser/app calls (China region, some networks). See Help in Settings.",
                    style: TextStyle(fontSize: 12, color: pal.dim),
                  ),
                ],
              ),
              const SizedBox(height: 10),
              Text(
                'Your credentials are only sent to Tuya\'s own servers and are stored only in this app\'s config on this device (and only if you tick “Keep me logged in”). Nothing is sent anywhere else.',
                style: TextStyle(fontSize: 12, color: pal.dim),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _proxyFields() {
    final preset = _prefs['proxy_preset'] as String? ?? 'auto';
    final custom = _prefs['proxy_custom'] as String? ?? '';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _fieldLabel('CORS proxy'),
        _dropdown<String>(
          value: preset,
          items: proxyPresets,
          onChanged: (v) {
            if (v != null) _mutPrefs('proxy_preset', v);
          },
        ),
        if (preset == 'custom') ...[
          const SizedBox(height: 6),
          _fieldLabel('Custom proxy prefix'),
          TextField(
            controller:
                TextEditingController(text: custom),
            style: TextStyle(color: _pal.text),
            decoration: _inputDecoration(
                'https://my-proxy.example/ or …?url={url}'),
            onSubmitted: (v) => _mutPrefs('proxy_custom', v),
          ),
        ],
      ],
    );
  }

  // ================= dashboard header =================

  Widget _dashboardView() {
    final pal = _pal;
    return Column(
      children: [
        Container(
          color: pal.card,
          padding:
              const EdgeInsets.symmetric(horizontal: 18, vertical: 12),
          child: Row(
            children: [
              const Text('🏠', style: TextStyle(fontSize: 22)),
              const SizedBox(width: 6),
              Text('SmartLife & Tuya Desktop',
                  style: TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.bold,
                      color: pal.text)),
              const Spacer(),
              if (_lastUpdated.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(right: 10),
                  child: Text(_lastUpdated,
                      style: TextStyle(fontSize: 12, color: pal.dim)),
                ),
              SizedBox(
                width: 180,
                child: TextField(
                  controller: _query,
                  focusNode: _searchFocus,
                  style: TextStyle(color: pal.text, fontSize: 14),
                  decoration:
                      _inputDecoration('🔍 Search devices…'),
                ),
              ),
              const SizedBox(width: 8),
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Checkbox(
                    value: _autorefresh,
                    activeColor: _accent,
                    onChanged: (v) =>
                        _mutPrefs('autorefresh', v ?? true),
                  ),
                  Text('Autorefresh',
                      style: TextStyle(color: pal.text, fontSize: 14)),
                ],
              ),
              const SizedBox(width: 4),
              _dropdown<int>(
                value: _interval,
                items: intervals,
                width: 74,
                onChanged: (v) {
                  if (v != null) _mutPrefs('interval', v);
                },
              ),
              const SizedBox(width: 8),
              OutlinedButton(
                onPressed: _refreshBusy ? null : () => _refresh(manual: true),
                child: const Text('↻ Refresh'),
              ),
              const SizedBox(width: 4),
              OutlinedButton(
                onPressed: () => _setEditing(!_editing),
                child: Text(_editing ? '✔ Done editing' : '✎ Edit'),
              ),
              const SizedBox(width: 4),
              OutlinedButton(
                onPressed: () =>
                    setState(() => _view = AppView.settings),
                child: const Text('⚙ Settings'),
              ),
              const SizedBox(width: 4),
              OutlinedButton(
                onPressed: _logout,
                child: const Text('Log out'),
              ),
              const SizedBox(width: 4),
              _monitorMenu(),
            ],
          ),
        ),
        if (_editing)
          Container(
            color: _accent.withOpacity(0.12),
            padding:
                const EdgeInsets.symmetric(horizontal: 18, vertical: 9),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    '✎ Edit mode — drag cards to reorder within a section, rename devices, assign groups, hide devices. Use 📌 on any card to pin a shortcut to the top.',
                    style: TextStyle(fontSize: 13, color: pal.dim),
                  ),
                ),
                OutlinedButton(
                  onPressed: () => _openCameraForm(null),
                  child: const Text('+ Add camera'),
                ),
                const SizedBox(width: 6),
                OutlinedButton(
                  onPressed: _openNewGroupDialog,
                  child: const Text('+ New group'),
                ),
                const SizedBox(width: 6),
                ElevatedButton(
                  onPressed: () => _setEditing(false),
                  style: ElevatedButton.styleFrom(
                    backgroundColor: _accent,
                    foregroundColor: Colors.white,
                  ),
                  child: const Text('Done'),
                ),
              ],
            ),
          ),
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.only(top: 6, bottom: 24),
            child: Center(
              child: Container(
                constraints:
                    const BoxConstraints(maxWidth: contentMaxWidth),
                padding: const EdgeInsets.symmetric(horizontal: 18),
                child: _dashboardBody(),
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _monitorMenu() {
    // TODO(GAP) snap-to-monitor: the dialog lists live displays
    // from tuya_monitors, but moving the Flutter window onto the
    // chosen monitor needs the window_manager plugin, which this
    // app does not depend on. Selecting a display explains that
    // instead of pretending the window moved.
    return OutlinedButton(
      onPressed: () async {
        final res = await RustBridge.monitors();
        if (!mounted) return;
        final mons = res['monitors'];
        final list = mons is List ? mons : [];
        final chosen = await showDialog<String>(
          context: context,
          builder: (context) => AlertDialog(
            backgroundColor: _pal.card,
            title: Text('🖥 Monitor',
                style: TextStyle(color: _pal.text)),
            content: list.isEmpty
                ? const Text('No displays found')
                : Column(
                    mainAxisSize: MainAxisSize.min,
                    children: list.map<Widget>((m) {
                      final rect = m['rect_px'];
                      final w = rect is List && rect.length > 2
                          ? rect[2].toString()
                          : '?';
                      final h = rect is List && rect.length > 3
                          ? rect[3].toString()
                          : '?';
                      return ListTile(
                        title: Text(
                            '${m['name']} — $w × $h${m['primary'] == true ? ' · Primary' : ''}',
                            style:
                                TextStyle(color: _pal.text)),
                        onTap: () => Navigator.of(context)
                            .pop(m['device']?.toString() ?? ''),
                      );
                    }).toList(),
                  ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(),
                child: const Text('Cancel'),
              ),
            ],
          ),
        );
        if (chosen != null && chosen.isNotEmpty) {
          _toast(
              'Moving the window to another monitor needs the window_manager plugin (GAP) — the window stays where it is.',
              isError: true);
        }
      },
      child: const Text('🖥 Monitor'),
    );
  }

  Widget _dashboardBody() {
    final pal = _pal;
    final sections = _buildSections();
    final children = <Widget>[];
    if (_updateInfo != null &&
        _versionNewer(_updateInfo!['version']?.toString() ?? '',
            appVersion) &&
        ((_config['update'] as Map<String, dynamic>?)?['dismissed'] !=
            _updateInfo!['version'])) {
      children.add(_updateBanner());
      children.add(const SizedBox(height: 12));
    }
    if (sections.isEmpty) {
      if (_queryText.trim().isNotEmpty) {
        children.add(Center(
          child: Padding(
            padding: const EdgeInsets.only(top: 32),
            child: Text('Nothing matches your search.',
                style: TextStyle(fontSize: 14, color: pal.dim)),
          ),
        ));
      } else {
        children.add(Center(
          child: Padding(
            padding: const EdgeInsets.only(top: 32),
            child: Text(
              'No devices to show. Press ↻ Refresh, or unhide devices in Settings.',
              style: TextStyle(fontSize: 14, color: pal.dim),
            ),
          ),
        ));
      }
    } else {
      for (final sec in sections) {
        children.add(_sectionView(sec));
        children.add(const SizedBox(height: 10));
      }
    }
    return Column(
        crossAxisAlignment: CrossAxisAlignment.start, children: children);
  }

  bool _versionNewer(String latest, String current) {
    List<int> parse(String s) {
      return s
          .trim()
          .replaceFirst(RegExp(r'^v'), '')
          .split('.')
          .map((p) =>
              int.tryParse(p.replaceAll(RegExp(r'[^0-9]'), '')) ?? 0)
          .toList();
    }

    final a = parse(latest);
    final b = parse(current);
    for (var i = 0; i < (a.length > b.length ? a.length : b.length); i++) {
      final x = i < a.length ? a[i] : 0;
      final y = i < b.length ? b[i] : 0;
      if (x != y) return x > y;
    }
    return false;
  }

  Widget _updateBanner() {
    final pal = _pal;
    final version = _updateInfo!['version']?.toString() ?? '';
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      decoration: BoxDecoration(
        color: _accent.withOpacity(0.12),
        border: Border.all(color: _accent),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Row(
        children: [
          Expanded(
            child: _updateDl != null
                ? Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      LinearProgressIndicator(
                        value: _updateDl![1] > 0
                            ? _updateDl![0] / _updateDl![1]
                            : 0,
                        color: _accent,
                      ),
                      const SizedBox(height: 6),
                      Text(
                        'Downloading the installer… ${(_updateDl![1] > 0 ? _updateDl![0] / _updateDl![1] * 100 : 0).round()}%',
                        style: TextStyle(color: pal.text, fontSize: 13.5),
                      ),
                    ],
                  )
                : Text('Version $version is available.',
                    style: TextStyle(
                        color: pal.text,
                        fontSize: 13.5,
                        fontWeight: FontWeight.bold)),
          ),
          const SizedBox(width: 12),
          ElevatedButton(
            onPressed: _updateChecking ? null : _downloadUpdate,
            style: ElevatedButton.styleFrom(
              backgroundColor: _accent,
              foregroundColor: Colors.white,
            ),
            child: const Text('Download & install'),
          ),
          const SizedBox(width: 6),
          OutlinedButton(
            onPressed: () => _openUrl(releasesPage),
            child: const Text('Open releases page'),
          ),
          const SizedBox(width: 6),
          TextButton(
            onPressed: () {
              (_config['update'] as Map<String, dynamic>)['dismissed'] =
                  version;
              _saveConfig();
              setState(() {});
            },
            child: const Text('Dismiss'),
          ),
        ],
      ),
    );
  }

  // ================= sections =================

  Widget _sectionView(_Section sec) {
    final pal = _pal;
    final collapsed = _collapsed[sec.key] ?? false;
    final sections = _buildSections();
    final secIdx = sections.indexWhere((s) => s.key == sec.key);
    final secTotal = sections.length;
    final radius = _compact ? cardRadiusCompact : cardRadius;
    return DragTarget<String>(
      onWillAcceptWithDetails: (d) => d.data != null,
      onAcceptWithDetails: (d) => _moveItem(d.data, sec, null),
      builder: (context, candidate, rejected) {
        return Container(
          decoration: candidate.isNotEmpty
              ? BoxDecoration(
                  border: Border.all(color: _accent, width: 2),
                  borderRadius: BorderRadius.circular(radius),
                )
              : null,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  TextButton(
                    onPressed: () {
                      final m = _collapsed;
                      m[sec.key] = !collapsed;
                      _mutLayout('collapsed', m);
                    },
                    child: Text(collapsed ? '▸' : '▾',
                        style: TextStyle(color: pal.text)),
                  ),
                  if (_editing && sec.kind == 'group')
                    SizedBox(
                      width: 170,
                      child: TextField(
                        controller: _groupControllers.putIfAbsent(
                            sec.groupId!, () => TextEditingController()),
                        style: TextStyle(color: pal.text, fontSize: 16),
                        decoration: const InputDecoration(
                            border: InputBorder.none,
                            isDense: true),
                        onSubmitted: (v) => _renameGroup(sec, v),
                      ),
                    )
                  else
                    Text(sec.title,
                        style: TextStyle(
                            fontSize: 16.5,
                            fontWeight: FontWeight.bold,
                            color: pal.text)),
                  const SizedBox(width: 8),
                  Container(
                    padding: const EdgeInsets.symmetric(
                        horizontal: 8, vertical: 1),
                    decoration: BoxDecoration(
                      color: pal.card,
                      border: Border.all(color: pal.border),
                      borderRadius: BorderRadius.circular(10),
                    ),
                    child: Text('${sec.ids.length}',
                        style:
                            TextStyle(fontSize: 12, color: pal.dim)),
                  ),
                  const Spacer(),
                  if (_editing && sec.kind == 'group')
                    TextButton(
                      onPressed: () => _deleteGroup(sec),
                      child: const Text('Delete',
                          style: TextStyle(color: dangerColor)),
                    ),
                  if (_editing &&
                      _sectionOrder.containsKey(sec.orderKey))
                    TextButton(
                      onPressed: () {
                        final all = _sectionOrder;
                        all.remove(sec.orderKey);
                        _mutLayout('section_order', all);
                        _toast('“${sec.title}” is back to name order');
                      },
                      child: const Text('↺ Reset to name order'),
                    ),
                  if (_editing) ...[
                    IconButton(
                      tooltip: 'Move section up',
                      onPressed: secIdx > 0
                          ? () => _moveSection(sec.orderKey, -1)
                          : null,
                      icon: const Text('↑'),
                    ),
                    IconButton(
                      tooltip: 'Move section down',
                      onPressed: secIdx + 1 < secTotal
                          ? () => _moveSection(sec.orderKey, 1)
                          : null,
                      icon: const Text('↓'),
                    ),
                  ],
                ],
              ),
              const SizedBox(height: 8),
              _groupControls(sec),
              if (collapsed)
                const SizedBox(height: 10)
              else if (sec.ids.isEmpty && _editing)
                Padding(
                  padding: const EdgeInsets.only(bottom: 14),
                  child: Text('No items',
                      style: TextStyle(color: pal.dim)),
                )
              else
                LayoutBuilder(
                  builder: (context, constraints) {
                    final compact = _compact;
                    final minW =
                        compact ? gridMinCompact : gridMinComfortable;
                    final gap =
                        compact ? gridGapCompact : gridGapComfortable;
                    final avail = constraints.maxWidth;
                    var cols =
                        ((avail + gap) / (minW + gap)).floor();
                    if (cols < 1) cols = 1;
                    // Greedy row packing, egui pack_rows: spans are
                    // camera sizes (1 = Normal, 2 = Large, cols =
                    // Wide); devices/scenes span 1.
                    final rows = <List<(_Entry, int)>>[];
                    var cur = <(_Entry, int)>[];
                    var used = 0;
                    for (final id in sec.ids) {
                      final e = _entryById(id);
                      if (e == null) continue;
                      var span = 1;
                      if (e.isCamera) {
                        final size =
                            _camSizes[e.id] ?? 'normal';
                        if (size == 'large') span = 2;
                        if (size == 'wide') span = cols;
                      }
                      if (span > cols) span = cols;
                      if (cur.isNotEmpty && used + span > cols) {
                        rows.add(cur);
                        cur = [];
                        used = 0;
                      }
                      cur.add((e, span));
                      used += span;
                    }
                    if (cur.isNotEmpty) rows.add(cur);
                    return Column(
                      children: rows.map((row) {
                        return Padding(
                          padding: EdgeInsets.only(bottom: gap),
                          child: Row(
                            crossAxisAlignment:
                                CrossAxisAlignment.start,
                            children: [
                              for (var i = 0; i < row.length; i++) ...[
                                if (i > 0) SizedBox(width: gap),
                                Expanded(
                                  flex: row[i].$2,
                                  child: _draggableCard(
                                      row[i].$1, sec),
                                ),
                              ],
                            ],
                          ),
                        );
                      }).toList(),
                    );
                  },
                ),
              const SizedBox(height: 14),
            ],
          ),
        );
      },
    );
  }

  void _renameGroup(_Section sec, String value) {
    final trimmed = value.trim();
    if (trimmed.isEmpty || trimmed == sec.title) return;
    for (final g in _groups) {
      if (g['id'].toString() == sec.groupId) {
        g['name'] = trimmed;
      }
    }
    _config['layout']['groups'] = _groups;
    _saveConfig();
    setState(() {});
  }

  void _deleteGroup(_Section sec) {
    final gid = sec.groupId;
    if (gid == null) return;
    _groups.removeWhere((g) => g['id'].toString() == gid);
    _config['layout']['groups'] = _groups;
    final assign = _assign..removeWhere((_, v) => v == gid);
    _layout['assign'] = assign;
    final order = _sectionOrder..remove('group:$gid');
    _layout['section_order'] = order;
    _saveConfig();
    setState(() {});
    _toast('Group deleted — its devices are Ungrouped now');
  }

  /// Card wrapper: LongPressDraggable + DragTarget (see the GAP
  /// note at the top of this file about the 8px threshold).
  Widget _draggableCard(_Entry entry, _Section sec) {
    final card = entry.isCamera
        ? _cameraCard(entry, sec)
        : _deviceCard(entry, sec);
    if (!_editing) return card;
    return DragTarget<String>(
      onWillAcceptWithDetails: (d) =>
          d.data != null && d.data != entry.id,
      onAcceptWithDetails: (d) =>
          _moveItem(d.data, sec, entry.id),
      builder: (context, candidate, rejected) {
        return LongPressDraggable<String>(
          data: entry.id,
          feedback: Material(
            elevation: 8,
            borderRadius: BorderRadius.circular(
                _compact ? cardRadiusCompact : cardRadius),
            // IgnorePointer: the feedback must never steal focus
            // from the alias/group TextFields on the real card.
            child: SizedBox(
                width: 240, child: IgnorePointer(child: card)),
          ),
          childWhenDragging: Opacity(opacity: 0.4, child: card),
          child: card,
        );
      },
    );
  }

  // ================= group controls =================

  Widget _groupControls(_Section sec) {
    if (sec.kind != 'lighting' && sec.kind != 'group') {
      return const SizedBox.shrink();
    }
    // Group controls ignore the search filter (egui rebuilds the
    // full sections with an empty query for this).
    final savedQuery = _queryText;
    _queryText = '';
    final full = _buildSections();
    _queryText = savedQuery;
    _sectionsCache = null;
    final fsec = full.firstWhere((s) => s.key == sec.key,
        orElse: () => sec);
    final members = <Map<String, dynamic>>[];
    for (final id in fsec.ids) {
      final d = _deviceById(id);
      if (d != null && d['is_scene'] != true) members.add(d);
    }
    final lights = members.where((d) {
      final kind = d['kind'] as String? ?? '';
      return kind == 'light';
    }).toList();
    if (lights.isEmpty) return const SizedBox.shrink();
    final pal = _pal;
    final anyOn = members.any((d) => d['state'] == true);
    final bVals = members
        .where((d) => d['has_brightness'] == true)
        .map((d) => d['brightness_pct'])
        .whereType<num>()
        .toList();
    final tVals = members
        .map((d) => d['temp_slider'])
        .whereType<num>()
        .toList();
    final hasColor = members.any((d) => d['has_color'] == true);
    final avgB = bVals.isEmpty
        ? null
        : brightnessDeviceToUi(
            (bVals.fold<int>(0, (a, b) => a + b.toInt()) / bVals.length)
                .round());
    final avgT = tVals.isEmpty
        ? null
        : (tVals.fold<int>(0, (a, b) => a + b.toInt()) / tVals.length)
            .round();
    final radius = _compact ? cardRadiusCompact : cardRadius;
    return Container(
      margin: const EdgeInsets.only(bottom: 12),
      padding:
          const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      decoration: BoxDecoration(
        color: pal.card,
        border: Border.all(color: pal.border),
        borderRadius: BorderRadius.circular(radius),
      ),
      child: Wrap(
        spacing: 24,
        runSpacing: 12,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text('All',
                  style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.bold,
                      color: pal.dim)),
              const SizedBox(width: 8),
              _toggleSwitch(
                  anyOn, true, (_) => _groupBroadcast(sec, 'toggle', 0, null)),
            ],
          ),
          if (avgB != null)
            _groupSliderRow('Brightness', avgB.toDouble(), 1, 100,
                (v) => '$v%', (v) {
              _groupBroadcast(sec, 'brightness', v.toInt(), null);
            }),
          if (avgT != null)
            _groupSliderRow(
                'Colour temperature',
                tempStopIndex(tempSliderToKelvin(avgT)).toDouble(),
                0,
                tempStopsK.length - 1,
                (v) => '${tempStopKelvin(v)}K', (v) {
              _groupBroadcast(
                  sec, 'temp', tempSliderForStop(v.toInt()), null);
            }),
          if (hasColor)
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final (_, hex) in swatches)
                  _swatch(hex, '', () {
                    _groupBroadcast(sec, 'color', 0, hex);
                  }),
              ],
            ),
        ],
      ),
    );
  }

  Widget _groupSliderRow(String label, double value, double min,
      double max, String Function(int) fmt, ValueChanged<double> onEnd) {
    return SizedBox(
      width: 260,
      child: _LiveSlider(
        label: label,
        value: value.clamp(min, max),
        min: min,
        max: max,
        divisions: (max - min).toInt(),
        format: fmt,
        accent: _accent,
        dim: _pal.dim,
        text: _pal.text,
        onStart: () => _sliderActive = true,
        onEnd: (v) {
          _sliderActive = false;
          onEnd(v.toDouble());
        },
      ),
    );
  }

  /// Group broadcast: the command goes to each member in turn
  /// (egui group_plan), offline members skipped, colour-only
  /// strips excluded from brightness (they report no brightness).
  Future<void> _groupBroadcast(
      _Section sec, String kind, int num, String? hex) async {
    final savedQuery = _queryText;
    _queryText = '';
    final full = _buildSections();
    _queryText = savedQuery;
    _sectionsCache = null;
    final fsec =
        full.firstWhere((s) => s.key == sec.key, orElse: () => sec);
    final members = <Map<String, dynamic>>[];
    for (final id in fsec.ids) {
      final d = _deviceById(id);
      if (d != null && d['is_scene'] != true && d['online'] == true) {
        members.add(d);
      }
    }
    final anyOn = members.any((d) => d['state'] == true);
    for (final d in members) {
      final id = d['id'].toString();
      if (kind == 'toggle') {
        await _control(id, 'turnOnOff', anyOn ? 0 : 1);
        setState(() => d['state'] = !anyOn);
      } else if (kind == 'brightness') {
        if (d['has_brightness'] != true) continue;
        await _control(id, 'brightnessSet', brightnessUiToDevice(num));
      } else if (kind == 'temp') {
        if (d['has_temp'] != true) continue;
        await _control(
            id, 'colorTemperatureSet', num.clamp(1000, 10000));
      } else if (kind == 'color' && hex != null) {
        if (d['has_color'] != true) continue;
        final (h, s, _) = hexToHsv(hex);
        final b = (d['brightness_pct'] is num)
            ? (d['brightness_pct'] as num).toInt()
            : 100;
        await _control(id, 'colorSet',
            {'hue': h, 'saturation': s, 'brightness': b});
      }
    }
    setState(() {});
  }

  // ================= shared card widgets =================

  /// 48×27 toggle switch, egui toggle_switch geometry: rounded
  /// 13.5 track, accent when on, track colour when off.
  Widget _toggleSwitch(
      bool on, bool enabled, ValueChanged<bool> onToggle) {
    return GestureDetector(
      onTap: enabled ? () => onToggle(!on) : null,
      child: Container(
        width: 48,
        height: 27,
        decoration: BoxDecoration(
          color: on ? _accent : _pal.track,
          borderRadius: BorderRadius.circular(13.5),
        ),
        child: Align(
          alignment: on ? Alignment.centerRight : Alignment.centerLeft,
          child: Container(
            width: 21,
            height: 21,
            margin: const EdgeInsets.symmetric(horizontal: 3),
            decoration: const BoxDecoration(
              color: Colors.white,
              shape: BoxShape.circle,
            ),
          ),
        ),
      ),
    );
  }

  Widget _swatch(String hex, String name, VoidCallback onTap) {
    final c = hexToColor(hex);
    return GestureDetector(
      onTap: onTap,
      child: Container(
        width: 26,
        height: 26,
        margin: const EdgeInsets.only(right: 6, bottom: 4),
        decoration: BoxDecoration(
          color: c,
          shape: BoxShape.circle,
          border: Border.all(color: _pal.border),
        ),
      ),
    );
  }

  Widget _cardFrame(Widget child, {bool onState = false}) {
    final radius = _compact ? cardRadiusCompact : cardRadius;
    return Container(
      decoration: BoxDecoration(
        color: _pal.card,
        border: Border.all(
            color: onState ? _accent : _pal.border,
            width: onState ? 1.5 : 1),
        borderRadius: BorderRadius.circular(radius),
      ),
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      child: child,
    );
  }

  // ================= device cards =================

  Widget _deviceCard(_Entry entry, _Section sec) {
    final dev = entry.raw;
    final pal = _pal;
    final id = entry.id;
    final name = _displayName(id, entry.name);
    final online = dev['online'] == true;
    final isScene = entry.isScene;
    final pinned = _pins.contains(id);
    final onState = dev['state'] == true && online;
    final iconKind = dev['icon_kind'] as String? ??
        (isScene ? 'scene' : (entry.light ? 'light' : 'switch'));
    return _cardFrame(
      Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              if (_editing)
                Padding(
                  padding: const EdgeInsets.only(right: 6),
                  child: Text('⠿',
                      style: TextStyle(color: pal.dim, fontSize: 16)),
                ),
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  color: pal.bg,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: Center(
                    child: Text(typeIcon(iconKind),
                        style: const TextStyle(fontSize: 21))),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: Text(name,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 15,
                                  fontWeight: FontWeight.bold,
                                  color: pal.text)),
                        ),
                        if (!online)
                          Container(
                            padding: const EdgeInsets.symmetric(
                                horizontal: 6, vertical: 2),
                            decoration: BoxDecoration(
                              color: dangerColor,
                              borderRadius: BorderRadius.circular(6),
                            ),
                            child: const Text('OFFLINE',
                                style: TextStyle(
                                    fontSize: 10.5,
                                    color: Colors.white,
                                    fontWeight: FontWeight.bold)),
                          ),
                      ],
                    ),
                    const SizedBox(height: 2),
                    Row(
                      children: [
                        Text('●',
                            style: TextStyle(
                                fontSize: 9,
                                color: !online
                                    ? pal.faint
                                    : (onState
                                        ? okGreenColor
                                        : pal.faint))),
                        const SizedBox(width: 4),
                        Expanded(
                          child: Text(statusText(dev),
                              style: TextStyle(
                                  fontSize: 12.5, color: pal.dim)),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              // Pin lands LEFT of the toggle (egui header_pin_toggle
              // adds right-to-left: toggle first, pin second).
              IconButton(
                tooltip: pinned
                    ? 'Unpin (remove shortcut from the top)'
                    : 'Pin',
                onPressed: () => _togglePin(id),
                icon: Text('📌',
                    style: TextStyle(
                        color: pinned ? pal.text : pal.faint)),
              ),
              if (!isScene)
                _toggleSwitch(onState, online, (_) {
                  if (online) _toggleDevice(dev);
                }),
              IconButton(
                tooltip: 'Hide',
                onPressed: () => _hideItem(id),
                icon: Text('✕',
                    style: TextStyle(color: pal.dim, fontSize: 13)),
              ),
            ],
          ),
          if (online && !isScene) ...[
            if (dev['has_brightness'] == true)
              _LiveSlider(
                label: 'Brightness',
                value: brightnessDeviceToUi(
                        (dev['brightness_pct'] is num)
                            ? (dev['brightness_pct'] as num).toInt()
                            : 100)
                    .toDouble(),
                min: 1,
                max: 100,
                divisions: 99,
                format: (v) => '$v%',
                accent: _accent,
                dim: pal.dim,
                text: pal.text,
                onStart: () => _sliderActive = true,
                onEnd: (v) async {
                  _sliderActive = false;
                  setState(() =>
                      dev['brightness_pct'] = brightnessUiToDevice(v));
                  await _control(id, 'brightnessSet',
                      brightnessUiToDevice(v));
                },
              ),
            if (dev['has_temp'] == true)
              _LiveSlider(
                label: 'Colour temperature',
                value: tempStopIndex(tempSliderToKelvin(
                        (dev['temp_slider'] is num)
                            ? (dev['temp_slider'] as num).toInt()
                            : 5500))
                    .toDouble(),
                min: 0,
                max: tempStopsK.length - 1,
                divisions: tempStopsK.length - 1,
                format: (v) => '${tempStopKelvin(v)}K',
                accent: _accent,
                dim: pal.dim,
                text: pal.text,
                onStart: () => _sliderActive = true,
                onEnd: (v) async {
                  _sliderActive = false;
                  final slider = tempSliderForStop(v);
                  setState(() => dev['temp_slider'] = slider);
                  await _control(id, 'colorTemperatureSet', slider);
                },
              ),
            if (dev['has_color'] == true) ...[
              const SizedBox(height: 4),
              Wrap(
                children: [
                  for (final (swName, hex) in swatches)
                    _swatch(hex, swName, () => _doColor(id, hex)),
                  TextButton(
                    onPressed: () {
                      setState(() {
                        if (_openColorFor == id) {
                          _openColorFor = null;
                        } else {
                          _openColorFor = id;
                          final hue = dev['hue'];
                          final sat = dev['saturation'];
                          _colorPick[id] = (hue is num)
                              ? hexToColor(hsvToHex(
                                  hue.toDouble(),
                                  (sat is num ? sat.toDouble() : 0),
                                  1.0))
                              : Colors.white;
                        }
                      });
                    },
                    child: const Text('🎨',
                        style: TextStyle(fontSize: 16)),
                  ),
                ],
              ),
              if (_openColorFor == id) ...[
                const SizedBox(height: 8),
                const Divider(),
                const SizedBox(height: 6),
                _ColorPickerRow(
                  initial: _colorPick[id] ?? Colors.white,
                  accent: _accent,
                  dim: pal.dim,
                  text: pal.text,
                  onApply: (c) {
                    _doColor(id, colorToHex(c));
                    setState(() => _openColorFor = null);
                  },
                ),
              ],
            ],
          ],
          if (isScene) ...[
            const SizedBox(height: 10),
            SizedBox(
              width: double.infinity,
              child: ElevatedButton(
                onPressed: online ? () => _startScene(id) : null,
                style: ElevatedButton.styleFrom(
                  backgroundColor: _accent,
                  foregroundColor: Colors.white,
                ),
                child: const Text('▶ Start scene',
                    style: TextStyle(fontSize: 14)),
              ),
            ),
          ],
          if (_editing) ...[
            const SizedBox(height: 10),
            const Divider(),
            const SizedBox(height: 8),
            Focus(
              onFocusChange: (hasFocus) {
                if (!hasFocus) {
                  final v =
                      _aliasControllers[id]?.text.trim() ?? '';
                  final alias = _alias;
                  if (v.isEmpty) {
                    alias.remove(id);
                  } else {
                    alias[id] = v;
                  }
                  _layout['alias'] = alias;
                  _saveConfig();
                  setState(() {});
                }
              },
              child: TextField(
                controller: _aliasControllers.putIfAbsent(
                    id, () => TextEditingController(text: _alias[id] ?? '')),
                style: TextStyle(color: pal.text, fontSize: 14),
                decoration:
                    _inputDecoration('Display name (alias)'),
                onSubmitted: (_) =>
                    FocusScope.of(context).unfocus(),
              ),
            ),
            if (!isScene) ...[
              const SizedBox(height: 6),
              Row(
                children: [
                  Expanded(
                    child: _dropdown<String>(
                      value: _assign[id],
                      items: [
                        (
                          null,
                          entry.light
                              ? 'Lighting (automatic)'
                              : 'Ungrouped'
                        ),
                        for (final g in _groups)
                          (g['id'].toString(),
                              g['name']?.toString() ?? ''),
                      ],
                      onChanged: (v) {
                        final assign = _assign;
                        if (v == null) {
                          assign.remove(id);
                        } else {
                          assign[id] = v;
                        }
                        _mutLayout('assign', assign);
                      },
                    ),
                  ),
                  const SizedBox(width: 6),
                  OutlinedButton(
                    onPressed: () => _togglePin(id),
                    child: Text(pinned ? '📌 Unpin' : '📌 Pin'),
                  ),
                ],
              ),
            ],
            const SizedBox(height: 6),
            Row(
              children: [
                OutlinedButton(
                  onPressed: () => _moveInSection(sec, id, -1),
                  child: const Text('↑ Move up'),
                ),
                const SizedBox(width: 6),
                OutlinedButton(
                  onPressed: () => _moveInSection(sec, id, 1),
                  child: const Text('↓ Move down'),
                ),
              ],
            ),
          ],
        ],
      ),
      onState: onState,
    );
  }

  // ================= camera helpers =================
  // egui file format: camera passwords are stored obfuscated
  // (XOR 0x5A + base64, exactly like the account password — this
  // is NOT encryption, it just avoids plain text at a glance).
  // Dart mirrors egui's obfuscate/deobfuscate so it can build the
  // exact RTSP address for Copy URL / Watch / external player.
  // CONTRACT for the Rust core: tuya_save_config writes the
  // config JSON verbatim (as egui's save_config did) — it must
  // NOT re-obfuscate values Dart already obfuscated.

  String _obfuscate(String plain) {
    final bytes =
        plain.codeUnits.map((c) => c ^ 0x5A).toList();
    return base64Encode(bytes);
  }

  String _deobfuscate(String s) {
    try {
      return String.fromCharCodes(
          base64Decode(s).map((b) => b ^ 0x5A));
    } catch (_) {
      return '';
    }
  }

  String _pctEncode(String s) {
    return Uri.encodeComponent(s);
  }

  String _cameraPath(Map<String, dynamic> cam) {
    final template = cam['template'] as String? ?? 'tapo1';
    if (template == 'custom') {
      final p = (cam['path'] as String? ?? '').trim();
      if (p.isEmpty) return '/';
      return p.startsWith('/') ? p : '/$p';
    }
    if (template == 'tapo2') return '/stream2';
    return '/stream1';
  }

  String _buildRtspUrl(Map<String, dynamic> cam) {
    final port = (cam['port'] is num && (cam['port'] as num) != 0)
        ? (cam['port'] as num).toInt()
        : 554;
    return 'rtsp://${_pctEncode(cam['user']?.toString() ?? '')}:'
        '${_pctEncode(_deobfuscate(cam['password']?.toString() ?? ''))}'
        '@${(cam['host']?.toString() ?? '').trim()}:$port'
        '${_cameraPath(cam)}';
  }

  /// Tile URL: Stream 2 (SD) whenever a Tapo template is set, else
  /// the camera's own path (egui tile_rtsp_url).
  String _tileRtspUrl(Map<String, dynamic> cam) {
    final template = cam['template'] as String? ?? 'tapo1';
    if (template == 'tapo1' || template == 'tapo2') {
      final copy = Map<String, dynamic>.from(cam)..['template'] = 'tapo2';
      return _buildRtspUrl(copy);
    }
    return _buildRtspUrl(cam);
  }

  String? _gatewayWatchUrl(Map<String, dynamic> cam) {
    var gw = (_prefs['gateway_url'] as String? ?? '').trim();
    if (gw.isEmpty) return null;
    while (gw.endsWith('/')) {
      gw = gw.substring(0, gw.length - 1);
    }
    final name = _pctEncode(cam['name']?.toString() ?? '');
    if (_prefs['gateway_type'] == 'mediamtx') {
      return '$gw/$name/index.m3u8';
    }
    return '$gw/stream.html?src=$name';
  }

  bool get _previewsOn => _prefs['previews'] != false;

  final Set<String> _tileStarted = {};

  Future<void> _refreshFfmpeg() async {
    try {
      final res = await RustBridge.findFfmpeg(
          _prefs['ffmpeg_path'] as String? ?? '');
      if (mounted) {
        setState(() {
          _ffmpegPath = res['path'] as String?;
          _ffmpegChecked = true;
        });
      }
    } catch (_) {
      if (mounted) {
        setState(() {
          _ffmpegPath = null;
          _ffmpegChecked = true;
        });
      }
    }
  }

  String get _ffmpegStatus {
    if (!_ffmpegChecked) return 'Checking for ffmpeg…';
    if (_ffmpegPath != null) return 'ffmpeg found: $_ffmpegPath';
    return 'ffmpeg not found — previews are off and ▶ Watch uses the external player.';
  }

  Future<void> _startTilePump(String itemId) async {
    if (_tileStarted.contains(itemId)) return;
    _tileStarted.add(itemId);
    try {
      await RustBridge.cameraStart(itemId, 'tile');
    } catch (_) {
      _tileStarted.remove(itemId);
    }
  }

  Future<void> _pollFrames() async {
    if (!mounted ||
        !_loggedIn ||
        _view != AppView.dashboard ||
        !_previewsOn ||
        _ffmpegPath == null ||
        _framePollBusy) {
      return;
    }
    _framePollBusy = true;
    try {
      for (final sec in _buildSections()) {
        for (final id in sec.ids) {
          final cam = _cameraByItemId(id);
          if (cam == null) continue;
          if (_pausedCams.contains(id) || _inPlayer.contains(id)) {
            continue;
          }
          await _startTilePump(id);
          try {
            final res =
                await RustBridge.cameraFrame(id, 'tile');
            if (res['ok'] != true) continue;
            final state = res['state']?.toString() ?? 'starting';
            final b64 = res['rgba_base64'] as String?;
            final w = res['width'];
            final h = res['height'];
            if (b64 != null && w is num && h is num) {
              final bytes = base64Decode(b64);
              final old = _frames[id];
              ui.decodeImageFromPixels(
                bytes,
                w.toInt(),
                h.toInt(),
                ui.PixelFormat.rgba8888,
                (img) {
                  old?.dispose();
                  if (mounted) {
                    setState(() {
                      _frames[id] = img;
                      _frameState[id] = 'live';
                    });
                  } else {
                    img.dispose();
                  }
                },
              );
            } else {
              if (mounted) {
                setState(() {
                  _frameState[id] = state;
                  if (res['reason'] is String) {
                    _frameReason[id] = res['reason'] as String;
                  }
                });
              }
            }
          } catch (_) {
            // A single failed poll never kills the loop or toasts.
          }
        }
      }
    } finally {
      _framePollBusy = false;
    }
  }

  Future<void> _retryTile(String itemId) async {
    _tileStarted.remove(itemId);
    _pausedCams.remove(itemId);
    setState(() => _frameState[itemId] = 'starting');
    await _startTilePump(itemId);
  }

  Future<void> _openUrl(String url) async {
    try {
      if (Platform.isWindows) {
        await Process.start('cmd', ['/C', 'start', '', url]);
      } else if (Platform.isMacOS) {
        await Process.start('open', [url]);
      } else {
        await Process.start('xdg-open', [url]);
      }
    } catch (e) {
      _toast('Could not open $url: $e', isError: true);
    }
  }

  Future<void> _openExternalPlayer(Map<String, dynamic> cam) async {
    final url = _buildRtspUrl(cam);
    final cmd = (_prefs['player_cmd'] as String? ?? 'vlc').trim();
    final parts =
        cmd.isEmpty ? ['vlc'] : cmd.split(RegExp(r'\s+'));
    try {
      await Process.start(parts.first, [...parts.skip(1), url]);
    } catch (e) {
      _toast('Could not start the external player: $e',
          isError: true);
    }
  }

  Future<void> _openWatch(_Entry entry) async {
    final cam = entry.raw;
    final gateway = _gatewayWatchUrl(cam);
    if (gateway != null) {
      // Gateway configured: Watch opens the gateway's watch page
      // in the browser (egui gateway_watch_url path).
      await _openUrl(gateway);
      return;
    }
    if (_ffmpegPath == null) {
      // No ffmpeg: fall back to the external player (VLC default).
      await _openExternalPlayer(cam);
      return;
    }
    setState(() => _inPlayer.add(entry.id));
    try {
      await RustBridge.cameraStart(entry.id, 'player');
    } catch (_) {}
    if (!mounted) return;
    await showDialog(
      context: context,
      builder: (_) => _WatchPlayer(
        entry: entry,
        cam: cam,
        rtspUrl: _buildRtspUrl(cam),
        gatewayUrl: null,
        onCopyUrl: () {
          Clipboard.setData(
              ClipboardData(text: _buildRtspUrl(cam)));
          _toast('Stream URL copied');
        },
        onExternalPlayer: () => _openExternalPlayer(cam),
        onTemplateChanged: (tpl) async {
          cam['template'] = tpl;
          _saveConfig();
          setState(() {});
          try {
            await RustBridge.cameraStop(entry.id, 'player');
            await RustBridge.cameraStart(entry.id, 'player');
          } catch (_) {}
        },
      ),
    );
    setState(() => _inPlayer.remove(entry.id));
    _tileStarted.remove(entry.id);
    try {
      await RustBridge.cameraStop(entry.id, 'player');
    } catch (_) {}
  }

  // ================= camera cards =================

  Widget _cameraCard(_Entry entry, _Section sec) {
    final cam = entry.raw;
    final pal = _pal;
    final id = entry.id;
    final name = _displayName(id, entry.name);
    final pinned = _pins.contains(id);
    final paused = _pausedCams.contains(id);
    final inPlayer = _inPlayer.contains(id);
    final state = _frameState[id] ?? 'starting';
    final backoff = state == 'backoff' && !paused;
    final frame = _frames[id];
    final template = cam['template'] as String? ?? 'tapo1';
    return _cardFrame(
      Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // ---- live tile: 16:9, black background, small radius ----
          GestureDetector(
            onTap: () => _openWatch(entry),
            child: AspectRatio(
              aspectRatio: 16 / 9,
              child: Container(
                decoration: BoxDecoration(
                  color: Colors.black,
                  borderRadius: BorderRadius.circular(8),
                ),
                child: Stack(
                  children: [
                    Positioned.fill(
                      child: ClipRRect(
                        borderRadius: BorderRadius.circular(8),
                        child: frame != null
                            ? RawImage(
                                image: frame, fit: BoxFit.contain)
                            : Center(
                                child: Text(
                                  paused
                                      ? 'Preview paused'
                                      : (backoff
                                          ? 'Feed retrying…'
                                          : (inPlayer
                                              ? 'In the big player — tile paused'
                                              : 'Starting feed…')),
                                  textAlign: TextAlign.center,
                                  style: const TextStyle(
                                      color: Colors.white70,
                                      fontSize: 12.5),
                                ),
                              ),
                      ),
                    ),
                    if (state == 'live' && !paused)
                      Positioned(
                        top: 8,
                        left: 8,
                        child: Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 6, vertical: 2),
                          decoration: BoxDecoration(
                            color: Colors.black54,
                            borderRadius: BorderRadius.circular(6),
                          ),
                          child: const Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Text('🔴',
                                  style: TextStyle(fontSize: 9)),
                              SizedBox(width: 4),
                              Text('LIVE',
                                  style: TextStyle(
                                      color: Colors.white,
                                      fontSize: 11,
                                      fontWeight: FontWeight.bold)),
                            ],
                          ),
                        ),
                      ),
                    // Play/pause chip, clear of the LIVE badge.
                    Positioned(
                      top: 6,
                      right: 6,
                      child: GestureDetector(
                        onTap: () async {
                          if (paused) {
                            setState(
                                () => _pausedCams.remove(id));
                            await _retryTile(id);
                          } else {
                            setState(() => _pausedCams.add(id));
                            _tileStarted.remove(id);
                            try {
                              await RustBridge.cameraStop(id, 'tile');
                            } catch (_) {}
                          }
                        },
                        child: Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 8, vertical: 4),
                          decoration: BoxDecoration(
                            color: Colors.black54,
                            borderRadius: BorderRadius.circular(12),
                          ),
                          child: Text(paused ? '▶' : '⏸',
                              style: const TextStyle(
                                  color: Colors.white, fontSize: 12)),
                        ),
                      ),
                    ),
                    if (backoff)
                      Positioned(
                        bottom: 8,
                        left: 8,
                        child: Column(
                          crossAxisAlignment:
                              CrossAxisAlignment.start,
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            if ((_frameReason[id] ?? '').isNotEmpty)
                              Text(_frameReason[id]!,
                                  style: const TextStyle(
                                      color: Colors.white70,
                                      fontSize: 11)),
                            TextButton(
                              onPressed: () => _retryTile(id),
                              child: const Text('↻ Retry now',
                                  style: TextStyle(
                                      color: Colors.white)),
                            ),
                          ],
                        ),
                      ),
                    if (state == 'error' && !paused)
                      Positioned(
                        bottom: 8,
                        left: 8,
                        child: TextButton(
                          onPressed: () => _retryTile(id),
                          child: const Text(
                              'Feed unavailable — ↻ Retry',
                              style:
                                  TextStyle(color: Colors.white)),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              if (_editing)
                Padding(
                  padding: const EdgeInsets.only(right: 6),
                  child: Text('⠿',
                      style: TextStyle(color: pal.dim, fontSize: 16)),
                ),
              Container(
                width: 40,
                height: 40,
                decoration: BoxDecoration(
                  color: pal.bg,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: const Center(
                    child: Text('📷', style: TextStyle(fontSize: 21))),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(name,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 15,
                            fontWeight: FontWeight.bold,
                            color: pal.text)),
                    const SizedBox(height: 2),
                    Text(cameraStatusText(cam),
                        style:
                            TextStyle(fontSize: 12.5, color: pal.dim)),
                  ],
                ),
              ),
              IconButton(
                tooltip: pinned
                    ? 'Unpin (remove shortcut from the top)'
                    : 'Pin',
                onPressed: () => _togglePin(id),
                icon: Text('📌',
                    style: TextStyle(
                        color: pinned ? pal.text : pal.faint)),
              ),
              IconButton(
                tooltip: 'Hide',
                onPressed: () => _hideItem(id),
                icon: Text('✕',
                    style: TextStyle(color: pal.dim, fontSize: 13)),
              ),
            ],
          ),
          if (template == 'tapo1' || template == 'tapo2') ...[
            const SizedBox(height: 8),
            Row(
              children: [
                for (final (tpl, label) in [
                  ('tapo1', 'Stream 1 · HD'),
                  ('tapo2', 'Stream 2 · SD'),
                ])
                  Padding(
                    padding: const EdgeInsets.only(right: 6),
                    child: tpl == template
                        ? ElevatedButton(
                            onPressed: () {},
                            style: ElevatedButton.styleFrom(
                              backgroundColor: _accent,
                              foregroundColor: Colors.white,
                              disabledForegroundColor: Colors.white,
                            ),
                            child: Text(label),
                          )
                        : OutlinedButton(
                            onPressed: () async {
                              cam['template'] = tpl;
                              _saveConfig();
                              setState(() {});
                              await _retryTile(id);
                            },
                            child: Text(label),
                          ),
                  ),
              ],
            ),
          ],
          const SizedBox(height: 8),
          Row(
            children: [
              ElevatedButton(
                onPressed: () => _openWatch(entry),
                style: ElevatedButton.styleFrom(
                  backgroundColor: _accent,
                  foregroundColor: Colors.white,
                ),
                child: const Text('▶ Watch',
                    style: TextStyle(fontSize: 14)),
              ),
              const SizedBox(width: 6),
              OutlinedButton(
                onPressed: () {
                  Clipboard.setData(
                      ClipboardData(text: _buildRtspUrl(cam)));
                  _toast('Stream URL copied');
                },
                child: const Text('Copy URL'),
              ),
              const SizedBox(width: 6),
              OutlinedButton(
                onPressed: () => _openExternalPlayer(cam),
                child: const Text('Open in external player'),
              ),
            ],
          ),
          if (_editing) ...[
            const SizedBox(height: 10),
            const Divider(),
            const SizedBox(height: 8),
            Focus(
              onFocusChange: (hasFocus) {
                if (!hasFocus) {
                  final v =
                      _aliasControllers[id]?.text.trim() ?? '';
                  final alias = _alias;
                  if (v.isEmpty) {
                    alias.remove(id);
                  } else {
                    alias[id] = v;
                  }
                  _layout['alias'] = alias;
                  _saveConfig();
                  setState(() {});
                }
              },
              child: TextField(
                controller: _aliasControllers.putIfAbsent(
                    id, () => TextEditingController(text: _alias[id] ?? '')),
                style: TextStyle(color: pal.text, fontSize: 14),
                decoration:
                    _inputDecoration('Display name (alias)'),
                onSubmitted: (_) =>
                    FocusScope.of(context).unfocus(),
              ),
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                Expanded(
                  child: _dropdown<String>(
                    value: _assign[id],
                    items: [
                      (null, 'Cameras (automatic)'),
                      for (final g in _groups)
                        (g['id'].toString(),
                            g['name']?.toString() ?? ''),
                    ],
                    onChanged: (v) {
                      final assign = _assign;
                      if (v == null) {
                        assign.remove(id);
                      } else {
                        assign[id] = v;
                      }
                      _mutLayout('assign', assign);
                    },
                  ),
                ),
                const SizedBox(width: 6),
                OutlinedButton(
                  onPressed: () => _togglePin(id),
                  child: Text(pinned ? '📌 Unpin' : '📌 Pin'),
                ),
              ],
            ),
            const SizedBox(height: 6),
            Row(
              children: [
                OutlinedButton(
                  onPressed: () => _moveInSection(sec, id, -1),
                  child: const Text('↑ Move up'),
                ),
                const SizedBox(width: 6),
                OutlinedButton(
                  onPressed: () => _moveInSection(sec, id, 1),
                  child: const Text('↓ Move down'),
                ),
                const SizedBox(width: 6),
                OutlinedButton(
                  onPressed: () => _openCameraForm(id),
                  child: const Text('✎ Edit'),
                ),
                const SizedBox(width: 6),
                OutlinedButton(
                  onPressed: () => _deleteCamera(id),
                  child: const Text('🗑 Delete',
                      style: TextStyle(color: dangerColor)),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Row(
              children: [
                Text('Size',
                    style: TextStyle(fontSize: 12.5, color: pal.dim)),
                const SizedBox(width: 8),
                for (final (sz, label) in [
                  ('normal', 'Normal'),
                  ('large', 'Large'),
                  ('wide', 'Wide'),
                ])
                  Padding(
                    padding: const EdgeInsets.only(right: 6),
                    child: (_camSizes[id] ?? 'normal') == sz
                        ? ElevatedButton(
                            onPressed: () {},
                            style: ElevatedButton.styleFrom(
                              backgroundColor: _accent,
                              foregroundColor: Colors.white,
                              disabledForegroundColor: Colors.white,
                            ),
                            child: Text(label),
                          )
                        : OutlinedButton(
                            onPressed: () {
                              final sizes = _camSizes;
                              if (sz == 'normal') {
                                sizes.remove(id);
                              } else {
                                sizes[id] = sz;
                              }
                              _mutLayout('cam_sizes', sizes);
                            },
                            child: Text(label),
                          ),
                  ),
              ],
            ),
          ],
        ],
      ),
    );
  }

  void _deleteCamera(String itemId) {
    // Only claim success when a record actually went away (egui
    // delete_camera): layout entries are scrubbed with it.
    final cameras = _cameras;
    final idx =
        cameras.indexWhere((c) => 'cam:${c['id']}' == itemId);
    if (idx < 0) {
      _toast(
          "Couldn't find that camera — it may already have been deleted.",
          isError: true);
      return;
    }
    cameras.removeAt(idx);
    _config['cameras'] = cameras;
    final pins = _pins..remove(itemId);
    _layout['pins'] = pins;
    final hidden = _hidden..remove(itemId);
    _layout['hidden'] = hidden;
    final assign = _assign..remove(itemId);
    _layout['assign'] = assign;
    final alias = _alias..remove(itemId);
    _layout['alias'] = alias;
    final sizes = _camSizes..remove(itemId);
    _layout['cam_sizes'] = sizes;
    final order = _sectionOrder;
    for (final list in order.values) {
      list.remove(itemId);
    }
    order.removeWhere((_, list) => list.isEmpty);
    _layout['section_order'] = order;
    _saveConfig();
    _pausedCams.remove(itemId);
    _tileStarted.remove(itemId);
    _frames[itemId]?.dispose();
    _frames.remove(itemId);
    setState(() {});
    _toast('Camera deleted');
  }

  // ================= camera form / new group =================

  Future<void> _openCameraForm(String? editItemId) async {
    Map<String, dynamic>? existing;
    if (editItemId != null) {
      existing = _cameraByItemId(editItemId);
      if (existing == null) {
        // NEVER fall through to a blank Add form here (egui
        // open_camera_form): say the camera is gone.
        _toast(
            "Couldn't find that camera — it may already have been deleted.",
            isError: true);
        return;
      }
    }
    final result = await showDialog<Map<String, dynamic>>(
      context: context,
      builder: (_) => _CameraFormDialog(
        existing: existing,
        // egui deobfuscates the stored password into the edit
        // form; Dart mirrors that with the same XOR+base64 scheme.
        initialPassword: existing == null
            ? ''
            : _deobfuscate(
                existing['password']?.toString() ?? ''),
        onDelete: existing == null
            ? null
            : () => Navigator.of(context).pop({'__delete': true}),
      ),
    );
    if (result == null) return;
    if (result['__delete'] == true && editItemId != null) {
      _deleteCamera(editItemId);
      return;
    }
    final name = (result['name'] as String).trim();
    final host = (result['host'] as String).trim();
    if (name.isEmpty || host.isEmpty) return; // dialog validates
    final port = (int.tryParse(result['port'].toString()) ?? 554)
        .clamp(0, 65535);
    final cameras = _cameras;
    if (existing != null) {
      existing['name'] = name;
      existing['user'] = (result['user'] as String).trim();
      existing['password'] =
          _obfuscate(result['password'] as String);
      existing['host'] = host;
      existing['port'] = port;
      existing['template'] = result['template'];
      existing['path'] = (result['path'] as String).trim();
    } else {
      var id =
          '${DateTime.now().microsecondsSinceEpoch}-${_cameras.length}';
      while (cameras.any((c) => c['id'].toString() == id)) {
        id =
            '${DateTime.now().microsecondsSinceEpoch}-${cameras.length + 1}';
      }
      cameras.add({
        'id': id,
        'name': name,
        'user': (result['user'] as String).trim(),
        'password': _obfuscate(result['password'] as String),
        'host': host,
        'port': port,
        'template': result['template'],
        'path': (result['path'] as String).trim(),
      });
    }
    _config['cameras'] = cameras;
    _saveConfig();
    setState(() {});
    _toast(existing != null ? 'Camera saved' : 'Camera added');
  }

  Future<void> _openNewGroupDialog() async {
    final nameCtrl = TextEditingController();
    final name = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: _pal.card,
        title: Text('New group',
            style: TextStyle(color: _pal.text)),
        content: TextField(
          controller: nameCtrl,
          autofocus: true,
          style: TextStyle(color: _pal.text),
          decoration: _inputDecoration('Group name'),
          onSubmitted: (v) => Navigator.of(context).pop(v.trim()),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          ElevatedButton(
            onPressed: () =>
                Navigator.of(context).pop(nameCtrl.text.trim()),
            style: ElevatedButton.styleFrom(
              backgroundColor: _accent,
              foregroundColor: Colors.white,
            ),
            child: const Text('Create'),
          ),
        ],
      ),
    );
    if (name == null || name.isEmpty) return;
    final groups = _groups;
    groups.add({
      'id': 'g${DateTime.now().millisecondsSinceEpoch}',
      'name': name,
    });
    _config['layout']['groups'] = groups;
    _saveConfig();
    setState(() {});
  }

  // ================= settings =================

  String get _autostartLabel =>
      Platform.isWindows ? 'Start with Windows' : 'Start at login';
  String get _closeToTrayLabel => Platform.isWindows
      ? 'Keep running in the system tray when the window is closed'
      : 'Keep running in the menu bar when the window is closed';

  String get _autostartHelp {
    if (Platform.isWindows) {
      return 'Start with Windows uses the current-user registry Run key — no administrator rights needed, and OFF means no autostart entry exists at all. The “Launch as” choice applies to every launch of the app — autostart, Start Menu, and shortcuts alike — not just the Windows-startup one; a launch that carries its own --minimized or --maximized flag (as the autostart entry does) overrides it for that launch only. “Minimized (to tray)” starts the app hidden: its icon sits in the notification area by the Windows clock, and clicking the icon (or “Open” on its right-click menu) brings the window back. With “Keep running in the system tray” on, closing the window hides it instead of quitting — device refresh and camera previews keep running in the background — and Quit lives on the tray icon\'s right-click menu. The tray icon appears when the app starts minimized or while close-to-tray is on, and then stays for the session. Logging out doesn\'t quit the app either way. This panel is a Windows-app feature; the web edition has no equivalent.';
    }
    return 'Start at login uses a per-user LaunchAgent in ~/Library/LaunchAgents — no administrator rights needed, and OFF means no LaunchAgent file exists at all. The “Launch as” choice applies to every launch of the app — login, Finder, and Dock alike — not just the login one; a launch that carries its own --minimized or --maximized flag (as the LaunchAgent entry does) overrides it for that launch only. “Minimized (to tray)” starts the app hidden: its icon sits in the menu bar at the top of the screen, and clicking the icon (or “Open” on its menu) brings the window back. With “Keep running in the menu bar” on, closing the window hides it instead of quitting — device refresh and camera previews keep running in the background — and Quit lives on the icon\'s menu. The icon appears when the app starts minimized or while close-to-tray is on, and then stays for the session. Logging out doesn\'t quit the app either way. This panel is a native-app feature; the web edition has no equivalent.';
  }

  Future<void> _refreshAutostart() async {
    try {
      final res = await RustBridge.autostartGet();
      if (res['ok'] == true && mounted) {
        setState(() {
          _autostartEnabled = res['enabled'] == true;
          _launchMode =
              res['launch_mode'] as String? ?? 'normal';
          _prefs['start_with_windows'] = _autostartEnabled;
          _prefs['launch_mode'] = _launchMode;
        });
      }
    } catch (_) {}
  }

  Future<void> _setAutostart(bool enabled, String mode) async {
    final prevEnabled = _autostartEnabled;
    final prevMode = _launchMode;
    setState(() {
      _autostartEnabled = enabled;
      _launchMode = mode;
    });
    try {
      final res = await RustBridge.autostartSet(enabled, mode);
      if (res['ok'] == true) {
        _prefs['start_with_windows'] = enabled;
        _prefs['launch_mode'] = mode;
        _saveConfig();
        _toast('$_autostartLabel: ${enabled ? 'on' : 'off'}');
      } else {
        throw StateError(res['error']?.toString() ?? 'failed');
      }
    } catch (_) {
      // Show the store's truth, not the wish (egui behaviour).
      setState(() {
        _autostartEnabled = prevEnabled;
        _launchMode = prevMode;
      });
      _toast("Couldn't update the startup entry.", isError: true);
      _refreshAutostart();
    }
  }

  Widget _settingsPanel(String title, Widget child) {
    final pal = _pal;
    final radius = _compact ? cardRadiusCompact : cardRadius;
    return Container(
      margin: const EdgeInsets.only(bottom: 16),
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: pal.card,
        border: Border.all(color: pal.border),
        borderRadius: BorderRadius.circular(radius),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title,
              style: TextStyle(
                  fontSize: 16,
                  fontWeight: FontWeight.bold,
                  color: pal.text)),
          const SizedBox(height: 12),
          child,
        ],
      ),
    );
  }

  Widget _settingsView() {
    final pal = _pal;
    return Column(
      children: [
        Container(
          color: pal.card,
          padding:
              const EdgeInsets.symmetric(horizontal: 18, vertical: 12),
          child: Row(
            children: [
              const Text('⚙️', style: TextStyle(fontSize: 22)),
              const SizedBox(width: 6),
              Text('Settings',
                  style: TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.bold,
                      color: pal.text)),
              const Spacer(),
              OutlinedButton(
                onPressed: () =>
                    setState(() => _view = AppView.dashboard),
                child: const Text('← Back to dashboard'),
              ),
            ],
          ),
        ),
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.symmetric(vertical: 18),
            child: Center(
              child: Container(
                constraints: const BoxConstraints(maxWidth: 760),
                padding: const EdgeInsets.symmetric(horizontal: 18),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    _settingsPanel(
                        'Appearance',
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            _fieldLabel('Theme'),
                            _dropdown<String>(
                              value: _theme,
                              items: const [
                                ('dark', 'Dark'),
                                ('light', 'Light'),
                                ('black', 'Black'),
                              ],
                              onChanged: (v) {
                                if (v != null) {
                                  _mutPrefs('theme', v);
                                }
                              },
                            ),
                            const SizedBox(height: 10),
                            _fieldLabel('Accent colour'),
                            Row(
                              children: [
                                for (final (name, hex) in accents)
                                  GestureDetector(
                                    onTap: () =>
                                        _mutPrefs('accent', hex),
                                    child: Container(
                                      width: 30,
                                      height: 30,
                                      margin: const EdgeInsets.only(
                                          right: 8),
                                      decoration: BoxDecoration(
                                        color: hexToColor(hex),
                                        shape: BoxShape.circle,
                                        border: _accentHex == hex
                                            ? Border.all(
                                                color: pal.text,
                                                width: 3)
                                            : null,
                                      ),
                                    ),
                                  ),
                              ],
                            ),
                            const SizedBox(height: 10),
                            _fieldLabel('Card density'),
                            _dropdown<String>(
                              value: _compact ? 'compact' : 'comfortable',
                              items: const [
                                ('comfortable', 'Comfortable'),
                                ('compact', 'Compact'),
                              ],
                              onChanged: (v) {
                                if (v != null) {
                                  _mutPrefs('density', v);
                                }
                              },
                            ),
                          ],
                        )),
                    _settingsPanel(
                        'App updates',
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              "You're running version $appVersion. The app checks GitHub for updates at most once a day; you can also check now.",
                              style: TextStyle(
                                  fontSize: 12.5, color: pal.dim),
                            ),
                            const SizedBox(height: 8),
                            if (_updateChecking)
                              const Text('Checking for updates…',
                                  style: TextStyle(fontSize: 13.5))
                            else if (_updateDl != null)
                              Column(
                                crossAxisAlignment:
                                    CrossAxisAlignment.start,
                                children: [
                                  LinearProgressIndicator(
                                    value: _updateDl![1] > 0
                                        ? _updateDl![0] /
                                            _updateDl![1]
                                        : 0,
                                    color: _accent,
                                  ),
                                  const SizedBox(height: 6),
                                  Text(
                                    'Downloading the installer… ${(_updateDl![1] > 0 ? _updateDl![0] / _updateDl![1] * 100 : 0).round()}%',
                                    style: const TextStyle(fontSize: 13.5),
                                  ),
                                ],
                              )
                            else if (_updateInfo != null &&
                                _versionNewer(
                                    _updateInfo!['version']
                                            ?.toString() ??
                                        '',
                                    appVersion)) ...[
                              Text(
                                'Version ${_updateInfo!['version']} is available.',
                                style: const TextStyle(
                                    fontSize: 13.5,
                                    fontWeight: FontWeight.bold),
                              ),
                              const SizedBox(height: 8),
                              Row(
                                children: [
                                  ElevatedButton(
                                    onPressed: _downloadUpdate,
                                    style: ElevatedButton.styleFrom(
                                      backgroundColor: _accent,
                                      foregroundColor: Colors.white,
                                    ),
                                    child: const Text(
                                        'Download & install'),
                                  ),
                                  const SizedBox(width: 8),
                                  OutlinedButton(
                                    onPressed: () =>
                                        _openUrl(releasesPage),
                                    child: const Text(
                                        'Open releases page'),
                                  ),
                                ],
                              ),
                              const SizedBox(height: 6),
                              Text(
                                Platform.isMacOS
                                    ? 'The download\'s file name is verified before it opens. The disk image opens so you can drag the new version into Applications — quit and relaunch the app afterwards.'
                                    : 'The download\'s file name is verified before it runs. This app closes itself once the installer starts, so the installer can replace it.',
                                style: TextStyle(
                                    fontSize: 12.5, color: pal.dim),
                              ),
                            ] else if (_updateInfo != null)
                              Text(
                                "You're up to date — ${_updateInfo!['version']} is the latest version.",
                                style: const TextStyle(fontSize: 13.5),
                              ),
                            if (_updateError != null) ...[
                              const SizedBox(height: 6),
                              Text(_updateError!,
                                  style: const TextStyle(
                                      fontSize: 12.5,
                                      color: dangerColor)),
                              const SizedBox(height: 4),
                              OutlinedButton(
                                onPressed: () => _openUrl(releasesPage),
                                child:
                                    const Text('Open releases page'),
                              ),
                            ],
                            const SizedBox(height: 8),
                            OutlinedButton(
                              onPressed: _updateChecking
                                  ? null
                                  : () =>
                                      _maybeUpdateCheck(manual: true),
                              child: const Text('Check for updates'),
                            ),
                          ],
                        )),
                    _settingsPanel(
                        'Startup & background',
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Row(
                              children: [
                                Checkbox(
                                  value: _autostartEnabled,
                                  activeColor: _accent,
                                  onChanged: (v) => _setAutostart(
                                      v ?? false, _launchMode),
                                ),
                                Text(_autostartLabel,
                                    style: TextStyle(
                                        color: pal.text,
                                        fontSize: 14)),
                              ],
                            ),
                            if (_autostartEnabled) ...[
                              const SizedBox(height: 6),
                              _fieldLabel('Launch as'),
                              Row(
                                children: [
                                  for (final (mode, label) in [
                                    ('normal', 'Normal window'),
                                    ('minimized',
                                        'Minimized (to tray)'),
                                    ('maximized', 'Maximized'),
                                  ])
                                    Padding(
                                      padding: const EdgeInsets.only(
                                          right: 6),
                                      child: _launchMode == mode
                                          ? ElevatedButton(
                                              onPressed: () {},
                                              style: ElevatedButton
                                                  .styleFrom(
                                                backgroundColor:
                                                    _accent,
                                                foregroundColor:
                                                    Colors.white,
                                                disabledForegroundColor:
                                                    Colors.white,
                                              ),
                                              child: Text(label),
                                            )
                                          : OutlinedButton(
                                              onPressed: () =>
                                                  _setAutostart(
                                                      true, mode),
                                              child: Text(label),
                                            ),
                                    ),
                                ],
                              ),
                            ],
                            const SizedBox(height: 10),
                            Row(
                              children: [
                                Checkbox(
                                  value:
                                      _prefs['close_to_tray'] == true,
                                  activeColor: _accent,
                                  onChanged: (v) {
                                    final on = v ?? false;
                                    _mutPrefs('close_to_tray', on);
                                    if (on) {
                                      try {
                                        RustBridge.trayActions(
                                            'ensure');
                                      } catch (_) {}
                                    }
                                  },
                                ),
                                Expanded(
                                  child: Text(_closeToTrayLabel,
                                      style: TextStyle(
                                          color: pal.text,
                                          fontSize: 14)),
                                ),
                              ],
                            ),
                            const SizedBox(height: 10),
                            Text(_autostartHelp,
                                style: TextStyle(
                                    fontSize: 12.5, color: pal.dim)),
                            // TODO(GAP) tray hide-on-close: see the
                            // file header. The setting above is
                            // saved and the Rust tray icon is
                            // registered, but Flutter cannot hide
                            // its window on close without the
                            // window_manager plugin.
                            const SizedBox(height: 8),
                            Text(
                              'Note: hiding the window on close needs the window_manager plugin (GAP) — until that is wired, closing the window quits the app even with this setting on. The setting itself is saved and the tray icon is registered.',
                              style: TextStyle(
                                  fontSize: 12.5,
                                  color: dangerColor),
                            ),
                          ],
                        )),
                    _settingsPanel(
                        'Connection',
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              "Tuya's cloud API is called straight from this app. The US and EU endpoints accept direct calls; the China endpoint (and some networks) need a CORS-style proxy in between.",
                              style: TextStyle(
                                  fontSize: 12.5, color: pal.dim),
                            ),
                            const SizedBox(height: 8),
                            _proxyFields(),
                            const SizedBox(height: 10),
                            _fieldLabel('Autorefresh interval'),
                            _dropdown<int>(
                              value: _interval,
                              items: intervals,
                              onChanged: (v) {
                                if (v != null) {
                                  _mutPrefs('interval', v);
                                }
                              },
                            ),
                          ],
                        )),
                    _settingsPanel(
                        'Stream gateway & cameras',
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              'Cameras you add (✎ Edit → + Add camera) stream over RTSP. This app plays them in place: a bundled copy of ffmpeg decodes each stream for the live tile previews and the ▶ Watch player — it just works, nothing to set up. Alternatively, point ▶ Watch at a small stream gateway on your network — go2rtc or MediaMTX — and Watch opens the gateway\'s watch page in your browser instead (the gateway needs a stream with the same name as the camera here, fed from the same RTSP address — the card\'s Copy URL button gives you the exact address). If ffmpeg can\'t be found, previews switch off and Watch falls back to the external player app below.',
                              style: TextStyle(
                                  fontSize: 12.5, color: pal.dim),
                            ),
                            const SizedBox(height: 8),
                            Row(
                              children: [
                                Checkbox(
                                  value: _previewsOn,
                                  activeColor: _accent,
                                  onChanged: (v) => _mutPrefs(
                                      'previews', v ?? true),
                                ),
                                Text('Live camera previews',
                                    style: TextStyle(
                                        color: pal.text,
                                        fontSize: 14)),
                              ],
                            ),
                            const SizedBox(height: 4),
                            Text(
                              'Live video inside each camera tile (Stream 2 on Tapo cameras, to keep it light). The ▶ Watch player always uses the quality picked on the card.',
                              style: TextStyle(
                                  fontSize: 12.5, color: pal.dim),
                            ),
                            const SizedBox(height: 10),
                            _fieldLabel(
                                'ffmpeg path (optional override)'),
                            TextField(
                              controller: TextEditingController(
                                  text: _prefs['ffmpeg_path'] as String? ??
                                      ''),
                              style: TextStyle(color: pal.text),
                              decoration: _inputDecoration(
                                  'Leave empty — the bundled ffmpeg is used'),
                              onSubmitted: (v) {
                                _mutPrefs('ffmpeg_path', v);
                                _refreshFfmpeg();
                              },
                            ),
                            const SizedBox(height: 4),
                            Text(_ffmpegStatus,
                                style: TextStyle(
                                    fontSize: 12.5, color: pal.dim)),
                            const SizedBox(height: 10),
                            _fieldLabel(
                                'Stream gateway address (optional)'),
                            TextField(
                              controller: TextEditingController(
                                  text: _prefs['gateway_url'] as String? ??
                                      ''),
                              style: TextStyle(color: pal.text),
                              decoration: _inputDecoration(
                                  'http://192.168.1.10:1984 (go2rtc) · http://192.168.1.10:8889 (MediaMTX)'),
                              onSubmitted: (v) =>
                                  _mutPrefs('gateway_url', v),
                            ),
                            const SizedBox(height: 10),
                            _fieldLabel('Gateway type'),
                            _dropdown<String>(
                              value: _prefs['gateway_type'] as String? ??
                                  'go2rtc',
                              items: const [
                                ('go2rtc', 'go2rtc'),
                                ('mediamtx', 'MediaMTX (HLS)'),
                              ],
                              onChanged: (v) {
                                if (v != null) {
                                  _mutPrefs('gateway_type', v);
                                }
                              },
                            ),
                            const SizedBox(height: 10),
                            _fieldLabel(
                                'External player command (fallback)'),
                            TextField(
                              controller: TextEditingController(
                                  text: _prefs['player_cmd'] as String? ??
                                      'vlc'),
                              style: TextStyle(color: pal.text),
                              decoration: _inputDecoration('vlc'),
                              onSubmitted: (v) =>
                                  _mutPrefs('player_cmd', v),
                            ),
                            const SizedBox(height: 4),
                            Text(
                              'The fallback when ffmpeg isn\'t available (and what “Open in external player” uses): this program is started with the stream URL as its only argument. A bare name like vlc is found even when it isn\'t on PATH — the Windows App Paths registry entries and the usual install folders are checked too.',
                              style: TextStyle(
                                  fontSize: 12.5, color: pal.dim),
                            ),
                          ],
                        )),
                    _settingsPanel(
                        'Saved login',
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              _config['creds'] is Map<String, dynamic>
                                  ? "A login is saved for ${(_config['creds'] as Map<String, dynamic>)['user_name']} in this device's config (used for silent re-login when the session expires)."
                                  : 'No login is saved on this device.',
                              style: TextStyle(
                                  fontSize: 12.5, color: pal.dim),
                            ),
                            const SizedBox(height: 8),
                            OutlinedButton(
                              onPressed: () {
                                _config['creds'] = null;
                                _config['session'] = null;
                                _saveConfig();
                                setState(() {});
                                _toast('Saved login forgotten');
                              },
                              child: const Text('Forget saved login',
                                  style:
                                      TextStyle(color: dangerColor)),
                            ),
                          ],
                        )),
                    _settingsPanel(
                        'Hidden devices',
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              'Devices and cameras you hid with the ✕ on their card. They stay hidden until you bring them back here.',
                              style: TextStyle(
                                  fontSize: 12.5, color: pal.dim),
                            ),
                            const SizedBox(height: 6),
                            if (_hidden.isEmpty)
                              Text('Nothing is hidden. 🎉',
                                  style: TextStyle(
                                      fontSize: 12.5, color: pal.dim))
                            else
                              for (final id in _hidden)
                                Column(
                                  children: [
                                    Row(
                                      children: [
                                        Expanded(
                                          child: Text(
                                              _hiddenName(id),
                                              style: const TextStyle(
                                                  fontSize: 14)),
                                        ),
                                        OutlinedButton(
                                          onPressed: () {
                                            final hidden = _hidden
                                              ..remove(id);
                                            _mutLayout(
                                                'hidden', hidden);
                                          },
                                          child:
                                              const Text('Unhide'),
                                        ),
                                      ],
                                    ),
                                    const Divider(),
                                  ],
                                ),
                          ],
                        )),
                    _settingsPanel(
                        'Help',
                        Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            for (final p in _helpParas) ...[
                              Text(p,
                                  style: TextStyle(
                                      fontSize: 13.5,
                                      color: pal.dim)),
                              const SizedBox(height: 12),
                            ],
                          ],
                        )),
                  ],
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }

  String _hiddenName(String id) {
    final d = _deviceById(id);
    if (d != null) return _displayName(id, d['name']?.toString() ?? id);
    final c = _cameraByItemId(id);
    if (c != null) return c['name']?.toString() ?? id;
    // A hidden device that is not currently in the list (e.g. the
    // account's device list changed) still shows its id so it can
    // be unhidden.
    return id;
  }

  static const List<String> _helpParas = [
    'How login works. This app signs in to Tuya\'s cloud with the same account you use in the Smart Life (or Tuya Smart) app, then asks Tuya for your device list and sends control commands — the same approach as the classic Smart Life web dashboards. Pick the region where your account was registered; the wrong region is the most common cause of “login failed”.',
    'Your credentials. Your username and password go directly to Tuya\'s servers (px1.tuya*.com) over HTTPS. If you tick “Keep me logged in”, they are kept in this app\'s config file (in your user profile, under SmartLifeTuyaDesktop\\config.json) so the app can silently sign back in when the session token expires — use it only on your own devices, and use “Forget saved login” above to erase them.',
    'The proxy. Some networks block direct calls to Tuya\'s endpoints. In “Auto” mode, China-region traffic is routed through a community CORS proxy (cors.smartathome.co.uk); US and EU traffic goes direct. Note a proxy can see the requests it forwards (they contain a session token, never your password after sign-in — the sign-in request itself does contain your password, so only use proxies you trust for CN logins).',
    'Customising. Sections appear as Cameras, Pinned, Lighting, your groups, Ungrouped, Scenes by default — and in ✎ Edit mode every section header has ↑/↓ buttons to move the section itself; your order sticks. Inside a section, cards sort A–Z by name by default: drag cards into your own order — drag a card by its body and drop it where it belongs, or use the ↑/↓ buttons (that order then sticks, and each section has a “Reset to name order” button) — create groups (rooms), rename devices with an alias, or hide devices you never use. Dragging a card onto another section moves it there. Use the 📌 button on any card to pin a shortcut copy of it in the Pinned section — the device also stays in its group below. Scenes always live in their own Scenes section.',
    'Lighting & group controls. Lights organise themselves: any bulb or strip that isn\'t assigned to one of your groups lives in the built-in Lighting section, and cameras get their own built-in Cameras section the same way — assigning a device to a group always wins over the automatic sections. The Lighting header (and the header of any group containing a light) has group controls: a master switch (on if any member is on; clicking turns the whole group on or off), brightness and colour-temperature sliders showing the members\' average, and colour swatches. Group controls broadcast — they send the command to each member in turn, and changing a single light afterwards simply overrides it for that light. Nothing about the group itself is stored.',
    'Cameras (RTSP). Tapo cameras: in the Tapo app, open the camera → Settings → Advanced Settings → Camera Account and create a camera account, and make sure RTSP is enabled for the camera. The username and password you add here are that camera account — not your TP-Link cloud account. Then use ✎ Edit → + Add camera and enter the camera\'s IP address (a fixed or DHCP-reserved IP works best, so the address doesn\'t change). The app builds the stream address for you: rtsp://user:pass@IP:554/stream1 (high quality) or /stream2 (standard) — or pick “Custom path” for other RTSP cameras. Camera cards live alongside your devices and can be pinned, grouped and reordered the same way; unassigned cameras live in the built-in Cameras section (the first section by default) until you assign them to a group of your own. In Edit mode a camera card can also be resized — Normal (one grid cell), Large (two columns) or Wide (the full row).',
    'Watching a stream. This app plays RTSP streams in place: a bundled copy of ffmpeg decodes the feed for the live tile previews (Tapo Stream 2, the standard-quality stream, to keep tiles light — Large tiles decode it bigger, and a Wide tile uses the quality picked on the card) and for the big ▶ Watch player, which uses the quality picked on the card. Tile previews can be turned off in Settings → Stream gateway & cameras, where you can also point the app at a different ffmpeg. If ffmpeg can\'t be found, previews switch off and ▶ Watch falls back to the external player app named in Settings (VLC by default) — or use Copy URL and paste the address into VLC\'s “Open Network Stream”. If a stream gateway (go2rtc / MediaMTX) is configured instead, ▶ Watch opens the gateway\'s watch page in your browser. (The web edition of this app can\'t decode RTSP itself: with a gateway configured, its tiles embed the gateway\'s feed; without one, its Watch hands the stream to a player app.)',
    'Limits. Tuya rate-limits how often the device list can be fetched and how fast commands can be sent. When that happens the app quietly keeps showing the last known states, retries commands a moment later, and stretches its refresh interval until Tuya is happy again — no action needed. Autorefresh every 30s or slower is comfortable.',
  ];

  // ================= updates =================

  Future<void> _maybeUpdateCheck({required bool manual}) async {
    final update = _config['update'];
    final last = update is Map<String, dynamic> &&
            update['last_check_ms'] is num
        ? (update['last_check_ms'] as num).toInt()
        : 0;
    final nowMs = DateTime.now().millisecondsSinceEpoch;
    if (!manual &&
        nowMs - last < 24 * 60 * 60 * 1000 &&
        _updateInfo == null) {
      // At most once per 24h (attempt-based, egui UPDATE_PERIOD_MS).
      return;
    }
    if (_updateChecking) return;
    setState(() {
      _updateChecking = true;
      _updateError = null;
    });
    try {
      final res = await RustBridge.updateCheck(
          {'action': 'check', 'manual': manual});
      if (update is Map<String, dynamic>) {
        update['last_check_ms'] = nowMs;
        _saveConfig();
      }
      if (res['ok'] == true && res['info'] is Map<String, dynamic>) {
        setState(
            () => _updateInfo = res['info'] as Map<String, dynamic>);
      } else if (res['ok'] != true) {
        setState(() => _updateError =
            res['error']?.toString() ?? 'Update check failed');
      }
    } catch (e) {
      setState(() => _updateError = 'Update check failed: $e');
    }
    setState(() => _updateChecking = false);
  }

  Future<void> _downloadUpdate() async {
    if (_updateInfo == null) return;
    setState(() {
      _updateDl = [0, 1];
      _updateError = null;
    });
    try {
      final dl = await RustBridge.updateCheck({'action': 'download'});
      if (dl['ok'] != true || dl['downloaded_path'] == null) {
        throw StateError(
            dl['error']?.toString() ?? 'Download failed');
      }
      final path = dl['downloaded_path'].toString();
      final inst = await RustBridge.updateCheck(
          {'action': 'install', 'path': path});
      if (inst['ok'] != true) {
        throw StateError(
            inst['error']?.toString() ?? 'Install failed');
      }
      if (Platform.isWindows) {
        // Windows: the installer replaces the running app, so this
        // app closes itself once the installer starts (egui).
        exit(0);
      } else {
        // macOS: the disk image opens; the user drags the new
        // version into Applications, then quits and relaunches.
        _toast(
            'Disk image opened — drag the new version into Applications, then quit and relaunch.');
      }
    } catch (e) {
      setState(() {
        _updateError = 'Download failed: $e';
        _updateDl = null;
      });
    }
    if (mounted) setState(() => _updateDl = null);
  }
}

// ================= reusable widgets =================

/// Slider with a live label while dragging; the command fires only
/// on release (egui sliders commit on release too, and autorefresh
/// pauses while a slider is being dragged).
class _LiveSlider extends StatefulWidget {
  final String label;
  final double value;
  final double min;
  final double max;
  final int divisions;
  final String Function(int) format;
  final Color accent;
  final Color dim;
  final Color text;
  final VoidCallback onStart;
  final ValueChanged<int> onEnd;

  const _LiveSlider({
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.divisions,
    required this.format,
    required this.accent,
    required this.dim,
    required this.text,
    required this.onStart,
    required this.onEnd,
  });

  @override
  State<_LiveSlider> createState() => _LiveSliderState();
}

class _LiveSliderState extends State<_LiveSlider> {
  late double _current;
  bool _dragging = false;

  @override
  void initState() {
    super.initState();
    _current = widget.value;
  }

  @override
  void didUpdateWidget(_LiveSlider old) {
    super.didUpdateWidget(old);
    if (!_dragging) _current = widget.value;
  }

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Text(widget.label,
            style: TextStyle(fontSize: 12.5, color: widget.dim)),
        const SizedBox(width: 8),
        Expanded(
          child: Slider(
            value: _current.clamp(widget.min, widget.max),
            min: widget.min,
            max: widget.max,
            divisions: widget.divisions,
            activeColor: widget.accent,
            onChangeStart: (_) {
              _dragging = true;
              widget.onStart();
            },
            onChanged: (v) => setState(() => _current = v),
            onChangeEnd: (v) {
              _dragging = false;
              widget.onEnd(v.round());
            },
          ),
        ),
        Text(widget.format(_current.round()),
            style: TextStyle(
                fontSize: 12.5,
                fontWeight: FontWeight.bold,
                color: widget.text)),
      ],
    );
  }
}

/// RGB sliders + hex field + Apply colour. Flutter has no
/// built-in colour editor, so this is the "More colours…" picker
/// (see the GAP note at the top of this file).
class _ColorPickerRow extends StatefulWidget {
  final Color initial;
  final Color accent;
  final Color dim;
  final Color text;
  final ValueChanged<Color> onApply;

  const _ColorPickerRow({
    required this.initial,
    required this.accent,
    required this.dim,
    required this.text,
    required this.onApply,
  });

  @override
  State<_ColorPickerRow> createState() => _ColorPickerRowState();
}

class _ColorPickerRowState extends State<_ColorPickerRow> {
  late double _r, _g, _b;
  late TextEditingController _hex;

  @override
  void initState() {
    super.initState();
    _r = widget.initial.red.toDouble();
    _g = widget.initial.green.toDouble();
    _b = widget.initial.blue.toDouble();
    _hex = TextEditingController(text: colorToHex(widget.initial));
  }

  @override
  void dispose() {
    _hex.dispose();
    super.dispose();
  }

  Color get _color => Color.fromARGB(
      255, _r.round(), _g.round(), _b.round());

  void _fromHex(String v) {
    final c = hexToColor(v);
    setState(() {
      _r = c.red.toDouble();
      _g = c.green.toDouble();
      _b = c.blue.toDouble();
    });
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Row(
          children: [
            Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                color: _color,
                borderRadius: BorderRadius.circular(8),
                border: Border.all(color: widget.dim),
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: TextField(
                controller: _hex,
                style: TextStyle(color: widget.text),
                decoration: const InputDecoration(
                  labelText: 'Hex colour',
                  isDense: true,
                ),
                onSubmitted: _fromHex,
              ),
            ),
            const SizedBox(width: 8),
            ElevatedButton(
              onPressed: () => widget.onApply(_color),
              style: ElevatedButton.styleFrom(
                backgroundColor: widget.accent,
                foregroundColor: Colors.white,
              ),
              child: const Text('Apply colour'),
            ),
          ],
        ),
        _rgbRow('R', _r, (v) => _r = v),
        _rgbRow('G', _g, (v) => _g = v),
        _rgbRow('B', _b, (v) => _b = v),
      ],
    );
  }

  Widget _rgbRow(
      String label, double value, ValueChanged<double> set) {
    return Row(
      children: [
        SizedBox(
          width: 16,
          child: Text(label,
              style:
                  TextStyle(fontSize: 12.5, color: widget.dim)),
        ),
        Expanded(
          child: Slider(
            value: value,
            min: 0,
            max: 255,
            divisions: 255,
            activeColor: widget.accent,
            onChanged: (v) => setState(() {
              set(v);
              _hex.text = colorToHex(_color);
            }),
          ),
        ),
        Text('${value.round()}',
            style:
                TextStyle(fontSize: 12.5, color: widget.text)),
      ],
    );
  }
}

/// Add / Edit camera dialog (egui camera_window): name, username,
/// password, IP address or host, port (default 554), template
/// (Tapo Stream 1 HD / Tapo Stream 2 SD / Custom path), stream
/// path only for Custom. Returns the form map, or {'__delete':
/// true} for the edit-mode Delete button.
class _CameraFormDialog extends StatefulWidget {
  final Map<String, dynamic>? existing;
  final String initialPassword;
  final VoidCallback? onDelete;

  const _CameraFormDialog(
      {this.existing, this.initialPassword = '', this.onDelete});

  @override
  State<_CameraFormDialog> createState() => _CameraFormDialogState();
}

class _CameraFormDialogState extends State<_CameraFormDialog> {
  late TextEditingController _name;
  late TextEditingController _user;
  late TextEditingController _pass;
  late TextEditingController _host;
  late TextEditingController _port;
  late TextEditingController _path;
  String _template = 'tapo1';
  String? _error;

  @override
  void initState() {
    super.initState();
    final e = widget.existing;
    _name = TextEditingController(text: e?['name']?.toString() ?? '');
    _user = TextEditingController(text: e?['user']?.toString() ?? '');
    // egui deobfuscates the stored password into the edit form;
    // the caller passes the deobfuscated value in, so editing a
    // camera keeps its password unless the user changes it.
    _pass = TextEditingController(text: widget.initialPassword);
    _host = TextEditingController(text: e?['host']?.toString() ?? '');
    _port = TextEditingController(
        text: e?['port']?.toString() ?? '554');
    _path = TextEditingController(text: e?['path']?.toString() ?? '');
    _template = e?['template']?.toString() ?? 'tapo1';
  }

  @override
  void dispose() {
    _name.dispose();
    _user.dispose();
    _pass.dispose();
    _host.dispose();
    _port.dispose();
    _path.dispose();
    super.dispose();
  }

  Widget _field(String label, TextEditingController c,
      {bool password = false}) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label,
            style: const TextStyle(
                fontSize: 12.5, fontWeight: FontWeight.bold)),
        const SizedBox(height: 4),
        TextField(
          controller: c,
          obscureText: password,
          decoration: const InputDecoration(isDense: true),
        ),
        const SizedBox(height: 8),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.existing != null ? 'Edit camera' : 'Add camera'),
      content: SingleChildScrollView(
        child: SizedBox(
          width: 320,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              _field('Name', _name),
              _field('Username', _user),
              _field('Password', _pass, password: true),
              _field('IP address or host', _host),
              _field('Port', _port),
              const Align(
                alignment: Alignment.centerLeft,
                child: Text('Template',
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.bold)),
              ),
              const SizedBox(height: 4),
              DropdownButton<String>(
                value: _template,
                isExpanded: true,
                items: cameraTemplates
                    .map((e) => DropdownMenuItem<String>(
                        value: e.$1, child: Text(e.$2)))
                    .toList(),
                onChanged: (v) {
                  if (v != null) setState(() => _template = v);
                },
              ),
              const SizedBox(height: 8),
              if (_template == 'custom') _field('Stream path', _path),
              const Text(
                'Tapo: in the Tapo app, enable RTSP (Advanced Settings → Camera Account) and create a camera account — the username and password are that account, not your TP-Link cloud login.',
                style: TextStyle(fontSize: 12.5),
              ),
              if (_error != null) ...[
                const SizedBox(height: 6),
                Text(_error!,
                    style: const TextStyle(color: dangerColor)),
              ],
            ],
          ),
        ),
      ),
      actions: [
        if (widget.existing != null)
          TextButton(
            onPressed: widget.onDelete,
            child: const Text('Delete',
                style: TextStyle(color: dangerColor)),
          ),
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        ElevatedButton(
          onPressed: () {
            if (_name.text.trim().isEmpty ||
                _host.text.trim().isEmpty) {
              setState(
                  () => _error = 'Name and host are required.');
              return;
            }
            Navigator.of(context).pop({
              'name': _name.text,
              'user': _user.text,
              'password': _pass.text,
              'host': _host.text,
              'port': _port.text,
              'template': _template,
              'path': _path.text,
            });
          },
          child: const Text('Save camera'),
        ),
      ],
    );
  }
}

/// The big ▶ Watch player (egui player_window): a 960×540 ffmpeg
/// pump for one camera, at the quality picked on the card, with
/// Close, ↻ Retry ("Restart the feed now"), Copy URL and Open in
/// external player. Closing stops the player pump.
class _WatchPlayer extends StatefulWidget {
  final _Entry entry;
  final Map<String, dynamic> cam;
  final String rtspUrl;
  final String? gatewayUrl;
  final VoidCallback onCopyUrl;
  final VoidCallback onExternalPlayer;
  final ValueChanged<String> onTemplateChanged;

  const _WatchPlayer({
    required this.entry,
    required this.cam,
    required this.rtspUrl,
    required this.gatewayUrl,
    required this.onCopyUrl,
    required this.onExternalPlayer,
    required this.onTemplateChanged,
  });

  @override
  State<_WatchPlayer> createState() => _WatchPlayerState();
}

class _WatchPlayerState extends State<_WatchPlayer> {
  ui.Image? _image;
  String _state = 'starting';
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(milliseconds: 500),
        (_) => _poll());
    _poll();
  }

  @override
  void dispose() {
    _timer?.cancel();
    _image?.dispose();
    super.dispose();
  }

  Future<void> _poll() async {
    if (!mounted) return;
    try {
      final res = await RustBridge.cameraFrame(
          widget.entry.id, 'player');
      if (res['ok'] != true || !mounted) return;
      final b64 = res['rgba_base64'] as String?;
      final w = res['width'];
      final h = res['height'];
      if (b64 != null && w is num && h is num) {
        final bytes = base64Decode(b64);
        final old = _image;
        ui.decodeImageFromPixels(
          bytes,
          w.toInt(),
          h.toInt(),
          ui.PixelFormat.rgba8888,
          (img) {
            old?.dispose();
            if (mounted) {
              setState(() {
                _image = img;
                _state = 'live';
              });
            } else {
              img.dispose();
            }
          },
        );
      } else {
        setState(
            () => _state = res['state']?.toString() ?? 'starting');
      }
    } catch (_) {}
  }

  @override
  Widget build(BuildContext context) {
    final cam = widget.cam;
    final template = cam['template'] as String? ?? 'tapo1';
    return AlertDialog(
      title: Row(
        children: [
          Expanded(child: Text(cam['name']?.toString() ?? '')),
          IconButton(
            tooltip: 'Close',
            onPressed: () => Navigator.of(context).pop(),
            icon: const Icon(Icons.close),
          ),
        ],
      ),
      content: SizedBox(
        width: 960,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              AspectRatio(
                aspectRatio: 16 / 9,
                child: Container(
                  color: Colors.black,
                  child: _image != null
                      ? RawImage(image: _image, fit: BoxFit.contain)
                      : Center(
                          child: Text(
                            _state == 'live'
                                ? 'LIVE'
                                : 'Starting feed…',
                            style: const TextStyle(
                                color: Colors.white70),
                          ),
                        ),
                ),
              ),
              const SizedBox(height: 8),
              Text(cameraStatusText(cam)),
              const SizedBox(height: 8),
              Row(
                children: [
                  OutlinedButton(
                    onPressed: () async {
                      try {
                        await RustBridge.cameraStop(
                            widget.entry.id, 'player');
                        await RustBridge.cameraStart(
                            widget.entry.id, 'player');
                      } catch (_) {}
                    },
                    child: const Text('↻ Retry'),
                  ),
                  const SizedBox(width: 6),
                  OutlinedButton(
                    onPressed: widget.onCopyUrl,
                    child: const Text('Copy URL'),
                  ),
                  const SizedBox(width: 6),
                  OutlinedButton(
                    onPressed: widget.onExternalPlayer,
                    child: const Text('Open in external player'),
                  ),
                ],
              ),
              if (template == 'tapo1' || template == 'tapo2') ...[
                const SizedBox(height: 8),
                Row(
                  children: [
                    const Text('Quality'),
                    const SizedBox(width: 8),
                    for (final (tpl, label) in [
                      ('tapo1', 'Stream 1 · HD'),
                      ('tapo2', 'Stream 2 · SD'),
                    ])
                      Padding(
                        padding: const EdgeInsets.only(right: 6),
                        child: tpl == template
                            ? ElevatedButton(
                                onPressed: () {},
                                child: Text(label),
                              )
                            : OutlinedButton(
                                onPressed: () =>
                                    widget.onTemplateChanged(tpl),
                                child: Text(label),
                              ),
                      ),
                  ],
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
