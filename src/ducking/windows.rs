//! Windows ducking backend — WASAPI per-session volume.
//!
//! While we speak, every *other* application's audio session on the default
//! render endpoint is faded down to `settings.level` via `ISimpleAudioVolume`
//! (the same per-app slider Windows shows in the Volume Mixer), then faded
//! back to exactly the level it had before. The endpoint/master volume is
//! never touched.
//!
//! # Threading / `Send`
//!
//! `Ducker` must be `Send` (the MCP server ducks from its playback thread),
//! but windows-rs COM interface handles are deliberately `!Send`. Rather than
//! pin a dedicated COM thread and shuttle commands over channels, this backend
//! keeps **no COM objects across calls**: `duck()` enters the MTA, enumerates
//! sessions, ramps them, and remembers only plain data per session (instance
//! identifier, pid, original volume). `restore()` re-enumerates and matches
//! sessions by instance identifier. Sessions that vanished in between are
//! simply skipped; a session that merely went idle is still found (we match
//! against all sessions on restore, not just active ones) and restored.
//!
//! This is simpler and has no lifetime/ownership subtleties: every COM object
//! is released before the apartment reference taken by the same call is
//! dropped, and the struct itself is trivially `Send`.

use std::ptr;
use std::sync::OnceLock;
use std::thread::sleep;

use anyhow::{Context, anyhow};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::Interface;

use super::{
    Ducker, DuckingSettings, PROBE_HOLD, PROBE_SETTINGS, ProbeResult, RAMP_STEP, ramp_steps,
};

pub const BACKEND_NAME: &str = "WASAPI per-session volume";

pub fn create(settings: &DuckingSettings) -> Option<Box<dyn Ducker>> {
    Some(Box::new(WasapiDucker {
        level: settings.level,
        fade_ms: settings.fade_ms,
        ducked: false,
        sessions: Vec::new(),
    }))
}

/// Ducking self-test: enumerate sessions and, if any foreign one is audibly
/// playing, run a full duck / hold / restore cycle on it. Never touches the
/// failure cooldown.
pub fn probe() -> ProbeResult {
    let playing = {
        let _com = match ComApartment::enter() {
            Ok(c) => c,
            Err(e) => return ProbeResult::Failed(format!("{e:#}")),
        };
        match enumerate_sessions() {
            Ok(live) => live.iter().filter(|s| s.active && !s.muted).count(),
            Err(e) => return ProbeResult::Failed(format!("enumerating audio sessions: {e:#}")),
        }
    };
    if playing == 0 {
        return ProbeResult::NothingPlaying;
    }
    let mut ducker = WasapiDucker {
        level: PROBE_SETTINGS.level,
        fade_ms: PROBE_SETTINGS.fade_ms,
        ducked: false,
        sessions: Vec::new(),
    };
    if let Err(e) = ducker.duck() {
        let _ = ducker.restore();
        return ProbeResult::Failed(format!("{e:#}"));
    }
    if ducker.sessions.is_empty() {
        // The session stopped between the two enumerations.
        return ProbeResult::NothingPlaying;
    }
    sleep(PROBE_HOLD);
    match ducker.restore() {
        Ok(()) => ProbeResult::Ok,
        Err(e) => ProbeResult::Failed(format!("restore failed: {e:#}")),
    }
}

// ── Debug logging ────────────────────────────────────────────────────────────

fn debug_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LOCAL_VOICE_DEBUG").is_some_and(|v| v == "1"))
}

macro_rules! debug {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!("[local-voice] ducking: {}", format_args!($($arg)*));
        }
    };
}

// ── COM apartment guard ──────────────────────────────────────────────────────

/// Balanced `CoInitializeEx` / `CoUninitialize` for the calling thread.
///
/// If the thread already lives in a different apartment (`RPC_E_CHANGED_MODE`)
/// we just use it and do not uninitialise. Must be declared *before* any COM
/// interface locals in the same scope so it is dropped last.
struct ComApartment {
    uninit: bool,
}

impl ComApartment {
    fn enter() -> anyhow::Result<Self> {
        // SAFETY: plain FFI call with no pointer arguments.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            // S_OK (first init) or S_FALSE (already initialised, ref-counted).
            Ok(Self { uninit: true })
        } else if hr == RPC_E_CHANGED_MODE {
            Ok(Self { uninit: false })
        } else {
            Err(anyhow!("CoInitializeEx failed: {}", hr.message()))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: balances the successful CoInitializeEx in `enter`.
            unsafe { CoUninitialize() };
        }
    }
}

