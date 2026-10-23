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
- Sections, in order: **Pinned** (only if any pins) → **Lighting** →
  **Cameras** → user groups (in creation order) → **Ungrouped** →
  **Scenes** (only if any scenes).
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
  remain as a secondary method. Both editions behave the same.
- **Grid:** uniform columns (`minmax(238px, 1fr)`, gap 14; compact
  198/10) and every card stretches to its row's tallest height. The web
  gets this from CSS grid; egui renders explicit rows — each row's
  height is its tallest card (measured and cached by content signature),
  every cell is allocated the full width × row height and the card frame
  fills the cell, content top-aligned. Card headers reserve a fixed
  trailing width for pin/toggle/hide so names truncate with an ellipsis
  and never overlap the controls.
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
- A pinned device shows as a full working card in Pinned **and** stays in
  its normal section. Scenes can't be pinned or grouped; they always live
  in Scenes.
- Login screen: centred single card (logo, title, subtitle, error box,
  username, password, region, platform, keep-logged-in, Log in button,
  collapsed Connection/proxy section, credentials fine print).
- Settings panels, in order: Appearance · App updates · Connection ·
  Stream gateway & cameras · Saved login · Hidden devices · Help.
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
