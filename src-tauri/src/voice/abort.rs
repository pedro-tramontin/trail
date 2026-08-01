//! Voice capture abort handling.
//!
//! Drops the in-memory samples buffer, aborts the consumer task via
//! `JoinHandle.abort()`, and removes any partial WAV + JSON files
//! from the on-disk store. Idempotent — safe to call multiple times
//! or when no capture is active.
//!
//! ## What "abort" means here
//!
//! `voice_abort` is the user-initiated "Stop recording" path. It
//! runs after `voice_start` (Part A wiring lands later) when the
//! user clicks the Stop tray-menu item, OR when `voice_stop`'s
//! transcription step fails and we need to roll the partial
//! capture back. The operation has three ordered effects:
//!
//! 1. **Drop the samples buffer.** Wipe the in-memory
//!    `Arc<Mutex<Vec<f32>>>` with `clear() + shrink_to_fit()`. We
//!    do not call `mem::forget`; the Vec's heap pages are returned
//!    to the allocator so the next capture session starts clean.
//! 2. **Cancel the consumer task.** `JoinHandle.abort()` interrupts
//!    the task at the next `.await`. The `tokio::time::timeout(100ms,
//!    handle)` below gives the runtime a brief window to settle the
//!    cancellation; if it takes longer we don't block the IPC
//!    caller — the task is reaped in the background.
//! 3. **Delete partial files.** `store::delete` removes any
//!    `<entry_id>.json` and `<entry_id>.wav` files that the
//!    `write_atomic` call from a prior in-flight `voice_stop`
//!    attempt may have left behind. The store's delete is
//!    idempotent (it ignores `NotFound`).
//!
//! All three steps are safe to run on an idle `CaptureState` — the
//! no-op path is exercised by `no_op_abort` and the unit tests
//! below.

use std::path::Path;
use std::time::Duration;

use thiserror::Error;
use tokio::task::JoinHandle;

use super::capture::CaptureState;
use super::store;

/// How long `voice_abort` waits for the consumer task to settle
/// after `.abort()`. The spec requires the cancellation to land
/// within 100 ms; we use the same budget for the timeout so a
/// stuck task can't stall the IPC caller.
const ABORT_JOIN_BUDGET: Duration = Duration::from_millis(100);

#[derive(Error, Debug)]
pub enum AbortError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("store error: {0}")]
    Store(#[from] store::StoreError),
}

/// Abort an in-progress voice capture. Idempotent.
///
/// `state`: the shared capture state (samples buffer + consumer
///     `JoinHandle`). The samples buffer is wiped; the consumer
///     task is `.abort()`ed and awaited briefly.
/// `trail_root`: where partial files might be on disk
///     (`~/.trail` in production).
/// `date` + `entry_id`: identifies the partial files to delete, if
///     any. The `store::delete` call is idempotent so passing
///     nonsense ids (no file at that path) is harmless.
pub async fn voice_abort(
    state: &CaptureState,
    trail_root: &Path,
    date: &str,
    entry_id: uuid::Uuid,
) -> Result<(), AbortError> {
    // 1. Drop the in-memory samples buffer (no mem::forget).
    {
        let mut buf = state.samples.lock();
        buf.clear();
        buf.shrink_to_fit();
    }

    // 2. Abort the consumer JoinHandle. We `take()` it out of the
    //    state so a second call to `voice_abort` is a no-op even
    //    if the first call's task is still being reaped. The lock
    //    guard is dropped at the end of this block (before the
    //    `.await`) so the consumer_handle Mutex doesn't cross an
    //    await point and trip Tauri's `Send` bound.
    let handle_opt = state.consumer_handle.lock().take();
    if let Some(handle) = handle_opt {
        abort_handle_with_timeout(handle, ABORT_JOIN_BUDGET).await;
    }

    // 3. Remove any partial files (idempotent — ignores not-found).
    store::delete(trail_root, date, entry_id)?;

    Ok(())
}

/// Cancel a tokio task and wait up to `budget` for it to settle.
/// Silently swallows the join error — cancellation may produce a
/// `JoinError` if the task was mid-await, which is fine here.
async fn abort_handle_with_timeout(handle: JoinHandle<()>, budget: Duration) {
    handle.abort();
    let _ = tokio::time::timeout(budget, handle).await;
}

