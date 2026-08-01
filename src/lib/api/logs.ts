import { invoke } from "@tauri-apps/api/core";
import type { LogEntry } from "$lib/types/logs";

/**
 * Thin IPC wrappers around the §4.1 Tauri commands. Every call
 * marshals typed arguments through Tauri's `invoke` channel and
 * returns the deserialized result.
 */

/** List all log files for the given `date` (YYYY-MM-DD). */
export async function listLogs(date: string): Promise<LogEntry[]> {
  return invoke<LogEntry[]>("list_logs", { date });
}

/** Delete the log file for `source` on `date`. */
export async function deleteLog(date: string, source: string): Promise<void> {
  await invoke("delete_log", { date, source });
}

/** Read the raw JSON payload for `source` on `date`. */
export async function getRawJson(
  date: string,
  source: string,
): Promise<unknown> {
  return invoke("get_raw_json", { date, source });
}
