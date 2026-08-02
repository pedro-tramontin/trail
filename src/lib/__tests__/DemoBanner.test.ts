import { render, screen } from "@testing-library/svelte";
import { describe, it, expect } from "vitest";
import DemoBanner from "../DemoBanner.svelte";

describe("DemoBanner.svelte", () => {
  it("renders nothing when is_demo is false", () => {
    render(DemoBanner, { is_demo: false });
    expect(screen.queryByTestId("demo-banner")).toBeNull();
  });

  it("renders the banner with the exact contract text when is_demo is true", () => {
    render(DemoBanner, { is_demo: true });
    const banner = screen.getByTestId("demo-banner");
    expect(banner).toBeInTheDocument();
    expect(banner.textContent).toContain(
      "Demo mode — no real captures. Go to Settings to set up real captures.",
    );
  });

  it("renders the banner with role='alert' for accessibility", () => {
    render(DemoBanner, { is_demo: true });
    const banner = screen.getByTestId("demo-banner");
    expect(banner.getAttribute("role")).toBe("alert");
  });
});
