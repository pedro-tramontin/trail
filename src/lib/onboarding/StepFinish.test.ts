import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";
import StepFinish from "./StepFinish.svelte";
import { MOCK_ANSWERS } from "./types";

const { invoke_mock } = vi.hoisted(() => {
  return { invoke_mock: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invoke_mock,
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

const WRITTEN_PATH = "/Users/test/.trail/config.json";

beforeEach(() => {
  vi.clearAllMocks();
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "write_onboarding_config")
      return Promise.resolve(WRITTEN_PATH);
    return Promise.reject(new Error(`Unknown command: ${cmd}`));
  });
});

describe("StepFinish.svelte", () => {
  it("(a) calls write_onboarding_config on mount with the answers + ssh_key_generated", async () => {
    const on_complete = vi.fn();
    render(StepFinish, {
      props: { answers: MOCK_ANSWERS, ssh_key_generated: true, on_complete },
    });
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "write_onboarding_config",
        expect.objectContaining({
          answers: MOCK_ANSWERS,
          sshKeyGenerated: true,
        }),
      );
    });
  });

  it("(b) shows the success message and calls on_complete with the path", async () => {
    const on_complete = vi.fn();
    render(StepFinish, {
      props: { answers: MOCK_ANSWERS, on_complete },
    });
    expect(await screen.findByTestId("finish-success")).toBeTruthy();
    // The success text should mention the path the mock returned.
    expect(screen.getByText(WRITTEN_PATH)).toBeTruthy();
    // on_complete fires after a 600ms delay — wait up to 3s to
    // allow for jsdom's timer slop.
    await waitFor(
      () => {
        expect(on_complete).toHaveBeenCalledWith(WRITTEN_PATH);
      },
      { timeout: 3000, interval: 50 },
    );
  });

  it("(c) shows the error state when write_onboarding_config rejects", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "write_onboarding_config")
        return Promise.reject(new Error("disk full"));
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    const on_complete = vi.fn();
    render(StepFinish, {
      props: { answers: MOCK_ANSWERS, on_complete },
    });
    expect(await screen.findByTestId("finish-error")).toBeTruthy();
    // on_complete should NOT have been called.
    expect(on_complete).not.toHaveBeenCalled();
  });

  it("(d) Retry button re-fires write_onboarding_config after a failure", async () => {
    let attempt = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd !== "write_onboarding_config")
        return Promise.reject(new Error(`Unknown command: ${cmd}`));
      attempt += 1;
      if (attempt === 1) return Promise.reject(new Error("first failed"));
      return Promise.resolve(WRITTEN_PATH);
    });
    const on_complete = vi.fn();
    render(StepFinish, {
      props: { answers: MOCK_ANSWERS, on_complete },
    });
    const retry = await screen.findByTestId("finish-retry");
    retry.click();
    expect(await screen.findByTestId("finish-success")).toBeTruthy();
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });
});
