//! macOS ducking backend built on Core Audio **process taps** (macOS 14.2+).
//!
//! # How it works
//!
//! macOS has no per-application volume API, but since 14.2 the HAL can create a
//! *process tap*: an audio object whose input stream is the mix of the output
//! of a chosen set of processes. Taps also have a mute behaviour; with
//! `CATapMutedWhenTapped` the tapped processes stop being sent to the
//! hardware **for as long as somebody reads the tap**. We use that to become a
//! tiny mixer in front of the speakers:
//!
//! 1. Create a *global* stereo tap that excludes only our own process, so it
//!    picks up every other app (including ones that start mid-duck) but not
//!    our own speech, which would otherwise feed back through the tap.
//! 2. Wrap the tap and the current default output device in a *private
//!    aggregate device*. The aggregate presents the tap as its input streams
//!    and the real device as its output streams.
//! 3. Attach an IOProc to the aggregate that copies input → output while
//!    multiplying by a gain. The gain is ramped per sample with a one-pole
//!    smoother, so the fade is click-free. While the IOProc runs, the tapped
//!    apps are muted at the hardware and only our attenuated copy is audible.
//! 4. On restore, ramp the gain back to 1.0, then stop the IOProc and destroy
//!    the aggregate and the tap. Destroying the tap hands the apps back to
//!    direct output. System / master / device volume is never touched.
//!
//! # Compatibility
//!
//! The two tap entry points (`AudioHardwareCreateProcessTap` /
//! `AudioHardwareDestroyProcessTap`) are resolved with `dlsym` at runtime and
//! the `CATapDescription` class is looked up dynamically, so the binary still
//! launches on macOS < 14.2, where [`create`] simply returns `None`. The rest
//! of the HAL API (aggregate devices, IOProcs, properties) has existed for
//! many years and is linked normally through `objc2-core-audio`.
//!
//! # Permission
//!
//! Reading a tap requires the *System Audio Recording* TCC permission. There
//! is no public API to query it, and denial is silent: every call returns
//! `noErr`, the tap delivers all-zero buffers, and the apps stay muted. We
//! defend against that by (a) only ducking when at least one other process is
//! actually producing output and (b) tearing everything down and returning an
//! error if ~500 ms of IO callbacks contained nothing but exact zeros.

use std::ffi::{CStr, c_void};
use std::mem::{MaybeUninit, size_of};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_core_audio::{
    AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID,
    AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareDestroyAggregateDevice, AudioObjectAddPropertyListener,
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectRemovePropertyListener, AudioObjectSetPropertyData,
    CATapDescription, CATapMuteBehavior, kAudioAggregateDeviceIsPrivateKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDevicePropertyActiveSubDeviceList, kAudioAggregateDevicePropertySubTapList,
    kAudioAggregateDeviceSubDeviceListKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyIOProcStreamUsage, kAudioDevicePropertyNominalSampleRate,
    kAudioDevicePropertyStreams, kAudioHardwareNoError, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyProcessObjectList, kAudioHardwarePropertyTranslatePIDToProcessObject,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyOwnedObjects,
    kAudioObjectPropertyOwner, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject, kAudioObjectUnknown,
    kAudioProcessPropertyIsRunningOutput, kAudioProcessPropertyPID,
    kAudioStreamPropertyStartingChannel, kAudioStreamPropertyTerminalType,
    kAudioStreamPropertyVirtualFormat, kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey,
    kAudioSubTapUIDKey, kAudioTapPropertyFormat, kAudioTapPropertyUID,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp,
    kAudioFormatFlagIsFloat, kAudioFormatLinearPCM,
};
use objc2_core_foundation::{CFDictionary, CFRetained, CFString};
use objc2_foundation::{
    NSArray, NSDictionary, NSNumber, NSOperatingSystemVersion, NSProcessInfo, NSString,
};

use super::{Ducker, DuckingSettings, PROBE_HOLD, PROBE_SETTINGS, ProbeResult};

/// `OSStatus` is `pub(crate)` inside `objc2-core-audio`, so spell it out.
type OSStatus = i32;

// ─── Debug logging ───────────────────────────────────────────────────────────

fn debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("LOCAL_VOICE_DEBUG").is_some_and(|v| v == "1"))
}

/// `eprintln!` gated on `LOCAL_VOICE_DEBUG=1`. Never called from the IOProc.
macro_rules! debug {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!("[local-voice] ducking: {}", format_args!($($arg)*));
        }
    };
}

// ─── Entry point ─────────────────────────────────────────────────────────────

/// Build the macOS backend, or `None` when process taps are unavailable
/// (macOS < 14.2, or the symbols/class cannot be resolved at runtime).
pub fn create(settings: &DuckingSettings) -> Option<Box<dyn Ducker>> {
    if !os_at_least(14, 2) {
        debug!("macOS < 14.2, process taps unavailable");
        return None;
    }
    // The class is looked up dynamically, so this is a cheap extra guard that
    // does not create a load-time dependency on the symbol.
    if AnyClass::get(c"CATapDescription").is_none() {
        debug!("CATapDescription class not found");
        return None;
    }
    let fns = TapFns::resolve()?;
    Some(Box::new(MacDucker {
        level: settings.level.clamp(0.0, 1.0),
        fade_ms: settings.fade_ms,
        fns,
        active: None,
    }))
}

fn os_at_least(major: isize, minor: isize) -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: major,
        minorVersion: minor,
        patchVersion: 0,
    })
}

// ─── Runtime-resolved tap functions ──────────────────────────────────────────

type CreateTapFn = unsafe extern "C-unwind" fn(*const c_void, *mut AudioObjectID) -> OSStatus;
type DestroyTapFn = unsafe extern "C-unwind" fn(AudioObjectID) -> OSStatus;

/// `AudioHardwareCreateProcessTap` / `AudioHardwareDestroyProcessTap`, found
/// with `dlsym` so that a binary linked against a new SDK still launches on
/// older systems where the symbols do not exist.
#[derive(Clone, Copy)]
struct TapFns {
    create: CreateTapFn,
    destroy: DestroyTapFn,
}

impl TapFns {
    fn resolve() -> Option<Self> {
        // SAFETY: plain dlsym lookups; CoreAudio is already loaded because we
        // link (and call) its other functions.
        unsafe {
            let create = libc::dlsym(
                libc::RTLD_DEFAULT,
                c"AudioHardwareCreateProcessTap".as_ptr(),
            );
            let destroy = libc::dlsym(
                libc::RTLD_DEFAULT,
                c"AudioHardwareDestroyProcessTap".as_ptr(),
            );
            if create.is_null() || destroy.is_null() {
                debug!("process tap symbols not found via dlsym");
                return None;
            }
            // SAFETY: the symbols have exactly these C signatures.
            Some(Self {
                create: std::mem::transmute::<*mut c_void, CreateTapFn>(create),
                destroy: std::mem::transmute::<*mut c_void, DestroyTapFn>(destroy),
            })
        }
    }
}

// ─── Small HAL helpers ───────────────────────────────────────────────────────

/// Render an `OSStatus` as its four-character code when printable.
fn fmt_status(status: OSStatus) -> String {
    let bytes = (status as u32).to_be_bytes();
    if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        format!("'{}' ({status})", String::from_utf8_lossy(&bytes))
    } else {
        status.to_string()
    }
}

fn check(status: OSStatus, what: &str) -> Result<()> {
    if status == kAudioHardwareNoError {
        Ok(())
    } else {
        Err(anyhow!("{what} failed with {}", fmt_status(status)))
    }
}

