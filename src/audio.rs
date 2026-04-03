use anyhow::{Context, Result};
use std::num::NonZero;
use std::path::Path;
use std::sync::mpsc::{self, SyncSender};
use std::thread;

use crate::engine::AudioOutput;

/// Play audio through the default output device (blocking)
pub fn play_audio(audio: &AudioOutput) -> Result<()> {
    let source = rodio::buffer::SamplesBuffer::new(
        NonZero::new(audio.channels).unwrap(),
        NonZero::new(audio.sample_rate).unwrap(),
        audio.samples.clone(),
    );

    let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
        .with_context(|| "Failed to open audio output device")?;
    sink.log_on_drop(false);

    let player = rodio::Player::connect_new(sink.mixer());
    player.append(source);
    player.sleep_until_end();

    // Keep sink alive until playback finishes — dropping it kills audio on Windows
    drop(player);
    drop(sink);

    Ok(())
}

/// Play audio using an existing sink (for persistent playback thread)
fn play_audio_on_sink(audio: &AudioOutput, sink: &rodio::MixerDeviceSink) -> Result<()> {
    let source = rodio::buffer::SamplesBuffer::new(
        NonZero::new(audio.channels).unwrap(),
        NonZero::new(audio.sample_rate).unwrap(),
        audio.samples.clone(),
    );

    let player = rodio::Player::connect_new(sink.mixer());
    player.append(source);
    player.sleep_until_end();

    Ok(())
}

/// Background audio queue — plays audio sequentially without blocking the caller.
pub struct AudioQueue {
    audio_tx: SyncSender<AudioOutput>,
    work_tx: SyncSender<Box<dyn FnOnce() -> Option<AudioOutput> + Send>>,
}

impl AudioQueue {
    pub fn new() -> Self {
        // Audio playback thread — opens sink ONCE and reuses it
        let (audio_tx, audio_rx) = mpsc::sync_channel::<AudioOutput>(16);
        thread::spawn(move || {
            let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    s
                }
                Err(e) => {
                    eprintln!("[local-voice] Failed to open audio device: {e}");
                    return;
                }
            };
            while let Ok(audio) = audio_rx.recv() {
                if let Err(e) = play_audio_on_sink(&audio, &sink) {
                    eprintln!("[local-voice] Playback error: {e}");
                }
            }
            drop(sink);
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
