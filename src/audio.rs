use anyhow::{Context, Result, anyhow};
use std::num::NonZero;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::ducking::{self, DuckingSettings};
use crate::engine::AudioOutput;
use crate::playback_lock::{self, PlaybackSlot};

/// How long the background queue keeps other apps ducked after an item
/// finishes, waiting for the next one. Back-to-back `speak_async` calls stay
/// under one duck instead of pumping the volume up and down between items.
const DUCK_HOLD: Duration = Duration::from_millis(400);

/// Extra time playback may take beyond the audio's own length before the
/// output stream is declared dead. Covers device latency, resampling and a
/// slow first callback; anything longer means the device stopped pulling
/// samples (unplugged, switched, or torn down across sleep/wake).
const STALL_GRACE: Duration = Duration::from_secs(3);

/// How often a playing thread checks whether the player has drained.
const DRAIN_POLL: Duration = Duration::from_millis(10);

/// Length of `audio` at its native rate.
pub fn audio_duration(audio: &AudioOutput) -> Duration {
    let frames = audio.samples.len() as u64 / u64::from(audio.channels.max(1));
    Duration::from_secs_f64(frames as f64 / f64::from(audio.sample_rate.max(1)))
}

/// Open the *current* default output device.
///
/// Callers open per playback burst, never once per process: a sink bound to
/// the device that was default at startup silently stops consuming samples
/// once that device goes away, and a player waiting on it never returns.
fn open_sink() -> Result<rodio::MixerDeviceSink> {
    let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
        .context("Failed to open audio output device")?;
    sink.log_on_drop(false);
    Ok(sink)
}

/// Play `audio` on `sink` and block until it has been consumed, or until it
/// has clearly stalled.
///
/// Never waits unboundedly: the wait is capped at the audio's length plus
/// [`STALL_GRACE`]. On a stall the player is stopped and an error returned so
/// the caller drops the dead sink and releases the duck and the queue slot.
/// If `slot` is given its ticket is stamped with the same deadline, so other
/// processes can tell a stalled holder from a long speech.
fn play_on_sink(
    audio: &AudioOutput,
    sink: &rodio::MixerDeviceSink,
    slot: Option<&PlaybackSlot>,
) -> Result<()> {
    let source = rodio::buffer::SamplesBuffer::new(
        NonZero::new(audio.channels).unwrap(),
        NonZero::new(audio.sample_rate).unwrap(),
        audio.samples.clone(),
    );

    let budget = audio_duration(audio) + STALL_GRACE;
    if let Some(slot) = slot {
        slot.promise_done_within(budget + DUCK_HOLD);
    }

    let player = rodio::Player::connect_new(sink.mixer());
    player.append(source);

    let started = Instant::now();
    while !player.empty() {
        if started.elapsed() > budget {
            player.stop();
            return Err(anyhow!(
                "playback stalled: the output device stopped consuming samples \
                 ({:.1?} of audio not finished after {:.1?}; device removed or changed?)",
                audio_duration(audio),
                started.elapsed()
            ));
        }
        thread::sleep(DRAIN_POLL);
    }
    Ok(())
}

/// Play audio through the default output device (blocking).
///
/// Other applications are ducked according to `ducking` for the duration of
/// playback. `ducking::duck` blocks for the fade, so speech starts once the
/// other apps are already quiet; they are restored before this returns.
pub fn play_audio(audio: &AudioOutput, ducking: &DuckingSettings) -> Result<()> {
    // Wait for every other local-voice process (other MCP servers, other
    // CLI invocations) to finish talking, then duck and play.
    let slot = playback_lock::acquire();
    let sink = open_sink()?;
    let duck_guard = ducking::duck(ducking);

    let result = play_on_sink(audio, &sink, slot.as_ref());

    // Keep sink alive until playback finishes — dropping it kills audio on Windows
    drop(sink);
    // Restore other apps' volume (fades back), then let the next process in.
    drop(duck_guard);
    drop(slot);

    result
}

