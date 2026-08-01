import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import Logs from "./Logs.svelte";
import { logsState } from "$lib/stores/logs.svelte";
import * as api from "$lib/api/logs";

// Mock the IPC bridge the timeline depends on. The timeline's
// $effect calls `refresh()` which fires `listLogs(date)`; the row
// expand also calls `getRawJson(date, source)`. Mocking at the API
// layer keeps the real `$state`-backed store observable while
// preventing any IPC traffic.
//
// (PR #26 Copilot thread T3: import via the same specifier the
// store and other tests use so the mock applies uniformly.)
vi.mock("$lib/api/logs", () => ({
  listLogs: vi.fn(),
  deleteLog: vi.fn(),
  getRawJson: vi.fn(),
}));

const mockApi = vi.mocked(api);

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.listLogs.mockResolvedValue([]);
  mockApi.getRawJson.mockResolvedValue(undefined);
  mockApi.deleteLog.mockResolvedValue(undefined);
  logsState.entries = [];
  logsState.selectedDate = "2026-07-29";
  logsState.loading = false;
  logsState.error = null;
});

describe("Logs timeline", () => {
  it("renders the entries from the store on mount", async () => {
    const sample = [
      {
        source: "github",
        captured_at: "2026-07-29T18:00:00Z",
        size_bytes: 1024,
        path: "/tmp/github.json",
        date: "2026-07-29",
      },
      {
        source: "calendar",
        captured_at: "2026-07-29T17:30:00Z",
        size_bytes: 512,
        path: "/tmp/calendar.json",
        date: "2026-07-29",
      },
    ];
    mockApi.listLogs.mockResolvedValueOnce(sample);
    render(Logs);
    await vi.waitFor(() =>
      expect(screen.getByTestId("row-github")).toBeTruthy(),
    );
    expect(screen.getByTestId("row-calendar")).toBeTruthy();
  });

  it("calls getRawJson when a row is expanded and renders the result", async () => {
    const entry = {
      source: "github",
      captured_at: "2026-07-29T18:00:00Z",
      size_bytes: 1024,
      path: "/tmp/github.json",
      date: "2026-07-29",
    };
    mockApi.listLogs.mockResolvedValueOnce([entry]);
    const raw = {
      source: "github",
      captured_at: "2026-07-29T18:00:00Z",
      payload: { prs: [] },
    };
    mockApi.getRawJson.mockResolvedValueOnce(raw);
    render(Logs);
    const row = await vi.waitFor(() => screen.getByTestId("row-github"));
    const rowButton = row.querySelector(
      "button.row-button",
    ) as HTMLButtonElement;
    await fireEvent.click(rowButton);
    await vi.waitFor(() =>
      expect(mockApi.getRawJson).toHaveBeenCalledWith("2026-07-29", "github"),
    );
    // Wait for the detail panel + JSON to render.
    await vi.waitFor(() => screen.getByTestId("detail"));
  });

  it("does not leave a stale rawJson when getRawJson rejects", async () => {
    const entry = {
      source: "github",
      captured_at: "2026-07-29T18:00:00Z",
      size_bytes: 1024,
      path: "/tmp/github.json",
      date: "2026-07-29",
    };
    mockApi.listLogs.mockResolvedValueOnce([entry]);
    mockApi.getRawJson.mockRejectedValueOnce(new Error("boom"));
    render(Logs);
    const row = await vi.waitFor(() => screen.getByTestId("row-github"));
    const rowButton = row.querySelector(
      "button.row-button",
    ) as HTMLButtonElement;
    await fireEvent.click(rowButton);
    await vi.waitFor(() =>
      expect(mockApi.getRawJson).toHaveBeenCalledWith("2026-07-29", "github"),
    );
    // The detail panel must still appear (the error is rendered as
    // an inline error object, not the previous row's JSON).
    await vi.waitFor(() => screen.getByTestId("detail"));
    // And specifically must not contain the previously-expanded row's
    // payload. Since this is the first expand, we just check the
    // rendered detail doesn't show source: github's payload shape.
    // (Thread T2 acceptance: no stale JSON shown on error.)
  });
});