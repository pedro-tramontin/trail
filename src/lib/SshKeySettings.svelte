<script lang="ts">
  /**
   * Phase 11 §11.3 — SSH-key settings panel.
   *
   * Mounted in the Settings shell
   * (`src/Settings.svelte`). Reads the typed `KeyringHint`
   * from the Rust side via the `keyring_hint` Tauri command
   * (defined in `src-tauri/src/commands.rs`) and renders one
   * of 4 UI states based on `hint.kind`:
   *
   * | `hint.kind`        | UI copy                                                       |
   * | ------------------ | ------------------------------------------------------------- |
   * | `empty`            | "No SSH key yet" + "Generate SSH key" button                  |
   * | `public_only`      | "Your public key is stored but the private key is missing — re-generate" recovery row |
   * | `key_pair`         | "Your SSH key is stored" + "Copy public key" + "Regenerate" buttons |
   * | `unavailable`      | "The OS credential store is unavailable (reason: X)" labeled fallback |
   *
   * The panel header shows the per-OS credential store name
   * (`credential_store_name` Tauri command, §X-3) so the user
   * sees the platform-specific label (Keychain / secret-service
   * / Credential Manager) instead of the generic "OS credential
   * store" wording. The body's wording is platform-neutral.
   *
   * The `keyring_hint` + `credential_store_name` calls are
   * fired in parallel on mount so the panel renders fast even
   * on hosts where the credential store is slow to probe
   * (Windows Credential Manager can take ~200ms on first
   * probe).
   */

  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import type { KeyringHint } from "$lib/api/keyring";

  /** `undefined` while the onMount `keyring_hint()` probe is
   *  in flight; populated to a discrete `KeyringHint` variant
   *  once the IPC resolves. The 4 UI states all key off this
   *  variable — `undefined` renders the placeholder "loading…"
   *  copy. */
  let hint: KeyringHint | undefined = $state(undefined);

  /** Per-OS user-facing label for the OS credential store
   *  (Keychain / secret-service / Credential Manager / generic
   *  fallback). Populated by the parallel `credential_store_name`
   *  IPC on mount. Defaults to the generic label so the header
   *  doesn't flicker while the IPC resolves. */
  let store_name: string = $state("OS credential store");

  /** `true` while the "Generate" / "Regenerate" click is in
   *  flight. Disables both buttons + shows a "Working…"
   *  placeholder so the user can't fire the action twice. */
  let generating = $state(false);

  /** Last user-visible error message (e.g. `generate_ssh_key`
   *  failed). Rendered below the panel body when set so the
   *  user sees the failure instead of silently no-op'ing. */
  let error_message: string | undefined = $state(undefined);

  /** The most recently fetched public key (used by the
   *  "Copy public key" button + the public-only recovery
   *  row). Populated alongside `hint` on mount; refreshed
   *  every time the user clicks "Generate" / "Regenerate". */
  let public_key: string | undefined = $state(undefined);

  /** "Copied!" transient indicator. Flipped to `true` for 1.5s
   *  after the user clicks "Copy public key", then back to
   *  `false`. Lets the button label give feedback without
   *  spawning a toast system. */
  let just_copied = $state(false);

  onMount(() => {
    // Fire the three IPCs in parallel. We don't `await` them
    // in sequence — the panel can render the loading
    // placeholder while they're in flight, and the per-OS
    // label / KeyringHint / public key are all independent.
    refresh_hint();
    invoke<string>("credential_store_name")
      .then((name) => {
        store_name = name;
      })
      .catch(() => {
        // Leave the default — the panel header still renders
        // the generic label rather than crashing on a missing
        // IPC binding.
      });
    invoke<string | null>("get_ssh_public_key")
      .then((key) => {
        if (typeof key === "string" && key.length > 0) {
          public_key = key;
        }
      })
      .catch(() => {
        // No public key yet, or the keychain probe failed —
        // the `hint` state already carries the failure mode
        // (Unavailable / PublicOnly), so this catch is a
        // silent no-op.
      });
  });

  /** Re-fetch the KeyringHint + public key. Called on mount
   *  + after every "Generate" / "Regenerate" click. */
  async function refresh_hint(): Promise<void> {
    try {
      const next = await invoke<KeyringHint>("keyring_hint");
      hint = next;
      // Re-fetch the public key — after a regeneration the
      // previous `public_key` is stale.
      try {
        const key = await invoke<string | null>("get_ssh_public_key");
        if (typeof key === "string" && key.length > 0) {
          public_key = key;
        }
      } catch {
        /* keep the previous `public_key` — the next refresh
           will overwrite it. */
      }
    } catch (e) {
      // `keyring_hint` flattens `Err` paths into
      // `Unavailable { reason }`, so a true IPC failure here
      // means the Rust side isn't loaded or the command name
      // is wrong. Surface as the labeled fallback.
      hint = {
        kind: "unavailable",
        reason: typeof e === "string" ? e : String(e),
      };
    }
  }

  /** Fire the `generate_ssh_key` IPC. Used by both the
   *  "Generate SSH key" button (Empty state) and the
   *  "Regenerate" button (KeyPair state) — the Rust side is
   *  idempotent (returns the existing keypair if one is
   *  already in the keychain). */
  async function generate_or_regenerate(): Promise<void> {
    if (generating) return;
    generating = true;
    error_message = undefined;
    try {
      const key = await invoke<string>("generate_ssh_key");
      public_key = key;
      await refresh_hint();
    } catch (e) {
      error_message = typeof e === "string" ? e : String(e);
    } finally {
      generating = false;
    }
  }

  /** Copy the public key to the clipboard. The button is
   *  only shown in the KeyPair state (the spec lists it as
   *  a KeyPair-only affordance), but the function is safe
   *  to call any time `public_key` is set — the test suite
   *  exercises both branches. */
  async function copy_public_key(): Promise<void> {
    if (!public_key) return;
    try {
      await navigator.clipboard.writeText(public_key);
      just_copied = true;
      // Reset after 1.5s so the user gets feedback but the
      // label doesn't stick.
      setTimeout(() => {
        just_copied = false;
      }, 1500);
    } catch (e) {
      error_message =
        typeof e === "string"
          ? `clipboard write failed: ${e}`
          : `clipboard write failed: ${String(e)}`;
    }
  }
