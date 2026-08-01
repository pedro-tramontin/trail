import { render, screen, fireEvent } from "@testing-library/svelte";
import { vi, describe, it, beforeEach, expect } from "vitest";
import DaySelector from "./DaySelector.svelte";
import { logsState } from "./stores/logs.svelte";
import * as api from "$lib/api/logs";

// Mock the IPC bridge the store depends on. The store calls
// `selectDate(d)`, which internally invokes `refresh()`, which in turn
// fires `listLogs(date)`. Mocking at the API layer keeps the real
// `$state`-backed store observable (so the component's `value` binding
// reacts) while preventing any IPC traffic.
vi.mock("$lib/api/logs", () => ({
  listLogs: vi.fn().mockResolvedValue([]),
  deleteLog: vi.fn().mockResolvedValue(undefined),
  getRawJson: vi.fn(),
}));

const mockApi = vi.mocked(api);

/**
 * Return the ISO `YYYY-MM-DD` string for `n` days ago. Used to
 * build expected dates for deterministic tests without hard-coding
 * a value that would go stale when the wall clock moves past it.
 * (PR #25 Copilot thread T1 / T2, follow-up from PR #44 squash.)
 */
function isoDaysAgo(n: number): string {
  const today = new Date();
  const d = new Date(today);
  d.setDate(d.getDate() - n);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.listLogs.mockResolvedValue([]);
  // Reset to a deterministic state — overrides what `todayIso()`
  // produced on import so each test is reproducible. Derived from
  // the current time so the test stays correct as the wall clock
  // moves forward. (PR #25 Copilot thread T1.)
  logsState.selectedDate = isoDaysAgo(0);
  logsState.loading = false;
  logsState.entries = [];
  logsState.error = null;
});

describe("DaySelector", () => {
  it("(a) defaults to today (initial selectedDate from the store)", () => {
    // The store's `todayIso()` seeds `selectedDate` on import; we then
    // set it to a fixed value so the test stays deterministic. The
    // component must echo that value as its `value` attribute.
    const expectedToday = isoDaysAgo(0);
    render(DaySelector);
    const select = screen.getByTestId(
      "day-selector",
    ) as unknown as HTMLSelectElement;
    expect(select.value).toBe(expectedToday);
    expect(select.disabled).toBe(false);
  });

  it("(b) selecting a past date calls selectDate with that ISO string", async () => {
    render(DaySelector);
    const select = screen.getByTestId(
      "day-selector",
    ) as unknown as HTMLSelectElement;
    // Pick a date that is guaranteed to be in the dropdown (3 days
    // ago, well within the 30-day window).
    const targetDate = isoDaysAgo(3);
    // fireEvent.change sets `.value` and dispatches a real `change`
    // event; native <select> elements expose the selected option's
    // value back via `event.target.value` inside the handler.
    await fireEvent.change(select, { target: { value: targetDate } });
    // selectDate() calls refresh() which awaits listLogs(selectedDate).
    await vi.waitFor(() =>
      expect(mockApi.listLogs).toHaveBeenCalledWith(targetDate),
    );
  });

  it("(c) is disabled while logsState.loading is true", () => {
    logsState.loading = true;
    render(DaySelector);
    const select = screen.getByTestId(
      "day-selector",
    ) as unknown as HTMLSelectElement;
    expect(select.disabled).toBe(true);
  });

  it("(d) re-enables once loading flips back to false", async () => {
    render(DaySelector);
    const select = screen.getByTestId(
      "day-selector",
    ) as unknown as HTMLSelectElement;
    logsState.loading = true;
    await vi.waitFor(() => expect(select.disabled).toBe(true));
    logsState.loading = false;
    await vi.waitFor(() => expect(select.disabled).toBe(false));
  });
});
