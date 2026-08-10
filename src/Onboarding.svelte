<script lang="ts">
  import StepWelcome from "./lib/onboarding/StepWelcome.svelte";
  import StepScan from "./lib/onboarding/StepScan.svelte";
  import StepAsk from "./lib/onboarding/StepAsk.svelte";
  import StepTransport from "./lib/onboarding/StepTransport.svelte";
  import StepInstall from "./lib/onboarding/StepInstall.svelte";
  import StepFinish from "./lib/onboarding/StepFinish.svelte";
  import { writable, type Writable } from "svelte/store";
  import type {
    OnboardingAnswers,
    ScanReport,
    InstallOption,
    StepAskState,
    StepTransportState,
  } from "./lib/onboarding/types";

  /**
   * Phase 6 §6.4 — Onboarding wizard root.
   *
   * Drives the Phase A → B → C → D flow:
   *   0. StepWelcome
   *   1. StepScan        (scan_laptop_cmd → ScanReport)
   *   2. StepAsk         (ask_onboarding_cmd(scan) → OnboardingAnswers)
   *   3. StepTransport   (host/user/port + generate_ssh_key)
   *   4. StepInstall     (auto / show_script / skip)
   *   5. StepFinish      (write_onboarding_config + transition)
   *
   * Step state lives in the parent (`current_step` + the typed
   * values from each phase). Steps communicate back to the
   * parent via callback props (`on_next` / `on_complete`) — the
   * Svelte 5 idiomatic pattern, fully type-safe under
   * `strict: true` tsconfig.
   *
   * Steps share parent state via prop drilling — simpler than
   * the Svelte context API for a 6-step wizard, and keeps
   * each step independently testable.
   *
   * ## State-preservation rationale (PR #193)
   *
   * Steps 2 (Ask) and 3 (Transport) hold editable local state
   * (time picker, path lists, VPS details, key path). When the
   * user navigates Back from step 4 (Install) to step 2 (Ask),
   * Svelte's `{#if}` block remounts the step component, which
   * discards every `$state` declaration. The user loses any
   * edits they made.
   *
   * Fix: hoist the editable state into the parent wizard
   * (`step_ask_state`, `step_transport_state` below) and pass
   * it down as a single object prop. The child mutates the
   * object directly; Svelte 5's runes keep the parent's
   * reactive graph live so the next mount reads the persisted
   * values. LLM-fetch state (`loading`, `error`, `answers`)
   * stays in the child because the LLM call is fast and
   * idempotent — re-fetching on remount is fine.
   */

  let current_step = $state(0);
  let scan_report = $state<ScanReport | null>(null);
  let onboarding_answers = $state<OnboardingAnswers | null>(null);
  let install_choice = $state<InstallOption>("auto");
  // True once the user reaches StepInstall — by which point
  // StepTransport's `generate_ssh_key` has resolved and the
  // keychain holds the key. Read by StepFinish so the
  // config-writer can pick the `PublicKey` auth variant vs
  // the `Password` fallback (see
  // src-tauri/src/onboarding/config_writer.rs).
  let ssh_key_generated = $state(false);

  /** Per-step editable state. Hoisted to the wizard root so it
   *  survives step unmount on Back navigation. The child step
   *  mutates the object directly; Svelte 5 runes propagate
   *  the writes to the parent's reactive graph. */
  // Step 2 (Ask) — local edits the user typed in the answer
  // rows + the review-time picker. The LLM-fetched `answers`
  // object is NOT hoisted (re-running ask_onboarding_cmd on
  // remount is fast and idempotent). We use a writable store
  // for the same reason as step_transport_state — see that
  // block's comment for the Svelte 5.56 deep-reactivity
  // rationale.
  const step_ask_state: Writable<StepAskState> = writable({
    editing: false,
    edit_claude_paths: "",
    edit_github_repos: "",
    review_hhmm_local: "18:00",
    // PR #216 — voice-capture toggle lives in Edit mode.
    // `false` keeps the pre-PR behavior (LLM-disabled → write
    // voice=None). The default model matches the fallback in
    // config_writer.rs so flipping the toggle produces a config
    // entry that's identical to a hand-edited one.
    edit_voice_enabled: false,
    edit_voice_model: "base.en",
  });

  // Step 3 (Transport) — VPS connection details + key
  // material + test-connection transient state. Hoisted so
  // the user's typed values (and the "key already in
  // keychain" choice) persist when they navigate back from
  // step 4 (Install).
  //
  // We use a Svelte writable store here (not a $state object)
  // because Svelte 5's $state proxies are not transparently
  // deep-reactive across component boundaries when the child
  // reads `prop.field` directly — the child's $derived +
  // template only re-evaluate when the *prop reference* changes,
  // not when one of its nested properties does. (This is a
  // Svelte 5.56 limitation; the workaround is a writable store
  // from svelte/store, which IS transparently deep-reactive via
  // the $store-name auto-subscription syntax.) The store value
  // is the StepTransportState object; the child's form fields
  // read it via $state.host, $state.user, etc.
  const step_transport_state: Writable<StepTransportState> = writable({
    host: "",
    user: "",
    port: 22,
    ssh_key_path: null,
    ssh_key_source: null,
    generating: false,
    key_error: null,
    test_state: "idle",
    test_error: null,
  });

  /** Emitted when the wizard finishes writing the config. The
   * callback may be async (Phase 9 §9.3 — `App.svelte` awaits
   * the `start_collectors` IPC before flipping `config_exists`),
   * so the return type is `void | Promise<void>` to accept
   * both sync and async handlers. The wizard itself doesn't
   * await the return value — it just fires the callback after
   * `write_onboarding_config` resolves. */
  let { oncomplete }: {
    oncomplete?: (config_path: string) => void | Promise<void>;
  } = $props();

  function advance(): void {
    if (current_step < 5) current_step += 1;
  }

  function back(): void {
    if (current_step > 0) current_step -= 1;
  }

  function handle_step_0_next(): void {
    advance();
  }

  function handle_step_1_next(report: ScanReport): void {
    scan_report = report;
    advance();
  }

  function handle_step_2_next(answers: OnboardingAnswers): void {
    onboarding_answers = answers;
    advance();
  }

  function handle_step_3_next(): void {
    // StepTransport sets `ssh_key_generated` indirectly — if
    // the user reached here, the keychain has a key (or the
    // user manually advanced by force-firing the keygen).
    // For the v1 wizard, we mark it as done on entry to this
    // step's "Next" handler.
    ssh_key_generated = true;
    advance();
  }

  function handle_step_4_next(choice: InstallOption): void {
    install_choice = choice;
    advance();
  }

  function handle_step_5_complete(config_path: string): void {
    oncomplete?.(config_path);
  }
