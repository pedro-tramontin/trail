<script lang="ts">
  /**
   * Phase 7 §7.5 — yellow demo-mode banner.
   *
   * Renders the contract text ONLY when `is_demo === true`. The
   * parent (`App.svelte`) feeds this prop from the result of
   * `invoke('demo_status')`. The banner is intentionally
   * standalone (no business logic) so it can mount anywhere
   * (wizard + main shell).
   *
   * The literal banner text is the source of truth on the Rust
   * side (`demo::DEMO_BANNER_TEXT` in `src-tauri/src/demo.rs`).
   * The Svelte template mirrors the same string verbatim so the
   * vitest test can assert the exact bytes the user sees.
   */
  interface Props {
    is_demo: boolean;
  }
  let { is_demo = false }: Props = $props();
</script>

{#if is_demo}
  <div
    class="demo-banner"
    role="alert"
    data-testid="demo-banner"
    aria-live="polite"
  >
    <span class="icon" aria-hidden="true">⚠️</span>
    <span class="text">
      Demo mode — no real captures. Go to Settings to set up real captures.
    </span>
  </div>
{/if}

<style>
  .demo-banner {
    position: sticky;
    top: 0;
    z-index: 1000;
    background-color: #f5d76e;
    color: #1f1f1f;
    padding: 0.5rem 1rem;
    font-size: 0.9rem;
    font-weight: 500;
    text-align: center;
    border-bottom: 1px solid #c2a233;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    width: 100%;
    box-sizing: border-box;
  }
  .icon {
    font-size: 1.1rem;
  }
  .text {
    flex: 0 1 auto;
  }
</style>
