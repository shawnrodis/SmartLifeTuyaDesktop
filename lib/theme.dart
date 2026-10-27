// Design tokens for SmartLifeTuyaDesktop, mirrored 1:1 with the
// egui build's DESIGN.md values (src/main.rs: DARK / LIGHT / BLACK,
// ACCENTS, SWATCHES, R_CARD, grid minmax/gap, content max 1240px).
//
// Theme rule (egui parity): the STORED theme (Dark / Light /
// Black) + accent always win. The OS light/dark setting is only a
// fallback when no theme has ever been stored — the app never
// follows the system theme once a preference exists. Fresh-install
// default is Black (egui v1.3.0).

import 'package:flutter/material.dart';

/// One egui Palette: background, card, border, text, dim, faint,
/// slider track.
class AppPalette {
  final Color bg;
  final Color card;
  final Color border;
  final Color text;
  final Color dim;
  final Color faint;
  final Color track;

  const AppPalette({
    required this.bg,
    required this.card,
    required this.border,
    required this.text,
    required this.dim,
    required this.faint,
    required this.track,
  });
}

const AppPalette darkPalette = AppPalette(
  bg: Color(0xFF0B0E14),
  card: Color(0xFF151A24),
  border: Color(0xFF232B3A),
  text: Color(0xFFE7EBF2),
  dim: Color(0xFF8B94A7),
  faint: Color(0xFF5B6478),
  track: Color(0xFF2A3245),
);

const AppPalette lightPalette = AppPalette(
  bg: Color(0xFFEEF1F6),
  card: Color(0xFFFFFFFF),
  border: Color(0xFFDDE3EE),
  text: Color(0xFF17202E),
  dim: Color(0xFF5D6879),
  faint: Color(0xFF8B94A7),
  track: Color(0xFFDBE1EC),
);

const AppPalette blackPalette = AppPalette(
  bg: Color(0xFF000000),
  card: Color(0xFF0C0D11),
  border: Color(0xFF1F232E),
  text: Color(0xFFE7EBF2),
  dim: Color(0xFF8B94A7),
  faint: Color(0xFF5B6478),
  track: Color(0xFF22252E),
);

const Color dangerColor = Color(0xFFF43F5E);
const Color okGreenColor = Color(0xFF22C55E);

/// Accent choices, egui ACCENTS order. Indigo is the default.
const List<(String, String)> accents = [
  ('Indigo', '#6366f1'),
  ('Sky', '#0ea5e9'),
  ('Teal', '#14b8a6'),
  ('Green', '#22c55e'),
  ('Amber', '#f59e0b'),
  ('Rose', '#f43f5e'),
  ('Violet', '#8b5cf6'),
];

const String defaultAccentHex = '#6366f1';

/// Colour swatches on colour-capable cards, egui SWATCHES order.
const List<(String, String)> swatches = [
  ('Warm white', '#ffd9a0'),
  ('Cool white', '#eaf2ff'),
  ('Red', '#ff2d2d'),
  ('Orange', '#ff7a00'),
  ('Yellow', '#ffd400'),
  ('Green', '#22c55e'),
  ('Cyan', '#00c2ff'),
  ('Blue', '#2456ff'),
  ('Purple', '#8b3dff'),
  ('Pink', '#ff4fa3'),
];

/// Login regions, egui REGIONS: value = country calling code.
const List<(String, String)> regions = [
  ('1', 'United States (US)'),
  ('44', 'Europe (EU)'),
  ('86', 'China (CN)'),
];

/// App platforms, egui PLATFORMS.
const List<(String, String)> platforms = [
  ('smart_life', 'Smart Life'),
  ('tuya', 'Tuya Smart'),
];

/// Autorefresh intervals, egui INTERVALS.
const List<(int, String)> intervals = [
  (15, '15s'),
  (30, '30s'),
  (60, '60s'),
  (120, '2 min'),
];

String intervalLabel(int seconds) {
  for (final (v, l) in intervals) {
    if (v == seconds) return l;
  }
  return '30s';
}

