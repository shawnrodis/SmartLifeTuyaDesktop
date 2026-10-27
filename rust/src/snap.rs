use crate::monitor::MonitorInfo;
use std::time::{Duration, Instant};
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SnapPhase {
    /// Leave the states other layers own first: fullscreen is
    /// winit's (released via a viewport command), the maximized
    /// / minimized show state is released with a Win32 restore.
    LeaveStates,
    /// Issue the physical-pixel move (SetWindowPos).
    Move,
    /// Confirm arrival (MonitorFromWindow == the target) before
    /// maximizing; a miss retries the restore+move once.
    Verify,
}

/// What one snap step should do this frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SnapAction {
    SendFullscreenOff,
    Restore,
    Wait,
    MoveWindow,
    MaximizeAndFinish,
    Fail,
}

/// The facts the snap decision needs, gathered fresh per frame.
#[derive(Clone, Copy, Default)]

pub struct SnapFacts {
    /// UI reports the window in (borderless) fullscreen.
    pub fullscreen: bool,
    /// The window is maximized or minimized (needs a restore
    /// before it can be moved meaningfully).
    pub zoomed: bool,
    /// MonitorFromWindow says the window is on the target.
    pub on_target: bool,
    /// The job's overall time budget has run out.
    pub expired: bool,
}

/// Move attempts per snap: the initial move plus ONE retry.
pub const SNAP_MAX_MOVES: u8 = 2;
/// Overall budget for one snap. Past it the job fails safe:
/// the window stays wherever it verifiably is — visible, on a
/// real monitor (every move targets a clamped interior point),
/// never wedged mid-transition.
pub const SNAP_TIMEOUT: Duration = Duration::from_secs(3);

/// The pure snap decision: (next phase, this frame's action).
/// Arrival beats expiry — a window that just landed is
/// maximized, not failed. Kept free of Win32 so the whole
/// sequence is unit-testable headlessly.
pub fn snap_next(phase: SnapPhase, f: SnapFacts, attempts: u8) -> (SnapPhase, SnapAction) {
    use SnapAction::*;
    use SnapPhase::*;
    if phase == Verify && f.on_target {
        return (Verify, MaximizeAndFinish);
    }
    if f.expired {
        return (phase, Fail);
    }
    match phase {
        LeaveStates => {
            if f.fullscreen {
                (LeaveStates, SendFullscreenOff)
            } else if f.zoomed {
                (Move, Restore)
            } else {
                (Move, Wait)
            }
        }
        Move => (Verify, MoveWindow),
        Verify => {
            if attempts < SNAP_MAX_MOVES {
                (Move, Restore)
            } else {
                (Verify, Fail)
            }
        }
    }
}

/// An in-flight snap (v1.3.1): created by a Monitor-menu click,
/// advanced per frame by App::advance_snap, dropped on success,
/// failure or timeout. Never persisted.
pub struct SnapJob {
    pub target: MonitorInfo,
    pub phase: SnapPhase,
    pub attempts: u8,
    pub deadline: Instant,
}
