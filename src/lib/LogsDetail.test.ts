import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import LogsDetail from "./LogsDetail.svelte";

const sampleJson = {
  source: "github",
  captured_at: "2026-07-29T18:00:00Z",
  payload: { prs: [] },
};

beforeEach(() => {
  vi.clearAllMocks();
  // jsdom does not implement navigator.clipboard by default; the
  // component handles the rejection gracefully, but for the
  // clipboard test we install a stub explicitly so the assertion
  // fires reliably.
  Object.assign(navigator, {
    clipboard: {
      writeText: vi.fn().mockResolvedValue(undefined),
    },
  });
});

describe("LogsDetail", () => {
  it("renders the JSON pretty-printed inside a <pre>", () => {
    render(LogsDetail, { json: sampleJson });
    const pre = screen.getByTestId("logs-detail").querySelector("pre");
    expect(pre).toBeTruthy();
    const text = pre!.textContent ?? "";
    expect(text).toContain('"source": "github"');
    expect(text).toContain('"payload": {');
    // pretty-print must have produced multiple lines.
    expect(text.split("\n").length).toBeGreaterThan(1);
  });

  it("renders 'Copy' button before click, 'Copied!' after", async () => {
    const writeText = vi.fn().mockResolvedValueOnce(undefined);
    Object.assign(navigator, {
      clipboard: { writeText },
    });
    render(LogsDetail, { json: sampleJson });
    const button = screen.getByRole("button", { name: /copy/i });
    expect(button.textContent?.trim()).toBe("Copy");
    await fireEvent.click(button);
    expect(writeText).toHaveBeenCalledTimes(1);
    // The component flips to 'Copied!' on success.
    await vi.waitFor(() =>
      expect(button.textContent?.trim()).toBe("Copied!"),
    );
  });

  it("renders without throwing when json is null", () => {
    // The component must handle `json: null` from the parent
    // (the timeline sets rawJson to null on collapse / pre-fetch).
    render(LogsDetail, { json: null });
    const pre = screen.getByTestId("logs-detail").querySelector("pre");
    expect(pre).toBeTruthy();
    // JSON.stringify(null, null, 2) === "null" — the rendered text
    // must contain "null" verbatim.
    expect(pre!.textContent?.trim()).toBe("null");
  });
});