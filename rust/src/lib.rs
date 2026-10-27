//! tuya_core — Rust core for SmartLifeTuyaDesktop (Flutter edition).
//!
//! Port of the pure logic from the original Rust+UI app's single-file
//! `src/main.rs` (v1.4.1), stripped of all UI-framework code. The
//! Flutter shell talks to this crate through the C ABI in `ffi`
//! (JSON in/out), or directly as an rlib. No secrets are logged:
//! passwords/tokens are never printed, and the XOR+base64 storage
//! obfuscation is preserved byte-for-byte (it is obfuscation, not
//! encryption, exactly as in the original).

pub mod camera;
pub mod config;
pub mod convert;
pub mod device;
pub mod ffmpeg;
pub mod ffi;
pub mod launch;
pub mod monitor;
pub mod net;
pub mod obfuscate;
pub mod player;
pub mod pump;
pub mod region;
pub mod registry;
pub mod snap;
pub mod system;
pub mod update;
pub mod win32win;

pub use camera::*;
pub use config::*;
pub use convert::*;
pub use device::*;
pub use ffmpeg::*;
pub use launch::*;
pub use monitor::*;
pub use net::*;
pub use obfuscate::*;
pub use player::*;
pub use pump::*;
pub use region::*;
pub use snap::*;
pub use system::*;
pub use update::*;
pub use win32win::*;
