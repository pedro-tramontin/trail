//! Audio capture (macOS-only) + cross-platform resampling.
//!
//! On macOS, `spawn_capture_loop` builds a `cpal` input stream on a
//! dedicated `std::thread` (cpal streams are `!Send`) and bridges
//! each callback's interleaved f32 frames into a `tokio::sync::mpsc`
//! channel of mono 16 kHz frames. The consumer (spawned by §5.6's
//! `spawn_capture_loop` Part B fixup) drains the channel into the
//! shared `Arc<Mutex<Vec<f32>>>` that whisper consumes.
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
//! typed as `Vec<f32>` in v1; the spec sketch in STATE.md says
//! `Vec<f16>` but whisper-rs actually takes `&[f32]`, so we keep
//! `f32` end-to-end. This module owns the `CaptureState` struct
//! introduced in §5.6 — the shared `Arc<Mutex<Vec<f32>>>` plus the
//! consumer `JoinHandle` so `voice_abort` can drop the buffer and
//! cancel the task cleanly.

use std::sync::Arc;

use parking_lot::Mutex;
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

/// Shared state for an in-flight voice capture. The samples buffer
/// is `Arc<Mutex<Vec<f32>>>` so the cpal-callback-bound consumer
/// task and the abort handler can both reach it without owning it.
/// The consumer `JoinHandle` lets `voice_abort` cancel the drain loop
/// cleanly when the user clicks "Stop" (or transcription fails).
///
/// This replaces the Part-A pattern noted in §5.5's heads-up where
/// the cpal producer and the downstream consumer each held their
/// own `Mutex<Vec<f32>>` (the two buffers could drift). With the
/// shared state below there is exactly one source of truth for the
/// in-memory samples, and `voice_abort` can wipe it with one
/// `clear() + shrink_to_fit()` (no `mem::forget`, no stale writes).
pub struct CaptureState {
    /// Mono 16 kHz PCM frames captured since `voice_start`.
    pub samples: Arc<Mutex<Vec<f32>>>,
    /// Handle for the spawned consumer task. `None` when no capture
    /// is active; `voice_stop` / `voice_abort` take it and call
    /// `.abort()` to stop the drain loop.
    pub consumer_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl CaptureState {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            consumer_handle: Mutex::new(None),
        }
    }
}

impl Default for CaptureState {
    fn default() -> Self {
        Self::new()
    }
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

/// Spawn the cpal capture loop (macOS only) AND the consumer task
/// that drains the sample channel into the shared `CaptureState`.
///
/// The cpal stream lives on its own `std::thread` (streams are
/// `!Send`); the consumer task is a normal `tokio::spawn`ed future
/// that locks the shared `Arc<Mutex<Vec<f32>>>` and pushes frames
/// onto it. The `JoinHandle` for the consumer is stored inside
/// the `CaptureState` so `voice_abort` can call `.abort()` on it
/// directly without needing a separate reference.
///
/// The function returns `()` — the consumer task owns the receive
/// end of the sample channel, so the caller doesn't need one. The
/// shared `CaptureState.samples` Vec is the canonical frame store
/// that `voice_stop` / `voice_abort` (and eventually the whisper
/// pipeline in §5.7) read from.
///
/// On non-macOS hosts the function returns
/// `CaptureError::Cpal("...")` instead of compiling in a stub, so
/// upstream code can branch on the error without `#[cfg]` of its
/// own.
#[cfg(target_os = "macos")]
pub fn spawn_capture_loop(state: Arc<CaptureState>) -> Result<(), CaptureError> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let (tx, rx) = tokio::sync::mpsc::channel::<Frame>(CHANNEL_CAPACITY);
    // cpal 0.15 moved host selection + default-device lookup behind
    // the `HostTrait`. `cpal::default_host()` returns the active
    // audio host (CoreAudio on macOS); calling `.default_input_device()`
    // on it returns the system's default input. The device itself
    // still implements `DeviceTrait::default_input_config`.
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| CaptureError::Cpal("no input device available".into()))?;
    let supported = device
        .default_input_config()
        .map_err(|e| CaptureError::Cpal(e.to_string()))?;
    // cpal 0.18 dropped the `SampleRate(u32)` newtype wrapper and
    // now returns the sample rate as a plain `u32` from
    // `SupportedStreamConfig::sample_rate`. The `.0` field access
    // that worked under cpal 0.15 (`SampleRate::0`) is no longer
    // valid because `u32` is a primitive type.
    let sample_rate = supported.sample_rate();
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
            // cpal 0.18 also changed `Device::build_input_stream` to
            // take the `StreamConfig` by value rather than by reference.
            // The 0.15 form `&stream_config` produces a "expected
            // `StreamConfig`, found `&StreamConfig`" error.
            let stream = match device.build_input_stream(
                stream_config,
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

    // Spawn the consumer task: drain the channel into the shared
    // buffer until the receiver is closed (channel drop on abort)
    // or the task is `.abort()`ed. Holding the JoinHandle in
    // `CaptureState.consumer_handle` makes both shutdown paths
    // possible — graceful close on `voice_stop`, hard cancel on
    // `voice_abort`.
    let consumer_samples = state.samples.clone();
    let handle = tokio::spawn(async move {
        let mut rx = rx;
        while let Some(frame) = rx.recv().await {
            consumer_samples.lock().push(frame);
        }
    });
    *state.consumer_handle.lock() = Some(handle);

    Ok(())
}

/// Non-macOS stub: return an error so callers (5-3 transcription)
/// can degrade gracefully without `#[cfg]` of their own. The
/// `resample_to_16k` helper below is still available and exercised
/// by the tests in this module on every host.
#[cfg(not(target_os = "macos"))]
pub fn spawn_capture_loop(_state: Arc<CaptureState>) -> Result<(), CaptureError> {
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
            let state = std::sync::Arc::new(CaptureState::new());
            let result = spawn_capture_loop(state);
            assert!(result.is_err(), "expected unsupported-platform error");
        }
        #[cfg(target_os = "macos")]
        {
            // Don't actually open the mic in unit tests — too noisy
            // and not deterministic. Manual verification only.
        }
    }

    #[test]
    fn capture_state_starts_empty_with_no_handle() {
        // `CaptureState::new()` must hand back a usable state object
        // for `app.manage()` to register. The samples buffer is
        // empty; no consumer task is running; abort is a no-op on
        // an empty state.
        let state = CaptureState::new();
        assert!(state.samples.lock().is_empty());
        assert!(state.consumer_handle.lock().is_none());
    }

    #[tokio::test]
    async fn capture_state_samples_are_shared_via_arc() {
        // Cloning the Arc gives both handles the same backing Vec.
        // This is the property §5.6's "collapse the two-Mutex
        // pattern" fixup relies on — the consumer task and the
        // abort handler both reach the same buffer.
        let state = Arc::new(CaptureState::new());
        let samples_b = state.samples.clone();
        state.samples.lock().extend_from_slice(&[0.1_f32; 100]);
        assert_eq!(samples_b.lock().len(), 100);
        // Wipe from the other handle — the original sees the same
        // empty buffer.
        samples_b.lock().clear();
        assert!(state.samples.lock().is_empty());
    }
}
