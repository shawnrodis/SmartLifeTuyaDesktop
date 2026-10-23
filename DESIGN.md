# Shared design system — SmartLife & Tuya Desktop

The web edition (`web/SmartLife_Tuya_Desktop.html`, CSS variables) and the
Windows edition (`src/main.rs`, `Palette`/`ACCENTS` constants) render the
**same** design. When you change a token in one edition, change it in the
other, and update this file. The look is intentionally flat: solid surfaces
and 1px borders only — no drop shadows, no backdrop blur — so both renderers
(a browser and egui) can match exactly.

## Colour tokens

| Token | Dark | Light | Black | Used for |
|---|---|---|---|---|
| `--bg` / `bg` | `#0b0e14` | `#eef1f6` | `#000000` | App background |
| `--card` / `card` | `#151a24` | `#ffffff` | `#0c0d11` | Cards, header, panels |
| `--card-border` / `border` | `#232b3a` | `#dde3ee` | `#1f232e` | 1px borders everywhere |
| `--text` / `text` | `#e7ebf2` | `#17202e` | `#e7ebf2` | Primary text |
| `--text-dim` / `dim` | `#8b94a7` | `#5d6879` | `#8b94a7` | Secondary text, labels |
| `--text-faint` / `faint` | `#5b6478` | `#8b94a7` | `#5b6478` | Unpinned pin icon, placeholders |
| `--track` / `track` | `#2a3245` | `#dbe1ec` | `#22252e` | Slider track, switch off-state, icon box |
| `--accent` (default) | `#6366f1` | same | same | Interactive accent (user-selectable) |
| `--danger` | `#f43f5e` | same | same | Destructive actions, errors |
| `--ok` | `#22c55e` | same | same | "On" status dot |
| Toast background | `#1e2534` | `#17202e` | `#14161c` | Toasts (always dark text `#eef1f7`) |
| Toast error background | `#4c1120` | same | same | Error toasts |

Three themes — **Dark / Light / Black** — share the accent system; Black
is the true-black (OLED) variant: pure `#000` background, near-black
cards, subtle borders. The theme applies to the login screen too.

Accent choices (both editions, same order): Indigo `#6366f1`, Sky `#0ea5e9`,
Teal `#14b8a6`, Green `#22c55e`, Amber `#f59e0b`, Rose `#f43f5e`,
Violet `#8b5cf6`. The "on" card state is shown by the border turning the
accent colour. Accent-soft (edit bar, icon box when on) is the accent at
~16% alpha.

## Geometry

| Element | Value |
|---|---|
| Card radius | 18px (compact density: 14px) |
| Card padding | 15px top / 16px sides / 14px bottom (compact: 11/13/10) |
| Button / input radius | 11px |
| Login card radius | 22px; width 400px incl. padding (28px sides, 30px top) |
| Header padding | 12px vertical / 18px horizontal, solid card-colour bar |
| Content max width | 1240px, 18px page padding |
| Grid | `minmax(238px, 1fr)`, gap 14px (compact: 198px / 10px) |
| Device icon box | 42×42px, radius 13px (compact 36×36, radius 11) |
| Camera tile video | 16:9 of the card's inner width, black background, small radius; red-dot "LIVE" badge top-left; replaces the icon header when shown |
| Toggle switch | 48×27px, knob 21px, inset 3px |
| Slider | 22px tall hit area, 6px track (rounded 3px), accent fill, 18px white thumb |
| Swatches | 26px circles |
| Section header | 16.5px/750 title, count pill (card bg, 1px border, radius 20) |
| Toast | radius 13px, padding 10/17px, bottom-centre, auto-dismiss 3.6s |

## Type scale

Base font stack (web): system sans. egui uses its bundled proportional font
at the same sizes: page/brand title 18, login title 23, device name 15
(strong), body 14.5, buttons 13.5, small/field labels 12.5, status 12.5,
offline badge 10.5. Because egui's bundled fonts lack many symbols, the
Windows edition embeds three open-licensed fallback fonts
(`assets/fonts/`: DejaVu Sans, Noto Sans Symbols, Noto Sans Symbols 2) so
glyphs like ⚙ ← ✕ ↺ ⠿ ★ render instead of tofu boxes; every glyph the UI
uses is audited against those fonts' coverage.

