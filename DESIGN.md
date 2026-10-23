# Shared design system — SmartLife & Tuya Desktop

The web edition (`web/SmartLife_Tuya_Desktop.html`, CSS variables) and the
Windows edition (`src/main.rs`, `Palette`/`ACCENTS` constants) render the
**same** design. When you change a token in one edition, change it in the
other, and update this file. The look is intentionally flat: solid surfaces
and 1px borders only — no drop shadows, no backdrop blur — so both renderers
(a browser and egui) can match exactly.

## Colour tokens

| Token | Dark | Light | Used for |
|---|---|---|---|
| `--bg` / `bg` | `#0b0e14` | `#eef1f6` | App background |
| `--card` / `card` | `#151a24` | `#ffffff` | Cards, header, panels |
| `--card-border` / `border` | `#232b3a` | `#dde3ee` | 1px borders everywhere |
| `--text` / `text` | `#e7ebf2` | `#17202e` | Primary text |
| `--text-dim` / `dim` | `#8b94a7` | `#5d6879` | Secondary text, labels |
| `--text-faint` / `faint` | `#5b6478` | `#8b94a7` | Unpinned pin icon, placeholders |
| `--track` / `track` | `#2a3245` | `#dbe1ec` | Slider track, switch off-state, icon box |
| `--accent` (default) | `#6366f1` | same | Interactive accent (user-selectable) |
| `--danger` | `#f43f5e` | same | Destructive actions, errors |
| `--ok` | `#22c55e` | same | "On" status dot |
| Toast background | `#1e2534` | `#17202e` | Toasts (always dark text `#eef1f7`) |
| Toast error background | `#4c1120` | same | Error toasts |

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
offline badge 10.5.

## Layout & behaviour (identical rules, both editions)

- Header order: brand · "Updated …" · search · Autorefresh · interval ·
  Refresh · Edit · Settings · Log out.
- Sections, in order: **Pinned** (only if any pins) → user groups (in
  creation order) → **Ungrouped** → **Scenes** (only if any scenes).
- Within a section: A–Z by display name (alias wins) **unless** a custom
  order is saved for that section — covered items come first in the saved
  order, the rest follow A–Z. "Reset to name order" (Edit mode) clears it.
- A pinned device shows as a full working card in Pinned **and** stays in
  its normal section. Scenes can't be pinned or grouped; they always live
  in Scenes.
- Login screen: centred single card (logo, title, subtitle, error box,
  username, password, region, platform, keep-logged-in, Log in button,
  collapsed Connection/proxy section, credentials fine print).
- Settings panels, in order: Appearance · Connection · Saved login ·
  Hidden devices · Help.
