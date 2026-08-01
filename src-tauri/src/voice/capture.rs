//! Audio capture (macOS-only) + cross-platform resampling.
//!
//! On macOS, `spawn_capture_loop` builds a `cpal` input stream on a
//! dedicated `std::thread` (cpal streams are `!Send`) and bridges
//! each callback's interleaved f32 frames into a `tokio::sync::mpsc`
//! channel of mono 16 kHz frames. The consumer (item 5-3) drains
//! the channel into a `Vec<f32>` ring buffer that whisper consumes.
//!
//! On non-macOS, `spawn_capture_loop` returns
//! `CaptureError::Cpal` per §W4 (headless Linux agents have no
//! microphone and cannot exercise the real cpal pipeline). The
//! resampler (`resample_to_16k`) is platform-independent and the
//! tests in this module run on every host.
//!
//! ## Sample rate + format
//!
//! `cpal::default_input_config` returns whatever sample rate the
//! device prefers (typically 44.1 / 48 kHz). The callback hands us
//! interleaved f32 frames; `resample_to_16k` strips any extra
//! channels (the device is required to be mono) and uses
//! `rubato::SincFixedIn` to decimate to 16 kHz, which is the rate
//! whisper expects.
//!
//! ## v1 vs v2
//!
//! Plan §5.2 Part A ships f32 frames. The downstream ring buffer is
//! typed as `Vec<f16>` in v1 to align with the plan; v2 may move to
//! `i16`. This module doesn't own that buffer — it just produces
//! the 16 kHz mono stream on the channel.

use thiserror::Error;

/// All errors `capture` can surface. `Display` is implemented via
/// `thiserror` so callers can return `format!("{}", e)` to the IPC
/// layer without manual mapping.
#[derive(Error, Debug)]
pub enum CaptureError {
    #[error("cpal error: {0}")]
    Cpal(String),
    #[error("resample error: {0}")]
    Resample(String),
    #[error("audio thread join error")]
    Join,
}

/// One mono 16 kHz PCM frame. `f32` is the unit type the cpal
/// callback gives us and the unit type `rubato` consumes, so we
/// keep it through the channel instead of widening to f16 here.
pub type Frame = f32;

/// Buffer length for the mpsc channel between the cpal callback
/// thread and the consumer. At 16 kHz mono, 4096 frames is ~256 ms
/// of headroom — enough to absorb a brief consumer stall without
/// dropping audio but small enough to surface backpressure quickly
/// during sustained pauses.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const CHANNEL_CAPACITY: usize = 4096;

/// Spawn the cpal capture loop (macOS only) and return an mpsc
/// receiver of mono 16 kHz frames.
///
/// The caller should `tokio::task::spawn_blocking` this call so the
/// cpal stream lives on its own thread without contending with the
/// tokio reactor. The returned `Receiver<Frame>` is moved into a
/// `tokio::task::spawn`-ed consumer that drains frames into the
/// transcription ring buffer.
///
/// On non-macOS hosts the function returns
/// `CaptureError::Cpal("...")` instead of compiling in a stub, so
/// upstream code can branch on the error without `#[cfg]` of its
/// own.
#[cfg(target_os = "macos")]
pub fn spawn_capture_loop() -> Result<tokio::sync::mpsc::Receiver<Frame>, CaptureError> {
    use cpal::traits::{DeviceTrait, StreamTrait};

    let (tx, rx) = tokio::sync::mpsc::channel::<Frame>(CHANNEL_CAPACITY);

    let device = cpal::default_input_device()
        .ok_or_else(|| CaptureError::Cpal("no input device available".into()))?;
    let supported = device
        .default_input_config()
        .map_err(|e| CaptureError::Cpal(e.to_string()))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let stream_config = supported.config();

    if channels != 1 {
        return Err(CaptureError::Cpal(format!(
            "expected mono input, got {} channels",
            channels
        )));
    }

    // The cpal stream is !Send, so it must stay on a dedicated
    // thread that lives for the lifetime of the process. We park
    // the thread once `play()` returns; the stream's callback
    // continues to fire on cpal's internal audio thread.
    std::thread::Builder::new()
        .name("trail-cpal-capture".into())
        .spawn(move || {
            let stream = match device.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // `data` is already mono because we asserted
                    // `channels == 1` above; no interleaving to
                    // undo. Hand the whole buffer to the resampler
                    // in one go.
                    let resampled = match resample_to_16k(data, sample_rate) {
                        Ok(buf) => buf,
                        Err(e) => {
                            eprintln!("trail capture: resample failed: {}", e);
                            return;
                        }
                    };
                    for frame in resampled {
                        if tx.blocking_send(frame).is_err() {
                            // Consumer dropped — nothing useful we
                            // can do; the stream will keep firing
                            // but the sends are best-effort.
                            return;
                        }
                    }
                },
                move |err| eprintln!("trail capture: cpal stream error: {}", err),
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("trail capture: build_input_stream failed: {}", e);
                    return;
                }
            };

            if let Err(e) = stream.play() {
                eprintln!("trail capture: stream.play failed: {}", e);
                return;
            }

            // Park forever; the cpal stream drives the actual
            // audio callback on its own audio thread.
            std::thread::park();
        })
        .map_err(|e| CaptureError::Cpal(format!("spawn capture thread: {}", e)))?;

    Ok(rx)
}

