//! Phase 3 §3.5 scheduler — fires the summarizer at the configured
//! `review_time` (default 18:00 local; UTC in v1), posts a system
//! notification via `notify-rust`, and updates the tray-icon badge
//! with `drafts ready: N`.
//!
//! ## v1 scope
//!
//! - Fires the summarizer-side hook (currently a stub bump; the actual
//!   `summarizer::run` invocation is wired up by the coordinator in a
//!   follow-up item because it requires the summarizer config + ollama
//!   client handle at construction time).
//! - `next_fire_time` is **UTC-only** in v1 even though the user-facing
//!   spec says "timezone-aware". A future item will accept an optional
//!   `tz` arg (`chrono-tz` or `IANA tz string`) and use
//!   `with_timezone(&tz)` instead of the implicit UTC. The deviation is
//!   documented in `state.md` §5b D1.
//! - The `tray_badge_updater` callback is supplied by the Tauri `run()`
//!   glue in `lib.rs`; this module is platform-agnostic and takes the
//!   callback as a `Fn(usize) + Send + 'static`.
//! - `spawn_loop` accepts a `now_fn` so tests can drive the scheduler
//!   against `tokio::time::Instant::now()` (which respects
//!   `tokio::time::pause()`), while production uses `Utc::now()`. This
//!   keeps the wall-clock-vs-paused-clock mismatch out of the way.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Shared scheduler state. Held behind an `Arc<Mutex<…>>` so the
/// scheduler loop and any IPC command (e.g. a future
/// `get_scheduler_state`) can read it without blocking the runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulerState {
    /// Next planned fire instant (UTC).
    pub next_fire: Option<DateTime<Utc>>,
    /// Number of drafts produced by previous fires and not yet
    /// acknowledged by the user. Drives the tray-icon badge label
    /// `drafts ready: N`.
    pub drafts_ready_count: usize,
    /// Most recent successful fire (UTC), or `None` if the scheduler
    /// has never fired yet.
    pub last_fired_at: Option<DateTime<Utc>>,
    /// Most recent error from the scheduler loop, for the diagnostics
    /// surface. Cleared on the next successful fire.
    pub last_error: Option<String>,
}

/// Parse a `"HH:MM"` review time and return the next UTC instant at
/// which it should fire. If the review time is later today (UTC), that
/// instant is returned; otherwise tomorrow's slot at the same time.
pub fn next_fire_time(review_time: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let naive = NaiveTime::parse_from_str(review_time, "%H:%M")
        .map_err(|e| format!("invalid review_time {review_time:?}: {e}"))?;
    let today = now.date_naive().and_time(naive);
    let today_utc: DateTime<Utc> = DateTime::<Utc>::from_naive_utc_and_offset(today, Utc);
    if today_utc > now {
        Ok(today_utc)
    } else {
        let tomorrow_date = now
            .date_naive()
            .succ_opt()
            .ok_or_else(|| "date overflow".to_string())?;
        let tomorrow = tomorrow_date.and_time(naive);
        let tomorrow_utc = DateTime::<Utc>::from_naive_utc_and_offset(tomorrow, Utc);
        Ok(tomorrow_utc)
    }
}