fn addr(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn system_object() -> AudioObjectID {
    kAudioObjectSystemObject as AudioObjectID
}

/// Read a fixed-size property. `qualifier` is `(ptr, size)` for properties
/// that need one (e.g. PID → process object translation).
fn get_prop<T: Copy>(
    obj: AudioObjectID,
    selector: u32,
    scope: u32,
    qualifier: Option<(*const c_void, u32)>,
) -> Result<T> {
    let mut address = addr(selector, scope);
    let mut size = size_of::<T>() as u32;
    let mut out = MaybeUninit::<T>::uninit();
    let (qptr, qsize) = qualifier.unwrap_or((ptr::null(), 0));
    // SAFETY: all pointers are valid for the duration of the call and `size`
    // matches the buffer we hand over.
    let status = unsafe {
        AudioObjectGetPropertyData(
            obj,
            NonNull::from(&mut address),
            qsize,
            qptr,
            NonNull::from(&mut size),
            NonNull::new(out.as_mut_ptr().cast::<c_void>()).expect("stack pointer"),
        )
    };
    check(
        status,
        &format!(
            "AudioObjectGetPropertyData({})",
            fmt_status(selector as i32)
        ),
    )?;
    if size as usize != size_of::<T>() {
        bail!(
            "property {} returned {size} bytes, expected {}",
            fmt_status(selector as i32),
            size_of::<T>()
        );
    }
    // SAFETY: the HAL filled exactly `size_of::<T>()` bytes.
    Ok(unsafe { out.assume_init() })
}

/// Read an array-valued property (stream lists, process lists, …).
fn get_prop_vec<T: Copy>(obj: AudioObjectID, selector: u32, scope: u32) -> Result<Vec<T>> {
    let mut address = addr(selector, scope);
    let mut size = 0u32;
    // SAFETY: valid pointers.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            obj,
            NonNull::from(&mut address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
        )
    };
    check(
        status,
        &format!(
            "AudioObjectGetPropertyDataSize({})",
            fmt_status(selector as i32)
        ),
    )?;
    let count = size as usize / size_of::<T>();
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut buf: Vec<MaybeUninit<T>> = vec![MaybeUninit::uninit(); count];
    let mut size = (count * size_of::<T>()) as u32;
    // SAFETY: `buf` is at least `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            obj,
            NonNull::from(&mut address),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(buf.as_mut_ptr().cast::<c_void>()).expect("vec pointer"),
        )
    };
    check(
        status,
        &format!(
            "AudioObjectGetPropertyData({})",
            fmt_status(selector as i32)
        ),
    )?;
    let count = size as usize / size_of::<T>();
    buf.truncate(count);
    // SAFETY: the first `count` elements were initialised by the HAL.
    Ok(buf
        .into_iter()
        .map(|m| unsafe { m.assume_init() })
        .collect())
}

/// Read a `CFStringRef` property and convert it to a Rust string. The HAL
/// hands us a +1 reference, which `CFRetained::from_raw` takes over.
fn get_prop_string(obj: AudioObjectID, selector: u32, scope: u32) -> Result<String> {
    let raw: *const CFString = get_prop(obj, selector, scope, None)?;
    let raw = NonNull::new(raw.cast_mut()).ok_or_else(|| {
        anyhow!(
            "property {} returned NULL string",
            fmt_status(selector as i32)
        )
    })?;
    // SAFETY: the HAL returned an owned CFString reference.
    let s = unsafe { CFRetained::from_raw(raw) };
    Ok(s.to_string())
}

fn ns_key(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().expect("CoreAudio keys are ASCII"))
}

// ─── State shared with the realtime IOProc ───────────────────────────────────

/// Everything the IOProc needs, all lock-free. Floats are stored as bit
/// patterns in `AtomicU32`.
struct Shared {
    /// Gain the smoother is heading for. Written by the control thread.
    target: AtomicU32,
    /// Gain applied to the most recent sample. Owned by the IOProc, published
    /// so the control thread can observe fade progress.
    current: AtomicU32,
    /// One-pole coefficient `1 - exp(-1 / (tau * sample_rate))`.
    coeff: AtomicU32,
    /// Number of IO cycles processed so far.
    callbacks: AtomicU64,
    /// Number of input frames processed so far.
    frames: AtomicU64,
    /// Set once *any* input sample was not exactly 0.0 — our only signal that
    /// the System Audio Recording permission was actually granted.
    nonzero: AtomicBool,
    /// Diagnostics: peak |input| so far, RMS of the last input / output block.
    in_peak: AtomicU32,
    last_in_rms: AtomicU32,
    last_out_rms: AtomicU32,
    /// Diagnostics: peak |input| per input channel (f32 bits).
    chan_peak: [AtomicU32; MAX_CHANNELS],
    /// Diagnostics: input / output frames of the most recent IO cycle.
    last_in_frames: AtomicU32,
    last_out_frames: AtomicU32,
    /// Bit `i` set ⇒ input buffer `i` (= aggregate input stream `i`) belongs
    /// to the tap. The aggregate also exposes the output device's *own*
    /// input streams (microphones, line-in); those must never reach the
    /// speakers. Fixed before the IOProc starts.
    in_mask: AtomicU64,
    /// Set by the property listener when the default output device changed
    /// while we were ducked.
    device_changed: AtomicBool,
}

impl Shared {
    fn new(coeff: f32) -> Arc<Self> {
        Arc::new(Self {
            target: AtomicU32::new(1.0f32.to_bits()),
            current: AtomicU32::new(1.0f32.to_bits()),
            coeff: AtomicU32::new(coeff.to_bits()),
            callbacks: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            nonzero: AtomicBool::new(false),
            in_peak: AtomicU32::new(0),
            last_in_rms: AtomicU32::new(0),
            last_out_rms: AtomicU32::new(0),
            chan_peak: std::array::from_fn(|_| AtomicU32::new(0)),
            last_in_frames: AtomicU32::new(0),
            last_out_frames: AtomicU32::new(0),
            // Nothing is routed until the stream layout has been verified.
            in_mask: AtomicU64::new(0),
            device_changed: AtomicBool::new(false),
        })
    }

    /// Per-channel input peaks seen so far (diagnostics).
    fn channel_peaks(&self, channels: usize) -> Vec<f32> {
        self.chan_peak
            .iter()
            .take(channels.min(MAX_CHANNELS))
            .map(|p| f32::from_bits(p.load(Ordering::Relaxed)))
            .collect()
    }

    fn set_target(&self, gain: f32) {
        self.target.store(gain.to_bits(), Ordering::Release);
    }

    fn current_gain(&self) -> f32 {
        f32::from_bits(self.current.load(Ordering::Acquire))
    }
}

/// One-pole coefficient so that the exponential fade is ~99 % complete after
/// `fade_ms` (five time constants). `fade_ms == 0` means "instant".
fn smoothing_coeff(fade_ms: u64, sample_rate: f64) -> f32 {
    if fade_ms == 0 || sample_rate <= 0.0 {
        return 1.0;
    }
    let tau_samples = (fade_ms as f64 / 1000.0) * sample_rate / 5.0;
    (1.0 - (-1.0 / tau_samples).exp()) as f32
}

/// Upper bound on channels we will map; anything beyond is left silent.
const MAX_CHANNELS: usize = 32;

/// A view of one channel inside an `AudioBufferList`: base pointer to the
/// first sample plus the stride (in samples) between consecutive frames.
/// Interleaved buffers have stride = channel count; non-interleaved have 1.
#[derive(Clone, Copy)]
struct ChannelView {
    ptr: *mut f32,
    stride: usize,
}

/// Flatten an `AudioBufferList` into per-channel views, considering only
/// buffers whose index bit is set in `mask` (one buffer per stream, in
/// stream-list order). Returns the number of channels and the number of
/// frames common to all selected buffers.
///
/// # Safety
/// `list` must point at a valid `AudioBufferList` with `mNumberBuffers`
/// trailing `AudioBuffer`s, each holding Float32 samples.
unsafe fn channel_views(
    list: NonNull<AudioBufferList>,
    mask: u64,
    out: &mut [ChannelView; MAX_CHANNELS],
) -> (usize, usize) {
    // AudioBufferList is a variable-length struct: `mBuffers[mNumberBuffers]`.
    // Take pointers from the raw pointer, not from a `&AudioBufferList`, so
    // that reads past the declared 1-element array are well-formed.
    let list = list.as_ptr();
    // SAFETY: caller guarantees `list` is valid.
    let n_buffers = unsafe { (*list).mNumberBuffers } as usize;
    let buffers = unsafe { ptr::addr_of_mut!((*list).mBuffers) }.cast::<AudioBuffer>();
    let mut channels = 0usize;
    let mut frames = usize::MAX;
    for i in 0..n_buffers.min(64) {
        if mask & (1u64 << i) == 0 {
            continue;
        }
        // SAFETY: `i < mNumberBuffers`.
        let buf = unsafe { &*buffers.add(i) };
        let ch = buf.mNumberChannels as usize;
        if ch == 0 || buf.mData.is_null() {
            continue;
        }
        let buf_frames = buf.mDataByteSize as usize / (ch * size_of::<f32>());
        frames = frames.min(buf_frames);
        let base = buf.mData.cast::<f32>();
        for c in 0..ch {
            if channels == MAX_CHANNELS {
                break;
            }
            // SAFETY: `c < ch`, within the buffer's first frame.
            out[channels] = ChannelView {
                ptr: unsafe { base.add(c) },
                stride: ch,
            };
            channels += 1;
        }
    }
    if frames == usize::MAX {
        frames = 0;
    }
    (channels, frames)
}

