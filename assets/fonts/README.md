# Bundled fallback fonts (Windows edition)

The Rust app embeds these fonts so UI symbols (⚙ ← ✕ ↺ ⠿ ★ …)
render instead of tofu boxes; egui's bundled fonts don't cover them.
They are fallbacks only — Latin text still uses egui's default fonts.

- `DejaVuSans.ttf` — DejaVu Fonts License (permissive, public-domain-like).
  https://dejavu-fonts.github.io/
- `NotoSansSymbols-Regular.ttf`, `NotoSansSymbols2-Regular.ttf` —
  SIL Open Font License 1.1. https://fonts.google.com/noto

Both licences permit embedding and redistribution with the app, which
is itself GPL-3.0 (see the repository LICENSE).