Settings controls fill the panel width in both editions (the web uses
`width:100%` fields; egui sets the same widths explicitly). Settings
column max width: 760px.

## Layout & behaviour (identical rules, both editions)

- Header order: brand · "Updated …" · search · Autorefresh · interval ·
  Refresh · Edit · Settings · Log out.
- Sections, in DEFAULT order (v1.2.1): **Cameras** → **Pinned** (only
  if any pins) → **Lighting** → user groups (in creation order) →
  **Ungrouped** → **Scenes** (only if any scenes). Every section —
  built-ins included — can be moved in Edit mode (↑/↓ on the section
  header); the arrangement is stored as an ordered list of section
  keys (`sectionSeq`: built-ins as-is, groups as `group:<id>`). A
  stored order wins outright; sections missing from it (a new group,
  a newly-appearing built-in) slot in by the default order around the
  stored ones.
- **Automatic sections.** Lighting and Cameras are built-in (not
  renamable/deletable). An explicit group assignment always wins; an
  unassigned light-capable device (brightness / colour-temp / colour)
  lives in Lighting, an unassigned camera in Cameras, everything else
  unassigned in Ungrouped. In the group dropdown the no-group choice is
  labelled "Lighting (automatic)" / "Cameras (automatic)" / "Ungrouped"
  accordingly. Cameras are card-for-card peers of devices (same grid,
  sorting, pinning, grouping); scenes remain the only separate kind.
- Within a section: A–Z by display name (alias wins) **unless** a custom
  order is saved for that section — covered items come first in the saved
  order, the rest follow A–Z. "Reset to name order" (Edit mode) clears it.
- **Reordering:** in Edit mode, cards are dragged by their body within a
  section (ghost follows the pointer, an insertion gap shows the drop
  point; dropping on another section moves the card there). ↑/↓ buttons
  remain as a secondary method. Both editions behave the same. In the
  Windows edition a press becomes a drag only after ~8px of pointer
  travel (a "pending drag"), so ordinary clicks on card controls —
  Edit/Delete included — always land; the web's HTML5 drag has the
  same click-safe behaviour natively.
- **Grid:** uniform columns (`minmax(238px, 1fr)`, gap 14; compact
  198/10) and every card stretches to its row's tallest height. The web
  gets this from CSS grid; egui renders explicit rows — each row's
  height is its tallest card (measured and cached by content signature),
  every cell is allocated the full width × row height and the card frame
  fills the cell, content top-aligned. Card headers reserve a fixed
  trailing width for pin/toggle/hide so names truncate with an ellipsis
  and never overlap the controls. (v1.2.2: the reserves are the
  MEASURED footprints of those controls under the app's styles —
  device 104px, camera 48px, scene 24px including the gap before
  them; the earlier hand-set reserves over-counted by ~15–23px and,
  against the 40px budget floor, truncated names to "De…" on
  ordinary-width cards. The budget is a pure function of the cell's
  real remaining width, so wider cells widen names 1:1. The web
  edition needs no equivalent: CSS ellipsis at true overflow.)
  **Two-row headers (v1.2.3).** The reserves were never the whole
  problem: at the card widths the column formula actually
  produces (238–262px at ordinary windows/DPI), the total fixed
  overhead — card padding + icon + controls — leaves ~52–76px
  for the name while real names need ~80–106px, so NO single-row
  budget fits them. When the single-row budget falls below 112px
  (scenes: 144px), the header goes two-row: pin + toggle (a
  scene's lone hide ✕; a camera's pin) move to their own
  right-aligned row under the name, and the name reserves only
  the hide button (14px). The decision is a pure function of the
  measured remainder, so it is already encoded in the row-height
  cache's content signature via the card width. A headless
  render gate in the test suite drives the real dashboard and
  asserts the drawn (post-elision) text — names whole at
  1150–1920px widths, in Edit mode, and after a tiny-start
  resize. The web edition mirrors the shape in CSS: `.card-top`
  wraps and pin + switch travel as one `.head-controls` group
  that wraps right-aligned under the name.