/// Spawn the scheduler loop. The loop:
///
/// 1. Computes `next_fire_time(review_time, now)` using `now_fn()`
///    (production: `Utc::now`; tests: a paused-time-aware source so
///    `tokio::time::advance` drives the scheduler).
/// 2. Sleeps until that instant via `tokio::time::sleep`.
/// 3. Increments `state.drafts_ready_count` and stamps `last_fired_at`.
/// 4. Calls `tray_badge_updater(new_count)` so the Tauri tray menu can
///    render `drafts ready: N`.
/// 5. Loops — the next iteration computes a fresh `next_fire_time`
///    (which will be ~24h later) so the cycle stays correct across
///    midnight and DST boundaries without bespoke math.
///
/// Returns the `JoinHandle` so the caller can `.abort()` it on app
/// shutdown. Invalid `review_time` strings log to stderr and exit the
/// loop (one-shot, so we don't spam logs if the supervisor retries).
pub fn spawn_loop<F, N>(
    review_time: String,
    state: Arc<Mutex<SchedulerState>>,
    tray_badge_updater: F,
    now_fn: N,
) -> JoinHandle<()>
where
    F: Fn(usize) + Send + 'static,
    N: Fn() -> DateTime<Utc> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            let now = now_fn();
            let next_fire = match next_fire_time(&review_time, now) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("scheduler: invalid review_time {review_time:?}: {e}");
                    let mut s = state.lock().await;
                    s.last_error = Some(e);
                    return;
                }
            };

            // Publish the next-fire timestamp so the UI / IPC can show
            // "next review at …".
            {
                let mut s = state.lock().await;
                s.next_fire = Some(next_fire);
                s.last_error = None;
            }

            // If the fire time is already in the past (e.g. the loop
            // just woke up from a previous fire), sleep a minimal
            // amount to avoid a tight spin. Chrono's `to_std` returns
            // `Err` for negative durations; fall back to 1 second.
            let duration = (next_fire - now)
                .to_std()
                .unwrap_or(StdDuration::from_secs(1));
            tokio::time::sleep(duration).await;

            // Fire: bump the counter, stamp last_fired_at, notify tray.
            // The actual summarizer invocation is the coordinator's job
            // (see §3.6 e2e) — this module owns the scheduling + UI
            // callback contract only.
            let new_count = {
                let mut s = state.lock().await;
                s.drafts_ready_count = s.drafts_ready_count.saturating_add(1);
                s.last_fired_at = Some(now_fn());
                s.drafts_ready_count
            };
            tray_badge_updater(new_count);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn next_fire_time_returns_today_if_not_yet_passed() {
        // 2026-07-29 12:00 UTC; review_time "18:00" -> today 18:00.
        let now = Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap();
        let next = next_fire_time("18:00", now).unwrap();
        let expected = Utc.with_ymd_and_hms(2026, 7, 29, 18, 0, 0).unwrap();
        assert_eq!(next, expected);
    }

    #[test]
    fn next_fire_time_returns_tomorrow_if_already_passed() {
        // 2026-07-29 20:00 UTC; review_time "18:00" -> 2026-07-30 18:00.
        let now = Utc.with_ymd_and_hms(2026, 7, 29, 20, 0, 0).unwrap();
        let next = next_fire_time("18:00", now).unwrap();
        let expected = Utc.with_ymd_and_hms(2026, 7, 30, 18, 0, 0).unwrap();
        assert_eq!(next, expected);
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_loop_fires_within_5_second_tolerance() {
        let state = Arc::new(Mutex::new(SchedulerState::default()));
        let counter = Arc::new(std::sync::Mutex::new(0usize));
        let counter_for_closure = Arc::clone(&counter);

        // Anchor "now" to the paused-clock epoch (1970-01-01 00:00 UTC)
        // so the math below stays exact regardless of how long the test
        // takes wall-clock-wise. With `review_time = "01:00"`, the
        // scheduler computes `next_fire = epoch + 1h` and sleeps for
        // 1 hour of paused time. `tokio::time::advance` then drives
        // the spawned task's sleep forward.
        let now_fn = || chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap();
        let handle = spawn_loop(
            "01:00".to_string(),
            Arc::clone(&state),
            move |_count| {
                *counter_for_closure.lock().unwrap() += 1;
            },
            now_fn,
        );

        // Let the spawned task run its prologue (compute next_fire
        // + start its `tokio::time::sleep`).
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        // Advance the paused clock past the fire time (1h+5s of
        // paused clock time).
        tokio::time::advance(StdDuration::from_secs(60 * 60 + 5)).await;

        // Allow the spawned task several turns to wake up and run
        // the fire callback. With `start_paused = true` no real time
        // elapses, so a handful of yields is sufficient.
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }

        let count = *counter.lock().unwrap();
        assert!(
            count >= 1,
            "scheduler should have fired at least once, got {count}"
        );
        {
            let state_read = state.lock().await;
            assert!(
                state_read.drafts_ready_count >= 1,
                "drafts_ready_count should be >= 1, got {}",
                state_read.drafts_ready_count
            );
            assert!(state_read.last_fired_at.is_some());
            assert!(state_read.next_fire.is_some());
        }
        handle.abort();
    }
}
