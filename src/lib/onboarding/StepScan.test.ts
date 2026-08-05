import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import StepScan from "./StepScan.svelte";
import { MOCK_SCAN_REPORT } from "./types";

/**
 * Mock the @tauri-apps/api/core module the same way Greet.test.ts
 * does. We use `vi.hoisted` so the mock factory can reference the
 * per-test variable we set in beforeEach.
 */
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
  it("(a) shows the loading state before scan_laptop_cmd resolves", () => {
    // Indefinite pending promise so we can assert the loading
    // state without racing the auto-advance.
    mockInvoke.mockImplementation(
      () => new Promise(() => {}),
    );
    render(StepScan, { props: { on_next: () => {} } });
    expect(screen.getByTestId("scan-loading")).toBeTruthy();
  });

  it("(b) renders the findings list once the scan resolves", async () => {
    render(StepScan, { props: { on_next: () => {} } });
    expect(await screen.findByTestId("scan-summary")).toBeTruthy();
    expect(screen.getByTestId("scan-findings")).toBeTruthy();
    // Each mock candidate has a per-collector data-testid.
    expect(screen.getByTestId("finding-github")).toBeTruthy();
    expect(screen.getByTestId("finding-claude_sessions")).toBeTruthy();
    expect(screen.getByTestId("finding-calendar")).toBeTruthy();
  });

  it("(c) shows a visible auto-advance countdown after the scan resolves", async () => {
    render(StepScan, { props: { on_next: () => {} } });
    // Wait for the findings list to render, which means the scan
    // resolved and the countdown started.
    await screen.findByTestId("scan-findings");
    const countdown = screen.getByTestId("scan-countdown");
    expect(countdown).toBeTruthy();
    expect(countdown.textContent).toMatch(/Auto-advancing in \d+s/);
    // The initial value should be 10 (AUTO_ADVANCE_SECONDS).
    expect(countdown.textContent).toMatch(/Auto-advancing in 10s/);
    // The "Continue now" button should also be visible.
    expect(screen.getByTestId("scan-continue-now")).toBeTruthy();
  });

  it("(d) auto-advances after the countdown reaches zero", async () => {
    vi.useFakeTimers();
    try {
      const on_next = vi.fn();
      render(StepScan, { props: { on_next } });
      // Wait for the scan to resolve under fake timers — the
      // promise microtask needs to flush before the setInterval
      // is registered.
      await Promise.resolve();
      await vi.runOnlyPendingTimersAsync();
      // Advance 10 seconds in 1-second steps so the countdown
      // ticks down and the interval callback fires for each step.
      for (let i = 0; i < 10; i++) {
        await vi.advanceTimersByTimeAsync(1000);
      }
      // After 10 ticks the next() call should have fired.
      expect(on_next).toHaveBeenCalledWith(MOCK_SCAN_REPORT);
    } finally {
      vi.useRealTimers();
    }
  });

  it("(e) 'Continue now' button advances immediately", async () => {
    const on_next = vi.fn();
    render(StepScan, { props: { on_next } });
    await screen.findByTestId("scan-findings");
    const continue_now = screen.getByTestId("scan-continue-now");
    await fireEvent.click(continue_now);
    expect(on_next).toHaveBeenCalledWith(MOCK_SCAN_REPORT);
  });

  it("(f) shows the error state when scan_laptop_cmd rejects", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "scan_laptop_cmd")
        return Promise.reject(new Error("disk full"));
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepScan, { props: { on_next: () => {} } });
    expect(await screen.findByTestId("scan-error")).toBeTruthy();
    // The retry button should be visible.
    expect(screen.getByTestId("scan-retry")).toBeTruthy();
  });

  it("(g) Retry button re-fires scan_laptop_cmd after a failure", async () => {
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

  it("(h) clicking Continue now prevents the auto-advance timer from firing later", async () => {
    vi.useFakeTimers();
    try {
      const on_next = vi.fn();
      render(StepScan, { props: { on_next } });
      await Promise.resolve();
      await vi.runOnlyPendingTimersAsync();
      // Click Continue now BEFORE the countdown reaches zero.
      await screen.findByTestId("scan-findings");
      await fireEvent.click(screen.getByTestId("scan-continue-now"));
      expect(on_next).toHaveBeenCalledTimes(1);
      // Now advance the timers past the original 10s window. If
      // the interval hadn't been cleared, it would fire again.
      for (let i = 0; i < 15; i++) {
        await vi.advanceTimersByTimeAsync(1000);
      }
      // Still exactly 1 call — no duplicate advance.
      expect(on_next).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});