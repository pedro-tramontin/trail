<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { ScanReport, CollectorCandidate } from "./types";

  /**
   * Step 2 — Non-invasive laptop scan (item 6-1).
   *
   * On mount, fires `scan_laptop_cmd` (the Tauri command name
   * from src-tauri/src/onboarding/scan.rs). Shows a loading
   * state while in flight, then renders the returned
   * `ScanReport` as a per-collector findings list grouped by
   * status (Available / Unavailable / AlreadyConfigured).
   *
   * Auto-advances to StepAsk once the scan resolves
   * successfully by calling the `on_next` prop with the
   * `ScanReport` as the argument. The parent
   * `Onboarding.svelte` updates `scan_report` + `current_step`.
   */

  let { on_next }: { on_next: (report: ScanReport) => void } = $props();

  let report = $state<ScanReport | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let auto_advance_timer: ReturnType<typeof setTimeout> | null = null;

  async function run_scan(): Promise<void> {
    loading = true;
    error = null;
    report = null;
    if (auto_advance_timer !== null) {
      clearTimeout(auto_advance_timer);
      auto_advance_timer = null;
    }
    try {
      const result = await invoke<ScanReport>("scan_laptop_cmd");
      report = result;
      loading = false;
      // Auto-advance: the spec calls for the scan step to
      // transition to StepAsk on success. The 800ms delay
      // gives the user a moment to read the findings.
      auto_advance_timer = setTimeout(() => {
        auto_advance_timer = null;
        if (report) on_next(report);
      }, 800);
    } catch (err) {
      error = String(err);
      loading = false;
    }
  }

  $effect(() => {
    void run_scan();
    return () => {
      if (auto_advance_timer !== null) {
        clearTimeout(auto_advance_timer);
      }
    };
  });

  function available_count(list: CollectorCandidate[]): number {
    return list.filter((c) => c.status === "available").length;
  }
  function unavailable_count(list: CollectorCandidate[]): number {
    return list.filter((c) => c.status === "unavailable").length;
  }
  function configured_count(list: CollectorCandidate[]): number {
    return list.filter((c) => c.status === "already_configured").length;
  }
</script>

<section class="step" data-testid="step-scan">
  <h2>Scanning your laptop</h2>

  {#if loading}
    <p class="loading" data-testid="scan-loading">
      <span class="spinner" aria-hidden="true">⏳</span> Looking for the data
      sources Trail can use…
    </p>
  {:else if error}
    <p class="error" role="alert" data-testid="scan-error">
      Scan failed: {error}
    </p>
    <div class="actions">
      <button
        type="button"
        class="primary"
        data-testid="scan-retry"
        onclick={() => {
          void run_scan();
        }}
      >
        Retry scan
      </button>
    </div>
  {:else if report}
    <p class="muted" data-testid="scan-summary">
      Found <strong>{available_count(report.candidates)}</strong> available
      source{available_count(report.candidates) === 1 ? "" : "s"},
      {unavailable_count(report.candidates)} not detected,
      {configured_count(report.candidates)} already installed.
    </p>

    <ul class="findings" data-testid="scan-findings">
      {#each report.candidates as c (c.collector_id)}
        <li class="finding" data-testid="finding-{c.collector_id}">
          <span class="name">{c.display_name}</span>
          <span class="status status-{c.status}">
            {c.status.replace("_", " ")}
          </span>
          {#if c.notes}
            <span class="notes">{c.notes}</span>
          {/if}
        </li>
      {/each}
    </ul>

    <p class="muted" data-testid="scan-auto-advance">
      Auto-advancing to the next step…
    </p>
  {/if}
</section>

<style>
  .step {
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .loading {
    color: var(--muted, #666);
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .error {
    color: var(--danger, #c00);
  }
  .muted {
    color: var(--muted, #666);
    font-size: 0.9rem;
  }
  .findings {
    list-style: none;
    padding: 0;
    margin: 0;
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
  }
  .finding {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0.25rem 0.75rem;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--border, #ccc);
  }
  .finding:last-child {
    border-bottom: none;
  }
  .name {
    font-weight: 500;
  }
  .status {
    font-size: 0.8rem;
    text-transform: capitalize;
    padding: 0.1rem 0.4rem;
    border-radius: 3px;
    align-self: start;
  }
  .status-available {
    background: #dcfce7;
    color: #166534;
  }
  .status-unavailable {
    background: #f1f5f9;
    color: #475569;
  }
  .status-already_configured {
    background: #dbeafe;
    color: #1e40af;
  }
  .notes {
    grid-column: 1 / -1;
    font-size: 0.85rem;
    color: var(--muted, #666);
  }
  .actions {
    margin-top: 1rem;
    display: flex;
    justify-content: flex-end;
  }
  .primary {
    background: var(--primary, #2563eb);
    color: white;
    border: none;
    padding: 0.5rem 1.25rem;
    border-radius: 4px;
    cursor: pointer;
    font-weight: 600;
  }
  .primary:hover {
    background: var(--primary-hover, #1d4ed8);
  }
  .spinner {
    display: inline-block;
  }
</style>
