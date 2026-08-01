<script lang="ts">
  /**
   * Step 1 — Welcome screen. Pure intro; no IPC, no inputs.
   * The Next button is always enabled (the only way to leave
   * step 1 is to acknowledge the intro by clicking "Get started").
   *
   * Calls the `on_next` prop when the user clicks "Get started".
   * The parent `Onboarding.svelte` advances `current_step`
   * from 0 → 1 (StepScan).
   */

  let { on_next }: { on_next: () => void } = $props();
</script>

<section class="step" data-testid="step-welcome">
  <h2>Welcome to Trail</h2>
  <p>
    Trail watches your day across the apps you already use, then writes a
    private summary to your own VPS at the end of the day.
  </p>
  <p>
    This setup takes about a minute. We'll scan your laptop for the data
    sources Trail can use, ask a few short questions, and wire up the
    connection to your VPS. Your data never leaves your machines.
  </p>
  <p class="muted">
    You can re-run this setup later from <strong>Settings → Re-run onboarding</strong>.
  </p>

  <div class="actions">
    <button
      type="button"
      class="primary"
      data-testid="welcome-next"
      onclick={on_next}
    >
      Get started
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
  .muted {
    color: var(--muted, #666);
    font-size: 0.9rem;
  }
</style>
