<script lang="ts">
  import StepWelcome from "./lib/onboarding/StepWelcome.svelte";
  import StepScan from "./lib/onboarding/StepScan.svelte";
  import StepAsk from "./lib/onboarding/StepAsk.svelte";
  import StepTransport from "./lib/onboarding/StepTransport.svelte";
  import StepInstall from "./lib/onboarding/StepInstall.svelte";
  import StepFinish from "./lib/onboarding/StepFinish.svelte";
  import type {
    OnboardingAnswers,
    ScanReport,
    InstallOption,
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

  /** Emitted when the wizard finishes writing the config. */
  let { oncomplete }: { oncomplete?: (config_path: string) => void } =
    $props();

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
        on_next={handle_step_2_next}
      />
    {:else if current_step === 3}
      <StepTransport on_next={handle_step_3_next} />
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
  .wizard {
    max-width: 640px;
    margin: 0 auto;
    border: 1px solid var(--border, #ccc);
    border-radius: 6px;
    background: var(--bg, #fff);
    color: var(--fg, #111);
  }
  .wizard-header {
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
  .step-container {
    min-height: 200px;
  }
  .wizard-nav {
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