// ── Session snapshot ─────────────────────────────────────────────────────────

/// What we remember about a ducked session between `duck` and `restore`.
/// Plain data only, so the ducker stays `Send`.
#[derive(Debug, Clone)]
struct Remembered {
    /// `IAudioSessionControl2::GetSessionInstanceIdentifier` — unique per
    /// session instance, stable for its lifetime.
    id: String,
    pid: u32,
    original: f32,
}

/// A live session handle for the duration of one `duck`/`restore` call.
struct Live {
    volume: ISimpleAudioVolume,
    id: String,
    pid: u32,
    original: f32,
    muted: bool,
    active: bool,
}

/// Enumerate every session on the default console render endpoint, except
/// our own process and the system-sounds session.
fn enumerate_sessions() -> anyhow::Result<Vec<Live>> {
    let own_pid = std::process::id();

    // SAFETY: standard WASAPI object creation sequence; every call is made
    // with valid interface pointers owned by the wrappers below, on a thread
    // that has entered a COM apartment (caller holds a `ComApartment`).
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("CoCreateInstance(MMDeviceEnumerator)")?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .context("GetDefaultAudioEndpoint(eRender, eConsole)")?;
        let manager: IAudioSessionManager2 = device
            .Activate(CLSCTX_ALL, None)
            .context("IMMDevice::Activate(IAudioSessionManager2)")?;
        let sessions = manager
            .GetSessionEnumerator()
            .context("GetSessionEnumerator")?;
        let count = sessions
            .GetCount()
            .context("IAudioSessionEnumerator::GetCount")?;

        let mut out = Vec::with_capacity(count.max(0) as usize);
        for i in 0..count {
            let control = match sessions.GetSession(i) {
                Ok(c) => c,
                Err(e) => {
                    debug!("session {i}: GetSession failed ({e}); skipping");
                    continue;
                }
            };
            let control2: IAudioSessionControl2 = match control.cast() {
                Ok(c) => c,
                Err(e) => {
                    debug!("session {i}: no IAudioSessionControl2 ({e}); skipping");
                    continue;
                }
            };

            let pid = control2.GetProcessId().unwrap_or(0);
            if pid == own_pid {
                continue;
            }
            // S_OK => system sounds, S_FALSE => not. Both are success codes.
            if control2.IsSystemSoundsSession().0 == 0 {
                debug!("session {i}: system sounds; skipping");
                continue;
            }

            let id = match session_instance_id(&control2) {
                Some(id) => id,
                None => {
                    debug!("session {i} (pid {pid}): no instance identifier; skipping");
                    continue;
                }
            };

            let active = control2
                .GetState()
                .map(|s| s == AudioSessionStateActive)
                .unwrap_or(false);

            let volume: ISimpleAudioVolume = match control2.cast() {
                Ok(v) => v,
                Err(e) => {
                    debug!("session {i} (pid {pid}): no ISimpleAudioVolume ({e}); skipping");
                    continue;
                }
            };
            // A session being torn down at this very moment can fail here.
            // Never let one flaky session abort the whole enumeration:
            // `restore()` relies on this list to un-duck the others.
            let original = match volume.GetMasterVolume() {
                Ok(v) => v,
                Err(e) => {
                    debug!("session {i} (pid {pid}): GetMasterVolume failed ({e}); skipping");
                    continue;
                }
            };
            let muted = volume.GetMute().map(|b| b.as_bool()).unwrap_or(false);

            out.push(Live {
                volume,
                id,
                pid,
                original,
                muted,
                active,
            });
        }
        Ok(out)
    }
}

