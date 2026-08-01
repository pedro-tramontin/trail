<script lang="ts">
  /**
   * Inline JSON viewer for the expanded row in `Logs.svelte`. Renders
   * the raw payload as pretty-printed text inside a `<pre>` and offers
   * a copy-to-clipboard shortcut. No syntax highlighting in v1.
   */
  interface Props {
    json: unknown;
  }
  let { json }: Props = $props();

  let copied = $state(false);

  function prettyPrint(value: unknown): string {
    return JSON.stringify(value, null, 2);
  }

  async function copyToClipboard(): Promise<void> {
    try {
      await navigator.clipboard.writeText(prettyPrint(json));
      copied = true;
      setTimeout(() => {
        copied = false;
      }, 1500);
    } catch {
      // Clipboard access may fail in jsdom; ignore.
    }
  }
</script>

<div class="logs-detail" data-testid="logs-detail">
  <button class="copy" type="button" onclick={copyToClipboard}>
    {copied ? "Copied!" : "Copy"}
  </button>
  <pre><code>{prettyPrint(json)}</code></pre>
</div>

<style>
  .logs-detail {
    position: relative;
  }
  .copy {
    position: absolute;
    top: 0.5rem;
    right: 0.5rem;
    padding: 0.25rem 0.5rem;
    background: var(--bg, white);
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.85rem;
  }
  pre {
    background: var(--code-bg, #f5f5f5);
    padding: 1rem;
    border-radius: 4px;
    overflow-x: auto;
    font-size: 0.85rem;
    line-height: 1.4;
    margin: 0;
  }
</style>