/// The realtime IO callback body. **Must stay realtime-safe**: no
/// allocation, no locks, no logging, no panics.
///
/// Copies the tap's input (only the buffers selected by `Shared::in_mask`)
/// to the device's output multiplied by a per-sample smoothed gain.
///
/// * Channel mapping: output channel `k` takes input channel
///   `k % in_channels`; a mono output gets the average of all input channels.
/// * Rate mapping: the tap may run at a different sample rate than the
///   output device (the aggregate exposes it at its own rate), in which case
///   an IO cycle carries a different number of input and output frames. We
///   linearly interpolate the input across the output block, which is
///   identity when the counts match.
///
/// # Safety
/// Both lists must be valid Float32 buffer lists provided by the HAL.
unsafe fn process(
    shared: &Shared,
    input: NonNull<AudioBufferList>,
    output: NonNull<AudioBufferList>,
) {
    let null = ChannelView {
        ptr: ptr::null_mut(),
        stride: 1,
    };
    let mut ins = [null; MAX_CHANNELS];
    let mut outs = [null; MAX_CHANNELS];
    let in_mask = shared.in_mask.load(Ordering::Relaxed);
    // SAFETY: forwarded from the caller's guarantee.
    let (in_ch, in_frames) = unsafe { channel_views(input, in_mask, &mut ins) };
    let (out_ch, out_frames) = unsafe { channel_views(output, u64::MAX, &mut outs) };

    let target = f32::from_bits(shared.target.load(Ordering::Acquire));
    let coeff = f32::from_bits(shared.coeff.load(Ordering::Relaxed));
    let mut gain = f32::from_bits(shared.current.load(Ordering::Relaxed));

    // ── Pass 1: input statistics (silence detection + diagnostics).
    let mut any_nonzero = false;
    let mut peak = 0f32;
    let mut chan_peak = [0f32; MAX_CHANNELS];
    let mut in_sq = 0f32;
    for (c, view) in ins.iter().enumerate().take(in_ch) {
        let mut cp = 0f32;
        for f in 0..in_frames {
            // SAFETY: `f < in_frames`, which is within every selected buffer.
            let s = unsafe { *view.ptr.add(f * view.stride) };
            any_nonzero |= s != 0.0;
            let a = s.abs();
            if a > cp {
                cp = a;
            }
            in_sq += s * s;
        }
        chan_peak[c] = cp;
        if cp > peak {
            peak = cp;
        }
    }

    // ── Pass 2: render input → output with gain (and resampling if needed).
    let mut out_sq = 0f32;
    if in_ch > 0 && out_ch > 0 && in_frames > 0 && out_frames > 0 {
        let mono_out = out_ch == 1 && in_ch > 1;
        let inv_in_ch = 1.0 / in_ch as f32;
        let ratio = in_frames as f32 / out_frames as f32;
        let last = in_frames - 1;
        for f in 0..out_frames {
            gain += coeff * (target - gain);
            // Source position for this output frame (identity when ratio == 1).
            let pos = f as f32 * ratio;
            let i0 = (pos as usize).min(last);
            let i1 = (i0 + 1).min(last);
            let frac = pos - i0 as f32;
            // Interpolated input sample of channel `view`.
            let sample = |view: &ChannelView| -> f32 {
                // SAFETY: `i0, i1 <= last < in_frames`.
                let a = unsafe { *view.ptr.add(i0 * view.stride) };
                let b = unsafe { *view.ptr.add(i1 * view.stride) };
                a + (b - a) * frac
            };
            if mono_out {
                let mut mix = 0f32;
                for view in ins.iter().take(in_ch) {
                    mix += sample(view);
                }
                let v = mix * inv_in_ch * gain;
                out_sq += v * v;
                let o = outs[0];
                // SAFETY: `f < out_frames`.
                unsafe { *o.ptr.add(f * o.stride) = v };
            } else {
                for (k, o) in outs.iter().enumerate().take(out_ch) {
                    let v = sample(&ins[k % in_ch]) * gain;
                    out_sq += v * v;
                    // SAFETY: `f < out_frames`.
                    unsafe { *o.ptr.add(f * o.stride) = v };
                }
            }
        }
    } else {
        // Nothing usable to render this cycle (transient stream hiccup). The
        // HAL does not guarantee zeroed output buffers, and the tapped apps
        // are muted at the hardware, so emit silence rather than stale data.
        for o in outs.iter().take(out_ch) {
            for f in 0..out_frames {
                // SAFETY: `f < out_frames`.
                unsafe { *o.ptr.add(f * o.stride) = 0.0 };
            }
        }
    }

    // Publish state. Stores only; `fetch_max` on the bit pattern of a
    // non-negative float orders correctly because IEEE-754 positive values
    // sort like their integer representation.
    shared.current.store(gain.to_bits(), Ordering::Release);
    shared.callbacks.fetch_add(1, Ordering::Relaxed);
    shared.frames.fetch_add(in_frames as u64, Ordering::Relaxed);
    shared
        .last_in_frames
        .store(in_frames as u32, Ordering::Relaxed);
    shared
        .last_out_frames
        .store(out_frames as u32, Ordering::Relaxed);
    if any_nonzero {
        shared.nonzero.store(true, Ordering::Release);
    }
    shared.in_peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
    for (slot, p) in shared.chan_peak.iter().zip(chan_peak.iter()).take(in_ch) {
        slot.fetch_max(p.to_bits(), Ordering::Relaxed);
    }
    if in_frames > 0 && in_ch > 0 {
        let in_rms = (in_sq / (in_frames * in_ch) as f32).sqrt();
        let out_rms = (out_sq / (out_frames.max(1) * out_ch.max(1)) as f32).sqrt();
        shared
            .last_in_rms
            .store(in_rms.to_bits(), Ordering::Relaxed);
        shared
            .last_out_rms
            .store(out_rms.to_bits(), Ordering::Relaxed);
    }
}

type IoBlockFn = dyn Fn(
    NonNull<AudioTimeStamp>,
    NonNull<AudioBufferList>,
    NonNull<AudioTimeStamp>,
    NonNull<AudioBufferList>,
    NonNull<AudioTimeStamp>,
);

/// Keeps our reference to the IO block alive for the lifetime of the IOProc.
/// The HAL copies (retains) the block itself, but holding our own reference
/// until after `AudioDeviceDestroyIOProcID` is the conservative choice.
struct IoBlock(RcBlock<IoBlockFn>);

// SAFETY: the closure only captures an `Arc<Shared>` (Send + Sync) and the
// block is never invoked from Rust; we merely move the handle between
// threads and drop it.
unsafe impl Send for IoBlock {}

fn make_io_block(shared: Arc<Shared>) -> IoBlock {
    IoBlock(RcBlock::new(
        move |_now: NonNull<AudioTimeStamp>,
              input: NonNull<AudioBufferList>,
              _in_time: NonNull<AudioTimeStamp>,
              output: NonNull<AudioBufferList>,
              _out_time: NonNull<AudioTimeStamp>| {
            // SAFETY: the HAL passes valid buffer lists in Float32 (verified
            // against the streams' virtual formats before the IOProc starts).
            unsafe { process(&shared, input, output) }
        },
    ))
}

/// Property listener installed on the system object for the default output
/// device. Only flips a flag; teardown happens on the control thread.
unsafe extern "C-unwind" fn default_output_changed(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    client_data: *mut c_void,
) -> OSStatus {
    if !client_data.is_null() {
        // SAFETY: `client_data` is the `Arc<Shared>` payload, kept alive
        // until the listener is removed.
        let shared = unsafe { &*client_data.cast::<Shared>() };
        shared.device_changed.store(true, Ordering::Release);
    }
    0
}

