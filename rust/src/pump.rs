use crate::ffmpeg::{pump_backoff, FrameMsg, PumpState};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;
pub fn pump_source_key(ffmpeg: &Path, url: &str, w: usize, h: usize) -> String {
    format!("{}|{}|{}x{}", ffmpeg.display(), url, w, h)
}

/// One running (or backing-off) frame pump, owned by the UI thread.
pub struct Pump {
    pub state: PumpState,
    pub source_key: String,
    pub w: usize,
    pub h: usize,
    pub rx: Receiver<FrameMsg>,
    pub stop: Arc<AtomicBool>,
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    /// Consecutive failures — drives the backoff schedule.
    pub streak: u32,
    pub retry_at: Option<Instant>,
    /// Last mapped failure reason (v1.2.2) — shown under the
    /// tile's retry status after repeated failures.
    pub last_error: Option<String>,
}

impl Pump {
    /// Stop the worker: flag it and kill ffmpeg (killing closes
    /// stdout, which ends the worker's read loop immediately).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.child.lock() {
            if let Some(mut ch) = slot.take() {
                let _ = ch.kill();
            }
        }
    }

    /// A frame arrived: the pump is Live and the failure streak
    /// resets (state-machine transition, kept pure for tests).
    pub fn note_frame(&mut self) {
        self.state = PumpState::Live;
        self.streak = 0;
        self.retry_at = None;
        self.last_error = None;
    }

    /// ffmpeg exited / hit EOF: enter Backoff with the streak's
    /// delay, remembering why. A no-op when already backing off —
    /// one pump, one state machine, never duplicate restarts.
    pub fn note_ended(&mut self, reason: Option<String>) {
        if self.state != PumpState::Backoff {
            self.state = PumpState::Backoff;
            self.retry_at = Some(Instant::now() + pump_backoff(self.streak));
            self.streak += 1;
            if reason.is_some() {
                self.last_error = reason;
            }
        }
    }
}

/// Spawn the worker thread for one pump. The thread runs ffmpeg,
/// reads exact w×h RGBA frames and forwards them; it keeps ffmpeg
/// blocked on a full pipe rather than building a backlog here, and
/// the UI keeps only the latest frame, so a slow frame is dropped,
/// never queued. ffmpeg's stderr is captured (last few lines) so
/// an exit can carry its mapped failure reason (v1.2.2).
/// `source_key` is computed by the caller (sync_pumps) so the
/// generation tag for camera edits rides along.

/// Spawn the worker thread for one pump, WITHOUT any UI context
/// (the original's `ctx.request_repaint()` calls are removed — the
/// Flutter shell polls `tuya_camera_frame` instead). The thread runs
/// ffmpeg, reads exact w×h RGBA frames and forwards them; it keeps
/// ffmpeg blocked on a full pipe rather than building a backlog, and
/// the registry keeps only the latest frame, so a slow frame is
/// dropped, never queued. ffmpeg's stderr is captured (last few
/// lines) so an exit can carry its mapped failure reason.
pub fn spawn_pump(
    ffmpeg: &Path,
    url: &str,
    w: usize,
    h: usize,
    source_key: String,
) -> Pump {
    use crate::ffmpeg::{ffmpeg_args, ffmpeg_failure_reason, frame_bytes};
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::channel;
    let (tx, rx) = channel::<FrameMsg>();
    let stop = Arc::new(AtomicBool::new(false));
    let child_slot: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    {
        let stop = stop.clone();
        let child_slot = child_slot.clone();
        let ff = ffmpeg.to_path_buf();
        let url = url.to_string();
        std::thread::spawn(move || {
            let mut cmd = Command::new(&ff);
            cmd.args(ffmpeg_args(&url, w, h))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(_) => {
                    let _ = tx.send(FrameMsg::Ended(Some("Could not start ffmpeg".to_string())));
                    return;
                }
            };
            let err_tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            if let Some(stderr) = child.stderr.take() {
                let tail = err_tail.clone();
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let mut reader = BufReader::new(stderr);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                let t = line.trim().to_string();
                                if !t.is_empty() {
                                    if let Ok(mut v) = tail.lock() {
                                        v.push(t);
                                        if v.len() > 12 {
                                            v.remove(0);
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            }
            let mut stdout = child.stdout.take();
            if let Ok(mut slot) = child_slot.lock() {
                *slot = Some(child);
            }
            if let Some(mut out) = stdout.take() {
                let mut buf = vec![0u8; frame_bytes(w, h)];
                loop {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    match out.read_exact(&mut buf) {
                        Ok(()) => {
                            if tx.send(FrameMsg::Frame(buf.clone())).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            if let Ok(mut slot) = child_slot.lock() {
                if let Some(mut ch) = slot.take() {
                    let _ = ch.kill();
                    let _ = ch.wait();
                }
            }
            if !stop.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(30));
                let tail = err_tail.lock().map(|v| v.join("\n")).unwrap_or_default();
                let _ = tx.send(FrameMsg::Ended(ffmpeg_failure_reason(&tail)));
            }
        });
    }
    Pump {
        state: PumpState::Connecting,
        source_key,
        w,
        h,
        rx,
        stop,
        child: child_slot,
        streak: 0,
        retry_at: None,
        last_error: None,
    }
}
