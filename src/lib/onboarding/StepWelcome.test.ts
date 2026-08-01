import { vi, describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import StepWelcome from "./StepWelcome.svelte";

describe("StepWelcome.svelte", () => {
  it("(a) renders the welcome copy and the Get started button", () => {
    render(StepWelcome, { props: { on_next: () => {} } });
    expect(screen.getByTestId("step-welcome")).toBeTruthy();
    expect(screen.getByText(/Welcome to Trail/)).toBeTruthy();
    expect(screen.getByTestId("welcome-next")).toBeTruthy();
  });

  it("(b) clicking Get started invokes on_next", async () => {
    const on_next = vi.fn();
    render(StepWelcome, { props: { on_next } });
    await fireEvent.click(screen.getByTestId("welcome-next"));
    expect(on_next).toHaveBeenCalledTimes(1);
  });

  it("(c) the Next button is always enabled on the welcome step (no inputs to validate)", () => {
    render(StepWelcome, { props: { on_next: () => {} } });
    const next = screen.getByTestId("welcome-next") as HTMLButtonElement;
    expect(next.disabled).toBe(false);
  });
});
