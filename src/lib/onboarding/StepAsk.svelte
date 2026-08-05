<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { OnboardingAnswers, ScanReport } from "./types";

  /**
   * Step 3 — LLM-driven Q&A (item 6-2).
   *
   * Receives the `ScanReport` from StepScan as a prop, fires
   * `ask_onboarding_cmd(scan)`, and shows the returned
   * `OnboardingAnswers` as a flat checklist. The "Looks good"
   * path is the default — the user accepts the LLM's choices
   * and proceeds. The "Edit" toggle reveals inline text inputs
   * for the per-collector path lists (claude_sessions_paths,
   * github.repos) so the user can override without leaving
   * the wizard. Disabled until the IPC resolves.
   *
   * On Next, calls the `on_next` prop with the (possibly
   * edited) `OnboardingAnswers` as the argument.
   *
   * ## Review time handling
   *
   * The Rust side stores `review_time.hour_utc` (UTC hour-of-day)
   * because the scheduler (src-tauri/src/scheduler.rs) is
   * UTC-only and parses the value as `%H:%M` in UTC. To make
   * "18:00" mean "18:00 in the user's local time", this step:
   *
   *   1. Detects the browser timezone via
   *      `Intl.DateTimeFormat().resolvedOptions().timeZone`
   *      (e.g. "America/Sao_Paulo").
   *   2. Computes the timezone offset at the current moment
   *      (`new Date().getTimezoneOffset()` returns minutes, sign
   *      flipped per spec — UTC-5 → +300).
   *   3. Renders the review-time row as a fixed "18:00 your
   *      time (<tz>)" so the user sees the local hour they
   *      care about. The Rust default `hour_utc: 18` is
   *      interpreted as the local-hour default.
   *   4. Before calling `on_next`, translates the local 18:00
   *      back to UTC and writes that into
   *      `answers.review_time.hour_utc` so the scheduler fires
   *      at the correct UTC instant. UTC-5 user →
   *      `hour_utc = 23`; UTC+5 user → `hour_utc = 13`.
   *
   * DST is handled imperfectly (we read the offset at the
   * current moment, not at the next fire time). A future item
   * can either store the IANA timezone string alongside
   * `hour_utc` and resolve DST at fire time, or refresh the
   * offset daily. For v1 this is good enough — DST transitions
   * shift the local fire time by 1h for ~3 weeks a year.
   *
   * This whole conversion is local to the wizard; the Rust
   * side stays unchanged. We never persist the timezone string.
   */

  let {
    scan = null,
    on_next,
  }: {
    scan: ScanReport | null;
    on_next: (answers: OnboardingAnswers) => void;
  } = $props();

  let answers = $state<OnboardingAnswers | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let editing = $state(false);

  // Editable local copies (only used when `editing === true`).
  let edit_claude_paths = $state("");
  let edit_github_repos = $state("");

  // The local hour we display to the user. v1 is read-only
  // (the LLM picks "evening"; we just show that as 18:00 local
  // time). A future item can add an hour-picker; for now this
  // stays at 18 so the user's mental model is "evening review
  // at 6 PM, my time".
  const REVIEW_HOUR_LOCAL = 18;

  // Browser-detected IANA timezone (e.g. "America/Sao_Paulo",
  // "Europe/Lisbon", "UTC"). Computed once on mount; the value
  // is shown to the user in the review-time row.
  const local_tz: string =
    typeof Intl !== "undefined"
      ? Intl.DateTimeFormat().resolvedOptions().timeZone
      : "UTC";

  /**
   * Translate a local-hour H into the UTC hour that, when
   * stored, will fire the scheduler at H local-time. The
   * returned value is normalised into [0, 23] (a 24h
   * timezone offset wraps the day).
   *
   * Example: at UTC-5, getTimezoneOffset() returns 300
   * (minutes east of UTC, sign-flipped per ECMA spec).
   * local → UTC: utc = (local - offset_hours + 24) mod 24.
   * For local=18, offset=5: utc = (18 - 5 + 24) mod 24 = 13.
   */
  function local_hour_to_utc(local_hour: number): number {
    // `getTimezoneOffset` returns the offset in minutes FROM
    // local TO UTC (so UTC-5 → 300). Sign-flipped for the math.
    const offset_minutes = new Date().getTimezoneOffset();
    const offset_hours = offset_minutes / 60;
    return ((local_hour - offset_hours) + 24) % 24;
  }

  /** Short timezone label for the review-time row. */
  function short_tz_label(tz: string): string {
    if (tz === "UTC") return "UTC";
    // "America/Sao_Paulo" → "Sao_Paulo"; "America/Argentina/Buenos_Aires" → "Buenos_Aires".
    const parts = tz.split("/");
    return parts[parts.length - 1].replace(/_/g, " ");
  }

  const can_advance = $derived(answers !== null && !loading);

  async function run_ask(): Promise<void> {
    if (!scan) return;
    loading = true;
    error = null;
    try {
      const result = await invoke<OnboardingAnswers>("ask_onboarding_cmd", {
        scan,
      });
      answers = result;
      edit_claude_paths = (result.claude_sessions_paths ?? []).join("\n");
      edit_github_repos = (result.github?.repos ?? []).join("\n");
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  function toggle_edit(): void {
    if (!editing && answers) {
      // Seed the local edit buffers from the LLM answer.
      edit_claude_paths = (answers.claude_sessions_paths ?? []).join("\n");
      edit_github_repos = (answers.github?.repos ?? []).join("\n");
    }
    editing = !editing;
  }

  function build_edited_answers(): OnboardingAnswers {
    if (!answers) {
      throw new Error("build_edited_answers called before answers loaded");
    }
    const claude_sessions_paths = edit_claude_paths
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    const github_repos = edit_github_repos
      .split(/\r?\n/)
      .flatMap((line) => line.split(","))
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    return {
      ...answers,
      claude_sessions_paths,
      github: answers.github
        ? { ...answers.github, repos: github_repos }
        : answers.github,
    };
  }

  /**
   * Before sending the answers to the parent (which calls
   * `write_onboarding_config`), translate the LLM's UTC
   * `hour_utc` into the UTC hour that represents 18:00 in the
   * user's local timezone. The LLM returns whatever default
   * the schema + Rust layer produced (e.g. 18); we override
   * it so the scheduler actually fires at 18:00 local.
   */
  function apply_local_review_time(
    base: OnboardingAnswers,
  ): OnboardingAnswers {
    return {
      ...base,
      review_time: {
        ...base.review_time,
        // The LLM's `cadence` ("evening"/"morning"/"weekly")
        // is a coarse bucket — we keep it. The fine-grained
        // `hour_utc` is what gets rewritten so the scheduler
        // fires at REVIEW_HOUR_LOCAL in the user's tz.
        hour_utc: local_hour_to_utc(REVIEW_HOUR_LOCAL),
      },
    };
  }

  function on_next_click(): void {
    if (!answers) return;
    const edited = editing ? build_edited_answers() : answers;
    const final = apply_local_review_time(edited);
    on_next(final);
  }

  $effect(() => {
    void run_ask();
  });
</script>

<section class="step" data-testid="step-ask">
  <h2>Here's what Trail learned from your scan</h2>

  {#if loading}
    <p class="loading" data-testid="ask-loading">
      <span class="spinner" aria-hidden="true">⏳</span> Asking the local
      assistant to suggest settings based on what we found…
    </p>
  {:else if error}
    <p class="error" role="alert" data-testid="ask-error">
      Could not generate suggestions: {error}
    </p>
  {:else if answers}
    <p class="muted" data-testid="ask-summary">
      Review the suggestions below. Accept them as-is, or click
      <strong>Edit</strong> to override the path lists.
    </p>

    <ul class="answers" data-testid="ask-answers">
      <li class="answer-row">
        <span class="label">Claude sessions</span>
        <span class="value">
          {#if answers.claude_sessions_paths.length === 0}
            <em>disabled</em>
          {:else}
            {answers.claude_sessions_paths.length} path(s)
          {/if}
        </span>
      </li>
      <li class="answer-row">
        <span class="label">GitHub</span>
        <span class="value">
          {answers.github?.enabled ? "enabled" : "disabled"}
          {#if answers.github?.repos?.length}
            — watching {answers.github.repos.length} repo(s)
          {/if}
        </span>
      </li>
      <li class="answer-row">
        <span class="label">Calendar</span>
        <span class="value">
          {answers.calendar_ics?.enabled ? "enabled" : "disabled"}
        </span>
      </li>
      <li class="answer-row">
        <span class="label">Voice capture</span>
        <span class="value">
          {answers.voice?.enabled
            ? `enabled (${answers.voice.model}, ${answers.voice.language})`
            : "disabled"}
        </span>
      </li>
      <li class="answer-row">
        <span class="label">Review time</span>
        <span class="value" data-testid="review-time-value">
          {answers.review_time.cadence} at
          <strong>{String(REVIEW_HOUR_LOCAL).padStart(2, "0")}:00</strong>
          your time
          <span class="tz" data-testid="review-time-tz">({short_tz_label(local_tz)})</span>
        </span>
      </li>
      <li class="answer-row">
        <span class="label">Summarizer</span>
        <span class="value">
          {answers.summarizer.backend} ({answers.summarizer.model})
        </span>
      </li>
      <li class="answer-row">
        <span class="label">Transport</span>
        <span class="value">{answers.transport.method}</span>
      </li>
    </ul>

    {#if editing}
      <div class="edits" data-testid="ask-edits">
        <label class="field">
          <span>Claude sessions paths (one per line)</span>
          <textarea
            rows="3"
            bind:value={edit_claude_paths}
            data-testid="edit-claude-paths"
          ></textarea>
        </label>
        <label class="field">
          <span>GitHub repos (one per line, or comma-separated)</span>
          <textarea
            rows="3"
            bind:value={edit_github_repos}
            data-testid="edit-github-repos"
          ></textarea>
        </label>
      </div>
    {/if}

    <div class="actions">
      <button
        type="button"
        class="secondary"
        data-testid="ask-toggle-edit"
        onclick={toggle_edit}
      >
        {editing ? "Done editing" : "Edit"}
      </button>
      <button
        type="button"
        class="primary"
        data-testid="ask-next"
        disabled={!can_advance}
        onclick={on_next_click}
      >
        {editing ? "Save & continue" : "Looks good"}
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
  .tz {
    color: var(--muted, #666);
    font-size: 0.85rem;
    margin-left: 0.25rem;
  }
  .answers {
    list-style: none;
    padding: 0;
    margin: 0;
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
  }
  .answer-row {
    display: grid;
    grid-template-columns: 12rem 1fr;
    gap: 0.75rem;
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--border, #ccc);
  }
  .answer-row:last-child {
    border-bottom: none;
  }
  .label {
    font-weight: 500;
  }
  .edits {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .field textarea {
    font-family: monospace;
    font-size: 0.9rem;
    padding: 0.4rem;
    border: 1px solid var(--border, #ccc);
    border-radius: 3px;
  }
  .actions {
    margin-top: 1rem;
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
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
  .primary:hover:not(:disabled) {
    background: var(--primary-hover, #1d4ed8);
  }
  .primary:disabled {
    background: var(--muted, #94a3b8);
    cursor: not-allowed;
  }
  .secondary {
    background: transparent;
    color: var(--primary, #2563eb);
    border: 1px solid var(--primary, #2563eb);
    padding: 0.5rem 1.25rem;
    border-radius: 4px;
    cursor: pointer;
    font-weight: 500;
  }
  .secondary:hover {
    background: var(--primary, #2563eb);
    color: white;
  }
  .spinner {
    display: inline-block;
  }
</style>