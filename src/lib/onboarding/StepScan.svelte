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
   * successfully, after a 10-second countdown so the user has
   * time to read the findings. The countdown is visible at the
   * bottom of the step ("Auto-advancing in N…") and is
   * cancellable via the "Continue now" button, which advances
   * immediately. The user can also click "← Back" to revisit
   * this step (the wizard's nav button).
   *
   * The 10s default replaces the previous 800ms implementation
   * after feedback that the auto-advance felt jarring — users
   * had to click back 3+ times to actually read the scan
   * findings. The countdown gives them a clear time window
   * to read + an explicit control to skip it.
   */

  let { on_next }: { on_next: (report: ScanReport) => void } = $props();

  /** Seconds the user gets to read the findings before
   *  auto-advance. Tweakable for tests. */
  const AUTO_ADVANCE_SECONDS = 10;

  let report = $state<ScanReport | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  /** Remaining seconds on the auto-advance countdown. Starts
   *  at `null` (no countdown running) and goes to
   *  AUTO_ADVANCE_SECONDS when the scan resolves. */
  let countdown = $state<number | null>(null);
  let interval_timer: ReturnType<typeof setInterval> | null = null;
  let advance_timer: ReturnType<typeof setTimeout> | null = null;

  function clear_timers(): void {
    if (interval_timer !== null) {
      clearInterval(interval_timer);
      interval_timer = null;
    }
    if (advance_timer !== null) {
      clearTimeout(advance_timer);
      advance_timer = null;
    }
  }

  function start_countdown(): void {
    clear_timers();
    countdown = AUTO_ADVANCE_SECONDS;
    interval_timer = setInterval(() => {
      if (countdown === null) return;
      if (countdown <= 1) {
        clear_timers();
        countdown = null;
        if (report) on_next(report);
      } else {
        countdown -= 1;
      }
    }, 1000);
  }

  function continue_now(): void {
    clear_timers();
    countdown = null;
    if (report) on_next(report);
  }

  async function run_scan(): Promise<void> {
    loading = true;
    error = null;
    report = null;
    countdown = null;
    clear_timers();
    try {
      const result = await invoke<ScanReport>("scan_laptop_cmd");
      report = result;
      loading = false;
      // Start the visible auto-advance countdown so the user
      // can read the findings before we transition.
      start_countdown();
    } catch (err) {
      error = String(err);
      loading = false;
    }
  }

  $effect(() => {
    void run_scan();
    return () => {
      clear_timers();
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

    <div class="auto-advance" data-testid="scan-auto-advance">
      <span class="countdown" data-testid="scan-countdown">
        {#if countdown !== null}
          Auto-advancing in {countdown}s…
        {:else}
          Auto-advancing to the next step…
        {/if}
      </span>
      <button
        type="button"
        class="link"
        data-testid="scan-continue-now"
        onclick={continue_now}
      >
        Continue now
      </button>
    </div>
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
  .auto-advance {
    margin-top: 1rem;
    padding-top: 0.75rem;
    border-top: 1px solid var(--border, #ccc);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    font-size: 0.9rem;
  }
  .countdown {
    color: var(--muted, #666);
  }
  .link {
    background: transparent;
    border: none;
    color: var(--primary, #2563eb);
    cursor: pointer;
    padding: 0.25rem 0.5rem;
    font-size: 0.9rem;
    font-weight: 500;
  }
  .link:hover {
    text-decoration: underline;
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
