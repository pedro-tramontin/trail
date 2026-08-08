import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import StepTransport from "./StepTransport.svelte";
import { writable, type Writable } from "svelte/store";
import type { StepTransportState } from "./types";

const { invoke_mock } = vi.hoisted(() => {
  return { invoke_mock: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invoke_mock,
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

const MOCK_KEY_PATH = "/Users/test/.ssh/trail_ed25519";

/** Build a fresh, default-valued StepTransportState store for
 *  the test cases. Mirrors the wizard root's initial state
 *  shape. */
function fresh_state(): Writable<StepTransportState> {
  return writable({
    host: "",
    user: "",
    port: 22,
    ssh_key_path: null,
    ssh_key_source: null,
    generating: false,
    key_error: null,
    test_state: "idle" as "idle" | "testing" | "ok" | "error",
    test_error: null,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "generate_ssh_key") return Promise.resolve(MOCK_KEY_PATH);
    return Promise.reject(new Error(`Unknown command: ${cmd}`));
  });
});

describe("StepTransport.svelte", () => {
  it("(a) Next button is disabled until host + user + port are valid AND a key is generated", () => {
    render(StepTransport, { props: { state: fresh_state(), on_next: () => {} } });
    const next = screen.getByTestId("transport-next") as HTMLButtonElement;
    expect(next.disabled).toBe(true);
  });

  it("(b) host-empty shows the validation hint", async () => {
    render(StepTransport, { props: { state: fresh_state(), on_next: () => {} } });
    // Initially the field is empty, so the hint should already
    // be visible (it's bound to the derived `host_valid` flag).
    expect(screen.getByTestId("host-error")).toBeTruthy();
  });

  it("(c) typing a host clears the host error", async () => {
    render(StepTransport, { props: { state: fresh_state(), on_next: () => {} } });
    const host = screen.getByTestId("transport-host") as HTMLInputElement;
    await fireEvent.input(host, { target: { value: "vps.example.com" } });
    expect(screen.queryByTestId("host-error")).toBeNull();
  });

  it("(d) clicking Generate SSH key calls generate_ssh_key and shows the key path", async () => {
    render(StepTransport, { props: { state: fresh_state(), on_next: () => {} } });
    const gen = screen.getByTestId("transport-generate-key") as HTMLButtonElement;
    await fireEvent.click(gen);
    expect(await screen.findByTestId("transport-key-path")).toBeTruthy();
    expect(mockInvoke).toHaveBeenCalledWith("generate_ssh_key");
  });

  it("(e) Next button enables once all inputs are valid + a key was generated", async () => {
    const on_next = vi.fn();
    render(StepTransport, { props: { state: fresh_state(), on_next } });
    // Fill the form
    await fireEvent.input(screen.getByTestId("transport-host"), {
      target: { value: "vps.example.com" },
    });
    await fireEvent.input(screen.getByTestId("transport-user"), {
      target: { value: "pedro" },
    });
    // Port default 22 is valid; don't touch it.
    // Generate the key.
    await fireEvent.click(screen.getByTestId("transport-generate-key"));
    // Wait for the next button to become enabled.
    await waitFor(() => {
      const next = screen.getByTestId("transport-next") as HTMLButtonElement;
      expect(next.disabled).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("transport-next"));
    expect(on_next).toHaveBeenCalledTimes(1);
  });

  it("(f) an out-of-range port shows the port validation error", async () => {
    render(StepTransport, { props: { state: fresh_state(), on_next: () => {} } });
    const port = screen.getByTestId("transport-port") as HTMLInputElement;
    // Force the port to an invalid value. The input is a
    // number, so we set the .value directly via the input
    // event.
    await fireEvent.input(port, { target: { value: "99999" } });
    expect(screen.getByTestId("port-error")).toBeTruthy();
  });

  // PR #193 — back-navigation preserves typed values.
  it("(g) the host / user / port values persist when the parent store is updated", async () => {
    // Simulate the "Back" navigation: the parent wizard has
    // already populated the store with values from a previous
    // mount. The fresh mount of StepTransport should read
    // those values from the store and render them in the
    // inputs.
    const pre_populated = writable({
      host: "vps.example.com",
      user: "pedro",
      port: 2222,
      ssh_key_path: MOCK_KEY_PATH,
      ssh_key_source: "generated" as "generated" | "existing" | null,
      generating: false,
      key_error: null,
      test_state: "ok" as "idle" | "testing" | "ok" | "error",
      test_error: null,
    });
    render(StepTransport, { props: { state: pre_populated, on_next: () => {} } });
    expect(
      (screen.getByTestId("transport-host") as HTMLInputElement).value,
    ).toBe("vps.example.com");
    expect(
      (screen.getByTestId("transport-user") as HTMLInputElement).value,
    ).toBe("pedro");
    expect(
      (screen.getByTestId("transport-port") as HTMLInputElement).value,
    ).toBe("2222");
    // Next is enabled because the store has all three + a key.
    expect(
      (screen.getByTestId("transport-next") as HTMLButtonElement).disabled,
    ).toBe(false);
    // The key-path display should also persist.
    expect(screen.getByTestId("transport-key-path")).toBeTruthy();
    // The test-result is also persisted.
    expect(screen.getByTestId("transport-test-ok")).toBeTruthy();
  });

  // PR #193 — "Use existing key" path.
  it("(h) clicking 'Use existing key' adopts the keychain public key", async () => {
    const MOCK_PUB = "ssh-ed25519 AAAAC3... existing@example.com";
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "get_ssh_public_key") return Promise.resolve(MOCK_PUB);
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    const state = fresh_state();
    render(StepTransport, { props: { state, on_next: () => {} } });
    await fireEvent.click(screen.getByTestId("transport-use-existing-key"));
    // After click: the key path is set, the source is "existing".
    expect(await screen.findByTestId("transport-key-path")).toBeTruthy();
    let resolved: StepTransportState | undefined;
    state.subscribe((s) => (resolved = s))();
    expect(resolved!.ssh_key_path).toBe(MOCK_PUB);
    expect(resolved!.ssh_key_source).toBe("existing");
  });

  it("(i) 'Use existing key' surfaces an error when no key is in the keychain", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "get_ssh_public_key") return Promise.resolve(null);
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    const state = fresh_state();
    render(StepTransport, { props: { state, on_next: () => {} } });
    await fireEvent.click(screen.getByTestId("transport-use-existing-key"));
    // No key path is set; an error hint is shown.
    expect(await screen.findByTestId("transport-key-error")).toBeTruthy();
    expect(screen.queryByTestId("transport-key-path")).toBeNull();
    let resolved: StepTransportState | undefined;
    state.subscribe((s) => (resolved = s))();
    expect(resolved!.ssh_key_path).toBeNull();
  });

  // PR #193 — "Test connection" path.
  it("(j) clicking 'Test connection' with valid form fires test_ssh_connection and shows 'Connected' on success", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "generate_ssh_key") return Promise.resolve(MOCK_KEY_PATH);
      if (cmd === "test_ssh_connection") return Promise.resolve(undefined);
      return Promise.reject(new Error(`Unknown command: ${cmd}`));
    });
    const state = fresh_state();
    render(StepTransport, { props: { state, on_next: () => {} } });
    await fireEvent.input(screen.getByTestId("transport-host"), {
      target: { value: "vps.example.com" },
    });
    await fireEvent.input(screen.getByTestId("transport-user"), {
      target: { value: "pedro" },
    });
    await fireEvent.click(screen.getByTestId("transport-generate-key"));
    await waitFor(() => {
      expect(
        (screen.getByTestId("transport-next") as HTMLButtonElement).disabled,
      ).toBe(false);
    });
    await fireEvent.click(screen.getByTestId("transport-test-connection"));
    // The success test-id appears.
    expect(await screen.findByTestId("transport-test-ok")).toBeTruthy();
    expect(mockInvoke).toHaveBeenCalledWith("test_ssh_connection", {
      host: "vps.example.com",
      port: 22,
      user: "pedro",
    });
  });
});