</script>

<div class="wizard" data-testid="onboarding-wizard" data-step={current_step}>
  <header class="wizard-header">
    <h1>Set up Trail</h1>
    <p class="step-indicator" data-testid="step-indicator">
      Step {current_step + 1} of 6
    </p>
  </header>

  <div class="step-container" data-testid="step-container">
    {#if current_step === 0}
      <StepWelcome on_next={handle_step_0_next} />
    {:else if current_step === 1}
      <StepScan on_next={handle_step_1_next} />
    {:else if current_step === 2}
      <StepAsk
        scan={scan_report}
        initial_answers={onboarding_answers}
        state={step_ask_state}
        on_next={handle_step_2_next}
      />
    {:else if current_step === 3}
      <StepTransport
        state={step_transport_state}
        on_next={handle_step_3_next}
      />
    {:else if current_step === 4}
      <StepInstall on_next={handle_step_4_next} />
    {:else if current_step === 5}
      <StepFinish
        answers={onboarding_answers}
        ssh_key_generated={ssh_key_generated}
        on_complete={handle_step_5_complete}
      />
    {/if}
  </div>

  {#if current_step > 0 && current_step < 5}
    <nav class="wizard-nav" data-testid="wizard-nav">
      <button
        type="button"
        class="back"
        data-testid="wizard-back"
        onclick={back}
      >
        ← Back
      </button>
    </nav>
  {/if}
</div>

<style>
  /**
   * Wizard layout: fixed viewport so the Tauri window doesn't
   * resize between steps (which was clipping content on shorter
   * steps after navigating from a taller one). The header and
   * nav stay at the top/bottom edges; the step body is the only
   * scroll region. `100dvh` (dynamic viewport height) handles
   * mobile URL-bar collapse; `min-height: 100vh` falls back on
   * browsers that don't support `dvh`.
   *
   * Card-style box: a faint shadow + slightly thicker top/bottom
   * borders so the wizard reads as a distinct card against the
   * page background on every step (including short ones like
   * StepWelcome where the wizard used to "blend into the page").
   */
  .wizard {
    height: 100vh;
    height: 100dvh;
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    width: 640px;
    max-width: 100%;
    margin: 0 auto;
    border: 1px solid var(--border, #c4c4c4);
    border-radius: 6px;
    background: var(--bg, #fff);
    color: var(--fg, #111);
    overflow: hidden;
    box-shadow: 0 2px 16px rgba(0, 0, 0, 0.08);
  }
  /**
   * Card body — the wizard's three regions (header, step body,
   * nav) each get an explicit `width: 100%` so the wizard's
   * outer 640px track is what sizes everything inside. Without
   * this, a region could grow to fit its longest child
   * (e.g. a long URL in the nav) and push the wizard wider
   * than 640px on that step, then snap back on the next step.
   * Paired with `min-width: 0`, flex children honor overflow
   * rules and stay within the parent's bounded track.
   */
  .wizard-header,
  .wizard-nav,
  .step-container {
    width: 100%;
    min-width: 0;
    box-sizing: border-box;
  }
  .wizard-header {
    flex-shrink: 0;
    padding: 1rem 1.5rem;
    border-bottom: 1px solid var(--border, #ccc);
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .wizard-header h1 {
    margin: 0;
    font-size: 1.25rem;
  }
  .step-indicator {
    color: var(--muted, #666);
    font-size: 0.85rem;
    margin: 0;
  }
  /**
   * The step body fills the remaining height and scrolls
   * internally when content overflows. `min-height: 0` is
   * required on flex children for `overflow-y: auto` to take
   * effect — without it, the child grows past the container
   * and the parent grows too, defeating the purpose of the
   * fixed-height wizard.
   *
   * `scrollbar-gutter: stable` reserves the scrollbar track
   * space even when content fits in the visible area, so the
   * inner column width does NOT shift between steps based on
   * whether the inner scrollbar is rendered or not. This is
   * what keeps the wizard width visually identical across
   * steps (e.g. StepWelcome doesn't show a scrollbar, StepScan
   * or StepAsk can).
   */
  .step-container {
    flex: 1 1 auto;
    min-height: 0;
    min-width: 0;
    overflow-y: auto;
    overflow-x: hidden;
    scrollbar-gutter: stable;
  }
  /**
   * Bottom-of-step controls (StepScan's auto-advance footer,
   * StepAsk's Looks-good button row) should stay visible
   * even when the step's content is long enough to scroll.
   * `position: sticky` keeps them pinned to the bottom of
   * the visible scroll region without leaving the flow.
   */
  .step-container :global(footer.sticky-actions),
  .step-container :global(div.auto-advance),
  .step-container :global(div.actions) {
    position: sticky;
    bottom: 0;
    background: var(--bg, #fff);
    /* A subtle gradient masks content scrolling under the
     * sticky footer. Without it the footer's border collides
     * with overflowing list rows visually. */
    box-shadow: 0 -8px 8px -8px rgba(0, 0, 0, 0.08);
  }
  .wizard-nav {
    flex-shrink: 0;
    padding: 0.75rem 1.5rem;
    border-top: 1px solid var(--border, #ccc);
    display: flex;
    justify-content: flex-start;
  }
  .back {
    background: transparent;
    color: var(--muted, #666);
    border: none;
    cursor: pointer;
    padding: 0.4rem 0.75rem;
    font-size: 0.9rem;
  }
  .back:hover {
    color: var(--fg, #111);
  }
</style>
