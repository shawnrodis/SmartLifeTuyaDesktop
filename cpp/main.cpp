// SmartLife Tuya Desktop — Windows C++ frontend.
//
// 1:1 REFERENCE (Shawn, 2026-10-27): egui 1.4.0 (tag v1.4.0), the build
// that worked great — NOT the Flutter port, and NOT v1.4.1 (verified
// by diff: v1.4.0 -> v1.4.1 changed ONLY the renderer Glow -> wgpu,
// no UI or features, so the 1.4.0 UI is the spec). Flutter is only a
// secondary cross-check where egui is silent. Where this file differs
// from 1.4.0, that is a bug to fix, never a shortcut to keep.
// EXCLUDED from 1:1 on purpose: 1.4.0's window trailing and
// moving-to-another-monitor glitches. Those are egui renderer/window
// bugs, not features — the native Win32 window must drag smooth and
// move monitors cleanly, and that is a pass/fail test, not a copy.
//
// Native Win32 window, custom-painted to match the app's Black theme
// (black background, white text) — NOT a stock grey dialog. The Tuya
// logic lives in the Rust core (tuya_core.dll), loaded at runtime via
// cpp/core.h; this file owns only the window and controls.
// Tokens/passwords never leave Rust (same presence-marker contract
// as rust/README-API.md).
//
// Milestone 2: login view from egui view_login — card header, username,
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

void UpdateProxyCustomVisibility() {
  const bool custom = ComboValue(g_proxy) == L"custom";
  ShowWindow(g_proxyCustomLabel, custom ? SW_SHOW : SW_HIDE);
  ShowWindow(g_proxyCustom, custom ? SW_SHOW : SW_HIDE);
}

void ToggleProxy() {
  g_proxyShown = !g_proxyShown;
  const int show = g_proxyShown ? SW_SHOW : SW_HIDE;
  ShowWindow(g_proxyLabel, show);
  ShowWindow(g_proxy, show);
  UpdateProxyCustomVisibility();
  SetWindowTextW(g_proxyToggle,
                 g_proxyShown ? L"Connection / proxy settings ▾"
                              : L"Connection / proxy settings ▸");
}

