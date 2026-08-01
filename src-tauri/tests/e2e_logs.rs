//! E2E for the §4.1 logs backend. Verifies that `list_logs`,
//! `delete_log`, and `get_raw_json` work against a realistic
//! `trail_root` layout produced by the Phase 2 collectors.
//!
//! The fixtures live under `tests/fixtures/raw/2026-07-29/` and are
//! checked into the repo from §4.2. Here we exercise the *pure
//! function* entry points (no Tauri `AppHandle`, no IPC) so this
//! test runs in any environment — the actual Tauri IPC commands
//! are exercised on Pedro's Mac.
//!
//! Run with:
//!   cargo test --test e2e_logs -- --nocapture
//! or via the bash wrapper:
//!   bash tests/e2e_logs.sh

use std::path::{Path, PathBuf};

use trail_lib::logs::{delete_log, get_raw_json, list_logs};

/// Copy the fixture files for `day` into `dst/raw/<day>/`. Returns
/// the path to the freshly-populated raw dir.
fn seed_fixtures(src_raw_root: &Path, dst_trail_root: &Path, day: &str) -> PathBuf {
    let src_day = src_raw_root.join(day);
    let dst_day = dst_trail_root.join("raw").join(day);
    std::fs::create_dir_all(&dst_day).expect("create dst day dir");
    let entries = std::fs::read_dir(&src_day)
        .unwrap_or_else(|e| panic!("missing fixtures in {}: {e}", src_day.display()));
    let mut copied = 0usize;
    for entry in entries {
        let entry = entry.expect("read fixture dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read fixture file");
        let name = path.file_name().expect("fixture file_name");
        std::fs::write(dst_day.join(name), &bytes).expect("write dst fixture");
        copied += 1;
    }
    assert!(
        copied >= 4,
        "expected >=4 fixture JSONs for {day}, copied {copied}"
    );
    dst_day
}

#[test]
fn e2e_logs_full_flow() {
    // 1. Resolve the fixture source dir (REPO_ROOT/tests/fixtures/raw)
    //    and a fresh temp TRAIL_HOME.
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_dir
        .parent()
        .expect("src-tauri must live one level under repo root");
    let src_raw_root = repo_root.join("tests").join("fixtures").join("raw");
    assert!(
        src_raw_root.join("2026-07-29").exists(),
        "fixtures missing at {}/2026-07-29",
        src_raw_root.display()
    );

    let tmp = tempfile::tempdir().unwrap();
    let trail_root = tmp.path();
    let day = "2026-07-29";
    let raw_dir = seed_fixtures(&src_raw_root, trail_root, day);

    // 2. list_logs returns chronologically-ordered entries (calendar
    //    17:30 → claude_sessions 17:45 → voice 17:50; github 18:00
    //    was added in §4.2 so it sits last).
    let entries = list_logs(trail_root, day).expect("list_logs should succeed");
    assert_eq!(entries.len(), 4, "expected 4 fixtures (incl github)");
    assert_eq!(entries[0].source, "calendar");
    assert_eq!(entries[0].captured_at, "2026-07-29T17:30:00Z");
    assert_eq!(entries[1].source, "claude_sessions");
    assert_eq!(entries[1].captured_at, "2026-07-29T17:45:00Z");
    assert_eq!(entries[2].source, "voice");
    assert_eq!(entries[2].captured_at, "2026-07-29T17:50:00Z");
    assert_eq!(entries[3].source, "github");
    assert_eq!(entries[3].captured_at, "2026-07-29T18:00:00Z");
    for e in &entries {
        assert_eq!(e.date, day, "date must be the query arg");
        assert!(e.size_bytes > 0, "{} size_bytes should be > 0", e.source);
        assert!(Path::new(&e.path).is_absolute());
    }

    // 3. get_raw_json parses a file (calendar fixture carries a
    //    `payload.events` array).
    let calendar = get_raw_json(trail_root, day, "calendar").expect("get_raw_json should succeed");
    assert_eq!(calendar["source"], "calendar");
    assert_eq!(calendar["captured_at"], "2026-07-29T17:30:00Z");
    assert_eq!(calendar["date"], day);
    assert!(calendar["payload"]["events"].is_array());

    // 4. delete_log removes a file.
    let calendar_path = raw_dir.join("calendar.json");
    assert!(calendar_path.exists(), "fixture should exist before delete");
    delete_log(trail_root, day, "calendar").expect("delete_log should succeed");
    assert!(
        !calendar_path.exists(),
        "fixture should be gone after delete"
    );

    // 5. delete_log is idempotent (re-call doesn't error).
    delete_log(trail_root, day, "calendar").expect("delete_log idempotent");

    // 6. After delete, list_logs has 3 entries and no calendar.
    let entries_after = list_logs(trail_root, day).expect("list_logs after delete");
    assert_eq!(entries_after.len(), 3);
    assert!(
        entries_after.iter().all(|e| e.source != "calendar"),
        "calendar should be gone after delete"
    );

    // 7. Missing day returns empty list (not an error).
    let empty = list_logs(trail_root, "2099-01-01").expect("list_logs on missing day");
    assert_eq!(empty.len(), 0);

    // 8. get_raw_json on a deleted source returns an error.
    let missing = get_raw_json(trail_root, day, "calendar");
    assert!(
        missing.is_err(),
        "get_raw_json on a deleted source must error"
    );

    println!(
        "e2e_logs: PASS — list/delete/get all work; idempotent; \
         missing day handled; {count} fixtures exercised (incl github)",
        count = entries.len()
    );
}
