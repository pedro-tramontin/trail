import { describe, it, expect, beforeEach, vi } from "vitest";
import { logsState, refresh, remove } from "./stores/logs.svelte";
import { getRawJson } from "$lib/api/logs";

// Mock the IPC bridge the store and the timeline depend on. The
// store calls `refresh()` which fires `listLogs(date)`; the timeline
// also calls `getRawJson(date, source)` when a row is expanded.
// Mocking at the API layer keeps the real `$state`-backed store
// observable while preventing any IPC traffic.
vi.mock("$lib/api/logs", () => ({
  listLogs: vi.fn(),
  deleteLog: vi.fn(),
  getRawJson: vi.fn(),
}));

import * as api from "$lib/api/logs";
const mockApi = vi.mocked(api);

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

beforeEach(() => {
  vi.clearAllMocks();
  logsState.entries = [];
  logsState.selectedDate = "2026-07-29";
  logsState.loading = false;
  logsState.error = null;
});

describe("Logs timeline", () => {
  it("initial state is empty", () => {
    expect(logsState.entries).toEqual([]);
    expect(logsState.error).toBeNull();
    expect(logsState.loading).toBe(false);
  });

  it("loadDay populates entries", async () => {
    mockApi.listLogs.mockResolvedValueOnce(sample);
    await refresh();
    expect(logsState.entries).toEqual(sample);
    expect(logsState.loading).toBe(false);
    expect(logsState.error).toBeNull();
  });

  it("getRawJson returns the raw JSON when row expanded", async () => {
    const raw = {
      source: "github",
      captured_at: "2026-07-29T18:00:00Z",
      payload: { prs: [] },
    };
    mockApi.getRawJson.mockResolvedValueOnce(raw);
    const result = await getRawJson("2026-07-29", "github");
    expect(result).toEqual(raw);
    expect(mockApi.getRawJson).toHaveBeenCalledWith("2026-07-29", "github");
  });

  it("delete calls deleteLog and refreshes", async () => {
    mockApi.listLogs.mockResolvedValueOnce(sample);
    await refresh();
    mockApi.deleteLog.mockResolvedValueOnce(undefined);
    mockApi.listLogs.mockResolvedValueOnce([]);
    await remove("github");
    expect(mockApi.deleteLog).toHaveBeenCalledWith("2026-07-29", "github");
    expect(logsState.entries).toEqual([]);
  });
});