- **Camera card sizes (v1.2.1).** A per-camera size (stored in the
  layout, keyed by camera id): **Normal** = one grid cell, **Large** =
  2 columns, **Wide** = the full row; everything else is not
  resizable. Set from the card's Edit-mode tools in both editions. Web:
  CSS `grid-column` span classes (guarded to multi-column widths).
  Windows: the row packer places spans greedily; a row's height
  follows the tallest card it holds, and a spanning camera's video
  area grows with its width (16:9). The Windows tile pump follows the
  size: Normal decodes the SD tile stream at 480×270, Large the same
  SD source at 960×540, Wide the card's selected quality at 960×540.
- **Group controls.** The Lighting header — and any user-group header
  whose members include a light — shows a controls strip under the
  title: master toggle, brightness and colour-temperature sliders, and
  colour swatches + picker (the same components as the cards). Semantics
  are broadcast, never binding, and no group state is stored: the master
  toggle reads ON if any member is on and clicking toggles ALL device
  members; sliders show the capable members' average and broadcast on
  release; colour broadcasts to colour-capable members using each
  member's current brightness. Commands go out sequentially ~200ms
  apart, member states update optimistically, failures are summarised
  in one toast ("…: 4 of 5 updated — X didn't respond") and a refresh
  reconciles. Pinned, Ungrouped, Cameras and Scenes get no group
  controls. **Strip layout (v1.2.3):** each control group is one
  unbreakable mini-panel of uniform height (44px) — All toggle
  (84px), each slider group (240px, label+value over its slider),
  swatches + 🎨 (352px, one row) — and the strip flows whole
  groups left→right, wrapping whole groups only; the custom-colour
  picker opens BELOW the strip. (v1.2.2 allocated the slider
  groups at height 0 inside a wrapped row, which let egui's
  placer staircase and interleave the label/slider pairs. The
  web strip was already atomic — its groups are single flex
  items — so only its structure was verified, not changed.)
- **Camera video (v1.2.0) — a stated platform difference.** Both
  editions show live video inside camera tiles (16:9 area replacing
  the icon header; name/host/quality row and Watch/Copy stay below),
  with a "Live camera previews" Settings toggle (default on). The
  mechanism differs by platform, honestly: the **Windows** edition
  decodes RTSP natively — a bundled ffmpeg (installed beside the app
  exe; Settings can point at another) pumps raw frames to the tile
  (Stream 2/SD on Tapo templates) and to a big in-app player window
  (the card's selected quality; quality switch, external-player
  hand-off, Close). The **web** edition cannot decode RTSP, so its
  tiles embed the configured stream gateway's feed instead (go2rtc
  MJPEG `<img>`, MediaMTX HLS `<video>`); without a gateway its tiles
  keep the plain header and Watch hands off as before. Windows Watch
  routing: gateway configured → gateway page in the browser; else the
  in-app player; else (no ffmpeg found) the external player. The
  tile's pause/play overlay is a PAINTED icon — two rounded bars or a
  triangle inside a small (~28px) translucent rounded chip — never a
  font glyph (the ⏸ symbol-font rendering was a chunky blob).
