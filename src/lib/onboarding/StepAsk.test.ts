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
  it("(a) shows loading state before ask_onboarding_cmd resolves", () => {
    mockInvoke.mockImplementation(() => new Promise(() => {}));
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
    expect(screen.getByText(/enabled/)).toBeTruthy();
    expect(screen.getAllByText(/disabled/).length).toBeGreaterThan(0);
  });

  it("(c) Looks good button calls on_next with hour_utc adjusted to local 18:00", async () => {
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
    expect(called_with.claude_sessions_paths).toEqual(
      MOCK_ANSWERS.claude_sessions_paths,
    );
    expect(called_with.github).toEqual(MOCK_ANSWERS.github);
  });

  it("(c2) renders '18:00 your time (<tz>)' by default", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    const value = await screen.findByTestId("review-time-value");
    expect(value.textContent).toMatch(/18:00/);
    expect(value.textContent).toMatch(/your time/);
    const tz = screen.getByTestId("review-time-tz");
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

  it("(f) the Next button is disabled while IPC is in flight", () => {
    mockInvoke.mockImplementation(() => new Promise(() => {}));
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    expect(screen.queryByTestId("ask-next")).toBeNull();
  });

  it("(g) disabled fields show a '?' tooltip with the LLM's reasoning", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    // MOCK_ANSWERS has calendar_ics = null and voice = null →
    // both should render with a tooltip button. github is
    // enabled, so no 'why-github' button. claude_sessions has
    // one path so it's enabled, no 'why-claude_sessions'.
    const why_calendar = screen.getByTestId("why-calendar");
    const why_voice = screen.getByTestId("why-voice");
    expect(why_calendar).toBeTruthy();
    expect(why_voice).toBeTruthy();
    // Tooltip text comes from question_log — verify both have
    // a non-empty title that includes a "?" / reasoning.
    expect(why_calendar.getAttribute("title")).toBeTruthy();
    expect(why_calendar.getAttribute("title")?.length).toBeGreaterThan(0);
    expect(why_voice.getAttribute("title")).toBeTruthy();
    expect(why_voice.getAttribute("title")?.length).toBeGreaterThan(0);
    expect(screen.queryByTestId("why-github")).toBeNull();
    expect(screen.queryByTestId("why-claude_sessions")).toBeNull();
  });

  it("(h) 'Change time' opens an hour picker; user input updates the local hour", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    const change_btn = screen.getByTestId("review-time-edit");
    await fireEvent.click(change_btn);
    expect(screen.getByTestId("review-time-picker")).toBeTruthy();
    const input = screen.getByTestId(
      "review-hour-input",
    ) as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "9" } });
    // The summary line reflects the new hour.
    const hour = screen.getByTestId("review-time-hour");
    expect(hour.textContent).toMatch(/09:00/);
  });

  it("(i) editing the local hour updates the stored hour_utc on Next", async () => {
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next },
    });
    await screen.findByTestId("ask-answers");
    await fireEvent.click(screen.getByTestId("review-time-edit"));
    const input = screen.getByTestId(
      "review-hour-input",
    ) as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "9" } });
    const next = screen.getByTestId("ask-next");
    await fireEvent.click(next);

    expect(on_next).toHaveBeenCalledTimes(1);
    const called_with = on_next.mock.calls[0][0] as OnboardingAnswers;
    // expected = (9 - offset_hours + 24) % 24
    const offset_minutes = new Date().getTimezoneOffset();
    const offset_hours = offset_minutes / 60;
    const expected = ((9 - offset_hours) + 24) % 24;
    expect(called_with.review_time.hour_utc).toBe(expected);
  });

  it("(j) the picker shows the current UTC equivalent live as the user types", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    await fireEvent.click(screen.getByTestId("review-time-edit"));
    const input = screen.getByTestId(
      "review-hour-input",
    ) as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "7" } });
    const picker = screen.getByTestId("review-time-picker");
    const offset_minutes = new Date().getTimezoneOffset();
    const offset_hours = offset_minutes / 60;
    const expected_utc = ((7 - offset_hours) + 24) % 24;
    expect(picker.textContent).toContain(
      `Stored as ${expected_utc}:00 UTC`,
    );
  });
});