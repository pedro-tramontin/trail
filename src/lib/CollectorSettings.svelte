<script lang="ts">
  import { onMount } from "svelte";
  import {
    type CollectorInfo,
    type CollectorSource,
    listCollectors,
    runCollectorNow,
    setCollectorEnabled,
  } from "./api/collectors";

  let {
    configPath,
    collectorBin,
  }: { configPath: string; collectorBin: string } = $props();

  let collectors = $state<CollectorInfo[]>([]);
  let running = $state<Record<CollectorSource, boolean>>({
    github: false,
    claude_sessions: false,
    calendar: false,
  });
  let error = $state<string | null>(null);

  const LABELS: Record<CollectorSource, string> = {
    github: "GitHub PRs",
    claude_sessions: "Claude sessions",
    calendar: "Calendar",
  };

  async function refresh() {
    try {
      collectors = await listCollectors(configPath, collectorBin);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function clearError() {
    if (error !== null) error = null;
  }

  onMount(refresh);

  async function toggle(source: CollectorSource, enabled: boolean) {
    clearError();
    try {
      await setCollectorEnabled(source, enabled, configPath, collectorBin);
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function runNow(source: CollectorSource) {
    clearError();
    running[source] = true;
    try {
      await runCollectorNow(source, configPath, collectorBin);
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      running[source] = false;
    }
  }
</script>

<section aria-label="Collectors" data-testid="collector-settings">
  {#if error}
    <p class="error" role="alert" data-testid="collector-error">{error}</p>
  {/if}
  {#each collectors as c (c.source)}
    <div class="row" data-testid="row-{c.source}">
      <label>
        <input
          type="checkbox"
          checked={c.enabled}
          data-testid="toggle-{c.source}"
          onchange={(e) => toggle(c.source, e.currentTarget.checked)}
        />
        {LABELS[c.source]}
      </label>
      <span data-testid="last-run-{c.source}">
        {#if c.last_run_at}last run: {new Date(
            c.last_run_at,
          ).toLocaleString()}{:else}last run: never{/if}
        {#if c.last_exit_code !== null && c.last_exit_code !== 0}⚠ exit {c.last_exit_code}{/if}
      </span>
      <button
        type="button"
        data-testid="run-now-{c.source}"
        disabled={running[c.source]}
        onclick={() => runNow(c.source)}
      >
        {running[c.source] ? "Running…" : "Run now"}
      </button>
    </div>
  {/each}
</section>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.5rem 0;
    border-bottom: 1px solid #eee;
  }
  .error {
    color: #b00020;
    background: #fdecea;
    padding: 0.5rem;
    border-radius: 4px;
    margin-bottom: 0.5rem;
  }
</style>
