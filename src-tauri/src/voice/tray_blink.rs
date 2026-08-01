//! Tray-icon blink animation loop.
//!
//! Fires the supplied callback at a rate proportional to the audio
//! meter. Loops until the `CancellationToken` is cancelled.

use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use super::meter::Meter;

pub type IconCallback = Arc<dyn Fn(bool) + Send + Sync>;

pub struct BlinkLoop {
    pub cancel: CancellationToken,
}

impl BlinkLoop {
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
        }
    }

    /// Spawn the blink loop. Returns immediately; the loop runs until
    /// `cancel.cancel()` is called.
    ///
    /// `set_icon` is called with `true` to show the active icon, `false`
    /// to show the idle icon. The loop alternates at the rate dictated
    /// by the meter.
    pub fn spawn(
        &self,
        set_icon: IconCallback,
        meter: Arc<Mutex<Meter>>,
    ) -> tokio::task::JoinHandle<()> {
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut state = false;
            while !cancel.is_cancelled() {
                let period = {
                    let m = meter.lock().await;
                    m.blink_period_ms().unwrap_or(200) // Default to 200ms when quiet.
                };
                state = !state;
                set_icon(state);
                tokio::time::sleep(Duration::from_millis(period)).await;
            }
        })
    }
}

impl Default for BlinkLoop {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn cancellation_terminates_loop_within_100ms() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        let set_icon: IconCallback = Arc::new(move |_| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        let meter = Arc::new(Mutex::new(Meter::new()));
        // Set meter to a fast blink rate.
        {
            let mut m = meter.lock().await;
            m.ema = 0.8; // Loud → ~60ms blink.
        }

        let blink = BlinkLoop::new();
        let handle = blink.spawn(set_icon, meter);

        // Let it run for 200ms (3-4 blinks at 60ms each).
        tokio::time::sleep(Duration::from_millis(200)).await;
        blink.cancel.cancel();

        // Await the task; it should finish within 100ms.
        let start = std::time::Instant::now();
        let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "cancellation took too long: {:?}",
            elapsed
        );
        let count = counter.load(Ordering::SeqCst);
        assert!(count >= 2, "expected at least 2 ticks, got {}", count);
    }
}
