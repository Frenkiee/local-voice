//! Per-utterance loudness normalization and output volume.
//!
//! TTS models return raw tensors whose level varies by engine, voice and even
//! by sentence, and nothing in the playback path scales them — so speech lands
//! noticeably quieter than everything else on the machine. This module is the
//! one gain stage: it measures each utterance, brings it to a target RMS
//! without letting peaks run into the ceiling, then applies the operator's own
//! volume on top.
//!
//! It is deliberately pure — a slice of samples in, a slice of samples and a
//! report out — so the same numbers reach both the speaker and a `--output`
//! WAV file.

/// Below this peak an utterance counts as silence: there is nothing to measure,
/// so normalization would turn dither and noise into a roar.
const SILENCE_FLOOR: f32 = 1e-4;

/// Floor for reported dB values, so a digital-zero buffer logs as a number
/// instead of `-inf`.
const MIN_DB: f32 = -120.0;

/// How loud the output should be, and whether to measure at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessSettings {
    /// Measure each utterance and bring it to `target_rms_db`.
    pub normalize: bool,
    /// Loudness every utterance is brought to, in dBFS RMS.
    pub target_rms_db: f32,
    /// Normalization never pushes the loudest sample above this, in dBFS.
    /// While normalization is on this is an absolute ceiling: it bounds the
    /// total gain, so `volume` is absorbed into it rather than clipping.
    pub peak_ceiling_db: f32,
    /// The operator's own output volume, applied after normalization.
    /// 1.0 leaves the level alone. With normalization on the peak ceiling
    /// caps the combined gain, so loud utterances absorb the volume instead
    /// of clipping; with `normalize: false` it is raw gain and may clip.
    pub volume: f32,
}

impl Default for LoudnessSettings {
    fn default() -> Self {
        Self {
            normalize: true,
            target_rms_db: -20.0,
            peak_ceiling_db: -1.0,
            volume: 1.0,
        }
    }
}

/// What [`apply`] measured and did, for the debug log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessReport {
    /// Loudest sample before the gain, in dBFS.
    pub peak_before_db: f32,
    /// RMS level before the gain, in dBFS.
    pub rms_before_db: f32,
    /// The gain that was applied, in dB (normalization and volume together).
    pub gain_db: f32,
    /// Samples the gain pushed past ±1.0 and that were clamped.
    pub clipped_samples: usize,
}

/// Bring `samples` to the configured level, in place.
///
/// Silence (peak below [`SILENCE_FLOOR`]) only gets `volume`; there is no
/// measurable level to normalize towards. Otherwise the gain is whichever is
/// smaller of "reach the RMS target" and "keep the peak under the ceiling",
/// so a hot model is attenuated just as a quiet one is boosted, and a single
/// spike cannot drag the whole utterance down to a whisper. `volume`
/// multiplies on top, but while normalizing the peak ceiling bounds the
/// *total* gain: an emphasised volume is absorbed on loud passages instead of
/// running past full scale. Clamping (counted in the report) then remains a
/// backstop only reachable with `normalize: false` and a volume above 1.
///
/// Channel layout does not matter: interleaved frames take the same gain as
/// mono, so the stereo image is untouched.
pub fn apply(samples: &mut [f32], settings: &LoudnessSettings) -> LoudnessReport {
    let (peak, rms) = measure(samples);

    let measured = settings.normalize && peak >= SILENCE_FLOOR && rms > 0.0;
    let normalization = if measured {
        let to_target = db_to_linear(settings.target_rms_db) / rms;
        let under_ceiling = db_to_linear(settings.peak_ceiling_db) / peak;
        to_target.min(under_ceiling)
    } else {
        1.0
    };

    let mut gain = normalization * settings.volume;
    if measured {
        // The ceiling bounds the total gain, not just the normalization part.
        // Without this the volume multiplier is applied past the ceiling and
        // the clamp below flattens loud syllables into hard-clipped plateaus
        // — audible crackle on every stressed word.
        let under_ceiling = db_to_linear(settings.peak_ceiling_db) / peak;
        gain = gain.min(under_ceiling);
    }

    let mut clipped_samples = 0;
    if gain != 1.0 {
        for sample in samples.iter_mut() {
            let scaled = *sample * gain;
            if !(-1.0..=1.0).contains(&scaled) {
                clipped_samples += 1;
            }
            *sample = scaled.clamp(-1.0, 1.0);
        }
    }

    LoudnessReport {
        peak_before_db: linear_to_db(peak),
        rms_before_db: linear_to_db(rms),
        gain_db: linear_to_db(gain),
        clipped_samples,
    }
}

