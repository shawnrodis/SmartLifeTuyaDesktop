// SmartLife Tuya Desktop \u2014 Windows C++ frontend.
//
// 1:1 REFERENCE (Shawn, 2026-10-27): egui 1.4.0 (tag v1.4.0), the build
// that worked great \u2014 NOT the Flutter port, and NOT v1.4.1 (verified
// by diff: v1.4.0 -> v1.4.1 changed ONLY the renderer Glow -> wgpu,
// no UI or features, so the 1.4.0 UI is the spec). Flutter is only a
// secondary cross-check where egui is silent. Where this file differs
// from 1.4.0, that is a bug to fix, never a shortcut to keep.
// EXCLUDED from 1:1 on purpose: 1.4.0's window trailing and
// moving-to-another-monitor glitches. Those are egui renderer/window
// bugs, not features \u2014 the native Win32 window must drag smooth and
// move monitors cleanly, and that is a pass/fail test, not a copy.
//
// Native Win32 window, custom-painted to match the app's Black theme
// (black background, white text) \u2014 NOT a stock grey dialog. The Tuya
// logic lives in the Rust core (tuya_core.dll), loaded at runtime via
// cpp/core.h; this file owns only the window and controls.
// Tokens/passwords never leave Rust (same presence-marker contract
// as rust/README-API.md).
//
// Milestone 2: login view from egui view_login \u2014 card header, username,
// password, region, platform, keep-me-logged-in, "Log in" button,
// Connection/proxy settings, credentials note, error box, saved-config
// prefill + silent re-login on launch.
//
// KNOWN 1:1 GAP (named, not hidden): egui prefills the password from
// saved creds; the Rust core never exposes a password to any shell
// (Flutter had the same limit), so C++ cannot prefill it either.
// Silent login covers the saved-login case without retyping.

#define UNICODE
#define _UNICODE
#include <windows.h>
// EM_SETCUEBANNER (0x1507) is not declared by the headers on some
// MSVC/SDK combos (build 40466037329 failed on it) \u2014 define it when
// the SDK does not, instead of depending on header quirks.
#ifndef EM_SETCUEBANNER
#define EM_SETCUEBANNER 0x1507
#endif
#include <string>
#include <thread>

#include "core.h"

