<script lang="ts">
  import type { InstallOption } from "./types";

  /**
   * Step 5 — Phase D install selector.
   *
   * Three mutually-exclusive options for what to do with the
   * VPS install script after the wizard finishes writing
   * `~/.trail/config.json`:
   *
   *   - "auto"        — run the install on the VPS via the
   *                      `install_vps_collector` Tauri command
   *                      (item 6-6, will ship next).
   *   - "show_script" — open the rendered script in a viewer
   *                      so the user can review it before running.
   *   - "skip"        — don't install now; the user can run
   *                      `make install` later from the project.
   *
   * The wizard just records the choice. Phase D's actual
   * install path lives in item 6-6; this step is the UI hook
   * into it.
   */

  let {
    on_next,
  }: {
    on_next: (choice: InstallOption) => void;
  } = $props();

  let choice = $state<InstallOption>("auto");

  const can_advance = $derived(true); // always valid — a radio is always picked

  function emit(): void {
    on_next(choice);
  }
</script>

<section class="step" data-testid="step-install">
  <h2>Install the collector on your VPS?</h2>
  <p class="muted">
    Trail's collector runs on the VPS and writes your daily summary to the
    plan file. Pick how you want to set it up.
  </p>

  <fieldset class="options" data-testid="install-options">
    <legend class="visually-hidden">Install option</legend>
    <label class="option" data-testid="install-option-auto">
      <input
        type="radio"
        name="install-option"
        value="auto"
        checked={choice === "auto"}
        onchange={() => (choice = "auto")}
      />
      <span class="option-label">
        <strong>Auto</strong>
        <span class="option-desc">Run the install over SSH now.</span>
      </span>
    </label>
    <label class="option" data-testid="install-option-show-script">
      <input
        type="radio"
        name="install-option"
        value="show_script"
        checked={choice === "show_script"}
        onchange={() => (choice = "show_script")}
      />
      <span class="option-label">
        <strong>Show me the script</strong>
        <span class="option-desc">
          Render the install plan and let me run it manually.
        </span>
      </span>
    </label>
    <label class="option" data-testid="install-option-skip">
      <input
        type="radio"
        name="install-option"
        value="skip"
        checked={choice === "skip"}
        onchange={() => (choice = "skip")}
      />
      <span class="option-label">
        <strong>Skip for now</strong>
        <span class="option-desc">I'll install the collector later.</span>
      </span>
    </label>
  </fieldset>

  <div class="actions">
    <button
      type="button"
      class="primary"
      data-testid="install-next"
      disabled={!can_advance}
      onclick={emit}
    >
      Next
    </button>
  </div>
</section>

<style>
  .step {
    padding: 1.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .muted {
    color: var(--muted, #666);
    font-size: 0.9rem;
  }
  .options {
    border: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    border: 0;
    padding: 0;
    white-space: nowrap;
    clip-path: inset(100%);
    clip: rect(0 0 0 0);
    overflow: hidden;
  }
  .option {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.75rem 1rem;
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
    cursor: pointer;
  }
  .option:hover {
    background: var(--hover, #f5f5f5);
  }
  .option:has(input:checked) {
    border-color: var(--primary, #2563eb);
    background: #eff6ff;
  }
  .option-label {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  .option-desc {
    color: var(--muted, #666);
    font-size: 0.85rem;
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
  .primary:hover:not(:disabled) {
    background: var(--primary-hover, #1d4ed8);
  }
  .primary:disabled {
    background: var(--muted, #94a3b8);
    cursor: not-allowed;
  }
</style>
