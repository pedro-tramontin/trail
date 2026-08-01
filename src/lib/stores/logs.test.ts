import { describe, it, expect, beforeEach, vi } from "vitest";
import { logsState, refresh, remove, selectDate } from "./logs.svelte";
import * as api from "$lib/api/logs";

vi.mock("$lib/api/logs", () => ({
  listLogs: vi.fn(),
  deleteLog: vi.fn(),
  getRawJson: vi.fn(),
}));

const mockApi = vi.mocked(api);

const sampleEntries = [
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

describe("logsState", () => {
  it("refresh populates entries on success", async () => {
    mockApi.listLogs.mockResolvedValueOnce(sampleEntries);
    await refresh();
    expect(logsState.entries).toEqual(sampleEntries);
    expect(logsState.loading).toBe(false);
    expect(logsState.error).toBeNull();
  });

  it("refresh sets error on failure", async () => {
    mockApi.listLogs.mockRejectedValueOnce(new Error("boom"));
    await refresh();
    expect(logsState.entries).toEqual([]);
    expect(logsState.error).toBe("Error: boom");
  });

  it("remove calls api.deleteLog and refreshes", async () => {
    mockApi.listLogs.mockResolvedValueOnce(sampleEntries);
    await refresh();
    mockApi.deleteLog.mockResolvedValueOnce(undefined);
    mockApi.listLogs.mockResolvedValueOnce([sampleEntries[1]]);
    await remove("github");
    expect(mockApi.deleteLog).toHaveBeenCalledWith("2026-07-29", "github");
    expect(logsState.entries).toEqual([sampleEntries[1]]);
  });

  it("remove sets error on failure", async () => {
    mockApi.deleteLog.mockRejectedValueOnce(new Error("delete failed"));
    await remove("github");
    expect(logsState.error).toBe("Error: delete failed");
  });

  it("selectDate updates date and triggers refresh", async () => {
    mockApi.listLogs.mockResolvedValueOnce([]);
    selectDate("2026-07-30");
    expect(logsState.selectedDate).toBe("2026-07-30");
    // refresh is async; give it a tick.
    await new Promise((r) => setTimeout(r, 0));
    expect(mockApi.listLogs).toHaveBeenCalledWith("2026-07-30");
  });
});
