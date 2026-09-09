//! Audio ducking — lower every *other* application's output while we speak,
//! then bring it back. We never touch the system/master volume; only the
//! per-process (per-stream) level of other applications is changed.
//!
//! Platform backends live in submodules and expose a single entry point:
//!
//! ```ignore
//! pub fn create(settings: &DuckingSettings) -> Option<Box<dyn Ducker>>
//! ```
//!
//! `None` means "not supported here" (old OS, no sound server, …) and the
//! caller silently plays without ducking.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod platform {
    use super::{Ducker, DuckingSettings, ProbeResult};
    pub const BACKEND_NAME: &str = "none";
    pub fn create(_settings: &DuckingSettings) -> Option<Box<dyn Ducker>> {
        None
    }
    pub fn probe() -> ProbeResult {
        ProbeResult::Unsupported("no ducking backend for this platform".into())
    }
}

/// Human-readable name of the backend compiled for this platform (shown by
/// `local-voice doctor`).
pub fn backend_name() -> &'static str {
    platform::BACKEND_NAME
}

/// Outcome of [`probe`], the ducking self-test run by `local-voice doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    /// Ducking cannot work here (platform, OS version, no sound server…).
    Unsupported(String),
    /// The backend is usable but no other application is currently playing
    /// audio, so a full duck/restore cycle could not be verified.
    NothingPlaying,
    /// Another application's audio was captured, attenuated and restored.
    Ok,
    /// The backend exists but the duck failed; the string carries the reason
    /// and, where known, how to fix it (e.g. a denied permission on macOS).
    Failed(String),
}

/// Settings used by [`probe`]: a short fade so the whole test is quick.
pub const PROBE_SETTINGS: DuckingSettings = DuckingSettings {
    enabled: true,
    level: 0.1,
    fade_ms: 150,
};

/// How long [`probe`] keeps the other apps ducked before restoring them.
pub const PROBE_HOLD: Duration = Duration::from_millis(600);

/// Run the ducking self-test: duck whatever is playing (on macOS a tone we
/// play ourselves through `afplay`, so the tap has a foreign process to
/// capture), hold briefly, restore, and report what happened.
///
/// This is an explicit user action, so it ignores the failure cooldown set
/// by an earlier failed [`duck`] and does not start one itself.
pub fn probe() -> ProbeResult {
    platform::probe()
}

/// Effective ducking settings (already resolved from config + CLI overrides).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuckingSettings {
    /// Master switch. Default: on.
    pub enabled: bool,
    /// Target gain for other apps while we speak, 0.0..=1.0 (0.1 = 10 %).
    pub level: f32,
    /// Fade duration for both the duck and the restore, in milliseconds.
    pub fade_ms: u64,
}

impl DuckingSettings {
    pub const DEFAULT_LEVEL: f32 = 0.1;
    pub const DEFAULT_FADE_MS: u64 = 300;
}

impl Default for DuckingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            level: Self::DEFAULT_LEVEL,
            fade_ms: Self::DEFAULT_FADE_MS,
        }
    }
}

/// A platform backend. Implementations must be `Send` because the MCP server
/// ducks from its background playback thread.
pub trait Ducker: Send {
    /// Fade every other application's output down to `settings.level`.
    /// Blocks until the fade has completed (roughly `fade_ms`).
    /// Must be safe to call when nothing else is playing (no-op is fine).
    fn duck(&mut self) -> anyhow::Result<()>;

    /// Fade every other application back to the level it had before `duck`.
    /// Blocks until the fade has completed. Must be idempotent: calling it
    /// twice, or without a prior successful `duck`, must not error or change
    /// anything.
    fn restore(&mut self) -> anyhow::Result<()>;
}

/// RAII guard returned by [`duck`]. Dropping it restores the other apps.
pub struct DuckGuard {
    ducker: Box<dyn Ducker>,
}

impl Drop for DuckGuard {
    fn drop(&mut self) {
        if let Err(e) = self.ducker.restore() {
            eprintln!("[local-voice] ducking: failed to restore volumes: {e:#}");
        }
    }
}

