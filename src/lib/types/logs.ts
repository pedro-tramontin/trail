/**
 * Mirrors the Rust `logs::LogEntry` struct returned by the
 * `list_logs` Tauri command. Fields are ordered to match the
 * Rust definition for visual diff-ability.
 */
export interface LogEntry {
  source: string;
  captured_at: string;
  size_bytes: number;
  path: string;
  date: string;
}
