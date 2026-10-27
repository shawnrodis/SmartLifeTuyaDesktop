// SmartLife Tuya Desktop — Windows C++ frontend.
//
// Native Win32 window, custom-painted to match the app's Black theme
// (black background, white text) — NOT a stock grey dialog. The Tuya
// logic lives in the Rust core (tuya_core.dll), loaded at runtime via
// cpp/core.h; this file owns only the window and controls.
// Tokens/passwords never leave Rust (same presence-marker contract
// as rust/README-API.md).
//
// Milestone 2: login view (username, password, region, platform,
// keep-me-logged-in) calling tuya_login on a worker thread, so the
// window never freezes on the network call. Dashboard is next.

#define UNICODE
#define _UNICODE
#include <windows.h>
#include <string>
#include <thread>

#include "core.h"

namespace {

const wchar_t kClassName[] = L"SmartLifeTuyaDesktop";
const wchar_t kWindowTitle[] = L"SmartLife Tuya Desktop";

constexpr int kIdUser = 101;
constexpr int kIdPass = 102;
constexpr int kIdRegion = 103;
constexpr int kIdPlatform = 104;
constexpr int kIdKeep = 105;
constexpr int kIdLoginBtn = 106;
constexpr int kIdStatus = 107;
constexpr UINT kMsgLoginDone = WM_USER + 1;

core::Core g_core;
std::wstring g_coreVersion = L"core not loaded";
HBRUSH g_blackBrush = nullptr;
HWND g_user = nullptr, g_pass = nullptr, g_region = nullptr,
     g_platform = nullptr, g_keep = nullptr, g_loginBtn = nullptr,
     g_status = nullptr;
bool g_loggingIn = false;

std::wstring GetEditText(HWND h) {
  const int n = GetWindowTextLengthW(h);
  std::wstring out(n, L'\0');
  GetWindowTextW(h, out.data(), n + 1);
  return out;
}

void SetStatus(const std::wstring &s) {
  if (g_status != nullptr) SetWindowTextW(g_status, s.c_str());
}

void SetLoginEnabled(bool enabled) {
  EnableWindow(g_loginBtn, enabled ? TRUE : FALSE);
  EnableWindow(g_user, enabled ? TRUE : FALSE);
  EnableWindow(g_pass, enabled ? TRUE : FALSE);
  EnableWindow(g_region, enabled ? TRUE : FALSE);
  EnableWindow(g_platform, enabled ? TRUE : FALSE);
  EnableWindow(g_keep, enabled ? TRUE : FALSE);
}

// ComboBox item data holds the core value (region code / platform id).
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

void CreateLoginControls(HWND hwnd, HINSTANCE hInstance) {
  // Labels + fields, left-aligned like the egui/Flutter login form.
  // Region values are country calling codes (egui REGIONS); platform
  // values are smart_life / tuya (egui PLATFORMS).
  auto label = [&](int y, const wchar_t *text) {
    CreateWindowExW(0, L"STATIC", text, WS_CHILD | WS_VISIBLE, 16, y, 400,
                    20, hwnd, nullptr, hInstance, nullptr);
  };
  auto edit = [&](int id, int y, DWORD style) {
    return CreateWindowExW(0, L"EDIT", L"",
                           WS_CHILD | WS_VISIBLE | WS_BORDER | style, 16,
                           y, 400, 24, hwnd, reinterpret_cast<HMENU>(id),
                           hInstance, nullptr);
  };
  label(72, L"Username");
  g_user = edit(kIdUser, 94, ES_AUTOHSCROLL);
  label(126, L"Password");
  g_pass = edit(kIdPass, 148, ES_PASSWORD | ES_AUTOHSCROLL);
  label(182, L"Region (where your account was registered)");
  g_region = CreateWindowExW(0, L"COMBOBOX", L"",
                             WS_CHILD | WS_VISIBLE | CBS_DROPDOWNLIST, 16,
                             204, 400, 24, hwnd,
                             reinterpret_cast<HMENU>(kIdRegion), hInstance,
                             nullptr);
  AddComboItem(g_region, L"United States (US)", L"1");
  AddComboItem(g_region, L"Europe (EU)", L"44");
  AddComboItem(g_region, L"China (CN)", L"86");
  SendMessageW(g_region, CB_SETCURSEL, 0, 0);
  label(238, L"App platform");
  g_platform = CreateWindowExW(0, L"COMBOBOX", L"",
                               WS_CHILD | WS_VISIBLE | CBS_DROPDOWNLIST, 16,
                               260, 400, 24, hwnd,
                               reinterpret_cast<HMENU>(kIdPlatform),
                               hInstance, nullptr);
  AddComboItem(g_platform, L"Smart Life", L"smart_life");
  AddComboItem(g_platform, L"Tuya Smart", L"tuya");
  SendMessageW(g_platform, CB_SETCURSEL, 0, 0);
  g_keep = CreateWindowExW(0, L"BUTTON", L"Keep me logged in on this device",
                           WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX, 16, 296,
                           400, 24, hwnd, reinterpret_cast<HMENU>(kIdKeep),
                           hInstance, nullptr);
  SendMessageW(g_keep, BM_SETCHECK, BST_CHECKED, 0);
  g_loginBtn = CreateWindowExW(0, L"BUTTON", L"Login",
                               WS_CHILD | WS_VISIBLE | BS_DEFPUSHBUTTON, 16,
                               330, 120, 30, hwnd,
                               reinterpret_cast<HMENU>(kIdLoginBtn), hInstance,
                               nullptr);
  g_status = CreateWindowExW(0, L"STATIC", L"", WS_CHILD | WS_VISIBLE, 16,
                             372, 600, 40, hwnd,
                             reinterpret_cast<HMENU>(kIdStatus), hInstance,
                             nullptr);
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
    SetStatus(L"Enter your username and password.");
    return;
  }
  g_loggingIn = true;
  SetLoginEnabled(false);
  SetStatus(L"Signing in…");
  // Network on a worker thread; result comes back as kMsgLoginDone
  // with a heap string the window procedure frees.
  std::thread([hwnd, user, pass, region, platform, keep]() {
    const std::string req = "{\"username\":\"" + core::JsonEscape(user) +
                            "\",\"password\":\"" + core::JsonEscape(pass) +
                            "\",\"region\":\"" + core::JsonEscape(region) +
                            "\",\"platform\":\"" +
                            core::JsonEscape(platform) +
                            "\",\"keep\":" + (keep ? "true" : "false") + "}";
    std::string *resp = new std::string(g_core.Call(g_core.login, req));
    PostMessageW(hwnd, kMsgLoginDone, 0, reinterpret_cast<LPARAM>(resp));
  }).detach();
}

