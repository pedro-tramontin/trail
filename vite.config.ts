/// <reference types="vitest" />
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { fileURLToPath } from "node:url";
import path from "node:path";

// Svelte 5 detects jsdom as a "server" environment unless we explicitly
// hint it as browser via export conditions. The `browser` condition picks
// the client entry (which has working `mount()`), not the SSR `index-server.js`.
// Phase 1 §5b D3 carry-forward: this is REQUIRED for Svelte 5 runes to
// bundle correctly in the Tauri webview, not just for vitest.

// `__dirname` is undefined in ESM contexts. The package.json has
// `"type": "module"`, so this file is loaded as ESM and the
// pre-fix `__dirname` reference broke the dev server / vitest
// startup with `__dirname is not defined`. Derive the directory
// from `import.meta.url` instead. (PR #24 Copilot thread T4.)
const here = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  resolve: {
    conditions: ["browser"],
    alias: {
      $lib: path.resolve(here, "src/lib"),
    },
  },
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: process.env.TAURI_DEV_HOST || false,
    hmr: process.env.TAURI_DEV_HOST
      ? {
          protocol: "ws",
          host: process.env.TAURI_DEV_HOST,
          port: 1421,
        }
      : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**"] },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