/// Background audio queue — plays audio sequentially without blocking the caller.
pub struct AudioQueue {
    audio_tx: SyncSender<AudioOutput>,
    work_tx: SyncSender<Box<dyn FnOnce() -> Option<AudioOutput> + Send>>,
}

impl AudioQueue {
    pub fn new() -> Self {
        // Audio playback thread — one burst at a time, output opened per burst.
        let (audio_tx, audio_rx) = mpsc::sync_channel::<AudioOutput>(16);
        thread::spawn(move || {
            // Block for the first item of a burst, then hold the duck while
            // more items keep arriving within DUCK_HOLD of each other.
            while let Ok(first) = audio_rx.recv() {
                // Strict FIFO across processes: two agents whose servers
                // both call speak_async at once no longer talk over each
                // other. The slot is held for the whole burst, including the
                // DUCK_HOLD window, so a second process cannot squeeze in
                // while this one still has the other apps ducked.
                let slot = playback_lock::acquire();
                let sink = match open_sink() {
                    Ok(s) => s,
                    Err(e) => {
                        // Drop this item; the next burst retries the device.
                        eprintln!("[local-voice] {e:#}");
                        continue;
                    }
                };
                // Config may change at runtime (MCP set_config): re-read per burst.
                let settings = Config::load()
                    .map(|c| c.ducking_settings())
                    .unwrap_or_default();
                let duck_guard = ducking::duck(&settings);

                let mut audio = first;
                loop {
                    if let Err(e) = play_on_sink(&audio, &sink, slot.as_ref()) {
                        // The stream cannot be trusted any more: end the burst
                        // so the remaining items reopen the device.
                        eprintln!("[local-voice] Playback error: {e:#}");
                        break;
                    }
                    match audio_rx.recv_timeout(DUCK_HOLD) {
                        Ok(next) => audio = next,
                        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
                    }
                }
                // Burst over: close the device, fade other apps back up,
                // release the queue slot, then wait for the next burst.
                drop(sink);
                drop(duck_guard);
                drop(slot);
            }
        });

        // Synthesis worker thread — runs jobs that produce audio, then forwards to playback
        let work_audio_tx = audio_tx.clone();
        let (work_tx, work_rx) =
            mpsc::sync_channel::<Box<dyn FnOnce() -> Option<AudioOutput> + Send>>(16);
        thread::spawn(move || {
            while let Ok(job) = work_rx.recv() {
                if let Some(audio) = job() {
                    work_audio_tx.send(audio).ok();
                }
            }
        });

        Self { audio_tx, work_tx }
    }

    /// Enqueue pre-synthesized audio for playback
    pub fn enqueue(&self, audio: AudioOutput) {
        self.audio_tx.send(audio).ok();
    }

    /// Enqueue a synthesis job — runs in background, audio plays when ready
    pub fn enqueue_job(&self, job: Box<dyn FnOnce() -> Option<AudioOutput> + Send>) {
        self.work_tx.send(job).ok();
    }
}

/// Save audio to a WAV file
pub fn save_wav(audio: &AudioOutput, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let spec = hound::WavSpec {
        channels: audio.channels,
        sample_rate: audio.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer =
        hound::WavWriter::create(path, spec).with_context(|| "Failed to create WAV file")?;

    for &sample in &audio.samples {
        let clamped = sample.clamp(-1.0, 1.0);
        writer.write_sample((clamped * 32767.0) as i16)?;
    }

    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_duration_uses_frames_not_samples() {
        let stereo = AudioOutput {
            samples: vec![0.0; 48_000 * 2],
            sample_rate: 48_000,
            channels: 2,
        };
        assert_eq!(audio_duration(&stereo), Duration::from_secs(1));
        let mono = AudioOutput {
            samples: vec![0.0; 12_000],
            sample_rate: 24_000,
            channels: 1,
        };
        assert_eq!(audio_duration(&mono), Duration::from_millis(500));
        let empty = AudioOutput {
            samples: vec![],
            sample_rate: 0,
            channels: 0,
        };
        assert_eq!(audio_duration(&empty), Duration::ZERO);
    }
}
