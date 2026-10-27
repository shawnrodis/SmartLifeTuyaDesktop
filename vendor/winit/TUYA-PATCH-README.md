# Vendored winit 0.30.13 (Tuya patch)

Base: upstream winit 0.30.13 from crates.io, unchanged except for ONE patch:

- `src/platform_impl/windows/event_loop.rs` (`WM_DPICHANGED`) +
  `src/platform_impl/windows/dark_mode.rs` (`WIN10_BUILD_VERSION` made
  `pub(crate)`): on Windows 11 (build >= 22000), apply the rect Windows
  suggests instead of winit's conservative-rect + `MonitorFromWindow`
  adjustment. During a drag, `MonitorFromWindow` still answers with the
  monitor being left, so the old path nudges the window back, triggers
  another `WM_DPICHANGED`, and the window grows and stays on the wrong
  monitor at the wrong scale (rust-windowing/winit#4041, #4600).

This is a port of upstream winit#4341 (master commit 488c036a), as carried
by storytold/winit patch W1 (the patch that makes PdfCraft stable across
mixed-DPI monitors). It is NOT a fork: no other behavior is changed.

DROP THIS VENDORED COPY once an upstream winit release ships the fix and
eframe moves to it — then remove the `[patch.crates-io]` entry in Cargo.toml
and delete this directory.
