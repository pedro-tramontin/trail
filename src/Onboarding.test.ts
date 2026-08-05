import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import Onboarding from "./Onboarding.svelte";
import { MOCK_SCAN_REPORT, MOCK_ANSWERS } from "./lib/onboarding/types";

const { invoke_mock } = vi.hoisted(() => {
  return { invoke_mock: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invoke_mock,
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

const MOCK_KEY_PATH = "/Users/test/.ssh/trail_ed25519";
const WRITTEN_PATH = "/Users/test/.trail/config.json";

beforeEach(() => {
  vi.clearAllMocks();
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "scan_laptop_cmd") return Promise.resolve(MOCK_SCAN_REPORT);
    if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
    if (cmd === "generate_ssh_key") return Promise.resolve(MOCK_KEY_PATH);
    if (cmd === "write_onboarding_config")
      return Promise.resolve(WRITTEN_PATH);
    return Promise.reject(new Error(`Unknown command: ${cmd}`));
  });
});

/**
 * Skip StepScan's 10-second auto-advance countdown by clicking
 * the "Continue now" button. The wizard then lands on StepAsk.
 *
 * Used by the tests below to walk through the flow quickly
 * without waiting on the real countdown (which exists so end
 * users have time to read the scan findings). The countdown
 * behavior itself is covered by StepScan.test.ts.
 */
async function skip_scan_countdown(): Promise<void> {
  await waitFor(() => {
    expect(screen.getByTestId("scan-continue-now")).toBeTruthy();
  });
  await fireEvent.click(screen.getByTestId("scan-continue-now"));
  await waitFor(() => {
    expect(screen.getByTestId("step-ask")).toBeTruthy();
  });
}