void OnLoginDone(const std::string &resp) {
  g_loggingIn = false;
  SetLoginEnabled(true);
  if (resp.empty()) {
    SetStatus(L"Login failed — no response from the core.");
    return;
  }
  if (core::JsonBool(resp, "ok", false)) {
    // Dashboard milestone parses devices; for now, confirm sign-in.
    SetStatus(L"Signed in. Dashboard is the next milestone.");
  } else {
    std::string err = core::JsonString(resp, "error");
    if (err.empty()) err = "Login failed — check username, password and region.";
    SetStatus(core::Utf8ToWide(err));
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
      SetBkMode(hdc, TRANSPARENT);
      SetTextColor(hdc, RGB(255, 255, 255));
      const std::wstring line1 = L"SmartLife Tuya Desktop";
      const std::wstring line2 = L"Core: " + g_coreVersion;
      TextOutW(hdc, 16, 16, line1.c_str(), (int)line1.size());
      SetTextColor(hdc, RGB(180, 180, 180));
      TextOutW(hdc, 16, 44, line2.c_str(), (int)line2.size());
      EndPaint(hwnd, &ps);
      return 0;
    }
    case WM_CTLCOLORSTATIC:
    case WM_CTLCOLOREDIT:
    case WM_CTLCOLORBTN: {
      // Black theme for labels, fields and checkboxes: white text on
      // the same black brush as the window background.
      HDC hdc = reinterpret_cast<HDC>(wParam);
      SetTextColor(hdc, RGB(255, 255, 255));
      SetBkColor(hdc, RGB(0, 0, 0));
      return reinterpret_cast<LRESULT>(g_blackBrush);
    }
    case WM_COMMAND:
      if (LOWORD(wParam) == kIdLoginBtn && HIWORD(wParam) == BN_CLICKED) {
        DoLogin(hwnd);
      }
      return 0;
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
      640, nullptr, nullptr, hInstance, nullptr);
  if (hwnd == nullptr) return 1;
  CreateLoginControls(hwnd, hInstance);
  ShowWindow(hwnd, nCmdShow);
  UpdateWindow(hwnd);

  MSG msg;
  while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
    TranslateMessage(&msg);
    DispatchMessageW(&msg);
  }
  return (int)msg.wParam;
}