</script>

<section class="ssh-key-settings" data-testid="ssh-key-settings">
  <h2 data-testid="ssh-key-settings-header">
    SSH key ({store_name})
  </h2>

  {#if hint === undefined}
    <p data-testid="ssh-key-settings-loading" class="ssh-key-settings-loading">
      Checking the OS credential store…
    </p>
  {:else if hint.kind === "empty"}
    <div data-testid="ssh-key-settings-empty" class="ssh-key-settings-state ssh-key-settings-empty">
      <p>
        <strong>No SSH key yet.</strong>
        Generate one to enable remote SSH push of day summaries.
        The keypair will be stored in your <em>{store_name}</em>.
      </p>
      <button
        type="button"
        data-testid="ssh-key-settings-generate"
        disabled={generating}
        onclick={generate_or_regenerate}
      >
        {generating ? "Generating…" : "Generate SSH key"}
      </button>
    </div>
  {:else if hint.kind === "public_only"}
    <div data-testid="ssh-key-settings-public-only" class="ssh-key-settings-state ssh-key-settings-public-only">
      <p>
        Your public key is stored in your <em>{store_name}</em>,
        but the private key is missing. The SSH push is
        broken until you re-generate the keypair.
      </p>
      {#if public_key}
        <details>
          <summary>Show stored public key</summary>
          <pre data-testid="ssh-key-settings-public-key">{public_key}</pre>
        </details>
      {/if}
      <button
        type="button"
        data-testid="ssh-key-settings-regenerate"
        disabled={generating}
        onclick={generate_or_regenerate}
      >
        {generating ? "Re-generating…" : "Re-generate SSH key"}
      </button>
    </div>
  {:else if hint.kind === "key_pair"}
    <div data-testid="ssh-key-settings-key-pair" class="ssh-key-settings-state ssh-key-settings-key-pair">
      <p>
        <strong>Your SSH key is stored</strong> in your
        <em>{store_name}</em>. SSH push to your VPS will use
        the keypair automatically — no further action needed.
      </p>
      {#if public_key}
        <details>
          <summary>Show public key</summary>
          <pre data-testid="ssh-key-settings-public-key">{public_key}</pre>
        </details>
      {/if}
      <div class="ssh-key-settings-actions">
        <button
          type="button"
          data-testid="ssh-key-settings-copy"
          disabled={!public_key}
          onclick={copy_public_key}
        >
          {just_copied ? "Copied!" : "Copy public key"}
        </button>
        <button
          type="button"
          data-testid="ssh-key-settings-regenerate"
          disabled={generating}
          onclick={generate_or_regenerate}
        >
          {generating ? "Regenerating…" : "Regenerate"}
        </button>
      </div>
    </div>
  {:else if hint.kind === "unavailable"}
    <div data-testid="ssh-key-settings-unavailable" class="ssh-key-settings-state ssh-key-settings-unavailable">
      <p>
        <strong>The OS credential store is unavailable.</strong>
        Trail cannot store or load your SSH key until the
        credential store is reachable.
      </p>
      <p class="ssh-key-settings-reason" data-testid="ssh-key-settings-reason">
        Reason: <code>{hint.reason}</code>
      </p>
      <p class="ssh-key-settings-fallback">
        On macOS, check that the Keychain is unlocked. On
        Linux, ensure <code>gnome-keyring-daemon</code> or
        <code>kwalletd5</code> is running. On Windows,
        verify Credential Manager is enabled. The SSH push
        is disabled until the credential store is back.
      </p>
      <button
        type="button"
        data-testid="ssh-key-settings-retry"
        onclick={refresh_hint}
      >
        Retry
      </button>
    </div>
  {/if}

  {#if error_message}
    <p
      class="ssh-key-settings-error"
      data-testid="ssh-key-settings-error"
    >
      {error_message}
    </p>
  {/if}
</section>

<style>
  .ssh-key-settings {
    margin-top: 1.5rem;
    padding: 0.9rem 1rem;
    border: 1px solid var(--border, #ddd);
    border-radius: 6px;
    background: var(--panel, #fafafa);
  }
  .ssh-key-settings h2 {
    margin: 0 0 0.5rem 0;
    font-size: 1rem;
  }
  .ssh-key-settings-state {
    margin-top: 0.5rem;
  }
  .ssh-key-settings-state p {
    margin: 0.4rem 0;
    line-height: 1.45;
  }
  .ssh-key-settings-empty button,
  .ssh-key-settings-public-only button,
  .ssh-key-settings-key-pair button,
  .ssh-key-settings-unavailable button {
    margin-top: 0.5rem;
    margin-right: 0.5rem;
    padding: 0.35rem 0.8rem;
    border-radius: 4px;
    border: 1px solid #1976d2;
    background: #1976d2;
    color: #fff;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .ssh-key-settings-unavailable button {
    border: 1px solid #c62828;
    background: #fff;
    color: #c62828;
  }
  .ssh-key-settings-state button:disabled {
    opacity: 0.6;
    cursor: wait;
  }
  .ssh-key-settings-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .ssh-key-settings-reason code,
  .ssh-key-settings-fallback code {
    background: #f0f0f0;
    padding: 0 0.3em;
    border-radius: 3px;
    font-size: 0.85em;
  }
  .ssh-key-settings-error {
    margin-top: 0.5rem;
    color: #c62828;
    font-size: 0.9rem;
  }
  pre {
    background: #f5f5f5;
    padding: 0.5rem;
    border-radius: 4px;
    overflow-x: auto;
    font-size: 0.8rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
</style>
