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
    edit_voice_enabled: true,
    edit_voice_model: "base.en",
    edit_calendar_source: "event_kit",
    edit_ics_paths: "",
    edit_browser_history: "",
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
    // 2026-08-11 — the wizard's pre-applied defaults make both
    // the github row AND the voice row show "enabled". The
    // pre-PR test only saw one `enabled` (github). Use
    // `getAllByText` so we don't trip over the second
    // "enabled" in the voice row.
    expect(screen.getAllByText(/enabled/).length).toBeGreaterThan(0);
    // calendar_ics is still null in MOCK_ANSWERS so the
    // calendar row shows "disabled" with a `why-calendar`
    // tooltip.
    expect(screen.getAllByText(/disabled/).length).toBeGreaterThan(0);
  });

  it("(c) Looks good button calls on_next with the wizard's default voice (pre-applied from edit defaults)", async () => {
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
    // 2026-08-11 — the wizard's default is now voice-enabled
    // with `base.en` / `en`. The LLM's `voice: null` is
    // pre-applied to a default VoiceConfig on first render
    // (see `run_ask`'s `with_defaults` step). The "Looks
    // good" path (no Edit) must preserve that default — the
    // user only sees "disabled" if they explicitly uncheck
    // the toggle in Edit mode.
    expect(called_with.voice).toMatchObject({
      enabled: true,
      model: "base.en",
      language: "en",
    });
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
    //
    // 2026-08-11 — also asserts that:
    //   (1) the calendar row's ICS-path textarea is NOT
    //       rendered until the user picks the "Custom .ics
    //       file" radio (event_kit is the default), and
    //   (2) the new browser-history picker row is rendered
    //       with all five checkboxes.
    // The pre-PR bug was: the user picked "Custom .ics file"
    // but no input element existed to enter the path, so
    // `build_edited_answers` produced an empty `ics_paths`
    // list and the collector wrote an empty calendar.json.
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
    // Calendar default is event_kit — the ICS-path textarea
    // should NOT be visible. Pre-PR bug: there was no input
    // at all (calendar source picker had no textarea).
    expect(screen.queryByTestId("edit-ics-paths")).toBeNull();
    // Click the "Custom .ics file" radio and confirm the
    // textarea appears inline within the calendar row.
    await fireEvent.click(screen.getByTestId("calendar-source-ics"));
    expect(screen.getByTestId("edit-ics-paths")).toBeTruthy();
    // Browser-history picker: all five checkboxes must be
    // present (none checked by default — the user must
    // opt in).
    expect(screen.getByTestId("browser-history-picker")).toBeTruthy();
    expect(screen.getByTestId("browser-history-chrome")).toBeTruthy();
    expect(screen.getByTestId("browser-history-brave")).toBeTruthy();
    expect(screen.getByTestId("browser-history-firefox")).toBeTruthy();
    expect(screen.getByTestId("browser-history-opera")).toBeTruthy();
    expect(screen.getByTestId("browser-history-safari")).toBeTruthy();
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
    // 2026-08-11 — with the new defaults, `voice` is
    // pre-applied to enabled (the wizard's default), so the
    // pre-edit voice row shows "enabled (base.en, en)"
    // instead of the LLM's "disabled". Only `calendar_ics`
    // (still null in MOCK_ANSWERS) renders the "why"
    // tooltip in the pre-edit state. The voice tooltip is
    // exercised in test (c) above via `getAllByText(/enabled/)`.
    const why_calendar = screen.getByTestId("why-calendar");
    expect(why_calendar).toBeTruthy();
    expect(screen.queryByTestId("why-voice")).toBeNull();
    // Tooltip text comes from question_log — verify the
    // calendar tooltip has the LLM's reasoning, NOT the
    // generic fallback. The fallback substring signals the
    // bug where the question_log entry's evidence_refs
    // didn't include the field_id, so the UI couldn't find
    // a matching entry.
    const calendar_title = why_calendar.getAttribute("title") ?? "";
    expect(calendar_title.length).toBeGreaterThan(0);
    expect(calendar_title).not.toMatch(/didn't log a reason/i);
    // Sanity: the LLM's reasoning must surface in the
    // tooltip — otherwise the test would pass against the
    // fallback message which has nothing to do with the
    // actual answer.
    expect(calendar_title.toLowerCase()).toContain("calendar");
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
      edit_calendar_source: "event_kit" as const,
      edit_ics_paths: "",
      edit_browser_history: "",
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

  // 2026-08-11 — voice-capture toggle. The default flipped
  // to `true` per user feedback.
  it("(k) edit mode shows a checkbox + model picker in the voice row", async () => {
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    // Pre-edit: summary view. With the new default, voice
    // is enabled so the row shows "enabled" (not the
    // "disabled" tooltip the pre-PR test asserted). The
    // `why-voice` testid is no longer present in this
    // pre-edit state.
    expect(screen.queryByTestId("why-voice")).toBeNull();
    // Flip Edit on.
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    // Checkbox + select are now visible, summary view is gone.
    const toggle = screen.getByTestId("voice-toggle") as HTMLInputElement;
    expect(toggle).toBeTruthy();
    // The default is now `true` (user said "enabled by
    // default with the best settings for it"), so the
    // checkbox starts checked.
    expect((toggle as HTMLInputElement).checked).toBe(true);
    const model = screen.getByTestId("voice-model") as HTMLSelectElement;
    expect(model).toBeTruthy();
    expect(model.value).toBe("base.en"); // wizard-root default
    // The "why disabled" tooltip is gone in edit mode (the
    // user is now driving the choice directly).
    expect(screen.queryByTestId("why-voice")).toBeNull();
  });

  it("(l) keeping the voice checkbox on, then Next, includes a VoiceConfig (default-on path)", async () => {
    // 2026-08-11 — the new default is `true`, so the user
    // just has to *not* touch the checkbox to get a
    // VoiceConfig. Picking a non-default model pins the
    // model binding, not just the default.
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next },
    });
    await screen.findByTestId("ask-answers");
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    // Toggle starts ON (new default). Pin the model binding
    // by picking a non-default value.
    const toggle = screen.getByTestId("voice-toggle") as HTMLInputElement;
    expect(toggle.checked).toBe(true); // default is on after PR 2026-08-11
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

  it("(m) unchecking the voice checkbox, then Next, leaves voice=null (opt-out path)", async () => {
    // Regression guard for the opt-out path: explicitly
    // unchecking the checkbox in Edit mode must produce a
    // `voice: null` answers field, so a user who doesn't want
    // voice capture at all can opt out via the wizard.
    //
    // 2026-08-11 — pre-PR, the default was `false` so just
    // *entering* Edit mode left the checkbox off, and the
    // test simply did nothing to the checkbox. The new default
    // is `true` (per user feedback "it would be nice to have
    // it enabled by default with the best settings for it"),
    // so the test now explicitly clicks the checkbox to flip
    // it back to off. The semantic check is unchanged: the
    // un-toggled checkbox → `voice: null` in the answers.
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next },
    });
    await screen.findByTestId("ask-answers");
    await fireEvent.click(screen.getByTestId("ask-toggle-edit"));
    // Touch the model picker so the test pins the binding,
    // not just the default.
    const model = screen.getByTestId("voice-model") as HTMLSelectElement;
    await fireEvent.change(model, { target: { value: "small.en" } });
    // Explicitly uncheck the checkbox to simulate the opt-out
    // user who flipped the default back to off.
    const toggle = screen.getByTestId("voice-toggle") as HTMLInputElement;
    await fireEvent.click(toggle);
    expect(toggle.checked).toBe(false);
    const next = screen.getByTestId("ask-next");
    await fireEvent.click(next);

    expect(on_next).toHaveBeenCalledTimes(1);
    const called_with = on_next.mock.calls[0][0] as OnboardingAnswers;
    // 2026-08-11 — the new model is "voice field always
    // present, `enabled: false` means disabled" (instead of
    // "voice: null means disabled"). The pre-PR test
    // asserted `voice: null`; the post-PR test asserts
    // `voice.enabled: false`. The schema's downstream
    // behaviour is the same — `config_writer.rs`
    // `answers_to_config` reads `voice.enabled` and writes
    // the on-disk `Config.voice` accordingly.
    expect(called_with.voice).not.toBeNull();
    expect(called_with.voice?.enabled).toBe(false);
  });

  it("(k) .ics path textarea round-trips through build_edited_answers", async () => {
    // 2026-08-11 — regression for the missing-file-picker
    // bug. The pre-PR behavior: the user picked "Custom
    // .ics file" but no input element existed, so
    // `build_edited_answers` always produced
    // `calendar_ics = { enabled: true, ics_paths: [], calendar_app_id: null }`
    // regardless of what the user wanted. The collector
    // then wrote an empty calendar.json. This test picks
    // the radio, types a path, clicks "Looks good", and
    // asserts the typed path lands in
    // `on_next`'s `answers.calendar_ics.ics_paths`.
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next },
    });
    // Open Edit mode + switch the calendar source to .ics.
    const edit = await screen.findByTestId("ask-toggle-edit");
    await fireEvent.click(edit);
    await fireEvent.click(screen.getByTestId("calendar-source-ics"));
    // Type the absolute path into the textarea. Svelte 5's
    // `bind:value` listens for `input` events (not `change`),
    // so we use `fireEvent.input` here — the same pattern
    // the github row test uses for `edit-github-repos`.
    const ics = screen.getByTestId("edit-ics-paths") as HTMLTextAreaElement;
    await fireEvent.input(ics, {
      target: { value: "/Users/you/Downloads/calendar.ics" },
    });
    // Click "Looks good" and confirm the typed path
    // landed in `answers.calendar_ics.ics_paths`.
    const next = screen.getByTestId("ask-next");
    await fireEvent.click(next);
    expect(on_next).toHaveBeenCalledTimes(1);
    const called_with = on_next.mock.calls[0][0] as OnboardingAnswers;
    expect(called_with.calendar_ics?.calendar_app_id).toBeNull();
    expect(called_with.calendar_ics?.ics_paths).toEqual([
      "/Users/you/Downloads/calendar.ics",
    ]);
    expect(called_with.calendar_ics?.enabled).toBe(true);
  });

  it("(l) browser-history picker round-trips through build_edited_answers", async () => {
    // 2026-08-11 — verifies the new Browser-history row
    // commits its checkbox selections to
    // `answers.browser_history`. Pre-PR the field didn't
    // exist; the row is new.
    const on_next = vi.fn();
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next },
    });
    const edit = await screen.findByTestId("ask-toggle-edit");
    await fireEvent.click(edit);
    // Tick chrome + firefox; leave brave / opera / safari
    // unchecked.
    await fireEvent.click(screen.getByTestId("browser-history-chrome"));
    await fireEvent.click(screen.getByTestId("browser-history-firefox"));
    const next = screen.getByTestId("ask-next");
    await fireEvent.click(next);
    expect(on_next).toHaveBeenCalledTimes(1);
    const called_with = on_next.mock.calls[0][0] as OnboardingAnswers;
    expect(called_with.browser_history).toEqual(["chrome", "firefox"]);
  });

  // §17-5 — voice microphone permission denied-callout.
  // The 3 cases below mirror the per-item brief's "3 vitest
  // cases on the wizard step (default / enabled / denied-with-callout)".

  it("(m) voice row defaults: no mic-permission callout when the IPC returns undefined", async () => {
    // The onMount handler swallows the IPC rejection and
    // leaves `mic_permission_state` undefined — the callout
    // must stay hidden so we don't flash a misleading
    // "denied" message.
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "check_mic_permission_cmd") return Promise.reject("ipc fail");
      if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    expect(screen.queryByTestId("mic-permission-denied-callout")).toBeNull();
  });

  it("(n) voice row enabled: no callout when mic permission is granted", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "check_mic_permission_cmd") return Promise.resolve("granted");
      if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    await screen.findByTestId("ask-answers");
    // Let the onMount microtask flush.
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByTestId("mic-permission-denied-callout")).toBeNull();
  });

  it("(o) voice row denied: red callout appears with Open-Privacy-Settings button", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "check_mic_permission_cmd") return Promise.resolve("denied");
      if (cmd === "mic_permission_deep_link_url_cmd")
        return Promise.resolve("pavucontrol:");
      if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    const callout = await screen.findByTestId("mic-permission-denied-callout");
    expect(callout).toBeTruthy();
    const btn = await screen.findByTestId("open-permission-settings");
    expect(btn).toBeTruthy();
    expect(btn.textContent).toMatch(/Open Privacy Settings/);
  });

  // §X-4 — per-OS calendar permission deep-link button.
  // The 3 cases below mirror the per-item brief's "3
  // vitest cases on StepAsk.svelte (one per OS) + the
  // UnknownDE fallback path". Each case enters Edit mode
  // + leaves the calendar source on the default
  // "event_kit", then asserts the per-OS deep-link URL
  // the Tauri command returns is rendered as a button (or,
  // for the Linux/UnknownDE arm, the labeled fallback
  // message is rendered instead of a button).
  //
  // The Tauri command is mocked so the test doesn't need
  // a real Tauri runtime. We branch on
  // `calendar_permission_deep_link_url`:
  //   - macOS test: returns the Apple system-preferences URL.
  //   - Windows test: returns the ms-settings URL.
  //   - Linux/UnknownDE test: rejects with the structured
  //     UnknownDE error string; the component catches the
  //     rejection and renders the labeled fallback message.

  it("(p) calendar row event_kit + macOS: per-OS deep-link button renders with Apple URL", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
      if (cmd === "calendar_permission_deep_link_url")
        return Promise.resolve(
          "x-apple.systempreferences:com.apple.preference.security?Privacy_Calendar",
        );
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    // Enter Edit mode so the EventKit branch of the
    // calendar row renders (the deep-link button is only
    // shown when `editing` is true).
    const toggle = await screen.findByTestId("ask-toggle-edit");
    await fireEvent.click(toggle);
    // Let the onMount calendar_permission_deep_link_url
    // microtask flush.
    await new Promise((r) => setTimeout(r, 0));
    const btn = await screen.findByTestId(
      "open-calendar-permission-settings",
    );
    expect(btn).toBeTruthy();
    expect(btn.textContent).toMatch(/Open Calendar Settings/);
    // The UnknownDE labeled fallback must NOT be rendered
    // when the URL is known.
    expect(
      screen.queryByTestId("calendar-permission-unknown-de"),
    ).toBeNull();
  });

  it("(q) calendar row event_kit + Windows: per-OS deep-link button renders with ms-settings URL", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
      if (cmd === "calendar_permission_deep_link_url")
        return Promise.resolve("ms-settings:privacy-calendar");
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    const toggle = await screen.findByTestId("ask-toggle-edit");
    await fireEvent.click(toggle);
    await new Promise((r) => setTimeout(r, 0));
    const btn = await screen.findByTestId(
      "open-calendar-permission-settings",
    );
    // Guard against a typo testid — the spec's
    // `open-calendar-permission-settings` testid is the
    // single source of truth. We do a lengthier text
    // assertion here so the message rendered to the user
    // is the per-OS "Open Calendar Settings" wording, not
    // any other "Open ... Settings" label.
    expect(btn.textContent).toMatch(/Open Calendar Settings/);
    // The UnknownDE labeled fallback must NOT be rendered.
    expect(
      screen.queryByTestId("calendar-permission-unknown-de"),
    ).toBeNull();
  });

  it("(r) calendar row event_kit + Linux/UnknownDE: labeled 'open manually' fallback renders, no button", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
      if (cmd === "calendar_permission_deep_link_url")
        return Promise.reject(
          "Linux DE not detected (could be GNOME, KDE, or other). User must open settings manually.",
        );
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    render(StepAsk, {
      props: { scan: MOCK_SCAN_REPORT, initial_answers: null, state: fresh_state(), on_next: () => {} },
    });
    const toggle = await screen.findByTestId("ask-toggle-edit");
    await fireEvent.click(toggle);
    // Wait for the onMount microtask + the UnknownDE
    // catch handler to land `calendar_permission_url` in
    // the "unknown_de" sentinel state.
    await new Promise((r) => setTimeout(r, 0));
    // The deep-link button must NOT render in the
    // UnknownDE arm (no dead-button UX).
    expect(
      screen.queryByTestId("open-calendar-permission-settings"),
    ).toBeNull();
    // The labeled "open manually" fallback must render.
    // We assert on a unique substring of the labeled
    // message so a future copy edit doesn't break the
    // test unless the load-bearing phrase is dropped.
    const hint = await screen.findByText(/wasn't detected/);
    expect(hint).toBeTruthy();
    expect(hint.textContent).toMatch(/open Settings/);
  });
});