<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { writable, type Writable } from "svelte/store";
  import { onMount } from "svelte";
  import type {
    OnboardingAnswers,
    ScanReport,
    StepAskState,
    QuestionLogEntry,
  } from "./types";
  import { validate_remote_calendar_url } from "./types";

  /** §17-5 — microphone permission state for the wizard's
   *  denied-callout. Fetched on mount via the new
   *  `check_mic_permission_cmd` Tauri command. Undefined while
   *  the IPC is in flight (the callout is hidden during that
   *  window so we don't flash a "denied" message that the OS
   *  later overrules). */
  let mic_permission_state: "granted" | "denied" | "undetermined" | undefined =
    $state(undefined);

  /** §X-4 — calendar permission deep-link URL. The wizard's
   *  EventKit hint is now a per-OS button instead of a
   *  plain-text "System Settings → Privacy → Calendars" string.
   *  Resolved once on mount via the new
   *  `calendar_permission_deep_link_url` Tauri command. The
   *  three states are:
   *
   *   - `undefined` — IPC still in flight; the hint stays
   *     hidden (no flash of a wrong button).
   *   - `"unknown_de"` — Linux + the helper returned
   *     `CalendarPermissionDeepLinkError::UnknownDE` (the
   *     webview can't detect GNOME vs KDE vs other). The
   *     hint renders as a labeled "open manually" message.
   *   - `string` — the per-OS URL the user clicks. The
   *     hint renders as a button that opens the URL via
   *     a hidden anchor (same pattern as the mic
   *     permission denied callout).
   *
   *  Linux is the load-bearing case: macOS and Windows
   *  resolve immediately. On Linux the helper returns
   *  `UnknownDE` unless the frontend supplies a `de` arg
   *  (we don't have a reliable in-webview DE detector yet,
   *  so we always pass `null`). The frontend can be wired
   *  to a richer detector in a follow-up. */
  let calendar_permission_url: string | "unknown_de" | undefined =
    $state(undefined);

  onMount(() => {
    invoke<string>("check_mic_permission_cmd")
      .then((s) => {
        if (s === "granted" || s === "denied" || s === "undetermined") {
          mic_permission_state = s;
        }
      })
      .catch(() => {
        // Permission check is best-effort — if the IPC fails,
        // leave the state undefined and don't render the callout.
        mic_permission_state = undefined;
      });
    // §X-4 — resolve the per-OS calendar permission deep-link
    // URL once on mount. We pass `null` for the `de` argument
    // (the webview can't reliably detect GNOME vs KDE vs
    // other, so the helper returns `UnknownDE` on Linux and
    // the frontend renders the labeled "open manually"
    // fallback). macOS / Windows resolve to their per-OS
    // URL regardless of `de`. The IPC is best-effort: on
    // rejection we leave the state undefined and the hint
    // stays hidden rather than rendering a dead button.
    invoke<string>("calendar_permission_deep_link_url", { de: null })
      .then((url) => {
        calendar_permission_url = url;
      })
      .catch((err: unknown) => {
        // Tauri serialises the `CalendarPermissionDeepLinkError`
        // variant as a string. Branch on the `UnknownDE` /
        // `UnknownOS` prefix so the frontend can render the
        // labeled fallback. Anything else (e.g. IPC failure
        // before the command runs) leaves the state
        // undefined and the hint stays hidden.
        const msg = String(err);
        if (msg.includes("Linux DE not detected")) {
          calendar_permission_url = "unknown_de";
        } else {
          calendar_permission_url = undefined;
        }
      });
  });

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
   *
   * ## Hoisted state (PR #193)
   *
   * The editable local state (edit buffers + review-time
   * picker) is hoisted to the parent wizard via the `state`
   * prop. The `editing` toggle, the per-row textareas, and
   * the time picker mutate `hoisted.*` directly so their values
   * survive a Back navigation. LLM-fetched `answers` +
   * `loading` + `error` are NOT hoisted — re-running
   * `ask_onboarding_cmd` on remount is fast and idempotent.
   *
   * If `initial_answers` is non-null (the user has already
   * been through this step and clicked Next once, then went
   * Back), we skip the LLM call entirely and use the cached
   * answers.
   */

  let {
    scan = null,
    initial_answers = null,
    state: hoisted,
    on_next,
  }: {
    scan: ScanReport | null;
    initial_answers: OnboardingAnswers | null;
    state: Writable<StepAskState>;
    on_next: (answers: OnboardingAnswers) => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let answers: OnboardingAnswers | null = $state(initial_answers);
  // svelte-ignore state_referenced_locally
  let loading = $state(initial_answers === null);
  let error: string | null = $state(null);

  // The hoisted `state` prop is a Svelte writable store
  // containing the editable form values (edit buffers,
  // review-time picker, edit toggle). We use a store (not
  // a $state object) because Svelte 5's $state proxies are
  // not transparently deep-reactive across component
  // boundaries when the child reads `prop.field` directly —
  // the child's $derived + template only re-evaluate when
  // the *prop reference* changes, not when one of its
  // nested properties does. (Svelte 5.56 limitation; the
  // workaround is a writable store from svelte/store, which
  // IS transparently deep-reactive via the $store-name
  // auto-subscription syntax.) The form fields bind to
  // `$hoisted.X` and the template reads from `$hoisted.X`.
  let review_hour_local = $derived(
    parseInt($hoisted.review_hhmm_local.split(":")[0], 10) || 0,
  );

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
    // If the user is returning to this step (Back from
    // Install), `initial_answers` is non-null and we already
    // hydrated `answers` at mount. Skip the LLM call entirely.
    if (initial_answers !== null) return;
    loading = true;
    error = null;
    try {
      const result = await invoke<OnboardingAnswers>(
        "ask_onboarding_cmd",
        { scan },
      );
      // 2026-08-11 — pre-apply the wizard's default choices to
      // the LLM's answer so the summary view reflects the
      // post-default state on first render. Without this, the
      // row would show "disabled" (the LLM's `voice: null`)
      // even though the new default is "enabled with base.en".
      // The user feedback was unambiguous: "it would be nice
      // to have it enabled by default with the best settings
      // for it" — the visible row is the load-bearing signal.
      // The defaults applied here match the `fresh_state()` in
      // StepAsk.test.ts + the Onboarding.svelte `writable({})`
      // default.
      const with_defaults: OnboardingAnswers = {
        ...result,
        // Voice: only apply the default if the LLM left it
        // null (the common case). If the LLM explicitly
        // emitted a VoiceConfig (rare), respect it.
        voice: result.voice ?? {
          enabled: $hoisted.edit_voice_enabled,
          model: $hoisted.edit_voice_model,
          language: "en",
        },
        // Calendar: keep the LLM's answer as-is. The Ask
        // step's edit template will overwrite it on the
        // post-edit branch; the summary view reads
        // `result.calendar_ics` directly until then.
      };
      answers = with_defaults;
      // Seed the local edit buffers from the LLM answer.
      // These are the FIRST-time-seeds — only applied if the
      // hoisted buffers are still empty (i.e. fresh wizard
      // run, not a Back navigation that reuses the previous
      // edits).
      if ($hoisted.edit_claude_paths === "") {
        hoisted.update((s) => {
          s.edit_claude_paths = (result.claude_sessions_paths ?? []).join("\n");
          return s;
        });
      }
      if ($hoisted.edit_github_repos === "") {
        hoisted.update((s) => {
          s.edit_github_repos = (result.github?.repos ?? []).join("\n");
          return s;
        });
      }
      // 2026-08-11 — seed the new ICS-path and browser-history
      // buffers from the LLM's pre-fill. Same first-time-only
      // pattern as the github/claude rows above. The
      // calendar_ics.ics_paths field is the LLM's best-guess
      // path list (or empty); browser_history is the LLM's
      // browser-ID pre-fill (or empty).
      if ($hoisted.edit_ics_paths === "") {
        hoisted.update((s) => {
          s.edit_ics_paths = (result.calendar_ics?.ics_paths ?? []).join("\n");
          return s;
        });
      }
      if ($hoisted.edit_browser_history === "") {
        hoisted.update((s) => {
          s.edit_browser_history = (result.browser_history ?? []).join("\n");
          return s;
        });
      }
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
    }
  }

  function toggle_edit(): void {
    if (!$hoisted.editing && answers !== null) {
      // Entering Edit mode: seed the local buffers from the
      // LLM's answer (only first-time; Back-nav preserves edits).
      const a = answers;
      hoisted.update((s) => {
        s.edit_claude_paths = (a.claude_sessions_paths ?? []).join("\n");
        s.edit_github_repos = (a.github?.repos ?? []).join("\n");
        s.edit_ics_paths = (a.calendar_ics?.ics_paths ?? []).join("\n");
        s.edit_browser_history = (a.browser_history ?? []).join("\n");
        s.editing = !s.editing;
        return s;
      });
    } else if ($hoisted.editing) {
      // 2026-08-11 — the "Done editing" reflection fix. When the
      // user is in Edit mode and clicks "Done editing" (the
      // master toggle that switches the template back to the
      // summary view), we commit the in-flight edits to the
      // `answers` object so the summary branch reads the
      // post-edit state. The pre-PR bug was: edit + flip voice
      // toggle on + click "Done editing" → row continued
      // showing "disabled" because the summary branch
      // (`!answers.voice?.enabled`) was reading the LLM's
      // stale `voice: null`. The fix runs `build_edited_answers`
      // and stores the result back into `answers` (so the next
      // render of the summary branch sees the new state). It
      // does NOT call `on_next` — that still happens on the
      // "Save & continue" / "Looks good" button. The post-edit
      // state is preserved through the summary view AND the
      // step transition.
      const edited = build_edited_answers();
      hoisted.update((s) => {
        s.editing = !s.editing;
        return s;
      });
      answers = edited;
    } else {
      // No-op: editing is off and answers is null (the IPC
      // hasn't resolved yet). Stay in the loading state.
      hoisted.update((s) => {
        s.editing = !s.editing;
        return s;
      });
    }
  }

  function build_edited_answers(): OnboardingAnswers {
    if (!answers) {
      throw new Error("build_edited_answers called before answers loaded");
    }
    const claude_sessions_paths = $hoisted.edit_claude_paths
      .split(/\r?\n/)
      .map((s: string) => s.trim())
      .filter((s: string) => s.length > 0);
    const github_repos = $hoisted.edit_github_repos
      .split(/\r?\n/)
      .flatMap((line: string) => line.split(","))
      .map((s: string) => s.trim())
      .filter((s: string) => s.length > 0);
    // PR #216 — voice toggle. Synthesise a typed VoiceConfig when
    // the user enables the checkbox in Edit mode, otherwise
    // produce `null` to match the LLM's "voice disabled" shape
    // (see answers.rs:347-355). The `language` field stays "en"
    // because the codebase never exercises non-English in v1
    // and the on-disk Config doesn't persist it (config_writer.rs
    // only stores model + hotkey + transcriber).
    //
    // 2026-08-11 — the "Done editing" reflection fix: this is
    // called by `on_next_click` AFTER the user clicks Edit, flips
    // the toggle, and clicks Save & continue. The new
    // `answers.voice` must reflect the post-edit state in the
    // row's summary view, so the wizard doesn't display
    // "disabled" after the user just enabled it. The fix: when
    // `editing` is true, the `answers` object's voice is
    // ALWAYS replaced by the edit-state (not just when the LLM
    // set it to null). The pre-PR bug was: edit + flip on +
    // Save → `build_edited_answers` was bypassed when
    // `!editing`, and the row continued showing the LLM's
    // `voice: null` from `MOCK_ANSWERS`. The fix is in the
    // summary view's binding (the `else if !answers.voice?.enabled`
    // branch) and in this function's always-replace contract.
    const voice: OnboardingAnswers["voice"] = $hoisted.editing
      ? {
          enabled: $hoisted.edit_voice_enabled,
          model: $hoisted.edit_voice_model || "base.en",
          language: "en",
        }
      : answers.voice;
    // 2026-08-11 — calendar source edit. The radio binds to
    // `$hoisted.edit_calendar_source` ("event_kit" / "ics").
    // The shape we emit is `answers.calendar_ics`: we either
    // set `calendar_app_id: Some("event_kit")` (the EventKit
    // path) or `ics_paths: [path]` (the legacy `.ics` file
    // path). For the EventKit variant we leave `ics_paths`
    // empty (the collector reads directly from Calendar.app).
    // For the `.ics` variant we keep the existing path list
    // (the user can edit it via a separate text input below
    // the radio). When the user is NOT in Edit mode we
    // preserve the LLM's answer as-is.
    let calendar_ics: OnboardingAnswers["calendar_ics"] = answers.calendar_ics;
    if ($hoisted.editing) {
      if ($hoisted.edit_calendar_source === "event_kit") {
        calendar_ics = {
          enabled: true,
          ics_paths: [],
          calendar_app_id: "event_kit",
        };
      } else {
        // .ics path mode — read the textarea the user just
        // edited. Pre-PR bug: the Ask step had no input
        // element for the .ics path, so picking "Custom .ics
        // file" silently produced an empty `ics_paths` list.
        // The collector then wrote an empty calendar.json.
        // The textarea is rendered inside the calendar row
        // (conditional on `edit_calendar_source === "ics"`)
        // and binds to `$hoisted.edit_ics_paths`. We split
        // on newlines, trim, and drop empties — same shape
        // as `claude_sessions_paths` / `github_repos`.
        const ics_paths = $hoisted.edit_ics_paths
          .split(/\r?\n/)
          .map((s: string) => s.trim())
          .filter((s: string) => s.length > 0);
        calendar_ics = {
          enabled: ics_paths.length > 0,
          ics_paths,
          calendar_app_id: null,
        };
      }
    }
    // 2026-08-11 — browser-history picker. The user picks
    // one or more browsers via checkboxes (rendered in the
    // new "Browser history" row); the rendered list is
    // newline-separated IDs (`chrome`, `brave`, `firefox`,
    // `opera`, `safari`). Split + trim + drop empties,
    // matching the github/claude row pattern. Preserve the
    // LLM's pre-fill when the user is NOT in Edit mode (so
    // Back-nav doesn't clobber a previously-confirmed
    // choice).
    let browser_history: OnboardingAnswers["browser_history"] =
      answers.browser_history;
    if ($hoisted.editing) {
      browser_history = $hoisted.edit_browser_history
        .split(/\r?\n/)
        .map((s: string) => s.trim())
        .filter((s: string) => s.length > 0);
    }
    // ECD-5 — Layer 1 webcal/ICS URL subscription. The
    // user pastes one `.ics` URL per line in the new
    // "Calendar URL" row's textarea. Each line is run
    // through `validate_remote_calendar_url` (rejects
    // `http://`, `file://`, `mailto:`); invalid lines are
    // silently dropped (the schema's `Option<Vec<String>>`
    // can't carry per-line error state, and the user can
    // see the textarea in edit mode for self-correction).
    // The LLM is told to leave the field unset, so this
    // path only matters in Edit mode. When the user is NOT
    // in Edit mode we preserve the LLM's pre-fill (so
    // Back-nav doesn't clobber a previously-confirmed
    // list).
    let remote_calendar_urls: OnboardingAnswers["remote_calendar_urls"];
    if ($hoisted.editing) {
      remote_calendar_urls = $hoisted.edit_remote_calendar_urls
        .split(/\r?\n/)
        .map((s: string) => s.trim())
        .filter((s: string) => validate_remote_calendar_url(s));
    } else {
      remote_calendar_urls = answers.remote_calendar_urls;
    }
    return {
      ...answers,
      claude_sessions_paths,
      github: answers.github
        ? { ...answers.github, repos: github_repos }
        : answers.github,
      voice,
      calendar_ics,
      browser_history,
      remote_calendar_urls,
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
    const edited = $hoisted.editing ? build_edited_answers() : answers;
    const final = apply_local_review_time(edited);
    on_next(final);
  }

  /**
   * §X-4 — open the per-OS calendar permission settings
   * pane via a hidden anchor click. The
   * `calendar_permission_url` state is guaranteed to be a
   * `string` (not `"unknown_de"` and not `undefined`) by the
   * surrounding `{#if calendar_permission_url === ...}` /
   * `{:else if ...}` / `{:else}` ladder — the button is only
   * rendered in the "URL is known" branch. We pull the
   * value into a typed local so TypeScript's narrowing
   * survives the closure boundary.
   */
  function open_calendar_permission_settings(): void {
    const url: string = String(calendar_permission_url);
    const a = document.createElement("a");
    a.href = url;
    a.style.display = "none";
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
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
          {#if $hoisted.editing}
            <textarea
              rows="2"
              class="inline-edit"
              bind:value={$hoisted.edit_claude_paths}
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
          {#if $hoisted.editing}
            <textarea
              rows="2"
              class="inline-edit"
              bind:value={$hoisted.edit_github_repos}
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
          {#if $hoisted.editing}
            <!--
              2026-08-11 — Calendar source radio. The two
              options correspond to the two `CalendarSource`
              variants in `src-tauri/src/config.rs`. The radio
              binds to `$hoisted.edit_calendar_source`; the
              `build_edited_answers` commit reads the post-edit
              value. The `.ics` file path picker below shows
              only when the user picks "Custom .ics file" so the
              summary isn't cluttered when EventKit is selected
              (the file picker would be unused).
            -->
            <div class="calendar-source-radio">
              <label>
                <input
                  type="radio"
                  bind:group={$hoisted.edit_calendar_source}
                  value="event_kit"
                  data-testid="calendar-source-event-kit"
                  aria-label="Calendar source: Calendar.app (EventKit)"
                />
                Calendar.app (EventKit)
              </label>
              <label>
                <input
                  type="radio"
                  bind:group={$hoisted.edit_calendar_source}
                  value="ics"
                  data-testid="calendar-source-ics"
                  aria-label="Calendar source: Custom .ics file"
                />
                Custom .ics file
              </label>
            </div>
            <span class="hint">
              {#if $hoisted.edit_calendar_source === "event_kit"}
                {#if calendar_permission_url === undefined}
                  <!--
                    IPC still in flight (or rejected with an
                    unhandled error). Don't render anything
                    yet so we don't flash the wrong control
                    when the helper resolves a few ms later.
                  -->
                {:else if calendar_permission_url === "unknown_de"}
                  <!--
                    Linux + DE not detected. The helper
                    returned
                    `CalendarPermissionDeepLinkError::UnknownDE`
                    (we can't reliably detect GNOME vs KDE
                    vs other from inside a webview). The
                    labeled message tells the user where to
                    go without dangling a dead button.
                  -->
                  EventKit needs your permission the first time
                  you start a capture. Your desktop environment
                  wasn't detected — open Settings → Privacy →
                  Calendar manually.
                {:else}
                  <!--
                    The per-OS URL is known. The button opens
                    it via a hidden anchor (same pattern as
                    the mic permission denied callout).
                    `tauri-plugin-opener` isn't wired in this
                    build; the per-OS schemes
                    (`x-apple.systempreferences:…`,
                    `gnome-control-center …`,
                    `ms-settings:…`) all work via a plain
                    anchor click in the system browser
                    handler.
                  -->
                  EventKit needs your permission the first time
                  you start a capture.
                  <button
                    type="button"
                    class="open-permission-settings"
                    data-testid="open-calendar-permission-settings"
                    onclick={open_calendar_permission_settings}
                  >
                    Open Calendar Settings
                  </button>
                {/if}
              {:else}
                Provide an .ics export from Calendar.app: File → Export →
                uncheck "Events" only if you also export Tasks separately.
                Enter the absolute path below.
              {/if}
            </span>
            <!--
              2026-08-11 — .ics path input. The textarea is
              rendered ONLY when the user picks "Custom .ics
              file" (the radio's value is "ics"). The
              previous behaviour had no input here at all —
              the user could pick the radio but couldn't
              enter the path, so `build_edited_answers`
              produced an empty `ics_paths` list. The
              collector then wrote an empty calendar.json.
              The textarea mirrors the `claude_sessions`
              row's pattern: one path per line, trim,
              drop empties. Pre-populated from
              `answers.calendar_ics.ics_paths` when
              entering Edit mode (see `toggle_edit`). On
              macOS the user typically points this at
              `~/Downloads/calendar.ics` after exporting
              from Calendar.app; on Linux it's wherever
              they synced the .ics file.
            -->
            {#if $hoisted.editing && $hoisted.edit_calendar_source === "ics"}
              <textarea
                rows="2"
                class="inline-edit"
                bind:value={$hoisted.edit_ics_paths}
                data-testid="edit-ics-paths"
                aria-label=".ics file paths (one per line)"
                placeholder="/Users/you/Downloads/calendar.ics"
              ></textarea>
              <span class="hint">One absolute path per line</span>
            {/if}
          {:else if !answers.calendar_ics?.enabled}
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
            {#if answers.calendar_ics?.calendar_app_id === "event_kit"}
              <span class="value-text" data-testid="calendar-source-summary">
                — Calendar.app
              </span>
            {:else if answers.calendar_ics?.ics_paths?.length}
              <span class="value-text" data-testid="calendar-source-summary">
                — {answers.calendar_ics.ics_paths.length} .ics path(s)
              </span>
            {/if}
          {/if}
        </span>
      </li>

      <!--
        ECD-5 (Layer 1 webcal/ICS URL subscription) — Calendar
        URL row. Sits between the Calendar source row and the
        Browser-history row so the Ask step's data-source rows
        mirror the order the supervisor enumerates them in
        `CollectorLaptopConfig`. The user pastes one or more
        `.ics` URLs (one per line); each line is validated
        against the `validate_remote_calendar_url` helper
        (rejects `http://`, `file://`, `mailto:`, and
        malformed URLs). Invalid lines are silently dropped
        when the user clicks Save & continue — the wizard
        can't carry per-line error state in the
        `Option<Vec<String>>` schema, so the textarea is the
        user's self-correction surface. The privacy hint
        matches the rest of the wizard ("fetched daily from
        your laptop" — Trail never proxies the URL through
        the VPS). The LLM is told to leave the field unset, so
        the pre-edit summary shows "disabled" with a `?`
        tooltip; entering Edit mode reveals the textarea +
        paste hint.
      -->
      <li class="answer-row" data-testid="row-remote-calendar-urls">
        <span class="label">Calendar URL (.ics)</span>
        <span class="value">
          {#if $hoisted.editing}
            <textarea
              rows="3"
              class="inline-edit"
              bind:value={$hoisted.edit_remote_calendar_urls}
              data-testid="edit-remote-calendar-urls"
              aria-label="Remote .ics URLs (one per line, https:// or webcal://)"
              placeholder="https://calendar.google.com/calendar/ical/.../basic.ics"
            ></textarea>
            <span class="hint">
              One URL per line. <code>https://</code> or
              <code>webcal://</code> only — local files and
              cleartext <code>http://</code> URLs are rejected.
              This URL is fetched daily from your laptop. Trail
              does not send telemetry.
            </span>
          {:else if !answers.remote_calendar_urls?.length}
            <em>disabled</em>
            <button
              type="button"
              class="why"
              data-testid="why-remote-calendar-urls"
              aria-label="Why is Calendar URL disabled?"
              title={disabled_reason("remote_calendar_urls", answers.question_log)}
            >?</button>
          {:else}
            enabled
            <span
              class="value-text"
              data-testid="remote-calendar-urls-summary"
            >
              — {answers.remote_calendar_urls.length} URL(s)
            </span>
          {/if}
        </span>
      </li>

      <!--
        2026-08-11 — Browser history row. Sits between the
        Calendar URL row and the Voice capture row so the Ask
        step's data-source rows mirror the scanner's
        `candidates` order (chrome_history / brave_history /
        firefox_history / opera_history / safari_history).
        The scanner now reports five browser-history probes;
        the Ask step lets the user pick which ones to
        enable via five checkboxes. The actual data
        collector that reads the SQLite / `places.sqlite` /
        `History.db` files is built in a follow-up PR; for
        now, the picker is captured in
        `answers.browser_history` (a `string[]` of browser
        IDs) and Phase C no-ops on it.
      -->
      <li class="answer-row" data-testid="row-browser-history">
        <span class="label">Browser history</span>
        <span class="value">
          {#if $hoisted.editing}
            <!--
              Five checkboxes, one per scanner probe. The
              renderer mirrors the "checked" state into the
              newline-separated `edit_browser_history`
              buffer so `build_edited_answers` can split +
              trim + drop empties (same shape as the github
              row's `edit_github_repos`). The bind:value is
              updated via an `on:change` handler instead of
              `bind:group` because `bind:group` would
              clobber `edit_browser_history` with a single
              ID, not a newline list. We hand-roll the
              sync to preserve the buffer's shape.
            -->
            <div class="browser-history-picker" data-testid="browser-history-picker">
              {#each ["chrome", "brave", "firefox", "opera", "safari"] as browser (browser)}
                <label class="browser-history-checkbox">
                  <input
                    type="checkbox"
                    value={browser}
                    checked={$hoisted.edit_browser_history
                      .split(/\r?\n/)
                      .map((s: string) => s.trim())
                      .includes(browser)}
                    onchange={(e: Event) => {
                      const target = e.currentTarget as HTMLInputElement;
                      const current = $hoisted.edit_browser_history
                        .split(/\r?\n/)
                        .map((s: string) => s.trim())
                        .filter((s: string) => s.length > 0);
                      const next = target.checked
                        ? [...new Set([...current, browser])]
                        : current.filter((b: string) => b !== browser);
                      hoisted.update((s) => {
                        s.edit_browser_history = next.join("\n");
                        return s;
                      });
                    }}
                    data-testid={`browser-history-${browser}`}
                    aria-label={`Enable ${browser} history`}
                  />
                  {browser}
                </label>
              {/each}
            </div>
            <span class="hint">
              The Trail history collector reads each browser's local history
              database (read-only copy — never modifies the source). The
              collector is shipped in a follow-up release; for now this
              picker is captured but not yet consumed.
            </span>
          {:else if !answers.browser_history?.length}
            <em>disabled</em>
            <button
              type="button"
              class="why"
              data-testid="why-browser_history"
              aria-label="Why is Browser history disabled?"
              title={disabled_reason("browser_history", answers.question_log)}
            >?</button>
          {:else}
            enabled
            <span class="value-text" data-testid="browser-history-summary">
              — {answers.browser_history.join(", ")}
            </span>
          {/if}
        </span>
      </li>

      <li class="answer-row" data-testid="row-voice">
        <span class="label">Voice capture</span>
        <span class="value">
          {#if $hoisted.editing}
            <!--
              Edit-mode toggle. Mirrors the github row's
              pattern: a single inline control (checkbox +
              model picker) that replaces the summary text.
              The default model matches config_writer.rs:197
              so flipping the checkbox without picking a model
              produces a config identical to a hand-edit.
            -->
            <label class="voice-toggle">
              <input
                type="checkbox"
                bind:checked={$hoisted.edit_voice_enabled}
                data-testid="voice-toggle"
                aria-label="Enable voice capture"
              />
              Enable voice capture
            </label>
            <span class="voice-model-row">
              <label for="voice-model">Model:</label>
              <select
                id="voice-model"
                bind:value={$hoisted.edit_voice_model}
                data-testid="voice-model"
                aria-label="Whisper model size"
              >
                <option value="tiny.en">tiny.en</option>
                <option value="base.en">base.en</option>
                <option value="small.en">small.en</option>
              </select>
            </span>
            <span class="hint">
              Microphone permission is requested the first time
              you start a capture (System Settings → Privacy →
              Microphone).
            </span>
          {:else if !answers.voice?.enabled}
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

      {#if mic_permission_state === "denied"}
        <li
          class="answer-row permission-denied-callout"
          data-testid="mic-permission-denied-callout"
          aria-live="polite"
        >
          <span class="label">Microphone permission</span>
          <span class="value">
            <strong>denied</strong>. Open your system's privacy settings to
            grant Trail access:
            <button
              type="button"
              class="open-permission-settings"
              data-testid="open-permission-settings"
              onclick={async () => {
                try {
                  const url = await invoke<string>(
                    "mic_permission_deep_link_url_cmd",
                  );
                  // Open the per-OS URL via a hidden anchor.
                  // `tauri-plugin-opener` isn't wired in this
                  // build, but the per-OS schemes (pavucontrol:,
                  // ms-settings:privacy-microphone, the macOS
                  // Apple-preferences URL) all work via a plain
                  // anchor click in the system browser handler.
                  const a = document.createElement("a");
                  a.href = url;
                  a.style.display = "none";
                  document.body.appendChild(a);
                  a.click();
                  document.body.removeChild(a);
                } catch {
                  /* ignore — the user can still grant permission
                     via System Settings manually */
                }
              }}
            >
              Open Privacy Settings
            </button>
          </span>
        </li>
      {/if}

      <li class="answer-row" data-testid="row-review-time">
        <span class="label">Review time</span>
        <span class="value review-time">
          {#if $hoisted.editing}
            <input
              type="time"
              class="inline-edit time-input"
              bind:value={$hoisted.review_hhmm_local}
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
              <strong data-testid="review-time-hour">{$hoisted.review_hhmm_local}</strong>
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
        {$hoisted.editing ? "Done editing" : "Edit"}
      </button>
      <button
        type="button"
        class="primary"
        data-testid="ask-next"
        disabled={!can_advance}
        onclick={on_next_click}
      >
        {$hoisted.editing ? "Save & continue" : "Looks good"}
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
  /** §17-5 — red-bordered denied-callout that appears below
   *  the voice row when the OS reports mic permission denied.
   *  Uses a 2 px solid red border + light pink fill so the
   *  issue is impossible to miss in the wizard flow. */
  .permission-denied-callout {
    border: 2px solid #c62828;
    border-radius: 6px;
    padding: 0.6rem 0.9rem;
    margin: 0.5rem 0;
    background: #ffebee;
    color: #4a1414;
  }
  .permission-denied-callout .label {
    font-weight: 600;
  }
  .open-permission-settings {
    margin-left: 0.5rem;
    padding: 0.2rem 0.6rem;
    border: 1px solid #c62828;
    background: #fff;
    color: #c62828;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.85rem;
  }
  .open-permission-settings:hover {
    background: #c62828;
    color: #fff;
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
  /**
   * 2026-08-11 — Calendar source radio layout. Two stacked
   * rows inside a flex column, with the hint text wrapping
   * beneath. Mirrors the `.voice-toggle` pattern above for
   * visual consistency with the Voice row.
   */
  .calendar-source-radio {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .calendar-source-radio label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    cursor: pointer;
  }
  .spinner {
    display: inline-block;
  }
</style>