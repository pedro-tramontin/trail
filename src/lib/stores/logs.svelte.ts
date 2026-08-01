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
  // Capture the date at the start so a stale response from an
  // earlier `refresh()` (the user clicked through dates quickly)
  // can't overwrite the state for a date the user is no longer
  // viewing. Without this, the response from an older call
  // could land after `selectedDate` had been changed, leaving
  // `entries` showing data for the wrong day.
  const targetDate = logsState.selectedDate;
  logsState.loading = true;
  logsState.error = null;
  try {
    const entries = await listLogs(targetDate);
    // Only commit if the date is still the one we fetched for.
    if (logsState.selectedDate === targetDate) {
      logsState.entries = entries;
    }
  } catch (e) {
    if (logsState.selectedDate === targetDate) {
      logsState.error = String(e);
      logsState.entries = [];
    }
  } finally {
    if (logsState.selectedDate === targetDate) {
      logsState.loading = false;
    }
  }
}

export async function remove(source: string): Promise<void> {
  // Same date-snapshot pattern as `refresh()` — capture the date
  // at the start so the delete targets the date the user actually
  // clicked, even if they change the dropdown during the await.
  const targetDate = logsState.selectedDate;
  try {
    await deleteLog(targetDate, source);
  } catch (e) {
    if (logsState.selectedDate === targetDate) {
      logsState.error = String(e);
    }
    return;
  }
  // Re-fetch only if the user is still on the same date; otherwise
  // the next `refresh()` triggered by `selectDate` will fetch the
  // new view.
  if (logsState.selectedDate === targetDate) {
    await refresh();
  }
}

export function selectDate(d: string): void {
  logsState.selectedDate = d;
  void refresh();
}
