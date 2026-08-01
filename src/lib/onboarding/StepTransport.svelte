<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  /**
   * Step 4 — Transport configuration.
   *
   * Collects the VPS connection details (host, user, port) and
   * generates an ed25519 SSH keypair (stored in the macOS
   * Keychain via the `generate_ssh_key` Tauri command from
   * item 1-2). The returned key path is surfaced in the UI
   * so the user can confirm where the public key went.
   *
   * Validation:
   *   - host: non-empty
   *   - user: non-empty
   *   - port: 1-65535
   * Next is disabled until all three are valid AND a key has
   * been generated (the wizard shouldn't advance without a key
   * — Phase C's `write_onboarding_config` reads the key path
   * for the SSH transport auth variant).
   *
   * On Next, calls the `on_next` prop (no detail).
   */

  let { on_next }: { on_next: () => void } = $props();

  let host = $state("");
  let user = $state("");
  let port = $state(22);
  let ssh_key_path = $state<string | null>(null);
  let generating = $state(false);
  let error = $state<string | null>(null);

  const host_valid = $derived(host.trim().length > 0);
  const user_valid = $derived(user.trim().length > 0);
  const port_valid = $derived(
    Number.isInteger(port) && port >= 1 && port <= 65535,
  );
  const can_advance = $derived(
    host_valid && user_valid && port_valid && ssh_key_path !== null,
  );

  async function generate_key(): Promise<void> {
    generating = true;
    error = null;
    try {
      const path = await invoke<string>("generate_ssh_key");
      ssh_key_path = path;
    } catch (err) {
      error = String(err);
    } finally {
      generating = false;
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
        bind:value={host}
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
        bind:value={user}
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
        bind:value={port}
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
      {#if ssh_key_path}
        <p class="muted" data-testid="transport-key-path">
          ✅ Generated and stored in Keychain — <code>{ssh_key_path}</code>
        </p>
      {:else}
        <button
          type="button"
          class="secondary"
          data-testid="transport-generate-key"
          disabled={generating}
          onclick={() => {
            void generate_key();
          }}
        >
          {generating ? "Generating…" : "Generate SSH key"}
        </button>
      {/if}
      {#if error}
        <p class="hint hint-error" data-testid="transport-key-error">
          {error}
        </p>
      {/if}
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
</style>
