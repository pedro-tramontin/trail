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
   *
   * Phase 7 §7.5 — when `is_demo` is true the "Re-run
   * onboarding" button is replaced with a disabled "Demo mode —
   * settings are read-only" placeholder. The user can't actually
   * run onboarding in demo mode (the bootstrap refuses demo when
   * a real config exists) and the rest of Settings is
   * placeholder anyway, so the disabled copy is honest.
   *
   * §17-5 — voice microphone permission row. Reads the current
   * OS-level mic permission via `check_mic_permission_cmd`,
   * shows the human-readable state ("granted" / "denied" /
   * "undetermined"), and surfaces a per-OS deep-link button
   * when state == "denied". The "Test microphone" button runs
   * `voice_start` for 2 s then `voice_stop` so the user can
   * sanity-check the round-trip from Settings.
   *
   * §X-5 / Phase 11 §11.3 — SSH-key settings panel. The
   * `<SshKeySettings />` mount below reads the typed
   * `KeyringHint` (the §11.1 enum) from the `keyring_hint`
   * Tauri command and renders one of 4 UI states (Empty /
   * PublicOnly / KeyPair / Unavailable) inside the platform-
   * neutral "OS credential store" wording. The §X-3 work
   * already rewrote any "keychain" copy to the platform-
   * neutral wording — this comment block is the durable
   * pointer back to that audit.
   */

  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import SshKeySettings from "./lib/SshKeySettings.svelte";

  interface Props {
    onreset?: () => void;
    is_demo?: boolean;
  }

  let { onreset = () => {}, is_demo = false }: Props = $props();

  let mic_permission: "granted" | "denied" | "undetermined" | undefined =
    $state(undefined);
  let mic_permission_url: string | undefined = $state(undefined);
  let test_in_progress = $state(false);

  onMount(() => {
    invoke<string>("check_mic_permission_cmd")
      .then((s) => {
        if (s === "granted" || s === "denied" || s === "undetermined") {
          mic_permission = s;
        }
      })
      .catch(() => {
        mic_permission = undefined;
      });
    invoke<string>("mic_permission_deep_link_url_cmd")
      .then((u) => {
        mic_permission_url = u;
      })
      .catch(() => {
        mic_permission_url = undefined;
      });
  });

  async function test_microphone(): Promise<void> {
    if (test_in_progress) return;
    test_in_progress = true;
    try {
      await invoke("voice_start");
      await new Promise((r) => setTimeout(r, 2000));
      await invoke("voice_stop");
    } catch {
      /* The IPC may fail if permission is denied; surface
         the existing denied-callout instead of erroring here. */
    } finally {
      test_in_progress = false;
    }
  }

  async function open_permission_settings(): Promise<void> {
    if (!mic_permission_url) return;
    const a = document.createElement("a");
    a.href = mic_permission_url;
    a.style.display = "none";
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
  }

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
  {#if is_demo}
    <button
      data-testid="demo-mode-readonly"
      type="button"
      disabled
    >
      Demo mode — settings are read-only
    </button>
  {:else}
    <button
      data-testid="rerun-onboarding"
      type="button"
      onclick={rerun_onboarding}
    >
      Re-run onboarding
    </button>
  {/if}

  <section class="voice-permission-row" data-testid="voice-permission-row">
    <h2>Voice microphone permission</h2>
    <p data-testid="voice-permission-state" class="state-{mic_permission ?? 'unknown'}">
      Permission: <strong>{mic_permission ?? "checking…"}</strong>
    </p>
    {#if mic_permission === "denied"}
      <button
        type="button"
        class="open-permission-settings"
        data-testid="settings-open-permission"
        onclick={open_permission_settings}
      >
        Open Privacy Settings
      </button>
    {/if}
    <button
      type="button"
      class="test-microphone"
      data-testid="settings-test-microphone"
      disabled={test_in_progress}
      onclick={test_microphone}
    >
      {test_in_progress ? "Recording…" : "Test microphone"}
    </button>
  </section>

  <!--
    §X-5 / Phase 11 §11.3 — SSH-key settings panel mount.

    The panel reads the typed `KeyringHint` from the
    `keyring_hint` Tauri command and renders one of 4 UI
    states (Empty / PublicOnly / KeyPair / Unavailable)
    inside the platform-neutral "OS credential store"
    wording. Self-contained — no props needed; the
    component owns its IPC calls + clipboard + error
    rendering. The `is_demo` gate is intentionally NOT
    passed: a demo-mode install should still let the user
    inspect (and re-generate) the SSH keypair, since the
    SSH push is the load-bearing action that the demo
    mode fakes with fixture data.
  -->
  <SshKeySettings />
</section>

<style>
  .voice-permission-row {
    margin-top: 1.5rem;
    padding: 0.9rem 1rem;
    border: 1px solid var(--border, #ddd);
    border-radius: 6px;
    background: var(--panel, #fafafa);
  }
  .voice-permission-row h2 {
    margin: 0 0 0.5rem 0;
    font-size: 1rem;
  }
  .state-denied strong {
    color: #c62828;
  }
  .state-granted strong {
    color: #2e7d32;
  }
  .open-permission-settings {
    display: inline-block;
    margin-right: 0.5rem;
    padding: 0.35rem 0.8rem;
    border: 1px solid #c62828;
    background: #fff;
    color: #c62828;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.85rem;
  }
  .test-microphone {
    display: inline-block;
    padding: 0.35rem 0.8rem;
    border: 1px solid #1976d2;
    background: #1976d2;
    color: #fff;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.85rem;
  }
  .test-microphone:disabled {
    opacity: 0.6;
    cursor: wait;
  }
</style>