/// Loudest absolute sample and RMS level of `samples`, both linear.
fn measure(samples: &[f32]) -> (f32, f32) {
    let mut peak = 0.0f32;
    let mut sum_squares = 0.0f64;
    for &sample in samples {
        peak = peak.max(sample.abs());
        sum_squares += f64::from(sample) * f64::from(sample);
    }
    let rms = if samples.is_empty() {
        0.0
    } else {
        (sum_squares / samples.len() as f64).sqrt() as f32
    };
    (peak, rms)
}

fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        MIN_DB
    } else {
        (20.0 * linear.log10()).max(MIN_DB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of a 100 Hz sine at `amplitude`, 24 kHz mono.
    fn sine(amplitude: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| amplitude * (i as f32 * 100.0 * std::f32::consts::TAU / 24_000.0).sin())
            .collect()
    }

    fn rms_of(samples: &[f32]) -> f32 {
        measure(samples).1
    }

    fn peak_of(samples: &[f32]) -> f32 {
        measure(samples).0
    }

    #[test]
    fn silence_is_left_silent_and_only_volume_applies() {
        let mut samples = vec![0.0; 256];
        let report = apply(&mut samples, &LoudnessSettings::default());
        assert!(samples.iter().all(|&s| s == 0.0));
        assert_eq!(report.gain_db, 0.0, "silence must not be boosted");
        assert_eq!(report.peak_before_db, MIN_DB);
        assert_eq!(report.clipped_samples, 0);

        // A near-silent buffer is scaled by volume alone, never normalized.
        let mut whisper = vec![1e-5; 256];
        let report = apply(
            &mut whisper,
            &LoudnessSettings {
                volume: 0.5,
                ..Default::default()
            },
        );
        assert!((whisper[0] - 5e-6).abs() < 1e-12);
        assert!((report.gain_db - linear_to_db(0.5)).abs() < 1e-3);
    }

    #[test]
    fn quiet_signal_is_boosted_to_the_rms_target() {
        let mut samples = sine(0.01, 24_000);
        let settings = LoudnessSettings::default();
        let report = apply(&mut samples, &settings);

        let target = db_to_linear(settings.target_rms_db);
        assert!(
            (rms_of(&samples) - target).abs() < target * 0.01,
            "expected RMS ≈ {target}, got {}",
            rms_of(&samples)
        );
        assert!(report.gain_db > 20.0, "gain was {} dB", report.gain_db);
        assert_eq!(report.clipped_samples, 0);
    }

    #[test]
    fn hot_signal_is_attenuated_to_the_rms_target() {
        let mut samples = sine(0.99, 24_000);
        let settings = LoudnessSettings::default();
        let report = apply(&mut samples, &settings);

        let target = db_to_linear(settings.target_rms_db);
        assert!(
            (rms_of(&samples) - target).abs() < target * 0.01,
            "expected RMS ≈ {target}, got {}",
            rms_of(&samples)
        );
        assert!(report.gain_db < 0.0, "gain was {} dB", report.gain_db);
        assert_eq!(report.clipped_samples, 0);
    }

    #[test]
    fn a_spike_is_capped_by_the_ceiling_not_the_rms_target() {
        // Quiet speech with one loud transient: chasing the RMS target would
        // push the transient far past full scale.
        let mut samples = sine(0.002, 24_000);
        samples[12_000] = 0.9;

        let settings = LoudnessSettings::default();
        let report = apply(&mut samples, &settings);

        let ceiling = db_to_linear(settings.peak_ceiling_db);
        assert!(
            (peak_of(&samples) - ceiling).abs() < ceiling * 0.01,
            "expected peak ≈ {ceiling}, got {}",
            peak_of(&samples)
        );
        assert!(
            rms_of(&samples) < db_to_linear(settings.target_rms_db),
            "the ceiling, not the RMS target, must bind here"
        );
        assert_eq!(report.clipped_samples, 0);
    }

    #[test]
    fn volume_zero_mutes_everything() {
        let mut samples = sine(0.5, 1_024);
        let report = apply(
            &mut samples,
            &LoudnessSettings {
                volume: 0.0,
                ..Default::default()
            },
        );
        assert!(samples.iter().all(|&s| s == 0.0));
        assert_eq!(report.gain_db, MIN_DB);
        assert_eq!(report.clipped_samples, 0);
    }

    #[test]
    fn volume_two_clips_a_loud_signal_and_reports_the_count() {
        let mut samples = vec![0.9, -0.9, 0.4, -0.4, 0.5, 0.49];
        let report = apply(
            &mut samples,
            &LoudnessSettings {
                normalize: false,
                volume: 2.0,
                ..Default::default()
            },
        );
        // Only ±0.9 pass ±1.0; 0.5 lands exactly on full scale, which is not
        // clipping, and ±0.4 / 0.49 stay well inside.
        assert_eq!(report.clipped_samples, 2);
        assert_eq!(samples[0], 1.0);
        assert_eq!(samples[1], -1.0);
        assert!((samples[2] - 0.8).abs() < 1e-6);
        assert_eq!(samples[4], 1.0);
        assert!((report.gain_db - linear_to_db(2.0)).abs() < 1e-3);
    }

    #[test]
    fn normalize_off_applies_volume_only() {
        let quiet = sine(0.01, 1_024);
        let mut samples = quiet.clone();
        let report = apply(
            &mut samples,
            &LoudnessSettings {
                normalize: false,
                volume: 1.5,
                ..Default::default()
            },
        );
        for (scaled, original) in samples.iter().zip(&quiet) {
            assert!((scaled - original * 1.5).abs() < 1e-6);
        }
        assert!((report.gain_db - linear_to_db(1.5)).abs() < 1e-3);

        // Volume 1.0 with normalization off is a no-op.
        let mut untouched = quiet.clone();
        let report = apply(
            &mut untouched,
            &LoudnessSettings {
                normalize: false,
                ..Default::default()
            },
        );
        assert_eq!(untouched, quiet);
        assert_eq!(report.gain_db, 0.0);
    }

    #[test]
    fn volume_multiplies_on_top_of_normalization() {
        let mut once = sine(0.01, 24_000);
        apply(&mut once, &LoudnessSettings::default());

        let mut louder = sine(0.01, 24_000);
        apply(
            &mut louder,
            &LoudnessSettings {
                volume: 1.5,
                ..Default::default()
            },
        );

        assert!((rms_of(&louder) - rms_of(&once) * 1.5).abs() < 1e-4);
    }

    #[test]
    fn volume_cannot_push_a_hot_utterance_past_the_ceiling() {
        // Regression: the September 2026 loudness stage multiplied `volume`
        // on top of a ceiling-bound normalization, so `volume: 1.4` pushed
        // stressed syllables past full scale and the hard clamp crackled.
        // The volume must be absorbed by the ceiling instead.
        //
        // Speech-like crest factor (quiet body, loud stressed peaks, like the
        // measured -25.9 dBFS RMS / -7.9 dBFS peak real utterances produce):
        // normalization alone lands the peaks on the ceiling, and ×1.4 used
        // to put them a full dB above it.
        let settings = LoudnessSettings {
            volume: 1.4,
            ..Default::default()
        };
        let mut speech = sine(0.002, 24_000);
        for i in (0..24_000).step_by(2_400) {
            speech[i] = 0.7;
        }
        let report = apply(&mut speech, &settings);
        let ceiling = db_to_linear(settings.peak_ceiling_db);
        assert_eq!(
            report.clipped_samples, 0,
            "must never clip while normalizing"
        );
        assert!(
            (peak_of(&speech) - ceiling).abs() < ceiling * 0.01,
            "expected peak ≈ {ceiling}, got {}",
            peak_of(&speech)
        );
    }

    #[test]
    fn volume_still_lifts_signal_that_has_headroom() {
        // Absorption must only happen where the ceiling binds. A hot sine's
        // gain is bound by the RMS target, not the ceiling, so ×1.4 simply
        // makes it louder — and still cannot clip.
        let plain = LoudnessSettings::default();
        let emphasised = LoudnessSettings {
            volume: 1.4,
            ..Default::default()
        };
        let mut a = sine(0.99, 24_000);
        let mut b = sine(0.99, 24_000);
        apply(&mut a, &plain);
        let report = apply(&mut b, &emphasised);

        assert_eq!(report.clipped_samples, 0);
        assert!(
            peak_of(&b) > peak_of(&a),
            "volume must still make headroom-bound signals louder"
        );
        assert!(peak_of(&b) <= db_to_linear(emphasised.peak_ceiling_db) * 1.001);
    }

    #[test]
    fn interleaved_stereo_takes_one_uniform_gain() {
        // Left twice as loud as right; the ratio must survive the gain stage.
        let mut samples: Vec<f32> = sine(0.02, 4_096)
            .into_iter()
            .flat_map(|s| [s, s * 0.5])
            .collect();
        apply(&mut samples, &LoudnessSettings::default());

        for frame in samples.chunks_exact(2) {
            if frame[0].abs() > 1e-6 {
                assert!(
                    (frame[1] / frame[0] - 0.5).abs() < 1e-3,
                    "channel balance changed: {frame:?}"
                );
            }
        }
    }

    #[test]
    fn empty_input_is_harmless() {
        let mut samples: Vec<f32> = Vec::new();
        let report = apply(&mut samples, &LoudnessSettings::default());
        assert_eq!(report.peak_before_db, MIN_DB);
        assert_eq!(report.rms_before_db, MIN_DB);
        assert_eq!(report.clipped_samples, 0);
    }
}
