import { render, fireEvent, screen } from "@testing-library/svelte";
import { vi, describe, it, expect, beforeEach } from "vitest";
import Greet from "./Greet.svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = vi.mocked(invoke);

describe("Greet.svelte", () => {
  beforeEach(() => mockInvoke.mockReset());

  it("renders the input and button", () => {
    render(Greet);
    expect(screen.getByPlaceholderText("Enter a name")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Greet" })).toBeInTheDocument();
  });

  it("calls invoke('greet', { name: '...' }) on button click", async () => {
    mockInvoke.mockResolvedValue("Hello, test! You've been greeted from Rust.");
    render(Greet);
    const input = screen.getByPlaceholderText("Enter a name");
    await fireEvent.input(input, { target: { value: "test" } });
    await fireEvent.click(screen.getByRole("button", { name: "Greet" }));
    expect(mockInvoke).toHaveBeenCalledWith("greet", { name: "test" });
  });

  it("uses 'world' as default name when input is empty", async () => {
    mockInvoke.mockResolvedValue("Hello, world!");
    render(Greet);
    await fireEvent.click(screen.getByRole("button", { name: "Greet" }));
    expect(mockInvoke).toHaveBeenCalledWith("greet", { name: "world" });
  });

  it("renders the returned greeting after invoke resolves", async () => {
    mockInvoke.mockResolvedValue("Hello, test!");
    render(Greet);
    const input = screen.getByPlaceholderText("Enter a name");
    await fireEvent.input(input, { target: { value: "test" } });
    await fireEvent.click(screen.getByRole("button", { name: "Greet" }));
    expect(await screen.findByText("Hello, test!")).toBeInTheDocument();
  });
});
