import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/svelte";

// @testing-library/svelte does NOT auto-unmount components in vitest
// (only Jest + globals-cleanup configured). Without this, each `render(Greet)`
// appends to document.body and the next `getByRole("button")` finds multiple.
afterEach(() => {
  cleanup();
});
