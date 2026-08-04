<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import Greet from "./lib/Greet.svelte";
  import Onboarding from "./Onboarding.svelte";
  import Settings from "./Settings.svelte";
  import DemoBanner from "./lib/DemoBanner.svelte";

  /**
   * App shell. Gates rendering on the existence of
   * `~/.trail/config.json` (Phase 6 §6.4):
   *
   *   - if `config_exists` returns `true`, mount the regular
   *     shell (today: the Settings placeholder; the Greet
   *     placeholder is kept available for future iterations).
   *   - if `false`, mount the onboarding wizard. The wizard
   *     writes the config and emits `oncomplete` — we flip
   *     `config_exists` to `true` and re-render the shell.
   *
   * The `loaded` gate prevents an "Onboarding → shell → Onboarding"
   * flicker during the initial `config_exists` probe.
   *
   * Phase 6 §6.5 — `reset_for_onboarding()` deletes the existing
   * config and flips the `mount_wizard` rune so the Onboarding
   * wizard re-mounts. The Settings.svelte placeholder's
   * "Re-run onboarding" button calls this via its `onreset` prop.
   * The `$state(true)` rune is what actually drives the swap
   * (the {#if} block re-keys on `mount_wizard`, so a true→false→true
   * cycle unmounts and remounts the wizard from scratch — clearing
   * any stale step state from a previous run).
   *
   * Phase 7 §7.5 — `is_demo` is the Svelte mirror of the Rust
   * `demo::DemoState.active` flag. The banner at the top of
   * every window reads this prop; the Settings shell also reads
   * it to render the read-only "Demo mode" placeholder instead
   * of the real "Re-run onboarding" button.
   *
   * Phase 9 §9.3 — `start_collectors_error` holds the error
   * message from the last failed `start_collectors` IPC. The
   * wizard's `StepFinish` reads it via the `oncomplete` payload
   * so a failed start (e.g. an SSH key the keychain can't
   * unlock, a port the firewall blocks) surfaces inline in the
   * wizard instead of being silently swallowed — the user can
   * then click "Back" and fix the offending step before
   * retrying. The `handle_onboarding_complete` callback awaits
   * the IPC; only on `Ok` does it flip `config_exists` to `true`
   * and unmount the wizard.
   *
   * The `onMount` cold-restart probe: if `config_exists` was
   * already `true` on the first probe (cold restart after the
   * wizard finished but the app crashed before the next start),
   * the orchestrator isn't running yet — the Tauri setup
   * closure only brings it up for the `Ready(_)` arm; the
   * `AwaitingOnboarding` arm defers the orchestrator to
   * `start_collectors` IPC. We re-invoke it here so the
   * orchestrator is alive after a restart too. The IPC is
   * idempotent on the Rust side (calling it twice just
   * re-spawns the scheduler task — see §9.1 D1 in state.md).
   */

  let config_exists = $state(false);
  let loaded = $state(false);
  let mount_wizard = $state(false);
  let is_demo = $state(false);
  let start_collectors_error = $state<string | null>(null);

  async function probe(): Promise<void> {
    try {
      config_exists = await invoke<boolean>("config_exists");
    } catch (err) {
      // On any error (e.g. running outside Tauri in vitest),
      // assume the regular shell. The wizard will re-mount
      // only when a real Tauri runtime reports `false`.
      console.error("config_exists probe failed", err);
      config_exists = true;
    } finally {
      loaded = true;
    }
    // Phase 7 §7.5 — probe demo state. `null` (Tauri side returns
    // `None` when the bootstrap didn't activate demo) means "not
    // in demo mode"; an object with `active: true` means the
    // banner should render + Settings should be read-only.
    try {
      const status = await invoke<{ active: boolean } | null>("demo_status");
      is_demo = status != null && status.active === true;
    } catch (err) {
      console.error("demo_status probe failed", err);
      is_demo = false;
    }
    // Phase 9 §9.3 — cold-restart probe. If the config is
    // already on disk, the Tauri setup closure's
    // `AwaitingOnboarding` arm didn't run (we're in the
    // `Ready` arm), so the orchestrator IS already up — we
    // don't need to re-invoke. But if we somehow landed in
    // `AwaitingOnboarding` AND the config exists (a race the
    // §9.3 integration test exercises — the wizard wrote the
    // config but the Svelte side never got the `Ready` signal
    // before the page reload), we need to bring the
    // orchestrator up ourselves.
    if (config_exists) {
      try {
        await invoke("start_collectors");
      } catch (err) {
        // Non-fatal — the orchestrator might already be up
        // (the Tauri side returns a "already managed" error
        // if a second start fires). Log and move on.
        console.warn("start_collectors probe failed (likely already up)", err);
      }
    }
  }

  /**
   * Phase 9 §9.3 — wizard completion callback. Awaits the
   * `start_collectors` IPC (which builds the orchestrator +
   * scheduler on the Rust side) BEFORE flipping
   * `config_exists` to `true`. On failure, stores the error
   * message in `start_collectors_error` so the wizard can
   * surface it inline; the user can then click "Back" and
   * retry the offending step rather than getting a blank
   * shell with no collectors running.
   */
  async function handle_onboarding_complete(_path: string): Promise<void> {
    start_collectors_error = null;
    try {
      await invoke("start_collectors");
    } catch (err) {
      // Render the error in the wizard so the user can retry.
      // Don't flip `config_exists` — the wizard needs the
      // orchestrator up before unmounting (otherwise the
      // collector settings page would 500 on first render).
      const msg = err instanceof Error ? err.message : String(err);
      console.error("start_collectors failed after wizard completion", err);
      start_collectors_error = msg;
      return;
    }
    config_exists = true;
    mount_wizard = false;
  }

  /**
   * Reset the existing config and re-mount the wizard.
   * Called by Settings.svelte's "Re-run onboarding" button.
   *
   * Step 1: call the Rust `delete_config` IPC command. This
   * removes `~/.trail/config.json` (idempotent: a missing
   * file is not an error). The trailing `config_exists`
   * re-probe is what guarantees the next `{#if !config_exists}`
   * branch fires — but the explicit `mount_wizard` flip is
   * the primary mechanism so the remount happens even if
   * the probe races.
   *
   * Step 2: flip `mount_wizard` to `true`. The {#if} block
   * re-keys on this rune, so the wizard unmounts and remounts
   * cleanly (clearing any step state from a prior run).
   *
   * The IPC call's error path is logged but does not block
   * the remount — the wizard will re-run `write_onboarding_config`
   * at the end of StepFinish, which will surface a fresh
   * failure if the underlying disk is read-only.
   */
  export async function reset_for_onboarding(): Promise<void> {
    try {
      await invoke("delete_config", {
        cmd: "/Users/test/.trail/config.json",
      });
    } catch (err) {
      console.error("delete_config failed", err);
    }
    config_exists = false;
    mount_wizard = true;
  }

  onMount(() => {
    void probe();
  });
