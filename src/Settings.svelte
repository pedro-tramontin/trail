<script lang="ts">
  /**
   * Phase 6 §6.5 — Settings placeholder shell.
   *
   * The full Settings UI (collectors, voice config, schedule,
   * log retention, etc.) lands in a later phase. This component
   * exists today so the `App.svelte` tray-menu swap can mount a
   * real panel + so the "Re-run onboarding" action is reachable
   * from the menu-bar app.
   *
   * The button click:
   *   1. Asks the user to confirm via the native `confirm()`
   *      dialog (returns `true` on OK, `false` on Cancel).
   *   2. On OK, calls the `onreset` prop. The parent
   *      (`App.svelte`) handles it by calling
   *      `delete_config` + re-mounting the wizard.
   *
   * The component itself does NOT call `invoke("delete_config")`
   * — that keeps the IPC boundary inside `App.svelte` so the
   * "delete + re-mount" sequence is atomic at the parent level
   * and the wizard mount isn't racy.
   */

  interface Props {
    onreset?: () => void;
  }

  let { onreset = () => {} }: Props = $props();

  function rerun_onboarding(): void {
    if (confirm("This will reset your Trail config. Continue?")) {
      onreset();
    }
  }
</script>

<section data-testid="settings-shell">
  <h1>Trail Settings</h1>
  <p data-testid="settings-placeholder-note">
    The full Settings UI is coming in a later phase. For now,
    you can re-run the onboarding wizard if you'd like to add a
    new data source or change the SSH target.
  </p>
  <button
    data-testid="rerun-onboarding"
    type="button"
    onclick={rerun_onboarding}
  >
    Re-run onboarding
  </button>
</section>