describe("Onboarding.svelte (root)", () => {
  it("(a) mounts and shows step 1 (Welcome) with a 'Get started' button", () => {
    render(Onboarding, { props: { oncomplete: () => {} } });
    expect(screen.getByTestId("onboarding-wizard")).toBeTruthy();
    expect(screen.getByTestId("step-welcome")).toBeTruthy();
    expect(screen.getByTestId("welcome-next")).toBeTruthy();
    // Step indicator reads "Step 1 of 6".
    expect(screen.getByTestId("step-indicator").textContent).toMatch(
      /Step 1 of 6/,
    );
  });

  it("(b) advances to StepScan when the welcome Next button is clicked", async () => {
    render(Onboarding, { props: { oncomplete: () => {} } });
    await fireEvent.click(screen.getByTestId("welcome-next"));
    // StepScan mounts and immediately fires scan_laptop_cmd.
    // We don't wait for the auto-advance — we just check the
    // scan step rendered.
    expect(
      await screen.findByTestId("step-scan"),
    ).toBeTruthy();
    // The step indicator should now read "Step 2 of 6".
    expect(screen.getByTestId("step-indicator").textContent).toMatch(
      /Step 2 of 6/,
    );
  });

  it("(c) Continue-now button on StepScan advances to StepAsk", async () => {
    // The auto-advance countdown itself is covered in
    // StepScan.test.ts. Here we just verify the wizard routes
    // through StepScan → StepAsk via the user-facing control.
    render(Onboarding, { props: { oncomplete: () => {} } });
    await fireEvent.click(screen.getByTestId("welcome-next"));
    await skip_scan_countdown();
    expect(screen.getByTestId("step-indicator").textContent).toMatch(
      /Step 3 of 6/,
    );
  });

  it("(d) Back button returns to the previous step with state preserved", async () => {
    render(Onboarding, { props: { oncomplete: () => {} } });
    // Advance to step 2 (scan).
    await fireEvent.click(screen.getByTestId("welcome-next"));
    expect(await screen.findByTestId("step-scan")).toBeTruthy();
    // Click Back — should be back to step 1 (welcome).
    expect(screen.getByTestId("wizard-back")).toBeTruthy();
    await fireEvent.click(screen.getByTestId("wizard-back"));
    expect(screen.getByTestId("step-welcome")).toBeTruthy();
    expect(screen.getByTestId("step-indicator").textContent).toMatch(
      /Step 1 of 6/,
    );
  });

  it("(e) StepTransport shows the per-field validation errors when fields are empty", async () => {
    render(Onboarding, { props: { oncomplete: () => {} } });
    // Skip to step 4 (transport) by walking through the wizard
    // quickly. We bypass auto-advance by clicking Continue now
    // on StepScan and "Looks good" immediately when StepAsk
    // renders.
    await fireEvent.click(screen.getByTestId("welcome-next"));
    await skip_scan_countdown();
    await waitFor(() => {
      const next = screen.getByTestId("ask-next") as HTMLButtonElement;
      expect(next.disabled).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("ask-next"));
    // Now on StepTransport.
    expect(await screen.findByTestId("step-transport")).toBeTruthy();
    expect(screen.getByTestId("host-error")).toBeTruthy();
    expect(screen.getByTestId("user-error")).toBeTruthy();
  });

  it("(f) StepInstall renders 3 radio options after StepTransport advances", async () => {
    render(Onboarding, { props: { oncomplete: () => {} } });
    // Welcome → Scan → Ask → Transport
    await fireEvent.click(screen.getByTestId("welcome-next"));
    await skip_scan_countdown();
    await waitFor(() => {
      const next = screen.getByTestId("ask-next") as HTMLButtonElement;
      expect(next.disabled).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("ask-next"));
    // Transport
    expect(await screen.findByTestId("step-transport")).toBeTruthy();
    // Fill in + generate key
    await fireEvent.input(screen.getByTestId("transport-host"), {
      target: { value: "vps.example.com" },
    });
    await fireEvent.input(screen.getByTestId("transport-user"), {
      target: { value: "pedro" },
    });
    await fireEvent.click(screen.getByTestId("transport-generate-key"));
    await waitFor(() => {
      const next = screen.getByTestId("transport-next") as HTMLButtonElement;
      expect(next.disabled).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("transport-next"));
    // Install
    expect(await screen.findByTestId("step-install")).toBeTruthy();
    const radios = screen
      .getByTestId("install-options")
      .querySelectorAll('input[type="radio"]');
    expect(radios.length).toBe(3);
  });

  it("(g) the full walkthrough eventually calls oncomplete with the config path", async () => {
    const oncomplete = vi.fn();
    render(Onboarding, { props: { oncomplete } });
    await fireEvent.click(screen.getByTestId("welcome-next"));
    await skip_scan_countdown();
    await waitFor(() => {
      const next = screen.getByTestId("ask-next") as HTMLButtonElement;
      expect(next.disabled).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("ask-next"));
    expect(await screen.findByTestId("step-transport")).toBeTruthy();
    await fireEvent.input(screen.getByTestId("transport-host"), {
      target: { value: "vps.example.com" },
    });
    await fireEvent.input(screen.getByTestId("transport-user"), {
      target: { value: "pedro" },
    });
    await fireEvent.click(screen.getByTestId("transport-generate-key"));
    await waitFor(() => {
      const next = screen.getByTestId("transport-next") as HTMLButtonElement;
      expect(next.disabled).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("transport-next"));
    expect(await screen.findByTestId("step-install")).toBeTruthy();
    await fireEvent.click(screen.getByTestId("install-next"));
    // StepFinish mounts, calls write_onboarding_config,
    // succeeds, and after 600ms calls oncomplete.
    await waitFor(
      () => {
        expect(oncomplete).toHaveBeenCalledWith(WRITTEN_PATH);
      },
      { timeout: 3000 },
    );
    // write_onboarding_config should have been called exactly
    // once, with the LLM's answers (translated to the user's
    // local-time `hour_utc`) and the SSH key flag set.
    const offset_minutes = new Date().getTimezoneOffset();
    const offset_hours = offset_minutes / 60;
    const expected_hour_utc = ((18 - offset_hours) + 24) % 24;
    expect(mockInvoke).toHaveBeenCalledWith(
      "write_onboarding_config",
      expect.objectContaining({
        answers: expect.objectContaining({
          ...MOCK_ANSWERS,
          review_time: {
            ...MOCK_ANSWERS.review_time,
            hour_utc: expected_hour_utc,
          },
        }),
        sshKeyGenerated: true,
      }),
    );
  });

  it("(h) the Back button is not shown on the first step (no-op edge case)", () => {
    render(Onboarding, { props: { oncomplete: () => {} } });
    expect(screen.queryByTestId("wizard-back")).toBeNull();
  });

  it("(i) the Back button is not shown on the last step (StepFinish)", async () => {
    const oncomplete = vi.fn();
    render(Onboarding, { props: { oncomplete } });
    // Welcome → Scan → Ask (jump straight to "Looks good" once
    // the answers load).
    await fireEvent.click(screen.getByTestId("welcome-next"));
    await skip_scan_countdown();
    await waitFor(() => {
      const next = screen.getByTestId("ask-next") as HTMLButtonElement;
      expect(next.disabled).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("ask-next"));
    // Transport
    expect(await screen.findByTestId("step-transport")).toBeTruthy();
    await fireEvent.input(screen.getByTestId("transport-host"), {
      target: { value: "vps.example.com" },
    });
    await fireEvent.input(screen.getByTestId("transport-user"), {
      target: { value: "pedro" },
    });
    await fireEvent.click(screen.getByTestId("transport-generate-key"));
    await waitFor(() => {
      const next = screen.getByTestId("transport-next") as HTMLButtonElement;
      expect(next.disabled).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("transport-next"));
    expect(await screen.findByTestId("step-install")).toBeTruthy();
    await fireEvent.click(screen.getByTestId("install-next"));
    expect(await screen.findByTestId("step-finish")).toBeTruthy();
    expect(screen.queryByTestId("wizard-back")).toBeNull();
  });
});