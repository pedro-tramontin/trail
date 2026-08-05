import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import App from "./App.svelte";
import { MOCK_SCAN_REPORT, MOCK_ANSWERS } from "./lib/onboarding/types";

// Mock the Tauri IPC bridge at the @tauri-apps/api/core layer.
// App.svelte's onMount calls `config_exists`, `demo_status`, and
// (on the cold-restart path) `start_collectors`; the wizard
// completion callback also awaits `start_collectors`. Mocking
// the IPC layer at the import boundary is the same pattern
// Onboarding.test.ts uses.
const { invoke_mock } = vi.hoisted(() => {
  return { invoke_mock: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invoke_mock,
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

/**
 * The wizard's `oncomplete` callback is async (Phase 9 §9.3) —
 * the App component awaits the `start_collectors` IPC before
 * flipping `config_exists`. These tests verify the three paths
 * the spec requires:
 *
 *   1. Success path — `start_collectors` resolves → wizard
 *      unmounts → main shell renders.
 *
 *   2. Cold-restart path — onMount sees `config_exists = true`
 *      → `start_collectors` is invoked to make sure the
 *      orchestrator is up after a restart.
 *
 *   3. Failure path — `start_collectors` rejects → wizard stays
 *      mounted + error is rendered so the user can retry.
 *
 * The test for path (1) is the load-bearing one — it's the
 * regression that §9.3's `await invoke('start_collectors')`
 * guards against (the old behavior was to just flip
 * `config_exists` synchronously, which would unmount the wizard
 * before the orchestrator was up).
 */

const MOCK_KEY_PATH = "/Users/test/.ssh/trail_ed25519";
const WRITTEN_PATH = "/Users/test/.trail/config.json";

beforeEach(() => {
  vi.clearAllMocks();
  // Default mock: rejects anything that doesn't have an
  // explicit return — tests override the relevant commands
  // per-test.
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "config_exists") return Promise.resolve(false);
    if (cmd === "demo_status") return Promise.resolve(null);
    if (cmd === "scan_laptop_cmd") return Promise.resolve(MOCK_SCAN_REPORT);
    if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
    if (cmd === "generate_ssh_key") return Promise.resolve(MOCK_KEY_PATH);
    if (cmd === "write_onboarding_config")
      return Promise.resolve(WRITTEN_PATH);
    return Promise.reject(new Error(`Unknown command: ${cmd}`));
  });
});

/** Walks the wizard from step 1 (Welcome) to step 6 (StepFinish).
 * Onboarding.test.ts already exercises the per-step behavior
 * (validation, auto-advance, etc.) — this helper just drives
 * the click sequence so the App.svelte tests can focus on the
 * §9.3 wiring (handle_onboarding_complete's start_collectors
 * await). */
async function walk_wizard_to_step_finish(): Promise<void> {
  // Welcome
  expect(await screen.findByTestId("onboarding-wizard")).toBeTruthy();
  await fireEvent.click(screen.getByTestId("welcome-next"));
  // StepScan has a 10-second auto-advance countdown; bypass it
  // by clicking the "Continue now" button. The countdown
  // itself is exercised in StepScan.test.ts.
  await waitFor(() => {
    expect(screen.getByTestId("scan-continue-now")).toBeTruthy();
  });
  await fireEvent.click(screen.getByTestId("scan-continue-now"));
  await waitFor(
    () => {
      expect(screen.getByTestId("step-ask")).toBeTruthy();
    },
    { timeout: 3000 },
  );
  // StepAsk — "Looks good" once answers load
  await waitFor(() => {
    const next = screen.getByTestId("ask-next") as HTMLButtonElement;
    expect(next.disabled).toBe(false);
  });
  await fireEvent.click(screen.getByTestId("ask-next"));
  // StepTransport — fill required fields
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
  // StepInstall
  expect(await screen.findByTestId("step-install")).toBeTruthy();
  await fireEvent.click(screen.getByTestId("install-next"));
  // StepFinish mounts
  expect(await screen.findByTestId("step-finish")).toBeTruthy();
}