/// Duck other applications according to `settings`.
///
/// Returns `None` (and plays normally) when ducking is disabled, unsupported
/// on this platform/OS version, or the backend failed. Backend failures are
/// reported to stderr once per process so an MCP server does not spam logs.
pub fn duck(settings: &DuckingSettings) -> Option<DuckGuard> {
    if !settings.enabled || settings.level >= 1.0 {
        return None;
    }
    if in_cooldown() {
        return None;
    }
    let settings = DuckingSettings {
        level: settings.level.clamp(0.0, 1.0),
        ..*settings
    };
    let mut ducker = platform::create(&settings)?;
    match ducker.duck() {
        Ok(()) => Some(DuckGuard { ducker }),
        Err(e) => {
            warn_once(&format!(
                "ducking unavailable: {e:#} (will retry in {} min)",
                FAILURE_COOLDOWN.as_secs() / 60
            ));
            // Best effort: make sure we did not leave anything half-ducked.
            let _ = ducker.restore();
            start_cooldown();
            None
        }
    }
}

/// After a backend failure (typically a silently denied permission on macOS)
/// we stop trying for a while. Every attempt costs a few hundred ms and, on
/// macOS, briefly mutes other apps before we can tell the tap is silent, so
/// retrying on every `speak` would be worse than not ducking at all.
pub const FAILURE_COOLDOWN: Duration = Duration::from_secs(10 * 60);

static COOLDOWN_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

fn in_cooldown() -> bool {
    let guard = COOLDOWN_UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    matches!(*guard, Some(until) if Instant::now() < until)
}

fn start_cooldown() {
    let mut guard = COOLDOWN_UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(Instant::now() + FAILURE_COOLDOWN);
}

/// Print a `[local-voice] …` warning to stderr only the first time it occurs
/// in this process.
pub fn warn_once(msg: &str) {
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.set(()).is_ok() {
        eprintln!("[local-voice] {msg}");
    }
}

/// Interval between gain steps for backends that ramp by repeatedly setting
/// a volume (WASAPI, PulseAudio). Backends that own the sample stream
/// (macOS process tap) ramp per-sample instead.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const RAMP_STEP: Duration = Duration::from_millis(15);

/// Build a perceptually smooth ramp from `from` to `to` lasting `fade_ms`,
/// one entry per [`RAMP_STEP`]. Interpolation is done in the dB domain so the
/// fade sounds linear to the ear; `0.0` is handled as a floor of -60 dB.
/// The last entry is always exactly `to`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn ramp_steps(from: f32, to: f32, fade_ms: u64) -> Vec<f32> {
    let steps = (fade_ms / RAMP_STEP.as_millis() as u64).max(1) as usize;
    let floor_db = -60.0f32;
    let to_db = |g: f32| {
        if g <= 0.0 {
            floor_db
        } else {
            (20.0 * g.log10()).max(floor_db)
        }
    };
    let from_db = to_db(from);
    let target_db = to_db(to);
    let mut out = Vec::with_capacity(steps);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let db = from_db + (target_db - from_db) * t;
        let gain = if db <= floor_db {
            0.0
        } else {
            10f32.powf(db / 20.0)
        };
        out.push(gain);
    }
    if let Some(last) = out.last_mut() {
        *last = to;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_ends_exactly_at_target_and_is_monotonic() {
        let r = ramp_steps(1.0, 0.2, 300);
        assert_eq!(r.len(), 20);
        assert_eq!(*r.last().unwrap(), 0.2);
        assert!(r.windows(2).all(|w| w[0] >= w[1]));
        let up = ramp_steps(0.2, 1.0, 300);
        assert_eq!(*up.last().unwrap(), 1.0);
        assert!(up.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn ramp_with_zero_fade_is_single_step() {
        assert_eq!(ramp_steps(1.0, 0.5, 0), vec![0.5]);
    }

    #[test]
    fn disabled_settings_return_no_guard() {
        let s = DuckingSettings {
            enabled: false,
            ..Default::default()
        };
        assert!(duck(&s).is_none());
        let s = DuckingSettings {
            level: 1.0,
            ..Default::default()
        };
        assert!(duck(&s).is_none());
    }
}