- **Pump recovery (v1.2.2).** A tile/player pump's stream URL is
  derived from the CURRENT camera record on every reconcile, and the
  reconcile key includes a per-camera edit generation: saving a
  camera (host, port, template, quality, credentials — even with the
  id unchanged) restarts its pump immediately with backoff reset.
  Failures back off 2s → 4s → 8s cap; the backoff resets to an
  immediate retry when the window is shown/focused again, the user
  clicks the tile or Watch, a player connection for that camera
  succeeds, or the record changes. The "Feed unavailable" state is a
  clickable **Retry** button (tile and player); after repeated
  failures the last ffmpeg failure reason (mapped from its stderr:
  "Connection refused", "401 Unauthorized", "Connection timed out",
  "Host not found", "Stream path not found (404)", "Connection
  reset by camera") shows as small sub-text. **Session contention:**
  Tapo cameras allow only ~2 concurrent RTSP sessions, so while the
  in-app player shows a camera, that camera's tile pump stands down
  (its last frame is held on the tile) and resumes when the player
  closes; external-player hand-offs leave the tile alone. Duplicate
  app processes used to hog those sessions invisibly — the
  single-instance guard below removes that whole class.
- **Rate limiting (v1.2.0).** Rate-limit responses
  (`FrequentlyInvoke`) are NEVER shown to the user — no toast, banner
  or status line, in either edition. Discovery falls back to the
  cached devices silently and backs off internally (the effective
  refresh interval stretches ×2/×4/×8 with consecutive hits, 60s–5min,
  resetting on success; autorefresh skips cycles while backed off).
  Control commands retry silently ~1s later (up to 2 extra attempts)
  before counting as failures at all — group broadcasts rely on that
  retry for extra spacing, and their summary toast names only genuine
  failures. Genuine errors (offline, rejected) are still reported
  exactly as before.
- **Discovery merging & control capability (v1.2.2, both editions).**
  Whether a card or group header shows a brightness / colour-temp
  slider follows the device's parse-time CAPABILITY flags (field
  presence in the merged record), never the current value. Merging a
  fresh discovery onto the previous list keeps the last-known value
  for any field the fresh payload omits OR reports as a placeholder
  (JSON null; brightness / colour temp of 0 — some Tuya payloads
  carry those stubs while the real reading is unknown, and `0` used
  to parse as the 2700K slider floor). A capable control with no
  known value renders at a neutral default (brightness 100%, temp
  slider 5500 ≈ 4600K); group headers average members that have
  readings and fall back to the same neutrals. Every discovery is
  merged — including the post-login one (v1.2.1 ran it against an
  empty previous list, so a thin payload could replace the full
  state wholesale and become the new merge anchor).
- A pinned device shows as a full working card in Pinned **and** stays in
  its normal section. Scenes can't be pinned or grouped; they always live
  in Scenes.
- Login screen: centred single card (logo, title, subtitle, error box,
  username, password, region, platform, keep-logged-in, Log in button,
  collapsed Connection/proxy section, credentials fine print).
- Settings panels, in order: Appearance · App updates · Connection ·
  Stream gateway & cameras · Saved login · Hidden devices · Help.
  The Windows edition inserts **Startup & background** after App
  updates (below); the web edition shows a quiet "Web edition" info
  card above App updates (and a matching Help entry) noting that some
  features are Windows-app-only, with a "Get the Windows app" link.
