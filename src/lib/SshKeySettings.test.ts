import { vi, describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import SshKeySettings from "./SshKeySettings.svelte";
import type { KeyringHint } from "$lib/api/keyring";

// Mock the Tauri IPC bridge. The component fires 3 IPCs on
// mount in parallel: `keyring_hint`, `credential_store_name`,
// and `get_ssh_public_key`. The test cases below resolve
// each to the per-state fixture needed for that branch.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

// `navigator.clipboard.writeText` is jsdom-undefined; the
// "Copy public key" button's handler relies on it for the
// transient "Copied!" indicator. Stub the writeText so the
// KeyPair test case can click the button without a
// NotImplementedError from jsdom.
const writeTextMock = vi.fn(async () => undefined);
Object.defineProperty(navigator, "clipboard", {
  configurable: true,
  value: { writeText: writeTextMock },
});

beforeEach(() => {
  vi.clearAllMocks();
  writeTextMock.mockClear();
  // Default IPC behaviour: keyring is healthy + a keypair is
  // present + the per-OS store is "secret-service" (Linux).
  // Each test re-`mockImplementation`s the relevant command.
  mockInvoke.mockImplementation(async (cmd: string) => {
    if (cmd === "keyring_hint") {
      return { kind: "key_pair" } satisfies KeyringHint;
    }
    if (cmd === "credential_store_name") {
      return "secret-service / GNOME Keyring / KWallet";
    }
    if (cmd === "get_ssh_public_key") {
      return "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFIX";
    }
    return null;
  });
});

describe("SshKeySettings.svelte — 4 KeyringHint states", () => {
  it("(a) empty state renders 'No SSH key yet' + a Generate button", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "keyring_hint") return { kind: "empty" } satisfies KeyringHint;
      if (cmd === "credential_store_name") return "Keychain";
      if (cmd === "get_ssh_public_key") return null;
      return null;
    });
    render(SshKeySettings);

    // The empty branch mounts with the `ssh-key-settings-empty`
    // test id + the "Generate SSH key" button.
    const empty = await screen.findByTestId("ssh-key-settings-empty");
    expect(empty).toBeTruthy();
    expect(empty.textContent).toMatch(/No SSH key yet/);
    expect(empty.textContent).toMatch(/Keychain/);
    const generate = screen.getByTestId("ssh-key-settings-generate");
    expect(generate).toBeTruthy();
    expect(generate.textContent).toMatch(/Generate SSH key/);
    // The other branches must NOT be in the tree.
    expect(screen.queryByTestId("ssh-key-settings-key-pair")).toBeNull();
    expect(screen.queryByTestId("ssh-key-settings-public-only")).toBeNull();
    expect(screen.queryByTestId("ssh-key-settings-unavailable")).toBeNull();
  });

  it("(b) public_only state renders the recovery row + Re-generate button", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "keyring_hint") return { kind: "public_only" } satisfies KeyringHint;
      if (cmd === "credential_store_name") return "Credential Manager";
      // The half-state has a public key on disk but no
      // private key — `get_ssh_public_key` still returns the
      // public key string (derived from the PEM on disk).
      if (cmd === "get_ssh_public_key") return "ssh-ed25519 AAAAC3PUBLIC";
      return null;
    });
    render(SshKeySettings);

    const row = await screen.findByTestId("ssh-key-settings-public-only");
    expect(row).toBeTruthy();
    expect(row.textContent).toMatch(/public key is stored/i);
    expect(row.textContent).toMatch(/private key is missing/i);
    expect(row.textContent).toMatch(/Credential Manager/);
    // Re-generate button (distinct from the empty-state "Generate SSH key"
    // label — the public-only state uses "Re-generate SSH key").
    const regen = screen.getByTestId("ssh-key-settings-regenerate");
    expect(regen).toBeTruthy();
    expect(regen.textContent).toMatch(/Re-generate/);
    // The Copy button only exists in the KeyPair state.
    expect(screen.queryByTestId("ssh-key-settings-copy")).toBeNull();
  });

  it("(c) key_pair state renders the success row + Copy + Regenerate buttons", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "keyring_hint") return { kind: "key_pair" } satisfies KeyringHint;
      if (cmd === "credential_store_name") return "Keychain";
      if (cmd === "get_ssh_public_key") return "ssh-ed25519 AAAAC3NzaKEYPAIR";
      return null;
    });
    render(SshKeySettings);

    const row = await screen.findByTestId("ssh-key-settings-key-pair");
    expect(row).toBeTruthy();
    expect(row.textContent).toMatch(/Your SSH key is stored/);
    expect(row.textContent).toMatch(/Keychain/);

    // Both action buttons must be present.
    const copy_btn = screen.getByTestId("ssh-key-settings-copy");
    expect(copy_btn).toBeTruthy();
    expect(copy_btn.textContent).toMatch(/Copy public key/);
    const regen = screen.getByTestId("ssh-key-settings-regenerate");
    expect(regen).toBeTruthy();
    expect(regen.textContent).toMatch(/Regenerate/);

    // Clicking Copy invokes the clipboard write with the
    // public key bytes — verifies the button is wired
    // (jsdom's navigator.clipboard.writeText is stubbed at
    // the top of this file).
    await fireEvent.click(copy_btn);
    expect(writeTextMock).toHaveBeenCalledWith("ssh-ed25519 AAAAC3NzaKEYPAIR");
  });

  it("(d) unavailable state renders the labeled fallback + Retry button", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "keyring_hint")
        return {
          kind: "unavailable",
          reason: "keychain get_password failed: dbus daemon not running",
        } satisfies KeyringHint;
      // `credential_store_name` defaults to the generic
      // label when the keychain is unreachable — the panel
      // header should still render sensibly.
      if (cmd === "credential_store_name") return "OS credential store";
      if (cmd === "get_ssh_public_key") return null;
      return null;
    });
    render(SshKeySettings);

    const row = await screen.findByTestId("ssh-key-settings-unavailable");
    expect(row).toBeTruthy();
    expect(row.textContent).toMatch(/OS credential store is unavailable/i);
    // The reason must be visible to the user (in a <code>
    // block) — the spec requires the labeled fallback
    // message so the user can debug without a dev console.
    const reason = screen.getByTestId("ssh-key-settings-reason");
    expect(reason).toBeTruthy();
    expect(reason.textContent).toMatch(/dbus daemon not running/);
    // The retry button is the only action in the
    // unavailable state — no Generate / Regenerate / Copy.
    const retry = screen.getByTestId("ssh-key-settings-retry");
    expect(retry).toBeTruthy();
    expect(screen.queryByTestId("ssh-key-settings-copy")).toBeNull();
    // Clicking Retry re-fires `keyring_hint` so the user can
    // recover without a panel remount.
    await fireEvent.click(retry);
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith("keyring_hint"),
    );
  });
});
