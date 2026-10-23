<p align="center">
  <img src="assets/icon.png" alt="SmartLife & Tuya Desktop icon" width="120">
</p>

# SmartLife & Tuya Desktop

A clean, modern dashboard for your **Smart Life / Tuya** devices — lights,
plugs, switches and scenes — in two editions that look and behave the same:

- **Windows app** (this repo's Rust + egui program): a small native `.exe`,
  no installer required to try it, no admin rights needed.
- **Web app** (`web/SmartLife_Tuya_Desktop.html`): a single self-contained
  HTML file — open it in any modern browser, from disk or any static host.
  No build step, no dependencies.

Both sign in to Tuya's cloud with the same account you use in the Smart Life
(or Tuya Smart) app and control devices through Tuya's own API — the same
approach as the classic Smart Life web dashboards, with the tidy,
customisable interface they always deserved.

## Download (Windows)

Grab `SmartLifeTuyaDesktop-Setup-vX.Y.Z.exe` from
[Releases](https://github.com/shawnrodis/SmartLifeTuyaDesktop/releases) and
run it — per-user install to `%LOCALAPPDATA%\SmartLifeTuyaDesktop`, with
Start Menu shortcuts and an uninstaller. No administrator rights required.
(Building from source is optional; instructions below.)

## Features

- **Live dashboard** — every device and scene from your Smart Life / Tuya
  account, with autorefresh (15s–2min) and manual refresh.
- **Real controls** — on/off switches, brightness and colour-temperature
  sliders (2700K–6500K), colour swatches + full colour picker for colour
  lights and strips, one-tap scene running. Commands are optimistic and
  roll back with an error toast if Tuya rejects them.
- **Your order, your rooms** — sections sort A–Z by name by default; in
  Edit mode you can reorder devices inside a section (that order sticks,
  and each section has a "Reset to name order" button), create groups,
  rename devices with an alias, and hide devices you never use.
- **Pinned shortcuts** — pin any device with 📌 and a full working copy
  appears in the Pinned section at the top; the device also stays in its
  group below. Scenes always get their own Scenes section.
- **Make it yours** — dark/light theme, 7 accent colours, comfortable /
  compact card density, search box. Everything is remembered locally.
- **Private by design** — your credentials go only to Tuya's own servers
  over HTTPS. "Keep me logged in" stores them only on your device
  (web: this browser's local storage; Windows: a JSON config in your user
  profile) so the app can silently re-login when the session expires.

## Which region / platform?

Pick the **region where your Smart Life account was registered** (US / EU /
CN) and the app you use (**Smart Life** or **Tuya Smart**). The wrong region
is the most common cause of "login failed". US and EU accounts connect
directly; China-region traffic is routed through a community CORS proxy in
the web edition's "Auto" mode (configurable in Settings → Connection).

## Build from source

On Windows (install Rust via [rustup](https://rustup.rs) first):

```powershell
cargo build --release
```

Cross-compile from Linux (what the releases are built with):

```bash
# mingw-w64 provides the linker and windres
sudo apt install mingw-w64 nsis
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
# installer:
cp target/x86_64-pc-windows-gnu/release/SmartLifeTuyaDesktop.exe installer/
cd installer && makensis installer.nsi
```

The build script generates a small `.rc` (exe icon +
`assets/SmartLifeTuyaDesktop.manifest`), compiles it with windres and links
the result directly, embedding the icon and the asInvoker manifest. Settings are stored at
`%APPDATA%\SmartLifeTuyaDesktop\config.json`.

## The two editions stay in step

Both editions share one design system — same layout, colours, cards and
behaviour rules — documented in [DESIGN.md](DESIGN.md). The web edition is
at `web/SmartLife_Tuya_Desktop.html`; it talks to the same Tuya API with the
same value scales and control payloads as the Windows app.

## Project layout

```
SmartLifeTuyaDesktop/
├── src/main.rs            # the whole Windows app (egui)
├── build.rs               # embeds icon + admin-free manifest via windres
├── assets/
│   ├── SmartLifeTuyaDesktop.ico      # exe icon
│   ├── SmartLifeTuyaDesktop.manifest # asInvoker manifest
│   └── icon.png                      # runtime / README icon
├── web/
│   └── SmartLife_Tuya_Desktop.html   # the web edition (single file)
├── installer/
│   └── installer.nsi                 # per-user NSIS installer
├── DESIGN.md              # shared design tokens for both editions
└── Cargo.toml
```

## License

GPL-3.0 — see [LICENSE](LICENSE). Copyright (c) 2026 Shawn Rodis.
