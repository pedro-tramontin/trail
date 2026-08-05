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
  // They live in their respective answer rows so toggling `editing`
  // only swaps the value slot between summary text and editable
  // input — never the row's outer dimensions.
  let edit_claude_paths = $state("");
  let edit_github_repos = $state("");

  /*
   * Local review time, as an "HH:MM" string (24h) bound to a
   * <input type="time">. Default "18:00" to match the existing
   * baseline. We split into hour before converting to UTC.
   *
   * Note: the Rust scheduler parses `cfg.review_time` (in the
   * final config) as `%H:%M`, but the LLM answer object stores
   * `hour_utc` as an integer hour (see
   * src-tauri/src/onboarding/answers.rs). We preserve that
   * integer-only contract here (minute granularity is shown
   * to the user but only the hour is propagated back to
   * `answers.review_time.hour_utc`, which is what
   * config_writer.rs currently reads — see that file's
   * `answers_to_config` for the cadence propagation path).
   */
  let review_hhmm_local = $state("18:00");
  let review_hour_local = $derived(parseInt(review_hhmm_local.split(":")[0], 10) || 0);

  // Browser-detected IANA timezone (e.g. "America/Sao_Paulo").
  const local_tz: string =
    typeof Intl !== "undefined"
      ? Intl.DateTimeFormat().resolvedOptions().timeZone
      : "UTC";

  function short_tz_label(tz: string): string {
    if (tz === "UTC") return "UTC";
    const parts = tz.split("/");
    return parts[parts.length - 1].replace(/_/g, " ");
  }

  /**
   * Translate the picked local hour to UTC for storage.
   * `new Date().getTimezoneOffset()` returns minutes-from-local-
   * TO-UTC (sign-flipped per ECMA: UTC-5 → 300), so a user in
   * UTC-5 picking local hour 18 should see UTC stored as
   * hour 23. Used by `apply_local_review_time` to write
   * `answers.review_time.hour_utc` before the parent's on_next
   * callback — the Rust scheduler parses that field as `%H:%M`
   * in UTC.
   *
   * Note: we deliberately do NOT render the converted UTC in
   * the user-visible hint (per Anti-pattern D of
   * wizard-ux-patterns — don't tell the user "18:00 UTC").
   * The user picks their LOCAL time, the wizard translates
   * internally on Next, and the only context the user sees
   * is the city label (`Your time (Berlin)`).
   */
  function local_hour_to_utc(local_hour: number): number {
    const offset_minutes = new Date().getTimezoneOffset();
    const offset_hours = offset_minutes / 60;
    return ((local_hour - offset_hours) + 24) % 24;
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
      // Default the local review-time picker to 18:00. The user
      // can pick a different hour+minute via the inline picker
      // when the master Edit toggle is on.
      review_hhmm_local = "18:00";
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
      <li class="answer-row" data-testid="row-claude-sessions">
        <span class="label">Claude sessions</span>
        <span class="value">
          {#if editing}
            <textarea
              rows="2"
              class="inline-edit"
              bind:value={edit_claude_paths}
              data-testid="edit-claude-paths"
              aria-label="Claude sessions paths (one per line)"
            ></textarea>
            <span class="hint">One path per line</span>
          {:else if answers.claude_sessions_paths.length === 0}
            <em>disabled</em>
            <button
              type="button"
              class="why"
              data-testid="why-claude_sessions"
              aria-label="Why is Claude sessions disabled?"
              title={disabled_reason("claude_sessions", answers.question_log)}
            >?</button>
          {:else}
            <span class="value-text" data-testid="claude-sessions-summary">
              {answers.claude_sessions_paths.length} path(s)
            </span>
          {/if}
        </span>
      </li>

      <li class="answer-row" data-testid="row-github">
        <span class="label">GitHub</span>
        <span class="value">
          {#if editing}
            <textarea
              rows="2"
              class="inline-edit"
              bind:value={edit_github_repos}
              data-testid="edit-github-repos"
              aria-label="GitHub repos (one per line, or comma-separated)"
            ></textarea>
            <span class="hint">
              One repo per line (or comma-separated)
            </span>
          {:else if !answers.github?.enabled}
            <em>disabled</em>
            <button
              type="button"
              class="why"
              data-testid="why-github"
              aria-label="Why is GitHub disabled?"
              title={disabled_reason("github", answers.question_log)}
            >?</button>
          {:else}
            <span class="value-text" data-testid="github-summary">
              enabled
              {#if answers.github?.repos?.length}
                — watching {answers.github.repos.length} repo(s)
              {/if}
            </span>
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

      <li class="answer-row" data-testid="row-review-time">
        <span class="label">Review time</span>
        <span class="value review-time">
          {#if editing}
            <input
              type="time"
              class="inline-edit time-input"
              bind:value={review_hhmm_local}
              data-testid="review-time-input"
              aria-label="Review time (your local time, HH:MM)"
            />
            <!--
              Edit-mode hint. Per Pattern 2 (Store UTC, Show
              Local Time) and Anti-pattern D of
              wizard-ux-patterns (don't tell the user "Stored
              as 20:00 UTC"), we deliberately omit the UTC
              conversion. The user picks their local time; we
              translate it to UTC internally on save. The only
              context the user needs is "which timezone am I
              picking for?" — the city label answers that.
              Just dropping the "Stored as HH:MM UTC" line and
              showing the tz label alone is enough.
            -->
            <span class="hint">
              Your time
              <span class="tz" data-testid="review-time-tz">({short_tz_label(local_tz)})</span>
            </span>
          {:else}
            <span class="value-text review-time-summary" data-testid="review-time-value">
              {answers.review_time.cadence} at
              <strong data-testid="review-time-hour">{review_hhmm_local}</strong>
              your time
              <span class="tz" data-testid="review-time-tz">({short_tz_label(local_tz)})</span>
            </span>
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

    <!--
      No separate `.edits` block — textareas live in their
      respective answer rows above so flipping `editing` swaps
      summary → input inside the same row, keeping the row's
      outer dimensions stable.
    -->

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
  /**
   * `width: 100%` + `min-width: 0` + `box-sizing: border-box`
   * so the step body sits inside the wizard's fixed 640px track
   * without overflowing horizontally on any inner content.
   * Mirrors the same fix in StepScan — see that file for the
   * full rationale (long unbreakable tokens otherwise push
   * the wizard wider than the parent on certain steps).
   */
  .step {
    padding: 1.5rem;
    width: 100%;
    min-width: 0;
    box-sizing: border-box;
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
  /**
   * Inline value-slot layout. The right column of every
   * answer-row hosts either read-only summary text or the
   * inline editable input — never both at once. Both modes
   * live inside the same column track so toggling `editing`
   * only swaps content, never row dimensions (this is what
   * stops the wizard from "resizing when I click Edit").
   */
  .value {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    min-width: 0;
  }
  .value-text {
    line-height: 1.4;
    word-break: break-word;
  }
  /**
   * Inline edit controls fill the value column without
   * overflowing. `<textarea>` gets `resize: vertical` so the
   * user can grow a row if they paste a long path, but the
   * initial size (`rows="2"`) is consistent across users.
   */
  .inline-edit {
    width: 100%;
    box-sizing: border-box;
    font-family: monospace;
    font-size: 0.9rem;
    padding: 0.35rem 0.45rem;
    border: 1px solid var(--border, #c4c4c4);
    border-radius: 3px;
    background: var(--bg, #fff);
    color: var(--fg, #111);
    min-width: 0;
  }
  .inline-edit:focus {
    outline: 2px solid var(--primary, #2563eb);
    outline-offset: 1px;
  }
  /**
   * The time picker uses `<input type="time">` which on
   * some browsers expands to ~10rem; cap its width so it
   * doesn't push the column track. The TZ label / "Stored
   * as NN:MM UTC" hint sit underneath.
   */
  .time-input {
    max-width: 9rem;
  }
  .hint {
    font-size: 0.8rem;
    color: var(--muted, #666);
    line-height: 1.3;
  }
  /**
   * Review-time row uses a flex layout in its value slot so
   * summary text and the inline picker both flow naturally.
   * `.review-time` itself is just a thin wrapper to keep
   * the existing grid-column targeting.
   */
  .review-time {
    gap: 0.25rem;
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
  /*
   * `.link` class removed: the old review-time edit button
   * (which used this style) is gone. The master "Edit" /
   * "Done editing" toggle in `.actions` uses `.secondary`
   * instead. No replacement for `.link` is needed.
   */
  .spinner {
    display: inline-block;
  }
</style>