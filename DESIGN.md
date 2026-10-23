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
  and never overlap the controls.
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
  controls.
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
