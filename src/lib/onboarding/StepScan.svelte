<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { ScanReport, CollectorCandidate } from "./types";

  /**
   * Step 2 — Non-invasive laptop scan (item 6-1).
   *
   * On mount, fires `scan_laptop_cmd`. On success, shows a
   * 10-second auto-advance countdown at the bottom of the step
   * that the user can stop, skip, or let run. Three explicit
   * controls — no surprise transitions:
   *
   *   - "Stop countdown" — cancel the auto-advance; user stays
   *     on this step until they click Continue now.
   *   - "Continue now"   — skip the countdown and advance
   *     immediately.
   *   - "Resume countdown" (after Stop) — restart the 10s timer.
   *
   * The countdown state is exposed as three values via
   * `countdown_state`: "ticking" | "stopped" | null. The
   * control row renders the matching affordance. The user can
   * also click the wizard's "← Back" to revisit this step at
   * any time (no timer fighting that).
   *
   * 800ms was the original delay; user feedback said it felt
   * like the step "scared" them and they had to click back 3+
   * times to read the findings. 10s with explicit Stop is the
   * replacement.
   */

  let { on_next }: { on_next: (report: ScanReport) => void } = $props();

  const AUTO_ADVANCE_SECONDS = 10;

  let report = $state<ScanReport | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  /** Remaining seconds on the auto-advance countdown.
   *  `null` = no countdown visible (loading or stopped).
   *  number = ticking down to 0, then auto-advance. */
  let countdown = $state<number | null>(null);
  /** "ticking" = countdown running; "stopped" = user paused it. */
  let countdown_state = $state<"ticking" | "stopped" | null>(null);
  let interval_timer: ReturnType<typeof setInterval> | null = null;

  function clear_timers(): void {
    if (interval_timer !== null) {
      clearInterval(interval_timer);
      interval_timer = null;
    }
  }

  function start_countdown(): void {
    clear_timers();
    countdown = AUTO_ADVANCE_SECONDS;
    countdown_state = "ticking";
    interval_timer = setInterval(() => {
      if (countdown === null) return;
      if (countdown <= 1) {
        clear_timers();
        countdown = null;
        countdown_state = null;
        if (report) on_next(report);
      } else {
        countdown -= 1;
      }
    }, 1000);
  }

  function stop_countdown(): void {
    clear_timers();
    countdown_state = "stopped";
    countdown = null;
  }

  function resume_countdown(): void {
    if (countdown_state === "ticking") return;
    start_countdown();
  }

  function continue_now(): void {
    clear_timers();
    countdown = null;
    countdown_state = null;
    if (report) on_next(report);
  }

  async function run_scan(): Promise<void> {
    loading = true;
    error = null;
    report = null;
    countdown = null;
    countdown_state = null;
    clear_timers();
    try {
      const result = await invoke<ScanReport>("scan_laptop_cmd");
      report = result;
      loading = false;
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
      {#if countdown_state === "ticking"}
        <span class="countdown" data-testid="scan-countdown">
          Auto-advancing in {countdown}s…
        </span>
        <div class="controls">
          <button
            type="button"
            class="link"
            data-testid="scan-stop-countdown"
            onclick={stop_countdown}
          >
            Stop countdown
          </button>
          <button
            type="button"
            class="link"
            data-testid="scan-continue-now"
            onclick={continue_now}
          >
            Continue now
          </button>
        </div>
      {:else if countdown_state === "stopped"}
        <span class="countdown stopped" data-testid="scan-countdown">
          Auto-advance paused
        </span>
        <div class="controls">
          <button
            type="button"
            class="link"
            data-testid="scan-resume-countdown"
            onclick={resume_countdown}
          >
            Resume countdown
          </button>
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
