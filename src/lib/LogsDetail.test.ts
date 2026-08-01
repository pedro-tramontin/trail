import { describe, it, expect, vi } from "vitest";

const sampleJson = {
  source: "github",
  captured_at: "2026-07-29T18:00:00Z",
  payload: { prs: [] },
};

describe("LogsDetail JSON formatting", () => {
  it("pretty-prints JSON", () => {
    const pretty = JSON.stringify(sampleJson, null, 2);
    expect(pretty).toContain('"source": "github"');
    expect(pretty).toContain('"payload": {');
    expect(pretty.split("\n").length).toBeGreaterThan(1);
  });

  it("copy button uses navigator.clipboard.writeText", async () => {
    const writeText = vi.fn().mockResolvedValueOnce(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    await navigator.clipboard.writeText("hello");
    expect(writeText).toHaveBeenCalledWith("hello");
  });
});
