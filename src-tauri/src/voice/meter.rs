//! Audio level meter.
//!
//! Computes RMS over a sliding window + EMA-smoothed value. Used by
//! the tray-icon blink loop to scale blink rate with input loudness.

const EMA_ALPHA: f32 = 0.3;

#[derive(Debug, Clone, Default)]
pub struct Meter {
    pub rms: f32,
    pub ema: f32,
}

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the meter with a chunk of PCM samples (range -1.0..=1.0).
    /// Updates both `rms` (instantaneous) and `ema` (smoothed).
    pub fn update(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum_sq / samples.len() as f32).sqrt();
        self.rms = rms;
        // EMA: ema = alpha * rms + (1 - alpha) * prev_ema.
        self.ema = EMA_ALPHA * rms + (1.0 - EMA_ALPHA) * self.ema;
    }

    /// Compute the blink period in milliseconds. Higher meter → shorter
    /// period (faster blink). Returns None if the meter is below the
    /// silence threshold (no blinking when quiet).
    ///
    /// Range: rms 0.0 → 400ms; rms 0.5 → 100ms; rms ≥ 0.8 → 50ms.
    pub fn blink_period_ms(&self) -> Option<u64> {
        const SILENCE_THRESHOLD: f32 = 0.01;
        if self.ema < SILENCE_THRESHOLD {
            return None; // No blinking when quiet.
        }
        // Map [0.01, 1.0] → [400ms, 50ms] (logarithmic-ish).
        let normalized =
            ((self.ema - SILENCE_THRESHOLD) / (1.0 - SILENCE_THRESHOLD)).clamp(0.0, 1.0);
        let period_ms = 400.0 - (350.0 * normalized);
        Some(period_ms as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn rms_on_known_sine_wave() {
        // A 1 kHz sine at amplitude 0.5 has RMS = 0.5 / sqrt(2) ≈ 0.3536.
        let samples: Vec<f32> = (0..1600)
            .map(|i| 0.5 * (2.0 * PI * 1000.0 * i as f32 / 16000.0).sin())
            .collect();
        let mut meter = Meter::new();
        meter.update(&samples);
        // Allow 1% tolerance.
        assert!(
            (meter.rms - 0.3536).abs() < 0.01,
            "expected RMS ~0.3536, got {}",
            meter.rms
        );
    }

    #[test]
    fn ema_converges_after_repeated_updates() {
        let samples = vec![0.5_f32; 1600];
        let mut meter = Meter::new();
        // EMA with alpha=0.3 needs ~30+ iterations to converge within 1% of
        // the input RMS — use 50 to leave comfortable margin on the assertion.
        for _ in 0..50 {
            meter.update(&samples);
        }
        // After 50 updates, EMA should be very close to RMS = 0.5.
        assert!(
            (meter.ema - 0.5).abs() < 0.01,
            "expected EMA ~0.5, got {}",
            meter.ema
        );
    }

    #[test]
    fn blink_period_scales_with_meter() {
        let mut meter = Meter::new();
        // Quiet: no blinking.
        meter.update(&[0.0_f32; 100]);
        assert_eq!(meter.blink_period_ms(), None);

        // Mid loudness: 100-400ms.
        for _ in 0..20 {
            meter.update(&[0.3_f32; 1600]);
        }
        let mid = meter.blink_period_ms().expect("expected Some");
        assert!((100..=400).contains(&mid), "got {}", mid);

        // Loud: 50-150ms.
        for _ in 0..20 {
            meter.update(&[0.9_f32; 1600]);
        }
        let loud = meter.blink_period_ms().expect("expected Some");
        assert!((50..=150).contains(&loud), "got {}", loud);

        // Higher meter → shorter period.
        assert!(loud < mid, "loud should blink faster than mid");
    }
}
