import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import StepAsk from "./StepAsk.svelte";
import { MOCK_SCAN_REPORT, MOCK_ANSWERS } from "./types";

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
    if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
    return Promise.reject(new Error(`Unknown command: ${cmd}`));
  });
});

describe("StepAsk.svelte", () => {
  it("(a) shows the loading state before ask_onboarding_cmd resolves", () => {
    mockInvoke.mockImplementation(
      () => new Promise(() => {}),
    );
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    expect(screen.getByTestId("ask-loading")).toBeTruthy();
  });

  it("(b) renders the LLM's answers as a flat list", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    expect(await screen.findByTestId("ask-answers")).toBeTruthy();
    // MOCK_ANSWERS has github enabled + 0 repos.
    expect(screen.getByText(/enabled/)).toBeTruthy();
    // voice is null in MOCK_ANSWERS → renders "disabled"
    expect(screen.getAllByText(/disabled/).length).toBeGreaterThan(0);
  });

  it("(c) Looks good button calls on_next with the LLM's answers", async () => {
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next },
    });
    const next = await screen.findByTestId("ask-next");
    expect((next as HTMLButtonElement).disabled).toBe(false);
    await fireEvent.click(next);
    expect(on_next).toHaveBeenCalledWith(MOCK_ANSWERS);
  });

  it("(d) Edit toggle reveals the path-list textareas", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    const toggle = await screen.findByTestId("ask-toggle-edit");
    await fireEvent.click(toggle);
    expect(screen.getByTestId("ask-edits")).toBeTruthy();
    expect(screen.getByTestId("edit-claude-paths")).toBeTruthy();
    expect(screen.getByTestId("edit-github-repos")).toBeTruthy();
  });

  it("(e) shows the error state when ask_onboarding_cmd rejects", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "ask_onboarding_cmd")
        return Promise.reject(new Error("ollama down"));
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    expect(await screen.findByTestId("ask-error")).toBeTruthy();
  });

  it("(f) the Next button is disabled while the IPC is in flight", () => {
    mockInvoke.mockImplementation(
      () => new Promise(() => {}),
    );
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    // The next button isn't in the DOM while loading (the
    // `answers` is null). Once `answers` resolves, the button
    // appears AND is enabled. This guards against a regression
    // where a stale "looks good" button is rendered with a
    // null answer.
    expect(screen.queryByTestId("ask-next")).toBeNull();
  });
});