/// Read and free the CoTaskMem-allocated session instance identifier.
///
/// # Safety
/// Must be called on a thread that entered a COM apartment.
unsafe fn session_instance_id(control: &IAudioSessionControl2) -> Option<String> {
    // SAFETY: the returned PWSTR is a NUL-terminated string allocated with
    // CoTaskMemAlloc that we own and must free exactly once.
    unsafe {
        let pwstr = control.GetSessionInstanceIdentifier().ok()?;
        if pwstr.is_null() {
            return None;
        }
        let s = pwstr.to_string().ok();
        CoTaskMemFree(Some(pwstr.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

/// Set `original * gain` on every live session, dropping any session whose
/// stream went away mid-ramp so a single dying app cannot abort the fade.
fn apply_gain(live: &mut Vec<Live>, gain: f32) {
    live.retain(|s| {
        let target = (s.original * gain).clamp(0.0, 1.0);
        // SAFETY: `s.volume` is a valid interface pointer held by this call;
        // a null event context is explicitly permitted by the API.
        match unsafe { s.volume.SetMasterVolume(target, ptr::null()) } {
            Ok(()) => true,
            Err(e) => {
                debug!(
                    "pid {} ({}): SetMasterVolume failed ({e}); dropping",
                    s.pid, s.id
                );
                false
            }
        }
    });
}

/// Run one ramp over `live`, sleeping [`RAMP_STEP`] between steps.
fn ramp(live: &mut Vec<Live>, steps: &[f32]) {
    for (i, gain) in steps.iter().enumerate() {
        if i > 0 {
            sleep(RAMP_STEP);
        }
        apply_gain(live, *gain);
        if live.is_empty() {
            break;
        }
    }
}

// ── Ducker ───────────────────────────────────────────────────────────────────

struct WasapiDucker {
    level: f32,
    fade_ms: u64,
    ducked: bool,
    sessions: Vec<Remembered>,
}

impl Ducker for WasapiDucker {
    fn duck(&mut self) -> anyhow::Result<()> {
        if self.ducked {
            return Ok(());
        }

        // Declared first so it is dropped after every COM object below.
        let _com = ComApartment::enter()?;
        let mut live = enumerate_sessions().context("enumerating audio sessions")?;

        // Only fade what is audibly playing: active and not muted. Everything
        // else is left untouched (and therefore not remembered).
        live.retain(|s| {
            if !s.active {
                debug!("pid {} ({}): not active; skipping", s.pid, s.id);
                false
            } else if s.muted {
                debug!("pid {} ({}): muted; skipping", s.pid, s.id);
                false
            } else {
                true
            }
        });

        // Remember *before* ramping so a mid-ramp failure is still restorable.
        self.sessions = live
            .iter()
            .map(|s| Remembered {
                id: s.id.clone(),
                pid: s.pid,
                original: s.original,
            })
            .collect();
        self.ducked = true;

        if live.is_empty() {
            debug!("no active sessions to duck");
            return Ok(());
        }
        for s in &live {
            debug!(
                "ducking pid {} ({}) from {:.3} to {:.3}",
                s.pid,
                s.id,
                s.original,
                s.original * self.level
            );
        }

        let steps = ramp_steps(1.0, self.level, self.fade_ms);
        ramp(&mut live, &steps);
        drop(live);
        Ok(())
    }

    fn restore(&mut self) -> anyhow::Result<()> {
        if !self.ducked {
            return Ok(());
        }
        self.ducked = false;
        let remembered = std::mem::take(&mut self.sessions);
        if remembered.is_empty() {
            return Ok(());
        }

        let _com = ComApartment::enter()?;
        let current = enumerate_sessions().context("re-enumerating audio sessions")?;

        // Match by instance identifier, regardless of current state: a
        // session that went idle while we spoke still carries our lowered
        // volume and must be put back.
        let mut live: Vec<Live> = current
            .into_iter()
            .filter_map(|mut s| {
                let r = remembered.iter().find(|r| r.id == s.id)?;
                s.original = r.original;
                Some(s)
            })
            .collect();

        if debug_enabled() {
            for r in &remembered {
                if !live.iter().any(|s| s.id == r.id) {
                    debug!("pid {} ({}): session gone; skipping restore", r.pid, r.id);
                }
            }
            for s in &live {
                debug!("restoring pid {} ({}) to {:.3}", s.pid, s.id, s.original);
            }
        }
        if live.is_empty() {
            return Ok(());
        }

        // `ramp_steps` guarantees the last entry is exactly 1.0, so the final
        // value written is exactly `original`.
        let steps = ramp_steps(self.level, 1.0, self.fade_ms);
        ramp(&mut live, &steps);
        drop(live);
        Ok(())
    }
}

impl Drop for WasapiDucker {
    fn drop(&mut self) {
        if self.ducked
            && let Err(e) = self.restore()
        {
            eprintln!("[local-voice] ducking: failed to restore volumes on drop: {e:#}");
        }
    }
}
