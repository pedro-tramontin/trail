<script lang="ts">
  import { logsState, remove, refresh } from "$lib/stores/logs.svelte";
  import { getRawJson } from "$lib/api/logs";
  import LogsDetail from "./LogsDetail.svelte";

  /**
   * Timeline view for the Logs screen. Renders `logsState.entries`
   * chronologically in the order returned by the `list_logs` IPC
   * command (sorted oldest-first by `captured_at`, with `source`
   * as a deterministic tie-breaker — see `logs::list_logs`). Each
   * row can be expanded inline to reveal the raw JSON via
   * `LogsDetail`, or deleted via the per-row ✕ button.
   *
   * Auto-loads on mount via `$effect` so consumers don't have to
   * remember to call `refresh()` themselves.
   */

  let expanded = $state<string | null>(null);
  // Track the request token so we can ignore a stale response if
  // the user clicks a different row (or collapses) before the
  // current getRawJson() resolves. (PR #26 Copilot thread T2.)
  let rawToken = 0;
  let rawJson = $state<unknown>(null);
  let loadingRaw = $state(false);

  async function toggleExpand(source: string): Promise<void> {
    if (expanded === source) {
      expanded = null;
      rawJson = null;
      return;
    }
    expanded = source;
    // Clear any payload from a previously expanded row so we never
    // flash the old JSON while the new fetch is in flight, and
    // bump the token so an in-flight old request can't overwrite
    // this row's payload once it lands.
    rawJson = null;
    loadingRaw = true;
    const myToken = ++rawToken;
    try {
      const result = await getRawJson(logsState.selectedDate, source);
      if (myToken !== rawToken) return; // a newer fetch (or collapse) won
      rawJson = result;
    } catch (err) {
      if (myToken !== rawToken) return;
      // Leave rawJson null so the panel renders an error message
      // via LogsDetail instead of stale JSON. The error is logged
      // for the dev console but not surfaced as a banner — the
      // row was already expanded by user action.
      console.error("getRawJson failed for", source, err);
      rawJson = { error: String(err) };
    } finally {
      if (myToken === rawToken) loadingRaw = false;
    }
  }

  function onDelete(source: string, event: Event): void {
    // Stop the click from also triggering the row expand.
    event.stopPropagation();
    if (confirm(`Delete log ${source} for ${logsState.selectedDate}?`)) {
      void remove(source);
    }
  }

  function formatBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  }

  // Auto-load on mount.
  $effect(() => {
    void refresh();
  });
</script>

<section class="logs" data-testid="logs">
  <h2>Logs — {logsState.selectedDate}</h2>

  {#if logsState.loading}
    <p class="loading">Loading…</p>
  {/if}

  {#if logsState.error}
    <p class="error" role="alert">{logsState.error}</p>
  {/if}

  {#if !logsState.loading && logsState.entries.length === 0}
    <p class="empty">No logs for this day.</p>
  {/if}

  <ul class="timeline" role="list">
    {#each logsState.entries as entry (entry.source)}
      <li class="row" role="listitem" data-testid="row-{entry.source}">
        <div class="row-main">
          <button
            class="row-button"
            type="button"
            onclick={() => toggleExpand(entry.source)}
            aria-expanded={expanded === entry.source}
          >
            <span class="source">{entry.source}</span>
            <span class="time">{entry.captured_at}</span>
            <span class="size">{formatBytes(entry.size_bytes)}</span>
          </button>
          <button
            class="delete"
            type="button"
            aria-label="Delete {entry.source} log"
            onclick={(e) => onDelete(entry.source, e)}
          >
            ✕
          </button>
        </div>
        {#if expanded === entry.source}
          <div class="detail" data-testid="detail">
            {#if loadingRaw}
              <p>Loading JSON…</p>
            {:else}
              <LogsDetail json={rawJson} />
            {/if}
          </div>
        {/if}
      </li>
    {/each}
  </ul>
</section>

<style>
  .logs {
    padding: 1rem;
  }
  .timeline {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .row {
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
    margin-bottom: 0.5rem;
    overflow: hidden;
  }
  .row-main {
    display: flex;
    align-items: stretch;
  }
  .row-button {
    flex: 1 1 auto;
    display: flex;
    justify-content: space-between;
    padding: 0.5rem 1rem;
    background: none;
    border: none;
    text-align: left;
    cursor: pointer;
  }
  .row-button:hover {
    background: var(--hover, #f5f5f5);
  }
  .source {
    font-weight: 600;
    min-width: 8rem;
  }
  .time {
    color: var(--muted, #666);
    font-family: monospace;
  }
  .size {
    color: var(--muted, #666);
    margin-left: 1rem;
  }
  .delete {
    flex: 0 0 auto;
    background: none;
    border: none;
    border-left: 1px solid var(--border, #ccc);
    color: var(--danger, #c00);
    cursor: pointer;
    padding: 0.5rem 1rem;
  }
  .detail {
    background: var(--detail-bg, #fafafa);
    padding: 1rem;
    border-top: 1px solid var(--border, #ccc);
  }
  .empty,
  .loading,
  .error {
    color: var(--muted, #666);
    padding: 1rem;
  }
  .error {
    color: var(--danger, #c00);
  }
</style>