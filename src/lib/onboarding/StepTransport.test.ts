import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import StepTransport from "./StepTransport.svelte";

const { invoke_mock } = vi.hoisted(() => {
  return { invoke_mock: vi.fn() };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invoke_mock,
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

const MOCK_KEY_PATH = "/Users/test/.ssh/trail_ed25519";

beforeEach(() => {
  vi.clearAllMocks();
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "generate_ssh_key") return Promise.resolve(MOCK_KEY_PATH);
    return Promise.reject(new Error(`Unknown command: ${cmd}`));
  });
});

describe("StepTransport.svelte", () => {
  it("(a) Next button is disabled until host + user + port are valid AND a key is generated", () => {
    render(StepTransport, { props: { on_next: () => {} } });
    const next = screen.getByTestId("transport-next") as HTMLButtonElement;
    expect(next.disabled).toBe(true);
  });

  it("(b) host-empty shows the validation hint", async () => {
    render(StepTransport, { props: { on_next: () => {} } });
    // Initially the field is empty, so the hint should already
    // be visible (it's bound to the derived `host_valid` flag).
    expect(screen.getByTestId("host-error")).toBeTruthy();
  });

  it("(c) typing a host clears the host error", async () => {
    render(StepTransport, { props: { on_next: () => {} } });
    const host = screen.getByTestId("transport-host") as HTMLInputElement;
    await fireEvent.input(host, { target: { value: "vps.example.com" } });
    expect(screen.queryByTestId("host-error")).toBeNull();
  });

  it("(d) clicking Generate SSH key calls generate_ssh_key and shows the key path", async () => {
    render(StepTransport, { props: { on_next: () => {} } });
    const gen = screen.getByTestId("transport-generate-key") as HTMLButtonElement;
    await fireEvent.click(gen);
    expect(await screen.findByTestId("transport-key-path")).toBeTruthy();
    expect(mockInvoke).toHaveBeenCalledWith("generate_ssh_key");
  });

  it("(e) Next button enables once all inputs are valid + a key was generated", async () => {
    const on_next = vi.fn();
    render(StepTransport, { props: { on_next } });
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
    render(StepTransport, { props: { on_next: () => {} } });
    const port = screen.getByTestId("transport-port") as HTMLInputElement;
    // Force the port to an invalid value. The input is a
    // number, so we set the .value directly via the input
    // event.
    await fireEvent.input(port, { target: { value: "99999" } });
    expect(screen.getByTestId("port-error")).toBeTruthy();
  });
});
