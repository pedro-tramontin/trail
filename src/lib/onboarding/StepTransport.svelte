<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { writable, type Writable } from "svelte/store";
  import type { StepTransportState } from "./types";

  /**
   * Step 4 — Transport configuration.
   *
   * Collects the VPS connection details (host, user, port) and
   * either generates a fresh ed25519 SSH keypair (stored in the
   * macOS Keychain via the `generate_ssh_key` Tauri command from
   * item 1-2) or attaches an existing key already in the
   * keychain. The returned key path is surfaced in the UI so
   * the user can confirm where the public key went.
   *
   * Validation:
   *   - host: non-empty
   *   - user: non-empty
   *   - port: 1-65535
   * Next is disabled until all three are valid AND a key has
   * been attached (the wizard shouldn't advance without a key
   * — Phase C's `write_onboarding_config` reads the key path
   * for the SSH transport auth variant).
   *
   * On Next, calls the `on_next` prop (no detail).
   *
   * ## Hoisted state (PR #193)
   *
   * All form state is hoisted to the parent wizard via the
   * `state` prop. The user's typed values (host / user / port),
   * the key path, and the test-connection transient state all
   * survive a Back navigation.
   *
   * ## "Use existing key" affordance (PR #193)
   *
   * Two paths to attach a key:
   *   1. **Generate** — clicks the original "Generate SSH key"
   *      button. Calls `generate_ssh_key`, which is idempotent
   *      (re-running returns the existing public key in the
   *      keychain). First-run path.
   *   2. **Use existing** — clicks "Use existing key in
   *      keychain". Calls `get_ssh_public_key`; if it returns
   *      `Some(pubkey)`, we set `ssh_key_path` to that value
   *      without generating a new key. If `None`, we surface
   *      a "no existing key found" error and the user can
   *      fall back to Generate.
   *
   * Both paths end with the same `ssh_key_path` value — a
   * public key in OpenSSH single-line form. The `ssh_key_source`
   * tag is cosmetic (for the UI hint about which path was
   * used); it doesn't affect the Next-button enable logic.
   *
   * ## "Test connection" button (PR #193)
   *
   * Calls the `test_ssh_connection` Tauri command (added in
   * this PR), which builds an `SshTransport` in-memory with
   * the (host, port, user) the user typed + publickey auth
   * against whatever key is in the keychain, then runs
   * `health_check()`. Result is shown next to the button:
   * a green ✅ "Connected" on success or a red error with
   * the message on failure. We do NOT advance or block Next
   * on the result — it's informational, so the user can still
   * advance with a misconfigured VPS if they want (e.g. to
   * write the config now and fix the transport later).
   */

  let {
    state,
    on_next,
  }: {
    state: Writable<StepTransportState>;
    on_next: () => void;
  } = $props();

  // The form fields bind to `$state.X` (Svelte's auto-store
  // subscription). The $store-name prefix auto-subscribes to
  // the writable store and re-renders the consumer when
  // `set()` is called — this is the Svelte 4 store pattern
  // that Svelte 5 still supports for cross-component
  // deep-reactive state.
  const host_valid = $derived($state.host.trim().length > 0);
  const user_valid = $derived($state.user.trim().length > 0);
  const port_valid = $derived(
    Number.isInteger($state.port) && $state.port >= 1 && $state.port <= 65535,
  );
  const can_advance = $derived(
    host_valid && user_valid && port_valid && $state.ssh_key_path !== null,
  );

  async function generate_key(): Promise<void> {
    state.update((s) => {
      s.generating = true;
      s.key_error = null;
      return s;
    });
    try {
      const path = await invoke<string>("generate_ssh_key");
      state.update((s) => {
        s.ssh_key_path = path;
        s.ssh_key_source = "generated";
        s.generating = false;
        return s;
      });
    } catch (err) {
      state.update((s) => {
        s.key_error = String(err);
        s.generating = false;
        return s;
      });
    }
  }

  /** Read the public key for the key already in the keychain.
   *  If found, adopt it as the wizard's `ssh_key_path`. If
   *  no key exists, surface an actionable error so the user
   *  can fall back to Generate. */
  async function use_existing_key(): Promise<void> {
    state.update((s) => {
      s.generating = true;
      s.key_error = null;
      return s;
    });
    try {
      const pub = await invoke<string | null>("get_ssh_public_key");
      state.update((s) => {
        if (pub === null || pub === "") {
          s.key_error =
            "No existing SSH key found in keychain. Click 'Generate SSH key' to create one.";
        } else {
          s.ssh_key_path = pub;
          s.ssh_key_source = "existing";
        }
        s.generating = false;
        return s;
      });
    } catch (err) {
      state.update((s) => {
        s.key_error = String(err);
        s.generating = false;
        return s;
      });
    }
  }

  async function test_connection(): Promise<void> {
    state.update((s) => {
      s.test_state = "testing";
      s.test_error = null;
      return s;
    });
    try {
      await invoke("test_ssh_connection", {
        host: $state.host,
        port: $state.port,
        user: $state.user,
      });
      state.update((s) => {
        s.test_state = "ok";
        return s;
      });
    } catch (err) {
      state.update((s) => {
        s.test_state = "error";
        s.test_error = String(err);
        return s;
      });
    }
  }