// ─── Active duck session ─────────────────────────────────────────────────────

/// Resources of one duck session. Fields are filled in order during
/// [`MacDucker::start`] and released in reverse by [`Active::teardown`], so a
/// half-constructed session can always be cleaned up.
struct Active {
    shared: Arc<Shared>,
    tap_id: AudioObjectID,
    aggregate_id: AudioObjectID,
    io_proc: AudioDeviceIOProcID,
    _block: Option<IoBlock>,
    started: bool,
    listener_installed: bool,
}

impl Active {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            tap_id: kAudioObjectUnknown,
            aggregate_id: kAudioObjectUnknown,
            io_proc: None,
            _block: None,
            started: false,
            listener_installed: false,
        }
    }

    fn listener_address() -> AudioObjectPropertyAddress {
        addr(
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
        )
    }

    /// Release everything, best effort, in the right order. Safe to call on
    /// a partially built session and more than once.
    fn teardown(&mut self, fns: &TapFns) {
        // SAFETY: each call only uses handles this session created and has
        // not yet destroyed; every handle is cleared right after use.
        unsafe {
            if self.started {
                let st = AudioDeviceStop(self.aggregate_id, self.io_proc);
                debug!("AudioDeviceStop -> {}", fmt_status(st));
                self.started = false;
            }
            if self.io_proc.is_some() {
                let st = AudioDeviceDestroyIOProcID(self.aggregate_id, self.io_proc);
                debug!("AudioDeviceDestroyIOProcID -> {}", fmt_status(st));
                self.io_proc = None;
            }
            // The HAL no longer holds the block; drop our reference too.
            self._block = None;
            if self.listener_installed {
                let mut address = Self::listener_address();
                let st = AudioObjectRemovePropertyListener(
                    system_object(),
                    NonNull::from(&mut address),
                    Some(default_output_changed),
                    Arc::as_ptr(&self.shared).cast_mut().cast::<c_void>(),
                );
                debug!("AudioObjectRemovePropertyListener -> {}", fmt_status(st));
                self.listener_installed = false;
            }
            if self.aggregate_id != kAudioObjectUnknown {
                let st = AudioHardwareDestroyAggregateDevice(self.aggregate_id);
                debug!("AudioHardwareDestroyAggregateDevice -> {}", fmt_status(st));
                self.aggregate_id = kAudioObjectUnknown;
            }
            if self.tap_id != kAudioObjectUnknown {
                // With `MutedWhenTapped`, destroying the tap returns the apps
                // to direct hardware output.
                let st = (fns.destroy)(self.tap_id);
                debug!("AudioHardwareDestroyProcessTap -> {}", fmt_status(st));
                self.tap_id = kAudioObjectUnknown;
            }
        }
    }
}

// ─── The ducker ──────────────────────────────────────────────────────────────

struct MacDucker {
    level: f32,
    fade_ms: u64,
    fns: TapFns,
    active: Option<Active>,
}

/// How long we are willing to wait for the aggregate device to become ready
/// (streams present) after creation.
const AGGREGATE_READY_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long we are willing to wait for the first IO callback after start.
const FIRST_CALLBACK_TIMEOUT: Duration = Duration::from_millis(2000);
/// Amount of tapped audio (in seconds) that must be all-zero before we
/// conclude the permission was silently denied.
const ZERO_CHECK_SECONDS: f64 = 0.5;

const PERMISSION_HINT: &str = "no audio was captured from other apps. macOS may have silently denied \
     the \"System Audio Recording\" permission: open System Settings → Privacy & Security → \
     Screen & System Audio Recording (or System Audio Recording) and enable it for the app that \
     launched local-voice (your terminal, editor, or agent host)";

