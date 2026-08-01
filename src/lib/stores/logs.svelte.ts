import { listLogs, deleteLog } from "$lib/api/logs";
import type { LogEntry } from "$lib/types/logs";

/**
 * Module-scoped Svelte 5 store for the Logs UI. Uses the `$state`
 * rune (allowed in `.svelte.ts` files) instead of Svelte 4 stores.
 * Components import the reactive `logsState` object directly; mutating
 * its properties triggers fine-grained reactivity without subscribers.
 */

function todayIso(): string {
  const now = new Date();
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

interface LogsState {
  entries: LogEntry[];
  selectedDate: string;
  loading: boolean;
  error: string | null;
}

export const logsState: LogsState = $state({
  entries: [],
  selectedDate: todayIso(),
  loading: false,
  error: null,
});

export async function refresh(): Promise<void> {
  logsState.loading = true;
  logsState.error = null;
  try {
    logsState.entries = await listLogs(logsState.selectedDate);
  } catch (e) {
    logsState.error = String(e);
    logsState.entries = [];
  } finally {
    logsState.loading = false;
  }
}

export async function remove(source: string): Promise<void> {
  try {
    await deleteLog(logsState.selectedDate, source);
    await refresh();
  } catch (e) {
    logsState.error = String(e);
  }
}

export function selectDate(d: string): void {
  logsState.selectedDate = d;
  void refresh();
}
