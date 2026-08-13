import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import Settings from "./Settings.svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  vi.clearAllMocks();
  // Default mock: the §17-5 voice permission row's onMount
  // calls `check_mic_permission_cmd` + `mic_permission_deep_link_url_cmd`
  // (both Promise<string>); the existing Settings tests don't
  // care about those so we resolve them to safe defaults. Tests
  // that need specific behaviour re-`mockImplementation` the
  // relevant command. jsdom defaults `window.confirm` to
  // `false`; tests opt in explicitly via
  // `vi.spyOn(window, "confirm").mockReturnValue(true)`.
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "check_mic_permission_cmd")
      return Promise.resolve("granted");
    if (cmd === "mic_permission_deep_link_url_cmd")
      return Promise.resolve("pavucontrol:");
    return Promise.resolve(undefined);
  });
});

describe("Settings.svelte (placeholder shell)", () => {
  it("(a) renders the 'Re-run onboarding' button", () => {
    render(Settings, { props: { onreset: () => {} } });
    expect(screen.getByTestId("settings-shell")).toBeTruthy();
    expect(screen.getByTestId("rerun-onboarding")).toBeTruthy();
    // The button must carry the exact label we promise the user.
    expect(
      (screen.getByTestId("rerun-onboarding") as HTMLButtonElement)
        .textContent,
    ).toMatch(/Re-run onboarding/);
  });

  it("(b) clicking the button after a confirmed prompt fires the onreset callback", async () => {
    // The parent (App.svelte) is what actually calls
    // `invoke('delete_config')` from inside the onreset handler
    // — the Settings component is presentation-only. The test
    // asserts the prop fires + that the parent's handler would
    // then have issued the IPC call. We simulate the parent
    // by passing an onreset that invokes delete_config, mirroring
    // App.svelte's `reset_for_onboarding()`.
    //
    // The beforeEach has already wired the §17-5 onMount IPCs
    // to safe defaults; we leave the mock alone here so the
    // onMount chain still resolves.
    const onreset = vi.fn(async () => {
      await invoke("delete_config", {
        cmd: "/Users/test/.trail/config.json",
      });
    });

    const confirm_spy = vi
      .spyOn(window, "confirm")
      .mockReturnValue(true);

    render(Settings, { props: { onreset } });

    await fireEvent.click(screen.getByTestId("rerun-onboarding"));

    expect(confirm_spy).toHaveBeenCalledWith(
      "This will reset your Trail config. Continue?",
    );
    expect(onreset).toHaveBeenCalledOnce();
    // The parent handler (mirroring App.svelte) would have
    // fired delete_config exactly once with the config path.
    expect(mockInvoke).toHaveBeenCalledWith("delete_config", {
      cmd: "/Users/test/.trail/config.json",
    });

    confirm_spy.mockRestore();
  });

  it("(c) cancelling the confirm() dialog does NOT fire onreset (and does not call delete_config)", async () => {
    const onreset = vi.fn();
    const confirm_spy = vi
      .spyOn(window, "confirm")
      .mockReturnValue(false);

    render(Settings, { props: { onreset } });

    await fireEvent.click(screen.getByTestId("rerun-onboarding"));

    expect(confirm_spy).toHaveBeenCalledWith(
      "This will reset your Trail config. Continue?",
    );
    // No reset — the user backed out.
    expect(onreset).not.toHaveBeenCalled();
    // And therefore no `delete_config` IPC call (the
    // §17-5 voice permission onMount IPCs are the
    // beforeEach-default resolves, not user-driven calls).
    expect(mockInvoke).not.toHaveBeenCalledWith("delete_config", expect.anything());

    confirm_spy.mockRestore();
  });

  // §17-5 — voice microphone permission row. The two cases
  // below mirror the per-item brief's "2 vitest cases on the
  // Settings row (granted / denied-styling)". The row reads
  // the OS permission state via `check_mic_permission_cmd`
  // on mount and renders the per-OS deep-link button only
  // when state == "denied".

  it("(d) voice row granted: state-granted class is applied and no deep-link button is shown", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "check_mic_permission_cmd")
        return Promise.resolve("granted");
      if (cmd === "mic_permission_deep_link_url_cmd")
        return Promise.resolve("pavucontrol:");
      return Promise.resolve(undefined);
    });

    render(Settings, { props: { onreset: () => {} } });

    const state = await screen.findByTestId("voice-permission-state");
    // Let the onMount microtask flush so the $state value
    // is the resolved `"granted"` string, not the initial
    // `undefined`.
    await new Promise((r) => setTimeout(r, 0));
    expect(state.className).toContain("state-granted");
    expect(state.textContent).toMatch(/Permission:\s*granted/);
    // The deep-link button must NOT appear in the granted
    // state — it's only there to unblock a denial.
    expect(screen.queryByTestId("settings-open-permission")).toBeNull();
    // The "Test microphone" button is always shown.
    expect(screen.getByTestId("settings-test-microphone")).toBeTruthy();
  });

  it("(e) voice row denied: state-denied class is applied and the Open-Privacy-Settings button is shown", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "check_mic_permission_cmd")
        return Promise.resolve("denied");
      if (cmd === "mic_permission_deep_link_url_cmd")
        return Promise.resolve("pavucontrol:");
      return Promise.resolve(undefined);
    });

    render(Settings, { props: { onreset: () => {} } });

    const state = await screen.findByTestId("voice-permission-state");
    await new Promise((r) => setTimeout(r, 0));
    // The red-bordered "denied" styling must be present so
    // the row stands out from the rest of Settings.
    expect(state.className).toContain("state-denied");
    expect(state.textContent).toMatch(/Permission:\s*denied/);
    // The deep-link button appears in the denied state —
    // it's how the user reaches the per-OS privacy pane.
    const btn = await screen.findByTestId("settings-open-permission");
    expect(btn).toBeTruthy();
    expect(btn.textContent).toMatch(/Open Privacy Settings/);
    // "Test microphone" is still present.
    expect(screen.getByTestId("settings-test-microphone")).toBeTruthy();
  });
});
