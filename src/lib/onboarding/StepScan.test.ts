import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import StepScan from "./StepScan.svelte";
import { MOCK_SCAN_REPORT } from "./types";

const { invoke_mock } = vi.hoisted(() => {
  return { invoke_mock: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invoke_mock,
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  vi.clearAllMocks();
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "scan_laptop_cmd") return Promise.resolve(MOCK_SCAN_REPORT);
    return Promise.reject(new Error(`Unknown command: ${cmd}`));
  });
});

describe("StepScan.svelte", () => {
  it("(a) shows loading state before scan resolves", () => {
    mockInvoke.mockImplementation(() => new Promise(() => {}));
    render(StepScan, { props: { on_next: () => {} } });
    expect(screen.getByTestId("scan-loading")).toBeTruthy();
  });

  it("(b) renders findings once scan resolves", async () => {
    render(StepScan, { props: { on_next: () => {} } });
    expect(await screen.findByTestId("scan-summary")).toBeTruthy();
    expect(screen.getByTestId("scan-findings")).toBeTruthy();
    expect(screen.getByTestId("finding-github")).toBeTruthy();
    expect(screen.getByTestId("finding-claude_sessions")).toBeTruthy();
    expect(screen.getByTestId("finding-calendar")).toBeTruthy();
  });

  it("(c) shows ticking countdown with Stop + Continue now buttons", async () => {
    render(StepScan, { props: { on_next: () => {} } });
    await screen.findByTestId("scan-findings");
    const countdown = screen.getByTestId("scan-countdown");
    expect(countdown.textContent).toMatch(/Auto-advancing in 10s/);
    expect(screen.getByTestId("scan-stop-countdown")).toBeTruthy();
    expect(screen.getByTestId("scan-continue-now")).toBeTruthy();
  });

  it("(d) auto-advances when countdown reaches zero", async () => {
    vi.useFakeTimers();
    try {
      const on_next = vi.fn();
      render(StepScan, { props: { on_next } });
      await Promise.resolve();
      await vi.runOnlyPendingTimersAsync();
      for (let i = 0; i < 10; i++) {
        await vi.advanceTimersByTimeAsync(1000);
      }
      expect(on_next).toHaveBeenCalledWith(MOCK_SCAN_REPORT);
    } finally {
      vi.useRealTimers();
    }
  });

  it("(e) Stop countdown pauses the timer and shows only Continue now", async () => {
    vi.useFakeTimers();
    try {
      const on_next = vi.fn();
      render(StepScan, { props: { on_next } });
      await Promise.resolve();
      await vi.runOnlyPendingTimersAsync();
      await screen.findByTestId("scan-findings");
      await fireEvent.click(screen.getByTestId("scan-stop-countdown"));
      // The Stop button is gone. The only forward control is
      // Continue now — there is NO Resume button by design
      // (the user must explicitly advance after pausing).
      expect(screen.queryByTestId("scan-stop-countdown")).toBeNull();
      expect(screen.queryByTestId("scan-resume-countdown")).toBeNull();
      expect(screen.getByTestId("scan-continue-now")).toBeTruthy();
      expect(screen.getByTestId("scan-countdown").textContent).toMatch(
        /Auto-advance paused/,
      );
      // Advance timers past the original window — no auto-advance.
      for (let i = 0; i < 20; i++) {
        await vi.advanceTimersByTimeAsync(1000);
      }
      expect(on_next).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("(f) Continue now advances immediately after Stop", async () => {
    vi.useFakeTimers();
    try {
      const on_next = vi.fn();
      render(StepScan, { props: { on_next } });
      await Promise.resolve();
      await vi.runOnlyPendingTimersAsync();
      await screen.findByTestId("scan-findings");
      // Stop, then Continue now.
      await fireEvent.click(screen.getByTestId("scan-stop-countdown"));
      await fireEvent.click(screen.getByTestId("scan-continue-now"));
      expect(on_next).toHaveBeenCalledWith(MOCK_SCAN_REPORT);
      // Advance past the original window — no double-fire.
      for (let i = 0; i < 15; i++) {
        await vi.advanceTimersByTimeAsync(1000);
      }
      expect(on_next).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("(g) Continue now advances immediately and prevents later timer fire", async () => {
    vi.useFakeTimers();
    try {
      const on_next = vi.fn();
      render(StepScan, { props: { on_next } });
      await Promise.resolve();
      await vi.runOnlyPendingTimersAsync();
      await screen.findByTestId("scan-findings");
      await fireEvent.click(screen.getByTestId("scan-continue-now"));
      expect(on_next).toHaveBeenCalledTimes(1);
      for (let i = 0; i < 15; i++) {
        await vi.advanceTimersByTimeAsync(1000);
      }
      expect(on_next).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("(h) shows error state when scan_laptop_cmd rejects", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "scan_laptop_cmd") return Promise.reject(new Error("disk full"));
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepScan, { props: { on_next: () => {} } });
    expect(await screen.findByTestId("scan-error")).toBeTruthy();
    expect(screen.getByTestId("scan-retry")).toBeTruthy();
  });

  it("(i) Retry button re-fires scan_laptop_cmd after failure", async () => {
    let attempt = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd !== "scan_laptop_cmd")
        return Promise.reject(new Error(`Unknown command: ${cmd}`));
      attempt += 1;
      if (attempt === 1) return Promise.reject(new Error("first failed"));
      return Promise.resolve(MOCK_SCAN_REPORT);
    });
    render(StepScan, { props: { on_next: () => {} } });
    const retry = await screen.findByTestId("scan-retry");
    await fireEvent.click(retry);
    expect(await screen.findByTestId("scan-summary")).toBeTruthy();
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });
});