</script>

<main>
  {#if !loaded}
    <p data-testid="app-loading">Loading…</p>
  {:else if mount_wizard || !config_exists}
    <DemoBanner is_demo={is_demo} />
    <Onboarding oncomplete={handle_onboarding_complete} />
  {:else}
    <DemoBanner is_demo={is_demo} />
    <h1>Trail</h1>
    <Settings onreset={reset_for_onboarding} {is_demo} />
    <Greet />
  {/if}
</main>

<style>
  /* Phase 9 §9.3 — onboarding window center-anchoring.
   *
   * The `<main>` element wraps both the wizard and the regular
   * Settings shell. The wizard card is 640px max-width, so on any
   * window wider than that there will be horizontal whitespace
   * that needs to be distributed evenly on both sides — done by
   * the body flex reset in index.html, which translates into
   * even left/right gutter for <main>.
   *
   * Vertical spacing: `width: 100%` + `display: flex` +
   * `align-items: center` + `justify-content: center` on the body
   * (set in index.html) centers <main> too. No additional flex
   * on <main> is needed for that — the body's flex handles it.
   *
   * We only style the wizard margin-top to make sure it doesn't
   * crowd the title (when the Settings branch is rendered, the
   * <h1>Trail</h1> needs ~24px above it). The wizard handles its
   * own internal padding via `.step { padding: 1.5rem }` so we
   * don't add more here.
   */
  main {
    display: flex;
    flex-direction: column;
    align-items: center;
    width: 100%;
  }
</style>