impl MacDucker {
    /// Number of *other* processes currently producing output. Zero means
    /// there is nothing to duck and, more importantly, no way to tell a
    /// silent tap from a denied permission.
    fn other_output_processes(&self) -> Result<usize> {
        let me = unsafe { libc::getpid() };
        let procs: Vec<AudioObjectID> = get_prop_vec(
            system_object(),
            kAudioHardwarePropertyProcessObjectList,
            kAudioObjectPropertyScopeGlobal,
        )?;
        let mut count = 0;
        for p in procs {
            let pid: libc::pid_t = match get_prop(
                p,
                kAudioProcessPropertyPID,
                kAudioObjectPropertyScopeGlobal,
                None,
            ) {
                Ok(pid) => pid,
                Err(_) => continue,
            };
            if pid == me {
                continue;
            }
            let running: u32 = get_prop(
                p,
                kAudioProcessPropertyIsRunningOutput,
                kAudioObjectPropertyScopeGlobal,
                None,
            )
            .unwrap_or(0);
            if running != 0 {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Translate our PID into a HAL process object so the tap can exclude it.
    fn own_process_object(&self) -> Result<AudioObjectID> {
        let pid: libc::pid_t = unsafe { libc::getpid() };
        get_prop(
            system_object(),
            kAudioHardwarePropertyTranslatePIDToProcessObject,
            kAudioObjectPropertyScopeGlobal,
            Some((
                ptr::from_ref(&pid).cast::<c_void>(),
                size_of::<libc::pid_t>() as u32,
            )),
        )
        .context("translating our PID to a process object")
    }

    /// Build the whole tap → aggregate → IOProc chain and fade down.
    /// `Ok(None)` means nothing is playing, so nothing was set up.
    fn start(&mut self) -> Result<Option<Active>> {
        let others = self.other_output_processes()?;
        if others == 0 {
            debug!("no other process is producing output; skipping duck");
            return Ok(None);
        }
        debug!("{others} other process(es) producing output");

        // ── 1. Tap: everything except us, muted at the hardware while read.
        let own = self.own_process_object()?;
        let exclude = NSArray::from_retained_slice(&[NSNumber::new_u32(own)]);
        // SAFETY: standard ObjC init / property setters on a fresh object.
        let desc = unsafe {
            let desc = CATapDescription::initStereoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &exclude,
            );
            desc.setName(&NSString::from_str("local-voice ducking"));
            desc.setMuteBehavior(CATapMuteBehavior::MutedWhenTapped);
            desc.setPrivate(true);
            desc
        };

        // Default output device *before* we create anything that might
        // change what "default" means (private aggregates do not, but be safe).
        let output_device: AudioObjectID = get_prop(
            system_object(),
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
            None,
        )?;
        if output_device == kAudioObjectUnknown {
            bail!("no default output device");
        }
        let output_uid = get_prop_string(
            output_device,
            kAudioDevicePropertyDeviceUID,
            kAudioObjectPropertyScopeGlobal,
        )?;
        debug!("default output device {output_device} uid={output_uid:?}");

        // Placeholder coefficient; replaced once we know the sample rate.
        let shared = Shared::new(1.0);
        let mut active = Active::new(Arc::clone(&shared));

        let result = self.start_inner(&mut active, &desc, output_device, &output_uid);
        drop(desc);
        match result {
            Ok(()) => Ok(Some(active)),
            Err(e) => {
                active.teardown(&self.fns);
                Err(e)
            }
        }
    }

    fn start_inner(
        &mut self,
        active: &mut Active,
        desc: &CATapDescription,
        output_device: AudioObjectID,
        output_uid: &str,
    ) -> Result<()> {
        let shared = Arc::clone(&active.shared);

        // SAFETY: `desc` is a live CATapDescription; the fn was resolved via dlsym.
        let mut tap_id: AudioObjectID = kAudioObjectUnknown;
        let st = unsafe { (self.fns.create)(ptr::from_ref(desc).cast::<c_void>(), &mut tap_id) };
        check(st, "AudioHardwareCreateProcessTap")?;
        active.tap_id = tap_id;
        let tap_uid = get_prop_string(
            tap_id,
            kAudioTapPropertyUID,
            kAudioObjectPropertyScopeGlobal,
        )?;
        if let Ok(fmt) = get_prop::<AudioStreamBasicDescription>(
            tap_id,
            kAudioTapPropertyFormat,
            kAudioObjectPropertyScopeGlobal,
            None,
        ) {
            debug!(
                "tap {tap_id} uid={tap_uid:?} format: {} Hz, {} ch, flags {:#x}, {} bits",
                fmt.mSampleRate, fmt.mChannelsPerFrame, fmt.mFormatFlags, fmt.mBitsPerChannel
            );
        }

        // ── 2. Private aggregate: [default output] + [tap], tap auto-starts.
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let agg_uid = format!(
            "com.frenkiee.local-voice.ducking.{}.{}",
            unsafe { libc::getpid() },
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let one = NSNumber::new_u32(1);
        let output_uid_ns = NSString::from_str(output_uid);
        let tap_uid_ns = NSString::from_str(&tap_uid);

        let sub_device = {
            let key = ns_key(kAudioSubDeviceUIDKey);
            let value: &AnyObject = &output_uid_ns;
            NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[value])
        };
        let sub_tap = {
            let keys = [
                ns_key(kAudioSubTapUIDKey),
                ns_key(kAudioSubTapDriftCompensationKey),
            ];
            let tap_uid_obj: &AnyObject = &tap_uid_ns;
            let one_obj: &AnyObject = &one;
            NSDictionary::<NSString, AnyObject>::from_slices(
                &[&*keys[0], &*keys[1]],
                &[tap_uid_obj, one_obj],
            )
        };
        let sub_devices = NSArray::from_retained_slice(&[sub_device]);
        let taps = NSArray::from_retained_slice(&[sub_tap]);
        let name_ns = NSString::from_str("local-voice ducking");
        let agg_uid_ns = NSString::from_str(&agg_uid);

        let description = {
            let keys = [
                ns_key(kAudioAggregateDeviceNameKey),
                ns_key(kAudioAggregateDeviceUIDKey),
                ns_key(kAudioAggregateDeviceIsPrivateKey),
                ns_key(kAudioAggregateDeviceTapAutoStartKey),
                ns_key(kAudioAggregateDeviceMainSubDeviceKey),
                ns_key(kAudioAggregateDeviceSubDeviceListKey),
                ns_key(kAudioAggregateDeviceTapListKey),
            ];
            let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
            let values: [&AnyObject; 7] = [
                &name_ns,
                &agg_uid_ns,
                &one,
                &one,
                &output_uid_ns,
                &sub_devices,
                &taps,
            ];
            NSDictionary::<NSString, AnyObject>::from_slices(&key_refs, &values)
        };

        let mut aggregate_id: AudioObjectID = kAudioObjectUnknown;
        // SAFETY: NSDictionary is toll-free bridged to CFDictionary; the
        // object stays alive for the call.
        let st = unsafe {
            let cf: &CFDictionary = &*Retained::as_ptr(&description).cast::<CFDictionary>();
            AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate_id))
        };
        check(st, "AudioHardwareCreateAggregateDevice")?;
        active.aggregate_id = aggregate_id;
        debug!("aggregate device {aggregate_id} uid={agg_uid:?}");

        // ── 3. Wait until the aggregate exposes both the tap input streams and
        // the device output streams. Starting the IOProc before that point
        // silently produces no callbacks.
        let (in_streams, out_streams) = wait_for_streams(aggregate_id)?;
        dump_aggregate(aggregate_id, &in_streams, &out_streams);
        let in_ch = check_float32_streams(&in_streams).context("aggregate input streams")?;
        let out_ch = check_float32_streams(&out_streams).context("output streams")?;
        let sample_rate: f64 = get_prop(
            aggregate_id,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
            None,
        )
        .unwrap_or(48_000.0);
        debug!("aggregate ready: {in_ch} in ch / {out_ch} out ch @ {sample_rate} Hz");
        shared.coeff.store(
            smoothing_coeff(self.fade_ms, sample_rate).to_bits(),
            Ordering::Release,
        );

        // ── 3b. Separate the tap's input streams from the output device's own
        // input streams (microphone / line-in on interfaces such as USB audio
        // boxes). Those must never be routed to the speakers (feedback!) and
        // must not count as "audio captured" for the permission check.
        let tap_channels = get_prop::<AudioStreamBasicDescription>(
            tap_id,
            kAudioTapPropertyFormat,
            kAudioObjectPropertyScopeGlobal,
            None,
        )
        .map(|f| f.mChannelsPerFrame)
        .context("reading tap format")?;
        let device_in_channels = input_channel_count(output_device);
        let layout = classify_input_streams(&in_streams, device_in_channels)?;
        if layout.tap_channels != tap_channels || layout.device_channels != device_in_channels {
            bail!(
                "unexpected aggregate stream layout: found {} tap / {} device input channels, \
                 expected {tap_channels} / {device_in_channels}",
                layout.tap_channels,
                layout.device_channels
            );
        }
        debug!(
            "input layout: tap streams mask {:#b} ({} ch), device input {} ch",
            layout.mask, layout.tap_channels, layout.device_channels
        );
        shared.in_mask.store(layout.mask, Ordering::Release);

        // ── 4. IOProc with our realtime gain block.
        let block = make_io_block(Arc::clone(&shared));
        let mut io_proc: AudioDeviceIOProcID = None;
        // SAFETY: the block pointer is valid; the HAL copies it.
        let st = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut io_proc),
                aggregate_id,
                None,
                RcBlock::as_ptr(&block.0),
            )
        };
        check(st, "AudioDeviceCreateIOProcIDWithBlock")?;
        active.io_proc = io_proc;
        active._block = Some(block);
        // Tell the HAL our IOProc does not use the device's own input streams,
        // so it does not run them (and does not light the microphone-in-use
        // indicator). Best effort: the mask in `process` is the real guard.
        set_input_stream_usage(aggregate_id, io_proc, in_streams.len(), layout.mask);

        // Watch for the default output device changing under us.
        let mut address = Active::listener_address();
        // SAFETY: the client data pointer is kept alive by `active.shared`
        // until the listener is removed in teardown.
        let st = unsafe {
            AudioObjectAddPropertyListener(
                system_object(),
                NonNull::from(&mut address),
                Some(default_output_changed),
                Arc::as_ptr(&shared).cast_mut().cast::<c_void>(),
            )
        };
        active.listener_installed = st == kAudioHardwareNoError;

        // ── 5. Go. From this point the tapped apps are muted at the hardware
        // and only our copy is heard, starting at unity gain.
        // SAFETY: valid device + IOProc handles.
        let st = unsafe { AudioDeviceStart(aggregate_id, io_proc) };
        check(st, "AudioDeviceStart")?;
        active.started = true;
        shared.set_target(self.level);

        // ── 6. Wait for the fade to finish while checking that real audio
        // arrives. Silent denial of the TCC permission looks exactly like a
        // working tap that only ever delivers zeros.
        let fade = Duration::from_millis(self.fade_ms);
        let need_frames = (sample_rate * ZERO_CHECK_SECONDS) as u64;
        let t0 = Instant::now();
        loop {
            let elapsed = t0.elapsed();
            let frames = shared.frames.load(Ordering::Relaxed);
            let nonzero = shared.nonzero.load(Ordering::Acquire);
            if frames == 0 && elapsed > FIRST_CALLBACK_TIMEOUT {
                bail!(
                    "aggregate device produced no IO callbacks within {FIRST_CALLBACK_TIMEOUT:?}"
                );
            }
            if shared.device_changed.load(Ordering::Acquire) {
                bail!("default output device changed while starting to duck");
            }
            if elapsed >= fade && (nonzero || frames >= need_frames) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        debug!(
            "after {:?}: {} callbacks, last cycle {} in / {} out frames, tap channel peaks {:?}",
            t0.elapsed(),
            shared.callbacks.load(Ordering::Relaxed),
            shared.last_in_frames.load(Ordering::Relaxed),
            shared.last_out_frames.load(Ordering::Relaxed),
            shared.channel_peaks(tap_channels as usize)
        );
        if !shared.nonzero.load(Ordering::Acquire) {
            bail!("{PERMISSION_HINT}");
        }
        debug!(
            "ducked to {:.2} (gain now {:.3})",
            self.level,
            shared.current_gain()
        );
        Ok(())
    }
}

