<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { writable, get, type Writable } from "svelte/store";
  import type { StepTransportState } from "./types";

  /**
   * Step 4 — Transport configuration.
   *
   * Collects the VPS connection details (host, user, port) and
   * either generates a fresh ed25519 SSH keypair (stored in the
   * OS credential store via the `generate_ssh_key` Tauri command
   * from item 1-2) or attaches an existing key already in the
   * OS credential store. The returned key path is surfaced in
   * the UI so the user can confirm where the public key went.
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
   *   1. **Generate** — clicks the "Generate SSH key" button.
   *      Calls `generate_ssh_key`, which is idempotent
   *      (re-running returns the existing public key in the
   *      OS credential store). First-run path.
   *   2. **Use existing** — clicks "Use existing key in OS
   *      credential store". Calls `get_ssh_public_key`; if it
   *      returns `Some(pubkey)`, we set `ssh_key_path` to that
   *      value without generating a new key. If `None`, we
   *      surface a "no existing key found" error and the user
   *      can fall back to Generate.
   *
   * Both paths end with the same `ssh_key_path` value — a
   * public key in OpenSSH single-line form. The `ssh_key_source`
   * tag is cosmetic (for the UI hint about which path was
   * used); it doesn't affect the Next-button enable logic.
   *
   * ## "Always-editable key" UX (PR #219, 2026-08-11)
   *
   * The pre-PR behaviour hid the Generate / Use existing
   * buttons once a key was attached — only the key path was
   * shown. If the user picked the wrong key (e.g. ran "Use
   * existing" before generating and now wants to switch), they
   * had to leave the screen and come back, which made the
   * "test connection → see failure → switch key → test again"
   * loop impossible to do in one session. The new behaviour:
   *   - Generate + Use existing buttons are ALWAYS rendered
   *     (regardless of `ssh_key_path`).
   *   - The currently-attached key path is shown BELOW the
   *     buttons as info text, not as a replacement for them.
   *   - Clicking Generate when a key is already set
   *     re-runs `generate_ssh_key` (idempotent — same path
   *     returned) or rotates to a new key on the same
   *     `ssh_key_path` slot. The user can click as many
   *     times as they want.
   *   - Clicking Use existing overwrites `ssh_key_path` with
   *     whatever's in the OS credential store right now.
   *
   * ## "Test connection" button (PR #193)
   *
   * Calls the `test_ssh_connection` Tauri command (added in
   * this PR), which builds an `SshTransport` in-memory with
   * the (host, port, user) the user typed + publickey auth
   * against whatever key is in the OS credential store, then
   * runs `health_check()`. Result is shown next to the button:
   * a green ✅ "Connected" on success or a red error with
   * the message on failure. We do NOT advance or block Next
   * on the result — it's informational, so the user can still
   * advance with a misconfigured VPS if they want (e.g. to
   * write the config now and fix the transport later).
   *
   * The Test button is gated on the host / user / port being
   * valid (NOT on a key being attached) — even without a key
   * the user can press it, see the auth error, then attach a
   * key and test again, all on the same screen.
   *
   * ## Platform-neutral credential-store wording (§X-3)
   *
   * User-visible strings say "OS credential store" instead of
   * "keychain". A native HTML `title` tooltip on the relevant
   * affordances expands to the platform-specific name fetched
   * from the `credential_store_name` Tauri command (Keychain
   * on macOS, secret-service / GNOME Keyring / KWallet on
   * Linux, Credential Manager on Windows). The command runs
   * once on mount and the result is held in
   * `credential_store_label`; the tooltip falls back to
   * "OS credential store" if the IPC call fails so the UI
   * still renders.
   */

  let {
    state,
    on_next,
  }: {
    state: Writable<StepTransportState>;
    on_next: () => void;
  } = $props();

  // Module-local ref holding the most recent structured HostKeyUnknown
  // payload (host/port/key_b64/key_type). The trust prompt only stashes
  // the fingerprint in `$state`; the raw key bytes needed by
  // `pin_ssh_host_key` are kept here so `trust_key()` can hand them
  // straight to the command without a second connection.
  let last_unknown_payload: {
    host: string;
    port: number;
    key_b64: string;
    key_type: string;
  } | null = null;

  // Per-OS user-facing name of the OS credential store. Loaded
  // once on mount from the `credential_store_name` Tauri command
  // (PR §X-3). The fallback "OS credential store" matches the
  // platform-neutral wording in the body copy — if the IPC
  // call fails (test env, command not registered), the tooltip
  // degrades to the same label the user already sees inline.
  //
  // We use a Svelte `writable` store rather than a `$state` rune
  // for the label. The `state` prop above shadows the `$state`
  // rune (svelte-check would flag it as `store_rune_conflict`),
  // but a `writable` is just a regular variable — `$credential_label`
  // in the template auto-subscribes and re-renders when the
  // store's value changes. Net effect: tooltip updates on
  // mount without an explicit `$state` declaration in this file.
  const credential_label_store = writable<string>("OS credential store");
  onMount(async () => {
    try {
      const label = await invoke<string>("credential_store_name");
      if (label) credential_label_store.set(label);
    } catch {
      // Keep the fallback. The tooltip is best-effort UX; the
      // body copy already says "OS credential store" so a missing
      // IPC just means the tooltip repeats the body label.
    }
  });

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
  // 2026-08-11 — the Test-connection button gates on
  // inputs only (not on a key being attached). Pre-PR it
  // inherited `can_advance`, which requires
  // `ssh_key_path !== null`. That was wrong UX: a user
  // with no key picked can't even try the test, so they
  // can't see the auth error message that would tell
  // them "you need a key attached." Allowing them to
  // test without a key surfaces that exact error from
  // the backend, which is informative.
  const inputs_valid = $derived(host_valid && user_valid && port_valid);
  // 2026-09-09 (Copilot review on PR #281): the Mismatch hold should
  // release when the user changes the (host, port) they're targeting,
  // not only when they click "Test connection" again. The hold
  // applies to the SPECIFIC server that just presented the unexpected
  // key — editing host or port means the user has moved on to a
  // different target, so the held state + the stale-key payload both
  // must clear. Same applies to the trust prompt + the pinner's
  // stashed HostKeyUnknown payload: pinning after a host/port change
  // would otherwise pin the OLD server's key against the NEW
  // host_field, which is a real security bug (stale-key pinning).
  //
  // Implementation note — Svelte 5 store auto-subscribe ($store.X)
  // tracks the WHOLE store, not individual fields. We use the
  // auto-subscribe to track host + port (so the effect re-fires when
  // either changes), then compare against remembered previous values
  // to decide whether the body should run. The body is a no-op when
  // the trigger was NOT a host/port change (e.g. test_connection
  // setting pending_trust_action).
  let prev_host = $state.host;
  let prev_port = $state.port;
  $effect(() => {
    // Read host + port via auto-subscribe (these reads track the store
    // for THIS effect). The body only acts when one of them changed.
    const cur_host = $state.host;
    const cur_port = $state.port;
    const host_changed = cur_host !== prev_host;
    const port_changed = cur_port !== prev_port;
    prev_host = cur_host;
    prev_port = cur_port;
    if (!(host_changed || port_changed)) return;
    // Editing host or port clears the Mismatch hold + the trust prompt +
    // the test_error + the stale-key payload. The hold is bound to the
    // SPECIFIC server that just presented the unexpected key — a
    // different host:port means the user has moved on.
    state.update((s) => {
      s.mismatch_held = false;
      s.pending_fingerprint = null;
      s.pending_trust_action = null;
      s.test_error = null;
      s.test_state = "idle";
      return s;
    });
    last_unknown_payload = null;
  });
  const can_advance = $derived(
    inputs_valid && $state.ssh_key_path !== null && !$state.mismatch_held,
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

  /** Read the public key for the key already in the OS
   *  credential store. If found, adopt it as the wizard's
   *  `ssh_key_path`. If no key exists, surface an actionable
   *  error so the user can fall back to Generate. */
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
            "No existing SSH key found in OS credential store. Click 'Generate SSH key' to create one.";
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
      // A fresh test clears any prior trust prompt / mismatch banner.
      s.pending_fingerprint = null;
      s.pending_trust_action = null;
      s.mismatch_held = false;
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
      // `test_ssh_connection` returns `Result<(), TransportError>`
      // and Tauri serializes the error via serde's externally-tagged
      // form, so we see one of:
      //   {"HostKeyUnknown":   { host, port, fingerprint, key_b64, key_type }}
      //   {"HostKeyMismatch":  { host, port, presented_fingerprint }}
      //   {"Ssh":              "<reason>"}
      //   {"Config":           "<reason>"}
      //   {"Io":               "<reason>"}
      //
      // We dispatch on every variant so the wizard can:
      //   - show a TOFU prompt on HostKeyUnknown (raw key bytes are
      //     in the payload so we can pin on "Yes, trust")
      //   - show a hard red banner on HostKeyMismatch (no retry path)
      //   - show a useful error message on Ssh/Config/Io (rather
      //     than the previous behavior of rendering `[object Object]`
      //     because String(err) on a plain object produced that).
      //
      // If Tauri ever returns a shape we don't recognize (e.g. a
      // future TransportError variant), we still show SOMETHING
      // useful — JSON.stringify the whole payload so the user can
      // copy-paste it into a bug report.
      //
      // err-shape handling (the previous fix had a bug here):
      // Tauri's invoke() can reject with any of:
      //   - a string (the IPC-returned error serialized to a string)
      //   - an Error object (if the JS side threw, e.g. permission
      //     denied on the IPC channel)
      //   - a plain object (if the JSON payload was misparsed by a
      //     Tauri version mismatch — happened on the user's box
      //     and surfaced as "Unexpected error: [object Object]")
      // String() on the last two produces "[object Object]" or
      // "Error: <msg>". The robust pattern: prefer err.message when
      // it's an Error; otherwise try JSON.stringify directly on the
      // object (which produces a real JSON string we can then parse
      // + dispatch on). Only fall through to String(err) as a last
      // resort, after we have something useful to display.
      let raw: string;
      let parsed: Record<string, unknown> | null = null;
      if (err instanceof Error) {
        // Tauri IPC-level error (permission denied, command not
        // found, etc.). err.message is the useful part.
        raw = err.message;
      } else if (typeof err === "string") {
        raw = err;
      } else {
        // Plain object (or anything else non-string). JSON.stringify
        // it first so we can JSON.parse + dispatch on the result.
        try {
          raw = JSON.stringify(err);
        } catch {
          raw = String(err); // last-resort fallback; may be "[object Object]"
        }
      }
      try {
        parsed = JSON.parse(raw);
      } catch {
        parsed = null;
      }

      const hku = parsed?.HostKeyUnknown as
        | {
            host: string;
            port: number;
            fingerprint: string;
            key_b64: string;
            key_type: string;
          }
        | undefined;
      const hkm = parsed?.HostKeyMismatch as
        | {
            host: string;
            port: number;
            presented_fingerprint: string;
          }
        | undefined;
      const sshMsg =
        typeof parsed?.Ssh === "string" ? (parsed.Ssh as string) : null;
      const configMsg =
        typeof parsed?.Config === "string"
          ? (parsed.Config as string)
          : null;
      const ioMsg =
        typeof parsed?.Io === "string" ? (parsed.Io as string) : null;

      if (hku) {
        last_unknown_payload = {
          host: hku.host,
          port: hku.port,
          key_b64: hku.key_b64,
          key_type: hku.key_type,
        };
        state.update((s) => {
          s.test_state = "error";
          s.test_error = `Trail doesn't recognize this server's host key (${hku.fingerprint}).`;
          s.pending_fingerprint = hku.fingerprint;
          s.pending_trust_action = "unknown";
          return s;
        });
      } else if (hkm) {
        state.update((s) => {
          s.test_state = "error";
          s.test_error = `HOST KEY MISMATCH for ${hkm.host}:${hkm.port} — refusing to connect.`;
          s.pending_fingerprint = hkm.presented_fingerprint;
          s.pending_trust_action = "mismatch";
          s.mismatch_held = true;
          return s;
        });
      } else if (sshMsg) {
        state.update((s) => {
          s.test_state = "error";
          s.test_error = `SSH error: ${ sshMsg }`;
          return s;
        });
      } else if (configMsg) {
        state.update((s) => {
          s.test_state = "error";
          s.test_error = `Configuration error: ${ configMsg }`;
          return s;
        });
      } else if (ioMsg) {
        state.update((s) => {
          s.test_state = "error";
          s.test_error = `Network/I-O error: ${ ioMsg }`;
          return s;
        });
      } else {
        // Unrecognized error shape — stringify the whole payload
        // so the user can copy-paste it into a bug report rather
        // than seeing the useless "[object Object]".
        state.update((s) => {
          s.test_state = "error";
          s.test_error = parsed
            ? `Unexpected error: ${ JSON.stringify(parsed) }`
            : `Unexpected error: ${ raw }`;
          return s;
        });
      }
    }
  }

  /** Pin the server's host key on the explicit "Yes, trust this key"
   *  click. The raw key bytes + type come from the structured
   *  `HostKeyUnknown` error captured in `pending_*` state. On success,
   *  re-run `test_connection` to confirm the new entry is accepted and
   *  the green ✅ path renders. */
  async function trust_key(): Promise<void> {
    // The key bytes are carried on the structured error, but we only
    // stashed the fingerprint in state. Re-derive from the last error
    // is not possible here, so we re-test to re-fetch the structured
    // payload, then pin. Simpler: the component keeps the last
    // structured HostKeyUnknown payload in a module-local ref.
    const payload = last_unknown_payload;
    if (!payload) return;
    state.update((s) => {
      s.pinning = true;
      return s;
    });
    try {
      await invoke("pin_ssh_host_key", {
        host: payload.host,
        port: payload.port,
        keyB64: payload.key_b64,
        keyType: payload.key_type,
      });
      state.update((s) => {
        s.pinning = false;
        s.pending_fingerprint = null;
        s.pending_trust_action = null;
        return s;
      });
      // Re-test to confirm the new entry is accepted.
      await test_connection();
    } catch (err) {
      // pin_ssh_host_key returns Result<(), String> — a free-form
      // error message like "entry for vm:22 already exists" or
      // "write to ~/.trail/known_hosts: failed". Tauri rejects with
      // the string directly (no JSON envelope). Show it verbatim
      // rather than the "Error: " prefix String(Error) would yield.
      const message = err instanceof Error ? err.message : String(err);
      state.update((s) => {
        s.pinning = false;
        s.test_state = "error";
        s.test_error = `Couldn't pin host key: ${ message }`;
        return s;
      });
    }
  }

  /** Clear the trust prompt without writing to known_hosts. */
  async function cancel_trust(): Promise<void> {
    state.update((s) => {
      s.pending_fingerprint = null;
      s.pending_trust_action = null;
      s.test_error = null;
      s.test_state = "idle";
      return s;
    });
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
      <!--
        2026-08-11 — always render the Generate + Use
        existing buttons so the user can switch keys
        without leaving the screen (pre-PR the buttons
        disappeared once `ssh_key_path` was set, which
        made the "test → fail → switch key → test again"
        loop impossible). The currently-attached key is
        shown BELOW as info text — never replacing the
        buttons.
      -->
      <div class="key-actions">
        <button
          type="button"
          class="secondary"
          data-testid="transport-generate-key"
          disabled={$state.generating}
          title="Store a fresh ed25519 key in the {$credential_label_store}"
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
          title="Reuse the ed25519 key already in the {$credential_label_store}"
          onclick={() => {
            void use_existing_key();
          }}
        >
          Use existing key in OS credential store
        </button>
      </div>
      {#if $state.ssh_key_path}
        <p
          class="muted"
          data-testid="transport-key-path"
          title="Stored in the {$credential_label_store}"
        >
          ✅ Currently attached: {$state.ssh_key_source === "existing"
            ? "existing key from"
            : "generated key in"} OS credential store — <code>{$state.ssh_key_path}</code>
        </p>
      {:else}
        <p class="hint" data-testid="transport-key-hint">
          No key attached yet. Pick one above — Next stays disabled until a
          key is attached.
        </p>
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
          disabled={!inputs_valid || $state.test_state === "testing"}
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

  {#if $state.pending_trust_action === "unknown" && $state.pending_fingerprint}
    <div class="trust-prompt" data-testid="transport-trust-prompt">
      <p class="muted">
        Trail doesn't recognize this server's host key. To trust it, confirm
        the fingerprint matches what your VPS administrator told you:
      </p>
      <p class="fingerprint" data-testid="transport-fingerprint">
        <code>{$state.pending_fingerprint}</code>
      </p>
      <div class="trust-actions">
        <button
          type="button"
          class="primary"
          data-testid="transport-trust-confirm"
          disabled={$state.pinning}
          onclick={() => {
            void trust_key();
          }}
        >
          {$state.pinning ? "Pinning…" : "Yes, trust this key"}
        </button>
        <button
          type="button"
          class="secondary"
          data-testid="transport-trust-cancel"
          disabled={$state.pinning}
          onclick={() => {
            void cancel_trust();
          }}
        >
          Cancel
        </button>
      </div>
    </div>
  {/if}
  {#if $state.pending_trust_action === "mismatch"}
    <div
      class="mismatch-banner"
      data-testid="transport-mismatch-banner"
      role="alert"
    >
      <p class="banner-title">
        ⚠️ HOST KEY MISMATCH — possible man-in-the-middle.
      </p>
      <p class="muted">
        The server's host key has changed since you first trusted it. Trail
        will not connect until the issue is resolved out-of-band (e.g. your
        VPS was reprovisioned, or you are being attacked).
      </p>
      <p class="muted">
        Server presented: <code data-testid="transport-mismatch-fingerprint">{$state.pending_fingerprint ?? ""}</code>
      </p>
      <p class="hint">
        Change the host or port above to a different server, or contact your
        VPS administrator before retrying.
      </p>
    </div>
  {/if}

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
  .trust-prompt {
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .fingerprint {
    margin: 0;
  }
  .trust-actions {
    display: flex;
    gap: 0.5rem;
  }
  .mismatch-banner {
    border: 1px solid var(--danger, #c00);
    background: #fef2f2;
    border-radius: 4px;
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .banner-title {
    margin: 0;
    font-weight: 700;
    color: var(--danger, #c00);
  }
</style>