- **Startup & tray (v1.2.1) — Windows only, a stated platform
  difference** (the web edition has no equivalent; its Settings note
  above says so). "Start with Windows" is an ON/OFF toggle (default
  OFF = no autostart entry at all) writing the per-user Run key
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value
  `SmartLifeTuyaDesktop` = the quoted current-exe path; on load the
  toggle reflects the actual registry state (self-healing: an
  existing entry wins and is rewritten canonically). When ON, "Launch
  as" picks exactly one: **Normal window** (no flag), **Minimized
  (to tray)** (`--minimized`), or **Maximized** (`--maximized`);
  both flags also work for manual launches. "Keep running in the
  system tray when the window is closed" is an independent toggle
  (default OFF): with it ON, closing the window hides it instead of
  quitting (refresh and camera pumps keep running), and the first
  hide shows a one-time tray balloon explaining the app is still
  running. The tray icon exists for a session whenever the app
  started minimized or close-to-tray is on (including switched on at
  runtime) and stays until Quit: left-click/double-click restores
  the window; the right-click menu offers Open and Quit (Quit stops
  the pumps, saves and truly exits, as does the updater's installer
  hand-off). Logging out does not quit the app.
  **Effective launch mode (v1.2.4):** the launch presentation is
  resolved by `resolve_launch_mode` — an explicit argv flag wins
  for that launch; with no flag, the SAVED "Launch as" mode
  applies to every launch. (v1.2.1–v1.2.3 consulted only argv, so
  launches with no Run-entry command line — a Start Menu click,
  or Windows' "restartable apps" sign-in relaunch, which uses the
  bare exe path — opened as a normal window even with "Minimized
  (to tray)" saved.) main() resolves the mode through the same
  registry reconcile App::new runs, so the initial viewport and
  `window_hidden` agree; and while a Minimized launch has not yet
  been shown by a real Show action, every frame re-asserts
  `Visible(false)` (the launch hold, `enforce_launch_hold`) so
  the window can neither flash nor stick open.
- **Single instance & tray event pumping (v1.2.2) — Windows.** The
  app is single-instance: the first process holds the named mutex
  `SmartLifeTuyaDesktop.SingleInstance` for its lifetime. A later
  launch creates neither window nor tray icon: it sets the named
  auto-reset event `SmartLifeTuyaDesktop.ShowWindow`, which the
  primary's waiter thread turns into an atomic flag + a repaint
  poke, and exits immediately. The primary consumes the flag through
  the same restore path as tray Open: `Visible(true)` +
  `Minimized(false)` + `Focus` (v1.2.1 omitted `Minimized(false)`,
  so a taskbar-minimized window stayed minimized). Stale guards are
  impossible — the kernel releases the mutex when the owner process
  dies. Tray and menu events arrive on process-global channels and
  are drained at the top of every frame; while the window is hidden
  a 200 ms repaint heartbeat keeps frames coming so Open/Quit and
  the ShowWindow flag are handled promptly (a hidden window gets no
  paint-driven frames of its own). Exactly one tray icon exists per
  process — created once, never re-created on hide/show/toggle
  cycles — and Quit drops it explicitly before exiting so no
  phantom icon remains. v1.2.1 had none of this: each Start Menu
  launch while hidden spawned another process + icon, and the
  hidden original's tray events sat unread.
- **Tray event thread (v1.2.3) — Windows.** The v1.2.2 frame-drain
  model above still failed in the field ("Quit only works when
  the Window is open and open never works at all"): the heartbeat
  itself depends on repaints, and winit's `request_redraw` on an
  invisible Windows window never produces a redraw — so while
  hidden, no frame ran and the channels were never drained (Quit
  "worked" only because it was mostly clicked while visible).
  v1.2.3 therefore takes the frame loop out of the critical
  path: a dedicated thread owns BOTH receivers (muda's menu
  channel and tray-icon's icon channel — separate crossbeam
  channels, verified against the vendored sources), maps events
  through pure functions to actions on a shared queue, pokes the
  stored egui context, AND acts directly in Win32 with the main
  window's HWND (republished every frame): Show =
  `ShowWindowAsync(SW_RESTORE)` + `SetForegroundWindow` (works
  with zero frames); Quit = `ShowWindow(SW_SHOW)` purely to wake
  the frame pipeline so the UI thread runs the real quit path
  from the queue (a one-frame flash of the window on
  Quit-from-tray is the accepted tradeoff). The UI thread drains
  the queue per frame and applies the same unconditional
  show/quit paths; no hidden-state flag gates any action. The
  single-instance waiter routes through the same dispatch (its
  flag + repaint poke had the same starvation). The 200 ms
  hidden heartbeat remains as belt-and-braces. This is a
  Windows-only platform behaviour; the web edition has no tray.
- **Auto-update.** Both editions share one version number and check
  `api.github.com/repos/shawnrodis/SmartLifeTuyaDesktop/releases/latest`
  on launch, at most once per 24h (attempt-based, stored locally:
  web `slc_update`, Windows config `update`), silent on failure.
  Settings → App updates shows current vs latest with a manual check.
  Web: dismissible banner (per-version dismissal) + a download button
  for the release's `.html` asset (a page can't overwrite its own file).
  Windows: "Download & install" downloads the release's Setup `.exe`
  with progress, verifies the file name matches
  `SmartLifeTuyaDesktop-Setup-v{version}.exe`, launches it and exits so
  the installer can replace the app; "Open releases page" is the
  fallback.