/// Human-readable summary of a stream for `LOCAL_VOICE_DEBUG` output.
fn describe_stream(s: AudioObjectID) -> String {
    let g = kAudioObjectPropertyScopeGlobal;
    let owner: AudioObjectID = get_prop(s, kAudioObjectPropertyOwner, g, None).unwrap_or(0);
    let name = get_prop_string(s, kAudioObjectPropertyName, g).unwrap_or_default();
    let terminal: u32 = get_prop(s, kAudioStreamPropertyTerminalType, g, None).unwrap_or(0);
    let start: u32 = get_prop(s, kAudioStreamPropertyStartingChannel, g, None).unwrap_or(0);
    let (ch, rate) =
        get_prop::<AudioStreamBasicDescription>(s, kAudioStreamPropertyVirtualFormat, g, None)
            .map(|f| (f.mChannelsPerFrame, f.mSampleRate))
            .unwrap_or((0, 0.0));
    format!(
        "stream {s}: owner={owner} name={name:?} terminal={} start_ch={start} ch={ch} rate={rate}",
        fmt_status(terminal as i32)
    )
}

/// Dump the aggregate's composition (streams, sub-devices, sub-taps) for
/// debugging channel-layout questions. Only runs with `LOCAL_VOICE_DEBUG=1`.
fn dump_aggregate(
    aggregate_id: AudioObjectID,
    in_streams: &[AudioObjectID],
    out_streams: &[AudioObjectID],
) {
    if !debug_enabled() {
        return;
    }
    let g = kAudioObjectPropertyScopeGlobal;
    for s in in_streams {
        debug!("aggregate input  {}", describe_stream(*s));
    }
    for s in out_streams {
        debug!("aggregate output {}", describe_stream(*s));
    }
    for (label, selector) in [
        (
            "sub-device",
            kAudioAggregateDevicePropertyActiveSubDeviceList,
        ),
        ("sub-tap", kAudioAggregateDevicePropertySubTapList),
    ] {
        let objs = get_prop_vec::<AudioObjectID>(aggregate_id, selector, g).unwrap_or_default();
        for o in objs {
            let name = get_prop_string(o, kAudioObjectPropertyName, g).unwrap_or_default();
            let owned = get_prop_vec::<AudioObjectID>(o, kAudioObjectPropertyOwnedObjects, g)
                .unwrap_or_default();
            let ins = get_prop_vec::<AudioObjectID>(
                o,
                kAudioDevicePropertyStreams,
                kAudioObjectPropertyScopeInput,
            )
            .unwrap_or_default();
            debug!("{label} {o} name={name:?} owned={owned:?} input_streams={ins:?}");
        }
    }
}

/// Total input channels of a device (0 when it has no input side).
fn input_channel_count(device: AudioObjectID) -> u32 {
    get_prop_vec::<AudioObjectID>(
        device,
        kAudioDevicePropertyStreams,
        kAudioObjectPropertyScopeInput,
    )
    .unwrap_or_default()
    .iter()
    .filter_map(|&s| {
        get_prop::<AudioStreamBasicDescription>(
            s,
            kAudioStreamPropertyVirtualFormat,
            kAudioObjectPropertyScopeGlobal,
            None,
        )
        .ok()
    })
    .map(|f| f.mChannelsPerFrame)
    .sum()
}

/// Which of the aggregate's input streams belong to the tap.
struct InputLayout {
    /// Bit `i` set ⇒ input stream / buffer `i` is a tap stream.
    mask: u64,
    tap_channels: u32,
    device_channels: u32,
}

/// The aggregate lays out its sub-device's input streams first, followed by
/// the tap's, so a stream whose starting channel lies beyond the device's
/// input channel count is the tap's. The caller cross-checks the channel
/// totals against the tap format and the device, and refuses to run when
/// they disagree.
fn classify_input_streams(
    streams: &[AudioObjectID],
    device_in_channels: u32,
) -> Result<InputLayout> {
    if streams.len() > 64 {
        bail!(
            "aggregate has {} input streams; at most 64 are supported",
            streams.len()
        );
    }
    let mut layout = InputLayout {
        mask: 0,
        tap_channels: 0,
        device_channels: 0,
    };
    for (i, &s) in streams.iter().enumerate() {
        let g = kAudioObjectPropertyScopeGlobal;
        let start: u32 = get_prop(s, kAudioStreamPropertyStartingChannel, g, None)
            .with_context(|| format!("starting channel of stream {s}"))?;
        let channels =
            get_prop::<AudioStreamBasicDescription>(s, kAudioStreamPropertyVirtualFormat, g, None)
                .with_context(|| format!("format of stream {s}"))?
                .mChannelsPerFrame;
        if start > device_in_channels {
            layout.mask |= 1u64 << i;
            layout.tap_channels += channels;
        } else {
            layout.device_channels += channels;
        }
    }
    Ok(layout)
}

/// Declare which input streams our IOProc uses (`kAudioDevicePropertyIOProcStreamUsage`).
/// Best effort; failures are only logged.
fn set_input_stream_usage(
    aggregate_id: AudioObjectID,
    io_proc: AudioDeviceIOProcID,
    n_streams: usize,
    mask: u64,
) {
    let Some(proc_fn) = io_proc else { return };
    // Variable-length C struct:
    //   struct { void* mIOProc; UInt32 mNumberStreams; UInt32 mStreamIsOn[n]; }
    let bytes = 8 + 4 + 4 * n_streams;
    let mut buf = vec![0u64; bytes.div_ceil(8)];
    let p = buf.as_mut_ptr().cast::<u8>();
    // SAFETY: all writes are within `buf` (`bytes` ≤ its length).
    unsafe {
        p.cast::<*const c_void>()
            .write_unaligned(proc_fn as *const c_void);
        p.add(8).cast::<u32>().write_unaligned(n_streams as u32);
        for i in 0..n_streams {
            p.add(12 + 4 * i)
                .cast::<u32>()
                .write_unaligned(((mask >> i) & 1) as u32);
        }
    }
    let mut address = addr(
        kAudioDevicePropertyIOProcStreamUsage,
        kAudioObjectPropertyScopeInput,
    );
    // SAFETY: valid pointers; `bytes` matches the struct we filled.
    let st = unsafe {
        AudioObjectSetPropertyData(
            aggregate_id,
            NonNull::from(&mut address),
            0,
            ptr::null(),
            bytes as u32,
            NonNull::new(p.cast::<c_void>()).expect("vec pointer"),
        )
    };
    debug!(
        "IOProcStreamUsage(input, mask {mask:#b}) -> {}",
        fmt_status(st)
    );
}

