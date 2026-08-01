<script lang="ts">
  import { untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import type { OnboardingAnswers } from "./types";

  /**
   * Step 6 — Finish. Calls the Phase C `write_onboarding_config`
   * Tauri command (item 6-3) with the LLM's `OnboardingAnswers`
   * and the `ssh_key_generated` flag indicating the StepTransport
   * keygen succeeded. On success, calls the `on_complete` prop
   * with the written config path. On failure, renders the error
   * inline + a "Retry" button.
   *
   * The parent `Onboarding.svelte` listens for `on_complete` and
   * re-mounts the regular shell (App.svelte will see the new
   * `~/.trail/config.json` and skip the wizard on next render).
   */

  let {
    answers = null,
    ssh_key_generated = false,
    on_complete,
  }: {
    answers: OnboardingAnswers | null;
    ssh_key_generated?: boolean;
    on_complete: (config_path: string) => void;
  } = $props();

  let writing = $state(false);
  let done = $state(false);
  let written_path = $state<string | null>(null);
  let error = $state<string | null>(null);
  let complete_timer: ReturnType<typeof setTimeout> | null = null;

  async function write_now(): Promise<void> {
    if (!answers) return;
    writing = true;
    error = null;
    try {
      const path = await invoke<string>("write_onboarding_config", {
        answers,
        sshKeyGenerated: ssh_key_generated,
      });
      written_path = path;
      done = true;
      // Give the user a beat to read the success message, then
      // signal the parent to swap to the regular shell.
      complete_timer = setTimeout(() => on_complete(path), 600);
    } catch (err) {
      error = String(err);
    } finally {
      writing = false;
    }
  }

  $effect(() => {
    // Only re-run when `answers` changes; reading `done` and
    // `writing` inside `untrack` so the timer-scheduling effect
    // doesn't tear down the just-scheduled `complete_timer`
    // when the post-write `done = true` re-triggers us.
    if (answers) {
      untrack(() => {
        if (!done && !writing) {
          void write_now();
        }
      });
    }
    return () => {
      // Component unmount: clear the pending completion
      // callback so it doesn't fire after teardown (which
      // would call `on_complete` on a dead parent).
      if (complete_timer !== null) {
        clearTimeout(complete_timer);
        complete_timer = null;
      }
    };
  });
</script>

<section class="step" data-testid="step-finish">
  <h2>All set</h2>

  {#if writing}
    <p class="muted" data-testid="finish-writing">
      <span class="spinner" aria-hidden="true">⏳</span> Writing your config…
    </p>
  {:else if done && written_path}
    <p class="success" data-testid="finish-success">
      ✅ Config written to <code>{written_path}</code>
    </p>
    <p class="muted">Loading the main shell…</p>
  {:else if error}
    <p class="error" role="alert" data-testid="finish-error">
      Could not write the config: {error}
    </p>
    <div class="actions">
      <button
        type="button"
        class="primary"
        data-testid="finish-retry"
        onclick={() => {
          void write_now();
        }}
      >
        Retry
      </button>
    </div>
  {/if}
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
  .success {
    color: #166534;
  }
  .error {
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
  .primary:hover {
    background: var(--primary-hover, #1d4ed8);
  }
  .spinner {
    display: inline-block;
  }
</style>
