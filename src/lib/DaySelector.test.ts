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

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.listLogs.mockResolvedValue([]);
  // Reset to a fixed, deterministic state — overrides what `todayIso()`
  // produced on import so each test is reproducible.
  logsState.selectedDate = "2026-08-01";
  logsState.loading = false;
  logsState.entries = [];
  logsState.error = null;
});

describe("DaySelector", () => {
  it("(a) defaults to today (initial selectedDate from the store)", () => {
    // The store's `todayIso()` seeds `selectedDate` on import; we then
    // set it to a fixed value so the test stays deterministic. The
    // component must echo that value as its `value` attribute.
    render(DaySelector);
    const select = screen.getByTestId(
      "day-selector",
    ) as unknown as HTMLSelectElement;
    expect(select.value).toBe("2026-08-01");
    expect(select.disabled).toBe(false);
  });

  it("(b) selecting a past date calls selectDate with that ISO string", async () => {
    render(DaySelector);
    const select = screen.getByTestId(
      "day-selector",
    ) as unknown as HTMLSelectElement;
    // fireEvent.change sets `.value` and dispatches a real `change`
    // event; native <select> elements expose the selected option's
    // value back via `event.target.value` inside the handler.
    await fireEvent.change(select, { target: { value: "2026-07-29" } });
    // selectDate() calls refresh() which awaits listLogs(selectedDate).
    await vi.waitFor(() =>
      expect(mockApi.listLogs).toHaveBeenCalledWith("2026-07-29"),
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