/// CORS proxy presets, egui PROXY_PRESETS.
const List<(String, String)> proxyPresets = [
  ('auto', 'Auto — direct for US/EU, community proxy for CN (recommended)'),
  ('smartathome', 'SmartAtHome community proxy (all requests)'),
  ('corsproxy', 'corsproxy.io (encoded URL)'),
  ('cors-anywhere', 'cors-anywhere.herokuapp.com (may need activation)'),
  ('none', 'None — direct connection'),
  ('custom', 'Custom…'),
];

String presetLabel(List<(String, String)> table, String value) {
  for (final (v, l) in table) {
    if (v == value) return l;
  }
  return '';
}

/// Camera templates, egui CAMERA_TEMPLATES.
const List<(String, String)> cameraTemplates = [
  ('tapo1', 'Tapo Stream 1 — High Quality'),
  ('tapo2', 'Tapo Stream 2 — Standard'),
  ('custom', 'Custom path'),
];

/// Card geometry, egui R_CARD / grid tokens.
const double cardRadius = 18.0;
const double cardRadiusCompact = 14.0;
const double buttonRadius = 11.0;
const double loginRadius = 22.0;
const double contentMaxWidth = 1240.0;
const double gridMinComfortable = 238.0;
const double gridGapComfortable = 14.0;
const double gridMinCompact = 198.0;
const double gridGapCompact = 10.0;

/// Device-type icons, egui type_icon().
String typeIcon(String kind) {
  switch (kind) {
    case 'light':
      return '💡';
    case 'strip':
      return '🌈';
    case 'switch':
      return '🔌';
    case 'scene':
      return '🎬';
    case 'cover':
      return '🪟';
    case 'climate':
      return '🌡️';
    case 'fan':
      return '🌀';
    case 'lock':
      return '🔒';
    case 'sensor':
      return '📡';
    case 'camera':
      return '📷';
    default:
      return '🔘';
  }
}

Color hexToColor(String hex) {
  final h = hex.replaceFirst('#', '');
  if (h.length == 6) {
    final n = int.tryParse(h, radix: 16);
    if (n != null) {
      return Color(0xFF000000 | n);
    }
  }
  return Colors.white;
}

String colorToHex(Color c) {
  return '#${c.red.toRadixString(16).padLeft(2, '0')}'
      '${c.green.toRadixString(16).padLeft(2, '0')}'
      '${c.blue.toRadixString(16).padLeft(2, '0')}';
}

AppPalette paletteFor(String theme, Brightness system) {
  switch (theme) {
    case 'light':
      return lightPalette;
    case 'black':
      return blackPalette;
    case 'dark':
      return darkPalette;
    default:
      // No stored theme: system is the fallback only.
      return system == Brightness.light ? lightPalette : blackPalette;
  }
}

/// Brightness mapping, egui brightness_ui_to_device /
/// brightness_device_to_ui (v1.3.1): sliders show 1..100, sends
/// run 11..100. The floor is 11% — a bulb at its minimum displays
/// "1%".
int brightnessUiToDevice(int ui) {
  final clamped = ui.clamp(1, 100);
  return (11 + (clamped - 1) * 89 / 99).round();
}

int brightnessDeviceToUi(int dev) {
  return (1 + (dev - 11) * 99 / 89).round().clamp(1, 100);
}

/// Colour-temperature stops, egui TEMP_STOPS_K. Sliders show stop
/// indices (0..4); the reported value displays at its nearest
/// stop without being rewritten.
const List<int> tempStopsK = [2700, 3000, 4000, 5000, 6500];

/// Reported temp slider (1000..10000 scale) -> Kelvin label.
/// egui temp_slider_to_kelvin: kelvin = 2700 + (slider-1000) *
/// (6500-2700) / 9000, rounded to the nearest stop for display.
int tempSliderToKelvin(int slider) {
  final k = 2700 + (slider - 1000) * 3800 ~/ 9000;
  var best = tempStopsK.first;
  var bestDiff = (k - best).abs();
  for (final stop in tempStopsK) {
    final diff = (k - stop).abs();
    if (diff < bestDiff) {
      best = stop;
      bestDiff = diff;
    }
  }
  return best;
}

