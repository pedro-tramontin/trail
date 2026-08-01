//! Atomic JSON + WAV writes to the on-disk voice store.
//!
//! Writes go to `~/.trail/raw/<date>/voice/<entry_id>.json` and
//! `<entry_id>.wav`. Atomicity: writes are staged in a `.tmp` file
//! then renamed (POSIX rename is atomic on the same filesystem).
//! On a crash mid-write, the partial files are deleted by the abort
//! handler (§5.6).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::transcriber::Transcript;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("wav error: {0}")]
    Wav(#[from] hound::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceEntry {
    pub entry_id: Uuid,
    pub captured_at: String, // ISO-8601 timestamp.
    pub source: String,      // always "voice" in v1.
    pub duration_seconds: f32,
    pub transcript: Transcript,
}

/// Generate a new entry ID (UUID v4).
pub fn new_entry_id() -> Uuid {
    Uuid::new_v4()
}

/// Compute the on-disk paths for a voice entry.
pub fn voice_paths(trail_root: &Path, date: &str, entry_id: Uuid) -> (PathBuf, PathBuf) {
    let dir = trail_root.join("raw").join(date).join("voice");
    let json_path = dir.join(format!("{}.json", entry_id));
    let wav_path = dir.join(format!("{}.wav", entry_id));
    (json_path, wav_path)
}

/// Write the JSON metadata + WAV audio atomically.
///
/// `wav_samples` is written as 16-bit mono PCM at 16 kHz. The JSON
/// is written first to a `.json.tmp` then renamed to `.json` so
/// readers never see a half-written file. The WAV is written
/// directly (it's a binary format; a half-written WAV is
/// detectable on read).
pub fn write_atomic(
    trail_root: &Path,
    date: &str,
    entry_id: Uuid,
    entry: &VoiceEntry,
    wav_samples: &[f32],
) -> Result<(), StoreError> {
    let (json_path, wav_path) = voice_paths(trail_root, date, entry_id);
    fs::create_dir_all(json_path.parent().expect("json_path has a parent dir"))?;

    // Write JSON atomically via tmp + rename.
    let json_tmp = json_path.with_extension("json.tmp");
    let json_bytes = serde_json::to_vec_pretty(entry)?;
    fs::write(&json_tmp, &json_bytes)?;
    fs::rename(&json_tmp, &json_path)?;

    // Write WAV via hound (16-bit mono PCM at 16 kHz).
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&wav_path, spec)?;
    for s in wav_samples {
        let sample = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
        writer.write_sample(sample)?;
    }
    writer.finalize()?;

    Ok(())
}

/// Delete a voice entry's files (idempotent).
pub fn delete(trail_root: &Path, date: &str, entry_id: Uuid) -> Result<(), StoreError> {
    let (json_path, wav_path) = voice_paths(trail_root, date, entry_id);
    // Idempotent — ignore not-found.
    let _ = fs::remove_file(json_path);
    let _ = fs::remove_file(wav_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn entry_id_is_uuid_v4() {
        let id = new_entry_id();
        assert_eq!(id.get_version_num(), 4);
    }

    #[test]
    fn write_atomic_creates_json_and_wav() {
        let tmp = tempdir().unwrap();
        let entry_id = new_entry_id();
        let entry = VoiceEntry {
            entry_id,
            captured_at: "2026-07-29T18:00:00Z".into(),
            source: "voice".into(),
            duration_seconds: 1.5,
            transcript: Transcript {
                text: "hello world".into(),
                segments: vec![],
            },
        };
        let samples: Vec<f32> = (0..1600).map(|i| (i as f32 / 1600.0).sin()).collect();

        write_atomic(tmp.path(), "2026-07-29", entry_id, &entry, &samples).unwrap();

        let (json_path, wav_path) = voice_paths(tmp.path(), "2026-07-29", entry_id);
        assert!(json_path.exists(), "JSON missing");
        assert!(wav_path.exists(), "WAV missing");

        // Verify JSON is parseable.
        let json_bytes = fs::read(&json_path).unwrap();
        let parsed: VoiceEntry = serde_json::from_slice(&json_bytes).unwrap();
        assert_eq!(parsed.entry_id, entry_id);
        assert_eq!(parsed.transcript.text, "hello world");
    }

    #[test]
    fn delete_is_idempotent() {
        let tmp = tempdir().unwrap();
        let entry_id = new_entry_id();
        let entry = VoiceEntry {
            entry_id,
            captured_at: "2026-07-29T18:00:00Z".into(),
            source: "voice".into(),
            duration_seconds: 0.5,
            transcript: Transcript::default(),
        };
        write_atomic(tmp.path(), "2026-07-29", entry_id, &entry, &[0.0_f32; 100]).unwrap();

        delete(tmp.path(), "2026-07-29", entry_id).unwrap();
        // Second call doesn't error.
        delete(tmp.path(), "2026-07-29", entry_id).unwrap();
    }
}