/// Non-macOS stub: return an error so callers (5-3 transcription)
/// can degrade gracefully without `#[cfg]` of their own. The
/// `resample_to_16k` helper below is still available and exercised
/// by the tests in this module on every host.
#[cfg(not(target_os = "macos"))]
pub fn spawn_capture_loop() -> Result<tokio::sync::mpsc::Receiver<Frame>, CaptureError> {
    Err(CaptureError::Cpal(
        "cpal capture is only supported on macOS; Linux builds return this error per §W4".into(),
    ))
}

/// Resample a chunk of mono PCM frames from `from_rate` to 16 kHz
/// mono using `rubato::SincFixedIn` with `SincInterpolationType::Cubic`.
///
/// Behaviour:
/// - `from_rate == 16_000` → returned as-is (no resampler involved).
/// - `from_rate < 16_000` → returns
///   `CaptureError::Resample("upsampling not supported in v1")`.
///   whisper can't accept upsampled audio because the high
///   frequencies are already gone, so we refuse rather than
///   synthesise a fake band.
/// - `from_rate > 16_000` → decimates using 50 ms windows (one
///   `chunk_size` = `from_rate / 20` frames at a time). The output
///   length is approximately `input.len() * 16_000 / from_rate` plus
///   a tail of ~64 samples from the sinc filter's group delay; the
///   tests use a 256-sample tolerance to absorb that.
pub fn resample_to_16k(input: &[Frame], from_rate: u32) -> Result<Vec<Frame>, CaptureError> {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };

    const TO_RATE: u32 = 16_000;
    if from_rate == TO_RATE {
        return Ok(input.to_vec());
    }
    if from_rate < TO_RATE {
        return Err(CaptureError::Resample(
            "upsampling not supported in v1".into(),
        ));
    }

    // ~50 ms windows. SincFixedIn requires the chunk size to be
    // fixed, so we feed it one chunk at a time and concatenate.
    let chunk_size = (from_rate / 20) as usize;
    // Sinc interpolation parameters — moderate quality. The
    // defaults (sinc_len=256, BlackmanHarris2) are overkill for
    // voice; halve sinc_len to keep CPU in check on the capture
    // thread, which already runs the cpal callback. f_cutoff=0.95
    // is the rubato default and matches whisper's 8 kHz Nyquist.
    let params = SincInterpolationParameters {
        sinc_len: 128,
        oversampling_factor: 128,
        f_cutoff: 0.95,
        window: WindowFunction::BlackmanHarris2,
        interpolation: SincInterpolationType::Cubic,
    };
    let mut resampler = SincFixedIn::<Frame>::new(
        TO_RATE as f64 / from_rate as f64,
        1.0, // max_resample_ratio_relative
        params,
        chunk_size,
        1, // nbr_channels (mono)
    )
    .map_err(|e| CaptureError::Resample(e.to_string()))?;

    // ResampleResult<Vec<Vec<T>>> — one Vec<T> per channel. With
    // `nbr_channels == 1` the outer Vec has exactly one entry.
    let mut output = Vec::with_capacity(input.len() * 16_000 / from_rate as usize);
    for chunk in input.chunks(chunk_size) {
        // SincFixedIn requires each channel's input slice to have
        // exactly `chunk_size` frames. The final chunk is
        // zero-padded so the resampler can produce its last output
        // window deterministically.
        let mut padded = chunk.to_vec();
        if padded.len() < chunk_size {
            padded.resize(chunk_size, 0.0);
        }
        let waves = vec![padded];
        // `None` = no per-channel mask; all channels active.
        let resampled = resampler
            .process(&waves, None)
            .map_err(|e| CaptureError::Resample(e.to_string()))?;
        if let Some(channel) = resampled.into_iter().next() {
            // The last chunk may produce fewer valid output
            // frames than a full window; trust the resampler and
            // append them all.
            output.extend(channel);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_passthrough_when_rate_matches() {
        // 16 kHz input must round-trip without resampling. The
        // empty-input variant is covered separately by
        // `resample_empty_input_returns_empty_output`.
        let input: Vec<f32> = (0..1600).map(|i| (i as f32) / 1600.0).collect();
        let out = resample_to_16k(&input, 16_000).expect("16k passthrough");
        assert_eq!(out.len(), input.len());
        for (a, b) in out.iter().zip(input.iter()) {
            assert!((a - b).abs() < 1e-5, "frame drift: {} vs {}", a, b);
        }
    }

    #[test]
    fn resample_empty_input_returns_empty_output() {
        // §5.2 test 1: captures zero samples on an empty input.
        // The resampler's passthrough branch hits this directly on
        // a 16 kHz source.
        let out = resample_to_16k(&[], 16_000).expect("empty 16k");
        assert!(out.is_empty());

        // The 48 kHz path also has to handle empty input without
        // panicking — `chunks(0)` should produce no output.
        let out_48 = resample_to_16k(&[], 48_000).expect("empty 48k");
        assert!(out_48.is_empty());
    }

    #[test]
    fn resample_48k_to_16k_ratio() {
        // 48 kHz → 16 kHz is exactly a 1:3 decimation. We expect
        // ~16_000 output frames from 48_000 input frames, with a
        // small tail from the sinc filter's group delay.
        let input: Vec<f32> = (0..48_000).map(|i| ((i as f32) / 48000.0).sin()).collect();
        let out = resample_to_16k(&input, 48_000).expect("48k → 16k");

        let expected = 16_000_i64;
        let tolerance = 256_i64; // sinc_len + chunk remainder
        let diff = (out.len() as i64 - expected).abs();
        assert!(
            diff < tolerance,
            "expected ~{} ±{} frames, got {} (diff {})",
            expected,
            tolerance,
            out.len(),
            diff
        );

        // The output must contain real signal (not all zeros).
        let energy: f32 = out.iter().map(|f| f * f).sum();
        assert!(energy > 0.0, "resampled buffer is silent");
    }

    #[test]
    fn resample_rejects_upsampling() {
        // 8 kHz → 16 kHz is upsampling. whisper can't recover
        // frequencies we never captured, so the v1 contract is to
        // refuse rather than synthesise.
        let input = vec![0.0_f32; 800];
        let err = resample_to_16k(&input, 8_000).expect_err("upsampling should fail");
        assert!(
            matches!(err, CaptureError::Resample(_)),
            "expected Resample error, got {:?}",
            err
        );
    }

    #[test]
    fn spawn_capture_loop_unsupported_on_linux() {
        // On non-macOS, spawn_capture_loop must return an error
        // (not a stubbed receiver) so upstream code can degrade
        // gracefully. The macOS branch is a no-op here — real
        // cpal capture is verified manually on Pedro's Mac per §W4.
        #[cfg(not(target_os = "macos"))]
        {
            let result = spawn_capture_loop();
            assert!(result.is_err(), "expected unsupported-platform error");
        }
        #[cfg(target_os = "macos")]
        {
            // Don't actually open the mic in unit tests — too noisy
            // and not deterministic. Manual verification only.
        }
    }
}