int tempStopIndex(int kelvin) {
  var best = 0;
  var bestDiff = (kelvin - tempStopsK[0]).abs();
  for (var i = 1; i < tempStopsK.length; i++) {
    final diff = (kelvin - tempStopsK[i]).abs();
    if (diff < bestDiff) {
      best = i;
      bestDiff = diff;
    }
  }
  return best;
}

int tempStopKelvin(int index) {
  return tempStopsK[index.clamp(0, tempStopsK.length - 1)];
}

/// Slider value (1000..10000) for a stop index, egui
/// temp_stop_kelvin inverse: slider = 1000 + (K-2700)*9000/3800.
int tempSliderForStop(int index) {
  final k = tempStopKelvin(index);
  return 1000 + (k - 2700) * 9000 ~/ 3800;
}

/// HSV (h 0..360, s/v 0..1) -> hex, egui hsv_to_hex.
String hsvToHex(double h, double s, double v) {
  final c = v * s;
  final x = c * (1 - ((h / 60) % 2 - 1).abs());
  final m = v - c;
  double r = 0, g = 0, b = 0;
  if (h < 60) {
    r = c;
    g = x;
  } else if (h < 120) {
    r = x;
    g = c;
  } else if (h < 180) {
    g = c;
    b = x;
  } else if (h < 240) {
    g = x;
    b = c;
  } else if (h < 300) {
    r = x;
    b = c;
  } else {
    r = c;
    b = x;
  }
  final ri = ((r + m) * 255).round().clamp(0, 255);
  final gi = ((g + m) * 255).round().clamp(0, 255);
  final bi = ((b + m) * 255).round().clamp(0, 255);
  return '#${ri.toRadixString(16).padLeft(2, '0')}'
      '${gi.toRadixString(16).padLeft(2, '0')}'
      '${bi.toRadixString(16).padLeft(2, '0')}';
}

(double, double, double) hexToHsv(String hex) {
  final c = hexToColor(hex);
  final r = c.red / 255, g = c.green / 255, b = c.blue / 255;
  final maxV = [r, g, b].reduce((a, b) => a > b ? a : b);
  final minV = [r, g, b].reduce((a, b) => a < b ? a : b);
  final d = maxV - minV;
  double h = 0;
  if (d != 0) {
    if (maxV == r) {
      h = 60 * (((g - b) / d) % 6);
    } else if (maxV == g) {
      h = 60 * ((b - r) / d + 2);
    } else {
      h = 60 * ((r - g) / d + 4);
    }
  }
  if (h < 0) h += 360;
  final s = maxV == 0 ? 0.0 : d / maxV;
  return (h, s, maxV);
}

/// Device status line, egui status_text(): "On · 55% · 4000K",
/// "Off", "Offline", or "Scene — tap Start to run".
String statusText(Map<String, dynamic> dev) {
  if (dev['online'] != true) return 'Offline';
  if (dev['is_scene'] == true) return 'Scene — tap Start to run';
  final parts = <String>[dev['state'] == true ? 'On' : 'Off'];
  if (dev['state'] == true) {
    final b = dev['brightness_pct'];
    if (b is num) parts.add('${brightnessDeviceToUi(b.toInt())}%');
    if (dev['color_mode'] == 'colour') {
      parts.add('Colour');
    } else if (dev['temp_slider'] is num) {
      parts.add('${tempSliderToKelvin((dev['temp_slider'] as num).toInt())}K');
    }
  }
  return parts.join(' · ');
}

/// Camera status line, egui camera_status_text():
/// "host:port · Stream 1 · High" / "Stream 2 · Standard" / path.
String cameraStatusText(Map<String, dynamic> cam) {
  final port = (cam['port'] is num && (cam['port'] as num) != 0)
      ? (cam['port'] as num).toInt()
      : 554;
  final template = cam['template'] as String? ?? 'tapo1';
  String q;
  if (template == 'custom') {
    final p = (cam['path'] as String? ?? '').trim();
    q = p.isEmpty ? '/' : (p.startsWith('/') ? p : '/$p');
  } else if (template == 'tapo2') {
    q = 'Stream 2 · Standard';
  } else {
    q = 'Stream 1 · High';
  }
  return '${cam['host']}:$port · $q';
}