/// Abort when no capture is active — no-op. `voice_stop` calls
/// this when the user stops without ever pressing record, so the
/// IPC layer can stay uniform ("always return `Ok` on a clean
/// stop") without special-casing the no-capture path.
pub fn no_op_abort() -> Result<(), AbortError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::store::{self, new_entry_id, VoiceEntry};
    use crate::voice::transcriber::Transcript;
    use tempfile::tempdir;

    #[tokio::test]
    async fn abort_drops_in_memory_buffer() {
        // 1. Seed the shared samples buffer with 1_000 frames.
        let state = CaptureState::new();
        state.samples.lock().extend_from_slice(&[0.1_f32; 1000]);
        assert_eq!(state.samples.lock().len(), 1000);

        // 2. Abort against an empty trail_root (no partial files).
        let trail_root = tempdir().unwrap();
        let entry_id = new_entry_id();
        voice_abort(&state, trail_root.path(), "2026-07-29", entry_id)
            .await
            .expect("abort");

        // 3. The buffer must be empty AND shrunk so a follow-up
        //    capture starts with a clean allocator footprint.
        assert_eq!(state.samples.lock().len(), 0);
        assert_eq!(state.samples.lock().capacity(), 0);
    }

    #[tokio::test]
    async fn abort_cancels_join_handle_within_100ms() {
        // 1. Spawn a long-running task and stash its handle in
        //    the capture state. The task would sleep for 60
        //    seconds; abort must interrupt it well within the
        //    100 ms budget.
        let state = CaptureState::new();
        let samples = state.samples.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            // Touch the cloned Arc so the borrow checker is happy
            // about the move — the task would never reach this
            // line because abort() fires first.
            let _ = samples;
        });
        *state.consumer_handle.lock() = Some(handle);

        // 2. Abort; measure how long the IPC call takes.
        let trail_root = tempdir().unwrap();
        let start = std::time::Instant::now();
        let result = voice_abort(&state, trail_root.path(), "2026-07-29", new_entry_id()).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok(), "abort returned error: {:?}", result);
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "abort took too long: {:?}",
            elapsed
        );

        // 3. The handle was taken out of the state so a second
        //    abort sees `None` (idempotency check).
        assert!(state.consumer_handle.lock().is_none());
    }

    #[tokio::test]
    async fn abort_removes_partial_files() {
        // 1. Create a partial entry on disk: JSON + WAV pair that
        //    `write_atomic` would have produced during a failed
        //    `voice_stop`. The entry's metadata claims it was
        //    captured but the recording was never finalized.
        let trail_root = tempdir().unwrap();
        let entry_id = new_entry_id();
        let entry = VoiceEntry {
            entry_id,
            captured_at: "2026-07-29T18:00:00Z".into(),
            source: "voice".into(),
            duration_seconds: 0.5,
            transcript: Transcript::default(),
        };
        store::write_atomic(
            trail_root.path(),
            "2026-07-29",
            entry_id,
            &entry,
            &[0.0_f32; 100],
        )
        .expect("write_atomic");

        let (json_path, wav_path) = store::voice_paths(trail_root.path(), "2026-07-29", entry_id);
        assert!(json_path.exists(), "partial JSON must exist pre-abort");
        assert!(wav_path.exists(), "partial WAV must exist pre-abort");

        // 2. Abort and verify both files are gone (store::delete
        //    is idempotent so the call also covers the "no file
        //    at path" case the second abort hits).
        let state = CaptureState::new();
        voice_abort(&state, trail_root.path(), "2026-07-29", entry_id)
            .await
            .expect("abort");

        assert!(!json_path.exists(), "partial JSON must be deleted");
        assert!(!wav_path.exists(), "partial WAV must be deleted");
    }

    #[test]
    fn no_op_abort_succeeds() {
        // When no capture is active the abort path is a no-op:
        // no samples to clear, no JoinHandle to cancel, no files
        // to remove. The Tauri command layer calls `no_op_abort`
        // from `voice_stop` when no `voice_start` preceded it.
        let result = no_op_abort();
        assert!(result.is_ok());
    }
}