void CreateLoginControls(HWND hwnd, HINSTANCE hInstance) {
  auto label = [&](int y, const wchar_t *text) {
    return CreateWindowExW(0, L"STATIC", text, WS_CHILD | WS_VISIBLE, 16, y,
                           600, 20, hwnd, nullptr, hInstance, nullptr);
  };
  auto edit = [&](int id, int y, DWORD style) {
    return CreateWindowExW(0, L"EDIT", L"",
                           WS_CHILD | WS_VISIBLE | WS_BORDER | style, 16,
                           y, 400, 24, hwnd, reinterpret_cast<HMENU>(id),
                           hInstance, nullptr);
  };
  auto combo = [&](int id, int y) {
    // Dropdown height 200: for a dropdown list this is the dropped
    // list height, not the control height (24 would clip the list).
    return CreateWindowExW(0, L"COMBOBOX", L"",
                           WS_CHILD | WS_VISIBLE | CBS_DROPDOWNLIST, 16, y,
                           400, 200, hwnd, reinterpret_cast<HMENU>(id),
                           hInstance, nullptr);
  };
  // Egui login card header: house, app title, subtitle.
  {
    HWND title = CreateWindowExW(0, L"STATIC",
                                 L"🏠 SmartLife & Tuya Desktop",
                                 WS_CHILD | WS_VISIBLE | SS_NOPREFIX, 16, 16,
                                 600, 30, hwnd, nullptr, hInstance, nullptr);
    HFONT bold = CreateFontW(23, 0, 0, 0, FW_BOLD, FALSE, FALSE, FALSE,
                             DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                             CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY,
                             DEFAULT_PITCH, L"Segoe UI");
    SendMessageW(title, WM_SETFONT, reinterpret_cast<WPARAM>(bold), TRUE);
    g_subtitle = CreateWindowExW(
        0, L"STATIC",
        L"Your Smart Life / Tuya devices, in one clean dashboard",
        WS_CHILD | WS_VISIBLE, 16, 50, 600, 20, hwnd, nullptr, hInstance,
        nullptr);
  }
  label(84, L"Smart Life / Tuya username");
  g_user = edit(kIdUser, 106, ES_AUTOHSCROLL);
  SendMessageW(g_user, EM_SETCUEBANNER, TRUE,
               reinterpret_cast<LPARAM>(L"email or phone"));
  label(138, L"Password");
  g_pass = edit(kIdPass, 160, ES_PASSWORD | ES_AUTOHSCROLL);
  SendMessageW(g_pass, EM_SETCUEBANNER, TRUE,
               reinterpret_cast<LPARAM>(L"Your Smart Life password"));
  label(192, L"Region (where your account was registered)");
  g_region = combo(kIdRegion, 214);
  AddComboItem(g_region, L"United States (US)", L"1");
  AddComboItem(g_region, L"Europe (EU)", L"44");
  AddComboItem(g_region, L"China (CN)", L"86");
  SendMessageW(g_region, CB_SETCURSEL, 0, 0);
  label(248, L"App platform");
  g_platform = combo(kIdPlatform, 270);
  AddComboItem(g_platform, L"Smart Life", L"smart_life");
  AddComboItem(g_platform, L"Tuya Smart", L"tuya");
  SendMessageW(g_platform, CB_SETCURSEL, 0, 0);
  // Egui default: unchecked fresh; checked only when saved creds exist
  // (set in PrefillAndSilentLogin).
  g_keep = CreateWindowExW(0, L"BUTTON", L"Keep me logged in on this device",
                           WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX, 16, 306,
                           400, 24, hwnd, reinterpret_cast<HMENU>(kIdKeep),
                           hInstance, nullptr);
  g_loginBtn = CreateWindowExW(0, L"BUTTON", L"Log in",
                               WS_CHILD | WS_VISIBLE | BS_DEFPUSHBUTTON, 16,
                               338, 120, 30, hwnd,
                               reinterpret_cast<HMENU>(kIdLoginBtn), hInstance,
                               nullptr);
  g_status = CreateWindowExW(0, L"STATIC", L"", WS_CHILD | WS_VISIBLE, 16,
                             378, 600, 32, hwnd,
                             reinterpret_cast<HMENU>(kIdStatus), hInstance,
                             nullptr);
  g_proxyToggle = CreateWindowExW(
      0, L"BUTTON", L"Connection / proxy settings ▸",
      WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON, 16, 420, 300, 24, hwnd,
      reinterpret_cast<HMENU>(kIdProxyToggle), hInstance, nullptr);
  // Proxy controls start hidden (egui CollapsingHeader, closed).
  g_proxyLabel = label(452, L"CORS proxy");
  g_proxy = combo(kIdProxy, 474);
  AddComboItem(g_proxy, L"Auto — direct for US/EU, community proxy for CN (recommended)", L"auto");
  AddComboItem(g_proxy, L"SmartAtHome community proxy (all requests)", L"smartathome");
  AddComboItem(g_proxy, L"corsproxy.io (encoded URL)", L"corsproxy");
  AddComboItem(g_proxy, L"cors-anywhere.herokuapp.com (may need activation)", L"cors-anywhere");
  AddComboItem(g_proxy, L"None — direct connection", L"none");
  AddComboItem(g_proxy, L"Custom…", L"custom");
  SendMessageW(g_proxy, CB_SETCURSEL, 0, 0);
  ShowWindow(g_proxyLabel, SW_HIDE);
  ShowWindow(g_proxy, SW_HIDE);
  g_proxyCustomLabel = label(506, L"Custom proxy prefix");
  g_proxyCustom = edit(kIdProxyCustom, 528, ES_AUTOHSCROLL);
  SendMessageW(g_proxyCustom, EM_SETCUEBANNER, TRUE,
               reinterpret_cast<LPARAM>(
                   L"https://my-proxy.example/ or …?url={url}"));
  ShowWindow(g_proxyCustomLabel, SW_HIDE);
  ShowWindow(g_proxyCustom, SW_HIDE);
  // Egui explanatory texts, verbatim in substance.
  CreateWindowExW(0, L"STATIC",
                  L"Most regions connect directly. A proxy is only needed where Tuya's servers don't accept direct browser/app calls (China region, some networks). See Help in Settings.",
                  WS_CHILD | WS_VISIBLE, 16, 560, 600, 40, hwnd, nullptr,
                  hInstance, nullptr);
  CreateWindowExW(0, L"STATIC",
                  L"Your credentials are only sent to Tuya's own servers and are stored only in this app's config on this device (and only if you tick “Keep me logged in”). Nothing is sent anywhere else.",
                  WS_CHILD | WS_VISIBLE, 16, 612, 600, 40, hwnd, nullptr,
                  hInstance, nullptr);
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
  SetStatus(L"Signing in…", false);
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
// (no password, no token — see the gap note at the top of this file).
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
    // Saved creds exist → egui checks Keep me logged in.
    SendMessageW(g_keep, BM_SETCHECK, BST_CHECKED, 0);
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
    SetStatus(L"Login failed — no response from the core.", true);
    return;
  }
  if (core::JsonBool(resp, "ok", false)) {
    // Dashboard milestone parses devices; for now, confirm sign-in.
    SetStatus(L"Signed in. Dashboard is the next milestone.", false);
  } else {
    std::string err = core::JsonString(resp, "error");
    if (err.empty()) {
      err = "Login failed — check username, password and region.";
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
      EndPaint(hwnd, &ps);
      return 0;
    }
    case WM_CTLCOLORSTATIC:
    case WM_CTLCOLOREDIT:
    case WM_CTLCOLORBTN: {
      // Black theme: white text on the window's black brush, except
      // the subtitle (dim grey) and a login error (egui rose).
      HDC hdc = reinterpret_cast<HDC>(wParam);
      HWND ctl = reinterpret_cast<HWND>(lParam);
      if (ctl == g_subtitle) {
        SetTextColor(hdc, RGB(180, 180, 180));
      } else if (ctl == g_status && g_statusError) {
        SetTextColor(hdc, RGB(251, 113, 133));
      } else {
        SetTextColor(hdc, RGB(255, 255, 255));
      }
      SetBkColor(hdc, RGB(0, 0, 0));
      return reinterpret_cast<LRESULT>(g_blackBrush);
    }
    case WM_COMMAND: {
      const int id = LOWORD(wParam);
      const int code = HIWORD(wParam);
      if (id == kIdLoginBtn && code == BN_CLICKED) {
        DoLogin(hwnd);
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

  WNDCLASSW wc = {};
  wc.lpfnWndProc = WndProc;
  wc.hInstance = hInstance;
  wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
  // Icon from the embedded resource (cpp/app.rc) — the taskbar icon.
  wc.hIcon = LoadIconW(hInstance, MAKEINTRESOURCEW(1));
  wc.lpszClassName = kClassName;
  wc.hbrBackground = g_blackBrush;
  RegisterClassW(&wc);

  HWND hwnd = CreateWindowExW(
      0, kClassName, kWindowTitle,
      WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 960,
      760, nullptr, nullptr, hInstance, nullptr);
  if (hwnd == nullptr) return 1;
  CreateLoginControls(hwnd, hInstance);
  PrefillAndSilentLogin(hwnd);
  ShowWindow(hwnd, nCmdShow);
  UpdateWindow(hwnd);

  MSG msg;
  while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
    TranslateMessage(&msg);
    DispatchMessageW(&msg);
  }
  return (int)msg.wParam;
}
