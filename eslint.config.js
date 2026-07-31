// ESLint 9 flat config. Minimal: lint .ts and .svelte files using only the
// deps that ship in package.json (no @eslint/js, no svelte-eslint-parser).
// Pre-flight gate (Phase 1 §5b carry-forward). Custom rules can be added
// later without breaking this scaffold.
import tsParser from "@typescript-eslint/parser";
import tsPlugin from "@typescript-eslint/eslint-plugin";

const tsRules = {
  ...(tsPlugin.configs?.recommended?.rules ?? {}),
};

export default [
  {
    files: ["**/*.ts"],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: 2022,
        sourceType: "module",
      },
    },
    plugins: {
      "@typescript-eslint": tsPlugin,
    },
    rules: tsRules,
  },
  {
    // ESLint's built-in recommended rules for .svelte (Svelte-specific
    // rules are deferred until eslint-plugin-svelte flat support is
    // available via svelte-eslint-parser, which isn't in package.json).
    ignores: ["**/*.svelte"],
  },
  {
    ignores: [
      "node_modules/**",
      "src-tauri/**",
      "target/**",
      "dist/**",
      "build/**",
      "**/*.d.ts",
      "*.config.*",
    ],
  },
];
