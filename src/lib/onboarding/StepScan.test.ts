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

  it("(c) auto-advances on scan completion via the on_next prop", async () => {
    const on_next = vi.fn();
    render(StepScan, { props: { on_next } });
    // The component auto-advances after 800ms; wait > 1s.
    await waitFor(
      () => {
        expect(on_next).toHaveBeenCalledWith(MOCK_SCAN_REPORT);
      },
      { timeout: 2000 },
    );
  });

  it("(d) shows the error state when scan_laptop_cmd rejects", async () => {
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

  it("(e) Retry button re-fires scan_laptop_cmd after a failure", async () => {
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
