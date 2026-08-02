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
  // jsdom defaults `window.confirm` to `false`; tests opt in
  // explicitly via `vi.spyOn(window, "confirm").mockReturnValue(true)`.
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
    mockInvoke.mockResolvedValue(undefined);

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
    mockInvoke.mockResolvedValue(undefined);

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
    // And therefore no IPC call.
    expect(mockInvoke).not.toHaveBeenCalled();

    confirm_spy.mockRestore();
  });
});