namespace {

const wchar_t kClassName[] = L"SmartLifeTuyaDesktop";
// Egui window title, with the ampersand (Flutter used a plain title).
const wchar_t kWindowTitle[] = L"SmartLife & Tuya Desktop";

constexpr int kIdUser = 101;
constexpr int kIdPass = 102;
constexpr int kIdRegion = 103;
constexpr int kIdPlatform = 104;
constexpr int kIdKeep = 105;
constexpr int kIdLoginBtn = 106;
constexpr int kIdStatus = 107;
constexpr int kIdProxyToggle = 108;
constexpr int kIdProxy = 109;
constexpr int kIdProxyCustom = 110;
constexpr UINT kMsgLoginDone = WM_USER + 1;

core::Core g_core;
std::wstring g_coreVersion = L"core not loaded";
HBRUSH g_blackBrush = nullptr;
HWND g_user = nullptr, g_pass = nullptr, g_region = nullptr,
     g_platform = nullptr, g_keep = nullptr, g_loginBtn = nullptr,
     g_status = nullptr, g_subtitle = nullptr, g_proxyToggle = nullptr,
     g_proxy = nullptr, g_proxyLabel = nullptr, g_proxyCustom = nullptr,
     g_proxyCustomLabel = nullptr;
bool g_loggingIn = false;
bool g_statusError = false;
bool g_proxyShown = false;
HWND g_mainWnd = nullptr;
HWND g_emoji = nullptr, g_title = nullptr;
HWND g_lblUser = nullptr, g_lblPass = nullptr, g_lblRegion = nullptr,
     g_lblPlatform = nullptr;
HWND g_explainer = nullptr, g_note = nullptr;
HFONT g_fontEmoji = nullptr, g_fontTitle = nullptr, g_fontSubtitle = nullptr,
      g_fontLabel = nullptr, g_fontField = nullptr, g_fontButton = nullptr,
      g_fontNote = nullptr, g_fontProxy = nullptr;
HBRUSH g_cardBrush = nullptr;
RECT g_cardRect = {};
int g_sepY = -1;


// Set the Keep-me-logged-in check state AND repaint it. An
// owner-drawn button does not redraw itself on BM_SETCHECK, so
// without the InvalidateRect the box kept showing its old state:
// prefill could set CHECKED while the box still looked empty, and
// a click that flipped the state left the visual unchanged — the
// user saw a button that "doesn't work", and a click meant to turn
// keep ON actually turned it OFF (wiping saved creds on login).
void SetKeepChecked(bool checked) {
  if (g_keep == nullptr) return;
  SendMessageW(g_keep, BM_SETCHECK, checked ? BST_CHECKED : BST_UNCHECKED, 0);
  InvalidateRect(g_keep, nullptr, TRUE);
  UpdateWindow(g_keep);
}

void LayoutLogin();

void StartLoginRequest(HWND hwnd, const std::string &req);

std::wstring GetEditText(HWND h) {
  const int n = GetWindowTextLengthW(h);
  std::wstring out(n, L'\0');
  GetWindowTextW(h, out.data(), n + 1);
  return out;
}

void SetStatus(const std::wstring &s, bool error) {
  g_statusError = error;
  if (g_status != nullptr) SetWindowTextW(g_status, s.c_str());
  LayoutLogin();
}

void SetLoginEnabled(bool enabled) {
  const BOOL e = enabled ? TRUE : FALSE;
  EnableWindow(g_loginBtn, e);
  EnableWindow(g_user, e);
  EnableWindow(g_pass, e);
  EnableWindow(g_region, e);
  EnableWindow(g_platform, e);
  EnableWindow(g_keep, e);
  EnableWindow(g_proxy, e);
  EnableWindow(g_proxyCustom, e);
}

void AddComboItem(HWND combo, const wchar_t *label, const wchar_t *value) {
  const LRESULT idx = SendMessageW(combo, CB_ADDSTRING, 0,
                                   reinterpret_cast<LPARAM>(label));
  SendMessageW(combo, CB_SETITEMDATA, idx,
               reinterpret_cast<LPARAM>(value));
}

std::wstring ComboValue(HWND combo) {
  const LRESULT idx = SendMessageW(combo, CB_GETCURSEL, 0, 0);
  if (idx == CB_ERR) return L"";
  return reinterpret_cast<const wchar_t *>(
      SendMessageW(combo, CB_GETITEMDATA, idx, 0));
}

void SelectComboValue(HWND combo, const std::string &value) {
  if (value.empty()) return;
  const std::wstring wv = core::Utf8ToWide(value);
  const int count = (int)SendMessageW(combo, CB_GETCOUNT, 0, 0);
  for (int i = 0; i < count; i++) {
    const std::wstring item = reinterpret_cast<const wchar_t *>(
        SendMessageW(combo, CB_GETITEMDATA, i, 0));
    if (item == wv) {
      SendMessageW(combo, CB_SETCURSEL, i, 0);
      break;
    }
  }
}

// Extract the JSON object under "key" (brace-counting, string-aware).
// Returns "" if not found. Used to hand the saved config back to
// save_config with only the proxy prefs changed.
std::string ExtractObject(const std::string &json, const char *key) {
  const std::string pat = std::string("\"") + key + "\":";
  size_t at = json.find(pat);
  if (at == std::string::npos) return "";
  at = json.find('{', at + pat.size());
  if (at == std::string::npos) return "";
  int depth = 0;
  bool inStr = false;
  for (size_t i = at; i < json.size(); i++) {
    const char c = json[i];
    if (inStr) {
      if (c == '\\') {
        i++;
      } else if (c == '"') {
        inStr = false;
      }
    } else if (c == '"') {
      inStr = true;
    } else if (c == '{') {
      depth++;
    } else if (c == '}') {
      if (--depth == 0) return json.substr(at, i - at + 1);
    }
  }
  return "";
}

// Replace "key":"old" with "key":"value" inside an object, or append
// the pair before the closing brace if the key is absent.
std::string SetStringField(std::string obj, const char *key,
                           const std::string &value) {
  const std::string pat = std::string("\"") + key + "\":\"";
  const size_t at = obj.find(pat);
  const std::string pair =
      std::string("\"") + key + "\":\"" + core::JsonEscape(value) + "\"";
  if (at != std::string::npos) {
    const size_t start = at + pat.size();
    const size_t end = obj.find('"', start);
    if (end != std::string::npos) obj.replace(at, end - at, pair);
    return obj;
  }
  const size_t close = obj.rfind('}');
  if (close == std::string::npos) return obj;
  std::string insert = pair;
  // Comma needed unless the object is empty.
  if (obj.find('{') + 1 != close) insert = "," + insert;
  obj.insert(close, insert);
  return obj;
}

void SaveProxyPrefs() {
  // Egui saves proxy prefs immediately on change (self.save()).
  if (g_core.loadConfig == nullptr) return;
  const std::string resp = g_core.Call(g_core.loadConfig);
  std::string cfg = ExtractObject(resp, "config");
  if (cfg.empty()) return;
  std::string prefs = ExtractObject(cfg, "prefs");
  if (prefs.empty()) prefs = "{}";
  prefs = SetStringField(prefs, "proxy_preset",
                         core::WideToUtf8(ComboValue(g_proxy)));
  prefs = SetStringField(prefs, "proxy_custom",
                         core::WideToUtf8(GetEditText(g_proxyCustom)));
  // Swap the prefs object back into the config, then save it whole.
  const std::string oldPrefs = ExtractObject(cfg, "prefs");
  if (!oldPrefs.empty()) {
    const size_t at = cfg.find(oldPrefs);
    if (at != std::string::npos) cfg.replace(at, oldPrefs.size(), prefs);
  }
  // save_config takes the config object; the core preserves stored
  // creds/session on marker objects, so passing the loaded config
  // back unchanged except prefs is safe.
  if (g_core.saveConfig != nullptr) {
    g_core.Call(g_core.saveConfig, cfg);
  }
}

void LayoutLogin();

void UpdateProxyCustomVisibility() {
  LayoutLogin();
}

void ToggleProxy() {
  g_proxyShown = !g_proxyShown;
  SetWindowTextW(g_proxyToggle,
                 g_proxyShown ? L"Connection / proxy settings \u25be"
                              : L"Connection / proxy settings \u25b8");
  LayoutLogin();
}

void LayoutLogin() {
  if (g_mainWnd == nullptr || g_user == nullptr) return;
  RECT rc;
  GetClientRect(g_mainWnd, &rc);
  const int cw = rc.right - rc.left;
  int cardX = (cw - 400) / 2;
  if (cardX < 16) cardX = 16;
  const int x = cardX + 28;
  int y = 36 + 30;  // card top + top margin
  auto place = [&](HWND h, int yy, int hh) {
    if (h != nullptr) MoveWindow(h, x, yy, 344, hh, TRUE);
  };
  place(g_emoji, y, 40); y += 44;
  place(g_title, y, 30); y += 32;
  place(g_subtitle, y, 44); y += 62;  // 44 (2 lines) + 18 gap
  const bool err = g_statusError && GetWindowTextLengthW(g_status) > 0;
  if (err) {
    place(g_status, y, 44);
    ShowWindow(g_status, SW_SHOW);
    y += 56;
  }
  place(g_lblUser, y, 18); y += 18;
  place(g_user, y, 30); y += 40;
  place(g_lblPass, y, 18); y += 18;
  place(g_pass, y, 30); y += 40;
  place(g_lblRegion, y, 18); y += 18;
  place(g_region, y, 200); y += 40;
  place(g_lblPlatform, y, 18); y += 18;
  place(g_platform, y, 200); y += 38;  // 30 + 8 gap
  place(g_keep, y, 24); y += 36;  // 24 + 12 gap
  if (g_loginBtn != nullptr) MoveWindow(g_loginBtn, x, y, 120, 34, TRUE);
  y += 48;  // 34 + 14 gap
  if (!err) {
    if (GetWindowTextLengthW(g_status) > 0) {
      place(g_status, y, 20);
      ShowWindow(g_status, SW_SHOW);
      y += 34;
    } else {
      ShowWindow(g_status, SW_HIDE);
    }
  }
  g_sepY = y;
  y += 13;  // separator line + 12 gap
  if (g_proxyToggle != nullptr) MoveWindow(g_proxyToggle, x, y, 344, 24, TRUE);
  y += 24;
  const int pshow = g_proxyShown ? SW_SHOW : SW_HIDE;
  ShowWindow(g_proxyLabel, pshow);
  ShowWindow(g_proxy, pshow);
  if (g_proxyShown) {
    y += 6;
    place(g_proxyLabel, y, 18); y += 18;
    place(g_proxy, y, 200); y += 40;
    const bool custom = ComboValue(g_proxy) == L"custom";
    ShowWindow(g_proxyCustomLabel, custom ? SW_SHOW : SW_HIDE);
    ShowWindow(g_proxyCustom, custom ? SW_SHOW : SW_HIDE);
    if (custom) {
      place(g_proxyCustomLabel, y, 18); y += 18;
      place(g_proxyCustom, y, 30); y += 40;
    }
    place(g_explainer, y, 72); y += 78;
  } else {
    ShowWindow(g_proxyCustomLabel, SW_HIDE);
    ShowWindow(g_proxyCustom, SW_HIDE);
  }
  y += 10;
  place(g_note, y, 88); y += 88;
  g_cardRect.left = cardX;
  g_cardRect.top = 36;
  g_cardRect.right = cardX + 400;
  g_cardRect.bottom = y + 24;  // bottom margin
  InvalidateRect(g_mainWnd, nullptr, TRUE);
}

void CreateLoginControls(HWND hwnd, HINSTANCE hInstance) {
  auto mkfont = [](int px, bool bold, const wchar_t *face) {
    return CreateFontW(px, 0, 0, 0, bold ? FW_BOLD : FW_NORMAL, FALSE,
                       FALSE, FALSE, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                       CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY,
                       DEFAULT_PITCH, face);
  };
  g_fontEmoji = mkfont(40, false, L"Segoe UI Emoji");
  g_fontTitle = mkfont(23, true, L"Segoe UI");
  g_fontSubtitle = mkfont(-18, false, L"Segoe UI");
  g_fontLabel = mkfont(-17, true, L"Segoe UI");
  g_fontField = mkfont(-19, false, L"Segoe UI");
  g_fontButton = mkfont(-20, false, L"Segoe UI");
  g_fontNote = mkfont(-16, false, L"Segoe UI");
  g_fontProxy = mkfont(-19, false, L"Segoe UI");
  auto label = [&](int id, const wchar_t *text, HFONT font) {
    HWND h = CreateWindowExW(0, L"STATIC", text, WS_CHILD | WS_VISIBLE,
                             0, 0, 0, 0, hwnd, nullptr, hInstance, nullptr);
    SendMessageW(h, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    (void)id;
    return h;
  };
  auto edit = [&](int id, DWORD style) {
    HWND h = CreateWindowExW(0, L"EDIT", L"",
                             WS_CHILD | WS_VISIBLE | WS_BORDER | style, 0, 0,
                             0, 0, hwnd, reinterpret_cast<HMENU>(id),
                             hInstance, nullptr);
    SendMessageW(h, WM_SETFONT, reinterpret_cast<WPARAM>(g_fontField), TRUE);
    return h;
  };
  auto combo = [&](int id) {
    // Height 200 is the dropped-list height, not the control height.
    HWND h = CreateWindowExW(0, L"COMBOBOX", L"",
                             WS_CHILD | WS_VISIBLE | CBS_DROPDOWNLIST, 0, 0,
                             0, 200, hwnd, reinterpret_cast<HMENU>(id),
                             hInstance, nullptr);
    SendMessageW(h, WM_SETFONT, reinterpret_cast<WPARAM>(g_fontField), TRUE);
    return h;
  };
  g_emoji = CreateWindowExW(0, L"STATIC", L"\U0001F3E0",
                            WS_CHILD | WS_VISIBLE | SS_CENTER, 0, 0, 0, 0,
                            hwnd, nullptr, hInstance, nullptr);
  SendMessageW(g_emoji, WM_SETFONT, reinterpret_cast<WPARAM>(g_fontEmoji), TRUE);
  g_title = CreateWindowExW(0, L"STATIC", L"SmartLife & Tuya Desktop",
                            WS_CHILD | WS_VISIBLE | SS_CENTER | SS_NOPREFIX,
                            0, 0, 0, 0, hwnd, nullptr, hInstance, nullptr);
  SendMessageW(g_title, WM_SETFONT, reinterpret_cast<WPARAM>(g_fontTitle), TRUE);
  g_subtitle = CreateWindowExW(
      0, L"STATIC",
      L"Your Smart Life / Tuya devices, in one clean dashboard",
      WS_CHILD | WS_VISIBLE | SS_CENTER, 0, 0, 0, 0, hwnd, nullptr,
      hInstance, nullptr);
  SendMessageW(g_subtitle, WM_SETFONT,
               reinterpret_cast<WPARAM>(g_fontSubtitle), TRUE);
  g_lblUser = label(0, L"Smart Life / Tuya username", g_fontLabel);
  g_user = edit(kIdUser, ES_AUTOHSCROLL);
  SendMessageW(g_user, EM_SETCUEBANNER, TRUE,
               reinterpret_cast<LPARAM>(L"email or phone"));
  g_lblPass = label(0, L"Password", g_fontLabel);
  g_pass = edit(kIdPass, ES_PASSWORD | ES_AUTOHSCROLL);
  SendMessageW(g_pass, EM_SETCUEBANNER, TRUE,
               reinterpret_cast<LPARAM>(L"Your Smart Life password"));
  g_lblRegion = label(0, L"Region (where your account was registered)", g_fontLabel);
  g_region = combo(kIdRegion);
  AddComboItem(g_region, L"United States (US)", L"1");
  AddComboItem(g_region, L"Europe (EU)", L"44");
  AddComboItem(g_region, L"China (CN)", L"86");
  SendMessageW(g_region, CB_SETCURSEL, 0, 0);
  g_lblPlatform = label(0, L"App platform", g_fontLabel);
  g_platform = combo(kIdPlatform);
  AddComboItem(g_platform, L"Smart Life", L"smart_life");
  AddComboItem(g_platform, L"Tuya Smart", L"tuya");
  SendMessageW(g_platform, CB_SETCURSEL, 0, 0);
  // Egui default: unchecked fresh; checked only when saved creds exist
  // (set in PrefillAndSilentLogin).
  g_keep = CreateWindowExW(0, L"BUTTON", L"Keep me logged in on this device",
                           WS_CHILD | WS_VISIBLE | BS_OWNERDRAW, 0, 0, 0,
                           0, hwnd, reinterpret_cast<HMENU>(kIdKeep),
                           hInstance, nullptr);
  SendMessageW(g_keep, WM_SETFONT, reinterpret_cast<WPARAM>(g_fontField), TRUE);
  g_loginBtn = CreateWindowExW(0, L"BUTTON", L"Log in",
                               WS_CHILD | WS_VISIBLE | BS_OWNERDRAW, 0, 0, 0,
                               0, hwnd, reinterpret_cast<HMENU>(kIdLoginBtn),
                               hInstance, nullptr);
  g_status = CreateWindowExW(0, L"STATIC", L"", WS_CHILD | WS_VISIBLE, 0, 0,
                             0, 0, hwnd, reinterpret_cast<HMENU>(kIdStatus),
                             hInstance, nullptr);
  SendMessageW(g_status, WM_SETFONT, reinterpret_cast<WPARAM>(g_fontNote), TRUE);
  g_proxyToggle = CreateWindowExW(
      0, L"BUTTON", L"Connection / proxy settings \u25B8",
      WS_CHILD | WS_VISIBLE | BS_OWNERDRAW, 0, 0, 0, 0, hwnd,
      reinterpret_cast<HMENU>(kIdProxyToggle), hInstance, nullptr);
  g_proxyLabel = label(0, L"CORS proxy", g_fontLabel);
  g_proxy = combo(kIdProxy);
  AddComboItem(g_proxy, L"Auto \u2014 direct for US/EU, community proxy for CN (recommended)", L"auto");
  AddComboItem(g_proxy, L"SmartAtHome community proxy (all requests)", L"smartathome");
  AddComboItem(g_proxy, L"corsproxy.io (encoded URL)", L"corsproxy");
  AddComboItem(g_proxy, L"cors-anywhere.herokuapp.com (may need activation)", L"cors-anywhere");
  AddComboItem(g_proxy, L"None \u2014 direct connection", L"none");
  AddComboItem(g_proxy, L"Custom\u2026", L"custom");
  SendMessageW(g_proxy, CB_SETCURSEL, 0, 0);
  g_proxyCustomLabel = label(0, L"Custom proxy prefix", g_fontLabel);
  g_proxyCustom = edit(kIdProxyCustom, ES_AUTOHSCROLL);
  SendMessageW(g_proxyCustom, EM_SETCUEBANNER, TRUE,
               reinterpret_cast<LPARAM>(
                   L"https://my-proxy.example/ or \u2026?url={url}"));
  g_explainer = CreateWindowExW(
      0, L"STATIC",
      L"Most regions connect directly. A proxy is only needed where Tuya's servers don't accept direct browser/app calls (China region, some networks). See Help in Settings.",
      WS_CHILD | WS_VISIBLE, 0, 0, 0, 0, hwnd, nullptr, hInstance, nullptr);
  SendMessageW(g_explainer, WM_SETFONT, reinterpret_cast<WPARAM>(g_fontNote), TRUE);
  g_note = CreateWindowExW(
      0, L"STATIC",
      L"Your credentials are only sent to Tuya's own servers and are stored only in this app's config on this device (and only if you tick \u201CKeep me logged in\u201D). Nothing is sent anywhere else.",
      WS_CHILD | WS_VISIBLE, 0, 0, 0, 0, hwnd, nullptr, hInstance, nullptr);
  SendMessageW(g_note, WM_SETFONT, reinterpret_cast<WPARAM>(g_fontNote), TRUE);
  LayoutLogin();
}

void DoLogin(HWND hwnd) {
  if (g_loggingIn || g_core.login == nullptr) return;
  const std::string user = core::WideToUtf8(GetEditText(g_user));
  const std::string pass = core::WideToUtf8(GetEditText(g_pass));
  const std::string region = core::WideToUtf8(ComboValue(g_region));
  const std::string platform = core::WideToUtf8(ComboValue(g_platform));
  const bool keep =
      SendMessageW(g_keep, BM_GETCHECK, 0, 0) == BST_CHECKED;
  if (user.empty() || pass.empty()) {
    SetStatus(L"Enter your username and password.", true);
    return;
  }
  const std::string req = "{\"username\":\"" + core::JsonEscape(user) +
                          "\",\"password\":\"" + core::JsonEscape(pass) +
                          "\",\"region\":\"" + core::JsonEscape(region) +
                          "\",\"platform\":\"" +
                          core::JsonEscape(platform) +
                          "\",\"keep\":" + (keep ? "true" : "false") + "}";
  StartLoginRequest(hwnd, req);
  g_loggingIn = true;
  SetLoginEnabled(false);
  SetStatus(L"Signing in\u2026", false);
}

void StartLoginRequest(HWND hwnd, const std::string &req) {
  // Network on a worker thread; result comes back as kMsgLoginDone
  // with a heap string the window procedure frees.
  std::thread([hwnd, req]() {
    std::string *resp = new std::string(g_core.Call(g_core.login, req));
    PostMessageW(hwnd, kMsgLoginDone, 0, reinterpret_cast<LPARAM>(resp));
  }).detach();
}

// Prefill from saved config + silent re-login, like egui on launch.
// load_config returns the saved user_name / country_code / platform
// (no password, no token \u2014 see the gap note at the top of this file).
void PrefillAndSilentLogin(HWND hwnd) {
  if (g_core.loadConfig == nullptr) return;
  const std::string cfg = g_core.Call(g_core.loadConfig);
  if (cfg.empty()) return;
  const std::string user = core::JsonString(cfg, "user_name");
  const std::string region = core::JsonString(cfg, "country_code");
  const std::string platform = core::JsonString(cfg, "platform");
  const std::string proxy = core::JsonString(cfg, "proxy_preset");
  const std::string proxyCustom = core::JsonString(cfg, "proxy_custom");
  if (!user.empty()) {
    SetWindowTextW(g_user, core::Utf8ToWide(user).c_str());
    // Saved creds exist \u2192 egui checks Keep me logged in.
    SetKeepChecked(true);
  }
  SelectComboValue(g_region, region);
  SelectComboValue(g_platform, platform);
  SelectComboValue(g_proxy, proxy);
  if (!proxyCustom.empty()) {
    SetWindowTextW(g_proxyCustom,
                   core::Utf8ToWide(proxyCustom).c_str());
  }
  if (!user.empty() || !region.empty()) {
    StartLoginRequest(hwnd, "{\"silent\":true}");
  }
}

void OnLoginDone(const std::string &resp) {
  g_loggingIn = false;
  SetLoginEnabled(true);
  if (resp.empty()) {
    SetStatus(L"Login failed \u2014 no response from the core.", true);
    return;
  }
  if (core::JsonBool(resp, "ok", false)) {
    // Dashboard milestone parses devices; for now, confirm sign-in.
    SetStatus(L"Signed in. Dashboard is the next milestone.", false);
  } else {
    std::string err = core::JsonString(resp, "error");
    if (err.empty()) {
      err = "Login failed \u2014 check username, password and region.";
    }
    SetStatus(core::Utf8ToWide(err), true);
  }
}

LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam) {
  switch (msg) {
    case WM_PAINT: {
      PAINTSTRUCT ps;
      HDC hdc = BeginPaint(hwnd, &ps);
      RECT rc;
      GetClientRect(hwnd, &rc);
      FillRect(hdc, &rc, g_blackBrush);
      if (g_cardRect.right > g_cardRect.left) {
        HBRUSH card = g_cardBrush != nullptr ? g_cardBrush : g_blackBrush;
        HPEN pen = CreatePen(PS_SOLID, 1, RGB(0x1f, 0x23, 0x2e));
        HGDIOBJ oldBrush = SelectObject(hdc, card);
        HGDIOBJ oldPen = SelectObject(hdc, pen);
        RoundRect(hdc, g_cardRect.left, g_cardRect.top, g_cardRect.right,
                  g_cardRect.bottom, 44, 44);
        SelectObject(hdc, oldBrush);
        SelectObject(hdc, oldPen);
        DeleteObject(pen);
        if (g_sepY >= 0) {
          HPEN sep = CreatePen(PS_SOLID, 1, RGB(0x1f, 0x23, 0x2e));
          HGDIOBJ oldSep = SelectObject(hdc, sep);
          MoveToEx(hdc, g_cardRect.left + 28, g_sepY, nullptr);
          LineTo(hdc, g_cardRect.right - 28, g_sepY);
          SelectObject(hdc, oldSep);
          DeleteObject(sep);
        }
      }
      EndPaint(hwnd, &ps);
      return 0;
    }
    case WM_SIZE: {
      LayoutLogin();
      return 0;
    }
    case WM_DRAWITEM: {
      LPDRAWITEMSTRUCT dis = reinterpret_cast<LPDRAWITEMSTRUCT>(lParam);
      if (dis->CtlID == kIdLoginBtn) {
        const bool disabled = (dis->itemState & ODS_DISABLED) != 0;
        HBRUSH fill = CreateSolidBrush(disabled ? RGB(0x5b, 0x64, 0x78)
                                                : RGB(0x63, 0x66, 0xf1));
        HPEN pen = CreatePen(PS_SOLID, 1, disabled ? RGB(0x5b, 0x64, 0x78)
                                                  : RGB(0x63, 0x66, 0xf1));
        HGDIOBJ oldBrush = SelectObject(dis->hDC, fill);
        HGDIOBJ oldPen = SelectObject(dis->hDC, pen);
        RoundRect(dis->hDC, dis->rcItem.left, dis->rcItem.top,
                  dis->rcItem.right, dis->rcItem.bottom, 16, 16);
        SelectObject(dis->hDC, oldBrush);
        SelectObject(dis->hDC, oldPen);
        DeleteObject(fill);
        DeleteObject(pen);
        SetBkMode(dis->hDC, TRANSPARENT);
        SetTextColor(dis->hDC, RGB(0xff, 0xff, 0xff));
        if (g_fontButton != nullptr)
          SelectObject(dis->hDC, g_fontButton);
        wchar_t text[64];
        GetWindowTextW(dis->hwndItem, text, 64);
        DrawTextW(dis->hDC, text, -1, &dis->rcItem,
                  DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        return TRUE;
      }
      if (dis->CtlID == kIdProxyToggle) {
        HBRUSH fill = g_cardBrush != nullptr ? g_cardBrush : g_blackBrush;
        FillRect(dis->hDC, &dis->rcItem, fill);
        SetBkMode(dis->hDC, TRANSPARENT);
        SetTextColor(dis->hDC, RGB(0xe7, 0xeb, 0xf2));
        if (g_fontProxy != nullptr)
          SelectObject(dis->hDC, g_fontProxy);
        wchar_t text[128];
        GetWindowTextW(dis->hwndItem, text, 128);
        DrawTextW(dis->hDC, text, -1, &dis->rcItem,
                  DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        return TRUE;
      }
      if (dis->CtlID == kIdKeep) {
        HBRUSH fill = g_cardBrush != nullptr ? g_cardBrush : g_blackBrush;
        FillRect(dis->hDC, &dis->rcItem, fill);
        const int top = dis->rcItem.top + (dis->rcItem.bottom - dis->rcItem.top - 16) / 2;
        RECT box = {dis->rcItem.left, top, dis->rcItem.left + 16, top + 16};
        HPEN pen = CreatePen(PS_SOLID, 1, RGB(0x8b, 0x94, 0xa7));
        HGDIOBJ oldPen = SelectObject(dis->hDC, pen);
        HGDIOBJ oldBrush = SelectObject(dis->hDC, GetStockObject(NULL_BRUSH));
        Rectangle(dis->hDC, box.left, box.top, box.right, box.bottom);
        SelectObject(dis->hDC, oldPen);
        SelectObject(dis->hDC, oldBrush);
        DeleteObject(pen);
        if (SendMessageW(dis->hwndItem, BM_GETCHECK, 0, 0) == BST_CHECKED) {
          SetBkMode(dis->hDC, TRANSPARENT);
          SetTextColor(dis->hDC, RGB(0xe7, 0xeb, 0xf2));
          if (g_fontField != nullptr) SelectObject(dis->hDC, g_fontField);
          DrawTextW(dis->hDC, L"\u2713", 1, &box, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        SetBkMode(dis->hDC, TRANSPARENT);
        SetTextColor(dis->hDC, RGB(0xe7, 0xeb, 0xf2));
        if (g_fontField != nullptr) SelectObject(dis->hDC, g_fontField);
        RECT tr = dis->rcItem;
        tr.left += 24;
        wchar_t text[128];
        GetWindowTextW(dis->hwndItem, text, 128);
        DrawTextW(dis->hDC, text, -1, &tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        return TRUE;
      }
      return DefWindowProcW(hwnd, msg, wParam, lParam);
    }
    case WM_CTLCOLORSTATIC:
    case WM_CTLCOLOREDIT:
    case WM_CTLCOLORBTN:
    case WM_CTLCOLORLISTBOX: {
      // Black theme, egui 1.4.0: text #e7ebf2, dim #8b94a7, error
      // rose #fb7185, all on the card brush inside the login card.
      HDC hdc = reinterpret_cast<HDC>(wParam);
      HWND ctl = reinterpret_cast<HWND>(lParam);
      if (ctl == g_title) {
        SetTextColor(hdc, RGB(0xe7, 0xeb, 0xf2));
      } else if (ctl == g_status && g_statusError) {
        SetTextColor(hdc, RGB(251, 113, 133));
      } else if (ctl == g_status || ctl == g_keep || msg == WM_CTLCOLOREDIT ||
                 msg == WM_CTLCOLORLISTBOX) {
        SetTextColor(hdc, RGB(0xe7, 0xeb, 0xf2));
      } else {
        SetTextColor(hdc, RGB(0x8b, 0x94, 0xa7));
      }
      const COLORREF bk = 0x110d0c;  // card #0c0d11, as a COLORREF
      SetBkColor(hdc, bk);
      return reinterpret_cast<LRESULT>(
          g_cardBrush != nullptr ? g_cardBrush : g_blackBrush);
    }
    case WM_COMMAND: {
      const int id = LOWORD(wParam);
      const int code = HIWORD(wParam);
      if (id == kIdLoginBtn && code == BN_CLICKED) {
        DoLogin(hwnd);
      } else if (id == kIdKeep && code == BN_CLICKED) {
        const bool checked =
            SendMessageW(g_keep, BM_GETCHECK, 0, 0) == BST_CHECKED;
        SetKeepChecked(!checked);
      } else if (id == kIdProxyToggle && code == BN_CLICKED) {
        ToggleProxy();
      } else if (id == kIdProxy && code == CBN_SELCHANGE) {
        UpdateProxyCustomVisibility();
        SaveProxyPrefs();
      } else if (id == kIdProxyCustom && code == EN_CHANGE) {
        SaveProxyPrefs();
      }
      return 0;
    }
    case kMsgLoginDone: {
      std::string *resp = reinterpret_cast<std::string *>(lParam);
      if (resp != nullptr) {
        OnLoginDone(*resp);
        delete resp;
      }
      return 0;
    }
    case WM_DESTROY:
      PostQuitMessage(0);
      return 0;
    default:
      return DefWindowProcW(hwnd, msg, wParam, lParam);
  }
}

}  // namespace

int WINAPI wWinMain(HINSTANCE hInstance, HINSTANCE, PWSTR, int nCmdShow) {
  if (g_core.Load()) {
    g_coreVersion = core::Utf8ToWide(g_core.Call(g_core.version));
    if (g_coreVersion.empty()) g_coreVersion = L"core loaded, no version";
  } else {
    g_coreVersion = L"tuya_core.dll not found next to the exe";
  }
  g_blackBrush = CreateSolidBrush(RGB(0, 0, 0));
  g_cardBrush = CreateSolidBrush(RGB(0x0c, 0x0d, 0x11));

  WNDCLASSW wc = {};
  wc.lpfnWndProc = WndProc;
  wc.hInstance = hInstance;
  wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
  // Icon from the embedded resource (cpp/app.rc) \u2014 the taskbar icon.
  wc.hIcon = LoadIconW(hInstance, MAKEINTRESOURCEW(1));
  wc.lpszClassName = kClassName;
  wc.hbrBackground = g_blackBrush;
  RegisterClassW(&wc);

  HWND hwnd = CreateWindowExW(
      0, kClassName, kWindowTitle,
      WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 960,
      940, nullptr, nullptr, hInstance, nullptr);
  if (hwnd == nullptr) return 1;
  g_mainWnd = hwnd;
  CreateLoginControls(hwnd, hInstance);
  PrefillAndSilentLogin(hwnd);
  ShowWindow(hwnd, nCmdShow);
  UpdateWindow(hwnd);

  MSG msg;
  while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
    // Owner-drawn Log in is no longer a default pushbutton, so Enter
    // in a login field must submit explicitly (egui submits on Enter).
    if (msg.message == WM_KEYDOWN && msg.wParam == VK_RETURN) {
      HWND focus = GetFocus();
      if (focus == g_user || focus == g_pass || focus == g_region ||
          focus == g_platform || focus == g_proxyCustom) {
        DoLogin(hwnd);
        continue;
      }
    }
    TranslateMessage(&msg);
    DispatchMessageW(&msg);
  }
  return (int)msg.wParam;
}