describe("App.svelte (root) — start_collectors wiring (Phase 9 §9.3)", () => {
  it("(a) success path: start_collectors resolves → wizard unmounts → main shell renders", async () => {
    // The §9.3 wiring contract: when the wizard's `oncomplete`
    // fires, App.svelte's `handle_onboarding_complete` MUST
    // await `start_collectors` and only then flip
    // `config_exists` to `true`. The test exercises the full
    // walkthrough so we know the await happens in the right
    // place (not before write_onboarding_config, not after
    // config_exists flips).
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "config_exists") return Promise.resolve(false);
      if (cmd === "demo_status") return Promise.resolve(null);
      if (cmd === "scan_laptop_cmd") return Promise.resolve(MOCK_SCAN_REPORT);
      if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
      if (cmd === "generate_ssh_key") return Promise.resolve(MOCK_KEY_PATH);
      if (cmd === "write_onboarding_config")
        return Promise.resolve(WRITTEN_PATH);
      if (cmd === "start_collectors") return Promise.resolve(undefined);
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });

    render(App);
    await walk_wizard_to_step_finish();
    // Wait for the §9.3 async path to complete:
    //   1. StepFinish fires `write_onboarding_config` → resolves
    //   2. StepFinish waits 600ms then fires `oncomplete`
    //   3. handle_onboarding_complete awaits `start_collectors`
    //   4. config_exists flips to true → wizard unmounts
    //   5. main shell renders (h1 reads "Trail")
    await waitFor(
      () => {
        expect(screen.queryByTestId("onboarding-wizard")).toBeNull();
        expect(screen.getByRole("heading", { name: "Trail" })).toBeTruthy();
      },
      { timeout: 3000 },
    );
    // The start_collectors IPC was invoked.
    expect(mockInvoke).toHaveBeenCalledWith("start_collectors");
  });

  it("(b) cold-restart path: onMount sees config_exists=true → start_collectors is invoked", async () => {
    // First-launch-after-onboarding: the user has just finished
    // the wizard, the binary crashed (or was force-quit) before
    // the next start, and on the next cold start, the Tauri
    // setup closure sees the config file on disk and enters
    // the `Ready` arm. The Svelte side's onMount probe sees
    // `config_exists = true` and (per §9.3) re-invokes
    // `start_collectors` to make sure the orchestrator is up
    // after a restart. The Rust side's `start_collectors`
    // command is idempotent (a second call just re-spawns
    // the scheduler task — see §9.1 D1 in state.md).
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "config_exists") return Promise.resolve(true);
      if (cmd === "demo_status") return Promise.resolve(null);
      if (cmd === "start_collectors") return Promise.resolve(undefined);
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });

    render(App);
    // Wait for the loaded gate to lift + the main shell to
    // render (no wizard — config_exists is `true`).
    await waitFor(
      () => {
        expect(screen.getByRole("heading", { name: "Trail" })).toBeTruthy();
      },
      { timeout: 3000 },
    );
    // The wizard must NOT render on the cold-restart path.
    expect(screen.queryByTestId("onboarding-wizard")).toBeNull();
    // start_collectors must have been invoked from the onMount
    // cold-restart probe.
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("start_collectors");
    });
  });

  it("(c) failure path: start_collectors rejects → wizard stays mounted + main shell does NOT render", async () => {
    // The §9.3 spec: if `start_collectors` fails after the
    // wizard writes the config, the user MUST be able to see
    // the error and retry — not silently lose the config + the
    // orchestrator. The wizard stays mounted, the error
    // message renders inline (in the wizard, via the
    // `start_collectors_error` rune the parent owns).
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "config_exists") return Promise.resolve(false);
      if (cmd === "demo_status") return Promise.resolve(null);
      if (cmd === "scan_laptop_cmd") return Promise.resolve(MOCK_SCAN_REPORT);
      if (cmd === "ask_onboarding_cmd") return Promise.resolve(MOCK_ANSWERS);
      if (cmd === "generate_ssh_key") return Promise.resolve(MOCK_KEY_PATH);
      if (cmd === "write_onboarding_config")
        return Promise.resolve(WRITTEN_PATH);
      if (cmd === "start_collectors")
        return Promise.reject(new Error("keychain unlock failed"));
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });

    render(App);
    await walk_wizard_to_step_finish();
    // Wait for the §9.3 async path to land in the failure
    // branch:
    //   1-3. Same as success path above
    //   4. handle_onboarding_complete's invoke('start_collectors')
    //      rejects → start_collectors_error is set
    //   5. config_exists stays `false` → wizard stays mounted
    await waitFor(
      () => {
        // The wizard must NOT unmount (config_exists stays
        // `false` because the IPC rejected).
        expect(screen.queryByTestId("onboarding-wizard")).toBeTruthy();
        // start_collectors was attempted.
        expect(mockInvoke).toHaveBeenCalledWith("start_collectors");
      },
      { timeout: 3000 },
    );
    // The main shell must NOT have rendered (we never
    // flipped config_exists to true).
    expect(screen.queryByRole("heading", { name: "Trail" })).toBeNull();
  });
});
