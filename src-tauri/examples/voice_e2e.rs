//! Headless end-to-end harness for the Phase 5 voice pipeline.
//!
//! Run with:
//!   cargo run -p trail --example voice_e2e -- <path-to-5s-wav.wav>
//!
//! What it does:
//!   1. Decodes a 5-second WAV fixture from argv[1] via `hound`
//!   2. Runs the real `voice::transcriber::transcribe` pipeline
//!      against the decoded samples (using the lazy `WhisperContext`)
//!   3. Writes a `VoiceEntry` (JSON + WAV) via `voice::store::write_atomic`
//!      to `~/.trail/raw/<date>/voice/<entry_id>.{json,wav}` — matching
//!      the §5.5 `VoiceStore` on-disk contract
//!   4. Prints the output JSON path + the transcript length to stdout
//!   5. Exits 0 on success
//!
//! Model handling: the example requires the real whisper model
//! (`~/.trail/models/ggml-base.en.bin`, or the path set in
//! `TRAIL_WHISPER_MODEL`). If the model file is missing on disk, the
//! example prints `MODEL NOT FOUND — download via §5.1` and exits 0 —
//! the e2e bash script's skip-mode stays correct on hosts that
//! intentionally don't have the 150 MB model cached (CI / headless
//! build hosts). The macOS load-bearing proof lives in
//! `tests/MACOS_PHASE5_CHECKLIST.md`.

use std::path::PathBuf;

use trail_lib::voice::{new_entry_id, transcribe, write_atomic, VoiceEntry};

const SAMPLE_RATE: u32 = 16_000;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ---- 1. Resolve + decode the WAV fixture ----------------------------
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: voice_e2e <path-to-5s-wav.wav>");
        std::process::exit(2);
    }
    let wav_in = PathBuf::from(&args[1]);
    if !wav_in.exists() {
        eprintln!("input WAV not found: {}", wav_in.display());
        std::process::exit(2);
    }
    let samples = decode_wav(&wav_in)?;

    // ---- 2. Resolve the whisper model -----------------------------------
    // Per §5.5 the lazy context looks at `TRAIL_WHISPER_MODEL`. The
    // default convention is `~/.trail/models/ggml-base.en.bin` (set by
    // the §5.1 `model_manager::ensure_model` helper on first run).
    let model_path = std::env::var("TRAIL_WHISPER_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home)
                .join(".trail")
                .join("models")
                .join("ggml-base.en.bin")
        });

    if !model_path.exists() {
        // Skip mode — the model is the macOS load-bearing artifact,
        // not the headless build host's. We honor the script's
        // skip-mode contract (exit 0) so the e2e harness stays
        // PR-able from CI.
        println!(
            "MODEL NOT FOUND — download via §5.1: {}",
            model_path.display()
        );
        return Ok(());
    }

    // Tell the transcriber where the model is BEFORE invoking it —
    // the lazy context reads this on first call.
    std::env::set_var("TRAIL_WHISPER_MODEL", &model_path);

    // ---- 3. Run the real Rust transcription pipeline --------------------
    let transcript = transcribe(&samples).await?;

    // ---- 4. Write JSON + WAV via the §5.5 store -------------------------
    let trail_root = std::env::var("TRAIL_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".trail")
        });
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let entry_id = new_entry_id();
    let entry = VoiceEntry {
        entry_id,
        captured_at: chrono::Local::now().to_rfc3339(),
        source: "voice".to_string(),
        duration_seconds: samples.len() as f32 / SAMPLE_RATE as f32,
        transcript: transcript.clone(),
    };

    write_atomic(&trail_root, &date, entry_id, &entry, &samples)?;

    // ---- 5. Print the output JSON path + transcript length ---------------
    let (json_path, _) = trail_lib::voice::voice_paths(&trail_root, &date, entry_id);
    println!("json: {}", json_path.display());
    println!("transcript_len: {}", transcript.text.len());
    println!("duration_seconds: {}", entry.duration_seconds);

    Ok(())
}

/// Decode a mono 16-bit PCM WAV into f32 samples in [-1, 1].
///
/// The Phase 5 pipeline always emits 16 kHz mono 16-bit PCM (see
/// `voice::store::write_atomic`'s `hound::WavSpec`). The transcriber
/// expects f32 in that same range. We accept any input WAV `hound`
/// can decode and rescale — the e2e harness is shape-checking, not
/// re-encoding, so the source WAV just needs to be 16 kHz mono
/// (which the upstream `voice::capture` + `voice::resample` always
/// produce, and which the bash e2e script generates via
/// `WavWriter`).
fn decode_wav(path: &std::path::Path) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE {
        eprintln!(
            "warning: input WAV is {} Hz, expected {} Hz (transcriber is mono 16 kHz)",
            spec.sample_rate, SAMPLE_RATE
        );
    }
    let mut out = Vec::with_capacity(reader.len() as usize);
    match spec.bits_per_sample {
        16 => {
            for s in reader.samples::<i16>() {
                let v = s? as f32 / i16::MAX as f32;
                out.push(v);
            }
        }
        32 if spec.sample_format == hound::SampleFormat::Float => {
            for s in reader.samples::<f32>() {
                out.push(s?);
            }
        }
        _ => {
            return Err(format!(
                "unsupported WAV format: {} bits / {:?}",
                spec.bits_per_sample, spec.sample_format
            )
            .into());
        }
    }
    Ok(out)
}
