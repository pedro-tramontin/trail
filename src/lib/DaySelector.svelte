<script lang="ts">
  import { logsState, selectDate } from "./stores/logs.svelte";

  /**
   * Builds a list of the last 30 dates as ISO `YYYY-MM-DD` strings, with
   * today's date at index 0. Computed once at component mount — does
   * not need to be reactive (the date set never changes within a
   * component's lifetime; the user picks from a precomputed list).
   */
  const days: string[] = (() => {
    const out: string[] = [];
    const today = new Date();
    for (let i = 0; i < 30; i++) {
      const d = new Date(today);
      d.setDate(today.getDate() - i);
      const y = d.getFullYear();
      const m = String(d.getMonth() + 1).padStart(2, "0");
      const day = String(d.getDate()).padStart(2, "0");
      out.push(`${y}-${m}-${day}`);
    }
    return out;
  })();
  const selected: string = $derived(logsState.selectedDate);

  function onChange(event: Event): void {
    const target = event.target as HTMLSelectElement;
    selectDate(target.value);
  }
</script>

<label class="day-selector">
  <span class="visually-hidden">Day</span>
  <select
    value={selected}
    onchange={onChange}
    disabled={logsState.loading}
    data-testid="day-selector"
    aria-label="Select a day"
  >
    {#each days as day (day)}
      <option value={day}>{day}</option>
    {/each}
  </select>
</label>

<style>
  .day-selector {
    display: inline-block;
  }
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
  select {
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border, #ccc);
    border-radius: 4px;
    background: var(--bg, white);
    color: var(--fg, black);
  }
  select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
