<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import Greet from "./lib/Greet.svelte";
  import Onboarding from "./Onboarding.svelte";

  /**
   * App shell. Gates rendering on the existence of
   * `~/.trail/config.json` (Phase 6 §6.4):
   *
   *   - if `config_exists` returns `true`, mount the regular
   *     shell (today: the Greet placeholder; will be replaced
   *     by the Logs / Settings UI in later phases).
   *   - if `false`, mount the onboarding wizard. The wizard
   *     writes the config and emits `oncomplete` — we flip
   *     `config_exists` to `true` and re-render the shell.
   *
   * The `loaded` gate prevents an "Onboarding → shell → Onboarding"
   * flicker during the initial `config_exists` probe.
   */

  let config_exists = $state(false);
  let loaded = $state(false);

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
  }

  function handle_onboarding_complete(_path: string): void {
    config_exists = true;
  }

  onMount(() => {
    void probe();
  });
</script>

<main>
  {#if !loaded}
    <p data-testid="app-loading">Loading…</p>
  {:else if !config_exists}
    <Onboarding oncomplete={handle_onboarding_complete} />
  {:else}
    <h1>Trail</h1>
    <Greet />
  {/if}
</main>