</script>

<section class="step" data-testid="step-transport">
  <h2>Where should Trail send your day?</h2>
  <p class="muted">
    Enter the VPS where the collector will install. Trail will generate an
    ed25519 SSH keypair so the laptop can push without a password.
  </p>

  <div class="form">
    <label class="field">
      <span class="label-text">VPS host</span>
      <input
        type="text"
        bind:value={$state.host}
        placeholder="vps.example.com"
        data-testid="transport-host"
      />
      {#if !host_valid}
        <span class="hint hint-error" data-testid="host-error">
          Host is required.
        </span>
      {/if}
    </label>

    <label class="field">
      <span class="label-text">SSH user</span>
      <input
        type="text"
        bind:value={$state.user}
        placeholder="pedro"
        data-testid="transport-user"
      />
      {#if !user_valid}
        <span class="hint hint-error" data-testid="user-error">
          User is required.
        </span>
      {/if}
    </label>

    <label class="field">
      <span class="label-text">SSH port</span>
      <input
        type="number"
        bind:value={$state.port}
        min="1"
        max="65535"
        data-testid="transport-port"
      />
      {#if !port_valid}
        <span class="hint hint-error" data-testid="port-error">
          Port must be between 1 and 65535.
        </span>
      {/if}
    </label>

    <div class="field">
      <span class="label-text">SSH key</span>
      {#if $state.ssh_key_path}
        <p class="muted" data-testid="transport-key-path">
          ✅ {$state.ssh_key_source === "existing"
            ? "Using existing key from"
            : "Generated and stored in"} Keychain — <code>{$state.ssh_key_path}</code>
        </p>
      {:else}
        <div class="key-actions">
          <button
            type="button"
            class="secondary"
            data-testid="transport-generate-key"
            disabled={$state.generating}
            onclick={() => {
              void generate_key();
            }}
          >
            {$state.generating ? "Working…" : "Generate SSH key"}
          </button>
          <button
            type="button"
            class="secondary"
            data-testid="transport-use-existing-key"
            disabled={$state.generating}
            onclick={() => {
              void use_existing_key();
            }}
          >
            Use existing key in keychain
          </button>
        </div>
      {/if}
      {#if $state.key_error}
        <p class="hint hint-error" data-testid="transport-key-error">
          {$state.key_error}
        </p>
      {/if}
    </div>

    <div class="field">
      <span class="label-text">Test connection</span>
      <div class="test-row">
        <button
          type="button"
          class="secondary"
          data-testid="transport-test-connection"
          disabled={!can_advance || $state.test_state === "testing"}
          onclick={() => {
            void test_connection();
          }}
        >
          {$state.test_state === "testing" ? "Testing…" : "Test connection"}
        </button>
        {#if $state.test_state === "ok"}
          <span class="test-result test-ok" data-testid="transport-test-ok">
            ✅ Connected
          </span>
        {:else if $state.test_state === "error"}
          <span
            class="test-result test-error"
            data-testid="transport-test-error"
          >
            ❌ {$state.test_error}
          </span>
        {/if}
      </div>
    </div>
  </div>

  <div class="actions">
    <button
      type="button"
      class="primary"
      data-testid="transport-next"
      disabled={!can_advance}
      onclick={on_next}
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
  .form {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .label-text {
    font-weight: 500;
  }
  .field input {
    padding: 0.4rem;
    border: 1px solid var(--border, #ccc);
    border-radius: 3px;
    font-size: 0.95rem;
  }
  .hint {
    font-size: 0.8rem;
  }
  .hint-error {
    color: var(--danger, #c00);
  }
  code {
    font-family: monospace;
    font-size: 0.85rem;
    background: #f1f5f9;
    padding: 0.1rem 0.3rem;
    border-radius: 2px;
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
  .secondary {
    background: transparent;
    color: var(--primary, #2563eb);
    border: 1px solid var(--primary, #2563eb);
    padding: 0.4rem 1rem;
    border-radius: 4px;
    cursor: pointer;
    font-weight: 500;
  }
  .secondary:hover:not(:disabled) {
    background: var(--primary, #2563eb);
    color: white;
  }
  .secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .key-actions {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .test-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .test-result {
    font-size: 0.85rem;
    font-family: monospace;
  }
  .test-ok {
    color: var(--ok, #15803d);
  }
  .test-error {
    color: var(--danger, #c00);
  }
</style>
