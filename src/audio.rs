use anyhow::{Context, Result};
use std::num::NonZero;
use std::path::Path;

use crate::engine::AudioOutput;

/// Play audio through the default output device
pub fn play_audio(audio: &AudioOutput) -> Result<()> {
    let source = rodio::buffer::SamplesBuffer::new(
        NonZero::new(audio.channels).unwrap(),
        NonZero::new(audio.sample_rate).unwrap(),
        audio.samples.clone(),
    );

    let sink = rodio::DeviceSinkBuilder::open_default_sink()
        .with_context(|| "Failed to open audio output device")?;

    let player = rodio::Player::connect_new(sink.mixer());
    player.append(source);
    player.sleep_until_end();

    // Keep sink alive until playback finishes
    drop(sink);

    Ok(())
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
