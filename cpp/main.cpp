// SmartLife Tuya Desktop — Windows C++ shell (milestone 1: scaffold).
//
// Native Win32 window, custom-painted to match the app's Black theme
// (black background, white text) — NOT a stock grey dialog. The Tuya
// logic lives in the Rust core (tuya_core.dll), loaded at runtime;
// this file owns only the window. Tokens/passwords never cross into
// C++ (same presence-marker contract as rust/README-API.md).
//
// Build: see cpp/CMakeLists.txt. Run from a folder containing
// tuya_core.dll (the installer and the CI artifact both do this).

#define UNICODE
#define _UNICODE
#include <windows.h>
#include <string>
#include <vector>

namespace {

const wchar_t kClassName[] = L"SmartLifeTuyaDesktop";
const wchar_t kWindowTitle[] = L"SmartLife Tuya Desktop";

using VersionFn = char *(__cdecl *)();
using FreeFn = void(__cdecl *)(char *);

std::wstring g_coreVersion = L"core not loaded";
HBRUSH g_blackBrush = nullptr;

std::wstring ToWide(const std::string &s) {
  if (s.empty()) return L"";
  const int n = MultiByteToWideChar(CP_UTF8, 0, s.c_str(), (int)s.size(),
                                    nullptr, 0);
  std::wstring out(n, L'\0');
  MultiByteToWideChar(CP_UTF8, 0, s.c_str(), (int)s.size(), out.data(), n);
  return out;
}

void LoadCoreVersion() {
  // DLL sits next to the exe, exactly like the egui/Flutter installs.
  // Try the exact runtime name first; a test delivery once renamed it
  // to tuya_core-cpp-test.dll and the load failed on the name alone,
  // so fall back to that name rather than reporting "not found".
  HMODULE core = LoadLibraryW(L"tuya_core.dll");
  if (core == nullptr) core = LoadLibraryW(L"tuya_core-cpp-test.dll");
  if (core == nullptr) {
    g_coreVersion = L"tuya_core.dll not found next to the exe";
    return;
  }
  auto versionFn =
      reinterpret_cast<VersionFn>(GetProcAddress(core, "tuya_version"));
  auto freeFn = reinterpret_cast<FreeFn>(GetProcAddress(core, "tuya_free_string"));
  if (versionFn == nullptr) {
    g_coreVersion = L"tuya_version not exported by tuya_core.dll";
    return;
  }
  char *raw = versionFn();
  if (raw == nullptr) {
    g_coreVersion = L"tuya_version returned nothing";
    return;
  }
  g_coreVersion = ToWide(std::string(raw));
  if (freeFn != nullptr) freeFn(raw);
}

LRESULT CALLBACK WndProc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam) {
  switch (msg) {
    case WM_PAINT: {
      PAINTSTRUCT ps;
      HDC hdc = BeginPaint(hwnd, &ps);
      // Black theme background, then the scaffold status text.
      RECT rc;
      GetClientRect(hwnd, &rc);
      FillRect(hdc, &rc, g_blackBrush);
      SetBkMode(hdc, TRANSPARENT);
      SetTextColor(hdc, RGB(255, 255, 255));
      const std::wstring line1 = L"SmartLife Tuya Desktop — C++ shell";
      const std::wstring line2 = L"Core: " + g_coreVersion;
      TextOutW(hdc, 16, 16, line1.c_str(), (int)line1.size());
      SetTextColor(hdc, RGB(180, 180, 180));
      TextOutW(hdc, 16, 44, line2.c_str(), (int)line2.size());
      EndPaint(hwnd, &ps);
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
  LoadCoreVersion();
  g_blackBrush = CreateSolidBrush(RGB(0, 0, 0));

  WNDCLASSW wc = {};
  wc.lpfnWndProc = WndProc;
  wc.hInstance = hInstance;
  wc.hCursor = LoadCursorW(nullptr, IDC_ARROW);
  // Icon comes from the embedded resource (cpp/app.rc) — this is the
  // icon the taskbar shows, so a missing/broken resource is visible
  // immediately and must never be shipped as a black square.
  wc.hIcon = LoadIconW(hInstance, MAKEINTRESOURCEW(1));
  wc.lpszClassName = kClassName;
  wc.hbrBackground = g_blackBrush;
  RegisterClassW(&wc);

  HWND hwnd = CreateWindowExW(
      0, kClassName, kWindowTitle,
      WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 960,
      640, nullptr, nullptr, hInstance, nullptr);
  if (hwnd == nullptr) return 1;
  ShowWindow(hwnd, nCmdShow);
  UpdateWindow(hwnd);

  MSG msg;
  while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
    TranslateMessage(&msg);
    DispatchMessageW(&msg);
  }
  return (int)msg.wParam;
}
