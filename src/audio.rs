use anyhow::{Context, Result};
use std::num::NonZero;
use std::path::Path;
use std::sync::mpsc::{self, SyncSender};
use std::thread;

use crate::engine::AudioOutput;

/// Play audio through the default output device
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

    Ok(())
}

/// Background audio queue — plays audio sequentially without blocking the caller.
pub struct AudioQueue {
    tx: SyncSender<AudioOutput>,
}

impl AudioQueue {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel::<AudioOutput>(16);
        thread::spawn(move || {
            while let Ok(audio) = rx.recv() {
                if let Err(e) = play_audio(&audio) {
                    eprintln!("[local-voice] Playback error: {e}");
                }
            }
        });
        Self { tx }
    }

    pub fn enqueue(&self, audio: AudioOutput) {
        self.tx.send(audio).ok();
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
