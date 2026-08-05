import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import StepAsk from "./StepAsk.svelte";
import { MOCK_SCAN_REPORT, MOCK_ANSWERS } from "./types";
import type { OnboardingAnswers } from "./types";

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

  it("(c) Looks good button calls on_next with the answers (hour_utc adjusted to local 18:00)", async () => {
    // The wizard translates the LLM's `hour_utc` into the UTC
    // hour that represents 18:00 in the user's local timezone
    // before sending the answers on. This means the stored
    // `hour_utc` depends on the test environment's timezone,
    // not on the LLM's default. We compute the expected value
    // the same way `local_hour_to_utc` does inside StepAsk.
    const offset_minutes = new Date().getTimezoneOffset();
    const offset_hours = offset_minutes / 60;
    const expected_hour_utc = ((18 - offset_hours) + 24) % 24;

    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next },
    });
    const next = await screen.findByTestId("ask-next");
    expect((next as HTMLButtonElement).disabled).toBe(false);
    await fireEvent.click(next);

    expect(on_next).toHaveBeenCalledTimes(1);
    const called_with = on_next.mock.calls[0][0] as OnboardingAnswers;
    expect(called_with.review_time.hour_utc).toBe(expected_hour_utc);
    // The other fields pass through unchanged.
    expect(called_with.claude_sessions_paths).toEqual(
      MOCK_ANSWERS.claude_sessions_paths,
    );
    expect(called_with.github).toEqual(MOCK_ANSWERS.github);
  });

  it("(c2) renders the review-time row as '18:00 your time' with the IANA timezone", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    const value = await screen.findByTestId("review-time-value");
    // The local hour is always 18:00 regardless of timezone.
    expect(value.textContent).toMatch(/18:00/);
    // The "your time" wording makes it clear this is local, not UTC.
    expect(value.textContent).toMatch(/your time/);
    // The IANA timezone abbreviation is shown so the user can
    // sanity-check the auto-detection (e.g. "Sao Paulo",
    // "Lisbon", "UTC").
    const tz = screen.getByTestId("review-time-tz");
    expect(tz).toBeTruthy();
    // The tz label is non-empty (either "UTC" or a city name).
    expect(tz.textContent?.trim().length).toBeGreaterThan(2);
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