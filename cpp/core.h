// Minimal dynamic loader for tuya_core.dll (single-JSON C ABI).
// No import library, no codegen: the DLL is loaded at runtime and
// every function takes at most one JSON string (or nothing) and
// returns a JSON string freed with tuya_free_string. Tokens and
// passwords never leave Rust; C++ only ever sees presence markers
// and parsed device objects, exactly like the Flutter bridge.
#pragma once

#define UNICODE
#define _UNICODE
#include <windows.h>
#include <string>

namespace core {

using NoArgFn = char *(__cdecl *)();
using JsonFn = char *(__cdecl *)(const char *);
using FreeFn = void(__cdecl *)(char *);

struct Core {
  HMODULE dll = nullptr;
  NoArgFn version = nullptr;
  NoArgFn loadConfig = nullptr;
  JsonFn login = nullptr;
  JsonFn refresh = nullptr;
  JsonFn control = nullptr;
  JsonFn buildSections = nullptr;
  FreeFn freeString = nullptr;

  bool Load() {
    // Exact runtime name first, test-delivery name as fallback.
    dll = LoadLibraryW(L"tuya_core.dll");
    if (dll == nullptr) dll = LoadLibraryW(L"tuya_core-cpp-test.dll");
    if (dll == nullptr) return false;
    version = reinterpret_cast<NoArgFn>(GetProcAddress(dll, "tuya_version"));
    loadConfig =
        reinterpret_cast<NoArgFn>(GetProcAddress(dll, "tuya_load_config"));
    login = reinterpret_cast<JsonFn>(GetProcAddress(dll, "tuya_login"));
    refresh = reinterpret_cast<JsonFn>(GetProcAddress(dll, "tuya_refresh"));
    control = reinterpret_cast<JsonFn>(GetProcAddress(dll, "tuya_control"));
    buildSections = reinterpret_cast<JsonFn>(
        GetProcAddress(dll, "tuya_build_sections"));
    freeString =
        reinterpret_cast<FreeFn>(GetProcAddress(dll, "tuya_free_string"));
    return version != nullptr && freeString != nullptr;
  }

  // Call a no-arg function; empty string on failure.
  std::string Call(NoArgFn fn) {
    if (fn == nullptr) return "";
    char *raw = fn();
    if (raw == nullptr) return "";
    std::string out(raw);
    if (freeString != nullptr) freeString(raw);
    return out;
  }

  // Call a one-JSON-arg function; empty string on failure.
  std::string Call(JsonFn fn, const std::string &json) {
    if (fn == nullptr) return "";
    char *raw = fn(json.c_str());
    if (raw == nullptr) return "";
    std::string out(raw);
    if (freeString != nullptr) freeString(raw);
    return out;
  }
};

// Escape a string for embedding in a JSON string value.
inline std::string JsonEscape(const std::string &s) {
  std::string out;
  for (char c : s) {
    switch (c) {
      case '"': out += "\\\""; break;
      case '\\': out += "\\\\"; break;
      case '\n': out += "\\n"; break;
      case '\r': out += "\\r"; break;
      case '\t': out += "\\t"; break;
      default:
        if (static_cast<unsigned char>(c) < 0x20) {
          char buf[8];
          snprintf(buf, sizeof(buf), "\\u%04x", c);
          out += buf;
        } else {
          out += c;
        }
    }
  }
  return out;
}

// Tiny helpers for the flat fields of a core response. This is NOT a
// general JSON parser; the dashboard milestone parses device objects
// properly. Enough for ok/error/status here.
inline bool JsonBool(const std::string &json, const char *key, bool dflt) {
  const std::string pat = std::string("\"") + key + "\":";
  const size_t at = json.find(pat);
  if (at == std::string::npos) return dflt;
  const size_t v = at + pat.size();
  if (json.compare(v, 4, "true") == 0) return true;
  if (json.compare(v, 5, "false") == 0) return false;
  return dflt;
}

inline std::string JsonString(const std::string &json, const char *key) {
  const std::string pat = std::string("\"") + key + "\":\"";
  const size_t at = json.find(pat);
  if (at == std::string::npos) return "";
  const size_t start = at + pat.size();
  std::string out;
  for (size_t i = start; i < json.size(); i++) {
    if (json[i] == '\\' && i + 1 < json.size()) {
      out += json[++i];
    } else if (json[i] == '"') {
      break;
    } else {
      out += json[i];
    }
  }
  return out;
}

inline std::string WideToUtf8(const std::wstring &w) {
  if (w.empty()) return "";
  const int n = WideCharToMultiByte(CP_UTF8, 0, w.c_str(), (int)w.size(),
                                    nullptr, 0, nullptr, nullptr);
  std::string out(n, '\0');
  WideCharToMultiByte(CP_UTF8, 0, w.c_str(), (int)w.size(), out.data(), n,
                      nullptr, nullptr);
  return out;
}

inline std::wstring Utf8ToWide(const std::string &s) {
  if (s.empty()) return L"";
  const int n =
      MultiByteToWideChar(CP_UTF8, 0, s.c_str(), (int)s.size(), nullptr, 0);
  std::wstring out(n, L'\0');
  MultiByteToWideChar(CP_UTF8, 0, s.c_str(), (int)s.size(), out.data(), n);
  return out;
}

}  // namespace core
