import { render, screen, waitFor } from "@testing-library/svelte";
import { vi, describe, it, beforeEach, expect } from "vitest";
import CollectorSettings from "./CollectorSettings.svelte";
import type { CollectorInfo, CollectorSource } from "./api/collectors";

// Mock the Tauri IPC bridge at the @tauri-apps/api/core layer.
// This is what `src/lib/api/collectors.ts` calls under the hood, so
// resolving `invoke(...)` here with canned data feeds straight into
// the component's onMount `listCollectors(...)` call.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

const MOCK_COLLECTORS: CollectorInfo[] = [
  {
    source: "github",
    enabled: true,
    schedule: "@hourly",
    last_run_at: null,
    last_exit_code: null,
    last_error: null,
  },
  {
    source: "claude_sessions",
    enabled: false,
    schedule: "@hourly",
    last_run_at: null,
    last_exit_code: null,
    last_error: null,
  },
  {
    source: "calendar",
    enabled: true,
    schedule: "@hourly",
    last_run_at: null,
    last_exit_code: null,
    last_error: null,
  },
];

const SOURCES: CollectorSource[] = ["github", "claude_sessions", "calendar"];

describe("CollectorSettings", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    // Default: list_collectors returns the three-row fixture on every call.
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_collectors") return MOCK_COLLECTORS;
      if (cmd === "run_collector_now") return 0;
      if (cmd === "set_collector_enabled") return undefined;
      return null;
    });
  });

  it("(a) renders one row per mocked collector with the correct enabled state", async () => {
    render(CollectorSettings, {
      props: { configPath: "/x", collectorBin: "/y" },
    });

    for (const source of SOURCES) {
      await waitFor(() =>
        expect(screen.getByTestId(`row-${source}`)).toBeInTheDocument(),
      );
    }
    expect(
      (screen.getByTestId("toggle-github") as HTMLInputElement).checked,
    ).toBe(true);
    expect(
      (screen.getByTestId("toggle-claude_sessions") as HTMLInputElement)
        .checked,
    ).toBe(false);
    expect(
      (screen.getByTestId("toggle-calendar") as HTMLInputElement).checked,
    ).toBe(true);
  });

  it("(b) flipping a toggle dispatches set_collector_enabled", async () => {
    // Initial list_collectors returns MOCK_COLLECTORS; the post-toggle
    // refresh should observe the new state.
    const enabledClaude = {
      ...MOCK_COLLECTORS[1],
      enabled: true,
    } as CollectorInfo;
    const afterToggle: CollectorInfo[] = [
      MOCK_COLLECTORS[0],
      enabledClaude,
      MOCK_COLLECTORS[2],
    ];
    let listCallCount = 0;
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_collectors") {
        listCallCount++;
        return listCallCount === 1 ? MOCK_COLLECTORS : afterToggle;
      }
      if (cmd === "set_collector_enabled") return undefined;
      if (cmd === "run_collector_now") return 0;
      return null;
    });

    render(CollectorSettings, {
      props: { configPath: "/x", collectorBin: "/y" },
    });
    await waitFor(() =>
      expect(screen.getByTestId("toggle-claude_sessions")).toBeInTheDocument(),
    );

    (screen.getByTestId("toggle-claude_sessions") as HTMLInputElement).click();

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "set_collector_enabled",
        expect.objectContaining({
          source: "claude_sessions",
          enabled: true,
          configPath: "/x",
          collectorBin: "/y",
        }),
      ),
    );
  });

  it("(c) clicking 'Run now' dispatches run_collector_now", async () => {
    render(CollectorSettings, {
      props: { configPath: "/x", collectorBin: "/y" },
    });
    await waitFor(() =>
      expect(screen.getByTestId("run-now-github")).toBeInTheDocument(),
    );

    (screen.getByTestId("run-now-github") as HTMLButtonElement).click();

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "run_collector_now",
        expect.objectContaining({
          source: "github",
          configPath: "/x",
          collectorBin: "/y",
        }),
      ),
    );
  });
});
