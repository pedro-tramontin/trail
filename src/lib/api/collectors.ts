import { invoke } from "@tauri-apps/api/core";

/** Mirrors the Rust `CollectorSource` enum in `src-tauri/src/collectors.rs`. */
export type CollectorSource = "github" | "claude_sessions" | "calendar";

/**
 * Mirrors the Rust `collectors::CollectorInfo` struct returned by
 * `list_collectors()`. `last_run_at` is an ISO-8601 string (or `null`
 * if the collector has never run); `last_exit_code` is the process
 * exit code of the most recent invocation (`null` until first run).
 */
export interface CollectorInfo {
  source: CollectorSource;
  enabled: boolean;
  schedule: string;
  last_run_at: string | null;
  last_exit_code: number | null;
  last_error: string | null;
}

/**
 * List every collector's current state (enabled, schedule, last run).
 * Returns rows in canonical order so the Settings UI renders them
 * in a stable position.
 */
export async function listCollectors(
  configPath: string,
  collectorBin: string,
): Promise<CollectorInfo[]> {
  return invoke<CollectorInfo[]>("list_collectors", {
    configPath,
    collectorBin,
  });
}

/**
 * Run one collector now (used by the "Run now" button on each Settings
 * row). Returns the collector's exit code (0 = success).
 */
export async function runCollectorNow(
  source: CollectorSource,
  configPath: string,
  collectorBin: string,
): Promise<number> {
  return invoke<number>("run_collector_now", {
    source,
    configPath,
    collectorBin,
  });
}

/**
 * Flip a collector's enabled toggle. The Rust side persists the new
 * state to `~/.trail/config.json` so the cron scheduler sees it on
 * the next tick.
 */
export async function setCollectorEnabled(
  source: CollectorSource,
  enabled: boolean,
  configPath: string,
  collectorBin: string,
): Promise<void> {
  return invoke<void>("set_collector_enabled", {
    source,
    enabled,
    configPath,
    collectorBin,
  });
}