/// Poll until the aggregate reports at least one input and one output
/// stream, returning both lists.
fn wait_for_streams(
    aggregate_id: AudioObjectID,
) -> Result<(Vec<AudioObjectID>, Vec<AudioObjectID>)> {
    let t0 = Instant::now();
    loop {
        let ins = get_prop_vec::<AudioObjectID>(
            aggregate_id,
            kAudioDevicePropertyStreams,
            kAudioObjectPropertyScopeInput,
        )
        .unwrap_or_default();
        let outs = get_prop_vec::<AudioObjectID>(
            aggregate_id,
            kAudioDevicePropertyStreams,
            kAudioObjectPropertyScopeOutput,
        )
        .unwrap_or_default();
        if !ins.is_empty() && !outs.is_empty() {
            return Ok((ins, outs));
        }
        if t0.elapsed() > AGGREGATE_READY_TIMEOUT {
            bail!(
                "aggregate device not ready after {AGGREGATE_READY_TIMEOUT:?} ({} input / {} output streams)",
                ins.len(),
                outs.len()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Verify every stream's *virtual* format (what the IOProc sees) is 32-bit
/// float linear PCM, and return the total channel count.
fn check_float32_streams(streams: &[AudioObjectID]) -> Result<u32> {
    let mut channels = 0;
    for &s in streams {
        let f: AudioStreamBasicDescription = get_prop(
            s,
            kAudioStreamPropertyVirtualFormat,
            kAudioObjectPropertyScopeGlobal,
            None,
        )?;
        let is_f32 = f.mFormatID == kAudioFormatLinearPCM
            && f.mFormatFlags & kAudioFormatFlagIsFloat != 0
            && f.mBitsPerChannel == 32;
        if !is_f32 {
            bail!(
                "stream {s} is not Float32 PCM (format {}, flags {:#x}, {} bits)",
                fmt_status(f.mFormatID as i32),
                f.mFormatFlags,
                f.mBitsPerChannel
            );
        }
        channels += f.mChannelsPerFrame;
    }
    Ok(channels)
}

impl Ducker for MacDucker {
    fn duck(&mut self) -> Result<()> {
        if self.active.is_some() {
            return Ok(());
        }
        self.active = self.start()?;
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        let Some(mut active) = self.active.take() else {
            return Ok(());
        };
        active.shared.set_target(1.0);
        if active.shared.device_changed.load(Ordering::Acquire) {
            debug!("default output device changed mid-duck; tearing down without fade");
        } else {
            // Wait for the ramp; bail out early if callbacks stopped coming.
            let fade = Duration::from_millis(self.fade_ms);
            let t0 = Instant::now();
            let start_cb = active.shared.callbacks.load(Ordering::Relaxed);
            while t0.elapsed() < fade {
                std::thread::sleep(Duration::from_millis(10));
                if active.shared.current_gain() >= 0.999 {
                    break;
                }
                if t0.elapsed() > Duration::from_millis(200)
                    && active.shared.callbacks.load(Ordering::Relaxed) == start_cb
                {
                    debug!("no IO callbacks during restore; device gone?");
                    break;
                }
            }
        }
        debug!("restoring (gain {:.3})", active.shared.current_gain());
        active.teardown(&self.fns);
        Ok(())
    }
}

impl Drop for MacDucker {
    fn drop(&mut self) {
        if let Some(mut active) = self.active.take() {
            active.teardown(&self.fns);
        }
    }
}

// ─── Self-test (`local-voice doctor`) ────────────────────────────────────────

pub const BACKEND_NAME: &str = "Core Audio process tap (macOS 14.2+)";

/// How long we give `afplay` to open its output stream and show up as a
/// process producing output before we conclude nothing is playing.
const PROBE_PLAYER_TIMEOUT: Duration = Duration::from_millis(2500);

/// Write a stereo 440 Hz, -10 dBFS, 16-bit/48 kHz tone of `seconds` to `path`.
fn write_tone_wav(path: &std::path::Path, seconds: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for n in 0..(48_000 * seconds) {
        let t = n as f32 / 48_000.0;
        let v = (0.316 * (2.0 * std::f32::consts::PI * 440.0 * t).sin() * i16::MAX as f32) as i16;
        w.write_sample(v)?;
        w.write_sample(v)?;
    }
    w.finalize()?;
    Ok(())
}

/// Ducking self-test: play a tone with `afplay` (a separate process, so the
/// tap has something foreign to capture), duck it, hold, restore.
///
/// Unlike [`super::duck`] this ignores and never starts the failure cooldown:
/// it is an explicit user action whose whole point is to retry.
pub fn probe() -> ProbeResult {
    use std::process::{Command, Stdio};

    if !os_at_least(14, 2) {
        let v = NSProcessInfo::processInfo().operatingSystemVersion();
        return ProbeResult::Unsupported(format!(
            "macOS {}.{}.{} is older than 14.2, which introduced process taps",
            v.majorVersion, v.minorVersion, v.patchVersion
        ));
    }
    if AnyClass::get(c"CATapDescription").is_none() {
        return ProbeResult::Unsupported("CATapDescription class not found".into());
    }
    let Some(fns) = TapFns::resolve() else {
        return ProbeResult::Unsupported(
            "AudioHardwareCreateProcessTap is not exported by CoreAudio".into(),
        );
    };

    let wav = std::env::temp_dir().join(format!(
        "local-voice-ducking-probe-{}.wav",
        std::process::id()
    ));
    if let Err(e) = write_tone_wav(&wav, 4) {
        return ProbeResult::Failed(format!("cannot write test tone {}: {e:#}", wav.display()));
    }
    let mut player = match Command::new("afplay")
        .arg(&wav)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_file(&wav);
            return ProbeResult::Failed(format!("cannot run afplay to play a test tone: {e}"));
        }
    };

    let mut ducker = MacDucker {
        level: PROBE_SETTINGS.level,
        fade_ms: PROBE_SETTINGS.fade_ms,
        fns,
        active: None,
    };

    // Wait for afplay to actually start producing output; `start()` would
    // otherwise (correctly) see nothing to duck.
    let t0 = Instant::now();
    let mut playing = false;
    while t0.elapsed() < PROBE_PLAYER_TIMEOUT {
        match ducker.other_output_processes() {
            Ok(n) if n > 0 => {
                playing = true;
                break;
            }
            Ok(_) => {}
            Err(e) => {
                let _ = player.kill();
                let _ = player.wait();
                let _ = std::fs::remove_file(&wav);
                return ProbeResult::Failed(format!("cannot list audio processes: {e:#}"));
            }
        }
        if let Ok(Some(_)) = player.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // A little extra so the first tapped buffers are not the fade-in.
    std::thread::sleep(Duration::from_millis(150));

    let outcome = if !playing {
        ProbeResult::NothingPlaying
    } else {
        match ducker.start() {
            Ok(None) => ProbeResult::NothingPlaying,
            Ok(Some(active)) => {
                ducker.active = Some(active);
                std::thread::sleep(PROBE_HOLD);
                match ducker.restore() {
                    Ok(()) => ProbeResult::Ok,
                    Err(e) => ProbeResult::Failed(format!("restore failed: {e:#}")),
                }
            }
            Err(e) => {
                let _ = ducker.restore();
                let msg = format!("{e:#}");
                if msg.contains("System Audio Recording") {
                    ProbeResult::Failed(
                        "the test tone was playing but the tap captured only silence: macOS has \
                         most likely denied the \"System Audio Recording\" permission to the app \
                         that launched local-voice"
                            .into(),
                    )
                } else {
                    ProbeResult::Failed(msg)
                }
            }
        }
    };

    let _ = player.kill();
    let _ = player.wait();
    let _ = std::fs::remove_file(&wav);
    outcome
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// An `AudioBufferList` with room for two buffers, for driving `process`
    /// without the HAL.
    #[repr(C)]
    struct BufferList2 {
        count: u32,
        buffers: [AudioBuffer; 2],
    }

    fn list(count: u32, bufs: [AudioBuffer; 2]) -> BufferList2 {
        BufferList2 {
            count,
            buffers: bufs,
        }
    }

    fn buffer(data: &mut [f32], channels: u32) -> AudioBuffer {
        AudioBuffer {
            mNumberChannels: channels,
            mDataByteSize: (data.len() * 4) as u32,
            mData: data.as_mut_ptr().cast(),
        }
    }

    #[test]
    fn coefficient_reaches_target_in_fade_time() {
        let sr = 48_000.0;
        let c = smoothing_coeff(300, sr);
        let mut g = 1.0f32;
        for _ in 0..(sr * 0.3) as usize {
            g += c * (0.2 - g);
        }
        assert!((g - 0.2).abs() < 0.01, "gain after fade = {g}");
        assert_eq!(smoothing_coeff(0, sr), 1.0);
    }

    #[test]
    fn process_maps_interleaved_to_noninterleaved_with_gain() {
        let frames = 256;
        // Interleaved stereo input: L = 0.5, R = -0.25.
        let mut input: Vec<f32> = (0..frames).flat_map(|_| [0.5f32, -0.25]).collect();
        let mut out_l = vec![0f32; frames];
        let mut out_r = vec![0f32; frames];
        let mut empty = [0f32; 0];
        let mut in_list = list(1, [buffer(&mut input, 2), buffer(&mut empty, 0)]);
        let mut out_list = list(2, [buffer(&mut out_l, 1), buffer(&mut out_r, 1)]);

        let shared = Shared::new(1.0); // instant smoother
        shared.in_mask.store(u64::MAX, Ordering::Relaxed);
        shared.set_target(0.2);
        unsafe {
            process(
                &shared,
                NonNull::from(&mut in_list).cast::<AudioBufferList>(),
                NonNull::from(&mut out_list).cast::<AudioBufferList>(),
            );
        }
        assert!(out_l.iter().all(|v| (v - 0.1).abs() < 1e-6));
        assert!(out_r.iter().all(|v| (v + 0.05).abs() < 1e-6));
        assert!(shared.nonzero.load(Ordering::Relaxed));
        assert_eq!(shared.frames.load(Ordering::Relaxed), frames as u64);
        assert!((shared.current_gain() - 0.2).abs() < 1e-6);
        assert!((f32::from_bits(shared.in_peak.load(Ordering::Relaxed)) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn process_mixes_stereo_to_mono_and_detects_silence() {
        let frames = 64;
        let mut input = vec![0f32; frames * 2];
        let mut out = vec![1f32; frames];
        let mut empty = [0f32; 0];
        let mut in_list = list(1, [buffer(&mut input, 2), buffer(&mut empty, 0)]);
        let mut out_list = list(1, [buffer(&mut out, 1), buffer(&mut empty, 0)]);
        let shared = Shared::new(1.0);
        shared.in_mask.store(u64::MAX, Ordering::Relaxed);
        unsafe {
            process(
                &shared,
                NonNull::from(&mut in_list).cast::<AudioBufferList>(),
                NonNull::from(&mut out_list).cast::<AudioBufferList>(),
            );
        }
        assert!(out.iter().all(|v| *v == 0.0));
        assert!(!shared.nonzero.load(Ordering::Relaxed));

        // Now with signal: L = 0.4, R = 0.2 → mono 0.3.
        for f in 0..frames {
            input[f * 2] = 0.4;
            input[f * 2 + 1] = 0.2;
        }
        unsafe {
            process(
                &shared,
                NonNull::from(&mut in_list).cast::<AudioBufferList>(),
                NonNull::from(&mut out_list).cast::<AudioBufferList>(),
            );
        }
        assert!(out.iter().all(|v| (v - 0.3).abs() < 1e-6));
        assert!(shared.nonzero.load(Ordering::Relaxed));
    }

    #[test]
    fn process_resamples_when_frame_counts_differ() {
        // 4 mono input frames [0,1,2,3] → 8 output frames, linearly
        // interpolated (the last one clamps to the final input sample).
        let mut input = vec![0f32, 1.0, 2.0, 3.0];
        let mut out = vec![0f32; 8];
        let mut empty = [0f32; 0];
        let mut in_list = list(1, [buffer(&mut input, 1), buffer(&mut empty, 0)]);
        let mut out_list = list(1, [buffer(&mut out, 1), buffer(&mut empty, 0)]);
        let shared = Shared::new(1.0);
        shared.in_mask.store(u64::MAX, Ordering::Relaxed);
        unsafe {
            process(
                &shared,
                NonNull::from(&mut in_list).cast::<AudioBufferList>(),
                NonNull::from(&mut out_list).cast::<AudioBufferList>(),
            );
        }
        let expected = [0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.0];
        for (o, e) in out.iter().zip(expected) {
            assert!((o - e).abs() < 1e-6, "got {out:?}, expected {expected:?}");
        }
        assert_eq!(shared.last_in_frames.load(Ordering::Relaxed), 4);
        assert_eq!(shared.last_out_frames.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn process_ignores_buffers_outside_the_mask() {
        // Buffer 0 is a "microphone" stream (loud), buffer 1 is the tap
        // (quiet). Only the tap must reach the output or count as signal.
        let frames = 16;
        let mut mic = vec![0.9f32; frames];
        let mut tap = vec![0.1f32; frames];
        let mut out = vec![0f32; frames];
        let mut empty = [0f32; 0];
        let mut in_list = list(2, [buffer(&mut mic, 1), buffer(&mut tap, 1)]);
        let mut out_list = list(1, [buffer(&mut out, 1), buffer(&mut empty, 0)]);
        let shared = Shared::new(1.0);
        shared.in_mask.store(0b10, Ordering::Relaxed);
        unsafe {
            process(
                &shared,
                NonNull::from(&mut in_list).cast::<AudioBufferList>(),
                NonNull::from(&mut out_list).cast::<AudioBufferList>(),
            );
        }
        assert!(out.iter().all(|v| (v - 0.1).abs() < 1e-6), "{out:?}");
        assert!((f32::from_bits(shared.in_peak.load(Ordering::Relaxed)) - 0.1).abs() < 1e-6);

        // With an empty mask nothing is routed and nothing counts as signal.
        let shared = Shared::new(1.0);
        out.fill(0.0);
        unsafe {
            process(
                &shared,
                NonNull::from(&mut in_list).cast::<AudioBufferList>(),
                NonNull::from(&mut out_list).cast::<AudioBufferList>(),
            );
        }
        assert!(out.iter().all(|v| *v == 0.0));
        assert!(!shared.nonzero.load(Ordering::Relaxed));
    }

    #[test]
    fn create_returns_backend_on_supported_macos() {
        // On the CI/dev machine this runs on (macOS ≥ 14.2) the backend must
        // be constructible; the tap itself is only exercised by the ignored
        // test below.
        let d = create(&DuckingSettings::default());
        assert_eq!(d.is_some(), os_at_least(14, 2));
    }

    /// End-to-end: play a tone with `afplay` (a separate process, so it is
    /// tapped), duck, report what the tap captured, restore.
    ///
    /// Run with: `LOCAL_VOICE_DEBUG=1 cargo test -- --ignored --nocapture duck_afplay_tone`
    /// The first run may show the macOS "System Audio Recording" prompt.
    #[test]
    #[ignore = "needs audio hardware and the System Audio Recording permission"]
    fn duck_afplay_tone() {
        use std::process::{Command, Stdio};

        let dir = std::env::var_os("LOCAL_VOICE_TEST_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("ducking-tone.wav");
        write_tone_wav(&wav, 3).unwrap();

        let mut player = Command::new("afplay")
            .arg(&wav)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("afplay");
        // Give afplay time to open its output stream.
        std::thread::sleep(Duration::from_millis(600));

        let settings = DuckingSettings {
            enabled: true,
            level: 0.2,
            fade_ms: 300,
        };
        let mut ducker = create(&settings).expect("backend available on this macOS");
        let t0 = Instant::now();
        let result = ducker.duck();
        let duck_time = t0.elapsed();

        // Peek at the shared stats through the concrete type.
        let mac: &MacDucker = unsafe { &*(std::ptr::from_ref(&*ducker) as *const MacDucker) };
        let report = |label: &str| {
            if let Some(a) = &mac.active {
                let s = &a.shared;
                eprintln!(
                    "[{label}] gain={:.3} target={:.3} callbacks={} frames={} nonzero={} in_peak={:.4} in_rms={:.4} out_rms={:.4}",
                    s.current_gain(),
                    f32::from_bits(s.target.load(Ordering::Relaxed)),
                    s.callbacks.load(Ordering::Relaxed),
                    s.frames.load(Ordering::Relaxed),
                    s.nonzero.load(Ordering::Relaxed),
                    f32::from_bits(s.in_peak.load(Ordering::Relaxed)),
                    f32::from_bits(s.last_in_rms.load(Ordering::Relaxed)),
                    f32::from_bits(s.last_out_rms.load(Ordering::Relaxed)),
                );
                eprintln!(
                    "[{label}] tap-channel input peaks: {:?}, last cycle {} in / {} out frames, mask {:#b}",
                    s.channel_peaks(8),
                    s.last_in_frames.load(Ordering::Relaxed),
                    s.last_out_frames.load(Ordering::Relaxed),
                    s.in_mask.load(Ordering::Relaxed)
                );
            } else {
                eprintln!("[{label}] no active session");
            }
        };

        match &result {
            Ok(()) => eprintln!("duck() ok in {duck_time:?}"),
            Err(e) => eprintln!("duck() failed in {duck_time:?}: {e:#}"),
        }
        report("after duck");
        std::thread::sleep(Duration::from_secs(1));
        report("1 s later");
        let t1 = Instant::now();
        ducker.restore().unwrap();
        eprintln!("restore() ok in {:?}", t1.elapsed());
        report("after restore");
        ducker.restore().unwrap(); // idempotent

        let _ = player.kill();
        let _ = player.wait();
        let _ = std::fs::remove_file(&wav);

        // Do not fail the test on a denied permission — report it instead —
        // but a hard API failure is a real bug.
        if let Err(e) = result {
            assert!(
                e.to_string().contains("System Audio Recording"),
                "unexpected error: {e:#}"
            );
        }
    }
}
