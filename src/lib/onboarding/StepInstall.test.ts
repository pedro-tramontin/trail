import { vi, describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import StepInstall from "./StepInstall.svelte";

describe("StepInstall.svelte", () => {
  it("(a) defaults to 'auto' and exposes 3 mutually-exclusive radios", () => {
    render(StepInstall, { props: { on_next: () => {} } });
    const options = screen.getByTestId("install-options");
    const radios = options.querySelectorAll(
      'input[type="radio"]',
    ) as NodeListOf<HTMLInputElement>;
    expect(radios.length).toBe(3);
    // Default value: 'auto' is checked, the other two are not.
    expect(radios[0].value).toBe("auto");
    expect(radios[0].checked).toBe(true);
    expect(radios[1].checked).toBe(false);
    expect(radios[2].checked).toBe(false);
  });

  it("(b) clicking 'show_script' checks only that radio", async () => {
    render(StepInstall, { props: { on_next: () => {} } });
    const show_script_label = screen.getByTestId("install-option-show-script");
    const show_script_radio = show_script_label.querySelector(
      'input[type="radio"]',
    ) as HTMLInputElement;
    await fireEvent.click(show_script_radio);
    expect(show_script_radio.checked).toBe(true);
    const auto_radio = (
      screen
        .getByTestId("install-option-auto")
        .querySelector('input[type="radio"]') as HTMLInputElement
    );
    expect(auto_radio.checked).toBe(false);
  });

  it("(c) clicking Next invokes on_next with the currently-selected choice", async () => {
    const on_next = vi.fn();
    render(StepInstall, { props: { on_next } });
    const skip_label = screen.getByTestId("install-option-skip");
    const skip_radio = skip_label.querySelector(
      'input[type="radio"]',
    ) as HTMLInputElement;
    await fireEvent.click(skip_radio);
    await fireEvent.click(screen.getByTestId("install-next"));
    expect(on_next).toHaveBeenCalledWith("skip");
  });

  it("(d) Next button is enabled on mount (one radio is always selected)", () => {
    render(StepInstall, { props: { on_next: () => {} } });
    const next = screen.getByTestId("install-next") as HTMLButtonElement;
    expect(next.disabled).toBe(false);
  });
});
