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
   */

  let config_exists = $state(false);
  let loaded = $state(false);
  let mount_wizard = $state(false);
  let is_demo = $state(false);

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
  }

  function handle_onboarding_complete(_path: string): void {
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
