//! Global pump registry keyed by "cam:{id}" and "player".
//!
//! The Flutter shell polls; the registry keeps only the LATEST frame
//! per pump (older frames are dropped, never queued), matching the
//! original tile/player behaviour. Backoff/retry scheduling lives in
//! the shell: it re-calls `camera_start` after the backoff delay the
//! core reports, or polls `camera_frame` which reports "backoff".

use crate::ffmpeg::FrameMsg;
use crate::pump::{spawn_pump, Pump};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

pub struct PumpEntry {
    pub pump: Pump,
    /// Latest decoded frame (RGBA bytes), if any.
    pub latest: Option<Vec<u8>>,
}

static REGISTRY: LazyLock<Mutex<HashMap<String, PumpEntry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn registry_start(pump_id: &str, ffmpeg: &Path, url: &str, w: usize, h: usize) -> Result<(), String> {
    registry_stop(pump_id);
    let source_key = crate::pump::pump_source_key(ffmpeg, url, w, h);
    let pump = spawn_pump(ffmpeg, url, w, h, source_key);
    if let Ok(mut reg) = REGISTRY.lock() {
        reg.insert(pump_id.to_string(), PumpEntry { pump, latest: None });
        Ok(())
    } else {
        Err("pump registry locked".to_string())
    }
}

pub fn registry_stop(pump_id: &str) {
    if let Ok(mut reg) = REGISTRY.lock() {
        if let Some(entry) = reg.remove(pump_id) {
            entry.pump.stop();
        }
    }
}

pub fn registry_stop_all() {
    if let Ok(mut reg) = REGISTRY.lock() {
        for (_, entry) in reg.drain() {
            entry.pump.stop();
        }
    }
}

/// Drain the pump channel, keeping only the latest frame. Returns
/// (state, w, h, latest_frame_or_none, error_or_none).
pub fn registry_poll(pump_id: &str) -> Option<(String, usize, usize, Option<Vec<u8>>, Option<String>)> {
    let mut reg = REGISTRY.lock().ok()?;
    let entry = reg.get_mut(pump_id)?;
    while let Ok(msg) = entry.pump.rx.try_recv() {
        match msg {
            FrameMsg::Frame(bytes) => {
                entry.pump.note_frame();
                entry.latest = Some(bytes);
            }
            FrameMsg::Ended(reason) => {
                entry.pump.note_ended(reason);
            }
        }
    }
    let state = match entry.pump.state {
        crate::ffmpeg::PumpState::Live => "live",
        crate::ffmpeg::PumpState::Connecting => "connecting",
        crate::ffmpeg::PumpState::Backoff => "backoff",
    };
    let w = entry.pump.w;
    let h = entry.pump.h;
    let frame = entry.latest.clone();
    let err = entry.pump.last_error.clone();
    let retry_secs = entry.pump.retry_at.map(|t| {
        t.saturating_duration_since(std::time::Instant::now()).as_secs_f64()
    });
    let _ = retry_secs;
    Some((state.to_string(), w, h, frame, err))
}

/// Backoff delay remaining in seconds for a backed-off pump, if any.
pub fn registry_backoff_secs(pump_id: &str) -> Option<f64> {
    let reg = REGISTRY.lock().ok()?;
    let entry = reg.get(pump_id)?;
    entry.pump.retry_at.map(|t| t.saturating_duration_since(std::time::Instant::now()).as_secs_f64())
}

pub fn registry_exists(pump_id: &str) -> bool {
    REGISTRY.lock().map(|r| r.contains_key(pump_id)).unwrap_or(false)
}
