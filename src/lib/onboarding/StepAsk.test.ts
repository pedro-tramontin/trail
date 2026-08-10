import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import StepAsk from "./StepAsk.svelte";
import { writable, type Writable } from "svelte/store";
import { MOCK_SCAN_REPORT, MOCK_ANSWERS } from "./types";
import type { OnboardingAnswers, StepAskState } from "./types";

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

/** Build a fresh, default-valued StepAskState store for the
 *  test cases. Mirrors the wizard root's initial state shape. */
function fresh_state(): Writable<StepAskState> {
  return writable({
    editing: false,
    edit_claude_paths: "",
    edit_github_repos: "",
    review_hhmm_local: "18:00",
    edit_voice_enabled: false,
    edit_voice_model: "base.en",
  });
}

describe("StepAsk.svelte", () => {
  it("(a) shows loading state before ask_onboarding_cmd resolves", () => {
    mockInvoke.mockImplementation(() => new Promise(() => {}));
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    expect(screen.getByTestId("ask-loading")).toBeTruthy();
  });

  it("(b) renders the LLM's answers as a flat list", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
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
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next },
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
    // PR #216 — the "Looks good" path (no Edit) must preserve
    // the LLM's `voice: null` default. Only flipping the
    // checkbox in Edit mode should produce a Some(VoiceConfig).
    expect(called_with.voice).toBeNull();
  });

  it("(c2) renders '18:00 your time (<tz>)' by default", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    const value = await screen.findByTestId("review-time-value");
    expect(value.textContent).toMatch(/18:00/);
    expect(value.textContent).toMatch(/your time/);
    const tz = screen.getByTestId("review-time-tz");
    expect(tz.textContent?.trim().length).toBeGreaterThan(2);
  });

  it("(d) Edit toggle reveals the path-list textareas inline in their rows", async () => {
    // UX regression for the "click Edit and the box resizes"
    // bug: textareas used to appear in a separate `.edits`
    // block below the answers list, which added new rows to
    // the layout. Now the textareas live inline in their
    // respective answer rows (right side, next to the label)
    // so flipping the Edit toggle only swaps the value-slot
    // content — never the row's outer dimensions.
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    const toggle = await screen.findByTestId("ask-toggle-edit");
    await fireEvent.click(toggle);
    // The textareas now live inside the answer rows, not in
    // a separate `.edits` block. They should still exist and
    // be reachable via their testids.
    expect(screen.getByTestId("edit-claude-paths")).toBeTruthy();
    expect(screen.getByTestId("edit-github-repos")).toBeTruthy();
    // The summary view also goes away when editing — the
    // disabled-state summary text is replaced by the input.
    expect(screen.queryByTestId("claude-sessions-summary")).toBeNull();
    expect(screen.queryByTestId("github-summary")).toBeNull();
  });

  it("(e) shows the error state when ask_onboarding_cmd rejects", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "ask_onboarding_cmd")
        return Promise.reject(new Error("ollama down"));
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    expect(await screen.findByTestId("ask-error")).toBeTruthy();
  });

  it("(f) the Next button is disabled while IPC is in flight", () => {
    mockInvoke.mockImplementation(() => new Promise(() => {}));
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    expect(screen.queryByTestId("ask-next")).toBeNull();
  });

  it("(g) disabled fields show a '?' tooltip with the LLM's reasoning", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
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
    // the LLM's reasoning, NOT the generic fallback. The
    // fallback substring signals the bug where the question_log
    // entry's evidence_refs didn't include the field_id, so
    // the UI couldn't find a matching entry.
    const calendar_title = why_calendar.getAttribute("title") ?? "";
    const voice_title = why_voice.getAttribute("title") ?? "";
    expect(calendar_title.length).toBeGreaterThan(0);
    expect(voice_title.length).toBeGreaterThan(0);
    expect(calendar_title).not.toMatch(/didn't log a reason/i);
    expect(voice_title).not.toMatch(/didn't log a reason/i);
    // Sanity: the LLM's reasoning must surface in the tooltip
    // — otherwise the test would pass against the fallback
    // message which has nothing to do with the actual answer.
    expect(calendar_title.toLowerCase()).toContain("calendar");
    expect(voice_title.toLowerCase()).toContain("voice");
    expect(screen.queryByTestId("why-github")).toBeNull();
    expect(screen.queryByTestId("why-claude_sessions")).toBeNull();
  });

  it("(h) Edit toggle reveals an HH:MM time picker for the review-time row", async () => {
    // UX regression: the review-time row used to have a
    // standalone "Change time" button that expanded a
    // picker block BELOW the summary text. The user wanted
    // clicking the master Edit toggle to also make the
    // review time editable, AND for the picker to take the
    // exact spot of the summary text (not a block below it,
    // which would push other rows down). Now the picker is
    // an <input type="time"> that replaces the summary in
    // the same row when `editing` is on.
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    // Pre-edit: summary is rendered, no input is present.
    expect(screen.getByTestId("review-time-value")).toBeTruthy();
    expect(screen.queryByTestId("review-time-input")).toBeNull();
    // Toggle edit.
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    // The picker is now a single <input type="time"> in the
    // row's value slot. Summary is gone.
    expect(screen.getByTestId("review-time-input")).toBeTruthy();
    expect(screen.queryByTestId("review-time-value")).toBeNull();
  });

  it("(i) editing the local HH:MM picker keeps the tz label visible in edit mode", async () => {
    // UX regression: the edit-mode hint USED to read
    // "Stored as NN:MM UTC (Berlin)" — per Anti-pattern D of
    // wizard-ux-patterns ("don't tell the user '18:00 UTC'"),
    // we drop the UTC conversion from the user-visible text.
    // The user picks their LOCAL time; we translate to UTC
    // internally on Next (covered by test (i2) below). What
    // remains visible in edit mode is just the city label so
    // the user can confirm the auto-detected timezone is the
    // one they're picking for.
    //
    // Specifically: there must NOT be a `review-time-utc`
    // testid in edit mode (we removed that DOM node), and the
    // existing `review-time-tz` testid must still render.
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    // No UTC hint element anywhere in the document. The
    // picker takes the summary's slot; the hint is just the
    // tz label.
    expect(screen.queryByTestId("review-time-utc")).toBeNull();
    // City label still present so the user can verify
    // timezone detection.
    const tz = screen.getByTestId("review-time-tz");
    expect(tz.textContent?.trim().length).toBeGreaterThan(2);
  });

  it("(i2) editing the local HH:MM stores the converted hour on Next", async () => {
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next },
    });
    await screen.findByTestId("ask-answers");
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    const input = screen.getByTestId(
      "review-time-input",
    ) as HTMLInputElement;
    await fireEvent.input(input, { target: { value: "09:00" } });
    // Untoggle edit so "Looks good" label is shown.
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    const next = screen.getByTestId("ask-next");
    await fireEvent.click(next);

    expect(on_next).toHaveBeenCalledTimes(1);
    const called_with = on_next.mock.calls[0][0] as OnboardingAnswers;
    const offset_minutes = new Date().getTimezoneOffset();
    const offset_hours = offset_minutes / 60;
    const expected = ((9 - offset_hours) + 24) % 24;
    expect(called_with.review_time.hour_utc).toBe(expected);
  });

  // PR #193 — back-navigation preserves edits.
  it("(j) the hoisted state edits persist when the parent store is updated", async () => {
    // Simulate the "Back" navigation: the parent wizard has
    // already populated the StepAskState store with the
    // user's previous edits (editing=true, edit_claude_paths
    // non-empty, review_hhmm_local = "20:30"). The fresh
    // mount of StepAsk should read those values from the
    // store and render them.
    const pre_populated = writable({
      editing: true,
      edit_claude_paths: "/Users/back-nav/.claude/projects",
      edit_github_repos: "pedro-tramontin/trail",
      review_hhmm_local: "20:30",
      edit_voice_enabled: true,
      edit_voice_model: "small.en",
    });
    render(StepAsk, {
      props: {
        scan: MOCK_SCAN_REPORT,
        initial_answers: null,
        state: pre_populated,
        on_next: () => {},
      },
    });
    await screen.findByTestId("ask-answers");
    // The Edit toggle should be ON (so the textareas render).
    expect(screen.getByTestId("edit-claude-paths")).toBeTruthy();
    expect(screen.getByTestId("edit-github-repos")).toBeTruthy();
    // The textareas should be pre-populated from the store.
    expect(
      (screen.getByTestId("edit-claude-paths") as HTMLTextAreaElement).value,
    ).toBe("/Users/back-nav/.claude/projects");
    expect(
      (screen.getByTestId("edit-github-repos") as HTMLTextAreaElement).value,
    ).toBe("pedro-tramontin/trail");
    // The review-time picker should be pre-populated.
    expect(
      (screen.getByTestId("review-time-input") as HTMLInputElement).value,
    ).toBe("20:30");
  });

  // PR #216 — voice-capture toggle.
  it("(k) edit mode shows a checkbox + model picker in the voice row", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    // Pre-edit: summary view, no checkbox.
    expect(screen.getByTestId("why-voice")).toBeTruthy();
    expect(screen.queryByTestId("voice-toggle")).toBeNull();
    // Flip Edit on.
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    // Checkbox + select are now visible, summary view is gone.
    const toggle = screen.getByTestId("voice-toggle") as HTMLInputElement;
    expect(toggle).toBeTruthy();
    expect((toggle as HTMLInputElement).checked).toBe(false); // default off
    const model = screen.getByTestId("voice-model") as HTMLSelectElement;
    expect(model).toBeTruthy();
    expect(model.value).toBe("base.en"); // wizard-root default
    // The "why disabled" tooltip is gone in edit mode (the
    // user is now driving the choice directly).
    expect(screen.queryByTestId("why-voice")).toBeNull();
  });

  it("(l) flipping the voice checkbox on, then Next, includes a VoiceConfig", async () => {
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next },
    });
    await screen.findByTestId("ask-answers");
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    // Toggle on.
    const toggle = screen.getByTestId("voice-toggle") as HTMLInputElement;
    await fireEvent.click(toggle);
    // Pick a non-default model so the test pins the binding, not
    // just the default.
    const model = screen.getByTestId("voice-model") as HTMLSelectElement;
    await fireEvent.change(model, { target: { value: "small.en" } });
    // Click Next.
    const next = screen.getByTestId("ask-next");
    await fireEvent.click(next);

    expect(on_next).toHaveBeenCalledTimes(1);
    const called_with = on_next.mock.calls[0][0] as OnboardingAnswers;
    expect(called_with.voice).not.toBeNull();
    expect(called_with.voice).toMatchObject({
      enabled: true,
      model: "small.en",
      language: "en",
    });
  });

  it("(m) leaving the voice checkbox off, then Next, keeps voice=null (LLM-disabled preserved)", async () => {
    // Regression guard for the "Looks good" path: not flipping
    // the checkbox must NOT introduce a Some(VoiceConfig) into
    // the answers, even when Edit mode was entered. The user
    // needs to opt in explicitly.
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next },
    });
    await screen.findByTestId("ask-answers");
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    // Touch the model picker but leave the checkbox at its
    // default `false`. This catches a future regression where
    // editing the picker accidentally also flips the enable
    // bit.
    const model = screen.getByTestId("voice-model") as HTMLSelectElement;
    await fireEvent.change(model, { target: { value: "small.en" } });
    const next = screen.getByTestId("ask-next");
    await fireEvent.click(next);

    expect(on_next).toHaveBeenCalledTimes(1);
    const called_with = on_next.mock.calls[0][0] as OnboardingAnswers;
    expect(called_with.voice).toBeNull();
  });
});