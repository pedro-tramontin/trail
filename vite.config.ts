/// <reference types="vitest" />
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import path from "node:path";

// Svelte 5 detects jsdom as a "server" environment unless we explicitly
// hint it as browser via export conditions. The `browser` condition picks
// the client entry (which has working `mount()`), not the SSR `index-server.js`.
// Phase 1 §5b D3 carry-forward: this is REQUIRED for Svelte 5 runes to
// bundle correctly in the Tauri webview, not just for vitest.
export default defineConfig({
  resolve: {
    conditions: ["browser"],
    alias: {
      $lib: path.resolve(__dirname, "src/lib"),
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
