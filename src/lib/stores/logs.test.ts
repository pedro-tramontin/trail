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

  /// Race: refresh is in flight for date A; the user changes the
  /// date to B before the A-response lands. The fix snapshots the
  /// target date at the start of `refresh()` and only commits
  /// `entries` if the user is still on that date. Pre-fix code
  /// would commit A's data into B's view.
  it("refresh ignores stale response when selectedDate changes", async () => {
    // Make listLogs return different data per call so we can
    // distinguish which response landed.
    const aEntries = [
      { source: "a", captured_at: "2026-07-29T10:00:00Z", size_bytes: 1, path: "/tmp/a", date: "2026-07-29" },
    ];
    const bEntries = [
      { source: "b", captured_at: "2026-07-30T10:00:00Z", size_bytes: 1, path: "/tmp/b", date: "2026-07-30" },
    ];
    // Hold the A-response so we can switch the date before it lands.
    let releaseA: (v: typeof aEntries) => void = () => {};
    const aPromise = new Promise<typeof aEntries>((res) => {
      releaseA = res;
    });
    mockApi.listLogs
      .mockReturnValueOnce(aPromise as unknown as ReturnType<typeof mockApi.listLogs>)
      .mockResolvedValueOnce(bEntries);

    // Kick off refresh for date A.
    const aRefresh = refresh();
    // Switch the date to B before A's response lands.
    logsState.selectedDate = "2026-07-30";
    // Now release the A-response and await the A refresh.
    releaseA(aEntries);
    await aRefresh;
    // A's response must NOT have overwritten the (still-empty)
    // B-view state.
    expect(logsState.entries).not.toEqual(aEntries);
    // A subsequent refresh on B should populate B's view.
    await refresh();
    expect(logsState.entries).toEqual(bEntries);
  });
});
