<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type {
    OnboardingAnswers,
    ScanReport,
    QuestionLogEntry,
  } from "./types";

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
   * Two UX features layered on top of the basic checklist:
   *
   *   1. Editable review time — clicking the review-time row
   *      opens a 0–23 hour picker. The local hour the user
   *      picks is translated back to UTC by
   *      `apply_local_review_time` before being stored, so
   *      the scheduler fires at the user's local time (see
   *      the file's earlier commit message for the full
   *      timezone handling rationale).
   *   2. "Why disabled?" tooltips — every row that shows
   *      "disabled" has a `?` icon next to the value. Hover
   *      (or tap on touch) reveals the LLM's reasoning from
   *      `question_log`, matched by `evidence_refs` against
   *      the field name. If the LLM didn't log a reason, a
   *      generic fallback is shown so the tooltip is never
   *      empty.
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
   *      `Intl.DateTimeFormat().resolvedOptions().timeZone`.
   *   2. Reads the offset at the current moment
   *      (`new Date().getTimezoneOffset()`).
   *   3. Defaults the local hour to 18. The user can override
   *      via the picker on the review-time row.
   *   4. Before calling `on_next`, translates the local hour
   *      back to UTC and writes that into
   *      `answers.review_time.hour_utc`.
   *
   * DST is handled imperfectly (we read the offset at the
   * current moment, not at the next fire time). Documented
   * in earlier commit; the user can refine in Settings later.
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

  // Local review hour (0–23). Defaults to 18. The user can
  // override via the picker that appears when editing === true.
  let review_hour_local = $state(18);
  let editing_review_hour = $state(false);

  // Browser-detected IANA timezone (e.g. "America/Sao_Paulo").
  const local_tz: string =
    typeof Intl !== "undefined"
      ? Intl.DateTimeFormat().resolvedOptions().timeZone
      : "UTC";

  function local_hour_to_utc(local_hour: number): number {
    const offset_minutes = new Date().getTimezoneOffset();
    const offset_hours = offset_minutes / 60;
    return ((local_hour - offset_hours) + 24) % 24;
  }

  function short_tz_label(tz: string): string {
    if (tz === "UTC") return "UTC";
    const parts = tz.split("/");
    return parts[parts.length - 1].replace(/_/g, " ");
  }

  /**
   * Find the question_log entry that explains why a given
   * field is in its current state (enabled or disabled).
   * Matches by `evidence_refs` containing the field name.
   * Returns null if no entry exists.
   */
  function find_reason(
    field_id: string,
    log: QuestionLogEntry[],
  ): QuestionLogEntry | null {
    return (
      log.find((e) => e.evidence_refs.includes(field_id)) ?? null
    );
  }

  /**
   * Build the user-facing "why" string for a disabled field.
   * Tries question_log first, falls back to a generic message.
   */
  function disabled_reason(
    field_id: string,
    log: QuestionLogEntry[],
  ): string {
    const entry = find_reason(field_id, log);
    if (entry) return `${entry.question} → ${entry.reasoning}`;
    return `This field is disabled. The LLM didn't log a reason for "${field_id}".`;
  }

  const can_advance = $derived(answers !== null && !loading);

  async function run_ask(): Promise<void> {
    if (!scan) return;
    loading = true;
    error = null;
    try {
      const result = await invoke<OnboardingAnswers>(
        "ask_onboarding_cmd",
        { scan },
      );
      answers = result;
      // Seed the local edit buffers from the LLM answer.
      edit_claude_paths = (result.claude_sessions_paths ?? []).join("\n");
      edit_github_repos = (result.github?.repos ?? []).join("\n");
      // Default the local review-hour picker to 18. The user
      // can pick a different hour.
      review_hour_local = 18;
      editing_review_hour = false;
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  function toggle_edit(): void {
    if (!editing && answers) {
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
   * Translate the user's local review-hour back to UTC and
   * write it into `answers.review_time.hour_utc`. The LLM's
   * `cadence` ("evening"/"morning"/"weekly") is preserved.
   */
  function apply_local_review_time(
    base: OnboardingAnswers,
  ): OnboardingAnswers {
    return {
      ...base,
      review_time: {
        ...base.review_time,
        hour_utc: local_hour_to_utc(review_hour_local),
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
            <button
              type="button"
              class="why"
              data-testid="why-claude_sessions"
              aria-label="Why is Claude sessions disabled?"
              title={disabled_reason("claude_sessions", answers.question_log)}
            >?</button>
          {:else}
            {answers.claude_sessions_paths.length} path(s)
          {/if}
        </span>
      </li>

      <li class="answer-row">
        <span class="label">GitHub</span>
        <span class="value">
          {#if !answers.github?.enabled}
            <em>disabled</em>
            <button
              type="button"
              class="why"
              data-testid="why-github"
              aria-label="Why is GitHub disabled?"
              title={disabled_reason("github", answers.question_log)}
            >?</button>
          {:else}
            enabled
            {#if answers.github?.repos?.length}
              — watching {answers.github.repos.length} repo(s)
            {/if}
          {/if}
        </span>
      </li>

      <li class="answer-row">
        <span class="label">Calendar</span>
        <span class="value">
          {#if !answers.calendar_ics?.enabled}
            <em>disabled</em>
            <button
              type="button"
              class="why"
              data-testid="why-calendar"
              aria-label="Why is Calendar disabled?"
              title={disabled_reason("calendar", answers.question_log)}
            >?</button>
          {:else}
            enabled
          {/if}
        </span>
      </li>

      <li class="answer-row">
        <span class="label">Voice capture</span>
        <span class="value">
          {#if !answers.voice?.enabled}
            <em>disabled</em>
            <button
              type="button"
              class="why"
              data-testid="why-voice"
              aria-label="Why is Voice capture disabled?"
              title={disabled_reason("voice", answers.question_log)}
            >?</button>
          {:else}
            enabled ({answers.voice.model}, {answers.voice.language})
          {/if}
        </span>
      </li>

      <li class="answer-row">
        <span class="label">Review time</span>
        <span class="value review-time" data-testid="review-time-value">
          <span class="review-time-summary">
            {answers.review_time.cadence} at
            <strong data-testid="review-time-hour">{String(review_hour_local).padStart(2, "0")}:00</strong>
            your time
            <span class="tz" data-testid="review-time-tz">({short_tz_label(local_tz)})</span>
          </span>
          <button
            type="button"
            class="link"
            data-testid="review-time-edit"
            onclick={() => (editing_review_hour = !editing_review_hour)}
          >
            {editing_review_hour ? "Done" : "Change time"}
          </button>
          {#if editing_review_hour}
            <div class="review-time-picker" data-testid="review-time-picker">
              <label class="picker-label" for="review-hour-input">
                Hour (0–23, your local time)
              </label>
              <input
                id="review-hour-input"
                type="number"
                min="0"
                max="23"
                step="1"
                bind:value={review_hour_local}
                data-testid="review-hour-input"
              />
              <span class="picker-note">
                Stored as <strong>{local_hour_to_utc(review_hour_local)}:00 UTC</strong>
              </span>
            </div>
          {/if}
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
  .why {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.1rem;
    height: 1.1rem;
    margin-left: 0.4rem;
    border-radius: 50%;
    border: 1px solid var(--muted, #888);
    background: transparent;
    color: var(--muted, #666);
    font-size: 0.75rem;
    font-weight: 600;
    cursor: help;
    padding: 0;
    line-height: 1;
  }
  .why:hover,
  .why:focus {
    background: var(--muted, #666);
    color: white;
    outline: none;
  }
  .review-time {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }
  .review-time-summary {
    flex: 1;
  }
  .review-time-picker {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    width: 100%;
    margin-top: 0.5rem;
    padding: 0.5rem;
    background: var(--bg-soft, #f6f8fa);
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
  }
  .picker-label {
    font-size: 0.85rem;
    color: var(--muted, #666);
  }
  .picker-note {
    font-size: 0.8rem;
    color: var(--muted, #666);
  }
  .review-time-picker input {
    width: 4rem;
    padding: 0.25rem 0.4rem;
    font-size: 0.95rem;
    border: 1px solid var(--border, #ccc);
    border-radius: 3px;
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
  .link {
    background: transparent;
    border: none;
    color: var(--primary, #2563eb);
    cursor: pointer;
    padding: 0.25rem 0.5rem;
    font-size: 0.85rem;
    font-weight: 500;
  }
  .link:hover {
    text-decoration: underline;
  }
  .spinner {
    display: inline-block;
  }
</style>